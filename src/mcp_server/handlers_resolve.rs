use rmcp::model::*;
use rmcp::ErrorData as McpError;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;

use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::ranking::{extract_exact_id_candidates, matches_doc_identity};

use super::PkbSearchServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ResolveState {
    Exact,
    Alias,
    Prefix,
    Title,
    Ghost,
    Deleted,
    Ambiguous,
    Unknown,
}

impl ResolveState {
    pub(crate) fn is_live(self) -> bool {
        matches!(self, Self::Exact | Self::Alias | Self::Prefix | Self::Title)
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolveCandidateSummary {
    pub(crate) id: String,
    pub(crate) title: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    pub(crate) tier: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolveEntry {
    pub(crate) query: String,
    pub(crate) state: ResolveState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tier: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) candidates: Vec<ResolveCandidateSummary>,
}

fn node_is_ghost(node: &GraphNode) -> bool {
    node.path.as_os_str().is_empty()
}

fn node_file_exists(pkb_root: &Path, node: &GraphNode) -> bool {
    if node_is_ghost(node) {
        return false;
    }
    let abs = if node.path.is_absolute() {
        node.path.clone()
    } else {
        pkb_root.join(&node.path)
    };
    abs.is_file()
}

fn summarize_node(node: &GraphNode) -> ResolveCandidateSummary {
    ResolveCandidateSummary {
        id: node.id.clone(),
        title: node.label.clone(),
        doc_type: node.node_type.clone(),
        status: node.status.clone(),
        tier: node.tier.as_str().to_string(),
    }
}

fn build_resolved_entry(
    query: &str,
    live_state: ResolveState,
    pkb_root: &Path,
    mut matches: Vec<&GraphNode>,
) -> ResolveEntry {
    matches.sort_by(|a, b| a.id.cmp(&b.id));
    matches.dedup_by(|a, b| a.id == b.id);

    if matches.len() > 1 {
        return ResolveEntry {
            query: query.to_string(),
            state: ResolveState::Ambiguous,
            id: None,
            title: None,
            doc_type: None,
            status: None,
            tier: None,
            candidates: matches.into_iter().map(summarize_node).collect(),
        };
    }

    let node = matches[0];
    let state = if node_is_ghost(node) {
        ResolveState::Ghost
    } else if !node_file_exists(pkb_root, node) {
        ResolveState::Deleted
    } else {
        live_state
    };
    ResolveEntry {
        query: query.to_string(),
        state,
        id: Some(node.id.clone()),
        title: Some(node.label.clone()),
        doc_type: node.node_type.clone(),
        status: node.status.clone(),
        tier: Some(node.tier.as_str().to_string()),
        candidates: Vec::new(),
    }
}

/// Resolve a single identifier/query candidate against the graph in strict precedence:
/// Tier 0 exact ID/`task_id`/permalink -> Tier 1 alias/`permalinks`/`weak_keys` ->
/// Tier 2 prefix (`>= 6` chars) -> Tier 3 whole-query title -> `unknown`.
pub(crate) fn resolve_single_candidate(
    graph: &GraphStore,
    pkb_root: &Path,
    raw_query: &str,
) -> ResolveEntry {
    let q = raw_query.trim();
    if q.is_empty() {
        return ResolveEntry {
            query: raw_query.to_string(),
            state: ResolveState::Unknown,
            id: None,
            title: None,
            doc_type: None,
            status: None,
            tier: None,
            candidates: Vec::new(),
        };
    }
    let q_lower = q.to_ascii_lowercase();

    // Tier 0: Exact ID / task_id / permalink via shared `matches_doc_identity`.
    let mut exact_live: Vec<&GraphNode> = Vec::new();
    let mut exact_ghost: Vec<&GraphNode> = Vec::new();
    for node in graph.nodes() {
        let task_id_slice: &[String] = match node.task_id {
            Some(ref tid) => std::slice::from_ref(tid),
            None => &[],
        };
        if matches_doc_identity(q, &node.id, node.permalink.as_deref(), task_id_slice) {
            if node_is_ghost(node) {
                exact_ghost.push(node);
            } else {
                exact_live.push(node);
            }
        }
    }
    if !exact_live.is_empty() {
        return build_resolved_entry(q, ResolveState::Exact, pkb_root, exact_live);
    }
    if !exact_ghost.is_empty() {
        return build_resolved_entry(q, ResolveState::Ghost, pkb_root, exact_ghost);
    }

    // Tier 1: Alias / permalinks / weak_keys (file stem, id-prefix, alias array).
    let alias_matches: Vec<&GraphNode> = graph
        .nodes()
        .filter(|node| !node_is_ghost(node))
        .filter(|node| {
            node.permalinks
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&q_lower))
                || node
                    .weak_keys
                    .iter()
                    .any(|w| w.eq_ignore_ascii_case(&q_lower))
        })
        .collect();
    if !alias_matches.is_empty() {
        return build_resolved_entry(q, ResolveState::Alias, pkb_root, alias_matches);
    }

    // Tier 2: Unique ID prefix (>= 6 chars).
    if q.len() >= 6 {
        let mut prefix_live: Vec<&GraphNode> = Vec::new();
        let mut prefix_ghost: Vec<&GraphNode> = Vec::new();
        for node in graph.nodes() {
            let id_match = node.id.to_ascii_lowercase().starts_with(&q_lower);
            let tid_match = node
                .task_id
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().starts_with(&q_lower));
            if id_match || tid_match {
                if node_is_ghost(node) {
                    prefix_ghost.push(node);
                } else {
                    prefix_live.push(node);
                }
            }
        }
        if !prefix_live.is_empty() {
            return build_resolved_entry(q, ResolveState::Prefix, pkb_root, prefix_live);
        }
        if !prefix_ghost.is_empty() {
            return build_resolved_entry(q, ResolveState::Ghost, pkb_root, prefix_ghost);
        }
    }

    // Tier 3: Whole-query title match.
    let title_matches: Vec<&GraphNode> = graph
        .nodes()
        .filter(|node| !node_is_ghost(node))
        .filter(|node| node.label.trim().eq_ignore_ascii_case(q))
        .collect();
    if !title_matches.is_empty() {
        return build_resolved_entry(q, ResolveState::Title, pkb_root, title_matches);
    }

    ResolveEntry {
        query: q.to_string(),
        state: ResolveState::Unknown,
        id: None,
        title: None,
        doc_type: None,
        status: None,
        tier: None,
        candidates: Vec::new(),
    }
}

/// Extract deduplicated identifier candidates from explicit `ids` and/or inline `prompt` IDs
/// (via shared `crate::ranking::extract_exact_id_candidates`) and resolve each entry.
pub(crate) fn resolve_candidates(
    graph: &GraphStore,
    pkb_root: &Path,
    explicit_ids: &[String],
    prompt: Option<&str>,
) -> Vec<ResolveEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered_inputs: Vec<String> = Vec::new();

    for raw in explicit_ids {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            ordered_inputs.push(trimmed.to_string());
        }
    }
    if let Some(p) = prompt {
        for cand in extract_exact_id_candidates(p) {
            if seen.insert(cand.to_ascii_lowercase()) {
                ordered_inputs.push(cand);
            }
        }
    }

    ordered_inputs
        .into_iter()
        .map(|cand| resolve_single_candidate(graph, pkb_root, &cand))
        .collect()
}

fn parse_ids_arg(raw: Option<&JsonValue>) -> Result<(bool, Vec<String>), McpError> {
    match raw {
        None | Some(JsonValue::Null) => Ok((false, Vec::new())),
        Some(JsonValue::String(s)) => Ok((true, vec![s.clone()])),
        Some(JsonValue::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(s) = item.as_str() else {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from("Invalid 'ids' element: expected string"),
                        data: None,
                    });
                };
                out.push(s.to_string());
            }
            Ok((true, out))
        }
        Some(_) => Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from("Invalid 'ids' parameter: expected string or array of strings"),
            data: None,
        }),
    }
}

impl PkbSearchServer {
    pub(crate) fn handle_resolve(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.ensure_graph_fresh();

        let (has_ids, explicit_ids) = parse_ids_arg(args.get("ids"))?;
        let prompt = match args.get("prompt") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) => Some(s.as_str()),
            Some(_) => {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Invalid 'prompt' parameter: expected string"),
                    data: None,
                });
            }
        };

        if !has_ids && prompt.is_none() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Missing required parameter: provide 'ids' or 'prompt'"),
                data: None,
            });
        }

        let graph = self.graph.read();
        let mut entries = resolve_candidates(&graph, &self.pkb_root, &explicit_ids, prompt);
        drop(graph);

        // If any candidate was `unknown` after graph resolution (for example, after `ensure_graph_fresh`
        // evicted a deleted file from the graph while its stale index entry still remains in VectorStore),
        // check `store.documents()` for an orphaned/deleted document entry.
        if entries.iter().any(|e| e.state == ResolveState::Unknown) {
            let store = self.acquire_store_read();
            for entry in &mut entries {
                if entry.state != ResolveState::Unknown {
                    continue;
                }
                let q = entry.query.trim();
                let q_lower = q.to_ascii_lowercase();
                let mut deleted_matches: Vec<&crate::vectordb::DocumentEntry> = store
                    .documents()
                    .map(|(_, d)| d)
                    .filter(|d| {
                        let abs = if d.path.is_absolute() {
                            d.path.clone()
                        } else {
                            self.pkb_root.join(&d.path)
                        };
                        if abs.is_file() {
                            return false;
                        }
                        d.id.eq_ignore_ascii_case(q)
                            || (q.len() >= 6 && d.id.to_ascii_lowercase().starts_with(&q_lower))
                            || d.title.trim().eq_ignore_ascii_case(q)
                    })
                    .collect();
                deleted_matches.sort_by(|a, b| a.id.cmp(&b.id));
                deleted_matches.dedup_by(|a, b| a.id == b.id);
                if deleted_matches.len() == 1 {
                    let d = deleted_matches[0];
                    entry.state = ResolveState::Deleted;
                    entry.id = Some(d.id.clone());
                    entry.title = Some(d.title.clone());
                    entry.doc_type = d.doc_type.clone();
                    entry.status = d.status.clone();
                    entry.tier = Some(
                        crate::graph::derive_tier(d.doc_type.as_deref(), d.status.as_deref())
                            .as_str()
                            .to_string(),
                    );
                } else if deleted_matches.len() > 1 {
                    entry.state = ResolveState::Ambiguous;
                    entry.candidates = deleted_matches
                        .into_iter()
                        .map(|d| ResolveCandidateSummary {
                            id: d.id.clone(),
                            title: d.title.clone(),
                            doc_type: d.doc_type.clone(),
                            status: d.status.clone(),
                            tier: crate::graph::derive_tier(
                                d.doc_type.as_deref(),
                                d.status.as_deref(),
                            )
                            .as_str()
                            .to_string(),
                        })
                        .collect();
                }
            }
        }

        let mut resolved_ids: Vec<String> = Vec::new();
        for e in &entries {
            if e.state.is_live() {
                if let Some(ref id) = e.id {
                    if !resolved_ids.contains(id) {
                        resolved_ids.push(id.clone());
                    }
                }
            }
        }

        let payload = serde_json::json!({
            "count": entries.len(),
            "resolved_ids": resolved_ids,
            "entries": entries,
        });
        let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        Ok(Self::structured_result(text, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn make_seeded_server(root: &Path) -> PkbSearchServer {
        let embedder = Arc::new(crate::embeddings::Embedder::new_dummy());
        let dim = embedder.encode_query("seed").unwrap().len();
        let store = Arc::new(RwLock::new(crate::vectordb::VectorStore::new(dim)));
        let graph = Arc::new(RwLock::new(GraphStore::build(&[], root)));
        PkbSearchServer::new(store, embedder, root.to_path_buf(), root.join("db"), graph)
    }

    fn result_text(res: &CallToolResult) -> String {
        res.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn seed_node(server: &PkbSearchServer, filename: &str, raw_md: &str) {
        let path = server.pkb_root.join(filename);
        std::fs::write(&path, raw_md).unwrap();
        let doc = crate::pkb::parse_file(&path).unwrap();
        let emb = server.embedder.encode_query("seed").unwrap();
        server
            .store
            .write()
            .insert_precomputed(&doc, vec![doc.body.clone()], vec![emb]);
        let node = crate::graph::GraphNode::from_pkb_document(&doc);
        server.graph.write().replace_node(node);
    }

    #[test]
    fn test_resolve_all_states_and_inline_prompt_extraction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = make_seeded_server(dir.path());

        seed_node(
            &server,
            "doc-alpha-112233.md",
            "---\nid: doc-alpha-112233\ntitle: \"Alpha Architecture\"\ntype: note\nstatus: active\naliases:\n  - alpha-alias\ndepends_on:\n  - doc-ghost-9999\n---\n\nBody.\n",
        );
        seed_node(
            &server,
            "doc-ambig-aabb11.md",
            "---\nid: doc-ambig-aabb11\ntitle: \"Shared Title\"\ntype: task\nstatus: ready\n---\n\nA.\n",
        );
        seed_node(
            &server,
            "doc-ambig-aabb22.md",
            "---\nid: doc-ambig-aabb22\ntitle: \"Shared Title\"\ntype: task\nstatus: archived\n---\n\nB.\n",
        );
        seed_node(
            &server,
            "doc-gone-556677.md",
            "---\nid: doc-gone-556677\ntitle: \"Deleted File Node\"\ntype: note\n---\n\nGone soon.\n",
        );
        // Rebuild graph so ghost `doc-ghost-9999` is populated, then delete `doc-gone-556677.md` on disk
        {
            let docs: Vec<_> = crate::pkb::scan_directory(&server.pkb_root)
                .iter()
                .filter_map(|p| crate::pkb::parse_file_relative(p, &server.pkb_root))
                .collect();
            let rebuilt = crate::graph_store::GraphStore::build(&docs, &server.pkb_root);
            *server.graph.write() = rebuilt;
        }
        std::fs::remove_file(server.pkb_root.join("doc-gone-556677.md")).unwrap();

        let res = server
            .handle_resolve(&serde_json::json!({
                "ids": [
                    "doc-alpha-112233",   // exact
                    "alpha-alias",        // alias
                    "doc-alpha-11",       // prefix (>=6 chars, unique, not weak_key)
                    "Alpha Architecture", // title
                    "doc-ghost-9999",     // ghost
                    "doc-gone-556677",    // deleted
                    "doc-ambig-aabb",     // ambiguous prefix
                    "Shared Title",       // ambiguous title
                    "no-such-id-0000"     // unknown
                ],
                "prompt": "Check doc-alpha-112233 and doc-ambig-aabb11 for context."
            }))
            .expect("resolve");

        let parsed: serde_json::Value = serde_json::from_str(&result_text(&res)).unwrap();
        let entries = parsed["entries"].as_array().unwrap();
        let state_of = |q: &str| -> &str {
            entries
                .iter()
                .find(|e| e["query"].as_str() == Some(q))
                .and_then(|e| e["state"].as_str())
                .unwrap_or("MISSING")
        };

        assert_eq!(state_of("doc-alpha-112233"), "exact");
        assert_eq!(state_of("alpha-alias"), "alias");
        assert_eq!(state_of("doc-alpha-11"), "prefix");
        assert_eq!(state_of("Alpha Architecture"), "title");
        assert_eq!(state_of("doc-ghost-9999"), "ghost");
        assert_eq!(state_of("doc-gone-556677"), "deleted");
        assert_eq!(state_of("doc-ambig-aabb"), "ambiguous");
        assert_eq!(state_of("Shared Title"), "ambiguous");
        assert_eq!(state_of("no-such-id-0000"), "unknown");

        // Inline prompt extraction resolves `doc-ambig-aabb11` without re-querying `doc-alpha-112233`
        assert_eq!(state_of("doc-ambig-aabb11"), "exact");
        let ambig_entry = entries
            .iter()
            .find(|e| e["query"].as_str() == Some("doc-ambig-aabb"))
            .unwrap();
        assert_eq!(ambig_entry["candidates"].as_array().unwrap().len(), 2);

        let exact_entry = entries
            .iter()
            .find(|e| e["query"].as_str() == Some("doc-alpha-112233"))
            .unwrap();
        assert_eq!(exact_entry["tier"], "knowledge");
        assert_eq!(exact_entry["type"], "note");
        assert_eq!(exact_entry["status"], "in_progress");
    }
}
