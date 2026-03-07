//! Shared library for pkb (MCP server) and aops (CLI) binaries.

pub mod distance;
pub mod document_crud;
pub mod embeddings;
pub mod graph;
pub mod layout;
pub mod graph_display;
pub mod graph_store;
pub mod lint;
pub mod mcp_server;
pub mod metrics;
pub mod pkb;
pub mod task_index;
pub mod vectordb;

use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Check whether the vector store index is stale.
///
/// Returns the number of documents that need re-indexing (new or modified).
/// Returns 0 if the index is fully up to date.
pub fn check_index_staleness(
    pkb_root: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
) -> usize {
    let files = pkb::scan_directory(pkb_root);
    let store = store.read();
    files
        .iter()
        .filter(|file_path| {
            let (path_str, content_hash) = rel_path_and_hash(pkb_root, file_path);
            store.needs_update(&path_str, &content_hash)
        })
        .count()
}

/// Compute relative path string and blake3 content hash for a file.
fn rel_path_and_hash(
    pkb_root: &std::path::Path,
    file_path: &std::path::Path,
) -> (String, String) {
    let rel_path = file_path.strip_prefix(pkb_root).unwrap_or(file_path);
    let path_str = rel_path.to_string_lossy().to_string();
    let content_hash = std::fs::read(file_path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .unwrap_or_default();
    (path_str, content_hash)
}

/// Index PKB files into the vector store. Returns (indexed, removed, total).
///
/// Uses batch-parallel embedding: all chunks from all new/modified documents are
/// collected and embedded in a single encode_batch call, which distributes work
/// across all available ONNX sessions and CPU cores via rayon.
pub fn index_pkb(
    pkb_root: &std::path::Path,
    _db_path: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
    embedder: &embeddings::Embedder,
    force_all: bool,
) -> (usize, usize, usize) {
    let files = pkb::scan_directory(pkb_root);
    tracing::info!(
        "Found {} markdown files in {}",
        files.len(),
        pkb_root.display()
    );

    let existing_paths: HashSet<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(pkb_root)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let removed = {
        let mut store = store.write();
        store.remove_deleted(&existing_paths)
    };

    let mut docs_to_index: Vec<pkb::PkbDocument> = Vec::new();
    let mut all_chunks: Vec<String> = Vec::new();
    let mut chunk_map: Vec<(usize, usize, usize)> = Vec::new();

    for file_path in &files {
        let (path_str, content_hash) = rel_path_and_hash(pkb_root, file_path);

        let needs_update = force_all || {
            let store = store.read();
            store.needs_update(&path_str, &content_hash)
        };

        if !needs_update {
            continue;
        }

        if let Some(doc) = pkb::parse_file_relative(file_path, pkb_root) {
            let embedding_text = doc.embedding_text();
            let chunks =
                embeddings::chunk_text(&embedding_text, &embeddings::ChunkConfig::default());
            let chunk_start = all_chunks.len();
            let chunk_count = chunks.len();
            all_chunks.extend(chunks);
            chunk_map.push((docs_to_index.len(), chunk_start, chunk_count));
            docs_to_index.push(doc);
        } else {
            tracing::debug!("Skipped (parse failed): {}", file_path.display());
        }
    }

    if docs_to_index.is_empty() {
        let total = store.read().len();
        tracing::info!("Indexing complete: 0 indexed, {removed} removed, {total} total");
        return (0, removed, total);
    }

    tracing::info!(
        "Embedding {} chunks from {} documents across all available cores...",
        all_chunks.len(),
        docs_to_index.len()
    );

    let chunk_refs: Vec<&str> = all_chunks.iter().map(|s| s.as_str()).collect();
    let all_embeddings = match embedder.encode_batch(&chunk_refs) {
        Ok(embs) => embs,
        Err(e) => {
            tracing::error!("Batch embedding failed: {e}");
            let total = store.read().len();
            return (0, removed, total);
        }
    };

    let indexed = docs_to_index.len();
    {
        let mut store = store.write();
        for &(doc_idx, chunk_start, chunk_count) in &chunk_map {
            let doc = &docs_to_index[doc_idx];
            let embeddings = all_embeddings[chunk_start..chunk_start + chunk_count].to_vec();
            let chunks = all_chunks[chunk_start..chunk_start + chunk_count].to_vec();
            store.insert_precomputed(doc, chunks, embeddings);
            tracing::debug!("Indexed: {}", doc.title);
        }
    }

    let total = store.read().len();
    tracing::info!("Indexing complete: {indexed} indexed, {removed} removed, {total} total");

    (indexed, removed, total)
}
