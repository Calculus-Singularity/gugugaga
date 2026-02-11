//! ASCII art animation engine, adapted from Codex's `ascii_animation.rs`.
//!
//! Drives a looping animation from pre-rendered frame arrays embedded at
//! compile time. The engine is purely time-driven: call
//! [`AsciiAnimation::current_frame`] on every TUI draw and it returns the
//! correct frame for the elapsed wall-clock time.

use std::time::Instant;

use super::frames::{ALL_VARIANTS, FRAME_TICK};

/// Drives the ASCII art animation for the welcome screen.
pub struct AsciiAnimation {
    frames: &'static [&'static str],
    start: Instant,
}

impl AsciiAnimation {
    /// Create a new animation.
    pub fn new() -> Self {
        Self {
            frames: ALL_VARIANTS[0],
            start: Instant::now(),
        }
    }

    /// Return the frame string that should be displayed right now.
    pub fn current_frame(&self) -> &'static str {
        if self.frames.is_empty() {
            return "";
        }
        let tick_ms = FRAME_TICK.as_millis();
        if tick_ms == 0 {
            return self.frames[0];
        }
        let elapsed_ms = self.start.elapsed().as_millis();
        let idx = ((elapsed_ms / tick_ms) % self.frames.len() as u128) as usize;
        self.frames[idx]
    }
}
