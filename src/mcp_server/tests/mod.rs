use super::*;
use crate::embeddings::Embedder;
use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::pkb::PkbDocument;
use crate::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod task_list_tests;
mod task_mutation_tests;
mod tag_date_filter_tests;
mod stale_read_tests;


pub(crate) fn make_doc(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!(doc_type));
        fm.insert("status".to_string(), json!(status));
        fm.insert("id".to_string(), json!(id));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), json!(p));
        }
        if !depends_on.is_empty() {
            fm.insert("depends_on".to_string(), json!(depends_on));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

pub(crate) fn make_doc_with_priority(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
        priority: i32,
        assignee: Option<&str>,
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!(doc_type));
        fm.insert("status".to_string(), json!(status));
        fm.insert("id".to_string(), json!(id));
        fm.insert("priority".to_string(), json!(priority));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), json!(p));
        }
        if let Some(a) = assignee {
            fm.insert("assignee".to_string(), json!(a));
        }
        if !depends_on.is_empty() {
            fm.insert("depends_on".to_string(), json!(depends_on));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    /// Make an epic container doc that declares an explicit `project:` slug,
    /// which its children inherit via compute_project_field.
pub(crate) fn make_container_doc(path: &str, title: &str, id: &str, project: &str) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("id".to_string(), serde_json::json!(id));
        fm.insert("title".to_string(), serde_json::json!(title));
        fm.insert("type".to_string(), serde_json::json!("epic"));
        fm.insert("status".to_string(), serde_json::json!("active"));
        fm.insert("project".to_string(), serde_json::json!(project));
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some("epic".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    /// Write a minimal polecat.yaml registering the slugs (plus title-style
    /// aliases) the tests use, so project values pass registry validation and
    /// alias filtering works.
pub(crate) fn write_test_polecat_yaml(root: &Path) {
        let content = "\
projects:
  proj-alpha:
    aliases: [ProjectAlpha]
  proj-beta:
    aliases: [ProjectBeta]
  proj-gamma:
    aliases: [ProjectGamma]
  proj-partial: {}
  proj-test: {}
  test-project: {}
  aops: {}
  mem: {}
  test: {}
  p: {}
";
        let _ = std::fs::write(root.join("polecat.yaml"), content);
    }

    /// Build a test graph with 3 project-slug containers and tasks under each,
    /// plus an orphan.
    ///
    /// proj-alpha (epic, project: proj-alpha; alias ProjectAlpha):
    ///   - task-a1: active, priority 1, assignee "alice"
    ///   - task-a2: active, priority 2, assignee "bob" (depends on task-a1)
    ///   - task-a3: done, priority 1
    ///
    /// proj-beta:
    ///   - task-b1: active, priority 1 (leaf, no deps = ready)
    ///   - task-b2: active, priority 2 (depends on task-b1 = blocked)
    ///
    /// proj-gamma:
    ///   - task-g1: active, priority 3
    ///
    /// Orphan (no project):
    ///   - task-orphan: active, priority 1
pub(crate) fn build_project_test_graph() -> GraphStore {
        let docs = vec![
            // Container nodes declaring explicit project slugs
            make_container_doc(
                "projects/proj-alpha.md",
                "ProjectAlpha",
                "proj-alpha",
                "proj-alpha",
            ),
            make_container_doc(
                "projects/proj-beta.md",
                "ProjectBeta",
                "proj-beta",
                "proj-beta",
            ),
            make_container_doc(
                "projects/proj-gamma.md",
                "ProjectGamma",
                "proj-gamma",
                "proj-gamma",
            ),
            // ProjectAlpha tasks
            make_doc_with_priority(
                "tasks/task-a1.md",
                "Alpha Task 1",
                "task",
                "ready",
                "task-a1",
                Some("proj-alpha"),
                &[],
                1,
                Some("alice"),
            ),
            make_doc_with_priority(
                "tasks/task-a2.md",
                "Alpha Task 2",
                "task",
                "ready",
                "task-a2",
                Some("proj-alpha"),
                &["task-a1"],
                2,
                Some("bob"),
            ),
            make_doc_with_priority(
                "tasks/task-a3.md",
                "Alpha Task 3",
                "task",
                "done",
                "task-a3",
                Some("proj-alpha"),
                &[],
                1,
                None,
            ),
            make_doc_with_priority(
                "tasks/task-a4.md",
                "Alpha Task 4",
                "task",
                "archived",
                "task-a4",
                Some("proj-alpha"),
                &[],
                1,
                None,
            ),
            // ProjectBeta tasks — task-b1 is a leaf with no deps (ready), task-b2 depends on task-b1
            make_doc_with_priority(
                "tasks/task-b1.md",
                "Beta Task 1",
                "task",
                "ready",
                "task-b1",
                Some("proj-beta"),
                &[],
                1,
                None,
            ),
            make_doc_with_priority(
                "tasks/task-b2.md",
                "Beta Task 2",
                "task",
                "ready",
                "task-b2",
                Some("proj-beta"),
                &["task-b1"],
                2,
                None,
            ),
            // ProjectGamma task
            make_doc_with_priority(
                "tasks/task-g1.md",
                "Gamma Task 1",
                "task",
                "ready",
                "task-g1",
                Some("proj-gamma"),
                &[],
                3,
                None,
            ),
            // Orphan task (no parent, no project)
            make_doc_with_priority(
                "tasks/task-orphan.md",
                "Orphan Task",
                "task",
                "ready",
                "task-orphan",
                None,
                &[],
                1,
                None,
            ),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb-project"))
    }

pub(crate) fn build_test_server() -> PkbSearchServer {
        let graph = build_project_test_graph();
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        // Per-call isolated temp root so parallel write-tests (create_task etc.)
        // don't race on a shared directory.
        static TEST_ROOT_SEQ: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let seq = TEST_ROOT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mem-test-pkb-{}-{}", std::process::id(), seq));
        let _ = std::fs::create_dir_all(&root);
        write_test_polecat_yaml(&root);
        let db = root.join("db");
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root,
            db,
            Arc::new(RwLock::new(graph)),
        )
    }

    /// Helper to extract task IDs from a list_tasks call result.
pub(crate) fn extract_task_ids(result: &CallToolResult) -> Vec<String> {
        // Use JSON format for easier parsing
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        // Parse the JSON output
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(tasks) = val.get("tasks").and_then(|t| t.as_array()) {
                return tasks
                    .iter()
                    .filter_map(|t| t.get("id").and_then(|id| id.as_str()).map(String::from))
                    .collect();
            }
        }
        vec![]
    }

    /// Helper: parse the full `tasks` array from a JSON-format list_tasks result.
pub(crate) fn extract_task_objects(result: &CallToolResult) -> Vec<serde_json::Value> {
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("tasks").and_then(|t| t.as_array()).cloned())
            .unwrap_or_default()
    }

    // ── mem-e394a6d0 AC1/AC2/AC6: default focus_score-DESC ordering + status ──

