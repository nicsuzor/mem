//! Sugiyama layered DAG layout engine, ego-network extraction, and Excalidraw visual generator.
//!
//! Provides:
//! - 1-to-5 hop ego-network extraction around focus nodes
//! - Sugiyama layered DAG layout (Tarjan SCC cycle breaking, topological longest-path rank assignment, dummy vertices, barycentric crossing minimization)
//! - Spatial Frame elements enclosing parent Epics/Areas and child tasks ($W=240, H=84$)
//! - Normalized port connection calculations ($[0.0, 0.5]$ In, $[1.0, 0.5]$ Out, $[0.5, 0.0]$ Top, $[0.5, 1.0]$ Bottom)

use crate::excalidraw::schema::*;
use crate::graph::{Edge, EdgeType, GraphNode};
use crate::graph_store::GraphStore;
use std::collections::{HashMap, HashSet, VecDeque};

// ===========================================================================
// Configuration
// ===========================================================================

#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub hops: usize,
    pub card_width: f64,
    pub card_height: f64,
    pub horizontal_gap: f64,
    pub vertical_gap: f64,
    pub include_frames: bool,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            hops: 2,
            card_width: CARD_WIDTH,
            card_height: CARD_HEIGHT,
            horizontal_gap: 120.0,
            vertical_gap: 48.0,
            include_frames: true,
        }
    }
}

// ===========================================================================
// Ego-Network Extraction
// ===========================================================================

/// Extract an ego-network (1 to 5 hops) around a focus node from the [`GraphStore`].
pub fn extract_ego_subgraph(
    gs: &crate::graph_store::GraphStore,
    focus_id: &str,
    hops: usize,
) -> (Vec<crate::graph::GraphNode>, Vec<crate::graph::Edge>) {
    let hops = hops.clamp(1, 5);

    // Collect reachable node IDs via BFS over structural edges only
    let mut node_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let max_nodes = 100;
    
    if gs.get_node(focus_id).is_some() {
        node_ids.insert(focus_id.to_string());
        queue.push_back((focus_id.to_string(), 0));
    }

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        for edge in gs.edges() {
            if !matches!(
                edge.edge_type,
                crate::graph::EdgeType::Parent
                    | crate::graph::EdgeType::DependsOn
                    | crate::graph::EdgeType::SoftDependsOn
                    | crate::graph::EdgeType::ContributesTo
                    | crate::graph::EdgeType::Supersedes
                    | crate::graph::EdgeType::Closes
            ) {
                continue;
            }
            
            let neighbor = if edge.source == current {
                &edge.target
            } else if edge.target == current {
                &edge.source
            } else {
                continue;
            };

            if node_ids.len() < max_nodes && !node_ids.contains(neighbor) {
                node_ids.insert(neighbor.clone());
                queue.push_back((neighbor.clone(), depth + 1));
            }
        }
    }

    let mut nodes = Vec::with_capacity(node_ids.len());
    for nid in &node_ids {
        if let Some(node) = gs.get_node(nid) {
            nodes.push(node.clone());
        }
    }

    let mut edges = Vec::new();
    for edge in gs.edges() {
        if node_ids.contains(&edge.source) && node_ids.contains(&edge.target) {
            edges.push(edge.clone());
        }
    }

    (nodes, edges)
}


// ===========================================================================
// Sugiyama Layout Implementation
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LayoutVertex {
    Real(String),
    Dummy(usize), // dummy vertex index for multi-rank edges
}

#[derive(Debug, Clone)]
struct LayoutEdge {
    source: LayoutVertex,
    target: LayoutVertex,
}

/// Compute Sugiyama layered layout for given nodes and edges.
/// Returns a map of `node_id -> (x, y)` coordinates.
pub fn compute_sugiyama_layout(
    nodes: &[GraphNode],
    edges: &[Edge],
    config: &LayoutConfig,
) -> HashMap<String, (f64, f64)> {
    if nodes.is_empty() {
        return HashMap::new();
    }

    let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let node_set: HashSet<&str> = node_ids.iter().map(|s| s.as_str()).collect();

    // 1. Build adjacency for cycle breaking
    // For dependency flow, target unblocks source (target -> source) or source -> target
    // We treat edges directed from upstream to downstream:
    // depends_on: target -> source
    // parent: source (parent) -> target (child)
    // contributes_to: source -> target
    // link / other: source -> target
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for nid in &node_ids {
        adj.insert(nid.clone(), Vec::new());
    }

    let mut directed_edges: Vec<(String, String, Edge)> = Vec::new();
    for edge in edges {
        if !node_set.contains(edge.source.as_str()) || !node_set.contains(edge.target.as_str()) {
            continue;
        }
        let (u, v) = match edge.edge_type {
            EdgeType::DependsOn | EdgeType::SoftDependsOn => {
                (edge.target.clone(), edge.source.clone()) // prerequisite -> dependent
            }
            _ => (edge.source.clone(), edge.target.clone()),
        };
        if u != v {
            directed_edges.push((u.clone(), v.clone(), edge.clone()));
            adj.entry(u).or_default().push(v);
        }
    }

    // 2. Tarjan SCC / Cycle Breaking
    let feedback_edges = find_feedback_arc_set(&node_ids, &adj);
    let mut dag_adj: HashMap<String, Vec<String>> = HashMap::new();
    for nid in &node_ids {
        dag_adj.insert(nid.clone(), Vec::new());
    }

    for (u, v, _) in &directed_edges {
        if feedback_edges.contains(&(u.clone(), v.clone())) {
            // Reverse edge to maintain DAG property for ranking
            dag_adj.entry(v.clone()).or_default().push(u.clone());
        } else {
            dag_adj.entry(u.clone()).or_default().push(v.clone());
        }
    }

    // 3. Topological longest-path rank assignment
    let ranks = compute_longest_path_ranks(&node_ids, &dag_adj);

    // 4. Insert dummy vertices for edges spanning >= 2 layers
    let mut dummy_count = 0usize;
    let mut layout_edges: Vec<LayoutEdge> = Vec::new();
    let mut vertex_ranks: HashMap<LayoutVertex, usize> = HashMap::new();

    for (nid, &r) in &ranks {
        vertex_ranks.insert(LayoutVertex::Real(nid.clone()), r);
    }

    for (u, v, _orig_edge) in directed_edges {
        let u_vert = LayoutVertex::Real(u.clone());
        let v_vert = LayoutVertex::Real(v.clone());
        let r_u = *ranks.get(&u).unwrap_or(&0);
        let r_v = *ranks.get(&v).unwrap_or(&0);

        if r_v > r_u + 1 {
            // Span >= 2 ranks: create dummy vertex chain
            let mut prev = u_vert;
            for r in (r_u + 1)..r_v {
                dummy_count += 1;
                let dummy = LayoutVertex::Dummy(dummy_count);
                vertex_ranks.insert(dummy.clone(), r);
                layout_edges.push(LayoutEdge {
                    source: prev,
                    target: dummy.clone(),
                });
                prev = dummy;
            }
            layout_edges.push(LayoutEdge {
                source: prev,
                target: v_vert,
            });
        } else {
            layout_edges.push(LayoutEdge {
                source: u_vert,
                target: v_vert,
            });
        }
    }

    // Group vertices by rank
    let max_rank = vertex_ranks.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<LayoutVertex>> = vec![Vec::new(); max_rank + 1];

    for (vert, &r) in &vertex_ranks {
        if r < layers.len() {
            layers[r].push(vert.clone());
        }
    }

    // 5. Barycentric crossing minimization sweeps (4 iterations)
    barycentric_crossing_minimization(&mut layers, &layout_edges, 4);

    // 6. Assign final (x, y) coordinates
    let mut positions: HashMap<String, (f64, f64)> = HashMap::new();
    for (layer_idx, layer) in layers.iter().enumerate() {
        let real_vertices: Vec<&String> = layer
            .iter()
            .filter_map(|v| match v {
                LayoutVertex::Real(id) => Some(id),
                LayoutVertex::Dummy(_) => None,
            })
            .collect();

        for (pos_idx, &nid) in real_vertices.iter().enumerate() {
            let x = layer_idx as f64 * (config.card_width + config.horizontal_gap) + 100.0;
            let y = pos_idx as f64 * (config.card_height + config.vertical_gap) + 100.0;
            positions.insert(nid.clone(), (x, y));
        }
    }

    // Ensure all input nodes have coordinates
    for nid in &node_ids {
        positions.entry(nid.clone()).or_insert_with(|| (100.0, 100.0));
    }

    positions
}

/// Tarjan SCC based cycle breaking: find feedback arc set to make directed graph a DAG.
fn find_feedback_arc_set(
    nodes: &[String],
    adj: &HashMap<String, Vec<String>>,
) -> HashSet<(String, String)> {
    let mut feedback = HashSet::new();
    let mut visited: HashMap<String, u8> = HashMap::new(); // 0 = unvisited, 1 = visiting, 2 = visited

    for nid in nodes {
        visited.insert(nid.clone(), 0);
    }

    for nid in nodes {
        if visited.get(nid) == Some(&0) {
            dfs_cycle_find(nid, adj, &mut visited, &mut feedback);
        }
    }

    feedback
}

fn dfs_cycle_find(
    u: &str,
    adj: &HashMap<String, Vec<String>>,
    visited: &mut HashMap<String, u8>,
    feedback: &mut HashSet<(String, String)>,
) {
    visited.insert(u.to_string(), 1); // visiting

    if let Some(neighbors) = adj.get(u) {
        for v in neighbors {
            match visited.get(v) {
                Some(&1) => {
                    // Back-edge found -> cycle!
                    feedback.insert((u.to_string(), v.to_string()));
                }
                Some(&0) | None => {
                    dfs_cycle_find(v, adj, visited, feedback);
                }
                _ => {}
            }
        }
    }

    visited.insert(u.to_string(), 2); // finished
}

/// Compute longest path ranks in a DAG.
fn compute_longest_path_ranks(
    nodes: &[String],
    dag_adj: &HashMap<String, Vec<String>>,
) -> HashMap<String, usize> {
    // In-degree calculation
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for nid in nodes {
        in_degree.insert(nid.clone(), 0);
    }
    for neighbors in dag_adj.values() {
        for v in neighbors {
            *in_degree.entry(v.clone()).or_insert(0) += 1;
        }
    }

    let mut queue = VecDeque::new();
    let mut ranks: HashMap<String, usize> = HashMap::new();

    for nid in nodes {
        if *in_degree.get(nid).unwrap_or(&0) == 0 {
            queue.push_back(nid.clone());
            ranks.insert(nid.clone(), 0);
        }
    }

    while let Some(u) = queue.pop_front() {
        let u_rank = *ranks.get(&u).unwrap_or(&0);
        if let Some(neighbors) = dag_adj.get(&u) {
            for v in neighbors {
                let curr_v_rank = *ranks.get(v).unwrap_or(&0);
                ranks.insert(v.clone(), curr_v_rank.max(u_rank + 1));

                if let Some(deg) = in_degree.get_mut(v) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(v.clone());
                    }
                }
            }
        }
    }

    // Assign rank 0 to any remaining nodes
    for nid in nodes {
        ranks.entry(nid.clone()).or_insert(0);
    }

    ranks
}

/// Barycentric crossing minimization sweeps across layers.
fn barycentric_crossing_minimization(
    layers: &mut [Vec<LayoutVertex>],
    edges: &[LayoutEdge],
    iterations: usize,
) {
    if layers.len() <= 1 {
        return;
    }

    // Build fast lookup for forward and backward adjacency
    let mut forward_adj: HashMap<LayoutVertex, Vec<LayoutVertex>> = HashMap::new();
    let mut backward_adj: HashMap<LayoutVertex, Vec<LayoutVertex>> = HashMap::new();

    for edge in edges {
        forward_adj
            .entry(edge.source.clone())
            .or_default()
            .push(edge.target.clone());
        backward_adj
            .entry(edge.target.clone())
            .or_default()
            .push(edge.source.clone());
    }

    for _ in 0..iterations {
        // Forward sweep
        for i in 1..layers.len() {
            let prev_layer_pos: HashMap<LayoutVertex, f64> = layers[i - 1]
                .iter()
                .enumerate()
                .map(|(idx, v)| (v.clone(), idx as f64))
                .collect();

            layers[i].sort_by(|a, b| {
                let a_bary = calculate_barycenter(a, &backward_adj, &prev_layer_pos);
                let b_bary = calculate_barycenter(b, &backward_adj, &prev_layer_pos);
                a_bary
                    .partial_cmp(&b_bary)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Backward sweep
        for i in (0..layers.len() - 1).rev() {
            let next_layer_pos: HashMap<LayoutVertex, f64> = layers[i + 1]
                .iter()
                .enumerate()
                .map(|(idx, v)| (v.clone(), idx as f64))
                .collect();

            layers[i].sort_by(|a, b| {
                let a_bary = calculate_barycenter(a, &forward_adj, &next_layer_pos);
                let b_bary = calculate_barycenter(b, &forward_adj, &next_layer_pos);
                a_bary
                    .partial_cmp(&b_bary)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

fn calculate_barycenter(
    vertex: &LayoutVertex,
    adj: &HashMap<LayoutVertex, Vec<LayoutVertex>>,
    neighbor_positions: &HashMap<LayoutVertex, f64>,
) -> f64 {
    if let Some(neighbors) = adj.get(vertex) {
        let mut sum = 0.0;
        let mut count = 0;
        for n in neighbors {
            if let Some(&pos) = neighbor_positions.get(n) {
                sum += pos;
                count += 1;
            }
        }
        if count > 0 {
            return sum / count as f64;
        }
    }
    f64::MAX // Keep existing relative position for nodes without connections
}

// ===========================================================================
// Excalidraw Scene Generation
// ===========================================================================

/// Safely truncate title using `floor_char_boundary`.
fn truncate_title(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s.floor_char_boundary(max_len);
        format!("{}...", &s[..boundary])
    }
}

/// Generate a complete [`ExcalidrawFile`] from a list of nodes and edges with Sugiyama layout.
pub fn generate_excalidraw_scene(
    nodes: &[GraphNode],
    edges: &[Edge],
    config: &LayoutConfig,
) -> ExcalidrawFile {
    let positions = compute_sugiyama_layout(nodes, edges, config);

    let mut elements: Vec<ExcalidrawElement> = Vec::new();
    let mut card_elem_map: HashMap<String, String> = HashMap::new(); // node_id -> card_element_id

    // Group child nodes by parent for Frame generation
    let mut parent_to_children: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes {
        if let Some(ref pid) = node.parent {
            parent_to_children
                .entry(pid.clone())
                .or_default()
                .push(node.id.clone());
        }
    }

    // 1. Generate Frames for parents (Epics, Areas) enclosing children
    let mut node_to_frame: HashMap<String, String> = HashMap::new();
    if config.include_frames {
        for (parent_id, child_ids) in &parent_to_children {
            let child_positions: Vec<(f64, f64)> = child_ids
                .iter()
                .filter_map(|cid| positions.get(cid).copied())
                .collect();

            if child_positions.is_empty() {
                continue;
            }

            let min_x = child_positions
                .iter()
                .map(|(x, _)| *x)
                .fold(f64::INFINITY, f64::min);
            let max_x = child_positions
                .iter()
                .map(|(x, _)| *x)
                .fold(f64::NEG_INFINITY, f64::max);
            let min_y = child_positions
                .iter()
                .map(|(_, y)| *y)
                .fold(f64::INFINITY, f64::min);
            let max_y = child_positions
                .iter()
                .map(|(_, y)| *y)
                .fold(f64::NEG_INFINITY, f64::max);

            let frame_id = format!("frame-{}", parent_id);
            let frame_x = min_x - FRAME_PADDING;
            let frame_y = min_y - FRAME_PADDING - FRAME_HEADER_HEIGHT;
            let frame_w = (max_x - min_x) + config.card_width + 2.0 * FRAME_PADDING;
            let frame_h =
                (max_y - min_y) + config.card_height + 2.0 * FRAME_PADDING + FRAME_HEADER_HEIGHT;

            let mut frame_elem = ExcalidrawElement::default();
            frame_elem.id = frame_id.clone();
            frame_elem.element_type = "frame".to_string();
            frame_elem.name = Some(format!("Epic: {}", parent_id));
            frame_elem.x = frame_x;
            frame_elem.y = frame_y;
            frame_elem.width = frame_w;
            frame_elem.height = frame_h;
            frame_elem.stroke_color = "#4c6ef5".to_string();
            frame_elem.stroke_width = 2.0;
            frame_elem.background_color = "transparent".to_string();
            frame_elem.custom_data = Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some(parent_id.clone()),
                    node_type: Some("epic".to_string()),
                    is_pkb_managed: Some(true),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            });

            for cid in child_ids {
                node_to_frame.insert(cid.clone(), frame_id.clone());
            }

            elements.push(frame_elem);
        }
    }

    // 2. Generate Cards (Rectangle/Diamond) and Container-Bound Text
    for node in nodes {
        let (x, y) = positions.get(&node.id).copied().unwrap_or((100.0, 100.0));
        let card_id = format!("card-{}", node.id);
        let text_id = format!("text-{}", node.id);
        card_elem_map.insert(node.id.clone(), card_id.clone());

        let color_style =
            node_color_style(node.status.as_deref(), node.node_type.as_deref());

        // Card Container Shape
        let mut card_elem = ExcalidrawElement::default();
        card_elem.id = card_id.clone();
        card_elem.element_type = match node.node_type.as_deref() {
            Some("target") | Some("goal") => "diamond".to_string(),
            Some("area") => "ellipse".to_string(),
            _ => "rectangle".to_string(),
        };
        card_elem.x = x;
        card_elem.y = y;
        card_elem.width = config.card_width;
        card_elem.height = config.card_height;
        card_elem.background_color = color_style.bg_color.to_string();
        card_elem.stroke_color = color_style.stroke_color.to_string();
        card_elem.stroke_style = color_style.stroke_style.to_string();
        card_elem.fill_style = color_style.fill_style.to_string();
        card_elem.opacity = color_style.opacity as f64;
        card_elem.stroke_width = 1.5;
        card_elem.frame_id = node_to_frame.get(&node.id).cloned();
        card_elem.bound_elements = Some(vec![BoundElement {
            id: text_id.clone(),
            element_type: "text".to_string(),
        }]);
        card_elem.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some(node.id.clone()),
                node_type: node.node_type.clone(),
                status: node.status.clone(),
                priority: node.priority,
                parent: node.parent.clone(),
                tags: node.tags.clone(),
                is_pkb_managed: Some(true),
                ..Default::default()
            }),
            extra: HashMap::new(),
        });

        // Formatted Bound Text Element
        let title_line = truncate_title(&node.label, 36);
        let status_str = node.status.as_deref().unwrap_or("inbox").to_uppercase();
        let prio_str = node
            .priority
            .map(|p| format!("P{}", p))
            .unwrap_or_default();
        let header = if prio_str.is_empty() {
            format!("[{}]", status_str)
        } else {
            format!("[{} · {}]", status_str, prio_str)
        };
        let tag_str = if node.tags.is_empty() {
            String::new()
        } else {
            format!(
                "\n{}",
                truncate_title(&node.tags.iter().map(|t| format!("#{}", t)).collect::<Vec<_>>().join(" "), 32)
            )
        };
        let text_content = format!("{}\n{}{}", header, title_line, tag_str);

        let mut text_elem = ExcalidrawElement::default();
        text_elem.id = text_id;
        text_elem.element_type = "text".to_string();
        text_elem.container_id = Some(card_id.clone());
        text_elem.text = Some(text_content.clone());
        text_elem.original_text = Some(text_content);
        text_elem.font_size = Some(TITLE_FONT_SIZE);
        text_elem.font_family = Some(FONT_FAMILY_SANS);
        text_elem.text_align = Some("center".to_string());
        text_elem.vertical_align = Some("middle".to_string());
        text_elem.x = x + 8.0;
        text_elem.y = y + 8.0;
        text_elem.width = config.card_width - 16.0;
        text_elem.height = config.card_height - 16.0;
        text_elem.stroke_color = "#1e1e1e".to_string();
        text_elem.frame_id = node_to_frame.get(&node.id).cloned();

        elements.push(card_elem);
        elements.push(text_elem);
    }

    // 3. Generate Arrows with Normalized Port Bindings
    for edge in edges {
        let (Some(&source_pos), Some(&target_pos)) =
            (positions.get(&edge.source), positions.get(&edge.target))
        else {
            continue;
        };
        let (Some(source_card_id), Some(target_card_id)) = (
            card_elem_map.get(&edge.source),
            card_elem_map.get(&edge.target),
        ) else {
            continue;
        };

        let dx = target_pos.0 - source_pos.0;
        let dy = target_pos.1 - source_pos.1;

        // Determine port connections
        let (start_port, end_port, start_x, start_y, end_x, end_y) = if dx > 0.0 {
            // Flow from left to right: Out -> In
            (
                PORT_OUT,
                PORT_IN,
                source_pos.0 + config.card_width,
                source_pos.1 + config.card_height * 0.5,
                target_pos.0,
                target_pos.1 + config.card_height * 0.5,
            )
        } else if dx < 0.0 {
            // Flow from right to left: In -> Out
            (
                PORT_IN,
                PORT_OUT,
                source_pos.0,
                source_pos.1 + config.card_height * 0.5,
                target_pos.0 + config.card_width,
                target_pos.1 + config.card_height * 0.5,
            )
        } else if dy > 0.0 {
            // Same column, target below: Bottom -> Top
            (
                PORT_BOTTOM,
                PORT_TOP,
                source_pos.0 + config.card_width * 0.5,
                source_pos.1 + config.card_height,
                target_pos.0 + config.card_width * 0.5,
                target_pos.1,
            )
        } else {
            // Same column, target above: Top -> Bottom
            (
                PORT_TOP,
                PORT_BOTTOM,
                source_pos.0 + config.card_width * 0.5,
                source_pos.1,
                target_pos.0 + config.card_width * 0.5,
                target_pos.1 + config.card_height,
            )
        };

        let (stroke_color, stroke_style, stroke_width) =
            edge_color_style(edge.edge_type.as_str());

        let arrow_id = format!(
            "arrow-{}-{}-{}",
            edge.source,
            edge.target,
            edge.edge_type.as_str()
        );

        let mut arrow_elem = ExcalidrawElement::default();
        arrow_elem.id = arrow_id.clone();
        arrow_elem.element_type = "arrow".to_string();
        arrow_elem.x = start_x;
        arrow_elem.y = start_y;
        arrow_elem.width = (end_x - start_x).abs();
        arrow_elem.height = (end_y - start_y).abs();
        arrow_elem.points = Some(vec![[0.0, 0.0], [end_x - start_x, end_y - start_y]]);
        arrow_elem.stroke_color = stroke_color.to_string();
        arrow_elem.stroke_style = stroke_style.to_string();
        arrow_elem.stroke_width = stroke_width;
        arrow_elem.end_arrowhead = Some("arrow".to_string());
        arrow_elem.start_binding = Some(PointBinding {
            element_id: source_card_id.clone(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: Some(start_port),
        });
        arrow_elem.end_binding = Some(PointBinding {
            element_id: target_card_id.clone(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: Some(end_port),
        });
        arrow_elem.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                edge_type: Some(edge.edge_type.as_str().to_string()),
                source_id: Some(edge.source.clone()),
                target_id: Some(edge.target.clone()),
                is_pkb_managed: Some(true),
                ..Default::default()
            }),
            extra: HashMap::new(),
        });

        elements.push(arrow_elem);
    }

    ExcalidrawFile {
        file_type: "excalidraw".to_string(),
        version: 2,
        source: Some("https://excalidraw.com".to_string()),
        elements,
        app_state: AppState::default(),
        files: serde_json::json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sugiyama_layout_calculation() {
        let mut n1 = GraphNode::default();
        n1.id = "task-1".to_string();
        n1.label = "Prerequisite Task".to_string();
        n1.status = Some("ready".to_string());

        let mut n2 = GraphNode::default();
        n2.id = "task-2".to_string();
        n2.label = "Dependent Task".to_string();
        n2.status = Some("inbox".to_string());

        let nodes = vec![n1, n2];
        let edges = vec![Edge {
            source: "task-2".to_string(),
            target: "task-1".to_string(),
            edge_type: EdgeType::DependsOn,
        }];

        let config = LayoutConfig::default();
        let positions = compute_sugiyama_layout(&nodes, &edges, &config);

        assert_eq!(positions.len(), 2);
        let pos1 = positions.get("task-1").unwrap();
        let pos2 = positions.get("task-2").unwrap();

        // Prerequisite task-1 should be to the left of dependent task-2
        assert!(pos1.0 < pos2.0, "Prerequisite should precede dependent in X rank");
    }

    #[test]
    fn test_sugiyama_multi_layer_dummy_vertices() {
        let mut n1 = GraphNode::default();
        n1.id = "task-layer0".to_string();
        n1.label = "Layer 0".to_string();

        let mut n2 = GraphNode::default();
        n2.id = "task-layer1".to_string();
        n2.label = "Layer 1".to_string();

        let mut n3 = GraphNode::default();
        n3.id = "task-layer2".to_string();
        n3.label = "Layer 2".to_string();

        let nodes = vec![n1, n2, n3];
        let edges = vec![
            Edge {
                source: "task-layer1".to_string(),
                target: "task-layer0".to_string(),
                edge_type: EdgeType::DependsOn,
            },
            Edge {
                source: "task-layer2".to_string(),
                target: "task-layer1".to_string(),
                edge_type: EdgeType::DependsOn,
            },
            // Edge spanning 2 layers: layer2 depends on layer0
            Edge {
                source: "task-layer2".to_string(),
                target: "task-layer0".to_string(),
                edge_type: EdgeType::DependsOn,
            },
        ];

        let config = LayoutConfig::default();
        let positions = compute_sugiyama_layout(&nodes, &edges, &config);

        let pos0 = positions.get("task-layer0").unwrap();
        let pos1 = positions.get("task-layer1").unwrap();
        let pos2 = positions.get("task-layer2").unwrap();

        assert!(pos0.0 < pos1.0, "Layer 0 should precede Layer 1");
        assert!(pos1.0 < pos2.0, "Layer 1 should precede Layer 2");
    }

    #[test]
    fn test_spatial_frames_for_epics_enclosing_children() {
        let mut epic = GraphNode::default();
        epic.id = "epic-auth".to_string();
        epic.label = "Auth System".to_string();
        epic.node_type = Some("epic".to_string());

        let mut task1 = GraphNode::default();
        task1.id = "task-login".to_string();
        task1.label = "Login UI".to_string();
        task1.parent = Some("epic-auth".to_string());

        let mut task2 = GraphNode::default();
        task2.id = "task-jwt".to_string();
        task2.label = "JWT Tokens".to_string();
        task2.parent = Some("epic-auth".to_string());

        let nodes = vec![epic, task1, task2];
        let scene = generate_excalidraw_scene(&nodes, &[], &LayoutConfig::default());

        let frames: Vec<_> = scene.elements.iter().filter(|e| e.is_frame()).collect();
        assert_eq!(frames.len(), 1, "Should generate exactly 1 Frame for epic-auth");
        let frame = frames[0];
        assert_eq!(frame.id, "frame-epic-auth");

        let cards: Vec<_> = scene.elements.iter().filter(|e| e.is_card()).collect();
        for card in cards {
            if card.pkb_node_id() == Some("task-login") || card.pkb_node_id() == Some("task-jwt") {
                assert_eq!(card.frame_id.as_deref(), Some("frame-epic-auth"));
            }
        }
    }

    #[test]
    fn test_generate_excalidraw_scene_elements() {
        let mut n1 = GraphNode::default();
        n1.id = "task-a".to_string();
        n1.label = "Alpha".to_string();
        n1.status = Some("active".to_string());

        let mut n2 = GraphNode::default();
        n2.id = "task-b".to_string();
        n2.label = "Beta".to_string();
        n2.status = Some("ready".to_string());

        let nodes = vec![n1, n2];
        let edges = vec![Edge {
            source: "task-b".to_string(),
            target: "task-a".to_string(),
            edge_type: EdgeType::DependsOn,
        }];

        let scene = generate_excalidraw_scene(&nodes, &edges, &LayoutConfig::default());
        assert_eq!(scene.elements.len(), 5); // 2 cards + 2 bound texts + 1 arrow

        let arrows: Vec<_> = scene.elements.iter().filter(|e| e.is_arrow()).collect();
        assert_eq!(arrows.len(), 1);
        let arrow = arrows[0];
        assert_eq!(arrow.start_binding.as_ref().unwrap().fixed_point, Some(PORT_IN));
        assert_eq!(arrow.end_binding.as_ref().unwrap().fixed_point, Some(PORT_OUT));
    }

    #[test]
    fn test_truncate_title_utf8_multibyte_safety() {
        let multi_byte = "🚀 PKB 架构设计 with Unicode éàç and emojis ✨🔥";
        let truncated = truncate_title(multi_byte, 10);
        assert!(truncated.ends_with("..."));
        // Ensure no panic and valid char boundary
        let _ = truncated.chars().count();
    }
}
