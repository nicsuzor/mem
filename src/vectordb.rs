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
    /// File modification time (unix timestamp) — used for staleness detection
    pub mtime: u64,
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

    /// Load from disk, or create new if file doesn't exist
    pub fn load_or_create(path: &Path, dimension: usize) -> Result<Self> {
        if path.exists() {
            tracing::info!("Loading vector store from {path:?}");
            let data = std::fs::read(path)?;
            match bincode::deserialize::<VectorStore>(&data) {
                Ok(store) => {
                    tracing::info!("Loaded {} documents from store", store.documents.len());
                    Ok(store)
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

    /// Check if a document needs re-indexing based on mtime
    pub fn needs_update(&self, path: &str, mtime: u64) -> bool {
        match self.documents.get(path) {
            Some(entry) => entry.mtime < mtime,
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
            mtime: doc.mtime,
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
            mtime: doc.mtime,
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
                // Use body-only chunks for snippet (falls back to embedding chunks for old indexes)
                let snippet_source = if !entry.body_chunks.is_empty() {
                    &entry.body_chunks
                } else {
                    &entry.chunk_texts
                };
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
}
