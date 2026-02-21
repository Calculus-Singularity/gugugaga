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
    #[allow(dead_code)]
    SwitchFocus,
    /// Character input
    Input(char),
    /// Delete character
    Backspace,
    /// Delete word
    DeleteWord,
    /// Delete character at cursor
    Delete,
    /// Move cursor left
    CursorLeft,
    /// Move cursor right
    CursorRight,
    /// Move cursor to start
    CursorHome,
    /// Move cursor to end
    CursorEnd,
    /// Paste from clipboard
    #[allow(dead_code)]
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
    /// Last killed text chunk for Ctrl+Y yank.
    kill_buffer: String,
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
            kill_buffer: String::new(),
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
                self.buffer
                    .replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
            }
        }
    }

    /// Remove the character at the cursor position
    fn remove_char_at_cursor(&mut self) {
        let char_count = self.char_count();
        if self.cursor < char_count {
            let byte_idx = self.char_to_byte_index(self.cursor);
            if let Some((_, ch)) = self.buffer.char_indices().nth(self.cursor) {
                self.buffer
                    .replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
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
            // Ctrl+D is handled in App to match Codex quit semantics
            // (only when composer is empty and no modal/popup is active).
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::None
            }

            // Submit
            KeyCode::Enter => {
                if !self.buffer.is_empty() {
                    InputAction::Submit(self.buffer.clone())
                } else {
                    InputAction::None
                }
            }

            // Editing
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                self.delete_word();
                InputAction::DeleteWord
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.remove_char_before_cursor();
                InputAction::Backspace
            }
            KeyCode::Backspace => {
                self.remove_char_before_cursor();
                InputAction::Backspace
            }
            KeyCode::Delete if key.modifiers.contains(KeyModifiers::ALT) => {
                self.delete_word_forward();
                InputAction::Delete
            }
            KeyCode::Delete => {
                self.remove_char_at_cursor();
                InputAction::Delete
            }

            // Kill to beginning of line
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.kill_to_beginning_of_line();
                InputAction::Delete
            }

            // Delete word
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.delete_word();
                InputAction::DeleteWord
            }
            // Kill to end of line
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.kill_to_end_of_line();
                InputAction::Delete
            }
            // Yank previously killed text
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.yank();
                InputAction::Delete
            }

            // Cursor movement
            KeyCode::Left
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.cursor = self.beginning_of_previous_word();
                InputAction::CursorLeft
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                InputAction::CursorLeft
            }
            KeyCode::Right
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                self.cursor = self.end_of_next_word();
                InputAction::CursorRight
            }
            KeyCode::Right => {
                if self.cursor < self.char_count() {
                    self.cursor += 1;
                }
                InputAction::CursorRight
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                InputAction::CursorLeft
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.cursor < self.char_count() {
                    self.cursor += 1;
                }
                InputAction::CursorRight
            }
            KeyCode::Home => {
                self.cursor = 0;
                InputAction::CursorHome
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
                InputAction::CursorHome
            }
            KeyCode::End => {
                self.cursor = self.char_count();
                InputAction::CursorEnd
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.char_count();
                InputAction::CursorEnd
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::HistoryPrev
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::HistoryNext
            }

            // Up/Down for history navigation (when allowed by composer state)
            KeyCode::Up => InputAction::HistoryPrev,
            KeyCode::Down => InputAction::HistoryNext,

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
    #[allow(dead_code)]
    pub fn is_slash_command(&self) -> bool {
        self.buffer.starts_with('/')
    }

    /// Get the current slash command prefix (text after / for filtering)
    #[allow(dead_code)]
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

    /// Clear the current editable input (does not clear history).
    pub fn clear_current_input(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = -1;
    }

    /// Commit current buffer as a successful submission:
    /// add to history and clear editor state.
    pub fn commit_submission(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        self.history.push(self.buffer.clone());
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = -1;
    }

    /// Insert text at the current cursor position.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let byte_idx = self.char_to_byte_index(self.cursor);
        self.buffer.insert_str(byte_idx, text);
        self.cursor += text.chars().count();
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

    fn delete_word_forward(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        if self.cursor >= chars.len() {
            return;
        }

        let mut end = self.cursor;
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }

        if end > self.cursor {
            let start_byte = self.char_to_byte_index(self.cursor);
            let end_byte = self.char_to_byte_index(end);
            self.buffer.replace_range(start_byte..end_byte, "");
        }
    }

    fn kill_to_beginning_of_line(&mut self) {
        if self.cursor == 0 {
            self.kill_buffer.clear();
            return;
        }
        let end_byte = self.char_to_byte_index(self.cursor);
        self.kill_buffer = self.buffer[..end_byte].to_string();
        self.buffer.replace_range(..end_byte, "");
        self.cursor = 0;
    }

    fn kill_to_end_of_line(&mut self) {
        let total = self.char_count();
        if self.cursor >= total {
            self.kill_buffer.clear();
            return;
        }
        let start_byte = self.char_to_byte_index(self.cursor);
        self.kill_buffer = self.buffer[start_byte..].to_string();
        self.buffer.truncate(start_byte);
    }

    fn yank(&mut self) {
        if self.kill_buffer.is_empty() {
            return;
        }
        let byte_idx = self.char_to_byte_index(self.cursor);
        self.buffer.insert_str(byte_idx, &self.kill_buffer);
        self.cursor += self.kill_buffer.chars().count();
    }

    fn beginning_of_previous_word(&self) -> usize {
        if self.cursor == 0 {
            return 0;
        }
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut idx = self.cursor;
        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        while idx > 0 && !chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        idx
    }

    fn end_of_next_word(&self) -> usize {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut idx = self.cursor;
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        while idx < chars.len() && !chars[idx].is_whitespace() {
            idx += 1;
        }
        idx
    }

    fn current_history_entry(&self) -> Option<&str> {
        if self.history_index < 0 {
            return None;
        }
        let idx = self
            .history
            .len()
            .checked_sub(1 + self.history_index as usize)?;
        self.history.get(idx).map(|s| s.as_str())
    }

    fn cursor_at_line_boundary(&self) -> bool {
        self.cursor == 0 || self.cursor == self.char_count()
    }

    /// Returns whether Up/Down should navigate history for current input state.
    ///
    /// Empty buffer always allows history navigation. For non-empty buffer, this
    /// requires:
    /// - currently browsing history, and
    /// - buffer exactly equals the last recalled history entry, and
    /// - cursor is at line boundary (start or end).
    pub fn should_handle_history_navigation(&self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        if self.buffer.is_empty() {
            return true;
        }
        if !self.cursor_at_line_boundary() {
            return false;
        }
        matches!(self.current_history_entry(), Some(prev) if prev == self.buffer)
    }

    pub fn navigate_history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }

        if self.history_index == -1 {
            self.saved_input = self.buffer.clone();
        }

        if self.history_index < self.history.len() as isize - 1 {
            self.history_index += 1;
            let idx = self.history.len() - 1 - self.history_index as usize;
            self.buffer = self.history[idx].clone();
            self.cursor = self.char_count();
            true
        } else {
            false
        }
    }

    pub fn navigate_history_next(&mut self) -> bool {
        if self.history_index > 0 {
            self.history_index -= 1;
            let idx = self.history.len() - 1 - self.history_index as usize;
            self.buffer = self.history[idx].clone();
            self.cursor = self.char_count();
            true
        } else if self.history_index == 0 {
            self.history_index = -1;
            self.buffer = self.saved_input.clone();
            self.cursor = self.char_count();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    #[test]
    fn test_history_navigation_empty_buffer() {
        let mut state = InputState::new();
        state.history = vec!["first".to_string(), "second".to_string()];

        assert!(state.should_handle_history_navigation());
        assert!(state.navigate_history_prev());
        assert_eq!(state.buffer, "second");
        assert_eq!(state.cursor, 6);
    }

    #[test]
    fn test_history_navigation_requires_recalled_entry_when_non_empty() {
        let mut state = InputState::new();
        state.history = vec!["hello".to_string()];
        state.buffer = "draft".to_string();
        state.cursor = state.buffer.chars().count();

        assert!(!state.should_handle_history_navigation());

        assert!(state.navigate_history_prev());
        assert_eq!(state.buffer, "hello");
        assert!(state.should_handle_history_navigation());

        state.insert_char('!');
        assert!(!state.should_handle_history_navigation());
    }

    #[test]
    fn test_enter_does_not_clear_until_commit_submission() {
        let mut state = InputState::new();
        state.set_buffer("hello");

        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, InputAction::Submit(ref s) if s == "hello"));
        assert_eq!(state.buffer, "hello");
        assert!(state.history.is_empty());

        state.commit_submission();
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor, 0);
        assert_eq!(state.history, vec!["hello".to_string()]);
    }
}
