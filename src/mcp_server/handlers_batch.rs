use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::graph_store::{GraphStore, DEFAULT_DIVERGENCE_THRESHOLD_DAYS};

use super::{PkbSearchServer, ReindexStatus, MAX_RESULTS, DRY_RUN_WARNING};

impl PkbSearchServer {
    pub(crate) fn handle_get_network_metrics(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let graph = self.graph.read();
        let node = graph.get_node(id).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Node not found: {id}")),
            data: None,
        })?;

        let node_ids: Vec<String> = graph.nodes().map(|n| n.id.clone()).collect();
        let edges = graph.edges();

        let m = crate::metrics::compute_network_metrics(
            id,
            &node_ids,
            edges,
            node.downstream_weight,
            node.stakeholder_exposure,
        );

        match m {
            Some(metrics) => {
                let json = serde_json::to_string_pretty(&metrics).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "## Network metrics for {id}\n\n```json\n{json}\n```"
                ))]))
            }
            None => Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from("Failed to compute metrics"),
                data: None,
            }),
        }
    }

    pub(crate) fn handle_top_n_by_metric(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let metric = args
            .get("metric")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: metric"),
                data: None,
            })?;

        if metric != "pagerank" && metric != "betweenness" && metric != "degree" {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Invalid metric: must be 'pagerank', 'betweenness', or 'degree'",
                ),
                data: None,
            });
        }

        let n = (args.get("n").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(MAX_RESULTS);

        let node_type = args.get("node_type").and_then(|v| v.as_str());

        let graph = self.graph.read();
        let mut nodes: Vec<_> = graph
            .nodes()
            .filter(|n| {
                if let Some(nt) = node_type {
                    n.node_type
                        .as_deref()
                        .map(|t| t.eq_ignore_ascii_case(nt))
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .collect();

        match metric {
            "pagerank" => {
                nodes.sort_by(|a, b| {
                    b.pagerank
                        .partial_cmp(&a.pagerank)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.id.cmp(&b.id))
                });
            }
            "betweenness" => {
                nodes.sort_by(|a, b| {
                    b.betweenness
                        .partial_cmp(&a.betweenness)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.id.cmp(&b.id))
                });
            }
            "degree" => {
                nodes.sort_by(|a, b| {
                    let deg_a = a.indegree + a.outdegree;
                    let deg_b = b.indegree + b.outdegree;
                    deg_b.cmp(&deg_a).then_with(|| a.id.cmp(&b.id))
                });
            }
            _ => unreachable!(),
        }

        let top_nodes = nodes.into_iter().take(n);
        let results: Vec<serde_json::Value> = top_nodes
            .map(|n| {
                let val = match metric {
                    "pagerank" => n.pagerank,
                    "betweenness" => n.betweenness,
                    "degree" => (n.indegree + n.outdegree) as f64,
                    _ => 0.0,
                };
                serde_json::json!({
                    "id": n.id,
                    "title": n.label,
                    "metric_value": val,
                })
            })
            .collect();

        let json = serde_json::to_string_pretty(&results).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_batch_update(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let updates = args
            .get("updates")
            .cloned()
            .unwrap_or(JsonValue::Object(serde_json::Map::new()));
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // task_1381381c: agents must not originate intent bands via batch_update.
        if updates.get("intent").is_some() || updates.get("priority").is_some() {
            return Err(Self::reject_agent_intent("batch_update"));
        }

        if filters.is_empty() && updates.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter and one update field required"),
                data: None,
            });
        }

        let graph = self.graph.read();
        // Reject target/goal nodes as structural parents when the batch sets `parent`.
        if let Some(new_parent) = updates.get("parent").and_then(|v| v.as_str()) {
            if !new_parent.trim().is_empty() {
                if let Err(msg) = graph.reject_target_as_parent(new_parent.trim()) {
                    drop(graph);
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
            }
        }
        let summary = crate::batch_ops::update::batch_update(
            &graph,
            &self.pkb_root,
            &filters,
            &updates,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_batch_reparent(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let new_parent = args
            .get("new_parent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("new_parent is required"),
                data: None,
            })?
            .to_string();

        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let graph = self.graph.read();
        let summary = crate::batch_ops::reparent::batch_reparent(
            &graph,
            &self.pkb_root,
            &filters,
            &new_parent,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_batch_archive(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // default true!
        let reason = args.get("reason").and_then(|v| v.as_str());

        if filters.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter is required for batch archive"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::update::batch_archive(
            &graph,
            &self.pkb_root,
            &filters,
            reason,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_merge_node(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let canonical_id = args
            .get("canonical_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("canonical_id is required"),
                data: None,
            })?
            .to_string();

        let source_ids: Vec<String> = args
            .get("source_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if source_ids.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("source_ids must contain at least one ID"),
                data: None,
            });
        }

        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let summary =
            crate::document_crud::merge_node(&self.pkb_root, &source_ids, &canonical_id, dry_run)
                .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("merge_node failed: {e}")),
                data: None,
            })?;

        if !dry_run && (summary.nodes_archived > 0 || summary.files_updated > 0) {
            self.finalize_batch(&summary.modified_paths, &[]);
        }

        // Dry-run counters describe changes that did NOT happen. Report them in
        // the conditional, matching `pkb merge-node --dry-run`; the indicative
        // ("1 node(s) archived") reads as a completed write and has already been
        // mistaken for one.
        let mut msg = if dry_run {
            format!(
                "merge_node (dry run): {} file(s) would be updated, {} reference(s) would be \
                 redirected, {} node(s) would be archived — nothing was written",
                summary.files_updated, summary.refs_redirected, summary.nodes_archived,
            )
        } else {
            format!(
                "merge_node: {} files updated, {} references redirected, {} node(s) archived",
                summary.files_updated, summary.refs_redirected, summary.nodes_archived,
            )
        };
        if dry_run {
            msg = format!("{}{}", DRY_RUN_WARNING, msg);
        }
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    pub(crate) fn handle_graph_json(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let graph = self.graph.read();
        let json = graph.output_json().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate graph JSON: {e}")),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn handle_graph_excalidraw(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let node_id = args.get("node_id").and_then(|v| v.as_str());
        let hops = args
            .get("hops")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        let graph = self.graph.read();
        let (json, _, _) = graph.output_excalidraw(node_id, hops).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to export Excalidraw graph: {e}")),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn handle_export_graph(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let focus = args.get("focus").and_then(|v| v.as_str());
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;
        let project = args.get("project").and_then(|v| v.as_str());
        let include_done = args
            .get("include_done")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let graph = self.graph.read();
        let dot = graph.output_dot(focus, max_depth, project, include_done);

        Ok(CallToolResult::success(vec![Content::text(dot)]))
    }

    pub fn handle_diff_excalidraw(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let canvas_str = args
            .get("canvas")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: canvas"),
                data: None,
            })?;

        let canvas = crate::excalidraw::parse_canvas(canvas_str).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Failed to parse canvas JSON: {e}")),
            data: None,
        })?;

        let base_snapshot = if let Some(base_str) = args.get("base").and_then(|v| v.as_str()) {
            Some(
                crate::excalidraw::parse_base_snapshot(base_str).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Failed to parse base snapshot: {e}")),
                    data: None,
                })?,
            )
        } else {
            None
        };

        let graph = self.graph.read();
        let diff = crate::excalidraw::diff_canvas(base_snapshot.as_ref(), &graph, &canvas).map_err(|e| {
            McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to compute diff: {e}")),
                data: None,
            }
        })?;

        let json = serde_json::to_string_pretty(&diff).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to serialize diff: {e}")),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn handle_sync_excalidraw(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let canvas_str = args
            .get("canvas")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: canvas"),
                data: None,
            })?;

        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let sync_edge_removals = args
            .get("sync_edge_removals")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let canvas = crate::excalidraw::parse_canvas(canvas_str).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Failed to parse canvas JSON: {e}")),
            data: None,
        })?;

        let base_snapshot = if let Some(base_str) = args.get("base").and_then(|v| v.as_str()) {
            Some(
                crate::excalidraw::parse_base_snapshot(base_str).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Failed to parse base snapshot: {e}")),
                    data: None,
                })?,
            )
        } else {
            None
        };

        let diff = {
            let graph = self.graph.read();
            crate::excalidraw::diff_canvas(base_snapshot.as_ref(), &graph, &canvas).map_err(|e| {
                McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to compute diff: {e}")),
                    data: None,
                }
            })?
        };

        if dry_run {
            let res = serde_json::json!({
                "dry_run": true,
                "diff": diff,
                "message": format!(
                    "Dry run: {} added nodes, {} updated nodes, {} canvas-only removals, {} added edges, {} removed edges, {} conflicts",
                    diff.added_nodes.len(),
                    diff.updated_nodes.len(),
                    diff.removed_from_canvas.len(),
                    diff.added_edges.len(),
                    diff.removed_edges.len(),
                    diff.conflicts.len(),
                ),
            });
            let json = serde_json::to_string_pretty(&res).unwrap_or_default();
            return Ok(CallToolResult::success(vec![Content::text(json)]));
        }

        let report = {
            let mut graph = self.graph.write();
            crate::excalidraw::sync_canvas(&self.pkb_root, &mut graph, &diff, sync_edge_removals).map_err(|e| {
                McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to sync canvas to disk: {e}")),
                    data: None,
                }
            })?
        };

        // Update vector store and schedule background graph rebuild
        for (_id, path) in &report.created_nodes {
            if let Some(doc) = crate::pkb::parse_file_relative(path, &self.pkb_root) {
                self.try_upsert_document(&doc);
            }
        }
        for id in &report.updated_nodes {
            if let Some(node) = self.graph.read().get_node(id) {
                let path = self.pkb_root.join(&node.path);
                if let Some(doc) = crate::pkb::parse_file_relative(&path, &self.pkb_root) {
                    self.try_upsert_document(&doc);
                }
            }
        }
        self.schedule_graph_rebuild();

        let res = serde_json::json!({
            "success": true,
            "created_nodes": report.created_nodes.iter().map(|(id, path)| {
                let filename = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                serde_json::json!({ "id": id, "filename": filename })
            }).collect::<Vec<_>>(),
            "updated_nodes": report.updated_nodes,
            "updated_edges": report.updated_edges,
            "rejected_cycles": report.rejected_cycles,
            "warnings": report.warnings,
        });

        let json = serde_json::to_string_pretty(&res).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_graph_stats(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let graph = self.graph.read();
        let stats = crate::batch_ops::stats::graph_stats(&graph);
        drop(graph);

        let json = serde_json::to_string_pretty(&stats).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_detect_weight_divergence(
        &self,
        args: &JsonValue,
    ) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let threshold_days = args
            .get("threshold_days")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_DIVERGENCE_THRESHOLD_DAYS);

        let graph = self.graph.read();
        let divergences = graph.detect_weight_divergences(threshold_days);
        drop(graph);

        let total = divergences.len();

        if total == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No weight divergences detected (high stated weight with revealed inactivity).",
            )]));
        }

        let mut out = format!(
            "Detected **{}** weight divergences (high stated weight but source task inactive for >= {} days).\n\n",
            total, threshold_days
        );

        out.push_str("| Source Task | Weight | Target | Inactive | Justification |\n");
        out.push_str("| :--- | :--- | :--- | :--- | :--- |\n");

        for div in &divergences {
            out.push_str(&format!(
                "| {} (`{}`) | **{}** | {} (`{}`) | {}d | {} |\n",
                div.source_label,
                div.source_id,
                div.stated_weight,
                div.target_label,
                div.target_id,
                div.days_since_interaction,
                div.justification
            ));
        }

        out.push_str("\n\n**Resolution surface:** For each divergence, consider if the contribution weight is still accurate given the lack of recent activity on the source task.");

        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    pub(crate) fn handle_batch_merge(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let canonical = args
            .get("canonical")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("canonical is required"),
                data: None,
            })?
            .to_string();

        let merge_ids: Vec<String> = args
            .get("merge_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if merge_ids.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("merge_ids must contain at least one ID"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::duplicates::batch_merge(
            &graph,
            &self.pkb_root,
            &canonical,
            &merge_ids,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_batch_create_epics(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let parent = args.get("parent").and_then(|v| v.as_str());
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let epics: Vec<crate::batch_ops::epics::EpicDef> = args
            .get("epics")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if epics.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("epics array is required and must not be empty"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::epics::batch_create_epics(
            &graph,
            &self.pkb_root,
            parent,
            &epics,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_get_stats(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let stats = crate::telemetry::get_stats();
        let json = serde_json::to_string_pretty(&stats).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_status(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let node_count = self.graph.read().node_count();
        let vector_count = self.store.read().len();
        let last_reindex = self.last_reindex.read().clone();
        let embed_pending_count = self.embed_pending.lock().len();
        let deferred_paths_count = self.deferred_paths.lock().len();
        let queue_depth = embed_pending_count + deferred_paths_count;
        let index_locked = !self.index_lock_available();
        let external_lock_held = self.external_lock_held();
        let save_in_flight = self.save_in_flight.load(Ordering::Relaxed);
        let embed_worker_running = self.embed_worker_running.load(Ordering::Relaxed);
        let graph_rebuild_pending = self.graph_rebuild_pending.load(Ordering::Relaxed)
            || self.graph_rebuild_dirty.load(Ordering::Relaxed);
        let stale_docs = crate::check_index_staleness(&self.pkb_root, &self.store);
        let is_fresh = stale_docs == 0;

        let info = serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "git_hash": env!("BUILD_GIT_HASH"),
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "index": {
                "document_count": node_count,
                "vector_count": vector_count,
                "last_reindex": {
                    "timestamp": last_reindex.timestamp,
                    "outcome": last_reindex.outcome,
                }
            },
            "queue": {
                "depth": queue_depth,
                "embed_pending": embed_pending_count,
                "deferred_paths": deferred_paths_count,
            },
            "write_state": {
                "index_locked": index_locked,
                "external_lock_held": external_lock_held,
                "save_in_flight": save_in_flight,
                "embed_worker_running": embed_worker_running,
                "graph_rebuild_pending": graph_rebuild_pending,
            },
            "freshness": {
                "is_fresh": is_fresh,
                "stale_documents": stale_docs,
            }
        });
        let json = serde_json::to_string_pretty(&info).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_batch_reclassify(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let new_type = args
            .get("new_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("new_type is required"),
                data: None,
            })?;
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if filters.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("At least one filter is required"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let summary = crate::batch_ops::reclassify::batch_reclassify(
            &graph,
            &self.pkb_root,
            &filters,
            new_type,
            dry_run,
        );
        drop(graph);

        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }

        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub(crate) fn handle_refresh_graph(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let (scanned, parsed, node_count) = self.rebuild_graph();
        let skipped = scanned.saturating_sub(parsed);
        let json = serde_json::json!({
            "ok": true,
            "node_count": node_count,
            "scanned_files": scanned,
            "parsed_documents": parsed,
            "unparseable_or_skipped_files": skipped,
            "message": if skipped > 0 {
                format!("Graph index rebuilt from disk. {node_count} nodes loaded ({parsed} valid documents from {scanned} scanned files; {skipped} unparseable or skipped files).")
            } else {
                format!("Graph index rebuilt from disk. {node_count} nodes loaded.")
            }
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json)
                .expect("infallible: static JSON value with primitive types"),
        )]))
    }

    pub(crate) fn handle_apply_consolidation_batch(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let seed_id = args.get("seed_id").and_then(|v| v.as_str()).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Missing required parameter: seed_id"),
            data: None,
        })?;
        
        let updates_val = args.get("updates").ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Missing required parameter: updates"),
            data: None,
        })?;
        
        let updates: std::collections::HashMap<String, std::collections::HashMap<String, JsonValue>> = serde_json::from_value(updates_val.clone()).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Invalid updates format: {}", e)),
            data: None,
        })?;
        
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let graph = self.graph.read();
        let mut ctx = crate::batch_ops::BatchContext::new(&graph, &self.pkb_root);
        
        let summary = crate::batch_ops::consolidation::apply_consolidation_batch(&mut ctx, seed_id, updates, dry_run).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to apply consolidation batch: {}", e)),
            data: None,
        })?;
        
        drop(graph);
        if !dry_run && summary.changed > 0 {
            self.finalize_batch(&summary.modified_paths, &summary.removed_paths);
        }
        
        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if dry_run {
            let msg = format!("{}{}", DRY_RUN_WARNING, json);
            Ok(CallToolResult::success(vec![Content::text(msg)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
    }

    pub(crate) fn handle_get_consolidation_cluster(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();
        let seed_id = args.get("seed_id").and_then(|v| v.as_str());
        let max_nodes = args.get("max_nodes").and_then(|v| v.as_i64()).unwrap_or(10) as usize;
        let vector_top_k = args.get("vector_top_k").and_then(|v| v.as_i64()).unwrap_or(10) as usize;

        let graph = self.graph.read();
        let store = self.store.read();

        let cluster = crate::batch_ops::consolidation::get_consolidation_cluster(
            &graph,
            &store,
            seed_id,
            max_nodes,
            vector_top_k,
            &self.pkb_root,
        ).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get consolidation cluster: {}", e)),
            data: None,
        })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&cluster).unwrap_or_default()
        )]))
    }

    // =========================================================================
    // CONSOLIDATED TOOLS (Progressive Disclosure)
    // =========================================================================

}

#[cfg(test)]
mod batch_finalize_tests {
    use super::*;
    use crate::embeddings::Embedder;
    use crate::graph_store::GraphStore;
    use crate::vectordb::VectorStore;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;

    fn build_disk_server(pkb_root: &Path) -> PkbSearchServer {
        std::fs::create_dir_all(pkb_root.join("projects")).unwrap();
        std::fs::write(
            pkb_root.join("polecat.yaml"),
            "projects:\n  test-project: {}\n  p: {}\n  aops: {}\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("projects/test-project.md"),
            "---\nid: test-project\ntitle: Test Project\ntype: project\nstatus: active\n---\n\n\
             # Test Project\n",
        )
        .unwrap();

        let store = VectorStore::new(crate::embeddings::EMBEDDING_DIM);
        let embedder = Embedder::new_dummy();
        let graph = GraphStore::build_from_directory(pkb_root);
        PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            pkb_root.to_path_buf(),
            pkb_root.join("test-index.bin"),
            Arc::new(RwLock::new(graph)),
        )
    }

    /// After a `batch_update` mutates frontmatter, the in-memory vector
    /// store entries must reflect the new values — search/list_documents
    /// otherwise return stale fields. This was the correctness bug fixed
    /// by the batch finalize pipeline.
    #[test]
    fn test_batch_update_refreshes_vector_store_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let server = build_disk_server(pkb_root);

        // Create three tasks under the project. Each call goes through the
        // create + try_upsert_document path, which seeds the vector store.
        for i in 1..=3 {
            let args = json!({
                "title": format!("Task {i}"),
                "parent": "test-project",
                "project": "test-project",
                "status": "ready",
            });
            server
                .handle_create_task(&args)
                .unwrap_or_else(|e| panic!("create_task {i} failed: {e:?}"));
        }

        // Snapshot the store: pick the freshly created tasks (status=active).
        let pre: std::collections::HashMap<String, Option<String>> = {
            let s = server.store.read();
            s.documents()
                .filter_map(|(_path, e)| {
                    if !e.id.is_empty() {
                        Some((e.id.clone(), e.status.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        };
        let task_ids: Vec<String> = pre
            .iter()
            .filter(|(_, status)| status.as_deref() == Some("ready"))
            .filter(|(id, _)| id.starts_with("test_project_"))
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(
            task_ids.len(),
            3,
            "expected 3 ready task entries; got {pre:?}"
        );

        // Run a batch update that flips status to "blocked". mem_2ecf862b:
        // a bare status="blocked" write is rejected — carry a blocker so
        // this exercises the batch-update path, not the guard.
        let args = json!({
            "ids": task_ids,
            "updates": { "status": "blocked", "blocker": "test fixture: awaiting upstream" },
            "dry_run": false
        });
        let result = server.handle_batch_update(&args).expect("batch_update");
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<String>();
        assert!(
            text.contains("\"changed\": 3"),
            "expected 3 changes; got: {text}"
        );

        // The fix: store entries should reflect the new status.
        let post: std::collections::HashMap<String, Option<String>> = {
            let s = server.store.read();
            s.documents()
                .filter_map(|(_path, e)| {
                    if !e.id.is_empty() {
                        Some((e.id.clone(), e.status.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        };
        for id in &task_ids {
            let status = post
                .get(id)
                .unwrap_or_else(|| panic!("entry {id} missing after batch_update"));
            assert_eq!(
                status.as_deref(),
                Some("blocked"),
                "entry {id} should carry status=blocked in vector store; got {status:?}"
            );
        }
    }

    /// task_1381381c: agents must not be able to set `intent` (or legacy `priority`) via
    /// `batch_update` — one of the write paths that had only a range check
    /// (`is_valid_intent`) and no authority guard.
    #[test]
    fn test_batch_update_rejects_agent_intent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let server = build_disk_server(pkb_root);

        let create_args = json!({
            "title": "Batch Intent Guard Task",
            "parent": "test-project",
            "project": "test-project",
            "status": "ready",
        });
        server
            .handle_create_task(&create_args)
            .expect("create_task failed");

        let args = json!({
            "parent": "test-project",
            "updates": { "intent": 0 },
            "dry_run": false
        });
        let err = server
            .handle_batch_update(&args)
            .expect_err("agent-supplied intent via batch_update should be rejected");
        let msg = format!("{}", err.message);
        assert!(
            msg.to_lowercase().contains("intent"),
            "error should mention intent, got: {msg}"
        );

        // Nothing should have been written despite the filter matching a task.
        let graph = server.graph.read();
        let node = graph
            .resolve("Batch Intent Guard Task")
            .expect("task should exist");
        assert_ne!(
            node.intent,
            Some(0),
            "intent must not have been written despite rejection"
        );
    }

    use crate::batch_ops::tree_snapshot;

    fn response_text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<String>()
    }

    /// Three linked tasks under `test-project`: `t3` carries a wikilink to `t2`,
    /// so a merge with `t2` as the source has both a reference to redirect and a
    /// node to archive — i.e. real writes to decline.
    fn seed_linked_tasks(pkb_root: &Path, server: &PkbSearchServer) {
        for (id, body) in [("t1", ""), ("t2", ""), ("t3", "See [[t2]].\n")] {
            std::fs::write(
                pkb_root.join(format!("{id}.md")),
                format!(
                    "---\nid: {id}\ntitle: Task {id}\ntype: task\nstatus: active\n\
                     parent: test-project\n---\n\n{body}"
                ),
            )
            .unwrap();
        }
        let mut graph = server.graph.write();
        *graph = GraphStore::build_from_directory(pkb_root);
    }

    #[test]
    fn test_batch_and_merge_dry_run_write_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let server = build_disk_server(pkb_root);

        // Create tasks to update
        let task_ids = vec!["t1".to_string(), "t2".to_string()];
        for id in &task_ids {
            std::fs::write(
                pkb_root.join(format!("{}.md", id)),
                format!("---\nid: {}\ntitle: Task {}\ntype: task\nstatus: active\nparent: test-project\n---\n", id, id),
            )
            .unwrap();
        }
        // Force graph rebuild
        {
            let mut graph = server.graph.write();
            *graph = GraphStore::build_from_directory(pkb_root);
        }

        // A dry run must leave every byte on disk untouched. Asserting only on the
        // response text passes even when the write happens — that is exactly how a
        // dry-run regression shipped before (see the `--dry-run` sync defect).
        let before = tree_snapshot(pkb_root);
        assert!(
            !before.is_empty(),
            "snapshot helper found no files — the disk assertions below would be vacuous"
        );

        // Run batch update as dry-run
        let args = json!({
            "ids": task_ids,
            "updates": { "status": "blocked" },
            "dry_run": true
        });
        let result = server.handle_batch_update(&args).expect("batch_update");
        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<String>();
        assert!(
            text.contains("Pass dry_run=false to execute"),
            "expected dry-run warning in: {text}"
        );
        assert!(
            text.contains("\"dry_run\": true"),
            "expected dry_run: true in JSON: {text}"
        );
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "batch_update dry run modified files on disk"
        );

        // Run merge_node as dry-run
        let merge_args = json!({
            "canonical_id": "t1",
            "source_ids": ["t2"],
            "dry_run": true
        });
        let merge_result = server.handle_merge_node(&merge_args).expect("merge_node");
        let merge_text = merge_result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.clone()))
            .collect::<String>();
        assert!(
            merge_text.contains("Pass dry_run=false to execute"),
            "expected dry-run warning in: {merge_text}"
        );
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "merge_node dry run modified files on disk"
        );
        // The counters describe changes that did not happen, so they must not read
        // as completed writes.
        assert!(
            merge_text.contains("would be archived"),
            "dry-run counters must be phrased in the conditional: {merge_text}"
        );
        let src = std::fs::read_to_string(pkb_root.join("t2.md")).expect("t2.md");
        assert!(
            !src.contains("superseded_by"),
            "merge_node dry run archived the source node: {src}"
        );
    }

    /// Every MCP handler that takes a `dry_run` parameter, driven through both
    /// branches on its own fixture.
    ///
    /// Each case asserts two things, and the second is what makes the first
    /// worth anything:
    ///
    /// 1. with `dry_run: true`, every byte under the PKB root is unchanged;
    /// 2. the *same call* with `dry_run: false` changes something.
    ///
    /// Without (2) a dry-run test passes trivially whenever the operation
    /// matched nothing — and a test that cannot tell "declined to write" from
    /// "had nothing to write" cannot tell either of them from "wrote". That was
    /// the gap: the response text said no write had happened, and nothing
    /// checked. Response text is asserted here too, but only after disk.
    #[test]
    fn test_every_dry_run_gated_handler_writes_nothing() {
        type Handler = fn(&PkbSearchServer, &JsonValue) -> Result<CallToolResult, McpError>;

        // (label, handler, args without `dry_run`, substring the dry-run response must carry)
        let cases: Vec<(&str, Handler, JsonValue, &str)> = vec![
            (
                "batch_update",
                PkbSearchServer::handle_batch_update,
                json!({ "ids": ["t1", "t2"], "updates": { "status": "blocked", "blocker": "dry-run cover" } }),
                "\"dry_run\": true",
            ),
            (
                "batch_archive",
                PkbSearchServer::handle_batch_archive,
                json!({ "ids": ["t1", "t2"], "reason": "dry-run cover" }),
                "\"dry_run\": true",
            ),
            (
                "batch_reparent",
                PkbSearchServer::handle_batch_reparent,
                json!({ "ids": ["t2"], "new_parent": "t1" }),
                "\"dry_run\": true",
            ),
            (
                "batch_reclassify",
                PkbSearchServer::handle_batch_reclassify,
                json!({ "ids": ["t2"], "new_type": "note" }),
                "\"dry_run\": true",
            ),
            (
                "batch_merge",
                PkbSearchServer::handle_batch_merge,
                json!({ "canonical": "t1", "merge_ids": ["t2"] }),
                "\"dry_run\": true",
            ),
            (
                "batch_create_epics",
                PkbSearchServer::handle_batch_create_epics,
                json!({
                    "parent": "test-project",
                    "epics": [{ "title": "Epic E", "task_ids": ["t1", "t2"] }]
                }),
                "\"dry_run\": true",
            ),
            (
                "merge_node",
                PkbSearchServer::handle_merge_node,
                json!({ "canonical_id": "t1", "source_ids": ["t2"] }),
                "nothing was written",
            ),
            (
                "apply_consolidation_batch",
                PkbSearchServer::handle_apply_consolidation_batch,
                json!({ "seed_id": "t1", "updates": { "t1": { "status": "done" }, "t2": { "status": "blocked", "blocker": "dry-run cover" } } }),
                "\"dry_run\": true",
            ),
        ];

        for (label, handler, base, marker) in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            let pkb_root = dir.path();
            let server = build_disk_server(pkb_root);
            seed_linked_tasks(pkb_root, &server);

            let before = tree_snapshot(pkb_root);
            assert!(
                before.len() >= 4,
                "{label}: fixture is missing files, so the disk assertions below \
                 would be vacuous; snapshot held {}",
                before.len()
            );

            let mut dry_args = base.clone();
            dry_args["dry_run"] = json!(true);
            let dry = handler(&server, &dry_args)
                .unwrap_or_else(|e| panic!("{label} dry run returned an error: {e:?}"));

            assert_eq!(
                tree_snapshot(pkb_root),
                before,
                "{label} with dry_run=true modified files on disk"
            );

            let dry_text = response_text(&dry);
            assert!(
                dry_text.contains(marker),
                "{label} dry-run response is missing {marker:?}: {dry_text}"
            );

            // Vacuity guard: prove the operation had a write to decline.
            let mut live_args = base;
            live_args["dry_run"] = json!(false);
            handler(&server, &live_args)
                .unwrap_or_else(|e| panic!("{label} live run returned an error: {e:?}"));

            assert_ne!(
                tree_snapshot(pkb_root),
                before,
                "{label} wrote nothing even with dry_run=false — the dry_run=true \
                 assertion above proved nothing about the gate"
            );
        }
    }

    // ── mem_8035b002: batch_merge must record the merge via the canonical
    // node's own `supersedes` list, not a hand-written `superseded_by` on
    // the merged-away source (superseded_by is a computed reverse index).

    #[test]
    fn test_batch_merge_records_supersedes_on_canonical_not_superseded_by_on_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        let server = build_disk_server(pkb_root);

        std::fs::write(
            pkb_root.join("task-canon.md"),
            "---\nid: task-canon\ntitle: Canonical\ntype: task\nstatus: ready\nparent: test-project\n---\n\n# Canonical\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("task-dup.md"),
            "---\nid: task-dup\ntitle: Duplicate\ntype: task\nstatus: ready\nparent: test-project\n---\n\n# Duplicate\n",
        )
        .unwrap();
        {
            let mut graph = server.graph.write();
            *graph = GraphStore::build_from_directory(pkb_root);
        }

        server
            .handle_batch_merge(&json!({
                "canonical": "task-canon",
                "merge_ids": ["task-dup"],
                "dry_run": false,
            }))
            .unwrap_or_else(|e| panic!("batch_merge failed: {e:?}"));

        let canon_disk = std::fs::read_to_string(pkb_root.join("task-canon.md")).unwrap();
        assert!(
            canon_disk.contains("supersedes") && canon_disk.contains("task-dup"),
            "canonical must record the merge via its own supersedes list, got:\n{canon_disk}"
        );

        let dup_disk = std::fs::read_to_string(pkb_root.join("task-dup.md")).unwrap();
        assert!(
            !dup_disk.contains("superseded_by"),
            "merged-away source must NOT carry a hand-written superseded_by, got:\n{dup_disk}"
        );
        assert!(dup_disk.contains("status: done"));
    }
}

