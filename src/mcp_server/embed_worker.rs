use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use rayon::prelude::*;

use crate::vectordb::VectorStore;
use super::PkbSearchServer;

impl PkbSearchServer {
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
                            tracing::debug!(
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
            let dim = self.acquire_store_read().dimension_or_default();
            match VectorStore::load_or_create(&self.db_path, dim) {
                Ok(fresh) => {
                    tracing::info!(
                        "Cross-process index lock released — reloaded vector store from disk ({} docs)",
                        fresh.len()
                    );
                    *self.acquire_store_write() = fresh;
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
                if self.acquire_store_write().remove(&id) {
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
            tracing::debug!(
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
                                tracing::debug!(
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
            tracing::debug!(
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

                let existing = self.acquire_store_read().get_entry(&doc_id).cloned();
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
            let mut store = self.acquire_store_write();
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
    pub(crate) fn try_remove_document(&self, id: &str, abs_path: Option<&Path>) {
        self.maybe_drain_deferred();

        if !self.index_lock_available() {
            // Defer the remove. Resolve to absolute so the drain step's
            // existence check works correctly (the file will be absent on
            // disk, so the drain will pick the remove path).
            let abs = if let Some(p) = abs_path {
                p.to_path_buf()
            } else {
                let rel_path = {
                    let graph = self.graph.read();
                    graph
                        .resolve(id)
                        .map(|n| n.path.clone())
                        .unwrap_or_else(|| PathBuf::from(id))
                };
                self.pkb_root.join(rel_path)
            };
            self.deferred_paths.lock().insert(abs);
            self.lock_was_held
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!(
                "Index locked by another process — deferring in-memory remove for {id}"
            );
            return;
        }
        if let Err(e) = crate::vectordb::VectorStore::append_wal_record(
            &self.wal_path(),
            &crate::vectordb::WalRecord::Remove(id.to_string()),
        ) {
            tracing::error!("try_remove_document: Failed to append remove to WAL: {e}");
        }
        self.acquire_store_write().remove(id);
        self.save_store();
    }
}
