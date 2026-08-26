//! Canvas-Live Graph Merge Engine, Archimedean Spiral Placement, Cycle Validation, and Disk Sync.
//!
//! Provides:
//! - Strict preservation of user coordinates $(x, y)$ and custom styling
//! - Bounded Archimedean spiral probe (max 3x node dimension radius) for placing newly discovered backend nodes
//! - Global dependency cycle validation against live [`GraphStore`]
//! - Batch frontmatter synchronization under file locks via [`crate::document_crud`]

use crate::document_crud;
use crate::excalidraw::diff::GraphDiff;
use crate::excalidraw::reader::{CanvasCard, CanvasReader};
use crate::excalidraw::schema::*;
use crate::graph::{EdgeType, GraphNode};
use crate::graph_store::GraphStore;
use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

// ===========================================================================
// Sync Report
// ===========================================================================

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub created_nodes: Vec<(String, PathBuf)>, // (node_id, file_path)
    pub updated_nodes: Vec<String>,            // node_ids updated
    pub updated_edges: usize,
    pub rejected_cycles: Vec<String>, // edges rejected due to cycle
    pub warnings: Vec<String>,
}

// ===========================================================================
// Cycle Validation
// ===========================================================================

/// Validate that adding a directed dependency or parent edge `source -> target` will not create a cycle.
///
/// In PKB dependency semantics: `source` depends on `target` (target must finish before source).
/// If there is already a dependency path from `source` to `target`, adding `source depends on target`
/// would complete a cycle (`source -> target -> ... -> source`).
pub fn validate_no_cycle(
    gs: &GraphStore,
    source: &str,
    target: &str,
    edge_type: &EdgeType,
) -> Result<(), Vec<String>> {
    if !matches!(edge_type, EdgeType::DependsOn | EdgeType::Parent) {
        return Ok(()); // Soft dependencies and links cannot form hard blocking cycles
    }

    if source == target {
        return Err(vec![source.to_string(), target.to_string()]);
    }

    // Check if a directed path already exists from source to target in DependsOn/Parent subgraph
    // Adjacency: u depends on v -> edge u -> v
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in gs.edges() {
        if matches!(edge.edge_type, EdgeType::DependsOn | EdgeType::Parent) {
            adj.entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        }
    }
    for node in gs.nodes() {
        for dep in &node.depends_on {
            adj.entry(node.id.as_str()).or_default().push(dep.as_str());
        }
        if let Some(ref p) = node.parent {
            adj.entry(node.id.as_str()).or_default().push(p.as_str());
        }
    }

    // BFS from target to source
    let mut queue = VecDeque::new();
    let mut visited: HashMap<&str, Option<&str>> = HashMap::new(); // current -> parent in search
    visited.insert(target, None);
    queue.push_back(target);

    while let Some(curr) = queue.pop_front() {
        if curr == source {
            // Cycle detected! Reconstruct path
            let mut path = Vec::new();
            let mut step = Some(source);
            while let Some(node) = step {
                path.push(node.to_string());
                step = visited.get(node).copied().flatten();
            }
            path.reverse();
            path.push(target.to_string());
            return Err(path);
        }

        if let Some(neighbors) = adj.get(curr) {
            for &next in neighbors {
                if !visited.contains_key(next) {
                    visited.insert(next, Some(curr));
                    queue.push_back(next);
                }
            }
        }
    }

    Ok(())
}

// ===========================================================================
// Archimedean Spiral Probe Placement
// ===========================================================================

/// Bounded Archimedean spiral probe algorithm (max 3x node dimension search radius).
/// Finds the nearest collision-free position around $(x_0, y_0)$.
pub fn find_spiral_placement(
    center_x: f64,
    center_y: f64,
    card_w: f64,
    card_h: f64,
    occupied_boxes: &[[f64; 4]], // [min_x, min_y, max_x, max_y]
) -> (f64, f64) {
    let max_radius = 3.0 * card_w.max(card_h); // e.g. 720 px for 240px card
    let b = 18.0; // radial pitch factor
    let mut theta = 0.0;
    let delta_theta = 0.35; // radians per probe step
    let margin = 24.0;

    while theta * b <= max_radius {
        let r = b * theta;
        let candidate_x = center_x + r * theta.cos() - card_w * 0.5;
        let candidate_y = center_y + r * theta.sin() - card_h * 0.5;

        let cand_min_x = candidate_x - margin;
        let cand_min_y = candidate_y - margin;
        let cand_max_x = candidate_x + card_w + margin;
        let cand_max_y = candidate_y + card_h + margin;

        let mut collides = false;
        for b in occupied_boxes {
            if cand_min_x < b[2] && cand_max_x > b[0] && cand_min_y < b[3] && cand_max_y > b[1] {
                collides = true;
                break;
            }
        }

        if !collides {
            return (candidate_x, candidate_y);
        }

        theta += delta_theta;
    }

    // Fallback: place to the right of the rightmost element
    let max_x = occupied_boxes
        .iter()
        .map(|b| b[2])
        .fold(center_x, f64::max);
    (max_x + margin + 60.0, center_y)
}

// ===========================================================================
// Canvas & Live Graph Merge
// ===========================================================================

/// Merge live graph updates into an existing canvas scene while strictly preserving user coordinates.
pub fn merge_live_into_canvas(
    existing_file: &ExcalidrawFile,
    live: &GraphStore,
    target_node_ids: &[String],
) -> ExcalidrawFile {
    let parsed_canvas = CanvasReader::parse_file(existing_file.clone());

    let mut elements_out: Vec<ExcalidrawElement> = Vec::new();
    let mut existing_card_map: HashMap<String, ExcalidrawElement> = HashMap::new();
    let mut existing_text_map: HashMap<String, ExcalidrawElement> = HashMap::new();
    let mut existing_positions: HashMap<String, (f64, f64)> = HashMap::new();
    let mut occupied_boxes: Vec<[f64; 4]> = Vec::new();

    let mut canvas_card_map: HashMap<String, CanvasCard> = HashMap::new();
    for card in &parsed_canvas.cards {
        let node_id = card.node_id.clone().or_else(|| {
            live.get_node(&card.element_id).map(|_| card.element_id.clone())
        });
        if let Some(ref nid) = node_id {
            canvas_card_map.insert(nid.clone(), card.clone());
            existing_card_map.insert(nid.clone(), card.raw_card_element.clone());
            if let Some(ref bt) = card.bound_text_element {
                existing_text_map.insert(nid.clone(), bt.clone());
            }
            existing_positions.insert(nid.clone(), (card.x, card.y));
            occupied_boxes.push([
                card.x,
                card.y,
                card.x + card.width,
                card.y + card.height,
            ]);
        }
    }

    // Preserve user manual annotations, sticky notes, doodles
    for ann in &parsed_canvas.annotations {
        occupied_boxes.push([ann.x, ann.y, ann.x + ann.width, ann.y + ann.height]);
        elements_out.push(ann.clone());
    }

    // Preserve frames
    for frame in &parsed_canvas.frames {
        elements_out.push(frame.raw_frame_element.clone());
    }

    let mut target_set: HashSet<String> = target_node_ids.iter().cloned().collect();
    for card in &parsed_canvas.cards {
        let node_id = card.node_id.clone().or_else(|| {
            live.get_node(&card.element_id).map(|_| card.element_id.clone())
        });
        if let Some(ref nid) = node_id {
            target_set.insert(nid.clone());
        }
    }

    let mut card_elem_map: HashMap<String, String> = HashMap::new();

    for nid in &target_set {
        let live_node = live.get_node(nid);

        if let Some(mut existing_card) = existing_card_map.remove(nid) {
            // Existing node on canvas: PRESERVE EXACT $(x, y)$ COORDINATES AND STYLING!
            let card_id = existing_card.id.clone();
            card_elem_map.insert(nid.clone(), card_id.clone());

            let canvas_card = canvas_card_map.get(nid);
            let canvas_status = canvas_card.and_then(|c| c.status.as_deref());
            let live_status = live_node.and_then(|ln| ln.status.as_deref());

            let status_unchanged = match (canvas_status, live_status) {
                (Some(cs), Some(ls)) => cs.eq_ignore_ascii_case(ls),
                (None, None) => true,
                _ => false,
            };

            if let Some(ln) = live_node {
                if !status_unchanged {
                    // Status changed: apply the new status palette color
                    let color_style = node_color_style(ln.status.as_deref(), ln.node_type.as_deref());
                    existing_card.background_color = color_style.bg_color.to_string();
                    existing_card.stroke_color = color_style.stroke_color.to_string();
                    if let Some(ref mut cd) = existing_card.custom_data {
                        if let Some(ref mut pkb) = cd.pkb {
                            pkb.status = ln.status.clone();
                        }
                    }
                }
                // If status is unchanged, preserve user's custom background_color, stroke_color, stroke_width, roughness
            }

            elements_out.push(existing_card);

            if let Some(mut bound_text) = existing_text_map.remove(nid) {
                if let Some(ln) = live_node {
                    // Update text label to reflect current live title/status
                    let status_str = ln.status.as_deref().unwrap_or("inbox").to_uppercase();
                    let header = format!("[{}]", status_str);
                    let new_text = format!("{}\n{}", header, ln.label);
                    bound_text.text = Some(new_text.clone());
                    bound_text.original_text = Some(new_text);
                }
                elements_out.push(bound_text);
            }
        } else if let Some(ln) = live_node {
            // Newly discovered backend node: place using Archimedean spiral probe
            let center_x = occupied_boxes
                .first()
                .map(|b| b[0] + 100.0)
                .unwrap_or(200.0);
            let center_y = occupied_boxes
                .first()
                .map(|b| b[1] + 100.0)
                .unwrap_or(200.0);

            let (place_x, place_y) = find_spiral_placement(
                center_x,
                center_y,
                CARD_WIDTH,
                CARD_HEIGHT,
                &occupied_boxes,
            );

            occupied_boxes.push([
                place_x,
                place_y,
                place_x + CARD_WIDTH,
                place_y + CARD_HEIGHT,
            ]);
            existing_positions.insert(nid.clone(), (place_x, place_y));

            let card_id = format!("card-{}", nid);
            let text_id = format!("text-{}", nid);
            card_elem_map.insert(nid.clone(), card_id.clone());

            let color_style = node_color_style(ln.status.as_deref(), ln.node_type.as_deref());

            let mut card_elem = ExcalidrawElement::default();
            card_elem.id = card_id.clone();
            card_elem.element_type = "rectangle".to_string();
            card_elem.x = place_x;
            card_elem.y = place_y;
            card_elem.width = CARD_WIDTH;
            card_elem.height = CARD_HEIGHT;
            card_elem.background_color = color_style.bg_color.to_string();
            card_elem.stroke_color = color_style.stroke_color.to_string();
            card_elem.bound_elements = Some(vec![BoundElement {
                id: text_id.clone(),
                element_type: "text".to_string(),
            }]);
            card_elem.custom_data = Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some(nid.clone()),
                    node_type: ln.node_type.clone(),
                    status: ln.status.clone(),
                    priority: ln.priority,
                    parent: ln.parent.clone(),
                    is_pkb_managed: Some(true),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            });

            let status_str = ln.status.as_deref().unwrap_or("inbox").to_uppercase();
            let text_content = format!("[{}]\n{}", status_str, ln.label);

            let mut text_elem = ExcalidrawElement::default();
            text_elem.id = text_id;
            text_elem.element_type = "text".to_string();
            text_elem.container_id = Some(card_id);
            text_elem.text = Some(text_content.clone());
            text_elem.original_text = Some(text_content);
            text_elem.x = place_x + 8.0;
            text_elem.y = place_y + 8.0;
            text_elem.width = CARD_WIDTH - 16.0;
            text_elem.height = CARD_HEIGHT - 16.0;

            elements_out.push(card_elem);
            elements_out.push(text_elem);
        }
    }

    // Connect edges between placed nodes
    for edge in live.edges() {
        let (Some(&pos_s), Some(&pos_t)) = (
            existing_positions.get(&edge.source),
            existing_positions.get(&edge.target),
        ) else {
            continue;
        };
        let (Some(card_s), Some(card_t)) = (
            card_elem_map.get(&edge.source),
            card_elem_map.get(&edge.target),
        ) else {
            continue;
        };

        let arrow_id = format!(
            "arrow-{}-{}-{}",
            edge.source,
            edge.target,
            edge.edge_type.as_str()
        );

        let (stroke_color, stroke_style, stroke_width) = edge_color_style(edge.edge_type.as_str());

        let mut arrow = ExcalidrawElement::default();
        arrow.id = arrow_id;
        arrow.element_type = "arrow".to_string();
        arrow.x = pos_s.0 + CARD_WIDTH;
        arrow.y = pos_s.1 + CARD_HEIGHT * 0.5;
        arrow.width = (pos_t.0 - (pos_s.0 + CARD_WIDTH)).abs();
        arrow.height = (pos_t.1 - pos_s.1).abs();
        arrow.points = Some(vec![
            [0.0, 0.0],
            [pos_t.0 - (pos_s.0 + CARD_WIDTH), pos_t.1 - pos_s.1],
        ]);
        arrow.stroke_color = stroke_color.to_string();
        arrow.stroke_style = stroke_style.to_string();
        arrow.stroke_width = stroke_width;
        arrow.end_arrowhead = Some("arrow".to_string());
        arrow.start_binding = Some(PointBinding {
            element_id: card_s.clone(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: Some(PORT_OUT),
        });
        arrow.end_binding = Some(PointBinding {
            element_id: card_t.clone(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: Some(PORT_IN),
        });

        elements_out.push(arrow);
    }

    ExcalidrawFile {
        file_type: "excalidraw".to_string(),
        version: 2,
        source: existing_file.source.clone(),
        elements: elements_out,
        app_state: existing_file.app_state.clone(),
        files: existing_file.files.clone(),
    }
}

// ===========================================================================
// Batch Synchronization to Disk
// ===========================================================================

/// Synchronize canvas diff mutations to disk markdown files under file locks via [`document_crud`].
pub fn sync_diff_to_disk(
    pkb_root: &Path,
    gs: &mut GraphStore,
    diff: &GraphDiff,
    sync_edge_removals: bool,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();

    // 1. Process Added Nodes -> create markdown document files
    for added in &diff.added_nodes {
        let is_task = added.node_type == "task";
        let has_parent = added.parent.as_deref().map_or(false, |p| !p.is_empty());

        let result = if is_task && has_parent {
            let task_fields = document_crud::TaskFields {
                title: added.title.clone(),
                status: Some(added.status.clone()),
                priority: added.priority,
                parent: added.parent.clone(),
                tags: added.tags.clone(),
                ..Default::default()
            };
            document_crud::create_task(pkb_root, task_fields)
        } else {
            let doc_fields = document_crud::DocumentFields {
                title: added.title.clone(),
                doc_type: added.node_type.clone(),
                status: Some(added.status.clone()),
                priority: added.priority,
                parent: added.parent.clone(),
                tags: added.tags.clone(),
                ..Default::default()
            };
            document_crud::create_document(pkb_root, doc_fields)
        };

        match result {
            Ok(file_path) => {
                if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
                    report.created_nodes.push((stem.to_string(), file_path));
                }
            }
            Err(e) => {
                report
                    .warnings
                    .push(format!("Failed to create node '{}': {e}", added.title));
            }
        }
    }

    // 2. Process Updated Nodes -> patch frontmatter via update_document
    for updated in &diff.updated_nodes {
        let Some(live_node) = gs.get_node(&updated.node_id) else {
            report
                .warnings
                .push(format!("Node '{}' not found for update", updated.node_id));
            continue;
        };

        let file_path = if live_node.path.is_absolute() {
            live_node.path.clone()
        } else {
            pkb_root.join(&live_node.path)
        };

        let mut updates: HashMap<String, serde_json::Value> = HashMap::new();

        if let Some(ref title) = updated.title {
            updates.insert("title".to_string(), serde_json::Value::String(title.clone()));
        }
        if let Some(ref status) = updated.status {
            updates.insert(
                "status".to_string(),
                serde_json::Value::String(status.clone()),
            );
        }
        if let Some(ref prio_opt) = updated.priority {
            match prio_opt {
                Some(p) => updates.insert("priority".to_string(), serde_json::json!(p)),
                None => updates.insert("priority".to_string(), serde_json::Value::Null),
            };
        }
        if let Some(ref parent_opt) = updated.parent {
            match parent_opt {
                Some(pid) => {
                    updates.insert("parent".to_string(), serde_json::Value::String(pid.clone()))
                }
                None => updates.insert("parent".to_string(), serde_json::Value::Null),
            };
        }
        if let Some(ref tags) = updated.tags {
            updates.insert("tags".to_string(), serde_json::json!(tags));
        }

        if !updates.is_empty() {
            match document_crud::update_document(&file_path, updates) {
                Ok(()) => {
                    report.updated_nodes.push(updated.node_id.clone());
                }
                Err(e) => {
                    report.warnings.push(format!(
                        "Failed to update node '{}': {e}",
                        updated.node_id
                    ));
                }
            }
        }
    }

    // 3. Process Edge Mutations with Cycle Validation
    for edge in &diff.added_edges {
        // Validate cycle before applying
        if let Err(cycle_path) =
            validate_no_cycle(gs, &edge.source, &edge.target, &edge.edge_type)
        {
            let cycle_desc = cycle_path.join(" -> ");
            report.rejected_cycles.push(format!(
                "Rejected edge {} -> {}: cycle detected ({})",
                edge.source, edge.target, cycle_desc
            ));
            continue;
        }

        // Apply edge to source node frontmatter
        if let Some(source_node) = gs.get_node(&edge.source) {
            let file_path = if source_node.path.is_absolute() {
                source_node.path.clone()
            } else {
                pkb_root.join(&source_node.path)
            };

            let mut updates = HashMap::new();
            match edge.edge_type {
                EdgeType::DependsOn => {
                    let mut deps = source_node.depends_on.clone();
                    if !deps.contains(&edge.target) {
                        deps.push(edge.target.clone());
                    }
                    updates.insert("depends_on".to_string(), serde_json::json!(deps));
                }
                EdgeType::SoftDependsOn => {
                    let mut deps = source_node.soft_depends_on.clone();
                    if !deps.contains(&edge.target) {
                        deps.push(edge.target.clone());
                    }
                    updates.insert("soft_depends_on".to_string(), serde_json::json!(deps));
                }
                EdgeType::Parent => {
                    updates.insert(
                        "parent".to_string(),
                        serde_json::Value::String(edge.target.clone()),
                    );
                }
                _ => {}
            }

            if !updates.is_empty() {
                if let Err(e) = document_crud::update_document(&file_path, updates) {
                    report.warnings.push(format!(
                        "Failed to sync edge for node '{}': {e}",
                        edge.source
                    ));
                } else {
                    report.updated_edges += 1;
                }
            }
        }
    }

    // 4. Process Edge Removals (if opt-in enabled)
    if sync_edge_removals {
        for edge in &diff.removed_edges {
            if edge.edge_type != EdgeType::DependsOn {
                continue;
            }

            if let Some(source_node) = gs.get_node(&edge.source) {
                let file_path = if source_node.path.is_absolute() {
                    source_node.path.clone()
                } else {
                    pkb_root.join(&source_node.path)
                };

                let existing_deps: Vec<String> = if let Ok(content) = std::fs::read_to_string(&file_path) {
                    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
                    let result = matter.parse(&content);
                    result
                        .data
                        .as_ref()
                        .and_then(|d| d.deserialize::<serde_json::Value>().ok())
                        .and_then(|v| v.get("depends_on").cloned())
                        .and_then(|v| {
                            if let Some(arr) = v.as_array() {
                                Some(
                                    arr.iter()
                                        .filter_map(|x| x.as_str().map(String::from))
                                        .collect(),
                                )
                            } else if let Some(s) = v.as_str() {
                                Some(vec![s.to_string()])
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| source_node.depends_on.clone())
                } else {
                    source_node.depends_on.clone()
                };

                let new_deps: Vec<String> = existing_deps
                    .into_iter()
                    .filter(|d| {
                        let trimmed = d.trim();
                        let unbracketed = trimmed.trim_matches(&['[', ']'][..]).trim();
                        let target_md = format!("{}.md", edge.target);
                        !(trimmed == edge.target
                            || unbracketed == edge.target
                            || trimmed == target_md
                            || trimmed.ends_with(&format!("{}.md", edge.target))
                            || trimmed.ends_with(&format!("/{}", target_md)))
                    })
                    .collect();

                let mut updates = HashMap::new();
                updates.insert("depends_on".to_string(), serde_json::json!(new_deps));

                if let Err(e) = document_crud::update_document(&file_path, updates) {
                    report.warnings.push(format!(
                        "Failed to remove dependency edge for node '{}': {e}",
                        edge.source
                    ));
                } else {
                    report.updated_edges += 1;
                }
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spiral_placement_no_collision() {
        let occupied = vec![[100.0, 100.0, 340.0, 184.0]];
        let (x, y) = find_spiral_placement(200.0, 200.0, CARD_WIDTH, CARD_HEIGHT, &occupied);

        // Candidate must not overlap occupied box
        let cand_box = [x, y, x + CARD_WIDTH, y + CARD_HEIGHT];
        let collides = cand_box[0] < occupied[0][2]
            && cand_box[2] > occupied[0][0]
            && cand_box[1] < occupied[0][3]
            && cand_box[3] > occupied[0][1];

        assert!(!collides, "Spiral placement must avoid collision");
    }

    #[test]
    fn test_cycle_validation_rejects_cycle() {
        let mut gs = GraphStore::build(&[], Path::new("/tmp"));
        let mut n1 = GraphNode::default();
        n1.id = "task-a".to_string();
        n1.depends_on = vec!["task-b".to_string()];

        let mut n2 = GraphNode::default();
        n2.id = "task-b".to_string();

        gs.replace_node(n1);
        gs.replace_node(n2);

        // task-a depends on task-b. Trying to add task-b depends on task-a must fail.
        let res = validate_no_cycle(&gs, "task-b", "task-a", &EdgeType::DependsOn);
        assert!(res.is_err(), "Must reject circular dependency");
    }

    #[test]
    fn test_sync_diff_to_disk_creates_task() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = temp_dir.path();

        let mut gs = GraphStore::build(&[], pkb_root);

        let mut diff = GraphDiff::default();
        diff.added_nodes.push(crate::excalidraw::diff::AddedNodeMutation {
            temp_id: "elem-new".to_string(),
            title: "Newly Created Task".to_string(),
            node_type: "task".to_string(),
            status: "ready".to_string(),
            priority: Some(1),
            parent: None,
            tags: vec!["frontend".to_string()],
            x: 100.0,
            y: 100.0,
        });

        let report = sync_diff_to_disk(pkb_root, &mut gs, &diff, false).expect("sync to disk");
        assert_eq!(report.created_nodes.len(), 1, "Must report 1 created task");

        let (_, file_path) = &report.created_nodes[0];
        assert!(file_path.exists(), "Markdown file must exist on disk");
        let content = std::fs::read_to_string(file_path).unwrap();
        assert!(content.contains("Newly Created Task"));
        assert!(content.contains("ready"));
    }

    #[test]
    fn test_sync_edge_removals_flag_behavior() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let pkb_root = temp_dir.path();

        let task_file = pkb_root.join("task-a.md");
        std::fs::write(
            &task_file,
            "---\nid: task-a\ntitle: Task A\ntype: task\nstatus: ready\ndepends_on:\n  - task-b\n  - '[[task-c]]'\n  - tasks/task-d.md\n---\n# Task A\n",
        )
        .unwrap();

        let mut gs = GraphStore::build(&[], pkb_root);
        let mut node_a = GraphNode::default();
        node_a.id = "task-a".to_string();
        node_a.path = PathBuf::from("task-a.md");
        node_a.depends_on = vec!["task-b".to_string(), "task-c".to_string(), "task-d".to_string()];
        gs.replace_node(node_a);

        let mut diff = GraphDiff::default();
        diff.removed_edges.push(crate::excalidraw::diff::EdgeMutation {
            source: "task-a".to_string(),
            target: "task-b".to_string(),
            edge_type: EdgeType::DependsOn,
        });
        diff.removed_edges.push(crate::excalidraw::diff::EdgeMutation {
            source: "task-a".to_string(),
            target: "task-c".to_string(),
            edge_type: EdgeType::DependsOn,
        });

        // 1. sync_edge_removals = false (default) -> frontmatter depends_on must remain untouched
        let report = sync_diff_to_disk(pkb_root, &mut gs, &diff, false).unwrap();
        assert_eq!(report.updated_edges, 0);
        let content = std::fs::read_to_string(&task_file).unwrap();
        assert!(content.contains("task-b"));
        assert!(content.contains("task-c"));

        // 2. sync_edge_removals = true -> frontmatter depends_on must strip bare ID and wikilink
        let report = sync_diff_to_disk(pkb_root, &mut gs, &diff, true).unwrap();
        assert_eq!(report.updated_edges, 2);
        let content_after = std::fs::read_to_string(&task_file).unwrap();
        assert!(!content_after.contains("task-b"));
        assert!(!content_after.contains("task-c"));
        assert!(content_after.contains("task-d.md"));

        // 3. Remove filename reference tasks/task-d.md
        let mut diff_d = GraphDiff::default();
        diff_d.removed_edges.push(crate::excalidraw::diff::EdgeMutation {
            source: "task-a".to_string(),
            target: "task-d".to_string(),
            edge_type: EdgeType::DependsOn,
        });
        let report_d = sync_diff_to_disk(pkb_root, &mut gs, &diff_d, true).unwrap();
        assert_eq!(report_d.updated_edges, 1);
        let content_d = std::fs::read_to_string(&task_file).unwrap();
        assert!(!content_d.contains("task-d.md"));
    }

    #[test]
    fn test_merge_preserves_custom_card_styling_when_status_unchanged() {
        let mut file = ExcalidrawFile::default();
        let mut card = ExcalidrawElement::default();
        card.id = "card-task-1".to_string();
        card.element_type = "rectangle".to_string();
        card.background_color = "#ff00ff".to_string();
        card.stroke_color = "#00ff00".to_string();
        card.stroke_width = 3.0;
        card.roughness = 2.0;
        card.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some("task-1".to_string()),
                status: Some("ready".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        file.elements.push(card);

        let mut gs = GraphStore::build(&[], Path::new("/tmp"));
        let mut node = GraphNode::default();
        node.id = "task-1".to_string();
        node.label = "Task 1".to_string();
        node.node_type = Some("task".to_string());
        node.status = Some("ready".to_string());
        gs.replace_node(node);

        // Status unchanged: custom styling preserved
        let merged = merge_live_into_canvas(&file, &gs, &["task-1".to_string()]);
        let merged_card = merged.elements.iter().find(|e| e.id == "card-task-1").unwrap();
        assert_eq!(merged_card.background_color, "#ff00ff");
        assert_eq!(merged_card.stroke_color, "#00ff00");
        assert_eq!(merged_card.stroke_width, 3.0);
        assert_eq!(merged_card.roughness, 2.0);

        // Status changed: status palette applied
        let mut gs_updated = GraphStore::build(&[], Path::new("/tmp"));
        let mut node_done = GraphNode::default();
        node_done.id = "task-1".to_string();
        node_done.label = "Task 1".to_string();
        node_done.node_type = Some("task".to_string());
        node_done.status = Some("done".to_string());
        gs_updated.replace_node(node_done);

        let merged2 = merge_live_into_canvas(&file, &gs_updated, &["task-1".to_string()]);
        let merged_card2 = merged2.elements.iter().find(|e| e.id == "card-task-1").unwrap();
        let done_style = node_color_style(Some("done"), Some("task"));
        assert_eq!(merged_card2.background_color, done_style.bg_color);
        assert_eq!(merged_card2.stroke_color, done_style.stroke_color);
    }

    #[test]
    fn test_merge_preserves_card_identified_by_element_id() {
        let mut file = ExcalidrawFile::default();
        let mut card = ExcalidrawElement::default();
        card.id = "task-48234949".to_string(); // Named after the node, no customData
        card.element_type = "rectangle".to_string();
        card.x = 150.0;
        card.y = 250.0;
        file.elements.push(card);

        let mut gs = GraphStore::build(&[], Path::new("/tmp"));
        let mut node = GraphNode::default();
        node.id = "task-48234949".to_string();
        node.label = "Finish LED strip".to_string();
        node.node_type = Some("task".to_string());
        node.status = Some("ready".to_string());
        gs.replace_node(node);

        let merged = merge_live_into_canvas(&file, &gs, &["task-48234949".to_string()]);
        let matched = merged
            .elements
            .iter()
            .find(|e| e.id == "task-48234949")
            .unwrap();
        assert_eq!(matched.x, 150.0, "must preserve existing x position");
        assert_eq!(matched.y, 250.0, "must preserve existing y position");
    }
}
