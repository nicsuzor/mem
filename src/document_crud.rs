//! Document CRUD — create, update, and delete markdown files with YAML frontmatter.
//!
//! Supports task, memory, and general document types. Each type has its own
//! frontmatter conventions and subdirectory routing but shares the same
//! underlying file operations.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Document type determines subdirectory, ID prefix, and default frontmatter.
#[derive(Debug, Clone, Copy)]
pub enum DocType {
    Task,
    Memory,
}

impl DocType {
    pub fn subdir(&self) -> &str {
        match self {
            DocType::Task => "tasks",
            DocType::Memory => "memories",
        }
    }

    pub fn id_prefix(&self) -> &str {
        match self {
            DocType::Task => "task",
            DocType::Memory => "mem",
        }
    }

    pub fn type_str(&self) -> &str {
        match self {
            DocType::Task => "task",
            DocType::Memory => "memory",
        }
    }
}

/// General-purpose fields for creating any document type.
#[derive(Debug, Clone, Default)]
pub struct DocumentFields {
    pub title: String,
    pub doc_type: String,
    pub id: Option<String>,
    pub tags: Vec<String>,
    pub body: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
    pub assignee: Option<String>,
    pub complexity: Option<String>,
    pub source: Option<String>,
    pub due: Option<String>,
    pub confidence: Option<f64>,
    pub supersedes: Option<String>,
    pub severity: Option<i32>,
    pub goal_type: Option<String>,
    pub stakeholder: Option<String>,
    pub waiting_since: Option<String>,
    pub contributes_to: Vec<serde_json::Value>,
    /// Override subdirectory placement (e.g. "notes", "projects")
    pub dir: Option<String>,
}

/// Fields for creating a new task.
#[derive(Debug, Clone, Default)]
pub struct TaskFields {
    pub title: String,
    pub id: Option<String>,
    pub parent: Option<String>,
    pub priority: Option<i32>,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    pub assignee: Option<String>,
    pub complexity: Option<String>,
    pub effort: Option<String>,
    pub consequence: Option<String>,
    pub severity: Option<i32>,
    pub goal_type: Option<String>,
    pub body: Option<String>,
    pub stakeholder: Option<String>,
    pub waiting_since: Option<String>,
    pub due: Option<String>,
    pub project: Option<String>,
    pub task_type: Option<String>,
    pub status: Option<String>,
    pub session_id: Option<String>,
    pub issue_url: Option<String>,
    pub follow_up_tasks: Vec<String>,
    pub release_summary: Option<String>,
    pub contributes_to: Vec<serde_json::Value>,
}

/// Fields for creating a new memory.
#[derive(Debug, Clone, Default)]
pub struct MemoryFields {
    pub title: String,
    pub id: Option<String>,
    pub tags: Vec<String>,
    pub body: Option<String>,
    /// Memory subtype: "memory", "note", "insight", "observation"
    pub memory_type: Option<String>,
    /// Source context (e.g. session ID)
    pub source: Option<String>,
    pub confidence: Option<f64>,
    pub supersedes: Option<String>,
}

/// Create a new document file with YAML frontmatter.
///
/// General-purpose document creation with enforced metadata:
/// - `id` — auto: `{type_prefix}-{md5[..8]}`
/// - `title`, `type` — from input
/// - `created`, `modified` — auto UTC ISO-8601
/// - `alias` — auto: `["{id}-{slug}", "{id}"]`
/// - `permalink` — auto: `{id}`
///
/// Subdirectory routing (overridden by `dir` field):
/// - `task|bug|epic|feature` → `tasks/`
/// - `project` → `projects/`
/// - `goal` → `goals/`
/// - Everything else → `notes/`
pub fn create_document(root: &Path, fields: DocumentFields) -> Result<PathBuf> {
    if let Some(c) = fields.confidence {
        if !(0.0..=1.0).contains(&c) {
            anyhow::bail!("confidence must be between 0.0 and 1.0, got {}", c);
        }
    }

    // Validation
    if !crate::graph::is_valid_node_type(&fields.doc_type) {
        anyhow::bail!("Invalid node type: {}", fields.doc_type);
    }
    if let Some(ref status) = fields.status {
        let is_task = crate::graph::TASK_TYPES.contains(&fields.doc_type.as_str());
        if is_task && !crate::graph::is_valid_status(status) {
            anyhow::bail!("Invalid status for task type: {}", status);
        }
    }
    if let Some(priority) = fields.priority {
        if !crate::graph::is_valid_priority(priority) {
            anyhow::bail!("Invalid priority: {}. Must be between 0 and 4.", priority);
        }
    }

    let type_prefix = match fields.doc_type.as_str() {
        "task" | "epic" => "task",
        "project" => "proj",
        "memory" => "mem",
        "note" => "note",
        "knowledge" => "kb",
        "insight" => "ins",
        "observation" => "obs",
        other => &other[..other.len().min(4)],
    };

    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: sanitize to prevent path traversal
            let safe_id = sanitize_prefix(&explicit_id);
            let filename = format!("{}.md", safe_id);
            (safe_id, filename)
        }
        None => {
            // Use project as prefix when available, otherwise type-based prefix
            let prefix = type_prefix;
            let id = generate_id(prefix);
            let slug = slugify(&fields.title);
            let filename = format!("{}-{}.md", id, slug);
            (id, filename)
        }
    };

    // Determine subdirectory
    let subdir = fields
        .dir
        .map(|d| expand_env_vars(&d))
        .unwrap_or_else(|| match fields.doc_type.as_str() {
            "task" | "epic" | "learn" => "tasks".to_string(),
            "project" => "projects".to_string(),
            "memory" => "memories".to_string(),
            _ => "notes".to_string(),
        });

    let dir = root.join(&subdir);
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    let path = dir.join(&filename);

    if path.exists() {
        anyhow::bail!("File already exists: {}", path.display());
    }

    let now = chrono::Utc::now().to_rfc3339();

    // Build YAML frontmatter
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", id));
    fm.push_str(&format!(
        "title: \"{}\"\n",
        yaml_escape_double_quoted(&fields.title)
    ));
    fm.push_str(&format!("type: {}\n", fields.doc_type));
    fm.push_str(&format!("created: {}\n", now));
    fm.push_str(&format!("modified: {}\n", now));

    // Alias and permalink
    let slug = slugify(&fields.title);
    fm.push_str("alias:\n");
    fm.push_str(&format!("  - \"{}-{}\"\n", id, slug));
    fm.push_str(&format!("  - \"{}\"\n", id));
    fm.push_str(&format!("permalink: {}\n", id));

    if let Some(s) = &fields.status {
        fm.push_str(&format!("status: {}\n", s));
    }

    if let Some(p) = fields.priority {
        fm.push_str(&format!("priority: {}\n", p));
    }

    if let Some(ref parent) = fields.parent {
        fm.push_str(&format!("parent: {}\n", parent));
    }

    if !fields.tags.is_empty() {
        fm.push_str("tags:\n");
        for tag in &fields.tags {
            fm.push_str(&format!("  - {}\n", tag));
        }
    }

    if !fields.depends_on.is_empty() {
        fm.push_str("depends_on:\n");
        for dep in &fields.depends_on {
            fm.push_str(&format!("  - {}\n", dep));
        }
    }

    if let Some(ref assignee) = fields.assignee {
        fm.push_str(&format!("assignee: {}\n", assignee));
    }

    if let Some(ref complexity) = fields.complexity {
        fm.push_str(&format!("complexity: {}\n", complexity));
    }

    if let Some(ref source) = fields.source {
        fm.push_str(&format!("source: \"{}\"\n", yaml_escape_double_quoted(source)));
    }

    if let Some(c) = fields.confidence {
        fm.push_str(&format!("confidence: {}\n", c));
    }

    if let Some(ref s) = fields.supersedes {
        fm.push_str(&format!("supersedes: \"{}\"\n", yaml_escape_double_quoted(s)));
    }

    if let Some(sev) = fields.severity {
        append_severity_field(&mut fm, sev);
    }

    if let Some(ref gt) = fields.goal_type {
        append_goal_type_field(&mut fm, gt);
    }

    if let Some(ref due) = fields.due {
        fm.push_str(&format!("due: {}\n", due));
    }

    if let Some(ref stakeholder) = fields.stakeholder {
        fm.push_str(&format!(
            "stakeholder: \"{}\"\n",
            yaml_escape_double_quoted(stakeholder)
        ));
    }

    if let Some(ref waiting_since) = fields.waiting_since {
        fm.push_str(&format!("waiting_since: {}\n", waiting_since));
    }

    if !fields.contributes_to.is_empty() {
        let resolved = materialise_edge_inheritance(root, fields.contributes_to);
        if let Ok(yaml) = serde_yaml::to_string(&resolved) {
            fm.push_str("contributes_to:\n");
            for line in yaml.trim_start_matches("---\n").lines() {
                if !line.is_empty() {
                    fm.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    fm.push_str("---\n\n");

    let body = fields
        .body
        .unwrap_or_else(|| format!("# {}\n", fields.title));
    fm.push_str(&body);
    fm.push('\n');

    std::fs::write(&path, &fm)
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    Ok(path)
}

/// Fields for creating a new sub-task.
#[derive(Debug, Clone, Default)]
pub struct SubtaskFields {
    pub parent_id: String,
    pub title: String,
    pub body: Option<String>,
}

/// Create a new sub-task file with YAML frontmatter.
///
/// Sub-tasks use dot-notation IDs: `{parent_id}.{n}` where n is the next
/// available integer (1-based). The file is written to the same tasks/
/// directory as the parent.
///
/// Returns the path to the created file.
pub fn create_subtask(root: &Path, fields: SubtaskFields) -> Result<PathBuf> {
    if fields.parent_id.is_empty() {
        anyhow::bail!("parent_id cannot be empty");
    }

    let tasks_dir = root.join("tasks");
    let dir = if tasks_dir.is_dir() {
        tasks_dir
    } else {
        root.to_path_buf()
    };

    // Find next available subtask number by scanning existing files
    let prefix = format!("{}.", fields.parent_id);
    let next_n = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let stem = name.strip_suffix(".md")?;
            let suffix = stem.strip_prefix(&prefix)?;
            suffix.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0)
        + 1;

    let id = format!("{}.{}", fields.parent_id, next_n);
    let filename = format!("{}.md", id);
    let path = dir.join(&filename);

    if path.exists() {
        anyhow::bail!("Sub-task file already exists: {}", path.display());
    }

    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", id));
    fm.push_str(&format!(
        "title: \"{}\"\n",
        yaml_escape_double_quoted(&fields.title)
    ));
    fm.push_str("type: subtask\n");
    fm.push_str("status: active\n");
    fm.push_str(&format!("parent: {}\n", fields.parent_id));
    fm.push_str("---\n\n");

    let body = fields
        .body
        .unwrap_or_else(|| format!("# {}\n", fields.title));
    fm.push_str(&body);
    fm.push('\n');

    std::fs::write(&path, &fm)
        .with_context(|| format!("Failed to write sub-task file: {}", path.display()))?;

    Ok(path)
}

/// Create a new task file with YAML frontmatter.
///
/// Returns the path to the created file. The filename is derived from the
/// task ID and title (slugified).
/// Ensure the well-known root node for ad-hoc sessions exists.
/// If it doesn't exist, creates a new project document with ID "adhoc-sessions".
pub fn ensure_adhoc_sessions_root(root: &Path) -> Result<()> {
    // Check if it already exists in projects/
    let adhoc_path = root.join("projects").join("adhoc-sessions.md");
    if adhoc_path.exists() {
        return Ok(());
    }

    let fields = DocumentFields {
        title: "Ad-hoc Sessions".to_string(),
        doc_type: "project".to_string(),
        id: Some("adhoc-sessions".to_string()),
        status: Some("active".to_string()),
        body: Some("# Ad-hoc Sessions\n\nRoot node for tasks created during ad-hoc agent sessions.\n".to_string()),
        ..Default::default()
    };

    create_document(root, fields)?;
    Ok(())
}

pub fn create_task(root: &Path, fields: TaskFields) -> Result<PathBuf> {
    // parent is required — tasks must be linked to an existing node
    if fields.parent.as_deref().map(str::is_empty).unwrap_or(true) {
        anyhow::bail!(
            "parent is required: tasks must be linked to a parent node \
             (goal, epic, or project). Only top-level types (goal, project, learn) \
             can be root-level."
        );
    }

    // project is required — every task must belong to a project for routing/filtering
    if fields.project.as_deref().map(str::is_empty).unwrap_or(true) {
        anyhow::bail!(
            "project is required: every task must declare a project (e.g. 'aops', \
             'mem', 'adhoc-sessions'). Set fields.project before calling create_task."
        );
    }

    // Validation
    if let Some(ref t) = fields.task_type {
        if !crate::graph::is_valid_node_type(t) {
            anyhow::bail!("Invalid task type: {}", t);
        }
    }
    if let Some(ref status) = fields.status {
        if !crate::graph::is_valid_status(status) {
            anyhow::bail!("Invalid status: {}", status);
        }
    }
    if let Some(priority) = fields.priority {
        if !crate::graph::is_valid_priority(priority) {
            anyhow::bail!("Invalid priority: {}. Must be between 0 and 4.", priority);
        }
    }
    if let Some(ref effort) = fields.effort {
        if !crate::graph::is_valid_effort(effort) {
            anyhow::bail!("Invalid effort: {}. Expected duration like '1d', '2h', '1w'.", effort);
        }
    }

    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: sanitize to prevent path traversal
            let safe_id = sanitize_prefix(&explicit_id);
            let filename = format!("{}.md", safe_id);
            (safe_id, filename)
        }
        None => {
            // Prefix source: use `project` when set so IDs/filenames are
            // namespaced like `aops-<hash>` / `aops-<hash>-<slug>.md`.
            // Fallback when project is missing: literal `"task"` to preserve
            // legacy behaviour for projectless / ad-hoc tasks (yields
            // `task-<hash>` / `task-<hash>-<slug>.md`). The `task_type` field
            // is intentionally NOT used here — it is captured in the
            // frontmatter `type:` field, not the ID prefix.
            let prefix = fields
                .project
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("task");
            let id = generate_id(prefix);
            let slug = slugify(&fields.title);
            let filename = format!("{}-{}.md", id, slug);
            (id, filename)
        }
    };

    // Use tasks/ subdirectory if it exists, otherwise root
    let tasks_dir = root.join("tasks");
    let dir = if tasks_dir.is_dir() {
        tasks_dir
    } else {
        root.to_path_buf()
    };
    let path = dir.join(&filename);

    if path.exists() {
        anyhow::bail!("Task file already exists: {}", path.display());
    }

    // Build YAML frontmatter
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", id));
    fm.push_str(&format!(
        "title: \"{}\"\n",
        yaml_escape_double_quoted(&fields.title)
    ));
    fm.push_str(&format!(
        "type: {}\n",
        fields.task_type.as_deref().unwrap_or("task")
    ));
    fm.push_str(&format!(
        "status: {}\n",
        fields.status.as_deref().unwrap_or("inbox")
    ));

    if let Some(p) = fields.priority {
        fm.push_str(&format!("priority: {}\n", p));
    } else {
        fm.push_str("priority: 2\n");
    }

    if let Some(ref parent) = fields.parent {
        fm.push_str(&format!("parent: {}\n", parent));
    }

    if let Some(ref project) = fields.project {
        fm.push_str(&format!("project: {}\n", project));
    }

    if !fields.tags.is_empty() {
        fm.push_str("tags:\n");
        for tag in &fields.tags {
            fm.push_str(&format!("  - {}\n", tag));
        }
    }

    if !fields.depends_on.is_empty() {
        fm.push_str("depends_on:\n");
        for dep in &fields.depends_on {
            fm.push_str(&format!("  - {}\n", dep));
        }
    }

    if let Some(ref assignee) = fields.assignee {
        fm.push_str(&format!("assignee: {}\n", assignee));
    }

    if let Some(ref complexity) = fields.complexity {
        fm.push_str(&format!("complexity: {}\n", complexity));
    }

    if let Some(ref effort) = fields.effort {
        fm.push_str(&format!("effort: {}\n", effort));
    }

    if let Some(ref consequence) = fields.consequence {
        fm.push_str(&format!(
            "consequence: \"{}\"\n",
            yaml_escape_double_quoted(consequence)
        ));
    }

    if let Some(sev) = fields.severity {
        append_severity_field(&mut fm, sev);
    }

    if let Some(ref gt) = fields.goal_type {
        append_goal_type_field(&mut fm, gt);
    }

    if let Some(ref stakeholder) = fields.stakeholder {
        fm.push_str(&format!(
            "stakeholder: \"{}\"\n",
            yaml_escape_double_quoted(stakeholder)
        ));
    }

    if let Some(ref waiting_since) = fields.waiting_since {
        fm.push_str(&format!("waiting_since: {}\n", waiting_since));
    }

    if let Some(ref due) = fields.due {
        fm.push_str(&format!("due: {}\n", due));
    }

    if let Some(ref session_id) = fields.session_id {
        fm.push_str(&format!("session_id: {}\n", session_id));
    }

    if let Some(ref issue_url) = fields.issue_url {
        fm.push_str(&format!("issue_url: {}\n", issue_url));
    }

    if let Some(ref release_summary) = fields.release_summary {
        fm.push_str(&format!(
            "release_summary: \"{}\"\n",
            yaml_escape_double_quoted(release_summary)
        ));
    }

    if !fields.follow_up_tasks.is_empty() {
        fm.push_str("follow_up_tasks:\n");
        for task_id in &fields.follow_up_tasks {
            fm.push_str(&format!("  - {}\n", task_id));
        }
    }

    if !fields.contributes_to.is_empty() {
        let resolved = materialise_edge_inheritance(root, fields.contributes_to);
        if let Ok(yaml) = serde_yaml::to_string(&resolved) {
            fm.push_str("contributes_to:\n");
            for line in yaml.trim_start_matches("---\n").lines() {
                if !line.is_empty() {
                    fm.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    fm.push_str("---\n\n");

    let body = fields
        .body
        .unwrap_or_else(|| format!("# {}\n", fields.title));
    fm.push_str(&body);
    fm.push('\n');

    std::fs::write(&path, &fm)
        .with_context(|| format!("Failed to write task file: {}", path.display()))?;

    Ok(path)
}

/// Create a new memory file with YAML frontmatter.
///
/// Returns the path to the created file. Creates the `memories/` subdirectory
/// if it doesn't exist.
pub fn create_memory(root: &Path, fields: MemoryFields) -> Result<PathBuf> {
    if let Some(c) = fields.confidence {
        if !(0.0..=1.0).contains(&c) {
            anyhow::bail!("confidence must be between 0.0 and 1.0, got {}", c);
        }
    }

    // Validation
    let mem_type = fields.memory_type.as_deref().unwrap_or("memory");
    if !crate::graph::is_valid_node_type(mem_type) {
        anyhow::bail!("Invalid memory type: {}", mem_type);
    }

    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: use as-is for both frontmatter and filename
            let filename = format!("{}.md", explicit_id);
            (explicit_id, filename)
        }
        None => {
            let id = generate_id("mem");
            let slug = slugify(&fields.title);
            let filename = format!("{}-{}.md", id, slug);
            (id, filename)
        }
    };

    // Create memories/ subdirectory if needed
    let dir = root.join("memories");
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create memories directory: {}", dir.display()))?;
    }
    let path = dir.join(&filename);

    if path.exists() {
        anyhow::bail!("Memory file already exists: {}", path.display());
    }

    // Build YAML frontmatter
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", id));
    fm.push_str(&format!(
        "title: \"{}\"\n",
        yaml_escape_double_quoted(&fields.title)
    ));

    let mem_type = fields.memory_type.as_deref().unwrap_or("memory");
    fm.push_str(&format!("type: {}\n", mem_type));

    if !fields.tags.is_empty() {
        fm.push_str("tags:\n");
        for tag in &fields.tags {
            fm.push_str(&format!("  - {}\n", tag));
        }
    }

    if let Some(ref source) = fields.source {
        fm.push_str(&format!("source: \"{}\"\n", yaml_escape_double_quoted(source)));
    }

    if let Some(c) = fields.confidence {
        fm.push_str(&format!("confidence: {}\n", c));
    }

    if let Some(ref s) = fields.supersedes {
        fm.push_str(&format!("supersedes: \"{}\"\n", yaml_escape_double_quoted(s)));
    }

    fm.push_str(&format!("created: {}\n", chrono::Utc::now().to_rfc3339()));
    fm.push_str("---\n\n");

    let body = fields
        .body
        .unwrap_or_else(|| format!("# {}\n", fields.title));
    fm.push_str(&body);
    fm.push('\n');

    std::fs::write(&path, &fm)
        .with_context(|| format!("Failed to write memory file: {}", path.display()))?;

    Ok(path)
}

// =========================================================================
// `inherits_from:` edge resolution (one-time copy at creation)
// =========================================================================

/// Locate a markdown file in the PKB by node ID.
///
/// Scans the PKB directory for a file whose frontmatter `id` matches `node_id`,
/// or whose filename stem (with `.md` stripped) equals `node_id`. Returns the
/// first match. Used to resolve `inherits_from:` references on edge YAML.
fn find_node_file(pkb_root: &Path, node_id: &str) -> Option<PathBuf> {
    // Fast path: try common locations by stem before scanning the whole tree.
    for sub in &["tasks", "projects", "goals", "notes", "memories", ""] {
        let candidate = if sub.is_empty() {
            pkb_root.join(format!("{}.md", node_id))
        } else {
            pkb_root.join(sub).join(format!("{}.md", node_id))
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Slow path: scan the PKB and match by frontmatter id or filename stem.
    for path in crate::pkb::scan_directory_all(pkb_root) {
        // Match by stem prefix (e.g. "task-abc123-some-title.md" -> "task-abc123")
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem == node_id || stem.starts_with(&format!("{}-", node_id)) {
                return Some(path);
            }
        }
        // Match by frontmatter id
        if let Some(doc) = crate::pkb::parse_file(&path) {
            if let Some(fm) = doc.frontmatter.as_ref() {
                if fm.get("id").and_then(|v| v.as_str()) == Some(node_id) {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Read a prototype node's `edge_template` map (if any).
///
/// Returns `None` if the prototype file cannot be located or has no
/// `edge_template` mapping. Type is not enforced (any node can declare
/// an `edge_template`) so that prototypes can be promoted/demoted without
/// rewriting referencing edges.
fn read_edge_template(
    pkb_root: &Path,
    prototype_id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let path = find_node_file(pkb_root, prototype_id)?;
    let doc = crate::pkb::parse_file(&path)?;
    let fm = doc.frontmatter?;
    let tmpl = fm.get("edge_template")?.as_object()?.clone();
    Some(tmpl)
}

/// Edge fields that may be inherited from a prototype's `edge_template`.
///
/// Per spec §2.5: `weight`/`stated_weight`, `consequence`, `goal_type`,
/// `severity`, `justification`/`why`. Resolution order: instance > template.
const EDGE_TEMPLATE_FIELDS: &[&str] = &[
    "weight",
    "stated_weight",
    "consequence",
    "goal_type",
    "severity",
    "justification",
    "why",
];

/// Materialise `inherits_from:` references on `contributes_to` edges.
///
/// For each edge with `inherits_from: <prototype-id>`, fetches the prototype's
/// `edge_template` map and copies any field listed in [`EDGE_TEMPLATE_FIELDS`]
/// onto the edge — but only if the edge does NOT already declare that field
/// (instance > template). The `inherits_from:` key is preserved on the edge as
/// a provenance breadcrumb (it is NOT a live reference).
///
/// Resolution is one-time at write time. Subsequent edits to the prototype's
/// `edge_template` will NOT retroactively rewrite edges that have already
/// been materialised.
///
/// Edges without `inherits_from:` and edges whose prototype cannot be located
/// pass through unchanged. Resolution failures are silent (the edge is left
/// as-is) — validation is the caller's job.
pub fn materialise_edge_inheritance(
    pkb_root: &Path,
    edges: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    edges
        .into_iter()
        .map(|edge| materialise_one_edge(pkb_root, edge))
        .collect()
}

fn materialise_one_edge(pkb_root: &Path, edge: serde_json::Value) -> serde_json::Value {
    let mut obj = match edge {
        serde_json::Value::Object(m) => m,
        other => return other,
    };

    let prototype_id = match obj.get("inherits_from").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return serde_json::Value::Object(obj),
    };

    let template = match read_edge_template(pkb_root, &prototype_id) {
        Some(t) => t,
        None => return serde_json::Value::Object(obj),
    };

    // Treat `weight` and `stated_weight` as the same field for the
    // override check (and likewise `justification`/`why`).
    let weight_set = obj.contains_key("weight") || obj.contains_key("stated_weight");
    let justification_set = obj.contains_key("justification") || obj.contains_key("why");

    for (key, value) in template.iter() {
        if !EDGE_TEMPLATE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let already_set = match key.as_str() {
            "weight" | "stated_weight" => weight_set,
            "justification" | "why" => justification_set,
            other => obj.contains_key(other),
        };
        if already_set {
            continue;
        }
        obj.insert(key.clone(), value.clone());
    }

    serde_json::Value::Object(obj)
}

/// Keys that belong in the markdown body, not YAML frontmatter.
/// If any of these appear in `updates`, they update the body section instead of frontmatter.
const FRONTMATTER_EXCLUDED_KEYS: &[&str] = &["body", "content"];

/// Update frontmatter fields in an existing document file.
///
/// Reads the file, patches the YAML frontmatter, and rewrites it.
/// Auto-sets `modified` timestamp on every update.
/// Works for tasks, memories, and all document types.
///
/// Special handling: if `updates` contains a `body` or `content` key, the value
/// is written to the markdown body section (after `---`) rather than frontmatter.
pub fn update_document(path: &Path, updates: HashMap<String, serde_json::Value>) -> Result<()> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;

    let matter = Matter::<YAML>::new();
    let result = matter.parse(&content);

    let mut fm: serde_json::Map<String, serde_json::Value> = result
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    // Apply updates, routing body/content keys to the markdown body instead of frontmatter
    let mut new_body_text: Option<String> = None;
    for (key, value) in updates {
        if FRONTMATTER_EXCLUDED_KEYS.contains(&key.as_str()) {
            // Prefer "body" over "content" if both are provided
            if key == "body" || new_body_text.is_none() {
                new_body_text = value.as_str().map(|s| s.to_string());
            }
            continue;
        }

        // Validation for updated fields
        match key.as_str() {
            "status" => {
                if let Some(s) = value.as_str() {
                    if !crate::graph::is_valid_status(s) {
                        anyhow::bail!("Invalid status: {}", s);
                    }

                    // Check for backwards transition
                    if let Some(old_status) = fm.get("status").and_then(|v| v.as_str()) {
                        let old_rank = crate::graph::status_rank(old_status);
                        let new_rank = crate::graph::status_rank(s);
                        if new_rank >= 0 && old_rank >= 0 && new_rank < old_rank {
                            // Backwards transition — we allow it but maybe we should log?
                            // The task says "flagged (warn, not block)".
                            // In this context, we don't have a good way to "warn" without blocking
                            // unless we use a logger.
                            tracing::warn!(
                                "Backwards status transition detected: {} -> {}",
                                old_status,
                                s
                            );
                        }
                    }
                }
            }
            "type" => {
                if let Some(t) = value.as_str() {
                    if !crate::graph::is_valid_node_type(t) {
                        anyhow::bail!("Invalid node type: {}", t);
                    }
                }
            }
            "priority" => {
                if let Some(p) = value.as_i64() {
                    if !crate::graph::is_valid_priority(p as i32) {
                        anyhow::bail!("Invalid priority: {}. Must be between 0 and 4.", p);
                    }
                }
            }
            "effort" => {
                if let Some(e) = value.as_str() {
                    if !crate::graph::is_valid_effort(e) {
                        anyhow::bail!("Invalid effort: {}. Expected duration like '1d', '2h', '1w'.", e);
                    }
                }
            }
            _ => {}
        }

        if value.is_null() {
            fm.remove(&key);
        } else if key == "contributes_to" {
            // Materialise `inherits_from:` edge inheritance at write time (one-time copy).
            // Infer pkb_root from the file path: <pkb_root>/<subdir>/<file.md>.
            let pkb_root = path
                .parent()
                .and_then(|p| p.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let edges = match value {
                serde_json::Value::Array(arr) => arr,
                serde_json::Value::Null => Vec::new(),
                other => vec![other],
            };
            let resolved = materialise_edge_inheritance(&pkb_root, edges);
            fm.insert(key, serde_json::Value::Array(resolved));
        } else {
            fm.insert(key, value);
        }
    }

    // Auto-update modified timestamp
    fm.insert(
        "modified".to_string(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );

    // Rebuild the file
    let yaml = serde_yaml::to_string(&fm).context("Failed to serialize frontmatter")?;
    let body = new_body_text
        .as_deref()
        .unwrap_or_else(|| result.content.trim());

    let new_content = format!("---\n{}---\n\n{}\n", yaml, body);
    std::fs::write(path, &new_content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Append timestamped content to an existing document.
///
/// - If `section` is provided, finds `## {section}` heading and appends before
///   the next heading (or end of file).
/// - If no section: appends to end of body.
/// - Auto-updates `modified` timestamp in frontmatter.
/// - Content is timestamped: `\n\n**{UTC datetime}** — {content}\n`
pub fn append_to_document(path: &Path, content: &str, section: Option<&str>) -> Result<()> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;

    let file_content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;

    let matter = Matter::<YAML>::new();
    let result = matter.parse(&file_content);

    // Update modified timestamp in frontmatter
    let mut fm: serde_json::Map<String, serde_json::Value> = result
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    fm.insert(
        "modified".to_string(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );

    let yaml = serde_yaml::to_string(&fm).context("Failed to serialize frontmatter")?;

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
    let timestamped = format!("\n**{}** — {}\n", now, content);

    let body = result.content.clone();

    let new_body = if let Some(heading) = section {
        // Find ## {heading} and insert before the next heading
        let pattern = format!("## {}", heading);
        if let Some(section_pos) = body.find(&pattern) {
            let after_heading = section_pos + pattern.len();
            // Find next ## heading after this section
            let rest = &body[after_heading..];
            if let Some(next_heading) = rest.find("\n## ") {
                let insert_pos = after_heading + next_heading;
                format!(
                    "{}{}{}",
                    &body[..insert_pos],
                    timestamped,
                    &body[insert_pos..]
                )
            } else {
                // No next heading — append to end
                let trimmed = body.trim_end();
                format!("{}{}\n", trimmed, timestamped)
            }
        } else {
            // Section not found — create it and append
            let trimmed = body.trim_end();
            format!("{}\n\n## {}\n{}\n", trimmed, heading, timestamped)
        }
    } else {
        // No section — append to end of body
        let trimmed = body.trim_end();
        format!("{}{}\n", trimmed, timestamped)
    };

    let new_content = format!("---\n{}---\n\n{}\n", yaml, new_body.trim());
    std::fs::write(path, &new_content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Delete a document file from disk.
///
/// Returns the absolute path that was deleted (for VectorStore cleanup).
pub fn delete_document(path: &Path) -> Result<PathBuf> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    if !abs_path.exists() {
        anyhow::bail!("File not found: {}", abs_path.display());
    }

    std::fs::remove_file(&abs_path)
        .with_context(|| format!("Failed to delete: {}", abs_path.display()))?;

    Ok(abs_path)
}

fn append_severity_field(fm: &mut String, sev: i32) {
    fm.push_str(&format!("severity: {}\n", sev));
}

/// Escape a string for YAML double-quoted scalar form.
///
/// Replaces the four characters that break a single-line `"..."` scalar:
/// `\` (must escape first), `"`, `\n`, and `\r`. Tabs are passed through —
/// YAML allows them in double-quoted scalars.
///
/// Background: the previous one-liner `s.replace('"', "\\\"")` only escaped
/// embedded quotes. Strings containing newlines were emitted verbatim, e.g.
/// `consequence: "line1\nline2"` (with a literal newline). serde_yaml folds
/// such cases to a space silently, while gray_matter (used by the MCP
/// `get_task` reader) rejects the whole frontmatter and returns `Null` —
/// which surfaced as `frontmatter: null`, `parent: null`, default priority,
/// and a stub body for any task whose `consequence` / `release_summary` /
/// title / stakeholder field happened to contain a newline. Re-confirmation
/// of task-16fe56e6 on 2026-05-05.
pub(crate) fn yaml_escape_double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn append_goal_type_field(fm: &mut String, gt: &str) {
    fm.push_str(&format!("goal_type: {}\n", gt));
}

/// Generate a new random document ID: `{prefix}-{8 random hex chars}`.
fn generate_id(prefix: &str) -> String {
    crate::graph::create_id(&sanitize_prefix(prefix))
}

/// Sanitize a prefix string to prevent path traversal and invalid IDs.
/// Strips path separators, `..`, and non-alphanumeric/hyphen characters.
pub fn sanitize_prefix(prefix: &str) -> String {
    let sanitized: String = prefix
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .to_lowercase();
    // Collapse consecutive hyphens and trim leading/trailing hyphens
    let collapsed: String = sanitized
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "doc".to_string()
    } else {
        collapsed
    }
}

/// Result of a bulk reparent operation on a single file.
#[derive(Debug)]
pub enum ReparentResult {
    /// File was updated (or would be updated in dry-run mode).
    Updated(PathBuf),
    /// File was skipped because its permalink matches the parent_id.
    SkippedSelf(PathBuf),
    /// File was skipped because it already has the correct parent.
    SkippedAlreadyParented(PathBuf),
}

/// Bulk reparent: set `parent` field in YAML frontmatter for all .md files
/// matching a directory path or glob pattern.
///
/// - `pattern`: directory path or glob pattern (e.g. "archive/" or "tasks/*.md")
/// - `parent_id`: the parent ID to set
/// - `dry_run`: if true, don't write changes, just report what would change
///
/// Returns a list of results for each file processed.
pub fn bulk_reparent(
    root: &Path,
    pattern: &str,
    parent_id: &str,
    dry_run: bool,
) -> Result<Vec<ReparentResult>> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;

    // Resolve the pattern to a list of .md files
    let resolved = resolve_path_or_glob(root, pattern);
    if resolved.is_empty() {
        anyhow::bail!("No .md files found matching pattern: {}", pattern);
    }

    let matter = Matter::<YAML>::new();
    let mut results = Vec::new();

    for path in &resolved {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        let parsed = matter.parse(&content);

        let fm: serde_json::Map<String, serde_json::Value> = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

        // Skip if this file IS the parent (by permalink or id match)
        let file_id = fm.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let file_permalink = fm.get("permalink").and_then(|v| v.as_str()).unwrap_or("");
        if file_id == parent_id || file_permalink == parent_id {
            results.push(ReparentResult::SkippedSelf(path.clone()));
            continue;
        }

        // Skip if already has the correct parent
        if let Some(existing_parent) = fm.get("parent").and_then(|v| v.as_str()) {
            if existing_parent == parent_id {
                results.push(ReparentResult::SkippedAlreadyParented(path.clone()));
                continue;
            }
        }

        if !dry_run {
            let mut updates = HashMap::new();
            updates.insert(
                "parent".to_string(),
                serde_json::Value::String(parent_id.to_string()),
            );
            update_document(path, updates)
                .with_context(|| format!("Failed to update: {}", path.display()))?;
        }

        results.push(ReparentResult::Updated(path.clone()));
    }

    Ok(results)
}

/// Resolve a pattern to a list of .md files.
///
/// If `pattern` is a directory (absolute or relative to root), returns all .md
/// files in that directory (non-recursive). If it contains glob characters
/// (`*`, `?`, `[`), uses glob matching relative to root. Otherwise treats it
/// as a directory path.
fn resolve_path_or_glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let path = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        root.join(pattern)
    };

    // If it's a directory, list .md files in it (non-recursive)
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
            .collect();
        files.sort();
        return files;
    }

    // Try glob pattern
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let glob_pattern = if Path::new(pattern).is_absolute() {
            pattern.to_string()
        } else {
            format!("{}/{}", root.display(), pattern)
        };
        if let Ok(paths) = glob::glob(&glob_pattern) {
            let mut files: Vec<PathBuf> = paths
                .filter_map(|p| p.ok())
                .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
                .collect();
            files.sort();
            return files;
        }
    }

    Vec::new()
}

/// Expand environment variables in a string.
///
/// Handles `${VAR}` and `$VAR` patterns. Unresolved variables are left as-is.
/// Also expands `~` at the start to the user's home directory.
pub fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    // Handle ~ at start
    if chars.peek() == Some(&'~') {
        chars.next();
        if chars.peek().is_none() || chars.peek() == Some(&'/') {
            if let Some(home) = dirs::home_dir() {
                result.push_str(&home.to_string_lossy());
            } else {
                result.push('~');
            }
        } else {
            result.push('~');
        }
    }

    while let Some(c) = chars.next() {
        if c == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let var_name: String = chars.by_ref().take_while(|&ch| ch != '}').collect();
                if !var_name.is_empty() {
                    match std::env::var(&var_name) {
                        Ok(val) => result.push_str(&val),
                        Err(_) => {
                            result.push_str("${");
                            result.push_str(&var_name);
                            result.push('}');
                        }
                    }
                }
            } else {
                let mut var_name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        var_name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !var_name.is_empty() {
                    match std::env::var(&var_name) {
                        Ok(val) => result.push_str(&val),
                        Err(_) => {
                            result.push('$');
                            result.push_str(&var_name);
                        }
                    }
                } else {
                    result.push('$');
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Convert a title to a URL-safe slug.
fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_md(dir: &Path, name: &str, frontmatter: &str) {
        let path = dir.join(name);
        fs::write(&path, format!("---\n{}---\n\n# Body\n", frontmatter)).unwrap();
    }

    #[test]
    fn test_bulk_reparent_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subdir = root.join("archive");
        fs::create_dir_all(&subdir).unwrap();

        write_md(&subdir, "a.md", "id: a-123\ntitle: A\n");
        write_md(&subdir, "b.md", "id: b-456\ntitle: B\n");

        let results = bulk_reparent(root, "archive", "parent-001", true).unwrap();
        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], ReparentResult::Updated(_)));
        assert!(matches!(results[1], ReparentResult::Updated(_)));

        // Dry run: files should NOT be modified
        let content = fs::read_to_string(subdir.join("a.md")).unwrap();
        assert!(!content.contains("parent: parent-001"));
    }

    #[test]
    fn test_bulk_reparent_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subdir = root.join("tasks");
        fs::create_dir_all(&subdir).unwrap();

        write_md(&subdir, "t1.md", "id: t1\ntitle: Task1\n");
        write_md(&subdir, "t2.md", "id: t2\ntitle: Task2\nparent: old-parent\n");

        let results = bulk_reparent(root, "tasks", "new-parent", false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], ReparentResult::Updated(_)));
        assert!(matches!(results[1], ReparentResult::Updated(_)));

        // Applied: files should have parent field
        let content = fs::read_to_string(subdir.join("t1.md")).unwrap();
        assert!(content.contains("parent: new-parent"));
        let content2 = fs::read_to_string(subdir.join("t2.md")).unwrap();
        assert!(content2.contains("parent: new-parent"));
        assert!(!content2.contains("old-parent"));
    }

    #[test]
    fn test_bulk_reparent_skips_self() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subdir = root.join("docs");
        fs::create_dir_all(&subdir).unwrap();

        write_md(&subdir, "parent.md", "id: epic-001\npermalink: epic-001\ntitle: Epic\n");
        write_md(&subdir, "child.md", "id: child-001\ntitle: Child\n");

        let results = bulk_reparent(root, "docs", "epic-001", true).unwrap();
        assert_eq!(results.len(), 2);

        let self_skip = results.iter().any(|r| matches!(r, ReparentResult::SkippedSelf(_)));
        let updated = results.iter().any(|r| matches!(r, ReparentResult::Updated(_)));
        assert!(self_skip, "Should skip the parent file itself");
        assert!(updated, "Should update the child file");
    }

    #[test]
    fn test_bulk_reparent_skips_already_parented() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subdir = root.join("notes");
        fs::create_dir_all(&subdir).unwrap();

        write_md(&subdir, "already.md", "id: n1\ntitle: Note\nparent: target-parent\n");
        write_md(&subdir, "new.md", "id: n2\ntitle: New Note\n");

        let results = bulk_reparent(root, "notes", "target-parent", true).unwrap();
        assert_eq!(results.len(), 2);

        let already = results.iter().any(|r| matches!(r, ReparentResult::SkippedAlreadyParented(_)));
        let updated = results.iter().any(|r| matches!(r, ReparentResult::Updated(_)));
        assert!(already, "Should skip already-parented file");
        assert!(updated, "Should update the unparented file");
    }

    #[test]
    fn test_bulk_reparent_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let result = bulk_reparent(tmp.path(), "nonexistent", "parent-id", true);
        assert!(result.is_err());
    }

    #[test]
    fn expand_env_vars_braced_syntax() {
        std::env::set_var("_TEST_EXPAND_A", "/test/path");
        assert_eq!(expand_env_vars("${_TEST_EXPAND_A}/sub"), "/test/path/sub");
        std::env::remove_var("_TEST_EXPAND_A");
    }

    #[test]
    fn expand_env_vars_unbraced_syntax() {
        std::env::set_var("_TEST_EXPAND_B", "/other");
        assert_eq!(expand_env_vars("$_TEST_EXPAND_B/sub"), "/other/sub");
        std::env::remove_var("_TEST_EXPAND_B");
    }

    #[test]
    fn expand_env_vars_unresolved_kept() {
        assert_eq!(
            expand_env_vars("${_NONEXISTENT_VAR_XYZ}/path"),
            "${_NONEXISTENT_VAR_XYZ}/path"
        );
        assert_eq!(
            expand_env_vars("$_NONEXISTENT_VAR_XYZ/path"),
            "$_NONEXISTENT_VAR_XYZ/path"
        );
    }

    #[test]
    fn expand_env_vars_no_vars() {
        assert_eq!(expand_env_vars("plain/path"), "plain/path");
        assert_eq!(expand_env_vars(""), "");
    }

    #[test]
    fn expand_env_vars_tilde() {
        let expanded = expand_env_vars("~/documents");
        assert!(!expanded.starts_with('~'), "tilde should be expanded");
        assert!(expanded.ends_with("/documents"));
    }

    #[test]
    fn expand_env_vars_dollar_sign_alone() {
        assert_eq!(expand_env_vars("price is $"), "price is $");
    }

    #[test]
    fn expand_env_vars_multiple() {
        std::env::set_var("_TEST_EXPAND_C", "aaa");
        std::env::set_var("_TEST_EXPAND_D", "bbb");
        assert_eq!(
            expand_env_vars("${_TEST_EXPAND_C}/${_TEST_EXPAND_D}"),
            "aaa/bbb"
        );
        std::env::remove_var("_TEST_EXPAND_C");
        std::env::remove_var("_TEST_EXPAND_D");
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("foo--bar"), "foo-bar");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn generate_id_deterministic() {
        let id1 = generate_id("task");
        let id2 = generate_id("task");
        // IDs include random component, just check prefix
        assert!(id1.starts_with("task-"));
        assert!(id2.starts_with("task-"));
    }

    #[test]
    fn create_task_writes_project_type_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let fields = TaskFields {
            title: "Test task with metadata".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            task_type: Some("epic".to_string()),
            status: Some("in_progress".to_string()),
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        assert!(content.contains("type: epic"), "type field should be written: {content}");
        assert!(content.contains("status: in_progress"), "status field should be written: {content}");
        assert!(content.contains("project: aops"), "project field should be written: {content}");
        // ID should use the PROJECT as prefix (not task_type) — see
        // create_task: regression task-381788fb. Filename inherits the
        // project-prefixed ID.
        assert!(
            path.file_name().unwrap().to_string_lossy().starts_with("aops-"),
            "filename should use project prefix: {:?}",
            path.file_name()
        );
    }

    #[test]
    fn create_task_uses_project_as_id_prefix() {
        // Regression: task-381788fb. create_task(project="aops", title="Foo")
        // must produce ID `aops-<hash>` and filename `aops-<hash>-foo.md`.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("tasks")).unwrap();

        let fields = TaskFields {
            title: "Foo".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            filename.starts_with("aops-") && filename.ends_with("-foo.md"),
            "filename must be aops-<hash>-foo.md, got {filename}"
        );

        let content = fs::read_to_string(&path).unwrap();
        // Frontmatter id must match the project-prefixed ID (which is also
        // the filename stem minus the slug).
        let expected_id_prefix = "id: aops-";
        assert!(
            content.contains(expected_id_prefix),
            "frontmatter id must start with `aops-`: {content}"
        );
    }

    #[test]
    fn create_task_defaults_type_and_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let fields = TaskFields {
            title: "Default metadata task".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        assert!(content.contains("type: task"), "default type should be 'task': {content}");
        assert!(content.contains("status: inbox"), "default status should be 'inbox': {content}");
        assert!(content.contains("project: aops"), "project should be written: {content}");
    }

    #[test]
    fn create_task_rejects_missing_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let fields = TaskFields {
            title: "No project task".to_string(),
            parent: Some("parent-001".to_string()),
            ..Default::default()
        };

        let err = create_task(root, fields).unwrap_err();
        assert!(
            err.to_string().contains("project is required"),
            "error should mention project requirement: {err}"
        );
    }

    #[test]
    fn create_task_rejects_empty_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let fields = TaskFields {
            title: "Empty project task".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some(String::new()),
            ..Default::default()
        };

        let err = create_task(root, fields).unwrap_err();
        assert!(
            err.to_string().contains("project is required"),
            "error should mention project requirement: {err}"
        );
    }

    #[test]
    fn create_task_writes_body_verbatim() {
        // Regression: create_task was reportedly silently dropping the body, leaving
        // only `# <title>` on disk. Confirm a multi-line body — including headings,
        // lists, and blank lines — is preserved verbatim.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let body = "## Problem\n\nWhen X happens, Y goes wrong.\n\n## AC\n\n- [ ] item 1\n- [ ] item 2\n";
        let fields = TaskFields {
            title: "Body roundtrip task".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            body: Some(body.to_string()),
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // The body must appear verbatim *after* the closing frontmatter delimiter.
        let after_fm = content.split("---\n\n").nth(1).unwrap_or("");
        assert!(
            after_fm.contains(body),
            "body should be written verbatim after frontmatter; got after_fm:\n{after_fm}"
        );
        // Belt-and-braces: the synthesised fallback `# <title>` must NOT appear when a
        // body was supplied (otherwise we'd be appending a stray heading).
        assert!(
            !after_fm.starts_with("# Body roundtrip task\n"),
            "fallback `# <title>` heading should NOT appear when body is supplied: {after_fm}"
        );
    }

    #[test]
    fn create_task_handles_multiline_consequence() {
        // Regression: re-confirmation 2026-05-05 — create_task observed to drop
        // `parent`, `priority`, `frontmatter` and produce `null` round-trip when
        // the user supplied a multi-line `consequence` string. Newlines inside
        // double-quoted YAML scalars are illegal — `format!("consequence: \"{}\"")`
        // wrote an unparseable frontmatter block, so gray_matter returned no data
        // and downstream tools rendered the task as having no metadata.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("tasks")).unwrap();
        let multiline = "Single biggest token sink in the framework.\nEach cron \
                         firing reuses the prior context window.\nBlows up over time.";
        let fields = TaskFields {
            title: "fixed-interval mode accretes context".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            priority: Some(1),
            consequence: Some(multiline.to_string()),
            ..Default::default()
        };
        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Primary check: gray_matter is what `mcp_server::handle_get_task`
        // uses to parse the round-tripped frontmatter. Before the fix, raw
        // newlines in `consequence: "..."` made gray_matter return
        // `data: Some(Null)` for the *whole* frontmatter — surfaced to
        // callers as `frontmatter: null` / `parent: null` / default
        // priority / stub body.
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter.parse(&content);
        let fm_json = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        assert!(
            fm_json.is_object(),
            "frontmatter must parse as an object, not null/scalar — got {fm_json:?}"
        );
        let obj = fm_json.as_object().unwrap();
        assert_eq!(
            obj.get("priority").and_then(|v| v.as_i64()),
            Some(1),
            "priority must round-trip via gray_matter: {fm_json:?}"
        );
        assert_eq!(
            obj.get("parent").and_then(|v| v.as_str()),
            Some("parent-001"),
            "parent must round-trip via gray_matter: {fm_json:?}"
        );
        let consequence_back = obj
            .get("consequence")
            .and_then(|v| v.as_str())
            .expect("consequence must round-trip via gray_matter");
        assert_eq!(
            consequence_back, multiline,
            "multi-line consequence must round-trip verbatim"
        );
    }

    #[test]
    fn create_task_always_writes_id_title_type() {
        // Regression: re-confirmation 2026-04-30 — the on-disk frontmatter was
        // observed to be missing `id:`, `title:`, and `type:` fields. The canonical
        // create_task path MUST always write all three (the server generates the
        // ID, knows the title, and defaults type=task).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        // Minimal fields — no explicit id, no explicit type
        let fields = TaskFields {
            title: "Bare-minimum task".to_string(),
            parent: Some("parent-001".to_string()),
            project: Some("aops".to_string()),
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Frontmatter section is between the first two `---` delimiters
        let frontmatter = content
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(fm, _)| fm)
            .expect("file must have YAML frontmatter delimiters");

        // All three fields must be present, with non-empty values.
        let has_field = |key: &str| -> bool {
            frontmatter
                .lines()
                .any(|line| line.starts_with(&format!("{key}: ")) && line.len() > key.len() + 2)
        };
        assert!(has_field("id"), "id: must always be written; frontmatter:\n{frontmatter}");
        assert!(has_field("title"), "title: must always be written; frontmatter:\n{frontmatter}");
        assert!(has_field("type"), "type: must always be written; frontmatter:\n{frontmatter}");
        assert!(has_field("status"), "status: must always be written; frontmatter:\n{frontmatter}");
    }

    // =====================================================================
    // `inherits_from:` edge resolution tests (task-74d9c9db)
    // =====================================================================

    /// Set up a PKB tmpdir with a single prototype node.
    fn setup_pkb_with_prototype(template_yaml: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("tasks")).unwrap();
        fs::create_dir_all(root.join("notes")).unwrap();

        let proto_path = root.join("notes").join("task-b9d6ff7e.md");
        let content = format!(
            "---\nid: task-b9d6ff7e\ntitle: \"OSB voting prototype\"\ntype: prototype\n{}---\n\n# Body\n",
            template_yaml
        );
        fs::write(&proto_path, content).unwrap();
        (tmp, root)
    }

    #[test]
    fn inherits_from_materialises_template_fields_at_creation() {
        let (_tmp, root) = setup_pkb_with_prototype(
            "edge_template:\n  weight: Certain\n  goal_type: committed\n  severity: 3\n  consequence: \"OSB obligation\"\n",
        );

        let edge = serde_json::json!({
            "to": "task-b9d6ff7e",
            "inherits_from": "task-b9d6ff7e",
            "justification": "contractual OSB voting obligation"
        });

        let fields = TaskFields {
            title: "OSB instance".into(),
            parent: Some("task-b9d6ff7e".into()),
            project: Some("aops".into()),
            contributes_to: vec![edge],
            ..Default::default()
        };

        let path = create_task(&root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Template fields materialised onto the edge YAML.
        assert!(content.contains("weight: Certain"), "weight from template: {content}");
        assert!(content.contains("goal_type: committed"), "goal_type from template: {content}");
        assert!(content.contains("severity: 3"), "severity from template: {content}");
        assert!(
            content.contains("consequence: OSB obligation") || content.contains("consequence: 'OSB obligation'") || content.contains("consequence: \"OSB obligation\""),
            "consequence from template: {content}"
        );
        // Provenance preserved.
        assert!(
            content.contains("inherits_from: task-b9d6ff7e"),
            "inherits_from preserved as provenance: {content}"
        );
        // Instance-set field preserved.
        assert!(
            content.contains("contractual OSB voting obligation"),
            "instance justification preserved: {content}"
        );
    }

    #[test]
    fn inherits_from_instance_field_wins_over_template() {
        let (_tmp, root) = setup_pkb_with_prototype(
            "edge_template:\n  weight: Certain\n  goal_type: committed\n  severity: 3\n",
        );

        let edge = serde_json::json!({
            "to": "task-b9d6ff7e",
            "inherits_from": "task-b9d6ff7e",
            "weight": "Expected",
            "justification": "instance override"
        });

        let fields = TaskFields {
            title: "Override instance".into(),
            parent: Some("task-b9d6ff7e".into()),
            project: Some("aops".into()),
            contributes_to: vec![edge],
            ..Default::default()
        };

        let path = create_task(&root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Instance weight wins.
        assert!(content.contains("weight: Expected"), "instance weight wins: {content}");
        assert!(!content.contains("weight: Certain"), "template weight NOT applied: {content}");
        // Other template fields fill gaps.
        assert!(content.contains("goal_type: committed"), "template goal_type fills gap: {content}");
        assert!(content.contains("severity: 3"), "template severity fills gap: {content}");
    }

    #[test]
    fn editing_prototype_does_not_rewrite_existing_edges() {
        let (_tmp, root) = setup_pkb_with_prototype(
            "edge_template:\n  weight: Certain\n  goal_type: committed\n  severity: 3\n",
        );

        // 1. Create edge — materialised with weight: Certain.
        let edge = serde_json::json!({
            "to": "task-b9d6ff7e",
            "inherits_from": "task-b9d6ff7e",
            "justification": "first beliefs"
        });
        let fields = TaskFields {
            title: "Round-trip task".into(),
            parent: Some("task-b9d6ff7e".into()),
            project: Some("aops".into()),
            contributes_to: vec![edge],
            ..Default::default()
        };
        let edge_path = create_task(&root, fields).unwrap();
        let original = fs::read_to_string(&edge_path).unwrap();
        assert!(original.contains("weight: Certain"));
        assert!(original.contains("severity: 3"));

        // 2. Edit prototype — rewrite edge_template with new values.
        let proto_path = root.join("notes").join("task-b9d6ff7e.md");
        fs::write(
            &proto_path,
            "---\nid: task-b9d6ff7e\ntitle: \"OSB voting prototype\"\ntype: prototype\nedge_template:\n  weight: Improbable\n  goal_type: aspirational\n  severity: 0\n---\n\n# Body\n",
        )
        .unwrap();

        // 3. Existing edge file MUST NOT be rewritten.
        let after = fs::read_to_string(&edge_path).unwrap();
        assert_eq!(original, after, "existing edge file untouched by prototype edit");
        assert!(after.contains("weight: Certain"), "old materialised value still on edge");
        assert!(!after.contains("weight: Improbable"), "new template value not applied");
    }

    #[test]
    fn edge_without_inherits_from_passes_through_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("tasks")).unwrap();

        let edge = serde_json::json!({
            "to": "some-target",
            "weight": "Probable",
            "justification": "no inheritance here"
        });

        let fields = TaskFields {
            title: "Plain edge".into(),
            parent: Some("parent-001".into()),
            project: Some("aops".into()),
            contributes_to: vec![edge],
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("weight: Probable"));
        assert!(!content.contains("inherits_from"));
    }

    #[test]
    fn inherits_from_with_missing_prototype_passes_through() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("tasks")).unwrap();

        let edge = serde_json::json!({
            "to": "task-missing",
            "inherits_from": "task-missing",
            "weight": "Expected"
        });

        let fields = TaskFields {
            title: "Dangling inherits".into(),
            parent: Some("parent-001".into()),
            project: Some("aops".into()),
            contributes_to: vec![edge],
            ..Default::default()
        };

        let path = create_task(root, fields).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        // Edge written; provenance preserved; no extra fields synthesized.
        assert!(content.contains("inherits_from: task-missing"));
        assert!(content.contains("weight: Expected"));
        assert!(!content.contains("severity:"));
    }

    #[test]
    fn update_document_materialises_inheritance_on_contributes_to() {
        let (_tmp, root) = setup_pkb_with_prototype(
            "edge_template:\n  weight: Certain\n  goal_type: committed\n  severity: 3\n",
        );

        // Create a plain task without contributes_to.
        let fields = TaskFields {
            title: "Will gain edge later".into(),
            parent: Some("task-b9d6ff7e".into()),
            project: Some("aops".into()),
            ..Default::default()
        };
        let task_path = create_task(&root, fields).unwrap();

        // Update with contributes_to via update_document.
        let edge = serde_json::json!([{
            "to": "task-b9d6ff7e",
            "inherits_from": "task-b9d6ff7e",
            "justification": "added later"
        }]);
        let mut updates = HashMap::new();
        updates.insert("contributes_to".to_string(), edge);
        update_document(&task_path, updates).unwrap();

        let content = fs::read_to_string(&task_path).unwrap();
        assert!(content.contains("weight: Certain"), "materialised weight: {content}");
        assert!(content.contains("severity: 3"), "materialised severity: {content}");
        assert!(content.contains("inherits_from: task-b9d6ff7e"), "provenance preserved: {content}");
    }
}

// ── Merge node ────────────────────────────────────────────────────────────

/// Summary of a `merge_node` operation.
#[derive(Debug)]
pub struct MergeNodeSummary {
    /// Number of files in which at least one reference was updated.
    pub files_updated: usize,
    /// Total number of individual references redirected.
    pub refs_redirected: usize,
    /// Number of source nodes archived (status=done, superseded_by=canonical).
    pub nodes_archived: usize,
    pub dry_run: bool,
    /// Absolute paths of all files written during this merge — both files
    /// where references were redirected AND the archived source files.
    /// Used by callers to re-embed only the affected docs.
    pub modified_paths: Vec<PathBuf>,
}

/// Merge one or more source nodes into a canonical node.
///
/// For each source ID this function:
/// 1. Scans every PKB file and rewrites all references to the source ID
///    (`parent`, `depends_on`, `soft_depends_on`, `blocks`, `soft_blocks`,
///    `supersedes`, and wikilinks) to point to `canonical_id` instead — but
///    leaves the source file's own `id:` field untouched.
/// 2. Archives the source node by setting `status: done` and
///    `superseded_by: <canonical_id>` in its frontmatter.
///
/// Unlike `rename_id` (which changes the node's own ID), this operation
/// preserves the source node as an archived record.
pub fn merge_node(
    pkb_root: &Path,
    source_ids: &[String],
    canonical_id: &str,
    dry_run: bool,
) -> Result<MergeNodeSummary> {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;
    use std::collections::HashSet;

    if source_ids.is_empty() {
        anyhow::bail!("source_ids must not be empty");
    }

    let source_set: HashSet<&str> = source_ids.iter().map(|s| s.as_str()).collect();
    let ref_fields = [
        "parent",
        "depends_on",
        "soft_depends_on",
        "blocks",
        "soft_blocks",
        "supersedes",
    ];

    let files = crate::pkb::scan_directory(pkb_root);
    let mut files_updated = 0usize;
    let mut refs_redirected = 0usize;
    let mut modified_paths: Vec<PathBuf> = Vec::new();
    // Track each source ID → its file path for archiving
    let mut source_paths: HashMap<String, PathBuf> = HashMap::new();

    for file_path in &files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(&content);
        let fm_data = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok());
        let fm = match fm_data.as_ref().and_then(|v| v.as_object()) {
            Some(m) => m,
            None => continue,
        };

        // Track source file paths for later archiving
        if let Some(file_id) = fm.get("id").and_then(|v| v.as_str()) {
            if source_set.contains(file_id) {
                source_paths.insert(file_id.to_string(), file_path.clone());
                // Don't update reference fields in source files —
                // they'll be archived separately.
                continue;
            }
        }

        let mut modified = false;
        let mut new_content = content.clone();

        // Redirect frontmatter reference fields
        for field in &ref_fields {
            if let Some(val) = fm.get(*field) {
                match val {
                    serde_json::Value::String(s) if source_set.contains(s.as_str()) => {
                        let old_line = format!("{}: {}", field, s);
                        let new_line = format!("{}: {}", field, canonical_id);
                        if new_content.contains(&old_line) {
                            new_content = new_content.replace(&old_line, &new_line);
                            modified = true;
                            refs_redirected += 1;
                        }
                    }
                    serde_json::Value::Array(arr) => {
                        for item in arr {
                            if let Some(ref_id) = item.as_str() {
                                if source_set.contains(ref_id) {
                                    let old_item = format!("- {}", ref_id);
                                    let new_item = format!("- {}", canonical_id);
                                    if new_content.contains(&old_item) {
                                        new_content =
                                            new_content.replacen(&old_item, &new_item, 1);
                                        modified = true;
                                        refs_redirected += 1;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Redirect wikilinks
        for source_id in source_ids {
            let wiki_plain = format!("[[{}]]", source_id);
            let wiki_plain_new = format!("[[{}]]", canonical_id);
            if new_content.contains(&wiki_plain) {
                new_content = new_content.replace(&wiki_plain, &wiki_plain_new);
                modified = true;
                refs_redirected += 1;
            }
            let wiki_alias = format!("[[{}|", source_id);
            let wiki_alias_new = format!("[[{}|", canonical_id);
            if new_content.contains(&wiki_alias) {
                new_content = new_content.replace(&wiki_alias, &wiki_alias_new);
                modified = true;
                refs_redirected += 1;
            }
        }

        if modified {
            if !dry_run {
                let _ = std::fs::write(file_path, &new_content);
                modified_paths.push(file_path.clone());
            }
            files_updated += 1;
        }
    }

    // Archive source nodes
    let mut nodes_archived = 0usize;
    for (src_id, src_path) in &source_paths {
        if !dry_run {
            let mut updates = HashMap::new();
            updates.insert(
                "status".to_string(),
                serde_json::Value::String("done".to_string()),
            );
            updates.insert(
                "superseded_by".to_string(),
                serde_json::Value::String(canonical_id.to_string()),
            );
            if let Err(e) = update_document(src_path, updates) {
                eprintln!("Warning: failed to archive {}: {}", src_id, e);
            } else {
                nodes_archived += 1;
                modified_paths.push(src_path.clone());
            }
        } else {
            nodes_archived += 1;
        }
    }

    Ok(MergeNodeSummary {
        files_updated,
        refs_redirected,
        nodes_archived,
        dry_run,
        modified_paths,
    })
}
