#!/usr/bin/env python3
"""Build the committed live-graph fixture from a private export.

The repository is public, so the fixture keeps structure, prices, dates and
edge strengths but no titles or bodies, and replaces every node id not cited
in flow-rule.md with a keyed pseudonym (HMAC-SHA256 prefix). The key is set in
FLOW_FIXTURE_KEY and kept out of the repository, so a guessed id cannot be
confirmed by hashing it; whoever holds the key can map pseudonyms back.

Inputs:
  EXPORT   export_graph JSON (format: json, include_done: true), optionally
           with a top-level "links" list of [source, target] wikilink pairs
  RANK     today's list_tasks order: {"ids": [[id, focus_score], ...]}

Today's focus_score and value_lineage are kept for ranked tasks only, so the
comparison in flow-rule.md section 11 can be printed.

Usage:
    python3 extract_fixture.py EXPORT RANK fixtures/live-2026-10-05.json
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import sys

import flow

CITED = {
    # priced targets
    "task_b3f01c80", "targ_4e2cc92a", "task_7d6f78ad", "task_7327d7af",
    "targ-7d49f8a0", "research-supervision-1b44af33", "brain-b948a148",
    # nodes cited in flow-rule.md sections 4, 10 and 11
    "brain_448bb804", "brain_c57fbad6", "personal_033d02b8", "proj-76fbc546",
    "proj-f8b942d5", "admin-3e02c20b", "task_d5f610e6", "targ_safety",
    "proj-db6ded3c", "personal_344a9ec6", "brain_bf2be9d8", "brain_7f772690",
    "personal_92d5909f", "task_e79f1a57", "aops_bootstrap_dogfood",
    "aops_twin_cost_monitor", "aops_otel_full_text_container_spans",
    "spec_a98d0e11", "aops_epic_task_lifecycle", "admin_1fece2e0",
    "task_model_ten_tasks_properly", "admin-59965524",
    "aops_polecat_mcp_server_build", "academic-b738bdc7", "trustcon_1c23b18d",
    "admin_3fcff4d3",
}


KEY = os.environ.get("FLOW_FIXTURE_KEY", "").encode()


def alias(node_id: str) -> str:
    """Cited ids stay; every other id becomes a keyed hash (the key is private)."""
    if node_id in CITED:
        return node_id
    return "n_" + hmac.new(KEY, node_id.encode(), hashlib.sha256).hexdigest()[:10]


def main(export_path: str, rank_path: str, out_path: str) -> None:
    if not KEY:
        sys.exit("set FLOW_FIXTURE_KEY")
    data = json.load(open(export_path))
    g = flow.from_export(data)
    meta = {n["id"]: n for n in data["nodes"]}
    nodes = []
    for v in sorted(g.state, key=alias):
        row = {"id": alias(v), "state": g.state[v]}
        if v in g.worth:
            row["worth"] = g.worth[v]
        for k in ("due", "effort", "node_type"):
            if meta[v].get(k) is not None:
                row[k] = meta[v][k]
        nodes.append(row)
    edges = [[alias(e.src), alias(e.dst), e.label, None if e.unvalued else e.quantum] for e in g.edges]
    for src, dst in data.get("links", []):
        if src in g.state and dst in g.state:
            edges.append([alias(src), alias(dst), "relates", None])  # wikilink: not an edge of the model
    rank = [[alias(i), score, meta.get(i, {}).get("value_lineage")] for i, score in json.load(open(rank_path))["ids"]]
    json.dump(
        {"source": "pkb export_graph + list_tasks, 2026-10-05", "nodes": nodes, "edges": edges, "today_rank": rank},
        open(out_path, "w"),
        separators=(",", ":"),
    )
    print(f"{len(nodes)} nodes, {len(edges)} edges, {len(rank)} ranked -> {out_path}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
