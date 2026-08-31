//! MCP server for PKB semantic search + task graph.
//! 
//! Implements rmcp 0.1.5 ServerHandler trait manually with tool dispatch.
//! Provides 18 tools for search, documents, tasks, and knowledge graph.

pub mod handlers_batch;
pub mod handlers_document;
pub mod handlers_prompt;
pub mod handlers_search;
pub mod handlers_task;
pub mod handlers_task_lifecycle;
pub mod helpers;
pub mod schemas;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tier_rebuild_tests;
#[cfg(test)]
mod cross_process_recovery_tests;
#[cfg(test)]
mod claim_task_tests;

use crate::embeddings::Embedder;
use crate::facts::FactsProvider;
use crate::graph::is_completed;
use crate::graph_store::{GraphStore, DEFAULT_DIVERGENCE_THRESHOLD_DAYS};
use crate::vectordb::VectorStore;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(crate) const GOAL_TYPE_ENUM: &[&str] = &["committed", "aspirational", "learning"];

/// Hard ceiling on any client-supplied result-count parameter (`limit`,
/// `max_results`, `top_n`/`n`, etc). Applied on top of each tool's own
/// default — defaults are unchanged, this only prevents a client from
/// requesting an unbounded/huge result set that could blow out its own
/// context window. Tools with deliberate "0/None = unlimited" semantics
/// (e.g. `pkb_orphans`) are exempt for the unlimited case by design.
pub(crate) const MAX_RESULTS: usize = 1000;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReindexStatus {
    pub timestamp: String,
    pub outcome: String,
}

#[derive(Clone)]
pub struct PkbSearchServer {
    pub(crate) store: Arc<RwLock<VectorStore>>,
    pub(crate) embedder: Arc<Embedder>,
    pub(crate) reranker: Arc<crate::rerank::CrossEncoderReranker>,
    pub(crate) pkb_root: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) graph: Arc<RwLock<GraphStore>>,
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
    /// Test-only: optional sleep injected into Tier-2 between read and swap,
    /// used by the lost-patch race test to widen the window deterministically.
    #[cfg(test)]
    pub(crate) tier2_sleep_ms: Arc<std::sync::atomic::AtomicU64>,
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
}

pub(crate) const DRY_RUN_WARNING: &str = "DRY RUN — no files modified. Pass dry_run=false to execute.\n\n";

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
        Self {
            store,
            embedder,
            reranker,
            pkb_root,
            db_path,
            graph,
            stale_count: 0,
            save_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            save_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            deferred_paths: Arc::new(Mutex::new(std::collections::HashSet::new())),
            lock_was_held: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drain_in_progress: Arc::new(Mutex::new(())),
            graph_rebuild_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            graph_rebuild_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            patched_during_rebuild: Arc::new(Mutex::new(std::collections::HashSet::new())),
            #[cfg(test)]
            tier2_sleep_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            tier2_executions: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            embed_pending: Arc::new(Mutex::new(HashMap::new())),
            embed_worker_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_reindex: Arc::new(RwLock::new(ReindexStatus {
                timestamp: chrono::Utc::now().to_rfc3339(),
                outcome: "ok".to_string(),
            })),
        }
    }

    /// Number of Tier-2 background rebuilds that have actually executed.
    /// Used by bench/tests to measure coalescing effectiveness.
    #[doc(hidden)]
    pub fn tier2_execution_count(&self) -> u64 {
        self.tier2_executions
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Has a Tier-2 rebuild been queued (or is one running)?
    /// Bench harness polls this to know when the graph is fully consistent.
    #[doc(hidden)]
    pub fn graph_rebuild_pending(&self) -> bool {
        self.graph_rebuild_pending
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    #[doc(hidden)]
    pub fn set_tier2_sleep_ms(&self, ms: u64) {
        self.tier2_sleep_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
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


    /// Cheap staleness signal for list/filter surfaces (aops_fb137646 AC2).
    ///
    /// A silent short list — the served graph under-returning relative to
    /// disk with no indication — is the defect; a warned one is a known
    /// limitation. This can't detect every under-return (a swapped-in node
    /// with an unrelated node dropped would leave the count unchanged), but
    /// it catches the reproduced shape: a file that landed on disk without
    /// this process's in-memory graph ever observing it (e.g. written by a
    /// different `pkb mcp` process, or by direct disk/sync).
    ///
    /// Costs one directory walk (path listing only, no file parsing) against
    /// the in-memory node count. Returns `Some((disk_count, index_count))`
    /// only when they disagree, so the common (fresh) case pays for the walk
    /// but returns no extra payload.
    ///
    /// Counts only file-backed nodes on the index side — `GraphStore::build`
    /// also synthesizes "ghost" nodes for dangling references (e.g. a
    /// `parent:` that names no file, which `allow_missing_parent` permits by
    /// design). Ghost nodes carry an empty `path` (never written to disk),
    /// so including them would make every PKB with a dangling reference
    /// warn permanently on a correctly-synced index.
    pub(crate) fn list_staleness_signal(&self) -> Option<(usize, usize)> {
        let disk_count = crate::pkb::scan_directory(&self.pkb_root).len();
        let index_count = self
            .graph
            .read()
            .nodes_map()
            .values()
            .filter(|n| !n.path.as_os_str().is_empty())
            .count();
        if disk_count == index_count {
            None
        } else {
            Some((disk_count, index_count))
        }
    }

    /// Reconstruct an absolute path from a (possibly relative) graph node path.
    /// Full rebuild of the graph store from disk (for batch operations).
    ///
    /// Concurrency: takes the store read lock just long enough to snapshot
    /// the per-document averaged embeddings, then drops it. The (slow)
    /// scan + parse + similarity-edge build runs against the snapshot with
    /// no store lock held — concurrent writers can proceed during the
    /// rebuild instead of blocking behind it.
    pub(crate) fn rebuild_graph(&self) {
        let _t_snap = std::time::Instant::now();
        let snapshot = self.store.read().averaged_embeddings();
        tracing::debug!(
            target: "perf::graph_rebuild",
            phase = "embedding_snapshot",
            n = snapshot.len(),
            elapsed_ms = _t_snap.elapsed().as_secs_f64() * 1000.0
        );

        // Clear patched tracker at start so any concurrent writes landing during
        // scan and build are tracked and re-applied at swap time.
        self.patched_during_rebuild.lock().clear();

        let files = crate::pkb::scan_directory(&self.pkb_root);
        let docs: Vec<crate::pkb::PkbDocument> = files
            .par_iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, &self.pkb_root))
            .collect();

        let mut new_graph = GraphStore::build_with_embeddings(&docs, &self.pkb_root, &snapshot);
        {
            let mut g = self.graph.write();
            let patched_ids: Vec<String> =
                self.patched_during_rebuild.lock().iter().cloned().collect();
            for id in &patched_ids {
                if let Some(live_node) = g.nodes_map().get(id).cloned() {
                    new_graph.replace_node(live_node);
                }
            }
            if !patched_ids.is_empty() {
                new_graph.reclassify();
            }
            *g = new_graph;
        }
        *self.last_reindex.write() = ReindexStatus {
            timestamp: chrono::Utc::now().to_rfc3339(),
            outcome: "ok".to_string(),
        };
    }

    /// Synchronous fast path after a single file changed.
    ///
    /// Patches the node in place under the graph write lock (microseconds),
    /// runs cheap classification so `list_tasks(status="ready")` membership
    /// reflects the new status, and schedules a coalesced background
    /// rebuild that refreshes derived metrics (downstream_weight,
    /// effective_priority, focus_score, target_ancestors, similarity edges).
    ///
    /// Consistency: after this returns, `resolve(id)` finds the node, its
    /// `status`/`parent`/`priority` are fresh, and ready/blocked membership
    /// is fresh. Derived metric *values* (downstream_weight, focus_score,
    /// urgency ordering) lag by 1–3s until the background rebuild lands —
    /// the prior values are carried over from the existing node so they
    /// don't reset to defaults.
    ///
    /// Concurrency: the graph write lock serialises concurrent writers
    /// safely (each patch is a HashMap insert; no clone-and-swap race).
    pub(crate) fn rebuild_graph_for_pkb_document(&self, doc: &crate::pkb::PkbDocument) {
        let node = crate::graph::GraphNode::from_pkb_document(doc);
        let patched_id = node.id.clone();

        let _t = std::time::Instant::now();
        {
            let mut g = self.graph.write();
            g.upsert_node_in_place(node, &self.pkb_root);
            g.reclassify();
            // Insert inside the write lock so the ID is visible to any
            // concurrent Tier-2 phase-2 swap before the write lock is released.
            // If inserted after the lock, a Tier-2 that already holds the
            // write lock for its swap will miss this ID and drop the node.
            self.patched_during_rebuild.lock().insert(patched_id);
        }
        tracing::debug!(target: "perf::graph_rebuild", phase = "inplace_patch", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // Schedule the coalesced background full rebuild (edges, downstream
        // metrics, target ancestors, similarity edges).
        self.schedule_graph_rebuild();
    }

    /// Batch in-place patch for multiple documents. Holds the graph write
    /// lock once, upserts all nodes, runs reclassify once, then schedules
    /// a single coalesced background rebuild. Use this after creating
    /// multiple documents atomically (e.g. decompose_task) to avoid N
    /// separate lock acquisitions.
    pub(crate) fn batch_upsert_and_rebuild(&self, docs: Vec<crate::pkb::PkbDocument>) {
        let _t = std::time::Instant::now();
        {
            let mut g = self.graph.write();
            let mut patched = self.patched_during_rebuild.lock();
            for doc in &docs {
                let node = crate::graph::GraphNode::from_pkb_document(doc);
                let patched_id = node.id.clone();
                g.upsert_node_in_place(node, &self.pkb_root);
                patched.insert(patched_id);
            }
            g.reclassify();
        }
        tracing::debug!(
            target: "perf::graph_rebuild",
            phase = "inplace_batch_patch",
            n = docs.len(),
            elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0
        );
        self.schedule_graph_rebuild();
    }

    /// Synchronous fast path for node removal. In-place delete from the
    /// nodes map + resolution map. Stale edges are left for the background
    /// rebuild to clean up. See [`rebuild_graph_for_pkb_document`] for
    /// the consistency model.
    pub(crate) fn rebuild_graph_remove(&self, id: &str) {
        let _t = std::time::Instant::now();
        {
            let mut g = self.graph.write();
            g.remove_node_in_place(id);
            g.reclassify();
            // Same as rebuild_graph_for_pkb_document: insert inside the write
            // lock so the removal is visible to any concurrent Tier-2 swap.
            self.patched_during_rebuild.lock().insert(id.to_string());
        }
        tracing::debug!(target: "perf::graph_rebuild", phase = "inplace_remove", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        self.schedule_graph_rebuild();
    }

    /// Schedule a background (Tier-2) graph rebuild that recomputes
    /// similarity edges. Coalesces burst writes so a flurry of
    /// `update_task` calls produces at most ONE in-flight rebuild plus
    /// at most ONE queued follow-up.
    ///
    /// Concurrency model (single-worker / dirty-flag):
    ///   - Every call sets `graph_rebuild_dirty=true`. The worker reads
    ///     and clears this flag at the *start* of each iteration; if it
    ///     was set, it runs another rebuild; if clear, the worker exits.
    ///     This means writes that arrive during a rebuild are coalesced
    ///     into a single follow-up iteration, regardless of count.
    ///   - `graph_rebuild_pending` gates worker *spawning*: only the
    ///     caller that flips it false→true spawns the closure. All other
    ///     callers just mark dirty. The worker clears pending only after
    ///     it has observed dirty=false, with a re-check to avoid the
    ///     classic check-then-store race.
    ///   - `patched_during_rebuild` is drained at the start of each
    ///     iteration (after consuming `dirty`), so it captures only IDs
    ///     landing during this iteration's read-to-swap window.
    ///   - Patch-merge runs in two phases: most of the work (replace_node
    ///     for IDs known at read-end + reclassify) happens *without* the
    ///     graph write lock; only the swap (and a fold of any late
    ///     patches that landed during phase 1) holds the write lock.
    pub(crate) fn schedule_graph_rebuild(&self) {
        use std::sync::atomic::Ordering;

        // Mark there's work to do. If a worker is already running, it
        // will pick this up on its next loop iteration.
        self.graph_rebuild_dirty.store(true, Ordering::SeqCst);

        // Try to claim the worker slot. Only the caller that flips
        // pending false→true is responsible for spawning.
        if self.graph_rebuild_pending.swap(true, Ordering::SeqCst) {
            return;
        }

        let graph = self.graph.clone();
        let store = self.store.clone();
        let pkb_root = self.pkb_root.clone();
        let pending = self.graph_rebuild_pending.clone();
        let dirty = self.graph_rebuild_dirty.clone();
        let patched = self.patched_during_rebuild.clone();
        let executions = self.tier2_executions.clone();
        let last_reindex = self.last_reindex.clone();
        #[cfg(test)]
        let sleep_ms = self.tier2_sleep_ms.clone();

        let do_rebuild_once = move || {
            let _t_total = std::time::Instant::now();

            // Drain the patch tracker. Patches arriving after this point
            // are recorded for the *next* iteration (the dirty flag will
            // be set by their schedule_graph_rebuild call).
            patched.lock().clear();

            let snapshot = store.read().averaged_embeddings();
            let nodes = graph.read().nodes_cloned();

            let new_graph =
                GraphStore::rebuild_from_nodes_fast_with_embeddings(nodes, &pkb_root, &snapshot);

            #[cfg(test)]
            {
                let ms = sleep_ms.load(Ordering::Relaxed);
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }

            // Patch-merge in two phases to minimise the write-lock window.
            //
            // Phase 1 (no graph write lock): snapshot the patches that
            // landed between our read and now, clone the live state of
            // those nodes under a brief read lock, fold them into the
            // local `merged` graph, and reclassify. This is the expensive
            // step (reclassify is O(V+E)) but no Tier-1 caller is blocked.
            //
            // Phase 2 (write lock): fold any *late* patches that landed
            // during Phase 1, then swap. In the common case `late` is
            // empty and the locked section is just a pointer assignment.
            let mut merged = new_graph;
            let initial_patched: HashSet<String> = patched.lock().clone();
            let live_initial: Vec<crate::graph::GraphNode> = {
                let g = graph.read();
                initial_patched
                    .iter()
                    .filter_map(|id| g.nodes_map().get(id).cloned())
                    .collect()
            };
            for live_node in live_initial {
                merged.replace_node(live_node);
            }
            let mut reclassified = false;
            if !initial_patched.is_empty() {
                merged.reclassify();
                reclassified = true;
            }

            let mut g = graph.write();
            let late: Vec<String> = patched
                .lock()
                .iter()
                .filter(|id| !initial_patched.contains(*id))
                .cloned()
                .collect();
            for id in &late {
                if let Some(live_node) = g.nodes_map().get(id).cloned() {
                    merged.replace_node(live_node);
                }
                // If the node no longer exists in the live graph (e.g.
                // delete landed mid-rebuild), the just-built graph either
                // doesn't have it either, or we leave the just-built copy
                // — a follow-up rebuild will reconcile.
            }
            if !late.is_empty() {
                merged.reclassify();
                reclassified = true;
            }
            *g = merged;
            drop(g);
            let n_merged = initial_patched.len() + late.len();
            let _ = reclassified;

            *last_reindex.write() = ReindexStatus {
                timestamp: chrono::Utc::now().to_rfc3339(),
                outcome: "ok".to_string(),
            };

            executions.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                target: "perf::graph_rebuild",
                phase = "tier2_TOTAL",
                n_patched_merged = n_merged,
                n_late = late.len(),
                elapsed_ms = _t_total.elapsed().as_secs_f64() * 1000.0
            );
        };

        // Worker loop: run rebuilds while dirty stays set. The order
        // (consume dirty → run → re-check) is the key invariant: a write
        // arriving after we consume `dirty` and before we finish always
        // re-sets `dirty`, so the next loop iteration picks it up. After
        // observing dirty=clean we release `pending`; a subsequent write
        // will then spawn a fresh worker.
        let worker = move || {
            loop {
                // Consume the dirty flag for this iteration.
                if !dirty.swap(false, Ordering::SeqCst) {
                    // Nothing to do. Release the worker slot so the next
                    // schedule call can spawn a worker.
                    pending.store(false, Ordering::SeqCst);
                    // Re-check: a write may have set dirty after we
                    // consumed it but before we cleared pending. If so,
                    // try to reclaim the slot and continue. If another
                    // caller already grabbed it, we're done.
                    if dirty.load(Ordering::SeqCst)
                        && pending
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        continue;
                    }
                    return;
                }
                do_rebuild_once();
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                tokio::task::spawn_blocking(worker);
            }
            Err(_) => {
                // No tokio runtime (CLI / test path) — run inline.
                worker();
            }
        }
    }

    /// Check whether the index file lock is available (no reindex in progress).
    pub(crate) fn index_lock_available(&self) -> bool {
        match VectorStore::acquire_lock(&self.db_path) {
            Ok(mut lock) => match lock.try_write() {
                Ok(_guard) => true,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    /// True when an *external* process (e.g. `pkb reindex`) holds the
    /// cross-process index file lock. False when the lock is available OR
    /// when our own background save thread is the one holding it. This is
    /// the predicate the upsert path should consult when deciding whether
    /// to defer in-memory writes — deferring on a self-save triggers an
    /// expensive disk reload on the next call (the thing the
    /// `save_in_flight` flag exists to avoid).
    pub(crate) fn external_lock_held(&self) -> bool {
        if self
            .save_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        !self.index_lock_available()
    }

    /// Return path to the Write-Ahead Log (.wal) file for vector persistence.
    pub(crate) fn wal_path(&self) -> PathBuf {
        crate::vectordb::wal_path(&self.db_path)
    }

    /// Save the vector store snapshot to disk asynchronously when compaction is needed.
    ///
    /// Single writes append to the incremental WAL (`pkb_vectors.wal`) in microseconds.
    /// `save_store` checks if WAL compaction is needed (e.g. WAL size exceeds threshold)
    /// and if so, compacts by writing a full snapshot and removing the WAL file.
    pub(crate) fn save_store(&self) {
        let wal_p = self.wal_path();
        let wal_compacting_p = crate::vectordb::wal_compacting_path(&self.db_path);
        let should_compact = match std::fs::metadata(&wal_p) {
            Ok(meta) => meta.len() >= 16 * 1024 * 1024,
            Err(_) => false,
        } || wal_compacting_p.exists();
        if !should_compact {
            return;
        }

        // Coalesce: if a background save is already scheduled, skip — it will
        // read the latest in-memory state when it runs.
        if self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }

        let store = self.store.clone();
        let db_path = self.db_path.clone();
        let pending = self.save_pending.clone();
        let in_flight = self.save_in_flight.clone();

        let do_save = move || {
            // Mark self-save in flight BEFORE acquiring the file lock.
            // Concurrent upsert calls observing a non-available file lock
            // will see this flag and skip the defer-and-reload path that
            // would otherwise trigger a 1–2s store reload on the next call.
            in_flight.store(true, Ordering::SeqCst);
            // Clear save_pending BEFORE serializing: any concurrent write
            // that arrives during serialize will schedule a follow-up save
            // (rather than be silently dropped).
            pending.store(false, Ordering::SeqCst);

            let t_bg = std::time::Instant::now();

            let t_lock = std::time::Instant::now();
            match VectorStore::acquire_lock(&db_path) {
                Ok(mut lock) => {
                    let elapsed_lock_acquire = t_lock.elapsed();
                    let t_write_lock = std::time::Instant::now();
                    match lock.try_write() {
                        Ok(_guard) => {
                            let elapsed_write_lock = t_write_lock.elapsed();
                            let t_save = std::time::Instant::now();
                            if let Err(e) = store.read().save(&db_path) {
                                tracing::error!("Failed to save vector store: {e}");
                            }
                            let elapsed_save = t_save.elapsed();
                            tracing::debug!(
                                target: "perf::vector",
                                phase = "bg_save_success",
                                lock_acquire_ms = elapsed_lock_acquire.as_secs_f64() * 1000.0,
                                write_lock_ms = elapsed_write_lock.as_secs_f64() * 1000.0,
                                save_ms = elapsed_save.as_secs_f64() * 1000.0,
                                elapsed_ms = t_bg.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            tracing::info!(
                                "Vector store lock held by another process — disk save deferred"
                            );
                            tracing::debug!(
                                target: "perf::vector",
                                phase = "bg_save_deferred",
                                lock_acquire_ms = elapsed_lock_acquire.as_secs_f64() * 1000.0,
                                elapsed_ms = t_bg.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to acquire write lock for save: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to open lock file for save: {e}");
                }
            }

            in_flight.store(false, Ordering::SeqCst);
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

    /// Resolve an absolute PKB path back to the relative key used in the
    /// vector store. Falls back to the input path if it isn't under
    /// `pkb_root` (defensive).
    #[allow(dead_code)]
    pub(crate) fn rel_key_for(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.pkb_root)
            .unwrap_or(abs)
            .to_string_lossy()
            .to_string()
    }

    /// Self-heal the in-memory store after the cross-process index lock is
    /// released by another process (typically `pkb reindex`).
    ///
    /// Behaviour:
    /// - If the lock is currently held: mark `lock_was_held=true` and return.
    /// - If the lock is now available AND we previously observed it held:
    ///   reload the store from disk (the reindex is the authoritative
    ///   producer) and replay any deferred upserts/removes that arrived
    ///   while we were locked out. Re-checks each deferred path's existence
    ///   to decide upsert vs. remove.
    /// - If the lock is currently available and there are deferred entries
    ///   (e.g. transient lock contention without reload), still drain them.
    ///
    /// Concurrency: a single-thread `drain_in_progress` mutex prevents
    /// concurrent drains from racing on `store.write()`. If another thread
    /// is already draining, we return immediately — the in-flight drain
    /// will pick up newly-deferred entries when it iterates.
    pub(crate) fn maybe_drain_deferred(&self) {
        // Fast path: lock is available, nothing was deferred, nothing to do.
        // We treat a lock held by our own save thread as "available" — only
        // an *external* holder should drive `lock_was_held=true` (which in
        // turn triggers a full disk reload below).
        let externally_locked = self.external_lock_held();
        if externally_locked {
            self.lock_was_held.store(true, Ordering::Relaxed);
            return;
        }
        let was_held = self.lock_was_held.load(Ordering::Relaxed);
        let queue_empty = self.deferred_paths.lock().is_empty();
        if !was_held && queue_empty {
            return;
        }

        // Only one drainer at a time. If another thread is already inside
        // the slow path, drop the work — they'll handle our deferred items.
        let _drain = match self.drain_in_progress.try_lock() {
            Some(g) => g,
            None => return,
        };

        // Re-check now that we hold the drain lock.
        if self.external_lock_held() {
            self.lock_was_held.store(true, Ordering::Relaxed);
            return;
        }

        let was_held = self.lock_was_held.swap(false, Ordering::Relaxed);

        if was_held {
            // Adopt the reindex's authoritative on-disk state.
            let dim = self.store.read().dimension_or_default();
            match VectorStore::load_or_create(&self.db_path, dim) {
                Ok(fresh) => {
                    tracing::info!(
                        "Cross-process index lock released — reloaded vector store from disk ({} docs)",
                        fresh.len()
                    );
                    *self.store.write() = fresh;
                }
                Err(e) => {
                    tracing::warn!(
                        "Cross-process lock released but reload failed: {e} — keeping in-memory state"
                    );
                }
            }
        }

        // Replay deferred paths against the (possibly refreshed) store.
        let to_replay: std::collections::HashSet<PathBuf> = {
            let mut q = self.deferred_paths.lock();
            std::mem::take(&mut *q)
        };
        if to_replay.is_empty() {
            // Reload alone is reason enough to refresh the graph view —
            // the reindex may have touched files we didn't observe.
            if was_held {
                self.rebuild_graph();
                self.save_store();
            }
            return;
        }

        tracing::info!(
            "Replaying {} deferred path(s) after cross-process lock release",
            to_replay.len()
        );

        let mut applied_upserts = 0usize;
        let mut applied_removes = 0usize;
        for abs_path in &to_replay {
            if abs_path.exists() {
                if let Some(doc) = crate::pkb::parse_file_relative(abs_path, &self.pkb_root) {
                    self.try_upsert_document(&doc);
                    applied_upserts += 1;
                }
            } else {
                let rel = abs_path.strip_prefix(&self.pkb_root).unwrap_or(abs_path);
                let node_id = self
                    .graph
                    .read()
                    .nodes()
                    .find(|n| n.path == rel)
                    .map(|n| n.id.clone());
                let id = node_id.unwrap_or_else(|| crate::pkb::fallback_id(rel));
                if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
                    &self.wal_path(),
                    &crate::vectordb::WalRecord::Remove(id.clone()),
                ) {
                    tracing::error!("maybe_drain_deferred: Failed to append remove to WAL: {e}");
                }
                if self.store.write().remove(&id) {
                    applied_removes += 1;
                }
            }
        }

        tracing::info!(
            "Deferred drain complete: {applied_upserts} upserts, {applied_removes} removes"
        );

        // Refresh graph + persist exactly once for the whole drain.
        self.rebuild_graph();
        self.save_store();
    }

    /// Index a document into the vector store if the index is not locked by a
    /// reindex. When a reindex is in progress the markdown file is already
    /// written and the graph already updated — the reindex will pick up the
    /// new/changed file, so we defer the in-memory upsert to be replayed
    /// when the lock is released (see `maybe_drain_deferred`).
    ///
    /// Fast path: for documents that already have an entry, apply the
    /// metadata patch synchronously (microseconds) and queue the body
    /// re-embed for the background worker. The write handler returns
    /// without waiting for ONNX. For new documents (no existing entry),
    /// take the inline ONNX hit so the entry is immediately searchable —
    /// this is rare relative to update/append/release_task on existing docs.
    pub(crate) fn try_upsert_document(&self, doc: &crate::pkb::PkbDocument) {
        // Self-heal first: if the lock just became available, adopt the
        // reindex's state and drain anything queued.
        self.maybe_drain_deferred();

        // Only defer when an *external* process holds the file lock. When
        // our own background save thread is the holder, the in-memory
        // state is the source of truth — we should proceed with the
        // upsert, not defer (and not flag `lock_was_held=true`, which
        // would trigger a needless disk reload on the next call).
        if self.external_lock_held() {
            let abs = if doc.path.is_absolute() {
                doc.path.clone()
            } else {
                self.pkb_root.join(&doc.path)
            };
            self.deferred_paths.lock().insert(abs);
            self.lock_was_held
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(
                "Index locked by another process — deferring in-memory upsert for {}",
                doc.path.display()
            );
            return;
        }

        let doc_id = doc.id();
        let _t_upsert = std::time::Instant::now();

        // Check body-match under a brief read lock — extract only the two
        // boolean facts we need; avoid cloning the full entry (which carries
        // large chunk_embeddings: Vec<Vec<f32>>).
        let (entry_exists, body_unchanged) = {
            let store_read = self.store.read();
            match store_read.get_entry(&doc_id) {
                Some(e) => (
                    true,
                    crate::vectordb::VectorStore::body_matches(doc, Some(e)),
                ),
                None => (false, false),
            }
        };

        match (entry_exists, body_unchanged) {
            (true, true) => {
                // Body unchanged: just patch metadata fields (cheap).
                let patch = crate::vectordb::VectorStore::prepare_metadata_patch(doc);
                if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
                    &self.wal_path(),
                    &crate::vectordb::WalRecord::MetadataOnly(patch.clone()),
                ) {
                    tracing::error!("Failed to append to WAL: {e}");
                }
                self.store
                    .write()
                    .apply_prepared(crate::vectordb::PreparedUpsert::MetadataOnly(patch));
                tracing::debug!(
                    target: "perf::vector",
                    phase = "store_upsert_inmem",
                    body_changed = false,
                    elapsed_ms = _t_upsert.elapsed().as_secs_f64() * 1000.0
                );
            }
            (true, false) => {
                // Body changed: patch metadata now, queue re-embed for later.
                let patch = crate::vectordb::VectorStore::prepare_metadata_patch(doc);
                if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
                    &self.wal_path(),
                    &crate::vectordb::WalRecord::MetadataOnly(patch.clone()),
                ) {
                    tracing::error!("Failed to append to WAL: {e}");
                }
                self.store
                    .write()
                    .apply_prepared(crate::vectordb::PreparedUpsert::MetadataOnly(patch));
                self.schedule_embed(doc.clone());
                tracing::debug!(
                    target: "perf::vector",
                    phase = "store_upsert_inmem",
                    body_changed = true,
                    deferred_embed = true,
                    elapsed_ms = _t_upsert.elapsed().as_secs_f64() * 1000.0
                );
            }
            _ => {
                // New document: create a skeleton/placeholder entry instantly with empty vectors
                let fm = doc.frontmatter.as_ref();
                let id = fm
                    .and_then(|f| f.get("id").and_then(|v| v.as_str()).map(String::from))
                    .unwrap_or_else(|| crate::pkb::fallback_id(&doc.path));
                let confidence = fm.and_then(|f| f.get("confidence").and_then(|v| v.as_f64()));
                let date =
                    fm.and_then(|f| f.get("date").and_then(|v| v.as_str()).map(String::from));
                let entry = crate::vectordb::DocumentEntry {
                    path: doc.path.clone(),
                    title: doc.title.clone(),
                    doc_type: doc.doc_type.clone(),
                    status: doc.status.clone(),
                    tags: doc.tags.clone(),
                    id,
                    date,
                    modified: doc.modified.clone(),
                    confidence,
                    content_hash: None, // Keep None so the background worker knows it needs to embed it!
                    file_hash: Some(doc.file_hash.clone()),
                    body_hash: None, // Keep None so background worker knows it needs to embed it!
                    chunk_embeddings: Vec::new(),
                    chunk_texts: Vec::new(),
                    body_chunks: Vec::new(),
                    consolidated: doc.consolidated,
                    consolidated_at: doc.consolidated_at.clone(),
                };
                if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
                    &self.wal_path(),
                    &crate::vectordb::WalRecord::Upsert(Box::new(entry.clone())),
                ) {
                    tracing::error!("Failed to append to WAL: {e}");
                }
                self.store
                    .write()
                    .apply_prepared(crate::vectordb::PreparedUpsert::Full(Box::new(entry)));
                self.schedule_embed(doc.clone());
                tracing::debug!(
                    target: "perf::vector",
                    phase = "store_upsert_inmem",
                    body_changed = true,
                    deferred_embed = true,
                    new_doc = true,
                    elapsed_ms = _t_upsert.elapsed().as_secs_f64() * 1000.0
                );
            }
        }

        let _t_save = std::time::Instant::now();
        self.save_store();
        tracing::debug!(target: "perf::vector", phase = "save_store_dispatch", elapsed_ms = _t_save.elapsed().as_secs_f64() * 1000.0);
    }

    /// Queue a document for background re-embedding. Coalesces by path
    /// (latest doc wins). Spawns a single worker (via spawn_blocking) if
    /// one isn't already running.
    pub(crate) fn schedule_embed(&self, doc: crate::pkb::PkbDocument) {
        let path_key = doc.path.to_string_lossy().to_string();
        self.embed_pending.lock().insert(path_key, doc);

        // Single-worker gate: only the caller that flips false→true spawns.
        if self.embed_worker_running.swap(true, Ordering::SeqCst) {
            return;
        }

        let pending = self.embed_pending.clone();
        let running = self.embed_worker_running.clone();
        let store = self.store.clone();
        let embedder = self.embedder.clone();
        let save_pending = self.save_pending.clone();
        let save_in_flight = self.save_in_flight.clone();
        let db_path = self.db_path.clone();

        let worker = move || {
            loop {
                // Pop one doc at a time — drain().next() silently drops all
                // other queued docs when the Drain iterator is dropped.
                let next_doc = {
                    let mut lock = pending.lock();
                    let key = lock.keys().next().cloned();
                    key.and_then(|k| lock.remove(&k))
                };

                if let Some(doc) = next_doc {
                    let doc_id = doc.id();
                    let _t = std::time::Instant::now();

                    // Snapshot existing under a brief read lock.
                    let existing = store.read().get_entry(&doc_id).cloned();
                    let prepared = match crate::vectordb::VectorStore::prepare_upsert(
                        &doc,
                        &embedder,
                        existing.as_ref(),
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!(
                                "background embed: prepare_upsert failed for {}: {e}",
                                doc.path.display()
                            );
                            continue;
                        }
                    };
                    let wal_p = crate::vectordb::wal_path(&db_path);
                    let wal_res = match &prepared {
                        crate::vectordb::PreparedUpsert::MetadataOnly(patch) => {
                            crate::vectordb::VectorStore::append_wal_record(
                                &wal_p,
                                &crate::vectordb::WalRecord::MetadataOnly(patch.clone()),
                            )
                        }
                        crate::vectordb::PreparedUpsert::Full(entry) => {
                            crate::vectordb::VectorStore::append_wal_record(
                                &wal_p,
                                &crate::vectordb::WalRecord::Upsert(entry.clone()),
                            )
                        }
                    };
                    if let Err(e) = wal_res {
                        tracing::error!("background embed: Failed to append to WAL: {e}");
                    }
                    store.write().apply_prepared(prepared);
                    tracing::debug!(
                        target: "perf::vector",
                        phase = "bg_embed",
                        elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0
                    );
                } else {
                    // Queue empty. Clear the running flag, then re-check in
                    // case a write landed between the last pop and flag-clear.
                    running.store(false, Ordering::SeqCst);
                    if pending.lock().is_empty() {
                        break;
                    }
                    // Items arrived; try to re-claim the worker slot and loop.
                    if running.swap(true, Ordering::SeqCst) {
                        break; // Another worker claimed it; let them handle it.
                    }
                    // We re-claimed; continue the loop to drain new items.
                }
            }

            // Schedule a save so the embedded entries hit disk. Coalesces
            // with the foreground save trigger.
            if !save_pending.swap(true, Ordering::SeqCst) {
                let store = store.clone();
                let pending = save_pending.clone();
                let in_flight = save_in_flight.clone();
                let db_path = db_path.clone();
                let do_save = move || {
                    in_flight.store(true, Ordering::SeqCst);
                    pending.store(false, Ordering::SeqCst);
                    match crate::vectordb::VectorStore::acquire_lock(&db_path) {
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
                    in_flight.store(false, Ordering::SeqCst);
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(_) => {
                        tokio::task::spawn_blocking(do_save);
                    }
                    Err(_) => do_save(),
                }
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                tokio::task::spawn_blocking(worker);
            }
            Err(_) => {
                // No runtime (e.g. CLI test path) — run inline.
                worker();
            }
        }
    }

    /// Re-embed a batch of files into the vector store and rebuild the graph
    /// once at the end. Used by batch ops so search doesn't return stale
    /// frontmatter (title/status/tags/…) after a multi-doc mutation.
    ///
    /// Concurrency: `prepare_upsert` runs in parallel via rayon **outside**
    /// the store write lock. A single brief write lock is then taken to
    /// install all prepared entries, followed by one coalesced `save_store`.
    /// `removed_paths` are dropped from the store under the same write lock.
    /// The graph is rebuilt once.
    ///
    /// If the cross-process index lock is held by a reindex, this skips the
    /// in-memory work entirely; the on-disk markdown files are already
    /// written and the reindex will pick them up. (PR4 will add a deferred
    /// queue so the live server self-heals after the reindex completes.)
    pub(crate) fn finalize_batch(
        &self,
        modified_paths: &[std::path::PathBuf],
        removed_paths: &[std::path::PathBuf],
    ) {
        if modified_paths.is_empty() && removed_paths.is_empty() {
            // Still rebuild graph in case callers rely on it post-mutation.
            self.rebuild_graph();
            return;
        }

        if !self.index_lock_available() {
            tracing::info!(
                "Index locked by another process — skipping in-memory finalize for batch ({} modified, {} removed)",
                modified_paths.len(),
                removed_paths.len()
            );
            // Graph still needs rebuilding from disk so in-memory state
            // reflects the structural changes.
            self.rebuild_graph();
            return;
        }

        let _t_finalize = std::time::Instant::now();

        let graph = self.graph.read();

        // Parse + prepare in parallel (no lock). Everything that parses is
        // indexed — there are no type/sync exclusions.
        let results: Vec<crate::vectordb::PreparedUpsert> = modified_paths
            .par_iter()
            .filter_map(|abs_path| {
                let doc = crate::pkb::parse_file_relative(abs_path, &self.pkb_root)?;
                let doc_id = doc.id();

                let existing = self.store.read().get_entry(&doc_id).cloned();
                match crate::vectordb::VectorStore::prepare_upsert(
                    &doc,
                    &self.embedder,
                    existing.as_ref(),
                ) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        tracing::warn!(
                            "finalize_batch: prepare_upsert failed for {}: {e}",
                            abs_path.display()
                        );
                        None
                    }
                }
            })
            .collect();

        // Single brief write lock for the whole batch.
        {
            let mut store = self.store.write();
            let wal_p = self.wal_path();
            for p in &results {
                let wal_res = match p {
                    crate::vectordb::PreparedUpsert::MetadataOnly(patch) => {
                        crate::vectordb::VectorStore::append_wal_record(
                            &wal_p,
                            &crate::vectordb::WalRecord::MetadataOnly(patch.clone()),
                        )
                    }
                    crate::vectordb::PreparedUpsert::Full(entry) => {
                        crate::vectordb::VectorStore::append_wal_record(
                            &wal_p,
                            &crate::vectordb::WalRecord::Upsert(entry.clone()),
                        )
                    }
                };
                if let Err(e) = wal_res {
                    tracing::error!("finalize_batch: Failed to append to WAL: {e}");
                }
                store.apply_prepared(p.clone());
            }
            for abs_path in removed_paths {
                let rel = abs_path.strip_prefix(&self.pkb_root).unwrap_or(abs_path);
                let node_id = graph.nodes().find(|n| n.path == rel).map(|n| n.id.clone());
                let id = node_id.unwrap_or_else(|| crate::pkb::fallback_id(rel));
                if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
                    &wal_p,
                    &crate::vectordb::WalRecord::Remove(id.clone()),
                ) {
                    tracing::error!("finalize_batch: Failed to append remove to WAL: {e}");
                }
                store.remove(&id);
            }
        }

        // Release the graph read guard before rebuild_graph() takes the
        // write lock — holding both self-deadlocks the calling thread.
        drop(graph);

        tracing::debug!(
            target: "perf::batch_finalize",
            n_modified = modified_paths.len(),
            n_removed = removed_paths.len(),
            elapsed_ms = _t_finalize.elapsed().as_secs_f64() * 1000.0,
            "finalize_batch complete"
        );

        // One coalesced save and one graph rebuild for the whole batch.
        self.save_store();
        self.rebuild_graph();
    }

    /// Remove a document from the vector store if the index is not locked.
    pub(crate) fn try_remove_document(&self, id: &str) {
        self.maybe_drain_deferred();

        if !self.index_lock_available() {
            // Defer the remove. Resolve to absolute so the drain step's
            // existence check works correctly (the file will be absent on
            // disk, so the drain will pick the remove path).
            let rel_path = {
                let graph = self.graph.read();
                graph
                    .resolve(id)
                    .map(|n| n.path.clone())
                    .unwrap_or_else(|| PathBuf::from(id))
            };
            let abs = self.pkb_root.join(rel_path);
            self.deferred_paths.lock().insert(abs);
            self.lock_was_held
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("Index locked by another process — deferring in-memory remove for {id}");
            return;
        }
        if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
            &self.wal_path(),
            &crate::vectordb::WalRecord::Remove(id.to_string()),
        ) {
            tracing::error!("try_remove_document: Failed to append remove to WAL: {e}");
        }
        self.store.write().remove(id);
        self.save_store();
    }

    // =========================================================================
    // SEARCH & DOCUMENT TOOLS
    // =========================================================================

    pub fn dispatch_tool_sync(&self, name: &str, args: &JsonValue) -> Result<CallToolResult, McpError> {
        match name {
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
            "complete_task" => self.handle_complete_task(args),
            "release_task" => self.handle_release_task(args),
            "list_tasks" => self.handle_list_tasks(args),
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
            "graph_json" => self.handle_graph_json(args),
            "graph_excalidraw" => self.handle_graph_excalidraw(args),
            "export_graph" => self.handle_export_graph(args),
            "diff_excalidraw" => self.handle_diff_excalidraw(args),
            "sync_excalidraw" => self.handle_sync_excalidraw(args),
            "task_summary" => self.handle_task_summary(args),
            "find_duplicates" => self.handle_find_duplicates(args),
            "batch_merge" => self.handle_batch_merge(args),
            "merge_node" => self.handle_merge_node(args),
            "batch_create_epics" => self.handle_batch_create_epics(args),
            "batch_reclassify" => self.handle_batch_reclassify(args),
            "get_stats" => self.handle_get_stats(args),
            "status" => self.handle_status(args),
            "refresh_graph" => self.handle_refresh_graph(args),
            "get_consolidation_cluster" => self.handle_get_consolidation_cluster(args),
            "apply_consolidation_batch" => self.handle_apply_consolidation_batch(args),
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown tool: {name}")),
                data: None,
            }),
        }
    }
}


impl ServerHandler for PkbSearchServer {
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let start = std::time::Instant::now();
        let tool_name = request.name.clone();
        let args = Self::args_to_value(request.arguments);

        let effective_name = tool_name.to_string();

        let this = self.clone();
        async move {
            let (tx, rx) = tokio::sync::oneshot::channel();

            // Offload to a blocking thread so the async task can respond to
            // session cancellation (e.g. keep-alive timeout or disconnect).
            // Refs task-2ae61ce6.
            let tool_name_task = tool_name.clone();
            let args_task = args.clone();
            let this_task = this.clone();

            tokio::task::spawn_blocking(move || {
                let res = this_task.dispatch_tool_sync(&tool_name_task, &args_task);
                let _ = tx.send(res);
            });

            let result = tokio::select! {
                res = rx => res.unwrap_or_else(|_| Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from("Internal error in tool execution task"),
                    data: None,
                })),
                _ = context.ct.cancelled() => {
                    tracing::warn!("Session terminated while tool '{}' was in flight", tool_name);
                    Err(McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from("Session terminated"),
                        data: None,
                    })
                }
            };

            let latency = start.elapsed().as_millis();
            let is_error = result.is_err();
            let response_bytes = match &result {
                Ok(res) => serde_json::to_vec(res).map(|v| v.len()).unwrap_or(0),
                Err(e) => serde_json::to_vec(e).map(|v| v.len()).unwrap_or(0),
            };

            crate::telemetry::record_call(&effective_name, response_bytes, latency, is_error);

            result
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = Self::get_all_tools();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
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
        let mut instructions = String::from(
            "PKB Search — semantic search + task graph over personal knowledge base. \
             A suite of tools for search, documents, tasks, and knowledge graph. \
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



impl FactsProvider for PkbSearchServer {}

