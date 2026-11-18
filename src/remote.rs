//! Minimal client for calling a running PKB MCP server over HTTP.
//!
//! The CLI uses this so interactive commands such as `pkb search` reuse the
//! server's already-loaded embedder, reranker and indexes instead of loading
//! them from disk on every invocation.
//!
//! The server speaks stateless MCP 2026-07-28 Streamable HTTP: every request
//! carries `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers plus the
//! protocol metadata in `params._meta`, and every PKB tool call needs a
//! top-level `session_id` argument.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Environment variable naming the PKB MCP server endpoint.
pub const SERVER_URL_ENV: &str = "PKB_MCP_URL";

/// MCP protocol revision the server requires on every request.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// Request timeout for a single tool call.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Resolve the server URL: an explicit URL wins, otherwise `PKB_MCP_URL`.
/// There is no default — if neither is given this is an error.
pub fn resolve_server_url(explicit: Option<&str>) -> Result<String> {
    if let Some(url) = explicit.map(str::trim).filter(|u| !u.is_empty()) {
        return Ok(url.to_string());
    }
    match std::env::var(SERVER_URL_ENV) {
        Ok(url) if !url.trim().is_empty() => Ok(url.trim().to_string()),
        _ => bail!(
            "no PKB server URL: pass --server <URL> or set {SERVER_URL_ENV} \
             (or use --local to search the local index in-process)"
        ),
    }
}

/// The `session_id` sent with each call: the caller's agent session when one
/// is present in the environment, otherwise a fixed CLI client identifier.
pub fn client_session_id() -> String {
    ["CLAUDE_CODE_SESSION_ID", "ANTIGRAVITY_CONVERSATION_ID"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "pkb-cli".to_string())
}

/// Build the JSON-RPC body for a `tools/call` request.
pub fn tool_call_body(tool: &str, mut arguments: Value, session_id: &str) -> Value {
    if let Some(obj) = arguments.as_object_mut() {
        obj.entry("session_id")
            .or_insert_with(|| Value::String(session_id.to_string()));
    }
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {},
            },
            "arguments": arguments,
        }
    })
}

/// Parse a response body, accepting plain JSON or a single SSE `data:` frame.
fn parse_response_body(body: &str) -> Result<Value> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).context("invalid JSON from PKB server");
    }
    let data: String = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        bail!(
            "unrecognised response from PKB server: {}",
            body.chars().take(200).collect::<String>()
        );
    }
    serde_json::from_str(&data).context("invalid JSON in PKB server event stream")
}

/// Extract the tool result from a JSON-RPC response, turning protocol errors
/// and `isError` tool results into `Err`.
pub fn extract_tool_result(response: Value) -> Result<Value> {
    if let Some(err) = response.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("PKB server error: {msg}");
    }
    let result = response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("PKB server response has no result"))?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.iter().find_map(|p| p.get("text").and_then(Value::as_str)))
            .unwrap_or("tool reported an error");
        bail!("PKB server tool error: {text}");
    }
    Ok(result)
}

/// Call `tool` on the PKB MCP server at `url` and return the tool result
/// (`content`, `structuredContent`, ...).
pub fn call_tool(url: &str, tool: &str, arguments: Value) -> Result<Value> {
    let body = tool_call_body(tool, arguments, &client_session_id());
    let resp = ureq::post(url)
        .timeout(TIMEOUT)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/event-stream")
        .set("MCP-Protocol-Version", PROTOCOL_VERSION)
        .set("Mcp-Method", "tools/call")
        .set("Mcp-Name", tool)
        .send_json(body);
    let text = match resp {
        Ok(r) => r
            .into_string()
            .context("failed to read PKB server response")?,
        // JSON-RPC errors may arrive with a non-2xx status; surface their message.
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            match parse_response_body(&text) {
                Ok(v) if v.get("error").is_some() => text,
                _ => bail!("PKB server at {url} returned HTTP {code}: {text}"),
            }
        }
        Err(e) => return Err(anyhow!("cannot reach PKB server at {url}: {e}")),
    };
    extract_tool_result(parse_response_body(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_url_wins_and_blank_is_ignored() {
        assert_eq!(
            resolve_server_url(Some(" http://a:1/mcp ")).unwrap(),
            "http://a:1/mcp"
        );
    }

    #[test]
    fn body_carries_session_and_protocol_meta() {
        let b = tool_call_body("search", json!({"query": "x"}), "sid");
        assert_eq!(b["params"]["name"], "search");
        assert_eq!(b["params"]["arguments"]["session_id"], "sid");
        assert_eq!(
            b["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            PROTOCOL_VERSION
        );
        // An explicit session_id in the arguments is not overwritten.
        let b = tool_call_body("search", json!({"session_id": "mine"}), "sid");
        assert_eq!(b["params"]["arguments"]["session_id"], "mine");
    }

    #[test]
    fn parses_json_and_sse_bodies() {
        let v = parse_response_body(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
        assert!(v.get("result").is_some());
        let v =
            parse_response_body("event: message\ndata: {\"jsonrpc\":\"2.0\",\"result\":{}}\n\n")
                .unwrap();
        assert!(v.get("result").is_some());
    }

    #[test]
    fn errors_are_surfaced() {
        let e =
            extract_tool_result(json!({"error": {"code": -32602, "message": "bad"}})).unwrap_err();
        assert!(e.to_string().contains("bad"));
        let e = extract_tool_result(json!({"result": {"isError": true,
            "content": [{"type": "text", "text": "boom"}]}}))
        .unwrap_err();
        assert!(e.to_string().contains("boom"));
        assert!(extract_tool_result(json!({"result": {"content": []}})).is_ok());
    }
}
