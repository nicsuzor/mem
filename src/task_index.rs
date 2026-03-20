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
    pub priority: i32,
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
    #[serde(default)]
    pub downstream_weight: f64,
    #[serde(default)]
    pub stakeholder_exposure: bool,
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
                status: node.status.clone().unwrap_or_else(|| "active".to_string()),
                priority: node.priority.unwrap_or(2),
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
                downstream_weight: node.downstream_weight,
                stakeholder_exposure: node.stakeholder_exposure,
            },
        );
    }

    // Compute bidirectional relationship symmetry
    // (same logic as original build_mcp_index in main.rs)
    let task_ids: Vec<String> = entries.keys().cloned().collect();

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
                    if entries.contains_key(parent_id) {
                        child_updates.push((parent_id.clone(), tid.clone()));
                    }
                }
            }
            // children -> parent symmetry
            for child_id in &entry.children {
                if entries.contains_key(child_id) {
                    parent_updates.push((child_id.clone(), tid.clone()));
                }
            }
            // depends_on -> blocks symmetry
            for dep_id in &entry.depends_on {
                if entries.contains_key(dep_id) {
                    block_updates.push((dep_id.clone(), tid.clone()));
                }
            }
            // blocks -> depends_on symmetry
            for blocker_id in &entry.blocks {
                if entries.contains_key(blocker_id) {
                    dep_updates.push((blocker_id.clone(), tid.clone()));
                }
            }
            // soft_depends_on -> soft_blocks symmetry
            for sdep_id in &entry.soft_depends_on {
                if entries.contains_key(sdep_id) {
                    soft_block_updates.push((sdep_id.clone(), tid.clone()));
                }
            }
            // soft_blocks -> soft_depends_on symmetry
            for sblocker_id in &entry.soft_blocks {
                if entries.contains_key(sblocker_id) {
                    soft_dep_updates.push((sblocker_id.clone(), tid.clone()));
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
        .map(|(id, _)| id.clone())
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
            .filter(|d| !completed_ids.contains(*d))
            .collect();
        if !unmet.is_empty() || entry.status == "blocked" {
            blocked.push(tid.clone());
        } else if entry.leaf && entry.status == "active" && entry.task_type != "learn" {
            ready.push(tid.clone());
        }
    }

    // Sort ready
    ready.sort_by(|a, b| {
        let ea = entries.get(a).unwrap();
        let eb = entries.get(b).unwrap();
        ea.priority
            .cmp(&eb.priority)
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
