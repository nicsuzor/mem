//! Tests for field format validation in `update_task` and `batch_update` (mem_07e63c6c).
//!
//! Rejects malformed field values (e.g. `contributes_to` as a string, list of bare IDs,
//! non-integer intent, invalid status, malformed tags array) with an error naming the
//! field and the expected shape, instead of replying "Task updated" and saving nothing.

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
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .expect("create_task should succeed");
    json_of(&res)
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}

/// Acceptance criterion 1 & 2:
/// A red test shows a malformed value (`contributes_to` as a string) reporting success
/// without saving; the same test passes after the fix, error naming field and expected shape.
#[test]
fn test_update_task_refuses_malformed_contributes_to_string() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Malformed contributes_to string fixture");

    let res = server.handle_update_task(&json!({
        "id": id,
        "updates": { "contributes_to": "aops_8745d500" },
    }));

    let err = res.expect_err("update_task must refuse malformed contributes_to string with an error");
    assert!(
        err.message.contains("contributes_to"),
        "error message must name the field 'contributes_to', got: {}",
        err.message
    );
    assert!(
        err.message.contains("array of objects"),
        "error message must name the expected shape 'array of objects', got: {}",
        err.message
    );
}

/// Acceptance criterion 1 & 2:
/// `contributes_to` as a list of bare strings (e.g. `["aops_8745d500"]`).
#[test]
fn test_update_task_refuses_malformed_contributes_to_bare_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Malformed contributes_to bare IDs fixture");

    let res = server.handle_update_task(&json!({
        "id": id,
        "updates": { "contributes_to": ["aops_8745d500", "aops_12345678"] },
    }));

    let err = res.expect_err("update_task must refuse malformed contributes_to bare IDs with an error");
    assert!(
        err.message.contains("contributes_to"),
        "error message must name the field 'contributes_to', got: {}",
        err.message
    );
    assert!(
        err.message.contains("object"),
        "error message must name expected shape, got: {}",
        err.message
    );
}

/// Acceptance criterion 1 & 2:
/// `contributes_to` object without 'to' or 'target'.
#[test]
fn test_update_task_refuses_malformed_contributes_to_missing_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Malformed contributes_to missing target fixture");

    let res = server.handle_update_task(&json!({
        "id": id,
        "updates": { "contributes_to": [{ "stated_weight": "probable" }] },
    }));

    let err = res.expect_err("update_task must refuse contributes_to object without to/target");
    assert!(
        err.message.contains("contributes_to"),
        "error message must name the field 'contributes_to', got: {}",
        err.message
    );
    assert!(
        err.message.contains("target") || err.message.contains("to"),
        "error message must specify required 'to' or 'target' field, got: {}",
        err.message
    );
}

/// Acceptance criterion 1 & 2:
/// Batch update parity: `batch_update` must also refuse malformed `contributes_to`.
#[test]
fn test_batch_update_refuses_malformed_contributes_to() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Batch malformed contributes_to fixture");

    let res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": { "contributes_to": "aops_8745d500" },
            "dry_run": false,
        }))
        .expect("handle_batch_update returns Ok with errors in body");

    assert_eq!(
        batch_changed(&res),
        0,
        "batch_update must not report changed=1 on malformed contributes_to"
    );
    let errors = batch_errors(&res);
    assert!(
        errors.iter().any(|e| e.contains("contributes_to") && e.contains("array of objects")),
        "batch_update errors must name field and expected shape, got: {errors:?}"
    );
}

/// Acceptance criterion 2:
/// Other malformed fields (tags, status, intent, type, effort) return an error naming the field and expected shape.
#[test]
fn test_update_task_refuses_other_malformed_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Other malformed fields fixture");

    // 1. tags as a bare string
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "tags": "not-an-array" },
        }))
        .expect_err("update_task must refuse tags as a string");
    assert!(
        err.message.contains("tags") && err.message.contains("array of strings"),
        "tags error must name field and shape: {}",
        err.message
    );

    // 2. tags with non-string elements
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "tags": [123, 456] },
        }))
        .expect_err("update_task must refuse tags with non-string elements");
    assert!(
        err.message.contains("tags") && err.message.contains("string"),
        "tags element error must name field and shape: {}",
        err.message
    );

    // 3. intent as a string
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "intent": "high" },
        }))
        .expect_err("update_task must refuse intent as a string");
    assert!(
        err.message.contains("intent") && err.message.contains("integer"),
        "intent error must name field and shape: {}",
        err.message
    );

    // 4. status as a number
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "status": 123 },
        }))
        .expect_err("update_task must refuse status as a number");
    assert!(
        err.message.contains("status") && err.message.contains("string"),
        "status error must name field and shape: {}",
        err.message
    );

    // 5. type as a number
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "type": 123 },
        }))
        .expect_err("update_task must refuse type as a number");
    assert!(
        err.message.contains("type") && err.message.contains("string"),
        "type error must name field and shape: {}",
        err.message
    );

    // 6. effort as a number
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "effort": 123 },
        }))
        .expect_err("update_task must refuse effort as a number");
    assert!(
        err.message.contains("effort") && err.message.contains("string"),
        "effort error must name field and shape: {}",
        err.message
    );
}

/// Acceptance criterion 3:
/// Well-formed writes behave as before: valid contributes_to saves correctly,
/// and contributes_to: [] or null clears the field cleanly.
#[test]
fn test_well_formed_contributes_to_and_clearing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);
    let id = create_task(&server, "Well-formed contributes_to fixture");

    // 1. Well-formed write with target and weight
    let res = server
        .handle_update_task(&json!({
            "id": id,
            "updates": {
                "contributes_to": [{ "target": "aops_8745d500", "weight": "probable" }]
            },
        }))
        .expect("well-formed contributes_to must succeed");
    let text = text_of(&res);
    assert!(text.contains("updated"), "must report updated: {text}");

    // Verify it is readable from get_task
    let doc = server
        .handle_get_task(&json!({ "id": id }))
        .expect("get_task must succeed");
    let doc_json = json_of(&doc);
    let ct = doc_json
        .get("contributes_to")
        .and_then(|v| v.as_array())
        .expect("contributes_to should be an array in get_task");
    assert_eq!(ct.len(), 1, "contributes_to must contain 1 edge");

    // 2. Clear with contributes_to: []
    server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "contributes_to": [] },
        }))
        .expect("contributes_to: [] must succeed");
    let doc_after_empty = server.handle_get_task(&json!({ "id": id })).unwrap();
    let doc_after_empty_json = json_of(&doc_after_empty);
    let ct_after = doc_after_empty_json
        .get("contributes_to")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(ct_after, 0, "contributes_to must be empty after clearing with []");

    // 3. Clear with contributes_to: null
    // First set it back
    server
        .handle_update_task(&json!({
            "id": id,
            "updates": {
                "contributes_to": [{ "to": "aops_8745d500" }]
            },
        }))
        .unwrap();
    // Then clear with null
    server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "contributes_to": null },
        }))
        .expect("contributes_to: null must succeed");
    let doc_after_null = server.handle_get_task(&json!({ "id": id })).unwrap();
    let doc_after_null_json = json_of(&doc_after_null);
    let ct_null = doc_after_null_json
        .get("contributes_to")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(ct_null, 0, "contributes_to must be empty after clearing with null");
}
