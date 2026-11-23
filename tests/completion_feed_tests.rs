//! Integration tests for the completion feed in `list_tasks` and release/handling
//! metadata lifecycle (`released_at`, `origin_agent`, `handled_at`, `handled_by`,
//! `pr_url`, `branch`, `release_summary`, `reason`, `created`).

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use rmcp::model::ErrorCode;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

fn setup_server() -> (PkbSearchServer, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  proj-a: {}\n  proj-b: {}\n",
    )
    .unwrap();

    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("pkb_vectors.bin");

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    (server, tmp)
}

fn write_task_md(root: &Path, filename: &str, frontmatter: &str, body: &str) {
    let path = root.join("tasks").join(filename);
    fs::write(&path, format!("---\n{frontmatter}\n---\n\n{body}\n")).unwrap();
}

fn rebuild_from_disk(server: &PkbSearchServer, _root: &Path) {
    server
        .dispatch_tool_sync("refresh_graph", &json!({}))
        .expect("refresh_graph must succeed");
}

fn response_text(res: &rmcp::model::CallToolResult) -> String {
    res.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect()
}

fn response_json(res: &rmcp::model::CallToolResult) -> Value {
    let text = response_text(res);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("expected JSON ({e}), got:\n{text}"))
}

fn task_ids(res: &rmcp::model::CallToolResult) -> Vec<String> {
    let val = res
        .structured_content
        .clone()
        .unwrap_or_else(|| response_json(res));
    val.get("tasks")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn test_list_tasks_indexes_and_returns_completion_feed_fields_and_created() {
    let (server, _tmp) = setup_server();

    // Create task 1 with origin_agent, release as done with full release metadata, then mark handled.
    let created_done = server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "id": "task-done-meta",
                "title": "Completed task with metadata",
                "slug": "done-meta",
                "project": "proj-a",
                "origin_agent": "coordinator-a",
                "status": "ready"
            }),
        )
        .expect("create_task with origin_agent must succeed");
    assert_eq!(response_json(&created_done)["id"], "task-done-meta");

    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-done-meta",
                "status": "done",
                "summary": "Implemented completion feed",
                "pr_url": "https://github.com/example/repo/pull/42",
                "branch": "feat/completion-feed",
                "release_summary": "Indexed completion metadata and added feed filters"
            }),
        )
        .expect("release_task done must succeed");

    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "task-done-meta",
                "handled_at": "2026-11-23T06:30:00Z",
                "handled_by": "coordinator-a"
            }),
        )
        .expect("update_task handled_at/handled_by must succeed");

    // Create task 2 with origin_agent, release as failed with reason.
    server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "id": "task-failed-meta",
                "title": "Failed task with reason",
                "slug": "failed-meta",
                "project": "proj-a",
                "origin_agent": "coordinator-b",
                "status": "ready"
            }),
        )
        .expect("create_task for failed fixture must succeed");

    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-failed-meta",
                "status": "failed",
                "summary": "Build failed in CI",
                "reason": "Missing upstream test fixture",
                "branch": "fix/broken-fixture",
                "release_summary": "Aborted after fixture mismatch"
            }),
        )
        .expect("release_task failed must succeed");

    // Query JSON rows via list_tasks
    let res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,failed",
                "format": "json"
            }),
        )
        .expect("list_tasks json must succeed");

    let tasks = response_json(&res)["tasks"].as_array().unwrap().clone();
    assert_eq!(tasks.len(), 2);

    let done_row = tasks
        .iter()
        .find(|t| t["id"] == "task-done-meta")
        .expect("task-done-meta present");
    assert!(
        done_row["created"].as_str().is_some_and(|s| !s.is_empty()),
        "created must be returned on list_tasks JSON row: {done_row}"
    );
    assert!(
        done_row["released_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "released_at must be returned: {done_row}"
    );
    assert_eq!(done_row["origin_agent"], "coordinator-a");
    assert_eq!(done_row["handled_at"], "2026-11-23T06:30:00Z");
    assert_eq!(done_row["handled_by"], "coordinator-a");
    assert_eq!(
        done_row["pr_url"],
        "https://github.com/example/repo/pull/42"
    );
    assert_eq!(done_row["branch"], "feat/completion-feed");
    assert_eq!(
        done_row["release_summary"],
        "Indexed completion metadata and added feed filters"
    );
    assert!(done_row["reason"].is_null());

    let failed_row = tasks
        .iter()
        .find(|t| t["id"] == "task-failed-meta")
        .expect("task-failed-meta present");
    assert!(failed_row["created"].as_str().is_some());
    assert!(failed_row["released_at"].as_str().is_some());
    assert_eq!(failed_row["origin_agent"], "coordinator-b");
    assert!(failed_row["handled_at"].is_null());
    assert!(failed_row["handled_by"].is_null());
    assert_eq!(failed_row["branch"], "fix/broken-fixture");
    assert_eq!(
        failed_row["release_summary"],
        "Aborted after fixture mismatch"
    );
    assert_eq!(failed_row["reason"], "Missing upstream test fixture");

    // Verify `fields` projection includes any requested subset cleanly
    let proj_res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done",
                "fields": ["id", "created", "released_at", "origin_agent", "handled_at", "handled_by", "pr_url", "branch", "release_summary", "reason"]
            }),
        )
        .expect("list_tasks with fields projection must succeed");
    let proj_tasks = response_json(&proj_res)["tasks"].as_array().unwrap().clone();
    assert_eq!(proj_tasks.len(), 1);
    let obj = proj_tasks[0].as_object().unwrap();
    assert_eq!(obj.len(), 10);
    assert_eq!(obj["origin_agent"], "coordinator-a");
    assert_eq!(obj["pr_url"], "https://github.com/example/repo/pull/42");

    // Verify markdown release-feed mode also surfaces release metadata in both text and structured_content
    let md_res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,failed",
                "sort": "released_at_desc"
            }),
        )
        .expect("list_tasks markdown release feed must succeed");
    let md_text = response_text(&md_res);
    assert!(md_text.contains("| # | ID | Status | Released At | Origin | Handled | PR / Branch | Summary / Reason | Title |"));
    assert!(md_text.contains("coordinator-a"));
    assert!(md_text.contains("https://github.com/example/repo/pull/42"));
    assert!(md_text.contains("Missing upstream test fixture"));

    let structured = md_res
        .structured_content
        .as_ref()
        .expect("markdown mode must still provide structured_content");
    let st_tasks = structured["tasks"].as_array().unwrap();
    let st_done = st_tasks
        .iter()
        .find(|t| t["id"] == "task-done-meta")
        .unwrap();
    assert_eq!(st_done["origin_agent"], "coordinator-a");
    assert_eq!(st_done["handled_by"], "coordinator-a");
    assert!(st_done["created"].as_str().is_some());
}

#[test]
fn test_list_tasks_sort_released_at_desc_ordering_and_timezones() {
    let (server, tmp) = setup_server();
    let root = tmp.path();

    write_task_md(
        root,
        "t-early.md",
        "id: t-early\ntitle: Early\ntype: task\nstatus: done\nreleased_at: 2026-11-23T04:00:00Z\nmodified: 2026-11-23T04:00:00Z",
        "Early body",
    );
    write_task_md(
        root,
        "t-mid-offset.md",
        "id: t-mid-offset\ntitle: Mid with timezone offset\ntype: task\nstatus: done\nreleased_at: 2026-11-23T15:30:00+10:00\nmodified: 2026-11-23T05:30:00Z",
        "Mid body",
    );
    write_task_md(
        root,
        "t-latest.md",
        "id: t-latest\ntitle: Latest\ntype: task\nstatus: done\nreleased_at: 2026-11-23T06:00:00Z\nmodified: 2026-11-23T06:00:00Z",
        "Latest body",
    );
    write_task_md(
        root,
        "t-tie-b.md",
        "id: t-tie-b\ntitle: Tie B\ntype: task\nstatus: done\nreleased_at: 2026-11-23T05:00:00Z\nmodified: 2026-11-23T05:00:00Z",
        "Tie B body",
    );
    write_task_md(
        root,
        "t-tie-a.md",
        "id: t-tie-a\ntitle: Tie A\ntype: task\nstatus: done\nreleased_at: 2026-11-23T05:00:00Z\nmodified: 2026-11-23T05:00:00Z",
        "Tie A body",
    );
    // Legacy done tasks without released_at must always sort after released tasks,
    // even when their modified timestamp is newer.
    write_task_md(
        root,
        "t-legacy-newer.md",
        "id: t-legacy-newer\ntitle: Legacy Newer\ntype: task\nstatus: done\nmodified: 2026-11-23T09:00:00Z",
        "Legacy newer",
    );
    write_task_md(
        root,
        "t-legacy-older.md",
        "id: t-legacy-older\ntitle: Legacy Older\ntype: task\nstatus: done\nmodified: 2026-11-22T09:00:00Z",
        "Legacy older",
    );

    rebuild_from_disk(&server, root);

    let res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done",
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("list_tasks released_at_desc must succeed");

    assert_eq!(
        task_ids(&res),
        vec![
            "t-latest",
            "t-mid-offset",
            "t-tie-a",
            "t-tie-b",
            "t-early",
            "t-legacy-newer",
            "t-legacy-older",
        ]
    );

    // Truncation applies after sorting
    let top2 = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done",
                "sort": "released_at_desc",
                "limit": 2,
                "format": "json"
            }),
        )
        .expect("list_tasks with limit must succeed");
    assert_eq!(task_ids(&top2), vec!["t-latest", "t-mid-offset"]);
}

#[test]
fn test_list_tasks_released_since_filter_and_validation() {
    let (server, tmp) = setup_server();
    let root = tmp.path();

    write_task_md(
        root,
        "t-before.md",
        "id: t-before\ntitle: Before\ntype: task\nstatus: done\nreleased_at: 2026-11-23T04:59:59Z",
        "Body",
    );
    write_task_md(
        root,
        "t-exact.md",
        "id: t-exact\ntitle: Exact\ntype: task\nstatus: done\nreleased_at: 2026-11-23T05:00:00Z",
        "Body",
    );
    write_task_md(
        root,
        "t-after-tz.md",
        "id: t-after-tz\ntitle: After TZ\ntype: task\nstatus: partial\nreleased_at: 2026-11-23T15:01:00+10:00",
        "Body",
    );
    write_task_md(
        root,
        "t-unreleased.md",
        "id: t-unreleased\ntitle: Unreleased\ntype: task\nstatus: done\nmodified: 2026-11-23T08:00:00Z",
        "Body",
    );

    rebuild_from_disk(&server, root);

    // Exact inclusive timestamp bound; works without passing status or include_done
    let res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "released_since": "2026-11-23T05:00:00Z",
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("released_since query must succeed");
    assert_eq!(task_ids(&res), vec!["t-after-tz", "t-exact"]);

    // Explicit include_done: false hides terminal done tasks even when released_since is active
    let open_only = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "released_since": "2026-11-23T05:00:00Z",
                "include_done": false,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("released_since with include_done=false must succeed");
    assert_eq!(task_ids(&open_only), vec!["t-after-tz"]);

    // Date-only bound (UTC midnight inclusive)
    let date_only = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "released_since": "2026-11-23",
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("date-only released_since must succeed");
    assert_eq!(task_ids(&date_only), vec!["t-after-tz", "t-exact", "t-before"]);

    // Invalid inputs fail loud with INVALID_PARAMS
    for bad in [json!("not-a-date"), json!(""), json!(12345)] {
        let err = server
            .dispatch_tool_sync("list_tasks", &json!({ "released_since": bad }))
            .expect_err("invalid released_since must fail loud");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("released_since"),
            "error must name released_since: {}",
            err.message
        );
    }
}

#[test]
fn test_list_tasks_unhandled_filter_and_composed_feed_query() {
    let (server, tmp) = setup_server();
    let root = tmp.path();

    write_task_md(
        root,
        "t-done-unhandled.md",
        "id: t-done-unhandled\ntitle: Done unhandled\ntype: task\nstatus: done\nreleased_at: 2026-11-23T06:00:00Z\norigin_agent: coordinator-a",
        "Body",
    );
    write_task_md(
        root,
        "t-partial-unhandled.md",
        "id: t-partial-unhandled\ntitle: Partial unhandled\ntype: task\nstatus: partial\nreleased_at: 2026-11-23T05:30:00Z\nreason: Paused at seam",
        "Body",
    );
    write_task_md(
        root,
        "t-failed-unhandled.md",
        "id: t-failed-unhandled\ntitle: Failed unhandled\ntype: task\nstatus: failed\nreleased_at: 2026-11-23T05:00:00Z\nreason: Build error",
        "Body",
    );
    write_task_md(
        root,
        "t-cancelled-unhandled.md",
        "id: t-cancelled-unhandled\ntitle: Cancelled unhandled\ntype: task\nstatus: cancelled\nreleased_at: 2026-11-23T04:30:00Z\nreason: Obsolete",
        "Body",
    );
    write_task_md(
        root,
        "t-done-handled.md",
        "id: t-done-handled\ntitle: Done handled\ntype: task\nstatus: done\nreleased_at: 2026-11-23T06:15:00Z\nhandled_at: 2026-11-23T06:20:00Z\nhandled_by: coordinator-a",
        "Body",
    );
    // Legacy done task without released_at must NOT pollute unhandled feed
    write_task_md(
        root,
        "t-done-legacy.md",
        "id: t-done-legacy\ntitle: Legacy done\ntype: task\nstatus: done\nmodified: 2026-11-23T07:00:00Z",
        "Body",
    );
    // Open task without released_at must NOT pollute unhandled feed
    write_task_md(
        root,
        "t-open-ready.md",
        "id: t-open-ready\ntitle: Open ready\ntype: task\nstatus: ready",
        "Body",
    );

    rebuild_from_disk(&server, root);

    // Canonical pull-on-timer query
    let res = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,partial,failed,cancelled",
                "unhandled": true,
                "sort": "released_at_desc",
                "limit": 25,
                "format": "json"
            }),
        )
        .expect("canonical completion-feed query must succeed");
    assert_eq!(
        task_ids(&res),
        vec![
            "t-done-unhandled",
            "t-partial-unhandled",
            "t-failed-unhandled",
            "t-cancelled-unhandled",
        ]
    );

    // Without status/include_done, unhandled=true still surfaces released terminal tasks
    let bare = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "unhandled": true,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("bare unhandled=true query must succeed");
    assert_eq!(
        task_ids(&bare),
        vec![
            "t-done-unhandled",
            "t-partial-unhandled",
            "t-failed-unhandled",
            "t-cancelled-unhandled",
        ]
    );

    // unhandled: false is a no-op (returns handled and legacy done tasks too)
    let all_done = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done",
                "unhandled": false,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .expect("unhandled=false must succeed");
    assert_eq!(
        task_ids(&all_done),
        vec!["t-done-handled", "t-done-unhandled", "t-done-legacy"]
    );

    // Non-boolean unhandled fails loud
    let err = server
        .dispatch_tool_sync("list_tasks", &json!({ "unhandled": "true" }))
        .expect_err("non-bool unhandled must fail loud");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("unhandled"));

    // Marking t-done-unhandled as handled immediately removes it from unhandled: true
    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "t-done-unhandled",
                "handled_at": "2026-11-23T06:45:00Z",
                "handled_by": "coordinator-b"
            }),
        )
        .expect("marking handled via update_task must succeed");

    let after_handle = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,partial,failed,cancelled",
                "unhandled": true,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .unwrap();
    assert_eq!(
        task_ids(&after_handle),
        vec![
            "t-partial-unhandled",
            "t-failed-unhandled",
            "t-cancelled-unhandled",
        ]
    );

    // Clearing handled_at with null restores it to unhandled: true
    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "t-done-unhandled",
                "handled_at": null,
                "handled_by": null
            }),
        )
        .expect("clearing handled_at/handled_by must succeed");

    let after_clear = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done",
                "unhandled": true,
                "format": "json"
            }),
        )
        .unwrap();
    assert_eq!(task_ids(&after_clear), vec!["t-done-unhandled"]);

    // Non-string values for origin_agent, handled_at, handled_by are rejected
    for field in ["origin_agent", "handled_at", "handled_by"] {
        let err = server
            .dispatch_tool_sync(
                "update_task",
                &json!({
                    "id": "t-done-unhandled",
                    "updates": { field: 123 }
                }),
            )
            .expect_err("non-string value must be rejected");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains(field));
    }
}

#[test]
fn test_release_task_clears_handled_markers_and_preserves_origin_agent() {
    let (server, _tmp) = setup_server();

    // ── Case 1: partial task -> handled -> directly re-released as done ──
    server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "id": "task-partial-retry",
                "title": "Partial task to re-release",
                "slug": "partial-retry",
                "project": "proj-a",
                "origin_agent": "coordinator-a",
                "status": "in_progress"
            }),
        )
        .unwrap();

    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-partial-retry",
                "status": "partial",
                "summary": "Completed first phase",
                "reason": "Paused at milestone boundary"
            }),
        )
        .unwrap();

    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "task-partial-retry",
                "handled_at": "2026-11-23T05:00:00Z",
                "handled_by": "coordinator-a"
            }),
        )
        .unwrap();

    let before_re = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,partial,failed,cancelled",
                "unhandled": true,
                "format": "json"
            }),
        )
        .unwrap();
    assert!(!task_ids(&before_re).contains(&"task-partial-retry".to_string()));

    // Re-release partial task directly to done
    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-partial-retry",
                "status": "done",
                "summary": "Finished remaining phase",
                "pr_url": "https://github.com/example/repo/pull/101",
                "branch": "feat/phase-2",
                "release_summary": "Shipped phase 2 cleanly"
            }),
        )
        .expect("re-releasing partial task must succeed");

    let partial_doc = response_json(
        &server
            .dispatch_tool_sync("get_task", &json!({ "id": "task-partial-retry" }))
            .unwrap(),
    );
    let partial_fm = partial_doc["frontmatter"].as_object().unwrap();
    assert!(!partial_fm.contains_key("handled_at"));
    assert!(!partial_fm.contains_key("handled_by"));
    assert_eq!(
        partial_fm.get("origin_agent").and_then(|v| v.as_str()),
        Some("coordinator-a")
    );

    let after_partial_re = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,partial,failed,cancelled",
                "unhandled": true,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .unwrap();
    let tasks = response_json(&after_partial_re)["tasks"]
        .as_array()
        .unwrap()
        .clone();
    let row = tasks
        .iter()
        .find(|t| t["id"] == "task-partial-retry")
        .expect("re-released partial task must re-enter unhandled feed");
    assert_eq!(row["origin_agent"], "coordinator-a");
    assert!(row["handled_at"].is_null());
    assert!(row["handled_by"].is_null());
    assert_eq!(row["pr_url"], "https://github.com/example/repo/pull/101");
    assert_eq!(row["branch"], "feat/phase-2");
    assert_eq!(row["release_summary"], "Shipped phase 2 cleanly");

    // ── Case 2: failed task -> handled -> reopened via claim_task -> re-released as done ──
    server
        .dispatch_tool_sync(
            "create_task",
            &json!({
                "id": "task-failed-retry",
                "title": "Failed task to retry",
                "slug": "failed-retry",
                "project": "proj-b",
                "origin_agent": "coordinator-b",
                "status": "in_progress"
            }),
        )
        .unwrap();

    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-failed-retry",
                "status": "failed",
                "summary": "Initial run hit transient network failure",
                "reason": "Registry timeout"
            }),
        )
        .unwrap();

    server
        .dispatch_tool_sync(
            "update_task",
            &json!({
                "id": "task-failed-retry",
                "handled_at": "2026-11-23T05:10:00Z",
                "handled_by": "coordinator-b"
            }),
        )
        .unwrap();

    // Reopen failed task via claim_task, then re-release as done
    server
        .dispatch_tool_sync(
            "claim_task",
            &json!({ "id": "task-failed-retry", "assignee": "worker-2" }),
        )
        .expect("claiming reopened failed task must succeed");

    server
        .dispatch_tool_sync(
            "release_task",
            &json!({
                "id": "task-failed-retry",
                "status": "done",
                "summary": "Retry succeeded",
                "release_summary": "Green build on retry"
            }),
        )
        .expect("re-releasing reopened failed task must succeed");

    let failed_doc = response_json(
        &server
            .dispatch_tool_sync("get_task", &json!({ "id": "task-failed-retry" }))
            .unwrap(),
    );
    let failed_fm = failed_doc["frontmatter"].as_object().unwrap();
    assert!(!failed_fm.contains_key("handled_at"));
    assert!(!failed_fm.contains_key("handled_by"));
    assert_eq!(
        failed_fm.get("origin_agent").and_then(|v| v.as_str()),
        Some("coordinator-b")
    );

    let after_failed_re = server
        .dispatch_tool_sync(
            "list_tasks",
            &json!({
                "status": "done,partial,failed,cancelled",
                "unhandled": true,
                "sort": "released_at_desc",
                "format": "json"
            }),
        )
        .unwrap();
    assert!(
        task_ids(&after_failed_re).contains(&"task-failed-retry".to_string()),
        "re-released failed task must re-enter unhandled feed"
    );
}

#[test]
fn test_schemas_advertise_completion_feed_parameters_and_fields() {
    let tools = PkbSearchServer::get_published_tools();

    let list_tasks = tools
        .iter()
        .find(|t| t.name.as_ref() == "list_tasks")
        .expect("list_tasks published");
    let lt_schema = serde_json::to_value(&list_tasks.input_schema).unwrap();
    let lt_props = lt_schema["properties"].as_object().unwrap();
    assert!(lt_props.contains_key("released_since"));
    assert!(lt_props.contains_key("unhandled"));
    let sort_enum = lt_props["sort"]["enum"].as_array().unwrap();
    assert!(sort_enum.iter().any(|v| v.as_str() == Some("released_at_desc")));

    let create_task = tools
        .iter()
        .find(|t| t.name.as_ref() == "create_task")
        .expect("create_task published");
    let ct_schema = serde_json::to_value(&create_task.input_schema).unwrap();
    assert!(ct_schema["properties"]["origin_agent"].is_object());

    let update_task = tools
        .iter()
        .find(|t| t.name.as_ref() == "update_task")
        .expect("update_task published");
    let ut_schema = serde_json::to_value(&update_task.input_schema).unwrap();
    for key in ["origin_agent", "handled_at", "handled_by"] {
        assert!(
            ut_schema["properties"][key].is_object(),
            "update_task schema must advertise '{key}'"
        );
    }
}
