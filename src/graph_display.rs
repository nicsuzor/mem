use crate::graph_store::GraphStore;

/// A node reference with label, type, and status for rendering.
#[derive(Debug, Clone)]
pub struct ContextNode {
    pub label: String,
    pub node_type: Option<String>,
    pub status: Option<String>,
}

/// Structured local graph context for a node, usable by both CLI and TUI.
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
    pub is_orphan: bool,
}

/// Extract structured local context for a node. Returns `None` if the node doesn't exist.
pub fn get_local_context(gs: &GraphStore, node_id: &str) -> Option<LocalContext> {
    let node = gs.get_node(node_id)?;

    let target = ContextNode {
        label: node.label.clone(),
        node_type: node.node_type.clone(),
        status: node.status.clone(),
    };

    // Walk parent chain
    let mut parents = Vec::new();
    let mut parent_id = node.parent.as_deref();
    while let Some(pid) = parent_id {
        if let Some(parent) = gs.get_node(pid) {
            parents.push(ContextNode {
                label: parent.label.clone(),
                node_type: parent.node_type.clone(),
                status: parent.status.clone(),
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
                    label: n.label.clone(),
                    node_type: n.node_type.clone(),
                    status: n.status.clone(),
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
        .take(3)
        .map(|n| ContextNode {
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
        })
        .collect();

    let depends_on: Vec<_> = node.depends_on.iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
        })
        .collect();

    let blocks: Vec<_> = node.blocks.iter()
        .filter_map(|id| gs.get_node(id))
        .map(|n| ContextNode {
            label: n.label.clone(),
            node_type: n.node_type.clone(),
            status: n.status.clone(),
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
    })
}

/// Renders a compact 2D ASCII representation of a node's local neighbourhood.
///
/// Layout:
///         [Parent]
///            |
/// [Sibling]--[TARGET]--[Sibling]
///            |
///        [Children...]
///
/// Dependencies are shown with arrows:
/// [TARGET] <--- [Depends On]
/// [TARGET] ---> [Blocks]
pub fn render_ascii_graph(gs: &GraphStore, node_id: &str) -> Vec<String> {
    let ctx = match get_local_context(gs, node_id) {
        Some(c) => c,
        None => return vec![format!("Node not found: {}", node_id)],
    };

    let mut lines = Vec::new();
    let target_label = ctx.target.label.to_uppercase();

    // 1. Parent (immediate, above)
    if let Some(parent) = ctx.parents.first() {
        lines.push(format!("      [{}]", parent.label));
        lines.push("          |".to_string());
    }

    // 2. Siblings & Target (middle row)
    let mut middle = format!("[{}]", target_label);
    if !ctx.siblings.is_empty() {
        let sibs: Vec<_> = ctx.siblings.iter().take(2).collect();
        let mut left_sibs = Vec::new();
        let mut right_sibs = Vec::new();
        for (i, sib) in sibs.iter().enumerate() {
            if i % 2 == 0 {
                left_sibs.push(format!("[{}]", sib.label));
            } else {
                right_sibs.push(format!("[{}]", sib.label));
            }
        }
        if !left_sibs.is_empty() {
            middle = format!("{}--{}", left_sibs.join("--"), middle);
        }
        if !right_sibs.is_empty() {
            middle = format!("{}--{}", middle, right_sibs.join("--"));
        }
    }
    lines.push(middle);

    // 3. Children (below)
    if !ctx.children.is_empty() {
        lines.push("          |".to_string());
        for child in &ctx.children {
            lines.push(format!("          +-- [{}]", child.label));
        }
        if ctx.children_total > 3 {
            lines.push(format!("          +-- ... ({} more)", ctx.children_total - 3));
        }
    }

    // 4. Dependencies — use actual node label instead of [Target]
    if !ctx.depends_on.is_empty() || !ctx.blocks.is_empty() {
        lines.push("".to_string());
        for dep in &ctx.depends_on {
            lines.push(format!("  [{}] <--- [{}] (depends on)", target_label, dep.label));
        }
        for blocked in &ctx.blocks {
            lines.push(format!("  [{}] ---> [{}] (blocks)", target_label, blocked.label));
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
            mtime: 1000,
        }
    }

    /// Build a graph with rich relationships:
    ///   epic-1 (parent of task-a, task-b)
    ///   task-a depends on task-b
    ///   task-c depends on task-a (so task-a blocks task-c)
    ///   isolated (no parent, no deps)
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

    /// Build a graph where a parent has more than 3 children to test truncation.
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
        // task-a has parent epic-1, depends_on task-b, and blocks task-c
        let lines = render_ascii_graph(&gs, "task-a");
        let combined = lines.join("\n");

        // Should show parent above
        assert!(
            combined.contains("[Epic One]"),
            "Expected parent label [Epic One] in output:\n{}",
            combined
        );

        // The target node label is uppercased
        assert!(
            combined.contains("[TASK A]"),
            "Expected uppercased target [TASK A] in output:\n{}",
            combined
        );

        // Should show a dependency line with actual node label (not [Target])
        assert!(
            combined.contains("[TASK A] <--- [Task B] (depends on)"),
            "Expected depends_on line with actual label in output:\n{}",
            combined
        );

        // Should show a blocks line with actual node label
        assert!(
            combined.contains("[TASK A] ---> [Task C] (blocks)"),
            "Expected blocks line with actual label in output:\n{}",
            combined
        );

        // Sibling: task-b is also a child of epic-1, so it should appear as a sibling
        assert!(
            combined.contains("[Task B]"),
            "Expected sibling [Task B] in middle row in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_orphan_node_no_parent() {
        let gs = build_graph();
        // task-c has no parent but depends on task-a
        let lines = render_ascii_graph(&gs, "task-c");
        let combined = lines.join("\n");

        // No parent line — first line should be the target itself (uppercased)
        assert!(
            lines[0].contains("[TASK C]"),
            "Expected orphan's first line to be the target node, got:\n{}",
            combined
        );

        // Should still show dependency with actual label
        assert!(
            combined.contains("[TASK C] <--- [Task A] (depends on)"),
            "Expected depends_on line for Task A in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_isolated_node_no_relationships() {
        let gs = build_graph();
        // isolated has no parent, no children, no deps, no blocks
        let lines = render_ascii_graph(&gs, "isolated");
        let combined = lines.join("\n");

        // Should just have the target line
        assert_eq!(
            lines.len(),
            1,
            "Expected exactly 1 line for isolated node, got {} lines:\n{}",
            lines.len(),
            combined
        );
        assert!(
            lines[0].contains("[ISOLATED]"),
            "Expected [ISOLATED] in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_children_truncation_at_three() {
        let gs = build_graph_many_children();
        // big-parent has 5 children
        let lines = render_ascii_graph(&gs, "big-parent");
        let combined = lines.join("\n");

        // Should show exactly 3 "+-- [Child" lines
        let child_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("+-- [Child"))
            .collect();
        assert_eq!(
            child_lines.len(),
            3,
            "Expected exactly 3 child lines before truncation, got {}:\n{}",
            child_lines.len(),
            combined
        );

        // Should show a "... (2 more)" truncation line
        assert!(
            combined.contains("(2 more)"),
            "Expected truncation indicator '(2 more)' in output:\n{}",
            combined
        );
    }

    #[test]
    fn test_get_local_context_structured() {
        let gs = build_graph();
        let ctx = get_local_context(&gs, "task-a").expect("task-a should exist");

        assert_eq!(ctx.target.label, "Task A");
        assert!(!ctx.is_orphan);
        assert_eq!(ctx.parents.len(), 1);
        assert_eq!(ctx.parents[0].label, "Epic One");
        assert_eq!(ctx.parents[0].node_type.as_deref(), Some("epic"));

        // task-b is a sibling under epic-1
        assert!(!ctx.siblings.is_empty());
        assert!(ctx.siblings.iter().any(|s| s.label == "Task B"));

        // depends_on task-b
        assert_eq!(ctx.depends_on.len(), 1);
        assert_eq!(ctx.depends_on[0].label, "Task B");

        // blocks task-c
        assert_eq!(ctx.blocks.len(), 1);
        assert_eq!(ctx.blocks[0].label, "Task C");
    }

    #[test]
    fn test_get_local_context_orphan() {
        let gs = build_graph();
        let ctx = get_local_context(&gs, "isolated").expect("isolated should exist");
        assert!(ctx.is_orphan);
        assert!(ctx.parents.is_empty());
        assert!(ctx.siblings.is_empty());
        assert!(ctx.children.is_empty());
        assert!(ctx.depends_on.is_empty());
        assert!(ctx.blocks.is_empty());
    }

    #[test]
    fn test_get_local_context_none_for_missing() {
        let gs = build_graph();
        assert!(get_local_context(&gs, "nonexistent").is_none());
    }
}
