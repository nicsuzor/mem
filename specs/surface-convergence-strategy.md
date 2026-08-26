---
id: surface-convergence-strategy
title: "Strategy: PKB CLI/MCP Surface Convergence & Tool Reduction"
type: spec
status: review
tags: [spec, pkb, mcp, cli, surface-parity, tool-reduction, strategy]
created: 2026-08-26
depends_on: [pkb-server, crud-redesign, crud-audit]
contributes_to: [aops-789abb0d]
---

# Strategy: PKB CLI/MCP Surface Convergence & Tool Reduction

## Executive Summary & Context

This strategy addresses the mandate in `task_8404dbea` (derived from `kb_492f29d0`):
1. **Build feature parity** between the CLI (`pkb`) and MCP server surfaces.
2. **Drastically reduce the exposed MCP tool surface** (from 48 tools down toward the 18-tool architecture defined in `specs/pkb-server-spec.md`).
3. **Establish a clear architectural stance** on parity vs deliberate divergence (Decision D1) and spec governance (Decision D2).

This deliverable is the **reviewed strategy and capability specification**; no implementation code is modified in this pass, preserving the gate for Nic's review and sign-off.

---

## 1. Empirical Probes & Load-Bearing Evidence

### Probe F1: Usage Telemetry & Caller Code Classification
- **Question**: Is surface reduction a **deletion problem** (a long tail of unused tools) or a **merge problem** (folding actively used tools behind richer polymorphic parameters)?
- **Methodology**: 
  1. Live telemetry call counts and byte volumes were queried from the server via `pkb__get_stats` at `2026-08-26T03:13:33Z` (`[observed]`).
  2. An exhaustive regex search for all 48 MCP tool names across skills, agent definitions, plugins, and hooks was executed over `nicsuzor/academicOps` (at `origin/dev` `32bd31d1`) and `/home/worker/.gemini/config/plugins` (`[exhaustively-searched]`).
- **Findings**:
  - **Total MCP tools registered in source (`src/mcp_server.rs:7031-7078` at HEAD `41d82fe`)**: **48 tools**.
  - **Tools with active live telemetry (>0 calls)**: **36 tools** (top callers: `get_document` with 27,984 calls; `get_task` with 2,324 calls; `update_task` with 211 calls; `search` with 174 calls; `list_tasks` with 116 calls; `update_body` with 111 calls).
  - **Tools referenced directly by agent skills / hooks**: **45 tools**.
  - **Unused tools (0 telemetry calls and 0 caller references)**: Exactly **3 tools** (`diff_excalidraw`, `sync_excalidraw`, `get_consolidation_cluster`).
- **Conclusion**: Surface reduction is **strictly a MERGE problem**, not a deletion problem. A naive deletion strategy would break 45 active caller workflows. Drastic tool reduction requires consolidating related verbs into polymorphic tools with backward-compatible migration paths.

### Probe F2: Architectural Parity Analysis (Plumbing vs Core Logic)
- **Question**: Are CLI-to-MCP and MCP-to-CLI gaps rooted in missing core engine logic or adapter/parameter plumbing?
- **Methodology**: Traced three representative gaps across layers:
  1. `pkb new --due` (scalar passthrough).
  2. `pkb new --contributes-to` (structured edge mutation).
  3. `pkb focus` vs MCP ranking (command vs tool affordance).
- **Findings**:
  - `pkb new --due`: `document_crud::TaskFields` already contains `pub due: Option<String>` (`src/document_crud.rs:91`), and `create_task` already writes `due: ...` to YAML frontmatter (`src/document_crud.rs:287`). The CLI dispatch omitted `due` from `Commands::New` (`src/cli.rs:200`) and defaulted it via `..Default::default()` (`src/cli.rs:2016`). **Basis: Pure CLI clap plumbing gap.**
  - `pkb new --contributes-to`: `document_crud::TaskFields` already contains `pub contributes_to: Vec<serde_json::Value>` (`src/document_crud.rs:99`), and `create_task` already invokes `materialise_edge_inheritance` and serializes the edge list (`src/document_crud.rs:302-312`). MCP parses JSON directly (`src/mcp_server.rs:1803`), while CLI omitted the CLI parameter. **Basis: Pure CLI parsing plumbing gap.**
  - `pkb focus`: Both CLI `select_focus_picks` (`src/cli.rs:4411-4423`) and MCP `list_tasks` (`src/mcp_server.rs:5395`) share the exact same underlying `GraphStore::sort_by_focus` and `GraphStore::compute_focus_scores` in `src/graph_store.rs`.
- **Conclusion**: Both surfaces sit on an already unified Rust core (`src/document_crud.rs`, `src/graph_store.rs`, `src/graph.rs`). Parity gaps are entirely in adapter parameter bindings.

---

## 2. Two-Surface Capability Matrix

Audited from source at `nicsuzor/mem` HEAD `41d82fe` on `main`:

| Domain | Operation | CLI Command (`src/cli.rs`) | MCP Tool (`src/mcp_server.rs`) | Parity Status & Specific Gaps |
|---|---|---|---|---|
| **Search & Discovery** | Hybrid Search | `Search` (`:59`) | `search` (`:7031`) | CLI lacks `--boost-id` (proximity boost) and 3-tier `--detail` (has only boolean `--full`). |
| | Task Search | *(None)* (`tasks` is filter-only) | `task_search` (`:7034`) | CLI has no semantic search restricted to task types. |
| | Document Read | *(None)* (relies on editor/cat) | `get_document` (`:7032`) | MCP has byte truncation (`max_bytes`), CLI has no `show` equivalent for generic documents. |
| | Document Listing | `List` (`:183` spec only) | `list_documents` (`:7033`) | CLI lacks document listing command across non-task types (`aops_54dbbac6`). |
| | Memory Retrieval | `Recall` (`:404`), `Memories` (`:439`), `Tags` (`:418`) | `retrieve_memory` (`:7049`), `list_memories` (`:7052`), `search_by_tag` (`:7051`) | MCP exposes 3 separate memory query tools; CLI splits between `recall`, `memories`, `tags`. |
| | Semantic Neighbors | `Similarity` (`:603`) | `get_semantic_neighbors` (`:7059`) | Equivalent capability, different naming conventions. |
| | Deduplication | `Duplicates` (`:573`) | `find_duplicates` (`:7069`) | Parity achieved. |
| **Task Management** | Create Task | `New` (`:200`) | `create_task` (`:7037`) | CLI missing `--due`, `--contributes-to`, `--effort`, `--severity`, `--status`, `--stakeholder`, `--consequence`. |
| | Subtask Creation | `Subtask` (`:244`) | *(Merged into `create_task`)* | CLI supports dot-notation IDs (`parent.1`), MCP requires explicit parent field. |
| | Update Task | `Update` (`:325`) | `update_task` (`:7048`) | CLI missing `--depends-on`, `--contributes-to`, `--effort`, `--due`, `--severity`, `_add_*` deltas. |
| | Complete Task | `Done` (`:319`) | `complete_task` (`:7044`) | CLI lacks completion evidence validation flag (`--evidence`). |
| | Claim Task | *(None)* | `claim_task` (`:7038`) | CLI has no agent claim primitive. |
| | Release Task | *(None)* | `release_task` (`:7045`) | CLI has no agent release primitive. |
| | Decompose Task | *(None)* | `decompose_task` (`:7053`) | CLI has no multi-task batch insertion syntax. |
| | Task Listing / Triage | `Tasks` (`:132`), `Focus` (`:151`) | `list_tasks` (`:7046`), `task_summary` (`:7068`) | CLI formats rich ASCII tables; MCP returns raw JSON with filter options. |
| | Task Inspection | `Show` (`:159`) | `get_task` (`:7047`) | CLI lacks `--max-bytes` truncation. |
| | Dependencies / Blockers | `Deps` (`:167`), `Blocks` (`:451`) | `get_dependency_tree` (`:7054`), `get_task_children` (`:7055`) | CLI formats ASCII trees; MCP splits upstream tree and children into two tools. |
| **Document CRUD** | Create Generic Doc | `Remember` (`:257`) | `create` (`:7040`), `create_memory` (`:7039`) | Parity achieved; MCP has redundant `create_memory`. |
| | Append Content | `Append` (`:299`) | `append` (`:7041`) | Parity achieved (both support section targeting). |
| | Edit / Replace Body | *(None)* | `update_body` (`:7042`) | **Critical CLI defect (`aops_ad027e5c`)**: CLI cannot edit/replace body without prepending timestamps. |
| | Delete Document | `Delete` (`:313`), `Forget` (`:433`) | `delete` (`:7043`) | CLI splits delete by type (`delete` vs `forget`); MCP uses universal `delete`. |
| **Graph & Metrics** | Neighborhood Context | `Context` (`:351`) | `pkb_context` (`:7056`) | Parity achieved. |
| | Shortest Path Trace | `Trace` (`:361`) | `pkb_trace` (`:7057`) | Parity achieved. |
| | Orphan Detection | `Orphans` (`:374`) | `pkb_orphans` (`:7058`) | Parity achieved (both are type-aware). |
| | Network Metrics | `Metrics` (`:194`), `GraphStats` (`:542`) | `get_network_metrics` (`:7035`), `top_n_by_metric` (`:7036`), `graph_stats` (`:7063`), `detect_weight_divergence` (`:7050`) | MCP splits metrics across 4 discrete tools; CLI consolidates into `metrics` and `graph-stats`. |
| | Graph Export | `Graph` (`:385`) | `graph_json` (`:7064`) | CLI supports graphml/dot/json/excalidraw; MCP exposes only raw JSON. |
| | Graph Lifecycle | `Reindex` (`:87`) | `refresh_graph` (`:7076`) | Deliberate divergence: full reindex is CLI-only, lightweight refresh is MCP (`mem-fd66cb6e`). |
| **Canvas (Excalidraw)** | Visual Canvas | `Excalidraw` (`:531` Export/Diff/Sync) | `graph_excalidraw` (`:7065`), `diff_excalidraw` (`:7066`), `sync_excalidraw` (`:7067`) | MCP has 3 tools with near-zero usage; CLI consolidates under `pkb excalidraw <subcommand>`. |
| **Batch Operations** | Bulk Mutations | `Batch` (`:527` Update, Reparent, Archive, Merge, CreateEpics, Reclassify) | `batch_update` (`:7060`), `batch_reparent` (`:7061`), `batch_archive` (`:7062`), `batch_merge` (`:7070`), `batch_create_epics` (`:7072`), `batch_reclassify` (`:7073`), `merge_node` (`:7071`) | CLI groups into subcommands; MCP exposes 7 flat tools. |
| **System & Telemetry** | Diagnostics | `Status` (`:129`), `Stats` (`:596`) | `status` (`:7075`), `get_stats` (`:7074`) | **Name Collision**: CLI `pkb status` = index health; MCP `status` = binary build metadata. |

---

## 3. Structural Decision Evaluation (D1 & D2)

### Decision D1: Parity Model
- **Option A — Full Literal Parity**: Mirror every single command and tool across both surfaces.
  *Verdict*: **Rejected**. Contradicts the foundational two-user model (§Users in `specs/pkb-server-spec.md`). Humans need terse commands with rich terminal formatting; AI agents need structured data envelopes.
- **Option B — Capability Parity with Presentation Divergence [RECOMMENDED]**: A single shared Rust core provides 100% of mutation and query capabilities. CLI projects these into ergonomic subcommands with rich display; MCP projects these into a consolidated set of ~18 structured tools.
  *Verdict*: **Adopted**. Satisfies both halves of the user prompt: enables complete framework functionality on the CLI while shrinking the MCP surface by >60%.
- **Option C — Deliberate, Documented Divergence (Status Quo with Patches)**: Close only urgent bugs (`update_body`, missing `contributes_to`/`due` flags), leaving remaining differences open.
  *Verdict*: **Fallback only**. Leaves high cognitive friction and high agent token overhead in place.

### Decision D2: Authority of `specs/pkb-server-spec.md`
- **Option A — Ratify with Amended Inventory [RECOMMENDED]**: Formally ratify `specs/pkb-server-spec.md` from `inbox` to `ratified`, updating the 18-tool inventory table to match the polymorphic consolidation design.
- **Option B — Supersede with New Spec**: Archive `pkb-server-spec.md` and author a replacement.
  *Verdict*: **Rejected**. Re-litigates foundational user stories and architecture unnecessarily.

---

## 4. Distinct Surface Strategy Options

### Option 1: Incremental Patching & Conservative Cleanup (42 Tools)
* **Design**: Keep the existing flat MCP layout. Remove the 3 confirmed unused tools (`diff_excalidraw`, `sync_excalidraw`, `get_consolidation_cluster`) and fold the 3 redundant memory query tools (`list_memories`, `search_by_tag`, `retrieve_memory`) into `search` and `list_documents`. Add missing CLI clap arguments (`--due`, `--contributes-to`, `--effort`, `--severity`) and `pkb body-edit`.
* **Resulting MCP Surface**: **42 tools**.
* **Caller Migration Cost**: **Very Low**. 95% of existing skill calls remain unchanged.
* **What it Gives Up**: Fails the primary ask to drastically shrink the tool surface. Leaves high agent context overhead (~8,500 tokens of tool schemas in every agent prompt).

### Option 2: Polymorphic Facets & Unified Shared Core (18 Tools) [RECOMMENDED]
* **Design**: Group the 48 legacy MCP tools into **18 purposeful tools** aligned with the 6 functional domains in `specs/pkb-server-spec.md`. Consolidate operational variants using enum parameters and typed options:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        CONSOLIDATED MCP SURFACE                        │
│                               (18 TOOLS)                               │
├─────────────────┬──────────────────┬──────────────────┬────────────────┤
│ 1. Search & Doc │ 2. Task Lifecycle│ 3. Mutations     │ 4. Graph Intel │
│  - search       │  - list_tasks    │  - create        │  - graph_view  │
│  - get_document │  - get_task      │  - update        │  - graph_trace │
│  - list_docs    │  - claim_task    │  - update_body   │  - graph_stats │
│  - duplicates   │  - complete_task │  - delete        │  - batch_ops   │
│                 │  - release_task  │  - append        │  - canvas_ops  │
│                 │  - decompose_task│                  │  - system_info │
└─────────────────┴──────────────────┴──────────────────┴────────────────┘
```

#### Consolidated 18-Tool Inventory:
1. `search`: Universal search (hybrid semantic + graph boost via `boost_id`, type filtering for tasks/memories/notes, date filters). Consolidates `search`, `task_search`, `retrieve_memory`, `search_by_tag`.
2. `get_document`: Unified document fetcher by ID or path, with `detail` and `max_bytes` truncation.
3. `list_documents`: Universal catalog browse with filters (`type`, `tag`, `project`, `status`). Consolidates `list_documents` and `list_memories`.
4. `find_duplicates`: Semantic and title-based similarity clustering across nodes. Consolidates `find_duplicates` and `get_semantic_neighbors`.
5. `list_tasks`: Dedicated task state query tool with status filters (`ready`, `blocked`, `queued`, `in_progress`), sorting by `focus_score`, and inline blocker details.
6. `get_task`: Full task detail with complete relationship context (parent, children, blockers, dependents). Consolidates `get_task`, `get_dependency_tree`, `get_task_children`.
7. `claim_task`: Agent task claim primitive (in-place for tasks, instantiated for templates).
8. `complete_task`: Mark task done with mandatory non-whitespace `completion_evidence` validation.
9. `release_task`: Return task to queue or re-planning with failure reasons.
10. `decompose_task`: Structured multi-task creation under a parent with dependency resolution.
11. `create`: Universal creation verb with typed frontmatter validation (`type: task|memory|note|epic|...`). Consolidates `create`, `create_task`, `create_memory`.
12. `update`: Universal frontmatter update supporting scalar updates and special array operations (`_add_depends_on`, `_remove_tags`). Consolidates `update_task` and frontmatter edits.
13. `update_body`: Surgical document body replacement without timestamps.
14. `delete`: Safe deletion by ID with orphan checks.
15. `append`: Timestamped content addition with optional section header targeting.
16. `graph_context`: Subgraph traversal, ego-networks, and shortest path tracing. Consolidates `pkb_context` and `pkb_trace`.
17. `graph_metrics`: Topological metrics (PageRank, betweenness, downstream weight, weight divergence). Consolidates `get_network_metrics`, `top_n_by_metric`, `detect_weight_divergence`.
18. `batch_ops`: Unified batch mutation tool with `action: update|reparent|archive|merge|reclassify|create_epics` and `dry_run: true` default. Consolidates 7 separate `batch_*` tools.
19. `canvas_ops`: Excalidraw visual canvas generator. Consolidates `graph_excalidraw`, `diff_excalidraw`, `sync_excalidraw`.
20. `system_info`: Build info, health status, and tool telemetry. Consolidates `status` and `get_stats` (resolving the name collision).

* **Resulting MCP Surface**: **18–20 tools** (~60% reduction in exposed tool count; ~4,500 token context reduction).
* **Caller Migration Cost**: **Medium**. Handled smoothly via temporary server-side aliases during a 2-minor-version deprecation window.
* **What it Gives Up**: Requires updating prompt conventions and agent skill instructions over one coordinated release cycle.

### Option 3: Radical Micro-Core (10 Tools)
* **Design**: Compress the entire API down to 10 low-level CRUD and graph primitives (`search`, `get_node`, `query_nodes`, `mutate_node`, `claim_node`, `complete_node`, `graph_query`, `batch_mutate`, `canvas`, `system_info`).
* **Resulting MCP Surface**: **10 tools**.
* **Caller Migration Cost**: **Very High**. Completely breaks every existing agent workflow and skill template.
* **What it Gives Up**: Pushes extreme complexity into deeply nested polymorphic JSON parameters. Increases LLM tool call hallucination and schema validation error rates.

---

## 5. Critique & Lens Review (`wf-critique-lens`)

Selected Lenses: **Self-Consistency**, **Strategic Alignment**, **Assumption Hygiene**, and **Feasibility & Migration Safety**.

### Primary Concern (Feasibility & Migration Safety)
* **Concern**: Consolidating 48 tools into 18 tools risks breaking production agent workflows across the academicOps framework, as Probe F1 demonstrated that 45 tools are currently in active use.
* **Proposed Resolution**: Implement a **Two-Tier Compatibility Shim** inside `mcp_server.rs`:
  1. *Tier 1 (Core Layer)*: Implement the 18 clean, consolidated tool handlers.
  2. *Tier 2 (Compatibility Layer)*: Keep legacy tool names in `dispatch_tool_sync` as hidden aliases that rewrite legacy arguments into the new consolidated handlers and emit deprecation warnings to telemetry.
  3. Defer removing legacy aliases until academicOps skill templates are updated and telemetry confirms 0 legacy calls.

### Secondary Concern 1 (Strategic Alignment & ADHD Cognitive Load)
* **Concern**: Expanding the CLI to full capability parity risks bloating terminal `--help` menus and overwhelming human working memory.
* **Proposed Resolution**: Preserve terse CLI ergonomics. Provide smart defaults (`pkb new "Title"` requires only a string), while placing advanced graph and lifecycle flags under optional arguments (`--contributes-to`, `--due`). Group administrative verbs under dedicated subcommands (`pkb batch`, `pkb graph`).

### Secondary Concern 2 (Self-Consistency & The `status` Collision)
* **Concern**: CLI `pkb status` reports index health while MCP `status` reports build metadata. Agents invoking `status` receive version strings instead of knowledge base status.
* **Proposed Resolution**: Rename MCP `status` to `system_info` / `build_info` across all schemas. In the CLI, keep `pkb status` for index health and introduce `pkb version` for build identity.

---

## 6. Implementation Roadmap (Post-Approval)

```
┌───────────────────────────┐
│ Phase 0: Emergency Parity │ ➔ Fix CLI parity gaps: pkb new/update flags,
│   (Immediate, Low Risk)   │   add `pkb update-body` (aops_ad027e5c).
└─────────────┬─────────────┘
              ▼
┌───────────────────────────┐
│ Phase 1: Consolidated MCP │ ➔ Implement 18 consolidated tools in mem.
│  with Compatibility Shims │   Map 48 legacy names to aliases in dispatch.
└─────────────┬─────────────┘
              ▼
┌───────────────────────────┐
│ Phase 2: Skill Migration  │ ➔ Update academicOps skills & prompt templates
│      (Framework Wave)     │   to call the 18 consolidated tools directly.
└─────────────┬─────────────┘
              ▼
┌───────────────────────────┐
│ Phase 3: Deprecation Exit │ ➔ Monitor get_stats telemetry; retire legacy
│   (Zero-Downtime Clean)   │   compatibility aliases once call count hits 0.
└───────────────────────────┘
```

---

## 7. Recommendation

We recommend **Option 2 (Polymorphic Facets & Unified Shared Core)** under **D1 Option B** (Capability Parity, Presentation Divergence) and **D2 Option A** (Ratify `specs/pkb-server-spec.md` with amended 18-tool inventory). This achieves:
1. Complete capability parity on both surfaces.
2. A clean, modern 18-tool MCP surface fitting the spec's target.
3. A safe, zero-downtime migration path for all existing agent workflows.
