//! `graph_stats` — read-only graph health report.

use crate::graph::is_completed;
use crate::graph_store::{GraphStore, ACTIONABLE_TYPES};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const ACTIONABLE_FLAT_TYPES: &[&str] = &["task", "epic"];

/// Model B connectivity: returns true iff `node` or any node in its work-subtree
/// (children, recursively) has a `contributes_to` edge whose destination resolves
/// to a `target` or `goal` node. This is the Model-B replacement for parent-ancestry
/// "connection": work joins the strategy tier via `contributes_to`, never via a
/// structural parent. Goals & targets are out of the work tree.
fn subtree_contributes_to_outcome(
    graph: &GraphStore,
    node: &crate::graph::GraphNode,
    visited: &mut HashSet<String>,
) -> bool {
    if !visited.insert(node.id.clone()) {
        return false;
    }
    // Direct contribution: does this node contribute to a target or goal?
    for ct in &node.contributes_to {
        let dest = ct.resolved_to.clone().unwrap_or_else(|| ct.to.clone());
        if let Some(dest_node) = graph.resolve(&dest) {
            if crate::graph::is_strategic_target(dest_node.node_type.as_deref()) {
                return true;
            }
        }
    }
    // Transitive: recurse through child work nodes.
    for child_id in &node.children {
        if let Some(child) = graph.get_node(child_id) {
            if subtree_contributes_to_outcome(graph, child, visited) {
                return true;
            }
        }
    }
    false
}

/// Walk the parent chain looking for an ancestor whose type is in `types`.
fn has_ancestor_of_type(graph: &GraphStore, node: &crate::graph::GraphNode, types: &[&str]) -> bool {
    let mut current_id = node.parent.clone();
    let mut visited = HashSet::new();
    while let Some(ref pid) = current_id {
        if !visited.insert(pid.clone()) {
            break;
        }
        if let Some(ancestor) = graph.get_node(pid) {
            if let Some(ancestor_type) = ancestor.node_type.as_deref() {
                if types.contains(&ancestor_type) {
                    return true;
                }
            }
            current_id = ancestor.parent.clone();
        } else {
            break;
        }
    }
    false
}

/// Graph health statistics.
#[derive(Debug, Serialize)]
pub struct GraphStats {
    /// Count by status
    pub status_counts: HashMap<String, usize>,
    /// Total actionable tasks
    pub total_tasks: usize,
    /// Tasks with no parent and no project
    pub orphan_count: usize,
    /// Deepest parent-chain nesting level (computed by walking parent links)
    pub max_depth: usize,
    /// Actionable nodes (task/bug/feature/action/epic) with no parent AND no children, excluding completed
    pub flat_tasks: usize,
    /// Tasks not modified in >60 days
    pub stale_count: usize,
    /// Count per priority level
    pub priority_distribution: HashMap<String, usize>,
    /// Count per document type
    pub type_distribution: HashMap<String, usize>,
    /// Epics not connected (Model B) to a `target`/`goal` via a `contributes_to`
    /// edge from the epic or any node in its work-subtree.
    pub disconnected_epics: usize,
    /// Projects not linked to any goal
    pub projects_without_goals: usize,
    /// Projects with the explicit `goals` field populated
    pub projects_with_goals_field: usize,
    /// Total number of projects
    pub total_projects: usize,
    /// Hard cycles detected in the DependsOn+Parent subgraph via Tarjan's SCC.
    /// Each inner Vec contains the node IDs forming one cycle.
    /// An empty outer Vec means no hard cycles exist.
    pub hard_cycles: Vec<Vec<String>>,
    /// Count of SCCs with size > 1 in the SoftDependsOn subgraph.
    /// Soft cycles are healthy (mutual reinforcement) — counted but not flagged.
    pub soft_cycle_count: usize,
    /// Deterministic hash of health metrics for convergence detection.
    /// Compare with previous run's hash — if equal, the graph has stabilized.
    pub metrics_hash: String,
}

impl GraphStats {
    /// Format as human-readable report.
    pub fn display(&self) -> String {
        let mut out = String::new();
        out.push_str("Graph Health Report\n");
        out.push_str(&format!("{}\n\n", "═".repeat(40)));

        out.push_str(&format!("Total actionable tasks: {}\n\n", self.total_tasks));

        // Status breakdown
        out.push_str("Status breakdown:\n");
        let mut statuses: Vec<_> = self.status_counts.iter().collect();
        statuses.sort_by(|a, b| b.1.cmp(a.1));
        for (status, count) in &statuses {
            out.push_str(&format!("  {:<16} {}\n", status, count));
        }

        // Priority breakdown
        out.push_str("\nPriority breakdown:\n");
        let mut priorities: Vec<_> = self.priority_distribution.iter().collect();
        priorities.sort_by_key(|(k, _)| k.parse::<i32>().unwrap_or(99));
        for (priority, count) in &priorities {
            let label = match priority.as_str() {
                "0" => "P0 (critical)",
                "1" => "P1 (intended)",
                "2" => "P2 (active)",
                "3" => "P3 (planned)",
                "4" => "P4 (backlog)",
                _ => priority,
            };
            out.push_str(&format!("  {:<20} {}\n", label, count));
        }

        // Type breakdown
        out.push_str("\nType breakdown:\n");
        let mut types: Vec<_> = self.type_distribution.iter().collect();
        types.sort_by(|a, b| b.1.cmp(a.1));
        for (doc_type, count) in &types {
            out.push_str(&format!("  {:<16} {}\n", doc_type, count));
        }

        // Health indicators
        out.push_str("\nHealth indicators:\n");
        out.push_str(&format!("  Orphans:              {}\n", self.orphan_count));
        out.push_str(&format!("  Flat tasks:           {}\n", self.flat_tasks));
        out.push_str(&format!(
            "  Stale (>60d):         {}\n",
            self.stale_count
        ));
        out.push_str(&format!("  Max depth:            {}\n", self.max_depth));
        out.push_str(&format!(
            "  Empty epics:          {}\n",
            self.disconnected_epics
        ));
        out.push_str(&format!(
            "  Projects w/o goals:   {}\n",
            self.projects_without_goals
        ));
        out.push_str(&format!(
            "  Projects with goals:  {} / {}\n",
            self.projects_with_goals_field, self.total_projects
        ));
        out.push_str(&format!(
            "  Hard cycles:          {}\n",
            self.hard_cycles.len()
        ));
        if !self.hard_cycles.is_empty() {
            for cycle in &self.hard_cycles {
                out.push_str(&format!("    - [{}]\n", cycle.join(", ")));
            }
        }
        out.push_str(&format!(
            "  Soft cycle count:     {}\n",
            self.soft_cycle_count
        ));
        out.push_str(&format!(
            "  Metrics hash:         {}\n",
            self.metrics_hash
        ));

        out
    }
}

/// Compute graph health statistics.
pub fn graph_stats(graph: &GraphStore) -> GraphStats {
    let now = chrono::Utc::now().date_naive();
    let stale_threshold = 60i64;

    let mut status_counts: HashMap<String, usize> = HashMap::new();
    let mut priority_distribution: HashMap<String, usize> = HashMap::new();
    let mut type_distribution: HashMap<String, usize> = HashMap::new();
    let mut total_tasks = 0usize;
    let mut orphan_count = 0usize;
    let mut max_depth = 0usize;
    let mut flat_tasks = 0usize;
    let mut stale_count = 0usize;
    let mut disconnected_epics = 0usize;
    let mut projects_without_goals = 0usize;
    let mut projects_with_goals_field = 0usize;
    let mut total_projects = 0usize;

    for node in graph.nodes() {
        let node_type = node.node_type.as_deref().unwrap_or("unknown");

        // DEAD METRIC: `type: project` is retired and read-coerces to `epic` at
        // parse time, so node_type is never "project" and these three counters
        // (total_projects, projects_with_goals_field, projects_without_goals)
        // report 0 permanently. Kept only for output-schema stability; their
        // replacement (epics_without_areas et al.) is owned by the on-hold
        // areas migration — see specs/areas-not-projects.md, acceptance
        // criterion 8.
        if node_type == "project" {
            total_projects += 1;
            if !node.goals.is_empty() {
                projects_with_goals_field += 1;
            }
        }

        // Only count actionable types
        if !ACTIONABLE_TYPES.contains(&node_type) {
            // Still count type distribution
            *type_distribution
                .entry(node_type.to_string())
                .or_insert(0) += 1;
            continue;
        }

        total_tasks += 1;

        // Status
        let status = node.status.as_deref().unwrap_or("unknown");
        *status_counts.entry(status.to_string()).or_insert(0) += 1;

        // Skip completed for most metrics
        if is_completed(node.status.as_deref()) {
            *type_distribution
                .entry(node_type.to_string())
                .or_insert(0) += 1;
            continue;
        }

        // Priority
        let priority = node.priority.unwrap_or(2);
        *priority_distribution
            .entry(priority.to_string())
            .or_insert(0) += 1;

        // Type
        *type_distribution
            .entry(node_type.to_string())
            .or_insert(0) += 1;

        // Orphan: no parent
        if node.parent.is_none() {
            orphan_count += 1;
        }

        // Depth: walk parent chain to compute actual nesting depth
        let computed_depth = {
            let mut d = 0usize;
            let mut current_id = node.parent.clone();
            let mut visited = HashSet::new();
            while let Some(ref pid) = current_id {
                if !visited.insert(pid.clone()) {
                    break; // cycle guard
                }
                d += 1;
                current_id = graph.get_node(pid).and_then(|p| p.parent.clone());
            }
            d
        };
        if computed_depth > max_depth {
            max_depth = computed_depth;
        }

        // Flat tasks: actionable nodes with no parent AND no children
        if node.parent.is_none() && node.children.is_empty() {
            if ACTIONABLE_FLAT_TYPES.contains(&node_type) {
                flat_tasks += 1;
            }
        }

        // Stale check
        let date_str = node.modified.as_deref().or(node.created.as_deref());
        if let Some(ds) = date_str {
            if let Some(age_days) = parse_age_days(ds, now) {
                if age_days > stale_threshold {
                    stale_count += 1;
                }
            }
        }

        // Disconnected epics (Model B): an epic is connected iff it — or any node in
        // its work-subtree — has a `contributes_to` edge resolving to a `target` or
        // `goal`. Connection is via `contributes_to`, NOT parent-ancestry: goals and
        // targets live beside the work tree, never as parents.
        if node_type == "epic" {
            let mut visited = HashSet::new();
            if !subtree_contributes_to_outcome(graph, node, &mut visited) {
                disconnected_epics += 1;
            }
        }

        // Projects without goals: traverse full parent chain for goal ancestor.
        // DEAD METRIC — see the note on total_projects above; node_type is
        // never "project" post-retirement.
        if node_type == "project" {
            if !has_ancestor_of_type(graph, node, &["goal"]) {
                projects_without_goals += 1;
            }
        }
    }

    // Detect dependency cycles via Tarjan's SCC
    let hard_cycles = graph.find_hard_cycles();
    let soft_cycle_count = graph.find_soft_cycle_count();

    // Compute deterministic hash for convergence detection.
    // Uses health indicators (not counts that change with normal work like done/active).
    let hash_input = format!(
        "orphan={},flat={},disconnected_epics={},projects_wo_goals={},projects_w_goals={},max_depth={},stale={},hard_cycles={}",
        orphan_count, flat_tasks, disconnected_epics, projects_without_goals, projects_with_goals_field, max_depth, stale_count,
        hard_cycles.len()
    );
    let metrics_hash = format!("{:x}", {
        // Simple FNV-1a hash for deterministic, lightweight hashing
        let mut h: u64 = 0xcbf29ce484222325;
        for b in hash_input.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    });

    GraphStats {
        status_counts,
        total_tasks,
        orphan_count,
        max_depth,
        flat_tasks,
        stale_count,
        priority_distribution,
        type_distribution,
        disconnected_epics,
        projects_without_goals,
        projects_with_goals_field,
        total_projects,
        hard_cycles,
        soft_cycle_count,
        metrics_hash,
    }
}

/// Parse age in days from a date string.
fn parse_age_days(date_str: &str, now: chrono::NaiveDate) -> Option<i64> {
    let parsed = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        dt.date_naive()
    } else if let Ok(d) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        d
    } else if date_str.len() >= 10 {
        chrono::NaiveDate::parse_from_str(&date_str[..10], "%Y-%m-%d").ok()?
    } else {
        return None;
    };

    Some(now.signed_duration_since(parsed).num_days())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkb::PkbDocument;
    use std::path::{Path, PathBuf};

    /// Build a minimal PkbDocument for graph-stats tests.
    fn doc(
        path: &str,
        id: &str,
        doc_type: &str,
        parent: Option<&str>,
        contributes_to: Option<&str>,
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), serde_json::json!(id));
        fm.insert("type".to_string(), serde_json::json!(doc_type));
        fm.insert("status".to_string(), serde_json::json!("active"));
        fm.insert("id".to_string(), serde_json::json!(id));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), serde_json::json!(p));
        }
        if let Some(c) = contributes_to {
            fm.insert(
                "contributes_to".to_string(),
                serde_json::json!([{ "to": c, "weight": "Certain", "why": "test" }]),
            );
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: id.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some("active".to_string()),
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        }
    }

    /// Model B: an epic is connected iff it (or any node in its work-subtree) has a
    /// `contributes_to` edge resolving to a `target`/`goal`. Parent-ancestry under a
    /// goal/target no longer makes an epic "connected".
    #[test]
    fn disconnected_epics_uses_model_b_contributes_to_not_parent_ancestry() {
        let docs = vec![
            // Strategy tier — out of the work tree, never parents.
            doc("targets/target-1.md", "target-1", "target", None, None),
            doc("goals/goal-1.md", "goal-1", "goal", None, None),
            // Epic contributing directly to a target → CONNECTED.
            doc("tasks/epic-connected.md", "epic-connected", "epic", None, Some("target-1")),
            // Identical epic WITHOUT contributes_to → DISCONNECTED.
            doc("tasks/epic-disconnected.md", "epic-disconnected", "epic", None, None),
            // Epic with no contributes_to itself, but a child task that contributes
            // transitively → CONNECTED via the work-subtree.
            doc("tasks/epic-via-child.md", "epic-via-child", "epic", None, None),
            doc(
                "tasks/child-contrib.md",
                "child-contrib",
                "task",
                Some("epic-via-child"),
                Some("target-1"),
            ),
            // Epic parented UNDER a goal (parent-ancestry) but NO contributes_to →
            // DISCONNECTED. Proves parent-ancestry no longer changes the verdict.
            doc("tasks/epic-goal-parent.md", "epic-goal-parent", "epic", Some("goal-1"), None),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb-modelb"));
        let stats = graph_stats(&graph);

        // Disconnected: epic-disconnected + epic-goal-parent. Connected:
        // epic-connected (direct) + epic-via-child (transitive).
        assert_eq!(
            stats.disconnected_epics, 2,
            "Model B: only epics with no contributes_to→target/goal in their subtree are disconnected"
        );
    }
}
