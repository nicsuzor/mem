use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;

#[test]
fn test_list_tasks_superseded_by_mcp_e2e() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    // 1. Create an open task with superseded_by
    let open_task_file = root.join("tasks").join("task-open-superseded.md");
    std::fs::write(
        &open_task_file,
        "---\nid: task-open-superseded\ntitle: Open Superseded Task\ntype: task\nstatus: ready\nsuperseded_by: task-canonical-target\n---\n\nBody of open superseded task.",
    )
    .unwrap();

    // 2. Create a closed task with superseded_by
    let closed_task_file = root.join("tasks").join("task-closed-superseded.md");
    std::fs::write(
        &closed_task_file,
        "---\nid: task-closed-superseded\ntitle: Closed Superseded Task\ntype: task\nstatus: done\nsuperseded_by: task-canonical-target\n---\n\nBody of closed superseded task.",
    )
    .unwrap();

    // 3. Create a normal open task without superseded_by
    let normal_task_file = root.join("tasks").join("task-normal.md");
    std::fs::write(
        &normal_task_file,
        "---\nid: task-normal\ntitle: Normal Task\ntype: task\nstatus: ready\n---\n\nBody of normal task.",
    )
    .unwrap();

    let doc1 = mem::pkb::parse_file_relative(&open_task_file, root).unwrap();
    let doc2 = mem::pkb::parse_file_relative(&closed_task_file, root).unwrap();
    let doc3 = mem::pkb::parse_file_relative(&normal_task_file, root).unwrap();

    let graph = mem::graph_store::GraphStore::build(&[doc1, doc2, doc3], root);
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = mem::mcp_server::PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Call list_tasks with has_superseded_by: true (JSON)
    let res_json = server
        .dispatch_tool_sync(
            "list_tasks",
            &serde_json::json!({
                "has_superseded_by": true,
                "format": "json"
            }),
        )
        .expect("list_tasks call succeeds");

    let text_json: String = res_json
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed_json: Value = serde_json::from_str(&text_json).expect("valid json output");

    assert_eq!(parsed_json["total"], 1);
    assert_eq!(parsed_json["showing"], 1);
    assert_eq!(parsed_json["tasks"][0]["id"], "task-open-superseded");
    assert_eq!(parsed_json["tasks"][0]["superseded_by"], "task-canonical-target");

    // Call list_tasks with has_superseded_by: true (Markdown)
    let res_md = server
        .dispatch_tool_sync(
            "list_tasks",
            &serde_json::json!({
                "has_superseded_by": true,
                "format": "markdown"
            }),
        )
        .expect("list_tasks call succeeds");

    let text_md: String = res_md
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();

    assert!(text_md.contains("Superseded By"));
    assert!(text_md.contains("task-canonical-target"));
    assert!(text_md.contains("task-open-superseded"));
    assert!(!text_md.contains("task-normal"));
    assert!(!text_md.contains("task-closed-superseded"));

    // Call list_tasks without has_superseded_by (default Markdown)
    let res_unfiltered = server
        .dispatch_tool_sync(
            "list_tasks",
            &serde_json::json!({
                "format": "markdown"
            }),
        )
        .expect("list_tasks call succeeds");

    let text_unfiltered: String = res_unfiltered
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();

    assert!(!text_unfiltered.contains("Superseded By"));
}
