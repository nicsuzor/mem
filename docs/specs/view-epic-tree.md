---
title: "View: Epic Tree"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, epic-tree, hierarchy]
created: 2026-03-07
---

# View: Epic Tree

**Question answered:** "What's the structure of my work?"

Part of the [[planning-web]] spec.

---

## Design

Hierarchical tree of goal > project > epic > task with:

- Task counts per node (done / total)
- Progress bars for epics
- Priority badges (P0 red, P1 orange, P2 default, P3 dim)
- Staleness badges (yellow >14d, red >30d)
- Type icons (Bootstrap Icons per node type)
- Expand/collapse with progressive depth
- Status colour-coding on each row

---

## Interactions

- Click to expand/collapse subtrees
- Click task to open Node Detail
- Multi-select mode for batch operations (see Batch Operations in planning-web.md)

## Sorting

Within each level, sort by priority then by focus score.
