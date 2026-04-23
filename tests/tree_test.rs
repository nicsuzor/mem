#[cfg(test)]
mod tests {
    use mem::mcp_server::PkbSearchServer;
    use mem::graph_store::GraphStore;
    use mem::pkb::PkbDocument;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn make_doc(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), json!(title));
        fm.insert("type".to_string(), json!(doc_type));
        fm.insert("status".to_string(), json!(status));
        fm.insert("id".to_string(), json!(id));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), json!(p));
        }

        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            modified: None,
            content_hash: String::new(),
        }
    }

    fn build_test_server() -> PkbSearchServer {
        let docs = vec![
            make_doc("projects/p1.md", "Project One", "project", "active", "p1", None),
            make_doc("epics/e1.md", "Epic One", "epic", "active", "e1", Some("p1")),
            make_doc("tasks/t1.md", "Task One", "task", "active", "t1", Some("e1")),
            make_doc("tasks/t2.md", "Task Two", "task", "active", "t2", Some("e1")),
            make_doc("tasks/t3.md", "Task Three", "task", "active", "t3", None),
        ];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        PkbSearchServer::new(Arc::new(RwLock::new(gs)), Arc::new(RwLock::new(Default::default())))
    }

    #[test]
    fn test_list_tasks_json_include_parent() {
        let server = build_test_server();
        let result = server.handle_list_tasks(&json!({
            "format": "json",
            "include_parent": true
        })).unwrap();

        let val: serde_json::Value = serde_json::from_str(result.content[0].as_text().unwrap()).unwrap();
        let tasks = val["tasks"].as_array().unwrap();

        // t1 should have e1 as parent
        let t1 = tasks.iter().find(|t| t["id"] == "t1").unwrap();
        assert_eq!(t1["parent_id"], "e1");
        assert_eq!(t1["parent_title"], "Epic One");

        // t3 should have no parent
        let t3 = tasks.iter().find(|t| t["id"] == "t3").unwrap();
        assert!(t3["parent_id"].is_null());
        assert!(t3["parent_title"].is_null());
    }

    #[test]
    fn test_list_tasks_tree_format() {
        let server = build_test_server();
        let result = server.handle_list_tasks(&json!({
            "format": "tree"
        })).unwrap();

        let text = result.content[0].as_text().unwrap();
        println!("{}", text);

        assert!(text.contains("## Task Tree"));
        assert!(text.contains("**project: Project One**"));
        assert!(text.contains("**epic: Epic One**"));
        assert!(text.contains("task: `t1`"));
        assert!(text.contains("task: `t2`"));
        assert!(text.contains("task: `t3`"));
    }

    #[test]
    fn test_get_tree() {
        let server = build_test_server();
        let result = server.handle_get_tree(&json!({
            "id": "p1"
        })).unwrap();

        let text = result.content[0].as_text().unwrap();
        println!("{}", text);

        assert!(text.contains("## Tree for `p1`"));
        assert!(text.contains("project: `p1`"));
        assert!(text.contains("epic: `e1`"));
        assert!(text.contains("task: `t1`"));
    }
}
