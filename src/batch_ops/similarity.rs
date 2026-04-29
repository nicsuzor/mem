//! Semantic similarity discovery and analysis.

use crate::distance::cosine_similarity;
use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct SimilarityNeighbor {
    pub id: String,
    pub title: String,
    pub score: f64,
    pub is_explicit_edge: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarityReport {
    pub threshold: f64,
    pub total_nodes: usize,
    pub total_pairs_checked: usize,
    pub similarity_edges_found: usize,
    pub explicit_edges_skipped: usize,
    pub sample_neighbors: Vec<SimilarityPair>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarityPair {
    pub source_id: String,
    pub source_title: String,
    pub target_id: String,
    pub target_title: String,
    pub score: f64,
}

/// Find semantic neighbors for a specific node.
pub fn find_neighbors(
    node_id: &str,
    graph: &GraphStore,
    store: &VectorStore,
    threshold: f64,
    limit: usize,
) -> Vec<SimilarityNeighbor> {
    let node = match graph.resolve(node_id) {
        Some(n) => n,
        None => return vec![],
    };

    let node_embedding = match get_node_embedding(&node.id, &node.path.to_string_lossy(), store) {
        Some(emb) => emb,
        None => return vec![],
    };

    let mut neighbors = Vec::new();
    let explicit_targets: std::collections::HashSet<String> = graph
        .get_outgoing_edges(&node.id)
        .iter()
        .map(|e| e.target.clone())
        .collect();

    for other in graph.nodes() {
        if other.id == node.id {
            continue;
        }

        if let Some(other_emb) = get_node_embedding(&other.id, &other.path.to_string_lossy(), store) {
            let score = cosine_similarity(&node_embedding, &other_emb) as f64;
            if score >= threshold {
                neighbors.push(SimilarityNeighbor {
                    id: other.id.clone(),
                    title: other.label.clone(),
                    score,
                    is_explicit_edge: explicit_targets.contains(&other.id),
                });
            }
        }
    }

    neighbors.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    neighbors.truncate(limit);
    neighbors
}

/// Run empirical analysis on the full graph.
pub fn run_analysis(
    graph: &GraphStore,
    store: &VectorStore,
    threshold: f64,
) -> SimilarityReport {
    let nodes: Vec<&GraphNode> = graph.nodes().collect();
    let n = nodes.len();
    
    let mut embedding_map = HashMap::new();
    for node in &nodes {
        if let Some(emb) = get_node_embedding(&node.id, &node.path.to_string_lossy(), store) {
            embedding_map.insert(node.id.clone(), emb);
        }
    }

    let mut similarity_edges_found = 0;
    let mut explicit_edges_skipped = 0;
    let mut total_pairs_checked = 0;
    let mut samples = Vec::new();

    for i in 0..n {
        let node_a = nodes[i];
        let emb_a = match embedding_map.get(&node_a.id) {
            Some(e) => e,
            None => continue,
        };

        let explicit_targets: std::collections::HashSet<String> = graph
            .get_outgoing_edges(&node_a.id)
            .iter()
            .map(|e| e.target.clone())
            .collect();

        for j in (i + 1)..n {
            let node_b = nodes[j];
            let emb_b = match embedding_map.get(&node_b.id) {
                Some(e) => e,
                None => continue,
            };

            total_pairs_checked += 1;
            let score = cosine_similarity(emb_a, emb_b) as f64;

            if score >= threshold {
                let is_explicit = explicit_targets.contains(&node_b.id) || 
                                 graph.get_outgoing_edges(&node_b.id).iter().any(|e| e.target == node_a.id);
                
                if is_explicit {
                    explicit_edges_skipped += 1;
                } else {
                    similarity_edges_found += 1;
                    if samples.len() < 10 {
                        samples.push(SimilarityPair {
                            source_id: node_a.id.clone(),
                            source_title: node_a.label.clone(),
                            target_id: node_b.id.clone(),
                            target_title: node_b.label.clone(),
                            score,
                        });
                    }
                }
            }
        }
    }

    SimilarityReport {
        threshold,
        total_nodes: n,
        total_pairs_checked,
        similarity_edges_found,
        explicit_edges_skipped,
        sample_neighbors: samples,
    }
}

fn get_node_embedding(id: &str, path: &str, store: &VectorStore) -> Option<Vec<f32>> {
    let entry = store.get_entry(path).or_else(|| {
        let stripped = path.strip_prefix("tasks/").unwrap_or(path);
        store.get_entry(stripped)
    });

    if let Some(entry) = entry {
        average_embedding(&entry.chunk_embeddings)
    } else {
        None
    }
}

fn average_embedding(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    let mut avg = vec![0.0f32; dim];
    let n = embeddings.len() as f32;

    for emb in embeddings {
        for (i, v) in emb.iter().enumerate() {
            if i < dim {
                avg[i] += v / n;
            }
        }
    }
    Some(avg)
}
