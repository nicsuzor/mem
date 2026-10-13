//! Tests for removal of the stakeholder-waiting term from the mem ranking engine.
//!
//! Acceptance criteria:
//! 1. Two tasks identical except for `stakeholder` and `waiting_since` receive the same
//!    cost of delay and queue rank.
//! 2. A task under a container whose contribution edge reaches a priced target reads its
//!    class value with or without a stakeholder.
//! 3. `stakeholder` and `waiting_since` still read back from task reads and the graph export.

use mem::graph_store::{ExportGraph, GraphStore};
use serde_json::Value;

#[test]
fn test_identical_tasks_differing_only_by_stakeholder_receive_same_cost_of_delay_and_queue_rank() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    // A reference task with intent 1 (earning cost of delay = 5000)
    std::fs::write(
        root.join("tasks/ref-task.md"),
        "---\nid: ref-task\ntitle: Reference Task\ntype: task\nstatus: ready\nintent: 1\n---\n",
    )
    .unwrap();

    // Task A: ready task, intent 4, created 30 days ago, NO stakeholder, NO waiting_since
    std::fs::write(
        root.join("tasks/task-a.md"),
        "---\nid: task-a\ntitle: Task A without stakeholder\ntype: task\nstatus: ready\nintent: 4\ncreated: 2026-09-01\n---\n",
    )
    .unwrap();

    // Task B: identical to Task A, but WITH stakeholder and waiting_since set
    std::fs::write(
        root.join("tasks/task-b.md"),
        "---\nid: task-b\ntitle: Task B with stakeholder\ntype: task\nstatus: ready\nintent: 4\ncreated: 2026-09-01\nstakeholder: \"Important Partner\"\nwaiting_since: 2026-09-01\n---\n",
    )
    .unwrap();

    // Build Graph A with ref-task and task-a
    let tmp_a = tempfile::tempdir().unwrap();
    let root_a = tmp_a.path();
    std::fs::create_dir_all(root_a.join("tasks")).unwrap();
    std::fs::copy(
        root.join("tasks/ref-task.md"),
        root_a.join("tasks/ref-task.md"),
    )
    .unwrap();
    std::fs::copy(root.join("tasks/task-a.md"), root_a.join("tasks/task-a.md")).unwrap();

    let graph_a = GraphStore::build_from_directory(root_a);
    let json_a = graph_a.output_json_filtered(None, 0, None, false).unwrap();
    let export_a: ExportGraph = serde_json::from_str(&json_a).unwrap();
    let node_a = export_a
        .nodes
        .iter()
        .find(|n| n.node.id == "task-a")
        .unwrap();

    // Build Graph B with ref-task and task-b
    let tmp_b = tempfile::tempdir().unwrap();
    let root_b = tmp_b.path();
    std::fs::create_dir_all(root_b.join("tasks")).unwrap();
    std::fs::copy(
        root.join("tasks/ref-task.md"),
        root_b.join("tasks/ref-task.md"),
    )
    .unwrap();
    std::fs::copy(root.join("tasks/task-b.md"), root_b.join("tasks/task-b.md")).unwrap();

    let graph_b = GraphStore::build_from_directory(root_b);
    let json_b = graph_b.output_json_filtered(None, 0, None, false).unwrap();
    let export_b: ExportGraph = serde_json::from_str(&json_b).unwrap();
    let node_b = export_b
        .nodes
        .iter()
        .find(|n| n.node.id == "task-b")
        .unwrap();

    // 1. Cost of delay must be identical:
    assert_eq!(
        node_a.cost_of_delay,
        node_b.cost_of_delay,
        "Two tasks identical except for stakeholder and waiting_since must receive identical cost of delay: task-a got {:?}, task-b got {:?}",
        node_a.cost_of_delay,
        node_b.cost_of_delay
    );
    assert_eq!(
        node_a.cost_of_delay,
        Some(0),
        "Neither task should receive any stakeholder-waiting points in cost_of_delay"
    );

    // 2. Queue rank must be identical in equivalent graph contexts:
    assert_eq!(
        node_a.queue_rank,
        node_b.queue_rank,
        "Two tasks identical except for stakeholder and waiting_since must receive identical queue rank in equivalent graph contexts: task-a got {:?}, task-b got {:?}",
        node_a.queue_rank,
        node_b.queue_rank
    );
    assert_eq!(
        node_a.queue_rank,
        Some(2),
        "Both tasks must rank at position 2 (after ref-task which has cost_of_delay 5000)"
    );
}

#[test]
fn test_task_under_container_reaching_priced_target_reads_class_value_with_or_without_stakeholder()
{
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("projects")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    // Target priced at standing_weight 0.60
    std::fs::write(
        root.join("targets/target-class.md"),
        "---\nid: target-class\ntitle: Class Target\ntype: target\nstatus: ready\nstanding_weight: 0.60\n---\n",
    )
    .unwrap();

    // Epic container contributing 1.0 to target-class (raw value = 6000)
    std::fs::write(
        root.join("projects/epic-class.md"),
        "---\nid: epic-class\ntitle: Class Epic\ntype: epic\nstatus: ready\ncontributes_to:\n  - to: target-class\n    weight: 1.0\n---\n",
    )
    .unwrap();

    // Task 1: under epic-class, NO stakeholder
    std::fs::write(
        root.join("tasks/task-no-stakeholder.md"),
        "---\nid: task-no-stakeholder\ntitle: Task without stakeholder\ntype: task\nstatus: ready\nparent: epic-class\n---\n",
    )
    .unwrap();

    // Task 2: under epic-class, WITH stakeholder and waiting_since
    std::fs::write(
        root.join("tasks/task-with-stakeholder.md"),
        "---\nid: task-with-stakeholder\ntitle: Task with stakeholder\ntype: task\nstatus: ready\nparent: epic-class\nstakeholder: \"Important Stakeholder\"\nwaiting_since: 2026-09-01\n---\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);
    let json = graph.output_json_filtered(None, 0, None, false).unwrap();
    let export: ExportGraph = serde_json::from_str(&json).unwrap();

    let task_no_sh = export
        .nodes
        .iter()
        .find(|n| n.node.id == "task-no-stakeholder")
        .expect("task-no-stakeholder must be present");
    let task_with_sh = export
        .nodes
        .iter()
        .find(|n| n.node.id == "task-with-stakeholder")
        .expect("task-with-stakeholder must be present");

    // Both tasks share the container's 6000 value equally (3000 each)
    assert_eq!(task_no_sh.node.value_lineage, 3000.0);
    assert_eq!(task_with_sh.node.value_lineage, 3000.0);

    // Both tasks must read strictly their class value (3000) into cost_of_delay,
    // with or without a stakeholder (no stakeholder-waiting bonus added).
    assert_eq!(
        task_no_sh.cost_of_delay,
        Some(3000),
        "task without stakeholder reads class value (3000) into cost_of_delay"
    );
    assert_eq!(
        task_with_sh.cost_of_delay,
        Some(3000),
        "task with stakeholder reads class value (3000) into cost_of_delay without stakeholder additions"
    );
    assert_eq!(
        task_with_sh.cost_of_delay, task_no_sh.cost_of_delay,
        "cost of delay must match between tasks with and without stakeholder"
    );
}

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
