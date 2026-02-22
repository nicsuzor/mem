# mem — Developer Reference

## Overview

Semantic search + knowledge graph MCP server over a personal knowledge base (PKB) of markdown files with YAML frontmatter. Built in Rust.

## Architecture

```
src/
  server.rs       — main() for pkb-search binary (MCP stdio transport)
  cli.rs          — main() for aops CLI binary
  mcp_server.rs   — MCP ServerHandler: 18 tools, dispatch, tool registrations
  graph_store.rs  — GraphStore: builds/queries knowledge graph from PKB docs
  graph.rs        — GraphNode, Edge, EdgeType, link resolution helpers
  vectordb.rs     — VectorStore: cosine similarity search over embeddings
  embeddings.rs   — Embedder: MiniLM-L6-v2 via ONNX Runtime (384-dim)
  pkb.rs          — PkbDocument parsing, directory scanning
  document_crud.rs— Create/update/delete/append for markdown docs
  metrics.rs      — Network centrality (PageRank, betweenness, degree)
  task_index.rs   — MCP index export (used by CLI graph command)
```

## MCP Tools (18, as of v0.1.5)

### Search
- `search` — hybrid semantic + graph-proximity (was `pkb_search`)
- `get_document` — read full file contents
- `list_documents` — browse/filter with pagination
- `reindex` — full re-scan + rebuild graph

### Tasks
- `task_search` — semantic search filtered to tasks
- `list_tasks` — list with filters; `status="ready"` and `status="blocked"` are special
- `get_task` — returns frontmatter + body + relationship context (depends_on, blocks, children, parent)
- `create_task` — new task with frontmatter
- `update_task` — patch frontmatter fields
- `complete_task` — set status=done
- `get_network_metrics` — centrality metrics for a node

### Document CRUD
- `create` — generic document creation (was `create_document`)
- `create_memory` — memory/note creation
- `append` — timestamped append to existing doc (was `append_to_document`)
- `delete` — remove doc from disk + index (was `delete_document`)

### Knowledge Graph
- `pkb_context` — N-hop neighbourhood, backlinks, metadata
- `pkb_trace` — shortest paths between two nodes
- `pkb_orphans` — disconnected nodes

## Key Patterns

### Tool dispatch
`mcp_server.rs` uses a manual `match` in `call_tool()` mapping tool name strings to `handle_*` methods. Tool registrations are in `list_tools()` as a `Vec<Tool>`. Both must stay in sync.

### Graph rebuild
After any CRUD operation, `rebuild_graph()` is called to rebuild the full `GraphStore` from disk. The graph also persists to `{db_path}.graph.json` but rebuilds fast (~300ms).

### Flexible ID resolution
`GraphStore::resolve(query)` tries: exact ID match -> case-insensitive resolution map (id, task_id, filename stem, title, permalink). Used by most task/document tools.

### Status filters in list_tasks
`status="ready"` uses `graph.ready_tasks()` (leaf tasks with no unmet deps, not done/cancelled, not learn type). `status="blocked"` uses `graph.blocked_tasks()` (tasks with unmet deps or explicitly blocked status). Other status values use case-insensitive string match on frontmatter status.

## Build

```bash
make            # native Linux x86_64
make apple      # cross-compile to aarch64-apple-darwin (requires zig 0.13 + cargo-zigbuild)
make release    # bump patch, build both, install, tag, push
```

Requires: Rust >= 1.88, zig 0.13 for cross-compile.

## Consolidation History (v0.1.5)

22 tools consolidated to 18:
- **Removed**: `semantic_search` (superseded by hybrid `search`), `get_ready_tasks`, `get_blocked_tasks` (merged into `list_tasks`), `get_task_network` (merged into `get_task`)
- **Renamed**: `pkb_search` -> `search`, `create_document` -> `create`, `append_to_document` -> `append`, `delete_document` -> `delete`
- **Enhanced**: `get_task` now includes relationship context; `list_tasks` supports `status=ready` and `status=blocked` with specialized output formats

Spec reference: `/opt/nic/academicOps/specs/pkb-server-spec.md`
