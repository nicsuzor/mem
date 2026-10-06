---
id: home-directory-placement
title: "Home-directory placement: save notes that belong to a subproject in the subproject's directory"
type: spec
status: draft
tags: [spec, pkb, crud, placement, create]
created: 2026-10-06
task: task_741317bc
---

# Home-directory placement

**Status**: Draft. Not yet implemented. Awaiting approval.
**Implementation site**: `src/document_crud.rs` (`create_document`, `default_subdir_for_type`), `src/mcp_server/handlers_document.rs` (`handle_create_document`), `src/mcp_server/schemas.rs` (`create` tool).
**Tests**: `tests/home_directory_placement.rs` (new).

## 1. Problem and target

### 1.1 Request

Nic, 2026-10-06: *"when we save notes that clearly belong to a subproject, they should be saved in the subproject's path. e.g. those two joel notes should have been saved under hdr/joel"*.

### 1.2 What happened

On 2026-10-06 an agent created two notes, `note_0e1c112e` (Joel's PhD mindmap argument structure) and `note_6ca5834f` (popularity vs availability as Argdown), with `create`, `type: note`, `parent: hdr-982d610d`. Both landed in `notes/`.

Both notes were derived from `hdr/joel/Joel PhD Mindmap 261006.excalidraw`. Both bodies name that canvas as their source. Five other Joel canvases already live in `hdr/joel/` (`list_excalidraw dir=hdr`). So Joel's material had a directory, and the notes were saved somewhere else.

### 1.3 Why it happened

`create_document` picks the directory like this (`src/document_crud.rs`):

```rust
let subdir = fields
    .dir
    .map(|d| expand_env_vars(&d))
    .unwrap_or_else(|| default_subdir_for_type(&fields.doc_type).to_string());
```

The only inputs are an explicit `dir` and the document type. `parent`, `source` and the location of the source artefact are never consulted. Any `note` created without `dir` goes to `notes/`. The writer did not pass `dir`, and nothing in the tool led it to.

### 1.4 Target

When a non-task document is created without `dir`, the server puts it in the directory of the material it belongs to, if there is a clear structural signal for that. Otherwise it keeps today's default. The rule is deterministic and inspectable. Every create response says where the document went and why.

"Clearly belongs" is defined structurally and never by semantic guess:

1. **Source signal.** The document's `source` field names an existing file inside the PKB. The document goes next to that file.
2. **Ancestor signal.** The nearest ancestor on the `parent` chain whose file sits in a *home directory* (§2.1). The document goes into that directory.

### 1.5 Non-goals

- No inference from body text, tags, titles or embeddings. The bodies of the two Joel notes mention the canvas path in prose. Parsing prose for paths is rejected because it fails silently in both directions.
- Tasks, epics, learns, targets, goals, capabilities and memories are out of scope. `create_task`, `create_memory`, `convert_document` and the routing of task types through `create` do not change.
- Existing documents are not moved automatically. See §6.
- No new directories are created by inference.

## 2. Definitions

### 2.1 Routing directory and home directory

A **routing directory** is a directory that holds documents because of their type or repo, not their subject:

- the PKB root itself (`""`)
- the fixed type directories: `tasks`, `targets`, `goals`, `memories`, `notes`, `projects`
- any top-level directory whose name is a canonical project slug in `polecat.yaml`. These directories are filled by `create_task`'s `project` routing (`subdir = project`).

A **home directory** is any directory under the PKB root, relative and free of `..`, that is not a routing directory. Examples: `hdr`, `hdr/joel`. A node's **home** is the directory containing its file, if that directory is a home directory.

### 2.2 In-scope types

Placement inference applies only when `default_subdir_for_type(type) == "notes"`. As of 0.3.97, that is every type except `task`, `epic`, `learn`, `target`, `goal`, `capability` and `memory`.

## 3. Architecture and data flow

```
create(args)
  └─ handle_create_document                       (handlers_document.rs)
       ├─ build DocumentFields (unchanged)
       ├─ placement = resolve_placement(&pkb_root, &graph, &fields)   ◄─ NEW
       │     1. fields.dir is Some             → (dir,            Explicit)
       │     2. type not in scope (§2.2)       → (type default,   TypeDefault)
       │     3. source names a file under root,
       │        and its directory is a home dir → (file's dir,    Source)
       │     4. walk parent chain from fields.parent,
       │        nearest ancestor with a home    → (ancestor home, Ancestor(id))
       │     5. otherwise                       → (type default,  TypeDefault)
       ├─ fields.dir = Some(placement.dir)
       ├─ create_document(&pkb_root, fields)      (unchanged write path)
       └─ response text includes placement.dir and placement.reason
```

Only the handler changes. `create_document` already honours `fields.dir` and validates it with `is_safe_relative_path`. Placement is resolved before the write, so the existing write, git-commit and incremental-index path runs unchanged.

### 3.1 Source resolution (step 3)

`source` is free-form "source context" today and stays that way. It counts as a source signal only if all of these hold. Otherwise step 3 is skipped silently and never raises an error:

- it contains no URL scheme (`://`) and does not start with `/` or `~`
- `is_safe_relative_path(source)` is true
- `pkb_root.join(source)` is an existing regular file
- after canonicalisation, that file is still inside `pkb_root`, so a symlink cannot escape
- the file's parent directory, relative to the root, is a home directory (§2.1)

The source file can be of any kind (`.excalidraw`, `.pdf`, `.md`, …). It does not need to be a graph node.

### 3.2 Ancestor walk (step 4)

- Start at `fields.parent`. Resolve each ID with the graph (`GraphStore::resolve`) and read `GraphNode.path`.
- If the node's directory is a home directory, return it with reason `Ancestor(<node id>)`.
- Otherwise move to that node's `parent` and repeat.
- Stop, falling through to step 5, on any of these: an ID that does not resolve, a node with no parent, a node seen before (cycle), or 16 steps.
- The walk only reads the in-memory graph. It does not scan the filesystem.

### 3.3 Precedence rationale

- An explicit `dir` beats everything because the writer said so.
- Source beats ancestor because it is more specific. In the motivating case, the parent `hdr-982d610d` (Supervise HDR students) covers every student, while the source canvas pins the note to Joel.

## 4. Interface contracts

### 4.1 Internal

```rust
// src/document_crud.rs
pub enum PlacementReason { Explicit, Source, Ancestor(String), TypeDefault }
pub struct Placement { pub dir: String, pub reason: PlacementReason }

/// Pure apart from read-only filesystem stat calls (source file existence,
/// canonicalisation). Never creates directories, never errors.
pub fn resolve_placement(
    root: &Path,
    graph: &GraphStore,
    fields: &DocumentFields,
    project_slugs: &HashSet<String>,
) -> Placement;

pub fn is_routing_dir(rel_dir: &str, project_slugs: &HashSet<String>) -> bool;
```

`project_slugs` holds the canonical slugs from `polecat_config`. If `polecat.yaml` is absent, the set is empty.

### 4.2 MCP `create` tool

Input schema: no new parameters. Description changes:

| Field | New description |
|---|---|
| tool | "Generic document creation. Placement when `dir` is omitted: task types → `tasks/`; targets → `targets/`; memories → `memories/`; other types → the directory of `source` if it is a PKB-relative path to an existing file, else the home directory of the nearest `parent` ancestor (see specs/home-directory-placement.md), else `notes/`." |
| `source` | "Source context. If this is a PKB-relative path to an existing file (e.g. `hdr/joel/Joel PhD Mindmap 261006.excalidraw`), the document is saved in that file's directory unless `dir` is given." |
| `dir` | "Subdirectory relative to the PKB root. Overrides all placement inference." |

Success response: the first line changes from

```
Document created: `<filename>` (`<id>`)
```

to

```
Document created: `<filename>` (`<id>`) in `<dir>/` — placed by <reason>
```

`<reason>` is one of `dir`, `source`, `parent <ancestor-id>`, `type default`. Any existing warning lines follow unchanged.

### 4.3 Spec sync

`specs/pkb-server-spec.md` line "**Routing**: Documents auto-route to subdirectories by type (tasks/ projects/ goals/ notes/)." is replaced with a one-sentence summary of §3 and a link here, in the same PR as the implementation.

## 5. Acceptance criteria and tests

All tests go in `tests/home_directory_placement.rs`. They build a temp PKB with this fixture and call the MCP `create` handler:

```
hdr/hdr.md                       id: hdr-epic        type: epic
hdr/joel/joel.md                 id: joel-epic       type: epic   parent: hdr-epic
hdr/joel/mindmap.excalidraw      (non-markdown file)
tasks/t1.md                      id: t1              type: task   parent: joel-epic
tasks/t2.md                      id: t2              type: task   (no parent)
mem/t3.md                        id: t3              type: task   parent: hdr-epic
notes/existing.md                id: existing        type: note
tasks/ca.md                      id: ca              type: task   parent: cb   (cycle)
tasks/cb.md                      id: cb              type: task   parent: ca
polecat.yaml                     projects: { mem: … }
```

Each criterion is falsifiable by its test.

| # | Criterion | Test |
|---|---|---|
| AC1 | A note with `source: hdr/joel/mindmap.excalidraw` and no `dir` is written to `hdr/joel/<id>_<slug>.md`. | `source_file_places_in_its_directory`: assert the file exists at that path and not in `notes/`. |
| AC2 | An explicit `dir` wins over every signal: `dir: notes`, `source: hdr/joel/mindmap.excalidraw`, `parent: joel-epic` → `notes/`. | `explicit_dir_wins` |
| AC3 | A `source` that is a URL, an absolute path, contains `..`, names a missing file, or names a file in a routing directory (`notes/existing.md`) has no effect on placement and raises no error. | `non_file_sources_fall_through`: one case per form; each lands at the next applicable rule. |
| AC4 | A note with `parent: joel-epic` and no `source` → `hdr/joel/`; reason `parent joel-epic`. | `parent_home_directory` |
| AC5 | The nearest home ancestor wins: `parent: t1` (in `tasks/`) → walk passes `t1`, stops at `joel-epic` → `hdr/joel/`, not `hdr/`. | `nearest_home_ancestor_wins` |
| AC6 | Routing directories are never chosen by the ancestor rule: `parent: t2` → `notes/`; `parent: t3` (in project dir `mem/`) skips `mem/` and reaches `hdr-epic` → `hdr/`. | `routing_dirs_skipped` |
| AC7 | The walk terminates and falls back to the type default without error for `parent: ca` (cycle), `parent: no-such-id` (unresolvable; `create` does not validate `parent` today), and a 20-deep chain of tasks in `tasks/`. | `walk_terminates`: three cases; the test finishes in < 1 s. |
| AC8 | Source beats ancestor: `source: hdr/joel/mindmap.excalidraw`, `parent: hdr-epic` → `hdr/joel/`. | `source_beats_parent` |
| AC9 | Out-of-scope types keep today's placement: `type: task, parent: joel-epic` → same directory as before this change. A `memory` created through `create` → `memories/`. | `out_of_scope_types_unchanged`: compare against the 0.3.97 routing table. |
| AC10 | With no signals, a note goes to `notes/`. This is a regression guard. | `no_signal_defaults_to_notes` |
| AC11 | Every create response states the directory and reason in the §4.2 format. | Asserted as a regex in AC1, AC2, AC4, AC10. |
| AC12 | Inference never creates a directory. The directory set under the root is identical before and after AC1–AC10, except for the explicit `dir` in AC2, which already exists. | `no_directories_created` |
| AC13 | A document placed in a nested home directory is immediately retrievable: `get_document(id)` returns it, and `search` finds it by title, with no rebuild call. | `nested_placement_is_indexed` |
| AC14 | The `create` schema descriptions match §4.2, and `pkb-server-spec.md` no longer claims type-only routing. | Extend `tests/schema_doc_integrity.rs`: assert the `source` description contains "saved in that file's directory" and the spec routing line links this file. |

### 5.1 Verification beyond unit tests

After merge, on the live PKB:

- Re-run the motivating write with `source: "hdr/joel/Joel PhD Mindmap 261006.excalidraw"`, `parent: hdr-982d610d`, `type: note`.
- Confirm the response line reads `in \`hdr/joel/\` — placed by source`.
- Confirm the file is under `hdr/joel/`.
- Then delete the probe note.

## 6. Remediation of existing documents and complements

- **The two Joel notes.** Move them with the existing `convert_document(id, type: "note", dir: "hdr/joel")`. It renames in place, keeps the ID and records a git rename (`pkb-server-spec.md` §convert). This is a data fix and does not depend on this spec.
- **Writers must set `source`.** The motivating notes named their source only in prose. The source signal only fires if the writer passes the path in `source`. The note-writing skills in academicOps (for example, the argument-extraction and excalidraw-reading flows) should set `source` to the PKB-relative artefact path. That change belongs to that repo.
- **A durable Joel home.** Under §3.2, a node for Joel's supervision whose file lives in `hdr/joel/` makes `hdr/joel/` the home for everything parented to it, with no `source` needed. Today Joel's tasks and notes are parented directly to `hdr-982d610d` (e.g. `task_98721beb`, `hdr_joel_ch5_feedback_20260923`).

## 7. Open questions for approval

1. **Ancestor rule reach.** As written, a note parented anywhere under an epic whose file is in a home directory lands in that directory. That includes `hdr/` for any HDR note, if `hdr-982d610d`'s file is in `hdr/`. Its on-disk location is not visible through the MCP API and was not checked. Accept, or restrict the rule to the immediate parent only?
2. **Routing-directory list.** Are there other top-level directories in the live PKB that hold documents by type rather than subject (for example `archive`, `contacts`, `daily`) and should be added to §2.1's fixed list?
