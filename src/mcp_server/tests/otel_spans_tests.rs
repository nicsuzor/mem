use super::*;
use futures_util::future::BoxFuture;
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use opentelemetry_sdk::trace::TracerProvider;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub struct InMemorySpanExporter {
    pub spans: Arc<Mutex<Vec<SpanData>>>,
}

impl SpanExporter for InMemorySpanExporter {
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        if let Ok(mut lock) = self.spans.lock() {
            lock.extend(batch);
        }
        Box::pin(std::future::ready(Ok(())))
    }
    fn shutdown(&mut self) {}
}

static OTEL_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn test_named_spans_emitted() {
    let _lock = OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    std::fs::write(
        root.join("tasks/task1.md"),
        "---\nid: task1\ntype: task\ntitle: Searchable task\nstatus: active\n---\nBody content for search.\n",
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
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // 1. Run a search tool call (exercises ensure_graph_fresh, encode_query, session_mutex_wait, store_lock_acquisition, search_hybrid)
    let _ = server.handle_pkb_search(&json!({"query": "search query"})).unwrap();

    // 2. Modify disk so ensure_graph_fresh triggers rebuild_graph
    std::fs::write(
        root.join("tasks/task2.md"),
        "---\nid: task2\ntype: task\ntitle: Second task\nstatus: active\n---\nSecond body.\n",
    )
    .unwrap();
    server.ensure_graph_fresh();

    let finished_spans = exporter.spans.lock().unwrap();
    let span_names: Vec<String> = finished_spans.iter().map(|s| s.name.to_string()).collect();

    for expected in [
        "encode_query",
        "session_mutex_wait",
        "store_lock_acquisition",
        "ensure_graph_fresh",
        "rebuild_graph",
        "search_hybrid",
    ] {
        assert!(
            span_names.contains(&expected.to_string()),
            "expected span '{expected}' was not emitted. Emitted spans: {span_names:?}"
        );
    }
}

#[tokio::test]
async fn test_traceparent_parents_mcp_and_internal_spans() {
    let _lock = OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    std::fs::write(
        root.join("tasks/task1.md"),
        "---\nid: task1\ntype: task\ntitle: Searchable task\nstatus: active\n---\nBody content for search.\n",
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
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Incoming traceparent from caller
    let trace_id_str = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent_span_id_str = "00f067aa0ba902b7";
    let traceparent_header = format!("00-{}-{}-01", trace_id_str, parent_span_id_str);

    let req = http::Request::builder()
        .header("traceparent", &traceparent_header)
        .body(())
        .unwrap();
    let parts = req.into_parts().0;

    let res = server
        .execute_tool_call("search", &json!({"query": "search query"}), Some(&parts), None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap();
    let mcp_span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span must be emitted");

    assert_eq!(
        mcp_span.span_context.trace_id().to_string(),
        trace_id_str,
        "mcp.search span must have trace_id from traceparent"
    );
    assert_eq!(
        mcp_span.parent_span_id.to_string(),
        parent_span_id_str,
        "mcp.search span must be parented under incoming caller span"
    );

    // Child spans like encode_query, search_hybrid must be parented under mcp.search span
    let mcp_span_id = mcp_span.span_context.span_id().to_string();
    let child_spans: Vec<_> = spans
        .iter()
        .filter(|s| {
            s.parent_span_id.to_string() == mcp_span_id
                && (s.name == "encode_query" || s.name == "search_hybrid" || s.name == "ensure_graph_fresh")
        })
        .collect();

    assert!(!child_spans.is_empty(), "child spans must be present");
    for child in child_spans {
        assert_eq!(
            child.span_context.trace_id().to_string(),
            trace_id_str,
            "child span '{}' must share trace_id with traceparent",
            child.name
        );
        assert_eq!(
            child.parent_span_id.to_string(),
            mcp_span_id,
            "child span '{}' must be parented under mcp.search span",
            child.name
        );
    }
}

#[tokio::test]
async fn test_mcp_span_per_call_attributes() {
    let _lock = OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    std::fs::write(
        root.join("tasks/task1.md"),
        "---\nid: task1\ntype: task\ntitle: Searchable task\nstatus: active\n---\nBody content for search.\n",
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
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Modify disk so that ensure_graph_fresh triggers rebuild during tool call
    std::fs::write(
        root.join("tasks/task2.md"),
        "---\nid: task2\ntype: task\ntitle: Second task\nstatus: active\n---\nSecond body.\n",
    )
    .unwrap();

    let res = server
        .execute_tool_call("search", &json!({"query": "search query"}), None, None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap();
    let mcp_span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span must be emitted");

    let attrs: std::collections::HashMap<String, String> = mcp_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    // Verify result-bytes
    assert!(
        attrs.contains_key("result_bytes") || attrs.contains_key("result-bytes"),
        "mcp span must contain result-bytes attribute; attrs: {attrs:?}"
    );

    // Verify graph.read wait
    assert!(
        attrs.contains_key("graph.read_wait_ms") || attrs.contains_key("graph_read_wait_ms"),
        "mcp span must contain graph.read wait attribute; attrs: {attrs:?}"
    );

    // Verify rebuild-flag
    assert!(
        attrs.contains_key("rebuild_flag") || attrs.contains_key("ensure_graph_fresh.rebuilt"),
        "mcp span must contain rebuild-flag attribute; attrs: {attrs:?}"
    );
    let rebuild_val = attrs.get("rebuild_flag").or_else(|| attrs.get("ensure_graph_fresh.rebuilt")).unwrap();
    assert_eq!(rebuild_val, "true", "rebuild-flag must be true since disk changed before call");

    // Verify embed timing
    assert!(
        attrs.contains_key("embed_time_ms") || attrs.contains_key("embed.time_ms"),
        "mcp span must contain embed timing attribute; attrs: {attrs:?}"
    );

    // Verify hybrid-search timing
    assert!(
        attrs.contains_key("hybrid_search_time_ms") || attrs.contains_key("hybrid_search.time_ms"),
        "mcp span must contain hybrid-search timing attribute; attrs: {attrs:?}"
    );

    // Verify openinference project name
    assert_eq!(
        attrs.get("openinference.project.name").map(String::as_str),
        Some("academicOps"),
        "mcp span must contain openinference.project.name attribute; attrs: {attrs:?}"
    );
}

#[tokio::test]
async fn test_meta_traceparent_parents_mcp_and_internal_spans() {
    let _lock = OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    std::fs::write(
        root.join("tasks/task1.md"),
        "---\nid: task1\ntype: task\ntitle: Searchable task\nstatus: active\n---\nBody content for search.\n",
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
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let trace_id_str = "e4a7b54a88373b98c3683a3038612140";
    let parent_span_id_str = "a1b2c3d4e5f60718";
    let traceparent_str = format!("00-{}-{}-01", trace_id_str, parent_span_id_str);

    let mut meta_map = serde_json::Map::new();
    meta_map.insert("traceparent".to_string(), serde_json::Value::String(traceparent_str));
    let meta = rmcp::model::Meta(meta_map);

    let res = server
        .execute_tool_call("search", &json!({"query": "search query"}), None, Some(&meta))
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap();
    let mcp_span = spans
        .iter()
        .find(|s| s.name == "mcp.search")
        .expect("mcp.search span must be emitted");

    assert_eq!(
        mcp_span.span_context.trace_id().to_string(),
        trace_id_str,
        "mcp.search span must have trace_id from meta traceparent"
    );
    assert_eq!(
        mcp_span.parent_span_id.to_string(),
        parent_span_id_str,
        "mcp.search span must be parented under incoming caller span from meta"
    );
}

#[tokio::test]
async fn test_get_task_mcp_span_attributes() {
    let _lock = OTEL_TEST_MUTEX.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = opentelemetry::global::set_tracer_provider(provider.clone());

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    std::fs::write(
        root.join("tasks/task1.md"),
        "---\nid: task1\ntype: task\ntitle: Sample task\nstatus: active\n---\nBody content.\n",
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
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    let res = server
        .execute_tool_call("get_task", &json!({"id": "task1"}), None, None)
        .await
        .unwrap();
    assert!(!res.content.is_empty());

    let spans = exporter.spans.lock().unwrap();
    let mcp_span = spans
        .iter()
        .find(|s| s.name == "mcp.get_task")
        .expect("mcp.get_task span must be emitted");

    let attrs: std::collections::HashMap<String, String> = mcp_span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
        .collect();

    assert!(attrs.contains_key("result_bytes"));
    assert!(attrs.contains_key("graph.read_wait_ms"));
    assert!(attrs.contains_key("rebuild_flag"));
    assert_eq!(attrs.get("rebuild_flag").unwrap(), "false");
    assert!(attrs.contains_key("embed_time_ms"));
    assert_eq!(attrs.get("embed_time_ms").unwrap(), "0");
    assert!(attrs.contains_key("hybrid_search_time_ms"));
    assert_eq!(attrs.get("hybrid_search_time_ms").unwrap(), "0");
}



