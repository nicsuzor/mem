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

/// Summary report for `migrate_clean_aliases`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CleanAliasesReport {
    pub dry_run: bool,
    pub files_scanned: usize,
    pub files_changed: usize,
    pub permalinks_removed: usize,
    pub aliases_stripped: usize,
    pub references_rewritten: usize,
    pub preserved_user_aliases: Vec<(PathBuf, Vec<String>)>,
}

fn is_redundant_alias(alias: &str, id: &str, stem: Option<&str>, permalink: Option<&str>) -> bool {
    let a = alias.trim();
    let id_l = id.to_lowercase();
    let a_l = a.to_lowercase();
    if a_l == id_l {
        return true;
    }
    if a_l.starts_with(&format!("{id_l}_")) || a_l.starts_with(&format!("{id_l}-")) {
        return true;
    }
    if let Some(st) = stem {
        if a_l == st.to_lowercase() {
            return true;
        }
    }
    if let Some(pl) = permalink {
        if a_l == pl.trim().to_lowercase() {
            return true;
        }
    }
    false
}

fn rewrite_wikilinks(
    body: &str,
    alias_to_id: &std::collections::HashMap<String, String>,
) -> (String, usize) {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    let mut count = 0usize;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("]]") {
            let inner = &after_open[..end];
            let (target, suffix) = match inner.find('|') {
                Some(pipe) => (&inner[..pipe], Some(&inner[pipe..])),
                None => (inner, None),
            };
            let key = target.trim().to_lowercase();
            if let Some(canon) = alias_to_id.get(&key) {
                if canon != target.trim() {
                    out.push_str("[[");
                    out.push_str(canon);
                    if let Some(suf) = suffix {
                        out.push_str(suf);
                    }
                    out.push_str("]]");
                    count += 1;
                    rest = &after_open[end + 2..];
                    continue;
                }
            }
            out.push_str("[[");
            out.push_str(inner);
            out.push_str("]]");
            rest = &after_open[end + 2..];
        } else {
            out.push_str("[[");
            rest = after_open;
        }
    }
    out.push_str(rest);
    (out, count)
}

/// Remove `permalink`, strip ID/filename entries from `alias`/`aliases`,
/// preserve user-reorganisation aliases, and rewrite incoming references
/// (wikilinks and frontmatter edge fields) to canonical node IDs.
pub fn migrate_clean_aliases(pkb_root: &Path, dry_run: bool) -> Result<CleanAliasesReport> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;
    use std::collections::HashMap;

    let files = crate::pkb::scan_directory(pkb_root);
    let mut report = CleanAliasesReport {
        dry_run,
        files_scanned: files.len(),
        ..Default::default()
    };

    // Pass 1: build alias/permalink -> canonical_id map and compute per-file alias/permalink edits.
    struct ParsedFile {
        abs_path: PathBuf,
        rel_path: PathBuf,
        id: String,
        fm_map: serde_json::Map<String, serde_json::Value>,
        body: String,
        has_permalink: bool,
        new_aliases: Option<Vec<String>>,
        had_alias_key: bool,
        had_aliases_key: bool,
        stripped_count: usize,
    }

    let matter = Matter::<YAML>::new();
    let mut parsed_files = Vec::with_capacity(files.len());
    let mut alias_to_id: HashMap<String, String> = HashMap::new();

    for abs_path in &files {
        let Ok(content) = std::fs::read_to_string(abs_path) else {
            continue;
        };
        let parsed = matter.parse(&content);
        let Some(fm_val) = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        else {
            continue;
        };
        let Some(fm_map) = fm_val.as_object().cloned() else {
            continue;
        };
        let id = fm_map
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let rel = abs_path.strip_prefix(pkb_root).unwrap_or(abs_path);
                crate::pkb::fallback_id(rel)
            });

        let stem = abs_path.file_stem().and_then(|s| s.to_str());
        let permalink = fm_map
            .get("permalink")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let has_permalink = fm_map.contains_key("permalink");
        if let Some(ref pl) = permalink {
            let k = pl.trim().to_lowercase();
            if !k.is_empty() && k != id.to_lowercase() {
                alias_to_id.entry(k).or_insert_with(|| id.clone());
            }
        }

        let had_alias_key = fm_map.contains_key("alias");
        let had_aliases_key = fm_map.contains_key("aliases");
        let mut raw_aliases: Vec<String> = Vec::new();
        for key in ["alias", "aliases"] {
            if let Some(val) = fm_map.get(key) {
                if let Some(s) = val.as_str() {
                    raw_aliases.push(s.to_string());
                } else if let Some(arr) = val.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            raw_aliases.push(s.to_string());
                        }
                    }
                }
            }
        }

        let mut kept = Vec::new();
        let mut stripped_count = 0usize;
        for a in raw_aliases {
            if is_redundant_alias(&a, &id, stem, permalink.as_deref()) {
                let k = a.trim().to_lowercase();
                if !k.is_empty() && k != id.to_lowercase() {
                    alias_to_id.entry(k).or_insert_with(|| id.clone());
                }
                stripped_count += 1;
            } else if !kept.iter().any(|x| x == &a) {
                kept.push(a);
            }
        }

        let rel_path = abs_path
            .strip_prefix(pkb_root)
            .unwrap_or(abs_path)
            .to_path_buf();
        if !kept.is_empty() {
            report
                .preserved_user_aliases
                .push((rel_path.clone(), kept.clone()));
        }

        let new_aliases = if had_alias_key || had_aliases_key {
            if kept.is_empty() {
                Some(Vec::new())
            } else if stripped_count > 0 || had_aliases_key {
                Some(kept)
            } else {
                None
            }
        } else {
            None
        };

        parsed_files.push(ParsedFile {
            abs_path: abs_path.clone(),
            rel_path,
            id,
            fm_map,
            body: parsed.content,
            has_permalink,
            new_aliases,
            had_alias_key,
            had_aliases_key,
            stripped_count,
        });
    }

    // Pass 2: apply frontmatter updates and reference rewrites.
    for pf in parsed_files {
        let mut fm_updates: HashMap<String, serde_json::Value> = HashMap::new();
        let mut changed = false;

        if pf.has_permalink {
            fm_updates.insert("permalink".to_string(), serde_json::Value::Null);
            report.permalinks_removed += 1;
            changed = true;
        }
        if let Some(ref kept) = pf.new_aliases {
            if pf.had_aliases_key {
                fm_updates.insert("aliases".to_string(), serde_json::Value::Null);
            }
            if kept.is_empty() {
                if pf.had_alias_key {
                    fm_updates.insert("alias".to_string(), serde_json::Value::Null);
                }
            } else {
                fm_updates.insert("alias".to_string(), serde_json::json!(kept));
            }
            report.aliases_stripped += pf.stripped_count;
            changed = true;
        }

        // Rewrite incoming edge references in frontmatter
        let mut refs_here = 0usize;
        for field in ["parent", "supersedes"] {
            if let Some(val) = pf.fm_map.get(field).and_then(|v| v.as_str()) {
                if let Some(canon) = alias_to_id.get(&val.trim().to_lowercase()) {
                    if canon != val.trim() {
                        fm_updates.insert(field.to_string(), serde_json::json!(canon));
                        refs_here += 1;
                        changed = true;
                    }
                }
            }
        }
        for list_field in ["depends_on", "soft_depends_on"] {
            if let Some(arr) = pf.fm_map.get(list_field).and_then(|v| v.as_array()) {
                let mut new_arr = Vec::with_capacity(arr.len());
                let mut list_changed = false;
                for item in arr {
                    if let Some(s) = item.as_str() {
                        if let Some(canon) = alias_to_id.get(&s.trim().to_lowercase()) {
                            if canon != s.trim() {
                                new_arr.push(serde_json::json!(canon));
                                list_changed = true;
                                refs_here += 1;
                                continue;
                            }
                        }
                    }
                    new_arr.push(item.clone());
                }
                if list_changed {
                    fm_updates.insert(list_field.to_string(), serde_json::Value::Array(new_arr));
                    changed = true;
                }
            }
        }
        if let Some(arr) = pf.fm_map.get("contributes_to").and_then(|v| v.as_array()) {
            let mut new_arr = Vec::with_capacity(arr.len());
            let mut ct_changed = false;
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let mut new_obj = obj.clone();
                    for key in ["to", "target"] {
                        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                            if let Some(canon) = alias_to_id.get(&s.trim().to_lowercase()) {
                                if canon != s.trim() {
                                    new_obj.insert(key.to_string(), serde_json::json!(canon));
                                    ct_changed = true;
                                    refs_here += 1;
                                }
                            }
                        }
                    }
                    new_arr.push(serde_json::Value::Object(new_obj));
                } else {
                    new_arr.push(item.clone());
                }
            }
            if ct_changed {
                fm_updates.insert(
                    "contributes_to".to_string(),
                    serde_json::Value::Array(new_arr),
                );
                changed = true;
            }
        }

        // Rewrite incoming wikilinks in body
        let (new_body, wl_count) = rewrite_wikilinks(&pf.body, &alias_to_id);
        if wl_count > 0 {
            refs_here += wl_count;
            changed = true;
        }
        report.references_rewritten += refs_here;

        if changed {
            report.files_changed += 1;
            if !dry_run {
                if !fm_updates.is_empty() {
                    crate::document_crud::update_document(&pf.abs_path, fm_updates)?;
                }
                if wl_count > 0 {
                    crate::document_crud::rewrite_body(&pf.abs_path, &new_body, true, None)?;
                }
            }
        }
        let _ = (&pf.id, &pf.rel_path);
    }

    Ok(report)
}
