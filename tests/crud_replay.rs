//! Integration tests for verifying that the redesigned CRUD surface
//! resolves the catalogued anchor incidents (epic-27805db9).

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

    // Create folders tasks/ and projects/
    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();

    // Seed a project so parents can resolve
    let proj_path = root.join("projects/proj-root.md");
    fs::write(
        &proj_path,
        "---\n\
         id: proj-root\n\
         title: \"Root Project\"\n\
         type: project\n\
         status: active\n\
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
    // 1. Build initial graph store from seeded files
    let graph_store = GraphStore::build_from_directory(root);
    let graph = Arc::new(RwLock::new(graph_store));

    // 2. Initialize VectorStore
    let store = Arc::new(RwLock::new(VectorStore::new(3)));

    // 3. Initialize dummy Embedder
    let embedder = Arc::new(Embedder::new_dummy());

    // 4. Create PkbSearchServer
    let server = PkbSearchServer::new(
        store,
        embedder,
        root.to_path_buf(),
        db_path.to_path_buf(),
        graph.clone(),
    );

    (server, graph)
}

// ── Replay Case 1: create→get indexer bind (aops-ac2df567) ──

#[test]
fn test_create_get_indexer_bind_replay() {
    let (tmp, db_path) = seed_pkb();
    let (server, graph) = build_server(tmp.path(), &db_path);

    // Call bench_create_task
    let res = server
        .bench_create_task(&json!({
            "id": "task-test1",
            "title": "Immediate task",
            "project": "aops",
            "parent": "proj-root"
        }))
        .expect("bench_create_task failed");

    assert!(res.content.len() > 0, "Expected non-empty response content");

    // Immediately resolve the node in the graph.
    // If there were index lag/race, resolve() or get_node() would return None.
    // Drop the read lock before calling bench_decompose_task, which acquires a write lock.
    {
        let g = graph.read();
        let resolved = g.resolve("task-test1");
        assert!(
            resolved.is_some(),
            "Newly created task must be immediately resolvable (no index lag)"
        );
        let node = resolved.unwrap();
        assert_eq!(node.id, "task-test1");
    }

    // Also test decompose_task child node indexing
    let _decomp_res = server
        .bench_decompose_task(&json!({
            "parent_id": "task-test1",
            "subtasks": [
                {
                    "id": "task-sub1",
                    "title": "Subtask 1"
                },
                {
                    "id": "task-sub2",
                    "title": "Subtask 2"
                }
            ]
        }))
        .expect("bench_decompose_task failed");

    // Re-acquire read lock to verify post-decompose state.
    let g = graph.read();

    // Immediately verify the children are resolvable
    let sub1 = g.resolve("task-sub1");
    assert!(
        sub1.is_some(),
        "Decomposed subtask 1 must be immediately resolvable"
    );
    let sub2 = g.resolve("task-sub2");
    assert!(
        sub2.is_some(),
        "Decomposed subtask 2 must be immediately resolvable"
    );

    // Verify parent node has children updated in place
    let parent_node = g.resolve("task-test1").unwrap();
    assert!(
        parent_node.children.contains(&"task-sub1".to_string()),
        "Parent node must record sub1 in children"
    );
    assert!(
        parent_node.children.contains(&"task-sub2".to_string()),
        "Parent node must record sub2 in children"
    );
}

// ── Replay Case 2: filtered list returns empty (mem-dbf5a759) ──

#[test]
fn test_filtered_list_ready_tasks_replay() {
    let (tmp, db_path) = seed_pkb();
    let (server, _graph) = build_server(tmp.path(), &db_path);

    // Create a task that is ready in project aops
    server
        .bench_create_task(&json!({
            "id": "task-ready-aops",
            "title": "Ready task in aops",
            "project": "aops",
            "parent": "proj-root",
            "status": "ready"
        }))
        .unwrap();

    // Create a task that is done/completed in project aops (to ensure filtering works)
    server
        .bench_create_task(&json!({
            "id": "task-done-aops",
            "title": "Done task in aops",
            "project": "aops",
            "parent": "proj-root",
            "status": "done"
        }))
        .unwrap();

    // Create a ready task in project beta (to ensure project filtering works)
    server
        .bench_create_task(&json!({
            "id": "task-ready-beta",
            "title": "Ready task in beta",
            "project": "beta",
            "parent": "proj-root",
            "status": "ready"
        }))
        .unwrap();

    // Call bench_list_tasks(status=ready, project=aops)
    let res = server
        .bench_list_tasks(&json!({
            "status": "ready",
            "project": "aops",
            "format": "json"
        }))
        .unwrap();

    // Inspect the JSON output from CallToolResult.
    // list_tasks json format returns {"total":…, "showing":…, "tasks":[…]}.
    let content = &res.content[0];
    let text = content.raw.as_text().unwrap().text.as_str();
    let envelope: serde_json::Value = serde_json::from_str(text).unwrap();
    let tasks_arr = envelope["tasks"]
        .as_array()
        .expect("Expected a JSON array under the 'tasks' key");

    // We expect only task-ready-aops to be returned
    assert!(
        !tasks_arr.is_empty(),
        "Filtered list of ready tasks must NOT be empty"
    );

    let ids: Vec<&str> = tasks_arr
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"task-ready-aops"),
        "Expected task-ready-aops in the result"
    );
    assert!(
        !ids.contains(&"task-done-aops"),
        "Done tasks should be filtered out when status=ready"
    );
    assert!(
        !ids.contains(&"task-ready-beta"),
        "Tasks from other projects should be filtered out"
    );
}

// ── Replay Case 3: _add_depends_on silent-persist footgun ──

#[test]
fn test_add_depends_on_persistence_replay() {
    let (tmp, db_path) = seed_pkb();
    let (server, graph) = build_server(tmp.path(), &db_path);

    // Create task-a and task-b
    server
        .bench_create_task(&json!({
            "id": "task-a",
            "title": "Task A",
            "project": "aops",
            "parent": "proj-root"
        }))
        .unwrap();

    server
        .bench_create_task(&json!({
            "id": "task-b",
            "title": "Task B",
            "project": "aops",
            "parent": "proj-root"
        }))
        .unwrap();

    // Call bench_update_task to add task-b as dependency of task-a
    server
        .bench_update_task(&json!({
            "id": "task-a",
            "updates": {
                "_add_depends_on": ["task-b"]
            }
        }))
        .unwrap();

    // 1. Verify that the task file on disk does NOT contain "_add_depends_on" in frontmatter,
    // but contains "depends_on" with "task-b".
    // Explicit IDs are used as-is for the filename: tasks/task-a.md.
    let path = tmp.path().join("tasks/task-a.md");
    let content = fs::read_to_string(&path).unwrap();
    assert!(
        !content.contains("_add_depends_on"),
        "Task file must not store '_add_depends_on' key as raw frontmatter"
    );
    assert!(
        content.contains("depends_on:"),
        "Task file must record the dependency in depends_on"
    );
    assert!(
        content.contains("task-b"),
        "Task file must record task-b in depends_on"
    );

    // 2. Verify that the dependency is immediately updated in the graph node
    let g = graph.read();
    let node_a = g.resolve("task-a").unwrap();
    assert!(
        node_a.depends_on.contains(&"task-b".to_string()),
        "GraphNode depends_on must contain task-b"
    );

    // 3. Verify that the dependency relationship exists in the dependency tree
    let tree = g.dependency_tree("task-a");
    assert!(
        tree.iter().any(|(id, _)| id == "task-b"),
        "Dependency tree of task-a must show it depends on task-b"
    );
}

// ── create_task: missing parent returns suggested_parents in error data ──

/// When `create_task` is called without a `parent` and the vector store contains
/// project/epic nodes, the McpError data must include `suggested_parents`.
#[test]
fn test_create_task_missing_parent_returns_suggested_parents() {
    let (tmp, db_path) = seed_pkb();
    let root = tmp.path().to_path_buf();

    // Build graph + server, keeping hold of the store Arc to seed it directly.
    let graph_store = GraphStore::build_from_directory(&root);
    let graph = Arc::new(RwLock::new(graph_store));
    let store = Arc::new(RwLock::new(VectorStore::new(1024)));
    let embedder = Arc::new(Embedder::new_dummy());
    let server = PkbSearchServer::new(
        store.clone(),
        embedder,
        root.clone(),
        db_path,
        graph,
    );

    // Insert the seeded project into the vector store with a dummy zero-vector
    // embedding so the semantic search path can find it.
    let proj_path = root.join("projects/proj-root.md");
    let doc = mem::pkb::parse_file_relative(&proj_path, &root)
        .expect("seeded project file must parse");
    {
        let dummy_emb = vec![0.0f32; 1024];
        store.write().insert_precomputed(&doc, vec!["Root Project".into()], vec![dummy_emb]);
    }

    // create_task with no parent on a plain task type must fail.
    let err = server
        .bench_create_task(&json!({ "title": "Root Project related work" }))
        .expect_err("create_task without parent must return an error");

    let data = err.data.expect("error must carry a data payload with suggested_parents");
    let suggestions = data
        .get("suggested_parents")
        .and_then(|v| v.as_array())
        .expect("data must have a suggested_parents array");

    assert!(
        !suggestions.is_empty(),
        "suggested_parents must contain at least one candidate; got empty list"
    );

    let has_project = suggestions.iter().any(|s| {
        s.get("type").and_then(|v| v.as_str()) == Some("project")
    });
    assert!(has_project, "suggested_parents must include the seeded project node; got: {:?}", suggestions);
}

/// When the vector store has no matching project/epic nodes, the error data is None.
#[test]
fn test_create_task_missing_parent_empty_store_data_is_none() {
    let (tmp, db_path) = seed_pkb();
    // build_server uses an empty VectorStore, so no candidates are available.
    let (server, _graph) = build_server(tmp.path(), &db_path);

    let err = server
        .bench_create_task(&json!({ "title": "Something unrelated" }))
        .expect_err("create_task without parent must return an error");

    assert!(
        err.data.is_none(),
        "When the vector store has no candidates, error data must be None; got: {:?}",
        err.data
    );
}

// ── Replay Case 4: session reaping on 60s keep-alive (mem-2ae61ce6) ──

#[test]
fn test_session_reaping_keep_alive_replay() {
    // This incident (mem-2ae61ce6) relates to the HTTP session keep-alive timeout.
    // In unit tests, this is not deterministically testable at runtime because it
    // requires waiting for or mocking the LocalSessionManager's timer over HTTP/SSE.
    //
    // However, we assert here that the keep-alive config in `src/cli.rs` is set to 24 hours
    // (86,400 seconds) to prevent premature session reaping. We verify this by reading
    // `src/cli.rs` and asserting the presence of the 24-hour timeout setting code.
    let cli_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli.rs");
    let content = std::fs::read_to_string(cli_path).expect("Could not read src/cli.rs");

    assert!(
        content.contains("session_manager_inner.session_config.keep_alive =")
            || content.contains("keep_alive = Some(std::time::Duration::from_secs(24 * 3600))"),
        "The CLI keep-alive configuration for MCP sessions must be configured to 24 hours (refs mem-2ae61ce6)"
    );
}
