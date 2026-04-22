# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.10](https://github.com/nicsuzor/mem/compare/v0.3.9...v0.3.10) - 2026-04-22

### Fixed

- fix compile

### Other

- T9 CLEANUP (mem): Update release_task tool description and CORE.md ([#213](https://github.com/nicsuzor/mem/pull/213))

## [0.3.9](https://github.com/nicsuzor/mem/compare/v0.3.8...v0.3.9) - 2026-04-21

### Added

- *(lint)* replace task-no-parent type check with scope-aware severity ([#211](https://github.com/nicsuzor/mem/pull/211))
- status filter bar + node colouring by status/project ([#209](https://github.com/nicsuzor/mem/pull/209))
- *(mcp)* release_task writes session_id, issue_url, and follow-ups ([#207](https://github.com/nicsuzor/mem/pull/207))
- rename ForceViewV2 → GroupsView, drop MetroViewV2, add legend counts
- *(mcp)* enrich tool exposures with annotations and titles ([#198](https://github.com/nicsuzor/mem/pull/198))
- *(mcp)* add prompts for essential search patterns ([#199](https://github.com/nicsuzor/mem/pull/199))
- *(lint)* replace task-no-parent type check with scope-aware severity ([#195](https://github.com/nicsuzor/mem/pull/195))
- propagate effective_priority through graph; fix list_tasks filter ([#204](https://github.com/nicsuzor/mem/pull/204))
- add project filter parameter to list_tasks MCP tool ([#193](https://github.com/nicsuzor/mem/pull/193))
- add ForceViewV2 and MetroViewV2, slim TreemapView, update routing
- *(mcp)* add release_task tool for structured task handoff
- add goals field to frontmatter schema ([#177](https://github.com/nicsuzor/mem/pull/177))
- *(dashboard)* split Force/Metro views, add recency emphasis and progressive labels
- *(dashboard)* organic force layout and tri-state filters
- *(mcp)* switch get_document to ID-based lookup ([#175](https://github.com/nicsuzor/mem/pull/175))
- *(dashboard)* overhaul force map epic grouping and layout ([#168](https://github.com/nicsuzor/mem/pull/168))
- *(graph)* add computed properties scope, uncertainty, criticality ([#172](https://github.com/nicsuzor/mem/pull/172))
- *(ci)* add gemini workflows and commands
- *(graph)* edge-typed cycle detection via Tarjan's SCC ([#173](https://github.com/nicsuzor/mem/pull/173))
- unify type system — collapse to 4 actionable types ([#169](https://github.com/nicsuzor/mem/pull/169))
- add stakeholder waiting urgency to focus scoring ([#167](https://github.com/nicsuzor/mem/pull/167))

### Fixed

- *(mcp)* support contributes_to and other complex frontmatter fields ([#210](https://github.com/nicsuzor/mem/pull/210))
- fix dash
- *(mcp)* restore compilation on main — CI has been red for 3+ days ([#208](https://github.com/nicsuzor/mem/pull/208))
- *(mcp)* accept project/type/status in create_task and return structured JSON ([#194](https://github.com/nicsuzor/mem/pull/194))
- *(mcp)* incremental graph update optimizations and ID change handling ([#200](https://github.com/nicsuzor/mem/pull/200))
- fix leak svg
- fixtiny treemap
- fix tiny treemap
- *(dashboard)* resolve build errors and optimize structural collapse logic
- *(dashboard)* resolve ForceView constraints and add randomize functionality ([#187](https://github.com/nicsuzor/mem/pull/187))
- *(mcp)* skip vector index update when reindex holds the lock ([#186](https://github.com/nicsuzor/mem/pull/186))
- *(mem)* filter body from YAML frontmatter writes in update_document() ([#183](https://github.com/nicsuzor/mem/pull/183))
- *(dashboard)* restore named constants and intermediates in ForceView ([#181](https://github.com/nicsuzor/mem/pull/181))
- *(mcp)* improve update_task tool description with type warning
- *(dashboard)* remove unused FORCE_CONFIG import
- *(dashboard)* restore webcola grouping constraints
- *(dashboard)* remove data fallbacks, enforce fail-fast pipeline ([#176](https://github.com/nicsuzor/mem/pull/176))

### Other

- T4 IMPL (mem): release_task auto-creates ad-hoc task when no id bound ([#212](https://github.com/nicsuzor/mem/pull/212))
- typo
- *(mem)* YAML frontmatter extension for session handover ([#206](https://github.com/nicsuzor/mem/pull/206))
- Radically simplify MCP tool surface + docs ([#197](https://github.com/nicsuzor/mem/pull/197))
- Add usage telemetry to MCP server and pkb stats CLI command ([#202](https://github.com/nicsuzor/mem/pull/202))
- Compute project field from nearest project ancestor and ignore frontmatter ([#201](https://github.com/nicsuzor/mem/pull/201))
- integrate extra branch
- partially clarify metro
- overflow
- ui changes
- update ui
- resume don't reset
- ui
- rename
- metro
- adjustmnets
- groups work!
- layout works
- force graph showing again
- recover crash
- *(dashboard)* radically simplify ForceView — remove edges, hover, seeding (731→370 lines)
- Add effort and consequence fields to PKB data model ([#189](https://github.com/nicsuzor/mem/pull/189))
- Wire 'due' through create_task and handle_decompose_task in PKB server ([#188](https://github.com/nicsuzor/mem/pull/188))
- add user expectations to planning-web umbrella spec ([#185](https://github.com/nicsuzor/mem/pull/185))
- *(dashboard)* radically simplify ForceView (731→370 lines) ([#184](https://github.com/nicsuzor/mem/pull/184))
- *(dashboard)* simplify ForceView — extract helpers, remove dead code ([#178](https://github.com/nicsuzor/mem/pull/178))
- install PR reviewer agent with fix authority + runtime axiom loading (PR #465) ([#174](https://github.com/nicsuzor/mem/pull/174))
- read agent prompts from origin/main, not the PR branch ([#171](https://github.com/nicsuzor/mem/pull/171))
- replace pr-reviewer with three-agent review suite ([#170](https://github.com/nicsuzor/mem/pull/170))

## [0.3.8](https://github.com/nicsuzor/mem/compare/v0.3.7...v0.3.8) - 2026-04-02

### Added

- *(mcp)* add graph_json tool and update overwhelm-dashboard ([#164](https://github.com/nicsuzor/mem/pull/164))
- *(dashboard)* manhattan routing, red dep edges, project-colored epics, overlap fixes
- switch dashboard to HTTP MCP transport ([#161](https://github.com/nicsuzor/mem/pull/161))
- *(dashboard)* project colors, grouped sessions, clickable tasks, curved edges ([#160](https://github.com/nicsuzor/mem/pull/160))
- *(dashboard)* legend filters, project colors, refile button, lineage tree, metro priority
- *(dashboard)* project colors, grouped sessions, clickable tasks, curved edges ([#158](https://github.com/nicsuzor/mem/pull/158))
- *(dashboard)* unified label sizing, tighter packing, and type error fixes
- *(dashboard)* task editor action buttons ([#157](https://github.com/nicsuzor/mem/pull/157))

### Fixed

- *(dashboard)* QA audit fixes and spec compliance ([#163](https://github.com/nicsuzor/mem/pull/163))
- *(dashboard)* restore projectHue definition removed by circular import

### Other

- "Claude PR Assistant workflow" ([#165](https://github.com/nicsuzor/mem/pull/165))
- *(dashboard)* address PR 158 review comments ([#159](https://github.com/nicsuzor/mem/pull/159))
- graph improvements

## [0.3.7](https://github.com/nicsuzor/mem/compare/v0.3.6...v0.3.7) - 2026-03-28

### Added

- *(dashboard)* adjust treemap font sizes and add task editor status buttons
- *(dashboard)* collapse 1:1 containers into single child; add custom attractive force for epics
- *(mcp)* add HTTP/SSE transport for Cowork VM ([#151](https://github.com/nicsuzor/mem/pull/151))

### Fixed

- deny println! in library code to protect MCP transport ([#150](https://github.com/nicsuzor/mem/pull/150))
- *(dashboard)* visual hierarchy and blocked/dependency styling ([#149](https://github.com/nicsuzor/mem/pull/149))
- *(dashboard)* QA improvements to overwhelm dash main page ([#147](https://github.com/nicsuzor/mem/pull/147))

### Other

- add axiom-driven PR reviewer ([#153](https://github.com/nicsuzor/mem/pull/153))
- MCP integration tests for stdio and HTTP/SSE transports ([#154](https://github.com/nicsuzor/mem/pull/154))
- update dash and delete test files
- qa checkin

## [0.3.6](https://github.com/nicsuzor/mem/compare/v0.3.5...v0.3.6) - 2026-03-27

### Added

- *(dashboard)* overhaul force graph visuals and simplify controls
- *(dashboard)* centralize focus scoring in backend and address PR feedback
- *(dashboard)* promote narrative above fold, fail visibly when missing
- *(dashboard)* server-computed focus set replaces client-side intention path highlighting
- *(dashboard)* visual refinements — epics, hulls, intent highlighting
- *(dashboard)* fix tree/circle views, add intention path + focused arc

### Fixed

- graph accuracy, blocked propagation, ID resolution, completion evidence ([#146](https://github.com/nicsuzor/mem/pull/146))
- *(dashboard)* improve webcola layout spread and add force config sliders
- *(dashboard)* simplify to 3 forces, fix edge visibility and clumping
- *(dashboard)* narrow intention path to P0/P1 seeds, exclude remaining siblings from highlight
- *(dashboard)* move intention highlighting into drawStaticForce
- *(dashboard)* break parent cycles before stratify in tree/circle views
- *(dashboard)* add cycle detection to intention path parent walk

### Other

- *(dashboard)* webcola integration with epic-based grouping
- *(dashboard)* consolidate force simulation config into constants.ts
- add CLAUDE.md and GEMINI.md pointing to .agent/CORE.md
- update CORE.md — 36 tools, new source files

## [0.3.5](https://github.com/nicsuzor/mem/compare/v0.3.4...v0.3.5) - 2026-03-25

### Fixed

- *(treemap)* resolve parent IDs and prevent layout shuffle on click
- *(dash)* resolve treemap hierarchy bugs and remove virtual containers

### Other

- update PR reviewer agent prompt to latest
- Remove layout generation from Rust PKB tool; dashboard uses single graph.json
- Dim completed task outlines so priority borders only pop on active tasks
- radius-aware bounds for better circle pack viewport fit
- Circle pack: expand graph to full width when no task selected
- Circle pack: shrink completed tasks so active work dominates layout
- Circle pack: consistent status-based color encoding matching legend
- Circle pack: dim completed task text + larger parent labels
- Circle pack: depth-tiered parent containers with prominent labels
- Circle pack: zoom-responsive text labels hide when too small to read
- Mute active task colors so only attention states pop
- Make project boundaries visually explicit
- Increase task card spacing for calmer visual texture
- Add 3-tier visual hierarchy: projects → epics → tasks
- project-grouped hierarchy with priority-driven area
- Improve treemap readability: spacing, opacity, contrast, no grid
- Switch treemap to status-based fill colors with priority borders
- Fix treemap parent headers to grow dynamically with wrapped text
- Fix graph layout key mapping to match actual filenames

## [0.3.4](https://github.com/nicsuzor/mem/compare/v0.3.3...v0.3.4) - 2026-03-22

### Fixed

- remove extra closing brace in Status command that broke compilation

### Other

- Update .github/agents/pr-reviewer.agent.md
- Update .github/agents/pr-reviewer.agent.md
- add PR reviewer bot (axiom-driven review agent)

## [0.3.3](https://github.com/nicsuzor/mem/compare/v0.3.2...v0.3.3) - 2026-03-22

### Added

- add depth-dependent treemap padding and collapse single-child parents
- add treemap weight mode toggle (sqrt/priority/dw-bucket/equal)
- address overwhelm dashboard QA findings and improve graph performance
- *(pkb)* add task_summary MCP tool + fix ready filter to claimable types only
- *(pkb)* enforce parent/project on task creation, add merge-node command
- *(lint)* prohibit body as frontmatter key, auto-migrate to markdown body
- *(dashboard)* populate dashboard sections with live graph data instead of stale synthesis logs
- *(dashboard)* show completed tasks by default in circle pack
- *(dashboard)* remove FA2 graph layout, keep SFDP only
- enforce PKB node quality — require id, parent, status_group
- implement cross-process file locking for vector store index
- *(dashboard)* wire Complete/Ready buttons to persist via aops CLI
- add bench-reindex command and tune embedding parallelism defaults
- expand linter schema and add autofixes for type, status, and ID format
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- fix lint bug
- flatten treemap weight curve with sqrt to distribute space more evenly
- reduce leaf node font size (5-13px → 4-11px) to fit text on smaller nodes
- reduce treemap parent font size (10-14px → 8-11px) to prevent node cutoff
- add border clearance to treemap top padding (34→38px)
- align treemap padding top with header height so children sit below parent title
- improve treemap parent node text sizing, wrapping, and node spacing
- harden dashboard task editor — remove delete, guard project completion, warn on active children
- repair MCP connection and populate path reconstruction from session summaries
- add missing subtasks field to test helper GraphNode initializer
- remove duplicate line causing build failure in cli.rs
- dashboard reads synthesis.json from wrong path
- *(dashboard)* fix task completion by resolving aops binary path and adding missing toggle function
- orphan detection now finds nodes with no valid parent
- address PR review — single lock for stale check, GPU-aware batch size display
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.3.1
- release v0.3.1
- Update src/lint.rs
- spec
- Merge pull request #113 from nicsuzor/feat/dashboard-density-legend
- address PR #113 review comments — extract magic numbers and helpers
- release v0.2.37
- replace project field
- remove project from header
- synth
- change install script
- Merge pull request #111 from nicsuzor/crew/crew_37
- Update src/lint.rs
- *(dashboard)* use css hidden instead of svelte unmounting to prevent expensive graph re-renders on tab switch
- Merge pull request #84 from nicsuzor/release-plz-2026-03-11T08-24-29Z
- update treemap
- release v0.2.31
- release v0.2.30
- Merge pull request #80 from nicsuzor/crew/alexis_21
- Merge pull request #76 from nicsuzor/feat/search-detail-level
- release v0.2.29 ([#75](https://github.com/nicsuzor/mem/pull/75))
- address review feedback on batch graph operations
- batch graph operations for task graph restructuring
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require ACA_DATA
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.3.2](https://github.com/nicsuzor/mem/compare/v0.3.1...v0.3.2) - 2026-03-22

### Added

- add depth-dependent treemap padding and collapse single-child parents
- add treemap weight mode toggle (sqrt/priority/dw-bucket/equal)
- address overwhelm dashboard QA findings and improve graph performance
- *(pkb)* add task_summary MCP tool + fix ready filter to claimable types only
- *(pkb)* enforce parent/project on task creation, add merge-node command
- *(lint)* prohibit body as frontmatter key, auto-migrate to markdown body
- *(dashboard)* populate dashboard sections with live graph data instead of stale synthesis logs
- *(dashboard)* show completed tasks by default in circle pack
- *(dashboard)* remove FA2 graph layout, keep SFDP only
- enforce PKB node quality — require id, parent, status_group
- implement cross-process file locking for vector store index
- *(dashboard)* wire Complete/Ready buttons to persist via aops CLI
- add bench-reindex command and tune embedding parallelism defaults
- expand linter schema and add autofixes for type, status, and ID format
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- fix lint bug
- flatten treemap weight curve with sqrt to distribute space more evenly
- reduce leaf node font size (5-13px → 4-11px) to fit text on smaller nodes
- reduce treemap parent font size (10-14px → 8-11px) to prevent node cutoff
- add border clearance to treemap top padding (34→38px)
- align treemap padding top with header height so children sit below parent title
- improve treemap parent node text sizing, wrapping, and node spacing
- harden dashboard task editor — remove delete, guard project completion, warn on active children
- repair MCP connection and populate path reconstruction from session summaries
- add missing subtasks field to test helper GraphNode initializer
- remove duplicate line causing build failure in cli.rs
- dashboard reads synthesis.json from wrong path
- *(dashboard)* fix task completion by resolving aops binary path and adding missing toggle function
- orphan detection now finds nodes with no valid parent
- address PR review — single lock for stale check, GPU-aware batch size display
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.3.1
- Update src/lint.rs
- spec
- Merge pull request #113 from nicsuzor/feat/dashboard-density-legend
- address PR #113 review comments — extract magic numbers and helpers
- release v0.2.37
- replace project field
- remove project from header
- synth
- change install script
- Merge pull request #111 from nicsuzor/crew/crew_37
- Update src/lint.rs
- *(dashboard)* use css hidden instead of svelte unmounting to prevent expensive graph re-renders on tab switch
- Merge pull request #84 from nicsuzor/release-plz-2026-03-11T08-24-29Z
- update treemap
- release v0.2.31
- release v0.2.30
- Merge pull request #80 from nicsuzor/crew/alexis_21
- Merge pull request #76 from nicsuzor/feat/search-detail-level
- release v0.2.29 ([#75](https://github.com/nicsuzor/mem/pull/75))
- address review feedback on batch graph operations
- batch graph operations for task graph restructuring
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require ACA_DATA
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.3.1](https://github.com/nicsuzor/mem/compare/v0.3.0...v0.3.1) - 2026-03-22

### Added

- add depth-dependent treemap padding and collapse single-child parents
- add treemap weight mode toggle (sqrt/priority/dw-bucket/equal)
- address overwhelm dashboard QA findings and improve graph performance
- *(pkb)* add task_summary MCP tool + fix ready filter to claimable types only

### Fixed

- fix lint bug
- flatten treemap weight curve with sqrt to distribute space more evenly
- reduce leaf node font size (5-13px → 4-11px) to fit text on smaller nodes
- reduce treemap parent font size (10-14px → 8-11px) to prevent node cutoff
- add border clearance to treemap top padding (34→38px)
- align treemap padding top with header height so children sit below parent title
- improve treemap parent node text sizing, wrapping, and node spacing
- harden dashboard task editor — remove delete, guard project completion, warn on active children
- repair MCP connection and populate path reconstruction from session summaries

### Other

- Update src/lint.rs
- spec
- Merge pull request #113 from nicsuzor/feat/dashboard-density-legend
- address PR #113 review comments — extract magic numbers and helpers

## [0.3.0](https://github.com/nicsuzor/mem/compare/v0.2.37...v0.3.0) - 2026-03-21

### Added

- [**breaking**] merge aops and pkb into single `pkb` binary
- add -s/-t/-b parallelism options to `aops reindex`

### Fixed

- CLI reindex exits immediately if index is locked
- incremental graph rebuild + non-blocking vector store locks

## [0.2.37](https://github.com/nicsuzor/mem/compare/v0.2.36...v0.2.37) - 2026-03-20

### Added

- *(pkb)* enforce parent/project on task creation, add merge-node command
- *(lint)* prohibit body as frontmatter key, auto-migrate to markdown body

### Fixed

- add missing subtasks field to test helper GraphNode initializer
- remove duplicate line causing build failure in cli.rs
- dashboard reads synthesis.json from wrong path

### Other

- replace project field
- remove project from header
- synth
- change install script
- Merge pull request #111 from nicsuzor/crew/crew_37
- Update src/lint.rs

## [0.2.36](https://github.com/nicsuzor/mem/compare/v0.2.35...v0.2.36) - 2026-03-19

### Added

- exclude subtasks from list_tasks and task_search by default
- add sub-task format with dot-notation IDs

### Other

- Merge pull request #108 from nicsuzor/crew/crew_36
- Update src/cli.rs
- Merge branch 'main' into crew/crew_34
- update ACA_DATA

## [0.2.35](https://github.com/nicsuzor/mem/compare/v0.2.34...v0.2.35) - 2026-03-19

### Added

- *(dashboard)* MCP client integration for overwhelm dashboard ([#104](https://github.com/nicsuzor/mem/pull/104))

## [0.2.34](https://github.com/nicsuzor/mem/compare/v0.2.33...v0.2.34) - 2026-03-19

### Other

- Merge pull request #101 from nicsuzor/feat/manual-release

## [0.2.33](https://github.com/nicsuzor/mem/compare/v0.2.32...v0.2.33) - 2026-03-12

### Other

- update treemap

## [0.2.32](https://github.com/nicsuzor/mem/compare/v0.2.31...v0.2.32) - 2026-03-11

### Added

- add bench-reindex command and tune embedding parallelism defaults
- expand linter schema and add autofixes for type, status, and ID format
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- orphan detection now finds nodes with no valid parent
- address PR review — single lock for stale check, GPU-aware batch size display
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.30
- Merge pull request #80 from nicsuzor/crew/alexis_21
- Merge pull request #76 from nicsuzor/feat/search-detail-level
- release v0.2.29 ([#75](https://github.com/nicsuzor/mem/pull/75))
- address review feedback on batch graph operations
- batch graph operations for task graph restructuring
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.31](https://github.com/nicsuzor/mem/compare/v0.2.30...v0.2.31) - 2026-03-10

### Added

- add bench-reindex command and tune embedding parallelism defaults
- expand linter schema and add autofixes for type, status, and ID format
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- address PR review — single lock for stale check, GPU-aware batch size display
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.30
- Merge pull request #80 from nicsuzor/crew/alexis_21
- Merge pull request #76 from nicsuzor/feat/search-detail-level
- release v0.2.29 ([#75](https://github.com/nicsuzor/mem/pull/75))
- address review feedback on batch graph operations
- batch graph operations for task graph restructuring
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.30](https://github.com/nicsuzor/mem/compare/v0.2.29...v0.2.30) - 2026-03-10

### Added

- add bench-reindex command and tune embedding parallelism defaults

### Fixed

- address PR review — single lock for stale check, GPU-aware batch size display

### Other

- Merge pull request #80 from nicsuzor/crew/alexis_21
- Merge pull request #76 from nicsuzor/feat/search-detail-level

## [0.2.29](https://github.com/nicsuzor/mem/compare/v0.2.28...v0.2.29) - 2026-03-09

### Added

- expand linter schema and add autofixes for type, status, and ID format

### Other

- address review feedback on batch graph operations
- batch graph operations for task graph restructuring

## [0.2.28](https://github.com/nicsuzor/mem/compare/v0.2.27...v0.2.28) - 2026-03-08

### Added

- allow pkb server to start with stale index
- add search evaluation harness with golden queries
- semantic chunking, incremental saves, and granular progress
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- pin ort to =2.0.0-rc.11 to avoid VitisAI EP build failure
- cap ONNX session pool at 6 to prevent OOM during reindex
- remove incorrect BGE v1/v1.5 query prefix from BGE-M3 search
- harden eval module with div-by-zero guard, idiomatic Rust, and unit tests
- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.25
- release v0.2.25
- add concurrency group to release-plz workflow
- release v0.2.23
- Merge branch 'main' into crew/sylvia_59
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.27](https://github.com/nicsuzor/mem/compare/v0.2.26...v0.2.27) - 2026-03-07

### Fixed

- pin ort to =2.0.0-rc.11 to avoid VitisAI EP build failure

## [0.2.26](https://github.com/nicsuzor/mem/compare/v0.2.25...v0.2.26) - 2026-03-07

### Added

- add search evaluation harness with golden queries
- semantic chunking, incremental saves, and granular progress
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- cap ONNX session pool at 6 to prevent OOM during reindex
- remove incorrect BGE v1/v1.5 query prefix from BGE-M3 search
- harden eval module with div-by-zero guard, idiomatic Rust, and unit tests
- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.25
- add concurrency group to release-plz workflow
- release v0.2.23
- Merge branch 'main' into crew/sylvia_59
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.25](https://github.com/nicsuzor/mem/compare/v0.2.24...v0.2.25) - 2026-03-07

### Fixed

- cap ONNX session pool at 6 to prevent OOM during reindex

### Other

- add concurrency group to release-plz workflow

## [0.2.24](https://github.com/nicsuzor/mem/compare/v0.2.23...v0.2.24) - 2026-03-07

### Added

- add search evaluation harness with golden queries
- semantic chunking, incremental saves, and granular progress
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- remove incorrect BGE v1/v1.5 query prefix from BGE-M3 search
- harden eval module with div-by-zero guard, idiomatic Rust, and unit tests
- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.23
- Merge branch 'main' into crew/sylvia_59
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.23](https://github.com/nicsuzor/mem/compare/v0.2.22...v0.2.23) - 2026-03-07

### Added

- add search evaluation harness with golden queries

### Fixed

- remove incorrect BGE v1/v1.5 query prefix from BGE-M3 search
- harden eval module with div-by-zero guard, idiomatic Rust, and unit tests

## [0.2.22](https://github.com/nicsuzor/mem/compare/v0.2.21...v0.2.22) - 2026-03-07

### Added

- semantic chunking, incremental saves, and granular progress
- drop filtered-out nodes from per-layout graph output
- per-layout graph file output with version bump to 0.2.17
- per-layout graph file output
- add forceatlas2_focus layout and DOT export with positions
- FA2 clustering on reachable-only subgraph with warm-start init
- add reachable field to graph nodes
- improve treemap, circle_pack, arc layouts + interactive preview
- add treemap, circle packing, arc diagram layouts + rectangle-aware FA2
- include task classifications in graph JSON output
- load graph layout parameters from runtime layout.toml
- *(graph)* add precomputed ForceAtlas2 layout coordinates
- *(pkb)* add hierarchy validation warnings to create_task and create_document
- *(tui)* add focus reasoning, cross-project synergy detection (Phase 5)
- *(tui)* add quick capture modal and assumption engine in lib (Phase 4)
- *(tui)* add assumption engine with parsing, display, and health (Phase 3)
- *(tui)* add PKB integration with search and backlinks (Phase 2)
- *(tui)* add graph-native views with context rendering (Phase 1)
- *(tui)* add Planning Web TUI skeleton (Phase 0)
- publish releases to public academicOps repo
- add install script and cargo-binstall support
- add progress indicator for batch embedding during reindex
- complete mem CLI + MCP task command gaps (14 tasks)
- *(cli)* task tree display overhaul — focus view, dashboard, visual hierarchy
- *(cli)* UX improvements — show IDs, body, hints; rebuild graph after create
- *(mcp)* consolidate 22 tools down to 18
- *(cli)* tree view for `tasks`, remove `list` command
- add create_document, append_to_document, improve update_task
- *(pkb)* add get_task(id) tool to MCP server
- add --version flag and make install target
- *(pkb)* add pagination, project filter, and fix trace duplicates
- *(pkb)* add type filter to pkb_orphans tool
- use relative paths in vector and graph stores for portability
- add graph-aware tools (pkb_context, pkb_search, pkb_trace, pkb_orphans)
- *(cli)* add `mem done` and `mem update` commands
- add Makefile for Apple Silicon cross-build, fix macOS ONNX download
- extract graph modules, add task CLI/MCP tools, rename to aops

### Fixed

- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat
- filter nodes without positions from per-layout DOT exports
- address PR #35 review comments
- remove stale graph cache that silently served incomplete JSON output
- read node body from file in TUI detail view
- use atomic writes for model downloads to prevent corrupt cache
- use explicit task ID directly in filename without random suffix
- *(release)* use portable sed for macOS compatibility
- *(release)* use AOPS_RELEASE_TOKEN secret name
- *(release)* regenerate lockfile during version bump
- use model's sentence_embedding output instead of manual mean pooling
- use LazyLock for static regex, hold write lock across remove+save
- address 10 PR review comments

### Other

- release v0.2.21
- Merge branch 'main' into crew/sylvia_59
- automate releases with release-plz
- v0.2.19
- Update src/graph_store.rs
- v0.2.16
- defer layout computation to on-demand only
- v0.2.15
- split release into two-step bump/tag workflow
- v0.2.14
- Merge pull request #42 from nicsuzor/layouts
- Update src/graph_store.rs
- Update src/layout.rs
- add cargo install instructions to CORE.md
- document canonical status values and node types in README
- canonical status constants and archived alias
- v0.2.13
- v0.2.12
- ignore outputs
- v0.2.11 ([#39](https://github.com/nicsuzor/mem/pull/39))
- Merge branch 'main' into feat/layout-improvements-37
- v0.2.9
- Merge pull request #35 from nicsuzor/crew/audre_78
- v0.2.7
- v0.2.6
- add graph layout configuration section to README
- v0.2.5
- Merge PR #24: Add confidence, source, supersedes fields to memories
- v0.2.4
- v0.2.3
- v0.2.2
- v0.2.1
- Merge branch 'main' into polecat/mem-6d45faf2
- Merge branch 'main' into aops-tui-ascii-context-3751429817177404899
- Merge pull request #21 from nicsuzor/polecat/mem-a29aae98
- Add background worker dispatch method for TUI tasks
- rewrite README for public release
- v0.2.0
- clean up for public release
- v0.1.18
- combine
- combine
- v0.1.17
- v0.1.16
- cuda version
- v0.1.15
- v0.1.14
- v0.1.13
- extract shared lib.rs to eliminate per-binary dead code warnings
- Merge branch 'crew/barbara': switch to BGE-M3 embedding model
- v0.1.12
- warnings
- Merge pull request #5 from nicsuzor/crew/crew_12
- add 38 unit tests and update documentation for new tools
- add intentionally empty file
- *(cli)* rename _v2 display functions to drop suffix
- v0.1.10
- v0.1.9
- ready to release
- release scripts
- v0.1.6
- license GPLv3, rename binary pkb-search -> pkb, acknowledge shodh-memory
- task tree improvements
- v0.1.4
- v0.1.3
- v0.1.2
- add release workflow with version bumping and multi-arch builds
- document all 15 CLI commands and 15 MCP tools in README
- fold fast-indexer into aops CLI, delete separate binary
- generalize task_crud.rs to document_crud.rs for memory support
- lazy ONNX session pool — 3x faster search startup
- simplify default paths, require AOPS_SESSIONS
- increase chunk size
- 3x faster reindex with dynamic padding, rayon parallelism, progressive saves
- parallel
- add cli
- first version
- PKB Semantic Search MCP Server

## [0.2.21](https://github.com/nicsuzor/mem/compare/v0.2.20...v0.2.21) - 2026-03-07

### Added

- semantic chunking, incremental saves, and granular progress

### Fixed

- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat

### Other

- Merge branch 'main' into crew/sylvia_59

## [0.2.20](https://github.com/nicsuzor/mem/compare/v0.2.19...v0.2.20) - 2026-03-07

### Added

- add PKB linter with auto-fix capability

### Fixed

- lint --fix now actually fixes all fixable issues
- address PR review — security, correctness, and code quality
