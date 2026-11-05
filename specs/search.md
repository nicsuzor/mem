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
                                                       Cross-Encoder Reranker (by default, Top 40)
                                                       (or degraded fallback with explicit absent reason)
                                                                 │
                                                       Unified Multiplicative Priors
                                                       (M_struct * M_proximity * M_title)
                                                                 │
                                                       Temporal & Access Priors
                                                       (Recency Decay & Recent-Read Boost)
                                                                 │
                                                       Terminal Soft-Ceiling Scaling & Identity Reservation
                                                       (Identity match = 1.000; non-identity hits bounded
                                                        in [floor, 0.990] via C1 rational knee)
                                                                 │
                                                       Final Ranked Results
```

1. **Dual-Path Candidate Retrieval**: Top 100 dense vector matches (BGE-M3 embeddings, cosine similarity) and top 100 BM25 v2 lexical matches are retrieved concurrently.
2. **Raw-Score Silence Floor**: Candidates must pass the silence threshold ($\tau_{\cos} = 0.45$ or $s_{\text{raw}}^{\text{lex}} > 0$). If both candidate pools fail the threshold, the search returns empty immediately, preventing low-relevance noise from being inflated to 1.0.
3. **Local Min-Max Normalisation & TM2C2 Fusion**: Surviving candidates are normalised query-locally into $[0.0, 1.0]$ and combined adaptively via TM2C2 ($\alpha = 0.35$ for identifier/code queries, $\alpha = 0.65$ for natural language prose).
4. **Cross-Encoder Reranking by Default**: Enabled by default; the top 40 candidates are re-scored using a local transformer cross-encoder running on ONNX Runtime. When model files are absent, search proceeds with `ranking: "fused"` and explicitly reports the absence reason in the reply.
5. **Unified Multiplicative Priors**: Applied in a single pass incorporating identifier match, document lifecycle status, graph connectivity ($M_{\text{struct}} \in [0.30, 2.268]$), query-time graph proximity (`boost_id`), and token-overlap title match (`title_match_boost`).
6. **Temporal & Access Priors under Floor Invariance**: Exponential recency decay and recent-read boost apply via the modular `TemporalAccessHook`, strictly respecting relevance floor invariance so sub-floor items are never promoted.
7. **Terminal Soft-Ceiling Scaling & Identity Reservation**: Non-identity candidate scores crossing $T = 0.900$ are saturated asymptotically toward $C = 0.990$ using a $C^1$-continuous rational knee curve, strictly preserving discrimination and rank monotonicity while reserving $1.000$ exclusively for exact identity matches.

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
- **Score Invariant**: Relevance scores for non-identity search hits are strictly bounded in $[0.0, 0.990]$. The score $1.000$ is reserved exclusively for exact identity-key hits (`m_id` match on ID, filename stem, or alias).
- **Discrimination Invariant**: Distinct candidates with differing relevance or prior features produce distinct final scores ($s_i \ne s_{i+1}$), completely eliminating the 1.000 score plateau defect.

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

### 4.7. Concurrent Dense Query Encoding & Warm Session Pool

To ensure concurrent search calls execute in parallel without serialising behind a single session lock or stalling on synchronous disk I/O:
1. **Pre-warmed Session Pool**: In CPU mode, the ONNX `SessionPool` initializes up to `max_sessions_cpu()` sessions during construction using scoped parallel threads. Multi-core systems with $\ge 5$ cores allocate at least 5 sessions (up to 8, computed as $\max(\min(5, \text{cores}), \text{clamp}(\lfloor \text{cores} / 2 \rfloor, 2, 8))$), ensuring a fan-out of 5 searches runs on dedicated sessions simultaneously. On constrained hosts ($< 5$ cores), concurrency scales with available cores to bound memory and CPU footprint. The core-count target is then capped by memory (`max_sessions_for()`): each session holds its own optimised copy of the model weights and grows its own inference arena, so the pool takes at most as many sessions as fit in half the process memory limit (the cgroup limit if set, else physical memory) at one on-disk model size (`model.onnx` plus `model.onnx_data`) each, and never fewer than 1. The other half is headroom for the graph, the vector store and inference arenas. A 10 GiB container with BGE-M3 (2.27 GB) therefore runs 2 sessions, not the 6 its 12 cores would ask for. For test harnesses and mocks, `Embedder::new_dummy()` pre-warms at least 5 dummy session locks regardless of host cores to ensure deterministic concurrency verification in CI.
2. **Non-blocking Query Path**: `acquire_session` non-blockingly probes sessions via `try_lock()`. If all sessions are busy under heavy concurrency, it dispatches using round-robin index rotation across the pre-warmed pool. Synchronous ONNX model file reading or `commit_from_file` calls are strictly prohibited on the query path and never hold exclusive write locks on `sessions`.
3. **Concurrency Target (AC2)**: Five parallel searches complete in under twice the latency of a single search ($T_{\text{fanout}(5)} < 2 \times T_{\text{single}}$) on multi-core systems whose memory cap admits 5 sessions ($\ge 5$ cores and a limit of at least ten model sizes, or in mock test suites). Where the memory cap admits fewer, the fan-out queues on the sessions that fit: a slower fan-out, never an out-of-memory kill.

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
3. **Continuous Soft-Knee Saturation**: Destructive upper ceiling clamping (`.clamp(floor, 1.0)`) is replaced with a $C^1$-continuous rational soft-knee curve activating at threshold $T = 0.900$ asymptotically approaching ceiling $C = 0.990$:
   $$S_{\text{out}} = \begin{cases}
   S_{\text{boosted}} & \text{if } S_{\text{boosted}} \le T \\
   T + \frac{(C - T)(S_{\text{boosted}} - T)}{(C - T) + (S_{\text{boosted}} - T)} & \text{if } S_{\text{boosted}} > T
   \end{cases}$$
   With $T = 0.900$ and $C = 0.990$ ($C - T = 0.090$):
   - **Boundary Continuity ($C^0$)**: At $S_{\text{boosted}} = 0.900$, $S_{\text{out}} = 0.900$.
   - **Smooth Transition ($C^1$)**: $\left.\frac{d S_{\text{out}}}{d S_{\text{boosted}}}\right|_{T^+} = 1.0$, matching unit slope below $T$.
   - **Strict Monotonicity**: $\frac{d S_{\text{out}}}{d S_{\text{boosted}}} > 0$ for all finite $S_{\text{boosted}} \ge T$.
   - **Floor Invariance**: For any $floor \le T = 0.900$, candidates meeting floor ($S \ge floor$) are never depressed below floor.

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

### 9.1. Detail Levels and Hit Projections
- `metadata`: Lean projection (`id`, `title`, `type`, `status`, `score`) omitting tags, priors, extract, and snippet for minimal token footprint.
- `summary`: Compact agent scan projection (`id`, `title`, `type`, `status`, `score`, and matching `snippet`), enabling fast relevance judgment without full reads.
- `snippet`: Standard projection including `extract` as short snippet alongside priors and tags.
- `chunk`: Standard projection including full matched chunk body in `extract`.
- `full`: Complete document body up to `max_bytes`.
- `ids`: Identity list projection.

### 9.2. Reranker Execution and Absence Transparency
The cross-encoder reranker runs by default on candidate pools. Search replies explicitly declare execution state in the envelope:
- When reranker is loaded and executed: `ranking: "reranked"`, `reranker_loaded: true`.
- When model files are absent or unconfigured: `ranking: "fused"`, `reranker_loaded: false`, with `reranker_reason` explicitly identifying why (e.g. `"model absent"` or `"disabled by configuration"`).

### 9.3. Type Family Filtering Semantics
Filter expressions on `type` follow universal taxonomic categories (`specs/pkb-type-taxonomy.md`):
- **Taxonomic Families**:
  - `note` / `notes`: Expands to Category 2 Reference types `["note", "knowledge", "memory", "observation", "insight", "exploration"]`.
  - `task` / `actionable`: Expands to Category 1 Actionable types `["task", "epic", "learn", "pr", "bug", "feature", "action"]`.
- **Exact Matching (`=`)**: Prefixing with `=` (e.g. `type: "=note"`) restricts matching strictly to literal `type: note`.
- **Exact Negation (`!=`)**: Prefixing with `!=` (e.g. `type: "!=note"`) excludes only literal `type: note` while retaining other family members.
- **Family Negation (`!`)**: Prefixing with `!` (e.g. `type: "!note"`) excludes the entire family (`!note,!knowledge,!memory,...`).
- **Composite Lists**: Comma-separated expressions compose seamlessly (e.g. `type: "note,task"`, `type: "note,!memory"`, `type: "=note,task"`).

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
- **Top Hits Score Discrimination (C1)**: `test_search_scores_are_discriminating_and_do_not_saturate_at_one`
- **Identity-Key Hit Exclusivity (C2)**: `test_exact_id_scores_one_prose_hits_sub_one`
- **Unified Ranking & Prior Parity (C3)**: `test_ranked_score_deprecation_parity`
- **Soft-Ceiling Monotonicity & Floor Invariance (C4)**: `test_apply_priors_preserves_score_delta_above_floor`
- **Taxonomic Type Filtering (C5-C10)**:
  - `test_search_type_filter_note_family_expands_to_knowledge_and_memory` (C5)
  - `test_search_type_filter_actionable_family_expansion` (C6)
  - `test_matches_type_filter_negated_note_family` (C7)
  - `test_matches_type_filter_exact_syntax` (C8)
  - `test_matches_type_filter_exact_negation` (C9)
  - `test_matches_type_filter_composite_expressions` (C10)
- **Summary Detail & Lean Projections (AC1)**: `test_search_summary_detail_hit_carries_type_status_snippet_and_metadata_omits_snippet`
- **Reranker Execution and Absence Reason (AC3)**: `test_search_reranker_executes_by_default_or_reports_absent_reason`
- **Concurrent Search Fan-out (AC1 & AC2)**: `test_concurrent_five_searches_do_not_serialise` in `src/mcp_server/tests/slow_tail_tests.rs` ensuring 5 concurrent searches complete in under twice single query latency, and OpenTelemetry trace analysis verifying `encode_query` does not block on model disk loading or session mutexes.
