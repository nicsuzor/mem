use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::graph_store::GraphStore;

use super::{PkbSearchServer, MAX_RESULTS};

impl PkbSearchServer {
    pub(crate) fn handle_task_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;

        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(MAX_RESULTS);
        let include_subtasks = args
            .get("include_subtasks")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // By default `task_search` hides done/cancelled tasks — when the user
        // is searching for work to do, completed work is noise. Set
        // `include_done=true` (or `include_closed=true`) to opt back in.
        let include_done = args
            .get("include_done")
            .and_then(|v| v.as_bool())
            .or_else(|| args.get("include_closed").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        // Optional `type` filter: either a single type ("epic") or a comma-
        // separated list ("epic,feature"). When set, only matching types are
        // returned. Recognised actionable types: project, epic, task, learn.
        let type_filter: Option<HashSet<String>> =
            args.get("type").and_then(|v| v.as_str()).map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            });

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let store = self.store.read();
        // When a type filter is present (or done tasks are excluded) fetch
        // more candidates so we still fill the limit after filtering.
        let since = args.get("since").and_then(|v| v.as_str());
        let before = args.get("before").and_then(|v| v.as_str());

        let fetch_limit = if type_filter.is_some() || !include_done {
            limit * 10
        } else {
            limit * 3
        };
        let results = store.search(&query_embedding, fetch_limit, &self.pkb_root, since, before, None);

        let graph = self.graph.read();

        // Filter to actionable, in-scope candidates first (order not yet final —
        // store.search only sorted by raw semantic score, which has no title-match
        // term). Re-scoring + re-sorting by title_boost below mirrors the fix in
        // handle_pkb_search so a near-exact title match isn't buried behind
        // higher-raw-score-but-tangential tasks (task_1542e818).
        let mut candidates: Vec<(&crate::vectordb::SearchResult, f32)> = Vec::new();

        for r in &results {
            let is_target = r
                .doc_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("target"))
                .unwrap_or(false);
            let explicit_target = is_target
                && type_filter
                    .as_ref()
                    .map(|f| f.contains("target"))
                    .unwrap_or(false);

            let is_task = r
                .doc_type
                .as_deref()
                .map(|t| crate::graph_store::ACTIONABLE_TYPES.contains(&t))
                .unwrap_or(false)
                || explicit_target;

            if !is_task {
                continue;
            }
            let is_subtask = r
                .doc_type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("subtask"))
                .unwrap_or(false);
            let subtask_allowed = include_subtasks
                || type_filter
                    .as_ref()
                    .map(|f| f.contains("subtask"))
                    .unwrap_or(false);
            if is_subtask && !subtask_allowed {
                continue;
            }
            if let Some(ref filter) = type_filter {
                let matches = r
                    .doc_type
                    .as_deref()
                    .map(|t| filter.iter().any(|f| t.eq_ignore_ascii_case(f)))
                    .unwrap_or(false);
                if !matches {
                    continue;
                }
            }

            // Closed-status filter (default on): hide done/cancelled unless the
            // caller explicitly asked for them via include_done.
            if !include_done {
                if let Some(node) = graph.get_node(&r.id) {
                    if let Some(status) = node.status.as_deref() {
                        let s = status.to_ascii_lowercase();
                        if s == "done" || s == "cancelled" || s == "canceled" {
                            continue;
                        }
                    }
                }
            }

            candidates.push((r, Self::task_search_score(query, r)));
        }

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut output = String::new();
        let mut count = 0;

        for (r, score) in candidates.into_iter().take(limit) {
            count += 1;
            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                count, r.title, score
            ));

            // Direct ID lookup in the graph
            if let Some(node) = graph.get_node(&r.id) {
                let id = node.task_id.as_deref().unwrap_or(&node.id);
                output.push_str(&format!("**ID:** `{id}`\n"));
                if let Some(ref s) = node.status {
                    output.push_str(&format!("**Status:** {s}\n"));
                }
                if let Some(p) = node.priority {
                    output.push_str(&format!("**Priority:** {p}\n"));
                }
                if !node.blocks.is_empty() {
                    output.push_str(&format!("**Blocks:** {}\n", node.blocks.join(", ")));
                }
                if !node.depends_on.is_empty() {
                    output.push_str(&format!("**Depends on:** {}\n", node.depends_on.join(", ")));
                }
                if node.uncertainty > 0.0 || node.criticality > 0.0 || node.scope > 0 {
                    output.push_str(&format!(
                        "**Metrics:** scope={} uncertainty={:.2} criticality={:.2}\n",
                        node.scope, node.uncertainty, node.criticality
                    ));
                }
            }
            output.push('\n');
        }

        if count == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                "No tasks found matching query.",
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "**Found {count} tasks for:** \"{query}\"\n\n{output}"
        ))]))
    }

    /// Weight applied to the overlap-coefficient ratio in [`Self::title_match_boost`].
    /// Calibrated so a near-exact title match (overlap coefficient ~0.3-0.45
    /// against a realistic multi-word query with title boilerplate) outscores
    /// the existing 0.15 `type_boost` even when the type-boosted competitor has
    /// a higher raw similarity score (see task_1542e818 for the motivating case
    /// and derivation).
    const TITLE_BOOST_WEIGHT: f32 = 2.0;

    /// Minimal stopword list for title/query token overlap. Deliberately short —
    /// this only needs to strip the connective words common in both task titles
    /// and search queries, not do full NLP.
    const TITLE_BOOST_STOPWORDS: &'static [&'static str] = &[
        "a", "an", "and", "at", "for", "in", "is", "of", "on", "or", "the", "to", "with",
    ];

    /// Token-overlap boost rewarding queries that are a near-exact match to a
    /// document's title. Without this, a title-level match sits in the same
    /// score band as tangentially-related body hits (see task_1542e818) — there
    /// was previously no title-match term in the ranking formula at all.
    ///
    /// Returns a boost in `[0.0, TITLE_BOOST_WEIGHT]`: the overlap coefficient
    /// (`|query_tokens ∩ title_tokens| / min(|query_tokens|, |title_tokens|)`)
    /// of lowercased, stopword-filtered word tokens between `query` and `title`,
    /// scaled by `TITLE_BOOST_WEIGHT`. Composes as an additive term alongside
    /// `boost`/`type_boost` in the existing
    /// `score * (1.0 + boost + type_boost + title_boost)` formula.
    ///
    /// Overlap coefficient (not Jaccard) deliberately: task/document titles
    /// routinely carry boilerplate irrelevant to the query — due-dates,
    /// parenthetical codes, IDs — that inflates a Jaccard union and dilutes the
    /// signal without being wrong. Dividing by the *smaller* token set instead
    /// asks "is (most of) the query's content contained in the title?", which
    /// survives that boilerplate. See task_1542e818 for the motivating case
    /// where Jaccard proved too weak to overcome PR #406's `type_boost`.
    pub(crate) fn title_match_boost(query: &str, title: &str) -> f32 {
        fn tokens(s: &str) -> HashSet<String> {
            s.to_ascii_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 1 && !PkbSearchServer::TITLE_BOOST_STOPWORDS.contains(w))
                .map(|w| w.to_string())
                .collect()
        }

        let q = tokens(query);
        let t = tokens(title);
        if q.is_empty() || t.is_empty() {
            return 0.0;
        }

        let intersection = q.intersection(&t).count() as f32;
        let smaller = q.len().min(t.len()) as f32;

        (intersection / smaller) * Self::TITLE_BOOST_WEIGHT
    }

    /// `type_boost` for `search`'s type-based ranking bump (PR #406): surfaces
    /// knowledge/memory/note/insight/observation docs over transient tasks,
    /// absent a title match. Deliberately does not fire for tasks — see
    /// [`Self::ranked_score`] for how it composes with `title_match_boost`.
    pub(crate) fn type_boost_for(doc_type: Option<&str>) -> f32 {
        match doc_type.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("knowledge") | Some("memory") | Some("note") | Some("insight")
            | Some("observation") => 0.15,
            _ => 0.0,
        }
    }

    /// The `search` tool's per-result ranking score: raw semantic `r.score`,
    /// rescaled by graph-proximity `boost`, `type_boost` (knowledge/memory
    /// surfacing, PR #406) and `title_boost` (near-exact title match,
    /// task_1542e818), plus a small confidence nudge. Pulled out as a pure
    /// function so both the handler and its tests share one formula.
    pub(crate) fn ranked_score(query: &str, r: &crate::vectordb::SearchResult, boost: f32, confidence: f32) -> f32 {
        let type_boost = Self::type_boost_for(r.doc_type.as_deref());
        let title_boost = Self::title_match_boost(query, &r.title);
        r.score * (1.0 + boost + type_boost + title_boost) + (confidence * 0.05)
    }

    /// The `task_search` tool's per-result ranking score. Simpler than
    /// [`Self::ranked_score`]: `task_search` only ever surfaces actionable types
    /// (tasks/epics/learn), so `type_boost` never applies here — but it shares
    /// the same `title_boost` fix (task_1542e818), since `task_search` previously
    /// had no title-match term either and relied entirely on `store.search`'s
    /// pre-sort order.
    pub(crate) fn task_search_score(query: &str, r: &crate::vectordb::SearchResult) -> f32 {
        r.score * (1.0 + Self::title_match_boost(query, &r.title))
    }

    pub(crate) fn handle_pkb_search(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: query"),
                data: None,
            })?;

        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(MAX_RESULTS);

        let boost_id = args.get("boost_id").and_then(|v| v.as_str());
        let detail = args
            .get("detail")
            .and_then(|v| v.as_str())
            .unwrap_or("chunk");

        let query_embedding = self.embedder.encode_query(query).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Embedding error: {e}")),
            data: None,
        })?;

        let since = args.get("since").and_then(|v| v.as_str());
        let before = args.get("before").and_then(|v| v.as_str());
        let doc_type = args.get("type").and_then(|v| v.as_str());

        let store = self.store.read();
        let fetch_limit = if doc_type.is_some() {
            limit * 10
        } else {
            limit * 2
        };
        let results = store.search(
            &query_embedding,
            fetch_limit,
            &self.pkb_root,
            since,
            before,
            doc_type,
        );

        // Build proximity boost map if boost_id provided
        let boost_map: std::collections::HashMap<String, f32> = if let Some(bid) = boost_id {
            let graph = self.graph.read();
            if let Some(node) = graph.resolve(bid) {
                let node_id = node.id.clone();
                graph
                    .ego_subgraph(&node_id, 3)
                    .into_iter()
                    .map(|(nid, dist)| (nid, 0.3 / dist as f32))
                    .collect()
            } else {
                std::collections::HashMap::new()
            }
        } else {
            std::collections::HashMap::new()
        };

        // Score and sort results
        let graph = self.graph.read();

        let mut scored: Vec<_> = results
            .iter()
            .map(|r| {
                let node = graph.get_node(&r.id);
                let node_id = node.as_ref().map(|n| n.id.clone());
                let confidence = node.as_ref().and_then(|n| n.confidence).unwrap_or(0.5) as f32;

                let boost = *node_id
                    .as_ref()
                    .and_then(|nid| boost_map.get(nid))
                    .unwrap_or(&0.0);

                (r, Self::ranked_score(query, r, boost, confidence))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No results found.",
            )]));
        }

        let mut output = format!(
            "**Found {} results for:** \"{}\"{}\n\n",
            scored.len(),
            query,
            if boost_id.is_some() {
                " (with graph proximity boost)"
            } else {
                ""
            }
        );

        for (i, (r, score)) in scored.iter().enumerate() {
            let node = graph.get_node(&r.id);
            let display_id = node
                .map(|n| n.task_id.as_deref().unwrap_or(&n.id))
                .unwrap_or(&r.id);
            let node_type = r
                .doc_type
                .as_deref()
                .or_else(|| node.and_then(|n| n.node_type.as_deref()))
                .unwrap_or("untyped");
            let is_task = crate::graph::TASK_TYPES.contains(&node_type);

            output.push_str(&format!(
                "### {}. {} (score: {:.3})\n",
                i + 1,
                r.title,
                score
            ));
            output.push_str(&format!("**ID:** `{display_id}`\n"));
            output.push_str(&format!("**Type:** {node_type}\n"));
            // Only actionable task nodes carry actionable task status (AC3 / aops_9d3be3b3)
            if is_task {
                if let Some(status) = node.and_then(|n| n.status.as_deref()) {
                    output.push_str(&format!("**Status:** {status}\n"));
                }
            }
            if let Some(project) = node.and_then(|n| n.project.as_deref()) {
                output.push_str(&format!("**Project:** {project}\n"));
            }
            if !r.tags.is_empty() {
                output.push_str(&format!("**Tags:** {}\n", r.tags.join(", ")));
            }
            let extract: std::borrow::Cow<'_, str> = match detail {
                "snippet" => std::borrow::Cow::Borrowed(&r.snippet),
                "full" => {
                    // Read full document from disk
                    let abs_path = node
                        .map(|n| self.abs_path(&n.path))
                        .unwrap_or_else(|| r.path.clone());
                    match std::fs::read_to_string(&abs_path) {
                        Ok(content) => std::borrow::Cow::Owned(content),
                        Err(_) => std::borrow::Cow::Borrowed(&r.chunk_text),
                    }
                }
                _ => std::borrow::Cow::Borrowed(&r.chunk_text), // "chunk" (default)
            };
            if !extract.is_empty() {
                output.push_str(&format!("\n> {}\n", extract.replace('\n', "\n> ")));
            }
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_pkb_trace(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: from"),
                data: None,
            })?;

        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: to"),
                data: None,
            })?;

        let max_paths = args.get("max_paths").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

        let graph = self.graph.read();

        let from_node = graph.resolve(from).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Source node not found: {from}")),
            data: None,
        })?;
        let from_id = from_node.id.clone();
        let from_label = from_node.label.clone();

        let to_node = graph.resolve(to).ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Target node not found: {to}")),
            data: None,
        })?;
        let to_id = to_node.id.clone();
        let to_label = to_node.label.clone();

        let paths = graph.all_shortest_paths(&from_id, &to_id, max_paths);

        if paths.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No path found between `{from_id}` ({from_label}) and `{to_id}` ({to_label})."
            ))]));
        }

        let mut output = format!(
            "## Paths from `{from_id}` to `{to_id}`\n\n**{} path(s) found** (length: {} hops)\n\n",
            paths.len(),
            paths[0].len() - 1
        );

        for (i, path) in paths.iter().enumerate() {
            output.push_str(&format!("### Path {}\n", i + 1));
            for (j, nid) in path.iter().enumerate() {
                let label = graph.get_node(nid).map(|n| n.label.as_str()).unwrap_or("?");
                let prefix = if j == 0 { "  " } else { "  → " };
                output.push_str(&format!("{prefix}`{nid}` ({label})\n"));
            }
            output.push('\n');
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_pkb_orphans(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let include_all = args
            .get("include_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let type_filter: Option<Vec<String>> =
            args.get("types").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let graph = self.graph.read();
        let mut orphans = graph.orphans();
        let mut lint_warnings: Vec<&crate::graph::GraphNode> = graph
            .nodes()
            .filter(|n| !n.parse_warnings.is_empty())
            .collect();

        // Filter by type if requested
        if let Some(ref types) = type_filter {
            let matches_type = |n: &&crate::graph::GraphNode| {
                n.node_type
                    .as_deref()
                    .map(|t| types.iter().any(|f| f.eq_ignore_ascii_case(t)))
                    .unwrap_or(false)
            };
            orphans.retain(matches_type);
            lint_warnings.retain(matches_type);
        } else if !include_all {
            // Default: only show actionable types and exclude completed nodes
            // — matches graph_stats orphan_count definition
            let matches_default = |n: &&crate::graph::GraphNode| {
                let is_actionable = n
                    .node_type
                    .as_deref()
                    .map(|t| crate::graph_store::ACTIONABLE_TYPES.contains(&t))
                    .unwrap_or(false);
                let is_completed = crate::graph::is_completed(n.status.as_deref());
                is_actionable && !is_completed
            };
            orphans.retain(matches_default);
            lint_warnings.retain(matches_default);
        }

        if orphans.is_empty() && lint_warnings.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No orphan nodes or lint warnings found.",
            )]));
        }

        let mut output = String::new();

        if !orphans.is_empty() {
            // Sort by label for consistent output
            orphans.sort_by(|a, b| a.label.cmp(&b.label));

            let max = match limit {
                None | Some(0) => orphans.len(),
                Some(n) => n,
            };
            let total = orphans.len();
            let showing = total.min(max);

            let type_desc = type_filter
                .as_ref()
                .map(|t| format!(" (types: {})", t.join(", ")))
                .unwrap_or_default();

            output.push_str(&format!(
                "**{total} orphan nodes{type_desc}** (showing {showing})\n\nThese nodes have no valid parent.\n\n"
            ));

            for node in orphans.iter().take(max) {
                output.push_str(&format!("- **{}**", node.label));
                if let Some(ref t) = node.node_type {
                    output.push_str(&format!(" [{t}]"));
                }
                // Project status (AC2) so callers can tell open orphans from closed.
                output.push_str(&format!(" [{}]", node.status.as_deref().unwrap_or("-")));
                let cid = node.task_id.as_deref().unwrap_or(&node.id);
                output.push_str(&format!(" — `{cid}`\n"));
            }

            if total > max {
                output.push_str(&format!("\n...and {} more\n", total - max));
            }
            output.push_str("\n");
        }

        if !lint_warnings.is_empty() {
            lint_warnings.sort_by(|a, b| a.label.cmp(&b.label));

            let max = match limit {
                None | Some(0) => lint_warnings.len(),
                Some(n) => n,
            };
            let total = lint_warnings.len();
            let showing = total.min(max);

            output.push_str(&format!(
                "**{total} nodes with lint warnings** (showing {showing})\n\n"
            ));

            for node in lint_warnings.iter().take(max) {
                output.push_str(&format!("- **{}**", node.label));
                let cid = node.task_id.as_deref().unwrap_or(&node.id);
                output.push_str(&format!(" — `{cid}`\n"));
                for w in &node.parse_warnings {
                    output.push_str(&format!("  - [{}] {}\n", w.field, w.message));
                }
            }

            if total > max {
                output.push_str(&format!("\n...and {} more\n", total - max));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_get_semantic_neighbors(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: id"),
                data: None,
            })?;

        let threshold = args
            .get("threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.85);
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(MAX_RESULTS);

        let graph = self.graph.read();
        let store = self.store.read();

        let neighbors =
            crate::batch_ops::similarity::find_neighbors(id, &graph, &store, threshold, limit);

        if neighbors.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No semantic neighbors found for '{}' above threshold {:.2}.",
                id, threshold
            ))]));
        }

        let mut output = format!(
            "## Semantic Neighbors for `{id}` (threshold {:.2})\n\n**{} neighbor(s) found**\n\n",
            threshold,
            neighbors.len()
        );

        for n in neighbors {
            let edge = if n.is_explicit_edge {
                " (explicitly linked)"
            } else {
                ""
            };
            output.push_str(&format!(
                "- **{}** (`{}`): score {:.3}{}\n",
                n.title, n.id, n.score, edge
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_search_by_tag(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: tags"),
                data: None,
            })?;

        if tags.is_empty() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("tags array cannot be empty"),
                data: None,
            });
        }

        let type_filter = args.get("type").and_then(|v| v.as_str());
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize).min(MAX_RESULTS);

        let store = self.store.read();
        let all = store.list_documents(None, type_filter, None, &self.pkb_root);

        let mut matching: Vec<_> = all
            .into_iter()
            .filter(|r| {
                tags.iter()
                    .all(|tag| r.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
            })
            .collect();
        matching.truncate(limit);

        if matching.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No documents found with tags: {}",
                tags.join(", ")
            ))]));
        }

        let total = matching.len();
        let mut output = format!("**{total} documents with tags [{}]**\n\n", tags.join(", "));

        for r in &matching {
            output.push_str(&format!("- **{}**", r.title));
            if let Some(ref dt) = r.doc_type {
                output.push_str(&format!(" [{dt}]"));
            }
            output.push_str(&format!(" ({})", r.tags.join(", ")));
            let id = &r.id;
            output.push_str(&format!(" — `{id}`\n"));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    pub(crate) fn handle_find_duplicates(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        let filters = crate::batch_ops::filters::parse_filter_set(args);
        let mode_str = args.get("mode").and_then(|v| v.as_str()).unwrap_or("both");
        let mode = crate::batch_ops::duplicates::DuplicateMode::from_str(mode_str);
        let title_threshold = args
            .get("title_threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);
        let semantic_threshold = args
            .get("similarity_threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.85);
        let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize).min(MAX_RESULTS);

        let graph = self.graph.read();
        let store = self.store.read();
        let mut report = crate::batch_ops::duplicates::find_duplicates(
            &graph,
            &store,
            &filters,
            mode,
            title_threshold,
            semantic_threshold,
            &self.pkb_root,
        );
        drop(graph);
        drop(store);

        // Cap the number of returned clusters (output only — the O(n²) pairwise
        // comparison above already ran; `total_clusters`/`total_duplicates` still
        // report the true totals so a caller can tell truncation happened).
        report.clusters.truncate(limit);

        let json = serde_json::to_string_pretty(&report).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

}

// ===========================================================================

/// Regression coverage for task_1542e818: `search`/`task_search` previously had
/// no title-match term at all, so a near-exact title match could sit in the
/// same tight score band as tangential body hits (or lose outright to the
/// PR #406 `type_boost` given to knowledge/memory docs). These tests exercise
/// the exact pure functions (`title_match_boost`, `ranked_score`,
/// `task_search_score`) the handlers call — no ONNX/store round-trip needed,
/// since the dummy test embedder returns zero vectors and would collapse the
/// multiplicative formula to zero regardless of the fix.
#[cfg(test)]
mod title_match_ranking_tests {
    use super::*;
    use crate::vectordb::SearchResult;
    use std::path::PathBuf;

    fn result(title: &str, score: f32, doc_type: Option<&str>) -> SearchResult {
        SearchResult {
            path: PathBuf::new(),
            title: title.to_string(),
            score,
            snippet: String::new(),
            chunk_text: String::new(),
            id: title.to_string(),
            date: None,
            doc_type: doc_type.map(|s| s.to_string()),
            status: None,
            tags: vec![],
            confidence: None,
        }
    }

    #[test]
    fn title_match_boost_is_max_for_exact_match() {
        let boost = PkbSearchServer::title_match_boost(
            "write reference for Jo Gray",
            "Write Reference For Jo Gray",
        );
        assert!(
            (boost - PkbSearchServer::TITLE_BOOST_WEIGHT).abs() < 1e-6,
            "case/whitespace-only differences must still score as an exact match, got {boost}"
        );
    }

    #[test]
    fn title_match_boost_is_zero_for_disjoint_titles() {
        let boost = PkbSearchServer::title_match_boost(
            "write reference for Jo Gray promotion",
            "Quarterly infrastructure budget review",
        );
        assert_eq!(boost, 0.0, "completely unrelated titles must get no boost");
    }

    #[test]
    fn title_match_boost_is_zero_when_either_side_has_no_content_words() {
        // Only stopwords on the query side after filtering.
        let boost = PkbSearchServer::title_match_boost("the a of", "Write Reference For Jo Gray");
        assert_eq!(boost, 0.0);
    }

    #[test]
    fn title_match_boost_scales_with_partial_overlap() {
        let strong = PkbSearchServer::title_match_boost(
            "write reference for Jo Gray promotion university of sydney",
            "Referee report for Dr Joanne Gray promotion (Univ Sydney)",
        );
        let weak = PkbSearchServer::title_match_boost(
            "write reference for Jo Gray promotion university of sydney",
            "Jodie Siganto Paper Collaboration (2013)",
        );
        assert!(
            strong > weak,
            "a title sharing 'gray'/'promotion'/'sydney' must score higher than one sharing nothing relevant (strong={strong}, weak={weak})"
        );
        assert!(strong > 0.0 && strong < PkbSearchServer::TITLE_BOOST_WEIGHT);
        assert_eq!(weak, 0.0);
    }

    /// The motivating case from task_1542e818, reconstructed from real production
    /// evidence gathered 2026-07-17 against the live PKB MCP server (pkb 0.3.73):
    /// for the query "write reference for Jo Gray promotion university of sydney",
    /// `search` ranked a memory node (mem_353bbbe6, boosted by `type_boost`) at
    /// #1 and the actionable task (task_764ea48c) at #4, behind a second,
    /// less-precisely-titled task (task_102e721f). Raw (pre-`title_boost`) scores
    /// below are back-solved from the live displayed scores assuming the default
    /// confidence of 0.5: mem_353bbbe6 raw≈0.620 (displayed 0.738 incl. its 0.15
    /// type_boost), task_764ea48c raw≈0.644 (displayed 0.669), task_102e721f
    /// raw≈0.667 (displayed 0.692). Without `title_boost` this test fails exactly
    /// as production did (task_764ea48c is neither #1 nor even #2).
    #[test]
    fn ranked_score_surfaces_near_exact_title_match_ahead_of_type_boosted_and_higher_raw_score_rivals()
    {
        let query = "write reference for Jo Gray promotion university of sydney";

        let target = result(
            "Referee report for Dr Joanne Gray promotion (Univ Sydney) — due COB Thu 16 Jul",
            0.644,
            Some("task"),
        );
        let type_boosted_memory = result(
            "Referee letter draft — Dr Joanne Gray promotion to Level D (USyd)",
            0.620,
            Some("memory"),
        );
        let higher_raw_score_task = result(
            "Support Joanne Gray USyd out-of-round promotion",
            0.667,
            Some("task"),
        );

        let target_score = PkbSearchServer::ranked_score(query, &target, 0.0, 0.5);
        let memory_score = PkbSearchServer::ranked_score(query, &type_boosted_memory, 0.0, 0.5);
        let rival_score = PkbSearchServer::ranked_score(query, &higher_raw_score_task, 0.0, 0.5);

        assert!(
            target_score > memory_score,
            "near-exact title match (task_764ea48c, {target_score}) must outrank the type_boosted memory note ({memory_score})"
        );
        assert!(
            target_score > rival_score,
            "near-exact title match (task_764ea48c, {target_score}) must outrank the higher-raw-score-but-less-precisely-titled rival ({rival_score})"
        );
    }

    /// PR #406's original purpose must survive this change: absent a title
    /// match, a knowledge/memory-type doc still outranks a same-raw-score task.
    #[test]
    fn ranked_score_preserves_type_boost_when_no_title_match() {
        let query = "quarterly infrastructure budget review";

        let memory_doc = result("Notes on cloud spend patterns", 0.5, Some("memory"));
        let task_doc = result("Fix the flaky deploy pipeline", 0.5, Some("task"));

        let memory_score = PkbSearchServer::ranked_score(query, &memory_doc, 0.0, 0.5);
        let task_score = PkbSearchServer::ranked_score(query, &task_doc, 0.0, 0.5);

        assert!(
            memory_score > task_score,
            "absent any title match, type_boost must still surface the memory doc ({memory_score}) over the task ({task_score})"
        );
        // Exactly the pre-existing 15% multiplicative gap — confirms title_boost
        // contributed nothing here (both titles are irrelevant to the query).
        assert!((memory_score - task_score - 0.5 * 0.15).abs() < 1e-5);
    }

    /// `task_search` has its own scoring path (no type_boost — only actionable
    /// types ever reach it) but shared the same missing-title-term root cause.
    /// Confirms `task_search_score` reorders the same three candidates as
    /// `ranked_score` does above, using task_search's live raw scores
    /// (2026-07-17): task_102e721f 0.667, task_1542e818 0.656 (the bug-report
    /// task itself, which quotes the query verbatim in its body and so scores
    /// high on raw chunk similarity despite an unrelated title), task_764ea48c
    /// 0.644. Without title_boost, task_764ea48c is buried in 3rd.
    #[test]
    fn task_search_score_surfaces_near_exact_title_match_first() {
        let query = "write reference for Jo Gray promotion university of sydney";

        let target = result(
            "Referee report for Dr Joanne Gray promotion (Univ Sydney) — due COB Thu 16 Jul",
            0.644,
            Some("task"),
        );
        let higher_raw_score_rival = result(
            "Support Joanne Gray USyd out-of-round promotion",
            0.667,
            Some("task"),
        );
        let keyword_stuffed_unrelated_title = result(
            "PKB search ranking: no discriminative boost for exact/near-title matches",
            0.656,
            Some("task"),
        );

        let target_score = PkbSearchServer::task_search_score(query, &target);
        let rival_score = PkbSearchServer::task_search_score(query, &higher_raw_score_rival);
        let unrelated_score =
            PkbSearchServer::task_search_score(query, &keyword_stuffed_unrelated_title);

        assert!(target_score > rival_score, "{target_score} vs {rival_score}");
        assert!(
            target_score > unrelated_score,
            "{target_score} vs {unrelated_score}"
        );
    }
}



/// AC2 for the semantic-search surfaces (`search`, `task_search`): every task/
/// node hit must carry a `status` field in the DEFAULT projection. These methods
/// are exempt from AC1's focus_score reordering (their intrinsic sort is semantic
/// relevance — reordering by focus_score would defeat retrieval), but status
/// projection is required regardless. Driven in-process with a dummy embedder so
/// the assertions run without the ONNX model: tasks are created through the real
/// handler (which seeds the vector store), then searched through the real handler.
#[cfg(test)]
mod search_status_projection_tests {
    use super::*;
    use crate::embeddings::{Embedder, EMBEDDING_DIM};
    use crate::graph_store::GraphStore;
    use crate::vectordb::VectorStore;
    use std::path::Path;
    use std::sync::Arc;

    fn build_seeded_server(pkb_root: &Path) -> PkbSearchServer {
        std::fs::create_dir_all(pkb_root.join("projects")).unwrap();
        std::fs::write(
            pkb_root.join("polecat.yaml"),
            "projects:\n  p: {}\n  aops: {}\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("projects/p.md"),
            "---\nid: p\ntitle: P\ntype: project\nstatus: active\n---\n\n# P\n",
        )
        .unwrap();

        let store = VectorStore::new(EMBEDDING_DIM);
        let embedder = Embedder::new_dummy();
        let graph = GraphStore::build_from_directory(pkb_root);
        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            pkb_root.to_path_buf(),
            pkb_root.join("test-index.bin"),
            Arc::new(RwLock::new(graph)),
        );

        // Create searchable tasks
        for (title, body) in [
            (
                "Alpha widget task",
                "This task is about the alpha widget subsystem and retrieval.",
            ),
            (
                "Beta widget task",
                "This task concerns the beta widget subsystem and retrieval.",
            ),
        ] {
            server
                .handle_create_task(&serde_json::json!({
                    "title": title,
                    "body": body,
                    "parent": "p",
                    "project": "p",
                }))
                .expect("create_task");
        }

        // Create a non-task note document (with status: ready in frontmatter to test status suppression)
        let note_path = pkb_root.join("notes").join("widget-note.md");
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        std::fs::write(
            &note_path,
            "---\nid: note-widget-1\ntitle: \"Widget architecture note\"\ntype: note\nstatus: ready\ntags:\n  - widget\n---\n\nThis note documents the widget architecture and retrieval subsystem.",
        ).unwrap();

        // Create a template document
        let tmpl_path = pkb_root.join("templates").join("widget-tmpl.md");
        std::fs::create_dir_all(tmpl_path.parent().unwrap()).unwrap();
        std::fs::write(
            &tmpl_path,
            "---\nid: tmpl-widget-1\ntitle: \"Widget creation template\"\ntype: template\ntags:\n  - widget\n---\n\nTemplate for creating widget subsystem retrieval components.",
        ).unwrap();

        // Insert into vector store & graph
        let note_doc = crate::pkb::parse_file(&note_path).unwrap();
        let tmpl_doc = crate::pkb::parse_file(&tmpl_path).unwrap();
        let emb = server.embedder.encode_query("widget architecture note retrieval").unwrap();
        server.store.write().insert_precomputed(&note_doc, vec![note_doc.body.clone()], vec![emb.clone()]);
        server.store.write().insert_precomputed(&tmpl_doc, vec![tmpl_doc.body.clone()], vec![emb]);
        server.graph.write().replace_node(crate::graph::GraphNode::from_pkb_document(&note_doc));
        server.graph.write().replace_node(crate::graph::GraphNode::from_pkb_document(&tmpl_doc));

        server
    }

    fn result_text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<String>()
    }

    #[test]
    fn test_task_search_projects_status_on_hits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_task_search(&serde_json::json!({"query": "widget retrieval", "limit": 5}))
            .expect("task_search");
        let text = result_text(&result);

        assert!(
            !text.contains("No tasks found"),
            "seeded tasks should be retrievable; got: {text}"
        );
        // Every hit heading is followed by a Status line in the default projection.
        let hit_count = text.matches("**ID:**").count();
        let status_count = text.matches("**Status:**").count();
        assert!(
            hit_count >= 1,
            "expected at least one hit, got text: {text}"
        );
        assert_eq!(
            status_count, hit_count,
            "every task_search hit must project status (hits={hit_count}, status lines={status_count}). Text:\n{text}"
        );
    }

    #[test]
    fn test_search_projects_status_on_task_hits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget retrieval", "type": "task", "limit": 5}))
            .expect("search");
        let text = result_text(&result);

        assert!(
            !text.contains("No results found"),
            "seeded tasks should surface in search; got: {text}"
        );
        // The seeded task nodes carry status=inbox by default — it must be projected for tasks.
        assert!(
            text.contains("**Status:**"),
            "search must project status for task hits. Text:\n{text}"
        );
    }

    #[test]
    fn test_search_type_filter_task_only_returns_no_non_tasks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget", "type": "task", "limit": 10}))
            .expect("search");
        let text = result_text(&result);

        assert!(!text.contains("No results found"));
        assert!(text.contains("**Type:** task"));
        assert!(!text.contains("**Type:** note"), "task-only search must not return note");
        assert!(!text.contains("**Type:** template"), "task-only search must not return template");
    }

    #[test]
    fn test_search_type_filter_negation_excludes_tasks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget", "type": "!task", "limit": 10}))
            .expect("search");
        let text = result_text(&result);

        assert!(!text.contains("No results found"));
        assert!(!text.contains("**Type:** task"), "!task search must exclude tasks");
        assert!(text.contains("**Type:** note") || text.contains("**Type:** template"));
    }

    #[test]
    fn test_search_type_filter_template_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget", "type": "template", "limit": 10}))
            .expect("search");
        let text = result_text(&result);

        assert!(!text.contains("No results found"));
        assert!(text.contains("**Type:** template"));
        assert!(!text.contains("**Type:** task"));
        assert!(!text.contains("**Type:** note"));
    }

    #[test]
    fn test_search_type_filter_unknown_type_returns_no_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget", "type": "nonexistent_xyz", "limit": 10}))
            .expect("search");
        let text = result_text(&result);

        assert!(text.contains("No results found."));
    }

    #[test]
    fn test_search_without_type_filter_returns_all_types() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget", "limit": 10}))
            .expect("search");
        let text = result_text(&result);

        assert!(!text.contains("No results found"));
        assert!(text.contains("**Type:** task"));
        assert!(text.contains("**Type:** note"));
        assert!(text.contains("**Type:** template"));
    }

    #[test]
    fn test_search_unambiguous_type_label_and_non_task_status_suppression() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = build_seeded_server(dir.path());

        let result = server
            .handle_pkb_search(&serde_json::json!({"query": "widget architecture note", "type": "note", "limit": 5}))
            .expect("search");
        let text = result_text(&result);

        assert!(text.contains("**Type:** note"), "hit must be unambiguously labelled as note");
        // note-widget-1 had status: ready in frontmatter, but must NOT present **Status:** in search results (AC3 / aops_9d3be3b3)
        assert!(!text.contains("**Status:**"), "non-task note must not present actionable task status: {text}");
    }
}

