//! `find_duplicates` and `batch_merge` — duplicate detection and resolution.

use super::{BatchContext, BatchSummary, TaskAction, TaskError};
use crate::distance::cosine_similarity;
use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use crate::vectordb::VectorStore;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A cluster of potentially duplicate tasks.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateCluster {
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Suggested canonical task ID (oldest, most connected, most content)
    pub canonical: String,
    /// All tasks in the cluster
    pub tasks: Vec<DuplicateEntry>,
    /// Similarity scores
    pub similarity_scores: SimilarityScores,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateEntry {
    pub id: String,
    pub title: String,
    pub created: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarityScores {
    pub title: f64,
    pub semantic: f64,
}

/// Result of find_duplicates.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateReport {
    pub clusters: Vec<DuplicateCluster>,
    pub total_clusters: usize,
    pub total_duplicates: usize,
}

/// Duplicate detection mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuplicateMode {
    Title,
    Semantic,
    Both,
}

impl DuplicateMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "title" => DuplicateMode::Title,
            "semantic" => DuplicateMode::Semantic,
            _ => DuplicateMode::Both,
        }
    }
}

/// Find potential duplicate tasks using title similarity and/or semantic embedding similarity.
pub fn find_duplicates(
    graph: &GraphStore,
    store: &VectorStore,
    filters: &super::filters::FilterSet,
    mode: DuplicateMode,
    title_threshold: f64,
    semantic_threshold: f64,
) -> DuplicateReport {
    let matched_ids = filters.resolve(graph);

    // Collect nodes for comparison
    let nodes: Vec<&GraphNode> = matched_ids
        .iter()
        .filter_map(|id| graph.get_node(id))
        .filter(|n| !crate::graph::is_completed(n.status.as_deref()))
        .collect();

    if nodes.len() < 2 {
        return DuplicateReport {
            clusters: vec![],
            total_clusters: 0,
            total_duplicates: 0,
        };
    }

    // Build embedding lookup: node_id -> average embedding
    let embeddings: HashMap<String, Vec<f32>> = if mode != DuplicateMode::Title {
        build_embedding_map(&nodes, store)
    } else {
        HashMap::new()
    };

    // Pairwise comparison
    let mut pairs: Vec<(usize, usize, f64, f64)> = Vec::new(); // (i, j, title_sim, semantic_sim)

    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let title_sim = if mode != DuplicateMode::Semantic {
                title_similarity(&nodes[i].label, &nodes[j].label)
            } else {
                0.0
            };

            let semantic_sim = if mode != DuplicateMode::Title {
                match (embeddings.get(&nodes[i].id), embeddings.get(&nodes[j].id)) {
                    (Some(a), Some(b)) => cosine_similarity(a, b) as f64,
                    _ => 0.0,
                }
            } else {
                0.0
            };

            let is_duplicate = match mode {
                DuplicateMode::Title => title_sim >= title_threshold,
                DuplicateMode::Semantic => semantic_sim >= semantic_threshold,
                DuplicateMode::Both => {
                    title_sim >= title_threshold || semantic_sim >= semantic_threshold
                }
            };

            if is_duplicate {
                pairs.push((i, j, title_sim, semantic_sim));
            }
        }
    }

    // Build clusters via union-find
    let clusters = build_clusters(&nodes, &pairs, title_threshold, semantic_threshold);

    let total_duplicates: usize = clusters.iter().map(|c| c.tasks.len() - 1).sum();

    DuplicateReport {
        total_clusters: clusters.len(),
        total_duplicates,
        clusters,
    }
}

/// Merge duplicate tasks into a canonical task.
pub fn batch_merge(
    graph: &GraphStore,
    pkb_root: &Path,
    canonical_id: &str,
    merge_ids: &[String],
    dry_run: bool,
) -> BatchSummary {
    let mut summary = BatchSummary::new("merge", dry_run);

    // Validate canonical exists
    let canonical = match graph.resolve(canonical_id) {
        Some(n) => n,
        None => {
            summary.errors.push(TaskError {
                id: canonical_id.to_string(),
                error: "canonical task not found".to_string(),
            });
            return summary;
        }
    };
    let canonical_id = canonical.id.clone();
    summary.matched = merge_ids.len() + 1; // include canonical

    let mut ctx = BatchContext::new(graph, pkb_root);

    // Collect data from merged tasks
    let mut all_tags: HashSet<String> = canonical.tags.iter().cloned().collect();
    let mut all_depends_on: HashSet<String> = canonical.depends_on.iter().cloned().collect();
    let best_priority = canonical.priority.unwrap_or(2);
    let mut children_to_reparent: Vec<String> = Vec::new();
    let mut backlinks_to_update: Vec<(String, String)> = Vec::new(); // (node_id, field) to repoint

    for merge_id in merge_ids {
        let node = match graph.resolve(merge_id) {
            Some(n) => n,
            None => {
                summary.errors.push(TaskError {
                    id: merge_id.clone(),
                    error: "task not found".to_string(),
                });
                continue;
            }
        };

        // Collect tags, deps
        all_tags.extend(node.tags.iter().cloned());
        all_depends_on.extend(node.depends_on.iter().cloned());

        // Collect children to reparent
        children_to_reparent.extend(node.children.iter().cloned());

        // Find backlinks: anything that references this merged task
        for other in graph.nodes() {
            if other.id == canonical_id || other.id == node.id {
                continue;
            }
            if other.parent.as_deref() == Some(&node.id) {
                backlinks_to_update.push((other.id.clone(), "parent".to_string()));
            }
            if other.depends_on.contains(&node.id) {
                backlinks_to_update.push((other.id.clone(), "depends_on".to_string()));
            }
            if other.soft_depends_on.contains(&node.id) {
                backlinks_to_update.push((other.id.clone(), "soft_depends_on".to_string()));
            }
            if other.soft_blocks.contains(&node.id) {
                backlinks_to_update.push((other.id.clone(), "soft_blocks".to_string()));
            }
        }

        if dry_run {
            summary.changed += 1;
            summary.tasks.push(TaskAction {
                id: node.id.clone(),
                title: node.label.clone(),
                action: "would_merge".to_string(),
                detail: Some(format!("→ {canonical_id}")),
                old_value: None,
                new_value: None,
            });
            continue;
        }

        // Archive merged task with superseded_by
        let mut updates = HashMap::new();
        updates.insert(
            "status".to_string(),
            serde_json::Value::String("done".to_string()),
        );
        updates.insert(
            "superseded_by".to_string(),
            serde_json::Value::String(canonical_id.clone()),
        );

        match ctx.update_task(&node.id, updates) {
            Ok(()) => {
                summary.changed += 1;
                summary.tasks.push(TaskAction {
                    id: node.id.clone(),
                    title: node.label.clone(),
                    action: "merged".to_string(),
                    detail: Some(format!("archived, superseded by {canonical_id}")),
                    old_value: None,
                    new_value: None,
                });
            }
            Err(e) => {
                summary.errors.push(TaskError {
                    id: node.id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    // Remove self-references from deps
    all_depends_on.remove(&canonical_id);
    for merge_id in merge_ids {
        all_depends_on.remove(merge_id);
    }

    if dry_run {
        // Report what would happen to canonical
        summary.tasks.push(TaskAction {
            id: canonical_id,
            title: canonical.label.clone(),
            action: "canonical".to_string(),
            detail: Some("would receive merged tags/deps/children".to_string()),
            old_value: None,
            new_value: None,
        });
        return summary;
    }

    // Update canonical with merged data
    let mut canonical_updates = HashMap::new();

    let tags_vec: Vec<String> = all_tags.into_iter().collect();
    canonical_updates.insert(
        "tags".to_string(),
        serde_json::Value::Array(tags_vec.into_iter().map(serde_json::Value::String).collect()),
    );

    if !all_depends_on.is_empty() {
        let deps_vec: Vec<String> = all_depends_on.into_iter().collect();
        canonical_updates.insert(
            "depends_on".to_string(),
            serde_json::Value::Array(deps_vec.into_iter().map(serde_json::Value::String).collect()),
        );
    }

    // Keep highest priority (lowest number)
    let final_priority = merge_ids
        .iter()
        .filter_map(|id| graph.resolve(id))
        .filter_map(|n| n.priority)
        .chain(std::iter::once(best_priority))
        .min()
        .unwrap_or(2);
    canonical_updates.insert(
        "priority".to_string(),
        serde_json::Value::Number(final_priority.into()),
    );

    if let Err(e) = ctx.update_task(&canonical_id, canonical_updates) {
        summary.errors.push(TaskError {
            id: canonical_id.clone(),
            error: format!("failed to update canonical: {e}"),
        });
    } else {
        summary.tasks.push(TaskAction {
            id: canonical_id.clone(),
            title: canonical.label.clone(),
            action: "canonical".to_string(),
            detail: Some("updated with merged tags/deps".to_string()),
            old_value: None,
            new_value: None,
        });
    }

    // Reparent children of merged tasks
    for child_id in &children_to_reparent {
        let mut updates = HashMap::new();
        updates.insert(
            "parent".to_string(),
            serde_json::Value::String(canonical_id.clone()),
        );
        if let Err(e) = ctx.update_task(child_id, updates) {
            summary.errors.push(TaskError {
                id: child_id.clone(),
                error: format!("failed to reparent child: {e}"),
            });
        }
    }

    // Update backlinks
    for (node_id, field) in &backlinks_to_update {
        if let Some(node) = graph.get_node(node_id) {
            let mut updates = HashMap::new();
            match field.as_str() {
                "parent" => {
                    updates.insert(
                        "parent".to_string(),
                        serde_json::Value::String(canonical_id.clone()),
                    );
                }
                "depends_on" => {
                    let new_deps: Vec<String> = node
                        .depends_on
                        .iter()
                        .map(|d| {
                            if merge_ids.contains(d) {
                                canonical_id.clone()
                            } else {
                                d.clone()
                            }
                        })
                        .collect();
                    updates.insert(
                        "depends_on".to_string(),
                        serde_json::Value::Array(
                            new_deps.into_iter().map(serde_json::Value::String).collect(),
                        ),
                    );
                }
                "soft_depends_on" => {
                    let new_deps: Vec<String> = node
                        .soft_depends_on
                        .iter()
                        .map(|d| {
                            if merge_ids.contains(d) {
                                canonical_id.clone()
                            } else {
                                d.clone()
                            }
                        })
                        .collect();
                    updates.insert(
                        "soft_depends_on".to_string(),
                        serde_json::Value::Array(
                            new_deps.into_iter().map(serde_json::Value::String).collect(),
                        ),
                    );
                }
                "soft_blocks" => {
                    let new_blocks: Vec<String> = node
                        .soft_blocks
                        .iter()
                        .map(|d| {
                            if merge_ids.contains(d) {
                                canonical_id.clone()
                            } else {
                                d.clone()
                            }
                        })
                        .collect();
                    updates.insert(
                        "soft_blocks".to_string(),
                        serde_json::Value::Array(
                            new_blocks.into_iter().map(serde_json::Value::String).collect(),
                        ),
                    );
                }
                _ => {}
            }
            let _ = ctx.update_task(node_id, updates);
        }
    }

    summary
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Compute Jaccard similarity on word sets (normalized, lowercased).
fn title_similarity(a: &str, b: &str) -> f64 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let words_a: HashSet<&str> = a_lower.split_whitespace().collect();
    let words_b: HashSet<&str> = b_lower.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Build average embedding for each node from the vector store.
fn build_embedding_map(nodes: &[&GraphNode], store: &VectorStore) -> HashMap<String, Vec<f32>> {
    let mut map = HashMap::new();

    for node in nodes {
        let path_str = node.path.to_string_lossy().to_string();

        // Try exact path, then try with/without leading prefix
        let entry = store
            .get_entry(&path_str)
            .or_else(|| {
                // Try stripping leading path components
                let stripped = path_str.strip_prefix("tasks/").unwrap_or(&path_str);
                store.get_entry(stripped)
            });

        if let Some(entry) = entry {
            if let Some(avg) = average_embedding(&entry.chunk_embeddings) {
                map.insert(node.id.clone(), avg);
            }
        }
    }

    map
}

/// Average multiple chunk embeddings into one vector.
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

/// Build duplicate clusters from pairwise matches via union-find.
fn build_clusters(
    nodes: &[&GraphNode],
    pairs: &[(usize, usize, f64, f64)],
    _title_threshold: f64,
    _semantic_threshold: f64,
) -> Vec<DuplicateCluster> {
    // Simple union-find
    let n = nodes.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut Vec<usize>, x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }

    fn union(parent: &mut Vec<usize>, x: usize, y: usize) {
        let rx = find(parent, x);
        let ry = find(parent, y);
        if rx != ry {
            parent[ry] = rx;
        }
    }

    for &(i, j, _, _) in pairs {
        union(&mut parent, i, j);
    }

    // Group by root
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }

    // Build clusters (only groups with >1 member)
    let mut clusters: Vec<DuplicateCluster> = groups
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|members| {
            // Find best pair scores for this cluster
            let mut best_title = 0.0f64;
            let mut best_semantic = 0.0f64;
            for &(i, j, ts, ss) in pairs {
                if members.contains(&i) && members.contains(&j) {
                    best_title = best_title.max(ts);
                    best_semantic = best_semantic.max(ss);
                }
            }

            let confidence = best_title.max(best_semantic);

            // Select canonical: prefer oldest, then most connected, then most content
            let canonical_idx = *members
                .iter()
                .min_by(|&&a, &&b| {
                    // Prefer oldest
                    let date_cmp = nodes[a]
                        .created
                        .cmp(&nodes[b].created);
                    // Then most children
                    let children_cmp = nodes[b]
                        .children
                        .len()
                        .cmp(&nodes[a].children.len());
                    // Then most content
                    let content_cmp = nodes[b]
                        .word_count
                        .cmp(&nodes[a].word_count);
                    date_cmp.then(children_cmp).then(content_cmp)
                })
                .unwrap();

            let tasks: Vec<DuplicateEntry> = members
                .iter()
                .map(|&idx| DuplicateEntry {
                    id: nodes[idx].id.clone(),
                    title: nodes[idx].label.clone(),
                    created: nodes[idx].created.clone(),
                })
                .collect();

            DuplicateCluster {
                confidence,
                canonical: nodes[canonical_idx].id.clone(),
                tasks,
                similarity_scores: SimilarityScores {
                    title: best_title,
                    semantic: best_semantic,
                },
            }
        })
        .collect();

    clusters.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_similarity_identical() {
        assert!((title_similarity("Write spec for analyst skill", "Write spec for analyst skill") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_title_similarity_high() {
        let sim = title_similarity(
            "Write spec for analyst skill",
            "Write spec for the analyst skill",
        );
        assert!(sim > 0.7, "Expected >0.7, got {sim}");
    }

    #[test]
    fn test_title_similarity_low() {
        let sim = title_similarity("Fix memory leak in TUI", "Write spec for analyst");
        assert!(sim < 0.3, "Expected <0.3, got {sim}");
    }

    #[test]
    fn test_average_embedding() {
        let embs = vec![vec![1.0, 2.0, 3.0], vec![3.0, 2.0, 1.0]];
        let avg = average_embedding(&embs).unwrap();
        assert_eq!(avg, vec![2.0, 2.0, 2.0]);
    }
}
