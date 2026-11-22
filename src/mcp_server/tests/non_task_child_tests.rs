//! Prohibit creating or reparenting non-task nodes as children of a task.
//! A task's children are only its direct, separate steps, which must be tasks.

use super::*;
use crate::batch_ops::filters::FilterSet;
use crate::batch_ops::reparent::batch_reparent;
use crate::batch_ops::update::batch_update;

fn make_server(root: &Path) -> PkbSearchServer {
    write_test_polecat_yaml(root);
    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    )
}

fn seed_pkb(root: &Path) {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("tasks/task-parent.md"),
        "---\nid: task-parent\ntitle: Parent Task\ntype: task\nstatus: ready\n---\n\nParent body.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tasks/task-child.md"),
        "---\nid: task-child\ntitle: Child Task\ntype: task\nstatus: ready\nparent: task-parent\n---\n\nChild body.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("notes/note-existing.md"),
        "---\nid: note-existing\ntitle: Existing Note\ntype: note\n---\n\nNote body.\n",
    )
    .unwrap();
}

#[test]
fn test_create_document_rejects_non_task_child_of_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    for doc_type in ["note", "knowledge", "memory", "document", "spec"] {
        let err = server
            .handle_create_document(&json!({
                "title": format!("Child {doc_type}"),
                "slug": format!("child-{doc_type}"),
                "type": doc_type,
                "parent": "task-parent",
            }))
            .expect_err(&format!("expected {doc_type} under task-parent to be refused"));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        let msg = err.message.to_string();
        assert!(
            msg.contains("a task's children must be tasks"),
            "error for {doc_type} should explain rule; got: {msg}"
        );
    }
}

#[test]
fn test_create_document_allows_task_child_under_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let res = server
        .handle_create_document(&json!({
            "title": "Subtask via create",
            "slug": "subtask-via-create",
            "type": "task",
            "parent": "task-parent",
        }))
        .expect("creating a task child under a task should succeed");
    assert!(!res.is_error.unwrap_or(false));
}

#[test]
fn test_create_document_allows_note_under_note() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let res = server
        .handle_create_document(&json!({
            "title": "Subnote",
            "slug": "subnote",
            "type": "note",
            "parent": "note-existing",
        }))
        .expect("creating a note under another note should succeed");
    assert!(!res.is_error.unwrap_or(false));
}

#[test]
fn test_create_task_rejects_non_task_type_under_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let err = server
        .handle_create_task(&json!({
            "title": "Note via create_task",
            "slug": "note-via-create-task",
            "type": "note",
            "parent": "task-parent",
        }))
        .expect_err("create_task with type=note under a task must be refused");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("a task's children must be tasks"),
        "unexpected error: {}",
        err.message
    );
}

#[test]
fn test_create_task_allows_task_child_under_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let res = server
        .handle_create_task(&json!({
            "title": "Direct step",
            "slug": "direct-step",
            "parent": "task-parent",
        }))
        .expect("creating a task child under a task via create_task should succeed");
    assert!(!res.is_error.unwrap_or(false));
}

#[test]
fn test_update_task_rejects_reparenting_note_under_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let err = server
        .handle_update_task(&json!({
            "id": "note-existing",
            "parent": "task-parent",
        }))
        .expect_err("reparenting a note under a task must be refused");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("a task's children must be tasks"),
        "unexpected error: {}",
        err.message
    );
}

#[test]
fn test_update_task_rejects_retyping_task_child_to_note() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let err = server
        .handle_update_task(&json!({
            "id": "task-child",
            "type": "note",
        }))
        .expect_err("retyping a child of a task to note must be refused");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("a task's children must be tasks"),
        "unexpected error: {}",
        err.message
    );
}

#[test]
fn test_convert_document_rejects_converting_task_child_to_note() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let server = make_server(dir.path());

    let err = server
        .handle_convert_document(&json!({
            "id": "task-child",
            "type": "note",
        }))
        .expect_err("converting a child of a task to note must be refused");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("a task's children must be tasks"),
        "unexpected error: {}",
        err.message
    );
}

#[test]
fn test_batch_reparent_and_update_reject_non_task_child_of_task() {
    let dir = tempfile::tempdir().unwrap();
    seed_pkb(dir.path());
    let graph = GraphStore::build_from_directory(dir.path());

    let filters = FilterSet {
        ids: Some(vec!["note-existing".to_string()]),
        ..Default::default()
    };
    let reparent_summary = batch_reparent(&graph, dir.path(), &filters, "task-parent", false);
    assert_eq!(reparent_summary.changed, 0);
    assert_eq!(reparent_summary.errors.len(), 1);
    assert!(
        reparent_summary.errors[0]
            .error
            .contains("a task's children must be tasks"),
        "unexpected error: {}",
        reparent_summary.errors[0].error
    );

    let child_filters = FilterSet {
        ids: Some(vec!["task-child".to_string()]),
        ..Default::default()
    };
    let update_summary = batch_update(
        &graph,
        dir.path(),
        &child_filters,
        &json!({ "type": "note" }),
        false,
    );
    assert_eq!(update_summary.changed, 0);
    assert_eq!(update_summary.errors.len(), 1);
    assert!(
        update_summary.errors[0]
            .error
            .contains("a task's children must be tasks"),
        "unexpected error: {}",
        update_summary.errors[0].error
    );
}
