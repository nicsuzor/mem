---
id: ranking
title: "PKB Ranking: Ordering, Classification and Graph Diagnostics"
type: spec
status: settled
created: 2026-08-25
settled_at: 2026-10-12
tags:
  - ranking
  - flow-rule
  - graph-metrics
  - spec
  - pkb
---

# PKB Ranking: Ordering, Classification and Graph Diagnostics

`mem` has one ranking engine, the flow engine. The rule itself (gain, loss averted, decision value, worth, links and their quanta) is specified in [`flow-rule.md`](flow-rule.md); what the server stores, computes and returns for it is specified in [`pkb-flow-engine.md`](pkb-flow-engine.md). This document holds the parts of ranking that sit around the rule: the build pipeline, rebuild determinism, the ready/blocked/roots predicates, the single canonical comparator, the derived display fields, and the standing doctrine on graph centrality.

Per `.agents/CORE.md`, this specification documents approved current state only.

---

## 1. Build pipeline

`GraphStore::build_internal` (`src/graph_store.rs`) runs, in order:

```text
canonical node sort (node id ASC)
  → ghost-node discovery (referenced ids sorted, then nodes re-sorted)
  → build_node_edges, edge dedup
  → compute_inverses
  → compute_degree_metrics
  → compute_centrality_metrics    (skipped on incremental rebuilds)
  → compute_scope
  → compute_uncertainty
  → compute_project_field
  → compute_target_ancestors
  → [similarity edges, if requested]
  → find_reachable_set
  → build_flow_input → flow::compute_flow → flow::compute_decision_value
  → display_rank::classify_graph_nodes   (ready / blocked / roots)
  → compute_divergence_anomalies
  → build_resolution_map
```

The flow figures on each node (`GraphNode.flow`, a `FlowOutput`) come only from `links`, target `worth` and node status (`pkb-flow-engine.md` §3, §5). Nothing else in the pipeline feeds them.

### 1.1. Canonical node ordering and rebuild determinism

- **Canonical sort order.** The node collection is sorted by node `id` at entry and again after ghost-node discovery (whose referenced ids are themselves sorted first).
- **Invariance.** Two builds or rebuilds from byte-identical input produce identical flow figures and classification across every build entry point (`build`, `build_from_directory`, `rebuild_from_nodes`, `rebuild_from_nodes_fast`, `rebuild_from_nodes_fast_with_embeddings`, `rebuild_from_nodes_skip_similarity`). `test_rebuild_determinism_across_entry_points` asserts this on the serialised `FlowOutput`.
- **Precedent.** The same discipline governs `build_resolution_map` (ties broken by node id, never scan order) and `tarjan_scc`.

---

## 2. Task classification

`display_rank::classify_graph_nodes` adapts each `GraphNode` to a `DisplayTask` and partitions tasks into `ready`, `blocked` and `roots` (`pkb-flow-engine.md` R20). Classification is restricted to `ACTIONABLE_TYPES` (`["task", "learn", "pr"]`; `epic` and `project` read-coerce to `task`). `GraphStore::reclassify` reruns it after an in-place patch.

### 2.1. Ready predicate

A task is `ready` if and only if:

1. it is a leaf (`leaf == true`, no children);
2. its type is in `CLAIMABLE_TYPES` (`["task"]`);
3. its status is actionable: `ready` or `queued`, or `inbox` with acceptance criteria (`is_ready_status`);
4. it is not blocked (§2.2).

### 2.2. Blocked predicate

An actionable, uncompleted task is `blocked` if it has at least one `depends_on` (or mapped `needs`) prerequisite that is not completed, or it is reachable downstream of a blocked task along `blocks`. A bare `status: blocked` does not by itself make a task blocked (E16).

### 2.3. Roots predicate

Roots are the uncompleted actionable tasks with no parent, or whose parent is not in the node map. Root-level tasks need no container.

---

## 3. The canonical comparator

`display_rank::display_cmp` is the single ordering of work across the server (`pkb-flow-engine.md` R19):

1. the cliff lane first (`deadline_class: hard` and days left ≤ effort days + `CLIFF_BUFFER_DAYS`), by `due` ascending;
2. then nodes that carry figures, by `gain + loss_averted` descending, ties broken by `gain`, then `loss_averted`, then `decision_value`, each descending;
3. then nodes with no figure: those with `flow_status ≠ ok` before those whose figures are all zero;
4. then `id` ascending, so the order is total and deterministic.

`display_cmp_by` applies an explicit `sort` key instead (`default`, `gain`, `loss_averted`, `decision_value`, `gain_per_effort`, `loss_averted_per_effort`, `due`, `id`).

Every ordered surface goes through it, via `GraphStore::sort_by_display` / `sort_by_display_with_sort`:

- `list_tasks` (all views and formats) and `nested_tasks` siblings (`graph_display::sort_siblings`);
- the `ready`, `blocked` and `roots` lists, `all_tasks` and `all_nodes_with_id`;
- `pkb tasks` (flat and tree) and `pkb focus`;
- focus picks (`GraphStore::focus_picks`): the cliff lane, then the head of `display_cmp` over ready work;
- `display_rank` (1..N) on `export_graph` nodes and in the MCP index (`pkb graph --format mcp-index`).

The CLI and MCP surfaces therefore agree for the same query by construction; `tests/cli_default_ordering.rs` checks the parity.

---

## 4. Derived display fields

These are computed during the build and shown by tools. None of them enters `flow.rs` or `display_cmp`.

| Field | Computation | Read by |
|---|---|---|
| `scope` | Recursive count of distinct descendants through parent-child edges (cycle-protected). | Graph JSON. |
| `uncertainty` | `clamp(1 − confidence, 0, 1)` when `confidence` is set; else 0.50 when the body carries a genuine open question (`detect_open_question`); else 0.0. Missing acceptance criteria, body length and child count feed nothing. | The do-next draw's exploration weight (`display_rank::compute_draw_distribution`). |
| `target_ancestors` | Ancestors reachable through `parent`, `contributes_to` and `closes` edges. | Graph JSON; typed facts. |
| `pagerank` | Power iteration, 20 iterations, damping 0.85 (`src/metrics.rs`). | `top_n_by_metric`, `get_network_metrics`, `pkb metrics`. |
| `betweenness` | Exact Brandes over all edges, undirected normalisation. | As `pagerank`. |
| `indegree`, `outdegree` | Edge counts over all edge types. | As `pagerank`. |

---

## 5. Standing doctrine on graph metrics

> `pagerank`, `betweenness` and the degree counts **never enter the flow computation or `display_cmp`**. They stay as derived diagnostics for the surfaces that show them.

1. **Incommensurable units.** Centrality measures are unitless and structural over heterogeneous edges. They have no exchange rate with worth.
2. **Redundant causal information.** What matters about structure is already carried in the rule's own units: what a node serves, and what it needs, through its links.
3. **Authoring artefacts.** On a personal knowledge graph, high centrality often reflects authoring habits or incomplete decomposition rather than value. Centrality is a **gardening lens**: it finds decomposition smells, promotion candidates and structural hubs. It is not a rank.

---

## 6. Mechanism tests are not model validation

The test suite checks that the code does what this spec and `pkb-flow-engine.md` say: the flow maths (`tests/flow_maths.rs`), the display layer (`tests/display_rank.rs`), CLI/MCP parity (`tests/cli_default_ordering.rs`) and the export contract (`tests/export_graph_tool_test.rs`). These tests verify mechanism only. They are not empirical validation of the ranking against real outcomes, and they do not calibrate it.
