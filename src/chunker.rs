//! Line-boundary markdown chunking with contextual headers and multi-chunk scoring.
//!
//! Replaces naive character-based chunking with ~350-token (1,400-char)
//! line-boundary chunking with 64-token (256-char) overlap (P2-M6 / 6A).
//!
//! Prepending contextual headers (`"{title} | {doc_type} | {h1 > h2 > h3}\n\n"`)
//! ensures both dense embeddings and BM25 postings carry document-level context on
//! every chunk, while clean body slices and offsets are preserved for snippet generation.

use serde::{Deserialize, Serialize};

/// Target character length for chunks (~350 BGE-M3 tokens at ~4 chars/token).
pub const CHUNK_TARGET_CHARS: usize = 1400;

/// Character overlap between adjacent chunks (~64 tokens at ~4 chars/token).
pub const CHUNK_OVERLAP_CHARS: usize = 256;

/// A heading entry in the markdown hierarchy stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub level: usize,
    pub title: String,
}

/// Tracks the active hierarchy of markdown headings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadingStack {
    levels: Vec<HeadingEntry>,
}

impl HeadingStack {
    pub fn new() -> Self {
        Self { levels: Vec::new() }
    }

    /// Reset any headings at or below the given level, and push the new heading.
    pub fn push(&mut self, level: usize, title: String) {
        self.levels.retain(|entry| entry.level < level);
        self.levels.push(HeadingEntry { level, title });
    }

    /// Clear the entire heading stack.
    pub fn clear(&mut self) {
        self.levels.clear();
    }

    /// Returns the active heading path formatted as `"H1 > H2 > H3"`, or `None` if empty.
    pub fn heading_path(&self) -> Option<String> {
        if self.levels.is_empty() {
            None
        } else {
            Some(
                self.levels
                    .iter()
                    .map(|e| e.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" > "),
            )
        }
    }
}

/// Parse a markdown line to determine if it is an ATX heading outside code blocks.
///
/// Returns `Some((level, title))` if it is an ATX heading (`#` to `######`),
/// or `None` otherwise.
pub fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let mut hashes = 0;
    for c in trimmed.chars() {
        if c == '#' {
            hashes += 1;
        } else {
            break;
        }
    }

    if !(1..=6).contains(&hashes) {
        return None;
    }

    // CommonMark: Opening '#' characters must be followed by whitespace or end of line
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }

    let title = rest.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        return None;
    }

    Some((hashes, title.to_string()))
}

/// Format the contextual header for a chunk.
///
/// Format: `"{title} | {doc_type} | {heading_path}\n\n"`
/// Cleanly omits `heading_path` when `None` or empty: `"{title} | {doc_type}\n\n"`.
pub fn format_contextual_header(
    title: &str,
    doc_type: Option<&str>,
    heading_path: Option<&str>,
) -> String {
    let dt = doc_type.unwrap_or("").trim();
    let hp = heading_path.unwrap_or("").trim();

    match (!dt.is_empty(), !hp.is_empty()) {
        (true, true) => format!("{title} | {dt} | {hp}\n\n"),
        (true, false) => format!("{title} | {dt}\n\n"),
        (false, true) => format!("{title} | {hp}\n\n"),
        (false, false) => format!("{title}\n\n"),
    }
}

/// An emitted document chunk with contextualized text and clean body slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// Contextualized chunk text for dense embedding and BM25 indexing.
    pub text: String,
    /// Clean body slice text for display snippets (no synthetic header).
    pub body: String,
    /// Heading path active for this chunk, if any.
    pub heading_path: Option<String>,
    /// Byte start offset in original body.
    pub start_offset: usize,
    /// Byte end offset in original body.
    pub end_offset: usize,
}

/// An individual chunk record storing vectors, text, and offsets (AC3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub text: String,
    pub body: String,
    pub heading_path: Option<String>,
    pub vector: Option<Vec<f32>>,
    pub start_offset: usize,
    pub end_offset: usize,
}

/// Document record with multi-chunk vector pooling and best-chunk selection (AC3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: String,
    pub title: String,
    pub doc_type: Option<String>,
    /// Mean of chunk vectors for compatibility (AC3).
    pub vector: Vec<f32>,
    /// Per-chunk records (AC3).
    pub chunks: Vec<ChunkRecord>,
}

impl DocumentRecord {
    pub fn new(
        id: String,
        title: String,
        doc_type: Option<String>,
        chunks: Vec<ChunkRecord>,
    ) -> Self {
        let chunk_vecs: Vec<Vec<f32>> = chunks.iter().filter_map(|c| c.vector.clone()).collect();
        let vector = mean_vector(&chunk_vecs);
        Self {
            id,
            title,
            doc_type,
            vector,
            chunks,
        }
    }

    /// Compute max-pooled score over chunks: `s_raw^dense(q, d) = max_i cos(e(q), e(c_i))`.
    pub fn max_pool_score(&self, query_vec: &[f32]) -> (f32, usize) {
        let chunk_vecs: Vec<Vec<f32>> =
            self.chunks.iter().filter_map(|c| c.vector.clone()).collect();
        max_pool_dense_score(query_vec, &chunk_vecs)
    }

    /// Select snippet from the highest-scoring chunk `c_i*`.
    pub fn best_snippet(&self, query_vec: &[f32], max_len: usize) -> String {
        let (_, best_idx) = self.max_pool_score(query_vec);
        let bodies: Vec<String> = self.chunks.iter().map(|c| c.body.clone()).collect();
        select_best_chunk_snippet(&bodies, best_idx, max_len)
    }
}

/// Compute elementwise mean of chunk vectors for compatibility (AC3).
pub fn mean_vector(chunk_vectors: &[Vec<f32>]) -> Vec<f32> {
    if chunk_vectors.is_empty() {
        return Vec::new();
    }
    let dim = chunk_vectors[0].len();
    let mut mean = vec![0.0f32; dim];
    for v in chunk_vectors {
        for (i, val) in v.iter().enumerate() {
            mean[i] += val;
        }
    }
    let n = chunk_vectors.len() as f32;
    for val in &mut mean {
        *val /= n;
    }
    mean
}

/// Compute max-pooled cosine similarity over chunks: `max_i cos(e(q), e(c_i))` (AC3).
pub fn max_pool_dense_score(query_vector: &[f32], chunk_vectors: &[Vec<f32>]) -> (f32, usize) {
    let mut best_score = f32::NEG_INFINITY;
    let mut best_idx = 0;
    for (i, chunk_emb) in chunk_vectors.iter().enumerate() {
        let score = crate::distance::cosine_similarity(query_vector, chunk_emb);
        if score > best_score {
            best_score = score;
            best_idx = i;
        }
    }
    (best_score, best_idx)
}

/// Select snippet from the best-scoring chunk (AC3).
pub fn select_best_chunk_snippet(
    body_chunks: &[String],
    best_chunk_idx: usize,
    max_len: usize,
) -> String {
    let full = if best_chunk_idx < body_chunks.len() {
        &body_chunks[best_chunk_idx]
    } else if !body_chunks.is_empty() {
        &body_chunks[0]
    } else {
        return String::new();
    };

    let mut trunc = max_len.min(full.len());
    while trunc > 0 && !full.is_char_boundary(trunc) {
        trunc -= 1;
    }
    full[..trunc].to_string()
}

/// Line slice within a document body with byte offsets and heading information.
#[derive(Debug, Clone)]
struct LineInfo<'a> {
    text: &'a str,
    start_offset: usize,
    end_offset: usize,
    is_heading: bool,
    heading_path_after: Option<String>,
}

/// Split markdown body into lines and track code fences and headings.
fn prepare_lines<'a>(body: &'a str) -> Vec<LineInfo<'a>> {
    let mut lines = Vec::new();
    let mut current_offset = 0;
    let mut in_code_fence = false;
    let mut heading_stack = HeadingStack::new();

    for raw_line in body.split_inclusive('\n') {
        let line_len = raw_line.len();
        let start = current_offset;
        let end = current_offset + line_len;
        current_offset = end;

        // Strip trailing \r\n or \n for text inspection
        let trimmed_line = raw_line.trim_end_matches(['\r', '\n']);

        // Check for code fences
        let fence_candidate = trimmed_line.trim_start();
        if fence_candidate.starts_with("```") || fence_candidate.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            lines.push(LineInfo {
                text: trimmed_line,
                start_offset: start,
                end_offset: end,
                is_heading: false,
                heading_path_after: heading_stack.heading_path(),
            });
            continue;
        }

        let mut heading_detected = false;
        if !in_code_fence {
            if let Some((lvl, title)) = parse_atx_heading(trimmed_line) {
                heading_stack.push(lvl, title);
                heading_detected = true;
            }
        }

        // If a single line exceeds CHUNK_TARGET_CHARS, split it into chunks mid-line
        if trimmed_line.len() > CHUNK_TARGET_CHARS {
            let mut seg_start = start;
            let mut rem = trimmed_line;
            let mut is_first_seg = true;

            while !rem.is_empty() {
                let mut cut = CHUNK_TARGET_CHARS.min(rem.len());
                while cut > 0 && !rem.is_char_boundary(cut) {
                    cut -= 1;
                }
                if cut == 0 {
                    cut = rem
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| i)
                        .unwrap_or(rem.len());
                }

                let seg_end = seg_start + cut;
                lines.push(LineInfo {
                    text: &rem[..cut],
                    start_offset: seg_start,
                    end_offset: seg_end,
                    is_heading: is_first_seg && heading_detected,
                    heading_path_after: heading_stack.heading_path(),
                });

                seg_start = seg_end;
                rem = &rem[cut..];
                is_first_seg = false;
            }
        } else {
            lines.push(LineInfo {
                text: trimmed_line,
                start_offset: start,
                end_offset: end,
                is_heading: heading_detected,
                heading_path_after: heading_stack.heading_path(),
            });
        }
    }

    lines
}

/// Chunk a markdown document body into contextualized chunks (AC1, AC2).
///
/// Previews ~350-token (1,400-char) line-boundary chunks with ~64-token (256-char) overlap.
/// Prepends `"{title} | {doc_type} | {heading_path}\n\n"` to every chunk.
pub fn chunk_document(title: &str, doc_type: Option<&str>, body: &str) -> Vec<Chunk> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        let header = format_contextual_header(title, doc_type, None);
        return vec![Chunk {
            text: header.trim().to_string(),
            body: String::new(),
            heading_path: None,
            start_offset: 0,
            end_offset: 0,
        }];
    }

    let lines = prepare_lines(body);
    if lines.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start_idx = 0;

    while start_idx < lines.len() {
        let mut cur_len = 0;
        let mut cand_end = start_idx;

        for (i, line) in lines.iter().enumerate().skip(start_idx) {
            let line_len = line.text.len();
            let next_len = if cand_end == start_idx {
                line_len
            } else {
                cur_len + 1 + line_len
            };

            if next_len > CHUNK_TARGET_CHARS && cand_end > start_idx {
                break;
            }

            cur_len = next_len;
            cand_end = i + 1;
        }

        // Heading attachment rule: Never end a chunk on a trailing heading
        // when lines remain in the document. Back up to before the heading
        // so the heading stays attached to its following paragraph.
        if cand_end < lines.len() {
            let mut test_end = cand_end;
            while test_end > start_idx + 1 && lines[test_end - 1].text.trim().is_empty() {
                test_end -= 1;
            }
            if test_end > start_idx + 1 && lines[test_end - 1].is_heading {
                cand_end = test_end - 1;
                while cand_end > start_idx + 1 && lines[cand_end - 1].text.trim().is_empty() {
                    cand_end -= 1;
                }
            }
        }

        let chunk_lines = &lines[start_idx..cand_end];
        let start_offset = chunk_lines.first().unwrap().start_offset;
        let end_offset = chunk_lines.last().unwrap().end_offset;

        // Clean body slice (trimmed for neat display)
        let raw_slice = &body[start_offset..end_offset];
        let clean_body = raw_slice.trim().to_string();

        // Determine active heading path for this chunk:
        // Prefer the heading path established inside the chunk; fallback to the active path at start_idx.
        let heading_path = chunk_lines
            .iter()
            .rev()
            .find_map(|l| {
                if l.is_heading {
                    l.heading_path_after.clone()
                } else {
                    None
                }
            })
            .or_else(|| lines[start_idx].heading_path_after.clone());

        let header = format_contextual_header(title, doc_type, heading_path.as_deref());
        let text = format!("{header}{clean_body}");

        chunks.push(Chunk {
            text,
            body: clean_body,
            heading_path,
            start_offset,
            end_offset,
        });

        if cand_end >= lines.len() {
            break;
        }

        // Calculate overlap: walk backwards ~CHUNK_OVERLAP_CHARS from cand_end
        let mut overlap_len = 0;
        let mut next_start = cand_end;

        for i in (start_idx + 1..cand_end).rev() {
            let line_len = lines[i].text.len() + 1;
            if overlap_len + line_len <= CHUNK_OVERLAP_CHARS || next_start == cand_end {
                overlap_len += line_len;
                next_start = i;
            } else {
                break;
            }
        }

        if next_start <= start_idx {
            next_start = start_idx + 1;
        }

        start_idx = next_start;
    }

    chunks
}

/// Chunk only the markdown body without contextual headers (for snippet/body alignment).
pub fn chunk_markdown_clean(body: &str) -> Vec<String> {
    chunk_document("", None, body)
        .into_iter()
        .map(|c| c.body)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_stack_tracking() {
        let mut stack = HeadingStack::new();
        assert_eq!(stack.heading_path(), None);

        // 1. # H1
        stack.push(1, "H1".to_string());
        assert_eq!(stack.heading_path(), Some("H1".to_string()));

        // 2. ## H2
        stack.push(2, "H2".to_string());
        assert_eq!(stack.heading_path(), Some("H1 > H2".to_string()));

        // 3. ### H3
        stack.push(3, "H3".to_string());
        assert_eq!(stack.heading_path(), Some("H1 > H2 > H3".to_string()));

        // 4. Reset lower levels: new ## H2_new resets H2 and H3
        stack.push(2, "H2_new".to_string());
        assert_eq!(stack.heading_path(), Some("H1 > H2_new".to_string()));

        // 5. Reset all: new # H1_new resets all
        stack.push(1, "H1_new".to_string());
        assert_eq!(stack.heading_path(), Some("H1_new".to_string()));

        // 6. Test parsing headings and ignoring non-headings / code fences
        assert_eq!(parse_atx_heading("# Heading 1"), Some((1, "Heading 1".to_string())));
        assert_eq!(parse_atx_heading("## Subheading ##"), Some((2, "Subheading".to_string())));
        assert_eq!(parse_atx_heading("### Deep level"), Some((3, "Deep level".to_string())));
        assert_eq!(parse_atx_heading("#123 not a heading"), None);
        assert_eq!(parse_atx_heading("#tag"), None);
        assert_eq!(parse_atx_heading("Regular paragraph"), None);
    }

    #[test]
    fn test_line_boundary_splitting_and_overlap() {
        // Build 40 lines of 80 characters each (~3200 characters)
        let lines: Vec<String> = (0..40)
            .map(|i| format!("Line {:02} {}", i, "x".repeat(70)))
            .collect();
        let body = lines.join("\n");

        let chunks = chunk_document("Test Title", Some("note"), &body);
        assert!(chunks.len() >= 2, "Expected multiple chunks for 3200 chars");

        // Verify chunk bounds: each clean body slice is under target + tolerance
        for chunk in &chunks {
            assert!(
                chunk.body.len() <= CHUNK_TARGET_CHARS + 200,
                "Chunk body {} exceeded target {}",
                chunk.body.len(),
                CHUNK_TARGET_CHARS
            );
            // Verify chunks split on line boundaries: every line must start with "Line "
            for line in chunk.body.lines() {
                assert!(line.starts_with("Line "), "Line was split mid-line: {}", line);
            }
        }

        // Verify overlap between chunk 0 and chunk 1
        let chunk0_lines: Vec<&str> = chunks[0].body.lines().collect();
        let chunk1_lines: Vec<&str> = chunks[1].body.lines().collect();
        let last_line_c0 = chunk0_lines.last().unwrap();
        assert!(
            chunk1_lines.contains(last_line_c0),
            "Chunk 1 must overlap with the tail lines of Chunk 0"
        );
    }

    #[test]
    fn test_heading_attachment() {
        // Document where a heading appears near the 1400-char boundary (total 1480 chars > 1400)
        let prefix = "P".repeat(1350);
        let body = format!("{prefix}\n\n## Important Section\n\nThis is the content of the section.");

        let chunks = chunk_document("Title", Some("task"), &body);
        assert!(chunks.len() >= 2);

        // Chunk 0 must not end with "## Important Section" dangling without its paragraph
        assert!(
            !chunks[0].body.trim().ends_with("## Important Section"),
            "Heading must not be orphaned at end of chunk 0: {:?}",
            chunks[0].body
        );

        // Chunk 1 must contain the heading and its paragraph
        assert!(chunks[1].body.contains("## Important Section"));
        assert!(chunks[1].body.contains("This is the content of the section."));
        assert_eq!(chunks[1].heading_path.as_deref(), Some("Important Section"));
    }

    #[test]
    fn test_contextual_header_formatting() {
        // With heading
        let h1 = format_contextual_header("My Title", Some("task"), Some("H1 > H2"));
        assert_eq!(h1, "My Title | task | H1 > H2\n\n");

        // Without heading (omits heading_path cleanly)
        let h2 = format_contextual_header("My Title", Some("task"), None);
        assert_eq!(h2, "My Title | task\n\n");

        // Without heading (empty string)
        let h3 = format_contextual_header("My Title", Some("task"), Some(""));
        assert_eq!(h3, "My Title | task\n\n");

        // Document chunks check
        let md = "# Overview\n\nThis is overview text.";
        let chunks = chunk_document("Doc", Some("knowledge"), md);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Doc | knowledge | Overview\n\n# Overview\n\nThis is overview text.");
        assert_eq!(chunks[0].body, "# Overview\n\nThis is overview text.");
    }

    #[test]
    fn test_oversized_single_line_splits_mid_line() {
        // Line of 3000 chars exceeds 1400 chars
        let long_line = "A".repeat(3000);
        let chunks = chunk_document("Doc", Some("note"), &long_line);
        assert!(chunks.len() >= 2, "Oversized single line must be split");
        for c in &chunks {
            assert!(c.body.len() <= CHUNK_TARGET_CHARS);
        }
    }

    #[test]
    fn test_multi_chunk_max_pooling_and_snippet() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let v3 = vec![0.0, 0.0, 1.0];

        // Mean vector compatibility check (AC3)
        let mean = mean_vector(&[v1.clone(), v2.clone(), v3.clone()]);
        assert_eq!(mean, vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]);

        let chunks = vec![
            ChunkRecord {
                text: "Doc | task | H1\n\nChunk one about authentication.".to_string(),
                body: "Chunk one about authentication.".to_string(),
                heading_path: Some("H1".to_string()),
                vector: Some(v1.clone()),
                start_offset: 0,
                end_offset: 31,
            },
            ChunkRecord {
                text: "Doc | task | H2\n\nChunk two about database migrations.".to_string(),
                body: "Chunk two about database migrations.".to_string(),
                heading_path: Some("H2".to_string()),
                vector: Some(v2.clone()),
                start_offset: 32,
                end_offset: 68,
            },
        ];

        let record = DocumentRecord::new("doc-1".to_string(), "Doc".to_string(), Some("task".to_string()), chunks);
        assert_eq!(record.vector, vec![0.5, 0.5, 0.0]);

        // Query aligned with v2 (database migrations)
        let query = vec![0.0, 1.0, 0.0];
        let (score, best_idx) = record.max_pool_score(&query);
        assert!((score - 1.0).abs() < 1e-5);
        assert_eq!(best_idx, 1);

        // Best snippet must select chunk 1's clean body without synthetic header
        let snippet = record.best_snippet(&query, 100);
        assert_eq!(snippet, "Chunk two about database migrations.");
        assert!(!snippet.contains("Doc | task"));
    }
}
