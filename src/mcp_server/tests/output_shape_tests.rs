use super::*;
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
fn test_all_tools_declare_output_schema() {
    let tools = PkbSearchServer::get_all_tools();
    assert!(!tools.is_empty(), "tools list should not be empty");
    let mut missing_schema = Vec::new();
    for tool in &tools {
        match &tool.output_schema {
            Some(schema) => {
                let schema_val = serde_json::to_value(schema.as_ref()).unwrap();
                assert_eq!(
                    schema_val.get("type").and_then(|v| v.as_str()),
                    Some("object"),
                    "tool '{}' outputSchema type must be 'object'",
                    tool.name
                );
            }
            None => {
                missing_schema.push(tool.name.to_string());
            }
        }
    }
    assert!(
        missing_schema.is_empty(),
        "Tools missing outputSchema: {:?}",
        missing_schema
    );
}

#[test]
fn test_zero_matches_return_json_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // list_tasks on empty graph
    let res = server.handle_list_tasks(&json!({})).expect("list_tasks should succeed");
    assert!(res.structured_content.is_some(), "list_tasks must return structured content");
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("items").and_then(|v| v.as_array()).map(|a| a.len()), Some(0));

    // list_documents on empty graph
    let res = server.handle_list_documents(&json!({})).expect("list_documents should succeed");
    assert!(res.structured_content.is_some(), "list_documents must return structured content");
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("items").and_then(|v| v.as_array()).map(|a| a.len()), Some(0));

    // search on empty graph
    let res = server.handle_pkb_search(&json!({"query": "nonexistent term"})).expect("search should succeed");
    assert!(res.structured_content.is_some(), "search must return structured content");
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("items").and_then(|v| v.as_array()).map(|a| a.len()), Some(0));
}

#[test]
fn test_get_task_and_get_document_omit_null_and_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // Create a task with minimal fields
    let create_res = server
        .handle_create_task(&json!({
            "title": "Minimal task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .expect("create_task should succeed");

    let task_id = serde_json::from_str::<serde_json::Value>(
        &create_res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect::<String>()
    )
    .unwrap()
    .get("id")
    .and_then(|v| v.as_str())
    .unwrap()
    .to_string();

    let get_res = server.handle_get_task(&json!({"id": task_id})).expect("get_task should succeed");
    assert!(get_res.structured_content.is_some(), "get_task must return structured content");
    let val = get_res.structured_content.unwrap();
    let obj = val.as_object().expect("get_task return must be an object");

    // Null or empty values must NOT exist at top level
    for (k, v) in obj {
        assert!(!v.is_null(), "top-level field '{k}' must not be null");
        if let Some(s) = v.as_str() {
            assert!(!s.is_empty(), "top-level string field '{k}' must not be empty");
        }
        if let Some(arr) = v.as_array() {
            assert!(!arr.is_empty(), "top-level array field '{k}' must not be empty");
        }
        if let Some(m) = v.as_object() {
            assert!(!m.is_empty(), "top-level object field '{k}' must not be empty");
        }
    }

    // Frontmatter fields like due, effort, consequence, severity, goal_type must not be duplicated at top level
    assert!(obj.get("consequence").is_none(), "consequence should not be at top level");
    assert!(obj.get("effort").is_none(), "effort should not be at top level");
    assert!(obj.get("due").is_none(), "due should not be at top level");
}

#[test]
fn test_get_task_and_get_document_detail_and_max_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/doc-long.md");
    let long_body = "x".repeat(5000);
    let content = format!("---\nid: doc-long\ntitle: Long doc\ntype: task\nstatus: ready\n---\n\n{long_body}\n");
    std::fs::write(&doc_path, &content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // Default max_bytes bounds body to 4000 and returns truncated & body_bytes
    let get_res = server.handle_get_task(&json!({"id": "doc-long"})).expect("get_task should succeed");
    let val = get_res.structured_content.unwrap();
    assert_eq!(val.get("truncated").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(val.get("body_bytes").and_then(|v| v.as_u64()), Some(5000));
    let body = val.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(body.len() <= 4100, "body should be bounded by default max_bytes");

    // detail: "metadata" omits body
    let get_meta = server.handle_get_task(&json!({"id": "doc-long", "detail": "metadata"})).unwrap();
    let val_meta = get_meta.structured_content.unwrap();
    assert!(val_meta.get("body").is_none(), "detail=metadata must omit body");

    // get_document with max_bytes
    let get_doc_res = server.handle_get_document(&json!({"id": "doc-long", "max_bytes": 100})).unwrap();
    let val_doc = get_doc_res.structured_content.unwrap();
    assert_eq!(val_doc.get("truncated").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(val_doc.get("body_bytes").and_then(|v| v.as_u64()), Some(5000));
}

#[test]
fn test_get_task_and_get_document_batch_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc1_path = root.join("tasks/task-batch-1.md");
    let doc2_path = root.join("tasks/task-batch-2.md");
    std::fs::write(&doc1_path, "---\nid: task-batch-1\ntitle: Batch 1\ntype: task\nstatus: ready\n---\n\nBody 1\n").unwrap();
    std::fs::write(&doc2_path, "---\nid: task-batch-2\ntitle: Batch 2\ntype: task\nstatus: in_progress\n---\n\nBody 2\n").unwrap();

    let doc1 = crate::pkb::parse_file_relative(&doc1_path, root).unwrap();
    let doc2 = crate::pkb::parse_file_relative(&doc2_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc1);
    server.rebuild_graph_for_pkb_document(&doc2);

    let batch_res = server.handle_get_task(&json!({
        "ids": ["task-batch-1", "task-batch-2", "task-nonexistent"]
    })).expect("batch get_task should succeed");

    let val = batch_res.structured_content.expect("batch read must return structured content");
    let items = val.get("items").and_then(|v| v.as_array()).expect("items array must exist");
    assert_eq!(items.len(), 2);
    let not_found = val.get("not_found").and_then(|v| v.as_array()).expect("not_found array must exist");
    assert_eq!(not_found.len(), 1);
    assert_eq!(not_found[0].as_str(), Some("task-nonexistent"));

    // Similarly for get_document
    let doc_batch_res = server.handle_get_document(&json!({
        "ids": ["task-batch-1", "task-batch-2", "task-missing"]
    })).expect("batch get_document should succeed");

    let val_doc = doc_batch_res.structured_content.expect("batch get_document must return structured content");
    assert_eq!(val_doc.get("items").and_then(|v| v.as_array()).unwrap().len(), 2);
    assert_eq!(val_doc.get("not_found").and_then(|v| v.as_array()).unwrap().len(), 1);
}

#[test]
fn test_enums_and_update_keys_in_schema() {
    let tools = PkbSearchServer::get_all_tools();
    let update_task_tool = tools.iter().find(|t| t.name == "update_task").expect("update_task tool must exist");
    let input_schema = serde_json::to_value(update_task_tool.input_schema.as_ref()).unwrap();
    let props = input_schema.get("properties").and_then(|v| v.as_object()).expect("properties must exist");

    // Check status enum
    let status_prop = props.get("status").expect("status must be in update_task properties");
    let status_enum = status_prop.get("enum").and_then(|v| v.as_array()).expect("status must have enum");
    assert!(status_enum.iter().any(|v| v.as_str() == Some("ready")));
    assert!(status_enum.iter().any(|v| v.as_str() == Some("done")));
    assert!(status_enum.iter().any(|v| v.as_str() == Some("partial")));

    // Check intent enum or integer
    assert!(props.get("intent").is_some(), "intent must be in update_task properties");

    // Check severity
    assert!(props.get("severity").is_some(), "severity must be in update_task properties");

    // Check type
    assert!(props.get("type").is_some(), "type must be in update_task properties");

    // Check description mentions update keys or enum
    let desc = update_task_tool.description.as_deref().unwrap_or_default();
    assert!(
        desc.contains("status") && desc.contains("type") && desc.contains("intent"),
        "description must mention enumerated fields"
    );
}

#[test]
fn test_mutations_return_summary_with_modified() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/task-mut.md");
    std::fs::write(&doc_path, "---\nid: task-mut\ntitle: Mut task\ntype: task\nstatus: ready\nmodified: 2026-01-01T00:00:00Z\n---\n\nBody\n").unwrap();
    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // update_task
    let update_res = server.handle_update_task(&json!({
        "id": "task-mut",
        "updates": { "assignee": "alice" }
    })).expect("update_task should succeed");

    assert!(update_res.structured_content.is_some(), "update_task must return structured content");
    let update_val = update_res.structured_content.unwrap();
    assert_eq!(update_val.get("id").and_then(|v| v.as_str()), Some("task-mut"));
    assert!(update_val.get("modified").is_some(), "update_task must return modified timestamp");

    // release_task
    let release_res = server.handle_release_task(&json!({
        "id": "task-mut",
        "status": "partial",
        "summary": "Finished part 1",
        "reason": "Scope seam hit"
    })).expect("release_task should succeed");

    assert!(release_res.structured_content.is_some(), "release_task must return structured content");
    let release_val = release_res.structured_content.unwrap();
    assert_eq!(release_val.get("status").and_then(|v| v.as_str()), Some("partial"));
    assert!(release_val.get("modified").is_some(), "release_task must return modified timestamp");
}
