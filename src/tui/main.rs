//! aops-tui — The Planning Web TUI
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
use std::path::PathBuf;
use std::time::Duration;

use app::{App, View};

fn main() -> Result<()> {
    // Quiet logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let pkb_root = std::env::var("ACA_DATA").unwrap_or_else(|_| ".".to_string());
    let pkb_root = PathBuf::from(pkb_root);
    let db_path = pkb_root.join("pkb_vectors.bin");

    let mut app = App::new(&pkb_root, &db_path);
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

/// Handle a key event. Returns true if the app should quit.
fn handle_key(key: KeyEvent, app: &mut App) -> bool {
    // Ctrl-C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
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
        KeyCode::Enter => app.open_detail(),

        // Priority filter (epic tree)
        KeyCode::Char('1') => app.toggle_priority_filter(1),
        KeyCode::Char('2') => app.toggle_priority_filter(2),
        KeyCode::Char('3') => app.toggle_priority_filter(3),

        // Help
        KeyCode::Char('?') => app.show_help = !app.show_help,

        _ => {}
    }
    false
}
