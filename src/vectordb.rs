//! Persistent vector store for PKB semantic search.
//!
//! Simple brute-force cosine similarity search, persisted to disk via bincode.
//! Sufficient for <10k documents typical of a personal knowledge base.

use crate::distance;
use crate::embeddings;
use crate::pkb::PkbDocument;
use anyhow::Result;
use fd_lock::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod path_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::path::PathBuf;

    pub fn serialize<S>(path: &PathBuf, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let path_str = path.to_string_lossy().replace('\\', "/");
        path_str.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PathBuf::from(s))
    }
}

/// A stored document entry with its embeddings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentEntry {
    /// File path (relative to pkb_root for portability)
    #[serde(with = "path_serde")]
    pub path: PathBuf,
    /// Document title
    pub title: String,
    /// Document type
    pub doc_type: Option<String>,
    /// Document status
    pub status: Option<String>,
    /// Tags
    pub tags: Vec<String>,
    /// Document ID (from frontmatter or fallback)
    #[serde(default)]
    pub id: String,
    /// Document date (from frontmatter)
    #[serde(default)]
    pub date: Option<String>,
    /// Whether this document has been consolidated by the LLM sleep cycle
    #[serde(default)]
    pub consolidated: Option<bool>,
    /// When this document was last consolidated (RFC3339)
    #[serde(default)]
    pub consolidated_at: Option<String>,
    /// Last modified timestamp (RFC3339 or YYYY-MM-DD, from frontmatter `modified:`).
    /// Note: `#[serde(default)]` applies to text formats only; bincode is positional so
    /// adding or reordering fields breaks existing bincode files. `load_or_create` handles
    /// legacy stores via [`OldVectorStore`] fallback, salvaging existing embeddings without re-indexing.
    #[serde(default)]
    pub modified: Option<String>,
    /// Confidence level (0.0 - 1.0)
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Hash of the markdown body only (no YAML frontmatter).
    /// Used for skipping re-embedding when only frontmatter changed.
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Hash of the FULL file (blake3, hex-encoded) — used for metadata staleness detection
    #[serde(default)]
    pub file_hash: Option<String>,
    /// Hash of the markdown body only (no YAML frontmatter).
    /// Deprecated: use content_hash instead.
    #[serde(default)]
    pub body_hash: Option<String>,
    /// Embedding vectors for each chunk of the document
    pub chunk_embeddings: Vec<Vec<f32>>,
    /// The text chunks that were embedded (for returning snippets)
    pub chunk_texts: Vec<String>,
    /// Body-only text chunks for display snippets (excludes frontmatter metadata)
    #[serde(default)]
    pub body_chunks: Vec<String>,
}

/// An index entry whose backing document is absent from disk — a stale
/// embedding surviving deletion (or move) of its source file. Distinct from
/// a graph-referenced "ghost node" (an id named in another document's
/// `parent`/`depends_on` field with no file): this class has no graph
/// referrer at all, so it is invisible to `pkb_orphans` and only shows up as
/// a search hit that resolves to nothing on every by-id lookup.
///
/// See `find_orphaned_entries` and the `repair_index_orphans` MCP tool.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedIndexEntry {
    pub id: String,
    pub title: String,
    pub doc_type: Option<String>,
    /// Path as stored in the index (relative to pkb_root unless the entry
    /// predates path normalization).
    pub path: String,
}

/// Search result returned from queries
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub title: String,
    pub score: f32,
    /// Short extract (300 chars) for quick scanning
    pub snippet: String,
    /// Full text of the best-matching chunk
    pub chunk_text: String,
    pub id: String,
    pub date: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub confidence: Option<f64>,
}

/// Metadata-only patch produced by [`VectorStore::prepare_upsert`] when the
/// body hash matches the existing entry. Applied via [`VectorStore::apply_prepared`]
/// without touching `chunk_embeddings`, `chunk_texts`, or `body_chunks`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetadataPatch {
    pub id: String,
    pub title: String,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub date: Option<String>,
    pub consolidated: Option<bool>,
    pub consolidated_at: Option<String>,
    pub modified: Option<String>,
    pub confidence: Option<f64>,
    pub file_hash: String,
}

/// An incremental write operation recorded in the Write-Ahead Log (WAL).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalRecord {
    /// Full document entry upsert
    Upsert(Box<DocumentEntry>),
    /// Metadata-only patch
    MetadataOnly(MetadataPatch),
    /// Document deletion by ID
    Remove(String),
}

/// Returns the WAL file path corresponding to a database path (e.g. `pkb_vectors.wal`).
pub fn wal_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("wal")
}

/// Returns the compacting WAL file path corresponding to a database path (e.g. `pkb_vectors.wal.compacting`).
pub fn wal_compacting_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("wal.compacting")
}

/// Outcome of [`VectorStore::prepare_upsert`]. The caller applies this under
/// a brief write lock via [`VectorStore::apply_prepared`].
///
/// The point of splitting prepare/apply is to keep the slow work
/// (chunking + embedding) outside the `VectorStore` write lock, so that
/// concurrent writers don't serialise behind a multi-second embed.
#[derive(Debug, Clone)]
pub enum PreparedUpsert {
    /// Body is unchanged vs. the existing entry — apply only the cheap
    /// metadata mutation (`title`, `status`, `tags`, …) and keep the
    /// existing embeddings, chunk texts, and body chunks untouched.
    MetadataOnly(MetadataPatch),
    /// Body changed (or no prior entry) — replace the whole entry.
    Full(Box<DocumentEntry>),
}

/// Legacy vector store snapshot used for 1-step backwards compatibility migration.
///
/// See `specs/pkb-server-spec.md` § Vector Store (Persisted Format & Schema Evolution Policy)
/// for the authoritative format evolution policy.
///
/// **Schema Migration Protocol**:
/// Because `bincode` is a positional binary format, adding, removing, or reordering fields in
/// [`DocumentEntry`] breaks binary deserialization of existing vector stores on disk.
///
/// The default disposition on a format break is discard-and-reindex. Where a legacy migration
/// pathway is maintained (like [`OldVectorStore`]):
/// 1. `OldDocumentEntry` snapshots the previous `DocumentEntry` schema.
/// 2. `VectorStore::load_or_create` falls back to `OldVectorStore` on primary decode failure,
///    verifies matching embedding dimension, maps salvaged fields into the new schema, and
///    persists the upgraded store to disk.
#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct OldVectorStore {
    documents: HashMap<String, OldDocumentEntry>,
    dimension: usize,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct OldDocumentEntry {
    #[serde(with = "path_serde")]
    path: PathBuf,
    title: String,
    doc_type: Option<String>,
    status: Option<String>,
    tags: Vec<String>,
    #[serde(default)]
    id: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    content_hash: Option<String>,
    #[serde(default)]
    file_hash: Option<String>,
    #[serde(default)]
    body_hash: Option<String>,
    chunk_embeddings: Vec<Vec<f32>>,
    chunk_texts: Vec<String>,
    #[serde(default)]
    body_chunks: Vec<String>,
}

/// Persistent vector store
#[derive(Serialize, Deserialize)]
pub struct VectorStore {
    /// Map from file path (relative to pkb_root) to document entry
    documents: HashMap<String, DocumentEntry>,
    /// Embedding dimension
    dimension: usize,
    /// In-memory BM25 lexical index
    #[serde(skip)]
    bm25: Arc<parking_lot::RwLock<crate::bm25::Bm25Index>>,
}

/// Helper to check if a document's doc_type matches an optional type filter string.
///
/// Supports:
/// - Single type: e.g. `"task"`, `"template"`, `"note"`
/// - Comma-separated positive types: e.g. `"task,epic"`
/// - Negated types (prefixed with `!`): e.g. `"!task"`, `"!task,!epic"`
/// - Case-insensitive matching
pub fn matches_type_filter(doc_type: Option<&str>, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }

    let mut pos_types = Vec::new();
    let mut neg_types = Vec::new();

    for part in filter.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(negated) = trimmed.strip_prefix('!') {
            let neg_clean = negated.trim();
            if !neg_clean.is_empty() {
                neg_types.push(neg_clean);
            }
        } else {
            pos_types.push(trimmed);
        }
    }

    if pos_types.is_empty() && neg_types.is_empty() {
        return true;
    }

    // Check negations first: if doc_type matches any negated type, exclude it
    if !neg_types.is_empty() {
        if let Some(dt) = doc_type {
            if neg_types.iter().any(|n| dt.eq_ignore_ascii_case(n)) {
                return false;
            }
        }
    }

    // If positive types are specified, doc_type must match at least one positive type
    if !pos_types.is_empty() {
        match doc_type {
            Some(dt) => pos_types.iter().any(|p| dt.eq_ignore_ascii_case(p)),
            None => false,
        }
    } else {
        true
    }
}

impl VectorStore {
    pub fn new(dimension: usize) -> Self {
        Self {
            documents: HashMap::new(),
            dimension,
            bm25: Arc::new(parking_lot::RwLock::new(crate::bm25::Bm25Index::new())),
        }
    }

    /// Acquire an exclusive lock on the index to prevent concurrent updates.
    /// The lock is held until the returned lock itself is dropped.
    pub fn acquire_lock(path: &Path) -> Result<RwLock<File>> {
        let lock_path = path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)?;

        Ok(RwLock::new(file))
    }

    /// Append a single operation record to the Write-Ahead Log.
    /// Framed as 4-byte little-endian length prefix + bincode payload.
    /// Packs the length prefix and payload into a single contiguous buffer
    /// and issues a single write call so POSIX O_APPEND guarantees atomic writes
    /// without byte-interleaving on concurrent writes.
    pub fn append_wal_record(wal_path: &Path, record: &WalRecord) -> Result<()> {
        if let Some(parent) = wal_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encoded = bincode::serialize(record)?;
        let len = encoded.len() as u32;
        let mut buf = Vec::with_capacity(4 + encoded.len());
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&encoded);

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(wal_path)?;
        use std::io::Write;
        file.write_all(&buf)?;
        file.flush()?;
        Ok(())
    }

    /// Replay records from the WAL file into this in-memory store.
    /// Stops cleanly at the last valid record if the file is truncated / corrupted at the end.
    /// Defensively bounds allocation to prevent OOM panic on corrupted length headers (> 16MB).
    pub fn replay_wal(&mut self, wal_path: &Path) -> Result<usize> {
        if !wal_path.exists() {
            return Ok(0);
        }
        let mut file = match std::fs::File::open(wal_path) {
            Ok(f) => f,
            Err(e) => return Err(e.into()),
        };
        use std::io::Read;
        let mut count = 0;
        const MAX_RECORD_SIZE: usize = 16 * 1024 * 1024; // 16 MB max record size guard
        loop {
            let mut len_buf = [0u8; 4];
            match file.read_exact(&mut len_buf) {
                Ok(()) => {
                    let len = u32::from_le_bytes(len_buf) as usize;
                    if len > MAX_RECORD_SIZE {
                        tracing::warn!(
                            "WAL record length {len} exceeds max size {MAX_RECORD_SIZE} at record {count}. Stopping replay."
                        );
                        break;
                    }
                    let mut record_buf = vec![0u8; len];
                    match file.read_exact(&mut record_buf) {
                        Ok(()) => {
                            match bincode::deserialize::<WalRecord>(&record_buf) {
                                Ok(record) => {
                                    match record {
                                        WalRecord::Upsert(entry) => {
                                            self.documents.insert(entry.id.clone(), *entry);
                                        }
                                        WalRecord::MetadataOnly(patch) => {
                                            self.apply_prepared(PreparedUpsert::MetadataOnly(patch));
                                        }
                                        WalRecord::Remove(id) => {
                                            self.remove(&id);
                                        }
                                    }
                                    count += 1;
                                }
                                Err(e) => {
                                    tracing::warn!("WAL record deserialization error at record {count}: {e}. Stopping replay.");
                                    break;
                                }
                            }
                        }
                        Err(_) => {
                            tracing::warn!("Truncated WAL record at {count}. Stopping replay.");
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
        if count > 0 {
            tracing::info!("Replayed {count} WAL records from {wal_path:?}");
        }
        Ok(count)
    }

    /// Load from disk, or create new if file doesn't exist.
    /// Detects dimension mismatch (e.g. model upgrade) and creates a fresh store.
    /// Replays any uncompacted records from `pkb_vectors.wal.compacting` (if present from a crashed save)
    /// and `pkb_vectors.wal` on top of the snapshot.
    pub fn load_or_create(path: &Path, dimension: usize) -> Result<Self> {
        let mut store = if path.exists() {
            tracing::info!("Loading vector store from {path:?}");
            let t_read = std::time::Instant::now();
            let data = std::fs::read(path)?;
            let elapsed_read = t_read.elapsed();

            let t_deserialize = std::time::Instant::now();
            let result = bincode::deserialize::<VectorStore>(&data);
            let elapsed_deserialize = t_deserialize.elapsed();

            match result {
                Ok(store) => {
                    if store.dimension != dimension {
                        tracing::warn!(
                            "Vector store dimension mismatch: stored={}, expected={}. \
                             Creating fresh store (full reindex required).",
                            store.dimension,
                            dimension
                        );
                        Self::new(dimension)
                    } else {
                        tracing::info!(
                            "Loaded {} documents from store in {:.1}ms (read: {:.1}ms, deserialize: {:.1}ms)",
                            store.documents.len(),
                            (elapsed_read + elapsed_deserialize).as_secs_f64() * 1000.0,
                            elapsed_read.as_secs_f64() * 1000.0,
                            elapsed_deserialize.as_secs_f64() * 1000.0
                        );
                        store
                    }
                }
                Err(e) => {
                    tracing::info!("Failed to deserialize vector store as new schema ({e}). Attempting legacy migration...");
                    if let Ok(old_store) = bincode::deserialize::<OldVectorStore>(&data) {
                        if old_store.dimension != dimension {
                            tracing::warn!(
                                "Vector store dimension mismatch in legacy store: stored={}, expected={}. \
                                 Creating fresh store (full reindex required).",
                                old_store.dimension,
                                dimension
                            );
                            Self::new(dimension)
                        } else {
                            tracing::info!(
                                "Successfully migrated vector DB from legacy format. Salvaged {} documents.",
                                old_store.documents.len()
                            );
                            let mut new_store = Self::new(dimension);
                            for (key, old_entry) in old_store.documents {
                                new_store.documents.insert(
                                    key,
                                    DocumentEntry {
                                        path: old_entry.path,
                                        title: old_entry.title,
                                        doc_type: old_entry.doc_type,
                                        status: old_entry.status,
                                        tags: old_entry.tags,
                                        id: old_entry.id,
                                        date: old_entry.date,
                                        consolidated: None, // New fields initialized
                                        consolidated_at: None, // New fields initialized
                                        modified: old_entry.modified,
                                        confidence: old_entry.confidence,
                                        content_hash: old_entry.content_hash,
                                        file_hash: old_entry.file_hash,
                                        body_hash: old_entry.body_hash,
                                        chunk_embeddings: old_entry.chunk_embeddings,
                                        chunk_texts: old_entry.chunk_texts,
                                        body_chunks: old_entry.body_chunks,
                                    },
                                );
                            }

                            // Persist the migrated store immediately so subsequent runs do not re-migrate
                            if let Err(e) = new_store.save(path) {
                                tracing::warn!("Failed to persist migrated vector store to {path:?}: {e}");
                            }

                            new_store
                        }
                    } else {
                        tracing::warn!("Failed to deserialize vector store entirely: {e}. Creating new.");
                        Self::new(dimension)
                    }
                }
            }
        } else {
            tracing::info!("No existing vector store found. Creating new.");
            Self::new(dimension)
        };

        // Replay any WAL records on top of base store:
        // 1. Replay .wal.compacting first if present from a crashed prior save.
        let wal_compacting = wal_compacting_path(path);
        if wal_compacting.exists() {
            let _ = store.replay_wal(&wal_compacting);
        }
        // 2. Replay current .wal.
        let wal = wal_path(path);
        if wal.exists() {
            let _ = store.replay_wal(&wal);
        }

        store.sync_bm25_all();

        Ok(store)
    }

    /// Save full snapshot to disk (compaction).
    /// Rotates `pkb_vectors.wal` -> `pkb_vectors.wal.compacting` before writing snapshot,
    /// atomically writes snapshot to `path`, and deletes only the `.wal.compacting` file.
    /// Any incoming writes arriving during snapshot save write to a fresh `pkb_vectors.wal`
    /// and are preserved without data loss.
    pub fn save(&self, path: &Path) -> Result<()> {
        let t_start = std::time::Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // 1. Rotate current WAL to .wal.compacting before taking snapshot
        let wal = wal_path(path);
        let wal_compacting = wal_compacting_path(path);
        if wal.exists() {
            let _ = std::fs::rename(&wal, &wal_compacting);
        }

        let t_serialize = std::time::Instant::now();
        let data = bincode::serialize(self)?;
        let elapsed_serialize = t_serialize.elapsed();

        let t_write = std::time::Instant::now();
        // Atomic write: write to temp file then rename
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &data)?;
        std::fs::rename(&tmp_path, path)?;

        // Clean up ONLY the rotated compacting WAL file.
        // Any fresh pkb_vectors.wal written concurrently remains untouched.
        if wal_compacting.exists() {
            let _ = std::fs::remove_file(&wal_compacting);
        }

        let elapsed_write = t_write.elapsed();

        tracing::info!(
            "Saved vector store ({} documents, {:.1} MB) in {:.1}ms (serialize: {:.1}ms, io: {:.1}ms)",
            self.documents.len(),
            data.len() as f64 / 1_048_576.0,
            t_start.elapsed().as_secs_f64() * 1000.0,
            elapsed_serialize.as_secs_f64() * 1000.0,
            elapsed_write.as_secs_f64() * 1000.0
        );
        Ok(())
    }

    /// Number of indexed documents
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Embedding dimension this store was constructed with. Used by
    /// cross-process recovery to call `load_or_create` with a matching
    /// dimension (a mismatch triggers a fresh empty store, which would
    /// silently lose data in the recovery path).
    pub fn dimension_or_default(&self) -> usize {
        self.dimension
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Check if a document needs re-indexing (metadata OR body change)
    pub fn needs_update(&self, id: &str, file_hash: &str) -> bool {
        match self.documents.get(id) {
            Some(entry) => {
                // Prefer file_hash if present
                if let Some(stored_file) = &entry.file_hash {
                    if !stored_file.is_empty() {
                        return stored_file != file_hash;
                    }
                }
                // Fallback to content_hash (legacy compatibility: old stores
                // used content_hash for the full file)
                match &entry.content_hash {
                    Some(stored) if !stored.is_empty() => stored != file_hash,
                    _ => true,
                }
            }
            None => true,
        }
    }

    /// Extract id, confidence, and date from frontmatter
    fn extract_frontmatter_fields(
        doc: &PkbDocument,
    ) -> (Option<String>, Option<f64>, Option<String>) {
        let fm = doc.frontmatter.as_ref();
        let id = fm.and_then(|f| f.get("id").and_then(|v| v.as_str()).map(String::from));
        let confidence = fm.and_then(|f| f.get("confidence").and_then(|v| v.as_f64()));
        let date = fm.and_then(|f| f.get("date").and_then(|v| v.as_str()).map(String::from));
        (id, confidence, date)
    }

    /// Build a metadata-only patch from a PKB document. Always returns a
    /// patch (no embedding work). Used by the hot write path to refresh
    /// frontmatter fields immediately while deferring the body re-embed
    /// to a background worker.
    ///
    /// Apply via [`apply_prepared`] with `PreparedUpsert::MetadataOnly`.
    /// If no entry exists for this ID yet, the apply step drops the patch
    /// — callers should ensure an entry exists (via [`prepare_upsert`]) on
    /// first index of a new document.
    pub fn prepare_metadata_patch(doc: &PkbDocument) -> MetadataPatch {
        let (id, confidence, date) = Self::extract_frontmatter_fields(doc);
        let canonical_id = id.unwrap_or_else(|| doc.id());
        MetadataPatch {
            id: canonical_id,
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            date,
            consolidated: doc.consolidated,
            consolidated_at: doc.consolidated_at.clone(),
            modified: doc.modified.clone(),
            confidence,
            file_hash: doc.file_hash.clone(),
        }
    }

    /// True if the existing entry's body hash matches the doc's body hash —
    /// i.e. no re-embed is needed. False if either hash is missing or
    /// differs, or if no entry exists.
    pub fn body_matches(doc: &PkbDocument, existing: Option<&DocumentEntry>) -> bool {
        let incoming = doc.body_hash();
        match existing {
            Some(e) => {
                e.content_hash.as_deref() == Some(&incoming)
                    || e.body_hash.as_deref() == Some(&incoming)
            }
            None => false,
        }
    }

    /// Build the upsert payload **without holding any lock** on `self`.
    ///
    /// `existing` is an optional snapshot of the prior entry (clone the
    /// matching entry from a read-locked store). When the body hash matches,
    /// no embedding is run and a [`PreparedUpsert::MetadataOnly`] is returned;
    /// otherwise this calls [`Embedder::encode_batch`] (slow on CPU) and
    /// returns a [`PreparedUpsert::Full`] for replacement.
    pub fn prepare_upsert(
        doc: &PkbDocument,
        embedder: &embeddings::Embedder,
        existing: Option<&DocumentEntry>,
    ) -> Result<PreparedUpsert> {
        let incoming_body_hash = doc.body_hash();

        // Fast path: body unchanged → no chunking, no embedding, no rewrite of
        // chunk_texts/body_chunks. Just patch the metadata fields on the
        // existing entry.
        if let Some(existing) = existing {
            let body_match = existing
                .content_hash
                .as_deref()
                .map_or(false, |h| h == incoming_body_hash)
                || existing
                    .body_hash
                    .as_deref()
                    .map_or(false, |h| h == incoming_body_hash);
            if body_match {
                let (id, confidence, date) = Self::extract_frontmatter_fields(doc);
                let canonical_id = id.unwrap_or_else(|| doc.id());
                return Ok(PreparedUpsert::MetadataOnly(MetadataPatch {
                    id: canonical_id,
                    title: doc.title.clone(),
                    doc_type: doc.doc_type.clone(),
                    status: doc.status.clone(),
                    tags: doc.tags.clone(),
                    date,
                    consolidated: doc.consolidated,
                    consolidated_at: doc.consolidated_at.clone(),
                    modified: doc.modified.clone(),
                    confidence,
                    file_hash: doc.file_hash.clone(),
                }));
            }
        }

        // Slow path: re-chunk and re-embed.
        let embedding_text = doc.embedding_text();
        let chunks = embeddings::chunk_text(&embedding_text, &embeddings::ChunkConfig::default());
        let body_chunks =
            embeddings::chunk_text(doc.body.trim(), &embeddings::ChunkConfig::default());
        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let chunk_embeddings = embedder.encode_batch(&chunk_refs)?;

        let (id, confidence, date) = Self::extract_frontmatter_fields(doc);
        let canonical_id = id.unwrap_or_else(|| doc.id());

        // Defensive: an entry with empty chunk_embeddings is invisible to
        // search (the max-similarity loop never enters and best_score stays
        // at NEG_INFINITY). Warn at write time rather than only at startup.
        if chunk_embeddings.is_empty() {
            tracing::warn!(
                "Indexed document with no chunk embeddings: {} (title: {:?}). \
                 This entry will not appear in search results.",
                doc.path.display(),
                doc.title,
            );
        }

        let norm_path = PathBuf::from(doc.path.to_string_lossy().replace('\\', "/"));

        let entry = DocumentEntry {
            path: norm_path,
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            id: canonical_id,
            date,
            consolidated: doc.consolidated,
            consolidated_at: doc.consolidated_at.clone(),
            modified: doc.modified.clone(),
            confidence,
            content_hash: Some(incoming_body_hash.clone()),
            file_hash: Some(doc.file_hash.clone()),
            body_hash: Some(incoming_body_hash),
            chunk_embeddings,
            chunk_texts: chunks,
            body_chunks,
        };

        Ok(PreparedUpsert::Full(Box::new(entry)))
    }

    /// Synchronize the in-memory BM25 index with all documents currently in the store.
    pub fn sync_bm25_all(&self) {
        let mut bm25 = self.bm25.write();
        bm25.clear();
        for entry in self.documents.values() {
            let body_text = if !entry.chunk_texts.is_empty() {
                entry.chunk_texts.join(" ")
            } else {
                String::new()
            };
            let snippet = entry.body_chunks.first().cloned().unwrap_or_else(|| {
                entry.chunk_texts.first().cloned().unwrap_or_default()
            });
            let chunk_text = snippet.clone();
            bm25.upsert(
                &entry.id,
                entry.path.clone(),
                &entry.title,
                entry.doc_type.clone(),
                entry.status.clone(),
                entry.tags.clone(),
                entry.modified.clone(),
                entry.date.clone(),
                entry.confidence,
                snippet,
                chunk_text,
                &body_text,
            );
        }
    }

    /// Apply a [`PreparedUpsert`] under a brief write lock.
    ///
    /// `MetadataOnly` is a cheap HashMap mutation; `Full` is a single
    /// `insert` that replaces the entire entry. Either way the critical
    /// section is sub-millisecond — the expensive work happened earlier
    /// in [`prepare_upsert`].
    pub fn apply_prepared(&mut self, prepared: PreparedUpsert) {
        match prepared {
            PreparedUpsert::MetadataOnly(patch) => {
                if let Some(entry) = self.documents.get_mut(&patch.id) {
                    entry.title = patch.title;
                    entry.doc_type = patch.doc_type;
                    entry.status = patch.status;
                    entry.tags = patch.tags;
                    entry.date = patch.date;
                    entry.consolidated = patch.consolidated;
                    entry.consolidated_at = patch.consolidated_at;
                    entry.modified = patch.modified;
                    entry.confidence = patch.confidence;
                    entry.file_hash = Some(patch.file_hash);

                    let body_text = entry.chunk_texts.join(" ");
                    let snippet = entry.body_chunks.first().cloned().unwrap_or_else(|| {
                        entry.chunk_texts.first().cloned().unwrap_or_default()
                    });
                    self.bm25.write().upsert(
                        &entry.id,
                        entry.path.clone(),
                        &entry.title,
                        entry.doc_type.clone(),
                        entry.status.clone(),
                        entry.tags.clone(),
                        entry.modified.clone(),
                        entry.date.clone(),
                        entry.confidence,
                        snippet.clone(),
                        snippet,
                        &body_text,
                    );
                } else {
                    // Race: the entry was removed between prepare and apply.
                    // Drop the patch — there's nothing to update.
                    tracing::debug!(
                        "MetadataOnly upsert dropped: entry vanished for {}",
                        patch.id
                    );
                }
            }
            PreparedUpsert::Full(entry) => {
                let body_text = entry.chunk_texts.join(" ");
                let snippet = entry.body_chunks.first().cloned().unwrap_or_else(|| {
                    entry.chunk_texts.first().cloned().unwrap_or_default()
                });
                self.bm25.write().upsert(
                    &entry.id,
                    entry.path.clone(),
                    &entry.title,
                    entry.doc_type.clone(),
                    entry.status.clone(),
                    entry.tags.clone(),
                    entry.modified.clone(),
                    entry.date.clone(),
                    entry.confidence,
                    snippet.clone(),
                    snippet,
                    &body_text,
                );
                self.documents.insert(entry.id.clone(), *entry);
            }
        }
    }

    /// Insert or update a document — convenience wrapper around
    /// [`prepare_upsert`] + [`apply_prepared`]. **Holds the write lock for
    /// the full embed**; prefer the split form on the hot path so other
    /// writers don't serialise behind embedding.
    pub fn upsert(&mut self, doc: &PkbDocument, embedder: &embeddings::Embedder) -> Result<()> {
        let id = doc.id();
        let prepared = Self::prepare_upsert(doc, embedder, self.documents.get(&id))?;
        self.apply_prepared(prepared);
        Ok(())
    }

    /// Insert a document with pre-computed embeddings (no embedding call needed)
    pub fn insert_precomputed(
        &mut self,
        doc: &PkbDocument,
        chunks: Vec<String>,
        chunk_embeddings: Vec<Vec<f32>>,
    ) {
        let (id, confidence, date) = Self::extract_frontmatter_fields(doc);
        let canonical_id = id.unwrap_or_else(|| doc.id());
        let body_chunks =
            embeddings::chunk_text(doc.body.trim(), &embeddings::ChunkConfig::default());
        let body_hash = doc.body_hash();

        let norm_path = PathBuf::from(doc.path.to_string_lossy().replace('\\', "/"));

        let entry = DocumentEntry {
            path: norm_path.clone(),
            title: doc.title.clone(),
            doc_type: doc.doc_type.clone(),
            status: doc.status.clone(),
            tags: doc.tags.clone(),
            id: canonical_id.clone(),
            date: date.clone(),
            consolidated: doc.consolidated,
            consolidated_at: doc.consolidated_at.clone(),
            modified: doc.modified.clone(),
            confidence,
            content_hash: Some(body_hash.clone()),
            file_hash: Some(doc.file_hash.clone()),
            body_hash: Some(body_hash),
            chunk_embeddings,
            chunk_texts: chunks.clone(),
            body_chunks: body_chunks.clone(),
        };

        let body_text = chunks.join(" ");
        let snippet = body_chunks.first().cloned().unwrap_or_else(|| {
            chunks.first().cloned().unwrap_or_default()
        });
        self.bm25.write().upsert(
            &canonical_id,
            norm_path,
            &doc.title,
            doc.doc_type.clone(),
            doc.status.clone(),
            doc.tags.clone(),
            doc.modified.clone(),
            date,
            confidence,
            snippet.clone(),
            snippet,
            &body_text,
        );

        self.documents.insert(canonical_id, entry);
    }

    /// Remove a single document by its ID.
    ///
    /// Returns true if the document was found and removed.
    pub fn remove(&mut self, id: &str) -> bool {
        self.bm25.write().remove(id);
        self.documents.remove(id).is_some()
    }

    /// Remove documents whose IDs are no longer present in the PKB.
    pub fn remove_deleted_by_ids(
        &mut self,
        existing_ids: &std::collections::HashSet<String>,
    ) -> usize {
        let before = self.documents.len();
        let mut bm25 = self.bm25.write();
        self.documents.retain(|id, _| {
            let keep = existing_ids.contains(id);
            if !keep {
                bm25.remove(id);
            }
            keep
        });
        let removed = before - self.documents.len();
        if removed > 0 {
            tracing::info!("Removed {removed} deleted documents from index");
        }
        removed
    }

    /// Lexical BM25 search
    pub fn search_bm25(
        &self,
        query: &str,
        limit: usize,
        pkb_root: &Path,
        since: Option<&str>,
        before: Option<&str>,
        type_filter: Option<&str>,
    ) -> Vec<SearchResult> {
        self.bm25.read().search(query, limit, pkb_root, since, before, type_filter)
    }

    /// Hybrid search: Combines vector similarity and BM25 lexical retrieval via Reciprocal Rank Fusion (RRF).
    /// If a reranker is provided, scores and re-orders the fused candidates.
    pub fn search_hybrid(
        &self,
        query: &str,
        query_embedding: &[f32],
        limit: usize,
        pkb_root: &Path,
        since: Option<&str>,
        before: Option<&str>,
        type_filter: Option<&str>,
        reranker: Option<&crate::rerank::CrossEncoderReranker>,
    ) -> Vec<SearchResult> {
        let fetch_limit = (limit * 3).max(20);
        let vector_results = self.search(query_embedding, fetch_limit, pkb_root, since, before, type_filter);
        let bm25_results = self.search_bm25(query, fetch_limit, pkb_root, since, before, type_filter);

        let config = crate::rrf::RrfConfig {
            k: crate::rrf::DEFAULT_RRF_K,
            vector_weight: crate::rrf::DEFAULT_VECTOR_WEIGHT,
            bm25_weight: crate::rrf::DEFAULT_BM25_WEIGHT,
        };

        let mut fused = crate::rrf::fuse_rrf(vector_results, bm25_results, &config, fetch_limit);

        if let Some(r) = reranker {
            r.rerank(query, &mut fused, limit);
        }

        fused.truncate(limit);
        fused
    }

    /// Semantic search: find the top-k most similar documents to a query.
    ///
    /// Returned `SearchResult.path` values are reconstructed as absolute paths
    /// by joining with `pkb_root`.
    pub fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        pkb_root: &Path,
        since: Option<&str>,
        before: Option<&str>,
        type_filter: Option<&str>,
    ) -> Vec<SearchResult> {
        // Build candidate list: for each document, use max similarity across chunks
        let mut results: Vec<SearchResult> = Vec::new();

        for entry in self.documents.values() {
            // Filter by document type
            if !matches_type_filter(entry.doc_type.as_deref(), type_filter) {
                continue;
            }

            // Filter by modified date. RFC3339 timestamps start with YYYY-MM-DD so
            // the first 10 chars compare correctly against YYYY-MM-DD filter params.
            // Entries with no modified date are excluded when any date filter is active.
            if since.is_some() || before.is_some() {
                match entry.modified.as_deref() {
                    Some(m) => {
                        let date_prefix = &m[..m.floor_char_boundary(10)];
                        if since.map_or(false, |s| date_prefix < s)
                            || before.map_or(false, |b| date_prefix > b)
                        {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

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
                // Prefer body_chunks (no frontmatter metadata) for display;
                // fall back to chunk_texts if body_chunks unavailable
                let display_source = if !entry.body_chunks.is_empty() {
                    &entry.body_chunks
                } else {
                    &entry.chunk_texts
                };

                let full_chunk = if best_chunk_idx < display_source.len() {
                    display_source[best_chunk_idx].clone()
                } else if !display_source.is_empty() {
                    display_source[0].clone()
                } else {
                    String::new()
                };

                let snippet = {
                    let mut trunc = 300.min(full_chunk.len());
                    while trunc > 0 && !full_chunk.is_char_boundary(trunc) {
                        trunc -= 1;
                    }
                    full_chunk[..trunc].to_string()
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
                    snippet,
                    chunk_text: full_chunk,
                    id: entry.id.clone(),
                    date: entry.date.clone(),
                    doc_type: entry.doc_type.clone(),
                    status: entry.status.clone(),
                    tags: entry.tags.clone(),
                    confidence: entry.confidence,
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
            if !matches_type_filter(entry.doc_type.as_deref(), type_filter) {
                continue;
            }
            if let Some(st) = status_filter {
                match &entry.status {
                    Some(s) if s.eq_ignore_ascii_case(st) => {}
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
                chunk_text: String::new(),
                id: entry.id.clone(),
                date: entry.date.clone(),
                doc_type: entry.doc_type.clone(),
                status: entry.status.clone(),
                tags: entry.tags.clone(),
                confidence: entry.confidence,
            });
        }

        results.sort_by(|a, b| a.title.cmp(&b.title));
        results
    }

    /// Iterate over all document entries (for pairwise comparison in duplicate detection).
    pub fn documents(&self) -> impl Iterator<Item = (&String, &DocumentEntry)> {
        self.documents.iter()
    }

    /// Get a document entry by its relative path key.
    pub fn get_entry(&self, path: &str) -> Option<&DocumentEntry> {
        self.documents.get(path)
    }

    /// Scan every index entry and return those whose backing file is no
    /// longer a regular file on disk (deleted, or replaced by a directory).
    ///
    /// This is the detection half of the ghost-search-hit defect class
    /// (task_5f2c5fa6): a document can be deleted (or its file replaced)
    /// without the vector store being told, because only a full reindex
    /// (`remove_deleted_by_ids`, driven by a directory scan) reconciles the
    /// store against disk — `refresh_graph`/`ensure_graph_fresh` rebuild the
    /// *graph* from disk but never touch the vector index. The surviving
    /// entry keeps matching queries in `search`/`search_by_tag` forever,
    /// with no lookup path that can ever resolve it again.
    ///
    /// Uses `is_file()` rather than `exists()` so a path that now resolves
    /// to a directory (a name collision, not merely "gone") is also flagged
    /// — either way there is nothing for a by-id read to open.
    pub fn find_orphaned_entries(&self, pkb_root: &Path) -> Vec<OrphanedIndexEntry> {
        self.documents
            .values()
            .filter(|entry| {
                let abs = if entry.path.is_absolute() {
                    entry.path.clone()
                } else {
                    pkb_root.join(&entry.path)
                };
                !abs.is_file()
            })
            .map(|entry| OrphanedIndexEntry {
                id: entry.id.clone(),
                title: entry.title.clone(),
                doc_type: entry.doc_type.clone(),
                path: entry.path.to_string_lossy().into_owned(),
            })
            .collect()
    }

    /// Count documents whose `chunk_embeddings` is empty.
    ///
    /// Such entries can never match a query (the search loop never enters
    /// for an entry with no chunk embeddings, leaving its score at
    /// `f32::NEG_INFINITY`). When this count is non-zero the index is in
    /// a degraded state — `pkb reindex --force` should be run.
    pub fn count_docs_missing_embeddings(&self) -> usize {
        self.documents
            .values()
            .filter(|e| e.chunk_embeddings.is_empty())
            .count()
    }

    /// Snapshot of every document's averaged chunk embedding, keyed by the
    /// store's relative path.
    ///
    /// Used to feed the graph similarity-edge computation **without** holding
    /// the `VectorStore` read lock for the duration of `rebuild_graph`.
    /// Callers compute the snapshot under a brief read lock, drop the lock,
    /// and then run the (slow) graph rebuild against the cloned data.
    ///
    /// Memory: O(N × D) f32s — ~4 KB per doc at 1024 dims, ~40 MB for 10k
    /// docs. Cheap relative to the parse + rebuild that follows.
    pub fn averaged_embeddings(&self) -> HashMap<String, Vec<f32>> {
        self.documents
            .iter()
            .filter_map(|(path, entry)| {
                crate::batch_ops::similarity::average_embedding(&entry.chunk_embeddings)
                    .map(|avg| (path.clone(), avg))
            })
            .collect()
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
        id: Option<&str>,
        confidence: Option<f64>,
        embedding: Vec<f32>,
    ) -> DocumentEntry {
        DocumentEntry {
            path: PathBuf::from(path),
            title: title.to_string(),
            doc_type: doc_type.map(String::from),
            status: status.map(String::from),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            id: id
                .map(String::from)
                .unwrap_or_else(|| "fallback".to_string()),
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence,
            content_hash: Some("test_hash_123".to_string()),
            file_hash: Some("test_hash_123".to_string()),
            body_hash: Some("test_body_hash_123".to_string()),
            chunk_embeddings: vec![embedding],
            chunk_texts: vec![format!("body of {title}")],
            body_chunks: vec![format!("body of {title}")],
        }
    }

    fn build_test_store() -> VectorStore {
        let mut store = VectorStore::new(3);
        store.documents.insert(
            "task-abc".to_string(),
            make_entry(
                "tasks/task-abc.md",
                "Fix the bug",
                Some("task"),
                Some("active"),
                &["bugfix", "urgent"],
                Some("task-abc"),
                None,
                vec![1.0, 0.0, 0.0],
            ),
        );
        store.documents.insert(
            "mem-001".to_string(),
            make_entry(
                "memories/mem-001.md",
                "Important insight",
                Some("memory"),
                None,
                &["pattern", "urgent"],
                Some("mem-001"),
                None,
                vec![0.0, 1.0, 0.0],
            ),
        );
        store.documents.insert(
            "note-xyz".to_string(),
            make_entry(
                "notes/note-xyz.md",
                "Research notes",
                Some("note"),
                None,
                &["research"],
                Some("note-xyz"),
                None,
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
        let results = store.list_documents(None, None, None, root);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_list_documents_filter_by_tag() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("urgent"), None, None, root);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_list_documents_filter_by_type() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, Some("task"), None, root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fix the bug");
    }

    #[test]
    fn test_list_documents_filter_by_type_case_insensitive() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, Some("TASK"), None, root);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_list_documents_filter_by_status() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(None, None, Some("active"), root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fix the bug");
    }

    #[test]
    fn test_list_documents_combined_filters() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("urgent"), Some("memory"), None, root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Important insight");
    }

    #[test]
    fn test_list_documents_no_match() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.list_documents(Some("nonexistent"), None, None, root);
        assert!(results.is_empty());
    }

    // ── find_orphaned_entries (task_5f2c5fa6) ──

    #[test]
    fn test_find_orphaned_entries_flags_entry_with_no_backing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        // Live entry: file actually on disk.
        std::fs::write(root.join("tasks/task-abc.md"), "live").unwrap();
        // "mem-001" and "note-xyz" (from build_test_store) have no backing
        // file anywhere under `root` — they are the orphans under test.
        let store = build_test_store();

        let orphans = store.find_orphaned_entries(root);
        let orphan_ids: std::collections::HashSet<&str> =
            orphans.iter().map(|o| o.id.as_str()).collect();

        assert!(
            !orphan_ids.contains("task-abc"),
            "entry backed by a real file must not be flagged: {orphan_ids:?}"
        );
        assert!(
            orphan_ids.contains("mem-001"),
            "entry with no backing file must be flagged: {orphan_ids:?}"
        );
    }

    #[test]
    fn test_find_orphaned_entries_empty_when_all_backed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::create_dir_all(root.join("memories")).unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("tasks/task-abc.md"), "live").unwrap();
        std::fs::write(root.join("memories/mem-001.md"), "live").unwrap();
        std::fs::write(root.join("notes/note-xyz.md"), "live").unwrap();
        let store = build_test_store();

        let orphans = store.find_orphaned_entries(root);
        assert!(
            orphans.is_empty(),
            "every entry has a backing file, expected no orphans: {orphans:?}"
        );
    }

    #[test]
    fn test_find_orphaned_entries_flags_path_replaced_by_directory() {
        // A path that now resolves to a directory (not merely "gone") must
        // also be flagged — is_file() is false either way, and there is
        // nothing for a by-id read to open. Regression coverage for the
        // aops_fdf19283 collision class (task_5f2c5fa6 checklist item 3).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("memories")).unwrap();
        std::fs::create_dir_all(root.join("tasks/task-abc.md")).unwrap(); // dir, not file
        std::fs::write(root.join("memories/mem-001.md"), "live").unwrap();
        let store = build_test_store();

        let orphans = store.find_orphaned_entries(root);
        let orphan_ids: std::collections::HashSet<&str> =
            orphans.iter().map(|o| o.id.as_str()).collect();
        assert!(
            orphan_ids.contains("task-abc"),
            "path replaced by a directory must be flagged: {orphan_ids:?}"
        );
        assert!(!orphan_ids.contains("mem-001"));
    }

    // ── search ──

    #[test]
    fn test_search_returns_best_match() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        // Query vector aligned with [1,0,0] should match the task entry
        let results = store.search(&[1.0, 0.0, 0.0], 10, root, None, None, None);
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Fix the bug");
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_search_respects_limit() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.5], 2, root, None, None, None);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_sorted_by_score_descending() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.0], 10, root, None, None, None);
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score);
        }
    }

    #[test]
    fn test_search_empty_store() {
        let store = VectorStore::new(3);
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 10, root, None, None, None);
        assert!(results.is_empty());
    }

    // ── type filter tests ──

    #[test]
    fn test_matches_type_filter_logic() {
        assert!(matches_type_filter(Some("task"), None));
        assert!(matches_type_filter(Some("task"), Some("")));
        assert!(matches_type_filter(Some("task"), Some("task")));
        assert!(matches_type_filter(Some("Task"), Some("task")));
        assert!(matches_type_filter(Some("task"), Some("TASK")));
        assert!(!matches_type_filter(Some("note"), Some("task")));
        assert!(!matches_type_filter(None, Some("task")));

        // Comma-separated positive
        assert!(matches_type_filter(Some("task"), Some("task,epic")));
        assert!(matches_type_filter(Some("epic"), Some("task,epic")));
        assert!(!matches_type_filter(Some("note"), Some("task,epic")));

        // Negation
        assert!(!matches_type_filter(Some("task"), Some("!task")));
        assert!(matches_type_filter(Some("note"), Some("!task")));
        assert!(matches_type_filter(Some("template"), Some("!task")));
        assert!(matches_type_filter(None, Some("!task")));

        // Multi-negation
        assert!(!matches_type_filter(Some("task"), Some("!task,!epic")));
        assert!(!matches_type_filter(Some("epic"), Some("!task,!epic")));
        assert!(matches_type_filter(Some("note"), Some("!task,!epic")));
    }

    #[test]
    fn test_search_filter_by_type_task_only() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        // Store contains 1 task, 1 note, 1 memory
        let results = store.search(&[0.5, 0.5, 0.5], 10, root, None, None, Some("task"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_type.as_deref(), Some("task"));
        assert_eq!(results[0].title, "Fix the bug");
    }

    #[test]
    fn test_search_filter_by_type_negation_excludes_tasks() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        // !task should return note and memory, excluding task
        let results = store.search(&[0.5, 0.5, 0.5], 10, root, None, None, Some("!task"));
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_ne!(r.doc_type.as_deref(), Some("task"));
        }
    }

    #[test]
    fn test_search_filter_by_unknown_type_returns_empty() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.5], 10, root, None, None, Some("nonexistent_type"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_without_type_filter_returns_all_matches() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[0.5, 0.5, 0.5], 10, root, None, None, None);
        assert_eq!(results.len(), 3);
    }

    // ── search date filters (since/before on modified) ──

    fn build_dated_store() -> VectorStore {
        let mut store = VectorStore::new(3);
        // Three entries with known modified dates spanning a wide range.
        let mut old = make_entry(
            "tasks/old.md",
            "Old task",
            Some("task"),
            Some("active"),
            &[],
            Some("task-old"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        old.modified = Some("2019-06-01T00:00:00Z".to_string());

        let mut mid = make_entry(
            "tasks/mid.md",
            "Mid task",
            Some("task"),
            Some("active"),
            &[],
            Some("task-mid"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        mid.modified = Some("2023-03-15T12:00:00Z".to_string());

        let mut new = make_entry(
            "tasks/new.md",
            "New task",
            Some("task"),
            Some("active"),
            &[],
            Some("task-new"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        new.modified = Some("2026-06-01T09:00:00Z".to_string());

        let mut no_date = make_entry(
            "tasks/nodated.md",
            "No date",
            Some("task"),
            Some("active"),
            &[],
            Some("task-nodated"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        no_date.modified = None;

        store.documents.insert("task-old".to_string(), old);
        store.documents.insert("task-mid".to_string(), mid);
        store.documents.insert("task-new".to_string(), new);
        store.documents.insert("task-nodated".to_string(), no_date);
        store
    }

    #[test]
    fn test_search_before_ancient_date_returns_zero() {
        // Repro from task body: before=2020-01-01 must return zero rows.
        let store = build_dated_store();
        let results = store.search(
            &[1.0, 0.0, 0.0],
            10,
            Path::new("/pkb"),
            None,
            Some("2020-01-01"),
            None,
        );
        // Only the 2019 entry qualifies (modified <= 2020-01-01), but that's pre-2020 so it's in.
        // Actually 2019-06-01 <= 2020-01-01 so it should appear.
        // The key correctness check: no entry from 2023 or 2026 appears.
        for r in &results {
            let date_prefix = r.date.as_deref().unwrap_or("");
            let mod_prefix = &r.path.to_string_lossy().to_string();
            // verify only the 2019 entry slips through
            assert!(
                !mod_prefix.contains("mid") && !mod_prefix.contains("new"),
                "before=2020-01-01 must not include 2023 or 2026 entries, got: {mod_prefix}"
            );
        }
    }

    #[test]
    fn test_search_before_ancient_excludes_all_recent() {
        // A stricter version: before=2018-01-01 returns zero rows (all fixtures are after 2018).
        let store = build_dated_store();
        let results = store.search(
            &[1.0, 0.0, 0.0],
            10,
            Path::new("/pkb"),
            None,
            Some("2018-01-01"),
            None,
        );
        assert!(
            results.is_empty(),
            "before=2018-01-01 must return zero rows; got {} rows",
            results.len()
        );
    }

    #[test]
    fn test_search_since_future_returns_zero() {
        // since=2030-01-01 should return nothing (all fixtures are before 2030).
        let store = build_dated_store();
        let results = store.search(
            &[1.0, 0.0, 0.0],
            10,
            Path::new("/pkb"),
            Some("2030-01-01"),
            None,
            None,
        );
        assert!(
            results.is_empty(),
            "since=2030-01-01 must return zero rows; got {} rows",
            results.len()
        );
    }

    #[test]
    fn test_search_since_before_different_results() {
        // Wide range includes all dated entries; narrow range must be a strict subset.
        let store = build_dated_store();
        let root = Path::new("/pkb");
        let wide = store.search(&[1.0, 0.0, 0.0], 10, root, Some("2000-01-01"), None, None);
        let narrow = store.search(&[1.0, 0.0, 0.0], 10, root, Some("2024-01-01"), None, None);
        assert!(
            narrow.len() < wide.len(),
            "narrowing since must reduce result count: wide={} narrow={}",
            wide.len(),
            narrow.len()
        );
    }

    #[test]
    fn test_search_no_modified_excluded_when_filter_active() {
        // An entry without a modified date must be excluded when since/before is set.
        let store = build_dated_store();
        let results = store.search(
            &[1.0, 0.0, 0.0],
            10,
            Path::new("/pkb"),
            Some("2000-01-01"),
            None,
            None,
        );
        let titles: Vec<_> = results.iter().map(|r| r.title.as_str()).collect();
        assert!(
            !titles.contains(&"No date"),
            "entry with no modified date must be excluded by date filter; got: {titles:?}"
        );
    }

    // ── remove / remove_deleted ──

    #[test]
    fn test_remove_existing() {
        let mut store = build_test_store();
        assert_eq!(store.len(), 3);
        assert!(store.remove("task-abc"));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut store = build_test_store();
        assert!(!store.remove("nonexistent"));
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn test_remove_deleted() {
        let mut store = build_test_store();
        let mut existing = std::collections::HashSet::new();
        existing.insert("task-abc".to_string());
        // Only keep task-abc, remove the other two
        let removed = store.remove_deleted_by_ids(&existing);
        assert_eq!(removed, 2);
        assert_eq!(store.len(), 1);
    }

    // ── needs_update ──

    #[test]
    fn test_needs_update_new_doc() {
        let store = build_test_store();
        assert!(store.needs_update("new-doc", "any_hash"));
    }

    #[test]
    fn test_needs_update_stale() {
        let store = build_test_store();
        // Stored hash is "test_hash_123", different hash means changed
        assert!(store.needs_update("task-abc", "different_hash"));
    }

    #[test]
    fn test_needs_update_fresh() {
        let store = build_test_store();
        // Stored hash is "test_hash_123", same hash means unchanged
        assert!(!store.needs_update("task-abc", "test_hash_123"));
    }

    #[test]
    fn test_needs_update_missing_hash() {
        // Documents with None content_hash (old format) should always need update
        let mut store = VectorStore::new(3);
        let mut entry = make_entry(
            "tasks/old-task.md",
            "Old Task",
            Some("task"),
            None,
            &[],
            Some("old-task"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        entry.content_hash = None;
        store.documents.insert("old-task".to_string(), entry);

        assert!(store.needs_update("old-task", "any_hash"));
    }

    #[test]
    fn test_needs_update_empty_hash() {
        // Documents with empty content_hash should also need update
        let mut store = VectorStore::new(3);
        let mut entry = make_entry(
            "tasks/old-task.md",
            "Old Task",
            Some("task"),
            None,
            &[],
            Some("old-task"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        entry.content_hash = Some(String::new());
        store.documents.insert("old-task".to_string(), entry);

        assert!(store.needs_update("old-task", "any_hash"));
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
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec!["test".to_string()],
            frontmatter: None,
            content_hash: "test_doc_hash".to_string(),
            file_hash: "test_doc_hash".to_string(),
        };
        store.insert_precomputed(&doc, vec!["chunk1".to_string()], vec![vec![1.0, 0.0, 0.0]]);
        assert_eq!(store.len(), 1);
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 1, root, None, None, None);
        assert_eq!(results[0].title, "Test Doc");
    }
    #[test]
    fn test_search_uses_chunk_texts_for_snippets() {
        let store = build_test_store();
        let root = Path::new("/pkb");
        let results = store.search(&[1.0, 0.0, 0.0], 1, root, None, None, None);
        assert!(results[0].snippet.contains("body of"));
    }

    #[test]
    fn test_race_condition_resolved_with_locking() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("pkb_vectors.bin");
        let dimension = 3;

        // Simulate Process A
        {
            let mut lock_a = VectorStore::acquire_lock(&db_path).unwrap();
            let _guard_a = lock_a.write().unwrap();

            let mut sa = VectorStore::load_or_create(&db_path, dimension).unwrap();
            let doc_a = crate::pkb::PkbDocument {
                path: PathBuf::from("doc_a.md"),
                title: "Doc A".to_string(),
                body: "Body A".to_string(),
                doc_type: None,
                status: None,
                consolidated: None,
                consolidated_at: None,
                modified: None,
                tags: vec![],
                frontmatter: None,
                content_hash: "hash_a".to_string(),
                file_hash: "hash_a".to_string(),
            };
            sa.insert_precomputed(&doc_a, vec!["A".to_string()], vec![vec![1.0, 0.0, 0.0]]);
            sa.save(&db_path).unwrap();
        } // Lock A released

        // Simulate Process B
        {
            let mut lock_b = VectorStore::acquire_lock(&db_path).unwrap();
            let _guard_b = lock_b.write().unwrap();

            let mut sb = VectorStore::load_or_create(&db_path, dimension).unwrap();
            let doc_b = crate::pkb::PkbDocument {
                path: PathBuf::from("doc_b.md"),
                title: "Doc B".to_string(),
                body: "Body B".to_string(),
                doc_type: None,
                status: None,
                consolidated: None,
                consolidated_at: None,
                modified: None,
                tags: vec![],
                frontmatter: None,
                content_hash: "hash_b".to_string(),
                file_hash: "hash_b".to_string(),
            };
            sb.insert_precomputed(&doc_b, vec!["B".to_string()], vec![vec![0.0, 1.0, 0.0]]);
            sb.save(&db_path).unwrap();
        } // Lock B released

        // Reload and check
        let final_store = VectorStore::load_or_create(&db_path, dimension).unwrap();
        let has_a = final_store.get_entry("doc_a").is_some();
        let has_b = final_store.get_entry("doc_b").is_some();

        assert!(has_a, "Doc A should be present");
        assert!(has_b, "Doc B should be present");
    }

    /// A frontmatter-only change (e.g. title) on a document with the same body
    /// must NOT re-run the embedder and must NOT mutate the cached
    /// `chunk_embeddings` / `chunk_texts` / `body_chunks`. This is the contract
    /// that keeps metadata edits cheap.
    #[test]
    fn test_metadata_only_update_preserves_embeddings_without_embedder() {
        let mut store = VectorStore::new(3);

        // Seed an entry with known embeddings.
        let path = PathBuf::from("tasks/cheap-meta.md");
        let body = "the unchanging body of the document".to_string();
        let body_hash = blake3::hash(body.as_bytes()).to_hex().to_string();
        let original_embedding = vec![vec![0.1, 0.2, 0.3]];
        let original_chunks = vec!["seeded chunk text".to_string()];
        let original_body_chunks = vec!["seeded body chunk".to_string()];
        store.documents.insert(
            "cheap-meta".to_string(),
            DocumentEntry {
                path: path.clone(),
                title: "Original Title".to_string(),
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                tags: vec!["old".to_string()],
                id: "cheap-meta".to_string(),
                date: None,
                consolidated: None,
                consolidated_at: None,
                modified: None,
                confidence: None,
                content_hash: Some(body_hash.clone()),
                file_hash: Some("file_v1".to_string()),
                body_hash: Some(body_hash.clone()),
                chunk_embeddings: original_embedding.clone(),
                chunk_texts: original_chunks.clone(),
                body_chunks: original_body_chunks.clone(),
            },
        );

        // Build a PkbDocument with the SAME body but changed title/tags/status.
        let updated_doc = crate::pkb::PkbDocument {
            path: path.clone(),
            title: "Brand New Title".to_string(),
            tags: vec!["fresh".to_string()],
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            body: body.clone(),
            content_hash: body_hash.clone(),
            file_hash: "file_v2".to_string(),
            frontmatter: None,
        };

        let existing = store.get_entry("cheap-meta").cloned();

        // A dummy embedder would panic if invoked when not is_dummy. We rely
        // on prepare_upsert taking the metadata-only path so the embedder is
        // never called. Use a real (is_dummy=true) embedder that returns
        // zeros — if the slow path is taken accidentally, the assertions on
        // chunk_embeddings/chunk_texts below would fail loudly.
        let embedder = embeddings::Embedder::new_dummy();
        let prepared =
            VectorStore::prepare_upsert(&updated_doc, &embedder, existing.as_ref()).unwrap();
        assert!(
            matches!(prepared, PreparedUpsert::MetadataOnly(_)),
            "expected metadata-only path when body hash matches"
        );
        store.apply_prepared(prepared);

        let after = store.get_entry("cheap-meta").expect("entry present");
        assert_eq!(after.title, "Brand New Title");
        assert_eq!(after.status.as_deref(), Some("done"));
        assert_eq!(after.tags, vec!["fresh".to_string()]);
        assert_eq!(after.file_hash.as_deref(), Some("file_v2"));
        assert_eq!(
            after.chunk_embeddings, original_embedding,
            "cached embeddings must be preserved on metadata-only update"
        );
        assert_eq!(
            after.chunk_texts, original_chunks,
            "chunk_texts must be preserved on metadata-only update"
        );
        assert_eq!(
            after.body_chunks, original_body_chunks,
            "body_chunks must be preserved on metadata-only update"
        );
    }

    /// Body change must take the Full path and produce a fresh entry.
    #[test]
    fn test_body_change_takes_full_path() {
        let mut store = VectorStore::new(3);
        let path = PathBuf::from("tasks/body-change.md");
        let old_body_hash = blake3::hash(b"old body").to_hex().to_string();
        store.documents.insert(
            "body-change".to_string(),
            DocumentEntry {
                path: path.clone(),
                title: "T".to_string(),
                doc_type: None,
                status: None,
                tags: vec![],
                id: "body-change".to_string(),
                date: None,
                consolidated: None,
                consolidated_at: None,
                modified: None,
                confidence: None,
                content_hash: Some(old_body_hash.clone()),
                file_hash: Some("file_v1".to_string()),
                body_hash: Some(old_body_hash),
                chunk_embeddings: vec![vec![1.0, 0.0, 0.0]],
                chunk_texts: vec!["old".to_string()],
                body_chunks: vec!["old".to_string()],
            },
        );
        let new_body = "totally different body content here";
        let new_body_hash = blake3::hash(new_body.as_bytes()).to_hex().to_string();
        let doc = crate::pkb::PkbDocument {
            path: path.clone(),
            title: "T".to_string(),
            tags: vec![],
            doc_type: None,
            status: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            body: new_body.to_string(),
            content_hash: new_body_hash,
            file_hash: "file_v2".to_string(),
            frontmatter: None,
        };
        let existing = store.get_entry("body-change").cloned();
        let embedder = embeddings::Embedder::new_dummy();
        let prepared = VectorStore::prepare_upsert(&doc, &embedder, existing.as_ref()).unwrap();
        assert!(
            matches!(prepared, PreparedUpsert::Full(_)),
            "expected Full path when body hash changes"
        );
    }

    // ── WAL Persistence Tests ──

    #[test]
    fn test_wal_append_and_replay() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let wal = dir.path().join("test_store.wal");

        let entry = make_entry(
            "tasks/task-wal-1.md",
            "Task WAL 1",
            Some("task"),
            Some("active"),
            &["wal-tag"],
            Some("task-wal-1"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        VectorStore::append_wal_record(&wal, &WalRecord::Upsert(Box::new(entry))).unwrap();

        let patch = MetadataPatch {
            id: "task-wal-1".to_string(),
            title: "Task WAL 1 (Updated)".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            tags: vec!["wal-tag".to_string(), "done-tag".to_string()],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: Some("2026-08-27".to_string()),
            confidence: Some(0.95),
            file_hash: "updated_hash".to_string(),
        };
        VectorStore::append_wal_record(&wal, &WalRecord::MetadataOnly(patch)).unwrap();

        let mut store = VectorStore::new(3);
        let count = store.replay_wal(&wal).unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.len(), 1);

        let item = store.get_entry("task-wal-1").expect("item exists");
        assert_eq!(item.title, "Task WAL 1 (Updated)");
        assert_eq!(item.status.as_deref(), Some("done"));
        assert_eq!(item.chunk_embeddings, vec![vec![1.0, 0.0, 0.0]]);

        // Append Remove record
        VectorStore::append_wal_record(&wal, &WalRecord::Remove("task-wal-1".to_string())).unwrap();
        let mut store2 = VectorStore::new(3);
        let count2 = store2.replay_wal(&wal).unwrap();
        assert_eq!(count2, 3);
        assert_eq!(store2.len(), 0);
    }

    #[test]
    fn test_wal_replay_corrupted_tail_graceful() {
        use std::io::Write;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let wal = dir.path().join("test_corrupt.wal");

        let entry = make_entry(
            "tasks/valid.md",
            "Valid Task",
            Some("task"),
            Some("active"),
            &[],
            Some("valid-task"),
            None,
            vec![0.5, 0.5, 0.0],
        );
        VectorStore::append_wal_record(&wal, &WalRecord::Upsert(Box::new(entry))).unwrap();

        // Append incomplete / corrupted bytes at end of file (simulating power failure / SIGKILL mid-write)
        let mut file = std::fs::OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(&[0x10, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        file.flush().unwrap();
        drop(file);

        let mut store = VectorStore::new(3);
        let count = store.replay_wal(&wal).unwrap();
        // Should have replayed the 1 valid record and stopped cleanly without error
        assert_eq!(count, 1);
        assert_eq!(store.len(), 1);
        assert!(store.get_entry("valid-task").is_some());
    }

    #[test]
    fn test_wal_snapshot_compaction() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("pkb_vectors.bin");
        let wal_path = dir.path().join("pkb_vectors.wal");

        let mut store = VectorStore::new(3);
        let entry = make_entry(
            "tasks/t1.md",
            "T1",
            Some("task"),
            Some("active"),
            &[],
            Some("t1"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        store.documents.insert("t1".to_string(), entry.clone());
        store.save(&db_path).unwrap();
        assert!(!wal_path.exists());

        // Append incremental update to WAL
        let patch = MetadataPatch {
            id: "t1".to_string(),
            title: "T1 Patched".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            tags: vec![],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence: None,
            file_hash: "fh".to_string(),
        };
        VectorStore::append_wal_record(&wal_path, &WalRecord::MetadataOnly(patch)).unwrap();
        assert!(wal_path.exists());

        // load_or_create should load snapshot + replay WAL
        let loaded = VectorStore::load_or_create(&db_path, 3).unwrap();
        assert_eq!(loaded.get_entry("t1").unwrap().title, "T1 Patched");

        // Saving snapshot compacts and removes WAL
        loaded.save(&db_path).unwrap();
        assert!(!wal_path.exists(), "Snapshot save must remove compacted WAL file");

        // Re-load should find fresh snapshot with no WAL
        let reloaded = VectorStore::load_or_create(&db_path, 3).unwrap();
        assert_eq!(reloaded.get_entry("t1").unwrap().title, "T1 Patched");
    }

    #[test]
    fn test_wal_replay_oversized_length_graceful() {
        use std::io::Write;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let wal = dir.path().join("test_oversized.wal");

        let entry = make_entry(
            "tasks/v1.md",
            "Valid Task 1",
            Some("task"),
            Some("active"),
            &[],
            Some("v1"),
            None,
            vec![0.1, 0.2, 0.3],
        );
        VectorStore::append_wal_record(&wal, &WalRecord::Upsert(Box::new(entry))).unwrap();

        // Append an invalid oversized length (e.g. 0xFFFFFFFF = ~4GB or 32MB > 16MB)
        let mut file = std::fs::OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(&0x0200_0000u32.to_le_bytes()).unwrap(); // 32 MB > 16 MB limit
        file.flush().unwrap();
        drop(file);

        let mut store = VectorStore::new(3);
        let count = store.replay_wal(&wal).unwrap();
        assert_eq!(count, 1);
        assert_eq!(store.len(), 1);
        assert!(store.get_entry("v1").is_some());
    }

    #[test]
    fn test_wal_compacting_incoming_writes_preserved() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("pkb_vectors.bin");
        let wal_path = dir.path().join("pkb_vectors.wal");
        let wal_compacting_path = dir.path().join("pkb_vectors.wal.compacting");

        let mut store = VectorStore::new(3);
        let entry1 = make_entry(
            "tasks/t1.md",
            "T1 Initial",
            Some("task"),
            Some("active"),
            &[],
            Some("t1"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        store.documents.insert("t1".to_string(), entry1);
        store.save(&db_path).unwrap();

        // Write a WAL record
        let patch1 = MetadataPatch {
            id: "t1".to_string(),
            title: "T1 Mod1".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("in_progress".to_string()),
            tags: vec![],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence: None,
            file_hash: "fh1".to_string(),
        };
        VectorStore::append_wal_record(&wal_path, &WalRecord::MetadataOnly(patch1)).unwrap();

        // Simulate save rotating WAL to .wal.compacting
        std::fs::rename(&wal_path, &wal_compacting_path).unwrap();

        // While compacting is in-flight, an incoming write arrives and creates a fresh .wal
        let patch2 = MetadataPatch {
            id: "t1".to_string(),
            title: "T1 Mod2 Incoming".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            tags: vec![],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence: None,
            file_hash: "fh2".to_string(),
        };
        VectorStore::append_wal_record(&wal_path, &WalRecord::MetadataOnly(patch2)).unwrap();

        // Save finishes and removes ONLY .wal.compacting
        if wal_compacting_path.exists() {
            std::fs::remove_file(&wal_compacting_path).unwrap();
        }
        assert!(wal_path.exists(), "Concurrent incoming WAL must NOT be deleted by save");

        // On reload, the fresh incoming WAL must be replayed
        let loaded = VectorStore::load_or_create(&db_path, 3).unwrap();
        assert_eq!(loaded.get_entry("t1").unwrap().title, "T1 Mod2 Incoming");
        assert_eq!(loaded.get_entry("t1").unwrap().status.as_deref(), Some("done"));
    }

    #[test]
    fn test_wal_replay_both_compacting_and_wal() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("pkb_vectors.bin");
        let wal_path = dir.path().join("pkb_vectors.wal");
        let wal_compacting_path = dir.path().join("pkb_vectors.wal.compacting");

        let mut store = VectorStore::new(3);
        let entry1 = make_entry(
            "tasks/t1.md",
            "T1 Initial",
            Some("task"),
            Some("active"),
            &[],
            Some("t1"),
            None,
            vec![1.0, 0.0, 0.0],
        );
        store.documents.insert("t1".to_string(), entry1);
        store.save(&db_path).unwrap();

        // Record 1 in compacting WAL (from crashed save)
        let patch1 = MetadataPatch {
            id: "t1".to_string(),
            title: "T1 Mid".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("in_progress".to_string()),
            tags: vec![],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence: None,
            file_hash: "fh1".to_string(),
        };
        VectorStore::append_wal_record(&wal_compacting_path, &WalRecord::MetadataOnly(patch1)).unwrap();

        // Record 2 in new WAL
        let patch2 = MetadataPatch {
            id: "t1".to_string(),
            title: "T1 Final".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            tags: vec![],
            date: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            confidence: None,
            file_hash: "fh2".to_string(),
        };
        VectorStore::append_wal_record(&wal_path, &WalRecord::MetadataOnly(patch2)).unwrap();

        // Load or create must replay both in sequence
        let loaded = VectorStore::load_or_create(&db_path, 3).unwrap();
        assert_eq!(loaded.get_entry("t1").unwrap().title, "T1 Final");
        assert_eq!(loaded.get_entry("t1").unwrap().status.as_deref(), Some("done"));
    }

    #[test]
    fn test_legacy_store_migration_and_dimension_check() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let legacy_fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/legacy_v1_store.bin");

        // 1. Verify the checked-in fixture exists on disk
        assert!(
            legacy_fixture_path.exists(),
            "checked-in legacy vector store fixture must exist at {}",
            legacy_fixture_path.display()
        );

        let fixture_bytes = std::fs::read(&legacy_fixture_path).unwrap();

        // 2. Primary VectorStore deserialize MUST fail because fixture is in pre-break layout
        // (missing consolidated/consolidated_at fields)
        let primary_result = bincode::deserialize::<VectorStore>(&fixture_bytes);
        assert!(
            primary_result.is_err(),
            "Primary VectorStore deserialize must fail on legacy fixture binary"
        );

        // 3. Fallback OldVectorStore deserialize MUST succeed
        let legacy_result = bincode::deserialize::<OldVectorStore>(&fixture_bytes);
        assert!(
            legacy_result.is_ok(),
            "OldVectorStore deserialize must succeed on legacy fixture binary"
        );

        // 4. Test load_or_create with matching dimension (3):
        // Copy fixture to a temp location so we can test loading and in-place save
        let test_db_path = dir.path().join("pkb_vectors.bin");
        std::fs::write(&test_db_path, &fixture_bytes).unwrap();

        let migrated = VectorStore::load_or_create(&test_db_path, 3).unwrap();
        assert_eq!(migrated.dimension, 3);
        assert_eq!(migrated.len(), 2);

        let doc1 = migrated.get_entry("legacy-task-1").expect("legacy-task-1 present");
        assert_eq!(doc1.title, "Legacy Task 1");
        assert_eq!(doc1.doc_type.as_deref(), Some("task"));
        assert_eq!(doc1.status.as_deref(), Some("active"));
        assert_eq!(doc1.tags, vec!["legacy", "vectordb"]);
        assert_eq!(doc1.consolidated, None);
        assert_eq!(doc1.consolidated_at, None);
        assert_eq!(doc1.chunk_embeddings, vec![vec![1.0, 0.0, 0.0]]);

        let doc2 = migrated.get_entry("legacy-note-2").expect("legacy-note-2 present");
        assert_eq!(doc2.title, "Legacy Note 2");
        assert_eq!(doc2.doc_type.as_deref(), Some("note"));
        assert_eq!(doc2.status, None);
        assert_eq!(doc2.tags, vec!["notes"]);
        assert_eq!(doc2.consolidated, None);
        assert_eq!(doc2.consolidated_at, None);
        assert_eq!(doc2.chunk_embeddings, vec![vec![0.0, 1.0, 0.0]]);

        // 5. Verify the migrated store was persisted in the new VectorStore schema format:
        // Reading the file now MUST deserialize cleanly as VectorStore directly
        let persisted_bytes = std::fs::read(&test_db_path).unwrap();
        let reload_as_new = bincode::deserialize::<VectorStore>(&persisted_bytes);
        assert!(
            reload_as_new.is_ok(),
            "Persisted vector store must cleanly deserialize as new VectorStore schema without migration fallback"
        );

        // 6. Test dimension mismatch on legacy store (expected 1024, legacy is 3):
        let mismatch_db_path = dir.path().join("pkb_vectors_mismatch.bin");
        std::fs::write(&mismatch_db_path, &fixture_bytes).unwrap();

        let mismatch_store = VectorStore::load_or_create(&mismatch_db_path, 1024).unwrap();
        assert_eq!(mismatch_store.dimension, 1024);
        assert!(
            mismatch_store.is_empty(),
            "Dimension mismatch in legacy store must discard and create fresh empty store"
        );

        // 7. Test corrupted store bytes (neither VectorStore nor OldVectorStore):
        let corrupt_db_path = dir.path().join("pkb_vectors_corrupt.bin");
        std::fs::write(&corrupt_db_path, &[0xFF, 0xFE, 0xFD, 0xFC, 0x00, 0x01]).unwrap();

        let corrupt_store = VectorStore::load_or_create(&corrupt_db_path, 3).unwrap();
        assert_eq!(corrupt_store.dimension, 3);
        assert!(
            corrupt_store.is_empty(),
            "Corrupted binary must discard and create fresh empty store"
        );
    }
}
