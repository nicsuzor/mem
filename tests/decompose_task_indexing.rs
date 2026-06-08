use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn pkb_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = manifest.join("target/release/pkb");
    if release.exists() {
        return release;
    }
    let debug = manifest.join("target/debug/pkb");
    if debug.exists() {
        return debug;
    }
    PathBuf::from("pkb")
}

fn seed_pkb() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let projects_dir = tmp.path().join("projects");
    std::fs::create_dir_all(&projects_dir).unwrap();

    let project_md = projects_dir.join("proj-realdead.md");
    std::fs::write(
        &project_md,
        "---\n\
         id: proj-realdead\n\
         title: \"Real Project\"\n\
         type: project\n\
         status: active\n\
         priority: 2\n\
         project: aops\n\
         ---\n\n# Real Project\n",
    )
    .unwrap();

    let tasks_dir = tmp.path().join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    let parent_md = tasks_dir.join("task-parent.md");
    std::fs::write(
        &parent_md,
        "---\n\
         id: task-parent\n\
         title: \"Parent Task\"\n\
         type: task\n\
         status: active\n\
         priority: 2\n\
         project: aops\n\
         parent: proj-realdead\n\
         ---\n\n# Parent Task\n",
    )
    .unwrap();

    tmp
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

/// Drive the `pkb mcp` server over stdio, one request/response at a time.
///
/// Requests are sent interleaved — each id-bearing request is written and its
/// response awaited before the next request is sent. That ordering is
/// load-bearing for this test: `decompose_task` must finish indexing the new
/// subtask before the following `get_task` runs, and the request round-trip is
/// what lets indexing settle. (Batching all requests up front reintroduces a
/// "Task not found" race.)
///
/// Robust against the two flake modes the naive "one `read_line` per request"
/// loop suffered from:
///   1. A non-JSON line on stdout (e.g. a log line) used to be consumed against
///      a request slot, dropping that request's real response and misaligning
///      every later index. Here a background reader keeps only JSON-RPC
///      response objects (those carrying an `id`) and matches them to the
///      awaited request id; everything else is skipped.
///   2. A transient early EOF used to `break` and silently return a short
///      vector, surfacing later as an opaque `responses[1]` out-of-bounds
///      panic. Here each await is bounded by a timeout and, on EOF/timeout,
///      panics with the server's captured stderr so the failure is
///      diagnosable instead of an index panic.
fn stdio_session_sequential(aca_path: &std::path::Path, messages: &[String]) -> Vec<Value> {
    use std::collections::HashMap;
    use std::sync::{mpsc, Arc, Mutex};

    let mut child = Command::new(pkb_binary())
        .args(["mcp"])
        .env("ACA_DATA", aca_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn pkb mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout_handle = child.stdout.take().unwrap();
    let stderr_handle = child.stderr.take().unwrap();

    // Drain stderr in the background so a server-side panic is captured.
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    {
        let buf = stderr_buf.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::BufReader::new(stderr_handle).read_to_string(&mut s);
            *buf.lock().unwrap() = s;
        });
    }

    // Background reader: forward every JSON-RPC response object (those carrying
    // an `id`) to the channel; blank and non-JSON lines are skipped.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout_handle);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                if val.get("id").is_some() && tx.send(val).is_err() {
                    break;
                }
            }
        }
    });

    let timeout = Duration::from_secs(30);
    let mut pending: HashMap<u64, Value> = HashMap::new();
    let mut responses = Vec::new();

    for msg in messages {
        stdin.write_all(msg.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();

        // Notifications (no `id`) get no response.
        let want_id = serde_json::from_str::<Value>(msg)
            .ok()
            .and_then(|v| v.get("id").and_then(|id| id.as_u64()));
        let Some(want_id) = want_id else { continue };

        // Await this request's response, buffering any out-of-order arrivals.
        loop {
            if let Some(val) = pending.remove(&want_id) {
                responses.push(val);
                break;
            }
            match rx.recv_timeout(timeout) {
                Ok(val) => {
                    if let Some(id) = val.get("id").and_then(|id| id.as_u64()) {
                        pending.insert(id, val);
                    }
                }
                Err(_) => {
                    let err = stderr_buf.lock().unwrap().clone();
                    child.kill().ok();
                    child.wait().ok();
                    panic!(
                        "stdio session: no response for request id {want_id} within {timeout:?} \
                         (got {} of the requests answered).\n--- pkb stderr ---\n{err}",
                        responses.len()
                    );
                }
            }
        }
    }

    drop(stdin);
    child.kill().ok();
    child.wait().ok();

    responses
}

#[test]
fn test_decompose_task_indexing() {
    let pkb = seed_pkb();

    let messages = vec![
        initialize_request(1),
        jsonrpc_notification("notifications/initialized"),
        tool_call_request(
            2,
            "decompose_task",
            json!({
                "parent_id": "task-parent",
                "subtasks": [
                    {
                        "title": "Subtask 1",
                        "id": "task-sub1"
                    }
                ]
            }),
        ),
        tool_call_request(
            3,
            "get_task",
            json!({
                "id": "task-sub1"
            }),
        ),
        tool_call_request(
            4,
            "get_task_children",
            json!({
                "id": "task-parent"
            }),
        )
    ];

    let responses = stdio_session_sequential(pkb.path(), &messages);

    let decomp_res = &responses[1];
    assert!(
        decomp_res.get("result").is_some(),
        "decompose_task failed: {:?}", decomp_res
    );

    let get_res = &responses[2];
    assert!(
        get_res.get("result").is_some(),
        "get_task failed: {:?}", get_res
    );
    let text = get_res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Subtask 1"), "task body should contain title");

    let children_res = &responses[3];
    assert!(
        children_res.get("result").is_some(),
        "get_task_children failed: {:?}", children_res
    );
    let children_text = children_res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(children_text.contains("task-sub1"), "children response should contain the subtask ID");
}
