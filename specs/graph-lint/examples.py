#!/usr/bin/env python3
"""Reproduce the fixture-based examples and counts in graph-lint.md.

    python3 specs/graph-lint/examples.py

This is an evidence script, not the linter. It reads the committed live-graph
fixture of the flow-rule spec (flow-rule/fixtures/live-2026-10-05.json) through
the flow-rule reference calculator, and prints one block per check family.
Rows marked ASSUMED edit a real node hypothetically, because the live graph
does not yet carry the field being checked.
"""

from __future__ import annotations

import collections
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "flow-rule"))

import flow  # noqa: E402

FIXTURE = os.path.join(HERE, "..", "flow-rule", "fixtures", "live-2026-10-05.json")


STRATEGIC_TARGET_TYPES = {"target", "goal"}


def load():
    d = json.load(open(FIXTURE))
    return d, flow.from_export(d), {n["id"]: n for n in d["nodes"]}


def cycle_product(g: flow.Graph, comp: list[str]) -> float:
    """Largest product of edge strengths round any simple cycle in a small component."""
    out = collections.defaultdict(list)
    for e in g.flow_edges():
        if e.src in comp and e.dst in comp:
            out[e.src].append(e)
    best = 0.0

    def walk(start, v, seen, prod):
        nonlocal best
        for e in out[v]:
            if e.dst == start:
                best = max(best, prod * e.strength)
            elif e.dst not in seen:
                walk(start, e.dst, seen | {e.dst}, prod * e.strength)

    for s in comp:
        walk(s, s, {s}, 1.0)
    return best


def old_hard_cycles(g: flow.Graph) -> list[list[str]]:
    """Today's dep-hard-cycle adjacency (src/lint.rs:1627-1662): id -> parent and depends_on.

    In the fixture a part_of edge runs child -> parent (same direction as today's
    adjacency) and a needs edge runs dependency -> dependent (the reverse).
    """
    h = flow.Graph(state=g.state, worth={}, edges=[])
    for e in g.edges:
        if e.label == "part_of":
            h.link(e.src, e.dst, 1.0, label="part_of")
        elif e.label == "needs":
            h.link(e.dst, e.src, 1.0, label="needs")
    return flow._components(h)


def main() -> int:
    d, g, node = load()
    st = g.state
    typ = {k: v["node_type"] for k, v in node.items()}
    inc = collections.defaultdict(list)
    for e in g.edges:
        inc[e.dst].append(e)

    print("## Targets and worth")
    targets = sorted(v for v in st if typ[v] in STRATEGIC_TARGET_TYPES)
    unpriced = [t for t in targets if st[t] == "open" and t not in g.worth]
    print(f"open targets unpriced: {len(unpriced)} of {len(targets)}")
    fed = [(t, len(inc[t]), sum(st[e.src] == "open" for e in inc[t])) for t in unpriced if inc[t]]
    print(f"unpriced targets with incoming edges: {len(fed)}")
    for t, n, o in fed:
        if t == "targ_safety":
            print(f"  {t}: {n} incoming edges, {o} from open work")
    print(f"worth on a non-target: {sorted(v for v in g.worth if typ[v] not in STRATEGIC_TARGET_TYPES)}")

    print("\n## Edges")
    uv = [e for e in g.edges if e.unvalued]
    print(f"typed edges with no stated quantum: {len(uv)}")
    for e in uv:
        print(f"  {e.src} ({st[e.src]}) -> {e.dst} ({st[e.dst]}), label {e.label}")
    gone = [e for e in g.edges if st[e.src] == "open" and st.get(e.dst) == "gone"]
    print(f"edges from open work to a cancelled node: {len(gone)} "
          f"{dict(sorted(collections.Counter(e.label for e in gone).items()))}")
    for e in gone[:1]:
        print(f"  e.g. {e.src} -> {e.dst} ({e.label})")
    pairs = collections.defaultdict(list)
    for e in g.edges:
        pairs[(e.src, e.dst)].append(e.label)
    dup = {p: ls for p, ls in pairs.items() if len(ls) > 1}
    dup_open = {p: ls for p, ls in dup.items() if st[p[0]] == "open"}
    print(f"node pairs joined by more than one edge: {len(dup)}; from open work: {len(dup_open)}")
    for p, ls in sorted(dup_open.items()):
        print(f"  {p[0]} -> {p[1]}: {sorted(ls)}")
    print(f"self-edges: {sum(e.src == e.dst for e in g.edges)}")

    print("\n## Loops")
    loops = flow._components(g)
    print(f"loops of flow edges: {len(loops)}; saturated (error): {len(flow.saturated_loops(g))}")
    for comp in loops:
        states = {v: st[v] for v in comp}
        print(f"  {comp}: strongest cycle product {cycle_product(g, comp):.2f}; states {states}")
    old = old_hard_cycles(g)
    print(f"today's dep-hard-cycle rule on the same edges finds: {len(old)} cycles")
    # ASSUMED: reopen the done node of the child-depends-on-parent loop.
    g2 = flow.Graph(state=dict(st), worth=dict(g.worth), edges=list(g.edges))
    g2.state["n_22a3f9be31"] = "open"
    print(f"  ASSUMED n_22a3f9be31 reopened: saturated loops {flow.saturated_loops(g2)}; "
          f"today's rule still finds {len(old_hard_cycles(g2))}")

    print("\n## Deadlines")
    dues = sorted((v, n["due"], n.get("effort")) for v, n in node.items() if "due" in n and st[v] == "open")
    print(f"open nodes with a due date: {len(dues)}; with a deadline_class: 0 (field not yet in the graph)")
    print(f"  of which with no effort: {sum(1 for _, _, f in dues if not f)}")
    for v, due, eff in dues:
        print(f"  {v} due {due} effort {eff}")

    print("\n## Coverage")
    _, out = flow._index(g)
    open_nodes = [v for v in st if st[v] == "open"]
    unrouted = [v for v in open_nodes if not any(t in g.worth for t in flow._forward(out, v))]
    print(f"open nodes with no route to a priced target: {len(unrouted)} of {len(open_nodes)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
