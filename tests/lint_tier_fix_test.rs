use mem::lint::{self, FixMode, Severity};
use mem::pkb::parse_file_relative;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_lint_doc_tier_legacy_marker_archived_tag() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("legacy_tag.md");
    let initial = "---\nid: leg-00000001\ntitle: Legacy Archived\ntype: note\nstatus: ready\ntags:\n  - archived\n  - rust\n---\n\nPreserved body bytes.\n";
    fs::write(&file, initial).unwrap();

    let doc_before = parse_file_relative(&file, root).expect("parse before");
    let hash_before = doc_before.content_hash.clone();

    // 1. Lint detects doc-tier-legacy-marker
    let res = lint::lint_file_with_mode(&file, None, None, None, None);
    let diag = res
        .diagnostics
        .iter()
        .find(|d| d.rule == "doc-tier-legacy-marker")
        .expect("should flag doc-tier-legacy-marker");
    assert_eq!(diag.severity, Severity::Warning);
    assert!(diag.fixable);

    // 2. Fix with FixMode::Tier
    let res_fix = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    let fixed_content = res_fix.fixed_content.expect("should produce fixed content");
    fs::write(&file, &fixed_content).unwrap();

    // Check fixed content
    assert!(fixed_content.contains("status: archived"));
    assert!(!fixed_content.contains("status: ready"));
    assert!(!fixed_content.contains("- archived"));
    assert!(fixed_content.contains("- rust"));

    // Check content_hash preserved (body bytes untouched)
    let doc_after = parse_file_relative(&file, root).expect("parse after");
    assert_eq!(doc_after.content_hash, hash_before);
    assert_eq!(doc_after.status.as_deref(), Some("archived"));

    // 3. Idempotent: linting again produces no doc-tier-legacy-marker
    let res_again = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    assert!(
        res_again
            .diagnostics
            .iter()
            .all(|d| d.rule != "doc-tier-legacy-marker"),
        "idempotent pass should not flag doc-tier-legacy-marker"
    );
    assert!(res_again.fixed_content.is_none());
}

#[test]
fn test_lint_doc_tier_legacy_marker_archive_folder() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let archive_dir = root.join("archive");
    fs::create_dir_all(&archive_dir).unwrap();

    let file = archive_dir.join("old_spec.md");
    let initial = "---\nid: arc-00000002\ntitle: Old Spec\ntype: note\nstatus: ready\n---\n\nArchive folder body.\n";
    fs::write(&file, initial).unwrap();

    let doc_before = parse_file_relative(&file, root).expect("parse before");
    let hash_before = doc_before.content_hash.clone();

    // Lint flags archive folder
    let res = lint::lint_file_with_mode(&file, None, None, None, None);
    assert!(res.diagnostics.iter().any(|d| d.rule == "doc-tier-legacy-marker"));

    // Fix applies status: archived
    let res_fix = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    let fixed_content = res_fix.fixed_content.expect("fixed content");
    fs::write(&file, &fixed_content).unwrap();

    assert!(fixed_content.contains("status: archived"));
    let doc_after = parse_file_relative(&file, root).expect("parse after");
    assert_eq!(doc_after.content_hash, hash_before);
    assert_eq!(doc_after.status.as_deref(), Some("archived"));
}

#[test]
fn test_lint_doc_tier_legacy_marker_log_type() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("session_note.md");
    let initial = "---\nid: log-00000003\ntitle: Session Note\ntype: note\ntags:\n  - session-log\n  - ida\n---\n\nLog body.\n";
    fs::write(&file, initial).unwrap();

    let doc_before = parse_file_relative(&file, root).expect("parse before");
    let hash_before = doc_before.content_hash.clone();

    let res_fix = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    let fixed = res_fix.fixed_content.expect("fixed content");
    fs::write(&file, &fixed).unwrap();

    assert!(fixed.contains("type: session-log"));
    assert!(!fixed.contains("type: note"));
    assert!(!fixed.contains("- session-log"));
    assert!(fixed.contains("- ida"));

    let doc_after = parse_file_relative(&file, root).expect("parse after");
    assert_eq!(doc_after.content_hash, hash_before);
    assert_eq!(doc_after.doc_type.as_deref(), Some("session-log"));
}

#[test]
fn test_lint_doc_tier_legacy_marker_template_never_log() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("log_template.md");
    let initial = "---\nid: tpl-00000004\ntitle: Log Template\ntype: template\ntags:\n  - session-log\n---\n\nTemplate body.\n";
    fs::write(&file, initial).unwrap();

    let res = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    // Even if tags contains session-log, type: template must never be overwritten to session-log
    if let Some(fixed) = res.fixed_content {
        assert!(fixed.contains("type: template"));
    }
}

#[test]
fn test_lint_doc_daily_no_date() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("2026-10-24.md");
    let initial = "---\nid: day-20261024\ntitle: 2026-10-24\ntype: daily\n---\n\nDaily reflections.\n";
    fs::write(&file, initial).unwrap();

    let doc_before = parse_file_relative(&file, root).expect("parse before");
    let hash_before = doc_before.content_hash.clone();

    // 1. Flags doc-daily-no-date
    let res = lint::lint_file_with_mode(&file, None, None, None, None);
    let diag = res
        .diagnostics
        .iter()
        .find(|d| d.rule == "doc-daily-no-date")
        .expect("should flag doc-daily-no-date");
    assert_eq!(diag.severity, Severity::Warning);
    assert!(diag.fixable);

    // 2. Fix adds date: 2026-10-24
    let res_fix = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    let fixed = res_fix.fixed_content.expect("fixed content");
    fs::write(&file, &fixed).unwrap();

    assert!(fixed.contains("date: 2026-10-24"));
    let doc_after = parse_file_relative(&file, root).expect("parse after");
    assert_eq!(doc_after.content_hash, hash_before);

    // 3. Idempotent
    let res_again = lint::lint_file_with_mode(&file, Some(FixMode::Tier), None, None, None);
    assert!(
        res_again
            .diagnostics
            .iter()
            .all(|d| d.rule != "doc-daily-no-date"),
        "idempotent pass should not flag doc-daily-no-date"
    );
}

#[test]
fn test_lint_directory_tier_fix_and_dry_run() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("2026-10-24.md");
    let initial = "---\nid: day-20261024\ntitle: 2026-10-24\ntype: daily\n---\n\nDaily content.\n";
    fs::write(&file, initial).unwrap();

    // 1. Dry run: get fixes with FixMode::Tier but do not call write_fixes
    let (results, _) = lint::lint_directory_with_mode(root, Some(FixMode::Tier), false);
    assert_eq!(results.len(), 1);
    assert!(results[0].fixed_content.is_some());

    // File on disk remains unchanged (dry-run behavior)
    let content_on_disk = fs::read_to_string(&file).unwrap();
    assert_eq!(content_on_disk, initial);

    // 2. Apply fixes using write_fixes
    let written = lint::write_fixes(&results);
    assert_eq!(written, 1);

    // Now disk is updated
    let content_after = fs::read_to_string(&file).unwrap();
    assert!(content_after.contains("date: 2026-10-24"));
}

