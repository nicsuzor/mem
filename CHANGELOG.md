# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- simplify default paths, require ACA_DATA
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
