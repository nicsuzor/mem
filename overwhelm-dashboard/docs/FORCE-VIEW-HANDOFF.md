# ForceView Handoff: Cola Layout

## Current State

`src/lib/components/views/ForceView.svelte` (~220 lines) is a bare-minimum WebCola graph layout. It renders task cards as nodes, groups them hierarchically, and lets Cola handle positioning.

### What it does
- Renders all graph nodes via `buildTaskCardNode` (centered at origin, Cola center-coordinate model)
- Builds hierarchical Cola groups from parent links (`buildColaGroups`)
- Renders group boxes as simple rects from Cola's computed `cg.bounds`
- Async layout via `cola.d3adaptor(d3)` with `.on("tick", tickVisuals)`
- Start/stop button (`toggleRunning()` export, bound in `+page.svelte`)
- Click selects nodes/groups, drag repositions

### Settings wired up (via `$viewSettings` in `viewSettings.ts`)
- `colaLinkLength` (default 300, slider 30-1000) — ideal distance between linked nodes
- `colaConvergence` (default 0.01, slider 0.001-0.09) — convergence threshold (BROKEN, see below)

### Constants
- `CANVAS_AREA = 30_000_000` — determines canvas dimensions from viewport aspect ratio
- `GROUP_PADDING = 60` — padding inside Cola group boxes

## Cola Architecture

### How Cola works
Cola uses **stress majorization** — it minimizes the difference between graph-theoretic distances and geometric distances. Each tick runs a Runge-Kutta integration step on the stress function, then projects constraints (avoidOverlaps, group containment).

### The async tick path (`d3v4adaptor.js`)
```
kick() → d3.timer(tick)  // fires once per animation frame (~60fps)
tick() → if (_alpha < _threshold) stop; else rungeKutta() → set _alpha = displacement
```

### Key Cola API calls in ForceView
```typescript
cola.d3adaptor(d3)
    .size([cw, ch])
    .nodes(nodes)           // must have .width, .height set
    .links(parentLinks)     // only type='parent' links, resolved to objects
    .groups(colaGroups)     // hierarchical { leaves, groups, padding }
    .linkDistance(300)       // ideal link distance
    .convergenceThreshold(0.01)
    .avoidOverlaps(true)    // rectangle overlap removal
    .handleDisconnected(true) // pack disconnected components
    .on("tick", tickVisuals)
    .on("end", callback)
    .start()                // async — returns immediately, ticks via d3.timer
```

### Synchronous alternative
```typescript
.start(10, 30, 200, 0, false)  // blocks UI but gives full control
// args: unconstrained, userConstraint, allConstraint, gridSnap, keepRunning
```
The sync path blocks the main thread (~10-40 seconds for 500 nodes). Use async.

## Known Issues

### 1. Convergence threshold is broken in async mode
Cola reuses `_alpha` for two purposes:
- **Initial kick value**: `resume()` sets `_alpha = 0.1`
- **Running displacement**: after first tick, `_alpha = s1` (sum of squared position changes)

The convergence check (`_alpha < threshold`) runs BEFORE the first `rungeKutta()`:
- **threshold >= 0.1** → stops immediately (0.1 < threshold, no work done)
- **threshold < 0.1** → starts, but displacement is typically thousands for this graph, so it never stops

In Cola's sync `run()` method, convergence uses **relative stress change** (`|old/new - 1| < threshold`), which works correctly. The async path doesn't do this.

**Workaround options:**
1. Remove convergenceThreshold, let Cola run to natural convergence (displacement < 0.01)
2. Patch Cola's `tick()` to use relative stress change like `run()` does
3. Implement a custom tick wrapper that tracks relative stress change and calls `.stop()` manually

### 2. Node overlaps persist (~36-50 overlap pairs with 148 nodes)
Cola's `avoidOverlaps(true)` and hierarchical group constraints compete. The solver has finite iterations and can't always satisfy both "keep nodes inside groups" and "no overlaps." Increasing `linkDistance` helps (stress and avoidOverlaps pull in the same direction when ideal distance > node size).

### 3. handleDisconnected trade-off
- `true`: packs disconnected components into a grid AFTER the constraint solver. Can create new overlaps between components.
- `false`: disconnected components stay at random initial positions, guaranteed overlaps.

Currently set to `true` — less bad.

### 4. linkDistance vs node dimensions
Cards are ~180px wide. If `linkDistance` < card width, stress and avoidOverlaps fight each other (stress wants nodes closer, avoidOverlaps pushes apart). Keep linkDistance > card width. Default 300.

## Coordinate Model

Both Cola and `buildTaskCardNode` use **center coordinates**:
- Cola: `node.x`, `node.y` = center. Bounds = `x ± width/2, y ± height/2`
- `buildTaskCardNode`: draws from `(-w/2, -h/2)` to `(w/2, h/2)`
- SVG `<g>` is translated to `(d.x, d.y)`

No coordinate conversion needed — they match.

## Group Hierarchy

`buildColaGroups` builds Cola's native hierarchical groups:
1. Scans parent links to find parent → child relationships
2. Creates `{ leaves: [nodeIndices], groups: [], padding }` for each parent with children
3. Wires nesting: if a group's container is itself a child of another group, nests via `groups[]`
4. Deduplicates leaf indices (Cola requires each node in exactly one group's `leaves`)

Group boxes are rendered from `cg.bounds` (Cola's computed `Rectangle {x, X, y, Y}`).

## Files

| File | Role |
|------|------|
| `src/lib/components/views/ForceView.svelte` | Main component (~220 lines) |
| `src/lib/stores/viewSettings.ts` | Settings store (colaLinkLength, colaConvergence) |
| `src/lib/components/shared/ViewConfigOverlay.svelte` | Settings UI sliders |
| `src/lib/components/shared/NodeShapes.ts` | `buildTaskCardNode` — renders cards |
| `src/lib/data/prepareGraphData.ts` | Sets `w`, `h` on nodes (card body dimensions) |
| `src/routes/+page.svelte` | Binds ForceView, start/stop button |
| `node_modules/webcola/dist/src/layout.js` | Cola Layout — tick(), start(), convergence |
| `node_modules/webcola/dist/src/descent.js` | Descent — rungeKutta(), run(), stress calc |
| `node_modules/webcola/dist/src/d3v4adaptor.js` | d3 v4 adaptor — kick(), trigger() |

## What was removed in this refactor

- Project node stripping (complex parent remapping)
- Property patching reactive block
- Dimming + selection highlight reactive blocks
- Zoom-scale text visibility
- Group labels
- Nested group visual distinction (different fill/stroke/dash)
- Custom link distance calculation (avgW/avgH based)
- Debounced rebuild watching multiple Cola setting params
- CSS dimming styles
- `zoomScale` import
