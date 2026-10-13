use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;

#[test]
fn test_status_diagnostics_integration() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let task_file = root.join("tasks").join("task-init.md");
    std::fs::write(
        &task_file,
        "---\nid: task-init\ntitle: Initial Task\ntype: task\nstatus: ready\n---\n\nInitial task body.",
    )
    .unwrap();

    let doc = mem::pkb::parse_file_relative(&task_file, root).unwrap();
    let graph = mem::graph_store::GraphStore::build(&[doc], root);
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = mem::mcp_server::PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path.clone(),
        Arc::new(RwLock::new(graph)),
    );

    // Call status tool via dispatch_tool_sync
    let res = server
        .dispatch_tool_sync("status", &serde_json::json!({}))
        .expect("status tool call succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    // Verify 1: Build identity
    assert_eq!(parsed["name"], "mem");
    assert!(parsed.get("version").is_some());
    assert!(parsed.get("git_hash").is_some());
    assert!(parsed.get("build_profile").is_some());

    // Verify 2: Index state
    assert_eq!(parsed["index"]["document_count"], 1);
    assert_eq!(parsed["index"]["vector_count"], 0);
    assert_eq!(parsed["index"]["last_reindex"]["outcome"], "ok");
    assert!(parsed["index"]["last_reindex"]["timestamp"].is_string());

    // Verify 3: Queue state
    assert_eq!(parsed["queue"]["depth"], 0);
    assert_eq!(parsed["queue"]["embed_pending"], 0);
    assert_eq!(parsed["queue"]["deferred_paths"], 0);

    // Verify 4: Write state
    assert_eq!(parsed["write_state"]["index_locked"], false);
    assert_eq!(parsed["write_state"]["external_lock_held"], false);
    assert_eq!(parsed["write_state"]["save_in_flight"], false);
    assert_eq!(parsed["write_state"]["embed_worker_running"], false);
    assert_eq!(parsed["write_state"]["graph_rebuild_pending"], false);

    // Verify 5: Freshness
    assert_eq!(parsed["freshness"]["is_fresh"], false);
    assert_eq!(parsed["freshness"]["stale_documents"], 1);
}

#[test]
fn test_status_diagnostics_live_state_mutations() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let task_file = root.join("tasks").join("task-a.md");
    std::fs::write(
        &task_file,
        "---\nid: task-a\ntitle: Task A\ntype: task\nstatus: ready\n---\n\nBody A.",
    )
    .unwrap();

    let doc = mem::pkb::parse_file_relative(&task_file, root).unwrap();
    let graph = mem::graph_store::GraphStore::build(&[doc], root);
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = mem::mcp_server::PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path.clone(),
        Arc::new(RwLock::new(graph)),
    );

    let query_status = || -> Value {
        let res = server
            .dispatch_tool_sync("status", &serde_json::json!({}))
            .expect("status tool call succeeds");
        let text: String = res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        serde_json::from_str(&text).expect("valid json output")
    };

    // Live lock mutation test
    {
        let mut lock = mem::vectordb::VectorStore::acquire_lock(&db_path).unwrap();
        let _guard = lock.write().unwrap();
        let s = query_status();
        assert_eq!(s["write_state"]["index_locked"], true);
        assert_eq!(s["write_state"]["external_lock_held"], true);
    }
    let s_after = query_status();
    assert_eq!(s_after["write_state"]["index_locked"], false);
    assert_eq!(s_after["write_state"]["external_lock_held"], false);

    // Live file mutation test
    let task_file_b = root.join("tasks").join("task-b.md");
    std::fs::write(
        &task_file_b,
        "---\nid: task-b\ntitle: Task B\ntype: task\nstatus: ready\n---\n\nBody B.",
    )
    .unwrap();

    let s_file = query_status();
    assert_eq!(s_file["freshness"]["stale_documents"], 2);
    assert_eq!(s_file["freshness"]["is_fresh"], false);

    // Live refresh_graph test
    let ts_before = s_file["index"]["last_reindex"]["timestamp"].as_str().unwrap().to_string();
    std::thread::sleep(std::time::Duration::from_millis(10));
    server
        .dispatch_tool_sync("refresh_graph", &serde_json::json!({}))
        .expect("refresh_graph succeeds");

    let s_refreshed = query_status();
    assert_eq!(s_refreshed["index"]["document_count"], 2);
    assert_ne!(
        s_refreshed["index"]["last_reindex"]["timestamp"].as_str().unwrap(),
        ts_before
    );
}
