//! PKB Language Server Protocol (LSP) implementation.
//!
//! Provides hover preview, definition resolution, and document links
//! for PKB references (`[[id]]`, `[[id|alias]]`, `[label](target)`, etc.) in markdown files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use lsp_types::*;
use parking_lot::RwLock;
use regex::Regex;
use tracing::{debug, info, warn};

use crate::graph::GraphNode;
use crate::graph_store::GraphStore;

static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]+))?\]\]").unwrap());

static MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());

/// A reference span located within a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceSpan {
    pub target: String,
    pub alias: Option<String>,
    pub range: Range,
}

fn is_pkb_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == '.'
}

fn path_to_uri(path: &Path) -> Option<Uri> {
    if let Ok(url) = url::Url::from_file_path(path) {
        if let Ok(uri) = Uri::from_str(url.as_str()) {
            return Some(uri);
        }
    }
    Uri::from_str(&format!("file://{}", path.to_string_lossy())).ok()
}

/// A gutter badge item indicating a task reference on a specific line.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GutterBadgeItem {
    pub line: u32,
    #[serde(alias = "task_id")]
    pub task_id: String,
    pub title: String,
    pub status: Option<String>,
    pub weight: Option<f64>,
    pub badge: String,
}

pub fn type_codicon(node_type: &str) -> &'static str {
    match node_type {
        "task" | "subtask" => "$(checklist)",
        "note" => "$(note)",
        "knowledge" | "doc" | "reference" => "$(book)",
        "memory" => "$(history)",
        "project" | "epic" => "$(project)",
        "pr" => "$(git-pull-request)",
        "spec" | "template" => "$(file-code)",
        _ => "$(symbol-misc)",
    }
}

pub fn status_codicon(status: &str) -> &'static str {
    match status {
        "ready" => "$(play)",
        "in_progress" => "$(sync~spin)",
        "blocked" => "$(circle-slash)",
        "done" => "$(check)",
        "review" => "$(eye)",
        "paused" => "$(debug-pause)",
        "inbox" => "$(inbox)",
        "cancelled" | "failed" | "archived" => "$(close)",
        _ => "$(circle-outline)",
    }
}

/// Helper to check whether a node is a task or actionable work item.
pub fn is_task_node(node: &GraphNode) -> bool {
    if let Some(ref t) = node.node_type {
        let t_lower = t.to_lowercase();
        if crate::graph::TASK_TYPES.contains(&t_lower.as_str())
            || t_lower == "subtask"
            || t_lower == "epic"
            || t_lower == "project"
        {
            return true;
        }
    }
    node.id.starts_with("task-")
        || (node.status.is_some() && node.node_type.as_deref() != Some("note"))
}

pub struct LspServer {
    pub pkb_root: PathBuf,
    pub graph: Arc<RwLock<GraphStore>>,
    pub documents: HashMap<Uri, String>,
}

impl LspServer {
    pub fn new(pkb_root: PathBuf) -> Self {
        if !pkb_root.exists() {
            warn!(
                pkb_root = %pkb_root.display(),
                "PKB root directory does not exist when initializing LspServer"
            );
        }
        let start = Instant::now();
        let store = GraphStore::build_from_directory(&pkb_root);
        let elapsed = start.elapsed();
        let node_count = store.node_count();
        info!(
            node_count = node_count,
            pkb_root = %pkb_root.display(),
            elapsed_ms = elapsed.as_millis() as u64,
            "LspServer initialized graph from directory"
        );
        if node_count == 0 {
            warn!(
                pkb_root = %pkb_root.display(),
                "0 nodes were loaded from pkb_root"
            );
        }
        Self {
            pkb_root,
            graph: Arc::new(RwLock::new(store)),
            documents: HashMap::new(),
        }
    }

    /// Refresh graph from disk if files have changed.
    pub fn ensure_fresh(&self) {
        let current_gen = crate::pkb::scan_generation(&self.pkb_root);
        let cached_gen = self.graph.read().generation();
        if current_gen != cached_gen {
            let start = Instant::now();
            let new_store = GraphStore::build_from_directory(&self.pkb_root);
            let elapsed = start.elapsed();
            let node_count = new_store.node_count();
            info!(
                node_count = node_count,
                pkb_root = %self.pkb_root.display(),
                elapsed_ms = elapsed.as_millis() as u64,
                "Reloaded PKB graph after file changes"
            );
            let mut store = self.graph.write();
            *store = new_store;
        }
    }

    pub fn handle_did_open(&mut self, params: DidOpenTextDocumentParams) {
        debug!(uri = %params.text_document.uri.as_str(), "textDocument/didOpen");
        self.documents
            .insert(params.text_document.uri, params.text_document.text);
    }

    pub fn handle_did_change(&mut self, params: DidChangeTextDocumentParams) {
        debug!(uri = %params.text_document.uri.as_str(), "textDocument/didChange");
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents.insert(params.text_document.uri, change.text);
        }
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) {
        debug!(uri = %params.text_document.uri.as_str(), "textDocument/didClose");
        self.documents.remove(&params.text_document.uri);
    }

    /// Get document content either from memory or read from disk.
    pub fn get_document_content(&self, uri: &Uri) -> Option<String> {
        if let Some(text) = self.documents.get(uri) {
            return Some(text.clone());
        }

        // Try reading from file path if it's a file URI
        let uri_str = uri.as_str();
        if let Ok(url) = url::Url::parse(uri_str) {
            if let Ok(path) = url.to_file_path() {
                if let Ok(content) = fs::read_to_string(path) {
                    return Some(content);
                }
            }
        }
        if let Some(path_str) = uri_str.strip_prefix("file://") {
            if let Ok(content) = fs::read_to_string(path_str) {
                return Some(content);
            }
        }
        warn!(
            uri = %uri_str,
            "Document content could not be found in memory or read from disk"
        );
        None
    }

    /// Find a reference at the given line and character position.
    pub fn find_reference_at_position(
        text: &str,
        line_idx: u32,
        char_idx: u32,
    ) -> Option<ReferenceSpan> {
        let lines: Vec<&str> = text.lines().collect();
        let line = lines.get(line_idx as usize)?;

        // 1. Search for wikilinks: [[target]] or [[target|alias]]
        for cap in WIKILINK_RE.captures_iter(line) {
            let full_match = cap.get(0)?;
            let start = full_match.start() as u32;
            let end = full_match.end() as u32;

            if char_idx >= start && char_idx <= end {
                let target = cap.get(1)?.as_str().trim().to_string();
                let alias = cap.get(2).map(|m| m.as_str().trim().to_string());
                return Some(ReferenceSpan {
                    target,
                    alias,
                    range: Range {
                        start: Position {
                            line: line_idx,
                            character: start,
                        },
                        end: Position {
                            line: line_idx,
                            character: end,
                        },
                    },
                });
            }
        }

        // 2. Search for standard markdown links: [label](target)
        for cap in MD_LINK_RE.captures_iter(line) {
            let full_match = cap.get(0)?;
            let start = full_match.start() as u32;
            let end = full_match.end() as u32;

            if char_idx >= start && char_idx <= end {
                let target_raw = cap.get(2)?.as_str().trim();
                if !target_raw.starts_with("http://") && !target_raw.starts_with("https://") {
                    let label = cap.get(1)?.as_str().trim().to_string();
                    return Some(ReferenceSpan {
                        target: target_raw.to_string(),
                        alias: Some(label),
                        range: Range {
                            start: Position {
                                line: line_idx,
                                character: start,
                            },
                            end: Position {
                                line: line_idx,
                                character: end,
                            },
                        },
                    });
                }
            }
        }

        // 3. Search for bare word / ID at cursor
        let line_chars: Vec<char> = line.chars().collect();
        let c_idx = char_idx as usize;
        if c_idx < line_chars.len() && is_pkb_id_char(line_chars[c_idx]) {
            let mut start = c_idx;
            while start > 0 && is_pkb_id_char(line_chars[start - 1]) {
                start -= 1;
            }
            let mut end = c_idx;
            while end + 1 < line_chars.len() && is_pkb_id_char(line_chars[end + 1]) {
                end += 1;
            }
            let word: String = line_chars[start..=end].iter().collect();
            // Don't treat common short words as IDs unless they have an id prefix/structure or resolve
            return Some(ReferenceSpan {
                target: word,
                alias: None,
                range: Range {
                    start: Position {
                        line: line_idx,
                        character: start as u32,
                    },
                    end: Position {
                        line: line_idx,
                        character: (end + 1) as u32,
                    },
                },
            });
        }

        None
    }

    /// Resolve a reference string to a canonical GraphNode and absolute file path.
    pub fn resolve_node(&self, target: &str) -> Option<(GraphNode, PathBuf)> {
        let graph = self.graph.read();
        let node = graph.resolve(target)?.clone();
        let abs_path = if node.path.is_absolute() {
            node.path.clone()
        } else {
            self.pkb_root.join(&node.path)
        };
        Some((node, abs_path))
    }

    /// Resolve all `[[id]]` and `[[id|alias]]` in a text snippet to human title links.
    pub fn resolve_wikilinks_to_human_titles(&self, text: &str) -> String {
        WIKILINK_RE
            .replace_all(text, |caps: &regex::Captures| {
                let target = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                let explicit_alias = caps.get(2).map(|m| m.as_str().trim());
                if let Some((target_node, target_path)) = self.resolve_node(target) {
                    let label = explicit_alias.unwrap_or(&target_node.label);
                    let uri = path_to_uri(&target_path)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| format!("file://{}", target_path.to_string_lossy()));
                    format!("[{}]({})", label, uri)
                } else if let Some(alias) = explicit_alias {
                    alias.to_string()
                } else {
                    target.to_string()
                }
            })
            .to_string()
    }

    pub fn format_body_micro_headers(body: &str) -> String {
        let lines: Vec<&str> = body.lines().collect();
        let mut out_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if trimmed.starts_with('#') {
                let hashes = trimmed.chars().take_while(|&c| c == '#').count();
                if hashes > 0 && trimmed[hashes..].starts_with(' ') {
                    let heading_text = trimmed[hashes..].trim();
                    let micro_header = format!("**{}:**", heading_text);

                    // Find next non-empty line
                    let mut next_idx = i + 1;
                    while next_idx < lines.len() && lines[next_idx].trim().is_empty() {
                        next_idx += 1;
                    }

                    if next_idx < lines.len() {
                        let next_trimmed = lines[next_idx].trim();
                        let is_block_item = next_trimmed.starts_with('-')
                            || next_trimmed.starts_with('*')
                            || next_trimmed.starts_with('+')
                            || next_trimmed.starts_with('>')
                            || next_trimmed.starts_with("```")
                            || (next_trimmed
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_ascii_digit())
                                && next_trimmed.contains(". "));

                        if is_block_item {
                            out_lines.push(micro_header);
                            i = next_idx;
                            continue;
                        } else if !next_trimmed.starts_with('#') {
                            out_lines.push(format!("{} {}", micro_header, next_trimmed));
                            i = next_idx + 1;
                            continue;
                        }
                    }

                    out_lines.push(micro_header);
                    i = next_idx;
                    continue;
                }
            }

            if trimmed.is_empty() {
                if let Some(last) = out_lines.last() {
                    if !last.is_empty() {
                        out_lines.push(String::new());
                    }
                }
            } else {
                out_lines.push(line.to_string());
            }

            i += 1;
        }

        out_lines.join("\n")
    }

    /// Format a Markdown preview for a node.
    pub fn format_hover_preview(&self, node: &GraphNode, abs_path: &Path) -> String {
        let mut lines = Vec::new();

        // Title and ID header
        lines.push(format!("### {} (`{}`)\n", node.label, node.id));

        // Top bar: type, status, weight/intent, tags as codicons with compact labels
        let mut top_bar_items = Vec::new();
        if let Some(ref t) = node.node_type {
            top_bar_items.push(format!("{} {}", type_codicon(t), t));
        }
        if let Some(ref s) = node.status {
            top_bar_items.push(format!("{} {}", status_codicon(s), s));
        }
        if let Some(i) = node.intent {
            top_bar_items.push(format!("$(flame) intent {}", i));
        }
        if !node.tags.is_empty() {
            let tags_str = node
                .tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" ");
            top_bar_items.push(format!("$(tag) {}", tags_str));
        }

        if !top_bar_items.is_empty() {
            lines.push(top_bar_items.join("  ·  "));
            lines.push(String::new());
        }

        if let Some(ref parent_id) = node.parent {
            if let Some((parent_node, parent_path)) = self.resolve_node(parent_id) {
                let parent_uri = path_to_uri(&parent_path)
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| format!("file://{}", parent_path.to_string_lossy()));
                lines.push(format!(
                    "**Parent:** [{}]({})",
                    parent_node.label, parent_uri
                ));
            } else {
                lines.push(format!("**Parent:** {}", parent_id));
            }
            lines.push(String::new());
        }

        // Body excerpt
        if let Some(doc) = crate::pkb::parse_file(abs_path) {
            let body_trimmed = doc.body.trim();
            if !body_trimmed.is_empty() {
                lines.push("---\n".to_string());
                let resolved_body = self.resolve_wikilinks_to_human_titles(body_trimmed);
                let compact_body = Self::format_body_micro_headers(&resolved_body);
                let body_preview: Vec<&str> = compact_body.lines().take(12).collect();
                lines.push(body_preview.join("\n"));
                if compact_body.lines().count() > 12 {
                    lines.push("\n_..._".to_string());
                }
                lines.push(String::new());
            }
        }

        // Action footer: command links running extension commands on this node
        lines.push("---\n".to_string());
        let json_arg =
            serde_json::to_string(&[&node.id]).unwrap_or_else(|_| format!("[\"{}\"]", node.id));
        let encoded_arg: String = url::form_urlencoded::byte_serialize(json_arg.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let open_link = format!(
            "[$(go-to-file) Open](command:mem.openReference?{})",
            encoded_arg
        );
        let subtree_link = format!(
            "[$(type-hierarchy-sub) Subtree](command:mem.showSubtree?{})",
            encoded_arg
        );
        let mut actions = vec![open_link, subtree_link];
        if is_task_node(node) {
            actions.push(format!(
                "[$(check) Complete](command:mem.completeTask?{})",
                encoded_arg
            ));
            actions.push(format!(
                "[$(close) Cancel](command:mem.cancelTask?{})",
                encoded_arg
            ));
            actions.push(format!(
                "[$(inbox) Queue](command:mem.queueTask?{})",
                encoded_arg
            ));
        }
        lines.push(actions.join("  ·  "));

        lines.join("\n")
    }

    /// Handle textDocument/hover request.
    pub fn handle_hover(&self, params: &HoverParams) -> Option<Hover> {
        self.ensure_fresh();
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let content = self.get_document_content(uri)?;

        let span = Self::find_reference_at_position(&content, pos.line, pos.character);
        let Some(span) = span else {
            debug!(
                uri = %uri.as_str(),
                line = pos.line,
                character = pos.character,
                "hover: no reference span at position"
            );
            return None;
        };

        match self.resolve_node(&span.target) {
            Some((node, abs_path)) => {
                debug!(
                    uri = %uri.as_str(),
                    line = pos.line,
                    character = pos.character,
                    target = %span.target,
                    node_id = %node.id,
                    path = %abs_path.display(),
                    "hover: resolved node"
                );
                let preview = self.format_hover_preview(&node, &abs_path);
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: preview,
                    }),
                    range: Some(span.range),
                })
            }
            None => {
                let node_count = self.graph.read().node_count();
                debug!(
                    uri = %uri.as_str(),
                    line = pos.line,
                    character = pos.character,
                    target = %span.target,
                    node_count = node_count,
                    "hover: failed to resolve reference"
                );
                None
            }
        }
    }

    /// Handle textDocument/definition request.
    pub fn handle_definition(
        &self,
        params: &GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        self.ensure_fresh();
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let content = self.get_document_content(uri)?;

        let span = Self::find_reference_at_position(&content, pos.line, pos.character);
        let Some(span) = span else {
            debug!(
                uri = %uri.as_str(),
                line = pos.line,
                character = pos.character,
                "definition: no reference span at position"
            );
            return None;
        };

        match self.resolve_node(&span.target) {
            Some((node, abs_path)) => {
                debug!(
                    uri = %uri.as_str(),
                    line = pos.line,
                    character = pos.character,
                    target = %span.target,
                    node_id = %node.id,
                    path = %abs_path.display(),
                    "definition: resolved node"
                );
                let target_uri = path_to_uri(&abs_path)?;
                Some(GotoDefinitionResponse::Scalar(Location {
                    uri: target_uri,
                    range: Range::default(),
                }))
            }
            None => {
                let node_count = self.graph.read().node_count();
                debug!(
                    uri = %uri.as_str(),
                    line = pos.line,
                    character = pos.character,
                    target = %span.target,
                    node_count = node_count,
                    "definition: failed to resolve reference"
                );
                None
            }
        }
    }

    /// Handle textDocument/documentLink request.
    pub fn handle_document_link(&self, params: &DocumentLinkParams) -> Vec<DocumentLink> {
        self.ensure_fresh();
        let uri = &params.text_document.uri;
        let content = match self.get_document_content(uri) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut links = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let mut occupied_ranges: Vec<(u32, u32)> = Vec::new();

            // 1. Find wikilinks
            for cap in WIKILINK_RE.captures_iter(line) {
                if let (Some(full), Some(target_match)) = (cap.get(0), cap.get(1)) {
                    let start = full.start() as u32;
                    let end = full.end() as u32;
                    let target = target_match.as_str().trim();
                    if let Some((node, abs_path)) = self.resolve_node(target) {
                        if let Some(target_uri) = path_to_uri(&abs_path) {
                            occupied_ranges.push((start, end));
                            links.push(DocumentLink {
                                range: Range {
                                    start: Position {
                                        line: line_idx as u32,
                                        character: start,
                                    },
                                    end: Position {
                                        line: line_idx as u32,
                                        character: end,
                                    },
                                },
                                target: Some(target_uri),
                                tooltip: Some(format!("Open {}: {}", node.id, node.label)),
                                data: None,
                            });
                        }
                    }
                }
            }

            // 2. Find standard markdown links [label](target) that resolve to PKB nodes
            for cap in MD_LINK_RE.captures_iter(line) {
                if let (Some(full), Some(target_match)) = (cap.get(0), cap.get(2)) {
                    let start = full.start() as u32;
                    let end = full.end() as u32;
                    if occupied_ranges.iter().any(|&(s, e)| start < e && end > s) {
                        continue;
                    }
                    let target = target_match.as_str().trim();
                    if !target.starts_with("http://") && !target.starts_with("https://") {
                        if let Some((node, abs_path)) = self.resolve_node(target) {
                            if let Some(target_uri) = path_to_uri(&abs_path) {
                                occupied_ranges.push((start, end));
                                links.push(DocumentLink {
                                    range: Range {
                                        start: Position {
                                            line: line_idx as u32,
                                            character: start,
                                        },
                                        end: Position {
                                            line: line_idx as u32,
                                            character: end,
                                        },
                                    },
                                    target: Some(target_uri),
                                    tooltip: Some(format!("Open {}: {}", node.id, node.label)),
                                    data: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        debug!(
            uri = %uri.as_str(),
            resolved_links = links.len(),
            "documentLink: resolved links"
        );
        links
    }

    pub fn node_weight_display(node: &GraphNode) -> Option<String> {
        if let Some(ref flow) = node.flow {
            let val = flow.gain.unwrap_or(0.0) + flow.loss_averted.unwrap_or(0.0);
            if val > 0.0 {
                let formatted = format!("{:.1}", val);
                return Some(formatted.trim_end_matches(".0").to_string());
            }
        }
        if let Some(worth) = node.worth {
            if worth.abs() > 0.0 {
                let formatted = format!("{:.1}", worth);
                return Some(formatted.trim_end_matches(".0").to_string());
            }
        }
        if let Some(sw) = node.standing_weight {
            if sw > 0.0 {
                let formatted = format!("{:.1}", sw);
                return Some(formatted.trim_end_matches(".0").to_string());
            }
        }
        if let Some(intent) = node.intent {
            return Some(intent.to_string());
        }
        None
    }

    pub fn node_weight_f64(node: &GraphNode) -> Option<f64> {
        if let Some(ref flow) = node.flow {
            let val = flow.gain.unwrap_or(0.0) + flow.loss_averted.unwrap_or(0.0);
            if val > 0.0 {
                return Some(val);
            }
        }
        if let Some(worth) = node.worth {
            if worth.abs() > 0.0 {
                return Some(worth);
            }
        }
        if let Some(sw) = node.standing_weight {
            if sw > 0.0 {
                return Some(sw);
            }
        }
        if let Some(intent) = node.intent {
            return Some(intent as f64);
        }
        None
    }

    /// Handle textDocument/inlayHint request.
    pub fn handle_inlay_hint(&self, params: &InlayHintParams) -> Vec<InlayHint> {
        self.ensure_fresh();
        let uri = &params.text_document.uri;
        let content = match self.get_document_content(uri) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut hints = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let line_u32 = line_idx as u32;

            for cap in WIKILINK_RE.captures_iter(line) {
                let full = match cap.get(0) {
                    Some(f) => f,
                    None => continue,
                };
                let target_match = match cap.get(1) {
                    Some(t) => t,
                    None => continue,
                };
                let target = target_match.as_str().trim();
                let end = full.end() as u32;

                if let Some((node, _abs_path)) = self.resolve_node(target) {
                    let weight_opt = Self::node_weight_display(&node);
                    let status_opt = node.status.as_deref();

                    let has_explicit_alias = cap.get(2).is_some();
                    let hint_label = if has_explicit_alias {
                        match (status_opt, weight_opt.as_deref()) {
                            (Some(st), Some(w)) => format!(": [{} · w:{}]", st, w),
                            (Some(st), None) => format!(": [{}]", st),
                            (None, Some(w)) => format!(": [w:{}]", w),
                            (None, None) => String::new(),
                        }
                    } else {
                        match (status_opt, weight_opt.as_deref()) {
                            (Some(st), Some(w)) => format!(": {} [{} · w:{}]", node.label, st, w),
                            (Some(st), None) => format!(": {} [{}]", node.label, st),
                            (None, Some(w)) => format!(": {} [w:{}]", node.label, w),
                            (None, None) => format!(": {}", node.label),
                        }
                    };

                    if hint_label.is_empty() {
                        continue;
                    }

                    hints.push(InlayHint {
                        position: Position {
                            line: line_u32,
                            character: end,
                        },
                        label: InlayHintLabel::String(hint_label),
                        kind: Some(InlayHintKind::TYPE),
                        text_edits: None,
                        tooltip: Some(InlayHintTooltip::String(format!(
                            "{}: {}",
                            node.id, node.label
                        ))),
                        padding_left: Some(true),
                        padding_right: None,
                        data: None,
                    });
                }
            }
        }

        hints
    }

    /// Legend for semantic tokens.
    pub fn semantic_tokens_legend() -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: vec![
                SemanticTokenType::new("pkbTaskReady"),
                SemanticTokenType::new("pkbTaskDone"),
                SemanticTokenType::new("pkbTaskBlocked"),
                SemanticTokenType::new("pkbTaskOther"),
                SemanticTokenType::new("pkbNodeInfo"),
                SemanticTokenType::new("pkbNodeIndex"),
                SemanticTokenType::new("pkbNodeChapter"),
                SemanticTokenType::new("pkbNodeDefault"),
            ],
            token_modifiers: vec![
                SemanticTokenModifier::new("ready"),
                SemanticTokenModifier::new("done"),
                SemanticTokenModifier::new("blocked"),
                SemanticTokenModifier::new("inProgress"),
                SemanticTokenModifier::new("task"),
                SemanticTokenModifier::new("info"),
                SemanticTokenModifier::new("index"),
                SemanticTokenModifier::new("chapter"),
            ],
        }
    }

    /// Handle textDocument/semanticTokens/full request.
    pub fn handle_semantic_tokens_full(
        &self,
        params: &SemanticTokensParams,
    ) -> Option<SemanticTokensResult> {
        self.ensure_fresh();
        let uri = &params.text_document.uri;
        let content = self.get_document_content(uri)?;

        let mut raw_tokens: Vec<(u32, u32, u32, u32, u32)> = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let line_u32 = line_idx as u32;

            for cap in WIKILINK_RE.captures_iter(line) {
                let full = match cap.get(0) {
                    Some(f) => f,
                    None => continue,
                };
                let target_match = match cap.get(1) {
                    Some(t) => t,
                    None => continue,
                };
                let target = target_match.as_str().trim();
                let start = full.start() as u32;
                let length = (full.end() - full.start()) as u32;

                if let Some((node, _abs_path)) = self.resolve_node(target) {
                    let ntype = node.node_type.as_deref().unwrap_or("task");
                    let status = node.status.as_deref();

                    let (token_type, token_modifiers_bitset) = match ntype {
                        "task" | "subtask" | "epic" | "project" => match status {
                            Some("ready") => (0, 1 | 16),
                            Some("done") => (1, 2 | 16),
                            Some("blocked") => (2, 4 | 16),
                            Some("in_progress") => (3, 8 | 16),
                            _ => (3, 16),
                        },
                        "info" | "knowledge" | "reference" => (4, 32),
                        "index" | "topic" => (5, 64),
                        "chapter" | "spec" => (6, 128),
                        _ => (7, 0),
                    };

                    raw_tokens.push((line_u32, start, length, token_type, token_modifiers_bitset));
                }
            }
        }

        let mut prev_line = 0;
        let mut prev_start = 0;
        let mut data = Vec::new();

        for (line, start, length, token_type, token_modifiers_bitset) in raw_tokens {
            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 {
                start - prev_start
            } else {
                start
            };
            prev_line = line;
            prev_start = start;

            data.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type,
                token_modifiers_bitset,
            });
        }

        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }

    /// Handle textDocument/codeLens request.
    pub fn handle_code_lens(&self, params: &CodeLensParams) -> Vec<CodeLens> {
        self.ensure_fresh();
        let uri = &params.text_document.uri;
        let content = match self.get_document_content(uri) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let uri_str = uri.as_str();
        let target_node = {
            let graph = self.graph.read();
            let mut found: Option<GraphNode> = None;
            if let Ok(url) = url::Url::parse(uri_str) {
                if let Ok(path) = url.to_file_path() {
                    found = graph.resolve(&path.to_string_lossy()).cloned();
                }
            }
            if found.is_none() {
                // Try finding by parsing frontmatter id: only between opening and closing `---` delimiters
                let mut in_frontmatter = false;
                for (i, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    if i == 0 && trimmed == "---" {
                        in_frontmatter = true;
                        continue;
                    }
                    if in_frontmatter {
                        if trimmed == "---" {
                            break;
                        }
                        if let Some(rest) = trimmed.strip_prefix("id:") {
                            let id_val = rest.trim().trim_matches('"').trim_matches('\'');
                            if !id_val.is_empty() {
                                found = graph.resolve(id_val).cloned();
                                break;
                            }
                        }
                    } else if !trimmed.is_empty() {
                        // Not frontmatter
                        break;
                    }
                }
            }
            if found.is_none() {
                if let Some(path_str) = uri_str.strip_prefix("file://") {
                    found = graph.resolve(path_str).cloned();
                }
            }
            found
        };

        let node = match target_node {
            Some(n) => n,
            None => {
                let title = content
                    .lines()
                    .find(|l| l.starts_with("# "))
                    .map(|l| l.trim_start_matches("# ").trim().to_string())
                    .unwrap_or_else(|| "Note".to_string());
                GraphNode {
                    id: String::new(),
                    label: title,
                    ..Default::default()
                }
            }
        };

        let graph = self.graph.read();
        let mut lenses = Vec::new();

        // 1. Parent lens
        let (parent_title, parent_cmd) = if let Some(ref pid) = node.parent {
            if let Some(pnode) = graph.resolve(pid) {
                (
                    format!("Parent: {}", pnode.label),
                    Some(Command {
                        title: format!("Open {}", pnode.label),
                        command: "mem.openReference".to_string(),
                        arguments: Some(vec![serde_json::Value::String(pnode.id.clone())]),
                    }),
                )
            } else {
                (format!("Parent: {}", pid), None)
            }
        } else {
            ("Parent: none".to_string(), None)
        };

        lenses.push(CodeLens {
            range: Range::default(),
            command: Some(Command {
                title: parent_title,
                command: parent_cmd
                    .as_ref()
                    .map(|c| c.command.clone())
                    .unwrap_or_default(),
                arguments: parent_cmd.and_then(|c| c.arguments),
            }),
            data: None,
        });

        // 2. Backlinks lens
        let backlink_text = if node.backlink_count == 1 {
            "1 backlink".to_string()
        } else {
            format!("{} backlinks", node.backlink_count)
        };
        lenses.push(CodeLens {
            range: Range::default(),
            command: Some(Command {
                title: backlink_text,
                command: String::new(),
                arguments: None,
            }),
            data: None,
        });

        // 3. Subtasks counts by status
        let mut subtask_ids = node.children.clone();
        for st in &node.subtasks {
            if !subtask_ids.contains(st) {
                subtask_ids.push(st.clone());
            }
        }

        let mut status_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for sid in &subtask_ids {
            if let Some(snode) = graph.resolve(sid) {
                let st = snode.status.as_deref().unwrap_or("unknown");
                *status_counts.entry(st.to_string()).or_insert(0) += 1;
            }
        }

        let subtasks_text = if status_counts.is_empty() {
            "Subtasks: 0".to_string()
        } else {
            let order = ["ready", "in_progress", "blocked", "done"];
            let mut parts = Vec::new();
            for &key in &order {
                if let Some(&cnt) = status_counts.get(key) {
                    parts.push(format!("{} {}", cnt, key));
                }
            }
            for (k, cnt) in &status_counts {
                if !order.contains(&k.as_str()) {
                    parts.push(format!("{} {}", cnt, k));
                }
            }
            format!("Subtasks: {}", parts.join(" · "))
        };

        lenses.push(CodeLens {
            range: Range::default(),
            command: Some(Command {
                title: subtasks_text,
                command: String::new(),
                arguments: None,
            }),
            data: None,
        });

        // 4. Dispatch action
        lenses.push(CodeLens {
            range: Range::default(),
            command: Some(Command {
                title: "Dispatch".to_string(),
                command: "mem.dispatch".to_string(),
                arguments: Some(vec![serde_json::Value::String(node.id.clone())]),
            }),
            data: None,
        });

        lenses
    }

    /// Handle gutter badges calculation for a document.
    pub fn handle_gutter_badges(&self, uri: &Uri) -> Vec<GutterBadgeItem> {
        self.ensure_fresh();
        let content = match self.get_document_content(uri) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut badges = Vec::new();

        for (line_idx, line) in content.lines().enumerate() {
            let line_u32 = line_idx as u32;

            let mut found_task: Option<GutterBadgeItem> = None;

            for cap in WIKILINK_RE.captures_iter(line) {
                let target_match = match cap.get(1) {
                    Some(t) => t,
                    None => continue,
                };
                let target = target_match.as_str().trim();
                if let Some((node, _abs_path)) = self.resolve_node(target) {
                    let ntype = node.node_type.as_deref().unwrap_or("task");
                    if ntype == "task"
                        || ntype == "subtask"
                        || ntype == "epic"
                        || ntype == "project"
                        || node.id.starts_with("task-")
                    {
                        let weight = Self::node_weight_f64(&node);
                        let status = node.status.clone();
                        let badge = status.clone().unwrap_or_else(|| "task".to_string());

                        found_task = Some(GutterBadgeItem {
                            line: line_u32,
                            task_id: node.id.clone(),
                            title: node.label.clone(),
                            status,
                            weight,
                            badge,
                        });
                        break;
                    }
                }
            }

            if let Some(item) = found_task {
                badges.push(item);
            }
        }

        badges
    }

    /// Run the LSP server over an arbitrary lsp-server Connection (memory or stdio).
    pub fn run_connection(mut self, connection: lsp_server::Connection) -> anyhow::Result<()> {
        let server_capabilities = serde_json::to_value(&ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            document_link_provider: Some(DocumentLinkOptions {
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            }),
            inlay_hint_provider: Some(OneOf::Left(true)),
            semantic_tokens_provider: Some(
                SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: Self::semantic_tokens_legend(),
                    range: Some(false),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                }),
            ),
            code_lens_provider: Some(CodeLensOptions {
                resolve_provider: Some(false),
            }),
            ..Default::default()
        })?;

        let initialization_params = connection.initialize(server_capabilities)?;
        #[allow(deprecated)]
        match serde_json::from_value::<InitializeParams>(initialization_params) {
            Ok(params) => {
                info!(
                    client_info = ?params.client_info,
                    root_uri = ?params.root_uri,
                    workspace_folders = ?params.workspace_folders,
                    "LSP client initialized"
                );
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "Failed to deserialize InitializeParams"
                );
            }
        }

        for msg in &connection.receiver {
            match msg {
                lsp_server::Message::Request(req) => {
                    if connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    match req.method.as_str() {
                        "textDocument/hover" => {
                            match serde_json::from_value::<HoverParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_hover(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid hover params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid hover params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "textDocument/definition" => {
                            match serde_json::from_value::<GotoDefinitionParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_definition(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid definition params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid definition params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "textDocument/documentLink" => {
                            match serde_json::from_value::<DocumentLinkParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_document_link(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid documentLink params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid documentLink params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "textDocument/inlayHint" => {
                            match serde_json::from_value::<InlayHintParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_inlay_hint(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid inlayHint params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid inlayHint params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "textDocument/semanticTokens/full" => {
                            match serde_json::from_value::<SemanticTokensParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_semantic_tokens_full(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid semanticTokens params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid semanticTokens params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "textDocument/codeLens" => {
                            match serde_json::from_value::<CodeLensParams>(req.params) {
                                Ok(params) => {
                                    let result = self.handle_code_lens(&params);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid codeLens params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid codeLens params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "mem/status" => {
                            self.ensure_fresh();
                            let node_count = self.graph.read().node_count();
                            let status = serde_json::json!({
                                "pkbRoot": self.pkb_root,
                                "rootExists": self.pkb_root.exists(),
                                "nodeCount": node_count,
                                "openDocuments": self.documents.len(),
                                "version": env!("CARGO_PKG_VERSION"),
                            });
                            let resp = lsp_server::Response::new_ok(req.id, status);
                            let _ = connection.sender.send(lsp_server::Message::Response(resp));
                        }
                        "mem/resolveReference" => {
                            #[derive(serde::Deserialize)]
                            struct ResolveReferenceParams {
                                target: String,
                            }
                            match serde_json::from_value::<ResolveReferenceParams>(req.params) {
                                Ok(params) => {
                                    self.ensure_fresh();
                                    let result = if let Some((node, abs_path)) =
                                        self.resolve_node(&params.target)
                                    {
                                        let target_uri =
                                            path_to_uri(&abs_path).map(|u| u.as_str().to_string());
                                        let preview = self.format_hover_preview(&node, &abs_path);
                                        serde_json::json!({
                                            "id": node.id,
                                            "label": node.label,
                                            "type": node.node_type,
                                            "status": node.status,
                                            "intent": node.intent,
                                            "tags": node.tags,
                                            "parent": node.parent,
                                            "path": abs_path,
                                            "uri": target_uri,
                                            "previewMarkdown": preview,
                                        })
                                    } else {
                                        serde_json::Value::Null
                                    };
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid resolveReference params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid resolveReference params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        "mem/gutterBadges" => {
                            #[derive(serde::Deserialize)]
                            #[serde(rename_all = "camelCase")]
                            struct GutterBadgesParams {
                                text_document: TextDocumentIdentifier,
                            }
                            match serde_json::from_value::<GutterBadgesParams>(req.params) {
                                Ok(params) => {
                                    let result =
                                        self.handle_gutter_badges(&params.text_document.uri);
                                    let resp = lsp_server::Response::new_ok(req.id, result);
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                                Err(err) => {
                                    warn!(
                                        method = %req.method,
                                        id = ?req.id,
                                        error = %err,
                                        "Invalid gutterBadges params"
                                    );
                                    let resp = lsp_server::Response::new_err(
                                        req.id,
                                        lsp_server::ErrorCode::InvalidParams as i32,
                                        format!("Invalid gutterBadges params: {}", err),
                                    );
                                    let _ =
                                        connection.sender.send(lsp_server::Message::Response(resp));
                                }
                            }
                        }
                        _ => {
                            let resp =
                                lsp_server::Response::new_ok(req.id, serde_json::Value::Null);
                            let _ = connection.sender.send(lsp_server::Message::Response(resp));
                        }
                    }
                }
                lsp_server::Message::Notification(not) => match not.method.as_str() {
                    "textDocument/didOpen" => {
                        match serde_json::from_value::<DidOpenTextDocumentParams>(not.params) {
                            Ok(params) => self.handle_did_open(params),
                            Err(err) => warn!(
                                method = %not.method,
                                error = %err,
                                "Malformed textDocument/didOpen notification"
                            ),
                        }
                    }
                    "textDocument/didChange" => {
                        match serde_json::from_value::<DidChangeTextDocumentParams>(not.params) {
                            Ok(params) => self.handle_did_change(params),
                            Err(err) => warn!(
                                method = %not.method,
                                error = %err,
                                "Malformed textDocument/didChange notification"
                            ),
                        }
                    }
                    "textDocument/didClose" => {
                        match serde_json::from_value::<DidCloseTextDocumentParams>(not.params) {
                            Ok(params) => self.handle_did_close(params),
                            Err(err) => warn!(
                                method = %not.method,
                                error = %err,
                                "Malformed textDocument/didClose notification"
                            ),
                        }
                    }
                    _ => {}
                },
                lsp_server::Message::Response(_) => {}
            }
        }

        Ok(())
    }

    /// Run the LSP server over standard I/O.
    pub fn run_stdio(self) -> anyhow::Result<()> {
        let (connection, io_threads) = lsp_server::Connection::stdio();
        self.run_connection(connection)?;
        io_threads.join()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_test_node(id: &str, label: &str, node_type: Option<&str>, status: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: label.to_string(),
            node_type: node_type.map(|s| s.to_string()),
            status: status.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_is_task_node() {
        let task_node = make_test_node("task-123", "Task 123", Some("task"), Some("ready"));
        assert!(is_task_node(&task_node));

        let subtask_node = make_test_node("sub-123", "Subtask 123", Some("subtask"), None);
        assert!(is_task_node(&subtask_node));

        let prefixed_node = make_test_node("task-xyz", "Prefixed", None, None);
        assert!(is_task_node(&prefixed_node));

        let note_node = make_test_node("concept-abc", "Concept", Some("note"), None);
        assert!(!is_task_node(&note_node));
    }

    #[test]
    fn test_format_hover_preview_task_actions() {
        let dir = tempdir().unwrap();
        let pkb_root = dir.path().to_path_buf();
        let server = LspServer::new(pkb_root.clone());

        let task_node = make_test_node("task-foo", "Foo Task", Some("task"), Some("ready"));
        let fake_path = pkb_root.join("tasks").join("task-foo.md");
        let preview = server.format_hover_preview(&task_node, &fake_path);

        assert!(preview.contains("command:mem.openReference?%5B%22task-foo%22%5D"));
        assert!(preview.contains("command:mem.showSubtree?%5B%22task-foo%22%5D"));
        assert!(preview.contains("[$(check) Complete](command:mem.completeTask?%5B%22task-foo%22%5D)"));
        assert!(preview.contains("[$(close) Cancel](command:mem.cancelTask?%5B%22task-foo%22%5D)"));
        assert!(preview.contains("[$(inbox) Queue](command:mem.queueTask?%5B%22task-foo%22%5D)"));
    }

    #[test]
    fn test_format_hover_preview_non_task_omits_task_actions() {
        let dir = tempdir().unwrap();
        let pkb_root = dir.path().to_path_buf();
        let server = LspServer::new(pkb_root.clone());

        let note_node = make_test_node("concept-bar", "Bar Concept", Some("note"), None);
        let fake_path = pkb_root.join("notes").join("concept-bar.md");
        let preview = server.format_hover_preview(&note_node, &fake_path);

        assert!(preview.contains("command:mem.openReference?%5B%22concept-bar%22%5D"));
        assert!(preview.contains("command:mem.showSubtree?%5B%22concept-bar%22%5D"));
        assert!(!preview.contains("command:mem.completeTask?"));
        assert!(!preview.contains("command:mem.cancelTask?"));
        assert!(!preview.contains("command:mem.queueTask?"));
    }
}
