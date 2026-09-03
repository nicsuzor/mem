use rmcp::model::*;
use rmcp::ErrorData as McpError;
use std::borrow::Cow;

use super::PkbSearchServer;

impl PkbSearchServer {
    pub(crate) fn handle_list_prompts(&self) -> Result<ListPromptsResult, McpError> {
        fn required_arg(name: &str, description: &str) -> PromptArgument {
            PromptArgument::new(name)
                .with_description(description)
                .with_required(true)
        }
        let prompts = vec![
            Prompt::new(
                "find-task",
                Some("How do I find a task about X?"),
                Some(vec![required_arg("query", "The task to find")]),
            ),
            Prompt::new(
                "explore-topic",
                Some("What do we know about X?"),
                Some(vec![required_arg("query", "The topic to explore")]),
            ),
            Prompt::new(
                "navigate-graph",
                Some("What's connected to X?"),
                Some(vec![required_arg("id", "The node ID, title, or filename")]),
            ),
            Prompt::new(
                "find-by-tag",
                Some("Show me everything tagged X"),
                Some(vec![required_arg("tag", "The tag to filter by")]),
            ),
        ];
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    pub(crate) fn handle_get_prompt(
        &self,
        request: GetPromptRequestParams,
    ) -> Result<GetPromptResult, McpError> {
        let name = request.name;
        let arguments = request.arguments.unwrap_or_default();

        match name.as_str() {
            "find-task" => {
                let query = arguments.get("query").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: query"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
                    "I want to find a task about '{}'. Please use 'task_search' to find it, then 'get_task' to read the most relevant one.",
                    query
                ))]))
            }
            "explore-topic" => {
                let query = arguments.get("query").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: query"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
                    "What do we know about '{}'? Please use 'search' to find documents, then 'get_document' for the full content of relevant ones.",
                    query
                ))]))
            }
            "navigate-graph" => {
                let id = arguments.get("id").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: id"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(PromptMessageRole::User, format!(
                    "What's connected to '{}'? Please use 'get_task' or 'get_document' to see its relationships in the knowledge graph.",
                    id
                ))]))
            }
            "find-by-tag" => {
                let tag = arguments.get("tag").ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Missing required parameter: tag"),
                    data: None,
                })?;
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!(
                        "Show me everything tagged '{}'. Please use 'search_by_tag' with this tag.",
                        tag
                    ),
                )]))
            }
            _ => Err(McpError {
                code: ErrorCode::METHOD_NOT_FOUND,
                message: Cow::from(format!("Unknown prompt: {}", name)),
                data: None,
            }),
        }
    }

}
