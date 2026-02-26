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
