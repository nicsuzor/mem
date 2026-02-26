# mem

A fast, local semantic search engine and knowledge graph for your personal knowledge base. Works with plain markdown files, exposes an [MCP](https://modelcontextprotocol.io/) server for AI assistants, and includes a CLI for direct access.

**What it does:** Point it at a directory of markdown files and it builds a searchable vector index + knowledge graph from YAML frontmatter links. AI assistants (Claude, Gemini, etc.) can then search, create, and manage documents through MCP tools. You can also use the CLI directly.

## Features

- **Semantic search** — BGE-M3 embeddings (1024-dim) via ONNX Runtime, with hybrid graph-proximity boosting
- **Knowledge graph** — Automatic relationship extraction from YAML frontmatter (`depends_on`, `parent`, `tags`, etc.), with PageRank, betweenness centrality, and path tracing
- **Task management** — Create, prioritize, and track tasks with dependency graphs; `ready` and `blocked` filters use graph analysis
- **Memory system** — Store and retrieve observations, notes, and insights with semantic search
- **MCP server** — 18 tools for AI assistants over stdio transport
- **CLI** — Full-featured command-line interface for search, tasks, memory, and graph operations
- **Fast** — Lazy ONNX session pooling, SIMD-accelerated vector ops, parallel batch embedding
- **Local** — Everything runs on your machine. No cloud services, no API keys, no data leaves your disk
- **Auto-setup** — Model files and ONNX Runtime are downloaded automatically on first run

## Install

### Pre-built binaries (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/nicsuzor/mem/main/install.sh | sh
```

Supports Linux x86_64 and macOS Apple Silicon. Installs `pkb` (MCP server) and `aops` (CLI) to `/usr/local/bin`.

### From source

```bash
cargo install --git https://github.com/nicsuzor/mem.git
```

Requires Rust >= 1.88.

### cargo-binstall

```bash
cargo binstall mem
```

## Quick Start

### 1. Set your PKB directory

```bash
export ACA_DATA=~/brain  # or wherever your markdown files live
```

### 2. Index your files

```bash
aops reindex
```

### 3. Search

```bash
aops search "how does authentication work"
```

### 4. Connect to an AI assistant

Add to your MCP client config (e.g. Claude Code `.mcp.json`):

```json
{
  "mcpServers": {
    "pkb": {
      "command": "pkb",
      "args": []
    }
  }
}
```

## Document Format

mem works with plain markdown files that have YAML frontmatter:

```markdown
---
id: my-task-123
title: Implement user auth
type: task
status: active
priority: 2
tags: [backend, security]
depends_on: [design-doc-456]
parent: project-789
---

The actual content of the document goes here.
Any markdown is fine.
```

All frontmatter fields are optional. Files without frontmatter are indexed by filename and content.

## CLI Commands

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
| `aops new <title> [--parent ID] [--priority N] [--project P] [--tags T] [--depends-on ID]` | Create a new task |
| `aops done <id>` | Mark a task as done |
| `aops update <id> [--status S] [--priority N] [--project P] [--tags T]` | Update task fields |
| `aops deps <id> [--tree]` | Show dependency tree |
| `aops blocks <id> [--tree]` | Show what completing a task would unblock |

### Memory

| Command | Description |
|---------|-------------|
| `aops recall <query> [-n limit]` | Semantic search over memories and notes |
| `aops memories [--tag T]` | List memory-type documents |
| `aops tags [tag...] [--count] [--type T]` | Tag frequency summary or search by tags |
| `aops forget <id>` | Delete a memory document |

### Knowledge Graph

| Command | Description |
|---------|-------------|
| `aops context <id> [--hops N]` | Neighbourhood: metadata, backlinks, nearby nodes |
| `aops trace <from> <to> [-n max_paths]` | Shortest paths between two nodes |
| `aops orphans` | Disconnected nodes with no edges |
| `aops metrics [id]` | PageRank, betweenness, degree centrality |
| `aops graph [--format json\|graphml\|dot] [--output path]` | Export the knowledge graph |

## MCP Tools

The `pkb` binary exposes 18 tools over MCP stdio transport. Any MCP-compatible client can use them.

| Category | Tools |
|----------|-------|
| **Search** | `search`, `get_document`, `list_documents`, `reindex` |
| **Tasks** | `task_search`, `list_tasks`, `get_task`, `create_task`, `update_task`, `complete_task`, `get_network_metrics`, `decompose_task`, `get_dependency_tree`, `get_task_children` |
| **Memory** | `retrieve_memory`, `search_by_tag`, `list_memories`, `delete_memory` |
| **CRUD** | `create`, `create_memory`, `append`, `delete` |
| **Graph** | `pkb_context`, `pkb_trace`, `pkb_orphans` |

## Architecture

```text
MCP Client <--stdio--> pkb (MCP server)
                         |
                   +-----+------+
                   |  Dispatch  |  (18 tools, ServerHandler trait)
                   +-----+------+
                    +----|----+
             +------+ +--+-+ +----------+
             |Vector| |Graph| | Document |
             |Store | |Store| | CRUD     |
             +--+---+ +--+--+ +----------+
                |         |
          +-----+------+  |
          |  Embedder   |  |  BGE-M3 via ONNX Runtime (1024-dim)
          +-------------+  |
                           |
                     +-----+------+
                     | PKB Files  |  markdown + YAML frontmatter
                     +------------+
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ACA_DATA` | `~/brain` | PKB root directory |
| `RUST_LOG` | `info` | Log level filter |
| `AOPS_OFFLINE` | `false` | Disable model/runtime auto-download |
| `AOPS_MODEL_PATH` | (auto) | Override model directory path |
| `ORT_DYLIB_PATH` | (auto) | Override ONNX Runtime library path |

## Acknowledgments

The SIMD-optimized vector distance functions in `src/distance.rs` are adapted from
[shodh-memory](https://github.com/varun29ankuS/shodh-memory) by Varun Ankus,
originally licensed under Apache-2.0. The embedding and vector search architecture
also drew inspiration from shodh-memory's design.

## License

Copyright (C) 2025-2026 Nicolas Suzor

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

See [LICENSE](LICENSE) for the full text.
