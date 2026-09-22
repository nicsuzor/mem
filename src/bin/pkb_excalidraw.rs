//! Token-cheap projections and full CRUD operations for .excalidraw and .excalidrawlib files.
//!
//! Supports modes:
//!   Projections: summary, map, style, check, diff, lib, item
//!   Structural Diff: struct-diff
//!   Inspection: nodes, edges, arrows, inspect, get
//!   CRUD: add-node, add-text, connect, set-text, fit, move-elem, delete-elem, batch
//!   Theme: theme export, theme apply
//!   Validation: overlap, arrows-check

use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::process;

const INDEX_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_-";

const USAGE: &str = r#"Token-cheap projections of an .excalidraw file. Stdlib only.

Usage: pkb-excalidraw FILE [summary|map|style|check|overlap|arrows-check|nodes|edges|arrows]
       pkb-excalidraw FILE inspect <id>
       pkb-excalidraw FILE get <id>
       pkb-excalidraw FILE describe
       pkb-excalidraw FILE query [--type <type>] [--bbox X1,Y1,X2,Y2] [--filter KEY=VAL] [--filter-json '<json>']
       pkb-excalidraw FILE screenshot [--out <path>] [--format svg|png] [--no-background]
       pkb-excalidraw FILE1 diff FILE2
       pkb-excalidraw FILE1 struct-diff FILE2
       pkb-excalidraw FILE.excalidrawlib lib
       pkb-excalidraw FILE.excalidrawlib item SELECTOR --after INDEX [--at X,Y]

CRUD & Mutation Commands:
       pkb-excalidraw FILE add-node --type <type> --text "<text>" [--at X,Y] [--size W,H] [--role <role>] [--color <hex>] [--id <custom_id>]
       pkb-excalidraw FILE add-text --text "<text>" --at X,Y [--font-size <size>] [--color <hex>]
       pkb-excalidraw FILE connect --from <id1> --to <id2> [--label "<label>"] [--color <hex>]
       pkb-excalidraw FILE set-text <id> "<new_text>"
       pkb-excalidraw FILE fit <id> "<new_text>"
       pkb-excalidraw FILE move-elem <id> [--to X,Y | --by DX,DY]
       pkb-excalidraw FILE delete-elem <id> [--cascade-arrows]
       pkb-excalidraw FILE update <id> --set '<json>'
       pkb-excalidraw FILE arrange {align|distribute|group|ungroup|lock|unlock|duplicate} [args]
       pkb-excalidraw FILE apply <patch.json | - >
       pkb-excalidraw FILE batch <changes.json | - >
       pkb-excalidraw FILE clear [--yes]

Snapshot, Import & Export Commands:
       pkb-excalidraw FILE snapshot {save <name>|list|restore <name>}
       pkb-excalidraw FILE export [--out <path>] [--format json|obsidian]
       pkb-excalidraw FILE import <src.json | src.md | - > [--replace]

Theme Commands:
       pkb-excalidraw FILE theme export [out.json]
       pkb-excalidraw FILE theme apply <theme.json | default | retro-terminal | aops-default> [--all | --id <id>]
"#;

// ============================================================================
// Core Helper Functions & Projections
// ============================================================================

pub fn live(doc: &Value) -> Vec<&Value> {
    doc.get("elements")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|e| !e.get("isDeleted").and_then(|v| v.as_bool()).unwrap_or(false))
                .collect()
        })
        .unwrap_or_default()
}

pub fn element_text(e: &Value) -> &str {
    if let Some(orig) = e.get("originalText").and_then(|v| v.as_str()) {
        if !orig.is_empty() {
            return orig;
        }
    }
    if let Some(text) = e.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return text;
        }
    }
    ""
}

pub fn label_of(e: &Value, texts: &HashMap<&str, &Value>) -> String {
    if let Some(bound_arr) = e.get("boundElements").and_then(|v| v.as_array()) {
        for b in bound_arr {
            if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(id) = b.get("id").and_then(|v| v.as_str()) {
                    if let Some(text_el) = texts.get(id) {
                        return element_text(text_el).replace('\n', " / ");
                    }
                }
            }
        }
    }
    String::new()
}

pub fn extent(els: &[&Value]) -> (f64, f64, f64, f64) {
    if els.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for e in els {
        let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

        min_x = min_x.min(x).min(x + w);
        max_x = max_x.max(x).max(x + w);
        min_y = min_y.min(y).min(y + h);
        max_y = max_y.max(y).max(y + h);
    }

    if min_x.is_infinite() {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

pub fn truncate_safe(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn format_str_list(items: &[&str]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

fn format_str_counter(counts: &[(&str, usize)]) -> String {
    let inner: Vec<String> = counts
        .iter()
        .map(|(k, v)| format!("'{k}': {v}"))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

fn format_modal_val(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => (if *b { "True" } else { "False" }).to_string(),
        Value::Null => "None".to_string(),
        _ => val.to_string(),
    }
}

fn format_val_counter(counts: &[(&Value, usize)]) -> String {
    let inner: Vec<String> = counts
        .iter()
        .map(|(k, v)| {
            let k_str = match k {
                Value::String(s) => format!("'{s}'"),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => (if *b { "True" } else { "False" }).to_string(),
                Value::Null => "None".to_string(),
                _ => k.to_string(),
            };
            format!("{k_str}: {v}")
        })
        .collect();
    format!("{{{}}}", inner.join(", "))
}

pub fn new_id() -> String {
    let mut rng = rand::rng();
    (0..21)
        .map(|_| {
            let idx = rng.random_range(0..ID_ALPHABET.len());
            ID_ALPHABET[idx] as char
        })
        .collect()
}

pub fn mint_indices(after: &str, count: usize) -> Result<Vec<String>, String> {
    let alphabet_len = INDEX_ALPHABET.len();
    if count > alphabet_len * alphabet_len {
        return Err(format!("cannot mint {count} indices after {after}"));
    }
    let base = if after.is_empty() { "a0" } else { after };
    let mut res = Vec::with_capacity(count);
    for i in 0..count {
        let c1 = INDEX_ALPHABET[i / alphabet_len] as char;
        let c2 = INDEX_ALPHABET[i % alphabet_len] as char;
        res.push(format!("{base}{c1}{c2}"));
    }
    Ok(res)
}

pub fn max_index_of(doc: &Value) -> String {
    let els = live(doc);
    let idx: Vec<&str> = els
        .iter()
        .filter_map(|e| e.get("index").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .collect();
    idx.into_iter().max().unwrap_or("a0").to_string()
}

// ============================================================================
// Font Metrics & Geometry
// ============================================================================

pub fn compute_text_dimensions(text: &str, font_size: f64) -> (f64, f64) {
    let lines: Vec<&str> = text.split('\n').collect();
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // Virgil font metric: char width ~ 0.56 * font_size
    let width = (max_chars as f64 * font_size * 0.56).max(20.0);
    // Line height 1.25
    let line_height = font_size * 1.25;
    let height = (lines.len() as f64 * line_height).max(line_height);
    (width, height)
}

pub fn ccw(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

pub fn segments_intersect(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
    let d1 = ccw(p3, p4, p1);
    let d2 = ccw(p3, p4, p2);
    let d3 = ccw(p1, p2, p3);
    let d4 = ccw(p1, p2, p4);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    false
}

pub fn point_in_rect(p: (f64, f64), r: (f64, f64, f64, f64)) -> bool {
    p.0 >= r.0 && p.0 <= r.0 + r.2 && p.1 >= r.1 && p.1 <= r.1 + r.3
}

pub fn segment_intersects_rect(p1: (f64, f64), p2: (f64, f64), r: (f64, f64, f64, f64)) -> bool {
    let (rx, ry, rw, rh) = r;
    if rw <= 0.0 || rh <= 0.0 {
        return false;
    }

    // Midpoint check if segment is fully inside
    let mid = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
    if point_in_rect(mid, r) {
        return true;
    }

    let top_l = (rx, ry);
    let top_r = (rx + rw, ry);
    let bot_l = (rx, ry + rh);
    let bot_r = (rx + rw, ry + rh);

    if segments_intersect(p1, p2, top_l, top_r)
        || segments_intersect(p1, p2, top_r, bot_r)
        || segments_intersect(p1, p2, bot_r, bot_l)
        || segments_intersect(p1, p2, bot_l, top_l)
    {
        return true;
    }

    false
}

pub fn rects_overlap(r1: (f64, f64, f64, f64), r2: (f64, f64, f64, f64)) -> bool {
    let (x1, y1, w1, h1) = r1;
    let (x2, y2, w2, h2) = r2;
    x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2
}

pub fn rect_contains(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> bool {
    outer.0 <= inner.0
        && outer.1 <= inner.1
        && outer.0 + outer.2 >= inner.0 + inner.2
        && outer.1 + outer.3 >= inner.1 + inner.3
}

// ============================================================================
// Theme Definition
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub font_family: i64,
    pub roughness: i64,
    pub fill_style: String,
    pub stroke_color: String,
    pub text_color: String,
    pub roles: HashMap<String, String>,
}

impl Theme {
    pub fn default_retro() -> Self {
        let mut roles = HashMap::new();
        roles.insert("emphasis".to_string(), "#c9b458".to_string());
        roles.insert("success".to_string(), "#8fbc8f".to_string());
        roles.insert("info".to_string(), "#7a9fbf".to_string());
        roles.insert("warning".to_string(), "#ffa500".to_string());
        roles.insert("error".to_string(), "#ff6666".to_string());
        roles.insert("surface".to_string(), "#252525".to_string());
        roles.insert("muted".to_string(), "#888888".to_string());

        Theme {
            name: "retro-terminal".to_string(),
            font_family: 1,
            roughness: 2,
            fill_style: "hachure".to_string(),
            stroke_color: "#404040".to_string(),
            text_color: "#1a1a1a".to_string(),
            roles,
        }
    }

    pub fn color_for_role(&self, role: &str) -> Option<String> {
        self.roles.get(role).cloned()
    }
}

// ============================================================================
// Logical Nodes & Edges
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct LogicalNode {
    pub id: String,
    pub elem_type: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub label: String,
    pub role: String,
    pub text_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogicalEdge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub label: String,
    pub text_id: Option<String>,
}

pub fn get_logical_nodes_and_edges(doc: &Value) -> (Vec<LogicalNode>, Vec<LogicalEdge>) {
    let els = live(doc);
    let mut texts: HashMap<&str, &Value> = HashMap::new();
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts.insert(id, e);
            }
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for e in &els {
        let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let container_id = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");

        if e_type == "text" && !container_id.is_empty() {
            // Container-bound text is folded into container node or arrow edge
            continue;
        }

        if e_type == "arrow" {
            let s = e
                .get("startBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("-")
                .to_string();
            let t = e
                .get("endBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("-")
                .to_string();

            let mut label = String::new();
            let mut text_id = None;
            if let Some(bound_arr) = e.get("boundElements").and_then(|v| v.as_array()) {
                for b in bound_arr {
                    if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                            if let Some(text_el) = texts.get(bid) {
                                label = element_text(text_el).to_string();
                                text_id = Some(bid.to_string());
                                break;
                            }
                        }
                    }
                }
            }

            edges.push(LogicalEdge {
                id,
                from_id: s,
                to_id: t,
                label,
                text_id,
            });
        } else {
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let width = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let height = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let role = e
                .get("customData")
                .and_then(|v| v.get("role"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut label = String::new();
            let mut text_id = None;

            if e_type == "text" {
                label = element_text(e).to_string();
            } else if let Some(bound_arr) = e.get("boundElements").and_then(|v| v.as_array()) {
                for b in bound_arr {
                    if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                            if let Some(text_el) = texts.get(bid) {
                                label = element_text(text_el).to_string();
                                text_id = Some(bid.to_string());
                                break;
                            }
                        }
                    }
                }
            }

            nodes.push(LogicalNode {
                id,
                elem_type: e_type.to_string(),
                x,
                y,
                width,
                height,
                label,
                role,
                text_id,
            });
        }
    }

    (nodes, edges)
}

// ============================================================================
// Atomic File Safety & Check Verification
// ============================================================================

pub fn is_obsidian_excalidraw_md(content: &str) -> bool {
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return false;
    }
    content.contains("# Excalidraw Data") || content.contains("excalidraw-plugin:")
}

pub fn extract_scene_json_from_obsidian_md(md: &str) -> Result<String, String> {
    if let Some(drawing_idx) = md.find("## Drawing").or_else(|| md.find("# Drawing")) {
        let after_drawing = &md[drawing_idx..];
        if let Some(fence_start) = after_drawing.find("```") {
            let after_fence = &after_drawing[fence_start + 3..];
            if let Some(newline_pos) = after_fence.find('\n') {
                let json_content = &after_fence[newline_pos + 1..];
                if let Some(end_fence) = json_content.find("\n```").or_else(|| json_content.find("\r\n```")) {
                    let raw_json = json_content[..end_fence].trim();
                    return Ok(raw_json.to_string());
                }
            }
        }
    }
    Err("No Drawing code block found in Obsidian Excalidraw file".to_string())
}

pub fn wrap_scene_as_obsidian_md(scene: &Value) -> Result<String, String> {
    let elements = scene
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Not an Excalidraw scene: missing elements array".to_string())?;

    let mut text_entries = Vec::new();
    for el in elements {
        let is_deleted = el.get("isDeleted").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_deleted {
            continue;
        }
        let elem_type = el.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if elem_type == "text" {
            let id = el.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let raw_text = el
                .get("originalText")
                .or_else(|| el.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !raw_text.is_empty() && !id.is_empty() {
                text_entries.push(format!("{} ^{}", raw_text, id));
            }
        }
    }

    let text_section = if text_entries.is_empty() {
        String::new()
    } else {
        format!("{}\n", text_entries.join("\n\n"))
    };

    let json_str = serde_json::to_string_pretty(scene).map_err(|e| e.to_string())?;

    Ok(format!(
        "---\n\nexcalidraw-plugin: parsed\ntags: [excalidraw]\n\n---\n==⚠  Switch to EXCALIDRAW VIEW in the MORE OPTIONS menu of this document. ⚠==\n\n\n# Excalidraw Data\n## Text Elements\n{}%%\n## Drawing\n```json\n{}\n```\n%%\n",
        text_section, json_str
    ))
}

pub fn atomic_save(file_path: &str, doc: &mut Value) -> Result<(), String> {
    atomic_save_ex(file_path, doc, false)
}

pub fn atomic_save_ex(file_path: &str, doc: &mut Value, force_obsidian: bool) -> Result<(), String> {
    // 1. Maintain array sort order: elements.sort_by_key(|e| e.index)
    if let Some(arr) = doc.get_mut("elements").and_then(|v| v.as_array_mut()) {
        arr.sort_by(|a, b| {
            let idx_a = a.get("index").and_then(|v| v.as_str()).unwrap_or("");
            let idx_b = b.get("index").and_then(|v| v.as_str()).unwrap_or("");
            idx_a.cmp(idx_b)
        });
    }

    // 2. Validate structural integrity
    if let Err(fails) = cmd_check(doc) {
        return Err(format!("Validation failed before save:\n  {}", fails.join("\n  ")));
    }

    // 3. Write to temporary file in same directory (.tmp.<pid>.<rand>)
    let path = std::path::Path::new(file_path);
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut rng = rand::rng();
    let rand_val: u64 = rng.random();
    let pid = std::process::id();
    let temp_name = format!(".tmp.{}.{}", pid, rand_val);
    let temp_path = parent.join(temp_name);

    let is_obsidian = force_obsidian || file_path.ends_with(".md");
    let content = if is_obsidian {
        wrap_scene_as_obsidian_md(doc)?
    } else {
        serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?
    };

    fs::write(&temp_path, content).map_err(|e| format!("failed to write temp file {:?}: {}", temp_path, e))?;

    // 4. Atomically rename
    fs::rename(&temp_path, path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("failed to atomically rename {:?} to {:?}: {}", temp_path, path, e)
    })?;

    Ok(())
}

// ============================================================================
// CRUD Mutations
// ============================================================================


pub fn mutate_update_node(
    doc: &mut Value,
    target_id: &str,
    angle: Option<f64>,
    roughness: Option<f64>,
    fill_style: Option<&str>,
    preset: Option<&str>,
) -> Result<(), String> {
    let mut style = None;
    if let Some(p) = preset {
        style = Some(mem::excalidraw::schema::node_preset_style(p));
    }

    let elements = doc.get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or("no elements array")?;

    let mut found = false;
    for elem in elements.iter_mut() {
        if elem.get("id").and_then(|v| v.as_str()) == Some(target_id) {
            found = true;
            
            // Apply preset properties FIRST
            if let Some(s) = &style {
                elem["strokeColor"] = serde_json::json!(s.stroke_color);
                elem["backgroundColor"] = serde_json::json!(s.bg_color);
                elem["fillStyle"] = serde_json::json!(s.fill_style);
                elem["strokeStyle"] = serde_json::json!(s.stroke_style);
                elem["opacity"] = serde_json::json!(s.opacity);
                elem["roundness"] = serde_json::json!({ "type": 3 });
                if preset == Some("zone") {
                    elem["roughness"] = serde_json::json!(1.0);
                } else {
                    elem["roughness"] = serde_json::json!(2.0);
                }
            }

            // Apply explicit arguments SECOND (overriding presets if passed)
            if let Some(a) = angle {
                elem["angle"] = serde_json::json!(a);
            }
            if let Some(r) = roughness {
                elem["roughness"] = serde_json::json!(r);
            }
            if let Some(fs) = fill_style {
                elem["fillStyle"] = serde_json::json!(fs);
            }
            
            break;
        }
    }
    
    if !found {
        return Err(format!("node {} not found", target_id));
    }
    
    Ok(())
}


pub fn mutate_add_node(
    doc: &mut Value,
    elem_type: &str,
    text: &str,
    at: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
    role: Option<&str>,
    color: Option<&str>,
    theme: Option<&Theme>,
    custom_id: Option<&str>,
    angle: Option<f64>,
    roughness: Option<f64>,
    fill_style: Option<&str>,
    preset: Option<&str>,
) -> Result<(String, String), String> {
    let max_idx = max_index_of(doc);
    let minted = mint_indices(&max_idx, 2)?;
    let shape_index = &minted[0];
    let text_index = &minted[1];

    let shape_id = if let Some(cid) = custom_id {
        if live(doc).iter().any(|e| e.get("id").and_then(|v| v.as_str()) == Some(cid)) {
            return Err(format!("element with id {cid:?} already exists"));
        }
        cid.to_string()
    } else {
        new_id()
    };
    let text_id = new_id();

    let font_size = 20.0;
    let (tw, th) = compute_text_dimensions(text, font_size);

    let pad_x = 40.0;
    let pad_y = 30.0;

    let (bw, bh) = match size {
        Some((w, h)) => (w.max(tw + 10.0), h.max(th + 10.0)),
        None => ((tw + pad_x).max(100.0), (th + pad_y).max(50.0)),
    };

    let (x, y) = at.unwrap_or((100.0, 100.0));
    let tx = x + (bw - tw) / 2.0;
    let ty = y + (bh - th) / 2.0;

    let default_theme = Theme::default_retro();
    let th_ref = theme.unwrap_or(&default_theme);


    let mut bg_color = "transparent".to_string();
    let mut applied_stroke_color = th_ref.stroke_color.clone();
    let mut applied_fill_style = th_ref.fill_style.clone();
    let mut applied_roughness = th_ref.roughness as f64;

    if let Some(p) = preset {
        let style = mem::excalidraw::schema::node_preset_style(p);
        bg_color = style.bg_color.to_string();
        applied_stroke_color = style.stroke_color.to_string();
        applied_fill_style = style.fill_style.to_string();
        if p == "sticky" { applied_roughness = 2.0; }
    } else if let Some(c) = color {
        bg_color = c.to_string();
    } else if let Some(r) = role {
        bg_color = th_ref.color_for_role(r).unwrap_or_else(|| "transparent".to_string());
    }
    
    if let Some(fs) = fill_style {
        applied_fill_style = fs.to_string();
    }
    if let Some(r) = roughness {
        applied_roughness = r;
    }
    let applied_angle = angle.unwrap_or(0.0);


    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut rng = rand::rng();
    let seed1: i32 = rng.random_range(1..=i32::MAX);
    let nonce1: i32 = rng.random_range(1..=i32::MAX);
    let seed2: i32 = rng.random_range(1..=i32::MAX);
    let nonce2: i32 = rng.random_range(1..=i32::MAX);

    let mut shape_json = json!({
        "id": shape_id,
        "type": elem_type,
        "x": x,
        "y": y,
        "width": bw,
        "height": bh,
        "angle": applied_angle,
        "strokeColor": applied_stroke_color,
        "backgroundColor": bg_color,
        "fillStyle": applied_fill_style,
        "strokeWidth": 2,
        "strokeStyle": "solid",
        "roughness": applied_roughness,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "index": shape_index,
        "roundness": if elem_type == "rectangle" { json!({ "type": 3 }) } else { json!(null) },
        "seed": seed1,
        "version": 1,
        "versionNonce": nonce1,
        "isDeleted": false,
        "boundElements": [
            { "id": text_id, "type": "text" }
        ],
        "updated": stamp,
        "link": null,
        "locked": false
    });

    if let Some(r) = role {
        shape_json["customData"] = json!({ "role": r });
    }

    let text_json = json!({
        "id": text_id,
        "type": "text",
        "x": tx,
        "y": ty,
        "width": tw,
        "height": th,
        "angle": 0,
        "strokeColor": th_ref.text_color,
        "backgroundColor": "transparent",
        "fillStyle": th_ref.fill_style,
        "strokeWidth": 1,
        "strokeStyle": "solid",
        "roughness": 1,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "index": text_index,
        "roundness": null,
        "seed": seed2,
        "version": 1,
        "versionNonce": nonce2,
        "isDeleted": false,
        "boundElements": null,
        "updated": stamp,
        "link": null,
        "locked": false,
        "text": text,
        "originalText": text,
        "fontSize": font_size,
        "fontFamily": th_ref.font_family,
        "textAlign": "center",
        "verticalAlign": "middle",
        "containerId": shape_id,
        "lineHeight": 1.25
    });

    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    elements.push(shape_json);
    elements.push(text_json);

    Ok((shape_id, text_id))
}

pub fn mutate_add_text(
    doc: &mut Value,
    text: &str,
    at: (f64, f64),
    font_size_opt: Option<f64>,
    color: Option<&str>,
) -> Result<String, String> {
    let max_idx = max_index_of(doc);
    let minted = mint_indices(&max_idx, 1)?;
    let text_index = &minted[0];
    let text_id = new_id();

    let font_size = font_size_opt.unwrap_or(20.0);
    let (tw, th) = compute_text_dimensions(text, font_size);

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut rng = rand::rng();
    let seed: i32 = rng.random_range(1..=i32::MAX);
    let nonce: i32 = rng.random_range(1..=i32::MAX);

    let text_json = json!({
        "id": text_id,
        "type": "text",
        "x": at.0,
        "y": at.1,
        "width": tw,
        "height": th,
        "angle": 0,
        "strokeColor": color.unwrap_or("#1a1a1a"),
        "backgroundColor": "transparent",
        "fillStyle": "hachure",
        "strokeWidth": 1,
        "strokeStyle": "solid",
        "roughness": 1,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "index": text_index,
        "roundness": null,
        "seed": seed,
        "version": 1,
        "versionNonce": nonce,
        "isDeleted": false,
        "boundElements": null,
        "updated": stamp,
        "link": null,
        "locked": false,
        "text": text,
        "originalText": text,
        "fontSize": font_size,
        "fontFamily": 1,
        "textAlign": "left",
        "verticalAlign": "top",
        "containerId": null,
        "lineHeight": 1.25
    });

    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    elements.push(text_json);

    Ok(text_id)
}

pub fn mutate_connect(
    doc: &mut Value,
    from_id: &str,
    to_id: &str,
    label_opt: Option<&str>,
    color: Option<&str>,
    curved: Option<bool>,
    stroke_style: Option<&str>,
) -> Result<String, String> {
    let els = live(doc);
    let from_elem = els
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(from_id))
        .copied()
        .ok_or_else(|| format!("from element {from_id:?} not found"))?;
    let to_elem = els
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(to_id))
        .copied()
        .ok_or_else(|| format!("to element {to_id:?} not found"))?;

    let fx = from_elem.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let fy = from_elem.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let fw = from_elem.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let fh = from_elem.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let c1_x = fx + fw / 2.0;
    let c1_y = fy + fh / 2.0;

    let tx = to_elem.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ty = to_elem.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let tw = to_elem.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let th = to_elem.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let c2_x = tx + tw / 2.0;
    let c2_y = ty + th / 2.0;

    let dx = c2_x - c1_x;
    let dy = c2_y - c1_y;

    let arrow_id = new_id();
    let max_idx = max_index_of(doc);

    let has_label = label_opt.map(|l| !l.trim().is_empty()).unwrap_or(false);
    let minted = mint_indices(&max_idx, if has_label { 2 } else { 1 })?;
    let arrow_index = &minted[0];

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut rng = rand::rng();
    let seed1: i32 = rng.random_range(1..=i32::MAX);
    let nonce1: i32 = rng.random_range(1..=i32::MAX);

    let mut arrow_bound = Vec::new();
    let mut label_element = None;

    if has_label {
        let label_text = label_opt.unwrap();
        let label_id = new_id();
        let label_index = &minted[1];
        let font_size = 16.0;
        let (ltw, lth) = compute_text_dimensions(label_text, font_size);
        let lx = c1_x + dx / 2.0 - ltw / 2.0;
        let ly = c1_y + dy / 2.0 - lth / 2.0;

        let seed2: i32 = rng.random_range(1..=i32::MAX);
        let nonce2: i32 = rng.random_range(1..=i32::MAX);

        arrow_bound.push(json!({ "id": label_id, "type": "text" }));

        label_element = Some(json!({
            "id": label_id,
            "type": "text",
            "x": lx,
            "y": ly,
            "width": ltw,
            "height": lth,
            "angle": 0,
            "strokeColor": color.unwrap_or("#1a1a1a"),
            "backgroundColor": "transparent",
            "fillStyle": "hachure",
            "strokeWidth": 1,
            "strokeStyle": "solid",
            "roughness": 1,
            "opacity": 100,
            "groupIds": [],
            "frameId": null,
            "index": label_index,
            "roundness": null,
            "seed": seed2,
            "version": 1,
            "versionNonce": nonce2,
            "isDeleted": false,
            "boundElements": null,
            "updated": stamp,
            "link": null,
            "locked": false,
            "text": label_text,
            "originalText": label_text,
            "fontSize": font_size,
            "fontFamily": 1,
            "textAlign": "center",
            "verticalAlign": "middle",
            "containerId": arrow_id,
            "lineHeight": 1.25
        }));
    }

    let arrow_json = json!({
        "id": arrow_id,
        "type": "arrow",
        "x": c1_x,
        "y": c1_y,
        "width": dx.abs(),
        "height": dy.abs(),
        "angle": 0,
        "strokeColor": color.unwrap_or("#1a1a1a"),
        "backgroundColor": "transparent",
        "fillStyle": "hachure",
        "strokeWidth": 2,
        "strokeStyle": stroke_style.unwrap_or("solid"),
        "roughness": 2,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "index": arrow_index,
        "roundness": if curved.unwrap_or(false) { serde_json::json!({ "type": 3 }) } else { serde_json::json!({ "type": 2 }) },
        "seed": seed1,
        "version": 1,
        "versionNonce": nonce1,
        "isDeleted": false,
        "boundElements": if arrow_bound.is_empty() { Value::Null } else { Value::Array(arrow_bound) },
        "updated": stamp,
        "link": null,
        "locked": false,
        "points": [
            [0.0, 0.0],
            [dx, dy]
        ],
        "lastCommittedPoint": null,
        "startBinding": {
            "elementId": from_id,
            "focus": 0.0,
            "gap": 1.0
        },
        "endBinding": {
            "elementId": to_id,
            "focus": 0.0,
            "gap": 1.0
        },
        "startArrowhead": null,
        "endArrowhead": "arrow"
    });

    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    // Add arrow to from_elem's and to_elem's boundElements
    for e in elements.iter_mut() {
        let e_id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if e_id == from_id || e_id == to_id {
            if e.get("boundElements").is_none() || e["boundElements"].is_null() {
                e["boundElements"] = json!([]);
            }
            if let Some(barr) = e["boundElements"].as_array_mut() {
                barr.push(json!({ "id": arrow_id, "type": "arrow" }));
            }
        }
    }

    elements.push(arrow_json);
    if let Some(lbl) = label_element {
        elements.push(lbl);
    }

    Ok(arrow_id)
}

fn get_arrow_midpoint(arrow_elem: &Value) -> (f64, f64) {
    let ax = arrow_elem.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ay = arrow_elem.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if let Some(pts) = arrow_elem.get("points").and_then(|v| v.as_array()) {
        if pts.len() >= 2 {
            let p0 = pts.first().and_then(|v| v.as_array());
            let pend = pts.last().and_then(|v| v.as_array());
            if let (Some(p0), Some(pend)) = (p0, pend) {
                let p0_x = p0.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
                let p0_y = p0.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let pend_x = pend.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
                let pend_y = pend.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                return (ax + (p0_x + pend_x) / 2.0, ay + (p0_y + pend_y) / 2.0);
            }
        }
    }
    let aw = arrow_elem.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ah = arrow_elem.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    (ax + aw / 2.0, ay + ah / 2.0)
}

pub fn mutate_set_text(doc: &mut Value, id: &str, new_text: &str) -> Result<(), String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    // Check if `id` is a shape container, bound text, arrow, or free text
    let target_idx = elements
        .iter()
        .position(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
        .ok_or_else(|| format!("element {id:?} not found"))?;

    let e_type = elements[target_idx]
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let pad_x = 40.0;
    let pad_y = 30.0;

    if e_type == "text" {
        let container_id = elements[target_idx]
            .get("containerId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let font_size = elements[target_idx]
            .get("fontSize")
            .and_then(|v| v.as_f64())
            .unwrap_or(20.0);
        let (tw, th) = compute_text_dimensions(new_text, font_size);

        elements[target_idx]["text"] = json!(new_text);
        elements[target_idx]["originalText"] = json!(new_text);
        elements[target_idx]["width"] = json!(tw);
        elements[target_idx]["height"] = json!(th);

        if let Some(cid) = container_id {
            if let Some(cidx) = elements.iter().position(|e| e.get("id").and_then(|v| v.as_str()) == Some(&cid)) {
                let ctype = elements[cidx].get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if ctype == "arrow" {
                    let (mid_x, mid_y) = get_arrow_midpoint(&elements[cidx]);
                    elements[target_idx]["x"] = json!(mid_x - tw / 2.0);
                    elements[target_idx]["y"] = json!(mid_y - th / 2.0);
                } else {
                    let cw = elements[cidx].get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let ch = elements[cidx].get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let cx = elements[cidx].get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let cy = elements[cidx].get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

                    let old_mid_x = cx + cw / 2.0;
                    let old_mid_y = cy + ch / 2.0;

                    let new_cw = cw.max(tw + pad_x);
                    let new_ch = ch.max(th + pad_y);
                    let new_cx = old_mid_x - new_cw / 2.0;
                    let new_cy = old_mid_y - new_ch / 2.0;

                    elements[cidx]["x"] = json!(new_cx);
                    elements[cidx]["y"] = json!(new_cy);
                    elements[cidx]["width"] = json!(new_cw);
                    elements[cidx]["height"] = json!(new_ch);

                    elements[target_idx]["x"] = json!(new_cx + (new_cw - tw) / 2.0);
                    elements[target_idx]["y"] = json!(new_cy + (new_ch - th) / 2.0);
                }
            }
        }
    } else {
        // Shape or arrow container
        let bound_text_id = elements[target_idx]
            .get("boundElements")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                    .and_then(|b| b.get("id").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
            });

        if let Some(tid) = bound_text_id {
            if let Some(tidx) = elements.iter().position(|e| e.get("id").and_then(|v| v.as_str()) == Some(&tid)) {
                let font_size = elements[tidx]
                    .get("fontSize")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(20.0);
                let (tw, th) = compute_text_dimensions(new_text, font_size);

                elements[tidx]["text"] = json!(new_text);
                elements[tidx]["originalText"] = json!(new_text);
                elements[tidx]["width"] = json!(tw);
                elements[tidx]["height"] = json!(th);

                if e_type == "arrow" {
                    let (mid_x, mid_y) = get_arrow_midpoint(&elements[target_idx]);
                    elements[tidx]["x"] = json!(mid_x - tw / 2.0);
                    elements[tidx]["y"] = json!(mid_y - th / 2.0);
                } else {
                    let cw = elements[target_idx].get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let ch = elements[target_idx].get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let cx = elements[target_idx].get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let cy = elements[target_idx].get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

                    let old_mid_x = cx + cw / 2.0;
                    let old_mid_y = cy + ch / 2.0;

                    let new_cw = cw.max(tw + pad_x);
                    let new_ch = ch.max(th + pad_y);
                    let new_cx = old_mid_x - new_cw / 2.0;
                    let new_cy = old_mid_y - new_ch / 2.0;

                    elements[target_idx]["x"] = json!(new_cx);
                    elements[target_idx]["y"] = json!(new_cy);
                    elements[target_idx]["width"] = json!(new_cw);
                    elements[target_idx]["height"] = json!(new_ch);

                    elements[tidx]["x"] = json!(new_cx + (new_cw - tw) / 2.0);
                    elements[tidx]["y"] = json!(new_cy + (new_ch - th) / 2.0);
                }
            }
        } else {
            // Direct text attribute fallback
            elements[target_idx]["text"] = json!(new_text);
            elements[target_idx]["originalText"] = json!(new_text);
        }
    }

    Ok(())
}

pub fn mutate_move_elem(
    doc: &mut Value,
    id: &str,
    to: Option<(f64, f64)>,
    by: Option<(f64, f64)>,
) -> Result<(), String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let target_idx = elements
        .iter()
        .position(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
        .ok_or_else(|| format!("element {id:?} not found"))?;

    let curr_x = elements[target_idx].get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let curr_y = elements[target_idx].get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let (dx, dy) = if let Some((tx, ty)) = to {
        (tx - curr_x, ty - curr_y)
    } else if let Some((bx, by)) = by {
        (bx, by)
    } else {
        (0.0, 0.0)
    };

    // Find all elements to move: target + bound texts (or container if target is bound text)
    let mut moved_ids = HashSet::new();
    moved_ids.insert(id.to_string());

    if let Some(cid) = elements[target_idx].get("containerId").and_then(|v| v.as_str()) {
        moved_ids.insert(cid.to_string());
    }

    if let Some(bound_arr) = elements[target_idx].get("boundElements").and_then(|v| v.as_array()) {
        for b in bound_arr {
            if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                    moved_ids.insert(bid.to_string());
                }
            }
        }
    }

    // Shift positions
    for e in elements.iter_mut() {
        let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if moved_ids.contains(eid) {
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            e["x"] = json!(x + dx);
            e["y"] = json!(y + dy);
        }
    }

    // Adjust connected arrows and compute label translations
    let mut arrow_shifts: HashMap<String, (f64, f64)> = HashMap::new();
    let mut bound_text_shifts: HashMap<String, (f64, f64)> = HashMap::new();

    for e in elements.iter_mut() {
        if e.get("type").and_then(|v| v.as_str()) == Some("arrow") {
            let s_id = e.get("startBinding").and_then(|v| v.get("elementId")).and_then(|v| v.as_str()).unwrap_or("");
            let e_id = e.get("endBinding").and_then(|v| v.get("elementId")).and_then(|v| v.as_str()).unwrap_or("");

            let s_moved = moved_ids.contains(s_id);
            let e_moved = moved_ids.contains(e_id);

            let (label_dx, label_dy) = if s_moved && e_moved {
                let ax = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let ay = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                e["x"] = json!(ax + dx);
                e["y"] = json!(ay + dy);
                (dx, dy)
            } else if s_moved {
                let ax = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let ay = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                e["x"] = json!(ax + dx);
                e["y"] = json!(ay + dy);
                if let Some(pts) = e.get_mut("points").and_then(|v| v.as_array_mut()) {
                    if let Some(last_pt) = pts.last_mut().and_then(|v| v.as_array_mut()) {
                        if last_pt.len() >= 2 {
                            let px = last_pt[0].as_f64().unwrap_or(0.0);
                            let py = last_pt[1].as_f64().unwrap_or(0.0);
                            last_pt[0] = json!(px - dx);
                            last_pt[1] = json!(py - dy);
                            e["width"] = json!((px - dx).abs());
                            e["height"] = json!((py - dy).abs());
                        }
                    }
                }
                (dx / 2.0, dy / 2.0)
            } else if e_moved {
                if let Some(pts) = e.get_mut("points").and_then(|v| v.as_array_mut()) {
                    if let Some(last_pt) = pts.last_mut().and_then(|v| v.as_array_mut()) {
                        if last_pt.len() >= 2 {
                            let px = last_pt[0].as_f64().unwrap_or(0.0);
                            let py = last_pt[1].as_f64().unwrap_or(0.0);
                            last_pt[0] = json!(px + dx);
                            last_pt[1] = json!(py + dy);
                            e["width"] = json!((px + dx).abs());
                            e["height"] = json!((py + dy).abs());
                        }
                    }
                }
                (dx / 2.0, dy / 2.0)
            } else {
                (0.0, 0.0)
            };

            if label_dx != 0.0 || label_dy != 0.0 {
                let aid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
                arrow_shifts.insert(aid.to_string(), (label_dx, label_dy));
                if let Some(barr) = e.get("boundElements").and_then(|v| v.as_array()) {
                    for b in barr {
                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                                bound_text_shifts.insert(bid.to_string(), (label_dx, label_dy));
                            }
                        }
                    }
                }
            }
        }
    }

    // Shift bound arrow label texts
    if !arrow_shifts.is_empty() || !bound_text_shifts.is_empty() {
        for e in elements.iter_mut() {
            if e.get("type").and_then(|v| v.as_str()) != Some("text") {
                continue;
            }
            let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if moved_ids.contains(eid) {
                continue;
            }
            let shift = bound_text_shifts.get(eid).copied().or_else(|| {
                let cid = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");
                arrow_shifts.get(cid).copied()
            });
            if let Some((ldx, ldy)) = shift {
                let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                e["x"] = json!(x + ldx);
                e["y"] = json!(y + ldy);
            }
        }
    }

    Ok(())
}

pub fn mutate_delete_elem(doc: &mut Value, id: &str, cascade_arrows: bool) -> Result<(), String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let mut del_ids = HashSet::new();
    del_ids.insert(id.to_string());

    // Recursively expand del_ids for paired containers and bound texts
    let mut changed = true;
    while changed {
        changed = false;
        for e in elements.iter() {
            let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if del_ids.contains(eid) {
                if let Some(cid) = e.get("containerId").and_then(|v| v.as_str()) {
                    if !del_ids.contains(cid) {
                        del_ids.insert(cid.to_string());
                        changed = true;
                    }
                }
                if let Some(barr) = e.get("boundElements").and_then(|v| v.as_array()) {
                    for b in barr {
                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                                if !del_ids.contains(bid) {
                                    del_ids.insert(bid.to_string());
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            } else if let Some(cid) = e.get("containerId").and_then(|v| v.as_str()) {
                if del_ids.contains(cid) {
                    del_ids.insert(eid.to_string());
                    changed = true;
                }
            }
        }
    }

    // Connected arrows
    let mut unlinked_arrow_ids = HashSet::new();
    let mut arrow_ids_to_delete = Vec::new();

    for e in elements.iter_mut() {
        if e.get("type").and_then(|v| v.as_str()) == Some("arrow") {
            let s_id = e.get("startBinding").and_then(|v| v.get("elementId")).and_then(|v| v.as_str()).unwrap_or("");
            let e_id = e.get("endBinding").and_then(|v| v.get("elementId")).and_then(|v| v.as_str()).unwrap_or("");

            if (!s_id.is_empty() && del_ids.contains(s_id)) || (!e_id.is_empty() && del_ids.contains(e_id)) {
                let aid = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if cascade_arrows {
                    arrow_ids_to_delete.push(aid);
                } else {
                    // Unbind both ends to preserve 0-bound arrow invariant
                    e["startBinding"] = json!(null);
                    e["endBinding"] = json!(null);
                    unlinked_arrow_ids.insert(aid);
                }
            }
        }
    }

    if cascade_arrows {
        for aid in arrow_ids_to_delete {
            del_ids.insert(aid);
        }

        // Recursively include bound text elements attached to cascaded arrows
        let mut changed = true;
        while changed {
            changed = false;
            for e in elements.iter() {
                let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if del_ids.contains(eid) {
                    if let Some(barr) = e.get("boundElements").and_then(|v| v.as_array()) {
                        for b in barr {
                            if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                                if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                                    if !del_ids.contains(bid) {
                                        del_ids.insert(bid.to_string());
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(cid) = e.get("containerId").and_then(|v| v.as_str()) {
                    if del_ids.contains(cid) {
                        del_ids.insert(eid.to_string());
                        changed = true;
                    }
                }
            }
        }
    }

    // Clean up boundElements across all remaining elements:
    // Strip deleted element IDs and unlinked arrow IDs
    let mut ids_to_strip = del_ids.clone();
    for uaid in unlinked_arrow_ids {
        ids_to_strip.insert(uaid);
    }

    for e in elements.iter_mut() {
        if let Some(barr) = e.get_mut("boundElements").and_then(|v| v.as_array_mut()) {
            barr.retain(|b| {
                let bid = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                !ids_to_strip.contains(bid)
            });
            if barr.is_empty() {
                e["boundElements"] = json!(null);
            }
        }
    }

    // Delete elements
    elements.retain(|e| {
        let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        !del_ids.contains(eid)
    });

    Ok(())
}

pub fn mutate_theme_apply(
    doc: &mut Value,
    theme: &Theme,
    target_id: Option<&str>,
    all: bool,
) -> Result<usize, String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let mut target_ids = HashSet::new();
    if let Some(tid) = target_id {
        target_ids.insert(tid.to_string());
        // Cascade to paired container or bound text elements
        let mut changed = true;
        while changed {
            changed = false;
            for e in elements.iter() {
                let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if target_ids.contains(eid) {
                    if let Some(cid) = e.get("containerId").and_then(|v| v.as_str()) {
                        if !target_ids.contains(cid) {
                            target_ids.insert(cid.to_string());
                            changed = true;
                        }
                    }
                    if let Some(barr) = e.get("boundElements").and_then(|v| v.as_array()) {
                        for b in barr {
                            if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                                if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                                    if !target_ids.contains(bid) {
                                        target_ids.insert(bid.to_string());
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(cid) = e.get("containerId").and_then(|v| v.as_str()) {
                    if target_ids.contains(cid) {
                        target_ids.insert(eid.to_string());
                        changed = true;
                    }
                }
            }
        }
    }

    let mut applied_count = 0;

    for e in elements.iter_mut() {
        let eid = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let matches_id = target_ids.contains(eid);

        if all || target_id.is_none() || matches_id {
            let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if e_type == "text" {
                e["fontFamily"] = json!(theme.font_family);
                e["strokeColor"] = json!(theme.text_color);
                applied_count += 1;
            } else if e_type == "arrow" {
                e["strokeColor"] = json!(theme.stroke_color);
                e["roughness"] = json!(theme.roughness);
                applied_count += 1;
            } else {
                e["fillStyle"] = json!(theme.fill_style);
                e["roughness"] = json!(theme.roughness);
                e["strokeColor"] = json!(theme.stroke_color);

                if let Some(role) = e.get("customData").and_then(|v| v.get("role")).and_then(|v| v.as_str()) {
                    if let Some(bg) = theme.color_for_role(role) {
                        e["backgroundColor"] = json!(bg);
                    }
                }
                applied_count += 1;
            }
        }
    }

    Ok(applied_count)
}

pub fn mutate_align(
    doc: &mut Value,
    ids: &[String],
    alignment: &str,
) -> Result<usize, String> {
    if ids.len() < 2 {
        return Err("Need at least 2 elements to align".to_string());
    }

    let elements = doc
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "document missing elements array".to_string())?;

    struct TargetElem {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    let mut targets = Vec::new();
    for id in ids {
        if let Some(e) = elements.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)) {
            targets.push(TargetElem {
                id: id.clone(),
                x: e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
                y: e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
                width: e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0),
                height: e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0),
            });
        }
    }

    if targets.len() < 2 {
        return Err("Need at least 2 matching elements to align".to_string());
    }

    let mut moves: Vec<(String, f64, f64)> = Vec::new();

    match alignment {
        "left" => {
            let min_x = targets.iter().map(|t| t.x).fold(f64::INFINITY, f64::min);
            for t in &targets {
                moves.push((t.id.clone(), min_x - t.x, 0.0));
            }
        }
        "right" => {
            let max_right = targets.iter().map(|t| t.x + t.width).fold(f64::NEG_INFINITY, f64::max);
            for t in &targets {
                moves.push((t.id.clone(), max_right - (t.x + t.width), 0.0));
            }
        }
        "center" => {
            let centers: Vec<f64> = targets.iter().map(|t| t.x + t.width / 2.0).collect();
            let avg_center = centers.iter().sum::<f64>() / centers.len() as f64;
            for t in &targets {
                let target_x = avg_center - t.width / 2.0;
                moves.push((t.id.clone(), target_x - t.x, 0.0));
            }
        }
        "top" => {
            let min_y = targets.iter().map(|t| t.y).fold(f64::INFINITY, f64::min);
            for t in &targets {
                moves.push((t.id.clone(), 0.0, min_y - t.y));
            }
        }
        "bottom" => {
            let max_bottom = targets.iter().map(|t| t.y + t.height).fold(f64::NEG_INFINITY, f64::max);
            for t in &targets {
                moves.push((t.id.clone(), 0.0, max_bottom - (t.y + t.height)));
            }
        }
        "middle" => {
            let middles: Vec<f64> = targets.iter().map(|t| t.y + t.height / 2.0).collect();
            let avg_middle = middles.iter().sum::<f64>() / middles.len() as f64;
            for t in &targets {
                let target_y = avg_middle - t.height / 2.0;
                moves.push((t.id.clone(), 0.0, target_y - t.y));
            }
        }
        other => return Err(format!("Unknown alignment {other:?}; expected left|center|right|top|middle|bottom")),
    }

    let mut moved_count = 0;
    let mut moved_ids = HashSet::new();

    for (id, dx, dy) in moves {
        if moved_ids.contains(&id) {
            continue;
        }
        if dx.abs() > 0.0001 || dy.abs() > 0.0001 {
            mutate_move_elem(doc, &id, None, Some((dx, dy)))?;
        }
        moved_ids.insert(id);
        moved_count += 1;
    }

    Ok(moved_count)
}

pub fn mutate_distribute(
    doc: &mut Value,
    ids: &[String],
    direction: &str,
) -> Result<usize, String> {
    if ids.len() < 3 {
        return Err("Need at least 3 elements to distribute".to_string());
    }

    let elements = doc
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "document missing elements array".to_string())?;

    struct DistElem {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    let mut targets = Vec::new();
    for id in ids {
        if let Some(e) = elements.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)) {
            targets.push(DistElem {
                id: id.clone(),
                x: e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
                y: e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
                width: e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0),
                height: e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0),
            });
        }
    }

    if targets.len() < 3 {
        return Err("Need at least 3 matching elements to distribute".to_string());
    }

    let mut moves: Vec<(String, f64, f64)> = Vec::new();

    if direction == "horizontal" {
        targets.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let first = &targets[0];
        let last = &targets[targets.len() - 1];
        let total_span = (last.x + last.width) - first.x;
        let total_elem_w: f64 = targets.iter().map(|t| t.width).sum();
        let gap = (total_span - total_elem_w) / (targets.len() - 1) as f64;

        let mut curr_x = first.x;
        for t in &targets {
            moves.push((t.id.clone(), curr_x - t.x, 0.0));
            curr_x += t.width + gap;
        }
    } else if direction == "vertical" {
        targets.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
        let first = &targets[0];
        let last = &targets[targets.len() - 1];
        let total_span = (last.y + last.height) - first.y;
        let total_elem_h: f64 = targets.iter().map(|t| t.height).sum();
        let gap = (total_span - total_elem_h) / (targets.len() - 1) as f64;

        let mut curr_y = first.y;
        for t in &targets {
            moves.push((t.id.clone(), 0.0, curr_y - t.y));
            curr_y += t.height + gap;
        }
    } else {
        return Err(format!("Unknown direction {direction:?}; expected horizontal|vertical"));
    }

    let mut moved_count = 0;
    for (id, dx, dy) in moves {
        if dx.abs() > 0.0001 || dy.abs() > 0.0001 {
            mutate_move_elem(doc, &id, None, Some((dx, dy)))?;
        }
        moved_count += 1;
    }

    Ok(moved_count)
}

pub fn mutate_group(
    doc: &mut Value,
    ids: &[String],
) -> Result<String, String> {
    if ids.is_empty() {
        return Err("group requires at least one element ID".to_string());
    }

    let group_id = format!("grp_{}", new_id());
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let mut count = 0;
    for el in elements.iter_mut() {
        if let Some(id) = el.get("id").and_then(|v| v.as_str()) {
            if ids.contains(&id.to_string()) {
                if let Some(obj) = el.as_object_mut() {
                    let groups = obj.entry("groupIds").or_insert_with(|| json!([]));
                    if let Some(arr) = groups.as_array_mut() {
                        arr.push(json!(group_id));
                        count += 1;
                    }
                }
            }
        }
    }

    if count == 0 {
        return Err("No matching elements found to group".to_string());
    }

    Ok(group_id)
}

pub fn mutate_ungroup(
    doc: &mut Value,
    group_id: Option<&str>,
    ids: Option<&[String]>,
) -> Result<usize, String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let mut ungrouped_count = 0;

    for el in elements.iter_mut() {
        let el_id = el.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if let Some(target_ids) = ids {
            if !target_ids.contains(&el_id) {
                continue;
            }
        }

        if let Some(arr) = el.get_mut("groupIds").and_then(|v| v.as_array_mut()) {
            if let Some(gid) = group_id {
                let before = arr.len();
                arr.retain(|g| g.as_str() != Some(gid));
                if arr.len() < before {
                    ungrouped_count += 1;
                }
            } else if ids.is_some() {
                if !arr.is_empty() {
                    arr.clear();
                    ungrouped_count += 1;
                }
            }
        }
    }

    Ok(ungrouped_count)
}

pub fn mutate_lock(
    doc: &mut Value,
    ids: &[String],
    locked: bool,
) -> Result<usize, String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let mut target_set: HashSet<String> = ids.iter().cloned().collect();

    // Include bound text elements of targeted containers
    for el in elements.iter() {
        if let Some(id) = el.get("id").and_then(|v| v.as_str()) {
            if target_set.contains(id) {
                if let Some(bounds) = el.get("boundElements").and_then(|v| v.as_array()) {
                    for b in bounds {
                        if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                            target_set.insert(bid.to_string());
                        }
                    }
                }
            }
        }
    }

    let mut count = 0;
    for el in elements.iter_mut() {
        if let Some(id) = el.get("id").and_then(|v| v.as_str()) {
            if target_set.contains(id) {
                el["locked"] = json!(locked);
                count += 1;
            }
        }
    }

    Ok(count)
}

pub fn mutate_duplicate(
    doc: &mut Value,
    ids: &[String],
    offset: (f64, f64),
) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Err("duplicate requires at least one element ID".to_string());
    }

    let elements = doc
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "document missing elements array".to_string())?;

    // Gather all target elements and auto-include bound text elements
    let mut all_to_dup = Vec::new();
    for id in ids {
        if !all_to_dup.contains(id) {
            all_to_dup.push(id.clone());
        }
        if let Some(el) = elements.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)) {
            if let Some(bounds) = el.get("boundElements").and_then(|v| v.as_array()) {
                for b in bounds {
                    if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                            if !all_to_dup.contains(&bid.to_string()) {
                                all_to_dup.push(bid.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut id_map = HashMap::new();
    for id in &all_to_dup {
        id_map.insert(id.clone(), new_id());
    }

    let max_idx = max_index_of(doc);
    let new_indices = mint_indices(&max_idx, all_to_dup.len())?;

    let mut cloned_elements = Vec::new();
    for (i, old_id) in all_to_dup.iter().enumerate() {
        if let Some(el) = elements.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(old_id)) {
            let mut clone = el.clone();
            let new_elem_id = &id_map[old_id];
            clone["id"] = json!(new_elem_id);
            clone["index"] = json!(new_indices[i]);

            let curr_x = clone.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let curr_y = clone.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            clone["x"] = json!(curr_x + offset.0);
            clone["y"] = json!(curr_y + offset.1);

            // Remap containerId
            if let Some(cid) = clone.get("containerId").and_then(|v| v.as_str()) {
                if let Some(new_cid) = id_map.get(cid) {
                    clone["containerId"] = json!(new_cid);
                }
            }

            // Remap boundElements
            if let Some(bounds) = clone.get_mut("boundElements").and_then(|v| v.as_array_mut()) {
                for b in bounds.iter_mut() {
                    if let Some(bid) = b.get("id").and_then(|v| v.as_str()) {
                        if let Some(new_bid) = id_map.get(bid) {
                            b["id"] = json!(new_bid);
                        }
                    }
                }
            }

            // Remap arrow startBinding / endBinding
            if let Some(sb) = clone.get_mut("startBinding").and_then(|v| v.as_object_mut()) {
                if let Some(eid) = sb.get("elementId").and_then(|v| v.as_str()) {
                    if let Some(new_eid) = id_map.get(eid) {
                        sb.insert("elementId".to_string(), json!(new_eid));
                    }
                }
            }
            if let Some(eb) = clone.get_mut("endBinding").and_then(|v| v.as_object_mut()) {
                if let Some(eid) = eb.get("elementId").and_then(|v| v.as_str()) {
                    if let Some(new_eid) = id_map.get(eid) {
                        eb.insert("elementId".to_string(), json!(new_eid));
                    }
                }
            }

            cloned_elements.push(clone);
        }
    }

    let elements_mut = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .unwrap();

    let created_ids: Vec<String> = ids.iter().filter_map(|id| id_map.get(id).cloned()).collect();
    for cl in cloned_elements {
        elements_mut.push(cl);
    }

    Ok(created_ids)
}

pub fn mutate_update_elem(
    doc: &mut Value,
    target_id: &str,
    updates: &Value,
) -> Result<(), String> {
    let updates_obj = updates
        .as_object()
        .ok_or_else(|| "updates must be a JSON object".to_string())?;

    // Handle text update if provided
    if let Some(new_text) = updates_obj.get("text").and_then(|v| v.as_str()) {
        mutate_set_text(doc, target_id, new_text)?;
    }

    // Handle position update if provided
    let to_x = updates_obj.get("x").and_then(|v| v.as_f64());
    let to_y = updates_obj.get("y").and_then(|v| v.as_f64());
    if to_x.is_some() || to_y.is_some() {
        let elements = doc
            .get("elements")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "document missing elements array".to_string())?;
        let el = elements
            .iter()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(target_id))
            .ok_or_else(|| format!("element {target_id:?} not found"))?;
        let curr_x = el.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let curr_y = el.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tx = to_x.unwrap_or(curr_x);
        let ty = to_y.unwrap_or(curr_y);
        mutate_move_elem(doc, target_id, Some((tx, ty)), None)?;
    }

    // Handle other styling / geometry attributes directly
    let elements_mut = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;

    let el = elements_mut
        .iter_mut()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(target_id))
        .ok_or_else(|| format!("element {target_id:?} not found"))?;

    for (k, v) in updates_obj {
        if k == "text" || k == "x" || k == "y" || k == "id" {
            continue;
        }
        el[k] = v.clone();
    }

    Ok(())
}

pub fn mutate_clear(doc: &mut Value) -> Result<usize, String> {
    let elements = doc
        .get_mut("elements")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "document missing elements array".to_string())?;
    let count = elements.len();
    elements.clear();
    Ok(count)
}

// ============================================================================
// Commands Implementation
// ============================================================================

pub fn cmd_summary(doc: &Value) {
    let els = live(doc);
    let mut type_counts: Vec<(&str, usize)> = Vec::new();
    for e in &els {
        let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(pos) = type_counts.iter().position(|(k, _)| *k == t) {
            type_counts[pos].1 += 1;
        } else {
            type_counts.push((t, 1));
        }
    }
    println!(
        "elements: {}  types: {}",
        els.len(),
        format_str_counter(&type_counts)
    );

    if els.is_empty() {
        println!("extents: x [0, 0]  y [0, 0]");
    } else {
        let (min_x, min_y, w, h) = extent(&els);
        let max_x = min_x + w;
        let max_y = min_y + h;
        println!("extents: x [{min_x:.0}, {max_x:.0}]  y [{min_y:.0}, {max_y:.0}]");
    }

    let idx: Vec<Option<&str>> = els
        .iter()
        .map(|e| e.get("index").and_then(|v| v.as_str()))
        .collect();
    let non_empty_idx: Vec<&str> = idx
        .iter()
        .filter_map(|&opt| opt.filter(|s| !s.is_empty()))
        .collect();
    let max_idx = non_empty_idx.iter().max().copied().unwrap_or("");
    println!("max index: {max_idx}");

    let all_present = idx
        .iter()
        .all(|opt| opt.is_some() && !opt.unwrap().is_empty());
    let is_sorted = idx.windows(2).all(|w| w[0] <= w[1]);
    let ok = all_present && is_sorted;
    println!(
        "array order == index order: {}",
        if ok { "True" } else { "False" }
    );
}

pub fn cmd_map(doc: &Value) {
    let els = live(doc);
    let mut texts: HashMap<&str, &Value> = HashMap::new();
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts.insert(id, e);
            }
        }
    }

    for e in &els {
        let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let container_id = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");

        if e_type == "text" && !container_id.is_empty() {
            continue;
        }

        if e_type == "arrow" {
            let s = e
                .get("startBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("-");
            let t = e
                .get("endBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("-");
            let lbl = label_of(e, &texts);
            if !lbl.is_empty() {
                println!("{id}\tarrow\t{s} -> {t}\t{lbl}");
            } else {
                println!("{id}\tarrow\t{s} -> {t}");
            }
        } else if e_type == "text" {
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let body = element_text(e).replace('\n', " / ");
            println!("{id}\ttext\t{x:.0},{y:.0}\t{body}");
        } else {
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let geo = format!("{x:.0},{y:.0}\t{w:.0}x{h:.0}");
            let bg = match e.get("backgroundColor") {
                Some(Value::String(s)) => s.as_str(),
                Some(Value::Null) | None => "null",
                Some(other) => other.as_str().unwrap_or("null"),
            };
            let lbl = label_of(e, &texts);
            println!("{id}\t{e_type}\t{geo}\t{bg}\t{lbl}");
        }
    }
}

pub fn cmd_nodes(doc: &Value) {
    let (nodes, _) = get_logical_nodes_and_edges(doc);
    for n in nodes {
        let lbl = n.label.replace('\n', " / ");
        println!(
            "{}\t{}\t{:.0},{:.0}\t{:.0}x{:.0}\t{}\t{}",
            n.id, n.elem_type, n.x, n.y, n.width, n.height, n.role, lbl
        );
    }
}

pub fn cmd_edges(doc: &Value) {
    let (_, edges) = get_logical_nodes_and_edges(doc);
    for e in edges {
        let lbl = e.label.replace('\n', " / ");
        if !lbl.is_empty() {
            println!("{}\t{} -> {}\t{}", e.id, e.from_id, e.to_id, lbl);
        } else {
            println!("{}\t{} -> {}", e.id, e.from_id, e.to_id);
        }
    }
}

pub fn cmd_inspect(doc: &Value, id: &str) {
    let els = live(doc);
    let elem = els.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id));
    if elem.is_none() {
        eprintln!("element {id:?} not found");
        process::exit(1);
    }
    let e = elem.unwrap();
    let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");

    let (nodes, edges) = get_logical_nodes_and_edges(doc);

    if let Some(n) = nodes.iter().find(|n| n.id == id) {
        let inbound: Vec<&str> = edges
            .iter()
            .filter(|e| e.to_id == id)
            .map(|e| e.id.as_str())
            .collect();
        let outbound: Vec<&str> = edges
            .iter()
            .filter(|e| e.from_id == id)
            .map(|e| e.id.as_str())
            .collect();

        println!("id: {}", n.id);
        println!("type: {}", n.elem_type);
        println!("bounds: {:.0},{:.0} {:.0}x{:.0}", n.x, n.y, n.width, n.height);
        println!("label: {}", n.label);
        println!("role: {}", n.role);
        println!("inbound: [{}]", inbound.join(", "));
        println!("outbound: [{}]", outbound.join(", "));
    } else if let Some(edge) = edges.iter().find(|edge| edge.id == id) {
        println!("id: {}", edge.id);
        println!("type: arrow");
        println!("from: {}", edge.from_id);
        println!("to: {}", edge.to_id);
        println!("label: {}", edge.label);
    } else {
        let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let text = element_text(e);
        let cid = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");
        println!("id: {id}");
        println!("type: {e_type}");
        if !cid.is_empty() {
            println!("container: {cid}");
        }
        println!("bounds: {x:.0},{y:.0} {w:.0}x{h:.0}");
        println!("label: {text}");
    }
}

pub fn cmd_get(doc: &Value, id: &str) {
    let els = live(doc);
    let elem = els.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id));
    if let Some(e) = elem {
        println!("{}", serde_json::to_string_pretty(e).unwrap());
    } else {
        eprintln!("element {id:?} not found");
        process::exit(1);
    }
}

pub fn cmd_style(doc: &Value) {
    let els = live(doc);
    let keys = [
        "roughness",
        "fillStyle",
        "strokeStyle",
        "strokeWidth",
        "fontFamily",
        "fontSize",
        "opacity",
    ];

    for key in keys {
        let mut counts: Vec<(&Value, usize)> = Vec::new();
        for e in &els {
            if let Some(v) = e.get(key) {
                if !v.is_null() {
                    if let Some(pos) = counts.iter().position(|(k, _)| *k == v) {
                        counts[pos].1 += 1;
                    } else {
                        counts.push((v, 1));
                    }
                }
            }
        }
        if !counts.is_empty() {
            let modal_val = counts
                .iter()
                .max_by_key(|(_, c)| *c)
                .map(|(v, _)| *v)
                .unwrap();
            let modal_str = format_modal_val(modal_val);
            let all_str = format_val_counter(&counts);
            println!("{key}: modal={modal_str}  all={all_str}");
        }
    }

    let mut rd_counts: Vec<(String, usize)> = Vec::new();
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("rectangle") {
            let rd_json = match e.get("roundness") {
                Some(v) if !v.is_null() => {
                    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
                }
                _ => "null".to_string(),
            };
            if let Some(pos) = rd_counts.iter().position(|(k, _)| k == &rd_json) {
                rd_counts[pos].1 += 1;
            } else {
                rd_counts.push((rd_json, 1));
            }
        }
    }
    if !rd_counts.is_empty() {
        let modal = rd_counts
            .iter()
            .max_by_key(|(_, c)| *c)
            .map(|(k, _)| k)
            .unwrap();
        println!("roundness (rectangles): modal={modal}");
    }
}

pub fn cmd_check(doc: &Value) -> Result<String, Vec<String>> {
    let els = live(doc);
    let ids: Vec<&str> = els
        .iter()
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
        .collect();
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for e in &els {
        if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
            by_id.insert(id, e);
        }
    }

    let mut fails: Vec<String> = Vec::new();

    // 1. Duplicate IDs
    let mut id_counts: Vec<(&str, usize)> = Vec::new();
    for &id in &ids {
        if let Some(pos) = id_counts.iter().position(|(k, _)| *k == id) {
            id_counts[pos].1 += 1;
        } else {
            id_counts.push((id, 1));
        }
    }
    let dup_ids: Vec<&str> = id_counts
        .iter()
        .filter(|(_, c)| *c > 1)
        .map(|(k, _)| *k)
        .collect();
    if !dup_ids.is_empty() {
        fails.push(format!("duplicate ids: {}", format_str_list(&dup_ids)));
    }

    // 2. Elements missing index
    let mut missing_index_ids: Vec<&str> = Vec::new();
    for e in &els {
        let idx = e.get("index").and_then(|v| v.as_str());
        if idx.is_none() || idx.unwrap().is_empty() {
            missing_index_ids.push(e.get("id").and_then(|v| v.as_str()).unwrap_or(""));
        }
    }
    if !missing_index_ids.is_empty() {
        fails.push(format!(
            "elements missing index: {}",
            format_str_list(&missing_index_ids)
        ));
    } else {
        // 3. Elements array not sorted by index
        let idx_list: Vec<&str> = els
            .iter()
            .map(|e| e.get("index").and_then(|v| v.as_str()).unwrap())
            .collect();
        let is_sorted = idx_list.windows(2).all(|w| w[0] <= w[1]);
        if !is_sorted {
            fails.push("elements array is not sorted by index — file will not open".to_string());
        }
    }

    // 4. Duplicate indices
    let mut index_counts: Vec<(&str, usize)> = Vec::new();
    for e in &els {
        if let Some(idx) = e
            .get("index")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            if let Some(pos) = index_counts.iter().position(|(k, _)| *k == idx) {
                index_counts[pos].1 += 1;
            } else {
                index_counts.push((idx, 1));
            }
        }
    }
    let dup_indices: Vec<&str> = index_counts
        .iter()
        .filter(|(_, c)| *c > 1)
        .map(|(k, _)| *k)
        .collect();
    if !dup_indices.is_empty() {
        fails.push(format!(
            "duplicate indices: {}",
            format_str_list(&dup_indices)
        ));
    }

    // 5. Half-bound arrows (BUG FIX: task_737c102e)
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("arrow") {
            let s_id = e
                .get("startBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let e_id = e
                .get("endBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            if (s_id.is_some() && e_id.is_none()) || (s_id.is_none() && e_id.is_some()) {
                let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let s_ref = s_id.unwrap_or("none");
                let e_ref = e_id.unwrap_or("none");
                fails.push(format!(
                    "arrow {id} is half-bound: startBinding={s_ref}, endBinding={e_ref}"
                ));
            }
        }
    }

    // 6. Dangling bindings, containers, boundElements, text mismatches
    for e in &els {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");

        for side in ["startBinding", "endBinding"] {
            if let Some(ref_id) = e
                .get(side)
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if !by_id.contains_key(ref_id) {
                    fails.push(format!("{id}.{side} -> missing element {ref_id}"));
                }
            }
        }

        if let Some(cid) = e
            .get("containerId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            if let Some(container) = by_id.get(cid) {
                let bound_ids: Vec<&str> = container
                    .get("boundElements")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|b| b.get("id").and_then(|v| v.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();
                if !bound_ids.contains(&id) {
                    fails.push(format!(
                        "container {cid} lacks boundElements backref to text {id}"
                    ));
                }
            } else {
                fails.push(format!("text {id} bound to missing container {cid}"));
            }
        }

        if let Some(bound_arr) = e.get("boundElements").and_then(|v| v.as_array()) {
            for b in bound_arr {
                if let Some(b_id) = b
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    if !by_id.contains_key(b_id) {
                        fails.push(format!("{id}.boundElements -> missing element {b_id}"));
                    }
                }
            }
        }

        let t = e.get("text").and_then(|v| v.as_str());
        let o = e.get("originalText").and_then(|v| v.as_str());
        if let (Some(t_str), Some(o_str)) = (t, o) {
            let t_words: Vec<&str> = t_str.split_whitespace().collect();
            let o_words: Vec<&str> = o_str.split_whitespace().collect();
            if t_words != o_words {
                let t_short = truncate_safe(t_str, 60);
                let o_short = truncate_safe(o_str, 60);
                fails.push(format!(
                    "{id}: text and originalText disagree in content, not just wrapping — one layer is invisible and will be overwritten by the other on next layout (text={t_short:?} originalText={o_short:?})"
                ));
            }
        }
    }

    if !fails.is_empty() {
        Err(fails)
    } else {
        Ok(format!(
            "OK: {} elements, ids unique, index-sorted, all bindings resolve",
            els.len()
        ))
    }
}

fn bg_repr(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "None".to_string(),
        Some(other) => other.to_string(),
    }
}

pub fn cmd_diff(doc1: &Value, doc2: &Value) {
    let els1 = live(doc1);
    let els2 = live(doc2);

    let mut by_id1: HashMap<&str, &Value> = HashMap::new();
    for e in &els1 {
        if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
            by_id1.insert(id, e);
        }
    }
    let mut by_id2: HashMap<&str, &Value> = HashMap::new();
    for e in &els2 {
        if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
            by_id2.insert(id, e);
        }
    }

    let mut texts1: HashMap<&str, &Value> = HashMap::new();
    for e in &els1 {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts1.insert(id, e);
            }
        }
    }
    let mut texts2: HashMap<&str, &Value> = HashMap::new();
    for e in &els2 {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts2.insert(id, e);
            }
        }
    }

    let ids1: BTreeSet<&str> = by_id1.keys().copied().collect();
    let ids2: BTreeSet<&str> = by_id2.keys().copied().collect();

    let removed_ids: BTreeSet<&str> = ids1.difference(&ids2).copied().collect();
    let added_ids: BTreeSet<&str> = ids2.difference(&ids1).copied().collect();
    let common_ids: BTreeSet<&str> = ids1.intersection(&ids2).copied().collect();

    let delta = els2.len() as isize - els1.len() as isize;
    println!(
        "Summary Diff: {} elements -> {} elements (delta: {:+})",
        els1.len(),
        els2.len(),
        delta
    );

    // Type histogram diff
    let mut t1: HashMap<&str, usize> = HashMap::new();
    for e in &els1 {
        let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        *t1.entry(t).or_insert(0) += 1;
    }
    let mut t2: HashMap<&str, usize> = HashMap::new();
    for e in &els2 {
        let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        *t2.entry(t).or_insert(0) += 1;
    }
    let mut all_types: BTreeSet<&str> = t1.keys().copied().collect();
    all_types.extend(t2.keys().copied());

    let mut type_diffs: Vec<String> = Vec::new();
    for t in all_types {
        let c1 = t1.get(t).copied().unwrap_or(0);
        let c2 = t2.get(t).copied().unwrap_or(0);
        if c1 != c2 {
            type_diffs.push(format!("  {t}: {c1} -> {c2} ({:+})", c2 as isize - c1 as isize));
        }
    }
    if !type_diffs.is_empty() {
        println!("Type histogram changes:");
        for td in type_diffs {
            println!("{td}");
        }
    }

    // Color histogram diff
    let mut bg1: HashMap<&str, usize> = HashMap::new();
    for e in &els1 {
        if let Some(bg) = e
            .get("backgroundColor")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            *bg1.entry(bg).or_insert(0) += 1;
        }
    }
    let mut bg2: HashMap<&str, usize> = HashMap::new();
    for e in &els2 {
        if let Some(bg) = e
            .get("backgroundColor")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            *bg2.entry(bg).or_insert(0) += 1;
        }
    }
    let mut all_bgs: BTreeSet<&str> = bg1.keys().copied().collect();
    all_bgs.extend(bg2.keys().copied());

    let mut bg_diffs: Vec<String> = Vec::new();
    for bg in all_bgs {
        let c1 = bg1.get(bg).copied().unwrap_or(0);
        let c2 = bg2.get(bg).copied().unwrap_or(0);
        if c1 != c2 {
            bg_diffs.push(format!("  {bg}: {c1} -> {c2} ({:+})", c2 as isize - c1 as isize));
        }
    }
    if !bg_diffs.is_empty() {
        println!("Color histogram changes:");
        for bd in bg_diffs {
            println!("{bd}");
        }
    }

    // Disappeared elements
    if !removed_ids.is_empty() {
        println!("\nDisappeared elements ({}):", removed_ids.len());
        for rid in &removed_ids {
            let e = by_id1[rid];
            let mut lbl = label_of(e, &texts1);
            if lbl.is_empty() {
                lbl = element_text(e).replace('\n', " / ");
            }
            let lbl_str = if !lbl.is_empty() {
                format!(" label={lbl:?}")
            } else {
                String::new()
            };
            let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            println!("  - [{e_type} {rid}]{lbl_str} pos=({x:.0},{y:.0})");
        }
    }

    // Added elements
    if !added_ids.is_empty() {
        println!("\nAdded elements ({}):", added_ids.len());
        for aid in &added_ids {
            let e = by_id2[aid];
            let mut lbl = label_of(e, &texts2);
            if lbl.is_empty() {
                lbl = element_text(e).replace('\n', " / ");
            }
            let lbl_str = if !lbl.is_empty() {
                format!(" label={lbl:?}")
            } else {
                String::new()
            };
            let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            println!("  - [{e_type} {aid}]{lbl_str} pos=({x:.0},{y:.0})");
        }
    }

    // Modified elements
    let mut mods: Vec<String> = Vec::new();
    for cid in &common_ids {
        let e1 = by_id1[cid];
        let e2 = by_id2[cid];
        let mut changes: Vec<String> = Vec::new();

        let t1 = element_text(e1);
        let t2 = element_text(e2);
        if t1 != t2 {
            changes.push(format!("text: {t1:?} -> {t2:?}"));
        }

        let x1 = e1.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y1 = e1.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let x2 = e2.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y2 = e2.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let dx = (x1 - x2).abs();
        let dy = (y1 - y2).abs();
        if dx > 1.0 || dy > 1.0 {
            changes.push(format!(
                "position shifted by ({:+.0},{:+.0})",
                x2 - x1,
                y2 - y1
            ));
        }

        let w1 = e1.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h1 = e1.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w2 = e2.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h2 = e2.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let dw = (w1 - w2).abs();
        let dh = (h1 - h2).abs();
        if dw > 1.0 || dh > 1.0 {
            changes.push(format!("size: {w1:.0}x{h1:.0} -> {w2:.0}x{h2:.0}"));
        }

        let bg1_val = e1.get("backgroundColor");
        let bg2_val = e2.get("backgroundColor");
        if bg1_val != bg2_val {
            let bg1_str = bg_repr(bg1_val);
            let bg2_str = bg_repr(bg2_val);
            changes.push(format!("bg: {bg1_str} -> {bg2_str}"));
        }

        if !changes.is_empty() {
            let mut lbl = label_of(e1, &texts1);
            if lbl.is_empty() {
                lbl = element_text(e1).replace('\n', " / ");
            }
            if lbl.is_empty() {
                lbl = cid.to_string();
            }
            let e_type = e1.get("type").and_then(|v| v.as_str()).unwrap_or("");
            mods.push(format!(
                "  - [{e_type} {cid}] ({lbl:?}): {}",
                changes.join("; ")
            ));
        }
    }
    if !mods.is_empty() {
        println!("\nModified elements ({}):", mods.len());
        for m in mods {
            println!("{m}");
        }
    }

    // Topology / Arrow binding changes
    let mut topology_changes: Vec<String> = Vec::new();
    for cid in &common_ids {
        let e1 = by_id1[cid];
        let e2 = by_id2[cid];
        if e1.get("type").and_then(|v| v.as_str()) == Some("arrow") {
            let sb1 = e1
                .get("startBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str());
            let sb2 = e2
                .get("startBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str());
            let eb1 = e1
                .get("endBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str());
            let eb2 = e2
                .get("endBinding")
                .and_then(|v| v.get("elementId"))
                .and_then(|v| v.as_str());
            if sb1 != sb2 || eb1 != eb2 {
                let sb1_str = sb1.unwrap_or("None");
                let sb2_str = sb2.unwrap_or("None");
                let eb1_str = eb1.unwrap_or("None");
                let eb2_str = eb2.unwrap_or("None");
                topology_changes.push(format!(
                    "  - [arrow {cid}] start: {sb1_str} -> {sb2_str}, end: {eb1_str} -> {eb2_str}"
                ));
            }
        }
    }
    if !topology_changes.is_empty() {
        println!(
            "\nTopology / Arrow binding changes ({}):",
            topology_changes.len()
        );
        for tc in topology_changes {
            println!("{tc}");
        }
    }
}

pub fn cmd_struct_diff(doc1: &Value, doc2: &Value) -> Vec<String> {
    let (nodes1, edges1) = get_logical_nodes_and_edges(doc1);
    let (nodes2, edges2) = get_logical_nodes_and_edges(doc2);

    let mut n1_map: HashMap<&str, &LogicalNode> = HashMap::new();
    for n in &nodes1 {
        n1_map.insert(&n.id, n);
    }
    let mut n2_map: HashMap<&str, &LogicalNode> = HashMap::new();
    for n in &nodes2 {
        n2_map.insert(&n.id, n);
    }

    let mut e1_map: HashMap<&str, &LogicalEdge> = HashMap::new();
    for e in &edges1 {
        e1_map.insert(&e.id, e);
    }
    let mut e2_map: HashMap<&str, &LogicalEdge> = HashMap::new();
    for e in &edges2 {
        e2_map.insert(&e.id, e);
    }

    let mut diffs = Vec::new();

    // Added nodes
    for n2 in &nodes2 {
        if !n1_map.contains_key(n2.id.as_str()) {
            let role_str = if !n2.role.is_empty() {
                format!(" role=\"{}\"", n2.role)
            } else {
                String::new()
            };
            diffs.push(format!(
                "+ node [{}] type=\"{}\" label=\"{}\"{}",
                n2.id, n2.elem_type, n2.label, role_str
            ));
        }
    }

    // Removed nodes
    for n1 in &nodes1 {
        if !n2_map.contains_key(n1.id.as_str()) {
            diffs.push(format!(
                "- node [{}] type=\"{}\" label=\"{}\"",
                n1.id, n1.elem_type, n1.label
            ));
        }
    }

    // Modified nodes
    for n1 in &nodes1 {
        if let Some(n2) = n2_map.get(n1.id.as_str()) {
            if n1.label != n2.label {
                diffs.push(format!(
                    "~ node [{}] relabeled: \"{}\" -> \"{}\"",
                    n1.id, n1.label, n2.label
                ));
            }
            if n1.role != n2.role {
                diffs.push(format!(
                    "~ node [{}] role: \"{}\" -> \"{}\"",
                    n1.id, n1.role, n2.role
                ));
            }
        }
    }

    // Added edges
    for e2 in &edges2 {
        if !e1_map.contains_key(e2.id.as_str()) {
            let lbl_str = if !e2.label.is_empty() {
                format!(" (label=\"{}\")", e2.label)
            } else {
                String::new()
            };
            diffs.push(format!(
                "+ edge [{}] {} -> {}{}",
                e2.id, e2.from_id, e2.to_id, lbl_str
            ));
        }
    }

    // Removed edges
    for e1 in &edges1 {
        if !e2_map.contains_key(e1.id.as_str()) {
            let lbl_str = if !e1.label.is_empty() {
                format!(" (label=\"{}\")", e1.label)
            } else {
                String::new()
            };
            diffs.push(format!(
                "- edge [{}] {} -> {}{}",
                e1.id, e1.from_id, e1.to_id, lbl_str
            ));
        }
    }

    // Modified edges
    for e1 in &edges1 {
        if let Some(e2) = e2_map.get(e1.id.as_str()) {
            if e1.from_id != e2.from_id || e1.to_id != e2.to_id {
                diffs.push(format!(
                    "~ edge [{}] retargeted: {}->{} -> {}->{}",
                    e1.id, e1.from_id, e1.to_id, e2.from_id, e2.to_id
                ));
            }
            if e1.label != e2.label {
                diffs.push(format!(
                    "~ edge [{}] relabeled: \"{}\" -> \"{}\"",
                    e1.id, e1.label, e2.label
                ));
            }
        }
    }

    diffs
}

pub fn cmd_overlap(doc: &Value) -> Result<(), Vec<String>> {
    let els = live(doc);
    let mut texts: HashMap<&str, &Value> = HashMap::new();
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts.insert(id, e);
            }
        }
    }

    struct BoxItem<'a> {
        id: &'a str,
        label: String,
        rect: (f64, f64, f64, f64),
    }

    let mut boxes = Vec::new();

    for e in &els {
        let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let container_id = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");

        if e_type == "arrow" || (e_type == "text" && !container_id.is_empty()) {
            continue;
        }

        let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let mut lbl = label_of(e, &texts);
        if lbl.is_empty() {
            lbl = element_text(e).to_string();
        }

        boxes.push(BoxItem {
            id,
            label: lbl,
            rect: (x, y, w, h),
        });
    }

    let mut collisions = Vec::new();

    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let b1 = &boxes[i];
            let b2 = &boxes[j];

            // Ignore nested boxes (e.g. parent container surrounding children)
            if rect_contains(b1.rect, b2.rect) || rect_contains(b2.rect, b1.rect) {
                continue;
            }

            if rects_overlap(b1.rect, b2.rect) {
                collisions.push(format!(
                    "{} ({}) overlaps with {} ({}) [{:.0},{:.0} {:.0}x{:.0} vs {:.0},{:.0} {:.0}x{:.0}]",
                    b1.id, b1.label, b2.id, b2.label,
                    b1.rect.0, b1.rect.1, b1.rect.2, b1.rect.3,
                    b2.rect.0, b2.rect.1, b2.rect.2, b2.rect.3
                ));
            }
        }
    }

    if collisions.is_empty() {
        Ok(())
    } else {
        Err(collisions)
    }
}

pub fn cmd_arrows_check(doc: &Value) -> Result<(), Vec<String>> {
    let els = live(doc);
    let mut texts: HashMap<&str, &Value> = HashMap::new();
    for e in &els {
        if e.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                texts.insert(id, e);
            }
        }
    }

    struct ShapeItem<'a> {
        id: &'a str,
        label: String,
        rect: (f64, f64, f64, f64),
    }

    let mut shapes = Vec::new();
    let mut arrows = Vec::new();

    for e in &els {
        let e_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let container_id = e.get("containerId").and_then(|v| v.as_str()).unwrap_or("");

        if e_type == "arrow" {
            arrows.push(e);
        } else if e_type != "text" || container_id.is_empty() {
            let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

            let mut lbl = label_of(e, &texts);
            if lbl.is_empty() {
                lbl = element_text(e).to_string();
            }

            shapes.push(ShapeItem {
                id,
                label: lbl,
                rect: (x, y, w, h),
            });
        }
    }

    // Background zones: shapes containing >= 3 other shapes are ignored
    let mut bg_zone_ids = HashSet::new();
    for s1 in &shapes {
        let mut contained_count = 0;
        for s2 in &shapes {
            if s1.id != s2.id && rect_contains(s1.rect, s2.rect) {
                contained_count += 1;
            }
        }
        if contained_count >= 3 {
            bg_zone_ids.insert(s1.id);
        }
    }

    let mut collisions = Vec::new();

    for arrow in &arrows {
        let aid = arrow.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let s_id = arrow
            .get("startBinding")
            .and_then(|v| v.get("elementId"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let e_id = arrow
            .get("endBinding")
            .and_then(|v| v.get("elementId"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let ax = arrow.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let ay = arrow.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let points = arrow.get("points").and_then(|v| v.as_array());
        if points.is_none() {
            continue;
        }
        let pts = points.unwrap();
        if pts.len() < 2 {
            continue;
        }

        let first = pts[0].as_array();
        let last = pts.last().and_then(|v| v.as_array());
        if first.is_none() || last.is_none() {
            continue;
        }
        let (p1x, p1y) = (
            ax + first.unwrap()[0].as_f64().unwrap_or(0.0),
            ay + first.unwrap()[1].as_f64().unwrap_or(0.0),
        );
        let (p2x, p2y) = (
            ax + last.unwrap()[0].as_f64().unwrap_or(0.0),
            ay + last.unwrap()[1].as_f64().unwrap_or(0.0),
        );

        for shape in &shapes {
            if shape.id == s_id || shape.id == e_id || bg_zone_ids.contains(shape.id) {
                continue;
            }

            // Margin of 1.0 to avoid boundary grazing
            let test_rect = (
                shape.rect.0 + 1.0,
                shape.rect.1 + 1.0,
                (shape.rect.2 - 2.0).max(0.1),
                (shape.rect.3 - 2.0).max(0.1),
            );

            if segment_intersects_rect((p1x, p1y), (p2x, p2y), test_rect) {
                collisions.push(format!(
                    "arrow {aid} cuts through box {} ({})",
                    shape.id, shape.label
                ));
            }
        }
    }

    if collisions.is_empty() {
        Ok(())
    } else {
        Err(collisions)
    }
}

pub fn lib_items(doc: &Value) -> Vec<(String, Vec<Value>)> {
    if let Some(items) = doc.get("libraryItems").and_then(|v| v.as_array()) {
        items
            .iter()
            .enumerate()
            .map(|(n, it)| {
                let name = it
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("(unnamed {n})"));
                let elements = it
                    .get("elements")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                (name, elements)
            })
            .collect()
    } else if let Some(lib) = doc.get("library").and_then(|v| v.as_array()) {
        lib.iter()
            .map(|els| {
                let elements = els.as_array().cloned().unwrap_or_default();
                ("(unnamed)".to_string(), elements)
            })
            .collect()
    } else {
        Vec::new()
    }
}

pub fn cmd_lib(doc: &Value) {
    let items = lib_items(doc);
    let version = doc
        .get("version")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".to_string());
    let source = doc.get("source").and_then(|v| v.as_str()).unwrap_or("?");
    println!(
        "{} items  (format v{version}, source {source})",
        items.len()
    );
    for (n, (name, els)) in items.iter().enumerate() {
        let val_refs: Vec<&Value> = els.iter().collect();
        let (_, _, w, h) = if els.is_empty() {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            extent(&val_refs)
        };
        let mut type_set: BTreeSet<&str> = BTreeSet::new();
        for e in els {
            if let Some(t) = e.get("type").and_then(|v| v.as_str()) {
                type_set.insert(t);
            }
        }
        let types = type_set.into_iter().collect::<Vec<_>>().join("+");
        println!("#{n}\t{name}\t{} els\t{w:.0}x{h:.0}\t{types}", els.len());
    }
}

pub fn cmd_item(doc: &Value, selector: &str, after: &str, at: Option<(f64, f64)>) {
    let items = lib_items(doc);
    let picked: Vec<(String, Vec<Value>)> = if let Some(num_str) = selector.strip_prefix('#') {
        if let Ok(idx) = num_str.parse::<usize>() {
            if idx < items.len() {
                vec![items[idx].clone()]
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    } else {
        items
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&selector.to_lowercase()))
            .cloned()
            .collect()
    };

    if picked.len() != 1 {
        let names = items
            .iter()
            .enumerate()
            .map(|(n, (name, _))| format!("#{n} {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "selector {selector:?} matched {} items; choose one of: {names}",
            picked.len()
        );
        process::exit(1);
    }

    let mut els = picked[0].1.clone();
    if els.is_empty() {
        eprintln!("library item {selector:?} has no elements");
        process::exit(1);
    }

    let mut ids: HashMap<String, String> = HashMap::new();
    for e in &els {
        if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
            ids.insert(id.to_string(), new_id());
        }
    }

    let mut groups: HashMap<String, String> = HashMap::new();
    for e in &els {
        if let Some(group_ids) = e.get("groupIds").and_then(|v| v.as_array()) {
            for g in group_ids {
                if let Some(g_str) = g.as_str() {
                    groups.entry(g_str.to_string()).or_insert_with(new_id);
                }
            }
        }
    }

    let mut dx = 0.0;
    let mut dy = 0.0;
    if let Some((at_x, at_y)) = at {
        let val_refs: Vec<&Value> = els.iter().collect();
        let (x0, y0, _, _) = extent(&val_refs);
        dx = at_x - x0;
        dy = at_y - y0;
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let minted = match mint_indices(after, els.len()) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };

    let mut rng = rand::rng();

    for (e, index) in els.iter_mut().zip(minted.iter()) {
        if let Some(old_id) = e.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            if let Some(new_id) = ids.get(&old_id) {
                e["id"] = Value::String(new_id.clone());
            }
        }

        let curr_x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let curr_y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        e["x"] = json!(curr_x + dx);
        e["y"] = json!(curr_y + dy);

        e["index"] = Value::String(index.clone());

        let seed: i32 = rng.random_range(1..=i32::MAX);
        let version_nonce: i32 = rng.random_range(1..=i32::MAX);
        e["seed"] = json!(seed);
        e["versionNonce"] = json!(version_nonce);
        e["version"] = json!(1);
        e["updated"] = json!(stamp);
        e["isDeleted"] = json!(false);

        if let Some(g_arr) = e.get("groupIds").and_then(|v| v.as_array()) {
            let new_g_arr: Vec<Value> = g_arr
                .iter()
                .filter_map(|g| g.as_str())
                .filter_map(|g_str| groups.get(g_str))
                .map(|g_id| Value::String(g_id.clone()))
                .collect();
            e["groupIds"] = Value::Array(new_g_arr);
        }

        for key in ["containerId", "frameId"] {
            if let Some(val_str) = e.get(key).and_then(|v| v.as_str()) {
                if let Some(mapped) = ids.get(val_str) {
                    e[key] = Value::String(mapped.clone());
                }
            }
        }

        if let Some(bound_arr) = e.get_mut("boundElements").and_then(|v| v.as_array_mut()) {
            for b in bound_arr {
                if let Some(b_id) = b.get("id").and_then(|v| v.as_str()) {
                    if let Some(mapped) = ids.get(b_id) {
                        b["id"] = Value::String(mapped.clone());
                    }
                }
            }
        }

        for side in ["startBinding", "endBinding"] {
            if let Some(side_obj) = e.get_mut(side).and_then(|v| v.as_object_mut()) {
                if let Some(ref_id) = side_obj.get("elementId").and_then(|v| v.as_str()) {
                    if let Some(mapped) = ids.get(ref_id) {
                        side_obj.insert("elementId".to_string(), Value::String(mapped.clone()));
                    }
                }
            }
        }
    }

    match serde_json::to_string_pretty(&els) {
        Ok(json_str) => println!("{json_str}"),
        Err(err) => {
            eprintln!("failed to serialize item JSON: {err}");
            process::exit(1);
        }
    }
}

pub fn cmd_describe(doc: &Value) {
    let elements = live(doc);
    if elements.is_empty() {
        println!("The canvas is empty. No elements to describe.");
        return;
    }

    let mut type_counts: HashMap<&str, usize> = HashMap::new();
    for e in &elements {
        let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        *type_counts.entry(t).or_insert(0) += 1;
    }
    let mut sorted_types: Vec<_> = type_counts.into_iter().collect();
    sorted_types.sort_by_key(|&(t, _)| t);
    let types_summary = sorted_types
        .iter()
        .map(|(t, c)| format!("{}({})", t, c))
        .collect::<Vec<_>>()
        .join(", ");

    let (min_x, min_y, w, h) = extent(&elements);
    let max_x = min_x + w;
    let max_y = min_y + h;

    let texts: HashMap<&str, &Value> = elements
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("text"))
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(|id| (id, *e)))
        .collect();

    let mut sorted_elements = elements.clone();
    sorted_elements.sort_by(|a, b| {
        let ya = a.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let yb = b.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let row_a = (ya / 50.0).floor() as i64;
        let row_b = (yb / 50.0).floor() as i64;
        if row_a != row_b {
            row_a.cmp(&row_b)
        } else {
            let xa = a.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let xb = b.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            xa.partial_cmp(&xb).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut elem_descs = Vec::new();
    for e in sorted_elements {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let elem_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;
        let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;
        let width = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;
        let height = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;

        let mut parts = Vec::new();
        parts.push(format!("[{}] {}", id, elem_type));
        parts.push(format!("at ({}, {})", x, y));
        if width > 0 || height > 0 {
            parts.push(format!("size {}x{}", width, height));
        }

        let text = element_text(e);
        if !text.is_empty() {
            parts.push(format!("text: {:?}", text.replace('\n', " / ")));
        } else {
            let lbl = label_of(e, &texts);
            if !lbl.is_empty() {
                parts.push(format!("label: {:?}", lbl));
            }
        }

        if let Some(role) = e.get("customData").and_then(|cd| cd.get("role")).and_then(|v| v.as_str()) {
            parts.push(format!("role: {}", role));
        }
        if let Some(bg) = e.get("backgroundColor").and_then(|v| v.as_str()) {
            if bg != "transparent" && !bg.is_empty() {
                parts.push(format!("bg: {}", bg));
            }
        }
        if let Some(stroke) = e.get("strokeColor").and_then(|v| v.as_str()) {
            if stroke != "#000000" && stroke != "#1e1e1e" && !stroke.is_empty() {
                parts.push(format!("stroke: {}", stroke));
            }
        }
        if e.get("locked").and_then(|v| v.as_bool()).unwrap_or(false) {
            parts.push("(locked)".to_string());
        }
        if let Some(gids) = e.get("groupIds").and_then(|v| v.as_array()) {
            let gid_strs: Vec<_> = gids.iter().filter_map(|g| g.as_str()).collect();
            if !gid_strs.is_empty() {
                parts.push(format!("groups: [{}]", gid_strs.join(", ")));
            }
        }

        elem_descs.push(format!("  {}", parts.join(" | ")));
    }

    let mut connection_descs = Vec::new();
    for e in &elements {
        if e.get("type").and_then(|v| v.as_str()) == Some("arrow") {
            let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let from = e.get("startBinding").and_then(|b| b.get("elementId")).and_then(|v| v.as_str()).unwrap_or("?");
            let to = e.get("endBinding").and_then(|b| b.get("elementId")).and_then(|v| v.as_str()).unwrap_or("?");
            let label = label_of(e, &texts);
            if label.is_empty() {
                connection_descs.push(format!("  {} --> {} (arrow: {})", from, to, id));
            } else {
                connection_descs.push(format!("  {} --> {} (arrow: {}, label: {:?})", from, to, id, label));
            }
        }
    }

    let mut group_map: HashMap<String, Vec<String>> = HashMap::new();
    for e in &elements {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(gids) = e.get("groupIds").and_then(|v| v.as_array()) {
            for gid in gids.iter().filter_map(|g| g.as_str()) {
                group_map.entry(gid.to_string()).or_default().push(id.to_string());
            }
        }
    }
    let mut sorted_groups: Vec<_> = group_map.into_iter().collect();
    sorted_groups.sort_by(|a, b| a.0.cmp(&b.0));

    println!("## Canvas Description");
    println!("Total elements: {}", elements.len());
    println!("Types: {}", types_summary);
    println!("Bounding box: ({:.0}, {:.0}) to ({:.0}, {:.0}) = {:.0}x{:.0}", min_x, min_y, max_x, max_y, w, h);
    println!();
    println!("### Elements (top-to-bottom, left-to-right):");
    for line in elem_descs {
        println!("{}", line);
    }

    if !connection_descs.is_empty() {
        println!();
        println!("### Connections:");
        for line in connection_descs {
            println!("{}", line);
        }
    }

    if !sorted_groups.is_empty() {
        println!();
        println!("### Groups:");
        for (gid, ids) in sorted_groups {
            println!("  Group {}: [{}]", gid, ids.join(", "));
        }
    }
}

pub fn lookup_json_path<'a>(val: &'a Value, path: &str) -> Option<&'a Value> {
    let mut curr = val;
    for part in path.split('.') {
        curr = curr.get(part)?;
    }
    Some(curr)
}

pub fn coerce_filter_match(actual: &Value, expected: &str) -> bool {
    match actual {
        Value::Bool(b) => {
            if expected == "true" {
                *b
            } else if expected == "false" {
                !*b
            } else {
                false
            }
        }
        Value::Number(n) => {
            if let Some(num) = n.as_f64() {
                if let Ok(exp_num) = expected.parse::<f64>() {
                    (num - exp_num).abs() < 0.0001
                } else {
                    false
                }
            } else {
                false
            }
        }
        Value::String(s) => s == expected,
        Value::Array(arr) => {
            arr.iter().any(|item| coerce_filter_match(item, expected))
        }
        Value::Null => expected == "null",
        _ => false,
    }
}

pub fn cmd_query(
    doc: &Value,
    elem_type: Option<&str>,
    bbox: Option<(f64, f64, f64, f64)>,
    filters: &[(&str, &str)],
    filter_json: Option<&Value>,
) {
    let elements = live(doc);
    let mut results = Vec::new();

    for e in elements {
        if let Some(t) = elem_type {
            if e.get("type").and_then(|v| v.as_str()) != Some(t) {
                continue;
            }
        }

        if let Some((x0, y0, x1, y1)) = bbox {
            let ex = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let ey = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let ew = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let eh = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

            if !(ex < x1 && ex + ew > x0 && ey < y1 && ey + eh > y0) {
                continue;
            }
        }

        let mut matches_all_filters = true;
        for &(k, v) in filters {
            if let Some(actual) = lookup_json_path(e, k) {
                if !coerce_filter_match(actual, v) {
                    matches_all_filters = false;
                    break;
                }
            } else {
                matches_all_filters = false;
                break;
            }
        }
        if !matches_all_filters {
            continue;
        }

        if let Some(fj) = filter_json {
            if let Some(obj) = fj.as_object() {
                let mut matches_json = true;
                for (k, expected_v) in obj {
                    if let Some(actual) = lookup_json_path(e, k) {
                        if actual != expected_v {
                            matches_json = false;
                            break;
                        }
                    } else {
                        matches_json = false;
                        break;
                    }
                }
                if !matches_json {
                    continue;
                }
            }
        }

        results.push(e.clone());
    }

    let json_str = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
    println!("{json_str}");
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn render_svg(doc: &Value, include_background: bool) -> Result<String, String> {
    let elements = live(doc);
    let (min_x, min_y, w, h) = if elements.is_empty() {
        (0.0, 0.0, 800.0, 600.0)
    } else {
        extent(&elements)
    };

    let pad = 40.0;
    let view_x = min_x - pad;
    let view_y = min_y - pad;
    let view_w = (w + 2.0 * pad).max(100.0);
    let view_h = (h + 2.0 * pad).max(100.0);

    let bg_color = doc
        .get("appState")
        .and_then(|a| a.get("viewBackgroundColor"))
        .and_then(|v| v.as_str())
        .unwrap_or("#ffffff");

    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.1} {:.1} {:.1} {:.1}" width="{:.1}" height="{:.1}">"#,
        view_x, view_y, view_w, view_h, view_w, view_h
    ));
    svg.push('\n');

    svg.push_str(r##"  <defs>
    <marker id="arrowhead" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
      <path d="M 0 1 L 10 5 L 0 9 z" fill="#404040"/>
    </marker>
  </defs>
  <style>
    text { font-family: Virgil, Segoe UI Emoji, sans-serif; }
  </style>
"##);

    if include_background {
        svg.push_str(&format!(
            r#"  <rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{}" />"#,
            view_x, view_y, view_w, view_h, bg_color
        ));
        svg.push('\n');
    }

    for e in &elements {
        let elem_type = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let width = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let height = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let stroke_color = e.get("strokeColor").and_then(|v| v.as_str()).unwrap_or("#1e1e1e");
        let stroke_width = e.get("strokeWidth").and_then(|v| v.as_f64()).unwrap_or(2.0);
        let stroke_style = e.get("strokeStyle").and_then(|v| v.as_str()).unwrap_or("solid");
        let dash_attr = match stroke_style {
            "dashed" => " stroke-dasharray=\"6,6\"",
            "dotted" => " stroke-dasharray=\"2,4\"",
            _ => "",
        };
        let opacity = e.get("opacity").and_then(|v| v.as_f64()).unwrap_or(100.0);
        let opacity_attr = if opacity < 100.0 {
            format!(" opacity=\"{:.2}\"", opacity / 100.0)
        } else {
            "".to_string()
        };
        let bg = e.get("backgroundColor").and_then(|v| v.as_str()).unwrap_or("transparent");
        let fill = if bg == "transparent" || bg.is_empty() { "none" } else { bg };

        match elem_type {
            "rectangle" => {
                let rx_attr = if e.get("roundness").is_some() && !e.get("roundness").unwrap().is_null() {
                    " rx=\"8\" ry=\"8\""
                } else {
                    ""
                };
                svg.push_str(&format!(
                    r#"  <rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{}" stroke="{}" stroke-width="{:.1}"{}{}{} />"#,
                    x, y, width, height, fill, stroke_color, stroke_width, rx_attr, dash_attr, opacity_attr
                ));
                svg.push('\n');
            }
            "ellipse" => {
                let cx = x + width / 2.0;
                let cy = y + height / 2.0;
                let rx = width / 2.0;
                let ry = height / 2.0;
                svg.push_str(&format!(
                    r#"  <ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}" fill="{}" stroke="{}" stroke-width="{:.1}"{}{} />"#,
                    cx, cy, rx, ry, fill, stroke_color, stroke_width, dash_attr, opacity_attr
                ));
                svg.push('\n');
            }
            "diamond" => {
                let cx = x + width / 2.0;
                let cy = y + height / 2.0;
                svg.push_str(&format!(
                    r#"  <polygon points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1}" fill="{}" stroke="{}" stroke-width="{:.1}"{}{} />"#,
                    cx, y, x + width, cy, cx, y + height, x, cy, fill, stroke_color, stroke_width, dash_attr, opacity_attr
                ));
                svg.push('\n');
            }
            "line" | "arrow" => {
                let is_arrow = elem_type == "arrow";
                let marker_attr = if is_arrow { " marker-end=\"url(#arrowhead)\"" } else { "" };
                if let Some(pts) = e.get("points").and_then(|v| v.as_array()) {
                    let mut pts_tuples: Vec<(f64, f64)> = Vec::new();
                    for p in pts {
                        if let Some(arr) = p.as_array() {
                            if arr.len() == 2 {
                                pts_tuples.push((x + arr[0].as_f64().unwrap_or(0.0), y + arr[1].as_f64().unwrap_or(0.0)));
                            }
                        } else if let Some(obj) = p.as_object() {
                            pts_tuples.push((x + obj.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0), y + obj.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0)));
                        }
                    }

                    if pts_tuples.len() == 2 {
                        svg.push_str(&format!(
                            r#"  <line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="{:.1}"{}{}{} />"#,
                            pts_tuples[0].0, pts_tuples[0].1, pts_tuples[1].0, pts_tuples[1].1,
                            stroke_color, stroke_width, dash_attr, opacity_attr, marker_attr
                        ));
                        svg.push('\n');
                    } else if pts_tuples.len() > 2 {
                        let pts_str = pts_tuples.iter().map(|(px, py)| format!("{:.1},{:.1}", px, py)).collect::<Vec<_>>().join(" ");
                        svg.push_str(&format!(
                            r#"  <polyline points="{}" fill="none" stroke="{}" stroke-width="{:.1}"{}{}{} />"#,
                            pts_str, stroke_color, stroke_width, dash_attr, opacity_attr, marker_attr
                        ));
                        svg.push('\n');
                    }
                }
            }
            "text" => {
                let font_size = e.get("fontSize").and_then(|v| v.as_f64()).unwrap_or(20.0);
                let text_val = element_text(e);
                let text_align = e.get("textAlign").and_then(|v| v.as_str()).unwrap_or("left");
                let text_anchor = match text_align {
                    "center" => "middle",
                    "right" => "end",
                    _ => "start",
                };
                let text_x = match text_align {
                    "center" => x + width / 2.0,
                    "right" => x + width,
                    _ => x,
                };

                let font_family_code = e.get("fontFamily").and_then(|v| v.as_i64()).unwrap_or(1);
                let font_family_str = match font_family_code {
                    2 => "Helvetica Neue, Arial, sans-serif",
                    3 => "Cascadia Code, monospace",
                    _ => "Virgil, Segoe UI Emoji, cursive, sans-serif",
                };

                let lines: Vec<&str> = text_val.split('\n').collect();
                let line_height = font_size * 1.25;

                svg.push_str(&format!(
                    r#"  <text x="{:.1}" y="{:.1}" font-size="{:.1}" font-family="{}" fill="{}" text-anchor="{}"{}>"#,
                    text_x, y + font_size * 0.8, font_size, font_family_str, stroke_color, text_anchor, opacity_attr
                ));
                svg.push('\n');
                for (i, line) in lines.iter().enumerate() {
                    let dy = if i == 0 { 0.0 } else { line_height };
                    svg.push_str(&format!(
                        r#"    <tspan x="{:.1}" dy="{:.1}">{}</tspan>"#,
                        text_x, dy, xml_escape(line)
                    ));
                    svg.push('\n');
                }
                svg.push_str("  </text>\n");
            }
            _ => {}
        }
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

pub fn cmd_screenshot(doc: &Value, out_path: Option<&str>, format: Option<&str>, no_background: bool) {
    let fmt = format.unwrap_or_else(|| {
        if let Some(p) = out_path {
            if p.ends_with(".png") {
                "png"
            } else {
                "svg"
            }
        } else {
            "svg"
        }
    });

    let svg_content = match render_svg(doc, !no_background) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to render SVG: {e}");
            process::exit(1);
        }
    };

    if fmt == "svg" {
        if let Some(p) = out_path {
            if let Err(e) = fs::write(p, &svg_content) {
                eprintln!("failed to write SVG to {p:?}: {e}");
                process::exit(1);
            }
            let res = json!({
                "success": true,
                "file": p,
                "format": "svg"
            });
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            print!("{svg_content}");
        }
        return;
    }

    if fmt == "png" {
        let target_png = match out_path {
            Some(p) => p.to_string(),
            None => {
                eprintln!("--format png requires --out <path.png>");
                process::exit(1);
            }
        };

        let temp_svg = format!("{}.tmp.svg", target_png);
        if let Err(e) = fs::write(&temp_svg, &svg_content) {
            eprintln!("failed to write temporary SVG: {e}");
            process::exit(1);
        }

        let mut rasterized = false;
        let tools = [
            ("resvg", vec![temp_svg.as_str(), target_png.as_str()]),
            ("rsvg-convert", vec!["-f", "png", "-o", target_png.as_str(), temp_svg.as_str()]),
            ("magick", vec![temp_svg.as_str(), target_png.as_str()]),
            ("convert", vec![temp_svg.as_str(), target_png.as_str()]),
        ];

        for (cmd, args) in &tools {
            if let Ok(status) = process::Command::new(cmd).args(args).status() {
                if status.success() {
                    rasterized = true;
                    break;
                }
            }
        }

        let _ = fs::remove_file(&temp_svg);

        if rasterized {
            let res = json!({
                "success": true,
                "file": target_png,
                "format": "png"
            });
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            let fallback_svg = format!("{}.svg", target_png.trim_end_matches(".png"));
            let _ = fs::write(&fallback_svg, &svg_content);
            eprintln!(
                "Notice: PNG rasterizer not found on PATH (resvg, rsvg-convert, or ImageMagick required). Saved vector SVG to {:?} instead.",
                fallback_svg
            );
            let res = json!({
                "success": true,
                "file": fallback_svg,
                "format": "svg",
                "note": "Rasterizer unavailable; saved SVG fallback"
            });
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        }
    }
}

pub fn cmd_apply(doc: &mut Value, patch_val: &Value) -> Result<(usize, usize, usize), String> {
    let mut created_count = 0;
    let mut updated_count = 0;
    let mut deleted_count = 0;

    let (creates, updates, deletes) = if let Some(arr) = patch_val.as_array() {
        (Some(arr.as_slice()), None, None)
    } else if let Some(obj) = patch_val.as_object() {
        let c = obj.get("create").and_then(|v| v.as_array()).map(|a| a.as_slice());
        let u = obj.get("update").and_then(|v| v.as_array()).map(|a| a.as_slice());
        let d = obj.get("delete").and_then(|v| v.as_array()).map(|a| a.as_slice());
        (c, u, d)
    } else {
        return Err("apply patch must be a JSON array or object with create/update/delete".to_string());
    };

    if let Some(create_items) = creates {
        for item in create_items {
            let elem_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("rectangle");
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let at = item.get("at").and_then(|v| v.as_array()).and_then(|a| {
                if a.len() == 2 {
                    Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                } else {
                    None
                }
            });
            let custom_id = item.get("id").and_then(|v| v.as_str());

            if elem_type == "arrow" {
                let from = item.get("startElementId").or_else(|| item.get("from")).and_then(|v| v.as_str()).unwrap_or("");
                let to = item.get("endElementId").or_else(|| item.get("to")).and_then(|v| v.as_str()).unwrap_or("");
                let label = item.get("label").or_else(|| item.get("text")).and_then(|v| v.as_str());
                let color = item.get("color").or_else(|| item.get("strokeColor")).and_then(|v| v.as_str());
                let curved = item.get("curved").and_then(|v| v.as_bool());
                let stroke_style = item.get("strokeStyle").or_else(|| item.get("stroke_style")).and_then(|v| v.as_str());
                mutate_connect(doc, from, to, label, color, curved, stroke_style)?;
            } else if elem_type == "text" {
                let at_pos = at.unwrap_or((100.0, 100.0));
                let font_size = item.get("fontSize").and_then(|v| v.as_f64());
                let color = item.get("color").or_else(|| item.get("strokeColor")).and_then(|v| v.as_str());
                mutate_add_text(doc, text, at_pos, font_size, color)?;
            } else {
                let size = item.get("size").and_then(|v| v.as_array()).and_then(|a| {
                    if a.len() == 2 {
                        Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                    } else {
                        None
                    }
                });
                let role = item.get("role").and_then(|v| v.as_str());
                let color = item.get("color").or_else(|| item.get("backgroundColor")).and_then(|v| v.as_str());
                let angle = item.get("angle").and_then(|v| v.as_f64());
                let roughness = item.get("roughness").and_then(|v| v.as_f64());
                let fill_style = item.get("fillStyle").or_else(|| item.get("fill_style")).and_then(|v| v.as_str());
                let preset = item.get("preset").and_then(|v| v.as_str());
                mutate_add_node(doc, elem_type, text, at, size, role, color, None, custom_id, angle, roughness, fill_style, preset)?;
            }
            created_count += 1;
        }
    }

    if let Some(update_items) = updates {
        for item in update_items {
            let id = item.get("id").and_then(|v| v.as_str()).ok_or_else(|| "update item missing 'id'".to_string())?;
            let updates_map = if let Some(set_obj) = item.get("set") {
                set_obj
            } else {
                item
            };
            mutate_update_elem(doc, id, updates_map)?;
            updated_count += 1;
        }
    }

    if let Some(delete_items) = deletes {
        for item in delete_items {
            if let Some(id) = item.as_str() {
                mutate_delete_elem(doc, id, true)?;
                deleted_count += 1;
            }
        }
    }

    Ok((created_count, updated_count, deleted_count))
}

pub fn cmd_snapshot_save(file_path: &str, doc: &Value, name: &str) -> Result<(), String> {
    let path = std::path::Path::new(file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("canvas");
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let snap_dir = parent.join(format!(".snapshots_{}", stem));
    fs::create_dir_all(&snap_dir).map_err(|e| format!("failed to create snapshot dir: {e}"))?;

    let snap_file = snap_dir.join(format!("{}.json", name));
    let elements = live(doc);
    let snap_obj = json!({
        "name": name,
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "elementCount": elements.len(),
        "elements": elements
    });

    let content = serde_json::to_string_pretty(&snap_obj).map_err(|e| e.to_string())?;
    fs::write(&snap_file, content).map_err(|e| format!("failed to write snapshot file: {e}"))?;
    println!("OK: saved snapshot '{name}' ({} elements)", elements.len());
    Ok(())
}

pub fn cmd_snapshot_list(file_path: &str) -> Result<(), String> {
    let path = std::path::Path::new(file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("canvas");
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let snap_dir = parent.join(format!(".snapshots_{}", stem));

    if !snap_dir.exists() {
        println!("[]");
        return Ok(());
    }

    let mut list = Vec::new();
    if let Ok(entries) = fs::read_dir(&snap_dir) {
        for entry in entries.flatten() {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                if let Ok(val) = serde_json::from_str::<Value>(&content) {
                    list.push(json!({
                        "name": val.get("name").unwrap_or(&Value::Null),
                        "createdAt": val.get("createdAt").unwrap_or(&Value::Null),
                        "elementCount": val.get("elementCount").unwrap_or(&Value::Null),
                    }));
                }
            }
        }
    }

    let json_str = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    println!("{json_str}");
    Ok(())
}

pub fn cmd_snapshot_restore(file_path: &str, doc: &mut Value, name: &str) -> Result<(), String> {
    let path = std::path::Path::new(file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("canvas");
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let snap_file = parent.join(format!(".snapshots_{}", stem)).join(format!("{}.json", name));

    if !snap_file.exists() {
        return Err(format!("Snapshot '{name}' not found at {:?}", snap_file));
    }

    let content = fs::read_to_string(&snap_file).map_err(|e| format!("failed to read snapshot: {e}"))?;
    let snap_obj: Value = serde_json::from_str(&content).map_err(|e| format!("invalid snapshot JSON: {e}"))?;

    let snap_elements = snap_obj
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "snapshot missing elements array".to_string())?;

    doc["elements"] = json!(snap_elements);
    let is_obsidian = is_obsidian_excalidraw_md(&fs::read_to_string(file_path).unwrap_or_default());
    atomic_save_ex(file_path, doc, is_obsidian)?;
    println!("OK: restored snapshot '{name}' ({} elements)", snap_elements.len());
    Ok(())
}

// ============================================================================
// Main CLI Dispatch
// ============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("{USAGE}");
        process::exit(1);
    }

    if args.len() == 2 && (args[1] == "--help" || args[1] == "-h") {
        print!("{USAGE}");
        process::exit(0);
    }

    // Normalize command positioning:
    // Support both `pkb-excalidraw FILE COMMAND [args]` and `pkb-excalidraw COMMAND FILE [args]`
    let known_modes = [
        "summary", "map", "style", "check", "diff", "struct-diff", "lib", "item",
        "nodes", "edges", "arrows", "inspect", "get", "add-node", "add-text",
        "connect", "set-text", "fit", "move-elem", "delete-elem", "batch",
        "theme", "overlap", "arrows-check",
        "describe", "screenshot", "arrange", "align", "distribute", "group",
        "ungroup", "lock", "unlock", "duplicate", "update", "query", "apply",
        "export", "import", "snapshot", "clear"
    ];

    let (file_path, mode, extra_args) = if known_modes.contains(&args[1].as_str()) && args.len() > 2 {
        let m = args[1].as_str();
        let f = &args[2];
        let extra = &args[3..];
        (f, m, extra)
    } else {
        let f = &args[1];
        let m = if args.len() > 2 { args[2].as_str() } else { "summary" };
        let extra = if args.len() > 3 { &args[3..] } else { &[] as &[String] };
        (f, m, extra)
    };

    let file_content = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read {file_path:?}: {e}");
            process::exit(1);
        }
    };

    let (json_str, is_obsidian) = if is_obsidian_excalidraw_md(&file_content) {
        match extract_scene_json_from_obsidian_md(&file_content) {
            Ok(j) => (j, true),
            Err(e) => {
                eprintln!("failed to extract JSON from Obsidian Excalidraw file {file_path:?}: {e}");
                process::exit(1);
            }
        }
    } else {
        (file_content.clone(), false)
    };

    let mut doc: Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to parse JSON from {file_path:?}: {e}");
            process::exit(1);
        }
    };

    match mode {
        "summary" => cmd_summary(&doc),
        "map" => cmd_map(&doc),
        "nodes" => cmd_nodes(&doc),
        "edges" | "arrows" => cmd_edges(&doc),
        "inspect" => {
            if extra_args.is_empty() {
                eprintln!("inspect mode requires an element id: pkb-excalidraw FILE inspect <id>");
                process::exit(1);
            }
            cmd_inspect(&doc, &extra_args[0]);
        }
        "get" => {
            if extra_args.is_empty() {
                eprintln!("get mode requires an element id: pkb-excalidraw FILE get <id>");
                process::exit(1);
            }
            cmd_get(&doc, &extra_args[0]);
        }
        "style" => cmd_style(&doc),
        "check" => match cmd_check(&doc) {
            Ok(msg) => {
                println!("{msg}");
                process::exit(0);
            }
            Err(fails) => {
                println!("FAIL");
                for f in fails {
                    println!("  {f}");
                }
                process::exit(1);
            }
        },
        "overlap" => match cmd_overlap(&doc) {
            Ok(_) => {
                println!("OK: No overlapping elements detected.");
                process::exit(0);
            }
            Err(collisions) => {
                println!("OVERLAP DETECTED ({} collisions):", collisions.len());
                for c in collisions {
                    println!("  {c}");
                }
                process::exit(1);
            }
        },
        "arrows-check" => match cmd_arrows_check(&doc) {
            Ok(_) => {
                println!("OK: No arrow collisions detected.");
                process::exit(0);
            }
            Err(collisions) => {
                println!("ARROW INTERSECTION DETECTED ({} collisions):", collisions.len());
                for c in collisions {
                    println!("  {c}");
                }
                process::exit(1);
            }
        },
        "diff" => {
            if extra_args.is_empty() {
                eprintln!("diff mode requires a second file: pkb-excalidraw FILE1 diff FILE2");
                process::exit(1);
            }
            let file2_path = &extra_args[0];
            let file2_content = match fs::read_to_string(file2_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("failed to read second file {file2_path:?}: {e}");
                    process::exit(1);
                }
            };
            let doc2: Value = match serde_json::from_str(&file2_content) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to parse JSON from {file2_path:?}: {e}");
                    process::exit(1);
                }
            };
            cmd_diff(&doc, &doc2);
        }
        "struct-diff" => {
            if extra_args.is_empty() {
                eprintln!("struct-diff mode requires a second file: pkb-excalidraw FILE1 struct-diff FILE2");
                process::exit(1);
            }
            let file2_path = &extra_args[0];
            let file2_content = match fs::read_to_string(file2_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("failed to read second file {file2_path:?}: {e}");
                    process::exit(1);
                }
            };
            let doc2: Value = match serde_json::from_str(&file2_content) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to parse JSON from {file2_path:?}: {e}");
                    process::exit(1);
                }
            };
            let diffs = cmd_struct_diff(&doc, &doc2);
            for d in diffs {
                println!("{d}");
            }
        }
        "lib" => {
            cmd_lib(&doc);
        }
        "item" => {
            if extra_args.is_empty() {
                eprintln!("item mode needs a SELECTOR (a name substring or #N from `lib`)");
                process::exit(1);
            }
            let selector = &extra_args[0];
            let mut opts = &extra_args[1..];
            let mut after: Option<String> = None;
            let mut at: Option<(f64, f64)> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--after" && !opts.is_empty() {
                    after = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--at" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let x = parts[0].parse::<f64>().unwrap_or(0.0);
                        let y = parts[1].parse::<f64>().unwrap_or(0.0);
                        at = Some((x, y));
                    } else {
                        eprintln!("invalid coordinates for --at: expected X,Y");
                        process::exit(1);
                    }
                } else {
                    eprintln!("unrecognised argument: {flag}");
                    process::exit(1);
                }
            }

            let after = match after {
                Some(a) => a,
                None => {
                    eprintln!("item mode needs --after INDEX — the target file's max index, from `summary`");
                    process::exit(1);
                }
            };

            cmd_item(&doc, selector, &after, at);
        }
        
        "update-node" => {
            let mut opts = extra_args;
            let mut target_id = "".to_string();
            let mut angle: Option<f64> = None;
            let mut roughness: Option<f64> = None;
            let mut fill_style: Option<String> = None;
            let mut preset: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--id" && !opts.is_empty() {
                    target_id = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--angle" && !opts.is_empty() {
                    angle = opts[0].parse::<f64>().ok();
                    opts = &opts[1..];
                } else if flag == "--roughness" && !opts.is_empty() {
                    roughness = opts[0].parse::<f64>().ok();
                    opts = &opts[1..];
                } else if flag == "--fill-style" && !opts.is_empty() {
                    fill_style = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--preset" && !opts.is_empty() {
                    preset = Some(opts[0].clone());
                    opts = &opts[1..];
                } else {
                    eprintln!("unrecognised argument: {flag}");
                    std::process::exit(1);
                }
            }

            if target_id.is_empty() {
                eprintln!("--id is required for update-node");
                std::process::exit(1);
            }

            if let Err(e) = mutate_update_node(
                &mut doc,
                &target_id,
                angle,
                roughness,
                fill_style.as_deref(),
                preset.as_deref(),
            ) {
                eprintln!("error updating node: {e}");
                std::process::exit(1);
            }
            if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                eprintln!("error saving: {e}");
                std::process::exit(1);
            }
            println!("OK: updated node {}", target_id);
        }

        "add-node" => {
            let mut opts = extra_args;
            let mut elem_type = "rectangle".to_string();
            let mut text = "".to_string();
            let mut at: Option<(f64, f64)> = None;
            let mut size: Option<(f64, f64)> = None;
            let mut role: Option<String> = None;
            let mut color: Option<String> = None;
            let mut custom_id: Option<String> = None;
            let mut angle: Option<f64> = None;
            let mut roughness: Option<f64> = None;
            let mut fill_style: Option<String> = None;
            let mut preset: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--type" && !opts.is_empty() {
                    elem_type = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--text" && !opts.is_empty() {
                    text = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--at" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let x = parts[0].parse::<f64>().unwrap_or(0.0);
                        let y = parts[1].parse::<f64>().unwrap_or(0.0);
                        at = Some((x, y));
                    }
                } else if flag == "--size" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let w = parts[0].parse::<f64>().unwrap_or(100.0);
                        let h = parts[1].parse::<f64>().unwrap_or(50.0);
                        size = Some((w, h));
                    }
                } else if flag == "--role" && !opts.is_empty() {
                    role = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--color" && !opts.is_empty() {
                    color = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--id" && !opts.is_empty() {
                    custom_id = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--angle" && !opts.is_empty() {
                    angle = opts[0].parse().ok();
                    opts = &opts[1..];
                } else if flag == "--roughness" && !opts.is_empty() {
                    roughness = opts[0].parse().ok();
                    opts = &opts[1..];
                } else if flag == "--fill-style" && !opts.is_empty() {
                    fill_style = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--preset" && !opts.is_empty() {
                    preset = Some(opts[0].clone());
                    opts = &opts[1..];
                }
            }

            match mutate_add_node(
                &mut doc,
                &elem_type,
                &text,
                at,
                size,
                role.as_deref(),
                color.as_deref(),
                None,
                custom_id.as_deref(),
                angle,
                roughness,
                fill_style.as_deref(),
                preset.as_deref(),
            ) {
                Ok((sid, tid)) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: created node {sid} (text {tid})");
                }
                Err(e) => {
                    eprintln!("error creating node: {e}");
                    process::exit(1);
                }
            }
        }
        "add-text" => {
            let mut opts = extra_args;
            let mut text = "".to_string();
            let mut at: Option<(f64, f64)> = None;
            let mut font_size: Option<f64> = None;
            let mut color: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--text" && !opts.is_empty() {
                    text = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--at" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let x = parts[0].parse::<f64>().unwrap_or(0.0);
                        let y = parts[1].parse::<f64>().unwrap_or(0.0);
                        at = Some((x, y));
                    }
                } else if flag == "--font-size" && !opts.is_empty() {
                    font_size = opts[0].parse::<f64>().ok();
                    opts = &opts[1..];
                } else if flag == "--color" && !opts.is_empty() {
                    color = Some(opts[0].clone());
                    opts = &opts[1..];
                }
            }

            let at_coords = at.unwrap_or((100.0, 100.0));
            match mutate_add_text(&mut doc, &text, at_coords, font_size, color.as_deref()) {
                Ok(tid) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: created text {tid}");
                }
                Err(e) => {
                    eprintln!("error creating text: {e}");
                    process::exit(1);
                }
            }
        }
        "connect" => {
            let mut opts = extra_args;
            let mut from_id = "".to_string();
            let mut to_id = "".to_string();
            let mut label: Option<String> = None;
            let mut color: Option<String> = None;
            let mut curved: Option<bool> = None;
            let mut stroke_style: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--from" && !opts.is_empty() {
                    from_id = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--to" && !opts.is_empty() {
                    to_id = opts[0].clone();
                    opts = &opts[1..];
                } else if flag == "--label" && !opts.is_empty() {
                    label = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--color" && !opts.is_empty() {
                    color = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--curved" {
                    curved = Some(true);
                } else if flag == "--stroke-style" && !opts.is_empty() {
                    stroke_style = Some(opts[0].clone());
                    opts = &opts[1..];
                }
            }

            if from_id.is_empty() || to_id.is_empty() {
                eprintln!("connect requires --from <id1> and --to <id2>");
                process::exit(1);
            }

            match mutate_connect(&mut doc, &from_id, &to_id, label.as_deref(), color.as_deref(), curved, stroke_style.as_deref()) {
                Ok(aid) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: connected {from_id} -> {to_id} with arrow {aid}");
                }
                Err(e) => {
                    eprintln!("error connecting: {e}");
                    process::exit(1);
                }
            }
        }
        "set-text" | "fit" => {
            if extra_args.len() < 2 {
                eprintln!("set-text requires <id> and \"<new_text>\"");
                process::exit(1);
            }
            let target_id = &extra_args[0];
            let new_text = &extra_args[1];

            match mutate_set_text(&mut doc, target_id, new_text) {
                Ok(_) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: updated text for {target_id}");
                }
                Err(e) => {
                    eprintln!("error setting text: {e}");
                    process::exit(1);
                }
            }
        }
        "move-elem" => {
            if extra_args.is_empty() {
                eprintln!("move-elem requires <id> [--to X,Y | --by DX,DY]");
                process::exit(1);
            }
            let target_id = &extra_args[0];
            let mut opts = &extra_args[1..];
            let mut to: Option<(f64, f64)> = None;
            let mut by: Option<(f64, f64)> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--to" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let x = parts[0].parse::<f64>().unwrap_or(0.0);
                        let y = parts[1].parse::<f64>().unwrap_or(0.0);
                        to = Some((x, y));
                    }
                } else if flag == "--by" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    opts = &opts[1..];
                    if parts.len() == 2 {
                        let dx = parts[0].parse::<f64>().unwrap_or(0.0);
                        let dy = parts[1].parse::<f64>().unwrap_or(0.0);
                        by = Some((dx, dy));
                    }
                }
            }

            match mutate_move_elem(&mut doc, target_id, to, by) {
                Ok(_) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: moved element {target_id}");
                }
                Err(e) => {
                    eprintln!("error moving element: {e}");
                    process::exit(1);
                }
            }
        }
        "delete-elem" => {
            if extra_args.is_empty() {
                eprintln!("delete-elem requires <id> [--cascade-arrows]");
                process::exit(1);
            }
            let target_id = &extra_args[0];
            let cascade_arrows = extra_args.iter().any(|arg| arg == "--cascade-arrows");

            match mutate_delete_elem(&mut doc, target_id, cascade_arrows) {
                Ok(_) => {
                    if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    }
                    println!("OK: deleted element {target_id}");
                }
                Err(e) => {
                    eprintln!("error deleting element: {e}");
                    process::exit(1);
                }
            }
        }
        "batch" => {
            if extra_args.is_empty() {
                eprintln!("batch requires a JSON file or '-' for stdin: pkb-excalidraw FILE batch <changes.json | ->");
                process::exit(1);
            }
            let batch_input = if extra_args[0] == "-" {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).unwrap_or_default();
                buf
            } else {
                fs::read_to_string(&extra_args[0]).unwrap_or_default()
            };

            let changes: Value = match serde_json::from_str(&batch_input) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to parse batch JSON: {e}");
                    process::exit(1);
                }
            };

            let changes_arr = match changes.as_array() {
                Some(arr) => arr,
                None => {
                    eprintln!("batch JSON must be an array of mutation objects");
                    process::exit(1);
                }
            };

            for (idx, mut_obj) in changes_arr.iter().enumerate() {
                let action = mut_obj.get("action").and_then(|v| v.as_str()).unwrap_or("");
                match action {
                    
                    "update-node" | "update_node" => {
                        let target_id = mut_obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let angle = mut_obj.get("angle").and_then(|v| v.as_f64());
                        let roughness = mut_obj.get("roughness").and_then(|v| v.as_f64());
                        let fill_style = mut_obj.get("fill_style").or_else(|| mut_obj.get("fillStyle")).and_then(|v| v.as_str());
                        let preset = mut_obj.get("preset").and_then(|v| v.as_str());

                        if let Err(e) = mutate_update_node(&mut doc, target_id, angle, roughness, fill_style, preset) {
                            eprintln!("batch mutation #{idx} (update-node) failed: {e}");
                            std::process::exit(1);
                        }
                    }

                    "add-node" | "add_node" => {
                        let elem_type = mut_obj.get("type").and_then(|v| v.as_str()).unwrap_or("rectangle");
                        let text = mut_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let at = mut_obj.get("at").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                            } else {
                                None
                            }
                        });
                        let size = mut_obj.get("size").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                            } else {
                                None
                            }
                        });
                        let role = mut_obj.get("role").and_then(|v| v.as_str());
                        let color = mut_obj.get("color").and_then(|v| v.as_str());
                        let custom_id = mut_obj.get("id").and_then(|v| v.as_str());
                        let angle = mut_obj.get("angle").and_then(|v| v.as_f64());
                        let roughness = mut_obj.get("roughness").and_then(|v| v.as_f64());
                        let fill_style = mut_obj.get("fill_style").or_else(|| mut_obj.get("fillStyle")).and_then(|v| v.as_str());
                        let preset = mut_obj.get("preset").and_then(|v| v.as_str());

                        if let Err(e) = mutate_add_node(&mut doc, elem_type, text, at, size, role, color, None, custom_id, angle, roughness, fill_style, preset) {
                            eprintln!("batch mutation #{idx} (add-node) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "add-text" | "add_text" => {
                        let text = mut_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let at = mut_obj.get("at").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                            } else {
                                None
                            }
                        }).unwrap_or((100.0, 100.0));
                        let font_size = mut_obj.get("fontSize").or_else(|| mut_obj.get("font_size")).and_then(|v| v.as_f64());
                        let color = mut_obj.get("color").and_then(|v| v.as_str());

                        if let Err(e) = mutate_add_text(&mut doc, text, at, font_size, color) {
                            eprintln!("batch mutation #{idx} (add-text) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "connect" => {
                        let from_id = mut_obj.get("from").and_then(|v| v.as_str()).unwrap_or("");
                        let to_id = mut_obj.get("to").and_then(|v| v.as_str()).unwrap_or("");
                        let label = mut_obj.get("label").and_then(|v| v.as_str());
                        let color = mut_obj.get("color").and_then(|v| v.as_str());
                        let curved = mut_obj.get("curved").and_then(|v| v.as_bool());
                        let stroke_style = mut_obj.get("stroke_style").or_else(|| mut_obj.get("strokeStyle")).and_then(|v| v.as_str());

                        if let Err(e) = mutate_connect(&mut doc, from_id, to_id, label, color, curved, stroke_style) {
                            eprintln!("batch mutation #{idx} (connect) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "set-text" | "set_text" | "fit" => {
                        let target_id = mut_obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let new_text = mut_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");

                        if let Err(e) = mutate_set_text(&mut doc, target_id, new_text) {
                            eprintln!("batch mutation #{idx} (set-text) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "move" | "move-elem" | "move_elem" => {
                        let target_id = mut_obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let to = mut_obj.get("to").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                            } else {
                                None
                            }
                        });
                        let by = mut_obj.get("by").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
                            } else {
                                None
                            }
                        });

                        if let Err(e) = mutate_move_elem(&mut doc, target_id, to, by) {
                            eprintln!("batch mutation #{idx} (move) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "delete" | "delete-elem" | "delete_elem" => {
                        let target_id = mut_obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let cascade_arrows = mut_obj.get("cascade_arrows").or_else(|| mut_obj.get("cascadeArrows")).and_then(|v| v.as_bool()).unwrap_or(false);

                        if let Err(e) = mutate_delete_elem(&mut doc, target_id, cascade_arrows) {
                            eprintln!("batch mutation #{idx} (delete) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "theme-apply" | "apply-theme" | "apply_theme" => {
                        let default_theme = Theme::default_retro();
                        if let Err(e) = mutate_theme_apply(&mut doc, &default_theme, None, true) {
                            eprintln!("batch mutation #{idx} (apply-theme) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "align" | "arrange-align" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        let to = mut_obj.get("to").and_then(|v| v.as_str()).unwrap_or("left");
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_align(&mut doc, &ids_vec, to) {
                                eprintln!("batch mutation #{idx} (align) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "distribute" | "arrange-distribute" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        let to = mut_obj.get("to").and_then(|v| v.as_str()).unwrap_or("horizontal");
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_distribute(&mut doc, &ids_vec, to) {
                                eprintln!("batch mutation #{idx} (distribute) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "group" | "arrange-group" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_group(&mut doc, &ids_vec) {
                                eprintln!("batch mutation #{idx} (group) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "ungroup" | "arrange-ungroup" => {
                        let group_id = mut_obj.get("group").and_then(|v| v.as_str());
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        let ids_vec: Option<Vec<String>> = ids_arr.map(|a| a.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect());
                        if let Err(e) = mutate_ungroup(&mut doc, group_id, ids_vec.as_deref()) {
                            eprintln!("batch mutation #{idx} (ungroup) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "lock" | "arrange-lock" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_lock(&mut doc, &ids_vec, true) {
                                eprintln!("batch mutation #{idx} (lock) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "unlock" | "arrange-unlock" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_lock(&mut doc, &ids_vec, false) {
                                eprintln!("batch mutation #{idx} (unlock) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "duplicate" | "arrange-duplicate" => {
                        let ids_arr = mut_obj.get("ids").and_then(|v| v.as_array());
                        let offset = mut_obj.get("offset").and_then(|v| v.as_array()).and_then(|a| {
                            if a.len() == 2 {
                                Some((a[0].as_f64().unwrap_or(20.0), a[1].as_f64().unwrap_or(20.0)))
                            } else {
                                None
                            }
                        }).unwrap_or((20.0, 20.0));
                        if let Some(arr) = ids_arr {
                            let ids_vec: Vec<String> = arr.iter().filter_map(|s| s.as_str().map(|v| v.to_string())).collect();
                            if let Err(e) = mutate_duplicate(&mut doc, &ids_vec, offset) {
                                eprintln!("batch mutation #{idx} (duplicate) failed: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    "update" | "update-elem" | "update_elem" => {
                        let target_id = mut_obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let updates_map = if let Some(set_obj) = mut_obj.get("set") {
                            set_obj
                        } else {
                            mut_obj
                        };
                        if let Err(e) = mutate_update_elem(&mut doc, target_id, updates_map) {
                            eprintln!("batch mutation #{idx} (update) failed: {e}");
                            process::exit(1);
                        }
                    }
                    "clear" => {
                        let _ = mutate_clear(&mut doc);
                    }
                    other => {
                        eprintln!("unknown batch action: {other}");
                        process::exit(1);
                    }
                }
            }

            if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                eprintln!("error saving batch changes: {e}");
                process::exit(1);
            }
            println!("OK: applied {} batch mutations atomically", changes_arr.len());
        }
        "theme" => {
            if extra_args.is_empty() {
                eprintln!("theme mode requires a subcommand: 'export' or 'apply'");
                process::exit(1);
            }
            let subcmd = &extra_args[0];
            let sub_opts = &extra_args[1..];

            match subcmd.as_str() {
                "export" => {
                    let theme = Theme::default_retro();
                    let theme_json = serde_json::to_string_pretty(&theme).unwrap();
                    if !sub_opts.is_empty() {
                        let out_file = &sub_opts[0];
                        if let Err(e) = fs::write(out_file, &theme_json) {
                            eprintln!("failed to export theme to {out_file:?}: {e}");
                            process::exit(1);
                        }
                        println!("OK: exported theme to {out_file}");
                    } else {
                        println!("{theme_json}");
                    }
                }
                "apply" => {
                    if sub_opts.is_empty() {
                        eprintln!("theme apply requires <theme.json | default | retro-terminal | aops-default>");
                        process::exit(1);
                    }
                    let theme_name = &sub_opts[0];
                    let theme = if theme_name == "default" || theme_name == "retro-terminal" || theme_name == "aops-default" {
                        Theme::default_retro()
                    } else {
                        let content = match fs::read_to_string(theme_name) {
                            Ok(c) => c,
                            Err(e) => {
                                eprintln!("failed to read theme file {theme_name:?}: {e}");
                                process::exit(1);
                            }
                        };
                        match serde_json::from_str(&content) {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("failed to parse theme JSON: {e}");
                                process::exit(1);
                            }
                        }
                    };

                    let mut apply_opts = &sub_opts[1..];
                    let mut all = true;
                    let mut target_id: Option<String> = None;

                    while !apply_opts.is_empty() {
                        let flag = &apply_opts[0];
                        apply_opts = &apply_opts[1..];
                        if flag == "--all" {
                            all = true;
                        } else if flag == "--id" && !apply_opts.is_empty() {
                            target_id = Some(apply_opts[0].clone());
                            apply_opts = &apply_opts[1..];
                            all = false;
                        }
                    }

                    match mutate_theme_apply(&mut doc, &theme, target_id.as_deref(), all) {
                        Ok(count) => {
                            if let Err(e) = atomic_save_ex(file_path, &mut doc, is_obsidian) {
                                eprintln!("error saving file: {e}");
                                process::exit(1);
                            }
                            println!("OK: applied theme '{}' to {count} elements", theme.name);
                        }
                        Err(e) => {
                            eprintln!("error applying theme: {e}");
                            process::exit(1);
                        }
                    }
                }
                other => {
                    eprintln!("unknown theme subcommand {other:?}; expected 'export' or 'apply'");
                    process::exit(1);
                }
            }
        }
        "describe" => {
            cmd_describe(&doc);
        }
        "screenshot" => {
            let mut opts = extra_args;
            let mut out_path: Option<String> = None;
            let mut format: Option<String> = None;
            let mut no_background = false;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--out" && !opts.is_empty() {
                    out_path = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--format" && !opts.is_empty() {
                    format = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--no-background" {
                    no_background = true;
                }
            }

            cmd_screenshot(&doc, out_path.as_deref(), format.as_deref(), no_background);
        }
        "arrange" | "align" | "distribute" | "group" | "ungroup" | "lock" | "unlock" | "duplicate" => {
            let (op, opts) = if mode == "arrange" {
                if extra_args.is_empty() {
                    eprintln!("arrange requires a subcommand: align|distribute|group|ungroup|lock|unlock|duplicate");
                    process::exit(1);
                }
                (extra_args[0].as_str(), &extra_args[1..])
            } else {
                (mode, extra_args)
            };

            let mut ids: Vec<String> = Vec::new();
            let mut to: Option<String> = None;
            let mut group_id: Option<String> = None;
            let mut offset: (f64, f64) = (20.0, 20.0);

            let mut cur_opts = opts;
            while !cur_opts.is_empty() {
                let flag = &cur_opts[0];
                cur_opts = &cur_opts[1..];
                if flag == "--ids" && !cur_opts.is_empty() {
                    ids = cur_opts[0].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                    cur_opts = &cur_opts[1..];
                } else if flag == "--to" && !cur_opts.is_empty() {
                    to = Some(cur_opts[0].clone());
                    cur_opts = &cur_opts[1..];
                } else if flag == "--group" && !cur_opts.is_empty() {
                    group_id = Some(cur_opts[0].clone());
                    cur_opts = &cur_opts[1..];
                } else if flag == "--offset" && !cur_opts.is_empty() {
                    let parts: Vec<&str> = cur_opts[0].split(',').collect();
                    if parts.len() == 2 {
                        let dx = parts[0].trim().parse::<f64>().unwrap_or(20.0);
                        let dy = parts[1].trim().parse::<f64>().unwrap_or(20.0);
                        offset = (dx, dy);
                    }
                    cur_opts = &cur_opts[1..];
                }
            }

            match op {
                "align" => {
                    let to_dir = match to {
                        Some(d) => d,
                        None => {
                            eprintln!("arrange align requires --to left|center|right|top|middle|bottom");
                            process::exit(1);
                        }
                    };
                    match mutate_align(&mut doc, &ids, &to_dir) {
                        Ok(cnt) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: aligned {cnt} elements to {to_dir}");
                        }
                        Err(e) => {
                            eprintln!("error aligning: {e}");
                            process::exit(1);
                        }
                    }
                }
                "distribute" => {
                    let to_dir = match to {
                        Some(d) => d,
                        None => {
                            eprintln!("arrange distribute requires --to horizontal|vertical");
                            process::exit(1);
                        }
                    };
                    match mutate_distribute(&mut doc, &ids, &to_dir) {
                        Ok(cnt) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: distributed {cnt} elements {to_dir}");
                        }
                        Err(e) => {
                            eprintln!("error distributing: {e}");
                            process::exit(1);
                        }
                    }
                }
                "group" => {
                    match mutate_group(&mut doc, &ids) {
                        Ok(gid) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: grouped {} elements into {gid}", ids.len());
                        }
                        Err(e) => {
                            eprintln!("error grouping: {e}");
                            process::exit(1);
                        }
                    }
                }
                "ungroup" => {
                    let ids_opt = if ids.is_empty() { None } else { Some(ids.as_slice()) };
                    match mutate_ungroup(&mut doc, group_id.as_deref(), ids_opt) {
                        Ok(cnt) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: ungrouped {cnt} elements");
                        }
                        Err(e) => {
                            eprintln!("error ungrouping: {e}");
                            process::exit(1);
                        }
                    }
                }
                "lock" => {
                    match mutate_lock(&mut doc, &ids, true) {
                        Ok(cnt) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: locked {cnt} elements");
                        }
                        Err(e) => {
                            eprintln!("error locking: {e}");
                            process::exit(1);
                        }
                    }
                }
                "unlock" => {
                    match mutate_lock(&mut doc, &ids, false) {
                        Ok(cnt) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: unlocked {cnt} elements");
                        }
                        Err(e) => {
                            eprintln!("error unlocking: {e}");
                            process::exit(1);
                        }
                    }
                }
                "duplicate" => {
                    match mutate_duplicate(&mut doc, &ids, offset) {
                        Ok(new_ids) => {
                            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                                eprintln!("error saving: {e}");
                                process::exit(1);
                            });
                            println!("OK: duplicated {} elements: {}", new_ids.len(), new_ids.join(", "));
                        }
                        Err(e) => {
                            eprintln!("error duplicating: {e}");
                            process::exit(1);
                        }
                    }
                }
                other => {
                    eprintln!("unknown arrange operation {other:?}");
                    process::exit(1);
                }
            }
        }
        "update" => {
            if extra_args.is_empty() {
                eprintln!("update requires <id> --set '<json>'");
                process::exit(1);
            }
            let target_id = &extra_args[0];
            let mut opts = &extra_args[1..];
            let mut set_json: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--set" && !opts.is_empty() {
                    set_json = Some(opts[0].clone());
                    opts = &opts[1..];
                }
            }

            let updates: Value = match set_json {
                Some(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                    eprintln!("invalid JSON in --set: {e}");
                    process::exit(1);
                }),
                None => {
                    eprintln!("update requires --set '<json>'");
                    process::exit(1);
                }
            };

            match mutate_update_elem(&mut doc, target_id, &updates) {
                Ok(_) => {
                    atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                        eprintln!("error saving: {e}");
                        process::exit(1);
                    });
                    println!("OK: updated element {target_id}");
                }
                Err(e) => {
                    eprintln!("error updating element: {e}");
                    process::exit(1);
                }
            }
        }
        "query" => {
            let mut opts = extra_args;
            let mut elem_type: Option<String> = None;
            let mut bbox: Option<(f64, f64, f64, f64)> = None;
            let mut filter_strings: Vec<String> = Vec::new();
            let mut filter_json: Option<Value> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--type" && !opts.is_empty() {
                    elem_type = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--bbox" && !opts.is_empty() {
                    let parts: Vec<&str> = opts[0].split(',').collect();
                    if parts.len() == 4 {
                        let x0 = parts[0].trim().parse::<f64>().unwrap_or(0.0);
                        let y0 = parts[1].trim().parse::<f64>().unwrap_or(0.0);
                        let x1 = parts[2].trim().parse::<f64>().unwrap_or(0.0);
                        let y1 = parts[3].trim().parse::<f64>().unwrap_or(0.0);
                        bbox = Some((x0, y0, x1, y1));
                    }
                    opts = &opts[1..];
                } else if flag == "--filter" && !opts.is_empty() {
                    filter_strings.push(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--filter-json" && !opts.is_empty() {
                    filter_json = serde_json::from_str(&opts[0]).ok();
                    opts = &opts[1..];
                }
            }

            let mut filters: Vec<(&str, &str)> = Vec::new();
            for s in &filter_strings {
                if let Some(eq) = s.find('=') {
                    filters.push((&s[..eq], &s[eq + 1..]));
                }
            }

            cmd_query(&doc, elem_type.as_deref(), bbox, &filters, filter_json.as_ref());
        }
        "apply" => {
            let input_raw = if extra_args.is_empty() || extra_args[0] == "-" {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).unwrap_or_default();
                buf
            } else {
                fs::read_to_string(&extra_args[0]).unwrap_or_default()
            };

            let patch: Value = match serde_json::from_str(&input_raw) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to parse apply JSON patch: {e}");
                    process::exit(1);
                }
            };

            match cmd_apply(&mut doc, &patch) {
                Ok((c, u, d)) => {
                    atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    });
                    println!("OK: applied patch (created: {c}, updated: {u}, deleted: {d})");
                }
                Err(e) => {
                    eprintln!("error applying patch: {e}");
                    process::exit(1);
                }
            }
        }
        "export" => {
            let mut opts = extra_args;
            let mut out_path: Option<String> = None;
            let mut format: Option<String> = None;

            while !opts.is_empty() {
                let flag = &opts[0];
                opts = &opts[1..];
                if flag == "--out" && !opts.is_empty() {
                    out_path = Some(opts[0].clone());
                    opts = &opts[1..];
                } else if flag == "--format" && !opts.is_empty() {
                    format = Some(opts[0].clone());
                    opts = &opts[1..];
                }
            }

            let is_obs = match format.as_deref() {
                Some("obsidian") => true,
                Some("json") => false,
                _ => out_path.as_deref().map(|p| p.ends_with(".md")).unwrap_or(false),
            };

            let output = if is_obs {
                wrap_scene_as_obsidian_md(&doc).unwrap_or_else(|e| {
                    eprintln!("failed to wrap Obsidian markdown: {e}");
                    process::exit(1);
                })
            } else {
                serde_json::to_string_pretty(&doc).unwrap()
            };

            if let Some(p) = out_path {
                if let Err(e) = fs::write(&p, output) {
                    eprintln!("failed to write export to {p:?}: {e}");
                    process::exit(1);
                }
                let res = json!({
                    "success": true,
                    "file": p,
                    "elements": live(&doc).len(),
                    "format": if is_obs { "obsidian" } else { "json" }
                });
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                println!("{output}");
            }
        }
        "import" => {
            if extra_args.is_empty() {
                eprintln!("import requires a file or '-' for stdin: pkb-excalidraw FILE import <file | -> [--replace]");
                process::exit(1);
            }
            let src_file = &extra_args[0];
            let replace = extra_args.iter().any(|arg| arg == "--replace");

            let input_raw = if src_file == "-" {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).unwrap_or_default();
                buf
            } else {
                fs::read_to_string(src_file).unwrap_or_else(|e| {
                    eprintln!("failed to read import file {src_file:?}: {e}");
                    process::exit(1);
                })
            };

            let src_json_str = if is_obsidian_excalidraw_md(&input_raw) {
                extract_scene_json_from_obsidian_md(&input_raw).unwrap_or_else(|e| {
                    eprintln!("failed to extract JSON from import file: {e}");
                    process::exit(1);
                })
            } else {
                input_raw
            };

            let src_doc: Value = serde_json::from_str(&src_json_str).unwrap_or_else(|e| {
                eprintln!("failed to parse import JSON: {e}");
                process::exit(1);
            });

            let src_elements = src_doc
                .get("elements")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| {
                    eprintln!("import document missing elements array");
                    process::exit(1);
                });

            if replace {
                doc["elements"] = json!(src_elements);
            } else {
                let max_idx = max_index_of(&doc);
                let new_indices = mint_indices(&max_idx, src_elements.len()).unwrap_or_else(|e| {
                    eprintln!("failed to mint indices: {e}");
                    process::exit(1);
                });
                let elements_mut = doc.get_mut("elements").and_then(|v| v.as_array_mut()).unwrap();
                for (i, el) in src_elements.iter().enumerate() {
                    let mut clone = el.clone();
                    clone["index"] = json!(new_indices[i]);
                    elements_mut.push(clone);
                }
            }

            atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                eprintln!("error saving file: {e}");
                process::exit(1);
            });
            println!("OK: imported {} elements ({})", src_elements.len(), if replace { "replace" } else { "merge" });
        }
        "snapshot" => {
            if extra_args.is_empty() {
                eprintln!("snapshot requires subcommand: save <name> | list | restore <name>");
                process::exit(1);
            }
            let sub = &extra_args[0];
            match sub.as_str() {
                "save" => {
                    if extra_args.len() < 2 {
                        eprintln!("snapshot save requires a name");
                        process::exit(1);
                    }
                    cmd_snapshot_save(file_path, &doc, &extra_args[1]).unwrap_or_else(|e| {
                        eprintln!("error saving snapshot: {e}");
                        process::exit(1);
                    });
                }
                "list" => {
                    cmd_snapshot_list(file_path).unwrap_or_else(|e| {
                        eprintln!("error listing snapshots: {e}");
                        process::exit(1);
                    });
                }
                "restore" => {
                    if extra_args.len() < 2 {
                        eprintln!("snapshot restore requires a name");
                        process::exit(1);
                    }
                    cmd_snapshot_restore(file_path, &mut doc, &extra_args[1]).unwrap_or_else(|e| {
                        eprintln!("error restoring snapshot: {e}");
                        process::exit(1);
                    });
                }
                other => {
                    eprintln!("unknown snapshot subcommand {other:?}");
                    process::exit(1);
                }
            }
        }
        "clear" => {
            let yes = extra_args.iter().any(|a| a == "--yes");
            if !yes {
                eprintln!("clear wipes the entire canvas; pass --yes to confirm");
                process::exit(1);
            }
            match mutate_clear(&mut doc) {
                Ok(cnt) => {
                    atomic_save_ex(file_path, &mut doc, is_obsidian).unwrap_or_else(|e| {
                        eprintln!("error saving file: {e}");
                        process::exit(1);
                    });
                    println!("OK: cleared canvas ({cnt} elements removed)");
                }
                Err(e) => {
                    eprintln!("error clearing canvas: {e}");
                    process::exit(1);
                }
            }
        }
        _ => {
            let modes = [
                "summary", "map", "style", "check", "diff", "struct-diff", "lib", "item",
                "nodes", "edges", "arrows", "inspect", "get", "add-node", "add-text",
                "connect", "set-text", "fit", "move-elem", "delete-elem", "batch",
                "theme", "overlap", "arrows-check",
                "describe", "screenshot", "arrange", "align", "distribute", "group",
                "ungroup", "lock", "unlock", "duplicate", "update", "query", "apply",
                "export", "import", "snapshot", "clear"
            ];
            eprintln!(
                "unknown mode {mode:?}; expected one of: {}",
                modes.join(", ")
            );
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_and_element_text() {
        let doc = json!({
            "elements": [
                { "id": "1", "type": "rectangle", "isDeleted": false, "text": "foo" },
                { "id": "2", "type": "text", "isDeleted": true, "text": "deleted" },
                { "id": "3", "type": "text", "text": "wrapped\ntext", "originalText": "original text" }
            ]
        });
        let els = live(&doc);
        assert_eq!(els.len(), 2);
        assert_eq!(element_text(els[0]), "foo");
        assert_eq!(element_text(els[1]), "original text");
    }

    #[test]
    fn test_label_of() {
        let t1 = json!({ "id": "t1", "type": "text", "text": "Line1\nLine2" });
        let mut texts = HashMap::new();
        texts.insert("t1", &t1);

        let rect = json!({
            "id": "r1",
            "type": "rectangle",
            "boundElements": [
                { "id": "t1", "type": "text" }
            ]
        });
        assert_eq!(label_of(&rect, &texts), "Line1 / Line2");
    }

    #[test]
    fn test_map_arrow_label_bug_task_3bff27d4() {
        let t1 = json!({ "id": "t1", "type": "text", "text": "Flow Step", "containerId": "a1" });
        let mut texts = HashMap::new();
        texts.insert("t1", &t1);

        let arrow = json!({
            "id": "a1",
            "type": "arrow",
            "startBinding": { "elementId": "r1" },
            "endBinding": { "elementId": "r2" },
            "boundElements": [
                { "id": "t1", "type": "text" }
            ]
        });
        let lbl = label_of(&arrow, &texts);
        assert_eq!(lbl, "Flow Step");
    }

    #[test]
    fn test_check_half_bound_arrow_bug_task_737c102e() {
        let doc_half_start = json!({
            "elements": [
                { "id": "r1", "type": "rectangle", "index": "a0" },
                { "id": "a1", "type": "arrow", "index": "a1", "startBinding": { "elementId": "r1" } }
            ]
        });
        let res = cmd_check(&doc_half_start);
        assert!(res.is_err());
        let fails = res.unwrap_err();
        assert!(fails.iter().any(|f| f.contains("arrow a1 is half-bound: startBinding=r1, endBinding=none")));

        let doc_half_end = json!({
            "elements": [
                { "id": "r2", "type": "rectangle", "index": "a0" },
                { "id": "a1", "type": "arrow", "index": "a1", "endBinding": { "elementId": "r2" } }
            ]
        });
        let res = cmd_check(&doc_half_end);
        assert!(res.is_err());
        let fails = res.unwrap_err();
        assert!(fails.iter().any(|f| f.contains("arrow a1 is half-bound: startBinding=none, endBinding=r2")));

        let doc_unbound = json!({
            "elements": [
                { "id": "a1", "type": "arrow", "index": "a0" }
            ]
        });
        assert!(cmd_check(&doc_unbound).is_ok());

        let doc_both_bound = json!({
            "elements": [
                { "id": "r1", "type": "rectangle", "index": "a0" },
                { "id": "r2", "type": "rectangle", "index": "a1" },
                { "id": "a1", "type": "arrow", "index": "a2", "startBinding": { "elementId": "r1" }, "endBinding": { "elementId": "r2" } }
            ]
        });
        assert!(cmd_check(&doc_both_bound).is_ok());
    }

    #[test]
    fn test_mint_indices() {
        let indices = mint_indices("a0", 3).unwrap();
        assert_eq!(indices, vec!["a000", "a001", "a002"]);
        assert!(indices.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn test_compute_text_dimensions() {
        let (w, h) = compute_text_dimensions("Hello\nWorld", 20.0);
        assert!(w > 0.0);
        assert_eq!(h, 2.0 * 20.0 * 1.25);
    }

    #[test]
    fn test_geometry_overlap_and_segment_intersection() {
        let r1 = (100.0, 100.0, 100.0, 50.0);
        let r2 = (150.0, 120.0, 100.0, 50.0);
        let r3 = (300.0, 300.0, 50.0, 50.0);

        assert!(rects_overlap(r1, r2));
        assert!(!rects_overlap(r1, r3));

        // Line passing straight through r1
        let p1 = (50.0, 125.0);
        let p2 = (250.0, 125.0);
        assert!(segment_intersects_rect(p1, p2, r1));

        // Line missing r1
        let p3 = (50.0, 200.0);
        let p4 = (250.0, 200.0);
        assert!(!segment_intersects_rect(p3, p4, r1));
    }

    #[test]
    fn test_mutate_delete_elem_cascade_arrows_and_labels() {
        let mut doc = json!({
            "elements": [
                { "id": "n1", "type": "rectangle", "boundElements": [{ "id": "arr1", "type": "arrow" }] },
                { "id": "arr1", "type": "arrow", "startBinding": { "elementId": "n1" }, "endBinding": { "elementId": "n2" }, "boundElements": [{ "id": "lbl1", "type": "text" }] },
                { "id": "lbl1", "type": "text", "containerId": "arr1", "text": "Label" },
                { "id": "n2", "type": "rectangle", "boundElements": [{ "id": "arr1", "type": "arrow" }] }
            ]
        });
        mutate_delete_elem(&mut doc, "n1", true).unwrap();
        let els = doc["elements"].as_array().unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0]["id"], "n2");
        assert!(els[0]["boundElements"].is_null());
    }

    #[test]
    fn test_mutate_delete_elem_non_cascading_strips_survivor_bound_elements() {
        let mut doc = json!({
            "elements": [
                { "id": "n1", "type": "rectangle", "boundElements": [{ "id": "arr1", "type": "arrow" }] },
                { "id": "arr1", "type": "arrow", "startBinding": { "elementId": "n1" }, "endBinding": { "elementId": "n2" }, "boundElements": [{ "id": "lbl1", "type": "text" }] },
                { "id": "lbl1", "type": "text", "containerId": "arr1", "text": "Label" },
                { "id": "n2", "type": "rectangle", "boundElements": [{ "id": "arr1", "type": "arrow" }] }
            ]
        });
        mutate_delete_elem(&mut doc, "n1", false).unwrap();
        let els = doc["elements"].as_array().unwrap();
        assert_eq!(els.len(), 3);
        let n2 = els.iter().find(|e| e["id"] == "n2").unwrap();
        assert!(n2["boundElements"].is_null());
        let arr1 = els.iter().find(|e| e["id"] == "arr1").unwrap();
        assert!(arr1["startBinding"].is_null());
        assert!(arr1["endBinding"].is_null());
    }

    #[test]
    fn test_mutate_set_text_arrow_preserves_arrow_geometry() {
        let mut doc = json!({
            "elements": [
                {
                    "id": "arr1",
                    "type": "arrow",
                    "x": 100.0,
                    "y": 100.0,
                    "width": 200.0,
                    "height": 0.0,
                    "points": [[0.0, 0.0], [200.0, 0.0]],
                    "boundElements": [{ "id": "lbl1", "type": "text" }]
                },
                {
                    "id": "lbl1",
                    "type": "text",
                    "containerId": "arr1",
                    "x": 180.0,
                    "y": 90.0,
                    "width": 40.0,
                    "height": 20.0,
                    "text": "Old",
                    "originalText": "Old"
                }
            ]
        });
        mutate_set_text(&mut doc, "lbl1", "New Long Text").unwrap();
        let arr = &doc["elements"][0];
        let lbl = &doc["elements"][1];
        assert_eq!(arr["x"], 100.0);
        assert_eq!(arr["y"], 100.0);
        assert_eq!(arr["width"], 200.0);
        assert_eq!(arr["height"], 0.0);
        assert_eq!(lbl["text"], "New Long Text");
    }

    #[test]
    fn test_mutate_move_elem_translates_arrow_labels() {
        let mut doc = json!({
            "elements": [
                { "id": "n1", "type": "rectangle", "x": 100.0, "y": 100.0, "width": 100.0, "height": 50.0 },
                { "id": "n2", "type": "rectangle", "x": 300.0, "y": 100.0, "width": 100.0, "height": 50.0 },
                {
                    "id": "arr1",
                    "type": "arrow",
                    "x": 150.0,
                    "y": 125.0,
                    "width": 150.0,
                    "height": 0.0,
                    "points": [[0.0, 0.0], [150.0, 0.0]],
                    "startBinding": { "elementId": "n1" },
                    "endBinding": { "elementId": "n2" },
                    "boundElements": [{ "id": "lbl1", "type": "text" }]
                },
                {
                    "id": "lbl1",
                    "type": "text",
                    "containerId": "arr1",
                    "x": 205.0,
                    "y": 115.0,
                    "width": 40.0,
                    "height": 20.0,
                    "text": "Label"
                }
            ]
        });
        mutate_move_elem(&mut doc, "n1", None, Some((40.0, 20.0))).unwrap();
        let lbl = &doc["elements"][3];
        assert_eq!(lbl["x"], 225.0);
        assert_eq!(lbl["y"], 125.0);
    }

    #[test]
    fn test_mutate_theme_apply_cascades_container_and_text() {
        let mut doc = json!({
            "elements": [
                {
                    "id": "box1",
                    "type": "rectangle",
                    "strokeColor": "#000000",
                    "customData": { "role": "warning" },
                    "boundElements": [{ "id": "txt1", "type": "text" }]
                },
                {
                    "id": "txt1",
                    "type": "text",
                    "containerId": "box1",
                    "fontFamily": 3,
                    "strokeColor": "#000000"
                }
            ]
        });
        let theme = Theme::default_retro();
        let count = mutate_theme_apply(&mut doc, &theme, Some("box1"), false).unwrap();
        assert_eq!(count, 2);
        assert_eq!(doc["elements"][0]["backgroundColor"], "#ffa500");
        assert_eq!(doc["elements"][1]["fontFamily"], 1);
        assert_eq!(doc["elements"][1]["strokeColor"], "#1a1a1a");
    }
}
