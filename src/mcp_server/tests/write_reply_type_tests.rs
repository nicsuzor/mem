use super::*;

fn make_server(root: &Path) -> PkbSearchServer {
    write_test_polecat_yaml(root);
    let docs: Vec<_> = crate::pkb::scan_directory_all(root)
        .into_iter()
        .filter_map(|p| crate::pkb::parse_file_relative(&p, root))
        .collect();
    let graph = GraphStore::build(&docs, root);
    let mut store = VectorStore::new(3);
    for doc in &docs {
        let emb = vec![vec![1.0, 0.0, 0.0]];
        let chunks = vec![doc.body.clone()];
        store.insert_precomputed(doc, chunks, emb);
    }
    let embedder = Embedder::new_dummy();
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    )
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    if let Some(ref structured) = res.structured_content {
        return structured.clone();
    }
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

fn text_of(res: &CallToolResult) -> String {
    res.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect()
}

// =========================================================================
// AC1: Non-task writes name the written node's type
// =========================================================================

#[test]
fn test_ac1_create_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());

    let res = server
        .handle_create_document(&json!({
            "title": "Architecture Overview",
            "slug": "arch-overview",
            "type": "note",
            "body": "Overview of system architecture."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Note created:"),
        "create reply text must name node type 'Note', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "create structured reply must carry type: note, got: {j:?}"
    );
}

#[test]
fn test_ac1_update_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("note1.md"),
        "---\nid: note-alpha\ntitle: Alpha Note\ntype: note\nstatus: active\n---\nNote content.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_update_task(&json!({
            "id": "note-alpha",
            "title": "Alpha Note Updated",
            "status": "cancelled"
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Note updated:"),
        "update reply text must name node type 'Note', got: {text}"
    );
    assert!(
        !text.contains("Task updated:"),
        "update reply must not say 'Task updated:' on a note"
    );
    assert!(
        !text.contains("HINT: Use release_task"),
        "update reply must not offer release_task hint on a note"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "update structured reply must carry type: note, got: {j:?}"
    );
}

#[test]
fn test_ac1_append_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("note1.md"),
        "---\nid: note-append\ntitle: Append Note\ntype: note\n---\nInitial content.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_append_to_document(&json!({
            "id": "note-append",
            "content": "Appended insight."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Appended to note:"),
        "append reply text must name 'Appended to note:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "append structured reply must carry type: note, got: {j:?}"
    );
}

#[test]
fn test_ac1_edit_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("note1.md"),
        "---\nid: note-edit\ntitle: Edit Note\ntype: note\n---\nLine to change.\n",
    )
    .unwrap();

    let server = make_server(root);

    let diff = "@@ -1,1 +1,1 @@\n-Line to change.\n+Changed line.\n";
    let res = server
        .handle_edit_body(&json!({
            "id": "note-edit",
            "diff": diff
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Note edited:") || text.contains("\"type\":\"note\""),
        "edit reply text must name node type 'Note' or carry type note in JSON, got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "edit structured reply must carry type: note, got: {j:?}"
    );
}

#[test]
fn test_ac1_delete_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("note1.md"),
        "---\nid: note-del\ntitle: Delete Note\ntype: note\n---\nTo delete.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_delete_document(&json!({
            "id": "note-del"
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Deleted note:"),
        "delete reply text must name 'Deleted note:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "delete structured reply must carry type: note, got: {j:?}"
    );
}

#[test]
fn test_ac1_release_non_task_replies_with_node_type() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("note1.md"),
        "---\nid: note-rel\ntitle: Release Note\ntype: note\nstatus: active\n---\n## Acceptance\n- [x] Done\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_release_task(&json!({
            "id": "note-rel",
            "status": "done",
            "summary": "Completed note review."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Released note:"),
        "release reply text must name 'Released note:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("note"),
        "release structured reply must carry type: note, got: {j:?}"
    );
}

// =========================================================================
// AC2: Task writes still reply as tasks
// =========================================================================

#[test]
fn test_ac2_create_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());

    let res = server
        .handle_create_document(&json!({
            "title": "Build Feature",
            "slug": "build-feat",
            "type": "task",
            "body": "Task body."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Task created:"),
        "create reply text for task must name 'Task created:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "create structured reply for task must carry type: task, got: {j:?}"
    );
}

#[test]
fn test_ac2_update_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(
        tasks_dir.join("task1.md"),
        "---\nid: task-alpha\ntitle: Alpha Task\ntype: task\nstatus: ready\n---\nTask content.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_update_task(&json!({
            "id": "task-alpha",
            "title": "Alpha Task Updated",
            "status": "cancelled"
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Task updated:"),
        "update reply text must name 'Task updated:', got: {text}"
    );
    assert!(
        text.contains("HINT: Use release_task"),
        "update reply on a task must retain the release_task hint"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "update structured reply must carry type: task, got: {j:?}"
    );
}

#[test]
fn test_ac2_append_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(
        tasks_dir.join("task1.md"),
        "---\nid: task-append\ntitle: Append Task\ntype: task\n---\nInitial content.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_append_to_document(&json!({
            "id": "task-append",
            "content": "Appended progress."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Appended to task:"),
        "append reply text must name 'Appended to task:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "append structured reply must carry type: task, got: {j:?}"
    );
}

#[test]
fn test_ac2_edit_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(
        tasks_dir.join("task1.md"),
        "---\nid: task-edit\ntitle: Edit Task\ntype: task\n---\nTask line to change.\n",
    )
    .unwrap();

    let server = make_server(root);

    let diff = "@@ -1,1 +1,1 @@\n-Task line to change.\n+Changed task line.\n";
    let res = server
        .handle_edit_body(&json!({
            "id": "task-edit",
            "diff": diff
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Task edited:") || text.contains("\"type\":\"task\""),
        "edit reply text must name 'Task edited:' or carry type task in JSON, got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "edit structured reply must carry type: task, got: {j:?}"
    );
}

#[test]
fn test_ac2_delete_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(
        tasks_dir.join("task1.md"),
        "---\nid: task-del\ntitle: Delete Task\ntype: task\n---\nTo delete.\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_delete_document(&json!({
            "id": "task-del"
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Deleted task:"),
        "delete reply text must name 'Deleted task:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "delete structured reply must carry type: task, got: {j:?}"
    );
}

#[test]
fn test_ac2_release_task_replies_as_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(
        tasks_dir.join("task1.md"),
        "---\nid: task-rel\ntitle: Release Task\ntype: task\nstatus: ready\n---\n## Acceptance\n- [x] Done\n",
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_release_task(&json!({
            "id": "task-rel",
            "status": "done",
            "summary": "Completed task."
        }))
        .unwrap();

    let text = text_of(&res);
    let j = json_of(&res);

    assert!(
        text.contains("Released task:"),
        "release reply text must name 'Released task:', got: {text}"
    );
    assert_eq!(
        j.get("type").and_then(|v| v.as_str()),
        Some("task"),
        "release structured reply must carry type: task, got: {j:?}"
    );
}
