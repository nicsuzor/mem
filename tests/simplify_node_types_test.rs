//! Dedicated integration tests verifying the collapse and simplification of PKB node types
//! and contributes_to float multiplier propagation (mem_5c476567).

use mem::graph::{is_valid_node_type, ContributesTo, GraphNode, VALID_NODE_TYPES};
use mem::graph_store::GraphStore;
use mem::lint::lint_directory;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use mem::embeddings::Embedder;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::sync::Arc;

fn parse_test_node(content: &str, filename: &str) -> (tempfile::TempDir, GraphNode) {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join(filename);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, content).unwrap();
    let doc = mem::pkb::parse_file(&file_path).expect("failed to parse test document");
    let node = GraphNode::from_pkb_document(&doc);
    (tmp, node)
}

#[test]
fn test_node_types_collapsed_and_capability_wound_back() {
    // 1. Valid node types must contain task and target, but NOT epic, goal, or capability
    assert!(VALID_NODE_TYPES.contains(&"task"));
    assert!(VALID_NODE_TYPES.contains(&"target"));
    assert!(!VALID_NODE_TYPES.contains(&"capability"));
    assert!(!VALID_NODE_TYPES.contains(&"epic"));
    assert!(!VALID_NODE_TYPES.contains(&"goal"));

    assert!(is_valid_node_type("task"));
    assert!(is_valid_node_type("target"));
    assert!(!is_valid_node_type("capability"));
    assert!(!is_valid_node_type("epic"));
    assert!(!is_valid_node_type("goal"));
}

#[test]
fn test_read_coercion_for_legacy_types() {
    // 2. Legacy types are coerced on read
    let capability_doc = r#"---
id: cap_test123
title: Legacy Capability
type: capability
status: active
---
Legacy capability body.
"#;
    let (_tmp, node_cap) = parse_test_node(capability_doc, "targets/cap_test123.md");
    assert_eq!(node_cap.node_type.as_deref(), Some("target"));
    assert_eq!(node_cap.raw_node_type.as_deref(), Some("capability"));

    let goal_doc = r#"---
id: goal_test123
title: Legacy Goal
type: goal
status: active
---
Legacy goal body.
"#;
    let (_tmp, node_goal) = parse_test_node(goal_doc, "goals/goal_test123.md");
    assert_eq!(node_goal.node_type.as_deref(), Some("target"));
    assert_eq!(node_goal.raw_node_type.as_deref(), Some("goal"));

    let epic_doc = r#"---
id: epic_test123
title: Legacy Epic
type: epic
status: ready
---
Legacy epic body.
"#;
    let (_tmp, node_epic) = parse_test_node(epic_doc, "epics/epic_test123.md");
    assert_eq!(node_epic.node_type.as_deref(), Some("task"));
    assert_eq!(node_epic.raw_node_type.as_deref(), Some("epic"));

    let project_doc = r#"---
id: proj_test123
title: Legacy Project
type: project
status: ready
---
Legacy project body.
"#;
    let (_tmp, node_proj) = parse_test_node(project_doc, "projects/proj_test123.md");
    assert_eq!(node_proj.node_type.as_deref(), Some("task"));
    assert_eq!(node_proj.raw_node_type.as_deref(), Some("project"));
}

#[test]
fn test_target_accepts_empty_differing_fields() {
    // 3. Targets accept empty severity, consequence, and due
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("targets")).unwrap();

    let target_file = root.join("targets/targ_minimal.md");
    let content = r#"---
id: targ_minimal
title: Minimal Target
type: target
status: ready
---
A target without severity, consequence, or due.
"#;
    fs::write(&target_file, content).unwrap();

    let (results, _summary) = lint_directory(&root, false, false);
    let target_res = results.iter().find(|r| r.path == target_file);
    // There should be no errors or warnings regarding missing severity, consequence, or due
    if let Some(res) = target_res {
        for diag in &res.diagnostics {
            assert!(
                !diag.rule.contains("severity") && !diag.rule.contains("consequence") && !diag.rule.contains("due"),
                "Target must accept empty severity, consequence, and due. Found diagnostic: {:?}",
                diag
            );
        }
    }
}

#[test]
fn test_root_level_task_accepts_empty_parent() {
    // 4. Root-level task accepts empty parent without warnings or schema errors
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("pkb_vectors.bin");
    fs::create_dir_all(root.join("tasks")).unwrap();

    let graph_store = GraphStore::build_from_directory(&root);
    let graph = Arc::new(RwLock::new(graph_store));
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());

    let server = PkbSearchServer::new(store, embedder, root.clone(), db_path, graph.clone());

    let res = server
        .bench_create_task(&json!({
            "title": "Standalone Root Task",
            "type": "task"
        }))
        .unwrap();

    let text = res.content[0].raw.as_text().unwrap().text.as_str();
    let val: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(val["status"], "inbox");
    let id = val["id"].as_str().unwrap();

    // Verify task was written without a parent
    let task_res = server.bench_get_task(&json!({"id": id})).unwrap();
    let task_text = task_res.content[0].raw.as_text().unwrap().text.as_str();
    let task_val: serde_json::Value = serde_json::from_str(task_text).unwrap();
    assert!(task_val.get("parent").is_none() || task_val["parent"].is_null());
}

#[test]
fn test_contributes_to_float_multiplier_and_weight_propagation() {
    // 5. Test ContributesTo float multiplier
    let c_verbal: ContributesTo = serde_json::from_value(json!({
        "target": "targ_001",
        "weight": "probable",
        "multiplier": 0.5
    }))
    .unwrap();
    // probable = 0.85; 0.85 * 0.5 = 0.425
    assert!((c_verbal.numeric_weight() - 0.425).abs() < 1e-6);

    // Multiplier alias "x"
    let c_alias_x: ContributesTo = serde_json::from_value(json!({
        "target": "targ_001",
        "weight": "fifty-fifty",
        "x": 2.0
    }))
    .unwrap();
    // fifty-fifty = 0.5; 0.5 * 2.0 = 1.0
    assert!((c_alias_x.numeric_weight() - 1.0).abs() < 1e-6);

    // Float weight with multiplier
    let c_float_with_mult: ContributesTo = serde_json::from_value(json!({
        "target": "targ_001",
        "weight": "0.75",
        "multiplier": 0.5
    }))
    .unwrap();
    // 0.75 * 0.5 = 0.375
    assert!((c_float_with_mult.numeric_weight() - 0.375).abs() < 1e-6);

    // Multiplier alone without stated weight
    let c_mult_only: ContributesTo = serde_json::from_value(json!({
        "target": "targ_001",
        "x": 0.6
    }))
    .unwrap();
    assert!((c_mult_only.numeric_weight() - 0.6).abs() < 1e-6);
}

#[test]
fn test_contributes_to_multiplier_graph_store_propagation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join("targets")).unwrap();
    fs::create_dir_all(root.join("tasks")).unwrap();

    // Target targ_001
    fs::write(
        root.join("targets/targ_001.md"),
        "---\nid: targ_001\ntitle: Goal Target\ntype: target\nstatus: ready\nstanding_weight: 1.0\n---\nTarget.\n",
    )
    .unwrap();

    // Task 1: multiplier 0.5
    fs::write(
        root.join("tasks/task_mult_half.md"),
        "---\nid: task_mult_half\ntitle: Half Multiplier Task\ntype: task\nstatus: ready\ncontributes_to:\n  - target: targ_001\n    weight: \"fifty-fifty\"\n    multiplier: 0.5\n---\n## Acceptance criteria\n- Done\n",
    )
    .unwrap();

    // Task 2: multiplier 1.0 (default)
    fs::write(
        root.join("tasks/task_mult_one.md"),
        "---\nid: task_mult_one\ntitle: Full Multiplier Task\ntype: task\nstatus: ready\ncontributes_to:\n  - target: targ_001\n    weight: \"fifty-fifty\"\n---\n## Acceptance criteria\n- Done\n",
    )
    .unwrap();

    let store = GraphStore::build_from_directory(&root);
    let target = store.get_node("targ_001").expect("target must exist");
    assert!(target.downstream_weight > 0.0);

    let task_half = store.get_node("task_mult_half").unwrap();
    let task_one = store.get_node("task_mult_one").unwrap();
    // task_one has double the value lineage of task_half because of the 0.5 multiplier (0.5 vs 0.25 effective weight)
    assert!(task_half.value_lineage > 0.0);
    assert!((task_one.value_lineage - 2.0 * task_half.value_lineage).abs() < 1e-4);
}

