use super::*;
#[cfg(test)]
#[cfg(test)]
mod claim_task_tests {
    use super::*;
    use crate::embeddings::Embedder;
    use crate::graph_store::GraphStore;
    use crate::vectordb::VectorStore;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn setup_with_template() -> (PkbSearchServer, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let tasks_dir = root.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();

        // Write a template file
        let template_content = "\
---
id: daily-template
title: \"Daily\"
type: template
status: active
priority: 2
parent: proj-test
project: aops
tags:
  - daily
  - recurring
---

## Daily checklist

- [ ] Review inbox
- [ ] Check outstanding PRs
";
        std::fs::write(tasks_dir.join("daily-template.md"), template_content).unwrap();

        // Write the parent container file so validation passes
        let projects_dir = root.join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("proj-test.md"),
            "---\nid: proj-test\ntitle: \"Test Project\"\ntype: epic\nproject: proj-test\nstatus: active\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.join("polecat.yaml"),
            "projects:\n  proj-test: {}\n  aops: {}\n",
        )
        .unwrap();

        let docs = crate::pkb::scan_directory(root)
            .iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, root))
            .collect::<Vec<_>>();

        let graph = GraphStore::build(&docs, root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );
        (server, tmp)
    }

    /// A template whose `project:` slug is not registered in polecat.yaml
    /// must fail to claim — the claim path validates like every other write
    /// path that stamps an explicit project value.
    #[test]
    fn claim_task_rejects_unregistered_template_project() {
        let (server, tmp) = setup_with_template();

        // Rewrite the template's project to a slug the fixture registry
        // (proj-test, aops) does not contain, then refresh the graph.
        let template_path = tmp.path().join("tasks/daily-template.md");
        let content = std::fs::read_to_string(&template_path).unwrap();
        let stale = content.replace("project: aops", "project: deregistered-slug");
        assert_ne!(content, stale, "fixture must carry project: aops");
        std::fs::write(&template_path, stale).unwrap();
        {
            let graph = GraphStore::build_from_directory(tmp.path());
            *server.graph.write() = graph;
        }

        let err = server
            .handle_claim_task(&serde_json::json!({ "id": "daily-template" }))
            .expect_err("claiming a template with an unregistered project must fail");
        assert!(
            err.message.contains("deregistered-slug"),
            "error should name the unknown slug; got: {}",
            err.message
        );
    }

    #[test]
    fn claim_task_creates_datestamped_instance() {
        let (server, tmp) = setup_with_template();

        let result = server
            .handle_claim_task(&serde_json::json!({ "id": "daily-template" }))
            .expect("claim_task should succeed");

        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .next()
            .expect("result should have text content");

        // Response is get_task JSON — must contain the instance id
        assert!(
            text.contains("daily-"),
            "instance id should contain template slug; got: {text}"
        );
        assert!(
            text.contains("template_id"),
            "instance should back-reference the template; got: {text}"
        );

        // Verify instance file was written to tasks/
        let tasks_dir = tmp.path().join("tasks");
        let instance_files: Vec<_> = std::fs::read_dir(&tasks_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("daily-") && name != "daily-template.md"
            })
            .collect();
        assert_eq!(
            instance_files.len(),
            1,
            "exactly one instance file should exist; found: {:?}",
            instance_files
                .iter()
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        );

        // Instance must be type: task, not type: template
        let instance_path = instance_files[0].path();
        let content = std::fs::read_to_string(&instance_path).unwrap();
        assert!(
            content.contains("type: task"),
            "instance must be type: task"
        );
        assert!(
            content.contains("status: in_progress"),
            "instance must start as in_progress"
        );
        assert!(
            content.contains("template_id: \"daily-template\""),
            "instance must reference the template"
        );
        assert!(
            content.contains("- [ ] Review inbox"),
            "instance must inherit template body"
        );
    }

    #[test]
    fn claim_task_rejects_non_template() {
        let (server, _tmp) = setup_with_template();

        // Try to claim an epic (not a template, not a task)
        let err = server
            .handle_claim_task(&serde_json::json!({ "id": "proj-test" }))
            .expect_err("should fail on non-template, non-task node");

        assert!(
            err.message.contains("not 'template'"),
            "error should mention template type requirement; got: {}",
            err.message
        );
    }

    /// Regression test for mem_e8f3aa17: `claim_task` on a regular `type:
    /// task` node must claim it IN PLACE (status -> in_progress, no new
    /// node) instead of rejecting it with "not 'template'". This is the
    /// pre-template-feature claim_task behavior; a later change collapsed
    /// it into an unconditional template-only gate.
    #[test]
    fn claim_task_claims_regular_task_in_place() {
        let (server, tmp) = setup_with_template();

        // Add a plain, regular task node alongside the template fixture.
        let tasks_dir = tmp.path().join("tasks");
        let task_content = "\
---
id: plain-task
title: \"Plain Task\"
type: task
status: queued
priority: 2
parent: proj-test
project: aops
---

## Body
";
        std::fs::write(tasks_dir.join("plain-task.md"), task_content).unwrap();
        {
            let graph = GraphStore::build_from_directory(tmp.path());
            *server.graph.write() = graph;
        }

        let result = server
            .handle_claim_task(&serde_json::json!({ "id": "plain-task", "assignee": "polecat" }))
            .expect("claiming a regular task node must succeed (in-place claim)");

        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .next()
            .expect("result should have text content");

        // Response is get_task JSON for the SAME id — no copy was created.
        assert!(
            text.contains("\"id\": \"plain-task\""),
            "response should describe the same task, in place; got: {text}"
        );
        assert!(
            text.contains("in_progress"),
            "task should be moved to in_progress; got: {text}"
        );

        // No new file was created — exactly the original task file remains.
        let task_files: Vec<_> = std::fs::read_dir(&tasks_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("plain-task"))
            .collect();
        assert_eq!(
            task_files.len(),
            1,
            "in-place claim must not create a copy; found: {:?}",
            task_files.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );

        // On-disk frontmatter reflects the claim.
        let content = std::fs::read_to_string(tasks_dir.join("plain-task.md")).unwrap();
        assert!(
            content.contains("status: in_progress"),
            "on-disk status must be in_progress; got: {content}"
        );
        assert!(
            content.contains("assignee: polecat"),
            "on-disk assignee must be set from the claim call; got: {content}"
        );
    }

    /// An untyped node (no `type:` field) is treated as a plain task
    /// elsewhere in this codebase (see the `unwrap_or_else(|| "task")`
    /// prefix computation) — claim_task must honor the same default and
    /// claim it in place rather than rejecting it.
    #[test]
    fn claim_task_claims_untyped_node_as_task() {
        let (server, tmp) = setup_with_template();

        let tasks_dir = tmp.path().join("tasks");
        let task_content = "\
---
id: untyped-task
title: \"Untyped Task\"
status: queued
parent: proj-test
project: aops
---

## Body
";
        std::fs::write(tasks_dir.join("untyped-task.md"), task_content).unwrap();
        {
            let graph = GraphStore::build_from_directory(tmp.path());
            *server.graph.write() = graph;
        }

        let result = server
            .handle_claim_task(&serde_json::json!({ "id": "untyped-task" }))
            .expect("claiming an untyped node must succeed (defaults to task semantics)");

        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .next()
            .expect("result should have text content");
        assert!(
            text.contains("in_progress"),
            "task should be moved to in_progress; got: {text}"
        );
    }

    #[test]
    fn claim_task_rejects_unknown_id() {
        let (server, _tmp) = setup_with_template();

        let err = server
            .handle_claim_task(&serde_json::json!({ "id": "nonexistent-id" }))
            .expect_err("should fail on unknown id");

        assert!(
            err.message.contains("not found"),
            "error should indicate task not found; got: {}",
            err.message
        );
    }

    #[test]
    fn template_type_not_in_ready_queue() {
        let (server, _tmp) = setup_with_template();
        let ready = server.graph.read().ready_ids().to_vec();
        assert!(
            !ready.contains(&"daily-template".to_string()),
            "template should not appear in ready queue; ready={ready:?}"
        );
    }

    // ── M2/M3 ad-hoc grouping tests ──

    /// With a session_id, release_task(no id) should create a per-session epic
    /// and parent the task under it, not directly under adhoc-sessions root.
    #[test]
    fn test_adhoc_release_with_session_id_creates_session_epic() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let result = server
            .handle_release_task(&serde_json::json!({
                "project": "adhoc-sessions",
                "status": "done",
                "summary": "Fixed the retry logic in queue consumer",
                "session_id": "abc12345",
            }))
            .expect("release_task without id should succeed");

        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();

        // The response should include the created task id
        let val: serde_json::Value = serde_json::from_str(&text).unwrap();
        let task_id = val.get("id").and_then(|v| v.as_str()).unwrap().to_string();

        // The task should exist and be parented under a session epic (not the adhoc root directly)
        let graph = server.graph.read();
        let node = graph
            .resolve(&task_id)
            .expect("created task should be in graph");
        let parent = node.parent.as_deref().unwrap_or("");

        // Parent must NOT be the flat adhoc-sessions root — it must be a session epic
        assert_ne!(
            parent,
            crate::document_crud::ADHOC_SESSIONS_ROOT_ID,
            "task should be parented under a session epic, not directly under adhoc root; parent={parent}"
        );
        // Parent ID should follow the adhoc-{md5[..8]} pattern
        assert!(
            parent.starts_with("adhoc_"),
            "session epic parent should have adhoc_ prefix; parent={parent}"
        );

        // The session epic itself should exist in the graph as type: epic
        let epic_node = graph
            .resolve(parent)
            .expect("session epic should be in graph");
        assert_eq!(
            epic_node.node_type.as_deref(),
            Some("epic"),
            "session epic should have type=epic"
        );
        // Epic's parent should be the adhoc-sessions root
        assert_eq!(
            epic_node.parent.as_deref(),
            Some(crate::document_crud::ADHOC_SESSIONS_ROOT_ID),
            "session epic should be parented under adhoc-sessions root"
        );
    }

    /// Two release_task calls with the same session_id should create only one
    /// session epic and both tasks should share it as their parent.
    #[test]
    fn test_adhoc_same_session_reuses_epic() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let sid = "deadbeef";

        let r1 = server
            .handle_release_task(&serde_json::json!({
                "project": "adhoc-sessions",
                "status": "done",
                "summary": "First task in this session",
                "session_id": sid,
            }))
            .unwrap();
        let r2 = server
            .handle_release_task(&serde_json::json!({
                "project": "adhoc-sessions",
                "status": "done",
                "summary": "Second task in this session",
                "session_id": sid,
            }))
            .unwrap();

        let id1: String = {
            let t: String = r1
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            serde_json::from_str::<serde_json::Value>(&t)
                .unwrap()
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string()
        };
        let id2: String = {
            let t: String = r2
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            serde_json::from_str::<serde_json::Value>(&t)
                .unwrap()
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string()
        };

        let graph = server.graph.read();
        let parent1 = graph
            .resolve(&id1)
            .unwrap()
            .parent
            .clone()
            .unwrap_or_default();
        let parent2 = graph
            .resolve(&id2)
            .unwrap()
            .parent
            .clone()
            .unwrap_or_default();

        // Both tasks must share the same session epic
        assert_eq!(
            parent1, parent2,
            "both tasks from the same session should share the same parent epic; parent1={parent1}, parent2={parent2}"
        );
        // Only one SESSION epic should exist with an adhoc- prefix in the graph.
        // The adhoc-sessions bootstrap root is itself `type: epic` now (project
        // is no longer a node type), so exclude it from the count.
        let epic_count = graph
            .nodes()
            .filter(|n| {
                n.node_type.as_deref() == Some("epic")
                    && n.id.starts_with("adhoc_")
                    && n.id != crate::document_crud::ADHOC_SESSIONS_ROOT_ID
            })
            .count();
        assert_eq!(
            epic_count, 1,
            "exactly one session epic should be created for a single session_id; got {epic_count}"
        );
    }

    /// Without a session_id and with the dummy embedder (all zero scores, always
    /// below M3 threshold), release_task falls back to the adhoc-sessions root.
    #[test]
    fn test_adhoc_no_session_id_falls_back_to_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let result = server
            .handle_release_task(&serde_json::json!({
                "project": "adhoc-sessions",
                "status": "done",
                "summary": "Some ad-hoc work with no session tracking",
            }))
            .unwrap();

        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let task_id: String = serde_json::from_str::<serde_json::Value>(&text)
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let graph = server.graph.read();
        let parent = graph
            .resolve(&task_id)
            .unwrap()
            .parent
            .clone()
            .unwrap_or_default();
        assert_eq!(
            parent,
            crate::document_crud::ADHOC_SESSIONS_ROOT_ID,
            "without session_id, task should fall back to adhoc-sessions root; parent={parent}"
        );
    }

    #[test]
    fn test_adhoc_release_missing_project_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let err = server
            .handle_release_task(&serde_json::json!({
                "status": "done",
                "summary": "Some ad-hoc work without project parameter",
            }))
            .unwrap_err();

        assert!(
            err.message.contains("Missing required parameter: project"),
            "ad-hoc release without project must fail with missing project parameter error, got: {}",
            err.message
        );
    }

    #[test]
    fn test_adhoc_release_empty_project_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let err = server
            .handle_release_task(&serde_json::json!({
                "project": "   ",
                "status": "done",
                "summary": "Some ad-hoc work with whitespace project parameter",
            }))
            .unwrap_err();

        assert!(
            err.message.contains("Missing required parameter: project"),
            "ad-hoc release with empty project must fail, got: {}",
            err.message
        );
    }

    #[test]
    fn test_adhoc_release_with_long_summary_produces_short_id_and_bounded_filename() {
        // Regression test for mem-4a068cea:
        // release_task(status="done", summary=<500-char prose>) with no id
        // must produce an ID stem <= ~50 chars and on-disk filename stem within bound.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let long_summary = "triaged remaining 7 e2e failures in tests e2e test all invocation paths py post pr 921 polecat yaml ssot mapped to existing tasks task ac3e547b workspace bind mount scope 4 2 run only and task 376fd490 python viz subagent tools and cut 3 new tasks aops 4434236d entrypoint sh detached launch missing tests 1 1 aops f394236e run command test expects error but gets subagent launch and aops 9a871234 other long prose description that exceeds several hundred characters in length to thoroughly test slug truncation and bounded id stem generation";
        assert!(long_summary.len() > 400);

        let result = server
            .handle_release_task(&serde_json::json!({
                "project": "adhoc-sessions",
                "status": "done",
                "summary": long_summary,
            }))
            .expect("release_task with long summary and project should succeed");

        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let val: serde_json::Value = serde_json::from_str(&text).unwrap();
        let task_id = val.get("id").and_then(|v| v.as_str()).unwrap().to_string();

        // 1. ID stem must be short (<= 50 chars), matching create_task convention
        assert!(
            task_id.len() <= 50,
            "created task ID must be <= 50 chars, got {} (length {})",
            task_id,
            task_id.len()
        );
        assert!(
            task_id.starts_with("adhoc_sessions_"),
            "task ID should start with project prefix, got {task_id}"
        );

        // 2. The task file must exist on disk with bounded filename
        let tasks_dir = root.join("tasks");
        let entries: Vec<_> = std::fs::read_dir(&tasks_dir)
            .unwrap()
            .map(|r| r.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        let matching_file = entries
            .iter()
            .find(|f| f.starts_with(&task_id))
            .expect("task file starting with task_id must exist on disk");

        // Filename stem (excluding .md) has slug capped at 80 chars
        let stem = matching_file.strip_suffix(".md").unwrap();
        let slug_part = stem.strip_prefix(&format!("{task_id}-")).unwrap_or(stem);
        assert!(
            slug_part.len() <= 80,
            "slug portion of filename must be <= 80 chars, got {} (length {})",
            slug_part,
            slug_part.len()
        );

        // 3. Frontmatter id matches the short/truncated form
        let file_content = std::fs::read_to_string(tasks_dir.join(matching_file)).unwrap();
        assert!(
            file_content.contains(&format!("id: {task_id}")),
            "frontmatter id must match generated task_id: {file_content}"
        );
        assert!(
            file_content.contains("project: adhoc-sessions"),
            "frontmatter project must be adhoc-sessions: {file_content}"
        );
        assert!(
            file_content.contains("status: done"),
            "frontmatter status must be done: {file_content}"
        );
    }

    #[test]
    fn test_tool_schemas_match_valid_node_types() {
        let tools = PkbSearchServer::get_all_tools();
        let get_tool = |name: &str| tools.iter().find(|t| t.name.as_ref() == name).unwrap();
        let get_enum = |tool: &rmcp::model::Tool, prop: &str| -> Vec<String> {
            tool.input_schema.get("properties").unwrap()
                .get(prop).unwrap()
                .get("enum").unwrap()
                .as_array().unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };

        let create_task = get_tool("create_task");
        let task_types = get_enum(create_task, "type");
        for t in task_types {
            assert!(crate::graph::is_valid_node_type(&t), "create_task schema advertises invalid type: {t}");
        }

        let create_memory = get_tool("create_memory");
        let memory_types = get_enum(create_memory, "memory_type");
        for t in memory_types {
            assert!(crate::graph::is_valid_node_type(&t), "create_memory schema advertises invalid memory_type: {t}");
        }

        let create = get_tool("create");
        let create_types = get_enum(create, "type");
        for t in create_types {
            assert!(crate::graph::is_valid_node_type(&t), "create schema advertises invalid type: {t}");
        }
    }

    #[test]
    fn test_all_tools_synchronized_with_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let tools = PkbSearchServer::get_all_tools();
        assert!(tools.iter().any(|t| t.name.as_ref() == "graph_excalidraw"));
        assert!(tools.iter().any(|t| t.name.as_ref() == "diff_excalidraw"));
        assert!(tools.iter().any(|t| t.name.as_ref() == "sync_excalidraw"));

        // Verify that every single tool registered in get_all_tools has a dispatch arm
        for tool in &tools {
            let res = server.dispatch_tool_sync(tool.name.as_ref(), &serde_json::json!({}));
            if let Err(ref e) = res {
                assert_ne!(
                    e.code,
                    rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                    "Tool '{}' is registered in list_tools but missing from dispatch_tool_sync!",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn test_excalidraw_mcp_tools_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        // Seed initial task file
        let task_file = root.join("tasks").join("task-init.md");
        std::fs::write(
            &task_file,
            "---\nid: task-init\ntitle: Initial Task\ntype: task\nstatus: ready\n---\n\nInitial task body.",
        ).unwrap();

        let doc = crate::pkb::parse_file_relative(&task_file, root).unwrap();
        let graph = GraphStore::build(&[doc], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        // 1. Test graph_excalidraw
        let res = server.handle_graph_excalidraw(&serde_json::json!({
            "node_id": "task-init",
            "hops": 1
        })).expect("graph_excalidraw should succeed");
        let text: String = res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect();
        assert!(text.contains("Initial Task"), "diagram JSON should contain Initial Task");

        // 2. Test diff_excalidraw
        let diff_res = server.handle_diff_excalidraw(&serde_json::json!({
            "canvas": text
        })).expect("diff_excalidraw should succeed");
        let diff_text: String = diff_res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect();
        let diff: crate::excalidraw::GraphDiff = serde_json::from_str(&diff_text).expect("valid diff json");
        assert!(diff.is_empty(), "clean canvas diff should be empty");

        // 3. Test sync_excalidraw dry_run
        let sync_dry_res = server.handle_sync_excalidraw(&serde_json::json!({
            "canvas": text,
            "dry_run": true
        })).expect("sync dry_run should succeed");
        let dry_text: String = sync_dry_res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect();
        assert!(dry_text.contains("Dry run"), "dry run response mentions dry run");
    }

    #[test]
    fn test_status_reports_build_and_operational_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let task_file = root.join("tasks").join("task-1.md");
        std::fs::write(
            &task_file,
            "---\nid: task-1\ntitle: Task One\ntype: task\nstatus: ready\n---\n\nTask body.",
        )
        .unwrap();

        let doc = crate::pkb::parse_file_relative(&task_file, root).unwrap();
        let graph = GraphStore::build(&[doc], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        let res = server
            .handle_status(&serde_json::json!({}))
            .expect("status should succeed");
        let text: String = res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid status JSON");

        // 1. Build identity
        assert_eq!(parsed["name"], env!("CARGO_PKG_NAME"));
        assert_eq!(parsed["version"], env!("CARGO_PKG_VERSION"));
        assert!(parsed.get("git_hash").is_some());
        assert!(parsed.get("build_profile").is_some());

        // 2. Index state
        assert_eq!(parsed["index"]["document_count"], 1);
        assert_eq!(parsed["index"]["vector_count"], 0);
        assert_eq!(parsed["index"]["last_reindex"]["outcome"], "ok");
        assert!(parsed["index"]["last_reindex"]["timestamp"].is_string());

        // 3. Queue state
        assert_eq!(parsed["queue"]["depth"], 0);
        assert_eq!(parsed["queue"]["embed_pending"], 0);
        assert_eq!(parsed["queue"]["deferred_paths"], 0);

        // 4. Write state
        assert_eq!(parsed["write_state"]["index_locked"], false);
        assert_eq!(parsed["write_state"]["external_lock_held"], false);
        assert_eq!(parsed["write_state"]["save_in_flight"], false);
        assert_eq!(parsed["write_state"]["embed_worker_running"], false);
        assert_eq!(parsed["write_state"]["graph_rebuild_pending"], false);

        // 5. Freshness
        assert_eq!(parsed["freshness"]["is_fresh"], false);
        assert_eq!(parsed["freshness"]["stale_documents"], 1);
    }

    #[test]
    fn test_status_values_are_live_on_state_mutations() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let task1_file = root.join("tasks").join("task-1.md");
        std::fs::write(
            &task1_file,
            "---\nid: task-1\ntitle: Task One\ntype: task\nstatus: ready\n---\n\nTask body.",
        )
        .unwrap();

        let doc1 = crate::pkb::parse_file_relative(&task1_file, root).unwrap();
        let graph = GraphStore::build(&[doc1], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let db_path = root.join("db.bin");
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            db_path.clone(),
            Arc::new(RwLock::new(graph)),
        );

        // Helper to query status JSON
        let get_status = || -> serde_json::Value {
            let res = server
                .handle_status(&serde_json::json!({}))
                .expect("status call succeeds");
            let text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            serde_json::from_str(&text).expect("valid status json")
        };

        // Initially 1 doc on disk, 0 in vector store -> 1 stale doc
        let s0 = get_status();
        assert_eq!(s0["index"]["document_count"], 1);
        assert_eq!(s0["freshness"]["stale_documents"], 1);
        assert_eq!(s0["freshness"]["is_fresh"], false);

        // Mutation 1: Write a second document to disk -> immediately live in freshness
        let task2_file = root.join("tasks").join("task-2.md");
        std::fs::write(
            &task2_file,
            "---\nid: task-2\ntitle: Task Two\ntype: task\nstatus: ready\n---\n\nTask two body.",
        )
        .unwrap();

        let s1 = get_status();
        assert_eq!(s1["freshness"]["stale_documents"], 2);
        assert_eq!(s1["freshness"]["is_fresh"], false);

        // Mutation 2: Refresh graph index -> immediately reflects updated document_count and last_reindex timestamp
        let initial_reindex_ts = s1["index"]["last_reindex"]["timestamp"].as_str().unwrap().to_string();
        std::thread::sleep(std::time::Duration::from_millis(10));
        server
            .handle_refresh_graph(&serde_json::json!({}))
            .expect("refresh_graph succeeds");

        let s2 = get_status();
        assert_eq!(s2["index"]["document_count"], 2);
        assert_ne!(
            s2["index"]["last_reindex"]["timestamp"].as_str().unwrap(),
            initial_reindex_ts,
            "last_reindex timestamp should update on refresh_graph"
        );

        // Mutation 3: Insert into embed_pending queue -> immediately live in queue diagnostics
        let doc2 = crate::pkb::parse_file_relative(&task2_file, root).unwrap();
        server.embed_pending.lock().insert("tasks/task-2.md".to_string(), doc2);

        let s3 = get_status();
        assert_eq!(s3["queue"]["depth"], 1);
        assert_eq!(s3["queue"]["embed_pending"], 1);

        // Mutation 4: Acquire cross-process file lock -> immediately live in write_state
        {
            let mut file_lock = VectorStore::acquire_lock(&db_path).expect("lock file creates");
            let _guard = file_lock.write().expect("lock acquired");

            let s4 = get_status();
            assert_eq!(s4["write_state"]["index_locked"], true);
            assert_eq!(s4["write_state"]["external_lock_held"], true);
        }

        // Lock released -> immediately false
        let s5 = get_status();
        assert_eq!(s5["write_state"]["index_locked"], false);
        assert_eq!(s5["write_state"]["external_lock_held"], false);

        // Mutation 5: In-flight write states reflect live atomic flags
        server.save_in_flight.store(true, Ordering::SeqCst);
        let s6 = get_status();
        assert_eq!(s6["write_state"]["save_in_flight"], true);

        server.save_in_flight.store(false, Ordering::SeqCst);
        let s7 = get_status();
        assert_eq!(s7["write_state"]["save_in_flight"], false);
    }
}

