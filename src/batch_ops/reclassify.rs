//! `batch_reclassify` — change document type and move files.

use super::{BatchContext, BatchSummary, TaskAction, TaskError};
use super::filters::FilterSet;
use crate::graph_store::GraphStore;
use std::collections::HashMap;
use std::path::Path;

/// Map document type to target subdirectory.
fn type_to_dir(doc_type: &str) -> &str {
    match doc_type {
        "task" | "bug" | "epic" | "feature" | "action" | "learn" => "tasks",
        "project" | "subproject" => "projects",
        "goal" => "goals",
        "memory" | "note" | "insight" | "observation" => "memories",
        "knowledge" => "notes",
        _ => "tasks",
    }
}

/// Reclassify tasks: change their type and optionally move to correct directory.
pub fn batch_reclassify(
    graph: &GraphStore,
    pkb_root: &Path,
    filters: &FilterSet,
    new_type: &str,
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("reclassify", dry_run);

    let matched_ids = filters.resolve(graph);
    summary.matched = matched_ids.len();

    if matched_ids.is_empty() {
        return summary;
    }

    let target_dir = type_to_dir(new_type);
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

        let current_type = node.node_type.as_deref().unwrap_or("unknown");

        // Skip if already the correct type
        if current_type == new_type {
            summary.skipped += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "skipped".to_string(),
                detail: Some(format!("already type={new_type}")),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        let abs_path = if node.path.is_absolute() {
            node.path.clone()
        } else {
            pkb_root.join(&node.path)
        };

        // Determine if file needs moving
        let current_dir = abs_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let needs_move = current_dir != target_dir;

        if dry_run {
            let detail = if needs_move {
                format!("{current_type}→{new_type}, move to {target_dir}/")
            } else {
                format!("{current_type}→{new_type}")
            };
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "would_reclassify".to_string(),
                detail: Some(detail),
                old_value: Some(current_type.to_string()),
                new_value: Some(new_type.to_string()),
            });
            continue;
        }

        // Update the type field
        let mut updates = HashMap::new();
        updates.insert(
            "type".to_string(),
            serde_json::Value::String(new_type.to_string()),
        );

        if let Err(e) = ctx.update_task(id, updates) {
            summary.errors.push(TaskError {
                id: id.clone(),
                error: e.to_string(),
            });
            continue;
        }

        // Move file if needed
        if needs_move {
            let target_dir_path = pkb_root.join(target_dir);
            if !target_dir_path.exists() {
                if let Err(e) = std::fs::create_dir_all(&target_dir_path) {
                    summary.errors.push(TaskError {
                        id: id.clone(),
                        error: format!("failed to create dir {target_dir}: {e}"),
                    });
                    continue;
                }
            }

            let filename = abs_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let new_path = target_dir_path.join(&filename);

            match std::fs::rename(&abs_path, &new_path) {
                Ok(()) => {
                    summary.changed += 1;
                    summary.tasks.push(TaskAction {
                        id: id.clone(),
                        title: node.label.clone(),
                        action: "reclassified".to_string(),
                        detail: Some(format!(
                            "{current_type}→{new_type}, moved to {target_dir}/{filename}"
                        )),
                        old_value: Some(current_type.to_string()),
                        new_value: Some(new_type.to_string()),
                    });
                }
                Err(e) => {
                    // Type was updated but move failed
                    summary.changed += 1;
                    summary.errors.push(TaskError {
                        id: id.clone(),
                        error: format!("type updated but file move failed: {e}"),
                    });
                    summary.tasks.push(TaskAction {
                        id: id.clone(),
                        title: node.label.clone(),
                        action: "reclassified".to_string(),
                        detail: Some(format!("{current_type}→{new_type} (move failed)")),
                        old_value: Some(current_type.to_string()),
                        new_value: Some(new_type.to_string()),
                    });
                }
            }
        } else {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: id.clone(),
                title: node.label.clone(),
                action: "reclassified".to_string(),
                detail: Some(format!("{current_type}→{new_type}")),
                old_value: Some(current_type.to_string()),
                new_value: Some(new_type.to_string()),
            });
        }
    }

    summary
}
