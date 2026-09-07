//! In-memory BM25 lexical search index for PKB.
//!
//! Provides Okapi BM25 ranking over document titles, IDs, tags, and body chunks.
//! Specifically optimized for exact terminology retrieval: PR numbers, env vars,
//! function names, task IDs, and code identifiers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// BM25 hyper-parameters.
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// Field weights for multi-field scoring.
const WEIGHT_ID: f32 = 5.0;
const WEIGHT_TITLE: f32 = 3.0;
const WEIGHT_TAGS: f32 = 2.0;
const WEIGHT_BODY: f32 = 1.0;

/// Tokenizer that preserves exact identifiers, compound tokens, and code symbols.
pub fn tokenize(text: &str) -> Vec<String> {
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

/// A stored document entry in the BM25 index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Document {
    pub id: String,
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
    pub length: usize,
    /// Term frequencies: term -> weighted frequency
    pub term_freqs: HashMap<String, f32>,
}

/// BM25 inverted index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Index {
    /// Inverted index: term -> list of (doc_id, term_frequency)
    postings: HashMap<String, Vec<(String, f32)>>,
    /// Documents by ID
    documents: HashMap<String, Bm25Document>,
    /// Total document length across corpus
    total_length: usize,
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Bm25Index {
    pub fn new() -> Self {
        Self {
            postings: HashMap::new(),
            documents: HashMap::new(),
            total_length: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn clear(&mut self) {
        self.postings.clear();
        self.documents.clear();
        self.total_length = 0;
    }

    /// Index a document or replace an existing one.
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
        // Remove prior entry if it exists to maintain posting list integrity
        self.remove(id);

        let mut term_freqs: HashMap<String, f32> = HashMap::new();
        let mut doc_len = 0usize;

        // 1. ID tokens (highest weight)
        let id_tokens = tokenize(id);
        doc_len += id_tokens.len();
        for t in id_tokens {
            *term_freqs.entry(t).or_insert(0.0) += WEIGHT_ID;
        }

        // 2. Title tokens (high weight)
        let title_tokens = tokenize(title);
        doc_len += title_tokens.len();
        for t in title_tokens {
            *term_freqs.entry(t).or_insert(0.0) += WEIGHT_TITLE;
        }

        // 3. Tags tokens
        for tag in &tags {
            let tag_tokens = tokenize(tag);
            doc_len += tag_tokens.len();
            for t in tag_tokens {
                *term_freqs.entry(t).or_insert(0.0) += WEIGHT_TAGS;
            }
        }

        // 4. Body / chunk tokens
        let body_tokens = tokenize(body_text);
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
                .push((id.to_string(), *tf));
        }

        let doc = Bm25Document {
            id: id.to_string(),
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
            length: doc_len.max(1),
            term_freqs,
        };

        self.documents.insert(id.to_string(), doc);
    }

    /// Remove a document by ID.
    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(old_doc) = self.documents.remove(id) {
            self.total_length = self.total_length.saturating_sub(old_doc.length);
            for term in old_doc.term_freqs.keys() {
                if let Some(list) = self.postings.get_mut(term) {
                    list.retain(|(doc_id, _)| doc_id != id);
                }
            }
            true
        } else {
            false
        }
    }

    /// Compute BM25 scores for a query across all matching documents.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        pkb_root: &Path,
        since: Option<&str>,
        before: Option<&str>,
        type_filter: Option<&str>,
    ) -> Vec<crate::vectordb::SearchResult> {
        let n_docs = self.documents.len();
        if n_docs == 0 {
            return Vec::new();
        }

        let avg_dl = (self.total_length as f32) / (n_docs as f32);
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        // Deduplicate query tokens while keeping counts
        let mut q_freqs: HashMap<String, usize> = HashMap::new();
        for t in query_tokens {
            *q_freqs.entry(t).or_insert(0) += 1;
        }

        // Accumulate scores per document ID
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (term, q_count) in q_freqs {
            if let Some(postings) = self.postings.get(&term) {
                let n_t = postings.len();
                // Standard smoothed BM25 IDF: ln(1 + (N - n_t + 0.5) / (n_t + 0.5))
                let idf = ((n_docs as f32 - n_t as f32 + 0.5) / (n_t as f32 + 0.5) + 1.0).ln();
                if idf <= 0.0 {
                    continue;
                }

                for (doc_id, tf) in postings {
                    if let Some(doc) = self.documents.get(doc_id) {
                        let dl = doc.length as f32;
                        let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avg_dl));
                        let term_score = idf * ((tf * (BM25_K1 + 1.0)) / denom) * (q_count as f32);
                        *scores.entry(doc_id.clone()).or_insert(0.0) += term_score;
                    }
                }
            }
        }

        let mut results: Vec<crate::vectordb::SearchResult> = Vec::new();

        for (doc_id, score) in scores {
            let doc = match self.documents.get(&doc_id) {
                Some(d) => d,
                None => continue,
            };

            // Filter by document type
            if !crate::vectordb::matches_type_filter(doc.doc_type.as_deref(), type_filter) {
                continue;
            }

            // Filter by modified date
            if since.is_some() || before.is_some() {
                match doc.modified.as_deref() {
                    Some(m) => {
                        let date_prefix = &m[..m.floor_char_boundary(10)];
                        if since.is_some_and(|s| date_prefix < s)
                            || before.is_some_and(|b| date_prefix > b)
                        {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            let abs_path = if doc.path.is_absolute() {
                doc.path.clone()
            } else {
                pkb_root.join(&doc.path)
            };

            results.push(crate::vectordb::SearchResult {
                path: abs_path,
                title: doc.title.clone(),
                score,
                snippet: doc.snippet.clone(),
                chunk_text: doc.chunk_text.clone(),
                id: doc.id.clone(),
                date: doc.date.clone(),
                doc_type: doc.doc_type.clone(),
                status: doc.status.clone(),
                tags: doc.tags.clone(),
                confidence: doc.confidence,
            });
        }

        // Sort descending by BM25 score
        results.sort_by(|a, b| {
            ordered_float::OrderedFloat(b.score).cmp(&ordered_float::OrderedFloat(a.score))
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
        let results = idx.search("mem_7cc72ed8", 5, Path::new("/pkb"), None, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "mem_7cc72ed8");

        // Query by PR number
        let results_pr = idx.search("PR #520", 5, Path::new("/pkb"), None, None, None);
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

        let tasks = idx.search("optimization", 5, Path::new("/pkb"), None, None, Some("task"));
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "doc1");

        let notes = idx.search("optimization", 5, Path::new("/pkb"), None, None, Some("!task"));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "doc2");
    }
}
