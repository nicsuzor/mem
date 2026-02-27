//! Centrality metrics for knowledge graphs.
//!
//! Computes betweenness centrality (Brandes), PageRank (power iteration),
//! and per-node degree metrics over all edge types.

use crate::graph::Edge;
use std::collections::{HashMap, VecDeque};

/// Network metrics for a single node.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkMetrics {
    pub id: String,
    pub in_degree: usize,
    pub out_degree: usize,
    pub downstream_weight: f64,
    pub stakeholder_exposure: bool,
    pub betweenness: f64,
    pub pagerank: f64,
}

/// Compute degree metrics for all nodes.
pub fn compute_degrees(node_ids: &[String], edges: &[Edge]) -> HashMap<String, (usize, usize)> {
    let mut degrees: HashMap<String, (usize, usize)> = HashMap::new();
    for id in node_ids {
        degrees.insert(id.clone(), (0, 0));
    }
    for e in edges {
        if let Some(d) = degrees.get_mut(&e.source) {
            d.1 += 1; // out_degree
        }
        if let Some(d) = degrees.get_mut(&e.target) {
            d.0 += 1; // in_degree
        }
    }
    degrees
}

/// Betweenness centrality via Brandes' algorithm.
///
/// Exact computation, O(V*E). Trivial at ~3.4k nodes.
/// Considers all edge types equally.
pub fn compute_betweenness_centrality(node_ids: &[String], edges: &[Edge]) -> HashMap<String, f64> {
    let n = node_ids.len();
    let id_to_idx: HashMap<&str, usize> = node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    // Build adjacency list
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if let (Some(&si), Some(&ti)) = (
            id_to_idx.get(e.source.as_str()),
            id_to_idx.get(e.target.as_str()),
        ) {
            adj[si].push(ti);
        }
    }

    let mut cb = vec![0.0f64; n];

    for s in 0..n {
        // BFS from s
        let mut stack: Vec<usize> = Vec::new();
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut sigma = vec![0.0f64; n];
        let mut dist = vec![-1i64; n];

        sigma[s] = 1.0;
        dist[s] = 0;
        let mut queue: VecDeque<usize> = VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            for &w in &adj[v] {
                if dist[w] < 0 {
                    dist[w] = dist[v] + 1;
                    queue.push_back(w);
                }
                if dist[w] == dist[v] + 1 {
                    sigma[w] += sigma[v];
                    pred[w].push(v);
                }
            }
        }

        // Back-propagation
        let mut delta = vec![0.0f64; n];
        while let Some(w) = stack.pop() {
            for &v in &pred[w] {
                delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
            }
            if w != s {
                cb[w] += delta[w];
            }
        }
    }

    // Normalize (undirected normalization: divide by (n-1)(n-2))
    let norm = if n > 2 {
        1.0 / ((n - 1) as f64 * (n - 2) as f64)
    } else {
        1.0
    };

    node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), (cb[i] * norm * 10000.0).round() / 10000.0))
        .collect()
}

/// PageRank via power iteration (20 iterations, damping=0.85).
pub fn compute_pagerank(node_ids: &[String], edges: &[Edge]) -> HashMap<String, f64> {
    let n = node_ids.len();
    if n == 0 {
        return HashMap::new();
    }

    let id_to_idx: HashMap<&str, usize> = node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    // Build adjacency: out_edges[i] = list of targets
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if let (Some(&si), Some(&ti)) = (
            id_to_idx.get(e.source.as_str()),
            id_to_idx.get(e.target.as_str()),
        ) {
            out_edges[si].push(ti);
            in_edges[ti].push(si);
        }
    }

    let d = 0.85f64;
    let base = (1.0 - d) / n as f64;
    let mut rank = vec![1.0 / n as f64; n];

    for _ in 0..20 {
        let mut new_rank = vec![base; n];
        for i in 0..n {
            if !out_edges[i].is_empty() {
                let share = rank[i] / out_edges[i].len() as f64;
                for &j in &out_edges[i] {
                    new_rank[j] += d * share;
                }
            } else {
                // Dangling node: distribute evenly
                let share = rank[i] / n as f64;
                for j in 0..n {
                    new_rank[j] += d * share;
                }
            }
        }
        rank = new_rank;
    }

    node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), (rank[i] * 10000.0).round() / 10000.0))
        .collect()
}

/// Compute all network metrics for a specific node.
pub fn compute_network_metrics(
    node_id: &str,
    node_ids: &[String],
    edges: &[Edge],
    downstream_weight: f64,
    stakeholder_exposure: bool,
) -> Option<NetworkMetrics> {
    if !node_ids.contains(&node_id.to_string()) {
        return None;
    }

    let degrees = compute_degrees(node_ids, edges);
    let betweenness = compute_betweenness_centrality(node_ids, edges);
    let pagerank = compute_pagerank(node_ids, edges);

    let (in_deg, out_deg) = degrees.get(node_id).copied().unwrap_or((0, 0));

    Some(NetworkMetrics {
        id: node_id.to_string(),
        in_degree: in_deg,
        out_degree: out_deg,
        downstream_weight,
        stakeholder_exposure,
        betweenness: betweenness.get(node_id).copied().unwrap_or(0.0),
        pagerank: pagerank.get(node_id).copied().unwrap_or(0.0),
    })
}
