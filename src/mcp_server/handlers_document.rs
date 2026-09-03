use rmcp::model::*;
use rmcp::ErrorData as McpError;
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::path::PathBuf;

use super::{PkbSearchServer, MAX_RESULTS};

impl PkbSearchServer {
    pub(crate) fn handle_get_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        if args.get("path").is_some() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("The 'path' parameter is no longer supported. Please use 'id' instead (supports ID, filename stem, or title)."),
                data: None,
            });
        }

        let query = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        // Try ID/flexible resolution first (covers IDs, filename stems, titles, permalinks)
        let (path, label) = {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(query) {
                let abs_path = self.abs_path_for_node(node, Some(&graph))?;
                (abs_path, node.label.clone())
            } else {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Document not found: {query}")),
                    data: None,
                });
            }
        };

        if !path.exists() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "File not found for ID '{query}'"
                )),
                data: None,
            });
        }

        // task_5f2c5fa6 (aops_fdf19283): a resolved path that exists but is a
        // directory (a document/directory name collision) must not fall
        // through to a bare EISDIR from read_to_string below.
        if path.is_dir() {
            let rel = path.strip_prefix(&self.pkb_root).unwrap_or(&path).display();
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "'{query}' resolves to a directory, not a file, at '{rel}' — a document/directory \
                     name collision. There is no file to read for this id. Rename or remove the \
                     colliding directory, or repair the index entry for '{query}'."
                )),
                data: None,
            });
        }

        let content = std::fs::read_to_string(&path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read file: {e}")),
            data: None,
        })?;

        let max_bytes = args
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let content = Self::truncate_body(content, max_bytes);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "## {}\n\n{}",
            label, content
        ))]))
    }

    pub(crate) fn handle_list_documents(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let tag = args.get("tag").and_then(|v| v.as_str());
        let doc_type = args.get("type").and_then(|v| v.as_str());
        let status = args.get("status").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let store = self.store.read();
        let results = store.list_documents(tag, doc_type, status, &self.pkb_root);
        let total = results.len();

        if total == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No documents found matching filters.",
            )]));
        }

        // Default is "all matching" but always capped at MAX_RESULTS so a giant
        // PKB can't blow up a client's context in one response — use `offset`
        // to page through the rest.
        let page: Vec<_> = results
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(total).min(MAX_RESULTS))
            .collect();
        let showing = page.len();

        let mut output =
            format!("**{total} documents found** (showing {showing}, offset {offset})\n\n");

        for r in &page {
            let id = &r.id;
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            output.push_str(&format!(" — `{id}`"));
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    // =========================================================================
    // GRAPH/TASK TOOLS (7)
    // =========================================================================

    pub(crate) fn handle_create_memory(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let fields = crate::document_crud::MemoryFields {
            title: title.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            memory_type: args
                .get("memory_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            source: args
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from),
            confidence: args.get("confidence").and_then(|v| v.as_f64()),
            supersedes: args
                .get("supersedes")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        let path =
            crate::document_crud::create_memory(&self.pkb_root, fields).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create memory: {e}")),
                data: None,
            })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::create_memory", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::create_memory", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        let filename = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mem_id = parsed
            .as_ref()
            .map(|d| d.id())
            .unwrap_or_else(|| filename.clone());

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::create_memory", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::create_memory", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::create_memory", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::create_memory", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Memory created: `{filename}` (`{mem_id}`)"
        ))]))
    }

    pub(crate) fn handle_create_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: title"),
                data: None,
            })?;

        let doc_type = args
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: type"),
                data: None,
            })?;

        let fields = crate::document_crud::DocumentFields {
            title: title.to_string(),
            doc_type: doc_type.to_string(),
            id: args.get("id").and_then(|v| v.as_str()).map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            body: args.get("body").and_then(|v| v.as_str()).map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .map(String::from),
            intent: args
                .get("intent")
                .or_else(|| args.get("priority"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            parent: args
                .get("parent")
                .and_then(|v| v.as_str())
                .map(String::from),
            depends_on: args
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            soft_depends_on: args
                .get("soft_depends_on")
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
            source: args
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from),
            due: args.get("due").and_then(|v| v.as_str()).map(String::from),
            confidence: args.get("confidence").and_then(|v| v.as_f64()),
            supersedes: args
                .get("supersedes")
                .and_then(|v| v.as_str())
                .map(String::from),
            severity: args
                .get("severity")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
            goal_type: args
                .get("goal_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            dir: args.get("dir").and_then(|v| v.as_str()).map(String::from),
            stakeholder: args
                .get("stakeholder")
                .and_then(|v| v.as_str())
                .map(String::from),
            waiting_since: args
                .get("waiting_since")
                .and_then(|v| v.as_str())
                .map(String::from),
            contributes_to: args
                .get("contributes_to")
                .and_then(|v| v.as_array()).cloned()
                .unwrap_or_default(),
        };

        // Hierarchy validation: warn if task-like type without parent
        let mut warnings = Vec::new();
        let root_allowed = ["learn", "epic"];
        let task_like = ["task", "epic"];
        if task_like.contains(&doc_type) && fields.parent.is_none() {
            warnings.push(format!(
                "Hierarchy warning: Type '{}' should have a parent. \
                 Only {} types can be root-level. \
                 Consider assigning a parent to maintain graph hierarchy.",
                doc_type,
                root_allowed.join(", "),
            ));
        }

        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        let path = crate::document_crud::create_document(&self.pkb_root, fields).map_err(|e| {
            McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create document: {e}")),
                data: None,
            }
        })?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::create_document", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::create_document", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        let filename = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let doc_id = parsed
            .as_ref()
            .map(|d| d.id())
            .unwrap_or_else(|| filename.clone());

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::create_document", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::create_document", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::create_document", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::create_document", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        let mut msg = format!("Document created: `{filename}` (`{doc_id}`)");
        if !warnings.is_empty() {
            msg.push_str("\n\nHierarchy warnings:\n");
            for w in &warnings {
                msg.push_str(&format!("- {}\n", w));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    /// Convert a `document_crud` write error into an `McpError`, giving a
    /// stale-write (CAS precondition failure) a distinct, structured shape so
    /// a caller can tell "your snapshot is stale, re-read and merge" apart
    /// from any other write failure — see `document_crud::StaleWrite`.
    pub(crate) fn write_error_to_mcp(action: &str, e: anyhow::Error) -> McpError {
        if let Some(stale) = e.downcast_ref::<crate::document_crud::StaleWrite>() {
            return McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Stale write rejected: the document changed since your read \
                     (expected_modified={}, current modified={}). Re-read the \
                     document and merge your changes before retrying.",
                    stale.expected_modified, stale.actual_modified
                )),
                data: Some(serde_json::json!({
                    "error_type": "stale_write",
                    "expected_modified": stale.expected_modified,
                    "actual_modified": stale.actual_modified,
                })),
            };
        }
        if let Some(not_found) = e.downcast_ref::<crate::document_crud::SelectorNotFound>() {
            return McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Observation selector not found: {:?} — selector matched no lines in the document",
                    not_found.0
                )),
                data: Some(serde_json::json!({
                    "error_type": "selector_not_found",
                    "selector": not_found.0,
                })),
            };
        }
        if let Some(diff_err) = e.downcast_ref::<crate::udiff::UdiffError>() {
            return McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("diff_application_failed: {diff_err}")),
                data: Some(serde_json::json!({
                    "error_type": "diff_application_failed",
                    "details": diff_err.to_string(),
                })),
            };
        }
        McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to {action}: {e}")),
            data: None,
        }
    }

    pub(crate) fn handle_append_to_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
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

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: content"),
                data: None,
            })?;

        let section = args.get("section").and_then(|v| v.as_str());
        let expected_modified = args.get("expected_modified").and_then(|v| v.as_str());

        // Resolve ID to path via graph
        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Document not found: {id}")),
            data: None,
        })?;

        let abs_path = self.abs_path_for_node(node, Some(&graph))?;
        let label = node.label.clone();
        drop(graph);

        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        let new_modified =
            crate::document_crud::append_to_document(&abs_path, content, section, expected_modified)
                .map_err(|e| Self::write_error_to_mcp("append", e))?;
        let elapsed_write = t.elapsed();
        tracing::debug!(target: "perf::append_to_document", phase = "write_file", elapsed_ms = elapsed_write.as_secs_f64() * 1000.0);

        let t = std::time::Instant::now();
        let parsed = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root);
        let elapsed_parse = t.elapsed();
        tracing::debug!(target: "perf::append_to_document", phase = "parse", elapsed_ms = elapsed_parse.as_secs_f64() * 1000.0);

        if let Some(doc) = parsed {
            let t = std::time::Instant::now();
            self.rebuild_graph_for_pkb_document(&doc);
            let elapsed_graph = t.elapsed();
            tracing::debug!(target: "perf::append_to_document", phase = "rebuild_graph_fast", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

            let t = std::time::Instant::now();
            self.try_upsert_document(&doc);
            let elapsed_upsert = t.elapsed();
            tracing::debug!(target: "perf::append_to_document", phase = "vector_upsert_and_save", elapsed_ms = elapsed_upsert.as_secs_f64() * 1000.0);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            let t = std::time::Instant::now();
            self.rebuild_graph();
            tracing::debug!(target: "perf::append_to_document", phase = "rebuild_graph_full", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0);
        }

        tracing::debug!(target: "perf::append_to_document", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        let section_msg = section
            .map(|s| format!(" under ## {s}"))
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Appended to: {} (`{}`){section_msg}\nmodified: {}",
            label, id, new_modified
        ))]))
    }

    pub(crate) fn handle_update_body(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let new_body = args
            .get("new_body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: new_body"),
                data: None,
            })?;

        let preserve_frontmatter = args
            .get("preserve_frontmatter")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let expected_modified = args.get("expected_modified").and_then(|v| v.as_str());

        let (abs_path, label) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            (self.abs_path_for_node(node, Some(&graph))?, node.label.clone())
        };

        let result = crate::document_crud::rewrite_body(
            &abs_path,
            new_body,
            preserve_frontmatter,
            expected_modified,
        )
        .map_err(|e| Self::write_error_to_mcp("rewrite body", e))?;

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            self.rebuild_graph();
        }

        let response_payload = serde_json::json!({
            "ok": true,
            "id": id,
            "title": label,
            "body_chars_before": result.body_chars_before,
            "body_chars_after": result.body_chars_after,
            "preserve_frontmatter": preserve_frontmatter,
            "modified": result.modified
        });
        let response_text = serde_json::to_string(&response_payload).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize response: {e}")),
            data: None,
        })?;
        Ok(CallToolResult::success(vec![Content::text(response_text)]))
    }

    pub(crate) fn handle_edit_body(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let diff = args
            .get("diff")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: diff"),
                data: None,
            })?;

        let preserve_frontmatter = args
            .get("preserve_frontmatter")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let expected_modified = args.get("expected_modified").and_then(|v| v.as_str());

        let (abs_path, label) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            (self.abs_path_for_node(node, Some(&graph))?, node.label.clone())
        };

        let result = crate::document_crud::edit_body(
            &abs_path,
            diff,
            preserve_frontmatter,
            expected_modified,
            dry_run,
        )
        .map_err(|e| Self::write_error_to_mcp("edit body", e))?;

        if !dry_run {
            if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
                self.rebuild_graph_for_pkb_document(&doc);
                self.try_upsert_document(&doc);
            } else {
                tracing::warn!(
                    "Incremental parse failed for {:?}, doing full rebuild",
                    abs_path
                );
                self.rebuild_graph();
            }
        }

        let response_payload = serde_json::json!({
            "ok": true,
            "id": id,
            "title": label,
            "dry_run": dry_run,
            "body_chars_before": result.body_chars_before,
            "body_chars_after": result.body_chars_after,
            "hunks_applied": result.hunks_applied,
            "preserve_frontmatter": preserve_frontmatter,
            "modified": result.modified
        });
        let response_text = serde_json::to_string(&response_payload).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize response: {e}")),
            data: None,
        })?;
        Ok(CallToolResult::success(vec![Content::text(response_text)]))
    }

    pub(crate) fn handle_add_observations(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let lines = args
            .get("lines")
            .and_then(|v| v.as_array())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: lines (must be an array of strings)"),
                data: None,
            })?;

        let lines_vec: Vec<String> = lines
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from("Each item in 'lines' must be a string"),
                        data: None,
                    })
            })
            .collect::<Result<Vec<String>, McpError>>()?;

        let section = args.get("section").and_then(|v| v.as_str());
        let timestamped = args
            .get("timestamped")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let expected_modified = args.get("expected_modified").and_then(|v| v.as_str());

        let (abs_path, label) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            (self.abs_path_for_node(node, Some(&graph))?, node.label.clone())
        };

        let result = crate::document_crud::add_observations(
            &abs_path,
            &lines_vec,
            section,
            timestamped,
            expected_modified,
        )
        .map_err(|e| Self::write_error_to_mcp("add observations", e))?;

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            self.rebuild_graph();
        }

        let response_payload = serde_json::json!({
            "ok": true,
            "id": id,
            "title": label,
            "added_count": result.added_count,
            "body_chars_before": result.body_chars_before,
            "body_chars_after": result.body_chars_after,
            "modified": result.modified
        });
        let response_text = serde_json::to_string(&response_payload).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize response: {e}")),
            data: None,
        })?;
        Ok(CallToolResult::success(vec![Content::text(response_text)]))
    }

    pub(crate) fn handle_delete_observations(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let selectors = args
            .get("selectors")
            .and_then(|v| v.as_array())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: selectors (must be an array of strings)"),
                data: None,
            })?;

        let selectors_vec: Vec<String> = selectors
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from("Each item in 'selectors' must be a string"),
                        data: None,
                    })
            })
            .collect::<Result<Vec<String>, McpError>>()?;

        let expected_modified = args.get("expected_modified").and_then(|v| v.as_str());

        let (abs_path, label) = {
            let graph = self.graph.read();
            let node = graph.resolve(id).ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Document not found: {id}")),
                data: None,
            })?;
            (self.abs_path_for_node(node, Some(&graph))?, node.label.clone())
        };

        let result = crate::document_crud::delete_observations(
            &abs_path,
            &selectors_vec,
            expected_modified,
        )
        .map_err(|e| Self::write_error_to_mcp("delete observations", e))?;

        if let Some(doc) = crate::pkb::parse_file_relative(&abs_path, &self.pkb_root) {
            self.rebuild_graph_for_pkb_document(&doc);
            self.try_upsert_document(&doc);
        } else {
            tracing::warn!(
                "Incremental parse failed for {:?}, doing full rebuild",
                abs_path
            );
            self.rebuild_graph();
        }

        let response_payload = serde_json::json!({
            "ok": true,
            "id": id,
            "title": label,
            "deleted_count": result.deleted_count,
            "body_chars_before": result.body_chars_before,
            "body_chars_after": result.body_chars_after,
            "modified": result.modified
        });
        let response_text = serde_json::to_string(&response_payload).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize response: {e}")),
            data: None,
        })?;
        Ok(CallToolResult::success(vec![Content::text(response_text)]))
    }

    pub(crate) fn handle_delete_document(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.resolve(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Document not found: {id}")),
            data: None,
        })?;

        // Optional type guard (folded in from the former `delete_memory` tool):
        // pass type="memory" to restrict deletion to memory-type nodes (memory,
        // note, insight, observation), or any other type string to require an
        // exact (case-insensitive) node_type match.
        if let Some(guard) = args.get("type").and_then(|v| v.as_str()) {
            let node_type = node.node_type.as_deref().unwrap_or("");
            let matches = if guard.eq_ignore_ascii_case("memory") {
                let memory_types = ["memory", "note", "insight", "observation"];
                memory_types
                    .iter()
                    .any(|mt| node_type.eq_ignore_ascii_case(mt))
            } else {
                node_type.eq_ignore_ascii_case(guard)
            };
            if !matches {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Document {id} does not match type guard '{guard}' (actual type: {:?})",
                        node.node_type
                    )),
                    data: None,
                });
            }
        }

        let abs_path = self.abs_path_for_node(node, Some(&graph))?;
        let label = node.label.clone();
        let node_id = node.id.clone();
        let rel_path = node.path.to_string_lossy().to_string();
        drop(graph); // release read lock before write operations

        let t_total = std::time::Instant::now();

        let t = std::time::Instant::now();
        crate::document_crud::delete_document(&abs_path).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;
        let elapsed_delete = t.elapsed();
        tracing::debug!(target: "perf::delete_document", phase = "delete_file", elapsed_ms = elapsed_delete.as_secs_f64() * 1000.0);

        // Incremental graph update — remove the deleted node
        let t = std::time::Instant::now();
        self.rebuild_graph_remove(&node_id);
        let elapsed_graph = t.elapsed();
        tracing::debug!(target: "perf::delete_document", phase = "rebuild_graph_remove", elapsed_ms = elapsed_graph.as_secs_f64() * 1000.0);

        // Remove from vector store (skipped if reindex holds the lock)
        let t = std::time::Instant::now();
        self.try_remove_document(&rel_path);
        let elapsed_remove = t.elapsed();
        tracing::debug!(target: "perf::delete_document", phase = "vector_remove", elapsed_ms = elapsed_remove.as_secs_f64() * 1000.0);

        tracing::debug!(target: "perf::delete_document", phase = "TOTAL", elapsed_ms = t_total.elapsed().as_secs_f64() * 1000.0);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Deleted: {} (`{}`)",
            label,
            node_id
        ))]))
    }

    /// Build the compact "mutation neighborhood" returned by `complete_task` and
    /// `release_task`. See `specs/mutation-neighborhood.md`. Computed against the
    /// post-write graph, so statuses, `siblings_open`, and `unblocked` reflect the
    /// just-applied close. Returns `Value::Null` when no field carries signal.
    ///
    /// `cascade_closed` is the number of descendants closed by a recursive cascade
    /// (0 when the close was non-recursive).
    pub(crate) fn handle_retrieve_memory(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(MAX_RESULTS);
        let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let query_embedding = self.embedder.encode(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        let results = store.search(&query_embedding, limit * 3, &self.pkb_root, None, None, None);

        let graph = self.graph.read();

        let memory_types = ["memory", "note", "insight", "observation"];
        let mut scored_results = Vec::new();

        for r in results {
            let is_memory = r
                .doc_type
                .as_deref()
                .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                .unwrap_or(false);
            if !is_memory {
                continue;
            }

            if let Some(ref required_tags) = tags {
                let has_all = required_tags
                    .iter()
                    .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)));
                if !has_all {
                    continue;
                }
            }

            let confidence = graph
                .get_node(&r.id)
                .and_then(|n| n.confidence)
                .unwrap_or(1.0);

            // Adjust score by confidence (tie-breaker)
            let combined_score = r.score + (confidence as f32 * 0.0001);
            scored_results.push((r, combined_score, confidence));
        }

        // Re-sort by combined score
        scored_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut output = String::new();
        let mut count = 0;

        for (r, _score, confidence) in scored_results {
            if count >= limit {
                break;
            }

            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3}, confidence: {:.2})\n",
                count, r.title, r.score, confidence
            ));

            // Check if superseded and get display ID
            let display_id = if let Some(node) = graph.get_node(&r.id) {
                let incoming = graph.get_incoming_edges(&node.id);
                let superseders: Vec<_> = incoming
                    .iter()
                    .filter(|e| e.edge_type == crate::graph::EdgeType::Supersedes)
                    .collect();
                for edge in &superseders {
                    let superseder_label = graph
                        .get_node(&edge.source)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    output.push_str(&format!(
                        "⚠️ **SUPERSEDED BY:** `{}` ({})\n",
                        edge.source, superseder_label
                    ));
                }
                node.task_id.as_deref().unwrap_or(&node.id).to_string()
            } else {
                "?".to_string()
            };
            output.push_str(&format!("**ID:** `{display_id}`\n"));
            if !r.tags.is_empty() {
                output.push_str(&format!("**Tags:** {}\n", r.tags.join(", ")));
            }
            // Show full body for memories (typically short)
            let abs_path = graph
                .get_node(&r.id)
                .and_then(|n| self.abs_path_for_node(n, Some(&graph)).ok())
                .unwrap_or_else(|| if r.path.as_os_str().is_empty() { PathBuf::new() } else { r.path.clone() });
            if !abs_path.as_os_str().is_empty() {
                if let Ok(content) = std::fs::read_to_string(&abs_path) {
                    let body = if content.starts_with("---") {
                        content.splitn(3, "---").nth(2).unwrap_or("").trim()
                    } else {
                        content.trim()
                    };
                    if !body.is_empty() {
                        output.push_str(&format!("\n{body}\n"));
                    }
                }
            }
            output.push('\n');
        }

        if count == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No memories found matching query.",
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "**Found {count} memories for:** \"{query}\"\n\n{output}"
        ))]))
    }

    pub(crate) fn handle_list_memories(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize).min(MAX_RESULTS);
        let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let memory_types = ["memory", "note", "insight", "observation"];
        let store = self.store.read();

        let mut memories: Vec<_> = store
            .list_documents(None, None, None, &self.pkb_root)
            .into_iter()
            .filter(|r| {
                r.doc_type
                    .as_deref()
                    .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                    .unwrap_or(false)
            })
            .collect();

        if let Some(ref required_tags) = tags {
            memories.retain(|r| {
                required_tags
                    .iter()
                    .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)))
            });
        }

        memories.truncate(limit);

        if memories.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No memories found.",
            )]));
        }

        let total = memories.len();
        let mut output = format!("**{total} memories**\n\n");

        for r in &memories {
            output.push_str(&format!("- **{}**", r.title));
            if !r.tags.is_empty() {
                output.push_str(&format!(" ({})", r.tags.join(", ")));
            }
            let id = &r.id;
            output.push_str(&format!(" — `{id}`\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

}
