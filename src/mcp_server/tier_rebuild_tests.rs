use super::*;
#[cfg(test)]
mod tier_rebuild_tests {
    use super::*;
    use crate::embeddings::Embedder;
    use crate::graph_store::GraphStore;
    use crate::pkb::PkbDocument;
    use crate::vectordb::VectorStore;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_task_doc(id: &str, status: &str, priority: i32, deps: &[&str]) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("id".to_string(), json!(id));
        fm.insert("title".to_string(), json!(id));
        fm.insert("type".to_string(), json!("task"));
        fm.insert("status".to_string(), json!(status));
        fm.insert("priority".to_string(), json!(priority));
        if !deps.is_empty() {
            fm.insert("depends_on".to_string(), json!(deps));
        }
        PkbDocument {
            path: PathBuf::from(format!("tasks/{}.md", id)),
            title: id.to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some(status.to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: format!("hash-{}", id),
            file_hash: format!("hash-{}", id),
        }
    }

    fn build_server(docs: Vec<PkbDocument>) -> PkbSearchServer {
        let pkb_root = PathBuf::from("/tmp/test-tier-rebuild");
        let graph = GraphStore::build(&docs, &pkb_root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            pkb_root.clone(),
            pkb_root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        )
    }

    #[test]
    fn test_list_tasks_filters() {
        let docs = vec![
            make_task_doc("task-1", "active", 1, &[]),
            PkbDocument {
                path: PathBuf::from("tasks/target-1.md"),
                title: "LLB242 ARC Pilot".to_string(),
                body: String::new(),
                doc_type: Some("target".to_string()),
                status: Some("active".to_string()),
                consolidated: None,
                consolidated_at: None,
                modified: None,
                tags: vec![],
                frontmatter: Some(json!({
                    "id": "target-1",
                    "type": "target",
                    "title": "LLB242 ARC Pilot",
                    "status": "active"
                })),
                content_hash: "hash-target-1".to_string(),
                file_hash: "hash-target-1".to_string(),
            },
        ];
        let server = build_server(docs);

        fn task_ids_from(result: &CallToolResult) -> Vec<String> {
            let text = result
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect::<String>();
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("tasks").and_then(|t| t.as_array()).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        }

        // Test type filter
        let ids = task_ids_from(
            &server
                .handle_list_tasks(&json!({"type": "target", "format": "json"}))
                .unwrap(),
        );
        assert!(ids.contains(&"target-1".to_string()));
        assert!(!ids.contains(&"task-1".to_string()));

        // Test title_contains filter
        let ids = task_ids_from(
            &server
                .handle_list_tasks(
                    &json!({"title_contains": "LLB242", "type": "target", "format": "json"}),
                )
                .unwrap(),
        );
        assert!(ids.contains(&"target-1".to_string()));
        assert!(!ids.contains(&"task-1".to_string()));

        // Test complexity filter
        let docs_with_complexity = vec![PkbDocument {
            path: PathBuf::from("tasks/complex-1.md"),
            title: "Complex Task".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(json!({
                "id": "complex-1",
                "type": "task",
                "title": "Complex Task",
                "status": "active",
                "complexity": "high"
            })),
            content_hash: "hash-complex-1".to_string(),
            file_hash: "hash-complex-1".to_string(),
        }];
        let server_complex = build_server(docs_with_complexity);

        let ids = task_ids_from(
            &server_complex
                .handle_list_tasks(&json!({"complexity": "high", "format": "json"}))
                .unwrap(),
        );
        assert!(ids.contains(&"complex-1".to_string()));

        let ids = task_ids_from(
            &server_complex
                .handle_list_tasks(&json!({"complexity": "low", "format": "json"}))
                .unwrap(),
        );
        assert!(ids.is_empty());
    }

    /// Tier-1 must produce fresh ready-list classification (membership) on
    /// the request thread — no waiting for the background rebuild.
    #[test]
    fn tier1_classification_membership_is_fresh() {
        let docs = vec![
            make_task_doc("task-x", "ready", 1, &[]),
            make_task_doc("task-y", "ready", 2, &[]),
        ];
        let server = build_server(docs);

        // Both tasks initially ready.
        let ready_initial: Vec<String> = server.graph.read().ready_ids().to_vec();
        assert!(ready_initial.contains(&"task-x".to_string()));
        assert!(ready_initial.contains(&"task-y".to_string()));

        // Tier-1 patch: take task-x out of contention by switching status to "blocked".
        let updated = make_task_doc("task-x", "blocked", 1, &[]);
        server.rebuild_graph_for_pkb_document(&updated);

        let ready_after: Vec<String> = server.graph.read().ready_ids().to_vec();
        assert!(
            !ready_after.contains(&"task-x".to_string()),
            "task-x should be excluded from ready list after Tier-1 patch (got {:?})",
            ready_after
        );
        assert!(
            ready_after.contains(&"task-y".to_string()),
            "task-y should still be ready"
        );
    }

    /// Tier-1 must not recompute similarity edges. With include_similarity=false
    /// the edge count for `SimilarTo` cannot grow as a result of a Tier-1 patch.
    #[test]
    fn tier1_does_not_recompute_similarity_edges() {
        let docs = vec![
            make_task_doc("task-x", "ready", 1, &[]),
            make_task_doc("task-y", "ready", 2, &[]),
        ];
        let server = build_server(docs);
        let sim_before = server.graph.read().similarity_edges_cloned().len();

        let updated = make_task_doc("task-x", "blocked", 1, &[]);
        server.rebuild_graph_for_pkb_document(&updated);

        let sim_after = server.graph.read().similarity_edges_cloned().len();
        assert_eq!(
            sim_before, sim_after,
            "Tier-1 must not change similarity-edge count; before={} after={}",
            sim_before, sim_after
        );
    }

    /// Tier-2 coalesces bursts: many `schedule_graph_rebuild` calls in flight
    /// should produce far fewer actual executions than calls.
    #[tokio::test(flavor = "multi_thread")]
    async fn tier2_coalesces_burst_writes() {
        let docs: Vec<PkbDocument> = (0..20)
            .map(|i| make_task_doc(&format!("task-{i}"), "active", 1, &[]))
            .collect();
        let server = build_server(docs);

        // Slow Tier-2 enough that subsequent calls overlap.
        server.set_tier2_sleep_ms(50);

        let n = 100;
        for _ in 0..n {
            server.schedule_graph_rebuild();
        }

        // Wait until at least one rebuild has executed AND the queue is
        // stable (no further executions pending or in flight). `pending`
        // clears at rebuild start, not end — so additionally poll until
        // the execution counter has stopped advancing.
        let mut last = u64::MAX;
        let mut stable_ticks = 0;
        for _ in 0..200 {
            tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
            let now = server.tier2_execution_count();
            if !server.graph_rebuild_pending() && now == last && now >= 1 {
                stable_ticks += 1;
                if stable_ticks >= 2 {
                    break;
                }
            } else {
                stable_ticks = 0;
            }
            last = now;
        }

        let executed = server.tier2_execution_count();
        assert!(executed >= 1, "at least one Tier-2 must have run; got 0");
        assert!(
            executed < (n as u64) / 4,
            "expected far fewer Tier-2 executions than calls due to coalescing; \
             called {n}, executed {executed}",
        );
    }

    /// Lost-patch race: a Tier-1 patch arriving during an in-flight Tier-2
    /// rebuild must survive the swap. Without the patch-merge step, this test
    /// fails (patched node reverts to its pre-patch state).
    #[tokio::test(flavor = "multi_thread")]
    async fn tier2_swap_does_not_revert_concurrent_tier1_patch() {
        let docs = vec![
            make_task_doc("task-x", "ready", 1, &[]),
            make_task_doc("task-y", "ready", 2, &[]),
        ];
        let server = build_server(docs);
        // Make Tier-2 slow enough to interleave with a Tier-1 patch.
        server.set_tier2_sleep_ms(200);

        // Kick off Tier-2 first (no Tier-1 yet, so patched_during_rebuild
        // starts empty when this rebuild runs).
        server.schedule_graph_rebuild();

        // While Tier-2 sleeps, fire Tier-1 patch that flips task-x to blocked.
        // The patch lands AFTER Tier-2 cleared `pending` and `patched_during_rebuild`,
        // so it gets recorded into patched_during_rebuild and the swap must
        // re-apply it.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let updated = make_task_doc("task-x", "blocked", 1, &[]);
        server.rebuild_graph_for_pkb_document(&updated);

        // Wait for the in-flight Tier-2 to swap, then for the follow-up
        // (queued by the Tier-1 schedule call) to also drain.
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if !server.graph_rebuild_pending() {
                break;
            }
        }

        let g = server.graph.read();
        let node = g
            .get_node("task-x")
            .expect("task-x should still exist after swap");
        assert_eq!(
            node.status.as_deref(),
            Some("blocked"),
            "Tier-1 patch (status=blocked) must survive Tier-2 swap"
        );
        let ready = g.ready_ids();
        assert!(
            !ready.contains(&"task-x".to_string()),
            "task-x must not be in ready list after status flipped to blocked"
        );
    }

    /// aops_mem_tag_integrity: a slow Tier-2 background rebuild whose
    /// `nodes_cloned()` snapshot predates a tag removal must not clobber the
    /// fresher state an explicit `refresh_graph` (full disk rescan) swaps in
    /// while Tier-2 is still computing. Without the `full_rebuild_epoch`
    /// guard, Tier-2's swap — built from the stale pre-removal snapshot —
    /// lands after `refresh_graph`'s swap and silently reintroduces the
    /// removed tag, matching the "two refresh_graph calls did not clear it"
    /// symptom on this task.
    #[tokio::test(flavor = "multi_thread")]
    async fn tier2_stale_swap_does_not_revert_refresh_graph_tag_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let file_path = root.join("tasks/task-tagged.md");
        std::fs::write(
            &file_path,
            "---\nid: task-tagged\ntitle: Tagged\ntype: task\nstatus: ready\ntags:\n  - onlytag\n---\n\nBody.\n",
        )
        .unwrap();

        let doc = crate::pkb::parse_file(&file_path).unwrap();
        let graph = GraphStore::build(&[doc], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = Arc::new(PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db"),
            Arc::new(RwLock::new(graph)),
        ));
        server.set_tier2_sleep_ms(300);

        // Kick off a Tier-2 rebuild. It snapshots `nodes_cloned()` NOW — with
        // "onlytag" present — then sleeps for 300ms before its swap.
        server.schedule_graph_rebuild();

        // While Tier-2 sleeps, remove the tag on disk and force a full
        // rebuild via the explicit refresh_graph path — disk truth the
        // in-flight Tier-2 snapshot has no way to know about.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        std::fs::write(
            &file_path,
            "---\nid: task-tagged\ntitle: Tagged\ntype: task\nstatus: ready\ntags: []\n---\n\nBody.\n",
        )
        .unwrap();
        server.handle_refresh_graph(&json!({})).unwrap();

        // refresh_graph's own swap must reflect the removal immediately.
        {
            let g = server.graph.read();
            let node = g.get_node("task-tagged").expect("node must still exist");
            assert!(
                !node.tags.iter().any(|t| t == "onlytag"),
                "refresh_graph's own swap must drop the removed tag immediately"
            );
        }

        // Wait for the in-flight (and any coalesced follow-up) Tier-2
        // rebuild to fully drain.
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if !server.graph_rebuild_pending() {
                break;
            }
        }

        let g = server.graph.read();
        let node = g
            .get_node("task-tagged")
            .expect("task-tagged should still exist after Tier-2 settles");
        assert!(
            !node.tags.iter().any(|t| t == "onlytag"),
            "a stale Tier-2 rebuild snapshotted before the tag removal must \
             not revert refresh_graph's fresher disk-truth state once it \
             swaps in"
        );
    }

    /// Concurrent Tier-1 rebuilds must not lose nodes. Two threads each
    /// inserting a new node simultaneously used to race on the
    /// `nodes_cloned()` snapshot: the later-finishing thread's snapshot
    /// pre-dated the earlier thread's swap, so its swap overwrote the new
    /// node. This test fires N concurrent inserts and asserts that *every*
    /// inserted node is present in the final graph.
    #[test]
    fn concurrent_tier1_rebuilds_do_not_lose_nodes() {
        use std::thread;

        let initial = vec![make_task_doc("task-seed", "active", 1, &[])];
        let server = Arc::new(build_server(initial));

        let n_inserts = 32;
        let handles: Vec<_> = (0..n_inserts)
            .map(|i| {
                let server = Arc::clone(&server);
                thread::spawn(move || {
                    let doc = make_task_doc(&format!("task-{i}"), "active", 1, &[]);
                    server.rebuild_graph_for_pkb_document(&doc);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let g = server.graph.read();
        let missing: Vec<String> = (0..n_inserts)
            .map(|i| format!("task-{i}"))
            .filter(|id| g.get_node(id).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "{} of {n_inserts} concurrent Tier-1 inserts were lost: {missing:?}",
            missing.len()
        );
        // Seed must still exist too.
        assert!(g.get_node("task-seed").is_some(), "seed node was lost");
    }

    /// Read-after-write semantics: a status change must be visible immediately
    /// in `list_tasks(status=ready)` membership.
    #[test]
    fn read_after_write_status_visible_immediately() {
        let docs = vec![
            make_task_doc("task-x", "ready", 1, &[]),
            make_task_doc("task-y", "ready", 2, &[]),
        ];
        let server = build_server(docs);

        let updated = make_task_doc("task-x", "blocked", 1, &[]);
        server.rebuild_graph_for_pkb_document(&updated);

        // Direct node read.
        let g = server.graph.read();
        assert_eq!(
            g.get_node("task-x").and_then(|n| n.status.clone()),
            Some("blocked".to_string()),
        );

        // Classification list.
        let ready = g.ready_ids();
        assert!(!ready.contains(&"task-x".to_string()));
        assert!(ready.contains(&"task-y".to_string()));
    }

    /// Fast path patches arriving during a full rebuild_graph build phase must survive the swap.
    #[test]
    fn rebuild_graph_full_swap_does_not_revert_concurrent_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let file_path = root.join("tasks/task-x.md");
        std::fs::write(
            &file_path,
            "---\nid: task-x\ntitle: Task X\ntype: task\nstatus: ready\n---\n\n# X\n",
        )
        .unwrap();

        let doc = crate::pkb::parse_file(&file_path).unwrap();
        let graph = GraphStore::build(&[doc], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = Arc::new(PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db"),
            Arc::new(RwLock::new(graph)),
        ));
        server.set_tier2_sleep_ms(500);

        // 1. Simulate rebuild_graph beginning: snapshot embeddings and scan/build graph from disk (which has status: ready)
        let snapshot = server.store.read().averaged_embeddings();
        server.patched_during_rebuild.lock().clear();

        let files = crate::pkb::scan_directory(&server.pkb_root);
        let docs: Vec<crate::pkb::PkbDocument> = files
            .iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, &server.pkb_root))
            .collect();
        let mut new_graph = GraphStore::build_with_embeddings(&docs, &server.pkb_root, &snapshot);

        // 2. While build is happening, a fast-path patch arrives in memory and marks patched_during_rebuild
        let updated_doc = make_task_doc("task-x", "done", 1, &[]);
        {
            let node = crate::graph::GraphNode::from_pkb_document(&updated_doc);
            let mut g = server.graph.write();
            g.upsert_node_in_place(node, &server.pkb_root);
            g.reclassify();
            server
                .patched_during_rebuild
                .lock()
                .insert("task-x".to_string());
        }

        // 3. Swap phase (identical to rebuild_graph implementation) merges patched nodes:
        {
            let mut g = server.graph.write();
            let patched_ids: Vec<String> = server
                .patched_during_rebuild
                .lock()
                .iter()
                .cloned()
                .collect();
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

        let g = server.graph.read();
        let node = g.get_node("task-x").expect("task-x exists");
        assert_eq!(
            node.status.as_deref(),
            Some("done"),
            "fast path patch must survive Tier-1 full rebuild swap"
        );
    }

    /// task_af93030b: the `full_rebuild_epoch` bump must be atomic with the
    /// graph swap it protects — i.e. it must happen under the same write-lock
    /// critical section, not after the lock is released. If the bump happens
    /// after `drop(g)`, a window opens where a concurrent Tier-2 rebuild can
    /// acquire the write lock, observe the *pre-bump* (stale) epoch value,
    /// and swap in its own stale snapshot — silently reverting a fresher
    /// Tier-1 disk-scan rebuild that just landed.
    ///
    /// This test makes that window deterministically observable (instead of
    /// relying on a nanosecond-scale race) via `pre_epoch_bump_delay_ms`,
    /// which injects a sleep immediately before the epoch bump in both
    /// `rebuild_graph` (Tier-1) and the Tier-2 swap. On correct (lock-atomic)
    /// code, that delay runs *inside* the write lock, so a concurrent Tier-2
    /// attempting to acquire the lock simply blocks for the delay's duration
    /// and only proceeds once the bump has already landed — it then sees the
    /// new epoch, detects staleness, and aborts instead of clobbering.
    ///
    /// If the bump were moved back outside the lock (reintroducing the
    /// pre-fix TOCTOU gap), Tier-2 would instead acquire the now-free lock
    /// *during* the delay, see the still-stale epoch, and swap its stale
    /// snapshot over Tier-1's fresh one — flipping the final assertion below
    /// from V2 back to V1.
    #[tokio::test(flavor = "multi_thread")]
    async fn tier1_epoch_bump_is_atomic_with_swap_under_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let file_path = root.join("tasks/task-race.md");
        std::fs::write(
            &file_path,
            "---\nid: task-race\ntitle: V1\ntype: task\nstatus: ready\n---\n\nBody.\n",
        )
        .unwrap();

        let doc = crate::pkb::parse_file(&file_path).unwrap();
        let graph = GraphStore::build(&[doc], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = Arc::new(PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db"),
            Arc::new(RwLock::new(graph)),
        ));

        // Tier-2 pauses briefly after computing its (stale, V1-based)
        // snapshot, before attempting to acquire the write lock for its swap.
        server.set_tier2_sleep_ms(100);
        // Tier-1 holds the write lock for a long delay right before bumping
        // the epoch, giving Tier-2's write-lock attempt a wide window to
        // land while Tier-1's critical section is still open.
        server.set_pre_epoch_bump_delay_ms(400);

        // Kick off Tier-2 first: it snapshots epoch=0 and nodes with V1,
        // then sleeps for 100ms before trying to swap.
        server.schedule_graph_rebuild();

        // Give Tier-2 time to complete its read phase (epoch + nodes
        // snapshot) before Tier-1 starts, then run Tier-1's disk-scanning
        // rebuild synchronously on a blocking thread: writes V2 to disk and
        // calls rebuild_graph(), which will hold the write lock across its
        // 400ms pre-bump delay starting well before Tier-2's ~100ms mark.
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
        let server_for_tier1 = server.clone();
        let file_path_for_tier1 = file_path.clone();
        let tier1 = tokio::task::spawn_blocking(move || {
            std::fs::write(
                &file_path_for_tier1,
                "---\nid: task-race\ntitle: V2\ntype: task\nstatus: ready\n---\n\nBody.\n",
            )
            .unwrap();
            server_for_tier1.rebuild_graph();
        });
        tier1.await.unwrap();

        // Tier-1's own swap must be visible immediately on return.
        {
            let g = server.graph.read();
            let node = g.get_node("task-race").expect("node must still exist");
            assert_eq!(
                node.label, "V2",
                "Tier-1's own swap must be visible immediately"
            );
        }

        // Wait for Tier-2 (and any coalesced follow-up it queues after
        // detecting staleness) to fully drain.
        for _ in 0..100 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if !server.graph_rebuild_pending() {
                break;
            }
        }

        let g = server.graph.read();
        let node = g
            .get_node("task-race")
            .expect("task-race should still exist after Tier-2 settles");
        assert_eq!(
            node.label, "V2",
            "a Tier-2 rebuild snapshotted before Tier-1's disk-scan swap must not \
             clobber it after the fact just because it acquired the write lock \
             before observing the bumped epoch — the bump must be atomic with \
             the swap"
        );
    }

    /// T-carryover (A27, R16): after an in-place write, `get_task`, `list_tasks`
    /// and `export_graph` show the node's previous figures until the rebuild lands;
    /// a new node shows 0 / 0 / 0 with ok. After the rebuild, all show recomputed figures.
    #[tokio::test(flavor = "multi_thread")]
    async fn flow_carried_over_until_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let target_file = root.join("tasks/target-1.md");
        std::fs::write(
            &target_file,
            "---\nid: target-1\ntitle: Target 1\ntype: target\nstatus: ready\nworth: 1.0\n---\nBody.\n",
        )
        .unwrap();

        let task_a_file = root.join("tasks/task-a.md");
        std::fs::write(
            &task_a_file,
            "---\nid: task-a\ntitle: Task A\ntype: task\nstatus: ready\nlinks:\n  - to: target-1\n    label: serves\n    quantum: 1.0\n---\nBody.\n",
        )
        .unwrap();

        let task_b_file = root.join("tasks/task-b.md");
        std::fs::write(
            &task_b_file,
            "---\nid: task-b\ntitle: Task B\ntype: task\nstatus: ready\nlinks:\n  - to: target-1\n    label: serves\n    quantum: 0.5\n---\nBody.\n",
        )
        .unwrap();

        let doc_t = crate::pkb::parse_file(&target_file).unwrap();
        let doc_a = crate::pkb::parse_file(&task_a_file).unwrap();
        let doc_b = crate::pkb::parse_file(&task_b_file).unwrap();

        let graph = GraphStore::build(&[doc_t, doc_a, doc_b], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = Arc::new(PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db"),
            Arc::new(RwLock::new(graph)),
        ));

        // Initial sanity check: task-a has gain 1.0, task-b has gain 0.5.
        // In export_graph, task-a is display_rank 1 and task-b is display_rank 2.
        {
            let res = server.handle_get_task(&json!({"id": "task-a"})).unwrap();
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            let flow = val.get("flow").expect("initial get_task must have flow");
            assert_eq!(flow.get("gain").and_then(|v| v.as_f64()), Some(1.0));
            assert_eq!(flow.get("loss_averted").and_then(|v| v.as_f64()), Some(0.0));
            assert_eq!(
                flow.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );

            let exp_res = server
                .handle_export_graph(&json!({"format": "json"}))
                .unwrap();
            let exp_text: String = exp_res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let exp_val: serde_json::Value = serde_json::from_str(&exp_text).unwrap();
            let nodes = exp_val.get("nodes").and_then(|n| n.as_array()).unwrap();

            let node_a = nodes
                .iter()
                .find(|n| n.get("id").and_then(|id| id.as_str()) == Some("task-a"))
                .unwrap();
            assert_eq!(node_a.get("display_rank").and_then(|r| r.as_u64()), Some(2));
        }

        // Slow down Tier-2 rebuild so we can inspect in-memory state before it lands.
        server.set_tier2_sleep_ms(300);

        // Update task-a on disk so quantum is 0.2 instead of 1.0 (now lower than task-b's 0.5).
        std::fs::write(
            &task_a_file,
            "---\nid: task-a\ntitle: Task A Patched\ntype: task\nstatus: ready\nlinks:\n  - to: target-1\n    label: serves\n    quantum: 0.2\n---\nBody.\n",
        )
        .unwrap();

        // Perform in-place write
        let updated_a = crate::pkb::parse_file(&task_a_file).unwrap();
        server.rebuild_graph_for_pkb_document(&updated_a);

        // Also create a brand new task-c
        let task_c_file = root.join("tasks/task-c.md");
        std::fs::write(
            &task_c_file,
            "---\nid: task-c\ntitle: Task C New\ntype: task\nstatus: ready\n---\nBody.\n",
        )
        .unwrap();
        let new_c = crate::pkb::parse_file(&task_c_file).unwrap();
        server.rebuild_graph_for_pkb_document(&new_c);

        // BEFORE Tier-2 rebuild lands:
        // 1. get_task: task-a keeps previous flow (gain 1.0, loss_averted 0.0, decision_value 0.0); task-c shows 0/0/0 ok
        {
            let res = server.handle_get_task(&json!({"id": "task-a"})).unwrap();
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            let flow = val
                .get("flow")
                .filter(|v| !v.is_null())
                .expect("patched get_task must have flow");
            assert_eq!(
                flow.get("gain").and_then(|v| v.as_f64()),
                Some(1.0),
                "task-a must keep previous gain (1.0) before rebuild"
            );
            assert_eq!(
                flow.get("loss_averted").and_then(|v| v.as_f64()),
                Some(0.0),
                "task-a must keep previous loss_averted (0.0) before rebuild"
            );
            assert_eq!(
                flow.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0),
                "task-a must keep previous decision_value (0.0) before rebuild"
            );

            // New node task-c has 0 / 0 / 0 with ok
            let res_c = server.handle_get_task(&json!({"id": "task-c"})).unwrap();
            let text_c: String = res_c
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val_c: serde_json::Value = serde_json::from_str(&text_c).unwrap();
            let flow_c = val_c
                .get("flow")
                .filter(|v| !v.is_null())
                .expect("new node task-c must have flow");
            assert_eq!(flow_c.get("gain").and_then(|v| v.as_f64()), Some(0.0));
            assert_eq!(
                flow_c.get("loss_averted").and_then(|v| v.as_f64()),
                Some(0.0)
            );
            assert_eq!(
                flow_c.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );
            assert_eq!(
                flow_c.get("flow_status").and_then(|v| v.as_str()),
                Some("ok")
            );
        }

        // 2. list_tasks: task-a keeps previous figures (gain 1.0, loss_averted 0.0, decision_value 0.0)
        {
            let res = server
                .handle_list_tasks(&json!({"format": "json"}))
                .unwrap();
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            let tasks = val.get("tasks").and_then(|t| t.as_array()).unwrap();
            let task_a = tasks
                .iter()
                .find(|t| t.get("id").and_then(|id| id.as_str()) == Some("task-a"))
                .unwrap();
            assert_eq!(
                task_a.get("gain").and_then(|v| v.as_f64()),
                Some(1.0),
                "list_tasks task-a must keep previous gain (1.0) before rebuild"
            );
            assert_eq!(
                task_a.get("loss_averted").and_then(|v| v.as_f64()),
                Some(0.0),
                "list_tasks task-a must keep previous loss_averted (0.0) before rebuild"
            );
            assert_eq!(
                task_a.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0),
                "list_tasks task-a must keep previous decision_value (0.0) before rebuild"
            );
        }

        // 3. export_graph: task-a keeps previous figures and display_rank 2 (above task-b's 0.5 at rank 3)
        {
            let exp_res = server
                .handle_export_graph(&json!({"format": "json"}))
                .unwrap();
            let exp_text: String = exp_res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let exp_val: serde_json::Value = serde_json::from_str(&exp_text).unwrap();
            let nodes = exp_val.get("nodes").and_then(|n| n.as_array()).unwrap();
            let node_a = nodes
                .iter()
                .find(|n| n.get("id").and_then(|id| id.as_str()) == Some("task-a"))
                .unwrap();
            let flow_a = node_a
                .get("flow")
                .filter(|v| !v.is_null())
                .expect("export_graph task-a must have flow");
            assert_eq!(flow_a.get("gain").and_then(|v| v.as_f64()), Some(1.0));
            assert_eq!(
                flow_a.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );
            assert_eq!(
                node_a.get("display_rank").and_then(|r| r.as_u64()),
                Some(2),
                "export_graph task-a must keep previous display_rank (2) before rebuild"
            );
        }

        // Wait for Tier-2 rebuild to drain.
        for _ in 0..100 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if !server.graph_rebuild_pending() {
                break;
            }
        }

        // AFTER Tier-2 rebuild lands:
        // Figures are recomputed: task-a has gain 0.2, decision_value 0.0; display_rank is now 3 (below task-b's 0.5 at rank 2).
        // 1. get_task
        {
            let res = server.handle_get_task(&json!({"id": "task-a"})).unwrap();
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            let flow = val
                .get("flow")
                .expect("after rebuild get_task must have flow");
            assert_eq!(flow.get("gain").and_then(|v| v.as_f64()), Some(0.2));
            assert_eq!(
                flow.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );
        }

        // 2. list_tasks
        {
            let res = server
                .handle_list_tasks(&json!({"format": "json"}))
                .unwrap();
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            let tasks = val.get("tasks").and_then(|t| t.as_array()).unwrap();
            let task_a = tasks
                .iter()
                .find(|t| t.get("id").and_then(|id| id.as_str()) == Some("task-a"))
                .unwrap();
            assert_eq!(task_a.get("gain").and_then(|v| v.as_f64()), Some(0.2));
            assert_eq!(
                task_a.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );
        }

        // 3. export_graph
        {
            let exp_res = server
                .handle_export_graph(&json!({"format": "json"}))
                .unwrap();
            let exp_text: String = exp_res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let exp_val: serde_json::Value = serde_json::from_str(&exp_text).unwrap();
            let nodes = exp_val.get("nodes").and_then(|n| n.as_array()).unwrap();
            let node_a = nodes
                .iter()
                .find(|n| n.get("id").and_then(|id| id.as_str()) == Some("task-a"))
                .unwrap();
            let flow_a = node_a
                .get("flow")
                .expect("export_graph task-a must have flow");
            assert_eq!(flow_a.get("gain").and_then(|v| v.as_f64()), Some(0.2));
            assert_eq!(
                flow_a.get("decision_value").and_then(|v| v.as_f64()),
                Some(0.0)
            );
            assert_eq!(
                node_a.get("display_rank").and_then(|r| r.as_u64()),
                Some(3),
                "export_graph task-a must show recomputed display_rank (3) after rebuild"
            );
        }
    }

    #[test]
    fn tier1_inplace_mutation_updates_generation_incrementally() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::write(
            root.join("tasks/task-x.md"),
            "---\nid: task-x\ntype: task\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tasks/task-y.md"),
            "---\nid: task-y\ntype: task\n---\n",
        )
        .unwrap();

        let docs = vec![
            make_task_doc("task-x", "ready", 1, &[]),
            make_task_doc("task-y", "ready", 2, &[]),
        ];
        let graph = GraphStore::build(&docs, root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );
        let gen_initial = server.graph.read().generation();
        assert_eq!(gen_initial.file_count, 2);

        // 1. In-place update of an existing task -> file_count unchanged, mtime preserved/bumped
        let updated_doc = make_task_doc("task-x", "done", 1, &[]);
        let node_x = crate::graph::GraphNode::from_pkb_document(&updated_doc);
        server
            .graph
            .write()
            .upsert_node_in_place(node_x, &server.pkb_root);

        let gen_updated = server.graph.read().generation();
        assert_eq!(
            gen_updated.file_count, 2,
            "file_count must not change on update of existing node"
        );
        assert!(gen_updated.max_mtime >= gen_initial.max_mtime);

        // 2. In-place addition of a new task -> file_count increments by 1
        std::fs::write(
            root.join("tasks/task-z.md"),
            "---\nid: task-z\ntype: task\n---\n",
        )
        .unwrap();
        let new_doc = make_task_doc("task-z", "ready", 3, &[]);
        let node_z = crate::graph::GraphNode::from_pkb_document(&new_doc);
        server
            .graph
            .write()
            .upsert_node_in_place(node_z, &server.pkb_root);

        let gen_new = server.graph.read().generation();
        assert_eq!(
            gen_new.file_count, 3,
            "file_count must increment by 1 when new node added"
        );
        assert!(gen_new.max_mtime >= gen_updated.max_mtime);

        // 3. In-place removal -> file_count decrements by 1 and generation matches disk
        std::fs::remove_file(root.join("tasks/task-z.md")).unwrap();
        server
            .graph
            .write()
            .remove_node_in_place("task-z", &server.pkb_root);
        let gen_removed = server.graph.read().generation();
        assert_eq!(
            gen_removed.file_count, 2,
            "file_count must decrement by 1 on removal"
        );
        assert_eq!(
            gen_removed,
            crate::pkb::scan_generation(&server.pkb_root),
            "generation stamp after removal must match scan_generation on disk"
        );
    }
}
