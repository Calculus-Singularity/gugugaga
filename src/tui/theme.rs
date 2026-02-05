//! Color theme for the TUI - using ANSI colors for better terminal compatibility

use ratatui::style::{Color, Modifier, Style};

/// Theme using ANSI colors that work well across terminal themes
pub struct Theme;

impl Theme {
    // Use ANSI colors for better terminal compatibility
    pub const CYAN: Color = Color::Cyan;
    pub const GREEN: Color = Color::Green;
    pub const YELLOW: Color = Color::Yellow;
    pub const RED: Color = Color::Red;
    pub const BLUE: Color = Color::Blue;
    pub const MAGENTA: Color = Color::Magenta;
    pub const DARK_GRAY: Color = Color::DarkGray;
    pub const GRAY: Color = Color::Gray;

    // Semantic styles
    pub fn title() -> Style {
        Style::default()
            .fg(Self::CYAN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn subtitle() -> Style {
        Style::default()
            .fg(Self::MAGENTA)
            .add_modifier(Modifier::BOLD)
    }

    pub fn text() -> Style {
        Style::default()
    }

    pub fn dim() -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    pub fn bold() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn success() -> Style {
        Style::default().fg(Self::GREEN)
    }

    pub fn warning() -> Style {
        Style::default().fg(Self::YELLOW)
    }

    pub fn error() -> Style {
        Style::default().fg(Self::RED)
    }

    pub fn info() -> Style {
        Style::default().fg(Self::BLUE)
    }

    pub fn accent() -> Style {
        Style::default().fg(Self::CYAN)
    }

    pub fn muted() -> Style {
        Style::default().fg(Self::DARK_GRAY)
    }

    pub fn thinking() -> Style {
        Style::default()
            .fg(Self::GRAY)
            .add_modifier(Modifier::ITALIC)
    }

    pub fn border() -> Style {
        Style::default().fg(Self::DARK_GRAY)
    }

    pub fn border_focused() -> Style {
        Style::default().fg(Self::CYAN)
    }

    // Badge styles
    pub fn user_badge() -> Style {
        Style::default()
            .bg(Self::CYAN)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn codex_badge() -> Style {
        Style::default()
            .bg(Self::GREEN)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn gugugaga_badge() -> Style {
        Style::default()
            .bg(Self::MAGENTA)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn correction_badge() -> Style {
        Style::default()
            .bg(Self::YELLOW)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn system_badge() -> Style {
        Style::default()
            .fg(Self::DARK_GRAY)
            .add_modifier(Modifier::ITALIC)
    }

    // Status indicators
    pub fn running() -> Style {
        Style::default()
            .fg(Self::GREEN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn idle() -> Style {
        Style::default().fg(Self::DARK_GRAY)
    }

    // Key hints
    pub fn key() -> Style {
        Style::default()
            .fg(Self::CYAN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn key_desc() -> Style {
        Style::default().fg(Self::DARK_GRAY)
    }
}
