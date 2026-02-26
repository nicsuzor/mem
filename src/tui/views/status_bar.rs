//! Top status bar showing view tabs and summary stats.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, View};
use crate::tui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(60)])
        .split(area);

    // Tabs
    let mut tab_spans = Vec::new();
    for (i, view) in View::ALL.iter().enumerate() {
        if i > 0 {
            tab_spans.push(Span::styled(" ", Style::default().fg(Theme::MUTED)));
        }

        let style = if *view == app.current_view {
            Style::default()
                .fg(Theme::BG)
                .bg(Theme::ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Theme::MUTED)
        };
        tab_spans.push(Span::styled(format!(" {} ", view.label()), style));
    }

    let tabs = Paragraph::new(Line::from(tab_spans))
        .alignment(Alignment::Left)
        .style(Style::default().bg(Theme::BG));
    frame.render_widget(tabs, chunks[0]);

    // Stats
    let stats = vec![
        Span::styled(
            format!(" {} total ", app.total_tasks),
            Style::default().fg(Theme::FG),
        ),
        Span::styled("│", Style::default().fg(Theme::MUTED)),
        Span::styled(
            format!(" {} ready ", app.ready_count),
            Style::default().fg(Theme::SUCCESS),
        ),
        Span::styled("│", Style::default().fg(Theme::MUTED)),
        Span::styled(
            format!(" {} blocked ", app.blocked_count),
            Style::default().fg(Theme::ERROR),
        ),
        Span::styled("│", Style::default().fg(Theme::MUTED)),
        Span::styled(
            format!(" {} projects ", app.project_count),
            Style::default().fg(Theme::ACCENT_SECONDARY),
        ),
    ];

    let stats_bar = Paragraph::new(Line::from(stats))
        .alignment(Alignment::Right)
        .style(Style::default().bg(Theme::BG));
    frame.render_widget(stats_bar, chunks[1]);
}
