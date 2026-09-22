# Excalidraw in mem

`mem` talks to [Excalidraw](https://excalidraw.com) at two levels, and they are deliberately separate:

| Level | Binary / surface | What it touches | Knows about the PKB? |
|-------|------------------|-----------------|----------------------|
| **Graph canvas** (server side) | `pkb excalidraw export\|diff\|sync`, `pkb graph --format excalidraw`, MCP tools `graph_excalidraw` / `diff_excalidraw` / `sync_excalidraw` | The knowledge graph and the markdown files behind it | Yes — the whole point |
| **File tooling** (companion binary) | `pkb-excalidraw FILE <command>` | One `.excalidraw` or `.excalidrawlib` file on disk | No — never reads or writes the PKB |

The graph canvas turns a neighbourhood of PKB nodes into a scene of cards and arrows, lets a human (or agent) edit it in any Excalidraw editor, and writes the structural edits back into frontmatter. The companion binary is a token-cheap inspector and invariant-preserving mutator for any Excalidraw file, whether or not it came from the PKB.

Two things `mem` does **not** do, so you don't go looking for them:

- **It does not store canvases.** The MCP tools are stateless: `graph_excalidraw` returns JSON and keeps nothing ([`handlers_batch.rs:397-413`](../mcp_server/handlers_batch.rs#L397-L413)); `diff` and `sync` take the canvas (and optional base snapshot) as strings from the caller. The caller owns the file.
- **It does not index `.excalidraw` files as nodes.** The PKB walker admits only `*.md` ([`pkb.rs:242-245`](../pkb.rs#L242-L245)). A canvas sitting in the PKB directory is invisible to search and to the graph.

## Where things live

| Module | File | Responsibility |
|--------|------|----------------|
| Schema | [`schema.rs`](schema.rs) | Typed Excalidraw V2 AST, `PkbCustomData` ([L630-L688](schema.rs#L630-L688)), card sizing ([L83-L128](schema.rs#L83-L128)), node/edge colour tables ([L196-L249](schema.rs#L196-L249)), presets and the optional theme file ([L153-L192](schema.rs#L153-L192)) |
| Layout | [`layout.rs`](layout.rs) | Ego-network extraction ([L47-L121](layout.rs#L47-L121)), layered DAG layout, card and bound-text generation ([L623-L761](layout.rs#L623-L761)), epic frames ([L556-L620](layout.rs#L556-L620)), two-bound arrows ([L835-L874](layout.rs#L835-L874)) |
| Reader | [`reader.rs`](reader.rs) | Multi-pass canvas deserialiser: resolves bound text, arrow endpoints, frame membership, duplicate ids; recovers node ids and edge types from `customData` or text |
| Validate | [`validate.rs`](validate.rs) | Shape gate ([L49-L84](validate.rs#L49-L84)) and structural gate ([L102-L263](validate.rs#L102-L263)) that every canvas must pass before `diff`/`sync` |
| Diff | [`diff.rs`](diff.rs) | 3-way reconciliation of base snapshot, live graph and edited canvas → `GraphDiff` ([L84-L93](diff.rs#L84-L93)) |
| Merge | [`merge.rs`](merge.rs) | Writes a `GraphDiff` to markdown ([L515-L750](merge.rs#L515-L750)); merges live graph into an existing canvas ([L164](merge.rs#L164)); cycle detection ([L41-L107](merge.rs#L41-L107)) |
| Entry points | [`mod.rs`](mod.rs) | `parse_canvas` ([L94-L118](mod.rs#L94-L118)), `parse_base_snapshot`, `diff_canvas`, `sync_canvas`, `merge_canvas_with_live` |
| MCP handlers | [`../mcp_server/handlers_batch.rs`](../mcp_server/handlers_batch.rs) | `handle_graph_excalidraw` (L397), `handle_diff_excalidraw` (L455), `handle_sync_excalidraw` (L503); schemas in [`schemas.rs:587-653`](../mcp_server/schemas.rs#L587-L653) |
| CLI | [`../cli.rs`](../cli.rs) | `ExcalidrawCommands` ([L653-L697](../cli.rs#L653-L697)), `handle_excalidraw_command` ([L3837-L3977](../cli.rs#L3837-L3977)), `pkb graph --format excalidraw` ([L2549-L2560](../cli.rs#L2549-L2560)) |
| Companion binary | [`../bin/pkb_excalidraw.rs`](../bin/pkb_excalidraw.rs) | The `pkb-excalidraw` CLI (declared in [`Cargo.toml`](../../Cargo.toml) as a second `[[bin]]`) |

Tests: [`tests/excalidraw_e2e_test.rs`](../../tests/excalidraw_e2e_test.rs) (engine), [`tests/pkb_excalidraw_test.rs`](../../tests/pkb_excalidraw_test.rs) (binary), plus MCP-level gates in [`src/mcp_server/claim_task_tests.rs`](../mcp_server/claim_task_tests.rs#L950-L1180).

Longer reading: [`specs/excalidraw-tooling.md`](../../specs/excalidraw-tooling.md) (invariants for the companion binary) and [`references/EXCALIDRAW_AGENT_GUIDE.md`](../../references/EXCALIDRAW_AGENT_GUIDE.md) (copy-paste patterns for agents).

## Install

Both binaries ship together. The release tarball and `install.sh` install `pkb` and `pkb-excalidraw` side by side ([`install.sh:74-90`](../../install.sh#L74-L90)); the container image does the same ([`Dockerfile:38-39`](../../Dockerfile#L38-L39)).

```bash
curl -fsSL https://raw.githubusercontent.com/nicsuzor/mem/main/install.sh | sh
# or
cargo install --git https://github.com/nicsuzor/mem.git   # builds both [[bin]] targets
```

**`ACA_DATA` is mandatory for every `pkb` invocation, including `pkb excalidraw --help`**: the global `--pkb-root` / `--db-path` flags have eager defaults computed from `ACA_DATA` ([`cli.rs:25-30`](../cli.rs#L25-L30), [L905-L920](../cli.rs#L905-L920)), so with the variable unset the binary exits with `error: ACA_DATA environment variable is not set` before parsing flags. The flags only override the defaults. `pkb-excalidraw` needs neither the variable nor a PKB; `pkb-excalidraw --help` prints its usage ([`pkb_excalidraw.rs:2777-2780`](../bin/pkb_excalidraw.rs#L2777-L2780)).

Older installs may have `pkb` without `pkb-excalidraw`. Re-run `install.sh` and confirm with `command -v pkb-excalidraw`.

**Where to keep scenes.** The PKB walker only reads `*.md`, so a `.excalidraw` file inside the PKB tree is harmless but invisible; keep canvases wherever you like (a `canvases/` directory outside the PKB, or beside the notes they describe) and keep the `base.json` snapshot next to its canvas so `diff`/`sync` always get the matching base.

## The PKB node model on canvas

Every exported scene is a plain Excalidraw V2 file (`type: "excalidraw"`, `version: 2`, `source: "https://excalidraw.com"`) that opens in any editor. PKB identity rides in `customData.pkb`, so moving, restyling or re-laying-out elements never breaks the link back to the graph.

### Cards

One shape plus one container-bound text element per node ([`layout.rs:623-761`](layout.rs#L623-L761)):

- **Shape** — `diamond` for `target`/`goal`, `ellipse` for `area`, otherwise `rectangle` ([L640-L644](layout.rs#L640-L644)). Element id = node id; text id = `text-{node_id}`.
- **`customData.pkb`** on the shape ([L665-L676](layout.rs#L665-L676)):
  ```json
  { "nodeId": "aops_1234abcd", "nodeType": "task", "status": "ready",
    "intent": 2, "parent": "epic_id", "tags": ["a", "b"], "isPkbManaged": true }
  ```
  The reader accepts the snake_case aliases `node_id`, `priority`, etc. ([`schema.rs:630-688`](schema.rs#L630-L688)).
- **Bound text** — a header line `[STATUS · P{intent} · now]`, an optional markers line (`START` when `focus_score ≥ 1000`, `LSL`, `WORK`, and the node's `effort`; [L697-L720](layout.rs#L697-L720)), the title **truncated to 36 characters**, then tags as `#a #b` **truncated to 32 characters** ([L679-L738](layout.rs#L679-L738)). See *Known limitations* for why the truncation matters.
- **Size** — widths S 200 / M 240 / L 300 / A 380, height 84, chosen by type and intent, bumped one tier when `focus_score ≥ 1000` ([`schema.rs:83-128`](schema.rs#L83-L128)).
- **Colour** — by type first (epic, target/goal, area, learn, memory/note), then by status (ready, queued/active, blocked/waiting, review/testing, done, cancelled, default inbox/draft) ([`schema.rs:196-235`](schema.rs#L196-L235)). A red `#e03131` ring marks stakeholder-flagged or in-review nodes ([`schema.rs:75-80`](schema.rs#L75-L80)).
- **Not drawn** — nodes whose status is `someday`, `cancelled` or `abandoned`, and their edges ([`schema.rs:67-72`](schema.rs#L67-L72), [`layout.rs:519-530`](layout.rs#L519-L530)).

### Frames

Children of the same `parent` are grouped in a `frame` element with id `frame-{parent_id}` and name `Epic: {parent_id}`; children carry `frameId` ([`layout.rs:556-620`](layout.rs#L556-L620)). The frame's `customData.pkb` names the parent.

### Arrows

Each graph edge between two exported nodes becomes a two-bound arrow, id `arrow-{source}-{target}-{edge_type}`, with `startBinding`/`endBinding` on fixed ports and `customData.pkb = { edgeType, sourceId, targetId, isPkbManaged }` ([`layout.rs:835-874`](layout.rs#L835-L874)). No label text is emitted. Stroke encodes the edge type ([`schema.rs:238-249`](schema.rs#L238-L249)):

| Edge | Stroke |
|------|--------|
| `depends_on` | `#e03131` solid 2.0 |
| `soft_depends_on` | `#868e96` dashed 1.5 |
| `parent` | `#4c6ef5` solid 2.5 |
| `contributes_to` | `#1971c2` solid 2.0 |
| `closes` | `#2b8a3e` solid 1.5 |
| `supersedes` | `#f08c00` dashed 1.5 |
| `similar_to` | `#ced4da` dashed 1.0 |
| `link` (body wikilinks) and others | `#adb5bd` solid 1.0 |

### Which edges reach the canvas

Neighbourhood traversal follows structural edges only — `parent`, `depends_on`, `soft_depends_on`, `contributes_to`, `supersedes`, `closes` ([`layout.rs:68-78`](layout.rs#L68-L78)). `link` (wikilinks) and `similar_to` edges are never *followed*, but once the node set is fixed every edge between included nodes is drawn ([L103-L108](layout.rs#L103-L108)), so wikilink arrows appear when both ends were reached structurally. Wikilinks in bodies are not parsed by this module; they arrive as `EdgeType::Link` edges from the graph builder.

### Hand-drawn cards and arrows

The reader also understands cards and arrows that were never exported:

- A shape with no `customData.pkb.nodeId` is a **new node**. If its text contains an id matching `(task|epic|mem|target|goal)-[a-zA-Z0-9]{4,16}` that id is used; otherwise it becomes an `added_node` with `node_type: "task"`, `status: "inbox"` ([`reader.rs:517`](reader.rs#L517), [`diff.rs:206-218`](diff.rs#L206-L218)). That regex requires a **hyphen**, but ids minted by `mem` are `{prefix}_{8 hex}` with an underscore ([`graph.rs:609-615`](../graph.rs#L609-L615)), so typing a real id such as `task_1a2b3c4d` into a hand-drawn card does **not** link it — sync will create a duplicate node. Link hand-drawn cards by setting `customData.pkb.nodeId` instead.
- An arrow's edge type comes from `customData.pkb.edgeType`, else from a label prefix — `dep:`/`depends_on:`, `soft:`, `parent:`/`child:`, `contrib:`/`weight:`, `close:`, `super:`, `sim:` — else `link` ([`reader.rs:472-513`](reader.rs#L472-L513)). Arrows with an unresolved endpoint are treated as annotations, not edges.

## Server side: export → edit → diff → sync

### CLI

Help text from `pkb 0.3.95`, abridged (the real output also lists the `help` subcommand and `-h, --help`, and prints the `--pkb-root`/`--db-path` defaults as the expanded `ACA_DATA` path):

```
$ pkb excalidraw --help
Excalidraw visual canvas operations (export, diff, sync)

Usage: pkb excalidraw [OPTIONS] <COMMAND>

Commands:
  export  Export graph or ego-network to an Excalidraw JSON canvas
  diff    Compute 3-way diff between canvas and live PKB
  sync    Apply visual mutations from Excalidraw canvas to PKB markdown files

Options:
      --pkb-root <PKB_ROOT>  Path to the PKB root directory [default: $ACA_DATA]
      --db-path <DB_PATH>    Path to the persistent vector database file [default: $ACA_DATA/pkb_vectors.bin]
```

```bash
pkb excalidraw export <OUTPUT_PATH> [-f|--focus <node>] [-H|--hops <n>]      # default hops 2
pkb excalidraw diff   <CANVAS_PATH> [-b|--base <snapshot>] [--json]
pkb excalidraw sync   <CANVAS_PATH> [-b|--base <snapshot>] [--dry-run] [--sync-edge-removals]
pkb graph --format excalidraw [-o <path>] [-F|--focus <node>] [-H|--hops <n>] # same exporter, stdout if no -o
```

`export` into a path that already holds a valid canvas does not overwrite it: it merges the live graph into the existing scene, keeping positions and adding new nodes in a spiral around the focus ([`cli.rs:3847-3871`](../cli.rs#L3847-L3871), [`merge.rs:113-160`](merge.rs#L113-L160)).

### MCP tools

Served over stdio or Streamable HTTP at `/mcp` ([`cli.rs:3431`](../cli.rs#L3431)); there is no other HTTP surface. Schemas: [`schemas.rs:587-653`](../mcp_server/schemas.rs#L587-L653).

| Tool | Parameters | Read-only | Returns |
|------|------------|-----------|---------|
| `graph_excalidraw` | `node_id` (id, filename or title; flexible resolution), `hops` (1–5, default 2) | yes | Pretty-printed Excalidraw V2 JSON as text |
| `diff_excalidraw` | `canvas` (required, JSON string), `base` (optional snapshot JSON) | yes | `GraphDiff` JSON |
| `sync_excalidraw` | `canvas` (required), `base`, `dry_run` (default `false`), `sync_edge_removals` (default `false`) | **no** | dry run: `{dry_run, diff, message}`; live: `{success, created_nodes[{id,filename}], updated_nodes, updated_edges, rejected_cycles, warnings}` |

Typical agent loop: `graph_excalidraw` → save the JSON as both `canvas.excalidraw` and `base.json` → human edits `canvas.excalidraw` → `diff_excalidraw(canvas, base)` to review → `sync_excalidraw(canvas, base, dry_run: true)` → `sync_excalidraw(canvas, base)`.

### Export semantics

- `hops` is clamped to 1–5 ([`layout.rs:52`](layout.rs#L52)); the neighbourhood is capped at **100 nodes** ([L57](layout.rs#L57), enforced at [L88](layout.rs#L88)).
- With `node_id` omitted the exporter does **not** emit the entire graph, despite the tool description. It takes the top 10 focus picks and unions their neighbourhoods ([`graph_store.rs:2246-2255`](../graph_store.rs#L2246-L2255)). The 100-node cap is per focus, so the default export can reach ~1000 nodes.
- Focus resolution goes through `GraphStore::resolve` (id, filename stem or title); an unresolvable focus errors with `Focus node '…' not found in graph` ([`mod.rs:53`](mod.rs#L53)).
- Measured on a live PKB (epic `aops-41e428a6`, `hops: 1`): 269 elements, ~300 KB of JSON, 174 ms.

### Validation gate

Every canvas passed to `diff` or `sync` — and every `base` that is not already a serialised snapshot — must pass two gates in `parse_canvas` ([`mod.rs:94-118`](mod.rs#L94-L118)):

1. **Shape** ([`validate.rs:49-84`](validate.rs#L49-L84)): valid JSON object with an `elements` array and `type: "excalidraw"`. `{}` is rejected.
2. **Structure** ([`validate.rs:102-263`](validate.rs#L102-L263)), over non-deleted elements. Blocking: duplicate ids; half-bound arrows (exactly one of `startBinding`/`endBinding`); bindings or `containerId` pointing at missing elements; a container that lacks the `boundElements` back-reference to its text. Warnings only: stale `boundElements` entries; `text`/`originalText` word drift.

Canvases with pre-existing drift that is not structurally broken are accepted ([`claim_task_tests.rs:1115`](../mcp_server/claim_task_tests.rs#L1115)).

### Diff semantics

`GraphDiff` ([`diff.rs:84-93`](diff.rs#L84-L93)) classifies, per [`diff.rs:184-405`](diff.rs#L184-L405):

| Field | Meaning |
|-------|---------|
| `added_nodes` | Cards with no resolvable node id, or an id absent from the live graph |
| `updated_nodes` | Title (when non-empty and ≠ live label / id / `Untitled`), status, intent, parent, tags (set compare; only when the card has tags) |
| `removed_from_canvas` | Node ids in `base` that are no longer on the canvas. Informational — files are never deleted |
| `added_edges` | Canvas arrows (both ends resolved) not present in the live graph |
| `removed_edges` | `base` edges missing from the canvas (both ends still on canvas). Only computed when `base` is given |
| `retargeted_edges` | Declared but never populated |
| `visual_mutations` | Only with `base`: position delta > 0.5 px or stroke/background change |
| `conflicts` | Only with `base`, for title and status: live ≠ base **and** canvas ≠ base **and** live ≠ canvas. The canvas value is still recorded as the update |

### Sync semantics

`sync_diff_to_disk` ([`merge.rs:515-750`](merge.rs#L515-L750)) is additive: it never deletes files and never removes edges unless told to.

- **New cards** become markdown files via `document_crud::create_task` (when `node_type == "task"` and a parent is set) or `create_document`. Written fields: `title`, `status`, `intent`, `parent`, `tags`. Filename `{id}_{slug}.md` with a generated `{prefix}_{8 hex}` id, placed in `tasks/`, `memories/` or `notes/` by type. Canvas coordinates are not persisted.
- **Updated cards** write `title`, `status`, `intent`, `parent`, `tags` to frontmatter ([`merge.rs:582-606`](merge.rs#L582-L606)). Writes go through `document_crud::update_document`, whose policies can refuse them — e.g. `status: blocked` without a `blocker` ([`document_crud.rs:1717`](../document_crud.rs#L1717)) — and a refusal is reported only as a `warnings` entry, not an error ([`merge.rs:610-619`](merge.rs#L610-L619)). **Read `warnings` after every sync.**
- **Added edges** are checked for cycles first (over `depends_on` and `parent` only, [`merge.rs:41-107`](merge.rs#L41-L107)) and reported in `rejected_cycles` if they close one. Then, on the **source** node's file: `depends_on` → appended to `depends_on`; `soft_depends_on` → `soft_depends_on`; `parent` → `parent`. **All other edge types (`contributes_to`, `closes`, `supersedes`, `similar_to`, `link`) are silently not written** ([`merge.rs:646-668`](merge.rs#L646-L668)).
- **`sync_edge_removals: true`** removes `depends_on` entries (and only `depends_on`) whose target arrow was deleted, matching bare ids, `[[id]]` and `…/id.md` forms ([`merge.rs:685-742`](merge.rs#L685-L742)).
- All writes go through `atomic_write_file` (temp file, fsync, rename) in `document_crud.rs`. After a live sync the MCP handler re-embeds the touched documents and schedules a graph rebuild ([`handlers_batch.rs:581-594`](../mcp_server/handlers_batch.rs#L581-L594)).

### Known limitations

- **Round trips are not idempotent.** Titles are truncated to 36 characters and tag lines to 32 on export ([`layout.rs:723-738`](layout.rs#L723-L738)); the reader takes the card text back as the title and re-extracts `#tags` ([`reader.rs:466`](reader.rs#L466), [L449-L452](reader.rs#L449-L452)); the diff sees a mismatch ([`diff.rs:235-254`](diff.rs#L235-L254), [L288-L294](diff.rs#L288-L294)); and `sync` writes it to frontmatter ([`merge.rs:582-606`](merge.rs#L582-L606)). Feeding an unmodified export straight back into `diff_excalidraw` on a real PKB produced 14+ spurious `updated_nodes` with titles like `Restore and make effective the strat...` and tags like `cleanu`. Tags read back from card text are **unioned** with `customData.pkb.tags` ([`reader.rs:255-262`](reader.rs#L255-L262)), so a truncated fragment is added rather than replacing anything: a live (non-dry) sync during review wrote a bogus tag `inf` (from `#information-theory` cut at 32 chars) into a real note. **Always review `updated_nodes` in the diff, and never run a non-dry `sync` on a canvas whose cards carry long titles or tag lines.**
- `sync_excalidraw` does not refresh the graph before diffing (compare [`handlers_batch.rs:456`](../mcp_server/handlers_batch.rs#L456) with [L503](../mcp_server/handlers_batch.rs#L503)); run `diff_excalidraw` first if the PKB may have changed.
- `retargeted_edges` is never populated; a retargeted arrow shows up as one `removed_edge` plus one `added_edge`.
- There are no size or time caps on the canvas string itself; the only limits are the hop clamp and the 100-node neighbourhood cap.

## Companion binary: `pkb-excalidraw`

A single-file Rust CLI ([`src/bin/pkb_excalidraw.rs`](../bin/pkb_excalidraw.rs)) for agents that need to read or edit Excalidraw files without paying for the whole JSON in context. It reads exactly one file, validates it, and writes it back atomically. It never touches the PKB — the only thing it imports from the library is the preset colour table ([L513](../bin/pkb_excalidraw.rs#L513)).

Usage text as printed by the binary ([L22-L47](../bin/pkb_excalidraw.rs#L22-L47)):

```
Usage: pkb-excalidraw FILE [summary|map|style|check|overlap|arrows-check|nodes|edges|arrows]
       pkb-excalidraw FILE inspect <id>
       pkb-excalidraw FILE get <id>
       pkb-excalidraw FILE1 diff FILE2
       pkb-excalidraw FILE1 struct-diff FILE2
       pkb-excalidraw FILE.excalidrawlib lib
       pkb-excalidraw FILE.excalidrawlib item SELECTOR --after INDEX [--at X,Y]

CRUD & Mutation Commands:
       pkb-excalidraw FILE add-node --type <type> --text "<text>" [--at X,Y] [--size W,H] [--role <role>] [--color <hex>] [--id <custom_id>]
       pkb-excalidraw FILE add-text --text "<text>" --at X,Y [--font-size <size>] [--color <hex>]
       pkb-excalidraw FILE connect --from <id1> --to <id2> [--label "<label>"] [--color <hex>]
       pkb-excalidraw FILE set-text <id> "<new_text>"
       pkb-excalidraw FILE fit <id> "<new_text>"
       pkb-excalidraw FILE move-elem <id> [--to X,Y | --by DX,DY]
       pkb-excalidraw FILE delete-elem <id> [--cascade-arrows]
       pkb-excalidraw FILE batch <changes.json | - >

Theme Commands:
       pkb-excalidraw FILE theme export [out.json]
       pkb-excalidraw FILE theme apply <theme.json | default | retro-terminal | aops-default> [--all | --id <id>]
```

`FILE` and the mode may be given in either order; with no mode, `summary` is assumed ([L2784-L2801](../bin/pkb_excalidraw.rs#L2784-L2801)). Also accepted but not in the usage text: `update-node --id <id> [--angle] [--roughness] [--fill-style] [--preset]` ([L2972-L3025](../bin/pkb_excalidraw.rs#L2972-L3025)) — it is missing from the `known_modes` list ([L2784-L2789](../bin/pkb_excalidraw.rs#L2784-L2789)), so only the `FILE update-node …` order works; `--angle`, `--roughness`, `--fill-style` and `--preset hero|sticky|zone|badge` on `add-node` ([L3075-L3084](../bin/pkb_excalidraw.rs#L3075-L3084)); and `--curved` / `--stroke-style` on `connect` ([L3163-L3213](../bin/pkb_excalidraw.rs#L3163-L3213)).

### Projections (read-only)

| Command | Output |
|---------|--------|
| `summary` | Per-type element counts and scene extents (`extents: x [min, max]  y [min, max]`) ([L1503](../bin/pkb_excalidraw.rs#L1503)) |
| `map` | One TSV line per top-level element — `id  type  geometry  backgroundColor  label` (bound text folded into its container), arrows as `id  arrow  from -> to  [label]` ([L1551](../bin/pkb_excalidraw.rs#L1551)) |
| `nodes` | TSV: `id  type  x,y  WxH  role  label` |
| `edges` / `arrows` | TSV: `id  from -> to  [label]` |
| `inspect <id>` / `get <id>` | Formatted details / raw pretty JSON of one element |
| `style` | Value histogram for `roughness`, `fillStyle`, `strokeStyle`, `strokeWidth`, `fontFamily`, `fontSize`, `opacity`, plus the modal `roundness` of rectangles ([L1700](../bin/pkb_excalidraw.rs#L1700)) |
| `diff FILE2` / `struct-diff FILE2` | Element-level diff / semantic node-and-edge diff that ignores coordinate jitter |
| `lib` / `item SELECTOR --after INDEX [--at X,Y]` | List `.excalidrawlib` items; emit one item (`#N` or unique name substring) re-minted with fresh ids and indices after `INDEX`, to stdout |

### Audits

| Command | Exit 0 when |
|---------|-------------|
| `check` | Ids unique; every element has an `index`; array sorted by `index`; no half-bound arrows; all `startBinding`/`endBinding`/`containerId`/`boundElements` resolve; `text`/`originalText` agree ([L1763-L1951](../bin/pkb_excalidraw.rs#L1763-L1951)) |
| `overlap` | No AABB collisions between top-level boxes (arrows, bound text and fully nested boxes are ignored) |
| `arrows-check` | No arrow segment crosses a box it is not bound to |

### Mutations

Every mutating command ends in `atomic_save` ([L462-L495](../bin/pkb_excalidraw.rs#L462-L495)): sort elements by fractional `index`, run `check`, refuse to save if it fails, write `.tmp.<pid>.<rand>` beside the file, `rename`. `connect` requires both endpoints to exist and writes both `startBinding`/`endBinding` and the reciprocal `boundElements` entries. `delete-elem` also removes the paired text and unbinds touching arrows (or deletes them with `--cascade-arrows`). New ids are 21 chars; new fractional indices append to the current maximum.

`batch` takes a JSON array (file or `-` for stdin) of `{"action": …}` objects and applies them in one save ([L3304-L3469](../bin/pkb_excalidraw.rs#L3304-L3469)):

| `action` | Fields |
|----------|--------|
| `add-node` | `type` (default `rectangle`), `text`, `at: [x,y]`, `size: [w,h]`, `role`, `color`, `id`, `angle`, `roughness`, `fill_style`, `preset` |
| `add-text` | `text`, `at`, `font_size`, `color` |
| `connect` | `from`, `to`, `label`, `color`, `curved`, `stroke_style` |
| `set-text` / `fit` | `id`, `text` |
| `move` | `id`, `to` or `by` |
| `delete` | `id`, `cascade_arrows` |
| `update-node` | `id`, `angle`, `roughness`, `fill_style`, `preset` |
| `theme-apply` | none (default theme, all elements) |

Action names accept `snake_case` and `kebab-case` aliases (`update_node`, `add_node`, `add_text`, `move-elem`/`move_elem`, `delete-elem`/`delete_elem`, `apply-theme`/`apply_theme`), and camelCase field aliases (`fillStyle`, `fontSize`, `strokeStyle`, `cascadeArrows`) ([L3337-L3456](../bin/pkb_excalidraw.rs#L3337-L3456)). Any failing op aborts the whole batch before anything is written.

### Themes and presets

`theme apply default|retro-terminal|aops-default` all resolve to the built-in retro theme (hachure fill, roughness 2, stroke `#404040`, text `#1a1a1a`, roles `emphasis #c9b458 · success #8fbc8f · info #7a9fbf · warning #ffa500 · error #ff6666 · surface #252525 · muted #888888`, [L283-L311](../bin/pkb_excalidraw.rs#L283-L311)); any other argument is read as a theme JSON path. `--role <name>` stores `customData.role` and picks the fill from that table. Preset colours for `hero`, `sticky`, `zone`, `badge` can be overridden by a JSON file at `$EXCALIDRAW_THEME_PATH` (default `~/.gemini/config/excalidraw_theme.json`), read once per process ([`schema.rs:153-192`](schema.rs#L153-L192)).

### Exit codes

`0` on success and on a clean `check`/`overlap`/`arrows-check`; `1` for usage errors, unreadable or unparseable JSON, a failed pre-save `check`, or any detected collision. Two things do **not** fail: a parseable file with no `elements` (e.g. `{}`) yields a zero-element `summary` and exit 0, and `delete-elem <unknown id>` prints `OK: deleted element <id>` and exits 0 ([L3282-L3303](../bin/pkb_excalidraw.rs#L3282-L3303)) — only `move-elem`, `set-text`/`fit` and `inspect`/`get` exit 1 on an unknown id. Output is plain text except `get`, `item` and `theme export`, which emit JSON.
