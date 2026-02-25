//! Search overlay — fuzzy substring search across all graph nodes.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    // Center the overlay
    let width = (area.width * 3 / 5).max(40).min(area.width.saturating_sub(4));
    let height = (area.height * 3 / 5).max(10).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search input
            Constraint::Min(1),   // results
        ])
        .split(overlay);

    // Search input
    let input_text = format!(" / {}", app.search_query);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Search ")
                .title_style(Style::default().fg(Color::Cyan).bold()),
        );
    frame.render_widget(input, inner[0]);

    // Cursor position
    let cursor_x = inner[0].x + 4 + app.search_query.len() as u16;
    let cursor_y = inner[0].y + 1;
    frame.set_cursor_position((cursor_x.min(inner[0].right().saturating_sub(2)), cursor_y));

    // Results
    let mut lines: Vec<Line> = Vec::new();

    if app.search_query.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Type to search tasks, notes, projects...",
            Style::default().fg(Color::DarkGray).italic(),
        )));
    } else if app.search_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matches",
            Style::default().fg(Color::DarkGray),
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

            let spans = vec![
                if selected {
                    Span::styled("  ▸ ", Style::default().fg(Color::Cyan).bold())
                } else {
                    Span::raw("    ")
                },
                Span::styled(
                    format!("{icon} "),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    hit.label.clone(),
                    if selected {
                        Style::default().fg(Color::White).bold()
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::styled(
                    format!("  [{type_label}]"),
                    Style::default().fg(Color::DarkGray),
                ),
            ];

            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::default().bg(Color::Rgb(30, 30, 50)));
            }
            lines.push(line);
        }
    }

    let results = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(50, 50, 70))),
    );
    frame.render_widget(results, inner[1]);
}
