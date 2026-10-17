//! Graph store — builds, queries, and exports knowledge graphs.
//!
//! [`GraphStore`] holds all nodes, edges, and pre-computed task indices.
//! Build from `PkbDocument`s via [`GraphStore::build`], then query with
//! the various accessor methods.

use crate::graph::{self, deduplicate_vec, Edge, EdgeType, GraphNode, ParseWarning};
pub use crate::graph::{FlowEdge, ResolvedLink};
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

/// One node as emitted by `export_graph` JSON: the node's serialized fields
/// plus the engine's ranking, so consumers never re-derive it.
#[derive(Serialize, Deserialize, Debug)]
pub struct ExportNode {
    #[serde(flatten)]
    pub node: GraphNode,
    /// Flow display metadata structure (§7.2, `export_graph`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<crate::display_rank::TaskDisplay>,
    /// 1-based position among exported nodes ordered by `display_cmp` (§7.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_rank: Option<usize>,
}

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
    /// Hard-deadline cliff lane tasks (§7.2, `export_graph`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cliff: Vec<String>,
}

/// `export_graph` JSON document: [`OutputGraph`] with ranked [`ExportNode`]s.
#[derive(Serialize, Deserialize, Debug)]
pub struct ExportGraph {
    pub nodes: Vec<ExportNode>,
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ready: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Hard-deadline cliff lane tasks (§7.2, `export_graph`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cliff: Vec<String>,
}

pub const DEFAULT_DIVERGENCE_THRESHOLD_DAYS: i64 = 14;
pub const DIVERGENCE_WEIGHT_THRESHOLD: f64 = 0.75;

/// Result of a stated-vs-revealed weight divergence check.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WeightDivergence {
    pub source_id: String,
    pub source_label: String,
    pub target_id: String,
    pub target_label: String,
    pub stated_weight: String,
    pub numeric_weight: f64,
    pub justification: String,
    pub days_since_interaction: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
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
    /// Generation stamp recorded at build/patch time for validate-on-read cache invalidation
    generation: crate::pkb::GenerationStamp,
    pub flow_input: Option<crate::flow::FlowInput>,
}

/// Document types considered actionable work items in task trees and dashboards.
/// `project` and `epic` are retired as node types (epic collapsed into task;
/// project = polecat.yaml routing slug in frontmatter). Legacy `type: project`
/// and `type: epic` files read-coerce to `task`.
pub const ACTIONABLE_TYPES: &[&str] = &["task", "learn", "pr"];

/// Document types that represent claimable work items — leaf tasks a worker can actually do.
/// Excludes containers (epic) and observational types (learn).
pub const CLAIMABLE_TYPES: &[&str] = &["task"];

/// Status-level ready predicate shared by the in-memory ready-gate and the MCP
/// index export, so the two never silently diverge.
///
/// A leaf, claimable, dependency-resolved task is *ready* when its (alias-resolved)
/// status is explicitly `ready`/`queued`, OR when it is still `inbox` but has
/// **graduated** — i.e. it carries acceptance criteria. The `inbox → ready`
/// transition is therefore computed, not a manual status edit: a bare `inbox`
/// task (no AC) is never surfaced as ready. Callers still apply the leaf,
/// claimable-type, and dependency checks separately.
pub fn is_ready_status(status: &str, has_acceptance_criteria: bool) -> bool {
    matches!(status, "ready" | "queued") || (status == "inbox" && has_acceptance_criteria)
}

impl GraphStore {
    /// Build a complete graph from parsed PKB documents.
    ///
    /// Full pipeline:
    /// 1. Extract `GraphNode`s from `PkbDocument`s
    /// 2. Build lookup indices (permalink -> path, path -> id)
    /// 3. Resolve links and frontmatter refs into edges
    /// 4. Compute inverse relationships (depends_on -> blocks, etc.)
    /// 5. Compute degree/centrality, scope, uncertainty and target ancestors
    /// 6. Compute flow figures and decision values (`crate::flow`)
    /// 7. Classify ready/blocked tasks (flow display layer)
    pub fn build(docs: &[PkbDocument], pkb_root: &Path) -> Self {
        let nodes: Vec<GraphNode> = docs.par_iter().map(GraphNode::from_pkb_document).collect();
        Self::build_internal(nodes, pkb_root, true, None, true)
    }

    /// Build a complete graph from parsed PKB documents, including similarity edges.
    ///
    /// Convenience wrapper that locks the store, builds an embedding snapshot,
    /// drops the lock, and proceeds. Prefer
    /// [`Self::build_with_embeddings`] from MCP-server hot paths so the
    /// caller can hold the read lock as briefly as possible.
    pub fn build_with_store(
        docs: &[PkbDocument],
        pkb_root: &Path,
        store: &crate::vectordb::VectorStore,
    ) -> Self {
        let snapshot = store.averaged_embeddings();
        Self::build_with_embeddings(docs, pkb_root, &snapshot)
    }

    /// Build a complete graph from parsed PKB documents, using a pre-built
    /// embedding snapshot for similarity edges. Caller is responsible for
    /// taking + dropping any store read lock before calling.
    pub fn build_with_embeddings(
        docs: &[PkbDocument],
        pkb_root: &Path,
        embeddings_by_path: &HashMap<String, Vec<f32>>,
    ) -> Self {
        let nodes: Vec<GraphNode> = docs.par_iter().map(GraphNode::from_pkb_document).collect();
        Self::build_internal(nodes, pkb_root, true, Some(embeddings_by_path), true)
    }

    /// Build from a directory: scan, parse (with relative paths), build graph.
    pub fn build_from_directory(root: &Path) -> Self {
        let files = crate::pkb::scan_directory(root);
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
        Self::build_internal(nodes_vec, pkb_root, true, None, true)
    }

    /// Fast incremental rebuild: skips centrality metrics (PageRank, betweenness).
    ///
    /// A single-file update shifts PageRank/betweenness by a negligible amount,
    /// and recomputing them is O(V*E) — the dominant cost of a rebuild at
    /// multi-thousand-node PKB sizes. The previous values on the cloned nodes
    /// are preserved (they drift slowly until the next full rebuild).
    pub fn rebuild_from_nodes_fast(nodes: HashMap<String, GraphNode>, pkb_root: &Path) -> Self {
        let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
        Self::build_internal(nodes_vec, pkb_root, false, None, true)
    }

    /// Fast incremental rebuild with similarity edges. Convenience wrapper
    /// around [`Self::rebuild_from_nodes_fast_with_embeddings`] — see notes
    /// on [`Self::build_with_store`].
    pub fn rebuild_from_nodes_fast_with_store(
        nodes: HashMap<String, GraphNode>,
        pkb_root: &Path,
        store: &crate::vectordb::VectorStore,
    ) -> Self {
        let snapshot = store.averaged_embeddings();
        Self::rebuild_from_nodes_fast_with_embeddings(nodes, pkb_root, &snapshot)
    }

    /// Fast incremental rebuild with similarity edges, given a pre-built
    /// embedding snapshot. Use from hot paths so the store read lock is
    /// dropped before the (slow) graph build runs.
    pub fn rebuild_from_nodes_fast_with_embeddings(
        nodes: HashMap<String, GraphNode>,
        pkb_root: &Path,
        embeddings_by_path: &HashMap<String, Vec<f32>>,
    ) -> Self {
        let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
        Self::build_internal(nodes_vec, pkb_root, false, Some(embeddings_by_path), true)
    }

    /// Tier-1 incremental rebuild: runs the full O(V+E) pipeline but skips the
    /// O(N²) similarity-edge step. Existing `SimilarTo` edges from the prior
    /// graph are carried over so query surfaces that depend on them remain
    /// usable until a follow-up Tier-2 rebuild refreshes them.
    ///
    /// Used on the synchronous `update_task` write path so the handler returns
    /// in tens of ms while ready/blocked classification, flow figures, and
    /// target ancestors stay fresh.
    pub fn rebuild_from_nodes_skip_similarity(
        nodes: HashMap<String, GraphNode>,
        pkb_root: &Path,
        prior_similarity_edges: Vec<Edge>,
    ) -> Self {
        let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
        let mut g = Self::build_internal(nodes_vec, pkb_root, false, None, false);
        g.edges.extend(prior_similarity_edges);
        g
    }

    /// Internal helper to build GraphStore from a vector of nodes.
    ///
    /// When `include_centrality` is false, PageRank and betweenness computation
    /// are skipped — any pre-existing values on the input nodes are retained.
    /// When `include_similarity` is false, similarity-edge computation is
    /// skipped (Tier-1 incremental rebuild path).
    fn build_internal(
        mut nodes: Vec<GraphNode>,
        pkb_root: &Path,
        include_centrality: bool,
        opt_embeddings: Option<&HashMap<String, Vec<f32>>>,
        include_similarity: bool,
    ) -> Self {
        // 1. Sort nodes by id for deterministic processing across all rebuild and build paths.
        nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        // 2. Build lookup maps
        // Node paths may be relative — reconstruct absolute for canonicalize & link resolution.
        // Canonical paths are cached on each GraphNode so incremental rebuilds avoid the
        // O(N) fs-stat cost. Refs mem-fd02c2f9.
        let _t_lookup = std::time::Instant::now();
        let mut id_map: HashMap<String, String> = HashMap::with_capacity(nodes.len() * 2); // permalink -> abs_path
        let mut path_to_id: HashMap<String, String> = HashMap::with_capacity(nodes.len()); // abs_path -> id

        let mut canonicalize_misses: usize = 0;
        for n in nodes.iter_mut() {
            // Ghost nodes carry an empty `path` and are re-registered via
            // virtual paths during ghost discovery below. Skip the canonicalize
            // loop for them — pkb_root.join("").canonicalize() collides on a
            // single key for every ghost and burns one fs-stat per ghost.
            if n.path.as_os_str().is_empty() {
                continue;
            }
            let abs_path = if let Some(ref cached) = n.canonical_abs_path {
                cached.clone()
            } else {
                let full_path = if n.path.is_absolute() {
                    n.path.clone()
                } else {
                    pkb_root.join(&n.path)
                };
                let resolved = full_path
                    .canonicalize()
                    .unwrap_or(full_path)
                    .to_string_lossy()
                    .to_string();
                n.canonical_abs_path = Some(resolved.clone());
                canonicalize_misses += 1;
                resolved
            };
            path_to_id.insert(abs_path.clone(), n.id.clone());
        }

        // Register lookup keys in two passes, authoritative first.
        //
        // A single pass let a key derived from one file's *name* overwrite the
        // key another file claims as its *id*, because both kinds were registered
        // in file-scan order with last-write-wins. That is how a stored reference
        // came to resolve to a node that did not own the string: `personal.md`
        // (id `personal-66647271`) claimed the derived key `personal`, and any
        // `parent: personal` bound to it. Two passes make an exact `id:` or
        // explicit `permalink:` unconditionally beat every derived key, so a
        // reference can only ever bind to a derived key when no node claims that
        // string as its identity at all.
        //
        // Within a pass, ties are broken by node id rather than by scan order, so
        // repeated rebuilds of the same tree always agree. Previously the winner
        // depended on directory-walk order, which is why one string could resolve
        // differently at different call sites.
        let mut key_owner: HashMap<String, String> = HashMap::with_capacity(nodes.len() * 2);
        let mut ambiguous_keys: Vec<(String, String, String)> = Vec::new();
        for authoritative in [true, false] {
            let mut claims: Vec<(&str, &str, &str)> = Vec::new();
            for n in nodes.iter() {
                if n.path.as_os_str().is_empty() {
                    continue;
                }
                let abs_path = match n.canonical_abs_path {
                    Some(ref p) => p.as_str(),
                    None => continue,
                };
                let keys = if authoritative {
                    &n.permalinks
                } else {
                    &n.weak_keys
                };
                for key in keys {
                    claims.push((key.as_str(), n.id.as_str(), abs_path));
                }
            }
            // Deterministic winner for same-tier collisions: lowest node id.
            claims.sort_unstable();
            for (key, owner_id, abs_path) in claims {
                match key_owner.get(key) {
                    // Already claimed in this same pass — a genuine ambiguity.
                    Some(prev) if authoritative && prev != owner_id => {
                        ambiguous_keys.push((key.to_string(), prev.clone(), owner_id.to_string()));
                    }
                    Some(_) => {}
                    None => {
                        key_owner.insert(key.to_string(), owner_id.to_string());
                        id_map.insert(key.to_string(), abs_path.to_string());
                    }
                }
            }
        }
        if !ambiguous_keys.is_empty() {
            // Not fatal: the graph still builds and resolution is deterministic.
            // But two nodes claiming one identity means references to that string
            // are guesses, so say so rather than picking one in silence.
            tracing::warn!(
                target: "graph::identity",
                count = ambiguous_keys.len(),
                sample = ?ambiguous_keys.iter().take(10).collect::<Vec<_>>(),
                "identity keys claimed by more than one node; references to these strings are ambiguous"
            );
        }
        tracing::debug!(
            target: "perf::graph_rebuild",
            phase = "lookup_maps",
            n_nodes = nodes.len(),
            canonicalize_misses = canonicalize_misses,
            elapsed_ms = _t_lookup.elapsed().as_secs_f64() * 1000.0
        );

        // 2b. Discover ghost nodes (referenced but not in files)
        // Scans all reference fields for IDs that weren't resolved to files,
        // then creates minimal virtual nodes for them.
        let mut referenced_ids = HashSet::with_capacity(nodes.len() * 2);
        for n in &nodes {
            if let Some(ref p) = n.parent {
                referenced_ids.insert(p.clone());
            }
            for c in &n.closes {
                referenced_ids.insert(c.clone());
            }
            for ct in &n.contributes_to {
                referenced_ids.insert(ct.to.clone());
            }
            for d in &n.depends_on {
                referenced_ids.insert(d.clone());
            }
            for sd in &n.soft_depends_on {
                referenced_ids.insert(sd.clone());
            }
            for b in &n.blocks {
                referenced_ids.insert(b.clone());
            }
            for sb in &n.soft_blocks {
                referenced_ids.insert(sb.clone());
            }
            for child in &n.children {
                referenced_ids.insert(child.clone());
            }
            for follow in &n.follow_up_tasks {
                referenced_ids.insert(follow.clone());
            }
            for link in &n.links {
                if let Some(ref to) = link.to {
                    referenced_ids.insert(to.clone());
                }
                if let Some(ref from) = link.from {
                    referenced_ids.insert(from.clone());
                }
            }
        }

        let mut sorted_referenced_ids: Vec<String> = referenced_ids.into_iter().collect();
        sorted_referenced_ids.sort_unstable();

        let mut ghost_nodes = Vec::with_capacity(sorted_referenced_ids.len() / 4);
        for ref_id in sorted_referenced_ids {
            let lower = ref_id.to_lowercase();
            if let std::collections::hash_map::Entry::Vacant(e) = id_map.entry(lower) {
                let mut ghost = GraphNode::default();
                ghost.id = ref_id.clone();
                ghost.label = ref_id.clone();
                // Guess node type from ID prefix
                if ref_id.starts_with("epic-")
                    || ref_id.starts_with("task-")
                    || ref_id.starts_with("project-")
                {
                    ghost.node_type = Some("task".to_string());
                } else if ref_id.starts_with("goal-")
                    || ref_id.starts_with("target-")
                    || ref_id.starts_with("capability-")
                {
                    ghost.node_type = Some("target".to_string());
                } else if ref_id.starts_with("pr-") {
                    ghost.node_type = Some("pr".to_string());
                }

                let virtual_path = format!("/virtual/{}", ref_id);
                path_to_id.insert(virtual_path.clone(), ref_id.clone());
                e.insert(virtual_path);
                ghost_nodes.push(ghost);
            }
        }
        nodes.extend(ghost_nodes);
        nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        // 3. Build edges from links and frontmatter refs
        let _t_edges = std::time::Instant::now();
        let edges: Vec<Edge> = nodes
            .par_iter()
            .flat_map(|n| build_node_edges(n, &id_map, &path_to_id, pkb_root))
            .collect();
        tracing::debug!(target: "perf::graph_rebuild", phase = "build_edges", n_nodes = nodes.len(), n_edges = edges.len(), elapsed_ms = _t_edges.elapsed().as_secs_f64() * 1000.0);

        // Deduplicate edges by (source, target, type)
        let mut seen: HashSet<(String, String, String)> = HashSet::with_capacity(edges.len());
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
        let _t = std::time::Instant::now();
        compute_inverses(&mut nodes, &edges, &id_map, &path_to_id);
        tracing::debug!(target: "perf::graph_rebuild", phase = "inverses", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // Attach duplicate disagreement warnings (pkb-flow-engine §3.1 R4, E4)
        let (_, duplicate_warnings) = resolve_flow_edges_from_nodes(&nodes);
        for (target_node_id, warning) in duplicate_warnings {
            if let Some(node) = nodes.iter_mut().find(|n| n.id == target_node_id) {
                node.parse_warnings.push(warning);
            }
        }

        // 5. Compute degree metrics (indegree/outdegree)
        let _t = std::time::Instant::now();
        compute_degree_metrics(&mut nodes, &edges);
        tracing::debug!(target: "perf::graph_rebuild", phase = "degree_metrics", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 6. Compute centrality metrics (PageRank, betweenness).
        //    Skipped on incremental rebuilds — O(V*E) dominates write latency at
        //    multi-thousand-node PKB sizes, and single-file updates barely shift
        //    these scores. Prior values on the input nodes are retained instead.
        if include_centrality {
            let _t = std::time::Instant::now();
            compute_centrality_metrics(&mut nodes, &edges);
            tracing::debug!(target: "perf::graph_rebuild", phase = "centrality", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);
        }

        // 7. Compute derived properties: scope (subtree size) and uncertainty
        //    (read by the flow display layer's exploration weights).
        let _t = std::time::Instant::now();
        compute_scope(&mut nodes);
        compute_uncertainty(&mut nodes);
        tracing::debug!(target: "perf::graph_rebuild", phase = "scope_uncertainty", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 8. Compute project field (nearest ancestor with node_type == "project")
        let _t = std::time::Instant::now();
        compute_project_field(&mut nodes, &edges);
        tracing::debug!(target: "perf::graph_rebuild", phase = "project_field", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 9a. Compute target ancestors (materialised field)
        let _t = std::time::Instant::now();
        compute_target_ancestors(&mut nodes, &edges);
        tracing::debug!(target: "perf::graph_rebuild", phase = "target_ancestors", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 9b. Compute similarity edges if an embedding snapshot is provided.
        // threshold 0.85 as default for materialised edges.
        // Tier-1 callers pass include_similarity=false to skip this O(N²)
        // step; they merge prior similarity edges in afterwards.
        let _t_sim = std::time::Instant::now();
        let similarity_edges = if include_similarity {
            if let Some(snapshot) = opt_embeddings {
                compute_similarity_edges(&nodes, &edges, snapshot, 0.85)
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        tracing::debug!(target: "perf::graph_rebuild", phase = "similarity_edges", n_nodes = nodes.len(), n_sim_edges = similarity_edges.len(), elapsed_ms = _t_sim.elapsed().as_secs_f64() * 1000.0);

        let mut edges = edges;
        edges.extend(similarity_edges);

        // 9c. Compute reachable set (upstream BFS from active leaves)
        //    Mark nodes reachable from active leaves via BFS.
        let _t = std::time::Instant::now();
        let reachable_set = find_reachable_set(&nodes, &edges);
        for node in &mut nodes {
            node.reachable = reachable_set.contains(&node.id);
        }
        // 9d. Compute flow and decision values (pkb-flow-engine §3.1, §5, §5.3)
        let _t_flow = std::time::Instant::now();
        let flow_input = build_flow_input(&nodes);
        let mut flow_outputs = crate::flow::compute_flow(&flow_input);
        crate::flow::compute_decision_value(&flow_input, &mut flow_outputs);
        for node in &mut nodes {
            node.flow = flow_outputs.get(&node.id).cloned();
        }
        tracing::debug!(target: "perf::graph_rebuild", phase = "flow_computation", elapsed_ms = _t_flow.elapsed().as_secs_f64() * 1000.0);

        // 10. Build node map and classify tasks (flow display layer, §7.2)
        let _t = std::time::Instant::now();
        let mut node_map: HashMap<String, GraphNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let today = chrono::Utc::now().date_naive();
        let flow_map: HashMap<String, crate::flow::FlowOutput> = flow_outputs.into_iter().collect();
        let (ready, blocked, roots) = crate::display_rank::classify_graph_nodes(
            &node_map,
            Some(&flow_map),
            today,
            crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS,
        );
        tracing::debug!(target: "perf::graph_rebuild", phase = "classify_tasks", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 11. Compute divergence anomalies and set flags on nodes (default 14 days)
        let _t = std::time::Instant::now();
        compute_divergence_anomalies(&mut node_map, &blocked, DEFAULT_DIVERGENCE_THRESHOLD_DAYS);
        tracing::debug!(target: "perf::graph_rebuild", phase = "divergence_anomalies", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        // 12. Build resolution map for flexible node lookup
        let _t = std::time::Instant::now();
        let resolution_map = build_resolution_map(&node_map);
        tracing::debug!(target: "perf::graph_rebuild", phase = "resolution_map", elapsed_ms = _t.elapsed().as_secs_f64() * 1000.0);

        let generation = crate::pkb::scan_generation(pkb_root);

        GraphStore {
            nodes: node_map,
            edges,
            ready,
            blocked,
            roots,
            resolution_map,
            generation,
            flow_input: Some(flow_input),
        }
    }

    // -----------------------------------------------------------------------
    // Query API
    // -----------------------------------------------------------------------

    /// Generation stamp recorded at build/patch time.
    pub fn generation(&self) -> crate::pkb::GenerationStamp {
        self.generation
    }

    /// Set generation stamp (e.g. after in-place mutation or sync).
    pub fn set_generation(&mut self, gen: crate::pkb::GenerationStamp) {
        self.generation = gen;
    }

    pub fn get_node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.get(id)
    }

    /// Resolve flow edges and duplicate warnings across all nodes in the store (pkb-flow-engine §3.1, R4).
    pub fn resolve_flow_edges_with_warnings(&self) -> (Vec<FlowEdge>, Vec<(String, ParseWarning)>) {
        resolve_flow_edges_from_nodes(self.nodes.values())
    }

    /// Resolve all flow edges across the store.
    pub fn resolve_flow_edges(&self) -> Vec<FlowEdge> {
        self.resolve_flow_edges_with_warnings().0
    }

    /// Outgoing flow links from a given node (pkb-flow-engine §7.2).
    pub fn links_out(&self, node_id: &str) -> Vec<ResolvedLink> {
        let mut links = Vec::new();
        for edge in self.resolve_flow_edges() {
            if edge.src == node_id {
                let strength = edge.strength();
                links.push(ResolvedLink {
                    to: Some(edge.dst),
                    from: None,
                    label: edge.label,
                    quantum: edge.quantum,
                    probability: edge.probability,
                    effect: edge.effect,
                    justification: edge.justification,
                    set_by: edge.set_by,
                    strength,
                });
            }
        }
        links.sort_by(|a, b| {
            (a.to.as_deref().unwrap_or(""), a.label.as_str())
                .cmp(&(b.to.as_deref().unwrap_or(""), b.label.as_str()))
        });
        links
    }

    /// Incoming flow links into a given node (pkb-flow-engine §7.2).
    pub fn links_in(&self, node_id: &str) -> Vec<ResolvedLink> {
        let mut links = Vec::new();
        for edge in self.resolve_flow_edges() {
            if edge.dst == node_id {
                let strength = edge.strength();
                links.push(ResolvedLink {
                    to: None,
                    from: Some(edge.src),
                    label: edge.label,
                    quantum: edge.quantum,
                    probability: edge.probability,
                    effect: edge.effect,
                    justification: edge.justification,
                    set_by: edge.set_by,
                    strength,
                });
            }
        }
        links.sort_by(|a, b| {
            (a.from.as_deref().unwrap_or(""), a.label.as_str())
                .cmp(&(b.from.as_deref().unwrap_or(""), b.label.as_str()))
        });
        links
    }

    /// Flow input graph used to compute flow figures.
    pub fn flow_input(&self) -> Option<&crate::flow::FlowInput> {
        self.flow_input.as_ref()
    }

    /// Compute routes, truncation status, and one-sentence explanation for a node (§7.2, `get_task`).
    pub fn routes_for_node(
        &self,
        id: &str,
    ) -> Option<(std::collections::BTreeMap<String, crate::flow::TargetRoutes>, bool, String)> {
        let node = self.get_node(id)?;
        let flow = node.flow.as_ref()?;
        let flow_input = self.flow_input.as_ref()?;
        let routes = crate::flow::compute_routes(flow_input, flow, id);
        let truncated = routes.values().any(|tr| tr.routes_truncated);
        let explanation = crate::flow::build_explanation(id, &routes, &flow.stake);
        Some((routes, truncated, explanation))
    }

    /// Collect IDs of all descendants (recursive via children) whose status is open
    /// (not done, cancelled, or archived), excluding `node_type: template` (which are
    /// perpetual meta-definitions that are never 'done'). Used to enforce open-children constraint
    /// before closing a parent.
    pub fn open_descendants(&self, id: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut stack = Vec::new();
        let mut visited = HashSet::new();
        if let Some(node) = self.get_node(id) {
            visited.insert(id.to_string());
            stack.extend(node.children.iter().cloned());
        }
        while let Some(child_id) = stack.pop() {
            if !visited.insert(child_id.clone()) {
                continue;
            }
            if let Some(child) = self.get_node(&child_id) {
                if child.node_type.as_deref() != Some("template")
                    && !crate::graph::is_closed_for_hierarchy(child.status.as_deref())
                {
                    result.push(child_id.clone());
                }
                stack.extend(child.children.iter().cloned());
            }
        }
        result
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

    /// Extract a clone of the existing `SimilarTo` edges. Used by the Tier-1
    /// rebuild path to carry over prior similarity edges while skipping the
    /// O(N²) recomputation.
    pub fn similarity_edges_cloned(&self) -> Vec<Edge> {
        self.edges
            .iter()
            .filter(|e| matches!(e.edge_type, EdgeType::SimilarTo))
            .cloned()
            .collect()
    }

    /// Replace `self.nodes[id]` with the supplied node, dropping any pre-existing
    /// node mapped to the same path. Used by Tier-2 swap to re-apply node
    /// patches that landed during the in-flight rebuild.
    pub fn replace_node(&mut self, node: GraphNode) {
        let abs = node.canonical_abs_path.clone();
        if let Some(ref new_path) = abs {
            self.nodes.retain(|_id, existing| {
                existing.canonical_abs_path.as_ref() != Some(new_path) || existing.id == node.id
            });
        }
        self.nodes.insert(node.id.clone(), node);
    }

    /// Cheap in-place upsert of a single node. Used by the synchronous write
    /// path so handlers return in microseconds; the full pipeline (edges,
    /// flow figures, target ancestors, similarity edges) runs in the
    /// background coalesced rebuild.
    ///
    /// Carries over computed fields from the prior node so derived state
    /// (pagerank, children/subtasks inverses) stays consistent until the
    /// background rebuild refreshes them.
    /// Caller is expected to hold the graph write lock.
    pub fn upsert_node_in_place(&mut self, mut new_node: GraphNode, pkb_root: &Path) {
        // Resolve canonical absolute path once, mirroring the build pipeline.
        if new_node.canonical_abs_path.is_none() && !new_node.path.as_os_str().is_empty() {
            let full_path = if new_node.path.is_absolute() {
                new_node.path.clone()
            } else {
                pkb_root.join(&new_node.path)
            };
            new_node.canonical_abs_path = Some(
                full_path
                    .canonicalize()
                    .unwrap_or(full_path)
                    .to_string_lossy()
                    .to_string(),
            );
        }

        // Carry over derived/computed fields from the prior node (same id or
        // same path). Without this, an in-place patch wipes children/subtasks
        // /pagerank/etc. until the background rebuild
        // recomputes them — observable as `leaf=true` flips and ready-order
        // churn between Tier-1 and Tier-2.
        let prior = self.nodes.get(&new_node.id).cloned().or_else(|| {
            // Same path, different id (id rename) — find by canonical path.
            new_node.canonical_abs_path.as_ref().and_then(|p| {
                self.nodes
                    .values()
                    .find(|n| n.canonical_abs_path.as_ref() == Some(p))
                    .cloned()
            })
        });
        // Capture the old parent and old id before consuming `prior`, so we can
        // fix up parent→child edges below after the derived-field carry-over.
        // old_node_id differs from new_node.id when the same file is renamed.
        let old_parent_id: Option<String> = prior.as_ref().and_then(|n| n.parent.clone());
        let old_node_id: Option<String> = prior.as_ref().map(|n| n.id.clone());

        if let Some(old) = prior {
            // Derived: only carry over if the incoming node didn't supply one.
            // The from_pkb_document path always emits empty children/subtasks/
            // contributed_by (they come from inverses), so this preserves them.
            if new_node.children.is_empty() {
                new_node.children = old.children;
            }
            if new_node.subtasks.is_empty() {
                new_node.subtasks = old.subtasks;
            }
            if new_node.contributed_by.is_empty() {
                new_node.contributed_by = old.contributed_by;
            }
            if new_node.superseded_by.is_empty() {
                new_node.superseded_by = old.superseded_by;
            }
            if new_node.target_ancestors.is_empty() {
                new_node.target_ancestors = old.target_ancestors;
            }
            new_node.leaf = new_node.children.is_empty();
            new_node.pagerank = old.pagerank;
            new_node.betweenness = old.betweenness;
            new_node.indegree = old.indegree;
            new_node.outdegree = old.outdegree;
            new_node.backlink_count = old.backlink_count;
            if new_node.project.is_none() {
                new_node.project = old.project;
            }
        }

        // Synchronously fix parent→child edges when a reparent occurred.
        //
        // `upsert_node_in_place` only patches the single child node; the
        // background Tier-2 rebuild recomputes inverses from disk, but it runs
        // asynchronously.  Until it lands, `open_descendants(old_parent)` still
        // traverses `old_parent.children` — which still contained the child —
        // causing `complete_task(old_parent)` to reject with "open children"
        // even though the reparent already succeeded on disk and in `get_task`.
        //
        // Fix: update the live `children` lists of both the old and new parent
        // nodes in the same graph write-lock window, making the close-gate
        // immediately consistent with what `get_task` reports.
        let new_parent_id = new_node.parent.clone();
        let reparented = old_parent_id != new_parent_id;
        let renamed = old_node_id
            .as_ref()
            .is_some_and(|old_id| old_id != &new_node.id);

        if reparented || renamed {
            if let Some(ref old_pid) = old_parent_id {
                if let Some(p) = self.nodes.get_mut(old_pid.as_str()) {
                    let old_nid = old_node_id.as_ref().unwrap_or(&new_node.id);
                    p.children.retain(|c| c != old_nid);
                    p.leaf = p.children.is_empty();
                }
            }
            if let Some(ref new_pid) = new_parent_id {
                if let Some(p) = self.nodes.get_mut(new_pid.as_str()) {
                    if !p.children.contains(&new_node.id) {
                        p.children.push(new_node.id.clone());
                    }
                    p.leaf = false;
                }
            }
        }

        if new_node.project.is_some() {
            new_node.parse_warnings.retain(|w| w.field != "project");
        }

        // Drop any pre-existing node mapped to the same path (handles id
        // renames where the same file changes its frontmatter id).
        if let Some(ref new_path) = new_node.canonical_abs_path {
            self.nodes.retain(|_id, existing| {
                existing.canonical_abs_path.as_ref() != Some(new_path) || existing.id == new_node.id
            });
        }

        // Patch resolution map for the new node's keys. Old keys are not
        // removed — they may collide on rename, but the background rebuild
        // will rebuild the full map. Stale keys resolving to a missing id
        // are handled by `resolve()` (it returns None on lookup miss).
        //
        // Authoritative keys (own id, task id, explicit permalink) overwrite
        // whatever held the key. Derived keys (filename stem, scraped id-prefix,
        // title) must not: this path runs on every single-node write, and letting
        // a newly-written file's *name* seize a key that is another node's *id*
        // reintroduces the shadowing bug one node at a time, between full
        // rebuilds. Mirrors the tier order in `build_resolution_map`.
        let id_lower = new_node.id.to_lowercase();
        self.resolution_map
            .insert(id_lower.clone(), new_node.id.clone());
        if let Some(ref tid) = new_node.task_id {
            self.resolution_map
                .insert(tid.to_lowercase(), new_node.id.clone());
        }
        for pl in &new_node.permalinks {
            self.resolution_map.insert(pl.clone(), new_node.id.clone());
        }

        let claim_weak = |map: &mut HashMap<String, String>, key: String| {
            if key.is_empty() {
                return;
            }
            map.entry(key).or_insert_with(|| new_node.id.clone());
        };
        for wk in &new_node.weak_keys {
            claim_weak(&mut self.resolution_map, wk.clone());
        }
        if let Some(stem) = new_node.path.file_stem() {
            claim_weak(
                &mut self.resolution_map,
                stem.to_string_lossy().to_lowercase(),
            );
        }
        claim_weak(&mut self.resolution_map, new_node.label.to_lowercase());

        self.nodes.insert(new_node.id.clone(), new_node);
        self.generation = crate::pkb::scan_generation(pkb_root);
    }

    /// Cheap in-place removal. Drops the node from `nodes`; stale edges and
    /// resolution-map entries are left for the background rebuild to fix.
    pub fn remove_node_in_place(&mut self, id: &str, pkb_root: &Path) {
        self.nodes.remove(id);
        self.resolution_map.retain(|_, target| target != id);
        self.generation = crate::pkb::scan_generation(pkb_root);
    }

    /// Re-run classification (ready / blocked / roots) from the current nodes
    /// and update self. Cheap O(V+E). Called by Tier-2 swap after merging
    /// patched nodes.
    pub fn reclassify(&mut self) {
        let today = chrono::Utc::now().date_naive();
        let flow_map: HashMap<String, crate::flow::FlowOutput> = self
            .nodes
            .iter()
            .filter_map(|(id, n)| n.flow.as_ref().map(|f| (id.clone(), f.clone())))
            .collect();
        let (ready, blocked, roots) = crate::display_rank::classify_graph_nodes(
            &self.nodes,
            Some(&flow_map),
            today,
            crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS,
        );
        self.ready = ready;
        self.blocked = blocked;
        self.roots = roots;
    }

    /// Direct access to the node map (for Tier-2 patch-merge).
    pub fn nodes_map(&self) -> &HashMap<String, GraphNode> {
        &self.nodes
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

    /// Direct access to the ready-task ID list (in classification order).
    pub fn ready_ids(&self) -> &[String] {
        &self.ready
    }

    pub fn blocked_tasks(&self) -> Vec<&GraphNode> {
        self.blocked
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn is_blocked(&self, id: &str) -> bool {
        let lower = id.to_lowercase();
        self.blocked.iter().any(|s| s.to_lowercase() == lower)
    }

    /// Sort a slice of node refs in place by canonical flow display order (display_cmp).
    pub fn sort_by_display(
        nodes: &mut [&GraphNode],
        today: chrono::NaiveDate,
        buffer_days: i64,
    ) {
        Self::sort_by_display_with_sort(
            nodes,
            crate::display_rank::TaskSort::Default,
            today,
            buffer_days,
        );
    }

    /// Sort a slice of node refs in place by flow display order with an explicit sort key.
    pub fn sort_by_display_with_sort(
        nodes: &mut [&GraphNode],
        sort: crate::display_rank::TaskSort,
        today: chrono::NaiveDate,
        buffer_days: i64,
    ) {
        let items: HashMap<String, crate::display_rank::DisplayItem> = nodes
            .iter()
            .map(|node| {
                let dt =
                    crate::display_rank::DisplayTask::from_graph_node(node, node.flow.as_ref());
                (node.id.clone(), dt.to_display_item(today, buffer_days))
            })
            .collect();

        nodes.sort_by(|a, b| {
            let item_a = &items[&a.id];
            let item_b = &items[&b.id];
            crate::display_rank::display_cmp_by(item_a, item_b, sort)
        });
    }

    /// Focus picks under the flow engine (§7.3, `pkb focus`):
    /// the cliff lane, then the head of display_cmp over ready work.
    pub fn flow_focus_picks(
        &self,
        max: usize,
        today: chrono::NaiveDate,
        buffer_days: i64,
    ) -> (Vec<String>, Vec<String>) {
        let mut candidate_map: HashMap<String, &GraphNode> = HashMap::new();

        for id in &self.ready {
            if let Some(node) = self.nodes.get(id) {
                candidate_map.insert(node.id.clone(), node);
            }
        }

        let mut candidates: Vec<&GraphNode> = candidate_map.into_values().collect();
        Self::sort_by_display(&mut candidates, today, buffer_days);

        let mut cliff_picks = Vec::new();
        let mut regular_picks = Vec::new();

        for node in candidates {
            let due_date = node.due.as_deref().and_then(crate::display_rank::parse_due_date);
            let eff = crate::display_rank::resolve_effort_days(node.effort.as_deref());
            let resolved_class = crate::display_rank::DeadlineClass::resolve(
                node.deadline_class.map(|dc| match dc {
                    crate::graph::DeadlineClass::Fake => crate::display_rank::DeadlineClass::Fake,
                    crate::graph::DeadlineClass::Soft => crate::display_rank::DeadlineClass::Soft,
                    crate::graph::DeadlineClass::Hard => crate::display_rank::DeadlineClass::Hard,
                }),
                due_date.is_some(),
            );
            if crate::display_rank::is_on_cliff(due_date, resolved_class, eff, today, buffer_days) {
                cliff_picks.push(node.id.clone());
            } else {
                regular_picks.push(node.id.clone());
            }
        }

        if cliff_picks.len() > max {
            cliff_picks.truncate(max);
        }
        let remaining = max.saturating_sub(cliff_picks.len());
        if regular_picks.len() > remaining {
            regular_picks.truncate(remaining);
        }

        (cliff_picks, regular_picks)
    }

    /// Focus picks for today: the cliff lane, then the head of `display_cmp`
    /// over ready work, at most `max` in total.
    pub fn focus_picks(&self, max: usize) -> Vec<String> {
        let today = chrono::Utc::now().date_naive();
        let (cliff, regular) =
            self.flow_focus_picks(max, today, crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS);
        let mut all = cliff;
        all.extend(regular);
        all
    }

    /// Returns all open actionable tasks (type in [`ACTIONABLE_TYPES`] and not completed/archived).
    pub fn all_tasks(&self) -> Vec<&GraphNode> {
        self.actionable_tasks(false)
    }

    /// Returns actionable tasks (type in [`ACTIONABLE_TYPES`]), optionally including completed/archived tasks.
    pub fn actionable_tasks(&self, include_done: bool) -> Vec<&GraphNode> {
        let mut tasks: Vec<&GraphNode> = self
            .nodes
            .values()
            .filter(|n| {
                let is_actionable = match n.node_type.as_deref() {
                    Some(t) => ACTIONABLE_TYPES.contains(&t),
                    None => n.task_id.is_some(),
                };
                if !is_actionable {
                    return false;
                }
                if !include_done && crate::graph::is_completed(n.status.as_deref()) {
                    return false;
                }
                true
            })
            .collect();
        Self::sort_by_display(
            &mut tasks,
            chrono::Utc::now().date_naive(),
            crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS,
        );
        tasks
    }

    /// Returns all nodes that carry a task ID, optionally including completed/archived nodes.
    pub fn all_nodes_with_id(&self, include_done: bool) -> Vec<&GraphNode> {
        let mut tasks: Vec<&GraphNode> = self
            .nodes
            .values()
            .filter(|n| {
                if n.task_id.is_none() {
                    return false;
                }
                if !include_done && crate::graph::is_completed(n.status.as_deref()) {
                    return false;
                }
                true
            })
            .collect();
        Self::sort_by_display(
            &mut tasks,
            chrono::Utc::now().date_naive(),
            crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS,
        );
        tasks
    }

    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Get the dependency tree for a node (BFS through depends_on).
    /// Returns (node_id, depth) pairs. Unbounded depth — see
    /// [`Self::dependency_tree_bounded`] for a depth-limited variant.
    pub fn dependency_tree(&self, id: &str) -> Vec<(String, usize)> {
        self.dependency_tree_bounded(id, usize::MAX)
    }

    /// Get the dependency tree for a node (BFS through depends_on), stopping
    /// once `max_depth` hops have been traversed. `max_depth == usize::MAX`
    /// is unbounded (equivalent to [`Self::dependency_tree`]).
    /// Returns (node_id, depth) pairs.
    pub fn dependency_tree_bounded(&self, id: &str, max_depth: usize) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let start_id = self.resolve(id).map(|n| n.id.as_str()).unwrap_or(id);
        queue.push_back((start_id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            let lower = current_id.to_lowercase();
            if !visited.insert(lower) {
                continue;
            }
            if depth > 0 {
                result.push((current_id.clone(), depth));
            }
            if depth >= max_depth {
                continue;
            }
            if let Some(node) = self.resolve(&current_id) {
                for dep_id in &node.depends_on {
                    if !visited.contains(&dep_id.to_lowercase()) {
                        queue.push_back((dep_id.clone(), depth + 1));
                    }
                }
            }
        }
        result
    }

    /// Get what this node blocks (BFS through blocks).
    /// Returns (node_id, depth) pairs. Unbounded depth — see
    /// [`Self::blocks_tree_bounded`] for a depth-limited variant.
    pub fn blocks_tree(&self, id: &str) -> Vec<(String, usize)> {
        self.blocks_tree_bounded(id, usize::MAX)
    }

    /// Get what this node blocks (BFS through blocks), stopping once
    /// `max_depth` hops have been traversed. `max_depth == usize::MAX` is
    /// unbounded (equivalent to [`Self::blocks_tree`]).
    /// Returns (node_id, depth) pairs.
    pub fn blocks_tree_bounded(&self, id: &str, max_depth: usize) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let start_id = self.resolve(id).map(|n| n.id.as_str()).unwrap_or(id);
        queue.push_back((start_id.to_string(), 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            let lower = current_id.to_lowercase();
            if !visited.insert(lower) {
                continue;
            }
            if depth > 0 {
                result.push((current_id.clone(), depth));
            }
            if depth >= max_depth {
                continue;
            }
            if let Some(node) = self.resolve(&current_id) {
                for blocked_id in &node.blocks {
                    if !visited.contains(&blocked_id.to_lowercase()) {
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

    /// Reject an attempt to use `parent_id` as a structural parent when it
    /// resolves to a strategic target/goal node. Targets are "black holes":
    /// invisible attractors that work links to via `contributes_to`, never via
    /// the parent hierarchy. Returns a caller-facing error message on rejection.
    ///
    /// If `parent_id` does not resolve, returns `Ok(())` — the caller's own
    /// existence check is responsible for reporting a missing parent. The set
    /// `{target, goal}` is keyed on `node_type`, not any ID prefix (`targ-` is a
    /// project slug, not a type marker).
    pub fn reject_target_as_parent(&self, parent_id: &str) -> Result<(), String> {
        if let Some(node) = self.resolve(parent_id) {
            if crate::graph::is_strategic_target(node.node_type.as_deref()) {
                let ty = node.node_type.as_deref().unwrap_or("target");
                return Err(format!(
                    "Parent '{}' is a `{}` node. Targets/goals are strategic priorities, not \
                     structural parents — they are excluded from the work tree. Link this work to \
                     the target via `contributes_to: [{{to: {}, stated_weight: Expected}}]` \
                     instead of setting it as `parent`.",
                    parent_id, ty, node.id
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Parent-chain helpers
    // -----------------------------------------------------------------------

    /// Walk the parent chain starting at `start` looking for `target`. If the
    /// target is reached (including when `start == target`), returns the chain
    /// from `start` to (and including) `target` as canonical IDs. Returns
    /// `None` if the chain terminates at a root, hits an unknown node, or
    /// loops back on itself without ever touching `target`.
    ///
    /// Used to reject parent-chain cycles before they're written. Bounded at
    /// 256 hops as a defensive limit against malformed graphs.
    pub fn parent_chain_to(&self, start: &str, target: &str) -> Option<Vec<String>> {
        let target_canonical = self
            .resolve(target)
            .map(|n| n.id.clone())
            .unwrap_or_else(|| target.to_string());

        let mut current = self.resolve(start).map(|n| n.id.clone())?;
        let mut chain: Vec<String> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();

        for _ in 0..256 {
            if !visited.insert(current.clone()) {
                return None;
            }
            chain.push(current.clone());
            if current == target_canonical {
                return Some(chain);
            }
            let parent_id = self
                .nodes
                .get(&current)
                .and_then(|n| n.parent.as_deref())?
                .to_string();
            current = self
                .resolve(&parent_id)
                .map(|n| n.id.clone())
                .unwrap_or(parent_id);
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

    /// Find orphan nodes (nodes disconnected from the graph).
    ///
    /// Orphan-ness is **type-aware**, reflecting the two structural models the
    /// PKB uses (SSoT: `specs/pkb-server-spec.md`, "Orphan Detection"):
    ///
    /// - **Actionable types** (`ACTIONABLE_TYPES`: task/learn/pr)
    ///   live in a *tree* whose backbone is the `parent` field. One of these is
    ///   an orphan iff its `parent` is absent or dangling — having dependency or
    ///   link edges does NOT excuse a missing hierarchy parent. This preserves
    ///   task-hygiene detection and keeps `pkb_orphans`'s default in step with
    ///   `graph_stats.orphan_count`.
    /// - **All other types** — knowledge nodes (memory/note), out-of-tree
    ///   strategic nodes (goal/target), and untyped documents — live in a *web*
    ///   of associations. One of these is an orphan only if it has no valid
    ///   parent AND no deliberate structural edge (any edge except the
    ///   auto-computed `SimilarTo` similarity edges) connecting it to another
    ///   node, in either direction: an inbound edge rescues a node just as an
    ///   outbound one does. A memory carrying a `[[wikilink]]` to a related
    ///   concept is thus correctly *not* an orphan, and neither is a parentless
    ///   goal that work `contributes_to`.
    pub fn orphans(&self) -> Vec<&GraphNode> {
        // IDs touched (as source or target) by a deliberate structural edge.
        // `SimilarTo` is excluded: semantic similarity is auto-derived, not an
        // intentional link, so it must not silently un-orphan a node.
        let mut linked = HashSet::with_capacity(self.edges.len());
        for e in &self.edges {
            if e.edge_type != EdgeType::SimilarTo {
                linked.insert(e.source.as_str());
                linked.insert(e.target.as_str());
            }
        }

        self.nodes
            .values()
            .filter(|n| {
                let has_valid_parent = match &n.parent {
                    None => false,
                    Some(pid) => self.nodes.contains_key(pid.as_str()),
                };
                if has_valid_parent {
                    return false;
                }
                // No valid parent. Actionable nodes are orphans regardless of
                // other edges; knowledge nodes are rescued by any structural edge.
                let is_actionable = n
                    .node_type
                    .as_deref()
                    .map(|t| ACTIONABLE_TYPES.contains(&t))
                    .unwrap_or(false);
                if is_actionable {
                    true
                } else {
                    !linked.contains(n.id.as_str())
                }
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

        components.sort_by_key(|b| std::cmp::Reverse(b.len()));
        components
    }

    /// Detect edges with high stated weight/strength but zero interaction
    /// on the source task in N days (pkb-flow-engine §7.2).
    pub fn detect_weight_divergences(&self, threshold_days: i64) -> Vec<WeightDivergence> {
        let now = Utc::now();
        let mut divergences = Vec::new();
        let blocked_ids: HashSet<&str> = self.blocked.iter().map(|s| s.as_str()).collect();

        let (flow_edges, _) = self.resolve_flow_edges_with_warnings();
        for edge in flow_edges {
            let node = match self.nodes.get(&edge.src) {
                Some(n) => n,
                None => continue,
            };
            if blocked_ids.contains(node.id.as_str()) {
                continue;
            }
            if graph::is_completed(node.status.as_deref()) {
                continue;
            }
            let days_since_interaction = days_since_modified(node.modified.as_deref(), now);
            if days_since_interaction >= threshold_days
                && edge.strength() >= DIVERGENCE_WEIGHT_THRESHOLD
            {
                let target_id = edge.dst.clone();
                let target_label = self
                    .nodes
                    .get(&target_id)
                    .map(|n| n.label.clone())
                    .unwrap_or_else(|| target_id.clone());

                divergences.push(WeightDivergence {
                    source_id: node.id.clone(),
                    source_label: node.label.clone(),
                    target_id,
                    target_label,
                    stated_weight: format!("{:.2}", edge.strength()),
                    numeric_weight: edge.strength(),
                    justification: edge.justification.clone().unwrap_or_default(),
                    days_since_interaction,
                    label: Some(edge.label.as_str().to_string()),
                    quantum: Some(edge.quantum),
                    probability: Some(edge.probability),
                    strength: Some(edge.strength()),
                });
            }
        }
        divergences
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

    /// Check if adding a hard dependency edge (DependsOn or Parent) from `from` to `to`
    /// would create a cycle using Tarjan's SCC.
    ///
    /// Hard dependencies (DependsOn + Parent) must form a DAG. Cycles in this subgraph
    /// represent decomposition errors and are rejected at write time.
    pub fn would_create_hard_cycle(&self, from: &str, to: &str) -> Result<(), String> {
        self.would_create_hard_cycle_multi(from, &[to])
    }

    /// Check if adding multiple hard dependency edges from `from` to `targets`
    /// would create a cycle using Tarjan's SCC.
    pub fn would_create_hard_cycle_multi(
        &self,
        from: &str,
        targets: &[&str],
    ) -> Result<(), String> {
        let edges: Vec<(&str, &str)> = targets.iter().map(|&t| (from, t)).collect();
        self.would_create_hard_cycle_edges(&edges)
    }

    /// Check if adding a batch of hard dependency edges (DependsOn or Parent)
    /// would create a cycle using Tarjan's SCC.
    pub fn would_create_hard_cycle_edges(
        &self,
        extra_edges: &[(&str, &str)],
    ) -> Result<(), String> {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if matches!(edge.edge_type, EdgeType::DependsOn | EdgeType::Parent) {
                adjacency
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());
            }
        }

        let mut touched_nodes: HashSet<String> = HashSet::new();

        for &(from, to) in extra_edges {
            let from_id = self
                .resolve(from)
                .map(|n| n.id.clone())
                .unwrap_or_else(|| from.to_string());
            let to_id = self
                .resolve(to)
                .map(|n| n.id.clone())
                .unwrap_or_else(|| to.to_string());

            if from_id == to_id {
                return Err(format!(
                    "A task cannot be its own parent or depend on itself (id '{from_id}')."
                ));
            }

            touched_nodes.insert(from_id.clone());
            touched_nodes.insert(to_id.clone());

            adjacency.entry(from_id).or_default().push(to_id);
        }

        let sccs = tarjan_scc(&adjacency);
        for scc in sccs {
            if scc.len() > 1 && scc.iter().any(|id| touched_nodes.contains(id)) {
                return Err(format!(
                    "Adding hard dependencies would create a circular dependency cycle: [{}]. Hard dependencies (DependsOn + Parent) must be a DAG.",
                    scc.join(" -> ")
                ));
            }
        }

        Ok(())
    }

    /// Validate that assigning `proposed_parent` as the parent of `child` would not
    /// create a circular parent/child or hard dependency relationship.
    ///
    /// Evaluates the hard dependency graph (Parent + DependsOn) via Tarjan's SCC.
    /// Self-parenting (`child == proposed_parent`) is rejected immediately.
    ///
    /// Both IDs are resolved through `GraphStore::resolve` so callers may pass any
    /// alias the resolver accepts (canonical ID, filename stem, permalink, etc.).
    pub fn would_create_parent_cycle(
        &self,
        child: &str,
        proposed_parent: &str,
    ) -> Result<(), String> {
        let child_id = self
            .resolve(child)
            .map(|n| n.id.clone())
            .unwrap_or_else(|| child.to_string());
        let parent_id = self
            .resolve(proposed_parent)
            .map(|n| n.id.clone())
            .unwrap_or_else(|| proposed_parent.to_string());

        if child_id == parent_id {
            return Err(format!(
                "A task cannot be its own parent (id '{child_id}')."
            ));
        }

        self.would_create_hard_cycle(&child_id, &parent_id).map_err(|e| {
            format!(
                "Setting parent of '{child_id}' to '{parent_id}' would create a circular \
                 parent/child relationship ({e}). Parent/child and hard dependency hierarchy must be a DAG."
            )
        })
    }

    /// Find all parent-only cycles in the graph.
    ///
    /// Returns a list of strongly connected components (size > 1) over the
    /// `Parent` edge subgraph. Each inner vector contains the node IDs that
    /// participate in one parent cycle.
    pub fn find_parent_cycles(&self) -> Vec<Vec<String>> {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if matches!(edge.edge_type, EdgeType::Parent) {
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

        // Excalidraw
        let excalidraw_path = format!("{base}.excalidraw");
        std::fs::write(&excalidraw_path, self.output_excalidraw(None, 1)?.0)?;
        written.push(excalidraw_path);

        Ok(written)
    }

    /// Full graph as JSON — all task nodes and their edges, with optional filters.
    pub fn output_json_filtered(
        &self,
        focus: Option<&str>,
        max_depth: usize,
        project: Option<&str>,
        include_done: bool,
    ) -> Result<String> {
        let (mut nodes, mut edges): (Vec<GraphNode>, Vec<Edge>) = match focus {
            Some(query) => match self.resolve(query).map(|n| n.id.clone()) {
                Some(id) => crate::excalidraw::extract_ego_subgraph(self, &id, max_depth.max(1)),
                None => (Vec::new(), Vec::new()),
            },
            None => (self.nodes.values().cloned().collect(), self.edges.clone()),
        };

        // Only include nodes with explicit task_id (real tasks, not bare notes)
        nodes.retain(|n| n.task_id.is_some());

        if !include_done {
            nodes.retain(|n| {
                !crate::graph::COMPLETED_STATUSES.contains(&n.status.as_deref().unwrap_or(""))
            });
        }
        if let Some(proj) = project {
            nodes.retain(|n| n.project.as_deref() == Some(proj));
        }
        nodes.sort_by(|a, b| a.label.cmp(&b.label));

        let placed_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        edges.retain(|e| {
            placed_ids.contains(e.source.as_str()) && placed_ids.contains(e.target.as_str())
        });

        let ready: Vec<String> = self
            .ready
            .iter()
            .filter(|id| placed_ids.contains(id.as_str()))
            .cloned()
            .collect();
        let blocked: Vec<String> = self
            .blocked
            .iter()
            .filter(|id| placed_ids.contains(id.as_str()))
            .cloned()
            .collect();
        let roots: Vec<String> = self
            .roots
            .iter()
            .filter(|id| placed_ids.contains(id.as_str()))
            .cloned()
            .collect();
        let today = chrono::Utc::now().date_naive();
        let buffer_days = crate::display_rank::DISPLAY_CLIFF_BUFFER_DAYS;

        let cliff_list: Vec<String> = nodes
            .iter()
            .filter(|n| {
                let due_date = n.due.as_deref().and_then(crate::display_rank::parse_due_date);
                let eff = crate::display_rank::resolve_effort_days(n.effort.as_deref());
                let resolved_class = crate::display_rank::DeadlineClass::resolve(
                    n.deadline_class.map(|dc| match dc {
                        crate::graph::DeadlineClass::Fake => crate::display_rank::DeadlineClass::Fake,
                        crate::graph::DeadlineClass::Soft => crate::display_rank::DeadlineClass::Soft,
                        crate::graph::DeadlineClass::Hard => crate::display_rank::DeadlineClass::Hard,
                    }),
                    due_date.is_some(),
                );
                crate::display_rank::is_on_cliff(due_date, resolved_class, eff, today, buffer_days)
            })
            .map(|n| n.id.clone())
            .filter(|id| placed_ids.contains(id.as_str()))
            .collect();

        let mut display_ranked: Vec<&GraphNode> = nodes.iter().collect();
        Self::sort_by_display(&mut display_ranked, today, buffer_days);
        let display_ranks: HashMap<String, usize> = display_ranked
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i + 1))
            .collect();

        let nodes: Vec<ExportNode> = nodes
            .into_iter()
            .map(|node| {
                let display_rank = display_ranks.get(&node.id).copied();
                let is_ready = ready.contains(&node.id);
                let is_blocked = blocked.contains(&node.id);
                let eff = crate::display_rank::resolve_effort_days(node.effort.as_deref());
                let dc = node.deadline_class.map(|d| match d {
                    crate::graph::DeadlineClass::Fake => crate::display_rank::DeadlineClass::Fake,
                    crate::graph::DeadlineClass::Soft => crate::display_rank::DeadlineClass::Soft,
                    crate::graph::DeadlineClass::Hard => crate::display_rank::DeadlineClass::Hard,
                });
                let due_date = node.due.as_deref().and_then(crate::display_rank::parse_due_date);
                let display = Some(crate::display_rank::TaskDisplay::new(
                    is_ready,
                    is_blocked,
                    due_date,
                    dc,
                    eff,
                    today,
                    buffer_days,
                ));
                ExportNode {
                    node,
                    display,
                    display_rank,
                }
            })
            .collect();

        let graph = ExportGraph {
            nodes,
            edges,
            ready,
            blocked,
            roots,
            cliff: cliff_list,
        };
        Ok(serde_json::to_string_pretty(&graph)?)
    }

    /// Full graph as JSON — all task nodes and their edges.
    pub fn output_json(&self) -> Result<String> {
        self.output_json_filtered(None, 2, None, true)
    }

    /// Export graph or ego network to Excalidraw JSON canvas format.
    /// If `focus_id` is provided, exports an ego-network with given `hops`.
    /// Otherwise exports the union of the top-10 focus picks' ego neighborhoods.
    pub fn output_excalidraw(
        &self,
        focus_id: Option<&str>,
        hops: usize,
    ) -> Result<(String, usize, usize)> {
        let (file, node_count, edge_count) = if let Some(focus) = focus_id {
            let resolved_id = self.resolve(focus).map(|n| n.id.as_str()).unwrap_or(focus);
            let file = crate::excalidraw::export_ego_network(self, resolved_id, hops, None)?;
            let (nodes, edges) = crate::excalidraw::extract_ego_subgraph(self, resolved_id, hops);
            (file, nodes.len(), edges.len())
        } else {
            // Default to highest focus tasks tree instead of entire graph
            let focus_roots = self.focus_picks(10);

            let mut node_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            for root in &focus_roots {
                let (sub_nodes, _) = crate::excalidraw::extract_ego_subgraph(self, root, hops);
                for n in sub_nodes {
                    node_ids.insert(n.id);
                }
            }
            let mut all_ids: Vec<String> = node_ids.into_iter().collect();
            all_ids.sort();
            let file = crate::excalidraw::export_subgraph(self, &all_ids, None)?;

            let mut edge_count = 0;
            let node_set: std::collections::HashSet<_> = all_ids.iter().cloned().collect();
            for edge in self.edges() {
                if node_set.contains(&edge.source) && node_set.contains(&edge.target) {
                    edge_count += 1;
                }
            }
            (file, all_ids.len(), edge_count)
        };
        Ok((serde_json::to_string_pretty(&file)?, node_count, edge_count))
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
            cliff: vec![],
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
  <key id="d5" for="node" attr.name="intent" attr.type="int"/>
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
            if let Some(p) = node.intent {
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

    /// Export the graph (or a focused subgraph) as GraphViz DOT syntax.
    ///
    /// With `focus` set, exports the `max_depth`-hop ego-network around that
    /// node (same traversal as [`GraphStore::output_excalidraw`] — structural
    /// edges only: parent, depends_on, soft_depends_on, contributes_to,
    /// supersedes, closes). Without `focus`, exports every node. `project`
    /// and `include_done` filter the resulting node set further; edges are
    /// then restricted to those whose endpoints both survived filtering.
    ///
    /// `blocks:` frontmatter is compiled into `DependsOn` edges at build time
    /// (blocked node -> blocker), so there is no separate `blocks` edge type
    /// to render — `depends_on` already carries that relationship.
    pub fn output_dot(
        &self,
        focus: Option<&str>,
        max_depth: usize,
        project: Option<&str>,
        include_done: bool,
    ) -> String {
        let (mut nodes, mut edges): (Vec<GraphNode>, Vec<Edge>) = match focus {
            Some(query) => match self.resolve(query).map(|n| n.id.clone()) {
                Some(id) => crate::excalidraw::extract_ego_subgraph(self, &id, max_depth.max(1)),
                None => (Vec::new(), Vec::new()),
            },
            None => (self.nodes.values().cloned().collect(), self.edges.clone()),
        };

        if !include_done {
            nodes.retain(|n| {
                !graph::COMPLETED_STATUSES.contains(&n.status.as_deref().unwrap_or(""))
            });
        }
        if let Some(proj) = project {
            nodes.retain(|n| n.project.as_deref() == Some(proj));
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let kept_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        edges.retain(|e| {
            kept_ids.contains(e.source.as_str()) && kept_ids.contains(e.target.as_str())
        });

        let mut dot = String::from("digraph PKB {\n  rankdir=LR;\n");
        for node in &nodes {
            let label = dot_escape(&format!(
                "{}\n{}",
                node.label,
                node.status.as_deref().unwrap_or("")
            ));
            dot.push_str(&format!(
                "  \"{}\" [label=\"{}\", type=\"{}\", status=\"{}\"];\n",
                dot_escape(&node.id),
                label,
                dot_escape(node.node_type.as_deref().unwrap_or("")),
                dot_escape(node.status.as_deref().unwrap_or("")),
            ));
        }
        for edge in &edges {
            dot.push_str(&format!(
                "  \"{}\" -> \"{}\" [type=\"{}\"];\n",
                dot_escape(&edge.source),
                dot_escape(&edge.target),
                edge.edge_type.as_str(),
            ));
        }
        dot.push_str("}\n");
        dot
    }
}

/// Escape a string for safe use inside a double-quoted GraphViz DOT label or
/// attribute value: backslashes and quotes are escaped, real newlines become
/// the DOT literal `\n` line-break escape. Markdown syntax (`[[..]]`, `**..**`)
/// is not special in DOT and passes through unchanged.
fn dot_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Recency signal (0.0 - 1.0) based on modification date.
///
/// 1.0 if modified today, decaying exponentially with exp(-days / 30).
/// Clamps to 0.0 at or after 90 days.
#[allow(dead_code)]
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

/// Resolve a `contributes_to` target reference to a canonical node ID.
///
/// Centralises the resolution call used in both `build_node_edges` (to emit
/// `ContributesTo` edges) and `compute_inverses` (to populate
/// `ContributesTo::resolved_to`), so both stay in sync when resolution logic
/// changes.
fn resolve_ct_ref(
    to: &str,
    id_map: &HashMap<String, String>,
    path_to_id: &HashMap<String, String>,
) -> Option<String> {
    graph::resolve_ref(to, id_map, path_to_id)
}

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

    // contributes_to -> ContributesTo edge
    for ct in &n.contributes_to {
        if let Some(target_id) = resolve_ct_ref(&ct.to, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::ContributesTo,
                });
            }
        }
    }

    // closes -> Closes edge
    for close_ref in &n.closes {
        if let Some(target_id) = graph::resolve_ref(close_ref, id_map, path_to_id) {
            if n.id != target_id {
                edges.push(Edge {
                    source: n.id.clone(),
                    target: target_id,
                    edge_type: EdgeType::Closes,
                });
            }
        }
    }

    // supersedes -> Supersedes edge (this -> old memory), one per named target
    for old_id_ref in &n.supersedes {
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

/// Helper to resolve flow edges from a collection of nodes, detecting duplicates and disagreement (pkb-flow-engine §3.1 R4, E4).
pub fn resolve_flow_edges_from_nodes<'a, I>(
    nodes: I,
) -> (Vec<FlowEdge>, Vec<(String, ParseWarning)>)
where
    I: IntoIterator<Item = &'a GraphNode>,
{
    struct Decl<'a> {
        is_source: bool,
        link: &'a crate::graph::Link,
    }

    let mut decls_by_edge: HashMap<(String, String, crate::graph::LinkLabel), Vec<Decl<'a>>> =
        HashMap::new();

    for node in nodes {
        for link in &node.links {
            if let Some(ref target) = link.to {
                decls_by_edge
                    .entry((node.id.clone(), target.clone(), link.label))
                    .or_default()
                    .push(Decl {
                        is_source: true,
                        link,
                    });
            } else if let Some(ref source) = link.from {
                decls_by_edge
                    .entry((source.clone(), node.id.clone(), link.label))
                    .or_default()
                    .push(Decl {
                        is_source: false,
                        link,
                    });
            }
        }
    }

    let mut edges = Vec::with_capacity(decls_by_edge.len());
    let mut warnings = Vec::new();

    for ((src, dst, label), decls) in decls_by_edge {
        if decls.len() == 1 {
            let d = &decls[0];
            edges.push(FlowEdge {
                src,
                dst,
                label,
                quantum: d.link.quantum,
                probability: d.link.probability,
                effect: d.link.effect,
                justification: d.link.justification.clone(),
                set_by: d.link.set_by,
            });
        } else {
            let src_decl = decls.iter().find(|d| d.is_source);
            let dst_decl = decls.iter().find(|d| !d.is_source);

            match (src_decl, dst_decl) {
                (Some(s), Some(d)) => {
                    let agree = (s.link.quantum - d.link.quantum).abs() < 1e-9
                        && (s.link.probability - d.link.probability).abs() < 1e-9
                        && s.link.effect == d.link.effect
                        && s.link.justification == d.link.justification
                        && s.link.set_by == d.link.set_by;

                    if agree {
                        edges.push(FlowEdge {
                            src,
                            dst,
                            label,
                            quantum: s.link.quantum,
                            probability: s.link.probability,
                            effect: s.link.effect,
                            justification: s.link.justification.clone(),
                            set_by: s.link.set_by,
                        });
                    } else {
                        // Disagreement: from-node (src) wins; warning emitted for dst-node (dst)
                        edges.push(FlowEdge {
                            src: src.clone(),
                            dst: dst.clone(),
                            label,
                            quantum: s.link.quantum,
                            probability: s.link.probability,
                            effect: s.link.effect,
                            justification: s.link.justification.clone(),
                            set_by: s.link.set_by,
                        });
                        warnings.push((
                            dst.clone(),
                            ParseWarning {
                                field: "links".to_string(),
                                message: format!(
                                    "duplicate link declaration disagreement between {} and {} for label {:?}; entry on from-node {} wins",
                                    src, dst, label, src
                                ),
                            },
                        ));
                    }
                }
                (Some(s), None) => {
                    edges.push(FlowEdge {
                        src,
                        dst,
                        label,
                        quantum: s.link.quantum,
                        probability: s.link.probability,
                        effect: s.link.effect,
                        justification: s.link.justification.clone(),
                        set_by: s.link.set_by,
                    });
                }
                (None, Some(d)) => {
                    edges.push(FlowEdge {
                        src,
                        dst,
                        label,
                        quantum: d.link.quantum,
                        probability: d.link.probability,
                        effect: d.link.effect,
                        justification: d.link.justification.clone(),
                        set_by: d.link.set_by,
                    });
                }
                (None, None) => unreachable!(),
            }
        }
    }

    edges.sort_by(|a, b| {
        (&a.src, &a.dst, a.label.as_str()).cmp(&(&b.src, &b.dst, b.label.as_str()))
    });
    warnings
        .sort_by(|a, b| (&a.0, &a.1.field, &a.1.message).cmp(&(&b.0, &b.1.field, &b.1.message)));

    (edges, warnings)
}

/// Build a `FlowInput` from parsed `GraphNode`s and resolved flow edges (pkb-flow-engine §3.1, R1).
pub fn build_flow_input(nodes: &[GraphNode]) -> crate::flow::FlowInput {
    let mut flow_input = crate::flow::FlowInput::new();
    for node in nodes {
        let state = crate::flow::status_to_flow_state(node.status.as_deref());
        flow_input.add_node(node.id.clone(), state, node.worth);
    }
    let (resolved_edges, _) = resolve_flow_edges_from_nodes(nodes);
    for e in resolved_edges {
        flow_input.add_edge(crate::flow::FlowEdge {
            src: e.src,
            dst: e.dst,
            label: e.label.as_str().to_string(),
            quantum: e.quantum,
            probability: e.probability,
            effect: match e.effect {
                crate::graph::LinkEffect::Helps => crate::flow::FlowEffect::Helps,
                crate::graph::LinkEffect::Harms => crate::flow::FlowEffect::Harms,
            },
            unvalued: false,
        });
    }
    flow_input
}

/// Compute inverse relationships on nodes from resolved edges.
///
/// For each DependsOn edge (source depends on target):
///   target.blocks += source
/// For each SoftDependsOn edge:
///   target.soft_blocks += source
/// For each Parent edge (source is child of target):
///   target.children += source
/// For each ContributesTo edge (source contributes to target):
///   target.contributed_by += source
fn compute_inverses(
    nodes: &mut [GraphNode],
    edges: &[Edge],
    id_map: &HashMap<String, String>,
    path_to_id: &HashMap<String, String>,
) {
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

    // Resolve contributes_to.resolved_to on each node, and canonicalize frontmatter references to actual node IDs
    for node in nodes.iter_mut() {
        for ct in node.contributes_to.iter_mut() {
            ct.resolved_to = resolve_ct_ref(&ct.to, id_map, path_to_id);
        }
        for dep in node.depends_on.iter_mut() {
            if let Some(target_id) = graph::resolve_ref(dep, id_map, path_to_id) {
                *dep = target_id;
            }
        }
        for sdep in node.soft_depends_on.iter_mut() {
            if let Some(target_id) = graph::resolve_ref(sdep, id_map, path_to_id) {
                *sdep = target_id;
            }
        }
        for c in node.closes.iter_mut() {
            if let Some(target_id) = graph::resolve_ref(c, id_map, path_to_id) {
                *c = target_id;
            }
        }
        for link in node.links.iter_mut() {
            if let Some(ref mut to_id) = link.to {
                if let Some(target_id) = graph::resolve_ref(to_id, id_map, path_to_id) {
                    *to_id = target_id;
                }
            }
            if let Some(ref mut from_id) = link.from {
                if let Some(target_id) = graph::resolve_ref(from_id, id_map, path_to_id) {
                    *from_id = target_id;
                }
            }
        }
    }

    // Collect updates to avoid borrow issues
    let mut block_updates: Vec<(usize, String)> = Vec::new(); // (target_idx, source_id)
    let mut soft_block_updates: Vec<(usize, String)> = Vec::new();
    let mut dep_updates: Vec<(usize, String)> = Vec::new();
    let mut soft_dep_updates: Vec<(usize, String)> = Vec::new();
    let mut children_updates: Vec<(usize, String)> = Vec::new();
    let mut subtask_updates: Vec<(usize, String)> = Vec::new();
    let mut contributed_by_updates: Vec<(usize, String)> = Vec::new();
    // (target_idx, superseder_id) — mem_8035b002: superseded_by is computed
    // from Supersedes edges, the same way contributed_by derives from
    // ContributesTo, so a hand-written `superseded_by:` can never dangle or
    // desync from the `supersedes:` edge that is its only source of truth.
    let mut superseded_by_updates: Vec<(usize, String)> = Vec::new();
    // Resolve parent field: raw frontmatter value → actual node ID
    let mut parent_updates: Vec<(usize, String)> = Vec::new(); // (child_idx, resolved_parent_id)

    for edge in edges {
        match edge.edge_type {
            EdgeType::DependsOn => {
                // source depends on target -> target blocks source
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    block_updates.push((idx, edge.source.clone()));
                }
                if let Some(&idx) = id_to_idx.get(&edge.source) {
                    dep_updates.push((idx, edge.target.clone()));
                }
            }
            EdgeType::SoftDependsOn => {
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    soft_block_updates.push((idx, edge.source.clone()));
                }
                if let Some(&idx) = id_to_idx.get(&edge.source) {
                    soft_dep_updates.push((idx, edge.target.clone()));
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
            EdgeType::ContributesTo => {
                // source contributes to target -> target is contributed to by source
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    contributed_by_updates.push((idx, edge.source.clone()));
                }
            }
            EdgeType::Supersedes => {
                // source supersedes target -> target is superseded by source
                if let Some(&idx) = id_to_idx.get(&edge.target) {
                    superseded_by_updates.push((idx, edge.source.clone()));
                }
            }
            EdgeType::Link | EdgeType::SimilarTo | EdgeType::Closes => {}
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
    for (idx, dep_id) in dep_updates {
        if !nodes[idx].depends_on.contains(&dep_id) {
            nodes[idx].depends_on.push(dep_id);
        }
    }
    for (idx, sdep_id) in soft_dep_updates {
        if !nodes[idx].soft_depends_on.contains(&sdep_id) {
            nodes[idx].soft_depends_on.push(sdep_id);
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
    for (idx, contributor_id) in contributed_by_updates {
        if !nodes[idx].contributed_by.contains(&contributor_id) {
            nodes[idx].contributed_by.push(contributor_id);
        }
    }
    for (idx, superseder_id) in superseded_by_updates {
        if !nodes[idx].superseded_by.contains(&superseder_id) {
            nodes[idx].superseded_by.push(superseder_id);
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
        deduplicate_vec(&mut node.contributed_by);
        deduplicate_vec(&mut node.depends_on);
        deduplicate_vec(&mut node.soft_depends_on);
        node.leaf = node.children.is_empty();
    }
}

/// Compute semantic similarity edges between nodes based on vector proximity.
///
/// Skips pairs that already share an explicit edge (in either direction).
fn compute_similarity_edges(
    nodes: &[GraphNode],
    existing_edges: &[Edge],
    embeddings_by_path: &HashMap<String, Vec<f32>>,
    threshold: f64,
) -> Vec<Edge> {
    let mut similarity_edges = Vec::new();
    let n = nodes.len();
    if n < 2 {
        return vec![];
    }

    // Pre-build adjacency set for O(1) existence check
    let mut explicit_adj: HashSet<(String, String)> =
        HashSet::with_capacity(existing_edges.len() * 2);
    for e in existing_edges {
        explicit_adj.insert((e.source.clone(), e.target.clone()));
        explicit_adj.insert((e.target.clone(), e.source.clone()));
    }

    // Pre-fetch all embeddings via the snapshot (no store lock held).
    let embeddings: Vec<Option<&Vec<f32>>> = nodes
        .iter()
        .map(|node| {
            let path = node.path.to_string_lossy();
            embeddings_by_path.get(path.as_ref()).or_else(|| {
                let stripped = path.strip_prefix("tasks/").unwrap_or(&path);
                embeddings_by_path.get(stripped)
            })
        })
        .collect();

    // Pairwise comparison (parallel)
    // We use rayon to parallelize the outer loop for performance on 1000+ nodes
    use rayon::prelude::*;

    let results: Vec<Vec<Edge>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut local_edges = Vec::new();
            let node_a = &nodes[i];
            let emb_a = match &embeddings[i] {
                Some(e) => e,
                None => return local_edges,
            };

            for j in (i + 1)..n {
                let node_b = &nodes[j];
                let emb_b = match &embeddings[j] {
                    Some(e) => e,
                    None => continue,
                };

                // Skip if explicit edge exists
                if explicit_adj.contains(&(node_a.id.clone(), node_b.id.clone())) {
                    continue;
                }

                let score = crate::distance::cosine_similarity(emb_a, emb_b) as f64;
                if score >= threshold {
                    local_edges.push(Edge {
                        source: node_a.id.clone(),
                        target: node_b.id.clone(),
                        edge_type: EdgeType::SimilarTo,
                    });
                }
            }
            local_edges
        })
        .collect();

    for batch in results {
        similarity_edges.extend(batch);
    }

    similarity_edges
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

/// Compute the `project` field for each node by walking up the `parent` chain
/// to the nearest ancestor that declared an explicit `project: <slug>` in its
/// own frontmatter, and inheriting that value.
///
/// `project` is an operational routing slug validated against polecat.yaml at
/// write time — not a node type. A node's own explicit value always wins (the
/// skip below preserves it); descendants without one inherit the nearest
/// ancestor's. Only `parent` edges participate: `contributes_to`/`closes`
/// point at out-of-tree goals/targets, which carry no routing slug.
fn compute_project_field(nodes: &mut [GraphNode], edges: &[Edge]) {
    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // parent adjacency: child -> parent (a strict tree, but tolerate multiple
    // recorded parent edges defensively by taking the first that resolves)
    let mut parent_of: Vec<Option<usize>> = vec![None; nodes.len()];
    for edge in edges {
        if matches!(edge.edge_type, EdgeType::Parent) {
            if let (Some(&s_idx), Some(&t_idx)) =
                (id_to_idx.get(&edge.source), id_to_idx.get(&edge.target))
            {
                if parent_of[s_idx].is_none() {
                    parent_of[s_idx] = Some(t_idx);
                }
            }
        }
    }

    let mut project_labels: Vec<Option<String>> = vec![None; nodes.len()];

    for i in 0..nodes.len() {
        // Node's own explicit (already-validated) value wins untouched.
        if nodes[i].project.is_some() {
            continue;
        }

        // Walk up the parent chain to the nearest explicit project value.
        let mut current = parent_of[i];
        let mut depth = 0;
        while let Some(idx) = current {
            depth += 1;
            if depth > 100 {
                break; // cycle guard
            }
            if let Some(ref proj) = nodes[idx].project {
                project_labels[i] = Some(proj.clone());
                break;
            }
            current = parent_of[idx];
        }
    }

    // Apply the resolved computed projects
    for i in 0..nodes.len() {
        if let Some(proj) = project_labels[i].take() {
            nodes[i].project = Some(proj);
        }
        if nodes[i].project.is_some() {
            nodes[i].parse_warnings.retain(|w| w.field != "project");
        }
    }
}

/// Compute eager-materialised ancestors for each node.
///
/// Follows upward edges: `parent`, `contributes_to`, and `closes`.
/// Results are stored in `node.target_ancestors`.
fn compute_target_ancestors(nodes: &mut [GraphNode], edges: &[Edge]) {
    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Upward adjacency list: source -> targets for upward edge types
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for edge in edges {
        if matches!(
            edge.edge_type,
            EdgeType::Parent | EdgeType::ContributesTo | EdgeType::Closes
        ) {
            if let (Some(&s_idx), Some(&t_idx)) =
                (id_to_idx.get(&edge.source), id_to_idx.get(&edge.target))
            {
                adj[s_idx].push(t_idx);
            }
        }
    }

    // Compute ancestors for each node using BFS
    for i in 0..nodes.len() {
        let mut ancestors = HashSet::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        visited.insert(i);
        for &neighbor_idx in &adj[i] {
            queue.push_back(neighbor_idx);
        }

        while let Some(curr_idx) = queue.pop_front() {
            if !visited.insert(curr_idx) {
                continue;
            }
            ancestors.insert(nodes[curr_idx].id.clone());
            for &neighbor_idx in &adj[curr_idx] {
                queue.push_back(neighbor_idx);
            }
        }

        let mut sorted: Vec<String> = ancestors.into_iter().collect();
        sorted.sort();
        nodes[i].target_ancestors = sorted;
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
/// In Phase 3, uncertainty measures epistemic variance (honest uncertainty),
/// NOT documentation hygiene or metadata completeness.
///
/// Inputs:
/// 1. Elicited confidence (`node.confidence`): primary numeric input.
///    uncertainty = clamp(1.0 - confidence, 0.0, 1.0)
/// 2. Open question (`node.has_open_question`): defaults to 0.50 (fifty-fifty) if confidence is unset.
/// 3. Otherwise: 0.0 (certain).
///
/// Missing acceptance criteria, body length, and child count feed NO score path.
fn compute_uncertainty(nodes: &mut [GraphNode]) {
    for node in nodes.iter_mut() {
        if let Some(conf) = node.confidence {
            node.uncertainty = (1.0 - conf).clamp(0.0, 1.0);
        } else if node.has_open_question {
            node.uncertainty = 0.50;
        } else {
            node.uncertainty = 0.0;
        }
    }
}

fn days_since_modified(modified: Option<&str>, now: DateTime<Utc>) -> i64 {
    modified
        .and_then(|m| DateTime::parse_from_rfc3339(m).ok())
        .map(|dt| (now - dt.with_timezone(&Utc)).num_days())
        .unwrap_or(0)
}

/// Compute stated-revealed weight divergence anomalies on nodes.
///
/// Flags `contributes_to` edges with high stated weight (>= 0.75) where
/// the source task has been inactive for `threshold_days`.
fn compute_divergence_anomalies(
    nodes: &mut HashMap<String, GraphNode>,
    effectively_blocked: &[String],
    threshold_days: i64,
) {
    let now = Utc::now();
    let blocked_set: HashSet<&str> = effectively_blocked.iter().map(|s| s.as_str()).collect();

    for node in nodes.values_mut() {
        // Skip blocked tasks (false-positive guard)
        if blocked_set.contains(node.id.as_str()) {
            continue;
        }
        // Skip completed tasks
        if graph::is_completed(node.status.as_deref()) {
            continue;
        }

        let days_since_interaction = days_since_modified(node.modified.as_deref(), now);

        if days_since_interaction >= threshold_days {
            for ct in &mut node.contributes_to {
                if ct.numeric_weight() >= DIVERGENCE_WEIGHT_THRESHOLD {
                    ct.anomaly_flag = true;
                }
            }
        }
    }
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

    let mut unfinished_ids: HashSet<&str> = HashSet::with_capacity(nodes.len());
    for n in nodes {
        if !graph::is_completed(n.status.as_deref()) {
            unfinished_ids.insert(n.id.as_str());
        }
    }

    // Build children mapping
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::with_capacity(nodes.len());
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
        let is_actionable = match node.node_type.as_deref() {
            Some(t) => ACTIONABLE_TYPES.contains(&t),
            None => node.task_id.is_some(),
        };
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

    // Build upstream adjacency from edges (parent, depends_on, soft_depends_on, contributes_to)
    let mut upstream_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        match edge.edge_type {
            EdgeType::Parent
            | EdgeType::DependsOn
            | EdgeType::SoftDependsOn
            | EdgeType::ContributesTo
                if all_ids.contains(edge.target.as_str()) =>
            {
                upstream_of
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
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
    // Registered in strict authority order, and deterministically within each
    // tier. Iterating the node HashMap directly meant the winner of a contested
    // key changed between rebuilds of an unchanged tree — the same lookup string
    // could resolve to different nodes at different moments. Sorting by node id
    // inside each tier removes that nondeterminism.
    //
    // Tier order matters as much as determinism: a node's own `id` must outrank
    // another node's filename or title, or a lookup binds to whichever node
    // merely *looks* like the string being asked for.
    let mut map: HashMap<String, String> = HashMap::new();
    let mut ordered: Vec<&GraphNode> = nodes.values().collect();
    ordered.sort_unstable_by(|a, b| a.id.cmp(&b.id));

    // Tier 0 — the node's own id, and the identity keys it claims in frontmatter.
    for node in &ordered {
        map.entry(node.id.to_lowercase())
            .or_insert_with(|| node.id.clone());
        if let Some(ref tid) = node.task_id {
            map.entry(tid.to_lowercase())
                .or_insert_with(|| node.id.clone());
        }
        for pl in &node.permalinks {
            map.entry(pl.clone()).or_insert_with(|| node.id.clone());
        }
    }
    // Tier 1 — keys derived from the file's name (stem, scraped id-prefix).
    for node in &ordered {
        for wk in &node.weak_keys {
            map.entry(wk.clone()).or_insert_with(|| node.id.clone());
        }
        if let Some(stem) = node.path.file_stem() {
            map.entry(stem.to_string_lossy().to_lowercase())
                .or_insert_with(|| node.id.clone());
        }
    }
    // Tier 2 — human-facing title. Weakest: titles are prose, freely duplicated,
    // and never an identity claim.
    for node in &ordered {
        let label_key = node.label.to_lowercase();
        if !label_key.is_empty() {
            map.entry(label_key).or_insert_with(|| node.id.clone());
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
        assert!(
            s_today > 0.99 && s_today <= 1.0,
            "Today signal should be ~1.0, got {}",
            s_today
        );

        // (b) modified 30 days ago -> ~0.37 (exp(-1))
        let thirty_days_ago = now - Duration::try_days(30).unwrap();
        let s_30 = recency_signal(&thirty_days_ago, &now);
        let expected_30 = (-1.0f64).exp();
        assert!(
            (s_30 - expected_30).abs() < 0.0001,
            "30 days ago signal should be ~0.37, got {}",
            s_30
        );

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
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    fn make_doc_with_fields(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
        assignee: Option<&str>,
        tags: &[&str],
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
        if let Some(a) = assignee {
            fm.insert("assignee".to_string(), serde_json::json!(a));
        }
        if !tags.is_empty() {
            fm.insert("tags".to_string(), serde_json::json!(tags));
        }

        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    /// A memory-type document with a freeform `body`. Used to exercise the
    /// wikilink -> Link-edge -> orphan-detection path (the `/remember` quick
    /// path leans on body wikilinks rather than a structured parent field).
    fn make_memory_doc(path: &str, title: &str, id: &str, body: &str) -> PkbDocument {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), serde_json::json!(title));
        fm.insert("type".to_string(), serde_json::json!("memory"));
        fm.insert("id".to_string(), serde_json::json!(id));

        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: body.to_string(),
            doc_type: Some("memory".to_string()),
            status: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        }
    }

    #[test]
    fn test_memory_with_wikilink_is_not_orphan() {
        // AC: a memory created via the recommended quick path with one related
        // node (a body wikilink) must NOT be reported as an orphan.
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
            make_memory_doc(
                "memory/linked-note.md",
                "Linked Note",
                "linked-note",
                "A fact about [[epic-1]] worth remembering.",
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        // Sanity: the wikilink actually produced a Link edge.
        let linked = graph
            .resolve("linked-note")
            .expect("memory node should exist");
        assert!(
            graph.edges().iter().any(|e| e.source == linked.id
                && e.target == "epic-1"
                && e.edge_type == EdgeType::Link),
            "body wikilink should produce a Link edge"
        );

        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        assert!(
            !orphan_ids.contains("linked-note"),
            "a memory with a wikilink to a related node must not be an orphan"
        );
    }

    #[test]
    fn test_memory_without_links_is_orphan() {
        // A genuinely disconnected memory (no parent, no links) is still an orphan.
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
            make_memory_doc(
                "memory/lonely-note.md",
                "Lonely Note",
                "lonely-note",
                "A fact with no links at all.",
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        assert!(
            orphan_ids.contains("lonely-note"),
            "a memory with no parent and no links must be reported as an orphan"
        );
    }

    #[test]
    fn test_actionable_task_without_parent_is_orphan_despite_deps() {
        // Task hygiene is unchanged: an actionable node with no hierarchy parent
        // is an orphan even though it has dependency edges (web edges do not
        // excuse a missing tree parent). This keeps pkb_orphans in step with
        // graph_stats.orphan_count.
        let graph = build_test_graph();
        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        // task-c has a depends_on edge but no parent -> still an orphan.
        assert!(
            orphan_ids.contains("task-c"),
            "parentless task must remain an orphan despite dependency edges"
        );
        // task-a is parented under epic-1 -> not an orphan.
        assert!(
            !orphan_ids.contains("task-a"),
            "parented task must not be an orphan"
        );
    }

    #[test]
    fn test_memory_similarity_edge_does_not_unorphan() {
        // SimilarTo edges are auto-derived and must NOT rescue a node from
        // orphan status — only deliberate structural edges do.
        let docs = vec![make_memory_doc(
            "memory/solo-note.md",
            "Solo Note",
            "solo-note",
            "No deliberate links here.",
        )];
        let mut graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        // Inject a phantom SimilarTo edge (as the similarity pass would).
        graph.edges.push(Edge {
            source: "solo-note".to_string(),
            target: "some-other-node".to_string(),
            edge_type: EdgeType::SimilarTo,
        });

        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        assert!(
            orphan_ids.contains("solo-note"),
            "a SimilarTo edge must not un-orphan a node"
        );
    }

    #[test]
    fn test_memory_inbound_link_rescues() {
        // Connectivity counts in either direction: a memory with no outbound
        // links is still rescued when another node wikilinks TO it. (Pins the
        // spec's "in either direction" clause; /remember-contract enforcement
        // of outbound links is a separate, downstream concern.)
        let docs = vec![
            make_memory_doc(
                "memory/hub-note.md",
                "Hub Note",
                "hub-note",
                "See also [[quiet-note]].",
            ),
            make_memory_doc(
                "memory/quiet-note.md",
                "Quiet Note",
                "quiet-note",
                "No outbound links here.",
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        assert!(
            !orphan_ids.contains("quiet-note"),
            "a memory that is the target of another node's wikilink must not be an orphan"
        );
        assert!(
            !orphan_ids.contains("hub-note"),
            "the linking memory must not be an orphan either"
        );
    }

    #[test]
    fn test_goal_with_inbound_contributes_to_is_not_orphan() {
        // Out-of-tree strategic nodes (goal/target) are deliberately parentless;
        // an inbound ContributesTo edge from work is a deliberate structural
        // edge and must rescue them from orphan status.
        let mut goal_fm = serde_json::Map::new();
        goal_fm.insert("title".to_string(), serde_json::json!("Goal One"));
        goal_fm.insert("type".to_string(), serde_json::json!("goal"));
        goal_fm.insert("id".to_string(), serde_json::json!("goal-1"));
        let goal_doc = PkbDocument {
            path: PathBuf::from("goals/goal-1.md"),
            title: "Goal One".to_string(),
            body: String::new(),
            doc_type: Some("goal".to_string()),
            status: None,
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(goal_fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        };

        let mut task_fm = serde_json::Map::new();
        task_fm.insert("title".to_string(), serde_json::json!("Task A"));
        task_fm.insert("type".to_string(), serde_json::json!("task"));
        task_fm.insert("status".to_string(), serde_json::json!("active"));
        task_fm.insert("id".to_string(), serde_json::json!("task-ct"));
        task_fm.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "goal-1"}]),
        );
        let task_doc = PkbDocument {
            path: PathBuf::from("tasks/task-ct.md"),
            title: "Task A".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(task_fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        };

        let graph = GraphStore::build(&[goal_doc, task_doc], Path::new("/tmp/test-pkb"));

        // Sanity: the contributes_to frontmatter produced a ContributesTo edge.
        assert!(
            graph.edges().iter().any(|e| e.source == "task-ct"
                && e.target == "goal-1"
                && e.edge_type == EdgeType::ContributesTo),
            "contributes_to frontmatter should produce a ContributesTo edge"
        );

        let orphan_ids: HashSet<&str> = graph.orphans().iter().map(|n| n.id.as_str()).collect();
        assert!(
            !orphan_ids.contains("goal-1"),
            "a parentless goal with an inbound ContributesTo edge must not be an orphan"
        );
        // The task itself is actionable and parentless -> still an orphan
        // (tree rule unaffected by its outbound web edge).
        assert!(
            orphan_ids.contains("task-ct"),
            "the contributing task remains an orphan under the actionable tree rule"
        );
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
                "ready",
                "task-a",
                Some("epic-1"),
                &["task-b"],
            ),
            make_doc(
                "tasks/task-b.md",
                "Task B",
                "task",
                "ready",
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
        assert!(node.unwrap().id.contains("task"));
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

    // ── identity precedence: a derived key must never shadow a real id ──
    //
    // Regression coverage for aops-9461748b. Each case below binds a stored
    // reference to the wrong node on the pre-fix resolver, because filename
    // stems and the scraped id-prefix shared one flat, last-write-wins namespace
    // with frontmatter ids.

    /// Reproduction 2, verbatim: a file named `personal.md` whose id is
    /// `personal-66647271` claims the derived key `personal`, while a *different*
    /// node genuinely has `id: personal`. `parent: personal` must reach the node
    /// that owns the id, not the node that owns the filename.
    #[test]
    fn test_exact_id_beats_another_nodes_filename_stem() {
        let docs = vec![
            // Genuinely owns the id "personal".
            make_doc(
                "roots/personal-root.md",
                "Personal Root",
                "epic",
                "ready",
                "personal",
                None,
                &[],
            ),
            // Filename stem "personal" — a derived claim on that string. Listed
            // AFTER the id owner on purpose: last-write-wins is what let the
            // derived key seize the string in the live incident.
            make_doc(
                "personal/personal.md",
                "Personal — projects, home and life",
                "epic",
                "ready",
                "personal-66647271",
                None,
                &[],
            ),
            make_doc(
                "projects/proj-04dcf372-life-admin.md",
                "Handle life admin",
                "epic",
                "ready",
                "proj-04dcf372",
                Some("personal"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        assert_eq!(
            graph.resolve("personal").map(|n| n.id.as_str()),
            Some("personal"),
            "the string `personal` must resolve to the node whose id IS `personal`, \
             not to the node that merely lives in a file called personal.md"
        );
        let child = graph.resolve("proj-04dcf372").expect("child node missing");
        assert_eq!(
            child.parent.as_deref(),
            Some("personal"),
            "`parent: personal` must bind to the id owner; binding it to \
             personal-66647271 is the duplicate-ancestry defect"
        );
    }

    /// Reproduction 1, verbatim: `epic-a0523a25` is a strict string prefix of
    /// `epic-a0523a25-20260531-2153`. The dated instance's filename synthesises
    /// the key `epic-a0523a25` via TASK_ID_PREFIX_RE. The short id must still win.
    #[test]
    fn test_exact_id_beats_scraped_id_prefix_of_a_longer_id() {
        let docs = vec![
            make_doc(
                "tasks/epic-a0523a25-systematically-triage-issues.md",
                "Issue sweep",
                "epic",
                "ready",
                "epic-a0523a25",
                None,
                &[],
            ),
            // Registered AFTER the id owner: its filename scrapes the key
            // `epic-a0523a25`, and last-write-wins handed it the string.
            make_doc(
                "tasks/epic-a0523a25-20260531-2153-issue-sweep.md",
                "Issue sweep (dated instance)",
                "epic",
                "ready",
                "epic-a0523a25-20260531-2153",
                None,
                &[],
            ),
            make_doc(
                "tasks/child-1.md",
                "A real child",
                "task",
                "ready",
                "child-1",
                Some("epic-a0523a25"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        assert_eq!(
            graph.resolve("epic-a0523a25").map(|n| n.id.as_str()),
            Some("epic-a0523a25"),
            "a strict prefix of a longer id must resolve to the node that owns it exactly"
        );
        let dated = graph
            .resolve("epic-a0523a25-20260531-2153")
            .expect("dated instance missing");
        assert!(
            !dated.children.iter().any(|c| c == "child-1"),
            "child-1 declares parent epic-a0523a25; attributing it to the dated \
             instance is the phantom-children defect"
        );
        let short = graph.resolve("epic-a0523a25").unwrap();
        assert!(
            short.children.iter().any(|c| c == "child-1"),
            "the real child must land on the node it actually names"
        );
    }

    /// The collision is symmetric — it must not depend on which node the
    /// directory walk reaches first. Same fixture, reversed input order.
    #[test]
    fn test_identity_precedence_is_independent_of_scan_order() {
        let mk = || {
            vec![
                make_doc(
                    "personal/personal.md",
                    "Personal",
                    "epic",
                    "ready",
                    "personal-66647271",
                    None,
                    &[],
                ),
                make_doc(
                    "roots/personal-root.md",
                    "Personal Root",
                    "epic",
                    "ready",
                    "personal",
                    None,
                    &[],
                ),
            ]
        };
        let forward = mk();
        let mut reversed = mk();
        reversed.reverse();

        for (label, mut docs) in [("forward", forward), ("reversed", reversed)] {
            // A child whose stored `parent: personal` must bind the same way in
            // both orders. Edge building goes through the lookup map, so this —
            // unlike `resolve()`, which short-circuits on an exact id hit —
            // actually exercises the contested namespace.
            docs.push(make_doc(
                "projects/child.md",
                "Child",
                "epic",
                "ready",
                "child-1",
                Some("personal"),
                &[],
            ));
            let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
            assert_eq!(
                graph
                    .resolve("child-1")
                    .and_then(|n| n.parent.clone())
                    .as_deref(),
                Some("personal"),
                "scan order `{label}` changed which node `parent: personal` bound to"
            );
        }
    }

    /// A derived key with no id owner still resolves — the fix restricts
    /// precedence, it does not withdraw filename-stem lookup. This guards the
    /// established `parent: <project-root-filename>` shorthand from regressing.
    #[test]
    fn test_filename_stem_still_resolves_when_no_node_claims_the_id() {
        let docs = vec![make_doc(
            "personal/personal.md",
            "Personal",
            "epic",
            "ready",
            "personal-66647271",
            None,
            &[],
        )];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        assert_eq!(
            graph.resolve("personal").map(|n| n.id.as_str()),
            Some("personal-66647271"),
            "with no node claiming the id `personal`, the filename stem must still resolve"
        );
    }

    /// `parent_raw` preserves what the file says even when `parent` is rewritten
    /// to the resolved id. This is the field the write paths compare against.
    #[test]
    fn test_parent_raw_preserves_the_stored_string() {
        let docs = vec![
            make_doc(
                "personal/personal.md",
                "Personal",
                "epic",
                "ready",
                "personal-66647271",
                None,
                &[],
            ),
            make_doc(
                "projects/proj-04dcf372-life-admin.md",
                "Handle life admin",
                "epic",
                "ready",
                "proj-04dcf372",
                Some("personal"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let child = graph.resolve("proj-04dcf372").unwrap();
        assert_eq!(
            child.parent.as_deref(),
            Some("personal-66647271"),
            "resolved parent is the canonical id"
        );
        assert_eq!(
            child.parent_raw.as_deref(),
            Some("personal"),
            "parent_raw must still hold the unnormalised string that is on disk — \
             without it no write path can tell the file needs repairing"
        );
    }

    // ── parent_chain_to ──

    #[test]
    fn test_parent_chain_to_reaches_target() {
        // task-a has parent epic-1; walking from task-a reaches epic-1.
        let graph = build_test_graph();
        let chain = graph.parent_chain_to("task-a", "epic-1");
        assert!(chain.is_some(), "parent chain should reach epic-1");
        let chain = chain.unwrap();
        assert_eq!(chain.first().map(|s| s.as_str()), Some("task-a"));
        assert_eq!(chain.last().map(|s| s.as_str()), Some("epic-1"));
    }

    #[test]
    fn test_parent_chain_to_reflexive() {
        // start == target should return Some([target]).
        let graph = build_test_graph();
        let chain = graph.parent_chain_to("epic-1", "epic-1");
        assert_eq!(chain, Some(vec!["epic-1".to_string()]));
    }

    #[test]
    fn test_parent_chain_to_unreachable() {
        // task-a's chain is [task-a, epic-1]; task-b is not in it.
        let graph = build_test_graph();
        assert!(graph.parent_chain_to("task-a", "task-b").is_none());
    }

    #[test]
    fn test_parent_chain_to_unknown_start() {
        let graph = build_test_graph();
        assert!(graph.parent_chain_to("nonexistent", "epic-1").is_none());
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

        // Verify a specific derived value survives the rebuild
        let node_a = graph.get_node("task-a").unwrap();
        let node_a2 = graph2.get_node("task-a").unwrap();
        assert_eq!(node_a.flow, node_a2.flow);
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

    #[test]
    fn test_all_tasks_filters_non_actionable_and_completed() {
        let docs = vec![
            make_doc(
                "tasks/task-open.md",
                "Open Task",
                "task",
                "ready",
                "task-open",
                None,
                &[],
            ),
            make_doc(
                "tasks/task-done.md",
                "Done Task",
                "task",
                "done",
                "task-done",
                None,
                &[],
            ),
            make_doc(
                "tasks/task-archived.md",
                "Archived Task",
                "task",
                "archived",
                "task-archived",
                None,
                &[],
            ),
            make_doc(
                "notes/note-1.md",
                "Random Note",
                "note",
                "inbox",
                "note-1",
                None,
                &[],
            ),
        ];
        let root = Path::new("/tmp/test-pkb");
        let graph = GraphStore::build(&docs, root);

        let open_tasks = graph.all_tasks();
        assert_eq!(open_tasks.len(), 1);
        assert_eq!(open_tasks[0].id, "task-open");

        let all_actionable = graph.actionable_tasks(true);
        assert_eq!(all_actionable.len(), 3);
        assert!(all_actionable.iter().any(|n| n.id == "task-open"));
        assert!(all_actionable.iter().any(|n| n.id == "task-done"));
        assert!(all_actionable.iter().any(|n| n.id == "task-archived"));
        assert!(!all_actionable.iter().any(|n| n.id == "note-1"));

        let all_nodes = graph.all_nodes_with_id(true);
        assert_eq!(all_nodes.len(), 4);
    }

    // ── ready/blocked classification ──

    #[test]
    fn test_ready_tasks() {
        let graph = build_test_graph();
        let ready = graph.ready_tasks();
        // task-b should be ready (no deps, leaf, status=ready)
        let task_b_id = graph.resolve("task-b").unwrap().id.clone();
        assert!(ready.iter().any(|n| n.id == task_b_id));
    }

    #[test]
    fn test_ready_excludes_container_types() {
        // Container tasks with children, projects, and learn nodes are not claimable in ready.
        // Leaf tasks are claimable.
        let docs = vec![
            make_doc(
                "tasks/epic-1.md",
                "Container Task",
                "task",
                "ready",
                "epic-1",
                None,
                &[],
            ),
            make_doc(
                "tasks/proj-1.md",
                "Lone Project",
                "project",
                "ready",
                "proj-1",
                None,
                &[],
            ),
            make_doc(
                "tasks/task-1.md",
                "Task One",
                "task",
                "ready",
                "task-1",
                Some("epic-1"),
                &[],
            ),
            make_doc(
                "tasks/task-2.md",
                "Task Two",
                "task",
                "ready",
                "task-2",
                Some("proj-1"),
                &[],
            ),
            make_doc(
                "tasks/learn-1.md",
                "Learn One",
                "learn",
                "ready",
                "learn-1",
                None,
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let ready = graph.ready_tasks();
        let ready_ids: Vec<&str> = ready.iter().map(|n| n.id.as_str()).collect();
        assert!(
            !ready_ids.contains(&"epic-1"),
            "container tasks with children must not appear in ready"
        );
        assert!(
            !ready_ids.contains(&"proj-1"),
            "projects must not appear in ready"
        );
        assert!(
            !ready_ids.contains(&"learn-1"),
            "learn must not appear in ready"
        );
        assert!(ready_ids.contains(&"task-1"), "task must be in ready");
        assert!(ready_ids.contains(&"task-2"), "task must be in ready");
    }

    #[test]
    fn test_ready_excludes_inbox_tasks() {
        // Computed graduation (`inbox → ready` is automatic, not a manual edit):
        // a *bare* inbox task (no AC, not decomposed) still needs triage and must
        // NOT be ready. An inbox leaf that has *graduated* — acceptance criteria
        // present, all deps resolved — IS surfaced as ready with no status edit.
        // An explicit `ready` task remains ready.
        let mut inbox_graduated = make_doc(
            "tasks/task-inbox-ac.md",
            "Graduated Inbox Task",
            "task",
            "inbox",
            "task-inbox-ac",
            None,
            &[],
        );
        inbox_graduated.body = "## Acceptance Criteria\n- it ships\n".to_string();

        let docs = vec![
            make_doc(
                "tasks/task-inbox.md",
                "Bare Inbox Task",
                "task",
                "inbox",
                "task-inbox",
                None,
                &[],
            ),
            inbox_graduated,
            make_doc(
                "tasks/task-ready.md",
                "Ready Task",
                "task",
                "ready",
                "task-ready",
                None,
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let ready_ids: Vec<&str> = graph.ready_tasks().iter().map(|n| n.id.as_str()).collect();
        assert!(
            !ready_ids.contains(&"task-inbox"),
            "bare inbox tasks (no AC) must not appear in ready"
        );
        assert!(
            ready_ids.contains(&"task-inbox-ac"),
            "graduated inbox leaf (AC + deps resolved) must be surfaced as ready"
        );
        assert!(
            ready_ids.contains(&"task-ready"),
            "ready tasks must appear in ready"
        );
    }

    #[test]
    fn test_blocked_tasks() {
        let graph = build_test_graph();
        let blocked = graph.blocked_tasks();
        // task-a should be blocked (depends on task-b which isn't done)
        let task_a_id = graph.resolve("task-a").unwrap().id.clone();
        assert!(blocked.iter().any(|n| n.id == task_a_id));
    }

    #[test]
    fn test_blocker_resolution_case_insensitive() {
        // Reproduce academicOps #547:
        // Blocker task has lowercase id: "academicops-d067e425", status: "done"
        // Downstream tasks reference it with mixed case: "academicOps-d067e425" or uppercase
        let docs = vec![
            make_doc(
                "tasks/academicops-d067e425.md",
                "Upstream Done Task",
                "task",
                "done",
                "academicops-d067e425",
                None,
                &[],
            ),
            make_doc(
                "tasks/academicops-4a31fae0.md",
                "Downstream Task A",
                "task",
                "ready",
                "academicops-4a31fae0",
                None,
                &["academicOps-d067e425"],
            ),
            make_doc(
                "tasks/academicops-2200e5e8.md",
                "Downstream Task B",
                "task",
                "ready",
                "academicops-2200e5e8",
                None,
                &["ACADEMICOPS-D067E425"],
            ),
            make_doc(
                "tasks/open-blocker.md",
                "Open Blocker Task",
                "task",
                "inbox",
                "open-blocker-1234",
                None,
                &[],
            ),
            make_doc(
                "tasks/blocked-task.md",
                "Blocked Task",
                "task",
                "ready",
                "blocked-task-5678",
                None,
                &["OPEN-BLOCKER-1234"],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        // 1. Downstream tasks depending on completed blocker (with mixed case) must NOT be blocked
        assert!(
            !graph.is_blocked("academicops-4a31fae0"),
            "task-4a31fae0 should not be blocked"
        );
        assert!(
            !graph.is_blocked("academicops-2200e5e8"),
            "task-2200e5e8 should not be blocked"
        );
        assert!(
            graph.ready.contains(&"academicops-4a31fae0".to_string()),
            "task-4a31fae0 should be in ready queue"
        );
        assert!(
            graph.ready.contains(&"academicops-2200e5e8".to_string()),
            "task-2200e5e8 should be in ready queue"
        );

        // 2. Downstream task depending on open blocker (with mixed case) MUST be blocked
        assert!(
            graph.is_blocked("blocked-task-5678"),
            "blocked-task-5678 should be blocked"
        );
        assert!(
            graph.blocked.contains(&"blocked-task-5678".to_string()),
            "blocked-task-5678 should be in blocked list"
        );

        // 3. Blocks relationship symmetry is populated on the blocker
        let upstream = graph.resolve("academicops-d067e425").unwrap();
        assert!(upstream
            .blocks
            .contains(&"academicops-4a31fae0".to_string()));
        assert!(upstream
            .blocks
            .contains(&"academicops-2200e5e8".to_string()));

        // 4. Depends_on relationship on downstream task is canonicalized
        let downstream_a = graph.resolve("academicops-4a31fae0").unwrap();
        assert_eq!(downstream_a.depends_on, vec!["academicops-d067e425"]);
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
        assert!(
            task_a.reachable,
            "task-a should be reachable (leaf with unmet dep)"
        );

        let epic_1 = graph.resolve("epic-1").unwrap();
        assert!(
            epic_1.reachable,
            "epic-1 should be reachable (parent of leaves)"
        );

        let task_c = graph.resolve("task-c").unwrap();
        assert!(
            task_c.reachable,
            "task-c should be reachable (leaf with dep)"
        );
    }

    #[test]
    fn test_reachable_excludes_done_orphans() {
        // A completed node with no active descendants should NOT be reachable
        let docs = vec![
            make_doc(
                "tasks/done-task.md",
                "Done Task",
                "task",
                "done",
                "done-task",
                None,
                &[],
            ),
            make_doc(
                "tasks/active-task.md",
                "Active Task",
                "task",
                "active",
                "active-task",
                None,
                &[],
            ),
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
            make_doc(
                "tasks/done-parent.md",
                "Done Parent",
                "project",
                "done",
                "done-parent",
                None,
                &[],
            ),
            make_doc(
                "tasks/active-child.md",
                "Active Child",
                "task",
                "active",
                "active-child",
                Some("done-parent"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let parent = graph.resolve("done-parent").unwrap();
        assert!(
            parent.reachable,
            "done parent of active leaf should be reachable"
        );
    }

    #[test]
    fn test_reachable_excludes_notes() {
        // Notes without status should not seed BFS
        let docs = vec![
            make_doc(
                "notes/my-note.md",
                "My Note",
                "note",
                "",
                "my-note",
                None,
                &[],
            ),
            make_doc(
                "tasks/my-task.md",
                "My Task",
                "task",
                "active",
                "my-task",
                None,
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let note = graph.resolve("my-note").unwrap();
        assert!(
            !note.reachable,
            "note without status should not be reachable"
        );
    }

    // ── subtask relationships ──

    #[test]
    fn test_subtasks_separate_from_children() {
        // A parent task with both a regular child and a subtask:
        // - the subtask must appear in parent.subtasks, NOT in parent.children
        // - parent.leaf must remain true (subtasks don't affect leaf status)
        let docs = vec![
            make_doc(
                "tasks/parent.md",
                "Parent Task",
                "task",
                "active",
                "parent-abc",
                None,
                &[],
            ),
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
            make_doc(
                "tasks/parent2.md",
                "Parent 2",
                "task",
                "active",
                "parent-def",
                None,
                &[],
            ),
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
        assert!(
            parent2.leaf,
            "parent with only subtasks should still be a leaf"
        );
    }

    /// Helper: create a PkbDocument with soft_depends_on frontmatter.
    fn make_doc_with_soft_dep(
        path: &str,
        title: &str,
        id: &str,
        soft_deps: &[&str],
    ) -> PkbDocument {
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
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
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

    /// Task aops_fd5283aa AC3 & AC4:
    /// Focus picks exclude review status tasks; ready_tasks excludes review status tasks.
    #[test]
    fn test_review_status_excluded_from_focus_picks_and_ready_tasks() {
        let docs = vec![
            make_doc_with_fields(
                "tasks/review-task.md",
                "DECISION (Nic): push the rtk worker image",
                "task",
                "review",
                "review-task-1",
                None,
                &[],
                Some("Nic"),
                &["decision", "needs-nic", "human-approval"],
            ),
            make_doc_with_fields(
                "tasks/claimable-work.md",
                "Implement telemetry exporter",
                "task",
                "ready",
                "claimable-work-1",
                None,
                &[],
                None,
                &["telemetry"],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        // Focus picks must not surface the review task
        let picks = graph.focus_picks(10);
        assert!(
            !picks.contains(&"review-task-1".to_string()),
            "review-status task MUST NOT appear in focus_picks"
        );
        assert!(
            picks.contains(&"claimable-work-1".to_string()),
            "ready task must appear in focus_picks"
        );

        // ready_tasks() must strictly exclude the review-status task (staying non-claimable by polecats)
        let ready_ids: Vec<&str> = graph.ready_tasks().iter().map(|n| n.id.as_str()).collect();
        assert!(
            !ready_ids.contains(&"review-task-1"),
            "review-status task MUST NOT be in ready_tasks() (not claimable by polecats)"
        );
        assert!(
            ready_ids.contains(&"claimable-work-1"),
            "ready task must be in ready_tasks()"
        );
    }

    #[test]
    fn test_blocked_task_excluded_from_focus_picks() {
        let docs = vec![
            make_doc_with_fields(
                "tasks/prereq.md",
                "Prerequisite task",
                "task",
                "inbox",
                "prereq-1",
                None,
                &[],
                None,
                &[],
            ),
            make_doc_with_fields(
                "tasks/blocked-task.md",
                "DECISION: deploy to cluster",
                "task",
                "ready",
                "blocked-task-1",
                None,
                &["prereq-1"],
                None,
                &["decision"],
            ),
            make_doc_with_fields(
                "tasks/ready-work.md",
                "Claimable work item",
                "task",
                "ready",
                "ready-work-1",
                None,
                &[],
                None,
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        assert!(
            graph.is_blocked("blocked-task-1"),
            "task with unfinished prerequisite must be classified as blocked"
        );
        let picks = graph.focus_picks(10);
        assert!(
            !picks.contains(&"blocked-task-1".to_string()),
            "blocked task with unfinished dependency MUST NOT appear in focus_picks"
        );
        assert!(
            picks.contains(&"ready-work-1".to_string()),
            "unblocked ready work MUST appear in focus_picks"
        );

        // When prerequisite finishes, the task is unblocked and surfaces
        let docs_resolved = vec![
            make_doc_with_fields(
                "tasks/prereq.md",
                "Prerequisite task",
                "task",
                "done",
                "prereq-1",
                None,
                &[],
                None,
                &[],
            ),
            make_doc_with_fields(
                "tasks/blocked-task.md",
                "DECISION: deploy to cluster",
                "task",
                "ready",
                "blocked-task-1",
                None,
                &["prereq-1"],
                None,
                &["decision"],
            ),
        ];
        let graph_resolved = GraphStore::build(&docs_resolved, Path::new("/tmp/test-pkb"));
        assert!(
            !graph_resolved.is_blocked("blocked-task-1"),
            "task with completed dependency is unblocked"
        );
        let picks_resolved = graph_resolved.focus_picks(10);
        assert!(
            picks_resolved.contains(&"blocked-task-1".to_string()),
            "unblocked ready task MUST now surface in focus_picks"
        );
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
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                None,
                &["task-b"],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                None,
                &["task-a"],
            ),
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
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                None,
                &["task-b"],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                None,
                &[],
            ),
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
        assert_eq!(
            graph.find_soft_cycle_count(),
            1,
            "soft mutual dependency = one soft cycle"
        );
        // Must not appear in hard cycles
        assert!(graph.find_hard_cycles().is_empty());
    }

    #[test]
    fn test_compute_project_field_hierarchy() {
        // Setup a hierarchy (project = explicit routing slug, inherited via parent):
        // Epic A (type: epic, project: "aops")
        //   Epic B (type: epic, parent: Epic A, project: "mem")   -- subtree override
        //     Task C (type: task, parent: Epic B)                  -- inherits "mem"
        //   Task F (type: task, parent: Epic A)                    -- inherits "aops"
        // Task D (type: task, project: "other")                    -- keeps own value

        let mut fm_a = serde_json::Map::new();
        fm_a.insert("type".to_string(), serde_json::json!("epic"));
        fm_a.insert("id".to_string(), serde_json::json!("epic-a"));
        fm_a.insert("project".to_string(), serde_json::json!("aops"));

        let mut fm_b = serde_json::Map::new();
        fm_b.insert("type".to_string(), serde_json::json!("epic"));
        fm_b.insert("id".to_string(), serde_json::json!("epic-b"));
        fm_b.insert("parent".to_string(), serde_json::json!("epic-a"));
        fm_b.insert("project".to_string(), serde_json::json!("mem"));

        let mut fm_d = serde_json::Map::new();
        fm_d.insert("type".to_string(), serde_json::json!("task"));
        fm_d.insert("id".to_string(), serde_json::json!("task-d"));
        fm_d.insert("project".to_string(), serde_json::json!("other"));

        let mk_fm_doc =
            |path: &str, title: &str, fm: serde_json::Map<String, serde_json::Value>| PkbDocument {
                path: PathBuf::from(path),
                title: title.to_string(),
                body: String::new(),
                doc_type: fm.get("type").and_then(|v| v.as_str()).map(String::from),
                status: Some("active".to_string()),
                consolidated: None,
                consolidated_at: None,
                modified: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm)),
                content_hash: "test_hash".to_string(),
                file_hash: "test_hash".to_string(),
            };

        let docs = vec![
            mk_fm_doc("epics/a.md", "Epic A", fm_a),
            mk_fm_doc("epics/b.md", "Epic B", fm_b),
            make_doc(
                "tasks/c.md",
                "Task C",
                "task",
                "active",
                "task-c",
                Some("epic-b"),
                &[],
            ),
            mk_fm_doc("tasks/d.md", "Task D", fm_d),
            make_doc(
                "tasks/f.md",
                "Task F",
                "task",
                "active",
                "task-f",
                Some("epic-a"),
                &[],
            ),
        ];

        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let a = graph.get_node("epic-a").unwrap();
        let b = graph.get_node("epic-b").unwrap();
        let c = graph.get_node("task-c").unwrap();
        let d = graph.get_node("task-d").unwrap();
        let f = graph.get_node("task-f").unwrap();

        assert_eq!(
            a.project.as_deref(),
            Some("aops"),
            "Epic A keeps its explicit slug"
        );
        assert_eq!(
            b.project.as_deref(),
            Some("mem"),
            "Epic B's explicit slug overrides for its subtree"
        );
        assert_eq!(
            c.project.as_deref(),
            Some("mem"),
            "Task C inherits the NEAREST explicit value (Epic B's)"
        );
        assert_eq!(
            d.project.as_deref(),
            Some("other"),
            "Task D keeps its explicit value"
        );
        assert_eq!(
            f.project.as_deref(),
            Some("aops"),
            "Task F inherits Epic A's slug"
        );
    }

    /// AC5 (part 1): a non-empty `stated_weight` that fails to match the
    /// verbal contribution-weight scale is rejected at parse time — recorded
    /// as a `ParseWarning` and contributing zero, never the legacy 0.3
    /// "soft" default.
    #[test]
    fn test_stated_weight_out_of_scale_rejected_at_parse_time_not_defaulted() {
        let mut fm = serde_json::Map::new();
        fm.insert(
            "title".to_string(),
            serde_json::json!("garbage weight test"),
        );
        fm.insert("type".to_string(), serde_json::json!("task"));
        fm.insert("id".to_string(), serde_json::json!("garbage-weight-task"));
        fm.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "some-target", "stated_weight": "super duper critical"}]),
        );
        let doc = PkbDocument {
            path: std::path::PathBuf::from("tasks/garbage-weight-task.md"),
            title: "garbage weight test".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };
        let node = GraphNode::from_pkb_document(&doc);

        assert_eq!(
            node.contributes_to[0].numeric_weight(),
            0.0,
            "an unrecognized stated_weight must contribute zero, not the legacy 0.3 default"
        );
        assert!(
            node.parse_warnings
                .iter()
                .any(|w| w.field == "contributes_to.stated_weight"),
            "an unrecognized non-empty stated_weight must be flagged via ParseWarning, not \
             silently accepted; got warnings: {:?}",
            node.parse_warnings
        );
    }

    /// AC5 (part 2): a genuinely *omitted* `stated_weight` is not an error —
    /// it's a deliberately unstated edge — so it must be silently zero with
    /// no ParseWarning.
    #[test]
    fn test_stated_weight_omitted_is_silently_zero_no_warning() {
        let mut fm = serde_json::Map::new();
        fm.insert(
            "title".to_string(),
            serde_json::json!("unstated weight test"),
        );
        fm.insert("type".to_string(), serde_json::json!("task"));
        fm.insert("id".to_string(), serde_json::json!("unstated-weight-task"));
        fm.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "some-target"}]),
        );
        let doc = PkbDocument {
            path: std::path::PathBuf::from("tasks/unstated-weight-task.md"),
            title: "unstated weight test".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };
        let node = GraphNode::from_pkb_document(&doc);

        assert_eq!(node.contributes_to[0].numeric_weight(), 0.0);
        assert!(
            node.parse_warnings.is_empty(),
            "an omitted stated_weight is not an error and must not be flagged; got {:?}",
            node.parse_warnings
        );
    }

    /// `standing_weight` out-of-range input is rejected the same way
    /// `severity`/`goal_type` already are — flagged, not clamped or defaulted
    /// (Zero Defaults / Zero Inference applies most strongly to this field).
    #[test]
    fn test_standing_weight_out_of_range_rejected() {
        let mut fm = serde_json::Map::new();
        fm.insert("title".to_string(), serde_json::json!("overpriced target"));
        fm.insert("type".to_string(), serde_json::json!("target"));
        fm.insert("id".to_string(), serde_json::json!("overpriced-target"));
        fm.insert("goal_type".to_string(), serde_json::json!("committed"));
        fm.insert("standing_weight".to_string(), serde_json::json!(1.5));
        let doc = PkbDocument {
            path: std::path::PathBuf::from("targets/overpriced-target.md"),
            title: "overpriced target".to_string(),
            body: String::new(),
            doc_type: Some("target".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };
        let node = GraphNode::from_pkb_document(&doc);

        assert_eq!(node.standing_weight, None);
        assert!(
            node.parse_warnings
                .iter()
                .any(|w| w.field == "standing_weight"),
            "out-of-range standing_weight must be flagged, not clamped or defaulted; got {:?}",
            node.parse_warnings
        );
    }

    // ── Weight Divergence Detection ──────────────────────────────────────────

    #[test]
    fn test_weight_divergence_detection() {
        use chrono::Utc;

        let now = Utc::now();
        // chrono 0.4.31+ requires Duration::try_days, or just use 24*3600*N
        let stale_dt = (now - chrono::Duration::try_days(20).unwrap()).to_rfc3339();
        let fresh_dt = (now - chrono::Duration::try_days(2).unwrap()).to_rfc3339();

        let make_doc_with_modified = |id: &str,
                                      modified: Option<String>,
                                      contributes_to: Vec<serde_json::Value>|
         -> PkbDocument {
            let mut fm = serde_json::Map::new();
            fm.insert("id".to_string(), serde_json::json!(id));
            fm.insert("type".to_string(), serde_json::json!("task"));
            fm.insert("status".to_string(), serde_json::json!("ready"));
            // Mirror each contributes_to edge as a flow `serves` link whose
            // strength (quantum x probability) tracks the verbal weight.
            let links: Vec<serde_json::Value> = contributes_to
                .iter()
                .map(|ct| {
                    let quantum = if ct["weight"] == "Certain" { 1.0 } else { 0.2 };
                    serde_json::json!({
                        "to": ct["to"],
                        "label": "serves",
                        "quantum": quantum,
                        "probability": 1.0,
                        "why": ct["why"],
                    })
                })
                .collect();
            fm.insert(
                "contributes_to".to_string(),
                serde_json::json!(contributes_to),
            );
            fm.insert("links".to_string(), serde_json::json!(links));
            PkbDocument {
                path: std::path::PathBuf::from(format!("tasks/{}.md", id)),
                title: id.to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                consolidated: None,
                consolidated_at: None,
                modified,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm)),
                content_hash: "test".to_string(),
                file_hash: "test".to_string(),
            }
        };

        let ct_high = serde_json::json!({
            "to": "target",
            "weight": "Certain",
            "why": "Crucial contribution"
        });

        let docs = vec![
            // 1. Stale task with high weight -> should be flagged
            make_doc_with_modified("stale-high", Some(stale_dt), vec![ct_high.clone()]),
            // 2. Fresh task with high weight -> should NOT be flagged
            make_doc_with_modified("fresh-high", Some(fresh_dt), vec![ct_high.clone()]),
            // 3. Stale task with low weight -> should NOT be flagged
            make_doc_with_modified(
                "stale-low",
                Some((now - chrono::Duration::try_days(20).unwrap()).to_rfc3339()),
                vec![serde_json::json!({
                    "to": "target",
                    "weight": "Uncertain",
                    "why": "Minor contribution"
                })],
            ),
            // 4. Stale blocked task -> should NOT be flagged
            {
                let mut doc = make_doc_with_modified(
                    "stale-blocked",
                    Some((now - chrono::Duration::try_days(20).unwrap()).to_rfc3339()),
                    vec![ct_high],
                );
                if let Some(ref mut fm) = doc.frontmatter {
                    fm.as_object_mut()
                        .unwrap()
                        .insert("status".to_string(), serde_json::json!("inbox"));
                    fm.as_object_mut()
                        .unwrap()
                        .insert("depends_on".to_string(), serde_json::json!(["unmet-dep"]));
                }
                doc.status = Some("inbox".to_string());
                doc
            },
        ];

        let graph = GraphStore::build(&docs, Path::new("/tmp/test-divergence"));
        let divergences = graph.detect_weight_divergences(14);

        assert_eq!(
            divergences.len(),
            1,
            "Expected exactly 1 divergence, found {:?}",
            divergences
        );
        assert_eq!(divergences[0].source_id, "stale-high");
        assert_eq!(divergences[0].strength, Some(1.0));

        // Verify that the anomaly_flag was also set during build
        let node = graph.get_node("stale-high").unwrap();
        assert!(
            node.contributes_to[0].anomaly_flag,
            "anomaly_flag should be set during build"
        );

        let fresh_node = graph.get_node("fresh-high").unwrap();
        assert!(
            !fresh_node.contributes_to[0].anomaly_flag,
            "anomaly_flag should NOT be set for fresh task"
        );
    }

    // ── parent/child cycle detection ─────────────────────────────────────────

    /// Build a small linear parent chain:  root → mid → leaf
    fn build_parent_chain_graph() -> GraphStore {
        let docs = vec![
            make_doc("tasks/root.md", "Root", "epic", "active", "root", None, &[]),
            make_doc(
                "tasks/mid.md",
                "Mid",
                "task",
                "active",
                "mid",
                Some("root"),
                &[],
            ),
            make_doc(
                "tasks/leaf.md",
                "Leaf",
                "task",
                "active",
                "leaf",
                Some("mid"),
                &[],
            ),
            make_doc(
                "tasks/sib.md",
                "Sibling",
                "task",
                "active",
                "sib",
                Some("root"),
                &[],
            ),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    #[test]
    fn parent_cycle_self_parent_rejected() {
        let g = build_parent_chain_graph();
        let err = g.would_create_parent_cycle("leaf", "leaf").unwrap_err();
        assert!(err.contains("cannot be its own parent"), "got: {err}");
    }

    #[test]
    fn parent_cycle_two_node_rejected() {
        // Existing chain: mid → root. Attempt to make root's parent be mid.
        let g = build_parent_chain_graph();
        let err = g.would_create_parent_cycle("root", "mid").unwrap_err();
        assert!(err.contains("circular"), "got: {err}");
    }

    #[test]
    fn parent_cycle_three_node_rejected() {
        // Chain: leaf → mid → root. Setting root's parent to leaf would close a 3-cycle.
        let g = build_parent_chain_graph();
        let err = g.would_create_parent_cycle("root", "leaf").unwrap_err();
        assert!(err.contains("circular"), "got: {err}");
    }

    #[test]
    fn parent_cycle_legal_deep_chain_allowed() {
        // New node `deeper` with parent `leaf` extends the chain — no cycle.
        let g = build_parent_chain_graph();
        assert!(g.would_create_parent_cycle("deeper", "leaf").is_ok());
    }

    #[test]
    fn parent_cycle_sibling_not_a_cycle() {
        // `sib` and `mid` are both children of `root`. Reparenting `sib` under `mid`
        // is NOT a cycle (mid's parent is root, not sib).
        let g = build_parent_chain_graph();
        assert!(g.would_create_parent_cycle("sib", "mid").is_ok());
    }

    #[test]
    fn parent_cycle_unrelated_node_allowed() {
        // Brand-new id with no graph presence — parent chain doesn't reach it.
        let g = build_parent_chain_graph();
        assert!(g.would_create_parent_cycle("brand-new-id", "mid").is_ok());
    }

    #[test]
    fn find_parent_cycles_empty_when_dag() {
        let g = build_parent_chain_graph();
        assert!(g.find_parent_cycles().is_empty());
    }

    #[test]
    fn test_hard_cycle_depends_on_two_node() {
        let docs = vec![
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                None,
                &["task-b"],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                None,
                &[],
            ),
        ];
        let g = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let err = g.would_create_hard_cycle("task-b", "task-a").unwrap_err();
        assert!(err.contains("circular dependency cycle"), "got: {err}");
    }

    #[test]
    fn test_hard_cycle_depends_on_transitive() {
        let docs = vec![
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                None,
                &["task-b"],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                None,
                &["task-c"],
            ),
            make_doc(
                "tasks/c.md",
                "Task C",
                "task",
                "active",
                "task-c",
                None,
                &[],
            ),
        ];
        let g = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let err = g.would_create_hard_cycle("task-c", "task-a").unwrap_err();
        assert!(err.contains("circular dependency cycle"), "got: {err}");
    }

    #[test]
    fn test_hard_cycle_mixed_parent_depends_on() {
        // task-a has parent task-b (edge task-a -> task-b)
        // task-b depends on task-c (edge task-b -> task-c)
        // Attempting to make task-c depend on task-a (edge task-c -> task-a) closes cycle: task-a -> task-b -> task-c -> task-a
        let docs = vec![
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                Some("task-b"),
                &[],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "epic",
                "active",
                "task-b",
                None,
                &["task-c"],
            ),
            make_doc(
                "tasks/c.md",
                "Task C",
                "task",
                "active",
                "task-c",
                None,
                &[],
            ),
        ];
        let g = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let err = g.would_create_hard_cycle("task-c", "task-a").unwrap_err();
        assert!(err.contains("circular dependency cycle"), "got: {err}");
    }

    #[test]
    fn test_hard_cycle_self_depends_on_rejected() {
        let docs = vec![make_doc(
            "tasks/a.md",
            "Task A",
            "task",
            "active",
            "task-a",
            None,
            &[],
        )];
        let g = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let err = g.would_create_hard_cycle("task-a", "task-a").unwrap_err();
        assert!(
            err.contains("cannot be its own parent or depend on itself"),
            "got: {err}"
        );
    }

    #[test]
    fn test_hard_cycle_dag_allowed() {
        // Diamond graph: A -> B, A -> C, B -> D, C -> D
        let docs = vec![
            make_doc(
                "tasks/a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                None,
                &["task-b", "task-c"],
            ),
            make_doc(
                "tasks/b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                None,
                &["task-d"],
            ),
            make_doc(
                "tasks/c.md",
                "Task C",
                "task",
                "active",
                "task-c",
                None,
                &["task-d"],
            ),
            make_doc(
                "tasks/d.md",
                "Task D",
                "task",
                "active",
                "task-d",
                None,
                &[],
            ),
        ];
        let g = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        assert!(g.would_create_hard_cycle("task-a", "task-d").is_ok());
    }

    #[test]
    fn test_target_ancestors_materialization() {
        let root = PathBuf::from("/tmp/pkb");
        let docs = vec![
            make_doc(
                "tasks/epic-1.md",
                "Epic 1",
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
                &[],
            ),
            // Multi-parent via contributes_to
            {
                let mut d = make_doc(
                    "tasks/task-b.md",
                    "Task B",
                    "task",
                    "active",
                    "task-b",
                    Some("epic-1"),
                    &[],
                );
                let mut fm = d.frontmatter.as_ref().unwrap().as_object().unwrap().clone();
                fm.insert(
                    "contributes_to".to_string(),
                    serde_json::json!([
                        {"to": "epic-2", "weight": "Probable", "why": "reason"}
                    ]),
                );
                d.frontmatter = Some(serde_json::Value::Object(fm));
                d
            },
            // task-c closes task-b
            {
                let mut d = make_doc(
                    "tasks/task-c.md",
                    "Task C",
                    "task",
                    "active",
                    "task-c",
                    None,
                    &[],
                );
                let mut fm = d.frontmatter.as_ref().unwrap().as_object().unwrap().clone();
                fm.insert("closes".to_string(), serde_json::json!(["task-b"]));
                d.frontmatter = Some(serde_json::Value::Object(fm));
                d
            },
        ];

        let store = GraphStore::build(&docs, &root);

        // epic-1 ancestors: []
        let n_epic1 = store.get_node("epic-1").expect("epic-1 should exist");
        assert!(
            n_epic1.target_ancestors.is_empty(),
            "epic-1 ancestors should be empty, got {:?}",
            n_epic1.target_ancestors
        );

        // task-a ancestors: ["epic-1"]
        let n_taska = store.get_node("task-a").expect("task-a should exist");
        assert_eq!(n_taska.target_ancestors, vec!["epic-1"]);

        // task-b ancestors: ["epic-1", "epic-2"] (epic-2 is a ghost)
        let n_taskb = store.get_node("task-b").expect("task-b should exist");
        assert_eq!(n_taskb.target_ancestors, vec!["epic-1", "epic-2"]);

        // ghost epic-2 should exist and have correct type (epic prefix coerces to task)
        let n_epic2 = store.get_node("epic-2").expect("epic-2 ghost should exist");
        assert_eq!(n_epic2.node_type.as_deref(), Some("task"));

        // task-c ancestors: ["epic-1", "epic-2", "task-b"]
        let n_taskc = store.get_node("task-c").expect("task-c should exist");
        assert_eq!(n_taskc.target_ancestors, vec!["epic-1", "epic-2", "task-b"]);
    }

    #[test]
    fn test_target_ancestors_cycles() {
        let root = PathBuf::from("/tmp/pkb");
        // A -> B -> A
        let docs = vec![
            make_doc(
                "tasks/task-a.md",
                "Task A",
                "task",
                "active",
                "task-a",
                Some("task-b"),
                &[],
            ),
            make_doc(
                "tasks/task-b.md",
                "Task B",
                "task",
                "active",
                "task-b",
                Some("task-a"),
                &[],
            ),
        ];

        let store = GraphStore::build(&docs, &root);
        let n_a = store.get_node("task-a").unwrap();
        let n_b = store.get_node("task-b").unwrap();

        assert_eq!(n_a.target_ancestors, vec!["task-b"]);
        assert_eq!(n_b.target_ancestors, vec!["task-a"]);
    }

    /// Parse-intolerance regression: a contributes_to entry missing `why:` must not
    /// silently drop the entire edge list (ContributesTo.justification has #[serde(default)]).
    /// Without that default, serde_json::from_value::<Vec<ContributesTo>> fails on the
    /// whole list and contributed_by on the target stays empty.
    #[test]
    fn test_contributes_to_missing_justification_still_populates_contributed_by() {
        let root = PathBuf::from("/tmp/pkb");
        let goal = make_doc(
            "goals/goal-g.md",
            "Goal G",
            "target",
            "active",
            "goal-g",
            None,
            &[],
        );
        // Entry has `to:` and `weight:` but deliberately omits `why:` / `justification:`.
        // Without #[serde(default)] on ContributesTo::justification, serde_json would return Err
        // and the whole Vec parse would silently become vec![], leaving contributed_by empty.
        let mut contributor = make_doc(
            "tasks/task-a.md",
            "Task A",
            "task",
            "active",
            "task-a",
            None,
            &[],
        );
        let mut fm = contributor
            .frontmatter
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        fm.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "goal-g", "weight": "Certain"}]),
        );
        contributor.frontmatter = Some(serde_json::Value::Object(fm));

        let store = GraphStore::build(&[goal, contributor], &root);

        let a = store.get_node("task-a").expect("task-a must exist");
        assert_eq!(
            a.contributes_to.len(),
            1,
            "entry with missing why: should still be parsed"
        );

        let g = store.get_node("goal-g").expect("goal-g must exist");
        assert!(
            g.contributed_by.contains(&"task-a".to_string()),
            "goal-g.contributed_by must contain task-a even when entry omits `why:`, got {:?}",
            g.contributed_by
        );
    }

    /// AC3 regression: task A contributes_to goal G → G.contributed_by must contain A.
    #[test]
    fn test_contributed_by_inverse_index_is_materialised() {
        let root = PathBuf::from("/tmp/pkb");

        // goal-g: a target with no own outbound edges
        let goal = make_doc(
            "goals/goal-g.md",
            "Goal G",
            "target",
            "active",
            "goal-g",
            None,
            &[],
        );

        // task-a and task-b both contribute_to goal-g
        let make_contributor = |path: &str, title: &str, id: &str| -> PkbDocument {
            let mut d = make_doc(path, title, "task", "active", id, None, &[]);
            let mut fm = d.frontmatter.as_ref().unwrap().as_object().unwrap().clone();
            fm.insert(
                "contributes_to".to_string(),
                serde_json::json!([{
                    "to": "goal-g",
                    "stated_weight": "Certain",
                    "justification": "test"
                }]),
            );
            d.frontmatter = Some(serde_json::Value::Object(fm));
            d
        };

        let docs = vec![
            goal,
            make_contributor("tasks/task-a.md", "Task A", "task-a"),
            make_contributor("tasks/task-b.md", "Task B", "task-b"),
        ];

        let store = GraphStore::build(&docs, &root);
        let g = store.get_node("goal-g").expect("goal-g must exist");

        // AC3: contributed_by inverse index must contain both contributors
        assert!(
            g.contributed_by.contains(&"task-a".to_string()),
            "goal-g.contributed_by should contain task-a, got {:?}",
            g.contributed_by
        );
        assert!(
            g.contributed_by.contains(&"task-b".to_string()),
            "goal-g.contributed_by should contain task-b, got {:?}",
            g.contributed_by
        );
    }

    #[test]
    fn test_compute_project_field_resolves_via_parent_ancestor() {
        // Verify the runtime resolution path: a task with no explicit `project`
        // field inherits the nearest parent-chain ancestor's explicit value.
        let mut fm_epic = serde_json::Map::new();
        fm_epic.insert("type".to_string(), serde_json::json!("epic"));
        fm_epic.insert("id".to_string(), serde_json::json!("epic-x"));
        fm_epic.insert("project".to_string(), serde_json::json!("proj-x"));
        let epic_node = PkbDocument {
            path: std::path::PathBuf::from("tasks/epic-x.md"),
            title: "Epic X".to_string(),
            body: String::new(),
            doc_type: Some("epic".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_epic)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        };
        let task_node = make_doc(
            "tasks/task-no-proj.md",
            "Task Without Project",
            "task",
            "ready",
            "task-no-proj",
            Some("epic-x"),
            &[],
        );
        let store = GraphStore::build(&[epic_node, task_node], Path::new("/tmp/test-pkb-proj"));
        let node = store
            .get_node("task-no-proj")
            .expect("task-no-proj should exist");
        assert!(
            node.project.is_some(),
            "task with no explicit project but an ancestor with one must have project resolved"
        );
        assert_eq!(
            node.project.as_deref(),
            Some("proj-x"),
            "resolved project label should be the ancestor's explicit slug"
        );
    }

    #[test]
    fn test_legacy_project_type_reads_as_epic() {
        // Legacy `type: project` files read-coerce to epic; their explicit
        // `project:` value (self-referential idiom) still inherits normally.
        let mut fm_proj = serde_json::Map::new();
        fm_proj.insert("type".to_string(), serde_json::json!("project"));
        fm_proj.insert("id".to_string(), serde_json::json!("tja-951bc29c"));
        fm_proj.insert("permalink".to_string(), serde_json::json!("tja"));
        fm_proj.insert("project".to_string(), serde_json::json!("tja"));

        let project_node = PkbDocument {
            path: std::path::PathBuf::from("projects/tja.md"),
            title: "Trans Journalists Association".to_string(),
            body: String::new(),
            doc_type: Some("project".to_string()),
            status: Some("active".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_proj)),
            content_hash: "test_hash".to_string(),
            file_hash: "test_hash".to_string(),
        };

        let task_node = make_doc(
            "tasks/task-tja.md",
            "Some TJA Task",
            "task",
            "ready",
            "task-tja",
            Some("tja-951bc29c"),
            &[],
        );

        let store = GraphStore::build(&[project_node, task_node], Path::new("/tmp/test-pkb-tja"));
        let container = store
            .get_node("tja-951bc29c")
            .expect("container should exist");
        assert_eq!(
            container.node_type.as_deref(),
            Some("task"),
            "legacy type: project must read-coerce to task"
        );
        let node = store.get_node("task-tja").expect("task-tja should exist");
        assert_eq!(
            node.project.as_deref(),
            Some("tja"),
            "child inherits the container's explicit project value"
        );
    }

    #[test]
    fn test_output_excalidraw_and_output_all_files() {
        let doc1 = make_doc(
            "tasks/task-a.md",
            "Task A",
            "task",
            "ready",
            "task-a",
            None,
            &["task-b"],
        );
        let doc2 = make_doc(
            "tasks/task-b.md",
            "Task B",
            "task",
            "ready",
            "task-b",
            None,
            &[],
        );

        let gs = GraphStore::build(&[doc1, doc2], Path::new("/tmp"));

        // Test output_excalidraw with ego focus
        let (ex_ego, _, _) = gs
            .output_excalidraw(Some("task-a"), 1)
            .expect("output ego excalidraw");
        assert!(ex_ego.contains("Task A"), "ego excalidraw contains Task A");

        // Test output_excalidraw without focus (all nodes)
        let (ex_all, _, _) = gs
            .output_excalidraw(None, 1)
            .expect("output all excalidraw");
        assert!(ex_all.contains("Task A"), "all excalidraw contains Task A");
        assert!(ex_all.contains("Task B"), "all excalidraw contains Task B");

        // Test output_all_files writes .excalidraw alongside .json and .graphml
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("export_test");
        let written = gs
            .output_all_files(base.to_str().unwrap())
            .expect("output all files");
        assert_eq!(written.len(), 3);
        assert!(written.iter().any(|p| p.ends_with(".json")));
        assert!(written.iter().any(|p| p.ends_with(".graphml")));
        assert!(written.iter().any(|p| p.ends_with(".excalidraw")));
        for p in &written {
            assert!(std::path::Path::new(p).exists(), "file must exist: {p}");
        }
    }

    // ── Phase 0a Defect Tests (D1, D2, D3) ──

    #[test]
    fn test_rebuild_determinism_across_entry_points() {
        let root = Path::new("/tmp/test-pkb");
        let empty_embeddings = HashMap::new();

        // Cold build from docs in different slice orders
        let mut fm_target = serde_json::Map::new();
        fm_target.insert("title".to_string(), serde_json::json!("Target Goal"));
        fm_target.insert("type".to_string(), serde_json::json!("goal"));
        fm_target.insert("id".to_string(), serde_json::json!("target-goal"));
        fm_target.insert("priority".to_string(), serde_json::json!(1));
        fm_target.insert("status".to_string(), serde_json::json!("ready"));

        let mut fm_contrib_a = serde_json::Map::new();
        fm_contrib_a.insert("title".to_string(), serde_json::json!("Contributor A"));
        fm_contrib_a.insert("type".to_string(), serde_json::json!("task"));
        fm_contrib_a.insert("id".to_string(), serde_json::json!("contrib-a"));
        fm_contrib_a.insert("priority".to_string(), serde_json::json!(2));
        fm_contrib_a.insert("status".to_string(), serde_json::json!("ready"));
        fm_contrib_a.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "target-goal", "weight": "probable"}]),
        );
        fm_contrib_a.insert("blocks".to_string(), serde_json::json!(["shared-leaf"]));

        let mut fm_contrib_b = serde_json::Map::new();
        fm_contrib_b.insert("title".to_string(), serde_json::json!("Contributor B"));
        fm_contrib_b.insert("type".to_string(), serde_json::json!("task"));
        fm_contrib_b.insert("id".to_string(), serde_json::json!("contrib-b"));
        fm_contrib_b.insert("priority".to_string(), serde_json::json!(3));
        fm_contrib_b.insert("status".to_string(), serde_json::json!("ready"));
        fm_contrib_b.insert(
            "contributes_to".to_string(),
            serde_json::json!([{"to": "target-goal", "weight": "uncertain"}]),
        );
        fm_contrib_b.insert(
            "soft_blocks".to_string(),
            serde_json::json!(["shared-leaf"]),
        );

        let mut fm_leaf = serde_json::Map::new();
        fm_leaf.insert("title".to_string(), serde_json::json!("Shared Leaf"));
        fm_leaf.insert("type".to_string(), serde_json::json!("task"));
        fm_leaf.insert("id".to_string(), serde_json::json!("shared-leaf"));
        fm_leaf.insert("priority".to_string(), serde_json::json!(0));
        fm_leaf.insert("status".to_string(), serde_json::json!("ready"));
        fm_leaf.insert("due".to_string(), serde_json::json!("2026-09-01"));
        fm_leaf.insert("effort".to_string(), serde_json::json!("1d"));

        let doc_target = PkbDocument {
            path: PathBuf::from("goals/target.md"),
            title: "Target Goal".to_string(),
            body: String::new(),
            doc_type: Some("goal".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_target)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };

        let doc_contrib_a = PkbDocument {
            path: PathBuf::from("tasks/contrib-a.md"),
            title: "Contributor A".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_contrib_a)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };

        let doc_contrib_b = PkbDocument {
            path: PathBuf::from("tasks/contrib-b.md"),
            title: "Contributor B".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_contrib_b)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };

        let doc_leaf = PkbDocument {
            path: PathBuf::from("tasks/shared-leaf.md"),
            title: "Shared Leaf".to_string(),
            body: String::new(),
            doc_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            consolidated: None,
            consolidated_at: None,
            modified: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm_leaf)),
            content_hash: "test".to_string(),
            file_hash: "test".to_string(),
        };

        let docs1 = vec![
            doc_target.clone(),
            doc_contrib_a.clone(),
            doc_contrib_b.clone(),
            doc_leaf.clone(),
        ];
        let docs2 = vec![
            doc_leaf.clone(),
            doc_contrib_b.clone(),
            doc_contrib_a.clone(),
            doc_target.clone(),
        ];

        let g_cold1 = GraphStore::build(&docs1, root);
        let g_cold2 = GraphStore::build(&docs2, root);

        for (id, n1) in &g_cold1.nodes {
            let n2 = g_cold2.nodes.get(id).expect("node must exist");
            assert_eq!(
                serde_json::to_value(&n1.flow).unwrap(),
                serde_json::to_value(&n2.flow).unwrap(),
                "cold build: flow figures differ for node {id}"
            );
        }

        // 1. rebuild_from_nodes_fast_with_embeddings (the live MCP server path)
        let map_cold1 = g_cold1.nodes_cloned();
        let map_cold2 = g_cold2.nodes_cloned();

        let g1_fast_emb = GraphStore::rebuild_from_nodes_fast_with_embeddings(
            map_cold1.clone(),
            root,
            &empty_embeddings,
        );
        let g2_fast_emb = GraphStore::rebuild_from_nodes_fast_with_embeddings(
            map_cold2.clone(),
            root,
            &empty_embeddings,
        );

        for (id, n1) in &g1_fast_emb.nodes {
            let n2 = g2_fast_emb.nodes.get(id).expect("node must exist");
            assert_eq!(
                serde_json::to_value(&n1.flow).unwrap(),
                serde_json::to_value(&n2.flow).unwrap(),
                "rebuild_from_nodes_fast_with_embeddings: flow figures differ for node {id}"
            );
        }

        // 2. rebuild_from_nodes (full rebuild with centrality)
        let g1_full = GraphStore::rebuild_from_nodes(map_cold1.clone(), root);
        let g2_full = GraphStore::rebuild_from_nodes(map_cold2.clone(), root);

        for (id, n1) in &g1_full.nodes {
            let n2 = g2_full.nodes.get(id).expect("node must exist");
            assert_eq!(
                serde_json::to_value(&n1.flow).unwrap(),
                serde_json::to_value(&n2.flow).unwrap(),
                "rebuild_from_nodes: flow figures differ for node {id}"
            );
        }

        // 3. rebuild_from_nodes_fast
        let g1_fast = GraphStore::rebuild_from_nodes_fast(map_cold1.clone(), root);
        let g2_fast = GraphStore::rebuild_from_nodes_fast(map_cold2.clone(), root);

        for (id, n1) in &g1_fast.nodes {
            let n2 = g2_fast.nodes.get(id).expect("node must exist");
            assert_eq!(
                serde_json::to_value(&n1.flow).unwrap(),
                serde_json::to_value(&n2.flow).unwrap(),
                "rebuild_from_nodes_fast: flow figures differ for node {id}"
            );
        }

        // 4. rebuild_from_nodes_skip_similarity
        let g1_skip = GraphStore::rebuild_from_nodes_skip_similarity(map_cold1, root, vec![]);
        let g2_skip = GraphStore::rebuild_from_nodes_skip_similarity(map_cold2, root, vec![]);

        for (id, n1) in &g1_skip.nodes {
            let n2 = g2_skip.nodes.get(id).expect("node must exist");
            assert_eq!(
                serde_json::to_value(&n1.flow).unwrap(),
                serde_json::to_value(&n2.flow).unwrap(),
                "rebuild_from_nodes_skip_similarity: flow figures differ for node {id}"
            );
        }
    }

    // ── output_dot (GraphViz DOT export) ──

    fn build_dot_test_graph() -> GraphStore {
        let docs = vec![
            make_doc(
                "tasks/epic-1.md",
                "Epic \"One\"",
                "epic",
                "active",
                "epic-1",
                None,
                &[],
            ),
            make_doc(
                "tasks/task-a.md",
                "Task A [[wikilink]] **bold**",
                "task",
                "active",
                "task-a",
                Some("epic-1"),
                &[],
            ),
            make_doc(
                "tasks/task-b.md",
                "Task B\nline two \\ with backslash and \"quotes\"",
                "task",
                "active",
                "task-b",
                None,
                &["task-a"],
            ),
            make_doc(
                "tasks/task-c.md",
                "Task C (done)",
                "task",
                "done",
                "task-c",
                None,
                &[],
            ),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    #[test]
    fn test_output_dot_basic_structure_and_edges() {
        let graph = build_dot_test_graph();
        let dot = graph.output_dot(None, 2, None, false);

        assert!(
            dot.starts_with("digraph PKB {"),
            "must open with digraph PKB {{, got: {dot}"
        );
        assert!(
            dot.trim_end().ends_with('}'),
            "must close with }}, got: {dot}"
        );
        assert!(dot.contains("\"task-a\""), "must contain task-a node id");
        assert!(
            dot.contains("\"task-a\" -> \"epic-1\""),
            "parent edge must run child -> parent"
        );
        assert!(
            dot.contains("\"task-b\" -> \"task-a\""),
            "depends_on edge must run dependent -> dependency"
        );
        // done task excluded by default (include_done=false)
        assert!(
            !dot.contains("\"task-c\""),
            "done task must be excluded when include_done=false"
        );
    }

    #[test]
    fn test_output_dot_include_done_true_includes_completed_nodes() {
        let graph = build_dot_test_graph();
        let dot = graph.output_dot(None, 2, None, true);
        assert!(
            dot.contains("\"task-c\""),
            "done task must be included when include_done=true"
        );
    }

    #[test]
    fn test_output_dot_focus_and_max_depth_limits_subgraph() {
        let graph = build_dot_test_graph();
        // task-a is 1 hop from epic-1; task-b depends on task-a (2 hops from epic-1)
        let dot = graph.output_dot(Some("epic-1"), 1, None, true);
        assert!(dot.contains("\"epic-1\""));
        assert!(dot.contains("\"task-a\""));
        assert!(
            !dot.contains("\"task-b\""),
            "task-b is 2 hops away, must be excluded at max_depth=1"
        );
    }

    #[test]
    fn test_output_dot_project_filter() {
        let mut docs = vec![make_doc_with_fields(
            "tasks/proj-a.md",
            "Project A Task",
            "task",
            "active",
            "proj-a-task",
            None,
            &[],
            None,
            &[],
        )];
        if let Some(fm) = docs[0].frontmatter.as_mut().and_then(|f| f.as_object_mut()) {
            fm.insert("project".to_string(), serde_json::json!("alpha"));
        }
        docs.push(make_doc(
            "tasks/proj-b.md",
            "Project B Task",
            "task",
            "active",
            "proj-b-task",
            None,
            &[],
        ));
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));

        let dot = graph.output_dot(None, 2, Some("alpha"), true);
        assert!(
            dot.contains("\"proj-a-task\""),
            "node in project 'alpha' must be included"
        );
        assert!(
            !dot.contains("\"proj-b-task\""),
            "node without matching project must be excluded"
        );
    }

    #[test]
    fn test_output_dot_empty_graph() {
        let graph = GraphStore::build(&[], Path::new("/tmp/test-pkb"));
        let dot = graph.output_dot(None, 2, None, true);
        assert_eq!(dot, "digraph PKB {\n  rankdir=LR;\n}\n");
    }

    #[test]
    fn test_output_dot_single_node_no_edges() {
        let docs = vec![make_doc(
            "tasks/solo.md",
            "Solo Task",
            "task",
            "active",
            "solo-task",
            None,
            &[],
        )];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let dot = graph.output_dot(None, 2, None, true);
        assert!(dot.contains("\"solo-task\""));
        assert!(
            !dot.contains("->"),
            "single node with no edges must have no edge lines"
        );
    }

    #[test]
    fn test_output_dot_escapes_special_characters_in_labels() {
        let graph = build_dot_test_graph();
        let dot = graph.output_dot(None, 2, None, true);

        // task-b's title contains a literal newline, a backslash, and double quotes —
        // none of these may appear unescaped inside a quoted DOT string, or the
        // output is not valid DOT syntax.
        for line in dot.lines() {
            if line.trim_start().starts_with("\"task-b\" [") {
                // Every quote inside the label must be preceded by a backslash,
                // and the line must not contain a raw (unescaped) newline — i.e.
                // this must be a single line of output.
                assert!(
                    !line.contains("\\\"\\\""),
                    "label quoting must be properly escaped, not doubled: {line}"
                );
                assert!(
                    line.contains("\\\\") || !line.contains('\\'),
                    "backslash in title must be escaped as \\\\\\\\: {line}"
                );
            }
        }
        // wikilink/markdown syntax in task-a's title must survive verbatim
        // inside the quoted label (not special in DOT, just needs quote-safety).
        assert!(
            dot.contains("[[wikilink]]"),
            "markdown/wikilink syntax must appear in output: {dot}"
        );
    }

    #[test]
    fn test_open_descendants_excludes_templates() {
        let docs = vec![
            make_doc(
                "tasks/parent.md",
                "Parent Task",
                "task",
                "ready",
                "task-parent",
                None,
                &[],
            ),
            make_doc(
                "tasks/child-task.md",
                "Child Task",
                "task",
                "ready",
                "task-child",
                Some("task-parent"),
                &[],
            ),
            make_doc(
                "tasks/child-template.md",
                "Child Template",
                "template",
                "ready",
                "tpl-child",
                Some("task-parent"),
                &[],
            ),
        ];
        let graph = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let descs = graph.open_descendants("task-parent");
        assert!(
            descs.contains(&"task-child".to_string()),
            "open child task must be in open_descendants"
        );
        assert!(
            !descs.contains(&"tpl-child".to_string()),
            "template child must not be in open_descendants"
        );
    }

    // ── Phase 3 Honest Uncertainty Tests ────────────────────────────────────

    #[test]
    fn test_phase3_elicited_confidence_verbal_anchors() {
        assert_eq!(crate::graph::parse_verbal_confidence("certain"), Some(1.00));
        assert_eq!(
            crate::graph::parse_verbal_confidence("almost certain"),
            Some(1.00)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("very probable"),
            Some(0.85)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("probable"),
            Some(0.85)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("highly likely"),
            Some(0.85)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("expected"),
            Some(0.75)
        );
        assert_eq!(crate::graph::parse_verbal_confidence("likely"), Some(0.75));
        assert_eq!(
            crate::graph::parse_verbal_confidence("fifty-fifty"),
            Some(0.50)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("even chance"),
            Some(0.50)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("uncertain"),
            Some(0.25)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("possible"),
            Some(0.25)
        );
        assert_eq!(crate::graph::parse_verbal_confidence("perhaps"), Some(0.25));
        assert_eq!(crate::graph::parse_verbal_confidence("maybe"), Some(0.25));
        assert_eq!(
            crate::graph::parse_verbal_confidence("improbable"),
            Some(0.15)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("unlikely"),
            Some(0.15)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("very unlikely"),
            Some(0.15)
        );
        assert_eq!(
            crate::graph::parse_verbal_confidence("impossible"),
            Some(0.00)
        );
        assert_eq!(crate::graph::parse_verbal_confidence("none"), Some(0.00));

        // Structured JSON object with mandatory prose why/justification
        let structured_obj = serde_json::json!({
            "value": "probable",
            "why": "Preliminary benchmarks show 85% pass rate under stress."
        });
        assert_eq!(crate::graph::parse_confidence(&structured_obj), Some(0.85));
    }

    #[test]
    fn test_phase3_ready_classification_unchanged() {
        assert!(
            !is_ready_status("inbox", false),
            "inbox without AC is not ready"
        );
        assert!(is_ready_status("inbox", true), "inbox with AC is ready");
        assert!(is_ready_status("ready", false), "ready without AC is ready");
        assert!(is_ready_status("ready", true), "ready with AC is ready");
        assert!(is_ready_status("queued", false), "queued is ready");
        assert!(!is_ready_status("done", true), "done is not ready");
    }

    // ── Anti-gaming regression tests (mem_b320cf42) ─────────────────────────
    //
    // Close the three confirmed gaming bypasses in the adversarial review at
    // mem_7b139fb6 (verdict: FAIL). Each test below reproduces the reviewer's
    // attack shape (1 subject node + 2 trivial downstream dependents, so
    // has_downstream_divergence is trivially satisfied and the only variable
    // under test is condition 1 / has_open_question).

    #[test]
    fn test_bypass1_heading_filler_and_settled_reorder_closed() {
        let root = Path::new("/tmp/test-bypass1");

        // Attack A (mem_7b139fb6): a heading matching the open-question prefix
        // set followed by a single filler word used to earn the full bonus with
        // zero real inquiry content.
        let mut filler_heading = make_doc(
            "tasks/filler-heading.md",
            "Filler heading spike",
            "task",
            "ready",
            "filler-heading",
            None,
            &[],
        );
        filler_heading.body = "## Questions\ntbd\n".to_string();

        let fh_dep_1 = make_doc(
            "tasks/fh-dep-1.md",
            "FH dep 1",
            "task",
            "ready",
            "fh-dep-1",
            None,
            &["filler-heading"],
        );
        let fh_dep_2 = make_doc(
            "tasks/fh-dep-2.md",
            "FH dep 2",
            "task",
            "ready",
            "fh-dep-2",
            None,
            &["filler-heading"],
        );

        // Attack F (mem_7b139fb6): the exact "question is settled" phrase typed
        // *inside* a matched Open Questions section used to be dead code, because
        // the section-scan short-circuit returned true before the settled check
        // ever ran — even though the section content otherwise reads as genuine
        // inquiry content (a real question, an "unclear" cue).
        let mut dead_settled_check = make_doc(
            "tasks/dead-settled.md",
            "Dead settled-check spike",
            "task",
            "ready",
            "dead-settled",
            None,
            &[],
        );
        dead_settled_check.body =
            "## Open Questions\nIs this still unclear? The question is settled now, no further work needed.\n"
                .to_string();

        let ds_dep_1 = make_doc(
            "tasks/ds-dep-1.md",
            "DS dep 1",
            "task",
            "ready",
            "ds-dep-1",
            None,
            &["dead-settled"],
        );
        let ds_dep_2 = make_doc(
            "tasks/ds-dep-2.md",
            "DS dep 2",
            "task",
            "ready",
            "ds-dep-2",
            None,
            &["dead-settled"],
        );

        let docs = vec![
            filler_heading,
            fh_dep_1,
            fh_dep_2,
            dead_settled_check,
            ds_dep_1,
            ds_dep_2,
        ];
        let graph = GraphStore::build(&docs, root);

        let fh_node = graph.get_node("filler-heading").unwrap();
        assert!(
            !fh_node.has_open_question,
            "a heading match plus one filler word must not count as an open question"
        );
        assert_eq!(
            fh_node.uncertainty, 0.0,
            "filler-heading spike must earn 0 uncertainty"
        );

        let ds_node = graph.get_node("dead-settled").unwrap();
        assert!(
            !ds_node.has_open_question,
            "an explicitly settled question inside a matched section must not count as open, regardless of scan order"
        );
        assert_eq!(
            ds_node.uncertainty, 0.0,
            "explicitly-settled spike must earn 0 uncertainty"
        );
    }

    #[test]
    fn test_bypass3_frontmatter_open_question_flag_does_not_bypass_body_detection() {
        let root = Path::new("/tmp/test-bypass3");

        // Attack J (mem_7b139fb6): a bare frontmatter `open_question: true` flag
        // with an empty body used to skip body detection entirely and earn the
        // full uncertainty bonus for zero body edits — the cheapest of the confirmed
        // gaming attacks.
        let mut frontmatter_flag = make_doc(
            "tasks/frontmatter-flag.md",
            "Frontmatter flag only",
            "task",
            "ready",
            "frontmatter-flag",
            None,
            &[],
        );
        {
            let mut fm = frontmatter_flag
                .frontmatter
                .as_ref()
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            fm.insert("open_question".to_string(), serde_json::json!(true));
            frontmatter_flag.frontmatter = Some(serde_json::Value::Object(fm));
        }
        // body stays empty.

        let ff_dep_1 = make_doc(
            "tasks/ff-dep-1.md",
            "FF dep 1",
            "task",
            "ready",
            "ff-dep-1",
            None,
            &["frontmatter-flag"],
        );
        let ff_dep_2 = make_doc(
            "tasks/ff-dep-2.md",
            "FF dep 2",
            "task",
            "ready",
            "ff-dep-2",
            None,
            &["frontmatter-flag"],
        );

        let docs = vec![frontmatter_flag, ff_dep_1, ff_dep_2];
        let graph = GraphStore::build(&docs, root);

        let ff_node = graph.get_node("frontmatter-flag").unwrap();
        assert!(
            !ff_node.has_open_question,
            "a bare frontmatter open_question flag with no body content must not set has_open_question"
        );
        assert_eq!(
            ff_node.uncertainty, 0.0,
            "a frontmatter-only open_question flag must earn 0 uncertainty"
        );
    }
}
