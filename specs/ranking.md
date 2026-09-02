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

This document is the canonical specification for task prioritisation, focus scoring, graph centrality metrics, and queue ordering in `mem`. It describes the ranking machinery **as it actually ships at commit `d187656ca69e16b85d75b1726ec031d43429883a`**.

Per `.agents/CORE.md`, this specification documents approved current state only.

---

## 1. Overview of the Scoring Architecture & Sort Tuple

The PKB prioritisation model ranks every active, uncompleted task (`status ∉ {"done", "cancelled"}`) using an explicit, ordered sort tuple under a derived ordering:

```text
focus_tuple = (severity_gate, deadline_band, cost_of_delay, tie_breakers)
```

The sort tuple replaces the hand-built positional encoding of the earlier eight-term additive accumulator. It maintains the strict "one signal" invariant — the tuple **is** the single sort key across all ranking surfaces (`GraphStore::focus_cmp`, `list_tasks`, and the CLI). A synthetic display number — **`focus_score`** — is derived from the tuple for human-facing output and backwards-compatibility, but is demonstrably not a sort input.

Ranking in `mem` computes the sort tuple in `GraphStore::compute_focus_scores` (`src/graph_store.rs:1689`), executed at the end of the graph build pipeline:

```text
canonical node sort (node id ASC)
  → compute_inverses
  → compute_project_field / compute_target_ancestors
  → compute_cone_base_weights
  → compute_downstream_metrics
  → compute_effective_priority
  → compute_degree_metrics / compute_centrality_metrics
  → compute_criticality
  → compute_scope
  → compute_uncertainty
  → compute_voi_term
  → compute_blocking_urgency
  → compute_urgency              (also populates chain_slack — §4.3a)
  → compute_value_lineage        (§4.11, Phase 2)
  → compute_unlock_breadth       (§4.10, Phase 2)
  → compute_focus_scores
  → classify_tasks
```

`compute_value_lineage` and `compute_unlock_breadth` are both pure, single-hop, per-node scans over fields already finalised earlier in the pipeline (`resolved_to`, `urgency`, `voi_value`) — neither is a new whole-graph BFS/cone-walk mechanism alongside the two that already exist (the downstream cone walk in `compute_downstream_metrics`/`compute_voi_term`, and the urgency-propagation relaxation in `compute_urgency`).

### 1.1. Canonical Node Ordering & Rebuild Determinism

To guarantee that graph rebuilds are strictly deterministic and immune to Rust's randomized `HashMap` iteration order or non-deterministic file scan order:
- **Canonical Sort Order**: In `GraphStore::build_internal`, the node collection is canonically sorted by node `id` (`nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id))`) at entry and immediately following ghost node discovery (where referenced IDs are also sorted).
- **Invariance Property**: Two builds or rebuilds from byte-identical input produce byte-identical derived scores (`downstream_weight`, `criticality`, `focus_score`, `voi_value`, `urgency`, `effective_priority`) across all build entry points (`build`, `build_from_directory`, `rebuild_from_nodes`, `rebuild_from_nodes_fast`, `rebuild_from_nodes_fast_with_embeddings`, `rebuild_from_nodes_skip_similarity`).
- **Precedence**: Follows established precedents in `build_resolution_map` (`src/graph_store.rs:3622-3635`) and `tarjan_scc` (`src/graph_store.rs:3691`).


---

## 2. `focus_tuple` and `focus_score` Components

Ranking is produced by the explicit 4-component tuple `FocusTuple`:

1. **`severity_gate`**: Non-linear catastrophic obligation override (`Catastrophic` for SEV4-committed, else `Normal`).
2. **`deadline_band`**: Discrete calendar float band (`Overdue` > `Imminent` > `Urgent` > `Approaching` > `None`).
3. **`cost_of_delay`**: Commensurable dynamic pressure combining `priority_base`, fine-grained `deadline_score`, `stakeholder_waiting`, `urgency_term`, `voi_term`, and (Phase 2) `value_lineage_term`.
4. **`tie_breakers`**: Deterministic tie-breaking signals (`downstream_weight × 10`, (Phase 2) `unlock_breadth × 10`, `age_staleness_bonus`, `effective_priority`, `order`, `id`).

```
+---------------------------------------------------------------------------------------------------+
| Term / Component           | Formula / Logic                                   | Shipped Range    |
+---------------------------------------------------------------------------------------------------+
| 1. priority_base           | match pri { 0 => 10000, 1 => 5000, _ => 0 }       | 0 – 10,000       |
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

Term 10, `value_lineage_term`, is the fix for the failure terms 5/11 cannot be: `downstream_weight × 10` and `unlock_breadth × 10` are both deliberately capped, shallow, tie-breaker-only signals (§4.1, §4.10), so the sanctioned importance channel (`contributes_to`) needed a term that lives inside `cost_of_delay` itself, at comparable magnitude to `priority_base`/`urgency_term`, to be able to move a ranking at all (§3, §4.11).

### 2.1. Priority Base (`priority_base`)
- **Code reference**: `src/graph_store.rs:1699–1703`
- **Formula**:
  ```rust
  let pri = node.priority.unwrap_or(4);
  let score = match pri {
      0 => 10000,
      1 => 5000,
      _ => 0,
  };
  ```
- **Theoretical Range**: `0`, `5000`, or `10000`.
- **Observed Range**: `0` for >97% of tasks (P0 is rare/transient).
- **Default (absent input)**: `0` (defaults to P4 / P3).
- **Zeroing conditions**: `priority >= 2` or unset.
- **Consumers**: `compute_focus_scores`.

### 2.2. Severity Bonus (`severity_bonus`)
- **Code reference**: `src/graph_store.rs:1710–1716`
- **Formula**:
  ```rust
  let sev = node.severity.unwrap_or(0);
  score += match sev {
      4 => 100000,
      3 => 20000,
      2 => 10000,
      1 => 5000,
      _ => 0,
  };
  ```
- **Theoretical Range**: `0` to `100,000`.
- **Observed Range**: `0` on standard work tasks; `10,000`–`100,000` on targets or tasks carrying explicit severity.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: `severity` is `0` or unset.
- **Consumers**: `compute_focus_scores`.

### 2.3. Deadline Urgency Ramp (`deadline_score`)
- **Code reference**: `src/graph_store.rs:1717–1753`
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

### 2.4. Age / Staleness Bonus (`age_staleness_bonus`)
- **Code reference**: `src/graph_store.rs:1754–1766`
- **Formula**:
  Only applies if `priority >= 2`:
  $$\text{age\_bonus} = \min(\max(\text{days\_since\_created}, 0), 200)$$
- **Theoretical Range**: `0` to `200`.
- **Observed Range**: `0` to `200`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: `priority < 2` (P0 and P1 bypass staleness ramp) or `created` date missing/unparseable.
- **Consumers**: `compute_focus_scores`.

### 2.5. Downstream Weight Term (`(downstream_weight * 10.0) as i64`)
- **Code reference**: `src/graph_store.rs:1767`
- **Formula**:
  $$\text{term} = \lfloor \text{node.downstream\_weight} \times 10.0 \rfloor$$
- **Theoretical Range**: `0` to $\infty$.
- **Observed Range**: `0` to `~500`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: Empty downstream cone or all descendants completed.
- **Consumers**: `compute_focus_scores`.

### 2.6. Stakeholder & Human-Gate Waiting Urgency (`stakeholder_waiting_bonus`)
- **Code reference**: `src/graph_store.rs:1767–1798`
- **Formula**:
  Applies when `node.stakeholder.is_some()` or `node.is_human_gate()`:
  - If `deadline_ramp_fired` is `true`:
    $$\text{score} += 2000 \quad \text{(base only, suppressing per-day lateness growth to prevent double-counting)}$$
  - Else (`deadline_ramp_fired` is `false`):
    Let $\text{anchor} = \text{parse}(\text{node.waiting\_since} \lor \text{node.created})$, and $\text{days} = \max((\text{today} - \text{anchor}).\text{num\_days}(), 0)$:
    $$\text{score} += 2000 + \min(\text{days} \times 200, 6000) \quad \in [2000, 8000]$$
    If anchor date is missing or unparseable, score is `2000`.
- **Theoretical Range**: `0`, or `2000` to `8000`.
- **Observed Range**: `0` (unset) or `2000`–`8000`.
- **Default (absent input)**: `0`.
- **Zeroing conditions**: `stakeholder` field is absent (`None`) and `node.is_human_gate()` is `false`.
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
- **Formula**: $\text{term} = \text{round}(\text{node.value\_lineage})$ as `i64`. See §4.11 for how `node.value_lineage` itself is computed.
- **Theoretical Range**: `0` to `10,000` (a Critical/1.00 standing weight × a Certain/1.00 edge × full/1.0 confidence).
- **Observed Range**: `0` for every node as of this phase landing — no target has been priced yet (`pkb-standing-weight-elicitation-instrument` §"Current state"). Elicited standing weights are not a prerequisite for this mechanism to exist; they are a prerequisite for it to produce a nonzero number on the live PKB.
- **Default (absent input)**: `0` — a contributor with no `contributes_to` edge to a priced committed target, or a target with no `standing_weight` elicited, contributes nothing. No inference, no default weight (Zero Defaults / Zero Inference).
- **Consumers**: `compute_focus_scores`.

---

## 3. Disparity Between Theoretical Caps and Observed Ranges

The eight additive terms carry widely disparate theoretical caps versus realised empirical dynamics. This was the diagnosed failure Phase 2 exists to fix for the graph/importance channel specifically: `downstream_weight × 10` (a tie-breaker, not `cost_of_delay`) contributed at most ~53 observed points against a scale running to ~11,000, while `contributes_to` — the sanctioned channel by which agents are instructed to express importance instead of setting priority bands — reached the score only through that same ~53-point term. `value_lineage_term` (§2.9, §4.11) is the repair: it enters `cost_of_delay` directly, at the same 0–10,000 order of magnitude as `priority_base`/`urgency_term`, so it can actually move a ranking.

| Term | Theoretical Range | Observed Range (Typical) |
| --- | --- | --- |
| `severity_bonus` | 0 – 100,000 | 0 – 100,000 |
| `deadline_score` | 0 – 12,000 | 0 – 12,000 |
| `priority_base` | 0 – 10,000 | 0 – 10,000 |
| `urgency_term` | 0 – 10,000 | 0 – 10,000 |
| `value_lineage_term` | 0 – 10,000 | 0 (unpriced — §2.9) |
| `stakeholder_waiting` | 0 – 8,000 | 0 – 8,000 |
| `voi_term` | 0 – 5,000 | 0 – ~1,500 |
| `age_staleness_bonus` | 0 – 200 | 0 – 200 |
| `downstream_weight × 10` (tie-breaker) | 0 – $\infty$ | 0 – ~500 |
| `unlock_breadth × 10` (tie-breaker) | 0 – $\infty$ | not yet measured on the live PKB |
| **`focus_score` Composite** | **0 – ~155,200** | **0 – ~11,000+** |

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
- **Code reference**: `src/graph_store.rs:3074–3222`
- **Formula**:
  $$\\text{urgency}(x) = S_{\\text{lex}}(x) \\times f(\\text{Slack}(x))$$
  - Base Severity Score $S_{\\text{lex}}$:
    - If `severity == 4` and `goal_type == "committed"`: $S_{\\text{lex}} = 10000.0$ (lexicographic override).
    - Else: $S_{\\text{lex}} = 10^{\\min(\\text{severity}, 3)}$ (e.g. SEV0 $\\to 1$, SEV1 $\\to 10$, SEV2 $\\to 100$, SEV3 $\\to 1000$).
  - Slack Calculation: $\\text{slack}(x) = (\\text{due} - \\text{today}).\\text{num\\_days}() - \\text{effort\\_days}$. Default unconstrained slack is $100.0$ days.
  - Urgency Propagation: Urgency propagates backward from blocked tasks to blockers **by relaxation** (Phase 2; previously a single-pass BFS — see 4.3a) over paths up to depth 20:
    - `blocks`: factor $1.0$
    - `soft_blocks`: factor $0.3$
    - `children` (parent $\\to$ child): factor $0.5$
    - `contributes_to`: verbal weight anchor ($0.00$ to $1.00$)
    Propagated values: $\\text{propagated\\_s\\_lex}(x) = \\max(S_{\\text{lex}}(x), \\max_{\\text{paths}} S_{\\text{lex}}(t) \\times \\text{path\\_factor})$, and $\\text{min\\_slack}(x) = \\min(\\text{slack}(x), \\min_{t} \\text{slack}(t))$.
  - Piecewise-Exponential Slack Function $f(\\text{Slack})$ ($\\text{SAFE\\_HORIZON} = 30.0$, $k = \\frac{\\ln(10)}{30.0}$):
    $$f(\\text{slack}) = \\begin{cases} 0.001 & \\text{if } \\text{slack} > 30.0 \\\\ e^{k(30.0 - \\text{slack})} & \\text{if } 0.0 < \\text{slack} \\le 30.0 \\\\ 10.0 & \\text{if } \\text{slack} \\le 0.0 \\end{cases}$$
  - Guard: If committed SEV4 and $\\text{slack} \\le 0.0$, urgency is clamped to exactly $10000.0$.
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

### 4.6. `effective_priority`
- **Code reference**: `src/graph_store.rs:2815–2870`
- **Formula**:
  $$\\text{effective\\_priority}(x) = \\min(\\text{own\\_priority}(x), \\min_{t \\in \\text{downstream\\_cone}(x)} \\text{priority}(t))$$
  where downstream cone traverses `blocks`, `soft_blocks`, `children`, and `contributes_to` (skipping completed nodes).
- **Theoretical Range**: `0` (P0) to `4` (P4). Default: `4`.
- **Consumers**: Secondary tie-breaker in `focus_cmp`, priority filtering in `list_tasks`.

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
- **Theoretical Range**: $[0.0, \infty)$ — same reason `downstream_weight` is unbounded (§4.1); in practice bounded by the number and cost-of-delay of a node's direct dependents.
- **Consumers**: `compute_focus_scores` (`tie_breakers.unlock_breadth_x10` — a tie-breaker, **not** `cost_of_delay`; see the note at the end of §3), `get_task` / `list_tasks` signals.

### 4.11. `value_lineage` — Phase 2
- **Code reference**: `compute_value_lineage`, `src/graph_store.rs`.
- **Definition**: Standing weight elicited on a committed target/goal node, flowing multiplicatively to this node via its own `contributes_to` edges. This is the mechanism the doctrine in §7 and the parent plan's "Nic prices the destinations; the system prices the routes" require.
- **Formula**:
  $$\text{value\_lineage}(x) = K_{\text{VL}} \times \text{confidence}(x) \times \sum_{\substack{ct \,\in\, x.\text{contributes\_to} \\ \text{goal\_type}(ct.\text{target}) = \text{committed}}} ct.\text{numeric\_weight}() \times \text{standing\_weight}(ct.\text{target})$$
  where $K_{\text{VL}} = 10{,}000$ (§2.9), $\text{confidence}(x)$ defaults to $1.0$ when unset (mirrors `compute_uncertainty`'s existing "missing confidence, no open question ⇒ certain" default — a default on the *contributor's* stated confidence, never on the target's `standing_weight`, which is strictly `None`-means-zero), and a target contributes nothing to the sum unless it both carries `goal_type: committed` and has a priced `standing_weight` (`pkb-standing-weight-elicitation-instrument` §1 scope guard; "Zero Defaults / Zero Inference").
- **One hop only**: this walks a node's own `contributes_to` edges directly. It does not chain transitively through a contributor's own further `contributes_to` edges — a contributor of a contributor of a priced target is not credited unless it also has its own direct edge to a priced target. This keeps the computation a local per-node scan, not a new cone-walk mechanism ("no fourth graph computation").
- **Sibling-contributor combination semantics (settled)**: independent and additive. Multiple nodes contributing to the same target are each scored off their own edge alone — nothing reduces a contributor's credit because other contributors also point at the same target. This is a deliberate rejection of a Birnbaum-style reliability combination (cut sets, structure functions) across sibling edges, consistent with the parent plan's "Explicitly not building: Birnbaum importance proper" and §7's existing disclaimer that the verbal scale computes no such thing. See §7 for the corrected gloss this setting fixes.
- **Theoretical Range**: `0` to `10,000` per contributing edge (§2.9); a node with edges to multiple priced targets sums across them.
- **Observed Range**: `0` for every node as of this phase landing — no target has been priced yet.
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

1. **`severity_bonus` in `compute_focus_scores`**:
   - `SEV4` = `100,000` (lexicographic bonus)
   - `SEV3` = `20,000`
   - `SEV2` = `10,000`
   - `SEV1` = `5,000`
   - `SEV0` / unset = `0`
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
All flat task listings in MCP (`list_tasks`) and CLI (`pkb tasks`, `pkb list`) use the single canonical comparator `GraphStore::focus_cmp` (`src/graph_store.rs:895–906`):
1. `focus_score` **DESC** (unscored nodes sort last)
2. `effective_priority` **ASC** (P0 before P1 before P2)
3. `order` **ASC** (manual sequence order)
4. `id` **ASC** (guarantees a deterministic, total order)

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
| `effective_priority` | `focus_cmp` tie-breaker, `classify_tasks` ready sorting, `list_tasks` filter. |
| `scope` | `get_task` / `list_tasks` `signals: {}`. |
| `pagerank` | `compute_criticality`, `top_n_by_metric`, `get_network_metrics`. |
| `betweenness` | `top_n_by_metric`, `get_network_metrics`. |
| `degrees` (`in`/`out`) | `get_network_metrics`. |
