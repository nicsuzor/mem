//! Graph store — builds, queries, and exports knowledge graphs.
//!
//! [`GraphStore`] holds all nodes, edges, and pre-computed task indices.
//! Build from `PkbDocument`s via [`GraphStore::build`], then query with
//! the various accessor methods.

use crate::graph::{self, deduplicate_vec, Edge, EdgeType, GraphNode};
use crate::metrics;
use crate::pkb::PkbDocument;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

// ===========================================================================
// Output graph (for JSON serialization)
// ===========================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct OutputGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ready: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Top focus picks: ready tasks ranked by priority + deadline + staleness + downstream weight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub focus: Vec<String>,
}

// ===========================================================================
// GraphStore
// ===========================================================================

/// Knowledge graph over a PKB.
///
/// Holds all nodes and edges with pre-computed lookup indices,
/// task lists (ready/blocked), and per-node downstream metrics.
/// The `resolution_map` enables flexible node lookup by ID, filename, or title.
pub struct GraphStore {
    nodes: HashMap<String, GraphNode>,
    edges: Vec<Edge>,
    ready: Vec<String>,
    blocked: Vec<String>,
    roots: Vec<String>,
    /// Lowercase (id | filename stem | title | permalink) → canonical node ID
    resolution_map: HashMap<String, String>,
}

/// Document types considered actionable work items in task trees and dashboards.
pub const ACTIONABLE_TYPES: &[&str] = &["project", "epic", "task", "learn"];

/// Document types that represent claimable work items — leaf tasks a worker can actually do.
/// Excludes containers (epic, project) and observational types (learn).
pub const CLAIMABLE_TYPES: &[&str] = &["task"];

impl GraphStore {
    /// Build a complete graph from parsed PKB documents.
    ///
    /// Full pipeline:
    /// 1. Extract `GraphNode`s from `PkbDocument`s
    /// 2. Build lookup indices (permalink -> path, path -> id)
    /// 3. Resolve links and frontmatter refs into edges
    /// 4. Compute inverse relationships (depends_on -> blocks, etc.)
    /// 5. Compute downstream_weight + stakeholder_exposure (BFS)
    /// 6. Classify ready/blocked tasks
    pub fn build(docs: &[PkbDocument], pkb_root: &Path) -> Self {
        let nodes: Vec<GraphNode> = docs.par_iter().map(GraphNode::from_pkb_document).collect();
        Self::build_internal(nodes, pkb_root, true)
    }

    /// Build from a directory: scan, parse (with relative paths), build graph.
    pub fn build_from_directory(root: &Path) -> Self {
        let files = crate::pkb::scan_directory_all(root);
        let docs: Vec<PkbDocument> = files
            .par_iter()
            .filter_map(|p| crate::pkb::parse_file_relative(p, root))
            .collect();
        Self::build(&docs, root)
    }

    /// Rebuild graph from existing nodes (avoids re-scanning/re-parsing all files).
    ///
    /// Takes the current node map, rebuilds all edges, metrics, and indices.
    /// Use after updating/inserting/removing a single node to avoid a full
    /// directory walk.
    pub fn rebuild_from_nodes(nodes: HashMap<String, GraphNode>, pkb_root: &Path) -> Self {
        let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
        Self::build_internal(nodes_vec, pkb_root, true)
    }

    /// Fast incremental rebuild: skips centrality metrics (PageRank, betweenness).
    ///
    /// A single-file update shifts PageRank/betweenness by a negligible amount,
    /// and recomputing them is O(V*E) — the dominant cost of a rebuild at
    /// multi-thousand-node PKB sizes. The previous values on the cloned nodes
    /// are preserved (they drift slowly until the next full rebuild).
    pub fn rebuild_from_nodes_fast(nodes: HashMap<String, GraphNode>, pkb_root: &Path) -> Self {
        let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
        Self::build_internal(nodes_vec, pkb_root, false)
    }

    /// Internal helper to build GraphStore from a vector of nodes.
    ///
    /// This is the core pipeline shared by build() and rebuild_from_nodes().
    /// When `include_centrality` is false, PageRank and betweenness computation
    /// are skipped — any pre-existing values on the input nodes are retained.
    fn build_internal(mut nodes: Vec<GraphNode>, pkb_root: &Path, include_centrality: bool) -> Self {
        // 2. Build lookup maps
        // Node paths may be relative — reconstruct absolute for canonicalize & link resolution
        let mut id_map: HashMap<String, String> = HashMap::new(); // permalink -> abs_path
        let mut path_to_id: HashMap<String, String> = HashMap::new(); // abs_path -> id

        for n in &nodes {
            let full_path = if n.path.is_absolute() {
                n.path.clone()
            } else {
                pkb_root.join(&n.path)
            };
            let abs_path = full_path
                .canonicalize()
                .unwrap_or(full_path)
                .to_string_lossy()
                .to_string();
            path_to_id.insert(abs_path.clone(), n.id.clone());
            for key in &n.permalinks {
                id_map.insert(key.clone(), abs_path.clone());
            }
        }

        // 3. Build edges from links and frontmatter refs
        let edges: Vec<Edge> = nodes
            .par_iter()
            .flat_map(|n| build_node_edges(n, &id_map, &path_to_id, pkb_root))
            .collect();

        // Deduplicate edges by (source, target, type)
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        let edges: Vec<Edge> = edges
            .into_iter()
            .filter(|e| {
                let key = (
                    e.source.clone(),
                    e.target.clone(),
                    format!("{:?}", e.edge_type),
                );
                seen.insert(key)
            })
            .collect();

        // 4. Compute inverse relationships on nodes
        compute_inverses(&mut nodes, &edges);

        // 5. Compute degree metrics (indegree/outdegree)
        compute_degree_metrics(&mut nodes, &edges);

        // 6. Compute centrality metrics (PageRank, betweenness).
        //    Skipped on incremental rebuilds — O(V*E) dominates write latency at
        //    multi-thousand-node PKB sizes, and single-file updates barely shift
        //    these scores. Prior values on the input nodes are retained instead.
        if include_centrality {
            compute_centrality_metrics(&mut nodes, &edges);
        }

        // 7. Compute downstream metrics (BFS through blocks/soft_blocks/children)
        compute_downstream_metrics(&mut nodes);

        // 7b. Compute effective_priority (min priority in downstream cone)
        compute_effective_priority(&mut nodes);
        compute_blocking_urgency(&mut nodes);

        // 8. Compute derived properties: scope, uncertainty, criticality
        compute_scope(&mut nodes);
        compute_uncertainty(&mut nodes);
        compute_criticality(&mut nodes);

        // 9. Compute focus scores
        Self::compute_focus_scores(&mut nodes);

        // 9. Compute project field (nearest ancestor with node_type == "project")
        compute_project_field(&mut nodes);

        // 9. Compute reachable set (upstream BFS from active leaves)
        //    Mark nodes reachable from active leaves via BFS.
        let reachable_set = find_reachable_set(&nodes, &edges);
        for node in &mut nodes {
            node.reachable = reachable_set.contains(&node.id);
        }

        // 10. Build node map and classify tasks
        let node_map: HashMap<String, GraphNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let (ready, blocked, roots) = classify_tasks(&node_map);

        // 12. Build resolution map for flexible node lookup
        let resolution_map = build_resolution_map(&node_map);

        GraphStore {
            nodes: node_map,
            edges,
            ready,
            blocked,
            roots,
            resolution_map,
        }
    }


    // -----------------------------------------------------------------------
    // Query API
    // -----------------------------------------------------------------------

    pub fn get_node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.get(id)
    }

    /// Clone the full node map (for incremental rebuilds).
    pub fn nodes_cloned(&self) -> HashMap<String, GraphNode> {
        self.nodes.clone()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.values()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn get_edges_for(&self, id: &str) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|e| e.source == id || e.target == id)
            .collect()
    }

    pub fn get_outgoing_edges(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.source == id).collect()
    }

    pub fn get_incoming_edges(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.target == id).collect()
    }

    pub fn get_neighbors(&self, id: &str) -> Vec<&GraphNode> {
        let mut neighbor_ids: HashSet<&str> = HashSet::new();
        for e in &self.edges {
            if e.source == id {
                neighbor_ids.insert(&e.target);
            } else if e.target == id {
                neighbor_ids.insert(&e.source);
            }
        }
        neighbor_ids
            .iter()
            .filter_map(|nid| self.nodes.get(*nid))
            .collect()
    }

    pub fn ready_tasks(&self) -> Vec<&GraphNode> {
        self.ready
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn blocked_tasks(&self) -> Vec<&GraphNode> {
        self.blocked
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn all_tasks(&self) -> Vec<&GraphNode> {
        let mut tasks: Vec<&GraphNode> = self
            .nodes
            .values()
            .filter(|n| n.task_id.is_some())
            .collect();
        tasks.sort_by(|a, b| {
            b.severity
                .unwrap_or(0)
                .cmp(&a.severity.unwrap_or(0))
                .then(
                    a.priority
                        .unwrap_or(2)
                        .cmp(&b.priority.unwrap_or(2))
                )
                .then(
                    b.downstream_weight
                        .partial_cmp(&a.downstream_weight)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.order.cmp(&b.order))
                .then(a.label.cmp(&b.label))
        });
        tasks
    }

    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Get the dependency tree for a node (BFS through depends_on).
    /// Returns (node_id, depth) pairs.
    pub fn dependency_tree(&self, id: &str) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if !visited.insert(current_id.clone()) {
                continue;
            }
            if depth > 0 {
                result.push((current_id.clone(), depth));
            }
            if let Some(node) = self.nodes.get(&current_id) {
                for dep_id in &node.depends_on {
                    if !visited.contains(dep_id) {
                        queue.push_back((dep_id.clone(), depth + 1));
                    }
                }
            }
        }
        result
    }

    /// Get what this node blocks (BFS through blocks).
    /// Returns (node_id, depth) pairs.
    pub fn blocks_tree(&self, id: &str) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if !visited.insert(current_id.clone()) {
                continue;
            }
            if depth > 0 {
                result.push((current_id.clone(), depth));
            }
            if let Some(node) = self.nodes.get(&current_id) {
                for blocked_id in &node.blocks {
                    if !visited.contains(blocked_id) {
                        queue.push_back((blocked_id.clone(), depth + 1));
                    }
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Flexible node resolution
    // -----------------------------------------------------------------------

    /// Resolve a query to a node using flexible matching.
    ///
    /// Tries (in order): exact ID → resolution map (case-insensitive match on
    /// ID, task_id, filename stem, title, permalink) → path-based fallbacks
    /// (strip .md extension, extract filename stem from full paths).
    pub fn resolve(&self, query: &str) -> Option<&GraphNode> {
        // 1. Exact ID match
        if let Some(node) = self.nodes.get(query) {
            return Some(node);
        }
        // 2. Resolution map (case-insensitive)
        let lower = query.to_lowercase();
        if let Some(canonical_id) = self.resolution_map.get(&lower) {
            return self.nodes.get(canonical_id);
        }
        // 3. Strip .md extension and retry
        if let Some(stripped) = lower.strip_suffix(".md") {
            if let Some(canonical_id) = self.resolution_map.get(stripped) {
                return self.nodes.get(canonical_id);
            }
        }
        // 4. Extract filename stem from path-like queries (e.g. "/abs/path/to/task-abc.md")
        let as_path = std::path::Path::new(query);
        if let Some(stem) = as_path.file_stem() {
            let stem_lower = stem.to_string_lossy().to_lowercase();
            if stem_lower != lower {
                if let Some(canonical_id) = self.resolution_map.get(&stem_lower) {
                    return self.nodes.get(canonical_id);
                }
            }
        }
        // 5. Match by absolute path directly against node paths
        for node in self.nodes.values() {
            let node_path_str = node.path.to_string_lossy();
            if node_path_str == query || node_path_str.to_lowercase() == lower {
                return Some(node);
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Backlinks
    // -----------------------------------------------------------------------

    /// Get incoming edges grouped by source node type.
    ///
    /// Returns a map from node_type (e.g. "task", "note") to a list of
    /// `(source_node, edge_type)` tuples. Nodes without a type are grouped
    /// under "unknown".
    pub fn backlinks_by_type(&self, id: &str) -> HashMap<String, Vec<(&GraphNode, &EdgeType)>> {
        let mut result: HashMap<String, Vec<(&GraphNode, &EdgeType)>> = HashMap::new();
        for edge in self.get_incoming_edges(id) {
            if let Some(source_node) = self.nodes.get(&edge.source) {
                let node_type = source_node
                    .node_type
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_string();
                result
                    .entry(node_type)
                    .or_default()
                    .push((source_node, &edge.edge_type));
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Path-finding algorithms
    // -----------------------------------------------------------------------

    /// Find the shortest path between two nodes (undirected BFS).
    ///
    /// Returns the path as a list of node IDs including both endpoints,
    /// or `None` if no path exists.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return None;
        }

        // Build undirected adjacency for BFS
        let adj = self.undirected_adjacency();

        let mut visited: HashSet<&str> = HashSet::new();
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut queue: VecDeque<&str> = VecDeque::new();

        visited.insert(from);
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            if let Some(neighbors) = adj.get(current) {
                for &neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor);
                        parent.insert(neighbor, current);
                        if neighbor == to {
                            // Reconstruct path
                            let mut path = vec![to.to_string()];
                            let mut node = to;
                            while let Some(&prev) = parent.get(node) {
                                path.push(prev.to_string());
                                node = prev;
                            }
                            path.reverse();
                            return Some(path);
                        }
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        None
    }

    /// Find up to `max_paths` shortest paths between two nodes (undirected).
    ///
    /// All returned paths have the same (minimum) length. Uses BFS to find
    /// the shortest distance, then bounded DFS to enumerate paths at that distance.
    pub fn all_shortest_paths(&self, from: &str, to: &str, max_paths: usize) -> Vec<Vec<String>> {
        if from == to {
            return vec![vec![from.to_string()]];
        }
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return vec![];
        }

        let adj = self.undirected_adjacency();

        // BFS to find distance from `from` to every node
        let mut dist: HashMap<&str, usize> = HashMap::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        dist.insert(from, 0);
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            let d = dist[current];
            if let Some(neighbors) = adj.get(current) {
                for &neighbor in neighbors {
                    if !dist.contains_key(neighbor) {
                        dist.insert(neighbor, d + 1);
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        let target_dist = match dist.get(to) {
            Some(&d) => d,
            None => return vec![], // unreachable
        };

        // DFS to enumerate all shortest paths (follow only edges that decrease distance to target)
        let mut results: Vec<Vec<String>> = Vec::new();
        let mut stack: Vec<(Vec<String>, &str)> = vec![(vec![from.to_string()], from)];

        while let Some((path, current)) = stack.pop() {
            if results.len() >= max_paths {
                break;
            }
            if current == to {
                results.push(path);
                continue;
            }
            let current_dist = path.len() - 1; // distance from `from`
            if current_dist >= target_dist {
                continue; // already at max depth without reaching target
            }
            if let Some(neighbors) = adj.get(current) {
                for &neighbor in neighbors {
                    // Only follow edges where neighbor is one step closer to target
                    if let Some(&nd) = dist.get(neighbor) {
                        if nd == current_dist + 1 && nd <= target_dist {
                            // Also verify the neighbor can reach target in remaining steps
                            // (since dist is from `from`, we need reverse BFS or just trust
                            // the undirected shortest path property)
                            let mut new_path = path.clone();
                            new_path.push(neighbor.to_string());
                            stack.push((new_path, neighbor));
                        }
                    }
                }
            }
        }

        results
    }

    /// Extract the N-hop ego subgraph around a node (BFS).
    ///
    /// Returns `(node_id, hop_distance)` pairs for all nodes within `max_hops`
    /// of the center node (excluding the center itself).
    pub fn ego_subgraph(&self, id: &str, max_hops: usize) -> Vec<(String, usize)> {
        let adj = self.undirected_adjacency();
        let mut visited: HashMap<&str, usize> = HashMap::new();
        let mut queue: VecDeque<(&str, usize)> = VecDeque::new();

        visited.insert(id, 0);
        queue.push_back((id, 0));

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            if let Some(neighbors) = adj.get(current) {
                for &neighbor in neighbors {
                    if !visited.contains_key(neighbor) {
                        visited.insert(neighbor, depth + 1);
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }

        visited
            .into_iter()
            .filter(|&(nid, _)| nid != id)
            .map(|(nid, d)| (nid.to_string(), d))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Analysis tools
    // -----------------------------------------------------------------------

    /// Find orphan nodes (nodes with no valid parent).
    ///
    /// A node is an orphan if its `parent` field is either absent or references
    /// an ID that doesn't exist in the graph.
    pub fn orphans(&self) -> Vec<&GraphNode> {
        self.nodes
            .values()
            .filter(|n| match &n.parent {
                None => true,
                Some(pid) => !self.nodes.contains_key(pid.as_str()),
            })
            .collect()
    }

    /// Compute connected components (undirected).
    ///
    /// Returns a list of components, each a list of node IDs.
    /// Components are sorted by size (largest first).
    pub fn connected_components(&self) -> Vec<Vec<String>> {
        let adj = self.undirected_adjacency();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut components: Vec<Vec<String>> = Vec::new();

        for id in self.nodes.keys() {
            if visited.contains(id.as_str()) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue: VecDeque<&str> = VecDeque::new();
            visited.insert(id);
            queue.push_back(id);

            while let Some(current) = queue.pop_front() {
                component.push(current.to_string());
                if let Some(neighbors) = adj.get(current) {
                    for &neighbor in neighbors {
                        if !visited.contains(neighbor) {
                            visited.insert(neighbor);
                            queue.push_back(neighbor);
                        }
                    }
                }
            }

            components.push(component);
        }

        components.sort_by(|a, b| b.len().cmp(&a.len()));
        components
    }

    // -----------------------------------------------------------------------
    // Cycle detection
    // -----------------------------------------------------------------------

    /// Detect hard dependency cycles using Tarjan's SCC.
    ///
    /// Runs Tarjan's SCC on the subgraph of `DependsOn` + `Parent` edges.
    /// Returns SCCs with size > 1 — these are actual hard cycles.
    /// Each inner `Vec` contains the node IDs in one cycle.
    pub fn find_hard_cycles(&self) -> Vec<Vec<String>> {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if matches!(edge.edge_type, EdgeType::DependsOn | EdgeType::Parent) {
                adjacency
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());
            }
        }
        tarjan_scc(&adjacency)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect()
    }

    /// Count soft dependency cycles using Tarjan's SCC.
    ///
    /// Runs Tarjan's SCC on the `SoftDependsOn` edge subgraph.
    /// Returns the count of SCCs with size > 1. Soft cycles are considered healthy
    /// (mutual reinforcement) and are counted but not flagged as errors.
    pub fn find_soft_cycle_count(&self) -> usize {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if matches!(edge.edge_type, EdgeType::SoftDependsOn) {
                adjacency
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());
            }
        }
        tarjan_scc(&adjacency)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .count()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Build undirected adjacency list from edges (deduplicated).
    fn undirected_adjacency(&self) -> HashMap<&str, Vec<&str>> {
        let mut adj: HashMap<&str, HashSet<&str>> = HashMap::new();
        for edge in &self.edges {
            adj.entry(edge.source.as_str())
                .or_default()
                .insert(edge.target.as_str());
            adj.entry(edge.target.as_str())
                .or_default()
                .insert(edge.source.as_str());
        }
        adj.into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Output formats
    // -----------------------------------------------------------------------

    /// Produce all export files. Returns list of written file paths.
    pub fn output_all_files(&self, base: &str) -> Result<Vec<String>> {
        let mut written = Vec::new();

        // Full graph JSON (dashboard views compute layouts client-side)
        let json_path = format!("{base}.json");
        std::fs::write(&json_path, self.output_json()?)?;
        written.push(json_path);

        // GraphML
        let graphml_path = format!("{base}.graphml");
        std::fs::write(&graphml_path, self.output_graphml())?;
        written.push(graphml_path);

        Ok(written)
    }

    /// Compute focus scores for all nodes.
    ///
    /// Score based on priority, deadline urgency, staleness, and downstream weight.
    /// Results are stored in node.focus_score.
    fn compute_focus_scores(nodes: &mut [GraphNode]) {
        let today = chrono::Utc::now().date_naive();
        for node in nodes.iter_mut() {
            let pri = node.priority.unwrap_or(2);
            let sev = node.severity.unwrap_or(0);
            let mut score: i64 = match pri {
                0 => 10000,
                1 => 5000,
                _ => 0,
            };
            // Severity bonus (lexicographic for SEV4)
            score += match sev {
                4 => 100000,
                3 => 20000,
                2 => 10000,
                1 => 5000,
                _ => 0,
            };
            if let Some(ref due) = node.due {
                let len = std::cmp::min(10, due.len());
                if let Ok(due_date) =
                    chrono::NaiveDate::parse_from_str(&due[..due.floor_char_boundary(len)], "%Y-%m-%d")
                {
                    let days_until = (due_date - today).num_days();
                    let effort_days = node
                        .effort
                        .as_deref()
                        .and_then(crate::graph::parse_effort_days)
                        .unwrap_or(3);

                    let mut deadline_score = if days_until < 0 {
                        8000 + std::cmp::min((-days_until) * 200, 4000)
                    } else {
                        let ratio = effort_days as f64 / (days_until.max(1) as f64);
                        if ratio >= 1.0 {
                            6000
                        } else if ratio > 0.5 {
                            // linear interpolation: 0.5 -> 2000, 1.0 -> 6000
                            2000 + ((ratio - 0.5) * 8000.0) as i64
                        } else if days_until <= 30 {
                            1000
                        } else {
                            0
                        }
                    };

                    if node.consequence.is_some() {
                        deadline_score = (deadline_score as f64 * 1.5) as i64;
                    }
                    score += deadline_score;
                }
            }
            if pri >= 2 {
                if let Some(ref created) = node.created {
                    if created.len() >= 10 {
                        if let Ok(created_dt) =
                            chrono::NaiveDate::parse_from_str(&created[..created.floor_char_boundary(10)], "%Y-%m-%d")
                        {
                            let days = (today - created_dt).num_days();
                            score += std::cmp::min(days.max(0), 200);
                        }
                    }
                }
            }
            score += (node.downstream_weight * 10.0) as i64;
            // Stakeholder waiting urgency: someone external is waiting on this task.
            // Base +2000 (someone is waiting at all), growing +200/day, capped at +8000 total.
            if node.stakeholder.is_some() {
                let anchor = node.waiting_since.as_ref().or(node.created.as_ref());
                if let Some(anchor_str) = anchor {
                    let len = std::cmp::min(10, anchor_str.len());
                    if let Ok(anchor_date) = chrono::NaiveDate::parse_from_str(
                        &anchor_str[..anchor_str.floor_char_boundary(len)],
                        "%Y-%m-%d",
                    ) {
                        let days = (today - anchor_date).num_days().max(0);
                        score += 2000 + std::cmp::min(days * 200, 6000);
                    } else {
                        score += 2000; // stakeholder set but unparseable date
                    }
                } else {
                    score += 2000; // stakeholder set but no date at all
                }
            }
            node.focus_score = Some(score);
        }
    }

    /// Compute focus picks: top ready tasks ranked by pre-computed focus_score.
    pub fn focus_picks(&self, max: usize) -> Vec<String> {
        let mut scored: Vec<(&GraphNode, i64)> = self.ready
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .map(|t| (t, t.focus_score.unwrap_or(0)))
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().take(max).map(|(t, _)| t.id.clone()).collect()
    }

    /// Full graph as JSON — all task nodes and their edges.
    pub fn output_json(&self) -> Result<String> {
        let mut nodes: Vec<GraphNode> = self.nodes.values().cloned().collect();
        nodes.sort_by(|a, b| a.label.cmp(&b.label));
        // Only include nodes with explicit task_id (real tasks, not bare notes)
        nodes.retain(|n| n.task_id.is_some());
        let placed_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let edges: Vec<_> = self.edges.iter()
            .filter(|e| placed_ids.contains(e.source.as_str()) && placed_ids.contains(e.target.as_str()))
            .cloned()
            .collect();
        let focus = self.focus_picks(50);
        let graph = OutputGraph {
            nodes,
            edges,
            ready: self.ready.clone(),
            blocked: self.blocked.clone(),
            roots: self.roots.clone(),
            focus,
        };
        Ok(serde_json::to_string_pretty(&graph)?)
    }

    pub fn output_graphml(&self) -> String {
        let mut nodes: Vec<GraphNode> = self.nodes.values().cloned().collect();
        nodes.sort_by(|a, b| a.label.cmp(&b.label));
        let graph = OutputGraph {
            nodes,
            edges: self.edges.clone(),
            ready: self.ready.clone(),
            blocked: self.blocked.clone(),
            roots: self.roots.clone(),
            focus: vec![],
        };
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<graphml xmlns="http://graphml.graphdrawing.org/xmlns"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://graphml.graphdrawing.org/xmlns http://graphml.graphdrawing.org/xmlns/1.0/graphml.xsd">
  <key id="d0" for="node" attr.name="label" attr.type="string"/>
  <key id="d1" for="node" attr.name="path" attr.type="string"/>
  <key id="d2" for="node" attr.name="tags" attr.type="string"/>
  <key id="d3" for="node" attr.name="type" attr.type="string"/>
  <key id="d4" for="node" attr.name="status" attr.type="string"/>
  <key id="d5" for="node" attr.name="priority" attr.type="int"/>
  <key id="d6" for="node" attr.name="project" attr.type="string"/>
  <key id="d7" for="node" attr.name="assignee" attr.type="string"/>
  <key id="d8" for="node" attr.name="complexity" attr.type="string"/>
  <key id="d9" for="node" attr.name="depends_on" attr.type="string"/>
  <key id="d10" for="node" attr.name="soft_depends_on" attr.type="string"/>
  <key id="d11" for="node" attr.name="blocks" attr.type="string"/>
  <key id="d12" for="node" attr.name="soft_blocks" attr.type="string"/>
  <key id="d13" for="node" attr.name="parent" attr.type="string"/>
  <key id="d14" for="node" attr.name="children" attr.type="string"/>
  <key id="d15" for="node" attr.name="due" attr.type="string"/>
  <key id="e0" for="edge" attr.name="type" attr.type="string"/>
  <graph id="G" edgedefault="directed">
"#,
        );

        for node in &graph.nodes {
            let esc = |s: &str| {
                s.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;")
            };
            let label = esc(&node.label);
            let path = esc(&node.path.to_string_lossy());
            let tags_str = node.tags.join(",");

            let mut ns = format!(
                "    <node id=\"{}\">\n      <data key=\"d0\">{}</data>\n      <data key=\"d1\">{}</data>\n      <data key=\"d2\">{}</data>\n",
                node.id, label, path, tags_str
            );

            let append = |ns: &mut String, key: &str, val: &str| {
                if !val.is_empty() {
                    ns.push_str(&format!("      <data key=\"{}\">{}</data>\n", key, val));
                }
            };

            append(&mut ns, "d3", node.node_type.as_deref().unwrap_or(""));
            append(&mut ns, "d4", node.status.as_deref().unwrap_or(""));
            if let Some(p) = node.priority {
                ns.push_str(&format!("      <data key=\"d5\">{}</data>\n", p));
            }
            append(&mut ns, "d6", "");
            append(&mut ns, "d7", node.assignee.as_deref().unwrap_or(""));
            append(&mut ns, "d8", node.complexity.as_deref().unwrap_or(""));
            append(&mut ns, "d9", &node.depends_on.join(","));
            append(&mut ns, "d10", &node.soft_depends_on.join(","));
            append(&mut ns, "d11", &node.blocks.join(","));
            append(&mut ns, "d12", &node.soft_blocks.join(","));
            append(&mut ns, "d13", node.parent.as_deref().unwrap_or(""));
            append(&mut ns, "d14", &node.children.join(","));
            append(&mut ns, "d15", node.due.as_deref().unwrap_or(""));

            ns.push_str("    </node>\n");
            xml.push_str(&ns);
        }

        for (i, edge) in graph.edges.iter().enumerate() {
            xml.push_str(&format!(
                "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n      <data key=\"e0\">{}</data>\n    </edge>\n",
                i, edge.source, edge.target, edge.edge_type.as_str()
            ));
        }

        xml.push_str("  </graph>\n</graphml>\n");
        xml
    }

}

/// Recency signal (0.0 - 1.0) based on modification date.
///
/// 1.0 if modified today, decaying exponentially with exp(-days / 30).
/// Clamps to 0.0 at or after 90 days.
fn recency_signal(modified: &DateTime<Utc>, now: &DateTime<Utc>) -> f64 {
    const MS_PER_DAY: f64 = 86_400_000.0;
    let duration = now.signed_duration_since(*modified);
    let days = duration.num_milliseconds() as f64 / MS_PER_DAY;

    if days >= 90.0 {
        return 0.0;
    }

    if days <= 0.0 {
        return 1.0;
    }

    (-days / 30.0).exp()
}

// ===========================================================================
// Internal build helpers
// ===========================================================================

/// Build all edges originating from a single node.
fn build_node_edges(
    n: &GraphNode,
    id_map: &HashMap<String, String>,
    path_to_id: &HashMap<String, String>,
    pkb_root: &Path,
) -> Vec<Edge> {
    let mut edges = Vec::new();

    // Reconstruct absolute source path for link resolution
    let abs_source = if n.path.is_absolute() {
        n.path.clone()
    } else {
        pkb_root.join(&n.path)
    };

    // Wikilinks / markdown links -> Link edges
    for link in &n.raw_links {
        if let Some(target_path) = graph::resolve_link(link, &abs_source, id_map) {
            if let Some(target_id) = path_to_id.get(&target_path) {
                if n.id != *target_id {
                    edges.push(Edge {
                        source: n.id.clone(),
                        target: target_id.clone(),
                        edge_type: EdgeType::Link,
                    });
                }
            }
        }
    }

    // Parent -> Parent edge (this -> parent)
    if let Some(ref parent_ref) = n.parent {
        if let Some(target_id) = graph::resolve_ref(parent_ref, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::Parent,
                });
            }
        }
    }

    // depends_on -> DependsOn edge (this -> dependency)
    for dep in &n.depends_on {
        if let Some(target_id) = graph::resolve_ref(dep, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::DependsOn,
                });
            }
        }
    }

    // soft_depends_on -> SoftDependsOn edge
    for dep in &n.soft_depends_on {
        if let Some(target_id) = graph::resolve_ref(dep, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::SoftDependsOn,
                });
            }
        }
    }

    // children -> Parent edge (child -> this)
    for child in &n.children {
        if let Some(target_id) = graph::resolve_ref(child, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: target_id,
                    target: n.id.clone(),
                    edge_type: EdgeType::Parent,
                });
            }
        }
    }

    // blocks -> DependsOn edge (blocked -> this, i.e. blocked depends on this)
    for blocked in &n.blocks {
        if let Some(target_id) = graph::resolve_ref(blocked, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: target_id,
                    target: n.id.clone(),
                    edge_type: EdgeType::DependsOn,
                });
            }
        }
    }

    // soft_blocks -> SoftDependsOn edge (soft-blocked -> this)
    for blocked in &n.soft_blocks {
        if let Some(target_id) = graph::resolve_ref(blocked, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: target_id,
                    target: n.id.clone(),
                    edge_type: EdgeType::SoftDependsOn,
                });
            }
        }
    }

    // supersedes -> Supersedes edge (this -> old memory)
    if let Some(ref old_id_ref) = n.supersedes {
        if let Some(target_id) = graph::resolve_ref(old_id_ref, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::Supersedes,
                });
            }
        }
    }

    edges
}

/// Compute inverse relationships on nodes from resolved edges.
///
/// For each DependsOn edge (source depends on target):
///   target.blocks += source
/// For each SoftDependsOn edge:
///   target.soft_blocks += source
/// For each Parent edge (source is child of target):
///   target.children += source
fn compute_inverses(nodes: &mut [GraphNode], edges: &[Edge]) {
    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Pre-build a set of subtask IDs (type == "subtask") for O(1) lookup
    let subtask_ids: HashSet<String> = nodes
        .iter()
        .filter(|n| n.node_type.as_deref() == Some("subtask"))
        .map(|n| n.id.clone())
        .collect();

    // Collect updates to avoid borrow issues
    let mut block_updates: Vec<(usize, String)> = Vec::new(); // (target_idx, source_id)
    let mut soft_block_updates: Vec<(usize, String)> = Vec::new();
    let mut children_updates: Vec<(usize, String)> = Vec::new();
    let mut subtask_updates: Vec<(usize, String)> = Vec::new();
    // Resolve parent field: raw frontmatter value → actual node ID
    let mut parent_updates: Vec<(usize, String)> = Vec::new(); // (child_idx, resolved_parent_id)

    for edge in edges {
        match edge.edge_type {
            EdgeType::DependsOn => {
                // source depends on target -> target blocks source
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    block_updates.push((idx, edge.source.clone()));
                }
            }
            EdgeType::SoftDependsOn => {
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    soft_block_updates.push((idx, edge.source.clone()));
                }
            }
            EdgeType::Parent => {
                // source is child of target; route to subtasks or children
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    if subtask_ids.contains(edge.source.as_str()) {
                        subtask_updates.push((idx, edge.source.clone()));
                    } else {
                        children_updates.push((idx, edge.source.clone()));
                    }
                }
                // Resolve the child's parent field to the actual target node ID
                if let Some(&child_idx) = id_to_idx.get(&edge.source) {
                    parent_updates.push((child_idx, edge.target.clone()));
                }
            }
            EdgeType::Link | EdgeType::Supersedes => {}
        }
    }

    for (idx, blocked_id) in block_updates {
        if !nodes[idx].blocks.contains(&blocked_id) {
            nodes[idx].blocks.push(blocked_id);
        }
    }
    for (idx, blocked_id) in soft_block_updates {
        if !nodes[idx].soft_blocks.contains(&blocked_id) {
            nodes[idx].soft_blocks.push(blocked_id);
        }
    }
    for (idx, child_id) in children_updates {
        if !nodes[idx].children.contains(&child_id) {
            nodes[idx].children.push(child_id);
        }
    }
    for (idx, subtask_id) in subtask_updates {
        if !nodes[idx].subtasks.contains(&subtask_id) {
            nodes[idx].subtasks.push(subtask_id);
        }
    }
    // Resolve parent fields: replace raw frontmatter references with actual node IDs.
    // This ensures node.parent matches node.id values throughout the graph,
    // so treemap hierarchy, project computation, and frontend lookups all work correctly.
    for (idx, resolved_parent_id) in parent_updates {
        nodes[idx].parent = Some(resolved_parent_id);
    }

    // Deduplicate and update leaf status (subtasks do not affect leaf status)
    for node in nodes.iter_mut() {
        deduplicate_vec(&mut node.blocks);
        deduplicate_vec(&mut node.soft_blocks);
        deduplicate_vec(&mut node.children);
        deduplicate_vec(&mut node.subtasks);
        deduplicate_vec(&mut node.depends_on);
        deduplicate_vec(&mut node.soft_depends_on);
        node.leaf = node.children.is_empty();
    }
}

/// Compute indegree and outdegree for each node.
fn compute_degree_metrics(nodes: &mut [GraphNode], edges: &[Edge]) {
    let mut out_counts: HashMap<String, i32> = HashMap::new();
    let mut in_counts: HashMap<String, i32> = HashMap::new();
    let mut backlink_counts: HashMap<String, i32> = HashMap::new();

    for edge in edges {
        *out_counts.entry(edge.source.clone()).or_insert(0) += 1;
        *in_counts.entry(edge.target.clone()).or_insert(0) += 1;
        if edge.edge_type == EdgeType::Link {
            *backlink_counts.entry(edge.target.clone()).or_insert(0) += 1;
        }
    }

    for node in nodes.iter_mut() {
        node.outdegree = *out_counts.get(&node.id).unwrap_or(&0);
        node.indegree = *in_counts.get(&node.id).unwrap_or(&0);
        node.backlink_count = *backlink_counts.get(&node.id).unwrap_or(&0);
    }
}

/// Compute PageRank and betweenness centrality.
fn compute_centrality_metrics(nodes: &mut [GraphNode], edges: &[Edge]) {
    let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let pagerank = metrics::compute_pagerank(&node_ids, edges);
    let betweenness = metrics::compute_betweenness_centrality(&node_ids, edges);

    for node in nodes.iter_mut() {
        if let Some(&pr) = pagerank.get(&node.id) {
            node.pagerank = pr;
        }
        if let Some(&bt) = betweenness.get(&node.id) {
            node.betweenness = bt;
        }
    }
}

/// Compute the `project` field for each node by walking up the parent chain
/// to find the nearest ancestor with `node_type == "project"`.
fn compute_project_field(nodes: &mut [GraphNode]) {
    // Build id -> (parent, node_type, label) lookup
    let info: HashMap<String, (Option<String>, Option<String>, String)> = nodes
        .iter()
        .map(|n| {
            (
                n.id.clone(),
                (
                    n.parent.clone(),
                    n.node_type.clone(),
                    n.label.clone(),
                ),
            )
        })
        .collect();

    for node in nodes.iter_mut() {
        // 1. If this node IS a project, its own project is its own label
        if node.node_type.as_deref() == Some("project") {
            node.project = Some(node.label.clone());
            continue;
        }

        // 2. Walk up parent chain
        let mut current = node.parent.clone();
        let mut depth = 0;
        while let Some(ref pid) = current {
            if depth > 100 {
                break; // cycle guard
            }
            if let Some((parent, ntype, label)) = info.get(pid) {
                // Ancestor is a project node
                if ntype.as_deref() == Some("project") {
                    node.project = Some(label.clone());
                    break;
                }
                current = parent.clone();
            } else {
                break;
            }
            depth += 1;
        }
    }
}

/// Compute downstream_weight and stakeholder_exposure via BFS through
/// blocks/soft_blocks. Mirrors the logic from fast-indexer main.rs.
fn compute_downstream_metrics(nodes: &mut [GraphNode]) {
    let excluded: HashSet<&str> = graph::COMPLETED_STATUSES.iter().copied().collect();

    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Pre-compute base weight for non-excluded nodes
    let base_weights: HashMap<String, f64> = nodes
        .iter()
        .filter(|n| {
            n.status
                .as_deref()
                .map(|s| !excluded.contains(s))
                .unwrap_or(false)
        })
        .map(|n| {
            let pw = match n.priority.unwrap_or(2) {
                0 => 5.0,
                1 => 3.0,
                2 => 2.0,
                3 => 1.0,
                _ => 0.5,
            };
            let dm = if n.due.is_some() { 2.0 } else { 1.0 };
            (n.id.clone(), pw * dm)
        })
        .collect();

    let has_due: HashSet<String> = nodes
        .iter()
        .filter(|n| {
            n.due.is_some()
                && n.status
                    .as_deref()
                    .map(|s| !excluded.contains(s))
                    .unwrap_or(false)
        })
        .map(|n| n.id.clone())
        .collect();

    // Snapshot blocks/soft_blocks/children to avoid borrow issues
    let blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.blocks.clone()))
        .collect();
    let soft_blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.soft_blocks.clone()))
        .collect();
    let children_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.children.clone()))
        .collect();

    let all_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();

    for start_id in &all_ids {
        let mut total_weight: f64 = 0.0;
        let mut has_stakeholder = false;
        let mut visited: HashSet<String> = HashSet::new();
        // Queue: (id, depth, edge_factor) where edge_factor < 1.0 for soft/child edges
        let mut queue: Vec<(String, u32, f64)> = Vec::new();

        let status_ok = |id: &str| -> bool {
            id_to_idx
                .get(id)
                .and_then(|&idx| nodes[idx].status.as_deref())
                .map(|s| !excluded.contains(s))
                .unwrap_or(false)
        };

        // Seed with direct blocks, soft_blocks, and children
        if let Some(blocked) = blocks_map.get(start_id) {
            for bid in blocked {
                if status_ok(bid) {
                    queue.push((bid.clone(), 1, 1.0));
                }
            }
        }
        if let Some(soft_blocked) = soft_blocks_map.get(start_id) {
            for sbid in soft_blocked {
                if status_ok(sbid) {
                    queue.push((sbid.clone(), 1, 0.3));
                }
            }
        }
        if let Some(ch) = children_map.get(start_id) {
            for cid in ch {
                if status_ok(cid) {
                    queue.push((cid.clone(), 1, 0.5));
                }
            }
        }

        while let Some((tid, depth, edge_factor)) = queue.pop() {
            if !visited.insert(tid.clone()) {
                continue;
            }
            if let Some(&bw) = base_weights.get(&tid) {
                let depth_decay = 1.0 / (depth as f64);
                total_weight += depth_decay * bw * edge_factor;
            }
            if has_due.contains(&tid) {
                has_stakeholder = true;
            }
            if let Some(next_blocks) = blocks_map.get(&tid) {
                for next in next_blocks {
                    if !visited.contains(next) {
                        queue.push((next.clone(), depth + 1, edge_factor));
                    }
                }
            }
            if let Some(next_soft) = soft_blocks_map.get(&tid) {
                for next in next_soft {
                    if !visited.contains(next) {
                        queue.push((next.clone(), depth + 1, edge_factor * 0.3));
                    }
                }
            }
            if let Some(next_ch) = children_map.get(&tid) {
                for next in next_ch {
                    if !visited.contains(next) {
                        queue.push((next.clone(), depth + 1, edge_factor * 0.5));
                    }
                }
            }
        }

        if let Some(&idx) = id_to_idx.get(start_id) {
            nodes[idx].downstream_weight = (total_weight * 100.0).round() / 100.0;
            // stakeholder_exposure: true if downstream has due-dated tasks OR this task
            // has an explicit stakeholder (someone external is waiting)
            nodes[idx].stakeholder_exposure =
                has_stakeholder || nodes[idx].stakeholder.is_some();
        }
    }
}

/// Compute effective_priority for each node: min(own priority, min priority in downstream cone).
///
/// Downstream cone = BFS through blocks, soft_blocks, children edges (skipping completed nodes).
/// A P2 blocker of a P0 child gets effective_priority=0.
fn compute_effective_priority(nodes: &mut [GraphNode]) {
    let excluded: HashSet<&str> = graph::COMPLETED_STATUSES.iter().copied().collect();

    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    let blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.blocks.clone()))
        .collect();
    let soft_blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.soft_blocks.clone()))
        .collect();
    let children_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.children.clone()))
        .collect();

    // Snapshot priorities to avoid borrow conflicts during mutation
    let priority_map: HashMap<String, i32> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.priority.unwrap_or(2)))
        .collect();
    let status_map: HashMap<String, bool> = nodes
        .iter()
        .map(|n| {
            let completed = n.status.as_deref().map(|s| excluded.contains(s)).unwrap_or(false);
            (n.id.clone(), completed)
        })
        .collect();

    let all_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();

    for start_id in &all_ids {
        let own_priority = priority_map.get(start_id.as_str()).copied().unwrap_or(2);
        let mut min_priority = own_priority;
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(start_id.clone());
        let mut queue: Vec<String> = Vec::new();

        for neighbours in [
            blocks_map.get(start_id),
            soft_blocks_map.get(start_id),
            children_map.get(start_id),
        ]
        .into_iter()
        .flatten()
        {
            for id in neighbours {
                if !status_map.get(id.as_str()).copied().unwrap_or(true) {
                    queue.push(id.clone());
                }
            }
        }

        while let Some(tid) = queue.pop() {
            if !visited.insert(tid.clone()) {
                continue;
            }
            let pri = priority_map.get(tid.as_str()).copied().unwrap_or(2);
            if pri < min_priority {
                min_priority = pri;
            }
            for neighbours in [
                blocks_map.get(&tid),
                soft_blocks_map.get(&tid),
                children_map.get(&tid),
            ]
            .into_iter()
            .flatten()
            {
                for id in neighbours {
                    if !visited.contains(id) && !status_map.get(id.as_str()).copied().unwrap_or(true) {
                        queue.push(id.clone());
                    }
                }
            }
        }

        if let Some(&idx) = id_to_idx.get(start_id) {
            nodes[idx].effective_priority = Some(min_priority);
        }
    }
}

/// Compute blocking_urgency for each node based on the status of tasks it blocks.
///
/// Algorithm:
/// - If any target has status: in_progress -> set blocking_urgency = 1.0
/// - Else if any target has status: active -> set blocking_urgency = 0.5
/// - Else -> 0.0
fn compute_blocking_urgency(nodes: &mut [GraphNode]) {
    let id_to_status: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.status.clone().unwrap_or_default()))
        .collect();

    for node in nodes.iter_mut() {
        let mut urgency = 0.0;
        for target_id in &node.blocks {
            if let Some(status) = id_to_status.get(target_id) {
                if status == "in_progress" {
                    urgency = 1.0;
                    break;
                } else if status == "active" {
                    urgency = 0.5f64.max(urgency);
                }
            }
        }
        node.blocking_urgency = urgency;
    }
}

/// Compute scope (subtree size) for each node via recursive descendant count.
///
/// Called after `compute_inverses` so `node.children` is fully populated.
/// Handles cycles in the parent-child graph gracefully via a visited set.
fn compute_scope(nodes: &mut [GraphNode]) {
    let children_map: HashMap<&str, &[String]> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.children.as_slice()))
        .collect();

    let mut scopes = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::new();
    
    for node in nodes.iter() {
        visited.clear();
        scopes.push(count_descendants(&node.id, &children_map, &mut visited) as i32);
    }
    
    for (node, scope) in nodes.iter_mut().zip(scopes) {
        node.scope = scope;
    }
}

fn count_descendants<'a>(
    id: &str,
    children_map: &HashMap<&str, &'a [String]>,
    visited: &mut HashSet<&'a str>,
) -> usize {
    let children = match children_map.get(id) {
        Some(c) if !c.is_empty() => *c,
        _ => return 0,
    };
    let mut count = 0;
    for child_id in children {
        if visited.insert(child_id.as_str()) {
            count += 1 + count_descendants(child_id.as_str(), children_map, visited);
        }
    }
    count
}

/// Compute uncertainty [0.0–1.0] for each node.
///
/// Composite of: missing acceptance criteria, unresolved scope (has children),
/// unresolved hard dependencies, and sparse body content.
/// If `node.confidence` is set, it overrides: uncertainty = 1.0 - confidence.
///
/// Called after `compute_inverses` (children populated) and node statuses are final.
fn compute_uncertainty(nodes: &mut [GraphNode]) {
    // Snapshot dep statuses to avoid borrow conflict
    let status_map: HashMap<&str, Option<&str>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.status.as_deref()))
        .collect();

    let mut uncertainties = Vec::with_capacity(nodes.len());

    for node in nodes.iter() {
        // Confidence override: user explicitly rated confidence
        if let Some(conf) = node.confidence {
            uncertainties.push((1.0 - conf).clamp(0.0, 1.0));
            continue;
        }

        let mut u = 0.0_f64;

        // No acceptance criteria → unclear completion condition
        if !node.has_acceptance_criteria {
            u += 0.30;
        }

        // Has children → scope not yet fully resolved
        if !node.children.is_empty() {
            u += 0.15;
        }

        // Unresolved hard dependencies → completion path still open
        if !node.depends_on.is_empty() {
            let resolved = node
                .depends_on
                .iter()
                .filter(|dep_id| {
                    let status = status_map.get(dep_id.as_str()).copied().flatten();
                    graph::is_completed(status)
                })
                .count();
            let ratio = resolved as f64 / node.depends_on.len() as f64;
            u += (1.0 - ratio) * 0.25;
        }

        // Sparse body → little context available
        let body_score = (node.word_count as f64 / 100.0).min(1.0);
        u += (1.0 - body_score) * 0.10;

        uncertainties.push(u.clamp(0.0, 1.0));
    }

    for (node, u) in nodes.iter_mut().zip(uncertainties) {
        node.uncertainty = u;
    }
}

/// Compute criticality [0.0–1.0] for each node, normalized across the graph.
///
/// Raw score = downstream_weight + (pagerank × 10) + stakeholder_exposure bonus.
/// Divides each raw score by the graph-wide maximum to get a normalized value.
///
/// Called after `compute_downstream_metrics` and `compute_centrality_metrics`.
fn compute_criticality(nodes: &mut [GraphNode]) {
    let raws: Vec<f64> = nodes
        .iter()
        .map(|n| {
            n.downstream_weight
                + n.pagerank * 10.0
                + if n.stakeholder_exposure { 3.0 } else { 0.0 }
        })
        .collect();

    let max = raws.iter().cloned().fold(0.0_f64, f64::max);

    for (node, &raw) in nodes.iter_mut().zip(raws.iter()) {
        node.criticality = if max > 0.0 {
            (raw / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

/// Classify tasks into ready/blocked lists and compute roots.
fn classify_tasks(
    nodes: &HashMap<String, GraphNode>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let completed_ids: HashSet<String> = nodes
        .iter()
        .filter(|(_, n)| graph::is_completed(n.status.as_deref()))
        .map(|(id, _)| id.clone())
        .collect();

    // Phase 1: identify directly blocked tasks (unmet deps or explicit status)
    let mut directly_blocked: HashSet<String> = HashSet::new();
    let mut task_ids: Vec<String> = Vec::new();

    for (id, node) in nodes {
        if node.task_id.is_none() {
            continue;
        }
        let status = node.status.as_deref().unwrap_or("active");
        if graph::is_completed(Some(status)) {
            continue;
        }
        task_ids.push(id.clone());

        let has_unmet = node.depends_on.iter().any(|d| !completed_ids.contains(d));
        if has_unmet || status == "blocked" {
            directly_blocked.insert(id.clone());
        }
    }

    // Phase 2: transitively propagate blocked status via BFS through blocks edges.
    let effectively_blocked = {
        let mut blocked_set = directly_blocked.clone();
        let mut queue: std::collections::VecDeque<String> = directly_blocked.into_iter().collect();
        while let Some(blocked_id) = queue.pop_front() {
            if let Some(node) = nodes.get(&blocked_id) {
                for downstream_id in &node.blocks {
                    if blocked_set.insert(downstream_id.clone()) {
                        queue.push_back(downstream_id.clone());
                    }
                }
            }
        }
        blocked_set
    };

    let mut ready: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();

    for id in &task_ids {
        let node = nodes.get(id).unwrap();
        if effectively_blocked.contains(id) {
            blocked.push(id.clone());
        } else if node.leaf && matches!(node.status.as_deref().unwrap_or("ready"), "ready" | "queued") {
            // Only claimable types — epics/projects/goals/containers are graph structure, not work items
            if CLAIMABLE_TYPES.contains(&node.node_type.as_deref().unwrap_or("")) {
                ready.push(id.clone());
            }
        }
    }

    // Sort ready by severity DESC (lexicographic short-circuit), then effective_priority (propagated),
    // then downstream_weight DESC, then order, then title
    ready.sort_by(|a, b| {
        let na = nodes.get(a).unwrap();
        let nb = nodes.get(b).unwrap();
        nb.severity
            .unwrap_or(0)
            .cmp(&na.severity.unwrap_or(0))
            .then(
                na.effective_priority
                    .unwrap_or(2)
                    .cmp(&nb.effective_priority.unwrap_or(2)),
            )
            .then(
                nb.downstream_weight
                    .partial_cmp(&na.downstream_weight)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(na.order.cmp(&nb.order))
            .then(na.label.cmp(&nb.label))
    });

    // Roots: tasks with no parent or parent not in index
    let roots: Vec<String> = nodes
        .iter()
        .filter(|(_, n)| n.task_id.is_some())
        .filter(|(_, n)| match &n.parent {
            None => true,
            Some(pid) => !nodes.contains_key(pid),
        })
        .map(|(id, _)| id.clone())
        .collect();

    (ready, blocked, roots)
}

/// Mark nodes reachable from active leaf tasks via upstream BFS.
///
/// Algorithm (matches Python `filter_reachable` in `scripts/task_graph.py`):
/// 1. Identify leaves: unfinished actionable-type nodes with explicit status
///    and no unfinished children.
/// 2. BFS upstream through parent, depends_on, soft_depends_on edges
///    (ignoring link/wikilink to prevent false positives).
/// 3. Set `reachable = true` on all visited nodes.
/// Compute the reachable set: BFS upstream from active leaf tasks.
///
/// Works on a node slice (used before layout in the build pipeline).
/// Returns the set of reachable node IDs so the caller can both mark nodes
/// and pass the set to layout algorithms.
fn find_reachable_set(nodes: &[GraphNode], edges: &[Edge]) -> HashSet<String> {
    let all_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    let unfinished_ids: HashSet<&str> = nodes
        .iter()
        .filter(|n| !graph::is_completed(n.status.as_deref()))
        .map(|n| n.id.as_str())
        .collect();

    // Build children mapping
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in nodes {
        // From node.parent (child → parent)
        if let Some(ref parent) = node.parent {
            if all_ids.contains(parent.as_str()) {
                children_of
                    .entry(parent.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }
        // From node.children (parent → child)
        for child_id in &node.children {
            if all_ids.contains(child_id.as_str()) {
                children_of
                    .entry(node.id.as_str())
                    .or_default()
                    .push(child_id.as_str());
            }
        }
    }

    // Identify leaves: unfinished, actionable type, explicit status,
    // no unfinished children
    let mut leaf_ids: Vec<&str> = Vec::new();
    for node in nodes {
        if !unfinished_ids.contains(node.id.as_str()) {
            continue;
        }
        if node.status.is_none() {
            continue;
        }
        let is_actionable = node
            .node_type
            .as_deref()
            .map(|t| ACTIONABLE_TYPES.contains(&t))
            .unwrap_or(false);
        if !is_actionable {
            continue;
        }
        let has_unfinished_child = children_of
            .get(node.id.as_str())
            .map(|kids| kids.iter().any(|c| unfinished_ids.contains(c)))
            .unwrap_or(false);
        if !has_unfinished_child {
            leaf_ids.push(node.id.as_str());
        }
    }

    // Build upstream adjacency from edges (parent, depends_on, soft_depends_on only)
    let mut upstream_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        match edge.edge_type {
            EdgeType::Parent | EdgeType::DependsOn | EdgeType::SoftDependsOn => {
                if all_ids.contains(edge.target.as_str()) {
                    upstream_of
                        .entry(edge.source.as_str())
                        .or_default()
                        .push(edge.target.as_str());
                }
            }
            _ => {}
        }
    }

    // BFS upstream from leaves
    let mut reachable: HashSet<String> = leaf_ids.into_iter().map(|s| s.to_string()).collect();
    let mut frontier: VecDeque<String> = reachable.iter().cloned().collect();
    while let Some(nid) = frontier.pop_front() {
        if let Some(upstreams) = upstream_of.get(nid.as_str()) {
            for &upstream_id in upstreams {
                if reachable.insert(upstream_id.to_string()) {
                    frontier.push_back(upstream_id.to_string());
                }
            }
        }
    }

    reachable
}

/// Build a resolution map for flexible node lookup.
///
/// Maps multiple lowercase keys to canonical node IDs:
/// - node.id
/// - node.task_id (if present)
/// - filename stem from node.path
/// - node.label (title)
fn build_resolution_map(nodes: &HashMap<String, GraphNode>) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for (_, node) in nodes {
        let id = &node.id;

        // Canonical ID (lowercase)
        map.entry(id.to_lowercase()).or_insert_with(|| id.clone());

        // Task ID
        if let Some(ref tid) = node.task_id {
            map.entry(tid.to_lowercase()).or_insert_with(|| id.clone());
        }

        // Filename stem
        if let Some(stem) = node.path.file_stem() {
            map.entry(stem.to_string_lossy().to_lowercase())
                .or_insert_with(|| id.clone());
        }

        // Title / label
        let label_key = node.label.to_lowercase();
        if !label_key.is_empty() {
            map.entry(label_key).or_insert_with(|| id.clone());
        }
    }
    map
}

// ===========================================================================
// Tarjan's SCC algorithm
// ===========================================================================

/// Run Tarjan's strongly connected components (SCC) algorithm on a directed graph.
///
/// `adjacency` maps node_id → list of successor node_ids.
/// Returns all SCCs as `Vec<Vec<node_id>>`. SCCs of size 1 are non-cycling nodes;
/// SCCs of size > 1 represent actual cycles.
///
/// Uses an iterative implementation to avoid stack overflow on large graphs.
pub fn tarjan_scc(adjacency: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    // Collect all node IDs (sources and targets)
    let mut all_node_ids: HashSet<String> = HashSet::new();
    for (k, vs) in adjacency {
        all_node_ids.insert(k.clone());
        for v in vs {
            all_node_ids.insert(v.clone());
        }
    }
    let mut all_nodes: Vec<String> = all_node_ids.into_iter().collect();
    all_nodes.sort(); // deterministic ordering

    let n = all_nodes.len();
    let node_to_idx: HashMap<&str, usize> = all_nodes
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    // Convert adjacency to integer form
    let adj: Vec<Vec<usize>> = all_nodes
        .iter()
        .map(|v| {
            adjacency
                .get(v.as_str())
                .map(|ws| {
                    ws.iter()
                        .filter_map(|w| node_to_idx.get(w.as_str()).copied())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    let undef = usize::MAX;
    let mut index_counter = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack = vec![false; n];
    let mut index = vec![undef; n];
    let mut lowlink = vec![0usize; n];
    let mut sccs: Vec<Vec<String>> = Vec::new();

    // Iterative Tarjan's: work stack entries = (node_idx, next_neighbor_to_process)
    let mut call_stack: Vec<(usize, usize)> = Vec::new();

    for start in 0..n {
        if index[start] != undef {
            continue;
        }
        call_stack.push((start, 0));

        'outer: while !call_stack.is_empty() {
            let (v, ni) = *call_stack.last().unwrap();

            // First visit: assign index/lowlink, push onto SCC stack
            if ni == 0 {
                index[v] = index_counter;
                lowlink[v] = index_counter;
                index_counter += 1;
                stack.push(v);
                on_stack[v] = true;
            }

            // Advance through neighbors starting at ni
            let mut cursor = ni;
            while cursor < adj[v].len() {
                let w = adj[v][cursor];
                cursor += 1;
                // Update the position for when we resume v
                call_stack.last_mut().unwrap().1 = cursor;

                if index[w] == undef {
                    // Unvisited neighbor — recurse into it
                    call_stack.push((w, 0));
                    continue 'outer;
                } else if on_stack[w] {
                    // Back edge — update lowlink
                    lowlink[v] = lowlink[v].min(index[w]);
                }
                // Already in a completed SCC — skip
            }

            // All neighbors processed — pop v
            call_stack.pop();

            // Propagate lowlink to parent
            if let Some(&(parent, _)) = call_stack.last() {
                lowlink[parent] = lowlink[parent].min(lowlink[v]);
            }

            // If v is an SCC root, pop the SCC from the stack
            if lowlink[v] == index[v] {
                let mut scc = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w] = false;
                    scc.push(all_nodes[w].clone());
                    if w == v {
                        break;
                    }
                }
                sccs.push(scc);
            }
        }
    }

    sccs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkb::PkbDocument;
    use std::path::PathBuf;

    #[test]
    fn test_recency_signal() {
        use chrono::Duration;

        let now = Utc::now();

        // (a) modified today -> ~1.0
        let today = now - Duration::try_minutes(5).unwrap();
        let s_today = recency_signal(&today, &now);
        assert!(s_today > 0.99 && s_today <= 1.0, "Today signal should be ~1.0, got {}", s_today);

        // (b) modified 30 days ago -> ~0.37 (exp(-1))
        let thirty_days_ago = now - Duration::try_days(30).unwrap();
        let s_30 = recency_signal(&thirty_days_ago, &now);
        let expected_30 = (-1.0f64).exp();
        assert!((s_30 - expected_30).abs() < 0.0001, "30 days ago signal should be ~0.37, got {}", s_30);

        // (c) modified 90+ days ago -> 0.0
        let ninety_days_ago = now - Duration::try_days(90).unwrap();
        let s_90 = recency_signal(&ninety_days_ago, &now);
        assert_eq!(s_90, 0.0, "90 days ago signal should be 0.0");

        let old = now - Duration::try_days(120).unwrap();
        let s_old = recency_signal(&old, &now);
        assert_eq!(s_old, 0.0, "120 days ago signal should be 0.0");

        // Extra: future date (clock skew) -> 1.0
        let future = now + Duration::try_hours(1).unwrap();
        let s_future = recency_signal(&future, &now);
        assert_eq!(s_future, 1.0, "Future signal should be 1.0");
    }

    /// Helper: create a PkbDocument with frontmatter for graph building.
    fn make_doc(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
    ) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), serde_json::json!(title));
        fm.insert("type".to_string(), serde_json::json!(doc_type));
        fm.insert("status".to_string(), serde_json::json!(status));
        fm.insert("id".to_string(), serde_json::json!(id));
        if let Some(p) = parent {
            fm.insert("parent".to_string(), serde_json::json!(p));
        }
        if !depends_on.is_empty() {
            fm.insert("depends_on".to_string(), serde_json::json!(depends_on));
        }

        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
        }
    }

    /// Build a small test graph:
    ///   epic-1 (parent of task-a, task-b)
    ///   task-a depends on task-b
    ///   task-c depends on task-a
    fn build_test_graph() -> GraphStore {
        let docs = vec![
            make_doc(
                "tasks/epic-1.md",
                "Epic One",
                "epic",
                "active",
                "epic-1",
                None,
                &[],
            ),
            make_doc(
                "tasks/task-a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                Some("epic-1"),
                &["task-b"],
            ),
            make_doc(
                "tasks/task-b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                Some("epic-1"),
                &[],
            ),
            make_doc(
                "tasks/task-c.md",
                "Task C",
                "task",
                "active",
                "task-c",
                None,
                &["task-a"],
            ),
        ];
        // Use an empty temp dir as pkb_root since we use relative paths
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    // ── effective_priority ──

    /// Build a graph to test priority propagation:
    ///   blocker (P2, active) --blocks--> p0-task (P0, active)
    ///   epic-p0 (P2) with child p0-task (P0, active)
    ///   unrelated (P3, active) -- standalone
    fn build_priority_test_graph() -> GraphStore {
        let mut make_with_priority = |path: &str, title: &str, id: &str, priority: i32, status: &str, parent: Option<&str>, depends_on: &[&str]| -> PkbDocument {
            let mut fm = serde_json::Map::new();
            fm.insert("title".to_string(), serde_json::json!(title));
            fm.insert("type".to_string(), serde_json::json!("task"));
            fm.insert("status".to_string(), serde_json::json!(status));
            fm.insert("id".to_string(), serde_json::json!(id));
            fm.insert("priority".to_string(), serde_json::json!(priority));
            if let Some(p) = parent {
                fm.insert("parent".to_string(), serde_json::json!(p));
            }
            if !depends_on.is_empty() {
                fm.insert("depends_on".to_string(), serde_json::json!(depends_on));
            }
            PkbDocument {
                path: std::path::PathBuf::from(path),
                title: title.to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some(status.to_string()),
                modified: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm)),
                content_hash: "test".to_string(),
            }
        };

        let docs = vec![
            // p0-task: P0, blocked by blocker
            make_with_priority("tasks/p0-task.md", "P0 Task", "p0-task", 0, "active", Some("epic-p0"), &["blocker"]),
            // blocker: P2, ready (no deps), blocks p0-task
            make_with_priority("tasks/blocker.md", "Blocker Task", "blocker", 2, "active", None, &[]),
            // epic-p0: P2 epic, parent of p0-task
            make_with_priority("tasks/epic-p0.md", "Epic P0", "epic-p0", 2, "active", None, &[]),
            // unrelated: P3, no connections
            make_with_priority("tasks/unrelated.md", "Unrelated Task", "unrelated", 3, "active", None, &[]),
        ];
        GraphStore::build(&docs, std::path::Path::new("/tmp/test-priority-pkb"))
    }

    #[test]
    fn test_compute_blocking_urgency() {
        let mut nodes = vec![
            GraphNode {
                id: "blocker-1".to_string(),
                blocks: vec!["target-in-progress".to_string()],
                ..Default::default()
            },
            GraphNode {
                id: "blocker-2".to_string(),
                blocks: vec!["target-active".to_string()],
                ..Default::default()
            },
            GraphNode {
                id: "blocker-3".to_string(),
                blocks: vec!["target-done".to_string()],
                ..Default::default()
            },
            GraphNode {
                id: "target-in-progress".to_string(),
                status: Some("in_progress".to_string()),
                ..Default::default()
            },
            GraphNode {
                id: "target-active".to_string(),
                status: Some("active".to_string()),
                ..Default::default()
            },
            GraphNode {
                id: "target-done".to_string(),
                status: Some("done".to_string()),
                ..Default::default()
            },
            GraphNode {
                id: "blocker-both".to_string(),
                blocks: vec!["target-in-progress".to_string(), "target-active".to_string()],
                ..Default::default()
            },
        ];

        compute_blocking_urgency(&mut nodes);

        let n1 = nodes.iter().find(|n| n.id == "blocker-1").unwrap();
        assert_eq!(n1.blocking_urgency, 1.0);

        let n2 = nodes.iter().find(|n| n.id == "blocker-2").unwrap();
        assert_eq!(n2.blocking_urgency, 0.5);

        let n3 = nodes.iter().find(|n| n.id == "blocker-3").unwrap();
        assert_eq!(n3.blocking_urgency, 0.0);

        let n_both = nodes.iter().find(|n| n.id == "blocker-both").unwrap();
        assert_eq!(n_both.blocking_urgency, 1.0);
    }

    #[test]
    fn test_effective_priority_blocker_inherits_from_downstream() {
        let graph = build_priority_test_graph();
        // blocker (P2) blocks p0-task (P0) → blocker's effective_priority should be 0
        let blocker = graph.resolve("blocker").expect("blocker not found");
        assert_eq!(
            blocker.effective_priority, Some(0),
            "blocker should inherit P0 from downstream p0-task"
        );
    }

    #[test]
    fn test_effective_priority_epic_inherits_from_child() {
        let graph = build_priority_test_graph();
        // epic-p0 (P2) is parent of p0-task (P0) → epic's effective_priority should be 0
        let epic = graph.resolve("epic-p0").expect("epic-p0 not found");
        assert_eq!(
            epic.effective_priority, Some(0),
            "epic should inherit P0 from its child p0-task"
        );
    }

    #[test]
    fn test_effective_priority_unrelated_unchanged() {
        let graph = build_priority_test_graph();
        let unrelated = graph.resolve("unrelated").expect("unrelated not found");
        assert_eq!(
            unrelated.effective_priority, Some(3),
            "unrelated task should keep its own priority"
        );
    }

    #[test]
    fn test_effective_priority_own_p0_stays_zero() {
        let graph = build_priority_test_graph();
        let p0 = graph.resolve("p0-task").expect("p0-task not found");
        assert_eq!(p0.effective_priority, Some(0));
    }

    // ── resolve ──

    #[test]
    fn test_resolve_by_exact_id() {
        let graph = build_test_graph();
        let node = graph.resolve("task-a");
        assert!(node.is_some());
        assert_eq!(node.unwrap().label, "Task A");
    }

    #[test]
    fn test_resolve_by_label_case_insensitive() {
        let graph = build_test_graph();
        let node = graph.resolve("task a");
        assert!(node.is_some());
        assert_eq!(node.unwrap().id.contains("task"), true);
    }

    #[test]
    fn test_resolve_by_filename_stem() {
        let graph = build_test_graph();
        let node = graph.resolve("task-b");
        assert!(node.is_some());
    }

    #[test]
    fn test_resolve_nonexistent() {
        let graph = build_test_graph();
        assert!(graph.resolve("ghost").is_none());
    }

    #[test]
    fn test_rebuild_from_nodes() {
        let graph = build_test_graph();
        let nodes = graph.nodes_cloned();
        let root = Path::new("/tmp/test-pkb");

        // Rebuild from same nodes
        let graph2 = GraphStore::rebuild_from_nodes(nodes.clone(), root);

        assert_eq!(graph.nodes.len(), graph2.nodes.len());
        assert_eq!(graph.edges.len(), graph2.edges.len());
        assert_eq!(graph.ready.len(), graph2.ready.len());

        // Verify a specific metric
        let node_a = graph.get_node("task-a").unwrap();
        let node_a2 = graph2.get_node("task-a").unwrap();
        assert_eq!(node_a.downstream_weight, node_a2.downstream_weight);
    }

    #[test]
    fn test_rebuild_from_nodes_incremental_update() {
        let graph = build_test_graph();
        let mut nodes = graph.nodes_cloned();
        let root = Path::new("/tmp/test-pkb");

        // Update task-b to be done
        let mut task_b = nodes.get("task-b").unwrap().clone();
        task_b.status = Some("done".to_string());
        nodes.insert("task-b".to_string(), task_b);

        let graph2 = GraphStore::rebuild_from_nodes(nodes, root);

        // Now task-a should be ready (it was blocked by task-b)
        assert!(graph2.ready.iter().any(|id| id == "task-a"));
        assert!(!graph.ready.iter().any(|id| id == "task-a"));
    }


    // ── dependency_tree ──

    #[test]
    fn test_dependency_tree_direct() {
        let graph = build_test_graph();
        // task-a depends on task-b
        // Find node ID for task-a (may be computed hash)
        let task_a = graph.resolve("task-a").expect("task-a not found");
        let tree = graph.dependency_tree(&task_a.id);
        // Should include task-b at depth 1
        assert!(!tree.is_empty());
        let task_b_id = graph.resolve("task-b").unwrap().id.clone();
        assert!(tree
            .iter()
            .any(|(id, depth)| id == &task_b_id && *depth == 1));
    }

    #[test]
    fn test_dependency_tree_transitive() {
        let graph = build_test_graph();
        // task-c depends on task-a, which depends on task-b
        let task_c = graph.resolve("task-c").expect("task-c not found");
        let tree = graph.dependency_tree(&task_c.id);
        // Should include task-a at depth 1 and task-b at depth 2
        let task_a_id = graph.resolve("task-a").unwrap().id.clone();
        let task_b_id = graph.resolve("task-b").unwrap().id.clone();
        assert!(tree.iter().any(|(id, _)| id == &task_a_id));
        assert!(tree.iter().any(|(id, _)| id == &task_b_id));
    }

    #[test]
    fn test_dependency_tree_empty() {
        let graph = build_test_graph();
        // task-b has no dependencies
        let task_b = graph.resolve("task-b").expect("task-b not found");
        let tree = graph.dependency_tree(&task_b.id);
        assert!(tree.is_empty());
    }

    // ── blocks_tree ──

    #[test]
    fn test_blocks_tree_direct() {
        let graph = build_test_graph();
        // task-b is depended upon by task-a, so task-b blocks task-a
        let task_b = graph.resolve("task-b").expect("task-b not found");
        let tree = graph.blocks_tree(&task_b.id);
        let task_a_id = graph.resolve("task-a").unwrap().id.clone();
        assert!(tree
            .iter()
            .any(|(id, depth)| id == &task_a_id && *depth == 1));
    }

    #[test]
    fn test_blocks_tree_transitive() {
        let graph = build_test_graph();
        // task-b blocks task-a, task-a blocks task-c
        let task_b = graph.resolve("task-b").expect("task-b not found");
        let tree = graph.blocks_tree(&task_b.id);
        let task_c_id = graph.resolve("task-c").unwrap().id.clone();
        assert!(tree.iter().any(|(id, _)| id == &task_c_id));
    }

    #[test]
    fn test_blocks_tree_leaf_empty() {
        let graph = build_test_graph();
        // task-c blocks nothing
        let task_c = graph.resolve("task-c").expect("task-c not found");
        let tree = graph.blocks_tree(&task_c.id);
        assert!(tree.is_empty());
    }

    // ── parent/children relationships ──

    #[test]
    fn test_parent_child_relationships() {
        let graph = build_test_graph();
        let epic = graph.resolve("epic-1").expect("epic-1 not found");
        // epic-1 should have task-a and task-b as children
        assert!(epic.children.len() >= 2);
    }

    // ── node_count / edge_count ──

    #[test]
    fn test_graph_counts() {
        let graph = build_test_graph();
        assert_eq!(graph.node_count(), 4);
        // Edges: task-a->task-b (depends_on), task-c->task-a (depends_on),
        //        task-a->epic-1 (parent), task-b->epic-1 (parent)
        assert!(graph.edge_count() >= 4);
    }

    // ── ready/blocked classification ──

    #[test]
    fn test_ready_tasks() {
        let graph = build_test_graph();
        let ready = graph.ready_tasks();
        // task-b should be ready (no deps, leaf, active)
        let task_b_id = graph.resolve("task-b").unwrap().id.clone();
        assert!(ready.iter().any(|n| n.id == task_b_id));
    }

    #[test]
    fn test_ready_excludes_container_types() {
        // Epics, projects are containers — not claimable work items.
        // With unified type system: only "task" is claimable; bug/feature/action
        // are now type=task with a classification field.
        let docs = vec![
            make_doc("tasks/epic-1.md", "Lone Epic", "epic", "active", "epic-1", None, &[]),
            make_doc("tasks/proj-1.md", "Lone Project", "project", "active", "proj-1", None, &[]),
            make_doc("tasks/task-1.md", "Task One", "task", "active", "task-1", None, &[]),
            make_doc("tasks/task-2.md", "Task Two", "task", "active", "task-2", None, &[]),
            make_doc("tasks/learn-1.md", "Learn One", "learn", "active", "learn-1", None, &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let ready = graph.ready_tasks();
        let ready_ids: Vec<&str> = ready.iter().map(|n| n.id.as_str()).collect();
        assert!(!ready_ids.contains(&"epic-1"), "epics must not appear in ready");
        assert!(!ready_ids.contains(&"proj-1"), "projects must not appear in ready");
        assert!(!ready_ids.contains(&"learn-1"), "learn must not appear in ready");
        assert!(ready_ids.contains(&"task-1"), "task must be in ready");
        assert!(ready_ids.contains(&"task-2"), "task must be in ready");
    }

    #[test]
    fn test_ready_excludes_inbox_tasks() {
        // Inbox tasks are not yet promoted to the ready queue — they need
        // review (AC / estimate / priority) before they become actionable.
        let docs = vec![
            make_doc("tasks/task-inbox.md", "Inbox Task", "task", "inbox", "task-inbox", None, &[]),
            make_doc("tasks/task-active.md", "Active Task", "task", "active", "task-active", None, &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let ready_ids: Vec<&str> = graph.ready_tasks().iter().map(|n| n.id.as_str()).collect();
        assert!(!ready_ids.contains(&"task-inbox"), "inbox tasks must not appear in ready");
        assert!(ready_ids.contains(&"task-active"), "active tasks must appear in ready");
    }

    #[test]
    fn test_blocked_tasks() {
        let graph = build_test_graph();
        let blocked = graph.blocked_tasks();
        // task-a should be blocked (depends on task-b which isn't done)
        let task_a_id = graph.resolve("task-a").unwrap().id.clone();
        assert!(blocked.iter().any(|n| n.id == task_a_id));
    }

    // ── reachable ──

    #[test]
    fn test_reachable_marks_leaves_and_ancestors() {
        let graph = build_test_graph();
        // Test graph: epic-1 (parent of task-a, task-b)
        //   task-b is a leaf (active, no deps, no unfinished children)
        //   task-a depends on task-b, so task-a is blocked but still unfinished
        //   task-c depends on task-a, no parent
        //   All are active task-type nodes → all are reachable seeds or upstream

        let task_b = graph.resolve("task-b").unwrap();
        assert!(task_b.reachable, "task-b should be reachable (leaf)");

        let task_a = graph.resolve("task-a").unwrap();
        assert!(task_a.reachable, "task-a should be reachable (leaf with unmet dep)");

        let epic_1 = graph.resolve("epic-1").unwrap();
        assert!(epic_1.reachable, "epic-1 should be reachable (parent of leaves)");

        let task_c = graph.resolve("task-c").unwrap();
        assert!(task_c.reachable, "task-c should be reachable (leaf with dep)");
    }

    #[test]
    fn test_reachable_excludes_done_orphans() {
        // A completed node with no active descendants should NOT be reachable
        let docs = vec![
            make_doc("tasks/done-task.md", "Done Task", "task", "done", "done-task", None, &[]),
            make_doc("tasks/active-task.md", "Active Task", "task", "active", "active-task", None, &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let done = graph.resolve("done-task").unwrap();
        assert!(!done.reachable, "completed orphan should not be reachable");

        let active = graph.resolve("active-task").unwrap();
        assert!(active.reachable, "active leaf should be reachable");
    }

    #[test]
    fn test_reachable_includes_done_ancestor() {
        // A completed node that is parent of an active leaf SHOULD be reachable
        let docs = vec![
            make_doc("tasks/done-parent.md", "Done Parent", "project", "done", "done-parent", None, &[]),
            make_doc("tasks/active-child.md", "Active Child", "task", "active", "active-child", Some("done-parent"), &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let parent = graph.resolve("done-parent").unwrap();
        assert!(parent.reachable, "done parent of active leaf should be reachable");
    }

    #[test]
    fn test_reachable_excludes_notes() {
        // Notes without status should not seed BFS
        let docs = vec![
            make_doc("notes/my-note.md", "My Note", "note", "", "my-note", None, &[]),
            make_doc("tasks/my-task.md", "My Task", "task", "active", "my-task", None, &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let note = graph.resolve("my-note").unwrap();
        assert!(!note.reachable, "note without status should not be reachable");
    }

    // ── subtask relationships ──

    #[test]
    fn test_subtasks_separate_from_children() {
        // A parent task with both a regular child and a subtask:
        // - the subtask must appear in parent.subtasks, NOT in parent.children
        // - parent.leaf must remain true (subtasks don't affect leaf status)
        let docs = vec![
            make_doc("tasks/parent.md", "Parent Task", "task", "active", "parent-abc", None, &[]),
            make_doc(
                "tasks/parent-abc.1.md",
                "Subtask One",
                "subtask",
                "active",
                "parent-abc.1",
                Some("parent-abc"),
                &[],
            ),
            make_doc(
                "tasks/child.md",
                "Child Task",
                "task",
                "active",
                "child-xyz",
                Some("parent-abc"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let parent = graph.resolve("parent-abc").expect("parent not found");
        let subtask = graph.resolve("parent-abc.1").expect("subtask not found");

        // Subtask must be in parent.subtasks, not parent.children
        assert!(
            parent.subtasks.contains(&subtask.id),
            "parent.subtasks should contain the subtask"
        );
        assert!(
            !parent.children.contains(&subtask.id),
            "parent.children must not contain the subtask"
        );

        // Regular child must be in parent.children
        let child = graph.resolve("child-xyz").expect("child not found");
        assert!(
            parent.children.contains(&child.id),
            "parent.children should contain the regular child"
        );

        // Parent with only subtasks (no regular children) should remain a leaf
        let docs_subtask_only = vec![
            make_doc("tasks/parent2.md", "Parent 2", "task", "active", "parent-def", None, &[]),
            make_doc(
                "tasks/parent-def.1.md",
                "Only Subtask",
                "subtask",
                "active",
                "parent-def.1",
                Some("parent-def"),
                &[],
            ),
        ];
        let graph2 = GraphStore::build(&docs_subtask_only, Path::new("/tmp/test-pkb"));
        let parent2 = graph2.resolve("parent-def").expect("parent2 not found");
        assert!(parent2.leaf, "parent with only subtasks should still be a leaf");
    }

    /// Helper: create a PkbDocument with soft_depends_on frontmatter.
    fn make_doc_with_soft_dep(path: &str, title: &str, id: &str, soft_deps: &[&str]) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), serde_json::json!(title));
        fm.insert("type".to_string(), serde_json::json!("task"));
        fm.insert("status".to_string(), serde_json::json!("active"));
        fm.insert("id".to_string(), serde_json::json!(id));
        if !soft_deps.is_empty() {
            fm.insert("soft_depends_on".to_string(), serde_json::json!(soft_deps));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
        }
    }

    #[test]
    fn test_parse_effort_days() {
        assert_eq!(crate::graph::parse_effort_days("1d"), Some(1));
        assert_eq!(crate::graph::parse_effort_days("1w"), Some(7));
        assert_eq!(crate::graph::parse_effort_days("2h"), Some(1));
        assert_eq!(crate::graph::parse_effort_days("10h"), Some(2));
        assert_eq!(crate::graph::parse_effort_days("5"), Some(5));
        assert_eq!(crate::graph::parse_effort_days(""), None);
    }

    #[test]
    fn test_focus_scoring_scenarios() {
        use crate::graph::GraphNode;
        use chrono::Utc;

        let today = Utc::now().date_naive();
        let tomorrow = today + chrono::Duration::days(1);
        let in_5d = today + chrono::Duration::days(5);
        let in_7d = today + chrono::Duration::days(7);
        let in_2w = today + chrono::Duration::days(14);
        let in_4w = today + chrono::Duration::days(28);

        // Scenario 1: Corporate card (effort=1d, due in 7d): ratio=1/7=0.14, +1000
        let mut node1 = GraphNode::default();
        node1.due = Some(in_7d.format("%Y-%m-%d").to_string());
        node1.effort = Some("1d".to_string());

        // Scenario 2: Corporate card (effort=1d, due tomorrow): ratio=1/1=1.0, +6000
        let mut node2 = GraphNode::default();
        node2.due = Some(tomorrow.format("%Y-%m-%d").to_string());
        node2.effort = Some("1d".to_string());

        // Scenario 3: Paper review (effort=3w, due in 4w): ratio=21/28=0.75, ~+4000
        let mut node3 = GraphNode::default();
        node3.due = Some(in_4w.format("%Y-%m-%d").to_string());
        node3.effort = Some("3w".to_string());

        // Scenario 4: Paper review (effort=3w, due in 2w): ratio=21/14=1.5 -> 1.0, +6000
        let mut node4 = GraphNode::default();
        node4.due = Some(in_2w.format("%Y-%m-%d").to_string());
        node4.effort = Some("3w".to_string());

        // Scenario 5: No effort, due in 5d: default 3d, ratio=3/5=0.6, ~+2800 (2000 + (0.6-0.5)*8000 = 2800)
        let mut node5 = GraphNode::default();
        node5.due = Some(in_5d.format("%Y-%m-%d").to_string());

        // Scenario 6: No due date: unchanged (0 deadline component)
        let mut node6 = GraphNode::default();

        let mut nodes = vec![
            node1,
            node2,
            node3.clone(),
            node4.clone(),
            node5.clone(),
            node6,
        ];
        GraphStore::compute_focus_scores(&mut nodes);

        // Verify scores
        assert_eq!(nodes[0].focus_score.unwrap(), 1000);
        assert_eq!(nodes[1].focus_score.unwrap(), 6000);
        assert!(
            nodes[2].focus_score.unwrap() >= 3900 && nodes[2].focus_score.unwrap() <= 4100,
            "Scenario 3 failed: expected ~4000, got {}",
            nodes[2].focus_score.unwrap()
        );
        assert_eq!(nodes[3].focus_score.unwrap(), 6000);
        assert!(
            nodes[4].focus_score.unwrap() >= 2700 && nodes[4].focus_score.unwrap() <= 2900,
            "Scenario 5 failed: expected ~2800, got {}",
            nodes[4].focus_score.unwrap()
        );
        assert_eq!(nodes[5].focus_score.unwrap(), 0);

        // Consequence multiplier: +50% on deadline score
        let mut node7 = GraphNode::default();
        node7.due = Some(tomorrow.format("%Y-%m-%d").to_string());
        node7.effort = Some("1d".to_string());
        node7.consequence = Some("high".to_string());
        let mut nodes7 = vec![node7];
        GraphStore::compute_focus_scores(&mut nodes7);
        assert_eq!(nodes7[0].focus_score.unwrap(), 9000);

        // Scenario 8: Overdue by 2 days: +8000 + 2*200 = 8400
        let mut node8 = GraphNode::default();
        node8.due = Some(
            (today - chrono::Duration::days(2))
                .format("%Y-%m-%d")
                .to_string(),
        );
        let mut nodes8 = vec![node8];
        GraphStore::compute_focus_scores(&mut nodes8);
        assert_eq!(nodes8[0].focus_score.unwrap(), 8400);
    }

    #[test]
    fn test_tarjan_scc_no_cycles() {
        // A → B → C (linear chain, no cycles)
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        adj.insert("A".to_string(), vec!["B".to_string()]);
        adj.insert("B".to_string(), vec!["C".to_string()]);

        let sccs = tarjan_scc(&adj);
        let cycles: Vec<_> = sccs.into_iter().filter(|s| s.len() > 1).collect();
        assert!(cycles.is_empty(), "linear chain should have no cycles");
    }

    #[test]
    fn test_tarjan_scc_simple_cycle() {
        // A → B → A (2-node cycle)
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        adj.insert("A".to_string(), vec!["B".to_string()]);
        adj.insert("B".to_string(), vec!["A".to_string()]);

        let sccs = tarjan_scc(&adj);
        let cycles: Vec<_> = sccs.into_iter().filter(|s| s.len() > 1).collect();
        assert_eq!(cycles.len(), 1, "should detect exactly one cycle");
        let cycle_ids: HashSet<_> = cycles[0].iter().collect();
        assert!(cycle_ids.contains(&"A".to_string()));
        assert!(cycle_ids.contains(&"B".to_string()));
    }

    #[test]
    fn test_tarjan_scc_three_node_cycle() {
        // A → B → C → A
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        adj.insert("A".to_string(), vec!["B".to_string()]);
        adj.insert("B".to_string(), vec!["C".to_string()]);
        adj.insert("C".to_string(), vec!["A".to_string()]);

        let sccs = tarjan_scc(&adj);
        let cycles: Vec<_> = sccs.into_iter().filter(|s| s.len() > 1).collect();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 3);
    }

    #[test]
    fn test_tarjan_scc_disjoint_cycle_and_chain() {
        // A ↔ B (cycle), C → D (no cycle)
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        adj.insert("A".to_string(), vec!["B".to_string()]);
        adj.insert("B".to_string(), vec!["A".to_string()]);
        adj.insert("C".to_string(), vec!["D".to_string()]);

        let sccs = tarjan_scc(&adj);
        let cycles: Vec<_> = sccs.into_iter().filter(|s| s.len() > 1).collect();
        assert_eq!(cycles.len(), 1, "only one cycle among the two components");
    }

    #[test]
    fn test_find_hard_cycles_on_graph() {
        // task-a depends on task-b; task-b depends on task-a → hard cycle
        let docs = vec![
            make_doc("tasks/a.md", "Task A", "task", "active", "task-a", None, &["task-b"]),
            make_doc("tasks/b.md", "Task B", "task", "active", "task-b", None, &["task-a"]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let hard_cycles = graph.find_hard_cycles();
        assert_eq!(hard_cycles.len(), 1, "should detect one hard cycle");
        let cycle_ids: HashSet<_> = hard_cycles[0].iter().cloned().collect();
        assert!(cycle_ids.contains("task-a"));
        assert!(cycle_ids.contains("task-b"));
    }

    #[test]
    fn test_find_hard_cycles_no_cycle() {
        // task-a depends on task-b (linear, no cycle)
        let docs = vec![
            make_doc("tasks/a.md", "Task A", "task", "active", "task-a", None, &["task-b"]),
            make_doc("tasks/b.md", "Task B", "task", "active", "task-b", None, &[]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        assert!(
            graph.find_hard_cycles().is_empty(),
            "linear dependency should have no hard cycles"
        );
    }

    #[test]
    fn test_find_soft_cycle_count() {
        // soft_depends_on cycle: task-soft-a ↔ task-soft-b
        let docs = vec![
            make_doc_with_soft_dep("tasks/a.md", "Task A", "task-soft-a", &["task-soft-b"]),
            make_doc_with_soft_dep("tasks/b.md", "Task B", "task-soft-b", &["task-soft-a"]),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        assert_eq!(graph.find_soft_cycle_count(), 1, "soft mutual dependency = one soft cycle");
        // Must not appear in hard cycles
        assert!(graph.find_hard_cycles().is_empty());
    }

    #[test]
    fn test_compute_project_field_hierarchy() {
        // Setup a hierarchy:
        // Project A (type: project)
        //   Epic B (type: epic, parent: Project A, project: "Wrong Project")
        //     Task C (type: task, parent: Epic B)
        // Task D (type: task, project: "Project E") -- no project ancestor
        
        let mut fm_b = serde_json::Map::new();
        fm_b.insert("type".to_string(), serde_json::json!("epic"));
        fm_b.insert("id".to_string(), serde_json::json!("epic-b"));
        fm_b.insert("parent".to_string(), serde_json::json!("project-a"));
        fm_b.insert("project".to_string(), serde_json::json!("Wrong Project"));
        
        let mut fm_d = serde_json::Map::new();
        fm_d.insert("type".to_string(), serde_json::json!("task"));
        fm_d.insert("id".to_string(), serde_json::json!("task-d"));
        fm_d.insert("project".to_string(), serde_json::json!("Project E"));

        let docs = vec![
            make_doc("projects/a.md", "Project A", "project", "active", "project-a", None, &[]),
            PkbDocument {
                path: PathBuf::from("epics/b.md"),
                title: "Epic B".to_string(),
                body: String::new(),
                doc_type: Some("epic".to_string()),
                status: Some("active".to_string()),
                modified: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_b)),
                content_hash: "test_hash".to_string(),
            },
            make_doc("tasks/c.md", "Task C", "task", "active", "task-c", Some("epic-b"), &[]),
            PkbDocument {
                path: PathBuf::from("tasks/d.md"),
                title: "Task D".to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                modified: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_d)),
                content_hash: "test_hash".to_string(),
            },
        ];

        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let a = graph.get_node("project-a").unwrap();
        let b = graph.get_node("epic-b").unwrap();
        let c = graph.get_node("task-c").unwrap();
        let d = graph.get_node("task-d").unwrap();

        assert_eq!(a.project.as_deref(), Some("Project A"));
        assert_eq!(b.project.as_deref(), Some("Project A"), "Epic B should inherit Project A and ignore 'Wrong Project'");
        assert_eq!(c.project.as_deref(), Some("Project A"), "Task C should inherit Project A via Epic B");
        assert_eq!(d.project, None, "Task D should have no project since 'Project E' in frontmatter must be ignored");
    }

    #[test]
    fn test_sev4_lexicographic_sorting() {
        let mut make_with_sev = |id: &str, priority: i32, severity: Option<i32>| -> PkbDocument {
            let mut fm = serde_json::Map::new();
            fm.insert("title".to_string(), serde_json::json!(id));
            fm.insert("type".to_string(), serde_json::json!("task"));
            fm.insert("status".to_string(), serde_json::json!("active"));
            fm.insert("id".to_string(), serde_json::json!(id));
            fm.insert("priority".to_string(), serde_json::json!(priority));
            if let Some(sev) = severity {
                fm.insert("severity".to_string(), serde_json::json!(sev));
            }
            PkbDocument {
                path: std::path::PathBuf::from(format!("tasks/{}.md", id)),
                title: id.to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("active".to_string()),
                modified: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm)),
                content_hash: "test".to_string(),
            }
        };

        let docs = vec![
            // high-pri-low-sev: P0, SEV2
            make_with_sev("high-pri-low-sev", 0, Some(2)),
            // low-pri-high-sev: P2, SEV4
            make_with_sev("low-pri-high-sev", 2, Some(4)),
            // mid-pri-mid-sev: P1, SEV3
            make_with_sev("mid-pri-mid-sev", 1, Some(3)),
        ];
        let graph = GraphStore::build(&docs, std::path::Path::new("/tmp/test-sev-pkb"));
        let ready = graph.ready_tasks();
        let ready_ids: Vec<&str> = ready.iter().map(|n| n.id.as_str()).collect();

        // SEV4 must come first, even if lower priority
        assert_eq!(ready_ids[0], "low-pri-high-sev");
        assert_eq!(ready_ids[1], "mid-pri-mid-sev");
        assert_eq!(ready_ids[2], "high-pri-low-sev");
    }
}
