---
id: pkb-type-taxonomy
title: "PKB Type Taxonomy: Unified Node Classification"
type: spec
status: inbox
created: 2026-03-11
updated: 2026-06-03
superseded_partial:
  - "project as actionable type (decision 2026-05-10: project = polecat repo, not a node type)"
tags:
  - pkb
  - type-system
  - graph
  - architecture
---

# PKB Type Taxonomy: Unified Node Classification

> **2026-10-01 — Simplification of Node Types & Edge Weights (mem_5c476567).**
> 1. **Capability wound back**: PR #637 is closed unmerged and the proposed `capability` node type is completely removed from spec and code.
> 2. **`goal`, `target`, and `capability` collapsed into `target`**: Strategic out-of-tree destinations are unified under `target`. Legacy `type: goal` and `type: capability` read-coerce in-memory to `target`.
> 3. **`epic` collapsed into `task`**: Actionable work containers are unified under `task`. Legacy `type: epic` and `type: project` read-coerce in-memory to `task`.
> 4. **Differing fields accepted empty**:
>    - `target` accepts empty `severity`, `consequence`, and `due` (subsuming qualitative/aspirational goals alongside quantifiable targets).
>    - `task` accepts empty `parent` (allowing top-level/root containers without requiring an artificial epic wrapper).
> 5. **`contributes_to` float multiplier $x$**: Edges support an optional `multiplier` (alias `x`), scaling verbal or raw float weights ($x \times \text{weight}$) into urgency, value lineage, and downstream weight.
>
> **`project` is no longer a node type.** "Project" is the narrow operational name for a polecat-registered repo, carried as the `project: <slug>` metadata field on tasks. See [[TAXONOMY]] §"Project (operational routing field)" and [[areas-not-projects]].

## Targets and Work — the Simplified Two-Tier Taxonomy

The PKB simplifies the node taxonomy into two structural categories:

- **`target` — strategic destination (why / what).** An out-of-tree destination (never a parent, never parented, never in "to-do" queues) subsuming former goals, targets, and capabilities. A target may be qualitative/identity-level (empty `severity`, `consequence`, and `due`) or measurable/time-bound (with `severity` SEV0–SEV4, `consequence`, and `due`). Standing value is priced via `standing_weight` (float 0.0..=1.0). Work connects to targets via `contributes_to`.
- **`task` / `learn` / `pr` — actionable work (how).** Actionable work items. `task` subsumes all discrete deliverables and container epics: a task with no `parent` acts as a top-level root; a task with children acts as a container; a task without children acts as an executable leaf. `learn` tracks hypotheses/observations (excluded from `ready`), and `pr` tracks code changes.

Linkage (out-of-tree, via `contributes_to`): `task → target`. Linkage is metadata, not tree structure. Target weight propagates through `contributes_to` edges scaled by the edge's stated weight and float multiplier.

## Problem

The PKB type system has diverged across four layers, creating invisible work items, inconsistent filtering, and semantic confusion.

### Current state: four definitions, no agreement

| Layer              | Location             | Types treated as "actionable"                                                 |
| ------------------ | -------------------- | ----------------------------------------------------------------------------- |
| `VALID_NODE_TYPES` | `graph.rs:273`       | 24 types (validation only, no filtering)                                      |
| `ACTIONABLE_TYPES` | `graph_store.rs:83`  | task, bug, feature, project, goal, epic, learn, subproject                    |
| MCP `task_search`  | `mcp_server.rs:215`  | **task, project, goal** (hardcoded)                                           |
| MCP `list_tasks`   | `mcp_server.rs:1826` | Everything with an `id` field (`all_tasks()`)                                 |
| `is_treemap_type`  | `layout.rs:608`      | task, project, epic, goal, bug, action, subproject, feature, learn, milestone |
| Python `TaskType`  | `task_model.py:55`   | goal, project, epic, task, action, bug, feature, learn                        |

### Impact

**352 real work items are invisible to `task_search`:**

| Type      | Count | In `ACTIONABLE_TYPES` | In MCP `task_search` |
| --------- | ----- | --------------------- | -------------------- |
| `bug`     | 127   | yes                   | **no**               |
| `epic`    | 97    | yes                   | **no**               |
| `feature` | 52    | yes                   | **no**               |
| `action`  | 45    | yes                   | **no**               |
| `learn`   | 31    | yes                   | **no**               |

Meanwhile, `list_tasks` returns **everything with an `id` field** — including notes, contacts, and knowledge entries — because `all_tasks()` checks `task_id.is_some()` and `task_id` is populated from the `id` frontmatter field on every document.

### Root cause

The `type` field conflates two things:

1. **Graph role** — how the node participates in hierarchy, filtering, and task operations
2. **Content classification** — what the work item is about (bug vs feature vs action)

`bug`, `feature`, and `action` are classifications of work, not structural graph roles. A bug is a task. A feature is a task. An action is a task. They all behave identically in the graph — they have parents, statuses, dependencies, and appear in ready queues. The only type with genuinely different behaviour is `learn`, which is excluded from `ready_tasks()`.

## Design

### Principle: type encodes graph behaviour, not content

The `type` field answers: **"How does this node participate in the graph?"** — not "what is this about?" Content classification moves to a separate `classification` field and/or tags.

### Canonical type taxonomy

Three categories, exhaustive and mutually exclusive:

#### Category 1: Actionable (work items)

These appear in task operations (`list_tasks`, `task_search`, ready/blocked queues, task trees, treemap layouts).

| Type    | Graph role             | Parent requirement                                         |
| ------- | ---------------------- | ---------------------------------------------------------- |
| `task`  | Discrete deliverable or container | Optional (root-level allowed for containers/standalones; child of task) |
| `learn` | Observational tracking | Optional or child of task (excluded from `ready_tasks()`)   |
| `pr`    | PR tracking deliverable| Optional or child of task                                   |

**`epic` is collapsed into `task`.** All containers and groupings are represented as `task` nodes. A task without a `parent` acts as a root-level container or standalone task; a task with children acts as a container; a task with no children acts as an executable leaf.

**`target` is out of the work tree.** Target nodes (type: `target`) represent strategic destinations and milestones. They have no children and never serve as a parent in the work tree. Work links to targets via `contributes_to` metadata.

**Removed from actionable types:**
- `project` — no longer a node type; refers to a polecat repo routing slug.
- `epic` — collapsed into `task`.

**Removed from type, moved to `classification`:** `bug`, `feature`, `action`, `subproject`, `milestone`.

- `bug` → `type: task, classification: bug`
- `feature` → `type: task, classification: feature`
- `action` → `type: task, classification: action`
- `subproject` → `type: task`
- `milestone` → `type: task, classification: milestone`

**`learn` stays as its own type** because it has distinct graph behaviour: excluded from `ready_tasks()` (not actionable work, but tracked observational items).

#### Category 2: Reference (knowledge items)

These never appear in task operations. They are knowledge artifacts, not work to be done.

| Type        | Content                                                                          |
| ----------- | -------------------------------------------------------------------------------- |
| `target`    | Strategic destination / outcome milestone (linked via `contributes_to` on tasks) |
| `note`      | General knowledge, observations, insights                                        |
| `memory`    | Agent/system memories                                                            |
| `contact`   | People                                                                           |
| `document`  | Generic documents                                                                |
| `reference` | External reference material                                                      |
| `review`    | Review notes, reading notes                                                      |
| `case`      | Case studies, legal cases                                                        |
| `spec`      | Specifications                                                                   |
| `knowledge` | Synthesised knowledge articles                                                   |

**Collapsed strategic types:**
- `goal` → collapsed into `target` (accepts empty `severity`, `consequence`, `due`).
- `capability` → PR #637 wound back and closed unmerged; type completely removed and read-coerced to `target`.

**Alias resolution** (linter auto-fixes):

- `observation`, `insight`, `exploration` → `note`
- `article`, `reading-guide`, `talk` → `reference`
- `review-notes`, `peer-review` → `review`
- `instructions`, `role`, `agent`, `bundle` → `document`
- `audit` → `audit-report`
- `design` → `spec`

#### Category 3: Structural (infrastructure)

Navigation and logging infrastructure. Never in task operations.

| Type           | Content              |
| -------------- | -------------------- |
| `index`        | Map of Content files |
| `daily`        | Daily notes          |
| `session-log`  | Session transcripts  |
| `audit-report` | Audit output         |

### Comprehensive Audit of Types Differing Only by Filled Fields

Per task `mem_5c476567`, every node type in `VALID_NODE_TYPES` has been audited to determine whether types differing only by filled/unfilled fields should be collapsed or kept:

1. **`goal` and `capability` vs. `target` — Collapsed into `target`**:
   - *Previous state*: `goal` differed from `target` by lacking `severity`, `consequence`, and `due`. `capability` was proposed in PR #637 as another variant.
   - *Action*: Collapsed into `target`. PR #637 wound back and closed unmerged.
   - *Rule*: `target` nodes now permit `severity`, `consequence`, and `due` to be empty. An aspirational or identity-level target omits these fields; a quantifiable operational target includes them.
2. **`epic` vs. `task` — Collapsed into `task`**:
   - *Previous state*: `epic` differed from `task` primarily by being parentless (root container) or possessing children.
   - *Action*: Collapsed into `task`.
   - *Rule*: `task` nodes now permit empty `parent`. Container vs. leaf status is derived from children (`!children.is_empty()`) or tree depth, eliminating the need for a separate type.
3. **`learn` vs. `task` — Kept Distinct (Explicit Justification)**:
   - *Justification*: `learn` has distinct, load-bearing algorithmic behaviour in the graph engine: it is explicitly excluded from `ready_tasks()` (`CLAIMABLE_TYPES = ["task"]`). It represents observational inquiry rather than an executable deliverable.
4. **`pr` vs. `task` — Kept Distinct (Explicit Justification)**:
   - *Justification*: `pr` nodes represent external GitHub pull requests with dedicated lifecycle sync semantics (branch tracking, merge reconciliation, external review gates).
5. **Reference Types (`note`, `memory`, `contact`, `document`, `reference`, `review`, `case`, `spec`, `knowledge`) — Kept Distinct (Explicit Justification)**:
   - *Justification*: These types do not differ merely by empty optional fields. They represent distinct domain ontologies, semantic indexing schemas, and external entity bindings across the PKB.
6. **Structural Types (`index`, `daily`, `session-log`, `audit-report`) — Kept Distinct (Explicit Justification)**:
   - *Justification*: These represent infrastructure files with dedicated maintenance lifecycles and automated rollups (e.g. daily note loggers, audit tooling).

### The `classification` field

Optional frontmatter field for content classification of work items. Free-form string, but common values:

- `bug` — defect to fix
- `feature` — new functionality
- `action` — single work session
- `milestone` — checkpoint
- `spike` — time-boxed exploration
- `decision` — requires a choice
- `review` — review task (distinct from `type: review` which is review _notes_)

This field is for display and filtering only. It has no effect on graph behaviour.

### The `contributes_to` edge

Optional frontmatter field on **task** and **learn** nodes. Each entry is an **edge object** declaring a weighted belief and multiplier scaling that this work contributes to a target.

```yaml
---
type: task
contributes_to:
  - to: targ_abc123
    stated_weight: Expected
    multiplier: 0.5
    justification: "contractual obligation to mark by 28 Apr"

  # Float weight variant with alias x:
  - to: targ_xyz789
    weight: 0.8
    x: 1.5
    why: "direct technical dependency"
---
```

**Canonical fields**:
- `to` / `target`: Target node ID.
- `stated_weight` / `weight`: Verbal Renooij-Witteman term or raw float string (`0.0..=1.0`).
- `multiplier` / `x`: Optional float factor scaling the transmitted weight ($W_{\text{effective}} = x \times W_{\text{base}}$).
- `justification` / `why`: Rationale sentence.

**Weight scale (Renooij-Witteman verbal anchors or direct floats):**

| Term | Anchor | Meaning |
|------|--------|---------|
| Impossible | 0.00 | This task cannot affect the target |
| Improbable | 0.15 | Unlikely to be load-bearing |
| Uncertain | 0.25 | Might matter |
| Fifty-Fifty | 0.50 | Moderate contribution |
| Expected | 0.75 | Likely to matter |
| Probable | 0.85 | Strong contribution |
| Certain | 1.00 | Single point of failure |
| *Direct Float* | `0.0..=1.0` | Explicit numeric weight |

### Edge Weight: Float Multiplier vs. Separate Contribution-Quantum Term Analysis

A key question settled in this specification (mem_5c476567) is whether `contributes_to` edges require a separate "contribution-quantum" term alongside the float `multiplier`.

**Verdict: A separate contribution-quantum term is NOT needed.**

The decision is grounded in four concrete reasons:

1. **Mathematical Redundancy (Single Degree of Freedom)**:
   In DAG edge weight propagation, the transmitted importance from node $u$ to target $v$ is linear:
   $$W_{\text{trans}} = W_{\text{upstream}} \times T_{uv}$$
   If an edge introduces both a continuous multiplier $x$ and a quantum term $q$, the effective transfer coefficient becomes:
   $$T_{uv} = W_{\text{base}} \times x \times q$$
   Mathematically, $x \cdot q$ is a single scalar factor. Splitting a single multiplicative degree of freedom into two distinct scalar terms introduces parameter redundancy without increasing mathematical expressiveness.
2. **Cognitive Overhead and Elicitation Ambiguity**:
   Requiring human authors or autonomous agents to specify both a "multiplier" and a "quantum" forces subjective distinction between two concepts that perform identical arithmetic operations. Agents and humans cannot reliably distinguish when to adjust the quantum versus the multiplier.
3. **Orthogonality with Existing Node Properties**:
   Conceptually, "quantum" is sometimes invoked to represent discrete chunks of work or deliverable sizing. However, sizing is already explicitly captured by the node's `effort` field, while confidence in whether the contribution will be realized is captured by `confidence`. Adding a quantum term to the edge conflates edge importance with task sizing.
4. **Ergonomic Simplicity and Clean Propagation**:
   The float `multiplier` (alias `x`) cleanly scales the verbal anchor or raw float:
   $$W_{\text{effective}} = x \times W_{\text{base}}$$
   This propagates seamlessly across all three engine pipelines:
   - **Downstream Weight**: Accumulated via reverse BFS edge product: $w = \text{ct.numeric\_weight}()$.
   - **Urgency Propagation**: Neighbor edge factor: $\text{edge\_factor} = \text{ct.numeric\_weight}()$.
   - **Value Lineage**: Direct standing weight flow: $\text{lineage} = K_{\text{VL}} \times \text{confidence} \times \text{ct.numeric\_weight}() \times \text{standing\_weight}$.

A single float `multiplier` completely satisfies the requirements without introducing dead schema terms.

```
Epics/tasks link to targets via contributes_to: [id1, id2] frontmatter field
```

### Single source of truth: `ACTIONABLE_TYPES`

All layers must use the same constant for determining what is a work item:

```rust
pub const ACTIONABLE_TYPES: &[&str] = &[
    "epic", "task", "learn", "pr",
];
```

Every place that currently has its own hardcoded type filter must reference this constant:

| Location                                         | Current filter        | Change                                             |
| ------------------------------------------------ | --------------------- | -------------------------------------------------- |
| `mcp_server.rs:215` (`task_search`)              | `task\|project\|goal` | Use `ACTIONABLE_TYPES`                             |
| `mcp_server.rs` (`all_tasks()` via `list_tasks`) | `task_id.is_some()`   | Add `ACTIONABLE_TYPES` check                       |
| `layout.rs:608` (`is_treemap_type`)              | 10 hardcoded types    | Use `ACTIONABLE_TYPES`                             |
| `task_index.rs:234`                              | Inline `!= "learn"`   | Keep (behavioural exception within actionable set) |
| `task_model.py:55` (`TaskType`)                  | 8 values              | Reduce to 3: epic, task, learn                     |

### `all_tasks()` must filter by type

Currently:

```rust
pub fn all_tasks(&self) -> Vec<&GraphNode> {
    self.nodes.values()
        .filter(|n| n.task_id.is_some())  // Too broad — includes notes, contacts
        .collect();
```

After:

```rust
pub fn all_tasks(&self) -> Vec<&GraphNode> {
    self.nodes.values()
        .filter(|n| {
            n.task_id.is_some()
                && n.node_type.as_deref()
                    .map(|t| ACTIONABLE_TYPES.contains(&t))
                    .unwrap_or(false)  // Untyped nodes with task_id: exclude for safety; migrate via Phase 2
        })
        .collect();
```

## Migration

### Phase 1: Code changes (mem repo)

1. Update `ACTIONABLE_TYPES` to the 3-type list: `epic, task, learn`
2. Fix `task_search` to use `ACTIONABLE_TYPES.contains()` instead of hardcoded filter
3. Fix `all_tasks()` to filter by `ACTIONABLE_TYPES`
4. Fix `is_treemap_type()` to use `ACTIONABLE_TYPES`
5. Update Python `TaskType` enum to match
6. Add `classification` field to `GraphNode` struct (optional string, read from frontmatter)
7. Add `goals` field to `GraphNode` struct (optional `Vec<String>`, read from frontmatter)

### Phase 2: Data migration (PKB)

Reclassify existing non-canonical types to `type: task` + `classification`:

| Current            | Count | Migration                                                                                          |
| ------------------ | ----- | -------------------------------------------------------------------------------------------------- |
| `type: bug`        | 127   | → `type: task, classification: bug`                                                                |
| `type: feature`    | 52    | → `type: task, classification: feature`                                                            |
| `type: action`     | 45    | → `type: task, classification: action`                                                             |
| `type: subproject` | ~0    | → `type: epic` (sub-epic with epic parent)                                                         |
| `type: milestone`  | ~0    | → `type: epic, classification: milestone`                                                          |
| `type: project`    | ~30   | → `type: epic` (root-level by default; per-node review per [[areas-not-projects]] migration heuristic) |

This can be done via `pkb lint --fix` after updating the linter's type alias resolution.

### Phase 3: Linter enforcement

Add lint rule: if `type` is not in `VALID_NODE_TYPES` (the reduced canonical set), emit error.

Update `resolve_type_alias` to handle the retired actionable types:

```rust
fn resolve_type_alias(t: &str) -> (&'static str, Option<&'static str>) {
    // Returns (canonical_type, optional_classification)
    match t {
        "bug" => ("task", Some("bug")),
        "feature" => ("task", Some("feature")),
        "action" => ("task", Some("action")),
        "subproject" => ("epic", None),
        "milestone" => ("epic", Some("milestone")),
        "project" => ("epic", None),    // 2026-05-10: project no longer a node type
        "goal" => ("goal", None),
        "target" => ("target", None),
        // ... existing reference aliases unchanged
    }
}
```

## User Expectations

### Work Item Management

- **Unified Visibility**: Users expect `task_search` and `list_tasks` to return ALL work items, including bugs, features, and learning tracks, without needing to guess which specific type a work item was filed under.
- **Clean Task Lists**: Users expect task management tools to show only work to be done, never cluttering results with research notes, meeting transcripts, or contact information.
- **Hierarchical Clarity**: With Action/Bug/Feature absorbed into Task and Project removed from the tree, the canonical structure is `EPIC → EPIC|TASK → …`. Targets sit alongside the tree and connect via `contributes_to` metadata. `classification` (e.g., `action`) provides the granularity for session-sized work.

### Knowledge Organization

- **Canonical Consistency**: Users expect the system to automatically suggest or fix non-canonical types (e.g., `insight` -> `note`) to keep the knowledge base organized and searchable.
- **Clear Boundaries**: Users expect a sharp distinction between _reference_ material (knowledge artifacts) and _actionable_ material (work to be done), ensuring that a research note never accidentally appears as a blocked task.

### Implementation Status (Audit Assessment)

- **What Works**: Basic hierarchical task graph and searching for the core `task` and `epic` types. The foundational infrastructure for `ACTIONABLE_TYPES` exists in the Rust layer. (Pre-2026-05-10 the set included `project`.)
- **Missing**:
  - **Cross-Layer Sync**: Python `TaskType` and Rust `ACTIONABLE_TYPES` are out of sync; the Python side still maintains retired types as top-level enums.
  - **Visibility Gaps**: Many work items (`bug`, `feature`, `action`) are currently invisible to search or buried in noise because they aren't yet unified under the `ACTIONABLE_TYPES` constant in all search/list operations.
  - **Metadata Standardization**: The `classification` field is not yet universally parsed or displayed across the dashboard, TUI, and CLI.
- **Aspirational**: Full automated migration of existing data using `pkb lint --fix` and a unified single-source-of-truth for types across the entire Rust/Python stack.

## Acceptance criteria

1. `task_search("anything")` returns results with type `epic`, `task`, `learn` (and legacy `bug`, `feature`, `action` resolved via aliases) — not just the historical `task|project|goal` set
2. `list_tasks()` does NOT return notes, contacts, or knowledge entries
3. All five layers use the same `ACTIONABLE_TYPES` constant (no hardcoded filters)
4. Existing `type: bug` files still work correctly (either via migration or alias resolution at query time)
5. `ready_tasks()` still excludes `learn` type
6. TUI task tree and treemap show all actionable types
7. No regressions in existing tests

## Risks

- **Data migration blast radius**: 224 files changed (bug + feature + action). Mitigated by: linter `--fix` with dry-run, git diff review before commit.
- **Downstream consumers**: Dashboard, TUI, and CLI may filter on specific type strings. Mitigated by: Phase 1 code changes use the constant, not string literals.
- **Semantic loss**: If `type: bug` becomes `type: task`, agents lose the ability to filter by type alone. Mitigated by: `classification` field preserves the distinction; `list_tasks` could gain a `classification` filter parameter.

## Out of scope

- Reclassifying the 55 `knowledge` items (they may be correctly typed)
- Reclassifying the 52 `review` items (need human judgment: are they review tasks or review notes?)
- Adding `classification` as a filter parameter to MCP tools (nice-to-have, separate PR)
