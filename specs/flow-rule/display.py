"""Display and agent logic that sits outside the flow (flow-rule.md section 7).

Every function here reads the numbers flow.py produced and never writes them.
"""

from __future__ import annotations

import math
import os
import sys
from datetime import date

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import flow

HORIZON_BUFFER_DAYS = 7  # open question Q9 in flow-rule.md
DEFAULT_EFFORT_DAYS = 3  # mirrors ranking.md:148
ACTIONABLE_TYPES = {"task", "learn", "pr"}  # ranking.md:499, parser maps epic→task


def effort_days(effort: str | None) -> int:
    """Parse '2d', '3h', '1w' as mem's parse_effort_days does (src/graph.rs:896)."""
    if not effort:
        return DEFAULT_EFFORT_DAYS
    s = str(effort).strip().lower()
    scale = {"w": 7.0, "d": 1.0, "h": 1.0 / 8.0}.get(s[-1:], None)
    try:
        days = float(s[:-1]) * scale if scale else float(s)
    except ValueError:
        return DEFAULT_EFFORT_DAYS
    return max(1, math.ceil(days))


def actionable(meta: dict, u: str) -> bool:
    return meta.get(u, {}).get("node_type", "task") in ACTIONABLE_TYPES


def is_ready_leaf(g: flow.Graph, u: str, meta: dict | None = None) -> bool:
    """Open and actionable, with no open `needs` prerequisite and no open actionable `part_of` child."""
    meta = meta or {}
    if g.state.get(u) != flow.OPEN or not actionable(meta, u):
        return False
    for e in g.edges:
        if e.dst == u and g.state.get(e.src) == flow.OPEN:
            if e.label == "needs" or (e.label == "part_of" and actionable(meta, e.src)):
                return False
    return True


def on_cliff(meta: dict, today: date) -> bool:
    """Hard deadlines only: surface once days left <= effort + buffer (item 13)."""
    if meta.get("deadline_class") != "hard" or not meta.get("due"):
        return False
    days_left = (date.fromisoformat(meta["due"][:10]) - today).days
    return days_left <= effort_days(meta.get("effort")) + HORIZON_BUFFER_DAYS


def view(g: flow.Graph, w: dict, meta: dict, today: date, ready_only: bool = True, key: str = "total") -> list[dict]:
    """One ordered list: cliff lane first, then by the plain sum of gain and loss averted (settled Q3, S17).

    Numbers are copied, never altered.
    """
    rows = []
    for u, r in w.items():
        if u in g.worth:  # targets are what work serves, not work to do
            continue
        if ready_only and not is_ready_leaf(g, u, meta):
            continue
        m = meta.get(u, {})
        gain = r.gain or 0.0
        loss = r.loss_averted or 0.0
        total = gain + loss
        rows.append({
            "id": u,
            "gain": r.gain,
            "loss_averted": r.loss_averted,
            "cliff": on_cliff(m, today),
            "per_day": total / effort_days(m.get("effort")),
            "total": total,
        })
    rows.sort(key=lambda row: (not row["cliff"], -row[key], row["id"]))
    return rows
