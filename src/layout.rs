//! Graph layout algorithms: ForceAtlas2, treemap, circle packing, arc diagram.
//!
//! Computes precomputed coordinates for graph nodes. The primary layout
//! (ForceAtlas2 by default) writes to `x, y` on each node. All four layouts
//! are also stored in the `layouts` map under their respective names.
//!
//! Layout parameters are loaded at runtime from `layout.toml` (searched in
//! the current directory, then next to the executable). Edit the file and
//! re-run — no recompilation needed.

use crate::graph::{Edge, EdgeType, GraphNode, LayoutPoint};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Explicit config path set via CLI `--layout-config`.
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Set the layout config file path (call once at startup from CLI).
pub fn set_config_path(path: PathBuf) {
    let _ = CONFIG_PATH.set(path);
}

// ── TOML-deserializable config ──────────────────────────────────────────

/// Top-level layout config file.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct LayoutFile {
    force: ForceConfig,
    edges: EdgeConfig,
    charges: ChargeConfig,
    nodes: NodeConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ForceConfig {
    k_repulsion: f64,
    k_gravity: f64,
    iterations: usize,
    tolerance: f64,
    viewport: f64,
    project_clustering: f64,
    max_displacement: f64,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct EdgeConfig {
    parent: [f64; 2],
    depends_on: [f64; 2],
    soft_depends_on: [f64; 2],
    link: [f64; 2],
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ChargeConfig {
    goal: f64,
    project: f64,
    epic: f64,
    subproject: f64,
    learn: f64,
    default: f64,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct NodeConfig {
    char_width: f64,
    min_width: f64,
    max_width: f64,
    height: f64,
}

// ── Defaults (match previous hardcoded values) ──────────────────────────

impl Default for LayoutFile {
    fn default() -> Self {
        Self {
            force: ForceConfig::default(),
            edges: EdgeConfig::default(),
            charges: ChargeConfig::default(),
            nodes: NodeConfig::default(),
        }
    }
}

impl Default for ForceConfig {
    fn default() -> Self {
        Self {
            k_repulsion: 100.0,
            k_gravity: 1.0,
            iterations: 200,
            tolerance: 1.0,
            viewport: 1000.0,
            project_clustering: 0.5,
            max_displacement: 10.0,
        }
    }
}

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            parent: [1.0, 40.0],
            depends_on: [0.15, 200.0],
            soft_depends_on: [0.08, 250.0],
            link: [0.02, 300.0],
        }
    }
}

impl Default for ChargeConfig {
    fn default() -> Self {
        Self {
            goal: 3.0,
            project: 2.5,
            epic: 2.0,
            subproject: 1.8,
            learn: 1.2,
            default: 1.0,
        }
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            char_width: 8.0,
            min_width: 60.0,
            max_width: 200.0,
            height: 30.0,
        }
    }
}

// ── Config loading ──────────────────────────────────────────────────────

/// Find `layout.toml`: explicit CLI path > cwd > next to executable.
fn find_config() -> Option<PathBuf> {
    // 1. Explicit path from --layout-config
    if let Some(p) = CONFIG_PATH.get() {
        if p.exists() {
            return Some(p.clone());
        }
        tracing::warn!("--layout-config path does not exist: {}", p.display());
    }
    // 2. Search cwd, then exe directory
    let candidates = [
        std::env::current_dir().ok().map(|d| d.join("layout.toml")),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("layout.toml"))),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

fn load_config() -> LayoutFile {
    let Some(path) = find_config() else {
        return LayoutFile::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(cfg) => {
                tracing::info!("loaded layout config from {}", path.display());
                cfg
            }
            Err(e) => {
                tracing::warn!("bad layout.toml (using defaults): {e}");
                LayoutFile::default()
            }
        },
        Err(e) => {
            tracing::warn!("could not read layout.toml (using defaults): {e}");
            LayoutFile::default()
        }
    }
}

// ── Per-node / per-edge helpers ─────────────────────────────────────────

/// Edge force parameters by type: (strength, ideal_distance).
fn edge_weight(edge_type: &EdgeType, cfg: &EdgeConfig) -> (f64, f64) {
    let pair = match edge_type {
        EdgeType::Parent => cfg.parent,
        EdgeType::DependsOn => cfg.depends_on,
        EdgeType::SoftDependsOn => cfg.soft_depends_on,
        EdgeType::Link | EdgeType::Supersedes => cfg.link,
    };
    (pair[0], pair[1])
}

/// Node charge multiplier by type (goals/projects repel more than tasks).
fn node_charge(node_type: Option<&str>, cfg: &ChargeConfig) -> f64 {
    match node_type {
        Some("goal") => cfg.goal,
        Some("project") => cfg.project,
        Some("epic") => cfg.epic,
        Some("subproject") => cfg.subproject,
        Some("learn") => cfg.learn,
        _ => cfg.default,
    }
}

/// Compute per-node rectangle dimensions from label length.
fn compute_node_dims(nodes: &[GraphNode], cfg: &NodeConfig) -> Vec<(f64, f64)> {
    nodes
        .iter()
        .map(|n| {
            let w = (n.label.len() as f64 * cfg.char_width)
                .clamp(cfg.min_width, cfg.max_width);
            (w, cfg.height)
        })
        .collect()
}

// ── Dispatch ────────────────────────────────────────────────────────────

/// Compute all layout algorithms and assign coordinates to each node.
///
/// - ForceAtlas2 (rectangle-aware) → primary `x, y` + `layouts["forceatlas2"]`
/// - Treemap → `layouts["treemap"]` (with `w`, `h`)
/// - Circle packing → `layouts["circle_pack"]` (with `r`)
/// - Arc diagram → `layouts["arc"]`
pub fn compute_layout(nodes: &mut [GraphNode], edges: &[Edge]) {
    let n = nodes.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        nodes[0].x = Some(500.0);
        nodes[0].y = Some(500.0);
        let layouts_to_add = [
            ("forceatlas2", LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: None }),
            ("treemap", LayoutPoint { x: 500.0, y: 500.0, w: Some(1000.0), h: Some(1000.0), r: None }),
            ("circle_pack", LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: Some(500.0) }),
            ("arc", LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: None }),
        ];
        for (name, point) in layouts_to_add {
            nodes[0].layouts.insert(name.into(), point);
        }
        return;
    }

    let cfg = load_config();
    let dims = compute_node_dims(nodes, &cfg.nodes);

    // Build shared index map (owned keys to avoid borrow conflict with &mut nodes)
    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // 1. ForceAtlas2 with rectangle-aware repulsion (primary x/y)
    compute_forceatlas2(nodes, edges, &cfg, &dims, &id_to_idx);

    // 2. Treemap
    compute_treemap(nodes, edges, &cfg, &id_to_idx);

    // 3. Circle packing
    compute_circle_pack(nodes, edges, &cfg, &id_to_idx);

    // 4. Arc diagram
    compute_arc(nodes, &cfg);
}

// ── Rectangle gap helper ────────────────────────────────────────────────

/// Compute the shortest distance between two axis-aligned rectangles.
/// Returns negative values when rectangles overlap.
fn rect_gap(
    x1: f64, y1: f64, hw1: f64, hh1: f64,
    x2: f64, y2: f64, hw2: f64, hh2: f64,
) -> f64 {
    let gap_x = (x2 - x1).abs() - hw1 - hw2;
    let gap_y = (y2 - y1).abs() - hh1 - hh2;
    match (gap_x > 0.0, gap_y > 0.0) {
        (true, true) => (gap_x * gap_x + gap_y * gap_y).sqrt(),
        (true, false) => gap_x,
        (false, true) => gap_y,
        (false, false) => gap_x.max(gap_y), // both negative = overlap
    }
}

// ── ForceAtlas2 (rectangle-aware) ───────────────────────────────────────

fn compute_forceatlas2(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    cfg: &LayoutFile,
    dims: &[(f64, f64)],
    id_to_idx: &HashMap<String, usize>,
) {
    let n = nodes.len();

    // Precompute half-widths and half-heights for rectangle repulsion
    let half_w: Vec<f64> = dims.iter().map(|(w, _)| w / 2.0).collect();
    let half_h: Vec<f64> = dims.iter().map(|(_, h)| h / 2.0).collect();

    // Precompute degree and charge
    let mut degree = vec![0u32; n];
    for edge in edges {
        if let Some(&si) = id_to_idx.get(edge.source.as_str()) {
            degree[si] += 1;
        }
        if let Some(&ti) = id_to_idx.get(edge.target.as_str()) {
            degree[ti] += 1;
        }
    }

    let charge: Vec<f64> = nodes
        .iter()
        .map(|n| node_charge(n.node_type.as_deref(), &cfg.charges))
        .collect();

    // Resolve edge indices once
    let resolved_edges: Vec<(usize, usize, f64)> = edges
        .iter()
        .filter_map(|e| {
            let si = *id_to_idx.get(e.source.as_str())?;
            let ti = *id_to_idx.get(e.target.as_str())?;
            if si == ti {
                return None;
            }
            let (strength, _ideal_dist) = edge_weight(&e.edge_type, &cfg.edges);
            Some((si, ti, strength))
        })
        .collect();

    // Initialize positions deterministically using golden-angle spiral
    let mut x = vec![0.0f64; n];
    let mut y = vec![0.0f64; n];
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    for i in 0..n {
        let r = (i as f64 + 0.5).sqrt() / (n as f64).sqrt() * 400.0;
        let theta = i as f64 * golden_angle;
        x[i] = 500.0 + r * theta.cos();
        y[i] = 500.0 + r * theta.sin();
    }

    // Project clustering: compute project centroids for additional gravity
    let project_nodes: HashMap<&str, Vec<usize>> = {
        let mut map: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            if let Some(ref proj) = node.project {
                map.entry(proj.as_str()).or_default().push(i);
            }
        }
        map
    };

    // ForceAtlas2 iteration
    let mut prev_fx = vec![0.0f64; n];
    let mut prev_fy = vec![0.0f64; n];
    let mut global_speed = 1.0f64;

    for _iter in 0..cfg.force.iterations {
        let mut fx = vec![0.0f64; n];
        let mut fy = vec![0.0f64; n];

        // Repulsive forces (all pairs, O(n^2)) — rectangle-aware
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = x[j] - x[i];
                let dy = y[j] - y[i];
                let center_dist = (dx * dx + dy * dy).sqrt().max(0.1);

                // Use rectangle gap instead of center distance for force magnitude
                let gap = rect_gap(
                    x[i], y[i], half_w[i], half_h[i],
                    x[j], y[j], half_w[j], half_h[j],
                ).max(0.1);

                // ForceAtlas2: degree-scaled repulsion
                let deg_i = degree[i] as f64 + 1.0;
                let deg_j = degree[j] as f64 + 1.0;
                let force =
                    cfg.force.k_repulsion * charge[i] * charge[j] * deg_i * deg_j / gap;

                // Direction still uses center-to-center vector
                let force_x = force * dx / center_dist;
                let force_y = force * dy / center_dist;

                fx[i] -= force_x;
                fy[i] -= force_y;
                fx[j] += force_x;
                fy[j] += force_y;
            }
        }

        // Attractive forces (edges only)
        for &(si, ti, strength) in &resolved_edges {
            let dx = x[ti] - x[si];
            let dy = y[ti] - y[si];
            let dist = (dx * dx + dy * dy).sqrt().max(0.1);

            // ForceAtlas2: linear attraction
            let force = dist * strength;
            let force_x = force * dx / dist;
            let force_y = force * dy / dist;

            fx[si] += force_x;
            fy[si] += force_y;
            fx[ti] -= force_x;
            fy[ti] -= force_y;
        }

        // Gravity (toward center)
        let cx = x.iter().sum::<f64>() / n as f64;
        let cy = y.iter().sum::<f64>() / n as f64;
        for i in 0..n {
            let dx = x[i] - cx;
            let dy = y[i] - cy;
            let dist = (dx * dx + dy * dy).sqrt().max(0.1);
            let deg = degree[i] as f64 + 1.0;
            let force = cfg.force.k_gravity * deg;
            fx[i] -= force * dx / dist;
            fy[i] -= force * dy / dist;
        }

        // Project clustering: attraction toward project centroid
        // Scaled by sqrt(project_size) so larger projects form tighter groups
        if cfg.force.project_clustering > 0.0 {
            for (_proj, members) in &project_nodes {
                if members.len() < 2 {
                    continue;
                }
                let proj_scale = (members.len() as f64).sqrt();
                let pcx: f64 =
                    members.iter().map(|&i| x[i]).sum::<f64>() / members.len() as f64;
                let pcy: f64 =
                    members.iter().map(|&i| y[i]).sum::<f64>() / members.len() as f64;
                for &i in members {
                    let dx = x[i] - pcx;
                    let dy = y[i] - pcy;
                    let dist = (dx * dx + dy * dy).sqrt().max(0.1);
                    let force = cfg.force.project_clustering * proj_scale;
                    fx[i] -= force * dx / dist;
                    fy[i] -= force * dy / dist;
                }
            }
        }

        // Adaptive speed (ForceAtlas2 swing/traction)
        let mut swing = 0.0f64;
        let mut traction = 0.0f64;
        for i in 0..n {
            let dfx = fx[i] - prev_fx[i];
            let dfy = fy[i] - prev_fy[i];
            swing += (dfx * dfx + dfy * dfy).sqrt();

            let avg_x = (fx[i] + prev_fx[i]) / 2.0;
            let avg_y = (fy[i] + prev_fy[i]) / 2.0;
            traction += (avg_x * avg_x + avg_y * avg_y).sqrt();
        }

        if swing > 0.0 {
            let target_speed = cfg.force.tolerance * traction / swing;
            global_speed += (target_speed - global_speed).min(global_speed * 0.5);
            global_speed = global_speed.max(0.01);
        }

        // Apply forces with per-node speed limit
        for i in 0..n {
            let force_mag = (fx[i] * fx[i] + fy[i] * fy[i]).sqrt().max(0.001);
            let node_swing =
                ((fx[i] - prev_fx[i]).powi(2) + (fy[i] - prev_fy[i]).powi(2)).sqrt();
            let node_speed = global_speed / (1.0 + global_speed * node_swing.sqrt());
            let displacement = node_speed * force_mag;
            let capped = displacement.min(cfg.force.max_displacement);

            x[i] += fx[i] / force_mag * capped;
            y[i] += fy[i] / force_mag * capped;
        }

        prev_fx = fx;
        prev_fy = fy;
    }

    // Normalize coordinates to viewport range (0..viewport)
    let x_min = x.iter().cloned().fold(f64::INFINITY, f64::min);
    let x_max = x.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_min = y.iter().cloned().fold(f64::INFINITY, f64::min);
    let y_max = y.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let x_range = (x_max - x_min).max(1.0);
    let y_range = (y_max - y_min).max(1.0);
    let margin = cfg.force.viewport * 0.05;
    let usable = cfg.force.viewport - 2.0 * margin;

    for i in 0..n {
        let nx = margin + (x[i] - x_min) / x_range * usable;
        let ny = margin + (y[i] - y_min) / y_range * usable;
        nodes[i].x = Some(nx);
        nodes[i].y = Some(ny);
        nodes[i].layouts.insert(
            "forceatlas2".into(),
            LayoutPoint { x: nx, y: ny, w: None, h: None, r: None },
        );
    }
}

// ── Treemap (squarified) ────────────────────────────────────────────────

/// Whether a node type should appear in the treemap (task-relevant types only).
fn is_treemap_type(node_type: Option<&str>) -> bool {
    matches!(
        node_type,
        Some("task") | Some("project") | Some("epic") | Some("goal")
            | Some("bug") | Some("action") | Some("subproject") | Some("feature")
            | Some("learn") | Some("milestone")
    )
}

fn compute_treemap(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    cfg: &LayoutFile,
    id_to_idx: &HashMap<String, usize>,
) {
    let n = nodes.len();

    // Filter to task-relevant types for a cleaner, less dense treemap
    let included: Vec<bool> = nodes.iter().map(|n| is_treemap_type(n.node_type.as_deref())).collect();

    // Build parent-child tree from Parent edges (only among included nodes)
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut has_parent = vec![false; n];
    for edge in edges {
        if edge.edge_type == EdgeType::Parent {
            if let (Some(&child_idx), Some(&parent_idx)) = (
                id_to_idx.get(edge.source.as_str()),
                id_to_idx.get(edge.target.as_str()),
            ) {
                if included[child_idx] && included[parent_idx] {
                    children_of.entry(parent_idx).or_default().push(child_idx);
                    has_parent[child_idx] = true;
                }
            }
        }
    }

    // Find roots among included nodes
    let roots: Vec<usize> = (0..n).filter(|&i| included[i] && !has_parent[i]).collect();

    // Compute subtree weight for each node (only included nodes)
    let mut weight = vec![0.0f64; n];
    for i in 0..n {
        if !included[i] {
            continue;
        }
        weight[i] = if nodes[i].downstream_weight > 0.0 {
            nodes[i].downstream_weight
        } else {
            1.0
        };
    }
    // Bottom-up: add children weights to parent
    fn subtree_weight(
        idx: usize,
        children_of: &HashMap<usize, Vec<usize>>,
        weight: &mut [f64],
    ) -> f64 {
        let child_sum: f64 = children_of
            .get(&idx)
            .map(|kids| {
                kids.iter()
                    .map(|&k| subtree_weight(k, children_of, weight))
                    .sum()
            })
            .unwrap_or(0.0);
        if child_sum > 0.0 {
            weight[idx] = child_sum;
        }
        weight[idx].max(1.0)
    }
    for &root in &roots {
        subtree_weight(root, &children_of, &mut weight);
    }

    // Allocate rectangles: (x, y, w, h) per node
    let viewport = cfg.force.viewport;
    let mut rects = vec![(0.0f64, 0.0f64, 0.0f64, 0.0f64); n];

    /// Squarified treemap: lay out items into a rectangle.
    fn squarify(
        items: &mut [(usize, f64)], // (node_idx, weight)
        rect_x: f64,
        rect_y: f64,
        rect_w: f64,
        rect_h: f64,
        rects: &mut [(f64, f64, f64, f64)],
        children_of: &HashMap<usize, Vec<usize>>,
        weight: &[f64],
    ) {
        if items.is_empty() {
            return;
        }
        if items.len() == 1 {
            let (idx, _) = items[0];
            rects[idx] = (rect_x, rect_y, rect_w, rect_h);
            // Recurse into children
            if let Some(kids) = children_of.get(&idx) {
                let padding = rect_w.min(rect_h) * 0.05;
                let inner_x = rect_x + padding;
                let inner_y = rect_y + padding;
                let inner_w = (rect_w - 2.0 * padding).max(1.0);
                let inner_h = (rect_h - 2.0 * padding).max(1.0);
                let mut child_items: Vec<(usize, f64)> =
                    kids.iter().map(|&k| (k, weight[k])).collect();
                child_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                squarify(
                    &mut child_items,
                    inner_x, inner_y, inner_w, inner_h,
                    rects, children_of, weight,
                );
            }
            return;
        }

        // Sort by weight descending
        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let total: f64 = items.iter().map(|(_, w)| w).sum();
        if total <= 0.0 {
            return;
        }

        // Lay out along shorter side
        let vertical = rect_w >= rect_h;

        // Find best row: greedily add items while aspect ratio improves
        let side = if vertical { rect_h } else { rect_w };
        let mut best_row_len = 1;
        let mut best_worst_ratio = f64::INFINITY;

        for row_len in 1..=items.len() {
            let row_sum: f64 = items[..row_len].iter().map(|(_, w)| w).sum();
            let row_frac = row_sum / total;
            let row_extent = if vertical {
                rect_w * row_frac
            } else {
                rect_h * row_frac
            };

            // Compute worst aspect ratio in this row
            let mut worst = 0.0f64;
            for &(_, w) in &items[..row_len] {
                let item_extent = side * (w / row_sum);
                let ratio = if row_extent > item_extent {
                    row_extent / item_extent
                } else {
                    item_extent / row_extent
                };
                worst = worst.max(ratio);
            }

            if worst <= best_worst_ratio {
                best_worst_ratio = worst;
                best_row_len = row_len;
            } else {
                break;
            }
        }

        // Place the row
        let row_sum: f64 = items[..best_row_len].iter().map(|(_, w)| w).sum();
        let row_frac = row_sum / total;

        let (row_x, row_y, row_w, row_h, rem_x, rem_y, rem_w, rem_h) = if vertical {
            let rw = rect_w * row_frac;
            (
                rect_x, rect_y, rw, rect_h,
                rect_x + rw, rect_y, rect_w - rw, rect_h,
            )
        } else {
            let rh = rect_h * row_frac;
            (
                rect_x, rect_y, rect_w, rh,
                rect_x, rect_y + rh, rect_w, rect_h - rh,
            )
        };

        // Place items within the row
        let mut offset = 0.0;
        for &(idx, w) in &items[..best_row_len] {
            let frac = w / row_sum;
            let (ix, iy, iw, ih) = if vertical {
                let h = row_h * frac;
                (row_x, row_y + offset, row_w, h)
            } else {
                let w_ext = row_w * frac;
                (row_x + offset, row_y, w_ext, row_h)
            };
            rects[idx] = (ix, iy, iw, ih);
            offset += if vertical { ih } else { iw };

            // Recurse into children within this cell
            if let Some(kids) = children_of.get(&idx) {
                let padding = iw.min(ih) * 0.05;
                let inner_x = ix + padding;
                let inner_y = iy + padding;
                let inner_w = (iw - 2.0 * padding).max(1.0);
                let inner_h = (ih - 2.0 * padding).max(1.0);
                let mut child_items: Vec<(usize, f64)> =
                    kids.iter().map(|&k| (k, weight[k])).collect();
                child_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                squarify(
                    &mut child_items,
                    inner_x, inner_y, inner_w, inner_h,
                    rects, children_of, weight,
                );
            }
        }

        // Recurse for remaining items
        if best_row_len < items.len() {
            squarify(
                &mut items[best_row_len..],
                rem_x, rem_y, rem_w, rem_h,
                rects, children_of, weight,
            );
        }
    }

    // Lay out roots across the viewport
    let mut root_items: Vec<(usize, f64)> =
        roots.iter().map(|&r| (r, weight[r])).collect();
    root_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    squarify(
        &mut root_items,
        0.0, 0.0, viewport, viewport,
        &mut rects, &children_of, &weight,
    );

    // Store results (only for included nodes — no min-rect clamping to avoid overlaps)
    for i in 0..n {
        if !included[i] {
            continue;
        }
        let (rx, ry, rw, rh) = rects[i];
        // If node wasn't placed (not reachable from roots), give it a default
        let (rx, ry, rw, rh) = if rw == 0.0 && rh == 0.0 {
            (viewport / 2.0, viewport / 2.0, 10.0, 10.0)
        } else {
            (rx, ry, rw, rh)
        };
        nodes[i].layouts.insert(
            "treemap".into(),
            LayoutPoint {
                x: rx + rw / 2.0,
                y: ry + rh / 2.0,
                w: Some(rw),
                h: Some(rh),
                r: None,
            },
        );
    }
}

// ── Circle packing ──────────────────────────────────────────────────────

/// Pack circles with given radii into a compact 2D arrangement.
/// Returns (x, y) positions for each circle (centered around origin).
fn pack_circles(radii: &[f64]) -> Vec<(f64, f64)> {
    let n = radii.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![(0.0, 0.0)];
    }

    let mut pos = vec![(0.0f64, 0.0f64); n];

    // Sort indices by radius descending for better packing
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| radii[b].partial_cmp(&radii[a]).unwrap());

    // Place first two along x-axis
    let d01 = radii[order[0]] + radii[order[1]];
    pos[order[0]] = (-d01 / 2.0, 0.0);
    pos[order[1]] = (d01 / 2.0, 0.0);

    // Place remaining circles tangent to existing pairs
    for ki in 2..n {
        let k = order[ki];
        let rk = radii[k];

        let mut best_pos: Option<(f64, f64)> = None;
        let mut best_dist = f64::INFINITY;

        // Only search recent circles for tangent pairs (O(n*K^2) vs O(n^3))
        let search_start = if ki > 15 { ki - 15 } else { 0 };
        for ai in search_start..ki {
            for bi in (ai + 1)..ki {
                let a = order[ai];
                let b = order[bi];
                if let Some((cx, cy)) = tangent_circle(
                    pos[a].0, pos[a].1, radii[a],
                    pos[b].0, pos[b].1, radii[b],
                    rk,
                ) {
                    // Check no overlap with all placed circles
                    let overlaps = order[..ki].iter().any(|&other| {
                        let dx = cx - pos[other].0;
                        let dy = cy - pos[other].1;
                        let min_d = rk + radii[other];
                        dx * dx + dy * dy < (min_d - 0.01) * (min_d - 0.01)
                    });
                    if !overlaps {
                        let dist = cx * cx + cy * cy;
                        if dist < best_dist {
                            best_dist = dist;
                            best_pos = Some((cx, cy));
                        }
                    }
                }
            }
        }

        // Fallback: place at angle on the outer boundary
        pos[k] = best_pos.unwrap_or_else(|| {
            let angle = ki as f64 * std::f64::consts::TAU / n as f64;
            let outer_r = order[..ki]
                .iter()
                .map(|&j| (pos[j].0.powi(2) + pos[j].1.powi(2)).sqrt() + radii[j])
                .fold(0.0f64, f64::max);
            ((outer_r + rk) * angle.cos(), (outer_r + rk) * angle.sin())
        });
    }

    pos
}

/// Compute the minimum enclosing radius for a set of positioned circles.
fn enclosing_radius(positions: &[(f64, f64)], radii: &[f64]) -> f64 {
    positions
        .iter()
        .zip(radii.iter())
        .map(|((x, y), r)| (x * x + y * y).sqrt() + r)
        .fold(0.0f64, f64::max)
}

fn compute_circle_pack(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    cfg: &LayoutFile,
    id_to_idx: &HashMap<String, usize>,
) {
    let n = nodes.len();
    let viewport = cfg.force.viewport;
    let min_radius_pre = 3.0; // minimum radius before normalization

    // Build parent mapping from Parent edges
    let mut parent_of: Vec<Option<usize>> = vec![None; n];
    for edge in edges {
        if edge.edge_type == EdgeType::Parent {
            if let (Some(&child_idx), Some(&parent_idx)) = (
                id_to_idx.get(edge.source.as_str()),
                id_to_idx.get(edge.target.as_str()),
            ) {
                parent_of[child_idx] = Some(parent_idx);
            }
        }
    }

    // Find root ancestor for each node (iterative to avoid stack overflow)
    let mut root_of = vec![0usize; n];
    for i in 0..n {
        let mut current = i;
        while let Some(p) = parent_of[current] {
            current = p;
        }
        root_of[i] = current;
    }

    let roots: Vec<usize> = (0..n).filter(|&i| parent_of[i].is_none()).collect();

    // Flatten to 2 levels: root → all descendants as direct children
    let mut root_children: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        if parent_of[i].is_some() {
            root_children.entry(root_of[i]).or_default().push(i);
        }
    }

    // Compute leaf radii using log-scale with minimum (avoids sub-pixel circles)
    let scale = 12.0;
    let mut leaf_radius = vec![0.0f64; n];
    for i in 0..n {
        let w = if nodes[i].downstream_weight > 0.0 {
            nodes[i].downstream_weight
        } else {
            1.0
        };
        leaf_radius[i] = ((w + 1.0).ln() * scale).max(min_radius_pre);
    }

    // Pack children within each root, compute root enclosing radii
    let mut child_positions: HashMap<usize, Vec<(usize, f64, f64)>> = HashMap::new();
    let mut root_radius = vec![0.0f64; n];

    for &root in &roots {
        let kids = match root_children.get(&root) {
            Some(k) if !k.is_empty() => k,
            _ => {
                root_radius[root] = leaf_radius[root];
                continue;
            }
        };

        let child_radii: Vec<f64> = kids.iter().map(|&k| leaf_radius[k]).collect();
        let positions = pack_circles(&child_radii);
        let enc_r = enclosing_radius(&positions, &child_radii) * 1.1; // 10% padding
        root_radius[root] = enc_r.max(leaf_radius[root]);

        let cp: Vec<(usize, f64, f64)> = kids
            .iter()
            .enumerate()
            .map(|(j, &kid)| (kid, positions[j].0, positions[j].1))
            .collect();
        child_positions.insert(root, cp);
    }

    // Pack roots in 2D (not a row!) for better viewport utilization
    let root_radii: Vec<f64> = roots.iter().map(|&r| root_radius[r]).collect();
    let root_pos = pack_circles(&root_radii);

    // Convert to absolute positions
    let mut abs_x = vec![0.0f64; n];
    let mut abs_y = vec![0.0f64; n];
    let mut final_radius = vec![0.0f64; n];
    let mut placed = vec![false; n];

    for (ri, &root) in roots.iter().enumerate() {
        abs_x[root] = root_pos[ri].0;
        abs_y[root] = root_pos[ri].1;
        final_radius[root] = root_radius[root];
        placed[root] = true;

        if let Some(children) = child_positions.get(&root) {
            for &(kid, rx, ry) in children {
                abs_x[kid] = abs_x[root] + rx;
                abs_y[kid] = abs_y[root] + ry;
                final_radius[kid] = leaf_radius[kid];
                placed[kid] = true;
            }
        }
    }

    // Place any orphan nodes not reachable from roots
    let orphan_x = abs_x
        .iter()
        .zip(final_radius.iter())
        .filter(|(_, r)| **r > 0.0)
        .map(|(x, r)| x + r)
        .fold(0.0f64, f64::max)
        + 40.0;
    let mut orphan_cursor = orphan_x;
    for i in 0..n {
        if !placed[i] {
            abs_x[i] = orphan_cursor;
            abs_y[i] = 0.0;
            final_radius[i] = min_radius_pre;
            placed[i] = true;
            orphan_cursor += min_radius_pre * 2.0 + 10.0;
        }
    }

    // Normalize to viewport (uniform scaling to preserve circular shapes)
    let x_min = (0..n)
        .map(|i| abs_x[i] - final_radius[i])
        .fold(f64::INFINITY, f64::min);
    let x_max = (0..n)
        .map(|i| abs_x[i] + final_radius[i])
        .fold(f64::NEG_INFINITY, f64::max);
    let y_min = (0..n)
        .map(|i| abs_y[i] - final_radius[i])
        .fold(f64::INFINITY, f64::min);
    let y_max = (0..n)
        .map(|i| abs_y[i] + final_radius[i])
        .fold(f64::NEG_INFINITY, f64::max);

    let x_range = (x_max - x_min).max(1.0);
    let y_range = (y_max - y_min).max(1.0);
    let margin = viewport * 0.05;
    let usable = viewport - 2.0 * margin;

    // Uniform scale to fit, then center on both axes
    let scale_factor = usable / x_range.max(y_range);
    let x_offset = margin + (usable - x_range * scale_factor) / 2.0;
    let y_offset = margin + (usable - y_range * scale_factor) / 2.0;

    let min_radius_post = 2.0; // minimum visible radius after normalization
    for i in 0..n {
        let nx = x_offset + (abs_x[i] - x_min) * scale_factor;
        let ny = y_offset + (abs_y[i] - y_min) * scale_factor;
        let nr = (final_radius[i] * scale_factor).max(min_radius_post);
        nodes[i].layouts.insert(
            "circle_pack".into(),
            LayoutPoint { x: nx, y: ny, w: None, h: None, r: Some(nr) },
        );
    }
}

/// Find position of a circle tangent to two existing circles.
/// Returns the position closer to the origin (better packing).
fn tangent_circle(
    x1: f64, y1: f64, r1: f64,
    x2: f64, y2: f64, r2: f64,
    r: f64,
) -> Option<(f64, f64)> {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let d = (dx * dx + dy * dy).sqrt();
    let d1 = r1 + r;
    let d2 = r2 + r;

    if d > d1 + d2 || d < (d1 - d2).abs() || d < 1e-10 {
        return None;
    }

    let a = (d1 * d1 - d2 * d2 + d * d) / (2.0 * d);
    let h_sq = d1 * d1 - a * a;
    if h_sq < 0.0 {
        return None;
    }
    let h = h_sq.sqrt();

    let px = x1 + a * dx / d;
    let py = y1 + a * dy / d;

    let cx1 = px + h * dy / d;
    let cy1 = py - h * dx / d;
    let cx2 = px - h * dy / d;
    let cy2 = py + h * dx / d;

    // Pick the one closer to origin
    let dist1 = cx1 * cx1 + cy1 * cy1;
    let dist2 = cx2 * cx2 + cy2 * cy2;
    if dist1 <= dist2 {
        Some((cx1, cy1))
    } else {
        Some((cx2, cy2))
    }
}

// ── Arc diagram ─────────────────────────────────────────────────────────

fn compute_arc(nodes: &mut [GraphNode], cfg: &LayoutFile) {
    let n = nodes.len();
    let viewport = cfg.force.viewport;
    let max_arc_nodes = 200;

    // Filter to active task-like nodes — arc is unreadable beyond ~200 nodes
    let mut candidates: Vec<usize> = (0..n)
        .filter(|&i| {
            let active = matches!(
                nodes[i].status.as_deref(),
                Some("active") | Some("in_progress") | Some("ready") | Some("blocked")
            );
            let task_like = matches!(
                nodes[i].node_type.as_deref(),
                Some("task") | Some("project") | Some("epic") | Some("goal")
                    | Some("bug") | Some("action") | Some("subproject") | Some("feature")
            );
            active && task_like
        })
        .collect();

    if candidates.is_empty() {
        return;
    }

    // Take top N by downstream_weight (most important tasks first)
    candidates.sort_by(|&a, &b| {
        nodes[b]
            .downstream_weight
            .partial_cmp(&nodes[a].downstream_weight)
            .unwrap()
    });
    candidates.truncate(max_arc_nodes);

    // Sort selected nodes for layout: project -> depth -> priority -> label
    candidates.sort_by(|&a, &b| {
        let proj_a = nodes[a].project.as_deref().unwrap_or("");
        let proj_b = nodes[b].project.as_deref().unwrap_or("");
        proj_a
            .is_empty()
            .cmp(&proj_b.is_empty()) // empty projects sort last
            .then_with(|| proj_a.cmp(proj_b))
            .then_with(|| nodes[a].depth.cmp(&nodes[b].depth))
            .then_with(|| {
                nodes[a]
                    .priority
                    .unwrap_or(i32::MAX)
                    .cmp(&nodes[b].priority.unwrap_or(i32::MAX))
            })
            .then_with(|| nodes[a].label.cmp(&nodes[b].label))
    });

    let m = candidates.len();
    let margin = viewport * 0.05;
    let usable = viewport - 2.0 * margin;
    let y_center = viewport / 2.0;
    let band = viewport * 0.04; // 40 units per band in 1000 viewport

    for (pos, &idx) in candidates.iter().enumerate() {
        let x = if m > 1 {
            margin + pos as f64 * usable / (m - 1) as f64
        } else {
            viewport / 2.0
        };

        // Y-banding by node type: different types at different y levels
        let y_offset = match nodes[idx].node_type.as_deref() {
            Some("goal") => -2.0 * band,
            Some("project") | Some("subproject") => -band,
            Some("epic") => 0.0,
            Some("task") | Some("action") | Some("bug") => band,
            _ => 2.0 * band,
        };

        nodes[idx].layouts.insert(
            "arc".into(),
            LayoutPoint { x, y: y_center + y_offset, w: None, h: None, r: None },
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Create a minimal GraphNode for layout testing.
    fn make_node(id: &str, node_type: &str, project: Option<&str>, dw: f64) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            path: PathBuf::from(format!("tasks/{id}.md")),
            label: id.to_string(),
            node_type: Some(node_type.to_string()),
            project: project.map(|s| s.to_string()),
            downstream_weight: dw,
            layouts: HashMap::new(),
            // defaults for everything else
            tags: vec![],
            status: Some("active".into()),
            priority: None,
            order: 0,
            parent: None,
            depends_on: vec![],
            soft_depends_on: vec![],
            blocks: vec![],
            soft_blocks: vec![],
            children: vec![],
            due: None,
            created: None,
            modified: None,
            assignee: None,
            complexity: None,
            source: None,
            confidence: None,
            supersedes: None,
            depth: 0,
            word_count: 0,
            leaf: true,
            raw_links: vec![],
            permalinks: vec![],
            task_id: None,
            pagerank: 0.0,
            betweenness: 0.0,
            indegree: 0,
            outdegree: 0,
            backlink_count: 0,
            stakeholder_exposure: false,
            reachable: false,
            assumptions: vec![],
            x: None,
            y: None,
        }
    }

    /// Build a test graph with ~100 nodes across 5 projects, each with a
    /// project root and child tasks, plus some orphan nodes.
    fn build_test_graph() -> (Vec<GraphNode>, Vec<Edge>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for p in 0..5 {
            let proj = format!("proj-{p}");
            let root_id = format!("proj-{p}-root");
            nodes.push(make_node(&root_id, "project", Some(&proj), 20.0));

            for t in 0..15 {
                let task_id = format!("proj-{p}-task-{t}");
                let mut node = make_node(&task_id, "task", Some(&proj), 1.0 + t as f64);
                node.parent = Some(root_id.clone());
                nodes.push(node);
                edges.push(Edge {
                    source: task_id,
                    target: root_id.clone(),
                    edge_type: EdgeType::Parent,
                });
            }
        }

        // Add some goals and epics for arc banding variety
        nodes.push(make_node("goal-1", "goal", None, 50.0));
        nodes.push(make_node("epic-1", "epic", Some("proj-0"), 10.0));

        // Add orphan nodes (no parent, no project)
        for i in 0..5 {
            nodes.push(make_node(&format!("orphan-{i}"), "note", None, 1.0));
        }

        (nodes, edges)
    }

    #[test]
    fn test_circle_pack_fills_viewport() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        let ys: Vec<f64> = nodes
            .iter()
            .filter_map(|n| n.layouts.get("circle_pack").map(|lp| lp.y))
            .collect();
        let y_min = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let y_range = y_max - y_min;

        // Must use more than 20% of the viewport on y-axis (was ~10% before fix)
        assert!(
            y_range > 200.0,
            "circle_pack y range too narrow: {y_range:.1} (need > 200)"
        );
    }

    #[test]
    fn test_circle_pack_minimum_radius() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        let radii: Vec<f64> = nodes
            .iter()
            .filter_map(|n| n.layouts.get("circle_pack").and_then(|lp| lp.r))
            .collect();
        let min_r = radii.iter().cloned().fold(f64::INFINITY, f64::min);

        // Minimum post-normalization radius should be >= 2.0 (visible)
        assert!(
            min_r >= 1.5,
            "circle_pack minimum radius too small: {min_r:.3} (need >= 1.5)"
        );
    }

    #[test]
    fn test_arc_has_multiple_y_bands() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        let ys: Vec<f64> = nodes
            .iter()
            .filter_map(|n| n.layouts.get("arc").map(|lp| lp.y))
            .collect();

        // Quantize to detect distinct bands (round to nearest integer)
        let unique_bands: HashSet<i32> = ys.iter().map(|y| *y as i32).collect();
        assert!(
            unique_bands.len() > 1,
            "arc layout should have multiple y bands, got {} unique values",
            unique_bands.len()
        );
    }

    #[test]
    fn test_arc_y_range() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        let ys: Vec<f64> = nodes
            .iter()
            .filter_map(|n| n.layouts.get("arc").map(|lp| lp.y))
            .collect();
        let y_min = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        // Y range should be > 0 (not all on same line)
        assert!(
            y_max - y_min > 50.0,
            "arc y range should be > 50, got {:.1}",
            y_max - y_min
        );
    }

    #[test]
    fn test_treemap_only_task_types() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        // Task-like nodes should have treemap layout
        let treemap_count = nodes
            .iter()
            .filter(|n| n.layouts.contains_key("treemap"))
            .count();
        assert!(treemap_count > 0, "no treemap layouts assigned");

        // Non-task nodes (type "note") should NOT have treemap layout
        for node in &nodes {
            if node.node_type.as_deref() == Some("note") {
                assert!(
                    !node.layouts.contains_key("treemap"),
                    "note {} should not have treemap layout",
                    node.id
                );
            }
        }
    }

    #[test]
    fn test_arc_filters_to_active_tasks() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        // Arc should only include active task-like nodes
        let arc_count = nodes
            .iter()
            .filter(|n| n.layouts.contains_key("arc"))
            .count();
        // All our test task nodes are active, so they should be included
        assert!(arc_count > 0, "no arc layouts assigned");
        assert!(
            arc_count <= 200,
            "arc should cap at 200 nodes, got {arc_count}"
        );
    }

    #[test]
    fn test_all_layouts_within_viewport() {
        let (mut nodes, edges) = build_test_graph();
        compute_layout(&mut nodes, &edges);

        // Only check nodes that actually have the layout (filtered layouts skip some nodes)
        for layout_name in &["forceatlas2", "treemap", "circle_pack", "arc"] {
            for node in &nodes {
                if let Some(lp) = node.layouts.get(*layout_name) {
                    assert!(
                        lp.x >= 0.0 && lp.x <= 1000.0,
                        "{layout_name} x out of range for {}: {:.1}",
                        node.id,
                        lp.x
                    );
                    assert!(
                        lp.y >= 0.0 && lp.y <= 1000.0,
                        "{layout_name} y out of range for {}: {:.1}",
                        node.id,
                        lp.y
                    );
                }
            }
        }
    }

    #[test]
    fn test_pack_circles_basic() {
        let radii = vec![10.0, 8.0, 6.0, 4.0];
        let positions = pack_circles(&radii);

        assert_eq!(positions.len(), 4);

        // No overlaps
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                let dx = positions[i].0 - positions[j].0;
                let dy = positions[i].1 - positions[j].1;
                let dist = (dx * dx + dy * dy).sqrt();
                let min_dist = radii[i] + radii[j] - 0.1; // small tolerance
                assert!(
                    dist >= min_dist,
                    "circles {i} and {j} overlap: dist={dist:.2}, min={min_dist:.2}"
                );
            }
        }
    }

    #[test]
    fn test_pack_circles_single() {
        let positions = pack_circles(&[5.0]);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0], (0.0, 0.0));
    }

    #[test]
    fn test_pack_circles_empty() {
        let positions = pack_circles(&[]);
        assert!(positions.is_empty());
    }
}
