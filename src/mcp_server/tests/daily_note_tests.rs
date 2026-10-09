use super::*;
use serde_json::json;

#[test]
fn daily_note_create_and_rewrite_leaves_exactly_one_file() {
    let server = build_test_server();

    // 1. Create a daily note through PKB tools
    let create_res = server
        .handle_create_document(&json!({
            "id": "20261009-daily",
            "type": "daily",
            "title": "20261009-daily-Friday",
            "body": "# Friday 9 October 2026\n\nInitial daily note content."
        }))
        .unwrap();

    let create_text: String = create_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    assert!(create_text.contains("Document created:"), "Expected creation success, got: {create_text}");

    let daily_dir = server.pkb_root.join("daily");
    let target_file = daily_dir.join("20261009-daily.md");

    assert!(
        target_file.exists(),
        "Expected daily note at {:?}, but file does not exist",
        target_file
    );

    let entries_after_create: Vec<String> = std::fs::read_dir(&daily_dir)
        .expect("daily/ directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries_after_create,
        vec!["20261009-daily.md"],
        "Expected exactly one file in daily/ after create, got: {:?}",
        entries_after_create
    );

    // 2. Rewrite the daily note through PKB tools (update_body)
    let update_res = server
        .handle_update_body(&json!({
            "id": "20261009-daily",
            "new_body": "# Friday 9 October 2026\n\nRewritten daily note content with latest tasks."
        }))
        .unwrap();

    let update_text: String = update_res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    assert!(update_text.contains("Updated:"), "Expected update success, got: {update_text}");

    // 3. Verify exactly one file exists at daily/YYYYMMDD-daily.md
    assert!(
        target_file.exists(),
        "Target file {:?} must exist after rewrite",
        target_file
    );

    let entries_after_rewrite: Vec<String> = std::fs::read_dir(&daily_dir)
        .expect("daily/ directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries_after_rewrite,
        vec!["20261009-daily.md"],
        "Expected exactly one file in daily/ after rewrite, got: {:?}",
        entries_after_rewrite
    );

    let content = std::fs::read_to_string(&target_file).unwrap();
    assert!(
        content.contains("Rewritten daily note content with latest tasks."),
        "File should contain rewritten body"
    );
    assert!(
        content.contains("id: 20261009-daily\n"),
        "File frontmatter should have id: 20261009-daily"
    );
}

#[test]
fn daily_note_lands_at_daily_yyyymmdd_daily_whatever_title_it_carries() {
    let server = build_test_server();

    // Daily run with title carrying weekday
    server
        .handle_create_document(&json!({
            "id": "20261010-daily",
            "type": "daily",
            "title": "20261010-daily-Saturday",
            "body": "Body 1"
        }))
        .unwrap();

    let target_file_1 = server.pkb_root.join("daily/20261010-daily.md");
    assert!(
        target_file_1.exists(),
        "Expected file at {:?}, slug must not be appended to filename",
        target_file_1
    );

    // Another daily run with an arbitrary descriptive title
    server
        .handle_create_document(&json!({
            "id": "20261011-daily",
            "type": "daily",
            "title": "Sunday 11 October 2026 - Sprint Planning & Strategy",
            "body": "Body 2"
        }))
        .unwrap();

    let target_file_2 = server.pkb_root.join("daily/20261011-daily.md");
    assert!(
        target_file_2.exists(),
        "Expected file at {:?}, arbitrary title slug must not be appended to filename",
        target_file_2
    );
}
