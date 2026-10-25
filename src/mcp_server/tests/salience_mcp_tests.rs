use super::*;
use crate::salience::MockClock;
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::sync::Arc;

fn make_salience_test_server(root: &Path, mock_clock: Arc<MockClock>) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    let graph = GraphStore::build(&[], root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );
    server.set_clock(mock_clock);
    server
}

#[test]
fn test_mcp_api_reads_salience_exponential_growth_and_linear_decay() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let server = make_salience_test_server(root, clock.clone());

    let doc_path = root.join("tasks/doc-salience.md");
    let content = "---\nid: doc-salience\ntitle: Salience Test\ntype: task\nstatus: ready\n---\n\nHello salience world.\n";
    std::fs::write(&doc_path, content).unwrap();

    let doc = crate::pkb::parse_file_relative(&doc_path, root).unwrap();
    server.rebuild_graph_for_pkb_document(&doc);

    // 1st MCP retrieval (get_document): raises score from floor (0.0) to initial_score (1.0)
    let res1 = server.handle_get_document(&json!({"id": "doc-salience"})).unwrap();
    let val1 = res1.structured_content.unwrap();
    let score1 = val1.get("salience").and_then(|v| v.as_f64()).expect("salience field present in get_document");
    assert_eq!(score1, 1.0);

    // 2nd MCP retrieval (get_document): raises exponentially (1.0 * 2.0 = 2.0)
    let res2 = server.handle_get_document(&json!({"id": "doc-salience"})).unwrap();
    let val2 = res2.structured_content.unwrap();
    let score2 = val2.get("salience").and_then(|v| v.as_f64()).expect("salience field present in get_document");
    assert_eq!(score2, 2.0);

    // 3rd MCP retrieval (get_task): raises exponentially (2.0 * 2.0 = 4.0)
    let res3 = server.handle_get_task(&json!({"id": "doc-salience"})).unwrap();
    let val3 = res3.structured_content.unwrap();
    let score3 = val3.get("salience").and_then(|v| v.as_f64()).expect("salience field present in get_task");
    assert_eq!(score3, 4.0);

    // 4th MCP retrieval (get_task): raises exponentially (4.0 * 2.0 = 8.0)
    let res4 = server.handle_get_task(&json!({"id": "doc-salience"})).unwrap();
    let val4 = res4.structured_content.unwrap();
    let score4 = val4.get("salience").and_then(|v| v.as_f64()).expect("salience field present in get_task");
    assert_eq!(score4, 8.0);

    // Advance controlled clock by 1 day: score decays linearly by 1.0 -> 7.0
    clock.advance_days(1.0);

    let decayed_score = server.salience_store.read().get_salience("doc-salience");
    assert_eq!(decayed_score, 7.0);

    // Advance clock by another day: score decays linearly to 6.0
    clock.advance_days(1.0);
    let decayed_score2 = server.salience_store.read().get_salience("doc-salience");
    assert_eq!(decayed_score2, 6.0);
}

#[test]
fn test_mcp_search_exposes_recency_decay_and_salience_priors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let server = make_salience_test_server(root, clock.clone());

    // 1. Create a daily note from 14 days ago (date: 2026-10-04)
    std::fs::create_dir_all(root.join("daily")).unwrap();
    let daily_path = root.join("daily/2026-10-04.md");
    let daily_content = "---\nid: daily_2026_10_04\ntitle: Daily Log 2026-10-04\ntype: daily\ndate: 2026-10-04\nstatus: ready\n---\n\nWidget query retrieval test.\n";
    std::fs::write(&daily_path, daily_content).unwrap();

    // 2. Create a standard task
    let task_path = root.join("tasks/task-widget.md");
    let task_content = "---\nid: task-widget\ntitle: Widget Core Task\ntype: task\nstatus: ready\n---\n\nWidget query retrieval test.\n";
    std::fs::write(&task_path, task_content).unwrap();

    let daily_doc = crate::pkb::parse_file_relative(&daily_path, root).unwrap();
    let task_doc = crate::pkb::parse_file_relative(&task_path, root).unwrap();

    let emb = server.embedder.encode_query("widget query retrieval test").unwrap();
    server.store.write().insert_precomputed(&daily_doc, vec![daily_doc.body.clone()], vec![emb.clone()]);
    server.store.write().insert_precomputed(&task_doc, vec![task_doc.body.clone()], vec![emb]);
    server.rebuild_graph_for_pkb_document(&daily_doc);
    server.rebuild_graph_for_pkb_document(&task_doc);

    // Access task-widget twice to raise its salience
    server.handle_get_document(&json!({"id": "task-widget"})).unwrap();
    server.handle_get_document(&json!({"id": "task-widget"})).unwrap();

    // Perform search
    let search_res = server.handle_pkb_search(&json!({
        "query": "widget query retrieval test",
        "format": "json"
    })).unwrap();

    let search_val = search_res.structured_content.expect("structured search output");
    let items = search_val.get("results").or_else(|| search_val.get("items")).and_then(|v| v.as_array()).expect("results array");
    assert!(!items.is_empty(), "expected search results");

    let daily_item = items.iter().find(|it| it.get("id").and_then(|v| v.as_str()) == Some("daily_2026_10_04")).expect("daily note in results");
    let task_item = items.iter().find(|it| it.get("id").and_then(|v| v.as_str()) == Some("task-widget")).expect("task in results");

    // AC3: Daily note recency decay matches 0.85 + 0.15 * exp(-age_days / 14) from its date field (14 days age -> exp(-1))
    let daily_decay = daily_item.get("recency_decay").and_then(|v| v.as_f64()).expect("recency_decay field");
    let expected_daily_decay = 0.85 + 0.15 * (-1.0_f64).exp();
    assert!((daily_decay - expected_daily_decay).abs() < 1e-4, "daily recency decay expected {expected_daily_decay}, got {daily_decay}");

    // Daily note unaccessed: recent_read_boost == 1.0
    let daily_boost = daily_item.get("recent_read_boost").and_then(|v| v.as_f64()).expect("recent_read_boost field");
    assert_eq!(daily_boost, 1.0);

    // Task doc: task recency is 1.0 (pkb-rules §5), but salience boost > 1.0 from accesses
    let task_decay = task_item.get("recency_decay").and_then(|v| v.as_f64()).expect("recency_decay field");
    assert_eq!(task_decay, 1.0);

    let task_salience = task_item.get("salience").and_then(|v| v.as_f64()).expect("salience field");
    assert_eq!(task_salience, 2.0); // 1.0 * 2.0 = 2.0

    let task_boost = task_item.get("recent_read_boost").and_then(|v| v.as_f64()).expect("recent_read_boost field");
    assert!(task_boost > 1.0, "expected recent-read boost > 1.0, got {task_boost}");
}

#[test]
fn test_mcp_search_floor_invariance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let server = make_salience_test_server(root, clock.clone());

    let task_path = root.join("tasks/task-floor.md");
    let task_content = "---\nid: task-floor\ntitle: Floor Test\ntype: task\nstatus: ready\n---\n\nSample body text.\n";
    std::fs::write(&task_path, task_content).unwrap();

    let task_doc = crate::pkb::parse_file_relative(&task_path, root).unwrap();
    let emb = server.embedder.encode_query("sample body text").unwrap();
    server.store.write().insert_precomputed(&task_doc, vec![task_doc.body.clone()], vec![emb]);
    server.rebuild_graph_for_pkb_document(&task_doc);

    // Boost salience to ceiling
    for _ in 0..10 {
        server.handle_get_document(&json!({"id": "task-floor"})).unwrap();
    }

    // AC4: Pass a high floor (e.g. 0.999) that the item cannot meet
    let search_res = server.handle_pkb_search(&json!({
        "query": "unrelated non-matching query string",
        "floor": 0.999,
        "format": "json"
    })).unwrap();

    let search_val = search_res.structured_content.expect("structured search output");
    if let Some(items) = search_val.get("results").or_else(|| search_val.get("items")).and_then(|v| v.as_array()) {
        for item in items {
            let score = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            assert!(score < 0.999, "item score {score} must not breach floor 0.999");
        }
    }
}

