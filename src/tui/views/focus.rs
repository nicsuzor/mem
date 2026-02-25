//! Focus View — "What should I do right now?"

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header
            Constraint::Min(1),   // task list
        ])
        .split(area);

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled("  NOW + NEXT", Style::default().fg(Color::White).bold()),
        Span::styled(
            format!("  ({} focus picks)", app.focus_picks.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(header, chunks[0]);

    // Focus picks
    if app.focus_picks.is_empty() {
        let msg = Paragraph::new("  No focus tasks. All clear!")
            .style(Style::default().fg(Color::Green));
        frame.render_widget(msg, chunks[1]);
        return;
    }

    let items: Vec<ListItem> = app
        .focus_picks
        .iter()
        .enumerate()
        .filter_map(|(idx, id)| {
            let node = gs.get_node(id)?;
            let selected = idx == app.selected_index;

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

            // Priority
            let exposure = if node.stakeholder_exposure { "!" } else { "" };
            spans.push(Span::styled(
                format!("P{pri}{exposure} "),
                Style::default().fg(pri_color).bold(),
            ));

            // Label
            let label_style = if pri <= 1 {
                Style::default().fg(Color::Red).bold()
            } else {
                Style::default().fg(Color::White)
            };
            spans.push(Span::styled(node.label.clone(), label_style));

            // Downstream weight
            if node.downstream_weight > 0.0 {
                spans.push(Span::styled(
                    format!("  wt:{:.1}", node.downstream_weight),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Staleness
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
                    spans.push(Span::styled(
                        format!("  {days}d"),
                        Style::default().fg(color),
                    ));
                }
            }

            // Enables annotation
            if let Some(ref parent_id) = node.parent {
                if let Some(parent) = gs.get_node(parent_id) {
                    if matches!(
                        parent.node_type.as_deref(),
                        Some("project") | Some("epic") | Some("goal")
                    ) {
                        spans.push(Span::styled(
                            format!("  → {}", parent.label),
                            Style::default().fg(Color::DarkGray).italic(),
                        ));
                    }
                }
            }

            let line = Line::from(spans);
            let style = if selected {
                Style::default().bg(Color::Rgb(30, 30, 50))
            } else {
                Style::default()
            };
            Some(ListItem::new(line).style(style))
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[1]);
}
