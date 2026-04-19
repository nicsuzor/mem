//! Shared library for the pkb binary (CLI + MCP server).
//!
//! stdout is reserved for MCP JSON-RPC when running as a server.
//! All diagnostics must go to stderr via `tracing` or `eprintln!`.
//! Library code must never write to stdout directly.
#![deny(clippy::print_stdout)]

pub mod batch_ops;
pub mod distance;
pub mod document_crud;
pub mod embeddings;
pub mod eval;
pub mod graph;
pub mod graph_display;
pub mod graph_store;
pub mod lint;
pub mod mcp_server;
pub mod metrics;
pub mod pkb;
pub mod task_index;
pub mod telemetry;
pub mod vectordb;

#[cfg(test)]
mod reproduction;

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

    // Process in batches of 20 docs with incremental saves for recoverability
    let batch_size = 20;
    let mut indexed = 0;
    let total_docs = docs_to_index.len();

    for batch_start_idx in (0..chunk_map.len()).step_by(batch_size) {
        let batch_end_idx = (batch_start_idx + batch_size).min(chunk_map.len());
        let batch_entries = &chunk_map[batch_start_idx..batch_end_idx];

        // Collect chunks for this batch
        let first_chunk = batch_entries.first().map(|e| e.1).unwrap_or(0);
        let last_entry = batch_entries.last().unwrap();
        let last_chunk_end = last_entry.1 + last_entry.2;
        let batch_chunks: Vec<&str> = all_chunks[first_chunk..last_chunk_end]
            .iter()
            .map(|s| s.as_str())
            .collect();

        match embedder.encode_batch(&batch_chunks) {
            Ok(batch_embeddings) => {
                let mut s = store.write();
                for &(doc_idx, chunk_start, chunk_count) in batch_entries {
                    let doc = &docs_to_index[doc_idx];
                    let local_start = chunk_start - first_chunk;
                    let embeddings =
                        batch_embeddings[local_start..local_start + chunk_count].to_vec();
                    let chunks = all_chunks[chunk_start..chunk_start + chunk_count].to_vec();
                    s.insert_precomputed(doc, chunks, embeddings);
                    indexed += 1;
                }
            }
            Err(e) => {
                tracing::error!("Batch embedding failed: {e}");
            }
        }

        // Incremental save after each batch
        if let Err(e) = store.read().save(_db_path) {
            tracing::error!("Incremental save failed: {e}");
        }

        tracing::info!("Progress: {indexed}/{total_docs} documents embedded");
    }

    let total = store.read().len();
    tracing::info!("Indexing complete: {indexed} indexed, {removed} removed, {total} total");

    (indexed, removed, total)
}

#[cfg(test)]
mod stdout_guard {
    //! Ensure no library source file writes to stdout, which would corrupt
    //! the MCP JSON-RPC transport. Excluded: cli.rs, reproduction.rs, lib.rs
    //! (contains this test), and the tui/ directory (CLI-only dashboard code paths).
    //! lib.rs is still guarded by `#![deny(clippy::print_stdout)]`.

    #[test]
    fn no_println_in_library_sources() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // lib.rs excluded because this test module itself references print patterns.
        // lib.rs is still guarded by #![deny(clippy::print_stdout)] at compile time.
        let allow_list: &[&str] = &["cli.rs", "reproduction.rs", "lib.rs"];

        let mut violations = Vec::new();
        check_dir(&src_dir, &src_dir, allow_list, &mut violations);

        assert!(
            violations.is_empty(),
            "stdout writes found in library code (would corrupt MCP transport):\n{}",
            violations.join("\n")
        );
    }

    fn check_dir(
        dir: &std::path::Path,
        src_root: &std::path::Path,
        allow_list: &[&str],
        violations: &mut Vec<String>,
    ) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!(
                "stdout_guard: failed to read directory {}: {e}",
                dir.display()
            )
        });
        for entry in entries.map(|e| e.unwrap()) {
            let path = entry.path();
            if path.is_dir() {
                // tui/ is CLI-only, skip it
                if path.file_name().map_or(false, |n| n == "tui") {
                    continue;
                }
                check_dir(&path, src_root, allow_list, violations);
            } else if path.extension().map_or(false, |e| e == "rs") {
                let rel = path.strip_prefix(src_root).unwrap_or(&path);
                let filename = rel.to_string_lossy();
                if allow_list.iter().any(|a| filename.ends_with(a)) {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        violations.push(format!(
                            "  {}:0: <error reading file: {e}>",
                            filename
                        ));
                        continue;
                    }
                };
                for (line_no, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    // Skip comments
                    if trimmed.starts_with("//") || trimmed.starts_with("///") {
                        continue;
                    }
                    // Match println!/print! but NOT eprintln!/eprint!
                    let has_println = trimmed.contains("println!(")
                        && !trimmed.contains("eprintln!(");
                    let has_print = trimmed.contains("print!(")
                        && !trimmed.contains("eprint!(")
                        && !trimmed.contains("println!(")
                        && !trimmed.contains("eprintln!(");
                    if has_println || has_print {
                        // Skip lines inside string literals: escaped quotes
                        // indicate the code is embedded in a string constant
                        if trimmed.contains("\\\"") || trimmed.contains("\\n") {
                            continue;
                        }
                        violations.push(format!(
                            "  {}:{}: {}",
                            filename,
                            line_no + 1,
                            trimmed
                        ));
                    }
                }
            }
        }
    }
}
