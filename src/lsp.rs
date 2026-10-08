//! PKB Language Server Protocol (LSP) implementation.
//!
//! Provides hover preview, definition resolution, and document links
//! for PKB references (`[[id]]`, `[[id|alias]]`, `[label](target)`, etc.) in markdown files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use lsp_types::*;
use parking_lot::RwLock;
use regex::Regex;

use crate::graph::GraphNode;
use crate::graph_store::GraphStore;

static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]+))?\]\]").unwrap());

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

pub struct LspServer {
    pub pkb_root: PathBuf,
    pub graph: Arc<RwLock<GraphStore>>,
    pub documents: HashMap<Uri, String>,
}

impl LspServer {
    pub fn new(pkb_root: PathBuf) -> Self {
        let store = GraphStore::build_from_directory(&pkb_root);
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
            let mut store = self.graph.write();
            *store = GraphStore::build_from_directory(&self.pkb_root);
        }
    }

    pub fn handle_did_open(&mut self, params: DidOpenTextDocumentParams) {
        self.documents.insert(params.text_document.uri, params.text_document.text);
    }

    pub fn handle_did_change(&mut self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents.insert(params.text_document.uri, change.text);
        }
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) {
        self.documents.remove(&params.text_document.uri);
    }

    /// Get document content either from memory or read from disk.
    pub fn get_document_content(&self, uri: &Uri) -> Option<String> {
        if let Some(text) = self.documents.get(uri) {
            return Some(text.clone());
        }

        // Try reading from file path if it's a file URI
        let uri_str = uri.as_str();
        if let Some(path_str) = uri_str.strip_prefix("file://") {
            if let Ok(content) = fs::read_to_string(path_str) {
                return Some(content);
            }
        }
        None
    }

    /// Find a reference at the given line and character position.
    pub fn find_reference_at_position(text: &str, line_idx: u32, char_idx: u32) -> Option<ReferenceSpan> {
        let lines: Vec<&str> = text.lines().collect();
        let line = lines.get(line_idx as usize)?;

        // 1. Search for wikilinks: [[target]] or [[target|alias]]
        let wiki_re = Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]+))?\]\]").ok()?;
        for cap in wiki_re.captures_iter(line) {
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
                        start: Position { line: line_idx, character: start },
                        end: Position { line: line_idx, character: end },
                    },
                });
            }
        }

        // 2. Search for standard markdown links: [label](target)
        let md_re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").ok()?;
        for cap in md_re.captures_iter(line) {
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
                            start: Position { line: line_idx, character: start },
                            end: Position { line: line_idx, character: end },
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
                    start: Position { line: line_idx, character: start as u32 },
                    end: Position { line: line_idx, character: (end + 1) as u32 },
                },
            });
        }

        None
    }

    /// Resolve a reference string to a canonical GraphNode and absolute file path.
    pub fn resolve_node(&self, target: &str) -> Option<(GraphNode, PathBuf)> {
        self.ensure_fresh();
        let graph = self.graph.read();
        let node = graph.resolve(target)?.clone();
        let abs_path = if node.path.is_absolute() {
            node.path.clone()
        } else {
            self.pkb_root.join(&node.path)
        };
        Some((node, abs_path))
    }

    /// Format a Markdown preview for a node.
    pub fn format_hover_preview(&self, node: &GraphNode, abs_path: &Path) -> String {
        let mut lines = Vec::new();

        // Title and ID header
        lines.push(format!("### {} (`{}`)\n", node.label, node.id));

        // Metadata badges
        let mut meta = Vec::new();
        if let Some(ref t) = node.node_type {
            meta.push(format!("**Type:** `{}`", t));
        }
        if let Some(ref s) = node.status {
            meta.push(format!("**Status:** `{}`", s));
        }
        if let Some(i) = node.intent {
            meta.push(format!("**Intent:** `{}`", i));
        }
        if !meta.is_empty() {
            lines.push(meta.join("  |  "));
            lines.push(String::new());
        }

        if !node.tags.is_empty() {
            lines.push(format!("**Tags:** {}", node.tags.iter().map(|t| format!("`#{}`", t)).collect::<Vec<_>>().join(" ")));
            lines.push(String::new());
        }

        if let Some(ref parent) = node.parent {
            lines.push(format!("**Parent:** `[[{}]]`", parent));
            lines.push(String::new());
        }

        // Body excerpt
        if let Some(doc) = crate::pkb::parse_file(abs_path) {
            let body_trimmed = doc.body.trim();
            if !body_trimmed.is_empty() {
                lines.push("---\n".to_string());
                let body_preview: Vec<&str> = body_trimmed
                    .lines()
                    .take(12)
                    .collect();
                lines.push(body_preview.join("\n"));
                if body_trimmed.lines().count() > 12 {
                    lines.push("\n_..._".to_string());
                }
                lines.push(String::new());
            }
        }

        // Action links
        let path_str = abs_path.to_string_lossy();
        lines.push("---\n".to_string());
        lines.push(format!("[Open in editor tab](file://{})", path_str));

        lines.join("\n")
    }

    /// Handle textDocument/hover request.
    pub fn handle_hover(&self, params: &HoverParams) -> Option<Hover> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let content = self.get_document_content(uri)?;

        let span = Self::find_reference_at_position(&content, pos.line, pos.character)?;
        let (node, abs_path) = self.resolve_node(&span.target)?;
        let preview = self.format_hover_preview(&node, &abs_path);

        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: preview,
            }),
            range: Some(span.range),
        })
    }

    /// Handle textDocument/definition request.
    pub fn handle_definition(&self, params: &GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let content = self.get_document_content(uri)?;

        let span = Self::find_reference_at_position(&content, pos.line, pos.character)?;
        let (_node, abs_path) = self.resolve_node(&span.target)?;

        let target_uri = Uri::from_str(&format!("file://{}", abs_path.to_string_lossy())).ok()?;
        Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range::default(),
        }))
    }

    /// Handle textDocument/documentLink request.
    pub fn handle_document_link(&self, params: &DocumentLinkParams) -> Vec<DocumentLink> {
        let uri = &params.text_document.uri;
        let content = match self.get_document_content(uri) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut links = Vec::new();
        
        for (line_idx, line) in content.lines().enumerate() {
            // Find wikilinks
            for cap in WIKILINK_RE.captures_iter(line) {
                if let (Some(full), Some(target_match)) = (cap.get(0), cap.get(1)) {
                    let target = target_match.as_str().trim();
                    if let Some((node, abs_path)) = self.resolve_node(target) {
                        if let Ok(target_uri) = Uri::from_str(&format!("file://{}", abs_path.to_string_lossy())) {
                            links.push(DocumentLink {
                                range: Range {
                                    start: Position { line: line_idx as u32, character: full.start() as u32 },
                                    end: Position { line: line_idx as u32, character: full.end() as u32 },
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

        links
    }

    /// Run the LSP server over an arbitrary lsp-server Connection (memory or stdio).
    pub fn run_connection(mut self, connection: lsp_server::Connection) -> anyhow::Result<()> {
        let server_capabilities = serde_json::to_value(&ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::FULL,
            )),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            document_link_provider: Some(DocumentLinkOptions {
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            }),
            ..Default::default()
        })?;

        let initialization_params = connection.initialize(server_capabilities)?;
        let _params: InitializeParams = serde_json::from_value(initialization_params)?;

        for msg in &connection.receiver {
            match msg {
                lsp_server::Message::Request(req) => {
                    if connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    match req.method.as_str() {
                        "textDocument/hover" => {
                            let params: HoverParams = serde_json::from_value(req.params)?;
                            let result = self.handle_hover(&params);
                            let resp = lsp_server::Response::new_ok(req.id, result);
                            connection.sender.send(lsp_server::Message::Response(resp))?;
                        }
                        "textDocument/definition" => {
                            let params: GotoDefinitionParams = serde_json::from_value(req.params)?;
                            let result = self.handle_definition(&params);
                            let resp = lsp_server::Response::new_ok(req.id, result);
                            connection.sender.send(lsp_server::Message::Response(resp))?;
                        }
                        "textDocument/documentLink" => {
                            let params: DocumentLinkParams = serde_json::from_value(req.params)?;
                            let result = self.handle_document_link(&params);
                            let resp = lsp_server::Response::new_ok(req.id, result);
                            connection.sender.send(lsp_server::Message::Response(resp))?;
                        }
                        _ => {
                            let resp = lsp_server::Response::new_ok(req.id, serde_json::Value::Null);
                            connection.sender.send(lsp_server::Message::Response(resp))?;
                        }
                    }
                }
                lsp_server::Message::Notification(not) => {
                    match not.method.as_str() {
                        "textDocument/didOpen" => {
                            if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params) {
                                self.handle_did_open(params);
                            }
                        }
                        "textDocument/didChange" => {
                            if let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(not.params) {
                                self.handle_did_change(params);
                            }
                        }
                        "textDocument/didClose" => {
                            if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(not.params) {
                                self.handle_did_close(params);
                            }
                        }
                        _ => {}
                    }
                }
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
