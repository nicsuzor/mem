//! Search evaluation harness for PKB.
//!
//! Provides golden query test cases and metrics for comparing search quality
//! across different chunking strategies and configurations.

use crate::{embeddings, vectordb};

/// A golden query with expected results for evaluation.
#[derive(Debug, Clone)]
pub struct GoldenQuery {
    /// The search query
    pub query: &'static str,
    /// Document paths (relative) that MUST appear in top-k results
    pub expected_hits: Vec<&'static str>,
    /// Document paths that should NOT appear in top-k results
    pub expected_misses: Vec<&'static str>,
    /// Maximum acceptable rank for the best expected hit (1-indexed)
    pub max_rank: usize,
}

/// Evaluation result for a single query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub query: String,
    /// Rank of the best expected hit (1-indexed), None if not found
    pub best_hit_rank: Option<usize>,
    /// How many expected hits appeared in top-k
    pub hits_found: usize,
    pub hits_expected: usize,
    /// How many expected misses appeared in top-k (lower is better)
    pub false_positives: usize,
    /// Cosine similarity of the best result
    pub top_score: f32,
    /// Average snippet length (chars) in top results
    pub avg_snippet_len: f32,
    /// Reciprocal rank of first relevant result (for MRR calculation)
    pub reciprocal_rank: f32,
}

/// Aggregate evaluation metrics across all golden queries.
#[derive(Debug, Clone)]
pub struct EvalSummary {
    pub total_queries: usize,
    /// Mean Reciprocal Rank — average of 1/rank for first relevant result
    pub mrr: f32,
    /// Recall@k — fraction of expected hits found in top-k
    pub recall_at_k: f32,
    /// Precision — fraction of queries where best hit was in top-k
    pub precision: f32,
    /// Average score of top results
    pub avg_top_score: f32,
    /// Average snippet length
    pub avg_snippet_len: f32,
    /// Per-query results
    pub query_results: Vec<QueryResult>,
}

/// Run evaluation against a vector store with golden queries.
pub fn evaluate(
    store: &vectordb::VectorStore,
    embedder: &embeddings::Embedder,
    queries: &[GoldenQuery],
    pkb_root: &std::path::Path,
    k: usize,
) -> EvalSummary {
    let mut query_results = Vec::new();

    for gq in queries {
        let query_emb = match embedder.encode_query(gq.query) {
            Ok(emb) => emb,
            Err(e) => {
                eprintln!("Failed to encode query '{}': {e}", gq.query);
                continue;
            }
        };

        let results = store.search(&query_emb, k, pkb_root);

        // Find ranks of expected hits
        let mut best_hit_rank: Option<usize> = None;
        let mut hits_found = 0;

        for expected in &gq.expected_hits {
            for (rank, result) in results.iter().enumerate() {
                let rel_path = result
                    .path
                    .strip_prefix(pkb_root)
                    .unwrap_or(&result.path)
                    .to_string_lossy();
                if rel_path.contains(expected) {
                    hits_found += 1;
                    let rank_1 = rank + 1;
                    if best_hit_rank.is_none() || rank_1 < best_hit_rank.unwrap() {
                        best_hit_rank = Some(rank_1);
                    }
                    break;
                }
            }
        }

        // Count false positives (expected misses that appeared)
        let mut false_positives = 0;
        for miss in &gq.expected_misses {
            for result in &results {
                let rel_path = result
                    .path
                    .strip_prefix(pkb_root)
                    .unwrap_or(&result.path)
                    .to_string_lossy();
                if rel_path.contains(miss) {
                    false_positives += 1;
                    break;
                }
            }
        }

        let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
        let avg_snippet_len = if results.is_empty() {
            0.0
        } else {
            results.iter().map(|r| r.snippet.len() as f32).sum::<f32>() / results.len() as f32
        };
        let reciprocal_rank = best_hit_rank.map(|r| 1.0 / r as f32).unwrap_or(0.0);

        query_results.push(QueryResult {
            query: gq.query.to_string(),
            best_hit_rank,
            hits_found,
            hits_expected: gq.expected_hits.len(),
            false_positives,
            top_score,
            avg_snippet_len,
            reciprocal_rank,
        });
    }

    let n = query_results.len() as f32;
    let mrr = query_results.iter().map(|r| r.reciprocal_rank).sum::<f32>() / n;
    let recall_at_k = query_results
        .iter()
        .map(|r| r.hits_found as f32 / r.hits_expected.max(1) as f32)
        .sum::<f32>()
        / n;
    let precision = query_results
        .iter()
        .filter(|r| r.best_hit_rank.is_some())
        .count() as f32
        / n;
    let avg_top_score = query_results.iter().map(|r| r.top_score).sum::<f32>() / n;
    let avg_snippet_len = query_results.iter().map(|r| r.avg_snippet_len).sum::<f32>() / n;

    EvalSummary {
        total_queries: query_results.len(),
        mrr,
        recall_at_k,
        precision,
        avg_top_score,
        avg_snippet_len,
        query_results,
    }
}

/// Format evaluation summary as a human-readable report.
pub fn format_report(summary: &EvalSummary, label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== Search Evaluation: {label} ===\n"));
    out.push_str(&format!("Queries:        {}\n", summary.total_queries));
    out.push_str(&format!("MRR:            {:.3}\n", summary.mrr));
    out.push_str(&format!("Recall@k:       {:.3}\n", summary.recall_at_k));
    out.push_str(&format!("Precision:      {:.3}\n", summary.precision));
    out.push_str(&format!("Avg top score:  {:.4}\n", summary.avg_top_score));
    out.push_str(&format!("Avg snippet:    {:.0} chars\n", summary.avg_snippet_len));
    out.push_str("\nPer-query breakdown:\n");

    for qr in &summary.query_results {
        let rank_str = qr
            .best_hit_rank
            .map(|r| format!("#{r}"))
            .unwrap_or_else(|| "MISS".to_string());
        out.push_str(&format!(
            "  [{rank_str}] \"{query}\" — score={score:.4} hits={hits}/{expected} fp={fp}\n",
            query = qr.query,
            score = qr.top_score,
            hits = qr.hits_found,
            expected = qr.hits_expected,
            fp = qr.false_positives,
        ));
    }

    out
}

/// Compare two evaluation runs and produce a diff report.
pub fn format_comparison(baseline: &EvalSummary, experiment: &EvalSummary) -> String {
    let mut out = String::new();
    out.push_str("=== A/B Comparison ===\n");
    out.push_str(&format!(
        "MRR:       {:.3} → {:.3} ({:+.3})\n",
        baseline.mrr,
        experiment.mrr,
        experiment.mrr - baseline.mrr
    ));
    out.push_str(&format!(
        "Recall@k:  {:.3} → {:.3} ({:+.3})\n",
        baseline.recall_at_k,
        experiment.recall_at_k,
        experiment.recall_at_k - baseline.recall_at_k
    ));
    out.push_str(&format!(
        "Precision: {:.3} → {:.3} ({:+.3})\n",
        baseline.precision,
        experiment.precision,
        experiment.precision - baseline.precision
    ));
    out.push_str(&format!(
        "Avg score: {:.4} → {:.4} ({:+.4})\n",
        baseline.avg_top_score,
        experiment.avg_top_score,
        experiment.avg_top_score - baseline.avg_top_score
    ));

    // Per-query regressions/improvements
    out.push_str("\nPer-query changes:\n");
    for (b, e) in baseline.query_results.iter().zip(&experiment.query_results) {
        let b_rank = b.best_hit_rank.unwrap_or(999);
        let e_rank = e.best_hit_rank.unwrap_or(999);
        if b_rank != e_rank {
            let icon = if e_rank < b_rank { "↑" } else { "↓" };
            out.push_str(&format!(
                "  {icon} \"{}\" — rank {} → {}\n",
                e.query,
                b.best_hit_rank
                    .map(|r| format!("#{r}"))
                    .unwrap_or("MISS".into()),
                e.best_hit_rank
                    .map(|r| format!("#{r}"))
                    .unwrap_or("MISS".into()),
            ));
        }
    }

    out
}
