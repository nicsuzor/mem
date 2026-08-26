use parking_lot::{Mutex, RwLock};
use rmcp::model::*;
use rmcp::{Error as McpError, ServerHandler};
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
    /// one of these requires a stated failure reason.
    pub(crate) const FAILURE_HANDBACK_STATUSES: &[&str] = &["blocked", "cancelled", "review", "partial"];

    /// Validate that completion_evidence is present and non-empty.
    pub(crate) fn require_evidence(evidence: Option<&str>) -> Result<&str, McpError> {
        let ev = evidence.ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "completion_evidence is required. Describe what was done before completing this task.",
            ),
            data: None,
        })?;
        if !ev.chars().any(|c| !c.is_whitespace()) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "completion_evidence is required. Describe what was done before completing this task.",
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

    pub(crate) fn args_to_value(args: Option<JsonObject>) -> JsonValue {
        match args {
            Some(map) => JsonValue::Object(map),
            None => JsonValue::Object(serde_json::Map::new()),
        }
    }
}
