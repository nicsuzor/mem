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
    let mut invalid_tools = Vec::new();
    for tool in &tools {
        match &tool.output_schema {
            Some(schema) => {
                let schema_val = serde_json::to_value(schema.as_ref()).unwrap();
                let is_obj = schema_val.get("type").and_then(|v| v.as_str()) == Some("object");
                let has_props = schema_val
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|m| !m.is_empty())
                    .unwrap_or(false);
                if !is_obj || !has_props {
                    invalid_tools.push(format!(
                        "tool '{}' outputSchema must have type='object' and non-empty properties (got: {})",
                        tool.name, schema_val
                    ));
                }
            }
            None => {
                invalid_tools.push(format!("tool '{}' missing outputSchema", tool.name));
            }
        }
    }
    assert!(
        invalid_tools.is_empty(),
        "Tools with invalid or missing outputSchema:\n{}",
        invalid_tools.join("\n")
    );
}

#[test]
fn test_generated_codemode_types_are_typed_not_bare_object() {
    let tools = PkbSearchServer::get_all_tools();
    let mut declarations = Vec::new();

    for tool in &tools {
        let schema = tool
            .output_schema
            .as_ref()
            .expect("output_schema must exist");
        let schema_val = serde_json::to_value(schema.as_ref()).unwrap();
        let props = schema_val
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("properties must exist and be object");
        assert!(
            !props.is_empty(),
            "Tool '{}' outputSchema properties must not be empty",
            tool.name
        );

        let mut field_decls = Vec::new();
        for (prop_name, prop_def) in props {
            let ts_type = match prop_def.get("type").and_then(|t| t.as_str()) {
                Some("string") => "string",
                Some("integer") | Some("number") => "number",
                Some("boolean") => "boolean",
                Some("array") => "any[]",
                Some("object") => "Record<string, any>",
                _ => "any",
            };
            field_decls.push(format!("    {prop_name}?: {ts_type};"));
        }
        let interface_name = format!("{}Result", tool.name);
        let decl = format!(
            "interface {interface_name} {{\n{}\n}}\nexport function {}(args: any): Promise<{interface_name}>;",
            field_decls.join("\n"),
            tool.name
        );
        declarations.push(decl);
    }

    let all_dts = declarations.join("\n\n");
    assert!(
        !all_dts.contains("Promise<object>"),
        "Generated Code Mode types must not return bare Promise<object>"
    );
    assert_eq!(declarations.len(), tools.len());
}

#[test]
fn test_zero_matches_return_json_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // list_tasks on empty graph
    let res = server
        .handle_list_tasks(&json!({}))
        .expect("list_tasks should succeed");
    assert!(
        res.structured_content.is_some(),
        "list_tasks must return structured content"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );

    // list_documents on empty graph
    let res = server
        .handle_list_documents(&json!({}))
        .expect("list_documents should succeed");
    assert!(
        res.structured_content.is_some(),
        "list_documents must return structured content"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );

    // search on empty graph
    let res = server
        .handle_pkb_search(&json!({"query": "nonexistent term"}))
        .expect("search should succeed");
    assert!(
        res.structured_content.is_some(),
        "search must return structured content"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
}

#[test]
fn test_task_search_list_memories_and_search_by_tag_zero_match_structured_content() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // 1. task_search on empty graph / no match
    let res = server
        .handle_task_search(&json!({"query": "nonexistent"}))
        .expect("task_search should succeed");
    assert!(
        res.structured_content.is_some(),
        "task_search must return structured content on zero match"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );

    // 2. list_memories on empty graph / no match
    let res = server
        .handle_list_memories(&json!({}))
        .expect("list_memories should succeed");
    assert!(
        res.structured_content.is_some(),
        "list_memories must return structured content on zero match"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );

    // 3. search_by_tag on empty graph / no match
    let res = server
        .handle_search_by_tag(&json!({"tags": ["nonexistent_tag"]}))
        .expect("search_by_tag should succeed");
    assert!(
        res.structured_content.is_some(),
        "search_by_tag must return structured content on zero match"
    );
    let structured = res.structured_content.as_ref().unwrap();
    assert_eq!(structured.get("total").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(structured.get("showing").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        structured
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
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
            "slug": "minimal-task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .expect("create_task should succeed");

    let task_id = serde_json::from_str::<serde_json::Value>(
        &create_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>(),
    )
    .unwrap()
    .get("id")
    .and_then(|v| v.as_str())
    .unwrap()
    .to_string();

    let get_res = server
        .handle_get_task(&json!({"id": task_id}))
        .expect("get_task should succeed");
    assert!(
        get_res.structured_content.is_some(),
        "get_task must return structured content"
    );
    let val = get_res.structured_content.unwrap();
    let obj = val.as_object().expect("get_task return must be an object");

    // Null or empty values must NOT exist at top level
    for (k, v) in obj {
        assert!(!v.is_null(), "top-level field '{k}' must not be null");
        if let Some(s) = v.as_str() {
            assert!(
                !s.is_empty(),
                "top-level string field '{k}' must not be empty"
            );
        }
        if let Some(arr) = v.as_array() {
            assert!(
                !arr.is_empty(),
                "top-level array field '{k}' must not be empty"
            );
        }
        if let Some(m) = v.as_object() {
            assert!(
                !m.is_empty(),
                "top-level object field '{k}' must not be empty"
            );
        }
    }

    // Frontmatter fields like due, effort, consequence, severity, goal_type must not be duplicated at top level
    assert!(
        obj.get("consequence").is_none(),
        "consequence should not be at top level"
    );
    assert!(
        obj.get("effort").is_none(),
        "effort should not be at top level"
    );
    assert!(obj.get("due").is_none(), "due should not be at top level");
}

#[test]
fn test_get_task_and_get_document_detail_and_max_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/doc-long.md");
    let long_body = "x".repeat(5000);
    let content = format!(
        "---\nid: doc-long\ntitle: Long doc\ntype: task\nstatus: ready\n---\n\n{long_body}\n"
    );
    std::fs::write(&doc_path, &content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // Default max_bytes returns full body (truncated=false)
    let get_res = server
        .handle_get_task(&json!({"id": "doc-long"}))
        .expect("get_task should succeed");
    let val = get_res.structured_content.unwrap();
    assert_eq!(val.get("truncated").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(val.get("body_bytes").and_then(|v| v.as_u64()), Some(5000));
    let body = val.get("body").and_then(|v| v.as_str()).unwrap();
    assert_eq!(body.len(), 5000, "body should be full length by default");

    // detail: "metadata" omits body
    let get_meta = server
        .handle_get_task(&json!({"id": "doc-long", "detail": "metadata"}))
        .unwrap();
    let val_meta = get_meta.structured_content.unwrap();
    assert!(
        val_meta.get("body").is_none(),
        "detail=metadata must omit body"
    );

    // get_document with max_bytes
    let get_doc_res = server
        .handle_get_document(&json!({"id": "doc-long", "max_bytes": 100}))
        .unwrap();
    let val_doc = get_doc_res.structured_content.unwrap();
    assert_eq!(
        val_doc.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        val_doc.get("body_bytes").and_then(|v| v.as_u64()),
        Some(5000)
    );
}

#[test]
fn test_get_task_and_get_document_detail_summary_yields_distinct_excerpt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/doc-excerpt.md");
    let long_body = "This is sentence one of the body. ".repeat(40); // ~1360 chars
    let content = format!("---\nid: doc-excerpt\ntitle: Excerpt test\ntype: task\nstatus: ready\n---\n\n{long_body}\n");
    std::fs::write(&doc_path, &content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1. get_task: compare full vs summary vs metadata
    let full_res = server
        .handle_get_task(&json!({"id": "doc-excerpt", "detail": "full"}))
        .unwrap();
    let full_val = full_res.structured_content.unwrap();
    let full_body = full_val
        .get("body")
        .and_then(|v| v.as_str())
        .expect("full body must be present");

    let summary_res = server
        .handle_get_task(&json!({"id": "doc-excerpt", "detail": "summary"}))
        .unwrap();
    let summary_val = summary_res.structured_content.unwrap();
    let summary_body = summary_val
        .get("body")
        .and_then(|v| v.as_str())
        .expect("summary body must be present");

    let meta_res = server
        .handle_get_task(&json!({"id": "doc-excerpt", "detail": "metadata"}))
        .unwrap();
    let meta_val = meta_res.structured_content.unwrap();
    assert!(
        meta_val.get("body").is_none(),
        "metadata detail must omit body"
    );

    assert_ne!(
        summary_body, full_body,
        "summary body must be distinct from full body in get_task"
    );
    assert!(
        summary_body.len() < full_body.len(),
        "summary body must be shorter than full body"
    );
    assert!(
        summary_body.len() <= 310,
        "summary body should be excerpted (around 300 chars)"
    );

    // 2. get_document: compare full vs summary vs metadata
    let doc_full_res = server
        .handle_get_document(&json!({"id": "doc-excerpt", "detail": "full"}))
        .unwrap();
    let doc_full_val = doc_full_res.structured_content.unwrap();
    let doc_full_body = doc_full_val
        .get("body")
        .and_then(|v| v.as_str())
        .expect("full body must be present");

    let doc_summary_res = server
        .handle_get_document(&json!({"id": "doc-excerpt", "detail": "summary"}))
        .unwrap();
    let doc_summary_val = doc_summary_res.structured_content.unwrap();
    let doc_summary_body = doc_summary_val
        .get("body")
        .and_then(|v| v.as_str())
        .expect("summary body must be present");

    let doc_meta_res = server
        .handle_get_document(&json!({"id": "doc-excerpt", "detail": "metadata"}))
        .unwrap();
    let doc_meta_val = doc_meta_res.structured_content.unwrap();
    assert!(
        doc_meta_val.get("body").is_none(),
        "metadata detail must omit body in get_document"
    );

    assert_ne!(
        doc_summary_body, doc_full_body,
        "summary body must be distinct from full body in get_document"
    );
    assert!(
        doc_summary_body.len() < doc_full_body.len(),
        "summary body must be shorter than full body in get_document"
    );
    assert!(
        doc_summary_body.len() <= 310,
        "summary body should be excerpted (around 300 chars) in get_document"
    );
}

#[test]
fn test_get_task_and_get_document_batch_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc1_path = root.join("tasks/task-batch-1.md");
    let doc2_path = root.join("tasks/task-batch-2.md");
    std::fs::write(
        &doc1_path,
        "---\nid: task-batch-1\ntitle: Batch 1\ntype: task\nstatus: ready\n---\n\nBody 1\n",
    )
    .unwrap();
    std::fs::write(
        &doc2_path,
        "---\nid: task-batch-2\ntitle: Batch 2\ntype: task\nstatus: in_progress\n---\n\nBody 2\n",
    )
    .unwrap();

    let doc1 = crate::pkb::parse_file_relative(&doc1_path, root).unwrap();
    let doc2 = crate::pkb::parse_file_relative(&doc2_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc1);
    server.rebuild_graph_for_pkb_document(&doc2);

    let batch_res = server
        .handle_get_task(&json!({
            "ids": ["task-batch-1", "task-batch-2", "task-nonexistent"]
        }))
        .expect("batch get_task should succeed");

    let val = batch_res
        .structured_content
        .expect("batch read must return structured content");
    let items = val
        .get("items")
        .and_then(|v| v.as_array())
        .expect("items array must exist");
    assert_eq!(items.len(), 2);
    let not_found = val
        .get("not_found")
        .and_then(|v| v.as_array())
        .expect("not_found array must exist");
    assert_eq!(not_found.len(), 1);
    assert_eq!(not_found[0].as_str(), Some("task-nonexistent"));

    // Similarly for get_document
    let doc_batch_res = server
        .handle_get_document(&json!({
            "ids": ["task-batch-1", "task-batch-2", "task-missing"]
        }))
        .expect("batch get_document should succeed");

    let val_doc = doc_batch_res
        .structured_content
        .expect("batch get_document must return structured content");
    assert_eq!(
        val_doc
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        val_doc
            .get("not_found")
            .and_then(|v| v.as_array())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_enums_and_update_keys_in_schema() {
    let tools = PkbSearchServer::get_all_tools();
    let update_task_tool = tools
        .iter()
        .find(|t| t.name == "update_task")
        .expect("update_task tool must exist");
    let input_schema = serde_json::to_value(update_task_tool.input_schema.as_ref()).unwrap();
    let props = input_schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("properties must exist");

    // Check status enum
    let status_prop = props
        .get("status")
        .expect("status must be in update_task properties");
    let status_enum = status_prop
        .get("enum")
        .and_then(|v| v.as_array())
        .expect("status must have enum");
    assert!(status_enum.iter().any(|v| v.as_str() == Some("ready")));
    assert!(status_enum.iter().any(|v| v.as_str() == Some("done")));
    assert!(status_enum.iter().any(|v| v.as_str() == Some("partial")));

    // Check intent enum or integer
    assert!(
        props.get("intent").is_some(),
        "intent must be in update_task properties"
    );

    // Check severity
    assert!(
        props.get("severity").is_some(),
        "severity must be in update_task properties"
    );

    // Check type
    assert!(
        props.get("type").is_some(),
        "type must be in update_task properties"
    );

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
    let update_res = server
        .handle_update_task(&json!({
            "id": "task-mut",
            "updates": { "assignee": "alice" }
        }))
        .expect("update_task should succeed");

    assert!(
        update_res.structured_content.is_some(),
        "update_task must return structured content"
    );
    let update_val = update_res.structured_content.unwrap();
    assert_eq!(
        update_val.get("id").and_then(|v| v.as_str()),
        Some("task-mut")
    );
    assert!(
        update_val.get("modified").is_some(),
        "update_task must return modified timestamp"
    );

    // release_task
    let release_res = server
        .handle_release_task(&json!({
            "id": "task-mut",
            "status": "partial",
            "summary": "Finished part 1",
            "reason": "Scope seam hit"
        }))
        .expect("release_task should succeed");

    assert!(
        release_res.structured_content.is_some(),
        "release_task must return structured content"
    );
    let release_val = release_res.structured_content.unwrap();
    assert_eq!(
        release_val.get("status").and_then(|v| v.as_str()),
        Some("partial")
    );
    assert!(
        release_val.get("modified").is_some(),
        "release_task must return modified timestamp"
    );
}

#[test]
fn test_complete_task_returns_structured_summary_with_modified() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc_path = root.join("tasks/task-comp.md");
    std::fs::write(
        &doc_path,
        "---\nid: task-comp\ntitle: Complete me\ntype: task\nstatus: ready\nmodified: 2026-01-01T00:00:00Z\n---\n\nInitial body\n",
    )
    .unwrap();
    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    let comp_res = server
        .handle_complete_task(&json!({
            "id": "task-comp",
            "completion_evidence": "All tests passed and verified"
        }))
        .expect("complete_task should succeed");

    assert!(
        comp_res.structured_content.is_some(),
        "complete_task must return structured content"
    );
    let val = comp_res.structured_content.unwrap();
    assert_eq!(val.get("id").and_then(|v| v.as_str()), Some("task-comp"));
    assert_eq!(
        val.get("title").and_then(|v| v.as_str()),
        Some("Complete me")
    );
    assert_eq!(val.get("status").and_then(|v| v.as_str()), Some("done"));
    assert!(
        val.get("modified").is_some(),
        "complete_task must return new modified timestamp in structured content"
    );
}
