//! `batch_update` — update frontmatter fields across multiple tasks.
//!
//! Covers the general case: status changes, priority adjustments, tag
//! modifications, field removal, and supersedes marking.

use super::{BatchContext, BatchSummary, TaskAction, TaskError};
use super::filters::FilterSet;
use crate::graph_store::GraphStore;
use std::collections::HashMap;
use std::path::Path;

/// Special update keys that get transformed (not set directly).
const SPECIAL_KEYS: &[&str] = &[
    "_add_tags",
    "_remove_tags",
    "_add_depends_on",
    "_remove_depends_on",
];

/// Execute a batch update operation.
///
/// Applies `updates` to all tasks matching `filters`. Supports special
/// keys like `_add_tags` and `_remove_tags` for array manipulation.
pub fn batch_update(
    graph: &GraphStore,
    pkb_root: &Path,
    filters: &FilterSet,
    updates: &serde_json::Value,
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("update", dry_run);

    let matched_ids = filters.resolve(graph);
    summary.matched = matched_ids.len();

    if matched_ids.is_empty() {
        return summary;
    }

    let updates_map = match updates.as_object() {
        Some(m) => {
            // Early validation of update fields
            for (key, value) in m {
                match key.as_str() {
                    "status" => {
                        if let Some(s) = value.as_str() {
                            if !crate::graph::is_valid_status(s) {
                                summary.errors.push(TaskError {
                                    id: "".to_string(),
                                    error: format!("Invalid status: {}", s),
                                });
                                return summary;
                            }
                        }
                    }
                    "type" => {
                        if let Some(t) = value.as_str() {
                            if !crate::graph::is_valid_node_type(t) {
                                summary.errors.push(TaskError {
                                    id: "".to_string(),
                                    error: format!("Invalid document type: {}", t),
                                });
                                return summary;
                            }
                        }
                    }
                    "priority" => {
                        if let Some(p) = value.as_i64() {
                            if !crate::graph::is_valid_priority(p as i32) {
                                summary.errors.push(TaskError {
                                    id: "".to_string(),
                                    error: format!("Invalid priority: {}. Must be between 0 and 4.", p),
                                });
                                return summary;
                            }
                        }
                    }
                    "effort" => {
                        if let Some(e) = value.as_str() {
                            if !crate::graph::is_valid_effort(e) {
                                summary.errors.push(TaskError {
                                    id: "".to_string(),
                                    error: format!("Invalid effort: {}. Expected duration string like '1d', '2h', '1w'.", e),
                                });
                                return summary;
                            }
                        }
                    }
                    _ => {}
                }
            }
            m
        }
        None => {
            summary.errors.push(TaskError {
                id: "".to_string(),
                error: "updates must be a JSON object".to_string(),
            });
            return summary;
        }
    };

    let mut ctx = BatchContext::new(graph, pkb_root);

    for id in &matched_ids {
        let node = match graph.get_node(id) {
            Some(n) => n,
            None => {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: "node not found in graph".to_string(),
                });
                continue;
            }
        };

        // Build the effective updates for this node
        let effective = build_effective_updates(updates_map, node);
        if effective.is_empty() {
            summary.skipped += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "skipped".to_string(),
                detail: Some("no changes needed".to_string()),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        let detail = describe_updates(&effective);

        if dry_run {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "would_update".to_string(),
                detail: Some(detail),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        // Apply updates
        match ctx.update_task(id, effective) {
            Ok(()) => {
                summary.changed += 1;
                summary.tasks.push(TaskAction {
                    id: id.clone(),
                    title: node.label.clone(),
                    action: "updated".to_string(),
                    detail: Some(detail),
                    old_value: None,
                    new_value: None,
                });
            }
            Err(e) => {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    summary
}

/// Execute a batch archive (convenience wrapper with dry-run-by-default).
pub fn batch_archive(
    graph: &GraphStore,
    pkb_root: &Path,
    filters: &FilterSet,
    reason: Option<&str>,
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("archive", dry_run);

    let matched_ids = filters.resolve(graph);
    summary.matched = matched_ids.len();

    if matched_ids.is_empty() {
        return summary;
    }

    let mut ctx = BatchContext::new(graph, pkb_root);

    for id in &matched_ids {
        let node = match graph.get_node(id) {
            Some(n) => n,
            None => {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: "node not found in graph".to_string(),
                });
                continue;
            }
        };

        // Skip already archived
        if node.status.as_deref() == Some("done") || node.status.as_deref() == Some("cancelled") {
            summary.skipped += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "skipped".to_string(),
                detail: Some(format!("already {}", node.status.as_deref().unwrap_or("done"))),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        // Warn about in_progress tasks
        let mut warnings = Vec::new();
        if node.status.as_deref() == Some("in_progress") {
            warnings.push("was in_progress");
        }
        if !node.children.is_empty() {
            let active_children: Vec<_> = node.children.iter().filter(|cid| {
                graph.get_node(cid)
                    .map(|c| !crate::graph::is_completed(c.status.as_deref()))
                    .unwrap_or(false)
            }).collect();
            if !active_children.is_empty() {
                warnings.push("has active children");
            }
        }

        let detail = if warnings.is_empty() {
            None
        } else {
            Some(format!("⚠ {}", warnings.join(", ")))
        };

        if dry_run {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "would_archive".to_string(),
                detail,
                old_value: node.status.clone(),
                new_value: Some("done".to_string()),
            });
            continue;
        }

        // Set status to done
        let mut updates = HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String("done".to_string()),
        );

        match ctx.update_task(id, updates) {
            Ok(()) => {
                // Append archive reason if provided
                if let Some(reason) = reason {
                    let note = format!("<!-- archived {}: {} -->",
                        chrono::Utc::now().format("%Y-%m-%d"), reason);
                    let _ = ctx.append_to_task(id, &note);
                }
                summary.changed += 1;
                summary.tasks.push(TaskAction {
                    id: id.clone(),
                    title: node.label.clone(),
                    action: "archived".to_string(),
                    detail,
                    old_value: node.status.clone(),
                    new_value: Some("done".to_string()),
                });
            }
            Err(e) => {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    summary
}

/// Build the effective update HashMap for a node, handling special keys.
fn build_effective_updates(
    updates_map: &serde_json::Map<String, serde_json::Value>,
    node: &crate::graph::GraphNode,
) -> HashMap<String, serde_json::Value> {
    let mut effective = HashMap::new();

    for (key, value) in updates_map {
        if SPECIAL_KEYS.contains(&key.as_str()) {
            continue; // Handle below
        }

        // Check if this would be a no-op
        let is_noop = match key.as_str() {
            "status" => node.status.as_deref() == value.as_str(),
            "priority" => node.priority == value.as_i64().map(|v| v as i32),
            "assignee" => node.assignee.as_deref() == value.as_str(),
            "complexity" => node.complexity.as_deref() == value.as_str(),
            "parent" => node.parent.as_deref() == value.as_str(),
            _ => false,
        };

        if !is_noop {
            effective.insert(key.clone(), value.clone());
        }
    }

    // Handle _add_tags
    if let Some(add_tags) = updates_map.get("_add_tags").and_then(|v| v.as_array()) {
        let new_tags: Vec<String> = add_tags
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .filter(|t| !node.tags.contains(t))
            .collect();
        if !new_tags.is_empty() {
            let mut all_tags = node.tags.clone();
            all_tags.extend(new_tags);
            effective.insert(
                "tags".to_string(),
                serde_json::Value::Array(all_tags.into_iter().map(serde_json::Value::String).collect()),
            );
        }
    }

    // Handle _remove_tags
    if let Some(remove_tags) = updates_map.get("_remove_tags").and_then(|v| v.as_array()) {
        let remove_set: std::collections::HashSet<&str> = remove_tags
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        if node.tags.iter().any(|t| remove_set.contains(t.as_str())) {
            let remaining: Vec<String> = node
                .tags
                .iter()
                .filter(|t| !remove_set.contains(t.as_str()))
                .cloned()
                .collect();
            effective.insert(
                "tags".to_string(),
                serde_json::Value::Array(remaining.into_iter().map(serde_json::Value::String).collect()),
            );
        }
    }

    // Handle _add_depends_on
    if let Some(add_deps) = updates_map.get("_add_depends_on").and_then(|v| v.as_array()) {
        let new_deps: Vec<String> = add_deps
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .filter(|d| !node.depends_on.contains(d))
            .collect();
        if !new_deps.is_empty() {
            let mut all_deps = node.depends_on.clone();
            all_deps.extend(new_deps);
            effective.insert(
                "depends_on".to_string(),
                serde_json::Value::Array(all_deps.into_iter().map(serde_json::Value::String).collect()),
            );
        }
    }

    // Handle _remove_depends_on
    if let Some(remove_deps) = updates_map.get("_remove_depends_on").and_then(|v| v.as_array()) {
        let remove_set: std::collections::HashSet<&str> = remove_deps
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        if node.depends_on.iter().any(|d| remove_set.contains(d.as_str())) {
            let remaining: Vec<String> = node
                .depends_on
                .iter()
                .filter(|d| !remove_set.contains(d.as_str()))
                .cloned()
                .collect();
            effective.insert(
                "depends_on".to_string(),
                serde_json::Value::Array(remaining.into_iter().map(serde_json::Value::String).collect()),
            );
        }
    }

    // Handle superseded_by (sets status to done + superseded_by field)
    if let Some(superseded_by) = updates_map.get("superseded_by") {
        effective.insert("superseded_by".to_string(), superseded_by.clone());
        if !effective.contains_key("status") {
            effective.insert(
                "status".to_string(),
                serde_json::Value::String("done".to_string()),
            );
        }
    }

    effective
}

/// Describe updates in human-readable form.
fn describe_updates(updates: &HashMap<String, serde_json::Value>) -> String {
    updates
        .iter()
        .map(|(k, v)| {
            if v.is_null() {
                format!("unset {k}")
            } else if let Some(s) = v.as_str() {
                format!("{k}={s}")
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
