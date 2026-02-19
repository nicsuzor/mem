//! Task CRUD — create and update task markdown files.
//!
//! Generates markdown with YAML frontmatter for new tasks,
//! and patches frontmatter fields on existing task files.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Create a new task file with YAML frontmatter.
///
/// Returns the path to the created file. The filename is derived from the
/// task ID and title (slugified).
pub fn create_task(root: &Path, fields: TaskFields) -> Result<PathBuf> {
    // Generate an ID if not provided
    let id = fields
        .id
        .unwrap_or_else(|| generate_task_id(&fields.title));

    // Build filename: {id}-{slug}.md
    let slug = slugify(&fields.title);
    let filename = format!("{}-{}.md", id, slug);

    // Determine directory (use tasks/ subdirectory if it exists)
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
    fm.push_str(&format!("title: \"{}\"\n", fields.title.replace('"', "\\\"")));
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

    // Body
    let body = fields
        .body
        .unwrap_or_else(|| format!("# {}\n", fields.title));
    fm.push_str(&body);
    fm.push('\n');

    std::fs::write(&path, &fm)
        .with_context(|| format!("Failed to write task file: {}", path.display()))?;

    Ok(path)
}

/// Update frontmatter fields in an existing task file.
///
/// Reads the file, patches the YAML frontmatter, and rewrites it.
pub fn update_task(path: &Path, updates: HashMap<String, serde_json::Value>) -> Result<()> {
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

    // Rebuild the file
    let yaml = serde_yaml::to_string(&fm)
        .context("Failed to serialize frontmatter")?;
    let body = result.content.trim();

    let new_content = format!("---\n{}---\n\n{}\n", yaml, body);
    std::fs::write(path, &new_content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Generate a task ID from the title (e.g., "task-a1b2c3").
fn generate_task_id(title: &str) -> String {
    let hash = format!("{:x}", md5::compute(title.as_bytes()));
    format!("task-{}", &hash[..6])
}

/// Convert a title to a URL-safe slug.
fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
