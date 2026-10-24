//! MCP server for PKB semantic search + task graph.
//!
//! Implements rmcp 0.1.5 ServerHandler trait manually with tool dispatch.
//! Provides 18 tools for search, documents, tasks, and knowledge graph.

pub mod embed_worker;
pub mod graph_fresh;
pub mod handlers_batch;
pub mod handlers_document;
pub mod handlers_excalidraw;
pub mod handlers_prompt;
pub mod handlers_search;
pub mod handlers_task;
pub mod handlers_task_lifecycle;
pub mod helpers;
pub mod schemas;
pub mod status;

pub use graph_fresh::RebuildStats;
pub use status::ReindexStatus;

#[cfg(test)]
mod claim_task_tests;
#[cfg(test)]
mod cross_process_recovery_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tier_rebuild_tests;

use crate::embeddings::Embedder;
use crate::facts::FactsProvider;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) const GOAL_TYPE_ENUM: &[&str] = &["committed", "aspirational", "learning"];

/// Hard ceiling on any client-supplied result-count parameter (`limit`,
/// `max_results`, `top_n`/`n`, etc). Applied on top of each tool's own
/// default — defaults are unchanged, this only prevents a client from
/// requesting an unbounded/huge result set that could blow out its own
/// context window. Tools with deliberate "0/None = unlimited" semantics
/// (e.g. `pkb_orphans`) are exempt for the unlimited case by design.
pub(crate) const MAX_RESULTS: usize = 1000;

/// Validate and extract required `slug` parameter from create action arguments.
/// Must be present, non-empty, and at most 30 characters.
pub(crate) fn parse_and_validate_slug(args: &JsonValue) -> Result<String, McpError> {
    match args.get("slug") {
        Some(v) if v.is_null() => Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Missing required parameter: slug (limit is 30 characters)"),
            data: None,
        }),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Parameter 'slug' must be a string (limit is 30 characters)"),
                data: None,
            })?;
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: slug (limit is 30 characters)"),
                    data: None,
                });
            }
            let len = trimmed.chars().count();
            if len > crate::document_crud::MAX_SLUG_LEN {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Parameter 'slug' is too long ({} characters). Limit is {} characters.",
                        len,
                        crate::document_crud::MAX_SLUG_LEN
                    )),
                    data: None,
                });
            }
            Ok(trimmed.to_string())
        }
        None => Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Missing required parameter: slug (limit is 30 characters)"),
            data: None,
        }),
    }
}

#[derive(Clone)]
pub struct InstrumentedGraphStore {
    inner: Arc<RwLock<GraphStore>>,
}

impl InstrumentedGraphStore {
    pub fn new(inner: Arc<RwLock<GraphStore>>) -> Self {
        Self { inner }
    }

    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, GraphStore> {
        let start = std::time::Instant::now();
        let guard = self.inner.read();
        let wait = start.elapsed();
        crate::otel::call_metrics::CallMetrics::record_graph_read_wait(wait);
        guard
    }

    pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, GraphStore> {
        self.inner.write()
    }

    pub fn inner(&self) -> &Arc<RwLock<GraphStore>> {
        &self.inner
    }
}

#[derive(Clone)]
pub struct PkbSearchServer {
    pub(crate) store: Arc<RwLock<VectorStore>>,
    pub(crate) embedder: Arc<Embedder>,
    pub(crate) reranker: Arc<crate::rerank::CrossEncoderReranker>,
    pub(crate) pkb_root: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) graph: InstrumentedGraphStore,
    pub(crate) session_registry: Arc<crate::otel::session_registry::SessionRegistry>,
    pub(crate) stale_count: usize,
    /// True when a background vector-store save is already scheduled. Used
    /// to coalesce burst writes: a subsequent `save_store` call while a save
    /// is in flight is skipped (the in-flight save will capture the latest
    /// state when it runs).
    pub(crate) save_pending: Arc<std::sync::atomic::AtomicBool>,
    /// Set of absolute paths whose in-memory upsert was skipped because the
    /// cross-process index lock was held (typically by `pkb reindex` running
    /// in another process). Drained the next time we observe the lock as
    /// released. Membership covers both upsert-deferred and remove-deferred
    /// paths — the drain step inspects whether the file exists on disk to
    /// pick the right action, so a single set is sufficient and idempotent.
    pub(crate) deferred_paths: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
    /// Becomes `true` the first time we observe `index_lock_available()`
    /// returning false. The drain step swaps it back to `false` when it
    /// reloads from disk to adopt the reindex's authoritative state.
    pub(crate) lock_was_held: Arc<std::sync::atomic::AtomicBool>,
    /// Single-drainer lock: ensures only one thread executes the reload +
    /// replay sequence at a time. `try_lock` makes the path effectively a
    /// no-op for concurrent callers — they'll see the drained state on
    /// their next handler invocation if the drain hasn't completed yet.
    pub(crate) drain_in_progress: Arc<Mutex<()>>,
    /// True when our own background save thread holds (or is about to
    /// acquire) the cross-process index file lock. Used by
    /// `try_upsert_document` and `maybe_drain_deferred` to distinguish
    /// "lock held by an external `pkb reindex` process" (we should defer
    /// our in-memory upsert and reload from disk when they're done) from
    /// "lock held by our own save thread" (the in-memory state is the
    /// source of truth — we should NOT defer or reload). Without this
    /// distinction, every iteration that overlaps a self-save triggered a
    /// 1–2s full store reload on the *next* iteration.
    pub(crate) save_in_flight: Arc<std::sync::atomic::AtomicBool>,
    /// True when a background graph rebuild (Tier 2) worker has been
    /// spawned and is either running or in its post-iteration check. Used
    /// only to gate spawning; consumers polling for "is a rebuild
    /// outstanding?" should look at `graph_rebuild_dirty`.
    pub(crate) graph_rebuild_pending: Arc<std::sync::atomic::AtomicBool>,
    /// True when at least one write has landed since the last Tier-2
    /// rebuild started. The worker drains this flag inside its loop: if
    /// dirty, run another iteration; if clean, exit. This guarantees at
    /// most one Tier-2 in flight + at most one queued (the "rerun"
    /// embodied by the dirty flag), eliminating CPU/lock contention from
    /// multiple concurrent Tier-2s seen with the prior clear-at-start
    /// pattern.
    pub(crate) graph_rebuild_dirty: Arc<std::sync::atomic::AtomicBool>,
    /// Node IDs that received a Tier-1 patch since the most recent Tier-2
    /// rebuild started reading state. Re-applied on Tier-2 swap so concurrent
    /// patches aren't reverted by the swap. Cleared at the start of each
    /// Tier-2 rebuild (before the read-state phase).
    pub(crate) patched_during_rebuild: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Monotonic counter bumped every time a *full* graph swap lands —
    /// either the disk-scanning `rebuild_graph()` (explicit `refresh_graph`,
    /// `ensure_graph_fresh` auto-heal, batch finalize) or a Tier-2 background
    /// rebuild's own swap. In-place Tier-1 patches (`upsert_node_in_place`,
    /// `remove_node_in_place`) do NOT bump this — only whole-graph replacement
    /// does.
    ///
    /// Tier-2 reads this at the start of its (slow) rebuild and compares
    /// again just before its swap. A mismatch means a full rebuild already
    /// landed while Tier-2 was computing from an older `nodes_cloned()`
    /// snapshot; swapping anyway would silently revert whatever that fresher
    /// rebuild picked up from disk (e.g. a tag removed from frontmatter and
    /// then `refresh_graph`-ed away, per aops_mem_tag_integrity). On a
    /// mismatch, Tier-2 discards its stale result and re-marks itself dirty
    /// so the next iteration re-snapshots from the now-current graph instead
    /// of clobbering it.
    pub(crate) full_rebuild_epoch: Arc<std::sync::atomic::AtomicU64>,
    /// Test-only: optional sleep injected into Tier-2 between read and swap,
    /// used by the lost-patch race test to widen the window deterministically.
    #[cfg(test)]
    pub(crate) tier2_sleep_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Test-only: optional sleep injected into Tier-1 (`rebuild_graph`) and
    /// the Tier-2 swap, positioned immediately before the `full_rebuild_epoch`
    /// bump. On correct (lock-atomic) code this delay runs *inside* the write
    /// lock, so a concurrent rebuild attempting to acquire the lock simply
    /// blocks until the delay elapses and the bump has landed. Used by the
    /// TOCTOU regression test to make the bump-vs-swap-atomicity window
    /// deterministically observable instead of relying on a nanosecond-scale
    /// race.
    #[cfg(test)]
    pub(crate) pre_epoch_bump_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Counter incremented each time a Tier-2 rebuild actually executes
    /// (not coalesced). Used by the bench harness and tests to verify
    /// coalescing is working.
    pub(crate) tier2_executions: Arc<std::sync::atomic::AtomicU64>,
    /// Coalesced re-embed queue: path_key → latest `PkbDocument` whose body
    /// changed and needs re-chunking + ONNX encoding. Drained by a single
    /// background worker so the write handler never blocks on embed cost.
    /// Bursts of writes to the same document collapse to a single embed
    /// (latest doc wins).
    pub(crate) embed_pending: Arc<Mutex<HashMap<String, crate::pkb::PkbDocument>>>,
    /// Single-worker gate for the embed queue (mirrors `graph_rebuild_pending`).
    pub(crate) embed_worker_running: Arc<std::sync::atomic::AtomicBool>,
    /// Last reindex timestamp and outcome.
    pub(crate) last_reindex: Arc<RwLock<ReindexStatus>>,
    /// Stats from the last disk scan and graph rebuild.
    pub(crate) last_rebuild_stats: Arc<RwLock<RebuildStats>>,
    /// Mutex ensuring single-flight graph rebuilds under concurrent callers.
    pub(crate) rebuild_mutex: Arc<Mutex<()>>,
    /// Total count of full graph rebuilds performed.
    pub(crate) rebuild_count: Arc<std::sync::atomic::AtomicUsize>,
}

pub(crate) const DRY_RUN_WARNING: &str =
    "DRY RUN — no files modified. Pass dry_run=false to execute.\n\n";

impl PkbSearchServer {
    pub fn new(
        store: Arc<RwLock<VectorStore>>,
        embedder: Arc<Embedder>,
        pkb_root: PathBuf,
        db_path: PathBuf,
        graph: Arc<RwLock<GraphStore>>,
    ) -> Self {
        let reranker = Arc::new(crate::rerank::CrossEncoderReranker::new(
            crate::rerank::RerankerConfig::default(),
        ));
        let initial_scanned = crate::pkb::scan_directory(&pkb_root).len();
        let initial_loaded = graph.read().node_count();
        let initial_file_backed = graph
            .read()
            .nodes_map()
            .values()
            .filter(|n| !n.path.as_os_str().is_empty())
            .count();
        Self {
            store,
            embedder,
            reranker,
            pkb_root,
            db_path,
            graph: InstrumentedGraphStore::new(graph),
            session_registry: crate::otel::session_registry::SessionRegistry::new(),
            stale_count: 0,
            save_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            save_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            deferred_paths: Arc::new(Mutex::new(std::collections::HashSet::new())),
            lock_was_held: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drain_in_progress: Arc::new(Mutex::new(())),
            graph_rebuild_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            graph_rebuild_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            patched_during_rebuild: Arc::new(Mutex::new(std::collections::HashSet::new())),
            full_rebuild_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(test)]
            tier2_sleep_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(test)]
            pre_epoch_bump_delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            tier2_executions: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            embed_pending: Arc::new(Mutex::new(HashMap::new())),
            embed_worker_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_reindex: Arc::new(RwLock::new(ReindexStatus {
                timestamp: chrono::Utc::now().to_rfc3339(),
                outcome: "ok".to_string(),
            })),
            last_rebuild_stats: Arc::new(RwLock::new(RebuildStats {
                scanned_files: initial_scanned,
                parsed_documents: initial_file_backed,
                loaded_nodes: initial_loaded,
            })),
            rebuild_mutex: Arc::new(Mutex::new(())),
            rebuild_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn with_session_registry(
        mut self,
        reg: Arc<crate::otel::session_registry::SessionRegistry>,
    ) -> Self {
        self.session_registry = reg;
        self
    }

    pub fn with_stale_count(mut self, count: usize) -> Self {
        self.stale_count = count;
        self
    }

    /// Public sync entries to MCP handlers for benchmarking. Not part of
    /// the MCP API. Refs task-a4dcc039.
    #[doc(hidden)]
    pub fn bench_update_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_update_task(args)
    }
    #[doc(hidden)]
    pub fn bench_append_to_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_append_to_document(args)
    }
    #[doc(hidden)]
    pub fn bench_release_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_release_task(args)
    }
    #[doc(hidden)]
    pub fn bench_create_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_create_task(args)
    }
    #[doc(hidden)]
    pub fn bench_complete_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_complete_task(args)
    }
    #[doc(hidden)]
    pub fn bench_decompose_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_decompose_task(args)
    }
    #[doc(hidden)]
    pub fn bench_list_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_list_tasks(args)
    }
    #[doc(hidden)]
    pub fn bench_nested_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_nested_tasks(args)
    }
    #[doc(hidden)]
    pub fn bench_get_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_get_task(args)
    }
    #[doc(hidden)]
    pub fn bench_get_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_get_document(args)
    }
    #[doc(hidden)]
    pub fn bench_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_pkb_search(args)
    }

    #[doc(hidden)]
    pub fn store_for_test(&self) -> Arc<parking_lot::RwLock<VectorStore>> {
        self.store.clone()
    }

    pub(crate) fn acquire_store_read(&self) -> parking_lot::RwLockReadGuard<'_, VectorStore> {
        let tracer = opentelemetry::global::tracer("mem");
        use opentelemetry::trace::Tracer;
        let mut span = tracer.start("store_lock_acquisition");
        let guard = self.store.read();
        use opentelemetry::trace::Span;
        span.end();
        guard
    }

    pub(crate) fn acquire_store_write(&self) -> parking_lot::RwLockWriteGuard<'_, VectorStore> {
        let tracer = opentelemetry::global::tracer("mem");
        use opentelemetry::trace::Tracer;
        let mut span = tracer.start("store_lock_acquisition");
        let guard = self.store.write();
        use opentelemetry::trace::Span;
        span.end();
        guard
    }


    pub fn list_tools_result(&self) -> rmcp::model::ListToolsResult {
        let tools = Self::get_published_tools();
        let mut meta = rmcp::model::Meta::new();
        meta.0
            .insert("ttlMs".to_string(), serde_json::json!(86_400_000));
        let mut result = rmcp::model::ListToolsResult::with_all_items(tools);
        result.meta = Some(meta);
        result
    }

    // =========================================================================
    // SEARCH & DOCUMENT TOOLS
    // =========================================================================

    pub fn dispatch_tool_sync(
        &self,
        name: &str,
        args: &JsonValue,
    ) -> Result<CallToolResult, McpError> {
        let mut res = match name {
            "search" => self.handle_pkb_search(args),
            "get_document" => self.handle_get_document(args),
            "list_documents" => self.handle_list_documents(args),
            "task_search" => self.handle_task_search(args),
            "get_network_metrics" => self.handle_get_network_metrics(args),
            "top_n_by_metric" => self.handle_top_n_by_metric(args),
            "create_task" => self.handle_create_task(args),
            "claim_task" => self.handle_claim_task(args),
            "create_memory" => self.handle_create_memory(args),
            "create" => self.handle_create_document(args),
            "append" => self.handle_append_to_document(args),
            "add_observations" => self.handle_add_observations(args),
            "delete_observations" => self.handle_delete_observations(args),
            "update_body" => self.handle_update_body(args),
            "edit_body" | "edit" => self.handle_edit_body(args),
            "delete" => self.handle_delete_document(args),
            "convert_document" => self.handle_convert_document(args),
            "complete_task" => self.handle_complete_task(args),
            "release_task" => self.handle_release_task(args),
            "list_tasks" => self.handle_list_tasks(args),
            "nested_tasks" => self.handle_nested_tasks(args),
            "get_task" => self.handle_get_task(args),
            "update_task" => self.handle_update_task(args),
            "retrieve_memory" => self.handle_retrieve_memory(args),
            "detect_weight_divergence" => self.handle_detect_weight_divergence(args),
            "search_by_tag" => self.handle_search_by_tag(args),
            "list_memories" => self.handle_list_memories(args),
            "decompose_task" => self.handle_decompose_task(args),
            "get_dependency_tree" => self.handle_get_dependency_tree(args),
            "get_task_children" => self.handle_get_task_children(args),
            "pkb_trace" => self.handle_pkb_trace(args),
            "pkb_orphans" => self.handle_pkb_orphans(args),
            "get_semantic_neighbors" => self.handle_get_semantic_neighbors(args),
            "batch_update" => self.handle_batch_update(args),
            "batch_reparent" => self.handle_batch_reparent(args),
            "batch_archive" => self.handle_batch_archive(args),
            "graph_stats" => self.handle_graph_stats(args),
            "graph_excalidraw" => self.handle_graph_excalidraw(args),
            "export_graph" => self.handle_export_graph(args),
            "diff_excalidraw" => self.handle_diff_excalidraw(args),
            "sync_excalidraw" => self.handle_sync_excalidraw(args),
            "list_excalidraw" => self.handle_list_excalidraw(args),
            "get_excalidraw" => self.handle_get_excalidraw(args),
            "write_excalidraw" => self.handle_write_excalidraw(args),
            "task_summary" => self.handle_task_summary(args),
            "find_duplicates" => self.handle_find_duplicates(args),
            "batch_merge" => self.handle_batch_merge(args),
            "merge_node" => self.handle_merge_node(args),
            "batch_create_epics" => self.handle_batch_create_epics(args),
            "batch_reclassify" => self.handle_batch_reclassify(args),
            "get_stats" => self.handle_get_stats(args),
            "status" => self.handle_status(args),
            "refresh_graph" => self.handle_refresh_graph(args),
            "repair_index_orphans" => self.handle_repair_index_orphans(args),
            "get_consolidation_cluster" => self.handle_get_consolidation_cluster(args),
            "apply_consolidation_batch" => self.handle_apply_consolidation_batch(args),
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown tool: {name}")),
                data: None,
            }),
        };

        if let Ok(ref mut result) = res {
            if result.structured_content.is_none() {
                let text = result
                    .content
                    .iter()
                    .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                    .collect::<String>();
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    result.structured_content = Some(val);
                } else {
                    result.structured_content = Some(serde_json::json!({
                        "ok": true,
                        "text": text,
                    }));
                }
            }
        }

        res
    }

    pub fn is_read_only_tool(name: &str) -> bool {
        static READ_ONLY_TOOLS: std::sync::OnceLock<std::collections::HashSet<String>> =
            std::sync::OnceLock::new();
        let set = READ_ONLY_TOOLS.get_or_init(|| {
            Self::get_all_tools()
                .into_iter()
                .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
                .map(|t| t.name.to_string())
                .collect()
        });
        set.contains(name)
    }

    pub async fn execute_tool_call(
        &self,
        tool_name: &str,
        args: &JsonValue,
        parts: Option<&http::request::Parts>,
        meta: Option<&rmcp::model::Meta>,
    ) -> Result<CallToolResult, McpError> {
        let start = std::time::Instant::now();
        let tool_name = tool_name.to_string();
        let mut args = args.clone();

        let client_context = crate::otel::client_context::ClientContext::extract(
            parts,
            meta,
            &mut args,
            &self.session_registry,
        );

        if let Some(ref mcp_id) = client_context.mcp_session_id {
            self.session_registry.update_activity(mcp_id);
        }

        let canonical_name = match tool_name.as_str() {
            "pkb_trace" | "pkb_orphans" => tool_name.clone(),
            other => other.strip_prefix("pkb_").unwrap_or(other).to_string(),
        };

        let is_read_only = Self::is_read_only_tool(&canonical_name);
        let is_mutating = !is_read_only;
        let is_unattributed =
            client_context.session_source == crate::otel::client_context::SessionSource::None;
        let session_mode = crate::otel::config::SessionMode::from_env();
        let log_session_id = client_context.session_id.clone();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let this_task = self.clone();
        let canonical_name_task = canonical_name.clone();

        tokio::task::spawn_blocking(move || {
            crate::otel::call_metrics::CallMetrics::start_recording();

            let parent_cx = client_context
                .parent_context
                .unwrap_or_else(opentelemetry::Context::current);

            let tracer = opentelemetry::global::tracer("mem");
            use opentelemetry::trace::Tracer;

            let mut builder = tracer
                .span_builder(format!("mcp.{}", canonical_name_task))
                .with_kind(opentelemetry::trace::SpanKind::Server);

            let mut attrs = vec![
                opentelemetry::KeyValue::new("openinference.span.kind", "TOOL"),
                opentelemetry::KeyValue::new("tool.name", canonical_name_task.clone()),
                opentelemetry::KeyValue::new(
                    "aops.session.source",
                    client_context.session_source.as_str(),
                ),
            ];

            if let Some(ref sid) = client_context.session_id {
                attrs.push(opentelemetry::KeyValue::new("session.id", sid.clone()));
            }
            if let Some(ref rebound) = client_context.rebound_from {
                attrs.push(opentelemetry::KeyValue::new(
                    "aops.session.rebound_from",
                    rebound.clone(),
                ));
            }
            if let Some(ref aid) = client_context.agent_id {
                attrs.push(opentelemetry::KeyValue::new("agent.id", aid.clone()));
            }
            if let Some(ref pid) = client_context.prompt_id {
                attrs.push(opentelemetry::KeyValue::new("prompt.id", pid.clone()));
            }
            if is_mutating
                && is_unattributed
                && session_mode == crate::otel::config::SessionMode::Warn
            {
                attrs.push(opentelemetry::KeyValue::new("aops.session.missing", true));
            }
            if let Some(ref tid) = client_context.task_id {
                attrs.push(opentelemetry::KeyValue::new(
                    "tag.tags",
                    format!("[\"task:{}\"]", tid),
                ));
            }
            if let Some(ref mcp_id) = client_context.mcp_session_id {
                attrs.push(opentelemetry::KeyValue::new(
                    "mcp.session_id",
                    mcp_id.clone(),
                ));
            }

            let input_str = serde_json::to_string(&args).unwrap_or_default();
            let truncated_input = Self::truncate_span_value(input_str);
            attrs.push(opentelemetry::KeyValue::new("input.value", truncated_input));
            attrs.push(opentelemetry::KeyValue::new(
                "input.mime_type",
                "application/json",
            ));

            builder = builder.with_attributes(attrs);

            let span = tracer.build_with_context(builder, &parent_cx);
            let cx = parent_cx.with_span(span);
            let _guard = cx.clone().attach();

            let res = if is_mutating
                && is_unattributed
                && session_mode == crate::otel::config::SessionMode::Enforce
            {
                Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Unattributed write refused: tool '{canonical_name_task}' requires a caller session ID when PKB_SESSION_MODE=enforce. \
                         Supply your session ID via the `_session` argument (e.g. `{{\"_session\": {{\"session_id\": \"<your-session-id>\"}}}}`). \
                         You can find your session ID in the environment variable AOPS_SESSION_ID or your agent session context."
                    )),
                    data: None,
                })
            } else {
                if is_mutating && is_unattributed {
                    tracing::warn!(
                        "Unattributed write to tool '{}' in warning mode (missing session id)",
                        canonical_name_task
                    );
                }
                this_task.dispatch_tool_sync(&canonical_name_task, &args)
            };

            let metrics =
                crate::otel::call_metrics::CallMetrics::finish_recording().unwrap_or_default();

            // Add output attributes
            let is_error = res.is_err();
            let output_str = match &res {
                Ok(r) => serde_json::to_string(r).unwrap_or_default(),
                Err(e) => serde_json::to_string(e).unwrap_or_default(),
            };
            let response_bytes = output_str.len();
            let truncated_output = Self::truncate_span_value(output_str);

            use opentelemetry::trace::TraceContextExt;
            let current_span = cx.span();

            current_span.set_attribute(opentelemetry::KeyValue::new(
                "output.value",
                truncated_output,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "output.mime_type",
                "application/json",
            ));

            // Required per-call attributes
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "result_bytes",
                response_bytes as i64,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "result-bytes",
                response_bytes as i64,
            ));

            let graph_read_wait_ms = (metrics.graph_read_wait_us as f64) / 1000.0;
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "graph.read_wait_ms",
                graph_read_wait_ms,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "graph_read_wait_ms",
                graph_read_wait_ms,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "graph.read.wait_ms",
                graph_read_wait_ms,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "graph.read_count",
                metrics.graph_read_count as i64,
            ));

            current_span.set_attribute(opentelemetry::KeyValue::new(
                "rebuild_flag",
                metrics.rebuilt,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "ensure_graph_fresh.rebuilt",
                metrics.rebuilt,
            ));

            let embed_time_ms = (metrics.embed_time_us as f64) / 1000.0;
            current_span
                .set_attribute(opentelemetry::KeyValue::new("embed_time_ms", embed_time_ms));
            current_span
                .set_attribute(opentelemetry::KeyValue::new("embed.time_ms", embed_time_ms));

            let hybrid_search_time_ms = (metrics.hybrid_search_time_us as f64) / 1000.0;
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "hybrid_search_time_ms",
                hybrid_search_time_ms,
            ));
            current_span.set_attribute(opentelemetry::KeyValue::new(
                "hybrid_search.time_ms",
                hybrid_search_time_ms,
            ));

            if is_error {
                current_span.set_status(opentelemetry::trace::Status::Error {
                    description: std::borrow::Cow::Borrowed("Tool error"),
                });
            } else {
                current_span.set_status(opentelemetry::trace::Status::Ok);
            }
            current_span.end();

            drop(_guard);

            let _ = tx.send((res, response_bytes, is_error));
        });

        let (result, response_bytes, is_error) = rx.await.map_err(|_| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from("Internal error in tool execution task"),
            data: None,
        })?;

        let latency = start.elapsed().as_millis();
        crate::telemetry::record_call(&tool_name, response_bytes, latency, is_error);

        match &log_session_id {
            None => {
                tracing::info!(
                    target: "pkb::tool_call",
                    tool = %tool_name,
                    status = %if is_error { "error" } else { "ok" },
                    latency_ms = latency as u64,
                    response_bytes,
                    "tool call"
                );
            }
            Some(sid) => {
                tracing::info!(
                    target: "pkb::tool_call",
                    tool = %tool_name,
                    status = %if is_error { "error" } else { "ok" },
                    latency_ms = latency as u64,
                    response_bytes,
                    session = %sid,
                    "tool call"
                );
            }
        }

        result
    }
}

impl ServerHandler for PkbSearchServer {
    fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::InitializeResult, McpError>> + Send + '_
    {
        let client_name = request.client_info.name.clone();
        let mcp_session_id = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.headers.get("mcp-session-id"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if let Some(mcp_id) = mcp_session_id {
            self.session_registry
                .register_handshake(&mcp_id, Some(client_name));
        }

        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        std::future::ready(Ok(self.get_info()))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let args = Self::args_to_value(request.arguments);
        let name = request.name.clone();
        let parts = context.extensions.get::<http::request::Parts>().cloned();
        let meta = request.meta.clone();
        let ct = context.ct.clone();
        let this = self.clone();

        async move {
            tokio::select! {
                res = this.execute_tool_call(&name, &args, parts.as_ref(), meta.as_ref()) => res,
                _ = ct.cancelled() => {
                    tracing::warn!("Session terminated while tool '{}' was in flight", name);
                    Err(McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from("Session terminated"),
                        data: None,
                    })
                }
            }
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(self.list_tools_result()))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(self.handle_list_prompts())
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        std::future::ready(self.handle_get_prompt(request))
    }

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_server_info(Implementation::new("pkb", env!("CARGO_PKG_VERSION")));

        if self.stale_count > 0 {
            info = info.with_instructions(format!(
                "WARNING: Index is stale — {} document(s) need re-indexing. \
                 Search results may be incomplete or outdated.",
                self.stale_count
            ));
        }
        info
    }
}

impl FactsProvider for PkbSearchServer {}
