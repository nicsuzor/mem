use super::*;
#[cfg(test)]
mod cross_process_recovery_tests {
    use super::*;
    use crate::embeddings::{Embedder, EMBEDDING_DIM};
    use crate::graph_store::GraphStore;
    use crate::vectordb::VectorStore;
    use std::path::Path;
    use std::sync::Arc;

    fn build_disk_server(pkb_root: &Path) -> PkbSearchServer {
        std::fs::create_dir_all(pkb_root.join("projects")).unwrap();
        std::fs::write(
            pkb_root.join("polecat.yaml"),
            "projects:\n  p: {}\n  aops: {}\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("projects/p.md"),
            "---\nid: p\ntitle: P\ntype: project\nstatus: active\n---\n\n# P\n",
        )
        .unwrap();

        let store = VectorStore::new(EMBEDDING_DIM);
        let embedder = Embedder::new_dummy();
        let graph = GraphStore::build_from_directory(pkb_root);
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            pkb_root.to_path_buf(),
            pkb_root.join("test-index.bin"),
            Arc::new(RwLock::new(graph)),
        )
    }

    /// When the cross-process index lock is held by another process during
    /// a write attempt, the path must be queued. After the lock is released,
    /// the next write must drain the queue and apply the deferred upsert.
    #[test]
    fn test_deferred_upserts_drain_after_lock_release() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let server = build_disk_server(pkb_root);

        // Create a task — this should succeed normally and seed the store.
        let result = server
            .handle_create_task(&serde_json::json!({
                "title": "First Task",
                "parent": "p",
                "project": "p",
            }))
            .expect("create_task");
        let _ = result;

        let initial_count = server.store.read().len();
        assert!(
            initial_count >= 1,
            "store should have entries after first create"
        );

        // Simulate `pkb reindex` running in another process: hold the
        // advisory file lock exclusively from a background thread so the
        // server's `index_lock_available()` returns false during the next
        // create_task call. We use a channel to coordinate: the server gets
        // its create call in while the lock is held, then we release.
        let (start_tx, start_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let db_path = server.db_path.clone();

        let lock_holder = std::thread::spawn(move || {
            let mut lock = VectorStore::acquire_lock(&db_path).expect("acquire lock");
            let _guard = lock.write().expect("hold write lock");
            start_tx.send(()).unwrap();
            // Hold until told to release.
            release_rx.recv().unwrap();
            // _guard drops here — lock released.
        });

        start_rx.recv().unwrap();

        // The lock is now held by the background thread. A new create_task
        // writes the file successfully but the in-memory upsert must defer.
        server
            .handle_create_task(&serde_json::json!({
                "title": "Deferred Task",
                "parent": "p",
                "project": "p",
            }))
            .expect("create_task during lock");

        // The deferred queue should contain at least one entry — the new
        // task's path. The store length should NOT have grown (the in-mem
        // upsert was skipped).
        assert!(
            !server.deferred_paths.lock().is_empty(),
            "deferred queue should be non-empty while cross-process lock is held"
        );
        let count_during_lock = server.store.read().len();
        assert_eq!(
            count_during_lock, initial_count,
            "store should not grow while cross-process lock is held"
        );

        // Release the cross-process lock.
        release_tx.send(()).unwrap();
        lock_holder.join().unwrap();

        // Drive the drain by calling the helper directly. (In production,
        // this is called from the next try_upsert/try_remove invocation.)
        server.maybe_drain_deferred();

        // Queue is drained, store reflects the deferred upsert.
        assert!(
            server.deferred_paths.lock().is_empty(),
            "deferred queue should be empty after drain"
        );
        assert!(
            server.store.read().len() > count_during_lock,
            "store should grow after drain processed the deferred upsert"
        );
    }
}

// ===========================================================================
// mem-e394a6d0 — semantic-search status projection (AC2)
// ===========================================================================


