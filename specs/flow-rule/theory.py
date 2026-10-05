#!/usr/bin/env python3
"""Theory check (flow-rule.md section 4): every candidate rule on one worked example.

    python3 theory.py

The example (all nodes open unless stated):

    T   target, worth +1.0          H   target to avoid, worth -0.5
    X   serves T 1.0                A, B   each needed by X (1.0)      parallel prerequisites
    C   serves T 0.5, supports X 0.5                                   two routes, one source
    L1, L2  each serve T 0.4, support each other 0.5; F supports L1 0.5   reinforcing loop
    P   protects against H 0.8 (harms H)                               protection
    Q   serves T 0.6 and brings about H 0.6                            gain with a loss
    D   open decision: O1 (p 0.4) serves T 0.6, O2 (p 0.7) serves T 0.3; S settles D
"""

from __future__ import annotations

import flow


def example(loop_quantum: float = 0.5) -> flow.Graph:
    g = flow.Graph()
    for n in "X A B C L1 L2 F P Q D O1 O2 S".split():
        g.add(n)
    g.add("T", worth=1.0)
    g.add("H", worth=-0.5)
    g.link("X", "T", 1.0)
    g.link("A", "X", 1.0, label="needs")
    g.link("B", "X", 1.0, label="needs")
    g.link("C", "T", 0.5)
    g.link("C", "X", 0.5, label="supports")
    g.link("L1", "T", 0.4)
    g.link("L2", "T", 0.4)
    g.link("L1", "L2", loop_quantum, label="supports")
    g.link("L2", "L1", loop_quantum, label="supports")
    g.link("F", "L1", 0.5, label="supports")
    g.link("P", "H", 0.8, effect=flow.HARMS)
    g.link("Q", "T", 0.6)
    g.link("Q", "H", 0.6)
    g.link("O1", "T", 0.6)
    g.link("O2", "T", 0.3)
    g.link("O1", "D", 1.0, label="alternative", probability=0.4)
    g.link("O2", "D", 1.0, label="alternative", probability=0.7)
    g.link("S", "D", 1.0, label="settles")
    return g


# ── Candidate rules for the positive part (questions 1 and 2) ──────────────


def cut_loops(g: flow.Graph) -> dict:
    """Treat the graph as acyclic: drop every edge that closes a loop, then knock out."""
    loops = flow.on_loops(g)
    h = flow.Graph(state=g.state, worth=g.worth, edges=[
        e for e in g.edges if not (e.src in loops and e.dst in loops and (e.src, e.dst) > (e.dst, e.src))
    ])
    return knockout(h)


def share(g: flow.Graph) -> dict:
    """Share (Shapley-style efficiency; Leontief-normalised): a node's worth is split
    among its inputs in proportion to their quantum, so parts sum to the whole."""
    worth = {v: max(w, 0.0) for v, w in g.worth.items()}
    inc, _ = flow._index(g)
    for _ in range(200):
        new = {v: max(g.worth.get(v, 0.0), 0.0) for v in g.state}
        for v, es in inc.items():
            tot = sum(e.strength for e in es)
            for e in es:
                new[e.src] = new.get(e.src, 0.0) + worth.get(v, 0.0) * e.strength / tot
        worth = new
    return worth


def strongest_route(g: flow.Graph) -> dict:
    """Necessity, strongest route (max-product semiring): each source once, best path."""
    out = {}
    for u in g.state:
        rs = flow.routes(g, u)
        out[u] = sum(max(0.0, max(s for s, _ in r)) * g.worth[t] for t, r in rs.items() if g.worth[t] > 0)
    return out


def walk_sum(g: flow.Graph, terms: int = 200) -> dict:
    """Path analysis / Katz / Leontief inverse: sum over every walk of the product of strengths."""
    inc, _ = flow._index(g)
    value = {v: max(g.worth.get(v, 0.0), 0.0) for v in g.state}
    total = dict(value)
    for _ in range(terms):
        nxt = {v: 0.0 for v in g.state}
        for v, es in inc.items():
            for e in es:
                nxt[e.src] += value[v] * e.strength
        value = nxt
        for v in g.state:
            total[v] += value[v]
    return total


def knockout(g: flow.Graph) -> dict:
    """Necessity, what is lost if never done (Birnbaum importance at the completion point)."""
    return {u: r.gain for u, r in flow.worth_all(g).items()}


CANDIDATES = [("share", share), ("cut loops", cut_loops), ("strongest route", strongest_route), ("walk sum", walk_sum), ("knockout", knockout)]


def safe(f, g, node):
    try:
        return fmt(f(g)[node])
    except flow.SaturatedLoop:
        return "rejected"


def fmt(v: float) -> str:
    return "inf" if v > 1e6 else "%.3f" % v


def main() -> None:
    g = example()
    print("Q1 necessity or share, Q2 loops: gain on T per candidate")
    print("node   " + "  ".join("%16s" % n for n, _ in CANDIDATES))
    results = {n: f(g) for n, f in CANDIDATES}
    for u in "X A B C F L1 L2".split():
        print("%-6s " % u + "  ".join("%16s" % fmt(results[n][u]) for n, _ in CANDIDATES))
    for lq in (0.9, 1.0):
        strong = example(loop_quantum=lq)
        print("F with loop quantum %.1f: " % lq + ", ".join("%s %s" % (n, safe(f, strong, "F")) for n, f in CANDIDATES))
    for eps in (0.5, 0.05, 0.01):
        weak = example(loop_quantum=0.9)
        next(e for e in weak.edges if e.src == "F").quantum = eps
        print("F feeding at %.2f, loop 0.9: knockout %s, strongest route %s" % (eps, safe(knockout, weak, "F"), safe(strongest_route, weak, "F")))
    g2 = example()
    g2.add("NEW")
    g2.link("NEW", "T", 1.0)
    print("X after adding an unrelated contributor NEW to T: " + ", ".join("%s %s" % (n, fmt(f(g2)["X"])) for n, f in CANDIDATES))

    print("\nQ3 decisions: worth of S, which settles D")
    w = flow.worth_all(g)
    opts = [(w["O1"].gain, 0.4), (w["O2"].gain, 0.7)]
    print("options (worth, p): %s" % opts)
    print("no decision rule: 0.000")
    print("stake (max - min worth): %.3f" % (max(o[0] for o in opts) - min(o[0] for o in opts)))
    print("EVPI (Howard): %.3f  [informed %.3f - blind %.3f]" % (flow.evpi(opts), flow.expected_best(opts), max(p * v for v, p in opts)))
    decided = example()
    decided.state["D"] = flow.DONE
    print("EVPI once D is decided: %.3f" % flow.decision_worth(decided, flow.worth_all(decided)).get("S", 0.0))

    print("\nQ4 negative targets and signed edges: (gain, loss averted)")
    for u in "P Q X".split():
        print("%s two columns: (%.3f, %.3f)   one net figure: %.3f" % (u, w[u].gain, w[u].loss_averted, w[u].gain + w[u].loss_averted))
    occ = example()
    occ.add("P2", flow.DONE)
    occ.link("P2", "H", 1.0, effect=flow.HARMS)
    print("P with a second, done, full protection P2: avoidance form %.3f" % flow.worth_all(occ)["P"].loss_averted)
    # Occurrence form: harm happens unless prevented; preventers combine as either-suffices.
    # It has no term for causes, so Q is undefined; X has no link to H.
    print("P alone under the occurrence form: %.3f" % (0.5 * (1.0 - (1 - 0.8))))
    harm_left_without_p = 1.0 * (1 - 1.0)  # P2 already prevents it all
    harm_left_with_p = (1 - 0.8) * (1 - 1.0)
    print("P with P2 under the occurrence form: %.3f" % (0.5 * (harm_left_without_p - harm_left_with_p)))


if __name__ == "__main__":
    main()
