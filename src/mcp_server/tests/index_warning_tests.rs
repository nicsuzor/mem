//! Regression tests for aops_mem_remove_index_disk_warning: list/search tool
//! responses must not carry an "index disagrees with disk" warning, even when
//! the in-memory index and disk really do disagree. The node/file counts stay
//! available through `status`.

use super::*;

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
        .collect()
}

fn assert_no_index_warning(surface: &str, text: &str) {
    for needle in ["disagrees with disk", "index_warning", "may be wrong", "refresh_graph and retry"] {
        assert!(
            !text.contains(needle),
            "{surface} must not carry an index/disk warning (found {needle:?}): {text}"
        );
    }
}

/// A server whose in-memory index holds one task while disk holds two
/// files, with the generation stamp pinned so `ensure_graph_fresh` does
/// not rebuild — the state that used to trigger the warning.
fn server_with_index_disk_disagreement() -> (tempfile::TempDir, PkbSearchServer) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(&root);

    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(VectorStore::new(3))),
        Arc::new(Embedder::new_dummy()),
        root.clone(),
        root.join("db"),
        Arc::new(RwLock::new(GraphStore::build(&[], &root))),
    );
    server
        .handle_create_task(&json!({
            "title": "Indexed task",
            "type": "task",
            "project": "proj-test",
            "parent": "proj-test",
            "allow_missing_parent": true,
        }))
        .unwrap();

    std::fs::write(
        root.join("tasks/unindexed.md"),
        "---\nid: unindexed\ntitle: Unindexed\ntype: task\nstatus: ready\nproject: proj-test\n---\n\nBody.\n",
    )
    .unwrap();
    let gen = crate::pkb::scan_generation(&root);
    server.graph.write().set_generation(gen);

    let (disk, indexed) = server.index_disk_counts();
    assert_ne!(disk, indexed, "fixture must reproduce an index/disk disagreement");
    (tmp, server)
}

#[test]
fn list_tasks_carries_no_index_disk_warning_in_any_format() {
    let (_tmp, server) = server_with_index_disk_disagreement();
    for format in ["markdown", "json", "tree", "nested_json"] {
        let res = server.handle_list_tasks(&json!({"format": format})).unwrap();
        assert_no_index_warning(&format!("list_tasks(format={format})"), &text_of(&res));
    }
}

#[test]
fn list_tasks_empty_result_carries_no_index_disk_warning() {
    let (_tmp, server) = server_with_index_disk_disagreement();
    let res = server
        .handle_list_tasks(&json!({"project": "no-such-project"}))
        .unwrap();
    let text = text_of(&res);
    assert!(text.starts_with("No tasks found"), "expected empty result: {text}");
    assert_no_index_warning("list_tasks(empty)", &text);
}

#[test]
fn task_summary_list_documents_and_search_carry_no_index_disk_warning() {
    let (_tmp, server) = server_with_index_disk_disagreement();
    let res = server.handle_task_summary(&json!({})).unwrap();
    assert_no_index_warning("task_summary", &text_of(&res));
    for format in ["markdown", "json"] {
        let res = server.handle_list_documents(&json!({"format": format})).unwrap();
        assert_no_index_warning(&format!("list_documents(format={format})"), &text_of(&res));
    }
    let res = server.handle_pkb_search(&json!({"query": "indexed task"})).unwrap();
    assert_no_index_warning("search", &text_of(&res));
}

#[test]
fn status_reports_node_and_disk_file_counts() {
    let (_tmp, server) = server_with_index_disk_disagreement();
    let text = text_of(&server.handle_status(&json!({})).unwrap());
    let status: serde_json::Value = serde_json::from_str(&text).unwrap();
    let index = status.get("index").expect("index section");
    let disk = crate::pkb::scan_directory(&server.pkb_root).len() as u64;
    assert_eq!(index.get("disk_file_count").and_then(|v| v.as_u64()), Some(disk), "{status}");
    assert_eq!(index.get("indexed_file_count").and_then(|v| v.as_u64()), Some(1), "{status}");
    assert!(index.get("document_count").and_then(|v| v.as_u64()).is_some(), "{status}");
}
