//! Search overlay — fuzzy substring search across all graph nodes.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;
use crate::tui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    // Center the overlay
    let width = (area.width * 3 / 5)
        .max(60)
        .min(area.width.saturating_sub(4));
    let height = (area.height * 3 / 5)
        .max(15)
        .min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    // Draw main border
    frame.render_widget(Theme::active_block().title(" Search "), overlay);

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search input
            Constraint::Min(1),    // results
        ])
        .margin(1)
        .split(overlay);

    // Search input
    let input_text = format!(" > {}", app.search_query);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Theme::FG))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Theme::ACCENT_SECONDARY)),
        );

    frame.render_widget(input, inner[0]);

    // Cursor position (account for border and margin)
    // overlay.x + 1 (margin) + 1 (border) + 3 (" > ") + len
    let cursor_x = inner[0].x + 3 + app.search_query.len() as u16;
    let cursor_y = inner[0].y + 1;
    frame.set_cursor_position((cursor_x.min(inner[0].right().saturating_sub(1)), cursor_y));

    // Results
    let mut lines: Vec<Line> = Vec::new();

    if app.search_query.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Type to search tasks, notes, projects...",
            Style::default().fg(Theme::MUTED).italic(),
        )));
    } else if app.search_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matches found.",
            Style::default().fg(Theme::MUTED),
        )));
    } else {
        for (i, hit) in app.search_results.iter().enumerate() {
            let selected = i == app.search_selected;

            let icon = match hit.node_type.as_deref() {
                Some("goal") => "◉",
                Some("project") | Some("subproject") | Some("epic") => "◈",
                Some("source") => "📖",
                Some("note") | Some("knowledge") => "📝",
                _ => "◇",
            };

            let type_label = hit.node_type.as_deref().unwrap_or("task");

            let mut spans = Vec::new();

            if selected {
                spans.push(Span::styled(
                    "▸ ",
                    Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
                ));
            } else {
                spans.push(Span::raw("  "));
            }

            spans.push(Span::styled(
                format!("{icon} "),
                Style::default().fg(Theme::FG),
            ));
            spans.push(Span::styled(
                hit.label.clone(),
                if selected {
                    Style::default().fg(Theme::FG).bold()
                } else {
                    Style::default().fg(Theme::FG)
                },
            ));

            spans.push(Span::styled(
                format!("  [{type_label}]"),
                Style::default().fg(Theme::MUTED),
            ));

            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::default().bg(Theme::HIGHLIGHT_BG));
            }
            lines.push(line);
        }
    }

    let results = List::new(lines).block(Block::default().borders(Borders::NONE)); // No border for inner list
    frame.render_widget(results, inner[1]);
}
