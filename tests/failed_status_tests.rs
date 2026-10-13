//! Tests for canonical task status `failed` (task mem_d1b403ee).
//!
//! Acceptance criteria:
//! 1. `failed` is a canonical status everywhere the server defines, validates,
//!    lints, orders, lists or renders statuses, and is treated as terminal.
//! 2. Tests prove a task can be created, updated and released with `status: failed`,
//!    and that it leaves active listings.

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::sync::Arc;

fn seed_pkb() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    fs::create_dir_all(root.join("tasks")).unwrap();
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

fn build_server(
    root: &std::path::Path,
    db_path: &std::path::Path,
) -> (PkbSearchServer, Arc<RwLock<GraphStore>>) {
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

fn extract_task_ids_from_tool_result(res: &rmcp::model::CallToolResult) -> Vec<String> {
    let text = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(tasks) = val.get("tasks").and_then(|t| t.as_array()) {
            return tasks
                .iter()
                .filter_map(|t| t.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect();
        }
    }
    vec![]
}

fn find_task_file(root: &std::path::Path, task_id: &str) -> std::path::PathBuf {
    mem::pkb::scan_directory(root)
        .into_iter()
        .find(|p| p.file_name().map(|n| n.to_string_lossy().starts_with(task_id)).unwrap_or(false))
        .unwrap_or_else(|| panic!("task file for {task_id} not found under {root:?}"))
}

#[test]
fn test_failed_status_canonical_definitions() {
    assert!(mem::graph::is_valid_status("failed"), "failed must be a valid status");
    assert!(mem::graph::VALID_STATUSES.contains(&"failed"), "failed must be in VALID_STATUSES");
    assert!(mem::graph::COMPLETED_STATUSES.contains(&"failed"), "failed must be in COMPLETED_STATUSES");
    assert!(mem::graph::FAILURE_HANDBACK_STATUSES.contains(&"failed"), "failed must be in FAILURE_HANDBACK_STATUSES");
    assert!(mem::graph::is_completed(Some("failed")), "failed must be completed");
    assert!(mem::graph::is_closed_for_hierarchy(Some("failed")), "failed must be closed for hierarchy");
    assert_eq!(mem::graph::status_group(Some("failed")), "completed");
    assert_eq!(mem::graph::status_rank("failed"), -1);
    assert_eq!(mem::graph::resolve_status_alias("failed"), "failed");
    assert_eq!(mem::graph::status_to_flow_state(Some("failed")), mem::graph::FlowState::Gone);
    assert_eq!(mem::flow::status_to_flow_state(Some("failed")), mem::flow::FlowState::Gone);
}

#[test]
fn test_task_created_with_status_failed_leaves_active_listings() {
    let (tmp, db_path) = seed_pkb();
    let (server, graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "title": "Created directly as failed",
                "parent": "proj-root",
                "project": "aops",
                "status": "failed"
            }),
        )
        .expect("create_task with status=failed should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let created: serde_json::Value = serde_json::from_str(text).unwrap();
    let task_id = created.get("id").and_then(|v| v.as_str()).unwrap();
    assert_eq!(created.get("status").and_then(|v| v.as_str()), Some("failed"));

    // Verify disk content has status: failed
    let path = find_task_file(tmp.path(), task_id);
    let disk = fs::read_to_string(&path).unwrap();
    assert!(disk.contains("status: failed"), "Disk must have status: failed, got: {disk}");

    // Verify it leaves active listings (list_tasks without include_done=true)
    let list_res = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json" }))
        .expect("list_tasks should succeed");
    let ids = extract_task_ids_from_tool_result(&list_res);
    assert!(
        !ids.contains(&task_id.to_string()),
        "failed task {task_id} must not appear in default list_tasks"
    );

    // Verify it appears when include_done=true
    let list_done_res = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json", "include_done": true }))
        .expect("list_tasks with include_done should succeed");
    let done_ids = extract_task_ids_from_tool_result(&list_done_res);
    assert!(
        done_ids.contains(&task_id.to_string()),
        "failed task {task_id} must appear in list_tasks when include_done=true"
    );

    // Verify it appears when status=failed is explicitly queried
    let list_filter_res = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json", "status": "failed" }))
        .expect("list_tasks with status=failed should succeed");
    let filter_ids = extract_task_ids_from_tool_result(&list_filter_res);
    assert!(
        filter_ids.contains(&task_id.to_string()),
        "failed task {task_id} must appear in list_tasks when status=failed"
    );

    // Verify graph_store all_tasks excludes it
    let g = graph.read();
    let all_tasks = g.all_tasks();
    assert!(
        !all_tasks.iter().any(|n| n.id == task_id),
        "failed task {task_id} must not be in g.all_tasks()"
    );
}

#[test]
fn test_task_updated_to_status_failed_leaves_active_listings() {
    let (tmp, db_path) = seed_pkb();
    let (server, graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "title": "Task to update to failed",
                "parent": "proj-root",
                "project": "aops",
                "status": "ready"
            }),
        )
        .expect("create_task should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let created: serde_json::Value = serde_json::from_str(text).unwrap();
    let task_id = created.get("id").and_then(|v| v.as_str()).unwrap();

    // Verify it initially appears in active list_tasks
    let list_init = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json" }))
        .unwrap();
    let init_ids = extract_task_ids_from_tool_result(&list_init);
    assert!(
        init_ids.contains(&task_id.to_string()),
        "initially ready task must appear in list_tasks"
    );

    // Update status to failed
    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": task_id,
                "status": "failed"
            }),
        )
        .expect("update_task with status=failed must succeed");

    // Verify disk content
    let path = find_task_file(tmp.path(), task_id);
    let disk = fs::read_to_string(&path).unwrap();
    assert!(disk.contains("status: failed"), "Disk must have status: failed, got: {disk}");

    // Verify it leaves active listings
    let list_after = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json" }))
        .unwrap();
    let after_ids = extract_task_ids_from_tool_result(&list_after);
    assert!(
        !after_ids.contains(&task_id.to_string()),
        "failed task {task_id} must leave default list_tasks after update"
    );

    // Verify graph_store all_tasks excludes it
    let g = graph.read();
    assert!(
        !g.all_tasks().iter().any(|n| n.id == task_id),
        "failed task {task_id} must not be in g.all_tasks()"
    );
}

#[test]
fn test_task_released_with_status_failed_leaves_active_listings() {
    let (tmp, db_path) = seed_pkb();
    let (server, graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "title": "Task to release as failed",
                "parent": "proj-root",
                "project": "aops",
                "status": "in_progress"
            }),
        )
        .expect("create_task should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let created: serde_json::Value = serde_json::from_str(text).unwrap();
    let task_id = created.get("id").and_then(|v| v.as_str()).unwrap();

    // 1. Releasing as failed without a reason MUST fail (failure-reason-mandatory contract)
    let err = server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": task_id,
                "status": "failed",
                "summary": "Tried and failed"
            }),
        )
        .expect_err("releasing as failed with no reason must fail");
    assert!(
        err.message.to_lowercase().contains("reason"),
        "error message must mention reason: {}",
        err.message
    );

    // 2. Releasing as failed with reason MUST succeed
    let release_res = server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": task_id,
                "status": "failed",
                "summary": "Attempted work but failed due to unsolvable upstream constraints",
                "reason": "Upstream service was deprecated and no alternative exists"
            }),
        )
        .expect("releasing as failed with reason must succeed");

    let release_text = release_res.content[0].raw.as_text().unwrap().text.as_str();
    let release_json: serde_json::Value = serde_json::from_str(release_text).unwrap();
    assert_eq!(release_json.get("status").and_then(|v| v.as_str()), Some("failed"));

    // 3. Verify disk content
    let path = find_task_file(tmp.path(), task_id);
    let disk = fs::read_to_string(&path).unwrap();
    assert!(disk.contains("status: failed"), "Disk must have status: failed, got: {disk}");
    assert!(disk.contains("reason: Upstream service was deprecated and no alternative exists"), "Disk must contain reason: {disk}");

    // 4. Verify it leaves active listings
    let list_res = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json" }))
        .unwrap();
    let after_ids = extract_task_ids_from_tool_result(&list_res);
    assert!(
        !after_ids.contains(&task_id.to_string()),
        "released failed task {task_id} must leave default list_tasks"
    );

    let g = graph.read();
    assert!(
        !g.all_tasks().iter().any(|n| n.id == task_id),
        "released failed task {task_id} must not be in g.all_tasks()"
    );
    drop(g);

    // 5. Verify task cannot be released again (already terminal)
    let re_release_err = server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": task_id,
                "status": "done",
                "summary": "Trying to complete already failed task"
            }),
        )
        .expect_err("re-releasing terminal failed task must be rejected");
    assert!(
        re_release_err.message.contains("already"),
        "re-release error must indicate task is already terminal: {}",
        re_release_err.message
    );
}
