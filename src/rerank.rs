//! Local Cross-Encoder Reranker for PKB Search.
//!
//! Evaluates fused candidate pairs (query, text) using a local transformer
//! model to reorder top results for maximal precision.

use crate::vectordb::SearchResult;
use ort::value::TensorRef;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_RERANK_TOP_N: usize = 40;

/// Reranker configuration.
#[derive(Debug, Clone)]
pub struct RerankerConfig {
    pub model_path: Option<PathBuf>,
    pub tokenizer_path: Option<PathBuf>,
    pub top_n_candidates: usize,
    pub enabled: bool,
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

        Self {
            model_path,
            tokenizer_path,
            top_n_candidates: DEFAULT_RERANK_TOP_N,
            enabled: true,
        }
    }
}

/// Cross-encoder reranker instance.
pub struct CrossEncoderReranker {
    config: RerankerConfig,
    session: Option<Arc<Mutex<ort::session::Session>>>,
    tokenizer: Option<tokenizers::Tokenizer>,
}

impl CrossEncoderReranker {
    pub fn new(config: RerankerConfig) -> Self {
        let mut session = None;
        let mut tokenizer = None;

        if config.enabled {
            if let (Some(m_path), Some(t_path)) = (&config.model_path, &config.tokenizer_path) {
                if m_path.exists() && t_path.exists() {
                    if let Ok(tok) = tokenizers::Tokenizer::from_file(t_path) {
                        tokenizer = Some(tok);
                    }
                    if let Ok(sess) = ort::session::Session::builder()
                        .and_then(|b| b.commit_from_file(m_path))
                    {
                        session = Some(Arc::new(Mutex::new(sess)));
                    }
                }
            }
        }

        Self {
            config,
            session,
            tokenizer,
        }
    }

    /// Check if the ONNX session is loaded and active.
    pub fn is_model_loaded(&self) -> bool {
        self.session.is_some() && self.tokenizer.is_some()
    }

    /// Rerank a slice of SearchResult candidates for the given query.
    pub fn rerank(&self, query: &str, candidates: &mut [SearchResult], top_k: usize) {
        if candidates.is_empty() || !self.config.enabled {
            return;
        }

        let n = candidates.len().min(self.config.top_n_candidates);
        let (to_rerank, _remainder) = candidates.split_at_mut(n);

        if let (Some(sess_mutex), Some(ref tokenizer)) = (&self.session, &self.tokenizer) {
            // ONNX Cross-Encoder scoring
            let mut scores = Vec::with_capacity(to_rerank.len());
            for r in to_rerank.iter() {
                let doc_text = if !r.snippet.is_empty() {
                    &r.snippet
                } else {
                    &r.chunk_text
                };

                let pair_input = format!("{query} [SEP] {doc_text}");
                if let Ok(encoding) = tokenizer.encode(pair_input.as_str(), true) {
                    let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
                    let mask: Vec<i64> = encoding
                        .get_attention_mask()
                        .iter()
                        .map(|&m| m as i64)
                        .collect();

                    let seq_len = ids.len();
                    let shape = [1usize, seq_len];

                    let input_ids_res = TensorRef::from_array_view((shape, ids.as_slice()));
                    let attention_res = TensorRef::from_array_view((shape, mask.as_slice()));

                    if let (Ok(input_ids_val), Ok(attention_val)) = (input_ids_res, attention_res) {
                        let mut score_val = None;
                        {
                            let mut sess = sess_mutex.lock();
                            let run_res = sess.run(ort::inputs![input_ids_val, attention_val]);
                            if let Ok(outputs) = run_res {
                                if let Some(tensor) = outputs.get("logits").or_else(|| outputs.get("output")) {
                                    if let Ok(val) = tensor.try_extract_tensor::<f32>() {
                                        score_val = val.1.first().copied();
                                    }
                                }
                            }
                        }
                        if let Some(logit) = score_val {
                            scores.push(logit);
                            continue;
                        }
                    }
                }
                // Fallback score if model inference failed on this single item
                scores.push(r.score);
            }

            for (r, score) in to_rerank.iter_mut().zip(scores) {
                r.score = score;
            }
        } else {
            // Fast lexical term-proximity and title alignment rerank pass
            // when full cross-encoder model is not loaded in memory.
            for r in to_rerank.iter_mut() {
                let boost = title_match_boost(query, &r.title);
                let query_lower = query.to_lowercase();
                let doc_lower = format!("{} {} {}", r.id, r.title, r.chunk_text).to_lowercase();

                let exact_id_match = if doc_lower.contains(&query_lower) { 1.5 } else { 1.0 };
                r.score = r.score * (1.0 + boost) * exact_id_match;
            }
        }

        to_rerank.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score).cmp(&ordered_float::OrderedFloat(a.score))
        });

        // Re-sort the whole slice up to top_k
        candidates.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score).cmp(&ordered_float::OrderedFloat(a.score))
        });
        if candidates.len() > top_k {
            // Caller may truncate
        }
    }
}

fn title_match_boost(query: &str, title: &str) -> f32 {
    let stopwords = [
        "a", "an", "and", "at", "for", "in", "is", "of", "on", "or", "the", "to", "with",
    ];
    let q_tokens: std::collections::HashSet<_> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|w| !stopwords.contains(w))
        .map(String::from)
        .collect();
    let t_tokens: std::collections::HashSet<_> = title
        .to_lowercase()
        .split_whitespace()
        .filter(|w| !stopwords.contains(w))
        .map(String::from)
        .collect();

    if q_tokens.is_empty() || t_tokens.is_empty() {
        return 0.0;
    }
    let intersection = q_tokens.intersection(&t_tokens).count() as f32;
    let min_len = q_tokens.len().min(t_tokens.len()) as f32;
    (intersection / min_len) * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reranker_fallback_reorders_exact_matches() {
        let mut results = vec![
            SearchResult {
                path: PathBuf::from("tasks/task-generic.md"),
                title: "General note on databases".to_string(),
                score: 0.05,
                snippet: "general text".to_string(),
                chunk_text: "general text".to_string(),
                id: "task-generic".to_string(),
                date: None,
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                tags: vec![],
                confidence: None,
            },
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
            },
        ];

        let config = RerankerConfig {
            model_path: None,
            tokenizer_path: None,
            top_n_candidates: 10,
            enabled: true,
        };
        let reranker = CrossEncoderReranker::new(config);
        reranker.rerank("PR #520", &mut results, 10);

        // Exact match for "PR #520" in task-target should elevate it to #1
        assert_eq!(results[0].id, "task-target");
    }
}
