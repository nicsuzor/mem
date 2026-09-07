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

        // Test weight_gte filter — negative case: isolated tasks have 0 downstream weight
        let ids = task_ids_from(
            &server
                .handle_list_tasks(&json!({"weight_gte": 1, "format": "json"}))
                .unwrap(),
        );
        assert!(ids.is_empty());

        // Test weight_gte filter — positive case: weight_gte=0 passes all tasks
        let ids = task_ids_from(
            &server
                .handle_list_tasks(&json!({"weight_gte": 0, "format": "json"}))
                .unwrap(),
        );
        assert!(!ids.is_empty());
        assert!(ids.contains(&"task-1".to_string()));
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
            server.patched_during_rebuild.lock().insert("task-x".to_string());
        }

        // 3. Swap phase (identical to rebuild_graph implementation) merges patched nodes:
        {
            let mut g = server.graph.write();
            let patched_ids: Vec<String> =
                server.patched_during_rebuild.lock().iter().cloned().collect();
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
}

