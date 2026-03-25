---
title: "View: Focus (Default Landing)"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, focus, adhd]
created: 2026-03-07
---

# View: Focus (Default Landing)

**Question answered:** "What should I do right now?"

Part of the [[planning-web]] spec. See also: `task-focus-scoring.md` for how items are ranked.

---

## Design

Shows top 5 actionable items ranked by the `task-focus-scoring.md` formula (weighted multi-signal: intention alignment, downstream weight, priority, project activity, recency, blocking urgency, user boost). See [[task-focus-scoring]] for signal definitions and weights. Capped at 5.

### Sections

- **Top picks** — 5 task cards with title, project, priority badge, staleness indicator, and a one-line description. Each card links to Node Detail.
- **Untested assumptions** — Load-bearing assumptions grouped by downstream impact. Visual treatment: untested = yellow warning, confirmed = green check, invalidated = red cross.
- **Blocked summary** — Count of blocked tasks, grouped by blocker. "3 tasks waiting on [blocker title]" with link to unblock.

### Empty State

Below the 5 picks, show recent completions and a "what got done" summary to provide momentum feedback.

---

## Data Sources

- `graph.json` — task hierarchy and downstream_weight for ranking
- PKB MCP server — task details, status, priority
- `task-focus-scoring.md` formula — determines which 5 tasks surface
- `intentions.yaml` — intention_alignment is the dominant scoring signal

---

## ADHD Principles Applied

- **Cap at 5** — more than 5 choices creates paralysis, not orientation
- **Progressive disclosure** — assumptions and blocked summary are secondary to the picks
- **Momentum feedback** — recent completions provide positive reinforcement
