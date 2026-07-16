//! Behavioural tests for "targets are black holes" (task mem-ba3963ec):
//!
//!   * No node whose `type` is `target`/`goal` may become a structural parent —
//!     enforced across the `pkb new` (create) path, the shared graph helper that
//!     every MCP parent-assignment site delegates to, and the batch reparent path.
//!   * The `pkb migrate target-parents` migration converts existing
//!     `parent: <target>` edges into `contributes_to` edges (weight still flows),
//!     is idempotent, and leaves legitimate task-parents untouched.
//!
//! Tests assert behaviour through the public API (the `pkb` binary + the `mem`
//! library), never file existence or hardcoded line numbers.

use std::path::{Path, PathBuf};
use std::process::Command;

use mem::graph_store::GraphStore;

fn pkb_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in ["target/release/pkb", "target/debug/pkb"] {
        let p = manifest.join(rel);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("pkb")
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn target_node(id: &str, title: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: \"{title}\"\ntype: target\nstatus: active\npriority: 1\n---\n\n# {title}\n"
    )
}

fn task_node(id: &str, title: &str, extra: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: \"{title}\"\ntype: task\nproject: aops\nstatus: ready\npriority: 2\n{extra}---\n\n# {title}\n"
    )
}

/// Seed a target + a normal task so `--parent` can resolve against both.
fn seed_basic() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // Register the `aops` slug so `--project aops` passes polecat.yaml validation.
    write(tmp.path(), "polecat.yaml", "projects:\n  aops: {}\n");
    write(
        tmp.path(),
        "targets/targ-strategy.md",
        &target_node("targ-strategy", "Strategy"),
    );
    write(
        tmp.path(),
        "tasks/task-normal.md",
        &task_node("task-normal", "Normal", ""),
    );
    tmp
}

fn run(pkb: &Path, args: &[&str]) -> std::process::Output {
    Command::new(pkb_binary())
        .args(args)
        .env("ACA_DATA", pkb)
        .output()
        .expect("failed to spawn pkb")
}

// ── Guard: create path (`pkb new`) ────────────────────────────────────────

#[test]
fn new_rejects_target_as_parent() {
    let tmp = seed_basic();
    let out = run(
        tmp.path(),
        &[
            "new",
            "Child",
            "--project",
            "aops",
            "--parent",
            "targ-strategy",
        ],
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when parenting under a target"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("contributes_to"),
        "error should redirect to contributes_to; got: {stderr}"
    );
}

#[test]
fn new_accepts_normal_task_as_parent() {
    let tmp = seed_basic();
    let out = run(
        tmp.path(),
        &[
            "new",
            "Child",
            "--project",
            "aops",
            "--parent",
            "task-normal",
        ],
    );
    assert!(
        out.status.success(),
        "parenting under a normal task must succeed; stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

// ── Guard: shared helper (every MCP site delegates here) ──────────────────

#[test]
fn reject_target_as_parent_helper() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "targets/targ-x.md", &target_node("targ-x", "X"));
    // `goal` is the retired alias — must be treated identically to `target`.
    write(
        tmp.path(),
        "targets/goal-y.md",
        "---\nid: goal-y\ntitle: \"Y\"\ntype: goal\nstatus: active\n---\n\n# Y\n",
    );
    write(
        tmp.path(),
        "tasks/task-ok.md",
        &task_node("task-ok", "OK", ""),
    );

    let graph = GraphStore::build_from_directory(tmp.path());
    assert!(
        graph.reject_target_as_parent("targ-x").is_err(),
        "target must be rejected"
    );
    assert!(
        graph.reject_target_as_parent("goal-y").is_err(),
        "goal alias must be rejected"
    );
    assert!(
        graph.reject_target_as_parent("task-ok").is_ok(),
        "task must be accepted"
    );
    // An unresolvable id is not this guard's concern — caller's existence check owns it.
    assert!(graph.reject_target_as_parent("does-not-exist").is_ok());
}

// ── Guard: batch reparent path ────────────────────────────────────────────

#[test]
fn batch_reparent_rejects_target() {
    let tmp = seed_basic();
    let out = run(
        tmp.path(),
        &[
            "batch",
            "reparent",
            "--new-parent",
            "targ-strategy",
            "--ids",
            "task-normal",
            "--dry-run",
        ],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("contributes_to") || combined.to_lowercase().contains("error"),
        "batch reparent onto a target must surface an error; got: {combined}"
    );
}

// ── Migration ─────────────────────────────────────────────────────────────

/// Seed a graph exercising every migration branch:
///   * task-child  — parent under a target (→ migrated)
///   * task-both   — parent under a target AND already contributes_to it (→ parent
///                   dropped, no duplicate edge)
///   * task-legit  — legitimate task-parent + a contributes_to to a *different*
///                   target (→ untouched)
fn seed_migration() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "polecat.yaml", "projects:\n  aops: {}\n");
    write(
        tmp.path(),
        "targets/targ-strategy.md",
        &target_node("targ-strategy", "Strategy"),
    );
    write(
        tmp.path(),
        "targets/targ-other.md",
        &target_node("targ-other", "Other"),
    );
    write(
        tmp.path(),
        "tasks/task-parent.md",
        &task_node("task-parent", "Parent", ""),
    );
    write(
        tmp.path(),
        "tasks/task-child.md",
        &task_node("task-child", "Child", "parent: targ-strategy\n"),
    );
    write(
        tmp.path(),
        "tasks/task-both.md",
        &task_node(
            "task-both",
            "Both",
            "parent: targ-strategy\ncontributes_to:\n  - to: targ-strategy\n    stated_weight: Probable\n    justification: \"pre-existing\"\n",
        ),
    );
    write(
        tmp.path(),
        "tasks/task-legit.md",
        &task_node(
            "task-legit",
            "Legit",
            "parent: task-parent\ncontributes_to:\n  - to: targ-other\n    stated_weight: Expected\n    justification: \"strategic\"\n",
        ),
    );
    tmp
}

#[derive(serde::Deserialize)]
struct Report {
    dry_run: bool,
    changes: Vec<Change>,
}

#[derive(serde::Deserialize)]
struct Change {
    task_id: String,
    target_id: String,
    kind: String,
}

fn migrate(pkb: &Path, dry_run: bool) -> Report {
    let mut args = vec!["migrate", "target-parents", "--format", "json"];
    if dry_run {
        args.push("--dry-run");
    }
    let out = run(pkb, &args);
    assert!(
        out.status.success(),
        "migrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("migrate must emit valid JSON report")
}

#[test]
fn migration_dry_run_reports_without_writing() {
    let tmp = seed_migration();
    let before = std::fs::read_to_string(tmp.path().join("tasks/task-child.md")).unwrap();

    let report = migrate(tmp.path(), true);
    assert!(report.dry_run);
    // task-child + task-both are the two target-parented nodes; task-legit is not.
    assert_eq!(
        report.changes.len(),
        2,
        "only target-parented nodes are touched"
    );
    assert!(report
        .changes
        .iter()
        .all(|c| c.target_id == "targ-strategy"));

    let after = std::fs::read_to_string(tmp.path().join("tasks/task-child.md")).unwrap();
    assert_eq!(before, after, "dry-run must not write files");
}

#[test]
fn migration_applies_converts_parent_to_contributes_to() {
    let tmp = seed_migration();
    let report = migrate(tmp.path(), false);
    assert!(!report.dry_run);

    let kind_of = |id: &str| {
        report
            .changes
            .iter()
            .find(|c| c.task_id == id)
            .map(|c| c.kind.as_str())
            .unwrap_or("<none>")
            .to_string()
    };
    assert_eq!(kind_of("task-child"), "Migrated");
    assert_eq!(kind_of("task-both"), "AlreadyLinked");

    let graph = GraphStore::build_from_directory(tmp.path());

    // task-child: parent gone, now contributes_to the target.
    let child = graph.resolve("task-child").expect("task-child");
    assert!(
        child.parent.is_none(),
        "parent must be removed: {:?}",
        child.parent
    );
    assert!(
        child.contributes_to.iter().any(|c| c.to == "targ-strategy"),
        "task-child must contribute_to targ-strategy"
    );

    // task-both: parent gone, exactly one edge to the target (no duplicate).
    let both = graph.resolve("task-both").expect("task-both");
    assert!(both.parent.is_none());
    let dup = both
        .contributes_to
        .iter()
        .filter(|c| c.to == "targ-strategy")
        .count();
    assert_eq!(dup, 1, "idempotent merge must not duplicate the edge");

    // Weight still flows: the target accrues downstream_weight from its contributors.
    let target = graph.resolve("targ-strategy").expect("targ-strategy");
    assert!(
        target.contributed_by.contains(&"task-child".to_string()),
        "target.contributed_by must include the migrated task"
    );
    assert!(
        target.downstream_weight > 0.0,
        "weight must propagate to the target, got {}",
        target.downstream_weight
    );

    // Regression: the legitimately-parented task is untouched.
    let legit = graph.resolve("task-legit").expect("task-legit");
    assert_eq!(legit.parent.as_deref(), Some("task-parent"));
    assert!(legit.contributes_to.iter().any(|c| c.to == "targ-other"));
}

#[test]
fn migration_is_idempotent() {
    let tmp = seed_migration();
    let first = migrate(tmp.path(), false);
    assert_eq!(first.changes.len(), 2);

    // Second pass: no node carries a target-parent anymore → nothing to do.
    let second = migrate(tmp.path(), false);
    assert_eq!(
        second.changes.len(),
        0,
        "re-running the migration must be a no-op"
    );
}
