//! Default-level log output of `pkb mcp --http` (the Docker deployment).
//!
//! At the default filter (no `RUST_LOG`) a transaction — one MCP tool call —
//! emits exactly one INFO summary line (`pkb::tool_call`). Per-request and
//! per-write chatter (rmcp session handshakes, vector-store saves, git
//! commits, create_task key dumps, ad-hoc grouping decisions) is DEBUG.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const STARTUP_MARKER: &str = "Starting MCP HTTP/SSE server on http://";

struct Server {
    child: Child,
    port: u16,
    stderr: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(ok, "git {args:?} failed");
}

fn start_server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::write(
        root.join("tasks/task-log-01.md"),
        "---\nid: task-log-01\ntitle: Log level fixture\ntype: task\nstatus: ready\n---\n\nBody.\n",
    )
    .unwrap();
    // The Docker brain volume is a git checkout; exercise the commit path.
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    let db = root.join("vectors.bin");

    let mut child = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .env_remove("RUST_LOG")
        .env_remove("PKB_MCP_URL")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env("ACA_DATA", &root)
        .env("AOPS_DUMMY_EMBEDDER", "1")
        .args(["--pkb-root", root.to_str().unwrap(), "--db-path", db.to_str().unwrap()])
        .args(["mcp", "--http", "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pkb mcp --http");

    let lines = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = lines.clone();
    let reader = BufReader::new(child.stderr.take().unwrap());
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            let line = strip_ansi(&line);
            if let Some(i) = line.find(STARTUP_MARKER) {
                let rest = &line[i + STARTUP_MARKER.len()..];
                let hostport = rest.split("/mcp").next().unwrap_or("");
                if let Some(p) = hostport.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                    let _ = tx.send(p);
                }
            }
            sink.lock().unwrap().push(line);
        }
    });
    let port = rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| {
        panic!("server did not start:\n{}", lines.lock().unwrap().join("\n"))
    });
    Server { child, port, stderr: lines, _dir: dir }
}

fn post(port: u16, body: &Value, session: Option<&str>) -> (Option<String>, String) {
    let body = body.to_string();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(sid) = session {
        req.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(&body);
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    let resp = String::from_utf8_lossy(&resp).to_string();
    let (head, rest) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
    let sid = head.lines().find_map(|l| {
        let (k, v) = l.split_once(": ")?;
        k.eq_ignore_ascii_case("mcp-session-id").then(|| v.trim().to_string())
    });
    (sid, rest.to_string())
}

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Run a session of `calls` tool calls; return stderr lines emitted after startup.
fn run_session(calls: &[(&str, Value, bool)]) -> Vec<String> {
    let server = start_server();
    let init = rpc(
        1,
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {},
               "clientInfo": {"name": "log-level-test", "version": "0.1"}}),
    );
    let (sid, _) = post(server.port, &init, None);
    let sid = sid.expect("no Mcp-Session-Id");
    post(
        server.port,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        Some(&sid),
    );
    for (i, (name, args, expects_error)) in calls.iter().enumerate() {
        let (_, body) = post(
            server.port,
            &rpc(10 + i as u64, "tools/call", json!({"name": name, "arguments": args})),
            Some(&sid),
        );
        if *expects_error {
            assert!(body.contains("\"error\""), "{name} returned no error: {body}");
        } else {
            assert!(body.contains("\"result\""), "{name} returned no result: {body}");
        }
    }
    // Let async log lines (post-response) flush.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let count = server.stderr.lock().unwrap().iter().filter(|l| level_of(l) == Some("INFO") && l.contains("pkb::tool_call")).count();
        if count >= calls.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let lines = server.stderr.lock().unwrap().clone();
    let start = lines
        .iter()
        .position(|l| l.contains(STARTUP_MARKER))
        .expect("startup marker");
    lines[start + 1..].to_vec()
}

fn level_of(line: &str) -> Option<&'static str> {
    ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"]
        .into_iter()
        .find(|lvl| line.split_whitespace().nth(1) == Some(*lvl))
}

fn calls() -> Vec<(&'static str, Value, bool)> {
    vec![
        ("get_task", json!({"id": "task-log-01"}), false),
        ("search", json!({"query": "fixture"}), false),
        (
            "create_task",
            json!({"task_title": "Logged write", "parent": "task-log-01",
                   "body": "A write transaction."}),
            false
        ),
        ("get_task", json!({"id": "missing-id-123"}), true),
        (
            "release_task",
            json!({"task_title": "some unrelated task", "reason": "unreachable", "status": "done"}),
            true
        ),
    ]
}

#[test]
fn default_level_hides_per_request_noise() {
    let lines = run_session(&calls());
    let dump = lines.join("\n");
    for noisy in [
        "Service initialized as server",
        "received notification",
        "client initialized",
        "create new session",
        "Enqueueing task for tool call",
        "Saved vector store",
        "git commit succeeded",
        "create_task invocation",
        "adhoc_grouping",
    ] {
        assert!(
            !lines.iter().any(|l| level_of(l) == Some("INFO") && l.contains(noisy)),
            "`{noisy}` still logged at INFO by default:\n{dump}"
        );
    }
    let stray: Vec<_> = lines
        .iter()
        .filter(|l| level_of(l) == Some("INFO") && !l.contains("pkb::tool_call"))
        .collect();
    assert!(stray.is_empty(), "non-summary INFO lines after startup:\n{stray:#?}\n---\n{dump}");
}

#[test]
fn each_tool_call_emits_one_info_summary() {
    let calls = calls();
    let lines = run_session(&calls);
    let dump = lines.join("\n");
    let summaries: Vec<_> = lines
        .iter()
        .filter(|l| level_of(l) == Some("INFO") && l.contains("pkb::tool_call"))
        .collect();
    assert_eq!(summaries.len(), calls.len(), "expected one summary per call:\n{dump}");
    for ((name, _, expects_error), line) in calls.iter().zip(&summaries) {
        assert!(line.contains(&format!("tool={name}")), "{line}");
        if *expects_error {
            assert!(line.contains("status=error"), "{line}");
        } else {
            assert!(line.contains("status=ok"), "{line}");
        }
        assert!(line.contains("latency_ms="), "{line}");
    }
}
