//! Input handling for the TUI

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Result of handling an input event
#[derive(Debug, Clone)]
pub enum InputAction {
    /// No action needed
    None,
    /// Submit the current input
    Submit(String),
    /// Quit the application
    Quit,
    /// Scroll up in messages
    ScrollUp,
    /// Scroll down in messages
    ScrollDown,
    /// Switch focus between panels
    SwitchFocus,
    /// Character input
    Input(char),
    /// Delete character
    Backspace,
    /// Delete word
    DeleteWord,
    /// Clear input
    ClearInput,
    /// Move cursor left
    CursorLeft,
    /// Move cursor right
    CursorRight,
    /// Move cursor to start
    CursorHome,
    /// Move cursor to end
    CursorEnd,
    /// Paste from clipboard
    Paste,
    /// Previous history item
    HistoryPrev,
    /// Next history item
    HistoryNext,
    /// Tab pressed (for autocomplete)
    Tab,
    /// Escape pressed (close popup)
    Escape,
}

/// Input state manager
/// 
/// Note: `cursor` is a CHARACTER index, not a byte index.
/// This is important for proper handling of multi-byte characters (e.g., Chinese).
#[derive(Debug, Clone)]
pub struct InputState {
    /// Current input buffer
    pub buffer: String,
    /// Cursor position in buffer (character index, not byte index!)
    pub cursor: usize,
    /// Command history
    pub history: Vec<String>,
    /// Current history index (-1 means current input)
    pub history_index: isize,
    /// Saved current input when browsing history
    pub saved_input: String,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: -1,
            saved_input: String::new(),
        }
    }

    /// Get the number of characters in the buffer
    fn char_count(&self) -> usize {
        self.buffer.chars().count()
    }

    /// Convert character index to byte index
    fn char_to_byte_index(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(byte_idx, _)| byte_idx)
            .unwrap_or(self.buffer.len())
    }

    /// Insert a character at the current cursor position (character index)
    fn insert_char(&mut self, c: char) {
        let byte_idx = self.char_to_byte_index(self.cursor);
        self.buffer.insert(byte_idx, c);
        self.cursor += 1;
    }

    /// Remove the character before the cursor
    fn remove_char_before_cursor(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_idx = self.char_to_byte_index(self.cursor);
            // Find the character at this position and remove it
            if let Some((_, ch)) = self.buffer.char_indices().nth(self.cursor) {
                self.buffer.replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
            }
        }
    }

    /// Remove the character at the cursor position
    fn remove_char_at_cursor(&mut self) {
        let char_count = self.char_count();
        if self.cursor < char_count {
            let byte_idx = self.char_to_byte_index(self.cursor);
            if let Some((_, ch)) = self.buffer.char_indices().nth(self.cursor) {
                self.buffer.replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
            }
        }
    }

    /// Handle a key event and return the action
    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            // Quit
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::Quit
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::Quit
            }

            // Submit
            KeyCode::Enter => {
                if !self.buffer.is_empty() {
                    let input = self.buffer.clone();
                    self.history.push(input.clone());
                    self.buffer.clear();
                    self.cursor = 0;
                    self.history_index = -1;
                    InputAction::Submit(input)
                } else {
                    InputAction::None
                }
            }

            // Editing
            KeyCode::Backspace => {
                self.remove_char_before_cursor();
                InputAction::Backspace
            }
            KeyCode::Delete => {
                self.remove_char_at_cursor();
                InputAction::None
            }

            // Clear
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buffer.clear();
                self.cursor = 0;
                InputAction::ClearInput
            }

            // Delete word
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.delete_word();
                InputAction::DeleteWord
            }

            // Cursor movement
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                InputAction::CursorLeft
            }
            KeyCode::Right => {
                if self.cursor < self.char_count() {
                    self.cursor += 1;
                }
                InputAction::CursorRight
            }
            KeyCode::Home | KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
                InputAction::CursorHome
            }
            KeyCode::End | KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.char_count();
                InputAction::CursorEnd
            }

            // Up/Down for scrolling (alternate scroll mode converts mouse wheel to arrows)
            KeyCode::Up => InputAction::HistoryPrev,  // Used as scroll in app.rs
            KeyCode::Down => InputAction::HistoryNext, // Used as scroll in app.rs

            // PageUp/PageDown for faster scrolling
            KeyCode::PageUp => InputAction::ScrollUp,
            KeyCode::PageDown => InputAction::ScrollDown,

            // Tab for autocomplete
            KeyCode::Tab => InputAction::Tab,

            // Escape to close popup or clear
            KeyCode::Esc => InputAction::Escape,

            // Regular input
            KeyCode::Char(c) => {
                self.insert_char(c);
                InputAction::Input(c)
            }

            _ => InputAction::None,
        }
    }

    /// Check if buffer starts with a slash (potential command)
    pub fn is_slash_command(&self) -> bool {
        self.buffer.starts_with('/')
    }

    /// Get the current slash command prefix (text after / for filtering)
    pub fn slash_prefix(&self) -> Option<&str> {
        if self.buffer.starts_with('/') {
            // Get text after / up to first space or end
            let after_slash = &self.buffer[1..];
            let end = after_slash.find(' ').unwrap_or(after_slash.len());
            Some(&after_slash[..end])
        } else {
            None
        }
    }

    /// Replace buffer content with a new string (for autocomplete)
    pub fn set_buffer(&mut self, content: &str) {
        self.buffer = content.to_string();
        self.cursor = self.char_count();
    }

    fn delete_word(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let chars: Vec<char> = self.buffer.chars().collect();
        
        // Skip trailing whitespace
        while self.cursor > 0 && chars.get(self.cursor - 1) == Some(&' ') {
            self.remove_char_before_cursor();
        }

        // Delete until whitespace or start
        while self.cursor > 0 {
            let chars: Vec<char> = self.buffer.chars().collect();
            if chars.get(self.cursor - 1) == Some(&' ') {
                break;
            }
            self.remove_char_before_cursor();
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        if self.history_index == -1 {
            self.saved_input = self.buffer.clone();
        }

        if self.history_index < self.history.len() as isize - 1 {
            self.history_index += 1;
            let idx = self.history.len() - 1 - self.history_index as usize;
            self.buffer = self.history[idx].clone();
            self.cursor = self.char_count();
        }
    }

    fn history_next(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let idx = self.history.len() - 1 - self.history_index as usize;
            self.buffer = self.history[idx].clone();
            self.cursor = self.char_count();
        } else if self.history_index == 0 {
            self.history_index = -1;
            self.buffer = self.saved_input.clone();
            self.cursor = self.char_count();
        }
    }

    /// Get cursor position for display (in terminal columns)
    /// This accounts for wide characters (like CJK) taking 2 columns
    pub fn cursor_display_width(&self) -> usize {
        self.buffer
            .chars()
            .take(self.cursor)
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_ascii() {
        let mut state = InputState::new();
        state.insert_char('h');
        state.insert_char('i');
        assert_eq!(state.buffer, "hi");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn test_insert_unicode() {
        let mut state = InputState::new();
        state.insert_char('\u{4f60}'); // Unicode char
        state.insert_char('\u{597d}'); // Unicode char
        assert_eq!(state.buffer, "\u{4f60}\u{597d}");
        assert_eq!(state.cursor, 2);
        assert_eq!(state.char_count(), 2);
    }

    #[test]
    fn test_backspace_unicode() {
        let mut state = InputState::new();
        state.buffer = "\u{4f60}\u{597d}".to_string();
        state.cursor = 2;
        state.remove_char_before_cursor();
        assert_eq!(state.buffer, "\u{4f60}");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn test_mixed_content() {
        let mut state = InputState::new();
        state.insert_char('h');
        state.insert_char('i');
        state.insert_char('\u{4f60}');
        state.insert_char('\u{597d}');
        assert_eq!(state.buffer, "hi\u{4f60}\u{597d}");
        assert_eq!(state.cursor, 4);
    }
}
