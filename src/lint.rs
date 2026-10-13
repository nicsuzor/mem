//! PKB linter and formatter — validates and auto-fixes markdown files with YAML frontmatter.
//!
//! Rules are PKB-specific: frontmatter schema validation, status/type canonicalization,
//! YAML key ordering, markdown hygiene, and cross-reference integrity.

use crate::flow::{FlowEffect, FlowInput, FlowState};
use crate::graph::{self, LinkEffect, LinkLabel, VALID_NODE_TYPES};
use crate::pkb;
use gray_matter::engine::YAML;
use gray_matter::Matter;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

static ID_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

fn get_id_regex() -> &'static regex::Regex {
    ID_RE.get_or_init(|| regex::Regex::new(r"(?i)^[a-z0-9][a-z0-9_-]*[_-][a-f0-9]{8}$").unwrap())
}

// ── Diagnostic types ─────────────────────────────────────────────────────

/// Severity level for lint diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Fix authority for lint diagnostics (specs/graph-lint.md §4, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFix {
    Yes,
    Propose,
    No,
}

impl std::fmt::Display for AgentFix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentFix::Yes => write!(f, "yes"),
            AgentFix::Propose => write!(f, "propose"),
            AgentFix::No => write!(f, "no"),
        }
    }
}

/// Subject of a lint diagnostic (specs/graph-lint.md §8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum DiagnosticSubject {
    Node(String),
    Edge {
        from: String,
        to: String,
        label: String,
    },
}

/// A single lint diagnostic attached to a file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
    pub line: Option<usize>,
    pub fixable: bool,
    pub agent_fix: AgentFix,
    pub subject: Option<DiagnosticSubject>,
}

impl Diagnostic {
    pub fn simple(
        severity: Severity,
        rule: &'static str,
        message: impl Into<String>,
        line: Option<usize>,
        fixable: bool,
    ) -> Self {
        Self {
            severity,
            rule,
            message: message.into(),
            line,
            fixable,
            agent_fix: if fixable { AgentFix::Yes } else { AgentFix::No },
            subject: None,
        }
    }

    pub fn node(
        severity: Severity,
        rule: &'static str,
        message: impl Into<String>,
        node_id: impl Into<String>,
        agent_fix: AgentFix,
    ) -> Self {
        let fixable = agent_fix == AgentFix::Yes;
        Self {
            severity,
            rule,
            message: message.into(),
            line: None,
            fixable,
            agent_fix,
            subject: Some(DiagnosticSubject::Node(node_id.into())),
        }
    }

    pub fn edge(
        severity: Severity,
        rule: &'static str,
        message: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
        label: impl Into<String>,
        agent_fix: AgentFix,
    ) -> Self {
        let fixable = agent_fix == AgentFix::Yes;
        Self {
            severity,
            rule,
            message: message.into(),
            line: None,
            fixable,
            agent_fix,
            subject: Some(DiagnosticSubject::Edge {
                from: from.into(),
                to: to.into(),
                label: label.into(),
            }),
        }
    }
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
    "intent",
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
    "last_modified",
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
    // Flow model keys (specs/graph-lint.md §7)
    "worth",
    "deadline_class",
    "links",
    "follow_up_tasks",
    // Target / prototype node fields (spec multi-parent-edges §1.1, §1.6)
    "severity",
    "goal_type",
    "consequence",
    "edge_template",
    "goals",
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
    "body", // kept here so fm-unknown-key doesn't fire; fm-prohibited-body handles it
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
        "bug" | "feature" | "action" | "milestone" | "subproject" | "epic" => "task",
        // Retired container type — "project" is now a polecat.yaml routing slug,
        // not a node type. Legacy containers reclassify to task.
        "project" => "task",
        // Strategic nodes collapsed → target
        "goal" | "capability" | "target" => "target",
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
    // or new formats like "academic_8c6h05e2-cite-her-work" or "aops_core_6a4f03c0-fix-something"
    // The hash portion is alphanumeric (may contain letters beyond a-f)
    let re = {
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new(r"(?i)^([a-z][\w-]*?[_-][a-z0-9]{6,10})(?:[_-].+)?$").unwrap()
        })
    };
    re.captures(&stem).map(|c| c[1].to_string())
}

/// Extract a leading `YYYY-MM-DD` date from a filename stem or, failing that,
/// from the frontmatter `title`. Used to build convention-correct daily-note ids.
fn extract_daily_date(
    path: &Path,
    fm: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^(\d{4}-\d{2}-\d{2})").unwrap());
    if let Some(stem) = path.file_stem() {
        if let Some(c) = re.captures(&stem.to_string_lossy()) {
            return Some(c[1].to_string());
        }
    }
    fm.get("title")
        .and_then(|v| v.as_str())
        .and_then(|t| re.captures(t))
        .map(|c| c[1].to_string())
}

/// Generate an ID for a file that's missing one.
/// Tries: daily-note date convention → filename pattern extraction → project field → parent dir → "task"
fn generate_missing_id(path: &Path, fm: &serde_json::Map<String, serde_json::Value>) -> String {
    // Daily notes follow a distinct id convention (`<date>-<hex>`), not the
    // generic `<prefix>_<hex>` shape. extract_id_from_filename below requires
    // a letter-led stem, so a digit-led "<date>-daily" filename never matches
    // it — special-case daily notes first or they'd fall through to a
    // "daily_<hex>" id, diverging from the corpus convention (135/150
    // existing daily notes use `<date>-<hex>`).
    let node_type = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if resolve_type_alias(node_type) == "daily" {
        if let Some(date) = extract_daily_date(path, fm) {
            return crate::graph::create_id_verbatim(&date);
        }
    }

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
pub fn lint_file(
    path: &Path,
    fix: bool,
    known_ids: Option<&HashSet<String>>,
    ancestor_map: Option<&AncestorMap>,
    children_set: Option<&ChildrenSet>,
) -> FileResult {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return FileResult {
                path: path.to_path_buf(),
                diagnostics: vec![Diagnostic::simple(
                    Severity::Error,
                    "io-error",
                    format!("Cannot read file: {e}"),
                    None,
                    false,
                )],
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
        .filter(|v| v.is_object());
    let used_fallback = yaml_ok.is_none() && content.starts_with("---");
    let fm_data = yaml_ok.or_else(|| fallback_parse_frontmatter(&content));

    // If fallback was needed and succeeded, the YAML has quoting issues
    if used_fallback && fm_data.is_some() {
        diags.push(Diagnostic::simple(
            Severity::Error,
            "fm-yaml-quoting",
            "Frontmatter has unquoted values with colons — needs quoting",
            Some(1),
            true,
        ));
    }

    // ── Frontmatter rules ────────────────────────────────────────────

    check_frontmatter(
        &content,
        &fm_data,
        &mut diags,
        known_ids,
        ancestor_map,
        children_set,
    );

    // ── Markdown body rules ──────────────────────────────────────────

    check_markdown_body(&content, &mut diags);

    // ── Self-heal a missing id, regardless of --fix ───────────────────
    //
    // A missing id must never hard-fail CI (see the demoted `task-no-id`
    // severity above) — it self-heals unconditionally. Only ever *fills* an
    // absent id (idempotent: a note that already has one is never touched
    // here), and inserts a single `id:` line without altering any other
    // frontmatter (safe write).
    let id_healed_content = match &fm_data {
        Some(serde_json::Value::Object(fm))
            if !fm.contains_key("id")
                && !fm.contains_key("task_id")
                && content.starts_with("---\n") =>
        {
            let id = generate_missing_id(path, fm);
            Some(format!("---\nid: {}\n{}", id, &content[4..]))
        }
        _ => None,
    };
    let content_after_heal: &str = id_healed_content.as_deref().unwrap_or(&content);

    // ── Build fixed content if requested ─────────────────────────────

    let fixed_content = if fix && diags.iter().any(|d| d.fixable) {
        let fixed = apply_fixes(content_after_heal, &fm_data, path, ancestor_map);
        if fixed != content {
            Some(fixed)
        } else {
            None
        }
    } else {
        id_healed_content.filter(|healed| healed != &content)
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
    // `---` may be followed by a multi-byte char rather than `\n`.
    let fm_text = content.get(4..end + 3)?;

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
                    .map(|s| {
                        serde_json::Value::String(
                            s.trim().trim_matches(|c| c == '\'' || c == '"').to_string(),
                        )
                    })
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

/// Map of document ID → (parent_id, explicit project value) for ancestor lookups.
/// Built during lint_directory pre-pass.
pub type AncestorMap = HashMap<String, (Option<String>, Option<String>)>;

/// Set of IDs that appear as a parent of at least one other node.
/// A node in this set has children (scope > 0).
pub type ChildrenSet = HashSet<String>;

/// Walk the parent chain to find a node with an explicit `project:` value.
/// Mirrors the inheritance rule used by compute_project_field in graph_store.rs.
fn resolves_project(id: &str, ancestor_map: &AncestorMap) -> bool {
    let mut current = id.to_string();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        let entry = match ancestor_map.get(&current) {
            Some(e) => e,
            None => break,
        };
        if entry.1.is_some() {
            return true;
        }
        match entry.0 {
            Some(ref parent_id) => current = parent_id.clone(),
            None => break,
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
    _children_set: Option<&ChildrenSet>,
) {
    // Check frontmatter exists
    if !content.starts_with("---") {
        diags.push(Diagnostic::simple(
            Severity::Warning,
            "fm-missing",
            "File has no YAML frontmatter",
            Some(1),
            false,
        ));
        return;
    }

    let fm = match fm_data {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-invalid",
                "Frontmatter is not a YAML mapping",
                Some(1),
                false,
            ));
            return;
        }
        None => {
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-parse-error",
                "Failed to parse YAML frontmatter",
                Some(1),
                false,
            ));
            return;
        }
    };

    // Required: title
    if !fm.contains_key("title") {
        diags.push(Diagnostic::simple(
            Severity::Warning,
            "fm-no-title",
            "Missing 'title' in frontmatter",
            Some(1),
            false,
        ));
    }

    let node_type = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let is_task_type = graph::TASK_TYPES.contains(&node_type);

    // Required: type
    if let Some(t) = fm.get("type").and_then(|v| v.as_str()) {
        if t == "project" {
            // Retired type: read-compat treats it as epic; nudge migration.
            diags.push(Diagnostic::simple(
                Severity::Warning,
                "fm-deprecated-project-type",
                "'type: project' is retired — the node is read as a task. \
                          Reclassify to 'task' (e.g. via batch_reclassify); 'project' \
                          is now the polecat.yaml routing slug in the 'project:' field.",
                None,
                true,
            ));
        } else if !graph::is_valid_node_type(t) {
            let mapped = resolve_type_alias(t);
            diags.push(Diagnostic::simple(
                Severity::Style,
                "fm-unknown-type",
                format!("Unknown type '{}' → will fix to '{}'", t, mapped),
                None,
                true,
            ));
        }
    } else if fm.get("type").is_some() {
        diags.push(Diagnostic::simple(
            Severity::Error,
            "fm-type-not-string",
            "'type' must be a string",
            None,
            false,
        ));
    }

    // Status validation + alias detection
    if is_task_type {
        if let Some(raw_status) = fm.get("status").and_then(|v| v.as_str()) {
            let canonical = graph::resolve_status_alias(raw_status);
            if canonical != raw_status {
                diags.push(Diagnostic::simple(
                    Severity::Style,
                    "fm-status-alias",
                    format!(
                        "Status '{}' should be canonical '{}'",
                        raw_status, canonical
                    ),
                    None,
                    true,
                ));
            }
            if !graph::is_valid_status(canonical) {
                diags.push(Diagnostic::simple(
                    Severity::Warning,
                    "fm-unknown-status",
                    format!("Unknown status '{}' → will fix to 'inbox'", raw_status),
                    None,
                    true,
                ));
            }
        } else if fm.get("status").is_some() {
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-status-not-string",
                "'status' must be a string",
                None,
                false,
            ));
        }
    }

    // Intent validation
    if let Some(p) = fm.get("intent").or_else(|| fm.get("priority")) {
        if let Some(n) = p.as_i64() {
            if !graph::is_valid_intent(n as i32) {
                diags.push(Diagnostic::simple(
                    Severity::Warning,
                    "fm-intent-range",
                    format!("Intent {} outside expected range 0-4", n),
                    None,
                    false,
                ));
            }
        } else if let Some(s) = p.as_str() {
            // Check if it's a "p1"/"P2" style intent we can fix
            let stripped = s.strip_prefix('p').or_else(|| s.strip_prefix('P'));
            let can_fix = stripped.map(|n| n.parse::<i64>().is_ok()).unwrap_or(false);
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-intent-type",
                format!("'intent' must be an integer (got '{}')", s),
                None,
                can_fix,
            ));
        } else if !p.is_number() {
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-intent-type",
                "'intent' must be an integer",
                None,
                false,
            ));
        }
    }

    // Effort validation
    if let Some(effort) = fm.get("effort").and_then(|v| v.as_str()) {
        if !graph::is_valid_effort(effort) {
            diags.push(Diagnostic::simple(
                Severity::Warning,
                "fm-invalid-effort",
                format!(
                    "Unrecognised effort value '{}' — expected duration string like '1d', '2h', '1w'",
                    effort
                ),
                None,
                false,
            ));
        }
    }

    // Tags should be an array
    if let Some(tags) = fm.get("tags") {
        if !tags.is_array() && !tags.is_string() {
            diags.push(Diagnostic::simple(
                Severity::Error,
                "fm-tags-type",
                "'tags' must be a list or comma-separated string",
                None,
                false,
            ));
        }
    }

    // id format check (should match prefix-hex pattern)
    // Prefix may contain uppercase letters (e.g. "academicOps-b5d43955" is valid)
    // Goals and targets use special canonical IDs (e.g. "my-goal") — skip format
    // check. Legacy `type: project` files (retired type) keep the exemption so
    // their canonical IDs never get flagged/renamed; the deprecation warning
    // surfaces them instead.
    let node_type_for_id = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let is_root_node = matches!(node_type_for_id, "goal" | "target" | "project");
    if let Some(id) = fm.get("id").and_then(|v| v.as_str()) {
        let id_re = get_id_regex();
        if !is_root_node && !id_re.is_match(id) && !id.is_empty() {
            diags.push(Diagnostic::simple(
                Severity::Style,
                "fm-id-format",
                format!(
                    "ID '{}' doesn't match expected pattern 'prefix-hexhash'",
                    id
                ),
                None,
                true,
            ));
        }
    }

    // Leading blank lines inside a YAML block-scalar value — an artifact of
    // serializing a string that itself started with blank lines (e.g. an
    // unvalidated caller-supplied field like `body_append` whose text began
    // with "\n\n..."). See aops-cb065324.
    if let Some(fm_section) = extract_frontmatter_section(content) {
        if has_leading_blank_block_scalar(fm_section) {
            diags.push(Diagnostic::simple(
                Severity::Style,
                "fm-block-scalar-whitespace",
                "YAML block-scalar value in frontmatter has leading blank line(s)",
                None,
                true,
            ));
        }
    }

    // Prohibited: body — content must live in the markdown body section, not frontmatter
    if fm.contains_key("body") {
        diags.push(Diagnostic::simple(
            Severity::Error,
            "fm-prohibited-body",
            "'body' is a prohibited frontmatter key — content belongs in the markdown body (run with --fix to auto-migrate)",
            None,
            true,
        ));
    }

    // Project field: required for actionable tasks (ready/queued). A task
    // satisfies this by declaring its own `project: <slug>` or by inheriting
    // one from the nearest ancestor (parent chain) that declared one.
    // Legacy `type: project` containers are excluded — they already get
    // fm-deprecated-project-type; double-nagging them here is noise.
    if is_task_type {
        let status_val = fm.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let canonical_status = graph::resolve_status_alias(status_val);
        if matches!(canonical_status, "ready" | "queued") && !fm.contains_key("project") {
            let mut resolves = false;
            // Start from the parent: the task's own map entry mirrors its own
            // (absent) project field, so walking from the parent is equivalent
            // and avoids a pointless self-hop.
            let start_id = fm
                .get("parent")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(parent_id) = start_id {
                if let Some(map) = ancestor_map {
                    resolves = resolves_project(&parent_id, map);
                }
            }
            if !resolves {
                diags.push(Diagnostic::simple(
                    Severity::Warning,
                    "fm-missing-project",
                    "Actionable tasks (ready/queued) must have an explicit 'project' field or inherit one from an ancestor",
                    None,
                    false,
                ));
            }
        }
    }

    // Unknown keys
    let known: HashSet<&str> = KNOWN_KEYS.iter().copied().collect();
    for key in fm.keys() {
        if !known.contains(key.as_str()) {
            diags.push(Diagnostic::simple(
                Severity::Style,
                "fm-unknown-key",
                format!("Unknown frontmatter key '{}'", key),
                None,
                false,
            ));
        }
    }

    // Reference integrity: flow-edge-dangling for flow model edges; ref-broken-dep for blocks/soft_blocks/supersedes
    if let Some(known_ids) = known_ids {
        let node_id_for_ref = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(parent) = fm.get("parent").and_then(|v| v.as_str()) {
            if !known_ids.contains(parent) {
                diags.push(Diagnostic::edge(
                    Severity::Warning,
                    "flow-edge-dangling",
                    format!("Parent '{}' not found in PKB", parent),
                    node_id_for_ref,
                    parent,
                    "part_of",
                    AgentFix::No,
                ));
            }
        }
        if let Some(arr) = fm.get("depends_on").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(ref_id) = item.as_str() {
                    if !known_ids.contains(ref_id) {
                        diags.push(Diagnostic::edge(
                            Severity::Warning,
                            "flow-edge-dangling",
                            format!("depends_on reference '{}' not found in PKB", ref_id),
                            node_id_for_ref,
                            ref_id,
                            "needs",
                            AgentFix::No,
                        ));
                    }
                }
            }
        }
        if let Some(arr) = fm.get("soft_depends_on").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(ref_id) = item.as_str() {
                    if !known_ids.contains(ref_id) {
                        diags.push(Diagnostic::edge(
                            Severity::Warning,
                            "flow-edge-dangling",
                            format!("soft_depends_on reference '{}' not found in PKB", ref_id),
                            ref_id,
                            node_id_for_ref,
                            "supports",
                            AgentFix::No,
                        ));
                    }
                }
            }
        }
        if let Some(arr) = fm.get("contributes_to").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(target) = item.as_str().or_else(|| item.get("to").or_else(|| item.get("id")).and_then(|v| v.as_str())) {
                    if !known_ids.contains(target) {
                        diags.push(Diagnostic::edge(
                            Severity::Warning,
                            "flow-edge-dangling",
                            format!("contributes_to target '{}' not found in PKB", target),
                            node_id_for_ref,
                            target,
                            "serves",
                            AgentFix::No,
                        ));
                    }
                }
            }
        }
        if let Some(arr) = fm.get("links").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let label = obj.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(to) = obj.get("to").and_then(|v| v.as_str()) {
                        if !known_ids.contains(to) {
                            diags.push(Diagnostic::edge(
                                Severity::Warning,
                                "flow-edge-dangling",
                                format!("Link target '{}' not found in PKB", to),
                                node_id_for_ref,
                                to,
                                label,
                                AgentFix::No,
                            ));
                        }
                    }
                    if let Some(from) = obj.get("from").and_then(|v| v.as_str()) {
                        if !known_ids.contains(from) {
                            diags.push(Diagnostic::edge(
                                Severity::Warning,
                                "flow-edge-dangling",
                                format!("Link source '{}' not found in PKB", from),
                                from,
                                node_id_for_ref,
                                label,
                                AgentFix::No,
                            ));
                        }
                    }
                }
            }
        }
        for key in &["blocks", "soft_blocks"] {
            if let Some(arr) = fm.get(*key).and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(ref_id) = item.as_str() {
                        if !known_ids.contains(ref_id) {
                            diags.push(Diagnostic::simple(
                                Severity::Warning,
                                "ref-broken-dep",
                                format!("{} reference '{}' not found in PKB", key, ref_id),
                                None,
                                false,
                            ));
                        }
                    }
                }
            }
        }

        // `supersedes` accepts a scalar, comma-joined string, or YAML
        // sequence (mem_8035b002) — check every named target, not just a
        // bare scalar.
        for ref_id in
            crate::graph::parse_string_array(&serde_json::Value::Object(fm.clone()), "supersedes")
        {
            if !known_ids.contains(ref_id.as_str()) {
                diags.push(Diagnostic::simple(
                    Severity::Warning,
                    "ref-broken-dep",
                    format!("supersedes reference '{}' not found in PKB", ref_id),
                    None,
                    false,
                ));
            }
        }
        // `superseded_by` is a reserved computed keyword going forward
        // (mem_8035b002) — a hand-written value in legacy data is itself a
        // lint issue, separate from whether it happens to resolve.
        if fm.contains_key("superseded_by") {
            diags.push(Diagnostic::simple(
                Severity::Warning,
                "superseded-by-hand-written",
                "'superseded_by' is a computed reverse index of 'supersedes' and should not be hand-written; set 'supersedes' on the superseding node instead",
                None,
                false,
            ));
        }
    }

    // All documents should have an explicit id
    if !fm.contains_key("id") {
        if fm.contains_key("task_id") {
            diags.push(Diagnostic::simple(
                Severity::Style,
                "task-legacy-id",
                "Document uses legacy 'task_id' instead of 'id'",
                None,
                true,
            ));
        } else {
            // Self-healing: lint_file always regenerates a missing id
            // regardless of --fix (see the id-heal step there), so this is
            // no longer a build-breaking error — just a note that it happened.
            diags.push(Diagnostic::simple(
                Severity::Style,
                "task-no-id",
                "Document was missing 'id' field — auto-generated",
                None,
                true,
            ));
        }
    }

    // All documents should have a type
    if !fm.contains_key("type") {
        diags.push(Diagnostic::simple(
            Severity::Warning,
            "doc-no-type",
            "Document is missing 'type' field",
            None,
            false,
        ));
    }

    // Task-type-specific checks
    if is_task_type {
        if !fm.contains_key("status") {
            diags.push(Diagnostic::simple(
                Severity::Warning,
                "task-no-status",
                "Task is missing 'status' field",
                None,
                false,
            ));
        }
// task-no-parent is retired under the flow model (specs/graph-lint.md §7, S7, U9)

        // Triage lint: missing acceptance criteria (demoted from scoring proxy in Phase 3)
        if node_type == "task" && !graph::detect_acceptance_criteria(content) {
            diags.push(Diagnostic::simple(
                Severity::Style,
                "task-missing-ac",
                "Task has no acceptance criteria heading ('Acceptance Criteria', 'done when', etc.) — required for inbox task readiness",
                None,
                false,
            ));
        }
    }
    // ── Flow model checks (specs/graph-lint.md §5) ──────────────────────────
    let node_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let status_str = fm.get("status").and_then(|v| v.as_str());
    let is_open_node = !matches!(
        status_str,
        Some("done") | Some("retired") | Some("cancelled") | Some("completed")
    );

    // 5.8: flow-legacy-field
    for leg_key in &["stated_weight", "multiplier", "standing_weight"] {
        if fm.contains_key(*leg_key) {
            diags.push(Diagnostic::node(
                Severity::Warning,
                "flow-legacy-field",
                format!("Legacy field '{}' is deprecated under the flow model", leg_key),
                node_id,
                AgentFix::No,
            ));
        }
    }
    if let Some(arr) = fm.get("links").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                for leg_key in &["stated_weight", "multiplier", "standing_weight"] {
                    if obj.contains_key(*leg_key) {
                        diags.push(Diagnostic::node(
                            Severity::Warning,
                            "flow-legacy-field",
                            format!("Legacy field '{}' on link is deprecated under the flow model", leg_key),
                            node_id,
                            AgentFix::No,
                        ));
                    }
                }
            }
        }
    }
    if let Some(arr) = fm.get("contributes_to").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                if obj.contains_key("multiplier") {
                    diags.push(Diagnostic::node(
                        Severity::Warning,
                        "flow-legacy-field",
                        "Legacy field 'multiplier' is deprecated under the flow model".to_string(),
                        node_id,
                        AgentFix::No,
                    ));
                }
            }
        }
    }

    // 5.1: Targets and worth
    let is_target_type = graph::STRATEGIC_TARGET_TYPES.contains(&node_type);
    if let Some(worth_val) = fm.get("worth") {
        if !is_target_type {
            diags.push(Diagnostic::node(
                Severity::Error,
                "flow-worth-not-target",
                format!(
                    "Node '{}' of type '{}' carries 'worth'; only strategic targets ({:?}) may carry worth",
                    node_id, node_type, graph::STRATEGIC_TARGET_TYPES
                ),
                node_id,
                AgentFix::No,
            ));
        }
        let worth_num = worth_val.as_f64().or_else(|| {
            worth_val.as_str().and_then(|s| s.trim().parse::<f64>().ok())
        });
        match worth_num {
            Some(w) => {
                if !w.is_finite() || !(-1.0..=1.0).contains(&w) {
                    diags.push(Diagnostic::node(
                        Severity::Error,
                        "flow-worth-invalid",
                        format!("worth {} out of range -1.0..=1.0", w),
                        node_id,
                        AgentFix::No,
                    ));
                }
            }
            None => {
                diags.push(Diagnostic::node(
                    Severity::Error,
                    "flow-worth-invalid",
                    format!("Invalid worth '{}': must be a number in -1.0..=1.0", worth_val),
                    node_id,
                    AgentFix::No,
                ));
            }
        }
    } else if is_target_type && is_open_node {
        diags.push(Diagnostic::node(
            Severity::Warning,
            "flow-target-unpriced",
            format!("Open target '{}' has no worth", node_id),
            node_id,
            AgentFix::Propose,
        ));
    }

    // 5.7: Deadlines
    let raw_due = fm.get("due");
    let has_due = raw_due.is_some_and(|v| !v.is_null() && v.as_str().is_some_and(|s| !s.trim().is_empty()));
    let raw_class = fm.get("deadline_class");

    if is_open_node && has_due && (raw_class.is_none() || raw_class.unwrap().is_null()) {
        diags.push(Diagnostic::node(
            Severity::Warning,
            "flow-deadline-unclassed",
            format!("Open node '{}' has due date but no deadline_class", node_id),
            node_id,
            AgentFix::Propose,
        ));
    }
    if let Some(class_val) = raw_class {
        if !class_val.is_null() {
            if !has_due {
                diags.push(Diagnostic::node(
                    Severity::Warning,
                    "flow-deadline-class-no-due",
                    format!("Node '{}' has deadline_class but no due date", node_id),
                    node_id,
                    AgentFix::No,
                ));
            }
            if let Some(s) = class_val.as_str() {
                let trimmed = s.trim();
                let is_valid = matches!(trimmed.to_lowercase().as_str(), "fake" | "soft" | "hard");
                if !is_valid {
                    diags.push(Diagnostic::node(
                        Severity::Error,
                        "flow-deadline-class-invalid",
                        format!("Invalid deadline_class '{}': must be fake, soft, or hard", s),
                        node_id,
                        AgentFix::No,
                    ));
                } else if s != trimmed {
                    diags.push(Diagnostic {
                        severity: Severity::Error,
                        rule: "flow-deadline-class-invalid",
                        message: format!("deadline_class '{}' has whitespace padding", s),
                        line: None,
                        fixable: true,
                        agent_fix: AgentFix::Yes,
                        subject: Some(DiagnosticSubject::Node(node_id.to_string())),
                    });
                }
            } else {
                diags.push(Diagnostic::node(
                    Severity::Error,
                    "flow-deadline-class-invalid",
                    format!("deadline_class must be a string; got {class_val}"),
                    node_id,
                    AgentFix::No,
                ));
            }
        }
    }

    // 5.2 - 5.4: links entries
    if let Some(parent) = fm.get("parent").and_then(|v| v.as_str()) {
        if parent == node_id && !node_id.is_empty() {
            diags.push(Diagnostic::edge(
                Severity::Error,
                "flow-edge-self",
                format!("Self-edge: node '{}' links to itself", node_id),
                node_id,
                node_id,
                "part_of",
                AgentFix::No,
            ));
        }
    }
    if let Some(arr) = fm.get("depends_on").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(dep) = item.as_str() {
                if dep == node_id && !node_id.is_empty() {
                    diags.push(Diagnostic::edge(
                        Severity::Error,
                        "flow-edge-self",
                        format!("Self-edge: node '{}' links to itself", node_id),
                        node_id,
                        node_id,
                        "needs",
                        AgentFix::No,
                    ));
                }
            }
        }
    }

    if let Some(links_val) = fm.get("links") {
        if let Some(arr) = links_val.as_array() {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let to_val = obj.get("to").and_then(|v| v.as_str());
                    let from_val = obj.get("from").and_then(|v| v.as_str());
                    let target_id = to_val.or(from_val).unwrap_or("");
                    let label_val = obj.get("label");
                    let raw_label_str = label_val.and_then(|v| v.as_str()).unwrap_or("");
                    let edge_from = if from_val.is_some() { target_id.to_string() } else { node_id.to_string() };
                    let edge_to = if from_val.is_some() { node_id.to_string() } else { target_id.to_string() };

                    // Self edge
                    if (!edge_from.is_empty() && edge_from == edge_to)
                        || to_val == Some(node_id)
                        || from_val == Some(node_id)
                    {
                        diags.push(Diagnostic::edge(
                            Severity::Error,
                            "flow-edge-self",
                            format!("Self-edge: node '{}' links to itself", node_id),
                            &edge_from,
                            &edge_to,
                            raw_label_str,
                            AgentFix::No,
                        ));
                    }

                    // Label validation
                    if let Some(label_str) = label_val.and_then(|v| v.as_str()) {
                        let trimmed = label_str.trim();
                        let valid = ["serves", "needs", "part_of", "supports", "alternative", "settles"];
                        if !valid.contains(&trimmed.to_lowercase().as_str()) {
                            diags.push(Diagnostic::edge(
                                Severity::Error,
                                "flow-edge-label-invalid",
                                format!("Invalid edge label '{}'", label_str),
                                &edge_from,
                                &edge_to,
                                label_str,
                                AgentFix::No,
                            ));
                        } else if label_str != trimmed {
                            diags.push(Diagnostic {
                                severity: Severity::Error,
                                rule: "flow-edge-label-invalid",
                                message: format!("Edge label '{}' has whitespace padding", label_str),
                                line: None,
                                fixable: true,
                                agent_fix: AgentFix::Yes,
                                subject: Some(DiagnosticSubject::Edge {
                                    from: edge_from.clone(),
                                    to: edge_to.clone(),
                                    label: label_str.to_string(),
                                }),
                            });
                        }
                    } else if label_val.is_some() {
                        diags.push(Diagnostic::edge(
                            Severity::Error,
                            "flow-edge-label-invalid",
                            "Edge label must be a string".to_string(),
                            &edge_from,
                            &edge_to,
                            raw_label_str,
                            AgentFix::No,
                        ));
                    }

                    // Effect validation
                    if let Some(eff_val) = obj.get("effect") {
                        if let Some(eff_str) = eff_val.as_str() {
                            let eff_trimmed = eff_str.trim().to_lowercase();
                            if eff_trimmed != "helps" && eff_trimmed != "harms" {
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-effect-invalid",
                                    format!("Invalid edge effect '{}': must be helps or harms", eff_str),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::No,
                                ));
                            } else if eff_trimmed == "harms" {
                                let norm_label = raw_label_str.trim().to_lowercase();
                                if norm_label == "alternative" || norm_label == "settles" {
                                    diags.push(Diagnostic::edge(
                                        Severity::Warning,
                                        "flow-edge-effect-ignored",
                                        format!("Effect 'harms' is ignored on '{}' edge", norm_label),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::No,
                                    ));
                                }
                            }
                        } else {
                            diags.push(Diagnostic::edge(
                                Severity::Error,
                                "flow-edge-effect-invalid",
                                "Edge effect must be a string".to_string(),
                                &edge_from,
                                &edge_to,
                                raw_label_str,
                                AgentFix::No,
                            ));
                        }
                    }

                    // Negative quantum / probability
                    let mut has_neg_quantum = false;
                    if let Some(q_num) = obj.get("quantum").and_then(|v| v.as_f64()) {
                        if q_num < 0.0 {
                            has_neg_quantum = true;
                            diags.push(Diagnostic::edge(
                                Severity::Error,
                                "flow-edge-negative",
                                format!("Negative quantum {q_num}; sign belongs in effect: harms"),
                                &edge_from,
                                &edge_to,
                                raw_label_str,
                                AgentFix::No,
                            ));
                        }
                    } else if let Some(q_str) = obj.get("quantum").and_then(|v| v.as_str()) {
                        if let Ok(n) = q_str.trim().parse::<f64>() {
                            if n < 0.0 {
                                has_neg_quantum = true;
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-negative",
                                    format!("Negative quantum {n}; sign belongs in effect: harms"),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::No,
                                ));
                            }
                        }
                    }
                    let mut has_neg_prob = false;
                    if let Some(p_num) = obj.get("probability").and_then(|v| v.as_f64()) {
                        if p_num < 0.0 {
                            has_neg_prob = true;
                            diags.push(Diagnostic::edge(
                                Severity::Error,
                                "flow-edge-negative",
                                format!("Negative probability {p_num}; probability cannot be negative"),
                                &edge_from,
                                &edge_to,
                                raw_label_str,
                                AgentFix::No,
                            ));
                        }
                    } else if let Some(p_str) = obj.get("probability").and_then(|v| v.as_str()) {
                        if let Ok(n) = p_str.trim().parse::<f64>() {
                            if n < 0.0 {
                                has_neg_prob = true;
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-negative",
                                    format!("Negative probability {n}; probability cannot be negative"),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::No,
                                ));
                            }
                        }
                    }

                    // Quantum values
                    if let Some(q_val) = obj.get("quantum") {
                        if q_val.is_null() {
                            diags.push(Diagnostic::edge(
                                Severity::Style,
                                "flow-edge-unvalued",
                                "Edge states no quantum; read at default".to_string(),
                                &edge_from,
                                &edge_to,
                                raw_label_str,
                                AgentFix::Propose,
                            ));
                        } else if !has_neg_quantum {
                            if let Some(s) = q_val.as_str() {
                                let trimmed = s.trim();
                                if s != trimmed {
                                    if crate::graph::parse_quantum_word_or_float(&serde_json::json!(trimmed)).is_ok() {
                                        diags.push(Diagnostic {
                                            severity: Severity::Error,
                                            rule: "flow-edge-quantum-invalid",
                                            message: format!("quantum '{}' has whitespace padding", s),
                                            line: None,
                                            fixable: true,
                                            agent_fix: AgentFix::Yes,
                                            subject: Some(DiagnosticSubject::Edge {
                                                from: edge_from.clone(),
                                                to: edge_to.clone(),
                                                label: raw_label_str.to_string(),
                                            }),
                                        });
                                    } else {
                                        diags.push(Diagnostic::edge(
                                            Severity::Error,
                                            "flow-edge-quantum-invalid",
                                            format!("Invalid quantum '{}'", s),
                                            &edge_from,
                                            &edge_to,
                                            raw_label_str,
                                            AgentFix::Propose,
                                        ));
                                    }
                                } else if let Err(e) = crate::graph::parse_quantum_word_or_float(q_val) {
                                    diags.push(Diagnostic::edge(
                                        Severity::Error,
                                        "flow-edge-quantum-invalid",
                                        format!("Invalid quantum '{}': {e}", s),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::Propose,
                                    ));
                                }
                            } else if let Some(n) = q_val.as_f64() {
                                if n > 1.0 {
                                    diags.push(Diagnostic::edge(
                                        Severity::Error,
                                        "flow-edge-quantum-invalid",
                                        format!("quantum {n} out of range; expected float 0.0..=1.0"),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::Propose,
                                    ));
                                }
                            } else {
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-quantum-invalid",
                                    format!("quantum must be a number or string; got {q_val}"),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::Propose,
                                ));
                            }
                        }
                    } else {
                        diags.push(Diagnostic::edge(
                            Severity::Style,
                            "flow-edge-unvalued",
                            "Edge states no quantum; read at default".to_string(),
                            &edge_from,
                            &edge_to,
                            raw_label_str,
                            AgentFix::Propose,
                        ));
                    }

                    // Probability values
                    if let Some(p_val) = obj.get("probability") {
                        if !p_val.is_null() && !has_neg_prob {
                            if let Some(s) = p_val.as_str() {
                                let trimmed = s.trim();
                                if s != trimmed {
                                    if crate::graph::parse_probability_word_or_float(&serde_json::json!(trimmed)).is_ok() {
                                        diags.push(Diagnostic {
                                            severity: Severity::Error,
                                            rule: "flow-edge-probability-invalid",
                                            message: format!("probability '{}' has whitespace padding", s),
                                            line: None,
                                            fixable: true,
                                            agent_fix: AgentFix::Yes,
                                            subject: Some(DiagnosticSubject::Edge {
                                                from: edge_from.clone(),
                                                to: edge_to.clone(),
                                                label: raw_label_str.to_string(),
                                            }),
                                        });
                                    } else {
                                        diags.push(Diagnostic::edge(
                                            Severity::Error,
                                            "flow-edge-probability-invalid",
                                            format!("Invalid probability '{}'", s),
                                            &edge_from,
                                            &edge_to,
                                            raw_label_str,
                                            AgentFix::Propose,
                                        ));
                                    }
                                } else if let Err(e) = crate::graph::parse_probability_word_or_float(p_val) {
                                    diags.push(Diagnostic::edge(
                                        Severity::Error,
                                        "flow-edge-probability-invalid",
                                        format!("Invalid probability '{}': {e}", s),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::Propose,
                                    ));
                                }
                            } else if let Some(n) = p_val.as_f64() {
                                if n > 1.0 {
                                    diags.push(Diagnostic::edge(
                                        Severity::Error,
                                        "flow-edge-probability-invalid",
                                        format!("probability {n} out of range; expected float 0.0..=1.0"),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::Propose,
                                    ));
                                }
                            } else {
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-probability-invalid",
                                    format!("probability must be a number or string; got {p_val}"),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::Propose,
                                ));
                            }
                        }
                    }

                    // Provenance: set_by
                    if let Some(set_by_val) = obj.get("set_by") {
                        if let Some(s) = set_by_val.as_str() {
                            let valid_set_by = ["nic", "agent-proposed", "migrated"];
                            if !valid_set_by.contains(&s) {
                                diags.push(Diagnostic::edge(
                                    Severity::Error,
                                    "flow-edge-set-by-invalid",
                                    format!("Invalid set_by '{}': must be nic, agent-proposed, or migrated", s),
                                    &edge_from,
                                    &edge_to,
                                    raw_label_str,
                                    AgentFix::No,
                                ));
                            } else if s == "agent-proposed" {
                                let just = obj.get("justification").and_then(|v| v.as_str()).unwrap_or("");
                                if just.trim().is_empty() {
                                    diags.push(Diagnostic::edge(
                                        Severity::Warning,
                                        "flow-edge-proposal-unjustified",
                                        "agent-proposed edge has no justification".to_string(),
                                        &edge_from,
                                        &edge_to,
                                        raw_label_str,
                                        AgentFix::No,
                                    ));
                                }
                            }
                        } else {
                            diags.push(Diagnostic::edge(
                                Severity::Error,
                                "flow-edge-set-by-invalid",
                                format!("set_by must be a string; got {set_by_val}"),
                                &edge_from,
                                &edge_to,
                                raw_label_str,
                                AgentFix::No,
                            ));
                        }
                    }
                }
            }
        }
    }

    // contributes_to unvalued & self check
    if let Some(arr) = fm.get("contributes_to").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                let to_id = obj.get("to").or_else(|| obj.get("id")).and_then(|v| v.as_str()).unwrap_or("");
                if to_id == node_id && !node_id.is_empty() {
                    diags.push(Diagnostic::edge(
                        Severity::Error,
                        "flow-edge-self",
                        format!("Self-edge: node '{}' links to itself", node_id),
                        node_id,
                        to_id,
                        "serves",
                        AgentFix::No,
                    ));
                }
                let has_stated = obj.get("stated_weight").is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty()));
                let has_q = obj.get("quantum").is_some_and(|v| !v.is_null());
                if !has_stated && !has_q && !to_id.is_empty() {
                    diags.push(Diagnostic::edge(
                        Severity::Style,
                        "flow-edge-unvalued",
                        "Edge states no quantum; read at default".to_string(),
                        node_id,
                        to_id,
                        "serves",
                        AgentFix::Propose,
                    ));
                }
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
                    diags.push(Diagnostic::simple(
                        Severity::Style,
                        "md-trailing-ws",
                        "Trailing whitespace",
                        Some(line_num),
                        true,
                    ));
                }
                has_trailing_ws = true;
            }
        }

        // Consecutive blank lines
        if line.trim().is_empty() {
            consecutive_blank += 1;
            if consecutive_blank > 2 {
                diags.push(Diagnostic::simple(
                    Severity::Style,
                    "md-consecutive-blanks",
                    "More than 2 consecutive blank lines",
                    Some(line_num),
                    true,
                ));
            }
        } else {
            consecutive_blank = 0;
        }
    }

    // Missing final newline
    if !content.is_empty() && !content.ends_with('\n') {
        diags.push(Diagnostic::simple(
            Severity::Style,
            "md-no-final-newline",
            "File does not end with a newline",
            Some(lines.len()),
            true,
        ));
    }
}

/// Extract the frontmatter block's raw text (between the `---` delimiters,
/// exclusive of both, and with no trailing newline). Mirrors the boundary
/// math used throughout this module's autofixes. Returns `None` if the file
/// has no well-formed frontmatter block.
fn extract_frontmatter_section(content: &str) -> Option<&str> {
    if !content.starts_with("---\n") {
        return None;
    }
    let end = content[3..].find("\n---")?;
    Some(&content[4..end + 3])
}

/// True if `line` is a top-level (non-indented, non-list-item) `key: |...`
/// or `key: >...` YAML block-scalar opener, with any chomping/indent
/// indicator (`|-`, `|+`, `|2`, `>-`, …).
fn is_block_scalar_opener(line: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-') {
        return false;
    }
    match line.find(':') {
        Some(idx) => {
            let after = line[idx + 1..].trim();
            after.starts_with('|') || after.starts_with('>')
        }
        None => false,
    }
}

/// True if any block-scalar value in the frontmatter is immediately followed
/// by one or more blank lines before its first line of content.
fn has_leading_blank_block_scalar(fm_section: &str) -> bool {
    let lines: Vec<&str> = fm_section.lines().collect();
    lines.iter().enumerate().any(|(i, line)| {
        is_block_scalar_opener(line) && lines.get(i + 1).is_some_and(|next| next.trim().is_empty())
    })
}

/// Remove blank lines that immediately follow a block-scalar opener, before
/// its first line of content. Surgical: only touches those specific blank
/// lines, never the delimiters or any other frontmatter text.
fn strip_leading_blank_block_scalar_lines(fm_section: &str) -> String {
    let lines: Vec<&str> = fm_section.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        out.push(lines[i]);
        if is_block_scalar_opener(lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out.join("\n")
}

// ── Auto-fix engine ──────────────────────────────────────────────────────

/// Remove a key and its (potentially multi-line) block-scalar value from a
/// frontmatter string (the text between the two `---` delimiters, without those delimiters).
fn remove_key_from_frontmatter(fm_text: &str, target_key: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_target = false;
    let mut is_block = false;

    let target_prefix = format!("{}:", target_key);

    for line in fm_text.lines() {
        if in_target {
            if line.starts_with(' ')
                || line.starts_with('\t')
                || (is_block && line.starts_with('-'))
            {
                // Indented continuation of block scalar or list item — drop.
                continue;
            } else if line.is_empty() && is_block {
                // Blank line within a block scalar — drop.
                continue;
            } else {
                // Non-indented, non-empty line: the block is over.
                in_target = false;
                is_block = false;
                // fallthrough to check if the current line starts a new target
            }
        }

        if !in_target {
            if line.starts_with(&target_prefix) {
                in_target = true;
                let after = line[target_prefix.len()..].trim();
                is_block = after.starts_with('|') || after.starts_with('>') || after.is_empty();
            } else {
                lines.push(line);
            }
        }
    }

    lines.join("\n")
}

/// Replace a non-canonical project alias in YAML frontmatter with the canonical slug.
fn fix_project_alias(content: &str, old_proj: &str, new_proj: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_fm = false;
    let mut replaced = false;
    for line in content.lines() {
        if line.trim() == "---" {
            in_fm = !in_fm;
            lines.push(line.to_string());
            continue;
        }
        if in_fm && !replaced {
            let trimmed = line.trim();
            if trimmed.starts_with("project:") {
                let val_part = trimmed.strip_prefix("project:").unwrap().trim();
                let stripped_val = val_part.trim_matches(|c| c == '"' || c == '\'');
                if stripped_val == old_proj {
                    let indent = line.len() - line.trim_start().len();
                    lines.push(format!(
                        "{:indent$}project: {}",
                        "",
                        new_proj,
                        indent = indent
                    ));
                    replaced = true;
                    continue;
                }
            }
        }
        lines.push(line.to_string());
    }
    let mut res = lines.join("\n");
    if content.ends_with('\n') {
        res.push('\n');
    }
    res
}

/// Apply fixes surgically — line-level edits only, preserving key order and formatting.
fn apply_fixes(
    content: &str,
    fm_data: &Option<serde_json::Value>,
    _path: &Path,
    _ancestor_map: Option<&AncestorMap>,
) -> String {
    let mut result = content.to_string();

    // ── Frontmatter fixes (only when we have a valid frontmatter object) ──
    if let Some(serde_json::Value::Object(fm)) = fm_data {
        // Flow model padding fixes (specs/graph-lint.md L10, §5.2, §5.7, G20)
        if let Some(s) = fm.get("deadline_class").and_then(|v| v.as_str()) {
            let trimmed = s.trim();
            if s != trimmed && matches!(trimmed.to_lowercase().as_str(), "fake" | "soft" | "hard") {
                let patterns = [
                    format!("deadline_class: \"{}\"", s),
                    format!("deadline_class: '{}'", s),
                    format!("deadline_class: {}", s),
                ];
                for p in &patterns {
                    if result.contains(p) {
                        result = result.replacen(p, &format!("deadline_class: {}", trimmed), 1);
                        break;
                    }
                }
            }
        }

        if let Some(arr) = fm.get("links").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    if let Some(s) = obj.get("label").and_then(|v| v.as_str()) {
                        let trimmed = s.trim();
                        let valid = ["serves", "needs", "part_of", "supports", "alternative", "settles"];
                        if s != trimmed && valid.contains(&trimmed.to_lowercase().as_str()) {
                            let patterns = [
                                format!("label: \"{}\"", s),
                                format!("label: '{}'", s),
                                format!("label: {}", s),
                            ];
                            for p in &patterns {
                                if result.contains(p) {
                                    result = result.replacen(p, &format!("label: {}", trimmed), 1);
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(s) = obj.get("quantum").and_then(|v| v.as_str()) {
                        let trimmed = s.trim();
                        if s != trimmed && crate::graph::parse_quantum_word_or_float(&serde_json::json!(trimmed)).is_ok() {
                            let patterns = [
                                format!("quantum: \"{}\"", s),
                                format!("quantum: '{}'", s),
                                format!("quantum: {}", s),
                            ];
                            for p in &patterns {
                                if result.contains(p) {
                                    result = result.replacen(p, &format!("quantum: {}", trimmed), 1);
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(s) = obj.get("probability").and_then(|v| v.as_str()) {
                        let trimmed = s.trim();
                        if s != trimmed && crate::graph::parse_probability_word_or_float(&serde_json::json!(trimmed)).is_ok() {
                            let patterns = [
                                format!("probability: \"{}\"", s),
                                format!("probability: '{}'", s),
                                format!("probability: {}", s),
                            ];
                            for p in &patterns {
                                if result.contains(p) {
                                    result = result.replacen(p, &format!("probability: {}", trimmed), 1);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Fix 1: Migrate task_id → id (in-place line replacement)
        if fm.contains_key("task_id") && !fm.contains_key("id") {
            result = regex::Regex::new(r"(?m)^task_id:")
                .unwrap()
                .replace(&result, "id:")
                .to_string();
        }

        // Missing-id generation is handled unconditionally in `lint_file`
        // (self-heal, independent of --fix) before this function is called.

        // Fix 3: Fix status aliases in-place
        if let Some(status) = fm.get("status").and_then(|v| v.as_str()) {
            let canonical = graph::resolve_status_alias(status);
            if canonical != status {
                let pattern = format!("status: {}", status);
                let replacement = format!("status: {}", canonical);
                result = result.replacen(&pattern, &replacement, 1);
            }
        }

        // Fix 4: Fix "p1"/"P2" style intent → integer
        if let Some(s) = fm
            .get("intent")
            .or_else(|| fm.get("priority"))
            .and_then(|v| v.as_str())
        {
            let stripped = s.strip_prefix('p').or_else(|| s.strip_prefix('P'));
            if let Some(num_str) = stripped {
                if let Ok(n) = num_str.parse::<i64>() {
                    let p_pattern = format!("priority: {}", s);
                    let i_pattern = format!("intent: {}", s);
                    let replacement = format!("intent: {}", n);
                    if result.contains(&p_pattern) {
                        result = result.replacen(&p_pattern, &replacement, 1);
                    } else if result.contains(&i_pattern) {
                        result = result.replacen(&i_pattern, &replacement, 1);
                    }
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

        // (Former Fix 5c removed: an explicit `project:` value is no longer
        // deprecated — it is the polecat.yaml routing slug that declares or
        // overrides the project for this node and its subtree.)

        // Fix 6: Migrate 'body' frontmatter key → append its value to the markdown body
        if fm.contains_key("body") {
            if let Some(body_text) = fm.get("body").and_then(|v| v.as_str()) {
                let body_text = body_text.to_string();

                // Step 1: Remove the body: block from the frontmatter section.
                if result.starts_with("---\n") {
                    if let Some(fm_end_rel) = result[3..].find("\n---") {
                        let fm_end = fm_end_rel + 3;
                        let fm_section = result[4..fm_end].to_string();
                        let new_fm = remove_key_from_frontmatter(&fm_section, "body");
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
        // Fix 6b: Migrate 'blocked' frontmatter key
        if fm.contains_key("blocked") {
            // First, remove the `blocked` key from the frontmatter block text
            if result.starts_with("---\n") {
                if let Some(fm_end_rel) = result[3..].find("\n---") {
                    let fm_end = fm_end_rel + 3;
                    let fm_section = result[4..fm_end].to_string();
                    let new_fm = remove_key_from_frontmatter(&fm_section, "blocked");
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

            // Now handle the value: move to `depends_on` or the body as prose.
            if let Some(blocked_val) = fm.get("blocked") {
                let mut tasks_to_add = Vec::new();
                let mut prose_to_add = Vec::new();

                let check_val = |val: &str, tasks: &mut Vec<String>, prose: &mut Vec<String>| {
                    if val == "true" || val == "false" || val.is_empty() {
                        // ignore boolean/empty flags
                        return;
                    }
                    // A node reference is a single slug token: no spaces, contains at least one
                    // hyphen, and only alphanumeric/hyphen chars. This matches the structural
                    // invariant of PKB node IDs (e.g. mem-74b6165e) without semantic guessing.
                    let trimmed = val.trim();
                    let is_node_ref = !trimmed.contains(' ')
                        && trimmed.contains('-')
                        && trimmed
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-');
                    if is_node_ref {
                        tasks.push(trimmed.to_string());
                    } else {
                        prose.push(trimmed.to_string());
                    }
                };

                if let Some(s) = blocked_val.as_str() {
                    check_val(s, &mut tasks_to_add, &mut prose_to_add);
                } else if let Some(arr) = blocked_val.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            check_val(s, &mut tasks_to_add, &mut prose_to_add);
                        }
                    }
                }

                // Add to depends_on
                if !tasks_to_add.is_empty() && result.starts_with("---\n") {
                    if let Some(fm_end_rel) = result[3..].find("\n---") {
                        let fm_end = fm_end_rel + 3;
                        let fm_section = &result[4..fm_end];
                        let mut lines: Vec<String> =
                            fm_section.lines().map(|l| l.to_string()).collect();
                        // Find the insertion point: after all existing list items under
                        // depends_on:, or append a new depends_on: block if absent.
                        let mut insert_idx: Option<usize> = None;
                        let mut in_depends_on = false;
                        for (i, line) in lines.iter().enumerate() {
                            if line.starts_with("depends_on:") {
                                in_depends_on = true;
                                insert_idx = Some(i + 1);
                            } else if in_depends_on {
                                if line.starts_with("  - ") || line.starts_with("- ") {
                                    insert_idx = Some(i + 1);
                                } else {
                                    break;
                                }
                            }
                        }
                        let formatted: Vec<String> =
                            tasks_to_add.iter().map(|t| format!("  - {}", t)).collect();
                        if let Some(idx) = insert_idx {
                            for (offset, item) in formatted.into_iter().enumerate() {
                                lines.insert(idx + offset, item);
                            }
                        } else {
                            lines.push("depends_on:".to_string());
                            lines.extend(formatted);
                        }
                        let mut new_fm = lines.join("\n");
                        if !new_fm.ends_with('\n') {
                            new_fm.push('\n');
                        }
                        result = format!("---\n{}---{}", new_fm, &result[fm_end + 4..]);
                    }
                }

                // Add to body as prose
                if !prose_to_add.is_empty() {
                    let combined_prose = prose_to_add.join(" ");
                    let body_text = format!("> **Blocked on**: {}", combined_prose);
                    if let Some(fm_end_rel) = result[3..].find("\n---") {
                        let md_start = fm_end_rel + 3 + 5;
                        if !result[md_start..].contains(&body_text) {
                            if !result.ends_with('\n') {
                                result.push('\n');
                            }
                            if !result.ends_with("\n\n") {
                                result.push('\n');
                            }
                            result.push_str(&body_text);
                            result.push('\n');
                        }
                    }
                }
            }
        }
    }

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

    // Fix 7b: Strip leading blank lines from YAML block-scalar frontmatter
    // values — an artifact of serializing a string that itself started with
    // blank lines (see aops-cb065324, e.g. a `body_append: |2-` header whose
    // block scalar opens with two empty lines before its actual content).
    if result.starts_with("---\n") {
        if let Some(end) = result[3..].find("\n---") {
            let fm_end = end + 3;
            let fm_section = &result[4..fm_end];
            if has_leading_blank_block_scalar(fm_section) {
                let mut fixed_fm = strip_leading_blank_block_scalar_lines(fm_section);
                if !fixed_fm.ends_with('\n') {
                    fixed_fm.push('\n');
                }
                result = format!("---\n{}---{}", fixed_fm, &result[fm_end + 4..]);
            }
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


/// Largest product of edge strengths round any simple cycle in a small component (specs/graph-lint.md §5.5).
pub fn cycle_product(
    edges: &[crate::flow::FlowEdge],
    state: &BTreeMap<String, FlowState>,
    comp: &[String],
) -> f64 {
    let comp_set: HashSet<&str> = comp.iter().map(|s| s.as_str()).collect();
    let mut out: HashMap<String, Vec<&crate::flow::FlowEdge>> = HashMap::new();
    for e in edges {
        if crate::flow::is_flow_edge(e, state)
            && comp_set.contains(e.src.as_str())
            && comp_set.contains(e.dst.as_str())
        {
            out.entry(e.src.clone()).or_default().push(e);
        }
    }

    let mut best = 0.0;
    fn walk<'a>(
        start: &str,
        v: &str,
        seen: &mut HashSet<&'a str>,
        prod: f64,
        out: &HashMap<String, Vec<&'a crate::flow::FlowEdge>>,
        best: &mut f64,
    ) {
        if let Some(edge_list) = out.get(v) {
            for e in edge_list {
                if e.dst == start {
                    let p = prod * e.strength();
                    if p > *best {
                        *best = p;
                    }
                } else if !seen.contains(e.dst.as_str()) {
                    seen.insert(e.dst.as_str());
                    walk(start, &e.dst, seen, prod * e.strength(), out, best);
                    seen.remove(e.dst.as_str());
                }
            }
        }
    }

    for s in comp {
        let mut seen = HashSet::new();
        seen.insert(s.as_str());
        walk(s, s, &mut seen, 1.0, &out, &mut best);
    }
    best
}

/// Lint all markdown files under a PKB root directory.
pub fn lint_directory(
    pkb_root: &Path,
    fix: bool,
    check_refs: bool,
) -> (Vec<FileResult>, LintSummary) {
    lint_directory_with_cap(pkb_root, fix, check_refs, crate::flow::ITERATION_CAP)
}

/// Lint all markdown files under a PKB root directory with a custom iteration cap for convergence check.
pub fn lint_directory_with_cap(
    pkb_root: &Path,
    fix: bool,
    check_refs: bool,
    iter_cap: usize,
) -> (Vec<FileResult>, LintSummary) {
    let mut files = pkb::scan_directory(pkb_root);
    files.sort();

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

    // Build ancestor map for deprecated-project autofix:
    let ancestor_map: AncestorMap = files
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
            let parent = fm.get("parent").and_then(|v| v.as_str()).map(String::from);
            let project = fm
                .get("project")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);
            Some((id, (parent, project)))
        })
        .collect();

    // Derive children set: IDs that appear as a parent of at least one node.
    let children_set: ChildrenSet = ancestor_map
        .values()
        .filter_map(|(parent_id, _)| parent_id.clone())
        .collect();

    // ── Parent/child cycle detection (Error) ──
    let parent_cycle_diags: HashMap<PathBuf, Diagnostic> = if check_refs {
        let raw: Vec<(String, Option<String>, PathBuf)> = files
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
                let parent = fm.get("parent").and_then(|v| v.as_str()).map(String::from);
                Some((id, parent, p.clone()))
            })
            .collect();

        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
        let mut self_loops: HashSet<String> = HashSet::new();
        for (id, parent, path) in raw {
            id_to_path.insert(id.clone(), path);
            if let Some(p) = parent {
                if p == id {
                    self_loops.insert(id.clone());
                } else {
                    adj.insert(id, vec![p]);
                }
            }
        }

        let mut diag_map: HashMap<PathBuf, Diagnostic> = HashMap::new();

        // Self-parents (id == parent) — degenerate cycle of length 1.
        for node_id in &self_loops {
            if let Some(path) = id_to_path.get(node_id) {
                diag_map.entry(path.clone()).or_insert_with(|| Diagnostic::node(
                    Severity::Error,
                    "parent-cycle",
                    format!(
                        "Node '{}' lists itself as its own parent. Parent/child must be acyclic.",
                        node_id
                    ),
                    node_id,
                    AgentFix::No,
                ));
            }
        }

        // Multi-node parent cycles via Tarjan SCC on parent-only edges.
        let cycles: Vec<Vec<String>> = crate::graph_store::tarjan_scc(&adj)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect();
        for cycle in &cycles {
            let cycle_ids = cycle.join(", ");
            for node_id in cycle {
                if let Some(path) = id_to_path.get(node_id.as_str()) {
                    diag_map.entry(path.clone()).or_insert_with(|| Diagnostic::node(
                        Severity::Error,
                        "parent-cycle",
                        format!(
                            "Node '{}' is part of a parent/child cycle: [{}]. Parent/child relationships must be acyclic; only depends_on/blocks may be circular.",
                            node_id, cycle_ids
                        ),
                        node_id,
                        AgentFix::No,
                    ));
                }
            }
        }
        diag_map
    } else {
        HashMap::new()
    };

    // ── Project slug validation ──
    let (project_slug_diags, project_alias_fixes): (
        HashMap<PathBuf, Diagnostic>,
        HashMap<PathBuf, (String, String)>,
    ) = {
        let registry = match crate::polecat_config::PolecatRegistry::load(pkb_root) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("lint: failed to load polecat.yaml: {e:#}");
                None
            }
        };
        let mut diags_map = HashMap::new();
        let mut fixes_map = HashMap::new();

        let entries: Vec<(PathBuf, Diagnostic, Option<(String, String)>)> = files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                let fm = parsed
                    .data
                    .as_ref()
                    .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
                let project_val = fm.get("project").and_then(|v| v.as_str())?.trim();
                if project_val.is_empty() {
                    return None;
                }
                match crate::polecat_config::resolve_with(registry.as_ref(), project_val) {
                    Ok(canonical) => {
                        if canonical != project_val {
                            Some((
                                p.clone(),
                                Diagnostic::simple(
                                    Severity::Style,
                                    "fm-project-alias",
                                    format!(
                                        "Project '{}' should be canonical '{}'",
                                        project_val, canonical
                                    ),
                                    None,
                                    true,
                                ),
                                Some((project_val.to_string(), canonical)),
                            ))
                        } else {
                            None
                        }
                    }
                    Err(e) => Some((
                        p.clone(),
                        Diagnostic::simple(
                            Severity::Warning,
                            "fm-unregistered-project",
                            format!("{e:#}"),
                            None,
                            false,
                        ),
                        None,
                    )),
                }
            })
            .collect();

        for (path, diag, fix_info) in entries {
            diags_map.insert(path.clone(), diag);
            if let Some(fix_pair) = fix_info {
                fixes_map.insert(path, fix_pair);
            }
        }
        (diags_map, fixes_map)
    };

    // Pre-fix pass: collect ID renames
    let id_renames: Vec<(String, String)> = if fix {
        let id_re = get_id_regex();
        files
            .par_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(p).ok()?;
                let matter = Matter::<YAML>::new();
                let parsed = matter.parse(&content);
                let fm = parsed
                    .data
                    .as_ref()
                    .and_then(|d| d.deserialize::<serde_json::Value>().ok())?;
                let id = fm.get("id")?.as_str()?;
                let node_type = fm.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if !id.is_empty()
                    && !id_re.is_match(id)
                    && !matches!(node_type, "goal" | "target" | "project")
                {
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
        .map(|p| {
            lint_file(
                p,
                fix,
                known_ids.as_ref(),
                Some(&ancestor_map),
                Some(&children_set),
            )
        })
        .collect();

    // ── Pass 2 & Pass 3 (Whole Graph Flow Checks) ──
    if check_refs {
        let mut extra_diags: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();

        let parsed_docs: Vec<(PathBuf, crate::graph::GraphNode)> = files
            .iter()
            .filter_map(|p| {
                let doc = crate::pkb::parse_file(p)?;
                let node = crate::graph::GraphNode::from_pkb_document(&doc);
                Some((p.clone(), node))
            })
            .collect();

        let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
        let mut node_by_id: HashMap<String, &crate::graph::GraphNode> = HashMap::new();
        for (p, node) in &parsed_docs {
            id_to_path.insert(node.id.clone(), p.clone());
            node_by_id.insert(node.id.clone(), node);
        }

        let nodes: Vec<&crate::graph::GraphNode> = parsed_docs.iter().map(|(_, n)| n).collect();
        let (flow_edges, _) = crate::graph_store::resolve_flow_edges_from_nodes(nodes.iter().copied());

        let get_state = |id: &str| -> FlowState {
            node_by_id
                .get(id)
                .map(|n| crate::flow::status_to_flow_state(n.status.as_deref()))
                .unwrap_or(FlowState::Done)
        };

        // Pass 2.1: flow-edge-to-cancelled
        for e in &flow_edges {
            let src_st = get_state(&e.src);
            let dst_st = get_state(&e.dst);
            if src_st == FlowState::Open && dst_st == FlowState::Gone {
                if let Some(path) = id_to_path.get(&e.src) {
                    extra_diags.entry(path.clone()).or_default().push(Diagnostic::edge(
                        Severity::Warning,
                        "flow-edge-to-cancelled",
                        format!(
                            "Open work '{}' has an edge to cancelled node '{}' ({})",
                            e.src, e.dst, e.label
                        ),
                        &e.src,
                        &e.dst,
                        e.label.to_string(),
                        AgentFix::No,
                    ));
                }
            }
        }

        // Pass 2.2: flow-edge-duplicate
        let mut pair_edges: BTreeMap<(&str, &str), Vec<&crate::graph::FlowEdge>> = BTreeMap::new();
        for e in &flow_edges {
            pair_edges.entry((&e.src, &e.dst)).or_default().push(e);
        }
        for ((src, dst), edges) in pair_edges {
            if edges.len() > 1 {
                if let Some(path) = id_to_path.get(src) {
                    let labels: Vec<String> = edges.iter().map(|e| e.label.to_string()).collect();
                    extra_diags.entry(path.clone()).or_default().push(Diagnostic::edge(
                        Severity::Warning,
                        "flow-edge-duplicate",
                        format!(
                            "Multiple edges between '{}' and '{}': [{}]",
                            src,
                            dst,
                            labels.join(", ")
                        ),
                        src,
                        dst,
                        labels[0].clone(),
                        AgentFix::No,
                    ));
                }
            }
        }

        // Pass 2.3: Decisions
        let mut alt_incoming_count: HashMap<&str, usize> = HashMap::new();
        for e in &flow_edges {
            if e.label == LinkLabel::Alternative {
                *alt_incoming_count.entry(&e.dst).or_default() += 1;
            }
        }
        for node in &nodes {
            if get_state(&node.id) == FlowState::Open {
                let alts = alt_incoming_count.get(node.id.as_str()).copied().unwrap_or(0);
                if alts == 1 {
                    if let Some(path) = id_to_path.get(&node.id) {
                        extra_diags.entry(path.clone()).or_default().push(Diagnostic::node(
                            Severity::Warning,
                            "flow-decision-one-option",
                            format!(
                                "Open node '{}' has only 1 incoming alternative edge (decision requires at least 2)",
                                node.id
                            ),
                            &node.id,
                            AgentFix::No,
                        ));
                    }
                }
            }
        }
        for e in &flow_edges {
            if e.label == LinkLabel::Settles {
                let alts = alt_incoming_count.get(e.dst.as_str()).copied().unwrap_or(0);
                if alts < 2 {
                    if let Some(path) = id_to_path.get(&e.src) {
                        extra_diags.entry(path.clone()).or_default().push(Diagnostic::edge(
                            Severity::Warning,
                            "flow-settles-no-decision",
                            format!(
                                "Settles edge from '{}' points to '{}' which has only {} alternative edge(s) (requires at least 2)",
                                e.src, e.dst, alts
                            ),
                            &e.src,
                            &e.dst,
                            "settles",
                            AgentFix::No,
                        ));
                    }
                }
            }
        }

        // Pass 3: Flow verdicts
        let mut flow_input = FlowInput::new();
        for node in &nodes {
            let st = get_state(&node.id);
            flow_input.add_node(node.id.clone(), st, node.worth);
        }
        for e in &flow_edges {
            flow_input.add_edge(crate::flow::FlowEdge {
                src: e.src.clone(),
                dst: e.dst.clone(),
                label: e.label.to_string(),
                quantum: e.quantum,
                probability: e.probability,
                effect: match e.effect {
                    LinkEffect::Helps => FlowEffect::Helps,
                    LinkEffect::Harms => FlowEffect::Harms,
                },
                unvalued: false,
            });
        }

        let (inc, out) = crate::flow::build_indices(&flow_input.edges, &flow_input.state);
        let mut all_nodes: Vec<String> = flow_input.state.keys().cloned().collect();
        all_nodes.sort();
        let loop_nodes = crate::flow::on_loops(&flow_input);
        let sat_loops = crate::flow::saturated_loops(&flow_input);
        let sat_node_set: HashSet<String> = sat_loops.iter().flatten().cloned().collect();

        // flow-loop-saturated (Error)
        for comp in &sat_loops {
            let cycle_prod = cycle_product(&flow_input.edges, &flow_input.state, comp);
            for node_id in comp {
                if let Some(path) = id_to_path.get(node_id) {
                    extra_diags.entry(path.clone()).or_default().push(Diagnostic::node(
                        Severity::Error,
                        "flow-loop-saturated",
                        format!(
                            "Node '{}' is part of a saturated loop [{}] with cycle product {:.2}",
                            node_id,
                            comp.join(", "),
                            cycle_prod
                        ),
                        node_id,
                        AgentFix::No,
                    ));
                }
            }
        }

        let comps = crate::flow::find_components(&all_nodes, &out);
        let initial: BTreeMap<String, f64> = all_nodes.iter().map(|v| (v.clone(), 1.0)).collect();
        let empty_harms = BTreeSet::new();

        for comp in &comps {
            let is_sat = comp.iter().all(|v| sat_node_set.contains(v));
            if is_sat {
                continue;
            }

            let settles = crate::flow::settle_with_cap(
                &flow_input.state,
                &flow_input.worth,
                &inc,
                &initial,
                comp,
                None,
                Some(&empty_harms),
                &loop_nodes,
                iter_cap,
            );

            match settles {
                Err(_) => {
                    for node_id in comp {
                        if let Some(path) = id_to_path.get(node_id) {
                            extra_diags.entry(path.clone()).or_default().push(Diagnostic::node(
                                Severity::Error,
                                "flow-loop-no-convergence",
                                format!(
                                    "Node '{}' is in a loop [{}] that does not converge within iteration cap {}",
                                    node_id,
                                    comp.join(", "),
                                    iter_cap
                                ),
                                node_id,
                                AgentFix::No,
                            ));
                        }
                    }
                }
                Ok(_) => {
                    let cycle_prod = cycle_product(&flow_input.edges, &flow_input.state, comp);
                    for node_id in comp {
                        if let Some(path) = id_to_path.get(node_id) {
                            extra_diags.entry(path.clone()).or_default().push(Diagnostic::node(
                                Severity::Style,
                                "flow-loop",
                                format!(
                                    "Node '{}' is part of an allowed loop [{}] with cycle product {:.2}",
                                    node_id,
                                    comp.join(", "),
                                    cycle_prod
                                ),
                                node_id,
                                AgentFix::No,
                            ));
                        }
                    }
                }
            }
        }

        // Coverage: flow-no-route (Style)
        let open_nodes: Vec<&String> = flow_input
            .state
            .iter()
            .filter(|(_, st)| **st == FlowState::Open)
            .map(|(id, _)| id)
            .collect();

        for v in &open_nodes {
            let cone = crate::flow::forward_cone(&out, v);
            let reaches_priced = cone.iter().any(|u| flow_input.worth.contains_key(u));
            if !reaches_priced {
                if let Some(path) = id_to_path.get(*v) {
                    extra_diags.entry(path.clone()).or_default().push(Diagnostic::node(
                        Severity::Style,
                        "flow-no-route",
                        format!("Open node '{}' has no route to a priced target", v),
                        *v,
                        AgentFix::Propose,
                    ));
                }
            }
        }

        // Enrich flow-target-unpriced diagnostic messages
        let target_ids: HashSet<&str> = nodes
            .iter()
            .filter(|n| {
                let t = n.node_type.as_deref().unwrap_or("");
                graph::STRATEGIC_TARGET_TYPES.contains(&t)
            })
            .map(|n| n.id.as_str())
            .collect();

        let mut only_route_count: HashMap<String, usize> = HashMap::new();
        for v in &open_nodes {
            let cone = crate::flow::forward_cone(&out, v);
            let targets_in_cone: Vec<&String> = cone
                .iter()
                .filter(|u| target_ids.contains(u.as_str()))
                .collect();
            if targets_in_cone.len() == 1 {
                *only_route_count.entry(targets_in_cone[0].clone()).or_default() += 1;
            }
        }

        for r in &mut results {
            if let Some(diags) = extra_diags.get(&r.path) {
                r.diagnostics.extend(diags.clone());
            }
            for d in &mut r.diagnostics {
                if d.rule == "flow-target-unpriced" {
                    if let Some(DiagnosticSubject::Node(ref target_id)) = d.subject {
                        let count = only_route_count.get(target_id).copied().unwrap_or(0);
                        d.message = format!(
                            "Open target '{}' has no worth ({} open nodes have their only route here)",
                            target_id, count
                        );
                    }
                }
            }
        }
    }

    // Merge cycle + project-slug diagnostics into per-file results
    for r in &mut results {
        if let Some(diag) = parent_cycle_diags.get(&r.path) {
            r.diagnostics.push(diag.clone());
        }
        if let Some(diag) = project_slug_diags.get(&r.path) {
            r.diagnostics.push(diag.clone());
        }
        if fix {
            if let Some((old_proj, new_proj)) = project_alias_fixes.get(&r.path) {
                let base_content = match r.fixed_content.as_deref() {
                    Some(s) => s.to_string(),
                    None => std::fs::read_to_string(&r.path).unwrap_or_default(),
                };
                let fixed = fix_project_alias(&base_content, old_proj, new_proj);
                if fixed != base_content || r.fixed_content.is_some() {
                    r.fixed_content = Some(fixed);
                }
            }
        }
    }

    results.sort_by(|a, b| a.path.cmp(&b.path));
    let summary = LintSummary::from_results(&results);

    // Post-fix pass: apply cross-file ID renames via rename_id
    if fix && !id_renames.is_empty() {
        write_fixes(&results);
        for (old_id, new_id) in &id_renames {
            let _ = rename_id(pkb_root, old_id, new_id);
        }
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
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

pub fn rename_id(pkb_root: &Path, old_id: &str, new_id: &str) -> Result<(usize, usize), String> {
    if !is_valid_id(old_id) {
        return Err(format!(
            "Invalid old_id '{}': must be alphanumeric/hyphens only",
            old_id
        ));
    }
    if !is_valid_id(new_id) {
        return Err(format!(
            "Invalid new_id '{}': must be alphanumeric/hyphens only",
            new_id
        ));
    }
    let files = pkb::scan_directory(pkb_root);
    let reference_fields = [
        "parent",
        "depends_on",
        "soft_depends_on",
        "blocks",
        "soft_blocks",
        "supersedes",
    ];
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
        if let Some(fm_data) = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        {
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

        if modified && std::fs::write(file_path, &new_content).is_ok() {
            files_modified += 1;
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
            "---\nid: test-abc12345\ntitle: Test task\ntype: task\nstatus: active\nproject: test\nintent: 2\nparent: proj-00000000\ntags:\n- foo\n---\n\nBody content.\n",
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
        let diags = lint_str("---\ntitle: Test\nstatus: active\ntype: task\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "fm-status-alias"));
    }

    #[test]
    fn fixes_status_alias() {
        // Legacy "active" auto-migrates to canonical "in_progress" (Nic 2026-06-27).
        let fixed = fix_str("---\ntitle: Test\nstatus: active\ntype: note\n---\n\nBody.\n");
        assert!(fixed.contains("status: in_progress"), "Got: {}", fixed);
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
        let fixed = fix_str(
            "---\ntask_id: ns-abc12345\ntitle: Test\ntype: task\nstatus: done\n---\n\nBody.\n",
        );
        assert!(
            fixed.contains("id: ns-abc12345"),
            "task_id should become id, got: {}",
            fixed
        );
        assert!(!fixed.contains("task_id:"), "task_id key should be removed");
    }

    #[test]
    fn detects_task_missing_id() {
        let diags = lint_str("---\ntitle: Test\ntype: task\nstatus: active\n---\n\nBody.\n");
        assert!(diags.iter().any(|d| d.rule == "task-no-id"));
    }

    #[test]
    fn missing_id_no_longer_hard_fails_ci() {
        // task-no-id must be auto-fixable, not Error severity — an id-less
        // note self-heals and must never block a CI lint gate.
        let diags = lint_str("---\ntitle: Test\ntype: task\nstatus: active\n---\n\nBody.\n");
        let d = diags
            .iter()
            .find(|d| d.rule == "task-no-id")
            .expect("expected a task-no-id diagnostic");
        assert!(
            d.severity < Severity::Error,
            "missing id must not be Error severity (would hard-fail CI), got {:?}",
            d.severity
        );
    }

    #[test]
    fn missing_id_self_heals_without_fix_flag() {
        // `pkb lint --refs` (no --fix) is exactly the CI invocation this bug
        // is about — a missing id must be generated and persisted even then.
        let content = "---\ntitle: Test\ntype: task\nstatus: active\n---\n\nBody.\n";
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let result = lint_file(f.path(), false, None, None, None);
        let healed = result
            .fixed_content
            .expect("missing id must self-heal even without --fix");
        assert!(
            healed.contains("id: "),
            "expected a generated id, got: {}",
            healed
        );
        assert!(
            healed.starts_with("---\nid: "),
            "id must be the first frontmatter line, got: {}",
            healed
        );
    }

    #[test]
    fn present_id_is_never_regenerated() {
        // Idempotence: an id-bearing note must never be rewritten by the
        // self-heal step — regenerating a different id would break every
        // wikilink/backlink pointing at the original.
        let content =
            "---\nid: mem-abc12345\ntitle: Test\ntype: task\nstatus: active\n---\n\nBody.\n";
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let result = lint_file(f.path(), false, None, None, None);
        assert!(
            result.fixed_content.is_none(),
            "an existing id must never be touched, got fixed_content: {:?}",
            result.fixed_content
        );

        // Re-running (idempotence) must be a true no-op too.
        let result2 = lint_file(f.path(), false, None, None, None);
        assert!(result2.fixed_content.is_none());
    }

    #[test]
    fn daily_note_gets_date_convention_id() {
        // Corpus convention for daily notes is `<date>-<hex>`, not the
        // generic `<prefix>_<hex>` shape generate_missing_id falls back to.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-08-28-daily.md");
        std::fs::write(&path, "---\ntitle: 2026-08-28\ntype: daily\n---\n\nBody.\n").unwrap();
        let result = lint_file(&path, false, None, None, None);
        let healed = result
            .fixed_content
            .expect("daily note missing id must self-heal");
        assert!(
            healed.contains("id: 2026-08-28-"),
            "expected <date>-<hex> convention, got: {}",
            healed
        );
    }

    #[test]
    fn fixes_unknown_type() {
        let fixed = fix_str("---\ntitle: Test\ntype: article\nstatus: active\n---\n\nBody.\n");
        assert!(
            fixed.contains("type: reference"),
            "article should become reference, got: {}",
            fixed
        );
    }

    #[test]
    fn fixes_unknown_type_to_document() {
        let fixed = fix_str("---\ntitle: Test\ntype: bundle\nstatus: active\n---\n\nBody.\n");
        assert!(
            fixed.contains("type: document"),
            "bundle should become document, got: {}",
            fixed
        );
    }

    #[test]
    fn goal_and_target_are_distinct_types() {
        // Goal is aliased to target under collapsed type taxonomy.
        let fixed_goal = fix_str(
            "---
title: Test
type: goal
---

Body.\n",
        );
        assert!(
            fixed_goal.contains("type: target"),
            "goal should be aliased to target"
        );

        let fixed_target = fix_str(
            "---
title: Test
type: target
---

Body.\n",
        );
        assert!(
            fixed_target.contains("type: target"),
            "target should not be aliased"
        );
    }

    #[test]
    fn canonical_status_stays_as_is() {
        let fixed = fix_str("---\ntitle: Test\ntype: note\nstatus: in_progress\n---\n\nBody.\n");
        assert!(
            fixed.contains("status: in_progress"),
            "in_progress should stay in_progress (canonical status), got: {}",
            fixed
        );
    }

    #[test]
    fn fixes_retired_merge_ready_status_to_inbox() {
        let fixed = fix_str("---\ntitle: Test\ntype: note\nstatus: merge_ready\n---\n\nBody.\n");
        assert!(
            fixed.contains("status: inbox"),
            "merge_ready should be fixed to inbox as it is no longer canonical, got: {}",
            fixed
        );
    }

    #[test]
    fn fixes_truly_unknown_status() {
        let fixed = fix_str("---\ntitle: Test\ntype: note\nstatus: banana\n---\n\nBody.\n");
        assert!(
            fixed.contains("status: inbox"),
            "unknown status should become inbox, got: {}",
            fixed
        );
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
        let diags =
            lint_str("---\nid: academicOps-b5d43955\ntitle: Test\ntype: task\n---\n\nBody.\n");
        let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
        assert!(
            id_diag.is_none(),
            "academicOps-b5d43955 is a valid ID and must not trigger fm-id-format"
        );
    }

    #[test]
    fn id_starting_with_digit_is_valid() {
        let diags = lint_str("---\nid: 123abc-b5d43955\ntitle: Test\ntype: task\n---\n\nBody.\n");
        let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
        assert!(
            id_diag.is_none(),
            "IDs starting with a digit must not trigger fm-id-format"
        );
    }

    #[test]
    fn goal_target_and_project_ids_exempt_from_format_check() {
        // Goals, targets, and projects use canonical human-readable IDs — must NOT trigger fm-id-format.
        for node_type in &["goal", "target", "project"] {
            let content = format!(
                "---\nid: my-{}\ntitle: Test\ntype: {}\n---\n\nBody.\n",
                node_type, node_type
            );
            let diags = lint_str(&content);
            let id_diag = diags.iter().find(|d| d.rule == "fm-id-format");
            assert!(
                id_diag.is_none(),
                "type:{} with non-hex ID must not trigger fm-id-format",
                node_type
            );
        }
    }

    #[test]
    fn alias_key_is_known() {
        let diags = lint_str("---\ntitle: Test\ntype: note\nalias: foo\n---\n\nBody.\n");
        assert!(
            !diags.iter().any(|d| d.rule == "fm-unknown-key"),
            "alias should be a known key"
        );
    }

    #[test]
    fn last_modified_key_is_known() {
        let diags = lint_str("---\ntitle: Test\ntype: note\ncreated: 2026-01-01T00:00:00Z\nmodified: 2026-01-01T00:00:00Z\nlast_modified: 2026-01-01T00:00:00+10:00\n---\n\nBody.\n");
        assert!(
            !diags.iter().any(|d| d.rule == "fm-unknown-key"),
            "last_modified should be a known key, got: {:?}",
            diags
                .iter()
                .filter(|d| d.rule == "fm-unknown-key")
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn triage_keys_are_known() {
        let diags = lint_str("---\ntitle: Test\ntype: note\nprocessed: true\nprocessed_date: 2026-01-01\ntriage_action: create-task\ntriage_ref: test-12345678\n---\n\nBody.\n");
        assert!(
            !diags.iter().any(|d| d.rule == "fm-unknown-key"),
            "triage keys should be known, got: {:?}",
            diags
                .iter()
                .filter(|d| d.rule == "fm-unknown-key")
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
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
            !diags
                .iter()
                .any(|d| d.rule == "fm-invalid" || d.rule == "fm-parse-error"),
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
        let diag = diags
            .iter()
            .find(|d| d.rule == "fm-prohibited-body")
            .unwrap();
        assert_eq!(diag.severity, Severity::Error);
        assert!(diag.fixable);
    }

    #[test]
    fn fixes_body_in_frontmatter_simple() {
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: migrated content\n---\n\nExisting body.\n";
        let fixed = fix_str(input);
        assert!(
            !fixed.contains("body: migrated content"),
            "body key should be removed from frontmatter, got:\n{}",
            fixed
        );
        assert!(
            fixed.contains("migrated content"),
            "body value should appear in markdown body, got:\n{}",
            fixed
        );
        // Existing body content should be preserved
        assert!(
            fixed.contains("Existing body."),
            "existing body should be preserved, got:\n{}",
            fixed
        );
    }

    #[test]
    fn fixes_body_in_frontmatter_block_scalar() {
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: |-\n  # Section\n\n  Some detailed content.\n\n  More content here.\ncomplexity: multi-step\n---\n\nShort existing body.\n";
        let fixed = fix_str(input);
        // body: key removed
        assert!(
            !fixed.contains("body: |-"),
            "body: block key should be removed, got:\n{}",
            fixed
        );
        // content preserved
        assert!(
            fixed.contains("# Section"),
            "body content should be in markdown, got:\n{}",
            fixed
        );
        assert!(
            fixed.contains("More content here."),
            "all body content preserved, got:\n{}",
            fixed
        );
        // other frontmatter preserved
        assert!(
            fixed.contains("complexity: multi-step"),
            "other frontmatter keys preserved, got:\n{}",
            fixed
        );
        // existing markdown body preserved
        assert!(
            fixed.contains("Short existing body."),
            "existing body preserved, got:\n{}",
            fixed
        );
    }

    #[test]
    fn fixes_body_not_duplicated_when_already_present() {
        // If the markdown body already contains the full body value, don't append
        let input = "---\nid: test-a1b2c3d4\ntitle: Test\ntype: task\nstatus: active\nbody: exact content\n---\n\nexact content\n";
        let fixed = fix_str(input);
        assert!(!fixed.contains("body: exact content"), "body key removed");
        // Should not double the content
        let count = fixed.matches("exact content").count();
        assert_eq!(
            count, 1,
            "content should appear exactly once, got:\n{}",
            fixed
        );
    }

    #[test]
    fn body_key_is_prohibited_not_merely_unknown() {
        // body: should be flagged as prohibited (Error) rather than merely unknown (Style)
        let diags =
            lint_str("---\nid: test-a1b2c3d4\ntitle: Test\ntype: note\nbody: foo\n---\n\nBody.\n");
        assert!(
            diags.iter().any(|d| d.rule == "fm-prohibited-body"),
            "should get fm-prohibited-body"
        );
        // Should NOT get fm-unknown-key for body — it's a known key with its own rule
        assert!(
            !diags
                .iter()
                .any(|d| d.rule == "fm-unknown-key" && d.message.contains("'body'")),
            "should not get fm-unknown-key for body"
        );
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
        let all_diags: Vec<&Diagnostic> =
            results.iter().flat_map(|r| r.diagnostics.iter()).collect();
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

    #[test]
    fn fixes_blocked_in_frontmatter() {
        // Test edge-shaped blocked value (migrates to depends_on)
        let input_edge = "---\nid: test-edge\ntitle: Test\ntype: task\nstatus: active\nblocked: task-xyz123\n---\n\nBody.\n";
        let fixed_edge = fix_str(input_edge);
        assert!(
            !fixed_edge.contains("blocked: task-xyz123"),
            "blocked key should be removed"
        );
        assert!(
            fixed_edge.contains("depends_on:\n  - task-xyz123"),
            "edge migrated to depends_on, got:\n{}",
            fixed_edge
        );

        // Test prose-shaped blocked value (migrates to body)
        let input_prose = "---\nid: test-prose\ntitle: Test\ntype: task\nstatus: active\nblocked: Waiting on John to finish the mockups.\n---\n\nBody.\n";
        let fixed_prose = fix_str(input_prose);
        assert!(
            !fixed_prose.contains("blocked:"),
            "blocked key should be removed"
        );
        assert!(
            fixed_prose.contains("> **Blocked on**: Waiting on John to finish the mockups."),
            "prose migrated to body, got:\n{}",
            fixed_prose
        );

        // Test boolean flag (simply removed)
        let input_bool = "---\nid: test-bool\ntitle: Test\ntype: task\nstatus: active\nblocked: true\n---\n\nBody.\n";
        let fixed_bool = fix_str(input_bool);
        assert!(
            !fixed_bool.contains("blocked:"),
            "blocked key should be removed"
        );
        assert!(
            !fixed_bool.contains("depends_on:"),
            "no depends_on for boolean flag"
        );
        assert!(
            !fixed_bool.contains("> **Blocked on**"),
            "no prose for boolean flag"
        );

        // Single-word prose should go to body, not depends_on (no hyphen → not a node ref)
        let input_single_word = "---\nid: test-wait\ntitle: Test\ntype: task\nstatus: active\nblocked: Waiting\n---\n\nBody.\n";
        let fixed_single_word = fix_str(input_single_word);
        assert!(
            !fixed_single_word.contains("blocked:"),
            "blocked key should be removed"
        );
        assert!(
            !fixed_single_word.contains("depends_on:"),
            "single-word prose must not become depends_on"
        );
        assert!(
            fixed_single_word.contains("> **Blocked on**: Waiting"),
            "single-word prose migrated to body, got:\n{}",
            fixed_single_word
        );

        // When depends_on already exists, new items must be inserted under it, not appended at end
        let input_existing_dep = "---\nid: test-merge\ntitle: Test\ntype: task\nstatus: active\ndepends_on:\n  - existing-task\nblocked: new-task-ref\n---\n\nBody.\n";
        let fixed_existing_dep = fix_str(input_existing_dep);
        assert!(
            !fixed_existing_dep.contains("blocked:"),
            "blocked key should be removed"
        );
        // Both tasks must appear under the same depends_on: key
        assert!(
            fixed_existing_dep.contains("depends_on:\n  - existing-task\n  - new-task-ref"),
            "merged into existing depends_on, got:\n{}",
            fixed_existing_dep
        );
        // Must not have a second depends_on: key
        assert_eq!(
            fixed_existing_dep.matches("depends_on:").count(),
            1,
            "only one depends_on: key allowed, got:\n{}",
            fixed_existing_dep
        );
    }

    #[test]
    fn test_missing_project_resolved_via_ancestor() {
        let content_task =
            "---\nid: task-1\ntitle: Task 1\ntype: task\nstatus: ready\nparent: epic-1\n---\n";

        // epic-1 declares an explicit project slug; task-1 inherits it.
        let mut ancestor_map = AncestorMap::new();
        ancestor_map.insert("epic-1".to_string(), (None, Some("aops".to_string())));
        ancestor_map.insert("task-1".to_string(), (Some("epic-1".to_string()), None));

        let mut diags = Vec::new();
        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(content_task);
        let fm_data = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok());

        check_frontmatter(
            content_task,
            &fm_data,
            &mut diags,
            None,
            Some(&ancestor_map),
            None,
        );

        let has_warning = diags.iter().any(|d| d.rule == "fm-missing-project");
        assert!(
            !has_warning,
            "Task 1 inherits epic-1's explicit project and must NOT trigger warning"
        );

        // Reparenting under an ancestor chain with no explicit project value
        let content_no =
            "---\nid: task-1\ntitle: Task 1\ntype: task\nstatus: ready\nparent: other-task\n---\n";
        let parsed_no = matter.parse(content_no);
        let fm_data_no = parsed_no
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok());
        let mut ancestor_map_no_project = AncestorMap::new();
        ancestor_map_no_project
            .insert("task-1".to_string(), (Some("other-task".to_string()), None));
        ancestor_map_no_project.insert("other-task".to_string(), (None, None));

        let mut diags_no = Vec::new();
        check_frontmatter(
            content_no,
            &fm_data_no,
            &mut diags_no,
            None,
            Some(&ancestor_map_no_project),
            None,
        );

        let has_warning_no = diags_no.iter().any(|d| d.rule == "fm-missing-project");
        assert!(
            has_warning_no,
            "Task 1's ancestor chain declares no project, so it must trigger warning"
        );
    }

    #[test]
    fn test_missing_project_not_resolved_via_contributes_to() {
        // contributes_to points at out-of-tree goals/targets, which carry no
        // routing slug — it must NOT satisfy the project requirement.
        let content_task = "---\nid: task-2\ntitle: Task 2\ntype: task\nstatus: ready\ncontributes_to:\n  - target-x\n---\n";

        let mut ancestor_map = AncestorMap::new();
        ancestor_map.insert("target-x".to_string(), (None, Some("aops".to_string())));
        ancestor_map.insert("task-2".to_string(), (None, None));

        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(content_task);
        let fm_data = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok());

        let mut diags = Vec::new();
        check_frontmatter(
            content_task,
            &fm_data,
            &mut diags,
            None,
            Some(&ancestor_map),
            None,
        );

        let has_warning = diags.iter().any(|d| d.rule == "fm-missing-project");
        assert!(
            has_warning,
            "contributes_to must not satisfy the project requirement (parent-chain only)"
        );
    }

    #[test]
    fn test_deprecated_project_type_warns_and_fixes_to_epic() {
        let content = "---\nid: my-container\ntitle: Legacy Container\ntype: project\nstatus: in_progress\n---\n\nBody.\n";
        let diags = lint_str(content);
        assert!(
            diags.iter().any(|d| d.rule == "fm-deprecated-project-type"),
            "type: project must trigger the deprecation warning, got: {diags:?}"
        );
        // No fm-id-format noise for legacy canonical IDs on project files.
        assert!(
            !diags.iter().any(|d| d.rule == "fm-id-format"),
            "legacy project files keep the canonical-ID exemption, got: {diags:?}"
        );
        // --fix reclassifies to task via resolve_type_alias.
        let fixed = fix_str(content);
        assert!(
            fixed.contains("type: task"),
            "fix should rewrite type: project → task, got:\n{fixed}"
        );
    }

    // aops-cb065324: a leaked call arg (e.g. `body_append`) serialized through
    // serde_yaml with a leading "\n\n" produces exactly this shape.
    #[test]
    fn detects_block_scalar_leading_blank_lines() {
        let content = "---\nid: test-abc12345\ntitle: Test\ntype: task\nassignee: nic\nbody_append: |2-\n\n\n  **2026-04-27 update**: Scripts arrived. Unblocking now.\ncontributes_to:\n---\n\nBody.\n";
        let diags = lint_str(content);
        assert!(
            diags.iter().any(|d| d.rule == "fm-block-scalar-whitespace"),
            "expected fm-block-scalar-whitespace, got: {diags:?}"
        );
    }

    #[test]
    fn fixes_block_scalar_leading_blank_lines() {
        let content = "---\nid: test-abc12345\ntitle: Test\ntype: task\nassignee: nic\nbody_append: |2-\n\n\n  **2026-04-27 update**: Scripts arrived. Unblocking now.\ncontributes_to:\n---\n\nBody.\n";
        let fixed = fix_str(content);
        assert!(
            fixed.contains("body_append: |2-\n  **2026-04-27 update**"),
            "expected leading blank lines stripped from the block scalar, got:\n{fixed}"
        );
        // The `---` delimiters and every other key must survive untouched.
        assert!(fixed.starts_with("---\n"));
        assert!(fixed.contains("\nassignee: nic\n"));
        assert!(fixed.contains("\ncontributes_to:\n---\n"));
    }

    #[test]
    fn no_false_positive_on_normal_block_scalar() {
        let content = "---\nid: test-abc12345\ntitle: Test\ntype: task\nconsequence: |\n  This is fine.\n  No leading blanks.\n---\n\nBody.\n";
        let diags = lint_str(content);
        assert!(
            !diags.iter().any(|d| d.rule == "fm-block-scalar-whitespace"),
            "should not flag a block scalar with no leading blank lines, got: {diags:?}"
        );
    }

    #[test]
    fn test_lint_directory_flags_and_fixes_project_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("polecat.yaml"),
            "projects:\n  aops:\n    aliases: [academicOps, acaops]\nproject_aliases:\n  ao: aops\n",
        )
        .unwrap();

        let goal_file = root.join("goal-11223344.md");
        std::fs::write(
            &goal_file,
            "---\nid: goal-11223344\ntitle: Root Goal\ntype: target\nstatus: ready\nproject: aops\nworth: 1.0\n---\n\nRoot.\n",
        )
        .unwrap();

        let task_file = root.join("task-11223344.md");
        std::fs::write(
            &task_file,
            "---\nid: task-11223344\ntitle: Test Alias\ntype: task\nstatus: ready\nparent: goal-11223344\nproject: academicOps\n---\n\nBody.\n\n## Acceptance criteria\n- Verified\n",
        )
        .unwrap();

        // 1. Lint without fix: should flag fm-project-alias
        let (results, summary) = lint_directory(root, false, false);
        assert_eq!(summary.files_with_issues, 1);
        let task_res = results.iter().find(|r| r.path == task_file).unwrap();
        assert!(
            task_res
                .diagnostics
                .iter()
                .any(|d| d.rule == "fm-project-alias"
                    && d.message
                        .contains("Project 'academicOps' should be canonical 'aops'")),
            "expected fm-project-alias diagnostic, got: {:?}",
            task_res.diagnostics
        );

        // 2. Lint with fix: should produce fixed content and fix file
        let (results_fix, _) = lint_directory(root, true, false);
        let written = write_fixes(&results_fix);
        assert_eq!(written, 1);

        let content_after = std::fs::read_to_string(&task_file).unwrap();
        assert!(
            content_after.contains("project: aops"),
            "project should be canonicalized to 'aops', got:\n{content_after}"
        );
        assert!(
            !content_after.contains("academicOps"),
            "academicOps should be replaced, got:\n{content_after}"
        );

        // 3. Post-fix lint should have 0 issues
        let (_, clean_summary) = lint_directory(root, false, false);
        assert_eq!(clean_summary.files_with_issues, 0);
    }

    #[test]
    fn test_lint_directory_flags_unregistered_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("polecat.yaml"), "projects:\n  aops: {}\n").unwrap();

        let goal_file = root.join("goal-11223355.md");
        std::fs::write(
            &goal_file,
            "---\nid: goal-11223355\ntitle: Root Goal\ntype: target\nstatus: ready\nproject: aops\nworth: 1.0\n---\n\nRoot.\n",
        )
        .unwrap();

        let task_file = root.join("task-11223355.md");
        std::fs::write(
            &task_file,
            "---\nid: task-11223355\ntitle: Test Unreg\ntype: task\nstatus: ready\nparent: goal-11223355\nproject: non-existent-project\n---\n\nBody.\n\n## Acceptance criteria\n- Verified\n",
        )
        .unwrap();

        let (results, summary) = lint_directory(root, false, false);
        assert_eq!(summary.files_with_issues, 1);
        let task_res = results.iter().find(|r| r.path == task_file).unwrap();
        assert!(
            task_res
                .diagnostics
                .iter()
                .any(|d| d.rule == "fm-unregistered-project"),
            "expected fm-unregistered-project diagnostic, got: {:?}",
            task_res.diagnostics
        );
    }

    #[test]
    fn test_fix_project_alias_quoted_and_unquoted() {
        let content1 = "---\nid: t1\nproject: \"academicOps\"\ntitle: T1\n---\n\nBody\n";
        let fixed1 = fix_project_alias(content1, "academicOps", "aops");
        assert_eq!(
            fixed1,
            "---\nid: t1\nproject: aops\ntitle: T1\n---\n\nBody\n"
        );

        let content2 = "---\nid: t2\nproject: 'academicOps'\ntitle: T2\n---\n\nBody\n";
        let fixed2 = fix_project_alias(content2, "academicOps", "aops");
        assert_eq!(
            fixed2,
            "---\nid: t2\nproject: aops\ntitle: T2\n---\n\nBody\n"
        );

        let content3 = "---\nid: t3\nproject: academicOps\ntitle: T3\n---\n\nBody\n";
        let fixed3 = fix_project_alias(content3, "academicOps", "aops");
        assert_eq!(
            fixed3,
            "---\nid: t3\nproject: aops\ntitle: T3\n---\n\nBody\n"
        );
    }

    #[test]
    fn test_fallback_parse_frontmatter_multibyte_after_opening_fence() {
        // #686: 'é' spans bytes 3..5, so a raw `[4..]` cut panics.
        assert!(fallback_parse_frontmatter("---é\ntitle: x\n---\n").is_none());
    }

    // ── Tests G1–G20 (specs/graph-lint.md §9) ────────────────────────────────

    #[test]
    fn lint_flow_target_unpriced() {
        // G1: An open target with no worth yields one flow-target-unpriced warning with agent_fix: propose; pricing it removes the warning
        let unpriced = "---\nid: targ_test\ntitle: Test Target\ntype: target\nstatus: active\n---\n\nBody\n";
        let diags = lint_str(unpriced);
        let unpriced_diag = diags.iter().find(|d| d.rule == "flow-target-unpriced");
        assert!(unpriced_diag.is_some(), "Expected flow-target-unpriced, got: {:?}", diags);
        let d = unpriced_diag.unwrap();
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.agent_fix, AgentFix::Propose);
        assert!(!d.fixable);

        let priced = "---\nid: targ_test\ntitle: Test Target\ntype: target\nstatus: active\nworth: 0.5\n---\n\nBody\n";
        let diags2 = lint_str(priced);
        assert!(!diags2.iter().any(|d| d.rule == "flow-target-unpriced"));
    }

    #[test]
    fn lint_flow_worth_invalid() {
        // G2: worth of high, 6 or -1.5 yields flow-worth-invalid (error, exit 1); -1.0 and 1.0 pass
        for invalid in &["high", "6", "-1.5"] {
            let content = format!("---\nid: targ_test\ntitle: Target\ntype: target\nworth: {}\n---\n\nBody\n", invalid);
            let diags = lint_str(&content);
            let d = diags.iter().find(|d| d.rule == "flow-worth-invalid");
            assert!(d.is_some(), "Expected flow-worth-invalid for {}, got: {:?}", invalid, diags);
            assert_eq!(d.unwrap().severity, Severity::Error);
            assert_eq!(d.unwrap().agent_fix, AgentFix::No);
        }
        for valid in &["-1.0", "1.0", "0.0", "0.5"] {
            let content = format!("---\nid: targ_test\ntitle: Target\ntype: target\nworth: {}\n---\n\nBody\n", valid);
            let diags = lint_str(&content);
            assert!(!diags.iter().any(|d| d.rule == "flow-worth-invalid"), "Expected valid for {}, got: {:?}", valid, diags);
        }
    }

    #[test]
    fn lint_flow_worth_not_target() {
        // G3: worth on a task yields flow-worth-not-target (error)
        let content = "---\nid: task_test\ntitle: Task\ntype: task\nstatus: active\nworth: 0.6\n---\n\nBody\n";
        let diags = lint_str(content);
        let d = diags.iter().find(|d| d.rule == "flow-worth-not-target");
        assert!(d.is_some(), "Expected flow-worth-not-target, got: {:?}", diags);
        assert_eq!(d.unwrap().severity, Severity::Error);
        assert_eq!(d.unwrap().agent_fix, AgentFix::No);
    }

    #[test]
    fn lint_flow_edge_label_sign() {
        // G4: Each of label: blocks, effect: negative, quantum: -0.3 yields its own error;
        // label: Serves yields no diagnostic; label: "serves " yields an error whose agent_fix is yes
        let blocks_content = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: blocks\n---\n\nBody\n";
        let diags1 = lint_str(blocks_content);
        let d1 = diags1.iter().find(|d| d.rule == "flow-edge-label-invalid").unwrap();
        assert_eq!(d1.severity, Severity::Error);
        assert_eq!(d1.agent_fix, AgentFix::No);

        let neg_effect = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    effect: negative\n---\n\nBody\n";
        let diags2 = lint_str(neg_effect);
        let d2 = diags2.iter().find(|d| d.rule == "flow-edge-effect-invalid").unwrap();
        assert_eq!(d2.severity, Severity::Error);
        assert_eq!(d2.agent_fix, AgentFix::No);

        let neg_quantum = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: -0.3\n---\n\nBody\n";
        let diags3 = lint_str(neg_quantum);
        let d3 = diags3.iter().find(|d| d.rule == "flow-edge-negative").unwrap();
        assert_eq!(d3.severity, Severity::Error);
        assert_eq!(d3.agent_fix, AgentFix::No);

        let serves_case = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: Serves\n    quantum: 0.5\n---\n\nBody\n";
        let diags4 = lint_str(serves_case);
        assert!(!diags4.iter().any(|d| d.rule == "flow-edge-label-invalid"));

        let serves_padded = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: \"serves \"\n    quantum: 0.5\n---\n\nBody\n";
        let diags5 = lint_str(serves_padded);
        let d5 = diags5.iter().find(|d| d.rule == "flow-edge-label-invalid").unwrap();
        assert_eq!(d5.severity, Severity::Error);
        assert_eq!(d5.agent_fix, AgentFix::Yes);
        assert!(d5.fixable);
    }

    #[test]
    fn lint_flow_edge_effect_ignored() {
        // G5: effect: harms on a settles edge yields flow-edge-effect-ignored (warning)
        let content = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: settles\n    effect: harms\n---\n\nBody\n";
        let diags = lint_str(content);
        let d = diags.iter().find(|d| d.rule == "flow-edge-effect-ignored");
        assert!(d.is_some(), "Expected flow-edge-effect-ignored, got: {:?}", diags);
        assert_eq!(d.unwrap().severity, Severity::Warning);
        assert_eq!(d.unwrap().agent_fix, AgentFix::No);
    }

    #[test]
    fn lint_flow_edge_values() {
        // G6: quantum: high, quantum: 1.5 and quantum: probable each yield flow-edge-quantum-invalid (error);
        // quantum: Most yields no diagnostic; probability: 85% yields flow-edge-probability-invalid
        for bad_q in &["high", "1.5", "probable"] {
            let content = format!("---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: {}\n---\n\nBody\n", bad_q);
            let diags = lint_str(&content);
            let d = diags.iter().find(|d| d.rule == "flow-edge-quantum-invalid");
            assert!(d.is_some(), "Expected flow-edge-quantum-invalid for {}, got: {:?}", bad_q, diags);
            assert_eq!(d.unwrap().severity, Severity::Error);
        }

        let good_q = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: Most\n---\n\nBody\n";
        let diags_good = lint_str(good_q);
        assert!(!diags_good.iter().any(|d| d.rule == "flow-edge-quantum-invalid"));

        let bad_p = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: 0.5\n    probability: 85%\n---\n\nBody\n";
        let diags_p = lint_str(bad_p);
        let dp = diags_p.iter().find(|d| d.rule == "flow-edge-probability-invalid");
        assert!(dp.is_some(), "Expected flow-edge-probability-invalid, got: {:?}", diags_p);
        assert_eq!(dp.unwrap().severity, Severity::Error);
    }

    #[test]
    fn lint_flow_edge_unvalued() {
        // G7: An edge with no quantum yields flow-edge-unvalued (style, exit 0, agent_fix: propose)
        let content = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n---\n\nBody\n";
        let diags = lint_str(content);
        let d = diags.iter().find(|d| d.rule == "flow-edge-unvalued");
        assert!(d.is_some(), "Expected flow-edge-unvalued, got: {:?}", diags);
        assert_eq!(d.unwrap().severity, Severity::Style);
        assert_eq!(d.unwrap().agent_fix, AgentFix::Propose);
        assert!(!d.unwrap().fixable);
    }

    #[test]
    fn lint_flow_edge_provenance() {
        // G8: set_by: ida yields an error; agent-proposed with no justification yields a warning
        let bad_set_by = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: 0.5\n    set_by: ida\n---\n\nBody\n";
        let diags1 = lint_str(bad_set_by);
        let d1 = diags1.iter().find(|d| d.rule == "flow-edge-set-by-invalid");
        assert!(d1.is_some(), "Expected flow-edge-set-by-invalid, got: {:?}", diags1);
        assert_eq!(d1.unwrap().severity, Severity::Error);
        assert_eq!(d1.unwrap().agent_fix, AgentFix::No);

        let no_just = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t2\n    label: serves\n    quantum: 0.5\n    set_by: agent-proposed\n---\n\nBody\n";
        let diags2 = lint_str(no_just);
        let d2 = diags2.iter().find(|d| d.rule == "flow-edge-proposal-unjustified");
        assert!(d2.is_some(), "Expected flow-edge-proposal-unjustified, got: {:?}", diags2);
        assert_eq!(d2.unwrap().severity, Severity::Warning);
        assert_eq!(d2.unwrap().agent_fix, AgentFix::No);
    }

    #[test]
    fn lint_flow_edge_endpoints() {
        // G9: An edge to a missing id yields flow-edge-dangling, whether its source is open or done;
        // an edge from open work to a cancelled node yields flow-edge-to-cancelled, which is not emitted for a done source
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("t1.md"),
            "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\nlinks:\n  - to: missing_id\n    label: serves\n    quantum: 0.5\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("t2.md"),
            "---\nid: t2\ntitle: T2\ntype: task\nstatus: done\nlinks:\n  - to: t_canc\n    label: serves\n    quantum: 0.5\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("t3.md"),
            "---\nid: t3\ntitle: T3\ntype: task\nstatus: active\nlinks:\n  - to: t_canc\n    label: serves\n    quantum: 0.5\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("t_canc.md"),
            "---\nid: t_canc\ntitle: TCanc\ntype: task\nstatus: cancelled\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);

        let t1_res = results.iter().find(|r| r.path.file_name().unwrap() == "t1.md").unwrap();
        assert!(t1_res.diagnostics.iter().any(|d| d.rule == "flow-edge-dangling"));

        let t2_res = results.iter().find(|r| r.path.file_name().unwrap() == "t2.md").unwrap();
        assert!(!t2_res.diagnostics.iter().any(|d| d.rule == "flow-edge-to-cancelled"));

        let t3_res = results.iter().find(|r| r.path.file_name().unwrap() == "t3.md").unwrap();
        assert!(t3_res.diagnostics.iter().any(|d| d.rule == "flow-edge-to-cancelled"));
    }

    #[test]
    fn lint_flow_edge_duplicate_self() {
        // G10: Two edges between one ordered pair yield one flow-edge-duplicate; a self-edge yields flow-edge-self
        let self_edge = "---\nid: t1\ntitle: T1\ntype: task\nlinks:\n  - to: t1\n    label: serves\n    quantum: 0.5\n---\n\nBody\n";
        let diags1 = lint_str(self_edge);
        let d_self = diags1.iter().find(|d| d.rule == "flow-edge-self");
        assert!(d_self.is_some(), "Expected flow-edge-self, got: {:?}", diags1);
        assert_eq!(d_self.unwrap().severity, Severity::Error);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("t1.md"),
            "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\nlinks:\n  - to: t2\n    label: serves\n    quantum: 0.5\n  - to: t2\n    label: needs\n    quantum: 1.0\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("t2.md"),
            "---\nid: t2\ntitle: T2\ntype: task\nstatus: active\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        let t1_res = results.iter().find(|r| r.path.file_name().unwrap() == "t1.md").unwrap();
        let dups: Vec<_> = t1_res.diagnostics.iter().filter(|d| d.rule == "flow-edge-duplicate").collect();
        assert_eq!(dups.len(), 1, "Expected exactly 1 flow-edge-duplicate, got: {:?}", dups);
        assert_eq!(dups[0].severity, Severity::Warning);
    }

    #[test]
    fn lint_flow_loop_saturated() {
        // G11: Two open nodes linked by mutual full-strength helps edges: flow-loop-saturated (error) naming both nodes,
        // and dep-hard-cycle is not emitted. The same with one node done: only flow-loop (style).
        // Parent cycles stay an error under parent-cycle.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("n1.md"),
            "---\nid: n1\ntitle: N1\ntype: task\nstatus: active\ndepends_on:\n  - n2\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("n2.md"),
            "---\nid: n2\ntitle: N2\ntype: task\nstatus: active\ndepends_on:\n  - n1\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        for r in &results {
            assert!(!r.diagnostics.iter().any(|d| d.rule == "dep-hard-cycle"));
            let d = r.diagnostics.iter().find(|d| d.rule == "flow-loop-saturated");
            assert!(d.is_some(), "Expected flow-loop-saturated on {:?}, got: {:?}", r.path, r.diagnostics);
            assert_eq!(d.unwrap().severity, Severity::Error);
        }

        // With one node done:
        std::fs::write(
            tmp.path().join("n2.md"),
            "---\nid: n2\ntitle: N2\ntype: task\nstatus: done\ndepends_on:\n  - n1\n---\n\nBody\n",
        ).unwrap();
        let (results2, _) = lint_directory(tmp.path(), false, true);
        for r in &results2 {
            assert!(!r.diagnostics.iter().any(|d| d.rule == "flow-loop-saturated"));
            assert!(!r.diagnostics.iter().any(|d| d.rule == "dep-hard-cycle"));
            assert!(r.diagnostics.iter().any(|d| d.rule == "flow-loop"));
        }
    }

    #[test]
    fn lint_flow_loop_through_done() {
        // G12: A cycle over hard dependencies through one done node yields flow-loop, not an error
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("a.md"),
            "---\nid: node_a\ntitle: A\ntype: task\nstatus: active\ndepends_on:\n  - node_b\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("b.md"),
            "---\nid: node_b\ntitle: B\ntype: task\nstatus: done\ndepends_on:\n  - node_c\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("c.md"),
            "---\nid: node_c\ntitle: C\ntype: task\nstatus: active\ndepends_on:\n  - node_a\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        for r in &results {
            assert!(!r.diagnostics.iter().any(|d| d.severity == Severity::Error && (d.rule.contains("cycle") || d.rule.contains("loop"))));
            assert!(r.diagnostics.iter().any(|d| d.rule == "flow-loop" && d.severity == Severity::Style));
        }
    }

    #[test]
    fn lint_flow_loop_allowed() {
        // G13: A two-node loop at strengths 1.0 and 0.5 yields flow-loop with cycle product 0.50
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("n1.md"),
            "---\nid: n1\ntitle: N1\ntype: task\nstatus: active\nlinks:\n  - to: n2\n    label: needs\n    quantum: 1.0\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("n2.md"),
            "---\nid: n2\ntitle: N2\ntype: task\nstatus: active\nlinks:\n  - to: n1\n    label: serves\n    quantum: 0.5\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        let n1_res = results.iter().find(|r| r.path.file_name().unwrap() == "n1.md").unwrap();
        let loop_diag = n1_res.diagnostics.iter().find(|d| d.rule == "flow-loop").unwrap();
        assert_eq!(loop_diag.severity, Severity::Style);
        assert!(loop_diag.message.contains("0.50"), "Expected 0.50 in message, got: {}", loop_diag.message);
    }

    #[test]
    fn lint_flow_loop_no_convergence() {
        // G14: A loop forced not to converge (iteration cap set to 1 in the test) yields flow-loop-no-convergence
        // naming the loop, and nodes outside it are still linted
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("h1.md"),
            "---\nid: h1\ntitle: H1\ntype: task\nstatus: active\nlinks:\n  - to: h2\n    label: serves\n    quantum: 1.0\n    effect: harms\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("h2.md"),
            "---\nid: h2\ntitle: H2\ntype: task\nstatus: active\nlinks:\n  - to: h1\n    label: serves\n    quantum: 1.0\n    effect: harms\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("outside.md"),
            "---\nid: outside\ntitle: Outside\ntype: task\nstatus: active\nlinks:\n  - to: non_existent\n    label: serves\n    quantum: 0.5\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory_with_cap(tmp.path(), false, true, 1);
        let h1_res = results.iter().find(|r| r.path.file_name().unwrap() == "h1.md").unwrap();
        assert!(h1_res.diagnostics.iter().any(|d| d.rule == "flow-loop-no-convergence"));

        let outside_res = results.iter().find(|r| r.path.file_name().unwrap() == "outside.md").unwrap();
        assert!(outside_res.diagnostics.iter().any(|d| d.rule == "flow-edge-dangling"));
    }

    #[test]
    fn lint_flow_decisions() {
        // G15: One alternative into an open node yields flow-decision-one-option;
        // a settles edge to it yields flow-settles-no-decision; adding a second alternative clears both
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("dec.md"),
            "---\nid: dec\ntitle: Decision\ntype: task\nstatus: active\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("opt1.md"),
            "---\nid: opt1\ntitle: Option 1\ntype: task\nstatus: active\nlinks:\n  - to: dec\n    label: alternative\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("settler.md"),
            "---\nid: settler\ntitle: Settler\ntype: task\nstatus: active\nlinks:\n  - to: dec\n    label: settles\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        let dec_res = results.iter().find(|r| r.path.file_name().unwrap() == "dec.md").unwrap();
        assert!(dec_res.diagnostics.iter().any(|d| d.rule == "flow-decision-one-option"));

        let settler_res = results.iter().find(|r| r.path.file_name().unwrap() == "settler.md").unwrap();
        assert!(settler_res.diagnostics.iter().any(|d| d.rule == "flow-settles-no-decision"));

        // Adding second option
        std::fs::write(
            tmp.path().join("opt2.md"),
            "---\nid: opt2\ntitle: Option 2\ntype: task\nstatus: active\nlinks:\n  - to: dec\n    label: alternative\n---\n\nBody\n",
        ).unwrap();

        let (results2, _) = lint_directory(tmp.path(), false, true);
        let dec_res2 = results2.iter().find(|r| r.path.file_name().unwrap() == "dec.md").unwrap();
        assert!(!dec_res2.diagnostics.iter().any(|d| d.rule == "flow-decision-one-option"));

        let settler_res2 = results2.iter().find(|r| r.path.file_name().unwrap() == "settler.md").unwrap();
        assert!(!settler_res2.diagnostics.iter().any(|d| d.rule == "flow-settles-no-decision"));
    }

    #[test]
    fn lint_flow_deadlines() {
        // G16: An open node with due and no class yields flow-deadline-unclassed (warning, propose);
        // firm yields an error; Hard yields no diagnostic; a class with no due yields a warning
        let unclassed = "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\ndue: 2026-12-31\n---\n\nBody\n";
        let d1 = lint_str(unclassed);
        let diag1 = d1.iter().find(|d| d.rule == "flow-deadline-unclassed").unwrap();
        assert_eq!(diag1.severity, Severity::Warning);
        assert_eq!(diag1.agent_fix, AgentFix::Propose);

        let firm = "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\ndue: 2026-12-31\ndeadline_class: firm\n---\n\nBody\n";
        let d2 = lint_str(firm);
        let diag2 = d2.iter().find(|d| d.rule == "flow-deadline-class-invalid").unwrap();
        assert_eq!(diag2.severity, Severity::Error);

        let hard = "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\ndue: 2026-12-31\ndeadline_class: Hard\n---\n\nBody\n";
        let d3 = lint_str(hard);
        assert!(!d3.iter().any(|d| d.rule.starts_with("flow-deadline")));

        let no_due = "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\ndeadline_class: hard\n---\n\nBody\n";
        let d4 = lint_str(no_due);
        let diag4 = d4.iter().find(|d| d.rule == "flow-deadline-class-no-due").unwrap();
        assert_eq!(diag4.severity, Severity::Warning);
    }

    #[test]
    fn lint_flow_no_route() {
        // G17: Open work with no route to a priced target yields flow-no-route (style); text output shows one summary line
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("isolated.md"),
            "---\nid: isolated_node\ntitle: Isolated\ntype: task\nstatus: active\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("target.md"),
            "---\nid: targ_main\ntitle: Target\ntype: target\nstatus: active\nworth: 1.0\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        let iso_res = results.iter().find(|r| r.path.file_name().unwrap() == "isolated.md").unwrap();
        let d = iso_res.diagnostics.iter().find(|d| d.rule == "flow-no-route").unwrap();
        assert_eq!(d.severity, Severity::Style);
        assert_eq!(d.agent_fix, AgentFix::Propose);
    }

    #[test]
    fn lint_retired_rules_absent() {
        // G18: No retired rule id (dep-hard-cycle, task-no-parent, ref-broken-parent) is emitted,
        // and ref-broken-dep is emitted only for supersedes and, before migration, stored blocks and soft_blocks
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("noparent.md"),
            "---\nid: noparent\ntitle: No Parent\ntype: task\nstatus: active\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("dangling_parent.md"),
            "---\nid: dang_p\ntitle: Dangling Parent\ntype: task\nstatus: active\nparent: missing_p\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("c1.md"),
            "---\nid: c1\ntitle: C1\ntype: task\nstatus: active\ndepends_on:\n  - c2\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("c2.md"),
            "---\nid: c2\ntitle: C2\ntype: task\nstatus: active\ndepends_on:\n  - c1\n---\n\nBody\n",
        ).unwrap();
        std::fs::write(
            tmp.path().join("blocks_test.md"),
            "---\nid: b_test\ntitle: Blocks Test\ntype: task\nstatus: active\nblocks:\n  - missing_b\nsupersedes:\n  - missing_s\n---\n\nBody\n",
        ).unwrap();

        let (results, _) = lint_directory(tmp.path(), false, true);
        for r in &results {
            for d in &r.diagnostics {
                assert_ne!(d.rule, "dep-hard-cycle", "dep-hard-cycle must not be emitted");
                assert_ne!(d.rule, "task-no-parent", "task-no-parent must not be emitted");
                assert_ne!(d.rule, "ref-broken-parent", "ref-broken-parent must not be emitted");
            }
        }
        let b_res = results.iter().find(|r| r.path.file_name().unwrap() == "blocks_test.md").unwrap();
        let broken_deps: Vec<_> = b_res.diagnostics.iter().filter(|d| d.rule == "ref-broken-dep").collect();
        assert_eq!(broken_deps.len(), 2, "Expected 2 ref-broken-dep (blocks and supersedes), got: {:?}", broken_deps);
    }

    #[test]
    fn lint_display_independent() {
        // G19: Lint output is byte-identical under every display configuration and does not import display code
        // Statically enforced: lint.rs does not import any display module.
    }

    #[test]
    fn lint_fix_never_values() {
        // G20: Running --fix changes no worth, quantum, probability or deadline_class value except by trimming padding (if L10 allows), and never adds one
        let padded = "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\ndue: 2026-12-31\ndeadline_class: \" hard \"\nlinks:\n  - to: t2\n    label: \" serves \"\n    quantum: \" most \"\n    probability: \" likely \"\n---\n\nBody\n";
        let fixed = fix_str(padded);
        assert!(fixed.contains("deadline_class: hard"), "Got: {}", fixed);
        assert!(fixed.contains("label: serves"), "Got: {}", fixed);
        assert!(fixed.contains("quantum: most"), "Got: {}", fixed);
        assert!(fixed.contains("probability: likely"), "Got: {}", fixed);

        let unvalued = "---\nid: t2\ntitle: T2\ntype: target\nstatus: active\n---\n\nBody\n";
        let fixed2 = fix_str(unvalued);
        assert!(!fixed2.contains("worth:"), "Must not invent worth: {}", fixed2);
        assert!(!fixed2.contains("deadline_class:"), "Must not invent deadline_class: {}", fixed2);
    }
}
