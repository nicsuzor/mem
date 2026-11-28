use super::*;
use crate::mcp_server::tests::otel_spans_tests::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;

fn make_meta(map: serde_json::Map<String, serde_json::Value>) -> rmcp::model::RequestMetaObject {
    rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map))
}

fn build_session_test_server() -> PkbSearchServer {
    let server = build_test_server();
    let doc = crate::pkb::PkbDocument {
        path: std::path::PathBuf::from("tasks/alpha.md"),
        title: "Alpha Task".to_string(),
        tags: vec![],
        doc_type: Some("task".to_string()),
        status: Some("ready".to_string()),
        consolidated: None,
        consolidated_at: None,
        modified: None,
        body: "Alpha body".to_string(),
        content_hash: "h".to_string(),
        file_hash: "h".to_string(),
        frontmatter: None,
    };
    server
        .store_for_test()
        .write()
        .insert_precomputed(&doc, vec!["Alpha body".to_string()], vec![vec![0.1; 3]]);
    server
}

#[tokio::test]
async fn test_session_source_header() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();
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
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();
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
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();
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
async fn test_session_precedence_meta_over_header_over_arg() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();
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

    exporter.spans.lock().unwrap().clear();
    server
        .execute_tool_call("search", &args, Some(&parts), Some(&meta))
        .await
        .unwrap();

    let spans = exporter.spans.lock().unwrap().clone();
    let span1 = spans
        .iter()
        .rfind(|s| s.name == "mcp.search")
        .expect("mcp.search span 1");
    let attrs1: std::collections::HashMap<String, String> = span1
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs1.get("session.id").map(|s| s.as_str()), Some("meta-2"));
    assert_eq!(
        attrs1.get("aops.session.source").map(|s| s.as_str()),
        Some("meta")
    );

    // Without meta, header wins over argument
    exporter.spans.lock().unwrap().clear();
    server
        .execute_tool_call("search", &args, Some(&parts), None)
        .await
        .unwrap();

    let spans2 = exporter.spans.lock().unwrap().clone();
    let span2 = spans2
        .iter()
        .rfind(|s| s.name == "mcp.search")
        .expect("mcp.search span 2");
    let attrs2: std::collections::HashMap<String, String> = span2
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs2.get("session.id").map(|s| s.as_str()), Some("hdr-1"));
    assert_eq!(
        attrs2.get("aops.session.source").map(|s| s.as_str()),
        Some("header")
    );
}

#[tokio::test]
async fn test_calls_without_session_id_succeed_and_baggage_populates_task_frontmatter() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();

    // 1. Read without session_id succeeds
    let read_res = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await
        .expect("read without session_id must succeed");
    assert!(!read_res.is_error.unwrap_or(false));

    // 2. Legacy caller passing session_id in args still succeeds without schema error
    let legacy_res = server
        .execute_tool_call(
            "search",
            &json!({"query": "alpha", "session_id": "legacy-sess-1"}),
            None,
            None,
        )
        .await
        .expect("legacy session_id arg must succeed");
    assert!(!legacy_res.is_error.unwrap_or(false));

    // 3. create_task without session_id succeeds and relies on assignee when baggage absent
    let create_no_sess = server
        .execute_tool_call(
            "create_task",
            &json!({"title": "Unattributed task", "slug": "unattr-task", "project": "test", "assignee": "test-agent", "allow_missing_parent": true}),
            None,
            None,
        )
        .await
        .expect("create_task without session_id must succeed");
    assert!(!create_no_sess.is_error.unwrap_or(false));

    // 4. create_task, claim_task, release_task with baggage["session.id"] populate frontmatter session_id
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("baggage".to_string(), json!("session.id=baggage-sess-999"));
    meta_map.insert(
        "io.modelcontextprotocol/clientInfo".to_string(),
        json!({"name": "claude-code", "version": "2.1.0"}),
    );
    let meta = make_meta(meta_map);
    let req = http::Request::builder()
        .header("x-forwarded-user-agent", "ClaudeCode/2.1.0")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    let created = server
        .execute_tool_call(
            "create_task",
            &json!({"title": "Baggage task", "slug": "baggage-task", "project": "test", "allow_missing_parent": true}),
            Some(&parts),
            Some(&meta),
        )
        .await
        .expect("create_task with baggage must succeed");
    let created_id = created
        .structured_content
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .expect("created task id")
        .to_string();

    let fetched = server
        .execute_tool_call("get_task", &json!({"id": created_id}), None, None)
        .await
        .unwrap();
    let fm = fetched
        .structured_content
        .as_ref()
        .and_then(|v| v.get("frontmatter"))
        .expect("frontmatter");
    assert_eq!(
        fm.get("session_id").and_then(|v| v.as_str()),
        Some("baggage-sess-999"),
        "create_task must populate frontmatter session_id from baggage['session.id']"
    );

    let claimed = server
        .execute_tool_call("claim_task", &json!({"id": created_id, "assignee": "test-agent"}), Some(&parts), Some(&meta))
        .await
        .expect("claim_task with baggage must succeed");
    assert_eq!(
        claimed
            .structured_content
            .as_ref()
            .and_then(|v| v.get("frontmatter"))
            .and_then(|v| v.get("session_id"))
            .and_then(|v| v.as_str()),
        Some("baggage-sess-999"),
        "claim_task must populate frontmatter session_id from baggage['session.id']"
    );

    let released = server
        .execute_tool_call(
            "release_task",
            &json!({"id": created_id, "status": "done", "summary": "Completed via baggage test"}),
            Some(&parts),
            Some(&meta),
        )
        .await
        .expect("release_task with baggage must succeed");
    assert!(!released.is_error.unwrap_or(false));

    let spans = exporter.spans.lock().unwrap().clone();
    let unattr_span = spans
        .iter()
        .find(|s| {
            s.name == "mcp.create_task"
                && s.attributes.iter().any(|kv| {
                    kv.key.as_str() == "input.value" && kv.value.as_str().contains("unattr-task")
                })
        })
        .expect("mcp.create_task span emitted");
    let attrs: std::collections::HashMap<String, String> = unattr_span
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

    let baggage_span = spans
        .iter()
        .rfind(|s| {
            s.name == "mcp.create_task"
                && s.attributes.iter().any(|kv| {
                    kv.key.as_str() == "input.value" && kv.value.as_str().contains("baggage-task")
                })
        })
        .expect("second mcp.create_task span emitted");
    let b_attrs: std::collections::HashMap<String, String> = baggage_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(
        b_attrs.get("session.id").map(|s| s.as_str()),
        Some("baggage-sess-999")
    );
    assert_eq!(
        b_attrs.get("client.name").map(|s| s.as_str()),
        Some("claude-code")
    );
    assert_eq!(
        b_attrs.get("user_agent.original").map(|s| s.as_str()),
        Some("ClaudeCode/2.1.0")
    );
}

#[tokio::test]
async fn test_stateless_client_info_trace_and_no_leakage() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_session_test_server();

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
    exporter.spans.lock().unwrap().clear();
    let res2 = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await;
    assert!(res2.is_ok(), "subsequent call without session_id must succeed");
    let spans2 = exporter.spans.lock().unwrap().clone();
    let span2 = spans2
        .iter()
        .rfind(|s| s.name == "mcp.search")
        .expect("second mcp.search span");
    let attrs2: std::collections::HashMap<String, String> = span2
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs2.get("session.id"), None);
    assert_eq!(
        attrs2.get("aops.session.source").map(|s| s.as_str()),
        Some("none")
    );
}

#[tokio::test]
async fn test_claim_task_without_prior_session_id_and_update_task_no_baggage_leak() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let server = build_session_test_server();

    // 1. Create a task WITHOUT session_id
    let created = server
        .execute_tool_call(
            "create_task",
            &json!({
                "title": "Task without initial session",
                "slug": "no-sess-init",
                "project": "test",
                "allow_missing_parent": true
            }),
            None,
            None,
        )
        .await
        .expect("create_task without session_id must succeed");
    assert!(!created.is_error.unwrap_or(false));
    let created_id = created
        .structured_content
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .expect("created task id")
        .to_string();

    let initial = server
        .execute_tool_call("get_task", &json!({"id": created_id}), None, None)
        .await
        .unwrap();
    let initial_fm = initial
        .structured_content
        .as_ref()
        .and_then(|v| v.get("frontmatter"))
        .expect("initial frontmatter");
    assert_eq!(
        initial_fm.get("session_id"),
        None,
        "task created without session_id must start with no session_id in frontmatter"
    );

    // 2. Claim that task with baggage["session.id"] and clientInfo -> must persist session_id (via updates) without leaking _baggage_session_id/_client
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("baggage".to_string(), json!("session.id=claim-baggage-sess"));
    meta_map.insert(
        "io.modelcontextprotocol/clientInfo".to_string(),
        json!({"name": "overwhelm-dashboard", "version": "1.0.0"}),
    );
    let meta = make_meta(meta_map);

    let claimed = server
        .execute_tool_call(
            "claim_task",
            &json!({"id": created_id, "assignee": "test-agent"}),
            None,
            Some(&meta),
        )
        .await
        .expect("claim_task with baggage must succeed");
    assert!(!claimed.is_error.unwrap_or(false));
    let claimed_fm = claimed
        .structured_content
        .as_ref()
        .and_then(|v| v.get("frontmatter"))
        .expect("claimed frontmatter");
    assert_eq!(
        claimed_fm.get("session_id").and_then(|v| v.as_str()),
        Some("claim-baggage-sess"),
        "claim_task must persist session_id from baggage onto task created without session_id"
    );
    assert_eq!(claimed_fm.get("status").and_then(|v| v.as_str()), Some("in_progress"));
    assert_eq!(claimed_fm.get("_baggage_session_id"), None);
    assert_eq!(claimed_fm.get("_client"), None);

    // 3. Call update_task with baggage["session.id"] and dashboard clientInfo -> must not leak routing keys into frontmatter
    let updated = server
        .execute_tool_call(
            "update_task",
            &json!({"id": created_id, "updates": {"priority": 2}}),
            None,
            Some(&meta),
        )
        .await
        .expect("update_task with baggage and clientInfo must succeed");
    assert!(
        !updated.is_error.unwrap_or(false),
        "update_task must succeed: {updated:?}"
    );

    let after_update = server
        .execute_tool_call("get_task", &json!({"id": created_id}), None, None)
        .await
        .unwrap();
    let updated_fm = after_update
        .structured_content
        .as_ref()
        .and_then(|v| v.get("frontmatter"))
        .expect("updated frontmatter");
    assert_eq!(updated_fm.get("priority").and_then(|v| v.as_i64()), Some(2));
    assert_eq!(
        updated_fm.get("_baggage_session_id"),
        None,
        "_baggage_session_id must never leak into frontmatter: {updated_fm:?}"
    );
    assert_eq!(
        updated_fm.get("_client"),
        None,
        "_client must never leak into frontmatter: {updated_fm:?}"
    );
    assert_eq!(
        updated_fm.get("_meta"),
        None,
        "_meta must never leak into frontmatter: {updated_fm:?}"
    );
}

#[tokio::test]
async fn test_task_summary_accepts_injected_baggage_and_client_info() {
    let _lock = crate::mcp_server::tests::otel_spans_tests::OTEL_TEST_MUTEX.lock().await;
    let server = build_session_test_server();

    let mut meta_map = serde_json::Map::new();
    meta_map.insert("baggage".to_string(), json!("session.id=summary-baggage-sess"));
    meta_map.insert(
        "io.modelcontextprotocol/clientInfo".to_string(),
        json!({"name": "overwhelm-dashboard", "version": "1.0.0"}),
    );
    let meta = make_meta(meta_map);

    let res = server
        .execute_tool_call("task_summary", &json!({}), None, Some(&meta))
        .await
        .expect("task_summary with baggage and clientInfo must succeed");
    assert!(
        !res.is_error.unwrap_or(false),
        "task_summary must not reject injected _baggage_session_id or _client: {res:?}"
    );
}
