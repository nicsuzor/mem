//! Top status bar showing view tabs and summary stats.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, View};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    for (i, view) in View::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }

        let style = if *view == app.current_view {
            Style::default().fg(Color::White).bg(Color::DarkGray).bold()
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {} ", view.label()), style));
    }

    // Active filter/mode indicators
    if app.reparent_mode {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            " REPARENT ",
            Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        ));
        if let Some(ref nid) = app.reparent_node_id {
            let label = app
                .graph
                .as_ref()
                .and_then(|gs| gs.get_node(nid))
                .map(|n| {
                    if n.label.len() > 20 {
                        let end = n.label.floor_char_boundary(20);
                        format!("{}...", &n.label[..end])
                    } else {
                        n.label.clone()
                    }
                })
                .unwrap_or_default();
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(Color::Yellow),
            ));
        }
    }

    if let Some(pri) = app.priority_filter {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            format!(" P{pri} "),
            Style::default()
                .fg(Color::Black)
                .bg(match pri {
                    0 | 1 => Color::Red,
                    2 => Color::White,
                    _ => Color::DarkGray,
                })
                .bold(),
        ));
    }

    if app.show_completed {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            " +done ",
            Style::default().fg(Color::Black).bg(Color::Green),
        ));
    }

    if let Some(ref tf) = app.type_filter {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            format!(" {tf} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ));
    }

    // Stats on the right
    let stats = format!(
        "  {} tasks │ {} ready │ {} blocked │ {} projects",
        app.total_tasks, app.ready_count, app.blocked_count, app.project_count
    );
    spans.push(Span::styled("  ", Style::default()));
    spans.push(Span::styled(stats, Style::default().fg(Color::DarkGray)));

    let line = Line::from(spans);
    let bar = Paragraph::new(line);
    frame.render_widget(bar, area);
}
