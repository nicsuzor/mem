//! `stakeholder` and `waiting_since` are informational fields, not ranking
//! inputs: they still read back from task reads and the graph export.

use mem::graph_store::{ExportGraph, GraphStore};
use serde_json::Value;

#[test]
fn test_stakeholder_and_waiting_since_still_read_back_from_task_reads_and_graph_export() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    std::fs::write(
        root.join("tasks/task-info.md"),
        "---\nid: task-info\ntitle: Task with Info Fields\ntype: task\nstatus: ready\nstakeholder: \"Dr. Jane Doe\"\nwaiting_since: 2026-08-15\n---\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);

    // 1. Verify read back from graph node (task reads):
    let node = graph
        .get_node("task-info")
        .expect("task-info must exist in graph");
    assert_eq!(node.stakeholder.as_deref(), Some("Dr. Jane Doe"));
    assert_eq!(node.waiting_since.as_deref(), Some("2026-08-15"));

    // 2. Verify read back from graph export JSON:
    let json_str = graph.output_json_filtered(None, 0, None, false).unwrap();
    let export_graph: ExportGraph = serde_json::from_str(&json_str).unwrap();
    let export_node = export_graph
        .nodes
        .iter()
        .find(|n| n.node.id == "task-info")
        .expect("task-info must exist in export graph");

    assert_eq!(
        export_node.node.stakeholder.as_deref(),
        Some("Dr. Jane Doe")
    );
    assert_eq!(
        export_node.node.waiting_since.as_deref(),
        Some("2026-08-15")
    );

    // 3. Raw JSON value inspection to ensure keys exist in serialized payload:
    let raw_val: Value = serde_json::from_str(&json_str).unwrap();
    let raw_node = raw_val["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "task-info")
        .unwrap();

    assert_eq!(raw_node["stakeholder"], "Dr. Jane Doe");
    assert_eq!(raw_node["waiting_since"], "2026-08-15");
}
