//! PKB linter and formatter — validates and auto-fixes markdown files with YAML frontmatter.
//!
//! Rules are PKB-specific: frontmatter schema validation, status/type canonicalization,
//! YAML key ordering, markdown hygiene, and cross-reference integrity.

use crate::graph::{self, VALID_NODE_TYPES, VALID_STATUSES};
use crate::pkb;
use gray_matter::engine::YAML;
use gray_matter::Matter;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

static ID_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

fn get_id_regex() -> &'static regex::Regex {
    ID_RE.get_or_init(|| regex::Regex::new(r"(?i)^[a-z0-9][a-z0-9-]*-[a-f0-9]{8}$").unwrap())
}

// ── Diagnostic types ─────────────────────────────────────────────────────

/// Severity level for lint diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Auto-fixable style issue
    Style,
    /// Potential problem worth investigating
    Warning,
    /// Definite error that will cause issues
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Style => write!(f, "style"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// A single lint diagnostic attached to a file.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
    pub line: Option<usize>,
    pub fixable: bool,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(line) = self.line {
            write!(
                f,
                "[{}] {}: {} (line {})",
                self.severity, self.rule, self.message, line
            )
        } else {
            write!(f, "[{}] {}: {}", self.severity, self.rule, self.message)
        }
    }
}

/// Results for a single file.
#[derive(Debug)]
pub struct FileResult {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    /// If fix mode is on, this contains the corrected file content.
    pub fixed_content: Option<String>,
}

/// Summary statistics across all linted files.
#[derive(Debug, Default)]
pub struct LintSummary {
    pub files_checked: usize,
    pub files_with_issues: usize,
    pub files_fixed: usize,
    pub errors: usize,
    pub warnings: usize,
    pub style: usize,
}

impl LintSummary {
    /// Build summary from a slice of file results.
    pub fn from_results(results: &[FileResult]) -> Self {
        let mut summary = LintSummary {
            files_checked: results.len(),
            ..Default::default()
        };
        for r in results {
            if !r.diagnostics.is_empty() {
                summary.files_with_issues += 1;
            }
            if r.fixed_content.is_some() {
                summary.files_fixed += 1;
            }
            for d in &r.diagnostics {
                match d.severity {
                    Severity::Error => summary.errors += 1,
                    Severity::Warning => summary.warnings += 1,
                    Severity::Style => summary.style += 1,
                }
            }
        }
        summary
    }
}

// ── Known frontmatter keys ───────────────────────────────────────────────

const KNOWN_KEYS: &[&str] = &[
    "id",
    "title",
    "type",
    "status",
    "priority",
    "project",
    "parent",
    "depends_on",
    "soft_depends_on",
    "blocks",
    "soft_blocks",
    "assignee",
    "complexity",
    "due",
    "created",
    "modified",
    "source",
    "confidence",
    "supersedes",
    "superseded_by",
    "permalink",
    "aliases",
    "alias",
    "order",
    "depth",
    "leaf",
    "tags",
    "children",
    "assumptions",
    "word_count",
    "date",
    "task_id",
    "archived_at",
    "classification",
    "metadata",
    "contributes_to",
    "follow_up_tasks",
    "session_id",
    "issue_url",
    "release_summary",
    "progress",
    // Mobile capture / triage workflow keys
    "processed",
    "processed_date",
    "triage_action",
    "triage_ref",
    // Content metadata keys
    "topic",
    "generated_by",
    "extracted",
    "body",     // kept here so fm-unknown-key doesn't fire; fm-prohibited-body handles it
    "epic",
    "summary",
    "notes",
    "description",
    // Email-sourced task keys
    "email_date",
    "email_from",
    "email_subject",
    // Workflow / decomposition keys
    "step",
    "total_steps",
    "end_goal",
    "updated",
    "duration_minutes",
    "scheduled",
    "deadline",
    "version",
    // Academic / publication keys
    "author",
    "authors",
    "reviewer",
    "venue",
    "manuscript",
    "publication",
    "journal",
];

// ── Type alias resolution ────────────────────────────────────────────────

/// Map unknown type values to the nearest canonical type.
fn resolve_type_alias(t: &str) -> &'static str {
    match t {
        // Collapsed actionable types → task
        "bug" | "feature" | "action" | "milestone" | "subproject" => "task",
        // Reference aliases
        "article" | "reading-guide" | "talk" => "reference",
        "observation" | "insight" | "exploration" => "note",
        "session-log" => "session-log",
        "review" | "review-notes" | "peer-review" => "review",
        "daily" => "daily",
        "case" => "case",
        "index" => "index",
        "spec" | "design" => "spec",
        "audit" | "audit-report" => "audit-report",
        "reference" => "reference",
        "goal" | "target" => "goal",
        "instructions" | "role" | "agent" | "bundle" => "document",
        _ => "document",
    }
}

// ── Core lint + fix engine ───────────────────────────────────────────────

/// Extract a prefix from a non-conforming ID for generating a new one.
/// e.g. "osb" → "osb", "explorations-np-003" → "explorations", "ip-australia" → "ip"
fn extract_id_prefix(id: &str) -> String {
    // Take the first segment (before the first hyphen), unless it's very short
    let parts: Vec<&str> = id.split('-').collect();
    if parts.len() >= 2 && parts[0].len() >= 2 {
        // Use first segment, or first two if both are alpha
        if parts[1].chars().all(|c| c.is_alphabetic()) && parts.len() >= 3 {
            return format!("{}-{}", parts[0], parts[1]);
        }
        return parts[0].to_string();
    }
    id.to_string()
}

/// Extract an ID prefix from a filename stem if it matches `prefix-hexhash-slug` pattern.
/// Returns the `prefix-hexhash` portion, or None.
fn extract_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    // Match patterns like "academic-8c6h05e2-cite-her-work" or "aops-core-6a4f03c0-fix-something"
    // The hash portion is alphanumeric (may contain letters beyond a-f)
    let re = { static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new(); RE.get_or_init(|| regex::Regex::new(r"(?i)^([a-z][\w-]*?-[a-z0-9]{6,10})(?:-.+)?$").unwrap()) };
    re.captures(&stem).map(|c| c[1].to_string())
}

/// Generate an ID for a file that's missing one.
/// Tries: filename pattern extraction → project field → parent dir → "task"
fn generate_missing_id(path: &Path, fm: &serde_json::Map<String, serde_json::Value>) -> String {
    // First try extracting from filename
    if let Some(id) = extract_id_from_filename(path) {
        return id;
    }

    // Use project field as prefix, or parent directory name
    let prefix = fm
        .get("project")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "task".to_string())
        });

    crate::graph::create_id(&prefix)
}

/// Lint a single file. If `fix` is true, also produce corrected content.
pub fn lint_file(path: &Path, fix: bool, known_ids: Option<&HashSet<String>>, ancestor_map: Option<&AncestorMap>, children_set: Option<&ChildrenSet>) -> FileResult {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return FileResult {
                path: path.to_path_buf(),
                diagnostics: vec![Diagnostic {
                    severity: Severity::Error,
                    rule: "io-error",
                    message: format!("Cannot read file: {e}"),
                    line: None,
                    fixable: false,
                }],
                fixed_content: None,
            };
        }
    };

    let mut diags = Vec::new();

    // Parse frontmatter
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(&content);
    let yaml_ok = parsed
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        .and_then(|v| if v.is_object() { Some(v) } else { None });
    let used_fallback = yaml_ok.is_none() && content.starts_with("---");
    let fm_data = yaml_ok.or_else(|| fallback_parse_frontmatter(&content));

    // If fallback was needed and succeeded, the YAML has quoting issues
    if used_fallback && fm_data.is_some() {
        diags.push(Diagnostic {
            severity: Severity::Error,
            rule: "fm-yaml-quoting",
            message: "Frontmatter has unquoted values with colons — needs quoting".into(),
            line: Some(1),
            fixable: true,
        });
    }

    // ── Frontmatter rules ────────────────────────────────────────────

    check_frontmatter(&content, &fm_data, &mut diags, known_ids, ancestor_map, children_set);

    // ── Markdown body rules ──────────────────────────────────────────

    check_markdown_body(&content, &mut diags);

    // ── Build fixed content if requested ─────────────────────────────

    let fixed_content = if fix && diags.iter().any(|d| d.fixable) {
        let fixed = apply_fixes(&content, &fm_data, path, ancestor_map);
        if fixed != content { Some(fixed) } else { None }
    } else {
        None
    };

    FileResult {
        path: path.to_path_buf(),
        diagnostics: diags,
        fixed_content,
    }
}

/// Fallback frontmatter parser for files where serde_yaml chokes
/// (e.g. unquoted values containing `: `). Parses simple `key: value` lines.
fn fallback_parse_frontmatter(content: &str) -> Option<serde_json::Value> {
    if !content.starts_with("---") {
        return None;
    }
    let end = content[3..].find("\n---")?;
    let fm_text = &content[4..end + 3];

    let mut map = serde_json::Map::new();
    let mut current_key: Option<String> = None;
    let mut in_array = false;
    let mut array_items: Vec<serde_json::Value> = Vec::new();

    for line in fm_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Array item (- value)
        if trimmed.starts_with("- ") && current_key.is_some() {
            in_array = true;
            let val = trimmed[2..].trim();
            let val = val.trim_matches(|c| c == '\'' || c == '"');
            array_items.push(serde_json::Value::String(val.to_string()));
            continue;
        }

        // Flush previous array
        if in_array {
            if let Some(ref key) = current_key {
                map.insert(key.clone(), serde_json::Value::Array(array_items.clone()));
            }
            array_items.clear();
            in_array = false;
        }

        // key: value — split on FIRST `:` (with optional space)
        if let Some((key_part, val_part)) = line.split_once(':') {
            let key = key_part.trim().to_string();
            let val = val_part.trim().to_string();

            // Inline array: [a, b, c]
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                let items: Vec<serde_json::Value> = inner
                    .split(',')
                    .map(|s| serde_json::Value::String(s.trim().trim_matches(|c| c == '\'' || c == '"').to_string()))
                    .collect();
                map.insert(key.clone(), serde_json::Value::Array(items));
            } else {
                let val = val.trim_matches(|c| c == '\'' || c == '"');
                // Try to parse as number
                if let Ok(n) = val.parse::<i64>() {
                    map.insert(key.clone(), serde_json::json!(n));
                } else if val == "true" || val == "false" {
                    map.insert(key.clone(), serde_json::json!(val == "true"));
                } else if val == "null" {
                    map.insert(key.clone(), serde_json::Value::Null);
                } else {
                    map.insert(key.clone(), serde_json::Value::String(val.to_string()));
                }
            }
            current_key = Some(key);
        }
    }

    // Flush trailing array
    if in_array {
        if let Some(ref key) = current_key {
            map.insert(key.clone(), serde_json::Value::Array(array_items));
        }
    }

    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

/// Map of document ID → (parent_id, doc_type) for ancestor lookups.
/// Built during lint_directory pre-pass.
pub type AncestorMap = HashMap<String, (Option<String>, Option<String>)>;

/// Set of IDs that appear as a parent of at least one other node.
/// A node in this set has children (scope > 0).
pub type ChildrenSet = HashSet<String>;

/// Walk the parent chain to check if any ancestor's ID matches the given value.
fn has_matching_ancestor(id: &str, project_value: &str, ancestor_map: &AncestorMap) -> bool {
    let mut current = id.to_string();
    let mut visited = HashSet::new();
    for _ in 0..20 {
        // max hops to avoid cycles
        if !visited.insert(current.clone()) {
            break;
        }
        let entry = match ancestor_map.get(&current) {
            Some(e) => e,
            None => break,
        };
        if let Some(ref parent_id) = entry.0 {
            if parent_id == project_value {
                return true;
            }
            current = parent_id.clone();
        } else {
            break;
        }
    }
    false
}

fn check_frontmatter(
    content: &str,
    fm_data: &Option<serde_json::Value>,
    diags: &mut Vec<Diagnostic>,
    known_ids: Option<&HashSet<String>>,
    ancestor_map: Option<&AncestorMap>,
    children_set: Option<&ChildrenSet>,
) {
    // Check frontmatter exists
    if !content.starts_with("---") {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            rule: "fm-missing",
            message: "File has no YAML frontmatter".into(),
            line: Some(1),
            fixable: false,
        });
        return;
    }

    let fm = match fm_data {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "fm-invalid",
                message: "Frontmatter is not a YAML mapping".into(),
                line: Some(1),
                fixable: false,
            });
            return;
        }
        None => {
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "fm-parse-error",
                message: "Failed to parse YAML frontmatter".into(),
                line: Some(1),
                fixable: false,
            });
            return;
        }
    };

    // Required: title
    if !fm.contains_key("title") {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            rule: "fm-no-title",
            message: "Missing 'title' in frontmatter".into(),
            line: Some(1),
            fixable: false,
        });
    }

    // Required: type
    if let Some(t) = fm.get("type").and_then(|v| v.as_str()) {
        if !graph::is_valid_node_type(t) {
            let mapped = resolve_type_alias(t);
            diags.push(Diagnostic {
                severity: Severity::Style,
                rule: "fm-unknown-type",
                message: format!(
                    "Unknown type '{}' → will fix to '{}'",
                    t, mapped
                ),
                line: None,
                fixable: true,
            });
        }
    } else if fm.get("type").is_some() {
        diags.push(Diagnostic {
            severity: Severity::Error,
            rule: "fm-type-not-string",
            message: "'type' must be a string".into(),
            line: None,
            fixable: false,
        });
    }

    // Status validation + alias detection
    if let Some(raw_status) = fm.get("status").and_then(|v| v.as_str()) {
        let canonical = graph::resolve_status_alias(raw_status);
        if canonical != raw_status {
            diags.push(Diagnostic {
                severity: Severity::Style,
                rule: "fm-status-alias",
                message: format!("Status '{}' should be canonical '{}'", raw_status, canonical),
                line: None,
                fixable: true,
            });
        }
        if !graph::is_valid_status(canonical) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                rule: "fm-unknown-status",
                message: format!(
                    "Unknown status '{}' → will fix to 'inbox'",
                    raw_status,
                ),
                line: None,
                fixable: true,
            });
        }
    } else if fm.get("status").is_some() {
        diags.push(Diagnostic {
            severity: Severity::Error,
            rule: "fm-status-not-string",
            message: "'status' must be a string".into(),
            line: None,
            fixable: false,
        });
    }

    // Priority validation
    if let Some(p) = fm.get("priority") {
        if let Some(n) = p.as_i64() {
            if !graph::is_valid_priority(n as i32) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    rule: "fm-priority-range",
                    message: format!("Priority {} outside expected range 0-4", n),
                    line: None,
                    fixable: false,
                });
            }
        } else if let Some(s) = p.as_str() {
            // Check if it's a "p1"/"P2" style priority we can fix
            let stripped = s.strip_prefix('p').or_else(|| s.strip_prefix('P'));
            let can_fix = stripped.map(|n| n.parse::<i64>().is_ok()).unwrap_or(false);
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "fm-priority-type",
                message: format!("'priority' must be an integer (got '{}')", s),
                line: None,
                fixable: can_fix,
            });
        } else if !p.is_number() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "fm-priority-type",
                message: "'priority' must be an integer".into(),
                line: None,
                fixable: false,
            });
        }
    }

    // Effort validation
    if let Some(effort) = fm.get("effort").and_then(|v| v.as_str()) {
        if !graph::is_valid_effort(effort) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                rule: "fm-invalid-effort",
                message: format!(
                    "Unrecognised effort value '{}' — expected duration string like '1d', '2h', '1w'",
                    effort
                ),
                line: None,
                fixable: false,
            });
        }
    }

    // Tags should be an array
    if let Some(tags) = fm.get("tags") {
        if !tags.is_array() && !tags.is_string() {
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "fm-tags-type",
                message: "'tags' must be a list or comma-separated string".into(),
                line: None,
                fixable: false,
            });
        }
    }

    // id format check (should match prefix-hex pattern)
    // Prefix may contain uppercase letters (e.g. "academicOps-b5d43955" is valid)
    // Goals and projects use special canonical IDs (e.g. "my-project") — skip format check.
    let node_type_for_id = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let is_root_node = matches!(node_type_for_id, "goal" | "project");
    if let Some(id) = fm.get("id").and_then(|v| v.as_str()) {
        let id_re = get_id_regex();
        if !is_root_node && !id_re.is_match(id) && !id.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Style,
                rule: "fm-id-format",
                message: format!(
                    "ID '{}' doesn't match expected pattern 'prefix-hexhash'",
                    id
                ),
                line: None,
                fixable: true,
            });
        }
    }

    // Prohibited: body — content must live in the markdown body section, not frontmatter
    if fm.contains_key("body") {
        diags.push(Diagnostic {
            severity: Severity::Error,
            rule: "fm-prohibited-body",
            message: "'body' is a prohibited frontmatter key — content belongs in the markdown body (run with --fix to auto-migrate)".into(),
            line: None,
            fixable: true,
        });
    }

    // Deprecated: project field
    if let Some(project_val) = fm.get("project").and_then(|v| v.as_str()) {
        let doc_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let has_parent = fm.contains_key("parent");
        // Fixable if: no parent (orphan), or has ancestor matching project value
        let fixable = if !has_parent {
            true
        } else if let Some(amap) = ancestor_map {
            has_matching_ancestor(doc_id, project_val, amap)
        } else {
            false
        };
        diags.push(Diagnostic {
            severity: Severity::Warning,
            rule: "fm-deprecated-project",
            message: "frontmatter contains deprecated 'project' field — project membership is derived from parent hierarchy".into(),
            line: None,
            fixable,
        });
    }

    // Unknown keys
    let known: HashSet<&str> = KNOWN_KEYS.iter().copied().collect();
    for key in fm.keys() {
        if !known.contains(key.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Style,
                rule: "fm-unknown-key",
                message: format!("Unknown frontmatter key '{}'", key),
                line: None,
                fixable: false,
            });
        }
    }

    // Reference integrity: parent, depends_on, soft_depends_on
    if let Some(known_ids) = known_ids {
        if let Some(parent) = fm.get("parent").and_then(|v| v.as_str()) {
            if !known_ids.contains(parent) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    rule: "ref-broken-parent",
                    message: format!("Parent '{}' not found in PKB", parent),
                    line: None,
                    fixable: false,
                });
            }
        }
        for key in &["depends_on", "soft_depends_on", "blocks", "soft_blocks"] {
            if let Some(arr) = fm.get(*key).and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(ref_id) = item.as_str() {
                        if !known_ids.contains(ref_id) {
                            diags.push(Diagnostic {
                                severity: Severity::Warning,
                                rule: "ref-broken-dep",
                                message: format!(
                                    "{} reference '{}' not found in PKB",
                                    key, ref_id
                                ),
                                line: None,
                                fixable: false,
                            });
                        }
                    }
                }
            }
        }

        // Single-value reference fields: supersedes, superseded_by
        for key in &["supersedes", "superseded_by"] {
            if let Some(ref_id) = fm.get(*key).and_then(|v| v.as_str()) {
                if !known_ids.contains(ref_id) {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        rule: "ref-broken-dep",
                        message: format!("{} reference '{}' not found in PKB", key, ref_id),
                        line: None,
                        fixable: false,
                    });
                }
            }
        }
    }

    // All documents should have an explicit id
    if !fm.contains_key("id") {
        if fm.contains_key("task_id") {
            diags.push(Diagnostic {
                severity: Severity::Style,
                rule: "task-legacy-id",
                message: "Document uses legacy 'task_id' instead of 'id'".into(),
                line: None,
                fixable: true,
            });
        } else {
            diags.push(Diagnostic {
                severity: Severity::Error,
                rule: "task-no-id",
                message: "Document is missing 'id' field".into(),
                line: None,
                fixable: true,
            });
        }
    }

    // All documents should have a type
    if !fm.contains_key("type") {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            rule: "doc-no-type",
            message: "Document is missing 'type' field".into(),
            line: None,
            fixable: false,
        });
    }

    // Task-type-specific checks
    let node_type = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let is_task_type = graph::TASK_TYPES.contains(&node_type);
    if is_task_type {
        if !fm.contains_key("status") {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                rule: "task-no-status",
                message: "Task is missing 'status' field".into(),
                line: None,
                fixable: false,
            });
        }
        // Parentless node check: severity depends on whether the node has children.
        // A node with children (scope > 0) but no parent is likely a structural gap.
        // A standalone leaf with no parent is valid under the information-theoretic model.
        if node_type == "task" || node_type == "epic" {
            let node_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !fm.contains_key("parent") {
                let node_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let has_children = children_set
                    .map(|cs| cs.contains(node_id))
                    .unwrap_or(false);
                let (severity, message) = if has_children {
                    (
                        Severity::Warning,
                        format!("Type '{}' has children but no parent — consider connecting to the graph", node_type),
                    )
                } else {
                    (
                        Severity::Style,
                        format!("Type '{}' has no parent (standalone leaf)", node_type),
                    )
                };
                diags.push(Diagnostic {
                    severity,
                    rule: "task-no-parent",
                    message,
                    line: None,
                    fixable: false,
                });
            }
        }
    }
}


fn check_markdown_body(content: &str, diags: &mut Vec<Diagnostic>) {
    let lines: Vec<&str> = content.lines().collect();

    // Skip frontmatter lines for line numbering
    let body_start = if content.starts_with("---") {
        // Find closing ---
        lines
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, l)| l.trim() == "---")
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    } else {
        0
    };

    let mut consecutive_blank = 0;
    let mut has_trailing_ws = false;

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        // Only check body (not frontmatter YAML)
        if i < body_start {
            continue;
        }

        // Trailing whitespace (not in code blocks)
        if line.ends_with(' ') || line.ends_with('\t') {
            // Allow exactly trailing double-space (markdown line break)
            let trimmed = line.trim_end();
            let trailing: String = line[trimmed.len()..].to_string();
            if trailing != "  " {
                if !has_trailing_ws {
                    diags.push(Diagnostic {
                        severity: Severity::Style,
                        rule: "md-trailing-ws",
                        message: "Trailing whitespace".into(),
                        line: Some(line_num),
                        fixable: true,
                    });
                }
                has_trailing_ws = true;
            }
        }

        // Consecutive blank lines
        if line.trim().is_empty() {
            consecutive_blank += 1;
            if consecutive_blank > 2 {
                diags.push(Diagnostic {
                    severity: Severity::Style,
                    rule: "md-consecutive-blanks",
                    message: "More than 2 consecutive blank lines".into(),
                    line: Some(line_num),
                    fixable: true,
                });
            }
        } else {
            consecutive_blank = 0;
        }
    }

    // Missing final newline
    if !content.is_empty() && !content.ends_with('\n') {
        diags.push(Diagnostic {
            severity: Severity::Style,
            rule: "md-no-final-newline",
            message: "File does not end with a newline".into(),
            line: Some(lines.len()),
            fixable: true,
        });
    }
}

// ── Auto-fix engine ──────────────────────────────────────────────────────

/// Remove the `body:` key and its (potentially multi-line) block-scalar value from a
/// frontmatter string (the text between the two `---` delimiters, without those delimiters).
fn remove_body_key_from_frontmatter(fm_text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    let mut is_block = false;

    for line in fm_text.lines() {
        if in_body {
            if line.starts_with(' ') || line.starts_with('\t') {
                // Indented continuation of block scalar — drop.
                continue;
            } else if line.is_empty() && is_block {
                // Blank line within a block scalar — drop.
                continue;
            } else {
                // Non-indented, non-empty line: the body block is over.
                in_body = false;
                is_block = false;
                lines.push(line);
            }
        } else if line.starts_with("body:") {
            in_body = true;
            let after = line["body:".len()..].trim();
            is_block = after.starts_with('|') || after.starts_with('>');
        } else {
            lines.push(line);
        }
    }

    lines.join("\n")
}

/// Apply fixes surgically — line-level edits only, preserving key order and formatting.
fn apply_fixes(content: &str, fm_data: &Option<serde_json::Value>, path: &Path, ancestor_map: Option<&AncestorMap>) -> String {
    let mut result = content.to_string();

    // ── Frontmatter fixes (only when we have a valid frontmatter object) ──
    if let Some(serde_json::Value::Object(fm)) = fm_data {
        // Fix 1: Migrate task_id → id (in-place line replacement)
        if fm.contains_key("task_id") && !fm.contains_key("id") {
            result = regex::Regex::new(r"(?m)^task_id:")
                .unwrap()
                .replace(&result, "id:")
                .to_string();
        }

        // Fix 2: Generate missing ID for all documents
        let has_id = fm.contains_key("id") || fm.contains_key("task_id");
        if !has_id {
            let id = generate_missing_id(path, fm);
            // Insert `id: xxx` right after the opening `---\n`
            if result.starts_with("---\n") {
                result = format!("---\nid: {}\n{}", id, &result[4..]);
            }
        }

        // Fix 3: Fix status aliases in-place
        if let Some(status) = fm.get("status").and_then(|v| v.as_str()) {
            let canonical = graph::resolve_status_alias(status);
            if canonical != status {
                let pattern = format!("status: {}", status);
                let replacement = format!("status: {}", canonical);
                result = result.replacen(&pattern, &replacement, 1);
            }
        }

        // Fix 4: Fix "p1"/"P2" style priorities → integer
        if let Some(s) = fm.get("priority").and_then(|v| v.as_str()) {
            let stripped = s.strip_prefix('p').or_else(|| s.strip_prefix('P'));
            if let Some(num_str) = stripped {
                if let Ok(n) = num_str.parse::<i64>() {
                    let pattern = format!("priority: {}", s);
                    let replacement = format!("priority: {}", n);
                    result = result.replacen(&pattern, &replacement, 1);
                }
            }
        }

        // Fix 5a: Fix unknown type → canonical type
        if let Some(t) = fm.get("type").and_then(|v| v.as_str()) {
            if !VALID_NODE_TYPES.contains(&t) {
                let mapped = resolve_type_alias(t);
                if mapped != t {
                    let pattern = format!("type: {}", t);
                    let replacement = format!("type: {}", mapped);
                    result = result.replacen(&pattern, &replacement, 1);
                }
            }
        }

        // Fix 5b: Fix unknown status → canonical (via alias or fallback to inbox)
        if let Some(raw_status) = fm.get("status").and_then(|v| v.as_str()) {
            let canonical = graph::resolve_status_alias(raw_status);
            if !graph::is_valid_status(canonical) {
                // Status is truly unknown even after alias resolution — default to inbox
                let pattern = format!("status: {}", raw_status);
                let replacement = "status: inbox".to_string();
                result = result.replacen(&pattern, &replacement, 1);
            }
        }

        // Note: fm-id-format fix is handled at directory level via rename_id
        // (requires cross-file reference updates)

        // Fix 5c: Remove deprecated 'project' field from frontmatter
        if let Some(project_val) = fm.get("project").and_then(|v| v.as_str()) {
            let has_parent = fm.contains_key("parent");
            let doc_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let can_fix = if !has_parent {
                true
            } else if let Some(amap) = ancestor_map {
                has_matching_ancestor(doc_id, project_val, amap)
            } else {
                false
            };
            if can_fix {
                let re = regex::Regex::new(r"(?m)^project:.*\n").unwrap();
                result = re.replace(&result, "").to_string();
            }
        }

        // Fix 6: Migrate 'body' frontmatter key → append its value to the markdown body
        if fm.contains_key("body") {
            if let Some(body_text) = fm.get("body").and_then(|v| v.as_str()) {
                let body_text = body_text.to_string();

                // Step 1: Remove the body: block from the frontmatter section.
                if result.starts_with("---\n") {
                    if let Some(fm_end_rel) = result[3..].find("\n---") {
                        let fm_end = fm_end_rel + 3;
                        let fm_section = result[4..fm_end].to_string();
                        let new_fm = remove_body_key_from_frontmatter(&fm_section);
                        let new_fm_str = if new_fm.trim().is_empty() {
                            String::new()
                        } else if new_fm.ends_with('\n') {
                            new_fm
                        } else {
                            format!("{}\n", new_fm)
                        };
                        result = format!("---\n{}---{}", new_fm_str, &result[fm_end + 4..]);
                    }
                }

                // Step 2: Append body_text to the markdown section if not already present.
                if let Some(fm_end_rel) = result[3..].find("\n---") {
                    let md_start = fm_end_rel + 3 + 5; // past \n---\n
                    if !result[md_start..].contains(body_text.trim()) {
                        if !result.ends_with('\n') {
                            result.push('\n');
                        }
                        if !result.ends_with("\n\n") {
                            result.push('\n');
                        }
                        result.push_str(body_text.trim());
                        result.push('\n');
                    }
                }
            }
        }
    }

    // ── Frontmatter structural fixes (need --- delimiters but not parsed data) ──

    // Fix 5: Remove blank line after opening ---
    if result.starts_with("---\n\n") {
        result = format!("---\n{}", &result[5..]);
    }

    // Fix 6: Convert `* item` to `- item` in frontmatter lists
    if result.starts_with("---\n") {
        if let Some(end) = result[3..].find("\n---") {
            let fm_end = end + 3;
            let fm_section = &result[4..fm_end];
            if fm_section.contains("\n* ") {
                let fixed_fm = fm_section.replace("\n* ", "\n- ");
                result = format!("---\n{}---{}", fixed_fm, &result[fm_end + 4..]);
            }
        }
    }

    // Fix 7: Quote frontmatter values that contain `: ` (breaks YAML parsers)
    if content.starts_with("---\n") {
        if let Some(end) = result[3..].find("\n---") {
            let fm_end = end + 3;
            let fm_section = result[4..fm_end].to_string();
            let mut new_fm = String::new();
            for line in fm_section.lines() {
                if let Some(first_colon) = line.find(": ") {
                    let key = &line[..first_colon];
                    let val = &line[first_colon + 2..];
                    // Skip lines that are already quoted, arrays, or continuation lines
                    let needs_quoting = !key.starts_with('-')
                        && !key.starts_with(' ')
                        && !val.starts_with('"')
                        && !val.starts_with('\'')
                        && !val.starts_with('[')
                        && val.contains(": ");
                    if needs_quoting {
                        let escaped = val.replace('"', "\\\"");
                        new_fm.push_str(&format!("{}: \"{}\"\n", key, escaped));
                    } else {
                        new_fm.push_str(line);
                        new_fm.push('\n');
                    }
                } else {
                    new_fm.push_str(line);
                    new_fm.push('\n');
                }
            }
            result = format!("---\n{}---{}", new_fm, &result[fm_end + 4..]);
        }
    }

    // ── Body fixes (always apply, regardless of frontmatter) ──

    // Fix 8: Remove trailing whitespace (preserve double-space line breaks)
    // Determine where the body starts
    let body_start = if result.starts_with("---\n") {
        result[3..].find("\n---").map(|end| end + 3 + 4) // past the \n---
    } else {
        Some(0) // no frontmatter — entire file is body
    };
    if let Some(bs) = body_start {
        let body = &result[bs..];
        let fixed_body: String = body
            .lines()
            .map(|line| {
                let trimmed = line.trim_end();
                let trailing = &line[trimmed.len()..];
                if trailing == "  " && !trimmed.is_empty() {
                    line // preserve intentional double-space line break
                } else {
                    trimmed
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        result = format!("{}{}", &result[..bs], fixed_body);
    }

    // Fix 9: Collapse more than 2 consecutive blank lines in body
    while result.contains("\n\n\n\n") {
        result = result.replace("\n\n\n\n", "\n\n\n");
    }

    // Fix 10: Ensure file ends with a newline
    if !result.ends_with('\n') {
        result.push('\n');
    }

    result
}


// ── Batch lint engine ────────────────────────────────────────────────────

/// Lint all markdown files under a PKB root directory.
pub fn lint_directory(
    pkb_root: &Path,
    fix: bool,
    check_refs: bool,
) -> (Vec<FileResult>, LintSummary) {
    let files = pkb::scan_directory(pkb_root);

    // Build known ID set for reference checking
    let known_ids: Option<HashSet<String>> = if check_refs {
        let ids: HashSet<String> = files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                parsed.data.as_ref().and_then(|d| {
                    let fm: serde_json::Value = d.deserialize().ok()?;
                    // Collect: id, filename stem, permalink, and all alias values
                    let mut ids = Vec::new();
                    if let Some(id) = fm.get("id").and_then(|v| v.as_str()) {
                        ids.push(id.to_string());
                    }
                    if let Some(stem) = p.file_stem() {
                        ids.push(stem.to_string_lossy().to_string());
                    }
                    if let Some(pl) = fm.get("permalink").and_then(|v| v.as_str()) {
                        ids.push(pl.to_string());
                    }
                    // Collect alias / aliases — other documents may reference by these names
                    for key in &["alias", "aliases"] {
                        if let Some(arr) = fm.get(*key).and_then(|v| v.as_array()) {
                            for item in arr {
                                if let Some(s) = item.as_str() {
                                    ids.push(s.to_string());
                                }
                            }
                        } else if let Some(s) = fm.get(*key).and_then(|v| v.as_str()) {
                            ids.push(s.to_string());
                        }
                    }
                    Some(ids)
                })
            })
            .flatten()
            .collect();
        Some(ids)
    } else {
        None
    };

    // Build ancestor map for deprecated-project autofix:
    // Maps each document ID → (parent_id, doc_type)
    let ancestor_map: AncestorMap = files
        .par_iter()
        .filter_map(|p| {
            let content = std::fs::read_to_string(p).ok()?;
            let matter = Matter::<YAML>::new();
            let parsed = matter.parse(&content);
            let fm = parsed.data.as_ref()
                .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
            let id = fm.get("id").and_then(|v| v.as_str())?.to_string();
            let parent = fm.get("parent").and_then(|v| v.as_str()).map(String::from);
            let doc_type = fm.get("type").and_then(|v| v.as_str()).map(String::from);
            Some((id, (parent, doc_type)))
        })
        .collect();

    // Derive children set: IDs that appear as a parent of at least one node.
    let children_set: ChildrenSet = ancestor_map
        .values()
        .filter_map(|(parent_id, _)| parent_id.clone())
        .collect();

    // ── Hard cycle detection ─────────────────────────────────────────────────
    // Scan all files for `parent` + `depends_on` references to build a directed
    // adjacency map, then run Tarjan's SCC to find hard dependency cycles.
    // Files that participate in a cycle receive an error-severity diagnostic.
    let cycle_diags: HashMap<PathBuf, Diagnostic> = {
        let raw: Vec<(String, Vec<String>, PathBuf)> = files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                let fm = parsed
                    .data
                    .as_ref()
                    .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
                let id = fm.get("id").and_then(|v| v.as_str())?.to_string();
                let mut deps: Vec<String> = Vec::new();
                if let Some(parent) = fm.get("parent").and_then(|v| v.as_str()) {
                    deps.push(parent.to_string());
                }
                if let Some(arr) = fm.get("depends_on").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            deps.push(s.to_string());
                        }
                    }
                }
                Some((id, deps, p.clone()))
            })
            .collect();

        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
        for (id, deps, path) in raw {
            id_to_path.insert(id.clone(), path);
            if !deps.is_empty() {
                adj.insert(id, deps);
            }
        }

        let cycles: Vec<Vec<String>> = crate::graph_store::tarjan_scc(&adj)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect();

        // Parent-only adjacency for the parent-cycle rule. Parent/child must
        // be a DAG; depends_on / blocks / soft_blocks may still be circular.
        let mut parent_adj: HashMap<String, Vec<String>> = HashMap::new();
        for (id, (parent, _doc_type)) in &ancestor_map {
            if let Some(p) = parent {
                parent_adj.insert(id.clone(), vec![p.clone()]);
            }
        }
        let parent_cycles: Vec<Vec<String>> = crate::graph_store::tarjan_scc(&parent_adj)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect();

        let mut diag_map: HashMap<PathBuf, Diagnostic> = HashMap::new();
        // Parent cycles take precedence over the combined dep-hard-cycle diagnostic
        // because parent/child is the constraint that's actually being violated.
        for cycle in &parent_cycles {
            let cycle_ids = cycle.join(", ");
            for node_id in cycle {
                if let Some(path) = id_to_path.get(node_id.as_str()) {
                    diag_map.entry(path.clone()).or_insert_with(|| Diagnostic {
                        severity: Severity::Error,
                        rule: "parent-cycle",
                        message: format!(
                            "Node '{}' is part of a parent/child cycle: [{}]. Parent/child \
                             hierarchy must be a DAG.",
                            node_id, cycle_ids
                        ),
                        line: None,
                        fixable: false,
                    });
                }
            }
        }
        for cycle in &cycles {
            let cycle_ids = cycle.join(", ");
            for node_id in cycle {
                if let Some(path) = id_to_path.get(node_id.as_str()) {
                    diag_map.entry(path.clone()).or_insert_with(|| Diagnostic {
                        severity: Severity::Error,
                        rule: "dep-hard-cycle",
                        message: format!(
                            "Node '{}' is part of a hard dependency cycle: [{}]",
                            node_id, cycle_ids
                        ),
                        line: None,
                        fixable: false,
                    });
                }
            }
        }
        diag_map
    };

    // Pre-fix pass: collect ID renames needed (old_id → new_id) before per-file fixes
    // Only IDs that genuinely don't match the prefix-hexhash pattern are renamed.
    // Prefix may contain uppercase letters (e.g. "academicOps-b5d43955").
    let id_renames: Vec<(String, String)> = if fix {
        let id_re = get_id_regex();
        files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                let fm = parsed.data.as_ref()
                    .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
                let id = fm.get("id")?.as_str()?;
                // Goals and projects use special canonical IDs — never auto-rename them.
                let node_type = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if !id.is_empty() && !id_re.is_match(id) && !matches!(node_type, "goal" | "project") {
                    let prefix = extract_id_prefix(id);
                    let new_id = crate::graph::create_id(&prefix);
                    Some((id.to_string(), new_id))
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut results: Vec<FileResult> = files
        .par_iter()
        .map(|p| lint_file(p, fix, known_ids.as_ref(), Some(&ancestor_map), Some(&children_set)))
        .collect();

    // Merge cycle diagnostics into per-file results
    for r in &mut results {
        if let Some(diag) = cycle_diags.get(&r.path) {
            r.diagnostics.push(diag.clone());
        }
    }

    let summary = LintSummary::from_results(&results);

    // Post-fix pass: apply cross-file ID renames via rename_id
    if fix && !id_renames.is_empty() {
        // First write per-file fixes (type, status, etc.)
        write_fixes(&results);

        // Then rename each non-conforming ID across all files
        for (old_id, new_id) in &id_renames {
            let _ = rename_id(pkb_root, old_id, new_id);
        }

        // Rebuild context maps — IDs have changed so the pre-rename maps are stale.
        // Without this, ref checks produce false positives and task-no-parent severity
        // is wrong for any node whose ID was just renamed.
        let known_ids: Option<HashSet<String>> = if check_refs {
            let ids: HashSet<String> = files
                .par_iter()
                .filter_map(|p| {
                    let content = std::fs::read_to_string(p).ok()?;
                    let matter = Matter::<YAML>::new();
                    let parsed = matter.parse(&content);
                    parsed.data.as_ref().and_then(|d| {
                        let fm: serde_json::Value = d.deserialize().ok()?;
                        let mut ids = Vec::new();
                        if let Some(id) = fm.get("id").and_then(|v| v.as_str()) {
                            ids.push(id.to_string());
                        }
                        if let Some(stem) = p.file_stem() {
                            ids.push(stem.to_string_lossy().to_string());
                        }
                        if let Some(pl) = fm.get("permalink").and_then(|v| v.as_str()) {
                            ids.push(pl.to_string());
                        }
                        for key in &["alias", "aliases"] {
                            if let Some(arr) = fm.get(*key).and_then(|v| v.as_array()) {
                                for item in arr {
                                    if let Some(s) = item.as_str() {
                                        ids.push(s.to_string());
                                    }
                                }
                            } else if let Some(s) = fm.get(*key).and_then(|v| v.as_str()) {
                                ids.push(s.to_string());
                            }
                        }
                        Some(ids)
                    })
                })
                .flatten()
                .collect();
            Some(ids)
        } else {
            None
        };
        let ancestor_map: AncestorMap = files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                let fm = parsed.data.as_ref()
                    .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
                let id = fm.get("id").and_then(|v| v.as_str())?.to_string();
                let parent = fm.get("parent").and_then(|v| v.as_str()).map(String::from);
                let doc_type = fm.get("type").and_then(|v| v.as_str()).map(String::from);
                Some((id, (parent, doc_type)))
            })
            .collect();
        let children_set: ChildrenSet = ancestor_map
            .values()
            .filter_map(|(parent_id, _)| parent_id.clone())
            .collect();

        // Return fresh results after renames (cycle diagnostics still apply)
        let mut results: Vec<FileResult> = files
            .par_iter()
            .map(|p| lint_file(p, false, known_ids.as_ref(), Some(&ancestor_map), Some(&children_set)))
            .collect();
        for r in &mut results {
            if let Some(diag) = cycle_diags.get(&r.path) {
                r.diagnostics.push(diag.clone());
            }
        }
        let summary = LintSummary::from_results(&results);
        return (results, summary);
    }

    (results, summary)
}

/// Rename an ID across the entire PKB — updates frontmatter reference fields
/// (parent, depends_on, soft_depends_on, blocks, soft_blocks, supersedes) and
/// wikilinks in all markdown files.
///
/// Returns (files_modified, references_updated).
/// Validate that an ID matches the expected format (alphanumeric + hyphens, no path traversal).
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('\n')
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

pub fn rename_id(pkb_root: &Path, old_id: &str, new_id: &str) -> Result<(usize, usize), String> {
    if !is_valid_id(old_id) {
        return Err(format!("Invalid old_id '{}': must be alphanumeric/hyphens only", old_id));
    }
    if !is_valid_id(new_id) {
        return Err(format!("Invalid new_id '{}': must be alphanumeric/hyphens only", new_id));
    }
    let files = pkb::scan_directory(pkb_root);
    let reference_fields = ["parent", "depends_on", "soft_depends_on", "blocks", "soft_blocks", "supersedes"];
    let mut files_modified = 0;
    let mut refs_updated = 0;

    for file_path in &files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut modified = false;
        let mut new_content = content.clone();

        // Update frontmatter references
        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(&content);
        if let Some(fm_data) = parsed.data.as_ref().and_then(|d| d.deserialize::<serde_json::Value>().ok()) {
            if let Some(fm) = fm_data.as_object() {
                for field in &reference_fields {
                    if let Some(val) = fm.get(*field) {
                        match val {
                            serde_json::Value::String(s) if s == old_id => {
                                // Single-value field (parent, supersedes)
                                let old_line = format!("{}: {}", field, old_id);
                                let new_line = format!("{}: {}", field, new_id);
                                if new_content.contains(&old_line) {
                                    new_content = new_content.replace(&old_line, &new_line);
                                    modified = true;
                                    refs_updated += 1;
                                }
                            }
                            serde_json::Value::Array(arr) => {
                                for item in arr {
                                    if item.as_str() == Some(old_id) {
                                        // Array item: "- old_id" → "- new_id"
                                        let old_item = format!("- {}", old_id);
                                        let new_item = format!("- {}", new_id);
                                        new_content = new_content.replacen(&old_item, &new_item, 1);
                                        modified = true;
                                        refs_updated += 1;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Also update the id field itself if this is the source file
                if fm.get("id").and_then(|v| v.as_str()) == Some(old_id) {
                    let old_line = format!("id: {}", old_id);
                    let new_line = format!("id: {}", new_id);
                    new_content = new_content.replacen(&old_line, &new_line, 1);
                    modified = true;
                    refs_updated += 1;
                }
            }
        }

        // Update wikilinks: [[old_id]] → [[new_id]], [[old_id|alias]] → [[new_id|alias]]
        let wiki_old = format!("[[{}]]", old_id);
        let wiki_new = format!("[[{}]]", new_id);
        if new_content.contains(&wiki_old) {
            new_content = new_content.replace(&wiki_old, &wiki_new);
            modified = true;
            refs_updated += 1;
        }
        let wiki_old_alias = format!("[[{}|", old_id);
        let wiki_new_alias = format!("[[{}|", new_id);
        if new_content.contains(&wiki_old_alias) {
            new_content = new_content.replace(&wiki_old_alias, &wiki_new_alias);
            modified = true;
            refs_updated += 1;
        }

        if modified {
            if std::fs::write(file_path, &new_content).is_ok() {
                files_modified += 1;
            }
        }
    }

    Ok((files_modified, refs_updated))
}

/// Write fixed files back to disk. Returns number of files written.
pub fn write_fixes(results: &[FileResult]) -> usize {
    let mut count = 0;
    for r in results {
        if let Some(ref fixed) = r.fixed_content {
            if std::fs::write(&r.path, fixed).is_ok() {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn lint_str(content: &str) -> Vec<Diagnostic> {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let result = lint_file(f.path(), false, None, None, None);
        result.diagnostics
    }

    fn fix_str(content: &str) -> String {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let result = lint_file(f.path(), true, None, None, None);
        result.fixed_content.unwrap_or_else(|| content.to_string())
    }

    #[test]
    fn valid_task_no_warnings() {
        let diags = lint_str(
            "---\nid: test-abc12345\ntitle: Test task\ntype: task\nstatus: active\npriority: 2\nparent: proj-00000000\ntags:\n- foo\n---\n\nBody content.\n",
        );
        // Should only have key-order style issues at most
        assert!(
            diags.iter().all(|d| d.severity <= Severity::Style),
            "Expected no errors/warnings, got: {:?}",
            diags
        );
    }

    #[test]
    fn detects_missing_frontmatter() {
        let diags = lint_str("# Just a heading\n\nSome text.\n");
        assert!(diags.iter().any(|d| d.rule == "fm-missing"));
    }

    #[test]
    fn detects_status_alias() {
        let diags = lint_str("---\ntitle: Test\nstatus: active\ntype: note\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "fm-status-alias"));
    }

    #[test]
    fn fixes_status_alias() {
        let fixed = fix_str("---\ntitle: Test\nstatus: active\ntype: note\n---\n\nBody.\n");
        assert!(fixed.contains("status: ready"), "Got: {}", fixed);
        assert!(!fixed.contains("status: active"));
    }

    #[test]
    fn detects_unknown_type() {
        let diags = lint_str("---\ntitle: Test\ntype: foobar\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "fm-unknown-type"));
    }

    #[test]
    fn detects_trailing_whitespace() {
        let diags = lint_str("---\ntitle: Test\ntype: note\n---\n\nBody text \n");
        assert!(diags.iter().any(|d| d.rule == "md-trailing-ws"));
    }

    #[test]
    fn detects_no_final_newline() {
        let diags = lint_str("---\ntitle: Test\ntype: note\n---\n\nBody text");
        assert!(diags.iter().any(|d| d.rule == "md-no-final-newline"));
    }

    #[test]
    fn fixes_task_id_to_id() {
        let fixed = fix_str("---\ntask_id: ns-abc12345\ntitle: Test\ntype: task\nstatus: done\n---\n\nBody.\n");
        assert!(fixed.contains("id: ns-abc12345"), "task_id should become id, got: {}", fixed);
        assert!(!fixed.contains("task_id:"), "task_id key should be removed");
    }

    #[test]
    fn detects_task_missing_id() {
        let diags = lint_str("---\ntitle: Test\ntype: task\nstatus: active\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "task-no-id"));
    }

    #[test]
    fn fixes_unknown_type() {
        let fixed = fix_str("---\ntitle: Test\ntype: article\nstatus: active\n---\n\nBody.\n");
        assert!(fixed.contains("type: reference"), "article should become reference, got: {}", fixed);
    }

    #[test]
    fn fixes_unknown_type_to_document() {
        let fixed = fix_str("---\ntitle: Test\ntype: bundle\nstatus: active\n---\n\nBody.\n");
        assert!(fixed.contains("type: document"), "bundle should become document, got: {}", fixed);
    }

    #[test]
    fn fixes_unknown_status_to_active() {
        let fixed = fix_str("---\ntitle: Test\ntype: note\nstatus: merge_ready\n---\n\nBody.\n");
        // merge_ready is now a canonical status, so it should stay as-is
        assert!(fixed.contains("status: merge_ready"), "merge_ready should stay merge_ready (canonical status), got: {}", fixed);
    }

    #[test]
    fn fixes_truly_unknown_status() {
        let fixed = fix_str("---\ntitle: Test\ntype: note\nstatus: banana\n---\n\nBody.\n");
        assert!(fixed.contains("status: inbox"), "unknown status should become inbox, got: {}", fixed);
    }

    #[test]
    fn id_format_flagged_as_fixable() {
        let diags = lint_str("---\nid: osb\ntitle: Test\ntype: note\n---\n\nBody.\n");
        let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
        assert!(id_diag.is_some(), "Should detect bad ID format");
        assert!(id_diag.unwrap().fixable, "fm-id-format should be fixable");
    }

    #[test]
    fn camel_case_prefix_id_is_valid() {
        // "academicOps-b5d43955" has a camelCase prefix — must NOT trigger fm-id-format.
        // Reassigning a valid existing ID would silently break all cross-references.
        let diags = lint_str("---\nid: academicOps-b5d43955\ntitle: Test\ntype: task\n---\n\nBody.\n");
        let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
        assert!(id_diag.is_none(), "academicOps-b5d43955 is a valid ID and must not trigger fm-id-format");
    }

    #[test]
    fn id_starting_with_digit_is_valid() {
        let diags = lint_str("---\nid: 123abc-b5d43955\ntitle: Test\ntype: task\n---\n\nBody.\n");
        let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
        assert!(id_diag.is_none(), "IDs starting with a digit must not trigger fm-id-format");
    }

    #[test]
    fn goal_and_project_ids_exempt_from_format_check() {
        // Goals and projects use canonical human-readable IDs — must NOT trigger fm-id-format.
        for node_type in &["goal", "project"] {
            let content = format!("---\nid: my-{}\ntitle: Test\ntype: {}\n---\n\nBody.\n", node_type, node_type);
            let diags = lint_str(&content);
            let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
            assert!(id_diag.is_none(), "type:{} with non-hex ID must not trigger fm-id-format", node_type);
        }
    }

    #[test]
    fn alias_key_is_known() {
        let diags = lint_str("---\ntitle: Test\ntype: note\nalias: foo\n---\n\nBody.\n");
        assert!(!diags.iter().any(|d| d.rule == "fm-unknown-key"), "alias should be a known key");
    }

    #[test]
    fn triage_keys_are_known() {
        let diags = lint_str("---\ntitle: Test\ntype: note\nprocessed: true\nprocessed_date: 2026-01-01\ntriage_action: create-task\ntriage_ref: test-12345678\n---\n\nBody.\n");
        assert!(!diags.iter().any(|d| d.rule == "fm-unknown-key"),
            "triage keys should be known, got: {:?}",
            diags.iter().filter(|d| d.rule == "fm-unknown-key").map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn extract_id_prefix_simple() {
        assert_eq!(extract_id_prefix("osb"), "osb");
        assert_eq!(extract_id_prefix("explorations-np-003"), "explorations-np");
        assert_eq!(extract_id_prefix("ip-australia"), "ip"); // "australia" is alpha → takes first+second, but len check splits
    }

    #[test]
    fn colon_in_value_fallback_parse() {
        // Values with colons (e.g. `title: Foo: Bar`) fail in serde_yaml
        // but should still be handled by our fallback parser
        let diags = lint_str("---\nid: test-a1b2c3d4\ntitle: Dashboard: UP NEXT\ntype: task\nstatus: active\n---\n\nBody.\n");
        // Should NOT get fm-invalid or fm-parse-error — fallback handles it
        assert!(
            !diags.iter().any(|d| d.rule == "fm-invalid" || d.rule == "fm-parse-error"),
            "Should not get fm-invalid with fallback parser, got: {:?}",
            diags.iter().map(|d| d.rule).collect::<Vec<_>>()
        );
    }

    #[test]
    fn detects_body_in_frontmatter() {
        let diags = lint_str("---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: some content\n---\n\nExisting body.\n");
        assert!(
            diags.iter().any(|d| d.rule == "fm-prohibited-body"),
            "Should detect body as frontmatter key, got: {:?}",
            diags.iter().map(|d| d.rule).collect::<Vec<_>>()
        );
        let diag = diags.iter().find(|d| d.rule == "fm-prohibited-body").unwrap();
        assert_eq!(diag.severity, Severity::Error);
        assert!(diag.fixable);
    }

    #[test]
    fn fixes_body_in_frontmatter_simple() {
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: migrated content\n---\n\nExisting body.\n";
        let fixed = fix_str(input);
        assert!(!fixed.contains("body: migrated content"), "body key should be removed from frontmatter, got:\n{}", fixed);
        assert!(fixed.contains("migrated content"), "body value should appear in markdown body, got:\n{}", fixed);
        // Existing body content should be preserved
        assert!(fixed.contains("Existing body."), "existing body should be preserved, got:\n{}", fixed);
    }

    #[test]
    fn fixes_body_in_frontmatter_block_scalar() {
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: |-\n  # Section\n\n  Some detailed content.\n\n  More content here.\ncomplexity: multi-step\n---\n\nShort existing body.\n";
        let fixed = fix_str(input);
        // body: key removed
        assert!(!fixed.contains("body: |-"), "body: block key should be removed, got:\n{}", fixed);
        // content preserved
        assert!(fixed.contains("# Section"), "body content should be in markdown, got:\n{}", fixed);
        assert!(fixed.contains("More content here."), "all body content preserved, got:\n{}", fixed);
        // other frontmatter preserved
        assert!(fixed.contains("complexity: multi-step"), "other frontmatter keys preserved, got:\n{}", fixed);
        // existing markdown body preserved
        assert!(fixed.contains("Short existing body."), "existing body preserved, got:\n{}", fixed);
    }

    #[test]
    fn fixes_body_not_duplicated_when_already_present() {
        // If the markdown body already contains the full body value, don't append
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: exact content\n---\n\nexact content\n";
        let fixed = fix_str(input);
        assert!(!fixed.contains("body: exact content"), "body key removed");
        // Should not double the content
        let count = fixed.matches("exact content").count();
        assert_eq!(count, 1, "content should appear exactly once, got:\n{}", fixed);
    }

    #[test]
    fn body_key_is_prohibited_not_merely_unknown() {
        // body: should be flagged as prohibited (Error) rather than merely unknown (Style)
        let diags = lint_str("---\nid: test-a1b2c3d4\ntitle: Test\ntype: note\nbody: foo\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "fm-prohibited-body"), "should get fm-prohibited-body");
        // Should NOT get fm-unknown-key for body — it's a known key with its own rule
        assert!(!diags.iter().any(|d| d.rule == "fm-unknown-key" && d.message.contains("'body'")),
            "should not get fm-unknown-key for body");
    }

    #[test]
    fn detects_parent_cycle_in_directory() {
        // Two-node parent cycle: a's parent is b, b's parent is a.
        // The directory-level cycle pass should flag both files with `parent-cycle`.
        let dir = tempfile::tempdir().unwrap();
        let a_path = dir.path().join("task-a.md");
        let b_path = dir.path().join("task-b.md");
        std::fs::write(
            &a_path,
            "---\nid: task-aaaaaaaa\ntitle: A\ntype: task\nstatus: ready\nparent: task-bbbbbbbb\n---\n\nbody\n",
        )
        .unwrap();
        std::fs::write(
            &b_path,
            "---\nid: task-bbbbbbbb\ntitle: B\ntype: task\nstatus: ready\nparent: task-aaaaaaaa\n---\n\nbody\n",
        )
        .unwrap();

        let (results, _summary) = lint_directory(dir.path(), false, true);
        let all_diags: Vec<&Diagnostic> = results
            .iter()
            .flat_map(|r| r.diagnostics.iter())
            .collect();
        assert!(
            all_diags.iter().any(|d| d.rule == "parent-cycle"),
            "expected parent-cycle diagnostic, got: {:?}",
            all_diags
        );
    }

    #[test]
    fn no_parent_cycle_for_dag() {
        // Linear chain: leaf -> mid -> root. No cycle expected.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("root.md"),
            "---\nid: epic-aaaaaaaa\ntitle: Root\ntype: epic\nstatus: ready\n---\n\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mid.md"),
            "---\nid: task-bbbbbbbb\ntitle: Mid\ntype: task\nstatus: ready\nparent: epic-aaaaaaaa\n---\n\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("leaf.md"),
            "---\nid: task-cccccccc\ntitle: Leaf\ntype: task\nstatus: ready\nparent: task-bbbbbbbb\n---\n\nbody\n",
        )
        .unwrap();

        let (results, _summary) = lint_directory(dir.path(), false, true);
        let parent_cycle_diags: Vec<&Diagnostic> = results
            .iter()
            .flat_map(|r| r.diagnostics.iter())
            .filter(|d| d.rule == "parent-cycle")
            .collect();
        assert!(
            parent_cycle_diags.is_empty(),
            "did not expect parent-cycle diagnostics for a DAG, got: {:?}",
            parent_cycle_diags
        );
    }
}
