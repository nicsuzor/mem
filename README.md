# mem

A fast, local semantic search engine and knowledge graph for your personal knowledge base. Works with plain markdown files, exposes an [MCP](https://modelcontextprotocol.io/) server for AI assistants, and includes a CLI for direct access.

**What it does:** Point it at a directory of markdown files and it builds a searchable vector index + knowledge graph from YAML frontmatter links. AI assistants (Claude, Gemini, etc.) can then search, create, and manage documents through MCP tools. You can also use the CLI directly.

## Features

- **Semantic search** — BGE-M3 embeddings (1024-dim) via ONNX Runtime, with hybrid graph-proximity boosting
- **Knowledge graph** — Seven edge types extracted from frontmatter (`parent`, `depends_on`, `soft_depends_on`, `supersedes`, `contributes_to`) and body (`link` from wikilinks), plus auto-discovered `similar_to` edges from semantic similarity. PageRank, betweenness centrality, and path tracing
- **Task management** — Create, prioritize, and track tasks with dependency graphs; `ready` and `blocked` filters use graph analysis
- **Memory system** — Store and retrieve observations, notes, and insights with semantic search
- **MCP server** — 40 tools for AI assistants over stdio transport
- **CLI** — Full-featured command-line interface for search, tasks, memory, and graph operations
- **Telemetry** — Built-in usage tracking for MCP tools (call counts, response sizes, latency)
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

| Category | Types | Role |
|----------|-------|------|
| **Actionable** | `goal`, `project`, `subproject`, `epic`, `task`, `action`, `bug`, `feature`, `milestone`, `learn` | Executed; appear in ready/blocked queues |
| **Obligation** | `target`, `prototype` | Declared deadline-bound obligations or class templates; not executed but propagate urgency to contributing tasks (see Focus Scoring) |
| **Reference** | `note`, `knowledge`, `memory`, `contact` | Knowledge content; searchable but excluded from task workflows |

`target` represents a one-shot terminal obligation (a deadline you must not miss). `prototype` is a class template for recurring obligations (e.g. peer review load) whose instances inherit `severity`, `goal_type`, and edge defaults at creation.

### Priority levels

| Level | Label | Use |
|-------|-------|-----|
| `0` | P0 — Critical | Drop everything; this is what you're doing now |
| `1` | P1 — High | Active commitment; this week |
| `2` | P2 — Standard | Default; ordinary work |
| `3` | P3 — Low | Background; pick up when capacity exists |
| `4` | P4 — Backlog | May never happen; keep visible |

Priority propagates upward via `effective_priority`: a P3 task blocking a P0 inherits P0 weighting in scoring even though its own field stays P3. See Focus Scoring for how priority composes with severity and urgency.

### Edge types

The knowledge graph has seven edge types. Some are derived from frontmatter, others are computed automatically.

| Edge type | Source | Affects ready/blocked? | Affects importance propagation? | Notes |
|-----------|--------|-------------------------|----------------------------------|-------|
| `parent` | `parent:` frontmatter or `children:` list | ✅ (via unfinished children) | ✅ | Hierarchy |
| `depends_on` | `depends_on:` list | ✅ blocks task | ✅ | Hard dependency |
| `soft_depends_on` | `soft_depends_on:` list | ❌ | ✅ | Informational ordering |
| `link` | `[[wikilinks]]` and markdown links in body | ❌ | ❌ | Cross-references; counted as backlinks |
| `supersedes` | `supersedes:` frontmatter | ❌ | ❌ | This node replaces the target |
| `contributes_to` | `contributes_to:` list with verbal weights | ❌ | ✅ | Strategic priority (Birnbaum importance with Renooij-Witteman terms) |
| `similar_to` | Computed from BGE-M3 embeddings (cosine ≥ 0.85) | ❌ | ❌ | Auto-discovered semantic similarity; appears in `pkb_context` and `pkb_trace` |

`similar_to` edges are materialised when the graph is built with the vector store available (e.g. via the MCP server). They participate in pathfinding (`pkb_trace`) and context display (`pkb_context`) but are deliberately excluded from blocking analysis and ready/blocked classification — semantic similarity is informational, not causal.

## Focus Scoring

Tasks are ranked by one composite integer, **`focus_score`** — the sum of priority, severity, deadline pressure, age, structural blast radius, stakeholder waiting time, and urgency (target propagation). Sort by it; ignore the components unless you're debugging a ranking.

For deadline-bound obligations that aren't tasks themselves (ARC submissions, contract signings, anything you must not fail), declare a **target node** and link contributing tasks to it:

```yaml
# The obligation
type: target
severity: 3                      # see severity ladder below
goal_type: committed             # committed | aspirational | learning
due: 2026-05-07
consequence: "Late review damages standing with the panel."

# A task contributing to it
contributes_to:
  - to: <target-id>
    weight: Certain              # see weight scale below
    why: "contractual obligation as assigned assessor"
```

`mem` propagates `severity × edge_weight × deadline-slack` back from each target to its contributors, writing `node.urgency` and folding it into `focus_score`. A P2 task blocking a SEV3-committed deadline rises automatically as the deadline approaches — no priority bumping.

### Severity ladder

| Level | Label | Example |
|-------|-------|---------|
| 0 | Negligible | Minor annoyance; no consequence beyond self |
| 1 | Low | Small reputational or time cost |
| 2 | Moderate | Meaningful commitment; recoverable if missed |
| 3 | High | Serious consequence; hard to recover |
| **4** | **Terminal** | **Job loss, bankruptcy, severe health, legal** |

SEV0–3 are compensatory (standard scalar math). **SEV4 + `goal_type: committed` is lexicographic** — it gets a 10 000× multiplier so any SEV4-adjacent task outranks any non-SEV4 task regardless of priority, deadline, or anything else. Use sparingly; the cognitive speedbump of writing `consequence:` prose is part of the design.

### `goal_type`

| Value | Effect |
|-------|--------|
| `committed` | Receives the lexicographic override at SEV4. Standard contractual / non-negotiable obligations. |
| `aspirational` | Linear propagation only. `consequence:` is reused as opportunity-cost prose. Prevents moonshots from hijacking the queue. |
| `learning` | Linear propagation only. Marks targets where the value is the attempt, not the outcome. |

### Weight scale (Renooij-Witteman)

`contributes_to.weight` accepts only verbal terms — raw decimals are rejected at parse time. Weights mean **Birnbaum importance** (probability that missing this task guarantees target failure), not "percent contribution":

| Term | Anchor | Reading |
|------|--------|---------|
| Certain | 1.00 | Single point of failure — miss this and the target fails |
| Probable | 0.85 | Strong contributor |
| Expected | 0.75 | Likely needed |
| Fifty-Fifty | 0.50 | Redundancy exists |
| Uncertain | 0.25 | Possibly needed |
| Improbable | 0.15 | Marginal |
| Impossible | 0.00 | No contribution |

Non-linearity defeats the spacing and centring biases that corrupt linear scales.

### `focus_score` components

| Term | Range | Trigger |
|------|-------|---------|
| `priority_base` | 0 / 5 000 / 10 000 | P0 = 10 000, P1 = 5 000, P2+ = 0 |
| `severity_bonus` | 0 – 100 000 | SEV0–4 on the task itself; SEV4 lexicographic |
| `deadline_score` | 0 – 12 000 | Overdue / tight / near-tight; ×1.5 if `consequence` set |
| `age_staleness_bonus` | 0 – 200 | P2+ only; min(days_since_created, 200) |
| `downstream_weight × 10` | 0 – ∞ | Structural blast radius from `parent` / `depends_on` BFS |
| `stakeholder_waiting_bonus` | 0 / 2 000 – 8 000 | When `stakeholder` set; +200/day |
| `urgency_term` | 0 – 10 000+ | `round(node.urgency)` — target propagation |

The formula lives in `compute_urgency` and `compute_focus_scores` in `src/graph_store.rs`. Prototype nodes (for recurring obligations like peer review) and the deferred calibration ritual extend the model — see the source for current behaviour.

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
| `aops stats [--sort count\|bytes\|latency\|errors]` | Show MCP tool usage telemetry |

## MCP Tools

The `pkb` binary exposes 39 tools over MCP stdio transport. Any MCP-compatible client can use them.
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
| **System** | `get_stats` |

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
# Tunable subset; other edge types (supersedes, contributes_to, similar_to)
# render with default weights. See "Edge types" above for full list.
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
