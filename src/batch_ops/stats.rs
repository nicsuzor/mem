//! `graph_stats` — read-only graph health report.

use crate::graph::is_completed;
use crate::graph_store::{GraphStore, ACTIONABLE_TYPES};
use serde::Serialize;
use std::collections::HashMap;

/// Graph health statistics.
#[derive(Debug, Serialize)]
pub struct GraphStats {
    /// Count by status
    pub status_counts: HashMap<String, usize>,
    /// Total actionable tasks
    pub total_tasks: usize,
    /// Tasks with no parent and no project
    pub orphan_count: usize,
    /// Deepest nesting level
    pub max_depth: usize,
    /// Count of leaf tasks directly under a project (no epic grouping)
    pub flat_tasks: usize,
    /// Tasks not modified in >60 days
    pub stale_count: usize,
    /// Count per priority level
    pub priority_distribution: HashMap<String, usize>,
    /// Count per document type
    pub type_distribution: HashMap<String, usize>,
    /// Epics with no children
    pub disconnected_epics: usize,
    /// Projects not linked to any goal
    pub projects_without_goals: usize,
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
                "1" => "P1 (high)",
                "2" => "P2 (medium)",
                "3" => "P3 (low)",
                "4" => "P4 (someday)",
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

        // Depth
        let depth = node.depth as usize;
        if depth > max_depth {
            max_depth = depth;
        }

        // Flat tasks: leaf tasks whose parent is a project (not an epic)
        if node.leaf && node.children.is_empty() {
            if let Some(ref parent_id) = node.parent {
                if let Some(parent) = graph.get_node(parent_id) {
                    if parent.node_type.as_deref() == Some("project") {
                        flat_tasks += 1;
                    }
                }
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

        // Disconnected epics (epics with no children)
        if node_type == "epic" && node.children.is_empty() {
            disconnected_epics += 1;
        }

        // Projects without goals (projects with no parent, or parent isn't a goal)
        if node_type == "project" {
            let has_goal_parent = node.parent.as_ref().and_then(|pid| {
                graph.get_node(pid)
            }).map(|p| p.node_type.as_deref() == Some("goal")).unwrap_or(false);
            if !has_goal_parent {
                projects_without_goals += 1;
            }
        }
    }

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
