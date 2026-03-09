//! Batch operations for the task graph.
//!
//! Provides [`BatchContext`] for deferred-rebuild batch mutations, and
//! individual operation modules for update, reparent, archive, and stats.

pub mod filters;
pub mod reparent;
pub mod stats;
pub mod update;

use crate::document_crud;
use crate::graph_store::GraphStore;
use crate::pkb;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Result of a single task operation within a batch.
#[derive(Debug, Clone, Serialize)]
pub struct TaskAction {
    pub id: String,
    pub title: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
}

/// Error for a single task within a batch (non-fatal).
#[derive(Debug, Clone, Serialize)]
pub struct TaskError {
    pub id: String,
    pub error: String,
}

/// Summary returned by all batch operations.
#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    pub operation: String,
    pub matched: usize,
    pub changed: usize,
    pub skipped: usize,
    pub tasks: Vec<TaskAction>,
    pub errors: Vec<TaskError>,
    pub dry_run: bool,
}

impl BatchSummary {
    pub fn new(operation: &str, dry_run: bool) -> Self {
        Self {
            operation: operation.to_string(),
            matched: 0,
            changed: 0,
            skipped: 0,
            tasks: Vec::new(),
            errors: Vec::new(),
            dry_run,
        }
    }

    /// Format as a human-readable summary table.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.dry_run {
            out.push_str("DRY RUN — no files modified. Pass --execute to apply.\n\n");
        }
        out.push_str(&format!(
            "Batch {}: {} matched, {} changed, {} skipped",
            self.operation, self.matched, self.changed, self.skipped
        ));
        if !self.errors.is_empty() {
            out.push_str(&format!(", {} errors", self.errors.len()));
        }
        out.push('\n');

        if !self.tasks.is_empty() {
            out.push('\n');
            // Header
            out.push_str(&format!(
                "  {:<24} {:<48} {:<12} {}\n",
                "ID", "Title", "Action", "Detail"
            ));
            out.push_str(&format!("  {}\n", "─".repeat(96)));

            for task in &self.tasks {
                let title = if task.title.len() > 46 {
                    let boundary = task.title.floor_char_boundary(43);
                    format!("{}...", &task.title[..boundary])
                } else {
                    task.title.clone()
                };
                let detail = task.detail.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "  {:<24} {:<48} {:<12} {}\n",
                    task.id, title, task.action, detail
                ));
            }
        }

        if !self.errors.is_empty() {
            out.push_str("\nErrors:\n");
            for err in &self.errors {
                out.push_str(&format!("  {} — {}\n", err.id, err.error));
            }
        }

        out
    }
}

/// Context for batch operations with deferred graph rebuild.
///
/// Accumulates file modifications and performs a single graph rebuild at the end.
pub struct BatchContext<'a> {
    pub graph: &'a GraphStore,
    pub pkb_root: &'a Path,
    modified_paths: Vec<PathBuf>,
}

impl<'a> BatchContext<'a> {
    pub fn new(graph: &'a GraphStore, pkb_root: &'a Path) -> Self {
        Self {
            graph,
            pkb_root,
            modified_paths: Vec::new(),
        }
    }

    /// Update frontmatter on a single task file. Records the path for later rebuild.
    pub fn update_task(
        &mut self,
        node_id: &str,
        updates: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let node = self
            .graph
            .get_node(node_id)
            .context(format!("Node not found: {node_id}"))?;

        let abs_path = self.abs_path(&node.path);
        if !abs_path.exists() {
            anyhow::bail!("File not found on disk (index stale?): {}", abs_path.display());
        }

        document_crud::update_document(&abs_path, updates)?;
        self.modified_paths.push(abs_path);
        Ok(())
    }

    /// Append content to a task file body. Records the path for later rebuild.
    pub fn append_to_task(
        &mut self,
        node_id: &str,
        content: &str,
    ) -> Result<()> {
        let node = self
            .graph
            .get_node(node_id)
            .context(format!("Node not found: {node_id}"))?;

        let abs_path = self.abs_path(&node.path);
        document_crud::append_to_document(&abs_path, content, None)?;
        self.modified_paths.push(abs_path);
        Ok(())
    }

    /// Resolve a relative path to absolute.
    fn abs_path(&self, rel: &Path) -> PathBuf {
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.pkb_root.join(rel)
        }
    }

    /// Paths modified during this batch (for re-indexing).
    pub fn modified_paths(&self) -> &[PathBuf] {
        &self.modified_paths
    }

    /// Re-embed modified documents and rebuild graph.
    /// Returns the new GraphStore.
    pub fn rebuild(self) -> GraphStore {
        GraphStore::build_from_directory(self.pkb_root)
    }

    /// Re-embed modified documents into vector store, then rebuild graph.
    /// This is the full finalize path when you have a vector store available.
    pub fn finalize(
        self,
        store: &parking_lot::RwLock<crate::vectordb::VectorStore>,
        embedder: &crate::embeddings::Embedder,
        db_path: &Path,
    ) -> Result<GraphStore> {
        // Re-embed modified files
        for path in &self.modified_paths {
            if let Some(doc) = pkb::parse_file_relative(path, self.pkb_root) {
                store.write().upsert(&doc, embedder)?;
            }
        }
        // Save vector store
        store.read().save(db_path)?;
        // Rebuild graph
        Ok(GraphStore::build_from_directory(self.pkb_root))
    }
}
