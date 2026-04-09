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
    pub stakeholder: Option<String>,
    pub waiting_since: Option<String>,
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
    pub body: Option<String>,
    pub stakeholder: Option<String>,
    pub waiting_since: Option<String>,
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
        fields.title.replace('"', "\\\"")
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
        fm.push_str(&format!("source: \"{}\"\n", source.replace('"', "\\\"")));
    }

    if let Some(c) = fields.confidence {
        fm.push_str(&format!("confidence: {}\n", c));
    }

    if let Some(ref s) = fields.supersedes {
        fm.push_str(&format!("supersedes: \"{}\"\n", s.replace('"', "\\\"")));
    }

    if let Some(ref due) = fields.due {
        fm.push_str(&format!("due: {}\n", due));
    }

    if let Some(ref stakeholder) = fields.stakeholder {
        fm.push_str(&format!("stakeholder: \"{}\"\n", stakeholder.replace('"', "\\\"")));
    }

    if let Some(ref waiting_since) = fields.waiting_since {
        fm.push_str(&format!("waiting_since: {}\n", waiting_since));
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
        fields.title.replace('"', "\\\"")
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
pub fn create_task(root: &Path, fields: TaskFields) -> Result<PathBuf> {
    // parent is required — tasks must be linked to an existing node
    if fields.parent.as_deref().map(str::is_empty).unwrap_or(true) {
        anyhow::bail!(
            "parent is required: tasks must be linked to a parent node \
             (goal, epic, or project). Only top-level types (goal, project, learn) \
             can be root-level."
        );
    }
    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: sanitize to prevent path traversal
            let safe_id = sanitize_prefix(&explicit_id);
            let filename = format!("{}.md", safe_id);
            (safe_id, filename)
        }
        None => {
            // Use project as prefix when available, otherwise "task"
            let prefix = "task";
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
        fields.title.replace('"', "\\\"")
    ));
    fm.push_str("type: task\n");
    fm.push_str("status: active\n");

    if let Some(p) = fields.priority {
        fm.push_str(&format!("priority: {}\n", p));
    } else {
        fm.push_str("priority: 2\n");
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

    if let Some(ref stakeholder) = fields.stakeholder {
        fm.push_str(&format!("stakeholder: \"{}\"\n", stakeholder.replace('"', "\\\"")));
    }

    if let Some(ref waiting_since) = fields.waiting_since {
        fm.push_str(&format!("waiting_since: {}\n", waiting_since));
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
        fields.title.replace('"', "\\\"")
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
        fm.push_str(&format!("source: \"{}\"\n", source.replace('"', "\\\"")));
    }

    if let Some(c) = fields.confidence {
        fm.push_str(&format!("confidence: {}\n", c));
    }

    if let Some(ref s) = fields.supersedes {
        fm.push_str(&format!("supersedes: \"{}\"\n", s.replace('"', "\\\"")));
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
        if value.is_null() {
            fm.remove(&key);
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
    })
}
