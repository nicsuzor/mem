---
id: mutation-neighborhood
title: "Mutation Neighborhood: graph context in task-close responses"
type: spec
status: ready
tier: data
depends_on: [work-management, multi-parent]
tags: [spec, tasks, mcp, graph, ux, tokens]
---

# Mutation Neighborhood: graph context in task-close responses

## Giving Effect

- PKB MCP server (Rust, `nicsuzor/mem`)
- [[mcp__pkb__complete_task]] — marks a task `done`; returns a neighborhood block
- [[mcp__pkb__release_task]] — releases a task to a terminal/handoff status; returns a neighborhood block
- Consumes existing graph helpers: `GraphStore::get_node`, `is_blocked`, `resolve`; `graph::is_completed`, `graph::is_closed_for_hierarchy`

## Purpose

An agent that closes or releases a task is mid-curation: it has just changed the graph
and the next useful action is almost always *on a neighbour* — close the parent now that
its last child is done, pick up a sibling, or start a task this one just unblocked.

`complete_task` and `release_task` return a compact **neighborhood** alongside the
confirmation: the minimal slice of the local graph an agent needs to curate related
tasks in the same turn, and nothing more. It removes the follow-up `get_task` /
`pkb_context` call that curation would otherwise require.

`get_task` and `pkb_context` are the surfaces for *full* relationship context. The
mutation neighborhood is a strict subset — a nudge, not a dump.

## Scope

| Tool            | Neighborhood | Notes                                                                       |
| --------------- | ------------ | --------------------------------------------------------------------------- |
| `complete_task` | yes          | Computed against the post-write graph (status already `done`)               |
| `release_task`  | yes          | Same block; `unblocked` is empty unless the release status clears the dependency |
| `create_task`, `update_task`, others | none | Served by `get_task` / planner capture trees                 |

## Response shape

The tool result is a JSON envelope (`ok`, `id`, `status`, `message`, `neighborhood`);
`message` is the human-readable confirmation line carrying the task title. The
neighborhood lives under `neighborhood`. Every member field is **omitted when empty**, so
a leaf task with no parent and no dependents returns `"neighborhood": null`. Reference
objects reuse the `get_task` shape: `{ "id", "title", "status" }`.

```json
{
  "ok": true,
  "id": "task-1a2b3c4d",
  "status": "done",
  "message": "Completed: Implement graph status response (`task-1a2b3c4d`)",
  "neighborhood": {
    "parent": {
      "id": "epic-45",
      "title": "Enhance PKB MCP",
      "status": "in_progress",
      "siblings_open": 0,
      "siblings_sample": []
    },
    "children": { "closed_by_cascade": 3 },
    "unblocked": [
      { "id": "task-126", "title": "Deploy new MCP server" }
    ]
  }
}
```

### Fields

- **`parent`** — `{id, title, status}` of the parent, plus:
  - `siblings_open` — count of the parent's *other* children still open (partitioned by
    `graph::is_closed_for_hierarchy`, so `done`/`cancelled`/`archived` all count as
    closed). `siblings_open == 0` is the "this was the last open child — consider closing
    the parent" signal; the agent derives it rather than spending tokens on a prose hint.
  - `siblings_sample` — up to **3** open siblings as `{id, title, status}`. Omitted when
    `siblings_open == 0`. Completed siblings are never enumerated and carry no count — the
    agent's only sibling decisions are "close the parent?" (answered by `siblings_open`)
    and "pick up a sibling?" (answered by `siblings_sample`).

  Omitted entirely when the task has no parent.
- **`children`** — present only when first-generation descendants carry a fact the close
  did not already imply:
  - `closed_by_cascade` — number of descendants closed by `complete_task(recursive=true)`;
    confirms the blast radius of a cascade.
  - `open` — count of still-open children. Reachable only via `release_task` to a
    non-closing status (a normal `complete_task` refuses to close over open children), so
    it flags unresolved descendants the release left behind.

  Omitted when neither applies — i.e. the common leaf/clean close emits no `children`.
- **`unblocked`** — dependents that are no longer blocked because this task closed: each
  id in the closed node's `blocks` (the tasks that depend on it) that is not
  `is_blocked` and not `is_completed`, as `{id, title}`. Capped at **5**; the cap is
  silent because the count past 5 is rarely actionable in one turn. Omitted when empty.
  This is the highest-value "what next" field.

## Verbosity budget

The neighborhood replaces a round-trip, so it stays well under a round-trip's tokens.
The budget is enforced by construction:

1. **Counts over lists.** Open siblings past the cap, and all closed siblings, are an
   integer (`siblings_open`) or nothing — never enumerated.
2. **Hard caps.** `siblings_sample` ≤ 3; `unblocked` ≤ 5. These are the only detail
   lists. A task with 40 siblings adds at most 3 sibling rows.
3. **Omit-when-empty.** Every field disappears when it carries no signal; the common
   leaf-task close emits `"neighborhood": null`, and a clean close emits no `children`.
4. **No duplicated values.** Sibling state lives only on `parent`; there is no parallel
   `siblings` object restating the same counts.
5. **Three keys of detail, max.** Reference objects carry `id`, `title`, `status` (the
   `unblocked` rows drop `status`, which is implicitly "no longer blocked"). Titles are
   truncated for display at 80 chars on a UTF-8 char boundary (`str::floor_char_boundary`).
6. **No derived prose.** No hint strings, no rendered trees — the agent derives "close
   the parent" from `siblings_open == 0`.

Worst-case payload is `parent` (one object) + 3 sampled siblings + 5 unblocked rows — an
order of magnitude smaller than the `get_task` it replaces.

## Computation

The neighborhood is computed **after** the write and graph patch, so statuses,
`siblings_open`, and `unblocked` reflect the just-applied close. `rebuild_graph_for_pkb_document`
patches the node and runs `reclassify()` synchronously under the graph write lock, so a
fresh read after the write sees correct `status` and correct blocked/ready *membership*
(membership is recomputed inline; only derived metric *values* lag a background rebuild,
and the neighborhood reads no metric values):

1. Apply the status write and `rebuild_graph_for_pkb_document` (existing path).
2. Re-acquire the graph read lock and resolve the closed node.
3. `parent` / siblings: from `parent.children` minus the closed task, partitioned by
   `graph::is_closed_for_hierarchy(status)`; `siblings_sample` takes the first 3 open ones.
4. `children`: `closed_by_cascade` from the count of descendants the recursive path
   closed; `open` from `node.children` filtered by `!is_closed_for_hierarchy`.
5. `unblocked`: for each `dep_id` in `node.blocks`, include it when
   `!graph.is_blocked(dep_id) && !graph::is_completed(dep.status)`. `is_blocked` reads the
   post-write `blocked` set (a node is blocked while any blocker is unfinished), so a
   dependent appears here exactly when this close cleared its last unmet dependency —
   regardless of whether the dependent has been pre-staged to `ready`. The set is then
   truncated to 5.

Because `unblocked` is gated on `!is_blocked`, a `release_task` to a non-clearing status
(e.g. `blocked`) yields an empty `unblocked` — the released task still blocks its
dependents, so none clear.

## Non-goals

- Not returned by read tools or by `create_task` / `update_task` — those have their own
  surfaces (`get_task`, planner capture trees).
- Not a replacement for `pkb_context` / `get_dependency_tree`; no multi-hop traversal, no
  metrics, no backlinks.
- No `ready`-set coupling: `unblocked` is membership against the blocked set, not the
  claimable `ready` queue, so it fires for dependents at any active status, not only those
  pre-staged to `ready`/`queued`.
- No configurability/flags — the caps are fixed so callers can rely on a bounded payload.
