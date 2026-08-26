use super::*;

    /// AC1 + AC6: the DEFAULT list_tasks output (no sort/order arg) is ordered by
    /// focus_score descending, asserted on the returned values (consecutive
    /// focus_scores are non-increasing), and is stable across repeated calls.
    #[test]
    fn test_top_n_by_metric() {
        let server = build_test_server();

        // 1. Test PageRank top 5
        let res_pr = server
            .handle_top_n_by_metric(&json!({
                "metric": "pagerank",
                "n": 5
            }))
            .unwrap();
        let pr_text = res_pr.content[0].raw.as_text().unwrap().text.as_str();
        let pr_val: serde_json::Value = serde_json::from_str(pr_text).unwrap();
        let pr_arr = pr_val.as_array().unwrap();
        assert_eq!(pr_arr.len(), 5);
        // Assert sorting descending
        let mut prev_val = f64::MAX;
        for item in pr_arr {
            let m_val = item.get("metric_value").unwrap().as_f64().unwrap();
            assert!(
                m_val <= prev_val,
                "pagerank must be sorted descending, saw {} then {}",
                prev_val,
                m_val
            );
            prev_val = m_val;
            assert!(item.get("id").is_some());
            assert!(item.get("title").is_some());
        }

        // 2. Test Betweenness top 3
        let res_bt = server
            .handle_top_n_by_metric(&json!({
                "metric": "betweenness",
                "n": 3
            }))
            .unwrap();
        let bt_text = res_bt.content[0].raw.as_text().unwrap().text.as_str();
        let bt_val: serde_json::Value = serde_json::from_str(bt_text).unwrap();
        let bt_arr = bt_val.as_array().unwrap();
        assert_eq!(bt_arr.len(), 3);
        let mut prev_val = f64::MAX;
        for item in bt_arr {
            let m_val = item.get("metric_value").unwrap().as_f64().unwrap();
            assert!(m_val <= prev_val, "betweenness must be sorted descending");
            prev_val = m_val;
        }

        // 3. Test Degree top 3
        let res_deg = server
            .handle_top_n_by_metric(&json!({
                "metric": "degree",
                "n": 3
            }))
            .unwrap();
        let deg_text = res_deg.content[0].raw.as_text().unwrap().text.as_str();
        let deg_val: serde_json::Value = serde_json::from_str(deg_text).unwrap();
        let deg_arr = deg_val.as_array().unwrap();
        assert_eq!(deg_arr.len(), 3);
        let mut prev_val = f64::MAX;
        for item in deg_arr {
            let m_val = item.get("metric_value").unwrap().as_f64().unwrap();
            assert!(m_val <= prev_val, "degree must be sorted descending");
            prev_val = m_val;
        }

        // 4. Test Node type filter (node_type = "epic" — the fixture's 3 containers)
        let res_proj = server
            .handle_top_n_by_metric(&json!({
                "metric": "degree",
                "node_type": "epic",
                "n": 10
            }))
            .unwrap();
        let proj_text = res_proj.content[0].raw.as_text().unwrap().text.as_str();
        let proj_val: serde_json::Value = serde_json::from_str(proj_text).unwrap();
        let proj_arr = proj_val.as_array().unwrap();
        // build_project_test_graph has exactly 3 epic containers: proj-alpha, proj-beta, proj-gamma
        assert_eq!(proj_arr.len(), 3);
        for item in proj_arr {
            let id = item.get("id").unwrap().as_str().unwrap();
            assert!(id.starts_with("proj-"));
        }
    }

    #[test]
    fn test_list_tasks_default_order_is_focus_desc() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"format": "json"}))
            .unwrap();
        let tasks = extract_task_objects(&result);
        assert!(
            tasks.len() >= 2,
            "fixture should return multiple tasks, got {}",
            tasks.len()
        );

        let scores: Vec<i64> = tasks
            .iter()
            .map(|t| {
                t.get("focus_score")
                    .and_then(|s| s.as_i64())
                    .unwrap_or(i64::MIN)
            })
            .collect();
        for w in scores.windows(2) {
            assert!(
                w[0] >= w[1],
                "focus_score must be non-increasing in default order, saw {} then {}. Full: {scores:?}",
                w[0], w[1]
            );
        }
    }

    /// AC2: every returned row carries a `status` field in the default projection.
    #[test]
    fn test_list_tasks_default_projects_status_on_every_row() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"format": "json"}))
            .unwrap();
        let tasks = extract_task_objects(&result);
        assert!(!tasks.is_empty(), "fixture should return tasks");
        for t in &tasks {
            let status = t.get("status").and_then(|s| s.as_str());
            assert!(
                status.is_some() && !status.unwrap().is_empty(),
                "every row must include a non-empty status, missing on: {t}"
            );
        }
    }

    /// AC6: default ordering is deterministic — two identical calls agree exactly.
    #[test]
    fn test_list_tasks_default_order_is_deterministic() {
        let server = build_test_server();
        let first = extract_task_ids(
            &server
                .handle_list_tasks(&json!({"format": "json"}))
                .unwrap(),
        );
        let second = extract_task_ids(
            &server
                .handle_list_tasks(&json!({"format": "json"}))
                .unwrap(),
        );
        assert_eq!(
            first, second,
            "repeated default calls must return identical ordering"
        );
        assert!(
            first.len() >= 2,
            "need multiple tasks to make the check meaningful"
        );
    }

    /// AC1: the `ready` special filter is also focus_score-DESC by default, with
    /// status present on every row.
    #[test]
    fn test_list_tasks_ready_order_is_focus_desc_with_status() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"status": "ready", "format": "json"}))
            .unwrap();
        let tasks = extract_task_objects(&result);
        assert!(!tasks.is_empty(), "fixture should have ready tasks");
        let scores: Vec<i64> = tasks
            .iter()
            .map(|t| {
                t.get("focus_score")
                    .and_then(|s| s.as_i64())
                    .unwrap_or(i64::MIN)
            })
            .collect();
        for w in scores.windows(2) {
            assert!(
                w[0] >= w[1],
                "ready focus_scores must be non-increasing: {scores:?}"
            );
        }
        for t in &tasks {
            assert!(
                t.get("status").and_then(|s| s.as_str()).is_some(),
                "ready row missing status: {t}"
            );
        }
    }

    /// AC4: an explicit `format` argument is honoured unchanged — the markdown
    /// projection still renders a table (additive status column does not break it)
    /// and the default-order change does not silently force JSON.
    #[test]
    fn test_list_tasks_explicit_markdown_format_honoured() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"format": "markdown"}))
            .unwrap();
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        assert!(
            text.contains("| # | ID |"),
            "explicit markdown format must render a table, got: {text}"
        );
        assert!(
            text.contains("Status"),
            "markdown table must include a Status column"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_err(),
            "explicit markdown must not be silently converted to JSON"
        );
    }

    // ── AC1: project filter returns only matching tasks, no leakage ──

    #[test]
    fn test_list_tasks_project_filter_returns_only_matching() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        // Default hides done/cancelled tasks — task-a3 (done) and task-a4 (archived) should not appear
        assert!(
            ids.contains(&"task-a1".to_string()),
            "should contain task-a1"
        );
        assert!(
            ids.contains(&"task-a2".to_string()),
            "should contain task-a2"
        );
        assert!(
            !ids.contains(&"task-a3".to_string()),
            "done task-a3 should be hidden by default"
        );
        assert!(
            !ids.contains(&"task-a4".to_string()),
            "archived task-a4 should be hidden by default"
        );
        // Should NOT contain tasks from other projects
        assert!(
            !ids.contains(&"task-b1".to_string()),
            "should not contain task-b1"
        );
        assert!(
            !ids.contains(&"task-b2".to_string()),
            "should not contain task-b2"
        );
        assert!(
            !ids.contains(&"task-g1".to_string()),
            "should not contain task-g1"
        );
        assert!(
            !ids.contains(&"task-orphan".to_string()),
            "should not contain orphan"
        );
    }

    #[test]
    fn test_list_tasks_include_done_surfaces_terminal_tasks() {
        let server = build_test_server();
        // With include_done=true, done tasks should appear
        let result = server
            .handle_list_tasks(
                &json!({"project": "ProjectAlpha", "include_done": true, "format": "json"}),
            )
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(
            ids.contains(&"task-a1".to_string()),
            "should contain task-a1"
        );
        assert!(
            ids.contains(&"task-a2".to_string()),
            "should contain task-a2"
        );
        assert!(
            ids.contains(&"task-a3".to_string()),
            "done task-a3 should appear with include_done=true"
        );
        assert!(
            ids.contains(&"task-a4".to_string()),
            "archived task-a4 should appear with include_done=true"
        );
    }

    #[test]
    fn test_done_tasks_no_live_focus_score_urgency() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(
                &json!({"project": "ProjectAlpha", "include_done": true, "format": "json"}),
            )
            .unwrap();

        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();

        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let tasks = parsed.get("tasks").unwrap().as_array().unwrap();

        // Find task-a3 (which is done)
        let task_a3 = tasks
            .iter()
            .find(|t| t.get("id").unwrap().as_str() == Some("task-a3"))
            .unwrap();

        assert_eq!(
            task_a3.get("focus_score"),
            Some(&serde_json::Value::Null),
            "done task should have null focus_score"
        );
        let signals = task_a3.get("signals").unwrap();
        assert_eq!(
            signals.get("urgency").unwrap().as_f64(),
            Some(0.0),
            "done task should have 0.0 urgency"
        );
    }

    #[test]
    fn test_list_tasks_explicit_status_done_overrides_default_filter() {
        let server = build_test_server();
        // status="done" explicitly requested — should return done tasks even without include_done
        let result = server
            .handle_list_tasks(&json!({"status": "done", "format": "json"}))
            .unwrap();
        let ids = extract_task_ids(&result);
        assert!(
            ids.contains(&"task-a3".to_string()),
            "explicit status=done should return done tasks"
        );
        assert!(
            !ids.contains(&"task-a1".to_string()),
            "active task should not appear in done filter"
        );
    }

    // ── AC2: case-insensitive matching ──

    #[test]
    fn test_list_tasks_project_filter_case_insensitive() {
        let server = build_test_server();
        let lower = server
            .handle_list_tasks(&json!({"project": "projectalpha", "format": "json"}))
            .unwrap();
        let upper = server
            .handle_list_tasks(&json!({"project": "PROJECTALPHA", "format": "json"}))
            .unwrap();
        let mixed = server
            .handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"}))
            .unwrap();
        let ids_lower = extract_task_ids(&lower);
        let ids_upper = extract_task_ids(&upper);
        let ids_mixed = extract_task_ids(&mixed);
        assert_eq!(ids_lower, ids_mixed, "lowercase should match mixed case");
        assert_eq!(ids_upper, ids_mixed, "uppercase should match mixed case");
        assert!(!ids_lower.is_empty(), "should return results");
    }

    // ── AC3a: composes with status + priority + assignee ──

    #[test]
    fn test_list_tasks_project_composes_with_other_filters() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({
                "project": "ProjectAlpha",
                "status": "ready",
                "priority": 1,
                "assignee": "alice",
                "format": "json"
            }))
            .unwrap();
        let ids = extract_task_ids(&result);
        // Only task-a1 matches: ProjectAlpha + active + priority 1 + assignee alice
        assert_eq!(ids, vec!["task-a1".to_string()]);
    }

    // ── AC3b: composes with status="ready" (different code path) ──

    #[test]
    fn test_list_tasks_project_composes_with_ready_status() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({
                "project": "ProjectAlpha",
                "status": "ready",
                "format": "json"
            }))
            .unwrap();
        let ids = extract_task_ids(&result);
        // task-a1 has a dependent (task-a2 depends on it), so task-a1 is not a leaf
        // task-a2 depends on task-a1 (unmet dep), so task-a2 is not ready
        // task-a3 is done, so not ready
        // The ready tasks in ProjectAlpha depend on the graph's ready_tasks() logic
        // Key assertion: no beta/gamma/orphan tasks leak through
        for id in &ids {
            assert!(
                id.starts_with("task-a"),
                "ready+project=ProjectAlpha should only return alpha tasks, got {}",
                id
            );
        }

        // Also verify that beta ready tasks are excluded
        let beta_result = server
            .handle_list_tasks(&json!({
                "project": "ProjectBeta",
                "status": "ready",
                "format": "json"
            }))
            .unwrap();
        let beta_ids = extract_task_ids(&beta_result);
        // task-b1 should be ready (leaf, no deps, active)
        // task-b2 depends on task-b1, so blocked
        for id in &beta_ids {
            assert!(
                id.starts_with("task-b"),
                "ready+project=ProjectBeta should only return beta tasks, got {}",
                id
            );
        }
    }

    // ── AC4: works for multiple distinct projects ──

    #[test]
    fn test_list_tasks_project_filter_multiple_projects() {
        let server = build_test_server();

        let alpha = server
            .handle_list_tasks(&json!({"project": "ProjectAlpha", "format": "json"}))
            .unwrap();
        let beta = server
            .handle_list_tasks(&json!({"project": "ProjectBeta", "format": "json"}))
            .unwrap();
        let gamma = server
            .handle_list_tasks(&json!({"project": "ProjectGamma", "format": "json"}))
            .unwrap();

        let alpha_ids = extract_task_ids(&alpha);
        let beta_ids = extract_task_ids(&beta);
        let gamma_ids = extract_task_ids(&gamma);

        assert!(!alpha_ids.is_empty(), "ProjectAlpha should have tasks");
        assert!(!beta_ids.is_empty(), "ProjectBeta should have tasks");
        assert!(!gamma_ids.is_empty(), "ProjectGamma should have tasks");

        // Verify no overlap
        for id in &alpha_ids {
            assert!(
                !beta_ids.contains(id),
                "alpha task {} should not be in beta",
                id
            );
            assert!(
                !gamma_ids.contains(id),
                "alpha task {} should not be in gamma",
                id
            );
        }
        for id in &beta_ids {
            assert!(
                !gamma_ids.contains(id),
                "beta task {} should not be in gamma",
                id
            );
        }
    }

    // ── AC5: non-existent project returns empty, not error ──

    #[test]
    fn test_list_tasks_project_filter_nonexistent_returns_empty() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"project": "NonExistentProject", "format": "json"}))
            .unwrap();
        // Should succeed (not error), and return empty or "no tasks found" message
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        // Either empty JSON tasks array or "No tasks found" message
        let is_empty = text.contains("No tasks found")
            || text.contains("\"tasks\":[]")
            || text.contains("\"tasks\": []");
        assert!(
            is_empty,
            "non-existent project should return empty: {}",
            text
        );
    }

    // ── partial status: live round-trip (create → release partial → list partial → appears) ──
    //
    // AC1 of mem-f9855d1e: the server must accept `partial` as a first-class task
    // status on release_task, and `list_tasks(status="partial")` must filter to it
    // rather than the prior behaviour (alias to in_progress → silently matched a
    // different set). Self-contained temp dir so create/release writes don't touch
    // the shared /tmp fixture other tests rely on.
    #[test]

    fn test_task_search_schema_includes_include_done() {
        let tools = PkbSearchServer::get_all_tools();
        let task_search = tools
            .iter()
            .find(|t| t.name.as_ref() == "task_search")
            .expect("task_search tool should exist");
        let schema = serde_json::to_string(&task_search.input_schema).unwrap();
        assert!(
            schema.contains("\"include_done\""),
            "task_search schema should advertise include_done parameter"
        );
    }

    // ── list_tasks: schema advertises include_done ──

    #[test]
    fn test_list_tasks_schema_includes_include_done() {
        let tools = PkbSearchServer::get_all_tools();
        let list_tasks = tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist");
        let schema = serde_json::to_string(&list_tasks.input_schema).unwrap();
        assert!(
            schema.contains("\"include_done\""),
            "list_tasks schema should advertise include_done parameter"
        );
    }

    // ── focus_score description ──

    #[test]
    fn test_list_tasks_description_mentions_focus_score() {
        let tools = PkbSearchServer::get_all_tools();
        let list_tasks = tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist");
        let desc = list_tasks.description.as_deref().unwrap_or("");
        assert!(
            desc.contains("focus_score"),
            "list_tasks description should mention focus_score, got: {desc}"
        );
    }

    #[test]
    fn test_get_task_description_mentions_focus_score() {
        let tools = PkbSearchServer::get_all_tools();
        let get_task = tools
            .iter()
            .find(|t| t.name.as_ref() == "get_task")
            .expect("get_task tool should exist");
        let desc = get_task.description.as_deref().unwrap_or("");
        assert!(
            desc.contains("focus_score"),
            "get_task description should mention focus_score, got: {desc}"
        );
    }

    // ── Payload shape: focus_score top-level, signals nested ──

    #[test]
    fn test_list_tasks_focus_score_toplevel_signals_nested() {
        let server = build_test_server();
        let result = server
            .handle_list_tasks(&json!({"status": "ready", "format": "json"}))
            .unwrap();
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        let val: serde_json::Value =
            serde_json::from_str(&text).expect("list_tasks json output should parse");
        let tasks = val
            .get("tasks")
            .and_then(|t| t.as_array())
            .expect("should have tasks array");
        assert!(!tasks.is_empty(), "fixture should have ready tasks");
        for t in tasks {
            assert!(
                t.get("focus_score").is_some(),
                "focus_score must be a top-level field"
            );
            let signals = t
                .get("signals")
                .and_then(|s| s.as_object())
                .expect("signals must be a top-level object");
            for key in &[
                "criticality",
                "urgency",
                "downstream_weight",
                "scope",
                "uncertainty",
                "voi_value",
            ] {
                assert!(
                    signals.contains_key(*key),
                    "signals.{key} must exist under signals, not at top level"
                );
                assert!(
                    t.get(key).is_none(),
                    "{key} must NOT appear at the top level (belongs in signals)"
                );
            }
        }
    }

    #[test]
    fn test_get_task_focus_score_toplevel_signals_nested() {
        // get_task reads from disk, so we build a self-contained server with
        // actual files in a dedicated temp directory (separate from the shared
        // /tmp/test-pkb-project used by other tests to avoid interference).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let task_dir = root.join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(
            task_dir.join("task-shape-test.md"),
            "---\ntitle: Shape Test Task\ntype: task\nstatus: active\nid: task-shape-test\n---\n\nBody.\n",
        )
        .unwrap();

        let doc = PkbDocument {
            path: PathBuf::from("tasks/task-shape-test.md"),
            title: "Shape Test Task".to_string(),
            body: "Body.".to_string(),
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::json!({
                "title": "Shape Test Task",
                "type": "task",
                "status": "active",
                "id": "task-shape-test",
            })),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        };
        let graph = GraphStore::build(&[doc], root);
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

        let result = server
            .handle_get_task(&json!({"id": "task-shape-test"}))
            .unwrap();
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>();
        let t: serde_json::Value =
            serde_json::from_str(&text).expect("get_task json output should parse");

        assert!(
            t.get("focus_score").is_some(),
            "focus_score must be a top-level field in get_task response"
        );
        let signals = t
            .get("signals")
            .and_then(|s| s.as_object())
            .expect("signals must be a top-level object in get_task response");
        for key in &[
            "criticality",
            "urgency",
            "downstream_weight",
            "scope",
            "uncertainty",
            "voi_value",
        ] {
            assert!(
                signals.contains_key(*key),
                "signals.{key} must exist under signals in get_task response"
            );
            assert!(
                t.get(key).is_none(),
                "{key} must NOT appear at the top level in get_task response (belongs in signals)"
            );
        }
    }

    // ── has_superseded_by filter on list_tasks (mem_6cda18bf) ──────────────

    #[test]
    fn test_list_tasks_schema_includes_has_superseded_by_parameter() {
        let tools = PkbSearchServer::get_all_tools();
        let list_tasks_tool = tools
            .iter()
            .find(|t| t.name.as_ref() == "list_tasks")
            .expect("list_tasks tool should exist");
        let schema = serde_json::to_string(&list_tasks_tool.input_schema).unwrap();
        assert!(
            schema.contains("\"has_superseded_by\""),
            "list_tasks schema should include 'has_superseded_by' parameter, got: {}",
            schema
        );
        let schema_val: serde_json::Value = serde_json::from_str(&schema).unwrap();
        let prop = schema_val
            .get("properties")
            .and_then(|p| p.get("has_superseded_by"))
            .expect("properties.has_superseded_by must exist");
        assert_eq!(
            prop.get("type").and_then(|t| t.as_str()),
            Some("boolean"),
            "has_superseded_by must have type: boolean"
        );
    }

    #[test]
    fn test_list_tasks_has_superseded_by_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // 1. Task with superseded_by at an OPEN status (ready)
        let mut fm_open = serde_json::Map::new();
        fm_open.insert("title".to_string(), json!("Open Superseded Task"));
        fm_open.insert("type".to_string(), json!("task"));
        fm_open.insert("status".to_string(), json!("ready"));
        fm_open.insert("id".to_string(), json!("task-open-superseded"));
        fm_open.insert("superseded_by".to_string(), json!("task-canonical-1"));
        let doc_open = PkbDocument {
            path: PathBuf::from("tasks/task-open-superseded.md"),
            title: "Open Superseded Task".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_open)),
            content_hash: "h1".to_string(),
            file_hash: "h1".to_string(),
        };

        // 2. Task with superseded_by at a CLOSED status (done)
        let mut fm_closed = serde_json::Map::new();
        fm_closed.insert("title".to_string(), json!("Closed Superseded Task"));
        fm_closed.insert("type".to_string(), json!("task"));
        fm_closed.insert("status".to_string(), json!("done"));
        fm_closed.insert("id".to_string(), json!("task-closed-superseded"));
        fm_closed.insert("superseded_by".to_string(), json!("task-canonical-2"));
        let doc_closed = PkbDocument {
            path: PathBuf::from("tasks/task-closed-superseded.md"),
            title: "Closed Superseded Task".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("done".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_closed)),
            content_hash: "h2".to_string(),
            file_hash: "h2".to_string(),
        };

        // 3. Task with superseded_by absent at an OPEN status
        let mut fm_normal = serde_json::Map::new();
        fm_normal.insert("title".to_string(), json!("Normal Open Task"));
        fm_normal.insert("type".to_string(), json!("task"));
        fm_normal.insert("status".to_string(), json!("ready"));
        fm_normal.insert("id".to_string(), json!("task-normal-open"));
        let doc_normal = PkbDocument {
            path: PathBuf::from("tasks/task-normal-open.md"),
            title: "Normal Open Task".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_normal)),
            content_hash: "h3".to_string(),
            file_hash: "h3".to_string(),
        };

        let graph = GraphStore::build(&[doc_open, doc_closed, doc_normal], root);
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

        // A. Filter has_superseded_by: true (default include_done=false)
        // Must return ONLY task-open-superseded (1 task), excluding closed and absent
        let res_filtered_json = server
            .handle_list_tasks(&json!({"has_superseded_by": true, "format": "json"}))
            .unwrap();
        let json_tasks = extract_task_objects(&res_filtered_json);
        assert_eq!(
            json_tasks.len(),
            1,
            "has_superseded_by: true should return exactly 1 open task, got {:?}",
            json_tasks
        );
        let t0 = &json_tasks[0];
        assert_eq!(t0.get("id").and_then(|v| v.as_str()), Some("task-open-superseded"));
        // AC2: returned row carries its superseded_by value
        assert_eq!(
            t0.get("superseded_by").and_then(|v| v.as_str()),
            Some("task-canonical-1"),
            "returned task row must carry superseded_by value"
        );

        // B. Filter has_superseded_by: true with include_done: true
        // Must return both open and closed tasks with superseded_by (2 tasks)
        let res_with_done = server
            .handle_list_tasks(&json!({"has_superseded_by": true, "include_done": true, "format": "json"}))
            .unwrap();
        let done_tasks = extract_task_objects(&res_with_done);
        assert_eq!(
            done_tasks.len(),
            2,
            "has_superseded_by: true with include_done should return 2 tasks"
        );

        // C. Filter has_superseded_by: false
        // Must return task-normal-open (1 task)
        let res_not_superseded = server
            .handle_list_tasks(&json!({"has_superseded_by": false, "format": "json"}))
            .unwrap();
        let normal_tasks = extract_task_objects(&res_not_superseded);
        assert_eq!(
            normal_tasks.len(),
            1,
            "has_superseded_by: false should return 1 normal task"
        );
        assert_eq!(
            normal_tasks[0].get("id").and_then(|v| v.as_str()),
            Some("task-normal-open")
        );

        // D. Empty result set: query for non-existent condition
        let res_empty = server
            .handle_list_tasks(&json!({"has_superseded_by": true, "project": "nonexistent", "format": "json"}))
            .unwrap();
        let text_empty = res_empty.content[0].raw.as_text().unwrap().text.as_str();
        assert!(
            text_empty.contains("No tasks found matching filters.") || text_empty.contains("\"total\": 0"),
            "empty result set must be reported cleanly, got: {}",
            text_empty
        );

        // E. Markdown format with has_superseded_by: true includes Superseded By column
        let res_filtered_md = server
            .handle_list_tasks(&json!({"has_superseded_by": true, "format": "markdown"}))
            .unwrap();
        let md_text = res_filtered_md.content[0].raw.as_text().unwrap().text.as_str();
        assert!(
            md_text.contains("Superseded By"),
            "markdown table must contain 'Superseded By' header when filtered, got: {}",
            md_text
        );
        assert!(
            md_text.contains("task-canonical-1"),
            "markdown table must contain 'task-canonical-1', got: {}",
            md_text
        );

        // F. AC3: Unfiltered query does NOT include superseded_by and has unmodified default columns
        let res_unfiltered_md = server
            .handle_list_tasks(&json!({"format": "markdown"}))
            .unwrap();
        let unf_md_text = res_unfiltered_md.content[0].raw.as_text().unwrap().text.as_str();
        assert!(
            !unf_md_text.contains("Superseded By"),
            "unfiltered markdown must NOT contain 'Superseded By' column"
        );

        let res_unfiltered_json = server
            .handle_list_tasks(&json!({"format": "json"}))
            .unwrap();
        let unf_json_tasks = extract_task_objects(&res_unfiltered_json);
        for task in unf_json_tasks {
            assert!(
                task.get("superseded_by").is_none(),
                "unfiltered json task rows must NOT contain 'superseded_by' key"
            );
        }
    }


