use super::*;
use crate::mcp_server::tests::otel_spans_tests::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;

static CALLER_SESSION_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn make_meta(map: serde_json::Map<String, serde_json::Value>) -> rmcp::model::RequestMetaObject {
    rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map))
}

#[tokio::test]
async fn test_session_source_header() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let req = http::Request::builder()
        .header("x-session-id", "header-sess-123")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    let res = server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts), None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(
        attrs.get("session.id").map(|s| s.as_str()),
        Some("header-sess-123")
    );
    assert_eq!(
        attrs.get("aops.session.source").map(|s| s.as_str()),
        Some("header")
    );
}

#[tokio::test]
async fn test_session_source_meta() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("session_id".to_string(), json!("meta-sess-456"));
    let meta = make_meta(meta_map);

    let res = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, Some(&meta))
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(
        attrs.get("session.id").map(|s| s.as_str()),
        Some("meta-sess-456")
    );
    assert_eq!(
        attrs.get("aops.session.source").map(|s| s.as_str()),
        Some("meta")
    );
}

#[tokio::test]
async fn test_session_source_argument_preserved() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let args = json!({
        "query": "alpha",
        "session_id": "arg-sess-789"
    });

    let res = server
        .execute_tool_call("search", &args, None, None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(
        attrs.get("session.id").map(|s| s.as_str()),
        Some("arg-sess-789")
    );
    assert_eq!(
        attrs.get("aops.session.source").map(|s| s.as_str()),
        Some("argument")
    );
    let input_val = attrs.get("input.value").expect("input.value");
    assert!(
        input_val.contains("arg-sess-789"),
        "top-level args['session_id'] must not be stripped: {input_val}"
    );
}

#[tokio::test]
async fn test_session_precedence_header_over_meta_over_arg() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let req = http::Request::builder()
        .header("x-session-id", "hdr-1")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    let mut meta_map = serde_json::Map::new();
    meta_map.insert("session_id".to_string(), json!("meta-2"));
    let meta = make_meta(meta_map);

    let args = json!({
        "query": "alpha",
        "session_id": "arg-3"
    });

    server
        .execute_tool_call("search", &args, Some(&parts), Some(&meta))
        .await
        .unwrap();

    let spans = exporter.spans.lock().unwrap().clone();
    let span1 = spans.last().expect("span 1");
    let attrs1: std::collections::HashMap<String, String> = span1
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs1.get("session.id").map(|s| s.as_str()), Some("hdr-1"));
    assert_eq!(
        attrs1.get("aops.session.source").map(|s| s.as_str()),
        Some("header")
    );

    // Without header, meta wins over argument
    let req_no_sess = http::Request::builder().body(()).unwrap();
    let parts_no_sess = req_no_sess.into_parts().0;

    server
        .execute_tool_call("search", &args, Some(&parts_no_sess), Some(&meta))
        .await
        .unwrap();

    let spans2 = exporter.spans.lock().unwrap().clone();
    let span2 = spans2.last().expect("span 2");
    let attrs2: std::collections::HashMap<String, String> = span2
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs2.get("session.id").map(|s| s.as_str()), Some("meta-2"));
    assert_eq!(
        attrs2.get("aops.session.source").map(|s| s.as_str()),
        Some("meta")
    );
}

#[tokio::test]
async fn test_missing_or_empty_session_id_rejected_on_reads_and_writes() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();

    // Missing session_id on read tool
    let read_err = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await
        .expect_err("read without session_id must fail");
    assert_eq!(read_err.code, ErrorCode::INVALID_PARAMS);
    assert!(read_err.message.contains("session_id"));

    // Empty / whitespace session_id on read tool
    let whitespace_err = server
        .execute_tool_call(
            "search",
            &json!({"query": "alpha", "session_id": "   "}),
            None,
            None,
        )
        .await
        .expect_err("read with whitespace session_id must fail");
    assert_eq!(whitespace_err.code, ErrorCode::INVALID_PARAMS);

    // Missing session_id on write tool
    let write_err = server
        .execute_tool_call(
            "create_task",
            &json!({"title": "Unattributed task", "project": "test"}),
            None,
            None,
        )
        .await
        .expect_err("write without session_id must fail");
    assert_eq!(write_err.code, ErrorCode::INVALID_PARAMS);

    // Providing session_id succeeds on both
    server
        .execute_tool_call(
            "search",
            &json!({"query": "alpha", "session_id": "valid-sess"}),
            None,
            None,
        )
        .await
        .expect("read with session_id must succeed");

    let spans = exporter.spans.lock().unwrap().clone();
    let missing_span = spans
        .iter()
        .find(|s| s.name == "mcp.create_task")
        .expect("mcp.create_task span emitted on rejection");
    let attrs: std::collections::HashMap<String, String> = missing_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(
        attrs.get("aops.session.source").map(|s| s.as_str()),
        Some("none")
    );
    assert_eq!(
        attrs.get("aops.session.missing").map(|s| s.as_str()),
        Some("true")
    );
}

#[tokio::test]
async fn test_stateless_client_info_trace_and_no_leakage() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();

    let trace_id_str = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent_span_id_str = "00f067aa0ba902b7";
    let traceparent = format!("00-{trace_id_str}-{parent_span_id_str}-01");

    let req = http::Request::builder()
        .header("traceparent", &traceparent)
        .header("x-session-id", "stateless-sess-123")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    let res = server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts), None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans
        .iter()
        .rfind(|s| s.name == "mcp.search")
        .expect("mcp.search span");
    assert_eq!(span.span_context.trace_id().to_string(), trace_id_str);
    assert_eq!(span.parent_span_id.to_string(), parent_span_id_str);

    // Subsequent call without session_id does not inherit prior request's session_id
    let res2 = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await;
    assert!(res2.is_err(), "subsequent call without session_id must fail");
}
