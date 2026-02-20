//! Terminal User Interface module
//!
//! Provides a beautiful TUI for the Gugugaga, inspired by Codex CLI's design.

mod app;
pub mod ascii_animation;
pub mod frames;
mod input;
mod picker;
mod shimmer;
mod slash_commands;
mod theme;
mod widgets;

pub use app::App;
pub use picker::{Picker, PickerItem};
pub use slash_commands::{parse_command, CodexCommand, GugugagaCommand, ParsedCommand, SlashPopup};
pub use theme::Theme;
