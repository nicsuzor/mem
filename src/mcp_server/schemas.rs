use rmcp::model::*;

use super::{PkbSearchServer, GOAL_TYPE_ENUM};

impl PkbSearchServer {
    fn attach_output_schemas(tools: &mut [Tool]) {
        for tool in tools {
            if tool.output_schema.is_none() {
                tool.output_schema = Some(std::sync::Arc::new(
                    serde_json::from_value(Self::output_schema_for_tool(&tool.name)).unwrap(),
                ));
            }
        }
    }

    pub fn get_published_tools() -> Vec<Tool> {
        let mut tools = vec![
            Tool::new(
                "search",
                "Hybrid semantic + graph search across the PKB using ONNX embeddings (~100-300ms). If search times out under load, back off and retry. Supports proximity boosting.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural language query" },
                        "limit": { "type": "integer" },
                        "type": { "type": "string" },
                        "actionable_only": { "type": "boolean" },
                        "include_done": { "type": "boolean", "description": "Include done/cancelled items" },
                        "since": { "type": "string" },
                        "before": { "type": "string" },
                        "boost_id": { "type": "string" },
                        "detail": { "type": "string", "enum": ["chunk", "snippet", "full", "metadata", "ids"], "default": "chunk" },
                        "ids_only": { "type": "boolean" },
                        "max_bytes": { "type": "integer" },
                        "format": { "type": "string", "enum": ["markdown", "json"] }
                    },
                    "required": ["query"]
                }))
                .unwrap(),
            )
            .with_title("Hybrid Semantic Search")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_document",
                "Read contents or metadata of a PKB document by ID, filename stem, or title. Supports batch reads via ids.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "ids": { "type": "array", "items": { "type": "string" } },
                        "detail": { "type": "string", "enum": ["metadata", "summary", "full"] },
                        "budget_ms": { "type": "integer" },
                        "max_bytes": { "type": "integer" },
                        "metadata_only": { "type": "boolean" },
                        "format": { "type": "string", "enum": ["markdown", "json"] }
                    }
                }))
                .unwrap(),
            )
            .with_title("Get Document Content")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_task",
                "Retrieve full task details, metadata, body, and graph context. Ranking figures come from the flow engine: `flow` carries gain, loss_averted, decision_value, flow_status and per-target stake; `display` carries ready, blocked, on_cliff, cliff_enters_on, deadline_class and days_until_due. With include_routes (default true), `routes` lists the paths to each served target, with `routes_truncated` and a one-sentence `explanation`.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "ids": { "type": "array", "items": { "type": "string" } },
                        "detail": { "type": "string", "enum": ["metadata", "summary", "full"] },
                        "budget_ms": { "type": "integer" },
                        "max_bytes": { "type": "integer" },
                        "metadata_only": { "type": "boolean" },
                        "include_routes": { "type": "boolean" },
                        "fields": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ]
                        }
                    }
                }))
                .unwrap(),
            )
            .with_title("Get Task Detail")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_documents",
                "List indexed documents with optional filters and pagination.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": { "type": "string" },
                        "tags": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ]
                        },
                        "type": { "type": "string" },
                        "status": { "type": "string" },
                        "limit": { "type": "integer" },
                        "offset": { "type": "integer" },
                        "budget_ms": { "type": "integer" },
                        "format": { "type": "string", "enum": ["markdown", "json"] }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Documents")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "list_tasks",
                "List tasks with smart filtering, in flow display order: the hard-deadline cliff lane first, then tasks carrying flow figures by gain + loss_averted descending. `sort` picks another key. JSON rows carry gain, loss_averted, decision_value, flow_status and on_cliff.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "status": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ],
                            "description": "Stored status: inbox, ready, queued, in_progress, review, done, etc."
                        },
                        "intent": { "type": "integer" },
                        "severity": { "type": "integer" },
                        "goal_type": { "type": "string" },
                        "assignee": { "type": "string" },
                        "type": { "type": "string" },
                        "title_contains": { "type": "string" },
                        "gain_gte": { "type": "number" },
                        "loss_averted_gte": { "type": "number" },
                        "sort": {
                            "type": "string",
                            "enum": ["default", "gain", "loss_averted", "decision_value", "gain_per_effort", "loss_averted_per_effort", "due", "id"]
                        },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "has_superseded_by": { "type": "boolean" },
                        "since": { "type": "string" },
                        "before": { "type": "string" },
                        "limit": { "type": "integer" },
                        "include_subtasks": { "type": "boolean" },
                        "include_done": { "type": "boolean", "description": "Include done and cancelled tasks (default: false)" },
                        "budget_ms": { "type": "integer" },
                        "fields": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ]
                        },
                        "nested": { "type": "boolean" },
                        "view": { "type": "string", "enum": ["flat", "tree"] },
                        "format": {
                            "type": "string",
                            "enum": ["markdown", "json", "tree", "ascii_tree", "nested_json", "json_tree"]
                        }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Tasks")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "create",
                "Create a doc; dir defaults by type (template: `[<project>/]workflows/<snake_id>.md`). Requires a slug (<= 30 chars) used to construct the filename.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "type": {
                            "type": "string",
                            "enum": ["task", "learn", "pr", "template", "target", "note", "knowledge", "memory", "insight", "observation", "contact", "document", "reference", "review", "case", "spec", "prototype", "index", "daily"]
                        },
                        "slug": { "type": "string", "maxLength": 30, "description": "Short filename slug (required, <= 30 characters). Used to construct the node filename." },
                        "id": { "type": "string" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "body": { "type": "string", "description": "Markdown body (wikilinks supported; host paths rejected)" },
                        "status": { "type": "string", "enum": ["inbox", "ready", "failed"] },
                        "intent": { "type": "integer", "description": "Intent band 0-4 (P0 Critical .. P4 Backlog). Default when unset: P4 (Backlog)." },
                        "parent": { "type": "string" },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "soft_depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string", "description": "Complexity size label (e.g. S, M, L; use effort for duration strings)" },
                        "effort": { "type": "string", "description": "Effort duration string: 1d, 2h, 1w (minimum 1h; use complexity for size labels)" },
                        "consequence": { "type": "string" },
                        "contributes_to": { "type": "array", "items": { "type": "object" } },
                        "source": { "type": "string" },
                        "due": { "type": "string" },
                        "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "supersedes": { "type": "string" },
                        "severity": { "type": "integer" },
                        "goal_type": { "type": "string", "enum": GOAL_TYPE_ENUM },
                        "dir": { "type": "string" },
                        "filename": { "type": "string" },
                        "project": { "type": "string" },
                        "stakeholder": { "type": "string" },
                        "waiting_since": { "type": "string" },
                        "links": { "type": "array", "items": { "type": "object" } },
                        "worth": { "anyOf": [{ "type": "number" }, { "type": "string" }] },
                        "deadline_class": { "type": "string", "enum": ["fake", "soft", "hard"] }
                    },
                    "required": ["title", "type", "slug"]
                }))
                .unwrap(),
            )
            .with_title("Create Document")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "create_task",
                "Create a new task markdown file with YAML frontmatter. Requires a slug (<= 30 chars) used to construct the filename. Parent is optional (omitted tasks are root tasks). Project inherits from ancestor if omitted.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Task title (required, alias task_title)" },
                        "slug": { "type": "string", "maxLength": 30, "description": "Short filename slug (required, <= 30 characters). Used to construct the task filename." },
                        "task_title": { "type": "string" },
                        "id": { "type": "string" },
                        "parent": { "type": "string" },
                        "intent": { "type": "integer", "description": "Intent band 0-4 (P0 Critical .. P4 Backlog). Default when unset: P4 (Backlog)." },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "soft_depends_on": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string", "description": "Complexity size label (e.g. S, M, L; use effort for duration strings)" },
                        "effort": { "type": "string", "description": "Effort duration string: 1d, 2h, 1w (minimum 1h; use complexity for size labels)" },
                        "consequence": { "type": "string" },
                        "severity": { "type": "integer" },
                        "goal_type": { "type": "string", "enum": GOAL_TYPE_ENUM },
                        "classification": { "type": "string" },
                        "body": { "type": "string", "description": "Markdown body (wikilinks supported; host paths rejected)" },
                        "stakeholder": { "type": "string" },
                        "waiting_since": { "type": "string" },
                        "due": { "type": "string" },
                        "project": { "type": "string" },
                        "filename": { "type": "string" },
                        "type": { "type": "string", "enum": ["task", "learn", "pr", "target"] },
                        "status": { "type": "string", "enum": ["inbox", "ready", "failed"] },
                        "allow_missing_parent": { "type": "boolean" },
                        "force": { "type": "boolean" },
                        "session_id": { "type": "string" },
                        "issue_url": { "type": "string" },
                        "release_summary": { "type": "string" },
                        "contributes_to": { "type": "array", "items": { "type": "object" } },
                        "links": { "type": "array", "items": { "type": "object" } },
                        "worth": { "anyOf": [{ "type": "number" }, { "type": "string" }] },
                        "deadline_class": { "type": "string", "enum": ["fake", "soft", "hard"] }
                    },
                    "required": ["title", "slug"]
                }))
                .unwrap(),
            )
            .with_title("Create Task")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "update_task",
                "Patch metadata fields on an existing task (flat or nested under updates). Supports status, type, intent, and severity. Requires completion_evidence when setting status=done. For state transitions, prefer release_task.",
                serde_json::from_str::<JsonObject>(r#"{
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "updates": { "type": "object" },
                        "status": {
                            "type": "string",
                            "enum": ["inbox", "ready", "queued", "in_progress", "review", "done", "paused", "someday", "cancelled", "partial", "failed"]
                        },
                        "type": {
                            "type": "string",
                            "enum": ["task", "learn", "pr", "template", "target", "note", "knowledge", "memory", "insight", "observation", "contact", "document", "reference", "review", "case", "spec", "prototype", "index", "daily", "session-log", "audit-report"]
                        },
                        "intent": { "type": "integer" },
                        "priority": { "type": "integer" },
                        "severity": { "type": "integer" },
                        "title": { "type": "string" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "parent": { "type": "string" },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "soft_depends_on": { "type": "array", "items": { "type": "string" } },
                        "blocks": { "type": "array", "items": { "type": "string" } },
                        "soft_blocks": { "type": "array", "items": { "type": "string" } },
                        "assignee": { "type": "string" },
                        "complexity": { "type": "string" },
                        "effort": { "type": "string" },
                        "consequence": { "type": "string" },
                        "goal_type": { "type": "string", "enum": ["committed", "aspirational", "learning"] },
                        "body": { "type": "string" },
                        "content": { "type": "string" },
                        "stakeholder": { "type": "string" },
                        "waiting_since": { "type": "string" },
                        "due": { "type": "string" },
                        "project": { "type": "string" },
                        "session_id": { "type": "string" },
                        "issue_url": { "type": "string" },
                        "release_summary": { "type": "string" },
                        "released_at": { "type": "string" },
                        "contributes_to": { "type": "array", "items": { "type": "object" } },
                        "supersedes": { "type": "string" },
                        "superseded_by": { "type": "array", "items": { "type": "string" } },
                        "blocked": { "type": "boolean" },
                        "blocker": { "type": "string" },
                        "reason": { "type": "string" },
                        "completion_evidence": { "type": "string", "description": "Required when setting status=done" },
                        "evidence": { "type": "string" },
                        "pr_url": { "type": "string" },
                        "_add_tags": { "type": "array", "items": { "type": "string" } },
                        "_remove_tags": { "type": "array", "items": { "type": "string" } },
                        "_add_depends_on": { "type": "array", "items": { "type": "string" } },
                        "_remove_depends_on": { "type": "array", "items": { "type": "string" } },
                        "classification": { "type": "string" },
                        "links": { "type": "array", "items": { "type": "object" } },
                        "worth": { "anyOf": [{ "type": "number" }, { "type": "string" }] },
                        "deadline_class": { "type": "string", "enum": ["fake", "soft", "hard"] },
                        "branch": { "type": "string" },
                        "container": { "type": "string" },
                        "alias": { "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }] },
                        "unparent": { "type": "boolean" },
                        "allow_missing_parent": { "type": "boolean" },
                        "force": { "type": "boolean" },
                        "recursive": { "type": "boolean" }
                    },
                    "required": ["id"]
                }"#)
                .unwrap(),
            )
            .with_title("Update Task")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
            Tool::new(
                "update_body",
                "Atomically rewrite markdown body of a document. Preserves frontmatter and updates modified date. Supports compare-and-swap via expected_modified.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "minLength": 1 },
                        "new_body": { "type": "string", "minLength": 1, "description": "New markdown body (wikilinks supported; host paths rejected)" },
                        "preserve_frontmatter": { "type": "boolean" },
                        "expected_modified": { "type": "string" }
                    },
                    "required": ["id", "new_body"]
                }))
                .unwrap(),
            )
            .with_title("Rewrite Document Body")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "edit_body",
                "Apply unified-diff hunks to document body. Preserves frontmatter and updates modified date. Supports compare-and-swap via expected_modified.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "diff": { "type": "string", "description": "Unified-diff hunks (host paths rejected)" },
                        "preserve_frontmatter": { "type": "boolean" },
                        "dry_run": { "type": "boolean" },
                        "expected_modified": { "type": "string" }
                    },
                    "required": ["id", "diff"]
                }))
                .unwrap(),
            )
            .with_title("Edit Document Body (Unified Diff)")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "append",
                "Append timestamped content to an existing document by ID or section. Supports compare-and-swap via expected_modified.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "content": { "type": "string", "description": "Content to append (wikilinks supported; host paths rejected)" },
                        "section": { "type": "string" },
                        "expected_modified": { "type": "string" }
                    },
                    "required": ["id", "content"]
                }))
                .unwrap(),
            )
            .with_title("Append to Document")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "delete",
                "Permanently delete a document by ID and remove from search index.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "type": { "type": "string" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Delete Document")
            .with_annotations(ToolAnnotations::new().destructive(true)),
            Tool::new(
                "convert_document",
                "Retype doc (keeps ID), move to `<dir>/<id>_<slug>.md` (template: `[<project>/]workflows/<snake_id>.md`), reindex.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Document ID, stem, or title" },
                        "type": { "type": "string", "description": "Target type (e.g. 'task')" },
                        "dir": { "type": "string", "description": "Target subdir (default by type)" },
                        "status": { "type": "string", "description": "Status to set (default 'inbox' for tasks)" }
                    },
                    "required": ["id", "type"]
                }))
                .unwrap(),
            )
            .with_title("Convert Document")
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(false)),
            Tool::new(
                "claim_task",
                "Claim a task or instantiate a template. Tasks are claimed in place (status: in_progress). Templates instantiate a datestamped copy.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "assignee": { "type": "string" },
                        "session_id": { "type": "string" }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Claim Template")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "release_task",
                "Release a task to a terminal or handoff status (done, review, blocked, cancelled, partial, failed). Evidence-or-failure-reason contract: `summary` (or `completion_evidence`) is always required; releasing to done requires all acceptance checklist items in the task body to be ticked (- [x]); releasing to blocked/cancelled/review/partial/failed additionally requires a non-empty `reason` (or `blocker`, for `blocked`) — a handback with neither is rejected.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID (flexible resolution: ID, filename stem, or title). If omitted, an ad-hoc task is created." },
                        "project": { "type": "string", "description": "Project routing slug (required if id omitted)" },
                        "status": {
                            "type": "string",
                            "enum": ["done", "review", "cancelled", "partial", "failed"],
                            "description": "Target status"
                        },
                        "summary": { "type": "string", "description": "What was done and outcome (or provide completion_evidence)." },
                        "completion_evidence": { "type": "string", "description": "Alias for summary: what was done and outcome (required when status is done). Releasing as done requires all acceptance checklist items to be checked (- [x])." },
                        "pr_url": { "type": "string", "description": "Pull request or commit URL" },
                        "branch": { "type": "string", "description": "Git branch name (optional)" },
                        "blocker": { "type": "string", "description": "Required when status=blocked (or provide reason)" },
                        "reason": { "type": "string", "description": "Why the task didn't ship cleanly. Required (non-empty) when status is blocked/cancelled/review/partial/failed (for status=blocked, `blocker` may be given instead)." },
                        "session_id": { "type": "string", "description": "Active session ID" },
                        "issue_url": { "type": "string", "description": "External issue/ticket URL" },
                        "release_summary": { "type": "string", "description": "Detailed technical summary for the release. Warning if > 500 chars." },
                        "body": { "type": "string", "description": "Markdown body for ad-hoc tasks (wikilinks supported; host paths rejected)" },
                        "recursive": { "type": "boolean", "description": "Cascade-close open descendant tasks when releasing as done. Default: false." }
                    },
                    "required": ["status", "summary"]
                }))
                .unwrap(),
            )
            .with_title("Release Task")
            .with_annotations(ToolAnnotations::new().read_only(false).idempotent(true)),
            Tool::new(
                "decompose_task",
                "Split a parent task into subtasks in one call. Supports positional dependency references () and sibling title matching.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent_id": { "type": "string" },
                        "subtasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "id": { "type": "string" },
                                    "intent": { "type": "integer", "description": "Intent band 0-4 (P0 Critical .. P4 Backlog). Default when unset: P4 (Backlog)." },
                                    "depends_on": { "type": "array", "items": { "type": "string" } },
                                    "tags": { "type": "array", "items": { "type": "string" } },
                                    "assignee": { "type": "string" },
                                    "complexity": { "type": "string" },
                                    "body": { "type": "string", "description": "Markdown body (wikilinks supported; host paths rejected)" },
                                    "stakeholder": { "type": "string" },
                                    "waiting_since": { "type": "string" },
                                    "consequence": { "type": "string" },
                                    "severity": { "type": "integer" },
                                    "goal_type": { "type": "string" },
                                    "project": { "type": "string" }
                                },
                                "required": ["title"]
                            }
                        }
                    },
                    "required": ["parent_id", "subtasks"]
                }))
                .unwrap(),
            )
            .with_title("Decompose Task")
            .with_annotations(ToolAnnotations::new().read_only(false)),
            Tool::new(
                "status",
                "Report server operational diagnostics: document count, reindex queue depth, write locks, and vector-store freshness.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            )
            .with_title("Server Status & Diagnostics")
            .with_annotations(ToolAnnotations::new().read_only(true))
        ];
        Self::attach_output_schemas(&mut tools);
        tools
    }

    pub fn get_retired_tools() -> Vec<Tool> {
        let mut tools = vec![
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
                "top_n_by_metric",
                "Return the top N nodes ranked by a specified centrality metric (pagerank, betweenness, degree), or full network metrics for a single node via 'id'. Supersedes get_network_metrics.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "metric": {
                            "type": "string",
                            "enum": ["pagerank", "betweenness", "degree", "gain", "loss_averted", "decision_value"],
                            "description": "Centrality or flow metric to rank by (pagerank, betweenness, degree, gain, loss_averted, decision_value). Required unless 'id' is supplied."
                        },
                        "id": {
                            "type": "string",
                            "description": "Optional: node ID to compute full centrality metrics for a single node."
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
                    }
                }))
                .unwrap(),
            )
            .with_title("Top N Nodes by Centrality Metric")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "nested_tasks",
                "List tasks in a hierarchical tree view grouped by project and epic (metadata only), mirroring the `pkb tasks` CLI command. Supports ASCII tree or brief JSON output. Ideal for understanding work hierarchy and structure without heavy body text.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter by project slug or any polecat.yaml alias (case-insensitive)." },
                        "status": {
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ],
                            "description": "Filter by task status (e.g. 'ready', 'blocked', 'in_progress')."
                        },
                        "intent": { "type": "integer", "description": "Filter to tasks whose intent ≤ N." },
                        "severity": { "type": "integer", "description": "Filter by exact severity" },
                        "goal_type": { "type": "string", "description": "Filter by goal type" },
                        "assignee": { "type": "string", "description": "Filter by assignee" },
                        "type": { "type": "string", "description": "Filter by document type (e.g. 'task', 'epic')" },
                        "title_contains": { "type": "string", "description": "Filter by title substring (case-insensitive)" },
                        "complexity": { "type": "string", "description": "Filter by complexity (e.g. 'low', 'medium', 'high')" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter by tags (all must match)." },
                        "since": { "type": "string", "description": "Filter: return only tasks whose modified date is on or after YYYY-MM-DD or RFC 3339 timestamp." },
                        "before": { "type": "string", "description": "Filter: return only tasks whose modified date is on or before YYYY-MM-DD or RFC 3339 timestamp." },
                        "limit": { "type": "integer", "description": "Max results (default: 50)" },
                        "include_subtasks": { "type": "boolean", "description": "Include sub-tasks (type=subtask). Default: false." },
                        "include_done": { "type": "boolean", "description": "Include done and cancelled tasks. Default: false." },
                        "fields": {
                            "description": "Optional: return only specific fields per task in JSON mode to reduce token usage. Comma-separated string or array of field names (e.g. ['id', 'title', 'status']).",
                            "anyOf": [
                                { "type": "string" },
                                { "type": "array", "items": { "type": "string" } }
                            ]
                        },
                        "format": {
                            "type": "string",
                            "enum": ["ascii_tree", "tree", "json", "nested_json", "json_tree"],
                            "description": "Output format: 'ascii_tree' or 'tree' (default: 'ascii_tree') returns an ASCII tree formatted like `pkb tasks`; 'json', 'nested_json', or 'json_tree' returns hierarchical JSON with brief metadata only."
                        }
                    }
                }))
                .unwrap(),
            )
            .with_title("Nested Tasks Tree")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_dependency_tree",
                "Visualize the task dependency graph for a specific node. Upstream shows what the task depends on; downstream shows what it blocks; children lists child tasks (supersedes get_task_children).",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Task ID" },
                        "direction": { "type": "string", "enum": ["upstream", "downstream", "children"], "description": "Direction: 'upstream' (depends on, default), 'downstream' (blocks), or 'children' (child tasks in hierarchy)" },
                        "recursive": { "type": "boolean", "description": "When direction is 'children', include all descendants, not just direct children (default: false)." },
                        "max_depth": { "type": "integer", "description": "Maximum traversal depth in hops (default: 10)" },
                        "format": { "type": "string", "enum": ["markdown", "json"], "description": "Output format: 'markdown' (default) or structured 'json'." }
                    },
                    "required": ["id"]
                }))
                .unwrap(),
            )
            .with_title("Get Dependency Tree")
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
                "batch_update",
                "Apply frontmatter updates to multiple tasks simultaneously. Supports complex filters (by parent, subtree, tags, age, etc.) or explicit ID lists. Always use _add_tags/_remove_tags for list fields.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit task IDs (flexible resolution)" },
                        "parent": { "type": "string", "description": "Filter: direct children of parent" },
                        "subtree": { "type": "string", "description": "Filter: all descendants of node" },
                        "status": { "type": "string", "description": "Filter by status" },
                        "intent": { "type": "integer", "description": "Filter by exact intent" },
                        "intent_gte": { "type": "integer", "description": "Filter: intent >= N" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: has ALL listed tags" },
                        "type": { "type": "string", "description": "Filter by document type" },
                        "older_than_days": { "type": "integer", "description": "Filter: created > N days ago" },
                        "stale_days": { "type": "integer", "description": "Filter: not modified in N days" },
                        "orphan": { "type": "boolean", "description": "Filter: no parent and no project" },
                        "title_contains": { "type": "string", "description": "Filter: title substring (case-insensitive)" },
                        "updates": { "type": "object", "description": "Fields to set (null to remove). Special keys: _add_tags, _remove_tags, _add_depends_on, _remove_depends_on. Accepts `links`, `worth`, `deadline_class`. An `intent` or `priority` key is accepted under Nic's standing delegation to agents — set only from Nic's strategic context read across the graph, never a self-assessment of the batch's own importance, and never propagated from one node onto its siblings." },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["updates"]
                }))
                .unwrap(),
            )
            .with_title("Batch Update Tasks")
            .with_annotations(ToolAnnotations::new().idempotent(true)),
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
                "graph_excalidraw",
                "Generates an Excalidraw JSON diagram for an ego neighborhood around node_id, or the union of the top-10 focus picks' ego neighborhoods when node_id is omitted. Returns valid Excalidraw V2 JSON.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Focus node ID, filename, or title (uses flexible resolution). If omitted, exports the union of the top-10 focus picks' ego neighborhoods." },
                        "hops": { "type": "integer", "description": "Neighborhood depth in hops (1 to 5, default: 2)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("Export Graph to Excalidraw")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "export_graph",
                "Export the knowledge/task graph as GraphViz DOT syntax (`digraph PKB { ... }`) or structured JSON for external rendering (`dot -Tsvg`, `neato`, `sfdp`), deep structural analysis, or visualization dashboards. Unlike graph_excalidraw (round-trip interactive canvas), this is a static, one-way export. Nodes carry id/title/status/type; edges cover depends_on, contributes_to, supersedes, closes, and parent-child hierarchy (blocks: frontmatter is compiled into depends_on edges, so it needs no separate edge type). Labels are escaped for safe DOT syntax.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "format": {
                            "type": "string",
                            "enum": ["dot", "json"],
                            "description": "Output format: 'dot' (GraphViz DOT digraph syntax, default) or 'json' (structured graph JSON with nodes, edges, ready, blocked, roots, and cliff; each node carries its flow figures, display metadata and display_rank)."
                        },
                        "focus": { "type": "string", "description": "Focus node ID, filename, or title (flexible resolution). If omitted, exports the full active graph." },
                        "max_depth": { "type": "integer", "description": "Traversal depth in hops from focus (1 to 5, default: 2). Ignored if focus is omitted." },
                        "project": { "type": "string", "description": "Filter to nodes whose project field matches exactly." },
                        "include_done": { "type": "boolean", "description": "Include done/cancelled nodes (default: false)." }
                    }
                }))
                .unwrap(),
            )
            .with_title("Export Graph")
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
                "list_excalidraw",
                "List the raw .excalidraw canvas files stored in the PKB, by PKB-relative path, with size and modified time. Canvases are files, not graph nodes: they are not indexed or searchable, so use this (not search/get_document) to find them, then get_excalidraw to read one.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "dir": { "type": "string", "description": "Only list canvases under this PKB-relative directory (e.g. 'knowledge/framework')." }
                    }
                }))
                .unwrap(),
            )
            .with_title("List Excalidraw Files")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "get_excalidraw",
                "Read a .excalidraw canvas file from the PKB verbatim, by PKB-relative path as returned by list_excalidraw. Returns the raw Excalidraw JSON.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "PKB-relative path ending in .excalidraw (e.g. 'knowledge/framework/academicops.excalidraw')." }
                    },
                    "required": ["path"]
                }))
                .unwrap(),
            )
            .with_title("Read Excalidraw File")
            .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new(
                "write_excalidraw",
                "Write a .excalidraw canvas file into the PKB at a PKB-relative path, creating it (and parent directories) or overwriting it. Content must be a valid Excalidraw scene; it is validated before an atomic write and is not indexed. This stores the file as-is; to push canvas edits into PKB notes use sync_excalidraw.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "PKB-relative path ending in .excalidraw." },
                        "content": { "type": "string", "description": "Full Excalidraw scene JSON (top-level type 'excalidraw' and an elements array)." }
                    },
                    "required": ["path", "content"]
                }))
                .unwrap(),
            )
            .with_title("Write Excalidraw File")
            .with_annotations(ToolAnnotations::new().read_only(false).destructive(true)),
            Tool::new(
                "task_summary",
                "Get high-level dashboard metrics: counts of 'ready' vs 'blocked' tasks, and intent breakdowns. Use for situational awareness.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Filter summary to a specific project slug or polecat.yaml alias (case-insensitive)" }
                    }
                }))
                .unwrap(),
            )
            .with_title("Task Summary Statistics")
            .with_annotations(ToolAnnotations::new().read_only(true)),
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
                "Merge multiple duplicate tasks or knowledge nodes into a single canonical task. Deletes merged duplicates outright (leaving no cancelled stub, per specs/pkb-rules.md §3.3); records the merge as `supersedes: [sources]` on the canonical. Unions tags and depends_on onto the canonical, reparents children and redirects all inbound references (parent/depends_on/soft_depends_on/blocks/soft_blocks/supersedes/closes/links). Source bodies are NOT carried across — write the consolidated body on the canonical yourself. A rejected merge (e.g. dependency cycle) writes nothing. Idempotent. Supersedes merge_node.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "canonical": { "type": "string", "description": "ID of the node to keep (alias: canonical_id)" },
                        "canonical_id": { "type": "string", "description": "Alias for canonical: ID of the node to keep" },
                        "merge_ids": { "type": "array", "items": { "type": "string" }, "description": "IDs of duplicates/source nodes to merge into canonical and delete (alias: source_ids)" },
                        "source_ids": { "type": "array", "items": { "type": "string" }, "description": "Alias for merge_ids: IDs of duplicate/source nodes to merge into canonical and delete" },
                        "dry_run": { "type": "boolean", "description": "Preview only (default: true — must explicitly set false to execute)" }
                    },
                    "required": ["canonical", "merge_ids"]
                }))
                .unwrap(),
            )
            .with_title("Batch Merge Tasks")
            .with_annotations(ToolAnnotations::new().destructive(true).idempotent(true)),
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
                                    "intent": { "type": "integer", "description": "Intent band 0-4 (P0 Critical .. P4 Backlog). Default when unset: P4 (Backlog)." },
                                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Existing task IDs to reparent under this new epic (required; unresolvable IDs are reported as errors, not fatal)" },
                                    "depends_on": { "type": "array", "items": { "type": "string" }, "description": "IDs the epic itself depends on" },
                                    "body": { "type": "string", "description": "Markdown body for the epic. Refer to other PKB files by wikilink (`[[id]]`) or PKB-root-relative path; a machine-specific path such as `~/brain/...` or `/home/nic/brain/...` is rejected with error_type `machine_specific_path`." }
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
            Tool::new(
                "repair_index_orphans",
                "Detect (and, on request, repair) orphaned semantic-index entries: ids returned by `search`/`search_by_tag` \
                 whose backing document no longer exists on disk. A deleted or moved document can leave a stale embedding \
                 behind — `refresh_graph` cannot fix this, since it rebuilds the graph from disk but never touches the \
                 vector index — so the ghost entry keeps resurfacing in search results with no `get_task`/`get_document` \
                 path that can ever reach it again. Call with dry_run=true (default) to list orphans without changing the \
                 index; call with dry_run=false to purge them.",
                serde_json::from_value::<JsonObject>(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "dry_run": {
                            "type": "boolean",
                            "description": "If true (default), only report orphaned entries — no index mutation. If false, purge every orphaned entry found.",
                            "default": true
                        }
                    }
                }))
                .unwrap(),
            )
            .with_title("Repair Orphaned Index Entries")
            .with_annotations(ToolAnnotations::new().read_only(false))
        ];
        Self::attach_output_schemas(&mut tools);
        tools
    }

    pub fn get_all_tools() -> Vec<Tool> {
        let mut tools = Self::get_published_tools();
        tools.extend(Self::get_retired_tools());
        tools
    }

    pub fn output_schema_for_tool(name: &str) -> serde_json::Value {
        match name {
            "get_consolidation_cluster" => serde_json::json!({
                "type": "object",
                "properties": {
                    "seed_id": { "type": "string" },
                    "nodes": { "type": "array", "items": { "type": "object" } }
                }
            }),
            "apply_consolidation_batch" | "batch_update" => serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string" },
                    "dry_run": { "type": "boolean" },
                    "total_matched": { "type": "integer" },
                    "total_modified": { "type": "integer" },
                    "total_skipped": { "type": "integer" },
                    "total_failed": { "type": "integer" },
                    "actions": { "type": "array", "items": { "type": "object" } },
                    "errors": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "search" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "count": { "type": "integer" },
                    "results": { "type": "array", "items": { "type": "object" } },
                    "ids": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "get_document" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "frontmatter": { "type": "object" },
                    "body": { "type": "string" },
                    "content": { "type": "string" },
                    "truncated": { "type": "boolean" },
                    "body_bytes": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "not_found": { "type": "array", "items": { "type": "string" } },
                    "timed_out": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "list_documents" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "documents": { "type": "array", "items": { "type": "object" } },
                    "timed_out": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "top_n_by_metric" => serde_json::json!({
                "type": "object",
                "properties": {
                    "metric": { "type": "string" },
                    "count": { "type": "integer" },
                    "nodes": { "type": "array", "items": { "type": "object" } },
                    "id": { "type": "string" },
                    "pagerank": { "type": "number" },
                    "betweenness": { "type": "number" },
                    "degree": { "type": "integer" }
                }
            }),
            "create_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "path": { "type": "string" },
                    "title": { "type": "string" },
                    "project": { "type": "string" },
                    "status": { "type": "string" },
                    "parent": { "type": "string" },
                    "message": { "type": "string" },
                    "modified": { "type": "string" }
                }
            }),
            "claim_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "assignee": { "type": "string" },
                    "session_id": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "frontmatter": { "type": "object" },
                    "body": { "type": "string" }
                }
            }),
            "create" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "path": { "type": "string" },
                    "title": { "type": "string" },
                    "type": { "type": "string" },
                    "status": { "type": "string" },
                    "message": { "type": "string" },
                    "modified": { "type": "string" }
                }
            }),
            "append" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "appended_bytes": { "type": "integer" }
                }
            }),
            "update_body" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "bytes_written": { "type": "integer" }
                }
            }),
            "edit_body" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "applied_hunks": { "type": "integer" },
                    "total_hunks": { "type": "integer" },
                    "dry_run": { "type": "boolean" }
                }
            }),
            "delete" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "deleted": { "type": "boolean" },
                    "message": { "type": "string" }
                }
            }),
            "convert_document" => serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "old_type": { "type": "string" },
                    "type": { "type": "string" },
                    "status": { "type": ["string", "null"] },
                    "old_path": { "type": "string" },
                    "path": { "type": "string" },
                    "retyped": { "type": "boolean" },
                    "moved": { "type": "boolean" }
                }
            }),
            "release_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "created_id": { "type": "string" },
                    "neighborhood": { "type": "object" }
                }
            }),
            "complete_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" },
                    "neighborhood": { "type": "object" }
                }
            }),
            "list_tasks" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "tasks": { "type": "array", "items": { "type": "object" } },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "timed_out": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "nested_tasks" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "projects": { "type": "object" },
                    "tasks": { "type": "array", "items": { "type": "object" } },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "tree": { "type": "string" }
                }
            }),
            "get_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "project": { "type": "string" },
                    "classification": { "type": "string" },
                    "frontmatter": { "type": "object" },
                    "depends_on": { "type": "array", "items": { "type": "object" } },
                    "blocks": { "type": "array", "items": { "type": "object" } },
                    "children": { "type": "array", "items": { "type": "object" } },
                    "subtasks": { "type": "array", "items": { "type": "object" } },
                    "parent": { "type": "object" },
                    "closes": { "type": "array", "items": { "type": "object" } },
                    "goals": { "type": "array", "items": { "type": "object" } },
                    "contributes_to": { "type": "array", "items": { "type": "object" } },
                    "contributed_by": { "type": "array", "items": { "type": "string" } },
                    "blocked": { "type": "boolean" },
                    "parse_warnings": { "type": "array", "items": { "type": "string" } },
                    "days_until_due": { "type": "integer" },
                    "links_out": { "type": "array", "items": { "type": "object" } },
                    "links_in": { "type": "array", "items": { "type": "object" } },
                    "flow": { "type": "object" },
                    "display": { "type": "object" },
                    "truncated": { "type": "boolean" },
                    "body_bytes": { "type": "integer" },
                    "routes": { "type": "object" },
                    "explanation": { "type": "string" },
                    "body": { "type": "string" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "not_found": { "type": "array", "items": { "type": "string" } },
                    "timed_out": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "update_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "modified": { "type": "string" },
                    "message": { "type": "string" }
                }
            }),
            "decompose_task" => serde_json::json!({
                "type": "object",
                "properties": {
                    "parent_id": { "type": "string" },
                    "subtasks": { "type": "array", "items": { "type": "object" } },
                    "created_ids": { "type": "array", "items": { "type": "string" } },
                    "message": { "type": "string" }
                }
            }),
            "get_dependency_tree" => serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "direction": { "type": "string" },
                    "depth": { "type": "integer" },
                    "nodes": { "type": "array", "items": { "type": "object" } },
                    "edges": { "type": "array", "items": { "type": "object" } },
                    "tree": { "type": "object" }
                }
            }),
            "pkb_trace" => serde_json::json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "object" } },
                    "found": { "type": "boolean" }
                }
            }),
            "detect_weight_divergence" => serde_json::json!({
                "type": "object",
                "properties": {
                    "divergences": { "type": "array", "items": { "type": "object" } },
                    "count": { "type": "integer" },
                    "threshold_days": { "type": "integer" }
                }
            }),
            "graph_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "type": { "type": "string" },
                    "version": { "type": "integer" },
                    "source": { "type": "string" },
                    "elements": { "type": "array", "items": { "type": "object" } },
                    "appState": { "type": "object" },
                    "files": { "type": "object" }
                }
            }),
            "export_graph" => serde_json::json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string" },
                    "nodes": { "type": "array", "items": { "type": "object" } },
                    "edges": { "type": "array", "items": { "type": "object" } },
                    "dot": { "type": "string" },
                    "ready": { "type": "array", "items": { "type": "string" } },
                    "blocked": { "type": "array", "items": { "type": "string" } },
                    "roots": { "type": "array", "items": { "type": "string" } },
                    "cliff": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "diff_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "added": { "type": "array", "items": { "type": "object" } },
                    "updated": { "type": "array", "items": { "type": "object" } },
                    "removed": { "type": "array", "items": { "type": "object" } },
                    "edge_mutations": { "type": "array", "items": { "type": "object" } },
                    "conflicts": { "type": "array", "items": { "type": "object" } }
                }
            }),
            "sync_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean" },
                    "applied": { "type": "array", "items": { "type": "object" } },
                    "errors": { "type": "array", "items": { "type": "string" } },
                    "message": { "type": "string" }
                }
            }),
            "list_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "canvases": { "type": "array", "items": { "type": "object" } },
                    "count": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "total": { "type": "integer" }
                }
            }),
            "get_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "scene": { "type": "object" },
                    "elements": { "type": "array", "items": { "type": "object" } },
                    "appState": { "type": "object" }
                }
            }),
            "write_excalidraw" => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "bytes_written": { "type": "integer" },
                    "message": { "type": "string" }
                }
            }),
            "task_summary" => serde_json::json!({
                "type": "object",
                "properties": {
                    "ready": { "type": "integer" },
                    "blocked": { "type": "integer" },
                    "total": { "type": "integer" },
                    "intent_breakdown": { "type": "object" },
                    "project": { "type": "string" }
                }
            }),
            "find_duplicates" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total_clusters": { "type": "integer" },
                    "total_duplicates": { "type": "integer" },
                    "clusters": { "type": "array", "items": { "type": "object" } }
                }
            }),
            "batch_merge" => serde_json::json!({
                "type": "object",
                "properties": {
                    "canonical": { "type": "string" },
                    "merged_ids": { "type": "array", "items": { "type": "string" } },
                    "dry_run": { "type": "boolean" },
                    "actions": { "type": "array", "items": { "type": "object" } },
                    "errors": { "type": "array", "items": { "type": "string" } },
                    "message": { "type": "string" }
                }
            }),
            "batch_create_epics" => serde_json::json!({
                "type": "object",
                "properties": {
                    "created_epics": { "type": "array", "items": { "type": "object" } },
                    "dry_run": { "type": "boolean" },
                    "message": { "type": "string" }
                }
            }),
            "get_stats" => serde_json::json!({
                "type": "object",
                "properties": {
                    "tools": { "type": "object" },
                    "total_calls": { "type": "integer" },
                    "total_response_bytes": { "type": "integer" }
                }
            }),
            "status" => serde_json::json!({
                "type": "object",
                "properties": {
                    "document_count": { "type": "integer" },
                    "last_reindex": { "type": "object" },
                    "is_fresh": { "type": "boolean" },
                    "stale_documents": { "type": "integer" },
                    "queue_depth": { "type": "integer" },
                    "version": { "type": "string" },
                    "git_commit": { "type": "string" }
                }
            }),
            "refresh_graph" => serde_json::json!({
                "type": "object",
                "properties": {
                    "nodes_loaded": { "type": "integer" },
                    "elapsed_ms": { "type": "number" },
                    "message": { "type": "string" },
                    "is_fresh": { "type": "boolean" }
                }
            }),
            "repair_index_orphans" => serde_json::json!({
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean" },
                    "orphans_found": { "type": "integer" },
                    "orphans_purged": { "type": "integer" },
                    "orphaned_ids": { "type": "array", "items": { "type": "string" } }
                }
            }),
            "task_search" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "tasks": { "type": "array", "items": { "type": "object" } }
                }
            }),
            "list_memories" | "retrieve_memory" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "memories": { "type": "array", "items": { "type": "object" } }
                }
            }),
            "search_by_tag" => serde_json::json!({
                "type": "object",
                "properties": {
                    "total": { "type": "integer" },
                    "showing": { "type": "integer" },
                    "items": { "type": "array", "items": { "type": "object" } },
                    "documents": { "type": "array", "items": { "type": "object" } }
                }
            }),
            _ => serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "message": { "type": "string" }
                }
            }),
        }
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
