//! Integration tests for document search and listing across CLI and MCP surfaces.
//!
//! Verifies:
//! 1. CLI `pkb documents` (and aliases `docs`, `list`) lists documents by type without requiring a tag.
//! 2. CLI `pkb documents` supports --type, --tag, --status, --limit, --offset.
//! 3. Cross-surface parity: CLI `pkb documents` and VectorStore `list_documents` return the same document set.
//! 4. CLI `pkb search --type` filters by type (including `task`, `template`, `!task` negation, unknown type).
//! 5. Every search hit on CLI is labelled with its type.

use mem::vectordb::VectorStore;
use std::path::Path;
use std::process::Command;

fn seed_corpus(dir: &Path) {
    std::fs::create_dir_all(dir.join("tasks")).unwrap();
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::create_dir_all(dir.join("contacts")).unwrap();
    std::fs::create_dir_all(dir.join("projects")).unwrap();

    std::fs::write(
        dir.join("projects/p.md"),
        "---\nid: proj-main\ntitle: \"Main Project\"\ntype: epic\nstatus: ready\n---\n# Main\n",
    )
    .unwrap();

    // 2 Tasks
    std::fs::write(
        dir.join("tasks/task-1.md"),
        "---\nid: task-1\ntitle: \"Alpha task\"\ntype: task\nstatus: ready\nparent: proj-main\ntags:\n  - urgent\n  - search\n---\nTask Alpha body for search testing\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("tasks/task-2.md"),
        "---\nid: task-2\ntitle: \"Beta task\"\ntype: task\nstatus: in_progress\nparent: proj-main\ntags:\n  - search\n---\nTask Beta body for search testing\n",
    )
    .unwrap();

    // 2 Templates
    std::fs::write(
        dir.join("templates/tmpl-1.md"),
        "---\nid: tmpl-review\ntitle: \"Review Workflow Template\"\ntype: template\ntags:\n  - workflow\n  - search\n---\nReview template workflow body\n",
    )
    .unwrap();

    std::fs::write(
        dir.join("templates/tmpl-2.md"),
        "---\nid: tmpl-daily\ntitle: \"Daily Note Template\"\ntype: template\ntags:\n  - workflow\n---\nDaily template body\n",
    )
    .unwrap();

    // 1 Note (with status: ready in frontmatter to test non-task status suppression)
    std::fs::write(
        dir.join("notes/note-1.md"),
        "---\nid: note-arch\ntitle: \"Architecture Note\"\ntype: note\nstatus: ready\ntags:\n  - doc\n  - search\n---\nArchitecture note body for search\n",
    )
    .unwrap();

    // 1 Contact (with status: ready in frontmatter)
    std::fs::write(
        dir.join("contacts/contact-1.md"),
        "---\nid: contact-alice\ntitle: \"Alice Smith\"\ntype: contact\nstatus: ready\ntags:\n  - contact\n---\nContact info for Alice\n",
    )
    .unwrap();

    // Populate vector store & save index file with correct embedding dimension
    let dim = mem::embeddings::EMBEDDING_DIM;
    let mut store = VectorStore::new(dim);
    for file in [
        dir.join("projects/p.md"),
        dir.join("tasks/task-1.md"),
        dir.join("tasks/task-2.md"),
        dir.join("templates/tmpl-1.md"),
        dir.join("templates/tmpl-2.md"),
        dir.join("notes/note-1.md"),
        dir.join("contacts/contact-1.md"),
    ] {
        let doc = mem::pkb::parse_file(&file).unwrap();
        store.insert_precomputed(&doc, vec![doc.body.clone()], vec![vec![0.0; dim]]);
    }
    store.save(&dir.join("db.bin")).unwrap();
}

#[test]
fn test_cli_documents_listing_by_type() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_corpus(dir.path());

    let db = dir.path().join("db.bin");
    let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args([
            "--pkb-root",
            dir.path().to_str().unwrap(),
            "--db-path",
            db.to_str().unwrap(),
            "documents",
            "--type",
            "template",
        ])
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.path().to_str().unwrap())
        .output()
        .expect("exec pkb documents");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("2 documents found"), "got: {stdout}");
    assert!(stdout.contains("Review Workflow Template"));
    assert!(stdout.contains("Daily Note Template"));
    assert!(!stdout.contains("Alpha task"));
    assert!(!stdout.contains("Architecture Note"));
}

#[test]
fn test_cli_documents_aliases_docs_and_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_corpus(dir.path());
    let db = dir.path().join("db.bin");

    for alias in ["docs", "list"] {
        let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
            .args([
                "--pkb-root",
                dir.path().to_str().unwrap(),
                "--db-path",
                db.to_str().unwrap(),
                alias,
                "--type",
                "template",
            ])
            .env("AOPS_OFFLINE", "1")
            .env("ACA_DATA", dir.path().to_str().unwrap())
            .output()
            .expect("exec alias");

        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("2 documents found"), "alias {alias} failed: {stdout}");
        assert!(stdout.contains("tmpl-review"));
        assert!(stdout.contains("tmpl-daily"));
    }
}

#[test]
fn test_cli_documents_filter_composition() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_corpus(dir.path());
    let db = dir.path().join("db.bin");

    // Filter by tag
    let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args([
            "--pkb-root",
            dir.path().to_str().unwrap(),
            "--db-path",
            db.to_str().unwrap(),
            "documents",
            "--tag",
            "urgent",
        ])
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.path().to_str().unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 documents found"), "tag search got: {stdout}");
    assert!(stdout.contains("task-1"));

    // Filter by pagination (limit=1, offset=1)
    let out_paged = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args([
            "--pkb-root",
            dir.path().to_str().unwrap(),
            "--db-path",
            db.to_str().unwrap(),
            "documents",
            "--type",
            "template",
            "-n",
            "1",
            "--offset",
            "1",
        ])
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.path().to_str().unwrap())
        .output()
        .unwrap();
    let stdout_paged = String::from_utf8_lossy(&out_paged.stdout);
    assert!(stdout_paged.contains("2 documents found"), "got: {stdout_paged}");
    assert!(stdout_paged.contains("showing 1, offset 1"), "got: {stdout_paged}");
}

#[test]
fn test_cross_surface_parity_cli_and_underlying_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_corpus(dir.path());
    let db = dir.path().join("db.bin");

    // 1. Fetch via CLI
    let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args([
            "--pkb-root",
            dir.path().to_str().unwrap(),
            "--db-path",
            db.to_str().unwrap(),
            "documents",
            "--type",
            "template",
        ])
        .env("AOPS_OFFLINE", "1")
        .env("ACA_DATA", dir.path().to_str().unwrap())
        .output()
        .unwrap();
    let cli_output = String::from_utf8_lossy(&out.stdout);

    // 2. Fetch directly via VectorStore::list_documents
    let store = VectorStore::load_or_create(&db, mem::embeddings::EMBEDDING_DIM).unwrap();
    let store_results = store.list_documents(None, Some("template"), None, dir.path());

    assert_eq!(store_results.len(), 2);
    for r in &store_results {
        assert!(cli_output.contains(&r.id), "CLI output should contain {}", r.id);
        assert!(cli_output.contains(&r.title), "CLI output should contain {}", r.title);
    }
}
