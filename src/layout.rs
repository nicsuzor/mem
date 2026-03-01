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
        nodes[0].layouts.insert(
            "forceatlas2".into(),
            LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: None },
        );
        nodes[0].layouts.insert(
            "treemap".into(),
            LayoutPoint { x: 500.0, y: 500.0, w: Some(1000.0), h: Some(1000.0), r: None },
        );
        nodes[0].layouts.insert(
            "circle_pack".into(),
            LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: Some(500.0) },
        );
        nodes[0].layouts.insert(
            "arc".into(),
            LayoutPoint { x: 500.0, y: 500.0, w: None, h: None, r: None },
        );
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

/// Copy a named layout's coordinates to the primary `x`/`y` fields.
pub fn promote_layout(nodes: &mut [GraphNode], layout_name: &str) {
    for node in nodes.iter_mut() {
        if let Some(lp) = node.layouts.get(layout_name) {
            node.x = Some(lp.x);
            node.y = Some(lp.y);
        }
    }
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

        // Project clustering: gentle attraction toward project centroid
        if cfg.force.project_clustering > 0.0 {
            for (_proj, members) in &project_nodes {
                if members.len() < 2 {
                    continue;
                }
                let pcx: f64 =
                    members.iter().map(|&i| x[i]).sum::<f64>() / members.len() as f64;
                let pcy: f64 =
                    members.iter().map(|&i| y[i]).sum::<f64>() / members.len() as f64;
                for &i in members {
                    let dx = x[i] - pcx;
                    let dy = y[i] - pcy;
                    let dist = (dx * dx + dy * dy).sqrt().max(0.1);
                    let force =
                        cfg.force.project_clustering * (degree[i] as f64 + 1.0);
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

fn compute_treemap(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    cfg: &LayoutFile,
    id_to_idx: &HashMap<String, usize>,
) {
    let n = nodes.len();

    // Build parent-child tree from Parent edges
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut has_parent = vec![false; n];
    for edge in edges {
        if edge.edge_type == EdgeType::Parent {
            if let (Some(&child_idx), Some(&parent_idx)) = (
                id_to_idx.get(edge.source.as_str()),
                id_to_idx.get(edge.target.as_str()),
            ) {
                children_of.entry(parent_idx).or_default().push(child_idx);
                has_parent[child_idx] = true;
            }
        }
    }

    // Find roots (nodes without parents)
    let roots: Vec<usize> = (0..n).filter(|&i| !has_parent[i]).collect();

    // Compute subtree weight for each node
    let mut weight = vec![0.0f64; n];
    for i in 0..n {
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
                &mut items[best_row_len..].to_vec(),
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

    // Store results
    for i in 0..n {
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

fn compute_circle_pack(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    cfg: &LayoutFile,
    id_to_idx: &HashMap<String, usize>,
) {
    let n = nodes.len();
    let viewport = cfg.force.viewport;

    // Build parent-child tree from Parent edges
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut has_parent = vec![false; n];
    for edge in edges {
        if edge.edge_type == EdgeType::Parent {
            if let (Some(&child_idx), Some(&parent_idx)) = (
                id_to_idx.get(edge.source.as_str()),
                id_to_idx.get(edge.target.as_str()),
            ) {
                children_of.entry(parent_idx).or_default().push(child_idx);
                has_parent[child_idx] = true;
            }
        }
    }

    let roots: Vec<usize> = (0..n).filter(|&i| !has_parent[i]).collect();

    // Compute weight
    let mut weight = vec![0.0f64; n];
    for i in 0..n {
        weight[i] = if nodes[i].downstream_weight > 0.0 {
            nodes[i].downstream_weight
        } else {
            1.0
        };
    }

    // Bottom-up: compute radii
    let scale = 15.0; // base scale factor
    let mut radius = vec![0.0f64; n];
    let mut rel_x = vec![0.0f64; n]; // relative position within parent
    let mut rel_y = vec![0.0f64; n];

    fn pack_node(
        idx: usize,
        children_of: &HashMap<usize, Vec<usize>>,
        weight: &[f64],
        radius: &mut [f64],
        rel_x: &mut [f64],
        rel_y: &mut [f64],
        scale: f64,
    ) {
        let kids = match children_of.get(&idx) {
            Some(k) if !k.is_empty() => k.clone(),
            _ => {
                // Leaf: radius from weight
                radius[idx] = weight[idx].sqrt() * scale;
                return;
            }
        };

        // Recursively pack children first
        for &kid in &kids {
            pack_node(kid, children_of, weight, radius, rel_x, rel_y, scale);
        }

        // Sort children by radius descending for better packing
        let mut sorted_kids: Vec<usize> = kids.clone();
        sorted_kids.sort_by(|&a, &b| radius[b].partial_cmp(&radius[a]).unwrap());

        // Place children using a simple greedy algorithm
        if sorted_kids.len() == 1 {
            let k = sorted_kids[0];
            rel_x[k] = 0.0;
            rel_y[k] = 0.0;
            radius[idx] = radius[k] * 1.15; // padding
        } else {
            // Place first two along x-axis
            let k0 = sorted_kids[0];
            let k1 = sorted_kids[1];
            let d01 = radius[k0] + radius[k1];
            rel_x[k0] = -d01 / 2.0;
            rel_y[k0] = 0.0;
            rel_x[k1] = d01 / 2.0;
            rel_y[k1] = 0.0;

            // Place remaining children tangent to two existing circles
            for ki in 2..sorted_kids.len() {
                let k = sorted_kids[ki];
                let rk = radius[k];

                // Find best position: try tangent to each pair, pick closest to origin
                let mut best_x = 0.0f64;
                let mut best_y = 0.0f64;
                let mut best_dist = f64::INFINITY;

                for ai in 0..ki {
                    for bi in (ai + 1)..ki {
                        let a = sorted_kids[ai];
                        let b = sorted_kids[bi];
                        // Place circle tangent to circles a and b
                        if let Some((cx, cy)) = tangent_circle(
                            rel_x[a], rel_y[a], radius[a],
                            rel_x[b], rel_y[b], radius[b],
                            rk,
                        ) {
                            // Check no overlap with existing circles
                            let overlaps = sorted_kids[..ki].iter().any(|&other| {
                                let dx = cx - rel_x[other];
                                let dy = cy - rel_y[other];
                                let min_dist = rk + radius[other];
                                dx * dx + dy * dy < (min_dist - 0.01) * (min_dist - 0.01)
                            });
                            if !overlaps {
                                let dist = cx * cx + cy * cy;
                                if dist < best_dist {
                                    best_dist = dist;
                                    best_x = cx;
                                    best_y = cy;
                                }
                            }
                        }
                    }
                }

                // Fallback: place at angle around the group
                if best_dist == f64::INFINITY {
                    let angle = ki as f64 * std::f64::consts::TAU / sorted_kids.len() as f64;
                    let outer_r = sorted_kids[..ki]
                        .iter()
                        .map(|&j| {
                            (rel_x[j] * rel_x[j] + rel_y[j] * rel_y[j]).sqrt() + radius[j]
                        })
                        .fold(0.0f64, f64::max);
                    best_x = (outer_r + rk) * angle.cos();
                    best_y = (outer_r + rk) * angle.sin();
                }

                rel_x[k] = best_x;
                rel_y[k] = best_y;
            }

            // Compute enclosing radius
            let mut max_extent = 0.0f64;
            for &k in &sorted_kids {
                let ext = (rel_x[k] * rel_x[k] + rel_y[k] * rel_y[k]).sqrt() + radius[k];
                max_extent = max_extent.max(ext);
            }
            radius[idx] = max_extent * 1.1; // 10% padding
        }
    }

    for &root in &roots {
        pack_node(root, &children_of, &weight, &mut radius, &mut rel_x, &mut rel_y, scale);
    }

    // Assign absolute positions: top-down pass
    let mut abs_x = vec![0.0f64; n];
    let mut abs_y = vec![0.0f64; n];
    let mut placed = vec![false; n];

    // Place roots in a row
    let total_root_width: f64 = roots.iter().map(|&r| radius[r] * 2.0).sum::<f64>()
        + (roots.len().saturating_sub(1)) as f64 * 20.0;
    let mut cursor_x = -total_root_width / 2.0;
    for &root in &roots {
        abs_x[root] = cursor_x + radius[root];
        abs_y[root] = 0.0;
        placed[root] = true;
        cursor_x += radius[root] * 2.0 + 20.0;
    }

    fn place_children(
        idx: usize,
        children_of: &HashMap<usize, Vec<usize>>,
        abs_x: &mut [f64],
        abs_y: &mut [f64],
        rel_x: &[f64],
        rel_y: &[f64],
        placed: &mut [bool],
    ) {
        if let Some(kids) = children_of.get(&idx) {
            for &k in kids {
                if !placed[k] {
                    abs_x[k] = abs_x[idx] + rel_x[k];
                    abs_y[k] = abs_y[idx] + rel_y[k];
                    placed[k] = true;
                    place_children(k, children_of, abs_x, abs_y, rel_x, rel_y, placed);
                }
            }
        }
    }

    for &root in &roots {
        place_children(root, &children_of, &mut abs_x, &mut abs_y, &rel_x, &rel_y, &mut placed);
    }

    // Place any orphan nodes
    let mut orphan_cursor = cursor_x + 40.0;
    for i in 0..n {
        if !placed[i] {
            abs_x[i] = orphan_cursor;
            abs_y[i] = 0.0;
            radius[i] = scale;
            placed[i] = true;
            orphan_cursor += scale * 2.0 + 10.0;
        }
    }

    // Normalize to viewport
    let x_min = abs_x.iter().zip(radius.iter()).map(|(x, r)| x - r).fold(f64::INFINITY, f64::min);
    let x_max = abs_x.iter().zip(radius.iter()).map(|(x, r)| x + r).fold(f64::NEG_INFINITY, f64::max);
    let y_min = abs_y.iter().zip(radius.iter()).map(|(y, r)| y - r).fold(f64::INFINITY, f64::min);
    let y_max = abs_y.iter().zip(radius.iter()).map(|(y, r)| y + r).fold(f64::NEG_INFINITY, f64::max);

    let x_range = (x_max - x_min).max(1.0);
    let y_range = (y_max - y_min).max(1.0);
    let scale_factor = viewport * 0.9 / x_range.max(y_range);
    let margin = viewport * 0.05;

    for i in 0..n {
        let nx = margin + (abs_x[i] - x_min) * scale_factor;
        let ny = margin + (abs_y[i] - y_min) * scale_factor;
        let nr = radius[i] * scale_factor;
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

    // Build sort key: (project, depth, priority, label)
    let mut indices: Vec<usize> = (0..n).collect();
    indices.sort_by(|&a, &b| {
        let proj_a = nodes[a].project.as_deref().unwrap_or("");
        let proj_b = nodes[b].project.as_deref().unwrap_or("");
        proj_a
            .cmp(proj_b)
            .then_with(|| nodes[a].depth.cmp(&nodes[b].depth))
            .then_with(|| {
                nodes[a]
                    .priority
                    .unwrap_or(i32::MAX)
                    .cmp(&nodes[b].priority.unwrap_or(i32::MAX))
            })
            .then_with(|| nodes[a].label.cmp(&nodes[b].label))
    });

    let margin = viewport * 0.05;
    let usable = viewport - 2.0 * margin;
    let y_center = viewport / 2.0;

    for (pos, &idx) in indices.iter().enumerate() {
        let x = if n > 1 {
            margin + pos as f64 * usable / (n - 1) as f64
        } else {
            viewport / 2.0
        };
        nodes[idx].layouts.insert(
            "arc".into(),
            LayoutPoint { x, y: y_center, w: None, h: None, r: None },
        );
    }
}
