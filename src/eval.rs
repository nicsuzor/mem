//! Search evaluation harness for PKB.
//!
//! Provides golden query test cases and metrics for comparing search quality
//! across different chunking strategies and configurations.

use std::fmt::Write;

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
                    if best_hit_rank.map_or(true, |prev| rank_1 < prev) {
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

    let n = query_results.len();
    let (mrr, recall_at_k, precision, avg_top_score, avg_snippet_len) = if n == 0 {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let nf = n as f32;
        (
            query_results.iter().map(|r| r.reciprocal_rank).sum::<f32>() / nf,
            query_results
                .iter()
                .map(|r| r.hits_found as f32 / r.hits_expected.max(1) as f32)
                .sum::<f32>()
                / nf,
            query_results
                .iter()
                .filter(|r| r.best_hit_rank.is_some())
                .count() as f32
                / nf,
            query_results.iter().map(|r| r.top_score).sum::<f32>() / nf,
            query_results.iter().map(|r| r.avg_snippet_len).sum::<f32>() / nf,
        )
    };

    EvalSummary {
        total_queries: n,
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
    let _ = writeln!(out, "=== Search Evaluation: {label} ===");
    let _ = writeln!(out, "Queries:        {}", summary.total_queries);
    let _ = writeln!(out, "MRR:            {:.3}", summary.mrr);
    let _ = writeln!(out, "Recall@k:       {:.3}", summary.recall_at_k);
    let _ = writeln!(out, "Precision:      {:.3}", summary.precision);
    let _ = writeln!(out, "Avg top score:  {:.4}", summary.avg_top_score);
    let _ = writeln!(out, "Avg snippet:    {:.0} chars", summary.avg_snippet_len);
    let _ = writeln!(out, "\nPer-query breakdown:");

    for qr in &summary.query_results {
        let rank_str = qr
            .best_hit_rank
            .map(|r| format!("#{r}"))
            .unwrap_or_else(|| "MISS".to_string());
        let _ = writeln!(
            out,
            "  [{rank_str}] \"{query}\" — score={score:.4} hits={hits}/{expected} fp={fp}",
            query = qr.query,
            score = qr.top_score,
            hits = qr.hits_found,
            expected = qr.hits_expected,
            fp = qr.false_positives,
        );
    }

    out
}

/// Compare two evaluation runs and produce a diff report.
pub fn format_comparison(baseline: &EvalSummary, experiment: &EvalSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "=== A/B Comparison ===");
    let _ = writeln!(
        out,
        "MRR:       {:.3} → {:.3} ({:+.3})",
        baseline.mrr, experiment.mrr, experiment.mrr - baseline.mrr
    );
    let _ = writeln!(
        out,
        "Recall@k:  {:.3} → {:.3} ({:+.3})",
        baseline.recall_at_k, experiment.recall_at_k, experiment.recall_at_k - baseline.recall_at_k
    );
    let _ = writeln!(
        out,
        "Precision: {:.3} → {:.3} ({:+.3})",
        baseline.precision, experiment.precision, experiment.precision - baseline.precision
    );
    let _ = writeln!(
        out,
        "Avg score: {:.4} → {:.4} ({:+.4})",
        baseline.avg_top_score, experiment.avg_top_score,
        experiment.avg_top_score - baseline.avg_top_score
    );

    fn rank_label(r: Option<usize>) -> String {
        r.map(|r| format!("#{r}")).unwrap_or_else(|| "MISS".to_string())
    }

    let _ = writeln!(out, "\nPer-query changes:");
    for (b, e) in baseline.query_results.iter().zip(&experiment.query_results) {
        if b.best_hit_rank != e.best_hit_rank {
            let improved = e.best_hit_rank.unwrap_or(usize::MAX) < b.best_hit_rank.unwrap_or(usize::MAX);
            let icon = if improved { "↑" } else { "↓" };
            let _ = writeln!(
                out,
                "  {icon} \"{}\" — rank {} → {}",
                e.query,
                rank_label(b.best_hit_rank),
                rank_label(e.best_hit_rank),
            );
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_query_result(rank: Option<usize>, hits_found: usize, hits_expected: usize) -> QueryResult {
        QueryResult {
            query: format!("test query (rank={rank:?})"),
            best_hit_rank: rank,
            hits_found,
            hits_expected,
            false_positives: 0,
            top_score: 0.9,
            avg_snippet_len: 100.0,
            reciprocal_rank: rank.map(|r| 1.0 / r as f32).unwrap_or(0.0),
        }
    }

    fn make_summary(results: Vec<QueryResult>) -> EvalSummary {
        let n = results.len();
        if n == 0 {
            return EvalSummary {
                total_queries: 0,
                mrr: 0.0,
                recall_at_k: 0.0,
                precision: 0.0,
                avg_top_score: 0.0,
                avg_snippet_len: 0.0,
                query_results: vec![],
            };
        }
        let nf = n as f32;
        EvalSummary {
            total_queries: n,
            mrr: results.iter().map(|r| r.reciprocal_rank).sum::<f32>() / nf,
            recall_at_k: results
                .iter()
                .map(|r| r.hits_found as f32 / r.hits_expected.max(1) as f32)
                .sum::<f32>()
                / nf,
            precision: results.iter().filter(|r| r.best_hit_rank.is_some()).count() as f32 / nf,
            avg_top_score: 0.9,
            avg_snippet_len: 100.0,
            query_results: results,
        }
    }

    #[test]
    fn format_report_contains_metrics() {
        let summary = make_summary(vec![
            make_query_result(Some(1), 1, 1),
            make_query_result(Some(3), 1, 1),
        ]);
        let report = format_report(&summary, "test");
        assert!(report.contains("=== Search Evaluation: test ==="));
        assert!(report.contains("MRR:"));
        assert!(report.contains("Recall@k:"));
        assert!(report.contains("Precision:"));
        assert!(report.contains("[#1]"));
        assert!(report.contains("[#3]"));
    }

    #[test]
    fn format_report_shows_miss() {
        let summary = make_summary(vec![make_query_result(None, 0, 1)]);
        let report = format_report(&summary, "test");
        assert!(report.contains("[MISS]"));
    }

    #[test]
    fn format_comparison_shows_improvement_and_regression() {
        let baseline = make_summary(vec![
            make_query_result(Some(5), 1, 1),
            make_query_result(Some(1), 1, 1),
        ]);
        let experiment = make_summary(vec![
            make_query_result(Some(1), 1, 1),
            make_query_result(Some(3), 1, 1),
        ]);
        let report = format_comparison(&baseline, &experiment);
        assert!(report.contains("A/B Comparison"));
        assert!(report.contains("↑"), "should show improvement");
        assert!(report.contains("↓"), "should show regression");
    }

    #[test]
    fn format_comparison_unchanged_ranks() {
        let a = make_summary(vec![make_query_result(Some(1), 1, 1)]);
        let b = make_summary(vec![make_query_result(Some(1), 1, 1)]);
        let report = format_comparison(&a, &b);
        assert!(!report.contains("↑"));
        assert!(!report.contains("↓"));
    }

    #[test]
    fn empty_summary_no_panic() {
        let summary = make_summary(vec![]);
        let report = format_report(&summary, "empty");
        assert!(report.contains("Queries:        0"));
    }

    #[test]
    fn perfect_mrr_when_all_rank_one() {
        let summary = make_summary(vec![
            make_query_result(Some(1), 1, 1),
            make_query_result(Some(1), 1, 1),
        ]);
        assert!((summary.mrr - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zero_mrr_when_all_miss() {
        let summary = make_summary(vec![
            make_query_result(None, 0, 1),
            make_query_result(None, 0, 1),
        ]);
        assert!((summary.mrr - 0.0).abs() < f32::EPSILON);
        assert!((summary.precision - 0.0).abs() < f32::EPSILON);
    }
}
