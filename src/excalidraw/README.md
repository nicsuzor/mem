# Excalidraw Component

The Excalidraw integration in `mem` bridges the PKB knowledge graph with Excalidraw visual whiteboards (`.excalidraw` and `.excalidrawlib`). The architecture operates across two levels:

1. **In-Core PKB Engine Integration (`src/excalidraw/`)**: Bidirectional visual sync for knowledge graphs directly inside `pkb`.
2. **Companion Agent Tooling (`src/bin/pkb_excalidraw.rs`)**: Standalone binary (`pkb-excalidraw`) providing token-efficient CLI projections and invariant-enforcing mutations for LLM agents.

---

## 1. In-Core PKB Engine (`src/excalidraw/`)

Exposed via `pkb excalidraw <command>` and `pkb export-graph --format excalidraw`.

### Module Overview

| Module | Source File | Responsibilities |
| :--- | :--- | :--- |
| **Schema** | [`schema.rs`](file:///home/nic/src/mem/src/excalidraw/schema.rs) | Typed AST for Excalidraw V2 JSON, unified status/type color mapping, port coordinates, and custom metadata binding (`PkbCustomData`). |
| **Layout** | [`layout.rs`](file:///home/nic/src/mem/src/excalidraw/layout.rs) | Ego-network extraction, Sugiyama layered DAG layout computation, card dimension calculation, frame containers, and boundary port bindings. |
| **Reader** | [`reader.rs`](file:///home/nic/src/mem/src/excalidraw/reader.rs) | 5-pass deserializer resolving container-bound text, edge sources/targets, frame memberships, and duplicate element IDs. |
| **Diff** | [`diff.rs`](file:///home/nic/src/mem/src/excalidraw/diff.rs) | 3-way reconciliation engine comparing base snapshot, live graph, and modified canvas to classify node additions, updates, retargeted edges, and deletions. |
| **Merge** | [`merge.rs`](file:///home/nic/src/mem/src/excalidraw/merge.rs) | Disk writer applying canvas mutations back to markdown frontmatter (`depends_on`, status, labels), spiral placement for new nodes, and cycle detection. |

### CLI Commands (`pkb excalidraw`)

```bash
# Export full knowledge graph or ego-network to canvas
pkb excalidraw export <output.excalidraw> [--focus <node-id>] [--hops <n>]

# Compute 3-way diff between modified canvas, base snapshot, and live graph
pkb excalidraw diff <canvas.excalidraw> [--base <base.json>] [--json]

# Sync visual changes back to markdown files on disk
pkb excalidraw sync <canvas.excalidraw> [--base <base.json>] [--dry-run] [--sync-edge-removals]
```

---

## 2. Companion Agent Binary (`pkb-excalidraw`)

Entry point: [`src/bin/pkb_excalidraw.rs`](file:///home/nic/src/mem/src/bin/pkb_excalidraw.rs).

`pkb-excalidraw` is designed for LLM coding agents, reducing multi-thousand token Excalidraw JSON down to 50–200 token tabular summaries and executing transactional, invariant-safe mutations.

### CLI Summary

| Category | Command | Description |
| :--- | :--- | :--- |
| **Projections** | `summary`, `map`, `nodes`, `edges`, `arrows` | Dense, token-efficient tabular representations of whiteboard elements. |
| **Inspection** | `inspect <id>`, `get <id>` | Retrieve full properties or formatted details for a specific element. |
| **Structural Diff** | `struct-diff <file1> <file2>` | Semantic diff of nodes, labels, and edges ignoring coordinate float jitter. |
| **Node CRUD** | `add-node`, `add-text`, `set-text`, `fit` | Add container shapes with centered text, update labels, or resize nodes symmetrically. |
| **Connections** | `connect --from <id1> --to <id2>` | Create 2-bound directed arrow with optional label. |
| **Transformations** | `move-elem <id> [--to X,Y \| --by DX,DY]` | Translate element, maintaining bound text and attached arrow endpoints. |
| **Deletions** | `delete-elem <id> [--cascade-arrows]` | Delete node and clean up dangling bindings and cascaded edges. |
| **Batching** | `batch <changes.json \| ->` | Execute atomic, multi-step mutations with dry-run integrity validation. |
| **Auditing** | `check`, `overlap`, `arrows-check` | Validate index ordering, detect AABB box collisions, and audit arrow-node intersections. |
| **Theming** | `theme apply <theme>`, `theme export` | Apply standardized palettes (`default`, `retro-terminal`, `aops-default`). |
| **Libraries** | `lib`, `item <selector>` | Inspect and extract elements from `.excalidrawlib` v1/v2 library files. |

---

## 3. Key Technical Invariants

- **Fractional Index Ordering**: Element order in the `elements` array matches fractional ASCII keys (`index` property) to prevent z-index tearing.
- **Bidirectional Bindings**: Container nodes reference text IDs via `boundElements: [{ "id": "...", "type": "text" }]`, and text nodes reference containers via `containerId`.
- **Text Wrapping Duplication**: Multi-line wrapped text stores both display text (`text`) and unwrapped text (`originalText`).
- **Atomic Disk Writes**: File updates are written to `.tmp.<pid>.<rand>` before atomic rename to prevent scene corruption.

---

## 4. Reference Documentation & Tests

- **Architecture Specification**: [`specs/excalidraw-tooling.md`](file:///home/nic/src/mem/specs/excalidraw-tooling.md)
- **Agent Manipulation Guide**: [`references/EXCALIDRAW_AGENT_GUIDE.md`](file:///home/nic/src/mem/references/EXCALIDRAW_AGENT_GUIDE.md)
- **Engine Tests**: [`tests/excalidraw_e2e_test.rs`](file:///home/nic/src/mem/tests/excalidraw_e2e_test.rs)
- **Tooling Tests**: [`tests/pkb_excalidraw_test.rs`](file:///home/nic/src/mem/tests/pkb_excalidraw_test.rs)
