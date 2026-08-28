---
id: ranking
title: "PKB Ranking & Prioritisation Specification"
type: spec
status: approved
created: 2026-08-25
pinned_commit: d187656ca69e16b85d75b1726ec031d43429883a
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
  → compute_urgency
  → compute_focus_scores
  → classify_tasks
```

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
3. **`cost_of_delay`**: Commensurable dynamic pressure combining `priority_base`, fine-grained `deadline_score`, `stakeholder_waiting`, `urgency_term`, and `voi_term`.
4. **`tie_breakers`**: Deterministic tie-breaking signals (`downstream_weight × 10`, `age_staleness_bonus`, `effective_priority`, `order`, `id`).

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
+---------------------------------------------------------------------------------------------------+
```

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
- **Zeroing conditions**: Non-leaf task (`leaf == false`), complete/cancelled status, no downstream dependents, or `dep_resolution_ratio == 0`.
- **Consumers**: `compute_focus_scores`.

---

## 3. Disparity Between Theoretical Caps and Observed Ranges

The eight additive terms carry widely disparate theoretical caps versus realised empirical dynamics:

| Term | Theoretical Range | Observed Range (Typical) |
| --- | --- | --- |
| `severity_bonus` | 0 – 100,000 | 0 – 100,000 |
| `deadline_score` | 0 – 12,000 | 0 – 12,000 |
| `priority_base` | 0 – 10,000 | 0 – 10,000 |
| `urgency_term` | 0 – 10,000 | 0 – 10,000 |
| `stakeholder_waiting` | 0 – 8,000 | 0 – 8,000 |
| `voi_term` | 0 – 5,000 | 0 – ~1,500 |
| `age_staleness_bonus` | 0 – 200 | 0 – 200 |
| `downstream_weight × 10` | 0 – $\infty$ | 0 – ~500 |
| **`focus_score` Composite** | **0 – ~145,200** | **0 – ~11,000+** |

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
  - Urgency Propagation: Urgency propagates backward from blocked tasks to blockers over BFS paths up to depth 20:
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

### 4.4. `voi_value` (Value of Information)
- **Code reference**: `src/graph_store.rs:2950–3030`
- **Formula**:
  Only computed for leaf tasks (`leaf == true`):
  $$\\text{VoI}(x) = \\min\\left(K_{\\text{VOI}}, K_{\\text{VOI}} \\times 1.0 \\times \\text{dep\\_resolution\\_ratio}(x) \\times \\frac{\\sum_{t \\in \\text{cone}_{\\text{Unblocking}}(x)} \\frac{1}{\\text{depth}(t)} \\cdot \\text{base\\_weight}(t) \\cdot \\text{edge\\_factor}(t) \\cdot \\text{uncertainty}(t)}{\\max(\\text{effort\\_days}, \\text{VOI\\_EFFORT\\_NEUTRAL\\_DAYS})}\\right)$$
  where:
  - $K_{\\text{VOI}} = 5000.0$
  - $\\text{VOI\\_EFFORT\\_NEUTRAL\\_DAYS} = 3.0$
  - $\\text{dep\\_resolution\\_ratio}(x) = \\frac{\\text{completed\\_deps}(x)}{\\text{total\\_deps}(x)}$ (or $1.0$ if no dependencies).
  - $\\text{cone}_{\\text{Unblocking}}(x)$ traverses `blocks`, `soft_blocks`, and parent $\\to$ child (excludes reverse `contributes_to`).
- **Theoretical Range**: `0.0` to `5000.0` (or `None` for non-leaf tasks).
- **Consumers**: `compute_focus_scores`, `get_task` / `list_tasks` signals.

### 4.5. `uncertainty`
- **Code reference**: `src/graph_store.rs:3265–3324`
- **Formula**:
  - If `confidence` is explicitly set: $\\text{uncertainty} = \\text{clamp}(1.0 - \\text{confidence}, 0.0, 1.0)$.
  - Else, composite heuristic:
    $$\\text{uncertainty} = \\text{clamp}(u_{\\text{ac}} + u_{\\text{children}} + u_{\\text{deps}} + u_{\\text{body}}, 0.0, 1.0)$$
    - $u_{\\text{ac}} = 0.30$ if `!has_acceptance_criteria`, else $0.0$
    - $u_{\\text{children}} = 0.15$ if `!children.is_empty()`, else $0.0$
    - $u_{\\text{deps}} = 0.25 \\times (1.0 - \\text{dep\\_resolution\\_ratio})$ if `!depends_on.is_empty()`, else $0.0$
    - $u_{\\text{body}} = 0.10 \\times \\left(1.0 - \\min\\left(\\frac{\\text{word\\_count}}{100.0}, 1.0\\right)\\right)$
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

---

## 5. Standing Doctrine on Graph Metrics

> **Standing Ruling (Phase 2):**
> `pagerank`, `betweenness`, and `criticality` **never enter any focus_score or ranking path** — but they stay as **derived diagnostics**, computed for the surfaces that already consume them (`top_n_by_metric`, `get_network_metrics`, `get_task`/`list_tasks` signals, the ready-view table, and the overwhelm dashboard).

This doctrine is grounded in four structural realities:

1. **Incommensurable Units:** PageRank, betweenness, and criticality are unitless structural measures over heterogeneous edges. They have no natural exchange rate with cost-of-delay or time urgency.
2. **Redundant Causal Information:** The actionable structural content of the graph is already captured directly in interpretable domain units — bottleneck urgency is captured by chain slack and LST; strategic importance is captured by target value lineage. Betweenness and PageRank are merely correlational shadows of those causal paths.
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
| Fifty-Fifty / Even Chance | `"fifty-fifty"`, `"even chance"` | **0.50** | Moderate contributor; redundancy exists |
| Uncertain / Possible | `"uncertain"`, `"possible"`, `"perhaps"`, `"maybe"` | **0.25** | Exploratory or optional contribution |
| Improbable / Unlikely | `"improbable"`, `"unlikely"`, `"very unlikely"` | **0.15** | Minor marginal contribution |
| Impossible / None | `"impossible"`, `"none"` | **0.00** | No contribution |
| *Unrecognized / Default* | *any other string or empty* | **0.30** | Default soft contribution |

> **Note on Naming:** This is an **elicitation scale** for human/agent calibration. It is **not** a computational Birnbaum structural reliability model (it computes no cut sets, partial derivatives, or multi-contributor Boolean structure functions). In all agent-facing schemas and documentation, it is referred to as the **verbal contribution-weight scale**.

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
- **Model Validation Does NOT Exist**: These tests verify only that *the code executes what the code specifies*. They do not constitute empirical validation, calibration against real user outcomes, or backtesting of queue throughput. Genuinely untested at the mechanism level are the `slack = 0` and `slack = 30` step boundaries and the ready comparator against live queues.

---

## 10. Summary of Consumers by Measure

| Measure | Consumers in Codebase |
| --- | --- |
| `focus_score` | Primary sort in `list_tasks`, `get_task`, `focus_picks`, `pkb tasks` CLI, `pkb list` CLI. |
| `downstream_weight` | `compute_focus_scores`, `compute_criticality`, `top_n_by_metric`, `get_network_metrics`, `list_tasks` ready table. |
| `criticality` | `top_n_by_metric`, `get_network_metrics`, `get_task` / `list_tasks` `signals: {}`, overwhelm dashboard (`focusBreakdown.ts`, `prepareGraphData.ts`). |
| `urgency` | `compute_focus_scores`, `focus_picks`, `get_task` / `list_tasks` `signals: {}`. |
| `voi_value` | `compute_focus_scores`, `get_task` / `list_tasks` `signals: {}`. |
| `uncertainty` | `compute_voi_term`, `get_task` / `list_tasks` `signals: {}`. |
| `effective_priority` | `focus_cmp` tie-breaker, `classify_tasks` ready sorting, `list_tasks` filter. |
| `scope` | `get_task` / `list_tasks` `signals: {}`. |
| `pagerank` | `compute_criticality`, `top_n_by_metric`, `get_network_metrics`. |
| `betweenness` | `top_n_by_metric`, `get_network_metrics`. |
| `degrees` (`in`/`out`) | `get_network_metrics`. |
