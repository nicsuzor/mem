//! Offline search ranking evaluation harness for PKB hybrid search.
//!
//! Loads an existing `VectorStore` snapshot strictly read-only, initializes the
//! production `Embedder` (with cross-encoder reranker disabled, matching
//! `reranker_loaded: false`), optionally loads `GraphStore` read-only from disk,
//! executes the exact production `handle_pkb_search` scoring sequence, and
//! emits aggregate retrieval metrics (`hit@5`, `MRR@10`, `nDCG@10`, `Judged@10`)
//! plus per-query, per-stage retrieval diagnostics.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use mem::embeddings::{Embedder, EMBEDDING_DIM};
use mem::graph_store::GraphStore;
use mem::ranking::{
    compute_alpha, compute_m_graph, compute_m_id, compute_m_status, compute_m_struct,
    is_exact_id_match, is_identifier_query, score_post_hybrid_candidate, GraphStructuralLookup,
    RankingConfig, POOL_SIZE_DENSE, POOL_SIZE_LEX,
};
use mem::rerank::{CrossEncoderReranker, RerankerConfig};
use mem::salience::document_recency_decay;
use mem::vectordb::{GateCandidate, SearchResult, VectorStore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "rank_eval",
    about = "Offline PKB search ranking evaluation harness"
)]
struct Args {
    /// Path to JSON array of evaluation queries and graded judgments.
    #[arg(long)]
    queries: PathBuf,

    /// Path to VectorStore snapshot (defaults to PKB_DB_PATH env var).
    #[arg(long, env = "PKB_DB_PATH")]
    db: PathBuf,

    /// Path to PKB markdown root for GraphStore (defaults to PKB_ROOT or ACA_DATA env var).
    #[arg(long, env = "PKB_ROOT")]
    pkb_root: Option<PathBuf>,

    /// Pinned timestamp (UNIX seconds or RFC3339) for deterministic recency decay.
    #[arg(long)]
    now: Option<String>,

    /// Result cutoff limit (default 10).
    #[arg(long, default_value_t = 10)]
    limit: usize,

    /// Document IDs to exclude from candidate scoring/ranking (repeatable).
    #[arg(long = "exclude-id")]
    exclude_ids: Vec<String>,

    /// Tags whose documents are excluded from candidate scoring/ranking (repeatable).
    #[arg(long = "exclude-tag")]
    exclude_tags: Vec<String>,

    /// Optional path to write full JSON diagnostic report.
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySpec {
    id: String,
    bucket: String,
    archetype: String,
    query: String,
    #[serde(default)]
    type_filter: Option<String>,
    judgments: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, Serialize)]
struct MetricSummary {
    count: usize,
    hit_at_5: f64,
    hit_at_5_g2: f64,
    hit_at_5_g3: f64,
    mrr_at_10: f64,
    ndcg_at_10: f64,
    judged_at_10: f64,
}

#[derive(Debug, Clone, Serialize)]
struct QueryMetrics {
    hit_at_5: f64,
    hit_at_5_g2: f64,
    hit_at_5_g3: f64,
    mrr_at_10: f64,
    ndcg_at_10: f64,
    judged_at_10: f64,
}

#[derive(Debug, Clone, Serialize)]
struct ExpectedDocDiagnostic {
    doc_id: String,
    canonical_id: String,
    grade: u8,
    exists_in_store: bool,
    exists_in_graph: bool,
    file_exists: bool,
    raw_dense_rank: Option<usize>,
    raw_dense_score: Option<f32>,
    in_p_dense_top100: bool,
    raw_lex_rank: Option<usize>,
    raw_lex_score: Option<f32>,
    in_p_lex_top100: bool,
    m_id: f32,
    m_id_prod: f32,
    m_status: f32,
    m_graph: f32,
    m_struct: f32,
    m_struct_prod: f32,
    fused_rank: Option<usize>,
    fused_score: Option<f32>,
    survived_fetch_limit: bool,
    type_boost: Option<f32>,
    title_boost: Option<f32>,
    recency_decay: Option<f64>,
    is_exact_id_match: Option<bool>,
    final_rank: Option<usize>,
    final_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
struct TopResultDiagnostic {
    rank: usize,
    id: String,
    doc_type: Option<String>,
    status: Option<String>,
    final_score: f32,
    m_id: f32,
    m_status: f32,
    judge_grade: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct QueryDiagnostic {
    id: String,
    bucket: String,
    archetype: String,
    query: String,
    type_filter: Option<String>,
    fetch_limit: usize,
    is_identifier_query: bool,
    alpha: f32,
    metrics: QueryMetrics,
    expected_docs: Vec<ExpectedDocDiagnostic>,
    top_10: Vec<TopResultDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
struct EvalReport {
    reranker_loaded: bool,
    now: String,
    limit: usize,
    store_documents: usize,
    graph_nodes: Option<usize>,
    by_bucket: BTreeMap<String, MetricSummary>,
    by_archetype: BTreeMap<String, MetricSummary>,
    queries: Vec<QueryDiagnostic>,
}

fn parse_now(raw: Option<&str>) -> Result<DateTime<Utc>> {
    match raw {
        None => Ok(Utc::now()),
        Some(s) => {
            let trimmed = s.trim();
            if let Ok(secs) = trimmed.parse::<i64>() {
                Utc.timestamp_opt(secs, 0)
                    .single()
                    .with_context(|| format!("Invalid UNIX timestamp: {trimmed}"))
            } else {
                let dt = DateTime::parse_from_rfc3339(trimmed).with_context(|| {
                    format!("Invalid --now timestamp (expected UNIX secs or RFC3339): {trimmed}")
                })?;
                Ok(dt.with_timezone(&Utc))
            }
        }
    }
}

fn resolve_doc_id(graph: Option<&GraphStore>, doc_id: &str) -> String {
    graph
        .and_then(|g| g.resolve(doc_id))
        .map(|n| n.id.clone())
        .unwrap_or_else(|| doc_id.to_string())
}

fn display_id_for(graph: Option<&GraphStore>, raw_id: &str) -> String {
    if let Some(g) = graph {
        if let Some(node) = g.get_node(raw_id).or_else(|| g.resolve(raw_id)) {
            return node.task_id.as_deref().unwrap_or(&node.id).to_string();
        }
    }
    raw_id.to_string()
}

fn match_judgment(
    r_id: &str,
    display_id: &str,
    judgments: &BTreeMap<String, u8>,
    resolved_judgment_map: &HashMap<String, (String, u8)>,
) -> Option<u8> {
    if let Some(&g) = judgments.get(r_id) {
        return Some(g);
    }
    if let Some(&g) = judgments.get(display_id) {
        return Some(g);
    }
    resolved_judgment_map.get(r_id).map(|(_, g)| *g)
}

fn compute_query_metrics(
    ranked_grades: &[Option<u8>],
    judgments: &BTreeMap<String, u8>,
) -> QueryMetrics {
    let hit_at_5 = if ranked_grades
        .iter()
        .take(5)
        .any(|g| g.is_some_and(|v| v >= 1))
    {
        1.0
    } else {
        0.0
    };
    let hit_at_5_g2 = if ranked_grades
        .iter()
        .take(5)
        .any(|g| g.is_some_and(|v| v >= 2))
    {
        1.0
    } else {
        0.0
    };
    let hit_at_5_g3 = if ranked_grades.iter().take(5).any(|g| g == &Some(3)) {
        1.0
    } else {
        0.0
    };

    let mrr_at_10 = ranked_grades
        .iter()
        .take(10)
        .position(|g| g.is_some_and(|v| v >= 1))
        .map(|idx| 1.0 / (idx + 1) as f64)
        .unwrap_or(0.0);

    let dcg_at_10: f64 = ranked_grades
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, g)| {
            let rel = g.unwrap_or(0) as f64;
            if rel > 0.0 {
                (2.0f64.powf(rel) - 1.0) / ((i + 2) as f64).log2()
            } else {
                0.0
            }
        })
        .sum();

    let mut ideal_grades: Vec<u8> = judgments.values().copied().filter(|&g| g > 0).collect();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));
    let idcg_at_10: f64 = ideal_grades
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, &g)| (2.0f64.powf(g as f64) - 1.0) / ((i + 2) as f64).log2())
        .sum();

    let ndcg_at_10 = if idcg_at_10 > 0.0 {
        (dcg_at_10 / idcg_at_10).min(1.0)
    } else {
        0.0
    };

    let top_10_len = ranked_grades.len().min(10);
    let judged_at_10 = if top_10_len == 0 {
        0.0
    } else {
        let judged_slots = ranked_grades
            .iter()
            .take(10)
            .filter(|g| g.is_some())
            .count();
        judged_slots as f64 / top_10_len as f64
    };

    QueryMetrics {
        hit_at_5,
        hit_at_5_g2,
        hit_at_5_g3,
        mrr_at_10,
        ndcg_at_10,
        judged_at_10,
    }
}

fn aggregate_metrics(items: &[&QueryMetrics]) -> MetricSummary {
    let count = items.len();
    if count == 0 {
        return MetricSummary {
            count: 0,
            hit_at_5: 0.0,
            hit_at_5_g2: 0.0,
            hit_at_5_g3: 0.0,
            mrr_at_10: 0.0,
            ndcg_at_10: 0.0,
            judged_at_10: 0.0,
        };
    }
    let n = count as f64;
    MetricSummary {
        count,
        hit_at_5: items.iter().map(|m| m.hit_at_5).sum::<f64>() / n,
        hit_at_5_g2: items.iter().map(|m| m.hit_at_5_g2).sum::<f64>() / n,
        hit_at_5_g3: items.iter().map(|m| m.hit_at_5_g3).sum::<f64>() / n,
        mrr_at_10: items.iter().map(|m| m.mrr_at_10).sum::<f64>() / n,
        ndcg_at_10: items.iter().map(|m| m.ndcg_at_10).sum::<f64>() / n,
        judged_at_10: items.iter().map(|m| m.judged_at_10).sum::<f64>() / n,
    }
}

fn is_excluded(
    id: &str,
    tags: &[String],
    graph: Option<&GraphStore>,
    exclude_ids: &HashSet<String>,
    exclude_tags: &HashSet<String>,
) -> bool {
    let lower_id = id.to_ascii_lowercase();
    if exclude_ids.contains(&lower_id) {
        return true;
    }
    if let Some(g) = graph {
        if let Some(node) = g.get_node(id).or_else(|| g.resolve(id)) {
            if exclude_ids.contains(&node.id.to_ascii_lowercase()) {
                return true;
            }
            if let Some(ref tid) = node.task_id {
                if exclude_ids.contains(&tid.to_ascii_lowercase()) {
                    return true;
                }
            }
        }
    }
    tags.iter()
        .any(|t| exclude_tags.contains(&t.to_ascii_lowercase()))
}

fn evaluate_query(
    q: &QuerySpec,
    store: &VectorStore,
    embedder: &Embedder,
    reranker: &CrossEncoderReranker,
    graph: Option<&GraphStore>,
    pkb_root: &Path,
    now: DateTime<Utc>,
    limit: usize,
    exclude_ids: &HashSet<String>,
    exclude_tags: &HashSet<String>,
) -> Result<QueryDiagnostic> {
    let query_embedding = embedder.encode_query(&q.query)?;
    let fetch_limit = (POOL_SIZE_DENSE + POOL_SIZE_LEX).max(if q.type_filter.is_some() {
        limit * 10
    } else {
        limit * 5
    });

    let exclude_pred = |cand: &GateCandidate<'_>| -> bool {
        !is_excluded(cand.id, cand.tags, graph, exclude_ids, exclude_tags)
    };

    let eval_config = RankingConfig {
        apply_type_boost: true,
        ..Default::default()
    };

    // Exact production `handle_pkb_search` single-pass call sequence.
    let results = match graph {
        Some(g) => store.try_search_hybrid_with_config(
            &q.query,
            &query_embedding,
            fetch_limit,
            pkb_root,
            None,
            None,
            q.type_filter.as_deref(),
            Some(reranker),
            Some(&exclude_pred),
            g,
            None,
            &eval_config,
        )?,
        None => store.try_search_hybrid_with_config(
            &q.query,
            &query_embedding,
            fetch_limit,
            pkb_root,
            None,
            None,
            q.type_filter.as_deref(),
            Some(reranker),
            Some(&exclude_pred),
            &(),
            None,
            &eval_config,
        )?,
    };

    let mut scored: Vec<_> = results
        .iter()
        .filter(|r| !is_excluded(&r.id, &r.tags, graph, exclude_ids, exclude_tags))
        .filter(|r| match graph {
            Some(g) => r.path.is_file() && g.resolve(&r.id).is_some(),
            None => true,
        })
        .collect();

    scored.sort_by(|a, b| {
        ordered_float::OrderedFloat(b.score)
            .cmp(&ordered_float::OrderedFloat(a.score))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    scored.truncate(limit);

    let mut resolved_judgment_map: HashMap<String, (String, u8)> = HashMap::new();
    for (jid, &grade) in &q.judgments {
        let canonical = resolve_doc_id(graph, jid);
        resolved_judgment_map.insert(canonical, (jid.clone(), grade));
    }

    let mut ranked_grades: Vec<Option<u8>> = Vec::with_capacity(scored.len());
    let mut top_10: Vec<TopResultDiagnostic> = Vec::with_capacity(scored.len().min(10));
    let mut final_rank_by_id: HashMap<String, (usize, f32)> = HashMap::new();

    for (idx, r) in scored.iter().enumerate() {
        let rank = idx + 1;
        let display_id = display_id_for(graph, &r.id);
        let grade = match_judgment(&r.id, &display_id, &q.judgments, &resolved_judgment_map);
        ranked_grades.push(grade);

        final_rank_by_id.insert(r.id.clone(), (rank, r.score));
        if display_id != r.id {
            final_rank_by_id.insert(display_id.clone(), (rank, r.score));
        }

        if rank <= 10 {
            let aliases = graph.map(|g| g.aliases(&r.id)).unwrap_or_default();
            let node_status = graph
                .and_then(|g| g.get_node(&r.id).or_else(|| g.resolve(&r.id)))
                .and_then(|n| n.status.clone())
                .or_else(|| r.status.clone());
            let node_type = r.doc_type.clone().or_else(|| {
                graph
                    .and_then(|g| g.get_node(&r.id).or_else(|| g.resolve(&r.id)))
                    .and_then(|n| n.node_type.clone())
            });

            top_10.push(TopResultDiagnostic {
                rank,
                id: display_id,
                doc_type: node_type,
                status: node_status,
                final_score: r.score,
                m_id: compute_m_id(&q.query, &r.id, &r.path, &aliases),
                m_status: compute_m_status(r.status.as_deref()),
                judge_grade: grade,
            });
        }
    }

    let metrics = compute_query_metrics(&ranked_grades, &q.judgments);

    // Stage diagnostics across the full dense, lexical, and fused pools.
    let clean_query = mem::bm25::strip_wikilink_brackets(&q.query);
    let config = RankingConfig::default();
    let is_id_q = is_identifier_query(&q.query);
    let alpha = compute_alpha(&q.query, &config);

    let full_dense = store.search(
        &query_embedding,
        store.len().max(1),
        pkb_root,
        None,
        None,
        q.type_filter.as_deref(),
        Some(&exclude_pred),
    );
    let full_lex = store.search_bm25(
        &clean_query,
        store.len().max(10_000),
        pkb_root,
        None,
        None,
        q.type_filter.as_deref(),
        Some(&exclude_pred),
    );

    let mut dense_pos: HashMap<String, (usize, f32)> = HashMap::with_capacity(full_dense.len());
    for (idx, r) in full_dense.iter().enumerate() {
        dense_pos.insert(r.id.clone(), (idx + 1, r.score));
    }

    let mut lex_pos: HashMap<String, (usize, f32)> = HashMap::with_capacity(full_lex.len());
    for (idx, r) in full_lex.iter().enumerate() {
        lex_pos.insert(r.id.clone(), (idx + 1, r.score));
    }

    let p_dense_pool: Vec<SearchResult> = full_dense
        .iter()
        .take(POOL_SIZE_DENSE.max(fetch_limit))
        .cloned()
        .collect();
    let p_lex_pool: Vec<SearchResult> = full_lex
        .iter()
        .take(POOL_SIZE_LEX.max(fetch_limit))
        .cloned()
        .collect();

    let fused_all = mem::ranking::try_fuse_tm2c2(
        p_dense_pool,
        p_lex_pool,
        &q.query,
        &config,
        Some(reranker),
        &(),
        None,
        usize::MAX,
    )?;

    let mut fused_pos: HashMap<String, (usize, &SearchResult)> =
        HashMap::with_capacity(fused_all.len());
    for (idx, r) in fused_all.iter().enumerate() {
        fused_pos.insert(r.id.clone(), (idx + 1, r));
    }

    let mut expected_docs = Vec::with_capacity(q.judgments.len());
    for (doc_id, &grade) in &q.judgments {
        let canonical_id = resolve_doc_id(graph, doc_id);
        let entry = store
            .get_entry(&canonical_id)
            .or_else(|| store.get_entry(doc_id));
        let graph_node =
            graph.and_then(|g| g.get_node(&canonical_id).or_else(|| g.resolve(doc_id)));

        let exists_in_store = entry.is_some();
        let exists_in_graph = graph_node.is_some();
        let file_exists = entry
            .map(|e| {
                let p = if e.path.is_absolute() {
                    e.path.clone()
                } else {
                    pkb_root.join(&e.path)
                };
                p.is_file()
            })
            .or_else(|| {
                graph_node.map(|n| {
                    let p = if n.path.is_absolute() {
                        n.path.clone()
                    } else {
                        pkb_root.join(&n.path)
                    };
                    p.is_file()
                })
            })
            .unwrap_or(false);

        let (raw_dense_rank, raw_dense_score) = dense_pos
            .get(&canonical_id)
            .or_else(|| dense_pos.get(doc_id))
            .copied()
            .map(|(rk, sc)| (Some(rk), Some(sc)))
            .unwrap_or((None, None));

        let (raw_lex_rank, raw_lex_score) = lex_pos
            .get(&canonical_id)
            .or_else(|| lex_pos.get(doc_id))
            .copied()
            .map(|(rk, sc)| (Some(rk), Some(sc)))
            .unwrap_or((None, None));

        let in_p_dense_top100 = raw_dense_rank.is_some_and(|rk| rk <= POOL_SIZE_DENSE);
        let in_p_lex_top100 = raw_lex_rank.is_some_and(|rk| rk <= POOL_SIZE_LEX);

        let doc_path = entry
            .map(|e| e.path.as_path())
            .or_else(|| graph_node.map(|n| n.path.as_path()))
            .unwrap_or_else(|| Path::new(""));
        let status = entry
            .and_then(|e| e.status.as_deref())
            .or_else(|| graph_node.and_then(|n| n.status.as_deref()));
        let outdegree = graph_node.map(|n| n.outdegree).unwrap_or(0);
        let aliases = graph.map(|g| g.aliases(&canonical_id)).unwrap_or_default();

        let m_id = compute_m_id(&q.query, &canonical_id, doc_path, &aliases);
        let m_id_prod = compute_m_id(&q.query, &canonical_id, doc_path, &[]);
        let m_status = compute_m_status(status);
        let m_graph = compute_m_graph(outdegree);
        let m_struct = compute_m_struct(
            &q.query,
            &canonical_id,
            doc_path,
            &aliases,
            status,
            outdegree,
        );
        let m_struct_prod = compute_m_struct(&q.query, &canonical_id, doc_path, &[], status, 0);

        let fused_hit = fused_pos
            .get(&canonical_id)
            .or_else(|| fused_pos.get(doc_id))
            .copied();
        let fused_rank = fused_hit.map(|(rk, _)| rk);
        let fused_score = fused_hit.map(|(_, r)| r.score);
        let survived_fetch_limit = results
            .iter()
            .any(|r| r.id == canonical_id || r.id == *doc_id);

        let (type_boost, title_boost, recency_decay, is_exact) = if let Some((_, r)) = fused_hit {
            let s = score_post_hybrid_candidate(&q.query, r, graph, None, None, now, 0.0);
            (
                Some(s.type_boost),
                Some(s.title_boost),
                Some(s.recency_decay),
                Some(s.is_exact_id),
            )
        } else if let Some(e) = entry {
            let tb = mem::ranking::type_boost_for(e.doc_type.as_deref());
            let title_b = mem::ranking::title_match_boost(&q.query, &e.title);
            let doc_type = e
                .doc_type
                .as_deref()
                .or_else(|| graph_node.and_then(|n| n.node_type.as_deref()));
            let modified = graph_node
                .and_then(|n| n.modified.as_deref())
                .or(e.modified.as_deref());
            let rd = document_recency_decay(doc_type, e.date.as_deref(), modified, now);
            let exact = is_exact_id_match(&q.query, &e.id, &e.path, &aliases);
            (Some(tb), Some(title_b), Some(rd), Some(exact))
        } else {
            (None, None, None, None)
        };

        let (final_rank, final_score) = final_rank_by_id
            .get(&canonical_id)
            .or_else(|| final_rank_by_id.get(doc_id))
            .copied()
            .map(|(rk, sc)| (Some(rk), Some(sc)))
            .unwrap_or((None, None));

        expected_docs.push(ExpectedDocDiagnostic {
            doc_id: doc_id.clone(),
            canonical_id,
            grade,
            exists_in_store,
            exists_in_graph,
            file_exists,
            raw_dense_rank,
            raw_dense_score,
            in_p_dense_top100,
            raw_lex_rank,
            raw_lex_score,
            in_p_lex_top100,
            m_id,
            m_id_prod,
            m_status,
            m_graph,
            m_struct,
            m_struct_prod,
            fused_rank,
            fused_score,
            survived_fetch_limit,
            type_boost,
            title_boost,
            recency_decay,
            is_exact_id_match: is_exact,
            final_rank,
            final_score,
        });
    }

    Ok(QueryDiagnostic {
        id: q.id.clone(),
        bucket: q.bucket.clone(),
        archetype: q.archetype.clone(),
        query: q.query.clone(),
        type_filter: q.type_filter.clone(),
        fetch_limit,
        is_identifier_query: is_id_q,
        alpha,
        metrics,
        expected_docs,
        top_10,
    })
}

fn print_summary_table(title: &str, rows: &BTreeMap<String, MetricSummary>) {
    println!("\n{title}");
    println!("-------------------------------------------------------------------------------------------");
    println!(
        "{:<26} {:>4} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9}",
        "Group", "N", "hit@5", "hit@5_g2", "hit@5_g3", "MRR@10", "nDCG@10", "Judged@10"
    );
    println!("-------------------------------------------------------------------------------------------");
    for (name, m) in rows {
        println!(
            "{:<26} {:>4} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>9.4}",
            name,
            m.count,
            m.hit_at_5,
            m.hit_at_5_g2,
            m.hit_at_5_g3,
            m.mrr_at_10,
            m.ndcg_at_10,
            m.judged_at_10
        );
    }
    println!("-------------------------------------------------------------------------------------------");
}

fn main() -> Result<()> {
    let args = Args::parse();
    let now = parse_now(args.now.as_deref())?;

    let queries_raw = std::fs::read_to_string(&args.queries)
        .with_context(|| format!("Failed to read queries file: {}", args.queries.display()))?;
    let queries: Vec<QuerySpec> = serde_json::from_str(&queries_raw)
        .with_context(|| format!("Failed to parse queries JSON: {}", args.queries.display()))?;
    if queries.is_empty() {
        bail!("Queries file is empty: {}", args.queries.display());
    }

    let store = VectorStore::load_read_only(&args.db, EMBEDDING_DIM)?;
    let embedder = Embedder::new()?;

    // Match production `reranker_loaded: false`.
    let reranker = CrossEncoderReranker::new(RerankerConfig {
        model_path: None,
        tokenizer_path: None,
        top_n_candidates: mem::rerank::DEFAULT_RERANK_TOP_N,
        query_token_cap: mem::rerank::DEFAULT_QUERY_TOKEN_CAP,
        chunk_token_cap: mem::rerank::DEFAULT_CHUNK_TOKEN_CAP,
        enabled: false,
        tau_h: mem::rerank::DEFAULT_TAU_H,
        tau_cos: mem::rerank::DEFAULT_TAU_COS,
        tau_s: mem::rerank::DEFAULT_TAU_S,
    });
    let reranker_loaded = reranker.is_model_loaded();
    if reranker_loaded {
        bail!("Expected reranker_loaded == false for baseline production configuration");
    }

    let resolved_pkb_root = args
        .pkb_root
        .clone()
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            ["PKB_ROOT", "ACA_DATA"].into_iter().find_map(|key| {
                std::env::var_os(key)
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
        });
    let (graph, pkb_root): (Option<GraphStore>, PathBuf) = match resolved_pkb_root {
        Some(root_path) => {
            if !root_path.is_dir() {
                bail!(
                    "--pkb-root directory does not exist: {}",
                    root_path.display()
                );
            }
            (
                Some(GraphStore::build_from_directory(&root_path)),
                root_path,
            )
        }
        None => (None, PathBuf::from(".")),
    };

    let exclude_ids: HashSet<String> = args
        .exclude_ids
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let exclude_tags: HashSet<String> = args
        .exclude_tags
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut query_diags = Vec::with_capacity(queries.len());
    for q in &queries {
        let diag = evaluate_query(
            q,
            &store,
            &embedder,
            &reranker,
            graph.as_ref(),
            &pkb_root,
            now,
            args.limit,
            &exclude_ids,
            &exclude_tags,
        )?;
        query_diags.push(diag);
    }

    let mut by_bucket: BTreeMap<String, MetricSummary> = BTreeMap::new();
    let all_metrics: Vec<&QueryMetrics> = query_diags.iter().map(|d| &d.metrics).collect();
    by_bucket.insert("all".to_string(), aggregate_metrics(&all_metrics));

    let mut bucket_groups: BTreeMap<String, Vec<&QueryMetrics>> = BTreeMap::new();
    let mut arch_groups: BTreeMap<String, Vec<&QueryMetrics>> = BTreeMap::new();
    for d in &query_diags {
        bucket_groups
            .entry(d.bucket.clone())
            .or_default()
            .push(&d.metrics);
        arch_groups
            .entry(d.archetype.clone())
            .or_default()
            .push(&d.metrics);
    }
    for (b, ms) in bucket_groups {
        by_bucket.insert(b, aggregate_metrics(&ms));
    }

    let mut by_archetype: BTreeMap<String, MetricSummary> = BTreeMap::new();
    for (a, ms) in arch_groups {
        by_archetype.insert(a, aggregate_metrics(&ms));
    }

    let graph_nodes_display = graph
        .as_ref()
        .map(|g| g.node_count().to_string())
        .unwrap_or_else(|| "none".to_string());
    println!(
        "rank_eval: store_docs={} graph_nodes={} reranker_loaded={} now={} limit={}",
        store.len(),
        graph_nodes_display,
        reranker_loaded,
        now.to_rfc3339(),
        args.limit
    );
    print_summary_table("Metrics by Bucket", &by_bucket);
    print_summary_table("Metrics by Archetype", &by_archetype);

    println!("\nPer-Query Summary:");
    for d in &query_diags {
        let best_rank = d
            .expected_docs
            .iter()
            .filter_map(|e| e.final_rank.map(|r| (r, e.grade)))
            .min_by_key(|x| x.0)
            .map(|(r, g)| format!("{r} (g={g})"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  {:<32} [{:<5} | {:<22}] id_q={:<5} alpha={:.2} hit@5={:.0} MRR={:.3} nDCG={:.3} J@10={:.2} best_rank={}",
            d.id,
            d.bucket,
            d.archetype,
            d.is_identifier_query,
            d.alpha,
            d.metrics.hit_at_5,
            d.metrics.mrr_at_10,
            d.metrics.ndcg_at_10,
            d.metrics.judged_at_10,
            best_rank
        );
    }

    if let Some(json_out) = &args.json {
        if let Some(parent) = json_out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let report = EvalReport {
            reranker_loaded,
            now: now.to_rfc3339(),
            limit: args.limit,
            store_documents: store.len(),
            graph_nodes: graph.as_ref().map(|g| g.node_count()),
            by_bucket,
            by_archetype,
            queries: query_diags,
        };
        let serialized = serde_json::to_string_pretty(&report)?;
        std::fs::write(json_out, serialized)?;
        println!("\nWrote JSON diagnostic report to {}", json_out.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_query_metrics_exact_top1_and_graded() {
        let mut judgments = BTreeMap::new();
        judgments.insert("doc_a".to_string(), 3);
        judgments.insert("doc_b".to_string(), 2);
        judgments.insert("doc_c".to_string(), 1);

        // Ideal ordering at top 3: grades [3, 2, 1, None, None]
        let ranked = vec![Some(3), Some(2), Some(1), None, None];
        let m = compute_query_metrics(&ranked, &judgments);
        assert_eq!(m.hit_at_5, 1.0);
        assert_eq!(m.hit_at_5_g2, 1.0);
        assert_eq!(m.hit_at_5_g3, 1.0);
        assert_eq!(m.mrr_at_10, 1.0);
        assert!((m.ndcg_at_10 - 1.0).abs() < 1e-9);
        assert!((m.judged_at_10 - 0.6).abs() < 1e-9); // 3 / 5 returned slots
    }

    #[test]
    fn test_compute_query_metrics_rank_6_misses_hit_at_5() {
        let mut judgments = BTreeMap::new();
        judgments.insert("doc_a".to_string(), 3);

        // First relevant hit is at rank 6 (index 5) across 10 returned slots.
        let mut ranked = vec![None; 10];
        ranked[5] = Some(3);
        let m = compute_query_metrics(&ranked, &judgments);
        assert_eq!(m.hit_at_5, 0.0);
        assert_eq!(m.hit_at_5_g2, 0.0);
        assert_eq!(m.hit_at_5_g3, 0.0);
        assert!((m.mrr_at_10 - (1.0 / 6.0)).abs() < 1e-9);
        assert!((m.judged_at_10 - 0.1).abs() < 1e-9);
        // DCG@10 = (2^3 - 1) / log2(7); IDCG@10 = (2^3 - 1) / log2(2)
        let expected_ndcg = (1.0 / 7.0f64.log2()) / (1.0 / 2.0f64.log2());
        assert!((m.ndcg_at_10 - expected_ndcg).abs() < 1e-9);
    }

    #[test]
    fn test_compute_query_metrics_grade1_at_rank2() {
        let mut judgments = BTreeMap::new();
        judgments.insert("doc_a".to_string(), 3);
        judgments.insert("doc_b".to_string(), 1);

        let ranked = vec![None, Some(1), None, None, None];
        let m = compute_query_metrics(&ranked, &judgments);
        assert_eq!(m.hit_at_5, 1.0);
        assert_eq!(m.hit_at_5_g2, 0.0);
        assert_eq!(m.hit_at_5_g3, 0.0);
        assert!((m.mrr_at_10 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_compute_query_metrics_empty_results() {
        let mut judgments = BTreeMap::new();
        judgments.insert("doc_a".to_string(), 3);
        let m = compute_query_metrics(&[], &judgments);
        assert_eq!(m.hit_at_5, 0.0);
        assert_eq!(m.mrr_at_10, 0.0);
        assert_eq!(m.ndcg_at_10, 0.0);
        assert_eq!(m.judged_at_10, 0.0);
    }

    #[test]
    fn test_aggregate_metrics_averages_correctly() {
        let m1 = QueryMetrics {
            hit_at_5: 1.0,
            hit_at_5_g2: 1.0,
            hit_at_5_g3: 1.0,
            mrr_at_10: 1.0,
            ndcg_at_10: 0.8,
            judged_at_10: 0.4,
        };
        let m2 = QueryMetrics {
            hit_at_5: 0.0,
            hit_at_5_g2: 0.0,
            hit_at_5_g3: 0.0,
            mrr_at_10: 0.25,
            ndcg_at_10: 0.2,
            judged_at_10: 0.2,
        };
        let agg = aggregate_metrics(&[&m1, &m2]);
        assert_eq!(agg.count, 2);
        assert!((agg.hit_at_5 - 0.5).abs() < 1e-9);
        assert!((agg.mrr_at_10 - 0.625).abs() < 1e-9);
        assert!((agg.ndcg_at_10 - 0.5).abs() < 1e-9);
        assert!((agg.judged_at_10 - 0.3).abs() < 1e-9);
    }

    #[test]
    fn test_parse_now_accepts_unix_and_rfc3339() {
        let dt1 = parse_now(Some("1794995648")).unwrap();
        assert_eq!(dt1.timestamp(), 1794995648);

        let dt2 = parse_now(Some("2026-11-18T09:54:08Z")).unwrap();
        assert_eq!(dt2.timestamp(), 1794995648);

        assert!(parse_now(Some("not-a-timestamp")).is_err());
    }
}
