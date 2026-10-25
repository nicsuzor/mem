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
  - tm2c2
  - fusion
  - ranking
  - bm25
  - vector
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

Retrieval in `mem` combines lexical precision, dense semantic matching, graph proximity, neural reranking, domain-informed structural priors, and temporal/access dynamics:

```text
Query
  │
  ├───► Dense Query Encoding (BGE-M3, 1024-dim ONNX) ──► Vector Index (Cosine Similarity)
  │                                                                 │ (Top 100)
  └───► NFKD / Lowercase / Stopwords / Porter Stemmer  ──► BM25 v2 Lexical Index
                                                                 │ (Top 100)
                                                       Raw-Score Silence Floor
                                                       (tau_cos >= 0.45 OR s_lex > 0)
                                                                 │
                                                       Local Min-Max Normalisation
                                                       (|P| == 1 => s_norm = 1.0)
                                                                 │
                                                       Adaptive Convex Combination (TM2C2)
                                                       (alpha = 0.35 id / 0.65 prose)
                                                                 │
                                                       Optional ONNX Cross-Encoder Reranker
                                                       (Top 40 candidates)
                                                                 │
                                                       Multiplicative Structural Priors
                                                       (M_struct = m_id * m_status * m_graph)
                                                                 │
                                                       Temporal & Access Priors
                                                       (Recency Decay & Recent-Read Boost)
                                                       under Strict Floor Invariance
                                                                 │
                                                       Final Ranked Results
```

1. **Dual-Path Candidate Retrieval**: Top 100 dense vector matches (BGE-M3 embeddings, cosine similarity) and top 100 BM25 v2 lexical matches are retrieved concurrently.
2. **Raw-Score Silence Floor**: Candidates must pass the silence threshold ($\tau_{\cos} = 0.45$ or $s_{\text{raw}}^{\text{lex}} > 0$). If both candidate pools fail the threshold, the search returns empty immediately, preventing low-relevance noise from being inflated to 1.0.
3. **Local Min-Max Normalisation & TM2C2 Fusion**: Surviving candidates are normalised query-locally into $[0.0, 1.0]$ and combined adaptively via TM2C2 ($\alpha = 0.35$ for identifier/code queries, $\alpha = 0.65$ for natural language prose).
4. **Cross-Encoder Reranking**: If enabled, the top 40 candidates are re-scored using a local transformer cross-encoder running on ONNX Runtime.
5. **Multiplicative Structural Priors**: Applied to incorporate identifier match, document lifecycle status, and graph connectivity ($M_{\text{struct}} \in [0.30, 2.268]$).
6. **Temporal & Access Priors under Floor Invariance**: Exponential recency decay and recent-read boost apply via the modular `TemporalAccessHook`, strictly respecting relevance floor invariance so sub-floor items are never promoted.

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
4. **Stopword Removal**: Eliminates standard English grammatical stopwords.
5. **Stemming**: Applies the Porter English stemming algorithm to reduce inflections.

### 3.3. Multi-Field Document Model

Document fields are weighted in the lexical index:

| Field | Weight | Description |
|---|---|---|
| `title` | 4.0 | Document title |
| `headings` | 2.5 | Contextual markdown headings |
| `tags` | 2.0 | Explicit tags in frontmatter |
| `aliases` | 2.0 | Alternate document identifiers |
| `body_chunk` | 1.0 | Chunk body content |
| `id_tokens` | 1.0 | Sub-token fragments of ID |

---

## 4. Dense Vector Retrieval, TM2C2 Fusion & Cross-Encoder Reranking

### 4.1. Dense Vector Store

Dense embeddings are generated by the BGE-M3 model (1024-dimensional floating point vectors) and persisted in `$ACA_DATA/pkb_vectors.bin`. Cosine similarity evaluates query vectors against chunk vectors.

### 4.2. Candidate Pools ($P_{\text{dense}}$, $P_{\text{lex}}$)

Candidate retrieval fetches up to:
- $N_{\text{dense}} = 100$ top vector candidates (`POOL_SIZE_DENSE`)
- $N_{\text{lex}} = 100$ top BM25 candidates (`POOL_SIZE_LEX`)

### 4.3. Raw-Score Silence Floor ($\tau_{\cos} = 0.45$)

To prevent irrelevant or noisy queries from stretching background noise to 1.0 during min-max normalization, a raw-score silence floor is enforced:

$$\tau_{\cos} = 0.45$$

1. If $\max(s_{\text{raw}}^{\text{dense}}) < \tau_{\cos}$ **AND** $\max(s_{\text{raw}}^{\text{lex}}) \le 0$, the query fails the silence floor and returns empty results (`[]`) immediately.
2. For candidate pool construction:
   - Dense candidates with $s_{\text{raw}}^{\text{dense}} < 0$ are discarded.
   - Lexical candidates with $s_{\text{raw}}^{\text{lex}} \le 0$ are discarded.

### 4.4. Query-Local Min-Max Normalisation & Single-Candidate Pool Rule

For each modality $m \in \{\text{dense}, \text{lex}\}$ with non-empty candidate pool $P_m$:

$$s_{\text{norm}}^m(d) = \frac{s_{\text{raw}}^m(d) - \min_{x \in P_m} s_{\text{raw}}^m(x)}{\max_{x \in P_m} s_{\text{raw}}^m(x) - \min_{x \in P_m} s_{\text{raw}}^m(x)}$$

- **Single-Candidate Pool Rule**: If $|P_m| = 1$ (or $\max = \min$), the single surviving candidate receives $s_{\text{norm}}^m(d) = 1.0$ for that modality, avoiding division by zero.
- **Missing Modality**: If candidate $d \notin P_m$, its score for that modality is set to $0.0$.

### 4.5. Adaptive Convex Combination (TM2C2)

Normalised scores are fused using query-adaptive convex combination:

$$s_{\text{fused}}(q, d) = \alpha(q) \cdot s_{\text{norm}}^{\text{dense}}(d) + (1 - \alpha(q)) \cdot s_{\text{norm}}^{\text{lex}}(d)$$

The mixing parameter $\alpha(q)$ dynamically routes based on query characteristics:

$$\alpha(q) = \begin{cases} 
0.35 & \text{if } q \text{ is identifier-like (lexical-favoured)} \\
0.65 & \text{if } q \text{ is prose / natural language (dense-favoured)}
\end{cases}$$

A query is classified as **identifier-like** if:
1. It contains an underscore `_`, hyphen `-`, or digit run (`0`–`9`); OR
2. It has $\le 3$ tokens and contains uppercase letters (e.g. camelCase, PascalCase) or path delimiters (`/` or `\`).

### 4.6. Local ONNX Cross-Encoder Reranker

When a `CrossEncoderReranker` is configured and loaded (`src/rerank.rs`):
- **Candidate Pool**: Top $N_{\text{rerank}} = 40$ candidates from TM2C2 fusion are evaluated.
- **Pair Encoding**: Evaluates $(q, \text{passage})$ using the local ONNX transformer model.
- **Scoring**: Sigmoid probability transformation with threshold gating ($\tau_h = 0.50$, $\tau_{\cos} = 0.65$, $\tau_s = 0.40$).
- **Ordering**: Reranker scores replace $s_{\text{fused}}$ for evaluated candidates; un-reranked candidates past $N_{\text{rerank}}$ preserve relative order.
- **Fail-Fast Discipline**: If reranking is enabled but fails, errors propagate as `RerankerError` via `try_search_hybrid`.

---

## 5. Multiplicative Structural Priors ($M_{\text{struct}}$)

After fusion (and optional reranking), domain knowledge and structural graph topology are incorporated through bounded multiplicative priors:

$$s(q, d) \leftarrow s_{\text{fused}}(q, d) \cdot M_{\text{struct}}(q, d)$$

$$M_{\text{struct}}(q, d) = m_{\text{id}}(q, d) \cdot m_{\text{status}}(d) \cdot m_{\text{graph}}(d)$$

### 5.1. Identifier and Alias Prior ($m_{\text{id}}$)

$$m_{\text{id}}(q, d) = \begin{cases} 
2.0 & \text{if } q \text{ matches document ID, filename, filename stem, or alias (case-insensitive)} \\
1.0 & \text{otherwise}
\end{cases}$$

Exact identifier lookups are strongly boosted to guarantee pinpoint navigation when querying known IDs or stems.

### 5.2. Lifecycle Status Prior ($m_{\text{status}}$)

Documents in active work states receive priority over historical, completed, or abandoned nodes:

| Status Category | Status Values | Multiplier $m_{\text{status}}$ |
|---|---|---|
| **Active / Ready** | `ready`, `in_progress`, `active` | $1.05$ |
| **Neutral / Default** | `inbox`, `queued`, untyped / missing status | $1.00$ |
| **Secondary / Paused** | `review`, `paused`, `someday`, `partial` | $0.85$ |
| **Completed** | `done`, `completed` | $0.60$ |
| **Deprecations / Inactive** | `cancelled`, `failed`, `archived`, `superseded` | $0.30$ |

### 5.3. Graph Topology Prior ($m_{\text{graph}}$)

A bounded multiplier rewards structural hubs and referenced notes using outward link degree $\text{deg}^+(d)$:

$$m_{\text{graph}}(d) = 1 + 0.08 \cdot \min\left(1, \frac{\ln(1 + \max(0, \text{deg}^+(d)))}{\ln(21)}\right)$$

- For nodes with no outward links ($\text{deg}^+ = 0$): $m_{\text{graph}} = 1.0$.
- For nodes with $20$ outward links ($\text{deg}^+ = 20$): $m_{\text{graph}} = 1 + 0.08 \cdot \frac{\ln(21)}{\ln(21)} = 1.08$.
- For nodes with $\text{deg}^+ > 20$: clamped at $1.08$.

### 5.4. Structural Prior Bounds

The total structural prior $M_{\text{struct}}$ is strictly bounded:

$$M_{\text{struct}} \in [0.30, 2.268]$$

- Minimum: $1.0 \times 0.30 \times 1.0 = 0.30$ (cancelled node without identifier match).
- Maximum: $2.0 \times 1.05 \times 1.08 = 2.268$ (exact ID match on active hub task).

---

## 6. Scoring Priors: Recency Decay & Recent-Read Boost

Temporal decay and usage salience are integrated via the modular `TemporalAccessHook`:

```rust
pub type TemporalAccessHook = Arc<dyn Fn(&SearchResult) -> f32 + Send + Sync>;
```

### 6.1. Daily Note Exponential Recency Decay

Daily notes (`type: daily`) decay exponentially based on their calendar `date` frontmatter field:

$$\text{decay}(\text{age\_days}) = 0.85 + 0.15 \cdot \exp\left(-\frac{\text{age\_days}}{14.0}\right)$$

- **Half-Life Scaling**: Recency multiplier begins at $1.0$ when $\text{age\_days} = 0$.
- **Asymptotic Floor**: As age increases, the multiplier asymptotically approaches $0.85$ ($\approx 0.905$ at 14 days, $\approx 0.870$ at 28 days).
- **Non-Daily Documents**: Notes carrying an explicit `modified` timestamp decay gradually with a 30-day denominator: $0.85 + 0.15 \cdot \exp(-\text{age\_days} / 30.0)$.

### 6.2. Actionable & Strategic Type Immunity

Per `pkb-rules` §5: **Age is not staleness for actionable work or strategic targets.**

The following document types are explicitly **immune** to recency decay ($\text{decay} = 1.0$):
- `task`
- `epic`
- `target`
- `project`

Type immunity is evaluated at the head of `document_recency_decay`. Even if a task carries a creation `date` or `modified` timestamp in the distant past, its recency decay multiplier remains strictly $1.0$.

### 6.3. Recent-Read Access Boost (Salience)

Documents accumulate salience through access events in the in-memory `SalienceStore` (`src/salience.rs`):
- **Access Types**: Direct read (`get_document`), search retrieval hits, graph traversals, and MCP calls.
- **Exponential Growth**: First access initializes score to $1.0$; each subsequent access multiplies the current score by $2.0$ up to a ceiling of $100.0$.
- **Linear Daily Decay**: Without access, scores decay linearly by $1.0$ per 24 hours down to a floor of $0.0$.
- **Multiplicative Boost**: Mapped to a bounded multiplier in $[1.0, 1.20]$:

$$\text{boost} = 1.0 + 0.20 \cdot \text{clamp}\left(\frac{\text{salience} - \text{floor}}{\text{ceiling} - \text{floor}}, 0.0, 1.0\right)$$

---

## 7. Strict Relevance Floor Invariance

To prevent high recency or frequent access from promoting tangentially matched documents into active search results, `mem` enforces **strict relevance floor invariance** in `apply_priors_with_floor`:

```rust
pub fn apply_priors_with_floor(
    score: f64,
    floor: f64,
    recency_factor: f64,
    recent_read_boost: f64,
) -> f64
```

### 7.1. Invariance Rules

1. **Below-Floor Containment**: If the base relevance score is strictly below the caller-specified relevance `floor` ($score < floor$), the combined score can **never** exceed the floor:
   $$\text{final\_score} < floor$$
   Priors may attenuate below-floor items further, but cannot lift them.
2. **At-or-Above-Floor Reordering**: Priors apply multiplicatively only to candidates already meeting or exceeding the relevance floor ($score \ge floor$).
3. **Clamping**: The `floor` parameter is guarded via `floor.clamp(0.0, 1.0)`. Final scores are clamped to $[floor, 1.0]$.

---

## 8. Deterministic Ordering and Tie-Breaking

Search candidates are sorted descending by final score. Ties are broken deterministically:
1. `score` descending (via `ordered_float::OrderedFloat`);
2. `id` ascending (lexicographical);
3. `chunk_id` ascending.

The final result vector is truncated to `limit`.

---

## 9. MCP and CLI Interface Contracts

Retrieval surfaces adhere to identical scoring semantics:
- **CLI**: `pkb search <query>`
- **MCP**: `search` and `task_search`

Structured result outputs expose `recency_decay`, `recent_read_boost`, and `salience` alongside composite `score` and contextual `extract` chunks to ensure complete transparency for inspecting agents.

---

## 10. Verification and Invariants

The automated test suite verifies all ranking and retrieval invariants:
- **Silence Floor**: `test_silence_floor_on_garbage_queries_returns_empty`, `test_silence_floor_dense_above_threshold_passes`, `test_silence_floor_lex_above_threshold_passes`
- **Single-Candidate Pools**: `test_single_candidate_dense_surviving`, `test_single_candidate_lex_surviving`
- **Adaptive Alpha Routing**: `test_alpha_routing_identifier_queries`, `test_alpha_routing_prose_queries`
- **Structural Priors**: `test_m_struct_overall_bounds`, `test_m_status_values`, `test_m_graph_monotonicity`
- **Chunking & Contextual Headers**: `test_contextual_header_formatting`, `test_line_boundary_splitting_and_overlap` in `src/chunker.rs`
- **BM25 Lexical Index**: `test_bm25_exact_match_retrieval`, `test_tokenizer_preserves_identifiers_and_splits` in `src/bm25.rs`
- **Recency Decay & Salience**: `test_daily_decay_monotonic`, `test_task_type_immunity`, `test_floor_invariance`
