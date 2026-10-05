#!/usr/bin/env python3
"""Demonstrate the seventeen invariants of the requirements brief (spec_866ee53d,
section C) on real nodes, using the committed live-graph fixture.

Every number in flow-rule.md section 10 is printed by this script:

    python3 invariants.py                # table to stdout
    python3 invariants.py --json out.json

Scenario edits to the live graph (a split, an added opportunity, a decision,
a negative target, a deadline class) are marked ASSUMED in the output: the
nodes are real, the edit is hypothetical, because the live graph does not yet
carry the field being tested.
"""

from __future__ import annotations

import copy
import json
import os
import re
import sys
import time
from datetime import date, timedelta

import display
import flow

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURE = os.path.join(HERE, "fixtures", "live-2026-10-05.json")


def load(default_quantum: float = 0.0, wikilinks: bool = False, date_shift: int = 0,
         part_of: float | None = None):
    """The live graph. Unvalued edges take `default_quantum`; wikilinks are not
    edges of the model unless `wikilinks` is set; `date_shift` moves every date;
    `part_of`, if given, overrides the quantum on every part_of edge (Q1)."""
    d = json.load(open(FIXTURE))
    g = flow.Graph()
    meta = {}
    for n in d["nodes"]:
        g.add(n["id"], n["state"], n.get("worth"))
        meta[n["id"]] = {k: n[k] for k in ("due", "effort", "node_type") if k in n}
        if date_shift and "due" in n:
            meta[n["id"]]["due"] = (date.fromisoformat(n["due"][:10]) + timedelta(days=date_shift)).isoformat()
    for src, dst, label, q in d["edges"]:
        if label == "relates" and not wikilinks:
            continue
        if label == "part_of" and part_of is not None:
            q = part_of
        g.link(src, dst, default_quantum if q is None else q, label=label)
    return g, meta, d["today_rank"]


def r3(v: float) -> float:
    return round(v, 4)


def total(w: flow.Worth) -> float:
    return w.gain + w.loss_averted


def diff(a: dict, b: dict, skip=()) -> float:
    """Largest change in gain or loss averted across nodes present in both, except `skip`."""
    m = 0.0
    for u in a:
        if u in b and u not in skip:
            m = max(m, abs(a[u].gain - b[u].gain), abs(a[u].loss_averted - b[u].loss_averted))
    return m


def upstream(g: flow.Graph, target: str) -> set:
    inc, _ = flow._index(g)
    seen, stack = {target}, [target]
    while stack:
        v = stack.pop()
        for e in inc.get(v, []):
            if e.src not in seen:
                seen.add(e.src)
                stack.append(e.src)
    return seen


G, META, TODAY_RANK = load()
t0 = time.perf_counter()
W = flow.worth_all(G)
RUNTIME = time.perf_counter() - t0


def inv1():
    b = "brain_448bb804"
    pre = sorted(e.src for e in G.edges if e.dst == b and e.label == "needs" and G.state[e.src] == flow.OPEN)
    g2 = copy.deepcopy(G)
    g2.edges = [e for e in g2.edges if not (e.dst == b and e.label == "needs")]
    w2 = flow.worth_all(g2)
    full = all(W[p].deltas.get(t, 0) >= d - 1e-9 for p in pre for t, d in W[b].deltas.items())
    # Control: remove every other route from the prerequisites, so `needs` is their only route.
    g3 = copy.deepcopy(G)
    g3.edges = [e for e in g3.edges if not (e.src in pre and e.dst != b)]
    w3 = flow.worth_all(g3, only=pre + [b])
    return {
        "claim": "two parallel prerequisites each carry the full worth of what they unblock",
        "nodes": [b] + pre,
        "numbers": {"worth(%s)" % b: r3(total(W[b])), **{"worth(%s)" % p: r3(total(W[p])) for p in pre},
                    "worth(%s) with its prerequisites removed" % b: r3(total(w2[b])),
                    **{"worth(%s), needs edge its only route" % p: r3(total(w3[p])) for p in pre},
                    "largest change elsewhere when prerequisites removed": r3(diff(W, w2, skip=set(pre)))},
        "pass": len(pre) >= 2 and full and abs(total(w2[b]) - total(W[b])) < 1e-9 and diff(W, w2, skip=set(pre)) < 1e-9
        and all(abs(total(w3[p]) - total(w3[b])) < 1e-9 for p in pre),
    }


def inv2():
    p = "personal_033d02b8"
    g2 = copy.deepcopy(G)
    g2.add("split_a")
    g2.add("split_b")
    for e in list(g2.edges):
        for part in ("split_a", "split_b"):
            if e.src == p:
                g2.link(part, e.dst, e.quantum, label=e.label, probability=e.probability, effect=e.effect)
            if e.dst == p:
                g2.link(e.src, part, e.quantum, label=e.label, probability=e.probability, effect=e.effect)
    g2.state[p] = flow.GONE
    w2 = flow.worth_all(g2)
    return {
        "claim": "splitting a task into necessary parts changes no unrelated node's standing",
        "nodes": [p], "assumed": "split of %s into two necessary parts" % p,
        "numbers": {"worth(original)": r3(total(W[p])), "worth(part a)": r3(total(w2["split_a"])),
                    "worth(part b)": r3(total(w2["split_b"])),
                    "largest change at any other node": r3(diff(W, w2, skip={p}))},
        "pass": diff(W, w2, skip={p}) < 1e-9 and abs(total(w2["split_a"]) - total(W[p])) < 1e-9,
    }


def inv3():
    u = "proj-76fbc546"
    rs = flow.routes(G, u)
    return {
        "claim": "one source by several routes counts once; two sources add",
        "nodes": [u, "targ_4e2cc92a", "task_7d6f78ad"],
        "numbers": {"routes to targ_4e2cc92a": [[r3(s), " > ".join(p)] for s, p in rs["targ_4e2cc92a"]],
                    "share of targ_4e2cc92a at stake": r3(W[u].deltas["targ_4e2cc92a"]),
                    "share of task_7d6f78ad at stake": r3(W[u].deltas["task_7d6f78ad"]),
                    "gain = 0.60 x share + 0.35 x share": r3(W[u].gain)},
        "pass": len(rs["targ_4e2cc92a"]) >= 2 and W[u].deltas["targ_4e2cc92a"] <= 1.0 + 1e-9
        and abs(W[u].gain - (0.6 * W[u].deltas["targ_4e2cc92a"] + 0.35 * W[u].deltas["task_7d6f78ad"])) < 1e-9,
    }


LOOP = ["aops_twin_cost_monitor", "aops_bootstrap_dogfood", "aops_otel_full_text_container_spans"]


def _loop_graph(close: bool, feed: float = 0.5, harm: bool = False, q: float | None = None):
    """The live three-node loop, fed partially by one new upstream task.

    Real loop: twin_cost_monitor -part_of-> bootstrap_dogfood -supports-> otel_spans -serves-> twin_cost_monitor.
    ASSUMED: an upstream task `feeder` supporting twin_cost_monitor at `feed`.
    """
    g2 = copy.deepcopy(G)
    g2.add("feeder")
    g2.link("feeder", LOOP[0], feed, label="supports")
    closing = [e for e in g2.edges if e.src == LOOP[2] and e.dst == LOOP[0]]
    for e in closing:
        if not close:
            e.quantum = 0.0
        if harm:
            e.effect = flow.HARMS
        if q is not None:
            e.quantum = q
    return g2


def inv4():
    ws, wo = (flow.worth_all(_loop_graph(c), only=["feeder"] + LOOP) for c in (True, False))
    pri = sorted(t for t in ws["feeder"].deltas)
    return {
        "claim": "a reinforcing loop converges and is worth at least as much as without the loop",
        "nodes": LOOP, "assumed": "a new task feeding the live loop at quantum 0.5",
        "numbers": {"worth(feeder) with loop": r3(total(ws["feeder"])), "worth(feeder) loop opened": r3(total(wo["feeder"])),
                    **{"worth(%s) with / without" % u: [r3(total(ws[u])), r3(total(wo[u]))] for u in LOOP},
                    "sources reached": pri},
        "pass": total(ws["feeder"]) >= total(wo["feeder"]) - 1e-12 and total(ws["feeder"]) > 0
        and all(total(ws[u]) >= total(wo[u]) - 1e-12 for u in LOOP),
    }


def inv5():
    b = "brain_448bb804"
    pre = sorted(e.src for e in G.edges if e.dst == b and e.label == "needs" and G.state[e.src] == flow.OPEN)
    g3 = copy.deepcopy(G)
    g3.edges = [e for e in g3.edges if not (e.src in pre and e.dst != b)]
    w3 = flow.worth_all(g3, only=pre)
    return {
        "claim": "a blocked task passes worth to its unblockers (and keeps its own)",
        "nodes": [b] + pre,
        "numbers": {"ready leaf? %s" % b: display.is_ready_leaf(G, b, META), "worth(%s)" % b: r3(total(W[b])),
                    **{"worth(%s)" % p: r3(total(W[p])) for p in pre},
                    **{"worth(%s), blocking its only route" % p: r3(total(w3[p])) for p in pre}},
        "pass": not display.is_ready_leaf(G, b, META) and total(W[b]) > 0 and all(total(W[p]) >= total(W[b]) - 1e-9 for p in pre)
        and all(abs(total(w3[p]) - total(W[b])) < 1e-9 for p in pre),
    }


def inv6():
    before = json.dumps({u: (r.gain, r.loss_averted) for u, r in W.items()}, sort_keys=True)
    views = []
    for ready_only in (True, False):
        for key in ("total", "per_day"):
            for buf in (7, 30):
                display.HORIZON_BUFFER_DAYS = buf
                views.append(display.view(G, W, META, date(2026, 10, 5), ready_only=ready_only, key=key)[:3])
    display.HORIZON_BUFFER_DAYS = 7
    after = json.dumps({u: (r.gain, r.loss_averted) for u, r in W.items()}, sort_keys=True)
    rerun = json.dumps({u: (r.gain, r.loss_averted) for u, r in flow.worth_all(G).items()}, sort_keys=True)
    flow_src = open(os.path.join(HERE, "flow.py")).read()
    return {
        "claim": "changing any display rule changes no number",
        "nodes": [],
        "numbers": {"display configurations run": len(views),
                    "distinct top-3 orderings": len({json.dumps([r["id"] for r in v]) for v in views}),
                    "worth table identical after all views": before == after == rerun,
                    "flow.py imports display": "import display" in flow_src},
        "pass": before == after == rerun and "import display" not in flow_src,
    }


def inv7():
    zero = [u for u, r in W.items() if not W[u].deltas]
    g5, _, _ = load(default_quantum=0.05)
    w5 = flow.worth_all(g5)
    gl, _, _ = load(default_quantum=0.05, wikilinks=True)
    wl = flow.worth_all(gl)
    gp, _, _ = load(part_of=0.0)
    wp = flow.worth_all(gp)
    return {
        "claim": "a node linked to nothing priced carries the default and nothing more",
        "nodes": ["academic-b738bdc7", "task_d5f610e6"],
        "numbers": {"open nodes": len(W), "open nodes with no route to a priced target": len(zero),
                    "of those, carrying any worth": sum(1 for u in zero if total(W[u]) != 0),
                    "worth(academic-b738bdc7)": r3(total(W["academic-b738bdc7"])),
                    "worth(task_d5f610e6)": r3(total(W["task_d5f610e6"])),
                    "open nodes carrying worth, default quantum 0": sum(1 for r in W.values() if total(r)),
                    "unvalued typed edges": sum(1 for e in json.load(open(FIXTURE))["edges"] if e[3] is None and e[2] != "relates"),
                    "open nodes carrying worth, default quantum 0.05 on unvalued edges": sum(1 for r in w5.values() if total(r)),
                    "same, if every wikilink also became an edge at 0.05": sum(1 for r in wl.values() if total(r)),
                    "nodes on loops then": len(flow.on_loops(gl)),
                    "open nodes carrying worth, part_of at quantum 0 (Q1)": sum(1 for r in wp.values() if total(r))},
        "pass": all(total(W[u]) == 0 for u in zero) and total(W["academic-b738bdc7"]) == 0,
    }


def inv8():
    parent, last = "proj-f8b942d5", "admin-3e02c20b"
    kids = [e.src for e in G.edges if e.dst == parent and e.label == "part_of"]
    rows = display.view(G, W, META, date(2026, 10, 5), key="per_day")
    pos = next(i for i, r in enumerate(rows) if r["id"] == last) + 1
    return {
        "claim": "when all but one necessary step is done, the remaining step carries the full worth (under necessity every open part does)",
        "nodes": [parent, last],
        "numbers": {"children done / open": [sum(G.state[k] == flow.DONE for k in kids), sum(G.state[k] == flow.OPEN for k in kids)],
                    "worth(%s)" % parent: r3(total(W[parent])), "worth(%s)" % last: r3(total(W[last])),
                    "position on worth-per-day among %d ready leaves" % len(rows): pos},
        "pass": abs(total(W[last]) - total(W[parent])) < 1e-9 and total(W[last]) > 0,
    }


def inv9():
    g2 = copy.deepcopy(G)
    g2.add("opportunity", flow.OPEN, 0.35)
    g2.link("admin-3e02c20b", "opportunity", 1.0, label="serves")
    w2 = flow.worth_all(g2)
    up = upstream(g2, "opportunity")
    added_nodes = set(g2.state) - set(G.state)
    added_edges = len(g2.edges) - len(G.edges)
    changed = sum(1 for a, b in zip(G.edges, g2.edges) if (a.src, a.dst, a.label, a.quantum, a.probability, a.effect) != (b.src, b.dst, b.label, b.quantum, b.probability, b.effect))
    changed += sum(1 for v in G.state if G.state[v] != g2.state[v] or G.worth.get(v) != g2.worth.get(v))
    gained_ok = all(abs((w2[u].gain - W[u].gain) - 0.35 * w2[u].deltas.get("opportunity", 0)) < 1e-9 for u in up if u in W)
    return {
        "claim": "adding an opportunity takes one node and its edges, and changes no other input",
        "nodes": ["admin-3e02c20b"], "assumed": "a new opportunity priced 0.35, served by admin-3e02c20b at quantum 1",
        "numbers": {"nodes added": len(added_nodes), "edges added": added_edges, "existing inputs changed": changed,
                    "nodes upstream of the opportunity": len(up) - 1,
                    "largest change at any node not upstream": r3(diff(W, w2, skip=up)),
                    "worth(admin-3e02c20b) before / after": [r3(total(W["admin-3e02c20b"])), r3(total(w2["admin-3e02c20b"]))]},
        "pass": diff(W, w2, skip=up) < 1e-9 and gained_ok and len(added_nodes) == 1 and added_edges == 1 and changed == 0,
    }


def inv10():
    d, a, b, s = "brain_bf2be9d8", "brain_7f772690", "proj-f8b942d5", "personal_92d5909f"

    def run(scale: float, decided: bool):
        g2 = copy.deepcopy(G)
        for t in g2.worth:
            g2.worth[t] *= scale
        g2.edges = [e for e in g2.edges if not (e.dst == d and e.src in (a, b) and e.label == "part_of")]
        for e in [e for e in g2.edges if e.src == d and e.label in ("serves", "part_of")]:
            for opt in (a, b):  # an option, if chosen, delivers what its decision delivers
                g2.link(opt, e.dst, e.quantum, label=e.label, probability=e.probability, effect=e.effect)
        g2.link(a, d, 1.0, label="alternative", probability=0.4)
        g2.link(b, d, 1.0, label="alternative", probability=0.3)
        g2.link(s, d, 1.0, label="settles")
        if decided:
            g2.state[d] = flow.DONE
        w2 = flow.worth_all(g2)
        return flow.decision_worth(g2, w2).get(s, 0.0), w2

    v1, w1 = run(1.0, False)
    v2, _ = run(2.0, False)
    v0, _ = run(1.0, True)
    opts = [(total(w1[a]), 0.4), (total(w1[b]), 0.3)]
    return {
        "claim": "an open decision weights the work that settles it in proportion to what rides on it; decided, it is gone",
        "nodes": [d, a, b, s],
        "assumed": "alternatives %s (p 0.4) and %s (p 0.3), relabelled from part_of, each inheriting the decision's own edges; %s settles the decision" % (a, b, s),
        "numbers": {"option worths": [r3(o[0]) for o in opts], "E[best | informed]": r3(flow.expected_best(opts)),
                    "best blind choice": r3(max(p * w for w, p in opts)), "EVPI to settle work": r3(v1),
                    "EVPI with every price doubled": r3(v2), "EVPI once decided": r3(v0)},
        "pass": v1 > 0 and abs(v2 - 2 * v1) < 1e-9 and v0 == 0.0,
    }


def inv11():
    src = open(os.path.join(HERE, "flow.py")).read()
    rule = src[: src.index("# ── Live adapter")]
    hits = re.findall(r"\b(due|date|created|today|deadline)\b", rule.split('"""', 2)[2])
    g2, meta2, _ = load(date_shift=400)
    w2 = flow.worth_all(g2)  # flow never receives META, which holds every date
    cliff_now = sum(display.on_cliff({**m, "deadline_class": "hard"}, date(2026, 10, 5)) for m in META.values())
    cliff_shifted = sum(display.on_cliff({**m, "deadline_class": "hard"}, date(2026, 10, 5)) for m in meta2.values())
    return {
        "claim": "no date arithmetic occurs in the flow",
        "nodes": [],
        "numbers": {"date tokens in the rule's code": hits, "flow reads node metadata": False,
                    "every due date shifted by days": 400,
                    "largest change in any figure": diff(W, w2),
                    "dates on the cliff if all were hard, before / after shift": [cliff_now, cliff_shifted]},
        "pass": not hits and diff(W, w2) == 0 and cliff_now != cliff_shifted,
    }


def inv12():
    loops = flow.on_loops(G)
    checked = below = viol = single = reinforced = 0
    extra = None
    for u, r in W.items():
        rs = flow.routes(G, u)
        for t, dlt in r.deltas.items():
            found = rs.get(t, [])
            strengths = [s for s, _ in found]
            if not strengths:
                continue
            checked += 1
            lo = max(strengths)
            hi = 1.0
            for s in strengths:
                hi *= 1.0 - s
            hi = min(1.0, 1.0 - hi)
            if dlt < lo - 1e-9:
                below += 1
            elif dlt > hi + 1e-9:
                if any(v in loops for _, p in found for v in p):
                    reinforced += 1
                    extra = [u, t, r3(dlt), r3(hi)]
                else:
                    viol += 1
            if len(strengths) == 1:
                single += abs(dlt - lo) < 1e-9
    u = "admin-3e02c20b"
    best = {t: max(flow.routes(G, u)[t]) for t in W[u].deltas}
    sentence = "; ".join("%s x %.2f via %s" % (t, s, " > ".join(p)) for t, (s, p) in sorted(best.items()))
    return {
        "claim": "every node's number is explained as a list of routes to priced sources",
        "nodes": [u],
        "numbers": {"(node, source) pairs checked": checked, "below the strongest route": below,
                    "above the combined routes, no loop to explain it": viol,
                    "above the combined routes by loop reinforcement (shown as such)": reinforced,
                    "that pair: node, target, share, combined routes": extra,
                    "single-route pairs equal to their route": single,
                    "nodes on live loops": len(loops), "explanation of %s" % u: sentence},
        "pass": checked > 0 and viol == 0 and below == 0,
    }


def inv13():
    scores = {u: (score, lineage) for u, score, lineage in TODAY_RANK}
    rank_today = {u: i + 1 for i, (u, _, _) in enumerate(TODAY_RANK)}
    listed = [u for u in rank_today if u in W]
    new = sorted(listed, key=lambda u: (-total(W[u]), rank_today[u]))
    rank_new = {u: i + 1 for i, u in enumerate(new)}
    cases = ["admin_1fece2e0", "task_model_ten_tasks_properly",
             "admin-3e02c20b", "trustcon_1c23b18d", "admin-59965524", "aops_polecat_mcp_server_build",
             "brain_448bb804", "proj-76fbc546", "academic-b738bdc7"]
    table = {u: {"state": G.state.get(u), "today": rank_today.get(u), "new": rank_new.get(u),
                 "worth": r3(total(W[u])) if u in W else None,
                 "today focus_score, value_lineage": list(scores.get(u, (None, None)))} for u in cases}
    top = {u: {"today": rank_today[u], "new": rank_new[u], "worth": r3(total(W[u])),
               "today focus_score, value_lineage": list(scores[u])} for u, _, _ in TODAY_RANK[:10]}
    flow_edges = len(G.flow_edges())
    return {
        "claim": "the rule runs over the live graph and differences from today's ranking are explained",
        "nodes": cases,
        "numbers": {"nodes": len(G.state), "open nodes": len(W), "flow edges (nonzero strength)": flow_edges,
                    "priced targets": len(G.worth), "under 0.5 seconds for every open node": RUNTIME < 0.5,
                    "tasks ranked today": len(TODAY_RANK), "of those, carrying worth": sum(1 for u in listed if total(W[u])),
                    "calibration cases": table, "today's top ten": top},
        "pass": len(W) > 1000 and flow_edges > 1000,
    }


SAFETY_WORK = ["proj-db6ded3c", "proj-76fbc546", "personal_344a9ec6"]


def inv14():
    pos = copy.deepcopy(G)
    pos.worth["targ_safety"] = 0.35
    neg = copy.deepcopy(G)
    neg.add("harm_someone_hurt", flow.OPEN, -0.35)
    for e in list(neg.edges):
        if e.dst == "targ_safety":
            neg.link(e.src, "harm_someone_hurt", e.quantum, label="serves", effect=flow.HARMS)
    wp, wn = flow.worth_all(pos, only=SAFETY_WORK), flow.worth_all(neg, only=SAFETY_WORK)
    return {
        "claim": "work that protects against a negative target carries positive weight by the same rule",
        "nodes": ["targ_safety"] + SAFETY_WORK,
        "assumed": "targ_safety priced +0.35; restated as a harm target priced -0.35 with harms edges",
        "numbers": {u: {"positive target: gain added": r3(wp[u].gain - W[u].gain),
                        "harm target: loss averted": r3(wn[u].loss_averted)} for u in SAFETY_WORK},
        "pass": all(wn[u].loss_averted > 0 and abs(wn[u].loss_averted - (wp[u].gain - W[u].gain)) < 1e-9 for u in SAFETY_WORK),
    }


def inv15():
    u, z = "proj-76fbc546", "academic-b738bdc7"
    g2 = copy.deepcopy(G)
    g2.add("harm_equal", flow.OPEN, -total(W[u]))
    g2.link(u, "harm_equal", 1.0, label="serves")
    w2 = flow.worth_all(g2, only=[u, z])
    return {
        "claim": "a large gain with an equal large loss is distinguishable from a task linked to nothing",
        "nodes": [u, z], "assumed": "%s also brings about a harm target priced at minus its gain" % u,
        "numbers": {"%s (gain, loss averted)" % u: [r3(w2[u].gain), r3(w2[u].loss_averted)],
                    "%s (gain, loss averted)" % z: [r3(w2[z].gain), r3(w2[z].loss_averted)],
                    "netted, both would read": [r3(total(w2[u])), r3(total(w2[z]))]},
        "pass": (w2[u].gain, w2[u].loss_averted) != (w2[z].gain, w2[z].loss_averted),
    }


def inv16():
    out = {}
    ok = True
    for label, q in (("closing edge harms, live quantum", None), ("closing edge harms, quantum 1 (pure negative feedback)", 1.0)):
        g2 = _loop_graph(True, feed=1.0, harm=True, q=q)
        try:
            w2 = flow.worth_all(g2, only=["feeder"] + LOOP)
            base = flow.baseline(g2)
            out[label] = {"worth(feeder)": r3(total(w2["feeder"])), "baseline realisation on loop": [r3(base[v]) for v in LOOP]}
        except flow.NoConvergence as exc:
            out[label] = str(exc)
            ok = False
    return {
        "claim": "a loop containing a harmful edge settles to a stable value",
        "nodes": LOOP, "assumed": "the live loop's closing edge restated as harms",
        "numbers": out, "pass": ok,
    }


def inv17():
    classes = {"task_d5f610e6": "hard", "n_c2e542fd02": "fake", "n_2dcb93a9c8": "soft"}
    meta = copy.deepcopy(META)
    for u, c in classes.items():
        meta[u]["deadline_class"] = c
    days = [date(2026, 9, 1), date(2026, 9, 25), date(2026, 10, 5), date(2026, 11, 20)]
    out = {}
    for u, c in classes.items():
        row = []
        for dday in days:
            rows = display.view(G, W, meta, dday, ready_only=False)
            pos = next(i for i, r in enumerate(rows) if r["id"] == u) + 1
            row.append({"date": dday.isoformat(), "cliff": rows[pos - 1]["cliff"], "position": pos})
        out["%s (%s, due %s, worth %s)" % (u, c, meta[u]["due"], r3(total(W[u])))] = row
    hard = out[next(k for k in out if "(hard" in k)]
    others = [v for k, v in out.items() if "(hard" not in k]
    return {
        "claim": "a hard deadline surfaces its work as the date nears whatever its worth; fake or soft never does",
        "nodes": list(classes), "assumed": "deadline classes as shown",
        "numbers": out,
        "pass": not hard[0]["cliff"] and hard[1]["cliff"] and hard[1]["position"] == 1
        and all(not r["cliff"] for v in others for r in v),
    }


ALL = [inv1, inv2, inv3, inv4, inv5, inv6, inv7, inv8, inv9, inv10, inv11, inv12, inv13, inv14, inv15, inv16, inv17]


def main(argv):
    results = []
    for i, f in enumerate(ALL, 1):
        r = f()
        r["invariant"] = i
        results.append(r)
        print("%2d %s  %s" % (i, "PASS" if r["pass"] else "FAIL", r["claim"]))
        if r.get("assumed"):
            print("     ASSUMED: " + r["assumed"])
        print("     " + json.dumps(r["numbers"], sort_keys=False))
    if "--json" in argv:
        json.dump(results, open(argv[argv.index("--json") + 1], "w"), indent=1)
    return 0 if all(r["pass"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
