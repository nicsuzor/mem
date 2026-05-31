---
id: multi-parent
title: "Multi-parent edges, target-node severity propagation, and focus scoring"
type: spec
created: 2026-05-04T06:32:47.344677231+00:00
modified: 2026-05-07T00:00:00.000000000+00:00
alias:
  - "multi-parent-multi-parent-edges-target-node-severity-propagation-and-focus-scoring"
  - "multi-parent"
permalink: multi-parent
status: ready
tags:
  - spec
  - pkb
  - graph
  - scoring
  - weights
---

# Multi-parent edges, target-node severity propagation, and focus scoring

SSoT for the PKB ranking pipeline: how nodes and edges are shaped, how urgency propagates from terminal obligations to contributing tasks, and how a single composite `focus_score` drives the ready queue.

## Design philosophy

- **One signal.** Every ranking surface sorts by `focus_score`. Component fields (`urgency`, `downstream_weight`, `criticality`) are visible on metadata for filter/debug only — never the primary sort.
- **Dumb server, smart agent.** `mem` computes Brier scores, decay, LST slack, transitivity diffs as numbers. Agents decide whether a task was necessary, whether prose is sufficient, when to re-rank.
- **Surface, don't gate.** Anomalies, cap breaches, and missing justifications surface in `/daily`, `/maintain`, `/sleep`. They do not block tool use.
- **MVP first.** Only fields and behaviours the pilot exercises are normative. Future extensions are tracked as separate tasks (see §References).

## Status

| Component | State | Reference |
|-----------|-------|-----------|
| `contributes_to` schema (read+write) | ✅ Implemented | mem PR #210 (2026-04-20) |
| `compute_urgency` BFS | ✅ Implemented | `mem/src/graph_store.rs:1902` |
| `focus_score` composition (incl. urgency term) | ✅ Implemented | mem PR #303 (2026-05-04) |
| Doc surfaces (skills, specs, README) | ✅ Aligned | task-d997a904 (2026-05-04) |
| Calibration ritual (Brier, side-log) | 🟡 Deferred | See §References |
| Stated-vs-revealed divergence detector | 🟡 Deferred | See §References |
| Daily-note presentation overlay | 🟡 Deferred | See §References |

## User stories

**US-1 — As a user with a deadline-bound obligation,** I want tasks contributing to that obligation to surface above unrelated work as the deadline approaches, without manually adjusting their priority.

**US-2 — As a user with a Severity-4 (terminal) commitment,** I want any task contributing to it to outrank every non-SEV4 task in the ready queue, regardless of its own priority. Catastrophic-loss obligations are incommensurable with ordinary work.

**US-3 — As a user with recurring class-like obligations** (peer review, OSB voting), I want to declare the obligation once as a *prototype* and have new instance tasks inherit severity/weight/consequence at creation, rather than re-typing them.

**US-4 — As a user reviewing an old plan,** I want to see *why* a task was thought to contribute to a target — a justification sentence captured at edge creation — so I can audit drift.

**US-5 — As a user scanning the morning queue,** I want one number (`focus_score`) to sort by, not a dashboard of competing metrics. Component fields exist for diagnosis, not selection.

**US-6 — As an agent triaging tasks,** I want to read `focus_score` from `list_tasks` / `focus_picks` and trust it as the canonical ordering — without re-implementing ranking logic in skills.

## Acceptance criteria

**AC-1 — Target nodes parse correctly.**
Given a node with `type: target`, `severity: 0-4`, `due:` (ISO-8601), `consequence:` (prose), and `goal_type: committed | aspirational | learning`, the graph builder MUST treat it as a target and exclude it from being executed as a task.

**AC-2 — `contributes_to` edges parse and resolve.**
Given a task with `contributes_to: [{to: <target-id>, weight: <verbal-term>, why: <prose>}]`, the edge MUST be visible in `get_task` output, in `pkb_context`, and consumed by `compute_urgency`. Invalid targets surface as parse warnings (not hard errors).

**AC-3 — Lexicographic override fires only for SEV4-committed.**
A task contributing to a `severity: 4, goal_type: committed` target with `Slack ≤ 0` MUST have `urgency ≥ 10_000`. SEV0–3 targets and `aspirational`/`learning` SEV4 targets MUST NOT trigger the override.

**AC-4 — `urgency` composes additively into `focus_score`.**
`focus_score` MUST equal `priority_base + severity_bonus + deadline_score + age_staleness_bonus + downstream_weight × 10 + stakeholder_waiting_bonus + round(urgency) + round(voi_value)` (see §2.2). The `voi_value` term is 0 on any graph without `contributes_to` edges, so this reduces to the legacy form (AC-9).

**AC-5 — Single sort signal.**
`focus_picks`, `list_tasks` default sort, and the TUI Focus view MUST sort by `focus_score` descending. Filtering by component fields (e.g. `urgency_gte`) is permitted; sorting by them is not.

**AC-6 — Imminent deadlines surface regardless of status.**
The ready-queue candidate set MUST include tasks with `Slack ≤ safe_horizon` even if their status is `in_progress` or `blocked`, before applying any status-based filter.

**AC-7 — Verbal weight terms only.**
`weight` (and its canonical alias `stated_weight`) on a `contributes_to` edge MUST be one of: `Impossible`, `Improbable`, `Uncertain`, `Fifty-Fifty`, `Expected`, `Probable`, `Certain`. Raw decimals MUST be rejected at parse time.

**AC-8 — Prototype inheritance is one-time copy.**
When a task is created with `contributes_to: [{to: <prototype-id>, inherits_from: <prototype-id>}]`, the prototype's `edge_template` fields MUST be copied into the materialised edge YAML at creation. Editing the prototype later MUST NOT retroactively rewrite existing edges.

**AC-9 — Backwards compatibility.**
A graph with no targets, prototypes, or `contributes_to` edges MUST produce identical `focus_score` values as before this spec landed (urgency contribution = 0).

**AC-10 — Mandatory consequence prose.**
Target nodes MUST have `consequence:` populated. Surface (don't block) missing prose via `/maintain`.

**AC-11 — VoI is capped to preserve lexicographic dominance.**
`voi_term` (`round(node.voi_value)`) MUST be ≤ 5 000 for every node, enforced via a hard clamp (e.g., `min(5000, round(voi_value))`). `K_voi` MUST be calibrated to target this range, but the hard cap guarantees the value-of-information premium stays below the SEV4-committed urgency floor of 10 000, preserving the AC-3 lexicographic override: a VoI-heavy leaf task can never outrank a SEV4-committed obligation.

**AC-12 — VoI accrues only to leaf nodes.**
`voi_value` MUST be 0 for any node with `leaf = false`. Undecomposed epics and projects MUST NOT accumulate value-of-information credit; the term gates on `leaf` (TAXONOMY.md §Core Computed Properties) so only actionable, decomposed work earns it.

## 1. Schema

### 1.1 Target nodes

A **target node** represents a non-negotiable obligation or terminal consequence — the thing whose failure other tasks must prevent. Not a task to execute.

```yaml
type: target
severity: 0-4              # SRE-style ladder
due: <ISO-8601>            # absolute deadline; triggers slack calc
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

SEV0–3 are compensatory (standard scalar math). **SEV4 is lexicographic**: it gets `S_lex = 10_000`, dwarfing standard priority weights. Any SEV4-adjacent task outranks any SEV0–3 task regardless of other factors.

### 1.3 `goal_type`

Only `committed` targets receive the lexicographic override. `aspirational` and `learning` use linear propagation. This prevents moonshots from hijacking the focus queue.

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

Every edge is a **belief**, not a fact: "I currently think task T contributes to O with weight W, as of this edit, because `why`." This framing is load-bearing for calibration: drift, audit, fallibility, prototype inheritance, and the side-log all derive from treating edges as dated estimates.

History does **not** live on the edge itself. Edges stay as lightweight YAML list items on the source task. Belief-drift history (Brier scores, decay checkpoints) lives in a side-log, written only when the calibration ritual fires (deferred — see §References).

Reified edges-as-nodes rejected: breaks Obsidian's markdown grain, noisies the graph view, pays calibration cost before the ritual earns its keep.

### 1.10 Pattern: deliverable-producing tasks wire to a class-level production target

When a task's deliverable is one instance of a recurring class of outputs (a release, a report, a dashboard, an external piece, etc.), wire it via `contributes_to` to a dedicated **class-level production target** for that deliverable type — not directly to a higher-level goal, to a project, or to a vague aggregate. The production target itself `contributes_to` upward toward broader goals.

Why: routing through a dedicated production target (a) preserves per-instance judgment on contribution × ship-risk; (b) prevents double-counting when one output touches multiple higher-level narratives; (c) lets the target's severity propagate cleanly back to instance-tasks via focus_score; (d) models continuous production correctly — the target is durable; individual deliverables come and go.

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

## 2. Formula

### 2.1 Urgency propagation

Implemented as `compute_urgency` (`mem/src/graph_store.rs:1902`).

```
urgency_contribution(task → target) = S_lex(target.severity, target.goal_type)
                                     × W_edge(contributes_to.weight)
                                     × f(Slack(target))
```

**`S_lex(s, g)`** — step function:
- `s == 4 && g == "committed"` → `10_000`
- else → scalar from priority→weight table:
  - SEV0=P3=1, SEV1=P2=2, SEV2=P1=3, SEV3=P0=5, SEV4=P0=5 (linear when not committed)

**`W_edge`** — numeric anchor from the verbal term (§1.7).

**`Slack`** = `due - now - e'` where `e'` is estimated execution time (sum of descendant task scope × uncertainty). Least Slack Time, not Earliest Deadline First — uses execution estimates to prevent starvation of large critical tasks. `e'` MUST be pre-computed and cached per target to avoid O(N²) traversal cost.

**`f(Slack)`** — piecewise-exponential:
- `Slack > safe_horizon` → `ε` (negligible; 0.001)
- `0 < Slack ≤ safe_horizon` → `e^(k × (safe_horizon - Slack))` where `k = ln(10) / safe_horizon`
- `Slack ≤ 0` → `1.0` (full unlock)

The function pre-computes `S_lex` and own slack per node, then BFS-propagates `S_lex × edge_weight` from contributing tasks toward targets. Result written to `node.urgency: f64`.

### 2.2 Focus score composition

Implemented as `compute_focus_scores` (`mem/src/graph_store.rs:914`).

```
focus_score =
    priority_base
  + severity_bonus
  + deadline_score (× consequence multiplier if set)
  + age_staleness_bonus (P2+ only)
  + downstream_weight × 10
  + stakeholder_waiting_bonus
  + urgency_term
  + voi_term
```

| Component | Range | Notes |
|-----------|-------|-------|
| `priority_base` | 0 / 5 000 / 10 000 | P0 = 10 000, P1 = 5 000, P2+ = 0 |
| `severity_bonus` | 0 / 5 000 / 10 000 / 20 000 / 100 000 | SEV0–4. Lexicographic at SEV4. |
| `deadline_score` | 0 – 12 000 | Overdue: 8 000 + min(days × 200, 4 000). Tight (effort ≥ days_until): 6 000. Near-tight: linear interp 2 000–6 000. ≤30 days: 1 000. `× 1.5` if `consequence` set. |
| `age_staleness_bonus` | 0 – 200 | min(days_since_created, 200), P2+ only. Prevents old low-priority items being buried. |
| `downstream_weight × 10` | 0 – ∞ | downstream_weight float × 10 to land in same magnitude as base scores. |
| `stakeholder_waiting_bonus` | 0 / 2 000 – 8 000 | When `stakeholder` set: 2 000 + min(days × 200, 6 000). Anchor: `waiting_since` ?? `created`. |
| `urgency_term` | 0 – 10 000+ | `round(node.urgency)`. SEV4-committed contribution drives this to 10 000+. |
| `voi_term` | 0 – 5 000 | `round(node.voi_value)`. Value-of-information premium for uncertainty-resolving leaf tasks. Capped at 5 000 to stay below the SEV4-committed urgency floor (AC-11); zero for non-leaf nodes (AC-12). |

`urgency_term` composes additively because `S_lex × W_edge × f(Slack)` produces values in the same integer-magnitude scale as the other terms (~0.001 to 10 000+). The lexicographic-override property survives: a SEV4-committed contribution with `Slack ≤ 0` pushes `urgency_term` past every non-SEV4 task's combined score, mirroring `severity_bonus = 100 000` for the target itself.

`voi_term` adds a **value-of-information** premium so that uncertainty-resolving work (spikes, probes, prototypes) is not starved by a purely exploitative signal — a leaf task whose own exploitation utility is low but which would resolve large downstream uncertainty earns ranking credit it would otherwise be denied. It is `round(node.voi_value)`, where:

```
voi_value = K_voi · leaf · dep_resolution_ratio
          · Σ_{d ∈ immediate_downstream(x)} uncertainty(d) × edge_weight(x→d) × downstream_weight(d)
          / max(effort_days, 0.5)
```

- `leaf` and `uncertainty(d)` are the computed properties defined in TAXONOMY.md §Core Computed Properties. `leaf` gates the term to actionable nodes — a node with `leaf = false` scores `voi_value = 0` (AC-12), so undecomposed epics never accumulate VoI.
- `dep_resolution_ratio(x)` gates credit to ready-now work (fraction of `depends_on` edges resolved).
- `edge_weight(x→d)` is the Birnbaum importance on the `contributes_to` edge (Renooij-Witteman scale, §1.7–1.8): the conditional likelihood that not doing `x` fails downstream node `d`.
- `downstream_weight(d)` is the existing structural-importance field (TAXONOMY.md §criticality).
- `/ max(effort_days, 0.5)` is the cost normalization (information-gap-ratio form, kb-9230ba76 §5): it prevents a long probe from outranking a short one with the same downstream uncertainty.
- `K_voi` is a calibration constant (config field) chosen so `voi_value` typically remains ≤ 5 000, with a hard clamp at 5 000 (AC-11) to guarantee it stays below the SEV4-committed urgency floor of 10 000, preserving the lexicographic override (AC-3).

The term reduces to 0 when no `contributes_to` edges exist, keeping backwards compatibility (AC-9). `voi_value` is surfaced on metadata for filter/debug only; ranking is always via `focus_score`. Canonical definitions of the inputs `uncertainty` and `downstream_weight` live in TAXONOMY.md §Core Computed Properties.

### 2.3 Compute order

`compute_urgency` MUST run before `compute_focus_scores` during graph build. Verified by current pipeline.

`compute_voi_term` MUST run **after** `compute_downstream_metrics` — it consumes `downstream_weight`, `leaf`, and `dep_resolution_ratio`, all produced by the downstream-metrics pass — and **before** `compute_focus_scores`, which sums `voi_term` into the composite. The Phase 1 dependency chain is therefore `compute_downstream_metrics` → `compute_voi_term` → `compute_focus_scores`; `compute_urgency` likewise runs before `compute_focus_scores`.

Scores are stored on `GraphNode.urgency`, `GraphNode.voi_value`, and `GraphNode.focus_score`, recomputed on every full rebuild (~300ms). No TTL cache needed at current scale.

## 3. Invariants

- **No targets, no propagation.** When no target nodes exist, urgency reduces to 0; `focus_score` reduces to its legacy 5-component form. Backwards compatible.
- **SEV4 trigger requires both severity and goal_type.** A task with `contributes_to` but no SEV4-committed target never triggers the lexicographic override.
- **Completed targets are excluded.** BFS skips targets in `COMPLETED_STATUSES`.
- **Status independence for imminent deadlines.** Tasks with `Slack ≤ safe_horizon` are included in candidate set regardless of status, before status-based filters apply.

## 4. Consumers

| Consumer | Reads | Notes |
|----------|-------|-------|
| `focus_picks(max)` | `focus_score` desc | Top-N ready tasks |
| `list_tasks` default sort | `focus_score` desc | Ready-queue ranking |
| TUI Focus view | `focus_score` desc | Ordered display |
| `/pull` skill | `focus_score` desc | Worker task selection |
| `/daily` skill | `focus_score` desc | Morning briefing |
| Diagnostic filters | `urgency_gte`, etc. | Permitted; sorting still by focus_score |

MCP tool descriptions (`list_tasks`, `get_task`) describe `focus_score` as the composite signal including target propagation.

## Appendix A — Cancelled signals (audit trail)

The retired `task-focus-scoring` spec listed five "deferred" signals. After 2026-04-26, three are formally cancelled because the new edge-weight model in [[task-18da4781]] subsumes them:

| Signal | Disposition | Cancelled task |
|--------|-------------|----------------|
| `intention_alignment` | Subsumed — verbal-scale edges + revealed-preference signals replace explicit intentions | [[task-fe5fa11a]] |
| `project_activity` | Subsumed — edge decay + revealed preference handle activity | [[task-b9f60d18]] |
| `user_boost` | Subsumed — edge weight elicitation with justification + decay | [[task-4c210ad9]] |
| `recency_signal` | Building block survives — pure function exists ([[task-fa47db90]] merged); not currently called from accumulator | n/a |
| `blocking_urgency` | Building block survives — field computed ([[task-6b3d7f3b]] merged); reinterpreted under Birnbaum semantics | n/a |

## References

### Code

- `compute_focus_scores` — `mem/src/graph_store.rs:914`
- `compute_urgency` — `mem/src/graph_store.rs:1902`
- `downstream_weight` BFS — `mem/src/graph_store.rs:1347-1470`
- Ready-list sort — `mem/src/graph_store.rs:1663-1676`

### Research briefs

- [[pkb-weight-98c1dd30]] — target nodes
- [[pkb-weight-2e455095]] — weight elicitation
- [[pkb-weight-9fe4d8f9]] — calibration & gaming resistance

### Phase 1 synthesis

- [[kb-9230ba76]] — literature survey synthesizing expected-utility ranking with value-of-information (Bayesian decision theory + Knowledge Gradient, effectuation/affordable-loss, real options, SRCPSP information-gap ratio, severity-ladder reconciliation). Source for the `voi_term` formulation in §2.2.
- [[aops-9f5bfbe2]] — parent epic "Reconcile focus_score with planning-under-uncertainty". §Synthesis defines the Phase 1 additive VoI term and AC-11/AC-12; Phase 2 extensions (effectuation gate, real-options multiplier, severity utility curve) are tracked separately there.

### Pilot

- [[task-0779b81b]] — pilot epic ("Don't lose my job" + LLB242 marking)
- [[task-78835f17]] — ARC DECRA assessor target (live SEV3 instance)
- [[task-b9d6ff7e]] — OSB obligations prototype

### Deferred work

Open tasks tagged `multi-parent`. Use `task_search` with that tag for the live list. Categories:

- Calibration ritual + Brier scoring + side-log (was §4 of pre-2026-05-07 draft)
- Stated-vs-revealed divergence detector
- Transitivity audit (AHP-style)
- Severity ↔ priority isomorphism review
- `e'` (estimated execution time) source decision
- SEV4 concurrency cap surfacing in `/daily`
- Daily-note presentation overlay for urgency-propagated tasks
