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
fn test_create_task_tool_overlong_title_succeeds_keeps_full_title_and_bounds_filename() {
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
            "slug": "bounded_name",
        }))
        .expect("creating a task with a title well over filename maximum must succeed");

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
fn test_create_document_note_overlong_title_succeeds_keeps_full_title_and_bounds_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // Title well over the 80-character filename maximum (165 characters)
    let long_title = "This is an extremely long note title well over the eighty character filename maximum limit designed to verify that PKB keeps the full title while bounding the filename";
    assert!(long_title.chars().count() > 80);

    let _res = server
        .handle_create_document(&json!({
            "title": long_title,
            "type": "note",
            "slug": "long_note_title",
        }))
        .expect("creating a note with a title well over filename maximum must succeed");

    let notes_dir = root.join("notes");
    let mut files = fs::read_dir(&notes_dir)
        .expect("notes directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one note file must be created");

    let filename = files.pop().unwrap();
    let file_path = notes_dir.join(&filename);
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
    assert_eq!(fm["type"].as_str().unwrap(), "note");
}

#[test]
fn test_create_document_task_overlong_title_succeeds_keeps_full_title_and_bounds_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // Title well over the 80-character filename maximum (165 characters)
    let long_title = "This is an extremely long task title created via create tool well over the eighty character filename maximum limit to verify bounding and full title retention";
    assert!(long_title.chars().count() > 80);

    let _res = server
        .handle_create_document(&json!({
            "title": long_title,
            "type": "task",
            "project": "aops",
            "slug": "long_task_title",
        }))
        .expect("creating a task via create tool with a title well over filename maximum must succeed");

    let tasks_dir = root.join("tasks");
    let mut files = fs::read_dir(&tasks_dir)
        .expect("tasks directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one task file must be created");

    let filename = files.pop().unwrap();
    let file_path = tasks_dir.join(&filename);
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
    assert_eq!(fm["type"].as_str().unwrap(), "task");
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
            "slug": "short",
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
            "slug": "title_500",
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

    let task_doc = json_of(&server.handle_get_task(&json!({ "id": task_id })).unwrap());
    assert_eq!(task_doc["title"].as_str().unwrap(), title_700);
}

#[test]
fn test_omitted_id_with_explicit_filename_generates_canonical_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // 1. Task: explicit filename, omitted id
    let res = server
        .handle_create_task(&json!({
            "title": "Task with explicit filename",
            "project": "aops",
            "filename": "custom_task_file.md",
            "slug": "task_slug",
        }))
        .expect("creating task with explicit filename must succeed");
    let out = json_of(&res);
    let task_id = out["id"].as_str().unwrap();

    assert_ne!(
        task_id, "custom_task_file",
        "id must NOT be derived from filename stem"
    );
    assert!(
        task_id.starts_with("aops_"),
        "id '{task_id}' must start with canonical project prefix 'aops_'"
    );
    assert_eq!(
        task_id.len(),
        5 + 8,
        "canonical id length must match 'aops_' + 8 hex chars"
    );

    let file_path = root.join("aops/custom_task_file.md");
    assert!(
        file_path.exists(),
        "task file must exist at explicit filename"
    );
    let fm = frontmatter(&file_path);
    assert_eq!(fm["id"].as_str().unwrap(), task_id);

    // 2. Memory: explicit filename, omitted id
    let _res = server
        .handle_create_memory(&json!({
            "title": "Memory with explicit filename",
            "filename": "custom_memory_file.md",
            "slug": "mem_slug",
        }))
        .expect("creating memory with explicit filename must succeed");
    let mem_file_path = root.join("memories/custom_memory_file.md");
    assert!(
        mem_file_path.exists(),
        "memory file must exist at explicit filename"
    );
    let fm_mem = frontmatter(&mem_file_path);
    let mem_id = fm_mem["id"].as_str().unwrap();

    assert_ne!(
        mem_id, "custom_memory_file",
        "id must NOT be derived from filename stem"
    );
    assert!(
        mem_id.starts_with("mem_"),
        "id '{mem_id}' must start with canonical prefix 'mem_'"
    );
    assert_eq!(
        mem_id.len(),
        4 + 8,
        "canonical id length must match 'mem_' + 8 hex chars"
    );

    // 3. Document: explicit filename, omitted id
    let _res = server
        .handle_create_document(&json!({
            "title": "Document with explicit filename",
            "type": "document",
            "filename": "custom_doc_file.md",
            "slug": "doc_slug",
        }))
        .expect("creating document with explicit filename must succeed");
    let doc_file_path = root.join("notes/custom_doc_file.md");
    assert!(
        doc_file_path.exists(),
        "document file must exist at explicit filename"
    );
    let fm_doc = frontmatter(&doc_file_path);
    let doc_id = fm_doc["id"].as_str().unwrap();

    assert_ne!(
        doc_id, "custom_doc_file",
        "id must NOT be derived from filename stem"
    );
    assert!(
        doc_id.starts_with("docu_"),
        "id '{doc_id}' must start with canonical prefix 'docu_'"
    );
    assert_eq!(
        doc_id.len(),
        5 + 8,
        "canonical id length must match 'docu_' + 8 hex chars"
    );
}

#[test]
fn test_create_task_rejects_missing_or_overlong_slug_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // 1. Missing slug rejects and writes nothing
    let err_missing = server
        .handle_create_task(&json!({
            "title": "Task without slug",
            "project": "aops",
        }))
        .expect_err("create_task with no slug must be rejected");
    assert_eq!(err_missing.code, ErrorCode::INVALID_PARAMS);
    let msg_missing = &err_missing.message;
    assert!(
        msg_missing.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_missing}"
    );
    assert!(
        msg_missing.contains("30"),
        "error message must state limit 30, got: {msg_missing}"
    );

    let aops_dir = root.join("aops");
    if aops_dir.exists() {
        assert_eq!(
            fs::read_dir(&aops_dir).unwrap().count(),
            0,
            "nothing must be written on missing slug failure"
        );
    }

    // 2. Overlong slug (> 30 characters) rejects and writes nothing
    let overlong_slug = "a".repeat(31);
    let err_overlong = server
        .handle_create_task(&json!({
            "title": "Task with overlong slug",
            "project": "aops",
            "slug": overlong_slug,
        }))
        .expect_err("create_task with slug over 30 chars must be rejected");
    assert_eq!(err_overlong.code, ErrorCode::INVALID_PARAMS);
    let msg_overlong = &err_overlong.message;
    assert!(
        msg_overlong.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_overlong}"
    );
    assert!(
        msg_overlong.contains("30"),
        "error message must state limit 30, got: {msg_overlong}"
    );

    if aops_dir.exists() {
        assert_eq!(
            fs::read_dir(&aops_dir).unwrap().count(),
            0,
            "nothing must be written on overlong slug failure"
        );
    }
}

#[test]
fn test_create_document_rejects_missing_or_overlong_slug_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // 1. Missing slug rejects and writes nothing
    let err_missing = server
        .handle_create_document(&json!({
            "title": "Doc without slug",
            "type": "note",
        }))
        .expect_err("create_document with no slug must be rejected");
    assert_eq!(err_missing.code, ErrorCode::INVALID_PARAMS);
    let msg_missing = &err_missing.message;
    assert!(
        msg_missing.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_missing}"
    );
    assert!(
        msg_missing.contains("30"),
        "error message must state limit 30, got: {msg_missing}"
    );

    let notes_dir = root.join("notes");
    if notes_dir.exists() {
        assert_eq!(
            fs::read_dir(&notes_dir).unwrap().count(),
            0,
            "nothing must be written on missing slug failure"
        );
    }

    // 2. Overlong slug (> 30 characters) rejects and writes nothing
    let overlong_slug = "b".repeat(31);
    let err_overlong = server
        .handle_create_document(&json!({
            "title": "Doc with overlong slug",
            "type": "note",
            "slug": overlong_slug,
        }))
        .expect_err("create_document with slug over 30 chars must be rejected");
    assert_eq!(err_overlong.code, ErrorCode::INVALID_PARAMS);
    let msg_overlong = &err_overlong.message;
    assert!(
        msg_overlong.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_overlong}"
    );
    assert!(
        msg_overlong.contains("30"),
        "error message must state limit 30, got: {msg_overlong}"
    );

    if notes_dir.exists() {
        assert_eq!(
            fs::read_dir(&notes_dir).unwrap().count(),
            0,
            "nothing must be written on overlong slug failure"
        );
    }
}

#[test]
fn test_create_memory_rejects_missing_or_overlong_slug_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    // 1. Missing slug rejects and writes nothing
    let err_missing = server
        .handle_create_memory(&json!({
            "title": "Memory without slug",
        }))
        .expect_err("create_memory with no slug must be rejected");
    assert_eq!(err_missing.code, ErrorCode::INVALID_PARAMS);
    let msg_missing = &err_missing.message;
    assert!(
        msg_missing.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_missing}"
    );
    assert!(
        msg_missing.contains("30"),
        "error message must state limit 30, got: {msg_missing}"
    );

    let memories_dir = root.join("memories");
    if memories_dir.exists() {
        assert_eq!(
            fs::read_dir(&memories_dir).unwrap().count(),
            0,
            "nothing must be written on missing slug failure"
        );
    }

    // 2. Overlong slug (> 30 characters) rejects and writes nothing
    let overlong_slug = "c".repeat(31);
    let err_overlong = server
        .handle_create_memory(&json!({
            "title": "Memory with overlong slug",
            "slug": overlong_slug,
        }))
        .expect_err("create_memory with slug over 30 chars must be rejected");
    assert_eq!(err_overlong.code, ErrorCode::INVALID_PARAMS);
    let msg_overlong = &err_overlong.message;
    assert!(
        msg_overlong.to_lowercase().contains("slug"),
        "error message must name 'slug', got: {msg_overlong}"
    );
    assert!(
        msg_overlong.contains("30"),
        "error message must state limit 30, got: {msg_overlong}"
    );

    if memories_dir.exists() {
        assert_eq!(
            fs::read_dir(&memories_dir).unwrap().count(),
            0,
            "nothing must be written on overlong slug failure"
        );
    }
}

#[test]
fn test_create_task_valid_slug_builds_filename_from_slug_and_keeps_full_title() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    let long_title = "This is an extremely long task title well over the thirty character slug limit designed to verify that filename is built from the slug while preserving the full title";
    let slug = "my_task_slug";

    let res = server
        .handle_create_task(&json!({
            "title": long_title,
            "project": "aops",
            "slug": slug,
        }))
        .expect("create_task with valid slug must succeed");

    let out = json_of(&res);
    let task_id = out["id"].as_str().unwrap();

    let aops_dir = root.join("aops");
    let mut files = fs::read_dir(&aops_dir)
        .expect("aops directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one task file must be created");

    let filename = files.pop().unwrap();
    assert_eq!(
        filename,
        format!("{task_id}_{slug}.md"),
        "filename must be built from id and slug"
    );

    let file_path = aops_dir.join(&filename);
    let fm = frontmatter(&file_path);
    assert_eq!(
        fm["title"].as_str().unwrap(),
        long_title,
        "full un-truncated title must be preserved in frontmatter"
    );
    assert_eq!(fm["id"].as_str().unwrap(), task_id);
}

#[test]
fn test_create_document_valid_slug_builds_filename_from_slug_and_keeps_full_title() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    let long_title = "This is an extremely long document title well over the thirty character slug limit designed to verify that filename is built from the slug while preserving the full title";
    let slug = "my_doc_slug";

    let _res = server
        .handle_create_document(&json!({
            "title": long_title,
            "type": "note",
            "slug": slug,
        }))
        .expect("create_document with valid slug must succeed");

    let notes_dir = root.join("notes");
    let mut files = fs::read_dir(&notes_dir)
        .expect("notes directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one note file must be created");

    let filename = files.pop().unwrap();
    let file_path = notes_dir.join(&filename);
    let fm = frontmatter(&file_path);
    let doc_id = fm["id"].as_str().unwrap();

    assert_eq!(
        filename,
        format!("{doc_id}_{slug}.md"),
        "filename must be built from id and slug"
    );

    assert_eq!(
        fm["title"].as_str().unwrap(),
        long_title,
        "full un-truncated title must be preserved in frontmatter"
    );
}

#[test]
fn test_create_memory_valid_slug_builds_filename_from_slug_and_keeps_full_title() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    let long_title = "This is an extremely long memory title well over the thirty character slug limit designed to verify that filename is built from the slug while preserving the full title";
    let slug = "my_mem_slug";

    let _res = server
        .handle_create_memory(&json!({
            "title": long_title,
            "slug": slug,
        }))
        .expect("create_memory with valid slug must succeed");

    let memories_dir = root.join("memories");
    let mut files = fs::read_dir(&memories_dir)
        .expect("memories directory must exist")
        .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "exactly one memory file must be created");

    let filename = files.pop().unwrap();
    let file_path = memories_dir.join(&filename);
    let fm = frontmatter(&file_path);
    let mem_id = fm["id"].as_str().unwrap();

    assert_eq!(
        filename,
        format!("{mem_id}_{slug}.md"),
        "filename must be built from id and slug"
    );

    assert_eq!(
        fm["title"].as_str().unwrap(),
        long_title,
        "full un-truncated title must be preserved in frontmatter"
    );
}

#[test]
fn test_create_actions_schemas_and_descriptions_declare_required_slug_and_limit() {
    let tools = PkbSearchServer::get_published_tools();

    // 1. Tool "create"
    let create_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "create")
        .expect("tool 'create' must exist");
    let create_desc = create_tool.description.as_deref().unwrap_or("");
    assert!(
        create_desc.to_lowercase().contains("slug") && create_desc.contains("30"),
        "create tool description must state required slug and 30-char limit, got: {create_desc}"
    );

    let create_schema = serde_json::to_value(&create_tool.input_schema).unwrap();
    let create_props = create_schema
        .get("properties")
        .unwrap()
        .as_object()
        .unwrap();
    assert!(
        create_props.contains_key("slug"),
        "create schema properties must include 'slug'"
    );
    let create_slug_desc = create_props["slug"]["description"].as_str().unwrap_or("");
    assert!(
        create_slug_desc.contains("30"),
        "create slug property description must state 30 limit, got: {create_slug_desc}"
    );
    let create_req: Vec<&str> = create_schema
        .get("required")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        create_req.contains(&"slug"),
        "create schema required array must include 'slug', got: {create_req:?}"
    );

    // 2. Tool "create_task"
    let create_task_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "create_task")
        .expect("tool 'create_task' must exist");
    let create_task_desc = create_task_tool.description.as_deref().unwrap_or("");
    assert!(
        create_task_desc.to_lowercase().contains("slug") && create_task_desc.contains("30"),
        "create_task tool description must state required slug and 30-char limit, got: {create_task_desc}"
    );

    let create_task_schema = serde_json::to_value(&create_task_tool.input_schema).unwrap();
    let create_task_props = create_task_schema
        .get("properties")
        .unwrap()
        .as_object()
        .unwrap();
    assert!(
        create_task_props.contains_key("slug"),
        "create_task schema properties must include 'slug'"
    );
    let create_task_slug_desc = create_task_props["slug"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        create_task_slug_desc.contains("30"),
        "create_task slug property description must state 30 limit, got: {create_task_slug_desc}"
    );
    let create_task_req: Vec<&str> = create_task_schema
        .get("required")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        create_task_req.contains(&"slug"),
        "create_task schema required array must include 'slug', got: {create_task_req:?}"
    );
}
