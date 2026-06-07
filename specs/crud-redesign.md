---
id: crud-redesign
title: PKB MCP CRUD Surface Redesign
type: spec
status: inbox
tags: [spec, pkb, mcp, crud, redesign]
created: 2026-05-19
modified: 2026-05-19
depends_on: [crud-audit, pkb-server]
---

# PKB MCP CRUD Surface Redesign

**Status**: Draft — needs review by pauli + rbg (and ideally james) before implementation.

Based on [[crud-audit]]. Addresses confirmed bugs and design issues. Does not require transcript data for the structural changes; AC#1 frequency data may prompt additional changes after the full evidence pass.

## Design Principles (reaffirmed)

1. **Fail-fast, never silent**: any write that cannot be completed cleanly returns an error. No warn-and-continue for data-integrity paths.
2. **Predictable surface**: same key names work the same way regardless of which tool you use. No hidden special-key semantics that only work in one path.
3. **Safe-by-default bulk ops**: all bulk destructive operations default to dry_run=true.
4. **Uniform response envelope**: all write tools return the same shape.
5. **One tool per operation**: no wrapper tools that re-expose other tools under different names.

---

## Bug Fixes (P0 — ship regardless of redesign)

These are correctness bugs that should be fixed independently of any surface changes.

### Fix 1: Expand special keys in update_task

`update_task` must handle `_add_depends_on`, `_remove_depends_on`, `_add_tags`, `_remove_tags` identically to `batch_update`.

**Implementation**: Extract the special-key expansion from `batch_ops/update.rs` into a shared helper in `document_crud.rs` (e.g., `expand_special_update_keys(node: &GraphNode, updates: Map) -> Result<Map, McpError>`). Call it from both `handle_update_task` and `batch_update` before invoking `update_document`. The helper must return `Err(McpError::invalid_params(...))` for malformed values — e.g., `_add_depends_on` value that is not a string or array of strings, `_add_tags` value that is not a string or array of strings. Callers propagate the error immediately without writing.

**Verification**: `update_task(id=X, updates={"_add_depends_on": ["Y"]})` must result in Y appearing in X's `depends_on` frontmatter field; graph must reflect the DependsOn edge after Tier-1 rebuild.

### Fix 2: Reject unknown fields in create_task

Unknown keys in `create_task` must return INVALID_PARAMS, not a warning.

**Implementation**: Change `src/mcp_server.rs:1289-1296` to return an error instead of `tracing::warn!`. Include the unknown key name and a suggestion hint in the message.

**Exception**: `allow_missing_parent` and `force` are override params that live outside the updates map — these are routing keys and must remain accepted.

### Fix 3: Strengthen completion_evidence validation

Replace `.trim().is_empty()` with a check that requires at least one non-whitespace character.

**Implementation**: `evidence.chars().any(|c| !c.is_whitespace())` or equivalent. Apply consistently to both `complete_task` (mcp_server.rs:2735) and `update_task` (mcp_server.rs:4510).

### Fix 4: Return partial-failure details on recursive close

When `complete_task` or `release_task` with `recursive=true` fails to close one or more descendants, include the failed IDs in the response rather than swallowing.

**Implementation**: Collect `Vec<(String, String)>` of (id, error) during the recursive loop; include them in the success message as a "partial_failures" field if non-empty.

### Fix 5: Reject unresolvable sibling references in decompose_task

`decompose_task` should return INVALID_PARAMS (before any write) for unresolvable `$N` or title-based sibling references.

**Implementation**: Change the silent-ignore path at `mcp_server.rs:3659-3672` to accumulate errors and return before the write phase.

### Fix 6: Require explicit opt-in for null-parent in update_task

Passing `parent: null` (or empty string) to `update_task` must require `unparent: true` or `force: true`.

**Implementation**: In `handle_update_task`, detect `parent` being null/empty. If no explicit unparent flag is set, return INVALID_PARAMS: "To remove a task's parent, pass unparent=true explicitly."

---

## Surface Changes (Needs Review)

These are structural changes to the tool surface. **Do not implement without pauli + rbg sign-off.**

### S1: Consistent dry_run defaults — all bulk ops default to true

**Status**: shipped in #394 (commit 6a2a842)

Change `batch_update`, `batch_reparent`, `merge_node`, and `batch_merge` to default `dry_run=true`.

**Rationale**: `batch_archive` and `bulk_reparent` already default to true. Inconsistency creates operational risk. The downside is one extra explicit parameter for callers who want live execution, which is a small cost for preventing accidental bulk mutations.

**Migration**: Callers currently relying on `dry_run=false` default for `batch_update` must add `dry_run: false` explicitly. Update all known call sites in academicOps and plugins.

### S2: Uniform success response envelope

**Status**: deferred, gated on AC#1 frequency data

All write tools return a JSON object with:

```json
{
  "id": "...",
  "status": "ok",
  "message": "...",
  "partial_failures": [],
  "warnings": []
}
```

Tools that currently return full task data (create_task) include it under a `data` key:

```json
{
  "id": "...",
  "status": "ok",
  "message": "Task created",
  "data": { ... },
  "partial_failures": [],
  "warnings": []
}
```

**Rationale**: Agents currently branch on which tool they called to know how to parse the result. Uniform envelope lets them handle success/warning/partial-failure consistently.

**Migration**: Callers parsing plain-text responses (create_subtask, complete_task, etc.) must update to read the envelope.

### S3: Retire the 6 consolidation wrapper tools

**Status**: shipped in #394 (commit 6a2a842)

Remove `create_document`, `manage_task`, `pkb_explore`, `pkb_batch`, `pkb_stats`, `pkb_tool_help` from the registered tool list.

**Rationale**: These re-expose existing tools with no new capability. They create a maintenance burden (two surfaces to keep in sync) and confuse agents about which tool to call.

**Migration**: Callers using wrapper tools must switch to the underlying tools. The wrappers exist in mcp_server.rs and can be removed by deleting the Tool registrations and handler branches.

### S4: Retire bulk_reparent in favour of batch_reparent

**Status**: shipped in #394 (commit 6a2a842)

`bulk_reparent` (glob/path pattern) is a legacy operation predating `batch_reparent` (structured filters). Remove it.

**Migration**: Any caller using `bulk_reparent` with glob patterns should use `batch_reparent` with `subtree` or `parent` filters instead.

### S5: Merge create_subtask into create_task

**Status**: shipped in #394 (commit 6a2a842)

`create_subtask` accepts fewer fields than `create_task` and returns a different response shape. Remove it; `create_task` with `parent=<id>` is used instead, though it is not fully equivalent: dot-notation `{parent}.{n}` numbered IDs survive only via the CLI path.

**Migration**: Callers of `create_subtask(parent_id=X, title=Y)` switch to `create_task(parent=X, title=Y)`.

### S6: Add `_add_depends_on` et al. to update_task tool description

Once Bug Fix 1 is shipped, update the `update_task` tool description to explicitly document `_add_depends_on`, `_remove_depends_on`, `_add_tags`, `_remove_tags` as supported special keys. Currently these are only mentioned in `batch_update`.

---

## Tool Inventory After Redesign

Target: 42 tools (down from 50; 18 core spec + task_search, find_duplicates, release_task, decompose_task, get_dependency_tree, get_task_children, task_summary, claim_task, batch_*, delete_memory, retrieve_memory, search_by_tag, list_memories, get_semantic_neighbors, update_body, detect_weight_divergence, graph_json, get_stats, status).

| # | Tool | Change |
|---|------|--------|
| 1 | search | keep |
| 2 | task_search | keep |
| 3 | get_document | keep |
| 4 | list_documents | keep |
| 5 | list_tasks | keep |
| 6 | get_task | keep |
| 7 | get_network_metrics | keep |
| 8 | create_task | keep + fix unknown-field rejection + fix _add_depends_on in same pass |
| 9 | update_task | keep + fix _add_depends_on + fix null-parent + uniform envelope |
| 10 | complete_task | keep + fix whitespace evidence + fix partial-failure reporting |
| 11 | release_task | keep + fix partial-failure reporting |
| 12 | decompose_task | keep + fix unresolved-ref rejection |
| 13 | claim_task | keep |
| 14 | create (doc) | keep |
| 15 | create_memory | keep |
| 16 | append | keep (non-idempotent — document clearly) |
| 17 | delete | keep + document orphan risk |
| 18 | delete_memory | keep |
| 19 | retrieve_memory / search_by_tag / list_memories | keep |
| 20 | pkb_context | keep |
| 21 | pkb_trace | keep |
| 22 | pkb_orphans | keep |
| 23 | graph_stats | keep |
| 24 | get_dependency_tree | keep |
| 25 | get_task_children | keep |
| 26 | task_summary | keep |
| 27 | find_duplicates | keep |
| 28 | get_semantic_neighbors | keep |
| 29 | batch_update | keep + dry_run default → true |
| 30 | batch_reparent | keep + dry_run default → true |
| 31 | batch_archive | keep |
| 32 | batch_merge | keep + dry_run default → true |
| 33 | merge_node | keep + dry_run default → true |
| 34 | batch_create_epics | keep |
| 35 | batch_reclassify | keep |
| — | create_subtask | **remove** (S5) |
| — | create_document | **remove** (S3) |
| — | manage_task | **remove** (S3) |
| — | pkb_explore | **remove** (S3) |
| — | pkb_batch | **remove** (S3) |
| — | pkb_stats | **remove** (S3) |
| — | pkb_tool_help | **remove** (S3) |
| — | bulk_reparent | **remove** (S4) |

Net: 42 tools (from 50). The deletions are consolidation wins; the remaining tools gain correctness.

---

## Migration Plan

### Phase 0: Bug fixes (P0, no surface change, no migration needed)

Fix 1–6 above. No caller impact except Fix 2 (unknown fields now error — callers with typos will surface failures they were previously silently eating).

### Phase 1: Tool description updates

Update `batch_update` and `update_task` tool descriptions to document special keys. No code change needed (Fix 1 already ships the behaviour).

### Phase 2: Surface reduction (needs sign-off first)

Remove wrapper tools (S3), bulk_reparent (S4), create_subtask (S5).

**Caller audit needed**: Search academicOps and plugins for calls to: `create_document`, `manage_task`, `pkb_explore`, `pkb_batch`, `pkb_stats`, `pkb_tool_help`, `bulk_reparent`, `create_subtask`. Update each call site before removing the tools.

### Caller audit — completed

The caller audit itself is completed and tracked via sibling epic-a99e1bf7.

### Phase 3: dry_run defaults (S1), uniform envelope (S2)

These are breaking changes for automated callers. Coordinate with aops framework sessions before shipping.

---

## Open Questions for Review

1. **Uniform envelope (S2)**: Is the complexity of changing all response shapes worth the consistency gain? Alternative: document current shapes and don't change them.
2. **create_subtask removal (S5)**: Are there callers that specifically need the "numbered subtask" auto-naming behaviour that create_task doesn't provide?
3. **dry_run defaults (S1)**: Are any current aops workflows relying on batch_update executing without dry_run=false? Impact could be widespread.
4. **append idempotency**: Should append grow a `dedup_key` parameter to prevent duplicate entries on retry? Or is the non-idempotent behaviour correct and just needs better documentation?
5. **AC#1 gap**: Full transcript data needed to verify frequency of each bug class. The evidence base here covers known incidents; the redesign may need adjustment after the full frequency analysis.
