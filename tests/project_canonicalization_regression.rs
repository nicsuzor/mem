//! Regression tests for project canonicalization on write and read paths (mem_55634ea4).
//!
//! Acceptance criteria:
//! 1. `create_task`/`update_task` canonicalize `project` on write.
//! 2. `pkb lint` flags any node whose stored `project` is not the canonical slug (`fm-project-alias`).
//! 3. `list_tasks(project=<alias>)` and `list_tasks(project=<canonical>)` return identical sets at every filter bound.
//! 4. Regression fixture reproduces the split from a node stored under a variant spelling.

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::lint::{lint_directory, write_fixes};
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::sync::Arc;

fn setup_test_pkb() -> (tempfile::TempDir, PkbSearchServer, Arc<RwLock<GraphStore>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("pkb_vectors.bin");

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("epics")).unwrap();
    fs::create_dir_all(root.join("goals")).unwrap();

    // Register aops with aliases academicOps and ao, and other-proj
    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  aops:\n    aliases: [academicOps, acaops]\n  other-proj: {}\nproject_aliases:\n  ao: aops\n  other: other-proj\n",
    )
    .unwrap();

    // Seed a root goal
    fs::write(
        root.join("goals/goal-root.md"),
        "---\nid: goal-root\ntitle: Root Goal\ntype: goal\nstatus: ready\nproject: aops\n---\n\nRoot Goal.\n",
    )
    .unwrap();

    // Seed a root epic container under the goal
    fs::write(
        root.join("epics/epic-12345678.md"),
        "---\nid: epic-12345678\ntitle: Root Epic\ntype: epic\nstatus: ready\nparent: goal-root\nproject: aops\n---\n\nRoot Epic.\n",
    )
    .unwrap();

    let graph_store = GraphStore::build_from_directory(&root);
    let graph = Arc::new(RwLock::new(graph_store));
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());

    let server = PkbSearchServer::new(store, embedder, root.clone(), db_path, graph.clone());
    (tmp, server, graph)
}

#[test]
fn test_create_task_canonicalizes_project_on_write() {
    let (_tmp, server, _graph) = setup_test_pkb();

    // 1. Create task with alias `academicOps`
    let res = server
        .bench_create_task(&json!({
            "title": "Task with academicOps alias",
            "parent": "epic-12345678",
            "project": "academicOps",
        }))
        .unwrap();

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();
    let id = val.get("id").unwrap().as_str().unwrap();

    // ID prefix should be canonical `aops_` or `aops-`
    assert!(
        id.starts_with("aops_") || id.starts_with("aops-"),
        "id should have prefix 'aops_' or 'aops-', got: {id}"
    );

    // Stored task frontmatter must have canonical `project: aops`
    let task_res = server
        .bench_get_task(&json!({"id": id}))
        .unwrap();
    let task_text = task_res.content[0].raw.as_text().unwrap().text.as_str();
    let task_val: serde_json::Value = serde_json::from_str(task_text).unwrap();
    assert_eq!(
        task_val.get("project").and_then(|v| v.as_str()),
        Some("aops"),
        "stored project must be canonical 'aops'"
    );

    // 2. Create task with shorthand alias `ao`
    let res2 = server
        .bench_create_task(&json!({
            "title": "Task with ao alias",
            "parent": "epic-12345678",
            "project": "ao",
        }))
        .unwrap();

    let text2 = res2.content[0].raw.as_text().unwrap().text.as_str();
    let val2: serde_json::Value = serde_json::from_str(text2).unwrap();
    let id2 = val2.get("id").unwrap().as_str().unwrap();
    assert!(id2.starts_with("aops_") || id2.starts_with("aops-"));

    let task_res2 = server
        .bench_get_task(&json!({"id": id2}))
        .unwrap();
    let task_text2 = task_res2.content[0].raw.as_text().unwrap().text.as_str();
    let task_val2: serde_json::Value = serde_json::from_str(task_text2).unwrap();
    assert_eq!(
        task_val2.get("project").and_then(|v| v.as_str()),
        Some("aops"),
        "stored project must be canonical 'aops'"
    );
}

#[test]
fn test_update_task_canonicalizes_project_on_write() {
    let (_tmp, server, _graph) = setup_test_pkb();

    // Create a task with project other-proj
    let res = server
        .bench_create_task(&json!({
            "title": "Task to update",
            "parent": "epic-12345678",
            "project": "other-proj",
        }))
        .unwrap();

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();
    let id = val.get("id").unwrap().as_str().unwrap();

    // Update project using alias `academicOps`
    server
        .bench_update_task(&json!({
            "id": id,
            "project": "academicOps",
        }))
        .unwrap();

    let task_res = server
        .bench_get_task(&json!({"id": id}))
        .unwrap();
    let task_text = task_res.content[0].raw.as_text().unwrap().text.as_str();
    let task_val: serde_json::Value = serde_json::from_str(task_text).unwrap();
    assert_eq!(
        task_val.get("project").and_then(|v| v.as_str()),
        Some("aops"),
        "updated project must be canonicalized to 'aops'"
    );
}

#[test]
fn test_list_tasks_returns_identical_sets_for_alias_and_canonical() {
    let (_tmp, server, _graph) = setup_test_pkb();

    // Create 3 tasks under aops using different alias spellings
    let res1 = server
        .bench_create_task(&json!({
            "title": "Task 1",
            "parent": "epic-12345678",
            "project": "aops",
        }))
        .unwrap();
    let id1 = serde_json::from_str::<serde_json::Value>(res1.content[0].raw.as_text().unwrap().text.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let res2 = server
        .bench_create_task(&json!({
            "title": "Task 2",
            "parent": "epic-12345678",
            "project": "academicOps",
        }))
        .unwrap();
    let id2 = serde_json::from_str::<serde_json::Value>(res2.content[0].raw.as_text().unwrap().text.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let res3 = server
        .bench_create_task(&json!({
            "title": "Task 3",
            "parent": "epic-12345678",
            "project": "ao",
        }))
        .unwrap();
    let id3 = serde_json::from_str::<serde_json::Value>(res3.content[0].raw.as_text().unwrap().text.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Query list_tasks by canonical slug
    let list_canonical = server
        .bench_list_tasks(&json!({
            "project": "aops",
            "format": "json",
        }))
        .unwrap();
    let canonical_val: serde_json::Value =
        serde_json::from_str(list_canonical.content[0].raw.as_text().unwrap().text.as_str()).unwrap();
    let canonical_ids: Vec<String> = canonical_val["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();

    // Query list_tasks by alias academicOps
    let list_alias1 = server
        .bench_list_tasks(&json!({
            "project": "academicOps",
            "format": "json",
        }))
        .unwrap();
    let alias1_val: serde_json::Value =
        serde_json::from_str(list_alias1.content[0].raw.as_text().unwrap().text.as_str()).unwrap();
    let alias1_ids: Vec<String> = alias1_val["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();

    // Query list_tasks by shorthand alias ao
    let list_alias2 = server
        .bench_list_tasks(&json!({
            "project": "ao",
            "format": "json",
        }))
        .unwrap();
    let alias2_val: serde_json::Value =
        serde_json::from_str(list_alias2.content[0].raw.as_text().unwrap().text.as_str()).unwrap();
    let alias2_ids: Vec<String> = alias2_val["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(canonical_ids.len(), 4);
    assert_eq!(canonical_ids, alias1_ids, "canonical and alias academicOps must return identical result sets");
    assert_eq!(canonical_ids, alias2_ids, "canonical and alias ao must return identical result sets");
    assert!(canonical_ids.contains(&id1));
    assert!(canonical_ids.contains(&id2));
    assert!(canonical_ids.contains(&id3));
}

#[test]
fn test_regression_reproduce_split_from_hand_written_variant_and_lint_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("pkb_vectors.bin");

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("epics")).unwrap();
    fs::create_dir_all(root.join("goals")).unwrap();

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  aops:\n    aliases: [academicOps]\n",
    )
    .unwrap();

    fs::write(
        root.join("goals/goal-root.md"),
        "---\nid: goal-root\ntitle: Root Goal\ntype: goal\nstatus: ready\nproject: aops\n---\n\nRoot.\n",
    )
    .unwrap();

    fs::write(
        root.join("epics/epic-12345678.md"),
        "---\nid: epic-12345678\ntitle: Root Epic\ntype: epic\nstatus: ready\nparent: goal-root\nproject: aops\n---\n\nRoot.\n",
    )
    .unwrap();

    // Node 1: stored cleanly as canonical `aops`
    fs::write(
        root.join("tasks/aops-11111111.md"),
        "---\nid: aops-11111111\ntitle: Canonical Task\ntype: task\nstatus: ready\nparent: epic-12345678\nproject: aops\n---\n\nBody.\n\n## Acceptance criteria\n- Verified\n",
    )
    .unwrap();

    // Node 2: simulates legacy/hand-written node stored with variant `academicOps`
    let variant_file = root.join("tasks/aops-22222222.md");
    fs::write(
        &variant_file,
        "---\nid: aops-22222222\ntitle: Variant Task\ntype: task\nstatus: ready\nparent: epic-12345678\nproject: academicOps\n---\n\nBody.\n\n## Acceptance criteria\n- Verified\n",
    )
    .unwrap();

    // 1. Lint detects the non-canonical variant node
    let (results, summary) = lint_directory(&root, false, false);
    assert_eq!(summary.files_with_issues, 1);
    let variant_res = results.iter().find(|r| r.path == variant_file).unwrap();
    assert!(
        variant_res
            .diagnostics
            .iter()
            .any(|d| d.rule == "fm-project-alias"
                && d.message.contains("Project 'academicOps' should be canonical 'aops'")),
        "lint must flag fm-project-alias on the variant node"
    );

    // 2. Lint autofix repairs the variant node
    let (results_fix, _) = lint_directory(&root, true, false);
    let written = write_fixes(&results_fix);
    assert_eq!(written, 1);

    let content_after = fs::read_to_string(&variant_file).unwrap();
    assert!(
        content_after.contains("project: aops"),
        "repaired file must have 'project: aops'"
    );

    // 3. Post-fix lint is completely clean
    let (_, clean_summary) = lint_directory(&root, false, false);
    assert_eq!(clean_summary.files_with_issues, 0);

    // 4. Server build on clean directory returns identical results for all query variants
    let graph_store = GraphStore::build_from_directory(&root);
    let graph = Arc::new(RwLock::new(graph_store));
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());
    let server = PkbSearchServer::new(store, embedder, root, db_path, graph);

    let list_canonical = server
        .bench_list_tasks(&json!({"project": "aops", "format": "json"}))
        .unwrap();
    let list_alias = server
        .bench_list_tasks(&json!({"project": "academicOps", "format": "json"}))
        .unwrap();

    let text_c = list_canonical.content[0].raw.as_text().unwrap().text.as_str();
    let text_a = list_alias.content[0].raw.as_text().unwrap().text.as_str();
    assert_eq!(text_c, text_a, "after repair, both queries return identical task sets");
}
