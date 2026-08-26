use crate::document_crud;
use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::pkb;
use crate::vectordb::VectorStore;
use crate::batch_ops::{BatchContext, BatchSummary, TaskAction};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
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

/// Applies a consolidation batch atomically.
pub fn apply_consolidation_batch(
    ctx: &mut BatchContext,
    seed_id: &str,
    updates: HashMap<String, HashMap<String, JsonValue>>,
) -> Result<BatchSummary> {
    let mut summary = BatchSummary::new("consolidation", false);
    
    // We will do a simple rollback strategy by snapshotting original files
    let mut snapshots = HashMap::new();
    for node_id in updates.keys() {
        if let Some(node) = ctx.graph.get_node(node_id) {
            let abs_path = if node.path.is_absolute() {
                node.path.clone()
            } else {
                ctx.pkb_root.join(&node.path)
            };
            if let Ok(content) = std::fs::read_to_string(&abs_path) {
                snapshots.insert(abs_path, content);
            }
        }
    }
    
    // Update documents
    for (node_id, mut upds) in updates {
        // If this is the seed node, ensure we mark it consolidated
        if node_id == seed_id {
            upds.insert("consolidated".to_string(), JsonValue::Bool(true));
            upds.insert("consolidated_at".to_string(), JsonValue::String(chrono::Utc::now().to_rfc3339()));
        }
        
        if let Err(e) = ctx.update_task(&node_id, upds) {
            // Rollback
            for (path, content) in snapshots {
                let _ = std::fs::write(&path, content);
            }
            anyhow::bail!("Failed to update node {}: {}. Rolled back.", node_id, e);
        }
        summary.matched += 1;
        summary.changed += 1;
    }
    
    Ok(summary)
}
