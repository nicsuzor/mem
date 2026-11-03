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
            .get("tasks")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
    assert!(
        structured.get("items").is_none(),
        "items key must not be present on list_tasks (single payload contract)"
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
            .get("documents")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
    assert!(
        structured.get("items").is_none(),
        "items key must not be present on list_documents (single payload contract)"
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
            .get("results")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
    assert!(
        structured.get("items").is_none(),
        "items key must not be present on search (single payload contract)"
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

#[test]
fn test_ac1_payload_carried_once_per_named_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // Write a test task and document
    let task_path = root.join("tasks/task-ac1.md");
    std::fs::write(
        &task_path,
        "---\nid: task-ac1\ntitle: Task AC1\ntype: task\nstatus: ready\n---\n\nAC1 task body content.\n",
    )
    .unwrap();
    let doc = crate::pkb::parse_file_relative(&task_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1. search: has 'results', NOT 'items'
    let search_res = server
        .handle_pkb_search(&json!({"query": "AC1", "format": "json"}))
        .expect("search should succeed");
    let search_val = search_res.structured_content.expect("search structured content");
    assert!(
        search_val.get("results").is_some(),
        "search response must carry 'results'"
    );
    assert!(
        search_val.get("items").is_none(),
        "search response must NOT carry duplicate 'items'"
    );

    // 2. get_document: has 'body', NOT 'content'
    let doc_res = server
        .handle_get_document(&json!({"id": "task-ac1"}))
        .expect("get_document should succeed");
    let doc_val = doc_res.structured_content.expect("get_document structured content");
    assert!(
        doc_val.get("body").is_some(),
        "get_document response must carry 'body'"
    );
    assert!(
        doc_val.get("content").is_none(),
        "get_document response must NOT carry duplicate 'content'"
    );

    // 3. list_documents: has 'documents', NOT 'items'
    let list_docs_res = server
        .handle_list_documents(&json!({"format": "json"}))
        .expect("list_documents should succeed");
    let list_docs_val = list_docs_res.structured_content.expect("list_documents structured content");
    assert!(
        list_docs_val.get("documents").is_some(),
        "list_documents response must carry 'documents'"
    );
    assert!(
        list_docs_val.get("items").is_none(),
        "list_documents response must NOT carry duplicate 'items'"
    );

    // 4. list_tasks: has 'tasks', NOT 'items'
    let list_tasks_res = server
        .handle_list_tasks(&json!({"format": "json"}))
        .expect("list_tasks should succeed");
    let list_tasks_val = list_tasks_res.structured_content.expect("list_tasks structured content");
    assert!(
        list_tasks_val.get("tasks").is_some(),
        "list_tasks response must carry 'tasks'"
    );
    assert!(
        list_tasks_val.get("items").is_none(),
        "list_tasks response must NOT carry duplicate 'items'"
    );
}

#[test]
fn test_ac2_detail_bogus_rejected_with_valid_values() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let task_path = root.join("tasks/task-ac2.md");
    std::fs::write(
        &task_path,
        "---\nid: task-ac2\ntitle: Task AC2\ntype: task\nstatus: ready\n---\n\nAC2 task body content.\n",
    )
    .unwrap();
    let doc = crate::pkb::parse_file_relative(&task_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1. search with bogus detail
    let err_search = server
        .handle_pkb_search(&json!({"query": "AC2", "detail": "bogus"}))
        .unwrap_err();
    assert_eq!(err_search.code, ErrorCode::INVALID_PARAMS);
    let msg = err_search.message.as_ref();
    assert!(
        msg.contains("chunk")
            && msg.contains("snippet")
            && msg.contains("full")
            && msg.contains("metadata")
            && msg.contains("ids"),
        "search error message must list valid values, got: {msg}"
    );

    // 2. get_document with bogus detail (single read)
    let err_doc = server
        .handle_get_document(&json!({"id": "task-ac2", "detail": "bogus"}))
        .unwrap_err();
    assert_eq!(err_doc.code, ErrorCode::INVALID_PARAMS);
    let msg_doc = err_doc.message.as_ref();
    assert!(
        msg_doc.contains("metadata") && msg_doc.contains("summary") && msg_doc.contains("full"),
        "get_document error message must list valid values, got: {msg_doc}"
    );

    // 3. get_document with bogus detail (batch read)
    let err_doc_batch = server
        .handle_get_document(&json!({"ids": ["task-ac2"], "detail": "bogus"}))
        .unwrap_err();
    assert_eq!(err_doc_batch.code, ErrorCode::INVALID_PARAMS);

    // 4. get_task with bogus detail (single read)
    let err_task = server
        .handle_get_task(&json!({"id": "task-ac2", "detail": "bogus"}))
        .unwrap_err();
    assert_eq!(err_task.code, ErrorCode::INVALID_PARAMS);
    let msg_task = err_task.message.as_ref();
    assert!(
        msg_task.contains("metadata") && msg_task.contains("summary") && msg_task.contains("full"),
        "get_task error message must list valid values, got: {msg_task}"
    );

    // 5. get_task with bogus detail (batch read)
    let err_task_batch = server
        .handle_get_task(&json!({"ids": ["task-ac2"], "detail": "bogus"}))
        .unwrap_err();
    assert_eq!(err_task_batch.code, ErrorCode::INVALID_PARAMS);
}

#[test]
fn test_ac3_search_metadata_detail_lean_and_under_2k_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // Write 10 documents with long bodies, tags, etc.
    for i in 1..=10 {
        let path = root.join(format!("tasks/doc-{i}.md"));
        let body = format!("Document body text {} with long description. ", i).repeat(20);
        let content = format!(
            "---\nid: doc-{i}\ntitle: Document Number {i}\ntype: task\nstatus: ready\ntags: [tag1, tag2, tag3, tag4]\n---\n\n{body}\n"
        );
        std::fs::write(&path, &content).unwrap();
        let doc = crate::pkb::parse_file_relative(&path, root).unwrap();
        server.rebuild_graph_for_pkb_document(&doc);
        server.try_upsert_document(&doc);
    }

    let search_res = server
        .handle_pkb_search(&json!({
            "query": "Document Number",
            "limit": 10,
            "detail": "metadata",
            "format": "json"
        }))
        .expect("search should succeed");

    let val = search_res.structured_content.expect("structured content");
    let results = val.get("results").and_then(|v| v.as_array()).expect("results array");
    assert_eq!(results.len(), 10, "should return 10 results");

    for hit in results {
        assert!(hit.get("id").is_some(), "hit must have id");
        assert!(hit.get("title").is_some(), "hit must have title");
        assert!(hit.get("type").is_some(), "hit must have type");
        assert!(hit.get("status").is_some(), "hit must have status");
        assert!(hit.get("score").is_some(), "hit must have score");

        // Must omit ranking internals and tag lists
        assert!(hit.get("recency_decay").is_none(), "metadata detail must omit recency_decay");
        assert!(hit.get("recent_read_boost").is_none(), "metadata detail must omit recent_read_boost");
        assert!(hit.get("salience").is_none(), "metadata detail must omit salience");
        assert!(hit.get("tags").is_none(), "metadata detail must omit tags");
        assert!(hit.get("extract").is_none(), "metadata detail must omit extract");
    }

    // Token estimate check: 10 hits must stay well under 2k tokens (~4 chars per token)
    let json_text = serde_json::to_string_pretty(&val).unwrap();
    let estimated_tokens = json_text.len() / 4;
    assert!(
        estimated_tokens < 2000,
        "10-hit metadata response must stay under 2k tokens (was ~{} tokens)",
        estimated_tokens
    );
}

#[test]
fn test_ac4_batch_reads_honour_fields_projection_and_single_reads_keep_full_body() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let doc1_path = root.join("tasks/batch-doc-1.md");
    let doc2_path = root.join("tasks/batch-doc-2.md");
    let prune_path = root.join("tasks/doc-prune.md");

    let long_body = "Important body line.\n".repeat(50); // ~1050 chars

    std::fs::write(
        &doc1_path,
        format!("---\nid: batch-doc-1\ntitle: Batch Doc 1\ntype: task\nstatus: ready\n---\n\n{long_body}"),
    )
    .unwrap();
    std::fs::write(
        &doc2_path,
        format!("---\nid: batch-doc-2\ntitle: Batch Doc 2\ntype: task\nstatus: in_progress\n---\n\n{long_body}"),
    )
    .unwrap();
    std::fs::write(
        &prune_path,
        "---\nid: doc-prune\ntitle: Prune Doc\ntype: task\nstatus: ready\nempty_str: \"\"\nempty_arr: []\nempty_obj: {}\nvalid_key: keep_me\n---\n\nPrune body.\n",
    )
    .unwrap();

    for p in [&doc1_path, &doc2_path, &prune_path] {
        let doc = crate::pkb::parse_file_relative(p, root).unwrap();
        server.rebuild_graph_for_pkb_document(&doc);
    }

    // 1. Batch read on get_document honours fields projection
    let batch_doc_res = server
        .handle_get_document(&json!({
            "ids": ["batch-doc-1", "batch-doc-2"],
            "fields": ["id", "title"]
        }))
        .expect("batch get_document should succeed");
    let batch_doc_val = batch_doc_res.structured_content.unwrap();
    let items = batch_doc_val.get("items").and_then(|v| v.as_array()).unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert!(item.get("id").is_some());
        assert!(item.get("title").is_some());
        assert!(item.get("body").is_none(), "projected fields must omit body");
        assert!(item.get("frontmatter").is_none(), "projected fields must omit frontmatter");
    }

    // 2. Batch read on get_task honours fields projection
    let batch_task_res = server
        .handle_get_task(&json!({
            "ids": ["batch-doc-1", "batch-doc-2"],
            "fields": ["id", "status"]
        }))
        .expect("batch get_task should succeed");
    let batch_task_val = batch_task_res.structured_content.unwrap();
    let task_items = batch_task_val.get("items").and_then(|v| v.as_array()).unwrap();
    assert_eq!(task_items.len(), 2);
    for item in task_items {
        assert!(item.get("id").is_some());
        assert!(item.get("status").is_some());
        assert!(item.get("title").is_none(), "projected fields must omit title");
        assert!(item.get("body").is_none(), "projected fields must omit body");
    }

    // 3. Single-target reads keep full body by default
    let single_doc_res = server
        .handle_get_document(&json!({"id": "batch-doc-1"}))
        .expect("single get_document should succeed");
    let single_doc_val = single_doc_res.structured_content.unwrap();
    assert_eq!(
        single_doc_val.get("truncated").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        single_doc_val.get("body").and_then(|v| v.as_str()).map(|s| s.len()),
        Some(long_body.trim().len())
    );

    let single_task_res = server
        .handle_get_task(&json!({"id": "batch-doc-1"}))
        .expect("single get_task should succeed");
    let single_task_val = single_task_res.structured_content.unwrap();
    assert_eq!(
        single_task_val.get("truncated").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        single_task_val.get("body").and_then(|v| v.as_str()).map(|s| s.len()),
        Some(long_body.trim().len())
    );

    // 4. Strip null and empty frontmatter keys
    let prune_res = server
        .handle_get_document(&json!({"id": "doc-prune"}))
        .expect("get_document on doc-prune should succeed");
    let prune_val = prune_res.structured_content.unwrap();
    let fm = prune_val.get("frontmatter").and_then(|v| v.as_object()).expect("frontmatter object");
    assert!(fm.get("empty_str").is_none(), "empty string frontmatter key must be stripped");
    assert!(fm.get("empty_arr").is_none(), "empty array frontmatter key must be stripped");
    assert!(fm.get("empty_obj").is_none(), "empty object frontmatter key must be stripped");
    assert_eq!(
        fm.get("valid_key").and_then(|v| v.as_str()),
        Some("keep_me")
    );
}

#[test]
fn test_ac5_schema_descriptions_document_detail_levels() {
    let tools = PkbSearchServer::get_published_tools();

    // 1. search schema description
    let search_tool = tools.iter().find(|t| t.name == "search").expect("search tool");
    let search_desc = search_tool.description.as_deref().unwrap_or_default();
    let search_schema = serde_json::to_value(search_tool.input_schema.as_ref()).unwrap();
    let search_detail_desc = search_schema
        .pointer("/properties/detail/description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let search_combined = format!("{search_desc} {search_detail_desc}");
    for level in ["chunk", "snippet", "full", "metadata", "ids"] {
        assert!(
            search_combined.contains(level),
            "search schema must document detail level '{level}'"
        );
    }

    // 2. get_document schema description
    let doc_tool = tools.iter().find(|t| t.name == "get_document").expect("get_document tool");
    let doc_desc = doc_tool.description.as_deref().unwrap_or_default();
    let doc_schema = serde_json::to_value(doc_tool.input_schema.as_ref()).unwrap();
    let doc_detail_desc = doc_schema
        .pointer("/properties/detail/description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let doc_combined = format!("{doc_desc} {doc_detail_desc}");
    for level in ["metadata", "summary", "full"] {
        assert!(
            doc_combined.contains(level),
            "get_document schema must document detail level '{level}'"
        );
    }

    // 3. get_task schema description
    let task_tool = tools.iter().find(|t| t.name == "get_task").expect("get_task tool");
    let task_desc = task_tool.description.as_deref().unwrap_or_default();
    let task_schema = serde_json::to_value(task_tool.input_schema.as_ref()).unwrap();
    let task_detail_desc = task_schema
        .pointer("/properties/detail/description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let task_combined = format!("{task_desc} {task_detail_desc}");
    for level in ["metadata", "summary", "full"] {
        assert!(
            task_combined.contains(level),
            "get_task schema must document detail level '{level}'"
        );
    }
}
