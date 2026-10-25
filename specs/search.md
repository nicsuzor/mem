---
id: search
title: "PKB Search: End-to-End Hybrid Retrieval Architecture"
type: spec
status: settled
created: 2026-10-25
settled_at: 2026-10-25
tags:
  - search
  - retrieval
  - hybrid-search
  - bm25
  - rerank
  - embeddings
  - salience
  - spec
  - pkb
---

# PKB Search: End-to-End Hybrid Retrieval Architecture

This document specifies the end-to-end retrieval architecture in `mem` (covering both the `pkb-search` MCP server and `aops` CLI search surfaces). It defines the ingestion pipeline, lexical and semantic indexing, candidate fusion, cross-encoder reranking, and scoring priors.

---

## 1. Retrieval Pipeline Architecture

Retrieval in `mem` combines lexical precision, dense semantic matching, graph proximity, neural reranking, and domain-informed scoring priors:

```text
Query
  │
  ├───► Dense Query Encoding (BGE-M3, 1024-dim ONNX) ──► Vector Index (Cosine Similarity)
  │                                                                 │
  └───► NFKD / Lowercase / Stopwords / Porter Stemmer  ──► BM25 v2 Lexical Index
                                                                 │
                                                       Reciprocal Rank Fusion (RRF)
                                                                 │
                                                    Local ONNX Cross-Encoder Reranker
                                                                 │
                                                    Graph Proximity & Title Matching
                                                                 │
                                                    Priors (Recency Decay & Read Boost)
                                                    under Strict Floor Invariance
                                                                 │
                                                          Final Ranked Results
```

1. **Dual-Path Retrieval**: Queries execute concurrently against the dense vector store (BGE-M3 embeddings) and the in-memory Okapi BM25 v2 lexical index.
2. **Reciprocal Rank Fusion (RRF)**: Merges vector and BM25 candidate pools with $k = 60$.
3. **Cross-Encoder Reranking**: Re-scores top fused candidate pairs `(query, chunk_text)` using a local transformer model running on ONNX Runtime.
4. **Proximity and Title Boosting**: Applies graph proximity boost if `boost_id` is supplied, plus exact/stemmed title match boosts.
5. **Prior Adjustment with Floor Invariance**: Applies exponential recency decay and salience-derived recent-read access boost while strictly guaranteeing that no candidate below the relevance floor is promoted above it.

---

## 2. Line-Boundary Chunking with Contextual Headers

Text chunking is implemented in `src/chunker.rs` and replaces character-based slicing with semantic, structure-preserving boundaries.

### 2.1. Parameters and Invariants

- **Target Size**: ~350 tokens (1,400 characters, `CHUNK_TARGET_CHARS`), aligned with BGE-M3 tokenization (~4 characters per token).
- **Chunk Overlap**: ~64 tokens (256 characters, `CHUNK_OVERLAP_CHARS`) between consecutive chunks within a document.
- **Line-Boundary Preservation**: Slices occur strictly on newline boundaries. Chunks never bisect words or sentences unless a single line without whitespace exceeds the target window.

### 2.2. Contextual Headers

Every chunk prepends document-level hierarchy metadata before dense encoding and lexical indexing:

$$\text{Header} = \texttt{"\{title\} | \{doc\_type\} | \{H1 > H2 > H3\}\textbackslash n\textbackslash n"}$$

- **Heading Stack**: ATX markdown headings (`#` through `######`) outside code fences are tracked in a hierarchical stack (`HeadingStack`).
- **Context Injection**: Prepending the contextual header ensures isolated chunks retain full topical and architectural context during vector and BM25 retrieval.
- **Clean Body Offsets**: Raw body byte slices and character offsets are preserved alongside the chunk so user-facing snippets and extracts do not repeat metadata headers.

---

## 3. In-Memory Lexical Indexing (BM25 v2)

Lexical search is implemented in `src/bm25.rs` using Okapi BM25 ranking over multi-field document and chunk tokens.

### 3.1. Okapi BM25 Parameters

- **Saturation Parameter ($k_1$)**: $1.2$
- **Length Normalization ($b$)**: $0.75$

### 3.2. Normalization and Tokenization Pipeline

The lexical pipeline standardizes queries and document chunks:
1. **Unicode NFKD Normalization**: Decomposes characters into compatibility base forms (e.g. `é` $\to$ `e` + `\u{0301}`).
2. **Diacritic Stripping**: Filters out Unicode combining diacritical marks (`U+0300`–`U+036F`).
3. **Case Folding**: Converts text to lowercase ASCII.
4. **Stopword Removal**: Eliminates standard English grammatical stopwords (`ENGLISH_STOPWORDS`, 119 tokens) from query and body text, preventing common words from inflating lexical scores.
5. **Porter Stemming**: Reduces inflected words to root stems using Porter 2 algorithm (`rust-stemmers`).

### 3.3. Multi-Field Weights

BM25 scores aggregate term frequencies across weighted document fields:

| Field | Weight | Description |
|---|---|---|
| `id` | 5.0 | Exact or normalized identifier |
| `title` | 3.0 | Document title |
| `aliases` | 3.0 | Alternate resolution aliases |
| `tags` | 2.0 | Document frontmatter tags |
| `heading` | 2.0 | Markdown section headings |
| `body` | 1.0 | Chunk body content |
| `id_tokens` | 1.0 | Sub-token fragments of ID |

---

## 4. Dense Vector Retrieval & Cross-Encoder Reranking

### 4.1. Dense Vector Store

Dense embeddings are generated by the BGE-M3 model (1024-dimensional floating point vectors) and persisted in `$ACA_DATA/pkb_vectors.bin`. Cosine similarity evaluates query vectors against chunk vectors.

### 4.2. Reciprocal Rank Fusion (RRF)

Dense and lexical results are fused using standard RRF:

$$RRF(d) = \sum_{m \in \{\text{dense}, \text{bm25}\}} \frac{w_m}{k + \text{rank}_m(d)}$$

where $k = 60$, $w_{\text{dense}} = 1.0$, and $w_{\text{bm25}} = 1.0$.

### 4.3. ONNX Cross-Encoder Reranker

Top candidates from RRF (default top 40) pass to the local ONNX cross-encoder reranker (`src/rerank.rs`):
- **Model**: Local ONNX transformer model (`cross-encoder-reranker`).
- **Pair Encoding**: `(query, title_heading_chunk)`.
- **Scoring**: Sigmoid probability transformation with threshold gating ($\tau_h = 0.50$, $\tau_{cos} = 0.65$, $\tau_s = 0.40$).
- **Fail-Fast Discipline**: If reranking is enabled but model weights are missing or uninitialized, operations return `reranker_unavailable` rather than silently degrading.

---

## 5. Scoring Priors: Recency Decay & Recent-Read Boost

Search ranking incorporates temporal decay and usage salience to reflect human attention dynamics without corrupting underlying relevance.

### 5.1. Daily Note Exponential Recency Decay

Daily notes (`type: daily`) decay exponentially based on their calendar `date` frontmatter field:

$$\text{decay}(\text{age\_days}) = 0.85 + 0.15 \cdot \exp\left(-\frac{\text{age\_days}}{14.0}\right)$$

- **Half-Life Scaling**: Recency multiplier begins at $1.0$ when $\text{age\_days} = 0$.
- **Asymptotic Floor**: As age increases, the multiplier asymptotically approaches $0.85$ ($\approx 0.905$ at 14 days, $\approx 0.870$ at 28 days). Old daily notes are slightly down-weighted but never eliminated.
- **Non-Daily Documents**: Notes carrying an explicit `modified` timestamp decay gradually with a 30-day denominator: $0.85 + 0.15 \cdot \exp(-\text{age\_days} / 30.0)$.

### 5.2. Actionable & Strategic Type Immunity

Per `pkb-rules` §5: **Age is not staleness for actionable work or strategic targets.**

The following document types are explicitly **immune** to recency decay ($\text{decay} = 1.0$):
- `task`
- `epic`
- `target`
- `project`

Type immunity is evaluated at the head of `document_recency_decay`. Even if a task carries a creation `date` or `modified` timestamp in the distant past, its recency decay multiplier remains strictly $1.0$.

### 5.3. Recent-Read Access Boost (Salience)

Documents accumulate salience through access events in the in-memory `SalienceStore` (`src/salience.rs`):
- **Access Types**: Direct read (`get_document`), search retrieval hits, graph traversals, and MCP calls.
- **Exponential Growth**: First access initializes score to $1.0$; each subsequent access multiplies the current score by $2.0$ up to a ceiling of $100.0$.
- **Linear Daily Decay**: Without access, scores decay linearly by $1.0$ per 24 hours down to a floor of $0.0$.
- **Multiplicative Boost**: Mapped to a bounded multiplier in $[1.0, 1.20]$:

$$\text{boost} = 1.0 + 0.20 \cdot \text{clamp}\left(\frac{\text{salience} - \text{floor}}{\text{ceiling} - \text{floor}}, 0.0, 1.0\right)$$

Unaccessed documents or those decayed to floor receive a boost of $1.0$ (no change). Highly active documents receive up to a $1.20$ multiplier.

---

## 6. Strict Relevance Floor Invariance

To prevent high recency or frequent access from promoting tangentially matched documents into active search results, `mem` enforces **strict relevance floor invariance** in `apply_priors_with_floor`:

```rust
pub fn apply_priors_with_floor(
    score: f64,
    floor: f64,
    recency_factor: f64,
    recent_read_boost: f64,
) -> f64
```

### 6.1. Invariance Rules

1. **Below-Floor Containment**: If the base relevance score is strictly below the caller-specified relevance `floor` ($score < floor$), the combined score can **never** exceed the floor:
   $$\text{final\_score} < floor$$
   Priors may attenuate below-floor items further, but cannot lift them.
2. **At-or-Above-Floor Reordering**: Priors apply multiplicatively only to candidates already meeting or exceeding the relevance floor ($score \ge floor$).
3. **Clamping**: The `floor` parameter is guarded via `floor.clamp(0.0, 1.0)`. Final scores are clamped to $[floor, 1.0]$.

---

## 7. MCP and CLI Interface Contracts

Retrieval surfaces adhere to identical scoring semantics:
- **CLI**: `pkb search <query>`
- **MCP**: `search` and `task_search`

Structured result outputs expose `recency_decay`, `recent_read_boost`, and `salience` alongside composite `score` and contextual `extract` chunks to ensure complete transparency for inspecting agents.
