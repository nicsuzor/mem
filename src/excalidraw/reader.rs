//! 5-Pass Excalidraw Canvas Reader and Parser.
//!
//! Robustly extracts PKB semantic models from Excalidraw V2 scenes:
//! - Pass 1: Element indexing & classification
//! - Pass 2: Container-bound text resolution & title/metadata parsing
//! - Pass 3: Frame hierarchy & grouping extraction
//! - Pass 4: Bound arrow resolution & safe typed prefix parsing (defaulting to `Link`)
//! - Pass 5: Duplicate ID detection (copy-paste guard) & non-PKB annotation preservation

use crate::excalidraw::schema::*;
use crate::graph::EdgeType;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static STATUS_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\[\s*([A-Za-z0-9_-]+)(?:\s*·\s*[Pp]?(\d+))?\s*\]\s*").unwrap()
});

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#([a-zA-Z0-9_\-]+)").unwrap());

// ===========================================================================
// Canvas Parsed Models
// ===========================================================================

/// A card on the canvas representing a task, note, epic, target, etc.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasCard {
    pub element_id: String,
    pub node_id: Option<String>,
    pub title: String,
    pub node_type: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub parent: Option<String>,
    pub tags: Vec<String>,
    pub frame_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub stroke_color: String,
    pub background_color: String,
    pub custom_data: Option<CustomData>,
    pub raw_card_element: ExcalidrawElement,
    pub bound_text_element: Option<ExcalidrawElement>,
    pub is_new: bool,
    pub is_duplicate: bool,
}

/// A directed arrow representing a relationship between two nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasArrow {
    pub element_id: String,
    pub source_node_id: Option<String>,
    pub target_node_id: Option<String>,
    pub source_element_id: Option<String>,
    pub target_element_id: Option<String>,
    pub edge_type: EdgeType,
    pub label: Option<String>,
    pub custom_data: Option<CustomData>,
    pub raw_arrow_element: ExcalidrawElement,
}

/// A spatial Frame element enclosing child cards.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasFrame {
    pub element_id: String,
    pub name: String,
    pub node_id: Option<String>,
    pub child_node_ids: Vec<String>,
    pub child_element_ids: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub raw_frame_element: ExcalidrawElement,
}

/// Complete parsed canvas model preserving PKB semantic items and user annotations.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasModel {
    pub cards: Vec<CanvasCard>,
    pub arrows: Vec<CanvasArrow>,
    pub frames: Vec<CanvasFrame>,
    /// User manual annotations, freehand doodles (`freedraw`), sticky notes, unattached shapes
    pub annotations: Vec<ExcalidrawElement>,
    pub duplicate_ids: Vec<String>,
    pub raw_file: ExcalidrawFile,
}

// ===========================================================================
// 5-Pass Parser Implementation
// ===========================================================================

pub struct CanvasReader;

impl CanvasReader {
    /// Parse raw JSON or [`ExcalidrawFile`] into a structured [`CanvasModel`].
    pub fn parse_file(file: ExcalidrawFile) -> CanvasModel {
        // Pass 1: Index elements by ID and classify
        let mut element_index: HashMap<String, ExcalidrawElement> = HashMap::new();
        let mut card_elements: Vec<ExcalidrawElement> = Vec::new();
        let mut text_elements: Vec<ExcalidrawElement> = Vec::new();
        let mut arrow_elements: Vec<ExcalidrawElement> = Vec::new();
        let mut frame_elements: Vec<ExcalidrawElement> = Vec::new();
        let mut user_annotations: Vec<ExcalidrawElement> = Vec::new();

        // Reverse map from bound text ID -> container ID
        let mut bound_to_container: HashMap<String, String> = HashMap::new();

        for elem in &file.elements {
            if elem.is_deleted {
                continue;
            }
            element_index.insert(elem.id.clone(), elem.clone());

            if let Some(ref bounds) = elem.bound_elements {
                for b in bounds {
                    if b.element_type == "text" {
                        bound_to_container.insert(b.id.clone(), elem.id.clone());
                    }
                }
            }
        }

        for elem in &file.elements {
            if elem.is_deleted {
                continue;
            }
            match elem.element_type.as_str() {
                "rectangle" | "diamond" | "ellipse" => {
                    // Check if card or user shape
                    if elem.custom_data.as_ref().map_or(false, |c| {
                        c.pkb.as_ref().map_or(false, |p| p.node_id.is_some())
                    }) || elem.bound_elements.as_ref().map_or(false, |b| {
                        b.iter().any(|x| x.element_type == "text")
                    }) {
                        card_elements.push(elem.clone());
                    } else {
                        // Unbound shape without PKB custom data -> user annotation
                        user_annotations.push(elem.clone());
                    }
                }
                "text" => {
                    text_elements.push(elem.clone());
                }
                "arrow" | "line" => {
                    arrow_elements.push(elem.clone());
                }
                "frame" | "magicframe" => {
                    frame_elements.push(elem.clone());
                }
                _ => {
                    // freedraw, image, etc.
                    user_annotations.push(elem.clone());
                }
            }
        }

        // Pass 2: Container-Bound Text Resolution
        let mut text_by_container: HashMap<String, ExcalidrawElement> = HashMap::new();
        let mut unattached_texts: Vec<ExcalidrawElement> = Vec::new();

        for text_elem in text_elements {
            let container_id = text_elem
                .container_id
                .as_ref()
                .cloned()
                .or_else(|| bound_to_container.get(&text_elem.id).cloned());

            if let Some(cid) = container_id {
                text_by_container.insert(cid, text_elem);
            } else {
                unattached_texts.push(text_elem);
            }
        }

        // Floating texts are treated as annotations
        user_annotations.extend(unattached_texts);

        let mut parsed_cards: Vec<CanvasCard> = Vec::new();
        let mut elem_to_node_id: HashMap<String, String> = HashMap::new();

        for card_elem in card_elements {
            let bound_text = text_by_container.get(&card_elem.id);
            let raw_text_content = bound_text
                .and_then(|t| t.text.as_deref().or(t.original_text.as_deref()))
                .unwrap_or("");

            // Parse text content for status, priority, title, tags
            let (status_from_text, priority_from_text, parsed_title, tags_from_text) =
                parse_card_text(raw_text_content);

            let custom_pkb = card_elem
                .custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref());

            let node_id = custom_pkb
                .and_then(|p| p.node_id.clone())
                .or_else(|| extract_id_from_text(raw_text_content));

            let node_type = custom_pkb
                .and_then(|p| p.node_type.clone())
                .or_else(|| {
                    if card_elem.element_type == "diamond" {
                        Some("target".to_string())
                    } else if card_elem.element_type == "ellipse" {
                        Some("area".to_string())
                    } else {
                        Some("task".to_string())
                    }
                });

            let status = custom_pkb
                .and_then(|p| p.status.clone())
                .or(status_from_text);

            let priority = custom_pkb
                .and_then(|p| p.priority)
                .or(priority_from_text);

            let parent = custom_pkb.and_then(|p| p.parent.clone());

            let mut tags = custom_pkb
                .map(|p| p.tags.clone())
                .unwrap_or_default();
            for t in tags_from_text {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }

            let title = if !parsed_title.is_empty() {
                parsed_title
            } else {
                node_id.clone().unwrap_or_else(|| "Untitled".to_string())
            };

            let is_new = custom_pkb.map_or(true, |p| p.node_id.is_none());

            if let Some(ref nid) = node_id {
                elem_to_node_id.insert(card_elem.id.clone(), nid.clone());
            }

            parsed_cards.push(CanvasCard {
                element_id: card_elem.id.clone(),
                node_id,
                title,
                node_type,
                status,
                priority,
                parent,
                tags,
                frame_id: card_elem.frame_id.clone(),
                x: card_elem.x,
                y: card_elem.y,
                width: card_elem.width,
                height: card_elem.height,
                stroke_color: card_elem.stroke_color.clone(),
                background_color: card_elem.background_color.clone(),
                custom_data: card_elem.custom_data.clone(),
                raw_card_element: card_elem,
                bound_text_element: bound_text.cloned(),
                is_new,
                is_duplicate: false,
            });
        }

        // Pass 3: Frame Hierarchy Resolution
        let mut parsed_frames: Vec<CanvasFrame> = Vec::new();
        for frame_elem in frame_elements {
            let name = frame_elem.name.clone().unwrap_or_else(|| "Frame".to_string());
            let node_id = frame_elem
                .custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref())
                .and_then(|p| p.node_id.clone())
                .or_else(|| extract_id_from_frame_name(&name));

            let mut child_element_ids = Vec::new();
            let mut child_node_ids = Vec::new();

            for card in &parsed_cards {
                if card.frame_id.as_deref() == Some(&frame_elem.id) {
                    child_element_ids.push(card.element_id.clone());
                    if let Some(ref nid) = card.node_id {
                        child_node_ids.push(nid.clone());
                    }
                }
            }

            parsed_frames.push(CanvasFrame {
                element_id: frame_elem.id.clone(),
                name,
                node_id,
                child_node_ids,
                child_element_ids,
                x: frame_elem.x,
                y: frame_elem.y,
                width: frame_elem.width,
                height: frame_elem.height,
                raw_frame_element: frame_elem,
            });
        }

        // Pass 4: Bound Arrow Resolution & Safe Arrow Typing
        let mut parsed_arrows: Vec<CanvasArrow> = Vec::new();
        for arrow_elem in arrow_elements {
            let start_elem_id = arrow_elem.start_binding.as_ref().map(|b| b.element_id.clone());
            let end_elem_id = arrow_elem.end_binding.as_ref().map(|b| b.element_id.clone());

            let source_node_id = start_elem_id.as_ref().and_then(|id| elem_to_node_id.get(id).cloned());
            let target_node_id = end_elem_id.as_ref().and_then(|id| elem_to_node_id.get(id).cloned());

            if source_node_id.is_none() || target_node_id.is_none() {
                user_annotations.push(arrow_elem);
                continue;
            }

            let label = arrow_elem
                .text
                .clone()
                .or_else(|| arrow_elem.original_text.clone());

            // Safe arrow typing: User-drawn arrows without typed prefix default to non-blocking EdgeType::Link
            let edge_type = parse_arrow_edge_type(
                arrow_elem
                    .custom_data
                    .as_ref()
                    .and_then(|c| c.pkb.as_ref())
                    .and_then(|p| p.edge_type.as_deref()),
                label.as_deref(),
            );

            parsed_arrows.push(CanvasArrow {
                element_id: arrow_elem.id.clone(),
                source_node_id,
                target_node_id,
                source_element_id: start_elem_id,
                target_element_id: end_elem_id,
                edge_type,
                label,
                custom_data: arrow_elem.custom_data.clone(),
                raw_arrow_element: arrow_elem,
            });
        }

        // Pass 5: Duplicate ID Detection & Flagging
        let mut seen_node_ids: HashSet<String> = HashSet::new();
        let mut duplicate_ids: Vec<String> = Vec::new();

        for card in &mut parsed_cards {
            if let Some(ref nid) = card.node_id {
                if !seen_node_ids.insert(nid.clone()) {
                    // Duplicate found (clipboard copy-pasted card)
                    duplicate_ids.push(nid.clone());
                    card.is_duplicate = true;
                    card.is_new = true;
                    // Reset node_id so sync generates a fresh ID instead of clobbering existing node
                    card.node_id = None;
                }
            }
        }

        CanvasModel {
            cards: parsed_cards,
            arrows: parsed_arrows,
            frames: parsed_frames,
            annotations: user_annotations,
            duplicate_ids,
            raw_file: file,
        }
    }
}

/// Parse card bound text for status, priority, clean title, and tags.
fn parse_card_text(raw_text: &str) -> (Option<String>, Option<i32>, String, Vec<String>) {
    let mut status = None;
    let mut priority = None;
    let mut tags = Vec::new();
    let mut clean_lines = Vec::new();

    for line in raw_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Extract status and priority from [STATUS · P1]
        if let Some(caps) = STATUS_HEADER_RE.captures(trimmed) {
            if let Some(s) = caps.get(1) {
                status = Some(s.as_str().to_lowercase());
            }
            if let Some(p) = caps.get(2) {
                priority = p.as_str().parse::<i32>().ok();
            }
            // Strip header from line
            let rest = &trimmed[caps[0].len()..];
            if !rest.trim().is_empty() {
                clean_lines.push(rest.trim().to_string());
            }
            continue;
        }

        // Extract tags
        if trimmed.starts_with('#') {
            for cap in TAG_RE.captures_iter(trimmed) {
                if let Some(t) = cap.get(1) {
                    tags.push(t.as_str().to_string());
                }
            }
            continue;
        }

        clean_lines.push(trimmed.to_string());
    }

    let title = clean_lines.join(" ");
    (status, priority, title, tags)
}

/// Parse edge type from custom_data or arrow label / prefix.
/// Defaults safely to non-blocking `EdgeType::Link`.
fn parse_arrow_edge_type(custom_edge_type: Option<&str>, label: Option<&str>) -> EdgeType {
    if let Some(t) = custom_edge_type {
        match t.to_lowercase().as_str() {
            "depends_on" | "dep" => return EdgeType::DependsOn,
            "soft_depends_on" | "soft" => return EdgeType::SoftDependsOn,
            "parent" => return EdgeType::Parent,
            "contributes_to" | "contrib" => return EdgeType::ContributesTo,
            "closes" => return EdgeType::Closes,
            "supersedes" => return EdgeType::Supersedes,
            "similar_to" | "sim" => return EdgeType::SimilarTo,
            _ => {}
        }
    }

    if let Some(lbl) = label {
        let trimmed = lbl.trim().to_lowercase();
        if trimmed.starts_with("dep:") || trimmed.starts_with("depends:") || trimmed.starts_with("depends_on:") {
            return EdgeType::DependsOn;
        }
        if trimmed.starts_with("soft:") || trimmed.starts_with("soft_dep:") {
            return EdgeType::SoftDependsOn;
        }
        if trimmed.starts_with("parent:") || trimmed.starts_with("child:") {
            return EdgeType::Parent;
        }
        if trimmed.starts_with("contrib:") || trimmed.starts_with("contributes:") || trimmed.starts_with("weight:") {
            return EdgeType::ContributesTo;
        }
        if trimmed.starts_with("close:") || trimmed.starts_with("closes:") {
            return EdgeType::Closes;
        }
        if trimmed.starts_with("super:") || trimmed.starts_with("supersedes:") {
            return EdgeType::Supersedes;
        }
        if trimmed.starts_with("sim:") || trimmed.starts_with("similar:") {
            return EdgeType::SimilarTo;
        }
    }

    // Safe default for user-drawn arrows
    EdgeType::Link
}

fn extract_id_from_text(text: &str) -> Option<String> {
    static ID_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:task|epic|mem|target|goal)-[a-zA-Z0-9]{4,16}").unwrap()
    });
    ID_RE.find(text).map(|m| m.as_str().to_string())
}

fn extract_id_from_frame_name(name: &str) -> Option<String> {
    static FRAME_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:epic|area|project)-[a-zA-Z0-9_-]+").unwrap()
    });
    FRAME_ID_RE.find(name).map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reader_card_parsing() {
        let mut file = ExcalidrawFile::default();
        let mut card = ExcalidrawElement::default();
        card.id = "card-1".to_string();
        card.element_type = "rectangle".to_string();
        card.bound_elements = Some(vec![BoundElement {
            id: "text-1".to_string(),
            element_type: "text".to_string(),
        }]);

        let mut text = ExcalidrawElement::default();
        text.id = "text-1".to_string();
        text.element_type = "text".to_string();
        text.container_id = Some("card-1".to_string());
        text.text = Some("[READY · P1] Implement reader pass\n#backend #pkb".to_string());

        file.elements.push(card);
        file.elements.push(text);

        let model = CanvasReader::parse_file(file);
        assert_eq!(model.cards.len(), 1);
        let c = &model.cards[0];
        assert_eq!(c.status.as_deref(), Some("ready"));
        assert_eq!(c.priority, Some(1));
        assert_eq!(c.title, "Implement reader pass");
        assert_eq!(c.tags, vec!["backend", "pkb"]);
    }

    #[test]
    fn test_safe_arrow_typing_default_link() {
        // 1. Unlinked arrow without bindings -> classified into annotations
        let mut file_unlinked = ExcalidrawFile::default();
        let mut arrow_unlinked = ExcalidrawElement::default();
        arrow_unlinked.id = "arrow-unlinked".to_string();
        arrow_unlinked.element_type = "arrow".to_string();
        arrow_unlinked.text = Some("user annotation arrow".to_string());
        file_unlinked.elements.push(arrow_unlinked);

        let model_unlinked = CanvasReader::parse_file(file_unlinked);
        assert_eq!(model_unlinked.arrows.len(), 0);
        assert_eq!(model_unlinked.annotations.len(), 1);

        // 2. Bound arrow between PKB nodes without typed prefix -> defaults safely to EdgeType::Link
        let mut file_bound = ExcalidrawFile::default();
        let mut card1 = ExcalidrawElement::default();
        card1.id = "elem-t1".to_string();
        card1.element_type = "rectangle".to_string();
        card1.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some("task-t1".to_string()),
                ..Default::default()
            }),
            extra: HashMap::new(),
        });

        let mut card2 = ExcalidrawElement::default();
        card2.id = "elem-t2".to_string();
        card2.element_type = "rectangle".to_string();
        card2.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some("task-t2".to_string()),
                ..Default::default()
            }),
            extra: HashMap::new(),
        });

        let mut arrow_bound = ExcalidrawElement::default();
        arrow_bound.id = "arrow-bound".to_string();
        arrow_bound.element_type = "arrow".to_string();
        arrow_bound.start_binding = Some(PointBinding {
            element_id: "elem-t1".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        arrow_bound.end_binding = Some(PointBinding {
            element_id: "elem-t2".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        arrow_bound.text = Some("plain user link".to_string());

        file_bound.elements.push(card1);
        file_bound.elements.push(card2);
        file_bound.elements.push(arrow_bound);

        let model_bound = CanvasReader::parse_file(file_bound);
        assert_eq!(model_bound.arrows.len(), 1);
        assert_eq!(model_bound.arrows[0].edge_type, EdgeType::Link);
    }

    #[test]
    fn test_duplicate_id_detection() {
        let mut file = ExcalidrawFile::default();
        for id in ["elem-1", "elem-2"] {
            let mut card = ExcalidrawElement::default();
            card.id = id.to_string();
            card.element_type = "rectangle".to_string();
            card.custom_data = Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some("task-dup".to_string()),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            });
            file.elements.push(card);
        }

        let model = CanvasReader::parse_file(file);
        assert_eq!(model.cards.len(), 2);
        assert_eq!(model.duplicate_ids, vec!["task-dup"]);
        assert!(!model.cards[0].is_duplicate);
        assert!(model.cards[1].is_duplicate);
    }

    #[test]
    fn test_reader_typed_arrow_prefixes() {
        let prefixes = [
            ("dep: blocker task", EdgeType::DependsOn),
            ("soft: informational context", EdgeType::SoftDependsOn),
            ("parent: subtask relationship", EdgeType::Parent),
            ("contrib: High strategic value", EdgeType::ContributesTo),
            ("close: resolves task", EdgeType::Closes),
            ("super: replaces legacy", EdgeType::Supersedes),
            ("sim: related note", EdgeType::SimilarTo),
            ("plain user link", EdgeType::Link),
        ];

        for (label, expected_type) in prefixes {
            let mut file = ExcalidrawFile::default();

            let mut card1 = ExcalidrawElement::default();
            card1.id = "elem-src".to_string();
            card1.element_type = "rectangle".to_string();
            card1.custom_data = Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some("task-src".to_string()),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            });

            let mut card2 = ExcalidrawElement::default();
            card2.id = "elem-tgt".to_string();
            card2.element_type = "rectangle".to_string();
            card2.custom_data = Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some("task-tgt".to_string()),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            });

            let mut arrow = ExcalidrawElement::default();
            arrow.id = "arrow-test".to_string();
            arrow.element_type = "arrow".to_string();
            arrow.start_binding = Some(PointBinding {
                element_id: "elem-src".to_string(),
                focus: 0.0,
                gap: 1.0,
                fixed_point: None,
            });
            arrow.end_binding = Some(PointBinding {
                element_id: "elem-tgt".to_string(),
                focus: 0.0,
                gap: 1.0,
                fixed_point: None,
            });
            arrow.text = Some(label.to_string());

            file.elements.push(card1);
            file.elements.push(card2);
            file.elements.push(arrow);

            let model = CanvasReader::parse_file(file);
            assert_eq!(model.arrows[0].edge_type, expected_type, "Failed on label: {}", label);
        }
    }

    #[test]
    fn test_reader_annotation_and_freedraw_preservation() {
        let mut file = ExcalidrawFile::default();

        // User doodle
        let mut freedraw = ExcalidrawElement::default();
        freedraw.id = "doodle-1".to_string();
        freedraw.element_type = "freedraw".to_string();
        freedraw.points = Some(vec![[0.0, 0.0], [10.0, 20.0], [30.0, 40.0]]);
        file.elements.push(freedraw);

        // Floating sticky note text
        let mut sticky = ExcalidrawElement::default();
        sticky.id = "sticky-note-1".to_string();
        sticky.element_type = "text".to_string();
        sticky.text = Some("Remember to test in staging first!".to_string());
        file.elements.push(sticky);

        let model = CanvasReader::parse_file(file);
        assert_eq!(model.cards.len(), 0);
        assert_eq!(model.annotations.len(), 2, "Must preserve both freehand doodle and sticky note");
        assert!(model.annotations.iter().any(|e| e.id == "doodle-1"));
        assert!(model.annotations.iter().any(|e| e.id == "sticky-note-1"));
    }
}
