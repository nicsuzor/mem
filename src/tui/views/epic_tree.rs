//! Epic Tree view — task list grouped by project/epic, with tree structure.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, TreeRow};
use crate::tui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if app.tree_rows.is_empty() {
        let msg = Paragraph::new("No ready tasks found.")
            .style(Style::default().fg(Theme::MUTED))
            .alignment(Alignment::Center)
            .block(Theme::block().title(" Epic Tree "));
        frame.render_widget(msg, area);
        return;
    }

    let inner_area = area; // or area.inner(&Margin { vertical: 1, horizontal: 1 }); if we want inner padding inside block
    let visible_height = inner_area.height.saturating_sub(2) as usize; // adjust for borders

    // Compute scroll offset to keep selected row visible
    let scroll_offset = compute_scroll(app.selected_index, visible_height, app.tree_rows.len());

    let width = inner_area.width as usize;

    let items: Vec<ListItem> = app
        .tree_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| render_row(row, idx == app.selected_index, width))
        .collect();

    let list = List::new(items)
        .block(Theme::block().title(" Epic Tree "))
        .highlight_style(Style::default());
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
        spans.push(Span::styled(
            "▸ ",
            Style::default().fg(Theme::ACCENT_SECONDARY).bold(),
        ));
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
        spans.push(Span::styled(prefix, Style::default().fg(Theme::MUTED)));
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
            Some("goal") => Theme::WARNING,
            Some("project") | Some("subproject") => Theme::ACCENT_SECONDARY,
            Some("epic") => Theme::ACCENT_SECONDARY,
            _ => Theme::ACCENT_PRIMARY,
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
                Style::default().fg(Theme::MUTED),
            ));
        }

        // Expand/collapse indicator
        if row.has_children {
            let indicator = if row.expanded { " ▾" } else { " ▸" };
            spans.push(Span::styled(indicator, Style::default().fg(Theme::MUTED)));
        }
    } else {
        // Task node
        let pri = row.priority.unwrap_or(2);
        let pri_color = match pri {
            0 | 1 => Theme::ERROR,
            2 => Theme::FG,
            _ => Theme::MUTED,
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
                Style::default().fg(Theme::MUTED),
            ));
        }

        // Label
        let label_style = match pri {
            0 | 1 => Style::default().fg(Theme::ERROR).bold(),
            2 => Style::default().fg(Theme::FG),
            _ => Style::default().fg(Theme::MUTED),
        };

        // Truncate label to fit
        // Need to calculate current length more carefully or just estimate
        // spans so far: selection(2) + connectors + icon/pri + icon
        // It's tricky to get exact width here without iterating spans, so we'll do a rough calc
        // or just rely on render truncating if it overflows (which it doesn't automatically)
        // Let's stick to the original logic but update it.

        let prefix_len = spans.iter().map(|s| s.content.len()).sum::<usize>();
        let right_width = 24; // space for staleness + id + weight
        let available = width.saturating_sub(prefix_len + right_width).max(10);

        let label = if row.label.len() > available {
            format!("{}…", &row.label[..available.saturating_sub(1)])
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
                let age_str = format!("{days}d");
                right_parts.push(age_str);
            }
        }

        if let Some(ref tid) = row.task_id {
            let short = if tid.len() > 8 {
                format!("{}…", &tid[..8])
            } else {
                tid.clone()
            };
            right_parts.push(format!("[{short}]"));
        }

        if !right_parts.is_empty() {
            // Pad to right-align
            let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
            let right_text = right_parts.join(" ");
            let padding = width.saturating_sub(current_len + right_text.len() + 2); // +2 for border safety
            if padding > 0 {
                spans.push(Span::raw(" ".repeat(padding)));
            }
            spans.push(Span::styled(right_text, Style::default().fg(Theme::MUTED)));
        }
    }

    let line = Line::from(spans);
    let style = if selected {
        Style::default().bg(Theme::HIGHLIGHT_BG)
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
