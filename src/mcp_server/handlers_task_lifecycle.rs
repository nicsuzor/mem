use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::graph::is_completed;
use crate::graph_store::GraphStore;

use super::{PkbSearchServer, MAX_RESULTS};

impl PkbSearchServer {
    pub(crate) fn handle_claim_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let resolved_type = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Task not found: {id}")),
                data: None,
            })?;
            node.node_type.clone()
        };

        match resolved_type.as_deref() {
            Some("template") => self.claim_template_instance(id),
            None | Some("task") => self.claim_task_in_place(id, args),
            Some(other) => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Node '{id}' has type '{other}', not 'template' or 'task'. \
                     claim_task only works on template or task nodes."
                )),
                data: None,
            }),
        }
    }

    /// Claim a regular `type: task` (or untyped) node IN PLACE: set
    /// `status: in_progress`, optionally record `assignee`/`session_id`, and
    /// return the task via get_task. No new node is created — this is
    /// distinct from `claim_template_instance`'s datestamped-copy behavior.
    pub(crate) fn claim_task_in_place(&self, id: &str, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let mut update_args = serde_json::Map::new();
        update_args.insert("id".to_string(), JsonValue::String(id.to_string()));
        update_args.insert(
            "status".to_string(),
            JsonValue::String("in_progress".to_string()),
        );
        if let Some(assignee) = args.get("assignee").and_then(|v| v.as_str()) {
            update_args.insert(
                "assignee".to_string(),
                JsonValue::String(assignee.to_string()),
            );
        }
        // session_id: arg or env $AOPS_SESSION_ID (same convention as release_task).
        let session_id = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| std::env::var("AOPS_SESSION_ID").ok());
        if let Some(sid) = session_id {
            update_args.insert("session_id".to_string(), JsonValue::String(sid));
        }

        self.handle_update_task(&JsonValue::Object(update_args))?;

        // Return the claimed task via get_task, matching claim_task's
        // established return shape (a full task, not update_task's plain
        // confirmation string).
        let get_args = serde_json::json!({ "id": id });
        self.handle_get_task(&get_args).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "claim_task set '{id}' to in_progress but the update is not yet visible in \
                 the graph. Underlying error: {}. Retry get_task in a moment.",
                e.message,
            )),
            data: None,
        })
    }

    /// Instantiate a `type: template` task.
    ///
    /// Creates a datestamped instance (`<slug>-<YYYYMMDD>-<HHMM>-<host>.md`) and
    /// returns the new task via get_task. The template itself is not modified.
    pub(crate) fn claim_template_instance(&self, id: &str) -> Result<CallToolResult, McpError> {
        // Resolve the node and validate it is a template.
        let (
            template_id,
            template_path,
            template_label,
            template_tags,
            template_priority,
            template_assignee,
        ) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Task not found: {id}")),
                data: None,
            })?;

            if node.node_type.as_deref() != Some("template") {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Node '{}' has type '{}', not 'template'. \
                         claim_task only works on template nodes.",
                        id,
                        node.node_type.as_deref().unwrap_or("unknown")
                    )),
                    data: None,
                });
            }

            (
                node.id.clone(),
                node.path.clone(),
                node.label.clone(),
                node.tags.clone(),
                node.priority,
                node.assignee.clone(),
            )
        };

        // Read template file to get body + frontmatter project/parent fields.
        let abs_path = self.abs_path(&template_path);
        let content = std::fs::read_to_string(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read template file: {e}")),
            data: None,
        })?;

        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter.parse(&content);

        let fm: serde_json::Value = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<serde_json::Value>().ok())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let template_project = fm.get("project").and_then(|v| v.as_str()).map(String::from);
        let template_parent = fm.get("parent").and_then(|v| v.as_str()).map(String::from);

        let consequence = fm
            .get("consequence")
            .and_then(|v| v.as_str())
            .map(String::from);

        let contributes_to = fm
            .get("contributes_to")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let depends_on = crate::graph::parse_string_array(&fm, "depends_on");
        let soft_depends_on = crate::graph::parse_string_array(&fm, "soft_depends_on");

        let goal_type = fm
            .get("goal_type")
            .and_then(|v| v.as_str())
            .map(String::from);

        let severity = fm
            .get("severity")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);

        let stakeholder = fm
            .get("stakeholder")
            .and_then(|v| v.as_str())
            .map(String::from);

        let body = parsed.content.trim().to_string();

        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        // Create the datestamped instance.
        let instance_path = crate::document_crud::claim_template_instance(
            &self.pkb_root,
            crate::document_crud::TemplateInstanceFields {
                template_title: template_label,
                template_body: body,
                tags: template_tags,
                priority: template_priority,
                project: template_project,
                parent: template_parent,
                consequence,
                contributes_to,
                depends_on,
                soft_depends_on,
                goal_type,
                severity,
                stakeholder,
                template_id: template_id.clone(),
                assignee: template_assignee,
            },
        )
        .map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create template instance: {e}")),
            data: None,
        })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::claim_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&instance_path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::claim_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        // Register instance in the graph.
        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::claim_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::claim_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                instance_path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::claim_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::claim_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        // Return via get_task for consistent shape.
        let instance_id = instance_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                instance_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });

        let get_args = serde_json::json!({ "id": instance_id });
        self.handle_get_task(&get_args).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "claim_task created '{}' from template '{}' but the instance is not \
                 yet visible in the graph. Underlying error: {}. Retry get_task in a moment.",
                instance_id, template_id, e.message,
            )),
            data: None,
        })
    }

    // =========================================================================
    // KNOWLEDGE GRAPH TOOLS (4)
    // =========================================================================

    pub(crate) fn build_mutation_neighborhood(
        graph: &GraphStore,
        node_id: &str,
        cascade_closed: usize,
    ) -> serde_json::Value {
        use crate::graph::{is_closed_for_hierarchy, is_completed, GraphNode};

        let Some(node) = graph.get_node(node_id) else {
            return serde_json::Value::Null;
        };

        // Titles are truncated for display at 80 chars on a UTF-8 char boundary.
        let title_of = |n: &GraphNode| -> String {
            let t = n.label.as_str();
            t[..t.floor_char_boundary(80)].to_string()
        };

        let mut neighborhood = serde_json::Map::new();

        // --- parent + siblings (omitted when the task has no parent) ---
        if let Some(parent_id) = node.parent.as_deref() {
            if let Some(parent) = graph.get_node(parent_id) {
                let mut siblings_open = 0usize;
                let mut sample: Vec<serde_json::Value> = Vec::new();
                for child_id in &parent.children {
                    if child_id == node_id {
                        continue;
                    }
                    let Some(child) = graph.get_node(child_id) else {
                        continue;
                    };
                    // archived/done/cancelled all count as closed for hierarchy.
                    if is_closed_for_hierarchy(child.status.as_deref()) {
                        continue;
                    }
                    siblings_open += 1;
                    if sample.len() < 3 {
                        sample.push(serde_json::json!({
                            "id": child_id,
                            "title": title_of(child),
                            "status": child.status,
                        }));
                    }
                }
                let mut parent_obj = serde_json::Map::new();
                parent_obj.insert("id".into(), serde_json::json!(parent_id));
                parent_obj.insert("title".into(), serde_json::json!(title_of(parent)));
                parent_obj.insert("status".into(), serde_json::json!(parent.status));
                parent_obj.insert("siblings_open".into(), serde_json::json!(siblings_open));
                if siblings_open > 0 {
                    parent_obj.insert("siblings_sample".into(), serde_json::Value::Array(sample));
                }
                neighborhood.insert("parent".into(), serde_json::Value::Object(parent_obj));
            }
        }

        // --- children: only when carrying a fact the close did not already imply ---
        let mut children_obj = serde_json::Map::new();
        if cascade_closed > 0 {
            children_obj.insert(
                "closed_by_cascade".into(),
                serde_json::json!(cascade_closed),
            );
        }
        let open_children = node
            .children
            .iter()
            .filter(|c| {
                graph
                    .get_node(c)
                    .is_some_and(|n| !is_closed_for_hierarchy(n.status.as_deref()))
            })
            .count();
        if open_children > 0 {
            children_obj.insert("open".into(), serde_json::json!(open_children));
        }
        if !children_obj.is_empty() {
            neighborhood.insert("children".into(), serde_json::Value::Object(children_obj));
        }

        // --- unblocked: dependents (node.blocks) now clear of all blockers, ≤5 ---
        let mut unblocked: Vec<serde_json::Value> = Vec::new();
        for dep_id in &node.blocks {
            if unblocked.len() >= 5 {
                break;
            }
            let Some(dep) = graph.resolve(dep_id) else {
                continue;
            };
            if !graph.is_blocked(&dep.id) && !is_completed(dep.status.as_deref()) {
                unblocked.push(serde_json::json!({
                    "id": dep.id,
                    "title": title_of(dep),
                }));
            }
        }
        if !unblocked.is_empty() {
            neighborhood.insert("unblocked".into(), serde_json::Value::Array(unblocked));
        }

        if neighborhood.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(neighborhood)
        }
    }

    pub(crate) fn handle_complete_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let evidence =
            Self::require_evidence(args.get("completion_evidence").and_then(|v| v.as_str()))?;

        let pr_url = args.get("pr_url").and_then(|v| v.as_str());

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Collect open descendants before any writes.
        let open_descs: Vec<(String, std::path::PathBuf)> = graph
            .open_descendants(&node.id)
            .into_iter()
            .filter_map(|desc_id| {
                graph
                    .get_node(&desc_id)
                    .map(|n| (desc_id, self.abs_path(&n.path)))
            })
            .collect();

        if !open_descs.is_empty() && !recursive {
            let ids: Vec<&str> = open_descs.iter().map(|(id, _)| id.as_str()).collect();
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Cannot close '{}': {} open child task(s) still exist: {}. \
                     Complete or cancel them first, or pass recursive=true to close all descendants.",
                    id,
                    open_descs.len(),
                    ids.join(", ")
                )),
                data: None,
            });
        }

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        let node_id = node.id.clone();
        drop(graph);

        // Number of descendants this close cascades shut (0 unless recursive).
        let cascade_closed = if recursive { open_descs.len() } else { 0 };

        let t_total = std::time::Instant::now();

        // Cascade-close open descendants when recursive=true.
        if recursive && !open_descs.is_empty() {
            let mut desc_updates = std::collections::HashMap::new();
            desc_updates.insert(
                "status".to_string(),
                serde_json::Value::String("done".to_string()),
            );
            for (desc_id, desc_path) in &open_descs {
                crate::document_crud::update_document(desc_path, desc_updates.clone())
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to recursively close child task '{desc_id}': {e}"
                        )),
                        data: None,
                    })?;
                if let Some(doc) = crate::pkb::parse_file_relative(desc_path, &self.pkb_root)
                {
                    self.rebuild_graph_for_pkb_document(&doc);
                    self.try_upsert_document(&doc);
                }
            }
        }

        let mut updates = std::collections::HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String("done".to_string()),
        );
        if let Some(url) = pr_url {
            updates.insert(
                "pr_url".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }

        let t = std::time::Instant::now();
        crate::document_crud::update_document(&abs_path, updates).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to complete task: {e}")),
            data: None,
        })?;

        // Append completion evidence to the document body
        Self::append_evidence(&abs_path, evidence, pr_url)?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::complete_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::complete_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::complete_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::complete_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::complete_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::complete_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        // Compute the neighborhood against the post-write graph.
        let neighborhood = {
            let graph = self.graph.read();
            Self::build_mutation_neighborhood(&graph, &node_id, cascade_closed)
        };

        let response = serde_json::json!({
            "ok": true,
            "id": id,
            "status": "done",
            "message": format!("Completed: {} (`{}`)", label, id),
            "neighborhood": neighborhood,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap_or_default(),
        )]))
    }

    /// M3: Semantic attach — search open epics for a confident topic match.
    ///
    /// Confidence basis: BGE-M3 cosine similarity ≥ 0.75 against the session
    /// summary, filtered to `type: epic` with non-terminal status in the vector
    /// store. Score 0.75 is calibrated against real session summaries: on-topic
    /// epics score 0.80+; unrelated epics cluster below 0.60. Fail-safe: any
    /// error or below-threshold result returns `None`, falling through to M2.
    pub(crate) fn find_topic_epic(&self, summary: &str) -> Option<(String, f32, String)> {
        const CONFIDENCE_THRESHOLD: f32 = 0.75;

        let embedding = match self.embedder.encode_query(summary) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(target: "adhoc_grouping", "M3: embed failed, skipping topic attach: {e}");
                return None;
            }
        };

        // Fetch extra candidates so type-filtering has enough to choose from.
        let candidates = self
            .store
            .read()
            .search(&embedding, 50, &self.pkb_root, None, None, None);

        for candidate in &candidates {
            if candidate.score < CONFIDENCE_THRESHOLD {
                break; // sorted descending — everything from here on is below threshold
            }
            let is_epic = candidate
                .doc_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("epic"))
                .unwrap_or(false);
            if !is_epic {
                continue;
            }
            let status_raw = candidate
                .status
                .as_deref()
                .unwrap_or("inbox")
                .to_ascii_lowercase();
            let canonical_status = crate::graph::resolve_status_alias(&status_raw);
            if matches!(canonical_status, "done" | "cancelled") {
                continue;
            }
            if !candidate.id.is_empty() {
                return Some((
                    candidate.id.clone(),
                    candidate.score,
                    candidate.title.clone(),
                ));
            }
        }
        None
    }

    /// M2: Find or create a deterministic per-session epic.
    ///
    /// Epic ID: `adhoc-{md5(session_id)[..8]}` — stable across multiple
    /// `release_task` calls from the same session so all releases from one
    /// session cohere into a single thread. Parented under the ad-hoc sessions
    /// root. Title derives from the first-seen release summary (truncated).
    pub(crate) fn find_or_create_session_epic(
        &self,
        session_id: &str,
        title_hint: &str,
    ) -> Result<String, McpError> {
        let hash = format!("{:x}", md5::compute(session_id.as_bytes()));
        let epic_id = format!("adhoc_{}", &hash[..8]);

        // Fast path: already present in graph
        {
            let graph = self.graph.read();
            if graph.resolve(&epic_id).is_some() {
                tracing::debug!(target: "adhoc_grouping", epic_id = epic_id.as_str(), "M2: reusing existing session epic");
                return Ok(epic_id);
            }
        }

        // Slow path: create the epic
        crate::document_crud::ensure_adhoc_sessions_root(&self.pkb_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to ensure adhoc-sessions root: {e}")),
            data: None,
        })?;

        {
            let graph = self.graph.read();
            if graph
                .resolve(crate::document_crud::ADHOC_SESSIONS_ROOT_ID)
                .is_none()
            {
                drop(graph);
                self.rebuild_graph();
            }
        }

        let hint = title_hint.trim();
        let hint_end = hint.floor_char_boundary(hint.len().min(80));
        let truncated = hint[..hint_end].trim_end();
        let title = if truncated.is_empty() {
            format!("Session {session_id}")
        } else {
            format!("Session {session_id}: {truncated}")
        };

        let fields = crate::document_crud::TaskFields {
            title,
            id: Some(epic_id.clone()),
            parent: Some(crate::document_crud::ADHOC_SESSIONS_ROOT_ID.to_string()),
            project: Some("adhoc-sessions".to_string()),
            task_type: Some("epic".to_string()),
            tags: vec!["adhoc".to_string(), "session-epic".to_string()],
            session_id: Some(session_id.to_string()),
            ..Default::default()
        };

        let path = match crate::document_crud::create_task(&self.pkb_root, fields) {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("already exists") {
                    // Concurrent request created the same epic; reuse it.
                    tracing::debug!(target: "adhoc_grouping", epic_id = epic_id.as_str(), "M2: session epic created concurrently, reusing");
                    return Ok(epic_id);
                }
                return Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to create session epic: {e}")),
                    data: None,
                });
            }
        };

        if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            self.rebuild_graph();
        }

        tracing::info!(target: "adhoc_grouping", epic_id = epic_id.as_str(), session_id, "M2: created session epic");
        Ok(epic_id)
    }

    /// Release a task to a handoff/terminal status with required summary.
    /// Create an ad-hoc task for a session release when no ID is provided.
    pub(crate) fn create_adhoc_task(&self, args: &JsonValue) -> Result<String, McpError> {
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: project. Ad-hoc tasks created via release_task require a project parameter for filing (e.g. project=\"aops\").",
                ),
                data: None,
            })?;

        let summary = args.get("summary").and_then(|v| v.as_str()).unwrap_or("");

        // Truncate summary to 200 chars for title
        let mut title = summary.trim().replace('\n', " ");
        if title.len() > 200 {
            let end = title.floor_char_boundary(200);
            let last_space = title[..end].rfind(' ').unwrap_or(end);
            title.truncate(last_space);
        }
        if title.is_empty() {
            title = "Ad-hoc Session Task".to_string();
        }

        // Ensure adhoc-sessions root exists
        crate::document_crud::ensure_adhoc_sessions_root(&self.pkb_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to ensure adhoc-sessions root: {e}")),
            data: None,
        })?;

        // Rebuild graph if adhoc-sessions is missing from memory
        {
            let graph = self.graph.read();
            if graph.resolve("adhoc-sessions").is_none() {
                drop(graph);
                self.rebuild_graph();
            }
        }

        let session_id_opt = args.get("session_id").and_then(|v| v.as_str());

        // M3 → M2 → root fallback: try topic match, then per-session epic, then root.
        let parent_id = if let Some((epic_id, score, ref epic_title)) =
            self.find_topic_epic(summary)
        {
            tracing::info!(
                target: "adhoc_grouping",
                routing = "M3",
                score,
                epic_id = epic_id.as_str(),
                epic_title = epic_title.as_str(),
                "M3: attaching session release to topic epic"
            );
            epic_id
        } else if let Some(sid) = session_id_opt {
            let epic_id = self.find_or_create_session_epic(sid, summary)?;
            tracing::info!(
                target: "adhoc_grouping",
                routing = "M2",
                session_id = sid,
                epic_id = epic_id.as_str(),
                "M2: attaching session release to per-session epic (no confident topic match)"
            );
            epic_id
        } else {
            tracing::info!(
                target: "adhoc_grouping",
                routing = "fallback",
                "Fallback: no session_id and no confident topic match; parenting to adhoc-sessions root"
            );
            crate::document_crud::ADHOC_SESSIONS_ROOT_ID.to_string()
        };

        let fields = crate::document_crud::TaskFields {
            title,
            parent: Some(parent_id),
            project: Some(project.to_string()),
            tags: vec!["adhoc".to_string(), "session-release".to_string()],
            session_id: session_id_opt.map(String::from),
            issue_url: args
                .get("issue_url")
                .and_then(|v| v.as_str())
                .map(String::from),
            release_summary: args
                .get("release_summary")
                .and_then(|v| v.as_str())
                .map(String::from),
            follow_up_tasks: args
                .get("follow_up_tasks")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        let path =
            crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Failed to create ad-hoc task: {e}")),
                data: None,
            })?;

        // Extract ID directly from parsed frontmatter
        let id = if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
            let doc_id = doc.id();
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
            doc_id
        } else {
            self.rebuild_graph();
            static ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(r"^([a-z0-9_]+_[0-9a-f]{8}|[a-z0-9_]+-[0-9a-f]{8})").unwrap()
            });
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| ID_RE.find(s).map(|m| m.as_str()).unwrap_or(s))
                .unwrap_or("task-unknown")
                .to_string()
        };

        Ok(id)
    }

    pub(crate) fn handle_release_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: status. Must be one of: merge_ready, done, review, blocked, cancelled, partial.",
                ),
                data: None,
            })?;

        // Validate status enum with helpful suggestions
        let valid_statuses = [
            "merge_ready",
            "done",
            "review",
            "blocked",
            "cancelled",
            "partial",
        ];
        if !valid_statuses.contains(&status) {
            let suggestion = match status {
                "complete" | "completed" => " Did you mean \"done\"?",
                "ready" | "merge-ready" => " Did you mean \"merge_ready\"?",
                "cancel" => " Did you mean \"cancelled\"?",
                _ => "",
            };
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Invalid status \"{status}\". Must be one of: merge_ready, done, review, blocked, cancelled, partial.{suggestion}\n\
                     For non-terminal updates (priority, tags, assignee), use update_task instead."
                )),
                data: None,
            });
        }

        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: summary. Describe what was done before releasing this task.\n\
                     Example: release_task(id=\"task-abc\", status=\"merge_ready\", summary=\"Implemented X with Y\", pr_url=\"https://...\")",
                ),
                data: None,
            })?;
        if summary.trim().is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "summary cannot be empty. Describe what was done before releasing this task.",
                ),
                data: None,
            });
        }

        let mut created_id = None;
        let id_val = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => {
                let nid = self.create_adhoc_task(args)?;
                created_id = Some(nid.clone());
                nid
            }
        };
        let id = &id_val;

        let pr_url = args.get("pr_url").and_then(|v| v.as_str());
        let branch = args.get("branch").and_then(|v| v.as_str());
        let blocker = args.get("blocker").and_then(|v| v.as_str());
        let reason = args.get("reason").and_then(|v| v.as_str());
        let session_id_arg = args.get("session_id").and_then(|v| v.as_str());
        let issue_url = args.get("issue_url").and_then(|v| v.as_str());
        let follow_up_tasks = args.get("follow_up_tasks").and_then(|v| v.as_array());
        let release_summary = args.get("release_summary").and_then(|v| v.as_str());

        // Resolve task and validate follow_up_tasks
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let mut follow_up_ids = Vec::new();
        if let Some(arr) = follow_up_tasks {
            let mut unresolved = Vec::new();
            for v in arr {
                if let Some(id_str) = v.as_str() {
                    if graph.resolve(id_str).is_none() {
                        unresolved.push(id_str.to_string());
                    } else {
                        follow_up_ids.push(id_str.to_string());
                    }
                }
            }
            if !unresolved.is_empty() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Unresolved follow_up_tasks: {}",
                        unresolved.join(", ")
                    )),
                    data: None,
                });
            }
        }

        // Check task is not already terminal
        let current_status = node.status.as_deref().unwrap_or("inbox");
        if current_status == "done" || current_status == "cancelled" {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Task \"{id}\" is already \"{current_status}\". Cannot release a completed task.\n\
                     To update a completed task's fields, use update_task."
                )),
                data: None,
            });
        }

        // Reject closing a task with open children unless recursive=true.
        // Applies to any status that closes for hierarchy (done, cancelled, archived).
        let recursive_close_descs: Vec<(String, std::path::PathBuf)> =
            if crate::graph::is_closed_for_hierarchy(Some(status)) {
                let recursive = args
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let open_descs: Vec<(String, std::path::PathBuf)> = graph
                    .open_descendants(&node.id)
                    .into_iter()
                    .filter_map(|desc_id| {
                        graph
                            .get_node(&desc_id)
                            .map(|n| (desc_id, self.abs_path(&n.path)))
                    })
                    .collect();
                if !open_descs.is_empty() && !recursive {
                    let ids: Vec<&str> = open_descs.iter().map(|(id, _)| id.as_str()).collect();
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Cannot release '{}' as '{}': {} open child task(s) still exist: {}. \
                             Complete or cancel them first, or pass recursive=true to close all descendants.",
                            id, status, open_descs.len(), ids.join(", ")
                        )),
                        data: None,
                    });
                }
                if recursive {
                    open_descs
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

        // Failure-reason-mandatory gate
        // specs/enforcement/evidence-contract.md: releasing to a handback status that isn't a clean
        // success requires a non-empty `reason` (or `blocker`, for
        // `blocked`) — the "evidence or a stated failure reason" contract's
        // failure-path half. Success statuses already require `summary`
        // above, unchanged. Presence-only: no content inspection.
        if Self::FAILURE_HANDBACK_STATUSES.contains(&status)
        {
            let has_reason = reason.map_or(false, |r| !r.trim().is_empty());
            let has_blocker =
                status == "blocked" && blocker.map_or(false, |b| !b.trim().is_empty());
            if !has_reason && !has_blocker {
                let field_hint = if status == "blocked" {
                    "reason or blocker"
                } else {
                    "reason"
                };
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Releasing \"{id}\" as \"{status}\" requires a non-empty {field_hint}. Describe why before releasing.\n\
                         Example: release_task(id=\"{id}\", status=\"{status}\", summary=\"...\", reason=\"...\")"
                    )),
                    data: None,
                });
            }
        }

        let abs_path = self.abs_path(&node.path);
        let label = node.label.clone();
        let node_id = node.id.clone();
        drop(graph);

        // Cascade-close open descendants when recursive=true. Batch the
        // graph patches into a single write lock + one reclassify, and queue
        // re-embeds for the background worker, so 100 descendants don't cost
        // 100 sequential write-lock acquisitions and 100 reclassify passes.
        let t_total = std::time::Instant::now();

        if !recursive_close_descs.is_empty() {
            let desc_updates: std::collections::HashMap<String, serde_json::Value> = [(
                "status".to_string(),
                serde_json::Value::String(status.to_string()),
            )]
            .into_iter()
            .collect();

            // Phase 1: rewrite descendant files + parse them. Sequential — the
            // file writes are I/O bound and not worth rayon for typical
            // cascades (<100 docs). Errors fail loud instead of being silently skipped.
            let mut parsed_descs: Vec<crate::pkb::PkbDocument> = Vec::with_capacity(recursive_close_descs.len());
            for (desc_id, desc_path) in &recursive_close_descs {
                crate::document_crud::update_document(desc_path, desc_updates.clone())
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to recursively close child task '{desc_id}': {e}"
                        )),
                        data: None,
                    })?;
                if let Some(doc) = crate::pkb::parse_file_relative(desc_path, &self.pkb_root) {
                    parsed_descs.push(doc);
                }
            }

            if !parsed_descs.is_empty() {
                // Phase 2: single graph write lock — patch all nodes, then one
                // reclassify pass for the whole batch. Insert into
                // patched_during_rebuild inside the same lock to prevent a
                // concurrent Tier-2 swap from dropping these nodes.
                {
                    let mut g = self.graph.write();
                    let mut patched = self.patched_during_rebuild.lock();
                    for doc in &parsed_descs {
                        let node = crate::graph::GraphNode::from_pkb_document(doc);
                        patched.insert(node.id.clone());
                        g.upsert_node_in_place(node, &self.pkb_root);
                    }
                    g.reclassify();
                }

                // Phase 3: queue vector-store work. Each call patches
                // metadata synchronously and schedules background embed if
                // body changed (no body changes here — status is frontmatter
                // only — so this is just metadata patch + save dispatch).
                for doc in &parsed_descs {
                    self.try_upsert_document(doc);
                }

                // Phase 4: schedule a single background rebuild for the
                // whole batch (the per-node schedule calls are coalesced, but
                // an explicit one here is harmless and documents intent).
                self.schedule_graph_rebuild();
            }
        }

        // Build frontmatter updates
        let mut updates = std::collections::HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String(status.to_string()),
        );
        if let Some(url) = pr_url {
            updates.insert(
                "pr_url".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }
        if let Some(b) = branch {
            updates.insert(
                "branch".to_string(),
                serde_json::Value::String(b.to_string()),
            );
        }

        // session_id: arg or env $AOPS_SESSION_ID
        let final_session_id = session_id_arg
            .map(|s| s.to_string())
            .or_else(|| std::env::var("AOPS_SESSION_ID").ok());
        if let Some(sid) = final_session_id {
            updates.insert("session_id".to_string(), serde_json::Value::String(sid));
        }

        if let Some(url) = issue_url {
            updates.insert(
                "issue_url".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }

        if !follow_up_ids.is_empty() {
            updates.insert(
                "follow_up_tasks".to_string(),
                serde_json::Value::Array(
                    follow_up_ids
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }

        if let Some(rs) = release_summary {
            updates.insert(
                "release_summary".to_string(),
                serde_json::Value::String(rs.to_string()),
            );
        }

        // mem_8a8aeb2a: the evidence-or-failure-reason contract requires
        // `reason`/`blocker` on handback statuses, but until now the values
        // were only appended as prose into the release-evidence body block —
        // never persisted to frontmatter, so no query or read could find them.
        // Persist both as frontmatter fields, matching the convention already
        // used elsewhere in the corpus (e.g. a `reason:` field on cancelled
        // nodes).
        if let Some(r) = reason {
            if !r.trim().is_empty() {
                updates.insert(
                    "reason".to_string(),
                    serde_json::Value::String(r.trim().to_string()),
                );
            }
        }
        if let Some(b) = blocker {
            if !b.trim().is_empty() {
                updates.insert(
                    "blocker".to_string(),
                    serde_json::Value::String(b.trim().to_string()),
                );
            }
        }

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        updates.insert(
            "released_at".to_string(),
            serde_json::Value::String(now.clone()),
        );

        let t = std::time::Instant::now();
        crate::document_crud::update_document(&abs_path, updates).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        // Build and append release evidence block
        let mut evidence_block = format!(
            "\n\n## Release: {status}\n\n**{now}**\n\n{}",
            summary.trim()
        );
        if let Some(url) = pr_url {
            evidence_block.push_str(&format!("\n\nPR: {url}"));
        }
        if let Some(b) = branch {
            evidence_block.push_str(&format!("\nBranch: {b}"));
        }
        if let Some(blk) = blocker {
            if !blk.trim().is_empty() {
                evidence_block.push_str(&format!("\n\nBlocker: {}", blk.trim()));
            }
        }
        if let Some(r) = reason {
            if !r.trim().is_empty() {
                evidence_block.push_str(&format!("\n\nReason: {}", r.trim()));
            }
        }
        evidence_block.push('\n');

        crate::document_crud::append_to_document(&abs_path, &evidence_block, None, None).map_err(
            |e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to append release evidence: {e}")),
                data: None,
            },
        )?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::release_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::release_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::release_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::release_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::release_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::release_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        // Build response with soft warnings
        let mut warnings = Vec::new();
        if status == "merge_ready" && pr_url.is_none() {
            warnings.push("WARNING: No pr_url for merge_ready. Update the task with the PR URL when available.");
        }
        if status == "blocked" && blocker.map_or(true, |b| b.trim().is_empty()) {
            warnings.push("WARNING: No blocker description. Consider updating with what's blocking this task.");
        }
        if (status == "cancelled" || status == "review")
            && reason.map_or(true, |r| r.trim().is_empty())
        {
            warnings.push("WARNING: No reason provided. Future you will want to know why.");
        }
        if let Some(rs) = release_summary {
            if rs.len() > 500 {
                warnings.push("WARNING: release_summary exceeds 500 characters.");
            }
        }

        let mut response_text = format!("Released: {} → {} (`{}`)", label, status, id);
        for w in &warnings {
            response_text.push_str(&format!("\n{w}"));
        }

        // Compute the neighborhood against the post-write graph. `unblocked` is
        // gated on `!is_blocked`, so a release to a non-clearing status (e.g.
        // `blocked`) yields an empty `unblocked` — the task still blocks its deps.
        let cascade_closed = recursive_close_descs.len();
        let neighborhood = {
            let graph = self.graph.read();
            Self::build_mutation_neighborhood(&graph, &node_id, cascade_closed)
        };

        let mut json = serde_json::json!({
            "ok": true,
            "id": id,
            "status": status,
            "message": response_text,
            "neighborhood": neighborhood,
        });
        if let Some(nid) = created_id {
            json["created_id"] = serde_json::json!(nid);
        }
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        )]))
    }

    // =========================================================================
    // NEW TOOLS: Memory CRUD, decompose, dependency tree, children
    // =========================================================================

    pub(crate) fn handle_decompose_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let parent_id = args
            .get("parent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: parent_id"),
                data: None,
            })?;

        let subtasks = args
            .get("subtasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: subtasks (array)"),
                data: None,
            })?;

        if subtasks.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("subtasks array cannot be empty"),
                data: None,
            });
        }

        // task_1381381c: agents must not originate priority bands via decompose_task.
        if let Some(bad) = subtasks.iter().find(|st| st.get("priority").is_some()) {
            let title = bad.get("title").and_then(|v| v.as_str()).unwrap_or("<untitled>");
            return Err(Self::reject_agent_priority(&format!(
                "decompose_task (subtask '{title}')"
            )));
        }

        let (project_prefix, parent_project) = {
            let graph = self.graph.read();
            match graph.resolve(parent_id) {
                None => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!("Parent task not found: {parent_id}")),
                        data: None,
                    });
                }
                Some(node) => {
                    if node.task_id.is_none() {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Parent ID must refer to a task node, but `{parent_id}` is not a task"
                            )),
                            data: None,
                        });
                    }
                    // Targets carry a task_id (from the `id` frontmatter) but are not
                    // structural parents — reject decomposing work under them.
                    if let Err(msg) = graph.reject_target_as_parent(parent_id) {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(msg),
                            data: None,
                        });
                    }
                    let prefix = node.node_type.clone().unwrap_or_else(|| "task".to_string());
                    // Read parent's raw frontmatter `project` field so subtasks can inherit it.
                    // GraphNode.project is a computed ancestor label, not the frontmatter value.
                    let parent_project =
                        crate::pkb::parse_file_relative(&self.abs_path(&node.path), &self.pkb_root)
                            .and_then(|doc| doc.frontmatter)
                            .and_then(|fm| {
                                fm.get("project").and_then(|v| v.as_str()).map(String::from)
                            })
                            .or_else(|| node.project.clone());
                    (prefix, parent_project)
                }
            }
        };

        // parent_project is Option<String>; subtasks inherit it when present, otherwise
        // project is graph-derived at read time (compute_project_field via ancestor chain).

        // First pass: assign IDs to all subtasks and build title map for cross-references
        let mut subtask_ids: Vec<String> = Vec::with_capacity(subtasks.len());
        let mut title_to_id: HashMap<String, String> = HashMap::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        for subtask in subtasks {
            let title = subtask
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Each subtask must have a 'title'"),
                    data: None,
                })?;

            let title_lower = title.to_lowercase();
            if title_to_id.contains_key(&title_lower) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Duplicate subtask title: '{}'. Sibling titles must be unique for cross-referencing.",
                        title
                    )),
                    data: None,
                });
            }

            let id = subtask
                .get("id")
                .and_then(|v| v.as_str())
                .map(crate::document_crud::sanitize_explicit_id)
                .unwrap_or_else(|| crate::graph::create_id(&project_prefix));

            if !seen_ids.insert(id.clone()) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Duplicate subtask ID: '{}'. Subtask IDs must be unique within a batch.",
                        id
                    )),
                    data: None,
                });
            }

            subtask_ids.push(id.clone());
            title_to_id.insert(title_lower, id);
        }

        let mut unresolvable = Vec::new();
        {
            let graph = self.graph.read();
            for subtask in subtasks.iter() {
                if let Some(deps) = subtask.get("depends_on").and_then(|v| v.as_array()) {
                    for dep_val in deps {
                        if let Some(dep) = dep_val.as_str() {
                            if dep.starts_with('$') {
                                if let Ok(idx) = dep[1..].parse::<usize>() {
                                    if idx == 0 || idx > subtask_ids.len() {
                                        unresolvable.push(dep.to_string());
                                    }
                                } else {
                                    unresolvable.push(dep.to_string());
                                }
                            } else if !title_to_id.contains_key(&dep.to_lowercase()) {
                                if graph.resolve(dep).is_none() {
                                    unresolvable.push(dep.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        if !unresolvable.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: std::borrow::Cow::from(format!(
                    "Unresolvable sibling references in decompose_task: {:?}",
                    unresolvable
                )),
                data: None,
            });
        }

        let mut created: Vec<(String, String, PathBuf)> = Vec::new();
        static ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^([a-z0-9_]+_[0-9a-f]{8}|[a-z]+-[0-9a-f]{8})").expect("valid static regex")
        });

        let mut severity_warning = false;

        // Second pass: create tasks with resolved dependencies
        for (i, subtask) in subtasks.iter().enumerate() {
            let title = subtask.get("title").and_then(|v| v.as_str()).unwrap(); // validated in first pass

            let mut depends_on: Vec<String> = subtask
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Resolve sibling cross-references
            for dep in depends_on.iter_mut() {
                // 1. Positional references: $1, $2, etc. (1-indexed)
                if dep.starts_with('$') {
                    if let Ok(idx) = dep[1..].parse::<usize>() {
                        if idx > 0 && idx <= subtask_ids.len() {
                            *dep = subtask_ids[idx - 1].clone();
                        }
                    }
                }
                // 2. Title references (exact case-insensitive match on sibling title)
                else if let Some(sid) = title_to_id.get(&dep.to_lowercase()) {
                    *dep = sid.clone();
                }
            }

            let fields = crate::document_crud::TaskFields {
                title: title.to_string(),
                id: Some(subtask_ids[i].clone()),
                parent: Some(parent_id.to_string()),
                priority: subtask
                    .get("priority")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32),
                tags: subtask
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                depends_on,
                soft_depends_on: Vec::new(),
                assignee: subtask
                    .get("assignee")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                complexity: subtask
                    .get("complexity")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                body: subtask
                    .get("body")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                stakeholder: subtask
                    .get("stakeholder")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                waiting_since: subtask
                    .get("waiting_since")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                due: subtask
                    .get("due")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                effort: subtask
                    .get("effort")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                consequence: subtask
                    .get("consequence")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                project: subtask
                    .get("project")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| parent_project.clone()),
                task_type: subtask
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                status: subtask
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                severity: {
                    let is_target = subtask.get("type").and_then(|v| v.as_str()) == Some("target");
                    let mut sev = subtask
                        .get("severity")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                    if !is_target && sev.unwrap_or(0) != 0 {
                        sev = Some(0);
                        severity_warning = true;
                    }
                    sev
                },
                goal_type: subtask
                    .get("goal_type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                session_id: None,
                issue_url: None,
                follow_up_tasks: vec![],
                release_summary: None,
                contributes_to: vec![],
            };

            let path = crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| {
                McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to create subtask '{title}': {e}")),
                    data: None,
                }
            })?;

            let id_str = path
                .file_stem()
                .map(|s| {
                    let stem = s.to_string_lossy();
                    ID_RE
                        .find(&stem)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| stem.to_string())
                })
                .unwrap_or_default();
            let filename = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            created.push((id_str, filename, path));
        }

        let mut docs = Vec::new();
        let mut failed = false;
        for (_id_str, _filename, path) in &created {
            if let Some(doc) = crate::pkb::parse_file_relative(path, &self.pkb_root) {
                docs.push(doc);
            } else {
                tracing::warn!(
                    "Incremental parse failed for {:?}, doing full rebuild",
                    path
                );
                failed = true;
            }
        }

        if !docs.is_empty() {
            self.batch_upsert_and_rebuild(docs);
        }

        if failed {
            self.rebuild_graph();
        }

        let mut output = format!(
            "**Created {} subtasks under `{parent_id}`:**\n\n",
            created.len()
        );
        for (id_str, filename, _path) in &created {
            output.push_str(&format!("- `{id_str}` — `{filename}`\n"));
        }

        if severity_warning {
            output.push_str("\nWarning: severity ignored for one or more non-target nodes.\n");
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

}
