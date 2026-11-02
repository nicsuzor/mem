use parking_lot::RwLock;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::pkb::parse_file_relative;
use mem::vectordb::VectorStore;

#[test]
fn test_ac2_stale_or_disagreeing_input_makes_flow_status_non_ok() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let target_file = root.join("targets").join("targ_priced.md");
    std::fs::write(
        &target_file,
        "---\nid: targ_priced\ntitle: Target Priced\ntype: target\nstatus: open\nworth: 0.50\n---\n",
    )
    .unwrap();

    let task_file = root.join("tasks").join("task_serving.md");
    std::fs::write(
        &task_file,
        "---\nid: task_serving\ntitle: Serving Task\ntype: task\nstatus: ready\nlinks:\n  - to: targ_priced\n    label: serves\n    quantum: 0.5\n    probability: 0.8\n    effect: helps\n---\n",
    )
    .unwrap();

    let doc1 = parse_file_relative(&target_file, root).unwrap();
    let doc2 = parse_file_relative(&task_file, root).unwrap();
    let graph = GraphStore::build(&[doc1, doc2.clone()], root);
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());
    let db_path = root.join("db.bin");

    // Index so the store is initially fresh
    mem::index_pkb(root, &db_path, &store, &embedder, false);

    let server = PkbSearchServer::new(
        store,
        embedder,
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Modify target on disk so task_serving's flow input is stale
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        &target_file,
        "---\nid: targ_priced\ntitle: Target Priced Modified\ntype: target\nstatus: open\nworth: 0.80\n---\n",
    )
    .unwrap();

    // 1. Check stale input makes flow_status non-ok
    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "task_serving"}))
        .expect("get_task succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    let flow_status = parsed["flow"]["flow_status"]
        .as_str()
        .expect("flow_status string");
    assert_ne!(
        flow_status, "ok",
        "stale inputs must make flow_status non-ok, got {flow_status}"
    );
    assert_eq!(flow_status, "stale");
}

#[test]
fn test_ac2_disagreeing_inputs_makes_flow_status_disagreeing_inputs() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let target_file = root.join("targets").join("targ_priced.md");
    std::fs::write(
        &target_file,
        "---\nid: targ_priced\ntitle: Target Priced\ntype: target\nstatus: done\nworth: 0.50\n---\n",
    )
    .unwrap();

    let task_file = root.join("tasks").join("task_serving.md");
    std::fs::write(
        &task_file,
        "---\nid: task_serving\ntitle: Serving Task\ntype: task\nstatus: ready\nlinks:\n  - to: targ_priced\n    label: serves\n    quantum: 0.5\n    probability: 0.8\n    effect: helps\n---\n",
    )
    .unwrap();

    let doc1 = parse_file_relative(&target_file, root).unwrap();
    let doc2 = parse_file_relative(&task_file, root).unwrap();
    let graph = GraphStore::build(&[doc1, doc2.clone()], root);
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());
    let db_path = root.join("db.bin");

    mem::index_pkb(root, &db_path, &store, &embedder, false);

    let server = PkbSearchServer::new(
        store,
        embedder,
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "task_serving"}))
        .expect("get_task succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    let flow_status = parsed["flow"]["flow_status"]
        .as_str()
        .expect("flow_status string");
    assert_eq!(
        flow_status, "disagreeing_inputs",
        "unpropagated serves to priced target must yield disagreeing_inputs"
    );
}

#[test]
fn test_ac3_status_lists_stale_documents_when_not_fresh() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let task_file = root.join("tasks").join("task_stale.md");
    std::fs::write(
        &task_file,
        "---\nid: task_stale\ntitle: Stale Task\ntype: task\nstatus: ready\n---\n",
    )
    .unwrap();

    let doc = parse_file_relative(&task_file, root).unwrap();
    let graph = GraphStore::build(&[doc], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let res = server
        .dispatch_tool_sync("status", &json!({}))
        .expect("status succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    assert_eq!(parsed["freshness"]["is_fresh"], false);
    // When is_fresh is false, the status must list the stale documents
    let stale_list = parsed["freshness"]["stale_document_paths"]
        .as_array()
        .or_else(|| parsed["freshness"]["stale_document_names"].as_array())
        .or_else(|| parsed["freshness"]["stale_documents_list"].as_array())
        .expect("status must list the stale documents when is_fresh is false");
    assert!(
        !stale_list.is_empty(),
        "stale documents list must not be empty"
    );
    assert!(
        stale_list
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("task_stale")),
        "stale list must contain task_stale"
    );
}
