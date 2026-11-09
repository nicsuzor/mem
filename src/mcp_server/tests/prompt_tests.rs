use super::*;
use rmcp::model::*;

fn prompt_text(res: &GetPromptResult) -> String {
    serde_json::to_value(&res.messages[0].content)
        .ok()
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
        .expect("expected text content in prompt message")
}

#[test]
fn test_handle_list_prompts_registers_all_shortcuts() {
    let server = build_test_server();
    let res = server.handle_list_prompts().expect("list prompts succeeds");
    let names: Vec<&str> = res.prompts.iter().map(|p| p.name.as_str()).collect();

    assert!(names.contains(&"find-task"));
    assert!(names.contains(&"explore-topic"));
    assert!(names.contains(&"navigate-graph"));
    assert!(names.contains(&"find-by-tag"));
    assert!(names.contains(&"task-status"));
    assert!(names.contains(&"ready-tasks"));
    assert!(names.contains(&"task-dependencies"));
    assert!(!names.contains(&"polecat-compose"));

    // Every prompt must have a description and defined arguments
    for p in &res.prompts {
        assert!(p.description.is_some(), "prompt {} missing description", p.name);
        assert!(!p.description.as_ref().unwrap().is_empty());
    }

    // Verify find-task argument requirements
    let find_task = res.prompts.iter().find(|p| p.name == "find-task").unwrap();
    let args = find_task.arguments.as_ref().unwrap();
    let query_arg = args.iter().find(|a| a.name == "query").unwrap();
    assert_eq!(query_arg.required, Some(true));
    let status_arg = args.iter().find(|a| a.name == "status").unwrap();
    assert_ne!(status_arg.required, Some(true));
    let limit_arg = args.iter().find(|a| a.name == "limit").unwrap();
    assert_ne!(limit_arg.required, Some(true));
}

#[test]
fn test_find_task_prompt_resolution_and_validation() {
    let server = build_test_server();

    // 1. Missing query -> error
    let req = GetPromptRequestParams::new("find-task");
    let err = server.handle_get_prompt(req).expect_err("missing query must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("Missing required parameter: query"));

    // 2. Empty query -> error
    let mut args = serde_json::Map::new();
    args.insert("query".to_string(), json!("   "));
    let req = GetPromptRequestParams::new("find-task").with_arguments(args);
    let err = server.handle_get_prompt(req).expect_err("empty query must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("Parameter 'query' cannot be empty"));

    // 3. Valid basic query (backward compatibility)
    let mut args = serde_json::Map::new();
    args.insert("query".to_string(), json!("auth rewrite"));
    let req = GetPromptRequestParams::new("find-task").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("valid query succeeds");
    assert_eq!(res.messages.len(), 1);
    let msg = prompt_text(&res);
    assert!(msg.contains("I want to find a task about 'auth rewrite'"));
    assert!(msg.contains("task_search"));
    assert!(msg.contains("get_task"));
    assert!(msg.contains("get_task_status"));

    // 4. Parameterized query with status and limit
    let mut args = serde_json::Map::new();
    args.insert("query".to_string(), json!("token efficiency"));
    args.insert("status".to_string(), json!("ready"));
    args.insert("limit".to_string(), json!(10));
    let req = GetPromptRequestParams::new("find-task").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("parameterized query succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("Status filter: \"ready\""));
    assert!(msg.contains("Result limit: 10"));

    // 5. Invalid limit (0 or non-integer)
    let mut args = serde_json::Map::new();
    args.insert("query".to_string(), json!("test"));
    args.insert("limit".to_string(), json!("0"));
    let req = GetPromptRequestParams::new("find-task").with_arguments(args);
    let err = server.handle_get_prompt(req).expect_err("limit=0 must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
}

#[test]
fn test_explore_topic_prompt_resolution_and_validation() {
    let server = build_test_server();

    // 1. Missing query -> error
    let req = GetPromptRequestParams::new("explore-topic");
    let err = server.handle_get_prompt(req).expect_err("missing query must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

    // 2. Valid exploration
    let mut args = serde_json::Map::new();
    args.insert("query".to_string(), json!("vector search algorithms"));
    args.insert("limit".to_string(), json!(3));
    let req = GetPromptRequestParams::new("explore-topic").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("valid explore succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("What do we know about 'vector search algorithms'?"));
    assert!(msg.contains("search"));
    assert!(msg.contains("get_document"));
    assert!(msg.contains("compact snippet"));
}

#[test]
fn test_navigate_graph_prompt_resolution_and_validation() {
    let server = build_test_server();

    // 1. Missing id -> error
    let req = GetPromptRequestParams::new("navigate-graph");
    let err = server.handle_get_prompt(req).expect_err("missing id must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

    // 2. Valid id with direction
    let mut args = serde_json::Map::new();
    args.insert("id".to_string(), json!("task-a1"));
    args.insert("direction".to_string(), json!("downstream"));
    let req = GetPromptRequestParams::new("navigate-graph").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("valid navigate succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("What's connected to 'task-a1'?"));
    assert!(msg.contains("Direction: \"downstream\""));
    assert!(msg.contains("get_dependency_tree"));
}

#[test]
fn test_find_by_tag_prompt_resolution_and_validation() {
    let server = build_test_server();

    // 1. Missing tag -> error
    let req = GetPromptRequestParams::new("find-by-tag");
    let err = server.handle_get_prompt(req).expect_err("missing tag must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

    // 2. Tag with leading '#' should strip '#'
    let mut args = serde_json::Map::new();
    args.insert("tag".to_string(), json!("#performance"));
    let req = GetPromptRequestParams::new("find-by-tag").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("valid tag succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("Show me everything tagged 'performance'."));
    assert!(msg.contains(r#"search_by_tag' with `{"tags": ["performance"]}`"#));
}

#[test]
fn test_task_status_fastpath_prompt() {
    let server = build_test_server();

    // 1. Missing id -> error
    let req = GetPromptRequestParams::new("task-status");
    let err = server.handle_get_prompt(req).expect_err("missing id must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

    // 2. Valid id
    let mut args = serde_json::Map::new();
    args.insert("id".to_string(), json!("task-test-01"));
    let req = GetPromptRequestParams::new("task-status").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("task-status succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("Check the status for task 'task-test-01'"));
    assert!(msg.contains("get_task_status"));
    assert!(msg.contains("< 150 bytes"));
}

#[test]
fn test_ready_tasks_prompt() {
    let server = build_test_server();

    let mut args = serde_json::Map::new();
    args.insert("limit".to_string(), json!(5));
    args.insert("project".to_string(), json!("mem"));
    let req = GetPromptRequestParams::new("ready-tasks").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("ready-tasks succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("Find unblocked tasks ready for work in priority order"));
    assert!(msg.contains("list_tasks"));
    assert!(msg.contains("Project filter: \"mem\""));
}

#[test]
fn test_task_dependencies_prompt() {
    let server = build_test_server();

    let mut args = serde_json::Map::new();
    args.insert("id".to_string(), json!("task-seed1"));
    let req = GetPromptRequestParams::new("task-dependencies").with_arguments(args);
    let res = server.handle_get_prompt(req).expect("task-dependencies succeeds");
    let msg = prompt_text(&res);
    assert!(msg.contains("Inspect dependencies and blockers for task 'task-seed1'"));
    assert!(msg.contains("get_dependency_tree"));
}

#[test]
fn test_unknown_prompt_returns_method_not_found() {
    let server = build_test_server();
    let req = GetPromptRequestParams::new("nonexistent-prompt");
    let err = server.handle_get_prompt(req).expect_err("unknown prompt must error");
    assert_eq!(err.code, ErrorCode::METHOD_NOT_FOUND);
    assert!(err.message.contains("Unknown prompt: nonexistent-prompt"));
}
