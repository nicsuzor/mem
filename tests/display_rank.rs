//! Acceptance and invariant tests for flow display ranking layer (`src/display_rank.rs`).
//!
//! Covers:
//! - T-I17: cliff lane isolation and trigger (A17, I17)
//! - T-I6: FlowOutput byte-identical under every sort and buffer (A6, I6)
//! - T-nosum: no group or aggregate view sums worth across nodes (A23, R21)
//! - T-sort: non-default sorting by gain_per_effort, etc. (A30, R22)
//! - T-ready: ready, blocked, and roots task classification (A29, R20, E16)
//! - Canonical `display_cmp` 4-tier ordering contract (R19)

use chrono::NaiveDate;
use mem::display_rank::{
    classify_tasks, compute_task_summary, display_cmp, display_cmp_by, resolve_effort_days,
    DeadlineClass, DisplayItem, DisplayTask, GroupProgress, TaskDisplay, TaskSort,
    ACTIONABLE_TYPES, CLAIMABLE_TYPES, DISPLAY_CLIFF_BUFFER_DAYS, DISPLAY_DEFAULT_EFFORT_DAYS,
};
use mem::flow::{
    clean_num, compute_flow, from_export_json, FlowEdge, FlowInput, FlowOutput, FlowState,
    FlowStatus, CLIFF_BUFFER_DAYS, DEFAULT_QUANTUM, PART_OF_QUANTUM,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

fn load_fixture() -> (FlowInput, Value) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join("specs/flow-rule/fixtures/live-2026-10-05.json");
    let content = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture at {:?}: {}", fixture_path, e));
    let json_val: Value = serde_json::from_str(&content).expect("invalid json fixture");
    let input = from_export_json(&json_val, DEFAULT_QUANTUM, Some(PART_OF_QUANTUM));
    (input, json_val)
}

// ── Acceptance Criteria Tests ────────────────────────────────────────────────

/// T-I17 (I17 / A17): A hard-deadline node with worth 0 is first in list_tasks from
/// effort + CLIFF_BUFFER_DAYS before due; fake or soft nodes never enter the cliff lane.
#[test]
fn test_t_i17_display_inv17_cliff_lane() {
    let due_date = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let effort_days = 1; // 1 day effort
    let buffer_days = CLIFF_BUFFER_DAYS; // 7 days -> triggers at 8 days before due

    // D - 8 days = 2026-10-07
    let trigger_date = due_date - chrono::Duration::days(effort_days + buffer_days);
    assert_eq!(trigger_date, NaiveDate::from_ymd_opt(2026, 10, 7).unwrap());

    // 1. Construct hard, fake, and soft items, plus a high-gain non-cliff node
    let flow_zero = FlowOutput {
        gain: Some(0.0),
        loss_averted: Some(0.0),
        deltas: BTreeMap::new(),
        stake: BTreeMap::new(),
        decision_value: Some(0.0),
        loop_extra: BTreeMap::new(),
        flow_status: FlowStatus::Ok,
        loop_nodes: None,
    };

    let flow_high = FlowOutput {
        gain: Some(5.0),
        loss_averted: Some(2.0),
        deltas: BTreeMap::new(),
        stake: BTreeMap::new(),
        decision_value: Some(1.0),
        loop_extra: BTreeMap::new(),
        flow_status: FlowStatus::Ok,
        loop_nodes: None,
    };

    // Step today from D - 40 to D
    for days_before in 0..=40 {
        let today = due_date - chrono::Duration::days(days_before);

        let hard_item = DisplayItem::new(
            "task_hard_zero",
            Some(&flow_zero),
            Some(due_date),
            Some(DeadlineClass::Hard),
            effort_days,
            today,
            buffer_days,
        );

        let fake_item = DisplayItem::new(
            "task_fake_zero",
            Some(&flow_zero),
            Some(due_date),
            Some(DeadlineClass::Fake),
            effort_days,
            today,
            buffer_days,
        );

        let soft_item = DisplayItem::new(
            "task_soft_zero",
            Some(&flow_zero),
            Some(due_date),
            Some(DeadlineClass::Soft),
            effort_days,
            today,
            buffer_days,
        );

        let high_item = DisplayItem::new(
            "task_high_gain",
            Some(&flow_high),
            None,
            None,
            3,
            today,
            buffer_days,
        );

        // Fake and soft NEVER enter the cliff lane, regardless of today
        assert!(
            !fake_item.on_cliff,
            "fake deadline entered cliff lane at {} days before due",
            days_before
        );
        assert!(
            !soft_item.on_cliff,
            "soft deadline entered cliff lane at {} days before due",
            days_before
        );

        let mut list = vec![
            fake_item.clone(),
            soft_item.clone(),
            high_item.clone(),
            hard_item.clone(),
        ];
        list.sort_by(display_cmp);

        if days_before <= 8 {
            // Within effort + buffer (1 + 7 = 8 days): hard deadline enters cliff lane
            assert!(
                hard_item.on_cliff,
                "hard deadline failed to enter cliff at {} days before due",
                days_before
            );
            // Must be first in the sorted list, despite carrying worth 0 while high_gain has 7.0
            assert_eq!(
                list[0].id, "task_hard_zero",
                "hard-deadline node with worth 0 must be first in list_tasks at {} days before due",
                days_before
            );
        } else {
            // Before cliff trigger: hard deadline is NOT in cliff lane
            assert!(
                !hard_item.on_cliff,
                "hard deadline entered cliff prematurely at {} days before due",
                days_before
            );
            // High gain task is first
            assert_eq!(
                list[0].id, "task_high_gain",
                "high-gain node must be first when not in cliff lane"
            );
        }
    }
}

/// T-I17 on the live fixture nodes: task_d5f610e6 (hard, worth 0), n_c2e542fd02 (fake), n_2dcb93a9c8 (soft).
#[test]
fn test_t_i17_fixture_inv17_nodes() {
    let (input, json_val) = load_fixture();
    let flow_outputs = compute_flow(&input);

    let nodes_list = json_val["nodes"].as_array().expect("nodes array");
    let node_map: HashMap<String, &Value> = nodes_list
        .iter()
        .map(|n| (n["id"].as_str().unwrap().to_string(), n))
        .collect();

    let classes = [
        ("task_d5f610e6", DeadlineClass::Hard),
        ("n_c2e542fd02", DeadlineClass::Fake),
        ("n_2dcb93a9c8", DeadlineClass::Soft),
    ];

    let test_dates = [
        (NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(), false), // D - 34: not on cliff
        (NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), true), // D - 10: on cliff (effort 3 + 7 = 10)
        (NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(), true), // Due date: on cliff
        (NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(), true), // Past due: on cliff
    ];

    for (today, exp_hard_cliff) in test_dates {
        let mut items = Vec::new();
        for (id, class) in &classes {
            let n = node_map.get(*id).expect("node not in fixture");
            let due_str = n.get("due").and_then(|v| v.as_str());
            let due = due_str.and_then(|s| NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok());
            let effort = resolve_effort_days(n.get("effort").and_then(|v| v.as_str()));
            let flow = flow_outputs.get(*id);

            let item = DisplayItem::new(
                *id,
                flow,
                due,
                Some(*class),
                effort,
                today,
                DISPLAY_CLIFF_BUFFER_DAYS,
            );
            items.push(item);
        }

        let hard_item = items.iter().find(|i| i.id == "task_d5f610e6").unwrap();
        let fake_item = items.iter().find(|i| i.id == "n_c2e542fd02").unwrap();
        let soft_item = items.iter().find(|i| i.id == "n_2dcb93a9c8").unwrap();

        assert_eq!(
            hard_item.on_cliff, exp_hard_cliff,
            "hard node cliff mismatch on {}",
            today
        );
        assert!(!fake_item.on_cliff, "fake node must never be on cliff");
        assert!(!soft_item.on_cliff, "soft node must never be on cliff");

        // When hard item is on cliff, it must sort ahead of fake and soft items
        items.sort_by(display_cmp);
        if exp_hard_cliff {
            assert_eq!(items[0].id, "task_d5f610e6");
        }
    }
}

/// T-I6 (I6 / A6): FlowOutput byte-identical under every sort, filter, and buffer.
#[test]
fn test_t_i6_flow_output_byte_identical_under_every_sort_and_buffer() {
    let (input, json_val) = load_fixture();
    let original_flow = compute_flow(&input);
    let original_json = serde_json::to_string(&original_flow).expect("serialize flow");

    let nodes_list = json_val["nodes"].as_array().expect("nodes array");
    let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();

    let all_sorts = [
        TaskSort::Default,
        TaskSort::Gain,
        TaskSort::LossAverted,
        TaskSort::DecisionValue,
        TaskSort::GainPerEffort,
        TaskSort::LossAvertedPerEffort,
        TaskSort::Due,
        TaskSort::Id,
    ];

    let all_buffers = [0i64, 7, 14, 30];

    for &buffer in &all_buffers {
        for &sort_mode in &all_sorts {
            // Build display items from the flow outputs
            let mut items: Vec<DisplayItem> = nodes_list
                .iter()
                .map(|n| {
                    let id = n["id"].as_str().unwrap();
                    let due_str = n.get("due").and_then(|v| v.as_str());
                    let due = due_str.and_then(|s| NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok());
                    let effort = resolve_effort_days(n.get("effort").and_then(|v| v.as_str()));
                    let flow = original_flow.get(id);

                    DisplayItem::new(id, flow, due, None, effort, today, buffer)
                })
                .collect();

            // Run display ordering
            items.sort_by(|a, b| display_cmp_by(a, b, sort_mode));

            // Verify that the flow outputs are untouched and 100% byte-identical
            let current_json = serde_json::to_string(&original_flow).expect("serialize flow");
            assert_eq!(
                original_json, current_json,
                "FlowOutput was mutated under sort {:?} with buffer {}",
                sort_mode, buffer
            );
        }
    }

    // Verify source code isolation: src/flow.rs must never import display_rank
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flow_src_path = manifest_dir.join("src/flow.rs");
    let flow_src = fs::read_to_string(&flow_src_path).expect("read src/flow.rs");
    assert!(
        !flow_src.contains("display_rank"),
        "src/flow.rs must not import or reference display_rank"
    );
    assert!(
        !flow_src.contains("DisplayItem"),
        "src/flow.rs must not know about DisplayItem"
    );
}

/// T-nosum (A23 / R21): No tool or CLI output contains a sum of gain, loss_averted,
/// or decision_value over two or more nodes.
#[test]
fn test_t_nosum_no_output_sums_worth() {
    // Two nodes with gain 0.37 and 0.58 under one parent
    let mut input = FlowInput::new();
    input.add_node("target", FlowState::Open, Some(1.0));
    input.add_node("parent", FlowState::Open, None);
    input.add_node("child1", FlowState::Open, None);
    input.add_node("child2", FlowState::Open, None);

    input.add_edge(FlowEdge::new("child1", "target", 0.37));
    input.add_edge(FlowEdge::new("child2", "target", 0.58));
    input.add_edge(FlowEdge::new("child1", "parent", 0.0).with_label("part_of"));
    input.add_edge(FlowEdge::new("child2", "parent", 0.0).with_label("part_of"));

    let flow_outputs = compute_flow(&input);

    let g1 = flow_outputs["child1"].gain.unwrap();
    let g2 = flow_outputs["child2"].gain.unwrap();
    assert_eq!(clean_num(g1), 0.37);
    assert_eq!(clean_num(g2), 0.58);

    let forbidden_sum = 0.95; // 0.37 + 0.58
    let forbidden_mean = 0.475; // 0.95 / 2

    // 1. Check GroupProgress
    let progress = GroupProgress::new(2, 1);
    let progress_json = serde_json::to_string(&progress).unwrap();

    fn assert_no_forbidden_numbers(val: &Value, sum: f64, mean: f64) {
        match val {
            Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    assert!(
                        (f - sum).abs() > 1e-9,
                        "forbidden sum {} found in output: {}",
                        sum,
                        f
                    );
                    assert!(
                        (f - mean).abs() > 1e-9,
                        "forbidden mean {} found in output: {}",
                        mean,
                        f
                    );
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    assert_no_forbidden_numbers(item, sum, mean);
                }
            }
            Value::Object(map) => {
                for (_, v) in map {
                    assert_no_forbidden_numbers(v, sum, mean);
                }
            }
            _ => {}
        }
    }

    let parsed_progress: Value = serde_json::from_str(&progress_json).unwrap();
    assert_no_forbidden_numbers(&parsed_progress, forbidden_sum, forbidden_mean);

    // 2. Check TaskSummaryCounts
    let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
    let item1 = DisplayItem::new("child1", Some(&flow_outputs["child1"]), None, None, 3, today, 7);
    let item2 = DisplayItem::new("child2", Some(&flow_outputs["child2"]), None, None, 3, today, 7);
    let mut ready_set = HashSet::new();
    ready_set.insert("child1".to_string());
    ready_set.insert("child2".to_string());

    let summary = compute_task_summary(&[item1, item2], &ready_set);
    let summary_json = serde_json::to_string(&summary).unwrap();
    let parsed_summary: Value = serde_json::from_str(&summary_json).unwrap();
    assert_no_forbidden_numbers(&parsed_summary, forbidden_sum, forbidden_mean);

    // 3. Check TaskDisplay serialization
    let display1 = TaskDisplay::new(true, false, None, None, 3, today, 7);
    let display_json = serde_json::to_string(&display1).unwrap();
    let parsed_display: Value = serde_json::from_str(&display_json).unwrap();
    assert_no_forbidden_numbers(&parsed_display, forbidden_sum, forbidden_mean);
}

// ── Additional Unit & Feature Tests ──────────────────────────────────────────

/// T-sort (A30 / R22): gain_per_effort sorting orders by gain / effort_days.
#[test]
fn test_t_sort_gain_per_effort() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();

    let make_flow = |g: f64| FlowOutput {
        gain: Some(g),
        loss_averted: Some(0.0),
        deltas: BTreeMap::new(),
        stake: BTreeMap::new(),
        decision_value: None,
        loop_extra: BTreeMap::new(),
        flow_status: FlowStatus::Ok,
        loop_nodes: None,
    };

    // Node A: gain 6.0, effort 3 days -> 2.0 per day
    let flow_a = make_flow(6.0);
    let item_a = DisplayItem::new("A", Some(&flow_a), None, None, 3, today, 7);

    // Node B: gain 3.0, effort 1 day -> 3.0 per day
    let flow_b = make_flow(3.0);
    let item_b = DisplayItem::new("B", Some(&flow_b), None, None, 1, today, 7);

    // Node C: gain 10.0, effort 10 days -> 1.0 per day
    let flow_c = make_flow(10.0);
    let item_c = DisplayItem::new("C", Some(&flow_c), None, None, 10, today, 7);

    // Default sort (Nic key): C (10.0) > A (6.0) > B (3.0)
    let mut default_list = vec![item_a.clone(), item_b.clone(), item_c.clone()];
    default_list.sort_by(display_cmp);
    assert_eq!(default_list[0].id, "C");
    assert_eq!(default_list[1].id, "A");
    assert_eq!(default_list[2].id, "B");

    // Gain per effort sort: B (3.0/d) > A (2.0/d) > C (1.0/d)
    let mut per_effort_list = vec![item_a.clone(), item_b.clone(), item_c.clone()];
    per_effort_list.sort_by(|x, y| display_cmp_by(x, y, TaskSort::GainPerEffort));
    assert_eq!(per_effort_list[0].id, "B");
    assert_eq!(per_effort_list[1].id, "A");
    assert_eq!(per_effort_list[2].id, "C");
}

/// T-ready (A29 / R20 / E16): Blocked tasks keep figures but are never ready.
/// `status: blocked` alone is NOT blocked without unmet/downstream dependency (E16).
#[test]
fn test_t_ready_predicate_and_classification() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();

    let make_flow = |g: f64| FlowOutput {
        gain: Some(g),
        loss_averted: Some(0.0),
        deltas: BTreeMap::new(),
        stake: BTreeMap::new(),
        decision_value: None,
        loop_extra: BTreeMap::new(),
        flow_status: FlowStatus::Ok,
        loop_nodes: None,
    };

    let mut tasks = HashMap::new();

    // Task 1: Open blocker (ready)
    tasks.insert(
        "blocker".to_string(),
        DisplayTask {
            id: "blocker".to_string(),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            is_leaf: true,
            has_acceptance_criteria: true,
            depends_on: vec![],
            blocks: vec!["blocked_task".to_string()],
            parent: None,
            due: None,
            deadline_class: None,
            effort_days: 1,
            flow: Some(make_flow(0.95)),
        },
    );

    // Task 2: Blocked by Task 1 (not ready, blocked)
    tasks.insert(
        "blocked_task".to_string(),
        DisplayTask {
            id: "blocked_task".to_string(),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            is_leaf: true,
            has_acceptance_criteria: true,
            depends_on: vec!["blocker".to_string()],
            blocks: vec!["downstream_blocked".to_string()],
            parent: None,
            due: None,
            deadline_class: None,
            effort_days: 1,
            flow: Some(make_flow(0.95)),
        },
    );

    // Task 3: Downstream blocked via transitive propagation
    tasks.insert(
        "downstream_blocked".to_string(),
        DisplayTask {
            id: "downstream_blocked".to_string(),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            is_leaf: true,
            has_acceptance_criteria: true,
            depends_on: vec!["blocked_task".to_string()],
            blocks: vec![],
            parent: None,
            due: None,
            deadline_class: None,
            effort_days: 1,
            flow: Some(make_flow(0.95)),
        },
    );

    // Task 4: Node with status="blocked" but NO unmet dependency (E16)
    tasks.insert(
        "explicit_status_blocked".to_string(),
        DisplayTask {
            id: "explicit_status_blocked".to_string(),
            node_type: Some("task".to_string()),
            status: Some("blocked".to_string()),
            is_leaf: true,
            has_acceptance_criteria: true,
            depends_on: vec![], // No unmet dependencies!
            blocks: vec![],
            parent: None,
            due: None,
            deadline_class: None,
            effort_days: 1,
            flow: Some(make_flow(0.50)),
        },
    );

    let (ready, blocked, roots) = classify_tasks(&tasks, today, 7);

    // Blocker is ready; blocked_task and downstream_blocked are NOT ready
    assert!(ready.contains(&"blocker".to_string()));
    assert!(!ready.contains(&"blocked_task".to_string()));
    assert!(!ready.contains(&"downstream_blocked".to_string()));

    // Blocked tasks are in blocked list
    assert!(blocked.contains(&"blocked_task".to_string()));
    assert!(blocked.contains(&"downstream_blocked".to_string()));
    assert!(!blocked.contains(&"blocker".to_string()));

    // E16 check: explicit_status_blocked has no unmet dependency -> NOT in blocked list
    assert!(
        !blocked.contains(&"explicit_status_blocked".to_string()),
        "status: blocked alone must not count as blocked in R20 (E16)"
    );

    // All root tasks with no parent are in roots
    assert!(roots.contains(&"blocker".to_string()));
    assert!(roots.contains(&"blocked_task".to_string()));
}

/// Test complete 4-tier display_cmp logic (R19).
#[test]
fn test_display_cmp_tiers_and_ties() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();

    let make_flow = |g: Option<f64>, l: Option<f64>, d: Option<f64>, st: FlowStatus| FlowOutput {
        gain: g,
        loss_averted: l,
        deltas: BTreeMap::new(),
        stake: BTreeMap::new(),
        decision_value: d,
        loop_extra: BTreeMap::new(),
        flow_status: st,
        loop_nodes: None,
    };

    // Tier 1: Cliff lane items (ordered by due ASC)
    let cliff_early = DisplayItem::new(
        "cliff_early",
        None,
        Some(NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()),
        Some(DeadlineClass::Hard),
        1,
        today,
        7,
    );
    let cliff_later = DisplayItem::new(
        "cliff_later",
        None,
        Some(NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()),
        Some(DeadlineClass::Hard),
        1,
        today,
        7,
    );

    // Tier 2: Nic key items
    let high_sum = DisplayItem::new(
        "high_sum",
        Some(&make_flow(Some(2.0), Some(1.0), None, FlowStatus::Ok)), // sum 3.0
        None,
        None,
        3,
        today,
        7,
    );
    let tied_sum_high_gain = DisplayItem::new(
        "tied_sum_high_gain",
        Some(&make_flow(Some(2.5), Some(0.5), None, FlowStatus::Ok)), // sum 3.0, gain 2.5
        None,
        None,
        3,
        today,
        7,
    );
    let tied_gain_high_loss = DisplayItem::new(
        "tied_gain_high_loss",
        Some(&make_flow(Some(1.5), Some(1.5), None, FlowStatus::Ok)), // sum 3.0, gain 1.5, loss 1.5
        None,
        None,
        3,
        today,
        7,
    );
    let tied_all_high_decision = DisplayItem::new(
        "tied_all_high_dec",
        Some(&make_flow(Some(1.5), Some(1.5), Some(0.5), FlowStatus::Ok)), // decision 0.5
        None,
        None,
        3,
        today,
        7,
    );

    // Tier 3a: Saturated loop / error (null figures)
    let err_item = DisplayItem::new(
        "loop_error",
        Some(&make_flow(None, None, None, FlowStatus::SaturatedLoop)),
        None,
        None,
        3,
        today,
        7,
    );

    // Tier 3b: Zero figures
    let zero_item_b = DisplayItem::new(
        "zero_b",
        Some(&make_flow(Some(0.0), Some(0.0), Some(0.0), FlowStatus::Ok)),
        None,
        None,
        3,
        today,
        7,
    );
    let zero_item_a = DisplayItem::new(
        "zero_a",
        Some(&make_flow(Some(0.0), Some(0.0), Some(0.0), FlowStatus::Ok)),
        None,
        None,
        3,
        today,
        7,
    );

    let mut items = vec![
        zero_item_b.clone(),
        tied_all_high_decision.clone(),
        cliff_later.clone(),
        high_sum.clone(),
        err_item.clone(),
        zero_item_a.clone(),
        tied_sum_high_gain.clone(),
        cliff_early.clone(),
        tied_gain_high_loss.clone(),
    ];

    items.sort_by(display_cmp);

    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "cliff_early",           // Tier 1: earlier due
            "cliff_later",           // Tier 1: later due
            "tied_sum_high_gain",    // Tier 2: sum 3.0, gain 2.5
            "high_sum",              // Tier 2: sum 3.0, gain 2.0
            "tied_all_high_dec",     // Tier 2: sum 3.0, gain 1.5, decision 0.5
            "tied_gain_high_loss",   // Tier 2: sum 3.0, gain 1.5, decision 0.0
            "loop_error",            // Tier 3a: error / null figures
            "zero_a",                // Tier 3b: all zero, id ASC
            "zero_b",                // Tier 3b: all zero, id ASC
        ]
    );
}

/// Verify constants match specs.
#[test]
fn test_display_constants() {
    assert_eq!(DISPLAY_CLIFF_BUFFER_DAYS, 7);
    assert_eq!(DISPLAY_DEFAULT_EFFORT_DAYS, 3.0);
    assert_eq!(ACTIONABLE_TYPES, &["task", "learn", "pr"]);
    assert_eq!(CLAIMABLE_TYPES, &["task"]);
}
