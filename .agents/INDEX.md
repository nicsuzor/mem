# Available Documentation

- **`README.md`**: Project overview — local semantic search + knowledge graph MCP server for a markdown PKB; features, install, usage
- **`CLAUDE.md`**: Repo entry point — imports `.agent/CORE.md`
- **`.agent/CORE.md`**: Developer & agent reference — specs-first rule, path discovery, fail-fast/HALT rules, architecture, MCP tool catalogue, key patterns, build & install
- **`CHANGELOG.md`**: Release history (release-please managed)
- **`INVENTORY.md`**: Inventory of repo components
- **`references/TAXONOMY.md`**: Canonical definitions for PKB/work-management concepts
- **`references/EXCALIDRAW_AGENT_GUIDE.md`**: Token-efficient Excalidraw manipulation guide and recipes for LLM agents
- **`specs/`**: Specifications (SSoT) — approved current-state design intent
- **`specs/excalidraw-tooling.md`**: Excalidraw tooling specification (`src/bin/pkb_excalidraw.rs`)
- **`specs/pkb-server-spec.md`**: PKB MCP server specification
- **`specs/pkb-type-taxonomy.md`**: Document/node type taxonomy
- **`specs/pkb-rules.md`**: PKB rules (SSoT) — what the PKB holds, writing/linking nodes, graph hygiene, write/read discipline, task lifecycle, prioritisation doctrine, rule placement, agent conduct around the PKB
- **`specs/ranking.md`**: Ranking engine — `focus_tuple`, `cost_of_delay`, urgency, severity ladder, verbal weight scale
- **`specs/work-management.md`**: Task and work-management model
- **`specs/batch-graph-operations.md`**, **`specs/multi-parent.md`**, **`specs/mutation-neighborhood.md`**, **`specs/typed-facts.md`**, **`specs/crud-redesign.md`**, **`specs/crud-audit.md`**, **`specs/areas-not-projects.md`**: Additional design specs
- **`src/`**: Rust source — CLI, MCP server, graph store, vector store, embeddings, document CRUD, metrics
- **`tests/`**: Test suite
- **`examples/`**: Usage examples and templates
