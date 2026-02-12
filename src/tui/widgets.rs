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
    } else {
        // Badge header line
        lines.push(Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::styled(format!(" {}", msg.timestamp), Theme::muted()),
        ]));

        // Content
        match msg.role {
            MessageRole::Thinking => {
                add_markdown(&mut lines, &content, text_avail, indent);
            }
            MessageRole::CommandExec => {
                let style = Style::default().fg(Color::DarkGray);
                add_plain(&mut lines, &content, style, text_avail, indent);
            }
            MessageRole::FileChange => {
                // Diff lines — wrap them too so they don't break the border
                for raw_line in content.lines() {
                    // Skip internal markers used for message replacement
                    if raw_line.starts_with("[fc:") || raw_line.starts_with("[turn diff]") {
                        continue;
                    }
                    let style = if raw_line.starts_with('+') {
                        Style::default().fg(Color::Green)
                    } else if raw_line.starts_with('-') {
                        Style::default().fg(Color::Red)
                    } else if raw_line.starts_with("@@") {
                        Style::default().fg(Color::Cyan)
                    } else if raw_line.starts_with("•") {
                        // File path header (e.g. "• Edited src/foo.rs")
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else if raw_line.starts_with("diff ") || raw_line.starts_with("---") || raw_line.starts_with("+++") {
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
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
                add_markdown(&mut lines, &content, text_avail, indent);
            }
            _ => {
                // User, Codex, Gugugaga — use markdown for agent messages
                if matches!(msg.role, MessageRole::Codex | MessageRole::Gugugaga) {
                    add_markdown(&mut lines, &content, text_avail, indent);
                } else {
                    let style = Theme::text();
                    add_plain(&mut lines, &content, style, text_avail, indent);
                }
            }
        }
    }

    // Spacing line
    lines.push(Line::from(""));
    lines
}
