use ratatui::prelude::*;
use ratatui::widgets::*;

#[allow(dead_code)]
pub struct Theme;

#[allow(dead_code)]
impl Theme {
    // Colors
    pub const BG: Color = Color::Rgb(15, 15, 25); // Very dark blue/black
    pub const FG: Color = Color::Rgb(220, 220, 220); // Off-white
    pub const ACCENT_PRIMARY: Color = Color::Rgb(189, 147, 249); // Dracula Purple
    pub const ACCENT_SECONDARY: Color = Color::Rgb(139, 233, 253); // Dracula Cyan
    pub const SUCCESS: Color = Color::Rgb(80, 250, 123); // Dracula Green
    pub const WARNING: Color = Color::Rgb(255, 184, 108); // Dracula Orange
    pub const ERROR: Color = Color::Rgb(255, 85, 85); // Dracula Red
    pub const MUTED: Color = Color::Rgb(98, 114, 164); // Dracula Comment
    pub const HIGHLIGHT_BG: Color = Color::Rgb(68, 71, 90); // Dracula Selection

    // Styles
    pub fn root() -> Style {
        Style::default().bg(Self::BG).fg(Self::FG)
    }

    pub fn block() -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Self::MUTED))
            .title_style(Style::default().fg(Self::ACCENT_SECONDARY).bold())
            .padding(Padding::new(1, 1, 0, 0))
    }

    pub fn active_block() -> Block<'static> {
        Self::block().border_style(Style::default().fg(Self::ACCENT_PRIMARY))
    }

    pub fn dim_block() -> Block<'static> {
        Self::block()
            .border_style(Style::default().fg(Color::DarkGray))
            .title_style(Style::default().fg(Color::Gray))
    }

    pub fn title(text: &str) -> Span<'static> {
        Span::styled(
            text.to_string(),
            Style::default().fg(Self::ACCENT_PRIMARY).bold(),
        )
    }

    pub fn subtitle(text: &str) -> Span<'static> {
        Span::styled(text.to_string(), Style::default().fg(Self::MUTED).italic())
    }

    pub fn key_binding(key: &str, desc: &str) -> Span<'static> {
        Span::styled(
            format!(" {} {} ", key, desc),
            Style::default().fg(Self::MUTED),
        )
    }
}
