use super::*;

    #[test]
    fn test_partial_status_release_and_list_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        write_test_polecat_yaml(root);

        let graph = GraphStore::build(&[], root);
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

        // helper: pull the JSON object out of a handler result and read a field
        let id_from = |res: &CallToolResult| -> String {
            let text = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect::<String>();
            let val: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|_| panic!("create_task should return JSON, got: {text}"));
            val.get("id").and_then(|v| v.as_str()).unwrap().to_string()
        };

        // 1. create the task that will be released `partial`
        let partial_id = id_from(
            &server
                .handle_create_task(&json!({
                    "title": "Partial round-trip task",
                    "type": "task",
                    "project": "proj-partial",
                    "parent": "proj-partial",
                    "allow_missing_parent": true,
                }))
                .unwrap(),
        );

        // 2. create a control task that stays open (must NOT leak into the filter)
        let control_id = id_from(
            &server
                .handle_create_task(&json!({
                    "title": "Control open task",
                    "type": "task",
                    "project": "proj-partial",
                    "parent": "proj-partial",
                    "allow_missing_parent": true,
                }))
                .unwrap(),
        );

        // 3. release the first task as `partial` — must be accepted, not rejected
        let released = server
            .handle_release_task(&json!({
                "id": partial_id,
                "status": "partial",
                "summary": "Shipped a clean scope-seam slice; remainder tracked in a live follow-up.",
                "reason": "Remainder scoped out for a follow-up task; not needed for this round-trip check.",
            }))
            .expect("release_task(status=\"partial\") must be accepted");
        let released_text = released
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            released_text.contains("partial"),
            "release response should confirm the partial transition: {released_text}"
        );

        // 4. list_tasks(status="partial") must contain the partial task …
        let listed = server
            .handle_list_tasks(&json!({"status": "partial", "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&listed);
        assert!(
            ids.contains(&partial_id),
            "partial task {partial_id} must appear in status=partial filter, got {ids:?}"
        );
        // … and NOT the still-open control — proving the filter is real, not match-all.
        assert!(
            !ids.contains(&control_id),
            "non-partial control {control_id} must NOT appear in status=partial filter \
             (guards against the old alias/match-everything behaviour), got {ids:?}"
        );
    }

    // ── create_task: project required ──

    #[test]
    fn test_create_task_severity_coercion() {
        let server = build_test_server();
        std::fs::create_dir_all("/tmp/test-pkb-project/targets").unwrap();
        std::fs::create_dir_all("/tmp/test-pkb-project/tasks").unwrap();
        // 1. Create a target task with severity
        let res_target = server
            .handle_create_task(&json!({
                "title": "target with sev",
                "type": "target",
                "severity": 3,
                "project": "proj-alpha",
                "parent": "proj-alpha"
            }))
            .unwrap();
        let target_text = res_target
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        let target_val: serde_json::Value = serde_json::from_str(&target_text).unwrap();
        let target_id = target_val.get("id").unwrap().as_str().unwrap().to_string();

        let get_target = server.handle_get_task(&json!({"id": target_id})).unwrap();
        let get_target_text = get_target
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            get_target_text.contains("\"severity\": 3"),
            "Target node should retain severity. text: {}",
            get_target_text
        );

        // 2. Create a standard task with severity
        let res_task = server
            .handle_create_task(&json!({
                "title": "task with sev",
                "type": "task",
                "severity": 3,
                "project": "proj-alpha",
                "parent": "proj-alpha"
            }))
            .unwrap();

        let mut has_warning = false;
        let mut task_json_text = String::new();
        for c in &res_task.content {
            if let Some(t) = c.raw.as_text() {
                if t.text.contains("severity ignored: not a target node") {
                    has_warning = true;
                } else if t.text.trim().starts_with('{') {
                    task_json_text = t.text.clone();
                }
            }
        }
        assert!(
            has_warning,
            "Standard task should return a warning about severity coercion"
        );

        let task_val: serde_json::Value = serde_json::from_str(&task_json_text).unwrap();
        let task_id = task_val.get("id").unwrap().as_str().unwrap().to_string();
        let get_task = server.handle_get_task(&json!({"id": task_id})).unwrap();
        let get_task_text = get_task
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            !get_task_text.contains("\"severity\": 3"),
            "Standard task should coerce severity to 0 or null"
        );
    }

    #[test]
    fn test_decompose_task_severity_coercion() {
        let server = build_test_server();
        std::fs::create_dir_all("/tmp/test-pkb-project/tasks").unwrap();

        // Create a real parent task so it has a project field and exists on disk
        let parent_res = server
            .handle_create_task(&json!({
                "title": "Parent Task for Decompose",
                "type": "epic",
                "project": "proj-alpha",
                "parent": "proj-alpha"
            }))
            .unwrap();

        let mut parent_id = String::new();
        for c in &parent_res.content {
            if let Some(t) = c.raw.as_text() {
                if t.text.trim().starts_with('{') {
                    let parent_val: serde_json::Value = serde_json::from_str(&t.text).unwrap();
                    parent_id = parent_val.get("id").unwrap().as_str().unwrap().to_string();
                }
            }
        }

        let subtasks = json!([
            {
                "title": "Subtask Target Sev",
                "type": "target",
                "severity": 3
            },
            {
                "title": "Subtask Sev",
                "type": "task",
                "severity": 3
            }
        ]);

        let res = server
            .handle_decompose_task(&json!({
                "parent_id": parent_id,
                "subtasks": subtasks
            }))
            .unwrap();

        let res_text = res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            res_text.contains("severity ignored for one or more non-target nodes"),
            "Should have warning"
        );

        let graph = server.graph.read();

        let target_node = graph.resolve("Subtask Target Sev").unwrap();
        assert_eq!(target_node.severity, Some(3));

        let subtask_node = graph.resolve("Subtask Sev").unwrap();
        assert_eq!(subtask_node.severity, Some(0)); // Coerced to 0
    }

    #[test]
    fn test_decompose_task_rejects_agent_priority() {
        let server = build_test_server();
        std::fs::create_dir_all("/tmp/test-pkb-project/tasks").unwrap();

        let parent_res = server
            .handle_create_task(&json!({
                "title": "Parent Task for Decompose Priority Guard",
                "type": "epic",
                "project": "proj-alpha",
                "parent": "proj-alpha"
            }))
            .unwrap();

        let mut parent_id = String::new();
        for c in &parent_res.content {
            if let Some(t) = c.raw.as_text() {
                if t.text.trim().starts_with('{') {
                    let parent_val: serde_json::Value = serde_json::from_str(&t.text).unwrap();
                    parent_id = parent_val.get("id").unwrap().as_str().unwrap().to_string();
                }
            }
        }

        let subtasks = json!([
            { "title": "Subtask With Priority", "type": "task", "priority": 1 }
        ]);

        let err = server
            .handle_decompose_task(&json!({
                "parent_id": parent_id,
                "subtasks": subtasks
            }))
            .expect_err("agent-supplied subtask priority via decompose_task should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("priority"),
            "error should mention priority, got: {msg}"
        );

        // No subtask should have been created (fail before any writes).
        let graph = server.graph.read();
        assert!(
            graph.resolve("Subtask With Priority").is_none(),
            "subtask must not have been created when priority guard rejects the batch"
        );
    }

    // ── epic_882b7576: soft session-identity display on subtasks (D1, display-only) ──

    #[test]
    fn test_subtask_identity_visible_on_get_task_and_children() {
        let server = build_test_server();
        std::fs::create_dir_all("/tmp/test-pkb-project/tasks").unwrap();

        let parent_res = server
            .handle_create_task(&json!({
                "title": "Parent Epic for Identity Display",
                "type": "epic",
                "project": "proj-alpha",
                "parent": "proj-alpha"
            }))
            .unwrap();
        let mut parent_id = String::new();
        for c in &parent_res.content {
            if let Some(t) = c.raw.as_text() {
                if t.text.trim().starts_with('{') {
                    let v: serde_json::Value = serde_json::from_str(&t.text).unwrap();
                    parent_id = v.get("id").unwrap().as_str().unwrap().to_string();
                }
            }
        }
        assert!(
            !parent_id.is_empty(),
            "parent task should have been created"
        );

        // Decompose into an executor subtask and a reviewer subtask, each
        // claimed by a distinct session/agent identity via `assignee`.
        let subtasks = json!([
            { "title": "Executor Subtask", "type": "task", "assignee": "session-executor-A" },
            { "title": "Reviewer Subtask", "type": "task", "assignee": "session-reviewer-B" }
        ]);
        server
            .handle_decompose_task(&json!({
                "parent_id": parent_id,
                "subtasks": subtasks
            }))
            .unwrap();

        // AC1: identity visible on get_task's child-task listing (decompose_task's
        // default node_type "task" lands in `children`, not the separate
        // node_type=="subtask" `subtasks` array).
        let get_res = server.handle_get_task(&json!({"id": parent_id})).unwrap();
        let get_text = get_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        let get_val: serde_json::Value = serde_json::from_str(&get_text).unwrap();
        let subtask_list = get_val
            .get("children")
            .and_then(|s| s.as_array())
            .expect("children array should be present");
        assert_eq!(subtask_list.len(), 2);
        let assignees: Vec<Option<&str>> = subtask_list
            .iter()
            .map(|s| s.get("assignee").and_then(|a| a.as_str()))
            .collect();
        assert!(
            assignees.contains(&Some("session-executor-A")),
            "get_task children should surface executor identity, got: {get_text}"
        );
        assert!(
            assignees.contains(&Some("session-reviewer-B")),
            "get_task children should surface reviewer identity, got: {get_text}"
        );

        // AC1 (continued): identity also visible on get_task_children listing.
        let children_res = server
            .handle_get_task_children(&json!({"id": parent_id}))
            .unwrap();
        let children_text = children_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            children_text.contains("(@session-executor-A)"),
            "get_task_children should display executor identity, got: {children_text}"
        );
        assert!(
            children_text.contains("(@session-reviewer-B)"),
            "get_task_children should display reviewer identity, got: {children_text}"
        );

        // AC2: zero blocking behaviour — a review subtask claimed by the SAME
        // identity as the executor must still release cleanly (D1: display
        // only, never a hard gate on identity match).
        let reviewer_task_id = subtask_list
            .iter()
            .find(|s| s.get("title").and_then(|t| t.as_str()) == Some("Reviewer Subtask"))
            .and_then(|s| s.get("id"))
            .and_then(|i| i.as_str())
            .unwrap()
            .to_string();

        // Re-claim the reviewer subtask under the *same* identity that owns
        // the executor subtask, simulating a reviewer==executor collision.
        server
            .handle_update_task(&json!({
                "id": reviewer_task_id,
                "updates": { "assignee": "session-executor-A" }
            }))
            .unwrap();

        let release_res = server.handle_release_task(&json!({
            "id": reviewer_task_id,
            "status": "merge_ready",
            "summary": "same-identity release should not be blocked (D1)"
        }));
        assert!(
            release_res.is_ok(),
            "same-identity review subtask must still release cleanly under D1: {release_res:?}"
        );
        let release_res = release_res.unwrap();
        assert!(
            !release_res.is_error.unwrap_or(false),
            "release_task should not return an error result for same-identity release"
        );
    }

    #[test]
    fn test_create_task_allows_missing_project() {
        let server = build_test_server();
        let result = server
            .handle_create_task(&json!({
                "title": "no project",
                "parent": "proj-alpha"
            }))
            .expect("missing project should be allowed when parent resolves project");
        assert!(!result.is_error.unwrap_or(false));
    }

    #[test]
    fn test_create_task_allows_blank_project() {
        let server = build_test_server();
        let result = server
            .handle_create_task(&json!({
                "title": "blank project",
                "parent": "proj-alpha",
                "project": "   "
            }))
            .expect("blank project should be allowed when parent resolves project");
        assert!(!result.is_error.unwrap_or(false));
    }

    #[test]
    fn test_create_task_schema_does_not_require_project() {
        let tools = PkbSearchServer::get_all_tools();
        let create = tools
            .iter()
            .find(|t| t.name.as_ref() == "create_task")
            .expect("create_task tool should exist");
        let schema = serde_json::to_string(&create.input_schema).unwrap();
        assert!(
            schema.contains("\"project\""),
            "create_task schema should include project field"
        );
        assert!(
            !schema.contains("\"required\":[\"title\",\"project\"]")
                && !schema.contains("\"required\": [\"title\", \"project\"]"),
            "create_task should not mark project required, got: {schema}"
        );
    }

    // ── update_task: parent cycle rejection ──

    #[test]
    fn test_update_task_rejects_self_parent() {
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "task-a1",
                "parent": "task-a1"
            }))
            .expect_err("self-parent should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("cycle") || msg.to_lowercase().contains("own parent"),
            "error should mention cycle/own-parent, got: {msg}"
        );
    }

    // ── update_task: agent-originated priority rejection (task_1381381c) ──

    #[test]
    fn test_update_task_rejects_agent_priority_nested() {
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "task-a1",
                "updates": { "priority": 1 }
            }))
            .expect_err("agent-supplied priority via update_task should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("priority"),
            "error should mention priority, got: {msg}"
        );
    }

    #[test]
    fn test_update_task_rejects_agent_priority_flat() {
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "task-a1",
                "priority": 0
            }))
            .expect_err("agent-supplied priority via update_task (flat form) should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("priority"),
            "error should mention priority, got: {msg}"
        );

        // The task's priority must be unchanged on disk/graph after the rejection.
        let graph = server.graph.read();
        let node = graph.resolve("task-a1").unwrap();
        assert_ne!(
            node.priority,
            Some(0),
            "priority must not have been written despite rejection"
        );
    }

    #[test]
    fn test_update_task_rejects_descendant_parent() {
        // proj-alpha is parent of task-a1. Setting proj-alpha's parent to
        // task-a1 would create a cycle proj-alpha → task-a1 → proj-alpha.
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "proj-alpha",
                "parent": "task-a1"
            }))
            .expect_err("descendant parent should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("cycle") || msg.to_lowercase().contains("circular"),
            "error should mention cycle/circular, got: {msg}"
        );
    }

    // ── Bug 2 (task-d802855c): unparent removes parent, never writes junk ──

    /// Build a server backed by a real temp directory with the given
    /// `<relative-path>, <full-file-contents>` task files on disk, so
    /// handle_update_task can actually read and rewrite them.
    pub(crate) fn build_disk_backed_server(files: &[(&str, &str)]) -> (tempfile::TempDir, PkbSearchServer) {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        for (rel, contents) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
        }
        let docs: Vec<PkbDocument> = crate::pkb::scan_directory(&root)
            .iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, &root))
            .collect();
        let graph = GraphStore::build(&docs, &root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.clone(),
            root.join("db"),
            Arc::new(RwLock::new(graph)),
        );
        (tmp, server)
    }

    #[test]
    fn test_update_task_unparent_top_level_removes_parent_no_junk() {
        let (tmp, server) = build_disk_backed_server(&[
            ("tasks/task-child.md", "---\nid: task-child\ntitle: Child\ntype: task\nstatus: ready\nparent: task-parent\n---\n\n# Child\n"),
            ("tasks/task-parent.md", "---\nid: task-parent\ntitle: Parent\ntype: task\nstatus: ready\n---\n\n# Parent\n"),
        ]);

        // Top-level form: was previously rejected with "No fields to update".
        server
            .handle_update_task(&json!({"id": "task-child", "unparent": true}))
            .expect("unparent=true should succeed");

        let content = std::fs::read_to_string(tmp.path().join("tasks/task-child.md")).unwrap();
        assert!(
            !content.contains("parent:"),
            "parent key must be gone, got:\n{content}"
        );
        assert!(
            !content.contains("unparent"),
            "unparent must never be persisted, got:\n{content}"
        );
        // Round-trip: the rest of the frontmatter survives.
        assert!(
            content.contains("id: task-child"),
            "id preserved:\n{content}"
        );
        assert!(
            content.contains("title: Child"),
            "title preserved:\n{content}"
        );
        assert!(content.contains("type: task"), "type preserved:\n{content}");
    }

    #[test]
    fn test_update_task_unparent_nested_form_removes_parent_no_junk() {
        let (tmp, server) = build_disk_backed_server(&[
            ("tasks/task-child.md", "---\nid: task-child\ntitle: Child\ntype: task\nstatus: ready\nparent: task-parent\n---\n\n# Child\n"),
            ("tasks/task-parent.md", "---\nid: task-parent\ntitle: Parent\ntype: task\nstatus: ready\n---\n\n# Parent\n"),
        ]);

        // Nested form: previously wrote a literal `unparent: true` frontmatter
        // key and left `parent` in place.
        server
            .handle_update_task(&json!({"id": "task-child", "updates": {"unparent": true}}))
            .expect("nested unparent should succeed");

        let content = std::fs::read_to_string(tmp.path().join("tasks/task-child.md")).unwrap();
        assert!(
            !content.contains("parent:"),
            "parent key must be gone, got:\n{content}"
        );
        assert!(
            !content.contains("unparent"),
            "unparent must never be persisted, got:\n{content}"
        );
        assert!(
            content.contains("id: task-child"),
            "id preserved:\n{content}"
        );
    }

    #[test]
    fn test_update_task_bare_parent_null_still_requires_unparent_flag() {
        let (_tmp, server) = build_disk_backed_server(&[
            ("tasks/task-child.md", "---\nid: task-child\ntitle: Child\ntype: task\nstatus: ready\nparent: task-parent\n---\n\n# Child\n"),
            ("tasks/task-parent.md", "---\nid: task-parent\ntitle: Parent\ntype: task\nstatus: ready\n---\n\n# Parent\n"),
        ]);

        // Setting parent:null WITHOUT the unparent flag must still be rejected
        // (guards against accidental parent clearing).
        let err = server
            .handle_update_task(&json!({"id": "task-child", "updates": {"parent": null}}))
            .expect_err("bare parent:null should require explicit unparent");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("unparent"),
            "error should direct caller to pass unparent=true, got: {msg}"
        );
    }

    // ── task_search: schema advertises include_done ──

    #[test]

    // ── Closed-parent validation (task-8f232401) ──────────────────────────────

    #[test]
    fn test_create_task_rejects_closed_parent() {
        // The test graph has task-a3 with status=done.
        // Creating a child under a done parent should be rejected.
        let server = build_test_server();
        let err = server
            .handle_create_task(&json!({
                "title": "child of done parent",
                "project": "test",
                "parent": "task-a3"
            }))
            .expect_err("should reject create under done parent");
        let msg = err.message.to_string();
        assert!(
            msg.to_lowercase().contains("closed") || msg.contains("done"),
            "error should mention closed/done status; got: {msg}"
        );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
    }

    #[test]
    fn test_create_task_allows_force_override_for_closed_parent() {
        // With force=true, creating under a done parent should not be rejected by
        // the graph validation. It will still fail on disk I/O (test server has
        // no real pkb_root), but must NOT fail on the closed-parent check.
        let server = build_test_server();
        let result = server.handle_create_task(&json!({
            "title": "child of done parent with force",
            "project": "test",
            "parent": "task-a3",
            "force": true
        }));
        // The test server's pkb_root (/tmp/test-pkb-project) likely does not have
        // real dirs, so we expect an error — but it must be a disk/IO error, NOT
        // the closed-parent error.
        match result {
            Ok(_) => { /* if disk happens to succeed, that's fine */ }
            Err(e) => {
                let msg = e.message.to_string();
                assert!(
                    !msg.to_lowercase().contains("closed"),
                    "force=true should bypass the closed-parent check; got: {msg}"
                );
            }
        }
    }

    // ── Open-children validation (task-8f232401) ──────────────────────────────

    /// Regression test for task-31a499fe:
    ///
    /// Scenario: oldParent has exactly one open child. Agent reparents the child
    /// to newParent. The close-gate must immediately reflect the reparent so that
    /// `complete_task(oldParent)` succeeds without waiting for the Tier-2 rebuild.
    ///
    /// Before the fix, `upsert_node_in_place` updated the child's `parent` field
    /// but left `oldParent.children` stale; `open_descendants(oldParent)` still
    /// found the child, causing `complete_task` to reject with "open children".
    #[test]
    fn test_reparent_only_child_then_complete_old_parent_succeeds() {
        let (tmp, server) = build_disk_backed_server(&[
            (
                "tasks/old-parent.md",
                "---\nid: old-parent\ntitle: Old Parent\ntype: task\nstatus: ready\n---\n\n# Old Parent\n",
            ),
            (
                "tasks/new-parent.md",
                "---\nid: new-parent\ntitle: New Parent\ntype: task\nstatus: ready\n---\n\n# New Parent\n",
            ),
            (
                "tasks/only-child.md",
                "---\nid: only-child\ntitle: Only Child\ntype: task\nstatus: ready\nparent: old-parent\n---\n\n# Only Child\n",
            ),
        ]);

        // Step 1: reparent the only child to new-parent.
        server
            .handle_update_task(&json!({
                "id": "only-child",
                "parent": "new-parent",
                "allow_missing_parent": false
            }))
            .expect("reparent should succeed");

        // Step 2: disk must reflect the new parent.
        let child_content =
            std::fs::read_to_string(tmp.path().join("tasks/only-child.md")).unwrap();
        assert!(
            child_content.contains("parent: new-parent"),
            "child's frontmatter must have new-parent on disk; got:\n{child_content}"
        );
        assert!(
            !child_content.contains("parent: old-parent"),
            "child must no longer reference old-parent on disk; got:\n{child_content}"
        );
        // modified must be bumped on every update.
        assert!(
            child_content.contains("modified:"),
            "modified timestamp must be present; got:\n{child_content}"
        );

        // Step 3: old-parent now has zero open children — complete_task must succeed.
        server
            .handle_complete_task(&json!({
                "id": "old-parent",
                "completion_evidence": "all children reparented"
            }))
            .expect(
                "complete_task(old-parent) must succeed after its only child was reparented out",
            );
    }

    #[test]
    fn test_template_child_does_not_block_parent_completion() {
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/parent-task.md",
                "---\nid: parent-task\ntitle: Parent Task\ntype: task\nstatus: ready\n---\n\n# Parent Task\n",
            ),
            (
                "tasks/child-template.md",
                "---\nid: child-template\ntitle: Child Template\ntype: template\nstatus: ready\nparent: parent-task\n---\n\n# Child Template\n",
            ),
        ]);

        // Completing parent-task must succeed despite child-template being open,
        // because templates are perpetual definitions excluded from open_descendants.
        server
            .handle_complete_task(&json!({
                "id": "parent-task",
                "completion_evidence": "work is finished; template child is a perpetual definition"
            }))
            .expect("complete_task(parent-task) must succeed with a template child");
    }

    // ── Mutation neighborhood (specs/mutation-neighborhood.md) ────────────────

    #[test]
    fn test_mutation_neighborhood_shape() {
        // Graph:
        //   epic-x ── children: task-c (done, the closed task),
        //             s1/s2/s3/s4 (open), s-arch (archived), s-done (done)
        //   task-c blocks task-d (active, only dep is task-c → now unblocked)
        //          and task-e (active, also depends on s1 which is open → still blocked)
        let docs = vec![
            make_doc("e.md", "Epic X", "epic", "active", "epic-x", None, &[]),
            make_doc(
                "c.md",
                "Closed task",
                "task",
                "done",
                "task-c",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "s1.md",
                "Sibling 1",
                "task",
                "active",
                "task-s1",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "s2.md",
                "Sibling 2",
                "task",
                "active",
                "task-s2",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "s3.md",
                "Sibling 3",
                "task",
                "active",
                "task-s3",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "s4.md",
                "Sibling 4",
                "task",
                "active",
                "task-s4",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "sa.md",
                "Archived sib",
                "task",
                "archived",
                "task-sa",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "sd.md",
                "Done sib",
                "task",
                "done",
                "task-sd",
                Some("epic-x"),
                &[],
            ),
            make_doc(
                "d.md",
                "Dependent D",
                "task",
                "active",
                "task-d",
                None,
                &["task-c"],
            ),
            make_doc(
                "ee.md",
                "Dependent E",
                "task",
                "active",
                "task-e",
                None,
                &["task-c", "task-s1"],
            ),
            // A bare leaf with no parent and no dependents.
            make_doc(
                "lone.md",
                "Lone leaf",
                "task",
                "done",
                "task-lone",
                None,
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb-nbhd"));

        let n = PkbSearchServer::build_mutation_neighborhood(&graph, "task-c", 0);

        // parent present with sibling counts; archived + done excluded from open.
        let parent = &n["parent"];
        assert_eq!(parent["id"], "epic-x");
        assert_eq!(
            parent["siblings_open"], 4,
            "s1..s4 are the only open siblings (archived/done excluded): {n}"
        );
        // siblings_sample capped at 3.
        assert_eq!(
            parent["siblings_sample"].as_array().unwrap().len(),
            3,
            "siblings_sample must cap at 3: {n}"
        );

        // unblocked: task-d cleared (only dep was task-c), task-e still blocked by task-s1.
        let unblocked: Vec<&str> = n["unblocked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert!(
            unblocked.contains(&"task-d"),
            "task-d must be unblocked: {n}"
        );
        assert!(
            !unblocked.contains(&"task-e"),
            "task-e is still blocked by open task-s1 and must NOT appear: {n}"
        );

        // task-c has no open children and no cascade → children omitted.
        assert!(
            n.get("children").is_none(),
            "children must be omitted for a clean close: {n}"
        );

        // Omit-when-empty: a lone leaf returns null.
        let empty = PkbSearchServer::build_mutation_neighborhood(&graph, "task-lone", 0);
        assert!(
            empty.is_null(),
            "lone leaf must yield null neighborhood: {empty}"
        );
    }

    #[test]
    fn test_mutation_neighborhood_cascade_count() {
        // A recursive close reports closed_by_cascade and omits parent when none.
        let docs = vec![make_doc(
            "p.md",
            "Parent task",
            "task",
            "done",
            "task-p",
            None,
            &[],
        )];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb-nbhd2"));
        let n = PkbSearchServer::build_mutation_neighborhood(&graph, "task-p", 3);
        assert_eq!(
            n["children"]["closed_by_cascade"], 3,
            "cascade count surfaced: {n}"
        );
        assert!(n.get("parent").is_none(), "no parent → parent omitted: {n}");
    }

    #[test]
    fn test_complete_task_returns_neighborhood_end_to_end() {
        // Full write path: epic-z parents z1 + z2; z2 depends on z1.
        // Completing z1 should return a JSON envelope whose neighborhood names
        // the parent and reports z2 as newly unblocked.
        let (tmp, server) = build_disk_backed_server(&[
            ("e.md", "---\nid: epic-z\ntitle: Epic Z\ntype: epic\nstatus: active\n---\n\n# Epic Z\n"),
            ("z1.md", "---\nid: task-z1\ntitle: Task Z1\ntype: task\nstatus: ready\nparent: epic-z\n---\n\n# Z1\n"),
            ("z2.md", "---\nid: task-z2\ntitle: Task Z2\ntype: task\nstatus: ready\nparent: epic-z\ndepends_on: [task-z1]\n---\n\n# Z2\n"),
        ]);

        let res = server
            .handle_complete_task(&json!({
                "id": "task-z1",
                "completion_evidence": "Implemented Z1 in the test fixture.",
            }))
            .expect("complete_task should succeed for a leaf task");
        let text = res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        let v: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("response must be JSON: {e}\n{text}"));

        assert_eq!(v["ok"], true, "envelope ok flag: {v}");
        assert_eq!(v["status"], "done", "status: {v}");
        assert_eq!(
            v["neighborhood"]["parent"]["id"], "epic-z",
            "parent surfaced: {v}"
        );
        let unblocked: Vec<&str> = v["neighborhood"]["unblocked"]
            .as_array()
            .map(|a| a.iter().filter_map(|o| o["id"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            unblocked.contains(&"task-z2"),
            "z2 should be unblocked once z1 closes: {v}"
        );
        drop(tmp);
    }

    #[test]
    fn test_complete_task_rejects_with_open_children() {
        // proj-alpha has children: task-a1 (active), task-a2 (active), task-a3 (done).
        // Completing proj-alpha without recursive=true should fail.
        let server = build_test_server();
        let err = server
            .handle_complete_task(&json!({
                "id": "proj-alpha",
                "completion_evidence": "done"
            }))
            .expect_err("should reject complete when open children exist");
        let msg = err.message.to_string();
        assert!(
            msg.to_lowercase().contains("open child") || msg.to_lowercase().contains("children"),
            "error should mention open children; got: {msg}"
        );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
    }

    // ── mem_a1668542: release_task failure-reason-mandatory gate
    //  specs/enforcement/evidence-contract.md. Releasing to
    // a handback status (blocked/cancelled/review/partial) requires a
    // non-empty `reason` (or `blocker`, for `blocked`).

    #[test]
    fn test_release_task_blocked_without_reason_or_blocker_is_rejected() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let err = server
            .handle_release_task(&json!({
                "id": "task-new",
                "status": "blocked",
                "summary": "Blocked with no reason given.",
            }))
            .expect_err("blocked with no reason/blocker must be rejected");
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("reason") && msg.contains("blocker"),
            "error should mention reason/blocker requirement; got: {msg}"
        );
    }

    #[test]
    fn test_release_task_blocked_with_reason_is_accepted() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let res = server
            .handle_release_task(&json!({
                "id": "task-new",
                "status": "blocked",
                "summary": "Blocked, with a stated reason.",
                "reason": "waiting on upstream API access",
            }))
            .expect("blocked with a non-empty reason must be accepted");
        assert!(
            !res.is_error.unwrap_or(false),
            "should not be an error result: {res:?}"
        );
    }

    #[test]
    fn test_release_task_blocked_with_only_blocker_is_accepted() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let res = server
            .handle_release_task(&json!({
                "id": "task-new",
                "status": "blocked",
                "summary": "Blocked, with only a blocker given.",
                "blocker": "waiting on infra team to provision the box",
            }))
            .expect("blocked with a non-empty blocker (no reason) must be accepted");
        assert!(
            !res.is_error.unwrap_or(false),
            "should not be an error result: {res:?}"
        );
    }

    #[test]
    fn test_release_task_cancelled_review_partial_without_reason_are_rejected() {
        for status in ["cancelled", "review", "partial"] {
            let (_tmp, server) = build_disk_backed_server(&[(
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            )]);
            let err = server
                .handle_release_task(&json!({
                    "id": "task-new",
                    "status": status,
                    "summary": format!("Releasing as {status} with no reason given."),
                }))
                .expect_err(&format!("{status} with no reason must be rejected"));
            assert!(
                matches!(err.code, ErrorCode::INVALID_PARAMS),
                "status={status}: should be INVALID_PARAMS, got: {:?}",
                err.code
            );
            assert!(
                err.message.to_lowercase().contains("reason"),
                "status={status}: error should mention reason requirement; got: {}",
                err.message
            );

            // ... and accepted once a reason is given.
            let (_tmp2, server2) = build_disk_backed_server(&[(
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            )]);
            let res = server2
                .handle_release_task(&json!({
                    "id": "task-new",
                    "status": status,
                    "summary": format!("Releasing as {status} with a reason."),
                    "reason": "declared explicitly for this test",
                }))
                .unwrap_or_else(|e| panic!("{status} with a reason must be accepted: {e:?}"));
            assert!(
                !res.is_error.unwrap_or(false),
                "status={status}: should not error: {res:?}"
            );
        }
    }

    #[test]
    fn test_release_task_success_status_unaffected_by_reason_gate() {
        // merge_ready and done are not handback statuses — no reason/blocker
        // is required, only `summary` (unchanged behavior).
        for status in ["merge_ready", "done"] {
            let (_tmp, server) = build_disk_backed_server(&[(
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            )]);
            let res = server
                .handle_release_task(&json!({
                    "id": "task-new",
                    "status": status,
                    "summary": "Shipped cleanly.",
                }))
                .unwrap_or_else(|e| {
                    panic!("status={status} success path must be unaffected: {e:?}")
                });
            assert!(
                !res.is_error.unwrap_or(false),
                "status={status}: should not error: {res:?}"
            );
        }
    }

    #[test]
    fn test_release_task_missing_created() {
        // No `created` frontmatter at all — fail-closed.
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-nocreated.md",
            "---\nid: task-nocreated\ntitle: No created field\ntype: task\nstatus: ready\n---\n\n# No created field\n",
        )]);
        let err = server
            .handle_release_task(&json!({
                "id": "task-nocreated",
                "status": "blocked",
                "summary": "No created field, no reason given.",
            }))
            .expect_err("missing `created` must fail closed");
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
    }

    #[test]
    fn test_update_task_rejects_closing_with_open_children() {
        // proj-alpha has active children; setting its status to done should be rejected.
        let server = build_test_server();
        let err = server
            .handle_update_task(&json!({
                "id": "proj-alpha",
                "status": "done",
                "completion_evidence": "done"
            }))
            .expect_err("should reject status=done when open children exist");
        let msg = err.message.to_string();
        assert!(
            msg.to_lowercase().contains("open child") || msg.to_lowercase().contains("children"),
            "error should mention open children; got: {msg}"
        );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "should be INVALID_PARAMS, got: {:?}",
            err.code
        );
    }

    #[test]
    fn test_update_task_allows_cancelled_without_children_check() {
        // task-g1 has no children — closing it should not be blocked.
        // (Will fail on disk I/O since test server has no real files, but
        // must NOT fail on the open-children guard.)
        let server = build_test_server();
        let result = server.handle_update_task(&json!({
            "id": "task-g1",
            "status": "cancelled"
        }));
        match result {
            Ok(_) => { /* success is fine */ }
            Err(e) => {
                let msg = e.message.to_string();
                assert!(
                    !msg.to_lowercase().contains("open child"),
                    "task with no children should not trigger open-children guard; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn test_update_task_allows_closed_status_with_recursive() {
        // proj-alpha has open children but recursive=true should bypass the block.
        // Will still fail on disk I/O, but must NOT fail on the open-children guard.
        let server = build_test_server();
        let result = server.handle_update_task(&json!({
            "id": "proj-alpha",
            "status": "cancelled",
            "recursive": true
        }));
        match result {
            Ok(_) => { /* success is fine */ }
            Err(e) => {
                let msg = e.message.to_string();
                assert!(
                    !msg.to_lowercase().contains("open child"),
                    "recursive=true should bypass the open-children guard; got: {msg}"
                );
            }
        }
    }

    /// refresh_graph must reflect new files written to disk after server startup.
    #[test]
    fn test_refresh_graph_reflects_disk_changes() {
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/initial.md",
            "---\nid: initial-task\ntitle: Initial Task\ntype: task\nstatus: ready\n---\n\n# Initial\n",
        )]);

        // new-task is not yet on disk — must be absent from graph.
        assert!(
            server.graph.read().get_node("new-task").is_none(),
            "new-task must not be in graph before disk write"
        );

        // Write a new file to disk without going through the server mutation path.
        std::fs::write(
            tmp.path().join("tasks/new-task.md"),
            "---\nid: new-task\ntitle: New Task\ntype: task\nstatus: ready\n---\n\n# New\n",
        )
        .unwrap();

        // Graph is still stale — in-memory index hasn't been told about the new file.
        assert!(
            server.graph.read().get_node("new-task").is_none(),
            "new-task must not be in graph before refresh_graph"
        );

        // Call refresh_graph — should scan disk and load new-task.
        let result = server
            .handle_refresh_graph(&json!({}))
            .expect("refresh_graph must succeed");
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();

        // After refresh, new-task must be in the graph.
        assert!(
            server.graph.read().get_node("new-task").is_some(),
            "new-task must be in graph after refresh_graph; response: {text}"
        );

        // Response JSON: ok=true, node_count positive.
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("response must be valid JSON");
        assert_eq!(parsed["ok"], true, "response ok must be true: {text}");
        assert_eq!(
            parsed["node_count"].as_u64().unwrap_or(0),
            2,
            "node_count must be exactly 2 (initial-task + new-task): {text}"
        );
    }

    #[test]
    fn test_mcp_add_and_delete_observations() {
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/test-task.md",
            "---\nid: test-task\nmodified: '2026-08-01T00:00:00Z'\ntitle: Test Task\ntype: task\nstatus: in_progress\n---\n\n# Header\n\n## Observations\n- obs 1\n",
        )]);

        // 1. Add observations via MCP dispatch
        let add_res = server
            .dispatch_tool_sync(
                "add_observations",
                &json!({
                    "id": "test-task",
                    "lines": ["obs 2", "- obs 3"],
                    "section": "Observations",
                    "expected_modified": "2026-08-01T00:00:00Z"
                }),
            )
            .expect("add_observations must succeed");

        let add_text: String = add_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let add_json: serde_json::Value = serde_json::from_str(&add_text).unwrap();
        assert_eq!(add_json["ok"], true);
        assert_eq!(add_json["added_count"], 2);
        let new_mod = add_json["modified"].as_str().unwrap();

        let disk_content = std::fs::read_to_string(tmp.path().join("tasks/test-task.md")).unwrap();
        assert!(disk_content.contains("- obs 1\n- obs 2\n- obs 3"));

        // 2. Delete observation via MCP dispatch
        let del_res = server
            .dispatch_tool_sync(
                "delete_observations",
                &json!({
                    "id": "test-task",
                    "selectors": ["obs 2"],
                    "expected_modified": new_mod
                }),
            )
            .expect("delete_observations must succeed");

        let del_text: String = del_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let del_json: serde_json::Value = serde_json::from_str(&del_text).unwrap();
        assert_eq!(del_json["ok"], true);
        assert_eq!(del_json["deleted_count"], 1);

        let final_disk = std::fs::read_to_string(tmp.path().join("tasks/test-task.md")).unwrap();
        assert!(final_disk.contains("- obs 1\n- obs 3"));
        assert!(!final_disk.contains("obs 2"));
    }

    #[test]
    fn test_mcp_delete_observations_not_found() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/test-task.md",
            "---\nid: test-task\ntitle: Test Task\ntype: task\nstatus: in_progress\n---\n\n# Header\n\n## Observations\n- obs 1\n",
        )]);

        let err = server
            .dispatch_tool_sync(
                "delete_observations",
                &json!({
                    "id": "test-task",
                    "selectors": ["nonexistent"]
                }),
            )
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("nonexistent"));
        let data = err.data.unwrap();
        assert_eq!(data["error_type"], "selector_not_found");
        assert_eq!(data["selector"], "nonexistent");
    }

    #[test]
    fn test_mcp_observations_stale_write() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/test-task.md",
            "---\nid: test-task\nmodified: '2026-08-01T00:00:00Z'\ntitle: Test Task\ntype: task\nstatus: in_progress\n---\n\n## Observations\n",
        )]);

        let err = server
            .dispatch_tool_sync(
                "add_observations",
                &json!({
                    "id": "test-task",
                    "lines": ["obs 1"],
                    "expected_modified": "2026-07-01T00:00:00Z"
                }),
            )
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("Stale write rejected"));
        let data = err.data.unwrap();
        assert_eq!(data["error_type"], "stale_write");
    }

    #[test]
    fn test_edit_body_mcp_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();

        let task_file = root.join("tasks").join("task-target.md");
        std::fs::write(
            &task_file,
            "---\nid: task-target\nmodified: '2026-01-01T00:00:00Z'\ntitle: Target Task\ntype: task\nstatus: inbox\n---\n\nFirst line.\nSecond line.\nThird line.\n",
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

        // 1. Dry run
        let dry_diff = "```diff\n@@ ... @@\n-Second line.\n+Changed second line.\n```";
        let dry_res = server.dispatch_tool_sync("edit_body", &serde_json::json!({
            "id": "task-target",
            "diff": dry_diff,
            "dry_run": true,
            "expected_modified": "2026-01-01T00:00:00Z"
        })).expect("dry run should succeed");
        let dry_text: String = dry_res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect();
        assert!(dry_text.contains("\"dry_run\":true"));
        let disk_text = std::fs::read_to_string(&task_file).unwrap();
        assert!(disk_text.contains("Second line."));

        // 2. Real application via "edit_body"
        let real_diff = "```diff\n@@ ... @@\n-Second line.\n+Replaced second line.\n```";
        let res = server.dispatch_tool_sync("edit_body", &serde_json::json!({
            "id": "task-target",
            "diff": real_diff,
            "expected_modified": "2026-01-01T00:00:00Z"
        })).expect("real edit should succeed");
        let res_text: String = res.content.iter().filter_map(|c| c.raw.as_text().map(|t| t.text.as_str())).collect();
        assert!(res_text.contains("\"ok\":true"));
        let updated_disk = std::fs::read_to_string(&task_file).unwrap();
        assert!(updated_disk.contains("Replaced second line."));
        assert!(!updated_disk.contains("Second line."));
        assert!(updated_disk.contains("status: inbox"));

        // 3. Stale write rejection
        let stale_diff = "```diff\n@@ ... @@\n-Third line.\n+Stale third line.\n```";
        let stale_res = server.dispatch_tool_sync("edit_body", &serde_json::json!({
            "id": "task-target",
            "diff": stale_diff,
            "expected_modified": "2026-01-01T00:00:00Z"
        }));
        let stale_err = stale_res.unwrap_err();
        assert!(stale_err.message.contains("Stale write rejected"));

        // 4. Non-matching diff failure
        let nomatch_diff = "```diff\n@@ ... @@\n-Nonexistent line.\n+Failure line.\n```";
        let nomatch_res = server.dispatch_tool_sync("edit_body", &serde_json::json!({
            "id": "task-target",
            "diff": nomatch_diff
        }));
        let nomatch_err = nomatch_res.unwrap_err();
        assert!(nomatch_err.message.contains("diff_application_failed"));
    }

    // ── mem_2ecf862b: `status: "blocked"` is a stored-vs-computed collision
    // unless it carries new information (a blocker/reason). A bare hand-write
    // (no blocker, no reason) duplicates what `depends_on` + the computed
    // `blocked` boolean already say and goes stale silently — reject it at
    // the shared write path (document_crud::update_document), which both
    // `update_task` and `release_task` funnel through.

    #[test]
    fn test_update_task_bare_status_blocked_is_rejected() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let err = server
            .handle_update_task(&json!({"id": "task-new", "status": "blocked"}))
            .expect_err("bare status=blocked with no blocker/reason must be rejected");
        assert!(
            matches!(err.code, ErrorCode::INTERNAL_ERROR),
            "got: {:?}",
            err.code
        );
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("blocker") && msg.contains("reason"),
            "error should mention blocker/reason requirement; got: {msg}"
        );
    }

    #[test]
    fn test_update_task_status_blocked_with_blocker_is_accepted() {
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        server
            .handle_update_task(&json!({
                "id": "task-new",
                "status": "blocked",
                "blocker": "waiting on infra provisioning",
            }))
            .expect("status=blocked with a non-empty blocker must be accepted");
        let disk = std::fs::read_to_string(tmp.path().join("tasks/task-new.md")).unwrap();
        assert!(disk.contains("status: blocked"));
        assert!(disk.contains("waiting on infra provisioning"));
    }

    #[test]
    fn test_update_task_status_blocked_reuses_already_stored_blocker() {
        // A metadata-only patch (e.g. re-affirming status=blocked after some
        // other field changed) must not be rejected just because THIS call
        // didn't repeat the blocker — the one already on disk still applies.
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: blocked\nblocker: waiting on upstream\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        server
            .handle_update_task(&json!({"id": "task-new", "tags": ["backend"]}))
            .expect("unrelated metadata patch on an already-blocked node must not be rejected");
        let disk = std::fs::read_to_string(tmp.path().join("tasks/task-new.md")).unwrap();
        assert!(disk.contains("status: blocked"));
    }

    #[test]
    fn test_update_document_rejects_manual_blocked_boolean() {
        // Pre-existing guard (mem-74b6165e) — reconfirmed still active after
        // the mem_2ecf862b status=blocked gate was added alongside it.
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let err = server
            .handle_update_task(&json!({"id": "task-new", "updates": {"blocked": true}}))
            .expect_err("manual 'blocked' boolean must be rejected");
        assert!(err.message.to_lowercase().contains("reserved computed keyword"));
    }

    // ── mem_8a8aeb2a: release_task must persist the exact status it reports,
    // and must persist `reason`/`blocker` to frontmatter (not just prose).

    #[test]
    fn test_release_task_reported_status_matches_persisted_status() {
        for status in ["cancelled", "blocked", "review", "partial", "done", "merge_ready"] {
            let (tmp, server) = build_disk_backed_server(&[(
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            )]);
            let mut args = json!({
                "id": "task-new",
                "status": status,
                "summary": format!("Releasing as {status}."),
            });
            if status == "cancelled" || status == "review" || status == "partial" {
                args["reason"] = json!("declared explicitly for this test");
            } else if status == "blocked" {
                args["blocker"] = json!("declared explicitly for this test");
            }
            if status == "done" {
                args["completion_evidence"] = json!("evidence for this test");
            }
            let res = server
                .handle_release_task(&args)
                .unwrap_or_else(|e| panic!("release to {status} should succeed: {e:?}"));
            let res_text: String = res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let reported: serde_json::Value = serde_json::from_str(&res_text)
                .unwrap_or_else(|_| panic!("release_task should return JSON, got: {res_text}"));
            let reported_status = reported.get("status").and_then(|v| v.as_str()).unwrap();
            assert_eq!(
                reported_status, status,
                "reported status must equal the requested status"
            );

            let disk = std::fs::read_to_string(tmp.path().join("tasks/task-new.md")).unwrap();
            assert!(
                disk.contains(&format!("status: {status}")),
                "status={status}: persisted frontmatter must match reported status, got:\n{disk}"
            );

            let get_res = server
                .handle_get_task(&json!({"id": "task-new"}))
                .expect("get_task should succeed");
            let get_text: String = get_res
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect();
            let get_json: serde_json::Value = serde_json::from_str(&get_text).unwrap();
            assert_eq!(
                get_json
                    .get("frontmatter")
                    .and_then(|f| f.get("status"))
                    .and_then(|v| v.as_str()),
                Some(status),
                "status={status}: get_task must read back the same status release_task reported"
            );
        }
    }

    #[test]
    fn test_release_task_persists_reason_and_blocker_to_frontmatter() {
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        server
            .handle_release_task(&json!({
                "id": "task-new",
                "status": "cancelled",
                "summary": "Cancelling.",
                "reason": "Duplicate of task-other, filed concurrently.",
            }))
            .expect("release to cancelled with a reason must succeed");
        let disk = std::fs::read_to_string(tmp.path().join("tasks/task-new.md")).unwrap();
        assert!(
            disk.contains("reason: Duplicate of task-other, filed concurrently."),
            "reason must be persisted to frontmatter, not just prose evidence, got:\n{disk}"
        );

        let (tmp2, server2) = build_disk_backed_server(&[(
            "tasks/task-blk.md",
            "---\nid: task-blk\ntitle: Blk task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Blk task\n",
        )]);
        server2
            .handle_release_task(&json!({
                "id": "task-blk",
                "status": "blocked",
                "summary": "Blocked.",
                "blocker": "waiting on external API access",
            }))
            .expect("release to blocked with a blocker must succeed");
        let disk2 = std::fs::read_to_string(tmp2.path().join("tasks/task-blk.md")).unwrap();
        assert!(
            disk2.contains("blocker: waiting on external API access"),
            "blocker must be persisted to frontmatter, not just prose evidence, got:\n{disk2}"
        );
    }

    // ── mem_e6245fc2: list_tasks(status=<S>) must filter strictly on the
    // node's stored frontmatter status, never a computed set.

    #[test]
    fn test_list_tasks_status_blocked_excludes_stored_ready_with_computed_block() {
        // task-d63bfe83-shaped fixture: stored status "ready", but has an
        // unmet dependency so the computed `blocked` boolean is true.
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-dep.md",
                "---\nid: task-dep\ntitle: Dependency\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Dependency\n",
            ),
            (
                "tasks/task-shadow.md",
                "---\nid: task-shadow\ntitle: Shadow\ntype: task\nstatus: ready\ndepends_on: [task-dep]\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Shadow\n",
            ),
        ]);
        // Sanity: the computed `blocked` flag IS true for task-shadow.
        let ready_list = server
            .handle_list_tasks(&json!({"status": "ready", "format": "json", "include_subtasks": true}))
            .unwrap();
        let ready_objs = extract_task_objects(&ready_list);
        let shadow = ready_objs
            .iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-shadow"))
            .expect("stored-ready task-shadow must appear under status=ready");
        assert_eq!(shadow.get("blocked").and_then(|v| v.as_bool()), Some(true));

        // The bug: status="blocked" must NOT match task-shadow just because
        // its computed `blocked` flag is true — its stored status is "ready".
        let blocked_list = server
            .handle_list_tasks(&json!({"status": "blocked", "format": "json"}))
            .unwrap();
        let blocked_ids: Vec<String> = extract_task_objects(&blocked_list)
            .iter()
            .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        assert!(
            !blocked_ids.contains(&"task-shadow".to_string()),
            "status=\"blocked\" must not admit a stored-ready node just because its \
             computed blocked flag is true; got: {blocked_ids:?}"
        );
    }

    #[test]
    fn test_list_tasks_status_ready_excludes_stored_inbox_and_queued() {
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-ready.md",
                "---\nid: task-ready\ntitle: Ready\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Ready\n",
            ),
            (
                "tasks/task-inbox.md",
                "---\nid: task-inbox\ntitle: Inbox\ntype: task\nstatus: inbox\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Inbox\n",
            ),
            (
                "tasks/task-queued.md",
                "---\nid: task-queued\ntitle: Queued\ntype: task\nstatus: queued\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Queued\n",
            ),
        ]);
        let result = server
            .handle_list_tasks(&json!({"status": "ready", "format": "json"}))
            .unwrap();
        let ids: Vec<String> = extract_task_objects(&result)
            .iter()
            .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        assert!(ids.contains(&"task-ready".to_string()), "got: {ids:?}");
        assert!(
            !ids.contains(&"task-inbox".to_string()),
            "status=\"ready\" must not admit stored inbox; got: {ids:?}"
        );
        assert!(
            !ids.contains(&"task-queued".to_string()),
            "status=\"ready\" must not admit stored queued; got: {ids:?}"
        );
    }

    #[test]
    fn test_blocker_resolution_case_insensitive_mcp() {
        // Blocker task has lowercase id: "academicops-d067e425", status: "done"
        // Downstream tasks reference it with mixed case: "academicOps-d067e425"
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/academicops-d067e425.md",
                "---\nid: academicops-d067e425\ntitle: Upstream Blocker\ntype: task\nstatus: done\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Upstream Blocker\n",
            ),
            (
                "tasks/academicops-4a31fae0.md",
                "---\nid: academicops-4a31fae0\ntitle: Downstream Task A\ntype: task\nstatus: ready\ndepends_on: [academicOps-d067e425]\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Downstream Task A\n",
            ),
            (
                "tasks/open-blocker.md",
                "---\nid: open-blocker-1234\ntitle: Open Blocker\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Open Blocker\n",
            ),
            (
                "tasks/blocked-task.md",
                "---\nid: blocked-task-5678\ntitle: Blocked Task B\ntype: task\nstatus: blocked\ndepends_on: [OPEN-BLOCKER-1234]\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Blocked Task B\n",
            ),
        ]);

        // 1. Task A depending on done blocker with mixed-case ID must NOT be blocked
        let ready_list = server
            .handle_list_tasks(&json!({"status": "ready", "format": "json"}))
            .unwrap();
        let ready_objs = extract_task_objects(&ready_list);
        let task_a = ready_objs
            .iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("academicops-4a31fae0"))
            .expect("academicops-4a31fae0 must be present in ready tasks");
        assert_eq!(
            task_a.get("blocked").and_then(|v| v.as_bool()),
            Some(false),
            "task depending on done blocker must have blocked=false"
        );

        // 2. get_task on task A resolves the dependency title and status despite mixed-case reference
        let get_a = server
            .handle_get_task(&json!({"id": "academicops-4a31fae0"}))
            .unwrap();
        let get_a_text = get_a.content[0].as_text().unwrap().text.as_str();
        assert!(
            get_a_text.contains("Upstream Blocker"),
            "get_task must resolve dependency title; got: {get_a_text}"
        );
        assert!(
            get_a_text.contains("done"),
            "get_task must resolve dependency status as done; got: {get_a_text}"
        );

        // 3. Task B depending on open blocker (mixed-case) is blocked, and list_tasks renders blocker status correctly
        let list_blocked_text = server
            .handle_list_tasks(&json!({"status": "blocked", "format": "markdown"}))
            .unwrap();
        let md_out = list_blocked_text.content[0].as_text().unwrap().text.as_str();
        assert!(
            md_out.contains("blocked-task-5678"),
            "blocked-task-5678 should appear in list_tasks blocked markdown view"
        );
        assert!(
            !md_out.contains("[?] ?"),
            "blocked view must not render '[?] ?' for resolvable blocker; got: {md_out}"
        );
        assert!(
            md_out.contains("Open Blocker"),
            "blocked view should show blocker title; got: {md_out}"
        );
    }

    #[test]
    fn test_list_tasks_status_archived_is_rejected_not_aliased_to_done() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-done.md",
            "---\nid: task-done\ntitle: Done\ntype: task\nstatus: done\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Done\n",
        )]);
        let err = server
            .handle_list_tasks(&json!({"status": "archived", "format": "json"}))
            .expect_err(
                "status=\"archived\" is not a supported stored value and must be rejected, \
                 not silently aliased to the done set",
            );
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "got: {:?}",
            err.code
        );
    }

    #[test]
    fn test_list_tasks_per_status_totals_disjoint_and_sum_correctly() {
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-r.md",
                "---\nid: task-r\ntitle: R\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# R\n",
            ),
            (
                "tasks/task-q.md",
                "---\nid: task-q\ntitle: Q\ntype: task\nstatus: queued\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Q\n",
            ),
            (
                "tasks/task-ip.md",
                "---\nid: task-ip\ntitle: IP\ntype: task\nstatus: in_progress\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# IP\n",
            ),
            (
                "tasks/task-b.md",
                "---\nid: task-b\ntitle: B\ntype: task\nstatus: blocked\nblocker: x\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# B\n",
            ),
        ]);
        let mut total = 0usize;
        for status in ["ready", "queued", "in_progress", "blocked", "review"] {
            let result = server
                .handle_list_tasks(&json!({"status": status, "format": "json"}))
                .unwrap();
            total += extract_task_objects(&result).len();
        }
        assert_eq!(
            total, 4,
            "disjoint per-status totals across the 5-leg spine must sum to exactly the \
             4 fixture tasks stored under those statuses (no overlap, no leak)"
        );
    }

    // ── mem_8035b002: supersedes/superseded_by referential integrity ──

    #[test]
    fn test_update_task_rejects_dangling_supersedes_target() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let err = server
            .handle_update_task(&json!({"id": "task-new", "updates": {"supersedes": "task-does-not-exist"}}))
            .expect_err("a supersedes target that does not resolve must be rejected");
        assert!(
            matches!(err.code, ErrorCode::INVALID_PARAMS),
            "got: {:?}",
            err.code
        );
        assert!(
            err.message.contains("task-does-not-exist"),
            "error should name the unresolved target; got: {}",
            err.message
        );
    }

    #[test]
    fn test_update_task_accepts_supersedes_with_existing_target() {
        let (tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            ),
            (
                "tasks/task-old.md",
                "---\nid: task-old\ntitle: Old task\ntype: task\nstatus: done\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Old task\n",
            ),
        ]);
        server
            .handle_update_task(&json!({"id": "task-new", "updates": {"supersedes": "task-old"}}))
            .expect("a supersedes target that resolves must be accepted");
        let disk = std::fs::read_to_string(tmp.path().join("tasks/task-new.md")).unwrap();
        assert!(disk.contains("supersedes"));
        assert!(disk.contains("task-old"));
    }

    #[test]
    fn test_update_task_rejects_manual_superseded_by_write() {
        // superseded_by is a computed reverse index of supersedes — never a
        // second hand-written field (mem_8035b002, mirrors mem-74b6165e's
        // pre-existing 'blocked' guard).
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-new.md",
            "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
        )]);
        let err = server
            .handle_update_task(&json!({"id": "task-new", "updates": {"superseded_by": "task-other"}}))
            .expect_err("manual 'superseded_by' write must be rejected");
        assert!(
            err.message.to_lowercase().contains("reserved computed keyword"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn test_supersedes_comma_joined_string_form_parses_as_multiple_targets() {
        // The cited defect: `supersedes: "id1,id2,id3"` inside a single quoted
        // scalar must not be stored/read as one id with commas in it — split
        // via the same parse_string_array convention already used by
        // depends_on/soft_depends_on/etc.
        let (_tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-new.md",
                "---\nid: task-new\ntitle: New task\ntype: task\nstatus: ready\nsupersedes: \"task-old-1,task-old-2\"\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# New task\n",
            ),
            (
                "tasks/task-old-1.md",
                "---\nid: task-old-1\ntitle: Old 1\ntype: task\nstatus: done\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Old 1\n",
            ),
            (
                "tasks/task-old-2.md",
                "---\nid: task-old-2\ntitle: Old 2\ntype: task\nstatus: done\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Old 2\n",
            ),
        ]);
        // Both old tasks must show task-new in their computed superseded_by.
        for old_id in ["task-old-1", "task-old-2"] {
            let res = server
                .handle_list_tasks(&json!({
                    "has_superseded_by": true,
                    "include_done": true,
                    "format": "json"
                }))
                .unwrap();
            let objs = extract_task_objects(&res);
            let row = objs
                .iter()
                .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(old_id))
                .unwrap_or_else(|| panic!("{old_id} should be in has_superseded_by=true set, got {objs:?}"));
            let arr: Vec<&str> = row
                .get("superseded_by")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(arr, vec!["task-new"], "for {old_id}, got {arr:?}");
        }
    }

    #[test]
    fn test_merge_node_records_supersedes_on_canonical_not_superseded_by_on_source() {
        let (tmp, server) = build_disk_backed_server(&[
            (
                "tasks/task-canon.md",
                "---\nid: task-canon\ntitle: Canonical\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Canonical\n",
            ),
            (
                "tasks/task-dup.md",
                "---\nid: task-dup\ntitle: Duplicate\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\n# Duplicate\n",
            ),
        ]);
        server
            .handle_merge_node(&json!({
                "canonical_id": "task-canon",
                "source_ids": ["task-dup"],
                "dry_run": false,
            }))
            .expect("merge_node should succeed");

        let canon_disk = std::fs::read_to_string(tmp.path().join("tasks/task-canon.md")).unwrap();
        assert!(
            canon_disk.contains("supersedes"),
            "canonical node must record the merge via its own supersedes list, got:\n{canon_disk}"
        );
        let dup_disk = std::fs::read_to_string(tmp.path().join("tasks/task-dup.md")).unwrap();
        assert!(
            !dup_disk.contains("superseded_by"),
            "source node must NOT carry a hand-written superseded_by, got:\n{dup_disk}"
        );
        assert!(dup_disk.contains("status: done"));

        // The reverse index must resolve via the graph (computed, not stored).
        let res = server
            .handle_list_tasks(&json!({"has_superseded_by": true, "include_done": true, "format": "json"}))
            .unwrap();
        let objs = extract_task_objects(&res);
        let row = objs
            .iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some("task-dup"))
            .expect("task-dup should show up with computed superseded_by");
        let arr: Vec<&str> = row
            .get("superseded_by")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(arr, vec!["task-canon"]);
    }

    // ── mem_45c5d3c9: `update_body` is a read-modify-write with no CAS by
    // default; PR #492 added an *optional* `expected_modified` precondition.
    // Reproduce both the fixed path (CAS supplied → stale write rejected,
    // content preserved) and the still-reachable gap (CAS omitted → silent
    // last-write-wins), matching the git-forensics-verified 6208→4597→5147
    // clobber this defect was filed against.

    #[test]
    fn test_update_body_concurrent_writers_stale_write_rejected_with_cas() {
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/task-race.md",
            "---\nid: task-race\ntitle: Race\ntype: task\nstatus: ready\nmodified: '2026-08-18T22:50:03Z'\ncreated: 2026-07-23T10:00:00+00:00\n---\n\nOriginal body.\n",
        )]);
        let path = tmp.path().join("tasks/task-race.md");

        // Both "writers" read the same snapshot (modified=T0).
        let t0 = "2026-08-18T22:50:03Z";

        // Writer B (the appender in the real incident) writes first and wins.
        server
            .handle_update_body(&json!({
                "id": "task-race",
                "new_body": "Original body.\n\nWriter B's trailer.",
                "expected_modified": t0,
            }))
            .expect("writer B's write against a fresh snapshot must succeed");
        let after_b = std::fs::read_to_string(&path).unwrap();
        assert!(after_b.contains("Writer B's trailer."));

        // Writer A (the "condensing" edit in the incident) still holds the
        // stale T0 snapshot and must be rejected, not silently overwrite B.
        let err = server
            .handle_update_body(&json!({
                "id": "task-race",
                "new_body": "Original body, condensed by writer A.",
                "expected_modified": t0,
            }))
            .expect_err("a stale snapshot must be rejected, not silently applied");
        assert_eq!(
            err.data.as_ref().and_then(|d| d.get("error_type")).and_then(|v| v.as_str()),
            Some("stale_write"),
            "failure must be specifically distinguishable as a stale write, got: {err:?}"
        );

        // Writer B's content must survive on disk — the whole point of the guard.
        let final_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            final_disk.contains("Writer B's trailer."),
            "writer A's stale write must not have destroyed writer B's content, got:\n{final_disk}"
        );
    }

    #[test]
    fn test_update_body_without_expected_modified_still_last_write_wins() {
        // Documents the residual, deliberately-unresolved gap: `expected_modified`
        // is optional (PR #492), so a caller that omits it still silently
        // clobbers a concurrent writer's content, unconditionally.
        let (tmp, server) = build_disk_backed_server(&[(
            "tasks/task-race2.md",
            "---\nid: task-race2\ntitle: Race2\ntype: task\nstatus: ready\ncreated: 2026-07-23T10:00:00+00:00\n---\n\nOriginal body.\n",
        )]);
        let path = tmp.path().join("tasks/task-race2.md");

        server
            .handle_update_body(&json!({
                "id": "task-race2",
                "new_body": "Original body.\n\nWriter B's trailer.",
            }))
            .expect("writer B's write must succeed");

        // Writer A, with no expected_modified, clobbers B's write unconditionally.
        server
            .handle_update_body(&json!({
                "id": "task-race2",
                "new_body": "Original body, condensed by writer A.",
            }))
            .expect("writer A's write without a CAS token is still accepted (known gap)");

        let final_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !final_disk.contains("Writer B's trailer."),
            "documents the known gap: omitting expected_modified still silently \
             destroys a concurrent writer's content"
        );
    }

    // ── aops_81bbdd77: `since=`/`before=` compare the UTC calendar date of
    // `modified`, not a local one. Pin that explicitly: a task modified late
    // in the UTC day (which is still "yesterday" in a UTC+ timezone at local
    // morning) must be excluded by since=<the UTC date + 1>, matching the
    // documented UTC semantics rather than silently using local dates.

    #[test]
    fn test_list_tasks_since_before_compare_utc_calendar_date_of_modified() {
        let (_tmp, server) = build_disk_backed_server(&[(
            "tasks/task-late-utc.md",
            "---\nid: task-late-utc\ntitle: Late UTC\ntype: task\nstatus: ready\nmodified: '2026-08-17T23:00:00Z'\ncreated: 2026-08-17T23:00:00+00:00\n---\n\n# Late UTC\n",
        )]);

        // since=2026-08-17 (the UTC date) must include it.
        let res = server
            .handle_list_tasks(&json!({"since": "2026-08-17", "format": "json"}))
            .unwrap();
        let ids: Vec<String> = extract_task_objects(&res)
            .iter()
            .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        assert!(
            ids.contains(&"task-late-utc".to_string()),
            "since=2026-08-17 (the UTC date of modified) must include the task, got {ids:?}"
        );

        // since=2026-08-18 (one day past the UTC date) must exclude it, even
        // though 2026-08-17T23:00:00Z is already 2026-08-18 local in any
        // timezone ahead of UTC (e.g. Brisbane, UTC+10) — the comparison is
        // against the UTC date, documented explicitly in the tool schema.
        let res2 = server
            .handle_list_tasks(&json!({"since": "2026-08-18", "format": "json"}))
            .unwrap();
        let ids2: Vec<String> = extract_task_objects(&res2)
            .iter()
            .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        assert!(
            !ids2.contains(&"task-late-utc".to_string()),
            "since=2026-08-18 must exclude a task whose UTC modified date is still 2026-08-17, got {ids2:?}"
        );
    }

    #[test]
    fn test_mcp_endpoints_never_disclose_server_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::create_dir_all(root.join("memories")).unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        write_test_polecat_yaml(root);

        let graph = GraphStore::build(&[], root);
        let store = VectorStore::new(3);
        let embedder = Embedder::new_dummy();
        let db_path = root.join("db.bin");
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            root.to_path_buf(),
            db_path,
            Arc::new(RwLock::new(graph)),
        );

        let root_str = root.to_str().unwrap();

        // 1. handle_create_document
        let create_doc_res = server
            .handle_create_document(&json!({
                "title": "Doc Without Path Leak",
                "type": "note",
                "body": "Body content"
            }))
            .unwrap();
        let doc_text: String = create_doc_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(doc_text.starts_with("Document created: "));
        assert!(!doc_text.contains(root_str), "Must not leak root path");
        assert!(!doc_text.contains(".md"), "Must not leak file extension in path");

        // 2. handle_create_memory
        let create_mem_res = server
            .handle_create_memory(&json!({
                "title": "Memory Without Path Leak",
                "body": "Memory body"
            }))
            .unwrap();
        let mem_text: String = create_mem_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(mem_text.starts_with("Memory created: "));
        assert!(!mem_text.contains(root_str), "Must not leak root path");
        assert!(!mem_text.contains(".md"), "Must not leak file extension in path");

        // 3. handle_delete_document
        // First create a doc to delete
        let _del_target_res = server
            .handle_create_document(&json!({
                "title": "Doc To Delete",
                "type": "note",
                "id": "note-todelete"
            }))
            .unwrap();
        let del_res = server
            .handle_delete_document(&json!({
                "id": "note-todelete"
            }))
            .unwrap();
        let del_text: String = del_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        assert_eq!(del_text, "Deleted: Doc To Delete (`note-todelete`)");
        assert!(!del_text.contains(root_str), "Must not leak root path");

        // 4. handle_decompose_task
        let parent_res = server
            .handle_create_task(&json!({
                "title": "Parent For Decompose",
                "type": "task",
                "project": "proj-alpha",
                "parent": "proj-alpha",
                "allow_missing_parent": true,
            }))
            .unwrap();
        let parent_text: String = parent_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        let parent_val: serde_json::Value = serde_json::from_str(&parent_text).unwrap();
        let parent_id = parent_val.get("id").unwrap().as_str().unwrap();

        let decomp_res = server
            .handle_decompose_task(&json!({
                "parent_id": parent_id,
                "subtasks": [
                    { "title": "Subtask 1", "type": "task" },
                    { "title": "Subtask 2", "type": "task" }
                ]
            }))
            .unwrap();
        let decomp_text: String = decomp_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(!decomp_text.contains(root_str), "Must not leak root path");
        assert!(!decomp_text.contains(".md"), "Must not leak .md extension");

        // 5. handle_sync_excalidraw created_nodes output shape
        let dummy_canvas = json!({
            "type": "excalidraw",
            "version": 2,
            "source": "https://excalidraw.com",
            "elements": [],
            "appState": { "viewBackgroundColor": "#ffffff", "gridSize": null },
            "files": {}
        });
        let sync_res = server
            .handle_sync_excalidraw(&json!({
                "canvas": dummy_canvas.to_string(),
                "dry_run": false
            }))
            .unwrap();
        let sync_text: String = sync_res
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(!sync_text.contains(root_str), "Must not leak root path");
        assert!(!sync_text.contains("\"path\":"), "Must not leak path key in created_nodes");

        // 6. get_document file not found error does not leak path
        let get_doc_err = server
            .handle_get_document(&json!({ "id": "non-existent-id" }))
            .unwrap_err();
        assert!(!get_doc_err.message.contains(root_str));
        assert!(!get_doc_err.message.contains("/"));

        // 7. get_task file not found error does not leak path
        let get_task_err = server
            .handle_get_task(&json!({ "id": "non-existent-task" }))
            .unwrap_err();
        assert!(!get_task_err.message.contains(root_str));
        assert!(!get_task_err.message.contains("/"));
    }
