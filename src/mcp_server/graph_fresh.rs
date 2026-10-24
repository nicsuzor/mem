use std::collections::HashSet;
use rayon::prelude::*;

use crate::graph_store::GraphStore;
use super::status::ReindexStatus;
use super::PkbSearchServer;

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct RebuildStats {
    pub scanned_files: usize,
    pub parsed_documents: usize,
    pub loaded_nodes: usize,
}

impl PkbSearchServer {
    /// Number of full graph rebuilds completed by this server instance.
    pub fn rebuild_count(&self) -> usize {
        self.rebuild_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of Tier-2 graph rebuild executions that have completed
    /// (incremented once per non-coalesced worker iteration). Used by tests
    /// and the bench harness to verify coalescing behaviour.
    pub fn tier2_execution_count(&self) -> u64 {
        self.tier2_executions
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// True when a Tier-2 background graph rebuild is either in flight or
    /// coalesced (dirty flag set). Callers can poll this to wait for a
    /// sequence of fast-path updates to fully settle.
    pub fn graph_rebuild_pending(&self) -> bool {
        self.graph_rebuild_pending
            .load(std::sync::atomic::Ordering::SeqCst)
            || self
                .graph_rebuild_dirty
                .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test helper: inject a sleep into Tier-2 rebuilds to widen race windows.
    #[cfg(test)]
    pub fn set_tier2_sleep_ms(&self, ms: u64) {
        self.tier2_sleep_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Test helper: inject a delay into Tier-1 full rebuild and Tier-2 swap,
    /// placed immediately before bumping `full_rebuild_epoch`. On correct
    /// code this delay executes *inside* the write lock. Used by TOCTOU
    /// regression tests to make atomicity windows deterministically observable.
    #[cfg(test)]
    pub fn set_pre_epoch_bump_delay_ms(&self, ms: u64) {
        self.pre_epoch_bump_delay_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Ensure the in-memory graph index is up-to-date with on-disk state.
    ///
    /// Validates the cached generation stamp against a fast stat-only scan of the
    /// filesystem. If an external write occurred (git-sync sidecar, CLI, direct disk edit),
    /// the graph is automatically rebuilt from disk before the read proceeds.
    pub(crate) fn ensure_graph_fresh(&self) {
        let tracer = opentelemetry::global::tracer("mem");
        use opentelemetry::trace::Tracer;
        let span = tracer.start("ensure_graph_fresh");
        let _guard = opentelemetry::trace::mark_span_as_active(span);

        let current_gen = crate::pkb::scan_generation(&self.pkb_root);
        let cached_gen = self.graph.read().generation();
        if current_gen != cached_gen {
            let _lock = self.rebuild_mutex.lock();
            let cached_gen = self.graph.read().generation();
            if current_gen != cached_gen {
                tracing::info!(
                    target: "pkb::invalidation",
                    disk_count = current_gen.file_count,
                    cached_count = cached_gen.file_count,
                    "In-memory graph index out of date with disk; invalidating and rebuilding"
                );
                self.rebuild_graph();
                crate::otel::call_metrics::CallMetrics::record_rebuild(true);
            }
        }
    }

    /// Reconstruct an absolute path from a (possibly relative) graph node path.
    /// Full rebuild of the graph store from disk (for batch operations).
    ///
    /// Returns (scanned_files_count, parsed_documents_count, loaded_nodes_count).
    ///
    /// Concurrency: takes the store read lock just long enough to snapshot
    /// the per-document averaged embeddings, then drops it. The (slow)
    /// scan + parse + similarity-edge build runs against the snapshot with
    /// no store lock held — concurrent writers can proceed during the
    /// rebuild instead of blocking behind it.
    pub(crate) fn rebuild_graph(&self) -> (usize, usize, usize) {
        let tracer = opentelemetry::global::tracer("mem");
        use opentelemetry::trace::Tracer;
        let span = tracer.start("rebuild_graph");
        let _guard = opentelemetry::trace::mark_span_as_active(span);

        let _t_snap = std::time::Instant::now();
        let snapshot = self.acquire_store_read().averaged_embeddings();
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
        let scanned = files.len();
        let docs: Vec<crate::pkb::PkbDocument> = files
            .par_iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, &self.pkb_root))
            .collect();
        let parsed = docs.len();

        let mut new_graph = GraphStore::build_with_embeddings(&docs, &self.pkb_root, &snapshot);
        let loaded = {
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
            let count = new_graph.node_count();
            *g = new_graph;
            #[cfg(test)]
            {
                let ms = self
                    .pre_epoch_bump_delay_ms
                    .load(std::sync::atomic::Ordering::Relaxed);
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
            // Bump the full-rebuild epoch atomically with the swap, still
            // under the write lock, so any Tier-2 rebuild currently in
            // flight (computed from a `nodes_cloned()` snapshot predating
            // this disk-scanning rebuild) is guaranteed to observe the new
            // epoch at its own swap-time check and discard its stale
            // result instead of reverting what this rebuild just picked up
            // from disk. Bumping after the lock is released leaves a
            // window where a Tier-2 worker can pass its epoch check before
            // the bump lands and then clobber this fresher graph.
            self.full_rebuild_epoch
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.rebuild_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            count
        };
        *self.last_reindex.write() = ReindexStatus {
            timestamp: chrono::Utc::now().to_rfc3339(),
            outcome: "ok".to_string(),
        };
        *self.last_rebuild_stats.write() = RebuildStats {
            scanned_files: scanned,
            parsed_documents: parsed,
            loaded_nodes: loaded,
        };
        (scanned, parsed, loaded)
    }

    /// Synchronous fast path after a single file changed.
    ///
    /// Patches the node in place under the graph write lock (microseconds),
    /// runs cheap classification so `list_tasks(status="ready")` membership
    /// reflects the new status, and schedules a coalesced background
    /// rebuild that refreshes derived metrics (flow figures, centrality,
    /// target_ancestors, similarity edges).
    ///
    /// Consistency: after this returns, `resolve(id)` finds the node, its
    /// `status`/`parent`/`priority` are fresh, and ready/blocked membership
    /// is fresh. Derived metric *values* (flow figures, centrality) lag by
    /// 1–3s until the background rebuild lands; structural inverses and
    /// centrality are carried over from the existing node so they don't
    /// reset to defaults.
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
            g.remove_node_in_place(id, &self.pkb_root);
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
        let full_rebuild_epoch = self.full_rebuild_epoch.clone();
        // `dirty` itself is still needed by the `worker` loop below (to
        // decide whether to run another iteration); clone a handle for the
        // stale-epoch abort path inside `do_rebuild_once` instead of moving
        // the original in.
        let dirty_for_abort = dirty.clone();
        #[cfg(test)]
        let sleep_ms = self.tier2_sleep_ms.clone();
        #[cfg(test)]
        let pre_bump_delay_ms = self.pre_epoch_bump_delay_ms.clone();

        let do_rebuild_once = move || {
            let _t_total = std::time::Instant::now();

            // Drain the patch tracker. Patches arriving after this point
            // are recorded for the *next* iteration (the dirty flag will
            // be set by their schedule_graph_rebuild call).
            patched.lock().clear();

            // Snapshot the full-rebuild epoch alongside the node state we're
            // about to compute from. If a disk-scanning `rebuild_graph()`
            // (explicit `refresh_graph`, `ensure_graph_fresh` auto-heal, ...)
            // swaps in a fresher graph while this (slower) Tier-2 rebuild is
            // still computing, `nodes` below is a stale snapshot relative to
            // it — checked again at swap time.
            let start_epoch = full_rebuild_epoch.load(Ordering::SeqCst);

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

            // A full rebuild (explicit `refresh_graph`, `ensure_graph_fresh`
            // auto-heal, batch finalize, ...) landed while we were computing
            // from the now-stale `nodes` snapshot above. Swapping `merged`
            // in now would silently revert whatever that fresher rebuild
            // picked up from disk (aops_mem_tag_integrity). Discard this
            // iteration's result and re-mark dirty so the worker loop runs
            // another iteration off the now-current graph instead.
            if full_rebuild_epoch.load(Ordering::SeqCst) != start_epoch {
                drop(g);
                dirty_for_abort.store(true, Ordering::SeqCst);
                tracing::debug!(
                    target: "perf::graph_rebuild",
                    phase = "tier2_aborted_stale_epoch",
                    elapsed_ms = _t_total.elapsed().as_secs_f64() * 1000.0
                );
                return;
            }

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
            // Preserve the live generation stamp (which may have been updated
            // by in-place node removals or concurrent mutations during rebuild)
            // so the swap does not clobber it with the older scan from build start.
            merged.set_generation(g.generation());
            *g = merged;
            #[cfg(test)]
            {
                let ms = pre_bump_delay_ms.load(Ordering::Relaxed);
                if ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
            // Bump atomically with the swap, still under the write lock —
            // see the matching comment in `rebuild_graph` for why bumping
            // after `drop(g)` reopens the TOCTOU window this epoch exists
            // to close.
            full_rebuild_epoch.fetch_add(1, Ordering::SeqCst);
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
}
