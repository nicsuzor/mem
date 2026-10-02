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

Supports Linux x86_64 and macOS Apple Silicon. Installs the `pkb` binary (MCP server + CLI in one) to `/usr/local/bin`.

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
pkb reindex
```

### 3. Search

```bash
pkb search "how does authentication work"
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
| `contributes_to` | `contributes_to:` list with verbal weights | ❌ | ✅ | Strategic priority (verbal contribution weights with Renooij-Witteman terms) |
| `similar_to` | Computed from BGE-M3 embeddings (cosine ≥ 0.85) | ❌ | ❌ | Auto-discovered semantic similarity; appears in `pkb_trace` |

`similar_to` edges are materialised when the graph is built with the vector store available (e.g. via the MCP server). They participate in pathfinding (`pkb_trace`) but are deliberately excluded from blocking analysis and ready/blocked classification — semantic similarity is informational, not causal.

## Focus Scoring

Tasks are ranked by one composite integer, **`focus_score`** — the sum of priority, severity, deadline pressure, age, structural blast radius, stakeholder waiting time, urgency (target propagation), and a value-of-information premium. Sort by it; ignore the components unless you're debugging a ranking.

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

`contributes_to.weight` accepts only verbal terms — raw decimals are rejected at parse time. Weights represent a **verbal contribution-weight scale** (Renooij-Witteman elicitation anchors), not "percent contribution":

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
| `deadline_score` | 0 – 12 000 | Overdue / tight / near-tight. `consequence` applies no multiplier — stakes reach a task via target `severity` |
| `age_staleness_bonus` | 0 – 200 | P2+ only; min(days_since_created, 200) |
| `downstream_weight × 10` | 0 – ∞ | Structural blast radius: depth-decayed, edge-weighted sum of base weights over the **distinct** nodes reachable via `blocks` / `soft_blocks` / children / reverse `contributes_to` — not a count |
| `stakeholder_waiting_bonus` | 0 / 2 000 – 8 000 | When `stakeholder` set; +200/day |
| `urgency_term` | 0 – 10 000+ | `round(node.urgency)` — target propagation |
| `voi_term` | 0 – 5 000 | `round(node.voi_value)` — value-of-information premium for a leaf task that unblocks an uncertain cone; 0 for non-leaf nodes |

The formula lives in `compute_urgency`, `compute_voi_term` and `compute_focus_scores` in `src/graph_store.rs`; `downstream_weight` and the VoI sum are both accumulated by the shared `walk_cone` traversal. Prototype nodes (for recurring obligations like peer review) and the deferred calibration ritual extend the model — see the source for current behaviour.

## CLI Commands

### Search & Index

| Command | Description |
|---------|-------------|
| `pkb search <query> [-n limit] [--full]` | Semantic search across the knowledge base |
| `pkb add <files...>` | Add markdown files to the index |
| `pkb reindex [--force]` | Re-scan and re-index all PKB files |
| `pkb status` | Show index statistics (document count, DB size) |

### Task Management

| Command | Description |
|---------|-------------|
| `pkb tasks [ready\|blocked\|all] [--project P] [--sort S]` | List tasks sorted by priority + downstream weight |
| `pkb task <id>` | Show task details and relationships |
| `pkb new <title> [--parent ID] [--priority N] [--project P] [--tags T] [--depends-on ID]` | Create a new task |
| `pkb done <id>` | Mark a task as done |
| `pkb update <id> [--status S] [--priority N] [--project P] [--tags T]` | Update task fields |
| `pkb deps <id> [--tree]` | Show dependency tree |
| `pkb blocks <id> [--tree]` | Show what completing a task would unblock |

### Memory

| Command | Description |
|---------|-------------|
| `pkb recall <query> [-n limit]` | Semantic search over memories and notes |
| `pkb memories [--tag T]` | List memory-type documents |
| `pkb tags [tag...] [--count] [--type T]` | Tag frequency summary or search by tags |
| `pkb forget <id>` | Delete a memory document |

### Knowledge Graph

| Command | Description |
|---------|-------------|
| `pkb context <id> [--hops N]` | Neighbourhood: metadata, backlinks, nearby nodes |
| `pkb trace <from> <to> [-n max_paths]` | Shortest paths between two nodes |
| `pkb orphans` | Disconnected nodes with no edges |
| `pkb metrics [id]` | PageRank, betweenness, degree centrality |
| `pkb graph [--format json\|graphml\|mcp-index\|all] [--output path]` | Export the knowledge graph |
| `pkb stats [--sort count\|bytes\|latency\|errors]` | Show MCP tool usage telemetry |

### Excalidraw (`pkb excalidraw`, `pkb-excalidraw`)

Two surfaces, documented in full in [`src/excalidraw/README.md`](src/excalidraw/README.md):

| Command | Description |
|---------|-------------|
| `pkb excalidraw export <out> [--focus <id>] [--hops N]` | Export the graph (or an ego-network) as an Excalidraw canvas; merges into an existing canvas instead of overwriting |
| `pkb excalidraw diff <canvas> [--base <snapshot>] [--json]` | 3-way diff of an edited canvas against the live PKB |
| `pkb excalidraw sync <canvas> [--base <snapshot>] [--dry-run] [--sync-edge-removals]` | Write card additions, frontmatter updates and new `depends_on`/`soft_depends_on`/`parent` edges back to markdown |

These need `ACA_DATA` set (`--pkb-root`/`--db-path` override its defaults, but the variable itself is still required for `pkb` to start).

`pkb-excalidraw` is the companion binary for inspecting, diffing, mutating, and validating any Excalidraw file (`.excalidraw` and `.excalidrawlib`) without touching the PKB and without `ACA_DATA`. It includes full built-in manual and subcommand help (`pkb-excalidraw --help`, `pkb-excalidraw help <subcommand>`) formatted like a Unix man page, rendered with ANSI colour on a TTY and plain text when piped or when `NO_COLOR` is set.

| Category | Commands & Synopsis | Description |
|----------|---------------------|-------------|
| **Inspection & Projections** | `summary`, `map`, `nodes`, `edges`, `inspect <id>`, `get <id> <field>`, `describe`, `style`, `query [--type T] [--filter K=V]` | Token-cheap structural projections, spatial scene summaries, and element queries |
| **Validation & Integrity** | `check`, `overlap`, `arrows-check` | Verify structural invariants, index ordering, 2D AABB box collisions, and arrow polyline intersections |
| **Comparison & Diffs** | `diff <file2>`, `struct-diff <file2>` | Comprehensive 2-way comparison and semantic structural diffs without coordinate jitter |
| **Mutation & CRUD** | `add-node --text "T" [OPTIONS]`, `update-node --id <id> [OPTIONS]`, `add-text --text "T"`, `connect --from A --to B [--label L] [--curved]`, `set-text <id> "T"` / `fit`, `move-elem <id> [--to\|--by]`, `delete-elem <id> [--cascade-arrows]`, `update <id> --set '<json>'`, `apply <patch.json>`, `batch <changes.json>`, `clear [--yes]` | Atomic element creation, styling, symmetrical text fitting, connected arrow routing, translation, and transactional batch patching |
| **Arrangement & Grouping** | `arrange align --ids ... --to <dir>`, `distribute`, `group`, `ungroup`, `lock`, `unlock`, `duplicate --ids ... [--offset DX,DY]` | Bounding-box alignment, equal distribution, group management, canvas locking, and element cloning |
| **Component Libraries** | `lib`, `item <selector> --after <index> [--at X,Y]` | Inspect `.excalidrawlib` components and extract items with re-keyed monotonic indices |
| **Lifecycle & Export** | `snapshot <save\|list\|restore>`, `export [--format json\|obsidian]`, `import <src> [--replace]`, `screenshot [--format svg\|png]` | Snapshot checkpoints in `.pkb_snapshots/`, Obsidian markdown conversion, and zero-dependency SVG rendering |
| **Themes & Styling** | `theme export [out.json]`, `theme apply <theme> [--all] [--id <id>]` | Export or apply semantic color roles and styling presets (`default`, `retro-terminal`, `aops-default`) |

For the node model, diff/sync semantics and known limitations, see [`src/excalidraw/README.md`](src/excalidraw/README.md). For the companion binary's invariants, see the [Excalidraw Tooling Specification](specs/excalidraw-tooling.md); for agent patterns and copy-paste templates, the [Excalidraw Agent Guide](references/EXCALIDRAW_AGENT_GUIDE.md).

#### Excalidraw tool comparison

Create / Read / Update / Delete / List mark whether the tool adds, reports on, modifies, removes, or enumerates (one entry per item) the items it acts on: PKB notes and their frontmatter for `pkb` and the MCP tools; canvas elements for `pkb-excalidraw` unless the Notes say otherwise. Writing an export or report file is not Create.

| Tool | Surface | What it does | Input | Output | Create | Read | Update | Delete | List | Notes |
|---|---|---|---|---|:-:|:-:|:-:|:-:|:-:|---|
| `pkb graph --format excalidraw` | `pkb` CLI | Renders the graph as an Excalidraw canvas: the ego-network of `--focus`, or the merged ego-networks of the top 10 focus picks when `--focus` is omitted | `--focus <id>`, `--hops <n>` (default 2), `-o <path>` | Canvas JSON to `<path>` or stdout | N | Y | N | N | N | [`src/cli.rs:2549`](src/cli.rs#L2549); no-focus behaviour [`src/graph_store.rs:2243`](src/graph_store.rs#L2243) |
| `pkb excalidraw export` | `pkb` CLI | Renders the graph (or the ego-network of `--focus`) as a canvas; if the output file already holds a canvas, merges live PKB state into it and keeps hand-placed positions | `<output_path>`, `--focus <id>`, `--hops <n>` (default 2) | Canvas JSON written to `<output_path>` | N | Y | N | N | N | [`src/cli.rs:3843`](src/cli.rs#L3843); merge [`src/excalidraw/merge.rs:164`](src/excalidraw/merge.rs#L164) |
| `pkb excalidraw diff` | `pkb` CLI | 3-way diff of an edited canvas against the live PKB, optionally against a base snapshot | `<canvas_path>`, `--base <path>`, `--json` | Diff summary, or `GraphDiff` JSON with `--json` | N | Y | N | N | N | [`src/cli.rs:3882`](src/cli.rs#L3882) |
| `pkb excalidraw sync` | `pkb` CLI | Writes canvas changes back to PKB markdown: new cards become notes; title, status, intent and parent edits and new `depends_on`/`soft_depends_on`/`parent` edges go to frontmatter | `<canvas_path>`, `--base <path>`, `--dry-run`, `--sync-edge-removals` | Sync report; markdown files modified unless `--dry-run` | Y | Y | Y | Y | N | Delete removes `depends_on` entries only, and only with `--sync-edge-removals`; never deletes notes. [`src/cli.rs:3910`](src/cli.rs#L3910); engine [`src/excalidraw/merge.rs:515`](src/excalidraw/merge.rs#L515) |
| `graph_excalidraw` | MCP | Same rendering as `pkb graph --format excalidraw`, returned inline | `node_id` (optional), `hops` (optional, default 2) | Canvas JSON text | N | Y | N | N | N | [`src/mcp_server/handlers_batch.rs:401`](src/mcp_server/handlers_batch.rs#L401); schema [`src/mcp_server/schemas.rs:588`](src/mcp_server/schemas.rs#L588) |
| `diff_excalidraw` | MCP | Same diff as `pkb excalidraw diff`, taking the canvas as a string | `canvas` (required), `base` (optional) | `GraphDiff` JSON text | N | Y | N | N | N | [`src/mcp_server/handlers_batch.rs:459`](src/mcp_server/handlers_batch.rs#L459); schema [`src/mcp_server/schemas.rs:623`](src/mcp_server/schemas.rs#L623) |
| `sync_excalidraw` | MCP | Same write-back as `pkb excalidraw sync`, taking the canvas as a string | `canvas` (required), `base`, `dry_run`, `sync_edge_removals` | Sync report JSON, or the diff when `dry_run` | Y | Y | Y | Y | N | Same Delete limits as `pkb excalidraw sync`. [`src/mcp_server/handlers_batch.rs:507`](src/mcp_server/handlers_batch.rs#L507); schema [`src/mcp_server/schemas.rs:638`](src/mcp_server/schemas.rs#L638) |
| `pkb-excalidraw summary` | `pkb-excalidraw` CLI | Element counts by type, scene extents, index-ordering sanity | `FILE` | Text | N | Y | N | N | N | Default when no command is given. [`src/bin/pkb_excalidraw.rs:4955`](src/bin/pkb_excalidraw.rs#L4955) |
| `pkb-excalidraw map` | `pkb-excalidraw` CLI | One line per live element: shapes with geometry, colour and label; arrows with endpoints | `FILE` | TSV | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:4956`](src/bin/pkb_excalidraw.rs#L4956) |
| `pkb-excalidraw nodes` | `pkb-excalidraw` CLI | One line per node shape, bound text folded into its label | `FILE` | TSV | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:4957`](src/bin/pkb_excalidraw.rs#L4957) |
| `pkb-excalidraw edges` | `pkb-excalidraw` CLI | One line per arrow: source, target, label | `FILE` | TSV | N | Y | N | N | Y | Alias `arrows`. [`src/bin/pkb_excalidraw.rs:4958`](src/bin/pkb_excalidraw.rs#L4958) |
| `pkb-excalidraw inspect` | `pkb-excalidraw` CLI | Geometry, label, role, bindings and inbound/outbound arrows of one element | `FILE`, `<id>` | Text | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:4959`](src/bin/pkb_excalidraw.rs#L4959) |
| `pkb-excalidraw get` | `pkb-excalidraw` CLI | Raw JSON of one element | `FILE`, `<id>` | JSON | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:4966`](src/bin/pkb_excalidraw.rs#L4966) |
| `pkb-excalidraw describe` | `pkb-excalidraw` CLI | Scene overview: extents, counts, elements bucketed into rows, labelled arrow flows | `FILE` | Markdown | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:5782`](src/bin/pkb_excalidraw.rs#L5782) |
| `pkb-excalidraw style` | `pkb-excalidraw` CLI | Modal values and frequency counts of stroke, fill, roughness, opacity and font properties | `FILE` | Text | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:4973`](src/bin/pkb_excalidraw.rs#L4973) |
| `pkb-excalidraw query` | `pkb-excalidraw` CLI | Live elements matching type, bounding box, `KEY=VAL` or JSON-pattern filters | `FILE`, `--type <type>`, `--bbox X1,Y1,X2,Y2`, `--filter KEY=VAL` (repeatable), `--filter-json '<json>'` | JSON array | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:6017`](src/bin/pkb_excalidraw.rs#L6017) |
| `pkb-excalidraw check` | `pkb-excalidraw` CLI | Invariant audit: duplicate IDs, array vs fractional-index order, container/text and arrow bindings | `FILE` | `OK` (exit 0) or failures (exit 1) | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:4974`](src/bin/pkb_excalidraw.rs#L4974) |
| `pkb-excalidraw overlap` | `pkb-excalidraw` CLI | Bounding-box collisions between sibling elements | `FILE` | `OK` (exit 0) or collisions (exit 1) | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:4987`](src/bin/pkb_excalidraw.rs#L4987) |
| `pkb-excalidraw arrows-check` | `pkb-excalidraw` CLI | Arrow segments that cut through boxes they are not bound to | `FILE` | `OK` (exit 0) or collisions (exit 1) | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:5000`](src/bin/pkb_excalidraw.rs#L5000) |
| `pkb-excalidraw diff` | `pkb-excalidraw` CLI | Element-level comparison of two canvases: count deltas, added, deleted and modified elements, connection changes | `FILE1`, `FILE2` | Text | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:5013`](src/bin/pkb_excalidraw.rs#L5013) |
| `pkb-excalidraw struct-diff` | `pkb-excalidraw` CLI | Node/edge-level comparison of two canvases, ignoring coordinates | `FILE1`, `FILE2` | Diff lines (`+ node`, `- edge`, ...) | N | Y | N | N | N | [`src/bin/pkb_excalidraw.rs:5035`](src/bin/pkb_excalidraw.rs#L5035) |
| `pkb-excalidraw lib` | `pkb-excalidraw` CLI | One line per item in a component library | `FILE.excalidrawlib` | Text | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:5060`](src/bin/pkb_excalidraw.rs#L5060) |
| `pkb-excalidraw item` | `pkb-excalidraw` CLI | Extracts one library item with fresh IDs and indices ordered after `--after` | `FILE.excalidrawlib`, `<selector>`, `--after <index>`, `--at X,Y` | Element JSON array on stdout | N | Y | N | N | N | Does not write any canvas. [`src/bin/pkb_excalidraw.rs:5063`](src/bin/pkb_excalidraw.rs#L5063) |
| `pkb-excalidraw add-node` | `pkb-excalidraw` CLI | Adds a rectangle, diamond or ellipse with bound, centred text | `FILE`, `--text "<text>"`, `--type`, `--at X,Y`, `--size W,H`, `--role`, `--color`, `--id`, `--angle`, `--roughness`, `--fill-style`, `--preset` | Writes `FILE`; prints new IDs | Y | N | N | N | N | [`src/bin/pkb_excalidraw.rs:5162`](src/bin/pkb_excalidraw.rs#L5162) |
| `pkb-excalidraw add-text` | `pkb-excalidraw` CLI | Adds a standalone text element | `FILE`, `--text "<text>"`, `--at X,Y`, `--font-size`, `--color` | Writes `FILE`; prints new ID | Y | N | N | N | N | [`src/bin/pkb_excalidraw.rs:5253`](src/bin/pkb_excalidraw.rs#L5253) |
| `pkb-excalidraw connect` | `pkb-excalidraw` CLI | Adds an arrow bound at both ends and registers it on both endpoints | `FILE`, `--from <id>`, `--to <id>`, `--label`, `--color`, `--curved`, `--stroke-style` | Writes `FILE`; prints new ID | Y | N | Y | N | N | Update is the endpoints' `boundElements`. [`src/bin/pkb_excalidraw.rs:5298`](src/bin/pkb_excalidraw.rs#L5298) |
| `pkb-excalidraw update-node` | `pkb-excalidraw` CLI | Changes angle, roughness, fill style or preset of a node, leaving geometry and text alone | `FILE`, `--id <id>`, `--angle`, `--roughness`, `--fill-style`, `--preset` | Writes `FILE` | N | N | Y | N | N | [`src/bin/pkb_excalidraw.rs:5107`](src/bin/pkb_excalidraw.rs#L5107) |
| `pkb-excalidraw set-text` | `pkb-excalidraw` CLI | Replaces an element's text, keeping `text`/`originalText` identical and growing the container from its centre | `FILE`, `<id>`, `"<new_text>"` | Writes `FILE` | N | N | Y | N | N | Alias `fit`. [`src/bin/pkb_excalidraw.rs:5349`](src/bin/pkb_excalidraw.rs#L5349) |
| `pkb-excalidraw move-elem` | `pkb-excalidraw` CLI | Moves an element; bound text and connected arrow endpoints follow | `FILE`, `<id>`, `--to X,Y` or `--by DX,DY` | Writes `FILE` | N | N | Y | N | N | [`src/bin/pkb_excalidraw.rs:5371`](src/bin/pkb_excalidraw.rs#L5371) |
| `pkb-excalidraw delete-elem` | `pkb-excalidraw` CLI | Deletes an element and its bound text; connected arrows are unbound, or deleted with `--cascade-arrows` | `FILE`, `<id>`, `--cascade-arrows` | Writes `FILE` | N | N | Y | Y | N | Update is the unbinding of connected arrows. [`src/bin/pkb_excalidraw.rs:5417`](src/bin/pkb_excalidraw.rs#L5417) |
| `pkb-excalidraw update` | `pkb-excalidraw` CLI | Patches arbitrary element properties from a JSON object; text and position changes cascade | `FILE`, `<id>`, `--set '<json>'` | Writes `FILE` | N | N | Y | N | N | [`src/bin/pkb_excalidraw.rs:5974`](src/bin/pkb_excalidraw.rs#L5974) |
| `pkb-excalidraw apply` | `pkb-excalidraw` CLI | Applies a patch with `create`, `update` and `delete` arrays in one atomic write | `FILE`, `<patch.json>` or `-` | Writes `FILE`; prints counts | Y | N | Y | Y | N | [`src/bin/pkb_excalidraw.rs:6058`](src/bin/pkb_excalidraw.rs#L6058) |
| `pkb-excalidraw batch` | `pkb-excalidraw` CLI | Runs an array of mutation operations in one atomic write | `FILE`, `<changes.json>` or `-` | Writes `FILE` | Y | N | Y | Y | N | Operations: the mutation commands in this table plus `apply-theme` and `clear`. [`src/bin/pkb_excalidraw.rs:5439`](src/bin/pkb_excalidraw.rs#L5439) |
| `pkb-excalidraw clear` | `pkb-excalidraw` CLI | Removes every live element, keeping the file wrapper and metadata | `FILE`, `--yes` | Writes `FILE`; prints count removed | N | N | N | Y | N | [`src/bin/pkb_excalidraw.rs:6239`](src/bin/pkb_excalidraw.rs#L6239) |
| `pkb-excalidraw arrange align` | `pkb-excalidraw` CLI | Aligns elements to a shared edge or centreline | `FILE`, `--ids <id,...>`, `--to left\|center\|right\|top\|middle\|bottom` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw align`. [`src/bin/pkb_excalidraw.rs:5848`](src/bin/pkb_excalidraw.rs#L5848) |
| `pkb-excalidraw arrange distribute` | `pkb-excalidraw` CLI | Spaces three or more elements evenly along an axis | `FILE`, `--ids <id,...>`, `--to horizontal\|vertical` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw distribute`. [`src/bin/pkb_excalidraw.rs:5870`](src/bin/pkb_excalidraw.rs#L5870) |
| `pkb-excalidraw arrange group` | `pkb-excalidraw` CLI | Adds a new group ID to each element | `FILE`, `--ids <id,...>` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw group`. [`src/bin/pkb_excalidraw.rs:5892`](src/bin/pkb_excalidraw.rs#L5892) |
| `pkb-excalidraw arrange ungroup` | `pkb-excalidraw` CLI | Dissolves a group, or removes elements from their groups | `FILE`, `--group <id>` or `--ids <id,...>` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw ungroup`. [`src/bin/pkb_excalidraw.rs:5907`](src/bin/pkb_excalidraw.rs#L5907) |
| `pkb-excalidraw arrange lock` | `pkb-excalidraw` CLI | Sets `locked: true` on elements and their bound text | `FILE`, `--ids <id,...>` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw lock`. [`src/bin/pkb_excalidraw.rs:5923`](src/bin/pkb_excalidraw.rs#L5923) |
| `pkb-excalidraw arrange unlock` | `pkb-excalidraw` CLI | Sets `locked: false` on elements and their bound text | `FILE`, `--ids <id,...>` | Writes `FILE` | N | N | Y | N | N | Also `pkb-excalidraw unlock`. [`src/bin/pkb_excalidraw.rs:5938`](src/bin/pkb_excalidraw.rs#L5938) |
| `pkb-excalidraw arrange duplicate` | `pkb-excalidraw` CLI | Clones elements and their bound text at an offset with new IDs and indices | `FILE`, `--ids <id,...>`, `--offset DX,DY` (default 20,20) | Writes `FILE`; prints new IDs | Y | N | N | N | N | Also `pkb-excalidraw duplicate`. [`src/bin/pkb_excalidraw.rs:5953`](src/bin/pkb_excalidraw.rs#L5953) |
| `pkb-excalidraw snapshot save` | `pkb-excalidraw` CLI | Saves the live elements as a named snapshot | `FILE`, `save <name>` | Writes `.snapshots_<stem>/<name>.json` beside `FILE` | Y | N | N | N | N | Items here are snapshots; an existing name is overwritten. [`src/bin/pkb_excalidraw.rs:6207`](src/bin/pkb_excalidraw.rs#L6207); [`src/bin/pkb_excalidraw.rs:4750`](src/bin/pkb_excalidraw.rs#L4750) |
| `pkb-excalidraw snapshot list` | `pkb-excalidraw` CLI | One entry per saved snapshot | `FILE`, `list` | JSON array of name, `createdAt`, `elementCount` | N | Y | N | N | Y | [`src/bin/pkb_excalidraw.rs:6217`](src/bin/pkb_excalidraw.rs#L6217); [`src/bin/pkb_excalidraw.rs:4772`](src/bin/pkb_excalidraw.rs#L4772) |
| `pkb-excalidraw snapshot restore` | `pkb-excalidraw` CLI | Replaces the canvas's element list with a snapshot's | `FILE`, `restore <name>` | Writes `FILE` | Y | N | Y | Y | N | Create/Update/Delete: the whole element list is replaced. [`src/bin/pkb_excalidraw.rs:6223`](src/bin/pkb_excalidraw.rs#L6223); [`src/bin/pkb_excalidraw.rs:4803`](src/bin/pkb_excalidraw.rs#L4803) |
| `pkb-excalidraw export` | `pkb-excalidraw` CLI | Re-serialises the canvas as plain Excalidraw JSON or Obsidian `.excalidraw.md` | `FILE`, `--out <path>`, `--format json\|obsidian` | Text to stdout or `<path>` | N | Y | N | N | N | Obsidian format is also chosen when `--out` ends in `.md`. [`src/bin/pkb_excalidraw.rs:6089`](src/bin/pkb_excalidraw.rs#L6089) |
| `pkb-excalidraw import` | `pkb-excalidraw` CLI | Appends elements from a JSON or Obsidian scene with fresh indices, or replaces all elements with `--replace` | `FILE`, `<src.json>`, `<src.md>` or `-`, `--replace` | Writes `FILE`; prints count | Y | N | N | Y | N | Delete applies only with `--replace`. [`src/bin/pkb_excalidraw.rs:6137`](src/bin/pkb_excalidraw.rs#L6137) |
| `pkb-excalidraw screenshot` | `pkb-excalidraw` CLI | Renders the canvas to SVG, or to PNG via `resvg`, `rsvg-convert` or ImageMagick | `FILE`, `--out <path>`, `--format svg\|png`, `--no-background` | SVG on stdout, or image at `<path>` | N | Y | N | N | N | PNG requires `--out`. [`src/bin/pkb_excalidraw.rs:5785`](src/bin/pkb_excalidraw.rs#L5785); [`src/bin/pkb_excalidraw.rs:4565`](src/bin/pkb_excalidraw.rs#L4565) |
| `pkb-excalidraw theme export` | `pkb-excalidraw` CLI | Prints the built-in `retro-terminal` theme | `FILE`, `theme export [out.json]` | Theme JSON to stdout or `out.json` | N | N | N | N | N | Ignores the canvas content, though `FILE` must parse. [`src/bin/pkb_excalidraw.rs:5707`](src/bin/pkb_excalidraw.rs#L5707) |
| `pkb-excalidraw theme apply` | `pkb-excalidraw` CLI | Applies a theme's font, roughness, fill, stroke and role colours to all elements or one element | `FILE`, `theme apply <theme>`, `--all` or `--id <id>` | Writes `FILE` | N | N | Y | N | N | `<theme>` is `default`, `retro-terminal`, `aops-default` or a theme JSON path. [`src/bin/pkb_excalidraw.rs:5721`](src/bin/pkb_excalidraw.rs#L5721) |

## MCP Tools

The `pkb` binary exposes 39 tools over MCP stdio transport. Any MCP-compatible client can use them.
It also provides **MCP prompts** to guide AI assistants through common search and navigation patterns.

### Prompts

| Prompt | Description | Guidance |
|--------|-------------|----------|
| `find-task` | "How do I find a task about X?" | Demonstrates `task_search` then `get_task` |
| `explore-topic` | "What do we know about X?" | Demonstrates `search` then `get_document` |
| `navigate-graph` | "What's connected to X?" | Demonstrates `get_task` / `get_document` for relationships |
| `find-by-tag` | "Show me everything tagged X" | Demonstrates `search_by_tag` usage |

### Tools

| Category | Tools |
|----------|-------|
| **Search** | `search`, `get_document`, `list_documents`, `find_duplicates` |
| **Tasks** | `task_search`, `list_tasks`, `get_task`, `create_task`, `create_subtask`, `update_task`, `complete_task`, `release_task`, `decompose_task`, `get_dependency_tree`, `get_task_children`, `task_summary`, `get_network_metrics`, `top_n_by_metric` |
| **Memory** | `retrieve_memory`, `search_by_tag`, `list_memories`, `delete` (pass `type: "memory"` to restrict to memory-type documents) |
| **CRUD** | `create`, `create_memory`, `append`, `delete` |
| **Graph** | `pkb_trace`, `pkb_orphans`, `graph_stats`, `graph_excalidraw`, `diff_excalidraw`, `sync_excalidraw`, `export_graph` |
| **Batch** | `batch_update`, `batch_reparent`, `batch_archive`, `batch_merge`, `batch_create_epics`, `batch_reclassify`, `merge_node` |
| **System** | `get_stats` |

### `export_graph`: Graph export (DOT or JSON)

Read-only export of the knowledge/task graph as GraphViz DOT syntax (`digraph PKB { ... }`) or structured JSON, for external rendering (`dot`, `neato`, `sfdp`), deep structural analysis, or dashboards — a static, one-way counterpart to `graph_excalidraw`'s interactive round-trip canvas.

| Parameter | Type | Description |
|-----------|------|--------------|
| `format` | string, optional | Output format: `"dot"` (GraphViz DOT digraph syntax, default) or `"json"` (structured graph JSON with nodes, edges, ready, blocked, roots, and focus). |
| `focus` | string, optional | Focus node ID, filename, or title (flexible resolution). Omit for the full active graph. |
| `max_depth` | integer, optional | Traversal depth in hops from `focus` (1–5, default 2). Ignored if `focus` is omitted. |
| `project` | string, optional | Filter to nodes whose `project` field matches exactly. |
| `include_done` | boolean, optional | Include `done`/`cancelled` nodes (default `false`). |

Nodes carry `label`, `type`, and `status` attributes. Edges cover `depends_on`, `soft_depends_on`, `contributes_to`, `supersedes`, `closes`, and parent-child hierarchy — `blocks:` frontmatter is compiled into `depends_on` edges at graph-build time, so it needs no separate edge type. Labels and attribute values are escaped for safe DOT syntax (backslashes, quotes, embedded newlines).

Render the result with GraphViz:

```bash
# via an MCP client that writes the tool's text output to graph.dot
dot -Tsvg graph.dot -o graph.svg
```

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
| `AOPS_DUMMY_EMBEDDER` | `false` | Use zero-vector dummy embedder (for tests/offline) |
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
