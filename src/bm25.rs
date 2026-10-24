//! In-memory BM25 lexical search index for PKB (v2).
//!
//! Provides Okapi BM25 ranking over document titles, IDs, tags, headings, and body chunks.
//! Includes Unicode NFKD folding, lowercasing, English stopword filtering, Porter stemming,
//! chunk-level indexing, pre-pool gate predicate, and legacy tokenizer fallback behind `rank_v1`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// BM25 hyper-parameters.
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// Field weights for multi-field scoring.
pub const WEIGHT_ID: f32 = 5.0;
pub const WEIGHT_TITLE: f32 = 3.0;
pub const WEIGHT_ALIASES: f32 = 3.0;
pub const WEIGHT_TAGS: f32 = 2.0;
pub const WEIGHT_HEADING: f32 = 2.0;
pub const WEIGHT_BODY: f32 = 1.0;
pub const WEIGHT_ID_TOKENS: f32 = 1.0;

/// Standard English stopword list for lexical retrieval.
static ENGLISH_STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let words = [
        "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any", "are",
        "aren't", "arent", "as", "at", "be", "because", "been", "before", "being", "below", "between",
        "both", "but", "by", "can't", "cant", "cannot", "could", "couldn't", "couldnt", "did", "didn't",
        "didnt", "do", "does", "doesn't", "doesnt", "doing", "don't", "dont", "down", "during", "each",
        "few", "for", "from", "further", "had", "hadn't", "hadnt", "has", "hasn't", "hasnt", "have",
        "haven't", "havent", "having", "he", "he'd", "he'll", "he's", "her", "here", "here's", "hers",
        "herself", "him", "himself", "his", "how", "how's", "i", "i'd", "i'll", "i'm", "i've", "if",
        "in", "into", "is", "isn't", "isnt", "it", "it's", "its", "itself", "let's", "me", "more",
        "most", "mustn't", "mustnt", "my", "myself", "no", "nor", "not", "of", "off", "on", "once",
        "only", "or", "other", "ought", "our", "ours", "ourselves", "out", "over", "own", "same",
        "shan't", "shant", "she", "she'd", "she'll", "she's", "should", "shouldn't", "shouldnt", "so",
        "some", "such", "than", "that", "that's", "the", "their", "theirs", "them", "themselves", "then",
        "there", "there's", "these", "they", "they'd", "they'll", "they're", "they've", "this", "those",
        "through", "to", "too", "under", "until", "up", "very", "was", "wasn't", "wasnt", "we", "we'd",
        "we'll", "we're", "we've", "were", "weren't", "werent", "what", "what's", "when", "when's",
        "where", "where's", "which", "while", "who", "who's", "whom", "why", "why's", "with", "won't",
        "wont", "would", "wouldn't", "wouldnt", "you", "you'd", "you'll", "you're", "you've", "your",
        "yours", "yourself", "yourselves",
    ];
    words.into_iter().collect()
});

/// Returns true if the token is a standard English stopword.
pub fn is_stopword(word: &str) -> bool {
    ENGLISH_STOPWORDS.contains(word)
}

/// NFKD normalization and diacritic stripping.
///
/// Decomposes Unicode characters (e.g. 'á' -> 'a' + U+0301) and removes combining diacritical marks.
pub fn nfkd_fold(text: &str) -> String {
    text.nfkd().filter(|c| !is_combining_mark(*c)).collect()
}

fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// Porter stem a single word.
pub fn stem_word(word: &str) -> String {
    let stemmer = rust_stemmers::Stemmer::create(rust_stemmers::Algorithm::English);
    stemmer.stem(word).to_string()
}

/// Default tokenizer dispatcher.
///
/// If `MEM_RANK_V1` or `RANK_V1` environment variable is set to "1" or "true",
/// uses legacy v1 tokenizer. Otherwise uses v2 tokenizer.
pub fn tokenize(text: &str) -> Vec<String> {
    let rank_v1 = std::env::var("MEM_RANK_V1")
        .or_else(|_| std::env::var("RANK_V1"))
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if rank_v1 {
        tokenize_v1(text)
    } else {
        tokenize_v2(text)
    }
}

/// Legacy v1 tokenizer that preserves exact identifiers, compound tokens, and code symbols.
pub fn tokenize_v1(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let text_lower = text.to_lowercase();

    // Standard word splitter (splits on whitespace, quotes, parens, brackets, etc.)
    for raw in text_lower.split(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '\''
            || c == '`'
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
            || c == '{'
            || c == '}'
            || c == '<'
            || c == '>'
            || c == ','
            || c == ';'
            || c == '!'
            || c == '?'
    }) {
        let trimmed = raw.trim_matches(|c: char| c == '.' || c == ':' || c == '*' || c == '~');
        if trimmed.is_empty() {
            continue;
        }

        // Add the whole token (e.g. "mem_7cc72ed8", "#520", "task-parent", "aops_gpu", "1.4s")
        tokens.push(trimmed.to_string());

        // Also add without leading '#' if present (e.g. "#520" -> "520")
        if let Some(stripped) = trimmed.strip_prefix('#') {
            if !stripped.is_empty() {
                tokens.push(stripped.to_string());
            }
        }

        // Sub-token splitting on punctuation: '_', '-', '.', '/', ':', '@'
        let has_subsplit = trimmed.contains('_')
            || trimmed.contains('-')
            || trimmed.contains('.')
            || trimmed.contains('/')
            || trimmed.contains(':')
            || trimmed.contains('@');

        if has_subsplit {
            for sub in trimmed.split(|c: char| {
                c == '_' || c == '-' || c == '.' || c == '/' || c == ':' || c == '@'
            }) {
                let sub_trimmed = sub.trim();
                if !sub_trimmed.is_empty() && sub_trimmed.len() != trimmed.len() {
                    tokens.push(sub_trimmed.to_string());
                }
            }
        }
    }

    tokens
}

/// BM25 v2 tokenizer with NFKD folding, Unicode lowercasing, apostrophe preservation,
/// English stopword filtering, and Porter stemming.
pub fn tokenize_v2(text: &str) -> Vec<String> {
    let folded = nfkd_fold(text);
    let lower = folded.to_lowercase();
    let stemmer = rust_stemmers::Stemmer::create(rust_stemmers::Algorithm::English);
    let mut tokens = Vec::new();

    // Word splitter on whitespace, quotes, brackets, and punctuation (excluding apostrophes)
    for raw in lower.split(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '`'
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
            || c == '{'
            || c == '}'
            || c == '<'
            || c == '>'
            || c == ','
            || c == ';'
            || c == '!'
            || c == '?'
            || c == '*'
            || c == '~'
    }) {
        let trimmed = raw.trim_matches(|c: char| {
            c == '.' || c == ':' || c == '*' || c == '~' || c == '\'' || c == '\u{2019}'
        });
        if trimmed.is_empty() {
            continue;
        }

        // Handle internal apostrophes: clean out apostrophes without treating them as word delimiters
        // e.g., "user's" -> "users" (stems to "user"), "don't" -> "dont" (stopword), "o'connor" -> "oconnor"
        let clean_token: String = trimmed
            .chars()
            .filter(|&c| c != '\'' && c != '\u{2019}')
            .collect();
        if clean_token.is_empty() {
            continue;
        }

        add_v2_token(&clean_token, &stemmer, &mut tokens);

        if let Some(stripped) = clean_token.strip_prefix('#') {
            if !stripped.is_empty() {
                add_v2_token(stripped, &stemmer, &mut tokens);
            }
        }

        // Sub-token splitting on punctuation: '_', '-', '.', '/', ':', '@'
        let has_subsplit = clean_token.contains('_')
            || clean_token.contains('-')
            || clean_token.contains('.')
            || clean_token.contains('/')
            || clean_token.contains(':')
            || clean_token.contains('@');

        if has_subsplit {
            for sub in clean_token.split(|c: char| {
                c == '_' || c == '-' || c == '.' || c == '/' || c == ':' || c == '@'
            }) {
                let sub_trimmed = sub.trim();
                if !sub_trimmed.is_empty() && sub_trimmed.len() != clean_token.len() {
                    add_v2_token(sub_trimmed, &stemmer, &mut tokens);
                }
            }
        }
    }

    tokens
}

fn add_v2_token(token: &str, stemmer: &rust_stemmers::Stemmer, out: &mut Vec<String>) {
    if is_stopword(token) {
        return;
    }
    out.push(token.to_string());
    let stemmed = stemmer.stem(token).to_string();
    if stemmed != token && !is_stopword(&stemmed) {
        out.push(stemmed);
    }
}

/// A stored chunk entry in the BM25 index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Chunk {
    pub id: String,
    pub chunk_id: usize,
    pub path: PathBuf,
    pub title: String,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub modified: Option<String>,
    pub date: Option<String>,
    pub confidence: Option<f64>,
    pub snippet: String,
    pub chunk_text: String,
    pub heading: Option<String>,
    pub length: usize,
    /// Term frequencies: term -> weighted frequency
    pub term_freqs: HashMap<String, f32>,
}

/// BM25 inverted index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Index {
    /// Inverted index: term -> list of (doc_id, chunk_id, term_frequency)
    postings: HashMap<String, Vec<(String, usize, f32)>>,
    /// Chunks indexed by (doc_id, chunk_id)
    chunks: HashMap<(String, usize), Bm25Chunk>,
    /// Total document length across corpus
    total_length: usize,
    /// When true, use the legacy v1 tokenizer and scoring pipeline
    #[serde(default)]
    pub rank_v1: bool,
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Bm25Index {
    pub fn new() -> Self {
        let rank_v1 = std::env::var("MEM_RANK_V1")
            .or_else(|_| std::env::var("RANK_V1"))
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self::new_with_rank_v1(rank_v1)
    }

    pub fn new_with_rank_v1(rank_v1: bool) -> Self {
        Self {
            postings: HashMap::new(),
            chunks: HashMap::new(),
            total_length: 0,
            rank_v1,
        }
    }

    pub fn with_rank_v1(mut self, rank_v1: bool) -> Self {
        self.rank_v1 = rank_v1;
        self
    }

    pub fn set_rank_v1(&mut self, rank_v1: bool) {
        self.rank_v1 = rank_v1;
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn clear(&mut self) {
        self.postings.clear();
        self.chunks.clear();
        self.total_length = 0;
    }

    /// Index a document chunk.
    pub fn upsert_chunk(
        &mut self,
        id: &str,
        chunk_id: usize,
        path: PathBuf,
        title: &str,
        doc_type: Option<String>,
        status: Option<String>,
        tags: Vec<String>,
        modified: Option<String>,
        date: Option<String>,
        confidence: Option<f64>,
        snippet: String,
        chunk_text: String,
        heading: Option<&str>,
        aliases: &[String],
        body_text: &str,
    ) {
        // Remove prior chunk entry if it exists to maintain posting list integrity
        self.remove_chunk(id, chunk_id);

        let mut term_freqs: HashMap<String, f32> = HashMap::new();
        let mut doc_len = 0usize;

        let tokenizer = |text: &str| -> Vec<String> {
            if self.rank_v1 {
                tokenize_v1(text)
            } else {
                tokenize_v2(text)
            }
        };

        // 1. Exact ID token (highest weight)
        let id_clean = if self.rank_v1 {
            id.to_lowercase()
        } else {
            nfkd_fold(id).to_lowercase()
        };
        doc_len += 1;
        *term_freqs.entry(id_clean.clone()).or_insert(0.0) += WEIGHT_ID;

        // 2. ID sub-tokens (at body weight in v2; WEIGHT_ID in v1)
        let id_tokens = tokenizer(id);
        let id_token_weight = if self.rank_v1 {
            WEIGHT_ID
        } else {
            WEIGHT_ID_TOKENS
        };
        for t in id_tokens {
            if t != id_clean {
                doc_len += 1;
                *term_freqs.entry(t).or_insert(0.0) += id_token_weight;
            }
        }

        // 3. Title tokens
        let title_tokens = tokenizer(title);
        doc_len += title_tokens.len();
        for t in title_tokens {
            *term_freqs.entry(t).or_insert(0.0) += WEIGHT_TITLE;
        }

        // 4. Aliases tokens
        for alias in aliases {
            let alias_tokens = tokenizer(alias);
            doc_len += alias_tokens.len();
            for t in alias_tokens {
                *term_freqs.entry(t).or_insert(0.0) += WEIGHT_ALIASES;
            }
        }

        // 5. Tags tokens
        for tag in &tags {
            let tag_tokens = tokenizer(tag);
            doc_len += tag_tokens.len();
            for t in tag_tokens {
                *term_freqs.entry(t).or_insert(0.0) += WEIGHT_TAGS;
            }
        }

        // 6. Heading tokens
        if let Some(h) = heading {
            let heading_tokens = tokenizer(h);
            doc_len += heading_tokens.len();
            for t in heading_tokens {
                *term_freqs.entry(t).or_insert(0.0) += WEIGHT_HEADING;
            }
        }

        // 7. Body / chunk tokens
        let body_tokens = tokenizer(body_text);
        doc_len += body_tokens.len();
        for t in body_tokens {
            *term_freqs.entry(t).or_insert(0.0) += WEIGHT_BODY;
        }

        self.total_length += doc_len;

        // Update postings
        for (term, tf) in &term_freqs {
            self.postings
                .entry(term.clone())
                .or_default()
                .push((id.to_string(), chunk_id, *tf));
        }

        let chunk = Bm25Chunk {
            id: id.to_string(),
            chunk_id,
            path,
            title: title.to_string(),
            doc_type,
            status,
            tags,
            modified,
            date,
            confidence,
            snippet,
            chunk_text,
            heading: heading.map(String::from),
            length: doc_len.max(1),
            term_freqs,
        };

        self.chunks.insert((id.to_string(), chunk_id), chunk);
    }

    /// Document-level upsert: convenience wrapper that removes any prior chunks for `id`
    /// and indexes as chunk 0.
    pub fn upsert(
        &mut self,
        id: &str,
        path: PathBuf,
        title: &str,
        doc_type: Option<String>,
        status: Option<String>,
        tags: Vec<String>,
        modified: Option<String>,
        date: Option<String>,
        confidence: Option<f64>,
        snippet: String,
        chunk_text: String,
        body_text: &str,
    ) {
        self.remove(id);
        self.upsert_chunk(
            id,
            0,
            path,
            title,
            doc_type,
            status,
            tags,
            modified,
            date,
            confidence,
            snippet,
            chunk_text,
            None,
            &[],
            body_text,
        );
    }

    /// Update metadata across all chunks of a document without re-indexing text.
    pub fn update_metadata(
        &mut self,
        id: &str,
        title: &str,
        doc_type: Option<String>,
        status: Option<String>,
        tags: Vec<String>,
        modified: Option<String>,
        date: Option<String>,
        confidence: Option<f64>,
    ) {
        for ((doc_id, _), chunk) in self.chunks.iter_mut() {
            if doc_id == id {
                chunk.title = title.to_string();
                chunk.doc_type = doc_type.clone();
                chunk.status = status.clone();
                chunk.tags = tags.clone();
                chunk.modified = modified.clone();
                chunk.date = date.clone();
                chunk.confidence = confidence;
            }
        }
    }

    /// Remove all chunks for a given document ID.
    pub fn remove(&mut self, id: &str) -> bool {
        let keys_to_remove: Vec<(String, usize)> = self
            .chunks
            .keys()
            .filter(|(doc_id, _)| doc_id == id)
            .cloned()
            .collect();

        if keys_to_remove.is_empty() {
            return false;
        }

        for key in keys_to_remove {
            self.remove_chunk(&key.0, key.1);
        }
        true
    }

    /// Remove a specific chunk by document ID and chunk ID.
    pub fn remove_chunk(&mut self, id: &str, chunk_id: usize) -> bool {
        if let Some(old_chunk) = self.chunks.remove(&(id.to_string(), chunk_id)) {
            self.total_length = self.total_length.saturating_sub(old_chunk.length);
            for term in old_chunk.term_freqs.keys() {
                if let Some(list) = self.postings.get_mut(term) {
                    list.retain(|(doc_id, cid, _)| !(doc_id == id && *cid == chunk_id));
                }
            }
            true
        } else {
            false
        }
    }

    /// Compute BM25 scores for a query across all matching chunks.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        pkb_root: &Path,
        since: Option<&str>,
        before: Option<&str>,
        type_filter: Option<&str>,
        predicate: Option<&dyn Fn(&crate::vectordb::GateCandidate) -> bool>,
    ) -> Vec<crate::vectordb::SearchResult> {
        let n_chunks = self.chunks.len();
        if n_chunks == 0 {
            return Vec::new();
        }

        let avg_dl = (self.total_length as f32) / (n_chunks as f32);
        let query_tokens = if self.rank_v1 {
            tokenize_v1(query)
        } else {
            tokenize_v2(query)
        };
        if query_tokens.is_empty() {
            return Vec::new();
        }

        // Deduplicate query tokens while keeping counts
        let mut q_freqs: HashMap<String, usize> = HashMap::new();
        for t in query_tokens {
            *q_freqs.entry(t).or_insert(0) += 1;
        }

        // Accumulate scores per (doc_id, chunk_id)
        let mut scores: HashMap<(String, usize), f32> = HashMap::new();

        for (term, q_count) in q_freqs {
            if let Some(postings) = self.postings.get(&term) {
                let n_t = postings.len();
                // Standard smoothed BM25 IDF: ln(1 + (N - n_t + 0.5) / (n_t + 0.5))
                let idf = ((n_chunks as f32 - n_t as f32 + 0.5) / (n_t as f32 + 0.5) + 1.0).ln();
                if idf <= 0.0 {
                    continue;
                }

                let q_multiplier = if self.rank_v1 { q_count as f32 } else { 1.0 };

                for (doc_id, chunk_id, tf) in postings {
                    if let Some(chunk) = self.chunks.get(&(doc_id.clone(), *chunk_id)) {
                        let dl = chunk.length as f32;
                        let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avg_dl));
                        let term_score = idf * ((tf * (BM25_K1 + 1.0)) / denom) * q_multiplier;
                        *scores.entry((doc_id.clone(), *chunk_id)).or_insert(0.0) += term_score;
                    }
                }
            }
        }

        let mut results: Vec<crate::vectordb::SearchResult> = Vec::new();

        let date_filter = match crate::date_filter::DateFilter::parse(since, before) {
            Ok(df) => df,
            Err(_) => return Vec::new(),
        };

        for ((doc_id, chunk_id), score) in scores {
            let chunk = match self.chunks.get(&(doc_id, chunk_id)) {
                Some(c) => c,
                None => continue,
            };

            // Filter by document type
            if !crate::vectordb::matches_type_filter(chunk.doc_type.as_deref(), type_filter) {
                continue;
            }

            // Filter by modified date
            if date_filter.is_active() && !date_filter.matches(chunk.modified.as_deref()) {
                continue;
            }

            let abs_path = if chunk.path.is_absolute() {
                chunk.path.clone()
            } else {
                pkb_root.join(&chunk.path)
            };

            // Pre-pool gate predicate
            if let Some(pred) = predicate {
                let cand = crate::vectordb::GateCandidate {
                    id: &chunk.id,
                    path: &abs_path,
                    title: &chunk.title,
                    doc_type: chunk.doc_type.as_deref(),
                    status: chunk.status.as_deref(),
                    tags: &chunk.tags,
                    chunk_id: Some(chunk.chunk_id),
                };
                if !pred(&cand) {
                    continue;
                }
            }

            results.push(crate::vectordb::SearchResult {
                path: abs_path,
                title: chunk.title.clone(),
                score,
                snippet: chunk.snippet.clone(),
                chunk_text: chunk.chunk_text.clone(),
                id: chunk.id.clone(),
                date: chunk.date.clone(),
                doc_type: chunk.doc_type.clone(),
                status: chunk.status.clone(),
                tags: chunk.tags.clone(),
                confidence: chunk.confidence,
                chunk_id: Some(chunk.chunk_id),
            });
        }

        // Sort descending by BM25 score with deterministic tie-breaking by ID then chunk ID
        results.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score)
                .cmp(&ordered_float::OrderedFloat(a.score))
                .then_with(|| a.id.cmp(&b.id))
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });

        results.truncate(limit);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_preserves_identifiers_and_splits() {
        let tokens = tokenize("PR #520 fix(pkb): mem_7cc72ed8 AOPS_GPU task-parent");
        assert!(tokens.contains(&"pr".to_string()));
        assert!(tokens.contains(&"#520".to_string()) || tokens.contains(&"520".to_string()));
        assert!(tokens.contains(&"mem_7cc72ed8".to_string()));
        assert!(tokens.contains(&"mem".to_string()));
        assert!(tokens.contains(&"7cc72ed8".to_string()));
        assert!(tokens.contains(&"aops_gpu".to_string()));
        assert!(tokens.contains(&"aops".to_string()));
        assert!(tokens.contains(&"gpu".to_string()));
        assert!(tokens.contains(&"task-parent".to_string()));
    }

    #[test]
    fn test_bm25_exact_match_retrieval() {
        let mut idx = Bm25Index::new();
        idx.upsert(
            "mem_7cc72ed8",
            PathBuf::from("tasks/mem_7cc72ed8.md"),
            "PKB search: add a lexical (BM25) layer",
            Some("task".to_string()),
            Some("in_progress".to_string()),
            vec!["bm25".to_string(), "rrf".to_string()],
            Some("2026-08-31".to_string()),
            None,
            None,
            "Add a lexical (BM25) retrieval layer to PKB search".to_string(),
            "Add a lexical (BM25) retrieval layer to PKB search".to_string(),
            "Add a lexical (BM25) retrieval layer to PKB search and fuse with vector scan via RRF. PR #520.",
        );

        idx.upsert(
            "other_doc",
            PathBuf::from("notes/other.md"),
            "General architectural thoughts",
            Some("note".to_string()),
            Some("active".to_string()),
            vec!["architecture".to_string()],
            Some("2026-08-30".to_string()),
            None,
            None,
            "General note about system design".to_string(),
            "General note about system design".to_string(),
            "Nothing related to the specific query here.",
        );

        // Query by task id
        let results = idx.search("mem_7cc72ed8", 5, Path::new("/pkb"), None, None, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "mem_7cc72ed8");

        // Query by PR number
        let results_pr = idx.search("PR #520", 5, Path::new("/pkb"), None, None, None, None);
        assert_eq!(results_pr.len(), 1);
        assert_eq!(results_pr[0].id, "mem_7cc72ed8");
    }

    #[test]
    fn test_bm25_type_filter() {
        let mut idx = Bm25Index::new();
        idx.upsert(
            "doc1",
            PathBuf::from("tasks/task1.md"),
            "Search optimization",
            Some("task".to_string()),
            None,
            vec![],
            None,
            None,
            None,
            "".to_string(),
            "".to_string(),
            "content",
        );
        idx.upsert(
            "doc2",
            PathBuf::from("notes/note1.md"),
            "Search optimization note",
            Some("note".to_string()),
            None,
            vec![],
            None,
            None,
            None,
            "".to_string(),
            "".to_string(),
            "content",
        );

        let tasks = idx.search("optimization", 5, Path::new("/pkb"), None, None, Some("task"), None);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "doc1");

        let notes = idx.search("optimization", 5, Path::new("/pkb"), None, None, Some("!task"), None);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "doc2");
    }
}
