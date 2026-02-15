//! Custom widgets for the TUI

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthStr;

use super::shimmer::shimmer_spans;
use super::theme::Theme;

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
    /// Elapsed time since processing started (for "Thinking... 3.2s" display)
    pub elapsed_secs: Option<f64>,
}

impl Widget for StatusBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = if self.is_processing {
            let spinner = super::shimmer::AnimatedDots::new(self.spinner_frame);
            let elapsed_str = if let Some(secs) = self.elapsed_secs {
                if secs < 60.0 {
                    format!(" ({:.1}s)", secs)
                } else {
                    let mins = (secs / 60.0).floor() as u64;
                    let remaining = secs - (mins as f64 * 60.0);
                    format!(" ({}m {:.0}s)", mins, remaining)
                }
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(format!("{} ", spinner.current()), Theme::accent()),
                Span::styled(&self.status_text, Theme::accent()),
                Span::styled(elapsed_str, Theme::dim()),
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

/// Sanitize text for terminal display:
/// - Expand tabs to spaces (4-space tab stops)
/// - Remove carriage returns
/// - Replace other control chars with space
fn sanitize_for_display(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut col = 0usize;
    for c in text.chars() {
        match c {
            '\t' => {
                // Expand to next 4-column tab stop
                let spaces = 4 - (col % 4);
                for _ in 0..spaces {
                    out.push(' ');
                }
                col += spaces;
            }
            '\r' => {} // skip
            '\n' => {
                out.push('\n');
                col = 0;
            }
            c if c.is_control() => {
                out.push(' ');
                col += 1;
            }
            c => {
                out.push(c);
                col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            }
        }
    }
    out
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

    // Sanitize content: expand tabs, remove control chars
    let content = sanitize_for_display(&msg.content);

    // Codex-style role prefix: minimal markers instead of heavy badges
    let (role_prefix, role_style) = match msg.role {
        MessageRole::User => ("› ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::DIM)),
        MessageRole::Codex => ("", Style::default()),
        MessageRole::Thinking => ("", Theme::muted()),
        MessageRole::CommandExec => ("", Style::default().fg(Color::Yellow)),
        MessageRole::FileChange => ("", Style::default().fg(Color::Cyan)),
        MessageRole::Gugugaga => ("▹ ", Style::default().fg(Color::Magenta).add_modifier(Modifier::DIM)),
        MessageRole::Correction => ("! ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
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

    /// Helper: add plain text content with wrapping (no markdown)
    fn add_plain(
        lines: &mut Vec<Line<'static>>,
        content: &str,
        style: Style,
        avail: usize,
        indent: &str,
    ) {
        for raw_line in content.lines() {
            for wrapped in wrap_content(raw_line, avail) {
                lines.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled(wrapped, style),
                ]));
            }
        }
    }

    /// Render markdown content using pulldown-cmark, with styles aligned to Codex.
    ///
    /// Styles (matching Codex's `markdown_render.rs`):
    ///   inline code  → Cyan
    ///   bold         → Bold
    ///   italic       → Italic
    ///   strikethrough→ CrossedOut
    ///   heading H1   → Bold + Underlined, prefixed with `# `
    ///   heading H2   → Bold, prefixed with `## `
    ///   heading H3   → Bold + Italic, prefixed with `### `
    ///   heading H4-6 → Italic
    ///   link text    → Cyan + Underlined
    ///   blockquote   → Green, prefixed with `> `
    ///   list markers → ordered: LightBlue, unordered: `- `
    ///   code blocks  → no wrapping, blank line before/after
    ///   hr           → `———`
    fn add_markdown(
        lines: &mut Vec<Line<'static>>,
        content: &str,
        avail: usize,
        indent: &str,
    ) {
        use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, HeadingLevel};

        // ── Codex-aligned styles ──
        let style_code = Style::default().fg(Color::Cyan);
        let style_bold = Style::default().add_modifier(Modifier::BOLD);
        let style_italic = Style::default().add_modifier(Modifier::ITALIC);
        let style_strikethrough = Style::default().add_modifier(Modifier::CROSSED_OUT);
        let style_link = Style::default().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
        let style_blockquote = Style::default().fg(Color::Green);
        let style_ol_marker = Style::default().fg(Color::LightBlue);

        fn heading_style(level: HeadingLevel) -> Style {
            match level {
                HeadingLevel::H1 => Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                HeadingLevel::H2 => Style::default().add_modifier(Modifier::BOLD),
                HeadingLevel::H3 => Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC),
                _ => Style::default().add_modifier(Modifier::ITALIC),
            }
        }

        // ── State ──
        let mut style_stack: Vec<Style> = Vec::new();
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        let mut in_code_block = false;
        let mut in_blockquote = false;
        let mut list_indices: Vec<Option<u64>> = Vec::new();
        let mut needs_blank = false;
        let mut link_url: Option<String> = None;

        let current_style = |stack: &[Style]| -> Style {
            stack.last().copied().unwrap_or_default()
        };

        // Flush current_spans into `lines`, with wrapping for non-code-block content.
        let flush = |lines: &mut Vec<Line<'static>>,
                     spans: &mut Vec<Span<'static>>,
                     avail: usize,
                     indent: &str,
                     is_code_block: bool| {
            if spans.is_empty() {
                return;
            }
            let built = std::mem::take(spans);
            // Compute total display width
            let total_w: usize = built.iter().map(|s| s.content.as_ref().width()).sum();

            if is_code_block || total_w <= avail {
                let mut out = vec![Span::raw(indent.to_string())];
                out.extend(built);
                lines.push(Line::from(out));
            } else {
                // Style-preserving character-level wrapping
                let mut chars_styles: Vec<(char, Style)> = Vec::new();
                for s in &built {
                    for c in s.content.chars() {
                        chars_styles.push((c, s.style));
                    }
                }
                let mut cur: Vec<Span<'static>> = vec![Span::raw(indent.to_string())];
                let mut w = 0usize;
                for (c, sty) in chars_styles {
                    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
                    if w + cw > avail && w > 0 {
                        lines.push(Line::from(std::mem::take(&mut cur)));
                        cur = vec![Span::raw(indent.to_string())];
                        w = 0;
                    }
                    if let Some(last) = cur.last_mut() {
                        if last.style == sty {
                            let mut s = last.content.to_string();
                            s.push(c);
                            *last = Span::styled(s, sty);
                        } else {
                            cur.push(Span::styled(c.to_string(), sty));
                        }
                    } else {
                        cur.push(Span::styled(c.to_string(), sty));
                    }
                    w += cw;
                }
                if cur.len() > 1 || (cur.len() == 1 && cur[0].content != indent) {
                    lines.push(Line::from(cur));
                }
            }
        };

        let push_blank = |lines: &mut Vec<Line<'static>>, indent: &str| {
            lines.push(Line::from(Span::raw(indent.to_string())));
        };

        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        let parser = Parser::new_ext(content, opts);

        for event in parser {
            match event {
                // ── Block starts ──
                Event::Start(Tag::Paragraph) => {
                    if needs_blank {
                        push_blank(lines, indent);
                    }
                    needs_blank = false;
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    if needs_blank {
                        push_blank(lines, indent);
                    }
                    needs_blank = false;
                    let hs = heading_style(level);
                    let prefix = format!("{} ", "#".repeat(level as usize));
                    current_spans.push(Span::styled(prefix, hs));
                    style_stack.push(hs);
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    if needs_blank {
                        push_blank(lines, indent);
                    }
                    in_blockquote = true;
                    needs_blank = false;
                }
                Event::Start(Tag::CodeBlock(_kind)) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    if needs_blank || !lines.is_empty() {
                        push_blank(lines, indent);
                    }
                    in_code_block = true;
                    needs_blank = false;
                }
                Event::Start(Tag::List(start)) => {
                    if list_indices.is_empty() && needs_blank {
                        push_blank(lines, indent);
                    }
                    list_indices.push(start);
                    needs_blank = false;
                }
                Event::Start(Tag::Item) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    let depth = list_indices.len();
                    let pad = "  ".repeat(depth.saturating_sub(1));
                    if let Some(Some(idx)) = list_indices.last_mut() {
                        current_spans.push(Span::styled(
                            format!("{}{}. ", pad, idx),
                            style_ol_marker,
                        ));
                        *idx += 1;
                    } else {
                        current_spans.push(Span::raw(format!("{}- ", pad)));
                    }
                }
                Event::Start(Tag::Emphasis) => {
                    let merged = current_style(&style_stack).patch(style_italic);
                    style_stack.push(merged);
                }
                Event::Start(Tag::Strong) => {
                    let merged = current_style(&style_stack).patch(style_bold);
                    style_stack.push(merged);
                }
                Event::Start(Tag::Strikethrough) => {
                    let merged = current_style(&style_stack).patch(style_strikethrough);
                    style_stack.push(merged);
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link_url = Some(dest_url.to_string());
                    let merged = current_style(&style_stack).patch(style_link);
                    style_stack.push(merged);
                }

                // ── Block ends ──
                Event::End(TagEnd::Paragraph) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    needs_blank = true;
                }
                Event::End(TagEnd::Heading(_)) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    style_stack.pop();
                    needs_blank = true;
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    in_blockquote = false;
                    needs_blank = true;
                }
                Event::End(TagEnd::CodeBlock) => {
                    flush(lines, &mut current_spans, avail, indent, true);
                    in_code_block = false;
                    needs_blank = true;
                }
                Event::End(TagEnd::List(_)) => {
                    list_indices.pop();
                    needs_blank = true;
                }
                Event::End(TagEnd::Item) => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                }
                Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {
                    style_stack.pop();
                }
                Event::End(TagEnd::Link) => {
                    style_stack.pop();
                    if let Some(url) = link_url.take() {
                        current_spans.push(Span::raw(" ("));
                        current_spans.push(Span::styled(url, style_link));
                        current_spans.push(Span::raw(")"));
                    }
                }

                // ── Inline content ──
                Event::Text(text) => {
                    let sty = if in_blockquote {
                        current_style(&style_stack).patch(style_blockquote)
                    } else {
                        current_style(&style_stack)
                    };
                    // Handle multi-line text (e.g. in code blocks)
                    for (i, line_text) in text.lines().enumerate() {
                        if i > 0 || (in_code_block && !current_spans.is_empty()) {
                            flush(lines, &mut current_spans, avail, indent, in_code_block);
                        }
                        if in_blockquote && current_spans.is_empty() {
                            current_spans.push(Span::styled("> ", style_blockquote));
                        }
                        current_spans.push(Span::styled(line_text.to_string(), sty));
                    }
                }
                Event::Code(code) => {
                    // Inline code → Cyan (matching Codex)
                    current_spans.push(Span::styled(code.to_string(), style_code));
                }
                Event::SoftBreak | Event::HardBreak => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                }
                Event::Rule => {
                    flush(lines, &mut current_spans, avail, indent, in_code_block);
                    if needs_blank {
                        push_blank(lines, indent);
                    }
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::raw("———"),
                    ]));
                    needs_blank = true;
                }
                // Table / HTML / footnotes — render as plain text
                Event::Html(html) | Event::InlineHtml(html) => {
                    current_spans.push(Span::raw(html.to_string()));
                }
                _ => {}
            }
        }
        // Final flush
        flush(lines, &mut current_spans, avail, indent, in_code_block);
    }

    if msg.role == MessageRole::System {
        // System messages — dim italic, no badge header
        let style = Theme::dim().add_modifier(Modifier::ITALIC);
        add_plain(&mut lines, &content, style, text_avail, indent);
    } else if msg.role == MessageRole::CommandExec {
        // ── Codex-style command execution rendering ──
        // Content format: "$ {command}\n{output}\n✓ exit 0 • 50ms"
        //
        // Rendered as:
        //   • Ran {command}                (green/red/yellow bullet)
        //     │ {continuation lines}       (for long commands)
        //     └ {output line 1}            (dim)
        //       {output line 2}
        //   ✓ • 50ms                       (green/red status)

        // Parse content sections
        let mut cmd_text = String::new();
        let mut output_lines_vec: Vec<&str> = Vec::new();
        let mut status_line: Option<&str> = None;

        for line in content.lines() {
            if line.starts_with("$ ") {
                cmd_text = line[2..].to_string();
            } else if line.starts_with('\u{2713}') || line.starts_with('\u{2717}') {
                // ✓ or ✗
                status_line = Some(line);
            } else {
                output_lines_vec.push(line);
            }
        }

        let is_completed = status_line.is_some();
        let is_failed = status_line.map(|s| s.starts_with('\u{2717}')).unwrap_or(false);

        let bullet_style = if is_failed {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if is_completed {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        };
        let title = if is_completed { "Ran " } else { "Running " };

        // Header line: • Ran/Running {command first part}
        if !cmd_text.is_empty() {
            let prefix_display_w = 2 + title.len(); // "• " + title
            let cmd_avail = text_avail.saturating_sub(prefix_display_w);
            let cmd_wrapped = wrap_content(&cmd_text, cmd_avail);

            if let Some(first) = cmd_wrapped.first() {
                lines.push(Line::from(vec![
                    Span::styled("• ", bullet_style),
                    Span::styled(title.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(first.clone(), Style::default().add_modifier(Modifier::DIM)),
                ]));
            }
            // Continuation lines for long commands
            for wrapped_line in cmd_wrapped.iter().skip(1) {
                lines.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled("  │ ", Style::default().add_modifier(Modifier::DIM)),
                    Span::styled(wrapped_line.clone(), Style::default().add_modifier(Modifier::DIM)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("• ", bullet_style),
                Span::styled(title.trim().to_string(), Style::default().add_modifier(Modifier::BOLD)),
            ]));
        }

        // Output block with └ prefix
        let non_empty_output: Vec<&&str> = output_lines_vec.iter()
            .filter(|l| !l.is_empty())
            .collect();
        if !non_empty_output.is_empty() {
            let output_avail = text_avail.saturating_sub(4); // "  └ " or "    " prefix
            for (i, &&out_line) in non_empty_output.iter().enumerate() {
                let (initial_prefix, continuation_prefix) = if i == 0 {
                    ("  └ ", "    ")
                } else {
                    ("    ", "    ")
                };
                let out_wrapped = wrap_content(out_line, output_avail);
                for (j, wrapped) in out_wrapped.iter().enumerate() {
                    let pfx = if j == 0 { initial_prefix } else { continuation_prefix };
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(pfx.to_string(), Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(wrapped.clone(), Style::default().add_modifier(Modifier::DIM)),
                    ]));
                }
            }
        } else if is_completed {
            // (no output)
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled("  └ ", Style::default().add_modifier(Modifier::DIM)),
                Span::styled("(no output)", Style::default().add_modifier(Modifier::DIM)),
            ]));
        }

        // Status line: ✓ • 50ms or ✗ (1) • 50ms
        if let Some(status) = status_line {
            let style = if status.starts_with('\u{2713}') {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            };
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(status.to_string(), style),
            ]));
        }
    } else if msg.role == MessageRole::Thinking {
        // ── Codex-style thinking/reasoning rendering ──
        // Dim + italic text with • bullet prefix (matching ReasoningSummaryCell)
        let dim_italic = Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC);

        // Render markdown first (no indent — we add our own prefix)
        let mut md_lines: Vec<Line<'static>> = Vec::new();
        add_markdown(&mut md_lines, &content, text_avail.saturating_sub(2), "");

        for (i, line) in md_lines.into_iter().enumerate() {
            let prefix = if i == 0 {
                format!("{}• ", indent)
            } else {
                format!("{}  ", indent)
            };

            let mut new_spans = vec![Span::styled(
                prefix,
                Style::default().add_modifier(Modifier::DIM),
            )];
            for span in line.spans {
                new_spans.push(span.patch_style(dim_italic));
            }
            lines.push(Line::from(new_spans));
        }
    } else {
        // Codex-style rendering: role prefix on first content line, no separate badge row
        match msg.role {
            MessageRole::FileChange => {
                render_file_change_diff(&content, text_avail, indent, &mut lines);
            }
            MessageRole::Correction => {
                // Correction prefix on first line
                if !role_prefix.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(role_prefix, role_style),
                    ]));
                }
                add_markdown(&mut lines, &content, text_avail, indent);
            }
            MessageRole::User => {
                // Codex-style user message: background color card with "› " prefix
                // Subtle overlay: slightly lighter than typical dark terminal backgrounds
                let bg_color = Color::Rgb(55, 55, 60);
                let user_bg = Style::default().bg(bg_color);
                let user_text = Style::default().fg(Color::White).bg(bg_color);
                let prefix_style = Style::default()
                    .fg(Color::Cyan)
                    .bg(bg_color)
                    .add_modifier(Modifier::BOLD);

                // Top padding line with background
                lines.push(Line::from(Span::styled(
                    " ".repeat(max_width),
                    user_bg,
                )));

                let wrapped = wrap_content(&content, text_avail.saturating_sub(2));
                for (i, line_text) in wrapped.iter().enumerate() {
                    let text_len = line_text.width();
                    let pad_len = text_avail.saturating_sub(2).saturating_sub(text_len);
                    if i == 0 {
                        lines.push(Line::from(vec![
                            Span::styled(indent.to_string(), user_bg),
                            Span::styled("› ", prefix_style),
                            Span::styled(line_text.clone(), user_text),
                            Span::styled(" ".repeat(pad_len), user_bg),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(indent.to_string(), user_bg),
                            Span::styled("  ", user_bg),
                            Span::styled(line_text.clone(), user_text),
                            Span::styled(" ".repeat(pad_len), user_bg),
                        ]));
                    }
                }

                // Bottom padding line with background
                lines.push(Line::from(Span::styled(
                    " ".repeat(max_width),
                    user_bg,
                )));
            }
            MessageRole::Gugugaga => {
                // Gugugaga messages: "▹ " prefix inline with first line
                let mut md_lines = Vec::new();
                add_markdown(&mut md_lines, &content, text_avail.saturating_sub(2), "  ");
                for (i, line) in md_lines.into_iter().enumerate() {
                    if i == 0 {
                        let mut spans = vec![
                            Span::raw(indent.to_string()),
                            Span::styled(role_prefix, role_style),
                        ];
                        spans.extend(line.spans.into_iter().skip_while(|s| s.content.is_empty()));
                        lines.push(Line::from(spans));
                    } else {
                        lines.push(line);
                    }
                }
            }
            _ => {
                // Codex — clean markdown, no prefix (like Codex's "• " on first line)
                add_markdown(&mut lines, &content, text_avail, indent);
            }
        }
    }

    // Spacing line
    lines.push(Line::from(""));
    lines
}

/// Codex-style file change diff rendering.
///
/// Parses unified diff content and renders:
/// - File header: `• Edited filename (+N -M)` with colored counts
/// - Line-numbered diff with colored +/- lines and gutter
/// - Hunk separators with `⋮`
fn render_file_change_diff(
    content: &str,
    text_avail: usize,
    indent: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let style_gutter = Style::default().add_modifier(Modifier::DIM);
    let style_add = Style::default().fg(Color::Green);
    let style_del = Style::default().fg(Color::Red);
    let style_context = Style::default();

    // Parse the content into per-file diff blocks
    let blocks = parse_diff_blocks(content);

    if blocks.is_empty() {
        // Fallback: if we can't parse, show raw content dimmed
        for raw_line in content.lines() {
            if raw_line.starts_with("[fc:") || raw_line.starts_with("[turn diff]") {
                continue;
            }
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(raw_line.to_string(), Theme::dim()),
            ]));
        }
        return;
    }

    let file_count = blocks.len();
    let total_added: usize = blocks.iter().map(|b| b.added).sum();
    let total_removed: usize = blocks.iter().map(|b| b.removed).sum();

    // Summary header for multi-file changes
    if file_count > 1 {
        let noun = if file_count == 1 { "file" } else { "files" };
        lines.push(Line::from(vec![
            Span::styled(format!("{}• ", indent), style_gutter),
            Span::styled("Edited".to_string(), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {} {} ", file_count, noun)),
            Span::raw("("),
            Span::styled(format!("+{}", total_added), style_add),
            Span::raw(" "),
            Span::styled(format!("-{}", total_removed), style_del),
            Span::raw(")"),
        ]));
    }

    for (block_idx, block) in blocks.iter().enumerate() {
        if block_idx > 0 {
            lines.push(Line::from(""));
        }

        // Per-file header
        let verb = match block.change_type {
            DiffChangeType::Add => "Added",
            DiffChangeType::Delete => "Deleted",
            DiffChangeType::Update => "Edited",
        };

        let file_prefix = if file_count > 1 {
            format!("{}  └ ", indent)
        } else {
            format!("{}• ", indent)
        };

        lines.push(Line::from(vec![
            Span::styled(file_prefix, style_gutter),
            Span::styled(verb.to_string(), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {} ", block.filename)),
            Span::raw("("),
            Span::styled(format!("+{}", block.added), style_add),
            Span::raw(" "),
            Span::styled(format!("-{}", block.removed), style_del),
            Span::raw(")"),
        ]));

        // Diff content with line numbers
        let gutter_width = block.max_line_number.to_string().len().max(1);
        let content_avail = text_avail.saturating_sub(gutter_width + 5); // gutter + sign + padding

        let mut is_first_hunk = true;
        for hunk in &block.hunks {
            if !is_first_hunk {
                // Hunk separator
                let spacer = format!(
                    "{}{:>width$} ",
                    indent,
                    "",
                    width = gutter_width
                );
                lines.push(Line::from(vec![
                    Span::styled(spacer, style_gutter),
                    Span::styled("⋮".to_string(), style_gutter),
                ]));
            }
            is_first_hunk = false;

            for diff_line in &hunk.lines {
                let (sign, line_style, ln) = match diff_line {
                    DiffLine::Add(ln, _) => ('+', style_add, *ln),
                    DiffLine::Delete(ln, _) => ('-', style_del, *ln),
                    DiffLine::Context(ln, _) => (' ', style_context, *ln),
                };
                let text = match diff_line {
                    DiffLine::Add(_, t) | DiffLine::Delete(_, t) | DiffLine::Context(_, t) => t,
                };

                // Wrap long lines
                let mut remaining = text.as_str();
                let mut first = true;
                loop {
                    let chunk_len = remaining
                        .char_indices()
                        .nth(content_avail)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    let (chunk, rest) = remaining.split_at(chunk_len);
                    remaining = rest;

                    if first {
                        let gutter = format!(
                            "{}{:>width$} ",
                            indent,
                            ln,
                            width = gutter_width
                        );
                        lines.push(Line::from(vec![
                            Span::styled(gutter, style_gutter),
                            Span::styled(format!("{}{}", sign, chunk), line_style),
                        ]));
                        first = false;
                    } else {
                        let gutter = format!(
                            "{}{:>width$}  ",
                            indent,
                            "",
                            width = gutter_width
                        );
                        lines.push(Line::from(vec![
                            Span::styled(gutter, style_gutter),
                            Span::styled(chunk.to_string(), line_style),
                        ]));
                    }

                    if remaining.is_empty() {
                        break;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DiffChangeType {
    Add,
    Delete,
    Update,
}

#[derive(Debug)]
enum DiffLine {
    Add(usize, String),
    Delete(usize, String),
    Context(usize, String),
}

#[derive(Debug)]
struct DiffHunk {
    lines: Vec<DiffLine>,
}

#[derive(Debug)]
struct DiffBlock {
    filename: String,
    change_type: DiffChangeType,
    added: usize,
    removed: usize,
    max_line_number: usize,
    hunks: Vec<DiffHunk>,
}

/// Parse diff content into structured blocks.
/// Handles both:
/// - Our custom format: `• Edited/Added/Deleted path\n<unified diff>`
/// - Raw unified diff format: `diff --git a/path b/path\n---\n+++\n@@...`
fn parse_diff_blocks(content: &str) -> Vec<DiffBlock> {
    let mut blocks = Vec::new();
    let raw_lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < raw_lines.len() {
        let line = raw_lines[i];

        // Skip internal markers
        if line.starts_with("[fc:") || line.starts_with("[turn diff]") {
            i += 1;
            continue;
        }

        // Our custom format: "• Verb path"
        if line.starts_with('\u{2022}') {
            let rest = line.trim_start_matches('\u{2022}').trim();
            let (change_type, filename) = if let Some(name) = rest.strip_prefix("Added ") {
                (DiffChangeType::Add, name.trim().to_string())
            } else if let Some(name) = rest.strip_prefix("Deleted ") {
                (DiffChangeType::Delete, name.trim().to_string())
            } else if let Some(name) = rest.strip_prefix("Edited ") {
                (DiffChangeType::Update, name.trim().to_string())
            } else {
                (DiffChangeType::Update, rest.to_string())
            };
            i += 1;

            // Collect diff lines until next block or end
            let mut diff_text = String::new();
            while i < raw_lines.len()
                && !raw_lines[i].starts_with('\u{2022}')
                && !raw_lines[i].starts_with("[fc:")
                && !raw_lines[i].starts_with("diff ")
            {
                diff_text.push_str(raw_lines[i]);
                diff_text.push('\n');
                i += 1;
            }

            let (hunks, added, removed, max_ln) = parse_unified_diff(&diff_text, change_type);
            blocks.push(DiffBlock {
                filename,
                change_type,
                added,
                removed,
                max_line_number: max_ln,
                hunks,
            });
            continue;
        }

        // Raw unified diff: "diff --git a/path b/path"
        if line.starts_with("diff ") {
            let filename = extract_filename_from_diff_header(line);
            i += 1;
            // Skip --- and +++ lines
            while i < raw_lines.len()
                && (raw_lines[i].starts_with("---") || raw_lines[i].starts_with("+++"))
            {
                i += 1;
            }

            // Collect @@ hunks
            let mut diff_text = String::new();
            while i < raw_lines.len()
                && !raw_lines[i].starts_with("diff ")
                && !raw_lines[i].starts_with('\u{2022}')
                && !raw_lines[i].starts_with("[fc:")
            {
                diff_text.push_str(raw_lines[i]);
                diff_text.push('\n');
                i += 1;
            }

            let (hunks, added, removed, max_ln) = parse_unified_diff(&diff_text, DiffChangeType::Update);
            let change_type = if removed == 0 && added > 0 {
                DiffChangeType::Add
            } else if added == 0 && removed > 0 {
                DiffChangeType::Delete
            } else {
                DiffChangeType::Update
            };
            blocks.push(DiffBlock {
                filename,
                change_type,
                added,
                removed,
                max_line_number: max_ln,
                hunks,
            });
            continue;
        }

        // Skip unrecognized lines
        i += 1;
    }

    blocks
}

/// Check if a line is git diff metadata that should be skipped.
fn is_diff_metadata(line: &str) -> bool {
    line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("index ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
        || line.starts_with("diff --git ")
}

/// Parse unified diff text (starting from @@ lines) into hunks with line numbers.
/// `hint` indicates the known change type from the header (Add/Delete/Update),
/// used to correctly interpret raw content (no @@ headers) as additions or deletions.
fn parse_unified_diff(text: &str, hint: DiffChangeType) -> (Vec<DiffHunk>, usize, usize, usize) {
    let mut hunks = Vec::new();
    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    let mut max_line_number = 0usize;

    let mut current_hunk_lines: Vec<DiffLine> = Vec::new();
    let mut old_ln = 1usize;
    let mut new_ln = 1usize;
    let mut seen_hunk_header = false;

    for line in text.lines() {
        // Skip git diff metadata
        if is_diff_metadata(line) {
            continue;
        }
        if line.starts_with("@@") {
            seen_hunk_header = true;
            // Flush previous hunk
            if !current_hunk_lines.is_empty() {
                hunks.push(DiffHunk {
                    lines: std::mem::take(&mut current_hunk_lines),
                });
            }
            // Parse @@ -old_start,old_count +new_start,new_count @@
            if let Some((old_start, new_start)) = parse_hunk_header(line) {
                old_ln = old_start;
                new_ln = new_start;
            }
        } else if let Some(rest) = line.strip_prefix('+') {
            max_line_number = max_line_number.max(new_ln);
            current_hunk_lines.push(DiffLine::Add(new_ln, rest.to_string()));
            new_ln += 1;
            total_added += 1;
        } else if let Some(rest) = line.strip_prefix('-') {
            max_line_number = max_line_number.max(old_ln);
            current_hunk_lines.push(DiffLine::Delete(old_ln, rest.to_string()));
            old_ln += 1;
            total_removed += 1;
        } else if line.starts_with(' ') || (!line.is_empty() && !line.starts_with('\\')) {
            let text = if line.starts_with(' ') { &line[1..] } else { line };
            max_line_number = max_line_number.max(new_ln);
            current_hunk_lines.push(DiffLine::Context(new_ln, text.to_string()));
            old_ln += 1;
            new_ln += 1;
        }
        // Skip "\ No newline at end of file" and empty lines
    }

    // Flush last hunk
    if !current_hunk_lines.is_empty() {
        hunks.push(DiffHunk {
            lines: current_hunk_lines,
        });
    }

    // If no @@ hunk header was found, the content is raw text (e.g. new/deleted file).
    // Convert context lines to Add (new file) or Delete (deleted file) based on hint.
    if !seen_hunk_header && total_added == 0 && total_removed == 0 {
        let as_delete = matches!(hint, DiffChangeType::Delete);
        for hunk in &mut hunks {
            for dl in &mut hunk.lines {
                if let DiffLine::Context(ln, content) = dl {
                    let ln_val = *ln;
                    let content_val = std::mem::take(content);
                    if as_delete {
                        *dl = DiffLine::Delete(ln_val, content_val);
                        total_removed += 1;
                    } else {
                        *dl = DiffLine::Add(ln_val, content_val);
                        total_added += 1;
                    }
                }
            }
        }
    }

    if max_line_number == 0 {
        max_line_number = 1;
    }

    (hunks, total_added, total_removed, max_line_number)
}

/// Parse a hunk header like `@@ -1,5 +1,7 @@` and return (old_start, new_start).
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    // Format: @@ -old_start[,old_count] +new_start[,new_count] @@
    let trimmed = line.trim_start_matches('@').trim_end_matches('@').trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let old_start = parts[0]
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;
    let new_start = parts[1]
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;
    Some((old_start, new_start))
}

/// Extract filename from a "diff --git a/path b/path" header.
fn extract_filename_from_diff_header(line: &str) -> String {
    // "diff --git a/foo/bar.rs b/foo/bar.rs"
    if let Some(rest) = line.strip_prefix("diff --git ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() >= 2 {
            return parts[1].trim_start_matches("b/").to_string();
        }
        if !parts.is_empty() {
            return parts[0].trim_start_matches("a/").to_string();
        }
    }
    // Fallback
    line.to_string()
}

#[cfg(test)]
mod md_test {
    use super::*;
    
    #[test]
    fn test_markdown_rendering() {
        let content = "可以，文件在：\n\n- calculator.py:1\n\n功能包括：\n- 支持加减\n- 支持退出\n\n运行：\n\n```bash\npython3 calc.py\n```";
        let msg = Message::codex(content);
        let lines = render_message_lines(&msg, 80);
        // Badge + paragraph + blank + list item + blank + paragraph + blank
        // + 2 list items + blank + paragraph + blank + code line + spacing = 14
        assert!(lines.len() > 5, "Expected multiple lines, got {}", lines.len());
        // Check list items are on separate lines
        let texts: Vec<String> = lines.iter().map(|l| {
            l.spans.iter().map(|s| s.content.as_ref().to_string()).collect()
        }).collect();
        assert!(texts.iter().any(|t| t.contains("- 支持加减")));
        assert!(texts.iter().any(|t| t.contains("- 支持退出")));
        assert!(texts.iter().any(|t| t.contains("python3 calc.py")));
    }

    #[test]
    fn test_sanitize_preserves_newlines() {
        let input = "Hello\nWorld\n\n- item";
        let output = sanitize_for_display(input);
        assert_eq!(output, input, "sanitize_for_display should preserve newlines");
    }
}
