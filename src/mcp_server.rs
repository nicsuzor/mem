//! MCP server for PKB semantic search + task graph.
//!
//! Implements rmcp 0.1.5 ServerHandler trait manually with tool dispatch.
//! Provides 18 tools for search, documents, tasks, and knowledge graph.

use crate::embeddings::Embedder;
use crate::graph::is_completed;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use parking_lot::RwLock;
use rmcp::model::*;
use rmcp::{Error as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
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
    stale_count: usize,
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
            stale_count: 0,
        }
    }

    pub fn with_stale_count(mut self, count: usize) -> Self {
        self.stale_count = count;
        self
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

    /// Validate that completion_evidence is present and non-empty.
    fn require_evidence(evidence: Option<&str>) -> Result<&str, McpError> {
        let ev = evidence.ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "completion_evidence is required. Describe what was done before completing this task.",
            ),
            data: None,
        })?;
        if ev.trim().is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "completion_evidence is required. Describe what was done before completing this task.",
                ),
                data: None,
            });
        }
        Ok(ev)
    }

    /// Append a "## Completion Evidence" section to a document.
    fn append_evidence(path: &std::path::Path, evidence: &str, pr_url: Option<&str>) -> Result<(), McpError> {
        let evidence_block = if let Some(url) = pr_url {
            format!("\n\n## Completion Evidence\n\n{}\n\nPR: {}\n", evidence.trim(), url)
        } else {
            format!("\n\n## Completion Evidence\n\n{}\n", evidence.trim())
        };
        crate::document_crud::append_to_document(path, &evidence_block, None).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to append evidence: {e}")),
            data: None,
        })
    }

    fn args_to_value(args: Option<JsonObject>) -> JsonValue {
        match args {
            Some(map) => JsonValue::Object(map),
            None => JsonValue::Object(serde_json::Map::new()),
        }
    }

    /// Full rebuild of the graph store from disk (for batch operations).
    fn rebuild_graph(&self) {
        let new_graph = GraphStore::build_from_directory(&self.pkb_root);
        *self.graph.write() = new_graph;
    }

    /// Incremental graph update after a single file changed.
    /// Re-parses only the changed file, then rebuilds edges/metrics from existing nodes.
    /// Falls back to full rebuild if parsing fails.
    fn rebuild_graph_for_file(&self, abs_path: &std::path::Path) {
        if let Some(doc) = crate::pkb::parse_file_relative(abs_path, &self.pkb_root) {
            let node = crate::graph::GraphNode::from_pkb_document(&doc);
            let mut nodes = self.graph.read().nodes_cloned();
            nodes.insert(node.id.clone(), node);
            let new_graph = GraphStore::rebuild_from_nodes(nodes, &self.pkb_root);
            *self.graph.write() = new_graph;
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", abs_path);
            self.rebuild_graph();
        }
    }

    /// Incremental graph update after a node is removed.
    fn rebuild_graph_remove(&self, id: &str) {
        let mut nodes = self.graph.read().nodes_cloned();
        nodes.remove(id);
        let new_graph = GraphStore::rebuild_from_nodes(nodes, &self.pkb_root);
        *self.graph.write() = new_graph;
    }

    /// Save the vector store to disk with a non-blocking lock.
    /// If another process holds the lock, logs a warning and skips the save.
    fn save_store(&self) {
        match VectorStore::acquire_lock(&self.db_path) {
            Ok(mut lock) => match lock.try_write() {
                Ok(_guard) => {
                    if let Err(e) = self.store.read().save(&self.db_path) {
                        tracing::error!("Failed to save vector store: {e}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    tracing::info!("Vector store lock held by another process — disk save deferred (in-memory index updated, source file written)");
                }
                Err(e) => {
                    tracing::error!("Failed to acquire write lock for save: {e}");
                }
            },
            Err(e) => {
                tracing::error!("Failed to open lock file for save: {e}");
            }
        }
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

        // If the path doesn't exist on disk, try flexible ID resolution via the graph
        let path = if !path.exists() {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(path_str) {
                self.abs_path(&node.path)
            } else {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("File not found: {}", path.display())),
                    data: None,
                });
            }
        } else {
            path
        };

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
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let store = self.store.read();
        let results = store.list_documents(tag, doc_type, status, &self.pkb_root);
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
        let include_subtasks = args
            .get("include_subtasks")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = limit * 3;
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
                || r.doc_type.as_deref() == Some("goal")
                || r.doc_type.as_deref() == Some("subtask");

            if !is_task {
                continue;
            }
            if !include_subtasks && r.doc_type.as_deref() == Some("subtask") {
                continue;
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

        // Hierarchy validation: tasks must have a parent
        if fields.parent.is_none() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: parent. Tasks must have a parent node. \
                     Only goal, learn, and project types can be root-level.",
                ),
                data: None,
            });
        }

        // Validate parent exists in the PKB graph
        {
            let graph = self.graph.read();
            if let Some(ref parent_id) = fields.parent {
                if graph.resolve(parent_id).is_none() {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Parent '{}' not found in PKB. Create the parent node first or verify the ID.",
                            parent_id
                        )),
                        data: None,
                    });
                }
            }
        }

        let warnings: Vec<String> = Vec::new();

        let path =
            crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create task: {e}")),
                data: None,
            })?;

        // Index the new file (with relative path for portable storage)
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            self.save_store();
        }

        // Incremental graph update for the new file
        self.rebuild_graph_for_file(&path);

        let mut msg = format!("Task created: `{}`", path.display());
        if !warnings.is_empty() {
            msg.push_str("\n\nHierarchy warnings:\n");
            for w in &warnings {
                msg.push_str(&format!("- {}\n", w));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    fn handle_create_subtask(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let parent_id = args
            .get("parent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: parent_id"),
                data: None,
            })?;
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        // Validate parent exists
        {
            let graph = self.graph.read();
            if graph.resolve(parent_id).is_none() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Parent task not found: {parent_id}")),
                    data: None,
                });
            }
        }

        let fields = crate::document_crud::SubtaskFields {
            parent_id: parent_id.to_string(),
            title: title.to_string(),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
        };

        let path =
            crate::document_crud::create_subtask(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create sub-task: {e}")),
                data: None,
            })?;

        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            self.save_store();
        }
        self.rebuild_graph_for_file(&path);

        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Sub-task created: `{id}` at `{}`",
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
        if let Some(ref due) = node.due {
            output.push_str(&format!("**Due:** {due}\n"));
        }
        if let Some(ref source) = node.source {
            output.push_str(&format!("**Source:** {source}\n"));
        }
        if let Some(conf) = node.confidence {
            output.push_str(&format!("**Confidence:** {conf}\n"));
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
        if let Some(ref s) = node.supersedes {
            output.push_str("\n### Supersedes\n");
            let label = graph.get_node(s).map(|n| n.label.as_str()).unwrap_or("?");
            output.push_str(&format!("- `{s}` — {label}\n"));
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
                    let supersedes_note = if **edge_type == crate::graph::EdgeType::Supersedes {
                        " [SUPERSEDES THIS]"
                    } else {
                        ""
                    };
                    output.push_str(&format!(
                        "- `{}` [{:?}]{} {}\n",
                        source_node.id, edge_type, supersedes_note, source_node.label
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
        let detail = args
            .get("detail")
            .and_then(|v| v.as_str())
            .unwrap_or("chunk");

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let fetch_limit = limit * 2;
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
            let extract: std::borrow::Cow<'_, str> = match detail {
                "snippet" => std::borrow::Cow::Borrowed(&r.snippet),
                "full" => {
                    // Read full document from disk
                    match std::fs::read_to_string(&r.path) {
                        Ok(content) => std::borrow::Cow::Owned(content),
                        Err(_) => std::borrow::Cow::Borrowed(&r.chunk_text),
                    }
                }
                _ => std::borrow::Cow::Borrowed(&r.chunk_text), // "chunk" (default)
            };
            if !extract.is_empty() {
                output.push_str(&format!("\n> {}\n", extract.replace('\n', "\n> ")));
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

        let include_all = args
            .get("include_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let type_filter: Option<Vec<String>> =
            args.get("types").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
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
        } else if !include_all {
            // Default: only show actionable types (task, bug, feature, action, epic, project, goal)
            // and exclude completed nodes — matches graph_stats orphan_count definition
            let actionable: std::collections::HashSet<&str> =
                ["task", "bug", "feature", "action", "epic", "project", "goal"]
                    .into_iter()
                    .collect();
            orphans.retain(|n| {
                let is_actionable = n
                    .node_type
                    .as_deref()
                    .map(|t| actionable.contains(t))
                    .unwrap_or(false);
                let is_completed = crate::graph::is_completed(n.status.as_deref());
                is_actionable && !is_completed
            });
        }

        if orphans.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No orphan nodes found. All nodes have a valid parent.",
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
            "**{total} orphan nodes{type_desc}** (showing {showing})\n\nThese nodes have no valid parent.\n\n"
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

        // Build subtask list and inject a checklist section into the body
        let subtask_nodes: Vec<serde_json::Value> = node
            .subtasks
            .iter()
            .map(|sid| {
                if let Some(n) = graph.get_node(sid) {
                    serde_json::json!({
                        "id": sid,
                        "title": n.label,
                        "status": n.status,
                    })
                } else {
                    serde_json::json!({ "id": sid })
                }
            })
            .collect();

        // Sort subtasks by numeric suffix so they appear in order
        let mut subtask_nodes_sorted = subtask_nodes;
        subtask_nodes_sorted.sort_by(|a, b| {
            let parse_n = |v: &serde_json::Value| -> u32 {
                v.get("id")
                    .and_then(|id| id.as_str())
                    .and_then(|id| id.rsplit('.').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            parse_n(a).cmp(&parse_n(b))
        });

        let body = if !subtask_nodes_sorted.is_empty() {
            let mut checklist = String::from("\n\n## Subtasks\n\n");
            for st in &subtask_nodes_sorted {
                let done = crate::graph::is_completed(
                    st.get("status").and_then(|s| s.as_str()),
                );
                let check = if done { "x" } else { " " };
                let title = st
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("<untitled>");
                let id = st.get("id").and_then(|i| i.as_str()).unwrap_or("");
                checklist.push_str(&format!("- [{check}] **{id}**: {title}\n"));
            }
            format!("{body}{checklist}")
        } else {
            body
        };

        let result = serde_json::json!({
            "frontmatter": frontmatter,
            "body": body,
            "path": abs_path.to_string_lossy(),
            "depends_on": depends_on,
            "blocks": blocks,
            "children": children,
            "subtasks": subtask_nodes_sorted,
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
            confidence: args.get("confidence").and_then(|v| v.as_f64()),
            supersedes: args
                .get("supersedes")
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
            self.save_store();
        }

        self.rebuild_graph_for_file(&path);

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
            confidence: args.get("confidence").and_then(|v| v.as_f64()),
            supersedes: args
                .get("supersedes")
                .and_then(|v| v.as_str())
                .map(String::from),
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
            self.save_store();
        }

        self.rebuild_graph_for_file(&path);

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
            self.save_store();
        }

        self.rebuild_graph_for_file(&abs_path);

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
        let node_id = node.id.clone();
        let rel_path = node.path.to_string_lossy().to_string();
        drop(graph); // release read lock before write operations

        crate::document_crud::delete_document(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        // Remove from vector store
        self.store.write().remove(&rel_path);
        self.save_store();

        // Incremental graph update — remove the deleted node
        self.rebuild_graph_remove(&node_id);

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

        let evidence = Self::require_evidence(
            args.get("completion_evidence").and_then(|v| v.as_str()),
        )?;

        let pr_url = args.get("pr_url").and_then(|v| v.as_str());

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
        if let Some(url) = pr_url {
            updates.insert(
                "pr_url".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }

        crate::document_crud::update_document(&abs_path, updates).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to complete task: {e}")),
            data: None,
        })?;

        // Append completion evidence to the document body
        Self::append_evidence(&abs_path, evidence, pr_url)?;

        // Re-index
        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            self.save_store();
        }

        self.rebuild_graph_for_file(&abs_path);

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

        let graph = self.graph.read();

        // Build path -> node index
        let path_map: std::collections::HashMap<String, &crate::graph::GraphNode> = graph
            .nodes()
            .map(|n| (self.abs_path(&n.path).to_string_lossy().to_string(), n))
            .collect();

        let memory_types = ["memory", "note", "insight", "observation"];
        let mut scored_results = Vec::new();

        for r in results {
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

            let path_str = r.path.to_string_lossy();
            let confidence = path_map
                .get(&*path_str)
                .and_then(|n| n.confidence)
                .unwrap_or(1.0);

            // Adjust score by confidence (tie-breaker)
            let combined_score = r.score + (confidence as f32 * 0.0001);
            scored_results.push((r, combined_score, confidence));
        }

        // Re-sort by combined score
        scored_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut output = String::new();
        let mut count = 0;

        for (r, _score, confidence) in scored_results {
            if count >= limit {
                break;
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3}, confidence: {:.2})\n",
                count, r.title, r.score, confidence
            ));

            // Check if superseded
            let path_str = r.path.to_string_lossy();
            if let Some(node) = path_map.get(&*path_str) {
                let incoming = graph.get_incoming_edges(&node.id);
                let superseders: Vec<_> = incoming
                    .iter()
                    .filter(|e| e.edge_type == crate::graph::EdgeType::Supersedes)
                    .collect();
                for edge in &superseders {
                    let superseder_label = graph
                        .get_node(&edge.source)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    output.push_str(&format!(
                        "⚠️ **SUPERSEDED BY:** `{}` ({})\n",
                        edge.source, superseder_label
                    ));
                }
            }

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
        let all = store.list_documents(None, type_filter, None, &self.pkb_root);

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
            .list_documents(None, None, None, &self.pkb_root)
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

        let project_prefix = {
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
                    node.node_type.clone().unwrap_or_else(|| "task".to_string())
                }
            }
        };

        // First pass: assign IDs to all subtasks and build title map for cross-references
        let mut subtask_ids: Vec<String> = Vec::with_capacity(subtasks.len());
        let mut title_to_id: HashMap<String, String> = HashMap::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        for subtask in subtasks {
            let title = subtask.get("title").and_then(|v| v.as_str()).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Each subtask must have a 'title'"),
                data: None,
            })?;

            let title_lower = title.to_lowercase();
            if title_to_id.contains_key(&title_lower) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Duplicate subtask title: '{}'. Sibling titles must be unique for cross-referencing.",
                        title
                    )),
                    data: None,
                });
            }

            let id = subtask
                .get("id")
                .and_then(|v| v.as_str())
                .map(crate::document_crud::sanitize_prefix)
                .unwrap_or_else(|| {
                    crate::graph::create_id(&project_prefix)
                });

            if !seen_ids.insert(id.clone()) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Duplicate subtask ID: '{}'. Subtask IDs must be unique within a batch.",
                        id
                    )),
                    data: None,
                });
            }

            subtask_ids.push(id.clone());
            title_to_id.insert(title_lower, id);
        }

        let mut created: Vec<(String, String)> = Vec::new();
        static ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^([a-z]+-[0-9a-f]{8})").expect("valid static regex")
        });

        // Second pass: create tasks with resolved dependencies
        for (i, subtask) in subtasks.iter().enumerate() {
            let title = subtask.get("title").and_then(|v| v.as_str()).unwrap(); // validated in first pass

            let mut depends_on: Vec<String> = subtask
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Resolve sibling cross-references
            for dep in depends_on.iter_mut() {
                // 1. Positional references: $1, $2, etc. (1-indexed)
                if dep.starts_with('$') {
                    if let Ok(idx) = dep[1..].parse::<usize>() {
                        if idx > 0 && idx <= subtask_ids.len() {
                            *dep = subtask_ids[idx - 1].clone();
                        }
                    }
                }
                // 2. Title references (exact case-insensitive match on sibling title)
                else if let Some(sid) = title_to_id.get(&dep.to_lowercase()) {
                    *dep = sid.clone();
                }
            }

            let fields = crate::document_crud::TaskFields {
                title: title.to_string(),
                id: Some(subtask_ids[i].clone()),
                parent: Some(parent_id.to_string()),
                priority: subtask
                    .get("priority")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                tags: subtask
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                depends_on,
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

        self.save_store();
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
                        let is_done = is_completed(child.status.as_deref());
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
        let status = args.get("status").and_then(|v| v.as_str());
        let priority = args
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let assignee = args.get("assignee").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let include_subtasks = args
            .get("include_subtasks")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

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

        if !include_subtasks {
            tasks.retain(|t| t.node_type.as_deref() != Some("subtask"));
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

        // JSON output mode
        if format.eq_ignore_ascii_case("json") {
            let json_tasks: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.task_id.as_deref().unwrap_or(&t.id),
                        "title": t.label,
                        "status": t.status.as_deref().unwrap_or("unknown"),
                        "priority": t.priority.unwrap_or(2),
                        "project": t.project,
                        "assignee": t.assignee,
                        "modified": t.modified,
                        "tags": t.tags,
                        "downstream_weight": t.downstream_weight,
                        "parent": t.parent,
                        "depends_on": t.depends_on,
                        "node_type": t.node_type,
                    })
                })
                .collect();
            let result = serde_json::json!({
                "total": total,
                "showing": tasks.len(),
                "tasks": json_tasks,
            });
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&result).unwrap_or_default(),
            )]));
        }

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

    fn handle_bulk_reparent(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: pattern (directory path or glob)"),
                data: None,
            })?;

        let parent_id = args
            .get("parent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: parent_id"),
                data: None,
            })?;

        // Verify the parent exists in the graph
        {
            let graph = self.graph.read();
            if graph.resolve(parent_id).is_none() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Parent not found in graph: {}. Create it first or check the ID.",
                        parent_id
                    )),
                    data: None,
                });
            }
        }

        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let results =
            crate::document_crud::bulk_reparent(&self.pkb_root, pattern, parent_id, dry_run)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("bulk_reparent failed: {e}")),
                    data: None,
                })?;

        let mut updated = 0usize;
        let mut skipped_self = 0usize;
        let mut skipped_already = 0usize;
        let mut details = Vec::new();

        for r in &results {
            match r {
                crate::document_crud::ReparentResult::Updated(p) => {
                    updated += 1;
                    details.push(format!("  updated: {}", p.display()));
                }
                crate::document_crud::ReparentResult::SkippedSelf(p) => {
                    skipped_self += 1;
                    details.push(format!("  skipped (is parent): {}", p.display()));
                }
                crate::document_crud::ReparentResult::SkippedAlreadyParented(p) => {
                    skipped_already += 1;
                    details.push(format!("  skipped (already parented): {}", p.display()));
                }
            }
        }

        // Rebuild graph after bulk update (unless dry run)
        if !dry_run && updated > 0 {
            self.rebuild_graph();
        }

        let mode = if dry_run { "DRY RUN" } else { "APPLIED" };
        let summary = format!(
            "bulk_reparent [{}]: {} updated, {} skipped (self), {} skipped (already parented), {} total files\n{}",
            mode,
            updated,
            skipped_self,
            skipped_already,
            results.len(),
            details.join("\n")
        );

        Ok(CallToolResult::success(vec![Content::text(summary)]))
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

        // When setting status to "done", require completion_evidence
        let setting_done = updates
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("done"))
            .unwrap_or(false);

        if setting_done {
            let evidence = updates
                .get("completion_evidence")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if evidence.trim().is_empty() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "completion_evidence is required when setting status to done. Describe what was done before completing this task.",
                    ),
                    data: None,
                });
            }
        }

        // Build update map, filtering out completion_evidence (stored in body, not frontmatter)
        let evidence_text = updates
            .get("completion_evidence")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let pr_url_text = updates
            .get("pr_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let update_map: std::collections::HashMap<String, serde_json::Value> = updates
            .iter()
            .filter(|(k, _)| k.as_str() != "completion_evidence")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        crate::document_crud::update_document(&path, update_map).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        // Append completion evidence to body when completing via update_task
        if let Some(evidence) = evidence_text {
            if setting_done && !evidence.trim().is_empty() {
                Self::append_evidence(&path, &evidence, pr_url_text.as_deref())?;
            }
        }

        // Re-index the updated file (with relative path for portable storage)
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let _ = self.store.write().upsert(&doc, &self.embedder);
            self.save_store();
        }

        // Incremental graph update for the changed file
        self.rebuild_graph_for_file(&path);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task updated: `{}`",
            path.display()
        ))]))
    }

    // =========================================================================
    // BATCH OPERATIONS
    // =========================================================================

    fn handle_batch_update(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let updates = args.get("updates").cloned().unwrap_or(JsonValue::Object(serde_json::Map::new()));
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        if filters.is_empty() && updates.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter and one update field required"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::update::batch_update(&graph, &self.pkb_root, &filters, &updates, dry_run);
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_batch_reparent(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let new_parent = args
            .get("new_parent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("new_parent is required"),
                data: None,
            })?
            .to_string();

        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let graph = self.graph.read();
        let summary = crate::batch_ops::reparent::batch_reparent(
            &graph, &self.pkb_root, &filters, &new_parent, dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_batch_archive(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(true); // default true!
        let reason = args.get("reason").and_then(|v| v.as_str());

        if filters.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter is required for batch archive"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::update::batch_archive(
            &graph, &self.pkb_root, &filters, reason, dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_merge_node(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let canonical_id = args
            .get("canonical_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("canonical_id is required"),
                data: None,
            })?
            .to_string();

        let source_ids: Vec<String> = args
            .get("source_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if source_ids.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("source_ids must contain at least one ID"),
                data: None,
            });
        }

        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let summary =
            crate::document_crud::merge_node(&self.pkb_root, &source_ids, &canonical_id, dry_run)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("merge_node failed: {e}")),
                    data: None,
                })?;

        if !dry_run && summary.nodes_archived > 0 {
            self.rebuild_graph();
        }

        let msg = format!(
            "merge_node{}: {} files updated, {} references redirected, {} node(s) archived{}",
            if dry_run { " (dry run)" } else { "" },
            summary.files_updated,
            summary.refs_redirected,
            summary.nodes_archived,
            if dry_run { " — no changes written" } else { "" },
        );
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    fn handle_graph_stats(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let graph = self.graph.read();
        let stats = crate::batch_ops::stats::graph_stats(&graph);
        drop(graph);

        let json = serde_json::to_string_pretty(&stats).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_task_summary(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let graph = self.graph.read();
        let ready = graph.ready_tasks();
        let blocked = graph.blocked_tasks();

        let mut by_priority: std::collections::HashMap<i32, usize> =
            std::collections::HashMap::new();
        for task in &ready {
            let p = task.priority.unwrap_or(2);
            *by_priority.entry(p).or_insert(0) += 1;
        }

        let summary = serde_json::json!({
            "ready": ready.len(),
            "blocked": blocked.len(),
            "by_priority": {
                "p0": by_priority.get(&0).copied().unwrap_or(0),
                "p1": by_priority.get(&1).copied().unwrap_or(0),
                "p2": by_priority.get(&2).copied().unwrap_or(0),
                "p3": by_priority.get(&3).copied().unwrap_or(0),
            }
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&summary).unwrap_or_default(),
        )]))
    }

    fn handle_find_duplicates(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let mode_str = args.get("mode").and_then(|v| v.as_str()).unwrap_or("both");
        let mode = crate::batch_ops::duplicates::DuplicateMode::from_str(mode_str);
        let title_threshold = args.get("title_threshold").and_then(|v| v.as_f64()).unwrap_or(0.7);
        let semantic_threshold = args.get("similarity_threshold").and_then(|v| v.as_f64()).unwrap_or(0.85);

        let graph = self.graph.read();
        let store = self.store.read();
        let report = crate::batch_ops::duplicates::find_duplicates(
            &graph, &store, &filters, mode, title_threshold, semantic_threshold,
        );
        drop(graph);
        drop(store);

        let json = serde_json::to_string_pretty(&report).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_batch_merge(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let canonical = args
            .get("canonical")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("canonical is required"),
                data: None,
            })?
            .to_string();

        let merge_ids: Vec<String> = args
            .get("merge_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        if merge_ids.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("merge_ids must contain at least one ID"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::duplicates::batch_merge(
            &graph, &self.pkb_root, &canonical, &merge_ids, dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_batch_create_epics(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let parent = args.get("parent").and_then(|v| v.as_str());
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let epics: Vec<crate::batch_ops::epics::EpicDef> = args
            .get("epics")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if epics.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("epics array is required and must not be empty"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::epics::batch_create_epics(
            &graph, &self.pkb_root, parent, &epics, dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    fn handle_batch_reclassify(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let new_type = args
            .get("new_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("new_type is required"),
                data: None,
            })?;
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        if filters.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter is required"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::reclassify::batch_reclassify(
            &graph, &self.pkb_root, &filters, new_type, dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.rebuild_graph();
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
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
            "task_search" => self.handle_task_search(&args),
            "get_network_metrics" => self.handle_get_network_metrics(&args),
            "create_task" => self.handle_create_task(&args),
            "create_subtask" => self.handle_create_subtask(&args),
            "create_memory" => self.handle_create_memory(&args),
            "create" => self.handle_create_document(&args),
            "append" => self.handle_append_to_document(&args),
            "delete" => self.handle_delete_document(&args),
            "complete_task" => self.handle_complete_task(&args),
            "list_tasks" => self.handle_list_tasks(&args),
            "get_task" => self.handle_get_task(&args),
            "update_task" => self.handle_update_task(&args),
            "bulk_reparent" => self.handle_bulk_reparent(&args),
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
            "batch_update" => self.handle_batch_update(&args),
            "batch_reparent" => self.handle_batch_reparent(&args),
            "batch_archive" => self.handle_batch_archive(&args),
            "graph_stats" => self.handle_graph_stats(&args),
            "task_summary" => self.handle_task_summary(&args),
            "find_duplicates" => self.handle_find_duplicates(&args),
            "batch_merge" => self.handle_batch_merge(&args),
            "merge_node" => self.handle_merge_node(&args),
            "batch_create_epics" => self.handle_batch_create_epics(&args),
            "batch_reclassify" => self.handle_batch_reclassify(&args),
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
                        "detail": { "type": "string", "description": "Result detail level: 'snippet' (300 chars), 'chunk' (full matching chunk, default), 'full' (entire document)", "enum": ["snippet", "chunk", "full"], "default": "chunk" }
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
                        "limit": { "type": "integer", "description": "Max results (default: all)" },
                        "offset": { "type": "integer", "description": "Skip first N results (default: 0)" }
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
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false." }
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
                "create_subtask",
                "Create a numbered sub-task attached to a parent task. Sub-tasks use dot-notation IDs (e.g. proj-deadbeef.1) and appear as a markdown checkbox checklist when the parent is retrieved via get_task. They can also be addressed individually. Use for checklist-style completion tracking within a task.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent_id": { "type": "string", "description": "ID of the parent task (e.g. proj-deadbeef)" },
                        "title": { "type": "string", "description": "Sub-task title" },
                        "body": { "type": "string", "description": "Optional markdown body" }
                    },
                    "required": ["parent_id", "title"]
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
                        "source": { "type": "string", "description": "Source context (e.g. session ID)" },
                        "confidence": { "type": "number", "description": "Confidence level (0.0 - 1.0)", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string", "description": "ID of memory this one replaces" }
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
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "source": { "type": "string", "description": "Source context" },
                        "due": { "type": "string", "description": "Due date" },
                        "confidence": { "type": "number", "description": "Confidence level (0.0 - 1.0)", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string", "description": "ID of document this one replaces" },
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
                "Mark a task as done. Requires completion_evidence describing what was done. Sets status to 'done', appends evidence to body, and re-indexes.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (supports flexible resolution: ID, filename stem, or title)" },
                        "completion_evidence": { "type": "string", "description": "What was done + outcome. Required — describe the work before completing." },
                        "pr_url": { "type": "string", "description": "Link to PR/commit (optional, for code tasks)" }
                    },
                    "required": ["id", "completion_evidence"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "list_tasks",
                "List tasks with filtering by project, status, priority, and assignee. Use status='ready' for actionable tasks sorted by priority + downstream weight, or status='blocked' to see blocked tasks with their blockers.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "description": "Filter by status. Special values: 'ready' (actionable leaf tasks), 'blocked' (tasks with unmet deps). Also: active, in_progress, done, etc." },
                        "priority": { "type": "integer", "description": "Filter by exact priority (0-4)" },
                        "assignee": { "type": "string", "description": "Filter by assignee" },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false — subtasks are hidden since they travel with their parent task." },
                        "format": { "type": "string", "enum": ["markdown", "json"], "description": "Output format. 'json' returns structured {total, showing, tasks[]} for programmatic use. Default: 'markdown'." }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "get_task",
                "Retrieve a task by ID. Returns frontmatter, body, path, and relationship context (depends_on, blocks, children, subtasks, parent with titles/statuses, downstream_weight, stakeholder_exposure). Sub-tasks (type=subtask) are injected as a markdown checkbox checklist in the body and listed in the 'subtasks' field.",
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
                "Update frontmatter fields on an existing task file. Auto-sets modified timestamp. When setting status to 'done', completion_evidence is required inside updates.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to task file" },
                        "id": { "type": "string", "description": "Document ID (alternative to path — uses flexible resolution)" },
                        "updates": { "type": "object", "description": "Fields to update (null to remove). When status='done', include completion_evidence (string) describing what was done." }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "bulk_reparent",
                "Bulk reparent: set parent field on all .md files matching a directory or glob pattern. Dry run by default — set dry_run=false to apply. Skips files that ARE the parent (by permalink/id) and files already parented correctly.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Directory path or glob pattern (e.g. 'archive/', 'tasks/*.md'). Relative to PKB root." },
                        "parent_id": { "type": "string", "description": "Parent ID to set on matching files" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: true). Set to false to apply.", "default": true }
                    },
                    "required": ["pattern", "parent_id"]
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
                "Batch creation of subtasks under a parent task. Supports sibling cross-references in 'depends_on' using '$1', '$2', etc. (1-indexed position) or by exact sibling title.",
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
                                    "body": { "type": "string" }
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
                "Find orphan nodes with no valid parent. By default shows only actionable types (task/bug/feature/action/epic/project/goal) excluding completed — matching graph_stats orphan_count. Use include_all=true or types filter to override.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: all). Set to 0 for unlimited." },
                        "types": { "type": "array", "items": { "type": "string" }, "description": "Filter by node type (e.g. [\"task\"], [\"task\", \"project\"]). Overrides default actionable-only filter." },
                        "include_all": { "type": "boolean", "description": "Include all node types (notes, memories, etc.) — default false." }
                    }
                }))
                .unwrap(),
            ),
            // ── Batch Operations ──────────────────────────────────────────
            Tool::new(
                "batch_update",
                "Update frontmatter fields across multiple tasks in one operation. Supports filtered selection (by project, priority, tags, age, etc.) or explicit ID lists. Use _add_tags/_remove_tags for array manipulation. Set dry_run=true to preview changes.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs (flexible resolution)" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "type": { "type": "string", "description": "Filter by document type" },
                        "older_than_days": { "type": "integer", "description": "Filter: created > N days ago" },
                        "stale_days": { "type": "integer", "description": "Filter: not modified in N days" },
                        "orphan": { "type": "boolean", "description": "Filter: no parent and no project" },
                        "title_contains": { "type": "string", "description": "Filter: title substring (case-insensitive)" },
                        "directory": { "type": "string", "description": "Filter: file path contains directory" },
                        "weight_gte": { "type": "integer", "description": "Filter: downstream weight >= N" },
                        "updates": { "type": "object", "description": "Fields to set (null to remove). Special keys: _add_tags, _remove_tags, _add_depends_on, _remove_depends_on" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "batch_reparent",
                "Move multiple tasks to a new parent in one operation. Use when restructuring the task graph — grouping flat tasks into epics, or reorganizing tasks between projects.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "title_contains": { "type": "string", "description": "Filter: title substring" },
                        "new_parent": { "type": "string", "description": "ID of new parent (flexible resolution)" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["new_parent"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "batch_archive",
                "Archive multiple tasks (set status=done). Dry-run by default for safety — set dry_run=false to execute. Warns about in_progress tasks and those with active children.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "older_than_days": { "type": "integer", "description": "Filter: created > N days ago" },
                        "stale_days": { "type": "integer", "description": "Filter: not modified in N days" },
                        "title_contains": { "type": "string", "description": "Filter: title substring" },
                        "reason": { "type": "string", "description": "Archive reason (appended to task body)" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "graph_stats",
                "Report on graph health: status/priority/type distributions, orphan count, stale tasks, disconnected epics, and more. Read-only, no mutations.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "task_summary",
                "Returns pre-computed counts of ready and blocked tasks, plus a per-priority breakdown of ready tasks. 'ready' = leaf tasks with claimable types (task/bug/feature), active status, and all dependencies met. Use this instead of list_tasks for dashboard counts and priority bars.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            ),
            // ── Phase 2: Deduplication & Restructuring ────────────────────
            Tool::new(
                "find_duplicates",
                "Detect potential duplicate tasks using title similarity and/or semantic embedding similarity. Returns clusters with suggested canonical task. Read-only.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "description": "Filter by status" },
                        "mode": { "type": "string", "description": "Detection mode: title, semantic, or both (default: both)" },
                        "title_threshold": { "type": "number", "description": "Title similarity threshold 0-1 (default: 0.7)" },
                        "similarity_threshold": { "type": "number", "description": "Semantic similarity threshold 0-1 (default: 0.85)" }
                    }
                }))
                .unwrap(),
            ),
            Tool::new(
                "batch_merge",
                "Merge duplicate tasks into a canonical task. Archives merged tasks with superseded_by, unions tags/deps, reparents children, updates backlinks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canonical": { "type": "string", "description": "ID of the task to keep" },
                        "merge_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of duplicates to merge into canonical" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["canonical", "merge_ids"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "merge_node",
                "Merge source nodes into a canonical node. Redirects all references (parent, depends_on, blocks, wikilinks, etc.) from each source ID to the canonical ID across the entire PKB, then archives each source node (status=done, superseded_by=canonical). Unlike batch_merge, preserves source node files as archived records and performs complete reference updates including wikilinks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canonical_id": { "type": "string", "description": "ID of the node to merge into (must already exist)" },
                        "source_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of nodes to merge into canonical and archive" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["canonical_id", "source_ids"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "batch_create_epics",
                "Create multiple epic containers and reparent existing tasks under them. Primary tool for structuring a flat task list into organized groups.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent": { "type": "string", "description": "Parent for all new epics" },
                        "epics": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "id": { "type": "string" },
                                    "priority": { "type": "integer" },
                                    "task_ids": { "type": "array", "items": { "type": "string" } },
                                    "depends_on": { "type": "array", "items": { "type": "string" } },
                                    "body": { "type": "string" }
                                },
                                "required": ["title", "task_ids"]
                            },
                            "description": "Array of epic definitions with title and task_ids to reparent"
                        },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["epics"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "batch_reclassify",
                "Change the type field of matching tasks and move files to the correct subdirectory. Use for fixing memories filed as tasks and vice versa.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "type": { "type": "string", "description": "Filter by current type" },
                        "title_contains": { "type": "string", "description": "Filter by title substring" },
                        "new_type": { "type": "string", "description": "New document type (task, memory, note, knowledge, project, epic, goal)" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["new_type"]
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
            instructions: Some({
                let mut instructions = String::from(
                    "PKB Search — semantic search + task graph over personal knowledge base. \
                     26 tools: search, get_document, list_documents, \
                     task_search, get_network_metrics, create_task, create_memory, \
                     create, append, delete, complete_task, list_tasks, \
                     get_task, update_task, bulk_reparent, retrieve_memory, search_by_tag, \
                     list_memories, delete_memory, decompose_task, \
                     get_dependency_tree, get_task_children, \
                     pkb_context, pkb_trace, pkb_orphans, task_summary.",
                );
                if self.stale_count > 0 {
                    instructions.push_str(&format!(
                        " WARNING: Index is stale — {} document(s) need re-indexing. \
                         Search results may be incomplete or outdated. \
                         Run `pkb reindex` to update.",
                        self.stale_count
                    ));
                }
                instructions
            }),
        }
    }
}
