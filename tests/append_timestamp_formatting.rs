//! Integration tests for `pkb append` timestamp formatting and CLI echo (aops_c37992c1).
//!
//! Verifies:
//! 1. `pkb append <id> "## Heading\n\nbody"` keeps `## Heading` at column 0,
//!    places timestamp on its own line, and separates appended block with exactly one blank line.
//! 2. `pkb append <id> "- list item"` keeps `- list item` at column 0.
//! 3. `pkb append <id> "plain prose"` keeps prose at column 0.
//! 4. CLI stdout reports byte count, first line, and last line.
//! 5. Verification is performed by reading the file back from disk.

use std::path::PathBuf;
use std::process::Command;

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

fn seed_pkb_with_task() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("polecat.yaml"), "projects:\n  aops: {}\n").unwrap();
    let tasks_dir = tmp.path().join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    let task_path = tasks_dir.join("task-append-probe.md");
    std::fs::write(
        &task_path,
        "---\n\
         id: task-append-probe\n\
         title: Append Probe Task\n\
         type: task\n\
         status: active\n\
         priority: 2\n\
         project: aops\n\
         ---\n\n# Append Probe Task\n\nInitial body paragraph.\n",
    )
    .unwrap();

    (tmp, task_path)
}

#[test]
fn test_cli_append_heading_preserves_column_0_and_blank_line() {
    let (tmp, task_path) = seed_pkb_with_task();
    let pkb = pkb_binary();

    let payload = "## X\n\nbody text";
    let out = Command::new(&pkb)
        .env("ACA_DATA", tmp.path())
        .args(["append", "task-append-probe", payload])
        .output()
        .expect("Failed to execute pkb append");

    assert!(
        out.status.success(),
        "pkb append failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Appended to: Append Probe Task (task-append-probe)"),
        "stdout missing expected header: {}",
        stdout
    );
    assert!(
        stdout.contains("bytes") && stdout.contains("first:") && stdout.contains("last:"),
        "stdout missing echo metadata (bytes, first, last): {}",
        stdout
    );
    assert!(
        stdout.contains("\"## X\"") && stdout.contains("\"body text\""),
        "stdout missing first and last line in echo: {}",
        stdout
    );

    // Read back file from disk
    let file_content = std::fs::read_to_string(&task_path).unwrap();
    assert!(
        file_content.contains("Initial body paragraph.\n\n**"),
        "Expected exactly one blank line before timestamp block. Got:\n{}",
        file_content
    );
    assert!(
        file_content.contains("\n\n## X\n\nbody text"),
        "Expected heading ## X at column 0. Got:\n{}",
        file_content
    );
    assert!(
        !file_content.contains("— ## X"),
        "Heading was fused to timestamp: {}",
        file_content
    );
}

#[test]
fn test_cli_append_list_item_and_plain_prose_sequential() {
    let (tmp, task_path) = seed_pkb_with_task();
    let pkb = pkb_binary();

    // Append list item
    let list_payload = "- list item alpha\n- list item beta";
    let out1 = Command::new(&pkb)
        .env("ACA_DATA", tmp.path())
        .args(["append", "task-append-probe", list_payload])
        .output()
        .expect("Failed to execute pkb append");
    assert!(out1.status.success());
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(stdout1.contains("- list item alpha"));

    // Append prose
    let prose_payload = "Plain prose paragraph text.";
    let out2 = Command::new(&pkb)
        .env("ACA_DATA", tmp.path())
        .args(["append", "task-append-probe", prose_payload])
        .output()
        .expect("Failed to execute pkb append");
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("Plain prose paragraph text."));

    // Read back file from disk
    let file_content = std::fs::read_to_string(&task_path).unwrap();
    assert!(
        file_content.contains("Initial body paragraph.\n\n**"),
        "Missing blank line before first append:\n{}",
        file_content
    );
    assert!(
        file_content.contains("\n\n- list item alpha\n- list item beta"),
        "List items must start at column 0:\n{}",
        file_content
    );
    assert!(
        file_content.contains("- list item beta\n\n**"),
        "Missing blank line before second append:\n{}",
        file_content
    );
    assert!(
        file_content.contains("\n\nPlain prose paragraph text."),
        "Prose must start at column 0:\n{}",
        file_content
    );
    assert!(
        !file_content.contains("— - list item") && !file_content.contains("— Plain prose"),
        "Payload fused to timestamp in file:\n{}",
        file_content
    );
}

#[test]
fn test_cli_append_with_section() {
    let (tmp, task_path) = seed_pkb_with_task();
    let pkb = pkb_binary();

    let payload = "## Subheading In Log\n\nDetail line.";
    let out = Command::new(&pkb)
        .env("ACA_DATA", tmp.path())
        .args(["append", "task-append-probe", "--section", "Log", payload])
        .output()
        .expect("Failed to execute pkb append");
    assert!(out.status.success());

    let file_content = std::fs::read_to_string(&task_path).unwrap();
    assert!(
        file_content.contains("## Log\n\n**"),
        "Log heading should be followed by blank line and timestamp:\n{}",
        file_content
    );
    assert!(
        file_content.contains("\n\n## Subheading In Log\n\nDetail line."),
        "Subheading should be at column 0:\n{}",
        file_content
    );
    assert!(
        !file_content.contains("— ## Subheading In Log"),
        "Subheading fused to timestamp:\n{}",
        file_content
    );
}
