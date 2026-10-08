//! `aops_b6e26552`: `convert_document` turns an existing document into another
//! type in place — same file, same ID — moving and renaming it and
//! reindexing it. The motivating case is a mobile capture (a `type: note` in
//! `notes/mobile-captures/`, often with no `id:` key) becoming a task.

use super::*;

const CAPTURE_STEM: &str = "20261003-0912-ring-the-registrar";

fn make_server(root: &Path) -> PkbSearchServer {
    write_test_polecat_yaml(root);
    let graph = GraphStore::build_from_directory(root);
    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        root.join("db"),
        Arc::new(RwLock::new(graph)),
    )
}

/// A capture as the front ends write it: no `id:`, so its ID is the stem.
fn write_capture(root: &Path, frontmatter_extra: &str) -> PathBuf {
    let dir = root.join("notes/mobile-captures");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{CAPTURE_STEM}.md"));
    std::fs::write(
        &path,
        format!(
            "---\ntitle: Ring the registrar\ntype: note\n{frontmatter_extra}---\n\nAsk about the enrolment form.\n"
        ),
    )
    .unwrap();
    path
}

fn json_of(res: &CallToolResult) -> serde_json::Value {
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

fn frontmatter(path: &Path) -> serde_json::Value {
    use gray_matter::engine::YAML;
    use gray_matter::Matter;
    let content = std::fs::read_to_string(path).unwrap();
    Matter::<YAML>::new()
        .parse(&content)
        .data
        .unwrap()
        .deserialize()
        .unwrap()
}

#[test]
fn converts_capture_to_task_keeping_id_and_reindexing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let old_path = write_capture(root, "");
    let server = make_server(root);

    let out = json_of(
        &server
            .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "task" }))
            .unwrap(),
    );
    let new_rel = format!("tasks/{CAPTURE_STEM}_ring_the_registrar.md");
    assert_eq!(out["id"], CAPTURE_STEM);
    assert_eq!(out["old_type"], "note");
    assert_eq!(out["type"], "task");
    assert_eq!(out["status"], "inbox");
    assert_eq!(out["path"], new_rel.as_str());
    assert_eq!(out["moved"], true);
    assert_eq!(out["retyped"], true);

    // Disk: same file moved, ID pinned in frontmatter, body intact.
    let new_path = root.join(&new_rel);
    assert!(!old_path.exists(), "capture must leave notes/mobile-captures/");
    let fm = frontmatter(&new_path);
    assert_eq!(fm["id"], CAPTURE_STEM);
    assert_eq!(fm["type"], "task");
    assert_eq!(fm["status"], "inbox");
    assert_eq!(fm["title"], "Ring the registrar");
    assert!(std::fs::read_to_string(&new_path)
        .unwrap()
        .contains("Ask about the enrolment form."));
    assert_eq!(
        std::fs::read_dir(root.join("notes/mobile-captures")).unwrap().count(),
        0,
        "no file may be left behind"
    );

    // Graph: same ID resolves to the moved node as a task.
    {
        let graph = server.graph.read();
        let node = graph.resolve(CAPTURE_STEM).expect("node still resolves");
        assert_eq!(node.id, CAPTURE_STEM);
        assert_eq!(node.node_type.as_deref(), Some("task"));
        assert_eq!(node.path, PathBuf::from(&new_rel));
    }
    let task = json_of(&server.handle_get_task(&json!({ "id": CAPTURE_STEM })).unwrap());
    assert_eq!(task["status"], "inbox");

    // Vector index: one entry for the ID, at the new path.
    let store = server.store.read();
    let entry = store.get_entry(CAPTURE_STEM).expect("index entry for the ID");
    assert_eq!(entry.path, PathBuf::from(&new_rel));
    assert_eq!(entry.doc_type.as_deref(), Some("task"));
    assert_eq!(
        store.documents().filter(|(_, e)| e.id == CAPTURE_STEM).count(),
        1
    );
}

#[test]
fn rerun_is_a_noop_and_dir_and_status_are_honoured() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_capture(root, "");
    let server = make_server(root);
    let args = json!({ "id": CAPTURE_STEM, "type": "task", "dir": "proj-alpha", "status": "ready" });

    let first = json_of(&server.handle_convert_document(&args).unwrap());
    let new_rel = format!("proj-alpha/{CAPTURE_STEM}_ring_the_registrar.md");
    assert_eq!(first["path"], new_rel.as_str());
    assert_eq!(first["status"], "ready");

    let before = std::fs::read_to_string(root.join(&new_rel)).unwrap();
    let second = json_of(&server.handle_convert_document(&args).unwrap());
    assert_eq!(second["moved"], false);
    assert_eq!(second["retyped"], false);
    assert_eq!(second["path"], new_rel.as_str());
    assert_eq!(
        std::fs::read_to_string(root.join(&new_rel)).unwrap(),
        before,
        "a no-op rerun must not rewrite the file"
    );
}

#[test]
fn rejections_leave_the_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let old_path = write_capture(root, "status: someday-maybe\n");
    let server = make_server(root);
    let original = std::fs::read_to_string(&old_path).unwrap();

    // Existing status is not a valid task status and no replacement given.
    let err = server
        .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "task" }))
        .unwrap_err();
    assert!(err.message.contains("not a valid task status"), "{}", err.message);

    // Invalid type.
    let err = server
        .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "nonsense", "status": "inbox" }))
        .unwrap_err();
    assert!(err.message.contains("Invalid node type"), "{}", err.message);

    // Path traversal in dir.
    let err = server
        .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "task", "status": "inbox", "dir": "../outside" }))
        .unwrap_err();
    assert!(err.message.contains("Invalid dir path"), "{}", err.message);

    // Target file already exists.
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    let clash = root.join(format!("tasks/{CAPTURE_STEM}_ring_the_registrar.md"));
    std::fs::write(&clash, "occupied").unwrap();
    let err = server
        .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "task", "status": "inbox" }))
        .unwrap_err();
    assert!(err.message.contains("already exists"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(&clash).unwrap(), "occupied");

    assert_eq!(std::fs::read_to_string(&old_path).unwrap(), original);
}

#[test]
fn git_records_the_move_as_a_rename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    write_capture(root, "");
    write_test_polecat_yaml(root);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "capture"]);

    let server = make_server(root);
    server
        .handle_convert_document(&json!({ "id": CAPTURE_STEM, "type": "task" }))
        .unwrap();

    assert_eq!(git(&["status", "--porcelain", "--", "notes", "tasks"]).trim(), "", "conversion must be fully committed");
    let last = git(&["show", "--name-status", "-M", "--format=%s", "HEAD"]);
    assert!(last.starts_with(&format!("convert({CAPTURE_STEM})")), "{last}");
    assert!(
        last.lines().any(|l| l.starts_with('R')
            && l.contains(&format!("notes/mobile-captures/{CAPTURE_STEM}.md"))
            && l.contains(&format!("tasks/{CAPTURE_STEM}_ring_the_registrar.md"))),
        "expected a rename entry, got:\n{last}"
    );
}
