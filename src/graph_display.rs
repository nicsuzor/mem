use crate::graph::GraphNode;
use crate::graph_store::GraphStore;
use std::collections::HashSet;

/// A node reference with label, type, and status for rendering.
#[derive(Debug, Clone)]
pub struct ContextNode {
    pub id: String,
    pub label: String,
    pub node_type: Option<String>,
    pub status: Option<String>,
    pub intent: Option<i32>,
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
        intent: node.intent,
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
                intent: parent.intent,
            });
            parent_id = parent.parent.as_deref();
        } else {
            break;
        }
    }

    // Siblings (up to 5)
    let siblings = if let Some(ref pid) = node.parent {
        if let Some(parent) = gs.get_node(pid) {
            parent
                .children
                .iter()
                .filter(|id| id.as_str() != node_id)
                .filter_map(|id| gs.get_node(id))
                .take(5)
                .map(|n| ContextNode {
                    id: n.id.clone(),
                    label: n.label.clone(),
                    node_type: n.node_type.clone(),
                    status: n.status.clone(),
                    intent: n.intent,
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let children_total = node.children.len();
    let children: Vec<_> = node
        .children
        .iter()
        .filter_map(|id| gs.get_node(id))
        .take(5)
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
        })
        .collect();

    let depends_on: Vec<_> = node
        .depends_on
        .iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
        })
        .collect();

    let blocks: Vec<_> = node
        .blocks
        .iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
        })
        .collect();

    let contributes_to: Vec<_> = node
        .contributes_to
        .iter()
        .filter_map(|ct| ct.resolved_to.as_ref())
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
        })
        .collect();

    let contributed_by: Vec<_> = node
        .contributed_by
        .iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
        })
        .collect();

    let similar_to: Vec<_> = gs
        .get_outgoing_edges(node_id)
        .iter()
        .filter(|e| matches!(e.edge_type, crate::graph::EdgeType::SimilarTo))
        .filter_map(|e| gs.get_node(&e.target))
        .map(|n| ContextNode {
            id: n.id.clone(),
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
            intent: n.intent,
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
        Some("in_progress") => "\x1b[33min_progress\x1b[0m".to_string(),
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
    if plain {
        ""
    } else {
        code
    }
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
        "in_progress" | "ready" | "queued" => "\x1b[33m",
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

/// Human-readable name for the relation an edge belongs to, given the
/// direction it was traversed in. Used to say *which* relation a `(cycle)`
/// marker was found in, since `blocks`/`soft_blocks`/`children` (downstream)
/// and `depends_on`/`soft_depends_on` (upstream) are walked as one merged
/// tree for display but a cycle is only meaningful within a single relation.
fn relation_name(direction: Direction, edge: Edge) -> &'static str {
    match (direction, edge) {
        (Direction::Upstream, Edge::Hard) => "depends_on",
        (Direction::Upstream, Edge::Soft) => "soft_depends_on",
        (Direction::Downstream, Edge::Hard) => "blocks",
        (Direction::Downstream, Edge::Soft) => "soft_blocks",
        (_, Edge::Child) => "children",
    }
}

/// Recursively walk in either direction, emitting tree-formatted lines.
///
/// `path_ids` holds only the ancestors of the node currently being
/// expanded — entries are removed on backtrack — so re-visiting an
/// ancestor (rather than a node merely seen before) is what stops
/// recursion. `path_edges` is the same ancestor chain paired with the edge
/// relation used to reach each one, in order from the walk's root down to
/// the current node; it lets a revisit be checked for whether every edge
/// from the ancestor down to here — including the edge that closes the
/// loop — belongs to the *same* relation. Only that case is a true cycle
/// and gets marked `(cycle: <relation>)`. A revisit that mixes relations
/// (e.g. reached once as a child, once as a dependency) or that reaches a
/// node via two independent branches (a diamond, not an ancestor at all)
/// is not a cycle in any single relation; recursion is still cut short to
/// avoid unbounded output, tracked via the permanent `rendered` set, but no
/// marker is printed.
#[allow(clippy::too_many_arguments)]
fn walk(
    gs: &GraphStore,
    out: &mut Vec<String>,
    path_ids: &mut HashSet<String>,
    path_edges: &mut Vec<(String, Edge)>,
    rendered: &mut HashSet<String>,
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
        let is_ancestor = path_ids.contains(next_id.as_str());
        let is_cycle = is_ancestor && {
            let suffix = match path_edges.iter().position(|(id, _)| id == next_id.as_str()) {
                Some(idx) => &path_edges[idx + 1..],
                None => &path_edges[..], // next_id is the walk's own root
            };
            suffix.iter().all(|(_, e)| e == edge)
        };
        let already_rendered = !is_ancestor && !rendered.insert((*next_id).clone());
        let mut line = line_for_child(next_node, &prefix, is_last, *edge, opts.plain);
        if is_cycle {
            let dim_open = col(opts.plain, "\x1b[2m");
            let dim_close = col(opts.plain, "\x1b[0m");
            let rel = relation_name(direction, *edge);
            line.push_str(&format!("{dim_open} (cycle: {rel}){dim_close}"));
        }
        out.push(line);

        if !is_ancestor && !already_rendered {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}\u{2502}   ")
            };
            path_ids.insert((*next_id).clone());
            path_edges.push(((*next_id).clone(), *edge));
            walk(
                gs,
                out,
                path_ids,
                path_edges,
                rendered,
                next_id,
                direction,
                child_prefix,
                depth + 1,
                max_depth,
                opts,
            );
            path_edges.pop();
            path_ids.remove(next_id.as_str());
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
        out.push(format!("{dim_open}{}{dim_close}", chain.join(" \u{203A} ")));
        out.push(String::new());
    }

    // ── Upstream tree (recursive depends_on / soft_depends_on) ──
    let has_upstream =
        !node.depends_on.is_empty() || (opts.include_soft && !node.soft_depends_on.is_empty());
    if has_upstream && opts.upstream_depth > 0 {
        out.push(format!("{bold_open}Upstream (blocks this):{bold_close}"));
        let mut path_ids: HashSet<String> = HashSet::new();
        path_ids.insert(node.id.clone());
        let mut path_edges: Vec<(String, Edge)> = Vec::new();
        let mut rendered: HashSet<String> = HashSet::new();
        rendered.insert(node.id.clone());
        walk(
            gs,
            &mut out,
            &mut path_ids,
            &mut path_edges,
            &mut rendered,
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
        let mut path_ids: HashSet<String> = HashSet::new();
        path_ids.insert(node.id.clone());
        let mut path_edges: Vec<(String, Edge)> = Vec::new();
        let mut rendered: HashSet<String> = HashSet::new();
        rendered.insert(node.id.clone());
        walk(
            gs,
            &mut out,
            &mut path_ids,
            &mut path_edges,
            &mut rendered,
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

// ── Nested Task Tree Display & Brief JSON ─────────────────────────────────────

/// Brief metadata for a node in a nested task tree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct NestedTaskNode {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_intent: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downstream_weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_context: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    pub children: Vec<NestedTaskNode>,
}

fn strip_ansi(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    RE.replace_all(s, "").to_string()
}

pub fn days_since_created(created: Option<&str>) -> Option<i64> {
    let created = created?;
    if created.len() < 10 {
        return None;
    }
    let created_dt = chrono::NaiveDate::parse_from_str(&created[..10], "%Y-%m-%d").ok()?;
    let today = chrono::Utc::now().date_naive();
    Some((today - created_dt).num_days())
}

pub fn format_context_line(node: &GraphNode, child_task_count: usize, plain: bool) -> String {
    let ntype = node.node_type.as_deref().unwrap_or("group");
    let tid = node.task_id.as_deref().unwrap_or(&node.id);

    let count_str = if child_task_count > 0 {
        if plain {
            format!(" ({child_task_count})")
        } else {
            format!(" \x1b[2m({child_task_count})\x1b[0m")
        }
    } else {
        String::new()
    };

    if plain {
        format!("▌ {}{count_str}  [{tid}]", node.label)
    } else {
        let block_color = match ntype {
            "epic" => "\x1b[36m",
            "goal" => "\x1b[33m",
            "project" => "\x1b[1;36m",
            _ => "\x1b[2m",
        };
        format!(
            "{block_color}▌\x1b[0m \x1b[1m{}\x1b[0m{count_str}  \x1b[2;37m[{tid}]\x1b[0m",
            node.label,
        )
    }
}

pub fn format_task_line(task: &GraphNode, width: usize, plain: bool) -> String {
    let pri = task.intent.unwrap_or(4);
    let exposure = if task.stakeholder_exposure { "!" } else { " " };

    let left = if plain {
        format!("P{pri}{exposure} {}", task.label)
    } else {
        let color = match pri {
            0 => "\x1b[1;31m",
            1 => "\x1b[31m",
            2 => "\x1b[33m",
            _ => "\x1b[34m",
        };
        format!("{color}P{pri}{exposure}\x1b[0m {}", task.label)
    };

    let mut right_parts: Vec<String> = Vec::new();

    if task.downstream_weight > 0.0 {
        let wt = format!("wt:{:.1}", task.downstream_weight);
        if plain {
            right_parts.push(wt);
        } else {
            right_parts.push(format!("\x1b[2m{wt}\x1b[0m"));
        }
    }
    if let Some(ref cx) = task.complexity {
        if plain {
            right_parts.push(format!("[{cx}]"));
        } else {
            right_parts.push(format!("\x1b[2m[{cx}]\x1b[0m"));
        }
    }
    if let Some(ref due) = task.due {
        if plain {
            let len = std::cmp::min(10, due.len());
            let due_str = &due[..due.floor_char_boundary(len)];
            right_parts.push(format!("due:{due_str}"));
        } else {
            let today = chrono::Utc::now().date_naive();
            let len = std::cmp::min(10, due.len());
            let due_substr = &due[..due.floor_char_boundary(len)];
            let color = if let Ok(due_date) = chrono::NaiveDate::parse_from_str(due_substr, "%Y-%m-%d") {
                let days_until = (due_date - today).num_days();
                if days_until < 0 {
                    "\x1b[31m"
                } else if days_until <= 7 {
                    "\x1b[33m"
                } else {
                    "\x1b[2m"
                }
            } else {
                "\x1b[2m"
            };
            right_parts.push(format!("{color}due:{due_substr}\x1b[0m"));
        }
    }
    if let Some(days) = days_since_created(task.created.as_deref()) {
        if plain {
            right_parts.push(format!("{days}d"));
        } else {
            let color = if days > 30 {
                "\x1b[31m"
            } else if days >= 14 {
                "\x1b[33m"
            } else {
                "\x1b[2m"
            };
            right_parts.push(format!("{color}{days}d\x1b[0m"));
        }
    }
    let tid = task.task_id.as_deref().unwrap_or(&task.id);
    if plain {
        right_parts.push(format!("[{tid}]"));
    } else {
        right_parts.push(format!("\x1b[2;37m[{tid}]\x1b[0m"));
    }

    let right = right_parts.join("  ");

    let left_len = if plain { left.chars().count() } else { strip_ansi(&left).chars().count() };
    let right_len = if plain { right.chars().count() } else { strip_ansi(&right).chars().count() };
    let padding = width
        .saturating_sub(left_len)
        .saturating_sub(right_len)
        .max(2);

    format!("{left}{:>pad$}{right}", "", pad = padding)
}

pub fn sort_siblings(nodes: &mut [&GraphNode], context_ids: &HashSet<String>) {
    nodes.sort_by(|a, b| {
        let a_ctx = context_ids.contains(&a.id);
        let b_ctx = context_ids.contains(&b.id);
        match (a_ctx, b_ctx) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => a.label.cmp(&b.label),
            (false, false) => GraphStore::focus_cmp(a, b),
        }
    });
}

pub fn count_visible_tasks(
    gs: &GraphStore,
    node_id: &str,
    visible: &HashSet<&str>,
    context_ids: &HashSet<String>,
) -> usize {
    let mut count = 0;
    if let Some(node) = gs.get_node(node_id) {
        for cid in &node.children {
            if !visible.contains(cid.as_str()) {
                continue;
            }
            if context_ids.contains(cid) {
                count += count_visible_tasks(gs, cid, visible, context_ids);
            } else {
                count += 1;
            }
        }
    }
    count
}

pub fn collect_tree_roots<'a>(
    gs: &'a GraphStore,
    tasks: &[&'a GraphNode],
) -> (Vec<&'a GraphNode>, HashSet<&'a str>, HashSet<String>) {
    let mut visible: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let context_types = ["project", "epic", "goal"];
    let mut context_ids: HashSet<String> = HashSet::new();

    for task in tasks {
        let mut current_id = task.parent.as_deref();
        let mut visited_ancestors = HashSet::new();
        visited_ancestors.insert(task.id.as_str());

        while let Some(pid) = current_id {
            if !visited_ancestors.insert(pid) {
                break;
            }
            if visible.contains(pid) || context_ids.contains(pid) {
                break;
            }
            if let Some(parent_node) = gs.get_node(pid) {
                if parent_node
                    .node_type
                    .as_deref()
                    .map(|t| context_types.contains(&t))
                    .unwrap_or(false)
                {
                    context_ids.insert(pid.to_string());
                }
                current_id = parent_node.parent.as_deref();
            } else {
                break;
            }
        }
    }

    for cid in &context_ids {
        visible.insert(cid.as_str());
    }

    let mut roots: Vec<&GraphNode> = visible
        .iter()
        .filter_map(|id| gs.get_node(id))
        .filter(|n| match &n.parent {
            None => true,
            Some(pid) => !visible.contains(pid.as_str()),
        })
        .collect();

    sort_siblings(&mut roots, &context_ids);

    (roots, visible, context_ids)
}

fn render_tree_ascii_node(
    gs: &GraphStore,
    node: &GraphNode,
    visible: &HashSet<&str>,
    context_ids: &HashSet<String>,
    prefix: &str,
    is_last: bool,
    output: &mut Vec<String>,
    width: usize,
    plain: bool,
    ancestor_path: &mut HashSet<String>,
) {
    if !ancestor_path.insert(node.id.clone()) {
        return;
    }

    let connector = if is_last {
        "└── "
    } else {
        "├── "
    };
    let prefix_vis = if plain {
        prefix.chars().count() + 4
    } else {
        strip_ansi(prefix).chars().count() + 4
    };
    let available = width.saturating_sub(prefix_vis);

    let is_context = context_ids.contains(&node.id);
    let line = if is_context {
        let task_count = count_visible_tasks(gs, &node.id, visible, context_ids);
        format_context_line(node, task_count, plain)
    } else {
        format_task_line(node, available, plain)
    };
    output.push(format!("{prefix}{connector}{line}"));

    let mut children: Vec<&GraphNode> = node
        .children
        .iter()
        .filter(|cid| visible.contains(cid.as_str()))
        .filter_map(|cid| gs.get_node(cid))
        .collect();
    sort_siblings(&mut children, context_ids);

    let child_prefix = if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    let mut prev_was_context = false;
    for (i, child) in children.iter().enumerate() {
        let child_is_last = i == children.len() - 1;
        let child_is_context = context_ids.contains(&child.id);

        if child_is_context && prev_was_context && i > 0 {
            output.push(child_prefix.clone());
        }

        render_tree_ascii_node(
            gs,
            child,
            visible,
            context_ids,
            &child_prefix,
            child_is_last,
            output,
            width,
            plain,
            ancestor_path,
        );
        prev_was_context = child_is_context;
    }

    ancestor_path.remove(&node.id);
}

pub fn render_nested_task_ascii_tree(
    gs: &GraphStore,
    tasks: &[&GraphNode],
    width: usize,
    plain: bool,
) -> Vec<String> {
    let (roots, visible, context_ids) = collect_tree_roots(gs, tasks);
    let mut lines = Vec::new();
    let mut ancestor_path = HashSet::new();

    for (i, root) in roots.iter().enumerate() {
        let is_last = i == roots.len() - 1;
        render_tree_ascii_node(
            gs,
            root,
            &visible,
            &context_ids,
            "",
            is_last,
            &mut lines,
            width,
            plain,
            &mut ancestor_path,
        );
    }
    lines
}

fn build_nested_json_node(
    gs: &GraphStore,
    node: &GraphNode,
    visible: &HashSet<&str>,
    context_ids: &HashSet<String>,
    ancestor_path: &mut HashSet<String>,
) -> NestedTaskNode {
    let tid = node.task_id.as_deref().unwrap_or(&node.id).to_string();
    let is_ctx = context_ids.contains(&node.id);

    if !ancestor_path.insert(node.id.clone()) {
        return NestedTaskNode {
            id: tid,
            title: node.label.clone(),
            node_type: node.node_type.clone(),
            status: node.status.clone(),
            intent: node.intent,
            effective_intent: node.effective_intent,
            downstream_weight: if node.downstream_weight > 0.0 {
                Some(node.downstream_weight)
            } else {
                None
            },
            complexity: node.complexity.clone(),
            due: node.due.clone(),
            assignee: node.assignee.clone(),
            project: node.project.clone(),
            is_context: if is_ctx { Some(true) } else { None },
            blocked: if is_ctx { None } else { Some(gs.is_blocked(&node.id)) },
            children: Vec::new(),
        };
    }

    let mut children_nodes: Vec<&GraphNode> = node
        .children
        .iter()
        .filter(|cid| visible.contains(cid.as_str()))
        .filter_map(|cid| gs.get_node(cid))
        .collect();
    sort_siblings(&mut children_nodes, context_ids);

    let children = children_nodes
        .into_iter()
        .map(|child| build_nested_json_node(gs, child, visible, context_ids, ancestor_path))
        .collect();

    ancestor_path.remove(&node.id);

    NestedTaskNode {
        id: tid,
        title: node.label.clone(),
        node_type: node.node_type.clone(),
        status: node.status.clone(),
        intent: node.intent,
        effective_intent: node.effective_intent,
        downstream_weight: if node.downstream_weight > 0.0 {
            Some(node.downstream_weight)
        } else {
            None
        },
        complexity: node.complexity.clone(),
        due: node.due.clone(),
        assignee: node.assignee.clone(),
        project: node.project.clone(),
        is_context: if is_ctx { Some(true) } else { None },
        blocked: if is_ctx { None } else { Some(gs.is_blocked(&node.id)) },
        children,
    }
}

pub fn build_nested_task_json(
    gs: &GraphStore,
    tasks: &[&GraphNode],
) -> Vec<NestedTaskNode> {
    let (roots, visible, context_ids) = collect_tree_roots(gs, tasks);
    let mut ancestor_path = HashSet::new();
    roots
        .into_iter()
        .map(|root| build_nested_json_node(gs, root, &visible, &context_ids, &mut ancestor_path))
        .collect()
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
            consolidated: None,
            consolidated_at: None,
            tags: vec![],
            frontmatter: Some(serde_json::Value::Object(fm)),
            modified: None,
            content_hash: String::new(),
            file_hash: String::new(),
        }
    }

    fn build_graph() -> GraphStore {
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
                "active",
                "task-a",
                Some("epic-1"),
                &["task-b"],
            ),
            make_doc(
                "tasks/task-b.md",
                "Task B",
                "task",
                "active",
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
            make_doc(
                "tasks/isolated.md",
                "Isolated",
                "task",
                "active",
                "isolated",
                None,
                &[],
            ),
        ];
        GraphStore::build(&docs, Path::new("/tmp/test-pkb"))
    }

    fn build_graph_many_children() -> GraphStore {
        let docs = vec![
            make_doc(
                "tasks/parent.md",
                "Big Parent",
                "epic",
                "active",
                "big-parent",
                None,
                &[],
            ),
            make_doc(
                "tasks/c1.md",
                "Child 1",
                "task",
                "active",
                "child-1",
                Some("big-parent"),
                &[],
            ),
            make_doc(
                "tasks/c2.md",
                "Child 2",
                "task",
                "active",
                "child-2",
                Some("big-parent"),
                &[],
            ),
            make_doc(
                "tasks/c3.md",
                "Child 3",
                "task",
                "active",
                "child-3",
                Some("big-parent"),
                &[],
            ),
            make_doc(
                "tasks/c4.md",
                "Child 4",
                "task",
                "active",
                "child-4",
                Some("big-parent"),
                &[],
            ),
            make_doc(
                "tasks/c5.md",
                "Child 5",
                "task",
                "active",
                "child-5",
                Some("big-parent"),
                &[],
            ),
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
        assert!(
            combined.contains("Epic One"),
            "Expected parent in output:\n{}",
            combined
        );
        // Target highlighted with star
        assert!(
            combined.contains("★ Task A"),
            "Expected ★ Task A in output:\n{}",
            combined
        );
        // Dependency
        assert!(
            combined.contains("← Task B"),
            "Expected ← Task B dep in output:\n{}",
            combined
        );
        // Blocks
        assert!(
            combined.contains("→ Task C"),
            "Expected → Task C blocks in output:\n{}",
            combined
        );
        // Sibling
        assert!(
            combined.contains("Task B"),
            "Expected sibling Task B in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_orphan_node_no_parent() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "task-c");
        let combined = lines.join("\n");

        assert!(
            combined.contains("★ Task C"),
            "Expected ★ Task C in output:\n{}",
            combined
        );
        assert!(
            combined.contains("← Task A"),
            "Expected dep in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_isolated_node_no_relationships() {
        let gs = build_graph();
        let lines = render_ascii_graph(&gs, "isolated");
        let combined = lines.join("\n");

        assert_eq!(
            lines.len(),
            1,
            "Expected exactly 1 line for isolated node, got:\n{}",
            combined
        );
        assert!(
            combined.contains("★ Isolated"),
            "Expected ★ Isolated in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_children_truncation() {
        let gs = build_graph_many_children();
        let lines = render_ascii_graph(&gs, "big-parent");
        let combined = lines.join("\n");

        // Should show children section
        assert!(
            combined.contains("children:"),
            "Expected children section:\n{}",
            combined
        );
        // Should show truncation
        // 5 children, showing up to 5 now
        let child_lines: Vec<_> = lines.iter().filter(|l| l.contains("Child")).collect();
        assert!(
            child_lines.len() == 5,
            "Expected 5 child lines, got {}:\n{}",
            child_lines.len(),
            combined
        );
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
            fm.insert(
                "soft_depends_on".to_string(),
                serde_json::json!(soft_depends_on),
            );
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
            consolidated: None,
            consolidated_at: None,
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
            make_doc_full(
                "tasks/proj.md",
                "Project",
                "project",
                "active",
                "proj-1",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/epic.md",
                "Epic One",
                "epic",
                "active",
                "epic-1",
                Some("proj-1"),
                &[],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/up2.md",
                "Up Two",
                "task",
                "done",
                "task-up2",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/up1.md",
                "Up One",
                "task",
                "active",
                "task-up1",
                None,
                &["task-up2"],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/soft.md",
                "Soft Dep",
                "task",
                "active",
                "task-soft",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/mid.md",
                "Mid Task",
                "task",
                "in_progress",
                "task-mid",
                Some("epic-1"),
                &["task-up1"],
                &["task-soft"],
                &["task-down1"],
                &[],
            ),
            make_doc_full(
                "tasks/down1.md",
                "Down One",
                "task",
                "blocked",
                "task-down1",
                None,
                &[],
                &[],
                &["task-down2"],
                &[],
            ),
            make_doc_full(
                "tasks/down2.md",
                "Down Two",
                "task",
                "active",
                "task-down2",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
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
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Breadcrumb
        assert!(
            combined.contains("Project \u{203A} Epic One"),
            "missing breadcrumb:\n{combined}"
        );
        // Section headers present and ordered
        let up_pos = combined.find("Upstream").expect("upstream header");
        let down_pos = combined.find("Downstream").expect("downstream header");
        assert!(
            up_pos < down_pos,
            "upstream must precede downstream:\n{combined}"
        );
        // Target highlighted
        assert!(
            combined.contains("\u{2605} Mid Task"),
            "missing star+target:\n{combined}"
        );
        // Direct upstream + transitive upstream both visible (default depth 2)
        assert!(
            combined.contains("Up One"),
            "missing direct upstream:\n{combined}"
        );
        assert!(
            combined.contains("Up Two"),
            "missing transitive upstream:\n{combined}"
        );
        // Direct + transitive downstream
        assert!(
            combined.contains("Down One"),
            "missing direct downstream:\n{combined}"
        );
        assert!(
            combined.contains("Down Two"),
            "missing transitive downstream:\n{combined}"
        );
        // Status tags inline (plain mode → bracketed)
        assert!(
            combined.contains("[in_progress]"),
            "missing target status:\n{combined}"
        );
        assert!(
            combined.contains("[blocked]"),
            "missing downstream status:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_distinguishes_soft_edges() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Soft dep visible by default
        assert!(
            combined.contains("Soft Dep"),
            "expected soft dep included:\n{combined}"
        );
        // Soft tag present (label or dashed connector)
        assert!(
            combined.contains("(soft)") || combined.contains("\u{2504}\u{2504}"),
            "expected soft marker:\n{combined}"
        );

        // --no-soft → no soft dep listed
        let opts2 = NeighbourhoodOpts {
            include_soft: false,
            plain: true,
            ..Default::default()
        };
        let combined2 = render_neighbourhood(&gs, "task-mid", &opts2).join("\n");
        assert!(
            !combined2.contains("Soft Dep"),
            "soft dep should be hidden:\n{combined2}"
        );
    }

    #[test]
    fn neighbourhood_depth_limits_recursion() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts {
            upstream_depth: 1,
            downstream_depth: 1,
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");

        // Depth 1: direct deps only
        assert!(
            combined.contains("Up One"),
            "depth-1 must include direct upstream:\n{combined}"
        );
        assert!(
            !combined.contains("Up Two"),
            "depth-1 must exclude transitive:\n{combined}"
        );
        assert!(
            combined.contains("Down One"),
            "depth-1 must include direct downstream:\n{combined}"
        );
        assert!(
            !combined.contains("Down Two"),
            "depth-1 must exclude transitive:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_upstream_zero_hides_upstream_section() {
        let gs = build_chain();
        // pkb blocks semantics: only downstream
        let opts = NeighbourhoodOpts {
            upstream_depth: 0,
            downstream_depth: 3,
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");
        assert!(
            !combined.contains("Upstream"),
            "upstream section must be hidden:\n{combined}"
        );
        assert!(
            combined.contains("Downstream"),
            "downstream section expected:\n{combined}"
        );
        assert!(
            combined.contains("Down Two"),
            "transitive downstream expected:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_plain_mode_strips_ansi() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-mid", &opts).join("\n");
        assert!(
            !combined.contains("\x1b["),
            "plain mode must not emit ANSI:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_works_for_epic_with_children() {
        let gs = build_chain();
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "epic-1", &opts).join("\n");

        // Epic should show child task in its downstream tree
        assert!(
            combined.contains("\u{2605} Epic One"),
            "missing target epic:\n{combined}"
        );
        assert!(
            combined.contains("Mid Task"),
            "epic should show child:\n{combined}"
        );
        assert!(
            combined.contains("(child)") || combined.contains("Downstream"),
            "expected child section:\n{combined}"
        );
    }

    // -----------------------------------------------------------------------
    // Cycle-marker correctness: a diamond (two paths to the same node) is
    // not a cycle; a true back-edge within one relation is.
    // -----------------------------------------------------------------------

    #[test]
    fn neighbourhood_diamond_is_not_marked_cycle() {
        // root --blocks--> mid1 --blocks--> leaf
        // root --blocks--> mid2 --blocks--> leaf
        // `leaf` is reachable via two independent branches, not by looping
        // back on itself, so it must never get a `(cycle)` marker even
        // though the walk visits it twice.
        let docs = vec![
            make_doc_full(
                "tasks/root.md",
                "Root",
                "task",
                "active",
                "d-root",
                None,
                &[],
                &[],
                &["d-mid1", "d-mid2"],
                &[],
            ),
            make_doc_full(
                "tasks/mid1.md",
                "Mid One",
                "task",
                "active",
                "d-mid1",
                None,
                &[],
                &[],
                &["d-leaf"],
                &[],
            ),
            make_doc_full(
                "tasks/mid2.md",
                "Mid Two",
                "task",
                "active",
                "d-mid2",
                None,
                &[],
                &[],
                &["d-leaf"],
                &[],
            ),
            make_doc_full(
                "tasks/leaf.md",
                "Leaf",
                "task",
                "active",
                "d-leaf",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
        ];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "d-root", &opts).join("\n");

        assert!(
            !combined.contains("(cycle"),
            "diamond must not be marked as a cycle:\n{combined}"
        );
        assert!(
            combined.contains("Leaf"),
            "leaf should still be shown via at least one branch:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_true_backedge_is_marked_cycle_with_relation() {
        // task-x --blocks--> task-y --blocks--> task-x: a genuine back-edge,
        // entirely within the `blocks` relation.
        let docs = vec![
            make_doc_full(
                "tasks/x.md",
                "Task X",
                "task",
                "active",
                "cyc-x",
                None,
                &[],
                &[],
                &["cyc-y"],
                &[],
            ),
            make_doc_full(
                "tasks/y.md",
                "Task Y",
                "task",
                "active",
                "cyc-y",
                None,
                &[],
                &[],
                &["cyc-x"],
                &[],
            ),
        ];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "cyc-x", &opts).join("\n");

        assert!(
            combined.contains("(cycle: blocks)"),
            "true back-edge in `blocks` must be marked with its relation:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_cross_relation_revisit_is_not_marked_cycle() {
        // root --child--> mid, and mid --blocks--> root: mid reaches back to
        // root, but via a different relation than the one used to reach
        // mid, so this is not a cycle in any single relation. Recursion
        // must still stop (else it would loop forever), but no marker
        // should be printed.
        let docs = vec![
            make_doc_full(
                "tasks/croot.md",
                "C Root",
                "epic",
                "active",
                "cr-root",
                None,
                &[],
                &[],
                &[],
                &[],
            ),
            make_doc_full(
                "tasks/cmid.md",
                "C Mid",
                "task",
                "active",
                "cr-mid",
                Some("cr-root"),
                &[],
                &[],
                &["cr-root"],
                &[],
            ),
        ];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "cr-root", &opts).join("\n");

        assert!(
            !combined.contains("(cycle"),
            "a revisit across two different relations is not a single-relation cycle:\n{combined}"
        );
        assert!(
            combined.contains("C Mid"),
            "child should still be shown:\n{combined}"
        );
    }

    #[test]
    fn neighbourhood_isolated_node() {
        let docs = vec![make_doc_full(
            "tasks/lone.md",
            "Lonely",
            "task",
            "active",
            "task-lone",
            None,
            &[],
            &[],
            &[],
            &[],
        )];
        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let opts = NeighbourhoodOpts {
            plain: true,
            ..Default::default()
        };
        let combined = render_neighbourhood(&gs, "task-lone", &opts).join("\n");
        assert!(combined.contains("\u{2605} Lonely"));
        assert!(combined.contains("no dependency relationships"));
    }

    #[test]
    fn test_nested_task_ascii_tree_renders_hierarchy_and_metadata() {
        let mut fm_a = serde_json::Map::new();
        fm_a.insert("title".to_string(), serde_json::json!("Task A"));
        fm_a.insert("type".to_string(), serde_json::json!("task"));
        fm_a.insert("status".to_string(), serde_json::json!("ready"));
        fm_a.insert("id".to_string(), serde_json::json!("task-a"));
        fm_a.insert("priority".to_string(), serde_json::json!(1));
        fm_a.insert("parent".to_string(), serde_json::json!("epic-1"));
        fm_a.insert("due".to_string(), serde_json::json!("2026-09-15"));
        fm_a.insert("complexity".to_string(), serde_json::json!("medium"));

        let mut fm_b = serde_json::Map::new();
        fm_b.insert("title".to_string(), serde_json::json!("Task B"));
        fm_b.insert("type".to_string(), serde_json::json!("task"));
        fm_b.insert("status".to_string(), serde_json::json!("blocked"));
        fm_b.insert("id".to_string(), serde_json::json!("task-b"));
        fm_b.insert("priority".to_string(), serde_json::json!(2));
        fm_b.insert("parent".to_string(), serde_json::json!("epic-1"));

        let mut fm_lone = serde_json::Map::new();
        fm_lone.insert("title".to_string(), serde_json::json!("Orphan Task"));
        fm_lone.insert("type".to_string(), serde_json::json!("task"));
        fm_lone.insert("status".to_string(), serde_json::json!("ready"));
        fm_lone.insert("id".to_string(), serde_json::json!("orphan-1"));
        fm_lone.insert("priority".to_string(), serde_json::json!(0));

        let docs = vec![
            make_doc("tasks/epic-1.md", "Epic One", "epic", "active", "epic-1", None, &[]),
            PkbDocument {
                path: PathBuf::from("tasks/task-a.md"),
                title: "Task A".to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                consolidated: None,
                consolidated_at: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_a)),
                modified: None,
                content_hash: String::new(),
                file_hash: String::new(),
            },
            PkbDocument {
                path: PathBuf::from("tasks/task-b.md"),
                title: "Task B".to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("blocked".to_string()),
                consolidated: None,
                consolidated_at: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_b)),
                modified: None,
                content_hash: String::new(),
                file_hash: String::new(),
            },
            PkbDocument {
                path: PathBuf::from("tasks/orphan.md"),
                title: "Orphan Task".to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                consolidated: None,
                consolidated_at: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_lone)),
                modified: None,
                content_hash: String::new(),
                file_hash: String::new(),
            },
        ];

        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let node_a = gs.get_node("task-a").unwrap();
        let node_b = gs.get_node("task-b").unwrap();
        let node_lone = gs.get_node("orphan-1").unwrap();

        let lines = render_nested_task_ascii_tree(&gs, &[node_a, node_b, node_lone], 100, true);
        let output = lines.join("\n");

        // Epic One should appear as a context container with count 2
        assert!(output.contains("▌ Epic One (2)  [epic-1]"), "expected context epic in output:\n{output}");
        // Both tasks should be nested with priority and metadata
        assert!(output.contains("P1  Task A"), "expected Task A line in output:\n{output}");
        assert!(output.contains("[medium]"), "expected complexity on Task A in output:\n{output}");
        assert!(output.contains("due:2026-09-15"), "expected due date on Task A in output:\n{output}");
        assert!(output.contains("P2  Task B"), "expected Task B line in output:\n{output}");
        // Orphan task should be a root
        assert!(output.contains("P0  Orphan Task"), "expected Orphan Task in output:\n{output}");
        // Connectors should be used
        assert!(output.contains("├── ") || output.contains("└── "), "expected box drawing connectors in output:\n{output}");
        // Plain output must not contain raw ANSI escapes
        assert!(!output.contains("\x1b["), "plain output should have no ANSI escapes:\n{output}");
    }

    #[test]
    fn test_nested_task_json_structure_brief_metadata() {
        let mut fm_a = serde_json::Map::new();
        fm_a.insert("title".to_string(), serde_json::json!("Task A"));
        fm_a.insert("type".to_string(), serde_json::json!("task"));
        fm_a.insert("status".to_string(), serde_json::json!("ready"));
        fm_a.insert("id".to_string(), serde_json::json!("task-a"));
        fm_a.insert("priority".to_string(), serde_json::json!(1));
        fm_a.insert("parent".to_string(), serde_json::json!("epic-1"));
        fm_a.insert("complexity".to_string(), serde_json::json!("low"));

        let docs = vec![
            make_doc("tasks/epic-1.md", "Epic One", "epic", "active", "epic-1", None, &[]),
            PkbDocument {
                path: PathBuf::from("tasks/task-a.md"),
                title: "Task A".to_string(),
                body: String::new(),
                doc_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                consolidated: None,
                consolidated_at: None,
                tags: vec![],
                frontmatter: Some(serde_json::Value::Object(fm_a)),
                modified: None,
                content_hash: String::new(),
                file_hash: String::new(),
            },
        ];

        let gs = GraphStore::build(&docs, Path::new("/tmp/test-pkb"));
        let node_a = gs.get_node("task-a").unwrap();

        let json_tree = build_nested_task_json(&gs, &[node_a]);
        assert_eq!(json_tree.len(), 1, "expected 1 root (epic-1)");

        let root = &json_tree[0];
        assert_eq!(root.id, "epic-1");
        assert_eq!(root.title, "Epic One");
        assert_eq!(root.is_context, Some(true));
        assert_eq!(root.children.len(), 1);

        let child = &root.children[0];
        assert_eq!(child.id, "task-a");
        assert_eq!(child.title, "Task A");
        assert_eq!(child.status.as_deref(), Some("ready"));
        assert_eq!(child.intent, Some(1));
        assert_eq!(child.complexity.as_deref(), Some("low"));
        assert_eq!(child.is_context, None);
        assert_eq!(child.children.len(), 0);

        // Serialize and verify JSON structure
        let serialized = serde_json::to_string_pretty(&json_tree).unwrap();
        assert!(serialized.contains("\"id\": \"epic-1\""));
        assert!(serialized.contains("\"id\": \"task-a\""));
        assert!(!serialized.contains("\"signals\""), "brief metadata JSON must not contain signals");
        assert!(!serialized.contains("\"body\""), "brief metadata JSON must not contain body");
    }
