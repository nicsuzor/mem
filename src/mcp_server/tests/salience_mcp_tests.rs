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

    // Score read through MCP (with metadata_only or read):
    // Note: get_salience reads without inflating access count
    let sal_res = server.handle_get_salience(&json!({"id": "doc-salience"})).unwrap();
    let sal_val = sal_res.structured_content.unwrap();
    let decayed_score = sal_val.get("salience").and_then(|v| v.as_f64()).expect("salience in get_salience");
    assert_eq!(decayed_score, 7.0);

    // Advance clock by another day: score decays linearly to 6.0
    clock.advance_days(1.0);
    let sal_res2 = server.handle_get_salience(&json!({"id": "doc-salience"})).unwrap();
    let sal_val2 = sal_res2.structured_content.unwrap();
    let decayed_score2 = sal_val2.get("salience").and_then(|v| v.as_f64()).unwrap();
    assert_eq!(decayed_score2, 6.0);
}
