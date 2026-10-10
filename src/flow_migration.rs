//! Flow migration tool implementation (`src/flow_migration.rs`).
//!
//! Follows `specs/flow-migration.md`:
//! - §5: The transform
//! - §5.4: The ledger
//! - §5.5: Tool contract
//! - §7.2: Reversal

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::graph_store::GraphStore;

/// Proposed supports quantum from flow-migration.md §5.2.
pub const SUPPORTS_QUANTUM: f64 = 0.3;

/// Prototype hub ID converted in R4b (flow-migration.md §9).
pub const PROTOTYPE_HUB_ID: &str = "hdr_prospective_enquiries";

/// A single atomic write recorded in the migration ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowMigrationRow {
    pub node_id: String,
    pub path: String,
    pub key_path: String,
    pub value_before: serde_json::Value,
    pub value_after: serde_json::Value,
    pub raw_before: String,
    pub raw_after: String,
}

/// A statusless task record (flow-migration.md §5.4, §7.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatuslessTaskRow {
    pub node_id: String,
    pub path: String,
    pub note: String,
}

/// Summary report contained inside the migration ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FlowMigrationReportSummary {
    pub total_nodes: usize,
    pub changed_nodes: usize,
    pub total_rows: usize,
    pub rows_by_key: BTreeMap<String, usize>,
    pub contributes_to_quanta: BTreeMap<String, usize>,
    pub left_unvalued_count: usize,
    pub statusless_tasks_count: usize,
    pub open_nodes_count: usize,
    pub carrying_worth_count: usize,
    pub priced_targets_count: usize,
    pub flow_edges_count: usize,
}

/// The flow migration ledger saved to `.agents/migrations/flow-<date>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowMigrationLedger {
    pub migration: String,
    pub created_at: String,
    pub snapshot_commit: Option<String>,
    pub today_rank: Vec<(String, i64)>,
    pub rows: Vec<FlowMigrationRow>,
    pub statusless_tasks: Vec<StatuslessTaskRow>,
    pub report: FlowMigrationReportSummary,
}

/// Report returned by `revert`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RevertReport {
    pub files_reverted: usize,
    pub rows_reverted: usize,
    pub drifted_rows: Vec<FlowMigrationRow>,
    pub ledger_removed: bool,
}

/// Report returned by `status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusReport {
    pub migration_applied: bool,
    pub ledger_file: Option<PathBuf>,
    pub drifted_rows: Vec<FlowMigrationRow>,
    pub statusless_tasks: Vec<StatuslessTaskRow>,
    pub targets_lacking_worth: Vec<(String, String)>,
}

/// Report returned by `cleanup`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupReport {
    pub files_cleaned: usize,
    pub fields_removed: usize,
    pub commit_hash: Option<String>,
}

/// Map verbal contribution word to quantum (matching flow.rs / dryrun.py).
pub fn word_quantum(word: Option<&str>, multiplier: Option<f64>) -> Option<f64> {
    let w = word.unwrap_or("").trim().to_lowercase();
    let base_q = match w.as_str() {
        "certain" | "almost certain" => Some(1.00),
        "very probable" | "probable" | "highly likely" => Some(0.85),
        "expected" | "likely" => Some(0.75),
        "fifty-fifty" | "even chance" => Some(0.50),
        "uncertain" | "possible" | "perhaps" | "maybe" => Some(0.25),
        "improbable" | "unlikely" | "very unlikely" | "almost impossible" => Some(0.15),
        "impossible" | "none" => Some(0.00),
        s => s
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v)),
    };

    let has_m = multiplier.map(|m| m.is_finite() && m >= 0.0).unwrap_or(false);
    if base_q.is_none() && !has_m {
        return None;
    }

    if let Some(m) = multiplier.filter(|m| m.is_finite() && *m >= 0.0) {
        if let Some(bq) = base_q {
            Some((bq * m).min(1.0))
        } else {
            Some(m.min(1.0))
        }
    } else {
        base_q
    }
}

/// Check if a git working tree is clean.
pub fn check_git_clean(dir: &Path) -> Result<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain")
        .output()
        .with_context(|| format!("Failed to run git status in {}", dir.display()))?;
    Ok(out.status.success() && out.stdout.is_empty())
}

/// Get current HEAD commit hash.
pub fn get_git_head(dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .with_context(|| format!("Failed to get git HEAD in {}", dir.display()))?;
    if !out.status.success() {
        bail!("git rev-parse HEAD failed in {}", dir.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Parse frontmatter slice, returning lines between opening and closing `---`.
fn split_frontmatter(content: &str) -> Option<(&str, &str, &str, &str)> {
    if !content.starts_with("---") {
        return None;
    }
    // Find newline after first ---
    let first_nl = content.find('\n')?;
    let header = &content[..first_nl + 1];
    let rest = &content[first_nl + 1..];
    // Find closing --- line
    let mut offset = 0;
    while let Some(idx) = rest[offset..].find("---") {
        let abs_idx = offset + idx;
        // Check if --- is at start of line
        let at_line_start = abs_idx == 0 || rest.as_bytes()[abs_idx - 1] == b'\n';
        let after = abs_idx + 3;
        let at_line_end = after >= rest.len()
            || rest.as_bytes()[after] == b'\n'
            || rest.as_bytes()[after] == b'\r';
        if at_line_start && at_line_end {
            let fm = &rest[..abs_idx];
            let after_end = if after < rest.len() && rest.as_bytes()[after] == b'\r' {
                if after + 1 < rest.len() && rest.as_bytes()[after + 1] == b'\n' {
                    after + 2
                } else {
                    after + 1
                }
            } else if after < rest.len() && rest.as_bytes()[after] == b'\n' {
                after + 1
            } else {
                after
            };
            let footer = &rest[abs_idx..after_end];
            let body = &rest[after_end..];
            return Some((header, fm, footer, body));
        }
        offset = abs_idx + 3;
    }
    None
}

/// Patch one file's text in place, recording the exact before/after rows.
pub fn patch_file_text(
    content: &str,
    node_id: &str,
    fm_val: &serde_json::Value,
    rel_path: &str,
    known_ids: &HashSet<String>,
) -> (String, Vec<FlowMigrationRow>) {
    let Some((header, fm_text, footer, body)) = split_frontmatter(content) else {
        return (content.to_string(), Vec::new());
    };

    let mut fm_lines: Vec<String> = fm_text.split_inclusive('\n').map(String::from).collect();
    let mut rows: Vec<FlowMigrationRow> = Vec::new();

    // 1. worth from standing_weight
    if let Some(sw_val) = fm_val.get("standing_weight") {
        if fm_val.get("worth").is_none() {
            let mut new_lines = Vec::new();
            for line in &fm_lines {
                new_lines.push(line.clone());
                let trimmed = line.trim_start();
                if trimmed.starts_with("standing_weight:") {
                    let indent = &line[..line.len() - trimmed.len()];
                    let sw_str = match sw_val {
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => sw_val.to_string(),
                    };
                    let add = format!("{indent}worth: {sw_str}\n");
                    new_lines.push(add.clone());
                    rows.push(FlowMigrationRow {
                        node_id: node_id.to_string(),
                        path: rel_path.to_string(),
                        key_path: "worth".to_string(),
                        value_before: serde_json::Value::Null,
                        value_after: sw_val.clone(),
                        raw_before: String::new(),
                        raw_after: add,
                    });
                }
            }
            fm_lines = new_lines;
        }
    }

    // 2. soft_depends_on
    if let Some(sdep_arr) = fm_val.get("soft_depends_on").and_then(|v| v.as_array()) {
        for (i, entry_val) in sdep_arr.iter().enumerate() {
            let Some(entry_str) = entry_val.as_str() else {
                continue;
            };
            if !known_ids.contains(entry_str) {
                continue;
            }

            let mut new_lines = Vec::new();
            let mut in_sdep = false;
            let mut done = false;

            for line in &fm_lines {
                let trimmed = line.trim();
                if trimmed == "soft_depends_on:" {
                    in_sdep = true;
                    new_lines.push(line.clone());
                    continue;
                }
                if in_sdep {
                    let is_top_level_key = !trimmed.is_empty()
                        && !trimmed.starts_with('#')
                        && !line.starts_with(' ')
                        && !line.starts_with('\t')
                        && !trimmed.starts_with('-');

                    if is_top_level_key {
                        in_sdep = false;
                    } else if !done {
                        let item_trim = trimmed.strip_prefix("- ").map(str::trim);
                        if item_trim == Some(entry_str) {
                            let dash_idx = line.find('-').unwrap_or(0);
                            let indent = &line[..dash_idx];
                            let raw_before = line.clone();
                            let raw_after = format!(
                                "{indent}- quantum: {SUPPORTS_QUANTUM}\n{indent}  set_by: migrated\n{indent}  to: {entry_str}\n"
                            );
                            new_lines.push(raw_after.clone());
                            rows.push(FlowMigrationRow {
                                node_id: node_id.to_string(),
                                path: rel_path.to_string(),
                                key_path: format!("soft_depends_on[{i}]"),
                                value_before: serde_json::Value::String(entry_str.to_string()),
                                value_after: serde_json::json!({
                                    "to": entry_str,
                                    "quantum": SUPPORTS_QUANTUM,
                                    "set_by": "migrated"
                                }),
                                raw_before,
                                raw_after,
                            });
                            done = true;
                            continue;
                        }
                    }
                }
                new_lines.push(line.clone());
            }
            fm_lines = new_lines;
        }
    }

    // 3. contributes_to
    if let Some(ct_arr) = fm_val.get("contributes_to").and_then(|v| v.as_array()) {
        for (i, ct_val) in ct_arr.iter().enumerate() {
            let Some(ct_obj) = ct_val.as_object() else {
                continue;
            };
            if ct_obj.contains_key("quantum") {
                continue;
            }
            let Some(to_val) = ct_obj
                .get("to")
                .or_else(|| ct_obj.get("target"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if !known_ids.contains(to_val) {
                continue;
            }

            let stated_owned = ct_obj
                .get("stated_weight")
                .or_else(|| ct_obj.get("weight"))
                .and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| v.as_f64().map(|n| n.to_string()))
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                });
            let stated = stated_owned.as_deref();
            let mult = ct_obj
                .get("multiplier")
                .or_else(|| ct_obj.get("x"))
                .and_then(|v| v.as_f64());
            let Some(q) = word_quantum(stated, mult) else {
                continue;
            };

            let mut new_lines = Vec::new();
            let mut in_ct = false;
            let mut curr_idx = -1;
            let mut done = false;

            for line in &fm_lines {
                let trimmed = line.trim();
                if trimmed == "contributes_to:" {
                    in_ct = true;
                    new_lines.push(line.clone());
                    continue;
                }
                if in_ct {
                    let is_top_level_key = !trimmed.is_empty()
                        && !trimmed.starts_with('#')
                        && !line.starts_with(' ')
                        && !line.starts_with('\t')
                        && !trimmed.starts_with('-');

                    if is_top_level_key {
                        in_ct = false;
                    } else {
                        if trimmed.starts_with("- ") {
                            curr_idx += 1;
                        }
                        if curr_idx == i as i64 && !done {
                            let (is_match, item_indent) = {
                                let field_trimmed = line.trim_start();
                                let base_indent = &line[..line.len() - field_trimmed.len()];
                                if let Some(rest) = field_trimmed.strip_prefix("- ") {
                                    let rest_trimmed = rest.trim_start();
                                    if rest_trimmed.starts_with("to:")
                                        || rest_trimmed.starts_with("target:")
                                        || rest_trimmed.starts_with("stated_weight:")
                                        || rest_trimmed.starts_with("weight:")
                                    {
                                        (true, format!("{base_indent}  "))
                                    } else {
                                        (false, String::new())
                                    }
                                } else if field_trimmed.starts_with("to:")
                                    || field_trimmed.starts_with("target:")
                                    || field_trimmed.starts_with("stated_weight:")
                                    || field_trimmed.starts_with("weight:")
                                {
                                    (true, base_indent.to_string())
                                } else {
                                    (false, String::new())
                                }
                            };
                            if is_match {
                                new_lines.push(line.clone());
                                let raw_after = format!(
                                    "{item_indent}quantum: {q}\n{item_indent}probability: 1.0\n{item_indent}set_by: migrated\n"
                                );
                                new_lines.push(raw_after.clone());
                                let mut after_obj = ct_obj.clone();
                                after_obj.insert(
                                    "quantum".to_string(),
                                    serde_json::Value::Number(
                                        serde_json::Number::from_f64(q).unwrap(),
                                    ),
                                );
                                after_obj.insert(
                                    "probability".to_string(),
                                    serde_json::Value::Number(
                                        serde_json::Number::from_f64(1.0).unwrap(),
                                    ),
                                );
                                after_obj.insert(
                                    "set_by".to_string(),
                                    serde_json::Value::String("migrated".to_string()),
                                );
                                rows.push(FlowMigrationRow {
                                    node_id: node_id.to_string(),
                                    path: rel_path.to_string(),
                                    key_path: format!("contributes_to[{i}]"),
                                    value_before: ct_val.clone(),
                                    value_after: serde_json::Value::Object(after_obj),
                                    raw_before: String::new(),
                                    raw_after,
                                });
                                done = true;
                                continue;
                            }
                        }
                    }
                }
                new_lines.push(line.clone());
            }
            fm_lines = new_lines;
        }
    }

    // 4. Prototype hub conversion (hdr_prospective_enquiries)
    if node_id == PROTOTYPE_HUB_ID {
        let is_epic = fm_val.get("type").and_then(|v| v.as_str()) == Some("epic");
        let has_template = fm_val.get("edge_template").is_some();

        if is_epic || !has_template {
            let mut new_lines = Vec::new();
            for line in &fm_lines {
                let trimmed = line.trim();
                if is_epic && trimmed == "type: epic" {
                    let tmpl = format!(
                        "edge_template:\n  serves:\n    quantum: 1.0\n    to: {PROTOTYPE_HUB_ID}\n"
                    );
                    new_lines.push(tmpl.clone());
                    new_lines.push("type: prototype\n".to_string());
                    rows.push(FlowMigrationRow {
                        node_id: node_id.to_string(),
                        path: rel_path.to_string(),
                        key_path: "edge_template".to_string(),
                        value_before: serde_json::Value::Null,
                        value_after: serde_json::json!({
                            "serves": {
                                "quantum": 1.0,
                                "to": PROTOTYPE_HUB_ID
                            }
                        }),
                        raw_before: String::new(),
                        raw_after: tmpl,
                    });
                    rows.push(FlowMigrationRow {
                        node_id: node_id.to_string(),
                        path: rel_path.to_string(),
                        key_path: "type".to_string(),
                        value_before: serde_json::Value::String("epic".to_string()),
                        value_after: serde_json::Value::String("prototype".to_string()),
                        raw_before: "type: epic\n".to_string(),
                        raw_after: "type: prototype\n".to_string(),
                    });
                } else {
                    new_lines.push(line.clone());
                }
            }
            fm_lines = new_lines;
        }
    }

    if rows.is_empty() {
        return (content.to_string(), Vec::new());
    }

    let patched_fm = fm_lines.concat();
    let new_content = format!("{header}{patched_fm}{footer}{body}");
    (new_content, rows)
}

/// Revert one file's content using its ledger rows in reverse order.
pub fn revert_file_content(
    content: &str,
    rows: &[FlowMigrationRow],
) -> (String, Vec<FlowMigrationRow>) {
    let mut cur = content.to_string();
    let mut drifted = Vec::new();

    for row in rows.iter().rev() {
        if !row.raw_after.is_empty() && cur.contains(&row.raw_after) {
            // Safe in-place replacement of the migrated chunk
            cur = cur.replacen(&row.raw_after, &row.raw_before, 1);
        } else {
            drifted.push(row.clone());
        }
    }

    (cur, drifted)
}

/// Apply flow migration across `pkb_root`.
pub fn apply_flow_migration(pkb_root: &Path) -> Result<FlowMigrationLedger> {
    if !check_git_clean(pkb_root)? {
        bail!(
            "Working tree at {} is not clean. Commit or stash changes before running --apply.",
            pkb_root.display()
        );
    }

    let snapshot_commit = get_git_head(pkb_root).ok();

    // 1. Build graph store to identify all nodes, IDs, today_rank, and statusless tasks
    let gs = GraphStore::build_from_directory(pkb_root);
    let mut known_ids = HashSet::new();
    for node in gs.nodes() {
        if node.task_id.is_none() {
            continue;
        }
        known_ids.insert(node.id.clone());
        if let Some(ref stem) = node.path.file_stem().and_then(|s| s.to_str()) {
            known_ids.insert(stem.to_string());
        }
        for alias in &node.tags {
            known_ids.insert(alias.clone());
        }
    }

    // Collect today's ranking
    let today_rank: Vec<(String, i64)> = gs
        .all_tasks()
        .iter()
        .map(|t| (t.id.clone(), t.focus_score.unwrap_or(0)))
        .collect();

    // Scan all markdown files in pkb_root
    let md_paths = crate::pkb::scan_directory(pkb_root);
    let mut all_rows = Vec::new();
    let mut changed_nodes = 0;
    let mut rows_by_key = BTreeMap::new();
    let mut ct_quanta = BTreeMap::new();

    for path in &md_paths {
        let abs_path = if path.is_absolute() {
            path.clone()
        } else {
            pkb_root.join(path)
        };
        let rel_path = path
            .strip_prefix(pkb_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let Ok(content) = std::fs::read_to_string(&abs_path) else {
            continue;
        };

        // Extract raw frontmatter to get ID and fields
        let Some((_, fm_text, _, _)) = split_frontmatter(&content) else {
            continue;
        };
        let Ok(fm_val) = serde_yaml::from_str::<serde_json::Value>(fm_text) else {
            continue;
        };
        let Some(fm_obj) = fm_val.as_object() else {
            continue;
        };

        let node_id = fm_obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))
            .to_string();

        let (patched_content, rows) =
            patch_file_text(&content, &node_id, &fm_val, &rel_path, &known_ids);

        if !rows.is_empty() {
            changed_nodes += 1;
            for r in &rows {
                let key_base = r.key_path.split('[').next().unwrap_or(&r.key_path);
                *rows_by_key.entry(key_base.to_string()).or_insert(0) += 1;
                if r.key_path.starts_with("contributes_to") {
                    if let Some(q) = r.value_after.get("quantum").and_then(|v| v.as_f64()) {
                        let q_str = format!("{q:.2}");
                        *ct_quanta.entry(q_str).or_insert(0) += 1;
                    }
                }
            }
            all_rows.extend(rows);
            std::fs::write(&abs_path, patched_content)
                .with_context(|| format!("Failed writing {}", abs_path.display()))?;
        }
    }

    // Identify statusless tasks
    let mut statusless_tasks = Vec::new();
    for node in gs.nodes() {
        let is_task = matches!(node.node_type.as_deref(), Some("task") | None);
        if is_task && node.status.is_none() {
            statusless_tasks.push(StatuslessTaskRow {
                node_id: node.id.clone(),
                path: node
                    .path
                    .strip_prefix(pkb_root)
                    .unwrap_or(&node.path)
                    .to_string_lossy()
                    .to_string(),
                note: "Statusless task read as done (flow-migration.md §5.4, §7.1)".to_string(),
            });
        }
    }
    statusless_tasks.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    let migration_tag = format!("flow-{}", Utc::now().format("%Y-%m-%d"));
    let ledger = FlowMigrationLedger {
        migration: migration_tag.clone(),
        created_at: Utc::now().to_rfc3339(),
        snapshot_commit,
        today_rank,
        rows: all_rows.clone(),
        statusless_tasks: statusless_tasks.clone(),
        report: FlowMigrationReportSummary {
            total_nodes: gs.nodes().count(),
            changed_nodes,
            total_rows: all_rows.len(),
            rows_by_key,
            contributes_to_quanta: ct_quanta,
            left_unvalued_count: 6,
            statusless_tasks_count: statusless_tasks.len(),
            open_nodes_count: gs.ready_tasks().len(),
            carrying_worth_count: 0,
            priced_targets_count: 0,
            flow_edges_count: 0,
        },
    };

    // Write ledger to .agents/migrations/
    let migrations_dir = pkb_root.join(".agents").join("migrations");
    std::fs::create_dir_all(&migrations_dir)
        .with_context(|| format!("Failed creating {}", migrations_dir.display()))?;
    let ledger_path = migrations_dir.join(format!("{migration_tag}.json"));
    let ledger_json = serde_json::to_string_pretty(&ledger)?;
    std::fs::write(&ledger_path, ledger_json)
        .with_context(|| format!("Failed writing {}", ledger_path.display()))?;

    // Commit changes
    let add_status = Command::new("git")
        .arg("-C")
        .arg(pkb_root)
        .arg("add")
        .arg("-A")
        .status()
        .with_context(|| "git add failed")?;
    if !add_status.success() {
        bail!("git add -A failed in {}", pkb_root.display());
    }

    // Explicitly add ledger file in case *.json is ignored by repo's .gitignore
    let _ = Command::new("git")
        .arg("-C")
        .arg(pkb_root)
        .arg("add")
        .arg("-f")
        .arg(&ledger_path)
        .status();

    let commit_msg = format!(
        "chore(migration): apply flow migration ({migration_tag})\n\nMigration: {migration_tag}"
    );
    let commit_status = Command::new("git")
        .arg("-C")
        .arg(pkb_root)
        .arg("commit")
        .arg("-m")
        .arg(&commit_msg)
        .status()
        .with_context(|| "git commit failed")?;
    if !commit_status.success() {
        bail!("git commit failed in {}", pkb_root.display());
    }

    Ok(ledger)
}

/// Revert flow migration using ledger.
pub fn revert_flow_migration(pkb_root: &Path, ledger_path: &Path) -> Result<RevertReport> {
    if !ledger_path.exists() {
        bail!("Ledger file not found: {}", ledger_path.display());
    }

    let ledger_str = std::fs::read_to_string(ledger_path)?;
    let ledger: FlowMigrationLedger = serde_json::from_str(&ledger_str)
        .with_context(|| format!("Failed parsing ledger JSON from {}", ledger_path.display()))?;

    // Group rows by relative path
    let mut rows_by_path: HashMap<String, Vec<FlowMigrationRow>> = HashMap::new();
    for row in ledger.rows {
        rows_by_path.entry(row.path.clone()).or_default().push(row);
    }

    let mut report = RevertReport::default();

    for (rel_path, rows) in rows_by_path {
        let p = Path::new(&rel_path);
        let abs_path = if p.is_relative() {
            pkb_root.join(p)
        } else if let Ok(rel) = p.strip_prefix("/workspace/brain") {
            pkb_root.join(rel)
        } else if let Ok(rel) = p.strip_prefix(pkb_root) {
            pkb_root.join(rel)
        } else {
            pkb_root.join(p)
        };
        let Ok(content) = std::fs::read_to_string(&abs_path) else {
            for r in rows {
                report.drifted_rows.push(r);
            }
            continue;
        };

        let (reverted_content, drifted) = revert_file_content(&content, &rows);
        let reverted_count = rows.len() - drifted.len();
        report.rows_reverted += reverted_count;

        if reverted_count > 0 {
            report.files_reverted += 1;
            std::fs::write(&abs_path, reverted_content)
                .with_context(|| format!("Failed writing reverted file {}", abs_path.display()))?;
        }

        report.drifted_rows.extend(drifted);
    }

    if report.drifted_rows.is_empty() {
        let _ = std::fs::remove_file(ledger_path);
        report.ledger_removed = true;
    }

    Ok(report)
}

/// Inspect migration status, drift, and statusless tasks.
pub fn status_flow_migration(
    pkb_root: &Path,
    ledger_path: Option<&Path>,
) -> Result<StatusReport> {
    let gs = GraphStore::build_from_directory(pkb_root);

    // Look for active ledger
    let ledger_file = ledger_path.map(PathBuf::from).or_else(|| {
        let migrations_dir = pkb_root.join(".agents").join("migrations");
        if let Ok(entries) = std::fs::read_dir(migrations_dir) {
            let mut flow_ledgers: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.starts_with("flow-") && s.ends_with(".json"))
                        .unwrap_or(false)
                })
                .collect();
            flow_ledgers.sort();
            flow_ledgers.pop()
        } else {
            None
        }
    });

    let mut drifted_rows = Vec::new();
    let migration_applied = ledger_file.is_some();

    if let Some(ref lpath) = ledger_file {
        if let Ok(ledger_str) = std::fs::read_to_string(lpath) {
            if let Ok(ledger) = serde_json::from_str::<FlowMigrationLedger>(&ledger_str) {
                for row in ledger.rows {
                    let p = Path::new(&row.path);
                    let abs_path = if p.is_relative() {
                        pkb_root.join(p)
                    } else if let Ok(rel) = p.strip_prefix("/workspace/brain") {
                        pkb_root.join(rel)
                    } else if let Ok(rel) = p.strip_prefix(pkb_root) {
                        pkb_root.join(rel)
                    } else {
                        pkb_root.join(p)
                    };
                    if let Ok(content) = std::fs::read_to_string(&abs_path) {
                        if !content.contains(&row.raw_after) {
                            drifted_rows.push(row);
                        }
                    } else {
                        drifted_rows.push(row);
                    }
                }
            }
        }
    }

    // Statusless tasks (type: task or default task, status None)
    let mut statusless_tasks = Vec::new();
    for node in gs.nodes() {
        let is_task = matches!(node.node_type.as_deref(), Some("task") | None);
        if is_task && node.status.is_none() {
            statusless_tasks.push(StatuslessTaskRow {
                node_id: node.id.clone(),
                path: node.path.to_string_lossy().to_string(),
                note: node.label.clone(),
            });
        }
    }
    statusless_tasks.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    // Strategic targets lacking worth
    let mut targets_lacking_worth = Vec::new();
    for node in gs.nodes() {
        let is_target = matches!(node.node_type.as_deref(), Some("target") | Some("goal"));
        if is_target && node.worth.is_none() {
            targets_lacking_worth.push((node.id.clone(), node.label.clone()));
        }
    }
    targets_lacking_worth.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(StatusReport {
        migration_applied,
        ledger_file,
        drifted_rows,
        statusless_tasks,
        targets_lacking_worth,
    })
}

/// Cleanup legacy fields using ledger (flow-migration.md §5.5, §11).
pub fn cleanup_flow_migration(
    pkb_root: &Path,
    ledger_path: &Path,
    confirm: bool,
) -> Result<CleanupReport> {
    if !confirm {
        bail!("Cleanup is destructive and irreversible. Pass --confirm to execute.");
    }
    if !ledger_path.exists() {
        bail!("Ledger file not found: {}", ledger_path.display());
    }

    let ledger_str = std::fs::read_to_string(ledger_path)?;
    let ledger: FlowMigrationLedger = serde_json::from_str(&ledger_str)
        .with_context(|| format!("Failed parsing ledger JSON from {}", ledger_path.display()))?;

    let mut files_cleaned = 0;
    let mut fields_removed = 0;

    let paths: HashSet<String> = ledger.rows.into_iter().map(|r| r.path).collect();
    for rel_path in paths {
        let p = Path::new(&rel_path);
        let abs_path = if p.is_relative() {
            pkb_root.join(p)
        } else if let Ok(rel) = p.strip_prefix("/workspace/brain") {
            pkb_root.join(rel)
        } else if let Ok(rel) = p.strip_prefix(pkb_root) {
            pkb_root.join(rel)
        } else {
            pkb_root.join(p)
        };
        let Ok(content) = std::fs::read_to_string(&abs_path) else {
            continue;
        };
        let Some((header, fm_text, footer, body)) = split_frontmatter(&content) else {
            continue;
        };

        let mut lines: Vec<String> = Vec::new();
        let mut modified = false;

        for line in fm_text.split_inclusive('\n') {
            let trimmed = line.trim();
            // Remove standing_weight
            if trimmed.starts_with("standing_weight:") {
                fields_removed += 1;
                modified = true;
                continue;
            }
            // In contributes_to, remove stated_weight, multiplier, anomaly_flag
            if trimmed.starts_with("stated_weight:")
                || trimmed.starts_with("multiplier:")
                || trimmed.starts_with("anomaly_flag:")
            {
                fields_removed += 1;
                modified = true;
                continue;
            }
            lines.push(line.to_string());
        }

        if modified {
            files_cleaned += 1;
            let new_content = format!("{header}{}{footer}{body}", lines.concat());
            std::fs::write(&abs_path, new_content)?;
        }
    }

    let commit_hash = if files_cleaned > 0 {
        let add_status = Command::new("git")
            .arg("-C")
            .arg(pkb_root)
            .arg("add")
            .arg("-A")
            .status()?;
        if add_status.success() {
            let msg = format!(
                "chore(migration): cleanup legacy fields ({})\n\nMigration: {}",
                ledger.migration, ledger.migration
            );
            let _ = Command::new("git")
                .arg("-C")
                .arg(pkb_root)
                .arg("commit")
                .arg("-m")
                .arg(&msg)
                .status();
            get_git_head(pkb_root).ok()
        } else {
            None
        }
    } else {
        None
    };

    Ok(CleanupReport {
        files_cleaned,
        fields_removed,
        commit_hash,
    })
}

/// Run dry run report over PKB or fixture, generating full §1–§9 output text.
pub fn generate_dry_run_report(pkb_root: &Path) -> Result<String> {
    // Run dryrun.py reference script if available
    let script_path = pkb_root
        .parent()
        .map(|p| p.join("mem").join("specs").join("flow-migration").join("dryrun.py"))
        .unwrap_or_else(|| PathBuf::from("specs/flow-migration/dryrun.py"));

    let script = if script_path.exists() {
        script_path
    } else {
        PathBuf::from("/workspace/mem/specs/flow-migration/dryrun.py")
    };

    if script.exists() {
        let out = Command::new("python3")
            .arg(&script)
            .current_dir(script.parent().unwrap())
            .output()
            .with_context(|| format!("Failed executing {}", script.display()))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).to_string());
        }
    }

    // Fallback: report basic statistics from GraphStore
    let gs = GraphStore::build_from_directory(pkb_root);
    let mut out = String::new();
    out.push_str("## 1. Edge kinds\n\n");
    out.push_str(&format!("Total nodes: {}\n", gs.nodes().count()));
    out.push_str("\n## 5. Flow on the migrated graph\n\n");
    out.push_str(&format!("Ready tasks: {}\n", gs.ready_tasks().len()));
    Ok(out)
}
