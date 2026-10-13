---
id: graph-visualisation-export
title: "PKB graph data and Excalidraw primitives for visualising a subgraph"
type: spec
status: draft
tags: [spec, pkb, graph, export, excalidraw, visualisation]
created: 2026-10-08
task: mem_6695fcf9
---

# PKB graph data and Excalidraw primitives

**Status**: Draft. Not implemented. Awaiting approval.
**Implementation sites**: `src/graph_store.rs` (`output_json_filtered`, `output_dot`), `src/graph.rs` (`ContributesTo`), `src/excalidraw/` (layout, reader, diff, merge, schema), `src/bin/pkb_excalidraw.rs`, `src/mcp_server/schemas.rs` and `handlers_batch.rs` (`export_graph`, `graph_excalidraw`, `diff_excalidraw`, `sync_excalidraw`), `src/cli.rs` (`pkb graph`, `pkb excalidraw`).

## 1. Overview

mem gives any consumer the data to draw a chosen part of the PKB graph, and gives an agent drawing it in Excalidraw the general tools to do the design work. It also gives a faithful round trip: an unedited canvas changes nothing, and a hand edit is offered back without clobbering either side.

### 1.1 The boundary

mem provides data, selection, a faithful round trip, an edit-preserving merge, general canvas primitives, and rendering. It makes no design choices for any particular map. Layered columns, headings, the weight-to-width scale, the kind-and-state encoding, legends, badges and every other visual decision belong to the agent using the `tools:diagram` skill. The dashboard is a separate consumer of the same data. Sibling specs:

| Concern | Spec |
| --- | --- |
| Drawing PKB graphs in Excalidraw: layout, encoding, legends, badges, procedure | nicsuzor/academicOps `specs/tools/diagram-pkb-graphs.md` |
| Dashboard force view: layered subgraph, `/api/graph` passthrough | nicsuzor/overwhelm-dashboard `specs/view-force-layered.md` |

Those specs cite requirement IDs from this one (`GV-…`). They do not restate them, and this spec does not restate theirs.

### 1.2 Tags on criteria

- **[now]**: buildable against the model as it stands.
- **[needs model change]**: depends on a model change Nic has decided but that is not built. This spec says only how the export carries the change once it exists.

## 2. Current-state contract

### 2.1 What this spec builds on

| Capability | Where | Spec |
| --- | --- | --- |
| Verbal contribution-weight scale, synonyms, case-insensitive match, `multiplier` | `ContributesTo::numeric_weight`, `is_recognized_weight` (`src/graph.rs`) | `specs/ranking.md` §7, §7.1; `specs/pkb-rules.md` §6.4 |
| Graph export as JSON or DOT, hop-based focus, `project`, `include_done` | `export_graph` (MCP), `pkb graph` (CLI) | `specs/ranking.md` §8.5 |
| Excalidraw export, 3-way diff, sync, merge into an existing canvas | `graph_excalidraw` / `diff_excalidraw` / `sync_excalidraw`, `pkb excalidraw export\|diff\|sync` | `src/excalidraw/README.md` |
| Canvas files in the PKB by path | `list_excalidraw` / `get_excalidraw` / `write_excalidraw` | `specs/pkb-server-spec.md` §Data Format |
| File-level primitives: `query` (type, bbox, dot-path equality filters), `batch`, `apply`, `update --set`, `move-elem`, `align`, `distribute`, `group`, `lock`, `set-text`/`fit`, `snapshot`, `summary`, `describe`, `overlap`, `arrows-check`, `screenshot` | `pkb-excalidraw` | `specs/excalidraw-tooling.md` |

### 2.2 Constraints this spec must remove

These are properties of the current code that the requirements below must not keep. Each is listed with the requirement that removes it.

| Constraint in current code | Removed by |
| --- | --- |
| `Edge` serialises only `source`, `target`, `type`; DOT edges carry only `type`. | GV-D1, GV-D3 |
| The parse-warning check trims the stated term, but `numeric_weight` does not, so a padded term passes the warning and scores zero. | GV-D2 |
| `extract_ego_subgraph` stops adding nodes at 100 and says nothing. The same function feeds `export_graph` with `focus` and `graph_excalidraw`. | GV-S2 |
| The JSON `focus` array is a focus-pick list, not the requested node. | GV-S1 (response names its seeds) |
| Ego extraction iterates a `HashSet`, so card placement differs between identical calls. | GV-X4 |
| Card text truncates titles to 36 characters and tag lines to 32, and the reader takes card text back as title and tags. | GV-X3, GV-R1, GV-R2 |
| Sync writes `depends_on`, `soft_depends_on` and `parent` edges only. Every other edge type falls to `_ => {}` and is dropped without a report. | GV-R4, GV-R11 |
| The hand-drawn id regex `(task\|epic\|mem\|target\|goal)-[a-zA-Z0-9]{4,16}` rejects `{prefix}_{8 hex}` ids and unlisted prefixes. | GV-R6 |
| `retargeted_edges` is declared but never populated. | GV-R10 |
| `PkbCustomData.weight` is declared but never written. Arrows carry only `edgeType`, `sourceId`, `targetId`, `isPkbManaged`. | GV-X2 |
| Merge overwrites a card's colours when its status changes, even if the colours were set by hand. | GV-M4 |
| `arrows-check` tests only the chord from an arrow's first point to its last, not each segment. | GV-P4 |
| `query` filters match by equality only. | GV-P1 |
| `screenshot --format png` with no rasteriser on `PATH` writes an SVG and reports success. | GV-I1 |

## 3. Definitions

- **Selection**: a set of nodes, plus the edges between them, chosen by one of the selectors in §4.2. Every export in this spec takes a selection.
- **Managed element**: a canvas element carrying `customData.pkb` with `isPkbManaged: true`. Cards, their bound text, and arrows written by mem are managed. Everything else is **unmanaged**.
- **Presentation change**: a change to how a managed element looks, made without intending to change PKB data. Examples are restyling, re-wrapping, shortening displayed text, and repositioning. Adding unmanaged elements such as legends, headings and badges is also a presentation change.
- **Data change**: a change the diff proposes to write back to the PKB: titles, tags, status, nodes, edges, weights.
- **Layer**: a label on each node derived by the rule in GV-L1.
- **Sinks**: the end points a trace or path query runs to. By default these are nodes in the `future` and `goal` layers.

## 4. Requirements

### 4.1 Edge records carry their attributes

- **GV-D1 Edge attributes in JSON [now].** In `export_graph` JSON, every `contributes_to` edge record carries `stated_weight` (the term as written), `weight` (the value the engine scores with), `weight_valid`, and `justification`. When a `multiplier` is set, it is carried too. `weight_valid` is `true` for a recognised term or float, `false` for a non-empty unrecognised term, and `null` for an unstated weight. Other edge types carry no weight fields.
  - *Test*: build a fixture with one edge per verbal term, one capitalised term, one synonym (`possible`), one out-of-scale term (`medium`), one padded term (`" expected "`), and one unstated weight. Every `contributes_to` edge record has the four keys. The `medium` and padded edges have `weight_valid: false` and `weight: 0.0`. The unstated edge has `weight_valid: null` and `weight: 0.0`.
- **GV-D2 Export says what the engine scores [now].** For every `contributes_to` edge, the exported `weight` equals `ContributesTo::numeric_weight()` for that edge, and `weight_valid` agrees with it. A term that scores zero only because it is unrecognised is never marked valid. The parse-warning check and the scorer use one recognition function, so a term warned about is a term scored zero, and the reverse.
  - *Test*: for every edge in the GV-D1 fixture, compare the export with `numeric_weight()`. For every edge with `weight_valid: false`, the source node's `parse_warnings` names it, and no other edge is named.
- **GV-D3 Edge attributes in DOT [now].** DOT `contributes_to` edges carry the same values. They use attribute names that Graphviz does not read as layout instructions. `weight` is a Graphviz layout attribute, so the PKB value travels as `pkb_weight` and `stated_weight`, plus a `label` holding the term.
  - *Test*: `dot -Tsvg` renders the GV-D1 fixture's DOT export with no warnings. Each `contributes_to` edge line carries `pkb_weight`, `stated_weight` and `label`.
- **GV-D4 Model-change fields pass through [needs model change].** When the model supplies probability or certainty, a sign or signed weight, a derived effective weight, a non-linear severity on targets, a satisfied state, or a provenance marker for unconfirmed estimates, the same edge and node records carry them. They use the model's field names, unrenamed and not recomputed by the export. The existing `multiplier` (`specs/ranking.md` §7.1) is not one of these, and the export does not present it as one.

### 4.2 Selection that is not hop-based

- **GV-S1 Closure selector [now].** One selector, accepted by `export_graph`, `graph_excalidraw` and `pkb excalidraw export`, returns the closure over the chosen edge types from a seed set. Its parameters are:
  - seeds, by tag, by id, or both;
  - direction: `upstream`, `downstream` or `both`;
  - edge types to follow (default per GV-S7).

  Tag seeding reads tags from the in-memory graph, not the search index. The response names the seeds it resolved and the parameters it applied, so that no consumer has to infer the hub from another field. Hop-based `focus`/`hops` remains.
  - *Test*: on the career-set fixture (GV-F1), seeding on the futures tag in both directions returns exactly the GV-F1 node set. It returns no node that is reachable from a seed only through `parent` or `link`. It does return a feeder of a resource-layer target that sits more than two hops from any seed.
- **GV-S2 Truncation is reported [now].** Any node limit that cuts a result sets `truncated: true` and gives the number of nodes dropped. This applies to every selector and to the hop-based path.
  - *Test*: a fixture larger than the limit returns `truncated: true` with the correct dropped count from both `export_graph` and `graph_excalidraw`. A result under the limit returns `truncated: false`.
- **GV-S3 Cut edges and outside counts [now].** The response includes only edges between returned nodes. For each returned node it includes two counts over the edge types followed: `outside_feeders` (nodes outside the selection that feed it) and `outside_destinations` (nodes outside the selection it feeds). A renderer can then mark a cut edge, with its count, without drawing the far node.
  - *Test*: in a fixture where node A in the selection has two contributors and one destination outside it, A carries `outside_feeders: 2` and `outside_destinations: 1`, and no edge to those nodes is returned.
- **GV-S4 Trace selections [now].** Two named selections are built on GV-S1 and GV-S5:
  - `trace-from <id>`: the nodes and `contributes_to` edges on some path from `<id>` to any sink;
  - `trace-into <id>`: every node with a `contributes_to` path into `<id>`, and those edges.

  Sinks are configurable, with the default in §3. Both are available to `export_graph` and to the Excalidraw export.
  - *Test*: for a work node in GV-F1, every node in `trace-from` lies on a `contributes_to` path from it to a sink, computed independently with GV-S5, and every such path's nodes are present. For a future-layer node, `trace-into` equals its upstream `contributes_to` closure.
- **GV-S5 Path query [now].** Given a source and a set of sinks, return every simple `contributes_to` path, with each edge's GV-D1 fields. The query is bounded and reports truncation as in GV-S2. A product of weights along a path may be returned only under a name that says it is a product. It is not a ranking signal, and no mem output sorts by it.
  - *Test*: on a fixture with two paths of different lengths from one work node to one sink, both paths are returned with their per-edge weights.
- **GV-S6 Done nodes [now].** The Excalidraw export takes an option to include or exclude nodes with a completed status. Its default is open decision 7. This does not change `export_graph`'s existing `include_done` default.
  - *Test*: with the option off, no node with a completed status appears. With it on, the completed nodes in the selection appear.
- **GV-S7 Edge-type filter [now].** The edge types to draw or return are a parameter. For the closure selector and the Excalidraw export, the default is `contributes_to` only. Frames and `parent` containment are drawn only when asked for. See open decision 6.
  - *Test*: a default career-set Excalidraw export contains no `frame` element and no arrow whose `customData.pkb.edgeType` is anything other than `contributes_to`.

### 4.3 Layer

- **GV-L1 Layer derivation [now].** Each node record in a selection carries a `layer`, derived by one documented rule from a mapping that is data, not code. The mapping is a named configuration, stored in the PKB or passed with the request, and a subgraph can define its own. The first mapping is `career`:

  | Rule, first match wins | Layer |
  | --- | --- |
  | tag `career-future` | `future` |
  | tag `career-resource-class` | `resource` |
  | tag `career-audience` | `audience` |
  | tag `career-stakeholder` | `stakeholder` |
  | tag `career-channel` | `channel` |
  | tag `career-guard` | `guard` |
  | type `goal`, or a `target` with no outgoing `contributes_to` | `goal` |
  | any other `target` | `resource` |
  | anything else | `work` |

  - *Test*: the GV-F1 export's per-layer counts equal counts computed at test time by applying the table directly to the fixture's tags and types.
- **GV-L2 Layer order [now].** The mapping also gives the layer order (for `career`: work, channel, stakeholder, audience, resource, future, goal; guard outside the flow), and the order is returned with the selection. Edges that skip layers or run backwards are legal and are returned like any other. mem does not use the order to place cards; GV-X5 governs placement.

### 4.4 Derived counts

- **GV-C1 Feeder and onward counts [now].** Each node in a selection carries `feeder_count` and `onward_count` over `contributes_to`, counted across the whole graph, not just the selection. Together with GV-S3, a consumer can tell a true gap (count 0) from a cut edge (outside count above 0).
  - *Test*: in GV-F1, a target with no contributors has `feeder_count: 0`, a goal-layer node has `onward_count: 0`, and the guard has both at 0.

### 4.5 Shared test fixture: the career set

- **GV-F1 Career set [now].** This is the subgraph every selection, layer and round-trip test in this spec and its siblings is phrased against. It is defined by a rule:
  1. Seeds: every node tagged `career-future`.
  2. Add every node that reaches a seed along `contributes_to`, any number of steps.
  3. Add every node a seed reaches along `contributes_to`.
  4. Add every node tagged `career-guard`, which may have no edges.

  The concrete ids live in the PKB, not here. Tests compute expected sets and counts from the export at test time and never hard-code them. In the repo, an anonymised fixture PKB with the same shape carries the tests. That shape has:
  - every layer in GV-L1;
  - a goal reached only from futures;
  - a non-career target that feeds a future;
  - a guard with no edges;
  - one target with no feeders;
  - a feeder more than two hops from any seed;
  - an unrelated node reachable from the set only through `parent`;
  - every weight case in GV-D1;
  - one completed node.

  A live verification runs the same assertions against the PKB.

### 4.6 Excalidraw export, in general terms

- **GV-X1 Export any selection [now].** `graph_excalidraw` and `pkb excalidraw export` accept every selector in §4.2, plus the existing hop-based focus. They return the scene together with the selection's metadata (seeds, parameters, `truncated`, layer order).
- **GV-X2 Full record in `customData.pkb` [now].** Every managed element carries the full record of what it represents, so a consumer never reads data from display text.
  - Cards carry: `nodeId`, full `title`, `tags`, `status`, `nodeType`, `layer`, `feederCount`, `onwardCount`, `outsideFeeders`, `outsideDestinations`, and the existing `intent` and `parent`.
  - Arrows carry: `edgeType`, `sourceId`, `targetId`, and for `contributes_to` the GV-D1 fields (`statedWeight`, `weight`, `weightValid`, `justification`, `multiplier` when set).

  Key names follow the existing camelCase convention. The unused string `weight` field is replaced, not overloaded.
  - *Test*: for every card and arrow in a GV-F1 export, `customData.pkb` matches the corresponding `export_graph` record field for field.
- **GV-X3 Card text is never truncated [now].** A card's text contains the node's full title, wrapped over as many lines as needed, and the card grows to fit. No text extends outside its card. This is a data-fidelity rule: truncation is what corrupts the round trip. How the title is styled or shortened for display afterwards is a presentation change (GV-R9).
  - *Test*: for every card, the title in the bound text equals the node's `title`, ignoring line breaks. Every bound-text bounding box lies inside its container. This includes the fixture's longest title, which is at least 160 characters.
- **GV-X4 Deterministic output [now].** The same selection and the same data, with the same mem version, give identical element ids, positions, sizes and text.
  - *Test*: export GV-F1 twice and compare `(id, x, y, width, height, text)` for every element. The two sets are equal.
- **GV-X5 Default placement [now].** The exporter places cards by a simple documented default that is valid and has no card-to-card overlap. It does not lay out columns by layer, route arrows, or draw legends or headings. Those are the agent's, using §4.9.
  - *Test*: `pkb-excalidraw FILE overlap` exits 0 on a GV-F1 export, with frames off.
- **GV-X6 No containment by default [now].** Without an explicit request, the export draws no frames and no `parent` arrows (GV-S7).
- **GV-X7 Stated limit [now].** Help text for `graph_excalidraw` and `pkb excalidraw export` states that following connections inside the editor (for example, Excalidraw's "select connected") is not supported, and that tracing is done by re-exporting with GV-S4.

### 4.7 Re-export into an existing canvas

- **GV-M1 Keep hand placement, add new nodes [now].** Re-exporting a selection into an existing canvas file:
  - keeps the position and size of every managed card already there;
  - updates the `customData.pkb` record and card text from live data;
  - adds a card for each node new to the selection, flagged `newSinceLastExport: true` in `customData.pkb` and placed without overlapping any existing element.

  How a new card looks is the agent's choice.
  - *Test*: export, move and resize three cards, add a node to the selection in the fixture PKB, then re-export. The three cards keep their geometry, the new card exists with the flag, and `overlap` exits 0 for the new card.
- **GV-M2 Unmanaged elements are untouched [now].** A re-export never alters, reorders out of z-order, or removes an element without `customData.pkb`: free text, sketches, hand-drawn groups and frames, legends.
  - *Test*: every unmanaged element is byte-identical in the JSON before and after re-export, including fields mem's typed schema does not model.
- **GV-M3 Leaving the selection is marked, never deleted [now].** A managed card whose node is no longer in the selection, or no longer exists, stays on the canvas with `customData.pkb.inSelection: false`.
  - *Test*: remove a node's seed edge in the fixture and re-export. Its card remains, with the flag.
- **GV-M4 Hand overrides are kept [now].** If a managed card's colours, stroke or shape differ from what mem last wrote, re-export keeps them and records the fact in `customData.pkb` (for example, `styleOverridden: true`). This holds when the node's status has changed too. Status still updates in `customData.pkb` and in the card text.
  - *Test*: change one card's background, change its node's status in the fixture, then re-export. The background is unchanged, `status` is updated, and the override is recorded.

### 4.8 Round trip

- **GV-R1 An unmodified export diffs clean [now].** `diff_excalidraw` on the untouched output of any export in §4.6 reports zero entries in every array, with or without a base. This is the release gate for this spec: no change to §4.6 or §4.7 merges while it fails.
  - *Test*: export GV-F1, then diff with no base and with the export as base. Every array is empty.
- **GV-R2 Data is read from `customData` [now].** Title and tags are read from `customData.pkb`. Card text is read as a title only when it differs from the text mem last wrote for that card, and that comparison is against a stored copy, never against a re-derivation of the display. Tags are never read from card text on a managed card.
  - *Test*: re-wrap every card's text without changing its words. Diff reports no `updated_nodes`. Then edit one card's words, and diff reports exactly that node's retitle, with the new text in full.
- **GV-R3 Diff and dry-run report canvas edits to edges [now].** `diff` reports, and `sync --dry-run` previews:
  - a retitled card;
  - a new arrow between two managed cards, as a proposed `contributes_to` edge. Its weight is taken from the arrow's label if the label is a recognised term, and is otherwise flagged as needing a weight;
  - a changed weight label on an existing `contributes_to` arrow, as a proposed re-weight;
  - a deleted managed arrow, as a proposed removal.

  Whether sync may then write these is open decision 5.
  - *Test*: one fixture canvas carries each case, and each is reported once in the right class.
- **GV-R4 Sync writes contribution edges only when complete [now].** `sync` writes a `contributes_to` addition or re-weight only when it has a recognised weight term and a non-empty justification, taken from the arrow's `customData` or supplied by the caller. It removes an edge only with an explicit flag. It never deletes a node. Every written node is re-read, and the report includes its `parse_warnings`, which must be empty.
  - *Test*: an arrow with a valid term and a justification is written. One missing either is reported and not written.
- **GV-R5 Conflicts are not written [now].** With a base snapshot supplied, a field changed both on the canvas and in the PKB since the base is reported as a conflict and not written. This covers every field sync can write.
  - *Test*: change one title in both places. Sync writes nothing for that node and lists the conflict.
- **GV-R6 Hand-typed ids link [now].** A hand-drawn card whose text contains a real node id links to that node. This covers every id form the PKB mints or accepts, including `{prefix}_{8 hex}`, `{prefix}-{hex}`, and slug-style ids with underscores. Linking is confirmed against the live graph, not a fixed prefix list.
  - *Test*: hand-drawn cards carrying an underscore id, a hyphen id with an unlisted prefix, and a non-existent id. The first two link. The third is reported as a new node.
- **GV-R7 Protected fields are never written [now].** `sync` never writes `intent`, `standing_weight`, `severity` or `due`, whatever the canvas contains.
  - *Test*: alter each in `customData.pkb`. A dry run lists no change to any of them, and a live sync leaves them byte-identical.
- **GV-R8 Further edge fields round-trip [needs model change].** Probability or certainty, and sign, edited on an arrow label round-trip under the GV-R3 and GV-R4 rules for weight.
- **GV-R9 Presentation changes are not data changes [now].** mem provides a documented route by which an agent applies presentation changes to managed elements. Examples are restyling, re-wrapping, and shortening displayed text (such as dropping a title prefix that a layer already conveys). After such a change, `diff` reports no data change. A later hand edit of the same card's text is still detected as a retitle. Unmanaged elements are never read as data. mem chooses the mechanism, for example a display baseline in `customData.pkb` that the route updates, or a presentation flag. `visual_mutations` remain informational and are never written.
  - *Test*: export GV-F1. Through the route, restyle every card and arrow, shorten one card's display text, and add unmanaged headings and a legend. Diff reports zero data changes. Then hand-edit that card's text, and diff reports one retitle.
- **GV-R10 Retargeted arrows are reported as such [now].** A managed arrow whose endpoint binding moved to another card is reported in `retargeted_edges`, not as a removal plus an addition.
- **GV-R11 Nothing is dropped silently [now].** Any canvas change that sync does not write is listed in the sync report with the reason: an unsupported edge type, a refused write, or a missing weight.
  - *Test*: a canvas adding one arrow of each edge type sync does not write produces one report entry per arrow.

### 4.9 General canvas primitives for the agent

These are general primitives. They know nothing about layers, careers or encodings. They live in `pkb-excalidraw` (`specs/excalidraw-tooling.md`). Existing commands are cited, and only the gaps are specified.

- **GV-P1 Select by record, change in one batch [now].** Select elements by `customData.pkb` fields with equality, set membership and numeric comparison (for example, every arrow with `weight >= 0.75`, or every card with `layer` in `{future, goal}`), and apply one property change to all of them in one atomic save. This extends `query` (equality only) and `batch`/`apply`.
  - *Test*: one command sets `strokeWidth` on every `contributes_to` arrow with `weight >= 0.75` and on no other element. `check` passes afterwards.
- **GV-P2 Move and arrange keep identity [now].** The existing `move-elem`, `align`, `distribute`, `group` and `update --set` must keep `customData.pkb`, ids and arrow bindings intact on managed elements.
  - *Test*: after an `align` and a `distribute` over managed cards, GV-R1 still holds and every arrow stays two-bound.
- **GV-P3 Measure before placing [now].** Return the wrapped width and height of a given text at a given font size and maximum width, using the same metrics `fit` uses. Return the extents of a set of elements by id. `summary` already gives whole-scene extents.
- **GV-P4 Audits [now].** `overlap` covers standalone text against shapes, so a heading overlapping a card is caught. `arrows-check` tests every segment of a multi-point arrow. Both can emit machine-readable output: a count, plus the offending pairs.
  - *Test*: a three-point arrow whose middle segment crosses an unrelated card fails `arrows-check`, and the same arrow routed around the card passes. A text element on a card fails `overlap`.
- **GV-P5 Arrow routing aid [now].** Set waypoints on a bound arrow while keeping both bindings, and offer a route that avoids every shape other than its endpoints, or reports that none was found. This is enough for an agent to make `arrows-check` exit 0 on a layered scene. mem does not choose the layout.
  - *Test*: in a scene where a straight arrow crosses an unrelated card, the routing aid yields an arrow that stays two-bound, passes `arrows-check`, and leaves GV-R1 holding.

### 4.10 Rendering

- **GV-I1 One render command [now].** One documented command renders a canvas file to PNG and to SVG, at a requested scale. It runs in the container image agents work in. When PNG is requested and cannot be produced, the command exits non-zero and names what is missing. It never writes SVG in place of PNG while reporting success. The existing `pkb-excalidraw FILE screenshot` is the candidate.
  - *Test*: in that container, render the GV-F1 export to PNG at 2x. The output is a valid PNG of the scene's extents times the scale. With the rasteriser removed, the command exits 1.

## 5. Boundaries

mem does not:

- lay out columns by layer, draw headings, legends, badges or summary counts, or choose a weight-to-width scale or a kind-and-state encoding. These belong to nicsuzor/academicOps `specs/tools/diagram-pkb-graphs.md`;
- serve the dashboard's `/api/graph` or decide how the force view uses these fields. These belong to nicsuzor/overwhelm-dashboard `specs/view-force-layered.md`;
- infer data from prose. No "satisfied" state is read from a body, and no "unconfirmed estimate" marker from a justification;
- sort, rank or recommend futures or paths by any computed score;
- index canvases as graph nodes (`specs/pkb-server-spec.md` §Data Format).

### 5.1 Out of scope

- The model changes themselves: probability or certainty, non-linear severity, negative contributions, a satisfied state, a provenance field.
- Pricing targets, or changing weight, intent, severity or due, except a `contributes_to` write under GV-R4 if open decision 5 allows it.
- The dashboard's other views.
- Hand-built maps and their separate builder.
- Excalidraw+ or any hosted canvas service, and live multi-user editing.
- Fixing PKB search or `list_documents` tag filtering. GV-S1 reads tags from the graph instead.

## 6. Open decisions (Nic)

1. **Build order.** Recommended: §4.1–§4.4 first, then GV-X3 with GV-R1 and GV-R2 (clean round trip), then the rest of §4.6 and §4.9 so a read-only map can be drawn, then §4.7 and the rest of §4.8. Reason: the dashboard and the skill both need §4.1–§4.4, and nothing else in the Excalidraw path is safe until an unmodified canvas diffs clean.
2. **`possible` and the vocabulary.** The source request lists `possible` as outside the vocabulary and wants its edge marked invalid. `specs/ranking.md` §7 and `numeric_weight` treat `possible` as a synonym of `uncertain` (0.25). This spec follows `ranking.md`, so `possible` is valid. Recommended: keep the synonym. If Nic wants synonyms rejected, that is a change to `ranking.md` §7, not to the export.
3. **Where the goal end sits.** Under GV-L1, a target that feeds futures takes the `resource` layer, even if Nic thinks of it as a goal. Recommended: let the edges decide, as the rule does, and leave the goal layer to nodes with no onward edge unless Nic adds edges from futures to such a target.
4. **Knowledge notes.** Notes tagged into a subgraph but with no `contributes_to` edges are outside GV-F1 and outside the default closure. Recommended: leave them out by default. An agent can add them by seeding on id or tag.
5. **Weights from the canvas.** Should sync write a `contributes_to` addition or re-weight drawn on the canvas (GV-R3, GV-R4)? Recommended: yes, through dry-run and confirmation only, because the canvas is where Nic will be looking when he sets weights. This needs his ruling. It meets two existing rules:
   - `specs/pkb-rules.md` §6.5, under which only Nic or a supervised scorer sets `stated_weight`. A sync run by an agent must carry Nic's edit, not the agent's;
   - the standing rule for maps that agents do not author parentage, weight or status on a map, because the map is a projection.

   Until he rules, GV-R3 reports and GV-R4's write path stays behind an explicit flag.
6. **Edge types by default.** Recommended: `contributes_to` only (GV-S7), with `depends_on` as an option, and `parent` and body links off.
7. **Done nodes by default in the Excalidraw export.** Recommended: include them (GV-S6), because finished work that built a resource is part of how a future became reachable. How they look is the skill's choice.
8. **Unconfirmed-estimate marker.** No field records it. Recommended: do not parse prose. Treat it as a small model addition (a provenance value on the edge) decided with the other edge changes. GV-D4 then carries it.
