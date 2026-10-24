//! Local Cross-Encoder Reranker for PKB Search.
//!
//! Evaluates fused candidate pairs (query, title_heading_chunk) using a local transformer
//! model to reorder top results for maximal precision.

use crate::vectordb::SearchResult;
use ort::value::TensorRef;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DEFAULT_RERANK_TOP_N: usize = 40;
pub const DEFAULT_QUERY_TOKEN_CAP: usize = 160;
pub const DEFAULT_CHUNK_TOKEN_CAP: usize = 352;
pub const DEFAULT_TAU_H: f32 = 0.50;
pub const DEFAULT_TAU_COS: f32 = 0.65;
pub const DEFAULT_TAU_S: f32 = 0.40;

/// Errors produced during reranking.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RerankerError {
    #[error("reranker_unavailable")]
    Unavailable(String),
    #[error("inference error: {0}")]
    Inference(String),
}

/// Errors produced during threshold selection.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ThresholdError {
    #[error("empty dataset: {0}")]
    EmptyDataset(String),
    #[error("parse error: {0}")]
    ParseError(String),
}

/// Model manifest file entry describing expected model artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFileEntry {
    pub filename: &'static str,
    pub required: bool,
    pub min_bytes: u64,
}

/// Verification manifest for cross-encoder model files.
#[derive(Debug, Clone)]
pub struct ModelManifest {
    pub name: &'static str,
    pub files: &'static [ModelFileEntry],
}

pub static DEFAULT_RERANKER_MANIFEST: ModelManifest = ModelManifest {
    name: "cross-encoder-reranker",
    files: &[
        ModelFileEntry {
            filename: "model.onnx",
            required: true,
            min_bytes: 1024,
        },
        ModelFileEntry {
            filename: "tokenizer.json",
            required: true,
            min_bytes: 1024,
        },
    ],
};

/// Verify that model files meet the manifest requirements.
pub fn verify_manifest(dir: &Path, manifest: &ModelManifest) -> Result<(), RerankerError> {
    for entry in manifest.files {
        let file_path = dir.join(entry.filename);
        if !file_path.exists() {
            if entry.required {
                return Err(RerankerError::Unavailable(format!(
                    "manifest verification failed: missing file {:?}",
                    entry.filename
                )));
            }
            continue;
        }
        let metadata = std::fs::metadata(&file_path).map_err(|e| {
            RerankerError::Unavailable(format!(
                "manifest verification failed: cannot read metadata for {:?}: {e}",
                entry.filename
            ))
        })?;
        if metadata.len() < entry.min_bytes {
            return Err(RerankerError::Unavailable(format!(
                "manifest verification failed: {:?} is smaller than min bytes ({} < {})",
                entry.filename,
                metadata.len(),
                entry.min_bytes
            )));
        }
    }
    Ok(())
}

/// Reranker configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankerConfig {
    pub model_path: Option<PathBuf>,
    pub tokenizer_path: Option<PathBuf>,
    pub top_n_candidates: usize,
    pub query_token_cap: usize,
    pub chunk_token_cap: usize,
    pub enabled: bool,
    pub tau_h: f32,
    pub tau_cos: f32,
    pub tau_s: f32,
}

impl Default for RerankerConfig {
    fn default() -> Self {
        let base_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("aops/models/reranker");

        let model_path = std::env::var("AOPS_RERANKER_MODEL")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                let p = base_dir.join("model.onnx");
                if p.exists() {
                    Some(p)
                } else {
                    None
                }
            });

        let tokenizer_path = std::env::var("AOPS_RERANKER_TOKENIZER")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                let p = base_dir.join("tokenizer.json");
                if p.exists() {
                    Some(p)
                } else {
                    None
                }
            });

        let explicit_mode = std::env::var("AOPS_RERANKER").ok();

        let enabled = match explicit_mode.as_deref() {
            Some("off") | Some("false") | Some("0") => false,
            Some("on") | Some("true") | Some("1") => true,
            Some(_) => true,
            None => model_path.is_some() || std::env::var("AOPS_RERANKER_MODEL").is_ok(),
        };

        let top_n_candidates = std::env::var("AOPS_RERANKER_TOP_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RERANK_TOP_N);

        let query_token_cap = std::env::var("AOPS_RERANKER_QUERY_TOKEN_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_QUERY_TOKEN_CAP);

        let chunk_token_cap = std::env::var("AOPS_RERANKER_CHUNK_TOKEN_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_CHUNK_TOKEN_CAP);

        let tau_h = std::env::var("AOPS_RERANKER_TAU_H")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TAU_H);

        let tau_cos = std::env::var("AOPS_RERANKER_TAU_COS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TAU_COS);

        let tau_s = std::env::var("AOPS_RERANKER_TAU_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TAU_S);

        Self {
            model_path,
            tokenizer_path,
            top_n_candidates,
            query_token_cap,
            chunk_token_cap,
            enabled,
            tau_h,
            tau_cos,
            tau_s,
        }
    }
}

/// Sigmoid function mapping finite logits to (0, 1).
#[inline]
pub fn sigmoid(logit: f32) -> f32 {
    if logit < -80.0 {
        return 0.0000001;
    }
    if logit > 80.0 {
        return 0.9999999;
    }
    1.0 / (1.0 + (-logit).exp())
}

/// Formats candidate document as `title · heading · chunk` without redundant title duplication.
pub fn build_title_heading_chunk(title: &str, chunk: &str) -> String {
    let t = title.trim();
    let c = chunk.trim();
    if t.is_empty() {
        c.to_string()
    } else if c.is_empty() {
        t.to_string()
    } else if c.starts_with(t) {
        c.to_string()
    } else {
        format!("{t} · {c}")
    }
}

/// Truncate text to a maximum number of tokens using the given tokenizer.
pub fn truncate_to_tokens(tokenizer: &tokenizers::Tokenizer, text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    if let Ok(encoding) = tokenizer.encode(text, false) {
        let ids = encoding.get_ids();
        if ids.len() > max_tokens {
            if let Ok(decoded) = tokenizer.decode(&ids[..max_tokens], false) {
                return decoded;
            }
        }
    }
    text.to_string()
}

/// Load status of the reranker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankerLoadStatus {
    Off,
    Loaded,
    Failed(String),
}

/// Cross-encoder reranker instance.
pub struct CrossEncoderReranker {
    config: RerankerConfig,
    session: Option<Arc<Mutex<ort::session::Session>>>,
    tokenizer: Option<tokenizers::Tokenizer>,
    uses_token_type_ids: bool,
    status: RerankerLoadStatus,
}

impl CrossEncoderReranker {
    pub fn new(config: RerankerConfig) -> Self {
        if let Ok(mode) = std::env::var("AOPS_RERANKER") {
            if !matches!(
                mode.as_str(),
                "off" | "false" | "0" | "on" | "true" | "1"
            ) {
                return Self {
                    config,
                    session: None,
                    tokenizer: None,
                    uses_token_type_ids: false,
                    status: RerankerLoadStatus::Failed(format!(
                        "Invalid AOPS_RERANKER mode: {mode}"
                    )),
                };
            }
        }

        if !config.enabled {
            return Self {
                config,
                session: None,
                tokenizer: None,
                uses_token_type_ids: false,
                status: RerankerLoadStatus::Off,
            };
        }

        let (m_path, t_path) = match (&config.model_path, &config.tokenizer_path) {
            (Some(m), Some(t)) => (m, t),
            _ => {
                return Self {
                    config,
                    session: None,
                    tokenizer: None,
                    uses_token_type_ids: false,
                    status: RerankerLoadStatus::Failed("Missing model or tokenizer path".to_string()),
                };
            }
        };

        if !m_path.exists() || !t_path.exists() {
            return Self {
                config,
                session: None,
                tokenizer: None,
                uses_token_type_ids: false,
                status: RerankerLoadStatus::Failed("Model or tokenizer path does not exist".to_string()),
            };
        }

        if let (Some(m_dir), Some(t_dir)) = (m_path.parent(), t_path.parent()) {
            if m_dir == t_dir
                && m_path.file_name().and_then(|n| n.to_str()) == Some("model.onnx")
                && t_path.file_name().and_then(|n| n.to_str()) == Some("tokenizer.json")
            {
                if let Err(e) = verify_manifest(m_dir, &DEFAULT_RERANKER_MANIFEST) {
                    return Self {
                        config,
                        session: None,
                        tokenizer: None,
                        uses_token_type_ids: false,
                        status: RerankerLoadStatus::Failed(e.to_string()),
                    };
                }
            }
        }

        if let Ok(metadata) = std::fs::metadata(m_path) {
            if metadata.len() == 0 {
                return Self {
                    config,
                    session: None,
                    tokenizer: None,
                    uses_token_type_ids: false,
                    status: RerankerLoadStatus::Failed("Model file is empty".to_string()),
                };
            }
        }

        let tokenizer = match tokenizers::Tokenizer::from_file(t_path) {
            Ok(tok) => tok,
            Err(e) => {
                return Self {
                    config,
                    session: None,
                    tokenizer: None,
                    uses_token_type_ids: false,
                    status: RerankerLoadStatus::Failed(format!("Failed to load tokenizer: {e}")),
                };
            }
        };

        let session = match ort::session::Session::builder().and_then(|b| b.commit_from_file(m_path)) {
            Ok(sess) => sess,
            Err(e) => {
                return Self {
                    config,
                    session: None,
                    tokenizer: Some(tokenizer),
                    uses_token_type_ids: false,
                    status: RerankerLoadStatus::Failed(format!("Failed to load session: {e}")),
                };
            }
        };

        let uses_token_type_ids = session.inputs().iter().any(|i| i.name() == "token_type_ids");

        Self {
            config,
            session: Some(Arc::new(Mutex::new(session))),
            tokenizer: Some(tokenizer),
            uses_token_type_ids,
            status: RerankerLoadStatus::Loaded,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn is_model_loaded(&self) -> bool {
        matches!(self.status, RerankerLoadStatus::Loaded)
    }

    pub fn status(&self) -> &RerankerLoadStatus {
        &self.status
    }

    pub fn config(&self) -> &RerankerConfig {
        &self.config
    }

    /// Rerank a slice of SearchResult candidates for the given query.
    ///
    /// Encodes (query, title_heading_chunk) pairs using the tokenizer's native pair encoding,
    /// batches candidate pairs into a single session.run pass, and applies sigmoid so scores live in (0, 1).
    ///
    /// If configured off: does no work and returns Ok(()).
    /// If configured on and model failed to load: returns Err(RerankerError::Unavailable).
    pub fn rerank(&self, query: &str, candidates: &mut [SearchResult], top_k: usize) -> Result<(), RerankerError> {
        if candidates.is_empty() || !self.config.enabled {
            return Ok(());
        }

        if !self.is_model_loaded() {
            return Err(RerankerError::Unavailable("reranker_unavailable".to_string()));
        }

        let (Some(sess_mutex), Some(ref tokenizer)) = (&self.session, &self.tokenizer) else {
            return Err(RerankerError::Unavailable("reranker_unavailable".to_string()));
        };

        let n = candidates.len().min(self.config.top_n_candidates);
        let (to_rerank, _remainder) = candidates.split_at_mut(n);
        if to_rerank.is_empty() {
            return Ok(());
        }

        // Cap query tokens
        let query_truncated = truncate_to_tokens(tokenizer, query, self.config.query_token_cap);

        // Native pair encoding for each candidate in to_rerank
        let mut encodings = Vec::with_capacity(to_rerank.len());
        for r in to_rerank.iter() {
            let body_chunk = if !r.chunk_text.is_empty() {
                &r.chunk_text
            } else {
                &r.snippet
            };
            let doc_text = build_title_heading_chunk(&r.title, body_chunk);
            let doc_truncated = truncate_to_tokens(tokenizer, &doc_text, self.config.chunk_token_cap);

            // Native pair encoding: (query, doc)
            let encoding = tokenizer
                .encode((query_truncated.as_str(), doc_truncated.as_str()), true)
                .map_err(|e| RerankerError::Inference(format!("pair encoding failed: {e}")))?;
            encodings.push(encoding);
        }

        let batch_size = encodings.len();
        let max_seq_len = encodings.iter().map(|e| e.get_ids().len()).max().unwrap_or(0);
        if max_seq_len == 0 {
            return Ok(());
        }

        let total_tokens = batch_size * max_seq_len;
        let mut input_ids = vec![0i64; total_tokens];
        let mut attention_mask = vec![0i64; total_tokens];
        let mut token_type_ids = vec![0i64; total_tokens];

        for (b, enc) in encodings.iter().enumerate() {
            let offset = b * max_seq_len;
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let type_ids = enc.get_type_ids();

            for (i, &id) in ids.iter().enumerate() {
                input_ids[offset + i] = id as i64;
            }
            for (i, &m) in mask.iter().enumerate() {
                attention_mask[offset + i] = m as i64;
            }
            for (i, &t) in type_ids.iter().enumerate() {
                token_type_ids[offset + i] = t as i64;
            }
        }

        let shape = [batch_size, max_seq_len];
        let input_ids_tensor = TensorRef::from_array_view((shape, input_ids.as_slice()))
            .map_err(|e| RerankerError::Inference(format!("tensor input_ids failed: {e}")))?;
        let attention_tensor = TensorRef::from_array_view((shape, attention_mask.as_slice()))
            .map_err(|e| RerankerError::Inference(format!("tensor attention_mask failed: {e}")))?;

        let mut logits = Vec::with_capacity(batch_size);
        {
            let mut sess = sess_mutex.lock();
            let outputs = if self.uses_token_type_ids {
                let token_types_tensor = TensorRef::from_array_view((shape, token_type_ids.as_slice()))
                    .map_err(|e| RerankerError::Inference(format!("tensor token_type_ids failed: {e}")))?;
                sess.run(ort::inputs![input_ids_tensor, attention_tensor, token_types_tensor])
                    .map_err(|e| RerankerError::Inference(format!("session run failed: {e}")))?
            } else {
                sess.run(ort::inputs![input_ids_tensor, attention_tensor])
                    .map_err(|e| RerankerError::Inference(format!("session run failed: {e}")))?
            };

            let tensor = outputs
                .get("logits")
                .or_else(|| outputs.get("output"))
                .ok_or_else(|| RerankerError::Inference("model output has no logits or output tensor".to_string()))?;

            let (extracted_shape, extracted_data) = tensor
                .try_extract_tensor::<f32>()
                .map_err(|e| RerankerError::Inference(format!("tensor extraction failed: {e}")))?;

            if extracted_data.len() == batch_size {
                logits.extend_from_slice(extracted_data);
            } else if extracted_data.len() >= batch_size {
                let stride = extracted_data.len() / batch_size;
                for b in 0..batch_size {
                    logits.push(extracted_data[b * stride]);
                }
            } else {
                return Err(RerankerError::Inference(format!(
                    "unexpected output shape {:?} for batch_size {}",
                    extracted_shape, batch_size
                )));
            }
        }

        // Apply sigmoid transformation: 1 / (1 + exp(-logit)) into (0, 1)
        for (r, &logit) in to_rerank.iter_mut().zip(logits.iter()) {
            if logit.is_nan() {
                return Err(RerankerError::Inference(
                    "model produced NaN logit".to_string(),
                ));
            }
            r.score = sigmoid(logit);
        }

        // Sort reranked top-N candidates by sigmoid score descending;
        // preserve _remainder in its fused order after to_rerank.
        to_rerank.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score)
                .cmp(&ordered_float::OrderedFloat(a.score))
                .then_with(|| a.id.cmp(&b.id))
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });

        if candidates.len() > top_k {
            // caller may truncate
        }

        Ok(())
    }
}

/// Result of calibrating a single threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdResult {
    pub tau: f32,
    pub false_abstain_rate: f32,
    pub quiet_rate: f32,
}

/// Selected thresholds across τ_h, τ_cos, and τ_s.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedThresholds {
    pub tau_h: ThresholdResult,
    pub tau_cos: ThresholdResult,
    pub tau_s: ThresholdResult,
}

/// Compute threshold τ at false-abstain bound <= max_false_abstain (e.g. 0.15).
pub fn select_threshold(
    answerable_scores: &[f32],
    no_answer_scores: &[f32],
    max_false_abstain: f32,
) -> ThresholdResult {
    if answerable_scores.is_empty() {
        return ThresholdResult {
            tau: 0.5,
            false_abstain_rate: 0.0,
            quiet_rate: 1.0,
        };
    }

    let mut sorted_ans = answerable_scores.to_vec();
    sorted_ans.sort_by_key(|a| ordered_float::OrderedFloat(*a));

    let n_ans = sorted_ans.len();
    let max_k = ((n_ans as f32) * max_false_abstain).floor() as usize;
    let k = max_k.min(n_ans.saturating_sub(1));

    let tau = sorted_ans[k];
    let false_abstains = sorted_ans.iter().filter(|&&s| s < tau).count();
    let false_abstain_rate = false_abstains as f32 / n_ans as f32;

    let quiet_count = no_answer_scores.iter().filter(|&&s| s < tau).count();
    let quiet_rate = if no_answer_scores.is_empty() {
        1.0
    } else {
        quiet_count as f32 / no_answer_scores.len() as f32
    };

    ThresholdResult {
        tau,
        false_abstain_rate,
        quiet_rate,
    }
}

/// Parses TSV fixture and computes SelectedThresholds for τ_h, τ_cos, and τ_s.
pub fn select_thresholds_from_tsv(
    tsv_content: &str,
    max_false_abstain: f32,
) -> Result<SelectedThresholds, ThresholdError> {
    let mut ans_h = Vec::new();
    let mut noans_h = Vec::new();
    let mut ans_cos = Vec::new();
    let mut noans_cos = Vec::new();
    let mut ans_s = Vec::new();
    let mut noans_s = Vec::new();

    for line in tsv_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 5 {
            return Err(ThresholdError::ParseError(format!(
                "expected at least 5 TSV columns, got {}: {trimmed}",
                cols.len()
            )));
        }

        // Skip header line
        if cols[0].eq_ignore_ascii_case("query_id") || cols[1].eq_ignore_ascii_case("is_answerable") {
            continue;
        }

        let is_answerable = match cols[1].trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "answerable" => true,
            "false" | "0" | "no" | "no_answer" | "noanswer" => false,
            other => {
                return Err(ThresholdError::ParseError(format!(
                    "unknown is_answerable value: {other}"
                )));
            }
        };

        let score_h: f32 = cols[2]
            .trim()
            .parse()
            .map_err(|e| ThresholdError::ParseError(format!("invalid score_h: {e}")))?;
        let score_cos: f32 = cols[3]
            .trim()
            .parse()
            .map_err(|e| ThresholdError::ParseError(format!("invalid score_cos: {e}")))?;
        let score_s: f32 = cols[4]
            .trim()
            .parse()
            .map_err(|e| ThresholdError::ParseError(format!("invalid score_s: {e}")))?;

        if is_answerable {
            ans_h.push(score_h);
            ans_cos.push(score_cos);
            ans_s.push(score_s);
        } else {
            noans_h.push(score_h);
            noans_cos.push(score_cos);
            noans_s.push(score_s);
        }
    }

    if ans_h.is_empty() {
        return Err(ThresholdError::EmptyDataset(
            "no answerable scores found in TSV".to_string(),
        ));
    }

    let tau_h = select_threshold(&ans_h, &noans_h, max_false_abstain);
    let tau_cos = select_threshold(&ans_cos, &noans_cos, max_false_abstain);
    let tau_s = select_threshold(&ans_s, &noans_s, max_false_abstain);

    Ok(SelectedThresholds {
        tau_h,
        tau_cos,
        tau_s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigmoid_bounds_in_0_1() {
        assert_eq!(sigmoid(0.0), 0.5);
        assert!(sigmoid(10.0) < 1.0);
        assert!(sigmoid(10.0) > 0.999);
        assert!(sigmoid(-10.0) > 0.0);
        assert!(sigmoid(-10.0) < 0.001);
        assert!(sigmoid(100.0) <= 1.0 && sigmoid(100.0) > 0.0);
        assert!(sigmoid(-100.0) >= 0.0 && sigmoid(-100.0) < 1.0);
    }

    #[test]
    fn test_build_title_heading_chunk() {
        assert_eq!(
            build_title_heading_chunk("My Title", "Some content"),
            "My Title · Some content"
        );
        assert_eq!(
            build_title_heading_chunk("My Title", "My Title · Already has title"),
            "My Title · Already has title"
        );
        assert_eq!(
            build_title_heading_chunk("", "Some content"),
            "Some content"
        );
        assert_eq!(
            build_title_heading_chunk("My Title", ""),
            "My Title"
        );
    }

    #[test]
    fn test_reranker_missing_model_raises_unavailable() {
        let config = RerankerConfig {
            model_path: Some(PathBuf::from("/nonexistent/model.onnx")),
            tokenizer_path: Some(PathBuf::from("/nonexistent/tokenizer.json")),
            top_n_candidates: 10,
            query_token_cap: 160,
            chunk_token_cap: 352,
            enabled: true,
            tau_h: DEFAULT_TAU_H,
            tau_cos: DEFAULT_TAU_COS,
            tau_s: DEFAULT_TAU_S,
        };
        let reranker = CrossEncoderReranker::new(config);
        assert!(!reranker.is_model_loaded());
        assert!(reranker.is_enabled());

        let mut results = vec![
            SearchResult {
                path: PathBuf::from("tasks/task-target.md"),
                title: "Fix bug in search".to_string(),
                score: 0.04,
                snippet: "found PR #520 in the codebase".to_string(),
                chunk_text: "found PR #520 in the codebase".to_string(),
                id: "task-target".to_string(),
                date: None,
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                tags: vec![],
                confidence: None,
                chunk_id: None,
            },
        ];

        let res = reranker.rerank("PR #520", &mut results, 10);
        assert!(res.is_err(), "must return error when configured on and model fails to load");
        let err = res.unwrap_err();
        assert_eq!(err, RerankerError::Unavailable("reranker_unavailable".to_string()));
    }

    #[test]
    fn test_reranker_off_no_lexical_fallback() {
        let mut results = vec![
            SearchResult {
                path: PathBuf::from("tasks/task-generic.md"),
                title: "General note on databases".to_string(),
                score: 0.50,
                snippet: "general text".to_string(),
                chunk_text: "general text".to_string(),
                id: "task-generic".to_string(),
                date: None,
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                tags: vec![],
                confidence: None,
                chunk_id: None,
            },
            SearchResult {
                path: PathBuf::from("tasks/task-target.md"),
                title: "Fix bug in search".to_string(),
                score: 0.40,
                snippet: "found PR #520 in the codebase".to_string(),
                chunk_text: "found PR #520 in the codebase".to_string(),
                id: "task-target".to_string(),
                date: None,
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                tags: vec![],
                confidence: None,
                chunk_id: None,
            },
        ];

        let config = RerankerConfig {
            model_path: None,
            tokenizer_path: None,
            top_n_candidates: 10,
            query_token_cap: 160,
            chunk_token_cap: 352,
            enabled: false,
            tau_h: DEFAULT_TAU_H,
            tau_cos: DEFAULT_TAU_COS,
            tau_s: DEFAULT_TAU_S,
        };
        let reranker = CrossEncoderReranker::new(config);
        assert!(!reranker.is_enabled());
        assert!(!reranker.is_model_loaded());

        let res = reranker.rerank("PR #520", &mut results, 10);
        assert!(res.is_ok());
        // Scores should remain unchanged (fused order preserved), lexical fallback must NOT boost task-target
        assert_eq!(results[0].id, "task-generic");
        assert_eq!(results[0].score, 0.50);
        assert_eq!(results[1].id, "task-target");
        assert_eq!(results[1].score, 0.40);
    }

    #[test]
    fn test_threshold_selection_synthetic_tsv() {
        let tsv_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/synthetic_scores.tsv");
        let content = std::fs::read_to_string(&tsv_path)
            .expect("synthetic_scores.tsv fixture must exist");

        let thresholds = select_thresholds_from_tsv(&content, 0.15)
            .expect("threshold selection must succeed");

        assert!(
            thresholds.tau_h.false_abstain_rate <= 0.15,
            "tau_h false abstain must be <= 0.15, got {}",
            thresholds.tau_h.false_abstain_rate
        );
        assert!(
            thresholds.tau_cos.false_abstain_rate <= 0.15,
            "tau_cos false abstain must be <= 0.15, got {}",
            thresholds.tau_cos.false_abstain_rate
        );
        assert!(
            thresholds.tau_s.false_abstain_rate <= 0.15,
            "tau_s false abstain must be <= 0.15, got {}",
            thresholds.tau_s.false_abstain_rate
        );

        // Quiet rate on no-answer queries should be high
        assert!(
            thresholds.tau_h.quiet_rate >= 0.80,
            "tau_h quiet rate must be >= 0.80, got {}",
            thresholds.tau_h.quiet_rate
        );
        assert!(
            thresholds.tau_cos.quiet_rate >= 0.80,
            "tau_cos quiet rate must be >= 0.80, got {}",
            thresholds.tau_cos.quiet_rate
        );
        assert!(
            thresholds.tau_s.quiet_rate >= 0.80,
            "tau_s quiet rate must be >= 0.80, got {}",
            thresholds.tau_s.quiet_rate
        );
    }

    #[test]
    fn test_manifest_verification() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();

        let manifest = ModelManifest {
            name: "test-manifest",
            files: &[
                ModelFileEntry {
                    filename: "model.onnx",
                    required: true,
                    min_bytes: 10,
                },
                ModelFileEntry {
                    filename: "tokenizer.json",
                    required: true,
                    min_bytes: 10,
                },
            ],
        };

        // Missing files
        assert!(verify_manifest(path, &manifest).is_err());

        // Create empty files (below min_bytes)
        std::fs::write(path.join("model.onnx"), b"").unwrap();
        std::fs::write(path.join("tokenizer.json"), b"").unwrap();
        assert!(verify_manifest(path, &manifest).is_err());

        // Create valid files (>= min_bytes)
        std::fs::write(path.join("model.onnx"), b"12345678901").unwrap();
        std::fs::write(path.join("tokenizer.json"), b"12345678901").unwrap();
        assert!(verify_manifest(path, &manifest).is_ok());
    }
}
