"""Cost of the flow rule on the committed live fixture (pkb-flow-engine.md section 5.4).

Run: python3 specs/pkb-flow-engine/cost.py

Uses the reference calculator in specs/flow-rule/flow.py unchanged. It is a
checking aid for the cost figures in the spec, not the engine.

Scenarios:
  A  the fixture as committed: 7 of 26 targets priced.
  B  every target priced (assumed): unpriced targets set to +0.35, the
     "Substantial" anchor, so every cone that reaches a target is computed.
  C  as B, and every wikilink read as a flow edge at quantum 0.05 (assumed;
     flow-rule.md Q2 and Q23). This is the densest graph the open questions
     allow, and so the cost ceiling.

For each scenario it prints the graph size, the number of open nodes whose
forward cone reaches a priced target, the sum and maximum of those cones in
nodes and edges (the work the knockout rule does), the loops, and the wall
time of a full worth_all run (median of 5).

It also prints, for the recompute-on-write contract, the size of the upstream
set of each node: the nodes whose figures can change when that node's state,
worth or outgoing edges change (P4 in flow-rule.md section 3.2).
"""

from __future__ import annotations

import json
import os
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "flow-rule"))

import flow  # noqa: E402

FIXTURE = os.path.join(HERE, "..", "flow-rule", "fixtures", "live-2026-10-05.json")


def load(price_all: bool, wikilinks: bool = False) -> tuple[flow.Graph, dict]:
    data = json.load(open(FIXTURE))
    if wikilinks:
        data["edges"] = [e if e[2] != "relates" else [e[0], e[1], "wikilink", 0.05] for e in data["edges"]]
    if price_all:
        for n in data["nodes"]:
            if n.get("node_type") == "target" and n.get("worth") is None:
                n["worth"] = 0.35
    return flow.from_export(data), data


def pct(xs: list[int], p: float) -> int:
    if not xs:
        return 0
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p * (len(xs) - 1))))]


def cone_stats(g: flow.Graph) -> dict:
    inc, out = flow._index(g)
    open_nodes = [v for v, s in g.state.items() if s == flow.OPEN]
    sizes, edges = [], []
    for u in open_nodes:
        cone = flow._forward(out, u)
        if not any(t in g.worth for t in cone):
            continue
        cs = set(cone)
        sizes.append(len(cone))
        edges.append(sum(1 for v in cone for e in inc.get(v, []) if e.src in cs))
    return {
        "open": len(open_nodes),
        "with_priced_cone": len(sizes),
        "cone_nodes_sum": sum(sizes),
        "cone_nodes_max": max(sizes, default=0),
        "cone_edges_sum": sum(edges),
        "cone_edges_max": max(edges, default=0),
    }


def upstream_sizes(g: flow.Graph) -> list[int]:
    inc, _ = flow._index(g)
    rev = {v: [e.src for e in es] for v, es in inc.items()}
    sizes = []
    for v in g.state:
        seen, stack = {v}, [v]
        while stack:
            x = stack.pop()
            for w in rev.get(x, []):
                if w not in seen:
                    seen.add(w)
                    stack.append(w)
        sizes.append(len(seen) - 1)
    return sizes


def timed(g: flow.Graph, runs: int = 5) -> float:
    ts = []
    for _ in range(runs):
        t0 = time.perf_counter()
        flow.worth_all(g)
        ts.append(time.perf_counter() - t0)
    return statistics.median(ts)


def main() -> int:
    for name, price_all, wiki in (("A", False, False), ("B", True, False), ("C", True, True)):
        g, data = load(price_all, wiki)
        fe = [e for e in g.flow_edges()]
        loops = flow._components(g)
        cs = cone_stats(g)
        w = flow.worth_all(g)
        carrying = sum(1 for r in w.values() if r.gain or r.loss_averted)
        print(f"scenario {name}: nodes {len(g.state)}, stored edges {len(data['edges'])}, "
              f"flow edges {len(fe)}, priced targets {len(g.worth)}")
        print(f"  open nodes {cs['open']}; cone reaches a priced target {cs['with_priced_cone']}; "
              f"carrying worth {carrying}")
        print(f"  cone nodes sum {cs['cone_nodes_sum']}, max {cs['cone_nodes_max']}; "
              f"cone edges sum {cs['cone_edges_sum']}, max {cs['cone_edges_max']}")
        print(f"  loops {len(loops)}, sizes {sorted(len(c) for c in loops)}")
        runs = 1 if wiki else 5
        print(f"  worth_all median of {runs}: {timed(g, runs):.3f} s")
    for name, wiki in (("B", False), ("C", True)):
        g, _ = load(True, wiki)
        up = upstream_sizes(g)
        print(f"upstream set per node (scenario {name}): median {pct(up, 0.5)}, p95 {pct(up, 0.95)}, "
              f"p99 {pct(up, 0.99)}, max {max(up)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
