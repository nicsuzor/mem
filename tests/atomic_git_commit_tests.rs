//! Acceptance tests for atomic git commits on PKB writes.
//!
//! Ref: task `aops_bab29969`
//! Verifies that writes through the PKB produce individual, atomic git commits
//! with meaningful messages, and that non-git directories continue to work cleanly.

use std::collections::HashMap;
use std::process::Command;
use tempfile::TempDir;

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .expect("run git command");
        assert!(status.success(), "git command failed: {:?}", args);
    };

    run(&["init"]);
    run(&["config", "user.name", "Test Committer"]);
    run(&["config", "user.email", "test@example.com"]);
}

fn git_log_messages(dir: &std::path::Path) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["log", "--pretty=format:%s"])
        .output()
        .expect("git log");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().map(|s| s.to_string()).collect()
}

#[test]
fn test_atomic_git_commits_on_all_write_operations() {
    let tmp = TempDir::new().expect("tempdir");
    let pkb_root = tmp.path().to_path_buf();
    init_git_repo(&pkb_root);

    // 1. Create task
    let task_path = mem::document_crud::create_task(
        &pkb_root,
        mem::document_crud::TaskFields {
            title: "Test Task for Atomic Commits".to_string(),
            parent: Some("root-epic".to_string()),
            body: Some("# Test Task

Task body content.".to_string()),
            ..Default::default()
        },
    )
    .expect("create task");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].starts_with("create(task):"),
        "Expected create(task) commit, got: {}",
        messages[0]
    );
    assert!(messages[0].contains("Test Task for Atomic Commits"));

    // 2. Update task
    let mut updates = HashMap::new();
    updates.insert(
        "status".to_string(),
        serde_json::Value::String("in_progress".to_string()),
    );
    mem::document_crud::update_document(&task_path, updates).expect("update document");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 2);
    assert!(
        messages[0].starts_with("update("),
        "Expected update commit, got: {}",
        messages[0]
    );
    assert!(messages[0].contains("status: in_progress"));

    // 3. Add observations
    mem::document_crud::add_observations(
        &task_path,
        &["Observed that atomic commits work properly".to_string()],
        None,
        false,
        None,
    )
    .expect("add observations");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 3);
    assert!(
        messages[0].starts_with("observations("),
        "Expected observations commit, got: {}",
        messages[0]
    );
    assert!(messages[0].contains("add 1 observation(s)"));

    // 4. Edit body
    let diff = "--- a
+++ b
@@ -1,3 +1,3 @@
 # Test Task
 
-Task body content.
+Updated task body content.
";
    mem::document_crud::edit_body(&task_path, diff, true, None, false)
        .expect("edit body");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 4);
    assert!(
        messages[0].starts_with("edit("),
        "Expected edit commit, got: {}",
        messages[0]
    );

    // 5. Append to document
    mem::document_crud::append_to_document(
        &task_path,
        "Appended notes block",
        Some("Notes"),
        None,
    )
    .expect("append to document");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 5);
    assert!(
        messages[0].starts_with("append("),
        "Expected append commit, got: {}",
        messages[0]
    );

    // 6. Delete document
    mem::document_crud::delete_document(&task_path).expect("delete document");

    let messages = git_log_messages(&pkb_root);
    assert_eq!(messages.len(), 6);
    assert!(
        messages[0].starts_with("delete("),
        "Expected delete commit, got: {}",
        messages[0]
    );
}

#[test]
fn test_writes_succeed_in_non_git_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let pkb_root = tmp.path().to_path_buf();
    // Do NOT initialize git

    let task_path = mem::document_crud::create_task(
        &pkb_root,
        mem::document_crud::TaskFields {
            title: "Non-git Task".to_string(),
            parent: Some("root-epic".to_string()),
            body: Some("# Non-git Task
".to_string()),
            ..Default::default()
        },
    )
    .expect("create task in non-git directory");

    assert!(task_path.exists());

    let mut updates = HashMap::new();
    updates.insert(
        "status".to_string(),
        serde_json::Value::String("ready".to_string()),
    );
    mem::document_crud::update_document(&task_path, updates)
        .expect("update in non-git directory");

    mem::document_crud::delete_document(&task_path)
        .expect("delete in non-git directory");
    assert!(!task_path.exists());
}
