//! Focus View — "What should I do right now?"
//!
//! Shows top focus picks with NOW/NEXT sections, enables annotations,
//! and an "orphan tasks" note at the bottom.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    if app.focus_picks.is_empty() {
        let msg = Paragraph::new("  No focus tasks. All clear!")
            .style(Style::default().fg(Color::Green));
        frame.render_widget(msg, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // NOW section (first pick)
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  NOW", Style::default().fg(Color::White).bold()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ────", Style::default().fg(Color::DarkGray)),
    ]));

    if let Some(first_id) = app.focus_picks.first() {
        if let Some(node) = gs.get_node(first_id) {
            let selected = app.selected_index == 0;
            let reason = app.focus_reasons.get(first_id).map(|s| s.as_str());
            render_focus_item(&mut lines, node, gs, selected, true, reason);
        }
    }

    // NEXT section (remaining picks)
    if app.focus_picks.len() > 1 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  NEXT", Style::default().fg(Color::White).bold()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ────", Style::default().fg(Color::DarkGray)),
        ]));

        for (idx, id) in app.focus_picks.iter().enumerate().skip(1) {
            if let Some(node) = gs.get_node(id) {
                let selected = idx == app.selected_index;
                let reason = app.focus_reasons.get(id).map(|s| s.as_str());
                render_focus_item(&mut lines, node, gs, selected, false, reason);
            }
        }
    }

    // Remaining task count
    let remaining = app.ready_count.saturating_sub(app.focus_picks.len());
    if remaining > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ─── {remaining} more: Tab to Epic Tree ───"),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    // Orphan count
    if let Some(ref gs) = app.graph {
        let orphans = gs.orphans();
        let task_orphans: Vec<_> = orphans
            .iter()
            .filter(|n| {
                n.node_type.as_deref() == Some("task")
                    && !matches!(n.status.as_deref(), Some("done") | Some("dead"))
            })
            .collect();
        if !task_orphans.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  ○ {} orphan tasks (unlinked to any goal)", task_orphans.len()),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
    }

    // Untested assumptions (sorted by downstream weight)
    if !app.untested_assumptions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  UNTESTED ASSUMPTIONS", Style::default().fg(Color::Yellow).bold()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ────", Style::default().fg(Color::DarkGray)),
        ]));
        for (node_id, text, weight) in app.untested_assumptions.iter().take(5) {
            let label = app
                .graph
                .as_ref()
                .and_then(|gs| gs.get_node(node_id))
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            lines.push(Line::from(vec![
                Span::styled("    ? ", Style::default().fg(Color::Yellow)),
                Span::styled(text.clone(), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("      ↳ {label}"),
                    Style::default().fg(Color::DarkGray).italic(),
                ),
                if *weight > 0.0 {
                    Span::styled(
                        format!("  (weight: {weight:.1})"),
                        Style::default().fg(Color::DarkGray),
                    )
                } else {
                    Span::raw("")
                },
            ]));
        }
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, area);
}

fn render_focus_item(
    lines: &mut Vec<Line<'_>>,
    node: &mem::graph::GraphNode,
    gs: &mem::graph_store::GraphStore,
    selected: bool,
    is_now: bool,
    reason: Option<&str>,
) {
    let pri = node.priority.unwrap_or(2);
    let pri_color = match pri {
        0 | 1 => Color::Red,
        2 => Color::White,
        _ => Color::DarkGray,
    };

    let mut spans = Vec::new();

    // Selection indicator
    if selected {
        spans.push(Span::styled("  ▸ ", Style::default().fg(Color::Cyan).bold()));
    } else {
        spans.push(Span::raw("    "));
    }

    // Priority badge
    let exposure = if node.stakeholder_exposure { "!" } else { "" };
    if pri <= 1 || is_now {
        spans.push(Span::styled(
            format!("P{pri}{exposure} "),
            Style::default().fg(pri_color).bold(),
        ));
    } else {
        // Type icon for NEXT items
        let icon = infer_type_icon(&node.label);
        if !icon.is_empty() {
            spans.push(Span::styled(format!("{icon}  "), Style::default().fg(Color::Gray)));
        }
    }

    // Label
    let label_style = if pri <= 1 || is_now {
        Style::default().fg(Color::White).bold()
    } else {
        Style::default().fg(Color::White)
    };
    spans.push(Span::styled(node.label.clone(), label_style));

    // Staleness badge
    if let Some(ref created) = node.created {
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
            let days = (chrono::Local::now().date_naive() - dt).num_days();
            let color = if days > 30 {
                Color::Red
            } else if days > 14 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            spans.push(Span::styled(format!("  {days}d"), Style::default().fg(color)));
        }
    }

    // Due date badge
    if node.due.is_some() {
        spans.push(Span::styled(" ⏰", Style::default().fg(Color::Yellow)));
    }

    let mut main_line = Line::from(spans);
    if selected {
        main_line = main_line.style(Style::default().bg(Color::Rgb(30, 30, 50)));
    }
    lines.push(main_line);

    // Sub-annotations for NOW item (or all if expanded)
    if is_now || selected {
        // Enables annotation
        if let Some(ref parent_id) = node.parent {
            if let Some(parent) = gs.get_node(parent_id) {
                if matches!(
                    parent.node_type.as_deref(),
                    Some("project") | Some("epic") | Some("goal")
                ) {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("       → enables: {}", parent.label),
                            Style::default().fg(Color::DarkGray).italic(),
                        ),
                    ]));
                }
            }
        }

        // Blocks annotation
        if !node.blocks.is_empty() {
            let blocked_labels: Vec<String> = node
                .blocks
                .iter()
                .take(3)
                .filter_map(|bid| gs.get_node(bid).map(|n| n.label.clone()))
                .collect();
            if !blocked_labels.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("       → unblocks: {}", blocked_labels.join(", ")),
                        Style::default().fg(Color::DarkGray).italic(),
                    ),
                ]));
            }
        }

        // Reason annotation (why this task was picked)
        if let Some(reason) = reason {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("       ∵ {reason}"),
                    Style::default().fg(Color::Rgb(100, 100, 140)).italic(),
                ),
            ]));
        }
    }
}

/// Infer a type icon from the task title.
fn infer_type_icon(title: &str) -> &'static str {
    let lower = title.to_lowercase();
    if lower.starts_with("decide:") || lower.starts_with("decision:") {
        "⚖"
    } else if lower.starts_with("reply to") || lower.starts_with("email:") || lower.starts_with("respond to") {
        "✉"
    } else if lower.starts_with("call ") || lower.starts_with("phone:") {
        "📞"
    } else if lower.starts_with("write ") || lower.starts_with("draft ") {
        "✏"
    } else if lower.starts_with("research:") || lower.starts_with("investigate") {
        "🔬"
    } else if lower.starts_with("confirm") || lower.starts_with("rsvp") {
        "✉"
    } else {
        ""
    }
}
