//! Dashboard View — strategic overview with health metrics.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    let mut lines = Vec::new();

    // Header
    lines.push(Line::from(vec![
        Span::styled("  DASHBOARD", Style::default().fg(Color::White).bold()),
    ]));
    lines.push(Line::from(""));

    // Health stats
    let all_tasks = gs.all_tasks();
    let ready = gs.ready_tasks();
    let blocked = gs.blocked_tasks();

    let mut p1_count = 0;
    let mut p2_count = 0;
    let mut p3_count = 0;
    let mut oldest_days: i64 = 0;

    for task in &all_tasks {
        match task.priority.unwrap_or(2) {
            0 | 1 => p1_count += 1,
            2 => p2_count += 1,
            _ => p3_count += 1,
        }
        if let Some(ref created) = task.created {
            if let Ok(dt) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
                let days = (chrono::Local::now().date_naive() - dt).num_days();
                if days > oldest_days {
                    oldest_days = days;
                }
            }
        }
    }

    lines.push(Line::from(vec![
        Span::styled("  HEALTH", Style::default().fg(Color::Yellow).bold()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ─────", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {} total │ {} ready │ {} blocked", all_tasks.len(), ready.len(), blocked.len()),
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {} P0/P1 │ {} P2 │ {} P3+", p1_count, p2_count, p3_count),
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  Oldest task: {oldest_days}d"),
            Style::default().fg(if oldest_days > 30 { Color::Red } else { Color::Yellow }),
        ),
    ]));
    lines.push(Line::from(""));

    // By project
    let by_project = gs.by_project();
    lines.push(Line::from(Span::styled("  BY PROJECT", Style::default().fg(Color::Yellow).bold())));
    lines.push(Line::from(Span::styled("  ─────", Style::default().fg(Color::DarkGray))));

    let mut proj_list: Vec<(&String, &Vec<String>)> = by_project.iter().collect();
    proj_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (proj, ids) in proj_list {
        lines.push(Line::from(vec![
            Span::styled(format!("  ◈ {}", proj), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  ({})", ids.len()), Style::default().fg(Color::DarkGray)),
        ]));
    }
    lines.push(Line::from(""));

    // Assumptions health
    let (untested, confirmed, invalidated) = app.assumption_counts;
    let total_assumptions = untested + confirmed + invalidated;
    if total_assumptions > 0 {
        lines.push(Line::from(Span::styled(
            "  ASSUMPTIONS",
            Style::default().fg(Color::Yellow).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {total_assumptions} total │ "),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{untested} untested"),
                Style::default().fg(if untested > 0 { Color::Yellow } else { Color::Green }),
            ),
            Span::styled(" │ ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{confirmed} confirmed"),
                Style::default().fg(Color::Green),
            ),
            Span::styled(" │ ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{invalidated} invalidated"),
                Style::default().fg(if invalidated > 0 { Color::Red } else { Color::DarkGray }),
            ),
        ]));

        // Show top risky untested assumptions
        for (_node_id, text, weight) in app.untested_assumptions.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled("  ? ", Style::default().fg(Color::Yellow)),
                Span::styled(text.clone(), Style::default().fg(Color::White)),
                if *weight > 0.0 {
                    Span::styled(
                        format!("  (w:{weight:.0})"),
                        Style::default().fg(Color::DarkGray),
                    )
                } else {
                    Span::raw("")
                },
            ]));
        }
        lines.push(Line::from(""));
    }

    // Cross-project synergies
    if !app.synergies.is_empty() {
        lines.push(Line::from(Span::styled(
            "  SYNERGIES",
            Style::default().fg(Color::Yellow).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  ─────",
            Style::default().fg(Color::DarkGray),
        )));

        for (label_a, label_b, shared) in &app.synergies {
            lines.push(Line::from(vec![
                Span::styled("  ↔ ", Style::default().fg(Color::Magenta)),
                Span::styled(label_a.clone(), Style::default().fg(Color::White)),
                Span::styled(" ⟷ ", Style::default().fg(Color::DarkGray)),
                Span::styled(label_b.clone(), Style::default().fg(Color::White)),
                Span::styled(
                    format!("  ({shared} tags)"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Orphans
    if !app.orphans.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ○ {} orphan nodes (unlinked)", app.orphans.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .scroll((app.dashboard_scroll, 0));

    frame.render_widget(paragraph, area);
}
