---
title: "View: Metro (Routes to Destinations)"
type: spec
status: active
tier: ux
parent: planning-web
tags: [spec, planning-web, view, metro, priority-paths, destinations, visualization]
created: 2026-04-07
updated: 2026-04-23
---

# View: Metro (Routes to Destinations)

**Question answered:** "What do I need to do to get to each destination?"

Part of the [[planning-web]] spec. Separated from [[view-graph]] so that Force handles pure topology and Metro makes the route to each priority outcome visually traceable.

---

## Purpose

The metro map anchors the user's **destinations** — P0/P1 outcomes still to be reached — and arranges every incomplete dependency above them so the route to each destination reads as a single visual sweep.

Destinations are fixed terminal stations along the bottom of the map. Upstream blockers are positioned relative to the destinations they serve. When a blocker lies on the route to more than one destination, it naturally falls between those destinations' lanes and becomes an **interchange** — the highest-leverage work, visually emergent.

Force shows the forest. Metro shows the tracks to the destinations that matter.

---

## The User

Nic has 500+ tasks across many projects. He cannot hold in working memory what stands between him and each priority outcome. The metro map makes that set visible at a glance: which destinations exist, what lies on the route to each, and where routes converge on shared blockers.

---

## ADHD Design Principles

- **Destinations are anchors, not decorations.** Each P0/P1 outcome is a labelled terminus the eye can find and return to. If you can't immediately see the destinations, the view has failed.
- **The path is the answer.** Tracing from a destination back through its blockers must be a single visual sweep — no edge-following through density required.
- **Overview first, labels on zoom.** At map scale, shape, colour, and position convey meaning. Labels unlock as the user zooms in (via `min-zoomed-font-size`). Destinations and interchanges keep their labels at every zoom level.
- **Interchanges are emergent.** Shared blockers reveal themselves through geometry and edge blending, not through a bespoke marker.
- **Static positions, not animated physics.** Running a force simulation each mount destroys the mental model. Positions are computed once per graph structure and preserved across renders.

---

## Data Model

### Destinations (terminal stations)

A **destination** is any node that is **both**:

1. incomplete (status in `INCOMPLETE_STATUSES`), and
2. priority 0 or 1.

That's it. The user sets priorities to name their targets; this view renders every target as a terminal regardless of whether the node happens to be a leaf. Whether a destination has children does not decide *whether* it is a destination — it decides how the route to it is walked (see below).

Destinations are ordered deterministically: priority asc, then project, then label. Order defines their horizontal slot.

### Routes

For each destination `d`, the **route to `d`** is the set of incomplete nodes reached by walking `depends_on` / `soft_depends_on` edges upstream **and** walking `parent` edges in the direction dictated by `d`'s shape:

- **Container destinations** (type `goal`/`epic`, or any node with at least one incomplete child): walk `parent` parent → child (descendants) up to `GOAL_PARENT_HOP_CAP` hops, plus `depends_on` on every collected node. The subtree *is* the route — these destinations are reached by completing the work they contain.
- **Leaf destinations** (no incomplete children, not a container type): walk `parent` child → parent — the containing epic/project hierarchy becomes part of the route, plus `depends_on`. This is deliberate: a shared parent becoming an interchange is *useful* information ("this epic hosts three P0/P1 outcomes"), not noise.

Completed nodes are hidden or heavily dimmed — they are already-traversed track.

Each node carries `routes: Set<destinationId>` computed once during preparation.

### Station types

| Station type | Criteria | Visual role |
|---|---|---|
| **Terminal** | Node is a destination | Largest, always labelled, priority-coloured border, project fill |
| **Interchange** | `routes.size ≥ 2` | Enlarged, always labelled, edges blend at its position |
| **Route station** | `routes.size == 1` | Sized by downstream_weight, label on zoom |
| **Context station** | `routes.size == 0` | Track-width dot, no label, low opacity |

---

## Layout — target-anchored preset

This is the heart of the view. Layout is **not** force-directed; it is computed once and applied as a Cytoscape `preset`.

Algorithm:

1. Let `N` = number of destinations. Destinations are staggered into `rowCount = ceil(N / TERMINAL_PER_ROW)` rows (capped at 4) along the bottom band — adjacent destinations in the ordering go to different rows so their labels don't collide at dense N. Destination `i` gets anchor `(x_i, y_terminalBase − (i mod rowCount) × rowGap)` with `x_i` equally spaced across the horizontal extent.
2. For every non-destination route node, compute `depth` = shortest-path distance to the nearest destination it serves, following the direction chosen for that destination (leaf: deps; container: parent-down + deps). Its y-coordinate is `topOfTerminals − depth × rowHeight`.
3. Its x-coordinate is `mean(x_d for d in node.routes)`. Nodes on a single route sit directly above their destination; interchanges drift to the mean of their destinations' anchors, landing between lanes.
4. Context stations (empty routes) are **hidden by default**. They're noise for the "routes to destinations" question, and at this PKB's scale they overplot catastrophically. A "Show context" toggle surfaces at most `CONTEXT_CAP` (top-N by `downstream_weight`) in a bucketed grid above the route area.
5. Final collision pass: bucket by `(round(x/gridX), round(y/gridY))`; within a bucket, sort by stable `idHash` and nudge ±Δ along x to separate siblings. Sorting by hash (not id string) keeps layout stable when ids are added or removed.

The algorithm exploits the fact that the task graph is already a DAG with well-defined destinations — no need for Sugiyama or dagre.

---

## Visual Encoding

### Stations (nodes)

- **Terminals**: labelled, enlarged (≈3× base), priority-coloured border, project-coloured fill. Bloom on selection (à la Tokyo's start/end stations).
- **Interchanges**: slightly larger than route stations, always labelled, distinguished by multi-colour edge convergence at their position.
- **Route stations**: sized by `downstream_weight`, status-coloured fill, label revealed when zoom passes `min-zoomed-font-size`.
- **Context stations**: diameter ≤ track width, no label, low opacity.
- **Completed nodes**: desaturated and shrunk, regardless of station type.

### Lines (edges)

- **Thick, semi-transparent, straight.** Haystack-style rendering (Cytoscape `curve-style: haystack`, `haystack-radius: 0`): cheap, overlap-blending, and matches the metro-map aesthetic.
- **Width**: parent-route ≈ 6–8 px; `depends_on` route ≈ 4–5 px; non-route references 1 px.
- **Opacity**: ≈ 0.45 for route edges so colours mix where they overlap. That blending is how interchanges communicate "this blocker serves multiple routes" — no extra markup.
- **Colour** by the destination(s) a route serves. Single-route edges: coloured by that destination's project. **Multi-route edges render one stroke per shared destination (mandatory — Tokyo "duplicate-per-line" trick)**; each stroke at ~0.4 opacity so browser alpha compositing stacks the colours at the interchange. A single blended stroke is no longer an acceptable compromise — without per-route strokes the view cannot meet the "colours visibly mix" acceptance criterion on real data.
- **Direction**: `depends_on` carries a directional arrow; `parent` is undirected.
- **Non-route edges**: thin, grey, low opacity — present for context.

This spec deliberately drops the earlier "taxi/orthogonal routing" prescription in favour of haystack. Straight, blending edges are simpler and produce the target aesthetic directly.

---

## Interactions

- **Click a terminal** → enter route-highlight mode. Every node with that destination in its `routes` stays bright; everything else gets `.not-path` (opacity ≈ 0.12). This is the Tokyo A\* pattern repurposed — and it is the primary answer to the view's centring question.
- **Click any station** → highlight all nodes that share at least one route with the tapped station (i.e. "which destinations does finishing this unblock?"). Also opens the shared detail panel.
- **Click empty space** → clear highlight, restore full map.
- **Hover station** → neighbourhood flashlight: dim non-neighbours **and** show an HTML overlay tooltip anchored to the node with: title, priority, status, project, and the labels of up to 6 destinations the station serves (with a `+N more` count if truncated). Positioning is computed from `renderedPosition()` and translated into the container's coordinate frame. Implemented as a plain Svelte `{#if tooltip}` overlay — no `cytoscape-popper`/`tippy.js` dependency needed.
- **Click terminal in legend** → isolate that one route (dim others).

---

## Acceptance Criteria

### Destination handling

- [ ] Every incomplete P0/P1 leaf appears as a labelled terminal station.
- [ ] Terminal positions are deterministic and stable across re-renders of the same graph.
- [ ] Clicking a terminal highlights the full upstream route and dims everything else.

### Layout

- [ ] Layout is computed by the target-anchored algorithm and applied via Cytoscape `preset`; no live force simulation on mount or refresh.
- [ ] Y-axis encodes depth-to-destination; x-axis encodes route membership.
- [ ] Nodes with `routes.size ≥ 2` land between the relevant terminals' lanes.

### Visual encoding

- [ ] Route edges use haystack (or equivalent straight, thick, semi-transparent) rendering; overlap visibly blends colours at interchanges.
- [ ] Labels hidden at overview zoom (`min-zoomed-font-size`) except on terminals and interchanges, which remain labelled.
- [ ] Completed nodes are desaturated and shrunk.
- [ ] Context (non-route) nodes render at track width with no label at default zoom.

### Data integrity

- [ ] Route membership is computed purely from `depends_on` + `parent` edges and the destination set; deterministic for equal graph inputs.
- [ ] Interchange detection matches nodes where `routes.size ≥ 2`.
- [ ] A node reachable from no destination has empty routes and is classed as a context station.

### Interaction

- [ ] Click terminal ⇒ route highlight. Click station ⇒ routes-through highlight. Click empty ⇒ clear.
- [ ] Hover preserves the neighbourhood flashlight and shows a tooltip with destination memberships.

---

## Scope Boundary

| Need | Handled by |
|---|---|
| What's blocking outcome X? | Metro (this view) |
| Full network topology | Force view |
| Size/weight distribution | Treemap |
| Hierarchy quality | Circle Pack |
| Ranked next actions | Focus / Arc |
| Session context recovery | Dashboard |

---

## References

- **Cytoscape.js Tokyo Railways demo** — https://github.com/cytoscape/cytoscape.js/tree/unstable/documentation/demos/tokyo-railways (note: `master` branch 404s; use `unstable`). Three ideas borrowed directly:
  1. **Preset layout with coordinates baked into data** — gives a genuine map feel instead of a tidy schematic. We compute coordinates from destination anchors + depth rather than from geography, but the principle is identical.
  2. **Haystack edges at width ~20 and opacity ~0.5** — overlap blending is what makes interchanges visually emergent.
  3. **A\* path highlight with `.not-path` dimming** — repurposed as "click a destination, see everything in its route".
- Shared infrastructure: `projectUtils.ts` (`projectColor`), `constants.ts` (`PRIORITY_BORDERS`, `INCOMPLETE_STATUSES`). Reachable-filter semantics carry over from [[view-graph]].
