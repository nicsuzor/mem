//! Focus View — "What should I do right now?"
//!
//! Shows top focus picks with NOW/NEXT sections, enables annotations,
//! and an "orphan tasks" note at the bottom.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;
use crate::tui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    if app.focus_picks.is_empty() {
        let msg = Paragraph::new("  No focus tasks. All clear!")
            .style(Style::default().fg(Theme::SUCCESS).bold())
            .block(Theme::block().title(" Focus "))
            .alignment(Alignment::Center);
        frame.render_widget(msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10), // NOW box
            Constraint::Min(10),    // NEXT list
            Constraint::Length(8),  // Bottom info
        ])
        .split(area);

    // --- NOW Section ---
    if let Some(first_id) = app.focus_picks.first() {
        if let Some(node) = gs.get_node(first_id) {
            let reason = app.focus_reasons.get(first_id).map(|s| s.as_str());

            let mut lines = Vec::new();
            lines.push(Line::from(""));

            // Priority & Exposure
            let pri_str = format!("P{}", node.priority.unwrap_or(2));
            let exposure = if node.stakeholder_exposure {
                " !STAKEHOLDER! "
            } else {
                ""
            };
            lines.push(Line::from(vec![
                Span::styled(pri_str, Style::default().fg(Theme::ERROR).bold()),
                Span::styled(exposure, Style::default().fg(Theme::WARNING).bold()),
            ]));

            // Label (Big)
            lines.push(Line::from(Span::styled(
                node.label.clone(),
                Style::default()
                    .fg(Theme::ACCENT_SECONDARY)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            lines.push(Line::from(""));

            // Project context
            if let Some(ref proj) = node.project {
                lines.push(Line::from(vec![
                    Span::styled("Project: ", Style::default().fg(Theme::MUTED)),
                    Span::styled(proj, Style::default().fg(Theme::FG)),
                ]));
            }

            // Reason
            if let Some(r) = reason {
                lines.push(Line::from(vec![
                    Span::styled("Why: ", Style::default().fg(Theme::MUTED)),
                    Span::styled(r, Style::default().fg(Theme::ACCENT_PRIMARY).italic()),
                ]));
            }

            let block = if app.selected_index == 0 {
                Theme::active_block().title(" NOW ")
            } else {
                Theme::block().title(" NOW ")
            };

            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .alignment(Alignment::Center),
                chunks[0],
            );
        }
    }

    // --- NEXT Section ---
    if app.focus_picks.len() > 1 {
        let mut items = Vec::new();
        for (idx, id) in app.focus_picks.iter().enumerate().skip(1) {
            if let Some(node) = gs.get_node(id) {
                let selected = idx == app.selected_index;
                let reason = app.focus_reasons.get(id).map(|s| s.as_str());

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
                    format!("P{} ", node.priority.unwrap_or(2)),
                    Style::default().fg(Theme::MUTED),
                ));
                spans.push(Span::styled(
                    node.label.clone(),
                    Style::default().fg(Theme::FG),
                ));

                if let Some(r) = reason {
                    spans.push(Span::styled(
                        format!("  ({})", r),
                        Style::default().fg(Theme::MUTED).italic(),
                    ));
                }

                let style = if selected {
                    Style::default().bg(Theme::HIGHLIGHT_BG)
                } else {
                    Style::default()
                };
                items.push(ListItem::new(Line::from(spans)).style(style));
            }
        }

        let next_block = Theme::block().title(" Up Next ");
        frame.render_widget(List::new(items).block(next_block), chunks[1]);
    }

    // --- Bottom Info ---
    let mut bottom_lines = Vec::new();

    // Remaining count
    let remaining = app.ready_count.saturating_sub(app.focus_picks.len());
    if remaining > 0 {
        bottom_lines.push(Line::from(Span::styled(
            format!("... and {} more ready tasks not shown here.", remaining),
            Style::default().fg(Theme::MUTED),
        )));
    }

    // Assumptions
    if !app.untested_assumptions.is_empty() {
        bottom_lines.push(Line::from(""));
        bottom_lines.push(Line::from(Span::styled(
            "Untested Assumptions:",
            Style::default().fg(Theme::WARNING),
        )));
        for (_node_id, text, _) in app.untested_assumptions.iter().take(3) {
            bottom_lines.push(Line::from(vec![
                Span::styled(" ? ", Style::default().fg(Theme::WARNING)),
                Span::styled(text.clone(), Style::default().fg(Theme::FG)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(bottom_lines).block(Theme::dim_block()),
        chunks[2],
    );
}
