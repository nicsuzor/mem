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

fn protocol_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "integration-test", "version": "0.1"}
    })
}

fn discover_request(id: u64) -> String {
    jsonrpc_request(id, "server/discover", json!({ "_meta": protocol_meta() }))
}

fn legacy_initialize_request(id: u64) -> String {
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

fn tool_call_request_with_args(id: u64, name: &str, args: Value) -> String {
    jsonrpc_request(
        id,
        "tools/call",
        json!({"name": name, "arguments": args, "_meta": protocol_meta()}),
    )
}

fn tool_call_request(id: u64, name: &str, mut args: Value) -> String {
    if let Some(obj) = args.as_object_mut() {
        obj.entry("session_id".to_string())
            .or_insert_with(|| json!("test-integration-session"));
    }
    tool_call_request_with_args(id, name, args)
}

fn tools_list_request(id: u64) -> String {
    jsonrpc_request(id, "tools/list", json!({ "_meta": protocol_meta() }))
}

fn prompts_list_request(id: u64) -> String {
    jsonrpc_request(id, "prompts/list", json!({ "_meta": protocol_meta() }))
}

fn prompt_get_request(id: u64, name: &str, args: Value) -> String {
    jsonrpc_request(
        id,
        "prompts/get",
        json!({"name": name, "arguments": args, "_meta": protocol_meta()}),
    )
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

/// Raw HTTP POST to any route. Returns (status_code, headers, body).
fn http_post_route(
    port: u16,
    route: &str,
    body: &str,
    headers_extra: &[(&str, &str)],
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
        "POST {route} HTTP/1.1\r\n\
         Host: {host}:{actual_port}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers_extra {
        request.push_str(&format!("{k}: {v}\r\n"));
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

/// Raw HTTP POST to /mcp endpoint with MCP 2026-07-28 headers.
fn http_post_mcp(
    port: u16,
    body: &str,
    method: &str,
    name: Option<&str>,
    extra: &[(&str, &str)],
) -> (u16, HashMap<String, String>, String) {
    let mut headers = vec![
        ("MCP-Protocol-Version", "2026-07-28"),
        ("Mcp-Method", method),
    ];
    if let Some(n) = name {
        headers.push(("Mcp-Name", n));
    }
    headers.extend_from_slice(extra);
    http_post_route(port, "/mcp", body, &headers)
}

/// Discover server metadata over stateless /mcp.
fn http_discover(port: u16) -> Value {
    let (status, headers, body) =
        http_post_mcp(port, &discover_request(1), "server/discover", None, &[]);
    assert_eq!(
        status, 200,
        "server/discover failed with status {status}. Body: {body}"
    );
    assert!(
        !headers.contains_key("mcp-session-id"),
        "sessionless server must not emit Mcp-Session-Id: {headers:?}"
    );
    let resp: Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("invalid JSON from server/discover: {e}\nBody:\n{body}"));
    assert!(
        resp.get("result").is_some(),
        "server/discover missing 'result': {resp}"
    );
    resp
}

/// Call a tool statelessly over HTTP /mcp (no session handshake required).
fn http_call_tool(port: u16, id: u64, tool: &str, args: Value) -> Value {
    let (status, headers, body) = http_post_mcp(
        port,
        &tool_call_request(id, tool, args),
        "tools/call",
        Some(tool),
        &[],
    );
    assert!(
        status == 200 || status == 404,
        "tool call '{tool}' returned unexpected status {status}. Body:\n{body}"
    );
    assert!(
        !headers.contains_key("mcp-session-id"),
        "tool call response must not set Mcp-Session-Id: {headers:?}"
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("invalid JSON from tool call '{tool}': {e}\nBody:\n{body}"))
}

// ── Stdio tests ──────────────────────────────────────────────────────────

#[test]
fn test_stdio_discover() {
    let responses = stdio_session(&[discover_request(1)]);
    assert!(!responses.is_empty(), "no response from stdio server/discover");

    let result = &responses[0];
    assert!(
        result.get("result").is_some(),
        "no 'result' in response: {result}"
    );

    let server_info = &result["result"]["_meta"]["io.modelcontextprotocol/serverInfo"];
    assert_eq!(
        server_info["name"].as_str(),
        Some("pkb"),
        "server name mismatch: {server_info}"
    );
    assert_eq!(
        result["result"]["supportedVersions"],
        json!(["2026-07-28"]),
        "unexpected supportedVersions: {result}"
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

    // Write discover request then immediately drop stdin (EOF)
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", discover_request(1)).unwrap();
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
        discover_request(1),
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

#[test]
fn test_stdio_prompts_list_and_get() {
    let responses = stdio_session(&[
        discover_request(1),
        prompts_list_request(2),
        prompt_get_request(3, "find-task", json!({"query": "integration testing"})),
        prompt_get_request(4, "task-status", json!({"id": "task-test-01"})),
    ]);

    // 1. Verify prompts/list (id=2)
    let list_response = responses
        .iter()
        .find(|r| r.get("id") == Some(&json!(2)))
        .expect("no response with id=2 for prompts/list");

    assert!(
        list_response.get("result").is_some(),
        "prompts/list returned error: {list_response}"
    );
    let prompts = list_response["result"]["prompts"]
        .as_array()
        .expect("prompts array in prompts/list result");

    let prompt_names: Vec<&str> = prompts
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
        .collect();

    assert!(prompt_names.contains(&"find-task"));
    assert!(prompt_names.contains(&"explore-topic"));
    assert!(prompt_names.contains(&"navigate-graph"));
    assert!(prompt_names.contains(&"find-by-tag"));
    assert!(prompt_names.contains(&"task-status"));
    assert!(prompt_names.contains(&"ready-tasks"));
    assert!(prompt_names.contains(&"task-dependencies"));
    assert!(!prompt_names.contains(&"polecat-compose"));

    // 2. Verify prompts/get find-task (id=3)
    let get_find_task = responses
        .iter()
        .find(|r| r.get("id") == Some(&json!(3)))
        .expect("no response with id=3 for prompts/get find-task");

    assert!(
        get_find_task.get("result").is_some(),
        "find-task returned error: {get_find_task}"
    );
    let messages = get_find_task["result"]["messages"]
        .as_array()
        .expect("messages array in get_prompt result");
    assert!(!messages.is_empty());
    let text = messages[0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("I want to find a task about 'integration testing'"));
    assert!(text.contains("task_search"));
    assert!(text.contains("get_task"));

    // 3. Verify prompts/get task-status (id=4)
    let get_status = responses
        .iter()
        .find(|r| r.get("id") == Some(&json!(4)))
        .expect("no response with id=4 for prompts/get task-status");

    assert!(
        get_status.get("result").is_some(),
        "task-status returned error: {get_status}"
    );
    let status_messages = get_status["result"]["messages"]
        .as_array()
        .expect("messages array in task-status result");
    assert!(!status_messages.is_empty());
    let status_text = status_messages[0]["content"]["text"].as_str().unwrap();
    assert!(status_text.contains("task-test-01"));
    assert!(status_text.contains("get_task_status"));
}

// ── HTTP/SSE tests ───────────────────────────────────────────────────────

#[test]
fn test_http_server_starts() {
    let mut server = HttpServer::start();
    assert!(server.is_alive(), "server died after start");
    let _ = http_discover(server.port);
}

#[test]
fn test_http_discover() {
    let server = HttpServer::start();
    let (status, headers, body) =
        http_post_mcp(server.port, &discover_request(1), "server/discover", None, &[]);

    assert_eq!(status, 200, "discover status {status}. Body:\n{body}");

    let content_type = headers.get("content-type").expect("no Content-Type header");
    assert!(
        content_type.contains("application/json"),
        "expected application/json, got: {content_type}"
    );
    assert!(
        !headers.contains_key("mcp-session-id"),
        "stateless server must not emit Mcp-Session-Id. Headers: {headers:?}"
    );

    let msg: Value = serde_json::from_str(&body).expect("valid JSON discover response");
    let result = &msg["result"];
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"].as_str(),
        Some("pkb"),
        "server name mismatch: {result}"
    );
    assert_eq!(
        result["supportedVersions"],
        json!(["2026-07-28"]),
        "unexpected supportedVersions: {result}"
    );
    assert_eq!(result["ttlMs"].as_u64(), Some(300_000));
    assert_eq!(result["cacheScope"].as_str(), Some("public"));
    let instructions = result["instructions"]
        .as_str()
        .expect("discover must return server instructions");
    assert!(
        instructions.contains("session_id"),
        "instructions must tell agents to pass session_id: {instructions}"
    );
}

#[test]
fn test_http_legacy_initialize_and_duplicate_routes_rejected() {
    let mut server = HttpServer::start();

    // Legacy initialize without 2026-07-28 handshake support is rejected
    let (init_status, _, init_body) = http_post_route(
        server.port,
        "/mcp",
        &legacy_initialize_request(1),
        &[("Mcp-Method", "initialize")],
    );
    assert!(
        (400..500).contains(&init_status) || init_body.contains("\"error\""),
        "legacy initialize must be rejected; status={init_status}, body={init_body}"
    );

    // Duplicate stateless/sessionless routes must not exist (404)
    for removed_route in ["/mcp/stateless", "/mcp/sessionless", "/stateless", "/sessionless"] {
        let (st, _, _) = http_post_route(
            server.port,
            removed_route,
            &discover_request(1),
            &[
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "server/discover"),
            ],
        );
        assert_eq!(
            st, 404,
            "removed route {removed_route} must return 404, got {st}"
        );
    }
    assert!(server.is_alive());
}

#[test]
fn test_http_direct_stateless_tool_call_and_headers() {
    let server = HttpServer::start();

    // Direct tools/call on /mcp WITHOUT any prior discover/initialize or Mcp-Session-Id
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent_span_id = "00f067aa0ba902b7";
    let traceparent = format!("00-{trace_id}-{parent_span_id}-01");
    let (status, headers, body) = http_post_mcp(
        server.port,
        &tool_call_request_with_args(1, "search", json!({"query": "alpha"})),
        "tools/call",
        Some("search"),
        &[
            ("x-session-id", "test-stateless-header-session"),
            ("traceparent", &traceparent),
        ],
    );

    assert_eq!(
        status, 200,
        "/mcp stateless call must return 200 OK without prior handshake, got {status}. Body: {body}"
    );
    let content_type = headers.get("content-type").expect("Content-Type header");
    assert!(
        content_type.contains("application/json"),
        "expected application/json, got {content_type}"
    );
    assert!(
        !headers.contains_key("mcp-session-id"),
        "sessionless server must not issue Mcp-Session-Id: {headers:?}"
    );

    let parsed: Value = serde_json::from_str(&body).expect("valid JSON response");
    assert!(
        parsed.get("result").is_some(),
        "expected result in response: {parsed}"
    );

    // Sending Mcp-Session-Id from a stale client never establishes or persists a session,
    // and without explicit session_id a subsequent call is still rejected with -32602.
    let (hdr_status, hdr_headers, _) = http_post_mcp(
        server.port,
        &tool_call_request(2, "graph_stats", json!({})),
        "tools/call",
        Some("graph_stats"),
        &[("Mcp-Session-Id", "stale-client-session")],
    );
    assert_eq!(hdr_status, 200);
    assert!(
        !hdr_headers.contains_key("mcp-session-id"),
        "sessionless server must never return Mcp-Session-Id"
    );
    let (unattr_status, _, unattr_body) = http_post_mcp(
        server.port,
        &tool_call_request_with_args(3, "graph_stats", json!({})),
        "tools/call",
        Some("graph_stats"),
        &[("Mcp-Session-Id", "stale-client-session")],
    );
    assert_eq!(unattr_status, 200);
    let unattr_json: Value = serde_json::from_str(&unattr_body).expect("valid JSON-RPC error");
    assert_eq!(
        unattr_json["error"]["code"].as_i64(),
        Some(-32602),
        "Mcp-Session-Id must not bind or substitute for caller session_id: {unattr_json}"
    );
}

#[test]
fn test_http_multiple_tools() {
    let server = HttpServer::start();

    let tools = [
        ("graph_stats", json!({})),
        ("task_summary", json!({})),
        ("list_tasks", json!({"limit": 3})),
    ];

    for (i, (tool_name, args)) in tools.iter().enumerate() {
        let result = http_call_tool(server.port, (i + 2) as u64, tool_name, args.clone());
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

    let (status, _, body) =
        http_post_mcp(server.port, &tools_list_request(2), "tools/list", None, &[]);
    assert_eq!(status, 200, "tools/list returned {status}: {body}");

    let msg: Value = serde_json::from_str(&body).expect("valid JSON from tools/list");
    let tools = msg["result"]["tools"]
        .as_array()
        .expect("tools/list result missing 'tools' array");

    assert_eq!(
        tools.len(),
        18,
        "expected 18 published tools from tools/list, got {}",
        tools.len()
    );
    assert_eq!(msg["result"]["ttlMs"].as_u64(), Some(86_400_000));
    assert_eq!(msg["result"]["cacheScope"].as_str(), Some("public"));
    assert!(
        tools.iter().any(|t| t["name"] == "get_task_status"),
        "expected get_task_status to be in published tools"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "convert_document"),
        "expected convert_document in HTTP tools/list response"
    );
    for t in tools {
        let req = t["inputSchema"]["required"]
            .as_array()
            .expect("required array");
        assert!(
            req.iter().any(|v| v.as_str() == Some("session_id")),
            "tool {} missing required session_id",
            t["name"]
        );
        assert_eq!(
            t["annotations"]["openWorldHint"],
            json!(false),
            "tool {} must mark openWorldHint=false",
            t["name"]
        );
    }
}

#[test]
fn test_http_prompts_list_and_get() {
    let server = HttpServer::start();

    // 1. prompts/list over stateless HTTP
    let (status, _, body) =
        http_post_mcp(server.port, &prompts_list_request(2), "prompts/list", None, &[]);
    assert_eq!(status, 200, "prompts/list returned {status}: {body}");

    let msg: Value = serde_json::from_str(&body).expect("valid JSON from prompts/list");
    let prompts = msg["result"]["prompts"]
        .as_array()
        .expect("prompts/list result missing 'prompts' array");

    let prompt_names: Vec<&str> = prompts
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
        .collect();

    assert!(prompt_names.contains(&"find-task"));
    assert!(prompt_names.contains(&"ready-tasks"));
    assert!(prompt_names.contains(&"task-status"));

    // 2. prompts/get over stateless HTTP
    let (status, _, body) = http_post_mcp(
        server.port,
        &prompt_get_request(3, "ready-tasks", json!({"limit": 5, "project": "mem"})),
        "prompts/get",
        Some("ready-tasks"),
        &[],
    );
    assert_eq!(status, 200, "prompts/get returned {status}: {body}");

    let msg: Value = serde_json::from_str(&body).expect("valid JSON from prompts/get");
    let prompt_result = &msg["result"];
    let prompt_messages = prompt_result["messages"]
        .as_array()
        .expect("messages array in prompts/get response");
    assert!(!prompt_messages.is_empty());
    let text = prompt_messages[0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("Find unblocked tasks ready for work in priority order"));
    assert!(text.contains("list_tasks"));
    assert!(text.contains("Project filter: \"mem\""));
}

#[test]
fn test_http_error_missing_protocol_metadata_and_missing_session_id() {
    let mut server = HttpServer::start();

    // Missing MCP-Protocol-Version / _meta protocol metadata rejected at transport layer
    let (meta_status, _, _) = http_post_route(
        server.port,
        "/mcp",
        &jsonrpc_request(98, "tools/list", json!({})),
        &[],
    );
    assert!(
        (400..500).contains(&meta_status),
        "expected 4xx for missing protocol metadata, got {meta_status}"
    );

    // Missing tool session_id rejected with JSON-RPC INVALID_PARAMS (-32602) on reads & writes
    let (status, _, body) = http_post_mcp(
        server.port,
        &tool_call_request_with_args(99, "search", json!({"query": "alpha"})),
        "tools/call",
        Some("search"),
        &[],
    );
    assert_eq!(status, 200, "JSON-RPC error expected; status={status}, body={body}");
    let resp: Value = serde_json::from_str(&body).expect("valid JSON-RPC error");
    assert_eq!(resp["error"]["code"].as_i64(), Some(-32602));
    assert!(server.is_alive(), "server crashed after missing session_id");
}

#[test]
fn test_http_error_malformed_json() {
    let mut server = HttpServer::start();

    let (status, _, _) = http_post_mcp(
        server.port,
        "{not valid json at all!!",
        "tools/call",
        Some("search"),
        &[],
    );

    assert!(
        status >= 400,
        "expected error status for malformed JSON, got {status}"
    );
    assert!(server.is_alive(), "server crashed after malformed JSON");
}

#[test]
fn test_http_error_unknown_tool() {
    let server = HttpServer::start();

    let result = http_call_tool(
        server.port,
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
        http_call_tool(
            port,
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
fn test_http_concurrent_stateless_calls() {
    let server = HttpServer::start();

    let port = server.port;
    let handles: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let result = http_call_tool(
                    port,
                    2 + i,
                    if i == 0 {
                        "graph_stats"
                    } else {
                        "task_summary"
                    },
                    json!({ "session_id": format!("concurrent-sess-{i}") }),
                );
                (i, result)
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    for (i, result) in results {
        assert!(
            result.get("result").is_some(),
            "concurrent stateless call {i} failed: {result}"
        );
    }
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

        // Request 1: Discover server over HTTP/2
        let resp1 = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "server/discover")
            .body(discover_request(1))
            .send()
            .await
            .expect("failed to send discover request");

        assert_eq!(
            resp1.version(),
            reqwest::Version::HTTP_2,
            "Expected HTTP/2 protocol, got {:?}",
            resp1.version()
        );
        assert_eq!(resp1.status(), reqwest::StatusCode::OK);
        assert!(!resp1.headers().contains_key("mcp-session-id"));
        let _ = resp1.bytes().await;

        // Request 2: Tool call over the same keep-alive HTTP/2 connection without session header
        let tool_req = tool_call_request(2, "pkb_status", json!({}));
        let resp2 = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "pkb_status")
            .body(tool_req)
            .send()
            .await
            .expect("failed to send tool call");

        assert_eq!(
            resp2.version(),
            reqwest::Version::HTTP_2,
            "Expected HTTP/2 protocol on reused connection, got {:?}",
            resp2.version()
        );
        assert_eq!(resp2.status(), reqwest::StatusCode::OK);

        let body2 = resp2.text().await.expect("read response body");
        assert!(
            body2.contains("\"name\": \"mem\"") || body2.contains("\"name\":\"mem\""),
            "Expected tool response to contain name 'mem', got: {body2}"
        );
        let expected_version = env!("CARGO_PKG_VERSION");
        assert!(
            body2.contains(&format!("\"version\": \"{expected_version}\""))
                || body2.contains(&format!("\"version\":\"{expected_version}\"")),
            "Expected tool response to contain version '{expected_version}', got: {body2}"
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

        // Spawn 10 concurrent stateless requests multiplexed over the same HTTP/2 connection
        let mut tasks = Vec::new();
        for i in 0..10 {
            let client = client.clone();
            let url = url.clone();
            tasks.push(tokio::spawn(async move {
                let (name, req) = match i % 4 {
                    0 => ("pkb_status", tool_call_request(100 + i, "pkb_status", json!({}))),
                    1 => (
                        "pkb_get_task",
                        tool_call_request(100 + i, "pkb_get_task", json!({"id": "task-test-01"})),
                    ),
                    2 => (
                        "pkb_list_tasks",
                        tool_call_request(100 + i, "pkb_list_tasks", json!({"limit": 5})),
                    ),
                    _ => (
                        "pkb_search",
                        tool_call_request(100 + i, "pkb_search", json!({"query": "Sample"})),
                    ),
                };

                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", "2026-07-28")
                    .header("Mcp-Method", "tools/call")
                    .header("Mcp-Name", name)
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

        // ── Phase 3 Contract: 18 tools, outputSchema, ttlMs, cacheScope ──
        let list_resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/list")
            .body(tools_list_request(2))
            .send()
            .await
            .expect("tools list send");
        assert_eq!(list_resp.status(), reqwest::StatusCode::OK);
        let list_text = list_resp.text().await.unwrap();
        let list_json: Value = serde_json::from_str(&list_text).expect("parse list json");

        let tools = list_json["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 18, "Phase 3 contract: exactly 18 published tools");
        assert_eq!(
            list_json["result"]["ttlMs"].as_u64(),
            Some(86_400_000),
            "Phase 3 contract: 24h ttlMs"
        );
        assert_eq!(
            list_json["result"]["cacheScope"].as_str(),
            Some("public"),
            "Phase 3 contract: public cacheScope"
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
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "pkb_search")
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
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "pkb_get_task")
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
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "pkb_list_tasks")
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
            shim_handles.push(tokio::spawn(async move {
                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", "2026-07-28")
                    .header("Mcp-Method", "tools/call")
                    .header("Mcp-Name", tool_name)
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

