use mem::embeddings::Embedder;
use mem::eval::{evaluate_with_mode, EvalMode, GoldenQuery};
use mem::pkb::PkbDocument;
use mem::vectordb::VectorStore;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn make_doc(id: &str, title: &str, body: &str) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    fm.insert("title".to_string(), serde_json::Value::String(title.to_string()));

    PkbDocument {
        path: PathBuf::from(format!("tasks/{id}.md")),
        title: title.to_string(),
        tags: vec![],
        doc_type: Some("task".to_string()),
        status: Some("active".to_string()),
        consolidated: Some(false),
        consolidated_at: None,
        modified: Some("2026-08-31".to_string()),
        body: body.to_string(),
        content_hash: format!("chash_{id}"),
        file_hash: format!("hash_{id}"),
        frontmatter: Some(serde_json::Value::Object(fm)),
    }
}

#[test]
fn test_benchmark_latency_and_precision() {
    let mut store = VectorStore::new(3);
    let root = Path::new("/pkb");

    let docs = vec![
        make_doc("task-cbc9ee38", "BGE-M3 embedding model ONNX quantization", "BGE-M3 embedding ONNX quantization details"),
        make_doc("mem_7cc72ed8", "PKB search lexical BM25 layer", "Reference PR #520 and env var AOPS_MODEL_PATH"),
        make_doc("aops-1caa3b2f", "reindex startup performance timeout", "Reindex startup timeout fixes"),
        make_doc("mem-a54c550f", "TUI keyboard shortcut keybinding", "Crossterm keybindings in TUI"),
        make_doc("mem-7cbb684e", "claim task atomic locking concurrency", "Task claiming with atomic locking"),
    ];

    for doc in &docs {
        store.insert_precomputed(doc, vec![doc.body.clone()], vec![vec![0.5, 0.5, 0.5]]);
    }

    let queries = vec![
        GoldenQuery {
            query: "PR #520",
            expected_hits: vec!["mem_7cc72ed8"],
            expected_misses: vec![],
            max_rank: 1,
        },
        GoldenQuery {
            query: "AOPS_MODEL_PATH",
            expected_hits: vec!["mem_7cc72ed8"],
            expected_misses: vec![],
            max_rank: 1,
        },
        GoldenQuery {
            query: "BGE-M3 embedding model ONNX quantization",
            expected_hits: vec!["task-cbc9ee38"],
            expected_misses: vec![],
            max_rank: 1,
        },
        GoldenQuery {
            query: "TUI keyboard shortcut keybinding",
            expected_hits: vec!["mem-a54c550f"],
            expected_misses: vec![],
            max_rank: 1,
        },
        GoldenQuery {
            query: "claim task atomic locking concurrency",
            expected_hits: vec!["mem-7cbb684e"],
            expected_misses: vec![],
            max_rank: 1,
        },
    ];

    let embedder = Embedder::new_dummy();

    // 1. Measure Vector-Only
    let t0 = Instant::now();
    let res_vec = evaluate_with_mode(&store, &embedder, &queries, root, 5, EvalMode::VectorOnly);
    let dur_vec = t0.elapsed();

    // 2. Measure BM25-Only
    let t0 = Instant::now();
    let res_bm25 = evaluate_with_mode(&store, &embedder, &queries, root, 5, EvalMode::Bm25Only);
    let dur_bm25 = t0.elapsed();

    // 3. Measure Hybrid RRF
    let t0 = Instant::now();
    let res_hybrid = evaluate_with_mode(&store, &embedder, &queries, root, 5, EvalMode::Hybrid);
    let dur_hybrid = t0.elapsed();

    // 4. Measure Hybrid RRF + Rerank
    let t0 = Instant::now();
    let res_rerank = evaluate_with_mode(&store, &embedder, &queries, root, 5, EvalMode::HybridReranked);
    let dur_rerank = t0.elapsed();

    eprintln!("\n=== Benchmark Results ===");
    eprintln!("Vector-Only MRR: {:.3}, Precision: {:.3}, Duration: {:?}", res_vec.mrr, res_vec.precision, dur_vec);
    eprintln!("BM25-Only   MRR: {:.3}, Precision: {:.3}, Duration: {:?}", res_bm25.mrr, res_bm25.precision, dur_bm25);
    eprintln!("Hybrid RRF  MRR: {:.3}, Precision: {:.3}, Duration: {:?}", res_hybrid.mrr, res_hybrid.precision, dur_hybrid);
    eprintln!("Reranked    MRR: {:.3}, Precision: {:.3}, Duration: {:?}", res_rerank.mrr, res_rerank.precision, dur_rerank);

    assert_eq!(res_rerank.precision, 1.0);
    assert_eq!(res_rerank.mrr, 1.0);
}
