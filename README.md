# mem

Semantic search and knowledge graph over personal knowledge base markdown files, exposed as an MCP server and CLI.

Uses [MiniLM-L6-v2](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2) for 384-dimensional sentence embeddings via ONNX Runtime. Models and runtime are auto-downloaded on first run.

## Quick Start

```bash
# Build both binaries
cargo build --release

# Run MCP server (stdio transport)
./target/release/pkb

# Run CLI
./target/release/aops search "my query"
```

## CLI Commands

The `aops` binary provides direct access to search, task management, and graph analysis.

### Search & Index

| Command | Description |
|---------|-------------|
| `aops search <query> [-n limit] [--full]` | Semantic search across the knowledge base |
| `aops add <files...>` | Add markdown files to the index |
| `aops list [--tag T] [--type T] [--status S] [--count]` | List indexed documents with optional filters |
| `aops reindex [--force]` | Re-scan and re-index all PKB files |
| `aops status` | Show index statistics (document count, DB size) |

### Task Management

| Command | Description |
|---------|-------------|
| `aops tasks [ready\|blocked\|all] [--project P] [--sort S]` | List tasks sorted by priority + downstream weight |
| `aops task <id>` | Show task details and relationships |
| `aops new <title> [--parent ID] [--priority N] [--project P] [--tags T]` | Create a new task |
| `aops done <id>` | Mark a task as done |
| `aops update <id> [--status S] [--priority N] [--project P] [--assignee A] [--tags T]` | Update task frontmatter fields |
| `aops deps <id> [--tree]` | Show dependency tree for a task |

### Knowledge Graph

| Command | Description |
|---------|-------------|
| `aops context <id> [--hops N]` | Full knowledge neighbourhood: metadata, backlinks by type, nearby nodes within N hops. Supports flexible ID resolution (by ID, filename, or title). |
| `aops trace <from> <to> [-n max_paths]` | Find shortest paths between two nodes in the graph |
| `aops orphans` | List disconnected nodes with no incoming or outgoing edges |
| `aops metrics [id]` | Network centrality metrics (PageRank, betweenness, degree). Omit ID for top-20 summary. |
| `aops graph [--format json\|graphml\|dot\|mcp-index\|all] [--output path]` | Export the full knowledge graph |

## MCP Tools

The `pkb` server exposes 18 tools over MCP (stdio transport).

### Search Tools

| Tool | Description |
|------|-------------|
| `search` | Hybrid semantic + graph-proximity search. Params: `query` (required), `limit` (default 10), `boost_id` (optional), `project` |
| `get_document` | Read full contents of a file. Params: `path` (string) |
| `list_documents` | Browse/filter documents. Params: `tag`, `type`, `status`, `project` (all optional) |
| `reindex` | Force a full re-scan of the PKB directory and rebuild the knowledge graph |

### Task Tools

| Tool | Description |
|------|-------------|
| `task_search` | Semantic search filtered to tasks/projects/goals. Returns graph context. Params: `query`, `limit` |
| `list_tasks` | List tasks with filtering. Use `status="ready"` for actionable tasks with weight column, `status="blocked"` for tasks with blocker details. Params: `project`, `status`, `priority`, `assignee`, `limit` |
| `get_task` | Retrieve task by ID. Returns frontmatter, body, path, and relationship context (depends_on, blocks, children, parent with titles/statuses, downstream_weight, stakeholder_exposure). Params: `id` |
| `create_task` | Create a new task markdown file. Params: `title` (required), `id`, `parent`, `priority`, `project`, `tags`, `depends_on`, `assignee`, `complexity`, `body` |
| `update_task` | Update frontmatter fields on an existing task. Params: `path` or `id`, `updates` (object) |
| `complete_task` | Mark a task as done. Params: `id` |
| `get_network_metrics` | Centrality metrics: degree, betweenness, PageRank, downstream weight. Params: `id` |

### Document CRUD Tools

| Tool | Description |
|------|-------------|
| `create` | Create a new document with enforced frontmatter. Subdirectory routing by type. Params: `title`, `type` (required), plus optional fields |
| `create_memory` | Create a new memory/note. Stored in memories/ directory. Params: `title` (required) |
| `append` | Append timestamped content to an existing document. Params: `id`, `content` (required), `section` (optional) |
| `delete` | Delete a document by ID. Removes from disk and vector store. Params: `id` |

### Knowledge Graph Tools

| Tool | Description |
|------|-------------|
| `pkb_context` | Full knowledge neighbourhood: metadata, relationships, backlinks grouped by source type, nearby nodes within N hops. Supports flexible ID resolution. Params: `id` (required), `hops` (default 2) |
| `pkb_trace` | Find shortest paths between two nodes. Params: `from`, `to` (required), `max_paths` (default 3) |
| `pkb_orphans` | Find disconnected nodes with zero edges. Params: `limit` (default 20) |

## MCP Client Configuration

### Claude Code

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "pkb": {
      "command": "/path/to/pkb",
      "args": []
    }
  }
}
```

### Gemini CLI

Add to your extension or `settings.json`:

```json
{
  "mcpServers": {
    "pkb": {
      "command": "/path/to/pkb",
      "args": []
    }
  }
}
```

## Architecture

```text
MCP Client ◄──stdio──► pkb
                          │
                    ┌─────┴──────┐
                    │ MCP Server │  (rmcp 0.1, ServerHandler trait)
                    └─────┬──────┘
                     ┌────┼────┐
              ┌──────┴┐ ┌─┴──┐ ┌┴──────────┐
              │Vector │ │Graph│ │ Document  │
              │Store  │ │Store│ │ CRUD      │
              └───┬───┘ └─┬──┘ └───────────┘
                  │       │
            ┌─────┴──────┐│
            │  Embedder  ││  (MiniLM-L6-v2 via ONNX Runtime)
            └────────────┘│
                          │
                    ┌─────┴──────┐
                    │ PKB Files  │  (markdown + YAML frontmatter)
                    └────────────┘
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ACA_DATA` | `~/brain` | PKB root directory |
| `RUST_LOG` | `info` | Log level filter |
| `AOPS_OFFLINE` | `false` | Disable model/runtime auto-download |
| `AOPS_MODEL_PATH` | (auto) | Override model directory path |
| `ORT_DYLIB_PATH` | (auto) | Override ONNX Runtime library path |

## Requirements

- Rust >= 1.88

## Acknowledgments

The SIMD-optimized vector distance functions in `src/distance.rs` are adapted from
[shodh-memory](https://github.com/varun29ankuS/shodh-memory) by Varun Ankus,
originally licensed under Apache-2.0. The overall embedding and vector search
architecture also drew inspiration from shodh-memory's design.

## License

Copyright (C) 2025 Nicolas Suzor

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

See [LICENSE](LICENSE) for the full text.
