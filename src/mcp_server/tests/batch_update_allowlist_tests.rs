//! Regression tests for `mem_84b21efc`: `batch_update` bypassed `update_task`'s
//! `UPDATE_KNOWN_KEYS` allowlist. Any frontmatter key `update_task` refuses to
//! write was still writable — and null-clearable — through `batch_update`,
//! because `batch_update` enforced no allowlist of its own before handing the
//! updates map to `document_crud::update_document`.
//!
//! These tests exercise both entry points side by side through the real MCP
//! handlers (`handle_update_task` / `handle_batch_update`), not the bare
//! library functions, so they catch drift at the layer a caller actually
//! hits. Note `handle_batch_update` never returns `Err` for a per-node
//! rejection — a batch operation reports per-item failures inside its JSON
//! `errors` array while the call itself still returns `Ok` (it only returns
//! `Err` for whole-request validation, e.g. "no filters"), so these tests
//! parse the summary JSON rather than expecting a top-level `Err`.

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

/// A key `update_task` refuses (never on `UPDATE_KNOWN_KEYS`) must be refused
/// through `batch_update` too, at write-time — not silently written to disk.
///
/// Before the fix: `update_task` rejects this with "Unknown keys in
/// update_task", but `batch_update` writes it straight through with no
/// validation and reports `changed: 1`.
#[test]
fn batch_update_rejects_the_same_unknown_key_update_task_rejects() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Allowlist parity fixture A");

    let update_task_err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "totally_bogus_field": "sneaky" },
        }))
        .expect_err("update_task must refuse an unrecognized frontmatter key");
    assert!(
        update_task_err.message.contains("Unknown keys in update_task"),
        "unexpected update_task error: {}",
        update_task_err.message
    );

    let batch_res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "totally_bogus_field": "sneaky" },
            "dry_run": false,
        }))
        .expect("handle_batch_update itself returns Ok; per-item rejection is in the JSON body");
    assert_eq!(
        batch_changed(&batch_res),
        0,
        "batch_update must not report the write as applied"
    );
    let errors = batch_errors(&batch_res);
    assert!(
        errors.iter().any(|e| e.contains("Unknown keys in batch_update")),
        "batch_update must refuse the same unrecognized key update_task refuses — it must not \
         be a second, unguarded entry point onto update_document. Got errors: {errors:?}"
    );

    // And the field must genuinely not have landed on disk.
    let doc = server
        .handle_get_task(&json!({ "id": id }))
        .expect("get_task should succeed");
    assert!(
        !frontmatter_has_key(&doc, "totally_bogus_field"),
        "rejected key must not appear in frontmatter"
    );
}

/// The reachable exploit from `mem_84b21efc`/GitHub `nicsuzor/mem#574`:
/// `batch_update` null-clearing a field `update_task` refuses to touch at
/// all. A null value is not a no-op fast-path around the allowlist — it must
/// be rejected exactly like a non-null write of the same key.
#[test]
fn batch_update_rejects_null_clear_of_an_unknown_key() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Allowlist parity fixture B");

    // `created` is a system-stamped field, never on UPDATE_KNOWN_KEYS.
    let batch_res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "created": null },
            "dry_run": false,
        }))
        .expect("handle_batch_update itself returns Ok; per-item rejection is in the JSON body");
    assert_eq!(batch_changed(&batch_res), 0);
    let errors = batch_errors(&batch_res);
    assert!(
        errors.iter().any(|e| e.contains("Unknown keys in batch_update")),
        "batch_update must refuse a null-clear of a key update_task refuses — \
         update_document's null-removes-the-key behavior is what made the original defect \
         reachable. Got errors: {errors:?}"
    );
}

/// `released_at` decision (mem_84b21efc): the field is not permanently
/// sticky — GitHub #574 documents a legitimate need to clear it when a
/// release was wrong, and `release_task`'s sibling fields (`release_summary`,
/// `pr_url`, `session_id`) were already freely settable via `update_task`
/// with no such restriction. `released_at` now joins the shared allowlist on
/// the same terms, so both entry points accept it identically.
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
        .expect("update_task must accept released_at now that it is on the shared allowlist");
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
    assert_eq!(batch_errors(&set_res), Vec::<String>::new(), "batch_update must accept released_at");
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
