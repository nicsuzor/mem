---
id: ranking
title: "PKB Ranking & Prioritisation Specification"
type: spec
status: approved
created: 2026-08-25
pinned_commit: 9336a166f6bf5c40987503ce56f98eadc59cf859
tags:
  - ranking
  - focus-score
  - scoring
  - graph-metrics
  - spec
  - pkb
---

# PKB Ranking & Prioritisation Specification

This document is the canonical specification for task prioritisation, focus scoring, graph centrality metrics, and queue ordering in `mem`. It describes the ranking machinery **as it actually ships at commit `9336a166f6bf5c40987503ce56f98eadc59cf859`**.

Per `.agents/CORE.md`, this specification documents approved current state only.

---

## 1. Overview of the Scoring Architecture & Sort Tuple

The PKB prioritisation model ranks every active, uncompleted task (`status ∉ {"done", "cancelled"}`) using an explicit, ordered sort tuple under a derived ordering:

```text
focus_tuple = (severity_gate, deadline_band, cost_of_delay, tie_breakers)
```

The sort tuple replaces the hand-built positional encoding of the earlier eight-term additive accumulator. It maintains the strict "one signal" invariant — the tuple **is** the single sort key across all ranking surfaces (`GraphStore::focus_cmp`, `list_tasks`, and the CLI). A synthetic display number — **`focus_score`** — is derived from the tuple for human-facing output and backwards-compatibility, but is demonstrably not a sort input.

Ranking in `mem` computes the sort tuple in `GraphStore::compute_focus_scores` (`src/graph_store.rs:1993`), executed near the end of the graph build pipeline (`GraphStore::build_internal`, `src/graph_store.rs:447-513`):

```text
canonical node sort (node id ASC)
  → compute_inverses
  → compute_degree_metrics
  → compute_centrality_metrics    (skipped on incremental rebuilds)
  → compute_downstream_metrics
  → compute_effective_intent      (gated blocker + ancestor-pressure channels; mem_intent_ready_weight)
  → compute_blocking_urgency
  → compute_urgency              (also populates chain_slack — §4.3a)
  → compute_scope
  → compute_uncertainty
  → compute_criticality
  → compute_voi_term
  → compute_value_lineage        (§4.11, Phase 2)
  → compute_unlock_breadth       (§4.10, Phase 2)
  → compute_focus_scores
  → compute_project_field
  → compute_target_ancestors
  → [similarity edges, if requested]
```

There is no `compute_effective_priority` stage or `own_priority`/`priority_base` field anywhere in the engine — see §2.1 and §4.6 below. The only surviving "priority" spelling is a legacy frontmatter alias: a document with `priority:` instead of `intent:` still loads (`src/graph.rs:1334`), and `is_valid_priority` is a thin wrapper over `is_valid_intent` (`src/graph.rs:795-796`); the corpus key migration is deferred as its own decision per `.agent/CORE.md`.

`compute_value_lineage` and `compute_unlock_breadth` are both pure, single-hop, per-node scans over fields already finalised earlier in the pipeline (`resolved_to`, `urgency`, `voi_value`) — neither is a new whole-graph BFS/cone-walk mechanism alongside the two that already exist (the downstream cone walk in `compute_downstream_metrics`/`compute_voi_term`, and the urgency-propagation relaxation in `compute_urgency`).

### 1.1. Canonical Node Ordering & Rebuild Determinism

To guarantee that graph rebuilds are strictly deterministic and immune to Rust's randomized `HashMap` iteration order or non-deterministic file scan order:
- **Canonical Sort Order**: In `GraphStore::build_internal`, the node collection is canonically sorted by node `id` (`nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id))`) at entry and immediately following ghost node discovery (where referenced IDs are also sorted).
- **Invariance Property**: Two builds or rebuilds from byte-identical input produce byte-identical derived scores (`downstream_weight`, `criticality`, `focus_score`, `voi_value`, `urgency`, `effective_intent`) across all build entry points (`build`, `build_from_directory`, `rebuild_from_nodes`, `rebuild_from_nodes_fast`, `rebuild_from_nodes_fast_with_embeddings`, `rebuild_from_nodes_skip_similarity`).
- **Precedence**: Follows established precedents in `build_resolution_map` (`src/graph_store.rs:3622-3635`) and `tarjan_scc` (`src/graph_store.rs:3691`).


---

## 2. `focus_tuple` and `focus_score` Components

Ranking is produced by the explicit 4-component tuple `FocusTuple`:

1. **`severity_gate`**: Non-linear catastrophic obligation override (`Catastrophic` for SEV4-committed, else `Normal`).
2. **`deadline_band`**: Discrete calendar float band (`Overdue` > `Imminent` > `Urgent` > `Approaching` > `None`).
3. **`cost_of_delay`**: Commensurable dynamic pressure combining `intent_pressure`, fine-grained `deadline_score`, `stakeholder_waiting`, `urgency_term`, `voi_term`, and (Phase 2) `value_lineage_term`.
4. **`tie_breakers`**: Deterministic tie-breaking signals (`downstream_weight × 10`, (Phase 2) `unlock_breadth × 10`, `age_staleness_bonus`, `effective_intent`, `order`, `id`).

```
+---------------------------------------------------------------------------------------------------+
| Term / Component           | Formula / Logic                                   | Shipped Range    |
+---------------------------------------------------------------------------------------------------+
| 1. intent_pressure         | match effective_intent { 0=>10000, 1=>5000, _=>0 }| 0 – 10,000       |
| 2. severity_bonus          | Replaced by severity_gate (no double-count)       | Catastrophic/Norm|
| 3. deadline_score          | Piecewise ramp on due date & effort ratio         | 0 – 12,000       |
| 4. age_staleness_bonus     | If pri >= 2: min(days_since_created, 200)         | 0 – 200          |
| 5. downstream_weight × 10  | (downstream_weight * 10.0) as i64                 | 0 – ~53 (obs)    |
| 6. stakeholder/human gate  | Base 2000 + lateness ramp (unless deadline fired) | 0 / 2,000 – 8,000|
| 7. urgency_term            | round(node.urgency)                               | 0 – 10,000+      |
| 8. voi_term                | round(node.voi_value) (leaf nodes only)           | 0 – 5,000        |
| 9. affordable_loss         | Non-compensatory filter zeroing unaffordable tasks| Bool filter      |
| 10. value_lineage_term     | round(node.value_lineage); see §4.11 (Phase 2)    | 0 – 10,000       |
| 11. unlock_breadth × 10    | (node.unlock_breadth * 10.0) as i64; tie-breaker, | 0 – ∞ (§4.10)    |
|                            | not cost_of_delay (Phase 2)                       |                  |
+---------------------------------------------------------------------------------------------------+
```

Term 10, `value_lineage_term`, is the fix for the failure terms 5/11 cannot be: `downstream_weight × 10` and `unlock_breadth × 10` are both deliberately capped, shallow, tie-breaker-only signals (§4.1, §4.10), so the sanctioned importance channel (`contributes_to`) needed a term that lives inside `cost_of_delay` itself, at comparable magnitude to `intent_pressure`/`urgency_term`, to be able to move a ranking at all (§3, §4.11).

### 2.1. Intent Pressure (`intent_pressure`)
- **Code reference**: `compute_cost_of_delay`, `src/graph_store.rs:1788-1801`.
- **Formula**:
  ```rust
  let intent_val = node.intent.unwrap_or(4);
  let effective_intent_val = node.effective_intent.unwrap_or(intent_val);
  let intent_pressure: i64 = match effective_intent_val {
      0 => 10000,
      1 => 5000,
      _ => 0,
  };
  ```
  Bands the node's **gated, propagated** `effective_intent` (§4.6), not the raw stated `intent` — this is what lets a high-intent parent's pressure move a ready descendant's `cost_of_delay`, not just its tie-break order. `node.priority` does not exist as a field; a document authored with `priority:` in frontmatter loads into the same `intent` value via a parser-level alias (`src/graph.rs:1334`).
- **Theoretical Range**: `0`, `5000`, or `10000`.
- **Observed Range**: `0` for >97% of tasks (P0 is rare/transient).
- **Default (absent input)**: `0` (intent unset defaults to P4/4, which bands to `0`).
- **Zeroing conditions**: `effective_intent >= 2` or unset.
- **Consumers**: `compute_focus_scores` (via `compute_cost_of_delay`).

### 2.2. Severity Bonus (`severity_bonus`) — does not exist; see `severity_gate`
No additive `severity_bonus` term exists anywhere in the engine, and none
has since `severity_gate` replaced it (§1, tuple element 1). Severity never
contributes points to `cost_of_delay`; it only ever (a) gates a node into
the `Catastrophic` band via `severity_gate` (SEV4 + `goal_type: committed`
only — a binary admission, never a magnitude), or (b) reaches
`cost_of_delay` indirectly through `urgency_term` (§2.7), where a target's
`S_lex` propagates to contributors via `contributes_to`/`blocks`/
`soft_blocks` (§4.3) — never through `children`, and never landing on a
container (§4.3, "conduit pass"). The table row above ("Replaced by
`severity_gate` (no double-count)") is the complete, current story; any
text elsewhere describing a `severity_bonus` formula with SEV-keyed point
values (`100,000`/`20,000`/`10,000`/`5,000`) describes a mechanism that
predates this spec and was never restored.

### 2.3. Deadline Urgency Ramp (`deadline_score`)
- **Code reference**: `compute_cost_of_delay`, `src/graph_store.rs:1805-1850`
- **Formula**:
  Let `today = Utc::now().date_naive()`, `due_date = parse(node.due)`, `days_until = (due_date - today).num_days()`, and `effort_days = parse_effort_days(node.effort).unwrap_or(3)`:
  - If `days_until < 0` (overdue):
    $$\text{deadline\_score} = 8000 + \min((-\text{days\_until}) \times 200, 4000) \quad \in [8000, 12000]$$
  - Else (`days_until >= 0`), let $\text{ratio} = \frac{\text{effort\_days}}{\max(\text{days\_until}, 1)}$:
    - If $\text{ratio} \ge 1.0$: $\text{deadline\_score} = 6000$
    - Else if $\text{ratio} > 0.5$: $\text{deadline\_score} = 2000 + \lfloor(\text{ratio} - 0.5) \times 8000.0\rfloor \quad \in (2000, 6000)$
    - Else if $\text{days\_until} \le 30$: $\text{deadline\_score} = 1000$
    - Else: $\text{deadline\_score} = 0$
- **Theoretical Range**: `0` to `12,000`.
- **Flag side-effect**: Sets `deadline_ramp_fired = deadline_score > 0` (used to suppress lateness double-counting in the stakeholder waiting bonus).
- **Default (absent input)**: `0`.
- **Zeroing conditions**: `due` is absent or unparseable, or `days_until > 30` and $\text{ratio} \le 0.5$.
- **Consumers**: `compute_focus_scores`.

### 2.3a. Courtesy-Review Decay (task_e2afd38e)

Without this term, an overdue `due` date is a **permanent, unconditional override**: `deadline_band == Overdue` sorts ahead of every non-overdue band regardless of `cost_of_delay` magnitude (§1 — the tuple compares `deadline_band` before `cost_of_delay`), and `deadline_score` caps at `12000` after 20 days overdue and never falls again. A task nobody ever closes out — a standing courtesy review-invitation, a recurring nag with a stale `due` — therefore outranks live work forever, growing more entrenched, never less, the longer it is ignored. Empirically confirmed live on 2026-09-02: `task_f1f59685` (due 2026-09-01, `focus_score` 10237) outranked `teaching` (no `due`, `focus_score` 50021, `urgency` 50000) in `list_tasks(status="ready")` purely on `deadline_band` — a ~1-day-overdue task with modest stakes beat a P1/P2 epic with 5000× the propagated urgency. `task_f1f59685` is correctly *unaffected* by the mechanism below (it is inside the 20-day grace window and carries a real `stakeholder`); the example is cited to show the band-dominance property this term exists to bound is live and material, not hypothetical.

- **Code reference**: `src/graph_store.rs`, inside `compute_cost_of_delay`'s `days_until < 0` branch (immediately after `deadline_score`/`deadline_band` are set).
- **Gate — applies only when a node is judged to carry no real stakes**, checked via signals the engine already reads for other terms (never a new field, never a tag someone has to remember to apply):
  ```
  has_real_stakes = downstream_weight > 0.0
      OR stakeholder.is_some()
      OR is_human_gate()
      OR urgency > 50.0          // no severity propagated to it (§4.3): while
                                  // overdue f(slack) == 10.0 for every node, so
                                  // urgency ≈ 10 × propagated_S_lex; 50 sits
                                  // strictly between the SEV0 floor (~10) and
                                  // the SEV1 floor (~100)
      OR intent < 2               // Nic-curated P0/P1 override (§2.1)
  ```
  A node escapes decay the instant it acquires any of these through a channel the model already treats as authoritative: becoming a real blocker, being named a stakeholder, being wired to a severity-bearing target via `contributes_to`, or being promoted by Nic. `stated_weight` and `intent` are themselves closed to agents — pauli/Nic only, per `kb_pauli_prioritisation_doctrine` §5 — so a task cannot quietly game itself out of decay by editing its own frontmatter.
  - **`severity` and `consequence` are deliberately excluded from this gate.** `kb_pauli_prioritisation_doctrine` §4 is explicit that `consequence` is explanatory prose the ranking engine must never read, and that `severity` belongs to target nodes, never tasks — a task-level `severity` read here would silently no-op on every correctly modelled task (which never carries one) while also being a second, new violation of the same rule.
- **Formula** (only when `!has_real_stakes` and `days_overdue > 20`):
  ```
  decay_days = min(days_overdue - 20, 100)
  decay_frac = decay_days / 100.0                       // 0.0 .. 1.0
  deadline_score_after = deadline_score - round(deadline_score * decay_frac)
  deadline_band_after  = Overdue      if decay_frac < 0.25
                        = Imminent    if decay_frac < 0.5
                        = Urgent      if decay_frac < 0.75
                        = Approaching if decay_frac < 1.0
                        = None        otherwise
  ```
  The first 20 days overdue are untouched for every node, stakes or none — this matches the point at which the pre-existing ramp itself already saturates at `12000`, so nothing changes for a task freshly overdue. Past that, both terms decay together and smoothly over the following 100 days, landing at **exactly** the values a task with no `due` at all would get (`deadline_score == 0`, `DeadlineBand::None`) once fully decayed at 120 days overdue — an expired courtesy review is ranked as what it functionally is: undated. Nothing is hidden, filtered, or deleted; it keeps surfacing in every list, just without the permanent band override.
- **Theoretical range**: `deadline_score` as §2.3; `deadline_band` additionally reachable at any of the five values for an overdue, no-stakes node (not just `Overdue`).
- **Default / zeroing conditions**: no-op (`has_real_stakes == true`, or `days_overdue <= 20`, or no `due` at all — falls through to §2.3 unchanged).
- **Consumers**: `compute_focus_scores` (same call site as §2.3; not a separate pipeline stage).
- **Rejected alternatives** (recorded per `.agent/CORE.md` — specs document approved current state, not the road not taken, so the reasoning lives here only because the originating task required it be recorded where the model is documented):
  - **An explicit `courtesy: true` frontmatter tag.** Rejected on curation-burden grounds: every task-creation path in this system (`create_task`, `claim_task`, `decompose_task`, `batch_create_epics`, ad-hoc creation via `release_task`) can mint a courtesy-shaped task, and a tag only helps the instances someone remembers to label — which is precisely the failure mode that let the two originating example tasks crowd in the first place (neither was tagged). This flips if the system gains a single, enforced choke point for review-shaped task creation (e.g. a dedicated review-invitation template) where tagging could be applied at creation time and never bypassed; no such choke point exists today.
  - **Blanket overdue decay (decay every overdue task, stakes or none).** Rejected: it fails the requirement that a real hard deadline must still bite, and repeats — at the deadline-band layer instead of the edge-weight layer — the exact mistake the standing-weight/edge-decay mechanism was rejected for (`pkb-prioritisation-evolution-plan`, "Decay: parked dormant" ruling, 2026-08-28): shipping a signal that measures a node's age, under an "attention" or "importance" label, rather than anything real about the node.

### 2.4. Age / Staleness Bonus (`age_staleness_bonus`)
- **Code reference**: `compute_focus_scores`, `src/graph_store.rs:2011,2021-2034`
- **Formula**:
  Only applies if `intent (node.intent.unwrap_or(4)) >= 2`:
  $$\text{age\_bonus} = \min(\max(\text{days\_since\_created}, 0), 200)$$
- **Theoretical Range**: `0` to `200`.
- **Observed Range**: `0` to `200`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: raw `intent < 2` (P0 and P1 bypass staleness ramp; note this reads the node's own stated `intent`, not `effective_intent` — unlike `intent_pressure` in §2.1) or `created` date missing/unparseable.
- **Consumers**: `compute_focus_scores`.

### 2.5. Downstream Weight Term (`(downstream_weight * 10.0) as i64`)
- **Code reference**: `compute_focus_scores`, `src/graph_store.rs:2035`
- **Formula**:
  $$\text{term} = \lfloor \text{node.downstream\_weight} \times 10.0 \rfloor$$
- **Theoretical Range**: `0` to $\infty$.
- **Observed Range**: `0` to `~500`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: Empty downstream cone or all descendants completed.
- **Consumers**: `compute_focus_scores`.

### 2.6. Stakeholder Waiting Urgency (`stakeholder_waiting_bonus`)
- **Code reference**: `src/graph_store.rs:1932–1965` (`compute_cost_of_delay`).
- **Ruling (Nic, 2026-09-11, `mem_537e44a9` "Verdict: stakeholder_waiting / human gate"):** this term fires **only when a `stakeholder` is actually named** — never merely because `node.is_human_gate()` is true. A bare `decision`-tagged node, or any node satisfying `is_human_gate()` (`status: review`, or tags `decision`/`sign-off`/`signoff`/`one-way-door`/`human-approval`), with no named stakeholder has nobody waiting on it and earns no waiting clock. `is_human_gate()` continues to gate the courtesy-decay mechanism (§2.3a's `has_real_stakes`) and `focus_picks`/`pkb focus` surfacing (both structurally distinct from this accrual) — it no longer independently triggers this term.
- **Formula**:
  Applies when `node.stakeholder.is_some()`:
  - If `deadline_ramp_fired` is `true`:
    $$\text{score} += 2000 \quad \text{(base only, suppressing per-day lateness growth to prevent double-counting)}$$
  - Else (`deadline_ramp_fired` is `false`):
    Let $\text{anchor} = \text{parse}(\text{node.waiting\_since} \lor \text{node.created})$, and $\text{days} = \max((\text{today} - \text{anchor}).\text{num\_days}(), 0)$:
    $$\text{score} += 2000 + \min(\text{days} \times 200, 6000) \quad \in [2000, 8000]$$
    If anchor date is missing or unparseable, score is `2000`.
- **Theoretical Range**: `0`, or `2000` to `8000`.
- **Observed Range**: `0` (unset) or `2000`–`8000`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: `stakeholder` field is absent (`None`) — `node.is_human_gate()` is irrelevant to this term.
- **Consumers**: `compute_focus_scores`.

### 2.7. Urgency Term (`urgency_term`)
- **Code reference**: `src/graph_store.rs:1799`
- **Formula**:
  $$\text{term} = \text{round}(\text{node.urgency}) \quad \text{as } i64$$
- **Theoretical Range**: `0` to `10,000+`.
- **Observed Range**: `0` to `10,000`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: Completed status, or no severity/deadlines downstream.
- **Consumers**: `compute_focus_scores`.

### 2.8. Value of Information Term (`voi_term`)
- **Code reference**: `src/graph_store.rs:1800–1802`
- **Formula**:
  $$\text{term} = \begin{cases} \text{round}(\text{voi}) \text{ as } i64 & \text{if } \text{node.voi\_value} = \text{Some}(\text{voi}) \\ 0 & \text{if } \text{node.voi\_value} = \text{None} \end{cases}$$
- **Theoretical Range**: `0` to `5000`.
- **Observed Range**: `0` to `~1500`.
- **Default (absent input)**: `0` (non-leaf tasks produce `None`).
- **Gating Conditions (Phase 3)**:
  Requires two conjunctive conditions independently:
  1. **Open question**: Content-judged inquiry / open question or explicit confidence `< 1.0` (on the task itself or in its downstream unblocking cone). Settled questions (e.g. craft glue) produce zero VoI.
  2. **Downstream divergence**: Unblocking cone must contain $\ge 2$ distinct reachable nodes or direct blocks (branching paths). Purely linear, unbranching chains produce zero VoI.
  Neither condition alone earns the bonus.
- **Zeroing conditions**: Non-leaf task (`leaf == false`), complete/cancelled status, no open question, no downstream divergence, or `dep_resolution_ratio == 0`.
- **Consumers**: `compute_focus_scores`.

### 2.9. Value Lineage Term (`value_lineage_term`) — Phase 2
- **Code reference**: `compute_value_lineage`, `src/graph_store.rs`; folded into `cost_of_delay` in `compute_cost_of_delay`.
- **Formula**: $\text{term} = \text{round}(\text{node.value\_lineage})$ as `i64`. See §4.11 for how `node.value_lineage` itself is computed, including the conduit pass that routes a container's value down to its nearest ready, unblocked leaf (Nic, 2026-09-12, `mem_537e44a9` "Verdict: value flow to children").
- **Theoretical Range**: `0` to `10,000` (a Critical/1.00 standing weight × a Certain/1.00 edge × full/1.0 confidence).
- **Observed Range**: `0` for nodes with no priced target in their lineage. `[[targ_4e2cc92a]]` is priced (`standing_weight: 0.60`, `goal_type: null`) as of 2026-09-12 — the first live nonzero source, and proof that pricing is not gated on `goal_type` (§4.11).
- **Default (absent input)**: `0` — a node with no `contributes_to` edge to a priced target anywhere in its own edges or its ancestor chain, or a target with no `standing_weight` elicited, contributes nothing. No inference, no default weight (Zero Defaults / Zero Inference).
- **Consumers**: `compute_focus_scores`.

---

## 3. Disparity Between Theoretical Caps and Observed Ranges

The eight additive terms carry widely disparate theoretical caps versus realised empirical dynamics. This was the diagnosed failure Phase 2 exists to fix for the graph/importance channel specifically: `downstream_weight × 10` (a tie-breaker, not `cost_of_delay`) contributed at most ~53 observed points against a scale running to ~11,000, while `contributes_to` — the sanctioned channel by which agents are instructed to express importance instead of setting priority bands — reached the score only through that same ~53-point term. `value_lineage_term` (§2.9, §4.11) is the repair: it enters `cost_of_delay` directly, at the same 0–10,000 order of magnitude as `intent_pressure`/`urgency_term`, so it can actually move a ranking.

| Term | Theoretical Range | Observed Range (Typical) |
| --- | --- | --- |
| `deadline_score` | 0 – 12,000 | 0 – 12,000 |
| `intent_pressure` | 0 – 10,000 | 0 – 10,000 |
| `urgency_term` | 0 – 10,000 | 0 – 10,000 |
| `value_lineage_term` | 0 – 10,000 | 0 – 6,000 (`[[targ_4e2cc92a]]`, priced 0.60 — §2.9) |
| `stakeholder_waiting` | 0 – 8,000 | 0 – 8,000 |
| `voi_term` | 0 – 5,000 | 0 – ~1,500 |
| `age_staleness_bonus` | 0 – 200 | 0 – 200 |
| `downstream_weight × 10` (tie-breaker) | 0 – $\infty$ | 0 – ~500 |
| `unlock_breadth × 10` (tie-breaker) | 0 – $\infty$ | not yet measured on the live PKB |
| **`focus_score` Composite** | **0 – ~55,200** | **0 – ~11,000+** |

No `severity_bonus` row exists: the additive severity bonus this table
once carried was replaced by `severity_gate` (§1, §2.2) — a binary
admission to the `Catastrophic` band, never an additive point value — so
it is excluded from this sum. `severity` still reaches `cost_of_delay`
indirectly through `urgency_term`.

---

## 4. Graph Metrics and Derived Diagnostic Measures

In addition to `focus_score`, `mem` computes several topological and network measures during the graph build.

### 4.1. `downstream_weight`
- **Code reference**: `src/graph_store.rs:2770–2813`, `2606–2650`
- **Definition**: A depth-decayed, edge-factor-discounted sum of weighted base scores over the distinct nodes reachable in the downstream cone. **It is NOT a count of nodes.**
- **Formula**:
  $$\\text{downstream\\_weight}(x) = \\sum_{t \\in \\text{cone}_{\\text{Structural}}(x)} \\frac{1}{\\text{depth}(t)} \\cdot \\text{base\\_weight}(t) \\cdot \\text{edge\\_factor}(t)$$
  where:
  - $\\text{cone}_{\\text{Structural}}(x)$ expands via BFS over `blocks`, `soft_blocks`, parent→child (`children`), and reverse `contributes_to`.
  - BFS expansion depth is bounded by `MAX_CONE_DEPTH = 20`.
  - $\\text{base\\_weight}(t) = \\text{priority\\_weight}(t) \\times \\text{due\\_multiplier}(t)$:
    - $\\text{priority\\_weight} \\in \\{P0: 5.0, P1: 3.0, P2: 2.0, P3: 1.0, \\text{other}: 0.5\\}$
    - $\\text{due\\_multiplier} = 2.0 \\text{ if } due \\text{ is present, else } 1.0$.
    - Completed or cancelled nodes have $\\text{base\\_weight} = 0.0$.
  - $\\text{edge\\_factor}(t)$ is $1.0$ for structural edges, or the verbal contribution weight for `contributes_to`.
- **Theoretical Range**: $[0.0, \\infty)$.
- **Observed Range**: $0.0$ to $5.27$.
- **Default**: $0.0$.
- **Consumers**: `focus_score`, `compute_criticality`, `top_n_by_metric`, `get_network_metrics`, `list_tasks` ready view table.

### 4.2. `criticality`
- **Code reference**: `src/graph_store.rs:3326–3349`
- **Formula**:
  $$\\text{raw}(x) = \\text{downstream\\_weight}(x) + 10.0 \\times \\text{pagerank}(x) + (3.0 \\text{ if } \\text{stakeholder\\_exposure}(x) \\text{ else } 0.0)$$
  $$\\text{criticality}(x) = \\frac{\\text{raw}(x)}{\\max_{y \\in V} \\text{raw}(y)} \\quad \\in [0.0, 1.0]$$
- **Theoretical Range**: $[0.0, 1.0]$.
- **Observed Range**: $0.0$ to $1.0$.
- **Default**: $0.0$.
- **Consumers**: `top_n_by_metric`, `get_network_metrics`, signals in `get_task` and `list_tasks`, overwhelm dashboard (`focusBreakdown.ts`, `prepareGraphData.ts`).

### 4.3. `urgency` and Urgency Propagation
- **Code reference**: `compute_urgency`, `src/graph_store.rs:3691`.
- **Formula**:
  $$\\text{urgency}(x) = S_{\\text{lex}}(x) \\times f(\\text{Slack}(x))$$
  - Base Severity Score $S_{\\text{lex}}$:
    - If `severity == 4` and `goal_type == "committed"`: $S_{\\text{lex}} = 10000.0$ (lexicographic override).
    - Else: $S_{\\text{lex}} = 10^{\\min(\\text{severity}, 3)}$ (e.g. SEV0 $\\to 1$, SEV1 $\\to 10$, SEV2 $\\to 100$, SEV3 $\\to 1000$).
  - Slack Calculation: $\\text{slack}(x) = (\\text{due} - \\text{today}).\\text{num\\_days}() - \\text{effort\\_days}$. `effort_days` is the node's own parsed `effort` field, default `3` days. Default unconstrained slack (no reachable `due`) is $100.0$ days.
  - Urgency Propagation: Urgency propagates backward from blocked tasks to blockers **by relaxation** (Phase 2; previously a single-pass BFS — see 4.3a) over paths up to depth 20:
    - `blocks`: factor $1.0$
    - `soft_blocks`: factor $0.3$
    - `contributes_to`: verbal weight anchor ($0.00$ to $1.00$)
    - `children` is **not** a propagation edge (ruling, Nic, 2026-09-12, `mem_537e44a9` "Verdict: value flow to children"; superseded the prior `0.5`-factor parent-inherits-from-children edge, which was the wrong direction — "a child is what advances the parent, so it carries the parent's pressure," not the reverse). See the conduit pass below for how a container's urgency instead reaches its leaves.
    Propagated values: $\\text{propagated\\_s\\_lex}(x) = \\max(S_{\\text{lex}}(x), \\max_{\\text{paths}} S_{\\text{lex}}(t) \\times \\text{path\\_factor})$, and $\\text{min\\_slack}(x) = \\min(\\text{slack}(x), \\min_{t} \\text{slack}(t))$.
  - Slack Function $f(\\text{Slack})$ ($\\text{SAFE\\_HORIZON} = 30.0$, $k = \\frac{\\ln(10)}{30.0}$) — a single continuous exponential for all positive slack, **not** a step at `slack = 30`:
    $$f(\\text{slack}) = \\begin{cases} e^{k(30.0 - \\text{slack})}.\\max(0.001) & \\text{if } \\text{slack} > 0.0 \\\\ 10.0 & \\text{if } \\text{slack} \\le 0.0 \\end{cases}$$
    The `.max(0.001)` floor is reached only asymptotically — at `slack = 120` days, not `slack = 30`. A node at `slack = 30` still scores `f(30) = 1.0`; the curve keeps decaying smoothly past that point (e.g. `f(100) ≈ 0.0046`) until the floor binds around `slack = 120`.
  - Guard: If committed SEV4 and $\\text{slack} \\le 0.0$, urgency is clamped to exactly $10000.0$ — **and so is any contributor whose strongest propagation path originates from such a target** (ruling, Nic, 2026-09-12, `mem_537e44a9` "SEV4 overdue pin reaches contributors"): the pin was previously applied only to the target's own node, letting a contributor on, say, an Expected (`0.75`) edge score `10000 × 0.75 × 10 = 75000` uncapped. Contributors now compete among themselves at the same `10000` band instead.
  - **Conduit pass** (outcome 5, ruling as above, "Verdict: value flow to children"): after the relaxation above, any node with children (`!node.children.is_empty()`) does not keep its own computed `urgency` — it is zeroed. That value is instead pushed down, via one walk up the `parent` chain per leaf (mirroring `compute_effective_intent`'s ancestor-pressure channel, §4.6), to the nearest ready, unblocked leaf descendant, which inherits the maximum of its own urgency and every ancestor's (pre-conduit) urgency. A blocked or completed leaf is itself a conduit and inherits nothing from this *inheritance* — but it is not zeroed outright the way a blocked leaf's `value_lineage` is (§4.11): it keeps its own **intrinsic** baseline, `s_lex[i]` at its own (non-chain-propagated) slack, recomputed fresh rather than reused from the pre-conduit `urgency` value — because that pre-conduit value can itself already include a *received* boost from a target reached via `blocks`/`soft_blocks`/`contributes_to`, and a blocked node must not keep a boost it merely received (same "never on anything itself blocked" rule), even though it must keep whatever severity/deadline pressure is genuinely its own (mirroring `effective_intent`'s blocked gate, which likewise falls back to "its own stated intent, full stop," never to zero). Concretely: a committed-SEV4 node with its own near-term `due` that happens to also carry an unmet `depends_on` still shows a high `urgency` from its own severity — it does not read as 0 merely for being technically blocked; but a blocked node whose only urgency came from propagation through a `contributes_to` edge to some other severity-bearing target drops to that same near-zero intrinsic floor. "A target's weight lands on the ready, unblocked leaves that advance it — never on the container, never on anything itself blocked" (Nic, verbatim) — read in light of this distinction, "blocked" bars *inherited* value, not a node's own severity.
  - Completed nodes have $\\text{urgency} = 0.0$.
- **Theoretical Range**: $0.0$ to $10,000.0$.
- **Consumers**: `compute_focus_scores`, `focus_picks`, `get_task` / `list_tasks` signals.

### 4.3a. Chain Slack (`chain_slack`) and the V9 Relaxation Fix — Phase 2
- **Code reference**: `compute_urgency`, `src/graph_store.rs` (same function as §4.3 — one traversal, two published outputs, "no fourth graph computation").
- **What it is**: $\text{min\_slack}(x)$ from §4.3, in days, exposed as its own field (`node.chain_slack`) rather than only as an internal input to $f(\text{Slack})$. CPM-style: the minimum float/slack found anywhere in the blocking chain reachable from $x$.
- **The V9 fix**: the traversal in §4.3 used to mark a neighbour visited **on enqueue** and never revisit it, in a per-start-node BFS. Because $\text{propagated\_s\_lex}$ depends on *which path* reached a node (`s_lex[t] × path_factor`), whichever path arrived **first** permanently fixed the factor used — a stronger path discovered later (higher `path_factor`, e.g. a direct `blocks` chain found two hops after a `soft_blocks` shortcut) could never correct it upward. This under-counted urgency without any error or warning.
  - **Fix**: the traversal now relaxes on strict improvement — a node is (re-)enqueued whenever a larger `path_factor` than any seen so far for it is found in *this* start node's traversal, replacing the old "visited once" gate with a `best_factor` array. `propagated_s_lex` therefore always reflects the strongest path across the whole traversal.
  - **The min-slack half needed no such fix.** A node's own slack does not depend on `path_factor` — visiting it once already gave the correct minimum. Only the $S_{\text{lex}}$ half was defective; the fix does not touch how `min_slack` is computed, only how the shared traversal revisits nodes.
  - **Termination**: every edge factor is in $[0.0, 1.0]$, so `path_factor` is non-increasing with depth; the number of distinct strict improvements per node is bounded, and the pre-existing depth-20 cap bounds it further.
- **Theoretical Range**: $[-\infty, 100.0]$ in practice; $100.0$ is the "unconstrained" sentinel (no due date reachable within the depth cap).
- **Consumers**: `compute_urgency` (§4.3, internally), `get_task` / `list_tasks` signals.

### 4.4. `voi_value` (Value of Information)
- **Code reference**: `src/graph_store.rs:3144–3215`
- **Formula**:
  Only computed for leaf tasks (`leaf == true`):
  $$\text{VoI}(x) = \begin{cases} \min\left(K_{\text{VOI}}, K_{\text{VOI}} \times 1.0 \times \text{dep\_resolution\_ratio}(x) \times \frac{\sum_{t \in \text{cone}_{\text{Unblocking}}(x)} \frac{1}{\text{depth}(t)} \cdot \text{base\_weight}(t) \cdot \text{edge\_factor}(t) \cdot \text{uncertainty}(t)}{\max(\text{effort\_days}, \text{VOI\_EFFORT\_NEUTRAL\_DAYS})}\right) & \text{if } C_{\text{open}}(x) \land C_{\text{divergence}}(x) \\ 0.0 & \text{otherwise} \end{cases}$$
  where:
  - $K_{\text{VOI}} = 5000.0$
  - $\text{VOI\_EFFORT\_NEUTRAL\_DAYS} = 3.0$
  - $\text{dep\_resolution\_ratio}(x) = \frac{\text{completed\_deps}(x)}{\text{total\_deps}(x)}$ (or $1.0$ if no dependencies).
  - $\text{cone}_{\text{Unblocking}}(x)$ traverses `blocks`, `soft_blocks`, and parent $\to$ child (excludes reverse `contributes_to`).
  - **Conjunctive Gating Conditions (Phase 3)**:
    - $C_{\text{open}}(x)$: Task itself has an open question (`has_open_question = true` or `confidence < 1.0`), OR its unblocking cone contains an open question.
    - $C_{\text{divergence}}(x)$: Unblocking cone has branching paths ($|\text{cone}| \ge 2$ or $|\text{blocks}| \ge 2$).
    - Both conditions are independently required; neither alone earns the bonus.
- **Theoretical Range**: `0.0` to `5000.0` (or `None` for non-leaf tasks).
- **Consumers**: `compute_focus_scores`, `get_task` / `list_tasks` signals.

### 4.5. `uncertainty`
- **Code reference**: `src/graph_store.rs:3458–3470`
- **Formula (Phase 3 Honest Uncertainty)**:
  Uncertainty strictly measures epistemic variance, decoupled from documentation completeness:
  - If `confidence` is explicitly set: $\text{uncertainty} = \text{clamp}(1.0 - \text{confidence}, 0.0, 1.0)$.
    Supports numeric floats and Renooij-Witteman verbal anchors ("certain": 1.00, "probable": 0.85, "expected": 0.75, "fifty-fifty": 0.50, "uncertain": 0.25, "improbable": 0.15, "impossible": 0.00).
  - Else if `has_open_question` is `true`: $\text{uncertainty} = 0.50$ (default fifty-fifty epistemic uncertainty for an open question).
  - Else: $\text{uncertainty} = 0.0$ (settled / certain).
  - **Demotion**: Missing acceptance criteria, body length, and child count feed NO score path (demoted to triage lint `task-missing-ac` in `src/lint.rs`).
- **Theoretical Range**: $[0.0, 1.0]$.
- **Consumers**: `compute_voi_term`, `get_task` / `list_tasks` signals.

### 4.6. `effective_intent` (mem_intent_ready_weight; `kb_pauli_prioritisation_doctrine` §6)
- **Code reference**: `compute_effective_intent`, `src/graph_store.rs:3354-3464`.
- There is no `effective_priority` field, `own_priority` function, or single undirected downstream-cone cascade. The prior shape (a min-cascade over `blocks`/`soft_blocks`/`children`/`contributes_to` in one pass, letting a parent silently absorb a child's urgency and a blocked node inherit pressure from what it blocks) was replaced (PR #616, "stop intent cascading to children; gate ready-node weight on blocked status") by two independently gated channels, neither of which is `min` over a "downstream cone" containing `children`:
  - **Blocker channel.** DFS over `blocks`, `soft_blocks`, `contributes_to` — **excluding `children`** — taking the lowest `intent` found in what the node transitively blocks (skipping completed nodes). A node blocking a P0 task gets pulled toward `0`.
  - **Ancestor-pressure channel.** Walks `parent` to the root, taking the lowest `intent` found among ancestors. A ready child of a P0 epic gets pulled toward `0` — the opposite traversal direction from the blocker channel, and the reverse of what the old single-cascade shape did with `children`.
  - **Gate.** A node that is itself effectively blocked (`compute_effectively_blocked`: unmet hard `depends_on`, explicit `status: blocked`, or transitively blocked downstream of one) receives **neither** channel — its `effective_intent` is just its own stated `intent`, full stop.
  Both channels seed from, and can only lower (never raise), the node's own stated `intent` (`node.intent.unwrap_or(4)`).
- **Theoretical Range**: `0` (P0) to `4` (P4). Default: `4`.
- **Consumers**: `intent_pressure` in `compute_cost_of_delay` (§2.1 — reads the gated/propagated value, not raw `intent`), secondary tie-breaker in `focus_cmp` (§8.4), `classify_tasks` ready sorting, priority filtering in `list_tasks`.

### 4.7. `scope`
- **Code reference**: `src/graph_store.rs:3224–3263`
- **Formula**: Recursive count of all unique descendants in the parent-child hierarchy (cycle-protected via visited set).
- **Theoretical Range**: $0$ to $N$.
- **Consumers**: Signals in `get_task` and `list_tasks`.

### 4.8. `pagerank`, `betweenness`, and Degree Centralities
- **Code reference**: `src/metrics.rs:21–215`
- **PageRank**: Power iteration over 20 iterations, damping factor $d = 0.85$.
- **Betweenness Centrality**: Exact Brandes algorithm over all edges with undirected normalization $\\frac{1}{(n-1)(n-2)}$.
- **Degrees**: Count of `in_degree` and `out_degree` over all edge types.
- **Consumers**: `compute_criticality`, `top_n_by_metric`, `get_network_metrics`.

### 4.10. `unlock_breadth` — Phase 2
- **Code reference**: `compute_unlock_breadth`, `src/graph_store.rs`.
- **Definition**: The cost-of-delay-weighted mass of what completing this node would *directly* unblock. **It is NOT a count of unblocked nodes** — the same discipline §4.1 already applies to `downstream_weight`.
- **Formula**: For node $x$, and each $t$ in $x$'s `blocks` list:
  $$\text{unlock\_breadth}(x) = \sum_{t \,:\, \text{last\_blocker}(x, t)} \text{cost\_of\_delay}(t)$$
  where $\text{last\_blocker}(x, t)$ is true iff $t$ is not already completed and every id in $t$'s hard `depends_on` **other than** $x$ is itself in a completed status — i.e. finishing $x$ actually flips $t$ from blocked to unblocked, not merely removes one of several open blockers. Soft dependencies are excluded (they do not gate ready/blocked classification, §8.1–8.2). $\text{cost\_of\_delay}(t)$ is the exact same function used for $t$'s own tuple (`compute_cost_of_delay`, §2), so the two can never define "cost of delay" two different ways.
- **Deliberately shallow (one hop)**: multi-hop cascades are `downstream_weight`'s job (parent plan Phase 2: "one shallow metric kept").
- Each dependent's `cost_of_delay(t)` is defensively clamped to `>= 0` before summing (`.max(0)`); in practice every component of `cost_of_delay` is already non-negative, so this never changes a live value — it only guards against a future term with a signed contribution silently making `unlock_breadth` negative.
- **Theoretical Range**: $[0.0, \infty)$ — same reason `downstream_weight` is unbounded (§4.1); in practice bounded by the number and cost-of-delay of a node's direct dependents.
- **Consumers**: `compute_focus_scores` (`tie_breakers.unlock_breadth_x10` — a tie-breaker, **not** `cost_of_delay`; see the note at the end of §3), `get_task` / `list_tasks` signals.

### 4.11. `value_lineage` — Phase 2
- **Code reference**: `compute_value_lineage`, `src/graph_store.rs:4001`.
- **Definition**: Standing weight elicited on **any** target/goal node that carries one, flowing multiplicatively to a contributor via `contributes_to`. This is the mechanism the doctrine in §7 and the parent plan's "Nic prices the destinations; the system prices the routes" require.
- **Formula**:
  $$\text{value\_lineage}(x) = K_{\text{VL}} \times \text{confidence}(x) \times \sum_{ct \,\in\, x.\text{contributes\_to}} ct.\text{numeric\_weight}() \times \text{standing\_weight}(ct.\text{target})$$
  where $K_{\text{VL}} = 10{,}000$ (§2.9), $\text{confidence}(x)$ defaults to $1.0$ when unset (mirrors `compute_uncertainty`'s existing "missing confidence, no open question ⇒ certain" default — a default on the *contributor's* stated confidence, never on the target's `standing_weight`, which is strictly `None`-means-zero), and a target contributes nothing to the sum unless it has a priced `standing_weight` (`pkb-standing-weight-elicitation-instrument` §1; "Zero Defaults / Zero Inference"). **`goal_type` is not part of this gate** (ruling, Nic, 2026-09-12, `mem_537e44a9` "Verdict: goal_type gating"), verbatim: "price should operate even when targets have null category. we shouldn't encourage that state, but while it's legal, we shouldn't always count [i.e. discount] targets that exist." The prior formula filtered the sum to `goal_type(ct.target) == committed`; that filter is gone. `goal_type: committed` remains load-bearing only at the three SEV4 lexicographic-override sites (`severity_gate`, the `S_lex` base in §4.3, and the overdue-pin guard in §4.3) — those have a recorded rationale ("prevents moonshots from hijacking the focus queue," `specs/multi-parent.md` §1.3) that never applied to ordinary pricing.
- **One hop from the target, then down to the nearest ready leaf**: the pricing walk itself is one hop — it reads a node's own `contributes_to` edges directly and does not chain transitively through a contributor's own further edges (a contributor of a contributor of a priced target earns nothing unless it also has its own direct edge). But the edge-holder does not necessarily keep the resulting value: if it is a container (`!node.children.is_empty()`), the value is zeroed on the container and instead pushed down — one walk up the `parent` chain per leaf, same shape as `compute_effective_intent`'s ancestor-pressure channel — to its nearest ready, unblocked leaf descendant (ruling, Nic, 2026-09-12, `mem_537e44a9` "Verdict: value flow to children": "A target's weight lands on the ready, unblocked leaves that advance it — never on the container, never on anything itself blocked"). A blocked or completed leaf is itself a conduit: it inherits nothing from this pass **and** its own directly-computed value (from its own `contributes_to` edge to a priced target, if it has one) is zeroed too, not merely left un-boosted — `value_lineage` has no notion of an "intrinsic" value the way `urgency`'s `S_lex` does (§4.3), since value_lineage is entirely a function of edges to priced targets, so a blocked contributor's own direct edge is exactly the "value... from its target" the ruling bars. (Contrast `urgency` §4.3, where a blocked node's own severity/deadline baseline survives — the two terms are asymmetric here because only `urgency` has a self-contained baseline independent of any edge.) This replaces the prior strict one-hop-and-stop reading (`ranking.md:412`, pre-2026-09-12) and is still a local per-node scan, not a new whole-graph cone-walk mechanism ("no fourth graph computation").
- **Sibling-contributor combination semantics (settled)**: independent and additive. Multiple nodes contributing to the same target are each scored off their own edge alone — nothing reduces a contributor's credit because other contributors also point at the same target. This is a deliberate rejection of a Birnbaum-style reliability combination (cut sets, structure functions) across sibling edges, consistent with the parent plan's "Explicitly not building: Birnbaum importance proper" and §7's existing disclaimer that the verbal scale computes no such thing. See §7 for the corrected gloss this setting fixes.
- **Theoretical Range**: `0` to `10,000` per contributing edge (§2.9); a node with edges to multiple priced targets sums across them.
- **Observed Range**: `0` for nodes with no priced target in their lineage. `[[targ_4e2cc92a]]` is priced (`standing_weight: 0.60`, `goal_type: null`) as of 2026-09-12, the first live nonzero source.
- **Consumers**: `compute_focus_scores` (`cost_of_delay`, §2.9 — **not** a tie-breaker; this is the term that must be able to move a ranking), `get_task` / `list_tasks` signals.

---

## 5. Standing Doctrine on Graph Metrics

> **Standing Ruling (Phase 2):**
> `pagerank`, `betweenness`, and `criticality` **never enter any focus_score or ranking path** — but they stay as **derived diagnostics**, computed for the surfaces that already consume them (`top_n_by_metric`, `get_network_metrics`, `get_task`/`list_tasks` signals, the ready-view table, and the overwhelm dashboard).

This doctrine is grounded in four structural realities:

1. **Incommensurable Units:** PageRank, betweenness, and criticality are unitless structural measures over heterogeneous edges. They have no natural exchange rate with cost-of-delay or time urgency.
2. **Redundant Causal Information:** The actionable structural content of the graph is already captured directly in interpretable domain units — bottleneck urgency is captured by chain slack and LST (§4.3, §4.3a); strategic importance is captured by target value lineage (§4.11). Betweenness and PageRank are merely correlational shadows of those causal paths.
3. **Snapshot Incomparability:** Normalized criticality ($\\text{raw} / \\max(\\text{raw})$) changes scale whenever the max node changes, making values incomparable across time snapshots.
4. **Authoring Artifacts:** On a mixed-edge personal knowledge graph, high graph centrality frequently reflects authoring habits or incomplete decomposition rather than real-world task value. They serve as a valuable **gardening lens** (identifying decomposition smells, promotion candidates, or structural hubs) rather than a dynamic operational rank.

---

## 6. The Severity Ladder

Three different severity mappings historically coexisted across specifications. The shipped reality is:

1. **`severity_gate` in `compute_focus_scores`** — a binary admission to the `Catastrophic` band, not an additive bonus (§1, §2.2): `Catastrophic` iff `severity >= 4 && goal_type == "committed"`, else `Normal`. No `severity_bonus` term (`100,000`/`20,000`/`10,000`/`5,000` by SEV level) exists in code; that mapping was retired when the additive accumulator was replaced by the lexicographic tuple and was never restored.
2. **Urgency Base $S_{\\text{lex}}$ in `compute_urgency`**:
   - `SEV4` + `goal_type: committed` = `10,000.0`
   - `SEV0`–`SEV3` = $10^{\\min(\\text{sev}, 3)}$ (`1`, `10`, `100`, `1000`)
3. **Legacy tables** (e.g. `1/2/3/5/5` in early draft specs) do not exist in code and are obsolete.

---

## 7. Verbal Contribution-Weight Scale

The `contributes_to.weight` (or `stated_weight`) field implements a **verbal contribution-weight scale** using Renooij-Witteman verbal terms mapped to non-linear anchors (`src/graph.rs:111–134`):

| Verbal Term | Stated Weight Value | Numeric Anchor | Interpretation |
| --- | --- | --- | --- |
| Certain / Almost Certain | `"certain"`, `"almost certain"` | **1.00** | Critical path / single point of failure |
| Probable / Very Probable | `"probable"`, `"very probable"`, `"highly likely"` | **0.85** | Strong primary contributor |
| Expected / Likely | `"expected"`, `"likely"` | **0.75** | Standard intended contributor |
| Fifty-Fifty / Even Chance | `"fifty-fifty"`, `"even chance"` | **0.50** | Moderate, genuinely uncertain contribution — a coin-flip call on *this edge alone* |
| Uncertain / Possible | `"uncertain"`, `"possible"`, `"perhaps"`, `"maybe"` | **0.25** | Exploratory or optional contribution |
| Improbable / Unlikely | `"improbable"`, `"unlikely"`, `"very unlikely"` | **0.15** | Minor marginal contribution |
| Impossible / None | `"impossible"`, `"none"` | **0.00** | No contribution |
| *Unrecognized* | *any non-empty string not in this table* | **rejected** | `ParseWarning` at parse time (`GraphNode::from_pkb_document`, field `contributes_to.stated_weight`); contributes `0.0`, never a fabricated default |
| *Unstated* | *omitted, or empty string* | **0.00** | Not an error — a deliberately unstated edge — but still `0.0`, not a "soft" default |

> **Note on Naming:** This is an **elicitation scale** for human/agent calibration. It is **not** a computational Birnbaum structural reliability model (it computes no cut sets, partial derivatives, or multi-contributor Boolean structure functions). In all agent-facing schemas and documentation, it is referred to as the **verbal contribution-weight scale**.

> **Corrected (Phase 2): the fifty-fifty gloss and sibling-contributor semantics.** The fifty-fifty row previously read "moderate contributor; redundancy exists" — wording that reads as a claim about the graph's *combinatorial structure* (that other contributors exist and can substitute for this one), which only has computational meaning if the system combines sibling contributors' weights against each other. It does not, and per the Naming note above and the parent plan's "Explicitly not building: Birnbaum importance proper (structure functions, cut sets, partial derivatives)", it deliberately never will. Value lineage (§4.11) settles this: sibling contributors to the same target are scored **independently and additively** — each contributor's credit depends only on its own edge, its own confidence, and the target's standing weight, never on how many other edges also point at that target. The corrected reading of "fifty-fifty" is purely an elicitation instruction for *this one edge*: state 0.50 when you, the author of this specific edge, are genuinely unsure this contribution will land — not as a claim about the wider graph.
>
> **Two previously-silent behaviours are now explicit rejections, not defaults.** An empty/omitted `stated_weight` was always meant to be a neutral "unstated" edge, but `numeric_weight()`'s catch-all silently mapped it — and any *misspelled or invented* verbal term — to the same `0.30` "soft contribution" value, with no warning either way. As of Phase 2 the catch-all is `0.0`, and a non-empty term that fails to match this table is flagged via the same `ParseWarning` mechanism `severity`/`goal_type` already use (`src/graph.rs`), surfaced through the existing linter/`/maintain` parse_warnings channel (no new surfacing mechanism). A genuinely omitted weight is not flagged — omitting a weight is not a mistake.

---

## 8. Task Classification and Queue Predicates

`GraphStore::classify_tasks` (`src/graph_store.rs:3351–3450`) partitions tasks into `ready`, `blocked`, and `roots`. To prevent non-actionable documentation/spec nodes from polluting actionable queues, classification is restricted to `ACTIONABLE_TYPES` (`["epic", "task", "learn", "pr"]`):

### 8.1. Ready Predicate
A task is placed in the `ready` list if and only if:
1. It is a leaf node (`node.leaf == true`).
2. Its type is in `CLAIMABLE_TYPES` (`["task"]`).
3. Its status is actionable: either `status == "ready"`, or (`status == "inbox"` and `node.has_acceptance_criteria == true`).
4. It is **not** directly or transitively blocked (all hard `depends_on` are in `COMPLETED_STATUSES` `{"done", "cancelled"}`, and no ancestor in the `blocks` chain is blocked).

### 8.2. Blocked Predicate
A task is `blocked` if:
1. Its type is in `ACTIONABLE_TYPES` (`["epic", "task", "learn", "pr"]`), and
2. Either:
   a. It has $\\ge 1$ unmet `depends_on` dependency whose status is not completed, OR
   b. Its own status is `"blocked"`, OR
   c. It is reachable via downstream propagation through `blocks` edges from any directly blocked task.

### 8.3. Roots Predicate
Roots are defined as tasks with no parent or whose parent is not in the index, restricted to `ACTIONABLE_TYPES` (`["epic", "task", "learn", "pr"]`).

### 8.4. Canonical Sort Order (`focus_cmp`)
All flat task listings in MCP (`list_tasks`) and CLI (`pkb tasks`, `pkb list`) use the single canonical comparator `GraphStore::focus_cmp` (`src/graph_store.rs:937-949`):
1. **`focus_tuple` DESC** — nodes carrying a tuple sort by it (reversed `cmp`, so the "largest" tuple sorts first); a node with `None` (filtered/unscored — e.g. `affordable_loss: false`, or completed) sorts after any node that has a tuple. This is the §1 tuple, **not** `focus_score` — the two are demonstrably not the same sort key (§1).
2. **Only when *both* nodes have no tuple** (both filtered/unscored) does the comparator fall through to a secondary chain: `effective_intent` **ASC** (§4.6) → `order` **ASC** (manual sequence order) → `id` **ASC** (guarantees a deterministic, total order). This fallback exists so unscored nodes still sort deterministically relative to each other; it never runs when either node has a real tuple.

---

## 9. Testing vs. Validation Distinction

- **Mechanism Tests Exist**: The codebase includes unit and integration tests asserting that the implementation matches the declared algorithms:
  - `test_focus_scoring_scenarios` (`graph_store.rs:4994`)
  - `test_deadline_and_stakeholder_do_not_double_count_lateness` (`graph_store.rs:5094`)
  - `test_live_calibration_rescore_jolt_and_marking` (`graph_store.rs:5170`)
  - `test_urgency_propagation` (`graph_store.rs:5545`)
  - `test_compute_voi_term` (`graph_store.rs:6249`)
  - `tests/cli_default_ordering.rs`
  - Phase 2 (chain slack, unlock breadth, value lineage) constructed-graph tests, all in `src/graph_store.rs`'s test module, immediately after `test_urgency_propagation`:
    `test_chain_slack_relaxation_finds_true_minimum_across_path_lengths` (AC1), `test_urgency_first_path_bfs_defect_fixed` (AC2, the V9 regression test), `test_unlock_breadth_is_cost_of_delay_weighted_not_a_count` (AC3), `test_value_lineage_materially_differentiates_targets_by_standing_weight` (AC4, includes a `focus_cmp` rank-movement assertion), `test_sibling_contributors_to_same_target_are_independent_not_combined` (AC6), `test_stated_weight_out_of_scale_rejected_at_parse_time_not_defaulted` / `test_stated_weight_omitted_is_silently_zero_no_warning` (AC5), `test_standing_weight_out_of_range_rejected`, `test_criticality_never_enters_cost_of_delay` (AC7, executable companion to the grep-based verification).
- **Model Validation Does NOT Exist**: These tests verify only that *the code executes what the code specifies*. They do not constitute empirical validation, calibration against real user outcomes, or backtesting of queue throughput. Genuinely untested at the mechanism level are the `slack = 0` and `slack = 30` step boundaries and the ready comparator against live queues. Phase 2's `value_lineage_term` is likewise untested against live-corpus outcomes — no target has been priced yet, so there is nothing on the live PKB to measure (§2.9, §4.11).

---

## 10. Summary of Consumers by Measure

| Measure | Consumers in Codebase |
| --- | --- |
| `focus_score` | Primary sort in `list_tasks`, `get_task`, `focus_picks`, `pkb tasks` CLI, `pkb list` CLI. |
| `downstream_weight` | `compute_focus_scores` (tie-breaker), `compute_criticality`, `top_n_by_metric`, `get_network_metrics`, `list_tasks` ready table. |
| `criticality` | `top_n_by_metric`, `get_network_metrics`, `get_task` / `list_tasks` `signals: {}`, overwhelm dashboard (`focusBreakdown.ts`, `prepareGraphData.ts`). |
| `urgency` | `compute_focus_scores` (`cost_of_delay`), `focus_picks`, `get_task` / `list_tasks` `signals: {}`. |
| `chain_slack` | `compute_urgency` (internally, feeds `f(Slack)`), `get_task` / `list_tasks` `signals: {}`. |
| `unlock_breadth` | `compute_focus_scores` (`tie_breakers.unlock_breadth_x10` — tie-breaker only, not `cost_of_delay`), `get_task` / `list_tasks` `signals: {}`. |
| `value_lineage` | `compute_focus_scores` (`cost_of_delay`), `get_task` / `list_tasks` `signals: {}`. |
| `standing_weight` | Read by `compute_value_lineage`; elicited/written only via hand-edited frontmatter as of this phase (no MCP write-tool wiring — out of scope, elicitation session not yet run). |
| `voi_value` | `compute_focus_scores`, `get_task` / `list_tasks` `signals: {}`. |
| `uncertainty` | `compute_voi_term`, `get_task` / `list_tasks` `signals: {}`. |
| `effective_intent` | `intent_pressure` in `cost_of_delay` (§2.1), `focus_cmp` fallback tie-breaker (§8.4, unscored nodes only), `classify_tasks` ready sorting, `list_tasks` filter. |
| `scope` | `get_task` / `list_tasks` `signals: {}`. |
| `pagerank` | `compute_criticality`, `top_n_by_metric`, `get_network_metrics`. |
| `betweenness` | `top_n_by_metric`, `get_network_metrics`. |
| `degrees` (`in`/`out`) | `get_network_metrics`. |
