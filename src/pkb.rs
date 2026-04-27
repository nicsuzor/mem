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

/// Parsed document from the PKB
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PkbDocument {
    /// File path (relative to pkb_root when persisted, absolute at parse time)
    pub path: PathBuf,
    /// Document title (from frontmatter or filename)
    pub title: String,
    /// Tags extracted from frontmatter and inline hashtags
    pub tags: Vec<String>,
    /// Document type (from frontmatter "type" field)
    pub doc_type: Option<String>,
    /// Document status
    pub status: Option<String>,
    /// Last modified time (RFC3339)
    pub modified: Option<String>,
    /// Full body content (without frontmatter)
    pub body: String,
    /// Content hash (blake3, hex-encoded) for change detection
    pub content_hash: String,
    /// Frontmatter fields as JSON for metadata queries
    pub frontmatter: Option<serde_json::Value>,
}

impl PkbDocument {
    /// Hash of the body (markdown content only, no YAML frontmatter).
    /// Used to detect whether re-embedding is needed — frontmatter-only
    /// changes (status, priority, etc.) leave this hash unchanged.
    pub fn body_hash(&self) -> String {
        blake3::hash(self.body.as_bytes()).to_hex().to_string()
    }

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

/// Compute the blake3 content hash of a file
fn compute_content_hash(content: &[u8]) -> String {
    blake3::hash(content).to_hex().to_string()
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
    // Read file as bytes for hash computation, then convert to string
    let content_bytes = std::fs::read(path).ok()?;
    let content_hash = compute_content_hash(&content_bytes);
    let content = String::from_utf8(content_bytes).ok()?;

    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

    let matter = Matter::<YAML>::new();
    let result = matter.parse(&content);

    let fm_data = result
        .data
        .as_ref()
        .and_then(|d| d.deserialize::<serde_json::Value>().ok());

    // Title: frontmatter > filename
    let mut title = path.file_stem()?.to_string_lossy().to_string();
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

    Some(PkbDocument {
        path: path.to_path_buf(),
        title,
        tags,
        doc_type,
        status,
        modified,
        body: result.content.trim().to_string(),
        content_hash,
        frontmatter: fm_data,
    })
}

/// Parse a file and store a path relative to `pkb_root` (for portable persistence).
pub fn parse_file_relative(path: &Path, pkb_root: &Path) -> Option<PkbDocument> {
    let mut doc = parse_file(path)?;
    doc.path = path.strip_prefix(pkb_root).unwrap_or(path).to_path_buf();
    Some(doc)
}

/// Scan a directory for ALL markdown files (ignoring .gitignore).
///
/// Used by graph building to capture all tasks regardless of git status.
pub fn scan_directory_all(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
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
                ".git"
                    | ".obsidian"
                    | ".venv"
                    | "node_modules"
                    | ".claude"
                    | ".aops"
                    | "__pycache__"
                    | ".agent"
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
