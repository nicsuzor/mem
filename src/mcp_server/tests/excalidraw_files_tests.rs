//! `mem_24d027d4`: `.excalidraw` canvases in the PKB are listed, read and
//! written over MCP by PKB-relative path, and never enter the markdown scan
//! that feeds the graph, BM25 and vector index. Exercised through the real
//! MCP handlers.

use super::*;

fn make_server(root: &Path) -> PkbSearchServer {
    write_test_polecat_yaml(root);
    let graph = GraphStore::build_from_directory(root);
    PkbSearchServer::new(
        Arc::new(RwLock::new(VectorStore::new(3))),
        Arc::new(Embedder::new_dummy()),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    )
}

fn text_of(res: &CallToolResult) -> String {
    res.content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect::<String>()
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    let text = text_of(res);
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

/// A minimal canvas holding one rectangle with bound text `label`.
fn canvas(label: &str) -> String {
    json!({
        "type": "excalidraw",
        "version": 2,
        "source": "test",
        "elements": [
            {
                "id": "box1", "type": "rectangle", "x": 0, "y": 0, "width": 200, "height": 80,
                "boundElements": [{ "id": "txt1", "type": "text" }]
            },
            {
                "id": "txt1", "type": "text", "x": 10, "y": 30, "width": 180, "height": 20,
                "text": label, "originalText": label, "containerId": "box1"
            }
        ],
        "appState": {},
        "files": {}
    })
    .to_string()
}

fn write_file(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn listed_paths(res: &CallToolResult) -> Vec<String> {
    json_of(res)["canvases"]
        .as_array()
        .expect("canvases array")
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect()
}

// ── list ─────────────────────────────────────────────────────────────────

#[test]
fn list_returns_canvases_only_sorted_with_size() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "knowledge/framework/b.excalidraw", &canvas("B"));
    write_file(root, "a.excalidraw", &canvas("A"));
    write_file(root, "knowledge/note.md", "---\nid: n1\ntitle: N\n---\nbody\n");
    write_file(root, "lib/parts.excalidrawlib", "{}");
    write_file(root, ".hidden/secret.excalidraw", &canvas("H"));
    let server = make_server(root);

    let res = server.handle_list_excalidraw(&json!({})).unwrap();
    assert_eq!(
        listed_paths(&res),
        vec!["a.excalidraw", "knowledge/framework/b.excalidraw"]
    );
    let first = &json_of(&res)["canvases"][0];
    assert_eq!(
        first["bytes"].as_u64().unwrap(),
        canvas("A").len() as u64,
        "bytes must be the on-disk size"
    );
    assert!(first["modified"].is_string(), "modified timestamp: {first}");
}

#[test]
fn list_filters_by_directory_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "knowledge/framework/b.excalidraw", &canvas("B"));
    write_file(root, "knowledge/frameworks-old/c.excalidraw", &canvas("C"));
    write_file(root, "a.excalidraw", &canvas("A"));
    let server = make_server(root);

    let res = server
        .handle_list_excalidraw(&json!({ "dir": "knowledge/framework" }))
        .unwrap();
    assert_eq!(listed_paths(&res), vec!["knowledge/framework/b.excalidraw"]);
}

#[test]
fn list_rejects_dir_escaping_the_pkb() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let err = server
        .handle_list_excalidraw(&json!({ "dir": "../elsewhere" }))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{}", err.message);
}

// ── read ─────────────────────────────────────────────────────────────────

#[test]
fn get_returns_file_contents_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let body = canvas("verbatim label");
    write_file(root, "knowledge/framework/x.excalidraw", &body);
    let server = make_server(root);

    let res = server
        .handle_get_excalidraw(&json!({ "path": "knowledge/framework/x.excalidraw" }))
        .unwrap();
    assert_eq!(text_of(&res), body);
}

#[test]
fn get_rejects_missing_traversal_and_non_canvas_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "note.md", "---\nid: n1\n---\n");
    let outside = tmp.path().parent().unwrap().join("outside.excalidraw");
    let server = make_server(root);

    for bad in [
        "missing.excalidraw".to_string(),
        "note.md".to_string(),
        "../outside.excalidraw".to_string(),
        outside.display().to_string(),
        ".hidden/x.excalidraw".to_string(),
    ] {
        let err = server
            .handle_get_excalidraw(&json!({ "path": bad }))
            .expect_err(&format!("{bad} must be rejected"));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{bad}: {}", err.message);
    }
}

// ── write ────────────────────────────────────────────────────────────────

#[test]
fn write_creates_new_canvas_and_parent_dirs_then_reads_back() {
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let body = canvas("fresh");

    let res = server
        .handle_write_excalidraw(&json!({ "path": "sketches/new/fresh.excalidraw", "content": body }))
        .unwrap();
    let out = json_of(&res);
    assert_eq!(out["path"], "sketches/new/fresh.excalidraw");
    assert_eq!(out["created"], true);
    assert_eq!(out["bytes"].as_u64().unwrap(), body.len() as u64);

    let back = server
        .handle_get_excalidraw(&json!({ "path": "sketches/new/fresh.excalidraw" }))
        .unwrap();
    assert_eq!(text_of(&back), body);
}

#[test]
fn write_overwrites_existing_canvas() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "c.excalidraw", &canvas("old"));
    let server = make_server(root);

    let res = server
        .handle_write_excalidraw(&json!({ "path": "c.excalidraw", "content": canvas("new") }))
        .unwrap();
    assert_eq!(json_of(&res)["created"], false);
    assert_eq!(
        std::fs::read_to_string(root.join("c.excalidraw")).unwrap(),
        canvas("new")
    );
}

#[test]
fn write_rejects_content_that_is_not_an_excalidraw_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "c.excalidraw", &canvas("keep me"));
    let server = make_server(root);

    for bad in ["not json", "{}", r#"{"type":"excalidraw"}"#, r#"[1,2]"#] {
        let err = server
            .handle_write_excalidraw(&json!({ "path": "c.excalidraw", "content": bad }))
            .expect_err(&format!("{bad} must be rejected"));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{bad}: {}", err.message);
    }
    assert_eq!(
        std::fs::read_to_string(root.join("c.excalidraw")).unwrap(),
        canvas("keep me"),
        "a rejected write must leave the file untouched"
    );
}

#[test]
fn write_rejects_paths_outside_pkb_or_without_canvas_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pkb");
    std::fs::create_dir_all(&root).unwrap();
    let server = make_server(&root);

    for bad in ["../escape.excalidraw", "notes/x.md", "x.excalidraw.json", ".git/x.excalidraw"] {
        let err = server
            .handle_write_excalidraw(&json!({ "path": bad, "content": canvas("x") }))
            .expect_err(&format!("{bad} must be rejected"));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{bad}: {}", err.message);
    }
    assert!(!tmp.path().join("escape.excalidraw").exists());
    assert!(!root.join("notes/x.md").exists());
}

// ── not indexed ──────────────────────────────────────────────────────────

#[test]
fn canvas_written_over_mcp_is_not_indexed_or_searchable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "note.md", "---\nid: n1\ntitle: Plain note\n---\nordinary words\n");
    let server = make_server(root);
    server
        .handle_write_excalidraw(&json!({
            "path": "zanzibarquokka.excalidraw",
            "content": canvas("zanzibarquokka unique label"),
        }))
        .unwrap();

    // A full rebuild from disk must not pick the canvas up.
    server.handle_refresh_graph(&json!({})).unwrap();
    assert!(server.graph.read().resolve("zanzibarquokka").is_none());
    let scanned: Vec<_> = crate::pkb::scan_directory(root);
    assert!(scanned.iter().all(|p| p.extension().unwrap() == "md"), "{scanned:?}");

    let res = server
        .handle_pkb_search(&json!({ "query": "zanzibarquokka", "format": "json" }))
        .unwrap();
    assert!(
        !text_of(&res).contains("zanzibarquokka.excalidraw"),
        "search must not surface canvas JSON: {}",
        text_of(&res)
    );
}

#[test]
fn write_reports_warning_count_and_caps_the_listed_warnings() {
    // One box listing `n` arrows in boundElements that do not bind back:
    // each is a non-blocking "stale boundElements" warning.
    let tmp = tempfile::tempdir().unwrap();
    let server = make_server(tmp.path());
    let n = 14;
    let mut elements = vec![json!({
        "id": "box", "type": "rectangle", "x": 0, "y": 0, "width": 100, "height": 50,
        "boundElements": (0..n).map(|i| json!({ "id": format!("arr{i}"), "type": "arrow" })).collect::<Vec<_>>()
    })];
    for i in 0..n {
        elements.push(json!({
            "id": format!("arr{i}"), "type": "arrow", "x": 200, "y": i * 10,
            "width": 50, "height": 0, "points": [[0, 0], [50, 0]]
        }));
    }
    let content = json!({ "type": "excalidraw", "version": 2, "elements": elements }).to_string();

    let out = json_of(
        &server
            .handle_write_excalidraw(&json!({ "path": "stale.excalidraw", "content": content }))
            .unwrap(),
    );
    assert_eq!(out["warning_count"].as_u64().unwrap(), n as u64, "{out}");
    let listed = out["warnings"].as_array().unwrap();
    assert!(listed.len() < n, "warnings list must be capped: {}", listed.len());
    assert!(!listed.is_empty());
}
