//! ForceAtlas2 graph layout algorithm.
//!
//! Computes precomputed `x, y` coordinates for graph nodes using the
//! ForceAtlas2 force-directed layout algorithm. Degree-scaled repulsion
//! naturally spaces hubs vs leaves and reveals community structure.
//!
//! Layout parameters are loaded at runtime from `layout.toml` (searched in
//! the current directory, then next to the executable). Edit the file and
//! re-run — no recompilation needed.

use crate::graph::{Edge, EdgeType, GraphNode};
use serde::Deserialize;
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

// ── Defaults (match previous hardcoded values) ──────────────────────────

impl Default for LayoutFile {
    fn default() -> Self {
        Self {
            force: ForceConfig::default(),
            edges: EdgeConfig::default(),
            charges: ChargeConfig::default(),
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

// ── Main algorithm ──────────────────────────────────────────────────────

/// Compute ForceAtlas2 layout and assign `x, y` to each node.
///
/// Mutates nodes in-place. Uses a deterministic seed for reproducibility.
/// Parameters are loaded from `layout.toml` if present, otherwise defaults.
pub fn compute_layout(nodes: &mut [GraphNode], edges: &[Edge]) {
    let n = nodes.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        nodes[0].x = Some(500.0);
        nodes[0].y = Some(500.0);
        return;
    }

    let cfg = load_config();

    // Build node index map: id -> position in slice
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Precompute degree (total edges per node) and charge
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
    let project_nodes: std::collections::HashMap<&str, Vec<usize>> = {
        let mut map: std::collections::HashMap<&str, Vec<usize>> =
            std::collections::HashMap::new();
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

        // Repulsive forces (all pairs, O(n^2))
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = x[j] - x[i];
                let dy = y[j] - y[i];
                let dist = (dx * dx + dy * dy).sqrt().max(0.1);

                // ForceAtlas2: degree-scaled repulsion
                let deg_i = degree[i] as f64 + 1.0;
                let deg_j = degree[j] as f64 + 1.0;
                let force =
                    cfg.force.k_repulsion * charge[i] * charge[j] * deg_i * deg_j / dist;

                let force_x = force * dx / dist;
                let force_y = force * dy / dist;

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
    let margin = cfg.force.viewport * 0.05; // 5% margin
    let usable = cfg.force.viewport - 2.0 * margin;

    for i in 0..n {
        nodes[i].x = Some(margin + (x[i] - x_min) / x_range * usable);
        nodes[i].y = Some(margin + (y[i] - y_min) / y_range * usable);
    }
}
