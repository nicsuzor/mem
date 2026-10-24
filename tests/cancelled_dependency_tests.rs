//! Black-box tests for unblocking and readiness when dependencies are cancelled (task mem_f31cd4af).
//!
//! Acceptance criteria:
//! AC2: A task whose only open dependency is cancelled is unblocked and ready
//! in every tool that reports blocked or ready state; one test per tool, red trace quoted.

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::sync::Arc;

fn seed_pkb_with_cancelled_dep() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  aops: {}\n",
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

    let canc_path = root.join("tasks/task-canc.md");
    fs::write(
        &canc_path,
        "---\n\
         id: task-canc\n\
         title: \"Cancelled Dependency\"\n\
         type: task\n\
         status: cancelled\n\
         reason: \"No longer needed\"\n\
         parent: proj-root\n\
         project: aops\n\
         ---\n\n# Cancelled Dependency\n",
    )
    .unwrap();

    let dep_path = root.join("tasks/task-dep.md");
    fs::write(
        &dep_path,
        "---\n\
         id: task-dep\n\
         title: \"Dependent Task\"\n\
         type: task\n\
         status: ready\n\
         parent: proj-root\n\
         project: aops\n\
         depends_on:\n\
           - task-canc\n\
         ---\n\n# Dependent Task\n",
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

#[test]
fn test_get_task_reports_unblocked_and_ready_for_cancelled_dep() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync("get_task", &json!({ "id": "task-dep" }))
        .expect("get_task should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(
        val.get("blocked").and_then(|v| v.as_bool()),
        Some(false),
        "task-dep must not be blocked"
    );

    let display = val.get("display").expect("display object must exist");
    assert_eq!(
        display.get("blocked").and_then(|v| v.as_bool()),
        Some(false),
        "display.blocked must be false"
    );
    assert_eq!(
        display.get("ready").and_then(|v| v.as_bool()),
        Some(true),
        "display.ready must be true for task depending only on cancelled task"
    );
}

#[test]
fn test_list_tasks_reports_unblocked_and_ready_for_cancelled_dep() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync("list_tasks", &json!({ "format": "json" }))
        .expect("list_tasks should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();
    let tasks = val.get("tasks").and_then(|v| v.as_array()).unwrap();

    let task_dep = tasks
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-dep"))
        .expect("task-dep must be in list_tasks");

    assert_eq!(
        task_dep.get("blocked").and_then(|v| v.as_bool()),
        Some(false),
        "task-dep.blocked must be false in list_tasks"
    );
    assert_eq!(
        task_dep.get("ready").and_then(|v| v.as_bool()),
        Some(true),
        "task-dep.ready must be true in list_tasks"
    );

    let rel = task_dep.get("relationship_summary");
    if let Some(rel_obj) = rel {
        assert_eq!(
            rel_obj.get("blocked").and_then(|v| v.as_bool()),
            Some(false),
            "relationship_summary.blocked must be false"
        );
    }
}

#[test]
fn test_nested_tasks_reports_unblocked_for_cancelled_dep() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync("nested_tasks", &json!({ "format": "json" }))
        .expect("nested_tasks should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();

    fn find_in_tree(val: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
        if val.get("id").and_then(|v| v.as_str()) == Some(id) {
            return Some(val.clone());
        }
        if let Some(children) = val.get("children").and_then(|v| v.as_array()) {
            for c in children {
                if let Some(found) = find_in_tree(c, id) {
                    return Some(found);
                }
            }
        }
        None
    }

    let tasks = val.get("tasks").and_then(|v| v.as_array()).expect("tasks array must exist");
    let mut found_node = None;
    for root in tasks {
        if let Some(found) = find_in_tree(root, "task-dep") {
            found_node = Some(found);
            break;
        }
    }
    let node = found_node.expect("task-dep must be in nested_tasks tree");
    assert_eq!(
        node.get("blocked").and_then(|v| v.as_bool()),
        Some(false),
        "task-dep.blocked must be false in nested_tasks"
    );
}

#[test]
fn test_task_summary_counts_cancelled_dep_as_ready_not_blocked() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync("task_summary", &json!({}))
        .expect("task_summary should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(
        val.get("ready").and_then(|v| v.as_u64()),
        Some(1),
        "task_summary ready count must be 1 (task-dep is ready)"
    );
    assert_eq!(
        val.get("blocked").and_then(|v| v.as_u64()),
        Some(0),
        "task_summary blocked count must be 0"
    );
}

#[test]
fn test_export_graph_reports_unblocked_and_ready_for_cancelled_dep() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let res = server
        .dispatch_tool_sync("export_graph", &json!({"format": "json"}))
        .expect("export_graph should succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();
    let nodes = val.get("nodes").and_then(|v| v.as_array()).unwrap();

    let node = nodes
        .iter()
        .find(|n| n.get("id").and_then(|v| v.as_str()) == Some("task-dep"))
        .expect("task-dep must be in export_graph nodes");

    let display = node.get("display").expect("display object must exist");
    assert_eq!(
        display.get("ready").and_then(|v| v.as_bool()),
        Some(true),
        "export_graph display.ready must be true"
    );
    assert_eq!(
        display.get("blocked").and_then(|v| v.as_bool()),
        Some(false),
        "export_graph display.blocked must be false"
    );
}

#[test]
fn test_release_task_to_cancelled_unblocks_dependent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(root.join("polecat.yaml"), "projects:\n  aops: {}\n").unwrap();

    fs::write(
        root.join("projects/proj-root.md"),
        "---\nid: proj-root\ntitle: Root Project\ntype: epic\nstatus: in_progress\nproject: aops\n---\n# Root\n",
    ).unwrap();

    fs::write(
        root.join("tasks/task-blocker.md"),
        "---\nid: task-blocker\ntitle: Blocker Task\ntype: task\nstatus: in_progress\nparent: proj-root\nproject: aops\n---\n# Blocker\n",
    ).unwrap();

    fs::write(
        root.join("tasks/task-dependent.md"),
        "---\nid: task-dependent\ntitle: Dependent Task\ntype: task\nstatus: ready\nparent: proj-root\nproject: aops\ndepends_on:\n  - task-blocker\n---\n# Dependent\n",
    ).unwrap();

    let db_path = root.join("pkb_vectors.bin");
    let (server, graph) = build_server(root.as_path(), &db_path);

    // Verify task-dependent is blocked initially
    {
        let g = graph.read();
        assert!(g.is_blocked("task-dependent"), "must be blocked initially");
    }

    // Release task-blocker as cancelled
    let res = server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-blocker",
                "status": "cancelled",
                "summary": "Dropping this task",
                "reason": "Requirements changed"
            }),
        )
        .expect("release_task to cancelled must succeed");

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();

    let neighborhood = val.get("neighborhood").expect("neighborhood should exist");
    let unblocked = neighborhood
        .get("unblocked")
        .and_then(|v| v.as_array())
        .expect("unblocked array must exist in neighborhood");

    assert!(
        unblocked.iter().any(|u| u.get("id").and_then(|v| v.as_str()) == Some("task-dependent")),
        "releasing task-blocker as cancelled must report task-dependent in neighborhood.unblocked, got: {unblocked:?}"
    );

    // Verify graph state: task-dependent is now unblocked and ready
    let g = graph.read();
    assert!(
        !g.is_blocked("task-dependent"),
        "task-dependent must be unblocked after blocker was cancelled"
    );
    assert!(
        g.is_ready("task-dependent"),
        "task-dependent must be ready after blocker was cancelled"
    );
}

#[test]
fn test_graph_store_ready_and_blocked_queries() {
    let (tmp, db_path) = seed_pkb_with_cancelled_dep();
    let (_server, graph) = build_server(tmp.path(), &db_path);

    let g = graph.read();
    assert!(
        !g.is_blocked("task-dep"),
        "GraphStore::is_blocked must be false for task-dep"
    );
    assert!(
        g.is_ready("task-dep"),
        "GraphStore::is_ready must be true for task-dep"
    );

    let ready_tasks = g.ready_tasks();
    assert!(
        ready_tasks.iter().any(|n| n.id == "task-dep"),
        "ready_tasks() must contain task-dep"
    );

    let blocked_tasks = g.blocked_tasks();
    assert!(
        !blocked_tasks.iter().any(|n| n.id == "task-dep"),
        "blocked_tasks() must not contain task-dep"
    );
}

#[test]
fn test_task_index_build_mcp_index() {
    let (tmp, _db_path) = seed_pkb_with_cancelled_dep();
    let gs = GraphStore::build_from_directory(tmp.path());

    let index = mem::task_index::build_mcp_index(&gs, tmp.path());

    assert!(
        index.ready.iter().any(|id| id == "task-dep"),
        "index.ready must contain task-dep, got: {:?}",
        index.ready
    );
    assert!(
        !index.blocked.iter().any(|id| id == "task-dep"),
        "index.blocked must not contain task-dep, got: {:?}",
        index.blocked
    );
}

#[test]
fn test_transitive_blocking_does_not_propagate_through_cancelled_task() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(root.join("polecat.yaml"), "projects:\n  aops: {}\n").unwrap();

    fs::write(
        root.join("projects/proj-root.md"),
        "---\nid: proj-root\ntitle: Root Project\ntype: epic\nstatus: in_progress\nproject: aops\n---\n# Root\n",
    ).unwrap();

    // task-open is unmet / inbox with no acceptance criteria (so not completed and not ready)
    fs::write(
        root.join("tasks/task-open.md"),
        "---\nid: task-open\ntitle: Open Unmet Blocker\ntype: task\nstatus: inbox\nparent: proj-root\nproject: aops\n---\n# Open\n",
    ).unwrap();

    // task-canc is cancelled, but was downstream of task-open
    fs::write(
        root.join("tasks/task-canc.md"),
        "---\nid: task-canc\ntitle: Cancelled Intermediary\ntype: task\nstatus: cancelled\nreason: \"Scrapped\"\nparent: proj-root\nproject: aops\ndepends_on:\n  - task-open\n---\n# Cancelled\n",
    ).unwrap();

    // task-dep depends on task-canc
    fs::write(
        root.join("tasks/task-dep.md"),
        "---\nid: task-dep\ntitle: Dependent On Cancelled\ntype: task\nstatus: ready\nparent: proj-root\nproject: aops\ndepends_on:\n  - task-canc\n---\n# Dependent\n",
    ).unwrap();

    let db_path = root.join("pkb_vectors.bin");
    let (server, graph) = build_server(root.as_path(), &db_path);

    let g = graph.read();
    assert!(
        !g.is_blocked("task-dep"),
        "transitive blocking from task-open must not pass through cancelled task-canc to task-dep"
    );
    assert!(
        g.is_ready("task-dep"),
        "task-dep must be ready even though task-canc was downstream of an open task"
    );
    drop(g);

    let res = server
        .dispatch_tool_sync("get_task", &json!({ "id": "task-dep" }))
        .expect("get_task should succeed");
    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(val.get("blocked").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(val.get("display").and_then(|d| d.get("ready")).and_then(|v| v.as_bool()), Some(true));
}
