//! Dashboard View — strategic overview with health metrics.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Length(8),  // health stats
            Constraint::Length(8),  // by project
            Constraint::Min(1),    // rest
        ])
        .split(area);

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled("  DASHBOARD", Style::default().fg(Color::White).bold()),
    ]));
    frame.render_widget(header, chunks[0]);

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

    let health_lines = vec![
        Line::from(vec![
            Span::styled("  HEALTH", Style::default().fg(Color::Yellow).bold()),
        ]),
        Line::from(vec![
            Span::styled("  ─────", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("  {} total │ {} ready │ {} blocked", all_tasks.len(), ready.len(), blocked.len()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("  {} P0/P1 │ {} P2 │ {} P3+", p1_count, p2_count, p3_count),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("  Oldest task: {oldest_days}d"),
                Style::default().fg(if oldest_days > 30 { Color::Red } else { Color::Yellow }),
            ),
        ]),
    ];
    let health = Paragraph::new(health_lines);
    frame.render_widget(health, chunks[1]);

    // By project
    let by_project = gs.by_project();
    let mut proj_lines = vec![
        Line::from(Span::styled("  BY PROJECT", Style::default().fg(Color::Yellow).bold())),
        Line::from(Span::styled("  ─────", Style::default().fg(Color::DarkGray))),
    ];
    let mut proj_list: Vec<(&String, &Vec<String>)> = by_project.iter().collect();
    proj_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (proj, ids) in proj_list.iter().take(8) {
        proj_lines.push(Line::from(vec![
            Span::styled(format!("  ◈ {}", proj), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  ({})", ids.len()), Style::default().fg(Color::DarkGray)),
        ]));
    }
    let projects = Paragraph::new(proj_lines);
    frame.render_widget(projects, chunks[2]);

    // Orphans
    let orphans = gs.orphans();
    if !orphans.is_empty() {
        let orphan_text = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  ○ {} orphan nodes (unlinked)", orphans.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]));
        frame.render_widget(orphan_text, chunks[3]);
    }
}
