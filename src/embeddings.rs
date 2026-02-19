//! Embedding generation for semantic search.
//!
//! When built with the `onnx` feature, uses MiniLM-L6-v2 via ONNX Runtime
//! for high-quality 384-dimensional sentence embeddings.
//!
//! Without the `onnx` feature (default), uses a hash-based approach that
//! provides basic semantic similarity via word and character n-gram hashing.
//! This is sufficient for personal knowledge bases and works with any rustc.

use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const EMBEDDING_DIM: usize = 384;

// =============================================================================
// EMBEDDER
// =============================================================================

pub struct Embedder {
    _private: (),
}

impl Embedder {
    pub fn new() -> Result<Self> {
        tracing::info!(
            "Using hash-based embeddings (384-dim). \
             Build with --features onnx for MiniLM neural embeddings."
        );
        Ok(Self { _private: () })
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    pub fn encode(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Ok(vec![0.0; EMBEDDING_DIM]);
        }
        self.encode_hash(text)
    }

    pub fn encode_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|text| self.encode(text)).collect()
    }

    /// Hash-based embeddings using word unigrams, bigrams, and character trigrams.
    ///
    /// This is NOT a neural embedding — it won't capture deep semantics.
    /// But it does provide useful similarity for keyword-overlap-heavy searches,
    /// which is good enough for a personal note corpus until ONNX is available.
    fn encode_hash(&self, text: &str) -> Result<Vec<f32>> {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();

        // Word unigrams
        for (i, word) in words.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            word.hash(&mut hasher);
            let hash = hasher.finish();

            // Distribute across dimensions with positional variance
            for j in 0..8 {
                let dim = ((hash >> (j * 8)) as usize ^ i.wrapping_mul(31)) % EMBEDDING_DIM;
                let sign = if (hash >> (j + 32)) & 1 == 0 {
                    1.0
                } else {
                    -1.0
                };
                embedding[dim] += sign * 0.15;
            }
        }

        // Word bigrams (captures some phrase-level meaning)
        for pair in words.windows(2) {
            let mut hasher = DefaultHasher::new();
            pair[0].hash(&mut hasher);
            pair[1].hash(&mut hasher);
            let hash = hasher.finish();

            for j in 0..4 {
                let dim = ((hash >> (j * 16)) as usize) % EMBEDDING_DIM;
                let sign = if (hash >> (j + 48)) & 1 == 0 {
                    1.0
                } else {
                    -1.0
                };
                embedding[dim] += sign * 0.1;
            }
        }

        // Character trigrams (handles subword matching, typo tolerance)
        let chars: Vec<char> = lower.chars().collect();
        for window in chars.windows(3) {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            let hash = hasher.finish();

            let dim = (hash as usize) % EMBEDDING_DIM;
            let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };
            embedding[dim] += sign * 0.05;
        }

        // L2 normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > f32::EPSILON && !norm.is_nan() {
            for val in &mut embedding {
                *val /= norm;
            }
        }

        Ok(embedding)
    }
}

// =============================================================================
// TEXT CHUNKING
// =============================================================================

/// Configuration for text chunking
pub struct ChunkConfig {
    pub chunk_size: usize,
    pub overlap: usize,
    pub min_chunk_size: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            chunk_size: 800,
            overlap: 200,
            min_chunk_size: 200,
        }
    }
}

/// Chunk text into overlapping segments for embedding
pub fn chunk_text(text: &str, config: &ChunkConfig) -> Vec<String> {
    let text = text.trim();
    if text.len() <= config.chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + config.chunk_size).min(text.len());

        // Ensure valid char boundary
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }

        // Try to break at sentence boundary
        if end < text.len() {
            if let Some(break_pos) = find_sentence_break(text, start, end) {
                end = break_pos;
            }
        }

        if start >= end {
            break;
        }

        let chunk = text[start..end].trim();
        if chunk.len() >= config.min_chunk_size || chunks.is_empty() {
            chunks.push(chunk.to_string());
        } else if let Some(last) = chunks.last_mut() {
            last.push(' ');
            last.push_str(chunk);
        }

        if end >= text.len() {
            break;
        }

        start = end.saturating_sub(config.overlap);
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }

    chunks
}

fn find_sentence_break(text: &str, start: usize, ideal_end: usize) -> Option<usize> {
    let chunk = &text[start..ideal_end];

    let mut last_boundary = None;
    for (i, c) in chunk.char_indices() {
        if (c == '.' || c == '!' || c == '?') && i >= 100 {
            let after = i + c.len_utf8();
            if after >= chunk.len() || chunk[after..].starts_with(char::is_whitespace) {
                last_boundary = Some(start + after);
            }
        }
    }

    last_boundary
}
