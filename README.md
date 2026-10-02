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

#### Excalidraw Tool Comparison

*Note: Create/Read/Update/Delete/List flags indicate operations on Excalidraw canvas elements, PKB notes, or output files.*

| Tool | Surface (CLI/MCP) | What it does | Input | Output | Create | Read | Update | Delete | List | Notes |
|------|-------------------|--------------|-------|--------|:------:|:----:|:------:|:------:|:----:|-------|
| `pkb graph --format excalidraw` | CLI | Clean export of full graph or ego-network to an Excalidraw JSON canvas | `--focus <id>`, `--hops <n>`, optional `-o <path>` | Excalidraw JSON file or stdout | N | Y | N | N | Y | Defined in [`src/cli.rs:2549`](src/cli.rs#L2549); layout generator in [`src/excalidraw/layout.rs:47`](src/excalidraw/layout.rs#L47) |
| `pkb excalidraw export` | CLI | Exports graph or ego-network; merges live PKB state into existing canvas file if present, preserving layout | `<output_path>`, optional `--focus <id>`, `--hops <n>` | Excalidraw JSON canvas file | N | Y | Y | N | Y | Defined in [`src/cli.rs:3843`](src/cli.rs#L3843); schema in [`src/cli.rs:655`](src/cli.rs#L655); merge logic in [`src/excalidraw/merge.rs:164`](src/excalidraw/merge.rs#L164) |
| `pkb excalidraw diff` | CLI | Computes 3-way structural diff between an edited Excalidraw canvas and live PKB | `<canvas_path>`, optional `--base <path>`, `--json` | Human-readable diff summary or JSON | N | Y | N | N | N | Defined in [`src/cli.rs:3882`](src/cli.rs#L3882); schema in [`src/cli.rs:669`](src/cli.rs#L669); diff engine in [`src/excalidraw/diff.rs:184`](src/excalidraw/diff.rs#L184) |
| `pkb excalidraw sync` | CLI | Applies canvas mutations back to PKB markdown files (creates notes, updates frontmatter/edges, removes edges) | `<canvas_path>`, optional `--base <path>`, `--dry-run`, `--sync-edge-removals` | Sync summary report; modifies PKB files on disk | Y | Y | Y | Y | N | Defined in [`src/cli.rs:3910`](src/cli.rs#L3910); schema in [`src/cli.rs:683`](src/cli.rs#L683); sync engine in [`src/excalidraw/merge.rs:515`](src/excalidraw/merge.rs#L515). Deletions apply to `depends_on` edge removals only |
| `graph_excalidraw` | MCP | Generates Excalidraw V2 JSON diagram for an ego neighborhood around node_id or top focus picks across the PKB | `node_id` (string, optional), `hops` (integer, optional) | Pretty-printed Excalidraw V2 JSON text | N | Y | N | N | Y | Defined in [`src/mcp_server/handlers_batch.rs:397`](src/mcp_server/handlers_batch.rs#L397); schema in [`src/mcp_server/schemas.rs:588`](src/mcp_server/schemas.rs#L588) |
| `diff_excalidraw` | MCP | Parses Excalidraw JSON string and returns structured 3-way diff (`GraphDiff`) against live PKB | `canvas` (string, required), `base` (string, optional) | Structured `GraphDiff` JSON text | N | Y | N | N | N | Defined in [`src/mcp_server/handlers_batch.rs:456`](src/mcp_server/handlers_batch.rs#L456); schema in [`src/mcp_server/schemas.rs:623`](src/mcp_server/schemas.rs#L623) |
| `sync_excalidraw` | MCP | Applies card additions, frontmatter edits, and edge mutations from Excalidraw JSON back to PKB markdown files | `canvas` (string, required), `base` (string, optional), `dry_run` (bool), `sync_edge_removals` (bool) | Sync report JSON (dry run preview or counts of created/updated items and warnings) | Y | Y | Y | Y | N | Defined in [`src/mcp_server/handlers_batch.rs:503`](src/mcp_server/handlers_batch.rs#L503); schema in [`src/mcp_server/schemas.rs:638`](src/mcp_server/schemas.rs#L638). Edge removals only |
| `pkb-excalidraw summary` | CLI | Outputs per-type element counts and scene bounding box extents | `FILE` | Text summary of element counts and coordinate extents | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:2058`](src/bin/pkb_excalidraw.rs#L2058); dispatch in [`src/bin/pkb_excalidraw.rs:4103`](src/bin/pkb_excalidraw.rs#L4103) |
| `pkb-excalidraw map` | CLI | Generates TSV projection of top-level elements with geometry, colors, labels, and arrow relationships | `FILE` | TSV lines of element attributes and arrow bindings | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:2106`](src/bin/pkb_excalidraw.rs#L2106); dispatch in [`src/bin/pkb_excalidraw.rs:4104`](src/bin/pkb_excalidraw.rs#L4104) |
| `pkb-excalidraw nodes` | CLI | Outputs TSV table of node shapes (coordinates, dimensions, role, bound text label) | `FILE` | TSV table of nodes | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:2167`](src/bin/pkb_excalidraw.rs#L2167); dispatch in [`src/bin/pkb_excalidraw.rs:4105`](src/bin/pkb_excalidraw.rs#L4105) |
| `pkb-excalidraw edges` | CLI | Outputs TSV table of 2-bound arrows showing source, target, and edge label (synonym `arrows`) | `FILE` | TSV table of directed arrows | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:2178`](src/bin/pkb_excalidraw.rs#L2178); dispatch in [`src/bin/pkb_excalidraw.rs:4106`](src/bin/pkb_excalidraw.rs#L4106) |
| `pkb-excalidraw inspect` | CLI | Displays formatted structural details, bindings, and attributes of a single element | `FILE`, `<id>` | Plain text element inspection | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2190`](src/bin/pkb_excalidraw.rs#L2190); dispatch in [`src/bin/pkb_excalidraw.rs:4107`](src/bin/pkb_excalidraw.rs#L4107) |
| `pkb-excalidraw get` | CLI | Returns raw pretty-printed JSON representation of a single element by ID | `FILE`, `<id>` | Pretty-printed JSON | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2244`](src/bin/pkb_excalidraw.rs#L2244); dispatch in [`src/bin/pkb_excalidraw.rs:4114`](src/bin/pkb_excalidraw.rs#L4114) |
| `pkb-excalidraw describe` | CLI | Generates natural language summary of diagram structure, groups, frames, and connected components | `FILE` | Text narrative describing scene layout | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:3321`](src/bin/pkb_excalidraw.rs#L3321); dispatch in [`src/bin/pkb_excalidraw.rs:4930`](src/bin/pkb_excalidraw.rs#L4930) |
| `pkb-excalidraw query` | CLI | Queries elements matching type, bounding box, key-value filters, or custom JSON expressions | `FILE`, optional `--type`, `--bbox`, `--filter`, `--filter-json` | Filtered element list / TSV | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:3511`](src/bin/pkb_excalidraw.rs#L3511); dispatch in [`src/bin/pkb_excalidraw.rs:5165`](src/bin/pkb_excalidraw.rs#L5165) |
| `pkb-excalidraw screenshot` | CLI | Exports canvas as SVG or PNG image file, or outputs SVG to stdout | `FILE`, optional `--out <path>`, `--format svg\|png`, `--no-background` | Image file or SVG text | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:3771`](src/bin/pkb_excalidraw.rs#L3771); dispatch in [`src/bin/pkb_excalidraw.rs:4933`](src/bin/pkb_excalidraw.rs#L4933) |
| `pkb-excalidraw style` | CLI | Computes value histograms and distribution statistics for stroke, fill, roughness, and fonts across elements | `FILE` | Text histogram tables | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:2255`](src/bin/pkb_excalidraw.rs#L2255); dispatch in [`src/bin/pkb_excalidraw.rs:4121`](src/bin/pkb_excalidraw.rs#L4121) |
| `pkb-excalidraw check` | CLI | Audits canvas structural integrity, unique IDs, fractional index sorting, and valid binding references | `FILE` | Exit 0 with `OK`, or exit 1 with list of invariant failures | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2318`](src/bin/pkb_excalidraw.rs#L2318); dispatch in [`src/bin/pkb_excalidraw.rs:4122`](src/bin/pkb_excalidraw.rs#L4122) |
| `pkb-excalidraw overlap` | CLI | Audits scene for overlapping top-level bounding boxes (AABB collisions) | `FILE` | Exit 0 with `OK`, or exit 1 with collision list | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2904`](src/bin/pkb_excalidraw.rs#L2904); dispatch in [`src/bin/pkb_excalidraw.rs:4135`](src/bin/pkb_excalidraw.rs#L4135) |
| `pkb-excalidraw arrows-check` | CLI | Audits 2D arrow trajectories to ensure arrows do not intersect boxes they are not bound to | `FILE` | Exit 0 with `OK`, or exit 1 with intersection list | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2979`](src/bin/pkb_excalidraw.rs#L2979); dispatch in [`src/bin/pkb_excalidraw.rs:4148`](src/bin/pkb_excalidraw.rs#L4148) |
| `pkb-excalidraw diff` | CLI | Computes element-level diff between two Excalidraw files | `FILE1`, `FILE2` | Text summary of added, modified, and removed elements | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2516`](src/bin/pkb_excalidraw.rs#L2516); dispatch in [`src/bin/pkb_excalidraw.rs:4161`](src/bin/pkb_excalidraw.rs#L4161) |
| `pkb-excalidraw struct-diff` | CLI | Computes semantic structural diff of nodes and edges between two files, ignoring coordinate jitter | `FILE1`, `FILE2` | Semantic structural diff lines | N | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:2786`](src/bin/pkb_excalidraw.rs#L2786); dispatch in [`src/bin/pkb_excalidraw.rs:4183`](src/bin/pkb_excalidraw.rs#L4183) |
| `pkb-excalidraw lib` | CLI | Lists component items inside an `.excalidrawlib` library file | `FILE.excalidrawlib` | List of library item indices, titles, and element counts | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:3140`](src/bin/pkb_excalidraw.rs#L3140); dispatch in [`src/bin/pkb_excalidraw.rs:4208`](src/bin/pkb_excalidraw.rs#L4208) |
| `pkb-excalidraw item` | CLI | Extracts an item from an `.excalidrawlib` file and mints fresh element IDs and fractional indices after a given index | `FILE.excalidrawlib`, `SELECTOR`, `--after <index>`, optional `--at <x,y>` | Re-minted element JSON array to stdout | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:3169`](src/bin/pkb_excalidraw.rs#L3169); dispatch in [`src/bin/pkb_excalidraw.rs:4211`](src/bin/pkb_excalidraw.rs#L4211) |
| `pkb-excalidraw add-node` | CLI | Inserts container shape (rectangle, ellipse, diamond) with centered bound text and optional style presets | `FILE`, `--text "<text>"`, optional `--type`, `--at X,Y`, `--size W,H`, `--role`, `--color`, `--id`, `--angle`, `--roughness`, `--fill-style`, `--preset` | Mutates file; prints created shape and text IDs | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:650`](src/bin/pkb_excalidraw.rs#L650); dispatch in [`src/bin/pkb_excalidraw.rs:4310`](src/bin/pkb_excalidraw.rs#L4310) |
| `pkb-excalidraw add-text` | CLI | Inserts standalone text element at specified coordinates | `FILE`, `--text "<text>"`, `--at X,Y`, optional `--font-size`, `--color` | Mutates file; prints created text ID | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:819`](src/bin/pkb_excalidraw.rs#L819); dispatch in [`src/bin/pkb_excalidraw.rs:4401`](src/bin/pkb_excalidraw.rs#L4401) |
| `pkb-excalidraw connect` | CLI | Connects two shapes with a 2-bound directed arrow, establishing reciprocal binding references | `FILE`, `--from <id1>`, `--to <id2>`, optional `--label`, `--color`, `--curved`, `--stroke-style` | Mutates file; prints created arrow ID | Y | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:890`](src/bin/pkb_excalidraw.rs#L890); dispatch in [`src/bin/pkb_excalidraw.rs:4446`](src/bin/pkb_excalidraw.rs#L4446). Updates `boundElements` on connected endpoints |
| `pkb-excalidraw set-text` | CLI | Updates text on a text element or container shape with automatic symmetrical resizing (synonym `fit`) | `FILE`, `<id>`, `"<new_text>"` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1092`](src/bin/pkb_excalidraw.rs#L1092); dispatch in [`src/bin/pkb_excalidraw.rs:4497`](src/bin/pkb_excalidraw.rs#L4497) |
| `pkb-excalidraw fit` | CLI | Center-expanded text replacement and container shape resize (synonym of `set-text`) | `FILE`, `<id>`, `"<new_text>"` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1092`](src/bin/pkb_excalidraw.rs#L1092); dispatch in [`src/bin/pkb_excalidraw.rs:4497`](src/bin/pkb_excalidraw.rs#L4497) |
| `pkb-excalidraw move-elem` | CLI | Translates element position, updating bound text and connected arrow endpoints | `FILE`, `<id>`, `--to X,Y` or `--by DX,DY` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1223`](src/bin/pkb_excalidraw.rs#L1223); dispatch in [`src/bin/pkb_excalidraw.rs:4519`](src/bin/pkb_excalidraw.rs#L4519) |
| `pkb-excalidraw delete-elem` | CLI | Deletes element by ID, removing bound text and unbinding or cascading connected arrows | `FILE`, `<id>`, optional `--cascade-arrows` | Mutates file | N | Y | Y | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:1375`](src/bin/pkb_excalidraw.rs#L1375); dispatch in [`src/bin/pkb_excalidraw.rs:4565`](src/bin/pkb_excalidraw.rs#L4565) |
| `pkb-excalidraw update` | CLI | Updates element fields with arbitrary properties supplied as JSON | `FILE`, `<id>`, `--set '<json>'` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1990`](src/bin/pkb_excalidraw.rs#L1990); dispatch in [`src/bin/pkb_excalidraw.rs:5122`](src/bin/pkb_excalidraw.rs#L5122) |
| `pkb-excalidraw update-node` | CLI | Updates visual styling properties of a specific node (angle, roughness, fill style, preset) | `FILE`, `--id <id>`, optional `--angle`, `--roughness`, `--fill-style`, `--preset` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:590`](src/bin/pkb_excalidraw.rs#L590); dispatch in [`src/bin/pkb_excalidraw.rs:4255`](src/bin/pkb_excalidraw.rs#L4255) |
| `pkb-excalidraw arrange align` | CLI | Aligns multiple elements along an axis (left, center, right, top, middle, bottom; also callable as `align`) | `FILE`, `--ids <id1,id2...>`, `--to <left\|center\|right\|top\|middle\|bottom>` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1586`](src/bin/pkb_excalidraw.rs#L1586); dispatch in [`src/bin/pkb_excalidraw.rs:4996`](src/bin/pkb_excalidraw.rs#L4996) |
| `pkb-excalidraw arrange distribute` | CLI | Distributes elements evenly along horizontal or vertical axis (also callable as `distribute`) | `FILE`, `--ids <id1,id2...>`, `--to <horizontal\|vertical>` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1688`](src/bin/pkb_excalidraw.rs#L1688); dispatch in [`src/bin/pkb_excalidraw.rs:5018`](src/bin/pkb_excalidraw.rs#L5018) |
| `pkb-excalidraw arrange group` | CLI | Groups multiple elements under a common group ID (also callable as `group`) | `FILE`, `--ids <id1,id2...>` | Mutates file; prints new group ID | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1770`](src/bin/pkb_excalidraw.rs#L1770); dispatch in [`src/bin/pkb_excalidraw.rs:5040`](src/bin/pkb_excalidraw.rs#L5040) |
| `pkb-excalidraw arrange ungroup` | CLI | Removes group associations for elements or a group ID (also callable as `ungroup`) | `FILE`, optional `--group <id>` or `--ids <id1,id2...>` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1806`](src/bin/pkb_excalidraw.rs#L1806); dispatch in [`src/bin/pkb_excalidraw.rs:5055`](src/bin/pkb_excalidraw.rs#L5055) |
| `pkb-excalidraw arrange lock` | CLI | Locks elements to prevent accidental canvas movement (also callable as `lock` / `unlock`) | `FILE`, `--ids <id1,id2...>`, mode lock or unlock | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1843`](src/bin/pkb_excalidraw.rs#L1843); dispatch in [`src/bin/pkb_excalidraw.rs:5071`](src/bin/pkb_excalidraw.rs#L5071) |
| `pkb-excalidraw arrange unlock` | CLI | Unlocks previously locked elements (also callable as `unlock`) | `FILE`, `--ids <id1,id2...>` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1843`](src/bin/pkb_excalidraw.rs#L1843); dispatch in [`src/bin/pkb_excalidraw.rs:5087`](src/bin/pkb_excalidraw.rs#L5087) |
| `pkb-excalidraw arrange duplicate` | CLI | Clones elements and their bound text with a coordinate offset (also callable as `duplicate`) | `FILE`, `--ids <id1,id2...>`, optional `--offset DX,DY` | Mutates file; prints new element IDs | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1883`](src/bin/pkb_excalidraw.rs#L1883); dispatch in [`src/bin/pkb_excalidraw.rs:5101`](src/bin/pkb_excalidraw.rs#L5101) |
| `pkb-excalidraw apply` | CLI | Applies an atomic declarative JSON patch with create, update, and delete arrays in one atomic write | `FILE`, `<patch.json \| ->` | Mutates file; reports created, updated, and deleted counts | Y | Y | Y | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:3869`](src/bin/pkb_excalidraw.rs#L3869); dispatch in [`src/bin/pkb_excalidraw.rs:5206`](src/bin/pkb_excalidraw.rs#L5206) |
| `pkb-excalidraw batch` | CLI | Executes a transactional array of mutation operations (`add-node`, `connect`, `move`, `delete`, etc.) atomically | `FILE`, `<changes.json \| ->` | Mutates file; reports applied mutations count | Y | Y | Y | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:4587`](src/bin/pkb_excalidraw.rs#L4587) |
| `pkb-excalidraw clear` | CLI | Clears all elements from the canvas, producing a clean blank scene | `FILE`, `--yes` | Mutates file; reports cleared element count | N | Y | N | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:2044`](src/bin/pkb_excalidraw.rs#L2044); dispatch in [`src/bin/pkb_excalidraw.rs:5387`](src/bin/pkb_excalidraw.rs#L5387) |
| `pkb-excalidraw snapshot save` | CLI | Saves current scene elements as a named snapshot in canvas metadata | `FILE`, `save <name>` | Mutates file | Y | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:3956`](src/bin/pkb_excalidraw.rs#L3956); dispatch in [`src/bin/pkb_excalidraw.rs:5355`](src/bin/pkb_excalidraw.rs#L5355) |
| `pkb-excalidraw snapshot list` | CLI | Lists all saved snapshots stored in canvas metadata | `FILE`, `list` | Text list of snapshot names and dates | N | Y | N | N | Y | Defined in [`src/bin/pkb_excalidraw.rs:3978`](src/bin/pkb_excalidraw.rs#L3978); dispatch in [`src/bin/pkb_excalidraw.rs:5365`](src/bin/pkb_excalidraw.rs#L5365) |
| `pkb-excalidraw snapshot restore` | CLI | Restores canvas elements from a previously saved named snapshot | `FILE`, `restore <name>` | Mutates file | N | Y | Y | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:4009`](src/bin/pkb_excalidraw.rs#L4009); dispatch in [`src/bin/pkb_excalidraw.rs:5371`](src/bin/pkb_excalidraw.rs#L5371) |
| `pkb-excalidraw export` | CLI | Exports canvas to JSON or Obsidian Excalidraw Markdown format (`.md`) | `FILE`, optional `--out <path>`, `--format json\|obsidian` | JSON/Markdown string or output file | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:5237`](src/bin/pkb_excalidraw.rs#L5237); Obsidian serializer in [`src/bin/pkb_excalidraw.rs:499`](src/bin/pkb_excalidraw.rs#L499) |
| `pkb-excalidraw import` | CLI | Imports scene elements from JSON or Obsidian Markdown, either merging or replacing elements | `FILE`, `<src.json \| src.md \| ->`, optional `--replace` | Mutates file; reports imported count | Y | Y | Y | Y | N | Defined in [`src/bin/pkb_excalidraw.rs:5285`](src/bin/pkb_excalidraw.rs#L5285); Obsidian parser in [`src/bin/pkb_excalidraw.rs:482`](src/bin/pkb_excalidraw.rs#L482). Deletes existing elements when `--replace` is passed |
| `pkb-excalidraw theme export` | CLI | Exports built-in default/retro theme palette and style configuration as JSON | `FILE`, `theme export [out.json]` | Theme JSON to stdout or file | Y | Y | N | N | N | Defined in [`src/bin/pkb_excalidraw.rs:4855`](src/bin/pkb_excalidraw.rs#L4855) |
| `pkb-excalidraw theme apply` | CLI | Applies theme palette, roughness, and semantic color roles to canvas elements | `FILE`, `theme apply <theme> [--all \| --id <id>]` | Mutates file | N | Y | Y | N | N | Defined in [`src/bin/pkb_excalidraw.rs:1503`](src/bin/pkb_excalidraw.rs#L1503); dispatch in [`src/bin/pkb_excalidraw.rs:4869`](src/bin/pkb_excalidraw.rs#L4869) |

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
