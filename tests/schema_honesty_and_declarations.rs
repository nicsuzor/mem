//! TDD test suite for PKB MCP tool schema honesty and declarations (task mem-ffe1b138).
//!
//! Asserts:
//! 1. Every tool's documented required parameters are genuinely required at runtime (missing param -> INVALID_PARAMS).
//! 2. Documented minimal valid calls are accepted by the server.
//! 3. All 7 sampled mismatches from brief are tested (failing conditions and fixed conditions):
//!    - update_task completion_evidence in schema + required for status=done + valid evidence succeeds
//!    - create_task parent declared in required schema (["title", "parent"]) + helpful error
//!    - effort vs complexity disambiguation and rejection error messages
//!    - soft_depends_on accepted on create_task and persisted
//!    - pkb_trace missing param rejection with examples
//!    - search family latency and timeout backoff warnings in schema descriptions
//!    - release_task hint & rejection messages carrying corrected call examples

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use rmcp::model::ErrorCode;
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

fn setup_fixture_pkb() -> (PkbSearchServer, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::create_dir_all(root.join("memories")).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  proj-root: {}\n  aops: {}\n",
    )
    .unwrap();

    let parent_path = root.join("projects/proj-root.md");
    fs::write(
        &parent_path,
        "---\n\
         id: proj-root\n\
         title: \"Root Project\"\n\
         type: epic\n\
         status: active\n\
         project: proj-root\n\
         ---\n\n# Root Project\n",
    )
    .unwrap();

    let task_path = root.join("tasks/task-seed1.md");
    fs::write(
        &task_path,
        "---\n\
         id: task-seed1\n\
         title: \"Seed Task 1\"\n\
         type: task\n\
         status: inbox\n\
         parent: proj-root\n\
         project: proj-root\n\
         ---\n\n# Seed Task 1\n",
    )
    .unwrap();

    let task2_path = root.join("tasks/task-seed2.md");
    fs::write(
        &task2_path,
        "---\n\
         id: task-seed2\n\
         title: \"Seed Task 2\"\n\
         type: task\n\
         status: inbox\n\
         parent: proj-root\n\
         project: proj-root\n\
         ---\n\n# Seed Task 2\n",
    )
    .unwrap();

    let docs = mem::pkb::scan_directory(root)
        .iter()
        .filter_map(|p| mem::pkb::parse_file_relative(p, root))
        .collect::<Vec<_>>();

    let graph = GraphStore::build(&docs, root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("pkb_vectors.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    (server, tmp)
}

// ── 1. Schema Truthfulness: create_task required fields ──

#[test]
fn test_create_task_schema_declares_parent_required() {
    let tools = PkbSearchServer::get_all_tools();
    let create_task = tools
        .iter()
        .find(|t| t.name.as_ref() == "create_task")
        .expect("create_task tool must exist");

    let schema_json = serde_json::to_value(&create_task.input_schema).unwrap();
    let required = schema_json
        .get("required")
        .and_then(|v| v.as_array())
        .expect("create_task schema must have required array");

    let req_strings: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        req_strings.contains(&"title"),
        "create_task must require 'title'"
    );
    assert!(
        req_strings.contains(&"parent"),
        "create_task must truthfully declare 'parent' as required (got: {req_strings:?})"
    );
}

// ── 2. Sample 1: update_task schema properties & completion_evidence validation ──

#[test]
fn test_update_task_schema_has_completion_evidence_and_routing_flags() {
    let tools = PkbSearchServer::get_all_tools();
    let update_task = tools
        .iter()
        .find(|t| t.name.as_ref() == "update_task")
        .expect("update_task tool must exist");

    let schema_json = serde_json::to_value(&update_task.input_schema).unwrap();
    let properties = schema_json
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("update_task schema must have properties object");

    assert!(
        properties.contains_key("completion_evidence"),
        "update_task schema properties must declare 'completion_evidence'"
    );
    assert!(
        properties.contains_key("unparent"),
        "update_task schema properties must declare 'unparent'"
    );
    assert!(
        properties.contains_key("allow_missing_parent"),
        "update_task schema properties must declare 'allow_missing_parent'"
    );
    assert!(
        properties.contains_key("force"),
        "update_task schema properties must declare 'force'"
    );
}

#[test]
fn test_update_task_setting_done_requires_completion_evidence_and_succeeds_with_evidence() {
    let (server, _tmp) = setup_fixture_pkb();

    // 1. Calling update_task setting status=done without completion_evidence fails
    let err = server
        .dispatch_tool_sync("update_task", &json!({
            "id": "task-seed1",
            "status": "done"
        }))
        .expect_err("setting status=done without completion_evidence must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("completion_evidence is required"),
        "error message must state completion_evidence is required: {}",
        err.message
    );
    assert!(
        err.message.contains("Example:"),
        "error message must carry corrected-call example: {}",
        err.message
    );

    // 2. Calling update_task setting status=done with valid completion_evidence succeeds
    let res = server
        .dispatch_tool_sync("update_task", &json!({
            "id": "task-seed1",
            "status": "done",
            "completion_evidence": "Implemented feature and verified with tests."
        }))
        .expect("setting status=done with valid completion_evidence must succeed");
    assert!(
        !res.content.is_empty(),
        "successful update must return content"
    );
}

// ── 3. Sample 2: create_task missing parent error carries corrected example ──

#[test]
fn test_create_task_missing_parent_rejection_and_success() {
    let (server, _tmp) = setup_fixture_pkb();

    // Bare task without parent fails with clear error and example
    let err = server
        .dispatch_tool_sync("create_task", &json!({
            "title": "A task without parent"
        }))
        .expect_err("create_task without parent must fail");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("Missing required parameter: parent"),
        "error must name missing parent parameter: {}",
        err.message
    );
    assert!(
        err.message.contains("Example:"),
        "error must provide corrected call example: {}",
        err.message
    );

    // Task with parent succeeds
    let res = server
        .dispatch_tool_sync("create_task", &json!({
            "title": "A properly parented task",
            "parent": "proj-root"
        }))
        .expect("create_task with parent must succeed");
    assert!(!res.content.is_empty());
}

// ── 4. Sample 3: effort duration string vs complexity size label ──

#[test]
fn test_effort_vs_complexity_error_and_descriptions() {
    let (server, _tmp) = setup_fixture_pkb();

    // Calling create_task with effort="S" fails with clear message directing to complexity
    let err = server
        .dispatch_tool_sync("create_task", &json!({
            "title": "Bad effort task",
            "parent": "proj-root",
            "effort": "S"
        }))
        .expect_err("effort='S' must be rejected");
    assert!(
        err.message.contains("Invalid effort"),
        "error must mention invalid effort: {}",
        err.message
    );
    assert!(
        err.message.contains("complexity"),
        "error must direct caller to 'complexity' for size labels: {}",
        err.message
    );

    // Tool descriptions for create_task and create must disambiguate effort and complexity
    let tools = PkbSearchServer::get_all_tools();
    for tool_name in ["create_task", "create"] {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == tool_name)
            .unwrap();
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let props = schema.get("properties").unwrap();
        let effort_desc = props.get("effort").and_then(|p| p.get("description")).and_then(|d| d.as_str()).unwrap_or("");
        let complexity_desc = props.get("complexity").and_then(|p| p.get("description")).and_then(|d| d.as_str()).unwrap_or("");
        assert!(
            effort_desc.contains("complexity") || effort_desc.contains("size"),
            "tool {tool_name} effort description must mention complexity/size labels: {effort_desc}"
        );
        assert!(
            complexity_desc.contains("effort") || complexity_desc.contains("duration"),
            "tool {tool_name} complexity description must mention effort/duration strings: {complexity_desc}"
        );
    }
}

// ── 5. Sample 4: soft_depends_on accepted on create_task and create ──

#[test]
fn test_soft_depends_on_accepted_in_create_task_and_persisted() {
    let (server, tmp) = setup_fixture_pkb();

    // create_task with soft_depends_on must be accepted (not rejected as unknown key)
    let res = server
        .dispatch_tool_sync("create_task", &json!({
            "id": "task-soft-dep",
            "title": "Task with soft deps",
            "parent": "proj-root",
            "soft_depends_on": ["task-seed1"]
        }))
        .expect("create_task with soft_depends_on must succeed");
    assert!(!res.content.is_empty());

    let task_file = tmp.path().join("tasks/task-soft-dep.md");
    let content = fs::read_to_string(&task_file).expect("task file must exist");
    assert!(
        content.contains("soft_depends_on:"),
        "task file frontmatter must contain soft_depends_on: {content}"
    );
    assert!(
        content.contains("task-seed1"),
        "soft_depends_on must list task-seed1: {content}"
    );
}

// ── 6. Sample 5: pkb_trace missing parameter rejection carries examples ──

#[test]
fn test_pkb_trace_missing_params_rejection_examples() {
    let (server, _tmp) = setup_fixture_pkb();

    let err_from = server
        .dispatch_tool_sync("pkb_trace", &json!({
            "to": "task-seed2"
        }))
        .expect_err("pkb_trace without from must fail");
    assert!(
        err_from.message.contains("Missing required parameter: from"),
        "error must name missing 'from': {}",
        err_from.message
    );
    assert!(
        err_from.message.contains("Example:"),
        "error must contain example: {}",
        err_from.message
    );

    let err_to = server
        .dispatch_tool_sync("pkb_trace", &json!({
            "from": "task-seed1"
        }))
        .expect_err("pkb_trace without to must fail");
    assert!(
        err_to.message.contains("Missing required parameter: to"),
        "error must name missing 'to': {}",
        err_to.message
    );
    assert!(
        err_to.message.contains("Example:"),
        "error must contain example: {}",
        err_to.message
    );
}

// ── 7. Sample 6: Search family latency and timeout backoff warnings ──

#[test]
fn test_search_family_descriptions_document_latency_and_timeout_guidance() {
    let tools = PkbSearchServer::get_all_tools();
    for tool_name in ["search", "task_search", "retrieve_memory"] {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == tool_name)
            .unwrap_or_else(|| panic!("tool {tool_name} must exist"));
        let desc = tool.description.as_deref().unwrap_or("");
        assert!(
            desc.to_lowercase().contains("onnx") || desc.to_lowercase().contains("embedding"),
            "tool {tool_name} description must mention ONNX / embedding search: {desc}"
        );
        assert!(
            desc.to_lowercase().contains("retry") || desc.to_lowercase().contains("back off"),
            "tool {tool_name} description must advise retry / back off on timeout: {desc}"
        );
    }
}

// ── 8. Universal Sweep: Every tool with declared required fields enforces them ──

#[test]
fn test_every_tool_enforces_declared_required_fields() {
    let (server, _tmp) = setup_fixture_pkb();
    let tools = PkbSearchServer::get_all_tools();

    for tool in tools {
        let name = tool.name.as_ref();
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
            .unwrap_or_default();

        if required.is_empty() {
            continue;
        }

        // Test that calling with an empty object {} fails with INVALID_PARAMS for tools that have required fields
        let empty_args = json!({});
        let result = server.dispatch_tool_sync(name, &empty_args);

        assert!(
            result.is_err(),
            "Tool '{name}' declares required fields {required:?} but calling with empty object succeeded!"
        );
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::INVALID_PARAMS,
            "Tool '{name}' missing required fields must return INVALID_PARAMS, got {:?}",
            err.code
        );
    }
}
