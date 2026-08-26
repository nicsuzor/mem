//! Ranking Regression Test Harness — Phase 0c ([[mem_07ca70cf]])
//!
//! Provides an immutable frozen corpus fixture (`ranking_corpus_20260824.json`),
//! distribution validation against dossier §3, stable diffable ranking generation,
//! and evaluation of the four core prioritisation regression properties.

use std::collections::HashMap;
use std::path::Path;

use mem::graph::GraphNode;
use mem::graph_store::GraphStore;
use serde::{Deserialize, Serialize};

/// Metadata stored alongside the frozen corpus fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusMetadata {
    pub corpus_id: String,
    pub date: String,
    pub source_commit: String,
    pub node_count: usize,
    pub task_count: usize,
    pub sampled_task_count: usize,
    pub ready_count: usize,
    pub blocked_count: usize,
    pub privacy_assessment: String,
}

/// The serialized corpus fixture container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFixture {
    pub metadata: CorpusMetadata,
    pub nodes: Vec<GraphNode>,
}

/// A scored task entry for diffable ranking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedTask {
    pub id: String,
    pub label: String,
    pub status: String,
    pub priority: Option<i32>,
    pub severity: Option<i32>,
    pub focus_score: i64,
    pub voi_value: f64,
    pub downstream_weight: f64,
    pub urgency: f64,
    pub uncertainty: f64,
}

/// Order diff summary between two ranking runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderDiff {
    pub total_tasks: usize,
    pub moved_count: usize,
    pub max_displacement: usize,
    pub top_20_entered: Vec<(usize, RankedTask)>,
    pub top_20_exited: Vec<(usize, RankedTask)>,
    pub movements: Vec<TaskMovement>,
}

/// Movement of a single task between two rankings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMovement {
    pub id: String,
    pub label: String,
    pub old_rank: usize,
    pub new_rank: usize,
    pub delta: isize,
    pub old_score: i64,
    pub new_score: i64,
}

/// Load the immutable frozen ranking corpus.
pub fn load_frozen_corpus() -> CorpusFixture {
    let fixture_str = include_str!("fixtures/ranking_corpus_20260824.json");
    serde_json::from_str(fixture_str).expect("Failed to deserialize frozen ranking corpus fixture")
}

/// Build graph and score tasks from the corpus nodes.
pub fn score_corpus(mut nodes: Vec<GraphNode>) -> (GraphStore, Vec<RankedTask>) {
    let dummy_root = Path::new("/tmp/pkb_corpus_root");
    let mut node_map = HashMap::with_capacity(nodes.len());
    for n in &mut nodes {
        if n.permalinks.is_empty() {
            n.permalinks.push(n.id.trim().to_lowercase());
            if let Some(ref pl) = n.permalink {
                n.permalinks.push(pl.trim().to_lowercase());
            }
        }
        if n.weak_keys.is_empty() {
            if let Some(stem) = n.path.file_stem().and_then(|s| s.to_str()) {
                n.weak_keys.push(stem.to_lowercase());
            }
        }
        node_map.insert(n.id.clone(), n.clone());
    }
    let graph = GraphStore::rebuild_from_nodes(node_map, dummy_root);

    let mut ranked = Vec::new();
    for node in graph.ready_tasks().into_iter().chain(graph.blocked_tasks().into_iter()) {
        ranked.push(RankedTask {
            id: node.id.clone(),
            label: node.label.clone(),
            status: node.status.clone().unwrap_or_else(|| "ready".to_string()),
            priority: node.priority,
            severity: node.severity,
            focus_score: node.focus_score.unwrap_or(0),
            voi_value: node.voi_value.unwrap_or(0.0),
            downstream_weight: node.downstream_weight,
            urgency: node.urgency,
            uncertainty: node.uncertainty,
        });
    }

    ranked.sort_by(|a, b| {
        b.focus_score
            .cmp(&a.focus_score)
            .then_with(|| a.id.cmp(&b.id))
    });

    (graph, ranked)
}

/// Compute order diff between a baseline ranking and a candidate ranking.
pub fn compute_order_diff(baseline: &[RankedTask], candidate: &[RankedTask]) -> OrderDiff {
    let baseline_ranks: HashMap<&str, (usize, &RankedTask)> = baseline
        .iter()
        .enumerate()
        .map(|(idx, t)| (t.id.as_str(), (idx + 1, t)))
        .collect();

    let mut moved_count = 0;
    let mut max_displacement = 0;
    let mut movements = Vec::new();

    for (c_idx, c_task) in candidate.iter().enumerate() {
        let new_rank = c_idx + 1;
        if let Some(&(old_rank, b_task)) = baseline_ranks.get(c_task.id.as_str()) {
            if old_rank != new_rank || b_task.focus_score != c_task.focus_score {
                let disp = if old_rank > new_rank {
                    old_rank - new_rank
                } else {
                    new_rank - old_rank
                };
                if disp > max_displacement {
                    max_displacement = disp;
                }
                moved_count += 1;
                movements.push(TaskMovement {
                    id: c_task.id.clone(),
                    label: c_task.label.clone(),
                    old_rank,
                    new_rank,
                    delta: (old_rank as isize) - (new_rank as isize),
                    old_score: b_task.focus_score,
                    new_score: c_task.focus_score,
                });
            }
        }
    }

    let top_20_baseline: HashMap<&str, usize> = baseline
        .iter()
        .take(20)
        .enumerate()
        .map(|(idx, t)| (t.id.as_str(), idx + 1))
        .collect();

    let top_20_candidate: HashMap<&str, usize> = candidate
        .iter()
        .take(20)
        .enumerate()
        .map(|(idx, t)| (t.id.as_str(), idx + 1))
        .collect();

    let mut top_20_entered = Vec::new();
    for (idx, task) in candidate.iter().take(20).enumerate() {
        if !top_20_baseline.contains_key(task.id.as_str()) {
            top_20_entered.push((idx + 1, task.clone()));
        }
    }

    let mut top_20_exited = Vec::new();
    for (idx, task) in baseline.iter().take(20).enumerate() {
        if !top_20_candidate.contains_key(task.id.as_str()) {
            top_20_exited.push((idx + 1, task.clone()));
        }
    }

    OrderDiff {
        total_tasks: candidate.len(),
        moved_count,
        max_displacement,
        top_20_entered,
        top_20_exited,
        movements,
    }
}

/// Format order diff as a markdown table suitable for PR attachment.
pub fn format_order_diff(diff: &OrderDiff) -> String {
    let mut out = String::new();
    out.push_str("### Ranking Order Diff Report\n\n");
    out.push_str(&format!(
        "- **Total evaluated tasks:** {}\n",
        diff.total_tasks
    ));
    out.push_str(&format!(
        "- **Moved tasks:** {} (max displacement: {})\n",
        diff.moved_count, diff.max_displacement
    ));
    out.push_str(&format!(
        "- **Top-20 entries:** {} | **Top-20 exits:** {}\n\n",
        diff.top_20_entered.len(),
        diff.top_20_exited.len()
    ));

    if !diff.top_20_entered.is_empty() {
        out.push_str("#### Top-20 Entered\n");
        for (rank, t) in &diff.top_20_entered {
            out.push_str(&format!(
                "- `#{:02}` `{}` (score: {}) — {}\n",
                rank, t.id, t.focus_score, t.label
            ));
        }
        out.push('\n');
    }

    if !diff.top_20_exited.is_empty() {
        out.push_str("#### Top-20 Exited\n");
        for (rank, t) in &diff.top_20_exited {
            out.push_str(&format!(
                "- `#{:02}` `{}` (score: {}) — {}\n",
                rank, t.id, t.focus_score, t.label
            ));
        }
        out.push('\n');
    }

    if !diff.movements.is_empty() {
        out.push_str("#### Sample Movements (Top 10 displaced)\n");
        let mut sorted_mov = diff.movements.clone();
        sorted_mov.sort_by_key(|m| -(m.delta.abs()));
        for m in sorted_mov.iter().take(10) {
            let arrow = if m.delta > 0 { "↑" } else { "↓" };
            out.push_str(&format!(
                "- {} `{}`: #{} → #{} (Δ{:+} | score: {} → {}) — {}\n",
                arrow, m.id, m.old_rank, m.new_rank, m.delta, m.old_score, m.new_score, m.label
            ));
        }
    } else {
        out.push_str("*(No task rank movements detected — rankings are identical)*\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frozen_corpus_loads_and_reproduces_distribution() {
        let fixture = load_frozen_corpus();
        assert_eq!(fixture.metadata.corpus_id, "ranking_corpus_20260824");
        assert_eq!(fixture.metadata.date, "2026-08-24");
        assert_eq!(fixture.metadata.source_commit, "1ee69e3");
        assert!(fixture.nodes.len() >= 6226);

        let (_graph, ranked) = score_corpus(fixture.nodes);
        assert!(!ranked.is_empty());

        let n = ranked.len() as f64;
        let non_zero_focus = ranked.iter().filter(|t| t.focus_score > 0).count() as f64 / n;
        let non_zero_voi = ranked.iter().filter(|t| t.voi_value > 0.0).count() as f64 / n;
        let non_zero_dw = ranked.iter().filter(|t| t.downstream_weight > 0.0).count() as f64 / n;
        let non_zero_urgency = ranked.iter().filter(|t| t.urgency > 0.0).count() as f64 / n;
        let non_zero_uncertainty = ranked.iter().filter(|t| t.uncertainty > 0.0).count() as f64 / n;

        let max_focus = ranked.iter().map(|t| t.focus_score).max().unwrap_or(0);
        let max_voi = ranked
            .iter()
            .map(|t| t.voi_value)
            .fold(0.0, f64::max);
        let max_dw = ranked
            .iter()
            .map(|t| t.downstream_weight)
            .fold(0.0, f64::max);
        let max_urgency = ranked.iter().map(|t| t.urgency).fold(0.0, f64::max);

        println!("Evaluated {} ready/blocked tasks:", ranked.len());
        println!("  focus_score non-zero: {:.1}%, max: {}", non_zero_focus * 100.0, max_focus);
        println!("  voi_value non-zero: {:.1}%, max: {}", non_zero_voi * 100.0, max_voi);
        println!("  downstream_weight non-zero: {:.1}%, max: {:.2}", non_zero_dw * 100.0, max_dw);
        println!("  urgency non-zero: {:.1}%, max: {}", non_zero_urgency * 100.0, max_urgency);
        println!("  uncertainty non-zero: {:.1}%", non_zero_uncertainty * 100.0);

        assert!(non_zero_focus > 0.40, "non-zero focus proportion: {}", non_zero_focus);
        assert!(non_zero_dw > 0.05, "non-zero dw proportion: {}", non_zero_dw);
        assert_eq!(non_zero_urgency, 1.0, "urgency is always non-zero (0.001 floor)");
        assert!(non_zero_uncertainty > 0.50, "non-zero uncertainty proportion: {}", non_zero_uncertainty);

        assert!(max_focus >= 11000, "max focus_score: {}", max_focus);
        assert!((max_dw - 5.27).abs() < 0.5, "max downstream_weight should reproduce ~5.27, got {}", max_dw);
        assert_eq!(max_urgency, 900.0, "max urgency should reproduce 900.0");
    }

    #[test]
    fn test_stable_diffable_ranking_and_self_diff() {
        let fixture = load_frozen_corpus();
        let (_graph1, ranked1) = score_corpus(fixture.nodes.clone());
        let (_graph2, ranked2) = score_corpus(fixture.nodes);

        assert_eq!(ranked1, ranked2, "Scoring must be deterministic");

        let diff = compute_order_diff(&ranked1, &ranked2);
        assert_eq!(diff.moved_count, 0);
        assert_eq!(diff.max_displacement, 0);
        assert!(diff.top_20_entered.is_empty());
        assert!(diff.top_20_exited.is_empty());

        let report = format_order_diff(&diff);
        assert!(report.contains("No task rank movements detected"));
    }

    /// Property 1: Overdue monotonicity
    /// A task ranks at least as high the day after its deadline as the day before.
    /// Owned by Phase 0a ([[mem_8a59ccfd]]).
    #[test]
    fn test_property_1_overdue_monotonicity_evaluated() {
        let fixture = load_frozen_corpus();
        let mut nodes = fixture.nodes;

        let today = chrono::Utc::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let tomorrow = today + chrono::Duration::days(1);

        let mut node_before = GraphNode::default();
        node_before.id = "prop1_task_before".to_string();
        node_before.label = "Prop1 Task Day Before".to_string();
        node_before.node_type = Some("task".to_string());
        node_before.task_id = Some("prop1_task_before".to_string());
        node_before.status = Some("ready".to_string());
        node_before.due = Some(tomorrow.format("%Y-%m-%d").to_string());
        node_before.effort = Some("1d".to_string());
        node_before.severity = Some(3);

        let mut node_after = GraphNode::default();
        node_after.id = "prop1_task_after".to_string();
        node_after.label = "Prop1 Task Day After".to_string();
        node_after.node_type = Some("task".to_string());
        node_after.task_id = Some("prop1_task_after".to_string());
        node_after.status = Some("ready".to_string());
        node_after.due = Some(yesterday.format("%Y-%m-%d").to_string());
        node_after.effort = Some("1d".to_string());
        node_after.severity = Some(3);

        nodes.push(node_before);
        nodes.push(node_after);

        let (_graph, ranked) = score_corpus(nodes);
        let pos_before = ranked.iter().position(|t| t.id == "prop1_task_before");
        let pos_after = ranked.iter().position(|t| t.id == "prop1_task_after");

        assert!(pos_before.is_some() && pos_after.is_some(), "Both tasks must be in ranked list");
        let rank_before = pos_before.unwrap();
        let rank_after = pos_after.unwrap();

        assert!(rank_after <= rank_before || ranked[rank_after].focus_score >= ranked[rank_before].focus_score,
            "Overdue task score ({}) should be >= day before score ({})",
            ranked[rank_after].focus_score, ranked[rank_before].focus_score);
    }

    /// Property 2: Honest effort is not punished
    /// Adding a truthful `effort` to a task never lowers that task's own rank.
    /// Owned by Phases 1–2 ([[mem_e2b7cd99]]).
    /// Evaluates effort sensitivity across deadline and slack terms (dossier §6 V3).
    #[test]
    fn test_property_2_honest_effort_punishment_evaluated() {
        let fixture = load_frozen_corpus();
        let mut nodes1 = fixture.nodes.clone();
        let mut nodes2 = fixture.nodes;

        // Target a task with a deadline to observe effort inversion on slack / deadline score
        let today = chrono::Utc::now().date_naive();
        let due_date = today + chrono::Duration::days(5);

        let mut t1 = GraphNode::default();
        t1.id = "prop2_effort_unestimated".to_string();
        t1.label = "Effort Test Unestimated".to_string();
        t1.node_type = Some("task".to_string());
        t1.task_id = Some("prop2_effort_unestimated".to_string());
        t1.status = Some("ready".to_string());
        t1.due = Some(due_date.format("%Y-%m-%d").to_string());
        t1.effort = None; // unestimated default 3d -> ratio 3/5 = 0.6 -> deadline_score = 2800

        let mut t2 = GraphNode::default();
        t2.id = "prop2_effort_truthful_1d".to_string();
        t2.label = "Effort Test Truthful 1d".to_string();
        t2.node_type = Some("task".to_string());
        t2.task_id = Some("prop2_effort_truthful_1d".to_string());
        t2.status = Some("ready".to_string());
        t2.due = Some(due_date.format("%Y-%m-%d").to_string());
        t2.effort = Some("1d".to_string()); // truthful 1d -> ratio 1/5 = 0.2 -> deadline_score = 1000

        nodes1.push(t1);
        nodes2.push(t2);

        let (_g1, r1) = score_corpus(nodes1);
        let (_g2, r2) = score_corpus(nodes2);

        let s1 = r1.iter().find(|t| t.id == "prop2_effort_unestimated").unwrap();
        let s2 = r2.iter().find(|t| t.id == "prop2_effort_truthful_1d").unwrap();

        println!("Prop 2 check: unestimated score={}, truthful 1d score={}", s1.focus_score, s2.focus_score);
        // On today's model, estimating honest small effort (1d) produces a LOWER score than leaving it unestimated (3d)!
        // This confirms the perverse incentive (dossier §6 V3) where honest estimation is punished.
        assert!(s1.focus_score > s2.focus_score, "Known defect confirmed: honest low effort is punished with lower score (targets Phase 1-2)");
    }

    /// Property 3: Settled questions do not score as open ones
    /// Craft-glue case leaves the top 20.
    /// Owned by Phase 3 ([[task_e9667ac7]]).
    /// Evaluates whether closed/settled tasks still receive artificial VoI bonuses due to missing AC regex.
    #[test]
    fn test_property_3_craft_glue_settled_evaluated() {
        let fixture = load_frozen_corpus();
        let (_graph, ranked) = score_corpus(fixture.nodes);

        let craft_glue = ranked.iter().enumerate().find(|(_, t)| t.id == "brain_8e7dcf7a");
        assert!(craft_glue.is_some(), "Craft-glue task brain_8e7dcf7a must be present in corpus");

        let (idx, task) = craft_glue.unwrap();
        let rank = idx + 1;
        println!("Craft glue task rank: #{}, voi_value: {}, uncertainty: {}", rank, task.voi_value, task.uncertainty);

        // At HEAD, craft-glue still scores voi_value > 1000 and uncertainty = 0.30 purely because of missing AC header regex
        // even though its body states "## Adhesive — settled".
        // Phase 3 will replace the regex proxy with honest uncertainty, driving this task's VoI to 0.
        assert!(task.voi_value > 1000.0, "Known defect confirmed: settled craft-glue receives VoI bonus ({}) due to regex proxy (targets Phase 3)", task.voi_value);
        assert_eq!(task.uncertainty, 0.3, "Uncertainty is 0.3 from missing AC regex despite settled question");
    }

    /// Property 4: Importance channel dynamic range
    /// `contributes_to` moves rankings by more than the ~53 points it can move them today.
    /// Owned by Phase 2.
    /// Confirms that downstream_weight * 10 maxes out at ~52.7 points (< 0.5% dynamic range).
    #[test]
    fn test_property_4_importance_dynamic_range_fails_today() {
        let fixture = load_frozen_corpus();
        let (_graph, ranked) = score_corpus(fixture.nodes);

        let max_dw = ranked
            .iter()
            .map(|t| t.downstream_weight)
            .fold(0.0, f64::max);

        let max_dw_points = (max_dw * 10.0) as i64;

        assert!(max_dw_points <= 53, "Max DW points is {} <= 53 (Target of Phase 2 to give real dynamic range)", max_dw_points);
    }
}
