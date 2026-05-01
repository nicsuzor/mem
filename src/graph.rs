//! Core graph data structures and PKB document extraction.
//!
//! Provides [`GraphNode`] (extracted from [`PkbDocument`]), edge types,
//! and link resolution helpers for building knowledge graphs over a PKB.

use crate::pkb::PkbDocument;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static TASK_ID_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z]{1,10}-[a-z0-9]+)-").unwrap());

// ===========================================================================
// Types
// ===========================================================================

/// Edge types for knowledge graph relationships.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum EdgeType {
    /// Hard dependency (blocking) — solid line
    #[serde(rename = "depends_on")]
    DependsOn,
    /// Soft dependency (informational, non-blocking) — dashed line
    #[serde(rename = "soft_depends_on")]
    SoftDependsOn,
    /// Parent-child hierarchy — thick line
    #[serde(rename = "parent")]
    Parent,
    /// Wiki/markdown link reference — thin line
    #[serde(rename = "link")]
    Link,
    /// Supersedes relationship (this node replaces the target)
    #[serde(rename = "supersedes")]
    Supersedes,
    /// Strategic contribution (importance propagation) — blue line
    #[serde(rename = "contributes_to")]
    ContributesTo,
    /// Semantic similarity relationship (automatically discovered) — gray line
    #[serde(rename = "similar_to")]
    SimilarTo,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::DependsOn => "depends_on",
            EdgeType::SoftDependsOn => "soft_depends_on",
            EdgeType::Parent => "parent",
            EdgeType::Link => "link",
            EdgeType::Supersedes => "supersedes",
            EdgeType::ContributesTo => "contributes_to",
            EdgeType::SimilarTo => "similar_to",
        }
    }
}

/// A directed edge in the knowledge graph.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Edge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: EdgeType,
}

/// A contribution relationship from one node to another (strategic priority).
///
/// Implements the Birnbaum importance model where weights are Renooij-Witteman
/// verbal terms mapped to non-linear anchors.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContributesTo {
    /// Target node ID this node contributes to.
    pub to: String,
    /// Verbal weight term (e.g. "Expected", "Probable", "Certain").
    #[serde(alias = "weight")]
    pub stated_weight: String,
    /// Mandatory single-sentence justification for the weight.
    #[serde(alias = "why")]
    pub justification: String,
    /// Current decayed weight value (computed at runtime).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_weight: Option<f64>,
    /// Resolved target node ID (computed at build time, not serialized).
    #[serde(skip)]
    pub resolved_to: Option<String>,
    /// Optional provenance (ID of prototype this edge inherits from).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    /// Longitudinal calibration history (Brier scores).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brier_history: Vec<f64>,
    /// Last interaction timestamp (feeds decay trigger).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_interacted: Option<String>,
    /// Stated-Revealed Divergence signal.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anomaly_flag: bool,
}

impl ContributesTo {
    /// Map verbal Renooij-Witteman terms to non-linear importance anchors.
    ///
    /// Based on academicOps core calibration for strategic priority:
    /// - Certain: 1.00
    /// - Probable: 0.85
    /// - Expected: 0.75
    /// - Fifty-Fifty: 0.50
    /// - Uncertain: 0.25
    /// - Improbable: 0.15
    /// - Impossible: 0.00
    pub fn numeric_weight(&self) -> f64 {
        match self.stated_weight.to_lowercase().as_str() {
            "certain" | "almost certain" => 1.00,
            "very probable" | "probable" | "highly likely" => 0.85,
            "expected" | "likely" => 0.75,
            "fifty-fifty" | "even chance" => 0.50,
            "uncertain" | "possible" | "perhaps" | "maybe" => 0.25,
            "improbable" | "unlikely" | "very unlikely" | "almost impossible" => 0.15,
            "impossible" | "none" => 0.00,
            _ => 0.3, // default "soft" contribution for unknown terms
        }
    }
}

/// A graph node extracted from a PKB document.
///
/// Contains all metadata needed for graph building, task management,
/// and centrality computation. Constructed via [`GraphNode::from_pkb_document`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphNode {
    pub id: String,
    pub path: PathBuf,
    pub label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub soft_depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub soft_blocks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// Sub-tasks (type=subtask) — travel with the parent and render as checkboxes.
    /// Computed from edges; not stored in frontmatter.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Who is waiting on this task (e.g. "Jacob", "funding-committee").
    /// Drives waiting urgency in focus scoring — the longer since waiting_since, the higher the score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stakeholder: Option<String>,
    /// When the stakeholder started waiting (ISO date). Falls back to `created` if absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<String>,
    /// Computed: label of nearest ancestor with node_type == "project"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub goals: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contributes_to: Vec<ContributesTo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contributed_by: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_up_tasks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
    /// Severity ladder (0-4) for target nodes. SEV4 is lexicographic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<i32>,
    /// Goal classification: committed | aspirational | learning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub depth: i32,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub word_count: i32,
    pub leaf: bool,
    /// Raw wikilinks/md links from body (not serialized — used only during build)
    #[serde(skip)]
    pub raw_links: Vec<String>,
    /// Resolution keys: filename stem, permalink, frontmatter id (not serialized)
    #[serde(skip)]
    pub permalinks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Computed status group: "active", "blocked", or "completed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_group: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub downstream_weight: f64,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub pagerank: f64,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub betweenness: f64,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub indegree: i32,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub outdegree: i32,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub backlink_count: i32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stakeholder_exposure: bool,
    /// True if this node is reachable from an active leaf task via upstream BFS
    /// (parent, depends_on, soft_depends_on edges). Used by renderers to show
    /// only the planning-relevant subgraph.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<Assumption>,
    /// Optional content classification (e.g. "bug", "feature", "action", "milestone").
    /// Display/filter only — does not affect graph behaviour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// True if the body contains acceptance criteria (## Acceptance Criteria, done when, etc.).
    /// Detected during parsing; used as an uncertainty input.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_acceptance_criteria: bool,
    /// Subtree size: recursive count of all descendants via parent-child edges.
    /// Computed during graph build (after inverse relationships are resolved).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub scope: i32,
    /// Residual ambiguity score [0.0–1.0]. Composite of: no acceptance criteria,
    /// unresolved scope (has children), unresolved deps, sparse body, confidence override.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub uncertainty: f64,
    /// Normalized impact score [0.0–1.0]. Derived from downstream_weight, pagerank,
    /// and stakeholder_exposure, normalized across all nodes in the graph.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub criticality: f64,
    /// Computed: urgency signal based on status of blocked tasks.
    /// 1.0 if blocking any in_progress, 0.5 if blocking any active, else 0.0.
    #[serde(skip)]
    pub blocking_urgency: f64,
    /// Computed: min priority across self + full downstream cone (blocks, soft_blocks, children).
    /// Used for filtering/sorting — a P2 blocker of a P0 gets effective_priority=0.
    /// Never written back to frontmatter; skip serialization to avoid polluting YAML.
    #[serde(skip)]
    pub effective_priority: Option<i32>,
    /// Computed: lexicographic urgency propagation.
    /// Formula: Urgency = S_lex * W_edge * f(Slack)
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub urgency: f64,
    /// Edge template for `type: prototype` nodes (spec multi-parent-edges §1.6).
    /// Resolved at edge-creation time per §2.5.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_template: Option<EdgeTemplate>,
    /// Structured parse warnings collected during frontmatter validation.
    /// Surfaced by the linter and `/maintain`; non-fatal at parse time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parse_warnings: Vec<ParseWarning>,
}

/// An assumption attached to a planning node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Assumption {
    pub text: String,
    #[serde(default = "default_assumption_status")]
    pub status: String,
}

/// Edge template for `type: prototype` nodes (spec multi-parent-edges §1.6).
///
/// A prototype is class-like: at edge-creation time, fields are inherited
/// onto the freshly-created edge and may be overridden at the call site.
/// All fields are optional — overrides are applied only when present.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EdgeTemplate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
}

/// A structured frontmatter parse error/warning.
///
/// Returned via [`GraphNode::parse_warnings`] when a known field has the wrong
/// shape (e.g. non-integer severity, unknown goal_type). The node is still
/// constructed with the offending field dropped, so this is non-fatal — but
/// the linter and `/maintain` surface these as actionable parse errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParseWarning {
    /// Frontmatter key that failed validation (e.g. "severity").
    pub field: String,
    /// Human-readable error reason (e.g. "expected integer 0..=4, got string \"high\"").
    pub message: String,
}

fn default_assumption_status() -> String {
    "untested".to_string()
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

// ===========================================================================
// Parsing helpers
// ===========================================================================

/// Generate a new random ID with the given prefix: `{prefix}-{8 random hex chars}`.
///
/// Used when creating new documents. For reading existing documents without an
/// explicit `id` field, use the filename stem as the fallback ID instead.
pub fn create_id(prefix: &str) -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let random: u32 = rng.random();
    format!("{}-{:08x}", prefix, random)
}

/// Derive a fallback ID from a file path (filename stem, no extension).
///
/// Used only when reading documents that lack an explicit `id` in frontmatter.
/// This is stable across re-indexes (same file = same ID) but changes if the
/// file is renamed. Prefer explicit `id` fields — the linter flags missing IDs.
pub fn fallback_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Normalize legacy/alternate status values to the canonical set defined in
/// `aops-core/TAXONOMY.md`. Canonical statuses pass through unchanged.
///
/// Canonical set (11 values): `inbox, ready, queued, in_progress, merge_ready,
/// review, done, blocked, paused, someday, cancelled`.
pub fn resolve_status_alias(status: &str) -> &str {
    match status {
        // Passthrough — canonical values
        "inbox" | "ready" | "queued" | "in_progress" | "merge_ready" | "review"
        | "done" | "blocked" | "paused" | "someday" | "cancelled" => status,

        // Legacy "active" (old taxonomy collapsed ready/queued/in_progress into
        // one label). Map to "ready" — let the auto-computed leaf-with-no-unmet-deps
        // signal decide dispatchability rather than implicitly promoting to queued.
        "active" => "ready",

        // Inbox-family: untriaged capture
        "todo" | "open" | "draft" | "early-scaffold" | "planning" | "seed" | "incoming" => "inbox",

        // In-progress spellings (decomposing = was active work mid-flight)
        "in-progress" | "in-preparation" | "partial" | "decomposing" | "growing" => "in_progress",

        // Review-family: awaiting human or external decision
        "in_review" | "in-review" | "ready-for-review" | "ISSUES_FOUND"
        | "conditionally-accepted" | "revise-and-resubmit"
        | "waiting" | "invited" | "awaiting-approval" | "submitted" => "review",

        // Merge-ready
        "merge-ready" => "merge_ready",

        // Done-family: completed externally or internally
        "complete" | "completed" | "closed" | "archived" | "resolved"
        | "published-spir" | "historical" | "accepted" => "done",

        // Cancelled-family
        "dead" => "cancelled",

        // Paused-family
        "deferred" | "dormant" => "paused",

        // Unknown → passthrough so linter can flag
        other => other,
    }
}

/// Returns true if the status is a valid canonical status.
pub fn is_valid_status(status: &str) -> bool {
    VALID_STATUSES.contains(&status)
}

/// Parse the canonical status set from the vendored `references/TAXONOMY.md`.
///
/// Reads the `## Status Values and Transitions` table and extracts every status
/// token from the first column. Used by the `taxonomy_status_set_in_sync` test
/// to guarantee `VALID_STATUSES` cannot drift from the spec.
///
/// The taxonomy file is embedded into the binary at compile time, so the check
/// runs without filesystem access and the embedded snapshot is what ships.
#[doc(hidden)]
pub fn parse_canonical_statuses_from_taxonomy(md: &str) -> Vec<String> {
    let mut statuses = Vec::new();
    let mut in_status_section = false;
    let mut header_seen = false;
    for line in md.lines() {
        if line.starts_with("## ") {
            in_status_section = line.contains("Status Values and Transitions");
            header_seen = false;
            continue;
        }
        if !in_status_section {
            continue;
        }
        let trimmed = line.trim();
        // Match table rows: `| `token` | ... |`
        if trimmed.starts_with('|') && trimmed.contains('`') {
            // Skip the header separator row `| --- | --- |`
            if trimmed.chars().filter(|&c| c == '-').count() > 3
                && !trimmed.contains('`')
            {
                continue;
            }
            // Skip the header row "| Status | Meaning |" — it has no backticks
            if !header_seen {
                header_seen = true;
            }
            // Extract the first backtick-delimited token in the row
            if let Some(start) = trimmed.find('`') {
                if let Some(end_rel) = trimmed[start + 1..].find('`') {
                    let token = &trimmed[start + 1..start + 1 + end_rel];
                    if !token.is_empty() {
                        statuses.push(token.to_string());
                    }
                }
            }
        } else if header_seen && trimmed.is_empty() {
            // Blank line after the table ends the section
            break;
        }
    }
    statuses
}

#[cfg(test)]
mod taxonomy_sync_tests {
    use super::*;

    /// The vendored snapshot of the canonical taxonomy. Re-sync from
    /// `aops-core/skills/remember/references/TAXONOMY.md` whenever the spec
    /// changes — this test will fail until `VALID_STATUSES` is updated to match.
    const VENDORED_TAXONOMY: &str = include_str!("../references/TAXONOMY.md");

    #[test]
    fn taxonomy_status_set_in_sync() {
        let parsed = parse_canonical_statuses_from_taxonomy(VENDORED_TAXONOMY);
        assert!(
            !parsed.is_empty(),
            "Failed to parse any statuses from references/TAXONOMY.md — \
             check that the '## Status Values and Transitions' table is intact"
        );

        let parsed_set: std::collections::BTreeSet<&str> =
            parsed.iter().map(|s| s.as_str()).collect();
        let code_set: std::collections::BTreeSet<&str> =
            VALID_STATUSES.iter().copied().collect();

        assert_eq!(
            parsed_set, code_set,
            "VALID_STATUSES drifted from references/TAXONOMY.md.\n\
             Spec has: {:?}\n\
             Code has: {:?}\n\
             Either re-sync references/TAXONOMY.md or update VALID_STATUSES \
             in src/graph.rs to match.",
            parsed_set, code_set
        );
    }
}

/// Returns true if the node type is a valid canonical node type.
pub fn is_valid_node_type(node_type: &str) -> bool {
    VALID_NODE_TYPES.contains(&node_type)
}

/// Returns true if the priority is within the valid range [0, 4].
pub fn is_valid_priority(priority: i32) -> bool {
    (0..=4).contains(&priority)
}

/// Returns true if the effort string is a valid duration (e.g., "1d", "2h", "1w", "0.5d").
pub fn is_valid_effort(effort: &str) -> bool {
    parse_effort_days(effort).is_some()
}

/// Returns a rank for statuses to detect backwards transitions.
/// Higher rank = further along in the lifecycle.
pub fn status_rank(status: &str) -> i32 {
    match status {
        "inbox" => 0,
        "ready" => 1,
        "queued" => 2,
        "in_progress" => 3,
        "review" => 4,
        "merge_ready" => 5,
        "done" => 6,
        // Side states are generally ranked low but high enough to not flag everything
        "blocked" | "paused" | "someday" | "cancelled" => -1,
        _ => 0,
    }
}

/// Helper to parse duration strings into days.
///
/// Supports:
/// - 1d = 1
/// - 1w = 7
/// - 2h = ceil(2/8) = 1 (8h workday)
/// - 5 = 5 (bare number = days)
pub fn parse_effort_days(effort: &str) -> Option<i64> {
    let effort = effort.trim().to_lowercase();
    if effort.is_empty() {
        return None;
    }

    if effort.ends_with('w') {
        effort[..effort.len() - 1]
            .parse::<f64>()
            .ok()
            .map(|w| (w * 7.0).ceil() as i64)
    } else if effort.ends_with('d') {
        effort[..effort.len() - 1]
            .parse::<f64>()
            .ok()
            .map(|d| d.ceil() as i64)
    } else if effort.ends_with('h') {
        effort[..effort.len() - 1]
            .parse::<f64>()
            .ok()
            .map(|h| (h / 8.0).ceil() as i64)
    } else {
        effort.parse::<f64>().ok().map(|d| d.ceil() as i64)
    }
}

// ── Canonical status and type values ────────────────────────────────────

/// All recognized canonical status values (post-alias resolution).
/// See `aops-core/TAXONOMY.md` for semantic definitions.
///
/// Lifecycle: `inbox → ready → queued → in_progress → merge_ready → done`
/// with branches to `review`, `blocked`, `paused`, `someday`, `cancelled`.
///
/// - **inbox**: default for new nodes — captured but not triaged
/// - **ready**: decomposed with dependencies resolved (auto-computed)
/// - **queued**: human-gated — available for agent dispatch (manual promotion)
/// - **in_progress**: claimed and actively being worked
/// - **merge_ready**: work complete and committed, awaiting merge
/// - **review**: awaiting human review (mid-flight or post-PR)
/// - **done**: completed successfully
/// - **blocked**: waiting on an unresolved external dependency
/// - **paused**: intentionally stopped mid-flight with intent to resume
/// - **someday**: explicitly deferred idea — differs from inbox by intent
/// - **cancelled**: will not be done
pub const VALID_STATUSES: &[&str] = &[
    "inbox", "ready", "queued", "in_progress", "merge_ready", "review",
    "done", "blocked", "paused", "someday", "cancelled",
];

/// Terminal statuses — no further work expected.
pub const COMPLETED_STATUSES: &[&str] = &["done", "cancelled"];

/// Open work items — everything that is neither terminal nor blocked.
/// Used for surfacing active work in dashboards and filters.
pub const ACTIVE_STATUSES: &[&str] = &[
    "inbox", "ready", "queued", "in_progress", "merge_ready", "review",
    "paused", "someday",
];

/// Statuses that represent blocked work.
pub const BLOCKED_STATUSES: &[&str] = &["blocked"];

/// Returns true if the status represents a completed/finished state.
pub fn is_completed(status: Option<&str>) -> bool {
    matches!(status, Some("done") | Some("cancelled"))
}

/// Returns the coarse status group (`"active"`, `"blocked"`, or `"completed"`)
/// for a given status. Note: the `"active"` group name is a coarse bucket
/// meaning "open work" — it is NOT the retired `active` status value.
pub fn status_group(status: Option<&str>) -> &'static str {
    match status {
        Some(s) if COMPLETED_STATUSES.contains(&s) => "completed",
        Some(s) if BLOCKED_STATUSES.contains(&s) => "blocked",
        _ => "active",
    }
}

/// Node types that represent actionable work items (shown in dashboards).
pub const TASK_TYPES: &[&str] = &["task", "project", "epic", "learn"];

/// All recognized canonical node type values.
///
/// `target` is an accepted alias for `goal` — both represent user-declared strategic
/// priorities. Existing nodes with `type: target` parse correctly; the linter maps
/// `target` → `goal` in auto-fix mode.
pub const VALID_NODE_TYPES: &[&str] = &[
    // Actionable
    "project", "epic", "task", "learn",
    // Reference
    "goal", "target", "note", "knowledge", "memory", "contact",
    "document", "reference", "review", "case", "spec", "prototype",
    // Structural / log
    "index", "daily", "session-log", "audit-report",
];

/// Parse a string array from a JSON frontmatter value.
pub fn parse_string_array(fm: &serde_json::Value, key: &str) -> Vec<String> {
    fm.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract `[[wikilinks]]` and `[md links](target)` from markdown content.
pub fn parse_links(content: &str) -> Vec<String> {
    let mut links = Vec::new();

    // Wiki links: [[target]] or [[target|alias]]
    let wiki_re = Regex::new(r"\[\[([^\]\|]+)(?:\|[^\]]+)?\]\]").unwrap();
    for cap in wiki_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            links.push(m.as_str().trim().to_string());
        }
    }

    // Standard MD links: [label](target) — skip http/https and anchors
    let md_re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    for cap in md_re.captures_iter(content) {
        if let Some(m) = cap.get(2) {
            let link = m.as_str().trim();
            if !link.starts_with("http") && !link.starts_with('#') {
                links.push(link.to_string());
            }
        }
    }

    links
}

/// Resolve a wikilink or relative path to a target absolute path.
pub fn resolve_link(
    link: &str,
    source_path: &Path,
    id_map: &HashMap<String, String>,
) -> Option<String> {
    // Try exact lookup, then lowercase
    if let Some(path) = id_map.get(link) {
        return Some(path.clone());
    }
    if let Some(path) = id_map.get(&link.to_lowercase()) {
        return Some(path.clone());
    }

    // Try relative path resolution
    if let Some(parent) = source_path.parent() {
        let joined = parent.join(link);
        if joined.exists() {
            return Some(joined.canonicalize().ok()?.to_string_lossy().to_string());
        }
    }

    None
}

/// Resolve a frontmatter reference (task ID or filename) to a node ID.
pub fn resolve_ref(
    ref_str: &str,
    id_map: &HashMap<String, String>,
    path_to_id: &HashMap<String, String>,
) -> Option<String> {
    id_map
        .get(&ref_str.to_lowercase())
        .and_then(|path| path_to_id.get(path))
        .cloned()
}

/// Deduplicate a string vector in place, preserving order.
pub fn deduplicate_vec(vec: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    vec.retain(|item| seen.insert(item.clone()));
}

// ===========================================================================
// Acceptance criteria detection
// ===========================================================================

/// Return true if the body contains signals indicating acceptance criteria are specified.
///
/// Checks for common patterns: "Acceptance Criteria" headers, "done when" clauses,
/// and "definition of done" sections.
pub fn detect_acceptance_criteria(body: &str) -> bool {
    static AC_REGEX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(acceptance criteria|done when|definition of done|## ac\r?\n)").unwrap()
    });
    AC_REGEX.is_match(body)
}

// ===========================================================================
// GraphNode construction
// ===========================================================================

impl GraphNode {
    /// Extract graph-relevant fields from a `PkbDocument`.
    ///
    /// Uses the frontmatter JSON for structured fields and parses the body
    /// for wikilinks/markdown links. No file I/O is performed.
    pub fn from_pkb_document(doc: &PkbDocument) -> Self {
        let fm = &doc.frontmatter;

        let task_id = fm
            .as_ref()
            .and_then(|f| f.get("id").and_then(|v| v.as_str()).map(String::from));
        let id = task_id.clone().unwrap_or_else(|| fallback_id(&doc.path));

        let node_type = fm
            .as_ref()
            .and_then(|f| f.get("type").and_then(|v| v.as_str()).map(String::from));
        let status = fm.as_ref().and_then(|f| {
            f.get("status")
                .and_then(|v| v.as_str())
                .map(|s| resolve_status_alias(s).to_string())
        });
        let priority = fm
            .as_ref()
            .and_then(|f| f.get("priority").and_then(|v| v.as_i64()).map(|v| v as i32));
        let order = fm
            .as_ref()
            .and_then(|f| f.get("order").and_then(|v| v.as_i64()).map(|v| v as i32))
            .unwrap_or(0);
        let parent = fm
            .as_ref()
            .and_then(|f| f.get("parent").and_then(|v| v.as_str()).map(String::from));
        let due = fm
            .as_ref()
            .and_then(|f| f.get("due").and_then(|v| v.as_str()).map(String::from));
        let complexity = fm
            .as_ref()
            .and_then(|f| f.get("complexity").and_then(|v| v.as_str()).map(String::from));
        let effort = fm
            .as_ref()
            .and_then(|f| f.get("effort").and_then(|v| v.as_str()).map(String::from));
        let consequence = fm
            .as_ref()
            .and_then(|f| f.get("consequence").and_then(|v| v.as_str()).map(String::from));
        let mut parse_warnings: Vec<ParseWarning> = Vec::new();

        // Severity: integer 0..=4. Anything else is rejected with a structured warning.
        let severity = match fm.as_ref().and_then(|f| f.get("severity")) {
            Some(v) => {
                if let Some(n) = v.as_i64() {
                    if (0..=4).contains(&n) {
                        Some(n as i32)
                    } else {
                        parse_warnings.push(ParseWarning {
                            field: "severity".to_string(),
                            message: format!("severity {n} out of range; expected integer 0..=4"),
                        });
                        None
                    }
                } else if !v.is_null() {
                    parse_warnings.push(ParseWarning {
                        field: "severity".to_string(),
                        message: format!(
                            "severity must be an integer 0..=4; got {}",
                            v
                        ),
                    });
                    None
                } else {
                    None
                }
            }
            None => None,
        };

        // goal_type: enum { committed, aspirational, learning }.
        let goal_type = match fm.as_ref().and_then(|f| f.get("goal_type")) {
            Some(v) => {
                if let Some(s) = v.as_str() {
                    if matches!(s, "committed" | "aspirational" | "learning") {
                        Some(s.to_string())
                    } else {
                        parse_warnings.push(ParseWarning {
                            field: "goal_type".to_string(),
                            message: format!(
                                "goal_type \"{s}\" not in {{committed, aspirational, learning}}"
                            ),
                        });
                        None
                    }
                } else if !v.is_null() {
                    parse_warnings.push(ParseWarning {
                        field: "goal_type".to_string(),
                        message: format!("goal_type must be a string; got {v}"),
                    });
                    None
                } else {
                    None
                }
            }
            None => None,
        };
        // edge_template: nested object on `type: prototype` nodes (spec §1.6).
        // Each sub-field is validated with the same rules as the corresponding
        // top-level field on a target node.
        let edge_template = fm.as_ref().and_then(|f| f.get("edge_template")).and_then(|v| {
            if !v.is_object() {
                if !v.is_null() {
                    parse_warnings.push(ParseWarning {
                        field: "edge_template".to_string(),
                        message: format!("edge_template must be a mapping; got {v}"),
                    });
                }
                return None;
            }
            let tmpl_severity = match v.get("severity") {
                Some(sv) => {
                    if let Some(n) = sv.as_i64() {
                        if (0..=4).contains(&n) {
                            Some(n as i32)
                        } else {
                            parse_warnings.push(ParseWarning {
                                field: "edge_template.severity".to_string(),
                                message: format!(
                                    "edge_template.severity {n} out of range; expected integer 0..=4"
                                ),
                            });
                            None
                        }
                    } else if !sv.is_null() {
                        parse_warnings.push(ParseWarning {
                            field: "edge_template.severity".to_string(),
                            message: format!(
                                "edge_template.severity must be an integer 0..=4; got {sv}"
                            ),
                        });
                        None
                    } else {
                        None
                    }
                }
                None => None,
            };
            let tmpl_goal_type = match v.get("goal_type") {
                Some(gv) => {
                    if let Some(s) = gv.as_str() {
                        if matches!(s, "committed" | "aspirational" | "learning") {
                            Some(s.to_string())
                        } else {
                            parse_warnings.push(ParseWarning {
                                field: "edge_template.goal_type".to_string(),
                                message: format!(
                                    "edge_template.goal_type \"{s}\" not in {{committed, aspirational, learning}}"
                                ),
                            });
                            None
                        }
                    } else if !gv.is_null() {
                        parse_warnings.push(ParseWarning {
                            field: "edge_template.goal_type".to_string(),
                            message: format!("edge_template.goal_type must be a string; got {gv}"),
                        });
                        None
                    } else {
                        None
                    }
                }
                None => None,
            };
            let tmpl_weight = v
                .get("weight")
                .and_then(|wv| wv.as_str().map(String::from));
            let tmpl_consequence = v
                .get("consequence")
                .and_then(|cv| cv.as_str().map(String::from));
            // Only return Some if at least one field is present.
            if tmpl_severity.is_none()
                && tmpl_goal_type.is_none()
                && tmpl_weight.is_none()
                && tmpl_consequence.is_none()
            {
                None
            } else {
                Some(EdgeTemplate {
                    severity: tmpl_severity,
                    goal_type: tmpl_goal_type,
                    weight: tmpl_weight,
                    consequence: tmpl_consequence,
                })
            }
        });

        let goals = fm
            .as_ref()
            .map(|f| parse_string_array(f, "goals"))
            .unwrap_or_default();
        let contributes_to = fm
            .as_ref()
            .and_then(|f| f.get("contributes_to"))
            .and_then(|v| serde_json::from_value::<Vec<ContributesTo>>(v.clone()).ok())
            .unwrap_or_default();
        let follow_up_tasks = fm
            .as_ref()
            .map(|f| parse_string_array(f, "follow_up_tasks"))
            .unwrap_or_default();
        let source = fm
            .as_ref()
            .and_then(|f| f.get("source").and_then(|v| v.as_str()).map(String::from));
        let created = fm
            .as_ref()
            .and_then(|f| f.get("created").and_then(|v| v.as_str()).map(String::from));
        let depth = fm
            .as_ref()
            .and_then(|f| f.get("depth").and_then(|v| v.as_i64()).map(|v| v as i32))
            .unwrap_or(0);
        let leaf = fm
            .as_ref()
            .and_then(|f| f.get("leaf").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        let assignee = fm
            .as_ref()
            .and_then(|f| f.get("assignee").and_then(|v| v.as_str()).map(String::from));
        let stakeholder = fm
            .as_ref()
            .and_then(|f| f.get("stakeholder").and_then(|v| v.as_str()).map(String::from));
        let waiting_since = fm
            .as_ref()
            .and_then(|f| f.get("waiting_since").and_then(|v| v.as_str()).map(String::from));
        let confidence = fm
            .as_ref()
            .and_then(|f| f.get("confidence").and_then(|v| v.as_f64()));
        let supersedes = fm.as_ref().and_then(|f| {
            f.get("supersedes")
                .and_then(|v| v.as_str())
                .map(String::from)
        });

        let word_count = doc.body.split_whitespace().count() as i32;
        let has_acceptance_criteria = detect_acceptance_criteria(&doc.body);

        let (depends_on, soft_depends_on, children, blocks, soft_blocks) = match fm {
            Some(f) => (
                parse_string_array(f, "depends_on"),
                parse_string_array(f, "soft_depends_on"),
                parse_string_array(f, "children"),
                parse_string_array(f, "blocks"),
                parse_string_array(f, "soft_blocks"),
            ),
            None => (vec![], vec![], vec![], vec![], vec![]),
        };

        // Build permalinks for link resolution
        let mut permalinks = Vec::new();
        if let Some(stem) = doc.path.file_stem() {
            permalinks.push(stem.to_string_lossy().to_lowercase());
        }
        if let Some(ref f) = fm {
            if let Some(pl) = f.get("permalink").and_then(|v| v.as_str()) {
                permalinks.push(pl.trim().to_lowercase());
            }
            if let Some(fid) = f.get("id").and_then(|v| v.as_str()) {
                permalinks.push(fid.trim().to_lowercase());
            }
        }
        // Task ID prefix pattern (e.g. "aops-123" from "aops-123-do-something.md")
        if let Some(stem) = doc.path.file_stem() {
            let stem_str = stem.to_string_lossy();
            if let Some(cap) = TASK_ID_PREFIX_RE.captures(&stem_str) {
                if let Some(m) = cap.get(1) {
                    permalinks.push(m.as_str().to_lowercase());
                }
            }
        }

        // Extract links from body content
        let raw_links = parse_links(&doc.body);

        // Parse assumptions from frontmatter
        let assumptions = fm
            .as_ref()
            .and_then(|f| f.get("assumptions"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        if let Some(text) = item.as_str() {
                            // Simple string: "some assumption"
                            Some(Assumption {
                                text: text.to_string(),
                                status: "untested".to_string(),
                            })
                        } else if let Some(obj) = item.as_object() {
                            // Object: { text: "...", status: "..." }
                            let text = obj
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let status = obj
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("untested")
                                .to_string();
                            if text.is_empty() {
                                None
                            } else {
                                Some(Assumption { text, status })
                            }
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let classification = fm.as_ref().and_then(|f| {
            f.get("classification")
                .and_then(|v| v.as_str())
                .map(String::from)
        });

        let sg = status.as_deref().map(|s| status_group(Some(s)).to_string());

        GraphNode {
            id,
            path: doc.path.clone(),
            label: doc.title.clone(),
            tags: doc.tags.clone(),
            node_type,
            status,
            priority,
            order,
            parent,
            contributes_to,
            contributed_by: Vec::new(),
            follow_up_tasks,
            depends_on,
            soft_depends_on,
            blocks,
            soft_blocks,
            children,
            subtasks: Vec::new(),
            due,
            created,
            modified: doc.modified.clone(),
            assignee,
            stakeholder,
            waiting_since,
            project: None,
            goals,
            complexity,
            effort,
            consequence,
            severity,
            goal_type,
            source,
            confidence,
            supersedes,
            depth,
            word_count,
            leaf,
            raw_links,
            permalinks,
            status_group: sg,
            task_id,
            downstream_weight: 0.0,
            blocking_urgency: 0.0,
            pagerank: 0.0,
            betweenness: 0.0,
            indegree: 0,
            outdegree: 0,
            backlink_count: 0,
            stakeholder_exposure: false,
            reachable: false,
            assumptions,
            focus_score: None,
            classification,
            has_acceptance_criteria,
            scope: 0,
            uncertainty: 0.0,
            criticality: 0.0,
            effective_priority: None,
            urgency: 0.0,
            edge_template,
            parse_warnings,
        }
    }
}

#[cfg(test)]
mod target_prototype_tests {
    //! Tests for spec multi-parent-edges §1.1 (target nodes) and §1.6 (prototype nodes).
    use super::*;
    use crate::pkb::PkbDocument;
    use serde_json::json;

    fn doc_with_fm(fm: serde_json::Value) -> PkbDocument {
        PkbDocument {
            path: PathBuf::from("/tmp/fixture.md"),
            title: "Fixture".to_string(),
            tags: vec![],
            doc_type: fm
                .get("type")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: None,
            modified: None,
            body: "Body text.".to_string(),
            content_hash: "h".to_string(),
            file_hash: "h".to_string(),
            frontmatter: Some(fm),
        }
    }

    #[test]
    fn parses_sev4_target_node() {
        let fm = json!({
            "id": "target-01",
            "type": "target",
            "severity": 4,
            "due": "2026-12-31",
            "consequence": "Lab loses funding.",
            "goal_type": "committed",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.node_type.as_deref(), Some("target"));
        assert_eq!(n.severity, Some(4));
        assert_eq!(n.goal_type.as_deref(), Some("committed"));
        assert_eq!(n.consequence.as_deref(), Some("Lab loses funding."));
        assert_eq!(n.due.as_deref(), Some("2026-12-31"));
        assert!(n.parse_warnings.is_empty());
        assert!(n.edge_template.is_none());
    }

    #[test]
    fn parses_sev3_target_node() {
        let fm = json!({
            "id": "target-02",
            "type": "target",
            "severity": 3,
            "due": "2026-09-01",
            "consequence": "Reviewer 2 escalates.",
            "goal_type": "committed",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.severity, Some(3));
        assert_eq!(n.goal_type.as_deref(), Some("committed"));
        assert!(n.parse_warnings.is_empty());
    }

    #[test]
    fn parses_sev2_aspirational_target() {
        let fm = json!({
            "id": "target-03",
            "type": "target",
            "severity": 2,
            "consequence": "Misses internal stretch goal.",
            "goal_type": "aspirational",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.severity, Some(2));
        assert_eq!(n.goal_type.as_deref(), Some("aspirational"));
        // Aspirational + missing due is allowed at parser level (no LST slack required).
        assert!(n.due.is_none());
        assert!(n.parse_warnings.is_empty());
    }

    #[test]
    fn parses_prototype_with_edge_template() {
        let fm = json!({
            "id": "proto-01",
            "type": "prototype",
            "edge_template": {
                "severity": 3,
                "goal_type": "committed",
                "weight": "Probable",
                "consequence": "Inherited consequence prose.",
            },
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.node_type.as_deref(), Some("prototype"));
        let tmpl = n.edge_template.expect("edge_template should parse");
        assert_eq!(tmpl.severity, Some(3));
        assert_eq!(tmpl.goal_type.as_deref(), Some("committed"));
        assert_eq!(tmpl.weight.as_deref(), Some("Probable"));
        assert_eq!(tmpl.consequence.as_deref(), Some("Inherited consequence prose."));
        // Prototype is class-like — no `due` at the node level.
        assert!(n.due.is_none());
        assert!(n.parse_warnings.is_empty());
    }

    #[test]
    fn rejects_severity_out_of_range() {
        let fm = json!({
            "id": "target-bad",
            "type": "target",
            "severity": 7,
            "consequence": "x",
            "goal_type": "committed",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.severity, None);
        assert_eq!(n.parse_warnings.len(), 1);
        assert_eq!(n.parse_warnings[0].field, "severity");
        assert!(n.parse_warnings[0].message.contains("out of range"));
    }

    #[test]
    fn rejects_severity_non_integer() {
        let fm = json!({
            "id": "target-bad2",
            "type": "target",
            "severity": "high",
            "goal_type": "committed",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.severity, None);
        assert_eq!(n.parse_warnings.len(), 1);
        assert_eq!(n.parse_warnings[0].field, "severity");
    }

    #[test]
    fn rejects_unknown_goal_type() {
        let fm = json!({
            "id": "target-bad3",
            "type": "target",
            "severity": 2,
            "goal_type": "wishful",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.goal_type, None);
        assert_eq!(n.parse_warnings.len(), 1);
        assert_eq!(n.parse_warnings[0].field, "goal_type");
        assert!(n.parse_warnings[0].message.contains("wishful"));
    }

    #[test]
    fn missing_consequence_is_warning_not_block() {
        // Parser does NOT enforce consequence presence — that's /maintain's job.
        let fm = json!({
            "id": "target-no-cons",
            "type": "target",
            "severity": 1,
            "goal_type": "learning",
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        assert_eq!(n.severity, Some(1));
        assert!(n.consequence.is_none());
        assert!(n.parse_warnings.is_empty());
    }

    #[test]
    fn edge_template_with_invalid_severity_warns() {
        let fm = json!({
            "id": "proto-bad",
            "type": "prototype",
            "edge_template": {
                "severity": 99,
                "goal_type": "committed",
            },
        });
        let n = GraphNode::from_pkb_document(&doc_with_fm(fm));
        let tmpl = n.edge_template.expect("partial template still returns Some");
        assert_eq!(tmpl.severity, None);
        assert_eq!(tmpl.goal_type.as_deref(), Some("committed"));
        assert!(n
            .parse_warnings
            .iter()
            .any(|w| w.field == "edge_template.severity"));
    }
}
