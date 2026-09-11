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

/// Regression test for mem_65496f77:
/// PKB MCP writes (`update_body`, `update_task`) must flush synchronously to disk
/// and update in-memory state so that an immediate read-after-write (`get_document`,
/// `get_task`, `list_tasks`) observes the written data without delay.
#[test]
fn test_read_after_write_synchronous_visibility_for_update_body_and_update_task() {
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

    // 1. Create a task via MCP
    let created = server
        .handle_create_task(&json!({
            "title": "Synchronous flush task",
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

    let abs_path = {
        let g = server.graph.read();
        let node = g.resolve(&id).unwrap();
        server.abs_path(&node.path)
    };

    // 2. update_body must be immediately readable from disk and via get_document / get_task
    let updated_body_text = "Updated body content with section header\n\n## Details\nSynchronous flush confirmed.";
    let update_body_res = server
        .handle_update_body(&json!({
            "id": id,
            "new_body": updated_body_text,
        }))
        .unwrap();
    assert!(update_body_res.is_error.is_none() || update_body_res.is_error == Some(false));

    // Immediate on-disk check (file must have been synced synchronously)
    let disk_content = std::fs::read_to_string(&abs_path).expect("read task file from disk");
    assert!(
        disk_content.contains("Synchronous flush confirmed."),
        "disk file must contain updated body immediately after update_body returns: {disk_content}"
    );

    // Immediate get_document check
    let get_doc_res = server.handle_get_document(&json!({"id": id})).unwrap();
    let get_doc_text = get_doc_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        get_doc_text.contains("Synchronous flush confirmed."),
        "get_document must observe updated body immediately: {get_doc_text}"
    );

    // Immediate get_task check
    let get_task_res = server.handle_get_task(&json!({"id": id})).unwrap();
    let get_task_text = get_task_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        get_task_text.contains("Synchronous flush confirmed."),
        "get_task must observe updated body immediately: {get_task_text}"
    );

    // 3. update_task must be immediately readable from disk, get_task, and list_tasks
    let update_task_res = server
        .handle_update_task(&json!({
            "id": id,
            "updates": {
                "status": "done",
                "completion_evidence": "Regression test verified synchronous flush.",
            }
        }))
        .unwrap();
    assert!(update_task_res.is_error.is_none() || update_task_res.is_error == Some(false));

    // Immediate on-disk check for status
    let disk_doc = crate::pkb::parse_file_relative(&abs_path, root).expect("parse relative");
    assert_eq!(
        disk_doc.status.as_deref(),
        Some("done"),
        "disk file status must be 'done' immediately after update_task returns"
    );

    // Immediate get_task check for status
    let get_task_done = server.handle_get_task(&json!({"id": id})).unwrap();
    let get_task_done_json = task_json(&get_task_done);
    assert_eq!(
        get_task_done_json.get("status").and_then(|v| v.as_str()),
        Some("done"),
        "get_task must return status 'done' immediately"
    );

    // Immediate list_tasks check for status
    let list_res = server
        .handle_list_tasks(&json!({
            "status": "done",
            "format": "json",
            "include_done": true,
        }))
        .unwrap();
    let list_json = task_json(&list_res);
    let tasks_arr = list_json
        .get("tasks")
        .and_then(|t| t.as_array())
        .expect("tasks array");
    let found = tasks_arr
        .iter()
        .any(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()));
    assert!(
        found,
        "list_tasks(status=done) must find the task immediately after update_task: {list_json}"
    );
}

/// Regression test for refresh_graph reporting unparseable/skipped files and closing index staleness gap
#[test]
fn test_refresh_graph_closes_disk_gap_and_reports_unparseable_files() {
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

    // Valid task 1
    server
        .handle_create_task(&json!({
            "title": "Task 1",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();

    // Valid task 2 written directly to disk
    std::fs::write(
        root.join("tasks/task-2.md"),
        "---\nid: task-2\ntitle: Task 2\ntype: task\nstatus: ready\nproject: proj-test\n---\n\nBody.\n",
    )
    .unwrap();

    // Call refresh_graph
    let refresh_res = server.handle_refresh_graph(&json!({})).unwrap();
    let refresh_json = task_json(&refresh_res);
    assert_eq!(refresh_json.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        refresh_json.get("scanned_files").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        refresh_json.get("parsed_documents").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        refresh_json.get("unparseable_or_skipped_files").and_then(|v| v.as_u64()),
        Some(0)
    );

    // Verify list_tasks has no staleness warning
    let list_res = server
        .handle_list_tasks(&json!({"format": "json", "include_done": true}))
        .unwrap();
    let list_text = list_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    assert!(
        !list_text.contains("index_warning"),
        "list_tasks must not warn after refresh_graph: {list_text}"
    );
}

/// Read a node's tags directly from the in-memory graph, bypassing
/// `list_tasks`/`ensure_graph_fresh`'s disk-generation self-heal
/// (`GraphStore::generation`/`scan_generation`, see `pkb::scan_generation`
/// callers in `mod.rs`). That self-heal exists to catch exactly this kind
/// of staleness on the *next* read, which is precisely why it must NOT be
/// used to observe the raw, momentary result of the Tier-1/Tier-2 race
/// below: any call that goes through `ensure_graph_fresh` (e.g.
/// `handle_list_tasks`) can silently repair a just-landed stale clobber
/// before the assertion ever sees it, masking the very defect under test.
fn node_tags_raw(server: &PkbSearchServer, id: &str) -> Vec<String> {
    server
        .graph
        .read()
        .nodes_map()
        .get(id)
        .map(|n| n.tags.clone())
        .unwrap_or_default()
}

fn list_tasks_tag_ids(server: &PkbSearchServer, tag: &str) -> Vec<String> {
    let result = server
        .handle_list_tasks(&json!({"tags": [tag], "format": "json"}))
        .unwrap();
    // `handle_list_tasks` renders a plain-text "no results" message instead
    // of `{"tasks": [], ...}` JSON when the filtered set is empty — only
    // parse as JSON when there's a non-empty result set to inspect.
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
        .collect();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => json
            .get("tasks")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// aops_mem_tag_integrity AC2: removing a tag from a file and calling
/// `refresh_graph` must drop that node from the tag's `list_tasks` result
/// set — the tag index (`GraphNode.tags`, read fresh from disk by
/// `refresh_graph`'s full rebuild) must not retain a tag the file no longer
/// carries.
#[test]
fn test_refresh_graph_drops_removed_tag_from_list_tasks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let task_path = root.join("tasks/task-tagged.md");
    std::fs::write(
        &task_path,
        "---\nid: task-tagged\ntitle: Tagged Task\ntype: task\nstatus: ready\nproject: proj-test\ntags:\n  - onlytag\n---\n\nBody.\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    );

    // Sanity: the tag is present before removal.
    let before_ids = list_tasks_tag_ids(&server, "onlytag");
    assert!(
        before_ids.contains(&"task-tagged".to_string()),
        "sanity: task-tagged must be found by its own tag before removal: {before_ids:?}"
    );

    // Remove the tag directly on disk (simulating an external edit/cleanup)
    // and force a full rebuild via the explicit `refresh_graph` escape hatch.
    std::fs::write(
        &task_path,
        "---\nid: task-tagged\ntitle: Tagged Task\ntype: task\nstatus: ready\nproject: proj-test\ntags: []\n---\n\nBody.\n",
    )
    .unwrap();
    server.handle_refresh_graph(&json!({})).unwrap();

    let after_ids = list_tasks_tag_ids(&server, "onlytag");
    assert!(
        !after_ids.contains(&"task-tagged".to_string()),
        "task-tagged must NOT be returned by list_tasks(tags=[onlytag]) after \
         the tag was removed on disk and refresh_graph was called: {after_ids:?}"
    );
}

/// task_af93030b: the single-shot version above (`refresh_graph` with no
/// concurrent activity) cannot fail on the actual defect mechanism this
/// mechanism (`full_rebuild_epoch`) exists to close — a Tier-2 background
/// rebuild computed from a *pre-removal* snapshot swapping in after
/// `refresh_graph`'s fresher disk-truth swap and silently reintroducing the
/// removed tag (the "two `refresh_graph` calls did not clear it" symptom,
/// aops_17c86b89). This variant interleaves a slow Tier-2 rebuild — snapshot
/// taken while the tag is still present — with the tag removal + explicit
/// `refresh_graph`, then checks the raw in-memory graph node once Tier-2 has
/// fully settled. Without the epoch guard (or with the TOCTOU gap
/// task_af93030b fixes), Tier-2's stale swap can land after
/// `refresh_graph`'s and reintroduce "onlytag" into the graph.
///
/// Reads the *raw graph node* (`node_tags_raw`), not `list_tasks`: an
/// earlier version of this test asserted through `list_tasks_tag_ids` and
/// passed on pre-fix code too, even with `pre_epoch_bump_delay_ms` widening
/// the race window to hundreds of milliseconds (confirmed empirically by
/// reverting the fix locally and tracing the interleaving with timestamped
/// debug prints). Root cause: `handle_list_tasks` calls `ensure_graph_fresh`,
/// whose disk-generation self-heal (`scan_generation` vs the cached
/// `GraphStore::generation()`) detects the post-clobber mismatch and
/// silently triggers *another* synchronous `rebuild_graph()` before the
/// assertion ever runs, repairing the very staleness the test exists to
/// catch. Reading the graph directly observes the actual post-race state
/// with no such side channel in the way.
#[tokio::test(flavor = "multi_thread")]
async fn test_refresh_graph_drops_removed_tag_from_list_tasks_under_concurrent_tier2_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    let task_path = root.join("tasks/task-tagged.md");
    std::fs::write(
        &task_path,
        "---\nid: task-tagged\ntitle: Tagged Task\ntype: task\nstatus: ready\nproject: proj-test\ntags:\n  - onlytag\n---\n\nBody.\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let server = Arc::new(PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    ));

    let before_ids = list_tasks_tag_ids(&server, "onlytag");
    assert!(
        before_ids.contains(&"task-tagged".to_string()),
        "sanity: task-tagged must be found by its own tag before removal: {before_ids:?}"
    );

    // Kick off a slow Tier-2 rebuild. It snapshots `nodes_cloned()` NOW —
    // with "onlytag" present — then sleeps for 300ms before its swap.
    server.set_tier2_sleep_ms(300);
    server.schedule_graph_rebuild();

    // While Tier-2 sleeps, remove the tag on disk and force a full rebuild
    // via the explicit refresh_graph path — disk truth the in-flight Tier-2
    // snapshot has no way to know about.
    //
    // Without an injected delay here, the gap between refresh_graph's swap
    // and its epoch bump is microseconds — far smaller than Tier-2's 300ms
    // sleep, so Tier-2's write-lock attempt at ~t=300ms would land long
    // after the bump has already landed regardless of whether the bump is
    // inside or outside the write lock, and this test would pass on both
    // pre-fix and post-fix ordering (verified: it does, 5/5 runs, against a
    // local revert of the fix). `pre_epoch_bump_delay_ms` widens that gap to
    // ~400ms so Tier-2's lock attempt at t=300ms lands *during* the window —
    // on the fix, that's still inside refresh_graph's write lock, so Tier-2
    // blocks until the bump has landed and correctly aborts on stale epoch;
    // on pre-fix ordering, the lock would already be free with the epoch
    // not yet bumped, so Tier-2 would acquire it and clobber the fresh
    // disk-truth swap with its own stale ("onlytag" still present) snapshot.
    server.set_pre_epoch_bump_delay_ms(400);
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    std::fs::write(
        &task_path,
        "---\nid: task-tagged\ntitle: Tagged Task\ntype: task\nstatus: ready\nproject: proj-test\ntags: []\n---\n\nBody.\n",
    )
    .unwrap();
    server.handle_refresh_graph(&json!({})).unwrap();

    // refresh_graph's own swap must be visible in the raw graph immediately
    // it returns — checked directly (see `node_tags_raw` doc comment above)
    // so a concurrent Tier-2 clobber that lands before refresh_graph returns
    // (as it can on pre-fix ordering: Tier-1 releases the write lock, then
    // delays, then bumps — leaving Tier-2 a window to acquire the lock,
    // observe the still-stale epoch, and swap its own pre-removal snapshot
    // in before Tier-1's `rebuild_graph()` call even returns) is visible
    // here rather than silently repaired by a side channel.
    let just_after_refresh_tags = node_tags_raw(&server, "task-tagged");
    assert!(
        !just_after_refresh_tags.iter().any(|t| t == "onlytag"),
        "refresh_graph's own swap must drop the removed tag from the graph \
         immediately, with no window for a concurrent Tier-2 rebuild to \
         clobber it back in before refresh_graph returns: {just_after_refresh_tags:?}"
    );

    // Wait for the in-flight (and any coalesced follow-up) Tier-2 rebuild to
    // fully drain. Generous bound: Tier-2's own bump is also subject to the
    // injected `pre_epoch_bump_delay_ms`, so a full settle can take ~700ms+.
    for _ in 0..120 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if !server.graph_rebuild_pending() {
            break;
        }
    }

    let settled_tags = node_tags_raw(&server, "task-tagged");
    assert!(
        !settled_tags.iter().any(|t| t == "onlytag"),
        "a stale Tier-2 rebuild snapshotted before the tag removal must not \
         resurface 'onlytag' in the graph once it settles, after \
         refresh_graph already swapped in the fresher disk-truth state: \
         {settled_tags:?}"
    );

    // Confirm the served `list_tasks` view agrees with the raw graph once
    // settled (it should, whether by the fix or by `ensure_graph_fresh`'s
    // self-heal on this read) — this is the user-visible surface the
    // original defect (aops_17c86b89) was reported against.
    let settled_ids = list_tasks_tag_ids(&server, "onlytag");
    assert!(
        !settled_ids.contains(&"task-tagged".to_string()),
        "task-tagged must not be served by list_tasks(tags=[onlytag]) once \
         settled: {settled_ids:?}"
    );
}
