//! Integration tests for MCP server transports (stdio and HTTP/SSE).
//!
//! These tests spawn the `pkb` binary as a child process and exercise the
//! MCP JSON-RPC protocol over both stdio and HTTP/SSE transports.
//!
//! Uses an isolated synthetic fixture PKB (`create_test_pkb`) to verify protocol
//! behavior, session lifecycle, tool calls, and error handling without coupling
//! to external real-scale `ACA_DATA` directories.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// ── Process-lifecycle backstop ──────────────────────────────────────────
//
// `Drop`-based child reaping (see `impl Drop for HttpServer` below) does not
// run when the harness process is itself killed abnormally (SIGKILL, OOM
// kill, CI timeout that sends SIGKILL) rather than unwinding normally. Spawned
// `pkb mcp --http` servers then get reparented to init and run forever.
//
// `PR_SET_PDEATHSIG` is a kernel-level backstop: it asks the kernel to send
// SIGKILL to the child when the thread that spawned it exits, for any reason,
// including the parent being SIGKILLed. It does NOT cover: parent processes
// SIGKILLed via a signal to the whole process group before the child's
// creating thread runs (fork/exec is effectively atomic for this purpose so
// that window doesn't exist in practice), or the child being adopted by a
// *different* thread than the one that called `spawn` (not applicable here —
// each call site spawns and forgets synchronously). It also does not cover
// non-Linux platforms, hence `#[cfg(unix)]` — on other unix platforms
// (e.g. macOS) `PR_SET_PDEATHSIG` doesn't exist and this becomes a no-op below.
#[cfg(target_os = "linux")]
trait KillOnParentDeath {
    fn kill_on_parent_death(&mut self) -> &mut Self;
}

#[cfg(target_os = "linux")]
impl KillOnParentDeath for Command {
    fn kill_on_parent_death(&mut self) -> &mut Self {
        use std::os::unix::process::CommandExt;
        unsafe {
            self.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        self
    }
}

#[cfg(not(target_os = "linux"))]
trait KillOnParentDeath {
    fn kill_on_parent_death(&mut self) -> &mut Self;
}

#[cfg(not(target_os = "linux"))]
impl KillOnParentDeath for Command {
    fn kill_on_parent_death(&mut self) -> &mut Self {
        self
    }
}

/// Read `/proc/<pid>/stat` and determine whether the process is alive.
/// A bare `kill(pid, 0)` is not sufficient — it returns success for zombies.
/// The `comm` field (2nd column) can itself contain spaces and parentheses,
/// so the state field is located from the LAST `)` in the line, not the
/// first `(`.
#[cfg(target_os = "linux")]
fn pid_is_live(pid: i32) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let after_comm = match stat.rfind(')') {
        Some(idx) => &stat[idx + 1..],
        None => return false,
    };
    match after_comm.trim_start().chars().next() {
        Some('Z') | None => false,
        Some(_) => true,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn pkb_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pkb"))
}

/// Create an isolated, synthetic fixture PKB for integration testing.
///
/// This eliminates any runtime dependency on a large real-scale `$ACA_DATA` directory
/// (which can span thousands of documents and cause tens of seconds of startup delay/timeouts).
/// The synthetic fixture provides valid tasks, epics, and memories subdirectories along with
/// a temporary vector database path, ensuring fast, deterministic, and isolated MCP test runs.
fn create_test_pkb() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir for test PKB");
    let pkb_root = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(pkb_root.join("tasks")).expect("create tasks dir");
    std::fs::create_dir_all(pkb_root.join("epics")).expect("create epics dir");
    std::fs::create_dir_all(pkb_root.join("memories")).expect("create memories dir");

    // Seed a sample task so task/graph queries return non-empty data if inspected
    let sample_task = "---\nid: task-test-01\ntitle: Sample Integration Task\ntype: task\nstatus: ready\npriority: 2\n---\n\nSample test body.";
    std::fs::write(pkb_root.join("tasks").join("task-test-01.md"), sample_task).expect("write sample task");

    let db_path = pkb_root.join("test_vectors.bin");
    (temp_dir, pkb_root, db_path)
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
    jsonrpc_request(id, "tools/call", json!({"name": name, "arguments": args}))
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
    let (_temp_dir, pkb_root, db_path) = create_test_pkb();
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
        .env_remove("PKB_MCP_URL")
        .args([
            "--pkb-root",
            pkb_root.to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "mcp",
        ])
        .env("ACA_DATA", pkb_root.to_str().unwrap())
        .env("AOPS_DUMMY_EMBEDDER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_parent_death()
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

    // Wait for responses with timeout (120s upper bound for debug/heavy load)
    let timeout = Duration::from_secs(120);
    let responses = rx.recv_timeout(timeout).unwrap_or_else(|_| {
        child.kill().ok();
        panic!("pkb mcp stdio timed out after {timeout:?}");
    });

    // Kill the process (it may still be running)
    child.kill().ok();
    child.wait().ok();

    responses
}

// ── HTTP/SSE helpers ─────────────────────────────────────────────────────

fn spawn_http_mcp(mut cmd: Command) -> (Child, u16) {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_parent_death();
    let mut child = cmd.spawn().expect("failed to spawn MCP HTTP server");

    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_lines_clone = stderr_lines.clone();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let mut sent = false;
        while let Ok(bytes) = reader.read_line(&mut line) {
            if bytes == 0 { break; }
            stderr_lines_clone.lock().unwrap().push(line.clone());
            if !sent {
                if let Some(idx) = line.find("Starting MCP HTTP/SSE server on http://") {
                    let start = idx + "Starting MCP HTTP/SSE server on http://".len();
                    let url = &line[start..];
                    if let Some(end) = url.find("/mcp") {
                        let host_port = &url[..end];
                        if let Some(port_str) = host_port.split(':').next_back() {
                            if let Ok(port) = port_str.parse::<u16>() {
                                let _ = tx.send(port);
                                sent = true;
                            }
                        }
                    }
                }
            }
            line.clear();
        }
    });

    // Raised from 10s to 60s to accommodate cold ONNX runtime initialization and loaded CI environments
    let startup_timeout = std::env::var("MCP_TEST_STARTUP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60));
    let port = rx.recv_timeout(startup_timeout).unwrap_or_else(|err| {
        let lines = stderr_lines.lock().unwrap().join("");
        panic!("failed to parse port from stderr ({err:?}). Full server stderr:\n{lines}");
    });
    (child, port)
}

struct HttpServer {
    child: Option<Child>,
    port: u16,
    _temp_dir: Option<tempfile::TempDir>,
}

impl HttpServer {
    fn start() -> Self {
        let (temp_dir, pkb_root, db_path) = create_test_pkb();

        let mut cmd = Command::new(pkb_binary());
        cmd.env_remove("PKB_MCP_URL")
            .args([
                "--pkb-root",
                pkb_root.to_str().unwrap(),
                "--db-path",
                db_path.to_str().unwrap(),
                "mcp",
                "--http",
                "--port",
                "0",
            ])
            .env("ACA_DATA", pkb_root.to_str().unwrap())
            .env("AOPS_DUMMY_EMBEDDER", "1")
            .stdin(Stdio::null());

        let (child, port) = spawn_http_mcp(cmd);

        let server = HttpServer {
            child: Some(child),
            port,
            _temp_dir: Some(temp_dir),
        };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        if self.child.is_none() {
            return;
        }
        let start = Instant::now();
        let timeout = Duration::from_secs(120);
        while start.elapsed() < timeout {
            if let Ok(stream) = std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{}", self.port).parse().unwrap(),
                Duration::from_millis(200),
            ) {
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
    let host = "127.0.0.1".to_string();
    let actual_port = port;

    let mut stream = std::net::TcpStream::connect(format!("{host}:{actual_port}"))
        .expect("failed to connect to HTTP server");
    // Raised from 15s to 30s to provide generous headroom under heavy CI load
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
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
    assert_eq!(
        status, 200,
        "initialize failed with status {status}. Body: {body}"
    );

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
fn http_call_tool(port: u16, session_id: &str, id: u64, tool: &str, args: Value) -> Value {
    let (status, _, body) = http_post(port, &tool_call_request(id, tool, args), Some(session_id));
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
    assert!(
        result.get("result").is_some(),
        "no 'result' in response: {result}"
    );

    let server_info = &result["result"]["serverInfo"];
    assert_eq!(
        server_info["name"].as_str(),
        Some("pkb"),
        "server name mismatch: {server_info}"
    );
}

#[test]
fn test_stdio_stdout_purity() {
    let (_temp_dir, pkb_root, db_path) = create_test_pkb();

    let mut child = Command::new(pkb_binary())
        .env_remove("PKB_MCP_URL")
        .args([
            "--pkb-root",
            pkb_root.to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "mcp",
        ])
        .env("ACA_DATA", pkb_root.to_str().unwrap())
        .env("AOPS_DUMMY_EMBEDDER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_parent_death()
        .spawn()
        .expect("failed to spawn pkb mcp");

    // Write initialize request then immediately drop stdin (EOF)
    // The server processes the initialize request, emits the response, sees EOF, and exits cleanly.
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", initialize_request(1)).unwrap();
        stdin.flush().unwrap();
    }

    let start = Instant::now();
    let timeout = Duration::from_secs(30);
    while start.elapsed() < timeout {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
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
    let content_type = headers.get("content-type").expect("no Content-Type header");
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

    let tools = [
        ("graph_stats", json!({})),
        ("task_summary", json!({})),
        ("list_tasks", json!({"limit": 3})),
    ];

    for (i, (tool_name, args)) in tools.iter().enumerate() {
        let result = http_call_tool(
            server.port,
            &session_id,
            (i + 2) as u64,
            tool_name,
            args.clone(),
        );
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

    let (status, _, body) = http_post(server.port, &tools_list_request(2), Some(&session_id));
    assert_eq!(status, 200);

    let messages = parse_sse_messages(&body);
    assert!(!messages.is_empty(), "no SSE data in tools/list response");

    let tools = messages[0]["result"]["tools"]
        .as_array()
        .expect("tools/list result missing 'tools' array");

    // Should match the 16 published primitives
    assert_eq!(
        tools.len(),
        16,
        "expected 16 published tools from tools/list, got {}",
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
        (400..500).contains(&status),
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
        status == 404 || (400..500).contains(&status),
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
    let reindex = Command::new(pkb_binary()).env_remove("PKB_MCP_URL")
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
        .kill_on_parent_death()
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
    let mut cmd = Command::new(pkb_binary());
    cmd.env_remove("PKB_MCP_URL")
        .args([
            "--pkb-root",
            &pkb_root_str,
            "--db-path",
            &db_path_str,
            "mcp",
            "--http",
            "--port",
            "0",
        ])
        .env("AOPS_OFFLINE", "1")
        .stdin(Stdio::null());

    let (mut child, port) = spawn_http_mcp(cmd);
    let start = Instant::now();
    let timeout = Duration::from_secs(60);
    while start.elapsed() < timeout {
        if let Ok(stream) = std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        ) {
            drop(stream);
            std::thread::sleep(Duration::from_millis(200));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Forcibly target the local daemon

    #[allow(clippy::redundant_closure_call)]
    let result = (|| -> Value {
        let (status, headers, body) = http_post(port, &initialize_request(1), None);
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
                    if i == 0 {
                        "graph_stats"
                    } else {
                        "task_summary"
                    },
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

// ── SIGKILL-of-harness backstop test ────────────────────────────────────
//
// Acceptance criterion: a test SIGKILLs the harness process (not the child)
// and asserts zero surviving grandchildren. `Drop`-based cleanup (the
// existing `impl Drop for HttpServer`) provably cannot satisfy this — no
// destructor runs across a SIGKILL. This test re-execs the test binary as a
// "helper" process that starts a real `HttpServer` fixture and then parks
// forever (so the helper's own cleanup never runs either — mirroring an
// abnormal cargo-test death), prints the grandchild's pid, and the outer
// test SIGKILLs *that helper* — never the grandchild directly — then polls
// for the grandchild's death via the PR_SET_PDEATHSIG backstop installed in
// `spawn_http_mcp` above.
#[cfg(target_os = "linux")]
#[test]
fn test_grandchild_server_dies_when_harness_is_sigkilled() {
    const HELPER_ENV: &str = "__TEST_HARNESS_HELPER";
    const PID_MARKER: &str = "CHILD_PID=";

    if std::env::var_os(HELPER_ENV).is_some() {
        // Helper mode: start a real HttpServer fixture, announce its pid,
        // then park forever so this process's own cleanup glue never runs.
        let server = HttpServer::start();
        let child_pid = server.child.as_ref().expect("child present").id();
        println!("{PID_MARKER}{child_pid}");
        std::io::stdout().flush().ok();
        std::mem::forget(server);
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    let exe = std::env::current_exe().expect("current_exe");
    let mut helper = Command::new(&exe)
        .arg("--exact")
        .arg("test_grandchild_server_dies_when_harness_is_sigkilled")
        .arg("--nocapture")
        .env(HELPER_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn helper harness process");

    let mut reader = std::io::BufReader::new(helper.stdout.take().unwrap());
    let mut grandchild_pid: Option<i32> = None;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF: helper exited before announcing a pid
            Ok(_) => {
                if let Some(rest) = line.trim().strip_prefix(PID_MARKER) {
                    grandchild_pid = rest.parse().ok();
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let grandchild_pid = grandchild_pid
        .unwrap_or_else(|| panic!("helper never announced a grandchild pid ({PID_MARKER}...)"));

    assert!(
        pid_is_live(grandchild_pid),
        "grandchild pid {grandchild_pid} was not alive before the kill — test setup is broken"
    );

    // SIGKILL the HELPER (the simulated harness process). Never the
    // grandchild directly — that would prove nothing about the backstop.
    helper.kill().expect("SIGKILL of helper harness failed");
    helper.wait().ok();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut still_alive = true;
    while Instant::now() < deadline {
        if !pid_is_live(grandchild_pid) {
            still_alive = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if still_alive {
        // Best-effort cleanup so a failing assertion doesn't also leak the
        // process it was testing for.
        unsafe {
            libc::kill(grandchild_pid, libc::SIGKILL);
        }
    }

    assert!(
        !still_alive,
        "grandchild pkb process (pid {grandchild_pid}) survived SIGKILL of its parent harness — \
         PR_SET_PDEATHSIG backstop failed"
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
#[ignore = "PR_SET_PDEATHSIG is Linux-only; this platform has no equivalent backstop to verify"]
fn test_grandchild_server_dies_when_harness_is_sigkilled() {
    unreachable!("ignored on non-Linux platforms — see #[ignore] reason");
}

#[test]
fn test_http2_listener_serves_requests_with_keepalive() {
    let server = HttpServer::start();
    let port = server.port;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .expect("build client");

        let url = format!("http://127.0.0.1:{port}/mcp");

        // Request 1: Initialize session over HTTP/2
        let init_req = initialize_request(1);
        let resp1 = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(init_req)
            .send()
            .await
            .expect("failed to send initialize request");

        assert_eq!(
            resp1.version(),
            reqwest::Version::HTTP_2,
            "Expected HTTP/2 protocol, got {:?}",
            resp1.version()
        );
        assert_eq!(resp1.status(), reqwest::StatusCode::OK);

        let session_id = resp1
            .headers()
            .get("mcp-session-id")
            .expect("missing mcp-session-id header")
            .to_str()
            .expect("valid ascii session id")
            .to_string();

        // Drain the first response bytes
        let _ = resp1.bytes().await;

        // Request 2: Send initialized notification over the same keep-alive HTTP/2 connection
        let notif_req = jsonrpc_notification("notifications/initialized");
        let resp2 = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(notif_req)
            .send()
            .await
            .expect("failed to send notification");

        assert_eq!(
            resp2.version(),
            reqwest::Version::HTTP_2,
            "Expected HTTP/2 protocol on reused connection, got {:?}",
            resp2.version()
        );
        assert!(resp2.status().is_success());

        // Request 3: Tool call over the same keep-alive HTTP/2 connection
        let tool_req = tool_call_request(2, "pkb_status", json!({}));
        let resp3 = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(tool_req)
            .send()
            .await
            .expect("failed to send tool call");

        assert_eq!(
            resp3.version(),
            reqwest::Version::HTTP_2,
            "Expected HTTP/2 protocol on 3rd request, got {:?}",
            resp3.version()
        );
        assert_eq!(resp3.status(), reqwest::StatusCode::OK);

        let body3 = resp3.text().await.expect("read response body");
        assert!(
            body3.contains("\"name\": \"mem\"") || body3.contains("\"name\":\"mem\""),
            "Expected tool response to contain name 'mem', got: {body3}"
        );
        let expected_version = env!("CARGO_PKG_VERSION");
        assert!(
            body3.contains(&format!("\"version\": \"{expected_version}\""))
                || body3.contains(&format!("\"version\":\"{expected_version}\"")),
            "Expected tool response to contain version '{expected_version}', got: {body3}"
        );
    });
}

#[test]
fn test_http2_concurrent_multiplexed_tool_calls() {
    let server = HttpServer::start();
    let port = server.port;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build multi-thread runtime");

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .expect("build client");

        let url = format!("http://127.0.0.1:{port}/mcp");

        // Initialize session over HTTP/2
        let init_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(initialize_request(1))
            .send()
            .await
            .expect("init send");
        assert_eq!(init_resp.version(), reqwest::Version::HTTP_2);
        assert_eq!(init_resp.status(), reqwest::StatusCode::OK);

        let session_id = init_resp
            .headers()
            .get("mcp-session-id")
            .expect("session id")
            .to_str()
            .unwrap()
            .to_string();
        let _ = init_resp.bytes().await;

        // Notification
        let notif_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(jsonrpc_notification("notifications/initialized"))
            .send()
            .await
            .expect("notif send");
        assert!(notif_resp.status().is_success());

        // Spawn 10 concurrent requests multiplexed over the same HTTP/2 connection
        let mut tasks = Vec::new();
        for i in 0..10 {
            let client = client.clone();
            let url = url.clone();
            let session_id = session_id.clone();
            tasks.push(tokio::spawn(async move {
                let req = match i % 4 {
                    0 => tool_call_request(100 + i, "pkb_status", json!({})),
                    1 => tool_call_request(100 + i, "pkb_get_task", json!({"id": "task-test-01"})),
                    2 => tool_call_request(100 + i, "pkb_list_tasks", json!({"limit": 5})),
                    _ => tool_call_request(100 + i, "pkb_search", json!({"query": "Sample"})),
                };

                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("Mcp-Session-Id", &session_id)
                    .body(req)
                    .send()
                    .await
                    .expect("concurrent req failed");

                assert_eq!(
                    resp.version(),
                    reqwest::Version::HTTP_2,
                    "Task {i} did not use HTTP/2"
                );
                assert_eq!(resp.status(), reqwest::StatusCode::OK);
                let text = resp.text().await.expect("read text");
                assert!(
                    text.contains("\"result\"") || text.contains("\"structuredContent\""),
                    "Task {i} missing result payload: {text}"
                );
            }));
        }

        for (i, t) in tasks.into_iter().enumerate() {
            t.await.unwrap_or_else(|e| panic!("Task {i} panicked: {e}"));
        }
    });
}

#[test]
fn test_phases_1_to_3_concurrency_and_contracts() {
    let server = HttpServer::start();
    let port = server.port;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .expect("build client");

        let url = format!("http://127.0.0.1:{port}/mcp");

        // Initialize session over HTTP/2
        let init_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(initialize_request(1))
            .send()
            .await
            .expect("init send");
        assert_eq!(init_resp.version(), reqwest::Version::HTTP_2);

        let session_id = init_resp
            .headers()
            .get("mcp-session-id")
            .expect("session id")
            .to_str()
            .unwrap()
            .to_string();
        let _ = init_resp.bytes().await;

        let notif_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(jsonrpc_notification("notifications/initialized"))
            .send()
            .await
            .expect("notif send");
        assert!(notif_resp.status().is_success());

        // ── Phase 3 Contract: 16 tools, outputSchema, ttlMs ──
        let list_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(tools_list_request(2))
            .send()
            .await
            .expect("tools list send");
        assert_eq!(list_resp.status(), reqwest::StatusCode::OK);
        let list_text = list_resp.text().await.unwrap();
        let list_json: Value = serde_json::from_str(
            list_text
                .lines()
                .find(|l| l.starts_with("data: {"))
                .map(|l| &l[6..])
                .unwrap_or(&list_text),
        )
        .expect("parse list json");

        let tools = list_json["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 16, "Phase 3 contract: exactly 16 published tools");
        assert_eq!(
            list_json["result"]["_meta"]["ttlMs"].as_u64(),
            Some(86_400_000),
            "Phase 3 contract: 24h ttlMs"
        );
        for t in tools {
            assert!(
                t.get("outputSchema").is_some(),
                "Phase 1 & 3 contract: tool {} missing outputSchema",
                t["name"]
            );
        }

        // ── Phase 1 Contract: Zero-match sentinel ──
        let zero_search_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(tool_call_request(3, "pkb_search", json!({"query": "nonexistent_term_xyz_123"})))
            .send()
            .await
            .unwrap();
        let zero_text = zero_search_resp.text().await.unwrap();
        assert!(
            zero_text.contains("\"total\":0") || zero_text.contains("\"total\": 0"),
            "Phase 1 contract: zero matches return structured total:0. Got:\n{zero_text}"
        );

        // ── Phase 1 Contract: Batch reads with items and not_found ──
        let batch_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(tool_call_request(4, "pkb_get_task", json!({"ids": ["task-test-01", "missing-task-id"]})))
            .send()
            .await
            .unwrap();
        let batch_text = batch_resp.text().await.unwrap();
        assert!(
            batch_text.contains("\"items\"") && batch_text.contains("\"not_found\""),
            "Phase 1 contract: batch read returns items and not_found. Got:\n{batch_text}"
        );
        assert!(
            batch_text.contains("missing-task-id"),
            "Phase 1 contract: not_found contains missing id"
        );

        // ── Phase 2 Contract: List projections and budget_ms ──
        let proj_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .body(tool_call_request(5, "pkb_list_tasks", json!({
                "fields": ["body_excerpt", "relationship_summary"],
                "budget_ms": 5000
            })))
            .send()
            .await
            .unwrap();
        let proj_text = proj_resp.text().await.unwrap();
        assert!(
            proj_text.contains("\"timed_out\""),
            "Phase 2 contract: budget_ms response returns timed_out field. Got:\n{proj_text}"
        );

        // ── Phase 3 Contract: Retired tool shims under concurrency ──
        let retired_tools = vec!["get_stats", "refresh_graph", "task_summary"];
        let mut shim_handles = Vec::new();
        for (idx, tool_name) in retired_tools.into_iter().enumerate() {
            let client = client.clone();
            let url = url.clone();
            let session_id = session_id.clone();
            shim_handles.push(tokio::spawn(async move {
                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("Mcp-Session-Id", &session_id)
                    .body(tool_call_request(200 + idx as u64, tool_name, json!({})))
                    .send()
                    .await
                    .expect("shim request send");
                assert_eq!(resp.status(), reqwest::StatusCode::OK);
                let text = resp.text().await.unwrap();
                assert!(
                    !text.contains("Method not found"),
                    "Shim for {tool_name} failed: {text}"
                );
            }));
        }

        for h in shim_handles {
            h.await.unwrap();
        }
    });
}

