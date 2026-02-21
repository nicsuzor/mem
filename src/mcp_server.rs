//! MCP server for PKB semantic search + task graph.
//!
//! Implements rmcp 0.1.5 ServerHandler trait manually with tool dispatch.
//! Provides 11 tools: 4 original search tools + 7 graph/task tools.

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
    // ORIGINAL TOOLS (4)
    // =========================================================================

    fn handle_semantic_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;
        let project = args.get("project").and_then(|v| v.as_str());

        let query_embedding = self.embedder.encode(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        // Over-fetch when filtering by project
        let fetch_limit = if project.is_some() { limit * 5 } else { limit };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root);

        let results: Vec<_> = if let Some(proj) = project {
            results
                .into_iter()
                .filter(|r| r.project.as_deref().map(|p| p.eq_ignore_ascii_case(proj)).unwrap_or(false))
                .take(limit)
                .collect()
        } else {
            results.into_iter().take(limit).collect()
        };

        if results.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No results found.",
            )]));
        }

        let mut output = format!(
            "**Found {} results for:** \"{}\"\n\n",
            results.len(),
            query
        );

        for (i, r) in results.iter().enumerate() {
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                i + 1,
                r.title,
                r.score
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
        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let store = self.store.read();
        let results = store.list_documents(tag, doc_type, status, project, &self.pkb_root);
        let total = results.len();

        if total == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No documents found matching filters.",
            )]));
        }

        let page: Vec<_> = results.into_iter().skip(offset).take(limit.unwrap_or(total)).collect();
        let showing = page.len();

        let mut output = format!("**{total} documents found** (showing {showing}, offset {offset})\n\n");

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

    fn handle_reindex(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let (indexed, removed, total) =
            crate::index_pkb(&self.pkb_root, &self.db_path, &self.store, &self.embedder, true);

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

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;
        let project = args.get("project").and_then(|v| v.as_str());

        let query_embedding = self.embedder.encode(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = if project.is_some() { limit * 10 } else { limit * 3 };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root);

        let graph = self.graph.read();
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
                if !r.project.as_deref().map(|p| p.eq_ignore_ascii_case(proj)).unwrap_or(false) {
                    continue;
                }
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                count, r.title, r.score
            ));
            output.push_str(&format!("**Path:** `{}`\n", r.path.display()));

            // Compare using absolute paths (SearchResult is abs, node.path may be relative)
            let path_str = r.path.to_string_lossy();
            for node in graph.nodes() {
                if self.abs_path(&node.path).to_string_lossy() == path_str {
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
                        output.push_str(&format!(
                            "**Depends on:** {}\n",
                            node.depends_on.join(", ")
                        ));
                    }
                    break;
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

    fn handle_get_ready_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;
        let project = args.get("project").and_then(|v| v.as_str());

        let graph = self.graph.read();
        let mut tasks = graph.ready_tasks();

        if let Some(proj) = project {
            tasks.retain(|t| t.project.as_deref() == Some(proj));
        }

        if tasks.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No ready tasks found.",
            )]));
        }

        let mut output = format!(
            "**{} ready tasks** (sorted by priority + downstream weight)\n\n",
            tasks.len()
        );
        output.push_str("| # | ID | Pri | Weight | Title |\n|---|---|---|---|---|\n");

        for (i, t) in tasks.iter().take(limit).enumerate() {
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
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                i + 1,
                id,
                t.priority.unwrap_or(2),
                weight,
                t.label
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_blocked_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let project = args.get("project").and_then(|v| v.as_str());

        let graph = self.graph.read();
        let mut tasks = graph.blocked_tasks();

        if let Some(proj) = project {
            tasks.retain(|t| t.project.as_deref() == Some(proj));
        }

        if tasks.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No blocked tasks.",
            )]));
        }

        let mut output = format!("**{} blocked tasks**\n\n", tasks.len());

        for t in &tasks {
            let id = t.task_id.as_deref().unwrap_or(&t.id);
            output.push_str(&format!("### {} — {}\n", id, t.label));

            if !t.depends_on.is_empty() {
                output.push_str("**Blocked by:**\n");
                for dep in &t.depends_on {
                    let dep_label = graph
                        .get_node(dep)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    let dep_status = graph
                        .get_node(dep)
                        .and_then(|n| n.status.as_deref())
                        .unwrap_or("?");
                    output.push_str(&format!("- `{}` [{}] {}\n", dep, dep_status, dep_label));
                }
            }
            if t.status.as_deref() == Some("blocked") {
                output.push_str("**Status:** explicitly blocked\n");
            }
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_task_network(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
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
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let mut output = format!("## {} — {}\n\n", id, node.label);
        output.push_str(&format!("**Path:** `{}`\n", self.abs_path(&node.path).display()));

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

        if node.downstream_weight > 0.0 {
            output.push_str(&format!(
                "\n**Downstream weight:** {:.2}{}\n",
                node.downstream_weight,
                if node.stakeholder_exposure {
                    " (stakeholder exposure)"
                } else {
                    ""
                }
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
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
            body: args
                .get("body")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

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

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task created: `{}`",
            path.display()
        ))]))
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

        let hops = args
            .get("hops")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Node not found: {id}")),
            data: None,
        })?;

        let node_id = node.id.clone();
        let mut output = format!("## {} — {}\n\n", node_id, node.label);
        output.push_str(&format!("**Path:** `{}`\n", self.abs_path(&node.path).display()));

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
                let label = graph
                    .get_node(nid)
                    .map(|n| n.label.as_str())
                    .unwrap_or("?");
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

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let boost_id = args.get("boost_id").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());

        let query_embedding = self.embedder.encode(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = if project.is_some() { limit * 5 } else { limit * 2 };
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
            scored.retain(|(r, _)| r.project.as_deref().map(|p| p.eq_ignore_ascii_case(proj)).unwrap_or(false));
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

        let max_paths = args
            .get("max_paths")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as usize;

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
                let label = graph
                    .get_node(nid)
                    .map(|n| n.label.as_str())
                    .unwrap_or("?");
                let prefix = if j == 0 {
                    "  "
                } else {
                    "  → "
                };
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

        let type_filter: Option<Vec<String>> = args
            .get("types")
            .and_then(|v| v.as_array())
            .map(|arr| {
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

        let result = serde_json::json!({
            "frontmatter": frontmatter,
            "body": body,
            "path": abs_path.to_string_lossy(),
        });

        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_update_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: path"),
                data: None,
            })?;

        let updates = args
            .get("updates")
            .and_then(|v| v.as_object())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: updates (object)"),
                data: None,
            })?;

        let path = self.resolve_path(path_str);
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
            "semantic_search" => self.handle_semantic_search(&args),
            "get_document" => self.handle_get_document(&args),
            "list_documents" => self.handle_list_documents(&args),
            "reindex" => self.handle_reindex(&args),
            "task_search" => self.handle_task_search(&args),
            "get_ready_tasks" => self.handle_get_ready_tasks(&args),
            "get_blocked_tasks" => self.handle_get_blocked_tasks(&args),
            "get_task_network" => self.handle_get_task_network(&args),
            "get_network_metrics" => self.handle_get_network_metrics(&args),
            "create_task" => self.handle_create_task(&args),
            "get_task" => self.handle_get_task(&args),
            "update_task" => self.handle_update_task(&args),
            "pkb_context" => self.handle_pkb_context(&args),
            "pkb_search" => self.handle_pkb_search(&args),
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
                "semantic_search",
                "Search the personal knowledge base by semantic similarity.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural language query" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
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
                "Force re-scan and re-index. Rebuilds vector store and knowledge graph.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
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
                "get_ready_tasks",
                "Get actionable tasks sorted by priority and downstream impact.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: 20)" },
                        "project": { "type": "string", "description": "Filter by project" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_blocked_tasks",
                "Get blocked tasks with their blockers listed.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_task_network",
                "Get full relationship context for a task.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" }
                    },
                    "required": ["id"]
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
                "get_task",
                "Retrieve a task by ID. Returns parsed YAML frontmatter as structured JSON, the markdown body, and the file path.",
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
                "Update frontmatter fields on an existing task file.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to task file" },
                        "updates": { "type": "object", "description": "Fields to update (null to remove)" }
                    },
                    "required": ["path", "updates"]
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
                "pkb_search",
                "Hybrid semantic + graph-proximity search. Optionally boost results near a specific node.",
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
                name: "pkb-search".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "PKB Search — semantic search + task graph over personal knowledge base. \
                 16 tools: semantic_search, get_document, list_documents, reindex, \
                 task_search, get_ready_tasks, get_blocked_tasks, get_task_network, \
                 get_network_metrics, create_task, get_task, update_task, pkb_context, \
                 pkb_search, pkb_trace, pkb_orphans."
                    .to_string(),
            ),
        }
    }
}
