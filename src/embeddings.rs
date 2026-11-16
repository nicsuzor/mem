//! Embedding generation using BGE-M3 via ONNX Runtime.
//!
//! Generates 1024-dimensional sentence embeddings for semantic search.
//! Uses asymmetric encoding: queries get an instruction prefix, documents do not.
//! Auto-downloads model files and ONNX Runtime on first run.

use anyhow::{bail, Context, Result};
use ort::session::Session;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

pub const EMBEDDING_DIM: usize = 1024;
pub const DEFAULT_QUERY_DENSE_TOKEN_CAP: usize = 160;
const DEFAULT_HF_BASE_URL: &str = "https://huggingface.co";
const MODEL_REPO: &str = "BAAI/bge-m3";
const FP16_MODEL_REPO: &str = "Xenova/bge-m3";

/// Instruction prefix for query encoding (asymmetric retrieval).
/// Documents are embedded without prefix.
/// BGE-M3 does NOT need an instruction prefix (unlike BGE v1/v1.5).
/// See: https://huggingface.co/BAAI/bge-m3
const QUERY_PREFIX: &str = "";

/// Thread-safe guard for ORT_DYLIB_PATH initialization.
static ORT_PATH_INIT: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();

/// Cached GPU availability check (computed once at startup).
static GPU_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check if CUDA GPU is available for ONNX Runtime inference.
/// Looks for libcuda.so in WSL2 paravirtualization path and standard locations, or nvcuda.dll on Windows.
fn gpu_available() -> bool {
    *GPU_AVAILABLE.get_or_init(|| {
        // Allow explicit override: AOPS_GPU=0 disables, AOPS_GPU=1 forces
        if let Ok(val) = std::env::var("AOPS_GPU") {
            return val == "1" || val.to_lowercase() == "true";
        }

        #[cfg(target_os = "windows")]
        let found = std::path::Path::new("C:\\Windows\\System32\\nvcuda.dll").exists();

        #[cfg(not(target_os = "windows"))]
        let found = {
            let cuda_paths = [
                "/usr/lib/wsl/lib/libcuda.so",
                "/usr/lib/x86_64-linux-gnu/libcuda.so",
                "/usr/local/cuda/lib64/libcudart.so",
            ];
            cuda_paths.iter().any(|p| std::path::Path::new(p).exists())
        };

        if found {
            tracing::info!("CUDA libraries detected, GPU acceleration available");
        }
        found
    })
}

/// Pre-load the WSL2 paravirtual `libcuda.so.1` into the global symbol namespace
/// before ORT loads its CUDA execution provider.
///
/// On WSL2 the real GPU driver lives at `/usr/lib/wsl/lib/libcuda.so.1`. If a native
/// Linux NVIDIA driver package is also installed it drops a *second* `libcuda.so.1`
/// under `/usr/lib/x86_64-linux-gnu/` which the dynamic loader resolves first (it is in
/// the ld.so cache). The CUDA runtime then loads the non-WSL driver and `cudaSetDevice`
/// fails with "no CUDA-capable device is detected", so ORT silently falls back to CPU.
///
/// Modifying `LD_LIBRARY_PATH` from inside the process does not help — glibc parses it
/// once at startup. Instead we `dlopen` the WSL driver by absolute path with
/// `RTLD_GLOBAL`, registering it under the soname `libcuda.so.1`. When ORT's CUDA
/// provider is later loaded, the loader reuses this already-resident object instead of
/// pulling in the native driver. No-op when the WSL driver is absent (i.e. not WSL).
#[cfg(target_os = "linux")]
fn preload_wsl_libcuda() {
    use std::sync::OnceLock;
    static PRELOADED: OnceLock<bool> = OnceLock::new();
    PRELOADED.get_or_init(|| {
        let wsl_libcuda = "/usr/lib/wsl/lib/libcuda.so.1";
        if !std::path::Path::new(wsl_libcuda).exists() {
            return false;
        }
        let c_path = match std::ffi::CString::new(wsl_libcuda) {
            Ok(p) => p,
            Err(_) => return false,
        };
        // SAFETY: dlopen with a valid NUL-terminated path; we never dlclose (process-lifetime).
        let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if handle.is_null() {
            tracing::warn!(
                "WSL libcuda preload failed (dlopen returned null) — GPU may be unavailable"
            );
            false
        } else {
            tracing::info!("Preloaded WSL libcuda from {wsl_libcuda} for CUDA EP");
            true
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn preload_wsl_libcuda() {}

// =============================================================================
// CONFIGURATION
// =============================================================================

#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    pub max_length: usize,
    pub query_dense_token_cap: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Which BGE-M3 ONNX weights to load.
///
/// Selected with `pkb --model <auto|quantized|fp16|fp32>` or `AOPS_MODEL_VARIANT`.
/// The int8 `quantized` model is fastest on CPU, but its dynamic-quantisation
/// ops run poorly under the CUDA execution provider (about 10x slower than
/// `fp32` on an RTX 3080), so GPU reindexing should use `fp32` or `fp16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelVariant {
    /// Prefer `model_quantized.onnx`, fall back to `model.onnx`.
    #[default]
    Auto,
    /// Require the int8 `model_quantized.onnx`.
    Quantized,
    /// Require the half-precision `model_fp16.onnx`.
    Fp16,
    /// Require the full-precision `model.onnx` (+ `model.onnx_data`).
    Fp32,
}

impl ModelVariant {
    /// Env var carrying the variant (set by the `--model` CLI switch).
    pub const ENV: &'static str = "AOPS_MODEL_VARIANT";

    pub fn from_env() -> Self {
        std::env::var(Self::ENV)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ModelVariant::Auto => "auto",
            ModelVariant::Quantized => "quantized",
            ModelVariant::Fp16 => "fp16",
            ModelVariant::Fp32 => "fp32",
        }
    }

    /// Candidate model filenames in priority order.
    fn candidates(self) -> &'static [&'static str] {
        match self {
            ModelVariant::Auto => &["model_quantized.onnx", "model.onnx"],
            ModelVariant::Quantized => &["model_quantized.onnx"],
            ModelVariant::Fp16 => &["model_fp16.onnx"],
            ModelVariant::Fp32 => &["model.onnx"],
        }
    }
}

impl std::str::FromStr for ModelVariant {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(ModelVariant::Auto),
            "quantized" => Ok(ModelVariant::Quantized),
            "fp16" => Ok(ModelVariant::Fp16),
            "fp32" => Ok(ModelVariant::Fp32),
            other => Err(format!(
                "unknown model variant '{other}' (expected auto, quantized, fp16 or fp32)"
            )),
        }
    }
}

impl EmbeddingConfig {
    pub fn from_env() -> Self {
        let variant = ModelVariant::from_env();
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
                    .find(|p| Self::find_model_variant(p, variant).is_some())
                    .unwrap_or_else(|| get_cache_dir().join("models/bge-m3"))
            });

        let mut config = Self::for_dir(&base_path, variant);
        if let Some(cap) = std::env::var("AOPS_QUERY_DENSE_TOKEN_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            config.query_dense_token_cap = cap;
        }
        config
    }

    /// Resolve configuration for an explicit model directory and variant without reading env vars.
    pub fn for_dir(base_path: &std::path::Path, variant: ModelVariant) -> Self {
        let model_file = Self::find_model_variant(base_path, variant)
            .unwrap_or_else(|| variant.candidates()[0].to_string());

        Self {
            model_path: base_path.join(model_file),
            tokenizer_path: base_path.join("tokenizer.json"),
            max_length: 512,
            query_dense_token_cap: DEFAULT_QUERY_DENSE_TOKEN_CAP,
        }
    }

    /// Ensure model and tokenizer files exist in `self.model_path.parent()`,
    /// downloading missing artifacts for `variant` from `hf_base_url` unless `offline` is set.
    fn ensure_files(
        &mut self,
        variant: ModelVariant,
        offline: bool,
        hf_base_url: &str,
    ) -> Result<()> {
        if self.model_path.exists() && self.tokenizer_path.exists() {
            return Ok(());
        }

        let models_dir = self
            .model_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(get_models_dir);

        if offline {
            match variant {
                ModelVariant::Fp16 => bail!(
                    "Model files not found and AOPS_OFFLINE=true.\n\
                     Download manually from https://huggingface.co/{FP16_MODEL_REPO}/tree/main:\n  \
                       onnx/model_fp16.onnx -> model_fp16.onnx, tokenizer.json\n\
                     Place them in: {models_dir:?}"
                ),
                ModelVariant::Auto | ModelVariant::Quantized | ModelVariant::Fp32 => bail!(
                    "Model files not found and AOPS_OFFLINE=true.\n\
                     Download manually from https://huggingface.co/{MODEL_REPO}/tree/main/onnx:\n  \
                       model.onnx, model.onnx_data, Constant_7_attr__value, tokenizer.json\n\
                     Place them in: {models_dir:?}"
                ),
            }
        }

        download_models_into(&models_dir, variant, hf_base_url)?;
        let model_file = Self::find_model_variant(&models_dir, variant).with_context(|| {
            format!(
                "No {} model in {models_dir:?} (looked for {:?})",
                variant.as_str(),
                variant.candidates()
            )
        })?;
        self.model_path = models_dir.join(model_file);
        self.tokenizer_path = models_dir.join("tokenizer.json");
        Ok(())
    }

    /// Find the best available model file in a directory.
    #[cfg(test)]
    fn find_model(dir: &std::path::Path) -> Option<String> {
        Self::find_model_variant(dir, ModelVariant::Auto)
    }

    fn find_model_variant(dir: &std::path::Path, variant: ModelVariant) -> Option<String> {
        variant
            .candidates()
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

/// Download missing model files for `variant` from `hf_base_url` into `models_dir`.
///
/// - `ModelVariant::Fp16` fetches self-contained `onnx/model_fp16.onnx` and root `tokenizer.json`
///   from `Xenova/bge-m3`.
/// - Other variants fetch split ONNX artifacts (`model.onnx`, `model.onnx_data`,
///   `Constant_7_attr__value`, `tokenizer.json`) from `BAAI/bge-m3`.
///
/// Existing destination files are never overwritten, and in-flight downloads stream to a
/// PID-scoped `.{filename}.{pid}.part` file before atomic rename so concurrent processes
/// sharing a cache directory never unlink each other's temp files.
fn download_models_into(
    models_dir: &std::path::Path,
    variant: ModelVariant,
    hf_base_url: &str,
) -> Result<()> {
    std::fs::create_dir_all(models_dir)?;

    let hf_base = hf_base_url.trim_end_matches('/');
    let files: Vec<(&str, String)> = match variant {
        ModelVariant::Fp16 => {
            let repo_base = format!("{hf_base}/{FP16_MODEL_REPO}/resolve/main");
            vec![
                (
                    "model_fp16.onnx",
                    format!("{repo_base}/onnx/model_fp16.onnx"),
                ),
                ("tokenizer.json", format!("{repo_base}/tokenizer.json")),
            ]
        }
        ModelVariant::Auto | ModelVariant::Quantized | ModelVariant::Fp32 => {
            let base_url = format!("{hf_base}/{MODEL_REPO}/resolve/main/onnx");
            vec![
                ("model.onnx", format!("{base_url}/model.onnx")),
                ("model.onnx_data", format!("{base_url}/model.onnx_data")),
                (
                    "Constant_7_attr__value",
                    format!("{base_url}/Constant_7_attr__value"),
                ),
                ("tokenizer.json", format!("{base_url}/tokenizer.json")),
            ]
        }
    };

    let pid = std::process::id();
    for (filename, url) in &files {
        let dest = models_dir.join(filename);
        if dest.exists() {
            continue;
        }

        let tmp = models_dir.join(format!(".{filename}.{pid}.part"));
        eprintln!("  Downloading {filename}...");
        let download_result = (|| -> Result<u64> {
            let resp = ureq::get(url)
                .call()
                .with_context(|| format!("Failed to download {url}"))?;
            let mut reader = resp.into_reader();
            let mut file = std::fs::File::create(&tmp)?;
            let bytes = std::io::copy(&mut reader, &mut file)?;
            drop(file);
            std::fs::rename(&tmp, &dest)
                .with_context(|| format!("Failed to move {tmp:?} to {dest:?}"))?;
            Ok(bytes)
        })();

        match download_result {
            Ok(bytes) => {
                eprintln!(
                    "  ✓ Downloaded {filename} ({:.1} MB)",
                    bytes as f64 / 1_048_576.0
                );
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(err);
            }
        }
    }

    Ok(())
}

const ORT_NUGET_CPU_URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.23.2/microsoft.ml.onnxruntime.1.23.2.nupkg";
const ORT_NUGET_GPU_LINUX_URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.gpu.linux/1.23.2/microsoft.ml.onnxruntime.gpu.linux.1.23.2.nupkg";
const ORT_NUGET_GPU_WINDOWS_URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.gpu.windows/1.23.2/microsoft.ml.onnxruntime.gpu.windows.1.23.2.nupkg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OrtNugetSpec {
    url: &'static str,
    rid: &'static str,
    lib_name: &'static str,
}

/// Resolve the authoritative NuGet `.nupkg` URL, runtime identifier (RID), and main library
/// filename for the given `(os, arch, use_gpu)` combination. Fails fast on unsupported targets
/// or GPU variants without a published NuGet package.
fn ort_nuget_spec(os: &str, arch: &str, use_gpu: bool) -> Result<OrtNugetSpec> {
    match (os, arch, use_gpu) {
        ("linux", "x86_64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "linux-x64",
            lib_name: "libonnxruntime.so",
        }),
        ("linux", "aarch64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "linux-arm64",
            lib_name: "libonnxruntime.so",
        }),
        ("macos", "aarch64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "osx-arm64",
            lib_name: "libonnxruntime.dylib",
        }),
        ("macos", "x86_64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "osx-x64",
            lib_name: "libonnxruntime.dylib",
        }),
        ("windows", "x86_64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "win-x64",
            lib_name: "onnxruntime.dll",
        }),
        ("windows", "aarch64", false) => Ok(OrtNugetSpec {
            url: ORT_NUGET_CPU_URL,
            rid: "win-arm64",
            lib_name: "onnxruntime.dll",
        }),
        ("linux", "x86_64", true) => Ok(OrtNugetSpec {
            url: ORT_NUGET_GPU_LINUX_URL,
            rid: "linux-x64",
            lib_name: "libonnxruntime.so",
        }),
        ("windows", "x86_64", true) => Ok(OrtNugetSpec {
            url: ORT_NUGET_GPU_WINDOWS_URL,
            rid: "win-x64",
            lib_name: "onnxruntime.dll",
        }),
        (_, _, true) => bail!("ONNX Runtime GPU/CUDA is not supported on {os}/{arch}"),
        (_, _, false) => bail!("Unsupported OS/architecture for ONNX Runtime: {os}/{arch}"),
    }
}

fn is_native_shared_lib(filename: &str) -> bool {
    filename.ends_with(".so")
        || filename.split_once(".so.").is_some_and(|(_, ver)| {
            !ver.is_empty() && ver.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        || filename.ends_with(".dylib")
        || filename.ends_with(".dll")
}

/// Extract native shared libraries matching `runtimes/<rid>/native/` from a NuGet `.nupkg` ZIP
/// archive directly into `dest_dir` (flattened, preserving filename, guarded against traversal).
fn extract_ort_from_zip<R: std::io::Read + std::io::Seek>(
    reader: R,
    rid: &str,
    lib_name: &str,
    dest_dir: &std::path::Path,
) -> Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    let mut archive =
        zip::ZipArchive::new(reader).context("Failed to read ONNX Runtime NuGet ZIP archive")?;
    let prefix = format!("runtimes/{rid}/native/");
    let mut found_main = false;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(fname) = entry.name().strip_prefix(&prefix).map(str::to_string) else {
            continue;
        };
        if fname.is_empty()
            || fname.contains('/')
            || fname.contains('\\')
            || fname == "."
            || fname == ".."
            || entry.enclosed_name().is_none()
        {
            continue;
        }
        if !is_native_shared_lib(&fname) {
            continue;
        }

        let dest = dest_dir.join(&fname);
        let mut out = std::fs::File::create(&dest)?;
        let bytes = std::io::copy(&mut entry, &mut out)?;
        if fname == lib_name {
            found_main = true;
        }
        eprintln!("  + {fname} ({:.1} MB)", bytes as f64 / 1_048_576.0);
    }

    if !found_main {
        bail!("Could not find {lib_name} under {prefix} in ONNX Runtime NuGet archive");
    }
    Ok(())
}

struct TempFileGuard<'a>(&'a std::path::Path);

impl Drop for TempFileGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Download ONNX Runtime shared library if not present.
/// Downloads the GPU (CUDA) build when CUDA is detected, otherwise CPU-only.
/// For GPU builds, extracts all shared libraries (CUDA EP providers must be
/// co-located with libonnxruntime.so for runtime discovery).
fn download_onnx_runtime() -> Result<PathBuf> {
    let use_gpu = gpu_available();
    let spec = ort_nuget_spec(std::env::consts::OS, std::env::consts::ARCH, use_gpu)?;
    let cache = get_cache_dir().join("onnxruntime");
    std::fs::create_dir_all(&cache)?;

    // If we have the GPU build marker but are requesting CPU (or vice versa),
    // we need to re-download. Use a marker file to track which variant is cached.
    let variant_marker = cache.join(".variant");
    let cached_variant = std::fs::read_to_string(&variant_marker).unwrap_or_default();
    let wanted_variant = if use_gpu { "gpu" } else { "cpu" };

    let lib_path = cache.join(spec.lib_name);
    if lib_path.exists() && cached_variant.trim() == wanted_variant {
        return Ok(lib_path);
    }

    let variant_label = if use_gpu {
        "ONNX Runtime (GPU/CUDA)"
    } else {
        "ONNX Runtime"
    };
    eprintln!("  Downloading {variant_label}...");
    let resp = ureq::get(spec.url)
        .call()
        .with_context(|| format!("Failed to download {variant_label} from {}", spec.url))?;

    let tmp_zip = cache.join(".onnxruntime.nupkg.tmp");
    let _cleanup = TempFileGuard(&tmp_zip);
    {
        let mut tmp_file = std::fs::File::create(&tmp_zip)?;
        let mut reader = resp.into_reader();
        std::io::copy(&mut reader, &mut tmp_file).with_context(|| {
            format!(
                "Failed to write downloaded ONNX Runtime archive to {}",
                tmp_zip.display()
            )
        })?;
    }
    let zip_file = std::fs::File::open(&tmp_zip)?;
    extract_ort_from_zip(zip_file, spec.rid, spec.lib_name, &cache)?;

    // Write variant marker so we know which build is cached
    let _ = std::fs::write(&variant_marker, wanted_variant);
    eprintln!("  Done ({variant_label})");

    Ok(lib_path)
}

/// True if the cached ONNX Runtime build satisfies the execution-provider variant
/// we want for this machine. The CPU build is always acceptable when we don't want
/// GPU. When CUDA is available we additionally require the CUDA EP provider library
/// to be present next to libonnxruntime.so — the GPU build co-locates it there, and
/// without it ORT falls back to CPU even though `CUDA::default()` "registers".
fn onnx_cache_satisfies_variant() -> bool {
    if !gpu_available() {
        return true;
    }
    let cache = get_cache_dir().join("onnxruntime");

    #[cfg(target_os = "windows")]
    let cuda_provider = "onnxruntime_providers_cuda.dll";
    #[cfg(not(target_os = "windows"))]
    let cuda_provider = "libonnxruntime_providers_cuda.so";

    cache.join(cuda_provider).exists()
}

fn get_onnx_runtime_path() -> Option<PathBuf> {
    let cache = get_cache_dir().join("onnxruntime");

    #[cfg(target_os = "linux")]
    let lib_name = "libonnxruntime.so";
    #[cfg(target_os = "macos")]
    let lib_name = "libonnxruntime.dylib";
    #[cfg(target_os = "windows")]
    let lib_name = "onnxruntime.dll";

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
const THREADS_PER_SESSION: usize = 8;

#[derive(Debug)]
struct LruQueryCache {
    capacity: usize,
    map: std::collections::HashMap<String, Vec<f32>>,
    order: std::collections::VecDeque<String>,
}

impl LruQueryCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Vec<f32>> {
        if let Some(val) = self.map.get(key) {
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key.to_string());
            Some(val.clone())
        } else {
            None
        }
    }

    fn insert(&mut self, key: String, val: Vec<f32>) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), val);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            return;
        }

        if self.map.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(key.clone(), val);
        self.order.push_back(key);
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

/// Max parallel ONNX sessions for CPU mode, computed from available cores.
/// Pool pre-warms with enough sessions so concurrent searches (such as a 5-search fan-out)
/// do not serialise behind busy sessions or stall on disk loading.
fn max_sessions_cpu() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if cores <= 1 {
        1
    } else {
        // Multi-core systems with >= 5 cores allocate at least 5 sessions (up to 8)
        // so a fan-out of 5 parallel searches runs concurrently.
        let half = cores / 2;
        let base = half.clamp(2, 8);
        base.max(5.min(cores))
    }
}

fn threads_per_session_cpu(cores: usize, max_sess: usize) -> usize {
    (cores / max_sess).clamp(1, 4)
}

/// Max parallel ONNX sessions for CPU mode: the core-count target, capped by memory.
///
/// Every session holds its own optimised copy of the model weights (BGE-M3: ~1.3 GB
/// resident per session), and each grows an inference arena on top. Sized from cores
/// alone, a 12-core host pre-warms 6 sessions (~8 GB) and a 10 GB container is
/// OOM-killed on the first document embed. The pool therefore takes at most as many
/// sessions as fit in half the memory limit, at one on-disk model size each; the other
/// half is headroom for the graph, the vector store and inference arenas.
fn max_sessions_for(model_path: &std::path::Path) -> usize {
    let cpu = max_sessions_cpu();
    match (memory_limit_bytes(), model_weight_bytes(model_path)) {
        (Some(limit), Some(per_session)) => sessions_within_memory(cpu, limit, per_session),
        _ => cpu,
    }
}

/// `cpu` sessions, reduced to those whose weights fit in half of `limit`. Never below 1.
fn sessions_within_memory(cpu: usize, limit: u64, per_session: u64) -> usize {
    if per_session == 0 {
        return cpu;
    }
    let fit = (limit / 2 / per_session) as usize;
    cpu.min(fit).max(1)
}

/// On-disk size of the model: the graph file plus its external weights file, if any.
fn model_weight_bytes(model_path: &std::path::Path) -> Option<u64> {
    let graph = std::fs::metadata(model_path).ok()?.len();
    let mut data_path = model_path.as_os_str().to_owned();
    data_path.push("_data");
    let data = std::fs::metadata(&data_path).map(|m| m.len()).unwrap_or(0);
    Some(graph + data)
}

/// Memory this process may use: the cgroup limit if one is set, else physical memory.
fn memory_limit_bytes() -> Option<u64> {
    let physical = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| parse_meminfo_total(&s));
    let cgroup = [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .and_then(|s| s.trim().parse::<u64>().ok());
    match (cgroup, physical) {
        (Some(c), Some(p)) => Some(c.min(p)),
        (c, p) => c.or(p),
    }
}

/// `MemTotal` from /proc/meminfo, in bytes.
fn parse_meminfo_total(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|rest| {
            rest.trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|kb| kb * 1024)
}

struct SessionPool {
    sessions: parking_lot::RwLock<Vec<Arc<Mutex<Session>>>>,
    next_session_idx: std::sync::atomic::AtomicUsize,
    model_path: PathBuf,
    tokenizer: tokenizers::Tokenizer,
    /// Whether the model expects token_type_ids as input (BERT: yes, XLM-RoBERTa: no).
    uses_token_type_ids: bool,
    /// True if the session is running on GPU (CUDA EP registered successfully).
    /// AtomicBool allows post-construction downgrade to CPU after warm-up probe.
    gpu_mode: AtomicBool,
    /// Override max sessions (0 = use default)
    override_max_sessions: usize,
    /// Override threads per session (0 = use default)
    override_threads: usize,
}

impl SessionPool {
    /// Create pool with a single session (fast startup for search).
    /// Additional sessions are added lazily via `ensure_sessions()`.
    /// Tries CUDA execution provider first if GPU is available, falls back to CPU.
    fn new(
        config: &EmbeddingConfig,
        override_max_sessions: usize,
        override_threads: usize,
    ) -> Result<Self> {
        use ort::session::builder::GraphOptimizationLevel;

        let threads = if override_threads > 0 {
            override_threads
        } else {
            THREADS_PER_SESSION
        };
        let want_gpu = gpu_available();
        let mut using_gpu = false;

        let session = if want_gpu {
            // Try GPU: register CUDA EP with silent fallback to CPU.
            // We detect actual GPU usage by checking if CUDA registered successfully.
            match Session::builder()
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                .and_then(|b| b.with_execution_providers([ort::ep::CUDA::default().build()]))
                .and_then(|b| b.commit_from_file(&config.model_path))
            {
                Ok(session) => {
                    using_gpu = true;
                    eprintln!(
                        "  CUDA execution provider registered — using GPU ({})",
                        config
                            .model_path
                            .file_name()
                            .map(|f| f.to_string_lossy())
                            .unwrap_or_default()
                    );
                    session
                }
                Err(e) => {
                    eprintln!("  CUDA EP failed ({e}), falling back to CPU");
                    Session::builder()
                        .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                        .and_then(|b| b.with_intra_threads(threads))
                        .and_then(|b| b.commit_from_file(&config.model_path))
                        .with_context(|| {
                            format!("Failed to load ONNX model from {:?}", config.model_path)
                        })?
                }
            }
        } else {
            Session::builder()
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                .and_then(|b| b.with_intra_threads(threads))
                .and_then(|b| b.commit_from_file(&config.model_path))
                .with_context(|| {
                    format!("Failed to load ONNX model from {:?}", config.model_path)
                })?
        };

        // Detect whether model expects token_type_ids (BERT does, XLM-RoBERTa does not)
        let uses_token_type_ids = session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids");

        let mut tokenizer =
            tokenizers::Tokenizer::from_file(&config.tokenizer_path).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to load tokenizer from {:?}: {e}",
                    config.tokenizer_path
                )
            })?;

        // BGE-M3 (XLM-RoBERTa): pad_id=1, pad_token="<pad>"
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            pad_id: 1,
            pad_token: "<pad>".to_string(),
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: config.max_length,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("Failed to set truncation: {e}"))?;

        let max_sess = if override_max_sessions > 0 {
            override_max_sessions
        } else {
            max_sessions_for(&config.model_path)
        };
        let mut initial_sessions = vec![Arc::new(Mutex::new(session))];
        if !using_gpu && max_sess > 1 {
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            let sess_threads = if override_threads > 0 {
                override_threads
            } else {
                threads_per_session_cpu(cores, max_sess)
            };
            let to_add = max_sess - 1;
            let model_path = &config.model_path;
            let new_sessions: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..to_add)
                    .map(|i| {
                        s.spawn(move || {
                            Session::builder()
                                .and_then(|b| {
                                    b.with_optimization_level(GraphOptimizationLevel::Level3)
                                })
                                .and_then(|b| b.with_intra_threads(sess_threads))
                                .and_then(|b| b.commit_from_file(model_path))
                                .map_err(|e| (i + 1, e))
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| match h.join() {
                        Ok(Ok(sess)) => Some(sess),
                        Ok(Err((idx, err))) => {
                            tracing::warn!("Failed to pre-warm ONNX session {idx}: {err:?}");
                            None
                        }
                        Err(panic_err) => {
                            tracing::warn!("Panic pre-warming ONNX session: {panic_err:?}");
                            None
                        }
                    })
                    .collect()
            });
            for extra_sess in new_sessions {
                initial_sessions.push(Arc::new(Mutex::new(extra_sess)));
            }
        }

        let pool = Self {
            sessions: parking_lot::RwLock::new(initial_sessions),
            next_session_idx: std::sync::atomic::AtomicUsize::new(0),
            model_path: config.model_path.clone(),
            tokenizer,
            uses_token_type_ids,
            gpu_mode: AtomicBool::new(using_gpu),
            override_max_sessions,
            override_threads,
        };

        // Verify GPU is actually accelerating, not just silently running on CPU.
        // This happens before the pool is returned, so no concurrency concerns.
        if using_gpu && !pool.verify_gpu_execution() {
            pool.downgrade_to_cpu()?;
        }

        Ok(pool)
    }

    /// Maximum wall-clock time (ms) for the GPU warm-up probe.
    /// Real GPU inference on 4 short texts completes in <100ms.
    /// CPU single-session takes 2-10s. 1500ms is a safe threshold.
    const GPU_PROBE_THRESHOLD_MS: u128 = 1500;

    /// Run a small timed inference to verify the GPU is actually executing operators.
    /// ONNX Runtime can "register" CUDA EP but silently run everything on CPU.
    /// Returns true if inference is fast enough to indicate real GPU execution.
    fn verify_gpu_execution(&self) -> bool {
        let probe_texts = [
            "The quick brown fox jumps over the lazy dog.",
            "Semantic search uses dense vector representations.",
            "GPU acceleration improves inference throughput.",
            "This is a warm-up probe for execution provider verification.",
        ];

        let batch_size = probe_texts.len();

        let encodings = match self.tokenizer.encode_batch(probe_texts.to_vec(), true) {
            Ok(enc) => enc,
            Err(e) => {
                tracing::warn!("GPU probe: tokenization failed ({e}), assuming no GPU");
                return false;
            }
        };

        let max_length = encodings
            .iter()
            .map(|enc| enc.get_ids().len().min(64))
            .max()
            .unwrap_or(64)
            .max(1);

        let total_len = batch_size * max_length;
        let mut input_ids_data = vec![0i64; total_len];
        let mut attention_data = vec![0i64; total_len];

        for (batch_idx, encoding) in encodings.iter().enumerate() {
            let offset = batch_idx * max_length;
            for (i, &token) in encoding.get_ids().iter().take(max_length).enumerate() {
                input_ids_data[offset + i] = token as i64;
            }
            for (i, &mask) in encoding
                .get_attention_mask()
                .iter()
                .take(max_length)
                .enumerate()
            {
                attention_data[offset + i] = mask as i64;
            }
        }

        let shape = [batch_size, max_length];

        use ort::value::TensorRef;
        let input_ids_val = match TensorRef::from_array_view((shape, input_ids_data.as_slice())) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("GPU probe: tensor creation failed ({e}), assuming no GPU");
                return false;
            }
        };
        let attention_val = match TensorRef::from_array_view((shape, attention_data.as_slice())) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("GPU probe: tensor creation failed ({e}), assuming no GPU");
                return false;
            }
        };

        let session_arc = self.acquire_session();
        let mut session = {
            let tracer = opentelemetry::global::tracer("mem");
            use opentelemetry::trace::Tracer;
            let mut span = tracer.start("session_mutex_wait");
            let s = session_arc.lock();
            use opentelemetry::trace::Span;
            span.end();
            s
        };

        let start = std::time::Instant::now();
        let result = if self.uses_token_type_ids {
            let token_type_data = vec![0i64; total_len];
            let token_types_val =
                match TensorRef::from_array_view((shape, token_type_data.as_slice())) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("GPU probe: tensor creation failed ({e}), assuming no GPU");
                        return false;
                    }
                };
            session.run(ort::inputs![input_ids_val, attention_val, token_types_val])
        } else {
            session.run(ort::inputs![input_ids_val, attention_val])
        };
        let elapsed_ms = start.elapsed().as_millis();

        if result.is_err() {
            tracing::warn!(
                "GPU probe: inference failed ({:?}), assuming no GPU",
                result.err()
            );
            return false;
        }

        let is_fast = elapsed_ms < Self::GPU_PROBE_THRESHOLD_MS;
        if is_fast {
            tracing::info!("GPU probe: {elapsed_ms}ms — GPU is working");
        } else {
            tracing::warn!(
                "GPU probe: {elapsed_ms}ms (threshold {threshold}ms) — CUDA registered but not accelerating",
                threshold = Self::GPU_PROBE_THRESHOLD_MS
            );
        }
        is_fast
    }

    /// Replace the CUDA session with a CPU session and flip gpu_mode to false.
    /// Called when the warm-up probe detects that CUDA isn't actually accelerating.
    fn downgrade_to_cpu(&self) -> Result<()> {
        use ort::session::builder::GraphOptimizationLevel;

        tracing::warn!("Downgrading from CUDA to CPU parallel mode");
        eprintln!("  GPU not accelerating — switching to CPU parallel mode");

        let max_sess = if self.override_max_sessions > 0 {
            self.override_max_sessions
        } else {
            max_sessions_for(&self.model_path)
        };
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let sess_threads = if self.override_threads > 0 {
            self.override_threads
        } else {
            threads_per_session_cpu(cores, max_sess)
        };

        let cpu_session = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(sess_threads))
            .and_then(|b| b.commit_from_file(&self.model_path))
            .with_context(|| {
                format!(
                    "Failed to create CPU fallback session from {:?}",
                    self.model_path
                )
            })?;

        let mut initial_sessions = vec![Arc::new(Mutex::new(cpu_session))];
        if max_sess > 1 {
            let to_add = max_sess - 1;
            let model_path = &self.model_path;
            let new_sessions: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..to_add)
                    .map(|i| {
                        s.spawn(move || {
                            Session::builder()
                                .and_then(|b| {
                                    b.with_optimization_level(GraphOptimizationLevel::Level3)
                                })
                                .and_then(|b| b.with_intra_threads(sess_threads))
                                .and_then(|b| b.commit_from_file(model_path))
                                .map_err(|e| (i + 1, e))
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| match h.join() {
                        Ok(Ok(sess)) => Some(sess),
                        Ok(Err((idx, err))) => {
                            tracing::warn!("Failed to pre-warm ONNX session {idx}: {err:?}");
                            None
                        }
                        Err(panic_err) => {
                            tracing::warn!("Panic pre-warming ONNX session: {panic_err:?}");
                            None
                        }
                    })
                    .collect()
            });
            for extra_sess in new_sessions {
                initial_sessions.push(Arc::new(Mutex::new(extra_sess)));
            }
        }

        let mut sessions = self.sessions.write();
        *sessions = initial_sessions;

        self.gpu_mode.store(false, Ordering::Relaxed);
        tracing::info!("Now using CPU parallel mode (up to {} sessions)", max_sess);
        Ok(())
    }

    /// Grow pool to `count` sessions (no-op if already large enough or in GPU mode).
    /// Called before parallel batch encoding.
    fn ensure_sessions(&self, count: usize) -> Result<()> {
        // GPU mode: single session, GPU parallelizes internally
        if self.gpu_mode.load(Ordering::Relaxed) {
            return Ok(());
        }

        use ort::session::builder::GraphOptimizationLevel;

        let max_sess = if self.override_max_sessions > 0 {
            self.override_max_sessions
        } else {
            max_sessions_for(&self.model_path)
        };
        let threads = if self.override_threads > 0 {
            self.override_threads
        } else {
            THREADS_PER_SESSION
        };
        let current = self.sessions.read().len();
        let needed = count.min(max_sess);
        if current >= needed {
            return Ok(());
        }

        let to_add = needed - current;
        eprintln!("  Scaling to {needed} ONNX sessions ({threads} threads each)...");

        let model_path = &self.model_path;
        let new_sessions: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..to_add)
                .map(|_| {
                    s.spawn(|| {
                        Session::builder()
                            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                            .and_then(|b| b.with_intra_threads(threads))
                            .and_then(|b| b.commit_from_file(model_path))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut sessions = self.sessions.write();
        for s in new_sessions {
            sessions.push(Arc::new(Mutex::new(s.with_context(|| {
                format!("Failed to load ONNX model from {:?}", self.model_path)
            })?)));
        }

        Ok(())
    }

    /// Get the first available (unlocked) session, or distribute across sessions via round-robin.
    /// Never performs synchronous disk I/O or takes exclusive write locks on the query path.
    fn acquire_session(&self) -> Arc<Mutex<Session>> {
        let sessions = self.sessions.read();
        // Try each session without blocking
        for session in sessions.iter() {
            if session.try_lock().is_some() {
                return Arc::clone(session);
            }
        }
        // All busy — distribute across existing sessions via round-robin so callers don't all block on session 0
        let idx = self.next_session_idx.fetch_add(1, Ordering::Relaxed) % sessions.len();
        Arc::clone(&sessions[idx])
    }
}

// =============================================================================
// EMBEDDER
// =============================================================================

pub struct Embedder {
    config: EmbeddingConfig,
    pool: OnceLock<std::result::Result<Arc<SessionPool>, String>>,
    /// Override max ONNX sessions (0 = use default from CPU core count)
    override_max_sessions: usize,
    /// Override threads per ONNX session (0 = use default THREADS_PER_SESSION)
    override_threads: usize,
    /// Override chunks per sub-batch (0 = use default MAX_BATCH_CPU/GPU)
    override_batch_size: usize,
    /// If true, return dummy zero vectors (used for testing without model files).
    is_dummy: bool,
    /// Query embedding cache (LRU)
    query_cache: parking_lot::Mutex<LruQueryCache>,
    /// Dummy sessions for testing concurrency without model files
    dummy_sessions: parking_lot::RwLock<Vec<Arc<Mutex<()>>>>,
    /// Artificial delay for dummy sessions in tests
    dummy_delay: parking_lot::Mutex<Option<std::time::Duration>>,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        if std::env::var("AOPS_DUMMY_EMBEDDER")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false)
        {
            return Ok(Self::new_dummy());
        }

        let offline = std::env::var("AOPS_OFFLINE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        // 1. Ensure ONNX Runtime is available
        ensure_ort_available(offline)
            .context("ONNX Runtime is required. Set ORT_DYLIB_PATH or allow auto-download.")?;

        // 2. Ensure model files exist in the resolved model directory (no-op if already cached)
        let variant = ModelVariant::from_env();
        let mut config = EmbeddingConfig::from_env();
        config.ensure_files(variant, offline, DEFAULT_HF_BASE_URL)?;

        Ok(Self::from_config(config))
    }

    /// Construct an embedder for an explicit model directory and variant, auto-downloading
    /// ONNX Runtime and missing model files unless `AOPS_OFFLINE=true`.
    pub fn new_for_variant(dir: &std::path::Path, variant: ModelVariant) -> Result<Self> {
        let offline = std::env::var("AOPS_OFFLINE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        ensure_ort_available(offline)
            .context("ONNX Runtime is required. Set ORT_DYLIB_PATH or allow auto-download.")?;

        let mut config = EmbeddingConfig::for_dir(dir, variant);
        config.ensure_files(variant, offline, DEFAULT_HF_BASE_URL)?;

        Ok(Self::from_config(config))
    }

    /// Log the configuration and effective parallelism (reflecting any overrides).
    pub fn log_info(&self) {
        if self.is_dummy {
            return;
        }
        let model_file = self.model_file_name();
        let (sessions, threads, batch_size) = self.effective_config();
        if gpu_available() {
            tracing::info!(
                "Using BGE-M3 ONNX embeddings ({model_file}, {EMBEDDING_DIM}-dim, GPU/CUDA mode, batch size {batch_size})"
            );
        } else {
            tracing::info!(
                "Using BGE-M3 ONNX embeddings ({model_file}, {EMBEDDING_DIM}-dim, up to {sessions} sessions × {threads} threads, batch size {batch_size})"
            );
        }
    }

    /// Construct an embedder from an already-resolved `EmbeddingConfig` (sessions load lazily).
    pub fn from_config(config: EmbeddingConfig) -> Self {
        Self {
            config,
            pool: OnceLock::new(),
            override_max_sessions: 0,
            override_threads: 0,
            override_batch_size: 0,
            is_dummy: false,
            query_cache: parking_lot::Mutex::new(LruQueryCache::new(512)),
            dummy_sessions: parking_lot::RwLock::new(
                (0..max_sessions_cpu().max(5))
                    .map(|_| Arc::new(Mutex::new(())))
                    .collect(),
            ),
            dummy_delay: parking_lot::Mutex::new(None),
        }
    }

    /// Filename of the configured ONNX model weights, or `"dummy"` for dummy embedders.
    pub fn model_file_name(&self) -> &str {
        if self.is_dummy {
            return "dummy";
        }
        self.config
            .model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }

    /// Create a dummy embedder that returns zero vectors (for testing).
    pub fn new_dummy() -> Self {
        Self {
            config: EmbeddingConfig {
                model_path: PathBuf::new(),
                tokenizer_path: PathBuf::new(),
                max_length: 512,
                query_dense_token_cap: DEFAULT_QUERY_DENSE_TOKEN_CAP,
            },
            pool: OnceLock::new(),
            override_max_sessions: 0,
            override_threads: 0,
            override_batch_size: 0,
            is_dummy: true,
            query_cache: parking_lot::Mutex::new(LruQueryCache::new(512)),
            dummy_sessions: parking_lot::RwLock::new(
                (0..max_sessions_cpu().max(5))
                    .map(|_| Arc::new(Mutex::new(())))
                    .collect(),
            ),
            dummy_delay: parking_lot::Mutex::new(None),
        }
    }

    pub fn query_cache_len(&self) -> usize {
        self.query_cache.lock().len()
    }

    pub fn is_dummy(&self) -> bool {
        self.is_dummy
    }

    pub fn set_dummy_delay(&mut self, delay: std::time::Duration) {
        *self.dummy_delay.lock() = Some(delay);
    }

    /// Set parallelism overrides for benchmarking. Zero values use defaults.
    pub fn with_overrides(mut self, sessions: usize, threads: usize, batch_size: usize) -> Self {
        self.override_max_sessions = sessions;
        self.override_threads = threads;
        self.override_batch_size = batch_size;
        if self.is_dummy && sessions > 0 {
            *self.dummy_sessions.write() =
                (0..sessions).map(|_| Arc::new(Mutex::new(()))).collect();
        }
        self
    }

    /// Return the effective config for display (sessions, threads, batch_size).
    pub fn effective_config(&self) -> (usize, usize, usize) {
        let sessions = if self.override_max_sessions > 0 {
            self.override_max_sessions
        } else {
            max_sessions_for(&self.config.model_path)
        };
        let threads = if self.override_threads > 0 {
            self.override_threads
        } else {
            THREADS_PER_SESSION
        };
        let batch_size = if self.override_batch_size > 0 {
            self.override_batch_size
        } else if gpu_available() {
            Self::MAX_BATCH_GPU
        } else {
            Self::MAX_BATCH_CPU
        };
        (sessions, threads, batch_size)
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    /// Encode a query with instruction prefix (asymmetric retrieval).
    pub fn encode_query(&self, text: &str) -> Result<Vec<f32>> {
        let tracer = opentelemetry::global::tracer("mem");
        use opentelemetry::trace::Tracer;
        let span = tracer.start("encode_query");
        let _guard = opentelemetry::trace::mark_span_as_active(span);
        let start = std::time::Instant::now();

        if let Some(cached) = self.query_cache.lock().get(text) {
            crate::otel::call_metrics::CallMetrics::record_embed_duration(start.elapsed());
            return Ok(cached);
        }

        let prefixed = format!("{QUERY_PREFIX}{text}");
        let results =
            self.encode_batch_with_cap(&[&prefixed], self.config.query_dense_token_cap)?;
        let res = results
            .into_iter()
            .next()
            .unwrap_or_else(|| vec![0.0; EMBEDDING_DIM]);

        self.query_cache
            .lock()
            .insert(text.to_string(), res.clone());
        crate::otel::call_metrics::CallMetrics::record_embed_duration(start.elapsed());
        Ok(res)
    }

    /// Encode a document without prefix (asymmetric retrieval).
    pub fn encode_document(&self, text: &str) -> Result<Vec<f32>> {
        self.encode(text)
    }

    fn ensure_pool(&self) -> Result<&Arc<SessionPool>> {
        let result = self.pool.get_or_init(|| {
            SessionPool::new(
                &self.config,
                self.override_max_sessions,
                self.override_threads,
            )
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
        Ok(results
            .into_iter()
            .next()
            .unwrap_or_else(|| vec![0.0; EMBEDDING_DIM]))
    }

    /// Chunks per ONNX sub-batch (CPU). Smaller = more sub-batches = more sessions in parallel.
    const MAX_BATCH_CPU: usize = 32;
    /// Chunks per ONNX sub-batch (GPU). Measured on an RTX 3080 (10 GB, WSL2) with
    /// the fp32 model: 32 → ~59 chunks/s, 64 → ~54, 256 → ~19. Large batches of
    /// ~512-token chunks blow up attention memory, so small batches win.
    const MAX_BATCH_GPU: usize = 32;

    pub fn encode_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.encode_batch_with_cap(texts, self.config.max_length)
    }

    pub fn encode_batch_with_cap(&self, texts: &[&str], token_cap: usize) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        if self.is_dummy {
            let tracer = opentelemetry::global::tracer("mem");
            use opentelemetry::trace::Tracer;
            let mut span = tracer.start("session_mutex_wait");
            let dummy_pool = self.dummy_sessions.read().clone();
            let mut _guard = None;
            for s in &dummy_pool {
                if let Some(g) = s.try_lock() {
                    _guard = Some(g);
                    break;
                }
            }
            let _guard = _guard.unwrap_or_else(|| {
                static DUMMY_IDX: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let idx = DUMMY_IDX.fetch_add(1, Ordering::Relaxed) % dummy_pool.len();
                dummy_pool[idx].lock()
            });
            use opentelemetry::trace::Span;
            span.end();

            let delay = *self.dummy_delay.lock();
            if let Some(d) = delay {
                std::thread::sleep(d);
            }

            return Ok(vec![vec![0.0; EMBEDDING_DIM]; texts.len()]);
        }

        let pool = self.ensure_pool()?;
        let max_batch = if self.override_batch_size > 0 {
            self.override_batch_size
        } else if pool.gpu_mode.load(Ordering::Relaxed) {
            Self::MAX_BATCH_GPU
        } else {
            Self::MAX_BATCH_CPU
        };

        // GPU mode: single session, sequential large batches (GPU parallelizes internally)
        if pool.gpu_mode.load(Ordering::Relaxed) {
            if texts.len() <= max_batch {
                return self.encode_single_batch(texts, pool, token_cap);
            }
            let mut all = Vec::with_capacity(texts.len());
            for chunk in texts.chunks(max_batch) {
                all.extend(self.encode_single_batch(chunk, pool, token_cap)?);
            }
            return Ok(all);
        }

        // CPU mode: scale up sessions and split in parallel via rayon
        if texts.len() > max_batch {
            use rayon::prelude::*;
            use std::sync::atomic::{AtomicUsize, Ordering};

            let sub_batches: Vec<&[&str]> = texts.chunks(max_batch).collect();
            let total_batches = sub_batches.len();
            let total_texts = texts.len();
            pool.ensure_sessions(sub_batches.len())?;

            let done = AtomicUsize::new(0);
            let results: Vec<Result<Vec<Vec<f32>>>> = sub_batches
                .par_iter()
                .map(|batch| {
                    let r = self.encode_single_batch(batch, pool, token_cap);
                    let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if completed.is_multiple_of(10) || completed == total_batches {
                        let pct = (completed * 100) / total_batches;
                        let chunks_done = (completed * max_batch).min(total_texts);
                        tracing::debug!("Embedded {chunks_done}/{total_texts} chunks ({pct}%)");
                    }
                    r
                })
                .collect();
            let mut all = Vec::with_capacity(texts.len());
            for r in results {
                all.extend(r?);
            }
            return Ok(all);
        }

        self.encode_single_batch(texts, pool, token_cap)
    }

    fn encode_single_batch(
        &self,
        texts: &[&str],
        pool: &Arc<SessionPool>,
        token_cap: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let configured_max = self.config.max_length.min(token_cap);
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
            for (i, &mask) in encoding
                .get_attention_mask()
                .iter()
                .take(max_length)
                .enumerate()
            {
                attention_data[offset + i] = mask as i64;
            }
        }

        let shape = [batch_size, max_length];

        use ort::value::TensorRef;
        let input_ids_val = TensorRef::from_array_view((shape, input_ids_data.as_slice()))?;
        let attention_val = TensorRef::from_array_view((shape, attention_data.as_slice()))?;

        // Acquire a session from the pool and run inference
        let session_arc = pool.acquire_session();
        let mut session = {
            let tracer = opentelemetry::global::tracer("mem");
            use opentelemetry::trace::Tracer;
            let mut span = tracer.start("session_mutex_wait");
            let s = session_arc.lock();
            use opentelemetry::trace::Span;
            span.end();
            s
        };

        // Conditionally include token_type_ids (BERT needs them, XLM-RoBERTa does not)
        let outputs = if pool.uses_token_type_ids {
            let token_type_data = vec![0i64; total_len];
            let token_types_val = TensorRef::from_array_view((shape, token_type_data.as_slice()))?;
            session.run(ort::inputs![input_ids_val, attention_val, token_types_val])?
        } else {
            session.run(ort::inputs![input_ids_val, attention_val])?
        };

        extract_session_embeddings(&outputs, batch_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputKind {
    /// Pre-pooled 2D output `[batch, EMBEDDING_DIM]` (`sentence_embedding`).
    SentenceEmbedding,
    /// Unpooled 3D token states `[batch, seq, EMBEDDING_DIM]` (`last_hidden_state` or `token_embeddings`).
    TokenEmbeddings,
}

enum RawTensorSlice<'a> {
    F32(&'a [f32]),
    F16(&'a [half::f16]),
}

impl RawTensorSlice<'_> {
    fn len(&self) -> usize {
        match self {
            RawTensorSlice::F32(s) => s.len(),
            RawTensorSlice::F16(s) => s.len(),
        }
    }

    fn copy_vec_f32(&self, start: usize, len: usize) -> Vec<f32> {
        match self {
            RawTensorSlice::F32(s) => s[start..start + len].to_vec(),
            RawTensorSlice::F16(s) => s[start..start + len].iter().map(|v| v.to_f32()).collect(),
        }
    }
}

fn select_output_key<'a>(keys: &[&'a str]) -> Result<(OutputKind, &'a str)> {
    if let Some(&k) = keys.iter().find(|&&k| k == "sentence_embedding") {
        Ok((OutputKind::SentenceEmbedding, k))
    } else if let Some(&k) = keys.iter().find(|&&k| k == "last_hidden_state") {
        Ok((OutputKind::TokenEmbeddings, k))
    } else if let Some(&k) = keys.iter().find(|&&k| k == "token_embeddings") {
        Ok((OutputKind::TokenEmbeddings, k))
    } else {
        bail!(
            "ONNX model outputs missing expected embedding tensor \
             (looked for 'sentence_embedding', 'last_hidden_state', or 'token_embeddings'; \
             found {keys:?})"
        )
    }
}

fn clean_and_l2_normalize(mut vec: Vec<f32>) -> Vec<f32> {
    for val in vec.iter_mut() {
        if val.is_nan() || val.is_infinite() {
            *val = 0.0;
        }
    }
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON && !norm.is_nan() {
        for val in &mut vec {
            *val /= norm;
        }
    }
    vec
}

fn extract_embeddings_from_raw(
    kind: OutputKind,
    output_name: &str,
    shape: &[i64],
    data: RawTensorSlice<'_>,
    expected_batch: usize,
) -> Result<Vec<Vec<f32>>> {
    match kind {
        OutputKind::SentenceEmbedding => {
            if shape.len() != 2 {
                bail!(
                    "Expected 2D [batch, {EMBEDDING_DIM}] tensor for '{output_name}', got shape {shape:?}"
                );
            }
            let batch = usize::try_from(shape[0])
                .ok()
                .filter(|&b| b == expected_batch)
                .with_context(|| {
                    format!(
                        "Batch size mismatch for '{output_name}': expected {expected_batch}, got shape {shape:?}"
                    )
                })?;
            let dim = usize::try_from(shape[1]).unwrap_or(0);
            if dim != EMBEDDING_DIM {
                bail!(
                    "Embedding dimension mismatch for '{output_name}': expected {EMBEDDING_DIM}, got {dim} (shape {shape:?})"
                );
            }
            let expected_len = batch
                .checked_mul(EMBEDDING_DIM)
                .context("Tensor buffer size overflow")?;
            if data.len() != expected_len {
                bail!(
                    "Tensor buffer length mismatch for '{output_name}': expected {expected_len}, got {}",
                    data.len()
                );
            }

            let mut results = Vec::with_capacity(batch);
            for batch_idx in 0..batch {
                let offset = batch_idx * EMBEDDING_DIM;
                let pooled = data.copy_vec_f32(offset, EMBEDDING_DIM);
                results.push(clean_and_l2_normalize(pooled));
            }
            Ok(results)
        }
        OutputKind::TokenEmbeddings => {
            if shape.len() != 3 {
                bail!(
                    "Expected 3D [batch, seq, {EMBEDDING_DIM}] tensor for '{output_name}', got shape {shape:?}"
                );
            }
            let batch = usize::try_from(shape[0])
                .ok()
                .filter(|&b| b == expected_batch)
                .with_context(|| {
                    format!(
                        "Batch size mismatch for '{output_name}': expected {expected_batch}, got shape {shape:?}"
                    )
                })?;
            let seq_len = usize::try_from(shape[1]).unwrap_or(0);
            if seq_len == 0 {
                bail!(
                    "Token embeddings sequence length must be >= 1 for '{output_name}', got shape {shape:?}"
                );
            }
            let dim = usize::try_from(shape[2]).unwrap_or(0);
            if dim != EMBEDDING_DIM {
                bail!(
                    "Embedding dimension mismatch for '{output_name}': expected {EMBEDDING_DIM}, got {dim} (shape {shape:?})"
                );
            }
            let expected_len = batch
                .checked_mul(seq_len)
                .and_then(|n| n.checked_mul(EMBEDDING_DIM))
                .context("Tensor buffer size overflow")?;
            if data.len() != expected_len {
                bail!(
                    "Tensor buffer length mismatch for '{output_name}': expected {expected_len}, got {}",
                    data.len()
                );
            }

            // BGE-M3 pooling_mode_cls_token = true: slice token index 0 ([CLS]) per batch item.
            let mut results = Vec::with_capacity(batch);
            for batch_idx in 0..batch {
                let offset = batch_idx * seq_len * EMBEDDING_DIM;
                let cls_vec = data.copy_vec_f32(offset, EMBEDDING_DIM);
                results.push(clean_and_l2_normalize(cls_vec));
            }
            Ok(results)
        }
    }
}

fn extract_session_embeddings(
    outputs: &ort::session::SessionOutputs<'_>,
    expected_batch: usize,
) -> Result<Vec<Vec<f32>>> {
    let keys: Vec<&str> = outputs.keys().collect();
    let (kind, output_name) = select_output_key(&keys)?;
    let value = outputs
        .get(output_name)
        .with_context(|| format!("Missing ONNX output '{output_name}'"))?;

    let elem_type = match value.dtype() {
        ort::value::ValueType::Tensor { ty, .. } => *ty,
        other => bail!("ONNX output '{output_name}' is not a tensor: {other:?}"),
    };

    match elem_type {
        ort::tensor::TensorElementType::Float32 => {
            let (shape, slice) = value
                .try_extract_tensor::<f32>()
                .with_context(|| format!("Failed to extract f32 tensor from '{output_name}'"))?;
            extract_embeddings_from_raw(
                kind,
                output_name,
                shape,
                RawTensorSlice::F32(slice),
                expected_batch,
            )
        }
        ort::tensor::TensorElementType::Float16 => {
            let (shape, slice) = value
                .try_extract_tensor::<half::f16>()
                .with_context(|| format!("Failed to extract f16 tensor from '{output_name}'"))?;
            extract_embeddings_from_raw(
                kind,
                output_name,
                shape,
                RawTensorSlice::F16(slice),
                expected_batch,
            )
        }
        other => bail!(
            "Unsupported tensor element type '{other}' for ONNX output '{output_name}' \
             (expected f32 or f16)"
        ),
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
    // On WSL2, ensure the paravirtual GPU driver wins over any native libcuda before
    // ORT loads its CUDA provider. Harmless (no-op) when not on WSL or when GPU is off.
    if gpu_available() {
        preload_wsl_libcuda();
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(current_path) = std::env::var("PATH") {
            let cache = get_cache_dir().join("onnxruntime");
            let cache_str = cache.to_string_lossy().to_string();
            if !current_path.contains(&cache_str) {
                let new_path = format!("{};{}", cache_str, current_path);
                std::env::set_var("PATH", new_path);
            }
        }
    }

    // Check existing env var
    if let Ok(existing) = std::env::var("ORT_DYLIB_PATH") {
        let path = PathBuf::from(&existing);
        if path.exists() {
            return Ok(path);
        }
    }

    // Check cache — but only accept a cached lib if it matches the execution-provider
    // variant we want for this machine. A stale CPU-only cache must NOT shadow a GPU
    // build when CUDA is available, or ORT silently registers the CUDA EP and then runs
    // every operator on CPU (no GPU memory allocated, no speed-up). The CUDA provider
    // shared libraries are co-located with the main lib only in the GPU build.
    if let Some(cached) = get_onnx_runtime_path() {
        if onnx_cache_satisfies_variant() {
            std::env::set_var("ORT_DYLIB_PATH", &cached);
            return Ok(cached);
        }
        if offline {
            // Can't re-download; use the stale (CPU) build rather than failing outright.
            tracing::warn!(
                "Cached ONNX Runtime is CPU-only but CUDA is available; \
                 AOPS_OFFLINE=true prevents fetching the GPU build — running on CPU."
            );
            std::env::set_var("ORT_DYLIB_PATH", &cached);
            return Ok(cached);
        }
        // Wanted the GPU build but cache is CPU-only — fall through to (re)download.
        eprintln!("  Cached ONNX Runtime is CPU-only; fetching GPU/CUDA build...");
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

/// Chunking strategy for embedding text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChunkStrategy {
    /// Fixed-size character windows with overlap (legacy).
    FixedSize,
    /// Markdown-aware semantic chunking at paragraph/list/code boundaries.
    Semantic,
}

pub struct ChunkConfig {
    pub strategy: ChunkStrategy,
    pub chunk_size: usize,
    pub overlap: usize,
    pub min_chunk_size: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            strategy: ChunkStrategy::Semantic,
            chunk_size: 1500,
            overlap: 0,
            min_chunk_size: 100,
        }
    }
}

impl ChunkConfig {
    /// Legacy fixed-size config for backward compatibility.
    pub fn fixed_size() -> Self {
        Self {
            strategy: ChunkStrategy::FixedSize,
            chunk_size: 2000,
            overlap: 500,
            min_chunk_size: 300,
        }
    }
}

pub fn chunk_text(text: &str, config: &ChunkConfig) -> Vec<String> {
    match config.strategy {
        ChunkStrategy::Semantic => chunk_markdown(text, config),
        ChunkStrategy::FixedSize => chunk_fixed_size(text, config),
    }
}

/// Markdown-aware semantic chunker.
///
/// Splits text at natural markdown boundaries (blank lines), classifies blocks,
/// tracks heading context, merges small blocks, and sub-splits oversized ones.
pub fn chunk_markdown(text: &str, config: &ChunkConfig) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    if text.len() <= config.chunk_size {
        return vec![text.to_string()];
    }

    // Split into raw blocks on blank lines
    let raw_blocks = split_on_blank_lines(text);

    // Track heading context and build semantic chunks
    let mut chunks: Vec<String> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut pending: Option<String> = None; // accumulator for merging small blocks

    for block in &raw_blocks {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        // Detect headings — update context but don't emit as standalone chunks
        if block.starts_with('#') {
            // Flush pending before switching heading context
            if let Some(p) = pending.take() {
                chunks.push(p);
            }
            // Extract heading text (strip # prefix)
            let heading_text = block.trim_start_matches('#').trim();
            current_heading = Some(heading_text.to_string());
            continue;
        }

        // Build chunk with heading context prepended
        let chunk_text = if let Some(ref heading) = current_heading {
            format!("{heading}\n\n{block}")
        } else {
            block.to_string()
        };

        // If oversized, sub-split at sentence boundaries
        if chunk_text.len() > config.chunk_size {
            if let Some(p) = pending.take() {
                chunks.push(p);
            }
            let sub_chunks = split_oversized(&chunk_text, config);
            chunks.extend(sub_chunks);
            continue;
        }

        // Try to merge with pending if both are small
        if let Some(ref mut p) = pending {
            if p.len() + chunk_text.len() + 2 <= config.chunk_size {
                p.push_str("\n\n");
                p.push_str(&chunk_text);
                continue;
            }
            // Pending is big enough — flush it
            chunks.push(p.clone());
        }

        // Start new pending
        if chunk_text.len() < config.min_chunk_size {
            pending = Some(chunk_text);
        } else {
            pending = None;
            chunks.push(chunk_text);
        }
    }

    // Flush any remaining pending
    if let Some(p) = pending {
        if !p.is_empty() {
            // Merge into last chunk if both are small
            if let Some(last) = chunks.last_mut() {
                if last.len() + p.len() + 2 <= config.chunk_size {
                    last.push_str("\n\n");
                    last.push_str(&p);
                } else {
                    chunks.push(p);
                }
            } else {
                chunks.push(p);
            }
        }
    }

    if chunks.is_empty() {
        vec![text.to_string()]
    } else {
        chunks
    }
}

/// Split text into blocks separated by blank lines, preserving code fences
/// and list blocks as single units.
fn split_on_blank_lines(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_code_fence = false;

    for line in text.lines() {
        // Track code fences
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            current.push_str(line);
            current.push('\n');
            continue;
        }

        if in_code_fence {
            current.push_str(line);
            current.push('\n');
            continue;
        }

        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                blocks.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }

    if !current.trim().is_empty() {
        blocks.push(current.trim().to_string());
    }

    blocks
}

/// Sub-split an oversized block at sentence boundaries.
fn split_oversized(text: &str, config: &ChunkConfig) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    // Split into sentences first, then reassemble into chunks
    let sentences = split_sentences(text);

    for sentence in &sentences {
        if current.len() + sentence.len() + 1 > config.chunk_size && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence.trim());
    }

    if !current.trim().is_empty() {
        if current.len() < config.min_chunk_size {
            if let Some(last) = chunks.last_mut() {
                last.push(' ');
                last.push_str(current.trim());
            } else {
                chunks.push(current.trim().to_string());
            }
        } else {
            chunks.push(current.trim().to_string());
        }
    }

    chunks
}

/// Split text into sentences at `. `, `! `, `? ` boundaries.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        current.push(chars[i]);
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && i + 1 < chars.len()
            && chars[i + 1].is_whitespace()
            && current.len() > 10
        {
            sentences.push(current.trim().to_string());
            current.clear();
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }
    sentences
}

/// Find a sentence break position within text, at least `min_pos` chars in.
fn find_sentence_break_in(text: &str, min_pos: usize) -> Option<usize> {
    let mut last_boundary = None;
    for (i, c) in text.char_indices() {
        if (c == '.' || c == '!' || c == '?') && i >= min_pos {
            let after = i + c.len_utf8();
            if after >= text.len() || text[after..].starts_with(char::is_whitespace) {
                last_boundary = Some(after);
            }
        }
    }
    last_boundary
}

/// Legacy fixed-size chunker with overlap.
fn chunk_fixed_size(text: &str, config: &ChunkConfig) -> Vec<String> {
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
            if let Some(break_pos) = find_sentence_break_in(&text[start..end], 100) {
                end = start + break_pos;
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

        let prev_start = start;
        start = end.saturating_sub(config.overlap);
        if start <= prev_start {
            start = end;
        }
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn session_pool_is_capped_by_memory() {
        // The 2026-11-05 nicwin crash loop: 12 cores want 6 sessions, but a 10 GiB
        // container holds only 2 copies of BGE-M3's 2.27 GB weights in half its limit.
        let bge_m3 = 2_267_545_531;
        assert_eq!(sessions_within_memory(6, 10 * GIB, bge_m3), 2);
        // Plenty of memory: the core-count target stands.
        assert_eq!(sessions_within_memory(6, 64 * GIB, bge_m3), 6);
        // Too little memory for even one in half the limit: still one session.
        assert_eq!(sessions_within_memory(6, 2 * GIB, bge_m3), 1);
        // Unknown model size: no memory cap.
        assert_eq!(sessions_within_memory(6, 10 * GIB, 0), 6);
    }

    #[test]
    fn meminfo_total_parses_to_bytes() {
        let meminfo = "MemTotal:       49283072 kB\nMemFree:         2000000 kB\n";
        assert_eq!(parse_meminfo_total(meminfo), Some(49_283_072 * 1024));
        assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn model_weight_bytes_counts_external_data() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.onnx");
        std::fs::write(&model, vec![0u8; 10]).unwrap();
        assert_eq!(model_weight_bytes(&model), Some(10));
        std::fs::write(dir.path().join("model.onnx_data"), vec![0u8; 90]).unwrap();
        assert_eq!(model_weight_bytes(&model), Some(100));
        assert_eq!(model_weight_bytes(&dir.path().join("missing.onnx")), None);
    }

    /// Verify that the embedding model produces discriminative vectors.
    /// A query should be more similar to a topically-related document than to an unrelated one.
    /// This prevents regressions like using the wrong model output (token_embeddings vs
    /// sentence_embedding) or broken mean pooling that collapses all vectors.
    #[test]
    #[ignore] // Requires ONNX model files on disk — run with: cargo test -- --ignored
    fn embedding_quality_regression() {
        let embedder = Embedder::new().expect("failed to create embedder");

        let query = embedder
            .encode_query("keyboard EMR interference")
            .expect("encode_query failed");

        let relevant_doc = embedder
            .encode_document(
                "The crkbd keyboard has EMR interference issues. \
                 Moving the phone and Bluetooth transmitter away from \
                 the keyboard fixed the disconnection problem.",
            )
            .expect("encode relevant doc failed");

        let irrelevant_doc = embedder
            .encode_document(
                "The Oversight Board considered whether the indefinite \
                 suspension of a political figure from Facebook was \
                 consistent with international human rights standards.",
            )
            .expect("encode irrelevant doc failed");

        let sim_relevant = crate::distance::cosine_similarity(&query, &relevant_doc);
        let sim_irrelevant = crate::distance::cosine_similarity(&query, &irrelevant_doc);

        eprintln!("sim(query, relevant)   = {sim_relevant:.4}");
        eprintln!("sim(query, irrelevant) = {sim_irrelevant:.4}");
        eprintln!(
            "delta                  = {:.4}",
            sim_relevant - sim_irrelevant
        );

        // The relevant doc must score meaningfully higher than the irrelevant one.
        // A delta < 0.05 indicates degenerate embeddings (the old mean-pooling bug
        // produced deltas near 0.0 with all scores clustered around 0.71).
        assert!(
            sim_relevant > sim_irrelevant + 0.05,
            "Relevant doc should score at least 0.05 higher than irrelevant doc, \
             got relevant={sim_relevant:.4} irrelevant={sim_irrelevant:.4} delta={:.4}",
            sim_relevant - sim_irrelevant,
        );

        // Sanity: relevant score should be in a reasonable range
        assert!(
            sim_relevant > 0.3,
            "Relevant doc similarity too low: {sim_relevant:.4}"
        );
    }

    // ── Semantic chunking tests ──

    #[test]
    fn chunk_markdown_mixed_content() {
        // Build content large enough to exceed 1500 chars so chunking kicks in
        let intro = (0..20).map(|i| format!("Introduction sentence {i} with enough words to make this paragraph substantial and meaningful.")).collect::<Vec<_>>().join(" ");
        let methods = (0..15)
            .map(|i| format!("- Step {i}: perform the analysis procedure carefully"))
            .collect::<Vec<_>>()
            .join("\n");
        let results = (0..20)
            .map(|i| {
                format!("Result finding {i} shows statistical significance in the data analysis.")
            })
            .collect::<Vec<_>>()
            .join(" ");

        let md = format!("# Introduction\n\n{intro}\n\n## Methods\n\n{methods}\n\n## Results\n\n```python\ndef analyze():\n    return 42\n```\n\n{results}");

        let config = ChunkConfig::default();
        let chunks = chunk_markdown(&md, &config);

        // Should produce multiple chunks, each a semantic unit
        assert!(
            chunks.len() >= 3,
            "Expected at least 3 chunks, got {} (total len={})",
            chunks.len(),
            md.len()
        );

        // First chunks should have heading context
        assert!(
            chunks[0].contains("Introduction"),
            "First chunk should have heading context"
        );

        // Code block should be preserved as a unit
        let code_chunk = chunks.iter().find(|c| c.contains("def analyze")).unwrap();
        assert!(
            code_chunk.contains("return 42"),
            "Code block should stay together"
        );
    }

    #[test]
    fn chunk_markdown_merges_small_blocks() {
        let md = "\
# Section

Short.

Also short.

Yet another short line.";

        let config = ChunkConfig::default();
        let chunks = chunk_markdown(md, &config);

        // Short paragraphs under min_chunk_size should be merged
        assert_eq!(chunks.len(), 1, "Small blocks should merge into one chunk");
        assert!(chunks[0].contains("Short."));
        assert!(chunks[0].contains("Also short."));
    }

    #[test]
    fn chunk_markdown_splits_oversized() {
        // Create a paragraph that exceeds max chunk size
        let long_para = (0..200)
            .map(|i| format!("Sentence number {i} with some extra words to pad it out."))
            .collect::<Vec<_>>()
            .join(" ");
        let md = format!("# Big Section\n\n{long_para}");

        let config = ChunkConfig::default();
        let chunks = chunk_markdown(&md, &config);

        assert!(chunks.len() > 1, "Oversized block should be sub-split");
        for chunk in &chunks {
            // Each chunk should be within bounds (with some tolerance for heading context)
            assert!(
                chunk.len() <= config.chunk_size + 200,
                "Chunk too large: {} chars",
                chunk.len()
            );
        }
    }

    #[test]
    fn chunk_markdown_preserves_code_fences() {
        let md = "\
# Code Example

Some intro text here that gives context.

```rust
fn main() {
    // This is inside a code fence
    println!(\"hello\");

    // Blank lines inside should NOT split the block
    let x = 42;
}
```

After the code.";

        let config = ChunkConfig::default();
        let chunks = chunk_markdown(md, &config);

        let code_chunk = chunks.iter().find(|c| c.contains("println")).unwrap();
        assert!(
            code_chunk.contains("let x = 42"),
            "Code fence should not be split on blank lines"
        );
    }

    #[test]
    fn chunk_markdown_short_text_single_chunk() {
        let md = "Just a short document with no headings.";
        let config = ChunkConfig::default();
        let chunks = chunk_markdown(md, &config);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], md);
    }

    #[test]
    fn chunk_strategy_default_is_semantic() {
        let config = ChunkConfig::default();
        assert_eq!(config.strategy, ChunkStrategy::Semantic);
    }

    #[test]
    fn chunk_text_dispatches_to_fixed_size() {
        let long_text = "a".repeat(3000);
        let config = ChunkConfig::fixed_size();
        let chunks = chunk_text(&long_text, &config);
        assert!(chunks.len() > 1, "Fixed-size should split long text");
    }

    #[test]
    fn test_find_model_ignores_partial_downloads() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let dir_path = temp_dir.path();

        // Create partial download files
        std::fs::write(dir_path.join(".model.onnx.part"), b"partial data").unwrap();
        std::fs::write(dir_path.join("model.onnx.part"), b"partial data").unwrap();
        std::fs::write(dir_path.join(".model_quantized.onnx.part"), b"partial data").unwrap();

        // Incomplete .part files must NOT be detected as valid cached models
        assert_eq!(
            EmbeddingConfig::find_model(dir_path),
            None,
            "find_model must ignore .part files"
        );

        // When complete model is present, find_model should detect it
        std::fs::write(dir_path.join("model.onnx"), b"complete model").unwrap();
        assert_eq!(
            EmbeddingConfig::find_model(dir_path),
            Some("model.onnx".to_string()),
            "find_model should detect complete model.onnx"
        );

        // When quantized model is present, find_model prefers quantized
        std::fs::write(dir_path.join("model_quantized.onnx"), b"quantized model").unwrap();
        assert_eq!(
            EmbeddingConfig::find_model(dir_path),
            Some("model_quantized.onnx".to_string()),
            "find_model should prefer model_quantized.onnx"
        );
    }

    #[test]
    fn test_atomic_download_simulation_interrupted_and_retry() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let models_dir = temp_dir.path();
        let filename = "model.onnx";
        let dest = models_dir.join(filename);
        let tmp = models_dir.join(format!(".{filename}.part"));

        // Step 1: Simulate interrupted download (leaves .part file, dest does not exist)
        std::fs::write(&tmp, b"incomplete payload").unwrap();
        assert!(
            !dest.exists(),
            "Interrupted download must not leave final destination file"
        );
        assert!(tmp.exists(), "Interrupted download leaves .part file");

        // Step 2: Simulate subsequent run logic from download_models()
        // 2a. dest.exists() is false, so download proceeds
        assert!(!dest.exists());

        // 2b. Clean up leftover partial download
        let _ = std::fs::remove_file(&tmp);
        assert!(!tmp.exists(), "Previous .part file must be cleaned up");

        // 2c. Write new payload to temp file
        let full_payload = b"full completed onnx weights and graph data";
        std::fs::write(&tmp, full_payload).unwrap();

        // 2d. Atomically rename to final destination
        std::fs::rename(&tmp, &dest).unwrap();

        // 2e. Assert final destination exists with full payload, tmp is gone
        assert!(
            dest.exists(),
            "Destination file must exist after successful rename"
        );
        assert!(
            !tmp.exists(),
            "Temp .part file must no longer exist after rename"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), full_payload);
    }

    #[test]
    fn find_model_prefers_quantized() {
        let temp_dir = tempfile::tempdir().unwrap();
        let quant_path = temp_dir.path().join("model_quantized.onnx");
        let unquant_path = temp_dir.path().join("model.onnx");

        std::fs::write(&quant_path, b"dummy quantized model").unwrap();
        std::fs::write(&unquant_path, b"dummy unquantized model").unwrap();

        let chosen = EmbeddingConfig::find_model(temp_dir.path());
        assert_eq!(chosen, Some("model_quantized.onnx".to_string()));
    }

    #[test]
    fn find_model_falls_back_to_unquantized() {
        let temp_dir = tempfile::tempdir().unwrap();
        let unquant_path = temp_dir.path().join("model.onnx");

        std::fs::write(&unquant_path, b"dummy unquantized model").unwrap();

        let chosen = EmbeddingConfig::find_model(temp_dir.path());
        assert_eq!(chosen, Some("model.onnx".to_string()));
    }

    #[test]
    fn find_model_variant_fp32_skips_quantized() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("model_quantized.onnx"), b"q").unwrap();
        std::fs::write(temp_dir.path().join("model.onnx"), b"f").unwrap();

        let chosen = EmbeddingConfig::find_model_variant(temp_dir.path(), ModelVariant::Fp32);
        assert_eq!(chosen, Some("model.onnx".to_string()));
    }

    #[test]
    fn find_model_variant_explicit_does_not_fall_back() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("model.onnx"), b"f").unwrap();

        let chosen = EmbeddingConfig::find_model_variant(temp_dir.path(), ModelVariant::Quantized);
        assert_eq!(chosen, None);
    }

    #[test]
    fn fp16_selects_model_fp16_while_auto_ignores_it() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // Empty directory: Fp16 default config targets model_fp16.onnx in dir
        let empty_cfg = EmbeddingConfig::for_dir(dir, ModelVariant::Fp16);
        assert_eq!(empty_cfg.model_path, dir.join("model_fp16.onnx"));
        assert_eq!(empty_cfg.tokenizer_path, dir.join("tokenizer.json"));
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Fp16),
            None,
            "Fp16 must return None when model_fp16.onnx is absent"
        );

        // Directory with only model_fp16.onnx: Fp16 selects it, Auto/Quantized/Fp32 ignore it
        std::fs::write(dir.join("model_fp16.onnx"), b"fp16 weights").unwrap();
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Fp16),
            Some("model_fp16.onnx".to_string()),
            "Fp16 must select model_fp16.onnx when present"
        );
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Auto),
            None,
            "Auto must never select model_fp16.onnx even when it is the only model in cache"
        );
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Quantized),
            None
        );
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Fp32),
            None
        );

        // Add model.onnx: Auto selects model.onnx (still ignoring model_fp16.onnx), Fp16 still selects model_fp16.onnx
        std::fs::write(dir.join("model.onnx"), b"fp32 weights").unwrap();
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Auto),
            Some("model.onnx".to_string()),
            "Auto must select model.onnx over model_fp16.onnx"
        );
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Fp16),
            Some("model_fp16.onnx".to_string())
        );

        // Remove model_fp16.onnx while model.onnx + model_quantized.onnx exist: Fp16 must NOT fall back
        std::fs::remove_file(dir.join("model_fp16.onnx")).unwrap();
        std::fs::write(dir.join("model_quantized.onnx"), b"int8 weights").unwrap();
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Fp16),
            None,
            "Fp16 must not fall back to quantized or fp32"
        );
        assert_eq!(
            EmbeddingConfig::find_model_variant(dir, ModelVariant::Auto),
            Some("model_quantized.onnx".to_string()),
            "Auto must still prefer model_quantized.onnx over model.onnx"
        );
    }

    #[test]
    fn embedder_model_file_name_reports_dummy_and_configured_basename() {
        let dummy = Embedder::new_dummy();
        assert_eq!(dummy.model_file_name(), "dummy");

        let temp_dir = tempfile::tempdir().unwrap();
        let fp16_embedder = Embedder::from_config(EmbeddingConfig::for_dir(
            temp_dir.path(),
            ModelVariant::Fp16,
        ));
        assert_eq!(fp16_embedder.model_file_name(), "model_fp16.onnx");

        let fp32_embedder = Embedder::from_config(EmbeddingConfig::for_dir(
            temp_dir.path(),
            ModelVariant::Fp32,
        ));
        assert_eq!(fp32_embedder.model_file_name(), "model.onnx");
    }

    /// Spawn a local HTTP/1.1 test server handling exactly `responses.len()` sequential requests,
    /// returning `(base_url, join_handle_yielding_request_paths)`.
    fn spawn_mock_hf_server(
        responses: Vec<(&'static [u8], Option<usize>)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("set_nonblocking");
        let addr = listener.local_addr().expect("local_addr");
        let base_url = format!("http://{addr}");

        let handle = std::thread::spawn(move || {
            let mut paths = Vec::with_capacity(responses.len());
            for (body, declared_len_override) in responses {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if std::time::Instant::now() >= deadline {
                                return paths;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(e) => panic!("accept failed: {e}"),
                    }
                };
                stream.set_nonblocking(false).expect("set blocking");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut req_buf = Vec::new();
                let mut chunk = [0u8; 512];
                while !req_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).expect("read request");
                    if n == 0 {
                        break;
                    }
                    req_buf.extend_from_slice(&chunk[..n]);
                }
                let req_str = String::from_utf8_lossy(&req_buf);
                let first_line = req_str.lines().next().unwrap_or("");
                let path = first_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                paths.push(path);

                let content_len = declared_len_override.unwrap_or(body.len());
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(header.as_bytes()).unwrap();
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
            paths
        });

        (base_url, handle)
    }

    fn part_files_in(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect()
    }

    #[test]
    fn download_fp16_and_fp32_fetch_expected_paths_without_clobbering_or_leaking_parts() {
        // 1. Fp16 into custom dir (two requests: Xenova onnx/model_fp16.onnx + root tokenizer.json)
        let fp16_dir = tempfile::tempdir().unwrap();
        let custom_sub = fp16_dir.path().join("custom_models");
        let (base_url, server) = spawn_mock_hf_server(vec![
            (b"xenova-fp16-onnx-bytes", None),
            (b"xenova-tokenizer-bytes", None),
        ]);

        let mut fp16_cfg = EmbeddingConfig::for_dir(&custom_sub, ModelVariant::Fp16);
        fp16_cfg
            .ensure_files(ModelVariant::Fp16, false, &base_url)
            .expect("fp16 download should succeed");
        let fp16_paths = server.join().expect("join mock server");

        assert_eq!(
            fp16_paths,
            vec![
                "/Xenova/bge-m3/resolve/main/onnx/model_fp16.onnx",
                "/Xenova/bge-m3/resolve/main/tokenizer.json",
            ]
        );
        assert_eq!(fp16_cfg.model_path, custom_sub.join("model_fp16.onnx"));
        assert_eq!(fp16_cfg.tokenizer_path, custom_sub.join("tokenizer.json"));
        assert_eq!(
            std::fs::read(custom_sub.join("model_fp16.onnx")).unwrap(),
            b"xenova-fp16-onnx-bytes"
        );
        assert_eq!(
            std::fs::read(custom_sub.join("tokenizer.json")).unwrap(),
            b"xenova-tokenizer-bytes"
        );
        assert!(
            part_files_in(&custom_sub).is_empty(),
            "No .part files may remain after completed download"
        );

        // 2. Shared directory with existing BAAI tokenizer.json + another PID's .part file:
        //    Fp16 must NOT overwrite tokenizer.json and must NOT unlink the other PID's .part file.
        let shared_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            shared_dir.path().join("tokenizer.json"),
            b"existing-baai-tokenizer",
        )
        .unwrap();
        let foreign_part = shared_dir.path().join(".model_fp16.onnx.999999.part");
        std::fs::write(&foreign_part, b"concurrent-inflight").unwrap();

        let (base_url2, server2) = spawn_mock_hf_server(vec![(b"xenova-fp16-shared-bytes", None)]);
        let mut shared_cfg = EmbeddingConfig::for_dir(shared_dir.path(), ModelVariant::Fp16);
        shared_cfg
            .ensure_files(ModelVariant::Fp16, false, &base_url2)
            .expect("shared fp16 download should succeed");
        let shared_paths = server2.join().expect("join mock server 2");

        assert_eq!(
            shared_paths,
            vec!["/Xenova/bge-m3/resolve/main/onnx/model_fp16.onnx"],
            "Existing tokenizer.json must be skipped without HTTP request"
        );
        assert_eq!(
            std::fs::read(shared_dir.path().join("tokenizer.json")).unwrap(),
            b"existing-baai-tokenizer",
            "Existing tokenizer.json must never be overwritten"
        );
        assert_eq!(
            std::fs::read(shared_dir.path().join("model_fp16.onnx")).unwrap(),
            b"xenova-fp16-shared-bytes"
        );
        assert_eq!(
            part_files_in(shared_dir.path()),
            vec![".model_fp16.onnx.999999.part".to_string()],
            "Concurrent PID's .part file must remain untouched while current PID's .part is renamed"
        );

        // 3. Fp32 / Auto downloads the 4 BAAI/bge-m3/resolve/main/onnx/* files and updates Auto model_path
        let auto_dir = tempfile::tempdir().unwrap();
        let (base_url3, server3) = spawn_mock_hf_server(vec![
            (b"baai-graph", None),
            (b"baai-weights", None),
            (b"baai-const7", None),
            (b"baai-tokenizer", None),
        ]);
        let mut auto_cfg = EmbeddingConfig::for_dir(auto_dir.path(), ModelVariant::Auto);
        assert_eq!(
            auto_cfg.model_path,
            auto_dir.path().join("model_quantized.onnx")
        );
        auto_cfg
            .ensure_files(ModelVariant::Auto, false, &base_url3)
            .expect("auto download should succeed");
        let auto_paths = server3.join().expect("join mock server 3");

        assert_eq!(
            auto_paths,
            vec![
                "/BAAI/bge-m3/resolve/main/onnx/model.onnx",
                "/BAAI/bge-m3/resolve/main/onnx/model.onnx_data",
                "/BAAI/bge-m3/resolve/main/onnx/Constant_7_attr__value",
                "/BAAI/bge-m3/resolve/main/onnx/tokenizer.json",
            ]
        );
        assert_eq!(auto_cfg.model_path, auto_dir.path().join("model.onnx"));
        assert!(part_files_in(auto_dir.path()).is_empty());

        // 4. Truncated HTTP body cleans up current PID's .part file and leaves no destination file
        let err_dir = tempfile::tempdir().unwrap();
        let (base_url4, server4) = spawn_mock_hf_server(vec![(b"short", Some(1024))]);
        let mut err_cfg = EmbeddingConfig::for_dir(err_dir.path(), ModelVariant::Fp16);
        let err = err_cfg
            .ensure_files(ModelVariant::Fp16, false, &base_url4)
            .expect_err("truncated stream must fail");
        let _ = server4.join();
        assert!(
            !err_dir.path().join("model_fp16.onnx").exists(),
            "Incomplete download must not create destination file (err: {err:#})"
        );
        assert!(
            part_files_in(err_dir.path()).is_empty(),
            "Failed download must remove its own PID .part file"
        );

        // 5. Offline bail messages are variant-accurate
        let offline_dir = tempfile::tempdir().unwrap();
        let mut off_fp16 = EmbeddingConfig::for_dir(offline_dir.path(), ModelVariant::Fp16);
        let msg_fp16 = off_fp16
            .ensure_files(ModelVariant::Fp16, true, "http://127.0.0.1:1")
            .expect_err("offline fp16 must bail")
            .to_string();
        assert!(
            msg_fp16.contains("Xenova/bge-m3")
                && msg_fp16.contains("onnx/model_fp16.onnx")
                && msg_fp16.contains("tokenizer.json"),
            "Unexpected fp16 offline error: {msg_fp16}"
        );

        let mut off_fp32 = EmbeddingConfig::for_dir(offline_dir.path(), ModelVariant::Fp32);
        let msg_fp32 = off_fp32
            .ensure_files(ModelVariant::Fp32, true, "http://127.0.0.1:1")
            .expect_err("offline fp32 must bail")
            .to_string();
        assert!(
            msg_fp32.contains("BAAI/bge-m3")
                && msg_fp32.contains("model.onnx_data")
                && msg_fp32.contains("Constant_7_attr__value"),
            "Unexpected fp32 offline error: {msg_fp32}"
        );
    }

    #[test]
    fn extract_2d_sentence_embedding_supports_f32_and_f16() {
        let mut f32_buf = vec![0.0f32; 2 * EMBEDDING_DIM];
        // Batch 0: (3, 4, 0, ...) -> norm 5 -> (0.6, 0.8, 0, ...)
        f32_buf[0] = 3.0;
        f32_buf[1] = 4.0;
        // Batch 1: (0, 5, 12, 0, ...) -> norm 13 -> (0, 5/13, 12/13, ...)
        f32_buf[EMBEDDING_DIM + 1] = 5.0;
        f32_buf[EMBEDDING_DIM + 2] = 12.0;

        let shape = [2i64, EMBEDDING_DIM as i64];
        let out_f32 = extract_embeddings_from_raw(
            OutputKind::SentenceEmbedding,
            "sentence_embedding",
            &shape,
            RawTensorSlice::F32(&f32_buf),
            2,
        )
        .expect("2D f32 extraction failed");

        assert_eq!(out_f32.len(), 2);
        assert!((out_f32[0][0] - 0.6).abs() < 1e-6);
        assert!((out_f32[0][1] - 0.8).abs() < 1e-6);
        assert_eq!(out_f32[0][2], 0.0);
        assert!((out_f32[1][1] - 5.0 / 13.0).abs() < 1e-6);
        assert!((out_f32[1][2] - 12.0 / 13.0).abs() < 1e-6);

        let f16_buf: Vec<half::f16> = f32_buf.iter().copied().map(half::f16::from_f32).collect();
        let out_f16 = extract_embeddings_from_raw(
            OutputKind::SentenceEmbedding,
            "sentence_embedding",
            &shape,
            RawTensorSlice::F16(&f16_buf),
            2,
        )
        .expect("2D f16 extraction failed");

        assert_eq!(out_f16.len(), 2);
        assert!((out_f16[0][0] - 0.6).abs() < 1e-3);
        assert!((out_f16[0][1] - 0.8).abs() < 1e-3);
        assert!((out_f16[1][1] - 5.0 / 13.0).abs() < 1e-3);
        assert!((out_f16[1][2] - 12.0 / 13.0).abs() < 1e-3);
    }

    #[test]
    fn extract_3d_cls_pooling_slices_token_zero_at_seq_stride_for_f32_and_f16() {
        let batch = 2usize;
        let seq = 4usize;
        let mut f32_buf = vec![0.0f32; batch * seq * EMBEDDING_DIM];

        // Batch 0, token 0 ([CLS]): (3, 4, 0, ...) -> (0.6, 0.8, 0, ...)
        f32_buf[0] = 3.0;
        f32_buf[1] = 4.0;
        // Batch 0, tokens 1..4: unequal distractor values that would corrupt mean pooling or wrong stride
        for tok in 1..seq {
            let base = tok * EMBEDDING_DIM;
            f32_buf[base] = -250.0 * (tok as f32);
            f32_buf[base + 3] = 512.0 * (tok as f32);
        }

        // Batch 1, token 0 ([CLS]) at stride 1 * seq * EMBEDDING_DIM: (0, 8, 15, 0, ...) -> norm 17
        let b1_cls = seq * EMBEDDING_DIM;
        f32_buf[b1_cls + 1] = 8.0;
        f32_buf[b1_cls + 2] = 15.0;
        // Batch 1, tokens 1..4: different unequal distractor values
        for tok in 1..seq {
            let base = b1_cls + tok * EMBEDDING_DIM;
            f32_buf[base + 5] = 999.0 / (tok as f32);
            f32_buf[base + 6] = -333.0;
        }

        let shape = [batch as i64, seq as i64, EMBEDDING_DIM as i64];
        let out_f32 = extract_embeddings_from_raw(
            OutputKind::TokenEmbeddings,
            "last_hidden_state",
            &shape,
            RawTensorSlice::F32(&f32_buf),
            batch,
        )
        .expect("3D f32 CLS extraction failed");

        assert_eq!(out_f32.len(), 2);
        assert!((out_f32[0][0] - 0.6).abs() < 1e-6);
        assert!((out_f32[0][1] - 0.8).abs() < 1e-6);
        assert_eq!(
            out_f32[0][3], 0.0,
            "Non-CLS tokens must not bleed into batch 0"
        );
        assert!((out_f32[1][1] - 8.0 / 17.0).abs() < 1e-6);
        assert!((out_f32[1][2] - 15.0 / 17.0).abs() < 1e-6);
        assert_eq!(
            out_f32[1][5], 0.0,
            "Non-CLS tokens must not bleed into batch 1"
        );

        let f16_buf: Vec<half::f16> = f32_buf.iter().copied().map(half::f16::from_f32).collect();
        let out_f16 = extract_embeddings_from_raw(
            OutputKind::TokenEmbeddings,
            "last_hidden_state",
            &shape,
            RawTensorSlice::F16(&f16_buf),
            batch,
        )
        .expect("3D f16 CLS extraction failed");

        assert_eq!(out_f16.len(), 2);
        assert!((out_f16[0][0] - 0.6).abs() < 1e-3);
        assert!((out_f16[0][1] - 0.8).abs() < 1e-3);
        assert_eq!(out_f16[0][3], 0.0);
        assert!((out_f16[1][1] - 8.0 / 17.0).abs() < 1e-3);
        assert!((out_f16[1][2] - 15.0 / 17.0).abs() < 1e-3);
        assert_eq!(out_f16[1][5], 0.0);
    }

    #[test]
    fn extract_embeddings_scrubs_nan_and_inf_before_l2_normalization() {
        let mut buf = vec![0.0f32; 2 * EMBEDDING_DIM];
        buf[0] = f32::NAN;
        buf[1] = f32::INFINITY;
        buf[2] = f32::NEG_INFINITY;
        buf[3] = 6.0;
        buf[4] = 8.0;
        // Second item all NaN -> scrubbed to all zeros without producing NaN on division
        for v in &mut buf[EMBEDDING_DIM..] {
            *v = f32::NAN;
        }

        let out = extract_embeddings_from_raw(
            OutputKind::SentenceEmbedding,
            "sentence_embedding",
            &[2, EMBEDDING_DIM as i64],
            RawTensorSlice::F32(&buf),
            2,
        )
        .expect("NaN/Inf cleanup failed");

        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[0][1], 0.0);
        assert_eq!(out[0][2], 0.0);
        assert!((out[0][3] - 0.6).abs() < 1e-6);
        assert!((out[0][4] - 0.8).abs() < 1e-6);
        assert!(out[1].iter().all(|&x| x == 0.0 && !x.is_nan()));
    }

    #[test]
    fn extract_embeddings_selects_output_keys_and_bails_on_invalid_shapes() {
        assert_eq!(
            select_output_key(&["token_embeddings", "sentence_embedding"]).unwrap(),
            (OutputKind::SentenceEmbedding, "sentence_embedding")
        );
        assert_eq!(
            select_output_key(&["last_hidden_state"]).unwrap(),
            (OutputKind::TokenEmbeddings, "last_hidden_state")
        );
        assert_eq!(
            select_output_key(&["token_embeddings"]).unwrap(),
            (OutputKind::TokenEmbeddings, "token_embeddings")
        );
        let missing_err = select_output_key(&["logits"]).unwrap_err().to_string();
        assert!(
            missing_err.contains("sentence_embedding") && missing_err.contains("last_hidden_state"),
            "Expected missing-key error message, got: {missing_err}"
        );

        // seq == 0 must bail
        let seq_zero_err = extract_embeddings_from_raw(
            OutputKind::TokenEmbeddings,
            "last_hidden_state",
            &[1, 0, EMBEDDING_DIM as i64],
            RawTensorSlice::F32(&[]),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(
            seq_zero_err.contains("sequence length"),
            "Expected seq==0 error, got: {seq_zero_err}"
        );

        // Wrong dim must bail (both 2D and 3D)
        let wrong_dim_2d = extract_embeddings_from_raw(
            OutputKind::SentenceEmbedding,
            "sentence_embedding",
            &[1, 768],
            RawTensorSlice::F32(&vec![0.0; 768]),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(
            wrong_dim_2d.contains("dimension"),
            "Expected 2D dim mismatch error, got: {wrong_dim_2d}"
        );

        let wrong_dim_3d = extract_embeddings_from_raw(
            OutputKind::TokenEmbeddings,
            "last_hidden_state",
            &[1, 2, 512],
            RawTensorSlice::F32(&vec![0.0; 1024]),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(
            wrong_dim_3d.contains("dimension"),
            "Expected 3D dim mismatch error, got: {wrong_dim_3d}"
        );

        // Wrong rank must bail (3D passed as SentenceEmbedding; 2D passed as TokenEmbeddings)
        let wrong_rank_se = extract_embeddings_from_raw(
            OutputKind::SentenceEmbedding,
            "sentence_embedding",
            &[1, 2, EMBEDDING_DIM as i64],
            RawTensorSlice::F32(&vec![0.0; 2 * EMBEDDING_DIM]),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(
            wrong_rank_se.contains("2D"),
            "Expected 2D rank error, got: {wrong_rank_se}"
        );

        let wrong_rank_tok = extract_embeddings_from_raw(
            OutputKind::TokenEmbeddings,
            "last_hidden_state",
            &[1, EMBEDDING_DIM as i64],
            RawTensorSlice::F32(&vec![0.0; EMBEDDING_DIM]),
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(
            wrong_rank_tok.contains("3D"),
            "Expected 3D rank error, got: {wrong_rank_tok}"
        );
    }

    #[test]
    fn model_variant_parses() {
        assert_eq!("auto".parse::<ModelVariant>(), Ok(ModelVariant::Auto));
        assert_eq!(
            "Quantized".parse::<ModelVariant>(),
            Ok(ModelVariant::Quantized)
        );
        assert_eq!(
            "fp16".parse::<ModelVariant>().map(|v| v.as_str()),
            Ok("fp16")
        );
        assert_eq!(
            "FP16".parse::<ModelVariant>().map(|v| v.as_str()),
            Ok("fp16")
        );
        assert_eq!("fp32".parse::<ModelVariant>(), Ok(ModelVariant::Fp32));
        assert_eq!(
            "bogus".parse::<ModelVariant>(),
            Err(
                "unknown model variant 'bogus' (expected auto, quantized, fp16 or fp32)"
                    .to_string()
            )
        );
    }

    #[test]
    fn find_model_returns_none_when_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let chosen = EmbeddingConfig::find_model(temp_dir.path());
        assert_eq!(chosen, None);
    }

    #[test]
    fn aops_model_path_override_honoured() {
        let temp_dir = tempfile::tempdir().unwrap();
        let custom_model = temp_dir.path().join("model_quantized.onnx");
        std::fs::write(&custom_model, b"dummy quantized model").unwrap();

        let custom_path_str = temp_dir.path().to_str().unwrap();
        std::env::set_var("AOPS_MODEL_PATH", custom_path_str);

        let config = EmbeddingConfig::from_env();
        std::env::remove_var("AOPS_MODEL_PATH");

        assert_eq!(
            config.model_path,
            temp_dir.path().join("model_quantized.onnx")
        );
        assert_eq!(
            config.tokenizer_path,
            temp_dir.path().join("tokenizer.json")
        );
    }

    #[test]
    fn cache_dir_and_models_dir_structure() {
        let cache_dir = get_cache_dir();
        assert!(
            cache_dir.ends_with("aops"),
            "Expected cache_dir to end with 'aops', got {:?}",
            cache_dir
        );

        let models_dir = get_models_dir();
        assert!(
            models_dir.ends_with(PathBuf::from("aops").join("models").join("bge-m3")),
            "Expected models_dir to end with 'aops/models/bge-m3', got {:?}",
            models_dir
        );
    }

    #[test]
    fn test_short_input_not_padded_to_512() {
        let config = EmbeddingConfig::from_env();
        if !config.model_path.exists() || !config.tokenizer_path.exists() {
            eprintln!("Skipping: model files not present on disk");
            return;
        }
        let embedder = Embedder::new().expect("failed to create embedder");
        let pool = embedder.ensure_pool().expect("ensure_pool failed");

        let short_input = "keyboard EMR interference";
        let encodings = pool
            .tokenizer
            .encode_batch(vec![short_input], true)
            .expect("tokenization failed");

        let token_count = encodings[0].get_ids().len();
        eprintln!("Token count for {:?}: {}", short_input, token_count);

        assert!(
            token_count < 512,
            "Short input was padded to {token_count} tokens; expected dynamic padding < 512 tokens"
        );
        assert!(
            matches!(
                pool.tokenizer.get_padding().map(|p| &p.strategy),
                Some(tokenizers::PaddingStrategy::BatchLongest)
            ),
            "Tokenizer padding strategy should be BatchLongest"
        );
    }

    #[test]
    fn test_embeddings_match_prefix_vectors_within_tolerance() {
        let config = EmbeddingConfig::from_env();
        if !config.model_path.exists() || !config.tokenizer_path.exists() {
            eprintln!("Skipping: model files not present on disk");
            return;
        }

        let embedder = Embedder::new().expect("failed to create embedder");
        let pool = embedder.ensure_pool().expect("ensure_pool failed");

        let text = "keyboard EMR interference";

        // Dynamic padding vector (post-fix behavior)
        let dynamic_vec = embedder.encode_query(text).expect("dynamic encode failed");

        // Compute pre-fix vector using Fixed(512) padding strategy
        let mut fixed_tokenizer =
            tokenizers::Tokenizer::from_file(&config.tokenizer_path).expect("load tokenizer");
        fixed_tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::Fixed(config.max_length),
            pad_id: 1,
            pad_token: "<pad>".to_string(),
            ..Default::default()
        }));
        fixed_tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: config.max_length,
                ..Default::default()
            }))
            .expect("truncation");

        let prefixed = format!("{QUERY_PREFIX}{text}");
        let encodings = fixed_tokenizer
            .encode_batch(vec![prefixed.as_str()], true)
            .expect("fixed tokenization failed");

        assert_eq!(
            encodings[0].get_ids().len(),
            512,
            "Fixed padding must produce 512 tokens"
        );

        let mut input_ids = vec![0i64; 512];
        let mut attention = vec![0i64; 512];
        for (i, &token) in encodings[0].get_ids().iter().enumerate() {
            input_ids[i] = token as i64;
        }
        for (i, &mask) in encodings[0].get_attention_mask().iter().enumerate() {
            attention[i] = mask as i64;
        }

        use ort::value::TensorRef;
        let shape = [1, 512];
        let input_ids_val = TensorRef::from_array_view((shape, input_ids.as_slice())).unwrap();
        let attention_val = TensorRef::from_array_view((shape, attention.as_slice())).unwrap();

        let session_arc = pool.acquire_session();
        let mut session = session_arc.lock();
        let outputs = if pool.uses_token_type_ids {
            let token_type_data = vec![0i64; 512];
            let token_types_val =
                TensorRef::from_array_view((shape, token_type_data.as_slice())).unwrap();
            session
                .run(ort::inputs![input_ids_val, attention_val, token_types_val])
                .expect("session run")
        } else {
            session
                .run(ort::inputs![input_ids_val, attention_val])
                .expect("session run")
        };
        let fixed_vec = extract_session_embeddings(&outputs, 1)
            .expect("extract embeddings")
            .into_iter()
            .next()
            .expect("single batch vector");

        let sim = crate::distance::cosine_similarity(&dynamic_vec, &fixed_vec);
        let max_diff = dynamic_vec
            .iter()
            .zip(fixed_vec.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);

        let model_file = embedder.model_file_name();
        eprintln!(
            "Model {model_file}: cosine similarity between dynamic and fixed(512) vectors: {sim:.8}"
        );
        eprintln!("Model {model_file}: max absolute difference: {max_diff:.8}");

        let (min_sim, max_allowed_diff) = if model_file == "model_fp16.onnx" {
            (0.999f32, 5e-3f32)
        } else {
            (0.9999f32, 1e-3f32)
        };

        assert!(
            sim > min_sim,
            "Cosine similarity between dynamic and fixed padding for {model_file} must be > {min_sim}, got {sim:.8}"
        );
        assert!(
            max_diff < max_allowed_diff,
            "Max absolute difference between dynamic and fixed padding for {model_file} must be < {max_allowed_diff}, got {max_diff:.8}"
        );
    }

    #[test]
    #[ignore]
    fn measure_single_query_latency() {
        let embedder = Embedder::new().expect("failed to create embedder");
        let query = "keyboard EMR interference";

        // Warmup
        let _ = embedder.encode_query(query).expect("warmup failed");

        let iters = 10;
        let mut total_duration = std::time::Duration::ZERO;

        for _ in 0..iters {
            let start = std::time::Instant::now();
            let _ = embedder.encode_query(query).expect("encode failed");
            total_duration += start.elapsed();
        }

        let avg_ms = total_duration.as_secs_f64() * 1000.0 / (iters as f64);
        eprintln!("POST_FIX_AVG_LATENCY_MS: {avg_ms:.2} ms");
    }

    fn build_synthetic_nupkg(entries: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
        use std::io::Write;
        let buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap()
    }

    #[test]
    fn ort_nuget_spec_maps_supported_targets_and_rejects_unsupported() {
        let cpu_cases = [
            ("linux", "x86_64", "linux-x64", "libonnxruntime.so"),
            ("linux", "aarch64", "linux-arm64", "libonnxruntime.so"),
            ("macos", "aarch64", "osx-arm64", "libonnxruntime.dylib"),
            ("macos", "x86_64", "osx-x64", "libonnxruntime.dylib"),
            ("windows", "x86_64", "win-x64", "onnxruntime.dll"),
            ("windows", "aarch64", "win-arm64", "onnxruntime.dll"),
        ];
        for (os, arch, expected_rid, expected_lib) in cpu_cases {
            let spec = ort_nuget_spec(os, arch, false).expect("CPU target should resolve");
            assert_eq!(spec.url, ORT_NUGET_CPU_URL);
            assert_eq!(spec.rid, expected_rid);
            assert_eq!(spec.lib_name, expected_lib);
        }

        let gpu_linux = ort_nuget_spec("linux", "x86_64", true).expect("linux GPU should resolve");
        assert_eq!(gpu_linux.url, ORT_NUGET_GPU_LINUX_URL);
        assert_eq!(gpu_linux.rid, "linux-x64");
        assert_eq!(gpu_linux.lib_name, "libonnxruntime.so");

        let gpu_win = ort_nuget_spec("windows", "x86_64", true).expect("win GPU should resolve");
        assert_eq!(gpu_win.url, ORT_NUGET_GPU_WINDOWS_URL);
        assert_eq!(gpu_win.rid, "win-x64");
        assert_eq!(gpu_win.lib_name, "onnxruntime.dll");

        for (os, arch) in [
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "aarch64"),
        ] {
            assert!(
                ort_nuget_spec(os, arch, true).is_err(),
                "GPU variant on {os}/{arch} must fail fast"
            );
        }

        assert!(ort_nuget_spec("freebsd", "x86_64", false).is_err());
        assert!(ort_nuget_spec("linux", "riscv64", false).is_err());
    }

    #[test]
    fn extract_ort_from_zip_filters_rid_and_shared_libs() {
        let cursor = build_synthetic_nupkg(&[
            (
                "runtimes/linux-x64/native/libonnxruntime.so",
                b"main-linux-so",
            ),
            (
                "runtimes/linux-x64/native/libonnxruntime_providers_shared.so",
                b"shared-so",
            ),
            (
                "runtimes/linux-x64/native/libonnxruntime_providers_cuda.so.1.23.2",
                b"cuda-versioned-so",
            ),
            ("runtimes/linux-x64/native/README.txt", b"ignore-txt"),
            (
                "runtimes/linux-x64/native/nested/libignored.so",
                b"ignore-nested",
            ),
            (
                "runtimes/linux-arm64/native/libonnxruntime.so",
                b"wrong-rid-arm64",
            ),
            ("runtimes/win-x64/native/onnxruntime.dll", b"wrong-rid-win"),
            ("[Content_Types].xml", b"<Types/>"),
            (
                "runtimes/linux-x64/native/../../traversal.so",
                b"ignore-traversal",
            ),
        ]);

        let out_dir = tempfile::tempdir().unwrap();
        extract_ort_from_zip(cursor, "linux-x64", "libonnxruntime.so", out_dir.path())
            .expect("extract should succeed");

        assert_eq!(
            std::fs::read(out_dir.path().join("libonnxruntime.so")).unwrap(),
            b"main-linux-so"
        );
        assert_eq!(
            std::fs::read(out_dir.path().join("libonnxruntime_providers_shared.so")).unwrap(),
            b"shared-so"
        );
        assert_eq!(
            std::fs::read(
                out_dir
                    .path()
                    .join("libonnxruntime_providers_cuda.so.1.23.2")
            )
            .unwrap(),
            b"cuda-versioned-so"
        );

        let mut extracted: Vec<String> = std::fs::read_dir(out_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        extracted.sort();
        assert_eq!(
            extracted,
            vec![
                "libonnxruntime.so",
                "libonnxruntime_providers_cuda.so.1.23.2",
                "libonnxruntime_providers_shared.so",
            ]
        );
    }

    #[test]
    fn extract_ort_from_zip_windows_ignores_lib_files() {
        let cursor = build_synthetic_nupkg(&[
            ("runtimes/win-x64/native/onnxruntime.dll", b"main-win-dll"),
            (
                "runtimes/win-x64/native/onnxruntime_providers_shared.dll",
                b"shared-win-dll",
            ),
            ("runtimes/win-x64/native/onnxruntime.lib", b"import-lib"),
            (
                "runtimes/win-arm64/native/onnxruntime.dll",
                b"wrong-rid-arm64",
            ),
        ]);

        let out_dir = tempfile::tempdir().unwrap();
        extract_ort_from_zip(cursor, "win-x64", "onnxruntime.dll", out_dir.path())
            .expect("windows extract should succeed");

        let mut extracted: Vec<String> = std::fs::read_dir(out_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        extracted.sort();
        assert_eq!(
            extracted,
            vec!["onnxruntime.dll", "onnxruntime_providers_shared.dll"]
        );
    }

    #[test]
    fn extract_ort_from_zip_fails_when_main_lib_missing() {
        let cursor = build_synthetic_nupkg(&[
            (
                "runtimes/linux-x64/native/libonnxruntime_providers_shared.so",
                b"shared-only",
            ),
            (
                "runtimes/linux-arm64/native/libonnxruntime.so",
                b"other-rid",
            ),
        ]);

        let out_dir = tempfile::tempdir().unwrap();
        let err = extract_ort_from_zip(cursor, "linux-x64", "libonnxruntime.so", out_dir.path())
            .expect_err("missing main lib must fail fast");
        assert!(err.to_string().contains("libonnxruntime.so"));
    }

    #[test]
    #[ignore] // Requires real BAAI/bge-m3 (fp32) and Xenova/bge-m3 (fp16) ONNX weights
    fn fp32_and_fp16_embeddings_are_interchangeable() {
        let models_dir = std::env::var("AOPS_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| get_models_dir());

        let queries: [&str; 6] = [
            "split ergonomic keyboard wireless receiver interference",
            "SessionOutputs try_extract_tensor half::f16 last_hidden_state",
            "dense retrieval asymmetric query token cap and chunk contextual headers",
            "append_wal_record wal.compacting bincode atomic snapshot compaction",
            "proportionality test for indefinite account suspension under human rights law",
            "/sys/fs/cgroup/memory.max ONNX session pool OOM cap",
        ];

        let spec_chunk_350_tokens = "\
PKB Search | spec | Retrieval > Dense Encoding\n\n\
Dense semantic retrieval encodes every markdown chunk and incoming search query into a \
1024-dimensional unit vector using the multilingual BGE-M3 transformer architecture. \
During indexing, the markdown-aware semantic chunker splits documents on structural \
paragraph, list, and fenced-code boundaries while prepending a concise contextual \
header formatted as `Title | type | Section > Subsection` ahead of each body passage \
so isolated paragraphs retain their document identity and topical hierarchy. \
Document passages are tokenized with XLM-RoBERTa SentencePiece up to `max_length = 512` \
tokens using dynamic batch padding (`PaddingStrategy::BatchLongest`) rather than fixed \
512-token padding, avoiding quadratic self-attention overhead on short notes while \
preserving prefix-identical attention masks. Incoming user queries apply an asymmetric \
dense token budget (`DEFAULT_QUERY_DENSE_TOKEN_CAP = 160`) and an in-memory LRU vector \
cache. At retrieval time, the engine computes exact cosine similarity against all chunk \
vectors in `VectorStore` and fuses dense similarity with lexical BM25 scores.";

        let documents: [&str; 6] = [
            "Wireless split keyboards using low-power 2.4 GHz transceivers suffer packet drops \
             when placed directly beside USB 3.0 hubs or unshielded phone chargers. Moving the \
             receiver dongle onto a shielded extension cable eliminates key-up stalls.",
            "In src/embeddings.rs, extract_session_embeddings inspects SessionOutputs for \
             sentence_embedding first and falls back to last_hidden_state or token_embeddings. \
             When ValueType::Tensor reports TensorElementType::Float16, \
             try_extract_tensor::<half::f16>() converts token 0 ([CLS]) via half::f16::to_f32 \
             before L2 normalization.",
            spec_chunk_350_tokens,
            "VectorStore::save rotates pkb_vectors.wal to pkb_vectors.wal.compacting before \
             serializing the bincode snapshot to a temporary file and atomically renaming it \
             into place. Each append_wal_record call frames a 4-byte little-endian payload \
             length with O_APPEND so concurrent writers never interleave bytes.",
            "Independent oversight review of indefinite platform account suspensions evaluates \
             legality, legitimate public-safety aim, necessity, and proportionality under \
             international freedom-of-expression standards, requiring periodic reassessment \
             criteria rather than permanent unreviewed bans.",
            "Every CPU ONNX Runtime session retains its own optimized copy of model weights \
             plus an inference arena. Reading /sys/fs/cgroup/memory.max alongside /proc/meminfo \
             MemTotal lets sessions_within_memory cap parallel sessions to those fitting within \
             half the container memory budget, preventing cgroup OOM kills.",
        ];

        let assert_valid_unit_vectors = |label: &str, vecs: &[Vec<f32>]| {
            for (i, v) in vecs.iter().enumerate() {
                assert_eq!(v.len(), EMBEDDING_DIM, "{label}[{i}] wrong dim");
                assert!(
                    v.iter().all(|x| x.is_finite()),
                    "{label}[{i}] contains NaN or Inf"
                );
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                assert!(
                    (norm - 1.0).abs() < 1e-4,
                    "{label}[{i}] expected unit L2 norm, got {norm:.6}"
                );
            }
        };

        // 1. Load fp32 with a single session (respecting 6 GiB cgroup limit), encode, then drop.
        let fp32_embedder = Embedder::new_for_variant(&models_dir, ModelVariant::Fp32)
            .expect("failed to initialize fp32 embedder")
            .with_overrides(1, 4, 8);
        assert_eq!(fp32_embedder.model_file_name(), "model.onnx");

        let tokenizer_path = models_dir.join("tokenizer.json");
        let tok_mtime_before = std::fs::metadata(&tokenizer_path)
            .expect("stat tokenizer.json after fp32 init")
            .modified()
            .expect("tokenizer.json mtime");
        let tok_hash_before = blake3::hash(
            &std::fs::read(&tokenizer_path).expect("read tokenizer.json after fp32 init"),
        );

        {
            let pool = fp32_embedder.ensure_pool().expect("fp32 ensure_pool");
            let spec_tokens = pool
                .tokenizer
                .encode(spec_chunk_350_tokens, true)
                .expect("tokenize ~350-token chunk")
                .get_ids()
                .len();
            eprintln!("Contextual spec chunk token count: {spec_tokens}");
            assert!(
                (280..=450).contains(&spec_tokens),
                "Expected representative ~350-token spec chunk, got {spec_tokens} tokens"
            );
        }

        let fp32_queries_single: Vec<Vec<f32>> = queries
            .iter()
            .map(|q| fp32_embedder.encode_query(q).expect("fp32 encode_query"))
            .collect();
        let fp32_queries_batch = fp32_embedder
            .encode_batch(&queries)
            .expect("fp32 encode_batch queries");
        let fp32_docs_batch = fp32_embedder
            .encode_batch(&documents)
            .expect("fp32 encode_batch documents");

        // Explicitly drop fp32 before constructing fp16 so both models are never co-resident in RAM.
        drop(fp32_embedder);

        // 2. Load fp16 against the same directory (auto-downloads model_fp16.onnx, preserves tokenizer.json).
        let fp16_embedder = Embedder::new_for_variant(&models_dir, ModelVariant::Fp16)
            .expect("failed to initialize fp16 embedder")
            .with_overrides(1, 4, 8);
        assert_eq!(fp16_embedder.model_file_name(), "model_fp16.onnx");
        assert!(models_dir.join("model_fp16.onnx").exists());

        let tok_mtime_after = std::fs::metadata(&tokenizer_path)
            .expect("stat tokenizer.json after fp16 init")
            .modified()
            .expect("tokenizer.json mtime after fp16");
        let tok_hash_after = blake3::hash(
            &std::fs::read(&tokenizer_path).expect("read tokenizer.json after fp16 init"),
        );
        assert_eq!(
            tok_mtime_before, tok_mtime_after,
            "fp16 initialization must not modify existing tokenizer.json mtime"
        );
        assert_eq!(
            tok_hash_before, tok_hash_after,
            "fp16 initialization must not alter existing tokenizer.json contents"
        );

        let fp16_queries_single: Vec<Vec<f32>> = queries
            .iter()
            .map(|q| fp16_embedder.encode_query(q).expect("fp16 encode_query"))
            .collect();
        let fp16_queries_batch = fp16_embedder
            .encode_batch(&queries)
            .expect("fp16 encode_batch queries");
        let fp16_docs_batch = fp16_embedder
            .encode_batch(&documents)
            .expect("fp16 encode_batch documents");

        drop(fp16_embedder);

        assert_valid_unit_vectors("fp32_queries_single", &fp32_queries_single);
        assert_valid_unit_vectors("fp32_queries_batch", &fp32_queries_batch);
        assert_valid_unit_vectors("fp32_docs_batch", &fp32_docs_batch);
        assert_valid_unit_vectors("fp16_queries_single", &fp16_queries_single);
        assert_valid_unit_vectors("fp16_queries_batch", &fp16_queries_batch);
        assert_valid_unit_vectors("fp16_docs_batch", &fp16_docs_batch);

        let max_abs_diff = |a: &[f32], b: &[f32]| -> f32 {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max)
        };

        let mut all_sims = Vec::new();
        let mut overall_max_diff = 0.0f32;
        for (i, q) in queries.iter().enumerate() {
            let sim_q_single = crate::distance::cosine_similarity(
                &fp32_queries_single[i],
                &fp16_queries_single[i],
            );
            let sim_q_batch =
                crate::distance::cosine_similarity(&fp32_queries_batch[i], &fp16_queries_batch[i]);
            let diff_q = max_abs_diff(&fp32_queries_single[i], &fp16_queries_single[i]);
            overall_max_diff = overall_max_diff.max(diff_q);
            eprintln!(
                "Query[{i}] cosine(fp32, fp16): encode_query={sim_q_single:.8}, encode_batch={sim_q_batch:.8}, max_abs_diff={diff_q:.8} ({q:?})"
            );
            assert!(
                sim_q_single > 0.99,
                "Query[{i}] encode_query fp32-vs-fp16 cosine {sim_q_single:.8} <= 0.99"
            );
            assert!(
                sim_q_batch > 0.99,
                "Query[{i}] encode_batch fp32-vs-fp16 cosine {sim_q_batch:.8} <= 0.99"
            );
            all_sims.push(sim_q_single);
            all_sims.push(sim_q_batch);
        }

        for (i, d) in documents.iter().enumerate() {
            let sim_doc =
                crate::distance::cosine_similarity(&fp32_docs_batch[i], &fp16_docs_batch[i]);
            let diff_doc = max_abs_diff(&fp32_docs_batch[i], &fp16_docs_batch[i]);
            overall_max_diff = overall_max_diff.max(diff_doc);
            let preview = &d[..d.floor_char_boundary(60)];
            eprintln!(
                "Doc[{i}] cosine(fp32, fp16): encode_batch={sim_doc:.8}, max_abs_diff={diff_doc:.8} ({preview:?}...)"
            );
            assert!(
                sim_doc > 0.99,
                "Doc[{i}] encode_batch fp32-vs-fp16 cosine {sim_doc:.8} <= 0.99"
            );
            all_sims.push(sim_doc);
        }

        let min_sim = all_sims.iter().copied().fold(f32::INFINITY, f32::min);
        let mean_sim = all_sims.iter().sum::<f32>() / (all_sims.len() as f32);
        eprintln!(
            "fp32-vs-fp16 summary across {} comparisons: min={min_sim:.8}, mean={mean_sim:.8}, max_abs_diff={overall_max_diff:.8}",
            all_sims.len()
        );

        // 3. Bidirectional VectorStore retrieval: fp32 index + fp16 queries, and fp16 index + fp32 queries.
        let mut fp32_store = crate::vectordb::VectorStore::new(EMBEDDING_DIM);
        let mut fp16_store = crate::vectordb::VectorStore::new(EMBEDDING_DIM);
        let dummy_root = std::path::Path::new("/tmp");

        for (idx, doc_text) in documents.iter().enumerate() {
            let doc_id = format!("doc_{idx}");
            let pkb_doc = crate::pkb::PkbDocument {
                path: PathBuf::from(format!("{doc_id}.md")),
                title: format!("Representative Document {idx}"),
                tags: vec![],
                doc_type: Some("spec".to_string()),
                status: None,
                consolidated: None,
                consolidated_at: None,
                modified: None,
                body: (*doc_text).to_string(),
                content_hash: format!("chash_{idx}"),
                file_hash: format!("fhash_{idx}"),
                frontmatter: Some(serde_json::json!({ "id": doc_id })),
            };
            fp32_store.insert_precomputed(
                &pkb_doc,
                vec![(*doc_text).to_string()],
                vec![fp32_docs_batch[idx].clone()],
            );
            fp16_store.insert_precomputed(
                &pkb_doc,
                vec![(*doc_text).to_string()],
                vec![fp16_docs_batch[idx].clone()],
            );
        }

        for q_idx in 0..queries.len() {
            let expected_id = format!("doc_{q_idx}");
            let baseline_fp32 = fp32_store.search(
                &fp32_queries_single[q_idx],
                6,
                dummy_root,
                None,
                None,
                None,
                None,
            );
            let cross_fp32_idx_fp16_q = fp32_store.search(
                &fp16_queries_single[q_idx],
                6,
                dummy_root,
                None,
                None,
                None,
                None,
            );
            let baseline_fp16 = fp16_store.search(
                &fp16_queries_single[q_idx],
                6,
                dummy_root,
                None,
                None,
                None,
                None,
            );
            let cross_fp16_idx_fp32_q = fp16_store.search(
                &fp32_queries_single[q_idx],
                6,
                dummy_root,
                None,
                None,
                None,
                None,
            );

            assert_eq!(baseline_fp32[0].id, expected_id);
            assert_eq!(
                cross_fp32_idx_fp16_q[0].id, baseline_fp32[0].id,
                "Query[{q_idx}]: fp16 query against fp32 index changed top-1 hit"
            );
            assert_eq!(baseline_fp16[0].id, expected_id);
            assert_eq!(
                cross_fp16_idx_fp32_q[0].id, baseline_fp16[0].id,
                "Query[{q_idx}]: fp32 query against fp16 index changed top-1 hit"
            );
        }
    }
}
