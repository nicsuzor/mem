//! Shared filter DSL for batch operations.
//!
//! [`FilterSet`] provides composable, AND-logic filters over the task graph.
//! Within a single filter field, multiple values use OR logic (e.g. multiple IDs).

use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use chrono::{NaiveDate, Utc};
use std::collections::HashSet;

/// Composable filter set for selecting tasks from the graph.
///
/// All filters are AND-composed: a task must match every non-None filter.
/// Multiple values within a single field (e.g. `ids`, `tags`) use OR logic.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    /// Explicit task IDs (flexible resolution)
    pub ids: Option<Vec<String>>,
    /// Direct children of parent
    pub parent: Option<String>,
    /// All descendants (recursive)
    pub subtree: Option<String>,
    /// Match status
    pub status: Option<String>,
    /// Exact priority match
    pub priority: Option<i32>,
    /// Priority >= N
    pub priority_gte: Option<i32>,
    /// Has ALL listed tags
    pub tags: Option<Vec<String>>,
    /// Match document type
    pub doc_type: Option<String>,
    /// Days since `created`
    pub older_than_days: Option<u64>,
    /// Days since `modified`
    pub stale_days: Option<u64>,
    /// No parent
    pub orphan: Option<bool>,
    /// Substring match on title (case-insensitive)
    pub title_contains: Option<String>,
    /// Match assignee
    pub assignee: Option<String>,
    /// Match complexity tag
    pub complexity: Option<String>,
    /// File path contains directory
    pub directory: Option<String>,
    /// Downstream weight >= N
    pub weight_gte: Option<u32>,
}

impl FilterSet {
    /// Returns true if no filters are set.
    pub fn is_empty(&self) -> bool {
        self.ids.is_none()
            && self.parent.is_none()
            && self.subtree.is_none()
            && self.status.is_none()
            && self.priority.is_none()
            && self.priority_gte.is_none()
            && self.tags.is_none()
            && self.doc_type.is_none()
            && self.older_than_days.is_none()
            && self.stale_days.is_none()
            && self.orphan.is_none()
            && self.title_contains.is_none()
            && self.assignee.is_none()
            && self.complexity.is_none()
            && self.directory.is_none()
            && self.weight_gte.is_none()
    }

    /// Resolve against graph, return matching node IDs.
    pub fn resolve(&self, graph: &GraphStore) -> Vec<String> {
        // Start with candidate set
        let candidates: Vec<&GraphNode> = if let Some(ref ids) = self.ids {
            // Explicit IDs — resolve each flexibly
            ids.iter()
                .filter_map(|id| graph.resolve(id))
                .collect()
        } else {
            // All nodes with a task_id (actionable documents)
            graph.nodes().filter(|n| n.task_id.is_some()).collect()
        };

        // Collect subtree IDs if needed
        let subtree_ids: Option<HashSet<String>> = self.subtree.as_ref().map(|root_id| {
            collect_subtree_ids(graph, root_id)
        });

        let now = Utc::now().date_naive();

        candidates
            .into_iter()
            .filter(|node| self.matches_node(node, graph, subtree_ids.as_ref(), now))
            .map(|node| node.id.clone())
            .collect()
    }

    /// Human-readable description of the active filters.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref ids) = self.ids {
            parts.push(format!("ids=[{}]", ids.join(", ")));
        }
        if let Some(ref p) = self.parent {
            parts.push(format!("parent={p}"));
        }
        if let Some(ref s) = self.subtree {
            parts.push(format!("subtree={s}"));
        }
        if let Some(ref s) = self.status {
            parts.push(format!("status={s}"));
        }
        if let Some(p) = self.priority {
            parts.push(format!("priority={p}"));
        }
        if let Some(p) = self.priority_gte {
            parts.push(format!("priority>={p}"));
        }
        if let Some(ref t) = self.tags {
            parts.push(format!("tags=[{}]", t.join(", ")));
        }
        if let Some(ref t) = self.doc_type {
            parts.push(format!("type={t}"));
        }
        if let Some(d) = self.older_than_days {
            parts.push(format!("older_than={d}d"));
        }
        if let Some(d) = self.stale_days {
            parts.push(format!("stale={d}d"));
        }
        if self.orphan == Some(true) {
            parts.push("orphan=true".to_string());
        }
        if let Some(ref t) = self.title_contains {
            parts.push(format!("title_contains=\"{t}\""));
        }
        if let Some(ref a) = self.assignee {
            parts.push(format!("assignee={a}"));
        }
        if let Some(ref c) = self.complexity {
            parts.push(format!("complexity={c}"));
        }
        if let Some(ref d) = self.directory {
            parts.push(format!("directory={d}"));
        }
        if let Some(w) = self.weight_gte {
            parts.push(format!("weight>={w}"));
        }
        if parts.is_empty() {
            "all tasks".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// Check if a single node matches all active filters.
    fn matches_node(
        &self,
        node: &GraphNode,
        graph: &GraphStore,
        subtree_ids: Option<&HashSet<String>>,
        now: NaiveDate,
    ) -> bool {
        // Parent filter (direct children only)
        if let Some(ref parent_id) = self.parent {
            let parent_node = graph.resolve(parent_id);
            let parent_canonical_id = parent_node.map(|n| n.id.as_str());
            if node.parent.as_deref() != parent_canonical_id {
                return false;
            }
        }

        // Subtree filter
        if let Some(ref ids) = subtree_ids {
            if !ids.contains(&node.id) {
                return false;
            }
        }

        // Status filter
        if let Some(ref status) = self.status {
            if node.status.as_deref() != Some(status.as_str()) {
                return false;
            }
        }

        // Priority exact
        if let Some(priority) = self.priority {
            if node.priority != Some(priority) {
                return false;
            }
        }

        // Priority gte
        if let Some(priority_gte) = self.priority_gte {
            match node.priority {
                Some(p) if p >= priority_gte => {}
                _ => return false,
            }
        }

        // Tags (must have ALL listed tags)
        if let Some(ref tags) = self.tags {
            for tag in tags {
                if !node.tags.contains(tag) {
                    return false;
                }
            }
        }

        // Document type
        if let Some(ref doc_type) = self.doc_type {
            if node.node_type.as_deref() != Some(doc_type.as_str()) {
                return false;
            }
        }

        // Older than N days (based on created date)
        if let Some(older_than) = self.older_than_days {
            if !is_older_than(node.created.as_deref(), now, older_than) {
                return false;
            }
        }

        // Stale N days (based on modified date)
        if let Some(stale) = self.stale_days {
            let date_str = node.modified.as_deref().or(node.created.as_deref());
            if !is_older_than(date_str, now, stale) {
                return false;
            }
        }

        // Orphan: no parent AND no project
        if self.orphan == Some(true) {
            if node.parent.is_some() {
                return false;
            }
        }

        // Title contains (case-insensitive)
        if let Some(ref needle) = self.title_contains {
            let label_lower = node.label.to_lowercase();
            if !label_lower.contains(&needle.to_lowercase()) {
                return false;
            }
        }

        // Assignee
        if let Some(ref assignee) = self.assignee {
            if node.assignee.as_deref() != Some(assignee.as_str()) {
                return false;
            }
        }

        // Complexity
        if let Some(ref complexity) = self.complexity {
            if node.complexity.as_deref() != Some(complexity.as_str()) {
                return false;
            }
        }

        // Directory filter (path contains)
        if let Some(ref dir) = self.directory {
            let path_str = node.path.to_string_lossy();
            if !path_str.contains(dir.as_str()) {
                return false;
            }
        }

        // Downstream weight >= N
        if let Some(weight_gte) = self.weight_gte {
            if node.downstream_weight < weight_gte as f64 {
                return false;
            }
        }

        true
    }
}

/// Parse a FilterSet from MCP JSON arguments.
pub fn parse_filter_set(args: &serde_json::Value) -> FilterSet {
    FilterSet {
        ids: args.get("ids").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
        }),
        parent: args.get("parent").and_then(|v| v.as_str().map(String::from)),
        subtree: args.get("subtree").and_then(|v| v.as_str().map(String::from)),
        status: args.get("status").and_then(|v| v.as_str().map(String::from)),
        priority: args.get("priority").and_then(|v| v.as_i64().map(|n| n as i32)),
        priority_gte: args.get("priority_gte").and_then(|v| v.as_i64().map(|n| n as i32)),
        tags: args.get("tags").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
        }),
        doc_type: args.get("type").and_then(|v| v.as_str().map(String::from)),
        older_than_days: args.get("older_than_days").and_then(|v| v.as_u64()),
        stale_days: args.get("stale_days").and_then(|v| v.as_u64()),
        orphan: args.get("orphan").and_then(|v| v.as_bool()),
        title_contains: args.get("title_contains").and_then(|v| v.as_str().map(String::from)),
        assignee: args.get("assignee").and_then(|v| v.as_str().map(String::from)),
        complexity: args.get("complexity").and_then(|v| v.as_str().map(String::from)),
        directory: args.get("directory").and_then(|v| v.as_str().map(String::from)),
        weight_gte: args.get("weight_gte").and_then(|v| v.as_u64().map(|n| n as u32)),
    }
}

/// Collect all descendant IDs under a root node (BFS through children).
fn collect_subtree_ids(graph: &GraphStore, root_id: &str) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    if let Some(root) = graph.resolve(root_id) {
        queue.push_back(root.id.clone());
    }

    while let Some(current) = queue.pop_front() {
        if !ids.insert(current.clone()) {
            continue;
        }
        if let Some(node) = graph.get_node(&current) {
            for child_id in &node.children {
                if !ids.contains(child_id) {
                    queue.push_back(child_id.clone());
                }
            }
        }
    }

    ids
}

/// Check if a date string is older than N days from `now`.
fn is_older_than(date_str: Option<&str>, now: NaiveDate, days: u64) -> bool {
    let Some(date_str) = date_str else {
        // No date = treat as infinitely old (matches the filter)
        return true;
    };

    // Try parsing as ISO-8601 datetime or date
    let parsed = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        dt.date_naive()
    } else if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        d
    } else if let Ok(d) = NaiveDate::parse_from_str(&date_str[..10.min(date_str.len())], "%Y-%m-%d") {
        d
    } else {
        // Can't parse date — skip this filter
        return true;
    };

    let age = now.signed_duration_since(parsed).num_days();
    age >= days as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_set_describe() {
        let f = FilterSet {
            priority_gte: Some(2),
            ..Default::default()
        };
        assert_eq!(f.describe(), "priority>=2");
    }

    #[test]
    fn test_filter_set_empty() {
        let f = FilterSet::default();
        assert!(f.is_empty());
        assert_eq!(f.describe(), "all tasks");
    }

    #[test]
    fn test_is_older_than() {
        let now = NaiveDate::from_ymd_opt(2026, 3, 9).unwrap();
        assert!(is_older_than(Some("2026-01-01"), now, 60));
        assert!(!is_older_than(Some("2026-03-08"), now, 60));
        assert!(is_older_than(None, now, 60)); // no date = old
    }

    #[test]
    fn test_parse_filter_set() {
        let args = serde_json::json!({
            "priority_gte": 2,
            "tags": ["batch-ops", "spec-ready"],
            "title_contains": "Write spec"
        });
        let f = parse_filter_set(&args);
        assert_eq!(f.priority_gte, Some(2));
        assert_eq!(f.tags.as_ref().map(|t| t.len()), Some(2));
        assert_eq!(f.title_contains.as_deref(), Some("Write spec"));
    }
}
