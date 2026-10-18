//! Behavioural tests for the CLI `tasks` surface: the DEFAULT ordering (no
//! `--sort` argument) is the flow display order (`display_cmp`), agreeing with
//! the MCP `list_tasks` default, while an EXPLICIT `--sort` key is honoured.
//!
//! These run fully offline — `pkb tasks` builds the graph from the PKB directory
//! and needs no embeddings/ONNX — so they execute on every machine. (mem-e394a6d0)

use std::process::Command;

// See tests/mcp_integration.rs for the full rationale: `Drop`-based child
// reaping doesn't run when the harness process itself is SIGKILLed, so a
// kernel-level PR_SET_PDEATHSIG backstop is installed on every spawned `pkb`
// child here too.
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

/// Run `pkb tasks [extra args]` and return the ordered, de-duplicated
/// list of known task IDs as they appear in the flat output.
fn ordered_ids(dir: &std::path::Path, known: &[&str], extra: &[&str]) -> Vec<String> {
    let db = dir.join("db.bin");
    let mut args: Vec<String> = vec![
        "--pkb-root".into(),
        dir.to_string_lossy().into(),
        "--db-path".into(),
        db.to_string_lossy().into(),
        "tasks".into(),
    ];
    let has_filter = extra
        .iter()
        .any(|arg| *arg == "all" || *arg == "ready" || *arg == "blocked");
    if !has_filter {
        args.push("all".into());
    }
    args.push("--flat".into());
    args.extend(extra.iter().map(|s| s.to_string()));

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pkb"));
    cmd.args(&args)
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.to_string_lossy().as_ref())
        .env_remove("AOPS_SESSIONS")
        .env_remove("AOPS_POLECAT_CONFIG");
    let out = cmd.kill_on_parent_death().output().expect("run pkb tasks");
    assert!(
        out.status.success(),
        "pkb tasks exited with {}: stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut seen = Vec::new();
    for line in stdout.lines() {
        for id in known {
            if line.contains(&format!("[{id}]")) && !seen.contains(&id.to_string()) {
                seen.push(id.to_string());
            }
        }
    }
    seen
}

/// Run `pkb tasks all [extra args]` (hierarchical tree mode) and return ordered IDs.
fn ordered_tree_ids(dir: &std::path::Path, known: &[&str], extra: &[&str]) -> Vec<String> {
    let db = dir.join("db.bin");
    let mut args: Vec<String> = vec![
        "--pkb-root".into(),
        dir.to_string_lossy().into(),
        "--db-path".into(),
        db.to_string_lossy().into(),
        "tasks".into(),
        "all".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));

    let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args(&args)
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.to_string_lossy().as_ref())
        .env_remove("AOPS_SESSIONS")
        .env_remove("AOPS_POLECAT_CONFIG")
        .kill_on_parent_death()
        .output()
        .expect("run pkb tasks tree");
    assert!(
        out.status.success(),
        "pkb tasks exited with {}: stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut seen = Vec::new();
    for line in stdout.lines() {
        for id in known {
            if line.contains(&format!("[{id}]")) && !seen.contains(&id.to_string()) {
                seen.push(id.to_string());
            }
        }
    }
    seen
}

/// Run `pkb focus` and return ordered IDs for the given known list.
fn ordered_focus_ids(dir: &std::path::Path, known: &[&str]) -> Vec<String> {
    let db = dir.join("db.bin");
    let args: Vec<String> = vec![
        "--pkb-root".into(),
        dir.to_string_lossy().into(),
        "--db-path".into(),
        db.to_string_lossy().into(),
        "focus".into(),
    ];

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pkb"));
    cmd.args(&args)
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.to_string_lossy().as_ref())
        .env_remove("AOPS_SESSIONS")
        .env_remove("AOPS_POLECAT_CONFIG");
    let out = cmd.kill_on_parent_death().output().expect("run pkb focus");
    assert!(
        out.status.success(),
        "pkb focus exited with {}: stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut seen = Vec::new();
    for line in stdout.lines() {
        for id in known {
            if line.contains(&format!("[{id}]")) && !seen.contains(&id.to_string()) {
                seen.push(id.to_string());
            }
        }
    }
    seen
}

fn seed_flow_parity(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("targets")).unwrap();
    std::fs::create_dir_all(dir.join("tasks")).unwrap();
    std::fs::create_dir_all(dir.join("projects")).unwrap();

    std::fs::write(
        dir.join("projects/p.md"),
        "---\nid: p\ntitle: Project\ntype: project\nstatus: active\n---\n# Project\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("targets/tg-a.md"),
        "---\nid: tg-a\ntitle: Target A\ntype: target\nworth: 1.0\nstatus: active\n---\n# Target A\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("targets/tg-b.md"),
        "---\nid: tg-b\ntitle: Target B\ntype: target\nworth: 0.5\nstatus: active\n---\n# Target B\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("tasks/t-high.md"),
        "---\nid: t-high\ntitle: Task High Gain\ntype: task\nstatus: ready\nparent: p\nlinks:\n  - to: tg-a\n    label: serves\n    quantum: 0.8\n---\nBody\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("tasks/t-low.md"),
        "---\nid: t-low\ntitle: Task Low Gain\ntype: task\nstatus: ready\nparent: p\nlinks:\n  - to: tg-b\n    label: serves\n    quantum: 0.2\n---\nBody\n",
    )
    .unwrap();

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    std::fs::write(
        dir.join("tasks/t-cliff.md"),
        format!("---\nid: t-cliff\ntitle: Task On Cliff\ntype: task\nstatus: ready\nparent: p\ndue: {today}\ndeadline_class: hard\nlinks:\n  - to: tg-b\n    label: serves\n    quantum: 0.1\n---\nBody\n"),
    )
    .unwrap();
}

const FLOW_KNOWN: [&str; 3] = ["t-cliff", "t-high", "t-low"];

/// AC1 (CLI parity): with NO `--sort` argument the default order is the flow
/// display order — the hard-deadline cliff task first, then by gain descending.
#[test]
fn cli_tasks_default_order_is_display_order() {
    let dir = tempfile::tempdir().unwrap();
    seed_flow_parity(dir.path());
    let ids = ordered_ids(dir.path(), &FLOW_KNOWN, &[]);
    assert_eq!(
        ids,
        vec!["t-cliff", "t-high", "t-low"],
        "default `pkb tasks` order must be display_cmp order, got {ids:?}"
    );
}

/// Tree sibling order parity: task siblings rendered hierarchically under a
/// parent follow the same display order as the flat view.
#[test]
fn cli_tasks_tree_sibling_order_matches_display_order() {
    let dir = tempfile::tempdir().unwrap();
    seed_flow_parity(dir.path());
    let tree_ids = ordered_tree_ids(dir.path(), &FLOW_KNOWN, &[]);
    let flat_ids = ordered_ids(dir.path(), &FLOW_KNOWN, &[]);
    assert_eq!(tree_ids, vec!["t-cliff", "t-high", "t-low"]);
    assert_eq!(tree_ids, flat_ids, "tree view must match flat view ordering");
}

/// An EXPLICIT `--sort` key is honoured: `--sort id` orders by id ascending,
/// where the default display order puts the figureless `a-plain` last.
#[test]
fn cli_tasks_explicit_sort_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    seed_flow_parity(dir.path());
    std::fs::write(
        dir.path().join("tasks/a-plain.md"),
        "---\nid: a-plain\ntitle: Plain Task\ntype: task\nstatus: ready\nparent: p\n---\nBody\n",
    )
    .unwrap();
    let known = ["a-plain", "t-cliff", "t-high", "t-low"];
    let default_ids = ordered_ids(dir.path(), &known, &[]);
    assert_eq!(
        default_ids,
        vec!["t-cliff", "t-high", "t-low", "a-plain"],
        "default order puts the task with no flow figures last, got {default_ids:?}"
    );
    let ids = ordered_ids(dir.path(), &known, &["--sort", "id"]);
    assert_eq!(
        ids,
        vec!["a-plain", "t-cliff", "t-high", "t-low"],
        "explicit `--sort id` must order by id ascending, got {ids:?}"
    );
}

/// AC6 (determinism): repeated identical default calls return identical ordering.
#[test]
fn cli_tasks_default_order_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    seed_flow_parity(dir.path());
    let first = ordered_ids(dir.path(), &FLOW_KNOWN, &[]);
    let second = ordered_ids(dir.path(), &FLOW_KNOWN, &[]);
    assert_eq!(first, second, "repeated default `pkb tasks` calls must agree");
    assert_eq!(first.len(), 3, "all three seeded tasks should appear");
}

/// T-cli-parity / A24: `pkb tasks`, `pkb focus`, and `list_tasks` agree on order
/// and figures under `display_cmp`.
#[test]
fn cli_and_mcp_display_order_parity() {
    let dir = tempfile::tempdir().unwrap();
    seed_flow_parity(dir.path());

    let known = FLOW_KNOWN;

    // 1. CLI tasks ready order
    let cli_tasks_order = ordered_ids(dir.path(), &known, &["ready"]);
    assert_eq!(
        cli_tasks_order,
        vec!["t-cliff", "t-high", "t-low"],
        "pkb tasks ready must sort cliff task first, then by gain descending"
    );

    // 2. CLI focus order
    let cli_focus_order = ordered_focus_ids(dir.path(), &known);
    assert_eq!(
        cli_focus_order,
        vec!["t-cliff", "t-high", "t-low"],
        "pkb focus must agree with pkb tasks display_cmp order"
    );

    // 3. MCP list_tasks ready order (JSON)
    let doc_p = mem::pkb::parse_file_relative(&dir.path().join("projects/p.md"), dir.path()).unwrap();
    let doc_ta = mem::pkb::parse_file_relative(&dir.path().join("targets/tg-a.md"), dir.path()).unwrap();
    let doc_tb = mem::pkb::parse_file_relative(&dir.path().join("targets/tg-b.md"), dir.path()).unwrap();
    let doc_th = mem::pkb::parse_file_relative(&dir.path().join("tasks/t-high.md"), dir.path()).unwrap();
    let doc_tl = mem::pkb::parse_file_relative(&dir.path().join("tasks/t-low.md"), dir.path()).unwrap();
    let doc_tc = mem::pkb::parse_file_relative(&dir.path().join("tasks/t-cliff.md"), dir.path()).unwrap();

    let graph = mem::graph_store::GraphStore::build(
        &[doc_p, doc_ta, doc_tb, doc_th, doc_tl, doc_tc],
        dir.path(),
    );
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = dir.path().join("db.bin");

    let server = mem::mcp_server::PkbSearchServer::new(
        std::sync::Arc::new(parking_lot::RwLock::new(store)),
        std::sync::Arc::new(embedder),
        dir.path().to_path_buf(),
        db_path,
        std::sync::Arc::new(parking_lot::RwLock::new(graph)),
    );

    let res = server
        .dispatch_tool_sync("list_tasks", &serde_json::json!({"status": "ready", "format": "json"}))
        .unwrap();
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    let tasks_arr = val
        .get("tasks")
        .and_then(|v| v.as_array())
        .expect("tasks array in list_tasks response");

    let mcp_ids: Vec<String> = tasks_arr
        .iter()
        .filter_map(|t| t.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
        .collect();

    assert_eq!(
        mcp_ids,
        vec!["t-cliff", "t-high", "t-low"],
        "list_tasks ready must match CLI display_cmp order"
    );

    // Verify figures on the MCP rows
    let th = tasks_arr
        .iter()
        .find(|t| t.get("id").and_then(|id| id.as_str()) == Some("t-high"))
        .unwrap();
    assert_eq!(th.get("gain").and_then(|g| g.as_f64()), Some(0.8));
    assert!(
        th.get("downstream_weight").is_none(),
        "retired downstream_weight must not be emitted"
    );

    // 4. MCP list_tasks ready order (markdown default)
    let res_md = server
        .dispatch_tool_sync("list_tasks", &serde_json::json!({"status": "ready"}))
        .unwrap();
    let text_md: String = res_md
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let mut mcp_md_ids = Vec::new();
    for line in text_md.lines() {
        for id in &known {
            if (line.contains(&format!("| {id} |")) || line.contains(&format!("[{id}]")))
                && !mcp_md_ids.contains(&id.to_string())
            {
                mcp_md_ids.push(id.to_string());
            }
        }
    }
    assert_eq!(
        mcp_md_ids,
        vec!["t-cliff", "t-high", "t-low"],
        "markdown list_tasks ready must also match CLI display_cmp order"
    );
}

/// R21 / A23 / T-nosum: No tool or CLI output contains a sum of worth across distinct targets.
#[test]
fn test_no_output_sums_worth() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("tasks")).unwrap();

    // Target A worth 0.8
    std::fs::write(
        root.join("targets/tg-a.md"),
        "---\nid: tg-a\ntitle: Target A\ntype: target\nworth: 0.8\nstatus: active\n---\nBody\n",
    )
    .unwrap();
    // Target B worth 0.6
    std::fs::write(
        root.join("targets/tg-b.md"),
        "---\nid: tg-b\ntitle: Target B\ntype: target\nworth: 0.6\nstatus: active\n---\nBody\n",
    )
    .unwrap();

    // Parent task
    std::fs::write(
        root.join("tasks/parent.md"),
        "---\nid: parent\ntitle: Parent Task\ntype: task\nstatus: ready\n---\nBody\n",
    )
    .unwrap();

    // Child 1 with quantum 0.5 to tg-a (gain = 0.4)
    std::fs::write(
        root.join("tasks/child-1.md"),
        "---\nid: child-1\ntitle: Child 1\ntype: task\nstatus: ready\nparent: parent\nlinks:\n  - to: tg-a\n    label: serves\n    quantum: 0.5\n---\nBody\n",
    )
    .unwrap();

    // Child 2 with quantum 0.5 to tg-b (gain = 0.3)
    std::fs::write(
        root.join("tasks/child-2.md"),
        "---\nid: child-2\ntitle: Child 2\ntype: task\nstatus: ready\nparent: parent\nlinks:\n  - to: tg-b\n    label: serves\n    quantum: 0.5\n---\nBody\n",
    )
    .unwrap();

    let doc_ta = mem::pkb::parse_file_relative(&root.join("targets/tg-a.md"), root).unwrap();
    let doc_tb = mem::pkb::parse_file_relative(&root.join("targets/tg-b.md"), root).unwrap();
    let doc_p = mem::pkb::parse_file_relative(&root.join("tasks/parent.md"), root).unwrap();
    let doc_c1 = mem::pkb::parse_file_relative(&root.join("tasks/child-1.md"), root).unwrap();
    let doc_c2 = mem::pkb::parse_file_relative(&root.join("tasks/child-2.md"), root).unwrap();

    let graph = mem::graph_store::GraphStore::build(
        &[doc_ta, doc_tb, doc_p, doc_c1, doc_c2],
        root,
    );
    let store = mem::vectordb::VectorStore::new(3);
    let embedder = mem::embeddings::Embedder::new_dummy();
    let db_path = root.join("db.bin");

    let server = mem::mcp_server::PkbSearchServer::new(
        std::sync::Arc::new(parking_lot::RwLock::new(store)),
        std::sync::Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        std::sync::Arc::new(parking_lot::RwLock::new(graph)),
    );

    // Group view 1: task_summary
    let summary_res = server
        .dispatch_tool_sync("task_summary", &serde_json::json!({}))
        .unwrap();
    let summary_text: String = summary_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    let summary_json: serde_json::Value = serde_json::from_str(&summary_text).unwrap();

    let mut numbers = Vec::new();
    fn collect_numbers(v: &serde_json::Value, nums: &mut Vec<f64>) {
        match v {
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    nums.push(f);
                }
            }
            serde_json::Value::Array(a) => {
                for item in a {
                    collect_numbers(item, nums);
                }
            }
            serde_json::Value::Object(m) => {
                for val in m.values() {
                    collect_numbers(val, nums);
                }
            }
            _ => {}
        }
    }
    collect_numbers(&summary_json, &mut numbers);

    // Group view 2: nested_tasks
    let nested_res = server
        .dispatch_tool_sync("nested_tasks", &serde_json::json!({}))
        .unwrap();
    let nested_text: String = nested_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    if let Ok(nested_json) = serde_json::from_str::<serde_json::Value>(&nested_text) {
        collect_numbers(&nested_json, &mut numbers);
    }

    // Verify R21 / A23: no number is the sum (0.70 or 1.40) or mean (0.35) of worths/gains
    for n in &numbers {
        assert!(
            (n - 0.70).abs() > 1e-6,
            "Forbidden sum 0.70 emitted in group output: {n}"
        );
        assert!(
            (n - 1.40).abs() > 1e-6,
            "Forbidden worth sum 1.40 emitted in group output: {n}"
        );
        assert!(
            (n - 0.35).abs() > 1e-6,
            "Forbidden mean 0.35 emitted in group output: {n}"
        );
    }

    // task_summary must emit carrying_worth count (2 tasks: child-1 and child-2), not sum of worths
    assert_eq!(
        summary_json.get("carrying_worth").and_then(|v| v.as_u64()),
        Some(2)
    );
}
