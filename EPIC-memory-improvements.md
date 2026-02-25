# Epic: Memory System Improvements

**Version:** 0.2.0 target
**Principle:** LLMs are fundamentally text-in, text-out systems. Rather than hard-coding human-like memory structures, focus on context engineering — managing the information available to the model at inference time.

---

## 1. Add `confidence`, `source`, `supersedes` fields to memories

**Priority:** High — Immediate
**Complexity:** Small

### Problem

Memory documents currently have no way to express how certain an observation is, where it came from, or whether it replaces a previous observation. An agent creating memories for "user prefers dark mode" and later "user switched to light mode" has no mechanism to mark the first as superseded.

### Design

Add three optional frontmatter fields to memory-type documents:

```yaml
---
id: mem-a1b2c3d4
title: "User prefers light mode"
type: observation
confidence: 0.9
source: "claude-session-2025-02-20"
supersedes: mem-f5e6d7c8
---
```

- **`confidence`** (`f64`, 0.0–1.0): How certain the observation is. Defaults to `1.0` if omitted. Populated by the creating agent. Used as a retrieval signal — lower-confidence memories can be ranked below higher-confidence ones at search time.
- **`source`** (`String`): Already exists in `MemoryFields` but not in `DocumentFields` or `GraphNode`. Normalize it: the session, conversation, URL, or context that produced this memory. Free-form text, not a node reference.
- **`supersedes`** (`String`, node ID): Points to the ID of a memory this one replaces. Creates a new `EdgeType::Supersedes` graph edge from the new memory to the old one. The old memory is not deleted — it remains in the graph as historical context — but retrieval should prefer the newer superseding memory.

### Implementation

1. **`document_crud.rs`**: Add `confidence: Option<f64>` and `supersedes: Option<String>` to `MemoryFields` and `DocumentFields`. Emit them in frontmatter generation.
2. **`graph.rs`**: Add `EdgeType::Supersedes` variant. Add `confidence: Option<f64>`, `source: Option<String>`, `supersedes: Option<String>` to `GraphNode`. Parse them in `from_pkb_document()`.
3. **`graph_store.rs`**: When building edges, emit a `Supersedes` edge if `supersedes` is set. During retrieval, if a memory has been superseded, deprioritize it (or annotate it in results).
4. **`mcp_server.rs`**: Add `confidence`, `supersedes` params to `create_memory` and `create` tool schemas. Pass through to `MemoryFields`/`DocumentFields`. In `handle_retrieve_memory`, use `confidence` as a tie-breaking signal and annotate superseded memories.
5. **`vectordb.rs`**: Optionally store `confidence` in `DocumentEntry` for use in search scoring.

### Acceptance criteria

- `create_memory` accepts `confidence`, `source`, `supersedes` parameters
- `create` accepts the same fields
- `retrieve_memory` results show confidence and note when a memory has been superseded
- `pkb_context` shows supersedes/superseded-by relationships
- Existing memories without these fields continue to work (all optional, backward-compatible)

---

## 2. Add `subject` field — entity-centric memory retrieval

**Priority:** High — Immediate
**Complexity:** Medium

### What `subject` is

`subject` is a **simple string label** (not a node ID) that names the entity a memory is *about*. It is a lightweight, denormalized tag that enables entity-centric retrieval without requiring a full entity graph.

Examples:
```yaml
subject: "Alice Chen"          # a person
subject: "mem project"         # a project
subject: "Rust async patterns" # a topic
subject: "user"                # the PKB owner themselves
```

### How it relates to other nodes

`subject` is deliberately **not a node ID** or a graph reference. It's a flat string stored in frontmatter, indexed for filtering, and searchable. This is the right trade-off because:

1. **Low-friction capture**: When an agent creates a memory like "Alice prefers morning meetings", it doesn't need to look up or create an entity node for Alice. It just writes `subject: "Alice"`.
2. **Emergent structure**: Subjects with many memories naturally surface as important entities via `aops tags`-style frequency analysis, without needing upfront schema design.
3. **Retrieval by entity**: "What do I know about Alice?" becomes a simple filter query (`list_memories --subject "Alice"`) rather than requiring graph traversal.
4. **Compatible with future entity nodes**: If entity nodes are added later, `subject` strings can be resolved to node IDs via the same fuzzy ID resolution that `GraphStore::resolve()` already provides. A future `EdgeType::About` could link memories to entity nodes — but that's additive, not required now.

### Relationship to existing fields

| Field | Purpose | Example |
|-------|---------|---------|
| `tags` | Categorical labels (many per doc) | `[meeting, preference]` |
| `project` | Organizational grouping | `mem` |
| `parent` | Hierarchical containment (node ID) | `task-abc` |
| `subject` | **What this memory is about** (one entity) | `"Alice Chen"` |

The key distinction: `tags` classify the memory, `subject` identifies the entity. "Alice prefers morning meetings" is tagged `[preference, scheduling]` with subject `"Alice"`.

### Implementation

1. **`document_crud.rs`**: Add `subject: Option<String>` to `MemoryFields` and `DocumentFields`. Emit in frontmatter.
2. **`graph.rs`**: Add `subject: Option<String>` to `GraphNode`, parsed from frontmatter.
3. **`vectordb.rs`**: Add `subject: Option<String>` to `DocumentEntry`. Add subject filtering to `list_documents()` and `search()`.
4. **`mcp_server.rs`**:
   - Add `subject` param to `create_memory`, `create`, `retrieve_memory`, `list_memories` tool schemas
   - `retrieve_memory` with `subject` filter returns only memories about that entity
   - Add `list_subjects` tool: returns subject frequency summary (like `list_all_tags`)
5. **`cli.rs`**: Add `--subject` filter to `recall`, `memories`, `list` commands.

### Acceptance criteria

- Memories can be created with a `subject` field
- `retrieve_memory` accepts optional `subject` filter
- `list_memories` accepts optional `subject` filter
- New `list_subjects` tool/command shows all subjects with counts
- Subject is displayed in memory retrieval results

---

## 3. Semantic chunking: paragraph-level chunks for markdown

**Priority:** High — Immediate
**Complexity:** Medium

### Problem

The current chunking strategy (`embeddings.rs:ChunkConfig`) uses fixed-size character windows (2000 chars with 500 char overlap) with sentence-break heuristics. This works but has two problems for a markdown PKB:

1. **Splits semantic units**: A paragraph or list discussing a single idea gets split across chunks, diluting its embedding.
2. **Mixes unrelated content**: A chunk spanning the end of one section and beginning of another produces an embedding that represents neither well.

### Design

Replace the fixed-size chunker with a **markdown-aware semantic chunker** that treats natural markdown structures as chunk boundaries:

- **Paragraphs** (text separated by blank lines) → one chunk each
- **List blocks** (contiguous `- ` or `* ` or `1. ` lines, including nested items) → one chunk per top-level list block
- **Headings + their content** → heading text is prepended to each chunk in its section for context
- **Code blocks** (fenced ``` or indented) → one chunk each
- **Frontmatter** → excluded from chunks (already handled separately in `embedding_text()`)

Size guardrails:
- **Max chunk**: ~1500 chars. If a paragraph or list exceeds this, fall back to sentence-level splitting within it.
- **Min chunk**: ~100 chars. Merge very short paragraphs with the next paragraph.
- **No overlap needed**: Since each chunk is a complete semantic unit, overlap is unnecessary and wastes tokens.

### Implementation

1. **`embeddings.rs`**: Add `chunk_markdown(text: &str) -> Vec<String>` function that:
   - Splits on blank lines to get raw blocks
   - Classifies each block (heading, paragraph, list, code fence, etc.)
   - Tracks current heading context (prepended to chunks)
   - Merges small blocks, splits oversized blocks
   - Returns a `Vec<String>` of semantic chunks
2. **`embeddings.rs`**: Add `ChunkStrategy` enum (`FixedSize`, `Semantic`) to `ChunkConfig`. Default to `Semantic` for new code, keep `FixedSize` available for backward compatibility.
3. **`pkb.rs`**: `embedding_text()` already prepends title/type/tags. The new chunker operates on the body portion.
4. **`vectordb.rs`**: No changes needed — it already stores arbitrary chunk lists.
5. **Reindex**: After deploying, run `aops reindex --force` to rebuild all embeddings with the new chunking.

### Testing

- Unit test: chunk a markdown document with mixed headings, paragraphs, lists, and code blocks. Verify each chunk is a complete semantic unit.
- Unit test: oversized paragraph gets sub-split at sentence boundaries.
- Unit test: short consecutive paragraphs get merged.
- Integration: compare retrieval quality (cosine scores) on a sample PKB before/after.

### Acceptance criteria

- Markdown documents are chunked at paragraph/list/code-block boundaries
- Heading context is preserved in each chunk
- Very short blocks are merged, very long blocks are sub-split
- `aops reindex --force` rebuilds with new chunking
- Old `ChunkConfig::default()` fixed-size strategy still available via config

---

## 4. Research: Memory decay, cementification, and co-retrieval edges

**Priority:** Medium — Requires investigation and experimentation
**Complexity:** Large (research)

### Background

Current retrieval ranks memories purely by cosine similarity. This ignores temporal relevance (old observations may be stale) and usage patterns (frequently accessed memories are probably important). Three mechanisms from cognitive science and recent AI memory research (particularly [shodh-memory](https://github.com/varun29ankuS/shodh-memory)) could improve retrieval quality.

### 4a. Memory Decay

**Concept**: Memories lose relevance over time unless refreshed. A memory from 2 years ago should rank lower than an equally similar memory from yesterday, unless it has been recently accessed.

**Questions to investigate:**
- What decay function? Exponential (`score * e^(-λ * age_days)`) or logarithmic (`score * 1/(1 + log(age_days))`)? Exponential is standard in spaced repetition; logarithmic is gentler.
- What time constant (λ)? Too aggressive and old-but-important memories vanish. Too gentle and it has no effect.
- Should decay apply to all document types or only memories/observations? Tasks have their own lifecycle (status). Notes and knowledge documents may be reference material that shouldn't decay.
- How to compute age? From `created` frontmatter? From file mtime? From `last_accessed` (requires tracking access)?
- **Implementation options**: (a) Apply decay at query time as a scoring modifier (no storage changes, easy to tune); (b) Periodically recompute a `relevance` score stored in frontmatter (visible to users but requires background jobs).

**Proposed experiment**: Add a query-time decay modifier behind a feature flag. Use `created` date as the age signal. Test with exponential decay at λ=0.01 (half-life ~70 days). Measure whether it improves retrieval relevance on a real PKB with >100 memories of varying ages.

### 4b. Cementification

**Concept**: Memories that are accessed frequently (above a small threshold, e.g. 3–5 retrievals) become "cemented" — they stop decaying. This captures the intuition that if you keep coming back to a piece of knowledge, it's a core fact, not a transient observation.

**Questions to investigate:**
- What's the access threshold? Too low (1-2) and everything cements. Too high (10+) and it never triggers. shodh-memory uses a threshold approach.
- How to track access count? Options: (a) `access_count` field in frontmatter (updated on every retrieval — high write load); (b) `access_count` in the in-memory `DocumentEntry` (lost on restart unless persisted); (c) Separate access log file/DB.
- Should cementification be binary (decays/doesn't decay) or gradual (decay rate decreases with access count)?
- Does cementification interact with `supersedes`? If memory A is superseded by B, should A's access count transfer to B?

**Proposed experiment**: Track access counts in `DocumentEntry` (option b), persist alongside the vector store. After implementing decay, add a `cemented: bool` flag computed as `access_count >= 5`. Cemented memories skip the decay modifier. Measure the effect on a real PKB over 2 weeks.

### 4c. Co-retrieval Edges

**Concept**: When two memories are frequently retrieved together in the same query context, they are likely related even if they don't have explicit links. A co-retrieval edge captures this emergent relationship for graph-proximity boosting.

This is implemented in [shodh-memory](https://github.com/varun29ankuS/shodh-memory) where memories that appear together in search results build association strength over time.

**Questions to investigate:**
- When to create a co-retrieval edge? Options: (a) When two memories appear in the same search result set (top-N); (b) When a user explicitly opens/reads two memories in the same session; (c) When two memories are in the system prompt simultaneously.
- Edge weight: Should it be a count (# of co-retrievals) or a decaying score?
- How to store? A new `EdgeType::CoRetrieved` in the graph with a weight field on `Edge`? Or a separate adjacency structure?
- Performance concern: Updating co-retrieval edges on every search adds write overhead. Batch updates? Async?
- How does this interact with the existing `boost_id` graph proximity in `handle_pkb_search`?

**Proposed experiment**: Add a `CoRetrieved` edge type with integer weight. On each `retrieve_memory` call, for each pair of results in the top-5, increment their co-retrieval weight by 1. Use co-retrieval weight as a secondary boost in hybrid search (similar to existing `boost_map`). Measure whether repeated queries on a real PKB surface increasingly relevant clusters of memories over time.

### Deliverables for this research task

1. **Design doc**: Written analysis of decay/cementification/co-retrieval trade-offs with specific parameter recommendations for mem's scale (~100–10,000 memories).
2. **Prototype**: Query-time decay modifier behind a flag, with access counting in DocumentEntry. No co-retrieval edges yet (most complex, least certain value).
3. **Evaluation**: Before/after retrieval quality comparison on a real PKB. Define metrics: mean reciprocal rank (MRR) on a set of test queries with known relevant memories.

---

## 5. Embed "context engineering" principle in framework and documentation

**Priority:** High — Ongoing
**Complexity:** Small (code review + docs)

### Principle

> LLMs are fundamentally text-in, text-out systems. Rather than hard-coding human-like memory structures, focus on context engineering — managing the information available to the model at inference time.

This means:

1. **Memory format stays natural language markdown.** The body of every memory is free-form prose that an LLM can consume directly. No JSON schemas, no structured data formats for the content itself. Structure lives in the YAML frontmatter metadata layer — it's for *filtering and routing*, not for the LLM to parse.

2. **Retrieval is context assembly.** The `retrieve_memory` tool's job is to assemble the best possible context window for the LLM, not to return a normalized data structure. This means: prefer returning full bodies, include enough context for the LLM to reason, and let the LLM synthesize across multiple memories.

3. **Avoid premature structure.** Don't add typed schemas, ontologies, or rigid categorization systems. Let structure emerge from usage patterns (tags, subjects, co-retrieval edges). The graph is for *finding connections*, not for enforcing a worldview.

4. **Simple beats clever.** ChatGPT's memory is a flat list of facts injected into the system prompt. Letta's filesystem agent beat graph-based memory systems on benchmarks. Start with the simplest approach that works and add complexity only when retrieval quality measurably improves.

### Implementation

1. Add this principle as a section in `README.md` under "Design Philosophy".
2. Add it to `.agent/CORE.md` so AI agents working on this codebase understand the constraint.
3. Review existing code for violations — are there places where we've over-structured the memory format? (Current review: no significant violations found. The frontmatter metadata layer is appropriately lightweight.)
4. Apply this lens to all future feature proposals. Every new field or structure must justify itself by improving retrieval quality or reducing token cost.

---

## 6. Improve `retrieve_memory` with confidence-weighted, recency-aware scoring

**Priority:** Medium — After items 1–3
**Complexity:** Medium

### Problem

`retrieve_memory` currently returns memories sorted purely by cosine similarity. With `confidence`, `subject`, `supersedes`, and (eventually) decay, the scoring function needs to become a composite.

### Design

Replace the simple cosine sort with a composite scoring function:

```
final_score = cosine_similarity
            * confidence_weight(confidence)
            * recency_weight(age_days)
            * (1.0 + graph_proximity_boost)
```

Where:
- `confidence_weight(c)` = `c` (linear, since confidence is already 0–1)
- `recency_weight(age)` = `1.0 / (1.0 + 0.01 * age_days)` (gentle decay, configurable)
- `graph_proximity_boost` = existing `boost_map` logic from `handle_pkb_search`, extended to `retrieve_memory`

Additionally:
- Filter out superseded memories by default (add `include_superseded: bool` param, default false)
- When a superseded memory is included, annotate it with "superseded by: {title}"

### Implementation

Depends on items 1 (confidence/supersedes fields) and 4a (decay research).

---

## 7. Add `memory_type` taxonomy and retrieval hints to tool descriptions

**Priority:** Low — Polish
**Complexity:** Small

### Problem

The `create_memory` tool description says `memory_type` can be "memory, note, insight, observation" but doesn't explain when to use each. Agents using the MCP server have to guess.

### Design

Update tool descriptions with clear guidance:

- **`observation`**: A specific fact or data point learned from a conversation or source. Short, atomic, objective. Example: "User's company uses PostgreSQL 15."
- **`insight`**: A synthesis or inference drawn from multiple observations. Example: "User seems to prefer functional programming patterns based on their code style and library choices."
- **`memory`**: A general-purpose memory that doesn't fit other categories. Example: "Had a productive discussion about API design on 2025-02-20."
- **`note`**: Reference material or documentation saved for later. Example: "Steps to deploy to production: 1. Run tests..."

Also add retrieval guidance to the `retrieve_memory` tool description: "Use this tool to check what you already know before asking the user. Good practice: retrieve memories at the start of a new conversation to build context."

---

## Summary

| # | Item | Priority | Complexity | Dependencies |
|---|------|----------|------------|-------------|
| 1 | `confidence`, `source`, `supersedes` fields | High | Small | None |
| 2 | `subject` field | High | Medium | None |
| 3 | Semantic chunking | High | Medium | None |
| 4 | Research: decay, cementification, co-retrieval | Medium | Large | None (but informs #6) |
| 5 | Context engineering principle in docs | High | Small | None |
| 6 | Composite retrieval scoring | Medium | Medium | #1, #4 |
| 7 | Memory type taxonomy in tool descriptions | Low | Small | None |

Items 1, 2, 3, and 5 can proceed in parallel immediately. Item 4 is research that can run concurrently. Items 6 and 7 follow after.
