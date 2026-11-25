//! Transport-level integration tests verifying `search` extract placement
//! and single-copy wire encoding over the MCP stdio JSON-RPC transport.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

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

fn pkb_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pkb"))
}

const EARLY_MARKER: &str = "EARLY_MARKER_ALPHA_917";
const LATE_MARKER: &str = "LATE_BODY_MARKER_OMEGA_482";

fn create_seeded_pkb() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir for test PKB");
    let pkb_root = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(pkb_root.join("notes")).expect("create notes dir");

    // Paragraph 1 exceeds the 300-char snippet cap so snippet truncates before Paragraph 2.
    let body = format!(
        "{EARLY_MARKER} — This opening paragraph establishes the primary retrieval fixture \
         context with enough detailed prose to exceed the three hundred character snippet \
         truncation boundary cleanly, ensuring that short snippet extracts end before the \
         second paragraph begins during transport verification runs.\n\n\
         Second paragraph continues with additional technical context and carries {LATE_MARKER} \
         well past the snippet cutoff so full and chunk extracts include it while snippet omits it.\n"
    );

    let note = format!(
        "---\nid: note-hydrogen-extract\ntitle: Hydrogen Extract Transport Fixture\ntype: note\ntags:\n  - transport-test\n---\n\n{body}"
    );
    std::fs::write(pkb_root.join("notes").join("note-hydrogen-extract.md"), note)
        .expect("write seeded note");

    let db_path = pkb_root.join("test_vectors.bin");

    // Populate the vector store for the seeded PKB using the dummy embedder.
    let reindex = Command::new(pkb_binary())
        .env_remove("PKB_MCP_URL")
        .args([
            "--pkb-root",
            pkb_root.to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "reindex",
        ])
        .env("ACA_DATA", pkb_root.to_str().unwrap())
        .env("AOPS_DUMMY_EMBEDDER", "1")
        .output()
        .expect("failed to run pkb reindex");
    assert!(
        reindex.status.success(),
        "pkb reindex failed: stdout={} stderr={}",
        String::from_utf8_lossy(&reindex.stdout),
        String::from_utf8_lossy(&reindex.stderr)
    );

    (temp_dir, pkb_root, db_path)
}

fn jsonrpc_request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn protocol_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "transport-test", "version": "0.1"}
    })
}

fn discover_request(id: u64) -> String {
    jsonrpc_request(id, "server/discover", json!({ "_meta": protocol_meta() }))
}

fn tool_call_request(id: u64, name: &str, mut args: Value) -> String {
    if let Some(obj) = args.as_object_mut() {
        obj.entry("session_id".to_string())
            .or_insert_with(|| json!("test-transport-session"));
    }
    jsonrpc_request(
        id,
        "tools/call",
        json!({"name": name, "arguments": args, "_meta": protocol_meta()}),
    )
}

/// Send JSON-RPC requests over stdio and return `(raw_line, parsed_value)` pairs.
fn stdio_session_raw(
    pkb_root: &std::path::Path,
    db_path: &std::path::Path,
    messages: &[String],
) -> Vec<(String, Value)> {
    let mut input = String::new();
    for msg in messages {
        input.push_str(msg);
        input.push('\n');
    }

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

    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

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
                responses.push((line, val));
                if responses.len() >= expected_responses {
                    break;
                }
            }
        }
        tx.send(responses).ok();
    });

    let timeout = Duration::from_secs(120);
    let responses = rx.recv_timeout(timeout).unwrap_or_else(|_| {
        child.kill().ok();
        panic!("pkb mcp stdio timed out after {timeout:?}");
    });

    child.kill().ok();
    child.wait().ok();

    responses
}

fn find_response(responses: &[(String, Value)], id: u64) -> &(String, Value) {
    responses
        .iter()
        .find(|(_, v)| v.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("missing JSON-RPC response for id={id}"))
}

#[test]
fn test_search_default_format_extract_and_single_wire_copy_over_mcp_stdio() {
    let (_temp_dir, pkb_root, db_path) = create_seeded_pkb();

    // Omit `format` on all calls to exercise default format.
    let requests = vec![
        discover_request(1),
        tool_call_request(
            2,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "detail": "snippet"}),
        ),
        tool_call_request(
            3,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "detail": "chunk"}),
        ),
        tool_call_request(
            4,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "detail": "full"}),
        ),
        tool_call_request(
            5,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "detail": "metadata"}),
        ),
        tool_call_request(
            6,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "detail": "ids"}),
        ),
        tool_call_request(
            7,
            "search",
            json!({"query": "Hydrogen Extract Transport Fixture", "ids_only": true}),
        ),
    ];

    let responses = stdio_session_raw(&pkb_root, &db_path, &requests);

    // AC1 & AC2 for detail: snippet, chunk, full
    let mut snippet_extract_len = 0usize;
    let mut full_extract_len = 0usize;

    for (req_id, detail) in [(2, "snippet"), (3, "chunk"), (4, "full")] {
        let (raw_line, resp) = find_response(&responses, req_id);
        let result = resp
            .get("result")
            .unwrap_or_else(|| panic!("detail={detail} missing result: {resp}"));
        let results = result
            .pointer("/structuredContent/results")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("detail={detail} missing structuredContent.results: {resp}"));
        assert!(
            !results.is_empty(),
            "detail={detail} expected non-empty results"
        );

        for (idx, hit) in results.iter().enumerate() {
            let extract = hit
                .get("extract")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("detail={detail} hit[{idx}] missing string extract: {hit}"));
            assert!(
                !extract.is_empty(),
                "detail={detail} hit[{idx}] extract must be non-empty"
            );
        }

        let first_extract = results[0]["extract"].as_str().unwrap();
        assert!(
            first_extract.contains(EARLY_MARKER),
            "detail={detail} extract should contain early marker: {first_extract}"
        );

        let content_text = result
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("detail={detail} missing content[0].text: {resp}"));
        assert!(
            !content_text.contains(EARLY_MARKER),
            "detail={detail} content[0].text must not duplicate extract: {content_text}"
        );
        assert!(
            !content_text.contains(LATE_MARKER),
            "detail={detail} content[0].text must not duplicate late extract: {content_text}"
        );

        assert_eq!(
            raw_line.matches(EARLY_MARKER).count(),
            1,
            "detail={detail} extract marker must appear exactly once on the wire: {raw_line}"
        );

        if detail == "snippet" {
            snippet_extract_len = first_extract.len();
            assert!(
                !first_extract.contains(LATE_MARKER),
                "detail=snippet must truncate before late marker: {first_extract}"
            );
        } else if detail == "chunk" {
            assert!(
                first_extract.contains(LATE_MARKER),
                "detail=chunk should include late marker in full chunk: {first_extract}"
            );
            assert_eq!(
                raw_line.matches(LATE_MARKER).count(),
                1,
                "detail=chunk late marker must appear once on the wire"
            );
        } else if detail == "full" {
            full_extract_len = first_extract.len();
            assert!(
                first_extract.contains(LATE_MARKER),
                "detail=full must include late marker: {first_extract}"
            );
            assert_eq!(
                raw_line.matches(LATE_MARKER).count(),
                1,
                "detail=full late marker must appear once on the wire"
            );
        }
    }

    assert!(
        full_extract_len > snippet_extract_len,
        "detail=full extract ({full_extract_len}) must exceed detail=snippet extract ({snippet_extract_len})"
    );

    // AC1 for detail: metadata, detail: ids, ids_only: true — zero extract field on hits
    for (req_id, label) in [(5, "metadata"), (6, "ids"), (7, "ids_only")] {
        let (raw_line, resp) = find_response(&responses, req_id);
        let result = resp
            .get("result")
            .unwrap_or_else(|| panic!("{label} missing result: {resp}"));
        let results = result
            .pointer("/structuredContent/results")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("{label} missing structuredContent.results: {resp}"));
        assert!(!results.is_empty(), "{label} expected non-empty results");
        for (idx, hit) in results.iter().enumerate() {
            assert!(
                hit.get("extract").is_none(),
                "{label} hit[{idx}] must not carry extract: {hit}"
            );
        }
        assert_eq!(
            raw_line.matches(EARLY_MARKER).count(),
            0,
            "{label} wire response must not contain body extract"
        );
    }
}
