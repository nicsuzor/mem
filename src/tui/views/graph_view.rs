//! Graph View — goal→project→task hierarchy showing the planning web.
//!
//! Shows all nodes organized by their graph depth, with goals at root,
//! projects underneath, and tasks at the leaves. Includes orphan detection.

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

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(""));

    // Build the full hierarchy: goals → projects → tasks
    // Find all goal nodes
    let mut goals: Vec<&mem::graph::GraphNode> = gs
        .nodes()
        .filter(|n| n.node_type.as_deref() == Some("goal"))
        .filter(|n| !matches!(n.status.as_deref(), Some("done") | Some("dead")))
        .collect();
    goals.sort_by(|a, b| a.label.cmp(&b.label));

    for goal in &goals {
        // Goal line
        lines.push(Line::from(vec![
            Span::styled("  ◉ ", Style::default().fg(Color::Yellow).bold()),
            Span::styled(goal.label.clone(), Style::default().fg(Color::Yellow).bold()),
            Span::styled(
                format!("  [{}]", goal.status.as_deref().unwrap_or("goal")),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Find child projects/epics
        let mut children: Vec<&mem::graph::GraphNode> = goal
            .children
            .iter()
            .filter_map(|cid| gs.get_node(cid))
            .filter(|n| !matches!(n.status.as_deref(), Some("done") | Some("dead")))
            .collect();
        children.sort_by(|a, b| a.label.cmp(&b.label));

        for (pi, project) in children.iter().enumerate() {
            let proj_is_last = pi == children.len() - 1;
            let connector = if proj_is_last { "  └── " } else { "  ├── " };
            let cont = if proj_is_last { "      " } else { "  │   " };

            let icon = match project.node_type.as_deref() {
                Some("project") | Some("subproject") => "◈ ",
                Some("epic") => "◈ ",
                _ => "◇ ",
            };
            let color = Color::Cyan;

            lines.push(Line::from(vec![
                Span::styled(connector, Style::default().fg(Color::DarkGray)),
                Span::styled(icon, Style::default().fg(color)),
                Span::styled(project.label.clone(), Style::default().fg(color)),
                Span::styled(
                    format!("  [{}]", project.node_type.as_deref().unwrap_or("project")),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

            // Find child tasks
            let mut tasks: Vec<&mem::graph::GraphNode> = project
                .children
                .iter()
                .filter_map(|cid| gs.get_node(cid))
                .filter(|n| !matches!(n.status.as_deref(), Some("done") | Some("dead")))
                .collect();
            tasks.sort_by(|a, b| {
                a.priority
                    .unwrap_or(2)
                    .cmp(&b.priority.unwrap_or(2))
                    .then(a.label.cmp(&b.label))
            });

            for (ti, task) in tasks.iter().enumerate() {
                let task_is_last = ti == tasks.len() - 1;
                let task_connector = if task_is_last { "└── " } else { "├── " };

                let pri = task.priority.unwrap_or(2);
                let task_color = match pri {
                    0 | 1 => Color::Red,
                    2 => Color::White,
                    _ => Color::DarkGray,
                };

                let status_str = task.status.as_deref().unwrap_or("active");
                let status_style = match status_str {
                    "active" | "in_progress" => Style::default().fg(Color::Green),
                    "blocked" => Style::default().fg(Color::Red),
                    "seed" => Style::default().fg(Color::DarkGray),
                    "growing" => Style::default().fg(Color::Yellow),
                    _ => Style::default().fg(Color::DarkGray),
                };

                // Blocked annotation
                let blocked_annotation = if !task.depends_on.is_empty() {
                    let unmet: Vec<&str> = task
                        .depends_on
                        .iter()
                        .filter(|did| {
                            gs.get_node(did)
                                .map(|n| !matches!(n.status.as_deref(), Some("done")))
                                .unwrap_or(false)
                        })
                        .map(|_| "←")
                        .collect();
                    if !unmet.is_empty() {
                        format!(" ← ← ")
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{cont}{task_connector}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled("◇ ", Style::default().fg(task_color)),
                    Span::styled(task.label.clone(), Style::default().fg(task_color)),
                    if !blocked_annotation.is_empty() {
                        Span::styled(blocked_annotation, Style::default().fg(Color::Red))
                    } else {
                        Span::raw("")
                    },
                    Span::styled(
                        format!(" [{status_str}]"),
                        status_style,
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));
    }

    // Orphan detection
    let orphans = gs.orphans();
    let task_orphans: Vec<_> = orphans
        .iter()
        .filter(|n| {
            matches!(n.node_type.as_deref(), Some("task") | None)
                && !matches!(n.status.as_deref(), Some("done") | Some("dead"))
        })
        .collect();

    if !task_orphans.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ○ {} orphan tasks (unlinked to any goal)", task_orphans.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // Also show projects not under any goal
    let mut rootless_projects: Vec<&mem::graph::GraphNode> = gs
        .nodes()
        .filter(|n| {
            matches!(n.node_type.as_deref(), Some("project") | Some("epic"))
                && !matches!(n.status.as_deref(), Some("done") | Some("dead"))
                && n.parent.as_ref().map(|pid| {
                    gs.get_node(pid)
                        .map(|p| p.node_type.as_deref() != Some("goal"))
                        .unwrap_or(true)
                }).unwrap_or(true)
        })
        .collect();

    if !rootless_projects.is_empty() {
        rootless_projects.sort_by(|a, b| a.label.cmp(&b.label));
        lines.push(Line::from(vec![
            Span::styled(
                "  (projects not under a goal)",
                Style::default().fg(Color::DarkGray).italic(),
            ),
        ]));
        for proj in &rootless_projects {
            let child_count = proj.children.iter()
                .filter(|cid| {
                    gs.get_node(cid)
                        .map(|n| !matches!(n.status.as_deref(), Some("done") | Some("dead")))
                        .unwrap_or(false)
                })
                .count();
            lines.push(Line::from(vec![
                Span::styled("  ◈ ", Style::default().fg(Color::Blue)),
                Span::styled(proj.label.clone(), Style::default().fg(Color::Blue)),
                Span::styled(
                    format!(" ({child_count} tasks)"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    let scroll = app.scroll_offset.min(lines.len().saturating_sub(1));
    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).scroll((scroll as u16, 0));
    frame.render_widget(paragraph, area);
}
