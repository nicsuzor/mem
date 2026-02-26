//! Node Detail overlay — shows full context for a selected node.
//!
//! Split panel layout: metadata/body on the left, graph context + PKB on the right.

use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;

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

    // Split into left and right panels
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(1),   // content
            Constraint::Length(1), // keybindings
        ])
        .split(overlay);

    // Title bar
    let tid = node.task_id.as_deref().unwrap_or(&node.id);
    let short_id = if tid.len() > 20 {
        format!("{}…", &tid[..20])
    } else {
        tid.to_string()
    };
    let title_bar = Paragraph::new(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(&node.label, Style::default().fg(Color::White).bold()),
        Span::styled(
            format!("  {short_id}"),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .style(Style::default().bg(Color::Rgb(30, 30, 60)));
    frame.render_widget(title_bar, inner[0]);

    // Bottom keybindings
    let keys = Paragraph::new(Line::from(vec![
        Span::styled(
            " Esc back │ j/k scroll ",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
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
        "active" | "in_progress" => Color::Green,
        "blocked" => Color::Red,
        "seed" => Color::DarkGray,
        "growing" => Color::Yellow,
        "dormant" => Color::Blue,
        "done" | "complete" => Color::DarkGray,
        _ => Color::White,
    };
    let pri = node.priority.unwrap_or(2);
    left_lines.push(Line::from(""));

    // Node type badge (prominent for non-task types)
    if let Some(ref ntype) = node.node_type {
        if ntype != "task" {
            let (icon, color) = match ntype.as_str() {
                "source" => ("📖", Color::Magenta),
                "note" | "knowledge" => ("📝", Color::Blue),
                "goal" => ("◉", Color::Yellow),
                "project" | "subproject" => ("◈", Color::Cyan),
                "epic" => ("◈", Color::Cyan),
                "memory" | "insight" => ("💡", Color::Yellow),
                _ => ("·", Color::White),
            };
            left_lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                Span::styled(ntype.clone(), Style::default().fg(color).bold()),
            ]));
        }
    }

    left_lines.push(Line::from(vec![
        Span::styled("  Status: ", Style::default().fg(Color::DarkGray)),
        Span::styled(status, Style::default().fg(status_color).bold()),
        Span::styled(format!("   P{pri}"), Style::default().fg(Color::White).bold()),
    ]));

    // Dates
    if let Some(ref created) = node.created {
        let mut spans = vec![
            Span::styled("  Created: ", Style::default().fg(Color::DarkGray)),
            Span::styled(created.clone(), Style::default().fg(Color::White)),
        ];
        // Staleness
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
            let days = (chrono::Local::now().date_naive() - dt).num_days();
            let color = if days > 30 {
                Color::Red
            } else if days > 14 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            spans.push(Span::styled(format!("  ({days}d)"), Style::default().fg(color)));
        }
        left_lines.push(Line::from(spans));
    }
    if let Some(ref due) = node.due {
        left_lines.push(Line::from(vec![
            Span::styled("  Due: ", Style::default().fg(Color::DarkGray)),
            Span::styled(due.clone(), Style::default().fg(Color::Yellow).bold()),
        ]));
    }

    // Project
    if let Some(ref project) = node.project {
        left_lines.push(Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Color::DarkGray)),
            Span::styled(project.clone(), Style::default().fg(Color::Cyan)),
        ]));
    }

    // Assignee
    if let Some(ref assignee) = node.assignee {
        left_lines.push(Line::from(vec![
            Span::styled("  Assignee: ", Style::default().fg(Color::DarkGray)),
            Span::styled(assignee.clone(), Style::default().fg(Color::White)),
        ]));
    }

    // Tags
    if !node.tags.is_empty() {
        left_lines.push(Line::from(vec![
            Span::styled("  Tags: ", Style::default().fg(Color::DarkGray)),
            Span::styled(node.tags.join(", "), Style::default().fg(Color::Green)),
        ]));
    }

    // Children / Subtasks
    if !node.children.is_empty() {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "  SUBTASKS",
            Style::default().fg(Color::Yellow).bold(),
        )));
        left_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));
        for child_id in &node.children {
            if let Some(child) = gs.get_node(child_id) {
                let (icon, color) = match child.status.as_deref() {
                    Some("done") | Some("complete") => ("✓", Color::Green),
                    Some("active") | Some("in_progress") => ("●", Color::Cyan),
                    Some("blocked") => ("✗", Color::Red),
                    Some("seed") => ("○", Color::DarkGray),
                    _ => ("○", Color::White),
                };
                left_lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} "), Style::default().fg(color)),
                    Span::styled(child.label.clone(), Style::default().fg(Color::White)),
                ]));
            }
        }
    }

    // Downstream weight
    if node.downstream_weight > 0.0 {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(vec![
            Span::styled("  Weight: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.1}", node.downstream_weight),
                Style::default().fg(Color::White),
            ),
            if node.stakeholder_exposure {
                Span::styled(" (stakeholder!)", Style::default().fg(Color::Yellow))
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
            Style::default().fg(Color::Yellow).bold(),
        )));
        left_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));
        for a in &node.assumptions {
            let (icon, color) = match a.status.as_str() {
                "confirmed" => ("✓", Color::Green),
                "invalidated" => ("✗", Color::Red),
                _ => ("?", Color::Yellow), // untested
            };
            left_lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                Span::styled(a.text.clone(), Style::default().fg(Color::White)),
                Span::styled(
                    format!("  [{}]", a.status),
                    Style::default().fg(color),
                ),
            ]));
        }
    }

    let scroll = app.detail_scroll.min(left_lines.len().saturating_sub(1));
    let left_text = Text::from(left_lines);
    let left_panel = Paragraph::new(left_text)
        .scroll((scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::LEFT | Borders::BOTTOM | Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(50, 50, 70))),
        );
    frame.render_widget(left_panel, panels[0]);

    // ── RIGHT PANEL: graph context ──
    let mut right_lines: Vec<Line> = Vec::new();

    right_lines.push(Line::from(""));
    right_lines.push(Line::from(Span::styled(
        "  GRAPH CONTEXT",
        Style::default().fg(Color::Yellow).bold(),
    )));
    right_lines.push(Line::from(Span::styled(
        "  ─────",
        Style::default().fg(Color::DarkGray),
    )));

    let graph_lines = crate::graph_display::render_ascii_graph(gs, node_id);
    for line in graph_lines {
        right_lines.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(Color::White),
        )));
    }

    // ── PKB CONNECTIONS ──

    // Backlinks (nodes that link TO this node via wikilinks/references)
    let backlinks = gs.backlinks_by_type(&node.id);
    if !backlinks.is_empty() {
        right_lines.push(Line::from(""));
        right_lines.push(Line::from(Span::styled(
            "  PKB BACKLINKS",
            Style::default().fg(Color::Yellow).bold(),
        )));
        right_lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));
        for (source_type, refs) in &backlinks {
            // Skip structural edges we already show above
            let has_link = refs.iter().any(|(_, et)| {
                matches!(et, mem::graph::EdgeType::Link)
            });
            if !has_link {
                continue;
            }
            right_lines.push(Line::from(Span::styled(
                format!("  [{source_type}]"),
                Style::default().fg(Color::DarkGray).italic(),
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
                    Style::default().fg(Color::White),
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
                Style::default().fg(Color::Yellow).bold(),
            )));
            right_lines.push(Line::from(Span::styled(
                "  ─────",
                Style::default().fg(Color::DarkGray),
            )));
            for (label, count) in &tag_matches {
                right_lines.push(Line::from(vec![
                    Span::styled(
                        format!("    · {label}"),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("  ({count} shared)"),
                        Style::default().fg(Color::DarkGray),
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
        Style::default().fg(Color::DarkGray),
    )));
    if let Some(ref path) = Some(&node.path) {
        right_lines.push(Line::from(Span::styled(
            format!("  Path: {}", path.display()),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let right_text = Text::from(right_lines);
    let right_panel = Paragraph::new(right_text)
        .scroll((scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::RIGHT | Borders::BOTTOM | Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(50, 50, 70))),
        );
    frame.render_widget(right_panel, panels[1]);
}
