---
title: "View: Node Detail"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, node-detail]
created: 2026-03-07
---

# View: Node Detail

**Question answered:** "What is this thing and where does it fit?"

Part of the [[planning-web]] spec.

---

## Layout

Split-panel layout:

### Left Panel — Content

- Status, priority, assignee, dates (created, modified)
- **Description** (markdown body) — promoted to top, immediately below metadata
- Subtask checklist with status indicators
- Assumptions with status (untested/confirmed/invalidated)
- Activity timeline (git-based modification history)

### Right Panel — Context

- **Breadcrumb:** Goal > Project > Epic > This Task
- **Graph neighbourhood:** Direct parents, children, dependencies (blocks/blocked-by) with status badges
- **PKB connections:** Related notes surfaced by wikilink resolution, tag overlap, fuzzy title matching, citation backlinks
- **Downstream impact:** "Completing this unblocks N tasks across M projects"

---

## Actions Available from Detail

- Change status, priority
- Edit title, body (inline)
- Add/edit assumptions
- Add links (wikilinks, dependencies)
- Complete task
- Navigate to parent/child/dependency
