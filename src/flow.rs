//! Flow ranking maths engine (`src/flow.rs`).
//!
//! Pure mathematical model for ranking work by its contribution to priced targets.
//! Follows `specs/pkb-flow-engine.md` §2 R1 and §5 (R12-R18), and `specs/flow-rule.md`.
//!
//! Pure function: does not read calendar fields, node categories, or metadata labels.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ── Constants (R11) ──────────────────────────────────────────────────────────

/// Default quantum for unvalued `serves` and `supports` links.
pub const DEFAULT_QUANTUM: f64 = 0.0;

/// Quantum for `part_of` and parent links (filing links; quantum 0.0).
pub const PART_OF_QUANTUM: f64 = 0.0;

/// Default duration in whole units when unstated.
pub const DEFAULT_EFFORT_DAYS: f64 = 3.0;

/// Default quantum for `needs` links.
pub const NEEDS_QUANTUM: f64 = 1.0;

/// Default quantum for `settles` links.
pub const SETTLES_QUANTUM: f64 = 1.0;

/// Horizon buffer in whole units for cliff lane calculation.
pub const CLIFF_BUFFER_DAYS: i64 = 7;

/// Cap on simple paths enumerated per target in route explanations.
pub const ROUTE_CAP: usize = 5;

/// Convergence tolerance for fixed-point iteration.
pub const TOL: f64 = 1e-12;

/// Maximum fixed-point iterations per component.
pub const ITERATION_CAP: usize = 20_000;

/// Damping factor for Jacobi iterations on mutual/cyclical harms.
pub const HARMS_DAMPING: f64 = 0.5;

// ── Types ────────────────────────────────────────────────────────────────────

/// Lifecycle state of a node in the flow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowState {
    Open,
    Done,
    Gone,
}

/// Map frontmatter status to flow lifecycle state (C8 / R1).
pub fn status_to_flow_state(status: Option<&str>) -> FlowState {
    match status {
        Some("cancelled") => FlowState::Gone,
        Some("done") | Some("retired") | None => FlowState::Done,
        Some(_) => FlowState::Open,
    }
}

/// Sign/direction of an edge's influence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowEffect {
    Helps,
    Harms,
}

/// An edge carrying flow between nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowEdge {
    pub src: String,
    pub dst: String,
    pub label: String,
    pub quantum: f64,
    pub probability: f64,
    pub effect: FlowEffect,
    pub unvalued: bool,
}

impl FlowEdge {
    pub fn new(src: impl Into<String>, dst: impl Into<String>, quantum: f64) -> Self {
        Self {
            src: src.into(),
            dst: dst.into(),
            label: "serves".to_string(),
            quantum,
            probability: 1.0,
            effect: FlowEffect::Helps,
            unvalued: false,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_probability(mut self, probability: f64) -> Self {
        self.probability = probability;
        self
    }

    pub fn with_effect(mut self, effect: FlowEffect) -> Self {
        self.effect = effect;
        self
    }

    pub fn with_unvalued(mut self, unvalued: bool) -> Self {
        self.unvalued = unvalued;
        self
    }

    pub fn strength(&self) -> f64 {
        self.quantum * self.probability
    }
}

/// Input graph for the flow maths engine (R1).
///
/// Contains strictly ids, states, worths, and edges.
/// Never contains time limits, categories, tags, or stakeholder labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowInput {
    pub ids: Vec<String>,
    pub state: BTreeMap<String, FlowState>,
    pub worth: BTreeMap<String, f64>,
    pub edges: Vec<FlowEdge>,
}

impl FlowInput {
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            state: BTreeMap::new(),
            worth: BTreeMap::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_node(&mut self, id: impl Into<String>, state: FlowState, worth: Option<f64>) {
        let node_id = id.into();
        if !self.state.contains_key(&node_id) {
            self.ids.push(node_id.clone());
        }
        self.state.insert(node_id.clone(), state);
        if let Some(w) = worth {
            self.worth.insert(node_id, w);
        }
    }

    pub fn add_edge(&mut self, edge: FlowEdge) {
        self.edges.push(edge);
    }
}

impl Default for FlowInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic status for flow computation on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    Ok,
    SaturatedLoop,
    NoConvergence,
}

/// Computed flow output for an open node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowOutput {
    pub gain: Option<f64>,
    pub loss_averted: Option<f64>,
    pub deltas: BTreeMap<String, f64>,
    pub stake: BTreeMap<String, f64>,
    pub decision_value: Option<f64>,
    pub loop_extra: BTreeMap<String, f64>,
    pub flow_status: FlowStatus,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_nodes: Option<Vec<String>>,
}

/// Fixed point non-convergence error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no fixed point reached within iteration cap")]
pub struct NoConvergence;

// ── Number Cleaning ──────────────────────────────────────────────────────────

/// Round to 9 decimal places and zero negligible residuals (< 1e-9).
pub fn clean_num(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        0.0
    } else {
        let rounded = (v * 1e9).round() / 1e9;
        if rounded.abs() < 1e-9 {
            0.0
        } else {
            rounded
        }
    }
}

// ── Internal Graph Helpers ───────────────────────────────────────────────────

/// Whether an edge helps its destination, reading effects in avoidance terms for negative targets.
pub fn edge_helps(edge: &FlowEdge, worth: &BTreeMap<String, f64>) -> bool {
    let mut flips = 0;
    if edge.effect == FlowEffect::Harms {
        flips += 1;
    }
    if worth.get(&edge.src).copied().unwrap_or(0.0) < 0.0 {
        flips += 1;
    }
    if worth.get(&edge.dst).copied().unwrap_or(0.0) < 0.0 {
        flips += 1;
    }
    flips % 2 == 0
}

/// Filter for active flow edges: non-decision labels, neither endpoint gone, non-zero strength.
pub fn is_flow_edge(edge: &FlowEdge, state: &BTreeMap<String, FlowState>) -> bool {
    if edge.label == "alternative" || edge.label == "settles" {
        return false;
    }
    if state.get(&edge.src).copied() == Some(FlowState::Gone) {
        return false;
    }
    if state.get(&edge.dst).copied() == Some(FlowState::Gone) {
        return false;
    }
    edge.strength() > 0.0
}

pub(crate) fn build_indices<'a>(
    edges: &'a [FlowEdge],
    state: &BTreeMap<String, FlowState>,
) -> (
    BTreeMap<String, Vec<&'a FlowEdge>>,
    BTreeMap<String, Vec<&'a FlowEdge>>,
) {
    let mut inc: BTreeMap<String, Vec<&'a FlowEdge>> = BTreeMap::new();
    let mut out: BTreeMap<String, Vec<&'a FlowEdge>> = BTreeMap::new();
    for e in edges {
        if is_flow_edge(e, state) {
            inc.entry(e.dst.clone()).or_default().push(e);
            out.entry(e.src.clone()).or_default().push(e);
        }
    }
    (inc, out)
}

/// Strongly connected components of size > 1 over a set of edges.
pub fn find_components(
    nodes: &[String],
    out: &BTreeMap<String, Vec<&FlowEdge>>,
) -> Vec<Vec<String>> {
    let mut index_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut low_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut on_stack: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<Vec<String>> = Vec::new();
    let mut counter = 0;

    fn visit(
        v: &str,
        out: &BTreeMap<String, Vec<&FlowEdge>>,
        index_map: &mut BTreeMap<String, usize>,
        low_map: &mut BTreeMap<String, usize>,
        stack: &mut Vec<String>,
        on_stack: &mut BTreeSet<String>,
        found: &mut Vec<Vec<String>>,
        counter: &mut usize,
    ) {
        index_map.insert(v.to_string(), *counter);
        low_map.insert(v.to_string(), *counter);
        *counter += 1;
        stack.push(v.to_string());
        on_stack.insert(v.to_string());

        if let Some(edges) = out.get(v) {
            for e in edges {
                if !index_map.contains_key(&e.dst) {
                    visit(&e.dst, out, index_map, low_map, stack, on_stack, found, counter);
                    let low_dst = low_map[&e.dst];
                    let low_v = low_map.get_mut(v).unwrap();
                    if low_dst < *low_v {
                        *low_v = low_dst;
                    }
                } else if on_stack.contains(&e.dst) {
                    let index_dst = index_map[&e.dst];
                    let low_v = low_map.get_mut(v).unwrap();
                    if index_dst < *low_v {
                        *low_v = index_dst;
                    }
                }
            }
        }

        if low_map[v] == index_map[v] {
            let mut comp = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack.remove(&w);
                comp.push(w.clone());
                if w == v {
                    break;
                }
            }
            if comp.len() > 1 {
                comp.sort();
                found.push(comp);
            }
        }
    }

    for v in nodes {
        if !index_map.contains_key(v) {
            visit(
                v,
                out,
                &mut index_map,
                &mut low_map,
                &mut stack,
                &mut on_stack,
                &mut found,
                &mut counter,
            );
        }
    }

    found.sort();
    found
}

/// Nodes participating in loops of flow edges.
pub fn on_loops(input: &FlowInput) -> BTreeSet<String> {
    let (_, out) = build_indices(&input.edges, &input.state);
    let mut all_nodes: Vec<String> = input.state.keys().cloned().collect();
    all_nodes.sort();
    let comps = find_components(&all_nodes, &out);
    let mut res = BTreeSet::new();
    for c in comps {
        for v in c {
            res.insert(v);
        }
    }
    res
}

/// Full-strength loops of open nodes joined by helps edges.
pub fn saturated_loops(input: &FlowInput) -> Vec<Vec<String>> {
    let mut sat_out: BTreeMap<String, Vec<&FlowEdge>> = BTreeMap::new();
    for e in &input.edges {
        if is_flow_edge(e, &input.state)
            && e.strength() >= 1.0 - 1e-12
            && edge_helps(e, &input.worth)
            && input.state.get(&e.src) == Some(&FlowState::Open)
            && input.state.get(&e.dst) == Some(&FlowState::Open)
        {
            sat_out.entry(e.src.clone()).or_default().push(e);
        }
    }
    let mut all_nodes: Vec<String> = input.state.keys().cloned().collect();
    all_nodes.sort();
    find_components(&all_nodes, &sat_out)
}

/// Forward reachable cone from start node across flow edges.
pub fn forward_cone(out: &BTreeMap<String, Vec<&FlowEdge>>, start: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start.to_string()];
    seen.insert(start.to_string());
    while let Some(v) = stack.pop() {
        if let Some(edges) = out.get(&v) {
            for e in edges {
                if !seen.contains(&e.dst) {
                    seen.insert(e.dst.clone());
                    stack.push(e.dst.clone());
                }
            }
        }
    }
    seen.into_iter().collect()
}

// ── Fixed-Point Iteration (Settle) ───────────────────────────────────────────

fn settle(
    state: &BTreeMap<String, FlowState>,
    worth: &BTreeMap<String, f64>,
    inc: &BTreeMap<String, Vec<&FlowEdge>>,
    x: &BTreeMap<String, f64>,
    nodes: &[String],
    knocked: Option<&str>,
    active_harms: Option<&BTreeSet<String>>,
    loop_nodes: &BTreeSet<String>,
) -> Result<BTreeMap<String, f64>, NoConvergence> {
    settle_with_cap(
        state,
        worth,
        inc,
        x,
        nodes,
        knocked,
        active_harms,
        loop_nodes,
        ITERATION_CAP,
    )
}

pub(crate) fn settle_with_cap(
    state: &BTreeMap<String, FlowState>,
    worth: &BTreeMap<String, f64>,
    inc: &BTreeMap<String, Vec<&FlowEdge>>,
    x: &BTreeMap<String, f64>,
    nodes: &[String],
    knocked: Option<&str>,
    active_harms: Option<&BTreeSet<String>>,
    loop_nodes: &BTreeSet<String>,
    iter_cap: usize,
) -> Result<BTreeMap<String, f64>, NoConvergence> {
    let harms_inside = nodes.iter().any(|v| {
        inc.get(v).is_some_and(|edges| {
            edges.iter().any(|e| !edge_helps(e, worth))
        })
    });

    if harms_inside {
        // Parallel (Jacobi) iteration with damping 0.5: node-ID independent on mutual harm
        let damping = HARMS_DAMPING;
        let mut y = x.clone();
        for _ in 0..iter_cap {
            let mut change: f64 = 0.0;
            let mut new_y: BTreeMap<String, f64> = BTreeMap::new();
            for v in nodes {
                let new_val = if Some(v.as_str()) == knocked {
                    0.0
                } else if state.get(v) == Some(&FlowState::Done) || !state.contains_key(v) {
                    1.0
                } else {
                    let mut prod = 1.0;
                    if let Some(incoming) = inc.get(v) {
                        for e in incoming {
                            if edge_helps(e, worth) {
                                let yw = y.get(&e.src).copied().unwrap_or(1.0);
                                prod *= 1.0 - e.strength() * (1.0 - yw);
                            } else {
                                let src_active = state.get(&e.src) == Some(&FlowState::Done)
                                    || (loop_nodes.contains(&e.src) && loop_nodes.contains(&e.dst))
                                    || active_harms.is_none()
                                    || active_harms.unwrap().contains(&e.src);
                                let yw = if src_active {
                                    y.get(&e.src).copied().unwrap_or(1.0)
                                } else {
                                    0.0
                                };
                                prod *= 1.0 - e.strength() * yw;
                            }
                        }
                    }
                    prod
                };
                let prev = y.get(v).copied().unwrap_or(1.0);
                let damped_new = prev + damping * (new_val - prev);
                let diff = (damped_new - prev).abs();
                if diff > change {
                    change = diff;
                }
                new_y.insert(v.clone(), damped_new);
            }
            for (k, v) in new_y {
                y.insert(k, v);
            }
            if change < TOL {
                return Ok(y);
            }
        }
        Err(NoConvergence)
    } else {
        // Gauss-Seidel iteration: monotone systems (Tarski 1955), fast convergence
        let mut y = x.clone();
        for _ in 0..iter_cap {
            let mut change: f64 = 0.0;
            for v in nodes {
                let new_val = if Some(v.as_str()) == knocked {
                    0.0
                } else if state.get(v) == Some(&FlowState::Done) || !state.contains_key(v) {
                    1.0
                } else {
                    let mut prod = 1.0;
                    if let Some(incoming) = inc.get(v) {
                        for e in incoming {
                            let yw = y.get(&e.src).copied().unwrap_or(1.0);
                            prod *= 1.0 - e.strength() * (1.0 - yw);
                        }
                    }
                    prod
                };
                let prev = y.get(v).copied().unwrap_or(1.0);
                let diff = (new_val - prev).abs();
                if diff > change {
                    change = diff;
                }
                y.insert(v.clone(), new_val);
            }
            if change < TOL {
                return Ok(y);
            }
        }
        Err(NoConvergence)
    }
}

/// Baseline counterfactual fixed point: open nodes assumed done, only done/internal harms fire.
pub fn baseline(input: &FlowInput) -> Result<BTreeMap<String, f64>, NoConvergence> {
    baseline_with_cap(input, ITERATION_CAP)
}

/// Baseline counterfactual fixed point with custom iteration cap.
pub fn baseline_with_cap(
    input: &FlowInput,
    iter_cap: usize,
) -> Result<BTreeMap<String, f64>, NoConvergence> {
    let (inc, _) = build_indices(&input.edges, &input.state);
    let mut all_nodes: Vec<String> = input.state.keys().cloned().collect();
    all_nodes.sort();
    let loop_nodes = on_loops(input);
    let initial: BTreeMap<String, f64> = all_nodes.iter().map(|v| (v.clone(), 1.0)).collect();
    let empty_harms = BTreeSet::new();
    settle_with_cap(
        &input.state,
        &input.worth,
        &inc,
        &initial,
        &all_nodes,
        None,
        Some(&empty_harms),
        &loop_nodes,
        iter_cap,
    )
}

// ── Flow Computation ─────────────────────────────────────────────────────────

/// Compute flow figures for open nodes (R1, §5).
pub fn compute_flow(input: &FlowInput) -> BTreeMap<String, FlowOutput> {
    let mut result = BTreeMap::new();
    let (inc, out) = build_indices(&input.edges, &input.state);
    let loop_nodes = on_loops(input);
    let bad_loops = saturated_loops(input);

    let base = match baseline(input) {
        Ok(b) => b,
        Err(_) => {
            // Whole graph baseline failure
            let mut failed_nodes: Vec<String> = loop_nodes.into_iter().collect();
            failed_nodes.sort();
            for (u, st) in &input.state {
                if *st == FlowState::Open {
                    result.insert(
                        u.clone(),
                        FlowOutput {
                            gain: None,
                            loss_averted: None,
                            deltas: BTreeMap::new(),
                            stake: BTreeMap::new(),
                            decision_value: None,
                            loop_extra: BTreeMap::new(),
                            flow_status: FlowStatus::NoConvergence,
                            loop_nodes: Some(failed_nodes.clone()),
                        },
                    );
                }
            }
            return result;
        }
    };

    let empty_harms = BTreeSet::new();

    for (u, st) in &input.state {
        if *st != FlowState::Open {
            continue;
        }

        let cone = forward_cone(&out, u);
        let priced: Vec<String> = cone
            .iter()
            .filter(|t| input.worth.contains_key(*t))
            .cloned()
            .collect();

        // R12 / R13: check priced target reach first
        if priced.is_empty() {
            result.insert(
                u.clone(),
                FlowOutput {
                    gain: Some(0.0),
                    loss_averted: Some(0.0),
                    deltas: BTreeMap::new(),
                    stake: BTreeMap::new(),
                    decision_value: Some(0.0),
                    loop_extra: BTreeMap::new(),
                    flow_status: FlowStatus::Ok,
                    loop_nodes: None,
                },
            );
            continue;
        }

        // Check if cone touches any saturated loop
        let bad_in_cone: Vec<&Vec<String>> = bad_loops
            .iter()
            .filter(|comp| comp.iter().any(|v| cone.contains(v)))
            .collect();
        if !bad_in_cone.is_empty() {
            let mut failed_loop = bad_in_cone[0].clone();
            failed_loop.sort();
            result.insert(
                u.clone(),
                FlowOutput {
                    gain: None,
                    loss_averted: None,
                    deltas: BTreeMap::new(),
                    stake: BTreeMap::new(),
                    decision_value: None,
                    loop_extra: BTreeMap::new(),
                    flow_status: FlowStatus::SaturatedLoop,
                    loop_nodes: Some(failed_loop),
                },
            );
            continue;
        }

        let cone_set: BTreeSet<&String> = cone.iter().collect();
        let has_ext_harms = cone.iter().any(|v| {
            if let Some(incoming) = inc.get(v) {
                incoming.iter().any(|e| {
                    cone_set.contains(&e.src)
                        && !edge_helps(e, &input.worth)
                        && !(loop_nodes.contains(&e.src) && loop_nodes.contains(&e.dst))
                })
            } else {
                false
            }
        });

        let deltas_res = if has_ext_harms {
            let mut active_u = BTreeSet::new();
            active_u.insert(u.clone());
            let with_u = settle(
                &input.state,
                &input.worth,
                &inc,
                &base,
                &cone,
                None,
                Some(&active_u),
                &loop_nodes,
            );
            let without_u = settle(
                &input.state,
                &input.worth,
                &inc,
                &base,
                &cone,
                Some(u),
                Some(&empty_harms),
                &loop_nodes,
            );
            match (with_u, without_u) {
                (Ok(wu), Ok(wou)) => {
                    let mut d = BTreeMap::new();
                    for t in &priced {
                        let v_wu = wu.get(t).copied().unwrap_or(0.0);
                        let v_wou = wou.get(t).copied().unwrap_or(0.0);
                        d.insert(t.clone(), v_wu - v_wou);
                    }
                    Ok(d)
                }
                _ => Err(NoConvergence),
            }
        } else {
            let ko = settle(
                &input.state,
                &input.worth,
                &inc,
                &base,
                &cone,
                Some(u),
                Some(&empty_harms),
                &loop_nodes,
            );
            match ko {
                Ok(k) => {
                    let mut d = BTreeMap::new();
                    for t in &priced {
                        let v_base = base.get(t).copied().unwrap_or(1.0);
                        let v_ko = k.get(t).copied().unwrap_or(0.0);
                        d.insert(t.clone(), v_base - v_ko);
                    }
                    Ok(d)
                }
                Err(e) => Err(e),
            }
        };

        match deltas_res {
            Ok(deltas) => {
                let mut gain = 0.0;
                let mut loss = 0.0;
                for (t, &d) in &deltas {
                    let wt = input.worth.get(t).copied().unwrap_or(0.0);
                    if wt > 0.0 && d > 0.0 {
                        gain += wt * d;
                    }
                    if wt < 0.0 {
                        loss += -wt * d;
                    }
                    if wt > 0.0 && d < 0.0 {
                        loss += wt * d;
                    }
                }
                let cleaned_gain = clean_num(gain);
                let cleaned_loss = clean_num(loss);
                let cleaned_deltas: BTreeMap<String, f64> = deltas
                    .into_iter()
                    .map(|(t, d)| (t, clean_num(d)))
                    .collect();
                let stake = cleaned_deltas.clone();

                result.insert(
                    u.clone(),
                    FlowOutput {
                        gain: Some(cleaned_gain),
                        loss_averted: Some(cleaned_loss),
                        deltas: cleaned_deltas,
                        stake,
                        decision_value: Some(0.0),
                        loop_extra: BTreeMap::new(),
                        flow_status: FlowStatus::Ok,
                        loop_nodes: None,
                    },
                );
            }
            Err(_) => {
                let mut unsettled: Vec<String> =
                    cone.iter().filter(|v| loop_nodes.contains(*v)).cloned().collect();
                unsettled.sort();
                result.insert(
                    u.clone(),
                    FlowOutput {
                        gain: None,
                        loss_averted: None,
                        deltas: BTreeMap::new(),
                        stake: BTreeMap::new(),
                        decision_value: None,
                        loop_extra: BTreeMap::new(),
                        flow_status: FlowStatus::NoConvergence,
                        loop_nodes: Some(unsettled),
                    },
                );
            }
        }
    }

    result
}

// ── Decision Rule (EVPI) ─────────────────────────────────────────────────────

/// Howard's expected best utility: E[max_i X_i w_i, 0] for independent X_i ~ Bernoulli(p_i).
pub fn expected_best(mut options: Vec<(f64, f64)>) -> f64 {
    options.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut total = 0.0;
    let mut none_better = 1.0;
    for (w, p) in options {
        if w <= 0.0 {
            break;
        }
        total += none_better * p * w;
        none_better *= 1.0 - p;
    }
    total
}

/// Howard's expected value of perfect information (EVPI).
pub fn evpi(options: &[(f64, f64)]) -> f64 {
    let mut blind = 0.0;
    for &(w, p) in options {
        let expected = p * w;
        if expected > blind {
            blind = expected;
        }
    }
    expected_best(options.to_vec()) - blind
}

/// Compute decision value on work settling open decisions (R14, §5.3).
pub fn compute_decision_value(
    input: &FlowInput,
    outputs: &mut BTreeMap<String, FlowOutput>,
) {
    let mut extra: BTreeMap<String, f64> = BTreeMap::new();

    for (d, st) in &input.state {
        if *st != FlowState::Open {
            continue;
        }
        let mut opts = Vec::new();
        for e in &input.edges {
            if &e.dst == d && e.label == "alternative" {
                if let Some(out) = outputs.get(&e.src) {
                    if let (Some(g), Some(l)) = (out.gain, out.loss_averted) {
                        opts.push((g + l, e.probability));
                    }
                }
            }
        }
        if opts.len() < 2 {
            continue;
        }
        let value = evpi(&opts);
        for e in &input.edges {
            if &e.dst == d
                && e.label == "settles"
                && input.state.get(&e.src) == Some(&FlowState::Open)
            {
                *extra.entry(e.src.clone()).or_insert(0.0) += e.quantum * value;
            }
        }
    }

    for (node, out) in outputs.iter_mut() {
        if out.flow_status == FlowStatus::Ok {
            let add = extra.get(node).copied().unwrap_or(0.0);
            let prev = out.decision_value.unwrap_or(0.0);
            out.decision_value = Some(clean_num(prev + add));
        } else {
            out.decision_value = None;
        }
    }
}

// ── Routes and Explanation ───────────────────────────────────────────────────

/// A simple route from work to a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub path: Vec<String>,
    pub strength: f64,
    pub labels: Vec<String>,
}

/// Discovered routes and diagnostics for a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetRoutes {
    pub routes: Vec<Route>,
    pub routes_truncated: bool,
    pub loop_extra: Option<f64>,
}

/// Compute on-demand routes and loop extras for node `u` (R18, §5.5).
pub fn compute_routes(
    input: &FlowInput,
    output: &FlowOutput,
    u: &str,
) -> BTreeMap<String, TargetRoutes> {
    let mut res = BTreeMap::new();
    let (_, out) = build_indices(&input.edges, &input.state);

    for (t, &dlt) in &output.deltas {
        if dlt.abs() < 1e-9 {
            continue;
        }

        // Depth-first search for simple routes from u to t
        let mut found_routes: Vec<Route> = Vec::new();

        fn walk(
            v: &str,
            target: &str,
            path: &mut Vec<String>,
            labels: &mut Vec<String>,
            strength: f64,
            out: &BTreeMap<String, Vec<&FlowEdge>>,
            state: &BTreeMap<String, FlowState>,
            worth: &BTreeMap<String, f64>,
            found: &mut Vec<Route>,
        ) {
            if v == target && path.len() > 1 {
                found.push(Route {
                    path: path.clone(),
                    strength: clean_num(strength),
                    labels: labels.clone(),
                });
                return;
            }
            if path.len() > 12 {
                return;
            }
            if let Some(edges) = out.get(v) {
                for e in edges {
                    if path.contains(&e.dst) {
                        continue;
                    }
                    if state.get(&e.dst) == Some(&FlowState::Done) && e.dst != target {
                        continue;
                    }
                    let sign = if edge_helps(e, worth) { 1.0 } else { -1.0 };
                    path.push(e.dst.clone());
                    labels.push(e.label.clone());
                    walk(
                        &e.dst,
                        target,
                        path,
                        labels,
                        strength * sign * e.strength(),
                        out,
                        state,
                        worth,
                        found,
                    );
                    path.pop();
                    labels.pop();
                }
            }
        }

        if u == t {
            found_routes.push(Route {
                path: vec![u.to_string()],
                strength: 1.0,
                labels: vec![],
            });
        } else {
            let mut path = vec![u.to_string()];
            let mut labels = Vec::new();
            walk(
                u,
                t,
                &mut path,
                &mut labels,
                1.0,
                &out,
                &input.state,
                &input.worth,
                &mut found_routes,
            );
        }

        found_routes.sort_by(|a, b| {
            b.strength
                .abs()
                .partial_cmp(&a.strength.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });

        // Compute loop-free bound: 1 - prod_{s} (1 - s)
        let mut hi = 1.0;
        for r in &found_routes {
            hi *= 1.0 - r.strength;
        }
        let bound = clean_num((1.0 - hi).min(1.0));
        let loop_extra = if dlt > bound + 1e-9 {
            Some(clean_num(dlt - bound))
        } else {
            None
        };

        let truncated = found_routes.len() > ROUTE_CAP;
        if found_routes.len() > ROUTE_CAP {
            found_routes.truncate(ROUTE_CAP);
        }

        res.insert(
            t.clone(),
            TargetRoutes {
                routes: found_routes,
                routes_truncated: truncated,
                loop_extra,
            },
        );
    }

    res
}

/// Build one-sentence explanation from strongest routes (R18, §5.5).
pub fn build_explanation(
    u: &str,
    target_routes: &BTreeMap<String, TargetRoutes>,
    stake: &BTreeMap<String, f64>,
) -> String {
    if target_routes.is_empty() {
        return format!("{u} serves no priced targets");
    }

    let mut parts = Vec::new();
    for (t, tr) in target_routes {
        let d = stake.get(t).copied().unwrap_or(0.0);
        let path_str = tr
            .routes
            .first()
            .map(|r| r.path.join(" > "))
            .unwrap_or_else(|| t.clone());
        parts.push(format!("{t} x {d:.2} via {path_str}"));
    }

    format!("{u}: {}", parts.join("; "))
}

// ── Export / JSON Loader ─────────────────────────────────────────────────────

/// Load a `FlowInput` from export JSON data.
pub fn from_export_json(data: &serde_json::Value, default_quantum: f64, part_of_quantum: Option<f64>) -> FlowInput {
    let mut input = FlowInput::new();

    if let Some(nodes) = data.get("nodes").and_then(|n| n.as_array()) {
        for n in nodes {
            let id = n.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let state = if let Some(st) = n.get("state").and_then(|v| v.as_str()) {
                match st {
                    "gone" => FlowState::Gone,
                    "done" => FlowState::Done,
                    _ => FlowState::Open,
                }
            } else {
                status_to_flow_state(n.get("status").and_then(|v| v.as_str()))
            };
            let worth = n
                .get("worth")
                .or_else(|| n.get("standing_weight"))
                .and_then(|v| v.as_f64());
            input.add_node(id, state, worth);
        }
    }

    if let Some(edges) = data.get("edges").and_then(|e| e.as_array()) {
        for item in edges {
            if let Some(arr) = item.as_array() {
                if arr.len() >= 3 {
                    let src = arr[0].as_str().unwrap_or_default();
                    let dst = arr[1].as_str().unwrap_or_default();
                    let label = arr[2].as_str().unwrap_or_default();

                    if label == "relates" {
                        continue;
                    }

                    let raw_q = arr.get(3).and_then(|v| v.as_f64());
                    let q = if label == "part_of" {
                        if let Some(poq) = part_of_quantum {
                            poq
                        } else {
                            raw_q.unwrap_or(default_quantum)
                        }
                    } else {
                        raw_q.unwrap_or(default_quantum)
                    };

                    let eff = match arr.get(4).and_then(|v| v.as_str()) {
                        Some("harms") => FlowEffect::Harms,
                        _ => FlowEffect::Helps,
                    };

                    let prob = arr.get(5).and_then(|v| v.as_f64()).unwrap_or(1.0);

                    input.add_edge(FlowEdge {
                        src: src.to_string(),
                        dst: dst.to_string(),
                        label: label.to_string(),
                        quantum: q,
                        probability: prob,
                        effect: eff,
                        unvalued: raw_q.is_none(),
                    });
                }
            }
        }
    }

    input
}
