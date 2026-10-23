//! `aops_pkb_path_lint`: body-writing tools reject a *new* machine-specific
//! path to a PKB file (`~/brain/...`, `/home/nic/brain/...`,
//! `/Users/suzor/brain/...`, or the server's own root), naming the offending
//! string, while a PKB-root-relative path or a wikilink to the same file
//! passes. Exercised through the real MCP handlers, not the bare lint
//! function, so the tests catch drift at the layer a caller hits.

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

fn create_task(server: &PkbSearchServer, body: &str) -> Result<String, McpError> {
    let res = server.handle_create_task(&json!({
        "title": "Path lint fixture",
        "slug": "path-lint-fixture",
        "type": "task",
        "project": "proj-test",
        "parent": "proj-test",
        "allow_missing_parent": true,
        "body": body,
    }))?;
    Ok(json_of(&res)
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string())
}

const OFFENDING: &str = "~/brain/knowledge/framework/foo.md";
const RELATIVE: &str = "knowledge/framework/foo.md";
const WIKILINK: &str = "[[mcp-tool-design-id-over-paths]]";

fn assert_rejected(err: McpError, offending: &str) {
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{}", err.message);
    assert!(
        err.message.contains(offending),
        "rejection must name the offending string {offending:?}: {}",
        err.message
    );
    let data = err.data.expect("structured error data");
    assert_eq!(data["error_type"], "machine_specific_path");
    assert_eq!(data["paths"][0]["matched"], offending);
}

// ── create ───────────────────────────────────────────────────────────────

#[test]
fn create_document_rejects_machine_path_and_names_it() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let err = server
        .handle_create_document(&json!({
            "title": "Note",
            "slug": "note",
            "type": "note",
            "body": format!("See {OFFENDING} for the principle."),
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    // Nothing was written.
    assert!(server.graph.read().resolve("note").is_none());
}

#[test]
fn create_document_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let res = server
        .handle_create_document(&json!({
            "title": "Note",
            "slug": "note",
            "type": "note",
            "body": format!("See `{RELATIVE}` or {WIKILINK} for the principle."),
        }))
        .unwrap();
    assert!(text_of(&res).contains("Document created"));
}

#[test]
fn create_task_rejects_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let err = create_task(&server, &format!("Read /home/nic/brain/{RELATIVE}")).unwrap_err();
    assert_rejected(err, &format!("/home/nic/brain/{RELATIVE}"));
}

#[test]
fn create_memory_rejects_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let err = server
        .handle_create_memory(&json!({
            "title": "Memory",
            "slug": "memory",
            "body": format!("Read /Users/suzor/brain/{RELATIVE}"),
        }))
        .unwrap_err();
    assert_rejected(err, &format!("/Users/suzor/brain/{RELATIVE}"));
}

#[test]
fn create_rejects_path_under_the_servers_own_root() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let own = format!("{}/{RELATIVE}", tmp.path().display());
    let err = create_task(&server, &format!("Read {own}")).unwrap_err();
    assert_rejected(err, &own);
}

#[test]
fn prose_naming_the_mount_points_is_not_a_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let body = "The PKB is replicated under different mount points (`~/brain`, \
                `/home/nic/brain`, `/data`, `/Users/suzor/...`); a note naming \
                one of them resolves on one machine only.";
    create_task(&server, body).expect("prose about the roots must pass");
}

// ── append / add_observations ────────────────────────────────────────────

#[test]
fn append_rejects_machine_path_and_leaves_document_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let err = server
        .handle_append_to_document(&json!({
            "id": id,
            "content": format!("Also see {OFFENDING}"),
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(
        !doc.contains("Also see"),
        "append must not have written: {doc}"
    );
}

#[test]
fn append_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    server
        .handle_append_to_document(&json!({
            "id": id,
            "content": format!("Also see `{RELATIVE}` and {WIKILINK}"),
        }))
        .unwrap();
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(doc.contains(RELATIVE) && doc.contains(WIKILINK));
}

#[test]
fn add_observations_rejects_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let err = server
        .handle_add_observations(&json!({
            "id": id,
            "lines": ["fine", format!("bad {OFFENDING}")],
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
}

// ── update_body ──────────────────────────────────────────────────────────

#[test]
fn update_body_rejects_new_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let err = server
        .handle_update_body(&json!({
            "id": id,
            "new_body": format!("Rewritten. See {OFFENDING}."),
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(
        doc.contains("Clean body."),
        "update_body must not have written: {doc}"
    );
}

#[test]
fn update_body_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let res = server
        .handle_update_body(&json!({
            "id": id,
            "new_body": format!("Rewritten. See `{RELATIVE}` and {WIKILINK}."),
        }))
        .unwrap();
    assert_eq!(json_of(&res)["ok"], true);
}

#[test]
fn update_body_tolerates_a_path_the_note_already_had() {
    // Existing notes still carry old machine paths until the sweep rewrites
    // them; a rewrite that keeps one must not be blocked, only one that adds
    // a new one. Seed the offending path on disk directly, bypassing the lint.
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let abs_path = {
        let g = server.graph.read();
        let node = g.resolve(&id).unwrap();
        server.abs_path(&node.path)
    };
    let seeded = std::fs::read_to_string(&abs_path)
        .unwrap()
        .replace("Clean body.", &format!("Old note mentions {OFFENDING}"));
    std::fs::write(&abs_path, seeded).unwrap();

    server
        .handle_update_body(&json!({
            "id": id,
            "new_body": format!("Rewritten, still mentions {OFFENDING}"),
        }))
        .expect("keeping an existing machine path must pass");

    let err = server
        .handle_update_body(&json!({
            "id": id,
            "new_body": format!("Rewritten, mentions {OFFENDING} and ~/brain/tasks/new.md"),
        }))
        .unwrap_err();
    assert_rejected(err, "~/brain/tasks/new.md");
}

#[test]
fn update_task_body_rejects_new_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let err = server
        .handle_update_task(&json!({
            "id": id,
            "updates": { "body": format!("See {OFFENDING}") },
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
}

// ── edit_body ────────────────────────────────────────────────────────────

#[test]
fn edit_body_rejects_diff_that_adds_a_machine_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let err = server
        .handle_edit_body(&json!({
            "id": id,
            "diff": format!("@@ -1,1 +1,2 @@\n Clean body.\n+See {OFFENDING}\n"),
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
}

#[test]
fn edit_body_passes_diff_adding_relative_path() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let diff = format!("@@ -1,1 +1,2 @@\n Clean body.\n+See `{RELATIVE}` and {WIKILINK}\n");
    let res = server
        .handle_edit_body(&json!({ "id": id, "diff": diff }))
        .unwrap();
    assert_eq!(json_of(&res)["ok"], true);
}

#[test]
fn edit_body_ignores_machine_paths_in_removed_and_context_lines() {
    // A note that still carries an old machine path can be edited — even to
    // remove that path — as long as the added lines are clean.
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Clean body.").unwrap();
    let abs_path = {
        let g = server.graph.read();
        let node = g.resolve(&id).unwrap();
        server.abs_path(&node.path)
    };
    let seeded = std::fs::read_to_string(&abs_path).unwrap().replace(
        "Clean body.",
        &format!("Old {OFFENDING}\nKeep ~/brain/tasks/ctx.md"),
    );
    std::fs::write(&abs_path, seeded).unwrap();

    let diff = format!(
        "@@ -1,2 +1,2 @@\n-Old {OFFENDING}\n+New `{RELATIVE}`\n Keep ~/brain/tasks/ctx.md\n"
    );
    let res = server
        .handle_edit_body(&json!({ "id": id, "diff": diff }))
        .expect("removed/context lines must not trigger the lint");
    assert_eq!(json_of(&res)["ok"], true);
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(!doc.contains(OFFENDING) && doc.contains(RELATIVE), "{doc}");
}

// ── decompose_task ───────────────────────────────────────────────────────

#[test]
fn decompose_task_subtask_body_rejects_machine_path_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let parent_id = create_task(&server, "Parent task body.").unwrap();
    let before_count = server.graph.read().node_count();
    let err = server
        .handle_decompose_task(&json!({
            "parent_id": parent_id,
            "subtasks": [
                {
                    "title": "Subtask with bad body",
                    "body": format!("See {OFFENDING} for the steps."),
                }
            ],
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    // Only the pre-existing tasks exist, no subtask written
    assert_eq!(server.graph.read().node_count(), before_count);
}

#[test]
fn decompose_task_subtask_body_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let parent_id = create_task(&server, "Parent task body.").unwrap();
    let res = server
        .handle_decompose_task(&json!({
            "parent_id": parent_id,
            "subtasks": [
                {
                    "title": "Subtask with good body",
                    "body": format!("See `{RELATIVE}` or {WIKILINK} for the steps."),
                }
            ],
        }))
        .unwrap();
    let text = text_of(&res);
    assert!(text.contains("Created 1 subtasks"));
    let subtask_id = {
        let graph = server.graph.read();
        let subtask = graph
            .nodes()
            .find(|n| n.label == "Subtask with good body")
            .unwrap();
        subtask.id.clone()
    };
    let doc = text_of(
        &server
            .handle_get_document(&json!({"id": subtask_id}))
            .unwrap(),
    );
    assert!(doc.contains(RELATIVE) && doc.contains(WIKILINK));
}

// ── release_task (ad-hoc) ────────────────────────────────────────────────

#[test]
fn release_task_adhoc_body_rejects_machine_path_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let err = server
        .handle_release_task(&json!({
            "project": "proj-test",
            "status": "done",
            "summary": "Completed adhoc work",
            "body": format!("Ad-hoc body with {OFFENDING}"),
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    assert_eq!(server.graph.read().node_count(), 0);
}

#[test]
fn release_task_adhoc_body_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let res = server
        .handle_release_task(&json!({
            "project": "proj-test",
            "status": "done",
            "summary": "Completed adhoc work",
            "body": format!("Ad-hoc body with `{RELATIVE}` and {WIKILINK}"),
        }))
        .unwrap();
    let json = json_of(&res);
    let id = json["id"].as_str().unwrap();
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(doc.contains(RELATIVE) && doc.contains(WIKILINK));
}

// ── batch tools ──────────────────────────────────────────────────────────

#[test]
fn batch_update_body_rejects_machine_path_and_leaves_documents_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Original body.").unwrap();
    let err = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": {
                "body": format!("Batch updated body mentioning {OFFENDING}"),
            },
            "dry_run": false,
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(doc.contains("Original body."));
    assert!(!doc.contains(OFFENDING));
}

#[test]
fn batch_update_body_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Original body.").unwrap();
    let res = server
        .handle_batch_update(&json!({
            "ids": [id],
            "updates": {
                "body": format!("Batch updated body mentioning `{RELATIVE}` and {WIKILINK}"),
            },
            "dry_run": false,
        }))
        .unwrap();
    let json = json_of(&res);
    assert_eq!(json["changed"], 1);
    let doc = text_of(&server.handle_get_document(&json!({"id": id})).unwrap());
    assert!(doc.contains(RELATIVE) && doc.contains(WIKILINK));
}

#[test]
fn batch_create_epics_body_rejects_machine_path_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Child task.").unwrap();
    let before_count = server.graph.read().node_count();
    let err = server
        .handle_batch_create_epics(&json!({
            "epics": [
                {
                    "title": "New Epic",
                    "task_ids": [id],
                    "body": format!("Epic body with {OFFENDING}"),
                }
            ],
            "dry_run": false,
        }))
        .unwrap_err();
    assert_rejected(err, OFFENDING);
    assert_eq!(server.graph.read().node_count(), before_count);
}

#[test]
fn batch_create_epics_body_passes_relative_path_and_wikilink() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let id = create_task(&server, "Child task.").unwrap();
    let res = server
        .handle_batch_create_epics(&json!({
            "epics": [
                {
                    "title": "New Epic",
                    "task_ids": [&id],
                    "body": format!("Epic body with `{RELATIVE}` and {WIKILINK}"),
                }
            ],
            "dry_run": false,
        }))
        .unwrap();
    let json = json_of(&res);
    assert_eq!(json["changed"], 1);
    let epic_id = json["tasks"][0]["id"].as_str().unwrap();
    let doc = text_of(&server.handle_get_document(&json!({"id": epic_id})).unwrap());
    assert!(doc.contains(RELATIVE) && doc.contains(WIKILINK));
}
