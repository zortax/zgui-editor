//! What the highlighter knows, on the UI side.
//!
//! The worker parses and queries on its own thread; what arrives here is per-line spans stamped
//! with the buffer revision they describe. A stale stamp is dropped — the worker always
//! converges on the newest text — so a line is only ever coloured by spans that describe it.

pub mod languages;
pub mod registry;
pub mod spans;
pub mod worker;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use zgui::canvas::zgui_color::Color;

use crate::render::theme::Theme;

/// One highlight span: byte range within its line, and which capture it is.
pub type LineSpan = (u32, u32, u16);

/// The spans the UI holds, and the colours its captures resolve to.
#[derive(Default)]
pub struct SyntaxState {
    /// Moves whenever the held spans change, so line caches know to re-colour.
    version: u64,
    /// Spans by line index, in line-local byte offsets.
    spans: FxHashMap<usize, SmallVec<[LineSpan; 8]>>,
    /// The capture names of the loaded language, by capture index.
    capture_names: Vec<String>,
    /// Each capture's colour under the current theme, by capture index.
    capture_colors: Vec<Color>,
}

impl SyntaxState {
    /// Which set of spans this is.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Adopts the capture vocabulary of a newly loaded language.
    pub fn set_captures(&mut self, names: Vec<String>, theme: &Theme) {
        self.capture_names = names;
        self.resolve_colors(theme);
        self.spans.clear();
        self.version += 1;
    }

    /// Re-resolves every capture colour, which a theme change makes necessary.
    pub fn resolve_colors(&mut self, theme: &Theme) {
        self.capture_colors = self
            .capture_names
            .iter()
            .map(|name| theme.capture_color(name))
            .collect();
    }

    /// Stores the spans for `line`.
    pub fn put_line(&mut self, line: usize, spans: SmallVec<[LineSpan; 8]>) {
        self.spans.insert(line, spans);
    }

    /// Says a batch of stored lines changed, once per frame of arrivals.
    pub fn bump(&mut self) {
        self.version += 1;
    }

    /// Forgets every span at or after `line`, which an edit there makes suspect.
    pub fn invalidate_from(&mut self, line: usize) {
        self.spans.retain(|held, _| *held < line);
        self.version += 1;
    }

    /// Moves every stored line at or after `first` by `delta` lines.
    ///
    /// This is what keeps colours on the text while the worker catches up with an edit that
    /// added or removed lines: the old spans are approximately right for the text that merely
    /// moved, and approximately right beats flashing plain until the fresh frame lands.
    pub fn shift_lines(&mut self, first: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        // A removal takes its own lines' spans with it; keeping them would land them on the
        // line that moved up into their place.
        let removed_until = if delta < 0 {
            first + delta.unsigned_abs()
        } else {
            first
        };
        let mut moved = FxHashMap::default();
        for (line, spans) in self.spans.drain() {
            if line < first {
                moved.insert(line, spans);
            } else if line < removed_until {
                // Gone with the deleted lines.
            } else if let Some(shifted) = line.checked_add_signed(delta) {
                moved.insert(shifted, spans);
            }
        }
        self.spans = moved;
        self.version += 1;
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.spans.clear();
        self.version += 1;
    }

    /// The coloured spans for `line`, in the form the line cache slices by.
    pub fn colored_spans(&self, line: usize) -> Vec<(u32, u32, Color)> {
        let Some(spans) = self.spans.get(&line) else {
            return Vec::new();
        };
        spans
            .iter()
            .filter_map(|(start, end, capture)| {
                let color = self.capture_colors.get(*capture as usize)?;
                Some((*start, *end, *color))
            })
            .collect()
    }
}
