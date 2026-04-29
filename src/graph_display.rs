use crate::graph_store::GraphStore;

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
}
