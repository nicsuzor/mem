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

fn stdio_session_sequential(aca_path: &std::path::Path, messages: &[String]) -> Vec<Value> {
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
    let mut reader = std::io::BufReader::new(stdout_handle);

    let mut responses = Vec::new();

    for msg in messages {
        stdin.write_all(msg.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();

        // If it's a notification, don't wait for a response
        if !msg.contains("\"id\"") {
            continue;
        }

        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if let Ok(val) = serde_json::from_str::<Value>(&line) {
            responses.push(val);
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
