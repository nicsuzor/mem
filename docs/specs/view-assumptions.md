---
title: "View: Assumption Tracker"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, assumptions, effectual-planning]
created: 2026-03-07
---

# View: Assumption Tracker

**Question answered:** "What am I betting on, and how risky are those bets?"

Part of the [[planning-web]] spec.

---

## Design

- Assumption registry extracted from node frontmatter `assumptions:` field
- Status tracking: untested / confirmed / invalidated
- Sorted by downstream impact (how much work depends on this assumption?)
- Inline status editing
- Visual treatment: untested = yellow, confirmed = green, invalidated = red
- When invalidated, highlights all at-risk downstream tasks
