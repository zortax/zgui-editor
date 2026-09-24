//! Styles an application puts on stretches of the text.
//!
//! A text style colours, emboldens, slants, dims, underlines or strikes out one stretch of one
//! line, over what the highlighter says. The application sets one table of styles, and each line
//! holds spans that index into it in line-local bytes. Terminal output maps onto this directly:
//! a log view shows the escape sequences of a program with it.
//!
//! ```no_run
//! # use zgui_editor::{EditorHandle, TextStyle};
//! # fn example(handle: &EditorHandle) {
//! handle.set_text_styles(vec![TextStyle {
//!     color: Some("terminal-red".into()),
//!     bold: true,
//!     ..TextStyle::default()
//! }]);
//! // The first four bytes of line 3 take style 0.
//! handle.put_text_styles(3, vec![smallvec::smallvec![(0, 4, 0)]]);
//! # }
//! ```
//!
//! Bold and italic are drawn by emboldening and shearing the glyphs of the face in use, so a
//! styled stretch keeps the advance of the text around it.

use std::collections::BTreeMap;

use smallvec::SmallVec;
use zgui::canvas::zgui_color::Color;

use crate::decoration::Paint;

/// One span of a line: line-local byte range, and which style of the table it takes.
pub type StyleSpan = (u32, u32, u16);

/// The spans of one line.
pub type StyleSpans = SmallVec<[StyleSpan; 4]>;

/// How much a dim stretch keeps of its colour's alpha.
const DIM: f32 = 0.55;

/// How a stretch of text is drawn.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TextStyle {
    /// The colour of the glyphs. The highlighter's colour, or the text colour, when not given.
    pub color: Option<Paint>,
    /// A band behind the glyphs.
    pub background: Option<Paint>,
    /// Emboldened glyphs.
    pub bold: bool,
    /// Sheared glyphs.
    pub italic: bool,
    /// Glyphs at part of their colour's alpha.
    pub dim: bool,
    /// A straight line under the glyphs, in their colour.
    pub underline: bool,
    /// A straight line through the glyphs, in their colour.
    pub strikethrough: bool,
    /// The colour and the background swap. A missing one is the text colour or the element's
    /// background.
    pub inverse: bool,
}

/// A style with its colours resolved, as one line of the cache is built with.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Look {
    /// The colour of the glyphs, when the style sets one.
    pub color: Option<Color>,
    /// The band behind the glyphs.
    pub background: Option<Color>,
    /// Emboldened glyphs.
    pub bold: bool,
    /// Sheared glyphs.
    pub italic: bool,
    /// Glyphs at part of their alpha.
    pub dim: bool,
    /// A line under the glyphs.
    pub underline: bool,
    /// A line through the glyphs.
    pub strikethrough: bool,
}

impl Look {
    /// The colour a glyph whose own colour is `base` takes under this look.
    #[must_use]
    pub fn glyph_color(&self, base: Color) -> Color {
        let color = self.color.unwrap_or(base);
        if self.dim {
            color.with_alpha(color.alpha() * DIM)
        } else {
            color
        }
    }
}

/// The table, the spans by line, and the looks the table resolves to.
#[derive(Default)]
pub struct StyleState {
    /// Moves whenever what the lines look like changes, so line caches know to rebuild.
    version: u64,
    /// The styles the spans count into.
    table: Vec<TextStyle>,
    /// Each style resolved under the current theme, by table index.
    looks: Vec<Look>,
    /// Spans by line index, sorted by start.
    spans: BTreeMap<usize, StyleSpans>,
}

impl StyleState {
    /// Which state of the styles this is.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Replaces the table. The spans stay.
    pub fn set_table(&mut self, table: Vec<TextStyle>) {
        self.table = table;
        self.version += 1;
    }

    /// The styles the spans count into.
    pub fn table(&self) -> &[TextStyle] {
        &self.table
    }

    /// Resolves every style, with `paint` answering a colour and `fg` and `bg` standing for the
    /// text colour and the element background. Moves the version when a look changed.
    pub fn resolve(&mut self, paint: impl Fn(&Paint) -> Color, fg: Color, bg: Color) {
        let looks: Vec<Look> = self
            .table
            .iter()
            .map(|style| {
                let color = style.color.as_ref().map(&paint);
                let background = style.background.as_ref().map(&paint);
                let (color, background) = if style.inverse {
                    (Some(background.unwrap_or(bg)), Some(color.unwrap_or(fg)))
                } else {
                    (color, background)
                };
                Look {
                    color,
                    background,
                    bold: style.bold,
                    italic: style.italic,
                    dim: style.dim,
                    underline: style.underline,
                    strikethrough: style.strikethrough,
                }
            })
            .collect();
        if looks != self.looks {
            self.looks = looks;
            self.version += 1;
        }
    }

    /// Every custom property the table names.
    pub fn properties(&self) -> impl Iterator<Item = &str> {
        self.table
            .iter()
            .flat_map(|style| [style.color.as_ref(), style.background.as_ref()])
            .flatten()
            .filter_map(|paint| match paint {
                Paint::Property(name) => Some(name.as_ref()),
                Paint::Color(_) => None,
            })
    }

    /// Stores the spans of `line`. Empty spans take the styles off it.
    pub fn put_line(&mut self, line: usize, spans: StyleSpans) {
        if spans.is_empty() {
            self.spans.remove(&line);
        } else {
            self.spans.insert(line, spans);
        }
    }

    /// Says a batch of stored lines changed.
    pub fn bump(&mut self) {
        self.version += 1;
    }

    /// Moves every stored line at or after `first` by `delta` lines. A removal takes the spans of
    /// its own lines with it.
    pub fn shift_lines(&mut self, first: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        let removed_until = if delta < 0 {
            first + delta.unsigned_abs()
        } else {
            first
        };
        let below = self.spans.split_off(&first);
        for (line, spans) in below {
            if line >= removed_until
                && let Some(shifted) = line.checked_add_signed(delta)
            {
                self.spans.insert(shifted, spans);
            }
        }
        self.version += 1;
    }

    /// Forgets every span.
    pub fn clear(&mut self) {
        if !self.spans.is_empty() {
            self.spans.clear();
            self.version += 1;
        }
    }

    /// Whether no line holds a span.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The resolved spans of `line`, in line-local bytes, sorted by start.
    pub fn looks_of(&self, line: usize) -> SmallVec<[(u32, u32, Look); 4]> {
        let Some(spans) = self.spans.get(&line) else {
            return SmallVec::new();
        };
        spans
            .iter()
            .filter_map(|(start, end, index)| {
                let look = self.looks.get(usize::from(*index))?;
                Some((*start, *end, *look))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use smallvec::smallvec;

    use super::*;

    fn red() -> Color {
        Color::srgb(1.0, 0.0, 0.0, 1.0)
    }

    fn fg() -> Color {
        Color::srgb(0.9, 0.9, 0.9, 1.0)
    }

    fn bg() -> Color {
        Color::srgb(0.1, 0.1, 0.1, 1.0)
    }

    #[test]
    fn a_style_resolves_its_property_and_its_flags() {
        let mut state = StyleState::default();
        state.set_table(vec![TextStyle {
            color: Some("terminal-red".into()),
            bold: true,
            underline: true,
            ..TextStyle::default()
        }]);
        state.resolve(|_| red(), fg(), bg());
        state.put_line(2, smallvec![(1, 4, 0)]);
        let looks = state.looks_of(2);
        assert_eq!(looks.len(), 1);
        assert_eq!(looks[0].2.color, Some(red()));
        assert!(looks[0].2.bold && looks[0].2.underline);
        assert_eq!(state.properties().collect::<Vec<_>>(), ["terminal-red"]);
    }

    #[test]
    fn inverse_swaps_the_colours_and_fills_what_is_missing() {
        let mut state = StyleState::default();
        state.set_table(vec![TextStyle {
            inverse: true,
            ..TextStyle::default()
        }]);
        state.resolve(|_| red(), fg(), bg());
        state.put_line(0, smallvec![(0, 1, 0)]);
        let look = state.looks_of(0)[0].2;
        assert_eq!(look.color, Some(bg()));
        assert_eq!(look.background, Some(fg()));
    }

    #[test]
    fn dim_keeps_part_of_the_alpha() {
        let look = Look {
            dim: true,
            ..Look::default()
        };
        assert!(look.glyph_color(red()).alpha() < 1.0);
        assert_eq!(Look::default().glyph_color(red()), red());
    }

    #[test]
    fn a_change_of_look_moves_the_version_and_the_same_look_does_not() {
        let mut state = StyleState::default();
        state.set_table(vec![TextStyle {
            color: Some("a".into()),
            ..TextStyle::default()
        }]);
        state.resolve(|_| red(), fg(), bg());
        let before = state.version();
        state.resolve(|_| red(), fg(), bg());
        assert_eq!(state.version(), before);
        state.resolve(|_| fg(), fg(), bg());
        assert!(state.version() > before);
    }

    #[test]
    fn spans_move_with_the_lines_an_edit_removes() {
        let mut state = StyleState::default();
        state.set_table(vec![TextStyle::default()]);
        state.resolve(|_| red(), fg(), bg());
        for line in [0, 3, 6] {
            state.put_line(line, smallvec![(0, 1, 0)]);
        }
        state.shift_lines(0, -3);
        assert!(state.looks_of(0).len() == 1, "line 3 moved to 0");
        assert!(state.looks_of(3).len() == 1, "line 6 moved to 3");
        assert!(state.looks_of(6).is_empty());
    }
}
