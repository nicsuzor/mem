//! Dashboard View — strategic overview with health metrics.

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

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12), // Top row: Health & Assumptions
            Constraint::Min(10),    // Middle row: Projects & Synergies
            Constraint::Length(3),  // Bottom: Orphans
        ])
        .split(area);

    let top_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    let mid_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // --- Health Stats ---
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

    let health_text = vec![
        Line::from(vec![Span::styled(
            format!("Total Tasks:   {}", all_tasks.len()),
            Style::default().fg(Theme::FG),
        )]),
        Line::from(vec![Span::styled(
            format!("Ready:         {}", ready.len()),
            Style::default().fg(Theme::SUCCESS),
        )]),
        Line::from(vec![Span::styled(
            format!("Blocked:       {}", blocked.len()),
            Style::default().fg(Theme::ERROR),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("P0/P1 High:    {}", p1_count),
            Style::default().fg(Theme::ACCENT_PRIMARY),
        )]),
        Line::from(vec![Span::styled(
            format!("P2 Normal:     {}", p2_count),
            Style::default().fg(Theme::FG),
        )]),
        Line::from(vec![Span::styled(
            format!("P3 Low:        {}", p3_count),
            Style::default().fg(Theme::MUTED),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("Oldest Task:   {} days", oldest_days),
            if oldest_days > 30 {
                Style::default().fg(Theme::ERROR)
            } else {
                Style::default().fg(Theme::WARNING)
            },
        )]),
    ];

    let health_block = Theme::block().title(" Health ");
    frame.render_widget(Paragraph::new(health_text).block(health_block), top_row[0]);

    // --- Assumptions ---
    let (untested, confirmed, invalidated) = app.assumption_counts;
    let total_assumptions = untested + confirmed + invalidated;

    let mut assumption_lines = vec![
        Line::from(vec![Span::styled(
            format!("Total:         {}", total_assumptions),
            Style::default().fg(Theme::FG),
        )]),
        Line::from(vec![Span::styled(
            format!("Untested:      {}", untested),
            Style::default().fg(Theme::WARNING),
        )]),
        Line::from(vec![Span::styled(
            format!("Confirmed:     {}", confirmed),
            Style::default().fg(Theme::SUCCESS),
        )]),
        Line::from(vec![Span::styled(
            format!("Invalidated:   {}", invalidated),
            Style::default().fg(Theme::ERROR),
        )]),
        Line::from(""),
        Line::from(Span::styled(
            "Top Risky Assumptions:",
            Style::default().fg(Theme::MUTED).italic(),
        )),
    ];

    for (_node_id, text, weight) in app.untested_assumptions.iter().take(4) {
        assumption_lines.push(Line::from(vec![
            Span::styled(" ? ", Style::default().fg(Theme::WARNING)),
            Span::styled(text.clone(), Style::default().fg(Theme::FG)),
            if *weight > 0.0 {
                Span::styled(
                    format!(" (w:{:.0})", weight),
                    Style::default().fg(Theme::MUTED),
                )
            } else {
                Span::raw("")
            },
        ]));
    }

    let assumption_block = Theme::block().title(" Assumptions ");
    frame.render_widget(
        Paragraph::new(assumption_lines).block(assumption_block),
        top_row[1],
    );

    // --- By Project ---
    let by_project = gs.by_project();
    let mut proj_list: Vec<(&String, &Vec<String>)> = by_project.iter().collect();
    proj_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let mut proj_items = Vec::new();
    for (proj, ids) in proj_list.iter().take(15) {
        let count = ids.len();
        // Simple text bar
        let bar_len = (count as f32 / 5.0).ceil() as usize; // scale it down
        let bar = "█".repeat(bar_len.min(10));

        proj_items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!("{: <20} ", proj),
                Style::default().fg(Theme::ACCENT_SECONDARY),
            ),
            Span::styled(format!("{: <3} ", count), Style::default().fg(Theme::FG)),
            Span::styled(bar, Style::default().fg(Theme::MUTED)),
        ])));
    }

    let projects_block = Theme::block().title(" Active Projects ");
    let projects_list = List::new(proj_items).block(projects_block);
    frame.render_widget(projects_list, mid_row[0]);

    // --- Synergies ---
    let mut syn_items = Vec::new();
    for (label_a, label_b, shared) in &app.synergies {
        syn_items.push(ListItem::new(vec![
            Line::from(vec![
                Span::styled(" ↔ ", Style::default().fg(Theme::ACCENT_PRIMARY)),
                Span::styled(
                    format!("{} & {}", label_a, label_b),
                    Style::default().fg(Theme::FG),
                ),
            ]),
            Line::from(vec![Span::styled(
                format!("    Shared Tags: {}", shared),
                Style::default().fg(Theme::MUTED),
            )]),
        ]));
    }

    if syn_items.is_empty() {
        syn_items.push(ListItem::new(Line::from(Span::styled(
            "No synergies detected.",
            Style::default().fg(Theme::MUTED),
        ))));
    }

    let syn_block = Theme::block().title(" Synergies ");
    let syn_list = List::new(syn_items).block(syn_block);
    frame.render_widget(syn_list, mid_row[1]);

    // --- Orphans ---
    let orphans = gs.orphans();
    if !orphans.is_empty() {
        let text = Paragraph::new(Line::from(vec![
            Span::styled(" ○ ", Style::default().fg(Theme::WARNING)),
            Span::styled(
                format!("{} orphan nodes found (unlinked)", orphans.len()),
                Style::default().fg(Theme::WARNING),
            ),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(text, chunks[2]);
    }
}
