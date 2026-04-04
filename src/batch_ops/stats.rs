//! `graph_stats` — read-only graph health report.

use crate::graph::is_completed;
use crate::graph_store::{GraphStore, ACTIONABLE_TYPES};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const ACTIONABLE_FLAT_TYPES: &[&str] = &["task", "epic"];

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
    /// Epics not connected to a project or goal via their ancestor chain
    pub disconnected_epics: usize,
    /// Projects not linked to any goal
    pub projects_without_goals: usize,
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

    for node in graph.nodes() {
        let node_type = node.node_type.as_deref().unwrap_or("unknown");

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

        // Disconnected epics: traverse full parent chain looking for project or goal ancestor
        if node_type == "epic" {
            if !has_ancestor_of_type(graph, node, &["project", "goal"]) {
                disconnected_epics += 1;
            }
        }

        // Projects without goals: traverse full parent chain for goal ancestor
        if node_type == "project" {
            if !has_ancestor_of_type(graph, node, &["goal"]) {
                projects_without_goals += 1;
            }
        }
    }

    // Compute deterministic hash for convergence detection.
    // Uses health indicators (not counts that change with normal work like done/active).
    let hash_input = format!(
        "orphan={},flat={},disconnected_epics={},projects_wo_goals={},max_depth={},stale={}",
        orphan_count, flat_tasks, disconnected_epics, projects_without_goals, max_depth, stale_count
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
