//! `batch_update` — update frontmatter fields across multiple tasks.
//!
//! Covers the general case: status changes, priority adjustments, tag
//! modifications, field removal, and supersedes marking.

use super::filters::FilterSet;
use super::{BatchContext, BatchSummary, TaskAction, TaskError};
use crate::graph_store::GraphStore;
use std::collections::HashMap;
use std::path::Path;

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

    let matched_ids = filters.resolve(graph, pkb_root);
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
                        // Status validation is deferred to per-node loop
                        // since non-task nodes can have arbitrary statuses.
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
                                    error: format!(
                                        "Invalid priority: {}. Must be between 0 and 4.",
                                        p
                                    ),
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

    // Validate + canonicalize a project change against polecat.yaml ONCE for
    // the whole batch (not per matched node — the registry lookup is a file
    // parse). `null` is an explicit removal and passes through untouched.
    let mut updates_map = updates_map.clone();
    if let Some(v) = updates_map.get("project") {
        if let Some(raw) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
            match crate::polecat_config::resolve_project(pkb_root, raw) {
                Ok(canonical) => {
                    updates_map.insert("project".to_string(), serde_json::Value::String(canonical));
                }
                Err(e) => {
                    summary.errors.push(TaskError {
                        id: "".to_string(),
                        error: format!("{e:#}"),
                    });
                    return summary;
                }
            }
        }
    }
    let updates_map = &updates_map;

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

        if let Some(s) = updates_map.get("status").and_then(|v| v.as_str()) {
            let is_task = node
                .node_type
                .as_deref()
                .map(|t| crate::graph::TASK_TYPES.contains(&t))
                .unwrap_or(false);
            if is_task && !crate::graph::is_valid_status(s) {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: format!("Invalid status for task: {}", s),
                });
                continue;
            }
        }

        // Build the effective updates for this node
        let effective = match crate::document_crud::expand_special_update_keys(node, updates_map) {
            Ok(e) => e,
            Err(e) => {
                summary.errors.push(TaskError {
                    id: id.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
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

    summary.modified_paths = ctx.modified_paths().to_vec();
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

    let matched_ids = filters.resolve(graph, pkb_root);
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
                detail: Some(format!(
                    "already {}",
                    node.status.as_deref().unwrap_or("done")
                )),
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
            let active_children: Vec<_> = node
                .children
                .iter()
                .filter(|cid| {
                    graph
                        .get_node(cid)
                        .map(|c| !crate::graph::is_completed(c.status.as_deref()))
                        .unwrap_or(false)
                })
                .collect();
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
                    let note = format!(
                        "<!-- archived {}: {} -->",
                        chrono::Utc::now().format("%Y-%m-%d"),
                        reason
                    );
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

    summary.modified_paths = ctx.modified_paths().to_vec();
    summary
}

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
