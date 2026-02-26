//! View rendering for the Planning Web TUI.

mod capture;
mod dashboard;
mod detail;
mod epic_tree;
mod focus;
mod graph_view;
mod help;
mod search;
mod status_bar;

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::app::{App, View};
use crate::tui::theme::Theme;

/// Main render dispatch — draws the current view plus chrome.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Fill background
    frame.render_widget(Block::default().style(Theme::root()), area);

    // Layout: status bar (1 line) at top, main content, keybindings (1 line) at bottom
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status bar
            Constraint::Min(1),    // main content
            Constraint::Length(1), // keybindings
        ])
        .split(area);

    status_bar::render(frame, app, chunks[0]);

    // Add a bit of margin for the main view if desired, but for TUI usually fullscreen is good.
    // We can wrap the main view in a block if we want a global border, but let's keep it simple for now.
    let main_area = chunks[1];

    match app.current_view {
        View::Focus => focus::render(frame, app, main_area),
        View::Graph => graph_view::render(frame, app, main_area),
        View::EpicTree => epic_tree::render(frame, app, main_area),
        View::Dashboard => dashboard::render(frame, app, main_area),
    }

    render_keybindings(frame, app, chunks[2]);

    // Overlays (rendered last, on top)
    if app.show_detail {
        detail::render(frame, app, area);
    }
    if app.show_search {
        search::render(frame, app, area);
    }
    if app.show_capture {
        capture::render(frame, app, area);
    }
    if app.show_help {
        help::render(frame, area);
    }
}

fn render_keybindings(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.current_view {
        View::EpicTree => "↑↓ navigate │ ←→ expand/collapse │ Enter detail │ Space toggle │ 1-3 filter │ / search │ Tab views │ ? help │ q quit",
        View::Focus => "↑↓ navigate │ Enter detail │ / search │ Tab views │ ? help │ q quit",
        View::Graph => "↑↓ navigate │ ←→ expand/collapse │ Enter detail │ / search │ Tab views │ ? help │ q quit",
        View::Dashboard => "/ search │ Tab views │ ? help │ q quit",
    };

    let bar = Paragraph::new(keys)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Theme::MUTED).bg(Theme::BG));
    frame.render_widget(bar, area);
}
