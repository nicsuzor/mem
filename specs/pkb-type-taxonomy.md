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

> **2026-06-03 — retire-goal reversed; goal & target are distinct out-of-tree types (Nic decision).** The unimplemented 2026-05-10 proposal to retire `goal` (alias `goal → target`) is reversed. `goal` and `target` are **distinct coexisting node types**, both **out of the work tree** (reference tier — never parents, never parented). `goal` is **not** an alias of `target`. The canonical three-tier model (Model B) below is authoritative.
>
> **`project` is no longer a node type.** "Project" is the narrow operational name for a polecat-registered repo, carried as the `project: <slug>` metadata field on tasks. See [[TAXONOMY]] §"Project (operational routing field)" and [[areas-not-projects]].
>
> This is **implemented in mem**: `project` is out of `VALID_NODE_TYPES`/`ACTIONABLE_TYPES` (new writes reject it; legacy `type: project` files read-coerce to `epic` and lint warns via `fm-deprecated-project-type`). `project:` frontmatter values are validated + canonicalized against the polecat.yaml registry (slug, `slug:` override, aliases, `project_aliases:`; builtins `task`/`adhoc-sessions`) at every write path, and tasks that omit the field inherit the nearest parent-chain ancestor's explicit value. See `specs/pkb-server-spec.md` §"Project registry (polecat.yaml)".
>
> The sections below are kept for historical context. Where they reference `project` as a tree role, treat those as superseded.

## Strategic Anchors and Work — Goals, Targets, Capabilities, and Work

The PKB separates **why / what / can / how**. `goal`, `target`, and `capability` are **strategic nodes beside the work tree** (reference tier): never parents, never in "to-do" surfaces, connected to work only by `contributes_to` (and consumer links). `epic`/`task`/`learn`/`pr` are the **work tree** and the only actionable tier.

- **`goal` — identity & purpose (why).** An identity-level commitment: *who I am / how I define myself*. **Unquantifiable** — you cannot count "achievement," and there is no meaningful consequence-of-missing an identity. So a goal has **no `severity`, no `consequence`, no `due`**. Roots of meaning (~10), e.g. *World-Class Academic Profile*. Out of the work tree: never a parent, never parented.
- **`target` — milestone & obligation (what).** A tangible, **countable, measurable** output/milestone or continuous operational baseline — *done / not done* or *met / unmet*. Carries the quantifiable stakes: **`severity` (SEV0–SEV4) + `consequence`** (+ optional `due`). The unit that propagates urgency and value lineage into the work tree, e.g. *Deliver LLB242 marks by deadline*. Out of the work tree: never a parent, never parented. Advances ≥1 goal via `contributes_to`.
- **`capability` — latent competence & methodological asset (can).** A durable, compoundable capacity, skill, or technical/cognitive method co-developed by human and agent (e.g. *capability to automatically model arguments from texts*, *capability to visualise argument structures*, *capability to assess logical gaps*). Unifies methods, skills, tools, and evaluators into a persistent, reusable asset. Out of the work tree: never a parent, never parented. Carries **`standing_weight`** (capital leverage), but **no `severity`, no `consequence`, no `due`** (deficits are opportunity costs, not operational failures). Advances targets and goals via `contributes_to`.
- **`epic` / `task` / `learn` — work (how).** Verbs. The only actionable tier (`ACTIONABLE_TYPES`) and the only nodes in the parent-child tree (`EPIC → EPIC|TASK → …`). Advances outcomes and capabilities via `contributes_to` to **targets**, **capabilities**, or directly to **goals**.

Linkage (out-of-tree, via `contributes_to`): `task/epic → capability/target → goal`. The `to:` of a `contributes_to` edge may be a **target, capability, or goal**. Linkage is metadata, not structure — never parent-child, never affects tree traversal; goals, targets, and capabilities are excluded from orphan detection (parentless is correct). **Severity lives only on targets** and propagates down `contributes_to` (Birnbaum); goals and capabilities carry no severity. `goal`, `target`, and `capability` are distinct coexisting types.

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
| `epic`  | Bundle of related work | None (root-level) or another epic                          |
| `task`  | Discrete deliverable   | Epic or task; root-level allowed for trivial standalones   |
| `learn` | Observational tracking | Epic or task                                               |
| `pr`    | PR tracking deliverable| Epic or task                                               |

**`target` and `goal` are not tree nodes.** Target nodes (type: target) join tasks to goals (type: goal) with an impact factor. They are excluded from the work-item tree: they have no children and never serve as a parent. Work links to targets via `contributes_to` metadata (formerly `goals: []`). Targets participate in priority/severity propagation but not in tree traversal, orphan detection, or task operations.

**Removed from actionable types:**

- `project` — no longer a node type; the word now refers to a polecat repo (operational routing field on tasks). See [[TAXONOMY]] §"Project (operational routing field)" and [[areas-not-projects]] for migration of existing `type: project` containers (most become root-level epics).

**Removed from type, moved to `classification`:** `bug`, `feature`, `action`, `subproject`, `milestone`.

- `bug` → `type: task, classification: bug`
- `feature` → `type: task, classification: feature`
- `action` → `type: task, classification: action`
- `subproject` → `type: epic` (sub-epics are just epics with an epic parent)
- `milestone` → `type: epic, classification: milestone` (a checkpoint grouping tasks)

**`learn` stays as its own type** because it has distinct graph behaviour: excluded from `ready_tasks()` (not actionable work, but tracked observational items).

#### Category 2: Reference (knowledge items)

These never appear in task operations. They are knowledge artifacts, not work to be done.

| Type        | Content                                                                          |
| ----------- | -------------------------------------------------------------------------------- |
| `target`    | Joins tasks to goals with an impact factor (linked via `contributes_to` on epics/tasks) |
| `capability`| Persistent capacity, methodological competence, or technical asset co-developed by human and agent |
| `goal`      | Strategic priorities/objectives                                                  |
| `note`      | General knowledge, observations, insights                                        |
| `memory`    | Agent/system memories                                               |
| `contact`   | People                                                              |
| `document`  | Generic documents                                                   |
| `reference` | External reference material                                         |
| `review`    | Review notes, reading notes                                         |
| `case`      | Case studies, legal cases                                           |
| `spec`      | Specifications                                                      |
| `knowledge` | Synthesised knowledge articles                                      |

### The `capability` node type

#### 1. Concept and Definition

A **capability** is a persistent, reusable cognitive, methodological, or technical competence/asset co-developed by human and agent (e.g. *automatically model arguments from texts*, *visualise argument structures*, *assess logical gaps*). It exposes a latent conceptual and methodological node in the PKB model that previously had no structural representation.

Where other tiers answer:
- **`goal`**: Why (identity-level commitment; unquantifiable; no severity or consequence).
- **`target`**: What (countable milestone or operational baseline; carries failure severity and consequence).
- **`epic` / `task`**: How (discrete, actionable work items that complete).

**`capability`** answers: **Can** — what methods, techniques, workflows, and automated competencies exist, what state of maturity they have reached, and how work compounds them over time.

#### 2. Architectural Decision: First-Class Strategic Node Type vs. Target vs. Reference

**Decision**: `capability` is defined as a **dedicated first-class node type** (`type: capability`) in the **Strategic Anchors** tier (coexisting with `goal` and `target`, outside the parent-child work tree).

##### Rationale for First-Class Node Type:

1. **Preservation of Orthogonal Strategic Axes**:
   - In `specs/ranking.md` and `pkb-strategic-axes-calibration`, the foundational principle is keeping intent, stakes, likelihood, and capability orthogonal.
   - `target` is explicitly defined by its quantifiable failure stakes: `severity` (SEV0–SEV4) and `consequence`. If a target fails, something breaks (e.g., student marks missed, publication pipeline halts).
   - In contrast, a capability represents a positive asset or capacity. The absence or delay of a capability does not have a catastrophic operational failure mode (it incurs an opportunity cost, not a SEV4 emergency). Forcing capabilities into `type: target` would force authors to either invent fabricated severity numbers (corrupting `urgency` propagation across the entire graph) or allow `severity: null` targets, eroding the contract that targets carry severity.
2. **Dual-Directional Graph Semantics**:
   - A target only has *inbound contributors* (`task -> contributes_to -> target`). Tasks never depend on or require a target as an operational prerequisite.
   - A capability has an intrinsic **dual relationship** to work:
     - **Upstream Development**: Tasks and epics *build, refine, or evaluate* the capability (`task -> contributes_to -> capability`).
     - **Downstream Enablement**: Tasks, research projects, and workflows *require, consume, or leverage* the capability (`task -> requires_capability -> capability` or prerequisite dependency).
3. **Maturity Lifecycle vs. Completion**:
   - Tasks and epics complete (`status: done`). Targets are achieved or sustained (`status: ready/done`).
   - Capabilities are never "done". They progress through maturity states (`nascent`, `developing`, `mature`, `deprecated`) and map to tangible backing artifacts (agent skills, CLI tools, evaluation benchmarks, code modules).

##### Rejected Alternatives:

1. **Rejected Alternative A: Capability as a Subtype or Tag on `target` (`type: target` with `target_type: capability` or tag `capability`)**:
   - *Why rejected*: Severely compromises the definition of `target`. Targets exist to bind quantifiable consequences and deadlines to work. Merging capabilities into targets creates semantic ambiguity, encourages fake severity assignments, and obscures whether an entity is an operational obligation or a technical asset.
2. **Rejected Alternative B: Capability as Reference/Knowledge Node (`type: knowledge` or `type: note`)**:
   - *Why rejected*: Knowledge nodes are passive documentation. They cannot legitimately anchor `contributes_to` edges from actionable work, cannot be priced with `standing_weight` under Nic's elicitation framework, and cannot flow `value_lineage` to prioritize tasks that build them. Treating capabilities as merely notes ignores that they are strategic investments.
3. **Rejected Alternative C: Capability as an Actionable Container (`type: epic` with `classification: capability`)**:
   - *Why rejected*: Epics belong to the actionable work tree (`EPIC -> TASK`). They represent bounded projects intended to close. Capabilities are enduring competencies that outlive individual projects and must not serve as structural containers for tasks.

#### 3. Linkage Specification

Capabilities sit outside the work tree (reference/strategic tier) and link via defined edge channels:

1. **Building & Enhancing (`contributes_to`)**:
   - Actionable work (`task`, `epic`, `learn`) that builds or improves a capability links via `contributes_to`:
     ```yaml
     contributes_to:
       - to: cap_argument_modeling
         stated_weight: expected
         justification: "Implements core AST parser for claim extraction from legal texts."
     ```
   - Uses the standard Renooij-Witteman verbal weight scale (`certain`, `probable`, `expected`, `fifty-fifty`, `uncertain`, `improbable`, `impossible`).
2. **Strategic Grounding (`contributes_to` upward)**:
   - A capability declares which higher-level strategic `target` or `goal` nodes it advances:
     ```yaml
     contributes_to:
       - to: targ-1e7d4733 # Scholarly publication target
         stated_weight: probable
         justification: "Automated argument modeling drastically reduces synthesis cycle time for empirical legal research."
     ```
3. **Enabling & Prerequisite (`requires_capability`)**:
   - Tasks or research projects that depend on the existence or maturity of a capability declare it via the frontmatter field `requires_capability: [<capability-id>]` or via body wikilinks (`[[cap_...]]`).
   - In graph terms, this enables agents and planning tools (`/ida:brief`, `/ida:decompose`) to detect capability gaps before scheduling complex empirical work.
4. **Out-of-Tree Invariant**:
   - Capabilities are **never parents** of actionable tasks (`reject_target_as_parent` logic generalized to all strategic nodes). Tasks must be parented by an `epic` or `task`, pointing to capabilities via `contributes_to` or `requires_capability`.

#### 4. Frontmatter Schema for `type: capability`

```yaml
---
id: cap_argument_modeling
title: "Capability: Automated argument modeling from texts"
type: capability
status: developing       # nascent | developing | mature | deprecated
standing_weight: 0.40   # Elicited from Nic (0.00 to 1.00); capital leverage
contributes_to:
  - to: targ-1e7d4733
    stated_weight: probable
    justification: "Enables high-throughput empirical analysis of platform terms and legal texts."
backing_assets:
  - skill: analyst
  - repo: nicsuzor/mem
  - tool: pkb_trace
tags:
  - capability
  - argument-mining
  - nlp
---
```

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

Optional frontmatter field on **epic**, **task**, and **learn** nodes (and upward on **capability** nodes). Each entry is an **edge object** (not a bare ID) declaring a weighted, justified belief that this work contributes to a target, capability, or goal. Canonical schema in [[multi-parent]] §1.6.

```yaml
---
type: epic
contributes_to:
  - to: target-abc123
    stated_weight: Expected
    justification: "contractual obligation to mark by 28 Apr"

  # Prototype-backed variant (recurring obligations):
  - to: prototype-osb-vote
    stated_weight: Certain
    justification: "OSB voting obligation"
    inherits_from: prototype-osb-vote

  # Capability contribution variant:
  - to: cap_argument_modeling
    stated_weight: Probable
    justification: "Implements argument extraction pipeline enabling legal text analysis"
---
```

**Canonical fields**: `to` (target, capability, or goal node ID), `stated_weight` (verbal term), `justification` (ICD 203 single sentence). The shorter aliases `weight` and `why` are accepted on read for backward compatibility (serde aliases as of mem PR #265).

**Weight scale (Renooij-Witteman, verbal only — raw decimals rejected at parse):**

| Term | Anchor | Meaning |
|------|--------|---------|
| Impossible | 0.00 | This task cannot affect the target |
| Improbable | 0.15 | Unlikely to be load-bearing |
| Uncertain | 0.25 | Might matter |
| Fifty-Fifty | 0.50 | Redundancy exists |
| Expected | 0.75 | Likely to matter |
| Probable | 0.85 | Strong contribution |
| Certain | 1.00 | Single point of failure |

**Weight semantics**: Birnbaum importance — the marginal probability that missing this task guarantees failure of the target. **Not** "percent contribution".

**Belief, not fact**: every edge is dated and re-evaluable. History lives in a side-log, not on the edge itself.

**Properties:**

- Valid on any actionable node (`epic`, `task`, `learn`) and strategic `capability` node
- Many-to-many: a task can contribute to multiple targets or capabilities; a target or capability can be served by many tasks
- Strategic linkage is **metadata, not structure** — it does not affect parent-child relationships, tree traversal, or orphan detection
- Consumed by `compute_urgency`, `compute_value_lineage`, and `focus_score` (see [[ranking]] §4.11)
- Legacy `goals: []` fields migrate to `contributes_to` with default `stated_weight: Expected` and a placeholder justification pending review

**Tree hierarchy (strict parent-child):**

```
EPIC → EPIC | TASK → …
```

Top-level work nodes are root-level epics (or root-level tasks for trivial standalones). Epics nest into other epics for sub-decomposition. Tasks may parent further tasks/epics where useful (most tasks are leaves).

**Strategic linkage (many-to-many, via metadata):**

```
Epics/tasks link to targets and capabilities via contributes_to: [{to: id, stated_weight: ...}] frontmatter field
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
