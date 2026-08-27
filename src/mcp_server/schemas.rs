use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{Error as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{PkbSearchServer, GOAL_TYPE_ENUM};

impl PkbSearchServer {
    pub fn get_all_tools() -> Vec<Tool> {
        vec![
            Tool::new(
                "get_consolidation_cluster",
                "Get a subgraph of related notes for a consolidation batch. Returns the seed node and top related notes based on graph proximity, vector similarity, and tag orphans.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "seed_id": { "type": "string", "description": "Optional specific seed node ID" },
                        "max_nodes": { "type": "integer", "description": "Max nodes to return (default: 10)" },
                        "vector_top_k": { "type": "integer", "description": "Vector nearest neighbors to consider (default: 10)" }
                    }
                })).unwrap()
            )
            .with_title("Get Consolidation Cluster")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "apply_consolidation_batch",
                "Applies a batch mutation for a consolidation run across multiple documents with pre-flight checks, atomic per-file writes, and verified rollback on failure.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "seed_id": { "type": "string", "description": "The seed node ID being consolidated" },
                        "updates": { 
                            "type": "object", 
                            "description": "Map of node ID to frontmatter modifications and/or 'body' replacement." 
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "Preview only (default: true — must explicitly set false to execute)"
                        }
                    },
                    "required": ["seed_id", "updates"]
                })).unwrap()
            )
            .with_title("Apply Consolidation Batch")
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(true)),
            Tool::new(
                "search",
                "Hybrid semantic + graph-proximity search across the personal knowledge base. Use this for general discovery and finding related knowledge. Supports proximity boosting.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural language search query" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
                        "type": { "type": "string", "description": "Filter by document type (e.g. 'task', 'template', 'note', '!task' to exclude tasks, or comma-separated list 'task,epic')" },
                        "since": { "type": "string", "description": "Filter: return only results modified on or after YYYY-MM-DD (inclusive). Documents with no modified date are excluded." },
                        "before": { "type": "string", "description": "Filter: return only results modified on or before YYYY-MM-DD (inclusive). Documents with no modified date are excluded." },
                        "boost_id": { "type": "string", "description": "Optional: boost results near this node (ID, filename, or title)" },
                        "detail": { "type": "string", "description": "Result detail level: 'snippet' (300 chars), 'chunk' (full matching chunk, default), 'full' (entire document)", "enum": ["snippet", "chunk", "full"], "default": "chunk" }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            )
            .with_title("Hybrid Semantic Search")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_document",
                "Read the full contents of a specific PKB document. Use when you need the complete text for analysis. ONLY accepts short-form ID (e.g. task-xxx), filename stem, or title.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID, filename stem, title, or permalink (uses flexible resolution)" },
                        "max_bytes": { "type": "integer", "description": "Optional: truncate the returned body to at most N bytes (UTF-8-safe), appending a truncation marker. Default: unset — full body is returned." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Document Content")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_documents",
                "List indexed documents with optional filters. Good for browsing specific types, tags, or status groups with pagination.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": { "type": "string", "description": "Filter by tag" },
                        "type": { "type": "string", "description": "Filter by type" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "limit": { "type": "integer", "description": "Max results (default: all matching, capped at 1000 — use offset to page further)" },
                        "offset": { "type": "integer", "description": "Skip first N results (default: 0)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Documents")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "task_search",
                "Semantic search filtered to actionable tasks. Returns results with rich graph context including status and dependencies. Done/cancelled tasks are hidden by default — when you're looking for work to do, completed tasks are noise; pass `include_done=true` to override. Use `type: \"epic\"` to find container tasks (with context and subtasks) rather than leaf tasks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Query to search tasks" },
                        "limit": { "type": "integer", "description": "Max results (default: 10)" },
                        "since": { "type": "string", "description": "Filter: return only tasks modified on or after YYYY-MM-DD (inclusive). Tasks with no modified date are excluded." },
                        "before": { "type": "string", "description": "Filter: return only tasks modified on or before YYYY-MM-DD (inclusive). Tasks with no modified date are excluded." },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false." },
                        "include_done": { "type": "boolean", "description": "Include done and cancelled tasks. Default: false (hides closed tasks so search returns actionable work)." },
                        "type": { "type": "string", "description": "Filter by task type. Single value (e.g. 'epic') or comma-separated list (e.g. 'epic,feature'). Recognised actionable types: epic, task, learn. Default: all actionable types." }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            )
            .with_title("Task Search")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_network_metrics",
                "Calculate centrality metrics (PageRank, betweenness, degree) for a node. Use to identify high-impact or 'load-bearing' tasks in the graph.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Network Metrics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "top_n_by_metric",
                "Return the top N nodes ranked by a specified centrality metric (pagerank, betweenness, degree), optionally filtered by node type.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "metric": {
                            "type": "string",
                            "enum": ["pagerank", "betweenness", "degree"],
                            "description": "Centrality metric to rank by (pagerank, betweenness, degree)"
                        },
                        "n": {
                            "type": "integer",
                            "description": "Number of top nodes to return (default: 10)",
                            "minimum": 1
                        },
                        "node_type": {
                            "type": "string",
                            "description": "Optional node type filter (e.g. 'task', 'epic')"
                        }
                    },
                    "required": ["metric"]
                }))
                .unwrap(),
            )
            .with_title("Top N Nodes by Centrality Metric")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "create_task",
                "Create a new task markdown file with YAML frontmatter. Only `title` is required; `parent` and `project` are optional but recommended — omitted tasks inherit the nearest ancestor's project and become orphans without a parent. Supports the verbal contribution-weight scale via `contributes_to` and severity-based prioritization. Parent/child cycles are rejected at write time.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Task title (also accepts `task_title` as alias)" },
                        "task_title": { "type": "string", "description": "Alias for title" },
                        "id": { "type": "string", "description": "Task ID (auto-generated if omitted)" },
                        "parent": { "type": "string", "description": "Parent task ID" },
                        "priority": { "type": "integer", "description": "Priority band 0-4 (P0 Critical / P1 Active intent / P2 Active work / P3 Planned / P4 Backlog). Default when unset: P3 (Planned). Agents must NOT originate a non-default band — leave unset (defaults to P3). Set only when Nic expressly directs a band. Express importance via `contributes_to` `stated_weight`, never priority. See specs/ranking.md §2.1 and §4.6." },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Free-form tags for search and filtering" },
                        "depends_on": { "type": "array", "items": { "type": "string" }, "description": "IDs of tasks that must complete before this one is unblocked" },
                        "assignee": { "type": "string", "description": "Who is responsible for this task" },
                        "complexity": { "type": "string", "description": "Free-form complexity/size label (e.g. 'S', 'M', 'L')" },
                        "effort": { "type": "string", "description": "Effort duration string: '1d', '2h', '1w'. Parser converts to days." },
                        "consequence": { "type": "string", "description": "Narrative description of what happens if this task is not done or fails." },
                        "severity": { "type": "integer", "description": "Severity ladder (0-4) for target nodes. SEV4 is lexicographic." },
                        "goal_type": { "type": "string", "description": "Goal classification: committed | aspirational | learning.", "enum": GOAL_TYPE_ENUM },
                        "body": { "type": "string", "description": "Markdown body" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." },
                        "due": { "type": "string", "description": "Due date (ISO date, e.g. '2026-06-01')" },
                        "project": { "type": "string", "description": "Project routing slug, validated against polecat.yaml (slug or any registered alias; canonicalized on write). Optional — omitted tasks inherit the nearest ancestor's project. Builtins 'task' and 'adhoc-sessions' are always accepted." },
                        "type": { "type": "string", "enum": ["task", "epic", "learn", "pr", "goal", "target"], "description": "Task type (default: 'task'). Also accepts: epic, learn, pr, goal, target. `goal` and `target` are out-of-tree strategic nodes (no parent required)." },
                        "status": { "type": "string", "enum": ["inbox", "ready"], "description": "Task status. Real default when unset: 'inbox' (captured, untriaged). Creators legitimately set only 'inbox' or 'ready' — 'ready' means decomposed to a leaf with all hard deps resolved. The inbox→ready transition is auto-computed once a task graduates, so leaving it 'inbox' is fine. Do NOT set queued/in_progress/terminal statuses at create time. See TAXONOMY §Status Values and Transitions." },
                        "allow_missing_parent": { "type": "boolean", "description": "Allow creating under a missing parent (logs warning). Default: false." },
                        "force": { "type": "boolean", "description": "Allow creating under a closed (done/cancelled/archived) parent. Default: false." },
                        "session_id": { "type": "string", "description": "Active session ID to record on the task. Falls back to $AOPS_SESSION_ID if omitted." },
                        "issue_url": { "type": "string", "description": "External issue/ticket URL to link on the task" },
                        "follow_up_tasks": { "type": "array", "items": { "type": "string" }, "description": "IDs of related follow-up tasks" },
                        "release_summary": { "type": "string", "description": "Detailed technical summary, if creating this task as part of a release/handover" },
                        "contributes_to": { "type": "array", "items": { "type": "object" }, "description": "Edges to goal/target nodes this task contributes to, e.g. [{\"target\": \"target-id\", \"stated_weight\": \"expected\"}]. Supports `inherits_from` to copy fields from a prototype edge." }
                    },
                    "required": ["title"]
                }))
                .unwrap(),
            )
            .with_title("Create Task")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "claim_task",
                "Claim a task. On a `type: task` (or untyped) node, claims IN PLACE: sets \
                 status: in_progress (+ optional assignee/session_id), no new node created. \
                 On a `type: template` node, instantiates a datestamped copy \
                 (<slug>-<YYYYMMDD>-<HHMM>-<host>.md) and returns it via get_task; the template \
                 itself is left untouched. Use the template path for recurring workflows: \
                 daily, reflect, handover, issue-sweep, etc.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (any type: task) or template ID (type: template)" },
                        "assignee": { "type": "string", "description": "Who is claiming this (task-node path only, ignored for templates)." },
                        "session_id": { "type": "string", "description": "Active session ID (task-node path only). Falls back to $AOPS_SESSION_ID if omitted." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Claim Template")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "create_memory",
                "Create a new memory, insight, or observation. Stored in memories/ directory. Use for recording persistent knowledge or session findings.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Memory title" },
                        "id": { "type": "string", "description": "Memory ID (auto-generated if omitted)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for the memory" },
                        "body": { "type": "string", "description": "Markdown body content" },
                        "memory_type": { "type": "string", "enum": ["memory", "note", "insight", "observation"], "description": "Subtype: memory (default), note, insight, observation" },
                        "source": { "type": "string", "description": "Source context (e.g. session ID)" },
                        "confidence": { "type": "number", "description": "Confidence level (0.0 - 1.0)", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string", "description": "ID of memory this one replaces" }
                    },
                    "required": ["title"]
                }))
                .unwrap(),
            )
            .with_title("Create Memory")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "create",
                "Generic document creation with automatic subdirectory routing (tasks/, projects/, goals/, notes/). Use when the specific specialized tool is not applicable.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Document title (required)" },
                        "type": { "type": "string", "enum": ["epic", "task", "learn", "pr", "template", "goal", "target", "note", "knowledge", "memory", "insight", "observation", "contact", "document", "reference", "review", "case", "spec", "prototype", "index"], "description": "Document type (required): note, knowledge, memory, insight, observation, task, epic, goal, target, etc." },
                        "id": { "type": "string", "description": "Document ID (auto-generated if omitted)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Free-form tags for search and filtering" },
                        "body": { "type": "string", "description": "Markdown body" },
                        "status": { "type": "string", "enum": ["inbox", "ready"], "description": "Document/task status. Real default when unset: 'inbox' (captured, untriaged). Creators legitimately set only 'inbox' or 'ready' — 'ready' means decomposed to a leaf with all hard deps resolved. The inbox→ready transition is auto-computed once a task graduates, so leaving it 'inbox' is fine. Do NOT set queued/in_progress/terminal statuses at create time. See TAXONOMY §Status Values and Transitions." },
                        "priority": { "type": "integer", "description": "Priority band 0-4 (P0 Critical / P1 Active intent / P2 Active work / P3 Planned / P4 Backlog). Default when unset: P3 (Planned). Agents must NOT originate a non-default band — leave unset (defaults to P3). Set only when Nic expressly directs a band. Express importance via `contributes_to` `stated_weight`, never priority. See specs/ranking.md §2.1 and §4.6." },
                        "parent": { "type": "string", "description": "Parent document ID" },
                        "depends_on": { "type": "array", "items": { "type": "string" }, "description": "IDs of documents that must complete before this one is unblocked" },
                        "assignee": { "type": "string", "description": "Who is responsible for this document" },
                        "complexity": { "type": "string", "description": "Free-form complexity/size label (e.g. 'S', 'M', 'L')" },
                        "source": { "type": "string", "description": "Source context" },
                        "due": { "type": "string", "description": "Due date" },
                        "confidence": { "type": "number", "description": "Confidence level (0.0 - 1.0)", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string", "description": "ID of document this one replaces" },
                        "severity": { "type": "integer", "description": "Severity ladder (0-4) for target nodes." },
                        "goal_type": { "type": "string", "description": "Goal classification: committed | aspirational | learning.", "enum": GOAL_TYPE_ENUM },
                        "dir": { "type": "string", "description": "Override subdirectory placement" },
                        "stakeholder": { "type": "string", "description": "Who is waiting on this task (e.g. 'Jacob', 'funding-committee'). Drives waiting urgency in focus scoring." },
                        "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date, e.g. '2026-03-20'). Falls back to created date if omitted." }
                    },
                    "required": ["title", "type"]
                }))
                .unwrap(),
            )
            .with_title("Create Document")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "append",
                "Append timestamped content to an existing document by ID. Use for logging progress, adding references, or updating a 'log' section. Not idempotent. The write path is otherwise last-write-wins: pass `expected_modified` (the `modified` value from your last read of this document) to get a compare-and-swap guard — the call is rejected with error_type `stale_write` if the document changed since your read, instead of silently discarding the other writer's change.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "content": { "type": "string", "description": "Content to append (will be timestamped)" },
                        "section": { "type": "string", "description": "Optional target section heading (e.g. 'Log', 'References'). Creates section if not found." },
                        "expected_modified": { "type": "string", "description": "Optional compare-and-swap precondition: the `modified` frontmatter value you last read from this document. If the document's current `modified` no longer matches, the call fails with error_type `stale_write` instead of overwriting a concurrent change. Omit to keep unconditional (last-write-wins) behaviour." }
                    },
                    "required": ["id", "content"]
                }))
                .unwrap(),
            )
            .with_title("Append to Document")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "add_observations",
                "Insert one or more single-line observations (bulleted; optionally timestamped) into a document body under an optional section heading. Atomically preserves all other body content without reformatting. The write path supports compare-and-swap via `expected_modified`: if the document's current `modified` value no longer matches, the call fails with error_type `stale_write`.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "lines": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "One or more single-line observations to insert"
                        },
                        "section": { "type": "string", "description": "Optional target section heading (e.g. 'Observations', 'Log'). Appends to section if found, or creates section if not found. If omitted, appends to the end of the document body." },
                        "timestamped": { "type": "boolean", "description": "Optional: prefix each observation with a UTC timestamp (default: false)" },
                        "expected_modified": { "type": "string", "description": "Optional compare-and-swap precondition: the `modified` frontmatter value you last read from this document. If current `modified` does not match, returns error_type `stale_write`." }
                    },
                    "required": ["id", "lines"]
                }))
                .unwrap(),
            )
            .with_title("Add Observations")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "delete_observations",
                "Delete one or more single-line observations from a document body by exact content match (matching the bullet line or the observation text). Atomically preserves all other body content without reformatting. A selector matching no lines returns an error naming it — no silent success. Supports compare-and-swap via `expected_modified`.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "selectors": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "One or more selectors to match and remove lines"
                        },
                        "expected_modified": { "type": "string", "description": "Optional compare-and-swap precondition: the `modified` frontmatter value you last read from this document. If current `modified` does not match, returns error_type `stale_write`." }
                    },
                    "required": ["id", "selectors"]
                }))
                .unwrap(),
            )
            .with_title("Delete Observations")
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(true)),
            Tool::new(
                "update_body",
                "Atomically rewrite the prose body of an existing document. Preserves YAML frontmatter and bumps `modified` timestamp by default. Use instead of `append` when you need full replacement rather than additive logging. The write path is otherwise last-write-wins: pass `expected_modified` (the `modified` value from your last read of this document) to get a compare-and-swap guard — the call is rejected with error_type `stale_write` if the document changed since your read, instead of silently discarding the other writer's change.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "new_body": { "type": "string", "description": "New markdown body content (full replacement — not appended)" },
                        "preserve_frontmatter": { "type": "boolean", "description": "Keep YAML frontmatter and bump modified timestamp (default: true). Set false only when replacing scratch documents with no meaningful frontmatter." },
                        "expected_modified": { "type": "string", "description": "Optional compare-and-swap precondition: the `modified` frontmatter value you last read from this document. If the document's current `modified` no longer matches, the call fails with error_type `stale_write` instead of overwriting a concurrent change. Omit to keep unconditional (last-write-wins) behaviour." }
                    },
                    "required": ["id", "new_body"]
                }))
                .unwrap(),
            )
            .with_title("Rewrite Document Body")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "edit_body",
                "Apply one or more unified-diff hunks to the body of an existing document (Aider-compatible format). Preserves YAML frontmatter and updates modified timestamp by default. Supports fallback matching strategies (whitespace tolerance, relative indentation, context shrinking) when a hunk does not match verbatim. Use expected_modified to enforce a compare-and-swap (CAS) check against concurrent updates.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, or title)" },
                        "diff": { "type": "string", "description": "Unified-diff hunks to apply to the body (standard git-style hunk format or markdown ```diff block)" },
                        "preserve_frontmatter": { "type": "boolean", "description": "Keep YAML frontmatter and bump modified timestamp (default: true). Set false only when editing scratch documents with no meaningful frontmatter." },
                        "dry_run": { "type": "boolean", "description": "If true, validates and previews the diff application without writing to disk (default: false)" },
                        "expected_modified": { "type": "string", "description": "Optional compare-and-swap precondition: the `modified` frontmatter value you last read from this document. If the document's current `modified` no longer matches, the call fails with error_type `stale_write` instead of overwriting a concurrent change. Omit to keep unconditional (last-write-wins) behaviour." }
                    },
                    "required": ["id", "diff"]
                }))
                .unwrap(),
            )
            .with_title("Edit Document Body (Unified Diff)")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "delete",
                "Permanently delete a document by ID. Removes the file from disk and the vector store index. Use with caution.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (task ID, memory ID, filename stem, or title). Uses flexible resolution." },
                        "type": { "type": "string", "description": "Optional type guard — if set, the call fails unless the resolved document's type matches. Pass 'memory' to restrict to memory-type documents (memory, note, insight, observation); any other value requires an exact (case-insensitive) node_type match." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Delete Document")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "complete_task",
                "Mark a task as done. Requires completion_evidence describing what was achieved. Sets status to 'done', appends evidence to body, and re-indexes.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (supports flexible resolution: ID, filename stem, or title)" },
                        "completion_evidence": { "type": "string", "description": "What was done + outcome. Required — describe the work before completing." },
                        "pr_url": { "type": "string", "description": "Link to PR/commit (optional, for code tasks)" },
                        "recursive": { "type": "boolean", "description": "Cascade-close all open descendant tasks. Default: false (rejects if open children exist)." }
                    },
                    "required": ["id", "completion_evidence"]
                }))
                .unwrap(),
            )
            .with_title("Complete Task")
            .with_annotations(ToolAnnotations::new().read_only(false).idempotent(true)),
            Tool::new(
                "release_task",
                "Release a task to a terminal or handoff status (merge_ready, done, review, blocked, cancelled, partial). Performs session handover by recording work history, linking PRs/issues, and tracking follow-up work. If 'id' is omitted, an ad-hoc session task is created. Evidence-or-failure-reason contract: `summary` is always required; releasing to blocked/cancelled/review/partial additionally requires a non-empty `reason` (or `blocker`, for `blocked`) — a handback with neither is rejected. Tasks created before this requirement shipped release under the old, optional rules.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (flexible resolution: ID, filename stem, or title). If omitted, an ad-hoc task is created." },
                        "status": {
                            "type": "string",
                            "enum": ["merge_ready", "done", "review", "blocked", "cancelled", "partial"],
                            "description": "Target status"
                        },
                        "summary": { "type": "string", "description": "What was done and outcome. 1-3 sentences minimum. Always required." },
                        "pr_url": { "type": "string", "description": "Pull request or commit URL (recommended for merge_ready)" },
                        "branch": { "type": "string", "description": "Git branch name (optional)" },
                        "blocker": { "type": "string", "description": "What is blocking this task. Required (non-empty), together with `reason`, when status=blocked — at least one of the two must be given." },
                        "reason": { "type": "string", "description": "Why the task didn't ship cleanly. Required (non-empty) when status is blocked/cancelled/review/partial (for status=blocked, `blocker` may be given instead)." },
                        "session_id": { "type": "string", "description": "Active session ID. Falls back to $AOPS_SESSION_ID if omitted." },
                        "issue_url": { "type": "string", "description": "External issue/ticket URL" },
                        "follow_up_tasks": { "type": "array", "items": { "type": "string" }, "description": "IDs of new tasks created as follow-ups. Validated for existence." },
                        "release_summary": { "type": "string", "description": "Detailed technical summary for the release. Warning if > 500 chars." },
                        "recursive": { "type": "boolean", "description": "Cascade-close all open descendant tasks when releasing as done or cancelled. Default: false." }
                    },
                    "required": ["status", "summary"]
                }))
                .unwrap(),
            )
            .with_title("Release Task")
            .with_annotations(ToolAnnotations::new().read_only(false).idempotent(true)),
            Tool::new(
                "list_tasks",
                "List tasks with smart filtering. Results are returned in `focus_score`-descending order by default (highest-focus first) across every path — default, ready, and blocked — with a deterministic tie-break, so the top rows are the most important and repeated calls are stable; no re-sort is needed. Every row carries `status`. Done/cancelled tasks are hidden by default — pass `include_done=true` to include them (matches task_search convention). JSON output puts `focus_score` (composite ranking integer) at the top level as the primary sort key, combining priority, severity, deadline urgency, age/staleness, downstream weight, stakeholder waiting urgency, urgency (propagated/chain), and value of information (VoI). Component signals — criticality, urgency, downstream_weight, scope, uncertainty — are nested under `signals: {}` for filter/debug use. See specs/ranking.md. Use status='ready' for actionable leaf tasks or status='blocked' to see blockers.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project slug or any polecat.yaml alias (case-insensitive). Returns tasks whose project field (own or inherited from the nearest ancestor that set one) matches." },
                        "status": { "type": "string", "description": "Filter by status. Special values: 'ready' (actionable leaf tasks), 'blocked' (tasks with unmet deps). Also any canonical status: inbox, queued, in_progress, review, merge_ready, paused, someday, partial, done, cancelled." },
                        "priority": { "type": "integer", "description": "Filter to tasks whose effective priority (own or any downstream task via blocks/parent) ≤ N. E.g. priority=0 returns every task that touches a P0, including its blockers." },
                        "severity": { "type": "integer", "description": "Filter by exact severity" },
                        "goal_type": { "type": "string", "description": "Filter by goal type" },
                        "assignee": { "type": "string", "description": "Filter by assignee" },
                        "type": { "type": "string", "description": "Filter by document type (e.g. 'target', 'epic', 'task')" },
                        "title_contains": { "type": "string", "description": "Filter by title substring (case-insensitive)" },
                        "complexity": { "type": "string", "description": "Filter by complexity (e.g. 'low', 'medium', 'high')" },
                        "weight_gte": { "type": "integer", "description": "Filter to tasks with downstream weight ≥ N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags. A task matches iff every requested tag is present in its frontmatter `tags` array (AND, case-insensitive)." },
                        "has_superseded_by": { "type": "boolean", "description": "Filter by presence of `superseded_by` frontmatter. When true, returns only tasks where `superseded_by` is set; when false, returns only tasks where it is absent. When this filter is active, returned rows (both markdown and JSON) include the `superseded_by` target value. Default: unset (no filter applied)." },
                        "focus_score_gte": { "type": "integer", "description": "Filter to tasks whose composite focus_score is ≥ N." },
                        "since": { "type": "string", "description": "Filter: return only tasks modified on or after YYYY-MM-DD (inclusive). Tasks with no modified date are excluded." },
                        "before": { "type": "string", "description": "Filter: return only tasks modified on or before YYYY-MM-DD (inclusive). Tasks with no modified date are excluded." },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask) in results. Default: false — subtasks are hidden since they travel with their parent task." },
                        "include_done": { "type": "boolean", "description": "Include done and cancelled tasks. Default: false (silently hides closed tasks so the list shows actionable work, which can cause state blindness if you aren't expecting it). Ignored when an explicit `status` filter is provided." },
                        "format": { "type": "string", "enum": ["markdown", "json"], "description": "Output format. 'json' returns structured {total, showing, tasks[]} for programmatic use. Default: 'markdown'." }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Tasks")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_task",
                "Retrieve full details for a task, including metadata, body content, and graph relationship context (dependencies, blockers, children, subtasks). The returned JSON puts `focus_score` (composite ranking integer) at the top level as the primary importance metric, combining priority, severity, deadline urgency, age/staleness, downstream weight, stakeholder waiting urgency, urgency (propagated/chain), and value of information (VoI). Component signals — criticality, urgency, downstream_weight, scope, uncertainty — are nested under `signals: {}` for filter/debug use. See specs/ranking.md.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (e.g. 'framework-6b4325a1'). Also accepts filename stem or title." },
                        "max_bytes": { "type": "integer", "description": "Optional: truncate the returned body to at most N bytes (UTF-8-safe), appending a truncation marker. Default: unset — full body is returned." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Task Detail")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "update_task",
                "Patch metadata fields on an existing task. Pass fields either nested as `updates: {status: \"done\"}` or flat at the top level (e.g. status=\"done\"). Use for non-terminal updates (tags, assignee, body). For state transitions (done, merge_ready, blocked, cancelled, review, partial), prefer release_task — it also enforces the evidence-or-failure-reason contract (completion_evidence for done; reason/blocker for handback statuses) that this generic patch path does not. Setting status=\"done\" here still requires a non-empty `completion_evidence` field (in updates or top-level), unchanged. A caller-supplied `priority` is rejected outright — priority bands are Nic's call only, never an agent's.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID (flexible resolution: ID, filename stem, title)" },
                        "updates": { "type": "object", "description": "Optional nested form. JSON object of fields to update (null to remove a field). If omitted, any top-level fields other than id/updates are treated as fields to update." },
                        "recursive": { "type": "boolean", "description": "When setting status to done/cancelled/archived, cascade-close all open descendant tasks. Default: false (rejects if open children exist)." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Update Task")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "retrieve_memory",
                "Find relevant memories, insights, or observations by semantic similarity. Returns full content for the top matches.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query for finding relevant memories" },
                        "limit": { "type": "integer", "description": "Maximum number of memories to return (default: 10)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Only return memories with all of these tags" }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            )
            .with_title("Retrieve Memory")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "search_by_tag",
                "Find all documents sharing a specific set of tags. Supports filtering by document type.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags to search for (all must match)" },
                        "type": { "type": "string", "description": "Filter by document type (e.g. 'memory', 'task')" },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" }
                    },
                    "required": ["tags"]
                }))
                .unwrap(),
            )
            .with_title("Search by Tag")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_memories",
                "Browse memory-type documents (notes, insights, observations) with optional tag filtering.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: 20)" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags (all must match)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Memories")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "decompose_task",
                "Split a large task into multiple subtasks in one operation. Supports relative sibling references (e.g. '$1') for dependencies. Subtasks inherit the parent's `project` field unless explicitly overridden. Use to structure a newly defined work package.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent_id": { "type": "string", "description": "Parent task ID to create subtasks under" },
                        "subtasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string", "description": "Subtask title (required; must be unique among siblings for cross-referencing)" },
                                    "id": { "type": "string", "description": "Optional custom task ID" },
                                    "priority": { "type": "integer", "description": "Priority band 0-4 (P0 Critical .. P4 Backlog). Default when unset: P3 (Planned). Agents must NOT set this — a caller-supplied value is rejected outright. Priority bands are Nic's call only." },
                                    "depends_on": { "type": "array", "items": { "type": "string" }, "description": "Dependency IDs. Supports positional sibling refs ('$1', '$2', ... 1-indexed into this subtasks array) and case-insensitive sibling title matches, in addition to existing task IDs." },
                                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Free-form tags for search and filtering" },
                                    "assignee": { "type": "string", "description": "Who is responsible for this subtask" },
                                    "complexity": { "type": "string", "description": "Free-form complexity/size label (e.g. 'S', 'M', 'L')" },
                                    "body": { "type": "string", "description": "Markdown body" },
                                    "stakeholder": { "type": "string", "description": "Who is waiting on this subtask. Drives waiting urgency in focus scoring." },
                                    "waiting_since": { "type": "string", "description": "When the stakeholder started waiting (ISO date). Falls back to created date if omitted." },
                                    "consequence": { "type": "string", "description": "Narrative description of what happens if this subtask is not done or fails" },
                                    "severity": { "type": "integer", "description": "Severity ladder (0-4); only honored for type: target subtasks, otherwise reset to 0 with a warning" },
                                    "goal_type": { "type": "string", "description": "Goal classification: committed | aspirational | learning" },
                                    "project": { "type": "string", "description": "Override project field (defaults to parent's project)" }
                                },
                                "required": ["title"]
                            },
                            "description": "Array of subtask definitions"
                        }
                    },
                    "required": ["parent_id", "subtasks"]
                }))
                .unwrap(),
            )
            .with_title("Decompose Task")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "get_dependency_tree",
                "Visualize the task dependency graph for a specific node. Upstream shows what the task depends on; downstream shows what it blocks.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "direction": { "type": "string", "description": "Direction: 'upstream' (depends on, default) or 'downstream' (blocks)" },
                        "max_depth": { "type": "integer", "description": "Maximum traversal depth in hops (default: 10)" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Dependency Tree")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_task_children",
                "List all direct or recursive children of a task. Returns completion counts and status for the subtree. Use to assess progress of an epic or parent task.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "recursive": { "type": "boolean", "description": "Include all descendants, not just direct children (default: false)" },
                        "limit": { "type": "integer", "description": "Max child rows to print, most relevant when recursive=true on a deep subtree (default: 200). The completion summary still reflects the full subtree." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Task Children")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_trace",
                "Find shortest paths between two nodes in the knowledge graph. Useful for understanding how two seemingly unrelated concepts or tasks are linked.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Source node (ID, filename, or title)" },
                        "to": { "type": "string", "description": "Target node (ID, filename, or title)" },
                        "max_paths": { "type": "integer", "description": "Maximum paths to return (default: 3)" }
                    },
                    "required": ["from", "to"]
                }))
                .unwrap(),
            )
            .with_title("PKB Path Trace")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "pkb_orphans",
                "Identify disconnected nodes with no valid parent. Use to maintain graph integrity and ensure all tasks are properly situated in the hierarchy.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max results (default: all). Set to 0 for unlimited." },
                        "types": { "type": "array", "items": { "type": "string" }, "description": "Filter by node type (e.g. [\"task\"], [\"task\", \"epic\"]). Overrides default actionable-only filter." },
                        "include_all": { "type": "boolean", "description": "Include all node types (notes, memories, etc.) — default false." }
                    }
                }))
                .unwrap(),
            )
            .with_title("Find Orphan Nodes")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            // ── Batch Operations ──────────────────────────────────────────
            Tool::new(
                "batch_update",
                "Apply frontmatter updates to multiple tasks simultaneously. Supports complex filters (by parent, subtree, tags, age, etc.) or explicit ID lists. Always use _add_tags/_remove_tags for list fields.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs (flexible resolution)" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "type": { "type": "string", "description": "Filter by document type" },
                        "older_than_days": { "type": "integer", "description": "Filter: created > N days ago" },
                        "stale_days": { "type": "integer", "description": "Filter: not modified in N days" },
                        "orphan": { "type": "boolean", "description": "Filter: no parent and no project" },
                        "title_contains": { "type": "string", "description": "Filter: title substring (case-insensitive)" },
                        "weight_gte": { "type": "integer", "description": "Filter: downstream weight >= N" },
                        "updates": { "type": "object", "description": "Fields to set (null to remove). Special keys: _add_tags, _remove_tags, _add_depends_on, _remove_depends_on. A `priority` key is rejected outright — priority bands are Nic's call only, never an agent's." },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            )
            .with_title("Batch Update Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "batch_reparent",
                "Bulk move tasks to a new parent node. Use for major restructuring, such as grouping flat tasks into a new epic.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "title_contains": { "type": "string", "description": "Filter: title substring" },
                        "new_parent": { "type": "string", "description": "ID of new parent (flexible resolution)" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["new_parent"]
                }))
                .unwrap(),
            )
            .with_title("Batch Reparent Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "batch_archive",
                "Bulk archive tasks by setting status to 'done'. Use for closing out entire subtrees or stale tasks. Dry-run by default.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "priority": { "type": "integer", "description": "Filter by exact priority" },
                        "priority_gte": { "type": "integer", "description": "Filter: priority >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "older_than_days": { "type": "integer", "description": "Filter: created > N days ago" },
                        "stale_days": { "type": "integer", "description": "Filter: not modified in N days" },
                        "title_contains": { "type": "string", "description": "Filter: title substring" },
                        "reason": { "type": "string", "description": "Archive reason (appended to task body)" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("Batch Archive Tasks")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "get_semantic_neighbors",
                "Find nodes semantically similar to a given node based on vector proximity of embeddings. Returns a list of nodes that are related by content even if not explicitly linked in the graph.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Node ID, task ID, filename stem, or title (flexible resolution)" },
                        "threshold": { "type": "number", "description": "Cosine similarity threshold (0.0-1.0, default: 0.85). Higher is more restrictive." },
                        "limit": { "type": "integer", "description": "Maximum number of neighbors to return (default: 10)." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Find Semantic Neighbors")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "detect_weight_divergence",
                "Detect 'contributes_to' edges with high stated weight but zero interaction on the source task. Captures drift where importance is high but activity is zero.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "threshold_days": { "type": "integer", "description": "Staleness threshold in days (default: 14)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("Detect Weight Divergence")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "graph_stats",
                "Get a summary of PKB health, including task distribution by status/priority, orphan counts, and disconnected clusters. Read-only.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                    }
                }))
                .unwrap(),
            )
            .with_title("Graph Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "graph_json",
                "Export the full knowledge graph as JSON. Use for external visualization or deep structural analysis.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Knowledge Graph JSON")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "graph_excalidraw",
                "Generates an Excalidraw JSON diagram for an ego neighborhood around node_id or the full knowledge graph. Returns valid Excalidraw V2 JSON.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Focus node ID, filename, or title (uses flexible resolution). If omitted, exports the entire graph." },
                        "hops": { "type": "integer", "description": "Neighborhood depth in hops (1 to 5, default: 2)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("Export Graph to Excalidraw")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "diff_excalidraw",
                "Parses an Excalidraw JSON string and returns a structured diff against live PKB. Identifies added/updated/removed nodes, edge mutations, and visual styling changes.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canvas": { "type": "string", "description": "Excalidraw JSON string representation of the canvas" },
                        "base": { "type": "string", "description": "Optional base snapshot JSON string for 3-way conflict detection" }
                    },
                    "required": ["canvas"]
                }))
                .unwrap(),
            )
            .with_title("Diff Excalidraw Canvas")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "sync_excalidraw",
                "Applies non-destructive modifications from Excalidraw JSON back to PKB markdown files. Safely handles card additions, frontmatter updates, and edge connections.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canvas": { "type": "string", "description": "Excalidraw JSON string representation of the canvas" },
                        "base": { "type": "string", "description": "Optional base snapshot JSON string for 3-way diff" },
                        "dry_run": { "type": "boolean", "description": "Preview modifications without writing to disk (default: false)" },
                        "sync_edge_removals": { "type": "boolean", "description": "If true, removes deleted dependency edges from frontmatter depends_on (default: false)" }
                    },
                    "required": ["canvas"]
                }))
                .unwrap(),
            )
            .with_title("Sync Excalidraw Canvas to PKB")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "task_summary",
                "Get high-level dashboard metrics: counts of 'ready' vs 'blocked' tasks, and priority breakdowns. Use for situational awareness.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Task Summary Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            // ── Phase 2: Deduplication & Restructuring ────────────────────
            Tool::new(
                "find_duplicates",
                "Identify potential duplicate tasks using both semantic and title similarity. Returns clusters with a suggested canonical task. Read-only.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "description": "Filter by status" },
                        "mode": { "type": "string", "description": "Detection mode: title, semantic, or both (default: both)" },
                        "title_threshold": { "type": "number", "description": "Title similarity threshold 0-1 (default: 0.7)" },
                        "similarity_threshold": { "type": "number", "description": "Semantic similarity threshold 0-1 (default: 0.85)" },
                        "limit": { "type": "integer", "description": "Max duplicate clusters to return (default: 100). `total_clusters`/`total_duplicates` in the response still report the true totals." }
                    }
                }))
                .unwrap(),
            )
            .with_title("Find Duplicate Tasks")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "batch_merge",
                "Merge multiple duplicate tasks into a single canonical task. Archives duplicates and redirects dependencies. Idempotent.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canonical": { "type": "string", "description": "ID of the task to keep" },
                        "merge_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of duplicates to merge into canonical" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["canonical", "merge_ids"]
                }))
                .unwrap(),
            )
            .with_title("Batch Merge Tasks")
            .with_annotations(ToolAnnotations::new().destructive(true).idempotent(true)),
            Tool::new(
                "merge_node",
                "Merge source knowledge nodes into a canonical node. Performs a deep merge of all references (wikilinks, parents, etc.) across the entire PKB. Destructive for source nodes (archived).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canonical_id": { "type": "string", "description": "ID of the node to merge into (must already exist)" },
                        "source_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of nodes to merge into canonical and archive" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["canonical_id", "source_ids"]
                }))
                .unwrap(),
            )
            .with_title("Merge Knowledge Node")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "batch_create_epics",
                "Group flat tasks into new epic containers. Use to organize a scattered list of tasks into coherent, parented groups.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent": { "type": "string", "description": "Parent for all new epics" },
                        "epics": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string", "description": "Epic title (required)" },
                                    "id": { "type": "string", "description": "Optional custom epic ID (auto-generated if omitted)" },
                                    "priority": { "type": "integer", "description": "Priority band 0-4 (P0 Critical .. P4 Backlog)" },
                                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Existing task IDs to reparent under this new epic (required; unresolvable IDs are reported as errors, not fatal)" },
                                    "depends_on": { "type": "array", "items": { "type": "string" }, "description": "IDs the epic itself depends on" },
                                    "body": { "type": "string", "description": "Markdown body for the epic" }
                                },
                                "required": ["title", "task_ids"]
                            },
                            "description": "Array of epic definitions with title and task_ids to reparent"
                        },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["epics"]
                }))
                .unwrap(),
            )
            .with_title("Batch Create Epics")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "batch_reclassify",
                "Correct the type field for multiple documents and move them to their appropriate subdirectories. Idempotent.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "type": { "type": "string", "description": "Filter by current type" },
                        "title_contains": { "type": "string", "description": "Filter by title substring" },
                        "new_type": { "type": "string", "description": "New document type (task, memory, note, knowledge, epic, goal)" },
                        "dry_run": { "type": "boolean", "description": "Preview changes without writing (default: false)" }
                    },
                    "required": ["new_type"]
                }))
                .unwrap(),
            )
            .with_title("Batch Reclassify Types")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "get_stats",
                "Show MCP tool usage telemetry — call counts and response bytes per tool.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Tool Usage Stats")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "status",
                "Report server operational diagnostics and build identity: document count, last reindex timestamp and outcome, reindex/upsert queue depth, write locks / in-flight operations, and vector-store freshness.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Server Status & Diagnostics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "refresh_graph",
                "Synchronously rebuild the in-memory graph index from on-disk state. \
                 Refreshes parent→child relations, edges, and derived metrics without re-embedding. \
                 Use when graph queries return stale results after bulk mutations (e.g. batch_reparent). \
                 Returns the number of nodes loaded. This is a cheap operation — no ONNX/embedding step.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Refresh Graph Index")
            .with_annotations(ToolAnnotations::new().read_only(false)),
        ]
    }
}



#[cfg(test)]
mod annotation_tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_annotations_audit() {
        let tools = PkbSearchServer::get_all_tools();

        for tool in tools {
            // 1. Every tool must have a title
            assert!(
                tool.title.is_some(),
                "Tool '{}' is missing a human-readable title. Use .with_title()",
                tool.name
            );

            // 2. Safety hints check
            // Note: Creation tools (create_*) don't require read_only or destructive hints
            // but they still need a title.
            let annotations = tool.annotations.as_ref();
            let is_creation = tool.name.starts_with("create")
                || tool.name.contains("decompose")
                || tool.name.contains("create_epics");

            if !is_creation {
                let has_hint = annotations
                    .map(|a| a.read_only_hint.is_some() || a.destructive_hint.is_some())
                    .unwrap_or(false);
                // Some tools like 'append', 'complete_task', 'release_task' are writes but NOT destructive nor read-only.
                // The requirement says "read-only OR destructive OR neither explicitly".
                // I'll check that if it's NOT one of those known write tools, it should probably have a hint.
                let is_known_write = [
                    "append",
                    "add_observations",
                    "delete_observations",
                    "claim_task",
                    "complete_task",
                    "release_task",
                    "update_task",
                    "update_body",
                    "edit_body",
                    "edit",
                    "batch_update",
                    "batch_reparent",
                    "batch_merge",
                    "batch_reclassify",
                ]
                .contains(&&*tool.name);

                if !is_known_write {
                    assert!(
                        has_hint,
                        "Tool '{}' should have a safety hint (readOnlyHint or destructiveHint).",
                        tool.name
                    );
                }
            }

            // 3. Contradictory hints check
            if let Some(ann) = annotations {
                assert!(
                    !(ann.read_only_hint == Some(true) && ann.destructive_hint == Some(true)),
                    "Tool '{}' has contradictory hints (read-only AND destructive)",
                    tool.name
                );
            }
        }
    }

    #[tokio::test]
    async fn test_no_top_level_composition_keywords() {
        // Anthropic rejects tool schemas that have anyOf/allOf/oneOf at the top level of
        // inputSchema. They are only allowed nested inside a property.
        let tools = PkbSearchServer::get_all_tools();
        for tool in &tools {
            let schema = serde_json::to_value(&tool.input_schema).unwrap();
            for keyword in ["anyOf", "allOf", "oneOf"] {
                assert!(
                    !schema.as_object().map(|o| o.contains_key(keyword)).unwrap_or(false),
                    "Tool '{}' has '{}' at the top level of inputSchema — Anthropic rejects this (HTTP 400). Nest it inside a property instead.",
                    tool.name,
                    keyword,
                );
            }
        }
    }

    /// Full embedding reindex (pkb reindex --force) must remain CLI-only.
    /// No MCP tool name may suggest a full reindex path.
    #[test]
    fn test_no_full_reindex_tool_on_mcp_surface() {
        let tools = PkbSearchServer::get_all_tools();
        let suspect: Vec<String> = tools
            .iter()
            .filter(|t| {
                t.name.contains("reindex")
                    || t.name.contains("force_index")
                    || t.name.contains("embed_all")
                    || t.name.contains("full_index")
            })
            .map(|t| t.name.to_string())
            .collect();
        assert!(
            suspect.is_empty(),
            "Full reindex must remain CLI-only (pkb reindex --force). \
             These MCP tool names look like full reindex paths: {suspect:?}",
        );
    }
}


