//! Shared library for the pkb binary (CLI + MCP server).
//!
//! stdout is reserved for MCP JSON-RPC when running as a server.
//! All diagnostics must go to stderr via `tracing` or `eprintln!`.
//! Library code must never write to stdout directly.
#![deny(clippy::print_stdout)]
pub mod otel;

pub mod batch_ops;
pub mod bm25;
pub mod cmd;
pub mod date_filter;
pub mod display_rank;
pub mod distance;
pub mod document_crud;
pub mod embeddings;
pub mod eval;
pub mod excalidraw;
pub mod facts;
pub mod flow;
pub mod flow_migration;
pub mod graph;
pub mod graph_display;
pub mod graph_store;
pub mod lint;
pub mod lsp;
pub mod mcp_server;
pub mod metrics;
pub mod migrations;
pub mod path_lint;
pub mod pkb;
pub mod polecat_config;
pub mod rerank;
pub mod rrf;
pub mod task_index;
pub mod telemetry;
pub mod udiff;
pub mod vectordb;

#[cfg(test)]
mod reproduction;

use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// THE single source of truth for index freshness.
///
/// Everything under the PKB root is indexed — there are no document-type or
/// `sync:` frontmatter exclusions. A document is stale (needs re-indexing) iff
/// it is new to the store or its content hash has changed. The staleness *count*
/// (`check_index_staleness`) and both reindex *work-list* builders (`index_pkb`
/// here and in `cli.rs`) all route their decision through this one predicate, so
/// the reported stale count and the set of docs reindex actually touches can
/// never disagree.
pub fn document_needs_reindex(store: &vectordb::VectorStore, doc: &pkb::PkbDocument) -> bool {
    store.needs_update(&doc.id(), &doc.file_hash)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StalenessReport {
    pub stale_count: usize,
    pub files_scanned: usize,
    pub files_read: usize,
}

/// Check whether the vector store index is stale, reporting detailed scan counts.
///
/// Unchanged documents are skipped by their filesystem stamp (mtime + size),
/// avoiding file reads, frontmatter parsing, and content hashing.
/// The filesystem scan and any necessary file reads run without holding the
/// store lock, preventing lock contention with concurrent writers or search queries.
pub fn check_index_staleness_with_stats(
    pkb_root: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
) -> StalenessReport {
    // 1. Snapshot store state under a brief read lock and release the lock immediately.
    // The filesystem scan and file parsing never hold the store lock.
    let (known_docs, cached_stamps) = {
        let s = store.read();
        let mut docs_by_path: std::collections::HashMap<
            std::path::PathBuf,
            (String, Option<String>),
        > = std::collections::HashMap::with_capacity(s.len());
        for entry in s.documents().map(|(_, e)| e) {
            docs_by_path.insert(
                entry.path.clone(),
                (entry.id.clone(), entry.file_hash.clone()),
            );
        }
        let stamps = s.stamps_cloned();
        (docs_by_path, stamps)
    };

    let files = pkb::scan_directory(pkb_root);
    let mut stale_count = 0;
    let mut files_read = 0;
    let mut new_stamps = std::collections::HashMap::new();

    for file_path in &files {
        let Ok(rel_path) = file_path.strip_prefix(pkb_root) else {
            continue;
        };
        let rel_path_buf = rel_path.to_path_buf();
        let Ok(meta) = file_path.metadata() else {
            continue;
        };
        let Some(current_stamp) = vectordb::FileStamp::from_metadata(&meta) else {
            continue;
        };

        // If the document is already in the store and its stamp matches disk, skip reading it.
        if let Some((_id, _stored_hash)) = known_docs.get(&rel_path_buf) {
            if cached_stamps.get(&rel_path_buf) == Some(&current_stamp) {
                continue;
            }
        }

        // Stamp differs or is missing, or document is new: read and parse
        files_read += 1;
        let Some(doc) = pkb::parse_file_relative(file_path, pkb_root) else {
            continue;
        };

        // Staleness predicate is checked without holding the lock across the scan
        let needs_reindex = {
            let s = store.read();
            document_needs_reindex(&s, &doc)
        };

        if needs_reindex {
            stale_count += 1;
        } else {
            // Document is fresh (content hash matched store): record current stamp
            new_stamps.insert(rel_path_buf, current_stamp);
        }
    }

    if !new_stamps.is_empty() {
        let s = store.read();
        s.set_stamps(new_stamps);
        s.save_stamps();
    }

    StalenessReport {
        stale_count,
        files_scanned: files.len(),
        files_read,
    }
}

/// Check whether the vector store index is stale.
///
/// Returns the number of documents that need re-indexing (new or modified).
/// Returns 0 if the index is fully up to date.
pub fn check_index_staleness(
    pkb_root: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
) -> usize {
    check_index_staleness_with_stats(pkb_root, store).stale_count
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

    let mut valid_ids: HashSet<String> = HashSet::new();
    let mut docs_to_index: Vec<pkb::PkbDocument> = Vec::new();
    let mut metadata_only_updates: Vec<pkb::PkbDocument> = Vec::new();
    let mut all_chunks: Vec<String> = Vec::new();
    let mut chunk_map: Vec<(usize, usize, usize)> = Vec::new();

    for file_path in &files {
        let Some(doc) = pkb::parse_file_relative(file_path, pkb_root) else {
            tracing::debug!("Skipped (parse failed): {}", file_path.display());
            continue;
        };

        let id = doc.id();
        valid_ids.insert(id.clone());

        let needs_update = force_all || {
            let store = store.read();
            document_needs_reindex(&store, &doc)
        };

        if !needs_update {
            continue;
        }

        // Check if only frontmatter changed by comparing body hash (doc.content_hash)
        let body_unchanged = {
            let store = store.read();
            if let Some(existing) = store.get_entry(&id) {
                // Check content_hash (body-only hash, new) or body_hash (deprecated).
                // Use explicit OR so a non-matching content_hash does not suppress
                // the body_hash fallback (old stores used content_hash for full file).
                existing
                    .content_hash
                    .as_deref()
                    .is_some_and(|h| h == doc.content_hash)
                    || existing
                        .body_hash
                        .as_deref()
                        .is_some_and(|h| h == doc.content_hash)
            } else {
                false
            }
        };

        if body_unchanged {
            metadata_only_updates.push(doc);
            continue;
        }

        let embedding_text = doc.embedding_text();
        let chunks = embeddings::chunk_text(&embedding_text, &embeddings::ChunkConfig::default());
        let chunk_start = all_chunks.len();
        let chunk_count = chunks.len();
        all_chunks.extend(chunks);
        chunk_map.push((docs_to_index.len(), chunk_start, chunk_count));
        docs_to_index.push(doc);
    }

    let removed = {
        let mut store = store.write();
        store.remove_deleted_by_ids(&valid_ids)
    };

    let mut indexed = 0;

    // Process metadata-only updates first (cheap)
    if !metadata_only_updates.is_empty() {
        let mut store = store.write();
        for doc in metadata_only_updates {
            let id = doc.id();
            // Extract data in a separate scope so the immutable borrow of `store`
            // is released before the mutable borrow in insert_precomputed.
            let existing_data = store
                .get_entry(&id)
                .map(|e| (e.chunk_embeddings.clone(), e.chunk_texts.clone()));
            if let Some((embeddings, chunks)) = existing_data {
                store.insert_precomputed(&doc, chunks, embeddings);
                indexed += 1;
            }
        }
        tracing::info!("Applied {indexed} metadata-only updates (skipped re-embedding)");
    }

    if docs_to_index.is_empty() {
        let total = store.read().len();
        tracing::info!("Indexing complete: {indexed} updated, {removed} removed, {total} total");
        return (indexed, removed, total);
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

    // Record filesystem stamps for all scanned files that are now up to date
    let mut initial_stamps = std::collections::HashMap::with_capacity(files.len());
    for file_path in &files {
        if let Ok(rel_path) = file_path.strip_prefix(pkb_root) {
            if let Ok(meta) = file_path.metadata() {
                if let Some(stamp) = vectordb::FileStamp::from_metadata(&meta) {
                    initial_stamps.insert(rel_path.to_path_buf(), stamp);
                }
            }
        }
    }
    store.read().set_stamps(initial_stamps);
    store.read().save_stamps();

    let total = store.read().len();
    tracing::info!("Indexing complete: {indexed} indexed, {removed} removed, {total} total");

    (indexed, removed, total)
}

#[cfg(test)]
mod stdout_guard {
    //! Ensure no library source file writes to stdout, which would corrupt
    //! the MCP JSON-RPC transport. Excluded: cli.rs, reproduction.rs, lib.rs
    //! (contains this test).
    //! lib.rs is still guarded by `#![deny(clippy::print_stdout)]`.

    #[test]
    fn no_println_in_library_sources() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // lib.rs excluded because this test module itself references print patterns.
        // lib.rs is still guarded by #![deny(clippy::print_stdout)] at compile time.

        let allow_list: &[&str] = &["cli.rs", "reproduction.rs", "lib.rs", "pkb_excalidraw.rs"];

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
                check_dir(&path, src_root, allow_list, violations);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(src_root).unwrap_or(&path);
                let filename = rel.to_string_lossy();
                if allow_list.iter().any(|a| filename.ends_with(a)) {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        violations.push(format!("  {}:0: <error reading file: {e}>", filename));
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
                    let has_println =
                        trimmed.contains("println!(") && !trimmed.contains("eprintln!(");
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
                        violations.push(format!("  {}:{}: {}", filename, line_no + 1, trimmed));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::{Embedder, EMBEDDING_DIM};
    use crate::vectordb::VectorStore;
    use parking_lot::RwLock;
    use std::sync::Arc;

    /// Regression test: a frontmatter-only change must NOT trigger encode_batch.
    ///
    /// Strategy: pre-seed the store with a sentinel embedding vector that is
    /// distinguishable from the dummy embedder's zero output. After calling
    /// index_pkb with a file whose body is unchanged but frontmatter differs,
    /// the sentinel must still be present — proving encode_batch was never called.
    #[test]
    fn frontmatter_only_update_skips_encode_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let db_path = pkb_root.join("test.db");
        let file_path = pkb_root.join("task.md");

        // Write initial file with frontmatter + body
        let initial_content = "---\nstatus: inbox\ntitle: Test Task\n---\n\nThis is the body text.";
        std::fs::write(&file_path, initial_content).unwrap();

        // Parse the file with relative path so store keys match what index_pkb uses
        let parsed =
            crate::pkb::parse_file_relative(&file_path, pkb_root).expect("parse initial file");
        let body_hash = parsed.content_hash.clone();

        // Build sentinel embeddings (non-zero, so distinguishable from dummy output)
        let sentinel_dim = EMBEDDING_DIM;
        let mut sentinel = vec![0.0f32; sentinel_dim];
        sentinel[0] = 99.0;

        // Pre-seed the store with these sentinel embeddings
        let store = Arc::new(RwLock::new(VectorStore::new(sentinel_dim)));
        {
            let mut w = store.write();
            w.insert_precomputed(
                &parsed,
                vec!["This is the body text.".to_string()],
                vec![sentinel.clone()],
            );
        }

        // Now mutate only the frontmatter (change status: inbox → active)
        let updated_content =
            "---\nstatus: active\ntitle: Test Task\n---\n\nThis is the body text.";
        std::fs::write(&file_path, updated_content).unwrap();

        // Run index_pkb with a dummy embedder (returns zero vectors if called)
        let embedder = Embedder::new_dummy();
        let (indexed, removed, _total) = index_pkb(pkb_root, &db_path, &store, &embedder, false);

        // Exactly 1 metadata-only update should have been processed
        assert_eq!(indexed, 1, "expected 1 metadata-only update");
        assert_eq!(removed, 0, "no documents should have been removed");

        // Sentinel embedding must still be present — if encode_batch had been called
        // the dummy embedder would have replaced it with zero vectors
        let entry = store
            .read()
            .get_entry(&parsed.id())
            .expect("entry must exist")
            .clone();
        let stored_embedding = &entry.chunk_embeddings[0];
        assert_eq!(
            stored_embedding[0], 99.0,
            "embedding[0] should be sentinel 99.0 — encode_batch must not have been called"
        );

        // Verify body hash is preserved (not overwritten with full-file hash)
        assert_eq!(
            entry.content_hash.as_deref(),
            Some(body_hash.as_str()),
            "content_hash must still be the body-only hash"
        );
    }

    /// Everything in the repo is indexed — including daily notes, which used to
    /// be excluded by `is_sync_enabled`. This also pins the invariant that the
    /// `status` count (`check_index_staleness`) and the indexer use the SAME
    /// staleness predicate, so the count reaches 0 after a reindex instead of
    /// being stuck forever on perpetually-"stale" daily notes.
    #[test]
    fn daily_notes_are_indexed_and_clear_staleness() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let db_path = pkb_root.join("test.db");
        let file_path = pkb_root.join("2026-06-25.md");

        // A daily note: previously skipped by the index, counted as perpetually stale.
        std::fs::write(
            &file_path,
            "---\ntype: daily\ntitle: 2026-06-25\n---\n\nWent for a walk.",
        )
        .unwrap();

        let store = Arc::new(RwLock::new(VectorStore::new(EMBEDDING_DIM)));

        // Empty store: the daily note must be reported stale (it WILL be indexed).
        assert_eq!(
            check_index_staleness(pkb_root, &store),
            1,
            "daily note must be counted as needing indexing"
        );
        let doc = crate::pkb::parse_file_relative(&file_path, pkb_root).expect("parse daily note");
        assert!(
            document_needs_reindex(&store.read(), &doc),
            "the shared staleness predicate must select the daily note"
        );

        // Index it, then status must agree the index is fresh (count -> 0).
        let embedder = Embedder::new_dummy();
        let (indexed, _removed, _total) = index_pkb(pkb_root, &db_path, &store, &embedder, false);
        assert_eq!(indexed, 1, "daily note must be indexed");
        assert_eq!(
            check_index_staleness(pkb_root, &store),
            0,
            "after reindex the daily note must no longer be stale (status == indexer)"
        );
    }

    #[test]
    fn test_check_index_staleness_skips_unchanged_and_detects_changed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let db_path = pkb_root.join("test.db");
        let f1 = pkb_root.join("unchanged.md");
        let f2 = pkb_root.join("to_change.md");

        std::fs::write(
            &f1,
            "---\nid: unchanged\ntitle: Unchanged\n---\n\nInitial unchanged content.",
        )
        .unwrap();
        std::fs::write(
            &f2,
            "---\nid: to_change\ntitle: To Change\n---\n\nInitial content to change.",
        )
        .unwrap();

        let store = Arc::new(RwLock::new(VectorStore::new(EMBEDDING_DIM)));
        let embedder = Embedder::new_dummy();

        // Index both files into the store
        let (indexed, _, _) = index_pkb(pkb_root, &db_path, &store, &embedder, false);
        assert_eq!(indexed, 2, "both files indexed");

        // Verify initial check sees index as fresh (stale_count == 0)
        let initial_report = check_index_staleness_with_stats(pkb_root, &store);
        assert_eq!(initial_report.stale_count, 0, "store is fresh after index");

        // Subsequent check on unchanged files: MUST NOT re-read documents from disk!
        let report_clean = check_index_staleness_with_stats(pkb_root, &store);
        assert_eq!(report_clean.stale_count, 0, "no stale files");
        assert_eq!(
            report_clean.files_read, 0,
            "unchanged documents must not be re-read; skipped by stamp"
        );

        // Mutate only to_change.md
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            &f2,
            "---\nid: to_change\ntitle: To Change\n---\n\nMutated content.",
        )
        .unwrap();

        // Check index staleness:
        // Changed file MUST be detected as stale.
        // Unchanged file MUST NOT be re-read.
        let report_mutated = check_index_staleness_with_stats(pkb_root, &store);
        assert_eq!(
            report_mutated.stale_count, 1,
            "changed document must be detected as stale"
        );
        assert_eq!(
            report_mutated.files_read, 1,
            "only the changed document must be re-read; unchanged must be skipped"
        );
    }

    #[test]
    fn test_check_index_staleness_never_holds_lock_during_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let db_path = pkb_root.join("test.db");

        for i in 0..10 {
            let f = pkb_root.join(format!("doc_{i}.md"));
            std::fs::write(
                &f,
                format!("---\nid: doc_{i}\ntitle: Doc {i}\n---\n\nBody {i}"),
            )
            .unwrap();
        }

        let store = Arc::new(RwLock::new(VectorStore::new(EMBEDDING_DIM)));
        let embedder = Embedder::new_dummy();
        let (indexed, _, _) = index_pkb(pkb_root, &db_path, &store, &embedder, false);
        assert_eq!(indexed, 10);

        let store_clone = store.clone();
        let pkb_root_buf = pkb_root.to_path_buf();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let b1 = barrier.clone();

        let handle = std::thread::spawn(move || {
            b1.wait();
            for _ in 0..5 {
                if let Some(w) = store_clone.try_write() {
                    w.clear_stamps();
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let w = store_clone.write();
            w.clear_stamps();
            true
        });

        barrier.wait();
        let count = check_index_staleness(&pkb_root_buf, &store);
        assert_eq!(count, 0);

        let acquired = handle.join().unwrap();
        assert!(acquired);
    }
}
