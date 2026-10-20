//! Workflows (`type: template`) are filed as `<snake_case id>.md` — the id
//! only, no title — in `workflows/` (no project) or `<project>/workflows/`
//! (project-scoped), by both `create` and `convert_document`. An explicit
//! `dir` still overrides the default directory.

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

fn json_of(res: &CallToolResult) -> serde_json::Value {
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("expected JSON, got: {text}"))
}

#[test]
fn create_template_without_project_lands_in_workflows_by_snake_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    server
        .handle_create_document(&json!({
            "title": "Email sweep",
            "type": "template",
            "id": "wf-email-sweep",
        }))
        .unwrap();

    let path = root.join("workflows/wf_email_sweep.md");
    assert!(path.exists(), "expected {}", path.display());
    let fm = frontmatter(&path);
    assert_eq!(
        fm["id"], "wf-email-sweep",
        "frontmatter id is kept verbatim"
    );
    assert_eq!(fm["type"], "template");
    assert!(fm.get("project").is_none());
}

#[test]
fn create_template_with_project_lands_in_project_workflows() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    server
        .handle_create_document(&json!({
            "title": "Dev task",
            "type": "template",
            "id": "aops-wf-dev-task",
            "project": "aops",
        }))
        .unwrap();

    let path = root.join("aops/workflows/aops_wf_dev_task.md");
    assert!(path.exists(), "expected {}", path.display());
    let fm = frontmatter(&path);
    assert_eq!(fm["id"], "aops-wf-dev-task");
    assert_eq!(fm["project"], "aops");
}

#[test]
fn create_template_canonicalises_project_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    server
        .handle_create_document(&json!({
            "title": "Alpha flow",
            "type": "template",
            "id": "wf-alpha-flow",
            "project": "ProjectAlpha",
        }))
        .unwrap();

    let path = root.join("proj-alpha/workflows/wf_alpha_flow.md");
    assert!(path.exists(), "expected {}", path.display());
    assert_eq!(frontmatter(&path)["project"], "proj-alpha");
}

#[test]
fn create_template_rejects_unregistered_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    let res = server.handle_create_document(&json!({
        "title": "Ghost",
        "type": "template",
        "id": "wf-ghost",
        "project": "no-such-project",
    }));
    assert!(res.is_err());
    assert!(!root.join("no-such-project").exists());
}

#[test]
fn create_template_explicit_dir_overrides_default() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    server
        .handle_create_document(&json!({
            "title": "Elsewhere",
            "type": "template",
            "id": "wf-elsewhere",
            "project": "aops",
            "dir": "custom/place",
        }))
        .unwrap();

    assert!(root.join("custom/place/wf_elsewhere.md").exists());
    assert!(!root.join("aops/workflows").exists());
}

#[test]
fn create_non_template_filename_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let server = make_server(root);

    server
        .handle_create_document(&json!({
            "title": "Some note",
            "type": "note",
            "id": "note-abc",
        }))
        .unwrap();

    assert!(root.join("notes/note-abc_some_note.md").exists());
}

fn write_note(root: &Path, rel: &str, extra: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "---\nid: wf-weekly-review\ntitle: Weekly review\ntype: note\n{extra}---\n\nSteps.\n"
        ),
    )
    .unwrap();
    path
}

#[test]
fn convert_to_template_without_project_lands_in_workflows() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let old = write_note(root, "notes/wf-weekly-review_weekly_review.md", "");
    let server = make_server(root);

    let out = json_of(
        &server
            .handle_convert_document(&json!({ "id": "wf-weekly-review", "type": "template" }))
            .unwrap(),
    );
    assert_eq!(out["path"], "workflows/wf_weekly_review.md");
    assert!(!old.exists());
    let fm = frontmatter(&root.join("workflows/wf_weekly_review.md"));
    assert_eq!(fm["id"], "wf-weekly-review");
    assert_eq!(fm["type"], "template");
}

#[test]
fn convert_to_template_uses_existing_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_note(
        root,
        "notes/wf-weekly-review_weekly_review.md",
        "project: mem\n",
    );
    let server = make_server(root);

    let out = json_of(
        &server
            .handle_convert_document(&json!({ "id": "wf-weekly-review", "type": "template" }))
            .unwrap(),
    );
    assert_eq!(out["path"], "mem/workflows/wf_weekly_review.md");
    assert!(root.join("mem/workflows/wf_weekly_review.md").exists());
}

#[test]
fn convert_to_template_explicit_dir_overrides_default() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_note(
        root,
        "notes/wf-weekly-review_weekly_review.md",
        "project: mem\n",
    );
    let server = make_server(root);

    let out = json_of(
        &server
            .handle_convert_document(&json!({
                "id": "wf-weekly-review",
                "type": "template",
                "dir": "elsewhere",
            }))
            .unwrap(),
    );
    assert_eq!(out["path"], "elsewhere/wf_weekly_review.md");
}
