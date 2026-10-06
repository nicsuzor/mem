#!/usr/bin/env python3
"""Build the committed migration fixture from a private export.

The repository is public, so the fixture keeps structure, edge kinds, the
fields the migration reads or leaves behind, and today's ranking, but no
titles, bodies, names or prose. Free-text fields (consequence, stakeholder,
justification) are reduced to "present". Every node id not cited in
flow-migration.md is replaced by a keyed pseudonym (HMAC-SHA256 prefix). Ids that carry a person's name are never
cited, so they are hashed like any other. The
key is read from MIGRATION_FIXTURE_KEY and is not stored anywhere: the
pseudonyms cannot be mapped back, and a guessed id cannot be confirmed.

Inputs:
  EXPORT   export_graph JSON (format: json, include_done: true): "nodes" and
           "edges" ({source, target, type}, wikilinks included as type "link")
  RANK     today's list_tasks order: {"ids": [[id, focus_score], ...]}

Usage:
    MIGRATION_FIXTURE_KEY=... python3 extract_fixture.py EXPORT RANK fixtures/live-2026-10-06.json
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import sys

CITED = {
    # targets, including the legacy `type: goal` nodes the engine reads as targets
    "task_b3f01c80", "targ_4e2cc92a", "task_7d6f78ad", "task_7327d7af",
    "targ-7d49f8a0", "research-supervision-1b44af33", "brain-b948a148",
    "targ-8f16a7c2", "targ_hdr_current_completion", "admin-73e215bc",
    "qut-f71664e8", "garage-69a576ed", "targ_safety", "targ-1e7d4733",
    "proj-home-c3997b42", "engagement-f7548c35", "toxicity-44e29ade",
    "channel-tja-advocacy", "channel-tja-journalists", "engagement-23b77fdc",
    "channel-tja-regulators", "channel-tja-techco", "targ-5b675f05",
    "target_no_walls_of_text", "brain-1b370df8", "channel-tja-academic",
    "task_c0b3e5b4", "ns-135cf982", "academicops-8b998f25",
    "academicops-5b784bb5", "academicops-506c5664", "ns-cb8d5b24",
    "goal-9ade5854", "accountability-4b99fcaa", "academicops-dfb31347",
    "goal_ws_conversation_discipline", "goal_ws_delegation",
    "goal_ws_evidence_adequacy", "goal_ws_memory_hygiene",
    "goal_ws_question_surfacing", "goal_ws_verification", "ns-a27b77ce",
    "aops-distributed-bazaar",
    # calibration cases (kb_weighting_user_stories_test_cases) and nodes cited in the spec
    "admin_1fece2e0", "task_model_ten_tasks_properly",
    "task-11182949", "admin-59965524",
    "brain_448bb804", "academic-b738bdc7", "admin-3e02c20b",
    "trustcon_1c23b18d", "aops_polecat_mcp_server_build", "proj-76fbc546",
    "mem_722b54d7", "task_d5f610e6", "task_e79f1a57", "personal_92d5909f",
    "brain_7f772690", "admin_3fcff4d3", "brain_bf2be9d8", "proj-f8b942d5",
    "creative-bc3db522", "task_1760e611", "brain_61467de3",
}


KEY = os.environ.get("MIGRATION_FIXTURE_KEY", "").encode()

# Fields kept as values; everything else the migration reads is kept as presence only.
VALUE_FIELDS = ["standing_weight", "severity", "goal_type", "intent", "due", "effort",
                "confidence", "focus_score", "value_lineage", "downstream_weight",
                "unlock_breadth", "urgency", "voi_value"]
PRESENCE_FIELDS = ["consequence", "stakeholder", "waiting_since", "complexity",
                   "classification", "order", "assignee", "has_open_question",
                   "stakeholder_exposure"]


def alias(node_id: str) -> str:
    if node_id in CITED:
        return node_id
    return "n_" + hmac.new(KEY, node_id.encode(), hashlib.sha256).hexdigest()[:10]


def main(export_path: str, rank_path: str, out_path: str) -> None:
    if not KEY:
        sys.exit("set MIGRATION_FIXTURE_KEY")
    data = json.load(open(export_path))
    rank = json.load(open(rank_path))
    nodes = []
    for n in sorted(data["nodes"], key=lambda n: n["id"]):
        o = {"id": alias(n["id"]), "type": n["node_type"], "status": n.get("status")}
        if n.get("parent"):
            o["parent"] = alias(n["parent"])
        for k in ("depends_on", "soft_depends_on", "goals", "supersedes"):
            if n.get(k):
                o[k] = [alias(x) for x in n[k]]
        if n.get("contributes_to"):
            o["contributes_to"] = [
                {"to": alias(c.get("to", "")), "stated_weight": c.get("stated_weight", ""),
                 **({"multiplier": c["multiplier"]} if "multiplier" in c else {}),
                 **({"justification": True} if c.get("justification") else {}),
                 **({"anomaly_flag": True} if c.get("anomaly_flag") else {})}
                for c in n["contributes_to"]
            ]
        for k in VALUE_FIELDS:
            if n.get(k) is not None:
                o[k] = n[k]
        for k in PRESENCE_FIELDS:
            if n.get(k) not in (None, "", [], False):
                o[k] = True
        nodes.append(o)
    edges = sorted([alias(e["source"]), alias(e["target"]), e["type"]] for e in data["edges"])
    out = {
        "source": "export_graph (include_done) and list_tasks order, 2026-10-06 ~11:55 UTC; "
                  "ids not cited in flow-migration.md replaced by a keyed hash whose key was discarded",
        "nodes": nodes,
        "edges": edges,
        "today_rank": [[alias(i), s] for i, s in rank["ids"]],
    }
    json.dump(out, open(out_path, "w"), separators=(",", ":"), sort_keys=True)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
