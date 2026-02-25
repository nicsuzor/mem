//! Graph View — goal→project→task hierarchy.
//!
//! For Phase 0, this reuses the same tree as the Epic Tree view but
//! with a different header. The full graph layout comes in Phase 1.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("  GRAPH", Style::default().fg(Color::White).bold()),
        Span::styled("  planning web", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(header, chunks[0]);

    // Reuse epic tree rendering for now
    super::epic_tree::render(frame, app, chunks[1]);
}
