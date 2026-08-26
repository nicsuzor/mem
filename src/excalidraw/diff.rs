//! 3-Way Diff Engine for Excalidraw Canvas, Base Snapshots, and Live Graph.
//!
//! Enforces safety invariants:
//! - Non-destructive node removal: Removing a card from the canvas produces `removed_from_canvas`,
//!   NEVER deleting the markdown document on disk.
//! - Detects added tasks, title updates, status transitions, retargeted edges, and visual modifications.
//! - Detects 3-way conflicts between base snapshot, canvas edits, and concurrent live graph mutations.

use crate::excalidraw::reader::CanvasModel;
use crate::graph::EdgeType;
use crate::graph_store::GraphStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ===========================================================================
// Mutation Types
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddedNodeMutation {
    pub temp_id: String,
    pub title: String,
    pub node_type: String,
    pub status: String,
    pub priority: Option<i32>,
    pub parent: Option<String>,
    pub tags: Vec<String>,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdatedNodeMutation {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Option<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeMutation {
    pub source: String,
    pub target: String,
    pub edge_type: EdgeType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetargetedEdgeMutation {
    pub edge_type: EdgeType,
    pub old_source: String,
    pub old_target: String,
    pub new_source: String,
    pub new_target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisualMutation {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub stroke_color: String,
    pub background_color: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffConflict {
    pub node_id: String,
    pub field: String,
    pub base_value: Option<String>,
    pub live_value: Option<String>,
    pub canvas_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GraphDiff {
    pub added_nodes: Vec<AddedNodeMutation>,
    pub updated_nodes: Vec<UpdatedNodeMutation>,
    /// Removed from canvas layout ONLY (non-destructive; markdown files are NEVER deleted)
    pub removed_from_canvas: Vec<String>,
    pub added_edges: Vec<EdgeMutation>,
    pub removed_edges: Vec<EdgeMutation>,
    pub retargeted_edges: Vec<RetargetedEdgeMutation>,
    pub visual_mutations: Vec<VisualMutation>,
    pub conflicts: Vec<DiffConflict>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.updated_nodes.is_empty()
            && self.removed_from_canvas.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.retargeted_edges.is_empty()
            && self.visual_mutations.is_empty()
            && self.conflicts.is_empty()
    }
}

// ===========================================================================
// Base Snapshot Representation
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BaseSnapshot {
    pub nodes: HashMap<String, SnapshotNode>,
    pub edges: HashSet<EdgeMutation>,
}

impl BaseSnapshot {
    pub fn from_canvas(canvas: &CanvasModel) -> Self {
        let mut nodes = HashMap::new();
        for card in &canvas.cards {
            if let Some(ref nid) = card.node_id {
                nodes.insert(
                    nid.clone(),
                    SnapshotNode {
                        id: nid.clone(),
                        title: card.title.clone(),
                        status: card.status.clone(),
                        priority: card.priority,
                        parent: card.parent.clone(),
                        tags: card.tags.clone(),
                        x: card.x,
                        y: card.y,
                        stroke_color: Some(card.stroke_color.clone()),
                        background_color: Some(card.background_color.clone()),
                    },
                );
            }
        }
        let mut edges = HashSet::new();
        for arrow in &canvas.arrows {
            if let (Some(ref src), Some(ref tgt)) = (&arrow.source_node_id, &arrow.target_node_id) {
                edges.insert(EdgeMutation {
                    source: src.clone(),
                    target: tgt.clone(),
                    edge_type: arrow.edge_type.clone(),
                });
            }
        }
        BaseSnapshot { nodes, edges }
    }

    pub fn from_file(file: &crate::excalidraw::schema::ExcalidrawFile) -> Self {
        let canvas = crate::excalidraw::reader::CanvasReader::parse_file(file.clone());
        Self::from_canvas(&canvas)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotNode {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub parent: Option<String>,
    pub tags: Vec<String>,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
}

// ===========================================================================
// Diff Engine Implementation
// ===========================================================================

pub struct DiffEngine;

impl DiffEngine {
    /// Compute a 3-way diff between base snapshot (if present), live graph, and parsed canvas.
    pub fn compute_diff(
        base: Option<&BaseSnapshot>,
        live: &GraphStore,
        canvas: &CanvasModel,
    ) -> GraphDiff {
        let mut diff = GraphDiff::default();

        let mut canvas_node_ids: HashSet<String> = HashSet::new();

        // 1. Process Cards on Canvas
        for card in &canvas.cards {
            // Last-resort identity: a generator that names each rectangle after
            // the node it draws (the natural convention, and what our own
            // exporter should do) leaves the id recoverable from the element id
            // alone. Only accept it when it resolves to a live node, so a card
            // hand-drawn in Excalidraw — whose element id is a random nanoid —
            // is still correctly reported as an addition.
            let recovered = card.node_id.clone().or_else(|| {
                live.get_node(&card.element_id)
                    .map(|_| card.element_id.clone())
            });

            if recovered.is_none() {
                // New node added to canvas
                diff.added_nodes.push(AddedNodeMutation {
                    temp_id: card.element_id.clone(),
                    title: card.title.clone(),
                    node_type: card.node_type.clone().unwrap_or_else(|| "task".to_string()),
                    status: card.status.clone().unwrap_or_else(|| "inbox".to_string()),
                    priority: card.priority,
                    parent: card.parent.clone(),
                    tags: card.tags.clone(),
                    x: card.x,
                    y: card.y,
                });
                continue;
            }

            let node_id = recovered.expect("recovered id checked above");
            canvas_node_ids.insert(node_id.clone());

            let live_node = live.get_node(&node_id);

            if let Some(ln) = live_node {
                let mut title_update = None;
                let mut status_update = None;
                let mut priority_update = None;
                let mut parent_update = None;
                let mut tags_update = None;

                // Title check
                if !card.title.is_empty()
                    && card.title != ln.label
                    && card.title != node_id
                    && card.title != "Untitled"
                {
                    // Check for 3-way conflict
                    if let Some(base_snap) = base {
                        if let Some(bn) = base_snap.nodes.get(&node_id) {
                            if ln.label != bn.title && card.title != bn.title && ln.label != card.title {
                                diff.conflicts.push(DiffConflict {
                                    node_id: node_id.clone(),
                                    field: "title".to_string(),
                                    base_value: Some(bn.title.clone()),
                                    live_value: Some(ln.label.clone()),
                                    canvas_value: Some(card.title.clone()),
                                });
                            }
                        }
                    }
                    title_update = Some(card.title.clone());
                }

                // Status check
                if let Some(ref cs) = card.status {
                    if ln.status.as_deref() != Some(cs.as_str()) {
                        if let Some(base_snap) = base {
                            if let Some(bn) = base_snap.nodes.get(&node_id) {
                                if ln.status != bn.status && card.status != bn.status && ln.status != card.status {
                                    diff.conflicts.push(DiffConflict {
                                        node_id: node_id.clone(),
                                        field: "status".to_string(),
                                        base_value: bn.status.clone(),
                                        live_value: ln.status.clone(),
                                        canvas_value: card.status.clone(),
                                    });
                                }
                            }
                        }
                        status_update = Some(cs.clone());
                    }
                }

                // Priority check
                if card.priority.is_some() && card.priority != ln.priority {
                    priority_update = Some(card.priority);
                }

                // Parent check
                if card.parent.is_some() && card.parent != ln.parent {
                    parent_update = Some(card.parent.clone());
                }

                // Tags check
                let mut sorted_card_tags = card.tags.clone();
                sorted_card_tags.sort();
                let mut sorted_live_tags = ln.tags.clone();
                sorted_live_tags.sort();
                if sorted_card_tags != sorted_live_tags && !sorted_card_tags.is_empty() {
                    tags_update = Some(card.tags.clone());
                }

                if title_update.is_some()
                    || status_update.is_some()
                    || priority_update.is_some()
                    || parent_update.is_some()
                    || tags_update.is_some()
                {
                    diff.updated_nodes.push(UpdatedNodeMutation {
                        node_id: node_id.clone(),
                        title: title_update,
                        status: status_update,
                        priority: priority_update,
                        parent: parent_update,
                        tags: tags_update,
                    });
                }

                // Visual modifications (tracked when base snapshot is provided)
                if let Some(base_snap) = base {
                    if let Some(bn) = base_snap.nodes.get(&node_id) {
                        let pos_changed = (card.x - bn.x).abs() > 0.5 || (card.y - bn.y).abs() > 0.5;
                        let color_changed = bn.stroke_color.as_ref().map_or(false, |sc| sc != &card.stroke_color)
                            || bn.background_color.as_ref().map_or(false, |bg| bg != &card.background_color);

                        if pos_changed || color_changed {
                            diff.visual_mutations.push(VisualMutation {
                                node_id: node_id.clone(),
                                x: card.x,
                                y: card.y,
                                width: card.width,
                                height: card.height,
                                stroke_color: card.stroke_color.clone(),
                                background_color: card.background_color.clone(),
                            });
                        }
                    }
                }
            } else {
                // Node ID present on canvas card but missing from live graph -> Added
                diff.added_nodes.push(AddedNodeMutation {
                    temp_id: card.element_id.clone(),
                    title: card.title.clone(),
                    node_type: card.node_type.clone().unwrap_or_else(|| "task".to_string()),
                    status: card.status.clone().unwrap_or_else(|| "inbox".to_string()),
                    priority: card.priority,
                    parent: card.parent.clone(),
                    tags: card.tags.clone(),
                    x: card.x,
                    y: card.y,
                });
            }
        }

        // 2. Check for Nodes Removed from Canvas (NON-DESTRUCTIVE: never deleted from disk)
        if let Some(base_snap) = base {
            for base_id in base_snap.nodes.keys() {
                if !canvas_node_ids.contains(base_id) {
                    diff.removed_from_canvas.push(base_id.clone());
                }
            }
        }

        // 3. Compare Edges
        let mut canvas_edges: HashSet<EdgeMutation> = HashSet::new();
        for arrow in &canvas.arrows {
            if let (Some(ref s), Some(ref t)) = (&arrow.source_node_id, &arrow.target_node_id) {
                canvas_edges.insert(EdgeMutation {
                    source: s.clone(),
                    target: t.clone(),
                    edge_type: arrow.edge_type.clone(),
                });
            }
        }

        let mut live_edges: HashSet<EdgeMutation> = HashSet::new();
        for edge in live.edges() {
            if canvas_node_ids.contains(&edge.source) && canvas_node_ids.contains(&edge.target) {
                live_edges.insert(EdgeMutation {
                    source: edge.source.clone(),
                    target: edge.target.clone(),
                    edge_type: edge.edge_type.clone(),
                });
            }
        }

        // Added edges
        for ce in &canvas_edges {
            if !live_edges.contains(ce) {
                diff.added_edges.push(ce.clone());
            }
        }

        // Removed edges (present in live induced graph or base, but removed from canvas)
        if let Some(base_snap) = base {
            for be in &base_snap.edges {
                if canvas_node_ids.contains(&be.source)
                    && canvas_node_ids.contains(&be.target)
                    && !canvas_edges.contains(be)
                {
                    diff.removed_edges.push(be.clone());
                }
            }
        }

        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excalidraw::schema::ExcalidrawElement;
    use crate::graph::GraphNode;

    #[test]
    fn test_diff_non_destructive_node_removal() {
        let mut base_snap = BaseSnapshot::default();
        base_snap.nodes.insert(
            "task-1".to_string(),
            SnapshotNode {
                id: "task-1".to_string(),
                title: "Task One".to_string(),
                status: Some("ready".to_string()),
                priority: Some(1),
                parent: None,
                tags: vec![],
                x: 100.0,
                y: 100.0,
                stroke_color: None,
                background_color: None,
            },
        );

        let live = GraphStore::build(&[], std::path::Path::new("/tmp"));
        let canvas = CanvasModel {
            cards: vec![], // Removed from canvas
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(Some(&base_snap), &live, &canvas);
        assert_eq!(diff.removed_from_canvas, vec!["task-1"]);
        assert!(diff.added_nodes.is_empty());
    }

    #[test]
    fn test_diff_detected_title_and_status_update() {
        let mut live_node = GraphNode::default();
        live_node.id = "task-alpha".to_string();
        live_node.label = "Old Title".to_string();
        live_node.status = Some("inbox".to_string());

        let mut live = GraphStore::build(&[], std::path::Path::new("/tmp"));
        live.replace_node(live_node);

        let card = crate::excalidraw::reader::CanvasCard {
            element_id: "elem-1".to_string(),
            node_id: Some("task-alpha".to_string()),
            title: "New Title".to_string(),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            priority: Some(1),
            parent: None,
            tags: vec![],
            frame_id: None,
            x: 200.0,
            y: 300.0,
            width: 240.0,
            height: 84.0,
            stroke_color: "#2b8a3e".to_string(),
            background_color: "#d3f9d8".to_string(),
            custom_data: None,
            raw_card_element: ExcalidrawElement::default(),
            bound_text_element: None,
            is_new: false,
            is_duplicate: false,
        };

        let canvas = CanvasModel {
            cards: vec![card],
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(None, &live, &canvas);
        assert_eq!(diff.updated_nodes.len(), 1);
        let update = &diff.updated_nodes[0];
        assert_eq!(update.node_id, "task-alpha");
        assert_eq!(update.title.as_deref(), Some("New Title"));
        assert_eq!(update.status.as_deref(), Some("ready"));
    }

    #[test]
    fn test_diff_3way_conflict_detection() {
        let mut base_snap = BaseSnapshot::default();
        base_snap.nodes.insert(
            "task-conflict".to_string(),
            SnapshotNode {
                id: "task-conflict".to_string(),
                title: "Base Title".to_string(),
                status: Some("inbox".to_string()),
                priority: None,
                parent: None,
                tags: vec![],
                x: 100.0,
                y: 100.0,
                stroke_color: None,
                background_color: None,
            },
        );

        // Live graph had title edited to "Live Title"
        let mut live_node = GraphNode::default();
        live_node.id = "task-conflict".to_string();
        live_node.label = "Live Title".to_string();
        live_node.status = Some("inbox".to_string());

        let mut live = GraphStore::build(&[], std::path::Path::new("/tmp"));
        live.replace_node(live_node);

        // Canvas concurrently edited title to "Canvas Title"
        let card = crate::excalidraw::reader::CanvasCard {
            element_id: "elem-c".to_string(),
            node_id: Some("task-conflict".to_string()),
            title: "Canvas Title".to_string(),
            node_type: Some("task".to_string()),
            status: Some("inbox".to_string()),
            priority: None,
            parent: None,
            tags: vec![],
            frame_id: None,
            x: 100.0,
            y: 100.0,
            width: 240.0,
            height: 84.0,
            stroke_color: "#1e1e1e".to_string(),
            background_color: "transparent".to_string(),
            custom_data: None,
            raw_card_element: ExcalidrawElement::default(),
            bound_text_element: None,
            is_new: false,
            is_duplicate: false,
        };

        let canvas = CanvasModel {
            cards: vec![card],
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(Some(&base_snap), &live, &canvas);
        assert_eq!(diff.conflicts.len(), 1, "Must detect 3-way title conflict");
        let conflict = &diff.conflicts[0];
        assert_eq!(conflict.field, "title");
        assert_eq!(conflict.base_value.as_deref(), Some("Base Title"));
        assert_eq!(conflict.live_value.as_deref(), Some("Live Title"));
        assert_eq!(conflict.canvas_value.as_deref(), Some("Canvas Title"));
    }

    #[test]
    fn test_diff_added_and_removed_edges() {
        let mut base_snap = BaseSnapshot::default();
        base_snap.nodes.insert(
            "task-x".to_string(),
            SnapshotNode {
                id: "task-x".to_string(),
                title: "X".to_string(),
                status: None,
                priority: None,
                parent: None,
                tags: vec![],
                x: 100.0,
                y: 100.0,
                stroke_color: None,
                background_color: None,
            },
        );
        base_snap.nodes.insert(
            "task-y".to_string(),
            SnapshotNode {
                id: "task-y".to_string(),
                title: "Y".to_string(),
                status: None,
                priority: None,
                parent: None,
                tags: vec![],
                x: 200.0,
                y: 100.0,
                stroke_color: None,
                background_color: None,
            },
        );
        base_snap.edges.insert(EdgeMutation {
            source: "task-x".to_string(),
            target: "task-y".to_string(),
            edge_type: EdgeType::DependsOn,
        });

        let mut live = GraphStore::build(&[], std::path::Path::new("/tmp"));
        let mut nx = GraphNode::default();
        nx.id = "task-x".to_string();
        let mut ny = GraphNode::default();
        ny.id = "task-y".to_string();
        live.replace_node(nx);
        live.replace_node(ny);

        // Canvas removed the DependsOn edge and added a ContributesTo edge
        let card_x = crate::excalidraw::reader::CanvasCard {
            element_id: "elem-x".to_string(),
            node_id: Some("task-x".to_string()),
            title: "X".to_string(),
            node_type: Some("task".to_string()),
            status: None,
            priority: None,
            parent: None,
            tags: vec![],
            frame_id: None,
            x: 100.0,
            y: 100.0,
            width: 240.0,
            height: 84.0,
            stroke_color: "#1e1e1e".to_string(),
            background_color: "transparent".to_string(),
            custom_data: None,
            raw_card_element: ExcalidrawElement::default(),
            bound_text_element: None,
            is_new: false,
            is_duplicate: false,
        };
        let card_y = crate::excalidraw::reader::CanvasCard {
            element_id: "elem-y".to_string(),
            node_id: Some("task-y".to_string()),
            title: "Y".to_string(),
            node_type: Some("task".to_string()),
            status: None,
            priority: None,
            parent: None,
            tags: vec![],
            frame_id: None,
            x: 200.0,
            y: 100.0,
            width: 240.0,
            height: 84.0,
            stroke_color: "#1e1e1e".to_string(),
            background_color: "transparent".to_string(),
            custom_data: None,
            raw_card_element: ExcalidrawElement::default(),
            bound_text_element: None,
            is_new: false,
            is_duplicate: false,
        };

        let arrow = crate::excalidraw::reader::CanvasArrow {
            element_id: "arrow-1".to_string(),
            source_node_id: Some("task-x".to_string()),
            target_node_id: Some("task-y".to_string()),
            source_element_id: Some("elem-x".to_string()),
            target_element_id: Some("elem-y".to_string()),
            edge_type: EdgeType::ContributesTo,
            label: None,
            custom_data: None,
            raw_arrow_element: ExcalidrawElement::default(),
        };

        let canvas = CanvasModel {
            cards: vec![card_x, card_y],
            arrows: vec![arrow],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(Some(&base_snap), &live, &canvas);
        assert_eq!(diff.removed_edges.len(), 1);
        assert_eq!(diff.removed_edges[0].edge_type, EdgeType::DependsOn);
        assert_eq!(diff.added_edges.len(), 1);
        assert_eq!(diff.added_edges[0].edge_type, EdgeType::ContributesTo);
    }

    /// A generator that names each rectangle after the node it draws leaves the
    /// identity recoverable from the element id. Without this the whole canvas
    /// diffs as additions and sync would propose duplicating the graph.
    #[test]
    fn element_id_recovers_identity_when_it_resolves_to_a_live_node() {
        fn card(element_id: &str, title: &str) -> crate::excalidraw::reader::CanvasCard {
            crate::excalidraw::reader::CanvasCard {
                element_id: element_id.to_string(),
                node_id: None,          // no customData, and no id in the card text
                title: title.to_string(),
                node_type: None,
                status: None,
                priority: None,
                parent: None,
                tags: vec![],
                frame_id: None,
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 80.0,
                stroke_color: String::new(),
                background_color: String::new(),
                custom_data: None,
                raw_card_element: Default::default(),
                bound_text_element: None,
                is_new: true,
                is_duplicate: false,
            }
        }

        let mut gs = GraphStore::build(&[], std::path::Path::new("/tmp"));
        let mut n = GraphNode::default();
        n.id = "task-48234949".to_string();
        n.label = "Finish LED strip above sink".to_string();
        gs.replace_node(n);

        let canvas = CanvasModel {
            cards: vec![
                card("task-48234949", "Finish LED strip above sink"), // named after its node
                card("Xk3p9qZr", "Something brand new"),              // hand-drawn nanoid
            ],
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(None, &gs, &canvas);
        assert_eq!(
            diff.added_nodes.len(),
            1,
            "only the genuinely new card is an addition"
        );
        assert_eq!(diff.added_nodes[0].temp_id, "Xk3p9qZr");
    }

    /// When a card's identity is recovered via element_id without bound text,
    /// reader sets title to "Untitled". DiffEngine must NOT propose updating
    /// the live node's title to "Untitled".
    #[test]
    fn element_id_recovery_without_title_does_not_overwrite_live_label_with_untitled() {
        let mut gs = GraphStore::build(&[], std::path::Path::new("/tmp"));
        let mut n = GraphNode::default();
        n.id = "task-12345678".to_string();
        n.label = "Important Existing Task".to_string();
        gs.replace_node(n);

        let canvas = CanvasModel {
            cards: vec![crate::excalidraw::reader::CanvasCard {
                element_id: "task-12345678".to_string(),
                node_id: None,
                title: "Untitled".to_string(), // placeholder from reader
                node_type: None,
                status: None,
                priority: None,
                parent: None,
                tags: vec![],
                frame_id: None,
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 80.0,
                stroke_color: String::new(),
                background_color: String::new(),
                custom_data: None,
                raw_card_element: Default::default(),
                bound_text_element: None,
                is_new: true,
                is_duplicate: false,
            }],
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(None, &gs, &canvas);
        assert!(
            diff.added_nodes.is_empty(),
            "element_id matches live node, so not an addition"
        );
        assert!(
            diff.updated_nodes.is_empty(),
            "placeholder Untitled must not update live node title"
        );
    }

    /// When a card's identity is recovered via element_id AND it has genuine
    /// bound text with a new title, DiffEngine SHOULD propose a title update.
    #[test]
    fn element_id_recovery_with_custom_title_updates_label() {
        let mut gs = GraphStore::build(&[], std::path::Path::new("/tmp"));
        let mut n = GraphNode::default();
        n.id = "task-12345678".to_string();
        n.label = "Old Title".to_string();
        gs.replace_node(n);

        let canvas = CanvasModel {
            cards: vec![crate::excalidraw::reader::CanvasCard {
                element_id: "task-12345678".to_string(),
                node_id: None,
                title: "Updated Node Title".to_string(),
                node_type: None,
                status: None,
                priority: None,
                parent: None,
                tags: vec![],
                frame_id: None,
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 80.0,
                stroke_color: String::new(),
                background_color: String::new(),
                custom_data: None,
                raw_card_element: Default::default(),
                bound_text_element: None,
                is_new: true,
                is_duplicate: false,
            }],
            arrows: vec![],
            frames: vec![],
            annotations: vec![],
            duplicate_ids: vec![],
            raw_file: Default::default(),
        };

        let diff = DiffEngine::compute_diff(None, &gs, &canvas);
        assert_eq!(diff.updated_nodes.len(), 1);
        assert_eq!(
            diff.updated_nodes[0].title.as_deref(),
            Some("Updated Node Title")
        );
    }
}
