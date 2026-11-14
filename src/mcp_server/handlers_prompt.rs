use rmcp::model::*;
use rmcp::ErrorData as McpError;
use std::borrow::Cow;

use super::PkbSearchServer;

fn get_required_str<'a>(
    args: &'a serde_json::Map<String, serde_json::Value>,
    param: &str,
) -> Result<&'a str, McpError> {
    let val = args.get(param).ok_or_else(|| McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(format!("Missing required parameter: {}", param)),
        data: None,
    })?;
    let s = val.as_str().ok_or_else(|| McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(format!("Parameter '{}' must be a string", param)),
        data: None,
    })?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Parameter '{}' cannot be empty", param)),
            data: None,
        });
    }
    Ok(trimmed)
}

fn get_optional_str<'a>(
    args: &'a serde_json::Map<String, serde_json::Value>,
    param: &str,
) -> Option<&'a str> {
    args.get(param)
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

fn get_optional_positive_int(
    args: &serde_json::Map<String, serde_json::Value>,
    param: &str,
    default_val: usize,
) -> Result<usize, McpError> {
    if let Some(val) = args.get(param) {
        if val.is_null() {
            return Ok(default_val);
        }
        let parsed = if let Some(n) = val.as_u64() {
            n as usize
        } else if let Some(s) = val.as_str() {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(default_val);
            }
            trimmed.parse::<usize>().map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Parameter '{}' must be a positive integer", param)),
                data: None,
            })?
        } else {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Parameter '{}' must be a positive integer", param)),
                data: None,
            });
        };

        if parsed == 0 {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Parameter '{}' must be greater than zero", param)),
                data: None,
            });
        }
        return Ok(parsed);
    }
    Ok(default_val)
}

fn prompt_arg(name: &str, description: &str, required: bool) -> PromptArgument {
    let mut arg = PromptArgument::new(name).with_description(description);
    if required {
        arg = arg.with_required(true);
    }
    arg
}

impl PkbSearchServer {
    pub(crate) fn handle_list_prompts(&self) -> Result<ListPromptsResult, McpError> {
        let prompts = vec![
            Prompt::new(
                "find-task",
                Some("Find tasks matching a query with optional status filter and compact retrieval"),
                Some(vec![
                    prompt_arg("query", "The task keywords or search phrase to find", true),
                    prompt_arg("status", "Optional status filter (e.g. ready, in_progress, blocked, done)", false),
                    prompt_arg("limit", "Optional maximum number of tasks to return (default: 5)", false),
                ]),
            ),
            Prompt::new(
                "explore-topic",
                Some("Explore knowledge base content and notes on a topic using compact snippets"),
                Some(vec![
                    prompt_arg("query", "The topic or concept to explore in the knowledge base", true),
                    prompt_arg("limit", "Optional maximum number of documents to return (default: 5)", false),
                ]),
            ),
            Prompt::new(
                "navigate-graph",
                Some("Inspect connections, dependencies, and neighbourhood of a knowledge graph node"),
                Some(vec![
                    prompt_arg("id", "The node ID, title, or filename stem", true),
                    prompt_arg("direction", "Optional traversal focus: 'both', 'upstream' (blockers), or 'downstream' (dependents)", false),
                ]),
            ),
            Prompt::new(
                "find-by-tag",
                Some("Show all documents and tasks associated with a specific tag"),
                Some(vec![
                    prompt_arg("tag", "The tag to filter by (e.g. bug, mcp, docs)", true),
                    prompt_arg("limit", "Optional maximum number of results (default: 10)", false),
                ]),
            ),
            Prompt::new(
                "task-status",
                Some("Quickly check status, readiness, blockers, and assignee for a task via fastpath"),
                Some(vec![
                    prompt_arg("id", "The task ID to check", true),
                ]),
            ),
            Prompt::new(
                "ready-tasks",
                Some("List unblocked tasks ready for work in flow priority order"),
                Some(vec![
                    prompt_arg("limit", "Optional maximum number of ready tasks to return (default: 10)", false),
                    prompt_arg("project", "Optional project routing slug to filter by", false),
                ]),
            ),
            Prompt::new(
                "task-dependencies",
                Some("Inspect upstream blockers and downstream unblocked tasks for a task"),
                Some(vec![
                    prompt_arg("id", "The task ID to inspect dependencies for", true),
                ]),
            ),
        ];
        Ok(ListPromptsResult::with_all_items(prompts)
            .with_ttl_ms(86_400_000)
            .with_cache_scope(CacheScope::Public))
    }

    pub(crate) fn handle_get_prompt(
        &self,
        request: GetPromptRequestParams,
    ) -> Result<GetPromptResult, McpError> {
        let name = request.name;
        let arguments = request.arguments.unwrap_or_default();

        match name.as_str() {
            "find-task" => {
                let query = get_required_str(&arguments, "query")?;
                let status = get_optional_str(&arguments, "status");
                let limit = get_optional_positive_int(&arguments, "limit", 5)?;

                let mut msg = format!(
                    "I want to find a task about '{}'. Please use 'task_search' to find it, then 'get_task' to read the most relevant one.\n\n\
                     Shortcut parameters:\n\
                     - Query: \"{}\"\n\
                     - Result limit: {}\n",
                    query, query, limit
                );

                if let Some(st) = status {
                    msg.push_str(&format!("- Status filter: \"{}\"\n", st));
                }

                msg.push_str(
                    "\nEfficiency guidance:\n\
                     1. Call 'task_search' with `{\"query\": \"...\", \"limit\": ...}`. It returns compact snippet excerpts by default to minimize context token usage.\n\
                     2. When inspecting a retrieved task, call 'get_task' with `\"detail\": \"summary\"` or `\"detail\": \"metadata\"` (which omits full task bodies and routes by default).\n\
                     3. If only checking completion or readiness, call fastpath 'get_task_status' (`{\"id\": \"...\"}`) which returns lightweight metadata (< 150 bytes)."
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Search for tasks and retrieve with compact token-efficient payload"))
            }
            "explore-topic" => {
                let query = get_required_str(&arguments, "query")?;
                let limit = get_optional_positive_int(&arguments, "limit", 5)?;

                let msg = format!(
                    "What do we know about '{}'? Please use 'search' to find documents, then 'get_document' for the full content of relevant ones.\n\n\
                     Shortcut parameters:\n\
                     - Query: \"{}\"\n\
                     - Result limit: {}\n\n\
                     Efficiency guidance:\n\
                     1. Call 'search' with `{{\"query\": \"{}\", \"limit\": {}}}`. Results default to compact snippet level to conserve token context.\n\
                     2. Drill down into matching documents using 'get_document' with `\"detail\": \"summary\"` or specific `\"fields\"` before fetching full content.",
                    query, query, limit, query, limit
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Explore knowledge base topics using compact search snippets"))
            }
            "navigate-graph" => {
                let id = get_required_str(&arguments, "id")?;
                let direction = get_optional_str(&arguments, "direction");

                let mut msg = format!(
                    "What's connected to '{}'? Please use 'get_task' or 'get_document' to see its relationships in the knowledge graph.\n\n\
                     Shortcut parameters:\n\
                     - Node ID: \"{}\"\n",
                    id, id
                );

                if let Some(dir) = direction {
                    msg.push_str(&format!("- Direction: \"{}\"\n", dir));
                }

                msg.push_str(
                    "\nEfficiency guidance:\n\
                     1. Call 'get_task' or 'get_document' with `\"id\": \"...\", \"detail\": \"summary\"` to inspect connected nodes and relationships without loading full document text or computing costly route paths.\n\
                     2. For dependency trees and blocker analysis, use 'get_dependency_tree' with `{\"id\": \"...\"}`.\n\
                     3. For broader graph topological neighborhood, use 'pkb_context' with `{\"id\": \"...\"}`."
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Navigate knowledge graph relationships and dependencies"))
            }
            "find-by-tag" => {
                let raw_tag = get_required_str(&arguments, "tag")?;
                let tag = raw_tag.strip_prefix('#').unwrap_or(raw_tag).trim();
                if tag.is_empty() {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from("Parameter 'tag' cannot be empty"),
                        data: None,
                    });
                }
                let limit = get_optional_positive_int(&arguments, "limit", 10)?;

                let msg = format!(
                    "Show me everything tagged '{}'. Please use 'search_by_tag' with this tag.\n\n\
                     Shortcut parameters:\n\
                     - Tag: \"{}\"\n\
                     - Result limit: {}\n\n\
                     Efficiency guidance:\n\
                     Call 'search_by_tag' with `{{\"tags\": [\"{}\"]}}`. Review the compact document list and use 'get_task' or 'get_document' with `\"detail\": \"summary\"` for specific items.",
                    tag, tag, limit, tag
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Find documents and tasks tagged with a specific tag"))
            }
            "task-status" => {
                let id = get_required_str(&arguments, "id")?;

                let msg = format!(
                    "Check the status for task '{}'.\n\n\
                     Shortcut parameters:\n\
                     - Task ID: \"{}\"\n\n\
                     Efficiency guidance:\n\
                     Call fastpath 'get_task_status' with `{{\"id\": \"{}\"}}` (returns an ultra-compact payload < 150 bytes containing {{id, status, ready, blocked, assignee}}), or call 'get_task' with `{{\"id\": \"{}\", \"detail\": \"metadata\"}}` without the token overhead of reading the full task body or calculating route traversals.",
                    id, id, id, id
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Fastpath task status, readiness, and blocker check"))
            }
            "ready-tasks" => {
                let limit = get_optional_positive_int(&arguments, "limit", 10)?;
                let project = get_optional_str(&arguments, "project");

                let mut msg = format!(
                    "Find unblocked tasks ready for work in priority order.\n\n\
                     Shortcut parameters:\n\
                     - Status: \"ready\"\n\
                     - Result limit: {}\n",
                    limit
                );

                let mut tool_args = serde_json::json!({
                    "status": "ready",
                    "limit": limit
                });

                if let Some(proj) = project {
                    msg.push_str(&format!("- Project filter: \"{}\"\n", proj));
                    tool_args["project"] = serde_json::json!(proj);
                }

                msg.push_str(&format!(
                    "\nEfficiency guidance:\n\
                     Call 'list_tasks' with `{}`. Tasks are returned in flow priority order (deadline cliff lane first, followed by highest gain/loss averted) without loading document bodies.",
                    serde_json::to_string(&tool_args).unwrap_or_default()
                ));

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("List ready and unblocked tasks in priority order"))
            }
            "task-dependencies" => {
                let id = get_required_str(&arguments, "id")?;

                let msg = format!(
                    "Inspect dependencies and blockers for task '{}'.\n\n\
                     Shortcut parameters:\n\
                     - Task ID: \"{}\"\n\n\
                     Efficiency guidance:\n\
                     Call 'get_dependency_tree' with `{{\"id\": \"{}\"}}` to view upstream blocking dependencies and downstream unblocked tasks in a compact tree structure.",
                    id, id, id
                );

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    msg,
                )]).with_description("Inspect upstream blockers and downstream dependencies"))
            }
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown prompt: {}", name)),
                data: None,
            }),
        }
    }
}
