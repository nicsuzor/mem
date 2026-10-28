use super::*;
use serde_json::json;
use std::sync::Arc;

fn make_test_server(root: &Path) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    let graph = GraphStore::build(&[], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    )
}

#[test]
fn test_ac1_single_node_read_no_size_limit_returns_full_body_longer_than_4000_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/task-long-ac1.md");
    let long_body = "A".repeat(5500);
    let content = format!(
        "---\nid: task-long-ac1\ntitle: AC1 Long Task\ntype: task\nstatus: ready\n---\n\n{long_body}\n"
    );
    std::fs::write(&doc_path, &content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1. Single get_document with no max_bytes returns full body (>4000 bytes)
    let doc_res = server
        .handle_get_document(&json!({"id": "task-long-ac1"}))
        .expect("get_document should succeed");
    let doc_val = doc_res.structured_content.expect("structured content");
    assert_eq!(
        doc_val.get("truncated").and_then(|v| v.as_bool()),
        Some(false),
        "AC1: get_document without max_bytes must have truncated=false"
    );
    assert_eq!(
        doc_val.get("body_bytes").and_then(|v| v.as_u64()),
        Some(5500),
        "AC1: get_document body_bytes must match full body length"
    );
    let doc_body = doc_val.get("body").and_then(|v| v.as_str()).unwrap();
    assert_eq!(
        doc_body.len(),
        5500,
        "AC1: get_document without max_bytes must return full body without cutting at 4000"
    );
    assert!(
        !doc_body.contains("[truncated"),
        "AC1: get_document full body must not contain truncation notice"
    );

    // 2. Single get_task with no max_bytes returns full body (>4000 bytes)
    let task_res = server
        .handle_get_task(&json!({"id": "task-long-ac1"}))
        .expect("get_task should succeed");
    let task_val = task_res.structured_content.expect("structured content");
    assert_eq!(
        task_val.get("truncated").and_then(|v| v.as_bool()),
        Some(false),
        "AC1: get_task without max_bytes must have truncated=false"
    );
    assert_eq!(
        task_val.get("body_bytes").and_then(|v| v.as_u64()),
        Some(5500),
        "AC1: get_task body_bytes must match full body length"
    );
    let task_body = task_val.get("body").and_then(|v| v.as_str()).unwrap();
    assert_eq!(
        task_body.len(),
        5500,
        "AC1: get_task without max_bytes must return full body without cutting at 4000"
    );
    assert!(
        !task_body.contains("[truncated"),
        "AC1: get_task full body must not contain truncation notice"
    );
}

#[test]
fn test_ac2_read_that_cuts_body_returns_notice_inside_body_text_at_cut() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/task-long-ac2.md");
    let long_body = "B".repeat(5000);
    let content = format!(
        "---\nid: task-long-ac2\ntitle: AC2 Long Task\ntype: task\nstatus: ready\n---\n\n{long_body}\n"
    );
    std::fs::write(&doc_path, &content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1. Single get_document with max_bytes=100
    let doc_res = server
        .handle_get_document(&json!({"id": "task-long-ac2", "max_bytes": 100}))
        .expect("get_document should succeed");
    let doc_val = doc_res.structured_content.expect("structured content");
    assert_eq!(
        doc_val.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        doc_val.get("body_bytes").and_then(|v| v.as_u64()),
        Some(5000)
    );
    let doc_body = doc_val.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(
        doc_body.starts_with(&"B".repeat(100)),
        "AC2: get_document body must preserve initial bytes before cut"
    );
    assert!(
        doc_body.contains("truncated") && doc_body.contains("100") && doc_body.contains("5000"),
        "AC2: get_document body text at cut must name the cut: {doc_body}"
    );
    assert!(
        doc_body.contains("max_bytes"),
        "AC2: get_document body text at cut must explain how to read the rest: {doc_body}"
    );

    // Also check text in markdown content
    let doc_text = &doc_res.content[0].raw.as_text().unwrap().text;
    assert!(
        doc_text.contains("truncated") && doc_text.contains("max_bytes"),
        "AC2: get_document markdown text must also include notice at cut: {doc_text}"
    );

    // 2. Single get_task with max_bytes=100
    let task_res = server
        .handle_get_task(&json!({"id": "task-long-ac2", "max_bytes": 100}))
        .expect("get_task should succeed");
    let task_val = task_res.structured_content.expect("structured content");
    assert_eq!(
        task_val.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    let task_body = task_val.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(
        task_body.starts_with(&"B".repeat(100)),
        "AC2: get_task body must preserve initial bytes before cut"
    );
    assert!(
        task_body.contains("truncated") && task_body.contains("100") && task_body.contains("5000"),
        "AC2: get_task body text at cut must name the cut: {task_body}"
    );
    assert!(
        task_body.contains("max_bytes"),
        "AC2: get_task body text at cut must explain how to read the rest: {task_body}"
    );
}

#[test]
fn test_ac3_batch_reads_return_full_body_by_default_and_cut_notice_when_limited() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    for i in 1..=2 {
        let doc_path = root.join(format!("tasks/task-batch-{i}.md"));
        let long_body = format!("Content of node {i}. {}", "C".repeat(4800));
        let content = format!(
            "---\nid: task-batch-{i}\ntitle: Batch Task {i}\ntype: task\nstatus: ready\n---\n\n{long_body}\n"
        );
        std::fs::write(&doc_path, &content).unwrap();
        let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
        server.rebuild_graph_for_pkb_document(&doc);
    }

    // --- 1. Batch get_document with NO size limit returns full bodies ---
    let doc_batch_res = server
        .handle_get_document(&json!({"ids": ["task-batch-1", "task-batch-2"]}))
        .expect("batch get_document should succeed");
    let doc_batch_val = doc_batch_res
        .structured_content
        .expect("structured content");
    let doc_items = doc_batch_val
        .get("items")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(doc_items.len(), 2);
    for item in doc_items {
        assert_eq!(
            item.get("truncated").and_then(|v| v.as_bool()),
            Some(false),
            "AC3: batch get_document with no size limit must have truncated=false"
        );
        let body = item.get("body").and_then(|v| v.as_str()).unwrap();
        assert!(
            body.len() > 4000,
            "AC3: batch get_document with no size limit must return full body (>4000 bytes), got len {}",
            body.len()
        );
        assert!(!body.contains("[truncated"));
    }

    // --- 2. Batch get_document with max_bytes=80 cuts body with notice ---
    let doc_cut_res = server
        .handle_get_document(&json!({"ids": ["task-batch-1", "task-batch-2"], "max_bytes": 80}))
        .expect("batch get_document with max_bytes should succeed");
    let doc_cut_val = doc_cut_res.structured_content.expect("structured content");
    let doc_cut_items = doc_cut_val.get("items").and_then(|v| v.as_array()).unwrap();
    assert_eq!(doc_cut_items.len(), 2);
    for item in doc_cut_items {
        assert_eq!(
            item.get("truncated").and_then(|v| v.as_bool()),
            Some(true),
            "AC3: batch get_document with max_bytes must have truncated=true"
        );
        let body = item.get("body").and_then(|v| v.as_str()).unwrap();
        assert!(
            body.contains("truncated") && body.contains("max_bytes"),
            "AC3: batch get_document cut body must carry cut notice: {body}"
        );
    }

    // --- 3. Batch get_task with NO size limit returns full bodies ---
    let task_batch_res = server
        .handle_get_task(&json!({"ids": ["task-batch-1", "task-batch-2"]}))
        .expect("batch get_task should succeed");
    let task_batch_val = task_batch_res
        .structured_content
        .expect("structured content");
    let task_items = task_batch_val
        .get("items")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(task_items.len(), 2);
    for item in task_items {
        assert_eq!(
            item.get("truncated").and_then(|v| v.as_bool()),
            Some(false),
            "AC3: batch get_task with no size limit must have truncated=false"
        );
        let body = item.get("body").and_then(|v| v.as_str()).unwrap();
        assert!(
            body.len() > 4000,
            "AC3: batch get_task with no size limit must return full body (>4000 bytes), got len {}",
            body.len()
        );
        assert!(!body.contains("[truncated"));
    }

    // --- 4. Batch get_task with max_bytes=80 cuts body with notice ---
    let task_cut_res = server
        .handle_get_task(&json!({"ids": ["task-batch-1", "task-batch-2"], "max_bytes": 80}))
        .expect("batch get_task with max_bytes should succeed");
    let task_cut_val = task_cut_res.structured_content.expect("structured content");
    let task_cut_items = task_cut_val
        .get("items")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(task_cut_items.len(), 2);
    for item in task_cut_items {
        assert_eq!(
            item.get("truncated").and_then(|v| v.as_bool()),
            Some(true),
            "AC3: batch get_task with max_bytes must have truncated=true"
        );
        let body = item.get("body").and_then(|v| v.as_str()).unwrap();
        assert!(
            body.contains("truncated") && body.contains("max_bytes"),
            "AC3: batch get_task cut body must carry cut notice: {body}"
        );
    }
}

#[test]
fn test_ac4_read_tools_descriptions_state_their_defaults() {
    let tools = PkbSearchServer::get_all_tools();

    let doc_tool = tools
        .iter()
        .find(|t| t.name == "get_document")
        .expect("get_document tool must exist");
    let doc_desc = doc_tool.description.as_deref().unwrap_or_default();
    assert!(
        doc_desc.contains("full body by default") || doc_desc.contains("default: full body") || doc_desc.contains("Returns full body by default"),
        "AC4: get_document tool description must state that it returns full body by default: {doc_desc}"
    );

    let task_tool = tools
        .iter()
        .find(|t| t.name == "get_task")
        .expect("get_task tool must exist");
    let task_desc = task_tool.description.as_deref().unwrap_or_default();
    assert!(
        task_desc.contains("full body by default") || task_desc.contains("default: full body") || task_desc.contains("Returns full body by default"),
        "AC4: get_task tool description must state that it returns full body by default: {task_desc}"
    );
}
