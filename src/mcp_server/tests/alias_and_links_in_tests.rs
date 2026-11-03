use super::*;
use serde_json::json;
use std::sync::Arc;

fn make_test_server(root: &Path) -> PkbSearchServer {
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
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
fn test_ac1_get_document_single_by_alias_returns_canonical_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    // Create a note file on disk with an alias
    let note_content = "\
---
id: note-canon-1
title: Canonical Note One
type: note
alias:
  - my-alias-note
  - secondary-alias
---
# Canonical Note One
This is the body.
";
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("note-canon-1.md"), note_content).unwrap();

    // Refresh graph so it indexes the new file
    server.handle_refresh_graph(&json!({})).unwrap();

    // 1. Single read by alias
    let res = server
        .handle_get_document(&json!({"id": "my-alias-note"}))
        .expect("get_document by alias must succeed");

    let structured = res.structured_content.expect("must return structured content");
    assert_eq!(
        structured["id"], "note-canon-1",
        "AC1: Single get_document by alias must return canonical ID"
    );
    assert_eq!(structured["title"], "Canonical Note One");
}

#[test]
fn test_ac1_get_document_batch_by_alias_returns_canonical_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let note_content = "\
---
id: note-canon-batch
title: Canonical Note Batch
type: note
aliases:
  - batch-alias-note
---
# Body
";
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("note-canon-batch.md"), note_content).unwrap();

    server.handle_refresh_graph(&json!({})).unwrap();

    // Batch read by alias
    let res = server
        .handle_get_document(&json!({"ids": ["batch-alias-note"]}))
        .expect("batch get_document by alias must succeed");

    let structured = res.structured_content.expect("must return structured content");
    let items = structured["items"].as_array().expect("items must be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["id"], "note-canon-batch",
        "AC1: Batch get_document by alias must return canonical ID"
    );
}

#[test]
fn test_ac1_get_task_single_and_batch_by_alias_returns_canonical_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let task_content = "\
---
id: task-canon-alias
title: Canonical Task with Alias
type: task
status: ready
project: proj-test
alias: task-shortcut-alias
---
# Body
";
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(tasks_dir.join("task-canon-alias.md"), task_content).unwrap();

    server.handle_refresh_graph(&json!({})).unwrap();

    // 1. Single get_task by alias
    let single_res = server
        .handle_get_task(&json!({"id": "task-shortcut-alias"}))
        .expect("get_task by alias must succeed");
    let single_val = single_res.structured_content.expect("must return structured content");
    assert_eq!(
        single_val["id"], "task-canon-alias",
        "AC1: Single get_task by alias must return canonical ID"
    );

    // 2. Batch get_task by alias
    let batch_res = server
        .handle_get_task(&json!({"ids": ["task-shortcut-alias"]}))
        .expect("batch get_task by alias must succeed");
    let batch_val = batch_res.structured_content.expect("must return structured content");
    let items = batch_val["items"].as_array().expect("items must be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["id"], "task-canon-alias",
        "AC1: Batch get_task by alias must return canonical ID"
    );
}

#[test]
fn test_ac2_get_document_note_with_inbound_wikilinks_lists_source_in_links_in() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let target_note = "\
---
id: note-target-wikilink
title: Target Note
type: note
---
# Target Note Content
";
    let source_doc = "\
---
id: doc-source-wikilink
title: Source Document
type: note
---
# Source references [[note-target-wikilink]]
";
    let notes_dir = root.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("note-target-wikilink.md"), target_note).unwrap();
    std::fs::write(notes_dir.join("doc-source-wikilink.md"), source_doc).unwrap();

    server.handle_refresh_graph(&json!({})).unwrap();

    // Read target note via get_document
    let res = server
        .handle_get_document(&json!({"id": "note-target-wikilink"}))
        .expect("get_document should succeed");
    let val = res.structured_content.expect("must return structured content");

    let links_in = val
        .get("links_in")
        .and_then(|v| v.as_array())
        .expect("AC2: get_document on note with inbound wikilinks must return links_in array");

    assert!(
        !links_in.is_empty(),
        "AC2: links_in must not be empty when inbound wikilinks exist"
    );
    let has_source = links_in.iter().any(|link| {
        link.get("from").and_then(|f| f.as_str()) == Some("doc-source-wikilink")
            || link.get("id").and_then(|id| id.as_str()) == Some("doc-source-wikilink")
    });
    assert!(
        has_source,
        "AC2: links_in must list source doc 'doc-source-wikilink', got: {links_in:?}"
    );

    // Also test batch get_document
    let batch_res = server
        .handle_get_document(&json!({"ids": ["note-target-wikilink"]}))
        .expect("batch get_document should succeed");
    let batch_val = batch_res.structured_content.unwrap();
    let batch_items = batch_val["items"].as_array().unwrap();
    let batch_links_in = batch_items[0]
        .get("links_in")
        .and_then(|v| v.as_array())
        .expect("AC2: batch get_document must return links_in");
    assert!(batch_links_in.iter().any(|link| {
        link.get("from").and_then(|f| f.as_str()) == Some("doc-source-wikilink")
            || link.get("id").and_then(|id| id.as_str()) == Some("doc-source-wikilink")
    }));
}

#[test]
fn test_ac2_get_document_note_with_inbound_edges_lists_source_in_links_in() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_test_server(root);

    let target_note = "\
---
id: note-dep-target
title: Dependency Target Note
type: note
---
# Architecture Note
";
    let source_task = "\
---
id: task-dep-source
title: Task Depending on Note
type: task
status: ready
project: proj-test
depends_on:
  - note-dep-target
---
# Task Body
";
    let notes_dir = root.join("notes");
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(notes_dir.join("note-dep-target.md"), target_note).unwrap();
    std::fs::write(tasks_dir.join("task-dep-source.md"), source_task).unwrap();

    server.handle_refresh_graph(&json!({})).unwrap();

    let res = server
        .handle_get_document(&json!({"id": "note-dep-target"}))
        .expect("get_document should succeed");
    let val = res.structured_content.expect("must return structured content");

    let links_in = val
        .get("links_in")
        .and_then(|v| v.as_array())
        .expect("AC2: get_document on note with inbound edges must return links_in array");

    assert!(
        links_in.iter().any(|link| {
            link.get("from").and_then(|f| f.as_str()) == Some("task-dep-source")
                || link.get("id").and_then(|id| id.as_str()) == Some("task-dep-source")
        }),
        "AC2: links_in must list source task 'task-dep-source', got: {links_in:?}"
    );
}
