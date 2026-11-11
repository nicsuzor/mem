use super::*;
use futures_util::future::BoxFuture;
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use opentelemetry_sdk::trace::TracerProvider;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
struct TestSpanExporter {
    spans: Arc<Mutex<Vec<SpanData>>>,
}

impl SpanExporter for TestSpanExporter {
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        if let Ok(mut lock) = self.spans.lock() {
            lock.extend(batch);
        }
        Box::pin(std::future::ready(Ok(())))
    }
    fn shutdown(&mut self) {}
}

static EFFICIENCY_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn setup_test_server(root: &Path) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    write_test_polecat_yaml(root);

    std::fs::write(
        root.join("tasks/task-1.md"),
        "---\nid: task-1\ntype: task\ntitle: Tool Efficiency Task\nstatus: active\nassignee: ida\nparent: proj-root\ncontributes_to:\n  - to: target-1\n---\n# Tool Efficiency\nDetailed task body instructions here.\n",
    )
    .unwrap();

    std::fs::write(
        root.join("tasks/target-1.md"),
        "---\nid: target-1\ntype: target\ntitle: Target 1\nstatus: active\n---\n# Target\n",
    )
    .unwrap();

    std::fs::write(
        root.join("notes/doc-1.md"),
        "---\nid: doc-1\ntype: note\ntitle: Architecture Documentation\n---\n# Architecture\nComprehensive architectural overview and specifications.\n",
    )
    .unwrap();

    let docs = crate::pkb::scan_directory(root)
        .iter()
        .filter_map(|p| crate::pkb::parse_file_relative(p, root))
        .collect::<Vec<_>>();
    let graph = GraphStore::build(&docs, root);
    let mut store = VectorStore::new(3);
    for doc in &docs {
        store.insert_precomputed(doc, vec![doc.body.clone()], vec![vec![0.1; 3]]);
    }
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

#[test]
fn test_get_task_status_fastpath_returns_lightweight_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let server = setup_test_server(tmp.path());

    // Valid task
    let res = server
        .handle_get_task_status(&json!({ "id": "task-1" }))
        .expect("get_task_status should succeed");
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .expect("text content");

    // AC2: Payload must be under 150 bytes
    assert!(
        text.len() < 150,
        "get_task_status payload must be < 150 bytes, got {} bytes: {text}",
        text.len()
    );

    // Verify fields
    let val: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
    assert_eq!(val["id"], "task-1");
    assert_eq!(val["status"], "in_progress");
    assert_eq!(val["ready"], false);
    assert_eq!(val["blocked"], false);
    assert_eq!(val["assignee"], "ida");

    // Rejection on non-existent task
    let err = server
        .handle_get_task_status(&json!({ "id": "nonexistent-id" }))
        .unwrap_err();
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("Task not found"));

    // Rejection on missing ID
    let err_missing = server
        .handle_get_task_status(&json!({}))
        .unwrap_err();
    assert_eq!(err_missing.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(err_missing.message.contains("Missing required parameter: id"));
}

#[test]
fn test_get_task_compact_and_routes_inversion() {
    let tmp = tempfile::tempdir().unwrap();
    let server = setup_test_server(tmp.path());

    // By default, include_routes must be false (routes omitted/null)
    let res = server
        .handle_get_task(&json!({ "id": "task-1" }))
        .expect("get_task should succeed");
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .expect("text content");

    // AC1: Output must be compact (no multiline indentation spaces like "\n  ")
    assert!(
        !text.contains("\n  "),
        "get_task payload should be compact JSON without indentation, got:\n{text}"
    );

    let val: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
    assert!(
        val.get("routes").is_none(),
        "routes should not be populated by default when include_routes is false"
    );

    // When include_routes is true, routes key should be present
    let res_with_routes = server
        .handle_get_task(&json!({ "id": "task-1", "include_routes": true }))
        .expect("get_task with routes should succeed");
    let text_with_routes = res_with_routes
        .content
        .first()
        .and_then(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .expect("text content");
    let val_with_routes: serde_json::Value =
        serde_json::from_str(text_with_routes).expect("valid JSON");
    assert!(
        val_with_routes.get("routes").is_some() || val_with_routes.get("explanation").is_some(),
        "routes or explanation should be populated when include_routes=true, got {text_with_routes}"
    );
}

#[test]
fn test_get_document_compact_and_projections() {
    let tmp = tempfile::tempdir().unwrap();
    let server = setup_test_server(tmp.path());

    let res = server
        .handle_get_document(&json!({ "id": "doc-1", "format": "json" }))
        .expect("get_document should succeed");
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .expect("text content");

    assert!(
        !text.contains("\n  "),
        "get_document JSON output must be compact, got:\n{text}"
    );

    let val: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
    assert_eq!(val["id"], "doc-1");
    assert_eq!(val["title"], "Architecture Documentation");
}

#[test]
fn test_search_defaults_to_snippet_detail_and_compact() {
    let tmp = tempfile::tempdir().unwrap();
    let server = setup_test_server(tmp.path());

    let res = server
        .handle_pkb_search(&json!({ "query": "Architecture", "format": "json" }))
        .expect("search should succeed");
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .expect("text content");

    // Verify compact JSON
    assert!(
        !text.contains("\n  "),
        "search JSON output must be compact, got:\n{text}"
    );

    let val: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
    let results = val["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected search results");
    let first = &results[0];
    assert!(
        first.get("extract").is_some(),
        "search result should have extract"
    );
}

#[tokio::test]
async fn test_otel_spans_record_token_metrics() {
    let _lock = EFFICIENCY_TEST_MUTEX.lock().await;
    let exporter = TestSpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let server = setup_test_server(tmp.path());

    // Execute tool call via server dispatch pipeline
    let result = server
        .execute_tool_call("get_task_status", &json!({ "id": "task-1" }), None, None)
        .await
        .expect("tool call succeeded");
    assert!(result.is_error.is_none() || result.is_error == Some(false));

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans
        .iter()
        .find(|s| s.name == "mcp.get_task_status")
        .expect("mcp.get_task_status span recorded");

    // AC3: Verify span attributes for response_bytes and estimated tokens
    let response_bytes_attr = span
        .attributes
        .iter()
        .find(|kv| kv.key.as_str() == "mcp.response_bytes")
        .expect("mcp.response_bytes attribute present");
    let response_bytes = match response_bytes_attr.value {
        opentelemetry::Value::I64(n) => n,
        _ => panic!("unexpected type for mcp.response_bytes"),
    };
    assert!(response_bytes > 0 && response_bytes < 500);

    let tokens_attr = span
        .attributes
        .iter()
        .find(|kv| kv.key.as_str() == "mcp.estimated_prompt_tokens")
        .expect("mcp.estimated_prompt_tokens attribute present");
    let tokens = match tokens_attr.value {
        opentelemetry::Value::I64(n) => n,
        _ => panic!("unexpected type for mcp.estimated_prompt_tokens"),
    };
    assert_eq!(tokens, response_bytes / 4);
}
