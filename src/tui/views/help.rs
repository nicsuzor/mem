//! Help overlay showing keybindings.

use crate::tui::theme::Theme;
use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn render(frame: &mut Frame, area: Rect) {
    let margin_h = (area.width / 6).max(4);
    let margin_v = (area.height / 6).max(2);

    let x = area.x + margin_h;
    let y = area.y + margin_v;
    let width = area.width.saturating_sub(margin_h * 2);
    let height = area.height.saturating_sub(margin_v * 2);

    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let help_text = vec![
        Line::from(Span::styled(
            "  Navigation",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        )),
        Line::from(""),
        Line::from("  Tab / Shift-Tab    Cycle views"),
        Line::from("  f g t d            Jump to Focus / Graph / Tree / Dashboard"),
        Line::from("  j/k or ↑/↓         Navigate items"),
        Line::from("  h/l or ←/→         Collapse / Expand"),
        Line::from("  Space              Toggle expand/collapse"),
        Line::from("  Enter              Open node detail"),
        Line::from("  Esc / ←            Close detail / help"),
        Line::from(""),
        Line::from(Span::styled(
            "  Filters & Priority",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        )),
        Line::from(""),
        Line::from("  1 2 3              Filter by priority (toggle)"),
        Line::from("  + / -              Increase / Decrease priority"),
        Line::from(""),
        Line::from(Span::styled(
            "  General",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        )),
        Line::from(""),
        Line::from("  ?                  Toggle this help"),
        Line::from("  q                  Quick capture"),
        Line::from("  Ctrl+C / Ctrl+D    Quit"),
    ];

    let help = Paragraph::new(help_text)
        .block(Theme::block().title(" Help (? to close) "))
        .style(Style::default().fg(Theme::FG));

    frame.render_widget(help, overlay);
}
