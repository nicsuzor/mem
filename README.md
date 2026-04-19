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

### Status values

| Status | Meaning |
|--------|---------|
| `active` | Open, ready to work on (default) |
| `in_progress` | Currently being worked on |
| `blocked` | Waiting on dependencies |
| `review` | In review / awaiting feedback |
| `paused` | Intentionally deferred |
| `someday` | Low priority / maybe later |
| `done` | Completed successfully |
| `cancelled` | Abandoned / no longer relevant |

**Aliases** (automatically normalized): `inbox`, `todo`, `open` → `active`; `in-progress` → `in_progress`; `in_review`, `in-review` → `review`; `complete`, `completed`, `closed`, `archived` → `done`; `dead` → `cancelled`.

### Node types

| Category | Types |
|----------|-------|
| **Actionable** | `goal`, `project`, `subproject`, `epic`, `task`, `action`, `bug`, `feature`, `milestone`, `learn` |
| **Reference** | `note`, `knowledge`, `memory`, `contact` |

Actionable types are used for task management (ready/blocked analysis, dependency graphs, focus picks). Reference types appear in search and the knowledge graph but are excluded from task workflows.

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
| `aops graph --layout-config path/to/layout.toml` | Use custom layout parameters |

## MCP Tools

The `pkb` binary exposes 38 tools over MCP stdio transport. Any MCP-compatible client can use them.
It also provides **MCP prompts** to guide AI assistants through common search and navigation patterns.

### Prompts

| Prompt | Description | Guidance |
|--------|-------------|----------|
| `find-task` | "How do I find a task about X?" | Demonstrates `task_search` then `get_task` |
| `explore-topic` | "What do we know about X?" | Demonstrates `search` then `get_document` |
| `navigate-graph` | "What's connected to X?" | Demonstrates `pkb_context` for relationships |
| `find-by-tag` | "Show me everything tagged X" | Demonstrates `search_by_tag` usage |

### Tools

| Category | Tools |
|----------|-------|
| **Search** | `search`, `get_document`, `list_documents`, `find_duplicates` |
| **Tasks** | `task_search`, `list_tasks`, `get_task`, `create_task`, `create_subtask`, `update_task`, `complete_task`, `release_task`, `decompose_task`, `get_dependency_tree`, `get_task_children`, `task_summary`, `get_network_metrics` |
| **Memory** | `retrieve_memory`, `search_by_tag`, `list_memories`, `delete_memory` |
| **CRUD** | `create`, `create_memory`, `append`, `delete` |
| **Graph** | `pkb_context`, `pkb_trace`, `pkb_orphans`, `graph_stats`, `graph_json` |
| **Batch** | `batch_update`, `batch_reparent`, `batch_archive`, `batch_merge`, `batch_create_epics`, `batch_reclassify`, `bulk_reparent`, `merge_node` |

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

## Graph Layout Configuration

The knowledge graph uses a [ForceAtlas2](https://journals.plos.org/plosone/article?id=10.1371/journal.pone.0098679) force-directed layout algorithm. All parameters can be tuned at runtime via a `layout.toml` file — no recompilation needed.

**Search order:** `--layout-config` CLI flag > `./layout.toml` in cwd > `layout.toml` next to the executable.

If no file is found, built-in defaults are used.

```toml
# layout.toml — edit and re-run `aops graph` to see changes

[force]
k_repulsion = 100.0       # Repulsion coefficient (higher = nodes push apart more)
k_gravity = 1.0           # Gravity toward center (higher = tighter cluster)
iterations = 200          # Number of simulation steps
tolerance = 1.0           # Adaptive speed tolerance (higher = faster but less stable)
viewport = 1000.0         # Output coordinate range
project_clustering = 0.5  # Strength of same-project attraction (0 = off)
max_displacement = 10.0   # Per-node per-iteration movement cap

# Edge attraction by type: [strength, ideal_distance]
[edges]
parent = [1.0, 40.0]
depends_on = [0.15, 200.0]
soft_depends_on = [0.08, 250.0]
link = [0.02, 300.0]

# Node repulsion charge multiplier by type
[charges]
goal = 3.0
project = 2.5
epic = 2.0
subproject = 1.8
learn = 1.2
default = 1.0
```

### Development workflow

For fast iteration on layout parameters, use debug builds:

```bash
# Edit layout.toml, then:
cargo run -- graph -f json -o graph.json    # ~4s incremental rebuild

# Or skip recompilation entirely with a pre-built binary:
aops graph -f json -o graph.json            # instant, reads layout.toml

# Use a custom config path:
aops --layout-config ~/experiments/tight.toml graph -f json
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
