use super::*;
use crate::mcp_server::tests::otel_spans_tests::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;

static CALLER_SESSION_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let span = spans.iter().find(|s| s.name == "mcp.search").expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(attrs.get("session.id").map(|s| s.as_str()), Some("header-sess-123"));
    assert_eq!(attrs.get("aops.session.source").map(|s| s.as_str()), Some("header"));
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
    let meta = rmcp::model::Meta(meta_map);

    let res = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, Some(&meta))
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans.iter().find(|s| s.name == "mcp.search").expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(attrs.get("session.id").map(|s| s.as_str()), Some("meta-sess-456"));
    assert_eq!(attrs.get("aops.session.source").map(|s| s.as_str()), Some("meta"));
}

#[tokio::test]
async fn test_session_source_argument_and_stripping() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let args = json!({
        "query": "alpha",
        "_session": {
            "session_id": "arg-sess-789",
            "agent_id": "agent-xyz",
            "prompt_id": "prompt-uvw"
        }
    });

    let res = server
        .execute_tool_call("search", &args, None, None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans.iter().find(|s| s.name == "mcp.search").expect("mcp.search span");
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert_eq!(attrs.get("session.id").map(|s| s.as_str()), Some("arg-sess-789"));
    assert_eq!(attrs.get("aops.session.source").map(|s| s.as_str()), Some("argument"));
    assert_eq!(attrs.get("agent.id").map(|s| s.as_str()), Some("agent-xyz"));
    assert_eq!(attrs.get("prompt.id").map(|s| s.as_str()), Some("prompt-uvw"));
}

#[tokio::test]
async fn test_session_precedence() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();

    // 1. Header beats Meta and Argument
    let req = http::Request::builder()
        .header("x-session-id", "header-wins")
        .header("mcp-session-id", "conn-prec-1")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("session_id".to_string(), json!("meta-loses"));
    let meta = rmcp::model::Meta(meta_map);
    let args = json!({
        "query": "alpha",
        "_session": { "session_id": "arg-loses" }
    });

    server
        .execute_tool_call("search", &args, Some(&parts), Some(&meta))
        .await
        .unwrap();

    let spans1 = exporter.spans.lock().unwrap().clone();
    let span1 = spans1.iter().find(|s| s.name == "mcp.search").unwrap();
    let attrs1: std::collections::HashMap<String, String> = span1
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs1.get("session.id").map(|s| s.as_str()), Some("header-wins"));
    assert_eq!(attrs1.get("aops.session.source").map(|s| s.as_str()), Some("header"));

    // 2. Meta beats Argument
    let req = http::Request::builder()
        .header("mcp-session-id", "conn-prec-2")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("session_id".to_string(), json!("meta-wins"));
    let meta = rmcp::model::Meta(meta_map);
    let args = json!({
        "query": "alpha",
        "_session": { "session_id": "arg-loses" }
    });

    server
        .execute_tool_call("search", &args, Some(&parts), Some(&meta))
        .await
        .unwrap();

    let spans2 = exporter.spans.lock().unwrap().clone();
    let span2 = spans2.iter().rfind(|s| s.name == "mcp.search").unwrap();
    let attrs2: std::collections::HashMap<String, String> = span2
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs2.get("session.id").map(|s| s.as_str()), Some("meta-wins"));
    assert_eq!(attrs2.get("aops.session.source").map(|s| s.as_str()), Some("meta"));
}

#[tokio::test]
async fn test_direct_connection_binding_and_rebinding() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let conn_id = "direct-conn-test-1";

    // Call 1: explicit header binds connection
    let req1 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .header("x-session-id", "sess-bound-1")
        .body(())
        .unwrap();
    let parts1 = req1.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts1), None)
        .await
        .unwrap();

    // Call 2: no explicit session -> uses binding
    let req2 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .body(())
        .unwrap();
    let parts2 = req2.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts2), None)
        .await
        .unwrap();

    // Call 3: different explicit session -> rebinds, records rebound_from
    let req3 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .header("x-session-id", "sess-bound-2")
        .body(())
        .unwrap();
    let parts3 = req3.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts3), None)
        .await
        .unwrap();

    // Call 4: no explicit session -> uses new binding
    let req4 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .body(())
        .unwrap();
    let parts4 = req4.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts4), None)
        .await
        .unwrap();

    let spans = exporter.spans.lock().unwrap().clone();
    let mcp_spans: Vec<_> = spans.iter().filter(|s| s.name == "mcp.search").collect();
    assert_eq!(mcp_spans.len(), 4);

    let get_attrs = |idx: usize| -> std::collections::HashMap<String, String> {
        mcp_spans[idx]
            .attributes
            .iter()
            .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
            .collect()
    };

    let a1 = get_attrs(0);
    assert_eq!(a1.get("session.id").map(|s| s.as_str()), Some("sess-bound-1"));
    assert_eq!(a1.get("aops.session.source").map(|s| s.as_str()), Some("header"));
    assert!(!a1.contains_key("aops.session.rebound_from"));

    let a2 = get_attrs(1);
    assert_eq!(a2.get("session.id").map(|s| s.as_str()), Some("sess-bound-1"));
    assert_eq!(a2.get("aops.session.source").map(|s| s.as_str()), Some("binding"));

    let a3 = get_attrs(2);
    assert_eq!(a3.get("session.id").map(|s| s.as_str()), Some("sess-bound-2"));
    assert_eq!(a3.get("aops.session.source").map(|s| s.as_str()), Some("header"));
    assert_eq!(a3.get("aops.session.rebound_from").map(|s| s.as_str()), Some("sess-bound-1"));

    let a4 = get_attrs(3);
    assert_eq!(a4.get("session.id").map(|s| s.as_str()), Some("sess-bound-2"));
    assert_eq!(a4.get("aops.session.source").map(|s| s.as_str()), Some("binding"));
}

#[tokio::test]
async fn test_gateway_client_never_binds() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    std::env::set_var("PKB_GATEWAY_CLIENTS", "gateway-proxy,services");

    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let conn_id = "gw-conn-test-1";

    // Simulate initialize handshake naming the gateway client
    server
        .session_registry
        .register_handshake(conn_id, Some("services".to_string()));

    // Call 1: carries explicit session
    let req1 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .header("x-session-id", "agent-session-via-gw")
        .body(())
        .unwrap();
    let parts1 = req1.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts1), None)
        .await
        .unwrap();

    // Call 2: carries no explicit session -> MUST NOT bind or attribute from binding!
    let req2 = http::Request::builder()
        .header("mcp-session-id", conn_id)
        .body(())
        .unwrap();
    let parts2 = req2.into_parts().0;
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts2), None)
        .await
        .unwrap();

    let spans = exporter.spans.lock().unwrap().clone();
    let mcp_spans: Vec<_> = spans.iter().filter(|s| s.name == "mcp.search").collect();
    assert_eq!(mcp_spans.len(), 2);

    let a1: std::collections::HashMap<String, String> = mcp_spans[0]
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(a1.get("session.id").map(|s| s.as_str()), Some("agent-session-via-gw"));
    assert_eq!(a1.get("aops.session.source").map(|s| s.as_str()), Some("header"));

    let a2: std::collections::HashMap<String, String> = mcp_spans[1]
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    // session.id must be UNSET
    assert!(!a2.contains_key("session.id"), "session.id must be unset on gateway call with no explicit id");
    assert_eq!(a2.get("aops.session.source").map(|s| s.as_str()), Some("none"));
    assert_eq!(a2.get("mcp.session_id").map(|s| s.as_str()), Some(conn_id));

    std::env::remove_var("PKB_GATEWAY_CLIENTS");
}

#[tokio::test]
async fn test_no_invented_session() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    std::env::set_var("AOPS_SESSION_ID", "invented-env-session-id");

    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();
    let req = http::Request::builder()
        .header("mcp-session-id", "transport-conn-no-explicit")
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    server
        .execute_tool_call("search", &json!({"query": "alpha"}), Some(&parts), None)
        .await
        .unwrap();

    let spans = exporter.spans.lock().unwrap().clone();
    let span = spans.iter().find(|s| s.name == "mcp.search").unwrap();
    let attrs: std::collections::HashMap<String, String> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    // Neither AOPS_SESSION_ID, nor mcp_session_id, nor "unknown" should be in session.id
    assert!(!attrs.contains_key("session.id"), "session.id must be unset when unattributed");
    assert_eq!(attrs.get("aops.session.source").map(|s| s.as_str()), Some("none"));
    assert_eq!(attrs.get("mcp.session_id").map(|s| s.as_str()), Some("transport-conn-no-explicit"));

    std::env::remove_var("AOPS_SESSION_ID");
}

#[tokio::test]
async fn test_session_mode_warn_and_enforce() {
    let _lock = CALLER_SESSION_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let server = build_test_server();

    // 1. Warn mode (default): mutating tool succeeds and span carries aops.session.missing = true
    std::env::set_var("PKB_SESSION_MODE", "warn");
    let create_args = json!({
        "title": "Unattributed Task Warn",
        "project": "proj-test"
    });
    let res_warn = server
        .execute_tool_call("create_task", &create_args, None, None)
        .await;
    assert!(res_warn.is_ok(), "warn mode must allow unattributed write to succeed");

    let spans1 = exporter.spans.lock().unwrap().clone();
    let create_span = spans1.iter().find(|s| s.name == "mcp.create_task").unwrap();
    let attrs1: std::collections::HashMap<String, String> = create_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs1.get("aops.session.source").map(|s| s.as_str()), Some("none"));
    assert_eq!(attrs1.get("aops.session.missing").map(|s| s.as_str()), Some("true"));
    assert!(!attrs1.contains_key("session.id"));

    // Read-only tool in warn mode: succeeds and does NOT set aops.session.missing
    server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await
        .unwrap();
    let spans2 = exporter.spans.lock().unwrap().clone();
    let read_span = spans2.iter().rfind(|s| s.name == "mcp.search").unwrap();
    let attrs2: std::collections::HashMap<String, String> = read_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();
    assert_eq!(attrs2.get("aops.session.source").map(|s| s.as_str()), Some("none"));
    assert!(!attrs2.contains_key("aops.session.missing"), "read-only tool should not have missing flag");

    // 2. Enforce mode: mutating tool without session is REFUSED with error naming _session and AOPS_SESSION_ID
    std::env::set_var("PKB_SESSION_MODE", "enforce");
    let create_args_enforce = json!({
        "title": "Unattributed Task Enforce",
        "project": "proj-test"
    });
    let res_enforce = server
        .execute_tool_call("create_task", &create_args_enforce, None, None)
        .await;
    assert!(res_enforce.is_err(), "enforce mode must refuse unattributed write");
    let err = res_enforce.unwrap_err();
    assert!(
        err.message.contains("_session"),
        "error message must name the _session argument, got: {}",
        err.message
    );
    assert!(
        err.message.contains("session") || err.message.contains("AOPS_SESSION_ID"),
        "error message must guide agent on session id, got: {}",
        err.message
    );

    // Mutating tool with _session in enforce mode succeeds
    let create_args_with_session = json!({
        "title": "Attributed Task Enforce",
        "project": "proj-test",
        "_session": { "session_id": "session-enforce-ok" }
    });
    let res_with_session = server
        .execute_tool_call("create_task", &create_args_with_session, None, None)
        .await;
    assert!(res_with_session.is_ok(), "enforce mode must allow attributed write");

    // Read-only tool in enforce mode without session still succeeds
    let res_read = server
        .execute_tool_call("search", &json!({"query": "alpha"}), None, None)
        .await;
    assert!(res_read.is_ok(), "read-only tool in enforce mode must succeed even without session");

    std::env::remove_var("PKB_SESSION_MODE");
}
