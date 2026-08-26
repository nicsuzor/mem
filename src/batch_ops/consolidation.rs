use crate::document_crud;
use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::pkb;
use crate::vectordb::VectorStore;
use crate::batch_ops::{BatchContext, BatchSummary, TaskAction};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use serde_json::Value as JsonValue;

/// Get a subgraph of related notes for a consolidation batch.
pub fn get_consolidation_cluster(
    graph: &GraphStore,
    vector_store: &VectorStore,
    seed_id: Option<&str>,
    max_nodes: usize,
    vector_top_k: usize,
    pkb_root: &std::path::Path,
) -> Result<serde_json::Value> {
    let seed_node = if let Some(id) = seed_id {
        graph.get_node(id).context("Seed node not found")?
    } else {
        graph
            .nodes()
            .filter(|n| !n.consolidated.unwrap_or(false))
            .filter(|n| {
                matches!(
                    n.node_type.as_deref().unwrap_or(""),
                    "epic" | "task" | "note" | "memory" | "insight" | "observation"
                )
            })
            .min_by_key(|n| n.created.clone().unwrap_or_else(|| "9999-99-99".to_string()))
            .context("No unconsolidated nodes found")?
    };

    let mut candidate_scores: HashMap<String, f64> = HashMap::new();

    // 1. Graph Proximity
    let mut graph_neighbors = HashSet::new();
    let edges = graph.edges();
    let mut hop1 = HashSet::new();
    for e in edges {
        if e.source == seed_node.id {
            hop1.insert(e.target.clone());
        } else if e.target == seed_node.id {
            hop1.insert(e.source.clone());
        }
    }
    for id in &hop1 {
        candidate_scores.insert(id.clone(), 0.3);
        for e in edges {
            if &e.source == id {
                graph_neighbors.insert(e.target.clone());
            } else if &e.target == id {
                graph_neighbors.insert(e.source.clone());
            }
        }
    }
    for id in graph_neighbors {
        if id != seed_node.id {
            let score = candidate_scores.entry(id).or_insert(0.0);
            if *score == 0.0 {
                *score = 0.15;
            }
        }
    }

    // 2. Vector Search Proximity
    let mut seed_embedding = None;
    if let Some((_, entry)) = vector_store.documents().find(|(k, _)| *k == &seed_node.id) {
        if !entry.chunk_embeddings.is_empty() {
            seed_embedding = Some(entry.chunk_embeddings[0].clone());
        }
    } else {
        for (_, entry) in vector_store.documents() {
            if entry.id == seed_node.id || entry.path == seed_node.path {
                if !entry.chunk_embeddings.is_empty() {
                    seed_embedding = Some(entry.chunk_embeddings[0].clone());
                    break;
                }
            }
        }
    }

    if let Some(emb) = seed_embedding {
        let results = vector_store.search(&emb, vector_top_k, pkb_root, None, None, None);
        for res in results {
            if res.id != seed_node.id {
                let score = candidate_scores.entry(res.id.clone()).or_insert(0.0);
                *score += res.score as f64 * 0.5;
            }
        }
    }

    // 3. Orphan Harvesting
    let seed_tags: HashSet<_> = seed_node.tags.iter().collect();
    if !seed_tags.is_empty() {
        for node in graph.nodes() {
            if node.id != seed_node.id && node.indegree + node.outdegree <= 1 {
                let node_tags: HashSet<_> = node.tags.iter().collect();
                if seed_tags.intersection(&node_tags).next().is_some() {
                    let score = candidate_scores.entry(node.id.clone()).or_insert(0.0);
                    *score += 0.2;
                }
            }
        }
    }

    // Compile Top N
    let mut candidates: Vec<(String, f64)> = candidate_scores.into_iter().collect();
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(max_nodes);

    let mut result_nodes = vec![seed_node.clone()];
    for (id, _score) in candidates {
        if let Some(n) = graph.get_node(&id) {
            result_nodes.push(n.clone());
        }
    }

    // Fetch Full content for these nodes
    let mut nodes_payload = Vec::new();
    for n in result_nodes {
        let abs_path = if n.path.is_absolute() {
            n.path.clone()
        } else {
            pkb_root.join(&n.path)
        };
        
        let pkb_doc = pkb::parse_file(&abs_path);
        
        nodes_payload.push(serde_json::json!({
            "id": n.id,
            "title": n.label,
            "path": n.path.to_string_lossy().to_string(),
            "frontmatter": pkb_doc.as_ref().and_then(|d| d.frontmatter.clone()),
            "body": pkb_doc.as_ref().map(|d| d.body.clone()),
            "edges": graph.edges().iter().filter(|e| e.source == n.id || e.target == n.id).collect::<Vec<_>>(),
        }));
    }

    Ok(serde_json::json!({
        "seed_id": seed_node.id,
        "nodes": nodes_payload,
    }))
}

/// Applies a consolidation batch across multiple documents with pre-flight checks, atomic per-file writes, and verified rollback on failure.
pub fn apply_consolidation_batch(
    ctx: &mut BatchContext,
    seed_id: &str,
    updates: HashMap<String, HashMap<String, JsonValue>>,
    dry_run: bool,
) -> Result<BatchSummary> {
    let mut summary = BatchSummary::new("consolidation", dry_run);

    if updates.is_empty() {
        return Ok(summary);
    }

    // Deterministic write order: sort updates by node_id.
    let mut sorted_updates: Vec<(String, HashMap<String, JsonValue>)> = updates.into_iter().collect();
    sorted_updates.sort_by(|a, b| a.0.cmp(&b.0));

    // Pre-flight snapshotting across all target nodes.
    // Every node must exist in the graph and its file must be readable on disk.
    // Any gap aborts before ANY write is performed.
    let mut snapshots = BTreeMap::new();
    for (node_id, _) in &sorted_updates {
        let node = ctx
            .graph
            .get_node(node_id)
            .ok_or_else(|| anyhow::anyhow!("Node not found in graph: {node_id}"))?;

        let abs_path = if node.path.is_absolute() {
            node.path.clone()
        } else {
            ctx.pkb_root.join(&node.path)
        };

        if !abs_path.exists() {
            anyhow::bail!(
                "File not found on disk for node {node_id}: {}",
                abs_path.display()
            );
        }

        let content = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read file for node {node_id} at {}", abs_path.display()))?;

        snapshots.insert(abs_path, content);
    }

    if dry_run {
        for (node_id, _) in &sorted_updates {
            let node = ctx.graph.get_node(node_id).unwrap();
            summary.matched += 1;
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: node_id.clone(),
                title: node.label.clone(),
                action: "would_consolidate".to_string(),
                detail: None,
                old_value: None,
                new_value: None,
            });
        }
        return Ok(summary);
    }

    // Apply updates in deterministic order
    for (node_id, mut upds) in sorted_updates {
        // If this is the seed node, ensure we mark it consolidated
        if node_id == seed_id {
            upds.insert("consolidated".to_string(), JsonValue::Bool(true));
            upds.insert("consolidated_at".to_string(), JsonValue::String(chrono::Utc::now().to_rfc3339()));
        }

        let node_label = ctx.graph.get_node(&node_id).map(|n| n.label.clone()).unwrap_or_default();

        if let Err(e) = ctx.update_task(&node_id, upds) {
            // Rollback with verified disk restore
            let mut failed_restores = Vec::new();
            for (path, original_content) in &snapshots {
                if let Err(write_err) = document_crud::atomic_write_file(path, original_content, "consolidation_rollback") {
                    failed_restores.push(format!("{}: {}", path.display(), write_err));
                    continue;
                }
                match std::fs::read_to_string(path) {
                    Ok(restored) if &restored == original_content => {}
                    Ok(_) => failed_restores.push(format!("{}: content mismatch after restore write", path.display())),
                    Err(read_err) => failed_restores.push(format!("{}: readback verification failed: {}", path.display(), read_err)),
                }
            }

            if !failed_restores.is_empty() {
                anyhow::bail!(
                    "Failed to update node {}: {}. Rollback FAILED for files: {:?}",
                    node_id,
                    e,
                    failed_restores
                );
            } else {
                anyhow::bail!("Failed to update node {}: {}. Rolled back.", node_id, e);
            }
        }

        summary.matched += 1;
        summary.changed += 1;
        summary.tasks.push(TaskAction {
            id: node_id.clone(),
            title: node_label,
            action: "consolidated".to_string(),
            detail: None,
            old_value: None,
            new_value: None,
        });
    }

    summary.modified_paths = ctx.modified_paths().to_vec();
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_ops::tree_snapshot;
    use crate::graph_store::GraphStore;
    use std::os::unix::fs::PermissionsExt;

    fn setup_test_pkb() -> (tempfile::TempDir, GraphStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = dir.path();
        std::fs::write(
            pkb_root.join("seed.md"),
            "---\nid: seed-1\ntitle: Seed Document\ntype: note\nstatus: active\n---\n\n# Seed Document\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("target1.md"),
            "---\nid: target-1\ntitle: Target Document 1\ntype: task\nstatus: ready\n---\n\n# Target 1\n",
        )
        .unwrap();
        std::fs::write(
            pkb_root.join("target2.md"),
            "---\nid: target-2\ntitle: Target Document 2\ntype: task\nstatus: ready\n---\n\n# Target 2\n",
        )
        .unwrap();

        let graph = GraphStore::build_from_directory(pkb_root);
        (dir, graph)
    }

    #[test]
    fn test_consolidation_dry_run_writes_nothing() {
        let (dir, graph) = setup_test_pkb();
        let pkb_root = dir.path();
        let before = tree_snapshot(pkb_root);

        let mut updates = HashMap::new();
        let mut seed_upd = HashMap::new();
        seed_upd.insert("status".to_string(), JsonValue::String("done".to_string()));
        updates.insert("seed-1".to_string(), seed_upd);

        let mut t1_upd = HashMap::new();
        t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-1".to_string(), t1_upd);

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let summary = apply_consolidation_batch(&mut ctx, "seed-1", updates.clone(), true)
            .expect("dry run should succeed");

        assert_eq!(summary.matched, 2);
        assert_eq!(summary.changed, 2);
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "dry_run=true modified files on disk"
        );

        // Execute for real and assert disk state changed
        let mut ctx = BatchContext::new(&graph, pkb_root);
        let live_summary = apply_consolidation_batch(&mut ctx, "seed-1", updates, false)
            .expect("live run should succeed");

        assert_eq!(live_summary.changed, 2);
        assert_ne!(
            tree_snapshot(pkb_root),
            before,
            "live run wrote nothing to disk"
        );
    }

    #[test]
    fn test_consolidation_missing_node_aborts_before_write() {
        let (dir, graph) = setup_test_pkb();
        let pkb_root = dir.path();
        let before = tree_snapshot(pkb_root);

        let mut updates = HashMap::new();
        let mut t1_upd = HashMap::new();
        t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-1".to_string(), t1_upd);

        let mut missing_upd = HashMap::new();
        missing_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("nonexistent-node-id".to_string(), missing_upd);

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let err = apply_consolidation_batch(&mut ctx, "seed-1", updates, false)
            .expect_err("should error on missing node");

        assert!(
            err.to_string().contains("nonexistent-node-id"),
            "error should name missing node: {err}"
        );
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "failed pre-flight must not modify any files on disk"
        );
    }

    #[test]
    fn test_consolidation_unreadable_file_aborts_before_write() {
        let (dir, graph) = setup_test_pkb();
        let pkb_root = dir.path();

        // Make target2 unreadable
        let target2_path = pkb_root.join("target2.md");
        std::fs::set_permissions(&target2_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let before = tree_snapshot(pkb_root);

        let mut updates = HashMap::new();
        let mut t1_upd = HashMap::new();
        t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-1".to_string(), t1_upd);

        let mut t2_upd = HashMap::new();
        t2_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-2".to_string(), t2_upd);

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let err = apply_consolidation_batch(&mut ctx, "seed-1", updates, false)
            .expect_err("should error on unreadable file");

        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "unreadable file pre-flight must leave disk untouched"
        );

        // Restore permissions for cleanup
        std::fs::set_permissions(&target2_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn test_consolidation_induced_failure_rolls_back_atomically() {
        let (dir, graph) = setup_test_pkb();
        let pkb_root = dir.path();

        // Induce corrupt frontmatter (duplicate key) in target-2 so that update_document bails on it
        let target2_path = pkb_root.join("target2.md");
        std::fs::write(
            &target2_path,
            "---\nid: target-2\ntitle: Target Document 2\ntitle: Duplicate Title Conflict\ntype: task\nstatus: ready\n---\n\n# Target 2\n",
        )
        .unwrap();

        let before = tree_snapshot(pkb_root);

        // target-1 sorts before target-2 alphabetically, so target-1 write will succeed first,
        // then target-2 will fail, triggering verified rollback of target-1.
        let mut updates = HashMap::new();
        let mut t1_upd = HashMap::new();
        t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-1".to_string(), t1_upd);

        let mut t2_upd = HashMap::new();
        t2_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-2".to_string(), t2_upd);

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let err = apply_consolidation_batch(&mut ctx, "seed-1", updates, false)
            .expect_err("should fail when target-2 update fails");

        assert!(
            err.to_string().contains("Rolled back."),
            "error message should confirm verified rollback: {err}"
        );
        assert_eq!(
            tree_snapshot(pkb_root),
            before,
            "induced failure must leave PKB byte-identical to pre-batch state"
        );
    }

    #[test]
    fn test_consolidation_deterministic_write_order() {
        // Run identical failing batch twice on fresh fixtures and verify identical trees
        let run_failing_batch = || -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
            let (dir, graph) = setup_test_pkb();
            let pkb_root = dir.path();
            let target2_path = pkb_root.join("target2.md");
            std::fs::write(
                &target2_path,
                "---\nid: target-2\ntitle: Target Document 2\ntitle: Duplicate Title Conflict\ntype: task\nstatus: ready\n---\n\n# Target 2\n",
            )
            .unwrap();

            let mut updates = HashMap::new();
            let mut t1_upd = HashMap::new();
            t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
            updates.insert("target-1".to_string(), t1_upd);

            let mut t2_upd = HashMap::new();
            t2_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
            updates.insert("target-2".to_string(), t2_upd);

            let mut ctx = BatchContext::new(&graph, pkb_root);
            let _ = apply_consolidation_batch(&mut ctx, "seed-1", updates, false);
            tree_snapshot(pkb_root)
        };

        let tree1 = run_failing_batch();
        let tree2 = run_failing_batch();
        assert_eq!(tree1, tree2, "two runs of identical failing batch must produce identical trees");
    }

    #[test]
    fn test_consolidation_rollback_failure_enumerates_all_failed_files() {
        let (dir, _graph) = setup_test_pkb();
        let pkb_root = dir.path();

        // Create subdirectories for target-3 and target-4
        let sub1 = pkb_root.join("sub1");
        let sub2 = pkb_root.join("sub2");
        std::fs::create_dir(&sub1).unwrap();
        std::fs::create_dir(&sub2).unwrap();

        let target3_path = sub1.join("target3.md");
        std::fs::write(
            &target3_path,
            "---\nid: target-3\ntitle: Target Document 3\ntype: task\nstatus: ready\n---\n\n# Target 3\n",
        )
        .unwrap();

        let target4_path = sub2.join("target4.md");
        std::fs::write(
            &target4_path,
            "---\nid: target-4\ntitle: Target Document 4\ntype: task\nstatus: ready\n---\n\n# Target 4\n",
        )
        .unwrap();

        // Rebuild graph to include all valid targets
        let graph = GraphStore::build_from_directory(pkb_root);

        // Induce corrupt frontmatter (duplicate key) in target-2 so update_document fails on it
        let target2_path = pkb_root.join("target2.md");
        std::fs::write(
            &target2_path,
            "---\nid: target-2\ntitle: Target Document 2\ntitle: Duplicate Title Conflict\ntype: task\nstatus: ready\n---\n\n# Target 2\n",
        )
        .unwrap();

        // Make sub1 and sub2 directories read-only so atomic_write_file cannot write temp files during rollback
        std::fs::set_permissions(&sub1, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::set_permissions(&sub2, std::fs::Permissions::from_mode(0o555)).unwrap();

        let mut updates = HashMap::new();
        let mut t1_upd = HashMap::new();
        t1_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-1".to_string(), t1_upd);

        let mut t2_upd = HashMap::new();
        t2_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-2".to_string(), t2_upd);

        let mut t3_upd = HashMap::new();
        t3_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-3".to_string(), t3_upd);

        let mut t4_upd = HashMap::new();
        t4_upd.insert("priority".to_string(), JsonValue::Number(1.into()));
        updates.insert("target-4".to_string(), t4_upd);

        let mut ctx = BatchContext::new(&graph, pkb_root);
        let err = apply_consolidation_batch(&mut ctx, "seed-1", updates, false)
            .expect_err("batch should fail and report failed rollback");

        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Rollback FAILED for files:"),
            "error should indicate rollback failure: {err_msg}"
        );
        assert!(
            err_msg.contains("target3.md"),
            "error must enumerate target3.md in rollback failures: {err_msg}"
        );
        assert!(
            err_msg.contains("target4.md"),
            "error must enumerate target4.md in rollback failures: {err_msg}"
        );

        // Restore permissions for cleanup
        std::fs::set_permissions(&sub1, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&sub2, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
