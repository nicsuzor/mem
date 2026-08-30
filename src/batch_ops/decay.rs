// Parked dormant by ruling, no caller by design.

use crate::batch_ops::{BatchContext, BatchSummary};
use crate::graph::ContributesTo;
use crate::pkb;
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

/// Preserves authored key names (e.g. `target:`, `why:`, `weight:`) by patching `current_weight`
/// directly into the existing frontmatter JSON structure rather than re-serializing the parsed struct.
fn patch_contributes_to_frontmatter(
    abs_path: &std::path::Path,
    new_edges: &[ContributesTo],
) -> JsonValue {
    let patched = (|| -> Option<JsonValue> {
        let content = std::fs::read_to_string(abs_path).ok()?;
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter.parse(&content);
        let data = parsed.data?.deserialize::<JsonValue>().ok()?;
        let mut raw_edges = match data.get("contributes_to")?.as_array()?.clone() {
            arr => arr,
        };
        if raw_edges.len() != new_edges.len() {
            return None;
        }
        for (i, edge) in new_edges.iter().enumerate() {
            if let Some(obj) = raw_edges[i].as_object_mut() {
                if let Some(cw) = edge.current_weight {
                    obj.insert("current_weight".to_string(), serde_json::json!(cw));
                } else {
                    obj.remove("current_weight");
                }
            } else {
                return None;
            }
        }
        Some(JsonValue::Array(raw_edges))
    })();

    patched.unwrap_or_else(|| serde_json::to_value(new_edges).unwrap_or(JsonValue::Null))
}

/// Runs exponential decay on ContributesTo edges.
pub fn run_decay(
    ctx: &mut BatchContext,
    lambda: f64,
    dry_run: bool,
) -> Result<BatchSummary> {
    let mut summary = BatchSummary::new("decay", dry_run);
    let now = chrono::Utc::now();

    let mut updates = HashMap::new();

    for node in ctx.graph.nodes() {
        if node.contributes_to.is_empty() {
            continue;
        }

        let mut modified = false;
        let mut new_edges = node.contributes_to.clone();

        for edge in new_edges.iter_mut() {
            let base_weight = edge.numeric_weight();
            if base_weight <= 0.0 {
                continue;
            }

            // Time since last interacted or created
            let timestamp_str = edge
                .last_interacted
                .as_deref()
                .or(node.created.as_deref())
                .unwrap_or("2000-01-01T00:00:00Z");
            
            if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(timestamp_str) {
                let days_elapsed = (now - ts.with_timezone(&chrono::Utc)).num_days() as f64;
                if days_elapsed > 0.0 {
                    let new_weight = base_weight * (-lambda * days_elapsed).exp();
                    
                    // Only update if difference is meaningful
                    let diff = edge.current_weight.unwrap_or(base_weight) - new_weight;
                    if diff.abs() > 0.01 {
                        edge.current_weight = Some(new_weight);
                        modified = true;
                    }
                }
            }
        }

        if modified {
            summary.matched += 1;
            summary.changed += 1;

            if !dry_run {
                let abs_path = if node.path.is_absolute() {
                    node.path.clone()
                } else {
                    ctx.pkb_root.join(&node.path)
                };

                let val = patch_contributes_to_frontmatter(&abs_path, &new_edges);
                let mut upds = HashMap::new();
                upds.insert("contributes_to".to_string(), val);
                updates.insert(node.id.clone(), upds);
            }
        }
    }

    if !dry_run {
        for (node_id, upds) in updates {
            if let Err(e) = ctx.update_task(&node_id, upds) {
                tracing::warn!("Failed to apply decay to {}: {}", node_id, e);
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphNode;

    /// `run_decay` with `dry_run: true` must leave every byte under the PKB root
    /// untouched. Proved non-vacuous by running the same decay for real
    /// afterwards and requiring that it *does* change the tree — otherwise a
    /// decay that matched nothing would pass this test while writing freely.
    #[test]
    fn test_run_decay_dry_run_writes_nothing() {
        use crate::batch_ops::tree_snapshot;
        use crate::graph_store::GraphStore;

        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        std::fs::write(
            pkb_root.join("targ.md"),
            "---\nid: targ-1\ntitle: Target\ntype: target\nstatus: active\n---\n",
        )
        .unwrap();
        // `created` far in the past, so the decayed weight moves by well over the
        // 0.01 threshold `run_decay` requires before it counts an edge as changed.
        std::fs::write(
            pkb_root.join("task.md"),
            "---\nid: task-1\ntitle: Task\ntype: task\nstatus: active\n\
             created: 2020-01-01T00:00:00Z\ncontributes_to:\n  - to: targ-1\n    \
             stated_weight: probable\n    justification: seeds a decayable edge\n---\n",
        )
        .unwrap();

        let graph = GraphStore::build_from_directory(pkb_root);
        let before = tree_snapshot(pkb_root);
        assert!(
            !before.is_empty(),
            "fixture is empty; assertions would be vacuous"
        );

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let summary = run_decay(&mut ctx, 0.01, true).expect("decay dry run");
        assert!(
            summary.changed > 0,
            "fixture produced no decayable edge, so the disk assertion below proves nothing"
        );
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "run_decay dry run modified files on disk"
        );

        let mut ctx = BatchContext::new(&graph, pkb_root);
        run_decay(&mut ctx, 0.01, false).expect("decay execute");
        assert_ne!(
            tree_snapshot(pkb_root),
            before,
            "run_decay wrote nothing even with dry_run=false — the dry_run=true \
             assertion above proved nothing about the gate"
        );
    }

    #[test]
    fn test_decay_logic() {
        // Just testing the math logic loosely
        let base_weight: f64 = 1.0;
        let lambda: f64 = 0.05;
        let days_elapsed: f64 = 10.0;
        
        let new_weight = base_weight * (-lambda * days_elapsed).exp();
        assert!(new_weight < base_weight);
        assert!((new_weight - 0.6065).abs() < 0.001);
    }

    #[test]
    fn test_run_decay_preserves_authored_frontmatter_keys() {
        use crate::graph_store::GraphStore;

        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        std::fs::write(
            pkb_root.join("targ.md"),
            "---\nid: targ-1\ntitle: Target\ntype: target\nstatus: active\n---\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("task.md"),
            "---\nid: task-1\ntitle: Task\ntype: task\nstatus: active\n\
             created: 2020-01-01T00:00:00Z\ncontributes_to:\n  - target: targ-1\n    \
             weight: probable\n    why: seeds a decayable edge\n---\n\nTask body\n",
        )
        .unwrap();

        let graph = GraphStore::build_from_directory(pkb_root);
        let mut ctx = BatchContext::new(&graph, pkb_root);
        run_decay(&mut ctx, 0.01, false).expect("decay execute");

        let after = std::fs::read_to_string(pkb_root.join("task.md")).unwrap();
        assert!(
            after.contains("target: targ-1"),
            "authored 'target:' key was rewritten. Full content:\n{}",
            after
        );
        assert!(
            after.contains("weight: probable"),
            "authored 'weight:' key was rewritten. Full content:\n{}",
            after
        );
        assert!(
            after.contains("why: seeds a decayable edge"),
            "authored 'why:' key was rewritten. Full content:\n{}",
            after
        );
    }
}
