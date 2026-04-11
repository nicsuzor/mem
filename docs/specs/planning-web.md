---
title: Planning Web — Svelte Web Application
type: spec
status: draft
tier: ux
tags: [spec, planning-web, svelte, dashboard, visualization, graph, adhd]
created: 2026-03-07
views: [view-focus, view-graph, view-node-detail, view-epic-tree, view-dashboard, view-assumptions, view-duplicates]
---

# Planning Web — Svelte Web Application

Umbrella spec for the Planning Web. Individual views have their own specs in this directory.

## Giving Effect

- Replaces: `aops-tui-spec-v0.1-a190d899` (ratatui TUI), Streamlit overwhelm dashboard
- Consumes: `aops graph` CLI output (`graph.json`, `knowledge-graph.json`), PKB MCP server, `synthesis.json`
- Related: `task-focus-scoring.md`, `theme-guide.md`, `batch-graph-operations.md`
- View specs: `view-focus.md`, `view-graph.md`, `view-node-detail.md`, `view-epic-tree.md`, `view-dashboard.md`, `view-assumptions.md`, `view-duplicates.md`

---

## Vision

Most task managers are flat lists with priorities bolted on. They treat tasks as independent atoms to be triaged. This is wrong for academic work, where everything connects: a peer review decision shapes a collaboration, which feeds a research paper, which advances a strategic goal, which rests on assumptions you haven't tested yet.

**The Planning Web makes the graph the interface.** Instead of staring at a list of 35 tasks wondering what to do next, you see the _shape_ of your work: what enables what, what blocks what, what assumptions are load-bearing, and which threads are converging on the same goal. The PKB (Personal Knowledge Base) provides the connective tissue — surfacing relevant knowledge, sources, and context alongside actionable work.

The organising metaphor is not a todo list. It's a **planning web** — a directed graph of goals, projects, and tasks connected by `enables` and `blocks` relationships, annotated with assumptions and uncertainty, and enriched by your knowledge base.

### Alignment with academicOps Vision

The Planning Web is the human-facing surface of the academicOps framework (see `VISION.md`). It provides:

- **Visibility into baseline capabilities** — the task graph, memory, and knowledge architecture become navigable and actionable through a visual interface
- **Context recovery for fragmented schedules** — reconstructs what was happening, what got dropped, and what needs attention, accommodating solo academic schedules and ADHD
- **Zero-friction capture** — ideas flow from the interface into the PKB without mode-switching
- **Nothing lost** — every task, assumption, and knowledge node is searchable and surfaced when relevant

### Design Principles

1. **Effectuation over causation.** The UI doesn't demand top-down planning. It supports bottom-up emergence: fragments arrive, get placed, and structure reveals itself.
2. **Progressive disclosure.** Default view shows 3-5 actionable items. Complexity is available but never forced.
3. **Graph-native.** Every view is a projection of the underlying graph.
4. **Knowledge-enriched.** Tasks don't exist in a vacuum. The interface surfaces related PKB nodes alongside work items.
5. **Uncertainty-aware.** The effectual lifecycle and assumption tracking are first-class UI citizens.
6. **ADHD-accommodating.** Zero-friction, clear boundaries, scannable not studyable, directive framing, collapsible density.

### User Expectations

1. **Immediacy and Orientation.** The user expects to answer "What should I do now?" and "Where did I leave off?" within 30 seconds of opening the application. The Focus View and Dashboard must provide immediate entry points without requiring manual data sorting.
2. **ADHD-Centric Design.** The interface must be scannable, not studyable. Visual hierarchy (size, colour, shape) must communicate priority and status before text is read. Dropped threads and active sessions must be prioritised to support context recovery.
3. **Graph Fidelity.** The Planning Web must be a high-fidelity projection of the underlying PKB. Changes made in the web interface (status, priority, relationships) must reflect in the markdown files near-instantly, and vice versa.
4. **Zero-Friction Capture.** The user expects to capture thoughts in under 5 seconds from any view. The Quick Capture mechanism must be omnipresent and require minimal metadata to succeed.
5. **Knowledge Integration.** Tasks should never appear in isolation. The user expects relevant knowledge (notes, citations, previous decisions) to be surfaced automatically based on the current task's context.
6. **Agency and Suggestion.** While the system provides focus scoring and agent-suggested actions, the user expects to maintain final authority over all task transitions and prioritisations. The UI should present "options for judgment," not "directives for compliance."

---

## Target Platform

**Svelte** web application (SvelteKit). Runs locally or on a private server. Communicates with the PKB backend via MCP or a thin REST/WebSocket API layer.

### Why Svelte

- Reactive by default — graph state changes propagate naturally
- Small bundle size and fast rendering for data-dense views
- Component model suits the view-based architecture
- SvelteKit provides routing, SSR, and API routes out of the box

### Architecture

```
+--------------------------------------------------+
|              Planning Web (Svelte)                |
|         SvelteKit + D3/LayerCake graphs           |
+--------------------------------------------------+
|              View / Component Layer               |
|  Focus | Graph | Epic Tree | Node Detail | Dash   |
+--------------------------------------------------+
|           State / Store Layer (Svelte stores)     |
|  Graph state, selections, filters, preferences    |
+--------------------------------------------------+
|              Data / API Layer                     |
|  PKB MCP client, graph.json loader, WebSocket     |
+--------------------------------------------------+
|              Backend Services                     |
|  PKB MCP server, aops CLI, synthesis pipeline     |
+--------------------------------------------------+
```

### Data Sources

| Source                 | Content                                                    | Update Frequency                                          |
| ---------------------- | ---------------------------------------------------------- | --------------------------------------------------------- |
| `graph.json`           | Task graph with hierarchy, dependencies, downstream_weight | Cron via `repo-sync-cron.sh` (`pkb graph`)                |
| `knowledge-graph.json` | PKB notes, wikilinks, tags                                 | Cron via `repo-sync-cron.sh` (`pkb graph`)                |
| PKB MCP server         | Task CRUD, search, graph metrics                           | Real-time via API                                         |
| `synthesis.json`       | Session synthesis (SSoT for dashboard session data)        | Cron via `repo-sync-cron.sh` + `synthesize_dashboard.py`  |
| `intentions.yaml`      | Active user intentions                                     | User-declared via `/intend`                               |
| Daily notes            | Today's accomplishments and context                        | File watch or poll                                        |

---

## Data Model

### Status Lifecycle

```
seed --> growing --> active --> blocked --> complete
                      |                      ^
                    dormant -----------------+
                      |
                     dead
```

### Focus Scoring

Tasks are ranked by a continuous focus score (see `task-focus-scoring.md`):

```
focus_score = (
    w_intention   * intention_alignment  +
    w_downstream  * downstream_signal    +
    w_priority    * priority_signal      +
    w_project     * project_activity     +
    w_recency     * recency_signal       +
    w_blocking    * blocking_urgency     +
    w_user        * user_boost
)
```

- **Hot** (score >= 0.3): shown in default views
- **Cold** (score < 0.3): searchable but hidden from focus views
- Scores are computed at query time, never stored
- The score breakdown is available on demand for transparency

### Node Types & Visual Hierarchy

| Type        | Visual Treatment              | Role                           |
| ----------- | ----------------------------- | ------------------------------ |
| Goal        | Largest, gold/amber accent    | Desired future states          |
| Project     | Large, blue accent            | Bounded efforts toward goals   |
| Epic        | Medium, distinct shape        | PR-sized verifiable work units |
| Task        | Standard size, white/neutral  | Single-session deliverables    |
| Action      | Small                         | Atomic steps within tasks      |
| Bug/Feature | Standard, distinct icon       | Typed work items               |
| Review      | Standard, review icon         | Review/approval work items     |
| PKB Note    | Green accent                  | Knowledge nodes                |
| Source      | Green with bibliographic info | Citation nodes                 |

### Edge Types

| Relationship      | Visual Treatment                   | Semantics                                |
| ----------------- | ---------------------------------- | ---------------------------------------- |
| Parent-child      | Solid, heavier weight              | Hierarchy (goal > project > epic > task) |
| `depends_on`      | Distinct colour (red/orange), bold | Hard blocking dependency                 |
| `soft_depends_on` | Dashed, lighter                    | Enabling/unlocking relationship          |
| Wikilink          | Dotted, subtle                     | Knowledge reference                      |

---

## Navigation & Interaction

### Global Navigation

Sidebar with page navigation:

- Focus (default landing)
- Graph
- Epic Tree
- Dashboard
- Assumptions
- Duplicates

### Global Controls

| Control           | Location                   | Purpose                                      |
| ----------------- | -------------------------- | -------------------------------------------- |
| Search            | Top bar                    | Unified fuzzy search across tasks + PKB      |
| Quick Capture     | Top bar or floating action | Zero-friction task creation                  |
| Theme toggle      | Settings                   | Light/dark mode                              |
| Time range filter | Sidebar                    | Filter completed tasks display (4H, 24H, 7D) |

### Keyboard Shortcuts

| Key             | Action                        | Context            |
| --------------- | ----------------------------- | ------------------ |
| `/`             | Open search                   | Global             |
| `n`             | Quick capture                 | Global             |
| `?`             | Help / shortcut reference     | Global             |
| `1-6`           | Navigate to view by number    | Global             |
| `j/k` or arrows | Navigate items                | Lists, tree        |
| `Enter`         | Open detail                   | Lists, tree, graph |
| `Escape`        | Close panel / clear selection | Global             |
| `Space`         | Toggle expand/collapse        | Tree view          |
| `f`             | Toggle reachable filter       | Graph view         |
| `s`             | Change status                 | Detail view        |
| `p`             | Change priority               | Detail view        |
| `a`             | Edit assumptions              | Detail view        |

Keyboard shortcuts should be discoverable — show a hint bar at the bottom of each view with context-relevant shortcuts.

---

## Task Management CRUD

Direct task manipulation from the interface:

| Operation | UI Element                                     | Backend                 |
| --------- | ---------------------------------------------- | ----------------------- |
| Create    | Quick capture (top bar) + full form in sidebar | PKB MCP `create_task`   |
| Read      | Node detail panel on click                     | PKB MCP `get_task`      |
| Update    | Inline edit on detail panel                    | PKB MCP `update_task`   |
| Delete    | Delete button with confirmation dialog         | PKB MCP `delete`        |
| Complete  | Single-click checkbox/button                   | PKB MCP `complete_task` |

### Inline Task Editor

Triggered by clicking edit on a task detail:

- Title (editable text)
- Status (dropdown)
- Priority (dropdown)
- Project (dropdown)
- Due date (date picker)
- Tags (tag input with autocomplete)
- Body (markdown editor with preview)
- Save / Cancel / Complete / Delete actions

**Principles:**

- Non-blocking: edits don't freeze the UI
- Optimistic updates: show changes immediately, sync in background
- Minimal clicks: common operations (complete, status change) in 1-2 clicks
- Context preservation: editing shouldn't lose graph position or scroll state

---

## Batch Operations

Multi-select mode available in Epic Tree View and Graph View:

- Select multiple tasks via shift-click, ctrl-click, or lasso (graph)
- Batch action bar appears when items selected:

```
N selected | Reparent | Archive | Update | Create Epic | Reclassify | Merge
```

### Available Batch Operations

| Operation   | Description                                       | Safety                  |
| ----------- | ------------------------------------------------- | ----------------------- |
| Reparent    | Move tasks to a new parent                        | Preview before apply    |
| Archive     | Set status to archived with optional reason       | Dry-run default         |
| Update      | Change priority, status, tags, or other fields    | Preview for >5 tasks    |
| Create Epic | Create new epic, reparent selected tasks under it | Confirm                 |
| Reclassify  | Change document type (task > epic, etc.)          | Preview                 |
| Merge       | Merge duplicates into canonical task              | Confirm, archive others |

All batch operations use the shared filter DSL from `batch-graph-operations.md` and support dry-run preview.

---

## PKB Integration

### How Knowledge Surfaces

1. **Explicit wikilinks** — `[[reference]]` resolution against both task and knowledge corpora
2. **Tag overlap** — shared YAML `tags:` values create associations
3. **Title similarity** — fuzzy matching surfaces related content
4. **Citation backlinks** — `type: source` notes tracked and surfaced in citation neighbourhood

### Semantic Queries

The interface supports natural-language-style queries through the PKB search:

- "What do I know about [topic]?" — PKB notes + tasks + goals
- "What are the dependencies?" — graph traversal
- "What's converging?" — multiple links to same concept/goal
- "What's orphaned?" — no goal-path tasks, unreferenced PKB notes

### Unified Search

Global search (triggered by `/` or search bar) searches across both task graph and PKB knowledge base. Results show:

- Icon for type (task, note, source, etc.)
- Title
- Status and priority (for tasks)
- Snippet of matching content
- Project/parent context

---

## Agent Integration

### MCP Server Interface

The planning web exposes graph queries to AI agents via MCP:

- Node lookup and traversal
- Search across graph and PKB
- Task CRUD operations
- Graph metrics (centrality, PageRank, downstream weight)

### Agent-Suggested Actions

- **Next action suggestions** — information-value ranking from graph structure
- **Probe suggestions** — "You're assuming X. A cheap test might be: [action]"
- **Synergy detection** — multiple tasks/projects linking to same concept surfaced as cross-project opportunities
- **Structure proposals** — agent can propose graph amendments (reparenting, new epics, dependency additions)

---

## ADHD Design Principles

These constrain all design decisions:

### Scannable, Not Studyable

The interface must communicate at a glance. Structure, colour, size, and spatial grouping carry meaning before any text is read. One-line items, coloured indicators, no paragraph-level reading required in overview modes.

### Dropped Threads First

The most actionable information for context recovery. Things started but not finished appear prominently. Directive framing: "YOUR PATH" not "Session History"; "NEEDS YOU" not "Status: waiting".

### No Flat Displays at Scale

500 same-sized items is noise, not information. Visual hierarchy (size, shape, emphasis) creates entry points. Bucket, group, and summarise — never dump hundreds of items in a list.

### Support Focus Transitions

The hardest moment is shifting from "seeing everything" to "working on one thing." The graph supports both modes: full overview for orientation, single-project filter for commitment.

### Collapsible Density

Important information above the fold. Detail available on demand. Sections collapse. Dense data (project grids, graph settings) is below or collapsed by default.

### Reactive, Not Demanding

Reconstructs context from existing data. No pre-planning required from the user. The system adapts to how you work, not the other way around.

### The Litmus Test

If a user sees a display element and can't answer "what does this mean?" within 3 seconds, the design has failed.

---

## Session Context Model

A session is a **conversation thread**, not an agent process. The user recognises sessions by what they asked, not by agent IDs.

### What Makes a Session Identifiable

1. **Initial prompt** — what the user first asked
2. **Follow-up prompts** — subsequent requests that shaped the work
3. **Working directory/project** — secondary context

### Session Display (Good)

```
[project icon] academicOps (2h ago)
   Started: "Review PR #42 for aops CLI changes"
   Now: Fixing 3 linting errors
   Next: Run tests, mark PR ready
```

### Session Triage

| Bucket     | Definition                | Display                                      |
| ---------- | ------------------------- | -------------------------------------------- |
| Active Now | Activity within 4 hours   | Full session cards with conversation context |
| Paused     | 4-24 hours since activity | Collapsed cards, click to expand             |
| Stale      | >24 hours since activity  | Archive prompt with batch actions            |

### Minimum Viable Context

A session must have initial prompt OR current task status. Sessions without meaningful context are hidden. "unknown: No specific task" is never displayed.

---

## Visual Design

See `theme-guide.md` for Operator and Holographic theme details.

### Typography

| Context                | Font                               | Notes                           |
| ---------------------- | ---------------------------------- | ------------------------------- |
| Body text              | Space Grotesk or Host Grotesk      | Distinctive sans-serif          |
| Code/IDs               | BlexMono Nerd Font / IBM Plex Mono | Monospace for technical content |
| Accessibility fallback | Atkinson Hyperlegible              | When legibility is paramount    |

**Never use:** Inter, Roboto, Arial, system-ui, Helvetica (generic AI defaults).

### Icons

**Bootstrap Icons exclusively.** No emoji as interface icons. Consistent visual language across all views.

### Colour Palette

Muted academic palette with sharp accents. Must work in both light and dark themes.

**Status colours:**

| Status    | Colour        | Hex     |
| --------- | ------------- | ------- |
| Active    | Blue          | #3b82f6 |
| Done      | Green         | #22c55e |
| Blocked   | Red           | #ef4444 |
| Waiting   | Yellow        | #eab308 |
| Review    | Purple        | #a855f7 |
| Cancelled | Grey          | #94a3b8 |
| Seed      | Dim italic    | —       |
| Growing   | Yellow        | —       |
| Dormant   | Dim blue      | —       |
| Dead      | Strikethrough | —       |

**Priority colours:**

| Priority        | Treatment       |
| --------------- | --------------- |
| P0 (critical)   | Bold red badge  |
| P1 (intended)   | Amber badge     |
| P2 (active)     | Default/neutral |
| P3 (planned)    | Dim grey        |
| P4 (backlog)    | Near-invisible  |

**Node type accents:**

| Type     | Accent                        |
| -------- | ----------------------------- |
| Goal     | Gold/amber                    |
| Project  | Blue                          |
| Task     | White/neutral                 |
| PKB Note | Green                         |
| Source   | Green + bibliographic styling |

### Responsive Design

- Minimum viewport: 1024px wide (desktop-first; this is a planning tool)
- Sidebar collapses on narrower viewports
- Graph view takes full available width
- Project grid uses CSS grid with `minmax(350px, 1fr)`
- Detail panels use slide-over or modal on smaller screens

---

## Non-Goals

- External task manager sync (Jira, Asana, etc.)
- Multi-user collaborative editing (single-user system)
- Mobile-first design (desktop planning tool; responsive but not mobile-optimised)
- Calendar integration
- AI-generated content in the interface (agents suggest, humans decide)

---

## Open Questions

1. **Real-time vs polling.** Should the web app use WebSocket for live graph updates, or poll on an interval? WebSocket is better UX but adds server complexity.
2. **Offline support.** Should the app work offline with a cached graph.json? Useful for travel but adds sync complexity.
3. **Graph rendering library.** D3.js force-graph (current), Cytoscape.js, or Svelte-native (LayerCake + custom)? Need to evaluate performance at 500+ nodes.
4. **Authentication.** Local-only (no auth needed) vs. private server deployment (needs auth). Start local-only.
5. **PKB write-back latency.** How fast do task edits need to propagate to the markdown files? Optimistic UI with background sync seems right.
6. **Focus score computation.** Server-side (PKB MCP) or client-side from graph.json data? Server-side is simpler; client-side is more responsive.
7. **Assumption adoption.** Will tracking assumptions change planning behaviour, or is it too much friction? (Core bet — same as original TUI spec.)

## References

- Sarasvathy (2001). Causation and Effectuation.
- McGrath & MacMillan (1995). Discovery-Driven Planning.
- Snowden & Boone (2007). A Leader's Framework for Decision Making.
- academicOps VISION.md — Framework vision and success criteria
- academicOps STYLE.md — Visual design and typography standards
- `theme-guide.md` — Operator and Holographic theme design details
