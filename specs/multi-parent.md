---
id: multi-parent
title: "Multi-parent edges: target nodes, prototypes and contributes_to"
type: spec
created: 2026-05-04T06:32:47.344677231+00:00
modified: 2026-10-17T00:00:00.000000000+00:00
alias:
  - "multi-parent-multi-parent-edges-target-node-severity-propagation-and-focus-scoring"
  - "multi-parent"
permalink: multi-parent
status: ready
tags:
  - spec
  - pkb
  - graph
  - weights
---

# Multi-parent edges: target nodes, prototypes and contributes_to

The stored schema for target nodes, prototype nodes and the multi-parent `contributes_to` edge: what each field holds and means.

These fields are stored and parsed. The ranking is the flow engine ([`flow-rule.md`](flow-rule.md), [`pkb-flow-engine.md`](pkb-flow-engine.md)), which reads `links` and target `worth`, not the fields here (`pkb-flow-engine.md` §3.4). `contributes_to` entries are mapped into flow `serves` links until the data migration retires the key (`pkb-flow-engine.md` R7).

## Design philosophy

- **Dumb server, smart agent.** `mem` parses and stores these fields. Agents decide whether a task was necessary and whether prose is sufficient.
- **Surface, don't gate.** Anomalies and missing justifications surface in `/daily`, `/maintain`, `/sleep`. They do not block tool use.
- **MVP first.** Only fields and behaviours the pilot exercises are normative. Future extensions are tracked as separate tasks (see §References).

## Status

| Component | State | Reference |
|-----------|-------|-----------|
| `contributes_to` schema (read+write) | ✅ Implemented | mem PR #210 (2026-04-20) |
| Mapping of `contributes_to` into flow links | ✅ Implemented | `pkb-flow-engine.md` R7 |

## User stories

**US-1 — As a user with recurring class-like obligations** (peer review, OSB voting), I want to declare the obligation once as a *prototype* and have new instance tasks inherit severity/weight/consequence at creation, rather than re-typing them.

**US-2 — As a user reviewing an old plan,** I want to see *why* a task was thought to contribute to a target — a justification sentence captured at edge creation — so I can audit drift.

## Acceptance criteria

**AC-1 — Target nodes parse correctly.**
Given a node with `type: target`, `severity: 0-4`, `due:` (ISO-8601), `consequence:` (prose), and `goal_type: committed | aspirational | learning`, the graph builder MUST treat it as a target and exclude it from being executed as a task.

**AC-2 — `contributes_to` edges parse and resolve.**
Given a task with `contributes_to: [{to: <target-id>, weight: <verbal-term>, why: <prose>}]`, the edge MUST be visible in `get_task` output and in `pkb_context`, and MUST be mapped into a flow link (`pkb-flow-engine.md` R7). Invalid targets surface as parse warnings (not hard errors).

**AC-3 — Verbal weight terms only.**
`weight` (and its canonical alias `stated_weight`) on a `contributes_to` edge MUST be one of: `Impossible`, `Improbable`, `Uncertain`, `Fifty-Fifty`, `Expected`, `Probable`, `Certain`. Raw decimals MUST be rejected at parse time.

**AC-4 — Prototype inheritance is one-time copy.**
When a task is created with `contributes_to: [{to: <prototype-id>, inherits_from: <prototype-id>}]`, the prototype's `edge_template` fields MUST be copied into the materialised edge YAML at creation. Editing the prototype later MUST NOT retroactively rewrite existing edges.

**AC-5 — Mandatory consequence prose.**
Target nodes MUST have `consequence:` populated. Surface (don't block) missing prose via `/maintain`.

## 1. Schema

### 1.1 Target nodes

A **target node** represents a non-negotiable obligation or terminal consequence — the thing whose failure other tasks must prevent. Not a task to execute.

```yaml
type: target
severity: 0-4              # SRE-style ladder
due: <ISO-8601>            # absolute deadline
consequence: <prose>       # mandatory; forces deliberative articulation
goal_type: committed | aspirational | learning
```

### 1.2 Severity ladder (0–4)

| Level | Label | Example |
|-------|-------|---------|
| 0 | Negligible | Minor annoyance; no consequence beyond self |
| 1 | Low | Small reputational or time cost |
| 2 | Moderate | Meaningful commitment; recoverable if missed |
| 3 | High | Serious consequence; hard to recover |
| **4** | **Terminal** | **Job loss, bankruptcy, severe health, legal** |

`severity` is a stored field. The flow engine does not read it (`pkb-flow-engine.md` §3.4).

### 1.3 `goal_type`

Stored category of the target: `committed`, `aspirational` or `learning`. The flow engine does not read it (`pkb-flow-engine.md` §3.4).

For `aspirational` targets, `consequence:` is reused as **opportunity cost** prose (e.g., "miss publication window; finding becomes stale").

### 1.4 Consequence prose

Mandatory free-text field. Two functions:

1. **Cognitive speedbump.** Forces the user to articulate failure — suppresses reflexive severity inflation.
2. **Post-mortem evidence.** Compared against actual outcomes during calibration review.

No character cap. `/maintain` reviews adequacy.

### 1.5 Prototype nodes (standing obligations)

Targets are one-shot. Recurring class-like obligations (OSB voting, peer-review load) use a separate node type whose individual instances each get their own `due` but share severity/goal_type/consequence.

```yaml
type: prototype
edge_template:
  severity: 3
  goal_type: committed
  weight: Certain
  consequence: "<prose applicable to any instance>"
```

Semantics:

- Prototype has no `due` of its own. It is a class definition, not a target.
- Tasks linking via `contributes_to: [{to: <prototype-id>, inherits_from: <prototype-id>}]` get `edge_template` fields copied into edge YAML at creation.
- **Inheritance is one-time copy.** Editing the prototype does not retroactively rewrite existing edges. Past edges represent past beliefs. Re-stamping is explicit opt-in.
- `inherits_from:` on an edge is a provenance breadcrumb, not a live reference.
- Obsidian graph treats prototypes as first-class nodes.

Instance-level fields (`weight`, `why` on the edge itself) override prototype defaults. Prototype-inherited fields fill gaps.

### 1.6 `contributes_to` edge

Multi-parent-capable strategic edge, distinct from `parent` / `blocks`.

```yaml
contributes_to:
  - to: <target-id>
    stated_weight: Expected
    justification: "contractual obligation to mark by 28 Apr"

  # Prototype-backed variant:
  - to: <prototype-id>
    stated_weight: Certain
    justification: "contractual OSB voting obligation"
    inherits_from: <prototype-id>
```

**Canonical fields**: `stated_weight` and `justification`. The shorter aliases `weight` and `why` are accepted on read for backward compatibility (serde aliases as of mem PR #265). New edges should prefer canonical names.

Until the data migration retires the key, each entry is mapped into a flow link `{to, label: serves, quantum: min(1, numeric_weight(stated_weight)), set_by: migrated}` (`pkb-flow-engine.md` R7).

**Destination (`to:`) may be a `target` OR a `goal`.** Both are out-of-tree strategic nodes (reference tier — never parents, never parented). The typical chain is `task/epic → target → goal`, but work may also contribute directly to a goal (`to: <goal-id>`). **Severity lives only on `target` nodes.** **Goals carry no `severity` (and no `consequence`/`due`)** — a goal is an unquantifiable identity-level commitment. See [[pkb-type-taxonomy]] §"Goals, Targets, and Work — the three tiers" and [[TAXONOMY]].

### 1.7 Weight scale — Renooij-Witteman

Verbal terms only. Raw decimals rejected at parse.

| Term | Anchor |
|------|--------|
| Impossible | 0.00 |
| Improbable | 0.15 |
| Uncertain | 0.25 |
| Fifty-Fifty | 0.50 |
| Expected | 0.75 |
| Probable | 0.85 |
| Certain | 1.00 |

Non-linearity defeats spacing and centring biases that corrupt linear scales.

### 1.8 Weight semantics — Birnbaum importance

`weight` is **not** "percent contribution". It is the marginal probability that **missing this task guarantees failure of the target** — the Birnbaum importance from fault-tree analysis. `Certain` (1.00) means single point of failure. `Fifty-Fifty` (0.50) means redundancy exists.

The `justification` field follows intelligence tradecraft ICD 203. A single sentence is sufficient. Missing justifications surface in `/maintain`, not blocked at write.

### 1.9 Belief semantics

Every edge is a **belief**, not a fact: "I currently think task T contributes to O with weight W, as of this edit, because `why`." Drift, audit, fallibility and prototype inheritance all derive from treating edges as dated estimates.

History does **not** live on the edge itself. Edges stay as lightweight YAML list items on the source task.

Reified edges-as-nodes rejected: breaks Obsidian's markdown grain and noisies the graph view.

### 1.10 Pattern: deliverable-producing tasks wire to a class-level production target

When a task's deliverable is one instance of a recurring class of outputs (a release, a report, a dashboard, an external piece, etc.), wire it via `contributes_to` to a dedicated **class-level production target** for that deliverable type — not directly to a higher-level goal, to a project, or to a vague aggregate. The production target itself `contributes_to` upward toward broader goals.

Why: routing through a dedicated production target (a) preserves per-instance judgment on contribution × ship-risk; (b) prevents double-counting when one output touches multiple higher-level narratives; (c) models continuous production correctly — the target is durable; individual deliverables come and go.

Weighting: combine **potential contribution** (the axes that matter for this deliverable class — reach, impact, prestige, novelty, durability, whichever apply) with **ship risk** (current state, dependencies, stall indicators, credible path to completion) into a single `stated_weight` from the Renooij-Witteman scale (§1.7). The `why:` field names both axes explicitly in one sentence.

#### Worked example

```yaml
# In a deliverable-producing task's frontmatter:
contributes_to:
  - to: <class-target-id>
    stated_weight: Probable
    why: "<one line: contribution axis + ship-risk axis>"
```

What does NOT wire here: micro-tasks that produce parts of a single deliverable (they wire to their parent deliverable-task), review or critique tasks for others' deliverables, ad-hoc one-offs that don't belong to a recurring class, projects that *might* yield an instance someday but have no current concrete artefact.

## References

### Code

Cite by function name — line anchors drift.

- `ContributesTo`, `EdgeTemplate`, `numeric_weight` — `mem/src/graph.rs`

### Research briefs

- [[pkb-weight-98c1dd30]] — target nodes
- [[pkb-weight-2e455095]] — weight elicitation
- [[pkb-weight-9fe4d8f9]] — calibration & gaming resistance

### Pilot

- [[task-0779b81b]] — pilot epic ("Don't lose my job" + LLB242 marking)
- [[task-78835f17]] — ARC DECRA assessor target (live SEV3 instance)
- [[task-b9d6ff7e]] — OSB obligations prototype

### Deferred work

Open tasks tagged `multi-parent`. Use `task_search` with that tag for the live list. Categories:

- Calibration ritual + side-log
- Stated-vs-revealed divergence detector
- Transitivity audit (AHP-style)
