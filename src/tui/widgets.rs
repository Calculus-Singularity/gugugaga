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

/// Truncate a string to fit within `max_display_width` display columns,
/// appending "..." if truncated. Safe for multi-byte UTF-8 and wide chars.
/// Public alias for use from other modules.
pub fn truncate_to_width_str(text: &str, max_display_width: usize) -> String {
    truncate_to_width(text, max_display_width)
}

fn truncate_to_width(text: &str, max_display_width: usize) -> String {
    if text.width() <= max_display_width {
        return text.to_string();
    }
    
    let suffix = "...";
    let suffix_width = suffix.width();
    let target_width = max_display_width.saturating_sub(suffix_width);
    
    let mut result = String::new();
    let mut current_width = 0;
    
    for c in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        if current_width + char_width > target_width {
            break;
        }
        result.push(c);
        current_width += char_width;
    }
    
    result.push_str(suffix);
    result
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

        // Project name on the right — use display width, not char count
        let project_str = format!("[{}]", self.project);
        let project_display_w = project_str.width() as u16;
        let project_x = area.x + area.width.saturating_sub(project_display_w + 1);
        let project_span = Span::styled(project_str, Theme::muted());
        buf.set_span(project_x, area.y, &project_span, project_display_w + 1);
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
        let w = inner.width as usize;

        // Monitoring status
        let status_line = if self.is_monitoring {
            Line::from(vec![
                Span::styled("● ", Theme::success()),
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

        // Current activity — prefix ">> " is 3 chars wide
        if let Some(activity) = self.current_activity {
            if y_offset < inner.height {
                let prefix = ">> ";
                let prefix_w: usize = prefix.width();
                let avail = w.saturating_sub(prefix_w);
                let truncated = truncate_to_width(&activity, avail);
                let line = Line::from(vec![
                    Span::styled(prefix, Theme::accent()),
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
            let line = Line::from(Span::styled("! Watch:", Theme::warning()));
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
            y_offset += 1;

            for (content, _) in high_priority.iter().take(3) {
                if y_offset >= inner.height {
                    break;
                }
                let prefix = "  ";
                let avail = w.saturating_sub(prefix.width());
                let truncated = truncate_to_width(content, avail);
                let line = Line::from(vec![
                    Span::styled(prefix, Theme::dim()),
                    Span::styled(truncated, Theme::dim()),
                ]);
                buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
                y_offset += 1;
            }
        }

        // Separator
        if y_offset < inner.height {
            y_offset += 1;
        }

        // Stats summary — use simple ASCII, no emoji
        if y_offset < inner.height {
            let stats_text = format!("{} done | {} viol", self.completed_count, self.violations);
            let truncated = truncate_to_width(&stats_text, w);
            let style = if self.violations > 0 { Theme::warning() } else { Theme::dim() };
            let line = Line::from(Span::styled(truncated, style));
            buf.set_line(inner.x, inner.y + y_offset, &line, inner.width);
            y_offset += 1;
        }

        if y_offset < inner.height && self.mistakes_count > 0 {
            let mistakes_text = format!("{} mistakes", self.mistakes_count);
            let truncated = truncate_to_width(&mistakes_text, w);
            let line = Line::from(Span::styled(truncated, Theme::dim()));
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
/// max_width: the usable inner width of the chat area in display columns
pub fn render_message_lines(msg: &Message, max_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Indent used for content lines: 2 spaces
    let indent: &str = "  ";
    let indent_w: usize = indent.width();
    // Width available for actual text content after indent
    let text_avail = max_width.saturating_sub(indent_w);

    // Role badge (use ASCII-safe badges to avoid emoji width issues)
    let (badge_text, badge_style) = match msg.role {
        MessageRole::User => (" You ", Theme::user_badge()),
        MessageRole::Codex => (" Codex ", Theme::codex_badge()),
        MessageRole::Thinking => (" Thinking ", Theme::muted()),
        MessageRole::CommandExec => (" $ Command ", Style::default().fg(Color::Yellow)),
        MessageRole::FileChange => (" ~ File ", Style::default().fg(Color::Cyan)),
        MessageRole::Gugugaga => (" Gugugaga ", Theme::gugugaga_badge()),
        MessageRole::Correction => (" ! Correction ", Theme::correction_badge()),
        MessageRole::System => ("", Theme::system_badge()),
    };

    /// Helper: wrap one physical line of text to fit `avail` display columns,
    /// returning multiple lines if needed.
    fn wrap_content(text: &str, avail: usize) -> Vec<String> {
        if avail == 0 {
            return vec![text.to_string()];
        }
        if text.width() <= avail {
            return vec![text.to_string()];
        }
        let mut result = Vec::new();
        let mut line = String::new();
        let mut w = 0usize;
        for c in text.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            if w + cw > avail && !line.is_empty() {
                result.push(line);
                line = String::new();
                w = 0;
            }
            line.push(c);
            w += cw;
        }
        if !line.is_empty() {
            result.push(line);
        }
        result
    }

    /// Helper: add wrapped + optionally markdown-parsed content lines
    fn add_content(
        lines: &mut Vec<Line<'static>>,
        content: &str,
        style: Style,
        avail: usize,
        indent: &str,
        use_markdown: bool,
    ) {
        for raw_line in content.lines() {
            for wrapped in wrap_content(raw_line, avail) {
                if use_markdown {
                    let mut spans = vec![Span::raw(indent.to_string())];
                    spans.extend(parse_markdown(&wrapped, style));
                    lines.push(Line::from(spans));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(wrapped, style),
                    ]));
                }
            }
        }
    }

    if msg.role == MessageRole::System {
        // System messages — dim italic, no badge header
        let style = Theme::dim().add_modifier(Modifier::ITALIC);
        add_content(&mut lines, &msg.content, style, text_avail, indent, false);
    } else {
        // Badge header line
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        // Content
        match msg.role {
            MessageRole::Thinking => {
                let style = Theme::thinking();
                add_content(&mut lines, &msg.content, style, text_avail, indent, true);
            }
            MessageRole::CommandExec => {
                let style = Style::default().fg(Color::DarkGray);
                add_content(&mut lines, &msg.content, style, text_avail, indent, false);
            }
            MessageRole::FileChange => {
                // Diff lines — wrap them too so they don't break the border
                for raw_line in msg.content.lines() {
                    let style = if raw_line.starts_with('+') {
                        Style::default().fg(Color::Green)
                    } else if raw_line.starts_with('-') {
                        Style::default().fg(Color::Red)
                    } else if raw_line.starts_with('@') {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Theme::dim()
                    };
                    for wrapped in wrap_content(raw_line, text_avail) {
                        lines.push(Line::from(vec![
                            Span::raw(indent.to_string()),
                            Span::styled(wrapped, style),
                        ]));
                    }
                }
            }
            MessageRole::Correction => {
                let style = Theme::warning();
                add_content(&mut lines, &msg.content, style, text_avail, indent, true);
            }
            _ => {
                // User, Codex, Gugugaga
                let style = Theme::text();
                let use_md = matches!(msg.role, MessageRole::Codex | MessageRole::Gugugaga);
                add_content(&mut lines, &msg.content, style, text_avail, indent, use_md);
            }
        }
    }

    // Spacing line
    lines.push(Line::from(""));
    lines
}
