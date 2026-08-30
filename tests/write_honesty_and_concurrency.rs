//! Acceptance tests for AC2 (write-result honesty) and AC7 (concurrency non-regression).
//!
//! Ref: task `mem_7f2a91c4`
//! AC2: Fault-injection harness across 3 failure modes with contingency table assertions.
//! AC7: Two-writer concurrency test on same node.
//! AC6: Format integrity and WAL persistence recovery without data loss.

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

struct TestEnv {
    _tmp: TempDir,
    pkb_root: PathBuf,
    db_path: PathBuf,
    server: Arc<PkbSearchServer>,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let pkb_root = tmp.path().to_path_buf();
        let tasks_dir = pkb_root.join("tasks");
        let notes_dir = pkb_root.join("notes");
        fs::create_dir_all(&tasks_dir).unwrap();
        fs::create_dir_all(&notes_dir).unwrap();

        // Seed an initial task
        fs::write(
            tasks_dir.join("task-init-01.md"),
            "---\nid: task-init-01\ntype: task\ntitle: Initial Task\nstatus: ready\npriority: 2\n---\n\nInitial task body.\n",
        ).unwrap();

        let db_path = pkb_root.join("pkb_vectors.bin");

        let files = mem::pkb::scan_directory(&pkb_root);
        let docs: Vec<_> = files
            .iter()
            .filter_map(|p| mem::pkb::parse_file_relative(p, &pkb_root))
            .collect();
        let graph = Arc::new(RwLock::new(GraphStore::build(&docs, &pkb_root)));
        let store = Arc::new(RwLock::new(VectorStore::load_or_create(&db_path, 3).unwrap()));
        let embedder = Arc::new(Embedder::new_dummy());

        let server = Arc::new(PkbSearchServer::new(
            store,
            embedder,
            pkb_root.clone(),
            db_path.clone(),
            graph,
        ));

        Self {
            _tmp: tmp,
            pkb_root,
            db_path,
            server,
        }
    }

    /// Restart server instance pointing at same on-disk directory and database
    fn restart_server(&mut self) {
        let files = mem::pkb::scan_directory(&self.pkb_root);
        let docs: Vec<_> = files
            .iter()
            .filter_map(|p| mem::pkb::parse_file_relative(p, &self.pkb_root))
            .collect();
        let graph = Arc::new(RwLock::new(GraphStore::build(&docs, &self.pkb_root)));
        let store = Arc::new(RwLock::new(VectorStore::load_or_create(&self.db_path, 3).unwrap()));
        let embedder = Arc::new(Embedder::new_dummy());

        self.server = Arc::new(PkbSearchServer::new(
            store,
            embedder,
            self.pkb_root.clone(),
            self.db_path.clone(),
            graph,
        ));
    }
}

#[derive(Debug, Default)]
struct ContingencyTable {
    success_on_disk: usize,
    success_not_on_disk: usize, // VIOLATION
    failure_on_disk: usize,     // VIOLATION
    failure_not_on_disk: usize,
    timeout_on_disk: usize,     // VIOLATION
}

// ─────────────────────────────────────────────────────────────────────────────
// AC2: Write-Result Honesty Fault Injection Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ac2_fault_injection_suite() {
    let mut table = ContingencyTable::default();
    let mut total_trials = 0;

    // ── Injection Point 1: Simulated Disk Write Failures (EACCES / read-only) ──
    // ≥ 10 trials
    for i in 0..10 {
        total_trials += 1;
        let mut env = TestEnv::new();
        let task_id = format!("task-inj1-{i:03}");
        let task_path = env.pkb_root.join("tasks").join(format!("{task_id}.md"));
        fs::write(
            &task_path,
            format!("---\nid: {task_id}\ntype: task\ntitle: Inj1 Task {i}\nstatus: ready\npriority: 2\n---\n\nBody.\n"),
        ).unwrap();
        env.restart_server();

        // Make file read-only on disk to simulate EACCES on write
        let mut perms = fs::metadata(&task_path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&task_path, perms).unwrap();

        // Attempt update_task — should fail cleanly with error response
        let res = env.server.dispatch_tool_sync(
            "update_task",
            &json!({"id": task_id, "updates": {"status": "done", "completion_evidence": "Evidence"}}),
        );

        // Reset permissions for cleanup / inspection
        let mut perms = fs::metadata(&task_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&task_path, perms).unwrap();

        let returned_success = res.is_ok();
        env.restart_server();

        let doc_after = mem::pkb::parse_file_relative(&task_path, &env.pkb_root);
        let on_disk_changed = doc_after.as_ref().and_then(|d| d.status.as_deref()) == Some("done");

        match (returned_success, on_disk_changed) {
            (true, true) => table.success_on_disk += 1,
            (true, false) => table.success_not_on_disk += 1,
            (false, true) => table.failure_on_disk += 1,
            (false, false) => table.failure_not_on_disk += 1,
        }
    }

    // ── Injection Point 2: Process Termination / Crash between In-Memory and Compaction ──
    // ≥ 10 trials
    for i in 0..10 {
        total_trials += 1;
        let mut env = TestEnv::new();
        let task_id = format!("task-inj2-{i:03}");
        let task_path = env.pkb_root.join("tasks").join(format!("{task_id}.md"));
        fs::write(
            &task_path,
            format!("---\nid: {task_id}\ntype: task\ntitle: Inj2 Task {i}\nstatus: ready\npriority: 2\n---\n\nBody.\n"),
        ).unwrap();
        env.restart_server();

        // Update task successfully
        let res = env.server.dispatch_tool_sync(
            "update_task",
            &json!({"id": task_id, "updates": {"status": "done", "completion_evidence": "Evidence"}}),
        );
        let returned_success = res.is_ok();

        // Simulate crash: DO NOT call save() or cleanup, restart server immediately
        env.restart_server();

        let doc_after = mem::pkb::parse_file_relative(&task_path, &env.pkb_root);
        let on_disk_changed = doc_after.as_ref().and_then(|d| d.status.as_deref()) == Some("done");
        let in_index = env.server.store_for_test().read().get_entry(&task_id).is_some();

        assert!(in_index, "Replay must recover index state on restart");

        match (returned_success, on_disk_changed) {
            (true, true) => table.success_on_disk += 1,
            (true, false) => table.success_not_on_disk += 1,
            (false, true) => table.failure_on_disk += 1,
            (false, false) => table.failure_not_on_disk += 1,
        }
    }

    // ── Injection Point 3: Invalid parameter / schema guard failures ──
    // ≥ 10 trials
    for i in 0..10 {
        total_trials += 1;
        let mut env = TestEnv::new();
        let task_id = format!("task-inj3-{i:03}");
        let task_path = env.pkb_root.join("tasks").join(format!("{task_id}.md"));
        fs::write(
            &task_path,
            format!("---\nid: {task_id}\ntype: task\ntitle: Inj3 Task {i}\nstatus: ready\npriority: 2\n---\n\nBody.\n"),
        ).unwrap();
        env.restart_server();

        // Attempt invalid update: setting status to "done" without required completion_evidence
        let res = env.server.dispatch_tool_sync(
            "update_task",
            &json!({"id": task_id, "updates": {"status": "done"}}),
        );
        let returned_success = res.is_ok();

        env.restart_server();

        let doc_after = mem::pkb::parse_file_relative(&task_path, &env.pkb_root);
        let on_disk_changed = doc_after.as_ref().and_then(|d| d.status.as_deref()) == Some("done");

        match (returned_success, on_disk_changed) {
            (true, true) => table.success_on_disk += 1,
            (true, false) => table.success_not_on_disk += 1,
            (false, true) => table.failure_on_disk += 1,
            (false, false) => table.failure_not_on_disk += 1,
        }
    }

    println!("\n=== AC2 Contingency Table (Total Trials: {total_trials}) ===");
    println!("  Returned Success, On-Disk Changed:     {}", table.success_on_disk);
    println!("  Returned Success, NOT on Disk:         {} (FAIL if > 0)", table.success_not_on_disk);
    println!("  Returned Failure, On-Disk Changed:     {} (FAIL if > 0)", table.failure_on_disk);
    println!("  Returned Failure, NOT on Disk:         {}", table.failure_not_on_disk);
    println!("==========================================================\n");

    assert!(total_trials >= 20, "AC2 requires >= 20 fault injection trials");
    assert_eq!(table.success_not_on_disk, 0, "Zero trials allowed in 'returned success, not on disk'");
    assert_eq!(table.failure_on_disk, 0, "Zero trials allowed in 'returned failure, but on disk'");
    assert_eq!(table.timeout_on_disk, 0, "Zero trials allowed in 'timeout, but on disk'");
}

// ─────────────────────────────────────────────────────────────────────────────
// AC7: Concurrency Non-Regression Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ac7_concurrent_two_writers_on_same_node() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    let mut env = TestEnv::new();
    let task_id = "task-concurrent-01";
    let task_path = env.pkb_root.join("tasks").join(format!("{task_id}.md"));
    fs::write(
        &task_path,
        format!("---\nid: {task_id}\ntype: task\ntitle: Concurrent Task\nstatus: ready\npriority: 2\n---\n\nInitial Body.\n"),
    ).unwrap();
    env.restart_server();

    let env_arc = Arc::new(env);
    let success_count = Arc::new(AtomicUsize::new(0));

    // Two parallel writer threads attempting to update the same task
    let mut handles = Vec::new();
    for thread_idx in 0..2 {
        let env_clone = env_arc.clone();
        let sc = success_count.clone();
        let h = thread::spawn(move || {
            let effort = format!("{}d", thread_idx + 1);
            let res = env_clone.server.dispatch_tool_sync(
                "update_task",
                &json!({
                    "id": task_id,
                    "updates": {
                        "effort": effort,
                    }
                }),
            );
            if res.is_ok() {
                sc.fetch_add(1, Ordering::SeqCst);
            }
        });
        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        success_count.load(Ordering::SeqCst),
        2,
        "Both concurrent writers must complete without error"
    );

    // Read back final state on disk
    let doc_final = mem::pkb::parse_file_relative(&task_path, &env_arc.pkb_root).expect("task parses");
    let effort = doc_final.frontmatter.as_ref().unwrap()["effort"].as_str().unwrap();
    assert!(effort == "1d" || effort == "2d", "Final effort must be one of the two written values");

    // Verify index state matches
    let index_entry = env_arc.server.store_for_test().read().get_entry(task_id).cloned().expect("in index");
    assert_eq!(index_entry.id, task_id);

    // Restart server to verify replay_wal recovers state from disk
    let mut env = Arc::into_inner(env_arc).expect("unwrap TestEnv arc after threads joined");
    env.restart_server();

    // Verify store recovers state from WAL / snapshot replay
    let replayed_entry = env.server.store_for_test().read().get_entry(task_id).cloned().expect("in index after restart");
    assert_eq!(replayed_entry.id, task_id);
    assert_eq!(replayed_entry.title, "Concurrent Task");
}

// ─────────────────────────────────────────────────────────────────────────────
// AC1 & AC2 Concurrency Acceptance Tests (mem_7451158a)
// ─────────────────────────────────────────────────────────────────────────────

fn init_git_repo(path: &std::path::Path) {
    use std::process::Command;
    let out = Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init");
    assert!(out.status.success(), "git init must succeed");

    let out = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("git config email");
    assert!(out.status.success());

    let out = Command::new("git")
        .args(["config", "user.name", "Test Runner"])
        .current_dir(path)
        .output()
        .expect("git config name");
    assert!(out.status.success());

    let out = Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("git add");
    assert!(out.status.success());

    let out = Command::new("git")
        .args(["commit", "-m", "initial seed"])
        .current_dir(path)
        .output()
        .expect("git commit");
    assert!(out.status.success(), "git commit initial seed must succeed");
}

#[test]
fn test_ac1_concurrent_writes_to_different_nodes_every_write_survives_git_verified() {
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    let mut env = TestEnv::new();
    let n_writers = 8;

    // Create N distinct documents across multiple directories (some pairs in same dir)
    let dirs = ["tasks", "notes", "projects"];
    for d in &dirs {
        fs::create_dir_all(env.pkb_root.join(d)).unwrap();
    }

    let mut doc_ids = Vec::new();
    for i in 0..n_writers {
        let dir_name = dirs[i % dirs.len()];
        let doc_id = format!("doc-ac1-{i:02}");
        let doc_path = env.pkb_root.join(dir_name).join(format!("{doc_id}.md"));
        fs::write(
            &doc_path,
            format!(
                "---\nid: {doc_id}\ntype: task\ntitle: AC1 Doc {i}\nstatus: ready\npriority: 2\n---\n\nInitial body for doc {i}.\n"
            ),
        )
        .unwrap();
        doc_ids.push((doc_id, dir_name.to_string()));
    }

    // Initialize git repo and commit initial state
    init_git_repo(&env.pkb_root);
    env.restart_server();

    let env_arc = Arc::new(env);
    let success_count = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    // Spawn N concurrent writer threads, each writing a distinct marker to a distinct node
    for (i, (doc_id, _)) in doc_ids.iter().enumerate() {
        let env_clone = env_arc.clone();
        let sc = success_count.clone();
        let doc_id = doc_id.clone();
        let marker = format!("MARKER_AC1_WRITER_{i:02}_UNIQUE_PAYLOAD");

        let h = thread::spawn(move || {
            let res = env_clone.server.dispatch_tool_sync(
                "update_body",
                &json!({
                    "id": doc_id,
                    "new_body": format!("Body updated concurrently.\n\n{marker}\n"),
                }),
            );
            if res.is_ok() {
                sc.fetch_add(1, Ordering::SeqCst);
            }
        });
        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        success_count.load(Ordering::SeqCst),
        n_writers,
        "All N concurrent writers must succeed"
    );

    // Commit all changes to git
    let out = Command::new("git")
        .args(["add", "."])
        .current_dir(&env_arc.pkb_root)
        .output()
        .expect("git add after writes");
    assert!(out.status.success());

    let out = Command::new("git")
        .args(["commit", "-m", "after N concurrent writes"])
        .current_dir(&env_arc.pkb_root)
        .output()
        .expect("git commit after writes");
    assert!(out.status.success());

    // AC1 VERIFICATION FROM GIT:
    // Every single marker must be present in the committed content, verified directly via git grep
    for i in 0..n_writers {
        let marker = format!("MARKER_AC1_WRITER_{i:02}_UNIQUE_PAYLOAD");
        let grep_out = Command::new("git")
            .args(["grep", &marker, "HEAD"])
            .current_dir(&env_arc.pkb_root)
            .output()
            .expect("git grep");
        assert!(
            grep_out.status.success(),
            "Marker '{marker}' must exist in committed git tree (AC1 requirement)"
        );
        let stdout = String::from_utf8_lossy(&grep_out.stdout);
        assert!(
            stdout.contains(&marker),
            "Git committed content must contain marker '{marker}', got: {stdout}"
        );
    }
}

#[test]
fn test_ac2_concurrent_writes_to_same_node_optimistic_concurrency_conflict() {
    use std::process::Command;

    let mut env = TestEnv::new();
    let task_id = "task-same-node-ac2";
    let task_path = env.pkb_root.join("tasks").join(format!("{task_id}.md"));
    let t0 = "2026-08-30T10:00:00Z";

    fs::write(
        &task_path,
        format!(
            "---\nid: {task_id}\ntype: task\ntitle: Same Node AC2\nstatus: ready\nmodified: '{t0}'\ncreated: 2026-08-30T10:00:00Z\npriority: 2\n---\n\nInitial node body at T0.\n"
        ),
    )
    .unwrap();

    init_git_repo(&env.pkb_root);
    env.restart_server();

    // Two writers both hold the initial read snapshot (T0)
    let read_modified = t0;

    // Writer 1 writes first with expected_modified = T0
    let res1 = env.server.dispatch_tool_sync(
        "update_body",
        &json!({
            "id": task_id,
            "new_body": "Updated body by Writer 1 (winner).\n",
            "expected_modified": read_modified,
        }),
    );
    assert!(res1.is_ok(), "Writer 1 write with matching snapshot must succeed");

    // Commit Writer 1's work to git
    let out = Command::new("git")
        .args(["add", "."])
        .current_dir(&env.pkb_root)
        .output()
        .expect("git add writer 1");
    assert!(out.status.success());
    let out = Command::new("git")
        .args(["commit", "-m", "Writer 1 update"])
        .current_dir(&env.pkb_root)
        .output()
        .expect("git commit writer 1");
    assert!(out.status.success());

    // Writer 2 (holding stale T0 snapshot) attempts to write with expected_modified = T0
    let res2 = env.server.dispatch_tool_sync(
        "update_body",
        &json!({
            "id": task_id,
            "new_body": "Updated body by Writer 2 (loser).\n",
            "expected_modified": read_modified,
        }),
    );

    // AC2 VERIFICATION: Loser receives non-success conflict result naming stale revision
    assert!(res2.is_err(), "Writer 2 write with stale snapshot must be rejected");
    let err = res2.unwrap_err();
    let err_data = err.data.expect("stale_write error must have data payload");
    assert_eq!(
        err_data.get("error_type").and_then(|v| v.as_str()),
        Some("stale_write"),
        "Error type must be 'stale_write'"
    );
    assert_eq!(
        err_data.get("expected_modified").and_then(|v| v.as_str()),
        Some(t0),
        "Error data must name expected_modified snapshot"
    );

    // Git verification: exactly Writer 1's content survived in git, Writer 2 was rejected
    let show_out = Command::new("git")
        .args(["show", &format!("HEAD:tasks/{task_id}.md")])
        .current_dir(&env.pkb_root)
        .output()
        .expect("git show HEAD");
    let git_content = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        git_content.contains("Writer 1 (winner)"),
        "Git committed state must contain Writer 1 content"
    );
    assert!(
        !git_content.contains("Writer 2 (loser)"),
        "Git committed state must NOT contain Writer 2 content"
    );
}
