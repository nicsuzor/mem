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

/// `export_graph` JSON carries the engine's own ranking per node, so consumers
/// (the overwhelm dashboard's `/api/graph`) never re-derive it:
/// `cost_of_delay` and `severity_gate` are the node's `FocusTuple` components,
/// and `queue_rank` is its 1-based position under the canonical focus order
/// (`GraphStore::focus_cmp`, the comparator `list_tasks` sorts by).
#[test]
fn test_export_graph_json_emits_engine_queue_rank_and_cost_of_delay() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let w = |name: &str, body: &str| std::fs::write(root.join(name), body).unwrap();

    w(
        "targ_x.md",
        "---\nid: targ_x\ntitle: Priced Target\ntype: target\nstatus: active\nstanding_weight: 1.0\n---\nT\n",
    );
    // Identical except that `t_valued` contributes to a priced target and so
    // carries value_lineage; the engine must rank it above `t_plain`.
    w(
        "t_valued.md",
        "---\nid: t_valued\ntitle: Valued\ntype: task\nstatus: ready\ncontributes_to:\n  - target: targ_x\n    weight: \"fifty-fifty\"\n---\nB\n",
    );
    w("t_plain.md", "---\nid: t_plain\ntitle: Plain\ntype: task\nstatus: ready\n---\nB\n");
    w("t_other.md", "---\nid: t_other\ntitle: Other\ntype: task\nstatus: in_progress\n---\nB\n");
    w("t_done.md", "---\nid: t_done\ntitle: Done\ntype: task\nstatus: done\n---\nB\n");

    let graph = GraphStore::build_from_directory(&root);

    // Engine-side expectations, read from the engine rather than pasted.
    let valued = graph.get_node("t_valued").unwrap();
    assert!(valued.value_lineage > 0.0, "fixture must give t_valued value_lineage");
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
    let nodes = parsed["nodes"].as_array().unwrap();
    let by_id = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap_or_else(|| panic!("{id} missing"));

    let g = shared.read();

    // cost_of_delay / severity_gate mirror the engine's FocusTuple exactly.
    for n in nodes {
        let id = n["id"].as_str().unwrap();
        match g.get_node(id).unwrap().focus_tuple.as_ref() {
            Some(ft) => {
                assert_eq!(n["cost_of_delay"].as_i64(), Some(ft.cost_of_delay), "{id} cost_of_delay");
                assert_eq!(
                    n["severity_gate"],
                    serde_json::to_value(ft.severity_gate).unwrap(),
                    "{id} severity_gate"
                );
                assert!(n["queue_rank"].is_u64(), "{id} must carry queue_rank: {n}");
            }
            None => {
                assert!(n.get("cost_of_delay").is_none(), "{id}: no tuple => no cost_of_delay");
                assert!(n.get("queue_rank").is_none(), "{id}: no tuple => no queue_rank");
                assert!(n.get("severity_gate").is_none(), "{id}: no tuple => no severity_gate");
            }
        }
    }
    assert!(by_id("t_done").get("queue_rank").is_none());

    // queue_rank is exactly the engine's canonical focus order, 1..N contiguous.
    let mut ranked: Vec<&mem::graph::GraphNode> = nodes
        .iter()
        .filter_map(|n| g.get_node(n["id"].as_str().unwrap()))
        .filter(|n| n.focus_tuple.is_some())
        .collect();
    GraphStore::sort_by_focus(&mut ranked);
    assert!(ranked.len() >= 3, "fixture should rank several nodes");
    for (i, n) in ranked.iter().enumerate() {
        assert_eq!(
            by_id(&n.id)["queue_rank"].as_u64(),
            Some(i as u64 + 1),
            "{} rank must match engine order",
            n.id
        );
    }

    // Behavioural anchor: value lineage lifts an otherwise-equal task.
    let rv = by_id("t_valued")["queue_rank"].as_u64().unwrap();
    let rp = by_id("t_plain")["queue_rank"].as_u64().unwrap();
    assert!(rv < rp, "t_valued (rank {rv}) must outrank t_plain (rank {rp})");
    assert!(
        by_id("t_valued")["cost_of_delay"].as_i64().unwrap()
            > by_id("t_plain")["cost_of_delay"].as_i64().unwrap()
    );
}
