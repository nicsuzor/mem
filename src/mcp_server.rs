//! MCP server for PKB semantic search + task graph.
//!
//! Implements rmcp 0.1.5 ServerHandler trait manually with tool dispatch.
//! Provides 18 tools for search, documents, tasks, and knowledge graph.

use crate::embeddings::Embedder;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use parking_lot::RwLock;
use rmcp::model::*;
use rmcp::{Error as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// =============================================================================
// MCP SERVER
// =============================================================================

#[derive(Clone)]
pub struct PkbSearchServer {
    store: Arc<RwLock<VectorStore>>,
    embedder: Arc<Embedder>,
    pkb_root: PathBuf,
    db_path: PathBuf,
    graph: Arc<RwLock<GraphStore>>,
}

impl PkbSearchServer {
    pub fn new(
        store: Arc<RwLock<VectorStore>>,
        embedder: Arc<Embedder>,
        pkb_root: PathBuf,
        db_path: PathBuf,
        graph: Arc<RwLock<GraphStore>>,
    ) -> Self {
        Self {
            store,
            embedder,
            pkb_root,
            db_path,
            graph,
        }
    }

    fn resolve_path(&self, path_str: &str) -> PathBuf {
        let path = Path::new(path_str);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.pkb_root.join(path)
        }
    }

    /// Reconstruct an absolute path from a (possibly relative) graph node path.
    fn abs_path(&self, rel: &Path) -> PathBuf {
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.pkb_root.join(rel)
        }
    }

    fn args_to_value(args: Option<JsonObject>) -> JsonValue {
        match args {
            Some(map) => JsonValue::Object(map),
            None => JsonValue::Object(serde_json::Map::new()),
        }
    }

    /// Rebuild the graph store (e.g. after CRUD operations) and persist to disk
    fn rebuild_graph(&self) {
        let new_graph = GraphStore::build_from_directory(&self.pkb_root);
        let graph_path = self.db_path.with_extension("graph.json");
        if let Err(e) = new_graph.save(&graph_path) {
            tracing::error!("Failed to save graph: {e}");
        }
        *self.graph.write() = new_graph;
    }

    // =========================================================================
    // SEARCH & DOCUMENT TOOLS
    // =========================================================================

    fn handle_get_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: path"),
                data: None,
            })?;

        let path = self.resolve_path(path_str);

        if !path.exists() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("File not found: {}", path.display())),
                data: None,
            });
        }

        let content = std::fs::read_to_string(&path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read file: {e}")),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "## {}\n\n{}",
            path.display(),
            content
        ))]))
    }

    fn handle_list_documents(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let tag = args.get("tag").and_then(|v| v.as_str());
        let doc_type = args.get("type").and_then(|v| v.as_str());
        let status = args.get("status").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let store = self.store.read();
        let results = store.list_documents(tag, doc_type, status, project, &self.pkb_root);
        let total = results.len();

        if total == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No documents found matching filters.",
            )]));
        }

        let page: Vec<_> = results
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(total))
            .collect();
        let showing = page.len();

        let mut output =
            format!("**{total} documents found** (showing {showing}, offset {offset})\n\n");

        for r in &page {
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            output.push_str(&format!(" — `{}`", r.path.display()));
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_reindex(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let (indexed, removed, total) = crate::index_pkb(
            &self.pkb_root,
            &self.db_path,
            &self.store,
            &self.embedder,
            force,
        );

        // Save vector store
        if let Err(e) = self.store.read().save(&self.db_path) {
            tracing::error!("Failed to save vector store: {e}");
        }

        // Rebuild graph
        self.rebuild_graph();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Reindex complete: {indexed} indexed, {removed} removed, {total} total documents."
        ))]))
    }

    // =========================================================================
    // GRAPH/TASK TOOLS (7)
    // =========================================================================

    fn handle_task_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let project = args.get("project").and_then(|v| v.as_str());

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = if project.is_some() {
            limit * 10
        } else {
            limit * 3
        };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root);

        let graph = self.graph.read();

        // Build path -> node index for O(1) lookups instead of O(n) per result
        let path_map: std::collections::HashMap<String, &crate::graph::GraphNode> = graph
            .nodes()
            .map(|n| (self.abs_path(&n.path).to_string_lossy().to_string(), n))
            .collect();

        let mut output = String::new();
        let mut count = 0;

        for r in &results {
            if count >= limit {
                break;
            }
            let is_task = r.doc_type.as_deref() == Some("task")
                || r.doc_type.as_deref() == Some("project")
                || r.doc_type.as_deref() == Some("goal");

            if !is_task {
                continue;
            }

            if let Some(proj) = project {
                if !r
                    .project
                    .as_deref()
                    .map(|p| p.eq_ignore_ascii_case(proj))
                    .unwrap_or(false)
                {
                    continue;
                }
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                count, r.title, r.score
            ));
            output.push_str(&format!("**Path:** `{}`\n", r.path.display()));

            // O(1) path lookup via pre-built index
            let path_str = r.path.to_string_lossy();
            if let Some(node) = path_map.get(&*path_str) {
                if let Some(ref s) = node.status {
                    output.push_str(&format!("**Status:** {s}\n"));
                }
                if let Some(p) = node.priority {
                    output.push_str(&format!("**Priority:** {p}\n"));
                }
                if !node.blocks.is_empty() {
                    output.push_str(&format!("**Blocks:** {}\n", node.blocks.join(", ")));
                }
                if !node.depends_on.is_empty() {
                    output.push_str(&format!("**Depends on:** {}\n", node.depends_on.join(", ")));
                }
            }
            output.push('\n');
        }

        if count == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No tasks found matching query.",
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "**Found {count} tasks for:** \"{query}\"\n\n{output}"
        ))]))
    }

    fn handle_get_network_metrics(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.get_node(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Node not found: {id}")),
            data: None,
        })?;

        let node_ids: Vec<String> = graph.nodes().map(|n| n.id.clone()).collect();
        let edges = graph.edges();

        let m = crate::metrics::compute_network_metrics(
            id,
            &node_ids,
            edges,
            node.downstream_weight,
            node.stakeholder_exposure,
        );

        match m {
            Some(metrics) => {
                let json = serde_json::to_string_pretty(&metrics).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "## Network metrics for {id}\n\n```json\n{json}\n```"
                ))]))
            }
            None => Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from("Failed to compute metrics"),
                data: None,
            }),
        }
    }

    fn handle_create_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let fields = crate::document_crud::TaskFields {
            title: title.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            parent: args
                .get("parent")
                .and_then(|v| v.as_str())
                .map(String::from),
            priority: args
                .get("priority")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            project: args
                .get("project")
                .and_then(|v| v.as_str())
                .map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            depends_on: args
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            assignee: args
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(String::from),
            complexity: args
                .get("complexity")
                .and_then(|v| v.as_str())
                .map(String::from),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
        };

        // Hierarchy validation: warn if task type should have a parent
        let mut warnings = Vec::new();
        if fields.parent.is_none() {
            // create_task always creates type "task", which should have a parent
            warnings.push(
                "Hierarchy warning: Task type 'task' should have a parent. \
                 Only goal, learn, and project types can be root-level. \
                 Consider assigning a parent to maintain graph hierarchy."
                    .to_string(),
            );
        }

        let path =
            crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create task: {e}")),
                data: None,
            })?;

        // Index the new file (with relative path for portable storage)
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        // Rebuild graph
        self.rebuild_graph();

        let mut msg = format!("Task created: `{}`", path.display());
        if !warnings.is_empty() {
            msg.push_str("\n\nHierarchy warnings:\n");
            for w in &warnings {
                msg.push_str(&format!("- {}\n", w));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    // =========================================================================
    // KNOWLEDGE GRAPH TOOLS (4)
    // =========================================================================

    fn handle_pkb_context(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let hops = args.get("hops").and_then(|v| v.as_u64()).unwrap_or(2) as usize;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Node not found: {id}")),
            data: None,
        })?;

        let node_id = node.id.clone();
        let mut output = format!("## {} — {}\n\n", node_id, node.label);
        output.push_str(&format!(
            "**Path:** `{}`\n",
            self.abs_path(&node.path).display()
        ));

        if let Some(ref t) = node.node_type {
            output.push_str(&format!("**Type:** {t}\n"));
        }
        if let Some(ref s) = node.status {
            output.push_str(&format!("**Status:** {s}\n"));
        }
        if let Some(p) = node.priority {
            output.push_str(&format!("**Priority:** {p}\n"));
        }
        if let Some(ref proj) = node.project {
            output.push_str(&format!("**Project:** {proj}\n"));
        }
        if let Some(ref due) = node.due {
            output.push_str(&format!("**Due:** {due}\n"));
        }
        if !node.tags.is_empty() {
            output.push_str(&format!("**Tags:** {}\n", node.tags.join(", ")));
        }

        // Direct relationships
        if !node.depends_on.is_empty() {
            output.push_str("\n### Depends on\n");
            for dep in &node.depends_on {
                let label = graph.get_node(dep).map(|n| n.label.as_str()).unwrap_or("?");
                output.push_str(&format!("- `{dep}` — {label}\n"));
            }
        }
        if !node.blocks.is_empty() {
            output.push_str("\n### Blocks\n");
            for b in &node.blocks {
                let label = graph.get_node(b).map(|n| n.label.as_str()).unwrap_or("?");
                output.push_str(&format!("- `{b}` — {label}\n"));
            }
        }
        if !node.children.is_empty() {
            output.push_str("\n### Children\n");
            for c in &node.children {
                let label = graph.get_node(c).map(|n| n.label.as_str()).unwrap_or("?");
                let status = graph
                    .get_node(c)
                    .and_then(|n| n.status.as_deref())
                    .unwrap_or("?");
                output.push_str(&format!("- `{c}` [{status}] {label}\n"));
            }
        }
        if let Some(ref p) = node.parent {
            let label = graph.get_node(p).map(|n| n.label.as_str()).unwrap_or("?");
            output.push_str(&format!("\n**Parent:** `{p}` — {label}\n"));
        }

        // Backlinks grouped by type
        let backlinks = graph.backlinks_by_type(&node_id);
        if !backlinks.is_empty() {
            output.push_str("\n### Backlinks (by source type)\n");
            let mut types: Vec<_> = backlinks.keys().collect();
            types.sort();
            for ntype in types {
                let entries = &backlinks[ntype];
                output.push_str(&format!("\n**{ntype}** ({} links)\n", entries.len()));
                for (source_node, edge_type) in entries {
                    output.push_str(&format!(
                        "- `{}` [{:?}] {}\n",
                        source_node.id, edge_type, source_node.label
                    ));
                }
            }
        }

        // Ego subgraph (nearby nodes)
        let nearby = graph.ego_subgraph(&node_id, hops);
        if !nearby.is_empty() {
            output.push_str(&format!("\n### Nearby nodes ({hops}-hop neighbourhood)\n"));
            let mut sorted = nearby;
            sorted.sort_by_key(|(_, d)| *d);
            for (nid, dist) in &sorted {
                let label = graph.get_node(nid).map(|n| n.label.as_str()).unwrap_or("?");
                output.push_str(&format!("- [hop {dist}] `{nid}` — {label}\n"));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_pkb_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let boost_id = args.get("boost_id").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = if project.is_some() {
            limit * 5
        } else {
            limit * 2
        };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root);

        // Build proximity boost map if boost_id provided
        let boost_map: std::collections::HashMap<String, f32> = if let Some(bid) = boost_id {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(bid) {
                let node_id = node.id.clone();
                graph
                    .ego_subgraph(&node_id, 3)
                    .into_iter()
                    .map(|(nid, dist)| (nid, 0.3 / dist as f32))
                    .collect()
            } else {
                std::collections::HashMap::new()
            }
        } else {
            std::collections::HashMap::new()
        };

        // Score and sort results
        let graph = self.graph.read();
        let mut scored: Vec<_> = results
            .iter()
            .map(|r| {
                let path_str = r.path.to_string_lossy();
                let node_id = graph
                    .nodes()
                    .find(|n| self.abs_path(&n.path).to_string_lossy() == path_str)
                    .map(|n| n.id.clone());

                let boost = node_id
                    .as_ref()
                    .and_then(|nid| boost_map.get(nid))
                    .unwrap_or(&0.0);

                (r, r.score * (1.0 + boost))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        if let Some(proj) = project {
            scored.retain(|(r, _)| {
                r.project
                    .as_deref()
                    .map(|p| p.eq_ignore_ascii_case(proj))
                    .unwrap_or(false)
            });
        }
        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No results found.",
            )]));
        }

        let mut output = format!(
            "**Found {} results for:** \"{}\"{}\n\n",
            scored.len(),
            query,
            if boost_id.is_some() {
                " (with graph proximity boost)"
            } else {
                ""
            }
        );

        for (i, (r, score)) in scored.iter().enumerate() {
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                i + 1,
                r.title,
                score
            ));
            output.push_str(&format!("**Path:** `{}`\n", r.path.display()));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!("**Type:** {dt}\n"));
            }
            if !r.tags.is_empty() {
                output.push_str(&format!("**Tags:** {}\n", r.tags.join(", ")));
            }
            if !r.snippet.is_empty() {
                output.push_str(&format!("\n> {}\n", r.snippet.replace('\n', "\n> ")));
            }
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_pkb_trace(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: from"),
                data: None,
            })?;

        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: to"),
                data: None,
            })?;

        let max_paths = args.get("max_paths").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

        let graph = self.graph.read();

        let from_node = graph.resolve(from).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Source node not found: {from}")),
            data: None,
        })?;
        let from_id = from_node.id.clone();
        let from_label = from_node.label.clone();

        let to_node = graph.resolve(to).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Target node not found: {to}")),
            data: None,
        })?;
        let to_id = to_node.id.clone();
        let to_label = to_node.label.clone();

        let paths = graph.all_shortest_paths(&from_id, &to_id, max_paths);

        if paths.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No path found between `{from_id}` ({from_label}) and `{to_id}` ({to_label})."
            ))]));
        }

        let mut output = format!(
            "## Paths from `{from_id}` to `{to_id}`\n\n**{} path(s) found** (length: {} hops)\n\n",
            paths.len(),
            paths[0].len() - 1
        );

        for (i, path) in paths.iter().enumerate() {
            output.push_str(&format!("### Path {}\n", i + 1));
            for (j, nid) in path.iter().enumerate() {
                let label = graph.get_node(nid).map(|n| n.label.as_str()).unwrap_or("?");
                let prefix = if j == 0 { "  " } else { "  → " };
                output.push_str(&format!("{prefix}`{nid}` ({label})\n"));
            }
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_pkb_orphans(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let type_filter: Option<Vec<String>> =
            args.get("types").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let project = args.get("project").and_then(|v| v.as_str());

        let graph = self.graph.read();
        let mut orphans = graph.orphans();

        // Filter by type if requested
        if let Some(ref types) = type_filter {
            orphans.retain(|n| {
                n.node_type
                    .as_deref()
                    .map(|t| types.iter().any(|f| f.eq_ignore_ascii_case(t)))
                    .unwrap_or(false)
            });
        }

        // Filter by project if requested
        if let Some(proj) = project {
            orphans.retain(|n| n.project.as_deref() == Some(proj));
        }

        if orphans.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No orphan nodes found. All nodes have at least one connection.",
            )]));
        }

        // Sort by label for consistent output
        orphans.sort_by(|a, b| a.label.cmp(&b.label));

        let max = limit.unwrap_or(orphans.len());
        let total = orphans.len();
        let showing = total.min(max);

        let type_desc = type_filter
            .as_ref()
            .map(|t| format!(" (types: {})", t.join(", ")))
            .unwrap_or_default();

        let mut output = format!(
            "**{total} orphan nodes{type_desc}** (showing {showing})\n\nThese nodes have no edges — no incoming or outgoing connections.\n\n"
        );

        for node in orphans.iter().take(max) {
            output.push_str(&format!("- **{}**", node.label));
            if let Some(ref t) = node.node_type {
                output.push_str(&format!(" [{t}]"));
            }
            output.push_str(&format!(" — `{}`\n", self.abs_path(&node.path).display()));
        }

        if total > max {
            output.push_str(&format!("\n...and {} more\n", total - max));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        // Resolve ID to graph node (supports exact ID, filename stem, title)
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path(&node.path);

        if !abs_path.exists() {
            return Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Task file not found on disk: {}",
                    abs_path.display()
                )),
                data: None,
            });
        }

        let content = std::fs::read_to_string(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read task file: {e}")),
            data: None,
        })?;

        // Parse YAML frontmatter
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter.parse(&content);

        let frontmatter = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<JsonValue>().ok())
            .unwrap_or(JsonValue::Object(serde_json::Map::new()));

        let body = parsed.content.trim().to_string();

        // Build relationship context from graph
        let resolve_ref = |rid: &str| -> serde_json::Value {
            if let Some(n) = graph.get_node(rid) {
                serde_json::json!({
                    "id": rid,
                    "title": n.label,
                    "status": n.status,
                })
            } else {
                serde_json::json!({ "id": rid })
            }
        };

        let depends_on: Vec<serde_json::Value> =
            node.depends_on.iter().map(|d| resolve_ref(d)).collect();
        let blocks: Vec<serde_json::Value> = node.blocks.iter().map(|b| resolve_ref(b)).collect();
        let children: Vec<serde_json::Value> =
            node.children.iter().map(|c| resolve_ref(c)).collect();
        let parent = node.parent.as_ref().map(|p| resolve_ref(p));

        let result = serde_json::json!({
            "frontmatter": frontmatter,
            "body": body,
            "path": abs_path.to_string_lossy(),
            "depends_on": depends_on,
            "blocks": blocks,
            "children": children,
            "parent": parent,
            "downstream_weight": node.downstream_weight,
            "stakeholder_exposure": node.stakeholder_exposure,
        });

        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // =========================================================================
    // MEMORY + DELETE + COMPLETE TOOLS (4)
    // =========================================================================

    fn handle_create_memory(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let fields = crate::document_crud::MemoryFields {
            title: title.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            memory_type: args
                .get("memory_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            source: args
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        let path =
            crate::document_crud::create_memory(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create memory: {e}")),
                data: None,
            })?;

        // Index the new file
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        self.rebuild_graph();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Memory created: `{}`",
            path.display()
        ))]))
    }

    fn handle_create_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let doc_type = args
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: type"),
                data: None,
            })?;

        let fields = crate::document_crud::DocumentFields {
            title: title.to_string(),
            doc_type: doc_type.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
            priority: args
                .get("priority")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            parent: args
                .get("parent")
                .and_then(|v| v.as_str())
                .map(String::from),
            depends_on: args
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            project: args
                .get("project")
                .and_then(|v| v.as_str())
                .map(String::from),
            assignee: args
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(String::from),
            complexity: args
                .get("complexity")
                .and_then(|v| v.as_str())
                .map(String::from),
            source: args
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from),
            due: args.get("due").and_then(|v| v.as_str()).map(String::from),
            dir: args.get("dir").and_then(|v| v.as_str()).map(String::from),
        };

        // Hierarchy validation: warn if task-like type without parent
        let mut warnings = Vec::new();
        let root_allowed = ["goal", "learn", "project"];
        let task_like = ["task", "epic", "action", "bug", "feature"];
        if task_like.contains(&doc_type) && fields.parent.is_none() {
            warnings.push(format!(
                "Hierarchy warning: Type '{}' should have a parent. \
                 Only {} types can be root-level. \
                 Consider assigning a parent to maintain graph hierarchy.",
                doc_type,
                root_allowed.join(", "),
            ));
        }

        let path = crate::document_crud::create_document(&self.pkb_root, fields).map_err(|e| {
            McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create document: {e}")),
                data: None,
            }
        })?;

        // Index the new file
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        self.rebuild_graph();

        let mut msg = format!("Document created: `{}`", path.display());
        if !warnings.is_empty() {
            msg.push_str("\n\nHierarchy warnings:\n");
            for w in &warnings {
                msg.push_str(&format!("- {}\n", w));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    fn handle_append_to_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: content"),
                data: None,
            })?;

        let section = args.get("section").and_then(|v| v.as_str());

        // Resolve ID to path via graph
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Document not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        drop(graph);

        crate::document_crud::append_to_document(&abs_path, content, section).map_err(|e| {
            McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to append: {e}")),
                data: None,
            }
        })?;

        // Re-index the updated file
        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        self.rebuild_graph();

        let section_msg = section
            .map(|s| format!(" under ## {s}"))
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Appended to: {} (`{}`){section_msg}",
            label, id
        ))]))
    }

    fn handle_delete_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Document not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        let rel_path = node.path.to_string_lossy().to_string();
        drop(graph); // release read lock before write operations

        crate::document_crud::delete_document(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        // Remove from vector store
        self.store.write().remove(&rel_path);
        let _ = self.store.read().save(&self.db_path);

        // Rebuild graph
        self.rebuild_graph();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Deleted: {} (`{}`)",
            label,
            abs_path.display()
        ))]))
    }

    fn handle_complete_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        drop(graph);

        let mut updates = std::collections::HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String("done".to_string()),
        );

        crate::document_crud::update_document(&abs_path, updates).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to complete task: {e}")),
            data: None,
        })?;

        // Re-index
        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        self.rebuild_graph();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Completed: {} (`{}`)",
            label, id
        ))]))
    }

    // =========================================================================
    // NEW TOOLS: Memory CRUD, decompose, dependency tree, children
    // =========================================================================

    fn handle_retrieve_memory(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let query_embedding = self.embedder.encode(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let results = store.search(&query_embedding, limit * 3, &self.pkb_root);

        let memory_types = ["memory", "note", "insight", "observation"];
        let mut output = String::new();
        let mut count = 0;

        for r in &results {
            if count >= limit {
                break;
            }
            let is_memory = r
                .doc_type
                .as_deref()
                .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                .unwrap_or(false);
            if !is_memory {
                continue;
            }

            if let Some(ref required_tags) = tags {
                let has_all = required_tags
                    .iter()
                    .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)));
                if !has_all {
                    continue;
                }
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                count, r.title, r.score
            ));
            output.push_str(&format!("**Path:** `{}`\n", r.path.display()));
            if !r.tags.is_empty() {
                output.push_str(&format!("**Tags:** {}\n", r.tags.join(", ")));
            }
            // Show full body for memories (typically short)
            if let Ok(content) = std::fs::read_to_string(&r.path) {
                let body = if content.starts_with("---") {
                    content.splitn(3, "---").nth(2).unwrap_or("").trim()
                } else {
                    content.trim()
                };
                if !body.is_empty() {
                    output.push_str(&format!("\n{body}\n"));
                }
            }
            output.push('\n');
        }

        if count == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No memories found matching query.",
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "**Found {count} memories for:** \"{query}\"\n\n{output}"
        ))]))
    }

    fn handle_search_by_tag(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: tags"),
                data: None,
            })?;

        if tags.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("tags array cannot be empty"),
                data: None,
            });
        }

        let type_filter = args.get("type").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let store = self.store.read();
        let all = store.list_documents(None, type_filter, None, None, &self.pkb_root);

        let mut matching: Vec<_> = all
            .into_iter()
            .filter(|r| {
                tags.iter()
                    .all(|tag| r.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
            })
            .collect();
        matching.truncate(limit);

        if matching.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No documents found with tags: {}",
                tags.join(", ")
            ))]));
        }

        let total = matching.len();
        let mut output = format!("**{total} documents with tags [{}]**\n\n", tags.join(", "));
        for r in &matching {
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            output.push_str(&format!(" ({})", r.tags.join(", ")));
            output.push_str(&format!(" — `{}`\n", r.path.display()));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_list_memories(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let memory_types = ["memory", "note", "insight", "observation"];
        let store = self.store.read();

        let mut memories: Vec<_> = store
            .list_documents(None, None, None, None, &self.pkb_root)
            .into_iter()
            .filter(|r| {
                r.doc_type
                    .as_deref()
                    .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                    .unwrap_or(false)
            })
            .collect();

        if let Some(ref required_tags) = tags {
            memories.retain(|r| {
                required_tags
                    .iter()
                    .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)))
            });
        }

        memories.truncate(limit);

        if memories.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No memories found.",
            )]));
        }

        let total = memories.len();
        let mut output = format!("**{total} memories**\n\n");
        for r in &memories {
            output.push_str(&format!("- **{}**", r.title));
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            output.push_str(&format!(" — `{}`\n", r.path.display()));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_delete_memory(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Memory not found: {id}")),
            data: None,
        })?;

        let memory_types = ["memory", "note", "insight", "observation"];
        let is_memory = node
            .node_type
            .as_deref()
            .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
            .unwrap_or(false);

        if !is_memory {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Not a memory document: {id} (type: {:?})",
                    node.node_type
                )),
                data: None,
            });
        }
        drop(graph);

        self.handle_delete_document(args)
    }

    fn handle_decompose_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let parent_id = args
            .get("parent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: parent_id"),
                data: None,
            })?;

        let subtasks = args
            .get("subtasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: subtasks (array)"),
                data: None,
            })?;

        if subtasks.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("subtasks array cannot be empty"),
                data: None,
            });
        }

        {
            let graph = self.graph.read();
            match graph.resolve(parent_id) {
                None => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!("Parent task not found: {parent_id}")),
                        data: None,
                    });
                }
                Some(node) => {
                    if node.task_id.is_none() {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Parent ID must refer to a task node, but `{parent_id}` is not a task"
                            )),
                            data: None,
                        });
                    }
                }
            }
        }

        let mut created: Vec<(String, String)> = Vec::new();
        static ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^([a-z]+-[0-9a-f]{8})").expect("valid static regex")
        });

        for subtask in subtasks {
            let title = subtask
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Each subtask must have a 'title'"),
                    data: None,
                })?;

            let fields = crate::document_crud::TaskFields {
                title: title.to_string(),
                id: subtask.get("id").and_then(|v| v.as_str()).map(String::from),
                parent: Some(parent_id.to_string()),
                priority: subtask
                    .get("priority")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                project: subtask
                    .get("project")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                tags: subtask
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                depends_on: subtask
                    .get("depends_on")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                assignee: subtask
                    .get("assignee")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                complexity: subtask
                    .get("complexity")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                body: subtask
                    .get("body")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            };

            let path = crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| {
                McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to create subtask '{title}': {e}")),
                    data: None,
                }
            })?;

            if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
                let _ = self.store.write().upsert(&doc, &self.embedder);
            }

            let id_str = path
                .file_stem()
                .map(|s| {
                    let stem = s.to_string_lossy();
                    ID_RE
                        .find(&stem)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| stem.to_string())
                })
                .unwrap_or_default();
            created.push((id_str, path.display().to_string()));
        }

        let _ = self.store.read().save(&self.db_path);
        self.rebuild_graph();

        let mut output = format!(
            "**Created {} subtasks under `{parent_id}`:**\n\n",
            created.len()
        );
        for (id_str, path) in &created {
            output.push_str(&format!("- `{id_str}` — `{path}`\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_dependency_tree(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("upstream");

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;
        let node_id = node.id.clone();
        let node_label = node.label.clone();

        let tree = if direction.eq_ignore_ascii_case("downstream") {
            graph.blocks_tree(&node_id)
        } else {
            graph.dependency_tree(&node_id)
        };

        if tree.is_empty() {
            let dir_label = if direction.eq_ignore_ascii_case("downstream") {
                "downstream"
            } else {
                "upstream"
            };
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No {dir_label} dependencies for `{id}` ({node_label})."
            ))]));
        }

        let dir_label = if direction.eq_ignore_ascii_case("downstream") {
            "Downstream (blocks)"
        } else {
            "Upstream (depends on)"
        };
        let mut output = format!("## {dir_label} tree for `{id}` ({node_label})\n\n");

        for (dep_id, depth) in &tree {
            let indent = "  ".repeat(*depth);
            let label = graph
                .get_node(dep_id)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            let status = graph
                .get_node(dep_id)
                .and_then(|n| n.status.as_deref())
                .unwrap_or("?");
            output.push_str(&format!("{indent}- `{dep_id}` [{status}] {label}\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_task_children(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;
        let node_id = node.id.clone();
        let node_label = node.label.clone();

        if node.children.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No children for `{id}` ({node_label})."
            ))]));
        }

        let mut output = format!("## Children of `{id}` ({node_label})\n\n");

        fn collect_children(
            graph: &GraphStore,
            parent_id: &str,
            recursive: bool,
            depth: usize,
            output: &mut String,
            total: &mut usize,
            done: &mut usize,
        ) {
            if let Some(node) = graph.get_node(parent_id) {
                for child_id in &node.children {
                    if let Some(child) = graph.get_node(child_id) {
                        *total += 1;
                        let is_done = child.status.as_deref() == Some("done")
                            || child.status.as_deref() == Some("cancelled");
                        if is_done {
                            *done += 1;
                        }
                        let indent = "  ".repeat(depth);
                        let status = child.status.as_deref().unwrap_or("-");
                        let pri = child.priority.map(|p| format!("P{p} ")).unwrap_or_default();
                        let cid = child.task_id.as_deref().unwrap_or(&child.id);
                        output.push_str(&format!(
                            "{indent}- `{cid}` [{status}] {pri}{}\n",
                            child.label
                        ));
                        if recursive && !child.children.is_empty() {
                            collect_children(
                                graph,
                                &child.id,
                                recursive,
                                depth + 1,
                                output,
                                total,
                                done,
                            );
                        }
                    }
                }
            }
        }

        let mut total = 0usize;
        let mut done_count = 0usize;
        collect_children(
            &graph,
            &node_id,
            recursive,
            0,
            &mut output,
            &mut total,
            &mut done_count,
        );

        output.push_str(&format!("\n**Summary:** {done_count}/{total} complete\n"));

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_list_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let project = args.get("project").and_then(|v| v.as_str());
        let status = args.get("status").and_then(|v| v.as_str());
        let priority = args
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let assignee = args.get("assignee").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let graph = self.graph.read();

        // Detect special status filters: "ready" and "blocked"
        let is_ready = status
            .map(|s| s.eq_ignore_ascii_case("ready"))
            .unwrap_or(false);
        let is_blocked = status
            .map(|s| s.eq_ignore_ascii_case("blocked"))
            .unwrap_or(false);

        let mut tasks: Vec<_> = if is_ready {
            // Use graph.ready_tasks() for the ready filter
            let ready_ids: std::collections::HashSet<String> =
                graph.ready_tasks().iter().map(|n| n.id.clone()).collect();
            graph
                .all_tasks()
                .into_iter()
                .filter(|t| ready_ids.contains(&t.id))
                .collect()
        } else if is_blocked {
            // Use graph.blocked_tasks() for the blocked filter
            let blocked_ids: std::collections::HashSet<String> =
                graph.blocked_tasks().iter().map(|n| n.id.clone()).collect();
            graph
                .all_tasks()
                .into_iter()
                .filter(|t| blocked_ids.contains(&t.id))
                .collect()
        } else {
            let mut all: Vec<_> = graph.all_tasks().into_iter().collect();
            if let Some(s) = status {
                all.retain(|t| {
                    t.status
                        .as_deref()
                        .map(|st| st.eq_ignore_ascii_case(s))
                        .unwrap_or(false)
                });
            }
            all
        };

        if let Some(proj) = project {
            tasks.retain(|t| {
                t.project
                    .as_deref()
                    .map(|p| p.eq_ignore_ascii_case(proj))
                    .unwrap_or(false)
            });
        }
        if let Some(pri) = priority {
            tasks.retain(|t| t.priority == Some(pri));
        }
        if let Some(a) = assignee {
            tasks.retain(|t| {
                t.assignee
                    .as_deref()
                    .map(|ag| ag.eq_ignore_ascii_case(a))
                    .unwrap_or(false)
            });
        }

        if tasks.is_empty() {
            let label = if is_ready {
                "No ready tasks found."
            } else if is_blocked {
                "No blocked tasks."
            } else {
                "No tasks found matching filters."
            };
            return Ok(CallToolResult::success(vec![Content::text(label)]));
        }

        let total = tasks.len();
        tasks.truncate(limit);

        let output = if is_blocked {
            // Blocked view: heading per task with blocker details
            let mut out = format!("**{total} blocked tasks** (showing {})\n\n", tasks.len());
            for t in &tasks {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                out.push_str(&format!("### {} — {}\n", id, t.label));
                if !t.depends_on.is_empty() {
                    out.push_str("**Blocked by:**\n");
                    for dep in &t.depends_on {
                        let dep_label =
                            graph.get_node(dep).map(|n| n.label.as_str()).unwrap_or("?");
                        let dep_status = graph
                            .get_node(dep)
                            .and_then(|n| n.status.as_deref())
                            .unwrap_or("?");
                        out.push_str(&format!("- `{}` [{}] {}\n", dep, dep_status, dep_label));
                    }
                }
                if t.status.as_deref() == Some("blocked") {
                    out.push_str("**Status:** explicitly blocked\n");
                }
                out.push('\n');
            }
            out
        } else if is_ready {
            // Ready view: table with Weight column, sorted by priority + downstream weight
            let mut out = format!(
                "**{total} ready tasks** (showing {}, sorted by priority + downstream weight)\n\n",
                tasks.len()
            );
            out.push_str("| # | ID | Pri | Weight | Title |\n|---|---|---|---|---|\n");
            for (i, t) in tasks.iter().enumerate() {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                let weight = if t.downstream_weight > 0.0 {
                    format!(
                        "{:.1}{}",
                        t.downstream_weight,
                        if t.stakeholder_exposure { "!" } else { "" }
                    )
                } else {
                    "-".to_string()
                };
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    i + 1,
                    id,
                    t.priority.unwrap_or(2),
                    weight,
                    t.label
                ));
            }
            out
        } else {
            // Default view: standard table
            let mut out = format!(
                "**{total} tasks** (showing {})\n\n| # | ID | Pri | Status | Title |\n|---|---|---|---|---|\n",
                tasks.len()
            );
            for (i, t) in tasks.iter().enumerate() {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                let pri = t.priority.unwrap_or(2);
                let status_str = t.status.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    i + 1,
                    id,
                    pri,
                    status_str,
                    t.label
                ));
            }
            out
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_update_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        // Accept either `path` or `id` — resolve id via graph if path not given
        let path = if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
            self.resolve_path(path_str)
        } else if let Some(id) = args.get("id").and_then(|v| v.as_str()) {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            self.abs_path(&node.path)
        } else {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: path or id"),
                data: None,
            });
        };

        let updates = args
            .get("updates")
            .and_then(|v| v.as_object())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: updates (object)"),
                data: None,
            })?;

        let update_map: std::collections::HashMap<String, serde_json::Value> = updates
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        crate::document_crud::update_document(&path, update_map).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        // Re-index the updated file (with relative path for portable storage)
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            let _ = self.store.read().save(&self.db_path);
        }

        // Rebuild graph
        self.rebuild_graph();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task updated: `{}`",
            path.display()
        ))]))
    }
}

impl ServerHandler for PkbSearchServer {
    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let args = Self::args_to_value(request.arguments);
        let result = match &*request.name {
            "search" => self.handle_pkb_search(&args),
            "get_document" => self.handle_get_document(&args),
            "list_documents" => self.handle_list_documents(&args),
            "reindex" => self.handle_reindex(&args),
            "task_search" => self.handle_task_search(&args),
            "get_network_metrics" => self.handle_get_network_metrics(&args),
            "create_task" => self.handle_create_task(&args),
            "create_memory" => self.handle_create_memory(&args),
            "create" => self.handle_create_document(&args),
            "append" => self.handle_append_to_document(&args),
            "delete" => self.handle_delete_document(&args),
            "complete_task" => self.handle_complete_task(&args),
            "list_tasks" => self.handle_list_tasks(&args),
            "get_task" => self.handle_get_task(&args),
            "update_task" => self.handle_update_task(&args),
            "retrieve_memory" => self.handle_retrieve_memory(&args),
            "search_by_tag" => self.handle_search_by_tag(&args),
            "list_memories" => self.handle_list_memories(&args),
            "delete_memory" => self.handle_delete_memory(&args),
            "decompose_task" => self.handle_decompose_task(&args),
            "get_dependency_tree" => self.handle_get_dependency_tree(&args),
            "get_task_children" => self.handle_get_task_children(&args),
            "pkb_context" => self.handle_pkb_context(&args),
            "pkb_trace" => self.handle_pkb_trace(&args),
            "pkb_orphans" => self.handle_pkb_orphans(&args),
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown tool: {}", request.name)),
                data: None,
            }),
        };
        std::future::ready(result)
    }

    fn list_tools(
        &self,
        _request: PaginatedRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = vec![
            Tool::new(
                "search",
                "Hybrid semantic + graph-proximity search across the personal knowledge base. Optionally boost results near a specific node.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural language search query" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
                        "boost_id": { "type": "string", "description": "Optional: boost results near this node (ID, filename, or title)" },
                        "project": { "type": "string", "description": "Filter by project" }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_document",
                "Read the full contents of a specific PKB document.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to document" }
                    },
                    "required": ["path"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "list_documents",
                "List indexed documents with optional filters and pagination.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": { "type": "string", "description": "Filter by tag" },
                        "type": { "type": "string", "description": "Filter by type" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "project": { "type": "string", "description": "Filter by project" },
                        "limit": { "type": "integer", "description": "Max results (default: all)" },
                        "offset": { "type": "integer", "description": "Skip first N results (default: 0)" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "reindex",
                "Re-scan and re-index. Rebuilds vector store and knowledge graph. Incremental by default (mtime-based); set force=true to re-embed all files.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "force": { "type": "boolean", "description": "Force full re-index of all files (default: false, incremental mtime-based)" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "task_search",
                "Semantic search filtered to tasks. Returns tasks with graph context.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Query to search tasks" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
                        "project": { "type": "string", "description": "Filter by project" }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_network_metrics",
                "Get centrality metrics: degree, betweenness, PageRank, downstream weight.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "create_task",
                "Create a new task markdown file with YAML frontmatter.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Task title" },
                        "id": { "type": "string", "description": "Task ID (auto-generated if omitted)" },
                        "parent": { "type": "string", "description": "Parent task ID" },
                        "priority": { "type": "integer", "description": "0-4 (0=critical)" },
                        "project": { "type": "string", "description": "Project name" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "body": { "type": "string", "description": "Markdown body" }
                    },
                    "required": ["title"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "create_memory",
                "Create a new memory/note markdown file with YAML frontmatter. Stored in memories/ directory.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Memory title" },
                        "id": { "type": "string", "description": "Memory ID (auto-generated if omitted)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for the memory" },
                        "body": { "type": "string", "description": "Markdown body content" },
                        "memory_type": { "type": "string", "description": "Subtype: memory (default), note, insight, observation" },
                        "source": { "type": "string", "description": "Source context (e.g. session ID)" }
                    },
                    "required": ["title"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "create",
                "Create a new document with full enforced frontmatter. Subdirectory routing: task/bug/epic/feature -> tasks/, project -> projects/, goal -> goals/, else -> notes/.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Document title (required)" },
                        "type": { "type": "string", "description": "Document type (required): note, knowledge, memory, insight, observation, task, project, goal, etc." },
                        "id": { "type": "string", "description": "Document ID (auto-generated if omitted)" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "body": { "type": "string", "description": "Markdown body" },
                        "status": { "type": "string" },
                        "priority": { "type": "integer", "description": "0-4 (0=critical)" },
                        "parent": { "type": "string", "description": "Parent document ID" },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "project": { "type": "string", "description": "Project name" },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "source": { "type": "string", "description": "Source context" },
                        "due": { "type": "string", "description": "Due date" },
                        "dir": { "type": "string", "description": "Override subdirectory placement" }
                    },
                    "required": ["title", "type"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "append",
                "Append timestamped content to an existing document. Optionally target a specific section heading. Auto-updates modified timestamp.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (supports flexible resolution: ID, filename stem, or title)" },
                        "content": { "type": "string", "description": "Content to append (will be timestamped)" },
                        "section": { "type": "string", "description": "Optional target section heading (e.g. 'Log', 'References'). Creates section if not found." }
                    },
                    "required": ["id", "content"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "delete",
                "Delete a document by ID. Removes the file from disk and the vector store index.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (task ID, memory ID, filename stem, or title). Uses flexible resolution." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "complete_task",
                "Mark a task as done. Sets status to 'done' and re-indexes.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (supports flexible resolution: ID, filename stem, or title)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "list_tasks",
                "List tasks with filtering by project, status, priority, and assignee. Use status='ready' for actionable tasks sorted by priority + downstream weight, or status='blocked' to see blocked tasks with their blockers.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project" },
                        "status": { "type": "string", "description": "Filter by status. Special values: 'ready' (actionable leaf tasks), 'blocked' (tasks with unmet deps). Also: active, in_progress, done, etc." },
                        "priority": { "type": "integer", "description": "Filter by exact priority (0-4)" },
                        "assignee": { "type": "string", "description": "Filter by assignee" },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_task",
                "Retrieve a task by ID. Returns frontmatter, body, path, and relationship context (depends_on, blocks, children, parent with titles/statuses, downstream_weight, stakeholder_exposure).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (e.g. 'framework-6b4325a1'). Also accepts filename stem or title." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "update_task",
                "Update frontmatter fields on an existing task file. Auto-sets modified timestamp.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to task file" },
                        "id": { "type": "string", "description": "Document ID (alternative to path — uses flexible resolution)" },
                        "updates": { "type": "object", "description": "Fields to update (null to remove)" }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "retrieve_memory",
                "Semantic search filtered to memory-type documents. Returns full content since memories are typically short.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query for finding relevant memories" },
                        "limit": { "type": "integer", "description": "Maximum number of memories to return (default: 10)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Only return memories with all of these tags" }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "search_by_tag",
                "Search documents by tags. Returns all documents matching the specified tags.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags to search for (all must match)" },
                        "type": { "type": "string", "description": "Filter by document type (e.g. 'memory', 'task')" },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" }
                    },
                    "required": ["tags"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "list_memories",
                "List memory-type documents (memory, note, insight, observation) with optional tag filtering.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: 20)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags (all must match)" },

                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "delete_memory",
                "Delete a memory document by ID. Only works on memory-type documents (memory, note, insight, observation).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Memory ID (supports flexible resolution)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "decompose_task",
                "Batch creation of subtasks under a parent task. More efficient than calling create_task N times (single graph rebuild).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent_id": { "type": "string", "description": "Parent task ID to create subtasks under" },
                        "subtasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "id": { "type": "string", "description": "Optional custom task ID" },
                                    "priority": { "type": "integer" },
                                    "depends_on": { "type": "array", "items": { "type": "string" } },
                                    "tags": { "type": "array", "items": { "type": "string" } },
                                    "assignee": { "type": "string" },
                                    "complexity": { "type": "string" },
                                    "body": { "type": "string" },
                                    "project": { "type": "string" }
                                },
                                "required": ["title"]
                            },
                            "description": "Array of subtask definitions"
                        }
                    },
                    "required": ["parent_id", "subtasks"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_dependency_tree",
                "Get the dependency tree for a task. Upstream shows what it depends on, downstream shows what it unblocks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "direction": { "type": "string", "description": "Direction: 'upstream' (depends on, default) or 'downstream' (blocks)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_task_children",
                "Get direct children of a task, or the full subtree if recursive. Returns status and completion counts.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "recursive": { "type": "boolean", "description": "Include all descendants, not just direct children (default: false)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "pkb_context",
                "Get full knowledge neighbourhood for a node: metadata, relationships, backlinks grouped by source type, and nearby nodes within N hops. Supports flexible ID resolution (by ID, filename, title).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID, task ID, filename stem, or title" },
                        "hops": { "type": "integer", "description": "Neighbourhood radius in hops (default: 2)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "pkb_trace",
                "Find shortest paths between two nodes in the knowledge graph. Shows up to N paths with node labels.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Source node (ID, filename, or title)" },
                        "to": { "type": "string", "description": "Target node (ID, filename, or title)" },
                        "max_paths": { "type": "integer", "description": "Maximum paths to return (default: 3)" }
                    },
                    "required": ["from", "to"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "pkb_orphans",
                "Find disconnected nodes with zero edges (no incoming or outgoing connections). Filter by node type or project.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: all). Set to 0 for unlimited." },
                        "types": { "type": "array", "items": { "type": "string" }, "description": "Filter by node type (e.g. [\"task\"], [\"task\", \"project\"]). Omit for all types." },
                        "project": { "type": "string", "description": "Filter by project" }
                    }
                }))
                .unwrap(),
            ),
        ];

        std::future::ready(Ok(ListToolsResult {
            tools,
            next_cursor: None,
        }))
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "pkb".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "PKB Search — semantic search + task graph over personal knowledge base. \
                 25 tools: search, get_document, list_documents, reindex, \
                 task_search, get_network_metrics, create_task, create_memory, \
                 create, append, delete, complete_task, list_tasks, \
                 get_task, update_task, retrieve_memory, search_by_tag, \
                 list_memories, delete_memory, decompose_task, \
                 get_dependency_tree, get_task_children, \
                 pkb_context, pkb_trace, pkb_orphans."
                    .to_string(),
            ),
        }
    }
}
