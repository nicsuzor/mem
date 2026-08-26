use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{Error as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::graph::is_completed;
use crate::graph_store::GraphStore;

use super::{PkbSearchServer, MAX_RESULTS, GOAL_TYPE_ENUM};

impl PkbSearchServer {
    pub(crate) fn handle_create_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        // Accept `title` (preferred) or `task_title` (alias — some skill docs use this name)
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("task_title").and_then(|v| v.as_str()))
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        if let Some(obj) = args.as_object() {
            const KNOWN_KEYS: &[&str] = &[
                "title",
                "task_title",
                "id",
                "parent",
                "priority",
                "tags",
                "depends_on",
                "assignee",
                "complexity",
                "effort",
                "consequence",
                "severity",
                "goal_type",
                "body",
                "stakeholder",
                "waiting_since",
                "due",
                "project",
                "type",
                "status",
                "session_id",
                "issue_url",
                "follow_up_tasks",
                "release_summary",
                "contributes_to",
                "allow_missing_parent",
                "force",
            ];
            let received: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
            let unknown: Vec<&str> = received
                .iter()
                .copied()
                .filter(|k| !KNOWN_KEYS.contains(k))
                .collect();
            tracing::info!(
                target: "pkb::create_task",
                title = %title,
                received_keys = ?received,
                unknown_keys = ?unknown,
                "create_task invocation"
            );
            if !unknown.is_empty() {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: std::borrow::Cow::from(format!(
                        "Unknown keys in create_task: {:?}. Pass them via update_task or extend the create_task schema.",
                        unknown
                    )),
                    data: None,
                });
            }
        }

        let mut severity_warning = false;
        let mut severity = args
            .get("severity")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let is_target = args.get("type").and_then(|v| v.as_str()) == Some("target");
        if !is_target && severity.unwrap_or(0) != 0 {
            severity = Some(0);
            severity_warning = true;
        }

        let fields = crate::document_crud::TaskFields {
            title: title.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            parent: args
                .get("parent")
                .and_then(|v| v.as_str())
                .map(String::from),
            priority: args
                .get("priority")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            depends_on: args
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            assignee: args
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(String::from),
            complexity: args
                .get("complexity")
                .and_then(|v| v.as_str())
                .map(String::from),
            effort: args
                .get("effort")
                .and_then(|v| v.as_str())
                .map(String::from),
            consequence: args
                .get("consequence")
                .and_then(|v| v.as_str())
                .map(String::from),
            severity,
            goal_type: args
                .get("goal_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            stakeholder: args
                .get("stakeholder")
                .and_then(|v| v.as_str())
                .map(String::from),
            waiting_since: args
                .get("waiting_since")
                .and_then(|v| v.as_str())
                .map(String::from),
            due: args.get("due").and_then(|v| v.as_str()).map(String::from),
            project,
            task_type: args.get("type").and_then(|v| v.as_str()).map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
            session_id: args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            issue_url: args
                .get("issue_url")
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
            release_summary: args
                .get("release_summary")
                .and_then(|v| v.as_str())
                .map(String::from),
            contributes_to: args
                .get("contributes_to")
                .and_then(|v| v.as_array())
                .map(|arr| arr.clone())
                .unwrap_or_default(),
        };

        // Hierarchy validation: actionable tasks must have a parent. Strategic
        // out-of-tree nodes (`goal`, `target`) and root-able types (`epic`,
        // `learn`) are exempt — goals & targets live beside the work tree and are
        // never parented (work links to them via `contributes_to`); epics may be
        // root-level containers per the PKB type taxonomy spec.
        let task_type_str = fields.task_type.as_deref().unwrap_or("task");
        let root_able = matches!(task_type_str, "goal" | "target" | "epic" | "learn");
        if fields.parent.is_none() && !root_able {
            // Semantic search for candidate parents so agents can immediately see options.
            let suggested_parents: Option<serde_json::Value> = if !fields.title.is_empty() {
                match self.embedder.encode_query(&fields.title) {
                    Ok(query_embedding) => {
                        let candidates = {
                            let store = self.store.read();
                            store.search(&query_embedding, 50, &self.pkb_root, None, None, None)
                        };
                        let mut suggestions: Vec<serde_json::Value> = Vec::new();
                        for r in candidates {
                            if suggestions.len() >= 5 {
                                break;
                            }
                            match r.doc_type.as_deref() {
                                Some("epic") => {
                                    suggestions.push(serde_json::json!({
                                        "id": r.id,
                                        "title": r.title,
                                        "type": r.doc_type,
                                        "score": r.score,
                                    }));
                                }
                                _ => {}
                            }
                        }
                        if suggestions.is_empty() {
                            None
                        } else {
                            Some(serde_json::json!({ "suggested_parents": suggestions }))
                        }
                    }
                    Err(_) => None,
                }
            } else {
                None
            };

            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Missing required parameter: parent. Tasks must have a parent node. \
                     Only goal, target, epic, and learn types can be root-level.",
                ),
                data: suggested_parents,
            });
        }

        // Validate parent exists and is open. Caller can override with flags:
        //   allow_missing_parent=true  — downgrade missing parent to a warning
        //   force=true                 — allow creating under a closed parent
        {
            let allow_missing = args
                .get("allow_missing_parent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let graph = self.graph.read();
            if let Some(ref parent_id) = fields.parent {
                match graph.resolve(parent_id) {
                    None => {
                        if allow_missing {
                            tracing::warn!(
                                "create_task: parent '{}' not found in PKB; proceeding because allow_missing_parent=true",
                                parent_id
                            );
                        } else {
                            return Err(McpError {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(format!(
                                    "Parent '{}' not found in PKB. Create the parent node first, fix the ID, or pass allow_missing_parent=true to override.",
                                    parent_id
                                )),
                                data: None,
                            });
                        }
                    }
                    Some(parent_node) => {
                        // Reject target/goal nodes as structural parents (targets are
                        // black holes — link via contributes_to, never parent).
                        if let Err(msg) = graph.reject_target_as_parent(parent_id) {
                            return Err(McpError {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(msg),
                                data: None,
                            });
                        }
                        // Reject closed parents unless caller opts in with force=true.
                        let parent_status = parent_node.status.as_deref().unwrap_or("");
                        if crate::graph::is_closed_for_hierarchy(parent_node.status.as_deref())
                            && !force
                        {
                            return Err(McpError {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(format!(
                                    "Parent '{}' is closed (status: {}). Cannot add tasks under a closed parent. \
                                     Pass force=true to override.",
                                    parent_id, parent_status
                                )),
                                data: None,
                            });
                        }

                        // Reject parent/child cycles. Only relevant when an explicit `id`
                        // is supplied (an auto-generated id cannot already be a parent).
                        if let Some(ref child_id) = fields.id {
                            if let Err(msg) = graph.would_create_parent_cycle(child_id, parent_id) {
                                return Err(McpError {
                                    code: ErrorCode::INVALID_PARAMS,
                                    message: Cow::from(msg),
                                    data: None,
                                });
                            }
                        }
                    }
                }
            }
        }


        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        let path =
            crate::document_crud::create_task(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create task: {e}")),
                data: None,
            })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::create_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::create_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        // Incremental graph update for the new file
        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::create_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::create_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::create_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::create_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        // Extract ID from filename stem (e.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4").
        // generate_id() always emits exactly 8 hex chars, so anchor the pattern to {8}
        // rather than `+` — keeps an accidental hex-shaped slug suffix from being absorbed.
        let task_id = path
            .file_stem()
            .map(|s| {
                let stem = s.to_string_lossy();
                static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                    regex::Regex::new(r"^([a-z0-9_]+_[0-9a-f]{8}|[a-z]+-[0-9a-f]{8})").unwrap()
                });
                RE.find(&stem)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| stem.to_string())
            })
            .unwrap_or_default();

        // Return structured JSON matching get_task shape. If the post-create graph
        // lookup fails (graph rebuild raced or the file landed in an un-scanned
        // location), fail loudly with the file path — silently returning null
        // frontmatter (the original 2026-04-30 regression mode) hides the bug.
        let get_args = serde_json::json!({ "id": task_id });
        let mut res = self.handle_get_task(&get_args).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "create_task wrote {} but the new node is not yet visible in the graph \
                 (id={task_id}). Underlying lookup error: {}. \
                 The file is on disk — retry get_task in a moment.",
                path.display(),
                e.message,
            )),
            data: None,
        })?;
        if severity_warning {
            res.content
                .push(Content::text("severity ignored: not a target node\n"));
        }
        Ok(res)
    }

    /// Claim a task node.
    ///
    /// Routes to one of two strategies based on the resolved node's type:
    ///   - `type: template` -> instantiate a datestamped copy (unchanged
    ///     behavior; see `claim_template_instance` below).
    ///   - `type: task` (or untyped, which this codebase's own conventions
    ///     treat as `task` — see the `unwrap_or_else(|| "task".to_string())`
    ///     prefix computation elsewhere in this file) -> claim IN PLACE: set
    ///     `status: in_progress` (+ optional `assignee`/`session_id`), no new
    ///     node created.
    ///   - anything else (epic, project, goal, target, ...) -> reject.
    ///
    /// The in-place path restores pre-regression behavior: `claim_task` used
    /// to work on regular task nodes before the template feature was added
    /// as a special case; a later change tightened the type gate to reject
    /// *all* non-template nodes instead of falling through to the original
    /// in-place claim. See `mem_e8f3aa17` / `task_9d568b1b`.
    pub(crate) fn handle_get_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        // Resolve ID to graph node (supports exact ID, filename stem, title)
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path(&node.path);

        if !abs_path.exists() {
            return Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Task file not found on disk: {}",
                    abs_path.display()
                )),
                data: None,
            });
        }

        let content = std::fs::read_to_string(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read task file: {e}")),
            data: None,
        })?;

        // Parse YAML frontmatter
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter.parse(&content);

        let mut frontmatter = parsed
            .data
            .as_ref()
            .and_then(|d| d.deserialize::<JsonValue>().ok())
            .unwrap_or(JsonValue::Object(serde_json::Map::new()));

        if let Some(obj) = frontmatter.as_object_mut() {
            if !obj.contains_key("project") {
                if let Some(ref proj) = node.project {
                    obj.insert("project".to_string(), serde_json::json!(proj));
                }
            }
        }

        let body = parsed.content.trim().to_string();

        // Build relationship context from graph
        let resolve_ref = |rid: &str| -> serde_json::Value {
            if let Some(n) = graph.get_node(rid) {
                serde_json::json!({
                    "id": rid,
                    "title": n.label,
                    "status": n.status,
                })
            } else {
                serde_json::json!({ "id": rid })
            }
        };

        let depends_on: Vec<serde_json::Value> =
            node.depends_on.iter().map(|d| resolve_ref(d)).collect();
        let blocks: Vec<serde_json::Value> = node.blocks.iter().map(|b| resolve_ref(b)).collect();
        // Children carry the claiming session/agent identity too (D1: display-only,
        // soft — reviewer-vs-executor separation visible on reads, never enforced).
        // This covers the common decompose_task case (child node_type "task"),
        // distinct from the `node.subtasks` list below (node_type "subtask").
        let children: Vec<serde_json::Value> = node
            .children
            .iter()
            .map(|c| {
                if let Some(n) = graph.get_node(c) {
                    serde_json::json!({
                        "id": c,
                        "title": n.label,
                        "status": n.status,
                        "assignee": n.assignee,
                    })
                } else {
                    serde_json::json!({ "id": c })
                }
            })
            .collect();
        let closes: Vec<serde_json::Value> = node.closes.iter().map(|c| resolve_ref(c)).collect();
        let parent = node.parent.as_ref().map(|p| resolve_ref(p));

        // Build subtask list and inject a checklist section into the body
        let subtask_nodes: Vec<serde_json::Value> = node
            .subtasks
            .iter()
            .map(|sid| {
                if let Some(n) = graph.get_node(sid) {
                    serde_json::json!({
                        "id": sid,
                        "title": n.label,
                        "status": n.status,
                        // Display-only session/agent identity (D1: soft, non-blocking —
                        // reviewer-vs-executor separation visible on reads, never enforced).
                        "assignee": n.assignee,
                    })
                } else {
                    serde_json::json!({ "id": sid })
                }
            })
            .collect();

        // Sort subtasks by numeric suffix so they appear in order
        let mut subtask_nodes_sorted = subtask_nodes;
        subtask_nodes_sorted.sort_by(|a, b| {
            let parse_n = |v: &serde_json::Value| -> u32 {
                v.get("id")
                    .and_then(|id| id.as_str())
                    .and_then(|id| id.rsplit('.').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            parse_n(a).cmp(&parse_n(b))
        });

        let body = if !subtask_nodes_sorted.is_empty() {
            let mut checklist = String::from("\n\n## Subtasks\n\n");
            for st in &subtask_nodes_sorted {
                let done = crate::graph::is_completed(st.get("status").and_then(|s| s.as_str()));
                let check = if done { "x" } else { " " };
                let title = st
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("<untitled>");
                let id = st.get("id").and_then(|i| i.as_str()).unwrap_or("");
                checklist.push_str(&format!("- [{check}] **{id}**: {title}\n"));
            }
            format!("{body}{checklist}")
        } else {
            body
        };

        let max_bytes = args
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let body = Self::truncate_body(body, max_bytes);

        // Compute deadline metadata
        let today = chrono::Utc::now().date_naive();
        let days_until_due: Option<i64> = node.due.as_deref().and_then(|due| {
            let len = std::cmp::min(10, due.len());
            chrono::NaiveDate::parse_from_str(&due[..due.floor_char_boundary(len)], "%Y-%m-%d")
                .ok()
                .map(|due_date| (due_date - today).num_days())
        });
        let effort_days = node
            .effort
            .as_deref()
            .and_then(crate::graph::parse_effort_days)
            .unwrap_or(3);
        let urgency_ratio: Option<f64> =
            days_until_due.map(|d| (effort_days as f64 / d.max(1) as f64).min(1.0));

        let contributes_to: Vec<serde_json::Value> = node
            .contributes_to
            .iter()
            .map(|c| {
                let mut res = serde_json::to_value(c).unwrap_or_else(|_| serde_json::json!({}));
                let target_id = c.resolved_to.as_deref().unwrap_or(&c.to);
                if let Some(n) = graph.resolve(target_id) {
                    if let Some(obj) = res.as_object_mut() {
                        obj.insert("title".to_string(), serde_json::json!(n.label));
                        if let Some(ref s) = n.status {
                            obj.insert("status".to_string(), serde_json::json!(s));
                        }
                    }
                }
                res
            })
            .collect();

        let result = serde_json::json!({
            "id": node.task_id.as_deref().unwrap_or(&node.id),
            "status": node.status,
            "project": node.project,
            "frontmatter": frontmatter,
            "body": body,
            "depends_on": depends_on,
            "blocks": blocks,
            "children": children,
            "subtasks": subtask_nodes_sorted,
            "parent": parent,
            "closes": closes,
            "target_ancestors": node.target_ancestors,
            "goals": node.goals,
            "contributes_to": contributes_to,
            "contributed_by": node.contributed_by,
            "follow_up_tasks": node.follow_up_tasks,
            "priority": node.priority.unwrap_or(4),
            "effective_priority": node.effective_priority.unwrap_or(node.priority.unwrap_or(4)),
            "focus_score": node.focus_score,
            "blocked": graph.is_blocked(&node.id),
            "signals": {
                "criticality": node.criticality,
                "urgency": node.urgency,
                "downstream_weight": node.downstream_weight,
                "scope": node.scope,
                "uncertainty": node.uncertainty,
                "voi_value": node.voi_value,
            },
            "stakeholder_exposure": node.stakeholder_exposure,
            "stakeholder": node.stakeholder,
            "waiting_since": node.waiting_since,
            "due": node.due,
            "effort": node.effort,
            "consequence": node.consequence,
            "severity": node.severity,
            "goal_type": node.goal_type,
            "edge_template": node.edge_template,
            "parse_warnings": node.parse_warnings,
            "days_until_due": days_until_due,
            "urgency_ratio": urgency_ratio,
        });

        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // =========================================================================
    // MEMORY + DELETE + COMPLETE TOOLS (4)
    // =========================================================================

    pub(crate) fn handle_get_dependency_tree(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("upstream");
        // Bounds traversal depth so a densely-linked graph can't return an
        // unbounded tree. MAX_RESULTS is not the right cap here (this bounds
        // hops, not row count) so it just gets a sane ceiling of its own.
        let max_depth = (args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(1000);

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;
        let node_id = node.id.clone();
        let node_label = node.label.clone();

        let tree = if direction.eq_ignore_ascii_case("downstream") {
            graph.blocks_tree_bounded(&node_id, max_depth)
        } else {
            graph.dependency_tree_bounded(&node_id, max_depth)
        };

        if tree.is_empty() {
            let dir_label = if direction.eq_ignore_ascii_case("downstream") {
                "downstream"
            } else {
                "upstream"
            };
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No {dir_label} dependencies for `{id}` ({node_label})."
            ))]));
        }

        let dir_label = if direction.eq_ignore_ascii_case("downstream") {
            "Downstream (blocks)"
        } else {
            "Upstream (depends on)"
        };
        let mut output = format!("## {dir_label} tree for `{id}` ({node_label})\n\n");

        for (dep_id, depth) in &tree {
            let indent = "  ".repeat(*depth);
            let label = graph
                .get_node(dep_id)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            let status = graph
                .get_node(dep_id)
                .and_then(|n| n.status.as_deref())
                .unwrap_or("?");
            output.push_str(&format!("{indent}- `{dep_id}` [{status}] {label}\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_get_task_children(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Caps how many child rows get *printed* when recursive=true (a deep
        // epic subtree could otherwise dump thousands of rows). The
        // completion summary (`total`/`done_count`) still walks the whole
        // subtree so the counts stay accurate even when printing is capped.
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize).min(MAX_RESULTS);

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {id}")),
            data: None,
        })?;
        let node_id = node.id.clone();
        let node_label = node.label.clone();

        if node.children.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No children for `{id}` ({node_label})."
            ))]));
        }

        let mut output = format!("## Children of `{id}` ({node_label})\n\n");

        fn collect_children(
            graph: &GraphStore,
            parent_id: &str,
            recursive: bool,
            depth: usize,
            limit: usize,
            output: &mut String,
            total: &mut usize,
            done: &mut usize,
            printed: &mut usize,
        ) {
            if let Some(node) = graph.get_node(parent_id) {
                for child_id in &node.children {
                    if let Some(child) = graph.get_node(child_id) {
                        *total += 1;
                        let is_done = is_completed(child.status.as_deref());
                        if is_done {
                            *done += 1;
                        }
                        if *printed < limit {
                            let indent = "  ".repeat(depth);
                            let status = child.status.as_deref().unwrap_or("-");
                            let pri = child.priority.map(|p| format!("P{p} ")).unwrap_or_default();
                            let cid = child.task_id.as_deref().unwrap_or(&child.id);
                            // Display-only session/agent identity (D1: soft, non-blocking —
                            // reviewer-vs-executor separation visible on reads, never enforced).
                            let who = child
                                .assignee
                                .as_deref()
                                .map(|a| format!(" (@{a})"))
                                .unwrap_or_default();
                            output.push_str(&format!(
                                "{indent}- `{cid}` [{status}] {pri}{}{who}\n",
                                child.label
                            ));
                            *printed += 1;
                        }
                        if recursive && !child.children.is_empty() {
                            collect_children(
                                graph,
                                &child.id,
                                recursive,
                                depth + 1,
                                limit,
                                output,
                                total,
                                done,
                                printed,
                            );
                        }
                    }
                }
            }
        }

        let mut total = 0usize;
        let mut done_count = 0usize;
        let mut printed = 0usize;
        collect_children(
            &graph,
            &node_id,
            recursive,
            0,
            limit,
            &mut output,
            &mut total,
            &mut done_count,
            &mut printed,
        );

        if total > printed {
            output.push_str(&format!(
                "\n…[truncated, showing {printed} of {total} children — pass a higher `limit` to see more]\n"
            ));
        }
        output.push_str(&format!("\n**Summary:** {done_count}/{total} complete\n"));

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_list_tasks(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let status = args.get("status").and_then(|v| v.as_str());
        let priority = args
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let severity = args
            .get("severity")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let goal_type = args.get("goal_type").and_then(|v| v.as_str());
        let assignee = args.get("assignee").and_then(|v| v.as_str());
        let doc_type = args.get("type").and_then(|v| v.as_str());
        let title_contains = args.get("title_contains").and_then(|v| v.as_str());
        let complexity = args.get("complexity").and_then(|v| v.as_str());
        let weight_gte = args.get("weight_gte").and_then(|v| v.as_i64());
        let project = args.get("project").and_then(|v| v.as_str());
        let focus_score_gte = args.get("focus_score_gte").and_then(|v| v.as_i64());
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize).min(MAX_RESULTS);
        let since = args.get("since").and_then(|v| v.as_str());
        let before = args.get("before").and_then(|v| v.as_str());
        let include_subtasks = args
            .get("include_subtasks")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // By default hide done/cancelled tasks — callers looking for work don't
        // want completed items cluttering the list. Matches task_search default.
        // Pass `include_done=true` to opt back in. When an explicit `status`
        // filter is provided the caller already knows what they want, so this
        // gate is skipped (e.g. status="done" still works).
        let include_done = args
            .get("include_done")
            .and_then(|v| v.as_bool())
            .or_else(|| args.get("include_closed").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let graph = self.graph.read();

        // Detect special status filters: "ready" and "blocked"
        let is_ready = status
            .map(|s| s.eq_ignore_ascii_case("ready"))
            .unwrap_or(false);
        let is_blocked = status
            .map(|s| s.eq_ignore_ascii_case("blocked"))
            .unwrap_or(false);

        // Hide terminal tasks by default unless explicitly requested via status or include_done
        let is_explicit_closed_status = status
            .map(|s| {
                let canonical = crate::graph::resolve_status_alias(s);
                canonical == "done" || canonical == "cancelled"
            })
            .unwrap_or(false);
        let query_include_done = include_done || is_explicit_closed_status;

        let base_nodes: Vec<&crate::graph::GraphNode> = if doc_type.is_some() {
            graph.all_nodes_with_id(query_include_done)
        } else {
            graph.actionable_tasks(query_include_done)
        };

        let mut tasks: Vec<_> = if is_ready {
            // Use graph.ready_tasks() for the ready filter
            let ready_ids: std::collections::HashSet<String> =
                graph.ready_tasks().iter().map(|n| n.id.clone()).collect();
            base_nodes
                .into_iter()
                .filter(|t| ready_ids.contains(&t.id))
                .collect()
        } else if is_blocked {
            // Use graph.blocked_tasks() for the blocked filter
            let blocked_ids: std::collections::HashSet<String> =
                graph.blocked_tasks().iter().map(|n| n.id.clone()).collect();
            base_nodes
                .into_iter()
                .filter(|t| blocked_ids.contains(&t.id))
                .collect()
        } else {
            let mut all: Vec<_> = base_nodes.into_iter().collect();
            if let Some(s) = status {
                let s_canonical = crate::graph::resolve_status_alias(s);
                all.retain(|t| {
                    t.status
                        .as_deref()
                        .map(|st| st.eq_ignore_ascii_case(s_canonical))
                        .unwrap_or(false)
                });
            }
            all
        };

        if !include_done && !is_explicit_closed_status {
            tasks.retain(|t| {
                t.status
                    .as_deref()
                    .map(|s| {
                        let canonical = crate::graph::resolve_status_alias(s);
                        canonical != "done" && canonical != "cancelled"
                    })
                    .unwrap_or(true)
            });
        }

        if let Some(pri) = priority {
            tasks.retain(|t| t.effective_priority.unwrap_or(4) <= pri);
        }
        if let Some(sev) = severity {
            tasks.retain(|t| t.severity == Some(sev));
        }
        if let Some(gt) = goal_type {
            tasks.retain(|t| {
                t.goal_type
                    .as_deref()
                    .map(|g| g.eq_ignore_ascii_case(gt))
                    .unwrap_or(false)
            });
        }
        if let Some(a) = assignee {
            tasks.retain(|t| {
                t.assignee
                    .as_deref()
                    .map(|ag| ag.eq_ignore_ascii_case(a))
                    .unwrap_or(false)
            });
        }
        if let Some(p) = project {
            // Resolve the filter value through the polecat.yaml registry so an
            // alias (e.g. `ao`) matches tasks stored with the canonical slug.
            // Deliberate read-path tradeoff: a missing OR malformed registry
            // degrades to literal matching (`.ok().flatten()`) rather than
            // failing the whole list_tasks call — searches should stay usable
            // when the registry is broken; the write paths stay fail-fast.
            let resolved_slug = crate::polecat_config::PolecatRegistry::load(&self.pkb_root)
                .ok()
                .flatten()
                .and_then(|reg| reg.resolve(p));

            tasks.retain(|t| {
                t.project
                    .as_deref()
                    .map(|pr| {
                        if let Some(ref slug) = resolved_slug {
                            pr.eq_ignore_ascii_case(slug) || pr.eq_ignore_ascii_case(p)
                        } else {
                            pr.eq_ignore_ascii_case(p)
                        }
                    })
                    .unwrap_or(false)
            });
        }
        if let Some(dt) = doc_type {
            tasks.retain(|t| {
                t.node_type
                    .as_deref()
                    .map(|d| d.eq_ignore_ascii_case(dt))
                    .unwrap_or(false)
            });
        }
        if let Some(needle) = title_contains {
            let needle_lower = needle.to_lowercase();
            tasks.retain(|t| t.label.to_lowercase().contains(&needle_lower));
        }
        if let Some(c) = complexity {
            tasks.retain(|t| {
                t.complexity
                    .as_deref()
                    .map(|cx| cx.eq_ignore_ascii_case(c))
                    .unwrap_or(false)
            });
        }
        if let Some(w_gte) = weight_gte {
            tasks.retain(|t| t.downstream_weight >= w_gte as f64);
        }
        if !tags.is_empty() {
            tasks.retain(|t| {
                tags.iter()
                    .all(|want| t.tags.iter().any(|have| have.eq_ignore_ascii_case(want)))
            });
        }

        if let Some(min_score) = focus_score_gte {
            tasks.retain(|t| t.focus_score.unwrap_or(0) >= min_score);
        }

        // Date filters on modified. RFC3339 timestamps start with YYYY-MM-DD so
        // the first 10 chars compare correctly against YYYY-MM-DD filter params.
        // Tasks with no modified date are excluded when any date filter is active.
        if let Some(s) = since {
            tasks.retain(|t| {
                t.modified
                    .as_deref()
                    .map(|m| &m[..m.floor_char_boundary(10)] >= s)
                    .unwrap_or(false)
            });
        }
        if let Some(b) = before {
            tasks.retain(|t| {
                t.modified
                    .as_deref()
                    .map(|m| &m[..m.floor_char_boundary(10)] <= b)
                    .unwrap_or(false)
            });
        }

        let explicitly_wants_target = doc_type
            .map(|dt| dt.eq_ignore_ascii_case("target"))
            .unwrap_or(false);

        if !explicitly_wants_target {
            tasks.retain(|t| {
                !t.node_type
                    .as_deref()
                    .map_or(false, |s| s.eq_ignore_ascii_case("target"))
            });
        }

        if !include_subtasks {
            tasks.retain(|t| t.node_type.as_deref() != Some("subtask"));
        }

        if tasks.is_empty() {
            let label = if is_ready {
                "No ready tasks found."
            } else if is_blocked {
                "No blocked tasks."
            } else {
                "No tasks found matching filters."
            };
            return Ok(CallToolResult::success(vec![Content::text(label)]));
        }

        // Default ordering (AC1/AC6): focus_score DESC with a deterministic
        // tie-break. `list_tasks` exposes no explicit sort argument, so this is
        // always the default projection ordering. Applied before truncation so
        // the top-`limit` rows are the highest-focus tasks, and applied to every
        // path (default / ready / blocked / filtered) since they all funnel into
        // `tasks`. Shares the same comparator as the CLI `list` default for parity.
        GraphStore::sort_by_focus(&mut tasks);

        let total = tasks.len();
        tasks.truncate(limit);

        // JSON output mode
        if format.eq_ignore_ascii_case("json") {
            let json_tasks: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.task_id.as_deref().unwrap_or(&t.id),
                        "title": t.label,
                        "status": t.status.as_deref().unwrap_or("unknown"),
                        "priority": t.priority.unwrap_or(4),
                        "effective_priority": t.effective_priority.unwrap_or(t.priority.unwrap_or(4)),
                        "focus_score": t.focus_score,
                        "blocked": graph.is_blocked(&t.id),
                        "signals": {
                            "criticality": t.criticality,
                            "urgency": t.urgency,
                            "downstream_weight": t.downstream_weight,
                            "scope": t.scope,
                            "uncertainty": t.uncertainty,
                            "voi_value": t.voi_value,
                        },
                        "project": t.project,
                        "assignee": t.assignee,
                        "modified": t.modified,
                        "tags": t.tags,
                        "parent": t.parent,
                        "depends_on": t.depends_on,
                        "node_type": t.node_type,
                        "due": t.due,
                        "effort": t.effort,
                        "consequence": t.consequence,
                        "severity": t.severity,
                        "goal_type": t.goal_type,
                        "edge_template": t.edge_template,
                    })
                })
                .collect();
            let result = serde_json::json!({
                "total": total,
                "showing": tasks.len(),
                "tasks": json_tasks,
            });
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&result).unwrap_or_default(),
            )]));
        }

        let output = if is_blocked {
            // Blocked view: heading per task with blocker details
            let mut out = format!("**{total} blocked tasks** (showing {})\n\n", tasks.len());
            for t in &tasks {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                out.push_str(&format!("### {} — {}\n", id, t.label));
                out.push_str(&format!(
                    "**Status:** {}\n",
                    t.status.as_deref().unwrap_or("-")
                ));
                if !t.depends_on.is_empty() {
                    out.push_str("**Blocked by:**\n");
                    for dep in &t.depends_on {
                        let dep_label =
                            graph.get_node(dep).map(|n| n.label.as_str()).unwrap_or("?");
                        let dep_status = graph
                            .get_node(dep)
                            .and_then(|n| n.status.as_deref())
                            .unwrap_or("?");
                        out.push_str(&format!("- `{}` [{}] {}\n", dep, dep_status, dep_label));
                    }
                }
                if t.status.as_deref() == Some("blocked") {
                    out.push_str("**Note:** explicitly blocked\n");
                }
                out.push('\n');
            }
            out
        } else if is_ready {
            // Ready view: table with Weight column, sorted by focus_score (default).
            let mut out = format!(
                "**{total} ready tasks** (showing {}, sorted by focus_score)\n\n",
                tasks.len()
            );
            let today = chrono::Utc::now().date_naive();
            out.push_str("| # | ID | Status | Pri | Weight | Crit | Urg | Due | Title |\n|---|---|---|---|---|---|---|---|---|\n");
            for (i, t) in tasks.iter().enumerate() {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                let weight = if t.downstream_weight > 0.0 {
                    format!(
                        "{:.1}{}",
                        t.downstream_weight,
                        if t.stakeholder_exposure { "!" } else { "" }
                    )
                } else {
                    "-".to_string()
                };
                let crit = if t.criticality > 0.0 {
                    format!("{:.2}", t.criticality)
                } else {
                    "-".to_string()
                };
                let urg = if t.urgency > 0.0 {
                    if t.urgency >= 10000.0 {
                        "SEV4".to_string()
                    } else if t.urgency >= 100.0 {
                        format!("{:.0}", t.urgency)
                    } else {
                        format!("{:.1}", t.urgency)
                    }
                } else {
                    "-".to_string()
                };
                let due_str = t
                    .due
                    .as_deref()
                    .map(|due| {
                        let len = std::cmp::min(10, due.len());
                        chrono::NaiveDate::parse_from_str(
                            &due[..due.floor_char_boundary(len)],
                            "%Y-%m-%d",
                        )
                        .ok()
                        .map(|due_date| {
                            let d = (due_date - today).num_days();
                            if d < 0 {
                                format!("{}d overdue", -d)
                            } else if d == 0 {
                                "today".to_string()
                            } else {
                                format!("{}d", d)
                            }
                        })
                        .unwrap_or_else(|| due[..due.floor_char_boundary(len)].to_string())
                    })
                    .unwrap_or_else(|| "-".to_string());
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    i + 1,
                    id,
                    t.status.as_deref().unwrap_or("-"),
                    t.priority.unwrap_or(4),
                    weight,
                    crit,
                    urg,
                    due_str,
                    t.label
                ));
            }
            out
        } else {
            // Default view: standard table
            let mut out = format!(
                "**{total} tasks** (showing {})\n\n| # | ID | Pri | Status | Title |\n|---|---|---|---|---|\n",
                tasks.len()
            );
            for (i, t) in tasks.iter().enumerate() {
                let id = t.task_id.as_deref().unwrap_or(&t.id);
                let pri = t.priority.unwrap_or(4);
                let status_str = t.status.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    i + 1,
                    id,
                    pri,
                    status_str,
                    t.label
                ));
            }
            out
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_update_task(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        if args.get("path").is_some() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "The 'path' parameter is no longer supported. Please use 'id' instead.",
                ),
                data: None,
            });
        }

        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let (path, canonical_id) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            (self.abs_path(&node.path), node.id.clone())
        };

        // Accept two forms for convenience:
        //   1. Nested: {"id": "...", "updates": {"status": "done"}}
        //   2. Flat:   {"id": "...", "status": "done"}
        // If `updates` is present, it wins. Otherwise collect top-level fields.
        const ROUTING_KEYS: &[&str] = &[
            "id",
            "updates",
            "recursive",
            "unparent",
            "force",
            "allow_missing_parent",
        ];
        let mut updates: serde_json::Map<String, serde_json::Value> =
            if let Some(nested) = args.get("updates").and_then(|v| v.as_object()) {
                nested.clone()
            } else {
                args.as_object()
                    .map(|obj| {
                        obj.iter()
                            .filter(|(k, _)| !ROUTING_KEYS.contains(&k.as_str()))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default()
            };

        // Bug 2 (task-d802855c): `unparent` is a routing directive, never a
        // frontmatter field. Accept it from EITHER the top-level args or the
        // nested `updates` object, strip it so it can never be written to the
        // document, and translate it into a `parent: null` removal. Previously:
        //   - top-level `unparent=true` was filtered out by ROUTING_KEYS, so
        //     `updates` was empty -> "No fields to update".
        //   - nested `{unparent: true}` fell through to the writer and persisted
        //     a junk `unparent: true` key while leaving `parent` in place.
        let unparent_flag = args
            .get("unparent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || updates
                .get("unparent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        updates.remove("unparent");
        if unparent_flag {
            updates.insert("parent".to_string(), serde_json::Value::Null);
        }

        if updates.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "No fields to update. Pass fields either nested as `updates: {status: \"done\"}` or flat as top-level params (e.g. status=\"done\").",
                ),
                data: None,
            });
        }

        // Validate + canonicalize a project change against polecat.yaml before
        // it reaches disk. `null` is an explicit "remove project" and passes
        // through untouched.
        if let Some(v) = updates.get("project") {
            if let Some(raw) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                let canonical = crate::polecat_config::resolve_project(&self.pkb_root, raw)
                    .map_err(|e| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!("{e:#}")),
                        data: None,
                    })?;
                updates.insert("project".to_string(), serde_json::Value::String(canonical));
            }
        }

        // Validate parent change: reject if the new parent does not exist
        // (task-89b2af87) AND reject if the change would create a cycle.
        if let Some(new_parent_val) = updates.get("parent") {
            // Treat null / empty string as "clear parent"
            let new_parent = new_parent_val.as_str().unwrap_or("").trim();
            if new_parent.is_empty() {
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                if !unparent_flag && !force {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: std::borrow::Cow::from(
                            "To remove a task's parent, pass unparent=true explicitly.",
                        ),
                        data: None,
                    });
                }
            } else {
                let allow_missing = args
                    .get("allow_missing_parent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let graph = self.graph.read();
                // 1. Existence check (task-89b2af87)
                if graph.resolve(new_parent).is_none() {
                    if allow_missing {
                        tracing::warn!(
                            "update_task: parent '{}' not found in PKB; proceeding because allow_missing_parent=true",
                            new_parent
                        );
                    } else {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Parent '{}' not found in PKB. Create the parent first, fix the ID, or pass allow_missing_parent=true.",
                                new_parent
                            )),
                            data: None,
                        });
                    }
                }
                // 2. Reject target/goal nodes as structural parents.
                if let Err(msg) = graph.reject_target_as_parent(new_parent) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
                // 3. Cycle check (pre-existing)
                if let Err(msg) = graph.would_create_parent_cycle(id, new_parent) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
            }
        }

        // Reject closing a task that still has open children, unless recursive=true.
        // Applies when status is being set to done, cancelled, or archived.
        let new_status = updates.get("status").and_then(|v| v.as_str());
        if let Some(ns) = new_status {
            if crate::graph::is_closed_for_hierarchy(Some(ns)) {
                let recursive = args
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let open_descs: Vec<(String, std::path::PathBuf)> = {
                    let graph = self.graph.read();
                    graph
                        .open_descendants(&canonical_id)
                        .into_iter()
                        .filter_map(|desc_id| {
                            graph
                                .get_node(&desc_id)
                                .map(|n| (desc_id, self.abs_path(&n.path)))
                        })
                        .collect()
                };
                if !open_descs.is_empty() && !recursive {
                    let ids: Vec<&str> = open_descs.iter().map(|(id, _)| id.as_str()).collect();
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Cannot set '{}' to '{}': {} open child task(s) still exist: {}. \
                             Complete or cancel them first, or pass recursive=true to close all descendants.",
                            id, ns, open_descs.len(), ids.join(", ")
                        )),
                        data: None,
                    });
                }
                if recursive && !open_descs.is_empty() {
                    let mut desc_updates = std::collections::HashMap::new();
                    desc_updates.insert(
                        "status".to_string(),
                        serde_json::Value::String(ns.to_string()),
                    );
                    for (_, desc_path) in &open_descs {
                        if let Err(e) =
                            crate::document_crud::update_document(desc_path, desc_updates.clone())
                        {
                            tracing::warn!(
                                "recursive close: failed to update {:?}: {}",
                                desc_path,
                                e
                            );
                        } else if let Some(doc) =
                            crate::pkb::parse_file_relative(desc_path, &self.pkb_root)
                        {
                            self.rebuild_graph_for_pkb_document(&doc);
                            self.try_upsert_document(&doc);
                        }
                    }
                }
            }
        }

        // When setting status to "done", require completion_evidence
        let setting_done = updates
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("done"))
            .unwrap_or(false);

        if setting_done {
            let evidence = updates
                .get("completion_evidence")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !evidence.chars().any(|c| !c.is_whitespace()) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "completion_evidence is required when setting status to done. Describe what was done before completing this task.",
                    ),
                    data: None,
                });
            }
        }

        // Build update map, filtering out completion_evidence (stored in body, not frontmatter).
        // The "body" key is intentionally passed through: update_document() routes it to the
        // markdown body section instead of frontmatter (see FRONTMATTER_EXCLUDED_KEYS).
        let evidence_text = updates
            .get("completion_evidence")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let pr_url_text = updates
            .get("pr_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let update_map = {
            let graph = self.graph.read();
            let node = graph.resolve(&canonical_id).ok_or_else(|| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: std::borrow::Cow::from(format!(
                    "Failed to resolve canonical id: {canonical_id}"
                )),
                data: None,
            })?;
            let mut filtered_updates = serde_json::Map::new();
            for (k, v) in &updates {
                if k != "completion_evidence" {
                    filtered_updates.insert(k.clone(), v.clone());
                }
            }
            crate::document_crud::expand_special_update_keys(&node, &filtered_updates).map_err(
                |e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: std::borrow::Cow::from(format!("Invalid updates: {e}")),
                    data: None,
                },
            )?
        };

        // Per-phase timing for write-path perf investigation (task-a4dcc039).
        // Emitted at debug; set RUST_LOG=mem::mcp_server=debug to observe.
        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        crate::document_crud::update_document(&path, update_map).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::update_task", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        // Append completion evidence to body when completing via update_task
        if let Some(evidence) = evidence_text {
            if setting_done && !evidence.trim().is_empty() {
                Self::append_evidence(&path, &evidence, pr_url_text.as_deref())?;
            }
        }

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::update_task", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::update_task", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::update_task", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::update_task", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::update_task", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        // Soft warning if setting a terminal status via update_task instead of release_task
        let terminal_statuses = [
            "merge_ready",
            "done",
            "review",
            "blocked",
            "cancelled",
            "partial",
        ];
        let hint = updates
            .get("status")
            .and_then(|v| v.as_str())
            .filter(|s| terminal_statuses.contains(s))
            .map(|s| {
                format!(
                "\nHINT: Use release_task(id=\"...\", status=\"{s}\", summary=\"...\") instead of \
                 update_task for status transitions. release_task captures work history."
            )
            })
            .unwrap_or_default();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Task updated: `{id}`{hint}"
        ))]))
    }

    // =========================================================================
    // BATCH OPERATIONS
    // =========================================================================

    pub(crate) fn handle_task_summary(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let graph = self.graph.read();
        let ready = graph.ready_tasks();
        let blocked = graph.blocked_tasks();
        let all_tasks = graph.all_tasks();

        let mut by_priority: std::collections::HashMap<i32, usize> =
            std::collections::HashMap::new();
        for task in &ready {
            let p = task.priority.unwrap_or(4);
            *by_priority.entry(p).or_insert(0) += 1;
        }

        // Compute deadline counts across all tasks
        let today = chrono::Utc::now().date_naive();
        let mut overdue: usize = 0;
        let mut due_today: usize = 0;
        let mut due_this_week: usize = 0;
        for task in &all_tasks {
            if let Some(ref due) = task.due {
                let len = std::cmp::min(10, due.len());
                if let Ok(due_date) = chrono::NaiveDate::parse_from_str(
                    &due[..due.floor_char_boundary(len)],
                    "%Y-%m-%d",
                ) {
                    let days_until = (due_date - today).num_days();
                    if days_until < 0 {
                        overdue += 1;
                    } else if days_until == 0 {
                        due_today += 1;
                    } else if days_until <= 7 {
                        due_this_week += 1;
                    }
                }
            }
        }

        let summary = serde_json::json!({
            "ready": ready.len(),
            "blocked": blocked.len(),
            "by_priority": {
                "p0": by_priority.get(&0).copied().unwrap_or(0),
                "p1": by_priority.get(&1).copied().unwrap_or(0),
                "p2": by_priority.get(&2).copied().unwrap_or(0),
                "p3": by_priority.get(&3).copied().unwrap_or(0),
            },
            "deadlines": {
                "overdue": overdue,
                "due_today": due_today,
                "due_this_week": due_this_week,
            }
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&summary).unwrap_or_default(),
        )]))
    }

}
