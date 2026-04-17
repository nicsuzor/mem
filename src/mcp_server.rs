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

    /// Incremental graph update after a single file changed, given an already-parsed document.
    /// This avoids re-reading/re-parsing the file when the caller already has a `PkbDocument`.
    fn rebuild_graph_for_pkb_document(&self, doc: &crate::pkb::PkbDocument) {
        let abs_path = self.abs_path(&doc.path);
        let node = crate::graph::GraphNode::from_pkb_document(doc);
        let mut nodes = self.graph.read().nodes_cloned();

        // Remove any existing node(s) that correspond to the same file path.
        // This handles cases where the frontmatter `id` changes for a given file,
        // ensuring we don't keep stale nodes/edges for the old id.
        nodes.retain(|_, existing_node| {
            self.abs_path(&existing_node.path) != abs_path
        });

        nodes.insert(node.id.clone(), node);
        let new_graph = GraphStore::rebuild_from_nodes(nodes, &self.pkb_root);
        *self.graph.write() = new_graph;
    }

    /// Incremental graph update after a node is removed.
    fn rebuild_graph_remove(&self, id: &str) {
        let mut nodes = self.graph.read().nodes_cloned();
        nodes.remove(id);
        let new_graph = GraphStore::rebuild_from_nodes(nodes, &self.pkb_root);
        *self.graph.write() = new_graph;
    }

    /// Check whether the index file lock is available (no reindex in progress).
    fn index_lock_available(&self) -> bool {
        match VectorStore::acquire_lock(&self.db_path) {
            Ok(mut lock) => match lock.try_write() {
                Ok(_guard) => true,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
                Err(_) => false,
            },
            Err(_) => false,
        }
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

    /// Index a document into the vector store if the index is not locked by a
    /// reindex. When a reindex is in progress the markdown file is already
    /// written and the graph already updated — the reindex will pick up the
    /// new/changed file, so we can safely skip the expensive embedding step.
    fn try_upsert_document(&self, doc: &crate::pkb::PkbDocument) {
        if !self.index_lock_available() {
            tracing::info!(
                "Index locked by another process — skipping in-memory upsert for {}",
                doc.path.display()
            );
            return;
        }
        let _ = self.store.write().upsert(doc, &self.embedder);
        self.save_store();
    }

    /// Remove a document from the vector store if the index is not locked.
    fn try_remove_document(&self, rel_path: &str) {
        if !self.index_lock_available() {
            tracing::info!(
                "Index locked by another process — skipping in-memory remove for {rel_path}"
            );
            return;
        }
        self.store.write().remove(rel_path);
        self.save_store();
    }

    // =========================================================================
    // SEARCH & DOCUMENT TOOLS
    // =========================================================================

    fn handle_get_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        // Accept `id` (preferred) or `path` (legacy) — both resolve via the graph
        let query = args
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("path").and_then(|v| v.as_str()))
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id (or path)"),
                data: None,
            })?;

        // Try ID/flexible resolution first (covers IDs, filename stems, titles, permalinks)
        let path = {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(query) {
                self.abs_path(&node.path)
            } else {
                // Fall back to treating the value as a literal path
                self.resolve_path(query)
            }
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
            let is_task = r
                .doc_type
                .as_deref()
                .map(|t| crate::graph_store::ACTIONABLE_TYPES.contains(&t))
                .unwrap_or(false);

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
                if node.uncertainty > 0.0 || node.criticality > 0.0 || node.scope > 0 {
                    output.push_str(&format!(
                        "**Metrics:** scope={} uncertainty={:.2} criticality={:.2}\n",
                        node.scope, node.uncertainty, node.criticality
                    ));
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
            effort: args.get("effort").and_then(|v| v.as_str()).map(String::from),
            consequence: args
                .get("consequence")
                .and_then(|v| v.as_str())
                .map(String::from),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            stakeholder: args
                .get("stakeholder")
                .and_then(|v| v.as_str())
                .map(String::from),
            waiting_since: args
                .get("waiting_since")
                .and_then(|v| v.as_str())
                .map(String::from),
            due: args
                .get("due")
                .and_then(|v| v.as_str())
                .map(String::from),
            project: args
                .get("project")
                .and_then(|v| v.as_str())
                .map(String::from),
            task_type: args
                .get("type")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
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

        // Incremental graph update for the new file
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            self.rebuild_graph();
        }

        // Extract ID from filename stem (e.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4")
        let task_id = path
            .file_stem()
            .map(|s| {
                let stem = s.to_string_lossy();
                // Match standard ID pattern: prefix-hexchars
                static RE: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-z]+-[0-9a-f]+").unwrap());
                RE.find(&stem)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| stem.to_string())
            })
            .unwrap_or_default();

        // Return structured JSON matching get_task shape
        let get_args = serde_json::json!({ "id": task_id });
        self.handle_get_task(&get_args)
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
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            self.rebuild_graph();
        }

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
        if !node.goals.is_empty() {
            output.push_str(&format!("**Goals:** {}\n", node.goals.join(", ")));
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
            // Default: only show actionable types and exclude completed nodes
            // — matches graph_stats orphan_count definition
            orphans.retain(|n| {
                let is_actionable = n
                    .node_type
                    .as_deref()
                    .map(|t| crate::graph_store::ACTIONABLE_TYPES.contains(&t))
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

        // Compute deadline metadata
        let today = chrono::Utc::now().date_naive();
        let days_until_due: Option<i64> = node.due.as_deref().and_then(|due| {
            let len = std::cmp::min(10, due.len());
            chrono::NaiveDate::parse_from_str(&due[..due.floor_char_boundary(len)], "%Y-%m-%d")
                .ok()
                .map(|due_date| (due_date - today).num_days())
        });
        let effort_days = node
            .effort
            .as_deref()
            .and_then(crate::graph_store::GraphStore::parse_effort_days)
            .unwrap_or(3);
        let urgency_ratio: Option<f64> = days_until_due.map(|d| {
            (effort_days as f64 / d.max(1) as f64).min(1.0)
        });

        let result = serde_json::json!({
            "frontmatter": frontmatter,
            "body": body,
            "path": abs_path.to_string_lossy(),
            "depends_on": depends_on,
            "blocks": blocks,
            "children": children,
            "subtasks": subtask_nodes_sorted,
            "parent": parent,
            "goals": node.goals,
            "priority": node.priority.unwrap_or(2),
            "effective_priority": node.effective_priority.unwrap_or(node.priority.unwrap_or(2)),
            "downstream_weight": node.downstream_weight,
            "stakeholder_exposure": node.stakeholder_exposure,
            "stakeholder": node.stakeholder,
            "waiting_since": node.waiting_since,
            "due": node.due,
            "focus_score": node.focus_score,
            "effort": node.effort,
            "consequence": node.consequence,
            "days_until_due": days_until_due,
            "urgency_ratio": urgency_ratio,
            "scope": node.scope,
            "uncertainty": node.uncertainty,
            "criticality": node.criticality,
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

        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            self.rebuild_graph();
        }

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
            stakeholder: args
                .get("stakeholder")
                .and_then(|v| v.as_str())
                .map(String::from),
            waiting_since: args
                .get("waiting_since")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        // Hierarchy validation: warn if task-like type without parent
        let mut warnings = Vec::new();
        let root_allowed = ["learn", "project"];
        let task_like = ["task", "epic"];
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

        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            self.rebuild_graph();
        }

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

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", abs_path);
            self.rebuild_graph();
        }

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

        // Incremental graph update — remove the deleted node
        self.rebuild_graph_remove(&node_id);

        // Remove from vector store (skipped if reindex holds the lock)
        self.try_remove_document(&rel_path);

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

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", abs_path);
            self.rebuild_graph();
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Completed: {} (`{}`)",
            label, id
        ))]))
    }

    /// Release a task to a handoff/terminal status with required summary.
    fn handle_release_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: status. Must be one of: merge_ready, done, review, blocked, cancelled.",
                ),
                data: None,
            })?;

        // Validate status enum with helpful suggestions
        let valid_statuses = ["merge_ready", "done", "review", "blocked", "cancelled"];
        if !valid_statuses.contains(&status) {
            let suggestion = match status {
                "complete" | "completed" => " Did you mean \"done\"?",
                "ready" | "merge-ready" => " Did you mean \"merge_ready\"?",
                "cancel" => " Did you mean \"cancelled\"?",
                _ => "",
            };
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Invalid status \"{status}\". Must be one of: merge_ready, done, review, blocked, cancelled.{suggestion}\n\
                     For non-terminal updates (priority, tags, assignee), use update_task instead."
                )),
                data: None,
            });
        }

        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: summary. Describe what was done before releasing this task.\n\
                     Example: release_task(id=\"task-abc\", status=\"merge_ready\", summary=\"Implemented X with Y\", pr_url=\"https://...\")",
                ),
                data: None,
            })?;
        if summary.trim().is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "summary cannot be empty. Describe what was done before releasing this task.",
                ),
                data: None,
            });
        }

        let pr_url = args.get("pr_url").and_then(|v| v.as_str());
        let branch = args.get("branch").and_then(|v| v.as_str());
        let blocker = args.get("blocker").and_then(|v| v.as_str());
        let reason = args.get("reason").and_then(|v| v.as_str());

        // Resolve task
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        // Check task is not already terminal
        let current_status = node.status.as_deref().unwrap_or("active");
        if current_status == "done" || current_status == "cancelled" {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Task \"{id}\" is already \"{current_status}\". Cannot release a completed task.\n\
                     To update a completed task's fields, use update_task."
                )),
                data: None,
            });
        }

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        drop(graph);

        // Build frontmatter updates
        let mut updates = std::collections::HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String(status.to_string()),
        );
        if let Some(url) = pr_url {
            updates.insert(
                "pr_url".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }
        if let Some(b) = branch {
            updates.insert(
                "branch".to_string(),
                serde_json::Value::String(b.to_string()),
            );
        }
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        updates.insert(
            "released_at".to_string(),
            serde_json::Value::String(now.clone()),
        );

        crate::document_crud::update_document(&abs_path, updates).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        // Build and append release evidence block
        let mut evidence_block = format!("\n\n## Release: {status}\n\n**{now}**\n\n{}", summary.trim());
        if let Some(url) = pr_url {
            evidence_block.push_str(&format!("\n\nPR: {url}"));
        }
        if let Some(b) = branch {
            evidence_block.push_str(&format!("\nBranch: {b}"));
        }
        if let Some(blk) = blocker {
            if !blk.trim().is_empty() {
                evidence_block.push_str(&format!("\n\nBlocker: {}", blk.trim()));
            }
        }
        if let Some(r) = reason {
            if !r.trim().is_empty() {
                evidence_block.push_str(&format!("\n\nReason: {}", r.trim()));
            }
        }
        evidence_block.push('\n');

        crate::document_crud::append_to_document(&abs_path, &evidence_block, None).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to append release evidence: {e}")),
            data: None,
        })?;

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", abs_path);
            self.rebuild_graph();
        }

        // Build response with soft warnings
        let mut warnings = Vec::new();
        if status == "merge_ready" && pr_url.is_none() {
            warnings.push("WARNING: No pr_url for merge_ready. Update the task with the PR URL when available.");
        }
        if status == "blocked" && blocker.map_or(true, |b| b.trim().is_empty()) {
            warnings.push("WARNING: No blocker description. Consider updating with what's blocking this task.");
        }
        if (status == "cancelled" || status == "review") && reason.map_or(true, |r| r.trim().is_empty()) {
            warnings.push("WARNING: No reason provided. Future you will want to know why.");
        }

        let mut response = format!("Released: {} → {} (`{}`)", label, status, id);
        for w in &warnings {
            response.push_str(&format!("\n{w}"));
        }

        Ok(CallToolResult::success(vec![Content::text(response)]))
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
                stakeholder: subtask
                    .get("stakeholder")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                waiting_since: subtask
                    .get("waiting_since")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                due: subtask
                    .get("due")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                effort: subtask
                    .get("effort")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                consequence: subtask
                    .get("consequence")
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

        self.rebuild_graph();

        // Index all created subtasks (skipped entirely if reindex holds the lock)
        if self.index_lock_available() {
            for (_, path_str) in &created {
                let path = std::path::Path::new(path_str);
                if let Some(doc) = crate::pkb::parse_file_relative(path, &self.pkb_root) {
                    let _ = self.store.write().upsert(&doc, &self.embedder);
                }
            }
            self.save_store();
        } else {
            tracing::info!("Index locked by another process — skipping upsert for {} decomposed subtasks", created.len());
        }

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
        let project = args.get("project").and_then(|v| v.as_str());
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
            tasks.retain(|t| t.effective_priority.unwrap_or(4) <= pri);
        }
        if let Some(a) = assignee {
            tasks.retain(|t| {
                t.assignee
                    .as_deref()
                    .map(|ag| ag.eq_ignore_ascii_case(a))
                    .unwrap_or(false)
            });
        }
        if let Some(p) = project {
            tasks.retain(|t| {
                t.project
                    .as_deref()
                    .map(|pr| pr.eq_ignore_ascii_case(p))
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
                        "effective_priority": t.effective_priority.unwrap_or(t.priority.unwrap_or(2)),
                        "project": t.project,
                        "assignee": t.assignee,
                        "modified": t.modified,
                        "tags": t.tags,
                        "downstream_weight": t.downstream_weight,
                        "parent": t.parent,
                        "depends_on": t.depends_on,
                        "node_type": t.node_type,
                        "scope": t.scope,
                        "uncertainty": t.uncertainty,
                        "criticality": t.criticality,
                        "due": t.due,
                        "effort": t.effort,
                        "consequence": t.consequence,
                        "focus_score": t.focus_score,
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
            let today = chrono::Utc::now().date_naive();
            out.push_str("| # | ID | Pri | Weight | Crit | U | Due | Title |\n|---|---|---|---|---|---|---|---|\n");
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
                let crit = if t.criticality > 0.0 {
                    format!("{:.2}", t.criticality)
                } else {
                    "-".to_string()
                };
                let unc = if t.uncertainty > 0.0 {
                    format!("{:.2}", t.uncertainty)
                } else {
                    "-".to_string()
                };
                let due_str = t.due.as_deref().map(|due| {
                    let len = std::cmp::min(10, due.len());
                    chrono::NaiveDate::parse_from_str(&due[..due.floor_char_boundary(len)], "%Y-%m-%d")
                        .ok()
                        .map(|due_date| {
                            let d = (due_date - today).num_days();
                            if d < 0 {
                                format!("{}d overdue", -d)
                            } else if d == 0 {
                                "today".to_string()
                            } else {
                                format!("{}d", d)
                            }
                        })
                        .unwrap_or_else(|| due[..due.floor_char_boundary(len)].to_string())
                }).unwrap_or_else(|| "-".to_string());
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    i + 1,
                    id,
                    t.priority.unwrap_or(2),
                    weight,
                    crit,
                    unc,
                    due_str,
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

        // Build update map, filtering out completion_evidence (stored in body, not frontmatter).
        // The "body" key is intentionally passed through: update_document() routes it to the
        // markdown body section instead of frontmatter (see FRONTMATTER_EXCLUDED_KEYS).
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

        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            self.rebuild_graph();
        }

        // Soft warning if setting a terminal status via update_task instead of release_task
        let terminal_statuses = ["merge_ready", "done", "review", "blocked", "cancelled"];
        let hint = updates
            .get("status")
            .and_then(|v| v.as_str())
            .filter(|s| terminal_statuses.contains(s))
            .map(|s| format!(
                "\nHINT: Use release_task(id=\"...\", status=\"{s}\", summary=\"...\") instead of \
                 update_task for status transitions. release_task captures work history."
            ))
            .unwrap_or_default();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task updated: `{}`{hint}",
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

    fn handle_graph_json(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let graph = self.graph.read();
        let json = graph.output_json().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate graph JSON: {e}")),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
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
        let all_tasks = graph.all_tasks();

        let mut by_priority: std::collections::HashMap<i32, usize> =
            std::collections::HashMap::new();
        for task in &ready {
            let p = task.priority.unwrap_or(2);
            *by_priority.entry(p).or_insert(0) += 1;
        }

        // Compute deadline counts across all tasks
        let today = chrono::Utc::now().date_naive();
        let mut overdue: usize = 0;
        let mut due_today: usize = 0;
        let mut due_this_week: usize = 0;
        for task in &all_tasks {
            if let Some(ref due) = task.due {
                let len = std::cmp::min(10, due.len());
                if let Ok(due_date) = chrono::NaiveDate::parse_from_str(
                    &due[..due.floor_char_boundary(len)],
                    "%Y-%m-%d",
                ) {
                    let days_until = (due_date - today).num_days();
                    if days_until < 0 {
                        overdue += 1;
                    } else if days_until == 0 {
                        due_today += 1;
                    } else if days_until <= 7 {
                        due_this_week += 1;
                    }
                }
            }
        }

        let summary = serde_json::json!({
            "ready": ready.len(),
            "blocked": blocked.len(),
            "by_priority": {
                "p0": by_priority.get(&0).copied().unwrap_or(0),
                "p1": by_priority.get(&1).copied().unwrap_or(0),
                "p2": by_priority.get(&2).copied().unwrap_or(0),
                "p3": by_priority.get(&3).copied().unwrap_or(0),
            },
            "deadlines": {
                "overdue": overdue,
                "due_today": due_today,
                "due_this_week": due_this_week,
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

    fn handle_list_prompts(&self) -> Result<ListPromptsResult, McpError> {
        let prompts = vec![
            Prompt::new(
                "find-task",
                "How do I find a task about X?",
            )
            .with_argument(PromptArgument::new("query", "The task to find").required(true)),
            Prompt::new(
                "explore-topic",
                "What do we know about X?",
            )
            .with_argument(PromptArgument::new("query", "The topic to explore").required(true)),
            Prompt::new(
                "navigate-graph",
                "What's connected to X?",
            )
            .with_argument(
                PromptArgument::new("id", "The node ID, title, or filename").required(true),
            ),
            Prompt::new("find-by-tag", "Show me everything tagged X").with_argument(
                PromptArgument::new("tag", "The tag to filter by").required(true),
            ),
        ];
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    fn handle_get_prompt(
        &self,
        request: GetPromptRequestParam,
    ) -> Result<GetPromptResult, McpError> {
        let name = request.name;
        let arguments = request.arguments.unwrap_or_default();

        match name.as_str() {
            "find-task" => {
                let query = arguments.get("query").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: query"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::user(format!(
                    "I want to find a task about '{}'. Please use 'task_search' to find it, then 'get_task' to read the most relevant one.",
                    query
                ))]))
            }
            "explore-topic" => {
                let query = arguments.get("query").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: query"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::user(format!(
                    "What do we know about '{}'? Please use 'search' to find documents, then 'get_document' for the full content of relevant ones.",
                    query
                ))]))
            }
            "navigate-graph" => {
                let id = arguments.get("id").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: id"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::user(format!(
                    "What's connected to '{}'? Please use 'pkb_context' to see its parents, children, and neighbours in the knowledge graph.",
                    id
                ))]))
            }
            "find-by-tag" => {
                let tag = arguments.get("tag").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: tag"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::user(format!(
                    "Show me everything tagged '{}'. Please use 'search_by_tag' with this tag.",
                    tag
                ))]))
            }
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown prompt: {}", name)),
                data: None,
            }),
        }
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
            "release_task" => self.handle_release_task(&args),
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
            "graph_json" => self.handle_graph_json(&args),
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
        _request: Option<PaginatedRequestParam>,
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
                "Read the full contents of a specific PKB document. Accepts an ID (preferred), filename stem, title, permalink, or path — uses flexible resolution.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID, filename stem, title, or permalink (preferred — uses flexible resolution)" },
                        "path": { "type": "string", "description": "Path to document (legacy — use id instead)" }
                    },
                    "anyOf": [
                        { "required": ["id"] },
                        { "required": ["path"] }
                    ]
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
                "Create a new task markdown file with YAML frontmatter. Returns structured JSON matching get_task shape (frontmatter, body, path, relationships).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Task title" },
                        "id": { "type": "string", "description": "Task ID (auto-generated if omitted)" },
                        "parent": { "type": "string", "description": "Parent task ID" },
                        "priority": { "type": "integer", "description": "0-4 (0=critical, 1=intended, 2=active, 3=planned, 4=backlog)" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "effort": { "type": "string", "description": "Effort duration string: '1d', '2h', '1w'. Parser converts to days." },
                        "consequence": { "type": "string", "description": "Narrative description of what happens if this task is not done or fails." },
                        "body": { "type": "string", "description": "Markdown body" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." },
                        "due": { "type": "string", "description": "Due date (ISO date, e.g. '2026-06-01')" },
                        "project": { "type": "string", "description": "Project identifier (e.g. 'aops')" },
                        "type": { "type": "string", "description": "Task type (default: 'task'). Also accepts: epic, bug, feature, learn, goal, project." },
                        "status": { "type": "string", "description": "Task status (default: 'active'). Also accepts: blocked, done, merge_ready, in_progress, etc." }
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
                        "priority": { "type": "integer", "description": "0-4 (0=critical, 1=intended, 2=active, 3=planned, 4=backlog)" },
                        "parent": { "type": "string", "description": "Parent document ID" },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "source": { "type": "string", "description": "Source context" },
                        "due": { "type": "string", "description": "Due date" },
                        "confidence": { "type": "number", "description": "Confidence level (0.0 - 1.0)", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string", "description": "ID of document this one replaces" },
                        "dir": { "type": "string", "description": "Override subdirectory placement" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." }
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
                "release_task",
                "Release a task after work is done. Use instead of update_task when transitioning to a handoff status \
                 (merge_ready, done, review, blocked, cancelled). Captures what was done so work history is never lost. \
                 All params are flat strings — no nested objects. \
                 For merge_ready: summary + pr_url. For done: summary of completion. \
                 For blocked: summary + blocker. For cancelled/review: summary + reason.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (flexible resolution: ID, filename stem, or title)" },
                        "status": {
                            "type": "string",
                            "enum": ["merge_ready", "done", "review", "blocked", "cancelled"],
                            "description": "Target status"
                        },
                        "summary": { "type": "string", "description": "What was done and outcome. 1-3 sentences minimum." },
                        "pr_url": { "type": "string", "description": "Pull request or commit URL (recommended for merge_ready)" },
                        "branch": { "type": "string", "description": "Git branch name (optional)" },
                        "blocker": { "type": "string", "description": "What is blocking this task (for status=blocked)" },
                        "reason": { "type": "string", "description": "Why cancelled or needs review (for status=cancelled/review)" }
                    },
                    "required": ["id", "status", "summary"]
                }))
                .unwrap(),
            ),
            Tool::new(
                "list_tasks",
                "List tasks with filtering by project, status, priority, and assignee. Use status='ready' for actionable tasks sorted by priority + downstream weight, or status='blocked' to see blocked tasks with their blockers.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project name (case-insensitive). Returns tasks whose computed project field (nearest ancestor with node_type=project) matches." },
                        "status": { "type": "string", "description": "Filter by status. Special values: 'ready' (actionable leaf tasks), 'blocked' (tasks with unmet deps). Also: active, in_progress, done, etc." },
                        "priority": { "type": "integer", "description": "Filter to tasks whose effective priority (own or any downstream task via blocks/parent) ≤ N. E.g. priority=0 returns every task that touches a P0, including its blockers." },
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
                "Retrieve a task by ID. Returns frontmatter, body, path, and relationship context (depends_on, blocks, children, subtasks, parent with titles/statuses, downstream_weight, stakeholder_exposure, stakeholder, waiting_since, focus_score). Sub-tasks (type=subtask) are injected as a markdown checkbox checklist in the body and listed in the 'subtasks' field.",
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
                "Update frontmatter fields on an existing task file. Use for non-terminal changes (priority, tags, assignee, body, etc.). \
                 For status transitions to merge_ready/done/review/blocked/cancelled, prefer release_task instead — it captures work history. \
                 IMPORTANT: `updates` must be a JSON object, NOT a string. \
                 Example: update_task(id=\"task-abc\", updates={\"priority\": 1, \"assignee\": \"polecat\"})",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to task file" },
                        "id": { "type": "string", "description": "Document ID (alternative to path — uses flexible resolution)" },
                        "updates": { "type": "object", "description": "JSON object of fields to update (null to remove a field). Common fields: status, assignee, body, pr_url, priority. When status='done', include completion_evidence (string). MUST be a JSON object like {\"status\": \"done\", \"completion_evidence\": \"what was done\"}, NOT a JSON string." }
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
                                    "body": { "type": "string" },
                                    "stakeholder": { "type": "string" },
                                    "waiting_since": { "type": "string" },
                                    "due": { "type": "string" }
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
                "graph_json",
                "Get the full knowledge graph as JSON for visualizations.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
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

        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(self.handle_list_prompts())
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        std::future::ready(self.handle_get_prompt(request))
    }

    fn get_info(&self) -> ServerInfo {
        let mut instructions = String::from(
            "PKB Search — semantic search + task graph over personal knowledge base. \
             38 tools for search, documents, tasks, and knowledge graph. \
             Use MCP prompts (find-task, explore-topic, navigate-graph, find-by-tag) for search pattern guidance.",
        );
        if self.stale_count > 0 {
            instructions.push_str(&format!(
                " WARNING: Index is stale — {} document(s) need re-indexing. \
                 Search results may be incomplete or outdated. \
                 Run `pkb reindex` to update.",
                self.stale_count
            ));
        }
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_server_info(Implementation::new("pkb", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::Embedder;
    use crate::graph_store::GraphStore;
    use crate::pkb::PkbDocument;
    use crate::vectordb::VectorStore;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn make_doc(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!(doc_type));
        fm.insert("status".to_string(), json!(status));
        fm.insert("id".to_string(), json!(id));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), json!(p));
        }
        if !depends_on.is_empty() {
            fm.insert("depends_on".to_string(), json!(depends_on));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
        }
    }

    fn make_doc_with_priority(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
        priority: i32,
        assignee: Option<&str>,
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!(doc_type));
        fm.insert("status".to_string(), json!(status));
        fm.insert("id".to_string(), json!(id));
        fm.insert("priority".to_string(), json!(priority));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), json!(p));
        }
        if let Some(a) = assignee {
            fm.insert("assignee".to_string(), json!(a));
        }
        if !depends_on.is_empty() {
            fm.insert("depends_on".to_string(), json!(depends_on));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
        }
    }

    /// Build a test graph with 3 projects and tasks under each, plus an orphan.
    ///
    /// ProjectAlpha:
    ///   - task-a1: active, priority 1, assignee "alice"
    ///   - task-a2: active, priority 2, assignee "bob" (depends on task-a1)
    ///   - task-a3: done, priority 1
    ///
    /// ProjectBeta:
    ///   - task-b1: active, priority 1 (leaf, no deps = ready)
    ///   - task-b2: active, priority 2 (depends on task-b1 = blocked)
    ///
    /// ProjectGamma:
    ///   - task-g1: active, priority 3
    ///
    /// Orphan (no project):
    ///   - task-orphan: active, priority 1
    fn build_project_test_graph() -> GraphStore {
        let docs = vec![
            // Project nodes
            make_doc("projects/proj-alpha.md", "ProjectAlpha", "project", "active", "proj-alpha", None, &[]),
            make_doc("projects/proj-beta.md", "ProjectBeta", "project", "active", "proj-beta", None, &[]),
            make_doc("projects/proj-gamma.md", "ProjectGamma", "project", "active", "proj-gamma", None, &[]),
            // ProjectAlpha tasks
            make_doc_with_priority("tasks/task-a1.md", "Alpha Task 1", "task", "active", "task-a1", Some("proj-alpha"), &[], 1, Some("alice")),
            make_doc_with_priority("tasks/task-a2.md", "Alpha Task 2", "task", "active", "task-a2", Some("proj-alpha"), &["task-a1"], 2, Some("bob")),
            make_doc_with_priority("tasks/task-a3.md", "Alpha Task 3", "task", "done", "task-a3", Some("proj-alpha"), &[], 1, None),
            // ProjectBeta tasks — task-b1 is a leaf with no deps (ready), task-b2 depends on task-b1
            make_doc_with_priority("tasks/task-b1.md", "Beta Task 1", "task", "active", "task-b1", Some("proj-beta"), &[], 1, None),
            make_doc_with_priority("tasks/task-b2.md", "Beta Task 2", "task", "active", "task-b2", Some("proj-beta"), &["task-b1"], 2, None),
            // ProjectGamma task
            make_doc_with_priority("tasks/task-g1.md", "Gamma Task 1", "task", "active", "task-g1", Some("proj-gamma"), &[], 3, None),
            // Orphan task (no parent, no project)
            make_doc_with_priority("tasks/task-orphan.md", "Orphan Task", "task", "active", "task-orphan", None, &[], 1, None),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb-project"))
    }

    fn build_test_server() -> PkbSearchServer {
        let graph = build_project_test_graph();
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            PathBuf::from("/tmp/test-pkb-project"),
            PathBuf::from("/tmp/test-pkb-project/db"),
            Arc::new(RwLock::new(graph)),
        )
    }

    /// Helper to extract task IDs from a list_tasks call result.
    fn extract_task_ids(result: &CallToolResult) -> Vec<String> {
        // Use JSON format for easier parsing
        let text = result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        // Parse the JSON output
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(tasks) = val.get("tasks").and_then(|t| t.as_array()) {
                return tasks
                    .iter()
                    .filter_map(|t| t.get("id").and_then(|id| id.as_str()).map(String::from))
                    .collect();
            }
        }
        vec![]
    }

    // ── AC1: project filter returns only matching tasks, no leakage ──

    #[test]
    fn test_list_tasks_project_filter_returns_only_matching() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        // Should contain only alpha tasks
        assert!(ids.contains(&"task-a1".to_string()), "should contain task-a1");
        assert!(ids.contains(&"task-a2".to_string()), "should contain task-a2");
        assert!(ids.contains(&"task-a3".to_string()), "should contain task-a3");
        // Should NOT contain tasks from other projects
        assert!(!ids.contains(&"task-b1".to_string()), "should not contain task-b1");
        assert!(!ids.contains(&"task-b2".to_string()), "should not contain task-b2");
        assert!(!ids.contains(&"task-g1".to_string()), "should not contain task-g1");
        assert!(!ids.contains(&"task-orphan".to_string()), "should not contain orphan");
    }

    // ── AC2: case-insensitive matching ──

    #[test]
    fn test_list_tasks_project_filter_case_insensitive() {
        let server = build_test_server();
        let lower = server
            .handle_list_tasks(&json!({"project": "projectalpha", "format": "json"}))
            .unwrap();
        let upper = server
            .handle_list_tasks(&json!({"project": "PROJECTALPHA", "format": "json"}))
            .unwrap();
        let mixed = server
            .handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"}))
            .unwrap();
        let ids_lower = extract_task_ids(&lower);
        let ids_upper = extract_task_ids(&upper);
        let ids_mixed = extract_task_ids(&mixed);
        assert_eq!(ids_lower, ids_mixed, "lowercase should match mixed case");
        assert_eq!(ids_upper, ids_mixed, "uppercase should match mixed case");
        assert!(!ids_lower.is_empty(), "should return results");
    }

    // ── AC3a: composes with status + priority + assignee ──

    #[test]
    fn test_list_tasks_project_composes_with_other_filters() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({
                "project": "ProjectAlpha",
                "status": "active",
                "priority": 1,
                "assignee": "alice",
                "format": "json"
            }))
            .unwrap();
        let ids = extract_task_ids(&result);
        // Only task-a1 matches: ProjectAlpha + active + priority 1 + assignee alice
        assert_eq!(ids, vec!["task-a1".to_string()]);
    }

    // ── AC3b: composes with status="ready" (different code path) ──

    #[test]
    fn test_list_tasks_project_composes_with_ready_status() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({
                "project": "ProjectAlpha",
                "status": "ready",
                "format": "json"
            }))
            .unwrap();
        let ids = extract_task_ids(&result);
        // task-a1 has a dependent (task-a2 depends on it), so task-a1 is not a leaf
        // task-a2 depends on task-a1 (unmet dep), so task-a2 is not ready
        // task-a3 is done, so not ready
        // The ready tasks in ProjectAlpha depend on the graph's ready_tasks() logic
        // Key assertion: no beta/gamma/orphan tasks leak through
        for id in &ids {
            assert!(
                id.starts_with("task-a"),
                "ready+project=ProjectAlpha should only return alpha tasks, got {}",
                id
            );
        }

        // Also verify that beta ready tasks are excluded
        let beta_result = server
            .handle_list_tasks(&json!({
                "project": "ProjectBeta",
                "status": "ready",
                "format": "json"
            }))
            .unwrap();
        let beta_ids = extract_task_ids(&beta_result);
        // task-b1 should be ready (leaf, no deps, active)
        // task-b2 depends on task-b1, so blocked
        for id in &beta_ids {
            assert!(
                id.starts_with("task-b"),
                "ready+project=ProjectBeta should only return beta tasks, got {}",
                id
            );
        }
    }

    // ── AC4: works for multiple distinct projects ──

    #[test]
    fn test_list_tasks_project_filter_multiple_projects() {
        let server = build_test_server();

        let alpha = server.handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"})).unwrap();
        let beta = server.handle_list_tasks(&json!({"project": "ProjectBeta", "format": "json"})).unwrap();
        let gamma = server.handle_list_tasks(&json!({"project": "ProjectGamma", "format": "json"})).unwrap();

        let alpha_ids = extract_task_ids(&alpha);
        let beta_ids = extract_task_ids(&beta);
        let gamma_ids = extract_task_ids(&gamma);

        assert!(!alpha_ids.is_empty(), "ProjectAlpha should have tasks");
        assert!(!beta_ids.is_empty(), "ProjectBeta should have tasks");
        assert!(!gamma_ids.is_empty(), "ProjectGamma should have tasks");

        // Verify no overlap
        for id in &alpha_ids {
            assert!(!beta_ids.contains(id), "alpha task {} should not be in beta", id);
            assert!(!gamma_ids.contains(id), "alpha task {} should not be in gamma", id);
        }
        for id in &beta_ids {
            assert!(!gamma_ids.contains(id), "beta task {} should not be in gamma", id);
        }
    }

    // ── AC5: non-existent project returns empty, not error ──

    #[test]
    fn test_list_tasks_project_filter_nonexistent_returns_empty() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"project": "NonExistentProject", "format": "json"}))
            .unwrap();
        // Should succeed (not error), and return empty or "no tasks found" message
        let text = result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        // Either empty JSON tasks array or "No tasks found" message
        let is_empty = text.contains("No tasks found") || text.contains("\"tasks\":[]") || text.contains("\"tasks\": []");
        assert!(is_empty, "non-existent project should return empty: {}", text);
    }

    // ── AC6: tool schema includes project parameter ──

    #[tokio::test]
    async fn test_list_tasks_schema_includes_project_parameter() {
        let server = build_test_server();
        let ctx = rmcp::service::RequestContext::<rmcp::service::RoleServer>::default();
        let tools_result = ServerHandler::list_tools(&server, None, ctx).await.unwrap();
        let list_tasks_tool = tools_result
            .tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist");
        let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
        assert!(
            schema.contains("\"project\""),
            "list_tasks schema should include 'project' parameter, got: {}",
            schema
        );
    }
}
