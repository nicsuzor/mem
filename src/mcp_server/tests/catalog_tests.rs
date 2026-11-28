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

    // Publishes 20 tools
    let tool_count = list_res.tools.len();
    assert!(
        (15..=20).contains(&tool_count),
        "tools/list must publish 15..=20 tools, got {}",
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

    // Carries native MCP 2026-07-28 cache hints (ttlMs and cacheScope)
    let ttl_ms = list_res
        .ttl_ms
        .expect("list_tools result must carry ttl_ms");
    assert!(ttl_ms > 0, "ttl_ms must be positive, got {ttl_ms}");
    assert_eq!(
        list_res.cache_scope,
        Some(rmcp::model::CacheScope::Public),
        "list_tools result must carry public cache_scope"
    );
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
        (3500.0..=9000.0).contains(&tokens),
        "Catalog size ({:.0} tokens, {} bytes) must stay bounded under 9 K tokens",
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

#[tokio::test]
async fn test_mcp_2026_07_28_catalog_annotations_2020_12_and_sep1303() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_test_server(tmp.path());

    // 1. Deterministic tools/list ordering across calls
    let list1 = server.list_tools_result();
    let list2 = server.list_tools_result();
    let names1: Vec<&str> = list1.tools.iter().map(|t| t.name.as_ref()).collect();
    let names2: Vec<&str> = list2.tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names1, names2, "tools/list ordering must be deterministic");

    // 2. No session_id in input_schema.properties or required; complete ToolAnnotations; 2020-12 outputSchema
    for tool in &list1.tools {
        let in_val = serde_json::to_value(tool.input_schema.as_ref()).unwrap();
        let props = in_val.get("properties").and_then(|p| p.as_object());
        assert!(
            !props.is_some_and(|m| m.contains_key("session_id")),
            "Published tool '{}' must not declare session_id in inputSchema.properties",
            tool.name
        );
        let req = in_val.get("required").and_then(|r| r.as_array());
        assert!(
            !req.is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("session_id"))),
            "Published tool '{}' must not require session_id in inputSchema.required",
            tool.name
        );

        let ann = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("Tool '{}' missing annotations", tool.name));
        assert!(ann.read_only_hint.is_some(), "Tool '{}' missing readOnlyHint", tool.name);
        assert!(ann.destructive_hint.is_some(), "Tool '{}' missing destructiveHint", tool.name);
        assert!(ann.idempotent_hint.is_some(), "Tool '{}' missing idempotentHint", tool.name);
        assert!(ann.open_world_hint.is_some(), "Tool '{}' missing openWorldHint", tool.name);

        let out_val = serde_json::to_value(tool.output_schema.as_ref().unwrap().as_ref()).unwrap();
        assert_eq!(
            out_val.get("$schema").and_then(|v| v.as_str()),
            Some("https://json-schema.org/draft/2020-12/schema"),
            "Tool '{}' outputSchema must declare JSON Schema 2020-12",
            tool.name
        );
    }

    // 3. Server instructions must not require session_id or mention retired read-only proxy domains
    let info = rmcp::ServerHandler::get_info(&server);
    let instr = info.instructions.unwrap_or_default();
    assert!(
        !instr.contains("MUST include a non-empty `session_id`"),
        "Server instructions must not require session_id: {instr}"
    );
    assert!(
        !instr.contains("mcp-ro"),
        "Server instructions must omit retired read-only proxy domain references: {instr}"
    );

    // 4. SEP-1303: input validation failure on a known tool returns CallToolResult(isError=True)
    let err_res = server
        .execute_tool_call("create_task", &serde_json::json!({ "project": "mem" }), None, None)
        .await
        .expect("SEP-1303: validation failure must return Ok(CallToolResult) with is_error=true");
    assert_eq!(
        err_res.is_error,
        Some(true),
        "SEP-1303 validation failure must set is_error=Some(true)"
    );

    // 5. Unknown tool still returns protocol error (-32601 METHOD_NOT_FOUND)
    let proto_err = server
        .execute_tool_call("nonexistent_tool_xyz", &serde_json::json!({}), None, None)
        .await
        .expect_err("unknown tool must return McpError");
    assert_eq!(proto_err.code, rmcp::model::ErrorCode::METHOD_NOT_FOUND);
}
