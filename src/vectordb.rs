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
    /// Absolute file path
    pub path: PathBuf,
    /// Document title
    pub title: String,
    /// Document type
    pub doc_type: Option<String>,
    /// Document status
    pub status: Option<String>,
    /// Tags
    pub tags: Vec<String>,
    /// File modification time (unix timestamp) — used for staleness detection
    pub mtime: u64,
    /// Embedding vectors for each chunk of the document
    pub chunk_embeddings: Vec<Vec<f32>>,
    /// The text chunks that were embedded (for returning snippets)
    pub chunk_texts: Vec<String>,
}

/// Search result returned from queries
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
}

/// Persistent vector store
#[derive(Serialize, Deserialize)]
pub struct VectorStore {
    /// Map from absolute file path (string) to document entry
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

    /// Insert or update a document
    pub fn upsert(&mut self, doc: &PkbDocument, embedder: &embeddings::Embedder) -> Result<()> {
        let path_str = doc.path.to_string_lossy().to_string();

        let embedding_text = doc.embedding_text();
        let chunks = embeddings::chunk_text(&embedding_text, &embeddings::ChunkConfig::default());

        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let chunk_embeddings = embedder.encode_batch(&chunk_refs)?;

        let entry = DocumentEntry {
            path: doc.path.clone(),
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            mtime: doc.mtime,
            chunk_embeddings,
            chunk_texts: chunks,
        };

        self.documents.insert(path_str, entry);
        Ok(())
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

    /// Semantic search: find the top-k most similar documents to a query
    pub fn search(&self, query_embedding: &[f32], limit: usize) -> Vec<SearchResult> {
        // Build candidate list: for each document, use max similarity across chunks
        let mut results: Vec<SearchResult> = Vec::new();

        for entry in self.documents.values() {
            let mut best_score = f32::NEG_INFINITY;
            let mut best_snippet = String::new();

            for (i, chunk_emb) in entry.chunk_embeddings.iter().enumerate() {
                let score = distance::cosine_similarity(query_embedding, chunk_emb);
                if score > best_score {
                    best_score = score;
                    if i < entry.chunk_texts.len() {
                        // Truncate snippet for display
                        let text = &entry.chunk_texts[i];
                        let mut trunc = 300.min(text.len());
                        while trunc > 0 && !text.is_char_boundary(trunc) {
                            trunc -= 1;
                        }
                        best_snippet = format!("{}...", &text[..trunc]);
                    }
                }
            }

            if best_score > f32::NEG_INFINITY {
                results.push(SearchResult {
                    path: entry.path.clone(),
                    title: entry.title.clone(),
                    score: best_score,
                    snippet: best_snippet,
                    doc_type: entry.doc_type.clone(),
                    status: entry.status.clone(),
                    tags: entry.tags.clone(),
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

    /// List documents with optional filters
    pub fn list_documents(
        &self,
        tag_filter: Option<&str>,
        type_filter: Option<&str>,
        status_filter: Option<&str>,
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

            results.push(SearchResult {
                path: entry.path.clone(),
                title: entry.title.clone(),
                score: 0.0,
                snippet: String::new(),
                doc_type: entry.doc_type.clone(),
                status: entry.status.clone(),
                tags: entry.tags.clone(),
            });
        }

        results.sort_by(|a, b| a.title.cmp(&b.title));
        results
    }
}
