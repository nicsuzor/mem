//! Excalidraw visual canvas integration for PKB.
//!
//! Provides two-way synchronization between PKB knowledge graphs and Excalidraw V2 scenes:
//! - [`schema`]: Full typed AST and unified color matrix for Excalidraw V2 JSON.
//! - [`layout`]: Ego-network extraction, Sugiyama layered DAG layout, Frame grouping, and port bindings.
//! - [`reader`]: 5-pass parser with container-bound text resolution, safe arrow typing, and duplicate ID detection.
//! - [`diff`]: 3-way diff between base snapshot, canvas, and live graph with non-destructive node removal.
//! - [`merge`]: Spiral placement for new nodes, cycle validation, and disk frontmatter sync.

pub mod diff;
pub mod layout;
pub mod merge;
pub mod reader;
pub mod schema;

pub use diff::{
    AddedNodeMutation, BaseSnapshot, DiffConflict, DiffEngine, EdgeMutation, GraphDiff,
    RetargetedEdgeMutation, SnapshotNode, UpdatedNodeMutation, VisualMutation,
};
pub use layout::{
    compute_sugiyama_layout, extract_ego_subgraph, generate_excalidraw_scene, LayoutConfig,
};
pub use merge::{
    find_spiral_placement, merge_live_into_canvas, sync_diff_to_disk, validate_no_cycle,
    SyncReport,
};
pub use reader::{CanvasArrow, CanvasCard, CanvasFrame, CanvasModel, CanvasReader};
pub use schema::{
    edge_color_style, node_color_style, AppState, BoundElement, CustomData, ElementColorStyle,
    ExcalidrawElement, ExcalidrawFile, PkbCustomData, PointBinding, Roundness, CARD_HEIGHT,
    CARD_WIDTH, FRAME_HEADER_HEIGHT, FRAME_PADDING, PORT_BOTTOM, PORT_IN, PORT_OUT, PORT_TOP,
};

use crate::graph::{Edge, GraphNode};
use crate::graph_store::GraphStore;
use anyhow::{Context, Result};
use std::path::Path;

/// Export an ego-network around a focus node to an [`ExcalidrawFile`].
pub fn export_ego_network(
    gs: &GraphStore,
    focus_id: &str,
    hops: usize,
    config: Option<LayoutConfig>,
) -> Result<ExcalidrawFile> {
    let (nodes, edges) = extract_ego_subgraph(gs, focus_id, hops);
    if nodes.is_empty() {
        anyhow::bail!("Focus node '{}' not found in graph", focus_id);
    }
    let cfg = config.unwrap_or_default();
    Ok(generate_excalidraw_scene(&nodes, &edges, &cfg))
}

/// Export an arbitrary subgraph of nodes to an [`ExcalidrawFile`].
pub fn export_subgraph(
    gs: &GraphStore,
    node_ids: &[String],
    config: Option<LayoutConfig>,
) -> Result<ExcalidrawFile> {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let id_set: std::collections::HashSet<&str> = node_ids.iter().map(|s| s.as_str()).collect();

    for nid in node_ids {
        if let Some(n) = gs.get_node(nid) {
            nodes.push(n.clone());
        }
    }

    let mut edges: Vec<Edge> = Vec::new();
    for edge in gs.edges() {
        if id_set.contains(edge.source.as_str()) && id_set.contains(edge.target.as_str()) {
            edges.push(edge.clone());
        }
    }

    let cfg = config.unwrap_or_default();
    Ok(generate_excalidraw_scene(&nodes, &edges, &cfg))
}

/// Parse an Excalidraw JSON string into a structured [`CanvasModel`].
pub fn parse_canvas(json_str: &str) -> Result<CanvasModel> {
    let file: ExcalidrawFile =
        serde_json::from_str(json_str).context("Failed to deserialize Excalidraw JSON")?;
    Ok(CanvasReader::parse_file(file))
}

/// Parse a base snapshot from JSON string (supports either serialized [`BaseSnapshot`] or [`ExcalidrawFile`]).
pub fn parse_base_snapshot(json_str: &str) -> Result<BaseSnapshot> {
    if let Ok(snap) = serde_json::from_str::<BaseSnapshot>(json_str) {
        return Ok(snap);
    }
    let canvas = parse_canvas(json_str)?;
    Ok(BaseSnapshot::from_canvas(&canvas))
}

/// Compute a 3-way diff between base snapshot (if known), live graph, and parsed canvas.
pub fn diff_canvas(
    base: Option<&BaseSnapshot>,
    live: &GraphStore,
    canvas: &CanvasModel,
) -> Result<GraphDiff> {
    Ok(DiffEngine::compute_diff(base, live, canvas))
}

/// Synchronize canvas mutations to markdown files on disk.
pub fn sync_canvas(
    pkb_root: &Path,
    gs: &mut GraphStore,
    diff: &GraphDiff,
) -> Result<SyncReport> {
    sync_diff_to_disk(pkb_root, gs, diff)
}

/// Merge live graph state into an existing Excalidraw file preserving user $(x, y)$ positions.
pub fn merge_canvas_with_live(
    existing_canvas: &ExcalidrawFile,
    live: &GraphStore,
    target_nodes: &[String],
) -> Result<ExcalidrawFile> {
    Ok(merge_live_into_canvas(existing_canvas, live, target_nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_level_export_and_diff_pipeline() {
        let mut gs = GraphStore::build(&[], Path::new("/tmp"));
        let mut n1 = GraphNode::default();
        n1.id = "task-1".to_string();
        n1.label = "Initial Task".to_string();
        n1.status = Some("ready".to_string());
        gs.replace_node(n1);

        // Export ego network
        let file = export_ego_network(&gs, "task-1", 1, None).expect("export ego network");
        assert_eq!(file.elements.len(), 2); // 1 card + 1 bound text

        // Serialize and parse
        let json = serde_json::to_string(&file).unwrap();
        let canvas = parse_canvas(&json).expect("parse canvas");
        assert_eq!(canvas.cards.len(), 1);

        // Diff against live
        let diff = diff_canvas(None, &gs, &canvas).expect("diff canvas");
        assert!(diff.is_empty(), "Initial diff should be empty");
    }
}
