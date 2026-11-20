//! Integration tests for ID-only PKB addressing, no permalink/ID-alias emission,
//! optional slug with discouraged filename, and convert_document / status round-trip (#847, #848).

use mem::document_crud::{
    convert_document, create_document, create_memory, create_task, ensure_adhoc_sessions_root,
    DocumentFields, MemoryFields, TaskFields,
};
use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_create_document_task_memory_adhoc_omit_permalink_and_id_aliases() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let doc_path = create_document(
        root,
        DocumentFields {
            title: "My Design Note".to_string(),
            doc_type: "note".to_string(),
            slug: None,
            ..Default::default()
        },
    )
    .unwrap();
    let doc_text = fs::read_to_string(&doc_path).unwrap();
    assert!(
        !doc_text.contains("permalink:"),
        "create_document must not write permalink: {doc_text}"
    );
    assert!(
        !doc_text.contains("alias:"),
        "create_document must not write auto-generated alias: {doc_text}"
    );

    let task_path = create_task(
        root,
        TaskFields {
            title: "My Test Task".to_string(),
            slug: None,
            ..Default::default()
        },
    )
    .unwrap();
    let task_text = fs::read_to_string(&task_path).unwrap();
    assert!(
        !task_text.contains("permalink:"),
        "create_task must not write permalink: {task_text}"
    );
    assert!(
        !task_text.contains("alias:"),
        "create_task must not write auto-generated alias: {task_text}"
    );

    let mem_path = create_memory(
        root,
        MemoryFields {
            title: "Remembered Fact".to_string(),
            slug: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mem_text = fs::read_to_string(&mem_path).unwrap();
    assert!(
        !mem_text.contains("permalink:"),
        "create_memory must not write permalink: {mem_text}"
    );
    assert!(
        !mem_text.contains("alias:"),
        "create_memory must not write auto-generated alias: {mem_text}"
    );

    ensure_adhoc_sessions_root(root).unwrap();
    let adhoc_text = fs::read_to_string(root.join("projects/adhoc-sessions.md")).unwrap();
    assert!(
        !adhoc_text.contains("permalink:"),
        "ensure_adhoc_sessions_root must not write permalink: {adhoc_text}"
    );
    assert!(
        !adhoc_text.contains("alias:"),
        "ensure_adhoc_sessions_root must not write auto-generated alias: {adhoc_text}"
    );
}

#[test]
fn test_convert_document_move_does_not_write_old_stem_alias() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let doc_path = create_document(
        root,
        DocumentFields {
            id: Some("note_conv1234".to_string()),
            title: "Original Note".to_string(),
            doc_type: "note".to_string(),
            slug: Some("orig".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    let res = convert_document(
        root,
        &doc_path,
        "note_conv1234",
        "Original Note",
        None,
        Some("Renamed Note"),
        Some("renamed"),
        Some("reviews/sub"),
        None,
    )
    .unwrap();
    assert!(res.moved);
    let moved_text = fs::read_to_string(&res.new_path).unwrap();
    assert!(
        !moved_text.contains("alias:"),
        "convert_document move must not write old_stem or id into alias: {moved_text}"
    );
    assert!(
        !moved_text.contains("permalink:"),
        "convert_document move must not write permalink: {moved_text}"
    );
}

#[test]
fn test_schemas_slug_optional_filename_discouraged_and_id_only_descriptions() {
    let tools = mem::mcp_server::PkbSearchServer::published_tools();
    let by_name: HashMap<&str, _> = tools.iter().map(|t| (t.name.as_ref(), t)).collect();

    for name in ["create", "create_task"] {
        let tool = by_name.get(name).unwrap();
        let req = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            !req.iter().any(|v| v.as_str() == Some("slug")),
            "{name} must not require slug: {req:?}"
        );
        let fn_desc = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.get("filename"))
            .and_then(|f| f.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        assert!(
            fn_desc.to_lowercase().contains("discouraged")
                || fn_desc.to_lowercase().contains("automatic"),
            "{name}.filename description must discourage client-supplied filenames: {fn_desc:?}"
        );
    }

    for name in [
        "get_document",
        "get_task_status",
        "convert_document",
        "release_task",
        "pkb_trace",
        "graph_excalidraw",
        "export_graph",
    ] {
        let tool = mem::mcp_server::PkbSearchServer::lookup_tool(name)
            .unwrap_or_else(|| panic!("missing tool in lookup_tool: {name}"));
        let serialized = serde_json::to_string(&tool).unwrap().to_lowercase();
        assert!(
            !serialized.contains("filename stem"),
            "{name} schema must not advertise filename stem resolution: {serialized}"
        );
    }
}

fn setup_test_server_with_files(files: &[(&str, &str)]) -> (PkbSearchServer, TempDir) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::create_dir_all(root.join("tasks")).unwrap();
    for (rel, content) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
    }
    let db_path = root.join("pkb_vectors.bin");
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());
    let docs: Vec<_> = mem::pkb::scan_directory(root)
        .iter()
        .filter_map(|p| mem::pkb::parse_file_relative(p, root))
        .collect();
    let graph = Arc::new(RwLock::new(GraphStore::build(&docs, root)));
    let server = PkbSearchServer::new(store, embedder, root.to_path_buf(), db_path, graph);
    (server, tmp)
}

#[test]
fn test_crud_rejects_path_stem_and_title_resolves_only_by_id_and_alias() {
    let (server, _tmp) = setup_test_server_with_files(&[(
        "notes/note_idonly1_my_unique_slug.md",
        "---\nid: note_idonly1\ntitle: \"Unique Note Title\"\ntype: note\nalias:\n  - user-reorg-alias\n---\n\nBody text.\n",
    )]);

    // Canonical ID succeeds
    let res = server.dispatch_tool_sync(
        "get_document",
        &json!({ "id": "note_idonly1", "session_id": "test-sess" }),
    );
    assert!(res.is_ok(), "get_document by ID must succeed: {res:?}");

    // Explicit user-reorganisation alias succeeds
    let res_alias = server.dispatch_tool_sync(
        "get_document",
        &json!({ "id": "user-reorg-alias", "session_id": "test-sess" }),
    );
    assert!(
        res_alias.is_ok(),
        "get_document by explicit user alias must succeed: {res_alias:?}"
    );

    // Path, filename stem, and title must be rejected
    for bad_query in [
        "notes/note_idonly1_my_unique_slug.md",
        "note_idonly1_my_unique_slug",
        "Unique Note Title",
    ] {
        let err = server.dispatch_tool_sync(
            "get_document",
            &json!({ "id": bad_query, "session_id": "test-sess" }),
        );
        assert!(
            err.is_err(),
            "get_document must reject non-ID query '{bad_query}', got: {err:?}"
        );
    }
}

#[test]
fn test_convert_document_move_to_subdir_readable_and_writable_by_id() {
    let (server, _tmp) = setup_test_server_with_files(&[]);

    let created = server
        .dispatch_tool_sync(
            "create",
            &json!({
                "title": "Movable Note",
                "type": "note",
                "id": "note_move847",
                "body": "Initial body.",
                "session_id": "test-sess"
            }),
        )
        .unwrap();
    assert!(!created.is_error.unwrap_or(false));

    let converted = server
        .dispatch_tool_sync(
            "convert_document",
            &json!({
                "id": "note_move847",
                "dir": "reviews/sub",
                "slug": "moved-slug",
                "title": "Moved Note",
                "session_id": "test-sess"
            }),
        )
        .unwrap();
    assert!(!converted.is_error.unwrap_or(false));

    // Immediately read and write by ID (#847)
    let fetched = server
        .dispatch_tool_sync(
            "get_document",
            &json!({ "id": "note_move847", "session_id": "test-sess" }),
        )
        .unwrap();
    assert!(!fetched.is_error.unwrap_or(false));

    let updated = server
        .dispatch_tool_sync(
            "update_body",
            &json!({
                "id": "note_move847",
                "new_body": "Updated after move.",
                "session_id": "test-sess"
            }),
        )
        .unwrap();
    assert!(!updated.is_error.unwrap_or(false));

    // Status reports 0 unlinked nodes (#848)
    let status = server
        .dispatch_tool_sync("status", &json!({ "session_id": "test-sess" }))
        .unwrap();
    let status_text = status.content[0].as_text().unwrap().text.clone();
    let status_json: serde_json::Value = serde_json::from_str(&status_text).unwrap();
    assert_eq!(status_json["index"]["unlinked_node_count"], 0);
}

#[test]
fn test_lookup_miss_triggers_debounced_id_reindex_and_retries_once() {
    let (server, tmp) = setup_test_server_with_files(&[]);
    server.set_miss_reindex_debounce_ms(10);

    // Write a file directly to disk without updating GraphStore
    fs::write(
        tmp.path().join("notes/note_ext123_external.md"),
        "---\nid: note_ext123\ntitle: \"External Note\"\ntype: note\n---\n\nCreated externally.\n",
    )
    .unwrap();

    // Lookup succeeds after graph refresh/reindex
    let res = server
        .dispatch_tool_sync(
            "get_document",
            &json!({ "id": "note_ext123", "session_id": "test-sess" }),
        )
        .unwrap();
    assert!(!res.is_error.unwrap_or(false));
    assert!(server.rebuild_count() >= 1);

    // Non-existent ID triggers debounced miss reindex once and fails fast (no fallback)
    let before = server.miss_reindex_count();
    let missing = server.dispatch_tool_sync(
        "get_document",
        &json!({ "id": "note_does_not_exist", "session_id": "test-sess" }),
    );
    assert!(missing.is_err());
    assert_eq!(server.miss_reindex_count(), before + 1);
}

#[test]
fn test_bulk_operation_defers_miss_reindex_and_triggers_at_most_one() {
    let (server, _tmp) = setup_test_server_with_files(&[]);
    server.set_miss_reindex_debounce_ms(10);

    {
        let _guard = server.begin_bulk_op();
        for missing_id in ["miss_1", "miss_2", "miss_3"] {
            let err = server.dispatch_tool_sync(
                "get_document",
                &json!({ "id": missing_id, "session_id": "test-sess" }),
            );
            assert!(err.is_err());
        }
        // Deferred while bulk op is active
        assert_eq!(server.miss_reindex_count(), 0);
    }

    // At most one reindex triggered when bulk op finishes
    assert_eq!(server.miss_reindex_count(), 1);
}

#[test]
fn test_migrate_clean_aliases_strips_permalink_and_id_aliases_rewrites_refs() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::create_dir_all(root.join("tasks")).unwrap();

    fs::write(
        root.join("notes/note_a_old_slug.md"),
        "---\nid: note_a\ntitle: \"Note A\"\ntype: note\npermalink: note_a\nalias:\n  - \"note_a_old_slug\"\n  - \"note_a\"\n  - \"note_a_another\"\n  - \"user-reorg-tag\"\n---\n\nBody A.\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/note_b_auto.md"),
        "---\nid: note_b\ntitle: \"Note B\"\ntype: note\npermalink: note_b\nalias:\n  - \"note_b_auto\"\n  - \"note_b\"\n---\n\nBody B.\n",
    )
    .unwrap();
    fs::write(
        root.join("tasks/task_c_ref.md"),
        "---\nid: task_c\ntitle: \"Task C\"\ntype: task\nstatus: inbox\nparent: note_a_old_slug\ndepends_on:\n  - note_b_auto\n---\n\nSee [[note_a_another]] and [[note_b_auto|B]].\n",
    )
    .unwrap();

    let dry = mem::migrations::migrate_clean_aliases(root, true).unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.files_changed, 3);

    let report = mem::migrations::migrate_clean_aliases(root, false).unwrap();
    assert!(!report.dry_run);
    assert_eq!(report.files_changed, 3);

    let note_a = fs::read_to_string(root.join("notes/note_a_old_slug.md")).unwrap();
    assert!(
        !note_a.contains("permalink:"),
        "note_a must drop permalink: {note_a}"
    );
    assert!(
        !note_a.contains("note_a_old_slug"),
        "note_a must drop filename alias: {note_a}"
    );
    assert!(
        !note_a.contains("note_a_another"),
        "note_a must drop id-prefixed alias: {note_a}"
    );
    assert!(
        note_a.contains("user-reorg-tag"),
        "note_a must preserve user-reorg alias: {note_a}"
    );

    let note_b = fs::read_to_string(root.join("notes/note_b_auto.md")).unwrap();
    assert!(
        !note_b.contains("permalink:"),
        "note_b must drop permalink: {note_b}"
    );
    assert!(
        !note_b.contains("alias:"),
        "note_b must drop empty alias: {note_b}"
    );

    let task_c = fs::read_to_string(root.join("tasks/task_c_ref.md")).unwrap();
    assert!(
        task_c.contains("parent: note_a\n") || task_c.contains("parent: \"note_a\""),
        "task_c parent must be canonical id: {task_c}"
    );
    assert!(
        task_c.contains("[[note_a]]"),
        "task_c wikilink must be canonical id: {task_c}"
    );
    assert!(
        task_c.contains("[[note_b|B]]"),
        "task_c labeled wikilink must be canonical id: {task_c}"
    );

    // Idempotent on second run
    let second = mem::migrations::migrate_clean_aliases(root, false).unwrap();
    assert_eq!(second.files_changed, 0);
}
