---
title: "View: Dashboard"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, dashboard, sessions, adhd, context-recovery]
created: 2026-01-21
modified: 2026-03-24
---

# View: Dashboard

**Question answered:** "What's the health of my work and where's my attention going?"

Part of the [[planning-web]] spec. This is the strategic overview, ordered by ADHD priority — most actionable information first.

---

## The User

Nic is an academic with ADHD who runs parallel workstreams across multiple machines, terminals, and projects. His working memory is limited but his ambition isn't — at any given time there are 500+ tasks across research, tooling, governance, and teaching, with dozens of agent sessions running in parallel.

The dashboard exists because Nic's brain can't hold all this state. He needs an external system that reconstructs context for him: what was happening, what he intended, what got dropped, and what needs attention now. Off-the-shelf dashboards fail because they're designed for people who remember what they were doing — Nic often doesn't.

---

## User Stories

### US-D1: I can tell what's happening right now

**As** Nic returning to my desk after a break (or a context switch, or a meeting, or just losing track),
**I want** to see at a glance what's actively running and what state it's in,
**So that** I don't accidentally duplicate work or interrupt an agent mid-task.

**Acceptance test**: Nic opens the dashboard and within 5 seconds can count how many agents are actively running and whether any need his attention.

### US-D2: I can recover what I was doing before I got distracted

**As** Nic who just realized I've been down a rabbit hole for 2 hours and have lost track of my original plan,
**I want** to see what I started today, what I intended, and what got dropped,
**So that** I can pick up the threads I care about rather than continuing to drift.

**Acceptance test**: Nic can identify at least one dropped thread from today within 10 seconds of opening the dashboard.

### US-D3: I can see today's story, not just today's data

**As** Nic who runs 10+ parallel sessions and can't hold the narrative in my head,
**I want** a brief human-readable summary that tells today's story — what started, what got sidetracked, what's still hanging,
**So that** I can orient myself in 15 seconds without reading through session logs.

**Acceptance test**: Nic reads the synthesis narrative and it matches his lived experience of the day. If it's stale (>60 min), the dashboard flags it.

### US-D4: I can see what each project needs from me

**As** Nic who works across 6+ projects simultaneously,
**I want** per-project cards that show: what's actively being worked on, what's next in the queue, and what was recently accomplished,
**So that** I can quickly check in on any project without opening terminals or task files.

**Acceptance test**: Nic can answer "what's the status of project X?" for any active project within 5 seconds by finding its card.

### US-D5: I can see what I was working on yesterday and earlier this week

**As** Nic who lost a whole day to meetings and needs to pick up where I left off,
**I want** paused sessions (4-24h old) to be visible but de-emphasized, with enough context to resume them,
**So that** I can decide which threads to pick back up without hunting through terminal history.

**Acceptance test**: Nic can find a session from yesterday and understand what it was doing without expanding more than one click.

### US-D6: I can capture a thought without losing my place

**As** Nic who just had an idea while looking at the dashboard,
**I want** to quickly capture a task or note without navigating away,
**So that** the idea doesn't evaporate while I context-switch to a different tool.

**Acceptance test**: Nic can go from "I just thought of something" to "it's captured" in under 5 seconds.

### US-D7: The dashboard doesn't overwhelm me

**As** Nic whose whole problem is overwhelm,
**I want** the dashboard itself to not be another source of cognitive overload,
**So that** opening it calms me down rather than stressing me out more.

**Acceptance test**: The dashboard's above-the-fold content (before scrolling) answers the three critical questions: "What's running?", "What's dropped?", and "What needs me?"

---

## Sections

### 1. Current Activity

Agent count and active session indicator. "N agents running, M need your attention."

### 2. Where You Left Off + Focus Synthesis

**Data source:** `synthesis.json` is the **single source of truth** for all session data displayed in the dashboard. The dashboard does NOT read session state files or transcripts directly — all session context is pre-extracted by `synthesize_dashboard.py` (run on cron via `repo-sync-cron.sh`).

**What synthesis.json must contain per session:**

- `user_prompts` — all user prompts from covered sessions (already extracted by transcript pipeline as `[timestamp, role, text]` tuples; **TODO: synthesize_dashboard.py must pass these through to synthesis.json** — currently omitted)
- `session_id`, `project`, `summary`, `outcome`, `accomplishments`, `duration_minutes`
- Aggregated narrative, alignment, blockers, skill insights

**Session cards** for context recovery:

- **Active sessions** (<4h): Rich cards with project name, timestamp, initial prompt (what you asked), progress bar, current step, next step, status badge (Running / Needs You)
- **Paused sessions** (4-24h): Collapsed cards with outcome, accomplishment summary, reentry point. Subdued styling.
- **Stale sessions** (>24h): Archive prompt with count. "N stale sessions — [Archive All] [Review] [Dismiss]"

**Focus synthesis** (aggregated from same `synthesis.json`):

- 3-5 bullet narrative summary of today's story
- Status cards: accomplishments, alignment, blockers
- Session insights: skill compliance, corrections, context gaps
- Staleness indicator (STALE badge if >60 minutes)
- Graceful fallback when missing: message + regeneration hint

**Critical rule:** Sessions must show the user's initial prompt, not agent-generated descriptions. If a session can't answer "what was I doing?", it's filtered out.

### 3. Your Path (Dropped Threads)

Reconstructed user path across sessions:

- **Dropped threads first** (most actionable for ADHD context recovery): tasks created/claimed but not completed, grouped by project with coloured borders
- **Timeline threads:** Horizontal-scroll cards per project with initial goal, git branch, and coloured-dot event timeline (prompts, task creates, completions, updates)

### 4. Active Intentions

**Replaces:** former "Spotlight Epic" section (which guessed the most active epic).

**Data source:** `intentions.yaml` (see academicOps `specs/intentions.md`).

Shows the user's **declared intentions** — what they've explicitly said they intend to accomplish. Each intention card shows:

- Intention statement and linked PKB node (goal/project/epic)
- Progress bar based on downstream task completion
- Done/in-progress/blocked card grid for the intention's subgraph
- Clickable — drills into the intention's task tree

When no intentions are active, falls back to a prompt: "What do you intend to work on?" with quick-declare affordance. Does NOT guess or auto-select.

### 5. Project Grid

Responsive grid of project cards (CSS grid, min 350px per card):

| Section   | Content                                                   |
| --------- | --------------------------------------------------------- |
| Header    | Project name, colour-coded border                         |
| Epics     | Active epic titles + progress bars (max 3)                |
| Completed | Recently completed tasks with time_ago (max 3 + "X more") |
| Up Next   | Top 3 priority tasks with priority AND status badges      |
| Recent    | Accomplishments from daily notes (max 3)                  |

Sorted by activity score. Empty projects hidden. Sub-projects roll up.

### 6. Quick Capture

Text input + optional tags + submit. Creates a task in PKB inbox. Minimal friction: no project/priority required, just a title.

**Design target:** Idea to captured in under 5 seconds.

---

## Session Context Model

A session is a **conversation thread**, not an agent process. The user recognizes sessions by what they asked, not by agent IDs.

### Session Context Schema

Each displayed session MUST include:

```json
{
  "session_id": "abc123",
  "project": "academicOps",
  "initial_prompt": "Review PR #42 for the aops CLI changes",
  "follow_up_prompts": [
    "Also check the test coverage",
    "Fix the linting errors you found"
  ],
  "last_user_message": "Fix the linting errors you found",
  "current_status": "Fixing 3 linting errors in src/indexer.rs",
  "planned_next_step": "Run tests after fixes, then mark PR ready for review",
  "last_activity": "2026-02-03T10:30:00Z",
  "started": "2026-02-03T09:15:00Z"
}
```

### Session Triage

| Bucket         | Definition                | Display                                      |
| -------------- | ------------------------- | -------------------------------------------- |
| **Active Now** | Activity within 4 hours   | Full session cards with conversation context |
| **Paused**     | 4-24 hours since activity | Collapsed cards, click to expand             |
| **Stale**      | >24 hours since activity  | Archive prompt (see below)                   |

### Stale Session Handling

Sessions >24h without activity are **not displayed in the main list**. Instead:

```
+-----------------------------------------------------+
| 12 stale sessions (no activity >24h)                 |
|                                                     |
| [Archive All]  [Review & Select]  [Dismiss]         |
+-----------------------------------------------------+
```

---

## Design Principles

### ADHD Accommodation

- **Dropped threads first** — most actionable information for context recovery
- **Scannable, not studyable** — one-line items, colored indicators, no paragraph-level reading required
- **Reactive design** — reconstructs from existing data; no pre-planning required from user
- **Directive framing** — "YOUR PATH" not "Session History"; "NEEDS YOU" not "Status: waiting"
- **Collapsible density** — important information above the fold; detail available on demand
- **No flat displays at scale** — bucket, group, and summarize; never dump 499 items in a list

### Context Recovery, Not Decision Support

The dashboard answers:

- **What's running where?** — Multiple terminals, multiple projects
- **Where did I leave off?** — Per-project context recovery
- **What's the state of X?** — Quick status check

It does NOT try to: recommend ONE thing to do, hide options, force single-focus, or make decisions for the user.

### Anti-Patterns

**Display anti-patterns:**

- **Agent-centric display**: Showing "499 agents running" instead of meaningful session context
- **Unknown/empty sessions**: Displaying "unknown: No specific task" provides zero value
- **Flat lists at scale**: 499 items in a list creates paralysis, not orientation
- **Ignoring recency**: Treating 275h-old sessions the same as 2h-old sessions
- **Truncation that destroys meaning**: Cutting prompts to 60 chars makes them useless

**The litmus test**: If a user sees a session entry and can't answer "what was I doing there?", the display has failed.

---

## Acceptance Criteria

### Session Display (Critical)

- [ ] Each session shows initial prompt (what user asked)
- [ ] Each session shows current status or planned next step
- [ ] Sessions without meaningful context are hidden
- [ ] Truncation preserves enough context to identify session (>=120 chars minimum)
- [ ] Recency triage applied: Active (<4h), Paused (4-24h), Stale (>24h)
- [ ] Stale sessions trigger auto-archive prompt, not flat list display
- [ ] User can answer "what was I doing?" for every displayed session

### Session Triage

- [ ] Active sessions (last 4h) shown with full conversation context
- [ ] Paused sessions (4-24h) shown collapsed, expandable
- [ ] Stale sessions (>24h) show archive prompt with count
- [ ] Archive action moves session to archive, removes from display

### Path Reconstruction

- [ ] Dropped threads shown first, grouped by project
- [ ] Timeline events scannable (one line each with colored dots)
- [ ] Path section visible even when synthesis.json is missing

### Synthesis

- [ ] Narrative panel shows 3-5 bullet summary of today's story
- [ ] Staleness clearly indicated when >60 minutes
- [ ] Graceful fallback when synthesis.json missing (message + regeneration hint)

### Project Boxes

- [ ] Projects sorted by activity (active agents first)
- [ ] Each project shows active work, next priorities, and recent completions
- [ ] UP NEXT shows task status, not just priority
- [ ] Empty projects hidden

### Quick Capture

- [ ] Create task from dashboard UI
- [ ] Idea to captured in under 5 seconds
