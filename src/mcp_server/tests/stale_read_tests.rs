//! Regression tests for aops_fb137646: the served graph can disagree with
//! disk, in both directions, with no warning. These cover AC3 — a by-id
//! read (`get_task`) after a write must return what's on disk, or the
//! in-memory graph must be patched so it does.

use super::*;

fn task_json(result: &CallToolResult) -> serde_json::Value {
    let text = result
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

/// Reproduces the shape of Repro B on aops_fb137646: a task's status
/// changes on disk (here, via a direct write that bypasses the in-place
/// graph patch — standing in for either a same-process patch that failed
/// to land, or a write from a different `pkb mcp` process) while the
/// in-memory graph still holds the old status. A subsequent `get_task`
/// by id must return the disk truth, not the stale cached value.
#[test]
fn test_get_task_self_heals_stale_cached_status_after_direct_disk_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let graph = GraphStore::build(&[], root);
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

    // 1. Create a task through the normal MCP path (patches the in-memory
    // graph as part of create_task).
    let created = server
        .handle_create_task(&json!({
            "title": "Stale read regression",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();
    let id = task_json(&created)
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let initial_status = server
        .graph
        .read()
        .resolve(&id)
        .unwrap()
        .status
        .clone()
        .unwrap_or_default();
    assert_ne!(
        initial_status, "done",
        "task must not start out already 'done', or this test proves nothing"
    );

    // 2. Simulate the defect directly: write status=done to the file on
    // disk WITHOUT going through rebuild_graph_for_pkb_document, so the
    // in-memory graph's cached node.status is now stale relative to disk.
    let abs_path = {
        let g = server.graph.read();
        let node = g.resolve(&id).unwrap();
        server.abs_path(&node.path)
    };
    let mut updates = std::collections::HashMap::new();
    updates.insert(
        "status".to_string(),
        serde_json::Value::String("done".to_string()),
    );
    crate::document_crud::update_document(&abs_path, updates).unwrap();

    // Confirm the in-memory graph is indeed stale at this point — this is
    // the precondition the fix must overcome, not something it prevents.
    let cached_before_heal = server.graph.read().resolve(&id).unwrap().status.clone();
    assert_eq!(
        cached_before_heal.as_deref(),
        Some(initial_status.as_str()),
        "precondition: in-memory graph must still show the pre-write status"
    );

    // 3. A by-id read must return the disk truth, not the stale cache.
    let after = server.handle_get_task(&json!({"id": id})).unwrap();
    let after_val = task_json(&after);
    assert_eq!(
        after_val.get("status").and_then(|v| v.as_str()),
        Some("done"),
        "get_task must serve disk state after a write, even when the cached \
         graph node was never patched: {after_val}"
    );

    // 4. The self-heal must also have propagated into the graph itself, so
    // every other consumer in this process sees the fix too.
    let cached_after_heal = server.graph.read().resolve(&id).unwrap().status.clone();
    assert_eq!(
        cached_after_heal.as_deref(),
        Some("done"),
        "get_task's self-heal must patch the in-memory graph, not just the response"
    );
}

/// A by-id read where the cache is already fresh must not be perturbed by
/// the self-heal check (no spurious patch/log noise on the common path).
#[test]
fn test_get_task_no_op_when_cache_already_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let graph = GraphStore::build(&[], root);
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

    let created = server
        .handle_create_task(&json!({
            "title": "Fresh read control",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();
    let id = task_json(&created)
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let result = server.handle_get_task(&json!({"id": id})).unwrap();
    let val = task_json(&result);
    assert_eq!(val.get("id").and_then(|v| v.as_str()), Some(id.as_str()));
}

/// Reproduces the shape of Repro A on aops_fb137646: a real task file exists
/// on disk but the in-memory graph never observed it (standing in for a
/// file written by a different `pkb mcp` process, or a direct/sync write —
/// nothing this process's incremental patching would have seen). AC2 says
/// this must not be a *silent* short list: `list_tasks` must carry a signal
/// the caller can branch on.
/// Reproduces Repro A on aops_fb137646 and validates mem_3c018681:
/// An untracked file lands on disk (external write from git-sync sidecar, CLI,
/// or direct disk edit). A subsequent `list_tasks` must automatically self-invalidate
/// and return the new file without requiring `refresh_graph`.
#[test]
fn test_list_tasks_self_invalidates_and_returns_untracked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let graph = GraphStore::build(&[], root);
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

    // Seed one task through normal MCP path
    server
        .handle_create_task(&json!({
            "title": "Tracked task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();

    let clean = server
        .handle_list_tasks(&json!({"format": "json", "include_done": true}))
        .unwrap();
    let clean_text = clean
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        clean_text.contains("Tracked task"),
        "initial list must contain tracked task"
    );

    // Sleep briefly so mtime advances
    std::thread::sleep(std::time::Duration::from_millis(20));

    // An untracked file lands on disk
    std::fs::write(
        root.join("tasks/untracked-task.md"),
        "---\nid: untracked-task\ntitle: Untracked\ntype: task\nstatus: in_progress\nproject: proj-test\n---\n\nBody.\n",
    )
    .unwrap();

    // With self-invalidation, list_tasks automatically discovers the new file
    let after = server
        .handle_list_tasks(&json!({"format": "json", "include_done": true}))
        .unwrap();
    let after_text = after
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        after_text.contains("untracked-task") && after_text.contains("Untracked"),
        "list_tasks must automatically self-invalidate and return the untracked file: {after_text}"
    );

    // Markdown format also discovers the new file
    let after_md = server
        .handle_list_tasks(&json!({"format": "markdown", "include_done": true}))
        .unwrap();
    let after_md_text = after_md
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        after_md_text.contains("Untracked") && after_md_text.contains("untracked-task"),
        "markdown list_tasks must return the untracked file: {after_md_text}"
    );
}

/// Tests same-count node swap (delete 1 file, add 1 file) with self-invalidation.
#[test]
fn test_list_tasks_self_invalidates_on_same_count_node_swap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let graph = GraphStore::build(&[], root);
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

    let task1_file = root.join("tasks/task-one.md");
    std::fs::write(
        &task1_file,
        "---\nid: task-one\ntitle: Task One\ntype: task\nstatus: ready\nproject: proj-test\n---\n",
    )
    .unwrap();

    let list1 = server
        .handle_list_tasks(&json!({"format": "json", "include_done": true}))
        .unwrap();
    let list1_text = list1
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(list1_text.contains("task-one"));

    // Sleep so mtime advances
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Delete task-one and create task-two (total count remains 1)
    std::fs::remove_file(&task1_file).unwrap();
    let task2_file = root.join("tasks/task-two.md");
    std::fs::write(
        &task2_file,
        "---\nid: task-two\ntitle: Task Two\ntype: task\nstatus: ready\nproject: proj-test\n---\n",
    )
    .unwrap();

    let list2 = server
        .handle_list_tasks(&json!({"format": "json", "include_done": true}))
        .unwrap();
    let list2_text = list2
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();

    assert!(
        list2_text.contains("task-two"),
        "list_tasks must find newly swapped-in node: {list2_text}"
    );
    assert!(
        !list2_text.contains("task-one"),
        "list_tasks must NOT contain deleted node: {list2_text}"
    );
}

