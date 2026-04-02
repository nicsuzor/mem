# mem — Developer Reference

## Overview

Semantic search + knowledge graph MCP server for a personal knowledge base (PKB) of markdown files with YAML frontmatter. Built in Rust. Version 0.3.x.

## Architecture

```
src/
  cli.rs           — main() for pkb binary (CLI + MCP server via `pkb mcp`)
  mcp_server.rs    — MCP ServerHandler: 36 tools, dispatch, tool registrations
  graph_store.rs   — GraphStore: builds/queries knowledge graph from PKB docs
  graph.rs         — GraphNode, Edge, EdgeType, link resolution helpers
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

## MCP Tools (36)

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
- `update_task` — patch frontmatter fields
- `complete_task` — set status=done
- `decompose_task` — batch create subtasks under a parent
- `get_dependency_tree` — upstream/downstream dependency tree
- `get_task_children` — direct/recursive children with completion counts
- `get_network_metrics` — centrality metrics for a node
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

### Graph rebuild
After any CRUD operation, `rebuild_graph()` rebuilds the full `GraphStore` from disk. The graph persists to `{db_path}.graph.json` but rebuilds fast (~300ms).

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

## UX Testing / TUI Capture

The TUI uses Ratatui + crossterm. Use the helper script:
```bash
./scripts/tui-capture.sh start|capture|key <k>|restart|stop
```
View keys: `f` Focus, `g` Graph, `t` Tree, `d` Dashboard. Always `sleep 0.3-0.5` after `send-keys` before capturing.
