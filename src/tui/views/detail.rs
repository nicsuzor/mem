//! Node Detail overlay — shows full context for a selected node.
//!
//! Split panel layout: metadata/body on the left, graph context + PKB on the right.

use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;
use crate::tui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let node_id = match &app.detail_node_id {
        Some(id) => id,
        None => return,
    };

    let gs = match &app.graph {
        Some(gs) => gs,
        None => return,
    };

    let node = match gs.get_node(node_id) {
        Some(n) => n,
        None => return,
    };

    // Center the overlay with margins
    let margin_h = (area.width / 10).max(2);
    let margin_v = (area.height / 10).max(1);
    let overlay = Rect::new(
        area.x + margin_h,
        area.y + margin_v,
        area.width.saturating_sub(margin_h * 2),
        area.height.saturating_sub(margin_v * 2),
    );

    frame.render_widget(Clear, overlay);
    frame.render_widget(Theme::active_block().title(" Detail "), overlay);

    // Split into left and right panels
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(1),    // content
            Constraint::Length(1), // keybindings
        ])
        .margin(1)
        .split(overlay);

    // Title bar
    let tid = node.task_id.as_deref().unwrap_or(&node.id);
    let short_id = if tid.len() > 20 {
        format!("{}…", &tid[..20])
    } else {
        tid.to_string()
    };

    let title_bar = Paragraph::new(Line::from(vec![
        Span::styled(
            &node.label,
            Style::default().fg(Theme::FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  [{short_id}]"), Style::default().fg(Theme::MUTED)),
    ]));
    frame.render_widget(title_bar, inner[0]);

    // Bottom keybindings
    let keys = Paragraph::new(Line::from(vec![Span::styled(
        " Esc back │ j/k scroll ",
        Style::default().fg(Theme::MUTED),
    )]));
    frame.render_widget(keys, inner[2]);

    // Split content area into left and right panels
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // left: metadata
            Constraint::Percentage(50), // right: graph context
        ])
        .split(inner[1]);

    // ── LEFT PANEL: metadata, subtasks, body ──
    let mut left_lines: Vec<Line> = Vec::new();

    // Type + Status + Priority
    let status = node.status.as_deref().unwrap_or("unknown");
    let status_color = match status {
        "active" | "in_progress" => Theme::ACCENT_SECONDARY,
        "blocked" => Theme::ERROR,
        "seed" => Theme::MUTED,
        "growing" => Theme::WARNING,
        "dormant" => Theme::MUTED,
        "done" | "complete" => Theme::SUCCESS,
        _ => Theme::FG,
    };
    let pri = node.priority.unwrap_or(2);
    left_lines.push(Line::from(""));

    // Node type badge (prominent for non-task types)
    if let Some(ref ntype) = node.node_type {
        if ntype != "task" {
            let (icon, color) = match ntype.as_str() {
                "source" => ("📖", Theme::ACCENT_PRIMARY),
                "note" | "knowledge" => ("📝", Theme::ACCENT_SECONDARY),
                "goal" => ("◉", Theme::WARNING),
                "project" | "subproject" => ("◈", Theme::ACCENT_SECONDARY),
                "epic" => ("◈", Theme::ACCENT_SECONDARY),
                "memory" | "insight" => ("💡", Theme::WARNING),
                _ => ("·", Theme::FG),
            };
            left_lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                Span::styled(ntype.clone(), Style::default().fg(color).bold()),
            ]));
        }
    }

    left_lines.push(Line::from(vec![
        Span::styled("  Status: ", Style::default().fg(Theme::MUTED)),
        Span::styled(status, Style::default().fg(status_color).bold()),
        Span::styled(format!("   P{pri}"), Style::default().fg(Theme::FG).bold()),
    ]));

    // Dates
    if let Some(ref created) = node.created {
        let mut spans = vec![
            Span::styled("  Created: ", Style::default().fg(Theme::MUTED)),
            Span::styled(created.clone(), Style::default().fg(Theme::FG)),
        ];
        // Staleness
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
            let days = (chrono::Local::now().date_naive() - dt).num_days();
            let color = if days > 30 {
                Theme::ERROR
            } else if days > 14 {
                Theme::WARNING
            } else {
                Theme::MUTED
            };
            spans.push(Span::styled(
                format!("  ({days}d)"),
                Style::default().fg(color),
            ));
        }
        left_lines.push(Line::from(spans));
    }
    if let Some(ref due) = node.due {
        left_lines.push(Line::from(vec![
            Span::styled("  Due: ", Style::default().fg(Theme::MUTED)),
            Span::styled(due.clone(), Style::default().fg(Theme::WARNING).bold()),
        ]));
    }

    // Project
    if let Some(ref project) = node.project {
        left_lines.push(Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Theme::MUTED)),
            Span::styled(
                project.clone(),
                Style::default().fg(Theme::ACCENT_SECONDARY),
            ),
        ]));
    }

    // Assignee
    if let Some(ref assignee) = node.assignee {
        left_lines.push(Line::from(vec![
            Span::styled("  Assignee: ", Style::default().fg(Theme::MUTED)),
            Span::styled(assignee.clone(), Style::default().fg(Theme::FG)),
        ]));
    }

    // Tags
    if !node.tags.is_empty() {
        left_lines.push(Line::from(vec![
            Span::styled("  Tags: ", Style::default().fg(Theme::MUTED)),
            Span::styled(node.tags.join(", "), Style::default().fg(Theme::SUCCESS)),
        ]));
    }

    // Children / Subtasks
    if !node.children.is_empty() {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "  SUBTASKS",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        )));
        left_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Theme::MUTED),
        )));
        for child_id in &node.children {
            if let Some(child) = gs.get_node(child_id) {
                let (icon, color) = match child.status.as_deref() {
                    Some("done") | Some("complete") => ("✓", Theme::SUCCESS),
                    Some("active") | Some("in_progress") => ("●", Theme::ACCENT_SECONDARY),
                    Some("blocked") => ("✗", Theme::ERROR),
                    Some("seed") => ("○", Theme::MUTED),
                    _ => ("○", Theme::FG),
                };
                left_lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} "), Style::default().fg(color)),
                    Span::styled(child.label.clone(), Style::default().fg(Theme::FG)),
                ]));
            }
        }
    }

    // Downstream weight
    if node.downstream_weight > 0.0 {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(vec![
            Span::styled("  Weight: ", Style::default().fg(Theme::MUTED)),
            Span::styled(
                format!("{:.1}", node.downstream_weight),
                Style::default().fg(Theme::FG),
            ),
            if node.stakeholder_exposure {
                Span::styled(" (stakeholder!)", Style::default().fg(Theme::WARNING))
            } else {
                Span::raw("")
            },
        ]));
    }

    // Assumptions
    if !node.assumptions.is_empty() {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "  ASSUMPTIONS",
            Style::default().fg(Theme::WARNING).bold(),
        )));
        left_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Theme::MUTED),
        )));
        for a in &node.assumptions {
            let (icon, color) = match a.status.as_str() {
                "confirmed" => ("✓", Theme::SUCCESS),
                "invalidated" => ("✗", Theme::ERROR),
                _ => ("?", Theme::WARNING), // untested
            };
            left_lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                Span::styled(a.text.clone(), Style::default().fg(Theme::FG)),
                Span::styled(format!("  [{}]", a.status), Style::default().fg(color)),
            ]));
        }
    }

    let scroll = app.detail_scroll.min(left_lines.len().saturating_sub(1));
    let left_text = Text::from(left_lines);
    let left_panel = Paragraph::new(left_text).scroll((scroll as u16, 0)).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Theme::MUTED)),
    );

    frame.render_widget(left_panel, panels[0]);

    // ── RIGHT PANEL: graph context ──
    let mut right_lines: Vec<Line> = Vec::new();

    right_lines.push(Line::from(""));
    right_lines.push(Line::from(Span::styled(
        "  GRAPH CONTEXT",
        Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
    )));
    right_lines.push(Line::from(Span::styled(
        "  ─────",
        Style::default().fg(Theme::MUTED),
    )));

    // Walk up parent chain (enables)
    let mut parent_id = node.parent.as_deref();
    let mut parent_chain = Vec::new();
    while let Some(pid) = parent_id {
        if let Some(parent) = gs.get_node(pid) {
            let icon = match parent.node_type.as_deref() {
                Some("goal") => "◉",
                Some("project") | Some("subproject") => "◈",
                Some("epic") => "◈",
                _ => "◇",
            };
            let color = match parent.node_type.as_deref() {
                Some("goal") => Theme::WARNING,
                Some("project") | Some("subproject") | Some("epic") => Theme::ACCENT_SECONDARY,
                _ => Theme::FG,
            };
            parent_chain.push((format!("{icon} {}", parent.label), color));
            parent_id = parent.parent.as_deref();
        } else {
            break;
        }
    }

    if parent_chain.is_empty() {
        right_lines.push(Line::from(Span::styled(
            "  ↑ (orphan — no parent chain)",
            Style::default().fg(Theme::WARNING),
        )));
    } else {
        right_lines.push(Line::from(Span::styled(
            "  ↑ enables:",
            Style::default().fg(Theme::MUTED),
        )));
        parent_chain.reverse();
        for (label, color) in &parent_chain {
            right_lines.push(Line::from(Span::styled(
                format!("    {label}"),
                Style::default().fg(*color),
            )));
        }
    }

    // Dependencies (depends_on — blocked by)
    if !node.depends_on.is_empty() {
        right_lines.push(Line::from(""));
        right_lines.push(Line::from(Span::styled(
            "  ↓ depends on:",
            Style::default().fg(Theme::MUTED),
        )));
        for dep_id in &node.depends_on {
            if let Some(dep) = gs.get_node(dep_id) {
                let done = matches!(dep.status.as_deref(), Some("done"));
                let color = if done { Theme::SUCCESS } else { Theme::ERROR };
                let icon = if done { "✓" } else { "✗" };
                right_lines.push(Line::from(vec![
                    Span::styled(format!("    {icon} "), Style::default().fg(color)),
                    Span::styled(dep.label.clone(), Style::default().fg(color)),
                ]));
            } else {
                right_lines.push(Line::from(Span::styled(
                    format!("    ? {dep_id}"),
                    Style::default().fg(Theme::ERROR),
                )));
            }
        }
    }

    // Blocks (what completing this would unblock)
    if !node.blocks.is_empty() {
        right_lines.push(Line::from(""));
        right_lines.push(Line::from(Span::styled(
            "  → completing this unblocks:",
            Style::default().fg(Theme::MUTED),
        )));
        for block_id in &node.blocks {
            if let Some(blocked) = gs.get_node(block_id) {
                right_lines.push(Line::from(Span::styled(
                    format!("    ◇ {}", blocked.label),
                    Style::default().fg(Theme::FG),
                )));
            }
        }
    }

    // Related (siblings — same parent, different node)
    if let Some(ref pid) = node.parent {
        if let Some(parent) = gs.get_node(pid) {
            let siblings: Vec<&str> = parent
                .children
                .iter()
                .filter(|cid| cid.as_str() != node.id)
                .filter_map(|cid| gs.get_node(cid).map(|n| n.label.as_str()))
                .take(5)
                .collect();
            if !siblings.is_empty() {
                right_lines.push(Line::from(""));
                right_lines.push(Line::from(Span::styled(
                    "  ↔ related (siblings):",
                    Style::default().fg(Theme::MUTED),
                )));
                for sib in &siblings {
                    right_lines.push(Line::from(Span::styled(
                        format!("    ◇ {sib}"),
                        Style::default().fg(Theme::FG),
                    )));
                }
            }
        }
    }

    // ── PKB CONNECTIONS ──

    // Backlinks (nodes that link TO this node via wikilinks/references)
    let backlinks = gs.backlinks_by_type(&node.id);
    if !backlinks.is_empty() {
        right_lines.push(Line::from(""));
        right_lines.push(Line::from(Span::styled(
            "  PKB BACKLINKS",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        )));
        right_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Theme::MUTED),
        )));
        for (source_type, refs) in &backlinks {
            // Skip structural edges we already show above
            let has_link = refs
                .iter()
                .any(|(_, et)| matches!(et, mem::graph::EdgeType::Link));
            if !has_link {
                continue;
            }
            right_lines.push(Line::from(Span::styled(
                format!("  [{source_type}]"),
                Style::default().fg(Theme::MUTED).italic(),
            )));
            for (source_node, edge_type) in refs {
                if !matches!(edge_type, mem::graph::EdgeType::Link) {
                    continue;
                }
                let icon = match source_node.node_type.as_deref() {
                    Some("source") => "📖",
                    Some("note") | Some("knowledge") => "📝",
                    Some("task") => "◇",
                    Some("project") => "◈",
                    _ => "·",
                };
                right_lines.push(Line::from(Span::styled(
                    format!("    {icon} {}", source_node.label),
                    Style::default().fg(Theme::FG),
                )));
            }
        }
    }

    // Tag overlap — other nodes sharing tags with this one
    if !node.tags.is_empty() {
        let node_tags: HashSet<&str> = node.tags.iter().map(|t| t.as_str()).collect();
        let mut tag_matches: Vec<(&str, usize)> = Vec::new();
        for other in gs.nodes() {
            if other.id == node.id {
                continue;
            }
            let overlap = other
                .tags
                .iter()
                .filter(|t| node_tags.contains(t.as_str()))
                .count();
            if overlap > 0 {
                tag_matches.push((&other.label, overlap));
            }
        }
        tag_matches.sort_by(|a, b| b.1.cmp(&a.1));
        tag_matches.truncate(8);

        if !tag_matches.is_empty() {
            right_lines.push(Line::from(""));
            right_lines.push(Line::from(Span::styled(
                "  TAG OVERLAP",
                Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
            )));
            right_lines.push(Line::from(Span::styled(
                "  ─────",
                Style::default().fg(Theme::MUTED),
            )));
            for (label, count) in &tag_matches {
                right_lines.push(Line::from(vec![
                    Span::styled(format!("    · {label}"), Style::default().fg(Theme::FG)),
                    Span::styled(
                        format!("  ({count} shared)"),
                        Style::default().fg(Theme::MUTED),
                    ),
                ]));
            }
        }
    }

    // Node ID at bottom
    right_lines.push(Line::from(""));
    let full_id = node.task_id.as_deref().unwrap_or(&node.id);
    right_lines.push(Line::from(Span::styled(
        format!("  ID: {full_id}"),
        Style::default().fg(Theme::MUTED),
    )));
    if let Some(ref path) = Some(&node.path) {
        right_lines.push(Line::from(Span::styled(
            format!("  Path: {}", path.display()),
            Style::default().fg(Theme::MUTED),
        )));
    }

    let right_text = Text::from(right_lines);
    let right_panel = Paragraph::new(right_text)
        .scroll((scroll as u16, 0))
        .block(Block::default().borders(Borders::NONE)); // already inside outer block with padding

    frame.render_widget(right_panel, panels[1]);
}
