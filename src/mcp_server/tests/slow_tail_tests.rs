use super::*;
use std::sync::Arc;
use std::time::Instant;

fn make_test_server(root: &Path) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);
    let graph = GraphStore::build(&[], root);
    let store = VectorStore::new(3);
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
fn test_repeated_queries_hit_embedding_cache() {
    let embedder = Embedder::new_dummy();
    let query = "slow tail efficiency review";

    let start1 = Instant::now();
    let vec1 = embedder.encode_query(query).unwrap();
    let dur1 = start1.elapsed();

    let start2 = Instant::now();
    let vec2 = embedder.encode_query(query).unwrap();
    let dur2 = start2.elapsed();

    assert_eq!(vec1, vec2, "Cached vector must match original");
    // Verify cache hit: second call should be fast and hit cache
    assert!(
        embedder.query_cache_len() >= 1,
        "Query cache must store query embedding"
    );
    assert!(
        dur2 <= dur1 || dur2.as_millis() < 5,
        "Repeated query should hit cache quickly (dur1: {dur1:?}, dur2: {dur2:?})"
    );
}

#[test]
fn test_concurrent_searches_do_not_serialise() {
    let mut embedder = Embedder::new_dummy();
    // Configure simulated session inference delay to test concurrency
    embedder.set_dummy_delay(std::time::Duration::from_millis(50));
    let embedder = Arc::new(embedder);

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let b1 = Arc::clone(&barrier);
    let e1 = Arc::clone(&embedder);
    let t1 = std::thread::spawn(move || {
        b1.wait();
        e1.encode_query("query alpha").unwrap()
    });
    let b2 = Arc::clone(&barrier);
    let e2 = Arc::clone(&embedder);
    let t2 = std::thread::spawn(move || {
        b2.wait();
        e2.encode_query("query beta").unwrap()
    });

    let start = Instant::now();
    let _ = t1.join().unwrap();
    let _ = t2.join().unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(90),
        "Concurrent searches serialised! Elapsed: {elapsed:?}, expected < 90ms for two 50ms inferences"
    );
}

#[test]
fn test_pkb_search_ids_only_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // Create two sample tasks via server so vector store is populated
    server
        .handle_create_task(&json!({
            "id": "task-1",
            "title": "Alpha task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();
    server
        .handle_create_task(&json!({
            "id": "task-2",
            "title": "Beta task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();

    // 1. With detail: "ids"
    let res = server
        .handle_pkb_search(&json!({
            "query": "task",
            "detail": "ids",
            "format": "json"
        }))
        .expect("search should succeed");
    let structured = res.structured_content.expect("structuredContent required");
    assert!(
        structured.get("ids").is_some(),
        "Expected 'ids' field in ids-only mode"
    );
    let ids = structured["ids"].as_array().expect("ids must be an array");
    assert!(!ids.is_empty(), "Should return matching ids");

    // 2. With ids_only: true
    let res2 = server
        .handle_pkb_search(&json!({
            "query": "task",
            "ids_only": true,
            "format": "json"
        }))
        .expect("search should succeed");
    let structured2 = res2.structured_content.expect("structuredContent required");
    assert!(
        structured2.get("ids").is_some(),
        "Expected 'ids' field when ids_only=true"
    );
}

#[test]
fn test_list_tasks_projects_body_excerpt_and_relationship_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let parent_path = root.join("tasks/parent.md");
    std::fs::write(
        &parent_path,
        "---\nid: parent-1\ntitle: Parent Task\ntype: task\nstatus: ready\n---\nThis is the parent task long description body that should be excerpted.\n",
    )
    .unwrap();
    let child_path = root.join("tasks/child.md");
    std::fs::write(
        &child_path,
        "---\nid: child-1\ntitle: Child Task\ntype: task\nstatus: ready\nparent: parent-1\ndepends_on: [parent-1]\n---\nChild body content.\n",
    )
    .unwrap();
    server.rebuild_graph();

    // Call list_tasks requesting body_excerpt and relationship_summary
    let res = server
        .handle_list_tasks(&json!({
            "fields": ["id", "title", "body_excerpt", "relationship_summary"]
        }))
        .expect("list_tasks should succeed");
    let structured = res.structured_content.expect("structuredContent required");
    let items = structured["items"].as_array().expect("items array required");
    assert!(!items.is_empty());

    let child_item = items
        .iter()
        .find(|item| item["id"] == "child-1")
        .expect("child-1 item found");

    assert!(
        child_item.get("body_excerpt").is_some(),
        "Expected 'body_excerpt' projected field in: {child_item:?}"
    );
    let excerpt = child_item["body_excerpt"].as_str().unwrap();
    assert!(
        excerpt.contains("Child body content"),
        "body_excerpt should contain file body text: {excerpt}"
    );

    assert!(
        child_item.get("relationship_summary").is_some(),
        "Expected 'relationship_summary' projected field in: {child_item:?}"
    );
    let rel = &child_item["relationship_summary"];
    assert!(
        rel.get("parent").is_some(),
        "relationship_summary must include parent"
    );
    assert!(
        rel.get("depends_on").is_some(),
        "relationship_summary must include depends_on"
    );
    assert!(
        rel.get("blocked").is_some(),
        "relationship_summary must include blocked"
    );
}

#[test]
fn test_list_tasks_honours_budget_ms_and_returns_timed_out() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    for i in 0..10 {
        let p = root.join(format!("tasks/task_{i}.md"));
        std::fs::write(
            &p,
            format!("---\nid: task-{i}\ntitle: Task {i}\ntype: task\nstatus: ready\n---\nBody {i}\n"),
        )
        .unwrap();
    }
    server.rebuild_graph();

    // Call with budget_ms: 0 to force timeout
    let res = server
        .handle_list_tasks(&json!({
            "budget_ms": 0,
            "fields": ["id", "body_excerpt"]
        }))
        .expect("list_tasks should succeed with partial results");
    let structured = res.structured_content.expect("structuredContent required");
    assert!(
        structured.get("timed_out").is_some(),
        "Expected 'timed_out' field in response"
    );
    let timed_out = structured["timed_out"].as_array().expect("timed_out must be an array");
    assert!(
        !timed_out.is_empty(),
        "With 0 budget_ms, items should time out"
    );
}

#[test]
fn test_batch_get_task_and_document_honour_budget_ms_and_return_timed_out() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    for i in 0..5 {
        let p = root.join(format!("tasks/t_{i}.md"));
        std::fs::write(
            &p,
            format!("---\nid: t-{i}\ntitle: Task {i}\ntype: task\nstatus: ready\n---\nBody {i}\n"),
        )
        .unwrap();
    }
    server.rebuild_graph();

    // 1. get_task batch with budget_ms: 0
    let res_task = server
        .handle_get_task(&json!({
            "ids": ["t-0", "t-1", "t-2", "t-3", "t-4"],
            "budget_ms": 0
        }))
        .expect("batch get_task should succeed");
    let structured_task = res_task.structured_content.expect("structuredContent required");
    assert!(
        structured_task.get("timed_out").is_some(),
        "Expected 'timed_out' in batch get_task: {structured_task:?}"
    );
    let to_task = structured_task["timed_out"].as_array().unwrap();
    assert!(!to_task.is_empty(), "Expected timed_out IDs in batch get_task");

    // 2. get_document batch with budget_ms: 0
    let res_doc = server
        .handle_get_document(&json!({
            "ids": ["t-0", "t-1", "t-2", "t-3", "t-4"],
            "budget_ms": 0
        }))
        .expect("batch get_document should succeed");
    let structured_doc = res_doc.structured_content.expect("structuredContent required");
    assert!(
        structured_doc.get("timed_out").is_some(),
        "Expected 'timed_out' in batch get_document: {structured_doc:?}"
    );
    let to_doc = structured_doc["timed_out"].as_array().unwrap();
    assert!(!to_doc.is_empty(), "Expected timed_out IDs in batch get_document");
}

#[test]
fn test_list_documents_honours_budget_ms_and_returns_timed_out() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    for i in 0..10 {
        server
            .handle_create_memory(&json!({
                "id": format!("doc-{i}"),
                "title": format!("Document {i}"),
                "body": format!("Body {i}"),
            }))
            .unwrap();
    }

    let res = server
        .handle_list_documents(&json!({
            "budget_ms": 0,
            "format": "json"
        }))
        .expect("list_documents should succeed with partial results");
    let structured = res.structured_content.expect("structuredContent required");
    assert!(
        structured.get("timed_out").is_some(),
        "Expected 'timed_out' field in response"
    );
    let timed_out = structured["timed_out"].as_array().expect("timed_out must be an array");
    assert!(
        !timed_out.is_empty(),
        "With 0 budget_ms, items should time out"
    );
}

#[test]
fn test_ensure_graph_fresh_rebuilds_once_under_concurrent_callers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = Arc::new(make_test_server(root));

    // Initially fresh
    server.ensure_graph_fresh();
    let initial_rebuilds = server.rebuild_count();

    // Create a new file externally on disk to trigger stale generation
    let new_task = root.join("tasks/external_task.md");
    std::fs::write(
        &new_task,
        "---\nid: ext-1\ntitle: External Task\ntype: task\nstatus: ready\n---\nExternal\n",
    )
    .unwrap();

    // Launch 10 concurrent threads all calling ensure_graph_fresh simultaneously
    let barrier = Arc::new(std::sync::Barrier::new(10));
    let mut handles = Vec::new();
    for _ in 0..10 {
        let s = Arc::clone(&server);
        let b = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            b.wait();
            s.ensure_graph_fresh();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let final_rebuilds = server.rebuild_count();
    assert_eq!(
        final_rebuilds - initial_rebuilds,
        1,
        "ensure_graph_fresh should rebuild exactly once under concurrent callers (initial: {initial_rebuilds}, final: {final_rebuilds})"
    );
}
