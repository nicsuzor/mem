#!/usr/bin/env python3
"""Reference dry run for specs/flow-migration.md.

A specification aid, not the migration tool: it exists so every count and
before/after figure in the spec can be reproduced from the committed fixture.
Standard library only; the flow itself is the reference calculator in
../flow-rule/flow.py.

It prints, in order:
  1. every stored edge kind: entries, resolved edges, state of each end, open-source count
  2. every node field the migration reads or leaves behind, with counts
  3. the writes the migration makes (proposed storage, section 5 of the spec)
  4. the reversal check: apply, revert, apply twice, and the legacy view, node by node
  5. the flow on the migrated graph, cross-checked against flow-rule's own adapter
  6. the calibration cases before and after
  7. what leaves the top of today's list, and why
  8. the pricing illustration (assumed prices, clearly marked)

Usage:
    python3 dryrun.py [fixtures/live-2026-10-06.json]
"""

from __future__ import annotations

import copy
import json
import os
import sys
import time
from collections import Counter

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "flow-rule"))
import flow  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURE = os.path.join(HERE, "fixtures", "live-2026-10-06.json")

# Proposed migration values: flow-rule.md section 8, unchanged (Q22 there; M-Q3 here).
SUPPORTS_QUANTUM = 0.3
# Label defaults the engine applies when an edge states no quantum (flow-rule.md 5.2, 5.3).
LABEL_DEFAULT = {"part_of": 1.0, "needs": 1.0, "supports": 0.0, "serves": 0.0}

# Keys the migration may add. Nothing else is written (spec section 5.3).
ADDED_KEYS = {"worth", "quantum", "probability", "set_by"}

CASES = [
    # (case number in kb_weighting_user_stories_test_cases, node)
    ("1", "admin_1fece2e0"),
    ("2", "task_model_ten_tasks_properly"), ("2", "admin-59965524"),
    ("2", "brain_448bb804"), ("2", "academic-b738bdc7"),
    ("3", "mem_722b54d7"), ("3", "task-11182949"),
    ("4", "admin-3e02c20b"), ("4", "trustcon_1c23b18d"),
    ("5", "aops_polecat_mcp_server_build"),
    ("6", "proj-76fbc546"),
    # top of today's list, cited in flow-rule.md section 11
    ("-", "task_e79f1a57"), ("-", "personal_92d5909f"), ("-", "brain_7f772690"),
    ("-", "task_d5f610e6"), ("-", "admin_3fcff4d3"),
]


def present(x) -> bool:
    """A field counts as present unless absent, empty or false; 0 (SEV0, P0) is present."""
    return x is not None and x is not False and x != "" and x != []


def state(n: dict) -> str:
    """Node state as the flow reads it; mirrors flow.from_export."""
    st = n.get("status")
    return flow.GONE if st == "cancelled" else flow.DONE if st in (None, "done") else flow.OPEN


def word_quantum(word: str) -> float | None:
    """Quantum for a stated contributes_to word; None if empty or unrecognised."""
    w = (word or "").strip().lower()
    if w in flow.VERBAL:
        return flow.VERBAL[w]
    try:
        return min(max(float(w), 0.0), 1.0)
    except ValueError:
        return None


# ── The transform (spec section 5) ──────────────────────────────────────────

def apply(fm: dict, ids: set, snapshot: set | None = None) -> tuple[dict, list]:
    """Additive migration of one node's frontmatter.

    Returns (new frontmatter, ledger rows). Each row is (key path, value before,
    value after); a before of None means the key was absent. When `snapshot` is
    given, a bare soft_depends_on entry is migrated only if (node id, entry) is
    in it, so a re-run never re-values a link written after the snapshot
    (spec section 5.5).
    """
    out = copy.deepcopy(fm)
    ledger = []
    if out.get("standing_weight") is not None and "worth" not in out:
        out["worth"] = out["standing_weight"]
        ledger.append(("worth", None, out["worth"]))
    soft = out.get("soft_depends_on") or []
    for i, entry in enumerate(soft):
        if isinstance(entry, str) and entry in ids and (snapshot is None or (fm.get("id"), entry) in snapshot):
            soft[i] = {"to": entry, "quantum": SUPPORTS_QUANTUM, "set_by": "migrated"}
            ledger.append((f"soft_depends_on[{i}]", entry, copy.deepcopy(soft[i])))
    for i, ct in enumerate(out.get("contributes_to") or []):
        if "quantum" in ct or ct.get("to") not in ids:
            continue
        q = word_quantum(ct.get("stated_weight", ""))
        m = ct.get("multiplier")
        has_m = isinstance(m, (int, float)) and m >= 0
        if q is None and not has_m:
            continue  # empty or unrecognised word, no multiplier: left unvalued, listed for densify
        q = min(m if q is None else q * m, 1.0) if has_m else q  # as flow.stated_weight
        before = copy.deepcopy(ct)
        ct.update({"quantum": q, "probability": 1.0, "set_by": "migrated"})
        ledger.append((f"contributes_to[{i}]", before, copy.deepcopy(ct)))
    return out, ledger


def _get(fm: dict, path: str):
    if "[" not in path:
        return fm.get(path)
    key, i = path[:-1].split("[")
    lst = fm.get(key) or []
    return lst[int(i)] if int(i) < len(lst) else None


def _set(fm: dict, path: str, value) -> None:
    if "[" not in path:
        if value is None:
            fm.pop(path, None)
        else:
            fm[path] = value
        return
    key, i = path[:-1].split("[")
    fm[key][int(i)] = value


def revert(fm: dict, rows: list) -> tuple[dict, list]:
    """Inverse of apply, driven by the ledger rows for this node.

    A row is undone only if the current value still equals its "after" value;
    otherwise it is reported as drift and left alone (spec section 7.2, M15).
    Returns (frontmatter, drifted key paths).
    """
    out = copy.deepcopy(fm)
    drift = []
    for path, before, after in reversed(rows):
        if _get(out, path) == after:
            _set(out, path, before)
        else:
            drift.append(path)
    return out, drift


def r1_view(fm: dict) -> dict:
    """What the legacy ranking reads once the R1 engine change ships: every key
    except the added ones, with a map entry in soft_depends_on read as its id.
    Today's parser (parse_string_array, src/graph.rs:1065) keeps strings only and
    would drop map entries, which is why R1 must ship before apply (section 9)."""
    v = {k: x for k, x in fm.items() if k not in ADDED_KEYS}
    if v.get("soft_depends_on"):
        v["soft_depends_on"] = [e["to"] if isinstance(e, dict) else e for e in v["soft_depends_on"]]
    if v.get("contributes_to"):
        v["contributes_to"] = [{k: x for k, x in c.items() if k not in ADDED_KEYS} for c in v["contributes_to"]]
    return v


# ── Reading the migrated graph (spec section 5.4) ───────────────────────────

def read_migrated(nodes: list, label_default: dict = LABEL_DEFAULT, worth_override: dict | None = None) -> flow.Graph:
    g = flow.Graph()
    ids = {n["id"] for n in nodes}
    for n in nodes:
        w = n.get("worth")
        if worth_override and n["id"] in worth_override:
            w = worth_override[n["id"]]
        g.add(n["id"], state(n), w)
    for n in nodes:
        v = n["id"]
        if n.get("parent") in ids:
            g.link(v, n["parent"], label_default["part_of"], label="part_of")
        for d in n.get("depends_on") or []:
            if d in ids:
                g.link(d, v, label_default["needs"], label="needs")
        for e in n.get("soft_depends_on") or []:
            d, q = (e["to"], e["quantum"]) if isinstance(e, dict) else (e, None)
            if d in ids:
                g.link(d, v, label_default["supports"] if q is None else q, label="supports")
        for ct in n.get("contributes_to") or []:
            if ct.get("to") in ids:
                q = ct.get("quantum")
                g.link(v, ct["to"], label_default["serves"] if q is None else q,
                       label="serves", probability=ct.get("probability", 1.0))
    return g


def show(title: str) -> None:
    print(f"\n## {title}\n")


def main(path: str) -> int:
    t0 = time.time()
    data = json.load(open(path))
    nodes = data["nodes"]
    by_id = {n["id"]: n for n in nodes}
    ids = set(by_id)
    edges = data["edges"]

    # 1. Stored edge kinds
    show("1. Edge kinds (resolved edges in export_graph; stored entries in frontmatter)")
    stored = {
        "parent": sum(1 for n in nodes if n.get("parent")),
        "depends_on": sum(len(n.get("depends_on") or []) for n in nodes),
        "soft_depends_on": sum(len(n.get("soft_depends_on") or []) for n in nodes),
        "contributes_to": sum(len(n.get("contributes_to") or []) for n in nodes),
        "supersedes": sum(len(n.get("supersedes") or []) for n in nodes),
        "goals": sum(len(n.get("goals") or []) for n in nodes),
    }
    kinds = Counter(e[2] for e in edges)
    print("| kind | stored entries | resolved edges | entries naming no node | open source, live dest | open source, done dest | from done | from or to cancelled |")
    print("|---|---|---|---|---|---|---|---|")
    for k in ["parent", "depends_on", "soft_depends_on", "contributes_to", "supersedes", "link", "goals"]:
        es = [e for e in edges if e[2] == k]
        if k == "goals":
            es = [[n["id"], g, "goals"] for n in nodes for g in n.get("goals") or [] if g in ids]
        # export_graph orients depends_on/soft_depends_on as (dependent -> dependency); the work is the target
        def work_dest(e):
            return (e[1], e[0]) if k in ("depends_on", "soft_depends_on") else (e[0], e[1])
        st = Counter()
        for e in es:
            w, d = work_dest(e)
            sw, sd = state(by_id[w]), state(by_id[d])
            if flow.GONE in (sw, sd):
                st["gone"] += 1
            elif sw == flow.OPEN:
                st["open_live" if sd == flow.OPEN else "open_done"] += 1
            else:
                st["from_done"] += 1
        n_stored = stored.get(k, "—")
        refs = ([n["parent"]] if k == "parent" and n.get("parent") else n.get(k) or [] for n in nodes)
        refs = [r if isinstance(r, str) else r.get("to") for rs in refs for r in rs]
        dangling = sum(1 for r in refs if r not in ids) if isinstance(n_stored, int) else "—"
        print(f"| `{k}` | {n_stored} | {len(es)} | {dangling} | {st['open_live']} | {st['open_done']} | {st['from_done']} | {st['gone']} |")
    extra_parent = [e for e in edges if e[2] == "parent" and by_id[e[0]].get("parent") != e[1]]
    print(f"\nparent edges not from a `parent` key (stored `children`): {len(extra_parent)} {extra_parent}")
    print(f"edge kinds present: {dict(sorted(kinds.items()))}")

    # 2. Fields
    show("2. Node fields the migration reads or leaves behind")
    fields = ["standing_weight", "severity", "goal_type", "consequence", "intent", "stakeholder",
              "waiting_since", "due", "effort", "complexity", "confidence", "order", "classification",
              "assignee", "has_open_question", "stakeholder_exposure",
              "focus_score", "value_lineage", "downstream_weight", "unlock_breadth", "urgency", "voi_value"]
    COMPUTED = {"focus_score", "value_lineage", "downstream_weight", "unlock_breadth", "urgency", "voi_value"}  # counted when nonzero
    print("| field | nodes | open nodes |")
    print("|---|---|---|")
    for f in fields:
        have = [n for n in nodes if (n.get(f) not in (None, 0) if f in COMPUTED else present(n.get(f)))]
        print(f"| `{f}` | {len(have)} | {sum(1 for n in have if state(n) == flow.OPEN)} |")
    cts = [(n, c) for n in nodes for c in n.get("contributes_to") or []]
    words = Counter((c.get("stated_weight") or "").strip().lower() for _, c in cts)
    print(f"\ncontributes_to stated words: {dict(words.most_common())}")
    print(f"contributes_to with multiplier: {sum(1 for _, c in cts if 'multiplier' in c)}; "
          f"with justification: {sum(1 for _, c in cts if c.get('justification'))}; "
          f"with anomaly_flag: {sum(1 for _, c in cts if c.get('anomaly_flag'))}")
    strategic = [n for n in nodes if n["type"] in ("target", "goal")]
    print(f"strategic nodes: {len(strategic)} ({Counter(n['type'] for n in strategic)}); "
          f"priced: {sum(1 for n in strategic if n.get('standing_weight') is not None)}; "
          f"with severity: {sum(1 for n in strategic if n.get('severity') is not None)}; "
          f"goal_type committed: {sorted(n['id'] for n in strategic if n.get('goal_type') == 'committed')}")
    print(f"node states: {dict(Counter(n.get('status') for n in nodes))}")
    print(f"open nodes with due: {sorted((n['id'], n['due']) for n in nodes if n.get('due') and state(n) == flow.OPEN)}")

    # 3. Writes
    show("3. Writes the migration makes")
    migrated, ledger, rows_by = [], [], {}
    for n in nodes:
        m, rows = apply(n, ids)
        migrated.append(m)
        rows_by[n["id"]] = rows
        ledger.extend((n["id"],) + r for r in rows)
    kinds_w = Counter(r[1].split("[")[0] for r in ledger)
    changed = [n["id"] for n in nodes if rows_by[n["id"]]]
    print(f"ledger rows: {len(ledger)} on {len(changed)} nodes; by key: {dict(kinds_w)}")
    ct_q = Counter(r[3]["quantum"] for r in ledger if r[1].startswith("contributes_to"))
    print(f"contributes_to quanta written: {dict(sorted(ct_q.items()))}")
    left = [(n["id"], c.get("stated_weight")) for n, c in cts
            if c.get("to") in ids and word_quantum(c.get("stated_weight", "")) is None]
    print(f"contributes_to to a node, left unvalued (empty or unrecognised word): {len(left)} {left}")
    gone_words = [(n["id"], c.get("stated_weight")) for n, c in cts
                  if c.get("to") not in ids and word_quantum(c.get("stated_weight", "")) is None]
    print(f"contributes_to naming no node, with empty or unrecognised word: {gone_words}")

    # 4. Reversal
    show("4. Reversal")
    pairs = [(n, m) for n, m in zip(nodes, migrated) if rows_by[n["id"]]]
    rt = sum(1 for n, m in pairs if revert(m, rows_by[n["id"]]) == (n, []))
    idem = sum(1 for m in migrated if apply(m, ids)[1] == [])
    view = sum(1 for n, m in pairs if r1_view(m) == r1_view(n))
    touched = {k for n, m in pairs for k in set(m) | set(n) if m.get(k) != n.get(k)}
    print(f"changed nodes restored exactly by revert(apply(x)) from the ledger: {rt} of {len(pairs)}")
    print(f"second apply writes nothing: {idem} of {len(nodes)} nodes")
    print(f"changed nodes whose R1 legacy view is unchanged (no key outside the allowed set moved): {view} of {len(pairs)}")
    print(f"top-level keys changed: {sorted(touched)}")
    # edge cases: a worth already set is kept; an edit after apply is reported as drift and left alone
    pre = {"id": "x", "standing_weight": 0.35, "worth": 0.6, "soft_depends_on": [sorted(ids)[0]]}
    m, rows = apply(pre, ids)
    back, _ = revert(m, rows)
    print(f"case: node already has worth 0.6: worth after apply {m['worth']}; after revert {back.get('worth')}; restored exactly {back == pre}")
    m["soft_depends_on"][0]["quantum"] = 0.1  # edited by densify after apply
    back, drift = revert(m, rows)
    print(f"case: soft link edited after apply: drift reported {drift}; edited value kept {back['soft_depends_on'][0] == m['soft_depends_on'][0]}")
    snap = {(n["id"], d) for n in nodes for d in n.get("soft_depends_on") or []}
    late = {"id": "y", "soft_depends_on": [sorted(ids)[1]]}
    print(f"case: bare soft link added after the snapshot, re-run with snapshot: rows written {len(apply(late, ids, snap)[1])}")

    # 5. Flow on the migrated graph
    show("5. Flow on the migrated graph")
    g = read_migrated(migrated)
    t1 = time.time()
    w = flow.worth_all(g)
    t_flow = time.time() - t1
    ref = flow.worth_all(flow.from_export({"nodes": nodes}))
    same = sum(1 for u in w if (w[u].gain, w[u].loss_averted) == (ref[u].gain, ref[u].loss_averted))
    carrying = {u for u, r in w.items() if r.gain or r.loss_averted}
    print(f"open nodes: {len(w)}; carrying worth: {len(carrying)}; priced targets: {len(g.worth)}; "
          f"flow edges: {len(g.flow_edges())}; loops: {len(flow.on_loops(g))} nodes; "
          f"saturated loops: {flow.saturated_loops(g)}; flow time {t_flow:.1f} s")
    print(f"identical to flow-rule's own adapter (flow.from_export on the unmigrated fixture): {same} of {len(w)}")
    g0 = read_migrated(migrated, {**LABEL_DEFAULT, "part_of": 0.0})
    w0 = flow.worth_all(g0)
    print(f"variant part_of at quantum 0 (flow-rule Q1): carrying worth {sum(1 for r in w0.values() if r.gain or r.loss_averted)}")
    lbl = Counter(e.label for e in g.flow_edges())
    print(f"flow edges by label: {dict(lbl)}")

    # 6. Calibration cases
    show("6. Calibration cases, before and after")
    today = [i for i, _ in data["today_rank"]]
    score = dict(data["today_rank"])
    pos = {u: k for k, u in enumerate(today)}
    key = lambda u: (-(w[u].gain + w[u].loss_averted), pos[u])  # comparison device only (flow-rule Q3)
    new = sorted((u for u in today if u in w), key=key)
    npos = {u: k + 1 for k, u in enumerate(new)}
    print(f"ranked today: {len(today)}; of them carrying worth after: {sum(1 for u in today if u in carrying)}")
    print("| case | node | state | today rank | focus_score | after: gain | after: loss averted | after rank |")
    print("|---|---|---|---|---|---|---|---|")
    for case, u in CASES:
        n = by_id.get(u)
        if n is None:
            print(f"| {case} | `{u}` | not in graph | — | — | — | — | — |")
            continue
        r = w.get(u)
        print(f"| {case} | `{u}` | {n.get('status')} | {pos[u] + 1 if u in pos else '—'} | "
              f"{score.get(u, n.get('focus_score', '—'))} | {r.gain if r else '—'} | "
              f"{r.loss_averted if r else '—'} | {npos.get(u, '—')} |")

    # 7. What leaves the top
    show("7. Today's top 50 after migration")
    top = today[:50]
    zero = [u for u in top if u not in carrying]
    why = Counter()
    for u in zero:
        n = by_id[u]
        tags = [f for f in ("due", "stakeholder", "intent", "severity", "value_lineage") if present(n.get(f))]
        why[",".join(tags) or "none of due/stakeholder/intent/severity/value_lineage"] += 1
    print(f"of today's top 50: {len(top) - len(zero)} carry worth after, {len(zero)} carry none")
    print(f"what held the {len(zero)} up today: {dict(why.most_common())}")
    print(f"today's top 10 -> after rank: {[(u, npos.get(u)) for u in today[:10]]}")
    print(f"after top 10 -> today rank: {[(u, pos[u] + 1) for u in new[:10]]}")

    # 8. Pricing illustration (assumed)
    show("8. Pricing illustration (ASSUMED prices, not Nic's)")
    unpriced = {n["id"]: 0.15 for n in nodes if n["type"] in ("target", "goal") and n.get("standing_weight") is None}
    gp = read_migrated(migrated, worth_override=unpriced)
    wp = flow.worth_all(gp)
    cp = {u for u, r in wp.items() if r.gain or r.loss_averted}
    print(f"every unpriced target and goal at Moderate (0.15): priced {len(gp.worth)}; carrying worth {len(cp)} (was {len(carrying)})")
    for case, u in CASES:
        if u in wp:
            print(f"  {u}: gain {wp[u].gain}")

    # 9. Pricing sheet order
    show("9. Pricing sheet: reach of each strategic node (open nodes that carry worth once it is priced)")
    strategic = sorted(n["id"] for n in nodes if n["type"] in ("target", "goal"))
    inc = {}
    for e in g.flow_edges():
        inc.setdefault(e.dst, []).append(e.src)
    reach = {}
    for t in strategic:
        seen, stack = set(), [t]
        while stack:
            v = stack.pop()
            for s_ in inc.get(v, []):
                if s_ not in seen:
                    seen.add(s_)
                    stack.append(s_)
        reach[t] = sum(1 for v in seen if g.state.get(v) == flow.OPEN)
    tt = [(n["id"], c["to"]) for n in nodes if n["type"] in ("target", "goal")
          for c in n.get("contributes_to") or [] if by_id.get(c["to"], {}).get("type") in ("target", "goal")]
    print("| node | type | priced today | severity | goal_type | reach |")
    print("|---|---|---|---|---|---|")
    for t in sorted(strategic, key=lambda t: (-reach[t], t)):
        n = by_id[t]
        print(f"| `{t}` | {n['type']} | {n.get('standing_weight', '—')} | {n.get('severity', '—')} | {n.get('goal_type', '—')} | {reach[t]} |")
    print(f"\nstrategic nodes with reach 0: {sum(1 for t in strategic if reach[t] == 0)}")
    print(f"target-to-target contributes_to edges (flow-rule Q17): {len(tt)}")
    print(f"\ntotal run time {time.time() - t0:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else FIXTURE))
