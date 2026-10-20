use super::*;
use std::fs;

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

fn frontmatter(path: &Path) -> serde_json::Value {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;
    let content = fs::read_to_string(path).unwrap();
    Matter::<YAML>::new()
        .parse(&content)
        .data
        .unwrap()
        .deserialize()
        .unwrap()
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

#[test]
fn test_create_node_overlong_title_succeeds_keeps_full_title_and_bounds_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // Title well over the 80-character filename maximum (165 characters)
    let long_title = "This is an extremely long task title well over the eighty character filename maximum limit designed to verify that PKB keeps the full title while bounding the filename";
    assert!(long_title.chars().count() > 80);

    let res = server
        .handle_create_task(&json!({
            "title": long_title,
            "project": "aops",
            "type": "task",
        }))
        .expect("creating a node with a title well over filename maximum must succeed");

    let out = json_of(&res);
    let task_id = out["id"].as_str().expect("id in response");

    let aops_dir = root.join("aops");
    let mut files = fs::read_dir(&aops_dir)
        .expect("aops directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one task file must be created");

    let filename = files.pop().unwrap();
    let file_path = aops_dir.join(&filename);
    assert!(
        filename.chars().count() <= crate::document_crud::MAX_FILENAME_LEN,
        "derived filename '{}' (length {}) must be <= MAX_FILENAME_LEN ({})",
        filename,
        filename.chars().count(),
        crate::document_crud::MAX_FILENAME_LEN
    );

    let fm = frontmatter(&file_path);
    assert_eq!(
        fm["title"].as_str().unwrap(),
        long_title,
        "full un-truncated title must be preserved in frontmatter"
    );
    assert_eq!(fm["id"].as_str().unwrap(), task_id);
}

#[test]
fn test_rename_node_overlong_title_succeeds_keeps_full_title_and_bounds_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let capture_id = "capture-001";
    let notes_dir = root.join("notes/mobile-captures");
    fs::create_dir_all(&notes_dir).unwrap();
    let old_path = notes_dir.join(format!("{capture_id}.md"));

    // Title well over the 80-character filename maximum (163 characters)
    let long_title = "This is an extremely long capture title well over the eighty character filename maximum limit designed to verify that convert_document keeps the full title on rename";
    assert!(long_title.chars().count() > 80);

    fs::write(
        &old_path,
        format!("---\nid: {capture_id}\ntitle: \"{long_title}\"\ntype: note\n---\n\nInitial capture body.\n"),
    )
    .unwrap();

    let server = make_server(root);

    let res = server
        .handle_convert_document(&json!({
            "id": capture_id,
            "type": "task",
            "status": "inbox",
        }))
        .expect("converting/renaming a node with an over-long title must succeed");

    let out = json_of(&res);
    assert_eq!(out["ok"], true);
    assert!(!old_path.exists(), "original file must be moved/renamed");

    let new_path = root.join(out["path"].as_str().unwrap());
    assert!(new_path.exists(), "renamed file must exist at new path");

    let filename = new_path.file_name().unwrap().to_str().unwrap();
    assert!(
        filename.chars().count() <= crate::document_crud::MAX_FILENAME_LEN,
        "renamed filename '{}' (length {}) must be <= MAX_FILENAME_LEN ({})",
        filename,
        filename.chars().count(),
        crate::document_crud::MAX_FILENAME_LEN
    );

    let fm = frontmatter(&new_path);
    assert_eq!(
        fm["title"].as_str().unwrap(),
        long_title,
        "full un-truncated title must be kept after rename"
    );
    assert_eq!(fm["type"].as_str().unwrap(), "task");
}

#[test]
fn test_explicit_over_maximum_filename_returns_error_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // Explicit filename supplied by client exceeding MAX_FILENAME_LEN (80)
    let overlong_filename = format!("{}.md", "a".repeat(85));
    assert!(overlong_filename.chars().count() > 80);

    let err = server
        .handle_create_task(&json!({
            "title": "Short title",
            "project": "aops",
            "filename": overlong_filename,
        }))
        .expect_err("explicit over-maximum filename must be rejected");

    let msg = &err.message;
    assert!(
        msg.contains("80") && msg.contains("characters"),
        "error message must state limit (80) and actual length, got: {msg}"
    );

    let aops_dir = root.join("aops");
    if aops_dir.exists() {
        assert_eq!(
            fs::read_dir(&aops_dir).unwrap().count(),
            0,
            "nothing must be written on explicit over-maximum filename failure"
        );
    }
}

#[test]
fn test_create_and_update_node_title_over_500_chars_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // Title over 500 characters (exceeds default MAX_FRONTMATTER_SCALAR_LEN = 500)
    let title_600 = "word ".repeat(120);
    assert!(title_600.chars().count() > 500);

    let res = server
        .handle_create_task(&json!({
            "title": title_600,
            "project": "aops",
            "type": "task",
        }))
        .expect("creating node with title > 500 chars must succeed");

    let out = json_of(&res);
    let task_id = out["id"].as_str().unwrap();

    // Now update title to another title > 500 characters
    let title_700 = "updated ".repeat(90);
    assert!(title_700.chars().count() > 500);

    let _update_res = server
        .handle_update_task(&json!({
            "id": task_id,
            "title": title_700,
        }))
        .expect("updating node title to > 500 chars must succeed");

    let task_doc = json_of(
        &server
            .handle_get_task(&json!({ "id": task_id }))
            .unwrap(),
    );
    assert_eq!(task_doc["title"].as_str().unwrap(), title_700);
}

