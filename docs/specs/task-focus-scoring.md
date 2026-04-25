---
title: Task Focus Scoring
type: spec
status: active
tier: ux
depends_on: []
tags: [spec, ux, scoring, dashboard]
---

# Task Focus Scoring

How tasks are ranked in the ready queue and focus views.

## Problem

A task system serving an ADHD brain will always have more tasks than capacity. This is a permanent condition, not a backlog to be cleared. The system must surface what matters most without requiring manual triage.

## Goals and targets

`goal` and `target` are the same concept: user-declared strategic priorities that tasks contribute toward. Both `type: goal` and `type: target` are recognized node types; `target` is an accepted alias that resolves to `goal` in the linter's auto-fix mode. The `goals: [<id>]` field on a task links it to any goal or target node by ID. The `consequence` field on a goal/target feeds into deadline urgency scoring (see below).

## Focus score (implemented)

Each task's focus score is an integer point accumulator computed at graph-build time and stored in `node.focus_score`. Higher scores surface first. The formula is:

```
focus_score =
    priority_base
  + deadline_score (× consequence multiplier if set)
  + age_staleness_bonus (P2+ only)
  + downstream_weight × 10
  + stakeholder_waiting_bonus
```

### Priority base

| Priority | Base score |
|----------|-----------|
| 0 (critical) | 10 000 |
| 1 (high) | 5 000 |
| 2+ or unset | 0 |

### Deadline urgency

Computed only when a parseable `due: YYYY-MM-DD` is present. `effort_days` comes from the `effort` field (defaults to 3 if absent).

| Condition | Deadline score |
|-----------|---------------|
| Overdue (`days_until < 0`) | 8 000 + min(days_overdue × 200, 4 000) |
| Tight: `effort_days ≥ days_until` | 6 000 |
| Near-tight: `effort_days > 0.5 × days_until` | linear interp 2 000 → 6 000 |
| Due within 30 days (not tight) | 1 000 |
| Due > 30 days out | 0 |

**Consequence multiplier**: if `consequence` is set on the task (any non-null value), `deadline_score × 1.5`.

### Age/staleness bonus (P2+ only)

For tasks with priority ≥ 2 (or unset), adds `min(days_since_created, 200)`. Prevents old low-priority tasks from being buried forever.

### Downstream weight

`downstream_weight × 10` — the downstream weight (sum of transitive dependent counts, computed during graph build) directly boosts tasks that unblock others.

### Stakeholder waiting bonus

When `stakeholder` is set, adds `2 000 + min(days_waiting × 200, 6 000)`. Anchor date is `waiting_since` if set, falling back to `created`. Maximum bonus: 8 000.

## Unimplemented signals (FUTURE / DEFERRED)

The following signals were designed but not yet built. They are preserved here for future implementation.

| Signal | Description | Prerequisite |
|--------|-------------|-------------|
| `intention_alignment` | Whether task is in scope of an active intention. Requires `intentions.yaml` state file. Was specced in [[intentions.md]] and the infrastructure (intentions.yaml) was built then removed. | Rebuild intention state infrastructure |
| `project_activity` | How active is this task's project in recent N days. Requires per-project modified-date aggregation. | Project activity index |
| `recency_signal` | Exponential decay on `modified` timestamp. Was specced as `exp(-days/30)`. | — |
| `blocking_urgency` | 1.0 if blocking an `in_progress` task, 0.5 if blocking an active task. The field `blocking_urgency` is computed on each node but not currently included in the score accumulator. | Wire into accumulator |
| `user_boost` | Explicit `focus: boost` in frontmatter or daily note mentions with 7-day decay. | Boost field + decay logic |

> **Note on the old spec**: the previous version of this document described a normalized 0.0–1.0 weighted float formula (`focus_score = w1*s1 + w2*s2 + ...`) with configurable weights summing to 1.0. That formula was never implemented. The implementation is an integer point accumulator. This document now describes reality.

## Computation timing

Scores are computed during graph build (after all edge/metric passes) and stored in `GraphNode.focus_score`. They are recomputed on every full rebuild (~300ms). No TTL cache needed at current scale.

## Usage in the system

- `focus_picks(max)` — returns top-N ready tasks by focus_score descending
- `list_tasks` with `sort_by: focus_score` — ranks ready queue by score
- TUI Focus view — displays tasks ordered by focus_score

## Design principles

1. **Transparent**: the score formula is documented and the integer value is visible in task metadata.
2. **Automatic**: scores recompute on rebuild without manual tagging or triage.
3. **Reversible**: nothing is deleted or hidden by scoring — it only affects ordering.
4. **ADHD-aware**: overflow is the normal state. The scoring manages it structurally, not by demanding the user clear a backlog.
