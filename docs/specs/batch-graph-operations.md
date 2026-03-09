# Spec: Batch Task Graph Operations

**Status:** Draft (revised)
**Date:** 2026-03-04 (revised 2026-03-09)
**Author:** nic (via Claude)
**Repo:** nicsuzor/mem
**Surfaces:** CLI (`aops`), MCP (`pkb`), TUI

---

## Problem Statement

The PKB currently has 475 ready tasks. Many are duplicates, misclassified, stale, or structurally orphaned. The existing tooling can only manipulate tasks one at a time. Reorganizing a large task graph requires batch operations that don't exist yet.

Specifically, we cannot currently:

- Move multiple tasks to a new parent in one operation
- Archive or reclassify multiple tasks at once
- Detect and merge duplicate tasks
- Perform filtered bulk updates (e.g., "archive all P3 tasks older than 90 days in project X")
- Preview the effect of a batch operation before committing it

The `decompose_task` tool proves the pattern: batch creates with a single graph rebuild at the end. We need the same pattern for updates, reparents, archives, and merges.

---

## Design Principles

1. **Single graph rebuild per batch.** The current pattern of full `GraphStore::build_from_directory()` after each mutation is O(n). Batch operations must defer the rebuild to the end.

2. **Dry-run by default for destructive operations.** Any operation that changes more than 5 tasks should support `--dry-run` (CLI) / `dry_run: true` (MCP) that returns what *would* change without writing to disk.

3. **Feature parity across surfaces.** Every batch operation should be available as a CLI subcommand, an MCP tool, and a TUI action. The core logic lives in a shared `batch_ops` module; surfaces are thin wrappers.

4. **Composable filters.** Batch operations select targets via a shared filter DSL, not ad-hoc arguments. The same filter syntax works everywhere.

5. **Idempotent.** Running the same batch operation twice produces the same result. Operations that are already satisfied (task already archived, already has correct parent) are no-ops for that task.

6. **Atomic per-file, best-effort per-batch.** Each file write is atomic (write to temp, rename). If one task in a batch fails, the others still succeed. Failures are reported in the response.

---

## Shared Filter DSL

All batch operations accept a target set defined by filters. Filters are composable (AND logic; multiple values within a filter are OR logic).

### Filter Fields

| Filter | CLI Flag | MCP Param | Description |
|--------|----------|-----------|-------------|
| IDs | `--ids a,b,c` | `ids: ["a","b","c"]` | Explicit task IDs (flexible resolution) |
| Project | `--project aops` | `project: "aops"` | Match project field |
| Parent | `--parent epic-xyz` | `parent: "epic-xyz"` | Direct children of parent |
| Subtree | `--subtree epic-xyz` | `subtree: "epic-xyz"` | All descendants (recursive) |
| Status | `--status active` | `status: "active"` | Match status |
| Priority | `--priority 3` | `priority: 3` | Exact priority match |
| Priority range | `--priority-gte 2` | `priority_gte: 2` | Priority >= N |
| Tags | `--tags stale,blocked-human` | `tags: ["stale","blocked-human"]` | Has ALL listed tags |
| Type | `--type task` | `type: "task"` | Match document type |
| Age | `--older-than 90d` | `older_than_days: 90` | Days since `created` |
| Stale | `--stale 60d` | `stale_days: 60` | Days since `modified` |
| Orphan | `--orphan` | `orphan: true` | No parent and no project |
| Text | `--title-contains "PhD inquiry"` | `title_contains: "PhD inquiry"` | Substring match on title |
| Complexity | `--complexity mechanical` | `complexity: "mechanical"` | Match complexity tag |
| Directory | `--directory tasks/inbox` | `directory: "tasks/inbox"` | File path contains directory (useful for targeting inbox vs project dirs) |
| Weight | `--weight-gte 5` | `weight_gte: 5` | Downstream weight >= N (structurally important nodes) |

### CLI Composition

```bash
# Archive all P3+ tasks in 'framework' project older than 60 days
aops batch archive --project framework --priority-gte 3 --older-than 60d

# Reparent all tasks matching title pattern under a new epic
aops batch reparent --title-contains "Write spec for" --new-parent epic-skill-specs
```

### MCP Composition

```json
{
  "project": "framework",
  "priority_gte": 3,
  "older_than_days": 60
}
```

### Filter Resolution Pipeline

1. If `ids` is provided, start with those exact tasks
2. Otherwise, start with all indexed tasks
3. Apply each filter as a predicate (AND composition)
4. Return matching set with count

---

## Batch Operations

### 1. `batch_reparent`

Move multiple tasks to a new parent. The most critical operation for graph restructuring.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| filters | FilterSet | Yes | Target selection (see Filter DSL) |
| new_parent | string | Yes | ID of new parent (flexible resolution) |
| update_project | bool | No | Also set `project` field to match parent's project (default: true) |
| dry_run | bool | No | Preview only (default: false) |

**Behavior:**

- For each matching task, set `parent: <new_parent>` in frontmatter
- If `update_project` is true and the new parent has a `project` field, propagate it
- Skip tasks that already have the correct parent (idempotent)
- Validate that new_parent exists before starting
- Single graph rebuild after all files written

**CLI:**
```bash
aops batch reparent --project framework --title-contains "Write spec for" \
  --new-parent epic-skill-specs --dry-run

aops batch reparent --ids "task-a,task-b,task-c" --new-parent epic-xyz
```

**MCP:**
```json
{
  "tool": "batch_reparent",
  "filters": { "project": "framework", "title_contains": "Write spec for" },
  "new_parent": "epic-skill-specs",
  "dry_run": true
}
```

**Response:**
```json
{
  "matched": 6,
  "changed": 5,
  "skipped": 1,
  "tasks": [
    { "id": "framework-b0019056", "title": "Write spec for analyst skill", "action": "reparented", "old_parent": null, "new_parent": "epic-skill-specs" },
    { "id": "framework-5a21a675", "title": "Write spec for garden skill", "action": "skipped", "reason": "already has parent epic-skill-specs" }
  ]
}
```

**TUI:**
Multi-select tasks in tree view → `r` (reparent) → type/select new parent → confirm.


### 2. `batch_update`

Update one or more frontmatter fields across multiple tasks. Generalizes archive, priority change, tag updates, etc.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| filters | FilterSet | Yes | Target selection |
| updates | Map<String, Value> | Yes | Fields to set (null to remove) |
| dry_run | bool | No | Preview only (default: false) |

**Behavior:**

- Apply `updates` map to each matching task's frontmatter
- `null` value removes the field
- Preserves all other fields and the markdown body
- Auto-sets `modified` timestamp
- Single graph rebuild after all writes

**CLI:**
```bash
# Archive stale tasks
aops batch update --project framework --stale 90d --priority-gte 3 \
  --set status=archived

# Bulk priority adjustment
aops batch update --project aops --priority 1 --tags mechanical \
  --set priority=2

# Add a tag to all tasks under an epic
aops batch update --subtree epic-xyz --add-tag needs-review

# Remove a field
aops batch update --project osb --unset assignee
```

**MCP:**
```json
{
  "tool": "batch_update",
  "filters": { "project": "framework", "stale_days": 90, "priority_gte": 3 },
  "updates": { "status": "archived" },
  "dry_run": false
}
```

**Special update operations (CLI sugar, MCP uses `updates` map):**

| CLI Flag | MCP Equivalent | Effect |
|----------|---------------|--------|
| `--set key=value` | `"key": "value"` | Set field |
| `--unset key` | `"key": null` | Remove field |
| `--add-tag foo` | `"_add_tags": ["foo"]` | Append to tags array |
| `--remove-tag foo` | `"_remove_tags": ["foo"]` | Remove from tags array |
| `--add-dep id` | `"_add_depends_on": ["id"]` | Append to depends_on |
| `--remove-dep id` | `"_remove_depends_on": ["id"]` | Remove from depends_on |
| `--superseded-by id` | `"superseded_by": "id"` | Mark as superseded + set status to done |

Common pattern: manually deduplicating without full merge semantics — set `status: done` and `superseded_by: <canonical-id>` in one operation.

**Response:** Same shape as `batch_reparent` — list of tasks with action taken.

**TUI:**
Multi-select → `u` (update) → field picker → enter value → confirm.


### 3. `batch_archive`

Convenience wrapper around `batch_update` with `status: archived`. Included because it's the most common bulk operation and benefits from extra safety checks.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| filters | FilterSet | Yes | Target selection |
| dry_run | bool | No | Preview only (default: true — note default differs!) |
| reason | string | No | Appended to task body as archive note |

**Behavior:**

- Sets `status: archived` on all matching tasks
- If `reason` is provided, appends timestamped note to markdown body: `\n\n<!-- archived 2026-03-04: {reason} -->\n`
- **Dry-run defaults to true** for safety (must explicitly pass `--execute` / `dry_run: false`)
- Warns if any matched tasks have status `in_progress` or have unresolved children
- Archived tasks are excluded from `ready` and `blocked` views (already implemented)

**CLI:**
```bash
# Preview what would be archived
aops batch archive --project framework --stale 90d --priority-gte 3

# Execute with reason
aops batch archive --project framework --stale 90d --priority-gte 3 \
  --execute --reason "stale framework tasks, superseded by plugin arch"
```

**MCP:**
```json
{
  "tool": "batch_archive",
  "filters": { "project": "framework", "stale_days": 90, "priority_gte": 3 },
  "dry_run": false,
  "reason": "stale framework tasks, superseded by plugin arch"
}
```

**TUI:**
Multi-select → `a` (archive) → optional reason → confirm.


### 4. `batch_reclassify`

Change the `type` field and move the file to the correct subdirectory. Useful for fixing memories filed as tasks and vice versa.

> **Note:** This is lower priority than it appears. The more common problem is knowledge items (type: memory/insight) being indexed alongside tasks in views and queries. The primary fix for that is improving the indexer's type filtering, not reclassification. This operation is still valuable for cases where files genuinely need to move directories (e.g., promoting tasks to epics).

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| filters | FilterSet | Yes | Target selection |
| new_type | string | Yes | New document type (task, memory, note, knowledge, project, epic, goal) |
| dry_run | bool | No | Preview only (default: false) |

**Behavior:**

- Updates `type` field in frontmatter
- Moves the file to the correct subdirectory based on type routing:
  - task/bug/epic/feature → `tasks/`
  - project → `projects/`
  - goal → `goals/`
  - memory/note/insight/observation → `memories/`
  - knowledge → `notes/`
- Updates vector store entry with new path
- Preserves all other frontmatter and body content
- Single graph rebuild after all moves

**CLI:**
```bash
# Reclassify knowledge notes that were incorrectly filed as tasks
aops batch reclassify --ids "mem-154acb01,mem-7ce6835c,mem-64eb759c" \
  --new-type memory

# Promote tasks to epics
aops batch reclassify --ids "ns-858ae0fc,ns-b43aa260" --new-type epic
```

**MCP:**
```json
{
  "tool": "batch_reclassify",
  "filters": { "ids": ["mem-154acb01", "mem-7ce6835c"] },
  "new_type": "memory"
}
```

**TUI:**
Multi-select → `t` (type) → select new type from picker → confirm.


### 5. `find_duplicates`

Detect potential duplicate tasks using title similarity and/or semantic embedding similarity.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| filters | FilterSet | No | Scope to search within (default: all tasks) |
| similarity_threshold | f64 | No | Cosine similarity threshold (default: 0.85) |
| title_threshold | u32 | No | Levenshtein distance threshold (default: 10) |
| mode | string | No | `title`, `semantic`, or `both` (default: `both`) |

**Behavior:**

- Compares all tasks within the filtered set pairwise
- Groups tasks that exceed the similarity threshold into duplicate clusters
- For each cluster, identifies a "canonical" candidate (oldest, most connected, most content)
- Returns clusters sorted by confidence

**CLI:**
```bash
# Find duplicates across entire graph
aops duplicates

# Find duplicates within a project
aops duplicates --project hdr

# Strict title matching only
aops duplicates --mode title --title-threshold 5
```

**MCP:**
```json
{
  "tool": "find_duplicates",
  "filters": { "project": "hdr" },
  "mode": "both",
  "similarity_threshold": 0.85
}
```

**Response:**
```json
{
  "clusters": [
    {
      "confidence": 0.94,
      "canonical": "hdr-c127ad6d",
      "tasks": [
        { "id": "hdr-c127ad6d", "title": "Respond to PhD inquiry from Mohammad Sazzad Ali Sakib", "project": "hdr", "created": "2025-01-15" },
        { "id": "hdr-a6043de4", "title": "Respond to Mohammad Sazzad Ali Sakib - PhD inquiry (AI liability)", "project": "hdr", "created": "2025-02-09" },
        { "id": "20251124-fc3c8848", "title": "Respond to PhD inquiry from Mohammad Sazzad Ali Sakib", "project": "hdr", "created": "2024-11-24" }
      ],
      "similarity_scores": { "title": 0.88, "semantic": 0.96 }
    }
  ],
  "total_clusters": 12,
  "total_duplicates": 31
}
```

**TUI:**
Dedicated duplicates view → shows clusters → select canonical → merge or archive others.


### 6. `batch_merge`

Merge duplicate tasks into a canonical task and archive the others.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| canonical | string | Yes | ID of the task to keep |
| merge_ids | string[] | Yes | IDs of duplicates to merge into canonical |
| strategy | string | No | `append_body`, `keep_canonical`, `combine_tags` (default: all three) |
| dry_run | bool | No | Preview only (default: false) |

**Behavior:**

- **Body:** Append unique content from merged tasks to canonical's body (as a "Merged from" section)
- **Tags:** Union of all tags
- **Dependencies:** Union of all `depends_on` and `soft_depends_on`
- **Priority:** Keep the highest (lowest number) priority
- **Children:** Reparent any children of merged tasks to canonical
- **Backlinks:** Any task that had a merged task as `parent` or `depends_on` gets updated to point to canonical
- **Archive merged tasks** with `status: archived`, `supersedes` pointing to canonical
- Single graph rebuild at end

**CLI:**
```bash
aops batch merge --canonical hdr-c127ad6d \
  --merge hdr-a6043de4,20251124-fc3c8848
```

**MCP:**
```json
{
  "tool": "batch_merge",
  "canonical": "hdr-c127ad6d",
  "merge_ids": ["hdr-a6043de4", "20251124-fc3c8848"]
}
```

**TUI:**
From duplicates view → select cluster → pick canonical → `m` (merge) → confirm.


### 6b. `batch_deduplicate` (convenience)

Combines `find_duplicates` → interactive review → `batch_merge` in one flow. This is the practical workflow users actually follow.

**CLI:**
```bash
# Find and interactively resolve duplicates
aops deduplicate --project hdr

# Auto-merge high-confidence duplicates (>0.95 similarity)
aops deduplicate --project hdr --auto --threshold 0.95
```

**Behavior:**

1. Run `find_duplicates` with the provided filters
2. Present clusters for review (CLI: interactive prompt; TUI: dedicated view)
3. For each cluster, user confirms canonical task or skips
4. Run `batch_merge` on confirmed clusters
5. Single graph rebuild at end

In `--auto` mode, skip interactive review for clusters above the threshold and auto-select the canonical candidate (oldest, most connected).

**TUI:**
Accessible from duplicate finder view as "Resolve all" action.


### 7. `batch_create_epics`

Create multiple epic containers and reparent existing tasks under them in one operation. This is the primary tool for structuring a flat task list.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| parent | string | No | Parent for all new epics (e.g., a project ID) |
| project | string | No | Project field for all new epics |
| epics | Epic[] | Yes | Array of epic definitions |
| dry_run | bool | No | Preview only (default: false) |

**Epic definition:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| title | string | Yes | Epic title |
| id | string | No | Custom ID (auto-generated if omitted) |
| priority | i32 | No | Epic priority |
| task_ids | string[] | Yes | IDs of existing tasks to reparent under this epic |
| depends_on | string[] | No | Epic-level dependencies (on other epics) |
| body | string | No | Epic description/acceptance criteria |

**Behavior:**

- Create each epic as a new task file with `type: epic`
- Reparent listed tasks under their respective epic
- Optionally set `depends_on` between epics for sequencing
- Single graph rebuild at end

**CLI:**
```bash
# Interactive: guided epic creation
aops batch create-epics --interactive --project framework

# From YAML file
aops batch create-epics --from epics.yaml
```

**YAML file format:**
```yaml
parent: aops-d109aeee  # Framework Core
project: aops
epics:
  - title: "Epic: Skill Spec Backlog"
    task_ids:
      - framework-b0019056  # Write spec for analyst skill
      - framework-5a21a675  # Write spec for garden skill
      - framework-67bf5859  # Write spec for osb-drafting skill
      - framework-4b54c256  # Write spec for extractor skill
      - framework-ebc4a65f  # Write spec for framework-debug skill
  - title: "Epic: OMCP Fixes"
    task_ids:
      - aops-iura.1  # reply corruption
      - aops-iura.2  # sent mail threading
      - aops-iura.3  # thread correlation test
    depends_on: []
```

**MCP:**
```json
{
  "tool": "batch_create_epics",
  "parent": "aops-d109aeee",
  "project": "aops",
  "epics": [
    {
      "title": "Epic: Skill Spec Backlog",
      "task_ids": ["framework-b0019056", "framework-5a21a675", "framework-67bf5859"]
    }
  ]
}
```

**TUI:**
Multi-select tasks → `e` (create epic) → enter title → confirm → tasks reparented.


### 8. `graph_stats`

Report on graph health to guide reorganization. Not a mutation — pure read operation.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| project | string | No | Scope to a project |
| verbose | bool | No | Include per-task details (default: false) |

**Metrics returned:**

| Metric | Description |
|--------|-------------|
| total_tasks | Count by status (active, archived, done, blocked) |
| orphan_count | Tasks with no parent and no project |
| max_depth | Deepest nesting level |
| avg_depth | Average nesting level (goal=1, project=2, epic=3, task=4) |
| flat_tasks | Count of leaf tasks directly under a project (no epic) |
| duplicate_candidates | Count of potential duplicates (quick title hash) |
| stale_count | Tasks not modified in >60 days |
| priority_distribution | Count per priority level |
| type_distribution | Count per document type |
| disconnected_epics | Epics with no children |
| goals_without_projects | Goals with no project children |
| projects_without_goals | Projects not linked to any goal |

**CLI:**
```bash
aops graph-stats
aops graph-stats --project aops --verbose
```

**MCP:**
```json
{ "tool": "graph_stats" }
```

**TUI:**
Status bar or dedicated health view showing key metrics.


---

## Implementation Architecture

### Module Structure

```
src/
├── batch_ops/
│   ├── mod.rs          # BatchContext, shared types, filter resolution
│   ├── filters.rs      # FilterSet parsing & evaluation
│   ├── reparent.rs     # batch_reparent logic
│   ├── update.rs       # batch_update logic (archive is a wrapper)
│   ├── reclassify.rs   # batch_reclassify logic (type change + file move)
│   ├── duplicates.rs   # find_duplicates & batch_merge logic
│   ├── epics.rs        # batch_create_epics logic
│   └── stats.rs        # graph_stats computation
├── cli.rs              # CLI surface (adds `batch` subcommand group)
├── mcp_server.rs       # MCP surface (adds batch tool handlers)
└── tui/
    └── batch_view.rs   # TUI surface (multi-select + batch actions)
```

### BatchContext

All batch operations share a context that manages the deferred rebuild pattern:

```rust
pub struct BatchContext<'a> {
    graph: &'a GraphStore,
    vectordb: &'a mut VectorDb,
    embedder: &'a Embedder,
    brain_dir: &'a Path,

    // Accumulate changes
    modified_paths: Vec<PathBuf>,
    created_paths: Vec<PathBuf>,
    deleted_paths: Vec<PathBuf>,

    // Results
    results: Vec<BatchResult>,
    errors: Vec<BatchError>,
}

impl BatchContext<'_> {
    /// Apply all accumulated changes: re-embed modified docs, rebuild graph
    pub fn commit(&mut self) -> Result<BatchSummary> {
        // 1. Batch re-embed all modified + created files
        // 2. Remove deleted entries from vector store
        // 3. Save vector store
        // 4. Rebuild graph once
    }
}
```

### Filter Resolution

```rust
pub struct FilterSet {
    pub ids: Option<Vec<String>>,
    pub project: Option<String>,
    pub parent: Option<String>,
    pub subtree: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub priority_gte: Option<i32>,
    pub tags: Option<Vec<String>>,
    pub doc_type: Option<String>,
    pub older_than_days: Option<u64>,
    pub stale_days: Option<u64>,
    pub orphan: Option<bool>,
    pub title_contains: Option<String>,
    pub complexity: Option<String>,
    pub directory: Option<String>,
    pub weight_gte: Option<u32>,
}

impl FilterSet {
    /// Resolve against graph, return matching task IDs
    pub fn resolve(&self, graph: &GraphStore) -> Vec<String> { ... }

    /// Human-readable description of the filter
    pub fn describe(&self) -> String { ... }
}
```

### Deferred Rebuild Pattern

The key performance optimization. Current flow:

```
update_task → write file → embed file → save vectordb → rebuild graph
update_task → write file → embed file → save vectordb → rebuild graph
(repeat 50x)
```

Proposed batch flow:

```
batch_update → write file₁ ... write file₅₀ → batch embed → save vectordb → rebuild graph (once)
```

Expected speedup for 50-task batch: ~40x (graph rebuild dominates at ~2s; file writes are ~5ms each).

---

## CLI Surface Design

### Command Group

```
aops batch <operation> [filters] [params]
```

Operations: `reparent`, `update`, `archive`, `reclassify`, `merge`, `create-epics`

Standalone commands (not under `batch`): `aops duplicates`, `aops graph-stats`

### Output Format

All batch commands output a summary table:

```
Batch reparent: 6 matched, 5 changed, 1 skipped

  ID                  Title                                    Action     Detail
  ────────────────────────────────────────────────────────────────────────────────
  framework-b0019056  Write spec for analyst skill              reparented → epic-skill-specs
  framework-5a21a675  Write spec for garden skill               reparented → epic-skill-specs
  framework-ebc4a65f  Write spec for framework-debug skill      reparented → epic-skill-specs
  framework-67bf5859  Write spec for osb-drafting skill         reparented → epic-skill-specs
  framework-4b54c256  Write spec for extractor skill            reparented → epic-skill-specs
  20260105-write-...  Write spec for framework-review skill     skipped    already under epic-skill-specs
```

With `--dry-run`, prefix output with:

```
DRY RUN — no files modified. Pass --execute to apply.
```

### Confirmation Prompt

For non-dry-run operations affecting >5 tasks:

```
This will modify 23 tasks. Continue? [y/N]
```

Override with `--yes` flag.

---

## MCP Surface Design

### Tool Registration

Add 8 new tools to `PkbSearchServer`:

| Tool Name | Category |
|-----------|----------|
| `batch_reparent` | Batch Operations |
| `batch_update` | Batch Operations |
| `batch_archive` | Batch Operations |
| `batch_reclassify` | Batch Operations |
| `batch_merge` | Batch Operations |
| `batch_deduplicate` | Batch Operations |
| `batch_create_epics` | Batch Operations |
| `find_duplicates` | Analysis |
| `graph_stats` | Analysis |

### Tool Descriptions

Tool descriptions should be agent-optimized — clear about when to use, what parameters mean, and what the response contains. Example:

```
batch_reparent: Move multiple tasks to a new parent in one operation.
Use when restructuring the task graph — grouping flat tasks into epics,
or reorganizing tasks between projects. Supports filtered selection
(by project, priority, tags, age, etc.) or explicit ID lists.
Returns a list of tasks with the action taken on each.
Set dry_run=true to preview changes without writing.
```

### Error Handling

MCP responses include both successes and failures:

```json
{
  "matched": 10,
  "changed": 8,
  "skipped": 1,
  "errors": [
    { "id": "task-xyz", "error": "file not found on disk (index stale?)" }
  ],
  "tasks": [ ... ]
}
```

---

## TUI Surface Design

### Multi-Select Mode

The TUI tree view gets a multi-select mode:

- `Space` — toggle selection on current item
- `v` — enter visual/multi-select mode (like vim visual mode)
- `V` — select all visible tasks at current level
- `Escape` — clear selection

### Batch Action Bar

When tasks are selected, a context bar appears at the bottom:

```
3 selected │ [r]eparent [a]rchive [u]pdate [e]pic [t]ype [m]erge │ [Esc] clear
```

### Duplicate Finder View

A dedicated view (`d` from main) showing duplicate clusters as collapsible groups. Within each group, radio-select the canonical task and apply merge.

### Graph Stats Dashboard

A dedicated view (`g` from main) showing the health metrics from `graph_stats`, with drill-down into each metric (e.g., click "47 orphans" to see the list).

---

## Implementation Priority

### Phase 1: Core Batch Infrastructure (ship first)

1. `FilterSet` — shared filter resolution module
2. `BatchContext` — deferred rebuild pattern
3. `batch_update` — most general, covers archive as special case
4. `batch_reparent` — highest immediate need
5. `batch_archive` — convenience wrapper with safety defaults
6. `graph_stats` — read-only, no risk, guides further work

### Phase 2: Deduplication & Restructuring

7. `find_duplicates` — analysis tool
8. `batch_merge` — act on duplicate findings
9. `batch_deduplicate` — convenience flow combining find + review + merge
10. `batch_create_epics` — primary structuring tool
11. `batch_reclassify` — type corrections and file moves (lower priority; indexer type filtering is the primary fix for misclassified items appearing in views)

### Phase 3: TUI Integration

12. Multi-select mode in tree view
13. Batch action bar
14. Duplicate finder view
15. Graph stats dashboard

---

## Test Strategy

### Unit Tests

- FilterSet resolution against a synthetic graph (10-20 nodes)
- Each batch operation with mock filesystem
- Idempotency: running same operation twice produces same result
- Error cases: missing files, invalid IDs, circular reparent attempts

### Integration Tests

- Create temp brain directory with known task files
- Run batch operations through CLI and MCP surfaces
- Verify file contents match expected YAML + body
- Verify graph rebuild produces correct edges

### Property Tests

- For any valid FilterSet, `resolve()` returns a subset of all tasks
- For any batch_reparent, all modified tasks have correct `parent` field
- For any batch_merge, merged tasks have `status: archived` and `supersedes` set

---

## Design Decisions (resolved)

1. **`batch_archive` — status field only, don't move files.** File moves break path references and make git history harder to follow. The indexer already filters by status. Archived tasks stay in their current directory.

2. **`batch_reparent` cascades project field by default.** When reparenting a task under an epic that belongs to project X, the task's `project` field also updates. Opt out with `--no-cascade` / `update_project: false`. This prevents tasks from appearing as "ungrouped" when they have a parent but no project.

3. **No transaction rollback in v1.** The per-file atomic pattern (write to temp, rename) is sufficient. Git gives you rollback for free (`git checkout -- .`). Failures are reported in the response.

4. **Full graph rebuild in v1.** `build_from_directory()` at ~2s is acceptable for batch ops. Incremental graph updates are a future optimization.

5. **`--weight-gte` included in Phase 1.** Useful for finding structurally important nodes during grooming sessions.

---

## Autocommit Integration

Batch operations interact with the `autocommit_state.py` PostToolUse hook. Since batch MCP tools use `batch_*` prefixes, they won't match the current suffix patterns in `get_modified_repos()`.

**Decision:** The batch ops module owns the commit. Since it already performs a single graph rebuild at the end, it produces a single commit with a descriptive message:

```
tasks: batch reparent 12 tasks under epic-xyz
tasks: batch archive 23 stale framework tasks
tasks: batch merge 3 duplicates into hdr-c127ad6d
```

This is cleaner than having the autocommit hook try to infer what changed. The `BatchContext::commit()` method handles:
1. Write all files
2. Re-embed modified documents
3. Rebuild graph
4. Git add + commit with descriptive message
5. Return `BatchSummary`

The autocommit hook should be updated to skip batch tool calls (they handle their own commits).
