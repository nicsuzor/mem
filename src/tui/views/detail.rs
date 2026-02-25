//! Node Detail overlay — shows full context for a selected node.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

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
    let margin_h = (area.width / 8).max(2);
    let margin_v = (area.height / 8).max(1);
    let overlay = Rect::new(
        area.x + margin_h,
        area.y + margin_v,
        area.width.saturating_sub(margin_h * 2),
        area.height.saturating_sub(margin_v * 2),
    );

    // Clear background
    frame.render_widget(Clear, overlay);

    // Build detail content
    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(vec![
        Span::styled(&node.label, Style::default().fg(Color::White).bold()),
    ]));
    lines.push(Line::from(""));

    // Status + Priority
    let status = node.status.as_deref().unwrap_or("unknown");
    let status_color = match status {
        "active" | "in_progress" => Color::Green,
        "blocked" => Color::Red,
        "seed" => Color::DarkGray,
        "growing" => Color::Yellow,
        "dormant" => Color::Blue,
        "done" | "complete" => Color::DarkGray,
        _ => Color::White,
    };
    let pri = node.priority.unwrap_or(2);
    lines.push(Line::from(vec![
        Span::styled("  Status: ", Style::default().fg(Color::DarkGray)),
        Span::styled(status, Style::default().fg(status_color)),
        Span::styled(format!("   P{pri}"), Style::default().fg(Color::White).bold()),
    ]));

    // Dates
    if let Some(ref created) = node.created {
        lines.push(Line::from(vec![
            Span::styled("  Created: ", Style::default().fg(Color::DarkGray)),
            Span::styled(created.clone(), Style::default().fg(Color::White)),
        ]));
    }
    if let Some(ref due) = node.due {
        lines.push(Line::from(vec![
            Span::styled("  Due: ", Style::default().fg(Color::DarkGray)),
            Span::styled(due.clone(), Style::default().fg(Color::Yellow)),
        ]));
    }

    // Project
    if let Some(ref project) = node.project {
        lines.push(Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Color::DarkGray)),
            Span::styled(project.clone(), Style::default().fg(Color::Cyan)),
        ]));
    }

    lines.push(Line::from(""));

    // Graph context — parent chain
    lines.push(Line::from(Span::styled(
        "  GRAPH CONTEXT",
        Style::default().fg(Color::Yellow).bold(),
    )));
    lines.push(Line::from(Span::styled(
        "  ─────",
        Style::default().fg(Color::DarkGray),
    )));

    // Walk up parents
    let mut parent_id = node.parent.as_deref();
    let mut parent_chain = Vec::new();
    while let Some(pid) = parent_id {
        if let Some(parent) = gs.get_node(pid) {
            let icon = match parent.node_type.as_deref() {
                Some("goal") => "◉",
                Some("project") | Some("subproject") => "◈",
                Some("epic") => "██",
                _ => "◇",
            };
            parent_chain.push(format!("{icon} {}", parent.label));
            parent_id = parent.parent.as_deref();
        } else {
            break;
        }
    }
    parent_chain.reverse();
    if parent_chain.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (orphan — no parent)",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  ↑ enables:",
            Style::default().fg(Color::DarkGray),
        )));
        for p in &parent_chain {
            lines.push(Line::from(Span::styled(
                format!("    {p}"),
                Style::default().fg(Color::White),
            )));
        }
    }

    // Dependencies (blocks / blocked by)
    if !node.depends_on.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ↓ depends on:",
            Style::default().fg(Color::DarkGray),
        )));
        for dep_id in &node.depends_on {
            let label = gs
                .get_node(dep_id)
                .map(|n| n.label.as_str())
                .unwrap_or(dep_id);
            lines.push(Line::from(Span::styled(
                format!("    ◇ {label}"),
                Style::default().fg(Color::Red),
            )));
        }
    }

    if !node.blocks.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  → blocks:",
            Style::default().fg(Color::DarkGray),
        )));
        for block_id in &node.blocks {
            let label = gs
                .get_node(block_id)
                .map(|n| n.label.as_str())
                .unwrap_or(block_id);
            lines.push(Line::from(Span::styled(
                format!("    ◇ {label}"),
                Style::default().fg(Color::White),
            )));
        }
    }

    // Children
    if !node.children.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  CHILDREN",
            Style::default().fg(Color::Yellow).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));
        for child_id in &node.children {
            if let Some(child) = gs.get_node(child_id) {
                let status_icon = match child.status.as_deref() {
                    Some("done") | Some("complete") => "✓",
                    Some("active") | Some("in_progress") => "●",
                    Some("blocked") => "✗",
                    _ => "○",
                };
                lines.push(Line::from(Span::styled(
                    format!("  {status_icon} {}", child.label),
                    Style::default().fg(Color::White),
                )));
            }
        }
    }

    // Task ID
    lines.push(Line::from(""));
    let tid = node.task_id.as_deref().unwrap_or(&node.id);
    lines.push(Line::from(Span::styled(
        format!("  ID: {tid}"),
        Style::default().fg(Color::DarkGray),
    )));

    // Scrollable text
    let scroll = app.detail_scroll.min(lines.len().saturating_sub(1));
    let text = Text::from(lines);

    let detail = Paragraph::new(text)
        .scroll((scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " Node Detail (Esc to close) ",
                    Style::default().fg(Color::Cyan).bold(),
                )),
        );

    frame.render_widget(detail, overlay);
}
