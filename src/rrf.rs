//! Reciprocal Rank Fusion (RRF) for combining vector and BM25 search results.
//!
//! RRF fuses multiple ranked lists into a single ranking using:
//! `RRF_score(d) = sum_{m in models} (weight_m / (k + rank_m(d)))`
//! Default parameters: k = 60 (qmd precedent).

use crate::vectordb::SearchResult;
use std::collections::HashMap;

pub const DEFAULT_RRF_K: usize = 60;
pub const DEFAULT_VECTOR_WEIGHT: f32 = 1.0;
pub const DEFAULT_BM25_WEIGHT: f32 = 1.0;

#[derive(Debug, Clone)]
pub struct RrfConfig {
    pub k: usize,
    pub vector_weight: f32,
    pub bm25_weight: f32,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self {
            k: DEFAULT_RRF_K,
            vector_weight: DEFAULT_VECTOR_WEIGHT,
            bm25_weight: DEFAULT_BM25_WEIGHT,
        }
    }
}

/// Fuse vector and BM25 search results using Reciprocal Rank Fusion.
pub fn fuse_rrf(
    vector_results: Vec<SearchResult>,
    bm25_results: Vec<SearchResult>,
    config: &RrfConfig,
    limit: usize,
) -> Vec<SearchResult> {
    let k = config.k as f32;
    let mut fused_scores: HashMap<String, f32> = HashMap::new();
    let mut doc_map: HashMap<String, SearchResult> = HashMap::new();

    // 1. Process vector results
    for (rank_idx, r) in vector_results.into_iter().enumerate() {
        let rank = (rank_idx + 1) as f32;
        let rrf_score = config.vector_weight / (k + rank);
        *fused_scores.entry(r.id.clone()).or_insert(0.0) += rrf_score;
        doc_map.insert(r.id.clone(), r);
    }

    // 2. Process BM25 results
    for (rank_idx, r) in bm25_results.into_iter().enumerate() {
        let rank = (rank_idx + 1) as f32;
        let rrf_score = config.bm25_weight / (k + rank);
        *fused_scores.entry(r.id.clone()).or_insert(0.0) += rrf_score;
        doc_map.entry(r.id.clone()).or_insert(r);
    }

    // 3. Assemble and rank
    let mut results: Vec<SearchResult> = Vec::with_capacity(fused_scores.len());
    for (id, score) in fused_scores {
        if let Some(mut doc) = doc_map.remove(&id) {
            doc.score = score;
            results.push(doc);
        }
    }

    results.sort_by(|a, b| {
        ordered_float::OrderedFloat(b.score).cmp(&ordered_float::OrderedFloat(a.score))
    });

    results.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_result(id: &str, title: &str, score: f32) -> SearchResult {
        SearchResult {
            path: PathBuf::from(format!("docs/{id}.md")),
            title: title.to_string(),
            score,
            snippet: format!("Snippet for {title}"),
            chunk_text: format!("Chunk for {title}"),
            id: id.to_string(),
            date: None,
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            tags: vec![],
            confidence: None,
        }
    }

    #[test]
    fn test_rrf_fusion_combines_and_ranks() {
        let vec_res = vec![
            make_result("doc_a", "Document A", 0.95),
            make_result("doc_b", "Document B", 0.85),
            make_result("doc_c", "Document C", 0.75),
        ];

        let bm25_res = vec![
            make_result("doc_b", "Document B", 12.0),
            make_result("doc_d", "Document D", 10.0),
            make_result("doc_a", "Document A", 8.0),
        ];

        let config = RrfConfig::default();
        let fused = fuse_rrf(vec_res, bm25_res, &config, 10);

        // doc_b is #2 in vector (1/(60+2)) and #1 in BM25 (1/(60+1)) -> 1/62 + 1/61 = 0.016129 + 0.016393 = 0.032522
        // doc_a is #1 in vector (1/(60+1)) and #3 in BM25 (1/(60+3)) -> 1/61 + 1/63 = 0.016393 + 0.015873 = 0.032266
        // doc_b should be #1!
        assert_eq!(fused[0].id, "doc_b");
        assert_eq!(fused[1].id, "doc_a");
        assert_eq!(fused.len(), 4);
    }
}
