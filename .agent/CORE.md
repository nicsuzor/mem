# mem — Developer Reference

## Specifications (SSoT)

**Always check the specs directory first** for design intent, feature specs, methodology, or architecture decisions. The canonical home for specifications is `specs/`. Specs describe approved current state only. Agents are forbidden from writing change memos, ADRs, or decision records that narrate motion.

## Path Discovery

To discover project locations, read `.agents/INDEX.md` in the relevant repo. A missing or stale index is NOT a hard HALT — if you are already inside a mounted worktree, fall back to the repo's README/CLAUDE.md and top-level layout, and refresh the index where you can.

## Fail-Fast / Halt Rule (ENFORCED)

If you cannot do what was asked, **STOP and report** — do NOT search broadly, do NOT invent workarounds.

- **Missing Paths**: If a documented path does not exist, HALT.
- **No Broad Grep**: Never grep `$HOME` or `/` to find source repos or documents. Use `.agents/INDEX.md` for discovery.
- **Tool Failures**: If a tool doesn't work as documented, report the failure — do not invent alternatives.
- **Ambiguity**: If instructions conflict or are ambiguous, ask for clarification.

## Overview

Semantic search + knowledge graph MCP server for a personal knowledge base (PKB) of markdown files with YAML frontmatter. Built in Rust. Version 0.3.x.

## Architecture

```
src/
  cli.rs            — main() for pkb binary (CLI + MCP server via `pkb mcp`)
  mcp_server/       — MCP server module: tool schemas, dispatch, handlers
  graph_store.rs    — GraphStore: builds/queries knowledge graph from PKB docs
  graph.rs          — GraphNode (fields include stakeholder, waiting_since), Edge, EdgeType, link resolution helpers
  graph_display.rs  — Graph rendering/display utilities
  vectordb.rs       — VectorStore: cosine similarity search over embeddings
  embeddings.rs     — Embedder: BGE-M3 via ONNX Runtime (1024-dim)
  pkb.rs            — PkbDocument parsing, directory scanning
  polecat_config.rs — polecat.yaml project-slug registry (validation + alias resolution)
  document_crud.rs  — Create/update/delete/append for markdown docs
  metrics.rs        — Network centrality (PageRank, betweenness, degree)
  task_index.rs     — MCP index export (used by CLI graph command)
  distance.rs       — Distance/similarity calculations
  eval.rs           — Evaluation utilities
  lint.rs           — PKB linting/validation
  path_lint.rs      — rejects new machine-specific paths to PKB files (`~/brain/...`) in bodies written via MCP
  lsp.rs            — PKB Language Server Protocol (LSP) server (hover preview & goto-def/links)
  reproduction.rs   — Reproduction/test helpers
  lib.rs            — Library root
editors/
  vscode/           — VS Code LSP client extension (hover preview & open referenced files in new tab)
```

## Key Patterns

### Tool dispatch
`src/mcp_server/mod.rs` dispatches tool calls mapping tool name strings to handlers in `src/mcp_server/handlers_*.rs`. Tool registrations are in `src/mcp_server/schemas.rs`. Both must stay in sync.

### Graph rebuild — in-place patch + coalesced background rebuild
Single-doc CRUD operations patch the graph in place on the request thread and defer the full pipeline to a coalesced background worker.

- **Synchronous patch (request thread)**: `rebuild_graph_for_pkb_document` takes the graph write lock, calls `GraphStore::upsert_node_in_place` (HashMap insert + resolution-map patch + carry-over of derived fields from the prior node — pagerank, betweenness, degrees, target_ancestors, children, etc.), then runs cheap `reclassify()` (O(V+E)). The patched node id is recorded in `patched_during_rebuild` so a concurrent in-flight background rebuild re-applies it on swap. Direct reads after a write see the patched node by id, with fresh `status`/`parent`/`priority` and fresh ready/blocked *membership*.
- **Background rebuild (coalesced)**: `schedule_graph_rebuild` dispatches a full `rebuild_from_nodes_fast_with_embeddings` (flow figures, target ancestors, similarity edges) to `tokio::task::spawn_blocking`. `graph_rebuild_pending: AtomicBool` + `graph_rebuild_dirty: AtomicBool` collapse burst writes to a single in-flight rebuild plus at most one queued follow-up. The worker carries forward late-arriving patches on swap (lost-patch protection) and reclassifies ready/blocked/roots again (`display_rank::classify_graph_nodes`).

**What lags after a write returns**: derived metric *values* (flow figures — `gain`, `loss_averted`, `decision_value` — and therefore display order, similarity edges, target_ancestors for new nodes). Centrality and structural inverses are carried over from the prior node, so they're not "wrong" so much as "as of last rebuild"; the patched node's own `flow` is empty until the background rebuild lands. Body search lags too, because ONNX re-embed is deferred to `embed_pending` (see below). Background rebuild typically completes in 1–3s on a multi-thousand-node PKB.

### Deferred ONNX re-embed
`try_upsert_document` no longer blocks on ONNX. For existing entries, the metadata patch (`title`, `status`, `tags`, `id`, …) is applied synchronously and the body re-embed is queued onto `embed_pending: HashMap<path_key, PkbDocument>` for a single background worker (`embed_worker_running: AtomicBool`). Bursts of writes to the same document collapse to one embed (latest body wins). For new documents (no existing entry yet), the inline embed still runs so the entry exists at all — `MetadataOnly` apply drops the patch when no entry exists.

Bulk paths (`rebuild_graph()`, batch finalize, reindex) still run the full synchronous pipeline. The graph is **in-memory only** — it does not persist; it is reconstructed at startup via `GraphStore::build_from_directory` (~300 ms for a typical PKB). Only the vector store persists to `{db_path}` (bincode) plus `{db_path}.lock` for the cross-process advisory lock.

### Self-invalidating graph cache (validate-on-read)
Read paths automatically validate the cached in-memory graph against on-disk state via `ensure_graph_fresh()` (`mem_65c8e8fe` / `mem_3c018681`). A fast stat-only scan (`scan_generation()`, ~32–45ms for 8,000 files) computes the current disk `GenerationStamp` (file count + max mtime). If an external write occurred (git-sync sidecar pull, CLI mutation, or direct editor change), the mismatch triggers an automatic in-place graph rebuild (`rebuild_graph()`) before the read executes. `refresh_graph` remains available as an explicit escape hatch.

### Flexible ID resolution
`GraphStore::resolve(query)` tries: exact ID match -> case-insensitive resolution map (id, task_id, filename stem, title, permalink). Used by most task/document tools.

### Status filters in list_tasks
`status=<S>` matches the node's stored frontmatter `status` field exactly (case-insensitive) — never a computed set. `status="ready"` matches only nodes literally stored `status: ready`; `status="blocked"` matches only nodes literally stored `status: blocked` (the computed unmet-deps view is exposed separately as the top-level `blocked` boolean on each row). An unsupported status value is rejected rather than silently aliased to a different set (e.g. `status="archived"` no longer returns `done`).

### String safety (Rust)
Always use `str.floor_char_boundary(n)` before byte-slicing strings for display truncation. Direct `&str[..n]` panics on multi-byte UTF-8.

## Build & Install

```bash
cargo install --path .    # install pkb binary to ~/.cargo/bin
make            # native Linux x86_64 (release build only, no install)
make apple      # cross-compile to aarch64-apple-darwin (requires zig 0.13 + cargo-zigbuild)
make release    # bump patch, build both, install, tag, push
```

**Local install**: Always use `cargo install --path .` — never manually copy binaries to /usr/local/bin.

Requires: Rust >= 1.91, zig 0.13 for cross-compile.
