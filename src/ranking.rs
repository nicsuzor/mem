//! TM2C2 normalised convex-combination fusion, raw-score floor, and ranking pipeline.
//!
//! Replaces RRF (Reciprocal Rank Fusion, k = 60) with:
//! 1. Candidate pool union: P = P_dense (top 100) union P_lex (top 100).
//! 2. Raw-score silence floor: if max_{d in P} s_raw^dense(q, d) < 0.45 and
//!    max_{d in P} s_raw^lex(q, d) == 0, return [] before min-max normalisation.
//! 3. Query-local min-max normalisation: s_norm^m(q, d) over P with epsilon 1e-9;
//!    when |P| == 1, set s_norm = 1.0 for the surviving modality.
//! 4. Adaptive convex combination (TM2C2):
//!    s_fused(q, d) = alpha(q) * s_norm^dense(q, d) + (1 - alpha(q)) * s_norm^lex(q, d)
//!    where alpha(q) = 0.35 when q contains an identifier-shaped token (_ or - or digit run
//!    or <= 3 tokens with uppercase/path chars) and 0.65 otherwise.
//! 5. Multiplicative structural priors:
//!    M_struct(q, d) = m_id(q, d) * m_status(d) * m_graph(d)
//!    where m_id = 2.0 on exact ID/filename/alias match (else 1.0),
//!    m_status in {1.05, 1.00, 0.85, 0.60, 0.30},
//!    m_graph(d) = 1 + 0.08 * min(1, ln(1 + deg+(d)) / ln(21)).
//! 6. Modular temporal/access multiplier hook: M_recency_access(d) (8A composition point).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::vectordb::SearchResult;

/// Silence floor cosine threshold (tau_cos = 0.45).
pub const SILENCE_FLOOR_COSINE: f32 = 0.45;

/// Candidate pool size for dense vector retrieval (K_vec = 100).
pub const POOL_SIZE_DENSE: usize = 100;

/// Candidate pool size for BM25 lexical retrieval (K_bm25 = 100).
pub const POOL_SIZE_LEX: usize = 100;

/// Convex combination alpha for prose queries.
pub const ALPHA_PROSE: f32 = 0.65;

/// Convex combination alpha for identifier-heavy queries.
pub const ALPHA_IDENTIFIER: f32 = 0.35;

/// Stability epsilon for min-max normalisation divisor.
pub const EPSILON: f32 = 1e-9;

/// Multiplier for exact ID, filename, or alias matches (m_id = 2.0).
pub const M_ID_EXACT_MATCH: f32 = 2.0;

/// Dominant anchor boost added in hybrid retrieval for exact ID matches (pinned to 1.000).
pub const EXACT_ID_HYBRID_BOOST: f32 = 1.0;

/// Default ID multiplier when no exact match (m_id = 1.0).
pub const M_ID_DEFAULT: f32 = 1.0;

/// Hook for modular temporal and access multipliers (e.g. 8A recency decay and recent-read boost).
pub type TemporalAccessHook = Arc<dyn Fn(&SearchResult) -> f32 + Send + Sync>;

/// Graph structural information lookup interface for ranking priors.
pub trait GraphStructuralLookup {
    fn outdegree(&self, _id: &str) -> i32 {
        0
    }
    fn aliases(&self, _id: &str) -> Vec<String> {
        Vec::new()
    }
}

impl GraphStructuralLookup for () {}

impl GraphStructuralLookup for crate::graph_store::GraphStore {
    fn outdegree(&self, id: &str) -> i32 {
        self.get_node(id).map(|n| n.outdegree).unwrap_or(0)
    }

    fn aliases(&self, id: &str) -> Vec<String> {
        self.get_node(id)
            .map(|n| {
                let mut out = n.permalinks.clone();
                if let Some(ref p) = n.permalink {
                    if !out.contains(p) {
                        out.push(p.clone());
                    }
                }
                if let Some(ref tid) = n.task_id {
                    if !out.contains(tid) {
                        out.push(tid.clone());
                    }
                }
                out
            })
            .unwrap_or_default()
    }
}

/// Ranking pipeline configuration.
#[derive(Debug, Clone)]
pub struct RankingConfig {
    pub pool_size_dense: usize,
    pub pool_size_lex: usize,
    pub silence_floor_cosine: f32,
    pub alpha_prose: f32,
    pub alpha_identifier: f32,
    pub epsilon: f32,
    pub m_id_exact: f32,
    pub rerank_top_n: usize,
}

impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            pool_size_dense: POOL_SIZE_DENSE,
            pool_size_lex: POOL_SIZE_LEX,
            silence_floor_cosine: SILENCE_FLOOR_COSINE,
            alpha_prose: ALPHA_PROSE,
            alpha_identifier: ALPHA_IDENTIFIER,
            epsilon: EPSILON,
            m_id_exact: M_ID_EXACT_MATCH,
            rerank_top_n: crate::rerank::DEFAULT_RERANK_TOP_N,
        }
    }
}

/// Returns true if query `q` contains an identifier-shaped token:
/// - any token contains `_` or `-` or a digit run (0-9)
/// - OR `tokens.len() <= 3` and `q` contains uppercase or path characters (`/` or `\`).
pub fn is_identifier_query(query: &str) -> bool {
    let clean = crate::bm25::strip_wikilink_brackets(query);
    let tokens: Vec<&str> = clean.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }

    for token in &tokens {
        if token.contains('_') || token.contains('-') || token.chars().any(|c| c.is_ascii_digit()) {
            return true;
        }
    }

    tokens.len() <= 3 && clean.chars().any(|c| c.is_ascii_uppercase() || c == '/' || c == '\\')
}

/// Compute adaptive convex combination alpha(q).
/// Returns 0.35 when `q` is identifier-shaped, 0.65 otherwise.
pub fn compute_alpha(query: &str, config: &RankingConfig) -> f32 {
    if is_identifier_query(query) {
        config.alpha_identifier
    } else {
        config.alpha_prose
    }
}

/// Returns true if a sub-token is identifier-shaped (`_`, `-`, or an ASCII digit).
fn is_identifier_token(token: &str) -> bool {
    token.contains('_') || token.contains('-') || token.chars().any(|c| c.is_ascii_digit())
}

/// Extract explicit wikilink targets (`[[target]]`, `[[target#heading]]`, `[[target|alias]]`) from a raw query.
fn extract_wikilink_targets(query: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = query;
    while let Some(start) = rest.find("[[") {
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("]]") else {
            break;
        };
        let inner = &after_open[..end];
        let target = inner.split(['|', '#']).next().unwrap_or("").trim();
        if !target.is_empty() {
            targets.push(target);
        }
        rest = &after_open[end + 2..];
    }
    targets
}

/// Returns true if `candidate` case-insensitively matches `doc_id`, optional `file_stem`, or any alias.
fn matches_doc_identity(
    candidate: &str,
    doc_id: &str,
    file_stem: Option<&str>,
    aliases: &[String],
) -> bool {
    if candidate.eq_ignore_ascii_case(doc_id) {
        return true;
    }
    if let Some(stem) = file_stem {
        if candidate.eq_ignore_ascii_case(stem) {
            return true;
        }
    }
    aliases
        .iter()
        .any(|alias| candidate.eq_ignore_ascii_case(alias.trim()))
}

/// Compute m_id(q, d): 2.0 on exact ID, filename, or alias match; otherwise 1.0.
///
/// Whole-query exact matches (against `doc_id`, `file_name`, `file_stem`, or `alias`)
/// always return `M_ID_EXACT_MATCH`. When the query contains multiple tokens, individual
/// sub-tokens only match `doc_id`, `file_stem`, or `alias` if explicitly wrapped as a
/// `[[wikilink]]` in the raw query or if identifier-shaped (containing `_`, `-`, or an ASCII digit).
pub fn compute_m_id(query: &str, doc_id: &str, path: &Path, aliases: &[String]) -> f32 {
    let clean = crate::bm25::strip_wikilink_brackets(query);
    let q = clean.trim();
    if q.is_empty() {
        return M_ID_DEFAULT;
    }

    let file_name = path.file_name().and_then(|n| n.to_str());
    let file_stem = path.file_stem().and_then(|s| s.to_str());

    if matches_doc_identity(q, doc_id, file_stem, aliases)
        || file_name.is_some_and(|name| q.eq_ignore_ascii_case(name))
    {
        return M_ID_EXACT_MATCH;
    }

    for target in extract_wikilink_targets(query) {
        if matches_doc_identity(target, doc_id, file_stem, aliases)
            || file_name.is_some_and(|name| target.eq_ignore_ascii_case(name))
        {
            return M_ID_EXACT_MATCH;
        }
    }

    // Check individual tokens; in multi-token queries, restrict sub-token identity
    // matching to identifier-shaped tokens so plain prose words cannot pin documents.
    let tokens: Vec<&str> = q
        .split(|c: char| {
            c.is_whitespace()
                || c == '['
                || c == ']'
                || c == '('
                || c == ')'
                || c == '{'
                || c == '}'
                || c == '<'
                || c == '>'
        })
        .map(|t| {
            t.trim_matches(|c: char| {
                c == '.' || c == ':' || c == ',' || c == ';' || c == '`' || c == '"' || c == '\''
            })
        })
        .filter(|t| !t.is_empty())
        .collect();

    let is_multi_token = tokens.len() > 1;
    for trimmed_tok in tokens {
        if is_multi_token && !is_identifier_token(trimmed_tok) {
            continue;
        }
        if matches_doc_identity(trimmed_tok, doc_id, file_stem, aliases) {
            return M_ID_EXACT_MATCH;
        }
    }

    M_ID_DEFAULT
}

/// Returns true if query contains an exact match for the document ID, file stem, or aliases.
pub fn is_exact_id_match(query: &str, doc_id: &str, path: &Path, aliases: &[String]) -> bool {
    compute_m_id(query, doc_id, path, aliases) > 1.0
}

/// Compute m_status(d) in {1.05, 1.00, 0.85, 0.60, 0.30}.
pub fn compute_m_status(status: Option<&str>) -> f32 {
    let s = match status {
        Some(s) => s.trim().to_lowercase(),
        None => return 1.00,
    };

    match s.as_str() {
        "ready" | "in_progress" | "active" => 1.05,
        "inbox" | "queued" => 1.00,
        "review" | "paused" | "someday" | "partial" => 0.85,
        "done" | "complete" | "completed" => 0.60,
        "cancelled" | "failed" | "archived" | "superseded" => 0.30,
        _ => 1.00,
    }
}

/// Compute m_graph(d) = 1 + 0.08 * min(1, ln(1 + deg+(d)) / ln(21)).
pub fn compute_m_graph(outdegree: i32) -> f32 {
    if outdegree <= 0 {
        return 1.0;
    }
    let deg_f = outdegree as f32;
    let ratio = (1.0 + deg_f).ln() / 21.0f32.ln();
    1.0 + 0.08 * ratio.min(1.0)
}

/// Compute bounded M_struct(q, d) = m_id(q, d) * m_status(d) * m_graph(d).
pub fn compute_m_struct(
    query: &str,
    doc_id: &str,
    path: &Path,
    aliases: &[String],
    status: Option<&str>,
    outdegree: i32,
) -> f32 {
    compute_m_id(query, doc_id, path, aliases)
        * compute_m_status(status)
        * compute_m_graph(outdegree)
}

#[derive(Debug)]
struct Candidate {
    result: SearchResult,
    s_raw_dense: f32,
    s_raw_lex: f32,
    in_dense: bool,
    in_lex: bool,
}

/// Fuse vector and BM25 search results using TM2C2 normalised convex combination,
/// raw-score silence floor, and structural priors, propagating any `RerankerError`.
pub fn try_fuse_tm2c2<G: GraphStructuralLookup>(
    vector_results: Vec<SearchResult>,
    bm25_results: Vec<SearchResult>,
    query: &str,
    config: &RankingConfig,
    reranker: Option<&crate::rerank::CrossEncoderReranker>,
    graph_lookup: &G,
    temporal_hook: Option<&TemporalAccessHook>,
    limit: usize,
) -> std::result::Result<Vec<SearchResult>, crate::rerank::RerankerError> {
    // 1. Candidate pool union: P = P_dense (top 100) union P_lex (top 100)
    let mut map: HashMap<String, Candidate> = HashMap::new();

    for r in vector_results {
        let score = r.score;
        map.insert(
            r.id.clone(),
            Candidate {
                result: r,
                s_raw_dense: score,
                s_raw_lex: 0.0,
                in_dense: true,
                in_lex: false,
            },
        );
    }

    for r in bm25_results {
        let score = r.score;
        if let Some(c) = map.get_mut(&r.id) {
            c.s_raw_lex = score;
            c.in_lex = true;
        } else {
            map.insert(
                r.id.clone(),
                Candidate {
                    result: r,
                    s_raw_dense: 0.0,
                    s_raw_lex: score,
                    in_dense: false,
                    in_lex: true,
                },
            );
        }
    }

    if map.is_empty() {
        return Ok(Vec::new());
    }

    // 2. Raw-score silence floor:
    // if max_{d in P} s_raw^dense(q, d) < 0.45 and max_{d in P} s_raw^lex(q, d) == 0, return []
    let max_raw_dense = map
        .values()
        .filter(|c| c.in_dense)
        .map(|c| c.s_raw_dense)
        .fold(f32::NEG_INFINITY, f32::max);

    let max_raw_lex = map
        .values()
        .filter(|c| c.in_lex)
        .map(|c| c.s_raw_lex)
        .fold(0.0f32, f32::max);

    if max_raw_dense < config.silence_floor_cosine && max_raw_lex <= 0.0 {
        return Ok(Vec::new());
    }

    let mut candidates: Vec<Candidate> = map.into_values().collect();
    let p_len = candidates.len();

    // 3. Query-local min-max normalisation & single-candidate rule
    let clean_query = crate::bm25::strip_wikilink_brackets(query);
    let alpha = compute_alpha(&clean_query, config);

    if p_len == 1 {
        let c = &mut candidates[0];
        let s_norm_dense = if c.in_dense && (c.s_raw_dense >= config.silence_floor_cosine || c.s_raw_dense > 0.0) {
            1.0
        } else {
            0.0
        };
        let s_norm_lex = if c.in_lex && c.s_raw_lex > 0.0 {
            1.0
        } else {
            0.0
        };
        let s_fused = if s_norm_dense > 0.0 && s_norm_lex > 0.0 {
            alpha * s_norm_dense + (1.0 - alpha) * s_norm_lex
        } else if s_norm_dense > 0.0 || s_norm_lex > 0.0 {
            1.0
        } else {
            0.0
        };
        c.result.score = s_fused;
    } else {
        let min_dense = candidates
            .iter()
            .map(|c| c.s_raw_dense)
            .fold(f32::INFINITY, f32::min);
        let max_dense = candidates
            .iter()
            .map(|c| c.s_raw_dense)
            .fold(f32::NEG_INFINITY, f32::max);
        let range_dense = max_dense - min_dense;

        let min_lex = candidates
            .iter()
            .map(|c| c.s_raw_lex)
            .fold(f32::INFINITY, f32::min);
        let max_lex = candidates
            .iter()
            .map(|c| c.s_raw_lex)
            .fold(f32::NEG_INFINITY, f32::max);
        let range_lex = max_lex - min_lex;

        for c in &mut candidates {
            let s_norm_dense = if range_dense.abs() < config.epsilon {
                if max_dense > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                ((c.s_raw_dense - min_dense) / (range_dense + config.epsilon)).clamp(0.0, 1.0)
            };

            let s_norm_lex = if range_lex.abs() < config.epsilon {
                if max_lex > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                ((c.s_raw_lex - min_lex) / (range_lex + config.epsilon)).clamp(0.0, 1.0)
            };

            c.result.score = alpha * s_norm_dense + (1.0 - alpha) * s_norm_lex;
        }
    }

    // 4. Extract results for optional reranker and post-fusion priors
    let mut intermediate: Vec<SearchResult> = candidates.into_iter().map(|c| c.result).collect();

    // Sort by fused score descending before reranking
    intermediate.sort_by(|a, b| {
        ordered_float::OrderedFloat(b.score)
            .cmp(&ordered_float::OrderedFloat(a.score))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });

    if let Some(r) = reranker {
        r.rerank(&clean_query, &mut intermediate, config.rerank_top_n)?;
    }

    // 5. Apply multiplicative structural priors M_struct(q, d), dominant anchor boost, and modular temporal hook
    for r in &mut intermediate {
        let outdegree = graph_lookup.outdegree(&r.id);
        let aliases = graph_lookup.aliases(&r.id);
        let m_struct = compute_m_struct(
            query,
            &r.id,
            &r.path,
            &aliases,
            r.status.as_deref(),
            outdegree,
        );
        let m_temporal = match temporal_hook {
            Some(hook) => hook(r),
            None => 1.0,
        };
        let is_exact = is_exact_id_match(query, &r.id, &r.path, &aliases);
        if is_exact {
            r.score = EXACT_ID_HYBRID_BOOST;
        } else {
            let s_boosted = (r.score * m_struct * m_temporal) as f64;
            r.score = crate::salience::soft_knee_ceiling(s_boosted) as f32;
        }
    }

    // 6. Final sort descending by score, breaking ties by document ID then chunk ID
    intermediate.sort_by(|a, b| {
        ordered_float::OrderedFloat(b.score)
            .cmp(&ordered_float::OrderedFloat(a.score))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });

    intermediate.truncate(limit);
    Ok(intermediate)
}

/// Weight applied to the overlap-coefficient ratio in [`title_match_boost`].
pub const TITLE_BOOST_WEIGHT: f32 = 2.0;

/// Minimal stopword list for title/query token overlap.
const TITLE_BOOST_STOPWORDS: &[&str] = &[
    "a", "an", "and", "at", "for", "in", "is", "of", "on", "or", "the", "to", "with",
];

/// Token-overlap boost rewarding queries that are a near-exact match to a
/// document's title.
pub fn title_match_boost(query: &str, title: &str) -> f32 {
    fn tokens(s: &str) -> HashSet<String> {
        s.to_ascii_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1 && !TITLE_BOOST_STOPWORDS.contains(w))
            .map(|w| w.to_string())
            .collect()
    }

    let q = tokens(query);
    let t = tokens(title);
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }

    let intersection = q.intersection(&t).count() as f32;
    let smaller = q.len().min(t.len()) as f32;

    (intersection / smaller) * TITLE_BOOST_WEIGHT
}

/// `type_boost` for `search`'s type-based ranking bump: surfaces
/// knowledge/memory/note/insight/observation docs over transient tasks,
/// absent a title match.
pub fn type_boost_for(doc_type: Option<&str>) -> f32 {
    match doc_type.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("knowledge") | Some("memory") | Some("note") | Some("insight")
        | Some("observation") => 0.15,
        _ => 0.0,
    }
}

/// Per-result base ranking score for `search`: fused/semantic `r.score`
/// rescaled by graph-proximity `boost`, `type_boost`, and `title_boost`.
pub fn ranked_score(query: &str, r: &SearchResult, boost: f32) -> f32 {
    let type_boost = type_boost_for(r.doc_type.as_deref());
    let title_boost = title_match_boost(query, &r.title);
    r.score * (1.0 + boost + type_boost + title_boost)
}

/// Per-result ranking score for `task_search` (no `type_boost`, shares `title_match_boost`).
pub fn task_search_score(query: &str, r: &SearchResult) -> f32 {
    r.score * (1.0 + title_match_boost(query, &r.title))
}

/// Full breakdown of post-hybrid scoring components for a candidate.
#[derive(Debug, Clone)]
pub struct PostHybridScore {
    pub node_id: String,
    pub type_boost: f32,
    pub title_boost: f32,
    pub proximity_boost: f32,
    pub base_score: f32,
    pub recency_decay: f64,
    pub read_boost: f64,
    pub salience_score: f64,
    pub is_exact_id: bool,
    pub final_score: f32,
}

/// Shared production post-hybrid scoring function used by both `handle_pkb_search`
/// and offline evaluation (`examples/rank_eval.rs`).
pub fn score_post_hybrid_candidate(
    query: &str,
    r: &SearchResult,
    graph: Option<&crate::graph_store::GraphStore>,
    boost_map: Option<&HashMap<String, f32>>,
    salience_store: Option<&crate::salience::SalienceStore>,
    now: chrono::DateTime<chrono::Utc>,
    floor: f64,
) -> PostHybridScore {
    let node = graph.and_then(|g| g.get_node(&r.id).or_else(|| g.resolve(&r.id)));
    let node_id = node
        .as_ref()
        .map(|n| n.id.clone())
        .unwrap_or_else(|| r.id.clone());

    let proximity_boost = boost_map
        .and_then(|m| m.get(&node_id).copied())
        .unwrap_or(0.0);
    let type_boost = type_boost_for(r.doc_type.as_deref());
    let title_boost = title_match_boost(query, &r.title);
    let base_score = r.score * (1.0 + proximity_boost + type_boost + title_boost);

    let doc_type = r
        .doc_type
        .as_deref()
        .or_else(|| node.as_ref().and_then(|n| n.node_type.as_deref()));
    let date = r.date.as_deref();
    let modified = node.as_ref().and_then(|n| n.modified.as_deref());

    let recency_decay = crate::salience::document_recency_decay(doc_type, date, modified, now);
    let salience_score = salience_store
        .map(|s| s.get_salience(&node_id))
        .unwrap_or(0.0);
    let default_config;
    let salience_config = match salience_store {
        Some(s) => s.config(),
        None => {
            default_config = crate::salience::SalienceConfig::default();
            &default_config
        }
    };
    let read_boost = crate::salience::recent_read_boost(salience_score, salience_config);

    let aliases = graph.map(|g| g.aliases(&r.id)).unwrap_or_default();
    let is_exact_id = is_exact_id_match(query, &r.id, &r.path, &aliases);

    let final_score = if is_exact_id {
        1.0f32
    } else {
        crate::salience::apply_priors_with_floor(
            base_score as f64,
            floor,
            recency_decay,
            read_boost,
        ) as f32
    };

    PostHybridScore {
        node_id,
        type_boost,
        title_boost,
        proximity_boost,
        base_score,
        recency_decay,
        read_boost,
        salience_score,
        is_exact_id,
        final_score,
    }
}

/// Fuse vector and BM25 search results using TM2C2 normalised convex combination,
/// raw-score silence floor, and structural priors.
pub fn fuse_tm2c2<G: GraphStructuralLookup>(
    vector_results: Vec<SearchResult>,
    bm25_results: Vec<SearchResult>,
    query: &str,
    config: &RankingConfig,
    reranker: Option<&crate::rerank::CrossEncoderReranker>,
    graph_lookup: &G,
    temporal_hook: Option<&TemporalAccessHook>,
    limit: usize,
) -> Vec<SearchResult> {
    try_fuse_tm2c2(
        vector_results,
        bm25_results,
        query,
        config,
        reranker,
        graph_lookup,
        temporal_hook,
        limit,
    )
    .expect("reranker failed in fuse_tm2c2; use try_fuse_tm2c2 to handle RerankerError")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_result(id: &str, title: &str, score: f32, status: Option<&str>) -> SearchResult {
        SearchResult {
            path: PathBuf::from(format!("docs/{id}.md")),
            title: title.to_string(),
            score,
            snippet: format!("Snippet for {title}"),
            chunk_text: format!("Chunk for {title}"),
            id: id.to_string(),
            date: None,
            doc_type: Some("task".to_string()),
            status: status.map(|s| s.to_string()),
            tags: vec![],
            confidence: None,
            chunk_id: None,
        }
    }

    // ── AC1: Silence floor tests ──

    #[test]
    fn test_silence_floor_on_garbage_queries_returns_empty() {
        let config = RankingConfig::default();
        let vec_res = vec![
            make_result("doc_1", "Low match", 0.32, None),
            make_result("doc_2", "Lower match", 0.21, None),
        ];
        let bm25_res = vec![];

        let fused = fuse_tm2c2(vec_res, bm25_res, "garbage search", &config, None, &(), None, 10);
        assert!(fused.is_empty(), "Garbage query with max_dense < 0.45 and max_lex == 0 must return empty");
    }

    #[test]
    fn test_silence_floor_dense_above_threshold_passes() {
        let config = RankingConfig::default();
        let vec_res = vec![make_result("doc_1", "Good match", 0.52, None)];
        let bm25_res = vec![];

        let fused = fuse_tm2c2(vec_res, bm25_res, "search query", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 1, "Dense match >= 0.45 must pass silence floor");
        assert_eq!(fused[0].id, "doc_1");
    }

    #[test]
    fn test_silence_floor_lex_above_threshold_passes() {
        let config = RankingConfig::default();
        let vec_res = vec![make_result("doc_1", "Low match", 0.20, None)];
        let bm25_res = vec![make_result("doc_2", "Lexical match", 5.2, None)];

        let fused = fuse_tm2c2(vec_res, bm25_res, "search query", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 2, "Positive lexical score must pass silence floor");
    }

    #[test]
    fn test_silence_floor_empty_input_returns_empty() {
        let config = RankingConfig::default();
        let fused = fuse_tm2c2(vec![], vec![], "anything", &config, None, &(), None, 10);
        assert!(fused.is_empty());
    }

    // ── AC2: Query-local min-max normalisation & |P| == 1 ──

    #[test]
    fn test_single_candidate_dense_surviving() {
        let config = RankingConfig::default();
        let vec_res = vec![make_result("doc_single", "Single Dense", 0.75, None)];
        let bm25_res = vec![];

        let fused = fuse_tm2c2(vec_res, bm25_res, "prose query for doc", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 1);
        // Single candidate with surviving dense modality sets s_norm = 1.0
        // s_fused = 1.0 (surviving modality), M_struct = 1.0, then soft-knee ceiling
        // A normalised 1.0 is a non-identity hit, so it passes through the
        // soft-knee ceiling (search.md §7.1): never 1.000.
        assert!((fused[0].score as f64 - crate::salience::soft_knee_ceiling(1.0)).abs() < 1e-4);
    }

    #[test]
    fn test_single_candidate_lex_surviving() {
        let config = RankingConfig::default();
        let vec_res = vec![];
        let bm25_res = vec![make_result("doc_single", "Single Lex", 14.5, None)];

        let fused = fuse_tm2c2(vec_res, bm25_res, "prose query for doc", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 1);
        // A normalised 1.0 is a non-identity hit, so it passes through the
        // soft-knee ceiling (search.md §7.1): never 1.000.
        assert!((fused[0].score as f64 - crate::salience::soft_knee_ceiling(1.0)).abs() < 1e-4);
    }

    #[test]
    fn test_single_candidate_both_surviving() {
        let config = RankingConfig::default();
        let vec_res = vec![make_result("doc_both", "Both Legs", 0.85, None)];
        let bm25_res = vec![make_result("doc_both", "Both Legs", 12.0, None)];

        let fused = fuse_tm2c2(vec_res, bm25_res, "prose query", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 1);
        // A normalised 1.0 is a non-identity hit, so it passes through the
        // soft-knee ceiling (search.md §7.1): never 1.000.
        assert!((fused[0].score as f64 - crate::salience::soft_knee_ceiling(1.0)).abs() < 1e-4);
    }

    #[test]
    fn test_query_local_min_max_multiple_candidates() {
        let config = RankingConfig::default();
        let vec_res = vec![
            make_result("doc_a", "Doc A", 0.90, None),
            make_result("doc_b", "Doc B", 0.50, None),
        ];
        let bm25_res = vec![
            make_result("doc_a", "Doc A", 10.0, None),
            make_result("doc_b", "Doc B", 2.0, None),
        ];

        let fused = fuse_tm2c2(vec_res, bm25_res, "test query", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 2);
        // doc_a has max in both (s_norm_dense=1.0, s_norm_lex=1.0) -> fused = 1.0 before the soft-knee ceiling
        // doc_b has min in both (s_norm_dense=0.0, s_norm_lex=0.0) -> fused = 0.0
        assert_eq!(fused[0].id, "doc_a");
        assert!((fused[0].score as f64 - crate::salience::soft_knee_ceiling(1.0)).abs() < 1e-3);
        assert_eq!(fused[1].id, "doc_b");
        assert!(fused[1].score < 1e-3);
    }

    // ── AC3: Adaptive convex combination alpha(q) ──

    #[test]
    fn test_alpha_routing_identifier_queries() {
        let config = RankingConfig::default();
        // Underscore token
        assert!(is_identifier_query("mem_p1_m1a"));
        assert_eq!(compute_alpha("mem_p1_m1a", &config), ALPHA_IDENTIFIER);

        // Hyphen token
        assert!(is_identifier_query("task-123"));
        assert_eq!(compute_alpha("task-123", &config), ALPHA_IDENTIFIER);

        // Digit run
        assert!(is_identifier_query("issue 42"));
        assert_eq!(compute_alpha("issue 42", &config), ALPHA_IDENTIFIER);

        // <= 3 tokens with path chars
        assert!(is_identifier_query("src/search.rs"));
        assert_eq!(compute_alpha("src/search.rs", &config), ALPHA_IDENTIFIER);

        // <= 3 tokens with uppercase
        assert!(is_identifier_query("CrossEncoder"));
        assert_eq!(compute_alpha("CrossEncoder", &config), ALPHA_IDENTIFIER);
    }

    #[test]
    fn test_alpha_routing_prose_queries() {
        let config = RankingConfig::default();
        // Prose queries
        assert!(!is_identifier_query("how does search ranking work"));
        assert_eq!(compute_alpha("how does search ranking work", &config), ALPHA_PROSE);

        // <= 3 tokens with all lowercase and no special chars
        assert!(!is_identifier_query("search ranking pipeline"));
        assert_eq!(compute_alpha("search ranking pipeline", &config), ALPHA_PROSE);
    }

    #[test]
    fn test_alpha_weighting_affects_fused_scores() {
        let config = RankingConfig::default();
        // doc_sem wins dense, doc_lex wins BM25
        let vec_res = vec![
            make_result("doc_sem", "Semantic Doc", 0.90, None),
            make_result("doc_lex", "Lexical Doc", 0.50, None),
        ];
        let bm25_res = vec![
            make_result("doc_lex", "Lexical Doc", 15.0, None),
            make_result("doc_sem", "Semantic Doc", 2.0, None),
        ];

        // 1. Prose query (alpha = 0.65 -> dense favored)
        let fused_prose = fuse_tm2c2(vec_res.clone(), bm25_res.clone(), "how to search documents", &config, None, &(), None, 10);
        assert_eq!(fused_prose[0].id, "doc_sem", "Prose query must favor dense modality");

        // 2. Identifier query (alpha = 0.35 -> lex favored)
        let fused_id = fuse_tm2c2(vec_res, bm25_res, "doc_lex_id", &config, None, &(), None, 10);
        assert_eq!(fused_id[0].id, "doc_lex", "Identifier query must favor lexical modality");
    }

    // ── AC4: Multiplicative structural priors M_struct ──

    #[test]
    fn test_m_id_exact_matches() {
        let path = PathBuf::from("tasks/mem_p1_m1a.md");
        let aliases = vec!["p1-m1a".to_string(), "ranking-core".to_string()];

        // Match exact ID
        assert_eq!(compute_m_id("mem_p1_m1a", "mem_p1_m1a", &path, &aliases), 2.0);
        // Case-insensitive ID match
        assert_eq!(compute_m_id("MEM_P1_M1A", "mem_p1_m1a", &path, &aliases), 2.0);
        // Match filename
        assert_eq!(compute_m_id("mem_p1_m1a.md", "other_id", &path, &aliases), 2.0);
        // Match filename stem
        assert_eq!(compute_m_id("mem_p1_m1a", "other_id", &path, &aliases), 2.0);
        // Match alias
        assert_eq!(compute_m_id("p1-m1a", "other_id", &path, &aliases), 2.0);
        // No match
        assert_eq!(compute_m_id("unrelated", "other_id", &path, &aliases), 1.0);
    }

    #[test]
    fn test_m_status_values() {
        assert_eq!(compute_m_status(Some("ready")), 1.05);
        assert_eq!(compute_m_status(Some("in_progress")), 1.05);
        assert_eq!(compute_m_status(Some("active")), 1.05);
        assert_eq!(compute_m_status(Some("inbox")), 1.00);
        assert_eq!(compute_m_status(Some("queued")), 1.00);
        assert_eq!(compute_m_status(None), 1.00);
        assert_eq!(compute_m_status(Some("review")), 0.85);
        assert_eq!(compute_m_status(Some("paused")), 0.85);
        assert_eq!(compute_m_status(Some("someday")), 0.85);
        assert_eq!(compute_m_status(Some("partial")), 0.85);
        assert_eq!(compute_m_status(Some("done")), 0.60);
        assert_eq!(compute_m_status(Some("completed")), 0.60);
        assert_eq!(compute_m_status(Some("cancelled")), 0.30);
        assert_eq!(compute_m_status(Some("failed")), 0.30);
        assert_eq!(compute_m_status(Some("archived")), 0.30);
        assert_eq!(compute_m_status(Some("superseded")), 0.30);
    }

    #[test]
    fn test_m_graph_outdegree_formula_and_bounds() {
        // deg+ = 0 -> 1.0
        assert_eq!(compute_m_graph(0), 1.0);
        // negative outdegree defaults to 1.0
        assert_eq!(compute_m_graph(-5), 1.0);
        // deg+ = 20 -> 1 + 0.08 * (ln(21)/ln(21)) = 1.08
        let m_20 = compute_m_graph(20);
        assert!((m_20 - 1.08).abs() < 1e-5);
        // deg+ > 20 -> capped at 1.08
        assert_eq!(compute_m_graph(100), 1.08);
        // deg+ = 1 -> strictly between 1.0 and 1.08
        let m_1 = compute_m_graph(1);
        assert!(m_1 > 1.0 && m_1 < 1.08);
    }

    #[test]
    fn test_m_struct_overall_bounds() {
        // Min possible M_struct = 1.0 (no id match) * 0.30 (cancelled) * 1.0 (deg+ 0) = 0.30
        let min_m_struct = compute_m_struct("query", "doc_x", &PathBuf::from("x.md"), &[], Some("cancelled"), 0);
        assert!((min_m_struct - 0.30).abs() < 1e-5);

        // Max possible M_struct = 2.0 (id match) * 1.05 (ready) * 1.08 (deg+ >= 20) = 2.268
        let max_m_struct = compute_m_struct("doc_x", "doc_x", &PathBuf::from("doc_x.md"), &[], Some("ready"), 25);
        assert!((max_m_struct - 2.268).abs() < 1e-4);

        assert!(min_m_struct >= 0.30);
        assert!(max_m_struct <= 2.268);
    }

    // ── Modular hook composition ──

    #[test]
    fn test_modular_temporal_access_hook() {
        let config = RankingConfig::default();
        let vec_res = vec![
            make_result("doc_boosted", "Boosted Doc", 0.80, None),
            make_result("doc_regular", "Regular Doc", 0.70, None),
        ];

        let hook: TemporalAccessHook = Arc::new(|r: &SearchResult| {
            if r.id == "doc_boosted" {
                1.5
            } else {
                1.0
            }
        });

        let fused = fuse_tm2c2(vec_res, vec![], "test search", &config, None, &(), Some(&hook), 10);
        assert_eq!(fused[0].id, "doc_boosted");
    }

    // ── Tie breaking ──

    #[test]
    fn test_tie_breaking_by_id() {
        let config = RankingConfig::default();
        let vec_res = vec![
            make_result("doc_z", "Title Z", 0.60, None),
            make_result("doc_a", "Title A", 0.60, None),
        ];

        let fused = fuse_tm2c2(vec_res, vec![], "test query", &config, None, &(), None, 10);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].id, "doc_a", "Equal score must break ties alphabetically by ID");
        assert_eq!(fused[1].id, "doc_z");
    }

    #[test]
    fn test_exact_id_dominant_anchor_in_hybrid_fusion() {
        let config = RankingConfig::default();
        // Target document whose title does not repeat the slug
        let target = make_result(
            "aops_8a2448c0",
            "Framework basics: Ida follow-ups, follow-through and the three fixes",
            0.15,
            Some("ready"),
        );
        // Competitor document with high vector similarity
        let competitor = make_result(
            "doc_semantic",
            "Highly related vector match for generic topic",
            0.95,
            Some("active"),
        );

        let vec_res = vec![competitor.clone(), target.clone()];
        let bm25_res = vec![
            make_result("aops_8a2448c0", target.title.as_str(), 1005.0, Some("ready")),
            make_result("doc_semantic", competitor.title.as_str(), 5.0, Some("active")),
        ];

        let fused = fuse_tm2c2(vec_res, bm25_res, "aops_8a2448c0", &config, None, &(), None, 10);
        assert!(!fused.is_empty());
        assert_eq!(fused[0].id, "aops_8a2448c0", "Exact ID match must be ranked #1 in hybrid retrieval");
        assert!(fused[0].score >= EXACT_ID_HYBRID_BOOST, "Exact ID match must receive dominant anchor boost");
        if fused.len() > 1 {
            assert!(fused[0].score > fused[1].score);
        }
    }

    #[test]
    fn test_wikilink_bracketed_exact_id_in_hybrid_fusion() {
        let config = RankingConfig::default();
        let target = make_result("kb_295732fd", "System Architecture", 0.10, Some("active"));
        let competitor = make_result("academicops_guide", "academicops overview", 0.90, Some("active"));

        let vec_res = vec![competitor.clone(), target.clone()];
        let bm25_res = vec![
            make_result("kb_295732fd", target.title.as_str(), 1005.0, Some("active")),
            make_result("academicops_guide", competitor.title.as_str(), 10.0, Some("active")),
        ];

        let fused = fuse_tm2c2(vec_res, bm25_res, "[[kb_295732fd]]", &config, None, &(), None, 10);
        assert!(!fused.is_empty());
        assert_eq!(fused[0].id, "kb_295732fd", "Bracketed ID query must rank target #1 in hybrid retrieval");
        assert!(fused[0].score >= EXACT_ID_HYBRID_BOOST);
    }

    #[test]
    fn test_exact_id_scores_one_prose_hits_sub_one() {
        // C2: Searching by exact ID returns score: 1.000. Searching topical prose returns top hits with score <= 0.990.
        let config = RankingConfig::default();
        let target = make_result("kb_pkb_search_ranking", "Search Ranking and Fusion", 0.10, Some("active"));
        let competitor = make_result("doc_semantic", "Search Ranking in Practice", 0.95, Some("active"));

        let vec_res = vec![competitor.clone(), target.clone()];
        let bm25_res = vec![
            make_result("kb_pkb_search_ranking", target.title.as_str(), 1005.0, Some("active")),
            make_result("doc_semantic", competitor.title.as_str(), 5.0, Some("active")),
        ];

        // 1. Exact ID query returns 1.000
        let fused_exact = fuse_tm2c2(vec_res.clone(), bm25_res.clone(), "kb_pkb_search_ranking", &config, None, &(), None, 10);
        assert!(!fused_exact.is_empty());
        assert_eq!(fused_exact[0].id, "kb_pkb_search_ranking");
        assert_eq!(fused_exact[0].score, 1.0, "Exact ID query must score exactly 1.000");

        // 2. Topical prose query returns <= 0.990
        let fused_prose = fuse_tm2c2(vec_res, bm25_res, "search ranking in distributed systems", &config, None, &(), None, 10);
        assert!(!fused_prose.is_empty());
        for hit in &fused_prose {
            assert!(hit.score <= 0.990, "Topical prose hits must be <= 0.990; got {}", hit.score);
        }
    }

    #[test]
    fn test_token_exact_id_match_in_multiword_query() {
        let path = PathBuf::from("tasks/aops_8a2448c0.md");
        let aliases: Vec<String> = vec![];
        assert_eq!(
            compute_m_id(
                "check task aops_8a2448c0 please",
                "aops_8a2448c0",
                &path,
                &aliases
            ),
            2.0
        );
        assert!(is_exact_id_match(
            "check task aops_8a2448c0 please",
            "aops_8a2448c0",
            &path,
            &aliases
        ));
        assert!(is_exact_id_match(
            "see [[kb_295732fd]] here",
            "kb_295732fd",
            &path,
            &aliases
        ));
    }

    #[test]
    fn test_multiword_prose_plain_word_does_not_pin_id() {
        let config = RankingConfig::default();
        let notes_path = PathBuf::from("docs/notes.md");
        let aliases = vec!["notes".to_string(), "my multi word alias".to_string()];

        // (a) Multi-word prose containing plain alphabetic words matching doc_id/stem/alias must NOT pin
        assert_eq!(
            compute_m_id(
                "prohibit long titles mobile import rename notes",
                "notes",
                &notes_path,
                &aliases
            ),
            1.0
        );
        assert!(!is_exact_id_match(
            "prohibit long titles mobile import rename notes",
            "notes",
            &notes_path,
            &aliases
        ));
        assert_eq!(
            compute_m_id(
                "check project research details",
                "research_doc_1",
                &PathBuf::from("docs/research.md"),
                &[]
            ),
            1.0
        );

        // In hybrid fusion, a plain-word stem match in multi-word prose must NOT receive score 1.000
        let incidental = make_result("notes", "General Notes Index", 0.45, Some("active"));
        let semantic = make_result(
            "mobile_import_guide",
            "Mobile Import Title Rules",
            0.95,
            Some("active"),
        );
        let vec_res = vec![semantic.clone(), incidental.clone()];
        let bm25_res = vec![
            make_result("notes", incidental.title.as_str(), 12.0, Some("active")),
            make_result(
                "mobile_import_guide",
                semantic.title.as_str(),
                10.0,
                Some("active"),
            ),
        ];
        let fused = fuse_tm2c2(
            vec_res,
            bm25_res,
            "prohibit long titles mobile import rename notes",
            &config,
            None,
            &(),
            None,
            10,
        );
        let notes_hit = fused.iter().find(|r| r.id == "notes").unwrap();
        assert!(
            notes_hit.score <= 0.990,
            "Plain prose word matching doc stem must not pin to 1.0; got {}",
            notes_hit.score
        );
        assert_eq!(fused[0].id, "mobile_import_guide");

        // (b) Single-token exact IDs, multi-word exact aliases, [[wikilink]] tokens,
        // and structured ID tokens inside multi-word queries MUST still get m_id = 2.0
        assert_eq!(compute_m_id("notes", "notes", &notes_path, &[]), 2.0);
        assert_eq!(
            compute_m_id("my multi word alias", "other_id", &notes_path, &aliases),
            2.0
        );
        assert_eq!(
            compute_m_id("see [[notes]] here", "notes", &notes_path, &[]),
            2.0
        );
        assert_eq!(
            compute_m_id("see [[notes|custom]] here", "notes", &notes_path, &[]),
            2.0
        );
        assert_eq!(
            compute_m_id(
                "inspect task_1234 and kb-abcd please",
                "task_1234",
                &PathBuf::from("tasks/task_1234.md"),
                &[]
            ),
            2.0
        );
        assert_eq!(
            compute_m_id(
                "inspect task_1234 and kb-abcd please",
                "kb-abcd",
                &PathBuf::from("docs/kb-abcd.md"),
                &[]
            ),
            2.0
        );

        // Bracketed wikilink on a plain word inside multi-word fuse_tm2c2 still pins to 1.0
        let fused_wikilink = fuse_tm2c2(
            vec![semantic, incidental.clone()],
            vec![make_result(
                "notes",
                incidental.title.as_str(),
                12.0,
                Some("active"),
            )],
            "see [[notes]] here",
            &config,
            None,
            &(),
            None,
            10,
        );
        assert_eq!(fused_wikilink[0].id, "notes");
        assert_eq!(fused_wikilink[0].score, 1.0);
    }

    #[test]
    fn test_score_post_hybrid_candidate_shared_logic() {
        use chrono::{TimeZone, Utc};

        let now = Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap();
        let mut memory_doc = make_result("mem_alpha", "Synthetic Memory Note", 0.50, Some("active"));
        memory_doc.doc_type = Some("memory".to_string());

        let mut task_doc = make_result("task_beta", "Unrelated Synthetic Item", 0.50, Some("ready"));
        task_doc.doc_type = Some("task".to_string());

        // Without title overlap, memory doc receives 0.15 type_boost over task doc.
        let s_mem = score_post_hybrid_candidate(
            "quarterly audit plan",
            &memory_doc,
            None,
            None,
            None,
            now,
            0.0,
        );
        let s_task = score_post_hybrid_candidate(
            "quarterly audit plan",
            &task_doc,
            None,
            None,
            None,
            now,
            0.0,
        );
        assert!((s_mem.type_boost - 0.15).abs() < 1e-6);
        assert_eq!(s_task.type_boost, 0.0);
        assert!(!s_mem.is_exact_id);
        assert!(s_mem.final_score > s_task.final_score);

        // Exact ID match pins final_score to 1.0 regardless of raw score.
        let s_exact = score_post_hybrid_candidate(
            "task_beta",
            &task_doc,
            None,
            None,
            None,
            now,
            0.0,
        );
        assert!(s_exact.is_exact_id);
        assert_eq!(s_exact.final_score, 1.0);

        // Title match boost applies on overlapping tokens.
        let s_title = score_post_hybrid_candidate(
            "synthetic memory note",
            &memory_doc,
            None,
            None,
            None,
            now,
            0.0,
        );
        assert!((s_title.title_boost - TITLE_BOOST_WEIGHT).abs() < 1e-6);
        assert!(s_title.final_score <= 0.990);
        assert!(s_title.final_score > s_mem.final_score);
    }
}
