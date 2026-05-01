//! Integration tests for parent referential-integrity checks on `pkb new`
//! and the MCP `create_task` / `update_task` paths (task-89b2af87).
//!
//! Verifies:
//!   * `pkb new --parent <missing>` rejects with a non-zero exit and clear error.
//!   * `--allow-missing-parent` downgrades the rejection to a warning.
//!   * `pkb new --parent <existing>` succeeds.

use std::path::PathBuf;
use std::process::Command;

fn pkb_binary() -> PathBuf {
    // Mirror tests/mcp_integration.rs binary lookup
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

/// Seed a PKB tempdir with a single project node so `--parent` can resolve
/// against something real.
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
         alias:\n  - \"proj-realdead-real-project\"\n  - \"proj-realdead\"\n\
         permalink: proj-realdead\n\
         ---\n\n# Real Project\n",
    )
    .unwrap();

    // tasks/ dir is created on demand by the CLI; pre-create to be safe.
    std::fs::create_dir_all(tmp.path().join("tasks")).unwrap();

    tmp
}

#[test]
fn pkb_new_rejects_nonexistent_parent() {
    let pkb = seed_pkb();
    let out = Command::new(pkb_binary())
        .args(["new", "Sample title", "--parent", "task-does-not-exist"])
        .env("ACA_DATA", pkb.path())
        .output()
        .expect("failed to spawn pkb");

    assert!(
        !out.status.success(),
        "expected non-zero exit; stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("task-does-not-exist") && stderr.contains("not found"),
        "expected clear 'not found' error mentioning the bad ID; got: {stderr}"
    );

    // The task file must NOT have been created.
    let any_task = std::fs::read_dir(pkb.path().join("tasks"))
        .unwrap()
        .any(|e| {
            e.ok()
                .map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("task-")
                })
                .unwrap_or(false)
        });
    assert!(!any_task, "no task file should exist when parent is invalid");
}

#[test]
fn pkb_new_with_allow_missing_parent_proceeds_with_warning() {
    let pkb = seed_pkb();
    let out = Command::new(pkb_binary())
        .args([
            "new",
            "Sample title",
            "--parent",
            "task-does-not-exist",
            "--allow-missing-parent",
        ])
        .env("ACA_DATA", pkb.path())
        .output()
        .expect("failed to spawn pkb");

    assert!(
        out.status.success(),
        "expected zero exit with --allow-missing-parent; stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("warning"),
        "expected loud warning on stderr; got: {stderr}"
    );

    // The task file SHOULD exist, with the (unresolvable) parent recorded —
    // the override deliberately preserves the originally-requested edge so it
    // shows up in orphan/lint reports rather than silently vanishing.
    let task_file = std::fs::read_dir(pkb.path().join("tasks"))
        .unwrap()
        .find_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("task-") {
                Some(e.path())
            } else {
                None
            }
        })
        .expect("task file should have been created");
    let body = std::fs::read_to_string(&task_file).unwrap();
    assert!(
        body.contains("parent: task-does-not-exist"),
        "frontmatter should still record the requested (unresolvable) parent: {body}"
    );
}

#[test]
fn pkb_new_with_existing_parent_succeeds() {
    let pkb = seed_pkb();
    let out = Command::new(pkb_binary())
        .args(["new", "Sample title", "--parent", "proj-realdead"])
        .env("ACA_DATA", pkb.path())
        .output()
        .expect("failed to spawn pkb");

    assert!(
        out.status.success(),
        "expected success when parent resolves; stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
