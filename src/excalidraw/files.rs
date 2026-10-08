//! Raw `.excalidraw` canvas files stored in the PKB (`mem_24d027d4`).
//!
//! Canvases are addressed by PKB-relative path, never by graph id: they carry no
//! frontmatter, are not graph nodes, and are deliberately kept out of the
//! markdown scan ([`crate::pkb::scan_directory`]) that feeds the graph, BM25 and
//! vector index — canvas JSON is geometry, not prose worth embedding.
//!
//! Path rules (shared by list/read/write): relative, no `..`, no hidden
//! components, `.excalidraw` extension, and — after resolving symlinks — still
//! inside the PKB root. Writes are gated by the same shape + structural
//! validation as `parse_canvas`, then written atomically.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use super::schema::ExcalidrawFile;
use super::validate::{validate_file, validate_raw_shape};

pub const CANVAS_EXTENSION: &str = "excalidraw";

/// Most warnings echoed back by [`write_canvas`]; real canvases carry dozens.
pub const MAX_LISTED_WARNINGS: usize = 10;

/// Caller errors (bad path, bad content, missing file) vs. I/O failures.
#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Serialize)]
pub struct CanvasEntry {
    pub path: String,
    pub bytes: u64,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WriteOutcome {
    pub path: String,
    pub bytes: u64,
    pub created: bool,
    /// Number of non-blocking validation findings (stale backrefs, text drift).
    pub warning_count: usize,
    /// The first [`MAX_LISTED_WARNINGS`] of those findings.
    pub warnings: Vec<String>,
}

/// Check a caller-supplied relative path lexically. `require_ext` is false for
/// directory prefixes.
fn check_relative(rel: &str, require_ext: bool) -> Result<PathBuf, CanvasError> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(CanvasError::Invalid("path must not be empty".into()));
    }
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => {}
            Component::CurDir => {}
            _ => return Err(CanvasError::Invalid(format!(
                "path {rel:?} must be relative to the PKB root, without '..' or hidden components"
            ))),
        }
    }
    if require_ext && p.extension().and_then(|e| e.to_str()) != Some(CANVAS_EXTENSION) {
        return Err(CanvasError::Invalid(format!(
            "path {rel:?} must end in .{CANVAS_EXTENSION}"
        )));
    }
    Ok(p.components().collect())
}

/// Reject a resolved path that a symlink has carried outside the root.
/// Checks the nearest existing ancestor, so it works for not-yet-created files.
fn check_contained(root: &Path, abs: &Path, rel: &str) -> Result<(), CanvasError> {
    let root = root
        .canonicalize()
        .map_err(|e| CanvasError::Io(format!("cannot resolve PKB root: {e}")))?;
    let mut probe = abs;
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p,
            None => break,
        }
    }
    let real = probe
        .canonicalize()
        .map_err(|e| CanvasError::Io(format!("cannot resolve {rel:?}: {e}")))?;
    if !real.starts_with(&root) {
        return Err(CanvasError::Invalid(format!(
            "path {rel:?} resolves outside the PKB root"
        )));
    }
    Ok(())
}

fn rel_string(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn modified_rfc3339(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

/// List canvases under `root` (optionally under the `dir` subdirectory),
/// honouring the same ignore rules as the markdown scan. Sorted by path.
pub fn list_canvases(root: &Path, dir: Option<&str>) -> Result<Vec<CanvasEntry>, CanvasError> {
    let base = match dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            let abs = root.join(check_relative(d, false)?);
            check_contained(root, &abs, d)?;
            if !abs.is_dir() {
                return Ok(Vec::new());
            }
            abs
        }
        None => root.to_path_buf(),
    };
    let mut out: Vec<CanvasEntry> =
        crate::pkb::scan_directory_with_extension(&base, CANVAS_EXTENSION)
            .into_iter()
            .filter_map(|abs| {
                let meta = std::fs::metadata(&abs).ok()?;
                Some(CanvasEntry {
                    path: rel_string(root, &abs),
                    bytes: meta.len(),
                    modified: modified_rfc3339(&meta),
                })
            })
            .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Read a canvas verbatim.
pub fn read_canvas(root: &Path, rel: &str) -> Result<String, CanvasError> {
    let abs = root.join(check_relative(rel, true)?);
    if !abs.is_file() {
        return Err(CanvasError::Invalid(format!("canvas not found: {rel}")));
    }
    check_contained(root, &abs, rel)?;
    std::fs::read_to_string(&abs).map_err(|e| CanvasError::Io(format!("failed to read {rel}: {e}")))
}

/// Validate `content` as an Excalidraw scene and write it atomically to `rel`,
/// creating parent directories. Overwrites an existing canvas.
pub fn write_canvas(root: &Path, rel: &str, content: &str) -> Result<WriteOutcome, CanvasError> {
    let rel_path = check_relative(rel, true)?;
    let abs = root.join(&rel_path);
    check_contained(root, &abs, rel)?;

    validate_raw_shape(content).map_err(|f| {
        CanvasError::Invalid(format!(
            "content is not an Excalidraw scene: {}",
            f.join("; ")
        ))
    })?;
    let file: ExcalidrawFile = serde_json::from_str(content)
        .map_err(|e| CanvasError::Invalid(format!("content is not an Excalidraw scene: {e}")))?;
    let mut warnings = validate_file(&file).map_err(|f| {
        CanvasError::Invalid(format!(
            "canvas failed structural validation: {}",
            f.join("; ")
        ))
    })?;

    if abs.is_dir() {
        return Err(CanvasError::Invalid(format!("{rel} is a directory")));
    }
    let created = !abs.exists();
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CanvasError::Io(format!("failed to create {}: {e}", parent.display())))?;
    }
    crate::document_crud::atomic_write_file(&abs, content, "write_excalidraw")
        .map_err(|e| CanvasError::Io(format!("{e:#}")))?;

    let warning_count = warnings.len();
    warnings.truncate(MAX_LISTED_WARNINGS);
    Ok(WriteOutcome {
        path: rel_string(root, &abs),
        bytes: content.len() as u64,
        created,
        warning_count,
        warnings,
    })
}
