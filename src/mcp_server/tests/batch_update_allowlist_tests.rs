//! Tests for `mem_c9bbddaf` / `mem_84b21efc`: `update_task` and `batch_update`
//! accept unrecognised frontmatter keys without checking or refusing them,
//! leaving unmentioned keys untouched while still enforcing known-key field
//! format validation (`validate_update_field`) and handback status rules.
//!
//! These tests exercise both entry points side by side through the real MCP
//! handlers (`handle_update_task` / `handle_batch_update`), not the bare
//! library functions, so they catch drift at the layer a caller actually
//! hits.

use super::*;

fn make_server(root: &Path) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    let graph = GraphStore::build(&[], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    )
}

fn text_of(res: &CallToolResult) -> String {
    res.content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>()
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    let text = text_of(res);
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

/// Whether `key` is present in the task's on-disk `frontmatter` object, per
/// `get_task`. Deliberately narrower than a whole-document substring search:
/// a naive `doc_text.contains(key)` false-positives whenever the key name
/// also happens to appear in the task's title or body prose.
fn frontmatter_has_key(get_task_res: &CallToolResult, key: &str) -> bool {
    json_of(get_task_res)
        .get("frontmatter")
        .and_then(|fm| fm.as_object())
        .map(|obj| obj.contains_key(key))
        .unwrap_or(false)
}

/// Error strings from a `batch_update` summary's `errors[]`. A `dry_run`
/// summary is prefixed with a human-readable warning banner before the JSON
/// body, so strip that first.
fn batch_errors(res: &CallToolResult) -> Vec<String> {
    let text = text_of(res);
    let json_start = text.find('{').unwrap_or(0);
    let val: serde_json::Value = serde_json::from_str(&text[json_start..])
        .unwrap_or_else(|_| panic!("expected batch_update JSON, got: {text}"));
    val.get("errors")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("error").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn batch_changed(res: &CallToolResult) -> u64 {
    let text = text_of(res);
    let json_start = text.find('{').unwrap_or(0);
    let val: serde_json::Value = serde_json::from_str(&text[json_start..])
        .unwrap_or_else(|_| panic!("expected batch_update JSON, got: {text}"));
    val.get("changed").and_then(|v| v.as_u64()).unwrap_or(0)
}

fn create_task(server: &PkbSearchServer, title: &str) -> String {
    let res = server
        .handle_create_task(&json!({
            "title": title,
            "slug": "slug",
            "type": "task",
            "project": "proj-partial",
            "parent": "proj-partial",
            "allow_missing_parent": true,
        }))
        .expect("create_task should succeed");
    json_of(&res)
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}

/// Acceptance criterion (mem_c9bbddaf): `pkb_update_task` (and `pkb_batch_update`)
/// writes a key it does not recognise without refusing it, and leaves unrecognised
/// keys alone when they are not mentioned.
#[test]
fn update_task_and_batch_update_write_unrecognised_keys_and_leave_unmentioned_keys_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Unrecognised key fixture");

    // 1. Write an unrecognised key via update_task (nested form)
    let update_res = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "custom_unrecognised_key": "custom_value_1" },
        }))
        .expect("update_task must write an unrecognised key without refusing it");
    assert!(!update_res.is_error.unwrap_or(false));

    let doc = server
        .handle_get_task(&json!({ "id": id }))
        .expect("get_task should succeed");
    let fm = json_of(&doc);
    let frontmatter = fm.get("frontmatter").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        frontmatter
            .get("custom_unrecognised_key")
            .and_then(|v| v.as_str()),
        Some("custom_value_1"),
        "unrecognised key must be written to frontmatter"
    );

    // 2. Write a second unrecognised key via batch_update while leaving the first unmentioned
    let batch_res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "second_unrecognised_key": "custom_value_2" },
            "dry_run": false,
        }))
        .expect("handle_batch_update must succeed");
    assert_eq!(
        batch_errors(&batch_res),
        Vec::<String>::new(),
        "batch_update must not refuse unrecognised keys"
    );
    assert_eq!(batch_changed(&batch_res), 1);

    // 3. Subsequent update to a known key leaves both unrecognised keys untouched
    server
        .handle_update_task(&json!({
            "id": id,
            "status": "ready",
        }))
        .expect("update_task on status must succeed");

    let doc_after = server
        .handle_get_task(&json!({ "id": id }))
        .expect("get_task should succeed");
    let fm_after = json_of(&doc_after);
    let frontmatter_after = fm_after
        .get("frontmatter")
        .and_then(|v| v.as_object())
        .unwrap();
    assert_eq!(
        frontmatter_after
            .get("custom_unrecognised_key")
            .and_then(|v| v.as_str()),
        Some("custom_value_1"),
        "unmentioned unrecognised key 1 must be left alone"
    );
    assert_eq!(
        frontmatter_after
            .get("second_unrecognised_key")
            .and_then(|v| v.as_str()),
        Some("custom_value_2"),
        "unmentioned unrecognised key 2 must be left alone"
    );
}

/// Acceptance criterion (mem_c9bbddaf): `branch`, `container`, and `alias` still write
/// (and `alias` can be updated as string or array and cleared with null).
#[test]
fn update_task_still_writes_branch_container_and_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Branch container alias fixture");

    let res = server
        .handle_update_task(&json!({
            "id": id,
            "updates": {
                "branch": "pr-2735",
                "container": "polecat-0a9f8a5d",
                "alias": ["custom_alias_1", "custom_alias_2"]
            }
        }))
        .expect("update_task must write branch, container, and alias");
    assert!(!res.is_error.unwrap_or(false));

    let doc = server
        .handle_get_task(&json!({ "id": id }))
        .expect("get_task should succeed");
    let fm = json_of(&doc);
    let frontmatter = fm.get("frontmatter").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        frontmatter.get("branch").and_then(|v| v.as_str()),
        Some("pr-2735"),
        "frontmatter must carry branch: pr-2735"
    );
    assert_eq!(
        frontmatter.get("container").and_then(|v| v.as_str()),
        Some("polecat-0a9f8a5d"),
        "frontmatter must carry container: polecat-0a9f8a5d"
    );
    let alias_arr = frontmatter
        .get("alias")
        .and_then(|v| v.as_array())
        .expect("alias should be written as array");
    assert_eq!(alias_arr.len(), 2);
    assert_eq!(alias_arr[0].as_str(), Some("custom_alias_1"));
    assert_eq!(alias_arr[1].as_str(), Some("custom_alias_2"));
}

/// `released_at` decision (mem_84b21efc): the field is not permanently
/// sticky — GitHub #574 documents a legitimate need to clear it when a
/// release was wrong, and `release_task`'s sibling fields (`release_summary`,
/// `pr_url`, `session_id`) were already freely settable via `update_task`
/// with no such restriction. Both entry points accept and clear it identically.
#[test]
fn released_at_is_clearable_through_both_update_task_and_batch_update() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    let id1 = create_task(&server, "Un-release fixture, single-node path");
    server
        .handle_update_task(&json!({
            "id": id1,
            "updates": { "released_at": "2026-08-27T05:15:39Z" },
        }))
        .expect("update_task must accept released_at");
    let after_set = server.handle_get_task(&json!({ "id": id1 })).unwrap();
    assert!(
        frontmatter_has_key(&after_set, "released_at"),
        "released_at must be written before it can be tested as clearable"
    );
    server
        .handle_update_task(&json!({ "id": id1, "updates": { "released_at": null } }))
        .expect("update_task must be able to clear released_at (un-release correction, #574)");
    let after_clear = server.handle_get_task(&json!({ "id": id1 })).unwrap();
    assert!(
        !frontmatter_has_key(&after_clear, "released_at"),
        "released_at must be gone after clearing via update_task"
    );

    let id2 = create_task(&server, "Un-release fixture, batch path");
    let set_res = server
        .handle_batch_update(&json!({
            "ids": [id2],
            "updates": { "released_at": "2026-08-27T05:15:39Z" },
            "dry_run": false,
        }))
        .unwrap();
    assert_eq!(
        batch_errors(&set_res),
        Vec::<String>::new(),
        "batch_update must accept released_at"
    );
    assert_eq!(batch_changed(&set_res), 1);

    let clear_res = server
        .handle_batch_update(&json!({
            "ids": [id2],
            "updates": { "released_at": null },
            "dry_run": false,
        }))
        .unwrap();
    assert_eq!(
        batch_errors(&clear_res),
        Vec::<String>::new(),
        "batch_update must be able to clear released_at, matching update_task"
    );
    assert_eq!(batch_changed(&clear_res), 1);
    let doc2 = server.handle_get_task(&json!({ "id": id2 })).unwrap();
    assert!(
        !frontmatter_has_key(&doc2, "released_at"),
        "released_at must be gone after clearing via batch_update"
    );
}

/// Second guard closed by the same fix: `release_task` refuses to release a
/// single node to a handback status (blocked/cancelled/review/partial)
/// without a non-empty `reason`/`blocker`. `batch_update` writing
/// `status: cancelled` directly must enforce the identical contract, so a
/// bulk write cannot silently produce a terminal node with no record of why.
#[test]
fn batch_update_to_cancelled_requires_a_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Bulk terminal-status fixture");

    let no_reason_res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "status": "cancelled" },
            "dry_run": false,
        }))
        .unwrap();
    assert_eq!(
        batch_changed(&no_reason_res),
        0,
        "must not apply status=cancelled with no reason"
    );
    let errors = batch_errors(&no_reason_res);
    assert!(
        errors.iter().any(|e| e.to_lowercase().contains("reason")),
        "batch_update to status=cancelled with no reason must be refused, matching \
         release_task's evidence-or-failure-reason contract. Got errors: {errors:?}"
    );

    // With a reason supplied, the same call must succeed.
    let with_reason_res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "status": "cancelled", "reason": "superseded by a rewrite" },
            "dry_run": false,
        }))
        .unwrap();
    assert_eq!(batch_errors(&with_reason_res), Vec::<String>::new());
    assert_eq!(batch_changed(&with_reason_res), 1);

    let doc = server.handle_get_task(&json!({ "id": id })).unwrap();
    assert_eq!(
        json_of(&doc).get("status").and_then(|v| v.as_str()),
        Some("cancelled"),
        "status must have been written once a reason was supplied"
    );
}
