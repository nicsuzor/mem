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
    pub project: Option<String>,
    pub assignee: Option<String>,
    pub complexity: Option<String>,
    pub source: Option<String>,
    pub due: Option<String>,
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
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    pub assignee: Option<String>,
    pub complexity: Option<String>,
    pub body: Option<String>,
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
    let type_prefix = match fields.doc_type.as_str() {
        "task" | "bug" | "epic" | "feature" => "task",
        "project" => "proj",
        "goal" => "goal",
        "memory" => "mem",
        "note" => "note",
        "knowledge" => "kb",
        "insight" => "ins",
        "observation" => "obs",
        other => &other[..other.len().min(4)],
    };

    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: use as-is for both frontmatter and filename
            let filename = format!("{}.md", explicit_id);
            (explicit_id, filename)
        }
        None => {
            // Use project as prefix when available, otherwise type-based prefix
            let prefix = fields.project.as_deref().unwrap_or(type_prefix);
            let id = generate_id(prefix, &fields.title);
            let slug = slugify(&fields.title);
            let filename = format!("{}-{}.md", id, slug);
            (id, filename)
        }
    };

    // Determine subdirectory
    let subdir = fields
        .dir
        .unwrap_or_else(|| match fields.doc_type.as_str() {
            "task" | "bug" | "epic" | "feature" => "tasks".to_string(),
            "project" => "projects".to_string(),
            "goal" => "goals".to_string(),
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

    if let Some(ref source) = fields.source {
        fm.push_str(&format!("source: \"{}\"\n", source.replace('"', "\\\"")));
    }

    if let Some(ref due) = fields.due {
        fm.push_str(&format!("due: {}\n", due));
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

/// Create a new task file with YAML frontmatter.
///
/// Returns the path to the created file. The filename is derived from the
/// task ID and title (slugified).
pub fn create_task(root: &Path, fields: TaskFields) -> Result<PathBuf> {
    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: use as-is for both frontmatter and filename
            let filename = format!("{}.md", explicit_id);
            (explicit_id, filename)
        }
        None => {
            // Use project as prefix when available, otherwise "task"
            let prefix = fields.project.as_deref().unwrap_or("task");
            let id = generate_id(prefix, &fields.title);
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
    let (id, filename) = match fields.id {
        Some(explicit_id) => {
            // Explicit ID: use as-is for both frontmatter and filename
            let filename = format!("{}.md", explicit_id);
            (explicit_id, filename)
        }
        None => {
            let id = generate_id("mem", &fields.title);
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

/// Update frontmatter fields in an existing document file.
///
/// Reads the file, patches the YAML frontmatter, and rewrites it.
/// Auto-sets `modified` timestamp on every update.
/// Works for tasks, memories, and all document types.
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

    // Apply updates
    for (key, value) in updates {
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
    let body = result.content.trim();

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

/// Generate a document ID from prefix and title (e.g., "task-a1b2c3d4", "mem-d4e5f6a7").
fn generate_id(prefix: &str, title: &str) -> String {
    let hash = format!("{:x}", md5::compute(title.as_bytes()));
    format!("{}-{}", prefix, &hash[..8])
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
