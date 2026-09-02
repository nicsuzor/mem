use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::PkbSearchServer;

impl PkbSearchServer {
    pub(crate) fn abs_path(&self, rel: &Path) -> PathBuf {
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.pkb_root.join(rel)
        }
    }

    /// Construct a diagnostic `McpError` for a ghost node (an ID referenced in the
    /// graph that has no backing file on disk).
    ///
    /// The error message names the referrer count and up to 3 referencing
    /// nodes/edge types, avoiding path disclosure while providing an actionable diagnosis.
    pub(crate) fn ghost_node_error(id: &str, graph: Option<&crate::graph_store::GraphStore>) -> McpError {
        let msg = if let Some(g) = graph {
            let incoming = g.get_incoming_edges(id);
            let count = incoming.len();
            if count == 0 {
                format!(
                    "'{id}' is a ghost node: present in graph but has no file on disk. Restore it or repair references to it."
                )
            } else {
                let referrers: Vec<String> = incoming
                    .iter()
                    .take(3)
                    .map(|e| format!("{} ({})", e.source, e.edge_type.as_str()))
                    .collect();
                let ref_summary = referrers.join(", ");
                if count == 1 {
                    format!(
                        "'{id}' is a ghost node: referenced by 1 document ({ref_summary}) but has no file on disk. Restore it or repair the referring field."
                    )
                } else if count <= 3 {
                    format!(
                        "'{id}' is a ghost node: referenced by {count} documents ({ref_summary}) but has no file on disk. Restore it or repair the referring fields."
                    )
                } else {
                    format!(
                        "'{id}' is a ghost node: referenced by {count} documents (e.g. {ref_summary}) but has no file on disk. Restore it or repair the referring fields."
                    )
                }
            }
        } else {
            format!(
                "'{id}' is a ghost node: present in graph but has no file on disk. Restore it or repair references to it."
            )
        };

        McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(msg),
            data: None,
        }
    }

    /// Resolve a `GraphNode`'s path to an absolute path, rejecting ghost nodes
    /// (empty `node.path`) with a descriptive, actionable error.
    ///
    /// This is the central choke point for node path resolution in the MCP server.
    pub(crate) fn abs_path_for_node(
        &self,
        node: &crate::graph::GraphNode,
        graph: Option<&crate::graph_store::GraphStore>,
    ) -> Result<PathBuf, McpError> {
        if node.path.as_os_str().is_empty() {
            return Err(Self::ghost_node_error(&node.id, graph));
        }
        Ok(self.abs_path(&node.path))
    }

    /// Resolve a relative path to absolute, rejecting empty paths with a ghost node error.
    pub(crate) fn abs_path_checked(
        &self,
        rel: &Path,
        node_id: Option<&str>,
        graph: Option<&crate::graph_store::GraphStore>,
    ) -> Result<PathBuf, McpError> {
        if rel.as_os_str().is_empty() {
            let id = node_id.unwrap_or("<unknown>");
            return Err(Self::ghost_node_error(id, graph));
        }
        Ok(self.abs_path(rel))
    }

    /// Optionally truncate a document/task body to at most `max_bytes` bytes,
    /// on a UTF-8-safe boundary, appending a marker noting the true size.
    /// `max_bytes: None` (the default in every caller) is a no-op — full-body
    /// reads are unchanged unless a caller opts in.
    pub(crate) fn truncate_body(body: String, max_bytes: Option<usize>) -> String {
        match max_bytes {
            Some(max) if body.len() > max => {
                let boundary = body.floor_char_boundary(max);
                let total = body.len();
                let mut truncated = body[..boundary].to_string();
                truncated.push_str(&format!("\n…[truncated, {total} bytes total]"));
                truncated
            }
            _ => body,
        }
    }

    /// Statuses that are a handback rather than a clean success. Releasing to
    /// one of these requires a stated failure reason. Canonical list lives at
    /// `crate::graph::FAILURE_HANDBACK_STATUSES` so `batch_update` (which
    /// cannot see this `impl` block) enforces the identical set — see
    /// `mem_84b21efc`.
    pub(crate) const FAILURE_HANDBACK_STATUSES: &[&str] = crate::graph::FAILURE_HANDBACK_STATUSES;

    /// Validate that completion_evidence is present and non-empty.
    pub(crate) fn require_evidence(evidence: Option<&str>) -> Result<&str, McpError> {
        let ev = evidence.ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "completion_evidence is required. Describe what was done before completing this task. \
                 Example: complete_task(id=\"...\", completion_evidence=\"...\")",
            ),
            data: None,
        })?;
        if !ev.chars().any(|c| !c.is_whitespace()) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "completion_evidence is required. Describe what was done before completing this task. \
                     Example: complete_task(id=\"...\", completion_evidence=\"...\")",
                ),
                data: None,
            });
        }
        Ok(ev)
    }

    /// Append a "## Completion Evidence" section to a document.
    pub(crate) fn append_evidence(
        path: &std::path::Path,
        evidence: &str,
        pr_url: Option<&str>,
    ) -> Result<(), McpError> {
        let evidence_block = if let Some(url) = pr_url {
            format!(
                "\n\n## Completion Evidence\n\n{}\n\nPR: {}\n",
                evidence.trim(),
                url
            )
        } else {
            format!("\n\n## Completion Evidence\n\n{}\n", evidence.trim())
        };
        crate::document_crud::append_to_document(path, &evidence_block, None, None)
            .map(|_| ())
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to append evidence: {e}")),
                data: None,
            })
    }

    /// Intent bands are Nic's call only (task_1381381c). `create_task`/`create`
    /// already carry a prose-only guard against agent-originated intent
    /// (schemas.rs); this closes the remaining write paths with a runtime
    /// check. No provenance/authority mechanism distinguishes an agent's call
    /// from Nic's own on these paths, so the guard rejects unconditionally
    /// rather than trying to approximate "who" by heuristic.
    pub(crate) fn reject_agent_intent(tool: &str) -> McpError {
        McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "Agents must not set `intent` (or legacy `priority`) via {tool} — intent bands are Nic's call only. \
                 Leave it unset (or omit it from this update); ask Nic to change it directly if a \
                 re-prioritization is genuinely warranted."
            )),
            data: None,
        }
    }

    pub(crate) fn args_to_value(args: Option<JsonObject>) -> JsonValue {
        match args {
            Some(map) => JsonValue::Object(map),
            None => JsonValue::Object(serde_json::Map::new()),
        }
    }
}
