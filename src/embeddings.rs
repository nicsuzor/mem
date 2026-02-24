//! Embedding generation using BGE-M3 via ONNX Runtime.
//!
//! Generates 1024-dimensional sentence embeddings for semantic search.
//! Uses asymmetric encoding: queries get an instruction prefix, documents do not.
//! Auto-downloads model files and ONNX Runtime on first run.

use anyhow::{bail, Context, Result};
use ort::session::Session;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const EMBEDDING_DIM: usize = 1024;
const MODEL_REPO: &str = "BAAI/bge-m3";

/// Instruction prefix for query encoding (asymmetric retrieval).
/// Documents are embedded without prefix.
const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// Thread-safe guard for ORT_DYLIB_PATH initialization.
static ORT_PATH_INIT: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();

// =============================================================================
// CONFIGURATION
// =============================================================================

#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    pub max_length: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl EmbeddingConfig {
    /// Preferred model filenames in priority order.
    const MODEL_NAMES: &[&str] = &["model.onnx"];

    pub fn from_env() -> Self {
        let base_path = std::env::var("AOPS_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let candidates = vec![
                    Some(get_cache_dir().join("models/bge-m3")),
                    dirs::data_dir().map(|p| p.join("aops/models/bge-m3")),
                    Some(PathBuf::from("./models/bge-m3")),
                ];

                candidates
                    .into_iter()
                    .flatten()
                    .find(|p| Self::find_model(p).is_some())
                    .unwrap_or_else(|| get_cache_dir().join("models/bge-m3"))
            });

        let model_file = Self::find_model(&base_path)
            .unwrap_or_else(|| "model.onnx".to_string());

        Self {
            model_path: base_path.join(model_file),
            tokenizer_path: base_path.join("tokenizer.json"),
            max_length: 512,
        }
    }

    /// Find the best available model file in a directory.
    fn find_model(dir: &std::path::Path) -> Option<String> {
        Self::MODEL_NAMES
            .iter()
            .find(|name| dir.join(name).exists())
            .map(|s| s.to_string())
    }
}

// =============================================================================
// CACHE & DOWNLOAD
// =============================================================================

pub fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("aops")
}

fn get_models_dir() -> PathBuf {
    get_cache_dir().join("models/bge-m3")
}

/// Download model files from HuggingFace if not present.
/// BGE-M3 uses split ONNX format: model.onnx (graph) + model.onnx_data (weights).
fn download_models() -> Result<PathBuf> {
    let models_dir = get_models_dir();
    std::fs::create_dir_all(&models_dir)?;

    let base_url = format!("https://huggingface.co/{MODEL_REPO}/resolve/main/onnx");

    // BGE-M3 split ONNX: graph (724KB) + weights (2.1GB) + external initializer + tokenizer
    let files = [
        ("model.onnx", format!("{base_url}/model.onnx")),
        ("model.onnx_data", format!("{base_url}/model.onnx_data")),
        ("Constant_7_attr__value", format!("{base_url}/Constant_7_attr__value")),
        ("tokenizer.json", format!("{base_url}/tokenizer.json")),
    ];

    for (filename, url) in &files {
        let dest = models_dir.join(filename);
        if dest.exists() {
            continue;
        }

        eprintln!("  Downloading {filename}...");
        let resp = ureq::get(url)
            .call()
            .with_context(|| format!("Failed to download {url}"))?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&dest)?;
        let bytes = std::io::copy(&mut reader, &mut file)?;
        eprintln!("  ✓ Downloaded {filename} ({:.1} MB)", bytes as f64 / 1_048_576.0);
    }

    Ok(models_dir)
}

/// Download ONNX Runtime shared library if not present.
fn download_onnx_runtime() -> Result<PathBuf> {
    let cache = get_cache_dir().join("onnxruntime");
    std::fs::create_dir_all(&cache)?;

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let (url, lib_name) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-linux-x64-1.23.2.tgz",
        "libonnxruntime.so",
    );

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let (url, lib_name) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-linux-aarch64-1.23.2.tgz",
        "libonnxruntime.so",
    );

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (url, lib_name) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-arm64-1.23.2.tgz",
        "libonnxruntime.dylib",
    );

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let (url, lib_name) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-x86_64-1.23.2.tgz",
        "libonnxruntime.dylib",
    );

    let lib_path = cache.join(lib_name);
    if lib_path.exists() {
        return Ok(lib_path);
    }

    eprintln!("  Downloading ONNX Runtime...");
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("Failed to download ONNX Runtime from {url}"))?;
    let reader = resp.into_reader();
    let decoder = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        // Skip symlinks — they're 0 bytes in tar
        if entry.header().entry_type().is_symlink() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().to_string();
        // Match the real versioned library:
        //   Linux:  libonnxruntime.so.1.23.2    (contains "libonnxruntime.so")
        //   macOS:  libonnxruntime.1.23.2.dylib (version before .dylib extension)
        let is_ort = path.contains(lib_name)
            || (path.contains("libonnxruntime.") && path.ends_with(".dylib"));
        if is_ort && entry.size() > 0 {
            let mut file = std::fs::File::create(&lib_path)?;
            std::io::copy(&mut entry, &mut file)?;
            eprintln!("  ✓ Downloaded ONNX Runtime ({:.1} MB)", entry.size() as f64 / 1_048_576.0);
            return Ok(lib_path);
        }
    }

    bail!("Could not find {lib_name} in ONNX Runtime archive")
}

fn get_onnx_runtime_path() -> Option<PathBuf> {
    let cache = get_cache_dir().join("onnxruntime");

    #[cfg(target_os = "linux")]
    let lib_name = "libonnxruntime.so";
    #[cfg(target_os = "macos")]
    let lib_name = "libonnxruntime.dylib";

    let path = cache.join(lib_name);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

// =============================================================================
// SESSION POOL
// =============================================================================

/// Threads per ONNX inference session.
const THREADS_PER_SESSION: usize = 4;

/// Max parallel ONNX sessions, computed from available cores.
/// Pool starts with 1 session (fast startup for search), grows on demand for reindex.
fn max_sessions() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    // Use all cores: e.g. 48 cores / 4 threads = 12 sessions
    // Clamp to at least 2 and at most 24 (avoid excessive memory with large models)
    (cores / THREADS_PER_SESSION).clamp(2, 24)
}

struct SessionPool {
    sessions: parking_lot::RwLock<Vec<Arc<Mutex<Session>>>>,
    model_path: PathBuf,
    tokenizer: tokenizers::Tokenizer,
    /// Whether the model expects token_type_ids as input (BERT: yes, XLM-RoBERTa: no).
    uses_token_type_ids: bool,
}

impl SessionPool {
    /// Create pool with a single session (fast startup for search).
    /// Additional sessions are added lazily via `ensure_sessions()`.
    fn new(config: &EmbeddingConfig) -> Result<Self> {
        use ort::session::builder::GraphOptimizationLevel;

        let session = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(THREADS_PER_SESSION))
            .and_then(|b| b.commit_from_file(&config.model_path))
            .with_context(|| format!("Failed to load ONNX model from {:?}", config.model_path))?;

        // Detect whether model expects token_type_ids (BERT does, XLM-RoBERTa does not)
        let uses_token_type_ids = session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids");

        let mut tokenizer = tokenizers::Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer from {:?}: {e}", config.tokenizer_path))?;

        // BGE-M3 (XLM-RoBERTa): pad_id=1, pad_token="<pad>"
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::Fixed(config.max_length),
            pad_id: 1,
            pad_token: "<pad>".to_string(),
            ..Default::default()
        }));
        tokenizer.with_truncation(Some(tokenizers::TruncationParams {
            max_length: config.max_length,
            ..Default::default()
        })).map_err(|e| anyhow::anyhow!("Failed to set truncation: {e}"))?;

        Ok(Self {
            sessions: parking_lot::RwLock::new(vec![Arc::new(Mutex::new(session))]),
            model_path: config.model_path.clone(),
            tokenizer,
            uses_token_type_ids,
        })
    }

    /// Grow pool to `count` sessions (no-op if already large enough).
    /// Called before parallel batch encoding.
    fn ensure_sessions(&self, count: usize) -> Result<()> {
        use ort::session::builder::GraphOptimizationLevel;

        let max_sess = max_sessions();
        let current = self.sessions.read().len();
        let needed = count.min(max_sess);
        if current >= needed {
            return Ok(());
        }

        let to_add = needed - current;
        eprintln!("  Scaling to {needed} ONNX sessions ({THREADS_PER_SESSION} threads each)...");

        let model_path = &self.model_path;
        let new_sessions: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..to_add)
                .map(|_| {
                    s.spawn(|| {
                        Session::builder()
                            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                            .and_then(|b| b.with_intra_threads(THREADS_PER_SESSION))
                            .and_then(|b| b.commit_from_file(model_path))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut sessions = self.sessions.write();
        for s in new_sessions {
            sessions.push(Arc::new(Mutex::new(
                s.with_context(|| format!("Failed to load ONNX model from {:?}", self.model_path))?,
            )));
        }

        Ok(())
    }

    /// Get the first available (unlocked) session, or block on session 0.
    fn acquire_session(&self) -> Arc<Mutex<Session>> {
        let sessions = self.sessions.read();
        // Try each session without blocking
        for session in sessions.iter() {
            if session.try_lock().is_some() {
                return Arc::clone(session);
            }
        }
        // All busy — return first (caller will block on lock)
        Arc::clone(&sessions[0])
    }
}

// =============================================================================
// EMBEDDER
// =============================================================================

pub struct Embedder {
    config: EmbeddingConfig,
    pool: OnceLock<std::result::Result<Arc<SessionPool>, String>>,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let offline = std::env::var("AOPS_OFFLINE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        // 1. Ensure ONNX Runtime is available
        ensure_ort_available(offline)
            .context("ONNX Runtime is required. Set ORT_DYLIB_PATH or allow auto-download.")?;

        // 2. Ensure model files exist (download_models is a no-op if already cached)
        let mut config = EmbeddingConfig::from_env();

        if !config.model_path.exists() || !config.tokenizer_path.exists() {
            if offline {
                bail!(
                    "Model files not found and AOPS_OFFLINE=true.\n\
                     Download manually from https://huggingface.co/{MODEL_REPO}/tree/main/onnx:\n  \
                       model.onnx, model.onnx_data, Constant_7_attr__value, tokenizer.json\n\
                     Place them in: {:?}",
                    config.model_path.parent().unwrap_or(&config.model_path)
                );
            }

            let models_dir = download_models()?;
            let model_file = EmbeddingConfig::find_model(&models_dir)
                .unwrap_or_else(|| "model_quint8_avx2.onnx".to_string());
            config.model_path = models_dir.join(model_file);
            config.tokenizer_path = models_dir.join("tokenizer.json");
        }

        let max_sess = max_sessions();
        tracing::info!(
            "Using BGE-M3 ONNX embeddings ({EMBEDDING_DIM}-dim, up to {max_sess} sessions × {THREADS_PER_SESSION} threads)"
        );
        Ok(Self {
            config,
            pool: OnceLock::new(),
        })
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    /// Encode a query with instruction prefix (asymmetric retrieval).
    pub fn encode_query(&self, text: &str) -> Result<Vec<f32>> {
        let prefixed = format!("{QUERY_PREFIX}{text}");
        self.encode(&prefixed)
    }

    /// Encode a document without prefix (asymmetric retrieval).
    pub fn encode_document(&self, text: &str) -> Result<Vec<f32>> {
        self.encode(text)
    }

    fn ensure_pool(&self) -> Result<&Arc<SessionPool>> {
        let result = self.pool.get_or_init(|| {
            SessionPool::new(&self.config)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        });

        match result {
            Ok(pool) => Ok(pool),
            Err(e) => bail!("Failed to load ONNX model: {e}"),
        }
    }

    pub fn encode(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.encode_batch(&[text])?;
        Ok(results.into_iter().next().unwrap_or_else(|| vec![0.0; EMBEDDING_DIM]))
    }

    const MAX_BATCH: usize = 128;

    pub fn encode_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let pool = self.ensure_pool()?;

        // For large inputs, scale up sessions and split in parallel
        if texts.len() > Self::MAX_BATCH {
            use rayon::prelude::*;
            let sub_batches: Vec<&[&str]> = texts.chunks(Self::MAX_BATCH).collect();
            pool.ensure_sessions(sub_batches.len())?;
            let results: Vec<Result<Vec<Vec<f32>>>> = sub_batches
                .par_iter()
                .map(|batch| self.encode_single_batch(batch, pool))
                .collect();
            let mut all = Vec::with_capacity(texts.len());
            for r in results {
                all.extend(r?);
            }
            return Ok(all);
        }

        self.encode_single_batch(texts, pool)
    }

    fn encode_single_batch(&self, texts: &[&str], pool: &Arc<SessionPool>) -> Result<Vec<Vec<f32>>> {
        let configured_max = self.config.max_length;
        let batch_size = texts.len();

        // Tokenize all texts at once
        let encodings = pool
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("Batch tokenization failed: {e}"))?;

        // Dynamic padding: use the actual max token count in this batch
        // instead of always padding to configured_max. This is the biggest
        // speedup — most chunks use far fewer than 256 tokens.
        let max_length = encodings
            .iter()
            .map(|enc| enc.get_ids().len().min(configured_max))
            .max()
            .unwrap_or(configured_max);

        // Build flat batched tensors [batch_size, max_length]
        let total_len = batch_size * max_length;
        let mut input_ids_data = vec![0i64; total_len];
        let mut attention_data = vec![0i64; total_len];

        for (batch_idx, encoding) in encodings.iter().enumerate() {
            let offset = batch_idx * max_length;
            for (i, &token) in encoding.get_ids().iter().take(max_length).enumerate() {
                input_ids_data[offset + i] = token as i64;
            }
            for (i, &mask) in encoding.get_attention_mask().iter().take(max_length).enumerate() {
                attention_data[offset + i] = mask as i64;
            }
        }

        let shape = [batch_size, max_length];

        use ort::value::TensorRef;
        let input_ids_val = TensorRef::from_array_view((shape, input_ids_data.as_slice()))?;
        let attention_val = TensorRef::from_array_view((shape, attention_data.as_slice()))?;

        // Acquire a session from the pool and run inference
        let session_arc = pool.acquire_session();
        let mut session = session_arc.lock();

        // Conditionally include token_type_ids (BERT needs them, XLM-RoBERTa does not)
        let outputs = if pool.uses_token_type_ids {
            let token_type_data = vec![0i64; total_len];
            let token_types_val = TensorRef::from_array_view((shape, token_type_data.as_slice()))?;
            session.run(ort::inputs![
                input_ids_val, attention_val, token_types_val
            ])?
        } else {
            session.run(ort::inputs![
                input_ids_val, attention_val
            ])?
        };

        // Extract output — shape [batch_size, seq_len, EMBEDDING_DIM]
        let (_out_shape, out_data) = outputs[0].try_extract_tensor::<f32>()?;
        let seq_dim = max_length;

        // Mean pooling + L2 normalize for each item in batch
        let mut results = Vec::with_capacity(batch_size);
        for batch_idx in 0..batch_size {
            let batch_offset = batch_idx * seq_dim * EMBEDDING_DIM;
            let attn_offset = batch_idx * max_length;

            let mut pooled = vec![0.0f32; EMBEDDING_DIM];
            let mut mask_sum = 0.0f32;

            for seq_idx in 0..seq_dim {
                if attention_data[attn_offset + seq_idx] == 1 {
                    let token_offset = batch_offset + seq_idx * EMBEDDING_DIM;
                    for dim_idx in 0..EMBEDDING_DIM {
                        pooled[dim_idx] += out_data[token_offset + dim_idx];
                    }
                    mask_sum += 1.0;
                }
            }

            if mask_sum > 0.0 {
                for val in &mut pooled {
                    *val /= mask_sum;
                }
            }

            // Clean NaN/Inf
            for val in pooled.iter_mut() {
                if val.is_nan() || val.is_infinite() {
                    *val = 0.0;
                }
            }

            // L2 normalize
            let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > f32::EPSILON && !norm.is_nan() {
                for val in &mut pooled {
                    *val /= norm;
                }
            }

            results.push(pooled);
        }

        Ok(results)
    }
}

fn ensure_ort_available(offline: bool) -> Result<()> {
    let result = ORT_PATH_INIT.get_or_init(|| init_ort_path(offline));
    match result {
        Ok(_) => Ok(()),
        Err(e) => bail!("{e}"),
    }
}

fn init_ort_path(offline: bool) -> std::result::Result<PathBuf, String> {
    // Check existing env var
    if let Ok(existing) = std::env::var("ORT_DYLIB_PATH") {
        let path = PathBuf::from(&existing);
        if path.exists() {
            return Ok(path);
        }
    }

    // Check cache
    if let Some(cached) = get_onnx_runtime_path() {
        std::env::set_var("ORT_DYLIB_PATH", &cached);
        return Ok(cached);
    }

    if offline {
        return Err("ONNX Runtime not found and AOPS_OFFLINE=true".to_string());
    }

    let path = download_onnx_runtime().map_err(|e| e.to_string())?;
    std::env::set_var("ORT_DYLIB_PATH", &path);
    Ok(path)
}

// =============================================================================
// TEXT CHUNKING
// =============================================================================

pub struct ChunkConfig {
    pub chunk_size: usize,
    pub overlap: usize,
    pub min_chunk_size: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            chunk_size: 2000,
            overlap: 500,
            min_chunk_size: 300,
        }
    }
}

pub fn chunk_text(text: &str, config: &ChunkConfig) -> Vec<String> {
    let text = text.trim();
    if text.len() <= config.chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + config.chunk_size).min(text.len());

        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }

        if end < text.len() {
            if let Some(break_pos) = find_sentence_break(text, start, end) {
                end = break_pos;
            }
        }

        if start >= end {
            break;
        }

        let chunk = text[start..end].trim();
        if chunk.len() >= config.min_chunk_size || chunks.is_empty() {
            chunks.push(chunk.to_string());
        } else if let Some(last) = chunks.last_mut() {
            last.push(' ');
            last.push_str(chunk);
        }

        if end >= text.len() {
            break;
        }

        // Ensure forward progress: new start must be past old start
        let prev_start = start;
        start = end.saturating_sub(config.overlap);
        if start <= prev_start {
            start = end; // no overlap if it would go backward
        }
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }

    chunks
}

fn find_sentence_break(text: &str, start: usize, ideal_end: usize) -> Option<usize> {
    let chunk = &text[start..ideal_end];

    let mut last_boundary = None;
    for (i, c) in chunk.char_indices() {
        if (c == '.' || c == '!' || c == '?') && i >= 100 {
            let after = i + c.len_utf8();
            if after >= chunk.len() || chunk[after..].starts_with(char::is_whitespace) {
                last_boundary = Some(start + after);
            }
        }
    }

    last_boundary
}
