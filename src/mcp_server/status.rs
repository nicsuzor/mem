use std::sync::atomic::Ordering;
use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData as McpError;
use serde_json::Value as JsonValue;

use super::PkbSearchServer;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReindexStatus {
    pub timestamp: String,
    pub outcome: String,
}

impl PkbSearchServer {
    pub fn bench_status(&self, args: &JsonValue) -> Result<CallToolResult, McpError> {
        self.handle_status(args)
    }

    /// Markdown files on disk vs. file-backed nodes in the in-memory index,
    /// as `(disk_file_count, indexed_file_count)`. Diagnostic only: reported
    /// by `status`, never appended to list/search responses.
    pub(crate) fn index_disk_counts(&self) -> (usize, usize) {
        let disk_count = crate::pkb::scan_directory(&self.pkb_root).len();
        let index_count = self
            .graph
            .read()
            .nodes_map()
            .values()
            .filter(|n| !n.path.as_os_str().is_empty())
            .count();
        (disk_count, index_count)
    }

    pub(crate) fn handle_status(&self, _args: &JsonValue) -> Result<CallToolResult, McpError> {
        let node_count = self.graph.read().node_count();
        let (disk_file_count, indexed_file_count) = self.index_disk_counts();
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
                "disk_file_count": disk_file_count,
                "indexed_file_count": indexed_file_count,
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
}
