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
use rayon::prelude::*;

const GOAL_TYPE_ENUM: &[&str] = &["committed", "aspirational", "learning"];

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
    /// True when a background vector-store save is already scheduled. Used
    /// to coalesce burst writes: a subsequent `save_store` call while a save
    /// is in flight is skipped (the in-flight save will capture the latest
    /// state when it runs).
    save_pending: Arc<std::sync::atomic::AtomicBool>,
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
            save_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn with_stale_count(mut self, count: usize) -> Self {
        self.stale_count = count;
        self
    }

    /// Public sync entry to `handle_update_task` for benchmarking. Not part
    /// of the MCP API. Refs task-a4dcc039.
    #[doc(hidden)]
    pub fn bench_update_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_update_task(args)
    }

    /// Reconstruct an absolute path from a (possibly relative) graph node path.
    fn abs_path(&self, rel: &Path) -> PathBuf {
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.pkb_root.join(rel)
        }
    }

    /// Build an absolute-path -> &GraphNode index from the graph for O(1) lookups.
    fn build_path_to_node_map<'g>(
        &self,
        graph: &'g crate::graph_store::GraphStore,
    ) -> std::collections::HashMap<String, &'g crate::graph::GraphNode> {
        graph
            .nodes()
            .map(|n| (self.abs_path(&n.path).to_string_lossy().to_string(), n))
            .collect()
    }

    /// Build an absolute-path -> short ID map (task_id when present, else node id).
    fn build_path_to_id_map(
        &self,
        graph: &crate::graph_store::GraphStore,
    ) -> std::collections::HashMap<String, String> {
        graph
            .nodes()
            .map(|n| (self.abs_path(&n.path).to_string_lossy().to_string(), n.id.clone()))
            .collect()
    }

    /// Look up a node in a path_map using a path that may be relative or absolute.
    fn lookup_node<'a, 'g>(
        &self,
        path_map: &'a std::collections::HashMap<String, &'g crate::graph::GraphNode>,
        path: &Path,
    ) -> Option<&'g crate::graph::GraphNode> {
        path_map
            .get(self.abs_path(path).to_string_lossy().as_ref())
            .copied()
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
        let store = self.store.read();
        let files = crate::pkb::scan_directory_all(&self.pkb_root);
        let docs: Vec<crate::pkb::PkbDocument> = files
            .par_iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, &self.pkb_root))
            .collect();

        let new_graph = GraphStore::build_with_store(&docs, &self.pkb_root, &store);
        *self.graph.write() = new_graph;
    }

    /// Incremental graph update after a single file changed, given an already-parsed document.
    /// This avoids re-reading/re-parsing the file when the caller already has a `PkbDocument`.
    /// Uses the fast path (skips centrality recomputation).
    fn rebuild_graph_for_pkb_document(&self, doc: &crate::pkb::PkbDocument) {
        let abs_path = self.abs_path(&doc.path);
        let mut node = crate::graph::GraphNode::from_pkb_document(doc);

        let _t_clone = std::time::Instant::now();
        let mut nodes = self.graph.read().nodes_cloned();
        tracing::debug!(target: "perf::graph_rebuild", phase = "nodes_cloned", n_nodes = nodes.len(), elapsed_ms = _t_clone.elapsed().as_secs_f64() * 1000.0);

        // Carry over centrality scores from the prior node with the same id
        // so the fast-path rebuild doesn't zero them out.
        if let Some(old) = nodes.get(&node.id) {
            node.pagerank = old.pagerank;
            node.betweenness = old.betweenness;
        }

        // Remove any existing node(s) that correspond to the same file path.
        // This handles cases where the frontmatter `id` changes for a given file,
        // ensuring we don't keep stale nodes/edges for the old id.
        nodes.retain(|_, existing_node| {
            self.abs_path(&existing_node.path) != abs_path
        });

        nodes.insert(node.id.clone(), node);

        let store = self.store.read();
        let new_graph = GraphStore::rebuild_from_nodes_fast_with_store(nodes, &self.pkb_root, &store);
        *self.graph.write() = new_graph;
    }

    /// Incremental graph update after a node is removed.
    /// Uses the fast path (skips centrality recomputation).
    fn rebuild_graph_remove(&self, id: &str) {
        let mut nodes = self.graph.read().nodes_cloned();
        nodes.remove(id);
        let new_graph = GraphStore::rebuild_from_nodes_fast(nodes, &self.pkb_root);
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

    /// Save the vector store to disk asynchronously, off the write critical path.
    ///
    /// Uses a coalescing flag (`save_pending`) so that bursts of writes result
    /// in at most a single outstanding background save. The in-memory upsert
    /// has already happened; this only persists to disk.
    ///
    /// If the lock file is held by another process (e.g. a running reindex),
    /// logs and skips — the on-disk state will be refreshed when the other
    /// process releases. If no tokio runtime is present (e.g. a direct CLI
    /// caller), falls back to an inline save.
    fn save_store(&self) {
        use std::sync::atomic::Ordering;
        // Coalesce: if a background save is already scheduled, skip — it will
        // read the latest in-memory state when it runs.
        if self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }

        let store = self.store.clone();
        let db_path = self.db_path.clone();
        let pending = self.save_pending.clone();

        let do_save = move || {
            // Clear the flag BEFORE serializing: any concurrent write that
            // arrives during serialize will schedule a follow-up save (rather
            // than be silently dropped).
            pending.store(false, Ordering::SeqCst);
            match VectorStore::acquire_lock(&db_path) {
                Ok(mut lock) => match lock.try_write() {
                    Ok(_guard) => {
                        if let Err(e) = store.read().save(&db_path) {
                            tracing::error!("Failed to save vector store: {e}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        tracing::info!(
                            "Vector store lock held by another process — disk save deferred"
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to acquire write lock for save: {e}");
                    }
                },
                Err(e) => {
                    tracing::error!("Failed to open lock file for save: {e}");
                }
            }
        };

        // Offload serialize + file I/O to a blocking pool thread so the MCP
        // handler returns without waiting for disk.
        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                tokio::task::spawn_blocking(do_save);
            }
            Err(_) => {
                // No runtime (e.g. CLI test path) — save inline.
                do_save();
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
        let _t_upsert = std::time::Instant::now();
        let _ = self.store.write().upsert(doc, &self.embedder);
        tracing::debug!(target: "perf::vector", phase = "store_upsert_inmem", elapsed_ms = _t_upsert.elapsed().as_secs_f64() * 1000.0);
        let _t_save = std::time::Instant::now();
        self.save_store();
        tracing::debug!(target: "perf::vector", phase = "save_store_dispatch", elapsed_ms = _t_save.elapsed().as_secs_f64() * 1000.0);
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
        if args.get("path").is_some() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("The 'path' parameter is no longer supported. Please use 'id' instead (supports ID, filename stem, or title)."),
                data: None,
            });
        }

        let query = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        // Try ID/flexible resolution first (covers IDs, filename stems, titles, permalinks)
        let (path, label) = {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(query) {
                (self.abs_path(&node.path), node.label.clone())
            } else {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Document not found: {query}")),
                    data: None,
                });
            }
        };

        if !path.exists() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("File not found for ID '{query}': {}", path.display())),
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
            label,
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

        // Build path -> short ID map via graph
        let path_map = {
            let graph = self.graph.read();
            self.build_path_to_id_map(&graph)
        };

        let mut output =
            format!("**{total} documents found** (showing {showing}, offset {offset})\n\n");

        for r in &page {
            let id = path_map
                .get(&self.abs_path(&r.path).to_string_lossy().to_string())
                .cloned()
                .unwrap_or_else(|| r.id.clone().unwrap_or_else(|| r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            output.push_str(&format!(" — `{id}`"));
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

        // Optional `type` filter: either a single type ("epic") or a comma-
        // separated list ("epic,feature"). When set, only matching types are
        // returned. Recognised actionable types: project, epic, task, learn.
        let type_filter: Option<HashSet<String>> = args
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            });

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        // When a type filter is present, fetch more candidates so we still fill
        // the limit after filtering.
        let fetch_limit = if type_filter.is_some() {
            limit * 10
        } else {
            limit * 3
        };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root);

        let graph = self.graph.read();

        // Build path -> node index for O(1) lookups instead of O(n) per result
        let path_map = self.build_path_to_node_map(&graph);

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
            let is_subtask = r.doc_type.as_deref().map(|t| t.eq_ignore_ascii_case("subtask")).unwrap_or(false);
            let subtask_allowed = include_subtasks || type_filter.as_ref().map(|f| f.contains("subtask")).unwrap_or(false);
            if is_subtask && !subtask_allowed {
                continue;
            }
            if let Some(ref filter) = type_filter {
                let matches = r
                    .doc_type
                    .as_deref()
                    .map(|t| filter.iter().any(|f| t.eq_ignore_ascii_case(f)))
                    .unwrap_or(false);
                if !matches {
                    continue;
                }
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                count, r.title, r.score
            ));

            // O(1) path lookup via pre-built index
            if let Some(node) = self.lookup_node(&path_map, &r.path) {
                let id = node.task_id.as_deref().unwrap_or(&node.id);
                output.push_str(&format!("**ID:** `{id}`\n"));
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
        // Accept `title` (preferred) or `task_title` (alias — some skill docs use this name)
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("task_title").and_then(|v| v.as_str()))
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: project. Every task must declare a \
                     project (e.g. 'aops', 'mem', 'adhoc-sessions').",
                ),
                data: None,
            })?;
        // Surface silent-drop bugs at runtime: log which keys the caller passed and
        // which ones we don't recognise (so they get dropped before reaching disk).
        // task-16fe56e6 — friction was previously invisible because callers had no
        // way to tell the server had ignored their `project`/`type`/`status`/`body`.
        if let Some(obj) = args.as_object() {
            const KNOWN_KEYS: &[&str] = &[
                "title", "task_title", "id", "parent", "priority", "tags", "depends_on",
                "assignee", "complexity", "effort", "consequence", "severity", "goal_type",
                "body", "stakeholder", "waiting_since", "due", "project", "type", "status",
                "session_id", "issue_url", "follow_up_tasks", "release_summary", "contributes_to",
            ];
            let received: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
            let unknown: Vec<&str> = received
                .iter()
                .copied()
                .filter(|k| !KNOWN_KEYS.contains(k))
                .collect();
            tracing::info!(
                target: "pkb::create_task",
                title = %title,
                received_keys = ?received,
                unknown_keys = ?unknown,
                "create_task invocation"
            );
            if !unknown.is_empty() {
                tracing::warn!(
                    target: "pkb::create_task",
                    unknown_keys = ?unknown,
                    "create_task received unknown keys — these fields will NOT be written. \
                     Pass them via update_task or extend the create_task schema."
                );
            }
        }

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
            severity: args
                .get("severity")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            goal_type: args
                .get("goal_type")
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
            project: Some(project.to_string()),
            task_type: args
                .get("type")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
            session_id: args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            issue_url: args
                .get("issue_url")
                .and_then(|v| v.as_str())
                .map(String::from),
            follow_up_tasks: args
                .get("follow_up_tasks")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            release_summary: args
                .get("release_summary")
                .and_then(|v| v.as_str())
                .map(String::from),
            contributes_to: args
                .get("contributes_to")
                .and_then(|v| v.as_array())
                .map(|arr| arr.clone())
                .unwrap_or_default(),
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

        // Validate parent exists in the PKB graph. Rejects by default to
        // prevent silent orphan creation (task-89b2af87). Caller can pass
        // `allow_missing_parent: true` to downgrade to a warning.
        {
            let allow_missing = args
                .get("allow_missing_parent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let graph = self.graph.read();
            if let Some(ref parent_id) = fields.parent {
                if graph.resolve(parent_id).is_none() {
                    if allow_missing {
                        tracing::warn!(
                            "create_task: parent '{}' not found in PKB; proceeding because allow_missing_parent=true",
                            parent_id
                        );
                    } else {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Parent '{}' not found in PKB. Create the parent node first, fix the ID, or pass allow_missing_parent=true to override.",
                                parent_id
                            )),
                            data: None,
                        });
                    }
                }
                // Reject parent/child cycles. Only relevant when an explicit `id`
                // is supplied (an auto-generated id cannot already be a parent).
                if let Some(ref child_id) = fields.id {
                    if let Err(msg) = graph.would_create_parent_cycle(child_id, parent_id) {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(msg),
                            data: None,
                        });
                    }
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

        // Extract ID from filename stem (e.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4").
        // generate_id() always emits exactly 8 hex chars, so anchor the pattern to {8}
        // rather than `+` — keeps an accidental hex-shaped slug suffix from being absorbed.
        let task_id = path
            .file_stem()
            .map(|s| {
                let stem = s.to_string_lossy();
                static RE: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-z]+-[0-9a-f]{8}").unwrap());
                RE.find(&stem)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| stem.to_string())
            })
            .unwrap_or_default();

        // Return structured JSON matching get_task shape. If the post-create graph
        // lookup fails (graph rebuild raced or the file landed in an un-scanned
        // location), fail loudly with the file path — silently returning null
        // frontmatter (the original 2026-04-30 regression mode) hides the bug.
        let get_args = serde_json::json!({ "id": task_id });
        self.handle_get_task(&get_args).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "create_task wrote {} but the new node is not yet visible in the graph \
                 (id={task_id}). Underlying lookup error: {}. \
                 The file is on disk — retry get_task in a moment.",
                path.display(),
                e.message,
            )),
            data: None,
        })
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

        // Validate parent exists and has a project (subtasks inherit project from parent)
        {
            let graph = self.graph.read();
            let parent_node = graph.resolve(parent_id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Parent task not found: {parent_id}")),
                data: None,
            })?;

            let parent_has_project = crate::pkb::parse_file_relative(
                &self.abs_path(&parent_node.path),
                &self.pkb_root,
            )
            .and_then(|doc| doc.frontmatter)
            .and_then(|fm| {
                fm.get("project")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
            })
            .unwrap_or(false);

            if !parent_has_project {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Parent task '{parent_id}' has no `project` field. Subtasks \
                         inherit the parent's project — set the parent's project first."
                    )),
                    data: None,
                });
            }
            // Optional caller-supplied `id` — reject parent/child cycles.
            if let Some(child_id) = args.get("id").and_then(|v| v.as_str()) {
                if let Err(msg) = graph.would_create_parent_cycle(child_id, parent_id) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
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
        let path_map = self.build_path_to_node_map(&graph);

        let mut scored: Vec<_> = results
            .iter()
            .map(|r| {
                let node = self.lookup_node(&path_map, &r.path);
                let node_id = node.map(|n| n.id.clone());

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
            let node = self.lookup_node(&path_map, &r.path);
            let display_id = node
                .map(|n| n.task_id.as_deref().unwrap_or(&n.id))
                .unwrap_or("?");

            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                i + 1,
                r.title,
                score
            ));
            output.push_str(&format!("**ID:** `{display_id}`\n"));
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
            let cid = node.task_id.as_deref().unwrap_or(&node.id);
            output.push_str(&format!(" — `{cid}`\n"));
        }

        if total > max {
            output.push_str(&format!("\n...and {} more\n", total - max));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn handle_get_semantic_neighbors(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let threshold = args.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.85);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let graph = self.graph.read();
        let store = self.store.read();

        let neighbors = crate::batch_ops::similarity::find_neighbors(id, &graph, &store, threshold, limit);

        if neighbors.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No semantic neighbors found for '{}' above threshold {:.2}.",
                id, threshold
            ))]));
        }

        let mut output = format!(
            "## Semantic Neighbors for `{id}` (threshold {:.2})\n\n**{} neighbor(s) found**\n\n",
            threshold,
            neighbors.len()
        );

        for n in neighbors {
            let edge = if n.is_explicit_edge { " (explicitly linked)" } else { "" };
            output.push_str(&format!("- **{}** (`{}`): score {:.3}{}\n", n.title, n.id, n.score, edge));
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
            .and_then(crate::graph::parse_effort_days)
            .unwrap_or(3);
        let urgency_ratio: Option<f64> = days_until_due.map(|d| {
            (effort_days as f64 / d.max(1) as f64).min(1.0)
        });

        let contributes_to: Vec<serde_json::Value> = node
            .contributes_to
            .iter()
            .map(|c| {
                let mut res = serde_json::to_value(c).unwrap_or_else(|_| serde_json::json!({}));
                let target_id = c.resolved_to.as_deref().unwrap_or(&c.to);
                if let Some(n) = graph.resolve(target_id) {
                    if let Some(obj) = res.as_object_mut() {
                        obj.insert("title".to_string(), serde_json::json!(n.label));
                        if let Some(ref s) = n.status {
                            obj.insert("status".to_string(), serde_json::json!(s));
                        }
                    }
                }
                res
            })
            .collect();

        let result = serde_json::json!({
            "id": node.task_id.as_deref().unwrap_or(&node.id),
            "frontmatter": frontmatter,
            "body": body,
            "depends_on": depends_on,
            "blocks": blocks,
            "children": children,
            "subtasks": subtask_nodes_sorted,
            "parent": parent,
            "goals": node.goals,
            "contributes_to": contributes_to,
            "contributed_by": node.contributed_by,
            "follow_up_tasks": node.follow_up_tasks,
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
            "severity": node.severity,
            "goal_type": node.goal_type,
            "edge_template": node.edge_template,
            "parse_warnings": node.parse_warnings,
            "days_until_due": days_until_due,
            "urgency_ratio": urgency_ratio,
            "urgency": node.urgency,
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
            severity: args
                .get("severity")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            goal_type: args
                .get("goal_type")
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
            contributes_to: args
                .get("contributes_to")
                .and_then(|v| v.as_array())
                .map(|arr| arr.clone())
                .unwrap_or_default(),
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
        if args.get("path").is_some() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("The 'path' parameter is no longer supported. Please use 'id' instead."),
                data: None,
            });
        }

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
    /// Create an ad-hoc task for a session release when no ID is provided.
    fn create_adhoc_task(&self, args: &JsonValue) -> Result<String, McpError> {
        let summary = args.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        
        // Truncate summary to 200 chars for title
        let mut title = summary.trim().replace('\n', " ");
        if title.len() > 200 {
            let end = title.floor_char_boundary(200);
            let last_space = title[..end].rfind(' ').unwrap_or(end);
            title.truncate(last_space);
        }
        if title.is_empty() {
            title = "Ad-hoc Session Task".to_string();
        }

        // Ensure adhoc-sessions root exists
        crate::document_crud::ensure_adhoc_sessions_root(&self.pkb_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to ensure adhoc-sessions root: {e}")),
            data: None,
        })?;

        // Rebuild graph if adhoc-sessions is missing from memory
        {
            let graph = self.graph.read();
            if graph.resolve("adhoc-sessions").is_none() {
                drop(graph);
                self.rebuild_graph();
            }
        }

        let fields = crate::document_crud::TaskFields {
            title,
            parent: Some("adhoc-sessions".to_string()),
            project: Some("adhoc-sessions".to_string()),
            tags: vec!["adhoc".to_string(), "session-release".to_string()],
            session_id: args.get("session_id").and_then(|v| v.as_str()).map(String::from),
            issue_url: args.get("issue_url").and_then(|v| v.as_str()).map(String::from),
            release_summary: args.get("release_summary").and_then(|v| v.as_str()).map(String::from),
            follow_up_tasks: args.get("follow_up_tasks")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        let path = crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create ad-hoc task: {e}")),
            data: None,
        })?;

        // Extract ID from filename stem (e.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4")
        static ID_RE: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-z]+-[0-9a-f]{8}").unwrap());
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                ID_RE.find(s)
                    .map(|m| m.as_str())
                    .unwrap_or(s)
            })
            .unwrap_or("task-unknown")
            .to_string();

        // Update graph and vector DB
        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            self.rebuild_graph();
        }
        Ok(id)
    }

    fn handle_release_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let mut created_id = None;
        let id_val = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => {
                let nid = self.create_adhoc_task(args)?;
                created_id = Some(nid.clone());
                nid
            }
        };
        let id = &id_val;

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
        let session_id_arg = args.get("session_id").and_then(|v| v.as_str());
        let issue_url = args.get("issue_url").and_then(|v| v.as_str());
        let follow_up_tasks = args.get("follow_up_tasks").and_then(|v| v.as_array());
        let release_summary = args.get("release_summary").and_then(|v| v.as_str());

        // Resolve task and validate follow_up_tasks
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let mut follow_up_ids = Vec::new();
        if let Some(arr) = follow_up_tasks {
            let mut unresolved = Vec::new();
            for v in arr {
                if let Some(id_str) = v.as_str() {
                    if graph.resolve(id_str).is_none() {
                        unresolved.push(id_str.to_string());
                    } else {
                        follow_up_ids.push(id_str.to_string());
                    }
                }
            }
            if !unresolved.is_empty() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Unresolved follow_up_tasks: {}",
                        unresolved.join(", ")
                    )),
                    data: None,
                });
            }
        }

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

        // session_id: arg or env $AOPS_SESSION_ID
        let final_session_id = session_id_arg
            .map(|s| s.to_string())
            .or_else(|| std::env::var("AOPS_SESSION_ID").ok());
        if let Some(sid) = final_session_id {
            updates.insert("session_id".to_string(), serde_json::Value::String(sid));
        }

        if let Some(url) = issue_url {
            updates.insert("issue_url".to_string(), serde_json::Value::String(url.to_string()));
        }

        if !follow_up_ids.is_empty() {
            updates.insert(
                "follow_up_tasks".to_string(),
                serde_json::Value::Array(
                    follow_up_ids
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }

        if let Some(rs) = release_summary {
            updates.insert(
                "release_summary".to_string(),
                serde_json::Value::String(rs.to_string()),
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
        if let Some(rs) = release_summary {
            if rs.len() > 500 {
                warnings.push("WARNING: release_summary exceeds 500 characters.");
            }
        }

        let mut response_text = format!("Released: {} → {} (`{}`)", label, status, id);
        for w in &warnings {
            response_text.push_str(&format!("\n{w}"));
        }

        if let Some(nid) = created_id {
            let json = serde_json::json!({
                "status": "success",
                "message": response_text,
                "created_id": nid,
            });
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&json).unwrap_or_default(),
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(response_text)]))
        }
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
        let path_map = self.build_path_to_node_map(&graph);

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

            let confidence = self
                .lookup_node(&path_map, &r.path)
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

            // Check if superseded and get display ID
            let display_id = if let Some(node) = self.lookup_node(&path_map, &r.path) {
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
                node.task_id.as_deref().unwrap_or(&node.id).to_string()
            } else {
                "?".to_string()
            };
            output.push_str(&format!("**ID:** `{display_id}`\n"));
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

        // Build path -> ID map
        let graph = self.graph.read();
        let path_map = self.build_path_to_id_map(&graph);

        for r in &matching {
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            output.push_str(&format!(" ({})", r.tags.join(", ")));
            let id = path_map
                .get(self.abs_path(&r.path).to_string_lossy().as_ref())
                .cloned()
                .unwrap_or_else(|| r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            output.push_str(&format!(" — `{id}`\n"));
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

        // Build path -> ID map
        let graph = self.graph.read();
        let path_map = self.build_path_to_id_map(&graph);

        for r in &memories {
            output.push_str(&format!("- **{}**", r.title));
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            let id = path_map
                .get(self.abs_path(&r.path).to_string_lossy().as_ref())
                .cloned()
                .unwrap_or_else(|| r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            output.push_str(&format!(" — `{id}`\n"));
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

        let (project_prefix, parent_project) = {
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
                    let prefix = node.node_type.clone().unwrap_or_else(|| "task".to_string());
                    // Read parent's raw frontmatter `project` field so subtasks can inherit it.
                    // GraphNode.project is a computed ancestor label, not the frontmatter value.
                    let parent_project = crate::pkb::parse_file_relative(
                        &self.abs_path(&node.path),
                        &self.pkb_root,
                    )
                    .and_then(|doc| doc.frontmatter)
                    .and_then(|fm| {
                        fm.get("project")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    });
                    (prefix, parent_project)
                }
            }
        };

        // Subtasks created via decompose inherit the parent's project. Reject early if
        // the parent has none, rather than producing tasks that fail at create_task.
        let parent_project = parent_project.ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "Parent task '{parent_id}' has no `project` field. Subtasks inherit \
                 the parent's project — set it first or pass `project` per-subtask."
            )),
            data: None,
        })?;
        let parent_project = Some(parent_project);

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
                project: subtask
                    .get("project")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| parent_project.clone()),
                task_type: subtask
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                status: subtask
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                severity: subtask
                    .get("severity")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                goal_type: subtask
                    .get("goal_type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                session_id: None,
                issue_url: None,
                follow_up_tasks: vec![],
                release_summary: None,
                contributes_to: vec![],
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
        let severity = args
            .get("severity")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let goal_type = args.get("goal_type").and_then(|v| v.as_str());
        let assignee = args.get("assignee").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
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
                let s_canonical = crate::graph::resolve_status_alias(s);
                all.retain(|t| {
                    t.status
                        .as_deref()
                        .map(|st| st.eq_ignore_ascii_case(s_canonical))
                        .unwrap_or(false)
                });
            }
            all
        };

        if let Some(pri) = priority {
            tasks.retain(|t| t.effective_priority.unwrap_or(4) <= pri);
        }
        if let Some(sev) = severity {
            tasks.retain(|t| t.severity == Some(sev));
        }
        if let Some(gt) = goal_type {
            tasks.retain(|t| {
                t.goal_type
                    .as_deref()
                    .map(|g| g.eq_ignore_ascii_case(gt))
                    .unwrap_or(false)
            });
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
        if !tags.is_empty() {
            tasks.retain(|t| {
                tags.iter()
                    .all(|want| t.tags.iter().any(|have| have.eq_ignore_ascii_case(want)))
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
                        "urgency": t.urgency,
                        "due": t.due,
                        "effort": t.effort,
                        "consequence": t.consequence,
                        "severity": t.severity,
                        "goal_type": t.goal_type,
                        "edge_template": t.edge_template,
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
            // Ready view: table with Weight column, sorted by urgency
            let mut out = format!(
                "**{total} ready tasks** (showing {}, sorted by urgency)\n\n",
                tasks.len()
            );
            let today = chrono::Utc::now().date_naive();
            out.push_str("| # | ID | Pri | Weight | Crit | Urg | Due | Title |\n|---|---|---|---|---|---|---|---|\n");
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
                let urg = if t.urgency > 0.0 {
                    if t.urgency >= 10000.0 {
                        "SEV4".to_string()
                    } else if t.urgency >= 100.0 {
                        format!("{:.0}", t.urgency)
                    } else {
                        format!("{:.1}", t.urgency)
                    }
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
                    urg,
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
        if args.get("path").is_some() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("The 'path' parameter is no longer supported. Please use 'id' instead."),
                data: None,
            });
        }

        let id = args.get("id").and_then(|v| v.as_str()).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Missing required parameter: id"),
            data: None,
        })?;

        let path = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            self.abs_path(&node.path)
        };

        // Accept two forms for convenience:
        //   1. Nested: {"id": "...", "updates": {"status": "done"}}
        //   2. Flat:   {"id": "...", "status": "done"}
        // If `updates` is present, it wins. Otherwise collect top-level fields.
        const ROUTING_KEYS: &[&str] = &["id", "updates"];
        let updates: serde_json::Map<String, serde_json::Value> =
            if let Some(nested) = args.get("updates").and_then(|v| v.as_object()) {
                nested.clone()
            } else {
                args.as_object()
                    .map(|obj| {
                        obj.iter()
                            .filter(|(k, _)| !ROUTING_KEYS.contains(&k.as_str()))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default()
            };
        if updates.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "No fields to update. Pass fields either nested as `updates: {status: \"done\"}` or flat as top-level params (e.g. status=\"done\").",
                ),
                data: None,
            });
        }

        // Validate parent change: reject if the new parent does not exist
        // (task-89b2af87) AND reject if the change would create a cycle.
        if let Some(new_parent_val) = updates.get("parent") {
            // Treat null / empty string as "clear parent" — no validation needed.
            let new_parent = new_parent_val.as_str().unwrap_or("").trim();
            if !new_parent.is_empty() {
                let allow_missing = args
                    .get("allow_missing_parent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let graph = self.graph.read();
                // 1. Existence check (task-89b2af87)
                if graph.resolve(new_parent).is_none() {
                    if allow_missing {
                        tracing::warn!(
                            "update_task: parent '{}' not found in PKB; proceeding because allow_missing_parent=true",
                            new_parent
                        );
                    } else {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Parent '{}' not found in PKB. Create the parent first, fix the ID, or pass allow_missing_parent=true.",
                                new_parent
                            )),
                            data: None,
                        });
                    }
                }
                // 2. Cycle check (pre-existing)
                if let Err(msg) = graph.would_create_parent_cycle(id, new_parent) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
            }
        }

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

        // Per-phase timing for write-path perf investigation (task-a4dcc039).
        // Emitted at debug; set RUST_LOG=mem::mcp_server=debug to observe.
        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        crate::document_crud::update_document(&path, update_map).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::update_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        // Append completion evidence to body when completing via update_task
        if let Some(evidence) = evidence_text {
            if setting_done && !evidence.trim().is_empty() {
                Self::append_evidence(&path, &evidence, pr_url_text.as_deref())?;
            }
        }

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::update_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::update_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::update_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!("Incremental parse failed for {:?}, doing full rebuild", path);
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::update_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::update_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

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
            "Task updated: `{id}`{hint}"
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

    fn handle_get_stats(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let stats = crate::telemetry::get_stats();
        let json = serde_json::to_string_pretty(&stats).unwrap_or_default();
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

    // =========================================================================
    // CONSOLIDATED TOOLS (Progressive Disclosure)
    // =========================================================================

    fn handle_pkb_tool_help(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let tool = args.get("tool").and_then(|v| v.as_str());
        let action = args.get("action").and_then(|v| v.as_str());

        let help = match (tool, action) {
            (Some("create_document"), _) => {
                "## create_document\n\n\
                 Create a new PKB node (task, memory, note, etc.).\n\n\
                 **Parameters:**\n\
                 - `type`: task, memory, note, goal, project, epic\n\
                 - `title`: String (required)\n\
                 - `parent`: Parent ID (required for tasks)\n\
                 - `fields`: Object containing priority (0-4), tags (array), depends_on (array), assignee, etc.\n\
                 - `body`: Markdown content\n"
            }
            (Some("manage_task"), Some("release")) => {
                "## manage_task(action='release')\n\n\
                 Transition a task to a terminal status with work history.\n\n\
                 **Parameters:**\n\
                 - `id`: Task ID\n\
                 - `params`: Object\n\
                   - `status`: merge_ready, done, review, blocked, cancelled\n\
                   - `summary`: What was done (1-3 sentences)\n\
                   - `pr_url`: PR link (optional)\n"
            }
            (Some("manage_task"), _) => {
                "## manage_task\n\n\
                 Lifecycle management for tasks.\n\n\
                 **Actions:**\n\
                 - `update`: Update fields (params: {updates: {priority: 1, ...}})\n\
                 - `complete`: Mark as done (params: {completion_evidence: '...'})\n\
                 - `release`: Handoff (params: {status: 'merge_ready', summary: '...'})\n\
                 - `decompose`: Create subtasks (params: {subtasks: [{title: '...'}, ...]})\n"
            }
            (Some("pkb_batch"), _) => {
                "## pkb_batch\n\n\
                 Bulk operations across multiple nodes.\n\n\
                 **Actions:**\n\
                 - `update`: Filter and update fields\n\
                 - `reparent`: Move multiple tasks to new parent\n\
                 - `archive`: Set multiple to done\n\
                 - `merge`: Merge duplicate tasks into canonical\n\
                 - `node_merge`: Merge any nodes and redirect references\n\
                 - `epics`: Create epics and reparent tasks\n\
                 - `reclassify`: Change node types in bulk\n\
                 - `duplicates`: Detect potential duplicates\n\
                 - `orphans`: List disconnected nodes\n"
            }
            _ => {
                "## PKB Tools Help\n\n\
                 Use `pkb_tool_help(tool='TOOL_NAME')` for detailed schema.\n\n\
                 **Entrypoint Tools:**\n\
                 - `search`: Semantic search (all types)\n\
                 - `get_document`: Read by ID/path\n\
                 - `list_documents`: List and filter\n\
                 - `create_document`: Create new nodes\n\
                 - `manage_task`: Task lifecycle\n\
                 - `pkb_explore`: Graph relationships\n\
                 - `pkb_batch`: Bulk operations\n\
                 - `pkb_stats`: System status\n"
            }
        };

        Ok(CallToolResult::success(vec![Content::text(help)]))
    }

    fn remap_arg(args: &mut JsonValue, from: &str, to: &str) {
        if let Some(obj) = args.as_object_mut() {
            if let Some(val) = obj.remove(from) {
                obj.insert(to.to_string(), val);
            }
        }
    }

    fn handle_create_document_consolidated(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let doc_type = args.get("type").and_then(|v| v.as_str()).unwrap_or("note");
        let fields = args.get("fields").and_then(|v| v.as_object());

        let mut new_args = args.clone();
        if let Some(obj) = fields {
            for (k, v) in obj {
                new_args.as_object_mut().unwrap().insert(k.clone(), v.clone());
            }
        }

        match doc_type {
            "task" => self.handle_create_task(&new_args),
            "memory" => self.handle_create_memory(&new_args),
            "subtask" => {
                Self::remap_arg(&mut new_args, "parent", "parent_id");
                self.handle_create_subtask(&new_args)
            }
            _ => self.handle_create_document(&new_args),
        }
    }

    fn handle_manage_task_consolidated(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let action = args.get("action").and_then(|v| v.as_str()).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("action is required: update|complete|release|decompose"),
            data: None,
        })?;

        let params = args.get("params").and_then(|v| v.as_object());
        let mut merged_args = args.clone();
        if let Some(obj) = params {
            for (k, v) in obj {
                merged_args.as_object_mut().unwrap().insert(k.clone(), v.clone());
            }
        }

        match action {
            "update" => self.handle_update_task(&merged_args),
            "complete" => self.handle_complete_task(&merged_args),
            "release" => self.handle_release_task(&merged_args),
            "decompose" => {
                Self::remap_arg(&mut merged_args, "id", "parent_id");
                self.handle_decompose_task(&merged_args)
            }
            _ => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Unknown action: {action}")),
                data: None,
            }),
        }
    }

    fn handle_pkb_explore_consolidated(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let action = args.get("action").and_then(|v| v.as_str()).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("action is required: context|trace|tree|children|metrics"),
            data: None,
        })?;

        let params = args.get("params").and_then(|v| v.as_object());
        let mut merged_args = args.clone();
        if let Some(obj) = params {
            for (k, v) in obj {
                merged_args.as_object_mut().unwrap().insert(k.clone(), v.clone());
            }
        }

        match action {
            "context" => self.handle_pkb_context(&merged_args),
            "trace" => {
                Self::remap_arg(&mut merged_args, "id", "from");
                self.handle_pkb_trace(&merged_args)
            }
            "tree" => self.handle_get_dependency_tree(&merged_args),
            "children" => self.handle_get_task_children(&merged_args),
            "metrics" => self.handle_get_network_metrics(&merged_args),
            _ => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Unknown action: {action}")),
                data: None,
            }),
        }
    }

    fn handle_pkb_batch_consolidated(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let action = args.get("action").and_then(|v| v.as_str()).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("action is required"),
            data: None,
        })?;

        let params = args.get("params").and_then(|v| v.as_object());
        let mut merged_args = args.clone();
        if let Some(obj) = params {
            for (k, v) in obj {
                merged_args.as_object_mut().unwrap().insert(k.clone(), v.clone());
            }
        }

        match action {
            "update" => self.handle_batch_update(&merged_args),
            "reparent" => self.handle_batch_reparent(&merged_args),
            "archive" => self.handle_batch_archive(&merged_args),
            "merge" => self.handle_batch_merge(&merged_args),
            "node_merge" => self.handle_merge_node(&merged_args),
            "epics" => self.handle_batch_create_epics(&merged_args),
            "reclassify" => self.handle_batch_reclassify(&merged_args),
            "duplicates" => self.handle_find_duplicates(&merged_args),
            "orphans" => self.handle_pkb_orphans(&merged_args),
            _ => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Unknown action: {action}")),
                data: None,
            }),
        }
    }

    fn handle_pkb_stats_consolidated(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("summary");

        match action {
            "summary" => self.handle_task_summary(args),
            "graph_stats" => self.handle_graph_stats(args),
            "graph_json" => self.handle_graph_json(args),
            "tool_stats" => self.handle_get_stats(args),
            _ => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Unknown action: {action}")),
                data: None,
            }),
        }
    }

    fn handle_list_prompts(&self) -> Result<ListPromptsResult, McpError> {
        fn required_arg(name: &str, description: &str) -> PromptArgument {
            PromptArgument::new(name)
                .with_description(description)
                .with_required(true)
        }
        let prompts = vec![
            Prompt::new(
                "find-task",
                Some("How do I find a task about X?"),
                Some(vec![required_arg("query", "The task to find")]),
            ),
            Prompt::new(
                "explore-topic",
                Some("What do we know about X?"),
                Some(vec![required_arg("query", "The topic to explore")]),
            ),
            Prompt::new(
                "navigate-graph",
                Some("What's connected to X?"),
                Some(vec![required_arg(
                    "id",
                    "The node ID, title, or filename",
                )]),
            ),
            Prompt::new(
                "find-by-tag",
                Some("Show me everything tagged X"),
                Some(vec![required_arg("tag", "The tag to filter by")]),
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
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
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
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
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
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
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
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
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
        let start = std::time::Instant::now();
        let tool_name = request.name.clone();

        let args = Self::args_to_value(request.arguments);

        // For consolidated tools, build a granular name for telemetry
        let effective_name: String = match &*tool_name {
            "manage_task" | "pkb_explore" | "pkb_batch" | "pkb_stats" | "create_document" => {
                let sub = args
                    .get("action")
                    .or_else(|| args.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                format!("{tool_name}/{sub}")
            }
            other => other.to_string(),
        };

        let result = match &*request.name {
            // --- Consolidated Tools ---
            "create_document" => self.handle_create_document_consolidated(&args),
            "manage_task" => self.handle_manage_task_consolidated(&args),
            "pkb_explore" => self.handle_pkb_explore_consolidated(&args),
            "pkb_batch" => self.handle_pkb_batch_consolidated(&args),
            "pkb_stats" => self.handle_pkb_stats_consolidated(&args),
            "pkb_tool_help" => self.handle_pkb_tool_help(&args),

            // --- Legacy / Granular Tools ---
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
            "get_semantic_neighbors" => self.handle_get_semantic_neighbors(&args),
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
            "get_stats" => self.handle_get_stats(&args),
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown tool: {}", request.name)),
                data: None,
            }),
        };

        let latency = start.elapsed().as_millis();
        let is_error = result.is_err();
        let response_bytes = match &result {
            Ok(res) => serde_json::to_vec(res).map(|v| v.len()).unwrap_or(0),
            Err(e) => serde_json::to_vec(e).map(|v| v.len()).unwrap_or(0),
        };

        crate::telemetry::record_call(
            &effective_name,
            response_bytes,
            latency,
            is_error,
        );

        std::future::ready(result)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = Self::get_all_tools();
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
             39 tools for search, documents, tasks, and knowledge graph. \
             Use MCP prompts (find-task, explore-topic, navigate-graph, find-by-tag) for search pattern guidance. \
             Use get_stats to view per-tool usage telemetry.",
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

impl PkbSearchServer {
    fn get_all_tools() -> Vec<Tool> {
        vec![
            Tool::new(
                "search",
                "Hybrid semantic + graph-proximity search across the personal knowledge base. Use this for general discovery and finding related knowledge. Supports proximity boosting.",
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
            )
            .with_title("Hybrid Semantic Search")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_document",
                "Read the full contents of a specific PKB document. Use when you need the complete text for analysis. ONLY accepts short-form ID (e.g. task-xxx), filename stem, or title.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID, filename stem, title, or permalink (uses flexible resolution)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Document Content")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_documents",
                "List indexed documents with optional filters. Good for browsing specific types, tags, or status groups with pagination.",
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
            )
            .with_title("List Documents")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "task_search",
                "Semantic search filtered to actionable tasks. Returns results with rich graph context including status and dependencies. Use `type: \"epic\"` to find container tasks (with context and subtasks) rather than leaf tasks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Query to search tasks" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false." },
                        "type": { "type": "string", "description": "Filter by task type. Single value (e.g. 'epic') or comma-separated list (e.g. 'epic,feature'). Recognised actionable types: project, epic, task, learn. Default: all actionable types." }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            )
            .with_title("Task Search")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_network_metrics",
                "Calculate centrality metrics (PageRank, betweenness, degree) for a node. Use to identify high-impact or 'load-bearing' tasks in the graph.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Network Metrics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "create_task",
                "Create a new task markdown file with YAML frontmatter. Requires `parent` and `project`. Supports the Birnbaum importance model via `contributes_to` and severity-based prioritization.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Task title (also accepts `task_title` as alias)" },
                        "task_title": { "type": "string", "description": "Alias for title" },
                        "id": { "type": "string", "description": "Task ID (auto-generated if omitted)" },
                        "parent": { "type": "string", "description": "Parent task ID" },
                        "priority": { "type": "integer", "description": "0-4 (0=critical, 1=intended, 2=active, 3=planned, 4=backlog)" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "effort": { "type": "string", "description": "Effort duration string: '1d', '2h', '1w'. Parser converts to days." },
                        "consequence": { "type": "string", "description": "Narrative description of what happens if this task is not done or fails." },
                        "severity": { "type": "integer", "description": "Severity ladder (0-4) for target nodes. SEV4 is lexicographic." },
                        "goal_type": { "type": "string", "description": "Goal classification: committed | aspirational | learning.", "enum": GOAL_TYPE_ENUM },
                        "body": { "type": "string", "description": "Markdown body" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." },
                        "due": { "type": "string", "description": "Due date (ISO date, e.g. '2026-06-01')" },
                        "project": { "type": "string", "description": "Project identifier (required, e.g. 'aops', 'mem', 'adhoc-sessions')" },
                        "type": { "type": "string", "description": "Task type (default: 'task'). Also accepts: epic, bug, feature, learn, goal, project." },
                        "status": { "type": "string", "description": "Task status (default: 'draft' — new tasks start as draft and are excluded from ready queue until promoted to 'active'). Also accepts: active, blocked, done, merge_ready, in_progress, etc." }
                    },
                    "required": ["title", "project"]
                }))
                .unwrap(),
            )
            .with_title("Create Task"),
            Tool::new(
                "create_subtask",
                "Create a numbered sub-task attached to a parent task. Sub-tasks use dot-notation IDs (e.g. proj.1) and appear as a checklist when the parent is retrieved. Use for fine-grained completion tracking.",
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
            )
            .with_title("Create Sub-task"),
            Tool::new(
                "create_memory",
                "Create a new memory, insight, or observation. Stored in memories/ directory. Use for recording persistent knowledge or session findings.",
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
            )
            .with_title("Create Memory"),
            Tool::new(
                "create",
                "Generic document creation with automatic subdirectory routing (tasks/, projects/, goals/, notes/). Use when the specific specialized tool is not applicable.",
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
                        "severity": { "type": "integer", "description": "Severity ladder (0-4) for target nodes." },
                        "goal_type": { "type": "string", "description": "Goal classification: committed | aspirational | learning.", "enum": GOAL_TYPE_ENUM },
                        "dir": { "type": "string", "description": "Override subdirectory placement" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." }
                    },
                    "required": ["title", "type"]
                }))
                .unwrap(),
            )
            .with_title("Create Document"),
            Tool::new(
                "append",
                "Append timestamped content to an existing document by ID. Use for logging progress, adding references, or updating a 'log' section. Not idempotent.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "content": { "type": "string", "description": "Content to append (will be timestamped)" },
                        "section": { "type": "string", "description": "Optional target section heading (e.g. 'Log', 'References'). Creates section if not found." }
                    },
                    "required": ["id", "content"]
                }))
                .unwrap(),
            )
            .with_title("Append to Document"),
            Tool::new(
                "delete",
                "Permanently delete a document by ID. Removes the file from disk and the vector store index. Use with caution.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (task ID, memory ID, filename stem, or title). Uses flexible resolution." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Delete Document")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "complete_task",
                "Mark a task as done. Requires completion_evidence describing what was achieved. Sets status to 'done', appends evidence to body, and re-indexes.",
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
            )
            .with_title("Complete Task"),
            Tool::new(
                "release_task",
                "Release a task to a terminal or handoff status (merge_ready, done, review, blocked, cancelled). Performs session handover by recording work history, linking PRs/issues, and tracking follow-up work. If 'id' is omitted, an ad-hoc session task is created.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (flexible resolution: ID, filename stem, or title). If omitted, an ad-hoc task is created." },
                        "status": {
                            "type": "string",
                            "enum": ["merge_ready", "done", "review", "blocked", "cancelled"],
                            "description": "Target status"
                        },
                        "summary": { "type": "string", "description": "What was done and outcome. 1-3 sentences minimum." },
                        "pr_url": { "type": "string", "description": "Pull request or commit URL (recommended for merge_ready)" },
                        "branch": { "type": "string", "description": "Git branch name (optional)" },
                        "blocker": { "type": "string", "description": "What is blocking this task (for status=blocked)" },
                        "reason": { "type": "string", "description": "Why cancelled or needs review (for status=cancelled/review)" },
                        "session_id": { "type": "string", "description": "Active session ID. Falls back to $AOPS_SESSION_ID if omitted." },
                        "issue_url": { "type": "string", "description": "External issue/ticket URL" },
                        "follow_up_tasks": { "type": "array", "items": { "type": "string" }, "description": "IDs of new tasks created as follow-ups. Validated for existence." },
                        "release_summary": { "type": "string", "description": "Detailed technical summary for the release. Warning if > 500 chars." }
                    },
                    "required": ["status", "summary"]
                }))
                .unwrap(),
            )
            .with_title("Release Task"),
            Tool::new(
                "list_tasks",
                "List tasks with smart filtering. Supports sorting by focus score (incorporating lexicographic severity and exponential decay). Use status='ready' for actionable leaf tasks or status='blocked' to see blockers.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project name (case-insensitive). Returns tasks whose computed project field (nearest ancestor with node_type=project) matches." },
                        "status": { "type": "string", "description": "Filter by status. Special values: 'ready' (actionable leaf tasks), 'blocked' (tasks with unmet deps). Also: active, in_progress, done, etc." },
                        "priority": { "type": "integer", "description": "Filter to tasks whose effective priority (own or any downstream task via blocks/parent) ≤ N. E.g. priority=0 returns every task that touches a P0, including its blockers." },
                        "severity": { "type": "integer", "description": "Filter by exact severity" },
                        "goal_type": { "type": "string", "description": "Filter by goal type" },
                        "assignee": { "type": "string", "description": "Filter by assignee" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags. A task matches iff every requested tag is present in its frontmatter `tags` array (AND, case-insensitive)." },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false — subtasks are hidden since they travel with their parent task." },
                        "format": { "type": "string", "enum": ["markdown", "json"], "description": "Output format. 'json' returns structured {total, showing, tasks[]} for programmatic use. Default: 'markdown'." }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Tasks")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_task",
                "Retrieve full details for a task, including metadata, body content, and graph relationship context (dependencies, blockers, children, subtasks).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (e.g. 'framework-6b4325a1'). Also accepts filename stem or title." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Task Detail")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "update_task",
                "Patch metadata fields on an existing task. Pass fields either nested as `updates: {status: \"done\"}` or flat at the top level (e.g. status=\"done\"). Use for non-terminal updates (priority, tags, assignee). For state transitions (done, merge_ready), prefer release_task.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, title)" },
                        "updates": { "type": "object", "description": "Optional nested form. JSON object of fields to update (null to remove a field). If omitted, any top-level fields other than id/updates are treated as fields to update." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Update Task")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "bulk_reparent",
                "Set a new parent for all documents matching a pattern. Skips files already parented correctly. Dry run by default.",
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
            )
            .with_title("Bulk Reparent Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "retrieve_memory",
                "Find relevant memories, insights, or observations by semantic similarity. Returns full content for the top matches.",
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
            )
            .with_title("Retrieve Memory")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "search_by_tag",
                "Find all documents sharing a specific set of tags. Supports filtering by document type.",
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
            )
            .with_title("Search by Tag")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_memories",
                "Browse memory-type documents (notes, insights, observations) with optional tag filtering.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: 20)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags (all must match)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Memories")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "delete_memory",
                "Permanently delete a memory document. Only works on memory-type nodes (note, insight, observation). Destructive.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Memory ID (supports flexible resolution)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Delete Memory")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "decompose_task",
                "Split a large task into multiple subtasks in one operation. Supports relative sibling references (e.g. '$1') for dependencies. Subtasks inherit the parent's `project` field unless explicitly overridden. Use to structure a newly defined work package.",
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
                                    "consequence": { "type": "string" },
                                    "severity": { "type": "integer" },
                                    "goal_type": { "type": "string" },
                                    "project": { "type": "string", "description": "Override project field (defaults to parent's project)" }
                                },
                                "required": ["title"]
                            },
                            "description": "Array of subtask definitions"
                        }
                    },
                    "required": ["parent_id", "subtasks"]
                }))
                .unwrap(),
            )
            .with_title("Decompose Task"),
            Tool::new(
                "get_dependency_tree",
                "Visualize the task dependency graph for a specific node. Upstream shows what the task depends on; downstream shows what it blocks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "direction": { "type": "string", "description": "Direction: 'upstream' (depends on, default) or 'downstream' (blocks)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Dependency Tree")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_task_children",
                "List all direct or recursive children of a task. Returns completion counts and status for the subtree. Use to assess progress of an epic or parent task.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "recursive": { "type": "boolean", "description": "Include all descendants, not just direct children (default: false)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Task Children")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_context",
                "Explore the knowledge neighbourhood of a node. Returns metadata, relationships, and backlinks grouped by source type. Supports flexible ID resolution.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID, task ID, filename stem, or title" },
                        "hops": { "type": "integer", "description": "Neighbourhood radius in hops (default: 2)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("PKB Knowledge Context")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_trace",
                "Find shortest paths between two nodes in the knowledge graph. Useful for understanding how two seemingly unrelated concepts or tasks are linked.",
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
            )
            .with_title("PKB Path Trace")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_orphans",
                "Identify disconnected nodes with no valid parent. Use to maintain graph integrity and ensure all tasks are properly situated in the hierarchy.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: all). Set to 0 for unlimited." },
                        "types": { "type": "array", "items": { "type": "string" }, "description": "Filter by node type (e.g. [\"task\"], [\"task\", \"project\"]). Overrides default actionable-only filter." },
                        "include_all": { "type": "boolean", "description": "Include all node types (notes, memories, etc.) — default false." }
                    }
                }))
                .unwrap(),
            )
            .with_title("Find Orphan Nodes")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            // ── Batch Operations ──────────────────────────────────────────
            Tool::new(
                "batch_update",
                "Apply frontmatter updates to multiple tasks simultaneously. Supports complex filters (by parent, subtree, tags, age, etc.) or explicit ID lists. Always use _add_tags/_remove_tags for list fields.",
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
                        "weight_gte": { "type": "integer", "description": "Filter: downstream weight >= N" },
                        "updates": { "type": "object", "description": "Fields to set (null to remove). Special keys: _add_tags, _remove_tags, _add_depends_on, _remove_depends_on" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            )
            .with_title("Batch Update Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "batch_reparent",
                "Bulk move tasks to a new parent node. Use for major restructuring, such as grouping flat tasks into a new project or epic.",
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
            )
            .with_title("Batch Reparent Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "batch_archive",
                "Bulk archive tasks by setting status to 'done'. Use for closing out entire subtrees or stale tasks. Dry-run by default.",
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
            )
            .with_title("Batch Archive Tasks")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "get_semantic_neighbors",
                "Find nodes semantically similar to a given node based on vector proximity of embeddings. Returns a list of nodes that are related by content even if not explicitly linked in the graph.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID, task ID, filename stem, or title (flexible resolution)" },
                        "threshold": { "type": "number", "description": "Cosine similarity threshold (0.0-1.0, default: 0.85). Higher is more restrictive." },
                        "limit": { "type": "integer", "description": "Maximum number of neighbors to return (default: 10)." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Find Semantic Neighbors")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "graph_stats",
                "Get a summary of PKB health, including task distribution by status/priority, orphan counts, and disconnected clusters. Read-only.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                    }
                }))
                .unwrap(),
            )
            .with_title("Graph Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "graph_json",
                "Export the full knowledge graph as JSON. Use for external visualization or deep structural analysis.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Knowledge Graph JSON")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "task_summary",
                "Get high-level dashboard metrics: counts of 'ready' vs 'blocked' tasks, and priority breakdowns. Use for situational awareness.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Task Summary Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            // ── Phase 2: Deduplication & Restructuring ────────────────────
            Tool::new(
                "find_duplicates",
                "Identify potential duplicate tasks using both semantic and title similarity. Returns clusters with a suggested canonical task. Read-only.",
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
            )
            .with_title("Find Duplicate Tasks")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "batch_merge",
                "Merge multiple duplicate tasks into a single canonical task. Archives duplicates and redirects dependencies. Idempotent.",
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
            )
            .with_title("Batch Merge Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "merge_node",
                "Merge source knowledge nodes into a canonical node. Performs a deep merge of all references (wikilinks, parents, etc.) across the entire PKB. Destructive for source nodes (archived).",
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
            )
            .with_title("Merge Knowledge Node")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "batch_create_epics",
                "Group flat tasks into new epic containers. Use to organize a scattered list of tasks into coherent, parented groups.",
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
            )
            .with_title("Batch Create Epics"),
            Tool::new(
                "batch_reclassify",
                "Correct the type field for multiple documents and move them to their appropriate subdirectories. Idempotent.",
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
            )
            .with_title("Batch Reclassify Types")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "get_stats",
                "Show MCP tool usage telemetry — call counts and response bytes per tool.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Tool Usage Stats")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            // --- Consolidated (Progressive Disclosure) Tools ---
            Tool::new(
                "create_document",
                "Create a new PKB node (task, subtask, memory, note, goal, project, epic). Dispatches on `type`.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["task", "subtask", "memory", "note", "goal", "project", "epic"] },
                        "title": { "type": "string" },
                        "parent": { "type": "string", "description": "Parent ID (required for subtask)" },
                        "fields": { "type": "object", "description": "Metadata: priority, tags, depends_on, assignee, etc." },
                        "body": { "type": "string" }
                    },
                    "required": ["type", "title"]
                }))
                .unwrap(),
            )
            .with_title("Create Document"),
            Tool::new(
                "manage_task",
                "Lifecycle management for tasks: update, complete, release, or decompose.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "action": { "type": "string", "enum": ["update", "complete", "release", "decompose"] },
                        "params": { "type": "object", "description": "Action-specific parameters (e.g. updates, summary, pr_url, subtasks)" }
                    },
                    "required": ["id", "action"]
                }))
                .unwrap(),
            )
            .with_title("Manage Task")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "pkb_explore",
                "Explore graph relationships: context, trace, tree, children, metrics.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "action": { "type": "string", "enum": ["context", "trace", "tree", "children", "metrics"] },
                        "params": { "type": "object", "description": "Action-specific: e.g. {to: 'ID'} for trace, {recursive: true} for children" }
                    },
                    "required": ["id", "action"]
                }))
                .unwrap(),
            )
            .with_title("PKB Graph Explorer")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_batch",
                "Bulk operations: update, reparent, archive, merge, node_merge, epics, reclassify, duplicates, orphans.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["update", "reparent", "archive", "merge", "node_merge", "epics", "reclassify", "duplicates", "orphans"] },
                        "params": { "type": "object", "description": "Filters, updates, new_parent, merge_ids, etc." }
                    },
                    "required": ["action"]
                }))
                .unwrap(),
            )
            .with_title("PKB Batch Operations")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "pkb_stats",
                "System and graph status: summary, graph_stats, graph_json, tool_stats.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["summary", "graph_stats", "graph_json", "tool_stats"] }
                    }
                }))
                .unwrap(),
            )
            .with_title("PKB Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_tool_help",
                "Get detailed schema and examples for consolidated tools.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tool": { "type": "string" },
                        "action": { "type": "string" }
                    }
                }))
                .unwrap(),
            )
            .with_title("PKB Tool Help")
            .with_annotations(ToolAnnotations::new().read_only(true)),
        ]
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
            file_hash: "test_hash".to_string(),
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
            file_hash: "test_hash".to_string(),
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
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
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
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        // Either empty JSON tasks array or "No tasks found" message
        let is_empty = text.contains("No tasks found") || text.contains("\"tasks\":[]") || text.contains("\"tasks\": []");
        assert!(is_empty, "non-existent project should return empty: {}", text);
    }

    // ── Tag filtering ──

    fn make_doc_with_tags(
        path: &str,
        title: &str,
        id: &str,
        tags: &[&str],
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!("task"));
        fm.insert("status".to_string(), json!("active"));
        fm.insert("id".to_string(), json!(id));
        if !tags.is_empty() {
            fm.insert("tags".to_string(), json!(tags));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            modified: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    fn build_tag_test_server() -> PkbSearchServer {
        let docs = vec![
            make_doc_with_tags("tasks/t-overwhelm.md", "Overwhelm task", "t-overwhelm", &["overwhelm", "rust"]),
            make_doc_with_tags("tasks/t-overwhelm-only.md", "Overwhelm only", "t-overwhelm-only", &["overwhelm"]),
            make_doc_with_tags("tasks/t-rust-only.md", "Rust only", "t-rust-only", &["rust"]),
            make_doc_with_tags("tasks/t-untagged.md", "Untagged task", "t-untagged", &[]),
            make_doc_with_tags("tasks/t-other.md", "Other task", "t-other", &["misc"]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb-tags"));
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            PathBuf::from("/tmp/test-pkb-tags"),
            PathBuf::from("/tmp/test-pkb-tags/db"),
            Arc::new(RwLock::new(graph)),
        )
    }

    #[test]
    fn test_list_tasks_single_tag_filter() {
        let server = build_tag_test_server();
        let result = server
            .handle_list_tasks(&json!({"tags": ["overwhelm"], "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(ids.contains(&"t-overwhelm".to_string()));
        assert!(ids.contains(&"t-overwhelm-only".to_string()));
        assert!(!ids.contains(&"t-rust-only".to_string()));
        assert!(!ids.contains(&"t-untagged".to_string()));
        assert!(!ids.contains(&"t-other".to_string()));
    }

    #[test]
    fn test_list_tasks_multi_tag_and_filter() {
        let server = build_tag_test_server();
        let result = server
            .handle_list_tasks(&json!({"tags": ["overwhelm", "rust"], "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert_eq!(ids, vec!["t-overwhelm".to_string()]);
    }

    #[test]
    fn test_list_tasks_tag_no_match_returns_empty() {
        let server = build_tag_test_server();
        let result = server
            .handle_list_tasks(&json!({"tags": ["does-not-exist"], "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(ids.is_empty());
    }

    #[test]
    fn test_list_tasks_tag_filter_excludes_untagged() {
        let server = build_tag_test_server();
        let result = server
            .handle_list_tasks(&json!({"tags": ["overwhelm"], "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(!ids.contains(&"t-untagged".to_string()));
    }

    #[test]
    fn test_list_tasks_tag_filter_case_insensitive() {
        let server = build_tag_test_server();
        let result = server
            .handle_list_tasks(&json!({"tags": ["OVERWHELM"], "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(ids.contains(&"t-overwhelm".to_string()));
        assert!(ids.contains(&"t-overwhelm-only".to_string()));
    }

    #[test]
    fn test_list_tasks_schema_includes_tags_parameter() {
        let tools = PkbSearchServer::get_all_tools();
        let list_tasks_tool = tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist");
        let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
        assert!(
            schema.contains("\"tags\""),
            "list_tasks schema should include 'tags' parameter, got: {}",
            schema
        );
    }

    // ── AC6: tool schema includes project parameter ──

    #[test]
    fn test_list_tasks_schema_includes_project_parameter() {
        let tools = PkbSearchServer::get_all_tools();
        let list_tasks_tool = tools
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

    // ── Parent referential integrity (task-89b2af87) ───────────────────────

    #[test]
    fn test_create_task_rejects_nonexistent_parent() {
        let server = build_test_server();
        let err = server
            .handle_create_task(&json!({
                "title": "test",
                "project": "aops",
                "parent": "task-does-not-exist"
            }))
            .expect_err("expected rejection on missing parent");
        let msg = err.message.to_string();
        assert!(
            msg.contains("task-does-not-exist") && msg.to_lowercase().contains("not found"),
            "error should mention the missing ID and 'not found'; got: {msg}"
        );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
    }

    #[test]
    fn test_create_task_rejects_nonexistent_parent_even_with_explicit_id() {
        // Verify the validation runs regardless of whether a custom id was passed.
        let server = build_test_server();
        let err = server
            .handle_create_task(&json!({
                "title": "test",
                "project": "aops",
                "id": "task-89b2af87-child",
                "parent": "task-does-not-exist"
            }))
            .expect_err("expected rejection on missing parent");
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS"
        );
    }

    #[test]
    fn test_update_task_rejects_reparent_to_nonexistent() {
        let server = build_test_server();
        // task-a1 exists in the seeded graph; try to reparent it to a missing node.
        let err = server
            .handle_update_task(&json!({
                "id": "task-a1",
                "parent": "task-does-not-exist"
            }))
            .expect_err("expected rejection on reparent to missing node");
        let msg = err.message.to_string();
        assert!(
            msg.contains("task-does-not-exist"),
            "error should name the missing ID; got: {msg}"
        );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS"
        );
    }

    #[test]
    fn test_update_task_allows_clearing_parent_with_null() {
        // Setting parent to "" (or null) should not trigger the resolver — clearing
        // the parent edge is a legitimate operation distinct from reparenting.
        // The update will fail downstream because the test server's pkb_root is a
        // bogus path, but it must NOT fail on parent-validation grounds.
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "task-a1",
                "parent": ""
            }))
            .expect_err("test server has no real disk file");
        // The failure should NOT be the parent-validation error.
        assert!(
            !err.message.contains("not found in PKB"),
            "empty parent should bypass referential check; got: {}",
            err.message
        );
    }
}

#[cfg(test)]
mod annotation_tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_annotations_audit() {
        let tools = PkbSearchServer::get_all_tools();

        for tool in tools {
            // 1. Every tool must have a title
            assert!(
                tool.title.is_some(),
                "Tool '{}' is missing a human-readable title. Use .with_title()",
                tool.name
            );

            // 2. Safety hints check
            // Note: Creation tools (create_*) don't require read_only or destructive hints
            // but they still need a title.
            let annotations = tool.annotations.as_ref();
            let is_creation = tool.name.starts_with("create") || tool.name.contains("decompose") || tool.name.contains("create_epics");

            if !is_creation {
                let has_hint = annotations.map(|a| a.read_only_hint.is_some() || a.destructive_hint.is_some()).unwrap_or(false);
                // Some tools like 'append', 'complete_task', 'release_task' are writes but NOT destructive nor read-only.
                // The requirement says "read-only OR destructive OR neither explicitly".
                // I'll check that if it's NOT one of those known write tools, it should probably have a hint.
                let is_known_write = [
                    "append",
                    "complete_task",
                    "release_task",
                    "update_task",
                    "bulk_reparent",
                    "batch_update",
                    "batch_reparent",
                    "batch_merge",
                    "batch_reclassify",
                    "manage_task",
                ]
                .contains(&&*tool.name);

                if !is_known_write {
                    assert!(
                        has_hint,
                        "Tool '{}' should have a safety hint (readOnlyHint or destructiveHint).",
                        tool.name
                    );
                }
            }

            // 3. Contradictory hints check
            if let Some(ann) = annotations {
                assert!(
                    !(ann.read_only_hint == Some(true) && ann.destructive_hint == Some(true)),
                    "Tool '{}' has contradictory hints (read-only AND destructive)",
                    tool.name
                );
            }
        }
    }

    #[tokio::test]
    async fn test_no_top_level_composition_keywords() {
        // Anthropic rejects tool schemas that have anyOf/allOf/oneOf at the top level of
        // inputSchema. They are only allowed nested inside a property.
        let tools = PkbSearchServer::get_all_tools();
        for tool in &tools {
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            for keyword in ["anyOf", "allOf", "oneOf"] {
                assert!(
                    !schema.as_object().map(|o| o.contains_key(keyword)).unwrap_or(false),
                    "Tool '{}' has '{}' at the top level of inputSchema — Anthropic rejects this (HTTP 400). Nest it inside a property instead.",
                    tool.name,
                    keyword,
                );
            }
        }
    }
}

