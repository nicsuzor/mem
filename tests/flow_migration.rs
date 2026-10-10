use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use mem::flow_migration::{
    apply_flow_migration, cleanup_flow_migration, patch_file_text, revert_file_content,
    revert_flow_migration, status_flow_migration, PROTOTYPE_HUB_ID, SUPPORTS_QUANTUM,
};
use tempfile::tempdir;

fn init_git_repo(path: &Path) {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("init")
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("config")
        .arg("user.name")
        .arg("Test")
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("config")
        .arg("user.email")
        .arg("test@example.com")
        .status()
        .unwrap();
}

fn commit_all(path: &Path, msg: &str) {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("add")
        .arg("-A")
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("commit")
        .arg("-m")
        .arg(msg)
        .status()
        .unwrap();
}

#[test]
fn test_m3_git_diff_empty_after_revert() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    // Target file
    let target_content = r#"---
id: targ-1
title: Strategic Target
type: target
standing_weight: 0.35
status: ready
---

Target body.
"#;
    fs::write(root.join("target.md"), target_content).unwrap();

    // Task with soft_depends_on and contributes_to
    let task_content = r#"---
id: task-1
title: Task One
type: task
status: ready
soft_depends_on:
- targ-1
contributes_to:
- stated_weight: probable
  to: targ-1
---

Task body.
"#;
    fs::write(root.join("task.md"), task_content).unwrap();

    // Prototype hub candidate
    let hub_content = format!(
        r#"---
id: {PROTOTYPE_HUB_ID}
title: Respond to prospective PhD students
type: epic
status: ready
contributes_to:
- stated_weight: probable
  to: targ-1
---

Hub body.
"#
    );
    fs::write(root.join("hub.md"), hub_content).unwrap();

    commit_all(root, "Initial snapshot");

    let snapshot_head = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .unwrap();
    let snapshot_sha = String::from_utf8_lossy(&snapshot_head.stdout).trim().to_string();

    // Apply migration
    let ledger = apply_flow_migration(root).expect("apply failed");
    assert_eq!(ledger.report.changed_nodes, 3);
    assert!(!ledger.rows.is_empty());

    // Verify target has worth
    let new_target = fs::read_to_string(root.join("target.md")).unwrap();
    assert!(new_target.contains("worth: 0.35"));
    assert!(new_target.contains("standing_weight: 0.35"));

    // Verify task has migrated links
    let new_task = fs::read_to_string(root.join("task.md")).unwrap();
    assert!(new_task.contains(&format!("quantum: {SUPPORTS_QUANTUM}")));
    assert!(new_task.contains("set_by: migrated"));
    assert!(new_task.contains("quantum: 0.85"));

    // Verify hub has prototype type and edge_template
    let new_hub = fs::read_to_string(root.join("hub.md")).unwrap();
    assert!(new_hub.contains("type: prototype"));
    assert!(new_hub.contains("edge_template:"));

    // Revert migration using ledger
    let ledger_path = root
        .join(".agents")
        .join("migrations")
        .join(format!("{}.json", ledger.migration));
    assert!(ledger_path.exists());

    let revert_rep = revert_flow_migration(root, &ledger_path).expect("revert failed");
    assert_eq!(revert_rep.files_reverted, 3);
    assert!(revert_rep.drifted_rows.is_empty());
    assert!(revert_rep.ledger_removed);

    // M3 Criterion: git diff against snapshot_sha MUST BE EMPTY!
    let diff_out = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("diff")
        .arg(&snapshot_sha)
        .output()
        .unwrap();
    assert!(diff_out.status.success());
    let diff_str = String::from_utf8_lossy(&diff_out.stdout);
    assert!(
        diff_str.trim().is_empty(),
        "git diff against snapshot must be completely empty! Diff was:\n{diff_str}"
    );

    // Byte-identical checks
    assert_eq!(fs::read_to_string(root.join("target.md")).unwrap(), target_content);
    assert_eq!(fs::read_to_string(root.join("task.md")).unwrap(), task_content);
    let original_hub = format!(
        r#"---
id: {PROTOTYPE_HUB_ID}
title: Respond to prospective PhD students
type: epic
status: ready
contributes_to:
- stated_weight: probable
  to: targ-1
---

Hub body.
"#
    );
    assert_eq!(fs::read_to_string(root.join("hub.md")).unwrap(), original_hub);
}

#[test]
fn test_m4_second_apply_writes_nothing() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    let target_content = r#"---
id: targ-1
title: Target
type: target
standing_weight: 0.5
---

Body.
"#;
    fs::write(root.join("target.md"), target_content).unwrap();
    commit_all(root, "Initial snapshot");

    let ledger1 = apply_flow_migration(root).unwrap();
    assert_eq!(ledger1.report.changed_nodes, 1);

    // Second apply on clean tree should write nothing
    let ledger2 = apply_flow_migration(root).unwrap();
    assert_eq!(ledger2.report.changed_nodes, 0);
    assert_eq!(ledger2.rows.len(), 0);
}

#[test]
fn test_m14_cleanup_refuses_without_confirm() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    let target_content = r#"---
id: targ-1
title: Target
type: target
standing_weight: 0.5
---

Body.
"#;
    fs::write(root.join("target.md"), target_content).unwrap();
    commit_all(root, "Initial");

    let ledger = apply_flow_migration(root).unwrap();
    let ledger_path = root
        .join(".agents")
        .join("migrations")
        .join(format!("{}.json", ledger.migration));

    // Cleanup without confirm should fail
    let res = cleanup_flow_migration(root, &ledger_path, false);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("--confirm"));

    // Cleanup with confirm should succeed and delete standing_weight
    let rep = cleanup_flow_migration(root, &ledger_path, true).unwrap();
    assert_eq!(rep.files_cleaned, 1);
    let cleaned_content = fs::read_to_string(root.join("target.md")).unwrap();
    assert!(!cleaned_content.contains("standing_weight:"));
    assert!(cleaned_content.contains("worth: 0.5"));
}

#[test]
fn test_m15_status_and_drift_detection() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    let task_content = r#"---
id: task-1
title: Task One
type: task
soft_depends_on:
- targ-1
---

Body.
"#;
    fs::write(root.join("task.md"), task_content).unwrap();

    let target_content = r#"---
id: targ-1
title: Target
type: target
---

Body.
"#;
    fs::write(root.join("target.md"), target_content).unwrap();
    commit_all(root, "Initial");

    let ledger = apply_flow_migration(root).unwrap();

    // Check status before drift
    let st = status_flow_migration(root, None).unwrap();
    assert!(st.migration_applied);
    assert_eq!(st.drifted_rows.len(), 0);

    // Edit the migrated line in task.md to simulate drift (e.g. quantum changed to 0.7)
    let modified = fs::read_to_string(root.join("task.md"))
        .unwrap()
        .replace("quantum: 0.3", "quantum: 0.7");
    fs::write(root.join("task.md"), modified).unwrap();

    // Check status after drift
    let st2 = status_flow_migration(root, None).unwrap();
    assert_eq!(st2.drifted_rows.len(), 1);
    assert_eq!(st2.drifted_rows[0].key_path, "soft_depends_on[0]");

    // Revert skips the drifted row
    let ledger_path = root
        .join(".agents")
        .join("migrations")
        .join(format!("{}.json", ledger.migration));
    let rep = revert_flow_migration(root, &ledger_path).unwrap();
    assert_eq!(rep.drifted_rows.len(), 1);
    assert_eq!(rep.files_reverted, 0);

    // The file still contains quantum: 0.7
    let post_revert = fs::read_to_string(root.join("task.md")).unwrap();
    assert!(post_revert.contains("quantum: 0.7"));
}

#[test]
fn test_patch_file_text_unit() {
    let content = r#"---
id: my-node
title: My Node
type: task
standing_weight: 0.6
soft_depends_on:
- dep-1
contributes_to:
- stated_weight: Expected
  to: targ-1
---

## Body Content
Preserved exactly.
"#;

    let mut known = HashSet::new();
    known.insert("dep-1".to_string());
    known.insert("targ-1".to_string());

    let fm_val = serde_json::json!({
        "id": "my-node",
        "standing_weight": 0.6,
        "soft_depends_on": ["dep-1"],
        "contributes_to": [{
            "stated_weight": "Expected",
            "to": "targ-1"
        }]
    });

    let (patched, rows) = patch_file_text(content, "my-node", &fm_val, "my-node.md", &known);
    assert_eq!(rows.len(), 3);
    assert!(patched.contains("worth: 0.6"));
    assert!(patched.contains("quantum: 0.3"));
    assert!(patched.contains("quantum: 0.75"));
    assert!(patched.contains("## Body Content\nPreserved exactly."));

    let (reverted, drift) = revert_file_content(&patched, &rows);
    assert!(drift.is_empty());
    assert_eq!(reverted, content);
}
