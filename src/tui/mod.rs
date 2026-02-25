//! The Planning Web TUI (`aops tui`)
//!
//! A graph-native terminal interface for academic planning built on the mem library.
//! Makes the PKB graph the interface rather than a flat task list.

mod app;
mod views;

use anyhow::Result;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::path::Path;
use std::time::Duration;

use app::{App, CaptureField, View};

/// Launch the interactive TUI. Tracing must already be initialised by the caller.
pub fn run(pkb_root: &Path, db_path: &Path) -> Result<()> {
    let mut app = App::new(pkb_root, db_path);
    app.load_graph();

    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_event_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| views::render(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(key, app) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn get_selected_node_id(app: &App) -> Option<String> {
    match app.current_view {
        View::EpicTree | View::Graph => {
            app.tree_rows.get(app.selected_index).map(|r| r.node_id.clone())
        }
        View::Focus => app.focus_picks.get(app.selected_index).cloned(),
        _ => None,
    }
}

fn handle_key(key: KeyEvent, app: &mut App) -> bool {
    // Ctrl-C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }

    // Quick capture overlay
    if app.show_capture {
        match key.code {
            KeyCode::Esc => {
                app.show_capture = false;
            }
            KeyCode::Tab => {
                app.capture_field = match app.capture_field {
                    CaptureField::Title => CaptureField::Project,
                    CaptureField::Project => CaptureField::Priority,
                    CaptureField::Priority => CaptureField::Title,
                };
            }
            KeyCode::Enter => {
                app.submit_capture();
            }
            KeyCode::Left => match app.capture_field {
                CaptureField::Project => {
                    if app.capture_project_idx > 0 {
                        app.capture_project_idx -= 1;
                    }
                }
                CaptureField::Priority => {
                    if app.capture_priority > 0 {
                        app.capture_priority -= 1;
                    }
                }
                _ => {}
            },
            KeyCode::Right => match app.capture_field {
                CaptureField::Project => {
                    if !app.project_names.is_empty()
                        && app.capture_project_idx < app.project_names.len() - 1
                    {
                        app.capture_project_idx += 1;
                    }
                }
                CaptureField::Priority => {
                    if app.capture_priority < 4 {
                        app.capture_priority += 1;
                    }
                }
                _ => {}
            },
            KeyCode::Backspace => {
                if app.capture_field == CaptureField::Title {
                    app.capture_title.pop();
                }
            }
            KeyCode::Char(c) => {
                if app.capture_field == CaptureField::Title {
                    app.capture_title.push(c);
                }
            }
            _ => {}
        }
        return false;
    }

    // Search overlay captures all key input
    if app.show_search {
        match key.code {
            KeyCode::Esc => {
                app.show_search = false;
                app.search_query.clear();
                app.search_results.clear();
            }
            KeyCode::Enter => {
                app.open_search_result();
            }
            KeyCode::Up => {
                app.search_selected = app.search_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                if !app.search_results.is_empty() {
                    app.search_selected =
                        (app.search_selected + 1).min(app.search_results.len() - 1);
                }
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.update_search();
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.update_search();
            }
            _ => {}
        }
        return false;
    }

    // View-specific keys when in detail overlay
    if app.show_detail {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.show_detail = false;
                return false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
                return false;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.detail_scroll += 1;
                return false;
            }
            _ => return false,
        }
    }

    match key.code {
        KeyCode::Char('q') => return true,

        // View switching
        KeyCode::Tab => app.next_view(),
        KeyCode::BackTab => app.prev_view(),
        KeyCode::Char('f') => app.current_view = View::Focus,
        KeyCode::Char('g') => app.current_view = View::Graph,
        KeyCode::Char('t') => app.current_view = View::EpicTree,
        KeyCode::Char('d') => app.current_view = View::Dashboard,

        // Navigation
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Left | KeyCode::Char('h') => app.collapse(),
        KeyCode::Right | KeyCode::Char('l') => app.expand(),
        KeyCode::Char(' ') => app.toggle_expand(),

        // Detail view
        KeyCode::Enter => {
            if app.reparent_mode {
                app.confirm_reparent();
            } else {
                app.open_detail();
            }
        },

        // Quick capture
        KeyCode::Char('n') => app.open_capture(),

        // Search
        KeyCode::Char('/') => {
            app.show_search = true;
            app.search_query.clear();
            app.search_results.clear();
            app.search_selected = 0;
        }

        // Priority filter (epic tree)
        KeyCode::Char('1') => app.toggle_priority_filter(1),
        KeyCode::Char('2') => app.toggle_priority_filter(2),
        KeyCode::Char('3') => app.toggle_priority_filter(3),

        // Toggles
        KeyCode::Char('C') => app.toggle_show_completed(),
        KeyCode::Char('T') => app.cycle_type_filter(),

        // Manipulation
        KeyCode::Char('s') => {
            if let Some(id) = get_selected_node_id(app) {
                if let Some(node) = app.get_node(&id) {
                    let new_status = match node.status.as_deref().unwrap_or("active") {
                        "active" => "done",
                        "done" => "blocked",
                        "blocked" => "dead",
                        "dead" => "active",
                        _ => "active",
                    };
                    app.set_status(&id, new_status);
                }
            }
        },
        KeyCode::Char('p') => {
            if let Some(id) = get_selected_node_id(app) {
                if let Some(node) = app.get_node(&id) {
                    let new_pri = match node.priority.unwrap_or(2) {
                        0 => 1,
                        1 => 2,
                        2 => 3,
                        3 => 0,
                        _ => 2,
                    };
                    app.set_priority(&id, new_pri);
                }
            }
        },
        KeyCode::Char('r') => app.enter_reparent_mode(),

        // Help
        KeyCode::Char('?') => app.show_help = !app.show_help,

        _ => {}
    }
    false
}
