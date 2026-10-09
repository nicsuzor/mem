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

#[tokio::test]
async fn test_named_spans_emitted() {
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
