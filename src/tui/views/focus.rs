//! Focus View — "What should I do right now?"

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

    if app.focus_picks.is_empty() && app.untested_assumptions.is_empty() && app.orphans.is_empty() {
        let msg = Paragraph::new("  No focus tasks. All clear!")
            .style(Style::default().fg(Color::Green));
        frame.render_widget(msg, area);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();

    // 1. FOCUS PICKS
    for (idx, node_id) in app.focus_picks.iter().enumerate() {
        let mut lines = Vec::new();

        // Header lines for NOW / NEXT
        if idx == 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  NOW", Style::default().fg(Color::White).bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  ────", Style::default().fg(Color::DarkGray)),
            ]));
        } else if idx == 1 {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  NEXT", Style::default().fg(Color::White).bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  ────", Style::default().fg(Color::DarkGray)),
            ]));
        }

        if let Some(node) = gs.get_node(node_id) {
            let reason = app.focus_reasons.get(node_id).map(|s| s.as_str());
            let is_selected = idx == app.selected_index;
            // Determine if we should show details (annotations)
            // Show details for the first item (NOW) OR the selected item
            let show_details = idx == 0 || is_selected;

            // Build the main line and detail lines
            let item_lines = build_focus_item_lines(node, gs, is_selected, idx == 0, reason, show_details);
            lines.extend(item_lines);
        }

        items.push(ListItem::new(lines));
    }

    // 2. UNTESTED ASSUMPTIONS
    for (i, (node_id, text, weight)) in app.untested_assumptions.iter().enumerate() {
        let mut lines = Vec::new();

        // Header on first item
        if i == 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  UNTESTED ASSUMPTIONS", Style::default().fg(Color::Yellow).bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  ────", Style::default().fg(Color::DarkGray)),
            ]));
        }

        let label = gs.get_node(node_id).map(|n| n.label.as_str()).unwrap_or("?");

        // Selection check
        let list_idx = app.focus_picks.len() + i;
        let is_selected = list_idx == app.selected_index;

        let marker = if is_selected { "▸ " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(format!("  {}? ", marker), Style::default().fg(Color::Yellow)),
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

        items.push(ListItem::new(lines));
    }

    // 3. ORPHANS
    if !app.orphans.is_empty() {
        for (i, node_id) in app.orphans.iter().enumerate() {
            let mut lines = Vec::new();

            // Header on first item
            if i == 0 {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  ○ ORPHANS ({})", app.orphans.len()),
                        Style::default().fg(Color::Yellow).bold()
                    ),
                ]));
                 lines.push(Line::from(vec![
                    Span::styled("  ────", Style::default().fg(Color::DarkGray)),
                ]));
            }

            let list_idx = app.focus_picks.len() + app.untested_assumptions.len() + i;
            let is_selected = list_idx == app.selected_index;
            let marker = if is_selected { "▸ " } else { "  " };

            if let Some(node) = gs.get_node(node_id) {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}{}", marker, node.label), Style::default().fg(Color::Yellow)),
                ]));
            } else {
                 lines.push(Line::from(vec![
                    Span::styled(format!("  {}{}", marker, node_id), Style::default().fg(Color::Yellow)),
                ]));
            }
            items.push(ListItem::new(lines));
        }
    }

    // Render List
    let list = List::new(items)
        .highlight_style(Style::default()) // We handle highlighting manually
        .highlight_symbol(""); // We handle symbol manually

    // Calculate state
    let mut state = ListState::default();
    state.select(Some(app.selected_index));

    frame.render_stateful_widget(list, area, &mut state);
}

// Helper to build lines for a focus item
fn build_focus_item_lines<'a>(
    node: &'a mem::graph::GraphNode,
    gs: &'a mem::graph_store::GraphStore,
    selected: bool,
    is_now: bool,
    reason: Option<&str>,
    show_details: bool,
) -> Vec<Line<'a>> {
    let pri = node.priority.unwrap_or(2);
    let pri_color = match pri {
        0 | 1 => Color::Red,
        2 => Color::White,
        _ => Color::DarkGray,
    };

    let mut lines = Vec::new();
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

    if show_details {
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

    lines
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
