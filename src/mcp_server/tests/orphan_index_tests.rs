//! Regression tests for task_5f2c5fa6: PKB integrity — index entries that
//! resolve to no document, and a directory/document name collision that
//! previously surfaced as a bare EISDIR.
//!
//! Two distinct defect classes are covered here:
//!
//! 1. A *vector-index orphan*: an entry survives in the semantic index
//!    (`VectorStore`) after its backing file is deleted. Unlike a
//!    graph-referenced "ghost node" (see `ghost_node_tests.rs`, which covers
//!    an id named in another document's `parent`/`depends_on` field with no
//!    file), this class has no referrer in the graph at all — it is
//!    invisible to `pkb_orphans` and only surfaces as a `search`/
//!    `search_by_tag` hit that resolves to nothing on every by-id lookup.
//!    `refresh_graph` does not fix it: it rebuilds the *graph* from disk but
//!    never touches the vector index (this is the documented, expected
//!    behaviour per task_5f2c5fa6's own repro notes).
//!
//! 2. A path/document collision where an id's resolved path exists but is a
//!    directory, not a file (aops_fdf19283) — previously surfaced as a bare
//!    `Failed to read task file: Is a directory (os error 21)`.

use super::*;
use crate::embeddings::Embedder;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Extract the concatenated text content of a tool result (unescaped, unlike
/// `format!("{:?}", ...)`), so substring assertions don't need to account
/// for Debug-escaped newlines.
fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a server with two real on-disk task files indexed into both the
/// graph and the vector store, then delete `task-ghost.md` from disk and
/// rebuild the graph from disk (mirroring the task's own repro: refresh
/// the graph, confirm it does not touch the vector index). The result is
/// exactly the state task_5f2c5fa6 describes: a search hit
/// (`VectorStore` entry) with no backing file and no graph node.
fn setup_server_with_orphaned_index_entry() -> (tempfile::TempDir, PkbSearchServer) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    std::fs::write(
        root.join("tasks/task-live.md"),
        "---\nid: task-live\ntitle: Live Task\ntype: task\nstatus: active\nproject: proj-test\ntags: [orphantest]\n---\n\nLive body keyword zzzlivekeyword.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tasks/task-ghost.md"),
        "---\nid: task-ghost\ntitle: Ghost Task\ntype: task\nstatus: active\nproject: proj-test\ntags: [orphantest]\n---\n\nGhost body keyword zzzghostkeyword.\n",
    )
    .unwrap();

    let docs: Vec<crate::pkb::PkbDocument> = crate::pkb::scan_directory_all(root)
        .into_iter()
        .filter_map(|p| crate::pkb::parse_file_relative(&p, root))
        .collect();
    assert_eq!(docs.len(), 2, "expected exactly the two task fixtures");

    let graph = GraphStore::build(&docs, root);

    let mut store = VectorStore::new(3);
    for doc in &docs {
        // Fixed identical embedding for every doc: differentiation in these
        // tests comes from the BM25 lexical component of search_hybrid, not
        // vector similarity (the dummy test embedder always returns zeros).
        store.insert_precomputed(doc, vec![doc.body.clone()], vec![vec![1.0, 0.0, 0.0]]);
    }

    // Delete the file out from under the index — no try_remove_document,
    // no WAL record. This is the desync the task describes.
    std::fs::remove_file(root.join("tasks/task-ghost.md")).unwrap();

    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    // Rebuild the graph from (now-1-file) disk, exactly as the task's own
    // repro did with pkb__refresh_graph — confirms it does not fix the
    // vector-store desync while still leaving the graph node gone.
    server.rebuild_graph();

    (tmp, server)
}

// ── Goal 1 + 3: detect and repair via repair_index_orphans ──

#[test]
fn test_repair_index_orphans_dry_run_reports_without_purging() {
    let (_tmp, server) = setup_server_with_orphaned_index_entry();

    let result = server
        .handle_repair_index_orphans(&json!({}))
        .expect("dry-run repair call succeeds");
    let val: serde_json::Value =
        serde_json::from_str(&result_text(&result)).expect("valid JSON output");

    assert_eq!(val["ok"], json!(true));
    assert_eq!(val["dry_run"], json!(true));
    assert_eq!(val["orphan_count"], json!(1));
    let orphan_ids: Vec<&str> = val["orphans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(orphan_ids, vec!["task-ghost"]);

    // Dry run must not mutate the store: calling again reports the same orphan.
    let result2 = server
        .handle_repair_index_orphans(&json!({}))
        .expect("second dry-run call succeeds");
    let val2: serde_json::Value = serde_json::from_str(&result_text(&result2)).unwrap();
    assert_eq!(val2["orphan_count"], json!(1));
}

#[test]
fn test_repair_index_orphans_purges_when_dry_run_false() {
    let (_tmp, server) = setup_server_with_orphaned_index_entry();

    let result = server
        .handle_repair_index_orphans(&json!({"dry_run": false}))
        .expect("repair call succeeds");
    let val: serde_json::Value = serde_json::from_str(&result_text(&result)).unwrap();

    assert_eq!(val["ok"], json!(true));
    assert_eq!(val["dry_run"], json!(false));
    assert_eq!(val["orphan_count"], json!(1));
    let purged_ids: Vec<&str> = val["purged"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(purged_ids, vec!["task-ghost"]);

    // The orphan is gone from the store...
    assert!(
        server.store_for_test().read().get_entry("task-ghost").is_none(),
        "orphaned entry must be removed from the vector store"
    );
    // ...but the live entry is untouched.
    assert!(
        server.store_for_test().read().get_entry("task-live").is_some(),
        "live entry must survive the repair"
    );

    // A follow-up dry run now reports clean.
    let result2 = server
        .handle_repair_index_orphans(&json!({}))
        .expect("dry-run after purge succeeds");
    let val2: serde_json::Value = serde_json::from_str(&result_text(&result2)).unwrap();
    assert_eq!(val2["orphan_count"], json!(0));
}

#[test]
fn test_repair_index_orphans_reports_clean_store_as_clean() {
    // build_test_server's VectorStore is empty — nothing to flag.
    let server = build_test_server();
    let result = server
        .handle_repair_index_orphans(&json!({}))
        .expect("repair call succeeds on an empty store");
    let val: serde_json::Value = serde_json::from_str(&result_text(&result)).unwrap();
    assert_eq!(val["orphan_count"], json!(0));
    assert_eq!(val["orphans"], json!([]));
}

// ── Goal 2: search results label the orphan in the hit itself ──

#[test]
fn test_pkb_search_labels_orphaned_hit_and_spares_live_hit() {
    let (_tmp, server) = setup_server_with_orphaned_index_entry();

    let result = server
        .handle_pkb_search(&json!({"query": "zzzghostkeyword", "limit": 10}))
        .expect("search succeeds");
    let text = result_text(&result);

    let blocks: Vec<&str> = text.split("### ").collect();
    let ghost_block = blocks
        .iter()
        .find(|b| b.contains("`task-ghost`"))
        .unwrap_or_else(|| panic!("expected a hit for task-ghost in: {text}"));
    assert!(
        ghost_block.contains("ORPHANED INDEX ENTRY"),
        "orphaned hit must be labelled: {ghost_block}"
    );

    let result2 = server
        .handle_pkb_search(&json!({"query": "zzzlivekeyword", "limit": 10}))
        .expect("search succeeds");
    let text2 = result_text(&result2);
    let blocks2: Vec<&str> = text2.split("### ").collect();
    let live_block = blocks2
        .iter()
        .find(|b| b.contains("`task-live`"))
        .unwrap_or_else(|| panic!("expected a hit for task-live in: {text2}"));
    assert!(
        !live_block.contains("ORPHANED INDEX ENTRY"),
        "live hit must not be mislabelled: {live_block}"
    );
}

#[test]
fn test_search_by_tag_labels_orphaned_hit_and_spares_live_hit() {
    let (_tmp, server) = setup_server_with_orphaned_index_entry();

    let result = server
        .handle_search_by_tag(&json!({"tags": ["orphantest"]}))
        .expect("search_by_tag succeeds");
    let text = result_text(&result);

    let ghost_line = text
        .lines()
        .find(|l| l.contains("`task-ghost`"))
        .unwrap_or_else(|| panic!("expected a line for task-ghost in: {text}"));
    assert!(
        ghost_line.contains("ORPHANED INDEX ENTRY"),
        "orphaned hit must be labelled: {ghost_line}"
    );

    let live_line = text
        .lines()
        .find(|l| l.contains("`task-live`"))
        .unwrap_or_else(|| panic!("expected a line for task-live in: {text}"));
    assert!(
        !live_line.contains("ORPHANED INDEX ENTRY"),
        "live hit must not be mislabelled: {live_line}"
    );
}

// ── Goal 4: directory/document name collision (aops_fdf19283) ──

/// Build a server with one real task file, and a graph node for that same
/// id whose `path` has been patched (post-build) to point at a real
/// directory instead — reproducing "an id resolves to a path that exists
/// but is a directory" independent of whatever upstream bug produces that
/// state in a live store. Disk is not touched after `build_from_directory`,
/// so `ensure_graph_fresh`'s stat-based staleness check does not undo the
/// patch (the generation stamp still matches disk).
fn setup_server_with_directory_collision() -> (tempfile::TempDir, PkbSearchServer) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tasks")).unwrap();
    write_test_polecat_yaml(root);

    std::fs::write(
        root.join("tasks/task-collide.md"),
        "---\nid: task-collide\ntitle: Collide Task\ntype: task\nstatus: active\nproject: proj-test\n---\n\nBody.\n",
    )
    .unwrap();
    // The colliding directory: a real directory on disk, distinct from the
    // node's real file, that the node's path gets pointed at below.
    std::fs::create_dir_all(root.join("tasks/task-collide-dir")).unwrap();

    let mut graph = GraphStore::build_from_directory(root);
    {
        let mut collided = graph
            .get_node("task-collide")
            .expect("node built from the real file")
            .clone();
        collided.path = PathBuf::from("tasks/task-collide-dir");
        graph.replace_node(collided);
    }

    let store = VectorStore::new(3);
    let embedder = Embedder::new_dummy();
    let db_path = root.join("db");
    let server = PkbSearchServer::new(
        Arc::new(RwLock::new(store)),
        Arc::new(embedder),
        root.to_path_buf(),
        db_path,
        Arc::new(RwLock::new(graph)),
    );

    (tmp, server)
}

#[test]
fn test_get_task_directory_collision_names_it_instead_of_eisdir() {
    let (_tmp, server) = setup_server_with_directory_collision();

    let err = server
        .handle_get_task(&json!({"id": "task-collide"}))
        .expect_err("a directory-collision id must not read successfully");
    let msg = err.message.to_string();

    assert!(
        !msg.contains("os error 21") && !msg.contains("Is a directory"),
        "must never leak the bare EISDIR: {msg}"
    );
    assert!(
        msg.contains("task-collide") && msg.to_lowercase().contains("directory"),
        "must name the id and the collision: {msg}"
    );
}

#[test]
fn test_get_document_directory_collision_names_it_instead_of_eisdir() {
    let (_tmp, server) = setup_server_with_directory_collision();

    let err = server
        .handle_get_document(&json!({"id": "task-collide"}))
        .expect_err("a directory-collision id must not read successfully");
    let msg = err.message.to_string();

    assert!(
        !msg.contains("os error 21") && !msg.contains("Is a directory"),
        "must never leak the bare EISDIR: {msg}"
    );
    assert!(
        msg.contains("task-collide") && msg.to_lowercase().contains("directory"),
        "must name the id and the collision: {msg}"
    );
}
