# mem — Developer Reference

## Overview

Semantic search + knowledge graph MCP server over a personal knowledge base (PKB) of markdown files with YAML frontmatter. Built in Rust.

## Architecture

```
src/
  server.rs       — main() for pkb binary (MCP stdio transport)
  cli.rs          — main() for aops CLI binary
  mcp_server.rs   — MCP ServerHandler: 18 tools, dispatch, tool registrations
  graph_store.rs  — GraphStore: builds/queries knowledge graph from PKB docs
  graph.rs        — GraphNode, Edge, EdgeType, link resolution helpers
  vectordb.rs     — VectorStore: cosine similarity search over embeddings
  embeddings.rs   — Embedder: BGE-M3 via ONNX Runtime (1024-dim)
  pkb.rs          — PkbDocument parsing, directory scanning
  document_crud.rs— Create/update/delete/append for markdown docs
  metrics.rs      — Network centrality (PageRank, betweenness, degree)
  task_index.rs   — MCP index export (used by CLI graph command)
```

## MCP Tools (18, as of v0.1.15)

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
- `decompose_task` — batch create subtasks under a parent
- `get_dependency_tree` — upstream/downstream dependency tree traversal
- `get_task_children` — direct/recursive children with completion counts

### Memory
- `retrieve_memory` — semantic search filtered to memory/note/insight/observation types
- `search_by_tag` — find documents by tag intersection
- `list_memories` — list memory-type documents with optional tag filter
- `delete_memory` — delete a memory (validates type before deletion)

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

## Build & Install

```bash
cargo install --path .    # install both binaries (aops + pkb) to ~/.cargo/bin
make            # native Linux x86_64 (release build only, no install)
make apple      # cross-compile to aarch64-apple-darwin (requires zig 0.13 + cargo-zigbuild)
make release    # bump patch, build both, install, tag, push
```

**Local install**: Always use `cargo install --path .` — never manually copy binaries to /usr/local/bin.

Requires: Rust >= 1.88, zig 0.13 for cross-compile.

## UX Testing / TUI Capture

The TUI uses Ratatui + crossterm (alternate screen, raw mode) so it requires a real terminal. Use tmux to capture rendered output for evaluation:

### Helper script (preferred)
```bash
./scripts/tui-capture.sh start     # cargo build --release + launch in tmux (120x40)
./scripts/tui-capture.sh capture   # grab current screen with ANSI codes
./scripts/tui-capture.sh key f     # send key + capture (f=Focus, g=Graph, t=Tree, d=Dashboard)
./scripts/tui-capture.sh restart   # rebuild + relaunch after code changes
./scripts/tui-capture.sh stop      # kill tmux session
```

### Manual approach
```bash
# Launch with stderr capture for crash debugging
tmux new-session -d -s aops-tui -x 120 -y 40 \
  './target/release/aops tui 2>/tmp/tui-stderr.log'
sleep 2  # wait for graph load

# Capture current screen
tmux capture-pane -t aops-tui -p -e

# Send keys and capture result
tmux send-keys -t aops-tui '1' && sleep 0.5 && tmux capture-pane -t aops-tui -p -e

# Cleanup
tmux kill-server
```

### Tips
- Always `sleep 0.3-0.5` after `send-keys` before capturing — rendering is async
- If the tmux session dies unexpectedly, check `/tmp/tui-stderr.log` for panics
- View keys: `f` Focus, `g` Graph, `t` Epic Tree, `d` Dashboard
- Overlay keys: `?` Help, `/` Search, `q` Quick capture, `Enter` Detail
- Filter keys: `1`/`2`/`3` priority filter, `C` show completed, `T` type filter

### String safety (Rust)
Always use `str.floor_char_boundary(n)` before byte-slicing strings for display truncation. Direct `&str[..n]` panics on multi-byte UTF-8 characters (arrows, emoji, CJK). Available since Rust 1.80.

## Consolidation History (v0.1.5)

22 tools consolidated to 18:
- **Removed**: `semantic_search` (superseded by hybrid `search`), `get_ready_tasks`, `get_blocked_tasks` (merged into `list_tasks`), `get_task_network` (merged into `get_task`)
- **Renamed**: `pkb_search` -> `search`, `create_document` -> `create`, `append_to_document` -> `append`, `delete_document` -> `delete`
- **Enhanced**: `get_task` now includes relationship context; `list_tasks` supports `status=ready` and `status=blocked` with specialized output formats

Spec reference: see `README.md`
