//! One-shot, idempotent data migrations over the PKB on disk.
//!
//! Currently hosts `migrate_target_parents`, which converts illegal
//! `parent: <target/goal>` edges into `contributes_to` edges so that work
//! still propagates weight to its strategic target without the target acting
//! as a structural parent (targets are "black holes" — invisible attractors,
//! excluded from the work tree; see specs `pkb-type-taxonomy.md`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::graph_store::GraphStore;

/// `stated_weight` assigned to migrated edges. `Expected` (0.75 on the
/// Renooij–Witteman verbal scale) is the semantically correct default for a
/// former child — it strongly contributes to its target. Note this is *higher*
/// than the old parent-edge propagation factor (0.50); see the PR description
/// for the Expected-vs-Fifty-Fifty tradeoff.
pub const MIGRATED_WEIGHT: &str = "Expected";

const MIGRATED_JUSTIFICATION: &str =
    "Migrated from a parent edge: targets are strategic priorities, not structural \
     parents. The former child is presumed to strongly contribute to the target.";

/// What happened to a single task during the migration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum ChangeKind {
    /// `parent` removed and a fresh `contributes_to` entry appended.
    Migrated,
    /// Task already had a `contributes_to` entry for this target — `parent`
    /// removed only (kept idempotent: no duplicate edge appended).
    AlreadyLinked,
}

/// A single file the migration would touch (or did touch).
#[derive(Debug, Clone, Serialize)]
pub struct TargetParentChange {
    /// Relative path of the file under the PKB root.
    pub file: PathBuf,
    /// The task's own canonical id (best-effort; falls back to node id).
    pub task_id: String,
    /// The raw `parent` value that was being removed.
    pub old_parent: String,
    /// Canonical id of the target the parent resolved to.
    pub target_id: String,
    pub kind: ChangeKind,
}

/// Aggregate report for a migration run.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TargetParentReport {
    pub dry_run: bool,
    pub changes: Vec<TargetParentChange>,
}

impl TargetParentReport {
    pub fn migrated(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Migrated)
            .count()
    }
    pub fn already_linked(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.kind == ChangeKind::AlreadyLinked)
            .count()
    }
    pub fn total(&self) -> usize {
        self.changes.len()
    }
}

/// Find every node whose `parent` resolves to a `target`/`goal` node and
/// convert that parent edge into a `contributes_to` edge.
///
/// Idempotent: a node that already carries a `contributes_to` entry pointing at
/// the same (resolved) target keeps only that entry — the stale `parent` is
/// still removed, but no duplicate edge is appended. Re-running the migration
/// after a successful pass is a no-op (no `parent`-on-target edges remain).
///
/// With `dry_run`, no files are written; the returned report still lists every
/// change that *would* be made.
pub fn migrate_target_parents(pkb_root: &Path, dry_run: bool) -> Result<TargetParentReport> {
    let graph = GraphStore::build_from_directory(pkb_root);
    let mut report = TargetParentReport {
        dry_run,
        changes: Vec::new(),
    };

    for node in graph.nodes() {
        // Skip ghost/virtual nodes that have no backing file on disk; their
        // path is empty and any I/O attempt would resolve to pkb_root itself.
        if node.path.as_os_str().is_empty() {
            continue;
        }
        let Some(parent_ref) = node.parent.as_deref() else {
            continue;
        };
        let parent_ref = parent_ref.trim();
        if parent_ref.is_empty() {
            continue;
        }
        // Only act when the parent resolves to a strategic target/goal node.
        let Some(parent_node) = graph.resolve(parent_ref) else {
            continue;
        };
        if !crate::graph::is_strategic_target(parent_node.node_type.as_deref()) {
            continue;
        }
        let target_id = parent_node.id.clone();

        // Does the node already contribute to this target? (idempotency)
        let already_linked = node.contributes_to.iter().any(|edge| {
            let edge_target = graph
                .resolve(&edge.to)
                .map(|n| n.id.clone())
                .unwrap_or_else(|| edge.to.clone());
            edge_target == target_id
        });

        let abs_path = if node.path.is_absolute() {
            node.path.clone()
        } else {
            pkb_root.join(&node.path)
        };

        let kind = if already_linked {
            ChangeKind::AlreadyLinked
        } else {
            ChangeKind::Migrated
        };

        report.changes.push(TargetParentChange {
            file: node.path.clone(),
            task_id: node.task_id.clone().unwrap_or_else(|| node.id.clone()),
            old_parent: parent_ref.to_string(),
            target_id: target_id.clone(),
            kind: kind.clone(),
        });

        if dry_run {
            continue;
        }

        apply_change(&abs_path, &target_id, matches!(kind, ChangeKind::Migrated))?;
    }

    Ok(report)
}

/// Rewrite one file: drop the `parent` key and (when `append_edge`) merge a
/// fresh `contributes_to` entry pointing at `target_id`.
fn apply_change(abs_path: &Path, target_id: &str, append_edge: bool) -> Result<()> {
    use std::collections::HashMap;

    let mut updates: HashMap<String, serde_json::Value> = HashMap::new();
    // Null removes the frontmatter key (see document_crud::update_document).
    updates.insert("parent".to_string(), serde_json::Value::Null);

    if append_edge {
        let existing = read_contributes_to(abs_path)?;
        let mut edges = existing;
        edges.push(serde_json::json!({
            "to": target_id,
            "stated_weight": MIGRATED_WEIGHT,
            "justification": MIGRATED_JUSTIFICATION,
        }));
        updates.insert(
            "contributes_to".to_string(),
            serde_json::Value::Array(edges),
        );
    }

    crate::document_crud::update_document(abs_path, updates)
}

/// Read the existing `contributes_to` array (raw frontmatter objects) so the
/// migration merges rather than clobbers. Returns an empty Vec when absent.
fn read_contributes_to(abs_path: &Path) -> Result<Vec<serde_json::Value>> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;

    let content = std::fs::read_to_string(abs_path)?;
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(&content);
    let fm = parsed
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok());

    Ok(fm
        .as_ref()
        .and_then(|v| v.get("contributes_to"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}
