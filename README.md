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
status: inbox
priority: 2
tags: [backend, security]
depends_on: [design-doc-456]
parent: epic-auth-789
project: aops
---

The actual content of the document goes here.
Any markdown is fine.
```

All frontmatter fields are optional. Files without frontmatter are indexed by filename and content.

### Status Values and Transitions

All nodes progress through a canonical lifecycle (`src/graph.rs:880-893`):

$$\text{inbox} \longrightarrow \text{ready} \longrightarrow \text{queued} \longrightarrow \text{in\_progress} \longrightarrow \text{merge\_ready} \longrightarrow \text{done}$$

with branching states to `review`, `blocked`, `paused`, `someday`, `cancelled`, and `partial`.

| Status | Category | Meaning |
|---|---|---|
| `inbox` | Open | **Default.** Captured but untriaged — unknown priority or readiness. |
| `ready` | Open | Fully decomposed to a leaf task with all hard dependencies resolved (auto-computed). |
| `queued` | Open | **Human-gated.** Promoted manually by the user; agents pull strictly from `queued`. |
| `in_progress` | Open | Claimed and actively being executed by a human or agent. |
| `merge_ready` | Open | Work complete and committed, awaiting review/merge. |
| `review` | Open | Awaiting human review (mid-flight attention or post-PR requested changes). |
| `paused` | Open | Intentionally deferred mid-flight with intent to resume. |
| `someday` | Open | Explicitly deferred idea; differs from inbox by conscious deferral. |
| `partial` | Open | Worker stopped at a legitimate scope seam (draft PR + live follow-up task). |
| `blocked` | Blocked | Blocked by an unresolved hard dependency (`depends_on`). |
| `done` | Terminal | Completed and verified successfully. |
| `cancelled` | Terminal | Will not be done; discarded. |

### Valid Node Types

Every entity in the PKB is an instance of the fundamental `GraphNode` structure (`src/graph.rs:19`). There are exactly **23 valid canonical node types** defined in `VALID_NODE_TYPES` (`src/graph.rs:970-998`). Writes specifying any other type are rejected at parse and write boundaries (`src/document_crud.rs:191-194`).

#### 1. Actionable Work Items (`TASK_TYPES`)

Participate in active execution queues, ready/blocked triage, and dashboards (`src/graph.rs:952`):

| Type | Role & Semantics | Hierarchy & Parenting Rules | Source |
|---|---|---|---|
| `epic` | Bundle of related work that together achieves a coherent aim. | Root-level container or child of another epic. Never parented under a task, goal, or target. | `src/graph.rs:952, 972` |
| `task` | Discrete, concrete deliverable completable in a single focused session. Primary actionable unit. | Child of an epic or parent task. Allowed root-level for standalones. | `src/graph.rs:952, 973` |
| `learn` | Observational tracking item: discovery, research probe, or investigation spike. | Child of an epic or task. **Excluded from `ready_tasks()`** until decomposed into actionable follow-up tasks. | `src/graph.rs:952, 974` |
| `pr` | Pull request deliverable tracking an external code review or branch merge. | Child of an epic or task. | `src/graph.rs:952, 975` |

#### 2. Workflow Templates

Meta-artifacts that standardise repeatable procedures (`src/graph.rs:950-951, 977`):

| Type | Role & Semantics | Lifecycle & Instantiation | Source |
|---|---|---|---|
| `template` | Reusable canonical body and metadata for recurring workflows (daily standup, issue sweep, retrospective). | **Not an actionable work item.** Calling `claim_task(id)` on a template instantiates a fresh datestamped `type: task` instance (`<slug>-<date>-<host>.md`). Excluded from ready queues. | `src/graph.rs:950-951, 977` |

#### 3. Strategic Priorities & Knowledge References

Out-of-tree strategic attractors and reference knowledge (`src/graph.rs:954-963, 979-992`):

| Type | Role & Semantics | Operational Constraints | Source |
|---|---|---|---|
| `goal` | Strategic identity-level commitment (*why*). Roots of meaning. | **Out of the work tree.** Never a parent, never parented. **Unquantifiable**: carries NO `severity`, NO `consequence`, NO `due`. Connected to work only via `contributes_to`. | `src/graph.rs:957-963, 979` |
| `target` | Countable, measurable milestone (*what*) — done / not done. | **Out of the work tree.** Never a parent, never parented (`STRATEGIC_TARGET_TYPES`). Carries `severity` (SEV0–SEV4) + `consequence` (+ optional `due`). Propagates stakes to work via `contributes_to`. | `src/graph.rs:957-963, 980` |
| `note` | General knowledge note, atomic thought, meeting record, or insight. | Reference tier. Searchable, excluded from task queues. | `src/graph.rs:981` |
| `knowledge` | Durable, consolidated, and synthesised topic document. | Reference tier. High-authority search target. | `src/graph.rs:982` |
| `memory` | Atomic system or agent memory: durable fact, decision, or constraint. | Reference tier. Managed via `create_memory`. | `src/graph.rs:983` |
| `insight` | High-level synthesis or breakthrough conceptual finding. | Reference tier. | `src/graph.rs:984` |
| `observation` | Empirical finding, audit observation, or system test note. | Reference tier. | `src/graph.rs:985` |
| `contact` | Profile note for a collaborator, stakeholder, or person. | Reference tier. | `src/graph.rs:986` |
| `document` | General unstructured document, imported file, or instructions. | Reference tier. Catch-all for imported references. | `src/graph.rs:987` |
| `reference` | External literature reference, article summary, or reading guide. | Reference tier. | `src/graph.rs:988` |
| `review` | Peer review, manuscript evaluation, or grant assessment. | Reference tier. | `src/graph.rs:989` |
| `case` | Case study or legal analysis. | Reference tier. | `src/graph.rs:990` |
| `spec` | Technical architecture specification or RFC. | Reference tier. | `src/graph.rs:991` |
| `prototype` | Obligation class template (e.g. peer review load) whose instances inherit defaults. | Reference tier. Urgency template. | `src/graph.rs:992` |

#### 4. Structural & Logging Infrastructure

Navigation and audit infrastructure (`src/graph.rs:994-997`):

| Type | Role & Semantics | Operational Role | Source |
|---|---|---|---|
| `index` | Map of Content (MOC) or navigational hub. | Structural navigation. | `src/graph.rs:994` |
| `daily` | Daily tracking and planning note. | Structural time-anchor. | `src/graph.rs:995` |
| `session-log` | Raw session transcript or agent execution log. | Ephemeral execution record. | `src/graph.rs:996` |
| `audit-report` | System, code, or data audit output. | Automated verification record. | `src/graph.rs:997` |

#### 5. Retired Types, Invalid Labels, and Secondary Classifications

- **`project` is RETIRED as a node type** (`src/graph.rs:967-969`): "Project" is strictly an operational repository routing slug carried in `project:` frontmatter (referencing `polecat.yaml`). Legacy files with `type: project` are read-coerced to `epic`.
- **`bug`, `feature`, `action`, `subproject`, `milestone` are NOT node types**: These are legacy aliases mapped to canonical types (`bug`/`feature`/`action` $\to$ `task`; `subproject`/`milestone` $\to$ `epic`) (`src/lint.rs:204-227`).
- **`classification` is an orthogonal secondary label**: Records semantic subtypes (`bug`, `feature`, `spike`, `chore`, `refactor`, `docs`) without multiplying structural node types (`references/TAXONOMY.md:141-154`). Ranking deliberately ignores `classification`.
- **Target parenting is strictly forbidden**: Targets and goals cannot serve as structural parents (`GraphStore::reject_target_as_parent`). Work links upward to targets via `contributes_to`, never `parent`.

### Priority Levels

| Level | Label | Use |
|---|---|---|
| `0` | P0 — Critical | Immediate emergency; top priority |
| `1` | P1 — High | Active commitment; this week |
| `2` | P2 — Standard | Default; normal scheduled work |
| `3` | P3 — Low | Background; pick up when capacity exists |
| `4` | P4 — Backlog | May never happen; keep visible |

Priority propagates upward via `effective_priority` (or `effective_intent`): a P3 task blocking a P0 inherits P0 priority in scoring even though its own authored field remains P3.

### Edge Types

The knowledge graph supports seven edge types extracted from frontmatter and content:

| Edge type | Source | Affects ready/blocked? | Affects importance propagation? | Notes |
|---|---|---|---|---|
| `parent` | `parent:` frontmatter or `children:` list | ✅ (via unfinished children) | ✅ (0.5 factor) | Structural containment hierarchy |
| `depends_on` | `depends_on:` list | ✅ blocks task | ✅ (1.0 factor) | Hard dependency blocker |
| `soft_depends_on` | `soft_depends_on:` list | ❌ | ✅ (0.3 factor) | Enabling / informational ordering |
| `link` | `[[wikilinks]]` and markdown links in body | ❌ | ❌ | Cross-references; counted as backlinks |
| `supersedes` | `supersedes:` frontmatter | ❌ | ❌ | Node replaces another |
| `contributes_to` | `contributes_to:` list with verbal weights | ❌ | ✅ (verbal scale factor) | Strategic priority toward targets/goals |
| `similar_to` | BGE-M3 vector cosine $\ge 0.85$ | ❌ | ❌ | Semantic proximity; excluded from causal paths |

---

## Ranking System & Prioritisation Model

Tasks in `mem` are ranked by an explicit 4-component lexicographical sort tuple (`src/graph.rs:104-109`):

$$\mathbf{focus\_tuple} = (\text{severity\_gate}, \text{deadline\_band}, \text{cost\_of\_delay}, \text{tie\_breakers})$$

All task listings in MCP (`list_tasks`) and CLI (`pkb tasks`, `pkb list`) sort through this single tuple comparator (`src/graph_store.rs:937-949`, `GraphStore::focus_cmp`).

### 1. Non-Compensatory Pre-Filters

Before tuple evaluation, tasks are checked against two absolute gates:
1. **Affordable Loss Filter** (`src/graph_store.rs:1989-1995`): If `node.affordable_loss == Some(false)`, the node is completely zeroed and excluded from rankings (`focus_tuple = None`, `focus_score = None`).
2. **Terminal Status Filter** (`src/graph_store.rs:1997-2001`): Completed work (`done`, `cancelled`) is unscored (`focus_tuple = None`, `focus_score = None`).

### 2. Lexicographical Tuple Ordering (`FocusTuple::cmp`)

Pairwise sorting in `FocusTuple::cmp` evaluates components in strict sequence (`src/graph.rs:152-165`):

```text
1. severity_gate        (Catastrophic > Normal)
2. deadline_band        (Overdue > Imminent > Urgent > Approaching > None)
3. cost_of_delay        (DESC: higher cost-of-delay sorts first)
4. tie_breakers:
   a. downstream_weight_x10 (DESC: higher downstream mass sorts first)
   b. unlock_breadth_x10    (DESC: higher unblocked dependent mass sorts first)
   c. age_staleness         (DESC: older unworked P2+ tasks sort first)
   d. effective_intent      (ASC: P0 before P1 before P2...)
   e. order                 (ASC: authored sequence order)
   f. id                    (ASC: lexicographical total tie-breaker)
```

### 3. Detailed Component Formulations

#### Component 1: `severity_gate` (`SeverityGate`)
- **Code Reference**: `src/graph.rs:65-69`, `src/graph_store.rs:2005-2009`
- **Formula**:
  $$\text{severity\_gate} = \begin{cases} \text{Catastrophic} & \text{if } \text{severity} \ge 4 \land \text{goal\_type} = \text{"committed"} \\ \text{Normal} & \text{otherwise} \end{cases}$$
- **Semantics**: Non-linear lexicographic override. Existential obligations bypass standard scalar competition and sort ahead of all normal work regardless of deadline or cost of delay.

#### Component 2: `deadline_band` (`DeadlineBand`)
- **Code Reference**: `src/graph.rs:72-79`, `src/graph_store.rs:1799-1921`
- **Formula**:
  Let $\text{days\_until} = (\text{due} - \text{today}).\text{num\_days}()$ and $\text{effort\_days} = \text{parse\_effort\_days}(\text{effort}).\text{unwrap\_or}(3)$:
  - If $\text{days\_until} < 0$: Initial band is $\mathbf{Overdue}$.
  - Else ($\text{days\_until} \ge 0$), let $\text{ratio} = \frac{\text{effort\_days}}{\max(\text{days\_until}, 1)}$:
    - $\text{ratio} \ge 1.0 \implies \mathbf{Imminent}$
    - $\text{ratio} > 0.5 \implies \mathbf{Urgent}$
    - $\text{days\_until} \le 30 \implies \mathbf{Approaching}$
    - $\text{otherwise} \implies \mathbf{None}$
- **Courtesy-Review Decay Gate** (`src/graph_store.rs:1871-1901`):
  If an overdue task carries **no real stakes** ($\text{downstream\_weight} = 0.0 \land \text{stakeholder is None} \land \neg\text{is\_human\_gate}() \land \text{urgency} \le 50.0 \land \text{intent} \ge 2$) and $\text{days\_overdue} > 20$:
  $$\text{decay\_days} = \min(\text{days\_overdue} - 20, 100), \quad \text{decay\_frac} = \frac{\text{decay\_days}}{100.0}$$
  The band decays smoothly over 100 days:
  $<0.25 \to \text{Overdue}, <0.50 \to \text{Imminent}, <0.75 \to \text{Urgent}, <1.00 \to \text{Approaching}, \ge 1.00 \to \text{None}$.
  Expired courtesy reviews eventually rank as normal undated work.

#### Component 3: `cost_of_delay`
- **Code Reference**: `src/graph_store.rs:1788-1977`
- **Formula**:
  $$\text{cost\_of\_delay} = \text{intent\_pressure} + \text{deadline\_points} + \text{stakeholder\_waiting} + \text{urgency\_term} + \text{voi\_term} + \text{value\_lineage\_term}$$
  1. **`intent_pressure`** (`src/graph_store.rs:1793-1797`):
     $\text{intent } 0 \text{ (P0)} \implies 10{,}000$; $\text{intent } 1 \text{ (P1)} \implies 5{,}000$; $\text{intent } \ge 2 \text{ or unset} \implies 0$.
  2. **`deadline_points`** (`src/graph_store.rs:1803-1921`):
     - Overdue ($\text{days\_until} < 0$): $8{,}000 + \min((-\text{days\_until}) \times 200, 4{,}000) \in [8{,}000, 12{,}000]$. If courtesy decay applies: $\text{points} \gets \text{points} \times (1 - \text{decay\_frac})$.
     - Future ($\text{days\_until} \ge 0$): $\text{ratio} \ge 1.0 \implies 6{,}000$; $\text{ratio} > 0.5 \implies 2{,}000 + \lfloor(\text{ratio}-0.5)\times 8{,}000\rfloor$; $\text{days} \le 30 \implies \lfloor\text{ratio}\times 4{,}000\rfloor$; else $0$.
     - Side-effect: sets $\text{deadline\_ramp\_fired} = \text{deadline\_points} > 0$.
  3. **`stakeholder_waiting`** (`src/graph_store.rs:1925-1958`):
     Applies if $\text{node.stakeholder.is\_some}() \lor \text{node.is\_human\_gate}()$:
     - If $\text{deadline\_ramp\_fired}$ is true: flat $2{,}000$ (suppresses daily lateness growth to prevent double-counting external lateness).
     - Else: $2{,}000 + \min(\text{days\_waiting} \times 200, 6{,}000) \in [2{,}000, 8{,}000]$, anchored to `waiting_since` or `created`.
  4. **`urgency_term`** (`src/graph_store.rs:1960, 3074-3222`):
     $\text{round}(\text{node.urgency}) \in [0, 10{,}000+]$.
     Propagates target severity backward along incoming dependency paths via relaxation:
     $$\text{urgency}(x) = S_{\text{lex}}(x) \times f(\text{slack}(x))$$
     where $S_{\text{lex}} = 10{,}000$ for committed SEV4, else $10^{\min(\text{sev}, 3)}$ ($1, 10, 100, 1000$).
     Edge propagation factors: `blocks` ($1.0$), `soft_blocks` ($0.3$), `children` ($0.5$), `contributes_to` (verbal weight: certain $1.00$, probable $0.85$, expected $0.75$, fifty-fifty $0.50$, uncertain $0.25$, improbable $0.15$, impossible $0.00$).
     Piecewise slack function ($k = \ln(10)/30$):
     $$f(\text{slack}) = \begin{cases} 10.0 & \text{if } \text{slack} \le 0 \\ e^{k(30 - \text{slack})} & \text{if } 0 < \text{slack} \le 30 \\ 0.001 & \text{if } \text{slack} > 30 \end{cases}$$
  5. **`voi_term`** (`src/graph_store.rs:1961, 3144-3215`):
     $\text{round}(\text{node.voi\_value}) \in [0, 5{,}000]$. Value of Information bonus.
     Strictly gated to leaf nodes ($\text{node.leaf} = \text{node.children.is\_empty}()$). Requires two conjunctive gates:
     - Open question: task itself has open inquiry / $\text{confidence} < 1.0$, OR its unblocking cone contains an open question.
     - Downstream divergence: unblocking cone contains $\ge 2$ distinct reachable nodes or direct blocks.
  6. **`value_lineage_term`** (`src/graph_store.rs:1967, 3224-3260`):
     $\text{round}(\text{node.value\_lineage}) \in [0, 10{,}000+]$.
     Standing weight elicited on committed targets flowing directly to contributors:
     $$\text{value\_lineage}(x) = 10{,}000 \times \text{confidence}(x) \times \sum_{ct \in \text{contributes\_to}} ct.\text{stated\_weight} \times ct.\text{target}.\text{standing\_weight}$$

#### Component 4: `tie_breakers` (`FocusTieBreakers`)
- **Code Reference**: `src/graph.rs:83-97`, `src/graph_store.rs:2027-2037`
- **Signals**:
  1. `downstream_weight_x10`: $\lfloor \text{node.downstream\_weight} \times 10.0 \rfloor$. Depth-decayed, edge-factor-discounted sum of weighted base scores over distinct nodes in downstream cone.
  2. `unlock_breadth_x10`: $\lfloor \text{node.unlock\_breadth} \times 10.0 \rfloor$. Sum of $\text{cost\_of\_delay}$ of direct dependents for which this task is the sole remaining blocker.
  3. `age_staleness`: If `intent >= 2`: $\min(\max(\text{days\_since\_created}, 0), 200)$, else $0$.
  4. `effective_intent`: Minimum intent in the downstream cone (P0 before P1...).
  5. `order`: Explicit manual sequence order integer.
  6. `id`: String lexicographical tie-breaker ensuring total, deterministic ordering.

### 4. Synthetic Display Score (`focus_score`)

- **Code Reference**: `src/graph.rs:138-148`, `src/graph_store.rs:2046`
- **Formula**:
  $$\text{focus\_score} = \text{gate\_pts} + \text{cost\_of\_delay} + \text{downstream\_weight\_x10} + \text{unlock\_breadth\_x10} + \text{age\_staleness}$$
  where $\text{gate\_pts} = 100{,}000$ if $\text{severity\_gate} = \text{Catastrophic}$ else $0$.
- **Critical Invariant**: **`focus_score` is a display scalar only, NOT the ranking signal.** It completely omits `deadline_band`, which sorts ahead of `cost_of_delay` in `FocusTuple::cmp`. Two tasks can order one way by `FocusTuple` and the opposite way by `focus_score`. Never sort, compare, or explain queue position by `focus_score` alone.

### 5. Derived Diagnostic Metrics Excluded from Ranking

Per architectural doctrine (`specs/ranking.md` §5):
- **`pagerank`**, **`betweenness`**, and **`criticality`** **NEVER enter any ranking or focus_score path**.
- They are unitless structural diagnostics computed strictly for graph gardening, visualization, and overwhelm telemetry (`top_n_by_metric`, `get_network_metrics`, overwhelm dashboard).

### Severity Ladder & Target Linking

For deadline-bound obligations that aren't tasks themselves (submissions, signings, contractual obligations), declare a **target node** and link contributing tasks to it:

```yaml
# The obligation
type: target
severity: 3                      # see severity ladder below
goal_type: committed             # committed | aspirational | learning
due: 2026-05-07
consequence: "Late review damages standing with the panel."

# A task contributing to it
contributes_to:
  - target: <target-id>
    stated_weight: Certain       # see weight scale below
    why: "contractual obligation as assigned assessor"
```

#### Severity ladder

| Level | Label | Example |
|---|---|---|
| 0 | Negligible | Minor annoyance; no consequence beyond self |
| 1 | Low | Small reputational or time cost |
| 2 | Moderate | Meaningful commitment; recoverable if missed |
| 3 | High | Serious consequence; hard to recover |
| **4** | **Catastrophic** | **Job loss, bankruptcy, severe health, legal action** |

SEV0–3 are compensatory. **SEV4 + `goal_type: committed` triggers the lexicographic `severity_gate`** so any SEV4-adjacent task outranks any non-SEV4 task regardless of priority or deadline.

#### Verbal Contribution-Weight Scale (Renooij-Witteman)

`contributes_to.stated_weight` accepts only verbal terms — raw decimals or unrecognized terms are rejected at parse time (`src/graph.rs:111-134`):

| Verbal Term | Numeric Anchor | Interpretation |
|---|---|---|
| `certain`, `almost certain` | **1.00** | Critical path / single point of failure |
| `probable`, `very probable`, `highly likely` | **0.85** | Strong primary contributor |
| `expected`, `likely` | **0.75** | Standard intended contributor |
| `fifty-fifty`, `even chance` | **0.50** | Moderate, genuinely uncertain contribution |
| `uncertain`, `possible`, `perhaps`, `maybe` | **0.25** | Exploratory or optional contribution |
| `improbable`, `unlikely`, `very unlikely` | **0.15** | Minor marginal contribution |
| `impossible`, `none` | **0.00** | No contribution |
| *(omitted / empty)* | **0.00** | Unstated edge; contributes zero weight |


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

### Excalidraw Tooling (`pkb-excalidraw`)

A high-performance CLI companion binary for inspecting, diffing, mutating, and validating Excalidraw whiteboard files (`.excalidraw` and `.excalidrawlib`):

| Command | Description |
|---------|-------------|
| `pkb-excalidraw <file> [summary\|map\|nodes\|edges]` | Token-cheap projections of whiteboard scenes and diagrams |
| `pkb-excalidraw <file1> struct-diff <file2>` | Pure semantic structural diff of nodes and edges without coordinate jitter |
| `pkb-excalidraw <file> add-node --text "T" --at X,Y` | Add container shape with bound, centered text |
| `pkb-excalidraw <file> connect --from A --to B [--label L]` | Create 2-bound directed arrow with optional label |
| `pkb-excalidraw <file> fit <id> "New Text"` | Symmetrical center-expanded text resize |
| `pkb-excalidraw <file> move-elem <id> [--by DX,DY]` | Translate node, bound text, and connected arrow endpoints |
| `pkb-excalidraw <file> delete-elem <id> [--cascade-arrows]` | Delete node and clean up bindings / cascaded arrows |
| `pkb-excalidraw <file> batch <changes.json \| ->` | Execute atomic transactional mutation batch |
| `pkb-excalidraw <file> check` | Verify structural integrity, index ordering, and bindings |
| `pkb-excalidraw <file> overlap` | Audit AABB box collisions and element overlaps |
| `pkb-excalidraw <file> arrows-check` | Audit 2D arrow-segment box intersections |
| `pkb-excalidraw <file> theme apply <theme>` | Apply standardized styling and semantic color roles |

For technical architecture and invariants, see the [Excalidraw Tooling Specification](file:///workspace/specs/excalidraw-tooling.md).  
For LLM coding agent patterns and copy-paste templates, see the [Excalidraw Agent Guide](file:///workspace/references/EXCALIDRAW_AGENT_GUIDE.md).

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
| **Graph** | `pkb_trace`, `pkb_orphans`, `graph_stats`, `graph_json`, `graph_excalidraw`, `export_graph` |
| **Batch** | `batch_update`, `batch_reparent`, `batch_archive`, `batch_merge`, `batch_create_epics`, `batch_reclassify`, `merge_node` |
| **System** | `get_stats` |

### `export_graph`: GraphViz DOT export

Read-only export of the knowledge/task graph as GraphViz DOT syntax (`digraph PKB { ... }`), for external rendering (`dot`, `neato`, `sfdp`) or topological analysis pipelines — a static, one-way counterpart to `graph_excalidraw`'s interactive round-trip canvas.

| Parameter | Type | Description |
|-----------|------|--------------|
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
