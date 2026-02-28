//! Epic Tree view — task list grouped by project/epic, with tree structure.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, TreeRow};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if app.tree_rows.is_empty() {
        let msg = Paragraph::new("No ready tasks found.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(msg, area);
        return;
    }

    let visible_height = area.height as usize;

    // Compute scroll offset to keep selected row visible
    let scroll_offset = compute_scroll(app.selected_index, visible_height, app.tree_rows.len());

    let items: Vec<ListItem> = app
        .tree_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| render_row(row, idx == app.selected_index, area.width as usize))
        .collect();

    let list = List::new(items).highlight_style(Style::default());
    frame.render_widget(list, area);
}

fn compute_scroll(selected: usize, height: usize, total: usize) -> usize {
    if total <= height {
        return 0;
    }
    let margin = 3.min(height / 4);
    if selected < margin {
        0
    } else if selected + margin >= total {
        total.saturating_sub(height)
    } else {
        selected.saturating_sub(margin)
    }
}

fn render_row(row: &TreeRow, selected: bool, width: usize) -> ListItem<'static> {
    let mut spans = Vec::new();

    // Selection indicator
    if selected {
        spans.push(Span::styled("▸ ", Style::default().fg(Color::Cyan).bold()));
    } else {
        spans.push(Span::raw("  "));
    }

    // Tree connector lines
    if row.depth > 0 {
        let mut prefix = String::new();
        for (i, &is_last) in row.is_last_at_depth.iter().enumerate() {
            if i == 0 {
                continue; // skip root level
            }
            if i == row.is_last_at_depth.len() - 1 {
                // This is the connector for the current node
                prefix.push_str(if is_last { "└── " } else { "├── " });
            } else {
                // Continuation lines
                prefix.push_str(if is_last { "    " } else { "│   " });
            }
        }
        spans.push(Span::styled(prefix, Style::default().fg(Color::DarkGray)));
    }

    if row.is_context {
        // Context node (project/epic/goal)
        let icon = match row.node_type.as_deref() {
            Some("goal") => "◉ ",
            Some("project") | Some("subproject") => "◈ ",
            Some("epic") => "██ ",
            _ => "◈ ",
        };
        let color = match row.node_type.as_deref() {
            Some("goal") => Color::Yellow,
            Some("project") | Some("subproject") => Color::Cyan,
            Some("epic") => Color::Cyan,
            _ => Color::Blue,
        };
        spans.push(Span::styled(icon, Style::default().fg(color).bold()));
        spans.push(Span::styled(
            row.label.clone(),
            Style::default().fg(color).bold(),
        ));

        // Child count
        if row.child_count > 0 {
            spans.push(Span::styled(
                format!(" ({})", row.child_count),
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Expand/collapse indicator
        if row.has_children {
            let indicator = if row.expanded { " ▾" } else { " ▸" };
            spans.push(Span::styled(
                indicator,
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else {
        // Task node
        let pri = row.priority.unwrap_or(2);
        let pri_color = match pri {
            0 | 1 => Color::Red,
            2 => Color::White,
            _ => Color::DarkGray,
        };

        // Priority badge
        let exposure = if row.stakeholder_exposure { "!" } else { "" };
        spans.push(Span::styled(
            format!("P{pri}{exposure} "),
            Style::default().fg(pri_color).bold(),
        ));

        // Task type icon (inferred from title)
        let icon = infer_type_icon(&row.label);
        if !icon.is_empty() {
            spans.push(Span::styled(
                format!("{icon} "),
                Style::default().fg(Color::Gray),
            ));
        }

        // Status badge for non-active states
        match row.status.as_deref() {
            Some("blocked") => {
                spans.push(Span::styled("⊘ ", Style::default().fg(Color::Red)));
            }
            Some("waiting") => {
                spans.push(Span::styled("◷ ", Style::default().fg(Color::Yellow)));
            }
            _ => {}
        }

        // Label
        let label_style = match row.status.as_deref() {
            Some("blocked") => Style::default().fg(Color::Red),
            Some("waiting") => Style::default().fg(Color::Yellow),
            _ => match pri {
                0 | 1 => Style::default().fg(Color::Red).bold(),
                2 => Style::default().fg(Color::White),
                _ => Style::default().fg(Color::DarkGray),
            },
        };

        // Truncate label to fit (char-boundary safe)
        let prefix_len = spans.iter().map(|s| s.content.len()).sum::<usize>();
        let right_width = 20; // space for staleness + id
        let available = width.saturating_sub(prefix_len + right_width);
        let label = if row.label.len() > available {
            let truncate_at = available.saturating_sub(1);
            // Find a valid char boundary
            let safe_end = row.label.floor_char_boundary(truncate_at);
            format!("{}…", &row.label[..safe_end])
        } else {
            row.label.clone()
        };

        spans.push(Span::styled(label.clone(), label_style));

        // Right-aligned info: staleness + weight + task ID
        let mut right_parts = Vec::new();

        if row.downstream_weight > 0.0 {
            right_parts.push(format!("wt:{:.1}", row.downstream_weight));
        }

        if let Some(ref created) = row.created {
            if let Some(days) = days_since(created) {
                let (age_str, _age_color) = format_staleness(days);
                // We'll add this as the last span
                right_parts.push(age_str);
            }
        }

        if let Some(ref tid) = row.task_id {
            let short = if tid.len() > 12 {
                let end = tid.floor_char_boundary(12);
                format!("{}…", &tid[..end])
            } else {
                tid.clone()
            };
            right_parts.push(format!("[{short}]"));
        }

        if !right_parts.is_empty() {
            // Pad to right-align
            let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
            let right_text = right_parts.join(" ");
            let padding = width.saturating_sub(current_len + right_text.len() + 1);
            if padding > 0 {
                spans.push(Span::raw(" ".repeat(padding)));
            }
            // Use staleness color for the age portion if present
            let right_color = if let Some(ref created) = row.created {
                if let Some(days) = days_since(created) {
                    if days > 30 {
                        Color::Red
                    } else if days > 14 {
                        Color::Yellow
                    } else {
                        Color::DarkGray
                    }
                } else {
                    Color::DarkGray
                }
            } else {
                Color::DarkGray
            };
            spans.push(Span::styled(
                right_text,
                Style::default().fg(right_color),
            ));
        }
    }

    let line = Line::from(spans);
    let style = if selected {
        Style::default().bg(Color::Rgb(30, 30, 50))
    } else {
        Style::default()
    };
    ListItem::new(line).style(style)
}

/// Infer a type icon from the task title.
fn infer_type_icon(title: &str) -> &'static str {
    let lower = title.to_lowercase();
    if lower.starts_with("decide:") || lower.starts_with("decision:") {
        "⚖"
    } else if lower.starts_with("reply to")
        || lower.starts_with("email:")
        || lower.starts_with("respond to")
    {
        "✉"
    } else if lower.starts_with("call ") || lower.starts_with("phone:") {
        "📞"
    } else if lower.starts_with("write ") || lower.starts_with("draft ") {
        "✏"
    } else if lower.starts_with("research:") || lower.starts_with("investigate") {
        "🔬"
    } else if lower.starts_with("review ") || lower.starts_with("peer review") {
        "📋"
    } else if lower.starts_with("confirm") || lower.starts_with("rsvp") {
        "✉"
    } else {
        ""
    }
}

fn days_since(date_str: &str) -> Option<i64> {
    let dt = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    Some((chrono::Local::now().date_naive() - dt).num_days())
}

fn format_staleness(days: i64) -> (String, Color) {
    let color = if days > 30 {
        Color::Red
    } else if days > 14 {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    (format!("{days}d"), color)
}
