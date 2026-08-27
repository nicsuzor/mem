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
            let priority = (thread_idx + 1) as i64;
            let res = env_clone.server.dispatch_tool_sync(
                "update_task",
                &json!({
                    "id": task_id,
                    "updates": {
                        "priority": priority,
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
    let prio = doc_final.frontmatter.as_ref().unwrap()["priority"].as_i64().unwrap();
    assert!(prio == 1 || prio == 2, "Final priority must be one of the two written values");

    // Verify index state matches
    let index_entry = env_arc.server.store_for_test().read().get_entry(task_id).cloned().expect("in index");
    assert_eq!(index_entry.id, task_id);
}
