//! Tests for update_task mixed calls (top-level fields + updates map).
//!
//! Acceptance criteria (task mem_33da554c):
//! - AC1: A test red on main and green on the PR shows a mixed call (top-level fields
//!   plus `updates`) leaving no sent field silently unapplied, for at least `title`,
//!   `status` and one other field; red trace and green run recorded.
//! - AC2: The tool's description states how mixed calls behave.

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

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  proj-root: {}\n",
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
         assignee: alice\n\
         branch: feature-branch\n\
         parent: proj-root\n\
         project: proj-root\n\
         ---\n\n# Seed Task 1\n",
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

/// AC1: A mixed call (top-level fields plus `updates`) leaves no sent field silently
/// unapplied, for at least `title`, `status`, and one other field (`assignee`).
#[test]
fn test_mixed_update_task_applies_both_top_level_and_nested_fields() {
    let (server, _tmp) = setup_fixture_pkb();

    // Call update_task with mixed parameters:
    // - top-level: title, status, assignee
    // - nested updates: branch (set to null to clear)
    let res = server.dispatch_tool_sync(
        "update_task",
        &json!({
            "id": "task-seed1",
            "title": "Updated Task Title",
            "status": "queued",
            "assignee": "ida",
            "updates": {
                "branch": null
            }
        }),
    );
    assert!(res.is_ok(), "mixed update_task call must succeed: {:?}", res.err());

    // Inspect via get_task to confirm all sent fields were applied
    let get_res = server
        .dispatch_tool_sync("get_task", &json!({ "id": "task-seed1" }))
        .expect("get_task must succeed");

    let structured = get_res
        .structured_content
        .expect("get_task must return structured content");

    assert_eq!(
        structured.get("title").and_then(|v| v.as_str()),
        Some("Updated Task Title"),
        "title sent at top level must be applied"
    );
    assert_eq!(
        structured.get("status").and_then(|v| v.as_str()),
        Some("queued"),
        "status sent at top level must be applied"
    );

    let frontmatter = structured
        .get("frontmatter")
        .and_then(|v| v.as_object())
        .expect("frontmatter must be an object");

    assert_eq!(
        frontmatter.get("assignee").and_then(|v| v.as_str()),
        Some("ida"),
        "assignee sent at top level must be applied"
    );
    assert!(
        frontmatter.get("branch").is_none() || frontmatter.get("branch").unwrap().is_null(),
        "branch cleared via updates map must be null or omitted"
    );
}

/// A mixed call with conflicting values for the same field between top-level and
/// `updates` map must fail and name each conflicting field.
#[test]
fn test_mixed_update_task_rejects_conflicting_fields_naming_them() {
    let (server, _tmp) = setup_fixture_pkb();

    let err = server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "task-seed1",
                "title": "Title Top",
                "status": "queued",
                "updates": {
                    "title": "Title Nested",
                    "status": "ready"
                }
            }),
        )
        .expect_err("conflicting fields between top-level and updates must fail");

    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("title"),
        "error message must name conflicting field 'title': {}",
        err.message
    );
    assert!(
        err.message.contains("status"),
        "error message must name conflicting field 'status': {}",
        err.message
    );
}

/// AC2: The tool description for update_task states how mixed calls behave.
#[test]
fn test_update_task_description_states_mixed_call_behavior() {
    let tools = PkbSearchServer::get_published_tools();
    let update_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "update_task")
        .expect("update_task tool must exist");

    let desc = update_tool.description.as_deref().unwrap_or_default();
    assert!(
        desc.to_lowercase().contains("mixed"),
        "update_task description must document mixed calls: {desc}"
    );
    assert!(
        desc.to_lowercase().contains("conflict"),
        "update_task description must explain how conflicting fields behave: {desc}"
    );
}
