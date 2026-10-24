#!/usr/bin/env python3
"""Generate the synthetic flow-rule graph fixture.

This script constructs a completely synthetic graph fixture containing:
- No IDs, edge topologies, prices, or effort values taken from any personal PKB.
- Predictable subgraphs demonstrating all seventeen invariants of flow-rule.md.
- A background hierarchy of tasks to exercise scaling (>1,000 open nodes, >1,000 flow edges).

Outputs:
  fixtures/synthetic.json
"""

from __future__ import annotations

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_PATH = os.path.join(HERE, "fixtures", "synthetic.json")


def build_synthetic_graph() -> dict:
    nodes = []
    edges = []

    # ── 1. Strategic Targets (clean synthetic prices) ──────────────────────────
    # syn_targ_alpha: 0.50
    # syn_targ_beta:  0.30
    # syn_targ_gamma: 0.40
    # syn_targ_loop:  0.25
    # syn_targ_shield: unpriced open target (0.0 / None)
    # Background targets: syn_targ_bg_01 .. syn_targ_bg_10
    targets = [
        {"id": "syn_targ_alpha", "state": "open", "node_type": "target", "worth": 0.50},
        {"id": "syn_targ_beta", "state": "open", "node_type": "target", "worth": 0.30},
        {"id": "syn_targ_gamma", "state": "open", "node_type": "target", "worth": 0.40},
        {"id": "syn_targ_loop", "state": "open", "node_type": "target", "worth": 0.25},
        {"id": "syn_targ_shield", "state": "open", "node_type": "target"},
    ]
    for i in range(1, 11):
        targets.append({
            "id": f"syn_targ_bg_{i:02d}",
            "state": "open",
            "node_type": "target",
            "worth": 0.20 if i <= 2 else None,
        })
    nodes.extend(targets)

    # ── 2. Invariant 1 & 5: Parallel Prerequisites & Blocked Task ─────────────
    # syn_task_prereq_head serves syn_targ_alpha (0.50) and syn_targ_beta (0.30)
    # Total worth = 0.80.
    # syn_task_pre_1 and syn_task_pre_2 are open parallel prerequisites.
    nodes.append({"id": "syn_task_prereq_head", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_pre_1", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_pre_2", "state": "open", "node_type": "task"})
    edges.append(["syn_task_prereq_head", "syn_targ_alpha", "serves", 1.0])
    edges.append(["syn_task_prereq_head", "syn_targ_beta", "serves", 1.0])
    edges.append(["syn_task_pre_1", "syn_task_prereq_head", "needs", 1.0])
    edges.append(["syn_task_pre_2", "syn_task_prereq_head", "needs", 1.0])

    # ── 3. Invariant 2: Split into Necessary Parts ────────────────────────────
    nodes.append({"id": "syn_task_split", "state": "open", "node_type": "task"})
    edges.append(["syn_task_split", "syn_targ_alpha", "serves", 0.8])

    # ── 4. Invariant 3, Invariant 13 Case 6, Invariant 14: Multi-Route ─────────
    # syn_task_multi_route serves syn_targ_alpha directly (1.0) and via container (0.5).
    # Also serves syn_targ_beta directly (1.0).
    # Delta syn_targ_alpha = 1.0 (capped, not 1.5). Delta syn_targ_beta = 1.0.
    # Total gain = 0.50 * 1.0 + 0.30 * 1.0 = 0.80.
    nodes.append({"id": "syn_task_multi_parent", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_multi_route", "state": "open", "node_type": "task", "due": "2026-09-30", "effort": "2d"})
    edges.append(["syn_task_multi_route", "syn_targ_alpha", "serves", 1.0])
    edges.append(["syn_task_multi_route", "syn_task_multi_parent", "part_of", 1.0])
    edges.append(["syn_task_multi_parent", "syn_targ_alpha", "serves", 0.5])
    edges.append(["syn_task_multi_route", "syn_targ_beta", "serves", 1.0])
    # Also serves syn_targ_shield for Invariant 14
    edges.append(["syn_task_multi_route", "syn_targ_shield", "serves", 0.75])

    # ── 5. Invariant 4 & Invariant 16: Reinforcing & Harmful Loop ──────────────
    # Cycle: syn_loop_1 -> syn_loop_2 -> syn_loop_3 -> syn_loop_1
    # Loop product = 0.5 * 0.5 * 0.5 = 0.125 < 1.0.
    # syn_loop_2 serves syn_targ_loop (0.25) at 0.8.
    nodes.append({"id": "syn_loop_1", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_loop_2", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_loop_3", "state": "open", "node_type": "task"})
    edges.append(["syn_loop_1", "syn_loop_2", "supports", 0.5])
    edges.append(["syn_loop_2", "syn_loop_3", "supports", 0.5])
    edges.append(["syn_loop_3", "syn_loop_1", "serves", 0.5])
    edges.append(["syn_loop_2", "syn_targ_loop", "serves", 0.8])

    # ── 6. Invariant 7 & Invariant 13 Case 2: Unlinked Tasks ──────────────────
    nodes.append({"id": "syn_task_unlinked_1", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_unlinked_2", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_unlinked_3", "state": "open", "node_type": "task"})

    # ── 7. Invariant 8, Invariant 9, Invariant 13 Case 4: Last Open Step ──────
    # syn_task_parent_last serves syn_targ_gamma (0.40).
    # Has 3 done children and 1 open child (syn_task_child_last).
    nodes.append({"id": "syn_task_parent_last", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_child_done_1", "state": "done", "node_type": "task"})
    nodes.append({"id": "syn_task_child_done_2", "state": "done", "node_type": "task"})
    nodes.append({"id": "syn_task_child_done_3", "state": "done", "node_type": "task"})
    nodes.append({"id": "syn_task_child_last", "state": "open", "node_type": "task"})
    edges.append(["syn_task_parent_last", "syn_targ_gamma", "serves", 1.0])
    edges.append(["syn_task_child_done_1", "syn_task_parent_last", "part_of", 1.0])
    edges.append(["syn_task_child_done_2", "syn_task_parent_last", "part_of", 1.0])
    edges.append(["syn_task_child_done_3", "syn_task_parent_last", "part_of", 1.0])
    edges.append(["syn_task_child_last", "syn_task_parent_last", "part_of", 1.0])

    # ── 8. Invariant 10: Decision & EVPI ──────────────────────────────────────
    nodes.append({"id": "syn_task_decision", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_opt_a", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_opt_b", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_settles", "state": "open", "node_type": "task"})
    edges.append(["syn_task_decision", "syn_targ_gamma", "serves", 1.0])
    edges.append(["syn_task_opt_a", "syn_task_decision", "part_of", 1.0])
    edges.append(["syn_task_opt_b", "syn_task_decision", "part_of", 1.0])

    # ── 9. Invariant 12: Loop Reinforcement / Route Explanation ───────────────
    # Loop between syn_lex_a and syn_lex_b, serving syn_targ_gamma.
    # syn_task_loop_extra feeds into syn_lex_a.
    nodes.append({"id": "syn_task_loop_extra", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lex_a", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lex_b", "state": "open", "node_type": "task"})
    edges.append(["syn_task_loop_extra", "syn_lex_a", "serves", 0.5])
    edges.append(["syn_lex_a", "syn_lex_b", "needs", 1.0])
    edges.append(["syn_lex_b", "syn_lex_a", "serves", 0.5])
    edges.append(["syn_lex_a", "syn_targ_gamma", "serves", 0.75])

    # ── 10. Invariant 14: Protection Against Harm / Negative Target ───────────
    nodes.append({"id": "syn_task_safety_1", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_task_safety_2", "state": "open", "node_type": "task"})
    edges.append(["syn_task_safety_1", "syn_targ_shield", "serves", 0.75])
    edges.append(["syn_task_safety_2", "syn_targ_shield", "serves", 0.8])

    # ── 11. Invariant 17 & display_rank: Cliff Lane Deadlines ─────────────────
    # syn_task_hard_cliff: due 2026-10-05, effort None (default 3), hard class
    # syn_task_fake_cliff: due 2026-10-26, effort None, fake class
    # syn_task_soft_cliff: due 2026-11-21, effort 1h, soft class (worth 0.20 via syn_targ_bg_01)
    nodes.append({"id": "syn_task_hard_cliff", "state": "open", "node_type": "task", "due": "2026-10-05"})
    nodes.append({"id": "syn_task_fake_cliff", "state": "open", "node_type": "task", "due": "2026-10-26"})
    nodes.append({"id": "syn_task_soft_cliff", "state": "open", "node_type": "task", "due": "2026-11-21", "effort": "1h"})
    edges.append(["syn_task_soft_cliff", "syn_targ_bg_01", "serves", 1.0])

    # ── 12. Graph-Lint Examples Nodes ─────────────────────────────────────────
    nodes.append({"id": "syn_lint_unvalued_src", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lint_unvalued_dst", "state": "done", "node_type": "task"})
    edges.append(["syn_lint_unvalued_src", "syn_lint_unvalued_dst", "serves", None])

    nodes.append({"id": "syn_lint_gone_src", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lint_gone_dst", "state": "gone", "node_type": "task"})
    edges.append(["syn_lint_gone_src", "syn_lint_gone_dst", "supports", 0.5])

    nodes.append({"id": "syn_lint_multi_src", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lint_multi_dst", "state": "open", "node_type": "task"})
    edges.append(["syn_lint_multi_src", "syn_lint_multi_dst", "needs", 1.0])
    edges.append(["syn_lint_multi_src", "syn_lint_multi_dst", "supports", 0.5])

    nodes.append({"id": "syn_lint_loop_1", "state": "open", "node_type": "task"})
    nodes.append({"id": "syn_lint_loop_2", "state": "open", "node_type": "task"})
    edges.append(["syn_lint_loop_1", "syn_lint_loop_2", "supports", 0.5])
    edges.append(["syn_lint_loop_2", "syn_lint_loop_1", "supports", 0.5])

    nodes.append({"id": "syn_lint_reopen_node", "state": "done", "node_type": "task"})
    edges.append(["syn_lint_reopen_node", "syn_lint_loop_1", "part_of", 1.0])
    edges.append(["syn_lint_loop_1", "syn_lint_reopen_node", "needs", 1.0])

    # ── 13. Calibration Gone / Done nodes for Invariant 13 ────────────────────
    nodes.append({"id": "syn_task_gone_1", "state": "gone", "node_type": "task"})
    nodes.append({"id": "syn_task_done_1", "state": "done", "node_type": "task", "due": "2026-09-20", "effort": "1h"})
    nodes.append({"id": "syn_task_done_2", "state": "done", "node_type": "task"})

    # ── 14. Background Chain (> 1,000 open nodes, > 1,000 flow edges) ─────────
    NUM_BG_NODES = 1100
    for i in range(1, NUM_BG_NODES + 1):
        bg_id = f"syn_bg_task_{i:04d}"
        nodes.append({"id": bg_id, "state": "open", "node_type": "task"})
        if i > 1:
            # Chain link to previous node: creates 1,099 open flow edges
            edges.append([bg_id, f"syn_bg_task_{i-1:04d}", "supports", 0.1])
        # Occasional wikilinks for cost scenario C
        if i % 10 == 0 and i > 5:
            edges.append([bg_id, f"syn_bg_task_{i-5:04d}", "relates", None])

    # ── 15. Today's Rank ──────────────────────────────────────────────────────
    open_tasks = [n["id"] for n in nodes if n.get("state") == "open" and n.get("node_type") == "task"]
    today_rank = []
    priority_order = [
        "syn_task_multi_route",
        "syn_task_prereq_head",
        "syn_task_pre_1",
        "syn_task_pre_2",
        "syn_task_child_last",
        "syn_task_parent_last",
        "syn_loop_1",
        "syn_loop_2",
        "syn_task_hard_cliff",
        "syn_task_fake_cliff",
        "syn_task_soft_cliff",
        "syn_task_unlinked_1",
        "syn_task_unlinked_2",
    ]
    seen = set()
    score = 15000
    for tid in priority_order:
        if tid in open_tasks:
            today_rank.append([tid, score, score * 0.8])
            seen.add(tid)
            score -= 1000

    for tid in open_tasks:
        if tid not in seen:
            today_rank.append([tid, max(1, score), None])
            score = max(1, score - 10)

    nodes.sort(key=lambda n: n["id"])
    edges.sort(key=lambda e: (e[0], e[1], e[2]))

    return {
        "source": "synthetic graph generated by specs/flow-rule/generate_synthetic.py",
        "nodes": nodes,
        "edges": edges,
        "today_rank": today_rank,
    }


def main():
    graph_data = build_synthetic_graph()
    os.makedirs(os.path.dirname(OUT_PATH), exist_ok=True)
    with open(OUT_PATH, "w") as f:
        json.dump(graph_data, f, indent=1, sort_keys=True)
    print(f"Generated {len(graph_data['nodes'])} nodes, {len(graph_data['edges'])} edges -> {OUT_PATH}")


if __name__ == "__main__":
    main()
