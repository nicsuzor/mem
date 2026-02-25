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
