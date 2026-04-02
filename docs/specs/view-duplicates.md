---
title: "View: Duplicate Finder"
type: spec
status: draft
tier: ux
parent: planning-web
tags: [spec, planning-web, view, duplicates, graph-maintenance]
created: 2026-03-07
---

# View: Duplicate Finder

**Question answered:** "Am I tracking the same thing twice?"

Part of the [[planning-web]] spec.

---

## Design

- Shows duplicate clusters detected by title similarity and/or semantic embedding
- Each cluster: confidence score, canonical candidate, list of duplicates
- Actions: select canonical, merge others, archive
- Filters: scope by project, adjust similarity threshold
