---
id: crud-audit
title: PKB MCP CRUD Surface Audit
type: spec
status: active
tags: [spec, pkb, mcp, crud, audit]
created: 2026-05-19
modified: 2026-05-19
depends_on: [pkb-server]
---

# PKB MCP CRUD Surface Audit

Evidence-driven audit of the PKB MCP server's CRUD tools. Feeds into [[crud-redesign]].

## Evidence Base

### Known incidents (from task body mem-e28634a5)

| ID | Error | Root cause class |
|----|-------|-----------------|
| aops-ac2df567 | `create_task` → `get_task` indexer bind failure — silent UX hazard | Race: post-create graph lookup can fail if parse_file returns None; caller gets INTERNAL_ERROR pointing at a file path |
| mem-dbf5a759 | `list_tasks(status=ready, project=aops)` returns empty while unfiltered works | Filter interaction bug in graph; requires PKB data to reproduce fully |
| mem-2ae61ce6 | PKB session reaping on 60s keep-alive | Session/transport issue, not CRUD |
| epic-a0523a25 (cycle-15) | `update_task(updates={"_add_depends_on": [...]})` silently writes inert frontmatter | Special keys only handled in batch path, not singular update path |

### Bugs confirmed from source (this audit)

See § Critical Bugs below. All have file:line citations.

### AC#1 gap

Full transcript-based error catalogue (30-day frequency, error class counts) requires PKB location via context-map.json, which is missing from this repo. The incidents above are the known evidence base. Full AC#1 requires a separate evidence-gathering pass by a session with PKB access.

---

## Tool Surface Delta

The [pkb-server spec](pkb-server-spec.md) targets **18 tools**. The server currently registers **39 tools**.

| Category | Tools |
|----------|-------|
| Original 18 (spec-aligned) | search, task_search, get_document, list_documents, list_tasks, get_task, get_network_metrics, create_task, update_task, complete_task, create (doc), create_memory, append, delete, pkb_context, pkb_trace, pkb_orphans, graph_stats |
| Added since spec | create_subtask, release_task, decompose_task, find_duplicates, get_dependency_tree, get_task_children, task_summary, claim_task, batch_update, batch_reparent, batch_archive, batch_merge, batch_create_epics, batch_reclassify, bulk_reparent, merge_node, delete_memory, retrieve_memory, search_by_tag, list_memories |
| Consolidated wrappers | create_document, manage_task, pkb_explore, pkb_batch, pkb_stats, pkb_tool_help |

The 21 post-spec additions are legitimate capability growth. The 6 consolidation wrappers are redundant meta-tools that re-expose sub-surfaces under new names.

---

## Per-Tool Audit

Criteria rated P (pass) / W (warn) / F (fail):
- **Predictable**: does what the name says, no hidden side-effects
- **Idempotent**: safe to retry
- **Fail-fast**: rejects bad input before any write
- **Schema clarity**: parameters named consistently, required vs optional clear
- **Ergonomic defaults**: defaults reduce common-case friction

### create_task

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | Returns full task via get_task(); if post-write graph lookup fails, INTERNAL_ERROR with file path — surprising error shape |
| Idempotent | F | Two calls with same title create two tasks; no upsert semantics |
| Fail-fast | W | Unknown fields logged as WARN but silently accepted; typos in field names never error (mcp_server.rs:1289-1296) |
| Schema clarity | W | `title` / `task_title` alias: two names for same field. `allow_missing_parent` / `force` override params buried in docs |
| Ergonomic defaults | P | Minimal required fields (title + project); auto-routes by type |

**Source**: `src/mcp_server.rs:1243-1523`, `src/document_crud.rs:414-655`

### create_subtask

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | Returns plain text "Sub-task created: `{id}` at `{path}`" — inconsistent with create_task which returns full JSON |
| Idempotent | F | No dedup |
| Fail-fast | P | Parent existence + project field both validated hard |
| Schema clarity | W | Accepts far fewer fields than create_task; no way to set priority, depends_on, etc. at creation |
| Ergonomic defaults | P | Minimal: parent_id + title |

**Source**: `src/mcp_server.rs:1525-1615`

### update_task

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | **F** | `_add_depends_on`, `_remove_depends_on`, `_add_tags`, `_remove_tags` documented in batch_update but silently persist as raw frontmatter when passed to update_task. No error. (mcp_server.rs:4533-4537, document_crud.rs:1082) |
| Idempotent | W | Setting same status twice: second write is a no-op on disk but still does full file rewrite + graph rebuild |
| Fail-fast | W | Null parent silently clears parent with no validation (mcp_server.rs:4412-4413); allows potential orphaning |
| Schema clarity | W | Two accepted forms (nested `updates:{}` vs flat params) — flexible but undocumented in tool description |
| Ergonomic defaults | P | Flat form allows `update_task(id=x, status=done)` without wrapping in updates object |

**The `_add_depends_on` footgun is the most serious bug in the entire CRUD surface.** Agents that read batch_update docs and assume the same keys work in update_task will silently corrupt task files.

**Source**: `src/mcp_server.rs:4356-4598`, `src/batch_ops/update.rs:359-365`

### complete_task

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | Recursive close logs WARN and continues on descendant failure — some descendants may not close; caller gets success |
| Idempotent | W | Re-completing a done task: no explicit guard; depends on open_descendants check |
| Fail-fast | W | `completion_evidence` check uses `.trim().is_empty()` — whitespace-only ("   ") passes (mcp_server.rs:4510) |
| Schema clarity | P | Clear params: id, completion_evidence (required), recursive |
| Ergonomic defaults | P | recursive=false is safe default |

**Source**: `src/mcp_server.rs:2725-2831`

### release_task

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | No id → ad-hoc task auto-created (mcp_server.rs:2916-2923). Non-obvious; creates permanent artifact as side effect |
| Idempotent | W | Re-releasing a task that's already done/cancelled returns error; merge_ready + re-release is not tested |
| Fail-fast | P | Status enum validated with typo suggestions; summary required; follow_up_tasks existence validated |
| Schema clarity | W | Many optional params (pr_url, branch, blocker, reason, session_id, issue_url, follow_up_tasks, release_summary) with undifferentiated semantics |
| Ergonomic defaults | P | Ad-hoc creation handles the "I worked on something untracked" case |

**Source**: `src/mcp_server.rs:2914-3200`

### decompose_task

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | Unresolved sibling cross-references ($2, title-based) silently ignored — subtask created with broken depends_on (mcp_server.rs:3659-3672) |
| Idempotent | F | Re-decomposing same parent: duplicate title check blocks it, but partial failure leaves some subtasks created |
| Fail-fast | P | Two-pass validation catches duplicate titles, IDs, parent issues before write |
| Schema clarity | P | parent_id + subtasks array is clear |
| Ergonomic defaults | P | Minimal required fields per subtask |

**Source**: `src/mcp_server.rs:3516-3800`

### append

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | P | Timestamped append is explicit in docs; section creation documented |
| Idempotent | **F** | Every call adds a new timestamped line — retry creates duplicates. No hash/dedup check (document_crud.rs:1138) |
| Fail-fast | P | id + content required; deprecated path param explicitly rejected |
| Schema clarity | P | Clear: id, content, section (optional) |
| Ergonomic defaults | P | Section optional; appends to end of file if not specified |

**Source**: `src/mcp_server.rs:2617-2681`, `src/document_crud.rs:1112-1178`

### delete / delete_memory

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | No orphan check — children and inbound references left dangling silently |
| Idempotent | W | Deleting already-deleted: graph.resolve() fails → INVALID_PARAMS — not an explicit "not found" |
| Fail-fast | W | No check for incoming references or open children |
| Schema clarity | P | Single required param: id |
| Ergonomic defaults | N/A | Destructive op — no defaults needed |

**delete_memory** adds type check (must be note/insight/observation) — better than plain delete.

**Source**: `src/mcp_server.rs:2683-2723`, `src/document_crud.rs:1183-1198`

### batch_update

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | P | Correctly handles `_add_depends_on` / `_remove_depends_on` / `_add_tags` / `_remove_tags` (batch_ops/update.rs:359-365) |
| Idempotent | P | Apply same update twice: second write is a field-level set |
| Fail-fast | P | Pre-validates type/priority/effort enums before iterating; dry_run default false |
| Schema clarity | P | Special keys documented in tool description |
| Ergonomic defaults | W | dry_run=false is dangerous default for bulk operation; contrast with batch_archive (dry_run=true) — inconsistent |

**Source**: `src/mcp_server.rs:4604-4627`, `src/batch_ops/update.rs`

### batch_archive

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | P | dry_run=true by default — explicit opt-in to destructive action |
| Idempotent | P | Archiving already-archived tasks is harmless |
| Fail-fast | P | Requires at least one filter |
| Schema clarity | P | dry_run default documented |
| Ergonomic defaults | **P** | **Best default in the surface** — safe-by-default destructive op |

Contrast with batch_update (dry_run=false) and merge_node (dry_run=false). **batch_archive should be the model for all bulk destructive operations.**

### bulk_reparent (legacy)

| Criterion | Rating | Notes |
|-----------|--------|-------|
| Predictable | W | Glob/path pattern semantics unclear vs batch_reparent's filter semantics |
| Idempotent | P | Reparenting already-correct parent is a no-op |
| Fail-fast | P | dry_run=true default |
| Schema clarity | W | Overlaps with batch_reparent; unclear when to use which |
| Ergonomic defaults | P | dry_run=true default |

---

## Critical Bugs (Fix Required)

### Bug 1: `_add_depends_on` / `_add_tags` silent-persist in update_task

**Severity**: HIGH  
**File**: `src/mcp_server.rs:4533-4537`, `src/document_crud.rs:1082`  
**Symptom**: `update_task(id=X, updates={"_add_depends_on": ["Y"]})` writes `_add_depends_on: ["Y"]` as raw frontmatter. Graph never picks it up as a dependency edge.  
**Root cause**: `update_task` passes the updates map directly to `update_document()`, which does `fm.insert(key, value)` for all keys (line 1082). The special-key expansion logic only exists in `batch_ops/update.rs:359-365`.  
**Fix**: Either (a) expand special keys in `update_document()` so they work everywhere, or (b) handle them in `handle_update_task()` before calling `update_document()`.

### Bug 2: Unknown fields silently accepted in create_task

**Severity**: MEDIUM  
**File**: `src/mcp_server.rs:1289-1296`  
**Symptom**: Typos in create_task parameters (e.g. `prioriyt`, `depens_on`) are warned in server logs but not surfaced to the caller. The task is created with missing fields.  
**Fix**: Return INVALID_PARAMS on unrecognised fields, or at minimum include warnings in the success response.

### Bug 3: Whitespace-only completion_evidence accepted

**Severity**: MEDIUM  
**File**: `src/mcp_server.rs:4510`, `src/mcp_server.rs:2735`  
**Symptom**: `completion_evidence="   "` passes the `.trim().is_empty()` check.  
**Fix**: Use `evidence.split_whitespace().any(|_| true)` or `.chars().any(|c| !c.is_whitespace())`.

### Bug 4: Recursive close swallows descendant failures

**Severity**: MEDIUM  
**File**: `src/mcp_server.rs:2789`, `src/mcp_server.rs:3075`  
**Symptom**: When recursively closing a task tree, if any descendant write fails, the error is logged at WARN and skipped. Caller gets success; some tasks remain open.  
**Fix**: Collect all failures and return them in the response (not as an error, but as a partial-failure summary); or fail fast and roll back.

### Bug 5: Unresolved sibling references in decompose_task silently ignored

**Severity**: MEDIUM  
**File**: `src/mcp_server.rs:3659-3672`  
**Symptom**: `decompose_task` with `{"depends_on": ["$3"]}` where $3 doesn't exist creates the subtask with the broken reference unchanged.  
**Fix**: Return INVALID_PARAMS for unresolvable references, or surface them as warnings in the response.

### Bug 6: Null parent clears parent without validation in update_task

**Severity**: MEDIUM  
**File**: `src/mcp_server.rs:4412-4413`  
**Symptom**: `update_task(id=X, updates={"parent": null})` silently unparents the task. No orphan check, no confirmation.  
**Fix**: Require explicit `force=true` or a different parameter (e.g., `unparent=true`) for intentional orphaning.

---

## Design Pattern Issues

### Inconsistent dry_run defaults across batch operations

| Tool | dry_run default |
|------|----------------|
| batch_update | false |
| batch_reparent | false |
| batch_archive | **true** |
| bulk_reparent | true |
| merge_node | false |
| batch_merge | false |

Operators expect consistent defaults. All bulk destructive operations should default to dry_run=true.

### Inconsistent success response shapes

| Tool | Success shape |
|------|--------------|
| create_task | Full JSON (via get_task) |
| create_subtask | Plain text |
| create_memory | Plain text |
| complete_task | Plain text |
| release_task | JSON object |
| batch_* | JSON summary |

Agents must branch on tool choice to parse results. A uniform envelope would reduce hallucinated parse logic.

### Tool surface fragmentation (6 consolidated wrappers)

`create_document`, `manage_task`, `pkb_explore`, `pkb_batch`, `pkb_stats`, `pkb_tool_help` re-expose existing tools under different names. This creates two parallel surfaces that must be kept in sync — a maintenance burden with no capability gain.

### bulk_reparent vs batch_reparent overlap

Both reparent tasks. `bulk_reparent` uses glob/path patterns; `batch_reparent` uses structured filters. No documented guidance on when to use which. Legacy comment in source.

### decompose_task vs create_subtask vs create_task

Three tools for creating child tasks. No documented hierarchy of when to use each.

---

## Compliance With US4 ("Guaranteed Task Integrity")

> All CRUD operations work reliably 100% of the time. Format consistency enforced. Fail-fast on malformed data. No silent failures.

| Requirement | Status |
|------------|--------|
| All CRUD reliable 100% | W — recursive close silently drops failures |
| Format consistency enforced | P — YAML validation on all writes |
| Fail-fast on malformed data | F — unknown fields silently accepted in create_task; `_add_depends_on` silent corruption |
| No silent failures | F — 6 confirmed silent failure modes (see § Critical Bugs) |

The surface fails US4 on two of four requirements.
