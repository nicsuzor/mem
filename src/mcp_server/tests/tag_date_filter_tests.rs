use super::*;

// ── Tag filtering ──

pub(crate) fn make_doc_with_tags(path: &str, title: &str, id: &str, tags: &[&str]) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("title".to_string(), json!(title));
    fm.insert("type".to_string(), json!("task"));
    fm.insert("status".to_string(), json!("active"));
    fm.insert("id".to_string(), json!(id));
    if !tags.is_empty() {
        fm.insert("tags".to_string(), json!(tags));
    }
    PkbDocument {
        path: PathBuf::from(path),
        title: title.to_string(),
        body: String::new(),
        doc_type: Some("task".to_string()),
        status: Some("active".to_string()),
        consolidated: None,
        consolidated_at: None,
        modified: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        frontmatter: Some(serde_json::Value::Object(fm)),
        content_hash: "test_hash".to_string(),
        file_hash: "test_hash".to_string(),
    }
}

pub(crate) fn build_tag_test_server() -> PkbSearchServer {
    let docs = vec![
        make_doc_with_tags(
            "tasks/t-overwhelm.md",
            "Overwhelm task",
            "t-overwhelm",
            &["overwhelm", "rust"],
        ),
        make_doc_with_tags(
            "tasks/t-overwhelm-only.md",
            "Overwhelm only",
            "t-overwhelm-only",
            &["overwhelm"],
        ),
        make_doc_with_tags(
            "tasks/t-rust-only.md",
            "Rust only",
            "t-rust-only",
            &["rust"],
        ),
        make_doc_with_tags("tasks/t-untagged.md", "Untagged task", "t-untagged", &[]),
        make_doc_with_tags("tasks/t-other.md", "Other task", "t-other", &["misc"]),
    ];
    let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb-tags"));
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        PathBuf::from("/tmp/test-pkb-tags"),
        PathBuf::from("/tmp/test-pkb-tags/db"),
        Arc::new(RwLock::new(graph)),
    )
}

#[test]
fn test_list_tasks_single_tag_filter() {
    let server = build_tag_test_server();
    let result = server
        .handle_list_tasks(&json!({"tags": ["overwhelm"], "format": "json"}))
        .unwrap();
    let ids = extract_task_ids(&result);
    assert!(ids.contains(&"t-overwhelm".to_string()));
    assert!(ids.contains(&"t-overwhelm-only".to_string()));
    assert!(!ids.contains(&"t-rust-only".to_string()));
    assert!(!ids.contains(&"t-untagged".to_string()));
    assert!(!ids.contains(&"t-other".to_string()));
}

#[test]
fn test_list_tasks_multi_tag_and_filter() {
    let server = build_tag_test_server();
    let result = server
        .handle_list_tasks(&json!({"tags": ["overwhelm", "rust"], "format": "json"}))
        .unwrap();
    let ids = extract_task_ids(&result);
    assert_eq!(ids, vec!["t-overwhelm".to_string()]);
}

#[test]
fn test_list_tasks_tag_no_match_returns_empty() {
    let server = build_tag_test_server();
    let result = server
        .handle_list_tasks(&json!({"tags": ["does-not-exist"], "format": "json"}))
        .unwrap();
    let ids = extract_task_ids(&result);
    assert!(ids.is_empty());
}

#[test]
fn test_list_tasks_tag_filter_excludes_untagged() {
    let server = build_tag_test_server();
    let result = server
        .handle_list_tasks(&json!({"tags": ["overwhelm"], "format": "json"}))
        .unwrap();
    let ids = extract_task_ids(&result);
    assert!(!ids.contains(&"t-untagged".to_string()));
}

#[test]
fn test_list_tasks_tag_filter_case_insensitive() {
    let server = build_tag_test_server();
    let result = server
        .handle_list_tasks(&json!({"tags": ["OVERWHELM"], "format": "json"}))
        .unwrap();
    let ids = extract_task_ids(&result);
    assert!(ids.contains(&"t-overwhelm".to_string()));
    assert!(ids.contains(&"t-overwhelm-only".to_string()));
}

#[test]
fn test_list_tasks_schema_includes_tags_parameter() {
    let tools = PkbSearchServer::get_all_tools();
    let list_tasks_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "list_tasks")
        .expect("list_tasks tool should exist");
    let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
    assert!(
        schema.contains("\"tags\""),
        "list_tasks schema should include 'tags' parameter, got: {}",
        schema
    );
}

// ── focus_score_gte filter ──

#[test]
fn test_list_tasks_focus_score_gte_filters_low_score_tasks() {
    let server = build_test_server();
    let unfiltered = server
        .handle_list_tasks(&json!({"status": "ready", "format": "json"}))
        .unwrap();
    let unfiltered_text = unfiltered
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    let unfiltered_val: serde_json::Value =
        serde_json::from_str(&unfiltered_text).expect("ready listing should be JSON");
    let unfiltered_tasks = unfiltered_val
        .get("tasks")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !unfiltered_tasks.is_empty(),
        "fixture should have ready tasks"
    );

    let filtered = server
        .handle_list_tasks(&json!({
            "status": "ready",
            "focus_score_gte": 1,
            "format": "json",
        }))
        .unwrap();
    let filtered_text = filtered
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    if let Ok(filtered_val) = serde_json::from_str::<serde_json::Value>(&filtered_text) {
        if let Some(tasks) = filtered_val.get("tasks").and_then(|t| t.as_array()) {
            for t in tasks {
                let s = t
                    .get("focus_score")
                    .and_then(|v| v.as_i64())
                    .expect("focus_score should be present");
                assert!(
                    s >= 1,
                    "focus_score_gte=1 should only return tasks with focus_score >= 1, got {}",
                    s
                );
            }
            assert!(tasks.len() <= unfiltered_tasks.len());
        }
    }

    let none = server
        .handle_list_tasks(&json!({
            "status": "ready",
            "focus_score_gte": 1_000_000_000_i64,
            "format": "json",
        }))
        .unwrap();
    let none_text = none
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    let is_empty = none_text.contains("No tasks found")
        || none_text.contains("No ready tasks")
        || none_text.contains("\"tasks\":[]")
        || none_text.contains("\"tasks\": []");
    assert!(
        is_empty,
        "focus_score_gte=1e9 should return no tasks, got: {}",
        none_text
    );
}

#[test]
fn test_list_tasks_schema_includes_focus_score_gte_parameter() {
    let tools = PkbSearchServer::get_all_tools();
    let list_tasks_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "list_tasks")
        .expect("list_tasks tool should exist");
    let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
    assert!(
        schema.contains("\"focus_score_gte\""),
        "list_tasks schema should include 'focus_score_gte' parameter, got: {}",
        schema
    );
}

// ── AC6: tool schema includes project parameter ──

#[test]
fn test_list_tasks_schema_includes_project_parameter() {
    let tools = PkbSearchServer::get_all_tools();
    let list_tasks_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "list_tasks")
        .expect("list_tasks tool should exist");
    let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
    assert!(
        schema.contains("\"project\""),
        "list_tasks schema should include 'project' parameter, got: {}",
        schema
    );
}

// ── since/before date filters on list_tasks (mem-ef5e74ac) ──────────────

pub(crate) fn make_doc_with_modified_date(id: &str, modified: Option<&str>) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("title".to_string(), json!(id));
    fm.insert("type".to_string(), json!("task"));
    fm.insert("status".to_string(), json!("active"));
    fm.insert("id".to_string(), json!(id));
    if let Some(m) = modified {
        fm.insert("modified".to_string(), json!(m));
    }
    PkbDocument {
        path: PathBuf::from(format!("tasks/{id}.md")),
        title: id.to_string(),
        body: String::new(),
        doc_type: Some("task".to_string()),
        status: Some("active".to_string()),
        consolidated: None,
        consolidated_at: None,
        modified: modified.map(String::from),
        tags: vec![],
        frontmatter: Some(serde_json::Value::Object(fm)),
        content_hash: "test_hash".to_string(),
        file_hash: "test_hash".to_string(),
    }
}

pub(crate) fn build_dated_task_server() -> PkbSearchServer {
    let docs = vec![
        make_doc_with_modified_date("task-old", Some("2019-06-01T00:00:00Z")),
        make_doc_with_modified_date("task-mid", Some("2023-03-15T12:00:00Z")),
        make_doc_with_modified_date("task-new", Some("2026-06-01T09:00:00Z")),
        make_doc_with_modified_date("task-nodated", None),
    ];
    let graph = GraphStore::build(&docs, Path::new("/tmp/test-dated-pkb"));
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mem-dated-pkb-{}-{}", std::process::id(), seq));
    let _ = std::fs::create_dir_all(&root);
    let db = root.join("db");
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root,
        db,
        Arc::new(RwLock::new(graph)),
    )
}

#[test]
fn test_list_tasks_before_ancient_date_returns_zero() {
    // AC: before=2020-01-01 returns zero rows (only task-old is pre-2020,
    // but its modified is 2019-06-01 which is <= 2020-01-01, so it passes).
    // More strictly: before=2018-01-01 must return zero (all tasks are after 2018).
    let server = build_dated_task_server();
    let result = server
        .handle_list_tasks(&json!({"before": "2018-01-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks = extract_task_objects(&result);
    assert!(
        tasks.is_empty(),
        "before=2018-01-01 must return zero rows; got {} tasks",
        tasks.len()
    );
}

#[test]
fn test_list_tasks_since_future_returns_zero() {
    // AC: since=2030-01-01 returns zero rows.
    let server = build_dated_task_server();
    let result = server
        .handle_list_tasks(&json!({"since": "2030-01-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks = extract_task_objects(&result);
    assert!(
        tasks.is_empty(),
        "since=2030-01-01 must return zero rows; got {} tasks",
        tasks.len()
    );
}

#[test]
fn test_list_tasks_since_before_reduce_monotonically() {
    // AC: narrowing since/before yields strictly fewer results.
    let server = build_dated_task_server();
    let wide = extract_task_objects(
        &server
            .handle_list_tasks(
                &json!({"since": "2000-01-01", "include_done": true, "format": "json"}),
            )
            .unwrap(),
    );
    let narrow = extract_task_objects(
        &server
            .handle_list_tasks(
                &json!({"since": "2025-01-01", "include_done": true, "format": "json"}),
            )
            .unwrap(),
    );
    assert!(
        narrow.len() < wide.len(),
        "narrowing since must reduce results: wide={} narrow={}",
        wide.len(),
        narrow.len()
    );
}

#[test]
fn test_list_tasks_no_modified_excluded_when_filter_active() {
    // Tasks without a modified date must be excluded when any date filter is set.
    let server = build_dated_task_server();
    let result = server
        .handle_list_tasks(&json!({"since": "2000-01-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks = extract_task_objects(&result);
    let ids: Vec<_> = tasks
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(
        !ids.contains(&"task-nodated"),
        "task with no modified date must be excluded by date filter; ids present: {ids:?}"
    );
}

#[test]
fn test_list_tasks_repro_wide_date_range_not_identical() {
    // Regression: before the fix, since=2000 vs since=2030 returned the same rows.
    // After the fix, since=2030-01-01 returns zero and since=2000-01-01 returns some.
    let server = build_dated_task_server();
    let since_past = extract_task_objects(
        &server
            .handle_list_tasks(
                &json!({"since": "2000-01-01", "include_done": true, "format": "json"}),
            )
            .unwrap(),
    );
    let since_future = extract_task_objects(
        &server
            .handle_list_tasks(
                &json!({"since": "2030-01-01", "include_done": true, "format": "json"}),
            )
            .unwrap(),
    );
    assert!(
        since_past.len() != since_future.len()
            || (since_past.is_empty() && since_future.is_empty()),
        "since=2000 and since=2030 must not return identical non-empty row counts; \
             both returned {} rows (date filter is a no-op)",
        since_past.len()
    );
    assert!(
        since_future.is_empty(),
        "since=2030-01-01 must return zero rows; got {}",
        since_future.len()
    );
    assert!(
        !since_past.is_empty(),
        "since=2000-01-01 must return some rows (3 dated tasks exist); got 0"
    );
}

#[test]
fn test_list_tasks_schema_includes_since_before() {
    let tools = PkbSearchServer::get_all_tools();
    let schema = serde_json::to_string(
        &tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist")
            .input_schema,
    )
    .unwrap();
    assert!(
        schema.contains("\"since\""),
        "list_tasks schema must include 'since'; got: {schema}"
    );
    assert!(
        schema.contains("\"before\""),
        "list_tasks schema must include 'before'; got: {schema}"
    );
}

#[test]
fn test_list_tasks_filters_on_frontmatter_modified_not_disk_mtime() {
    // AC1: A task whose frontmatter `modified:` (2026-01-15) differs from its
    // on-disk filesystem mtime (today, e.g. 2026-08-22).
    // A query with date strictly between the two (2026-06-01):
    // - `since="2026-06-01"` must NOT match (frontmatter 2026-01-15 < 2026-06-01).
    //   Buggy code reading disk mtime (2026-08-22) would erroneously match.
    // - `before="2026-06-01"` MUST match (frontmatter 2026-01-15 <= 2026-06-01).
    //   Buggy code reading disk mtime (2026-08-22) would erroneously exclude it.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    let task_path = tasks_dir.join("task-stale.md");
    std::fs::write(
        &task_path,
        "---\n\
             id: task-stale\n\
             title: Stale Task\n\
             type: task\n\
             status: ready\n\
             modified: 2026-01-15T00:00:00Z\n\
             ---\n\n\
             # Stale Task Body\n",
    )
    .unwrap();

    let docs: Vec<PkbDocument> = crate::pkb::scan_directory_all(root)
        .into_iter()
        .filter_map(|p| crate::pkb::parse_file_relative(&p, root))
        .collect();

    let graph = GraphStore::build(&docs, root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db,
        Arc::new(RwLock::new(graph)),
    );

    // 1. `since="2026-06-01"` must return ZERO tasks.
    let result_since = server
        .handle_list_tasks(&json!({"since": "2026-06-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks_since = extract_task_objects(&result_since);
    assert!(
        tasks_since.is_empty(),
        "since=2026-06-01 must return 0 tasks for frontmatter modified 2026-01-15; got: {:?}",
        tasks_since
    );

    // 2. `before="2026-06-01"` must return task-stale with its frontmatter modified date.
    let result_before = server
        .handle_list_tasks(&json!({"before": "2026-06-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks_before = extract_task_objects(&result_before);
    assert_eq!(
        tasks_before.len(),
        1,
        "before=2026-06-01 must match task-stale (frontmatter modified 2026-01-15); got: {:?}",
        tasks_before
    );
    let returned_modified = tasks_before[0]
        .get("modified")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        returned_modified, "2026-01-15T00:00:00Z",
        "returned modified timestamp must match frontmatter modified date"
    );
}

#[test]
fn test_contract_holds_differing_vintage_sampling() {
    // AC3: 3 sampled node IDs of differing vintage.
    // Verifies list_tasks, task_search, and search date filtering and modified field preservation.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    let v1_mod = "2024-03-10T12:00:00Z";
    let v2_mod = "2025-07-20T08:30:00Z";
    let v3_mod = "2026-02-14T15:45:00Z";

    std::fs::write(
            tasks_dir.join("task-v1.md"),
            format!("---\nid: task-v1\ntitle: Vintage 2024 Task\ntype: task\nstatus: ready\nmodified: {v1_mod}\n---\n\nBody 2024 vintage.\n"),
        ).unwrap();

    std::fs::write(
            tasks_dir.join("task-v2.md"),
            format!("---\nid: task-v2\ntitle: Vintage 2025 Task\ntype: task\nstatus: ready\nmodified: {v2_mod}\n---\n\nBody 2025 vintage.\n"),
        ).unwrap();

    std::fs::write(
            tasks_dir.join("task-v3.md"),
            format!("---\nid: task-v3\ntitle: Vintage 2026 Task\ntype: task\nstatus: ready\nmodified: {v3_mod}\n---\n\nBody 2026 vintage.\n"),
        ).unwrap();

    let docs: Vec<PkbDocument> = crate::pkb::scan_directory_all(root)
        .into_iter()
        .filter_map(|p| crate::pkb::parse_file_relative(&p, root))
        .collect();

    let graph = GraphStore::build(&docs, root);
    let mut store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    for doc in &docs {
        let emb = vec![vec![1.0, 0.0, 0.0]];
        let chunks = vec![doc.body.clone()];
        store.insert_precomputed(doc, chunks, emb);
    }
    let db = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db,
        Arc::new(RwLock::new(graph)),
    );

    // 1. list_tasks side-by-side verification
    let res_all = server
        .handle_list_tasks(&json!({"include_done": true, "format": "json"}))
        .unwrap();
    let tasks = extract_task_objects(&res_all);
    assert_eq!(tasks.len(), 3);

    let t1 = tasks
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-v1"))
        .unwrap();
    assert_eq!(t1.get("modified").and_then(|v| v.as_str()), Some(v1_mod));

    let t2 = tasks
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-v2"))
        .unwrap();
    assert_eq!(t2.get("modified").and_then(|v| v.as_str()), Some(v2_mod));

    let t3 = tasks
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-v3"))
        .unwrap();
    assert_eq!(t3.get("modified").and_then(|v| v.as_str()), Some(v3_mod));

    // 2. list_tasks(since=D) returns node iff frontmatter modified >= D
    // since="2025-01-01" -> matches task-v2 (2025-07-20) and task-v3 (2026-02-14), excludes task-v1 (2024-03-10)
    let res_since_2025 = server
        .handle_list_tasks(&json!({"since": "2025-01-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks_2025 = extract_task_objects(&res_since_2025);
    let ids_2025: Vec<&str> = tasks_2025
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(ids_2025.len(), 2);
    assert!(ids_2025.contains(&"task-v2"));
    assert!(ids_2025.contains(&"task-v3"));
    assert!(!ids_2025.contains(&"task-v1"));

    // since="2026-01-01" -> matches only task-v3
    let res_since_2026 = server
        .handle_list_tasks(&json!({"since": "2026-01-01", "include_done": true, "format": "json"}))
        .unwrap();
    let tasks_2026 = extract_task_objects(&res_since_2026);
    let ids_2026: Vec<&str> = tasks_2026
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(ids_2026, vec!["task-v3"]);

    // 3. search with since / before date filters
    let s_res = server
        .handle_pkb_search(
            &json!({"query": "vintage", "since": "2025-01-01", "before": "2025-12-31"}),
        )
        .unwrap();
    let s_text = format!("{:?}", s_res);
    assert!(s_text.contains("task-v2"));
    assert!(!s_text.contains("task-v1"));
    assert!(!s_text.contains("task-v3"));

    // 4. task_search with since / before date filters
    let ts_res = server
        .handle_task_search(
            &json!({"query": "vintage", "since": "2026-01-01", "include_done": true}),
        )
        .unwrap();
    let ts_text = format!("{:?}", ts_res);
    assert!(ts_text.contains("task-v3"));
    assert!(!ts_text.contains("task-v1"));
    assert!(!ts_text.contains("task-v2"));
}

// ── Issue #670: Date filters support RFC 3339 timestamps and fail loud on invalid inputs ──

#[test]
fn test_list_tasks_since_before_rfc3339_timestamp() {
    // Regression for Issue #670:
    // Fixture tasks:
    // - task-old: 2019-06-01T00:00:00Z
    // - task-mid: 2023-03-15T12:00:00Z
    // - task-new: 2026-06-01T09:00:00Z
    // - task-nodated: None
    let server = build_dated_task_server();

    // 1. since on day of modification with 00:00:00Z must include the task (was 0 due to prefix bug)
    let res_since_midnight = server
        .handle_list_tasks(
            &json!({"since": "2026-06-01T00:00:00Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let tasks = extract_task_objects(&res_since_midnight);
    let ids: Vec<String> = tasks
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert_eq!(
        ids,
        vec!["task-new".to_string()],
        "since=2026-06-01T00:00:00Z must return task-new"
    );

    // 2. since at exact timestamp must include the task
    let res_since_exact = server
        .handle_list_tasks(
            &json!({"since": "2026-06-01T09:00:00Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let ids: Vec<String> = extract_task_objects(&res_since_exact)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert_eq!(
        ids,
        vec!["task-new".to_string()],
        "since=2026-06-01T09:00:00Z must return task-new"
    );

    // 3. since 1 second after timestamp must exclude the task
    let res_since_after = server
        .handle_list_tasks(
            &json!({"since": "2026-06-01T09:00:01Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let ids: Vec<String> = extract_task_objects(&res_since_after)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(
        ids.is_empty(),
        "since=2026-06-01T09:00:01Z must return zero tasks"
    );

    // 4. since intermediate timestamp
    let res_mid = server
        .handle_list_tasks(
            &json!({"since": "2023-03-15T12:00:00Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let mut ids: Vec<String> = extract_task_objects(&res_mid)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["task-mid".to_string(), "task-new".to_string()]);

    let res_mid_plus1 = server
        .handle_list_tasks(
            &json!({"since": "2023-03-15T12:00:01Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let ids: Vec<String> = extract_task_objects(&res_mid_plus1)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert_eq!(ids, vec!["task-new".to_string()]);

    // 5. before at 00:00:00Z on day of modification must exclude tasks modified later that day
    // (Previously kept task-new because "2026-06-01" < "2026-06-01T00:00:00Z")
    let res_before_midnight = server
        .handle_list_tasks(
            &json!({"before": "2026-06-01T00:00:00Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let mut ids: Vec<String> = extract_task_objects(&res_before_midnight)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["task-mid".to_string(), "task-old".to_string()],
        "before=2026-06-01T00:00:00Z must exclude task-new (modified at 09:00:00Z)"
    );

    // 6. before at exact timestamp includes it
    let res_before_exact = server
        .handle_list_tasks(
            &json!({"before": "2026-06-01T09:00:00Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let mut ids: Vec<String> = extract_task_objects(&res_before_exact)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "task-mid".to_string(),
            "task-new".to_string(),
            "task-old".to_string()
        ]
    );

    // 7. before 1 second prior excludes it
    let res_before_prior = server
        .handle_list_tasks(
            &json!({"before": "2026-06-01T08:59:59Z", "include_done": true, "format": "json"}),
        )
        .unwrap();
    let mut ids: Vec<String> = extract_task_objects(&res_before_prior)
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["task-mid".to_string(), "task-old".to_string()]);
}

#[test]
fn test_date_filters_fail_loud_on_invalid_inputs() {
    let server = build_dated_task_server();

    // list_tasks invalid inputs
    let err1 = server
        .handle_list_tasks(&json!({"since": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err1.code, ErrorCode::INVALID_PARAMS);
    assert!(err1.message.contains("Invalid date format for 'since'"));

    let err2 = server
        .handle_list_tasks(&json!({"before": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err2.code, ErrorCode::INVALID_PARAMS);
    assert!(err2.message.contains("Invalid date format for 'before'"));

    let err3 = server.handle_list_tasks(&json!({"since": ""})).unwrap_err();
    assert_eq!(err3.code, ErrorCode::INVALID_PARAMS);

    let err4 = server
        .handle_list_tasks(&json!({"since": "2026-13-45"}))
        .unwrap_err();
    assert_eq!(err4.code, ErrorCode::INVALID_PARAMS);

    let err5 = server
        .handle_list_tasks(&json!({"since": 12345}))
        .unwrap_err();
    assert_eq!(err5.code, ErrorCode::INVALID_PARAMS);

    // search invalid inputs
    let err_s1 = server
        .handle_pkb_search(&json!({"query": "test", "since": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err_s1.code, ErrorCode::INVALID_PARAMS);
    assert!(err_s1.message.contains("Invalid date format for 'since'"));

    let err_s2 = server
        .handle_pkb_search(&json!({"query": "test", "before": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err_s2.code, ErrorCode::INVALID_PARAMS);
    assert!(err_s2.message.contains("Invalid date format for 'before'"));

    // task_search invalid inputs
    let err_ts1 = server
        .handle_task_search(&json!({"query": "test", "since": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err_ts1.code, ErrorCode::INVALID_PARAMS);
    assert!(err_ts1.message.contains("Invalid date format for 'since'"));

    let err_ts2 = server
        .handle_task_search(&json!({"query": "test", "before": "not-a-date"}))
        .unwrap_err();
    assert_eq!(err_ts2.code, ErrorCode::INVALID_PARAMS);
    assert!(err_ts2.message.contains("Invalid date format for 'before'"));
}

// ── Parent referential integrity (task-89b2af87) ───────────────────────

#[test]
fn test_create_task_rejects_nonexistent_parent() {
    let server = build_test_server();
    let err = server
        .handle_create_task(&json!({
            "title": "test",
            "project": "aops",
            "parent": "task-does-not-exist"
        }))
        .expect_err("expected rejection on missing parent");
    let msg = err.message.to_string();
    assert!(
        msg.contains("task-does-not-exist") && msg.to_lowercase().contains("not found"),
        "error should mention the missing ID and 'not found'; got: {msg}"
    );
    assert!(
        matches!(err.code, ErrorCode::INVALID_PARAMS),
        "should be INVALID_PARAMS, got: {:?}",
        err.code
    );
}

#[test]
fn test_create_task_rejects_nonexistent_parent_even_with_explicit_id() {
    // Verify the validation runs regardless of whether a custom id was passed.
    let server = build_test_server();
    let err = server
        .handle_create_task(&json!({
            "title": "test",
            "project": "aops",
            "id": "task-89b2af87-child",
            "parent": "task-does-not-exist"
        }))
        .expect_err("expected rejection on missing parent");
    assert!(
        matches!(err.code, ErrorCode::INVALID_PARAMS),
        "should be INVALID_PARAMS"
    );
}

#[test]
fn test_update_task_rejects_reparent_to_nonexistent() {
    let server = build_test_server();
    // task-a1 exists in the seeded graph; try to reparent it to a missing node.
    let err = server
        .handle_update_task(&json!({
            "id": "task-a1",
            "parent": "task-does-not-exist"
        }))
        .expect_err("expected rejection on reparent to missing node");
    let msg = err.message.to_string();
    assert!(
        msg.contains("task-does-not-exist"),
        "error should name the missing ID; got: {msg}"
    );
    assert!(
        matches!(err.code, ErrorCode::INVALID_PARAMS),
        "should be INVALID_PARAMS"
    );
}

#[test]
fn test_update_task_allows_clearing_parent_with_null() {
    // Setting parent to "" (or null) should not trigger the resolver — clearing
    // the parent edge is a legitimate operation distinct from reparenting.
    // The update will fail downstream because the test server's pkb_root is a
    // bogus path, but it must NOT fail on parent-validation grounds.
    let server = build_test_server();
    let err = server
        .handle_update_task(&json!({
            "id": "task-a1",
            "parent": ""
        }))
        .expect_err("test server has no real disk file");
    // The failure should NOT be the parent-validation error.
    assert!(
        !err.message.contains("not found in PKB"),
        "empty parent should bypass referential check; got: {}",
        err.message
    );
}
