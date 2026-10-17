//! Tests that the `export_graph` MCP tool is registered with the schema the
//! `pkb__export_graph` MCP tool doc promises (params, read-only annotation)
//! and that dispatch handles both "dot" and "json" formats.

use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;

#[test]
fn test_export_graph_tool_is_registered_read_only_with_expected_params() {
    let tools = PkbSearchServer::get_all_tools();
    let tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "export_graph")
        .expect("export_graph tool must be registered");

    assert!(
        tool.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false),
        "export_graph must be marked read_only_hint: true"
    );

    let desc = tool.description.as_deref().unwrap_or("").to_lowercase();
    assert!(desc.contains("dot"), "description must mention DOT: {desc}");
    assert!(desc.contains("digraph"), "description must mention digraph syntax: {desc}");

    let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
    for param in ["format", "focus", "max_depth", "project", "include_done"] {
        assert!(
            schema_str.contains(param),
            "export_graph schema must declare param '{param}', got: {schema_str}"
        );
    }

    // graph_json must be removed completely from get_all_tools
    assert!(
        !tools.iter().any(|t| t.name.as_ref() == "graph_json"),
        "graph_json must not be registered"
    );
}

#[test]
fn test_export_graph_dispatch_dot_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let task1_file = root.join("task1.md");
    std::fs::write(
        &task1_file,
        "---\nid: task_1\ntitle: Active Task\ntype: task\nstatus: in_progress\nproject: myproj\n---\nBody",
    )
    .unwrap();
    let task2_file = root.join("task2.md");
    std::fs::write(
        &task2_file,
        "---\nid: task_2\ntitle: Done Task\ntype: task\nstatus: done\nproject: myproj\n---\nBody",
    )
    .unwrap();

    let doc1 = mem::pkb::parse_file_relative(&task1_file, root).unwrap();
    let doc2 = mem::pkb::parse_file_relative(&task2_file, root).unwrap();

    let graph = GraphStore::build(&[doc1, doc2], root);
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // 1. Default format (dot)
    let res_default = server
        .dispatch_tool_sync("export_graph", &serde_json::json!({}))
        .unwrap();
    let text_default: String = res_default
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    assert!(text_default.starts_with("digraph PKB {"));

    // 2. Format "dot" explicitly
    let res_dot = server
        .dispatch_tool_sync("export_graph", &serde_json::json!({"format": "dot"}))
        .unwrap();
    let text_dot: String = res_dot
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    assert!(text_dot.starts_with("digraph PKB {"));

    // 3. Format "json" with default include_done: false
    let res_json = server
        .dispatch_tool_sync("export_graph", &serde_json::json!({"format": "json"}))
        .unwrap();
    let text_json: String = res_json
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text_json).unwrap();
    // Default include_done: false excludes done tasks
    assert_eq!(parsed["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["nodes"][0]["id"], "task_1");

    // 4. Format "json" with include_done: true
    let res_json_all = server
        .dispatch_tool_sync(
            "export_graph",
            &serde_json::json!({"format": "json", "include_done": true}),
        )
        .unwrap();
    let text_json_all: String = res_json_all
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed_all: Value = serde_json::from_str(&text_json_all).unwrap();
    assert_eq!(parsed_all["nodes"].as_array().unwrap().len(), 2);

    // 5. Invalid format returns error
    let res_err = server.dispatch_tool_sync("export_graph", &serde_json::json!({"format": "xml"}));
    assert!(res_err.is_err());
}

/// `export_graph` JSON carries the flow engine's ranking per node, so
/// consumers (the overwhelm dashboard's `/api/graph`) never re-derive it:
/// `display_rank` is the node's 1-based position under `display_cmp` (the
/// comparator `list_tasks` sorts by), and no retired ranking field is emitted.
#[test]
fn test_export_graph_json_emits_display_rank_and_no_retired_ranking_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let w = |name: &str, body: &str| std::fs::write(root.join(name), body).unwrap();

    w(
        "targ_x.md",
        "---\nid: targ_x\ntitle: Priced Target\ntype: target\nstatus: active\nworth: 1.0\n---\nT\n",
    );
    w(
        "t_valued.md",
        "---\nid: t_valued\ntitle: Valued\ntype: task\nstatus: ready\nlinks:\n  - to: targ_x\n    label: serves\n    quantum: 0.5\n---\nB\n",
    );
    w("t_plain.md", "---\nid: t_plain\ntitle: Plain\ntype: task\nstatus: ready\n---\nB\n");
    w("t_other.md", "---\nid: t_other\ntitle: Other\ntype: task\nstatus: in_progress\n---\nB\n");
    w("t_done.md", "---\nid: t_done\ntitle: Done\ntype: task\nstatus: done\n---\nB\n");

    let graph = GraphStore::build_from_directory(&root);
    let shared = Arc::new(RwLock::new(graph));
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(mem::vectordb::VectorStore::new(3))),
        Arc::new(mem::embeddings::Embedder::new_dummy()),
        root.clone(),
        root.join("db.bin"),
        shared.clone(),
    );
    let res = server
        .dispatch_tool_sync(
            "export_graph",
            &serde_json::json!({"format": "json", "include_done": true}),
        )
        .unwrap();
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(parsed.get("focus").is_none(), "retired `focus` list must not be emitted");
    let nodes = parsed["nodes"].as_array().unwrap();
    let by_id = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap_or_else(|| panic!("{id} missing"));

    let retired = [
        "focus_score",
        "focus_tuple",
        "cost_of_delay",
        "severity_gate",
        "queue_rank",
        "effective_intent",
        "downstream_weight",
        "stakeholder_exposure",
        "criticality",
        "urgency",
        "chain_slack",
        "unlock_breadth",
        "value_lineage",
        "voi_value",
        "affordable_loss_filtered",
    ];
    for n in nodes {
        for key in retired {
            assert!(n.get(key).is_none(), "retired field `{key}` emitted: {n}");
        }
    }

    // display_rank is exactly the engine's display order, 1..N contiguous.
    let g = shared.read();
    let mut ranked: Vec<&mem::graph::GraphNode> = nodes
        .iter()
        .filter_map(|n| g.get_node(n["id"].as_str().unwrap()))
        .collect();
    GraphStore::sort_by_display(
        &mut ranked,
        chrono::Utc::now().date_naive(),
        mem::display_rank::DISPLAY_CLIFF_BUFFER_DAYS,
    );
    assert!(ranked.len() >= 4, "fixture should rank several nodes");
    for (i, n) in ranked.iter().enumerate() {
        assert_eq!(
            by_id(&n.id)["display_rank"].as_u64(),
            Some(i as u64 + 1),
            "{} rank must match engine display order",
            n.id
        );
    }
}
