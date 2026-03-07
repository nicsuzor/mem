//! Core graph data structures and PKB document extraction.
//!
//! Provides [`GraphNode`] (extracted from [`PkbDocument`]), edge types,
//! and link resolution helpers for building knowledge graphs over a PKB.

use crate::pkb::PkbDocument;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::DependsOn => "depends_on",
            EdgeType::SoftDependsOn => "soft_depends_on",
            EdgeType::Parent => "parent",
            EdgeType::Link => "link",
            EdgeType::Supersedes => "supersedes",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
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
    pub task_id: Option<String>,
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
    /// Precomputed layout X coordinate (force-directed graph layout)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Precomputed layout Y coordinate (force-directed graph layout)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Treemap/circle width (single-layout export only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    /// Treemap/circle height (single-layout export only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// Circle pack radius (single-layout export only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<f64>,
    /// Named layout coordinates (treemap, circle_pack, arc, etc.)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub layouts: HashMap<String, LayoutPoint>,
}

/// A point in a named layout coordinate system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutPoint {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<f64>,
}

/// An assumption attached to a planning node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Assumption {
    pub text: String,
    #[serde(default = "default_assumption_status")]
    pub status: String,
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

/// Normalize status values for backwards compatibility.
pub fn resolve_status_alias(status: &str) -> &str {
    match status {
        "inbox" | "todo" | "open" => "active",
        "in-progress" => "in_progress",
        "in_review" | "in-review" => "review",
        "complete" | "completed" | "closed" | "archived" | "resolved" => "done",
        "dead" => "cancelled",
        other => other,
    }
}

// ── Canonical status and type values ────────────────────────────────────

/// All recognized canonical status values (post-alias resolution).
///
/// - **active**: default / open / ready to work on
/// - **in_progress**: currently being worked on
/// - **blocked**: waiting on dependencies
/// - **review**: in review / awaiting feedback
/// - **paused**: intentionally deferred
/// - **someday**: low priority / maybe later
/// - **done**: completed successfully
/// - **cancelled**: abandoned / no longer relevant
pub const VALID_STATUSES: &[&str] = &[
    "active", "in_progress", "blocked", "review",
    "paused", "someday",
    "done", "cancelled",
];

/// Statuses that indicate a task is finished (no longer active).
pub const COMPLETED_STATUSES: &[&str] = &["done", "cancelled"];

/// Returns true if the status represents a completed/finished state.
pub fn is_completed(status: Option<&str>) -> bool {
    matches!(status, Some("done") | Some("cancelled"))
}

/// All recognized canonical node type values.
pub const VALID_NODE_TYPES: &[&str] = &[
    "goal", "project", "subproject", "epic",
    "task", "action", "bug", "feature",
    "milestone", "learn",
    "note", "knowledge", "memory", "contact",
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
        let project = fm
            .as_ref()
            .and_then(|f| f.get("project").and_then(|v| v.as_str()).map(String::from));
        let due = fm
            .as_ref()
            .and_then(|f| f.get("due").and_then(|v| v.as_str()).map(String::from));
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
        let complexity = fm.as_ref().and_then(|f| {
            f.get("complexity")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let source = fm.as_ref().and_then(|f| {
            f.get("source")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let confidence = fm
            .as_ref()
            .and_then(|f| f.get("confidence").and_then(|v| v.as_f64()));
        let supersedes = fm.as_ref().and_then(|f| {
            f.get("supersedes")
                .and_then(|v| v.as_str())
                .map(String::from)
        });

        let word_count = doc.body.split_whitespace().count() as i32;

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
            let task_re = Regex::new(r"^([a-z]{1,4}-[a-z0-9]+)-").unwrap();
            if let Some(cap) = task_re.captures(&stem_str) {
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
            depends_on,
            soft_depends_on,
            blocks,
            soft_blocks,
            children,
            project,
            due,
            created,
            modified: doc.modified.clone(),
            assignee,
            complexity,
            source,
            confidence,
            supersedes,
            depth,
            word_count,
            leaf,
            raw_links,
            permalinks,
            task_id,
            downstream_weight: 0.0,
            pagerank: 0.0,
            betweenness: 0.0,
            indegree: 0,
            outdegree: 0,
            backlink_count: 0,
            stakeholder_exposure: false,
            reachable: false,
            assumptions,
            x: None,
            y: None,
            w: None,
            h: None,
            r: None,
            layouts: HashMap::new(),
        }
    }
}
