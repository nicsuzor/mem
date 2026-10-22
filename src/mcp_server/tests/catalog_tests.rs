use super::*;
use parking_lot::RwLock;
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

#[tokio::test]
async fn test_tools_list_publishes_about_16_tools_with_output_schema_and_ttl() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let list_res = server.list_tools_result();

    // Publishes about 16 tools (15 to 17 tools)
    let tool_count = list_res.tools.len();
    assert!(
        (15..=17).contains(&tool_count),
        "tools/list must publish about 16 tools, got {}",
        tool_count
    );

    // Every tool must declare outputSchema with type='object' and non-empty properties
    for tool in &list_res.tools {
        let schema = tool
            .output_schema
            .as_ref()
            .unwrap_or_else(|| panic!("Tool '{}' missing outputSchema", tool.name));
        let schema_val = serde_json::to_value(schema.as_ref()).unwrap();
        assert_eq!(
            schema_val.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "Tool '{}' outputSchema type must be 'object'",
            tool.name
        );
        let props = schema_val
            .get("properties")
            .and_then(|p| p.as_object())
            .unwrap_or_else(|| panic!("Tool '{}' missing properties in outputSchema", tool.name));
        assert!(
            !props.is_empty(),
            "Tool '{}' outputSchema properties must not be empty",
            tool.name
        );
    }

    // Carries ttlMs in _meta
    let meta = list_res.meta.expect("list_tools result must carry meta");
    let ttl_ms = meta
        .0
        .get("ttlMs")
        .and_then(|v| v.as_u64())
        .expect("meta must carry ttlMs as a positive integer");
    assert!(ttl_ms > 0, "ttlMs must be positive, got {ttl_ms}");
}

#[test]
fn test_catalog_size_measured_near_5k_tokens() {
    let tools = PkbSearchServer::get_published_tools();
    let json_bytes = serde_json::to_string(&tools).unwrap().len();
    let all_tools = PkbSearchServer::get_all_tools();
    let all_bytes = serde_json::to_string(&all_tools).unwrap().len();
    eprintln!("All 39 tools size: {} chars, ~{:.0} tokens", all_bytes, all_bytes as f64 / 3.6);
    let tokens = json_bytes as f64 / 3.6;
    eprintln!("Published catalog size: {} chars, ~{:.0} tokens", json_bytes, tokens);
    for t in &tools {
        let s = serde_json::to_string(t).unwrap();
        let in_len = serde_json::to_string(&t.input_schema).unwrap().len();
        let out_len = serde_json::to_string(&t.output_schema).unwrap().len();
        eprintln!("  Tool {:18}: {:5} chars (in: {:4}, out: {:4}) ~{:4.0} tokens", t.name, s.len(), in_len, out_len, s.len() as f64 / 3.6);
    }
    assert!(
        (3500.0..=7000.0).contains(&tokens),
        "Catalog size ({:.0} tokens, {} bytes) must be near 5 K tokens",
        tokens,
        json_bytes
    );
}

#[test]
fn test_all_retired_tools_resolve_through_shims() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let retired_tools = PkbSearchServer::get_retired_tools();
    assert_eq!(retired_tools.len(), 22, "Must have 22 retired tools");

    for tool in &retired_tools {
        let res = server.dispatch_tool_sync(tool.name.as_ref(), &serde_json::json!({}));
        if let Err(ref e) = res {
            assert_ne!(
                e.code,
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "Retired tool '{}' must resolve through a shim, but got METHOD_NOT_FOUND",
                tool.name
            );
        }
    }
}

#[test]
fn test_retired_tools_behaviours_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // 1. task_summary
    let summary_res = server.dispatch_tool_sync("task_summary", &serde_json::json!({})).unwrap();
    assert!(summary_res.structured_content.is_some() || !summary_res.content.is_empty());

    // 2. nested_tasks
    let nested_res = server.dispatch_tool_sync("nested_tasks", &serde_json::json!({"format": "tree"})).unwrap();
    assert!(nested_res.structured_content.is_some() || !nested_res.content.is_empty());

    // 3. get_stats
    let stats_res = server.dispatch_tool_sync("get_stats", &serde_json::json!({})).unwrap();
    assert!(stats_res.structured_content.is_some() || !stats_res.content.is_empty());

    // 4. refresh_graph
    let refresh_res = server.dispatch_tool_sync("refresh_graph", &serde_json::json!({})).unwrap();
    assert!(refresh_res.structured_content.is_some() || !refresh_res.content.is_empty());

    // 5. find_duplicates
    let dup_res = server.dispatch_tool_sync("find_duplicates", &serde_json::json!({})).unwrap();
    assert!(dup_res.structured_content.is_some() || !dup_res.content.is_empty());
}

#[test]
fn test_release_task_schema_carries_checklist_validation_docstrings() {
    let tools = PkbSearchServer::get_published_tools();
    let release_tool = tools
        .iter()
        .find(|t| t.name == "release_task")
        .expect("release_task must be published");

    let desc = release_tool.description.as_deref().unwrap_or("");
    assert!(
        desc.contains("releasing to done requires all acceptance checklist items in the task body to be ticked (- [x])"),
        "release_task description must document checklist validation requirement, got: {desc}"
    );

    let schema_val = serde_json::to_value(&release_tool.input_schema).unwrap();
    let comp_ev_desc = schema_val
        .pointer("/properties/completion_evidence/description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        comp_ev_desc.contains("Releasing as done requires all acceptance checklist items to be checked (- [x])"),
        "completion_evidence description must document checklist validation requirement, got: {comp_ev_desc}"
    );
}
