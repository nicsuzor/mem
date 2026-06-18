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
  cli.rs           — main() for pkb binary (CLI + MCP server via `pkb mcp`)
  mcp_server.rs    — MCP ServerHandler: 39 tools, dispatch, tool registrations
  graph_store.rs   — GraphStore: builds/queries knowledge graph from PKB docs
  graph.rs         — GraphNode (fields include stakeholder, waiting_since), Edge, EdgeType, link resolution helpers
  graph_display.rs — Graph rendering/display utilities
  vectordb.rs      — VectorStore: cosine similarity search over embeddings
  embeddings.rs    — Embedder: BGE-M3 via ONNX Runtime (1024-dim)
  pkb.rs           — PkbDocument parsing, directory scanning
  document_crud.rs — Create/update/delete/append for markdown docs
  metrics.rs       — Network centrality (PageRank, betweenness, degree)
  task_index.rs    — MCP index export (used by CLI graph command)
  distance.rs      — Distance/similarity calculations
  eval.rs          — Evaluation utilities
  lint.rs          — PKB linting/validation
  reproduction.rs  — Reproduction/test helpers
  lib.rs           — Library root
```

## MCP Tools (39)

### Search
- `search` — hybrid semantic + graph-proximity
- `get_document` — read full file contents
- `list_documents` — browse/filter with pagination
- `find_duplicates` — detect duplicate documents

### Tasks
- `task_search` — semantic search filtered to tasks
- `list_tasks` — list with filters; `status="ready"` and `status="blocked"` are special
- `get_task` — frontmatter + body + relationship context
- `create_task` — new task with frontmatter
- `create_subtask` — subtask under a parent
- `claim_task` — instantiate a `type: template` node; creates a datestamped instance and returns it via get_task
- `update_task` — patch frontmatter fields
- `complete_task` — set status=done; returns `{ok, id, status, message, neighborhood}` JSON envelope with compact graph context (see `specs/mutation-neighborhood.md`)
- `release_task` — terminal status release and session handover (populates session_id, pr_url, issue_url, follow_up_tasks, release_summary); handles ad-hoc creation if ID omitted; returns `{ok, id, status, message, neighborhood}` JSON envelope with compact graph context (see `specs/mutation-neighborhood.md`)
- `decompose_task` — batch create subtasks under a parent
- `get_dependency_tree` — upstream/downstream dependency tree
- `get_task_children` — direct/recursive children with completion counts
- `get_network_metrics` — centrality metrics for a node
- `top_n_by_metric` — top N nodes ranked by centrality metric (pagerank, betweenness, degree), with optional node_type filter
- `task_summary` — summary statistics for tasks

### Memory
- `create_memory` — create memory/note
- `retrieve_memory` — semantic search filtered to memory types
- `search_by_tag` — find documents by tag intersection
- `list_memories` — list memory-type documents
- `delete_memory` — delete a memory

### Document CRUD
- `create` — generic document creation
- `append` — timestamped append to existing doc
- `delete` — remove doc from disk + index

### Knowledge Graph
- `pkb_context` — N-hop neighbourhood, backlinks, metadata
- `pkb_trace` — shortest paths between two nodes
- `pkb_orphans` — disconnected nodes
- `graph_stats` — graph statistics
- `refresh_graph` — synchronously rebuild in-memory graph index from disk (no ONNX re-embed)

### Batch Operations
- `batch_update` — bulk update frontmatter fields
- `batch_reparent` — bulk reparent tasks
- `batch_archive` — bulk archive documents
- `batch_merge` — merge multiple documents
- `batch_create_epics` — batch create epic tasks
- `batch_reclassify` — batch reclassify document types
- `bulk_reparent` — reparent tasks in bulk
- `merge_node` — merge a single node into another

## Key Patterns

### Tool dispatch
`mcp_server.rs` uses a manual `match` in `call_tool()` mapping tool name strings to `handle_*` methods. Tool registrations are in `list_tools()` as a `Vec<Tool>`. Both must stay in sync.

### Graph rebuild — in-place patch + coalesced background rebuild
Single-doc CRUD operations patch the graph in place on the request thread and defer the full pipeline to a coalesced background worker.

- **Synchronous patch (request thread)**: `rebuild_graph_for_pkb_document` takes the graph write lock, calls `GraphStore::upsert_node_in_place` (HashMap insert + resolution-map patch + carry-over of derived fields from the prior node — pagerank, downstream_weight, target_ancestors, children, etc.), then runs cheap `reclassify()` (O(V+E)). The patched node id is recorded in `patched_during_rebuild` so a concurrent in-flight background rebuild re-applies it on swap. Direct reads after a write see the patched node by id, with fresh `status`/`parent`/`priority` and fresh ready/blocked *membership*.
- **Background rebuild (coalesced)**: `schedule_graph_rebuild` dispatches a full `rebuild_from_nodes_fast_with_embeddings` (downstream metrics, urgency, focus scores, target ancestors, similarity edges) to `tokio::task::spawn_blocking`. `graph_rebuild_pending: AtomicBool` + `graph_rebuild_dirty: AtomicBool` collapse burst writes to a single in-flight rebuild plus at most one queued follow-up. The worker carries forward late-arriving patches on swap (lost-patch protection) and runs `classify_tasks` again.

**What lags after a write returns**: derived metric *values* (downstream_weight, effective_priority, focus_score, urgency ordering, similarity edges, target_ancestors for new nodes) — the prior values are carried over, so they're not "wrong" so much as "as of last rebuild". Body search lags too, because ONNX re-embed is deferred to `embed_pending` (see below). Background rebuild typically completes in 1–3s on a multi-thousand-node PKB.

### Deferred ONNX re-embed
`try_upsert_document` no longer blocks on ONNX. For existing entries, the metadata patch (`title`, `status`, `tags`, `id`, …) is applied synchronously and the body re-embed is queued onto `embed_pending: HashMap<path_key, PkbDocument>` for a single background worker (`embed_worker_running: AtomicBool`). Bursts of writes to the same document collapse to one embed (latest body wins). For new documents (no existing entry yet), the inline embed still runs so the entry exists at all — `MetadataOnly` apply drops the patch when no entry exists.

Bulk paths (`rebuild_graph()`, batch finalize, reindex) still run the full synchronous pipeline. The graph is **in-memory only** — it does not persist; it is reconstructed at startup via `GraphStore::build_from_directory` (~300 ms for a typical PKB). Only the vector store persists to `{db_path}` (bincode) plus `{db_path}.lock` for the cross-process advisory lock.

### Flexible ID resolution
`GraphStore::resolve(query)` tries: exact ID match -> case-insensitive resolution map (id, task_id, filename stem, title, permalink). Used by most task/document tools.

### Status filters in list_tasks
`status="ready"` = leaf tasks with no unmet deps, not done/cancelled, not learn type. `status="blocked"` = tasks with unmet deps or explicitly blocked status.

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

Requires: Rust >= 1.88, zig 0.13 for cross-compile.
