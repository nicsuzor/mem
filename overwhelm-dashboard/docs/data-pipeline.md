# Dashboard Data Pipeline

> Updated 2026-04-07. Reflects decisions from forensic QA audit session.

## Design Principles

1. **No fallbacks.** Each panel has exactly one data source. If it's missing, show an error — never fabricate data from a different source.
2. **Fail fast and loud.** Pipeline errors render as inline badges in the triage bar, not silent empty states.
3. **Only show what you can explain.** If a field's meaning isn't clear to a user scanning in 5 seconds, don't show it.

## Pipeline Flow

```
1. PER-SESSION (on session end)
   /dump skill → {hash}.summary.json → $AOPS_SESSIONS/summaries/

2. PERIODIC AGGREGATION (two writers to the same file)
   a) repo-sync-cron.sh (cron, every 5 min)
      → synthesize_dashboard.py (pure mechanical aggregation, no LLM)
      → $AOPS_SESSIONS/synthesis.json

   b) /daily skill (manual, typically once per day)
      Step 4.7: structural data (sessions, accomplishments, alignment, blockers)
      Step 5.3: daily_story + daily_narrative (LLM-generated)
      → $AOPS_SESSIONS/synthesis.json (read-merge-write)

3. CLIENT-SIDE
   /api/graph → PKB MCP graph_json → project dashboard, graph views
```

## Data Sources by Panel

| Panel              | Source                                         | Status       |
| ------------------ | ---------------------------------------------- | ------------ |
| Current Activity   | `$AOPS_SESSIONS/summaries/*.json` (today, no outcome, <4h) | **Working** |
| Triage Bar         | Derived from Current Activity + pipeline errors | Working |
| Today's Story      | `synthesis.json` → `daily_story` only          | Working (requires /daily) |
| Dropped Threads    | `summaries/*.json` → friction_points           | Working |
| Recent Activity    | `summaries/*.json` → accomplishments by project | Working |
| Project Dashboard  | `/api/graph` (PKB MCP) — only priority epics   | Working |
| Quick Capture      | Client-side POST to `/api/tasks/create`        | Working |

## Current Activity

**Purpose**: Context recovery after interruption. A user comes back from a meeting and needs to quickly see: what was I doing, on which machine, on which project, in which session?

### What counts as "current"

A session is "current" if ALL of:
1. It has a summary file in `$AOPS_SESSIONS/summaries/` with today's date prefix
2. It does NOT have an `outcome` field (i.e. it hasn't cleanly finished via /dump)
3. Its file was written to in the last 4 hours

### Session types

Classified from filename conventions:
- **polecat** — autonomous background agent. Display briefly: project + task description.
- **crew** — multi-agent orchestrated session. Show crew goal and progress.
- **scheduled** — cron-triggered background task.
- **interactive** — user-driven session. Most context-rich: project, what user was doing, recency.

### Status badges

- `RUNNING` — modified <10 minutes ago
- `IDLE` — modified 10-60 minutes ago
- `PAUSED` — modified 1-4 hours ago

## Today's Story

**Source**: `synthesis.json` → `daily_story` field only. No fallbacks.

`daily_story` is a 3-5 item array of second-person bullet points written by `/daily` Step 5.3.1. If `/daily` hasn't run, the panel shows "No narrative available. Run /daily to generate today's story."

The dashboard does NOT use:
- `daily_narrative` (prose version — not structured for the grouped display)
- `narrative` (mechanical `[project] summary` array from cron — not a "story")
- `alignment`, `waiting_on`, `context` (mechanical metadata — not actionable at a glance)

## Project Dashboard

**Source**: `/api/graph` (PKB MCP graph_json).

**Filtering**:
- Only shows epics with at least one outstanding P0 or P1 task
- Only shows projects that have at least one qualifying epic
- Projects sorted by number of priority epics, then by active task count
- Sub-projects roll up into major projects via `projectUtils.ts`

**Per project displays**:
- Priority epic progress bars (completed/total)
- Top 3 active tasks sorted by priority
- Recently completed items

## Upstream: Per-Session Summaries

Written by `/dump` skill at session end. One file per session in `$AOPS_SESSIONS/summaries/`.

Filename format: `YYYYMMDD-HH-project-sessionid-description.json`

Key fields:
- `session_id`, `date`, `project`
- `summary` — agent-generated session summary (null until /dump runs)
- `outcome` — completion status (null = still running or abandoned)
- `accomplishments` — list of completed items
- `user_prompt_count` — primary attention signal (0, 1, 2-3, 4+)
- `timeline_events` — detailed event log
- `friction_points` — issues encountered
- `token_metrics.efficiency.session_duration_minutes`

## Upstream: synthesis.json

**Two writers** to the same file — they coordinate via read-merge-write:

### synthesize_dashboard.py (cron, every 5 min)

Pure mechanical aggregation. No LLM. Reads all session summaries and produces:
- Session counts and project distribution
- Aggregated accomplishments (deduplicated)
- Alignment status (success rate)
- Atomic writes via `tempfile.mkstemp()` + `os.replace()`

**Known bug**: Aggregates all-time sessions without date filter. `sessions.total` = all-time count, not daily. See task-0b5fc188.

### /daily skill (manual, ~1x per day)

LLM-assisted synthesis. Writes in two phases:

**Step 4.7 (progress sync)** — structural data:
- `sessions.total`, `sessions.by_project`, `sessions.recent[]`
- `accomplishments`, `merged_prs`
- `alignment`, `waiting_on`, `skill_insights`
- `session_timeline[]`
- Does NOT write narrative fields

**Step 5.3.1 (work summary)** — narrative:
- `daily_story` — 3-5 bullet points, second person (**only field used by dashboard**)
- `daily_narrative` — prose version (not used by dashboard)
- `narrative_generated` — timestamp

### synthesis.json schema

```json
{
  "generated": "ISO timestamp",
  "date": "YYYYMMDD",
  "daily_story": ["You started...", "Got pulled into...", "..."],
  "daily_narrative": "...",
  "narrative_generated": "ISO timestamp",
  "sessions": {
    "total": "N (BUG: all-time, not daily)",
    "by_project": {"aops": 2, "writing": 1},
    "recent": [{ "session_id", "project", "summary", "engagement", "work_type" }]
  },
  "accomplishments": { "count": "N", "items": [{"project", "item"}] },
  "alignment": { "status": "on_track|blocked|drifted", "note": "..." },
  "_age_minutes": "N  // added by dashboard server at read time"
}
```

## Dashboard Server Functions (`+page.server.ts`)

- `loadSynthesis()` — reads `$AOPS_SESSIONS/synthesis.json`, adds `_age_minutes`
- `findActiveSessions(hours)` — scans `$AOPS_SESSIONS/summaries/` for today's files without outcome, modified within `hours`
- `loadRecentSummaries(days)` — reads `$AOPS_SESSIONS/summaries/*.json` for last N days
- `buildPathData(summaries)` — aggregates accomplishments + friction into activity feed and dropped threads

## Pipeline Error Reporting

Errors are shown as inline red badges in the triage bar (same row as "N running", "nothing needs you"):
- `$AOPS_SESSIONS not set` — session discovery disabled entirely
- `synthesis.json not found` — cron pipeline broken or env misconfigured
- `no daily_story` — /daily hasn't run today
