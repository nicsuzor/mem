//! Tests proving that focus picks no longer depend on status `review`, gate
//! tags, or title prefixes, and that a `review` task is still never claimable
//! as ready work (task mem_7f20951e).
//!
//! Acceptance criteria:
//! 1. Human gate paths and every caller's dependence on them are gone.
//! 2. Tests prove focus picks no longer depend on status `review`,
//!    gate tags or title prefixes, and that a `review` task is still never claimable as ready work.
//! 3. Test suite and lint pass; PR open against main.

use chrono::Utc;
use mem::graph_store::GraphStore;
use std::fs;

#[test]
fn test_focus_picks_and_flow_focus_picks_exclude_review_and_gate_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("tasks")).unwrap();

    // 1. Task in review status with gate tags and title prefix
    fs::write(
        root.join("tasks/review-gate.md"),
        "---\nid: review-gate\ntitle: \"DECISION (Nic): approve release\"\ntype: task\nstatus: review\ntags:\n  - decision\n  - human-approval\n---\n",
    ).unwrap();

    // 2. Task in paused status with gate tag and sign-off title prefix
    fs::write(
        root.join("tasks/paused-gate.md"),
        "---\nid: paused-gate\ntitle: \"SIGN-OFF: security review\"\ntype: task\nstatus: paused\ntags:\n  - sign-off\n---\n",
    ).unwrap();

    // 3. Task in inbox status (no AC) with gate tag and decision title prefix
    fs::write(
        root.join("tasks/inbox-gate.md"),
        "---\nid: inbox-gate\ntitle: \"DECISION: database migration plan\"\ntype: task\nstatus: inbox\ntags:\n  - decision\n  - one-way-door\n---\n",
    ).unwrap();

    // 4. Blocked task with gate tag and title prefix
    fs::write(
        root.join("tasks/blocked-gate.md"),
        "---\nid: blocked-gate\ntitle: \"SIGN-OFF: deploy to cluster\"\ntype: task\nstatus: ready\ndepends_on:\n  - non-existent-dep\ntags:\n  - signoff\n---\n",
    ).unwrap();

    // 5. Claimable ready work
    fs::write(
        root.join("tasks/ready-work.md"),
        "---\nid: ready-work\ntitle: \"Implement background metrics export\"\ntype: task\nstatus: ready\n---\n",
    ).unwrap();

    let graph = GraphStore::build_from_directory(root);

    // Standard focus_picks must only surface the ready task, never review or non-ready gate tasks
    let picks = graph.focus_picks(10);
    assert!(
        picks.contains(&"ready-work".to_string()),
        "ready work must appear in focus_picks, got: {picks:?}"
    );
    assert!(
        !picks.contains(&"review-gate".to_string()),
        "review task with gate markers must NOT appear in focus_picks, got: {picks:?}"
    );
    assert!(
        !picks.contains(&"paused-gate".to_string()),
        "paused task with gate markers must NOT appear in focus_picks, got: {picks:?}"
    );
    assert!(
        !picks.contains(&"inbox-gate".to_string()),
        "inbox task with gate markers must NOT appear in focus_picks, got: {picks:?}"
    );
    assert!(
        !picks.contains(&"blocked-gate".to_string()),
        "blocked task with gate markers must NOT appear in focus_picks, got: {picks:?}"
    );

    // Flow focus picks must likewise only surface ready work
    let today = Utc::now().date_naive();
    let (cliff, regular) = graph.flow_focus_picks(10, today, 7);
    let all_flow_picks: Vec<String> = cliff.into_iter().chain(regular).collect();

    assert!(
        all_flow_picks.contains(&"ready-work".to_string()),
        "ready work must appear in flow_focus_picks, got: {all_flow_picks:?}"
    );
    assert!(
        !all_flow_picks.contains(&"review-gate".to_string()),
        "review task must NOT appear in flow_focus_picks, got: {all_flow_picks:?}"
    );
    assert!(
        !all_flow_picks.contains(&"paused-gate".to_string()),
        "paused task with gate markers must NOT appear in flow_focus_picks, got: {all_flow_picks:?}"
    );
    assert!(
        !all_flow_picks.contains(&"inbox-gate".to_string()),
        "inbox task with gate markers must NOT appear in flow_focus_picks, got: {all_flow_picks:?}"
    );
    assert!(
        !all_flow_picks.contains(&"blocked-gate".to_string()),
        "blocked task with gate markers must NOT appear in flow_focus_picks, got: {all_flow_picks:?}"
    );
}

#[test]
fn test_review_task_is_never_claimable_as_ready_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("tasks")).unwrap();

    fs::write(
        root.join("tasks/review-task.md"),
        "---\nid: review-task\ntitle: \"Awaiting peer review on draft\"\ntype: task\nstatus: review\n---\n",
    ).unwrap();

    fs::write(
        root.join("tasks/ready-task.md"),
        "---\nid: ready-task\ntitle: \"Ready for pickup\"\ntype: task\nstatus: ready\n---\n",
    ).unwrap();

    let graph = GraphStore::build_from_directory(root);
    let ready_ids: Vec<String> = graph.ready_tasks().iter().map(|n| n.id.clone()).collect();

    assert!(
        ready_ids.contains(&"ready-task".to_string()),
        "ready-task must be claimable in ready_tasks(), got: {ready_ids:?}"
    );
    assert!(
        !ready_ids.contains(&"review-task".to_string()),
        "review-task MUST NOT be in ready_tasks() (never claimable as ready work), got: {ready_ids:?}"
    );
}
