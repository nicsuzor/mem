use super::*;
use serde_json::json;

fn setup_test_server() -> (PkbSearchServer, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.keep();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(&root);

    let graph = GraphStore::build(&[], &root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root,
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let get_val = |res: &CallToolResult| -> serde_json::Value {
        let text = res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        serde_json::from_str(&text).expect("Valid JSON response")
    };
    let get_id = |val: &serde_json::Value| -> String {
        val.get("id").and_then(|v| v.as_str()).unwrap().to_string()
    };

    let val_a = get_val(&server.handle_create_task(&json!({
        "slug": "task-a",
        "title": "Task A",
        "type": "task",
        "status": "ready",
        "allow_missing_parent": true,
    })).unwrap());
    let id_a = get_id(&val_a);

    let val_b = get_val(&server.handle_create_task(&json!({
        "slug": "task-b",
        "title": "Task B",
        "type": "task",
        "status": "ready",
        "allow_missing_parent": true,
    })).unwrap());
    let id_b = get_id(&val_b);

    (server, id_a, id_b)
}

fn parse_result(res: &CallToolResult) -> serde_json::Value {
    let text = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    serde_json::from_str(&text).expect("Valid JSON response")
}

fn wait_for_tier2(server: &PkbSearchServer) {
    for _ in 0..100 {
        if !server.graph_rebuild_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Helper to run the removal test for a specific update payload
fn run_dependency_removal_test<F>(build_payload: F)
where
    F: FnOnce(&str, &str) -> serde_json::Value,
{
    let (server, id_a, id_b) = setup_test_server();

    // Add hard dependency: B depends on A
    server
        .handle_update_task(&json!({
            "id": id_b.clone(),
            "updates": {
                "depends_on": [id_a.clone()]
            }
        }))
        .unwrap();

    // Verify initial blocked state
    let rec_b = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    assert_eq!(rec_b["blocked"], true, "task B must be blocked initially");
    let rec_a = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let a_blocks: Vec<String> = rec_a
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(a_blocks.contains(&id_b), "task A blocks must contain B initially");

    let list_all_init = parse_result(&server.handle_list_tasks(&json!({"format": "json"})).unwrap());
    let tasks_init = list_all_init["tasks"].as_array().unwrap();
    let b_init = tasks_init.iter().find(|t| t["id"] == id_b).unwrap();
    assert_eq!(b_init["blocked"], true, "task B must have blocked: true initially in list_tasks");
    assert_eq!(b_init["ready"], false, "task B must have ready: false initially in list_tasks");
    assert!(server.graph.read().is_blocked(&id_b), "graph.is_blocked(&id_b) must be true initially");
    assert!(!server.graph.read().is_ready(&id_b), "graph.is_ready(&id_b) must be false initially");

    // Remove dependency using the provided payload
    let update_payload = build_payload(&id_a, &id_b);
    let _update_res = server.handle_update_task(&update_payload).unwrap();

    // ── AC1 & AC2: Straight after write ──
    let rec_b_after = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_deps: Vec<String> = rec_b_after
        .get("depends_on")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !b_deps.contains(&id_a),
        "straight after write: task B depends_on must not contain A, got: {:?}",
        b_deps
    );
    assert_eq!(
        rec_b_after["blocked"], false,
        "straight after write: task B blocked must be false"
    );

    let rec_a_after = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let a_blocks_after: Vec<String> = rec_a_after
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !a_blocks_after.contains(&id_b),
        "straight after write: task A blocks must not contain B, got: {:?}",
        a_blocks_after
    );

    // AC2: task listings straight after write
    let list_all = parse_result(&server.handle_list_tasks(&json!({"format": "json"})).unwrap());
    let tasks = list_all["tasks"].as_array().unwrap();
    let b_in_list = tasks.iter().find(|t| t["id"] == id_b).unwrap();
    assert_eq!(
        b_in_list["blocked"], false,
        "straight after write: task B in list_tasks must have blocked: false"
    );
    assert_eq!(
        b_in_list["ready"], true,
        "straight after write: task B in list_tasks must have ready: true"
    );
    assert!(
        !server.graph.read().is_blocked(&id_b),
        "straight after write: graph.is_blocked(&id_b) must be false"
    );
    assert!(
        server.graph.read().is_ready(&id_b),
        "straight after write: graph.is_ready(&id_b) must be true"
    );

    // ── AC1 & AC2: After background rebuild completes ──
    wait_for_tier2(&server);

    let rec_b_tier2 = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_deps_tier2: Vec<String> = rec_b_tier2
        .get("depends_on")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !b_deps_tier2.contains(&id_a),
        "after background rebuild: task B depends_on must not contain A, got: {:?}",
        b_deps_tier2
    );
    assert_eq!(
        rec_b_tier2["blocked"], false,
        "after background rebuild: task B blocked must be false"
    );

    let rec_a_tier2 = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let a_blocks_tier2: Vec<String> = rec_a_tier2
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !a_blocks_tier2.contains(&id_b),
        "after background rebuild: task A blocks must not contain B, got: {:?}",
        a_blocks_tier2
    );

    // AC2: task listings after background rebuild
    let list_all_tier2 = parse_result(&server.handle_list_tasks(&json!({"format": "json"})).unwrap());
    let tasks_tier2 = list_all_tier2["tasks"].as_array().unwrap();
    let b_in_list_tier2 = tasks_tier2.iter().find(|t| t["id"] == id_b).unwrap();
    assert_eq!(
        b_in_list_tier2["blocked"], false,
        "after background rebuild: task B in list_tasks must have blocked: false"
    );
    assert_eq!(
        b_in_list_tier2["ready"], true,
        "after background rebuild: task B in list_tasks must have ready: true"
    );
    assert!(
        !server.graph.read().is_blocked(&id_b),
        "after background rebuild: graph.is_blocked(&id_b) must be false"
    );
    assert!(
        server.graph.read().is_ready(&id_b),
        "after background rebuild: graph.is_ready(&id_b) must be true"
    );
}

#[test]
fn test_depends_on_removal_empty_array() {
    run_dependency_removal_test(|_id_a, id_b| {
        json!({
            "id": id_b,
            "updates": {
                "depends_on": []
            }
        })
    });
}

#[test]
fn test_depends_on_removal_null() {
    run_dependency_removal_test(|_id_a, id_b| {
        json!({
            "id": id_b,
            "updates": {
                "depends_on": null
            }
        })
    });
}

#[test]
fn test_depends_on_removal_special_key() {
    run_dependency_removal_test(|id_a, id_b| {
        json!({
            "id": id_b,
            "updates": {
                "_remove_depends_on": [id_a]
            }
        })
    });
}

#[test]
fn test_soft_depends_on_removal_empty_array() {
    let (server, id_a, id_b) = setup_test_server();
    server
        .handle_update_task(&json!({
            "id": id_b.clone(),
            "updates": {
                "soft_depends_on": [id_a.clone()]
            }
        }))
        .unwrap();

    let rec_a = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let _a_soft_blocks: Vec<String> = rec_a["frontmatter"]["soft_blocks"]
        .as_array()
        .or_else(|| rec_a["soft_blocks"].as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().or_else(|| v.get("id").and_then(|s| s.as_str())).map(String::from)).collect())
        .unwrap_or_default();

    // Verify soft dependency can be removed with empty array
    server
        .handle_update_task(&json!({
            "id": id_b.clone(),
            "updates": {
                "soft_depends_on": []
            }
        }))
        .unwrap();

    let rec_b_after = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_sdep: Vec<String> = rec_b_after["frontmatter"]["soft_depends_on"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(b_sdep.is_empty(), "straight after write: soft_depends_on should be empty");

    let _rec_a_after = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let a_node_sb_after = server.graph.read().get_node(&id_a).unwrap().soft_blocks.clone();
    assert!(
        !a_node_sb_after.contains(&id_b),
        "straight after write: task A soft_blocks must not contain B, got: {:?}",
        a_node_sb_after
    );

    wait_for_tier2(&server);
    let rec_b_tier2 = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_sdep_tier2: Vec<String> = rec_b_tier2["frontmatter"]["soft_depends_on"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(b_sdep_tier2.is_empty(), "after tier2: soft_depends_on should remain empty");

    let a_node_sb_tier2 = server.graph.read().get_node(&id_a).unwrap().soft_blocks.clone();
    assert!(
        !a_node_sb_tier2.contains(&id_b),
        "after tier2: task A soft_blocks must not contain B, got: {:?}",
        a_node_sb_tier2
    );
}

#[test]
fn test_soft_depends_on_removal_null() {
    let (server, id_a, id_b) = setup_test_server();
    server
        .handle_update_task(&json!({
            "id": id_b.clone(),
            "updates": {
                "soft_depends_on": [id_a.clone()]
            }
        }))
        .unwrap();

    server
        .handle_update_task(&json!({
            "id": id_b.clone(),
            "updates": {
                "soft_depends_on": null
            }
        }))
        .unwrap();

    let rec_b_after = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_sdep: Vec<String> = rec_b_after["frontmatter"]["soft_depends_on"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(b_sdep.is_empty(), "straight after write: soft_depends_on should be empty");

    let _rec_a_after = parse_result(&server.handle_get_task(&json!({"id": id_a.clone()})).unwrap());
    let a_node_sb_after = server.graph.read().get_node(&id_a).unwrap().soft_blocks.clone();
    assert!(
        !a_node_sb_after.contains(&id_b),
        "straight after write: task A soft_blocks must not contain B, got: {:?}",
        a_node_sb_after
    );

    wait_for_tier2(&server);
    let rec_b_tier2 = parse_result(&server.handle_get_task(&json!({"id": id_b.clone()})).unwrap());
    let b_sdep_tier2: Vec<String> = rec_b_tier2["frontmatter"]["soft_depends_on"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(b_sdep_tier2.is_empty(), "after tier2: soft_depends_on should remain empty");

    let a_node_sb_tier2 = server.graph.read().get_node(&id_a).unwrap().soft_blocks.clone();
    assert!(
        !a_node_sb_tier2.contains(&id_b),
        "after tier2: task A soft_blocks must not contain B, got: {:?}",
        a_node_sb_tier2
    );
}
