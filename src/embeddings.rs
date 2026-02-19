//! Embedding generation using MiniLM-L6-v2 via ONNX Runtime.
//!
//! Generates 384-dimensional sentence embeddings for semantic search.
//! Uses ort v2 API. Falls back to hash-based embeddings if ONNX fails.

use anyhow::Result;

use ort::session::Session;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const EMBEDDING_DIM: usize = 384;

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
    pub use_quantized: bool,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl EmbeddingConfig {
    pub fn from_env() -> Self {
        let base_path = std::env::var("SHODH_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let candidates = vec![
                    Some(PathBuf::from("./models/minilm-l6")),
                    Some(PathBuf::from("../models/minilm-l6")),
                    Some(get_cache_dir().join("models/minilm-l6")),
                    dirs::data_dir().map(|p| p.join("shodh-memory/models/minilm-l6")),
                ];

                candidates
                    .into_iter()
                    .flatten()
                    .find(|p| {
                        p.join("model_quantized.onnx").exists() || p.join("model.onnx").exists()
                    })
                    .unwrap_or_else(|| get_cache_dir().join("models/minilm-l6"))
            });

        let use_quantized = std::env::var("SHODH_USE_QUANTIZED_MODEL")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);

        let model_filename = if use_quantized {
            "model_quantized.onnx"
        } else {
            "model.onnx"
        };

        Self {
            model_path: base_path.join(model_filename),
            tokenizer_path: base_path.join("tokenizer.json"),
            max_length: 256,
            use_quantized,
        }
    }
}

// =============================================================================
// CACHE & DOWNLOAD
// =============================================================================

pub fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("shodh-memory")
}

fn get_models_dir() -> PathBuf {
    get_cache_dir().join("models/minilm-l6")
}

/// Download model files from HuggingFace if not present
fn download_models() -> Result<PathBuf> {
    let models_dir = get_models_dir();
    std::fs::create_dir_all(&models_dir)?;

    let base_url = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";

    let files = [
        ("model.onnx", format!("{base_url}/onnx/model.onnx")),
        (
            "model_quantized.onnx",
            format!("{base_url}/onnx/model_quantized.onnx"),
        ),
        ("tokenizer.json", format!("{base_url}/tokenizer.json")),
    ];

    for (filename, url) in &files {
        let dest = models_dir.join(filename);
        if dest.exists() {
            continue;
        }

        tracing::info!("Downloading {filename}...");
        let resp = ureq::get(url).call()?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&dest)?;
        std::io::copy(&mut reader, &mut file)?;
        tracing::info!("Downloaded {filename}");
    }

    Ok(models_dir)
}

/// Download ONNX Runtime shared library if not present
fn download_onnx_runtime() -> Result<PathBuf> {
    let cache = get_cache_dir().join("onnxruntime");
    std::fs::create_dir_all(&cache)?;

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let (url, lib_name, archive_dir_prefix) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.17.0/onnxruntime-linux-x64-1.17.0.tgz",
        "libonnxruntime.so",
        "onnxruntime-linux-x64-1.17.0",
    );

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let (url, lib_name, archive_dir_prefix) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.17.0/onnxruntime-linux-aarch64-1.17.0.tgz",
        "libonnxruntime.so",
        "onnxruntime-linux-aarch64-1.17.0",
    );

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (url, lib_name, archive_dir_prefix) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.17.0/onnxruntime-osx-arm64-1.17.0.tgz",
        "libonnxruntime.dylib",
        "onnxruntime-osx-arm64-1.17.0",
    );

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let (url, lib_name, archive_dir_prefix) = (
        "https://github.com/microsoft/onnxruntime/releases/download/v1.17.0/onnxruntime-osx-x86_64-1.17.0.tgz",
        "libonnxruntime.dylib",
        "onnxruntime-osx-x86_64-1.17.0",
    );

    // Check if already downloaded
    let lib_path = cache.join(lib_name);
    if lib_path.exists() {
        return Ok(lib_path);
    }

    tracing::info!("Downloading ONNX Runtime from {url}...");
    let resp = ureq::get(url).call()?;
    let reader = resp.into_reader();
    let decoder = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(decoder);

    let lib_relative = format!("{archive_dir_prefix}/lib/{lib_name}");

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        if path == lib_relative || path.ends_with(lib_name) {
            let mut file = std::fs::File::create(&lib_path)?;
            std::io::copy(&mut entry, &mut file)?;
            tracing::info!("Extracted ONNX Runtime to {lib_path:?}");
            return Ok(lib_path);
        }
    }

    anyhow::bail!("Could not find {lib_name} in ONNX Runtime archive")
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
// LAZY MODEL
// =============================================================================

struct LazyModel {
    session: Mutex<Session>,
    tokenizer: tokenizers::Tokenizer,
}

impl LazyModel {
    fn new(config: &EmbeddingConfig) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(2)?
            .commit_from_file(&config.model_path)?;

        let tokenizer = tokenizers::Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }
}

// =============================================================================
// EMBEDDER
// =============================================================================

pub struct Embedder {
    config: EmbeddingConfig,
    lazy_model: OnceLock<std::result::Result<Arc<LazyModel>, String>>,
    simplified_mode: bool,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let config = EmbeddingConfig::from_env();

        let offline_mode = std::env::var("SHODH_OFFLINE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        // Ensure ORT_DYLIB_PATH is set
        if let Err(e) = ensure_ort_available(offline_mode) {
            tracing::warn!("ONNX Runtime not available: {e}. Using simplified embeddings.");
            return Ok(Self {
                config,
                lazy_model: OnceLock::new(),
                simplified_mode: true,
            });
        }

        // Check if model files exist, try downloading if not
        if !config.model_path.exists() || !config.tokenizer_path.exists() {
            if offline_mode {
                tracing::warn!("Model files not found and SHODH_OFFLINE=true. Using simplified embeddings.");
                return Ok(Self {
                    config,
                    lazy_model: OnceLock::new(),
                    simplified_mode: true,
                });
            }

            tracing::info!("Model files not found. Downloading...");
            match download_models() {
                Ok(models_dir) => {
                    let model_filename = if config.use_quantized {
                        "model_quantized.onnx"
                    } else {
                        "model.onnx"
                    };
                    let updated_config = EmbeddingConfig {
                        model_path: models_dir.join(model_filename),
                        tokenizer_path: models_dir.join("tokenizer.json"),
                        ..config
                    };
                    return Ok(Self {
                        config: updated_config,
                        lazy_model: OnceLock::new(),
                        simplified_mode: false,
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to download models: {e}. Using simplified embeddings.");
                    return Ok(Self {
                        config,
                        lazy_model: OnceLock::new(),
                        simplified_mode: true,
                    });
                }
            }
        }

        tracing::info!("Using MiniLM-L6-v2 ONNX embeddings (384-dim)");
        Ok(Self {
            config,
            lazy_model: OnceLock::new(),
            simplified_mode: false,
        })
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    fn ensure_model_loaded(&self) -> Result<&Arc<LazyModel>> {
        let result = self.lazy_model.get_or_init(|| {
            LazyModel::new(&self.config)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        });

        match result {
            Ok(model) => Ok(model),
            Err(e) => Err(anyhow::anyhow!("Failed to load model: {e}")),
        }
    }

    pub fn encode(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Ok(vec![0.0; EMBEDDING_DIM]);
        }

        if self.simplified_mode {
            return self.encode_simplified(text);
        }

        match self.encode_onnx(text) {
            Ok(embedding) => Ok(embedding),
            Err(e) => {
                tracing::warn!("ONNX inference failed: {e}. Falling back to simplified.");
                self.encode_simplified(text)
            }
        }
    }

    pub fn encode_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|text| self.encode(text)).collect()
    }

    fn encode_onnx(&self, text: &str) -> Result<Vec<f32>> {
        let model = self.ensure_model_loaded()?;

        let lock_timeout = std::time::Duration::from_secs(30);
        let mut session = model
            .session
            .try_lock_for(lock_timeout)
            .ok_or_else(|| anyhow::anyhow!("ONNX session lock timeout"))?;

        let encoding = model
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        let tokens = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();
        let max_length = self.config.max_length;

        // Build ndarray inputs
        let mut input_ids_data = vec![0i64; max_length];
        let mut attention_data = vec![0i64; max_length];
        let token_type_data = vec![0i64; max_length];

        for (i, &token) in tokens.iter().take(max_length).enumerate() {
            input_ids_data[i] = token as i64;
        }
        for (i, &mask) in attention_mask.iter().take(max_length).enumerate() {
            attention_data[i] = mask as i64;
        }

        let shape = [1usize, max_length];

        // ort v2: use (shape, &[T]) tuple form for tensor inputs
        use ort::value::TensorRef;
        let input_ids_val = TensorRef::from_array_view((shape, input_ids_data.as_slice()))?;
        let attention_val = TensorRef::from_array_view((shape, attention_data.as_slice()))?;
        let token_types_val = TensorRef::from_array_view((shape, token_type_data.as_slice()))?;

        let outputs = session.run(ort::inputs![
            input_ids_val, attention_val, token_types_val
        ])?;

        // Extract output — try_extract_tensor returns (&Shape, &[f32])
        let (out_shape, out_data) = outputs[0].try_extract_tensor::<f32>()?;
        let out_dim = out_shape.last().copied().unwrap_or(EMBEDDING_DIM as i64) as usize;

        // Mean pooling over sequence dimension
        let mut pooled = vec![0.0f32; EMBEDDING_DIM];
        let mut mask_sum = 0.0f32;

        for seq_idx in 0..max_length {
            if attention_data[seq_idx] == 1 {
                let offset = seq_idx * out_dim;
                for dim_idx in 0..EMBEDDING_DIM.min(out_dim) {
                    pooled[dim_idx] += out_data[offset + dim_idx];
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

        Ok(pooled)
    }

    /// Hash-based fallback embeddings when ONNX is unavailable
    fn encode_simplified(&self, text: &str) -> Result<Vec<f32>> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();

        // Word unigrams
        for (i, word) in words.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            word.hash(&mut hasher);
            let hash = hasher.finish();

            for j in 0..8 {
                let dim = ((hash >> (j * 8)) as usize ^ i.wrapping_mul(31)) % EMBEDDING_DIM;
                let sign = if (hash >> (j + 32)) & 1 == 0 {
                    1.0
                } else {
                    -1.0
                };
                embedding[dim] += sign * 0.15;
            }
        }

        // Word bigrams
        for pair in words.windows(2) {
            let mut hasher = DefaultHasher::new();
            pair[0].hash(&mut hasher);
            pair[1].hash(&mut hasher);
            let hash = hasher.finish();

            for j in 0..4 {
                let dim = ((hash >> (j * 16)) as usize) % EMBEDDING_DIM;
                let sign = if (hash >> (j + 48)) & 1 == 0 {
                    1.0
                } else {
                    -1.0
                };
                embedding[dim] += sign * 0.1;
            }
        }

        // Character trigrams
        let chars: Vec<char> = lower.chars().collect();
        for window in chars.windows(3) {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            let hash = hasher.finish();

            let dim = (hash as usize) % EMBEDDING_DIM;
            let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };
            embedding[dim] += sign * 0.05;
        }

        // L2 normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > f32::EPSILON && !norm.is_nan() {
            for val in &mut embedding {
                *val /= norm;
            }
        }

        Ok(embedding)
    }
}

fn ensure_ort_available(offline_mode: bool) -> Result<()> {
    let result = ORT_PATH_INIT.get_or_init(|| init_ort_path(offline_mode));
    match result {
        Ok(_) => Ok(()),
        Err(e) => anyhow::bail!("{e}"),
    }
}

fn init_ort_path(offline_mode: bool) -> std::result::Result<PathBuf, String> {
    // Check existing env var
    if let Ok(existing) = std::env::var("ORT_DYLIB_PATH") {
        let path = PathBuf::from(&existing);
        if path.exists() {
            tracing::debug!("Using existing ONNX Runtime from ORT_DYLIB_PATH: {path:?}");
            return Ok(path);
        }
    }

    // Check cache
    if let Some(cached) = get_onnx_runtime_path() {
        tracing::info!("Setting ORT_DYLIB_PATH to cached runtime: {cached:?}");
        std::env::set_var("ORT_DYLIB_PATH", &cached);
        return Ok(cached);
    }

    if offline_mode {
        return Err("ONNX Runtime not found and SHODH_OFFLINE=true".to_string());
    }

    tracing::info!("ONNX Runtime not found. Downloading...");
    let path = download_onnx_runtime().map_err(|e| e.to_string())?;
    tracing::info!("Setting ORT_DYLIB_PATH to downloaded runtime: {path:?}");
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
            chunk_size: 800,
            overlap: 200,
            min_chunk_size: 200,
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

        start = end.saturating_sub(config.overlap);
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
