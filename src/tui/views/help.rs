//! Help overlay showing keybindings.

use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn render(frame: &mut Frame, area: Rect) {
    let margin_h = (area.width / 8).max(4);
    let margin_v = (area.height / 10).max(1);
    let overlay = Rect::new(
        area.x + margin_h,
        area.y + margin_v,
        area.width.saturating_sub(margin_h * 2),
        area.height.saturating_sub(margin_v * 2),
    );

    frame.render_widget(Clear, overlay);

    let help_text = vec![
        Line::from(Span::styled(
            "  Navigation",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab / Shift-Tab    ", Style::default().fg(Color::Cyan)),
            Span::raw("Cycle views"),
        ]),
        Line::from(vec![
            Span::styled("  f g t d            ", Style::default().fg(Color::Cyan)),
            Span::raw("Jump to Focus / Graph / Tree / Dashboard"),
        ]),
        Line::from(vec![
            Span::styled("  j/k or ↑/↓         ", Style::default().fg(Color::Cyan)),
            Span::raw("Navigate items"),
        ]),
        Line::from(vec![
            Span::styled("  h/l or ←/→         ", Style::default().fg(Color::Cyan)),
            Span::raw("Collapse / Expand"),
        ]),
        Line::from(vec![
            Span::styled("  Space              ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle expand/collapse"),
        ]),
        Line::from(vec![
            Span::styled("  Enter              ", Style::default().fg(Color::Cyan)),
            Span::raw("Open detail (or confirm reparent)"),
        ]),
        Line::from(vec![
            Span::styled("  Esc / ←            ", Style::default().fg(Color::Cyan)),
            Span::raw("Close detail / overlay / cancel"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Filters",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  1 2 3              ", Style::default().fg(Color::Cyan)),
            Span::raw("Filter by max priority (toggle)"),
        ]),
        Line::from(vec![
            Span::styled("  C                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle show completed tasks"),
        ]),
        Line::from(vec![
            Span::styled("  T                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Cycle type filter"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Actions",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  s                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Cycle status (active → done → blocked → dead)"),
        ]),
        Line::from(vec![
            Span::styled("  p                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Cycle priority (0 → 1 → 2 → 3)"),
        ]),
        Line::from(vec![
            Span::styled("  + / -              ", Style::default().fg(Color::Cyan)),
            Span::raw("Raise / Lower priority"),
        ]),
        Line::from(vec![
            Span::styled("  r                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Reparent: mark node, navigate, Enter to place"),
        ]),
        Line::from(vec![
            Span::styled("  n or q             ", Style::default().fg(Color::Cyan)),
            Span::raw("Quick capture new task"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  General",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  /                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Search tasks, notes, projects"),
        ]),
        Line::from(vec![
            Span::styled("  ?                  ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+C / Ctrl+D    ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ]),
    ];

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
