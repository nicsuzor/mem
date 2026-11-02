use parking_lot::RwLock;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::pkb::parse_file_relative;
use mem::vectordb::VectorStore;

#[test]
fn test_ac1_node_serving_priced_target_shows_nonzero_gain_and_explanation() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let target_file = root.join("targets").join("targ_res_govt_relationships.md");
    let target_content = r#"---
id: targ_res_govt_relationships
title: "Audience: governments and regulators"
type: target
worth: 0.15
---

Body.
"#;
    std::fs::write(&target_file, target_content).unwrap();

    let node_file = root.join("targets").join("targ_aud_reg_australia.md");
    let node_content = r#"---
id: targ_aud_reg_australia
title: "Audience: Australian regulators and government"
type: target
links:
  - to: targ_res_govt_relationships
    label: serves
    quantum: 0.3
    probability: 0.75
    effect: helps
    set_by: agent-proposed
---

Body.
"#;
    std::fs::write(&node_file, node_content).unwrap();

    let doc1 = parse_file_relative(&target_file, root).unwrap();
    let doc2 = parse_file_relative(&node_file, root).unwrap();
    let graph = GraphStore::build(&[doc1, doc2.clone()], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "targ_aud_reg_australia"}))
        .expect("get_task succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    let gain = parsed["flow"]["gain"]
        .as_f64()
        .expect("gain should be present");
    assert!(gain > 0.0, "gain must be non-zero, got {gain}");

    let explanation = parsed["explanation"]
        .as_str()
        .expect("explanation should be present");
    assert!(
        explanation.contains("targ_aud_reg_australia")
            && explanation.contains("targ_res_govt_relationships"),
        "explanation must name the path, got: {explanation}"
    );

    // Also test dynamically updating a task to add a serves link to a priced target
    // Create task_dyn via tool
    server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "id": "task_dyn",
                "slug": "dynamic-task",
                "title": "Dynamic Task",
                "type": "task",
                "status": "ready"
            }),
        )
        .expect("create_task succeeds");

    // Now update task_dyn with links to targ_res_govt_relationships
    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "task_dyn",
                "links": [
                    {
                        "to": "targ_res_govt_relationships",
                        "label": "serves",
                        "quantum": 0.3,
                        "probability": 0.75,
                        "set_by": "agent-proposed"
                    }
                ]
            }),
        )
        .expect("update_task succeeds");

    let res_dyn = server
        .dispatch_tool_sync("get_task", &json!({"id": "task_dyn"}))
        .expect("get_task succeeds");
    let text_dyn: String = res_dyn
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed_dyn: Value = serde_json::from_str(&text_dyn).expect("valid json output");

    let gain_dyn = parsed_dyn["flow"]["gain"].as_f64().unwrap_or(0.0);
    assert!(
        gain_dyn > 0.0,
        "dynamically updated task must have non-zero gain, got {gain_dyn}"
    );
    let expl_dyn = parsed_dyn["explanation"].as_str().unwrap_or("");
    assert!(
        expl_dyn.contains("task_dyn") && expl_dyn.contains("targ_res_govt_relationships"),
        "explanation must name the path, got: {expl_dyn}"
    );
}

#[test]
fn test_ac2_stale_or_disagreeing_input_makes_flow_status_non_ok() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let target_file = root.join("targets").join("targ_res.md");
    std::fs::write(
        &target_file,
        "---\nid: targ_res\ntitle: Target Res\ntype: target\nstatus: inbox\nworth: 0.50\n---\n",
    )
    .unwrap();

    let task_file = root.join("tasks").join("task_a.md");
    std::fs::write(
        &task_file,
        "---\nid: task_a\ntitle: Task A\ntype: task\nstatus: ready\nlinks:\n  - to: targ_res\n    label: serves\n    quantum: 0.5\n    probability: 0.8\n    effect: helps\n---\n",
    )
    .unwrap();

    let doc1 = parse_file_relative(&target_file, root).unwrap();
    let doc2 = parse_file_relative(&task_file, root).unwrap();
    let graph = GraphStore::build(&[doc1, doc2.clone()], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Modify target on disk so task_a's flow input is stale
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        &target_file,
        "---\nid: targ_res\ntitle: Target Res Modified\ntype: target\nstatus: inbox\nworth: 0.80\n---\n",
    )
    .unwrap();

    // 1. Check stale input makes flow_status non-ok
    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "task_a"}))
        .expect("get_task succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    let flow_status = parsed["flow"]["flow_status"]
        .as_str()
        .expect("flow_status string");
    assert_ne!(
        flow_status, "ok",
        "stale inputs must make flow_status non-ok, got {flow_status}"
    );
}

#[test]
fn test_ac3_status_lists_stale_documents_when_not_fresh() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    let task_file = root.join("tasks").join("task_stale.md");
    std::fs::write(
        &task_file,
        "---\nid: task_stale\ntitle: Stale Task\ntype: task\nstatus: ready\n---\n",
    )
    .unwrap();

    let doc = parse_file_relative(&task_file, root).unwrap();
    let graph = GraphStore::build(&[doc], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let res = server
        .dispatch_tool_sync("status", &json!({}))
        .expect("status succeeds");
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).expect("valid json output");

    assert_eq!(parsed["freshness"]["is_fresh"], false);
    // When is_fresh is false, the status must list the stale documents
    let stale_list = parsed["freshness"]["stale_document_paths"]
        .as_array()
        .or_else(|| parsed["freshness"]["stale_document_names"].as_array())
        .or_else(|| parsed["freshness"]["stale_documents_list"].as_array())
        .expect("status must list the stale documents when is_fresh is false");
    assert!(
        !stale_list.is_empty(),
        "stale documents list must not be empty"
    );
    assert!(
        stale_list
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("task_stale")),
        "stale list must contain task_stale"
    );
}
