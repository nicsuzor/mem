//! Integration tests for parent referential-integrity checks on `pkb new`
//! and the MCP `create_task` / `update_task` paths (task-89b2af87).
//!
//! Verifies:
//!   * `pkb new --parent <missing>` rejects with a non-zero exit and clear error.
//!   * `--allow-missing-parent` downgrades the rejection to a warning.
//!   * `pkb new --parent <existing>` succeeds.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Configure a `Command` with a kernel-level backstop (`PR_SET_PDEATHSIG` on Linux)
/// so that the child process is automatically terminated by the kernel if the
/// parent harness process dies abnormally (e.g. SIGKILL, OOM, panic abort, CI timeout).
fn kill_on_parent_death(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            let parent_pid = libc::getppid();
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent_pid {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    cmd
}

fn pkb_command(pkb: &Path) -> Command {
    let mut cmd = Command::new(pkb);
    kill_on_parent_death(&mut cmd);
    cmd
}

fn pkb_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = manifest.join("target/release/pkb");
    let debug = manifest.join("target/debug/pkb");
    match (
        release.metadata().and_then(|m| m.modified()),
        debug.metadata().and_then(|m| m.modified()),
    ) {
        (Ok(rel_time), Ok(dbg_time)) => {
            if rel_time >= dbg_time {
                release
            } else {
                debug
            }
        }
        (Ok(_), Err(_)) => release,
        (Err(_), Ok(_)) => debug,
        (Err(_), Err(_)) => PathBuf::from("pkb"),
    }
}

/// Seed a PKB tempdir with a single project node so `--parent` can resolve
/// against something real.
fn seed_pkb() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // Register the `aops` slug so `--project aops` passes polecat.yaml validation.
    std::fs::write(tmp.path().join("polecat.yaml"), "projects:\n  aops: {}\n").unwrap();
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
    let out = pkb_command(&pkb_binary())
        .args([
            "new",
            "Sample title",
            "--project",
            "aops",
            "--parent",
            "task-does-not-exist",
        ])
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
                .map(|e| e.file_name().to_string_lossy().starts_with("aops_"))
                .unwrap_or(false)
        });
    assert!(
        !any_task,
        "no task file should exist when parent is invalid"
    );
}

#[test]
fn pkb_new_with_allow_missing_parent_proceeds_with_warning() {
    let pkb = seed_pkb();
    let out = pkb_command(&pkb_binary())
        .args([
            "new",
            "Sample title",
            "--project",
            "aops",
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
            if name.starts_with("aops_") {
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
    let out = pkb_command(&pkb_binary())
        .args([
            "new",
            "Sample title",
            "--project",
            "aops",
            "--parent",
            "proj-realdead",
        ])
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
