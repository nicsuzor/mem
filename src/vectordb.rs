//! Persistent vector store for PKB semantic search.
//!
//! Simple brute-force cosine similarity search, persisted to disk via bincode.
//! Sufficient for <10k documents typical of a personal knowledge base.

use crate::distance;
use crate::embeddings;
use crate::pkb::PkbDocument;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A stored document entry with its embeddings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentEntry {
    /// File path (relative to pkb_root for portability)
    pub path: PathBuf,
    /// Document title
    pub title: String,
    /// Document type
    pub doc_type: Option<String>,
    /// Document status
    pub status: Option<String>,
    /// Tags
    pub tags: Vec<String>,
    /// Project name (from frontmatter)
    #[serde(default)]
    pub project: Option<String>,
    /// Document ID (from frontmatter)
    #[serde(default)]
    pub id: Option<String>,
    /// Content hash (blake3, hex-encoded) — used for staleness detection
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Embedding vectors for each chunk of the document
    pub chunk_embeddings: Vec<Vec<f32>>,
    /// The text chunks that were embedded (for returning snippets)
    pub chunk_texts: Vec<String>,
    /// Body-only text chunks for display snippets (excludes frontmatter metadata)
    #[serde(default)]
    pub body_chunks: Vec<String>,
}

/// Search result returned from queries
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub id: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub project: Option<String>,
}

/// Persistent vector store
#[derive(Serialize, Deserialize)]
pub struct VectorStore {
    /// Map from file path (relative to pkb_root) to document entry
    documents: HashMap<String, DocumentEntry>,
    /// Embedding dimension
    dimension: usize,
}

impl VectorStore {
    pub fn new(dimension: usize) -> Self {
        Self {
            documents: HashMap::new(),
            dimension,
        }
    }

    /// Load from disk, or create new if file doesn't exist.
    /// Detects dimension mismatch (e.g. model upgrade) and creates a fresh store.
    pub fn load_or_create(path: &Path, dimension: usize) -> Result<Self> {
        if path.exists() {
            tracing::info!("Loading vector store from {path:?}");
            let data = std::fs::read(path)?;
            match bincode::deserialize::<VectorStore>(&data) {
                Ok(store) => {
                    if store.dimension != dimension {
                        tracing::warn!(
                            "Vector store dimension mismatch: stored={}, expected={}. \
                             Creating fresh store (full reindex required).",
                            store.dimension, dimension
                        );
                        Ok(Self::new(dimension))
                    } else {
                        tracing::info!("Loaded {} documents from store", store.documents.len());
                        Ok(store)
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to deserialize vector store: {e}. Creating new.");
                    Ok(Self::new(dimension))
                }
            }
        } else {
            tracing::info!("No existing vector store found. Creating new.");
            Ok(Self::new(dimension))
        }
    }

    /// Save to disk
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let data = bincode::serialize(self)?;
        // Atomic write: write to temp file then rename
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &data)?;
        std::fs::rename(&tmp_path, path)?;

        tracing::info!(
            "Saved vector store ({} documents, {:.1} MB)",
            self.documents.len(),
            data.len() as f64 / 1_048_576.0
        );
        Ok(())
    }

    /// Number of indexed documents
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Check if a document needs re-indexing based on content hash
    pub fn needs_update(&self, path: &str, content_hash: &str) -> bool {
        match self.documents.get(path) {
            Some(entry) => {
                // If entry has no hash (old format), needs update
                match &entry.content_hash {
                    Some(stored_hash) => stored_hash != content_hash,
                    None => true,
                }
            }
            None => true,
        }
    }

    /// Extract id and project from frontmatter
    fn extract_frontmatter_fields(doc: &PkbDocument) -> (Option<String>, Option<String>) {
        let fm = doc.frontmatter.as_ref();
        let id = fm.and_then(|f| f.get("id").and_then(|v| v.as_str()).map(String::from));
        let project = fm.and_then(|f| f.get("project").and_then(|v| v.as_str()).map(String::from));
        (id, project)
    }

    /// Insert or update a document
    pub fn upsert(&mut self, doc: &PkbDocument, embedder: &embeddings::Embedder) -> Result<()> {
        let path_str = doc.path.to_string_lossy().to_string();

        let embedding_text = doc.embedding_text();
        let chunks = embeddings::chunk_text(&embedding_text, &embeddings::ChunkConfig::default());
        let body_chunks = embeddings::chunk_text(doc.body.trim(), &embeddings::ChunkConfig::default());

        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let chunk_embeddings = embedder.encode_batch(&chunk_refs)?;

        let (id, project) = Self::extract_frontmatter_fields(doc);

        let entry = DocumentEntry {
            path: doc.path.clone(),
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            project,
            id,
            content_hash: Some(doc.content_hash.clone()),
            chunk_embeddings,
            chunk_texts: chunks,
            body_chunks,
        };

        self.documents.insert(path_str, entry);
        Ok(())
    }

    /// Insert a document with pre-computed embeddings (no embedding call needed)
    pub fn insert_precomputed(&mut self, doc: &PkbDocument, chunks: Vec<String>, chunk_embeddings: Vec<Vec<f32>>) {
        let path_str = doc.path.to_string_lossy().to_string();
        let (id, project) = Self::extract_frontmatter_fields(doc);
        let body_chunks = embeddings::chunk_text(doc.body.trim(), &embeddings::ChunkConfig::default());
        let entry = DocumentEntry {
            path: doc.path.clone(),
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            project,
            id,
            content_hash: Some(doc.content_hash.clone()),
            chunk_embeddings,
            chunk_texts: chunks,
            body_chunks,
        };
        self.documents.insert(path_str, entry);
    }

    /// Remove a single document by its absolute path string.
    ///
    /// Returns true if the document was found and removed.
    pub fn remove(&mut self, path: &str) -> bool {
        self.documents.remove(path).is_some()
    }

    /// Remove documents whose files no longer exist
    pub fn remove_deleted(&mut self, existing_paths: &std::collections::HashSet<String>) -> usize {
        let before = self.documents.len();
        self.documents.retain(|path, _| existing_paths.contains(path));
        let removed = before - self.documents.len();
        if removed > 0 {
            tracing::info!("Removed {removed} deleted documents from index");
        }
        removed
    }

    /// Semantic search: find the top-k most similar documents to a query.
    ///
    /// Returned `SearchResult.path` values are reconstructed as absolute paths
    /// by joining with `pkb_root`.
    pub fn search(&self, query_embedding: &[f32], limit: usize, pkb_root: &Path) -> Vec<SearchResult> {
        // Build candidate list: for each document, use max similarity across chunks
        let mut results: Vec<SearchResult> = Vec::new();

        for entry in self.documents.values() {
            let mut best_score = f32::NEG_INFINITY;
            let mut best_chunk_idx = 0usize;

            for (i, chunk_emb) in entry.chunk_embeddings.iter().enumerate() {
                let score = distance::cosine_similarity(query_embedding, chunk_emb);
                if score > best_score {
                    best_score = score;
                    best_chunk_idx = i;
                }
            }

            if best_score > f32::NEG_INFINITY {
                // Use chunk_texts for snippet to ensure alignment with embedding index
                let snippet_source = &entry.chunk_texts;
                let best_snippet = if best_chunk_idx < snippet_source.len() {
                    let text = &snippet_source[best_chunk_idx];
                    let mut trunc = 300.min(text.len());
                    while trunc > 0 && !text.is_char_boundary(trunc) {
                        trunc -= 1;
                    }
                    text[..trunc].to_string()
                } else if !snippet_source.is_empty() {
                    let text = &snippet_source[0];
                    let mut trunc = 300.min(text.len());
                    while trunc > 0 && !text.is_char_boundary(trunc) {
                        trunc -= 1;
                    }
                    text[..trunc].to_string()
                } else {
                    String::new()
                };

                let abs_path = if entry.path.is_absolute() {
                    entry.path.clone()
                } else {
                    pkb_root.join(&entry.path)
                };
                results.push(SearchResult {
                    path: abs_path,
                    title: entry.title.clone(),
                    score: best_score,
                    snippet: best_snippet,
                    id: entry.id.clone(),
                    doc_type: entry.doc_type.clone(),
                    status: entry.status.clone(),
                    tags: entry.tags.clone(),
                    project: entry.project.clone(),
                });
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score).cmp(&ordered_float::OrderedFloat(a.score))
        });

        results.truncate(limit);
        results
    }

    /// List documents with optional filters.
    ///
    /// Returned paths are reconstructed as absolute by joining with `pkb_root`.
    pub fn list_documents(
        &self,
        tag_filter: Option<&str>,
        type_filter: Option<&str>,
        status_filter: Option<&str>,
        project_filter: Option<&str>,
        pkb_root: &Path,
    ) -> Vec<SearchResult> {
        let mut results: Vec<SearchResult> = Vec::new();

        for entry in self.documents.values() {
            // Apply filters
            if let Some(tag) = tag_filter {
                if !entry.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                    continue;
                }
            }
            if let Some(dt) = type_filter {
                match &entry.doc_type {
                    Some(t) if t.eq_ignore_ascii_case(dt) => {}
                    _ => continue,
                }
            }
            if let Some(st) = status_filter {
                match &entry.status {
                    Some(s) if s.eq_ignore_ascii_case(st) => {}
                    _ => continue,
                }
            }
            if let Some(proj) = project_filter {
                match &entry.project {
                    Some(p) if p.eq_ignore_ascii_case(proj) => {}
                    _ => continue,
                }
            }

            let abs_path = if entry.path.is_absolute() {
                entry.path.clone()
            } else {
                pkb_root.join(&entry.path)
            };
            results.push(SearchResult {
                path: abs_path,
                title: entry.title.clone(),
                score: 0.0,
                snippet: String::new(),
                id: entry.id.clone(),
                doc_type: entry.doc_type.clone(),
                status: entry.status.clone(),
                tags: entry.tags.clone(),
                project: entry.project.clone(),
            });
        }

        results.sort_by(|a, b| a.title.cmp(&b.title));
        results
    }

    /// List all tags across all documents with their occurrence counts.
    pub fn list_all_tags(&self) -> HashMap<String, usize> {
        let mut tags: HashMap<String, usize> = HashMap::new();
        for entry in self.documents.values() {
            for tag in &entry.tags {
                let normalized = tag.to_lowercase();
                *tags.entry(normalized).or_insert(0) += 1;
            }
        }
        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper to create a DocumentEntry without needing an Embedder.
    fn make_entry(
        path: &str,
        title: &str,
        doc_type: Option<&str>,
        status: Option<&str>,
        tags: &[&str],
        project: Option<&str>,
        id: Option<&str>,
        embedding: Vec<f32>,
    ) -> DocumentEntry {
        DocumentEntry {
            path: PathBuf::from(path),
            title: title.to_string(),
            doc_type: doc_type.map(String::from),
            status: status.map(String::from),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            project: project.map(String::from),
            id: id.map(String::from),
            content_hash: Some("test_hash_123".to_string()),
            chunk_embeddings: vec![embedding],
            chunk_texts: vec![format!("chunk text for {title}")],
            body_chunks: vec![format!("body of {title}")],
        }
    }

    fn build_test_store() -> VectorStore {
        let mut store = VectorStore::new(3);
        store.documents.insert(
            "tasks/task-abc.md".to_string(),
            make_entry(
                "tasks/task-abc.md",
                "Fix the bug",
                Some("task"),
                Some("active"),
                &["bugfix", "urgent"],
                Some("mem"),
                Some("task-abc"),
                vec![1.0, 0.0, 0.0],
            ),
        );
        store.documents.insert(
            "memories/mem-001.md".to_string(),
            make_entry(
                "memories/mem-001.md",
                "Important insight",
                Some("memory"),
                None,
                &["pattern", "urgent"],
                None,
                Some("mem-001"),
                vec![0.0, 1.0, 0.0],
            ),
        );
        store.documents.insert(
            "notes/note-xyz.md".to_string(),
            make_entry(
                "notes/note-xyz.md",
                "Research notes",
                Some("note"),
                None,
                &["research"],
                Some("mem"),
                Some("note-xyz"),
                vec![0.0, 0.0, 1.0],
            ),
        );
        store
    }

    // ── list_all_tags ──

    #[test]
    fn test_list_all_tags_counts() {
        let store = build_test_store();
        let tags = store.list_all_tags();
        assert_eq!(tags.get("urgent"), Some(&2)); // appears in task + memory
        assert_eq!(tags.get("bugfix"), Some(&1));
        assert_eq!(tags.get("research"), Some(&1));
        assert_eq!(tags.get("pattern"), Some(&1));
        assert_eq!(tags.len(), 4);
    }

    #[test]
    fn test_list_all_tags_empty_store() {
        let store = VectorStore::new(3);
        let tags = store.list_all_tags();
        assert!(tags.is_empty());
    }

    // ── list_documents filters ──

    #[test]
    fn test_list_documents_no_filters() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, None, None, None, root);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_list_documents_filter_by_tag() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("urgent"), None, None, None, root);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_list_documents_filter_by_type() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, Some("task"), None, None, root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fix the bug");
    }

    #[test]
    fn test_list_documents_filter_by_type_case_insensitive() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, Some("TASK"), None, None, root);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_list_documents_filter_by_status() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, None, Some("active"), None, root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fix the bug");
    }

    #[test]
    fn test_list_documents_filter_by_project() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, None, None, Some("mem"), root);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_list_documents_combined_filters() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("urgent"), Some("memory"), None, None, root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Important insight");
    }

    #[test]
    fn test_list_documents_no_match() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("nonexistent"), None, None, None, root);
        assert!(results.is_empty());
    }

    // ── search ──

    #[test]
    fn test_search_returns_best_match() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        // Query vector aligned with [1,0,0] should match the task entry
        let results = store.search(&[1.0, 0.0, 0.0], 10, root);
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Fix the bug");
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_search_respects_limit() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.5], 2, root);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_sorted_by_score_descending() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.0], 10, root);
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score);
        }
    }

    #[test]
    fn test_search_empty_store() {
        let store = VectorStore::new(3);
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 10, root);
        assert!(results.is_empty());
    }

    // ── remove / remove_deleted ──

    #[test]
    fn test_remove_existing() {
        let mut store = build_test_store();
        assert_eq!(store.len(), 3);
        assert!(store.remove("tasks/task-abc.md"));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut store = build_test_store();
        assert!(!store.remove("nonexistent.md"));
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn test_remove_deleted() {
        let mut store = build_test_store();
        let mut existing = std::collections::HashSet::new();
        existing.insert("tasks/task-abc.md".to_string());
        // Only keep task-abc, remove the other two
        let removed = store.remove_deleted(&existing);
        assert_eq!(removed, 2);
        assert_eq!(store.len(), 1);
    }

    // ── needs_update ──

    #[test]
    fn test_needs_update_new_doc() {
        let store = build_test_store();
        assert!(store.needs_update("new-doc.md", "any_hash"));
    }

    #[test]
    fn test_needs_update_stale() {
        let store = build_test_store();
        // Stored hash is "test_hash_123", different hash means changed
        assert!(store.needs_update("tasks/task-abc.md", "different_hash"));
    }

    #[test]
    fn test_needs_update_fresh() {
        let store = build_test_store();
        // Stored hash is "test_hash_123", same hash means unchanged
        assert!(!store.needs_update("tasks/task-abc.md", "test_hash_123"));
    }

    #[test]
    fn test_needs_update_migration_old_format() {
        // Test migration: old format documents without content_hash should need update
        let mut store = VectorStore::new(3);
        let mut entry = make_entry(
            "tasks/old-task.md",
            "Old Task",
            Some("task"),
            None,
            &[],
            None,
            None,
            vec![1.0, 0.0, 0.0],
        );
        // Simulate old format by removing content_hash
        entry.content_hash = None;
        store.documents.insert("tasks/old-task.md".to_string(), entry);

        // Document with no hash should always need update
        assert!(store.needs_update("tasks/old-task.md", "any_hash"));
    }

    // ── persistence ──

    #[test]
    fn test_save_and_load() {
        let store = build_test_store();
        let dir = std::env::temp_dir().join("mem_vectordb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_store.bin");

        store.save(&path).unwrap();
        let loaded = VectorStore::load_or_create(&path, 3).unwrap();
        assert_eq!(loaded.len(), 3);
        let tags = loaded.list_all_tags();
        assert_eq!(tags.get("urgent"), Some(&2));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_nonexistent_creates_new() {
        let path = Path::new("/tmp/mem_test_nonexistent.bin");
        let _ = std::fs::remove_file(path); // ensure clean
        let store = VectorStore::load_or_create(path, 384).unwrap();
        assert_eq!(store.len(), 0);
    }

    // ── insert_precomputed ──

    #[test]
    fn test_insert_precomputed() {
        let mut store = VectorStore::new(3);
        let doc = crate::pkb::PkbDocument {
            path: PathBuf::from("test.md"),
            title: "Test Doc".to_string(),
            body: "Some body text".to_string(),
            doc_type: Some("note".to_string()),
            status: None,
            modified: None,
            tags: vec!["test".to_string()],
            frontmatter: None,
            content_hash: "test_doc_hash".to_string(),
        };
        store.insert_precomputed(&doc, vec!["chunk1".to_string()], vec![vec![1.0, 0.0, 0.0]]);
        assert_eq!(store.len(), 1);
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 1, root);
        assert_eq!(results[0].title, "Test Doc");
    }
    #[test]
    fn test_search_uses_chunk_texts_for_snippets() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 1, root);
        assert!(results[0].snippet.contains("chunk text for"));
    }
}
