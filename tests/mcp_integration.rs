//! Integration tests for MCP server transports (stdio and HTTP/SSE).
//!
//! These tests spawn the `pkb` binary as a child process and exercise the
//! MCP JSON-RPC protocol over both stdio and HTTP/SSE transports.
//!
//! Requires `ACA_DATA` to point at a PKB directory with indexed documents.
//! Skips gracefully if not available.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// ── Helpers ──────────────────────────────────────────────────────────────

fn pkb_binary() -> PathBuf {
    // Prefer release build, fall back to debug
    let release = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/pkb");
    if release.exists() {
        return release;
    }
    let debug = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug/pkb");
    if debug.exists() {
        return debug;
    }
    // Fall back to PATH
    PathBuf::from("pkb")
}

fn aca_data() -> Option<String> {
    std::env::var("ACA_DATA").ok()
}

fn skip_if_no_aca_data() -> String {
    match aca_data() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: ACA_DATA not set");
            std::process::exit(0);
        }
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn jsonrpc_request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn jsonrpc_notification(method: &str) -> String {
    json!({"jsonrpc": "2.0", "method": method}).to_string()
}

fn initialize_request(id: u64) -> String {
    jsonrpc_request(
        id,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "integration-test", "version": "0.1"}
        }),
    )
}

fn tool_call_request(id: u64, name: &str, args: Value) -> String {
    jsonrpc_request(
        id,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
}

fn tools_list_request(id: u64) -> String {
    jsonrpc_request(id, "tools/list", json!({}))
}

// ── Stdio helpers ────────────────────────────────────────────────────────

/// Send JSON-RPC messages over stdio and collect responses.
///
/// Writes all messages to stdin, closes stdin to signal EOF, then reads
/// stdout lines until the expected number of responses arrive or timeout.
fn stdio_session(messages: &[String]) -> Vec<Value> {
    let aca = skip_if_no_aca_data();
    let mut input = String::new();
    for msg in messages {
        input.push_str(msg);
        input.push('\n');
    }

    // Count expected responses (requests with "id" get responses; notifications don't)
    let expected_responses = messages
        .iter()
        .filter(|m| {
            serde_json::from_str::<Value>(m)
                .map(|v| v.get("id").is_some())
                .unwrap_or(false)
        })
        .count();

    let mut child = Command::new(pkb_binary())
        .args(["mcp"])
        .env("ACA_DATA", &aca)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn pkb mcp");

    // Write all messages to stdin, then drop to close the pipe
    let stdin = child.stdin.take().unwrap();
    {
        let mut stdin = stdin;
        stdin.write_all(input.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    // stdin dropped here — sends EOF

    // Read stdout in a thread, send results back via channel
    let stdout_handle = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout_handle);
        let mut responses = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                responses.push(val);
                if responses.len() >= expected_responses {
                    break;
                }
            }
        }
        tx.send(responses).ok();
    });

    // Wait for responses with timeout
    let timeout = Duration::from_secs(30);
    let responses = rx
        .recv_timeout(timeout)
        .unwrap_or_else(|_| {
            child.kill().ok();
            panic!("pkb mcp stdio timed out after {timeout:?}");
        });

    // Kill the process (it may still be running)
    child.kill().ok();
    child.wait().ok();

    responses
}

// ── HTTP/SSE helpers ─────────────────────────────────────────────────────

struct HttpServer {
    child: Option<Child>,
    port: u16,
}

impl HttpServer {
    fn start() -> Self {
        if let Ok(url) = std::env::var("PKB_MCP_URL") {
            let port_str = url.split(':').last().unwrap_or("8026");
            let port = port_str.split('/').next().unwrap_or("8026").parse().unwrap_or(8026);
            return HttpServer { child: None, port };
        }

        let aca = skip_if_no_aca_data();
        let port = free_port();

        let child = Command::new(pkb_binary())
            .args(["mcp", "--http", "--port", &port.to_string()])
            .env("ACA_DATA", &aca)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn pkb mcp --http");

        let server = HttpServer { child: Some(child), port };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        if self.child.is_none() {
            return;
        }
        let start = Instant::now();
        let timeout = Duration::from_secs(30);
        while start.elapsed() < timeout {
            if let Ok(stream) =
                std::net::TcpStream::connect_timeout(
                    &format!("127.0.0.1:{}", self.port).parse().unwrap(),
                    Duration::from_millis(200),
                )
            {
                drop(stream);
                // Give the server a moment after port is open
                std::thread::sleep(Duration::from_millis(500));
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!(
            "HTTP server on port {} not ready after {timeout:?}",
            self.port
        );
    }

    fn is_alive(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            matches!(child.try_wait(), Ok(None))
        } else {
            true
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            child.kill().ok();
            child.wait().ok();
        }
    }
}

/// Raw HTTP POST to /mcp endpoint. Returns (status_code, headers, body).
fn http_post(
    port: u16,
    body: &str,
    session_id: Option<&str>,
) -> (u16, HashMap<String, String>, String) {
    let (host, actual_port) = if let Ok(url) = std::env::var("PKB_MCP_URL") {
        let url = url.trim_start_matches("http://");
        let mut parts = url.split(':');
        let h = parts.next().unwrap().to_string();
        let p_str = parts.next().unwrap_or("80");
        let p = p_str.split('/').next().unwrap_or("80").parse().unwrap_or(80);
        (h, p)
    } else {
        ("127.0.0.1".to_string(), port)
    };

    let mut stream = std::net::TcpStream::connect(format!("{host}:{actual_port}"))
        .expect("failed to connect to HTTP server");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();

    let mut request = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: {host}:{actual_port}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    if let Some(sid) = session_id {
        request.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    request.push_str(body);

    stream.write_all(request.as_bytes()).unwrap();

    // Read full response
    let mut response = Vec::new();
    loop {
        let mut buf = [0u8; 4096];
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => panic!("read error: {e}"),
        }
    }

    let response_str = String::from_utf8_lossy(&response).to_string();

    // Parse HTTP response
    let parts: Vec<&str> = response_str.splitn(2, "\r\n\r\n").collect();
    let header_section = parts[0];
    let body_section = parts.get(1).unwrap_or(&"").to_string();

    // Parse status line
    let status_line = header_section.lines().next().unwrap_or("");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("failed to parse HTTP status code");

    // Parse headers
    let mut headers = HashMap::new();
    for line in header_section.lines().skip(1) {
        if let Some((key, value)) = line.split_once(": ") {
            headers.insert(key.to_lowercase(), value.to_string());
        }
    }

    (status_code, headers, body_section)
}

/// Parse SSE event data lines from response body.
fn parse_sse_messages(body: &str) -> Vec<Value> {
    body.lines()
        .filter(|line| line.starts_with("data: "))
        .filter_map(|line| {
            let json_str = &line[6..];
            serde_json::from_str(json_str).ok()
        })
        .collect()
}

/// Full HTTP session: initialize, send notifications/initialized, return session ID.
fn http_initialize(port: u16) -> (String, Value) {
    let (status, headers, body) = http_post(port, &initialize_request(1), None);
    assert_eq!(status, 200, "initialize failed with status {status}. Body: {body}");

    let session_id = headers
        .get("mcp-session-id")
        .expect("no Mcp-Session-Id header in initialize response")
        .clone();

    let messages = parse_sse_messages(&body);
    assert!(
        !messages.is_empty(),
        "no SSE data events in initialize response. Raw body:\n{body}"
    );

    let init_result = messages[0].clone();
    assert!(
        init_result.get("result").is_some(),
        "initialize response missing 'result': {init_result}"
    );

    // Send initialized notification
    let (notif_status, _, _) = http_post(
        port,
        &jsonrpc_notification("notifications/initialized"),
        Some(&session_id),
    );
    // 200 or 202 both acceptable for notifications
    assert!(
        notif_status == 200 || notif_status == 202,
        "notifications/initialized returned {notif_status}"
    );

    (session_id, init_result)
}

/// Call a tool over HTTP with an existing session.
fn http_call_tool(
    port: u16,
    session_id: &str,
    id: u64,
    tool: &str,
    args: Value,
) -> Value {
    let (status, _, body) = http_post(
        port,
        &tool_call_request(id, tool, args),
        Some(session_id),
    );
    assert!(
        status == 200,
        "tool call '{tool}' returned status {status}. Body:\n{body}"
    );

    let messages = parse_sse_messages(&body);
    assert!(
        !messages.is_empty(),
        "no SSE data in tool call '{tool}' response. Raw body:\n{body}"
    );

    messages[0].clone()
}

// ── Stdio tests ──────────────────────────────────────────────────────────

#[test]
fn test_stdio_initialize() {
    let responses = stdio_session(&[initialize_request(1)]);
    assert!(!responses.is_empty(), "no response from stdio initialize");

    let result = &responses[0];
    assert!(result.get("result").is_some(), "no 'result' in response: {result}");

    let server_info = &result["result"]["serverInfo"];
    assert_eq!(
        server_info["name"].as_str(),
        Some("pkb"),
        "server name mismatch: {server_info}"
    );
}

#[test]
fn test_stdio_stdout_purity() {
    let aca = skip_if_no_aca_data();

    let mut child = Command::new(pkb_binary())
        .args(["mcp"])
        .env("ACA_DATA", &aca)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn pkb mcp");

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", initialize_request(1)).unwrap();
    }

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    child.kill().ok();
    child.wait().ok();

    let mut stdout = String::new();
    child.stdout.unwrap().read_to_string(&mut stdout).unwrap();

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly 1 line on stdout (JSON-RPC response), got {}.\nFull stdout:\n{}",
        lines.len(),
        stdout
    );
    assert!(
        lines[0].starts_with('{'),
        "stdout line doesn't start with '{{': {}",
        lines[0]
    );

    // Verify it's valid JSON
    let _: Value = serde_json::from_str(lines[0]).expect("stdout is not valid JSON");
}

#[test]
fn test_stdio_tool_call() {
    let responses = stdio_session(&[
        initialize_request(1),
        jsonrpc_notification("notifications/initialized"),
        tool_call_request(2, "graph_stats", json!({})),
    ]);

    // Find the tool call response (id=2)
    let tool_response = responses
        .iter()
        .find(|r| r.get("id") == Some(&json!(2)))
        .expect("no response with id=2 for graph_stats");

    assert!(
        tool_response.get("result").is_some(),
        "graph_stats returned error: {tool_response}"
    );
    assert!(
        tool_response["result"].get("content").is_some(),
        "graph_stats result missing 'content': {tool_response}"
    );
}

// ── HTTP/SSE tests ───────────────────────────────────────────────────────

#[test]
fn test_http_server_starts() {
    let mut server = HttpServer::start();
    assert!(server.is_alive(), "server died after start");
}

#[test]
fn test_http_initialize() {
    let server = HttpServer::start();
    let (status, headers, body) = http_post(server.port, &initialize_request(1), None);

    assert_eq!(status, 200, "initialize status: {status}");

    // Validate SSE content type
    let content_type = headers
        .get("content-type")
        .expect("no Content-Type header");
    assert!(
        content_type.contains("text/event-stream"),
        "expected text/event-stream, got: {content_type}"
    );

    // Validate session ID
    assert!(
        headers.contains_key("mcp-session-id"),
        "no Mcp-Session-Id header. Headers: {headers:?}"
    );

    // Validate response content
    let messages = parse_sse_messages(&body);
    assert!(!messages.is_empty(), "no SSE data events. Body:\n{body}");

    let result = &messages[0]["result"];
    assert_eq!(
        result["serverInfo"]["name"].as_str(),
        Some("pkb"),
        "server name mismatch: {result}"
    );
}

#[test]
fn test_http_session_reuse() {
    // THIS IS THE KEY REGRESSION TEST.
    // Previous QA failed here: session was destroyed when curl closed the
    // SSE connection. This test uses separate TCP connections to verify
    // session persistence.
    let server = HttpServer::start();
    let (session_id, _) = http_initialize(server.port);

    // Call a tool on a NEW connection with the same session ID
    let result = http_call_tool(server.port, &session_id, 2, "graph_stats", json!({}));

    assert!(
        result.get("result").is_some(),
        "session reuse failed — got error instead of result: {result}\n\
         This is the exact regression from the previous QA failure.\n\
         Session ID was: {session_id}"
    );
    assert!(
        result["result"].get("content").is_some(),
        "graph_stats via HTTP missing content: {result}"
    );
}

#[test]
fn test_http_multiple_tools() {
    let server = HttpServer::start();
    let (session_id, _) = http_initialize(server.port);

    let tools = vec![
        ("graph_stats", json!({})),
        ("task_summary", json!({})),
        ("list_tasks", json!({"limit": 3})),
    ];

    for (i, (tool_name, args)) in tools.iter().enumerate() {
        let result = http_call_tool(server.port, &session_id, (i + 2) as u64, tool_name, args.clone());
        assert!(
            result.get("result").is_some(),
            "tool '{tool_name}' (call #{}) returned error: {result}",
            i + 1
        );
        assert!(
            result["result"].get("content").is_some(),
            "tool '{tool_name}' missing content: {result}"
        );
    }
}

#[test]
fn test_http_tools_list_count() {
    let server = HttpServer::start();
    let (session_id, _) = http_initialize(server.port);

    let (status, _, body) = http_post(
        server.port,
        &tools_list_request(2),
        Some(&session_id),
    );
    assert_eq!(status, 200);

    let messages = parse_sse_messages(&body);
    assert!(!messages.is_empty(), "no SSE data in tools/list response");

    let tools = messages[0]["result"]["tools"]
        .as_array()
        .expect("tools/list result missing 'tools' array");

    // Should match the count documented in CORE.md
    assert!(
        tools.len() >= 30,
        "expected at least 30 tools, got {}",
        tools.len()
    );
}

#[test]
fn test_http_error_missing_session_id() {
    let mut server = HttpServer::start();
    let (session_id, _) = http_initialize(server.port);
    let _ = session_id; // establish session but don't use its ID

    // POST a tool call WITHOUT the session ID header
    let (status, _, _) = http_post(
        server.port,
        &tool_call_request(99, "graph_stats", json!({})),
        None, // deliberately missing
    );

    // Server should reject with 4xx, NOT crash
    assert!(
        status >= 400 && status < 500,
        "expected 4xx for missing session ID, got {status}"
    );
    assert!(server.is_alive(), "server crashed after missing session ID");
}

#[test]
fn test_http_error_invalid_session_id() {
    let mut server = HttpServer::start();
    let _ = http_initialize(server.port);

    let (status, _, _) = http_post(
        server.port,
        &tool_call_request(99, "graph_stats", json!({})),
        Some("bogus-nonexistent-session-id-12345"),
    );

    assert!(
        status == 404 || (status >= 400 && status < 500),
        "expected 404 for invalid session ID, got {status}"
    );
    assert!(server.is_alive(), "server crashed after invalid session ID");
}

#[test]
fn test_http_error_malformed_json() {
    let mut server = HttpServer::start();

    let (status, _, _) = http_post(server.port, "{not valid json at all!!", None);

    assert!(
        status >= 400,
        "expected error status for malformed JSON, got {status}"
    );
    assert!(server.is_alive(), "server crashed after malformed JSON");
}

#[test]
fn test_http_error_unknown_tool() {
    let server = HttpServer::start();
    let (session_id, _) = http_initialize(server.port);

    let result = http_call_tool(
        server.port,
        &session_id,
        99,
        "nonexistent_tool_that_does_not_exist",
        json!({}),
    );

    // Should return a JSON-RPC error, not crash
    assert!(
        result.get("error").is_some(),
        "expected JSON-RPC error for unknown tool, got: {result}"
    );
}

/// Regression test for task-3c672195 / PR for "PKB semantic search returns no
/// results". Seeds a temp PKB with a single uniquely-keyed document, reindexes
/// it via the `pkb` CLI, then runs an MCP HTTP search for a query that should
/// match only that document, and asserts the document appears in the results.
///
/// Skips gracefully if the BGE-M3 ONNX model is not available locally
/// (CI that does not pre-cache the model would otherwise have to download
/// ~2 GB on every run).
#[test]
fn test_http_seeded_search_returns_seeded_doc() {
    use std::fs;
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(_) => {
            eprintln!("SKIP: could not create tempdir");
            return;
        }
    };
    let pkb_root = dir.path();
    let db_path = pkb_root.join("test_index.bin");

    // A unique, unmistakable phrase so the search has an obvious top hit.
    let unique_phrase = "xyzzyplugh-quokka-photoluminescent";
    let seed_md = format!(
        "---\ntitle: Seeded Search Marker\ntype: note\n---\n\n\
         This document exists only to prove search is wired up. \
         The unique marker phrase is {unique_phrase} which should be \
         the dominant signal in any embedding of this content."
    );
    fs::write(pkb_root.join("seeded.md"), seed_md).expect("write seed file");
    fs::write(
        pkb_root.join("decoy.md"),
        "---\ntitle: Decoy\ntype: note\n---\n\nUnrelated weather report — \
         yesterday was sunny, tomorrow may rain.",
    )
    .expect("write decoy file");

    // Run `pkb reindex` against the temp PKB. If the embedder cannot init
    // (no ONNX model cached, no network), skip — we cannot drive search
    // meaningfully without it.
    let pkb_root_str = pkb_root.to_string_lossy().to_string();
    let db_path_str = db_path.to_string_lossy().to_string();
    let reindex = Command::new(pkb_binary())
        .args([
            "--pkb-root",
            &pkb_root_str,
            "--db-path",
            &db_path_str,
            "reindex",
        ])
        .env("AOPS_OFFLINE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match reindex {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP: failed to spawn pkb reindex: {e}");
            return;
        }
    };
    if !output.status.success() {
        eprintln!(
            "SKIP: reindex failed (likely no cached ONNX model). stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    // Spawn an MCP HTTP daemon against the temp PKB.
    let port = free_port();
    let mut child = Command::new(pkb_binary())
        .args([
            "--pkb-root",
            &pkb_root_str,
            "--db-path",
            &db_path_str,
            "mcp",
            "--http",
            "--port",
            &port.to_string(),
        ])
        .env("AOPS_OFFLINE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pkb mcp --http");

    // Wait for the daemon to bind the port.
    let start = Instant::now();
    let timeout = Duration::from_secs(30);
    let mut ready = false;
    while start.elapsed() < timeout {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if !ready {
        child.kill().ok();
        panic!("MCP HTTP server on port {port} not ready after {timeout:?}");
    }
    std::thread::sleep(Duration::from_millis(500));

    // Forcibly target the local daemon — the http_post helper otherwise
    // honours PKB_MCP_URL and would silently exercise the user's production
    // server, which would defeat the seeded-search assertion entirely.
    let prior_pkb_mcp_url = std::env::var("PKB_MCP_URL").ok();
    std::env::remove_var("PKB_MCP_URL");

    let result = (|| -> Value {
        let (status, headers, body) =
            http_post(port, &initialize_request(1), None);
        assert_eq!(status, 200, "initialize failed: {body}");
        let session_id = headers
            .get("mcp-session-id")
            .expect("missing session id")
            .clone();

        let (_status, _, _) = http_post(
            port,
            &jsonrpc_notification("notifications/initialized"),
            Some(&session_id),
        );

        http_call_tool(
            port,
            &session_id,
            2,
            "search",
            json!({"query": unique_phrase, "limit": 5}),
        )
    })();

    if let Some(v) = prior_pkb_mcp_url {
        std::env::set_var("PKB_MCP_URL", v);
    }

    child.kill().ok();
    child.wait().ok();

    let result_text = result["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // The acceptance criterion: a query with an obvious hit must return
    // non-empty results, and the seeded document must be among them.
    assert!(
        !result_text.contains("No results found"),
        "search returned 'No results found' for a query that should match a \
         seeded document. Full response: {result}"
    );
    assert!(
        result_text.contains("Seeded Search Marker"),
        "search did not surface the seeded document. Response: {result_text}"
    );
}

#[test]
fn test_http_concurrent_sessions() {
    let server = HttpServer::start();

    let port = server.port;
    let handles: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let (session_id, _) = http_initialize(port);
                let result = http_call_tool(
                    port,
                    &session_id,
                    2,
                    if i == 0 { "graph_stats" } else { "task_summary" },
                    json!({}),
                );
                (session_id, result)
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Both sessions should succeed
    for (i, (sid, result)) in results.iter().enumerate() {
        assert!(
            result.get("result").is_some(),
            "concurrent session {i} (sid={sid}) failed: {result}"
        );
    }

    // Session IDs must be different
    assert_ne!(
        results[0].0, results[1].0,
        "concurrent sessions got the same session ID!"
    );
}
