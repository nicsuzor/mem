//! Regression tests for task aops_cfc71f8b:
//! Ghost nodes (nodes in graph with empty path because they are referenced by
//! other documents but lack backing files on disk) must never resolve to the PKB root
//! directory, which caused EISDIR (os error 21) on reads and hazardous writes/deletions.

use super::*;
use crate::embeddings::Embedder;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::sync::Arc;

fn setup_server_with_ghost() -> (tempfile::TempDir, PkbSearchServer) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    // Create a live task that references a ghost parent and depends on a ghost task
    let live_task = root.join("tasks/task-live.md");
    std::fs::write(
        &live_task,
        "---\nid: task-live\ntitle: Live Task\ntype: task\nstatus: active\nproject: proj-test\nparent: ghost-parent-123\ndepends_on:\n  - ghost-dep-456\n---\n# Live Task\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    (tmp, server)
}

#[test]
fn test_ghost_node_get_task_returns_actionable_ghost_error() {
    let (_tmp, server) = setup_server_with_ghost();

    let err = server
        .handle_get_task(&json!({"id": "ghost-parent-123"}))
        .unwrap_err();

    let msg = err.message.to_string();
    assert!(
        msg.contains("ghost node"),
        "Expected ghost node error message, got: {msg}"
    );
    assert!(
        msg.contains("referenced by 1 document"),
        "Expected referrer count in message, got: {msg}"
    );
    assert!(
        msg.contains("task-live (parent)"),
        "Expected referring node and edge type in message, got: {msg}"
    );
    assert!(
        !msg.contains("os error 21") && !msg.contains("Is a directory"),
        "Must never disclose EISDIR os error 21: {msg}"
    );
}

#[test]
fn test_ghost_node_get_document_returns_actionable_ghost_error() {
    let (_tmp, server) = setup_server_with_ghost();

    let err = server
        .handle_get_document(&json!({"id": "ghost-parent-123"}))
        .unwrap_err();

    let msg = err.message.to_string();
    assert!(
        msg.contains("ghost node"),
        "Expected ghost node error message, got: {msg}"
    );
    assert!(
        msg.contains("referenced by 1 document"),
        "Expected referrer count in message, got: {msg}"
    );
    assert!(
        !msg.contains("os error 21") && !msg.contains("Is a directory"),
        "Must never disclose EISDIR os error 21: {msg}"
    );
}

#[test]
fn test_ghost_node_write_paths_reject_before_io() {
    let (_tmp, server) = setup_server_with_ghost();

    // 1. append
    let err = server
        .handle_append_to_document(&json!({
            "id": "ghost-parent-123",
            "content": "test append"
        }))
        .unwrap_err();
    let msg = err.message.to_string();
    assert!(msg.contains("ghost node"), "append must reject ghost node: {msg}");

    // 2. update_task
    let err = server
        .handle_update_task(&json!({
            "id": "ghost-parent-123",
            "status": "done"
        }))
        .unwrap_err();
    let msg = err.message.to_string();
    assert!(msg.contains("ghost node"), "update_task must reject ghost node: {msg}");

    // 3. update_body
    let err = server
        .handle_update_body(&json!({
            "id": "ghost-parent-123",
            "new_body": "new body"
        }))
        .unwrap_err();
    let msg = err.message.to_string();
    assert!(msg.contains("ghost node"), "update_body must reject ghost node: {msg}");

    // 4. edit_body
    let err = server
        .handle_edit_body(&json!({
            "id": "ghost-parent-123",
            "diff": "@@ -1,1 +1,1 @@\n-a\n+b\n"
        }))
        .unwrap_err();
    let msg = err.message.to_string();
    assert!(msg.contains("ghost node"), "edit_body must reject ghost node: {msg}");

    // 5. delete
    let err = server
        .handle_delete_document(&json!({
            "id": "ghost-parent-123"
        }))
        .unwrap_err();
    let msg = err.message.to_string();
    assert!(msg.contains("ghost node"), "delete must reject ghost node: {msg}");
}

#[test]
fn test_unreferenced_id_control_not_regressed() {
    let (_tmp, server) = setup_server_with_ghost();

    // Unreferenced ID in get_task must return "Task not found"
    let err = server
        .handle_get_task(&json!({"id": "nonexistent-zzzz"}))
        .unwrap_err();
    assert_eq!(err.message.as_ref(), "Task not found: nonexistent-zzzz");

    // Unreferenced ID in get_document must return "Document not found"
    let err = server
        .handle_get_document(&json!({"id": "nonexistent-zzzz"}))
        .unwrap_err();
    assert_eq!(err.message.as_ref(), "Document not found: nonexistent-zzzz");
}
