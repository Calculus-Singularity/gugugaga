//! Generic picker widget for selecting from a list

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use super::theme::Theme;

/// A selectable item in the picker
#[derive(Debug, Clone)]
pub struct PickerItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
}

/// Generic picker state
#[derive(Debug, Clone)]
pub struct Picker {
    /// Whether the picker is visible
    pub visible: bool,
    /// Title of the picker
    pub title: String,
    /// Available items
    pub items: Vec<PickerItem>,
    /// Currently selected index
    pub selected: usize,
    /// Whether currently loading
    pub loading: bool,
    /// Scroll offset for long lists
    pub scroll_offset: usize,
}

impl Default for Picker {
    fn default() -> Self {
        Self::new("Select")
    }
}

impl Picker {
    pub fn new(title: &str) -> Self {
        Self {
            visible: false,
            title: title.to_string(),
            items: Vec::new(),
            selected: 0,
            loading: false,
            scroll_offset: 0,
        }
    }

    /// Open the picker with items
    pub fn open(&mut self, items: Vec<PickerItem>) {
        self.visible = true;
        self.items = items;
        self.selected = 0;
        self.scroll_offset = 0;
        self.loading = false;
    }

    /// Open in loading state
    pub fn open_loading(&mut self) {
        self.visible = true;
        self.items.clear();
        self.selected = 0;
        self.loading = true;
    }

    /// Set items (when loading completes)
    pub fn set_items(&mut self, items: Vec<PickerItem>) {
        self.items = items;
        self.loading = false;
    }

    /// Close the picker
    pub fn close(&mut self) {
        self.visible = false;
        self.items.clear();
        self.selected = 0;
        self.loading = false;
    }

    /// Move selection up
    pub fn select_prev(&mut self) {
        if !self.items.is_empty() {
            if self.selected == 0 {
                self.selected = self.items.len() - 1;
            } else {
                self.selected -= 1;
            }
            self.ensure_visible();
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
            self.ensure_visible();
        }
    }

    /// Ensure selected item is visible
    fn ensure_visible(&mut self) {
        const VISIBLE_ITEMS: usize = 8;
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + VISIBLE_ITEMS {
            self.scroll_offset = self.selected - VISIBLE_ITEMS + 1;
        }
    }

    /// Get the selected item
    pub fn selected_item(&self) -> Option<&PickerItem> {
        self.items.get(self.selected)
    }

    /// Render the picker
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.visible {
            return;
        }

        // Calculate picker size and position (centered)
        let width = (area.width * 2 / 3).min(60).max(40);
        let height = 12.min(area.height.saturating_sub(4));
        let x = area.x + (area.width - width) / 2;
        let y = area.y + (area.height - height) / 2;

        let picker_area = Rect::new(x, y, width, height);

        // Clear background
        Clear.render(picker_area, buf);

        // Build content
        let inner_height = height.saturating_sub(2) as usize;
        
        let lines: Vec<Line> = if self.loading {
            vec![Line::styled("Loading...", Theme::muted())]
        } else if self.items.is_empty() {
            vec![Line::styled("No sessions found.", Theme::muted())]
        } else {
            self.items
                .iter()
                .enumerate()
                .skip(self.scroll_offset)
                .take(inner_height)
                .map(|(i, item)| {
                    let is_selected = i == self.selected;
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let style = if is_selected {
                        Theme::accent()
                    } else {
                        Theme::text()
                    };

                    Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(&item.title, style.bold()),
                        Span::styled(format!(" - {}", item.subtitle), Theme::muted()),
                    ])
                })
                .collect()
        };

        let title = format!(" {} ", self.title);
        let help = " ↑↓ Select │ Enter Confirm │ Esc Cancel ";
        
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::accent())
            .title_top(Line::styled(title, Theme::title()))
            .title_bottom(Line::styled(help, Theme::muted()));

        let paragraph = Paragraph::new(lines).block(block);
        paragraph.render(picker_area, buf);
    }
}
