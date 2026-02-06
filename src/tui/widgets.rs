//! Custom widgets for the TUI

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use super::shimmer::shimmer_spans;
use super::theme::Theme;

/// Parse simple Markdown formatting and return styled spans
/// Supports: **bold**, *italic*, `code`, ***bold italic***
fn parse_markdown(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text.to_string();
    
    // Regex patterns for markdown
    let bold_italic = Regex::new(r"\*\*\*(.+?)\*\*\*").unwrap();
    let bold = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    let italic = Regex::new(r"\*([^*]+?)\*").unwrap();
    let code = Regex::new(r"`([^`]+?)`").unwrap();
    
    // Process patterns in order of priority
    let patterns: Vec<(&Regex, Style)> = vec![
        (&bold_italic, base_style.add_modifier(Modifier::BOLD | Modifier::ITALIC)),
        (&bold, base_style.add_modifier(Modifier::BOLD)),
        (&code, Style::default().fg(Color::Yellow)),
        (&italic, base_style.add_modifier(Modifier::ITALIC)),
    ];
    
    // Simple approach: find first match, split, recurse
    fn find_first_match<'a>(text: &str, patterns: &[(&'a Regex, Style)]) -> Option<(usize, usize, String, Style)> {
        let mut best: Option<(usize, usize, String, Style)> = None;
        for (regex, style) in patterns {
            if let Some(m) = regex.find(text) {
                if best.is_none() || m.start() < best.as_ref().unwrap().0 {
                    if let Some(caps) = regex.captures(text) {
                        let inner = caps.get(1).map(|c| c.as_str().to_string()).unwrap_or_default();
                        best = Some((m.start(), m.end(), inner, *style));
                    }
                }
            }
        }
        best
    }
    
    while !remaining.is_empty() {
        if let Some((start, end, inner, style)) = find_first_match(&remaining, &patterns) {
            // Add text before match
            if start > 0 {
                spans.push(Span::styled(remaining[..start].to_string(), base_style));
            }
            // Add styled match
            spans.push(Span::styled(inner, style));
            // Continue with rest
            remaining = remaining[end..].to_string();
        } else {
            // No more matches
            spans.push(Span::styled(remaining.clone(), base_style));
            break;
        }
    }
    
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
    }
    
    spans
}

/// Wrap a long string into multiple lines, respecting unicode width
fn wrap_text(text: &str, max_width: usize, indent: &str) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    
    let indent_width = indent.width();
    let content_width = max_width.saturating_sub(indent_width);
    
    if content_width == 0 || text.width() <= content_width {
        return vec![text.to_string()];
    }
    
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;
    
    for c in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        
        if current_width + char_width > content_width && !current_line.is_empty() {
            lines.push(current_line);
            current_line = String::new();
            current_width = 0;
        }
        
        current_line.push(c);
        current_width += char_width;
    }
    
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    
    lines
}

/// A message in the conversation
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Codex,
    Thinking,      // Codex reasoning/thinking
    CommandExec,   // Command execution output
    FileChange,    // File change notification
    Gugugaga,
    Correction,
    System,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn codex(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Codex,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn thinking(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Thinking,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn command_exec(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::CommandExec,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn file_change(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::FileChange,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn correction(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Correction,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }

    pub fn gugugaga(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Gugugaga,
            content: content.into(),
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
        }
    }
}

/// Header bar with title and project info
pub struct HeaderBar<'a> {
    pub title: &'a str,
    pub project: &'a str,
    pub is_processing: bool,
    #[allow(dead_code)]
    pub spinner_frame: usize,
}

impl Widget for HeaderBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        // Logo/title with shimmer effect when processing
        let title_spans = if self.is_processing {
            let mut spans = vec![Span::styled("◆ ", Theme::accent())];
            spans.extend(shimmer_spans(self.title));
            spans
        } else {
            vec![
                Span::styled("◆ ", Theme::accent()),
                Span::styled(self.title, Theme::title()),
            ]
        };

        let title_line = Line::from(title_spans);
        buf.set_line(area.x + 1, area.y, &title_line, area.width.saturating_sub(2));

        // Project name on the right
        let project_str = format!("📁 {} ", self.project);
        let project_len = project_str.chars().count() as u16;
        let project_x = area.x + area.width.saturating_sub(project_len + 1);
        let project_span = Span::styled(project_str, Theme::muted());
        buf.set_span(project_x, area.y, &project_span, project_len + 1);
    }
}

/// Status bar showing current state
pub struct StatusBar {
    pub is_processing: bool,
    pub spinner_frame: usize,
    pub status_text: String,
}

impl Widget for StatusBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = if self.is_processing {
            let spinner = super::shimmer::AnimatedDots::new(self.spinner_frame);
            Line::from(vec![
                Span::styled(format!("{} ", spinner.current()), Theme::accent()),
                Span::styled(&self.status_text, Theme::accent()),
            ])
        } else {
            Line::from(vec![
                Span::styled("● ", Theme::success()),
                Span::styled("Ready", Theme::dim()),
            ])
        };

        buf.set_line(area.x + 1, area.y, &line, area.width.saturating_sub(2));
    }
}

/// Stats panel showing gugugaga metrics
#[allow(dead_code)]
pub struct StatsPanel {
    pub violations: usize,
    pub corrections: usize,
    pub auto_replies: usize,
    pub is_monitoring: bool,
}

impl Widget for StatsPanel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the entire area first
        buf.set_style(area, Style::default());
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                buf[(x, y)].set_char(' ');
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::border())
            .title(Span::styled(" Stats ", Theme::muted()));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        // First show monitoring status
        let status_line = if self.is_monitoring {
            Line::from(vec![
                Span::styled("🛡️ ", Style::default()),
                Span::styled("Monitoring", Theme::success()),
            ])
        } else {
            Line::from(vec![
                Span::styled("○ ", Theme::dim()),
                Span::styled("Standby", Theme::dim()),
            ])
        };
        buf.set_line(inner.x, inner.y, &status_line, inner.width);

        let stats = vec![
            ("Violations", self.violations, if self.violations > 0 { Theme::warning() } else { Theme::success() }),
            ("Corrections", self.corrections, Theme::info()),
            ("Auto-replies", self.auto_replies, Theme::accent()),
        ];

        for (i, (label, value, style)) in stats.iter().enumerate() {
            let y_offset = i as u16 + 1; // +1 for status line
            if y_offset >= inner.height {
                break;
            }

            let line = Line::from(vec![
                Span::styled(format!("{}: ", label), Theme::dim()),
                Span::styled(value.to_string(), *style),
            ]);
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
        }
    }
}

/// Context panel showing Gugugaga notebook state
pub struct ContextPanel {
    /// Current activity Codex is doing
    pub current_activity: Option<String>,
    /// Number of completed items
    pub completed_count: usize,
    /// Attention items (content, is_high_priority)
    pub attention_items: Vec<(String, bool)>,
    /// Number of mistakes recorded
    pub mistakes_count: usize,
    /// Stats
    pub violations: usize,
    #[allow(dead_code)]
    pub corrections: usize,
    /// Monitoring status
    pub is_monitoring: bool,
}

impl Default for ContextPanel {
    fn default() -> Self {
        Self {
            current_activity: None,
            completed_count: 0,
            attention_items: Vec::new(),
            mistakes_count: 0,
            violations: 0,
            corrections: 0,
            is_monitoring: false,
        }
    }
}

impl Widget for ContextPanel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the entire area first
        buf.set_style(area, Style::default());
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                buf[(x, y)].set_char(' ');
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::border())
            .title(Span::styled(" Gugugaga Context ", Theme::accent()));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        let mut y_offset = 0u16;
        let width = inner.width as usize;

        // Monitoring status
        let status_line = if self.is_monitoring {
            Line::from(vec![
                Span::styled("🛡️ ", Style::default()),
                Span::styled("Monitoring", Theme::success()),
            ])
        } else {
            Line::from(vec![
                Span::styled("○ ", Theme::dim()),
                Span::styled("Standby", Theme::dim()),
            ])
        };
        buf.set_line(inner.x, inner.y + y_offset, &status_line, inner.width);
        y_offset += 1;

        // Current activity
        if let Some(activity) = self.current_activity {
            if y_offset < inner.height {
                let truncated = if activity.len() > width.saturating_sub(4) {
                    format!("{}...", &activity[..width.saturating_sub(7)])
                } else {
                    activity.to_string()
                };
                let line = Line::from(vec![
                    Span::styled("📍 ", Style::default()),
                    Span::styled(truncated, Theme::info()),
                ]);
                buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
                y_offset += 1;
            }
        }

        // Separator
        if y_offset < inner.height {
            y_offset += 1;
        }

        // Attention items (high priority first)
        let high_priority: Vec<_> = self.attention_items.iter()
            .filter(|(_, high)| *high)
            .collect();
        
        if !high_priority.is_empty() && y_offset < inner.height {
            let line = Line::from(vec![
                Span::styled("⚠️ Watch:", Theme::warning()),
            ]);
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
            y_offset += 1;

            for (content, _) in high_priority.iter().take(3) {
                if y_offset >= inner.height {
                    break;
                }
                let truncated = if content.len() > width.saturating_sub(4) {
                    format!("  {}...", &content[..width.saturating_sub(7)])
                } else {
                    format!("  {}", content)
                };
                let line = Line::from(Span::styled(truncated, Theme::dim()));
                buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
                y_offset += 1;
            }
        }

        // Separator
        if y_offset < inner.height {
            y_offset += 1;
        }

        // Stats summary
        if y_offset < inner.height {
            let line = Line::from(vec![
                Span::styled("📊 ", Style::default()),
                Span::styled(format!("{} done", self.completed_count), Theme::success()),
                Span::styled(" | ", Theme::dim()),
                Span::styled(
                    format!("{} violations", self.violations),
                    if self.violations > 0 { Theme::warning() } else { Theme::dim() }
                ),
            ]);
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
            y_offset += 1;
        }

        if y_offset < inner.height && self.mistakes_count > 0 {
            let line = Line::from(vec![
                Span::styled("   ", Style::default()),
                Span::styled(format!("{} mistakes logged", self.mistakes_count), Theme::dim()),
            ]);
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
        }
    }
}

/// Input box widget
pub struct InputBox<'a> {
    pub content: &'a str,
    #[allow(dead_code)]
    pub cursor: usize,
    pub focused: bool,
}

impl Widget for InputBox<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_style = if self.focused {
            Theme::border_focused()
        } else {
            Theme::border()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(" Message ", Theme::muted()));

        let inner = block.inner(area);
        block.render(area, buf);

        let display_text = if self.content.is_empty() {
            "Type your message... (Enter to send, Ctrl+C to quit)"
        } else {
            self.content
        };

        let style = if self.content.is_empty() {
            Theme::muted()
        } else {
            Theme::text()
        };

        let text = Paragraph::new(display_text)
            .style(style)
            .wrap(Wrap { trim: false });
        text.render(inner, buf);
    }
}

/// Help bar showing key bindings
pub struct HelpBar;

impl Widget for HelpBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bindings = [
            ("/", "Codex"),
            ("//", "Gugugaga"),
            ("Tab", "Complete"),
            ("Scroll/↑↓", "scroll"),
            ("Enter", "Send"),
            ("Ctrl+C", "Quit"),
        ];

        let mut spans = vec![Span::raw(" ")];
        for (i, (key, desc)) in bindings.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Theme::muted()));
            }
            spans.push(Span::styled(*key, Theme::key()));
            spans.push(Span::styled(format!(" {}", desc), Theme::key_desc()));
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

/// Render a single message to lines for display
/// max_width: terminal width for text wrapping (0 = no wrapping)
pub fn render_message_lines(msg: &Message, max_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let content_width = max_width.saturating_sub(4); // Account for "  " indent and some margin

    // Role badge
    let (badge_text, badge_style) = match msg.role {
        MessageRole::User => (" You ", Theme::user_badge()),
        MessageRole::Codex => (" Codex ", Theme::codex_badge()),
        MessageRole::Thinking => (" 💭 Thinking ", Theme::muted()),
        MessageRole::CommandExec => (" ⚡ Command ", Style::default().fg(Color::Yellow)),
        MessageRole::FileChange => (" 📝 File ", Style::default().fg(Color::Cyan)),
        MessageRole::Gugugaga => (" Gugugaga ", Theme::gugugaga_badge()),
        MessageRole::Correction => (" ⚠ Correction ", Theme::correction_badge()),
        MessageRole::System => ("", Theme::system_badge()),
    };

    // Helper to add wrapped content lines with markdown support
    let add_wrapped_lines = |lines: &mut Vec<Line<'static>>, content: &str, style: Style, width: usize, use_markdown: bool| {
        for content_line in content.lines() {
            let wrapped = wrap_text(content_line, width, "  ");
            for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                let indent = if i == 0 { "  " } else { "    " };
                if use_markdown {
                    let mut spans = vec![Span::raw(indent.to_string())];
                    spans.extend(parse_markdown(&wrapped_line, style));
                    lines.push(Line::from(spans));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(wrapped_line, style),
                    ]));
                }
            }
        }
    };

    if msg.role == MessageRole::System {
        // System messages - wrap long system messages too
        let style = Theme::dim().add_modifier(Modifier::ITALIC);
        let wrapped = wrap_text(&msg.content, content_width, "  ");
        for wrapped_line in wrapped {
            lines.push(Line::from(vec![
                Span::styled("  ", Theme::dim()),
                Span::styled(wrapped_line, style),
            ]));
        }
    } else if msg.role == MessageRole::Thinking {
        // Thinking messages - visible but distinct from main output
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        let style = Theme::thinking();
        add_wrapped_lines(&mut lines, &msg.content, style, content_width, true);
    } else if msg.role == MessageRole::CommandExec {
        // Command execution output - monospace style
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        let style = Style::default().fg(Color::DarkGray);
        add_wrapped_lines(&mut lines, &msg.content, style, content_width, false);
    } else if msg.role == MessageRole::FileChange {
        // File change - colored diff style (don't wrap diffs, they need to stay aligned)
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        for content_line in msg.content.lines() {
            let style = if content_line.starts_with('+') {
                Style::default().fg(Color::Green)
            } else if content_line.starts_with('-') {
                Style::default().fg(Color::Red)
            } else if content_line.starts_with('@') {
                Style::default().fg(Color::Cyan)
            } else {
                Theme::dim()
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(content_line.to_string(), style),
            ]));
        }
    } else {
        // Header line with badge and timestamp
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        // Content lines with left margin - enable markdown for Codex/Gugugaga messages
        let content_style = match msg.role {
            MessageRole::Correction => Theme::warning(),
            _ => Theme::text(),
        };
        let use_markdown = matches!(msg.role, MessageRole::Codex | MessageRole::Gugugaga | MessageRole::Correction);

        add_wrapped_lines(&mut lines, &msg.content, content_style, content_width, use_markdown);
    }

    // Add empty line for spacing
    lines.push(Line::from(""));

    lines
}
