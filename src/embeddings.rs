//! Embedding generation using MiniLM-L6-v2 via ONNX Runtime.
//!
//! Generates 384-dimensional sentence embeddings for semantic search.
//! Auto-downloads model files and ONNX Runtime on first run.

use anyhow::{bail, Context, Result};
use ort::session::Session;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const EMBEDDING_DIM: usize = 384;
const MODEL_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";

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
    pub fn from_env() -> Self {
        let base_path = std::env::var("SHODH_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let candidates = vec![
                    Some(get_cache_dir().join("models/minilm-l6")),
                    dirs::data_dir().map(|p| p.join("shodh-memory/models/minilm-l6")),
                    Some(PathBuf::from("./models/minilm-l6")),
                ];

                candidates
                    .into_iter()
                    .flatten()
                    .find(|p| p.join("model.onnx").exists())
                    .unwrap_or_else(|| get_cache_dir().join("models/minilm-l6"))
            });

        Self {
            model_path: base_path.join("model.onnx"),
            tokenizer_path: base_path.join("tokenizer.json"),
            max_length: 256,
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

/// Download model files from HuggingFace if not present.
fn download_models() -> Result<PathBuf> {
    let models_dir = get_models_dir();
    std::fs::create_dir_all(&models_dir)?;

    let base_url = format!("https://huggingface.co/{MODEL_REPO}/resolve/main");

    let files = [
        ("model.onnx", format!("{base_url}/onnx/model.onnx")),
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
        std::io::copy(&mut reader, &mut file)?;
        eprintln!("  ✓ Downloaded {filename}");
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
        // Match the real versioned library (e.g. libonnxruntime.so.1.17.0)
        if path.contains(lib_name) && entry.size() > 0 {
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
            .commit_from_file(&config.model_path)
            .with_context(|| format!("Failed to load ONNX model from {:?}", config.model_path))?;

        let tokenizer = tokenizers::Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer from {:?}: {e}", config.tokenizer_path))?;

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
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let offline = std::env::var("SHODH_OFFLINE")
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
                    "Model files not found and SHODH_OFFLINE=true.\n\
                     Download manually:\n  \
                       model:     https://huggingface.co/{MODEL_REPO}/resolve/main/onnx/model.onnx\n  \
                       tokenizer: https://huggingface.co/{MODEL_REPO}/resolve/main/tokenizer.json\n\
                     Place them in: {:?}",
                    config.model_path.parent().unwrap_or(&config.model_path)
                );
            }

            let models_dir = download_models()?;
            config.model_path = models_dir.join("model.onnx");
            config.tokenizer_path = models_dir.join("tokenizer.json");
        }

        eprintln!("  Using MiniLM-L6-v2 ONNX embeddings ({EMBEDDING_DIM}-dim)");
        Ok(Self {
            config,
            lazy_model: OnceLock::new(),
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
            Err(e) => bail!("Failed to load ONNX model: {e}"),
        }
    }

    pub fn encode(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Ok(vec![0.0; EMBEDDING_DIM]);
        }

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

        // Build inputs
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

        use ort::value::TensorRef;
        let input_ids_val = TensorRef::from_array_view((shape, input_ids_data.as_slice()))?;
        let attention_val = TensorRef::from_array_view((shape, attention_data.as_slice()))?;
        let token_types_val = TensorRef::from_array_view((shape, token_type_data.as_slice()))?;

        let outputs = session.run(ort::inputs![
            input_ids_val, attention_val, token_types_val
        ])?;

        // Extract output — shape [1, seq_len, 384]
        let (out_shape, out_data) = outputs[0].try_extract_tensor::<f32>()?;
        let out_dim = out_shape.last().copied().unwrap_or(EMBEDDING_DIM as i64) as usize;

        // Mean pooling over sequence dimension (attention-masked)
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

    pub fn encode_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|text| self.encode(text)).collect()
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
        return Err("ONNX Runtime not found and SHODH_OFFLINE=true".to_string());
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
