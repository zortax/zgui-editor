//! What the highlighter knows, on the UI side.
//!
//! The worker parses and queries on its own thread; what arrives here is per-line spans stamped
//! with the buffer revision they describe. A stale stamp is dropped — the worker always
//! converges on the newest text — so a line is only ever coloured by spans that describe it.

pub mod languages;
pub mod oneshot;
pub mod registry;
pub mod spans;
pub mod worker;

use std::collections::BTreeMap;

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
    ///
    /// Ordered, so a change moves only the lines after it: a line added at the end of a long
    /// buffer costs a lookup, and an edit near the top moves what lies below it.
    spans: BTreeMap<usize, SmallVec<[LineSpan; 8]>>,
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

    /// The capture names the spans count into.
    pub fn capture_names(&self) -> &[String] {
        &self.capture_names
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

    /// Forgets the spans for `line`.
    pub fn remove_line(&mut self, line: usize) {
        self.spans.remove(&line);
    }

    /// Says a batch of stored lines changed, once per frame of arrivals.
    pub fn bump(&mut self) {
        self.version += 1;
    }

    /// Forgets every span at or after `line`, which an edit there makes suspect.
    pub fn invalidate_from(&mut self, line: usize) {
        self.spans.split_off(&line);
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
        let below = self.spans.split_off(&first);
        for (line, spans) in below {
            if line < removed_until {
                // Gone with the deleted lines.
            } else if let Some(shifted) = line.checked_add_signed(delta) {
                self.spans.insert(shifted, spans);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_spans_resolve_through_the_capture_vocabulary() {
        let mut state = SyntaxState::default();
        let theme = Theme::fallback();
        state.set_captures(vec!["keyword".into(), "string".into()], &theme);
        let before = state.version();
        state.put_line(
            3,
            SmallVec::from_vec(vec![(0, 2, 0), (4, 9, 1), (10, 11, 7)]),
        );
        state.bump();
        assert!(state.version() > before, "a stored batch moves the version");
        let coloured = state.colored_spans(3);
        // The capture outside the vocabulary colours nothing; the rest take the theme's answer.
        assert_eq!(coloured.len(), 2);
        assert_eq!((coloured[0].0, coloured[0].1), (0, 2));
        assert!(state.colored_spans(4).is_empty());
    }

    #[test]
    fn a_shift_moves_only_the_lines_after_the_change() {
        let mut state = SyntaxState::default();
        let theme = Theme::fallback();
        state.set_captures(vec!["keyword".into()], &theme);
        for line in [0, 2, 5] {
            state.put_line(line, SmallVec::from_vec(vec![(0, 1, 0)]));
        }
        state.shift_lines(2, 3);
        assert!(!state.colored_spans(0).is_empty());
        assert!(!state.colored_spans(5).is_empty());
        assert!(!state.colored_spans(8).is_empty());
        assert!(state.colored_spans(2).is_empty());

        // Lines 0 to 4 go: line 0 takes its spans along, 5 and 8 move up to 0 and 3.
        state.shift_lines(0, -5);
        assert!(!state.colored_spans(0).is_empty());
        assert!(!state.colored_spans(3).is_empty());
        assert!(state.colored_spans(5).is_empty());
        assert!(state.colored_spans(8).is_empty());
    }
}
