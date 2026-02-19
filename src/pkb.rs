//! PKB file parsing and scanning.
//!
//! Reads markdown files with YAML frontmatter from the personal knowledge base,
//! extracts metadata and content for embedding.

use gray_matter::engine::YAML;
use gray_matter::Matter;
use ignore::WalkBuilder;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Parsed document from the PKB
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PkbDocument {
    /// Absolute file path
    pub path: PathBuf,
    /// Document title (from frontmatter or filename)
    pub title: String,
    /// Tags extracted from frontmatter and inline hashtags
    pub tags: Vec<String>,
    /// Document type (from frontmatter "type" field)
    pub doc_type: Option<String>,
    /// Document status
    pub status: Option<String>,
    /// Full body content (without frontmatter)
    pub body: String,
    /// File modification time (unix timestamp)
    pub mtime: u64,
    /// Frontmatter fields as JSON for metadata queries
    pub frontmatter: Option<serde_json::Value>,
}

impl PkbDocument {
    /// Build a text representation suitable for embedding.
    /// Combines title, tags, and body for semantic richness.
    pub fn embedding_text(&self) -> String {
        let mut parts = Vec::new();

        // Title gets repeated for emphasis
        parts.push(self.title.clone());

        if let Some(ref dt) = self.doc_type {
            parts.push(format!("type: {dt}"));
        }

        if !self.tags.is_empty() {
            parts.push(format!("tags: {}", self.tags.join(", ")));
        }

        if !self.body.is_empty() {
            parts.push(self.body.clone());
        }

        parts.join("\n\n")
    }
}

/// Get the modification time of a file as a unix timestamp
fn get_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Extract tags from frontmatter and inline hashtags in content
fn extract_tags(frontmatter: &Option<serde_json::Value>, content: &str) -> Vec<String> {
    let mut tags = HashSet::new();

    // Frontmatter tags
    if let Some(fm) = frontmatter {
        if let Some(tag_val) = fm.get("tags") {
            if let Some(arr) = tag_val.as_array() {
                for t in arr {
                    if let Some(s) = t.as_str() {
                        tags.insert(s.to_string());
                    }
                }
            } else if let Some(s) = tag_val.as_str() {
                for part in s.split(',') {
                    tags.insert(part.trim().to_string());
                }
            }
        }
    }

    // Inline hashtags
    let re = Regex::new(r"(?:^|\s)#([a-zA-Z0-9_\-]+)").unwrap();
    for cap in re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            tags.insert(m.as_str().to_string());
        }
    }

    tags.into_iter().collect()
}

/// Parse a single markdown file into a PkbDocument
pub fn parse_file(path: &Path) -> Option<PkbDocument> {
    let content = std::fs::read_to_string(path).ok()?;
    let matter = Matter::<YAML>::new();
    let result = matter.parse(&content);

    let fm_data = result
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok());

    // Title: frontmatter > filename
    let mut title = path
        .file_stem()?
        .to_string_lossy()
        .to_string();
    if let Some(ref fm) = fm_data {
        if let Some(t) = fm.get("title").and_then(|v| v.as_str()) {
            title = t.to_string();
        }
    }

    let tags = extract_tags(&fm_data, &result.content);

    let doc_type = fm_data
        .as_ref()
        .and_then(|fm| fm.get("type").and_then(|v| v.as_str()).map(String::from));

    let status = fm_data
        .as_ref()
        .and_then(|fm| fm.get("status").and_then(|v| v.as_str()).map(String::from));

    let mtime = get_mtime(path);

    Some(PkbDocument {
        path: path.to_path_buf(),
        title,
        tags,
        doc_type,
        status,
        body: result.content.trim().to_string(),
        mtime,
        frontmatter: fm_data,
    })
}

/// Scan a directory for markdown files, respecting .gitignore
pub fn scan_directory(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(true) // skip hidden dirs like .git, .obsidian
        .git_ignore(true)
        .git_global(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            // Skip common non-content directories
            !matches!(
                name.as_ref(),
                ".git" | ".obsidian" | ".venv" | "node_modules" | ".claude" | ".aops"
                    | "__pycache__" | ".agent"
            )
        })
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "md" {
                    paths.push(path.to_path_buf());
                }
            }
        }
    }

    paths
}
