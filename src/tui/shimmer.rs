//! Shimmer animation effect for text
//!
//! Creates a sweeping highlight effect across text, similar to Codex's loading animation.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::time::{Duration, Instant};
use std::sync::OnceLock;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn elapsed_since_start() -> Duration {
    let start = PROCESS_START.get_or_init(Instant::now);
    start.elapsed()
}

/// Create shimmer-animated spans from text
pub fn shimmer_spans(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos_f = (elapsed_since_start().as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32);
    let pos = pos_f as usize;
    let band_half_width = 5.0;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(chars.len());
    
    for (i, ch) in chars.iter().enumerate() {
        let i_pos = i as isize + padding as isize;
        let pos = pos as isize;
        let dist = (i_pos - pos).abs() as f32;

        let intensity = if dist <= band_half_width {
            let x = std::f32::consts::PI * (dist / band_half_width);
            0.5 * (1.0 + x.cos())
        } else {
            0.0
        };

        let style = style_for_intensity(intensity);
        spans.push(Span::styled(ch.to_string(), style));
    }

    spans
}

fn style_for_intensity(intensity: f32) -> Style {
    if intensity < 0.2 {
        Style::default().add_modifier(Modifier::DIM)
    } else if intensity < 0.6 {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Animated dots for loading indicator
pub struct AnimatedDots {
    frame: usize,
}

impl AnimatedDots {
    const FRAMES: &'static [&'static str] = &[
        "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"
    ];

    pub fn new(frame: usize) -> Self {
        Self { frame }
    }

    pub fn current(&self) -> &'static str {
        Self::FRAMES[self.frame % Self::FRAMES.len()]
    }

    pub fn next(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
}

/// Progress bar characters
pub struct ProgressBar;

impl ProgressBar {
    const FULL: char = '█';
    const EMPTY: char = '░';
    const PARTIAL: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

    pub fn render(progress: f32, width: usize) -> String {
        let progress = progress.clamp(0.0, 1.0);
        let filled = (progress * width as f32) as usize;
        let partial_idx = ((progress * width as f32 - filled as f32) * 8.0) as usize;
        
        let mut bar = String::with_capacity(width);
        for i in 0..width {
            if i < filled {
                bar.push(Self::FULL);
            } else if i == filled && partial_idx > 0 {
                bar.push(Self::PARTIAL[partial_idx.min(7)]);
            } else {
                bar.push(Self::EMPTY);
            }
        }
        bar
    }
}
