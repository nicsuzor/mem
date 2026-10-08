//! `list_excalidraw` / `get_excalidraw` / `write_excalidraw`: raw canvas files
//! by PKB-relative path (`mem_24d027d4`). See [`crate::excalidraw::files`].

use rmcp::model::*;
use rmcp::ErrorData as McpError;
use serde_json::Value as JsonValue;
use std::borrow::Cow;

use super::PkbSearchServer;
use crate::excalidraw::files::{self, CanvasError};

fn to_mcp(e: CanvasError) -> McpError {
    let code = match e {
        CanvasError::Invalid(_) => ErrorCode::INVALID_PARAMS,
        CanvasError::Io(_) => ErrorCode::INTERNAL_ERROR,
    };
    McpError {
        code,
        message: Cow::from(e.to_string()),
        data: None,
    }
}

fn required_str<'a>(args: &'a JsonValue, key: &str) -> Result<&'a str, McpError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Missing required parameter: {key}")),
            data: None,
        })
}

impl PkbSearchServer {
    pub(crate) fn handle_list_excalidraw(
        &self,
        args: &JsonValue,
    ) -> Result<CallToolResult, McpError> {
        let dir = args.get("dir").and_then(|v| v.as_str());
        let canvases = files::list_canvases(&self.pkb_root, dir).map_err(to_mcp)?;
        let out = serde_json::json!({ "count": canvases.len(), "canvases": canvases });
        Ok(CallToolResult::success(vec![Content::text(
            out.to_string(),
        )]))
    }

    pub(crate) fn handle_get_excalidraw(
        &self,
        args: &JsonValue,
    ) -> Result<CallToolResult, McpError> {
        let path = required_str(args, "path")?;
        let content = files::read_canvas(&self.pkb_root, path).map_err(to_mcp)?;
        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    pub(crate) fn handle_write_excalidraw(
        &self,
        args: &JsonValue,
    ) -> Result<CallToolResult, McpError> {
        let path = required_str(args, "path")?;
        let content = required_str(args, "content")?;
        let outcome = files::write_canvas(&self.pkb_root, path, content).map_err(to_mcp)?;
        let out = serde_json::to_string(&outcome).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize result: {e}")),
            data: None,
        })?;
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }
}
