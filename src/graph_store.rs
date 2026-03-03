//! Graph store — builds, queries, and exports knowledge graphs.
//!
//! [`GraphStore`] holds all nodes, edges, and pre-computed task indices.
//! Build from `PkbDocument`s via [`GraphStore::build`], then query with
//! the various accessor methods.

use crate::graph::{self, deduplicate_vec, Edge, EdgeType, GraphNode};
use crate::layout;
use crate::metrics;
use crate::pkb::PkbDocument;
use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

// ===========================================================================
// Output graph (for JSON serialization)
// ===========================================================================

/// Rendering hints for a named layout (consumed by D3/renderer).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LayoutMeta {
    /// How edges should be drawn: "manhattan", "arc", "hidden", "straight"
    pub edge_style: String,
    /// For arc layout: "alternate" draws arcs above/below the line
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arc_direction: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OutputGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ready: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub by_project: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub layout_metadata: HashMap<String, LayoutMeta>,
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
    by_project: HashMap<String, Vec<String>>,
    /// Lowercase (id | filename stem | title | permalink) → canonical node ID
    resolution_map: HashMap<String, String>,
}

/// Document types considered actionable work items in task trees and dashboards.
pub const ACTIONABLE_TYPES: &[&str] = &[
    "task", "bug", "feature", "project", "goal", "epic", "learn", "subproject",
];

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
        // 1. Extract graph nodes
        let mut nodes: Vec<GraphNode> = docs.par_iter().map(GraphNode::from_pkb_document).collect();

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

        // 6. Compute centrality metrics (PageRank, betweenness)
        compute_centrality_metrics(&mut nodes, &edges);

        // 7. Compute downstream metrics (BFS through blocks/soft_blocks)
        compute_downstream_metrics(&mut nodes);

        // 8. Compute reachable set (upstream BFS from active leaves)
        //    Done before layout so FA2 can run on reachable-only subgraph.
        let reachable_set = find_reachable_set(&nodes, &edges);
        for node in &mut nodes {
            node.reachable = reachable_set.contains(&node.id);
        }

        // 9. Compute layouts (FA2 on reachable-only, treemap/circle/arc on all)
        layout::compute_layout(&mut nodes, &edges, &reachable_set);

        // 10. Build node map and classify tasks
        let node_map: HashMap<String, GraphNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let (ready, blocked, roots, by_project) = classify_tasks(&node_map);

        // 11. Build resolution map for flexible node lookup
        let resolution_map = build_resolution_map(&node_map);

        GraphStore {
            nodes: node_map,
            edges,
            ready,
            blocked,
            roots,
            by_project,
            resolution_map,
        }
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

    // -----------------------------------------------------------------------
    // Query API
    // -----------------------------------------------------------------------

    pub fn get_node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.get(id)
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
            a.priority
                .unwrap_or(2)
                .cmp(&b.priority.unwrap_or(2))
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

    pub fn by_project(&self) -> &HashMap<String, Vec<String>> {
        &self.by_project
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
    /// ID, task_id, filename stem, title, permalink).
    pub fn resolve(&self, query: &str) -> Option<&GraphNode> {
        // 1. Exact ID match
        if let Some(node) = self.nodes.get(query) {
            return Some(node);
        }
        // 2. Resolution map (case-insensitive)
        if let Some(canonical_id) = self.resolution_map.get(&query.to_lowercase()) {
            return self.nodes.get(canonical_id);
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

    /// Find orphan nodes (nodes with zero edges — no incoming or outgoing).
    pub fn orphans(&self) -> Vec<&GraphNode> {
        // Build set of all node IDs that appear in any edge
        let mut connected: HashSet<&str> = HashSet::new();
        for edge in &self.edges {
            connected.insert(&edge.source);
            connected.insert(&edge.target);
        }
        self.nodes
            .values()
            .filter(|n| !connected.contains(n.id.as_str()))
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

    /// Build an `OutputGraph` suitable for JSON/GraphML/DOT export.
    pub fn to_output_graph(&self) -> OutputGraph {
        let mut nodes: Vec<GraphNode> = self.nodes.values().cloned().collect();
        nodes.sort_by(|a, b| a.label.cmp(&b.label));

        let mut layout_metadata = HashMap::new();
        layout_metadata.insert(
            "forceatlas2".into(),
            LayoutMeta { edge_style: "manhattan".into(), arc_direction: None },
        );
        layout_metadata.insert(
            "forceatlas2_focus".into(),
            LayoutMeta { edge_style: "manhattan".into(), arc_direction: None },
        );
        layout_metadata.insert(
            "treemap".into(),
            LayoutMeta { edge_style: "hidden".into(), arc_direction: None },
        );
        layout_metadata.insert(
            "circle_pack".into(),
            LayoutMeta { edge_style: "hidden".into(), arc_direction: None },
        );
        layout_metadata.insert(
            "arc".into(),
            LayoutMeta { edge_style: "arc".into(), arc_direction: Some("alternate".into()) },
        );

        OutputGraph {
            nodes,
            edges: self.edges.clone(),
            ready: self.ready.clone(),
            blocked: self.blocked.clone(),
            by_project: self.by_project.clone(),
            roots: self.roots.clone(),
            layout_metadata,
        }
    }

    /// Remove precomputed layout coordinates from all nodes.
    pub fn strip_layout(&mut self) {
        for node in self.nodes.values_mut() {
            node.x = None;
            node.y = None;
            node.layouts.clear();
        }
    }

    /// Copy a named layout's coordinates to the primary `x`/`y` fields.
    pub fn promote_layout(&mut self, layout_name: &str) {
        for node in self.nodes.values_mut() {
            if let Some(lp) = node.layouts.get(layout_name) {
                node.x = Some(lp.x);
                node.y = Some(lp.y);
            }
        }
    }

    pub fn output_json(&self) -> Result<String> {
        let graph = self.to_output_graph();
        Ok(serde_json::to_string_pretty(&graph)?)
    }

    pub fn output_graphml(&self) -> String {
        let graph = self.to_output_graph();
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
            append(&mut ns, "d6", node.project.as_deref().unwrap_or(""));
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

    pub fn output_dot(&self) -> String {
        self.output_dot_inner(None)
    }

    /// Produce DOT with pinned positions from a named layout.
    ///
    /// Use with `neato -n -Tsvg` to render with Graphviz spline routing
    /// while preserving our precomputed node positions.
    pub fn output_dot_with_layout(&self, layout_name: &str) -> String {
        self.output_dot_inner(Some(layout_name))
    }

    /// List available layout names from the output graph.
    pub fn layout_names(&self) -> Vec<String> {
        let graph = self.to_output_graph();
        graph.layout_metadata.keys().cloned().collect()
    }

    fn output_dot_inner(&self, layout: Option<&str>) -> String {
        let graph = self.to_output_graph();

        // Map our edge_style to Graphviz splines mode
        let splines = match layout {
            Some(name) => match graph.layout_metadata.get(name).map(|m| m.edge_style.as_str()) {
                Some("manhattan") => "ortho",
                Some("arc") => "curved",
                Some("hidden") => "false",
                _ => "spline",
            },
            None => "spline",
        };

        // When using pinned positions, use neato layout engine
        let header = if layout.is_some() {
            format!(
                "digraph G {{\n    layout=neato;\n    splines={splines};\n    overlap=false;\n    node [shape=box, style=filled, fontsize=10];\n\n"
            )
        } else {
            String::from(
                "digraph G {\n    rankdir=TB;\n    node [shape=box, style=filled, fillcolor=\"#e9ecef\"];\n\n",
            )
        };
        let mut dot = header;

        for node in &graph.nodes {
            let label = node.label.replace('"', "\\\"");
            let color = node_type_color(node.node_type.as_deref());

            // Get position from named layout, or fall back to primary x/y
            let pos = layout.and_then(|name| {
                node.layouts.get(name).map(|lp| (lp.x, lp.y))
            }).or_else(|| {
                if layout.is_some() {
                    // Layout requested but node doesn't have it — skip position
                    None
                } else {
                    node.x.zip(node.y)
                }
            });

            if let Some((px, py)) = pos {
                // Graphviz y-axis is bottom-up; our viewport is top-down (0-1000)
                let gy = 1000.0 - py;
                dot.push_str(&format!(
                    "    \"{}\" [label=\"{}\", fillcolor=\"{}\", pos=\"{:.1},{:.1}!\"];\n",
                    node.id, label, color, px, gy
                ));
            } else if layout.is_some() {
                // Node not in this layout — still include but unpinned
                dot.push_str(&format!(
                    "    \"{}\" [label=\"{}\", fillcolor=\"{}\", style=\"filled,dashed\"];\n",
                    node.id, label, color
                ));
            } else {
                dot.push_str(&format!(
                    "    \"{}\" [label=\"{}\", fillcolor=\"{}\"];\n",
                    node.id, label, color
                ));
            }
        }
        dot.push('\n');

        for edge in &graph.edges {
            let style = match edge.edge_type {
                EdgeType::DependsOn => "style=bold, color=\"#dc3545\", penwidth=2",
                EdgeType::SoftDependsOn => "style=dashed, color=\"#6c757d\", penwidth=1.5",
                EdgeType::Parent => "style=solid, color=\"#0d6efd\", penwidth=3",
                EdgeType::Link => "style=dotted, color=\"#adb5bd\", penwidth=1",
                EdgeType::Supersedes => "style=dashed, color=\"#fd7e14\", penwidth=2, label=\"supersedes\"",
            };
            dot.push_str(&format!(
                "    \"{}\" -> \"{}\" [{}];\n",
                edge.source, edge.target, style
            ));
        }

        dot.push_str("}\n");
        dot
    }

}

/// Map node type to a hex fill color (matches layout-preview.html palette).
fn node_type_color(node_type: Option<&str>) -> &'static str {
    match node_type {
        Some("goal") => "#e94560",
        Some("project") => "#0f3460",
        Some("subproject") => "#533483",
        Some("epic") => "#e9a045",
        Some("task") => "#45e980",
        Some("action") => "#45c9e9",
        Some("bug") => "#e94545",
        Some("note") => "#888888",
        Some("knowledge") => "#a0a0e0",
        Some("memory") => "#c9a0e0",
        Some("learn") => "#e0e0a0",
        Some("feature") => "#59a14f",
        Some("milestone") => "#edc948",
        Some("contact") => "#9c755f",
        _ => "#e9ecef",
    }
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

    // project -> Link edge (this -> project node)
    if let Some(ref proj) = n.project {
        if let Some(target_id) = graph::resolve_ref(proj, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::Link,
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

    // Collect updates to avoid borrow issues
    let mut block_updates: Vec<(usize, String)> = Vec::new(); // (target_idx, source_id)
    let mut soft_block_updates: Vec<(usize, String)> = Vec::new();
    let mut children_updates: Vec<(usize, String)> = Vec::new();

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
                // source is child of target -> target.children += source
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    children_updates.push((idx, edge.source.clone()));
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

    // Deduplicate and update leaf status
    for node in nodes.iter_mut() {
        deduplicate_vec(&mut node.blocks);
        deduplicate_vec(&mut node.soft_blocks);
        deduplicate_vec(&mut node.children);
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

/// Compute downstream_weight and stakeholder_exposure via BFS through
/// blocks/soft_blocks. Mirrors the logic from fast-indexer main.rs.
fn compute_downstream_metrics(nodes: &mut [GraphNode]) {
    let excluded: HashSet<&str> = ["done", "cancelled"].into_iter().collect();

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

    // Snapshot blocks/soft_blocks to avoid borrow issues
    let blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.blocks.clone()))
        .collect();
    let soft_blocks_map: HashMap<String, Vec<String>> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.soft_blocks.clone()))
        .collect();

    let all_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();

    for start_id in &all_ids {
        let mut total_weight: f64 = 0.0;
        let mut has_stakeholder = false;
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: Vec<(String, u32, bool)> = Vec::new();

        // Seed with direct blocks
        if let Some(blocked) = blocks_map.get(start_id) {
            for bid in blocked {
                let status_ok = id_to_idx
                    .get(bid)
                    .and_then(|&idx| nodes[idx].status.as_deref())
                    .map(|s| !excluded.contains(s))
                    .unwrap_or(false);
                if status_ok {
                    queue.push((bid.clone(), 1, false));
                }
            }
        }
        if let Some(soft_blocked) = soft_blocks_map.get(start_id) {
            for sbid in soft_blocked {
                let status_ok = id_to_idx
                    .get(sbid)
                    .and_then(|&idx| nodes[idx].status.as_deref())
                    .map(|s| !excluded.contains(s))
                    .unwrap_or(false);
                if status_ok {
                    queue.push((sbid.clone(), 1, true));
                }
            }
        }

        while let Some((tid, depth, is_soft)) = queue.pop() {
            if !visited.insert(tid.clone()) {
                continue;
            }
            if let Some(&bw) = base_weights.get(&tid) {
                let depth_decay = 1.0 / (depth as f64);
                let soft_factor = if is_soft { 0.3 } else { 1.0 };
                total_weight += depth_decay * bw * soft_factor;
            }
            if has_due.contains(&tid) {
                has_stakeholder = true;
            }
            if let Some(next_blocks) = blocks_map.get(&tid) {
                for next in next_blocks {
                    if !visited.contains(next) {
                        queue.push((next.clone(), depth + 1, is_soft));
                    }
                }
            }
            if let Some(next_soft) = soft_blocks_map.get(&tid) {
                for next in next_soft {
                    if !visited.contains(next) {
                        queue.push((next.clone(), depth + 1, true));
                    }
                }
            }
        }

        if let Some(&idx) = id_to_idx.get(start_id) {
            nodes[idx].downstream_weight = (total_weight * 100.0).round() / 100.0;
            nodes[idx].stakeholder_exposure = has_stakeholder;
        }
    }
}

/// Classify tasks into ready/blocked lists, compute roots and by_project.
fn classify_tasks(
    nodes: &HashMap<String, GraphNode>,
) -> (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    HashMap<String, Vec<String>>,
) {
    let completed: HashSet<&str> = ["done", "cancelled"].into_iter().collect();

    let completed_ids: HashSet<String> = nodes
        .iter()
        .filter(|(_, n)| {
            n.status
                .as_deref()
                .map(|s| completed.contains(s))
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect();

    let mut ready: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();

    for (id, node) in nodes {
        // Only classify nodes that have a task_id
        if node.task_id.is_none() {
            continue;
        }

        let status = node.status.as_deref().unwrap_or("active");
        if completed.contains(status) {
            continue;
        }

        let unmet_deps: Vec<&String> = node
            .depends_on
            .iter()
            .filter(|d| !completed_ids.contains(*d))
            .collect();

        if !unmet_deps.is_empty() || status == "blocked" {
            blocked.push(id.clone());
        } else if node.leaf && status == "active" {
            // Learn tasks are observational, not actionable
            if node.node_type.as_deref() != Some("learn") {
                ready.push(id.clone());
            }
        }
    }

    // Sort ready by priority, then downstream_weight DESC, then order, then title
    ready.sort_by(|a, b| {
        let na = nodes.get(a).unwrap();
        let nb = nodes.get(b).unwrap();
        na.priority
            .unwrap_or(2)
            .cmp(&nb.priority.unwrap_or(2))
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

    // By project
    let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
    for (id, node) in nodes {
        if node.task_id.is_some() {
            let proj = node
                .project
                .clone()
                .unwrap_or_else(|| "_no_project".to_string());
            by_project.entry(proj).or_default().push(id.clone());
        }
    }

    (ready, blocked, roots, by_project)
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
    let done_statuses: HashSet<&str> = ["done", "cancelled", "completed"].into_iter().collect();

    let all_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    let unfinished_ids: HashSet<&str> = nodes
        .iter()
        .filter(|n| {
            !n.status
                .as_deref()
                .map(|s| done_statuses.contains(s))
                .unwrap_or(false)
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkb::PkbDocument;
    use std::path::PathBuf;

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
        assert!(graph.resolve("nonexistent").is_none());
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
}
