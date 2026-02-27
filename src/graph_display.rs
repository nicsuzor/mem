use crate::graph_store::GraphStore;

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
/// [Target] <- [Depends On]
/// [Target] -> [Blocks]
pub fn render_ascii_graph(gs: &GraphStore, node_id: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let node = match gs.get_node(node_id) {
        Some(n) => n,
        None => return vec![format!("Node not found: {}", node_id)],
    };

    // 1. Parents (Above)
    if let Some(ref pid) = node.parent {
        if let Some(parent) = gs.get_node(pid) {
            lines.push(format!("      [{}]", parent.label));
            lines.push("          |".to_string());
        }
    }

    // 2. Siblings & Target (Middle)
    let mut middle = format!("[{}]", node.label.to_uppercase());
    
    // Add siblings if any
    if let Some(ref pid) = node.parent {
        if let Some(parent) = gs.get_node(pid) {
            let siblings: Vec<_> = parent.children.iter()
                .filter(|id| *id != node_id)
                .filter_map(|id| gs.get_node(id))
                .take(2)
                .collect();
            
            if !siblings.is_empty() {
                let mut left_sibs = Vec::new();
                let mut right_sibs = Vec::new();
                for (i, sib) in siblings.iter().enumerate() {
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
        }
    }
    lines.push(middle);

    // 3. Children (Below)
    if !node.children.is_empty() {
        lines.push("          |".to_string());
        for child_id in node.children.iter().take(3) {
            if let Some(child) = gs.get_node(child_id) {
                lines.push(format!("          +-- [{}]", child.label));
            }
        }
        if node.children.len() > 3 {
            lines.push(format!("          +-- ... ({} more)", node.children.len() - 3));
        }
    }

    // 4. Dependencies
    if !node.depends_on.is_empty() || !node.blocks.is_empty() {
        lines.push("".to_string());
        for dep_id in &node.depends_on {
            if let Some(dep) = gs.get_node(dep_id) {
                lines.push(format!("  [Target] <--- [{}] (depends on)", dep.label));
            }
        }
        for blocked_id in &node.blocks {
            if let Some(blocked) = gs.get_node(blocked_id) {
                lines.push(format!("  [Target] ---> [{}] (blocks)", blocked.label));
            }
        }
    }

    lines
}

/// Renders relationships for TUI or CLI using styled spans if possible.
/// For now, just providing the base ASCII is a good start.
pub fn get_local_context_lines(gs: &GraphStore, node_id: &str) -> Vec<String> {
    render_ascii_graph(gs, node_id)
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

        // Should show a dependency line (task-a depends on task-b)
        assert!(
            combined.contains("<--- [Task B] (depends on)"),
            "Expected depends_on line for Task B in output:\n{}",
            combined
        );

        // Should show a blocks line (task-a blocks task-c)
        assert!(
            combined.contains("---> [Task C] (blocks)"),
            "Expected blocks line for Task C in output:\n{}",
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

        // No parent line (no "[" before the target row with connector "|")
        // The first line should be the target itself (uppercased)
        assert!(
            lines[0].contains("[TASK C]"),
            "Expected orphan's first line to be the target node, got:\n{}",
            combined
        );

        // Should still show dependency
        assert!(
            combined.contains("<--- [Task A] (depends on)"),
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
    fn test_get_local_context_lines_delegates() {
        let gs = build_graph();
        let direct = render_ascii_graph(&gs, "task-a");
        let via_helper = get_local_context_lines(&gs, "task-a");
        assert_eq!(direct, via_helper);
    }
}
