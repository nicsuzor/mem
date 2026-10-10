#!/usr/bin/env python3
"""Reference calculator for specs/flow-rule.md.

A specification aid, not the engine: it exists so every number in the spec can
be reproduced. Standard library only.

The rule (flow-rule.md section 3): a piece of work is worth what would be lost
if it were never done, assuming everything else gets done.

    y_v = d_v * prod_{e = (w -> v)} phi_e(y_w)
    phi_e(y) = 1 - s_e * (1 - y)     effect = helps
    phi_e(y) = 1 - s_e * y           effect = harms
    s_e = quantum_e * probability_e

y_v is how far v is realised; for a target Nic wants to avoid (worth < 0) it is
how far v is avoided, and every edge touching it has its effect read reversed.

    delta_t(u) = y_t(baseline) - y_t(u never done)
    gain(u)          = sum_{t : worth_t > 0} worth_t   * delta_t(u)
    loss_averted(u)  = sum_{t : worth_t < 0} |worth_t| * delta_t(u)

Usage:
    python3 flow.py live EXPORT.json [--out results.json]
"""

from __future__ import annotations

import json
import math
import sys
from dataclasses import dataclass, field

HELPS, HARMS = "helps", "harms"
OPEN, DONE, GONE = "open", "done", "gone"

# Labels that feed the decision rule (section 6) instead of the flow.
DECISION_LABELS = {"alternative", "settles"}

TOL = 1e-12


@dataclass
class Edge:
    src: str  # the work
    dst: str  # what it serves
    label: str = "serves"
    quantum: float = 0.0
    probability: float = 1.0
    effect: str = HELPS
    unvalued: bool = False  # no quantum stated: read at the default quantum

    @property
    def strength(self) -> float:
        return self.quantum * self.probability


@dataclass
class Graph:
    state: dict[str, str] = field(default_factory=dict)  # id -> open | done | gone
    worth: dict[str, float] = field(default_factory=dict)  # priced targets only
    edges: list[Edge] = field(default_factory=list)

    def add(self, node: str, state: str = OPEN, worth: float | None = None) -> None:
        self.state[node] = state
        if worth is not None:
            self.worth[node] = worth

    def link(self, src: str, dst: str, quantum: float, **kw) -> Edge:
        e = Edge(src, dst, quantum=quantum, **kw)
        self.edges.append(e)
        return e

    def flow_edges(self) -> list[Edge]:
        """Edges the flow reads: not decision labels, no gone endpoint, nonzero strength."""
        return [
            e
            for e in self.edges
            if e.label not in DECISION_LABELS
            and self.state.get(e.src) != GONE
            and self.state.get(e.dst) != GONE
            and e.strength > 0
        ]


class NoConvergence(RuntimeError):
    pass


class SaturatedLoop(ValueError):
    """A loop of open nodes whose every edge is a full-strength helps edge.

    Each node needs the next entirely, so any loss anywhere collapses the whole
    loop: a feeder of quantum 0.001 would carry the loop's full worth. The rule
    rejects such loops (flow-rule.md 3.2), as the linter already rejects cycles
    over hard dependencies and parents.
    """


def _helps(g: Graph, e: Edge) -> bool:
    """Effect read in avoidance terms at each end that is a target to avoid."""
    flips = (e.effect == HARMS) + (g.worth.get(e.src, 0) < 0) + (g.worth.get(e.dst, 0) < 0)
    return flips % 2 == 0


def _index(g: Graph):
    inc: dict[str, list[Edge]] = {}
    out: dict[str, list[Edge]] = {}
    for e in g.flow_edges():
        inc.setdefault(e.dst, []).append(e)
        out.setdefault(e.src, []).append(e)
    return inc, out


def _settle(g: Graph, inc, x: dict, nodes: list[str], knocked: str | None,
            active_harms: set[str] | None = None, loop_nodes: set[str] | None = None) -> dict:
    """Iterate y_v = d_v * prod phi_e(y_w) over `nodes` to a fixed point.

    Uses synchronous updates to ensure node-ID independence on mutual harm.
    Harms edges only fire if already DONE in reality, internal to a loop,
    or specifically active (active_harms).
    """
    if loop_nodes is None:
        loop_nodes = on_loops(g)
    harms_inside = any(not _helps(g, e) for v in nodes for e in inc.get(v, []))
    if harms_inside:
        # Synchronous (Jacobi) iteration with damping=0.5: node-ID independent on mutual harm
        damping = 0.5
        y = dict(x)
        for _ in range(20000):
            change = 0.0
            new_y = {}
            for v in nodes:
                if v == knocked:
                    new = 0.0
                elif g.state.get(v) == DONE or g.state.get(v) is None:
                    new = 1.0
                else:
                    new = 1.0
                    for e in inc.get(v, []):
                        if _helps(g, e):
                            yw = y.get(e.src, 1.0)
                            new *= 1.0 - e.strength * (1.0 - yw)
                        else:
                            src_active = (
                                (g.state.get(e.src) == DONE)
                                or (e.src in loop_nodes and e.dst in loop_nodes)
                                or (active_harms is None)
                                or (e.src in active_harms)
                            )
                            yw = y.get(e.src, 1.0) if src_active else 0.0
                            new *= 1.0 - e.strength * yw
                damped_new = y[v] + damping * (new - y[v])
                change = max(change, abs(damped_new - y[v]))
                new_y[v] = damped_new
            y = new_y
            if change < TOL:
                return y
        raise NoConvergence(f"no fixed point reached over {len(nodes)} nodes (knocked={knocked})")
    else:
        # Gauss-Seidel iteration: monotone systems (Tarski 1955), fast convergence
        y = dict(x)
        for _ in range(20000):
            change = 0.0
            for v in nodes:
                if v == knocked:
                    new = 0.0
                elif g.state.get(v) == DONE or g.state.get(v) is None:
                    new = 1.0
                else:
                    new = 1.0
                    for e in inc.get(v, []):
                        yw = y.get(e.src, 1.0)
                        new *= 1.0 - e.strength * (1.0 - yw)
                diff = abs(new - y[v])
                if diff > change:
                    change = diff
                y[v] = new
            if change < TOL:
                return y
        raise NoConvergence(f"no fixed point reached over {len(nodes)} nodes (knocked={knocked})")


def _forward(out, start: str) -> list[str]:
    seen, stack = {start}, [start]
    while stack:
        v = stack.pop()
        for e in out.get(v, []):
            if e.dst not in seen:
                seen.add(e.dst)
                stack.append(e.dst)
    return sorted(seen)


def baseline(g: Graph) -> dict:
    inc, _ = _index(g)
    nodes = sorted(g.state)
    loop_nodes = on_loops(g)
    return _settle(g, inc, {v: 1.0 for v in nodes}, nodes, None, active_harms=set(), loop_nodes=loop_nodes)


@dataclass
class Worth:
    gain: float | None
    loss_averted: float | None
    deltas: dict  # priced target -> delta
    flow_status: str = "ok"
    loop: list[str] | None = None


def saturated_loops(g: Graph) -> list[list[str]]:
    full = Graph(state=g.state, worth=g.worth, edges=[
        e for e in g.flow_edges()
        if e.strength >= 1.0 - 1e-12 and _helps(g, e) and g.state.get(e.src) == OPEN and g.state.get(e.dst) == OPEN
    ])
    return _components(full)


def worth_all(g: Graph, only: list[str] | None = None) -> dict[str, Worth]:
    """Gain and loss averted for every open node (or the listed ones).

    Guarantees:
    - Open harmful nodes do not rescale or zero unrelated work.
    - Gain and loss averted sit side by side and are never netted inside one column.
    - Mutual harm is node-ID independent.
    - Isolated loop failures do not abort the ranking of independent nodes (engine E3).
    """
    bad_loops = saturated_loops(g)
    bad_nodes = {v for comp in bad_loops for v in comp}
    inc, out = _index(g)
    loop_nodes = on_loops(g)
    base = baseline(g)
    result = {}
    for u in only if only is not None else sorted(g.state):
        if g.state.get(u) != OPEN:
            continue
        cone = _forward(out, u)
        priced = [t for t in cone if t in g.worth]
        if not priced:
            result[u] = Worth(0.0, 0.0, {})
            continue
        bad_in_cone = [comp for comp in bad_loops if any(v in comp for v in cone)]
        if bad_in_cone:
            result[u] = Worth(None, None, {}, flow_status="saturated_loop", loop=sorted(bad_in_cone[0]))
            continue
        has_ext_harms = any(
            not _helps(g, e) and not (e.src in loop_nodes and e.dst in loop_nodes)
            for v in cone for e in inc.get(v, []) if e.src in cone
        )
        try:
            if has_ext_harms:
                with_u = _settle(g, inc, dict(base), cone, knocked=None, active_harms={u}, loop_nodes=loop_nodes)
                without_u = _settle(g, inc, dict(base), cone, knocked=u, active_harms=set(), loop_nodes=loop_nodes)
                deltas = {t: with_u[t] - without_u[t] for t in priced}
            else:
                ko = _settle(g, inc, dict(base), cone, knocked=u, active_harms=set(), loop_nodes=loop_nodes)
                deltas = {t: base[t] - ko[t] for t in priced}
        except NoConvergence:
            unsettled = sorted([v for v in cone if v in loop_nodes])
            result[u] = Worth(None, None, {}, flow_status="no_convergence", loop=unsettled)
            continue

        gain = sum(g.worth[t] * d for t, d in deltas.items() if g.worth[t] > 0 and d > 0)
        loss = sum(-g.worth[t] * d for t, d in deltas.items() if g.worth[t] < 0) + sum(g.worth[t] * d for t, d in deltas.items() if g.worth[t] > 0 and d < 0)
        result[u] = Worth(_clean(gain), _clean(loss), {t: _clean(d) for t, d in deltas.items()})
    return result


def _clean(v: float) -> float:
    return 0.0 if abs(v) < 1e-9 else round(v, 9)


# ── Decision rule (section 6): value of information on settle work ─────────


def expected_best(options: list[tuple[float, float]]) -> float:
    """E[max_i X_i w_i, 0] for independent X_i ~ Bernoulli(p_i). options = [(w, p)]."""
    total, none_better = 0.0, 1.0
    for w, p in sorted(options, key=lambda o: -o[0]):
        if w <= 0:
            break
        total += none_better * p * w
        none_better *= 1.0 - p
    return total


def evpi(options: list[tuple[float, float]]) -> float:
    """Howard's expected value of perfect information for a choice among options."""
    blind = max([p * w for w, p in options] + [0.0])
    return expected_best(options) - blind


def decision_worth(g: Graph, w: dict[str, Worth]) -> dict[str, float]:
    """Worth each settle edge gives its work: strength x EVPI of the open decision."""
    extra: dict[str, float] = {}
    for d, st in g.state.items():
        if st != OPEN:
            continue
        opts = [
            (w[e.src].gain + w[e.src].loss_averted, e.probability)
            for e in g.edges
            if e.dst == d and e.label == "alternative" and e.src in w
        ]
        if len(opts) < 2:
            continue
        value = evpi(opts)
        for e in g.edges:
            if e.dst == d and e.label == "settles" and g.state.get(e.src) == OPEN:
                extra[e.src] = extra.get(e.src, 0.0) + e.quantum * value
    return extra


# ── Explanation as routes (invariant 12) ───────────────────────────────────


def routes(g: Graph, u: str, max_len: int = 12) -> dict[str, list[tuple[float, list[str]]]]:
    """Every simple route from u to each priced target, with its signed strength."""
    _, out = _index(g)
    found: dict[str, list] = {}

    def walk(v, path, strength):
        if v in g.worth and v != u:
            found.setdefault(v, []).append((strength, path))
        if len(path) > max_len:
            return
        for e in out.get(v, []):
            if e.dst in path or g.state.get(e.dst) == DONE:
                continue
            sign = 1.0 if _helps(g, e) else -1.0
            walk(e.dst, path + [e.dst], strength * sign * e.strength)

    walk(u, [u], 1.0)
    if u in g.worth:
        found.setdefault(u, []).append((1.0, [u]))
    return found


def on_loops(g: Graph) -> set[str]:
    """Nodes on a directed loop of flow edges."""
    return {v for comp in _components(g) for v in comp}


def _components(g: Graph) -> list[list[str]]:
    """Loops of flow edges, as Tarjan's strongly connected components of size > 1."""
    _, out = _index(g)
    index, low, stack, on, found, n = {}, {}, [], set(), [], [0]

    def visit(v):
        index[v] = low[v] = n[0]
        n[0] += 1
        stack.append(v)
        on.add(v)
        for e in out.get(v, []):
            if e.dst not in index:
                visit(e.dst)
                low[v] = min(low[v], low[e.dst])
            elif e.dst in on:
                low[v] = min(low[v], index[e.dst])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on.discard(w)
                comp.append(w)
                if w == v:
                    break
            if len(comp) > 1:
                found.append(sorted(comp))

    sys.setrecursionlimit(max(10000, sys.getrecursionlimit()))
    for v in sorted(g.state):
        if v not in index:
            visit(v)
    return found


# ── Live adapter: export_graph JSON (mem) -> Graph, per section 8 defaults ──

VERBAL = {
    "certain": 1.0, "almost certain": 1.0,
    "very probable": 0.85, "probable": 0.85, "highly likely": 0.85,
    "expected": 0.75, "likely": 0.75,
    "fifty-fifty": 0.5, "even chance": 0.5,
    "uncertain": 0.25, "possible": 0.25, "perhaps": 0.25, "maybe": 0.25,
    "improbable": 0.15, "unlikely": 0.15, "very unlikely": 0.15, "almost impossible": 0.15,
    "impossible": 0.0, "none": 0.0,
}  # mirrors numeric_weight(), src/graph.rs:292-318

MIGRATION = {"part_of": 0.0, "needs": 1.0, "supports": 0.3, "default": 0.0}


def stated_weight(ct: dict) -> float | None:
    """Quantum for a migrated contributes_to edge; None when no weight was stated."""
    s = str(ct.get("stated_weight") or "").strip().lower()
    m = ct.get("multiplier", ct.get("x"))
    if not s and not isinstance(m, (int, float)):
        return None
    if s in VERBAL:
        base = VERBAL[s]
    else:
        try:
            base = min(max(float(s), 0.0), 1.0)
        except ValueError:
            base = 0.0
    if isinstance(m, (int, float)) and m >= 0:
        base = m * base if s else float(m)
    return min(base, 1.0)  # quantum is 0..1 (flow-rule.md 5.1); the linter rejects larger


def from_export(data: dict, migration: dict = MIGRATION) -> Graph:
    g = Graph()
    if "edges" in data and isinstance(data["edges"], list) and data.get("nodes") and "state" in data["nodes"][0]:
        for n in data["nodes"]:
            g.add(n["id"], n["state"], n.get("worth"))
        for edge_item in data["edges"]:
            src, dst, label = edge_item[0], edge_item[1], edge_item[2]
            q = edge_item[3] if len(edge_item) > 3 else None
            eff = edge_item[4] if len(edge_item) > 4 else HELPS
            prob = edge_item[5] if len(edge_item) > 5 else 1.0
            if label == "relates":
                continue
            e = g.link(src, dst, migration["default"] if q is None else q, label=label, effect=eff, probability=prob)
            e.unvalued = q is None
        return g

    for n in data["nodes"]:
        st = n.get("status")
        state = GONE if st == "cancelled" else DONE if st in (None, "done") else OPEN
        g.add(n["id"], state, n.get("standing_weight"))
    ids = set(g.state)
    for n in data["nodes"]:
        v = n["id"]
        if n.get("parent") in ids:
            g.link(v, n["parent"], migration["part_of"], label="part_of")
        for dep in n.get("depends_on") or []:
            if dep in ids:
                g.link(dep, v, migration["needs"], label="needs")
        for dep in n.get("soft_depends_on") or []:
            if dep in ids:
                g.link(dep, v, migration["supports"], label="supports")
        for ct in n.get("contributes_to") or []:
            if ct.get("to") in ids:
                q = stated_weight(ct)
                e = g.link(v, ct["to"], migration["default"] if q is None else q, label="serves")
                e.unvalued = q is None
    return g


def main(argv: list[str]) -> int:
    if len(argv) < 3 or argv[1] != "live":
        print(__doc__)
        return 2
    data = json.load(open(argv[2]))
    g = from_export(data)
    w = worth_all(g)
    rows = {u: {"gain": r.gain, "loss_averted": r.loss_averted} for u, r in w.items()}
    if "--out" in argv:
        json.dump(rows, open(argv[argv.index("--out") + 1], "w"), indent=1, sort_keys=True)
    carrying = sum(1 for r in w.values() if r.gain or r.loss_averted)
    print(f"open nodes {len(w)}; carrying worth {carrying}; priced targets {len(g.worth)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
