---
id: caller-session
title: "Caller session: attributing every MCP call to the agent session that made it"
type: spec
status: ready
tier: data
depends_on: [pkb-server]
tags: [spec, mcp, otel, observability, sessions]
---

# Caller session: attributing every MCP call to the agent session that made it

## Giving Effect

- PKB MCP server (Rust, `nicsuzor/mem`): `src/otel/client_context.rs` resolves the caller session per request and `src/mcp_server/mod.rs` enforces it on every tool call.

## Purpose

Every PKB read and write is attributed to the calling agent's active session or client identifier via stateless MCP `2026-07-28` requests, so the task graph and trace store can answer which session performed every operation. The server never invents or carries session state across requests: every request supplies its own `session_id`.

## Requirements

### US-1 Every call carries its caller's session

Every PKB call is stamped with the caller's session ID and exported on the server span (`session.id` and `aops.session.source` in `header`, `meta`, `argument`, or `none`).

- **AC-1.1** `ClientContext::extract` checks sources in precedence order:
  1. `x-session-id` (or `session-id`) HTTP header (`source = "header"`).
  2. Request `_meta.session_id` / `_meta.sessionId` (`source = "meta"`).
  3. Top-level `arguments["session_id"]` non-empty trimmed string (`source = "argument"`), preserved in `args` so handlers like `release_task` can read it.
- **AC-1.2** Every tool schema in `src/mcp_server/schemas.rs` declares `session_id` (`type: "string"`, `minLength: 1`) as a required parameter describing the calling agent's own active session/conversation ID (e.g., `CLAUDE_CODE_SESSION_ID` or `ANTIGRAVITY_CONVERSATION_ID`) or client identifier.
- **AC-1.3** Given `x-agent-id` / `_meta.agent_id` or `x-prompt-id` / `_meta.prompt_id`, the server span exports `agent.id` and `prompt.id`.
- **AC-1.4** Any W3C `traceparent` in HTTP headers or request `_meta` parents the server span under the caller's span.

### US-2 Missing or empty `session_id` is rejected on all tool calls

Because every request on the stateless `/mcp` endpoint is independent and sessionless:

- **AC-2.1** On every tool call (both reads and writes), when `session_id` resolves to `None` (missing, null, empty, or whitespace-only across header, `_meta`, and `arguments["session_id"]`), `execute_tool_call` refuses the call with JSON-RPC `INVALID_PARAMS` (`-32602`) and clear guidance explaining how to supply `session_id`.
- **AC-2.2** Refused calls still emit an OTel span carrying `aops.session.source = "none"` and `aops.session.missing = true`.
- **AC-2.3** `ServerHandler::get_info` and `ServerHandler::discover` publish server instructions stating that every PKB tool call (both reads and writes) MUST include a non-empty `session_id`.

### US-3 Stateless client identity

- **AC-3.1** Client identity (`client_name`) is extracted per request from `_meta["io.modelcontextprotocol/clientInfo"]["name"]` (with `x-client-name` header fallback) so `overwhelm-dashboard` calls can perform attributed human edits (`set_by: "nic"`) statelessly without persistent transport sessions.
- **AC-3.2** Caller-supplied `_client` in tool arguments is always stripped before execution so agents cannot spoof dashboard identity.
