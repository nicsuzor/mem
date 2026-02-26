//! Help overlay showing keybindings.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, View};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let margin_h = (area.width / 6).max(4);
    let margin_v = (area.height / 6).max(2);
    let overlay = Rect::new(
        area.x + margin_h,
        area.y + margin_v,
        area.width.saturating_sub(margin_h * 2),
        area.height.saturating_sub(margin_v * 2),
    );

    frame.render_widget(Clear, overlay);

    let mut help_text = vec![
        Line::from(Span::styled("  Navigation", Style::default().fg(Color::Yellow).bold())),
        Line::from(""),
        Line::from("  Tab / Shift-Tab    Cycle views"),
        Line::from("  f g t d            Jump to Focus / Graph / Tree / Dashboard"),
    ];

    match app.current_view {
        View::EpicTree => {
            help_text.push(Line::from("  j/k or ↑/↓         Navigate items"));
            help_text.push(Line::from("  h/l or ←/→         Collapse / Expand"));
            help_text.push(Line::from("  Space              Toggle expand/collapse"));
            help_text.push(Line::from("  Enter              Open node detail"));
        }
        View::Graph => {
            help_text.push(Line::from("  j/k or ↑/↓         Navigate items"));
            help_text.push(Line::from("  h/l or ←/→         Collapse / Expand"));
            help_text.push(Line::from("  Space              Toggle expand/collapse"));
            help_text.push(Line::from("  Enter              Open node detail"));
        }
        View::Focus => {
            help_text.push(Line::from("  j/k or ↑/↓         Navigate tasks / info"));
            help_text.push(Line::from("  Enter              Open detail (for tasks)"));
        }
        View::Dashboard => {
            help_text.push(Line::from("  j/k or ↑/↓         Scroll dashboard"));
        }
    }

    help_text.push(Line::from(""));
    help_text.push(Line::from(Span::styled("  Actions", Style::default().fg(Color::Yellow).bold())));
    help_text.push(Line::from(""));
    help_text.push(Line::from("  /                  Search"));
    help_text.push(Line::from("  n                  New Task (Quick Capture)"));

    if matches!(app.current_view, View::EpicTree | View::Graph | View::Focus) {
         help_text.push(Line::from("  s                  Toggle status (active/done/blocked)"));
         help_text.push(Line::from("  p                  Cycle priority"));
         help_text.push(Line::from("  + / -              Increase / Decrease priority"));
         help_text.push(Line::from("  r                  Reparent (move) node"));
    }

    if matches!(app.current_view, View::EpicTree) {
         help_text.push(Line::from("  1 2 3              Filter by priority"));
         help_text.push(Line::from("  C                  Toggle completed"));
         help_text.push(Line::from("  T                  Cycle type filter"));
    }

    help_text.push(Line::from(""));
    help_text.push(Line::from(Span::styled("  General", Style::default().fg(Color::Yellow).bold())));
    help_text.push(Line::from(""));
    help_text.push(Line::from("  ?                  Toggle this help"));
    help_text.push(Line::from("  Esc                Close overlay"));
    help_text.push(Line::from("  Ctrl+C / Ctrl+D    Quit"));

    let help = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Help (? to close) ",
                Style::default().fg(Color::Cyan).bold(),
            )),
    );

    frame.render_widget(help, overlay);
}
