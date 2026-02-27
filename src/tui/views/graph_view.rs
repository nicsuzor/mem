//! Graph View — Local Context Visualization.
//!
//! Shows the selected node in the center, with parents/ancestors above,
//! children below, blockers to the left, and blocked tasks to the right.

use crate::tui::app::{App, View};
use ratatui::prelude::*;
use ratatui::widgets::*;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let gs = match &app.graph {
        Some(gs) => gs,
        None => {
            frame.render_widget(Paragraph::new("Loading..."), area);
            return;
        }
    };

    // Determine selected node ID
    let node_id = match app.current_view {
        View::EpicTree | View::Graph => app
            .tree_rows
            .get(app.selected_index)
            .map(|r| r.node_id.clone()),
        View::Focus => app.focus_picks.get(app.selected_index).cloned(),
        _ => None,
    };

    let node_id = match node_id {
        Some(id) => id,
        None => {
            let p = Paragraph::new("Select a node in Tree or Focus view to see context.")
                .block(Block::default().borders(Borders::ALL).title(" Context "));
            frame.render_widget(p, area);
            return;
        }
    };

    let node = match gs.get_node(&node_id) {
        Some(n) => n,
        None => return,
    };

    // Layout:
    // Top: Parents (Containment Upstream)
    // Middle: Blockers (Left) | Node (Center) | Blocked (Right)
    // Bottom: Children (Containment Downstream)

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25), // Parents area
            Constraint::Length(8),      // Middle area
            Constraint::Percentage(65), // Children area
        ])
        .split(area);

    let middle_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // Dependencies (Blockers)
            Constraint::Percentage(40), // Node Details
            Constraint::Percentage(30), // Reverse Dependencies (Blocked)
        ])
        .split(vertical_chunks[1]);

    // 1. PARENTS (Top)
    let mut ancestors = Vec::new();
    let mut curr = node.parent.clone();
    while let Some(pid) = curr {
        if let Some(p) = gs.get_node(&pid) {
            ancestors.push(p);
            curr = p.parent.clone();
        } else {
            break;
        }
    }
    ancestors.reverse(); // Root first

    let parent_lines: Vec<Line> = ancestors
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let indent = "  ".repeat(i);
            Line::from(vec![
                Span::raw(format!("{}└─ ", indent)),
                Span::styled(p.label.clone(), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!(" [{}]", p.node_type.as_deref().unwrap_or("?")),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect();

    let parents_block = Block::default()
        .borders(Borders::ALL)
        .title(" Ancestors (Containment) ");
    let parents_widget = Paragraph::new(parent_lines).block(parents_block);
    frame.render_widget(parents_widget, vertical_chunks[0]);

    // 2. BLOCKERS (Left)
    let blockers: Vec<ListItem> = node
        .depends_on
        .iter()
        .map(|did| {
            let label = gs.get_node(did).map(|n| n.label.as_str()).unwrap_or(did);
            let status = gs
                .get_node(did)
                .and_then(|n| n.status.as_deref())
                .unwrap_or("?");
            let style = if status == "done" {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            ListItem::new(Line::from(vec![Span::styled(label, style)]))
        })
        .collect();

    let blockers_block = Block::default().borders(Borders::ALL).title(" Depends On ");
    let blockers_list = List::new(blockers).block(blockers_block);
    frame.render_widget(blockers_list, middle_chunks[0]);

    // 3. NODE DETAILS (Center)
    let status_color = match node.status.as_deref() {
        Some("done") => Color::Green,
        Some("blocked") => Color::Red,
        Some("active") => Color::Blue,
        _ => Color::White,
    };

    let details = vec![
        Line::from(vec![Span::styled(
            node.label.clone(),
            Style::default().fg(status_color).bold(),
        )]),
        Line::from(vec![
            Span::raw("Type: "),
            Span::raw(node.node_type.as_deref().unwrap_or("task")),
        ]),
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(
                node.status.as_deref().unwrap_or("active"),
                Style::default().fg(status_color),
            ),
        ]),
        Line::from(vec![
            Span::raw("Priority: "),
            Span::raw(node.priority.unwrap_or(2).to_string()),
        ]),
        Line::from(vec![Span::raw("ID: "), Span::raw(node.id.clone())]),
    ];

    let node_block = Block::default()
        .borders(Borders::ALL)
        .title(" Selected Node ")
        .border_style(Style::default().fg(status_color));
    let node_widget = Paragraph::new(details)
        .block(node_block)
        .alignment(Alignment::Center);
    frame.render_widget(node_widget, middle_chunks[1]);

    // 4. BLOCKED (Right)
    let blocked: Vec<ListItem> = node
        .blocks
        .iter()
        .map(|bid| {
            let label = gs.get_node(bid).map(|n| n.label.as_str()).unwrap_or(bid);
            ListItem::new(Line::from(vec![Span::raw(label)]))
        })
        .collect();

    let blocked_block = Block::default().borders(Borders::ALL).title(" Blocks ");
    let blocked_list = List::new(blocked).block(blocked_block);
    frame.render_widget(blocked_list, middle_chunks[2]);

    // 5. CHILDREN (Bottom)
    // We want to list children, maybe grouped by type?
    let mut children_lines: Vec<Line> = Vec::new();
    for cid in &node.children {
        if let Some(child) = gs.get_node(cid) {
            let icon = match child.node_type.as_deref() {
                Some("project") => "◈ ",
                Some("task") => "◇ ",
                _ => "○ ",
            };
            let color = match child.status.as_deref() {
                Some("done") => Color::Green,
                Some("blocked") => Color::Red,
                _ => Color::White,
            };

            children_lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(icon, Style::default().fg(color)),
                Span::styled(child.label.clone(), Style::default().fg(color)),
                Span::raw(format!(" [{}]", child.status.as_deref().unwrap_or("?"))),
            ]));
        }
    }

    let children_block = Block::default()
        .borders(Borders::ALL)
        .title(" Children (Containment) ");
    // Use paragraph for children so we can scroll if needed? App struct has detail_scroll but not specific scroll for this view.
    // For now just show top items.
    let children_widget = Paragraph::new(children_lines).block(children_block);
    frame.render_widget(children_widget, vertical_chunks[2]);

    // Draw arrows/connectors if we can?
    // Drawing lines on top of widgets is hard in ratatui without canvas.
    // We'll rely on the layout positioning to imply relationship.
    // Top -> Middle -> Bottom (Containment)
    // Left -> Middle -> Right (Dependency)
}
