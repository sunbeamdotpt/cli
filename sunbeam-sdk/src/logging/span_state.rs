//! Types for the threaded (indicatif-backed) log layer.

use indicatif::ProgressBar;
use std::collections::VecDeque;

/// Per-span render state used by the threaded layer.
pub struct SpanBar {
    /// The progress bar representing this span.
    pub pb: ProgressBar,
    /// Ring buffer of recent log lines.
    pub buffer: VecDeque<String>,
    /// Max lines to keep in the ring buffer.
    pub max_lines: usize,
}

impl SpanBar {
    pub fn new(pb: ProgressBar) -> Self {
        Self {
            pb,
            buffer: VecDeque::with_capacity(64),
            max_lines: 64,
        }
    }

    /// Append a line to the ring buffer, dropping old lines if at capacity.
    pub fn push(&mut self, line: String) {
        if self.buffer.len() >= self.max_lines {
            self.buffer.pop_front();
        }
        self.buffer.push_back(line);
    }
}
