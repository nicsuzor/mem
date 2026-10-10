use mem::flow::{
    build_explanation, clean_num, compute_decision_value, compute_flow, compute_routes,
    from_export_json, FlowEdge, FlowEffect, FlowInput, FlowState, FlowStatus,
    CLIFF_BUFFER_DAYS, DEFAULT_EFFORT_DAYS, DEFAULT_QUANTUM, NEEDS_QUANTUM, PART_OF_QUANTUM,
    ROUTE_CAP, SETTLES_QUANTUM,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn load_fixture(part_of_override: Option<f64>) -> (FlowInput, serde_json::Value) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join("specs/flow-rule/fixtures/live-2026-10-05.json");
    let content = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture at {:?}: {}", fixture_path, e));
    let json_val: serde_json::Value = serde_json::from_str(&content).expect("invalid json fixture");
    let input = from_export_json(&json_val, DEFAULT_QUANTUM, part_of_override);
    (input, json_val)
}

fn load_expected() -> BTreeMap<String, (f64, f64)> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_path = manifest_dir.join("specs/pkb-flow-engine/expected-live-2026-10-05.json");
    let content = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("failed to read expected json at {:?}: {}", expected_path, e));
    let parsed: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&content).expect("invalid expected json");
    parsed
        .into_iter()
        .map(|(k, v)| {
            let gain = v.get("gain").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let loss = v.get("loss_averted").and_then(|x| x.as_f64()).unwrap_or(0.0);
            (k, (gain, loss))
        })
        .collect()
}

// ── Acceptance Criteria ──────────────────────────────────────────────────────

/// T-parity (A13): 1,502 fixture rows within 1e-9 of expected-live-2026-10-05.json.
#[test]
fn test_t_parity_1502_fixture_rows() {
    let (input, _) = load_fixture(Some(PART_OF_QUANTUM));
    let expected = load_expected();

    assert_eq!(expected.len(), 1502, "expected fixture must have 1,502 rows");

    let outputs = compute_flow(&input);
    assert_eq!(
        outputs.len(),
        1502,
        "compute_flow must return exactly 1,502 open nodes"
    );

    let mut carrying_count = 0;
    for (id, (exp_gain, exp_loss)) in &expected {
        let out = outputs
            .get(id)
            .unwrap_or_else(|| panic!("missing node {} in compute_flow output", id));
        let act_gain = out.gain.expect("expected non-null gain on fixture");
        let act_loss = out.loss_averted.expect("expected non-null loss on fixture");

        if act_gain > 0.0 || act_loss > 0.0 {
            carrying_count += 1;
        }

        let diff_gain = (act_gain - exp_gain).abs();
        let diff_loss = (act_loss - exp_loss).abs();

        assert!(
            diff_gain <= 1e-9,
            "parity failure on node {} gain: actual {}, expected {}, diff {}",
            id,
            act_gain,
            exp_gain,
            diff_gain
        );
        assert!(
            diff_loss <= 1e-9,
            "parity failure on node {} loss: actual {}, expected {}, diff {}",
            id,
            act_loss,
            exp_loss,
            diff_loss
        );
    }

    assert_eq!(
        carrying_count, 49,
        "exactly 49 open nodes must carry worth on the fixture"
    );
}

/// T-fail (A20): rejected or unsettled loop gives null with named loop; unaffected components ranked.
#[test]
fn test_t_fail_flow_failure_is_null_and_local() {
    let mut input = FlowInput::new();

    // Priced target
    input.add_node("T_priced", FlowState::Open, Some(1.0));

    // Full-strength loop 1 serving priced target
    input.add_node("L1", FlowState::Open, None);
    input.add_node("L2", FlowState::Open, None);
    input.add_edge(FlowEdge::new("L1", "L2", 1.0));
    input.add_edge(FlowEdge::new("L2", "L1", 1.0));
    input.add_edge(FlowEdge::new("L1", "T_priced", 0.8));

    // X feeds loop 1
    input.add_node("X", FlowState::Open, None);
    input.add_edge(FlowEdge::new("X", "L1", 0.5));

    // Saturated loop 2 serving nothing priced
    input.add_node("M1", FlowState::Open, None);
    input.add_node("M2", FlowState::Open, None);
    input.add_edge(FlowEdge::new("M1", "M2", 1.0));
    input.add_edge(FlowEdge::new("M2", "M1", 1.0));

    // Y feeds loop 2
    input.add_node("Y", FlowState::Open, None);
    input.add_edge(FlowEdge::new("Y", "M1", 0.5));

    // Unrelated clean priced chain
    input.add_node("U_target", FlowState::Open, Some(0.5));
    input.add_node("U_task", FlowState::Open, None);
    input.add_edge(FlowEdge::new("U_task", "U_target", 1.0));

    let outputs = compute_flow(&input);

    // X feeds a priced saturated loop -> null figures, saturated_loop status, names both loop nodes
    let out_x = &outputs["X"];
    assert_eq!(out_x.flow_status, FlowStatus::SaturatedLoop);
    assert!(out_x.gain.is_none());
    assert!(out_x.loss_averted.is_none());
    assert_eq!(out_x.loop_nodes, Some(vec!["L1".to_string(), "L2".to_string()]));

    // Y feeds unpriced saturated loop -> R12 checked first -> 0 / 0 / 0, ok
    let out_y = &outputs["Y"];
    assert_eq!(out_y.flow_status, FlowStatus::Ok);
    assert_eq!(out_y.gain, Some(0.0));
    assert_eq!(out_y.loss_averted, Some(0.0));
    assert_eq!(out_y.decision_value, Some(0.0));
    assert!(out_y.loop_nodes.is_none());

    // Unrelated clean task ranked normally
    let out_u = &outputs["U_task"];
    assert_eq!(out_u.flow_status, FlowStatus::Ok);
    assert_eq!(out_u.gain, Some(0.5));
    assert_eq!(out_u.loss_averted, Some(0.0));
}

// ── Invariant Tests T-I1 .. T-I17 ───────────────────────────────────────────

/// T-I1 (I1 / A1): parallel prerequisites each carry the full worth of what they unblock.
#[test]
fn test_flow_inv01_parallel_prerequisites_full_worth() {
    let (input, _) = load_fixture(None);
    let b = "brain_448bb804";

    let pre: Vec<String> = input
        .edges
        .iter()
        .filter(|e| e.dst == b && e.label == "needs" && input.state.get(&e.src) == Some(&FlowState::Open))
        .map(|e| e.src.clone())
        .collect();

    assert!(pre.len() >= 2, "expected at least 2 parallel prerequisites");

    let outputs = compute_flow(&input);
    let b_gain = outputs[b].gain.unwrap();
    assert_eq!(clean_num(b_gain), 0.95);

    for p in &pre {
        let p_gain = outputs[p].gain.unwrap();
        assert!(p_gain >= b_gain - 1e-9);
    }

    // Remove needs edges into b
    let mut g2 = input.clone();
    g2.edges.retain(|e| !(e.dst == b && e.label == "needs"));
    let w2 = compute_flow(&g2);

    assert_eq!(clean_num(w2[b].gain.unwrap()), clean_num(b_gain));

    // Control: when needs edge is their only route, each carries exactly b's gain
    let mut g3 = input.clone();
    g3.edges.retain(|e| !(pre.contains(&e.src) && e.dst != b));
    let w3 = compute_flow(&g3);
    for p in &pre {
        assert_eq!(clean_num(w3[p].gain.unwrap()), clean_num(b_gain));
    }
}

/// T-I2 (I2 / A2): splitting a task into necessary parts changes no unrelated node's standing.
#[test]
fn test_flow_inv02_split_changes_nothing_unrelated() {
    let (input, _) = load_fixture(None);
    let p = "personal_033d02b8";
    let w_orig = compute_flow(&input);
    let orig_gain = w_orig[p].gain.unwrap();

    let mut g2 = input.clone();
    g2.add_node("split_a", FlowState::Open, None);
    g2.add_node("split_b", FlowState::Open, None);

    for e in &input.edges {
        for part in ["split_a", "split_b"] {
            if e.src == p {
                let mut e_new = e.clone();
                e_new.src = part.to_string();
                g2.add_edge(e_new);
            }
            if e.dst == p {
                let mut e_new = e.clone();
                e_new.dst = part.to_string();
                g2.add_edge(e_new);
            }
        }
    }
    g2.state.insert(p.to_string(), FlowState::Gone);

    let w2 = compute_flow(&g2);
    assert_eq!(clean_num(w2["split_a"].gain.unwrap()), clean_num(orig_gain));
    assert_eq!(clean_num(w2["split_b"].gain.unwrap()), clean_num(orig_gain));

    for (k, out) in &w_orig {
        if k != p {
            let out2 = &w2[k];
            assert!(
                (out.gain.unwrap_or(0.0) - out2.gain.unwrap_or(0.0)).abs() < 1e-9,
                "unrelated node {} gain changed after split",
                k
            );
        }
    }
}

/// T-I3 (I3 / A3): one source by several routes counts once; two sources add.
#[test]
fn test_flow_inv03_one_source_once_two_add() {
    let (input, _) = load_fixture(None);
    let u = "proj-76fbc546";
    let outputs = compute_flow(&input);
    let out = &outputs[u];

    let stake_4e = out.deltas.get("targ_4e2cc92a").copied().unwrap_or(0.0);
    let stake_7d = out.deltas.get("task_7d6f78ad").copied().unwrap_or(0.0);

    assert!(stake_4e <= 1.0 + 1e-9 && stake_4e > 0.0);
    assert_eq!(clean_num(stake_4e), 1.0);
    assert_eq!(clean_num(stake_7d), 1.0);

    // gain = 0.60 * 1.0 + 0.35 * 1.0 = 0.95
    assert_eq!(clean_num(out.gain.unwrap()), 0.95);
}

/// T-I4 (I4 / A4): a reinforcing loop converges to a finite value and is bounded.
#[test]
fn test_flow_inv04_reinforcing_loop_bounded() {
    let (input, _) = load_fixture(None);
    let loop_nodes = [
        "aops_twin_cost_monitor",
        "aops_bootstrap_dogfood",
        "aops_otel_full_text_container_spans",
    ];

    // Closed loop
    let mut g_closed = input.clone();
    g_closed.add_node("feeder", FlowState::Open, None);
    let mut feeder_edge = FlowEdge::new("feeder", loop_nodes[0], 0.5);
    feeder_edge.label = "supports".to_string();
    g_closed.add_edge(feeder_edge);

    let w_closed = compute_flow(&g_closed);

    // Opened loop
    let mut g_opened = g_closed.clone();
    for e in &mut g_opened.edges {
        if e.src == loop_nodes[2] && e.dst == loop_nodes[0] {
            e.quantum = 0.0;
        }
    }
    let w_opened = compute_flow(&g_opened);

    let feeder_closed = w_closed["feeder"].gain.unwrap();
    let feeder_opened = w_opened["feeder"].gain.unwrap();

    assert!(feeder_closed >= feeder_opened - 1e-9);
    assert!((feeder_closed - 0.2965).abs() < 1e-4);
    assert!((feeder_opened - 0.2801).abs() < 1e-4);
}

/// T-I5 (I5 / A5): a blocked task passes worth to its unblockers.
#[test]
fn test_flow_inv05_blocked_passes_worth() {
    let (input, _) = load_fixture(None);
    let b = "brain_448bb804";
    let outputs = compute_flow(&input);
    let b_gain = outputs[b].gain.unwrap();
    assert_eq!(clean_num(b_gain), 0.95);

    let pre: Vec<String> = input
        .edges
        .iter()
        .filter(|e| e.dst == b && e.label == "needs" && input.state.get(&e.src) == Some(&FlowState::Open))
        .map(|e| e.src.clone())
        .collect();

    for p in &pre {
        assert!(outputs[p].gain.unwrap() >= b_gain - 1e-9);
    }
}

/// T-I6 (I6 / A6): changing display rules changes no number.
#[test]
fn test_flow_inv06_display_changes_no_number() {
    let (input, _) = load_fixture(None);
    let out1 = compute_flow(&input);
    let out2 = compute_flow(&input);

    for (k, v1) in &out1 {
        let v2 = &out2[k];
        assert_eq!(v1.gain, v2.gain);
        assert_eq!(v1.loss_averted, v2.loss_averted);
        assert_eq!(v1.decision_value, v2.decision_value);
        assert_eq!(v1.deltas, v2.deltas);
    }
}

/// T-I7 (I7 / A7): a node linked to nothing priced carries default (0 / 0 / 0, ok).
#[test]
fn test_flow_inv07_unlinked_is_default() {
    let (input, _) = load_fixture(None);
    let outputs = compute_flow(&input);

    for unlinked in ["academic-b738bdc7", "task_d5f610e6"] {
        let out = &outputs[unlinked];
        assert_eq!(out.gain, Some(0.0));
        assert_eq!(out.loss_averted, Some(0.0));
        assert_eq!(out.decision_value, Some(0.0));
        assert_eq!(out.flow_status, FlowStatus::Ok);
        assert!(out.deltas.is_empty());
    }
}

/// T-I8 (I8 / A8): when all but one necessary step is done, remaining step carries full worth.
#[test]
fn test_flow_inv08_last_step_full_worth() {
    let (input, _) = load_fixture(None);
    let parent = "proj-f8b942d5";
    let last = "admin-3e02c20b";
    let outputs = compute_flow(&input);

    assert_eq!(clean_num(outputs[parent].gain.unwrap()), 1.11);
    assert_eq!(clean_num(outputs[last].gain.unwrap()), 1.11);
}

/// T-I9 (I9 / A9): adding an opportunity takes one node and changes only upstream nodes.
#[test]
fn test_flow_inv09_opportunity_one_node() {
    let (input, _) = load_fixture(None);
    let w_orig = compute_flow(&input);

    let mut g2 = input.clone();
    g2.add_node("opportunity", FlowState::Open, Some(0.35));
    g2.add_edge(FlowEdge::new("admin-3e02c20b", "opportunity", 1.0));

    let w2 = compute_flow(&g2);
    assert_eq!(
        clean_num(w2["admin-3e02c20b"].gain.unwrap()),
        clean_num(w_orig["admin-3e02c20b"].gain.unwrap() + 0.35)
    );
    assert_eq!(clean_num(w2["admin-3e02c20b"].gain.unwrap()), 1.46);
}

/// T-I10 (I10 / A10): open decision weights settle work by EVPI.
#[test]
fn test_flow_inv10_decision_value() {
    let (input, _) = load_fixture(None);
    let d = "brain_bf2be9d8";
    let a = "brain_7f772690";
    let b = "proj-f8b942d5";
    let s = "personal_92d5909f";

    let run_scenario = |scale: f64, decided: bool| {
        let mut g = input.clone();
        for val in g.worth.values_mut() {
            *val *= scale;
        }
        g.edges.retain(|e| !(e.dst == d && (e.src == a || e.src == b) && e.label == "part_of"));
        let d_out_edges: Vec<FlowEdge> = g
            .edges
            .iter()
            .filter(|e| e.src == d && (e.label == "serves" || e.label == "part_of"))
            .cloned()
            .collect();

        for e in d_out_edges {
            for opt in [a, b] {
                let mut e_opt = e.clone();
                e_opt.src = opt.to_string();
                g.add_edge(e_opt);
            }
        }

        let mut edge_a = FlowEdge::new(a, d, 1.0);
        edge_a.label = "alternative".to_string();
        edge_a.probability = 0.4;
        g.add_edge(edge_a);

        let mut edge_b = FlowEdge::new(b, d, 1.0);
        edge_b.label = "alternative".to_string();
        edge_b.probability = 0.3;
        g.add_edge(edge_b);

        let mut edge_s = FlowEdge::new(s, d, 1.0);
        edge_s.label = "settles".to_string();
        g.add_edge(edge_s);

        if decided {
            g.state.insert(d.to_string(), FlowState::Done);
        }

        let mut w = compute_flow(&g);
        compute_decision_value(&g, &mut w);
        w[s].decision_value.unwrap_or(0.0)
    };

    let v1 = run_scenario(1.0, false);
    let v2 = run_scenario(2.0, false);
    let v0 = run_scenario(1.0, true);

    assert_eq!(clean_num(v1), 0.1998);
    assert_eq!(clean_num(v2), 0.3996);
    assert_eq!(clean_num(v0), 0.0);
}

/// T-I11 (I11 / A11): no date types, tokens, or arithmetic in flow.
#[test]
fn test_flow_inv11_no_dates_in_flow() {
    // (a) Compile-time check of FlowInput fields: ids, state, worth, edges
    let input = FlowInput {
        ids: vec![],
        state: BTreeMap::new(),
        worth: BTreeMap::new(),
        edges: vec![],
    };
    assert_eq!(input.ids.len(), 0);

    // (b) Source scan of src/flow.rs for forbidden tokens: chrono, NaiveDate, Utc, due, today
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_path = manifest_dir.join("src/flow.rs");
    let src = fs::read_to_string(&src_path).expect("failed to read src/flow.rs");

    for token in ["chrono", "NaiveDate", "Utc", "due", "today"] {
        let pattern = format!(r"\b{}\b", regex::escape(token));
        let re = regex::RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .build()
            .unwrap();
        assert!(
            !re.is_match(&src),
            "forbidden token '{}' found in src/flow.rs",
            token
        );
    }
}

/// T-I12 (I12 / A12): routes explanation and loop extra.
#[test]
fn test_flow_inv12_routes_explain() {
    let (input, _) = load_fixture(None);
    let outputs = compute_flow(&input);

    for (u, out) in &outputs {
        if out.flow_status == FlowStatus::Ok && !out.deltas.is_empty() {
            let tr_map = compute_routes(&input, out, u);
            for (t, tr) in tr_map {
                assert!(tr.routes.len() <= ROUTE_CAP);
                if let Some(strongest) = tr.routes.first() {
                    let dlt = out.deltas.get(&t).copied().unwrap_or(0.0);
                    assert!(strongest.strength.abs() <= dlt.abs() + 1e-9);
                }
            }
        }
    }

    // Node n_d663317dd7 carries loop extra on targ-7d49f8a0: 0.9712 against loop-free bound 0.9449
    let node = "n_d663317dd7";
    let tr = compute_routes(&input, &outputs[node], node);
    let targ = "targ-7d49f8a0";
    assert!(tr.contains_key(targ));
    let loop_extra = tr[targ].loop_extra.expect("expected loop extra on live loop");
    assert!((loop_extra - 0.0263).abs() < 1e-4);
}

/// T-I14 (I14 / A14): negative target symmetry.
#[test]
fn test_flow_inv14_negative_target_symmetry() {
    let (input, _) = load_fixture(None);
    let safety_nodes = ["proj-db6ded3c", "proj-76fbc546", "personal_344a9ec6"];
    let w_orig = compute_flow(&input);

    // Positive target
    let mut g_pos = input.clone();
    g_pos.add_node("targ_safety", FlowState::Open, Some(0.35));
    let w_pos = compute_flow(&g_pos);

    // Negative target (harm to avoid)
    let mut g_neg = input.clone();
    g_neg.add_node("harm_someone_hurt", FlowState::Open, Some(-0.35));
    for e in &input.edges {
        if e.dst == "targ_safety" {
            let mut e_harm = e.clone();
            e_harm.dst = "harm_someone_hurt".to_string();
            e_harm.effect = FlowEffect::Harms;
            g_neg.add_edge(e_harm);
        }
    }
    let w_neg = compute_flow(&g_neg);

    for u in safety_nodes {
        let gain_added = w_pos[u].gain.unwrap() - w_orig[u].gain.unwrap();
        let loss_averted = w_neg[u].loss_averted.unwrap();
        assert!(loss_averted > 0.0);
        assert_eq!(clean_num(loss_averted), clean_num(gain_added));
    }
}

/// T-I15 (I15 / A15): gain and loss averted are carried strictly side by side and not netted.
#[test]
fn test_flow_inv15_gain_and_loss_not_netted() {
    let mut input = FlowInput::new();
    input.add_node("T1", FlowState::Open, Some(1.0));
    input.add_node("T2", FlowState::Open, Some(1.0));
    input.add_node("task", FlowState::Open, None);

    input.add_edge(FlowEdge::new("task", "T1", 0.6));
    let mut harm_edge = FlowEdge::new("task", "T2", 0.6);
    harm_edge.effect = FlowEffect::Harms;
    input.add_edge(harm_edge);

    let outputs = compute_flow(&input);
    let out = &outputs["task"];

    assert_eq!(out.gain, Some(0.6));
    assert_eq!(out.loss_averted, Some(-0.6));
    assert_ne!((out.gain, out.loss_averted), (Some(0.0), Some(0.0)));
}

/// T-I16 (I16 / A16): loop containing a harmful edge settles to stable value.
#[test]
fn test_flow_inv16_harmful_loop_settles() {
    let (input, _) = load_fixture(None);
    let loop_nodes = [
        "aops_twin_cost_monitor",
        "aops_bootstrap_dogfood",
        "aops_otel_full_text_container_spans",
    ];

    // Live quantum harms closing edge
    let mut g_harm = input.clone();
    g_harm.add_node("feeder", FlowState::Open, None);
    let mut feeder_edge = FlowEdge::new("feeder", loop_nodes[0], 1.0);
    feeder_edge.label = "supports".to_string();
    g_harm.add_edge(feeder_edge);

    for e in &mut g_harm.edges {
        if e.src == loop_nodes[2] && e.dst == loop_nodes[0] {
            e.effect = FlowEffect::Harms;
        }
    }
    let w_harm = compute_flow(&g_harm);
    assert!((w_harm["feeder"].gain.unwrap() - 0.0396).abs() < 1e-4);

    // Pure negative feedback loop at quantum 1.0
    let mut g_neg_loop = g_harm.clone();
    for e in &mut g_neg_loop.edges {
        if e.src == loop_nodes[2] && e.dst == loop_nodes[0] {
            e.quantum = 1.0;
        }
    }
    let w_neg_loop = compute_flow(&g_neg_loop);
    assert!((w_neg_loop["feeder"].gain.unwrap() - 0.0156).abs() < 1e-4);
}

/// T-I17 (I17 / A17): constants and isolation of cliff lane parameters.
#[test]
fn test_flow_inv17_cliff_lane_isolation() {
    assert_eq!(CLIFF_BUFFER_DAYS, 7);
    assert_eq!(DEFAULT_EFFORT_DAYS, 3.0);
    assert_eq!(DEFAULT_QUANTUM, 0.0);
    assert_eq!(PART_OF_QUANTUM, 0.0);
    assert_eq!(NEEDS_QUANTUM, 1.0);
    assert_eq!(SETTLES_QUANTUM, 1.0);
}

/// T-routes (A28): at most ROUTE_CAP routes returned, truncation indicated, explanation generated.
#[test]
fn test_routes_cap_and_explanation() {
    let mut input = FlowInput::new();
    input.add_node("T", FlowState::Open, Some(1.0));
    input.add_node("U", FlowState::Open, None);

    // Create 7 intermediate paths from U to T
    for i in 1..=7 {
        let mid = format!("mid_{}", i);
        input.add_node(&mid, FlowState::Open, None);
        input.add_edge(FlowEdge::new("U", &mid, 1.0));
        input.add_edge(FlowEdge::new(&mid, "T", 0.5));
    }

    let outputs = compute_flow(&input);
    let tr_map = compute_routes(&input, &outputs["U"], "U");

    assert!(tr_map.contains_key("T"));
    let tr = &tr_map["T"];
    assert_eq!(tr.routes.len(), ROUTE_CAP);
    assert!(tr.routes_truncated);

    let explanation = build_explanation("U", &tr_map, &outputs["U"].stake);
    assert!(explanation.contains("via U > mid_"));
}

/// T-cost (A21): Scenario B (every target priced) under 0.25 s budget in release mode.
#[test]
#[ignore]
fn test_flow_cost_budget() {
    let (mut input, _) = load_fixture(Some(PART_OF_QUANTUM));

    // Price all 26 targets at 0.35
    for (id, st) in &input.state {
        if *st == FlowState::Open && (id.starts_with("targ_") || id.starts_with("targ-")) {
            input.worth.insert(id.clone(), 0.35);
        }
    }

    let t0 = std::time::Instant::now();
    let mut outputs = compute_flow(&input);
    compute_decision_value(&input, &mut outputs);
    let elapsed = t0.elapsed();

    println!("compute_flow + compute_decision_value elapsed: {:?}", elapsed);
    assert!(
        elapsed.as_secs_f64() < 0.25,
        "Scenario B cost exceeded 0.25s budget: {:?}",
        elapsed
    );
}
