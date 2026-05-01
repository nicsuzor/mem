use crate::graph_store::GraphStore;
use std::collections::HashSet;

/// A node reference with label, type, and status for rendering.
#[derive(Debug, Clone)]
pub struct ContextNode {
    pub id: String,
    pub label: String,
    pub node_type: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
}

/// Structured local graph context for a node, usable by both CLI and dashboard views.
#[derive(Debug)]
pub struct LocalContext {
    pub target: ContextNode,
    /// Parent chain from immediate parent up to root.
    pub parents: Vec<ContextNode>,
    pub siblings: Vec<ContextNode>,
    pub children: Vec<ContextNode>,
    pub children_total: usize,
    /// Dependencies (what this node depends on).
    pub depends_on: Vec<ContextNode>,
    /// What completing this node would unblock.
    pub blocks: Vec<ContextNode>,
    /// Strategic contribution (what this node contributes to).
    pub contributes_to: Vec<ContextNode>,
    /// What nodes contribute to this one.
    pub contributed_by: Vec<ContextNode>,
    /// Semantically similar nodes (automatically discovered).
    pub similar_to: Vec<ContextNode>,
    pub is_orphan: bool,
}

/// Extract structured local context for a node. Returns `None` if the node doesn't exist.
pub fn get_local_context(gs: &GraphStore, node_id: &str) -> Option<LocalContext> {
    let node = gs.get_node(node_id)?;

    let target = ContextNode {
        id: node_id.to_string(),
        label: node.label.clone(),
        node_type: node.node_type.clone(),
        status: node.status.clone(),
        priority: node.priority,
    };

    // Walk parent chain (with cycle detection)
    let mut parents = Vec::new();
    let mut parent_id = node.parent.as_deref();
    let mut visited = std::collections::HashSet::new();
    visited.insert(node_id.to_string());
    while let Some(pid) = parent_id {
        if !visited.insert(pid.to_string()) {
            break; // cycle detected
        }
        if let Some(parent) = gs.get_node(pid) {
            parents.push(ContextNode {
                id: pid.to_string(),
                label: parent.label.clone(),
                node_type: parent.node_type.clone(),
                status: parent.status.clone(),
                priority: parent.priority,
            });
            parent_id = parent.parent.as_deref();
        } else {
            break;
        }
    }

    // Siblings (up to 5)
    let siblings = if let Some(ref pid) = node.parent {
        if let Some(parent) = gs.get_node(pid) {
            parent.children.iter()
                .filter(|id| id.as_str() != node_id)
                .filter_map(|id| gs.get_node(id))
                .take(5)
                .map(|n| ContextNode {
                    id: n.id.clone(),
                    label: n.label.clone(),
                    node_type: n.node_type.clone(),
                    status: n.status.clone(),
                    priority: n.priority,
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let children_total = node.children.len();
    let children: Vec<_> = node.children.iter()
        .filter_map(|id| gs.get_node(id))
        .take(5)
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    let depends_on: Vec<_> = node.depends_on.iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    let blocks: Vec<_> = node.blocks.iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    let contributes_to: Vec<_> = node.contributes_to.iter()
        .filter_map(|ct| ct.resolved_to.as_ref())
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    let contributed_by: Vec<_> = node.contributed_by.iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    let similar_to: Vec<_> = gs.get_outgoing_edges(node_id).iter()
        .filter(|e| matches!(e.edge_type, crate::graph::EdgeType::SimilarTo))
        .filter_map(|e| gs.get_node(&e.target))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            priority: n.priority,
        })
        .collect();

    Some(LocalContext {
        target,
        is_orphan: parents.is_empty(),
        parents,
        siblings,
        children,
        children_total,
        depends_on,
        blocks,
        contributes_to,
        contributed_by,
        similar_to,
    })
}

/// Format a status tag with ANSI color.
fn status_tag(status: Option<&str>) -> String {
    match status {
        Some("done" | "complete" | "completed") => "\x1b[32mdone\x1b[0m".to_string(),
        Some("active") => "\x1b[33mactive\x1b[0m".to_string(),
        Some("blocked") => "\x1b[31mblocked\x1b[0m".to_string(),
        Some(s) => format!("\x1b[2m{s}\x1b[0m"),
        None => String::new(),
    }
}

/// Format a node for display: "Label  [status]"
fn fmt_node(n: &ContextNode) -> String {
    let st = status_tag(n.status.as_deref());
    if st.is_empty() {
        n.label.clone()
    } else {
        format!("{}  {}", n.label, st)
    }
}

/// Renders a vertical tree view of a node's local neighbourhood.
///
/// ```text
///   Graph Context:
///     ╭─ parent ─╮
///     │  Grandparent
///     │  └─ Parent
///     │     ├─ Sibling A
///     │     ├─ ★ THIS NODE  ← you are here
///     │     └─ Sibling B
///     ╰──────────╯
///     depends on:
///       ← Dependency A  [active]
///     blocks:
///       → Blocked Task  [blocked]
///     children:
///       ├─ Child A  [active]
///       └─ Child B  [done]
/// ```
pub fn render_ascii_graph(gs: &GraphStore, node_id: &str) -> Vec<String> {
    let ctx = match get_local_context(gs, node_id) {
        Some(c) => c,
        None => return vec![format!("Node not found: {}", node_id)],
    };

    let mut lines = Vec::new();

    // --- Parent chain (reversed so root is first) ---
    if !ctx.parents.is_empty() {
        let mut chain: Vec<&ContextNode> = ctx.parents.iter().collect();
        chain.reverse(); // root first

        for (i, p) in chain.iter().enumerate() {
            let indent = "  ".repeat(i);
            let connector = if i == 0 { "" } else { "└─ " };
            lines.push(format!("    \x1b[2m{indent}{connector}{}\x1b[0m", p.label));
        }

        // Now show siblings + target as children of the immediate parent
        let depth = chain.len();
        let indent = "  ".repeat(depth);
        let total_siblings = ctx.siblings.len() + 1; // +1 for the target
        let mut all_nodes: Vec<(bool, &ContextNode)> = Vec::new();
        // We don't know the exact ordering, so put target first then siblings
        all_nodes.push((true, &ctx.target));
        for s in &ctx.siblings {
            all_nodes.push((false, s));
        }

        for (i, (is_target, node)) in all_nodes.iter().enumerate() {
            let is_last = i == total_siblings - 1;
            let branch = if is_last { "└─" } else { "├─" };
            if *is_target {
                lines.push(format!(
                    "    {indent}{branch} \x1b[1;36m★ {}\x1b[0m",
                    ctx.target.label
                ));
            } else {
                lines.push(format!(
                    "    {indent}{branch} \x1b[2m{}\x1b[0m",
                    fmt_node(node)
                ));
            }
        }
    } else {
        // Orphan — just show the target
        lines.push(format!("    \x1b[1;36m★ {}\x1b[0m", ctx.target.label));
    }

    // --- Dependencies ---
    if !ctx.depends_on.is_empty() {
        lines.push(format!("    \x1b[2mdepends on:\x1b[0m"));
        for dep in &ctx.depends_on {
            lines.push(format!("      \x1b[33m← {}\x1b[0m", fmt_node(dep)));
        }
    }

    // --- Blocks ---
    if !ctx.blocks.is_empty() {
        lines.push(format!("    \x1b[2mblocks:\x1b[0m"));
        for blocked in &ctx.blocks {
            lines.push(format!("      \x1b[36m→ {}\x1b[0m", fmt_node(blocked)));
        }
    }

    // --- Contributions ---
    if !ctx.contributes_to.is_empty() {
        lines.push(format!("    \x1b[2mcontributes to:\x1b[0m"));
        for target in &ctx.contributes_to {
            lines.push(format!("      \x1b[1;34m↗ {}\x1b[0m", fmt_node(target)));
        }
    }

    if !ctx.contributed_by.is_empty() {
        lines.push(format!("    \x1b[2mcontributed by:\x1b[0m"));
        for source in &ctx.contributed_by {
            lines.push(format!("      \x1b[34m↙ {}\x1b[0m", fmt_node(source)));
        }
    }

    // --- Similar to ---
    if !ctx.similar_to.is_empty() {
        lines.push(format!("    \x1b[2msimilar to:\x1b[0m"));
        for sim in &ctx.similar_to {
            lines.push(format!("      \x1b[2m≈ {}\x1b[0m", fmt_node(sim)));
        }
    }

    // --- Children ---
    if !ctx.children.is_empty() {
        lines.push(format!("    \x1b[2mchildren:\x1b[0m"));
        for (i, child) in ctx.children.iter().enumerate() {
            let is_last = i == ctx.children.len() - 1 && ctx.children_total <= ctx.children.len();
            let branch = if is_last { "└─" } else { "├─" };
            lines.push(format!("      {branch} {}", fmt_node(child)));
        }
        if ctx.children_total > ctx.children.len() {
            lines.push(format!(
                "      └─ \x1b[2m... ({} more)\x1b[0m",
                ctx.children_total - ctx.children.len()
            ));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Dependency-neighbourhood renderer
// ---------------------------------------------------------------------------

/// Options for [`render_neighbourhood`].
#[derive(Debug, Clone)]
pub struct NeighbourhoodOpts {
    /// Maximum recursion depth for upstream blockers. Set to 0 to suppress.
    pub upstream_depth: usize,
    /// Maximum recursion depth for downstream dependents. Set to 0 to suppress.
    pub downstream_depth: usize,
    /// Include `soft_depends_on` / `soft_blocks` edges.
    pub include_soft: bool,
    /// Include parent-child edges in the downstream tree (useful for epics).
    pub include_children: bool,
    /// Strip ANSI codes for clean LLM-consumable output.
    pub plain: bool,
}

impl Default for NeighbourhoodOpts {
    fn default() -> Self {
        Self {
            upstream_depth: 2,
            downstream_depth: 2,
            include_soft: true,
            include_children: true,
            plain: false,
        }
    }
}

/// How a graph edge is being traversed in the neighbourhood tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Hard `depends_on` (upstream) or `blocks` (downstream).
    Hard,
    /// Soft `soft_depends_on` / `soft_blocks`.
    Soft,
    /// Parent-child (only used in downstream when `include_children`).
    Child,
}

impl Edge {
    /// Tree connector segment for this edge type ("──" for hard, "┄┄" for soft, "──" for child).
    fn dash(self) -> &'static str {
        match self {
            Edge::Hard | Edge::Child => "\u{2500}\u{2500}",
            Edge::Soft => "\u{2504}\u{2504}",
        }
    }

    /// Short tag shown after the label, e.g. " (soft)". Empty for hard edges.
    fn tag(self) -> &'static str {
        match self {
            Edge::Hard => "",
            Edge::Soft => " (soft)",
            Edge::Child => " (child)",
        }
    }
}

fn col(plain: bool, code: &str) -> &str {
    if plain { "" } else { code }
}

/// Render a status tag, honouring the plain (no-ANSI) flag.
fn status_label(status: Option<&str>, plain: bool) -> String {
    let s = match status {
        Some(s) if !s.is_empty() => s,
        _ => return String::new(),
    };
    if plain {
        return format!("[{s}]");
    }
    let colour = match s {
        "done" | "complete" | "completed" => "\x1b[32m",
        "active" | "in_progress" | "ready" => "\x1b[33m",
        "blocked" | "waiting" => "\x1b[31m",
        _ => "\x1b[2m",
    };
    format!("{colour}[{s}]\x1b[0m")
}

fn fmt_node_inline(n: &crate::graph::GraphNode, plain: bool) -> String {
    let tid = n.task_id.as_deref().unwrap_or(&n.id);
    let status = status_label(n.status.as_deref(), plain);
    let id_dim = if plain {
        format!("[{tid}]")
    } else {
        format!("\x1b[2;37m[{tid}]\x1b[0m")
    };
    if status.is_empty() {
        format!("{}  {}", n.label, id_dim)
    } else {
        format!("{}  {}  {}", n.label, status, id_dim)
    }
}

/// Render a single tree-line for a child node with the given prefix and connector.
fn line_for_child(
    n: &crate::graph::GraphNode,
    prefix: &str,
    is_last: bool,
    edge: Edge,
    plain: bool,
) -> String {
    let connector = if is_last { "\u{2514}" } else { "\u{251C}" };
    let dim_open = col(plain, "\x1b[2m");
    let dim_close = col(plain, "\x1b[0m");
    let tag = edge.tag();
    let tag_str = if tag.is_empty() {
        String::new()
    } else {
        format!("{dim_open}{tag}{dim_close}")
    };
    format!(
        "{prefix}{connector}{dash} {label}{tag_str}",
        dash = edge.dash(),
        label = fmt_node_inline(n, plain),
    )
}

/// Recursively walk in either direction, emitting tree-formatted lines.
#[allow(clippy::too_many_arguments)]
fn walk(
    gs: &GraphStore,
    out: &mut Vec<String>,
    visited: &mut HashSet<String>,
    node_id: &str,
    direction: Direction,
    prefix: String,
    depth: usize,
    max_depth: usize,
    opts: &NeighbourhoodOpts,
) {
    if depth >= max_depth {
        return;
    }
    let node = match gs.get_node(node_id) {
        Some(n) => n,
        None => return,
    };

    // Collect outgoing edges in (id, edge) form.
    let mut edges: Vec<(&String, Edge)> = Vec::new();
    match direction {
        Direction::Upstream => {
            for d in &node.depends_on {
                edges.push((d, Edge::Hard));
            }
            if opts.include_soft {
                for d in &node.soft_depends_on {
                    edges.push((d, Edge::Soft));
                }
            }
        }
        Direction::Downstream => {
            for b in &node.blocks {
                edges.push((b, Edge::Hard));
            }
            if opts.include_soft {
                for b in &node.soft_blocks {
                    edges.push((b, Edge::Soft));
                }
            }
            if opts.include_children {
                for c in &node.children {
                    edges.push((c, Edge::Child));
                }
            }
        }
    }

    // De-dup while preserving order.
    let mut seen_local: HashSet<&String> = HashSet::new();
    edges.retain(|(id, _)| seen_local.insert(*id));

    let total = edges.len();
    for (i, (next_id, edge)) in edges.iter().enumerate() {
        let is_last = i == total - 1;
        let next_node = match gs.get_node(next_id) {
            Some(n) => n,
            None => continue,
        };
        let already = !visited.insert((*next_id).clone());
        let mut line = line_for_child(next_node, &prefix, is_last, *edge, opts.plain);
        if already {
            let dim_open = col(opts.plain, "\x1b[2m");
            let dim_close = col(opts.plain, "\x1b[0m");
            line.push_str(&format!("{dim_open} (cycle){dim_close}"));
        }
        out.push(line);

        if !already {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}\u{2502}   ")
            };
            walk(
                gs,
                out,
                visited,
                next_id,
                direction,
                child_prefix,
                depth + 1,
                max_depth,
                opts,
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Upstream,
    Downstream,
}

/// Render the dependency neighbourhood of a node as a single ASCII tree:
/// upstream blockers above, the highlighted target node in the middle, and
/// downstream dependents (plus children for epics) below. A breadcrumb of
/// the parent chain is shown at the top.
///
/// Returns one line per output row. Returns a single error line if the node
/// is not found.
pub fn render_neighbourhood(
    gs: &GraphStore,
    node_id: &str,
    opts: &NeighbourhoodOpts,
) -> Vec<String> {
    let node = match gs.get_node(node_id) {
        Some(n) => n,
        None => return vec![format!("Node not found: {}", node_id)],
    };

    let mut out: Vec<String> = Vec::new();
    let bold_open = col(opts.plain, "\x1b[1m");
    let bold_close = col(opts.plain, "\x1b[0m");
    let dim_open = col(opts.plain, "\x1b[2m");
    let dim_close = col(opts.plain, "\x1b[0m");
    let target_open = col(opts.plain, "\x1b[1;36m");
    let target_close = col(opts.plain, "\x1b[0m");

    // ── Breadcrumb header ──
    let mut chain: Vec<String> = Vec::new();
    let mut visited_parents: HashSet<String> = HashSet::new();
    visited_parents.insert(node.id.clone());
    let mut cursor = node.parent.clone();
    while let Some(pid) = cursor {
        if !visited_parents.insert(pid.clone()) {
            break;
        }
        match gs.get_node(&pid) {
            Some(p) => {
                chain.push(p.label.clone());
                cursor = p.parent.clone();
            }
            None => break,
        }
    }
    if !chain.is_empty() {
        chain.reverse();
        out.push(format!(
            "{dim_open}{}{dim_close}",
            chain.join(" \u{203A} ")
        ));
        out.push(String::new());
    }

    // ── Upstream tree (recursive depends_on / soft_depends_on) ──
    let has_upstream = !node.depends_on.is_empty()
        || (opts.include_soft && !node.soft_depends_on.is_empty());
    if has_upstream && opts.upstream_depth > 0 {
        out.push(format!("{bold_open}Upstream (blocks this):{bold_close}"));
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(node.id.clone());
        walk(
            gs,
            &mut out,
            &mut visited,
            &node.id,
            Direction::Upstream,
            String::new(),
            0,
            opts.upstream_depth,
            opts,
        );
        out.push(String::new());
    }

    // ── Target node ──
    let star = "\u{2605}"; // ★
    out.push(format!(
        "{target_open}{star} {label}{target_close}  {status}  {id_dim}[{tid}]{id_close}",
        label = node.label,
        status = status_label(node.status.as_deref(), opts.plain),
        id_dim = col(opts.plain, "\x1b[2;37m"),
        id_close = col(opts.plain, "\x1b[0m"),
        tid = node.task_id.as_deref().unwrap_or(&node.id),
    ));

    // ── Downstream tree (recursive blocks / soft_blocks / children) ──
    let has_downstream = !node.blocks.is_empty()
        || (opts.include_soft && !node.soft_blocks.is_empty())
        || (opts.include_children && !node.children.is_empty());
    if has_downstream && opts.downstream_depth > 0 {
        out.push(String::new());
        out.push(format!("{bold_open}Downstream (this blocks):{bold_close}"));
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(node.id.clone());
        walk(
            gs,
            &mut out,
            &mut visited,
            &node.id,
            Direction::Downstream,
            String::new(),
            0,
            opts.downstream_depth,
            opts,
        );
    }

    // ── Lonely-node hint ──
    if !has_upstream && !has_downstream {
        out.push(String::new());
        out.push(format!(
            "{dim_open}(no dependency relationships){dim_close}"
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_store::GraphStore;
    use crate::pkb::PkbDocument;
    use std::path::{Path, PathBuf};

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
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            modified: None,
            content_hash: String::new(),
            file_hash: String::new(),
        }
    }

    fn build_graph() -> GraphStore {
        let docs = vec![
            make_doc("tasks/epic-1.md", "Epic One", "epic", "active", "epic-1", None, &[]),
            make_doc("tasks/task-a.md", "Task A", "task", "active", "task-a", Some("epic-1"), &["task-b"]),
            make_doc("tasks/task-b.md", "Task B", "task", "active", "task-b", Some("epic-1"), &[]),
            make_doc("tasks/task-c.md", "Task C", "task", "active", "task-c", None, &["task-a"]),
            make_doc("tasks/isolated.md", "Isolated", "task", "active", "isolated", None, &[]),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    fn build_graph_many_children() -> GraphStore {
        let docs = vec![
            make_doc("tasks/parent.md", "Big Parent", "epic", "active", "big-parent", None, &[]),
            make_doc("tasks/c1.md", "Child 1", "task", "active", "child-1", Some("big-parent"), &[]),
            make_doc("tasks/c2.md", "Child 2", "task", "active", "child-2", Some("big-parent"), &[]),
            make_doc("tasks/c3.md", "Child 3", "task", "active", "child-3", Some("big-parent"), &[]),
            make_doc("tasks/c4.md", "Child 4", "task", "active", "child-4", Some("big-parent"), &[]),
            make_doc("tasks/c5.md", "Child 5", "task", "active", "child-5", Some("big-parent"), &[]),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    #[test]
    fn test_node_not_found() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "nonexistent");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Node not found"));
    }

    #[test]
    fn test_node_with_parent_children_deps_and_blocks() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "task-a");
        let combined = lines.join("\n");

        // Should show parent
        assert!(combined.contains("Epic One"), "Expected parent in output:\n{}", combined);
        // Target highlighted with star
        assert!(combined.contains("★ Task A"), "Expected ★ Task A in output:\n{}", combined);
        // Dependency
        assert!(combined.contains("← Task B"), "Expected ← Task B dep in output:\n{}", combined);
        // Blocks
        assert!(combined.contains("→ Task C"), "Expected → Task C blocks in output:\n{}", combined);
        // Sibling
        assert!(combined.contains("Task B"), "Expected sibling Task B in output:\n{}", combined);
    }

    #[test]
    fn test_orphan_node_no_parent() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "task-c");
        let combined = lines.join("\n");

        assert!(combined.contains("★ Task C"), "Expected ★ Task C in output:\n{}", combined);
        assert!(combined.contains("← Task A"), "Expected dep in output:\n{}", combined);
    }

    #[test]
    fn test_isolated_node_no_relationships() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "isolated");
        let combined = lines.join("\n");

        assert_eq!(lines.len(), 1, "Expected exactly 1 line for isolated node, got:\n{}", combined);
        assert!(combined.contains("★ Isolated"), "Expected ★ Isolated in output:\n{}", combined);
    }

    #[test]
    fn test_children_truncation() {
        let gs = build_graph_many_children();
        let lines = render_ascii_graph(&gs, "big-parent");
        let combined = lines.join("\n");

        // Should show children section
        assert!(combined.contains("children:"), "Expected children section:\n{}", combined);
        // Should show truncation
        // 5 children, showing up to 5 now
        let child_lines: Vec<_> = lines.iter().filter(|l| l.contains("Child")).collect();
        assert!(child_lines.len() == 5, "Expected 5 child lines, got {}:\n{}", child_lines.len(), combined);
    }

    // -----------------------------------------------------------------------
    // render_neighbourhood tests
    // -----------------------------------------------------------------------

    /// Helper: doc with arbitrary frontmatter fields.
    fn make_doc_full(
        path: &str,
        title: &str,
        doc_type: &str,
        status: &str,
        id: &str,
        parent: Option<&str>,
        depends_on: &[&str],
        soft_depends_on: &[&str],
        blocks: &[&str],
        soft_blocks: &[&str],
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
        if !soft_depends_on.is_empty() {
            fm.insert("soft_depends_on".to_string(), serde_json::json!(soft_depends_on));
        }
        if !blocks.is_empty() {
            fm.insert("blocks".to_string(), serde_json::json!(blocks));
        }
        if !soft_blocks.is_empty() {
            fm.insert("soft_blocks".to_string(), serde_json::json!(soft_blocks));
        }
        PkbDocument {
            path: PathBuf::from(path),
            title: title.to_string(),
            body: String::new(),
            doc_type: Some(doc_type.to_string()),
            status: Some(status.to_string()),
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            modified: None,
            content_hash: String::new(),
            file_hash: String::new(),
        }
    }

    /// Build a multi-level graph:
    ///   project › epic › task-mid
    ///     upstream:   task-mid depends_on task-up1 (hard) and task-soft (soft)
    ///                 task-up1 depends_on task-up2 (hard, transitive)
    ///     downstream: task-mid blocks task-down1; task-down1 blocks task-down2
    fn build_chain() -> GraphStore {
        let docs = vec![
            make_doc_full("tasks/proj.md", "Project", "project", "active", "proj-1", None, &[], &[], &[], &[]),
            make_doc_full("tasks/epic.md", "Epic One", "epic", "active", "epic-1", Some("proj-1"), &[], &[], &[], &[]),
            make_doc_full("tasks/up2.md", "Up Two", "task", "done", "task-up2", None, &[], &[], &[], &[]),
            make_doc_full("tasks/up1.md", "Up One", "task", "active", "task-up1", None, &["task-up2"], &[], &[], &[]),
            make_doc_full("tasks/soft.md", "Soft Dep", "task", "active", "task-soft", None, &[], &[], &[], &[]),
            make_doc_full(
                "tasks/mid.md", "Mid Task", "task", "in_progress", "task-mid",
                Some("epic-1"), &["task-up1"], &["task-soft"], &["task-down1"], &[],
            ),
            make_doc_full("tasks/down1.md", "Down One", "task", "blocked", "task-down1", None, &[], &[], &["task-down2"], &[]),
            make_doc_full("tasks/down2.md", "Down Two", "task", "active", "task-down2", None, &[], &[], &[], &[]),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    #[test]
    fn neighbourhood_node_not_found() {
        let gs = build_chain();
        let lines = render_neighbourhood(&gs, "no-such-id", &NeighbourhoodOpts::default());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Node not found"));
    }

    #[test]
    fn neighbourhood_renders_both_directions() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts { plain: true, ..Default::default() };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Breadcrumb
        assert!(combined.contains("Project \u{203A} Epic One"), "missing breadcrumb:\n{combined}");
        // Section headers present and ordered
        let up_pos = combined.find("Upstream").expect("upstream header");
        let down_pos = combined.find("Downstream").expect("downstream header");
        assert!(up_pos < down_pos, "upstream must precede downstream:\n{combined}");
        // Target highlighted
        assert!(combined.contains("\u{2605} Mid Task"), "missing star+target:\n{combined}");
        // Direct upstream + transitive upstream both visible (default depth 2)
        assert!(combined.contains("Up One"), "missing direct upstream:\n{combined}");
        assert!(combined.contains("Up Two"), "missing transitive upstream:\n{combined}");
        // Direct + transitive downstream
        assert!(combined.contains("Down One"), "missing direct downstream:\n{combined}");
        assert!(combined.contains("Down Two"), "missing transitive downstream:\n{combined}");
        // Status tags inline (plain mode → bracketed)
        assert!(combined.contains("[in_progress]"), "missing target status:\n{combined}");
        assert!(combined.contains("[blocked]"), "missing downstream status:\n{combined}");
    }

    #[test]
    fn neighbourhood_distinguishes_soft_edges() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts { plain: true, ..Default::default() };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Soft dep visible by default
        assert!(combined.contains("Soft Dep"), "expected soft dep included:\n{combined}");
        // Soft tag present (label or dashed connector)
        assert!(
            combined.contains("(soft)") || combined.contains("\u{2504}\u{2504}"),
            "expected soft marker:\n{combined}"
        );

        // --no-soft → no soft dep listed
        let opts2 = NeighbourhoodOpts { include_soft: false, plain: true, ..Default::default() };
        let combined2 = render_neighbourhood(&gs, "task-mid", &opts2).join("\n");
        assert!(!combined2.contains("Soft Dep"), "soft dep should be hidden:\n{combined2}");
    }

    #[test]
    fn neighbourhood_depth_limits_recursion() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts {
            upstream_depth: 1, downstream_depth: 1, plain: true, ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Depth 1: direct deps only
        assert!(combined.contains("Up One"), "depth-1 must include direct upstream:\n{combined}");
        assert!(!combined.contains("Up Two"), "depth-1 must exclude transitive:\n{combined}");
        assert!(combined.contains("Down One"), "depth-1 must include direct downstream:\n{combined}");
        assert!(!combined.contains("Down Two"), "depth-1 must exclude transitive:\n{combined}");
    }

    #[test]
    fn neighbourhood_upstream_zero_hides_upstream_section() {
        let gs = build_chain();
        // pkb blocks semantics: only downstream
        let opts = NeighbourhoodOpts {
            upstream_depth: 0, downstream_depth: 3, plain: true, ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");
        assert!(!combined.contains("Upstream"), "upstream section must be hidden:\n{combined}");
        assert!(combined.contains("Downstream"), "downstream section expected:\n{combined}");
        assert!(combined.contains("Down Two"), "transitive downstream expected:\n{combined}");
    }

    #[test]
    fn neighbourhood_plain_mode_strips_ansi() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts { plain: true, ..Default::default() };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");
        assert!(!combined.contains("\x1b["), "plain mode must not emit ANSI:\n{combined}");
    }

    #[test]
    fn neighbourhood_works_for_epic_with_children() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts { plain: true, ..Default::default() };
        let combined = render_neighbourhood(&gs, "epic-1", &opts).join("\n");

        // Epic should show child task in its downstream tree
        assert!(combined.contains("\u{2605} Epic One"), "missing target epic:\n{combined}");
        assert!(combined.contains("Mid Task"), "epic should show child:\n{combined}");
        assert!(combined.contains("(child)") || combined.contains("Downstream"),
            "expected child section:\n{combined}");
    }

    #[test]
    fn neighbourhood_isolated_node() {
        let docs = vec![
            make_doc_full("tasks/lone.md", "Lonely", "task", "active", "task-lone", None, &[], &[], &[], &[]),
        ];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let opts = NeighbourhoodOpts { plain: true, ..Default::default() };
        let combined = render_neighbourhood(&gs, "task-lone", &opts).join("\n");
        assert!(combined.contains("\u{2605} Lonely"));
        assert!(combined.contains("no dependency relationships"));
    }
}
