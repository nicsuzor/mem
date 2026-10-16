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

- PKB MCP server (Rust, `nicsuzor/mem`): `src/otel/client_context.rs` resolves the caller session and `src/otel/session_registry.rs` binds it to the transport connection
- Client side and gateway side: academicOps `specs/observability/observability.md` US-3. That spec owns the client hook and gateway forwarding; this spec owns what the server accepts, binds, records and refuses

## Purpose

Every PKB read and write is attributed to the agent session that made it, so the task graph and the trace store can answer "which session did this" without agent processing. The server never guesses: an id it cannot attribute stays unattributed, and it is visible as such.

## User stories

### US-1 Every call carries its caller's session

As the user, I want every PKB call stamped with the session id of the agent that made it, supplied mechanically by the client wherever the client allows it, so attribution never depends on an agent remembering.

- **AC-1.1** Given a call carrying a caller session id from any accepted source, when the server handles it, then the call's span carries that id in `session.id` and its source in `aops.session.source` (`header`, `meta`, `argument`, or `binding`).
- **AC-1.2** Given a caller session id in the reserved `_session` argument object (`session_id`, optional `agent_id`, optional `prompt_id`), when the server handles the call, then it removes `_session` before the tool's own argument validation runs, so every tool accepts it without declaring it.
- **AC-1.3** Given a call that carries `agent_id` or `prompt_id`, when its span is exported, then the span carries them as `agent.id` and `prompt.id`.

### US-2 A direct connection remembers its session

As the user, I want a client that names its session once on a direct connection to have later calls on that connection attributed to it, so a client that can only identify itself at the start of a session is still attributed reliably.

- **AC-2.1** Given a non-shared transport connection (`Mcp-Session-Id`) on which a call supplied an explicit session id, when a later call on that connection supplies none, then the later call is attributed to the most recently supplied id, with source `binding`.
- **AC-2.2** Given a connection bound to one session, when a call supplies a different explicit id, then that call is attributed to the new id, the binding moves to it, and the span records the previous id in `aops.session.rebound_from`.
- **AC-2.3** Given a connection whose client identifies itself in `initialize` as a gateway that multiplexes several agents, when any call on it supplies no explicit id, then the server never binds it and never attributes a call from the binding.

### US-3 The server never invents a session

As the user, I want an unattributable call recorded as unattributed, so trace-store queries by `session.id` contain only real agent sessions.

- **AC-3.1** Given a call with no explicit id and no valid binding, when the server handles it, then `session.id` is unset, the transport id is recorded only in `mcp.session_id`, and `aops.session.source = "none"`.
- **AC-3.2** Given any call, when the server binds a connection, then only an explicitly supplied id is ever bound: never a transport id, a server environment variable, or a placeholder.

### US-4 Writes require a session

As the user, I want writes that cannot be attributed refused with an error that tells the agent how to supply its session, so missing attribution is fixed at the source.

- **AC-4.1** Given a mutating tool call with `aops.session.source = "none"`, when the server runs in enforcing mode, then it refuses the call with an error naming the `_session` argument and how the agent finds its own session id.
- **AC-4.2** Given the same call in warning mode, when the server handles it, then the call succeeds and the span carries `aops.session.missing = true`, so coverage can be measured before enforcement is switched on.
- **AC-4.3** Given a read-only tool call with no session, when the server handles it, then the call succeeds in both modes and is recorded per AC-3.1.

## Design

### Resolution order

The server takes the first explicit id it finds:

1. The `x-session-id` header.
2. `_meta.session_id` on the request.
3. The `_session` argument.
4. The connection binding (US-2).

Only sources 1–3 update a binding. A W3C `traceparent` in headers or `_meta` parents the server span under the caller's span, whatever the session source.

### Connection classes

| Class | How identified | Binding |
| --- | --- | --- |
| Direct (one agent session per connection) | any `initialize` client not listed as a gateway | Bound to the last explicit id |
| Gateway (many agents share the connection) | `initialize` client name listed in the server's gateway list | Never bound; every call must carry its own id |

### Modes

`enforce` and `warn` select AC-4.1 or AC-4.2. The server starts in `warn`. It switches to `enforce` once the trace store shows writes carrying `aops.session.missing` have stopped.
