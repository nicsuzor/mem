//! Typed AST and Schema for Excalidraw V2 JSON format and PKB metadata.
//!
//! Provides [`ExcalidrawFile`], [`ExcalidrawElement`], [`AppState`],
//! and [`PkbCustomData`] with serialization/deserialization support,
//! unified color matrices, and port connection constants.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ===========================================================================
// Layout & Styling Constants
// ===========================================================================

pub const CARD_WIDTH: f64 = 240.0;
pub const CARD_HEIGHT: f64 = 84.0;
pub const FRAME_PADDING: f64 = 32.0;
pub const FRAME_HEADER_HEIGHT: f64 = 40.0;

pub const DEFAULT_FONT_SIZE: f64 = 16.0;
pub const TITLE_FONT_SIZE: f64 = 14.0;
pub const BADGE_FONT_SIZE: f64 = 11.0;
pub const FONT_FAMILY_SANS: i32 = 2; // Helvetica / Normal
pub const FONT_FAMILY_CODE: i32 = 3; // Cascadia / Code

// Normalized port connection anchors
pub const PORT_IN: [f64; 2] = [0.0, 0.5]; // Left port
pub const PORT_OUT: [f64; 2] = [1.0, 0.5]; // Right port
pub const PORT_TOP: [f64; 2] = [0.5, 0.0]; // Top port
pub const PORT_BOTTOM: [f64; 2] = [0.5, 1.0]; // Bottom port

// ===========================================================================
// PKB Color Matrix
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementColorStyle {
    pub bg_color: &'static str,
    pub stroke_color: &'static str,
    pub stroke_style: &'static str,
    pub fill_style: &'static str,
    pub opacity: i32,
}

impl ElementColorStyle {
    pub const fn new(
        bg_color: &'static str,
        stroke_color: &'static str,
        stroke_style: &'static str,
        fill_style: &'static str,
        opacity: i32,
    ) -> Self {
        Self {
            bg_color,
            stroke_color,
            stroke_style,
            fill_style,
            opacity,
        }
    }
}

/// Returns the visual styling for a node given its status and type.

/// Returns visual styling presets for a node based on its preset string.
use std::path::PathBuf;
use std::fs;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct ThemeConfig {
    #[serde(default)]
    presets: std::collections::HashMap<String, PresetConfig>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PresetConfig {
    bg_color: String,
    stroke_color: String,
    stroke_style: String,
    fill_style: String,
    opacity: i32,
}

static THEME_CONFIG: OnceLock<Option<ThemeConfig>> = OnceLock::new();

fn get_theme_config() -> Option<&'static ThemeConfig> {
    THEME_CONFIG.get_or_init(|| {
        let config_path = std::env::var("EXCALIDRAW_THEME_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".gemini/config/excalidraw_theme.json")
            });
        if let Ok(config_str) = fs::read_to_string(&config_path) {
            serde_json::from_str::<ThemeConfig>(&config_str).ok()
        } else {
            None
        }
    }).as_ref()
}

pub fn node_preset_style(preset: &str) -> ElementColorStyle {
    let p = preset.to_lowercase();
    
    if let Some(config) = get_theme_config() {
        if let Some(cfg) = config.presets.get(&p) {
            return ElementColorStyle::new(
                Box::leak(cfg.bg_color.clone().into_boxed_str()),
                Box::leak(cfg.stroke_color.clone().into_boxed_str()),
                Box::leak(cfg.stroke_style.clone().into_boxed_str()),
                Box::leak(cfg.fill_style.clone().into_boxed_str()),
                cfg.opacity,
            );
        }
    }

    match p.as_str() {

        "hero" => ElementColorStyle::new("#fff3bf", "#f59f00", "solid", "solid", 100),
        "sticky" => ElementColorStyle::new("#fff9db", "#f08c00", "solid", "hachure", 100),
        "zone" => ElementColorStyle::new("#edf2ff", "#4c6ef5", "dashed", "solid", 30),
        "badge" => ElementColorStyle::new("#1a1a1a", "#1a1a1a", "solid", "solid", 100),
        _ => ElementColorStyle::new("#f1f3f5", "#868e96", "solid", "solid", 100), // fallback
    }
}


pub fn node_color_style(status: Option<&str>, node_type: Option<&str>) -> ElementColorStyle {
    let t = node_type.unwrap_or("task").to_lowercase();
    let s = status.unwrap_or("inbox").to_lowercase();

    // Specific structural node types
    match t.as_str() {
        "epic" => return ElementColorStyle::new("#edf2ff", "#4c6ef5", "solid", "solid", 100),
        "area" => return ElementColorStyle::new("#e3fafc", "#0c8599", "solid", "solid", 100),
        "target" => return ElementColorStyle::new("#fff3bf", "#f59f00", "solid", "solid", 100),
        "goal" => return ElementColorStyle::new("#ffec99", "#e67700", "solid", "solid", 100),
        "learn" => return ElementColorStyle::new("#e6fcf5", "#099268", "solid", "solid", 100),
        "memory" | "note" | "insight" => {
            return ElementColorStyle::new("#fff9db", "#f08c00", "solid", "solid", 100)
        }
        _ => {}
    }

    // Status-driven styling for tasks and general items
    match s.as_str() {
        "ready" => ElementColorStyle::new("#d3f9d8", "#2b8a3e", "solid", "solid", 100),
        "queued" => ElementColorStyle::new("#e6fcf5", "#099268", "solid", "solid", 100),
        "active" | "in_progress" | "doing" => {
            ElementColorStyle::new("#d0ebff", "#1971c2", "solid", "solid", 100)
        }
        "blocked" | "waiting" => {
            ElementColorStyle::new("#ffe8cc", "#e8590c", "solid", "solid", 100)
        }
        "review" | "testing" => {
            ElementColorStyle::new("#f3d9fa", "#862e9c", "solid", "solid", 100)
        }
        "done" | "completed" | "released" => {
            ElementColorStyle::new("#e9ecef", "#adb5bd", "solid", "solid", 60)
        }
        "cancelled" | "abandoned" => {
            ElementColorStyle::new("#faecec", "#fa5252", "dashed", "solid", 60)
        }
        _ => ElementColorStyle::new("#f1f3f5", "#868e96", "solid", "solid", 100), // inbox / draft
    }
}

/// Returns the visual styling for an edge given its type.
pub fn edge_color_style(edge_type: &str) -> (&'static str, &'static str, f64) {
    match edge_type {
        "depends_on" | "dep" => ("#e03131", "solid", 2.0),
        "soft_depends_on" | "soft" => ("#868e96", "dashed", 1.5),
        "parent" => ("#4c6ef5", "solid", 2.5),
        "contributes_to" | "contrib" => ("#1971c2", "solid", 2.0),
        "closes" => ("#2b8a3e", "solid", 1.5),
        "supersedes" => ("#f08c00", "dashed", 1.5),
        "similar_to" | "sim" => ("#ced4da", "dashed", 1.0),
        _ => ("#adb5bd", "solid", 1.0), // link
    }
}

// ===========================================================================
// Excalidraw V2 Top-Level Schema
// ===========================================================================

/// Complete Excalidraw V2 file container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcalidrawFile {
    #[serde(rename = "type", default = "default_type_excalidraw")]
    pub file_type: String,
    #[serde(default = "default_version_2")]
    pub version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub elements: Vec<ExcalidrawElement>,
    #[serde(rename = "appState", default)]
    pub app_state: AppState,
    #[serde(default = "default_files_map")]
    pub files: serde_json::Value,
}

fn default_type_excalidraw() -> String {
    "excalidraw".to_string()
}

fn default_version_2() -> i32 {
    2
}

fn default_files_map() -> serde_json::Value {
    serde_json::json!({})
}

impl Default for ExcalidrawFile {
    fn default() -> Self {
        Self {
            file_type: "excalidraw".to_string(),
            version: 2,
            source: Some("https://excalidraw.com".to_string()),
            elements: Vec::new(),
            app_state: AppState::default(),
            files: serde_json::json!({}),
        }
    }
}

// ===========================================================================
// AppState
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppState {
    #[serde(
        rename = "viewBackgroundColor",
        default = "default_view_bg_color"
    )]
    pub view_background_color: String,
    #[serde(rename = "gridSize", skip_serializing_if = "Option::is_none")]
    pub grid_size: Option<i32>,
    #[serde(rename = "theme", skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(rename = "scrollX", default)]
    pub scroll_x: f64,
    #[serde(rename = "scrollY", default)]
    pub scroll_y: f64,
    #[serde(rename = "zoom", default = "default_zoom")]
    pub zoom: serde_json::Value,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_view_bg_color() -> String {
    "#ffffff".to_string()
}

fn default_zoom() -> serde_json::Value {
    serde_json::json!({ "value": 1.0 })
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            view_background_color: "#ffffff".to_string(),
            grid_size: Some(20),
            theme: Some("light".to_string()),
            scroll_x: 0.0,
            scroll_y: 0.0,
            zoom: default_zoom(),
            extra: HashMap::new(),
        }
    }
}

// ===========================================================================
// Excalidraw Element
// ===========================================================================

/// A single graphical or logical element in the Excalidraw scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcalidrawElement {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub angle: f64,
    #[serde(rename = "strokeColor", default = "default_stroke_color")]
    pub stroke_color: String,
    #[serde(rename = "backgroundColor", default = "default_bg_color")]
    pub background_color: String,
    #[serde(rename = "fillStyle", default = "default_fill_style")]
    pub fill_style: String,
    #[serde(rename = "strokeWidth", default = "default_stroke_width")]
    pub stroke_width: f64,
    #[serde(rename = "strokeStyle", default = "default_stroke_style")]
    pub stroke_style: String,
    #[serde(default = "default_roughness")]
    pub roughness: f64,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(rename = "groupIds", default)]
    pub group_ids: Vec<String>,
    #[serde(rename = "frameId", skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roundness: Option<Roundness>,
    #[serde(default = "default_seed")]
    pub seed: i64,
    #[serde(default = "default_version")]
    pub version: i64,
    #[serde(rename = "versionNonce", default = "default_version_nonce")]
    pub version_nonce: i64,
    #[serde(rename = "isDeleted", default)]
    pub is_deleted: bool,
    #[serde(rename = "boundElements", skip_serializing_if = "Option::is_none")]
    pub bound_elements: Option<Vec<BoundElement>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(rename = "customData", skip_serializing_if = "Option::is_none")]
    pub custom_data: Option<CustomData>,

    // Text specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "fontSize", skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(rename = "fontFamily", skip_serializing_if = "Option::is_none")]
    pub font_family: Option<i32>,
    #[serde(rename = "textAlign", skip_serializing_if = "Option::is_none")]
    pub text_align: Option<String>,
    #[serde(rename = "verticalAlign", skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(rename = "containerId", skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(rename = "originalText", skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    #[serde(rename = "lineHeight", skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(rename = "autoResize", skip_serializing_if = "Option::is_none")]
    pub auto_resize: Option<bool>,

    // Linear / Arrow / Line specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f64; 2]>>,
    #[serde(
        rename = "lastCommittedPoint",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_committed_point: Option<[f64; 2]>,
    #[serde(rename = "startBinding", skip_serializing_if = "Option::is_none")]
    pub start_binding: Option<PointBinding>,
    #[serde(rename = "endBinding", skip_serializing_if = "Option::is_none")]
    pub end_binding: Option<PointBinding>,
    #[serde(rename = "startArrowhead", skip_serializing_if = "Option::is_none")]
    pub start_arrowhead: Option<String>,
    #[serde(rename = "endArrowhead", skip_serializing_if = "Option::is_none")]
    pub end_arrowhead: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elbowed: Option<bool>,

    // Frame / MagicFrame specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    // Freedraw specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressures: Option<Vec<f64>>,
    #[serde(
        rename = "simulatePressure",
        skip_serializing_if = "Option::is_none"
    )]
    pub simulate_pressure: Option<bool>,

    // Catch-all for extra Excalidraw attributes
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_stroke_color() -> String {
    "#1e1e1e".to_string()
}

fn default_bg_color() -> String {
    "transparent".to_string()
}

fn default_fill_style() -> String {
    "solid".to_string()
}

fn default_stroke_width() -> f64 {
    1.0
}

fn default_stroke_style() -> String {
    "solid".to_string()
}

fn default_roughness() -> f64 {
    0.0
}

fn default_opacity() -> f64 {
    100.0
}

fn default_seed() -> i64 {
    1
}

fn default_version() -> i64 {
    1
}

fn default_version_nonce() -> i64 {
    1
}

impl Default for ExcalidrawElement {
    fn default() -> Self {
        Self {
            id: String::new(),
            element_type: "rectangle".to_string(),
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            angle: 0.0,
            stroke_color: default_stroke_color(),
            background_color: default_bg_color(),
            fill_style: default_fill_style(),
            stroke_width: default_stroke_width(),
            stroke_style: default_stroke_style(),
            roughness: default_roughness(),
            opacity: default_opacity(),
            group_ids: Vec::new(),
            frame_id: None,
            roundness: Some(Roundness {
                roundness_type: 3,
                value: None,
            }),
            seed: default_seed(),
            version: default_version(),
            version_nonce: default_version_nonce(),
            is_deleted: false,
            bound_elements: None,
            updated: None,
            link: None,
            locked: None,
            custom_data: None,
            text: None,
            font_size: None,
            font_family: None,
            text_align: None,
            vertical_align: None,
            baseline: None,
            container_id: None,
            original_text: None,
            line_height: None,
            auto_resize: None,
            points: None,
            last_committed_point: None,
            start_binding: None,
            end_binding: None,
            start_arrowhead: None,
            end_arrowhead: None,
            elbowed: None,
            name: None,
            pressures: None,
            simulate_pressure: None,
            extra: HashMap::new(),
        }
    }
}

impl ExcalidrawElement {
    pub fn is_card(&self) -> bool {
        (self.element_type == "rectangle" || self.element_type == "diamond" || self.element_type == "ellipse")
            && self.custom_data.as_ref().map_or(false, |c| c.pkb.as_ref().map_or(false, |p| p.node_id.is_some()))
    }

    pub fn is_frame(&self) -> bool {
        self.element_type == "frame" || self.element_type == "magicframe"
    }

    pub fn is_arrow(&self) -> bool {
        self.element_type == "arrow"
    }

    pub fn is_text(&self) -> bool {
        self.element_type == "text"
    }

    pub fn pkb_node_id(&self) -> Option<&str> {
        self.custom_data.as_ref()?.pkb.as_ref()?.node_id.as_deref()
    }
}

// ===========================================================================
// Roundness & BoundElement
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Roundness {
    #[serde(rename = "type")]
    pub roundness_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundElement {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: String,
}

// ===========================================================================
// PointBinding
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointBinding {
    #[serde(rename = "elementId")]
    pub element_id: String,
    #[serde(default)]
    pub focus: f64,
    #[serde(default)]
    pub gap: f64,
    #[serde(rename = "fixedPoint", skip_serializing_if = "Option::is_none")]
    pub fixed_point: Option<[f64; 2]>,
}

// ===========================================================================
// CustomData & PkbCustomData
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CustomData {
    #[serde(rename = "pkb", skip_serializing_if = "Option::is_none")]
    pub pkb: Option<PkbCustomData>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PkbCustomData {
    #[serde(
        rename = "nodeId",
        alias = "node_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub node_id: Option<String>,
    #[serde(
        rename = "nodeType",
        alias = "node_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub node_type: Option<String>,
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(
        rename = "intent",
        alias = "priority",
        skip_serializing_if = "Option::is_none"
    )]
    pub intent: Option<i32>,
    #[serde(rename = "parent", skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(rename = "tags", default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(
        rename = "edgeType",
        alias = "edge_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub edge_type: Option<String>,
    #[serde(
        rename = "sourceId",
        alias = "source_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_id: Option<String>,
    #[serde(
        rename = "targetId",
        alias = "target_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_id: Option<String>,
    #[serde(
        rename = "syncHash",
        alias = "sync_hash",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_hash: Option<String>,
    #[serde(
        rename = "isPkbManaged",
        alias = "is_pkb_managed",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_pkb_managed: Option<bool>,
    #[serde(rename = "weight", skip_serializing_if = "Option::is_none")]
    pub weight: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_serialization_roundtrip() {
        let mut file = ExcalidrawFile::default();
        let mut elem = ExcalidrawElement::default();
        elem.id = "elem-1".to_string();
        elem.element_type = "rectangle".to_string();
        elem.x = 100.0;
        elem.y = 200.0;
        elem.width = CARD_WIDTH;
        elem.height = CARD_HEIGHT;
        elem.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some("task-123".to_string()),
                node_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                intent: Some(1),
                parent: Some("epic-abc".to_string()),
                tags: vec!["backend".to_string()],
                edge_type: None,
                source_id: None,
                target_id: None,
                sync_hash: Some("hash123".to_string()),
                is_pkb_managed: Some(true),
                weight: None,
            }),
            extra: HashMap::new(),
        });
        file.elements.push(elem);

        let json = serde_json::to_string_pretty(&file).expect("serialize");
        let parsed: ExcalidrawFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.elements.len(), 1);
        assert_eq!(parsed.elements[0].id, "elem-1");
        assert!(parsed.elements[0].is_card());
        assert_eq!(parsed.elements[0].pkb_node_id(), Some("task-123"));
    }

    #[test]
    fn test_color_matrix() {
        let ready_style = node_color_style(Some("ready"), Some("task"));
        assert_eq!(ready_style.bg_color, "#d3f9d8");

        let epic_style = node_color_style(Some("inbox"), Some("epic"));
        assert_eq!(epic_style.bg_color, "#edf2ff");

        let (stroke, style, width) = edge_color_style("depends_on");
        assert_eq!(stroke, "#e03131");
        assert_eq!(style, "solid");
        assert_eq!(width, 2.0);
    }
}
