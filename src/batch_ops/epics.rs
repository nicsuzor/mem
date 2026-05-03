//! `batch_create_epics` — create epic containers and reparent tasks under them.

use super::{BatchSummary, TaskAction, TaskError};
use crate::document_crud::{self, DocumentFields};
use crate::graph_store::GraphStore;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Definition of an epic to create.
#[derive(Debug, Clone, Deserialize)]
pub struct EpicDef {
    pub title: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// Create multiple epics and reparent existing tasks under them.
pub fn batch_create_epics(
    graph: &GraphStore,
    pkb_root: &Path,
    parent: Option<&str>,
    epics: &[EpicDef],
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("create_epics", dry_run);
    summary.matched = epics.len();

    // Validate parent if specified
    if let Some(parent_id) = parent {
        if graph.resolve(parent_id).is_none() {
            summary.errors.push(TaskError {
                id: parent_id.to_string(),
                error: "parent not found in graph".to_string(),
            });
            return summary;
        }
    }

    for epic_def in epics {
        // Validate task_ids exist
        let mut valid_task_ids = Vec::new();
        for task_id in &epic_def.task_ids {
            match graph.resolve(task_id) {
                Some(n) => valid_task_ids.push(n.id.clone()),
                None => {
                    summary.errors.push(TaskError {
                        id: task_id.clone(),
                        error: format!("task not found (referenced in epic '{}')", epic_def.title),
                    });
                }
            }
        }

        if dry_run {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: epic_def.id.clone().unwrap_or_else(|| "auto".to_string()),
                title: epic_def.title.clone(),
                action: "would_create_epic".to_string(),
                detail: Some(format!("{} tasks to reparent", valid_task_ids.len())),
                old_value: None,
                new_value: None,
            });
            for tid in &valid_task_ids {
                let node = graph.get_node(tid);
                summary.tasks.push(TaskAction {
                    id: tid.clone(),
                    title: node.map(|n| n.label.clone()).unwrap_or_default(),
                    action: "would_reparent".to_string(),
                    detail: Some(format!("→ {}", epic_def.title)),
                    old_value: node.and_then(|n| n.parent.clone()),
                    new_value: epic_def.id.clone(),
                });
            }
            continue;
        }

        // Create the epic
        let fields = DocumentFields {
            title: epic_def.title.clone(),
            doc_type: "epic".to_string(),
            id: epic_def.id.clone(),
            priority: epic_def.priority,
            parent: parent.map(String::from),
            depends_on: epic_def.depends_on.clone(),
            body: epic_def.body.clone(),
            ..Default::default()
        };

        let epic_path = match document_crud::create_document(pkb_root, fields) {
            Ok(path) => path,
            Err(e) => {
                summary.errors.push(TaskError {
                    id: epic_def.id.clone().unwrap_or_default(),
                    error: format!("failed to create epic: {e}"),
                });
                continue;
            }
        };
        // The epic file is brand new — caller must embed it (no existing entry).
        summary.modified_paths.push(epic_path.clone());

        // Read back the epic's ID from the file
        let epic_id = read_id_from_file(&epic_path).unwrap_or_else(|| {
            epic_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        summary.changed += 1;
        summary.tasks.push(TaskAction {
            id: epic_id.clone(),
            title: epic_def.title.clone(),
            action: "created_epic".to_string(),
            detail: Some(format!("at {}", epic_path.display())),
            old_value: None,
            new_value: None,
        });

        // Reparent tasks under the new epic
        for task_id in &valid_task_ids {
            let mut updates = HashMap::new();
            updates.insert(
                "parent".to_string(),
                serde_json::Value::String(epic_id.clone()),
            );
            let node = graph.get_node(task_id);
            let abs_path = node.map(|n| {
                if n.path.is_absolute() {
                    n.path.clone()
                } else {
                    pkb_root.join(&n.path)
                }
            });

            match abs_path {
                Some(path) if path.exists() => {
                    match document_crud::update_document(&path, updates) {
                        Ok(()) => {
                            summary.modified_paths.push(path.clone());
                            summary.tasks.push(TaskAction {
                                id: task_id.clone(),
                                title: node.map(|n| n.label.clone()).unwrap_or_default(),
                                action: "reparented".to_string(),
                                detail: Some(format!("→ {epic_id}")),
                                old_value: node.and_then(|n| n.parent.clone()),
                                new_value: Some(epic_id.clone()),
                            });
                        }
                        Err(e) => {
                            summary.errors.push(TaskError {
                                id: task_id.clone(),
                                error: format!("reparent failed: {e}"),
                            });
                        }
                    }
                }
                _ => {
                    summary.errors.push(TaskError {
                        id: task_id.clone(),
                        error: "file not found on disk".to_string(),
                    });
                }
            }
        }
    }

    summary
}

/// Read the `id` field from a freshly-created frontmatter file.
fn read_id_from_file(path: &Path) -> Option<String> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;

    let content = std::fs::read_to_string(path).ok()?;
    let matter = Matter::<YAML>::new();
    let result = matter.parse(&content);

    result
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        .and_then(|v| v.get("id").and_then(|id| id.as_str().map(String::from)))
}
