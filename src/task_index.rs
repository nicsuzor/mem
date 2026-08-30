//! MCP task index — compatible with the tasks_server.py schema.
//!
//! Produces a JSON index with task metadata, relationships, and
//! pre-computed ready/blocked task lists.

use crate::graph::{self, deduplicate_vec};
use crate::graph_store::GraphStore;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A single task entry in the MCP index.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpIndexEntry {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub status: String,
    pub intent: i32,
    pub order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub depends_on: Vec<String>,
    pub blocks: Vec<String>,
    pub soft_depends_on: Vec<String>,
    pub soft_blocks: Vec<String>,
    pub depth: i32,
    pub leaf: bool,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stakeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<String>,
    #[serde(default)]
    pub downstream_weight: f64,
    #[serde(default)]
    pub stakeholder_exposure: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_score: Option<i64>,
    /// True if the body specifies acceptance criteria — the graduation signal
    /// that lets an `inbox` leaf be surfaced as ready without a manual status edit.
    #[serde(default)]
    pub has_acceptance_criteria: bool,
}

/// The full MCP task index.
#[derive(Serialize, Deserialize, Debug)]
pub struct McpIndex {
    pub version: i32,
    pub generated: String,
    pub tasks: HashMap<String, McpIndexEntry>,
    pub roots: Vec<String>,
    pub ready: Vec<String>,
    pub blocked: Vec<String>,
}

/// Build an MCP task index from a [`GraphStore`].
///
/// Only includes nodes that have a `task_id` (frontmatter `id` field).
/// Relationships (children, blocks, etc.) are already resolved in the graph.
pub fn build_mcp_index(store: &GraphStore, data_root: &Path) -> McpIndex {
    let mut entries: HashMap<String, McpIndexEntry> = HashMap::new();

    // Build entries from graph nodes
    for node in store.nodes() {
        let tid = match &node.task_id {
            Some(id) => id.clone(),
            None => continue,
        };
        let is_actionable = match node.node_type.as_deref() {
            Some(t) => crate::graph_store::ACTIONABLE_TYPES.contains(&t),
            None => true, // Untyped id-bearing node defaults to actionable ("task")
        };
        if !is_actionable {
            continue;
        }

        let rel_path = node
            .path
            .strip_prefix(data_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| node.path.to_string_lossy().to_string());

        entries.insert(
            tid.clone(),
            McpIndexEntry {
                id: tid,
                title: node.label.clone(),
                task_type: node.node_type.clone().unwrap_or_else(|| "task".to_string()),
                status: node.status.clone().unwrap_or_else(|| "inbox".to_string()),
                intent: node.intent.unwrap_or(4),
                order: node.order,
                parent: node.parent.clone(),
                children: node.children.clone(),
                depends_on: node.depends_on.clone(),
                blocks: node.blocks.clone(),
                soft_depends_on: node.soft_depends_on.clone(),
                soft_blocks: node.soft_blocks.clone(),
                depth: node.depth,
                leaf: node.leaf,
                path: rel_path,
                due: node.due.clone(),
                tags: node.tags.clone(),
                assignee: node.assignee.clone(),
                complexity: node.complexity.clone(),
                effort: node.effort.clone(),
                consequence: node.consequence.clone(),
                stakeholder: node.stakeholder.clone(),
                waiting_since: node.waiting_since.clone(),
                downstream_weight: node.downstream_weight,
                stakeholder_exposure: node.stakeholder_exposure,
                focus_score: node.focus_score,
                has_acceptance_criteria: node.has_acceptance_criteria,
            },
        );
    }

    // Compute bidirectional relationship symmetry
    // (same logic as original build_mcp_index in main.rs)
    let task_ids: Vec<String> = entries.keys().cloned().collect();
    let task_id_map: HashMap<String, String> = entries
        .keys()
        .map(|k| (k.to_lowercase(), k.clone()))
        .collect();

    // Collect inverse updates
    let mut parent_updates: Vec<(String, String)> = Vec::new();
    let mut child_updates: Vec<(String, String)> = Vec::new();
    let mut dep_updates: Vec<(String, String)> = Vec::new();
    let mut block_updates: Vec<(String, String)> = Vec::new();
    let mut soft_dep_updates: Vec<(String, String)> = Vec::new();
    let mut soft_block_updates: Vec<(String, String)> = Vec::new();

    for tid in &task_ids {
        if let Some(entry) = entries.get(tid) {
            // parent -> children symmetry (exclude subtasks — they travel with parent separately)
            if entry.task_type != "subtask" {
                if let Some(ref parent_id) = entry.parent {
                    if let Some(canonical_parent) = task_id_map.get(&parent_id.to_lowercase()) {
                        child_updates.push((canonical_parent.clone(), tid.clone()));
                    }
                }
            }
            // children -> parent symmetry
            for child_id in &entry.children {
                if let Some(canonical_child) = task_id_map.get(&child_id.to_lowercase()) {
                    parent_updates.push((canonical_child.clone(), tid.clone()));
                }
            }
            // depends_on -> blocks symmetry
            for dep_id in &entry.depends_on {
                if let Some(canonical_dep) = task_id_map.get(&dep_id.to_lowercase()) {
                    block_updates.push((canonical_dep.clone(), tid.clone()));
                }
            }
            // blocks -> depends_on symmetry
            for blocker_id in &entry.blocks {
                if let Some(canonical_blocker) = task_id_map.get(&blocker_id.to_lowercase()) {
                    dep_updates.push((canonical_blocker.clone(), tid.clone()));
                }
            }
            // soft_depends_on -> soft_blocks symmetry
            for sdep_id in &entry.soft_depends_on {
                if let Some(canonical_sdep) = task_id_map.get(&sdep_id.to_lowercase()) {
                    soft_block_updates.push((canonical_sdep.clone(), tid.clone()));
                }
            }
            // soft_blocks -> soft_depends_on symmetry
            for sblocker_id in &entry.soft_blocks {
                if let Some(canonical_sblocker) = task_id_map.get(&sblocker_id.to_lowercase()) {
                    soft_dep_updates.push((canonical_sblocker.clone(), tid.clone()));
                }
            }
        }
    }

    // Apply updates
    for (child_id, parent_id) in parent_updates {
        if let Some(e) = entries.get_mut(&child_id) {
            if e.parent.is_none() {
                e.parent = Some(parent_id);
            }
        }
    }
    for (parent_id, child_id) in child_updates {
        if let Some(e) = entries.get_mut(&parent_id) {
            e.children.push(child_id);
        }
    }
    for (blocker_id, dep_id) in dep_updates {
        if let Some(e) = entries.get_mut(&blocker_id) {
            e.depends_on.push(dep_id);
        }
    }
    for (dep_id, blocker_id) in block_updates {
        if let Some(e) = entries.get_mut(&dep_id) {
            e.blocks.push(blocker_id);
        }
    }
    for (sblocker_id, sdep_id) in soft_dep_updates {
        if let Some(e) = entries.get_mut(&sblocker_id) {
            e.soft_depends_on.push(sdep_id);
        }
    }
    for (sdep_id, sblocker_id) in soft_block_updates {
        if let Some(e) = entries.get_mut(&sdep_id) {
            e.soft_blocks.push(sblocker_id);
        }
    }

    // Deduplicate and update leaf status
    for tid in &task_ids {
        if let Some(e) = entries.get_mut(tid) {
            deduplicate_vec(&mut e.children);
            deduplicate_vec(&mut e.blocks);
            deduplicate_vec(&mut e.soft_blocks);
            deduplicate_vec(&mut e.depends_on);
            deduplicate_vec(&mut e.soft_depends_on);
            e.leaf = e.children.is_empty();
        }
    }

    // Build index metadata
    let roots = store.roots().to_vec();

    // Ready and blocked lists (already computed by GraphStore, but with index entries)
    let completed_ids: HashSet<String> = entries
        .iter()
        .filter(|(_, e)| graph::is_completed(Some(e.status.as_str())))
        .map(|(id, _)| id.to_lowercase())
        .collect();

    let mut ready: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();

    for (tid, entry) in &entries {
        if graph::is_completed(Some(entry.status.as_str())) {
            continue;
        }
        let unmet: Vec<&String> = entry
            .depends_on
            .iter()
            .filter(|d| !completed_ids.contains(&d.to_lowercase()))
            .collect();
        if !unmet.is_empty() || entry.status == "blocked" {
            blocked.push(tid.clone());
        } else if entry.leaf
            && crate::graph_store::CLAIMABLE_TYPES.contains(&entry.task_type.as_str())
            && crate::graph_store::is_ready_status(
                entry.status.as_str(),
                entry.has_acceptance_criteria,
            )
        {
            // Mirror the canonical ready-gate in graph_store.rs: a leaf claimable
            // task is ready when explicitly `ready`/`queued`, OR when it is still
            // `inbox` but has graduated (acceptance criteria present, deps resolved).
            // `entry.status` is already alias-resolved at node build, so legacy
            // "active" never reaches here as a ready signal.
            ready.push(tid.clone());
        }
    }

    // Sort ready
    ready.sort_by(|a, b| {
        let ea = entries.get(a).unwrap();
        let eb = entries.get(b).unwrap();
        ea.intent
            .cmp(&eb.intent)
            .then(
                eb.downstream_weight
                    .partial_cmp(&ea.downstream_weight)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(ea.order.cmp(&eb.order))
            .then(ea.title.cmp(&eb.title))
    });

    McpIndex {
        version: 2,
        generated: Utc::now().to_rfc3339(),
        tasks: entries,
        roots,
        ready,
        blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkb::PkbDocument;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn test_build_mcp_index_includes_untyped_node_with_id() {
        let mut fm = serde_json::Map::new();
        fm.insert("id".to_string(), json!("task-untyped-42"));
        fm.insert("title".to_string(), json!("Untyped Work Item"));
        fm.insert("status".to_string(), json!("ready"));

        let doc = PkbDocument {
            path: PathBuf::from("tasks/untyped-work.md"),
            title: "Untyped Work Item".to_string(),
            tags: vec![],
            doc_type: None,
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            body: "# Untyped Work Item\nSome description.".to_string(),
            content_hash: "h1".to_string(),
            file_hash: "h1".to_string(),
            frontmatter: Some(serde_json::Value::Object(fm)),
        };

        let store = GraphStore::build(&[doc], Path::new("/tmp/test-pkb"));
        let index = build_mcp_index(&store, Path::new("/tmp/test-pkb"));

        assert!(
            index.tasks.contains_key("task-untyped-42"),
            "untyped task with id should be indexed in McpIndex"
        );
        let entry = index.tasks.get("task-untyped-42").unwrap();
        assert_eq!(entry.task_type, "task");
        assert_eq!(entry.title, "Untyped Work Item");
        assert_eq!(entry.status, "ready");
    }

    #[test]
    fn test_task_index_blocker_resolution_case_insensitive() {
        let mut fm_done = serde_json::Map::new();
        fm_done.insert("id".to_string(), json!("academicops-d067e425"));
        fm_done.insert("title".to_string(), json!("Done Blocker"));
        fm_done.insert("status".to_string(), json!("done"));

        let mut fm_downstream = serde_json::Map::new();
        fm_downstream.insert("id".to_string(), json!("academicops-4a31fae0"));
        fm_downstream.insert("title".to_string(), json!("Downstream Task"));
        fm_downstream.insert("status".to_string(), json!("ready"));
        fm_downstream.insert("depends_on".to_string(), json!(["academicOps-d067e425"]));

        let doc_done = PkbDocument {
            path: PathBuf::from("tasks/academicops-d067e425.md"),
            title: "Done Blocker".to_string(),
            tags: vec![],
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            body: "# Done Blocker".to_string(),
            content_hash: "h1".to_string(),
            file_hash: "h1".to_string(),
            frontmatter: Some(serde_json::Value::Object(fm_done)),
        };

        let doc_downstream = PkbDocument {
            path: PathBuf::from("tasks/academicops-4a31fae0.md"),
            title: "Downstream Task".to_string(),
            tags: vec![],
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            body: "# Downstream Task".to_string(),
            content_hash: "h2".to_string(),
            file_hash: "h2".to_string(),
            frontmatter: Some(serde_json::Value::Object(fm_downstream)),
        };

        let store = GraphStore::build(&[doc_done, doc_downstream], Path::new("/tmp/test-pkb"));
        let index = build_mcp_index(&store, Path::new("/tmp/test-pkb"));

        assert!(
            index.ready.contains(&"academicops-4a31fae0".to_string()),
            "task with mixed-case completed dependency must be ready in index"
        );
        assert!(
            !index.blocked.contains(&"academicops-4a31fae0".to_string()),
            "task with mixed-case completed dependency must not be blocked in index"
        );

        let blocker_entry = index.tasks.get("academicops-d067e425").unwrap();
        assert!(
            blocker_entry.blocks.contains(&"academicops-4a31fae0".to_string()),
            "blocker should have downstream task in blocks"
        );
    }
}
