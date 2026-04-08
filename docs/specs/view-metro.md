---
title: "View: Metro (Priority Paths)"
type: spec
status: active
tier: ux
parent: planning-web
tags: [spec, planning-web, view, metro, priority-paths, visualization]
created: 2026-04-07
---

# View: Metro (Priority Paths)

**Question answered:** "What are the priority paths?"

Part of the [[planning-web]] spec. Separated from [[view-graph]] so that Force handles pure topology and Metro handles priority dependency chains.

---

## Purpose

The metro map extracts and highlights the dependency chains leading to P0 and P1 outcomes. It answers: "what's blocking my most important work, and where do the chains overlap?"

Each priority chain is rendered as a named "metro line" colored by project. Where chains cross (shared blockers, common ancestors), interchange stations appear — these are the highest-leverage intervention points.

Force shows the forest. Metro shows the trails that matter.

---

## The User

Nic has 500+ tasks across many projects. He needs to see which dependency chains lead to his most important outcomes, and where those chains share common blockers. The metro map makes critical paths visually obvious without requiring him to trace edges through a dense graph.

---

## ADHD Design Principles

- **Priority paths dominate.** Non-priority stations are background context, not foreground noise. They render at or below track width — visible but not competing for attention.
- **Interchange stations are the insight.** Where two priority chains share a blocker, that's the highest-value thing to see. Interchanges must be visually distinctive.
- **Chains, not clouds.** Unlike Force (spatial clusters), Metro shows linear paths. The layout should emphasize directionality — upstream blockers at top, downstream outcomes at bottom.
- **Scannable depth.** At overview zoom, you should see: how many priority lines exist, which are long (deep dependency chains), and where they cross. No label reading required.

---

## Data Model

### Priority chain extraction

1. Identify all P0 and P1 **leaf tasks** (incomplete, no incomplete children)
2. For each leaf, walk upstream through `depends_on` and `parent` edges
3. Each complete path (leaf → root ancestor) is a "metro line"
4. Nodes appearing on multiple lines are **interchange stations**

### Station types

| Station type | Criteria | Visual |
|---|---|---|
| **Priority station** | Node is P0/P1, or is on a priority chain with incomplete work downstream | Large, labeled, colored by status |
| **Interchange** | Node appears on 2+ priority lines | Extra-large, distinctive marker, always labeled |
| **Context station** | Node not on any priority chain | Small (≤ track width), no label at default zoom |

---

## Visual Encoding

### Lines (edges)

- **Priority parent edges**: Thick metro lines colored by project. Taxi/orthogonal routing.
- **Priority dependency edges**: Dashed, amber/orange, with directional arrows. Thinner than parent lines but still prominent.
- **Non-priority edges**: Thin, grey, low opacity. Visible for context but not competing.

### Stations (nodes)

- **Priority stations**: Sized by downstream_weight. Status fill color. Priority border. Label visible at default zoom.
- **Interchange stations**: Larger than priority stations. White or distinctive border. Multiple line colors visible (e.g. split fill or ring segments). Always labeled.
- **Context stations**: Diameter ≤ line width. Status color fill. No label (reveal on zoom). Low opacity.
- **Completed stations**: Dimmed and smaller than their type would normally be.

### Layout

- Directional: upstream (blockers/ancestors) at top, downstream (outcomes/leaves) at bottom.
- Lines should run roughly vertically with taxi-curve bends at branches.
- Interchange stations should be visually centered where lines cross.

---

## Interactions

- Click station: opens detail panel (shared with other views)
- Hover station: flashlight — dims non-neighborhood, shows tooltip with title, status, which lines pass through this station
- Click line label/color: filter to show only that line's stations
- Click empty space: clears selection

---

## Acceptance Criteria

### Rendering

- [ ] Metro view renders without freezing for typical graph size
- [ ] Only P0/P1 chains are visually prominent; non-priority nodes are background
- [ ] Interchange stations are visually distinctive and always labeled

### Visual Encoding

- [ ] Lines are colored by project
- [ ] Priority stations show status color and priority border
- [ ] Context stations are ≤ track width
- [ ] Completed nodes are dimmed
- [ ] Layout has clear directionality (upstream → downstream)

### Interaction

- [ ] Click station opens detail panel
- [ ] Hover shows neighborhood flashlight
- [ ] Click empty space clears selection

### Data Integrity

- [ ] Every P0/P1 incomplete leaf appears as an endpoint
- [ ] Every node on a priority chain is reachable from at least one P0/P1 leaf
- [ ] Interchange detection correctly identifies shared nodes

---

## Scope Boundary

| Need | Handled by |
|---|---|
| Full network topology | Force view |
| Size/weight distribution | Treemap |
| Hierarchy quality | Circle Pack |
| Ranked next actions | Arc Diagram |
| Session context recovery | Dashboard |
