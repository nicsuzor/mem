---
title: "View: Graph (Task Map)"
type: spec
status: active
tier: ux
parent: planning-web
tags: [spec, planning-web, view, graph, task-map, visualization, force-graph]
created: 2026-02-23
---

# View: Graph (Task Map)

**Question answered:** "How does my work connect?"

Part of the [[planning-web]] spec. This spec consolidates the former `task-map.md` (academicOps) into the planning-web view structure.

---

## Purpose

The task map is a structural overview of the user's entire work graph. Its job is to show how work _connects_ — the shape of the network, where bottlenecks are, where effort has impact across branches.

The task map is the only view that shows cross-project structure. Other views handle session context recovery (Dashboard), prioritized next actions (Focus), and hierarchy (Epic Tree). The task map shows the forest.

---

## The User

Nic is an academic with ADHD who runs parallel workstreams across multiple machines, terminals, and projects. His working memory is limited but his ambition isn't — at any given time there are 500+ incomplete tasks across research, tooling, governance, and teaching. He is building this system for himself because off-the-shelf project management tools don't work for how his brain operates.

The task map exists because Nic's brain can't hold the whole project graph. He needs an external representation that does the cognitive work his working memory can't: showing what connects to what, where work is stuck, and where effort would have the most impact across the network.

---

## ADHD Design Principles

- **Scannable, not studyable.** The graph must communicate at a glance. If the user has to zoom in and read individual labels to get oriented, it's failed. Structure, color, size, and spatial grouping should carry meaning before any text is read.
- **Active work dominates.** Completed and stale nodes must not compete for visual attention with work that needs action. The reachable-leaf filter handles this at the data level; the visual encoding must reinforce it.
- **No flat displays at scale.** 500 same-sized circles is noise, not information. Visual hierarchy (size, shape, emphasis) must create entry points and scannable structure.
- **Support focus transitions.** The hardest ADHD moment is shifting from "seeing everything" to "working on one thing." The graph should support both modes: full overview for orientation, and single-project focus for commitment.

---

## Node Encoding

- **Size** varies by `downstream_weight` — structural nodes (goals, projects) visually larger than leaf tasks
- **Shape/outline** varies by type — goals, projects, epics, and leaf tasks are visually distinguishable (circles, rounded squares, hexagons, etc.)
- **Fill colour** by status (blue=active, green=done, red=blocked, yellow=waiting, purple=review, grey=cancelled)
- **Recency emphasis** — recently-modified nodes are brighter/saturated; stale nodes (>14 days) desaturate; very stale (>30 days) fade further
- **Blocked + high downstream_weight** creates a distinctive danger signal (size + red)

## Edge Encoding

- Parent edges: solid, heavier
- `depends_on`: distinct colour (red/orange), bold
- `soft_depends_on`: dashed, lighter
- Wikilinks: dotted, subtle

## Layout Options

Top-Down, Left-Right, Radial, Force-directed (ForceAtlas2)

### How to Read Each Layout

#### ForceAtlas2 — "Where does everything live?"

Force-directed layout where connected nodes pull together and unconnected nodes push apart. You should see **project clusters** — groups of related tasks huddled together. The core shows your most interconnected work; the periphery shows isolated items.

#### Treemap — "How big is everything?"

Each rectangle's area is proportional to a project's task count. Nested rectangles show hierarchy. The biggest rectangles are where most of your work lives.

#### Circle Pack — "What's structured vs chaotic?"

The central circles show well-organized hierarchies. The radiating arms show items without clear project structure.

#### Arc — "What needs my attention NOW?"

Filtered to only ~200 active, important tasks ranked by downstream weight. The 5 horizontal bands are node types (goals at top, actions at bottom). Arcs show dependencies.

---

## Interactions

- Click node: opens detail panel, highlights neighbourhood, dims unrelated nodes
- Click empty space: clears selection
- Project filter: dropdown restricts to single project subgraph + cross-project dependencies
- Zoom: progressive label reveal (goals/projects always labelled; tasks visible on zoom)
- Hover: tooltip with title, status, downstream_weight

## Legend

Compact strip near the graph (not buried in settings). One or two lines. Must match what's rendered.

## Visual Settings (collapsible panel)

| Setting      | Range       | Default | Purpose                   |
| ------------ | ----------- | ------- | ------------------------- |
| Node Size    | 1-20        | 6       | Base size of nodes        |
| Link Width   | 0.5-5.0     | 1.0     | Edge thickness            |
| Text Size    | 6-24        | 12      | Base font size            |
| Link Opacity | 0.1-1.0     | 0.6     | Edge transparency         |
| Repulsion    | -500 to -10 | -100    | Force layout spacing      |
| Show Labels  | toggle      | On      | Label visibility          |
| Hide Orphans | toggle      | Off     | Remove disconnected nodes |

## Type Filter

Multiselect for node types. Default for task view: goal, project, epic, task, action, bug, feature, learn.

## Reachable Filter

Default on — shows only actionable leaves + their ancestor chains. Completed nodes appear only as structural ancestors. Toggle off for full graph.

### filter_reachable Algorithm

1. Identifies **leaves**: incomplete work-item nodes with no incomplete children (types: task, project, epic, bug, feature, review)
2. Walks **upstream** from each leaf through parent, `depends_on`, and `soft_depends_on` edges
3. Keeps all reachable nodes; completed nodes in the reachable set become **structural** (displayed differently)
4. Everything else is pruned

---

## Data Sources

- Tasks view: `$ACA_DATA/outputs/graph.json` (produced by `pkb graph`, run on cron)
- Knowledge Base view: `$ACA_DATA/outputs/knowledge-graph.json`

---

## User Stories

### US-TM1: I can see the forest, not just the trees

**As** Nic returning to work after time away,
**I want** the task map to show me the _structure_ of my work at a glance — which projects are tangled, which are linear, where clusters form,
**So that** I get oriented in seconds, not minutes.

**Acceptance test:** Nic can identify which project a cluster belongs to without reading any labels, just from spatial grouping and the visual weight of the project node anchoring the cluster.

### US-TM2: I can see where work is alive

**As** Nic trying to figure out what's happening across my workstreams,
**I want** recently-modified nodes to stand out from stale ones,
**So that** I can immediately see where effort is concentrated and where things have gone quiet.

**Acceptance test:** Without reading a single label, Nic can point to the 2-3 areas of the graph where work happened today.

### US-TM3: I can see what's stuck and what it's blocking

**As** Nic scanning for bottlenecks,
**I want** blocked nodes and their downstream impact to be immediately visible,
**So that** I can spot high-impact blockers without tracing edges manually.

**Acceptance test:** Nic can identify the highest-impact blocker in the graph within 5 seconds, without clicking anything.

### US-TM4: I can drill into a node and understand its context

**As** Nic who has spotted something interesting in the graph,
**I want** to click a node and immediately see its title, status, parent chain, children, and dependencies,
**So that** I can understand what it is and where it fits without leaving the dashboard.

**Acceptance test:** Nic clicks a node and within 1 second has enough context to decide "this needs attention" or "this is fine" without opening any other tool.

### US-TM5: I can read the graph without a manual

**As** Nic (or eventually, a colleague looking at my dashboard),
**I want** the visual encoding to be self-explanatory,
**So that** I don't have to remember what colors and sizes mean.

**Acceptance test:** Someone who has never seen the dashboard before can explain what the colors and sizes mean after looking at the graph for 10 seconds.

### US-TM6: I can focus on one project without the rest distracting me

**As** Nic who has decided to work on one specific project,
**I want** to filter the graph to show only that project's subgraph,
**So that** I can see its full structure without visual noise from everything else.

**Acceptance test:** Nic selects a project from the filter and sees only related nodes, clearly laid out, with enough detail to pick his next task.

### US-TM7: Labels tell me what things are, not what they're called in the database

**As** Nic trying to read the graph,
**I want** node labels to show human-readable titles, not task IDs,
**So that** I can identify what each node represents.

**Acceptance test:** At default zoom, Nic can read every project name. At any zoom level, no labels overlap to the point of illegibility.

### US-TM8: Edge types tell me about the nature of relationships

**As** Nic trying to understand why things are connected,
**I want** to distinguish between hierarchy edges, hard dependencies, and soft links,
**So that** I can trace parent chains and blocker chains through the graph.

**Acceptance test:** Nic can visually trace a dependency chain through the graph without confusing it with the parent hierarchy.

---

## Scope Boundary

The task map handles the **structural overview**. These adjacent needs belong to other views:

| Need                                   | Handled by                 |
| -------------------------------------- | -------------------------- |
| "What was I doing in that terminal?"   | Dashboard (session cards)  |
| "What got done today?"                 | Dashboard (synthesis)      |
| "What's my single next action?"        | Focus view                 |
| "How do I recover my dropped threads?" | Dashboard (dropped threads)|

---

## Acceptance Criteria

### Rendering

- [ ] Task graph renders without freezing browser
- [ ] Knowledge Base graph view displays notes and wikilinks
- [ ] Graph loads within 2 seconds for typical data size (~500 nodes after filtering)
- [ ] Node selection shows task/note details

### Visual Encoding

- [ ] Node size varies by `downstream_weight`
- [ ] Node shape or outline varies by type
- [ ] Edge style varies by relationship type
- [ ] Recency emphasis: recently-modified nodes visually brighter; stale nodes recede
- [ ] Blocked nodes with high downstream weight create a distinctive visual signal
- [ ] Labels use `title` field, not task ID; progressive reveal by zoom level
- [ ] Compact legend strip visible without opening an expander

### Interaction

- [ ] Clicking a node shows detail panel
- [ ] Clicking a node highlights its neighborhood; dims unrelated nodes
- [ ] Project filter dropdown restricts graph to single project subgraph
- [ ] Clicking empty space clears selection

### Filtering

- [ ] `filter_reachable` produces no false positives
- [ ] Default view shows only reachable nodes; completed-only subtrees are pruned

---

## Known Bugs

### Completed tasks leaking through the reachable filter

Some completed (green) nodes appear in the reachable view that shouldn't be there. Possible causes: wikilinks creating unexpected upstream paths, edges being traversed bidirectionally, or stale graph data.

**User expectation:** A completed task should never appear in the reachable view unless you can trace exactly why it's there.
