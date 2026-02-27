//! Quick Capture overlay — create a new task inline.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, CaptureField};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let width = 50u16.min(area.width.saturating_sub(4));
    let height = 12u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title input
            Constraint::Length(3), // project selector
            Constraint::Length(3), // priority selector
            Constraint::Min(1),    // status / keybindings
        ])
        .split(overlay);

    // Title field
    let title_style = if app.capture_field == CaptureField::Title {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title_input = Paragraph::new(format!(" {}", app.capture_title))
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(title_style)
                .title(" Title ")
                .title_style(title_style),
        );
    frame.render_widget(title_input, inner[0]);

    if app.capture_field == CaptureField::Title {
        let cursor_x = inner[0].x + 2 + app.capture_title.len() as u16;
        let cursor_y = inner[0].y + 1;
        frame.set_cursor_position((cursor_x.min(inner[0].right().saturating_sub(2)), cursor_y));
    }

    // Project selector
    let proj_style = if app.capture_field == CaptureField::Project {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let proj_name = app
        .project_names
        .get(app.capture_project_idx)
        .map(|s| s.as_str())
        .unwrap_or("(none)");
    let proj_display = format!(" ◀ {proj_name} ▶");
    let project_input = Paragraph::new(proj_display)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(proj_style)
                .title(" Project ")
                .title_style(proj_style),
        );
    frame.render_widget(project_input, inner[1]);

    // Priority selector
    let pri_style = if app.capture_field == CaptureField::Priority {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let pri_color = match app.capture_priority {
        0 | 1 => Color::Red,
        2 => Color::White,
        _ => Color::DarkGray,
    };
    let pri_display = format!(" ◀ P{} ▶", app.capture_priority);
    let priority_input = Paragraph::new(pri_display)
        .style(Style::default().fg(pri_color))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pri_style)
                .title(" Priority ")
                .title_style(pri_style),
        );
    frame.render_widget(priority_input, inner[2]);

    // Status bar
    let keys = Paragraph::new(Line::from(vec![Span::styled(
        " Tab fields │ ←→ cycle │ Enter create │ Esc cancel",
        Style::default().fg(Color::DarkGray),
    )]));
    frame.render_widget(keys, inner[3]);
}
