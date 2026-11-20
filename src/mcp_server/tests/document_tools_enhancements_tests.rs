use super::*;

fn make_server(root: &Path) -> PkbSearchServer {
    write_test_polecat_yaml(root);
    let docs: Vec<_> = crate::pkb::scan_directory_all(root)
        .into_iter()
        .filter_map(|p| crate::pkb::parse_file_relative(&p, root))
        .collect();
    let graph = GraphStore::build(&docs, root);
    let mut store = VectorStore::new(3);
    for doc in &docs {
        let emb = vec![vec![1.0, 0.0, 0.0]];
        let chunks = vec![doc.body.clone()];
        store.insert_precomputed(doc, chunks, emb);
    }
    let embedder = Embedder::new_dummy();
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    )
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    if let Some(ref structured) = res.structured_content {
        return structured.clone();
    }
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

#[test]
fn test_list_documents_filter_title_substring_and_date_window() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();

    // Doc 1: "Alpha Project Plan", modified May 2026
    std::fs::write(
        notes_dir.join("doc1.md"),
        "---\nid: doc-1\ntitle: Alpha Project Plan\ntype: note\nmodified: 2026-05-10T10:00:00Z\n---\nBody 1\n",
    ).unwrap();

    // Doc 2: "Alpha Design Spec", modified Aug 2026 (matches both title substring and date window)
    std::fs::write(
        notes_dir.join("doc2.md"),
        "---\nid: doc-2\ntitle: Alpha Design Spec\ntype: note\nmodified: 2026-08-15T12:00:00Z\n---\nBody 2\n",
    ).unwrap();

    // Doc 3: "Beta Research Notes", modified Aug 2026 (matches date window, but NOT title substring)
    std::fs::write(
        notes_dir.join("doc3.md"),
        "---\nid: doc-3\ntitle: Beta Research Notes\ntype: note\nmodified: 2026-08-20T12:00:00Z\n---\nBody 3\n",
    ).unwrap();

    // Doc 4: "Alpha Retrospective", modified Oct 2026 (matches title substring, but outside date window)
    std::fs::write(
        notes_dir.join("doc4.md"),
        "---\nid: doc-4\ntitle: Alpha Retrospective\ntype: note\nmodified: 2026-10-01T15:00:00Z\n---\nBody 4\n",
    ).unwrap();

    let server = make_server(root);

    let res = server.handle_list_documents(&json!({
        "title_contains": "alpha",
        "since": "2026-06-01",
        "before": "2026-09-01",
        "format": "json"
    })).expect("handle_list_documents should succeed");

    let out = json_of(&res);
    let items = out["documents"].as_array().expect("documents should be an array");
    assert_eq!(items.len(), 1, "Only doc-2 should match both filters: {items:?}");
    assert_eq!(items[0]["id"], "doc-2");
    assert_eq!(items[0]["title"], "Alpha Design Spec");
}

#[test]
fn test_list_documents_returns_real_modified_time_not_null() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();

    // Doc with frontmatter modified
    let p1 = notes_dir.join("doc_with_fm_mod.md");
    std::fs::write(
        &p1,
        "---\nid: doc-with-fm-mod\ntitle: Doc With FM Mod\ntype: note\nmodified: 2026-07-04T08:00:00Z\n---\nContent\n",
    ).unwrap();

    // Doc without frontmatter modified (relies on file modified time)
    let p2 = notes_dir.join("doc_no_fm_mod.md");
    std::fs::write(
        &p2,
        "---\nid: doc-no-fm-mod\ntitle: Doc No FM Mod\ntype: note\n---\nContent\n",
    ).unwrap();

    let server = make_server(root);

    // Test JSON mode
    let res_json = server.handle_list_documents(&json!({
        "format": "json"
    })).expect("handle_list_documents json mode");

    let out_json = json_of(&res_json);
    let items = out_json["documents"].as_array().expect("documents array");
    assert_eq!(items.len(), 2);

    for item in items {
        let mod_val = &item["modified"];
        assert!(
            mod_val.is_string(),
            "Document '{}' modified must be a string, got: {:?}",
            item["id"],
            mod_val
        );
        let s = mod_val.as_str().unwrap();
        assert!(!s.is_empty(), "Document '{}' modified must not be empty", item["id"]);
        // Verify parseable as date/timestamp
        assert!(
            crate::date_filter::parse_modified_instant(s).is_some(),
            "Document '{}' modified timestamp '{}' must be valid parseable date/time",
            item["id"],
            s
        );
    }

    // Verify Doc with FM has exactly the FM modified timestamp
    let fm_doc = items.iter().find(|i| i["id"] == "doc-with-fm-mod").unwrap();
    assert_eq!(fm_doc["modified"], "2026-07-04T08:00:00Z");

    // Test Markdown mode also carries non-null modified in structured payload
    let res_md = server.handle_list_documents(&json!({
        "format": "markdown"
    })).expect("handle_list_documents markdown mode");
    let out_md = json_of(&res_md);
    let md_items = out_md["documents"].as_array().expect("md documents array");
    for item in md_items {
        assert!(item["modified"].is_string());
    }
}

#[test]
fn test_delete_document_batch_reporting_outcomes_and_surviving_missing_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();

    let p1 = notes_dir.join("del1.md");
    std::fs::write(
        &p1,
        "---\nid: del-1\ntitle: Delete Me One\ntype: note\n---\nBody\n",
    ).unwrap();

    let p2 = notes_dir.join("del2.md");
    std::fs::write(
        &p2,
        "---\nid: del-2\ntitle: Delete Me Two\ntype: note\n---\nBody\n",
    ).unwrap();

    let server = make_server(root);

    // Call delete with a batch including a nonexistent ID in the middle
    let res = server.handle_delete_document(&json!({
        "ids": ["del-1", "nonexistent-doc-id", "del-2"]
    })).expect("batch delete should succeed and not crash on missing id");

    let out = json_of(&res);
    assert_eq!(out["deleted_count"], 2);
    assert_eq!(out["failed_count"], 1);

    let results = out["results"].as_array().expect("results array");
    assert_eq!(results.len(), 3);

    // First item: del-1 succeeded
    assert_eq!(results[0]["id"], "del-1");
    assert_eq!(results[0]["deleted"], true);
    assert!(!p1.exists(), "del1.md must be deleted from disk");

    // Second item: nonexistent failed without aborting del-2
    assert_eq!(results[1]["id"], "nonexistent-doc-id");
    assert_eq!(results[1]["deleted"], false);
    assert!(results[1]["error"].as_str().unwrap().contains("not found"));

    // Third item: del-2 succeeded
    assert_eq!(results[2]["id"], "del-2");
    assert_eq!(results[2]["deleted"], true);
    assert!(!p2.exists(), "del2.md must be deleted from disk");

    // Graph must have removed both del-1 and del-2
    let graph = server.graph.read();
    assert!(graph.resolve("del-1").is_none());
    assert!(graph.resolve("del-2").is_none());
}

#[test]
fn test_convert_document_rename_note_title_and_slug_remains_reachable_by_old_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();

    let old_path = notes_dir.join("note_123_original_title.md");
    std::fs::write(
        &old_path,
        "---\nid: note-123\ntitle: Original Note Title\ntype: note\n---\nValuable thoughts.\n",
    ).unwrap();

    let server = make_server(root);

    // Rename note title and slug using convert_document
    let res = server.handle_convert_document(&json!({
        "id": "note-123",
        "title": "Renamed Note Title",
        "slug": "renamed_slug"
    })).expect("handle_convert_document rename should succeed");

    let out = json_of(&res);
    assert_eq!(out["ok"], true);
    assert_eq!(out["id"], "note-123");
    assert_eq!(out["title"], "Renamed Note Title");
    assert_eq!(out["type"], "note");
    assert_eq!(out["moved"], true);

    let expected_new_rel = "notes/note-123_renamed_slug.md";
    assert_eq!(out["path"], expected_new_rel);

    // Check disk: old file is gone, new file exists with updated title and preserved id
    let new_path = root.join(expected_new_rel);
    assert!(!old_path.exists(), "Old path must no longer exist");
    assert!(new_path.exists(), "New path must exist");

    let new_content = std::fs::read_to_string(&new_path).unwrap();
    assert!(new_content.contains("title: Renamed Note Title") || new_content.contains("title: \"Renamed Note Title\""));
    assert!(new_content.contains("id: note-123"));
    assert!(new_content.contains("Valuable thoughts."));

    // Check that note stays reachable by its old id
    let get_res = server.handle_get_document(&json!({
        "id": "note-123"
    })).expect("Note must remain reachable by its old id 'note-123'");

    let get_out = json_of(&get_res);
    assert_eq!(get_out["id"], "note-123");
    assert_eq!(get_out["title"], "Renamed Note Title");
}
