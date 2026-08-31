use mem::pkb::PkbDocument;
use mem::vectordb::VectorStore;
use std::path::{Path, PathBuf};

fn make_doc(
    id: &str,
    path: &str,
    title: &str,
    doc_type: &str,
    body: &str,
    tags: Vec<&str>,
    modified: &str,
) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    fm.insert("title".to_string(), serde_json::Value::String(title.to_string()));
    fm.insert("type".to_string(), serde_json::Value::String(doc_type.to_string()));
    fm.insert("status".to_string(), serde_json::Value::String("active".to_string()));
    fm.insert("modified".to_string(), serde_json::Value::String(modified.to_string()));

    PkbDocument {
        path: PathBuf::from(path),
        title: title.to_string(),
        tags: tags.into_iter().map(String::from).collect(),
        doc_type: Some(doc_type.to_string()),
        status: Some("active".to_string()),
        consolidated: Some(false),
        consolidated_at: None,
        modified: Some(modified.to_string()),
        body: body.to_string(),
        content_hash: format!("chash_{id}"),
        file_hash: format!("hash_{id}"),
        frontmatter: Some(serde_json::Value::Object(fm)),
    }
}

#[test]
fn test_exact_pr_and_env_var_retrieval() {
    let mut store = VectorStore::new(3);
    let root = Path::new("/pkb");

    let doc1 = make_doc(
        "mem_7cc72ed8",
        "tasks/mem_7cc72ed8.md",
        "PKB search: add a lexical (BM25) layer",
        "task",
        "Add lexical retrieval fused via RRF. Reference PR #520 and env var AOPS_MODEL_PATH.",
        vec!["search", "bm25"],
        "2026-08-31",
    );

    let doc2 = make_doc(
        "note_other",
        "notes/note_other.md",
        "General Architecture",
        "note",
        "General notes on system design without specific keywords.",
        vec!["arch"],
        "2026-08-30",
    );

    let doc3 = make_doc(
        "task_function",
        "tasks/task_function.md",
        "Type filter implementation",
        "task",
        "Refactor matches_type_filter to support negation and comma lists.",
        vec!["refactor"],
        "2026-08-29",
    );

    store.insert_precomputed(
        &doc1,
        vec![doc1.body.clone()],
        vec![vec![0.1, 0.1, 0.1]],
    );
    store.insert_precomputed(
        &doc2,
        vec![doc2.body.clone()],
        vec![vec![0.9, 0.9, 0.9]],
    );
    store.insert_precomputed(
        &doc3,
        vec![doc3.body.clone()],
        vec![vec![0.2, 0.2, 0.2]],
    );

    // 1. Exact PR search
    let results_pr = store.search_hybrid(
        "PR #520",
        &[0.9, 0.9, 0.9], // Vector similarity biased toward doc2
        5,
        root,
        None,
        None,
        None,
        None,
    );
    assert!(!results_pr.is_empty());
    assert_eq!(results_pr[0].id, "mem_7cc72ed8");

    // 2. Exact env var search
    let results_env = store.search_hybrid(
        "AOPS_MODEL_PATH",
        &[0.9, 0.9, 0.9],
        5,
        root,
        None,
        None,
        None,
        None,
    );
    assert!(!results_env.is_empty());
    assert_eq!(results_env[0].id, "mem_7cc72ed8");

    // 3. Exact function name search
    let results_fn = store.search_hybrid(
        "matches_type_filter",
        &[0.9, 0.9, 0.9],
        5,
        root,
        None,
        None,
        None,
        None,
    );
    assert!(!results_fn.is_empty());
    assert_eq!(results_fn[0].id, "task_function");

    // 4. Bare task ID search
    let results_id = store.search_hybrid(
        "mem_7cc72ed8",
        &[0.9, 0.9, 0.9],
        5,
        root,
        None,
        None,
        None,
        None,
    );
    assert!(!results_id.is_empty());
    assert_eq!(results_id[0].id, "mem_7cc72ed8");
}

#[test]
fn test_hybrid_search_type_filter_and_negation() {
    let mut store = VectorStore::new(3);
    let root = Path::new("/pkb");

    let doc_task = make_doc(
        "t1",
        "tasks/t1.md",
        "Search indexing pipeline",
        "task",
        "Indexing details for search queries",
        vec!["search"],
        "2026-08-31",
    );
    let doc_note = make_doc(
        "n1",
        "notes/n1.md",
        "Search indexing discussion",
        "note",
        "Discussion notes about search indexing",
        vec!["search"],
        "2026-08-31",
    );

    store.insert_precomputed(&doc_task, vec![doc_task.body.clone()], vec![vec![1.0, 0.0, 0.0]]);
    store.insert_precomputed(&doc_note, vec![doc_note.body.clone()], vec![vec![1.0, 0.0, 0.0]]);

    // Positive filter: task only
    let tasks = store.search_hybrid(
        "indexing",
        &[1.0, 0.0, 0.0],
        5,
        root,
        None,
        None,
        Some("task"),
        None,
    );
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "t1");

    // Negative filter: !task (should return note only)
    let non_tasks = store.search_hybrid(
        "indexing",
        &[1.0, 0.0, 0.0],
        5,
        root,
        None,
        None,
        Some("!task"),
        None,
    );
    assert_eq!(non_tasks.len(), 1);
    assert_eq!(non_tasks[0].id, "n1");
}

#[test]
fn test_hybrid_search_incremental_updates() {
    let mut store = VectorStore::new(3);
    let root = Path::new("/pkb");

    let doc = make_doc(
        "doc_up",
        "tasks/doc_up.md",
        "Initial Title",
        "task",
        "Initial text with keyword ALPHA_KEYWORD",
        vec![],
        "2026-08-31",
    );

    store.insert_precomputed(&doc, vec![doc.body.clone()], vec![vec![1.0, 0.0, 0.0]]);

    let r1 = store.search_hybrid("ALPHA_KEYWORD", &[1.0, 0.0, 0.0], 5, root, None, None, None, None);
    assert_eq!(r1.len(), 1);
    assert_eq!(r1[0].id, "doc_up");

    // Remove document
    store.remove("doc_up");
    let r2 = store.search_hybrid("ALPHA_KEYWORD", &[1.0, 0.0, 0.0], 5, root, None, None, None, None);
    assert!(r2.is_empty());
}
