//! `batch_reparent` — move multiple tasks to a new parent.

use super::{BatchContext, BatchSummary, TaskAction, TaskError};
use super::filters::FilterSet;
use crate::graph_store::GraphStore;
use std::collections::HashMap;
use std::path::Path;

/// Execute a batch reparent operation.
///
/// Moves all tasks matching `filters` under `new_parent_id`.
pub fn batch_reparent(
    graph: &GraphStore,
    pkb_root: &Path,
    filters: &FilterSet,
    new_parent_id: &str,
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("reparent", dry_run);

    // Validate new parent exists
    let new_parent = match graph.resolve(new_parent_id) {
        Some(n) => n,
        None => {
            summary.errors.push(TaskError {
                id: new_parent_id.to_string(),
                error: "new parent not found in graph".to_string(),
            });
            return summary;
        }
    };
    let canonical_parent_id = new_parent.id.clone();

    let matched_ids = filters.resolve(graph);
    summary.matched = matched_ids.len();

    if matched_ids.is_empty() {
        return summary;
    }

    let mut ctx = BatchContext::new(graph, pkb_root);

    for id in &matched_ids {
        // Don't reparent a node under itself
        if id == &canonical_parent_id {
            summary.skipped += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: graph.get_node(id).map(|n| n.label.clone()).unwrap_or_default(),
                action: "skipped".to_string(),
                detail: Some("cannot reparent under self".to_string()),
                old_value: None,
                new_value: None,
            });
            continue;
        }

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

        // Skip if already under this parent (idempotent)
        if node.parent.as_deref() == Some(&canonical_parent_id) {
            summary.skipped += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "skipped".to_string(),
                detail: Some(format!("already under {canonical_parent_id}")),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        let old_parent = node.parent.clone();

        if dry_run {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "would_reparent".to_string(),
                detail: Some(format!("→ {canonical_parent_id}")),
                old_value: old_parent,
                new_value: Some(canonical_parent_id.clone()),
            });
            continue;
        }

        // Build updates
        let mut updates = HashMap::new();
        updates.insert(
            "parent".to_string(),
            serde_json::Value::String(canonical_parent_id.clone()),
        );

        match ctx.update_task(id, updates) {
            Ok(()) => {
                summary.changed += 1;
                summary.tasks.push(TaskAction {
                    id: id.clone(),
                    title: node.label.clone(),
                    action: "reparented".to_string(),
                    detail: Some(format!("→ {canonical_parent_id}")),
                    old_value: old_parent,
                    new_value: Some(canonical_parent_id.clone()),
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
