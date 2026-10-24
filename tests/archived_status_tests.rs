//! Tests for canonical note status `archived` and the archive script (task aops_cc0e118c).
//!
//! Acceptance criteria:
//! 1. Setting `archived` on a note succeeds: failing test before, passing after, cited.
//! 2. A script run moves archived notes into the sessions repo and they no longer resolve in the PKB: test output cited.

use mem::document_crud::{create_document, update_document, DocumentFields};
use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn seed_pkb() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("concepts")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  aops: {}\n  beta: {}\n",
    )
    .unwrap();

    let proj_path = root.join("projects/proj-root.md");
    fs::write(
        &proj_path,
        "---\n\
         id: proj-root\n\
         title: \"Root Project\"\n\
         type: epic\n\
         status: in_progress\n\
         project: aops\n\
         ---\n\n# Root Project\n",
    )
    .unwrap();

    let db_path = root.join("pkb_vectors.bin");
    (tmp, db_path)
}

fn build_server(root: &Path, db_path: &Path) -> (PkbSearchServer, Arc<RwLock<GraphStore>>) {
    let graph_store = GraphStore::build_from_directory(root);
    let graph = Arc::new(RwLock::new(graph_store));
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());

    let server = PkbSearchServer::new(
        store,
        embedder,
        root.to_path_buf(),
        db_path.to_path_buf(),
        graph.clone(),
    );

    (server, graph)
}

fn find_file_by_id(root: &Path, id: &str) -> Option<PathBuf> {
    mem::pkb::scan_directory(root).into_iter().find(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().starts_with(id))
            .unwrap_or(false)
    })
}

#[test]
fn test_archived_status_canonical_definitions() {
    assert!(
        mem::graph::is_valid_status("archived"),
        "archived must be a valid status"
    );
    assert!(
        mem::graph::VALID_STATUSES.contains(&"archived"),
        "archived must be in VALID_STATUSES"
    );
    assert!(
        mem::graph::COMPLETED_STATUSES.contains(&"archived"),
        "archived must be in COMPLETED_STATUSES"
    );
    assert!(
        mem::graph::is_completed(Some("archived")),
        "archived must be completed"
    );
    assert!(
        mem::graph::is_closed_for_hierarchy(Some("archived")),
        "archived must be closed for hierarchy"
    );
    assert_eq!(mem::graph::status_group(Some("archived")), "completed");
    assert_eq!(mem::graph::resolve_status_alias("archived"), "archived");
    assert_eq!(
        mem::graph::status_to_flow_state(Some("archived")),
        mem::graph::FlowState::Gone
    );
    assert_eq!(
        mem::flow::status_to_flow_state(Some("archived")),
        mem::flow::FlowState::Gone
    );
}

#[test]
fn test_setting_archived_on_task_and_non_task_notes_succeeds() {
    let (tmp, db_path) = seed_pkb();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    // 1. Task note: create and update to archived via update_task
    let res = server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "title": "Task to archive",
                "parent": "proj-root",
                "project": "aops",
                "status": "ready",
                "slug": "task_to_archive"
            }),
        )
        .expect("create_task should succeed");
    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let created: serde_json::Value = serde_json::from_str(text).unwrap();
    let task_id = created.get("id").and_then(|v| v.as_str()).unwrap();

    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": task_id,
                "status": "archived"
            }),
        )
        .expect("update_task with status=archived must succeed");

    let task_path = find_file_by_id(tmp.path(), task_id).expect("task file exists");
    let task_content = fs::read_to_string(&task_path).unwrap();
    assert!(
        task_content.contains("status: archived"),
        "task must have status: archived on disk: {task_content}"
    );

    // 2. Non-task note (e.g. note): create and update status to archived via update_document
    let note_fields = DocumentFields {
        id: Some("note-stale".to_string()),
        title: "Stale Note".to_string(),
        doc_type: "note".to_string(),
        body: Some("A note that is no longer active.".to_string()),
        status: None,
        ..Default::default()
    };
    let note_path =
        create_document(tmp.path(), note_fields).expect("create_document should succeed");

    let mut updates = HashMap::new();
    updates.insert("status".to_string(), json!("archived"));
    update_document(&note_path, updates)
        .expect("update_document with status=archived must succeed");

    let note_content = fs::read_to_string(&note_path).unwrap();
    assert!(
        note_content.contains("status: archived"),
        "note must have status: archived: {note_content}"
    );
}

#[test]
fn test_archive_script_moves_notes_and_removes_from_pkb() {
    let (tmp_pkb, db_path) = seed_pkb();
    let (_server, _graph) = build_server(tmp_pkb.path(), &db_path);

    // Create a temporary sessions repo directory with an archived/ subfolder
    let tmp_sessions = tempfile::tempdir().unwrap();
    let sessions_root = tmp_sessions.path().to_path_buf();
    fs::create_dir_all(sessions_root.join("archived")).unwrap();

    // Note 1: Task with status: archived
    let task_archived_path = tmp_pkb.path().join("tasks/task-to-archive.md");
    fs::write(
        &task_archived_path,
        "---\nid: task-to-archive\ntitle: \"Archived Task\"\ntype: task\nstatus: archived\n---\nBody 1\n",
    )
    .unwrap();

    // Note 2: Concept with status: archived
    let concept_archived_path = tmp_pkb.path().join("concepts/concept-to-archive.md");
    fs::write(
        &concept_archived_path,
        "---\nid: concept-to-archive\ntitle: \"Archived Concept\"\ntype: concept\nstatus: archived\n---\nBody 2\n",
    )
    .unwrap();

    // Note 3: Active task (status: ready) - must NOT be moved
    let task_active_path = tmp_pkb.path().join("tasks/task-active.md");
    fs::write(
        &task_active_path,
        "---\nid: task-active\ntitle: \"Active Task\"\ntype: task\nstatus: ready\n---\nActive task body\n",
    )
    .unwrap();

    // Note 4: Topic note without status - must NOT be moved
    let topic_path = tmp_pkb.path().join("concepts/topic-normal.md");
    fs::write(
        &topic_path,
        "---\nid: topic-normal\ntitle: \"Normal Topic\"\ntype: concept\n---\nTopic body\n",
    )
    .unwrap();

    // Run the archive script
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/archive_notes.py");
    let output = Command::new("python3")
        .arg(&script_path)
        .arg("--pkb-root")
        .arg(tmp_pkb.path())
        .arg("--sessions-repo")
        .arg(&sessions_root)
        .output()
        .expect("failed to execute archive script");

    assert!(
        output.status.success(),
        "script failed: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify archived notes moved into sessions repo
    let dest_task = sessions_root.join("archived/tasks/task-to-archive.md");
    let dest_concept = sessions_root.join("archived/concepts/concept-to-archive.md");
    assert!(
        dest_task.exists(),
        "task-to-archive.md must exist in sessions repo under archived/tasks/"
    );
    assert!(
        dest_concept.exists(),
        "concept-to-archive.md must exist in sessions repo under archived/concepts/"
    );

    // Verify archived notes removed from PKB
    assert!(
        !task_archived_path.exists(),
        "task-to-archive.md must be removed from PKB"
    );
    assert!(
        !concept_archived_path.exists(),
        "concept-to-archive.md must be removed from PKB"
    );

    // Verify non-archived notes remain in PKB
    assert!(
        task_active_path.exists(),
        "task-active.md must remain in PKB"
    );
    assert!(topic_path.exists(), "topic-normal.md must remain in PKB");

    // Verify graph reload in PKB: archived notes no longer resolve
    let fresh_graph = GraphStore::build_from_directory(tmp_pkb.path());
    assert!(
        fresh_graph.resolve("task-to-archive").is_none(),
        "task-to-archive must no longer resolve in PKB"
    );
    assert!(
        fresh_graph.resolve("concept-to-archive").is_none(),
        "concept-to-archive must no longer resolve in PKB"
    );
    assert!(
        fresh_graph.resolve("task-active").is_some(),
        "task-active must still resolve in PKB"
    );
    assert!(
        fresh_graph.resolve("topic-normal").is_some(),
        "topic-normal must still resolve in PKB"
    );
}
