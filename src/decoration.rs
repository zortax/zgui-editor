//! Marks an application draws on the text and in the gutter.
//!
//! A decoration is a byte range and a way of drawing it: a band behind the text, or a line under
//! it. A gutter mark is a line and a short label beside its number. Both arrive in *layers*, each
//! under a name the application chooses, so that the things that decorate a buffer for different
//! reasons — a language server's diagnostics, the matches of the last search, the hunks git says
//! changed — can be replaced one at a time without any of them knowing about the others.
//!
//! ```no_run
//! # use zgui_editor::{Decoration, EditorHandle, UnderlineStyle};
//! # fn example(handle: &EditorHandle, errors: Vec<std::ops::Range<usize>>) {
//! handle.set_decorations(
//!     "diagnostics",
//!     errors
//!         .into_iter()
//!         .map(|range| Decoration::underline(range, UnderlineStyle::Squiggly, "editor-error"))
//!         .collect(),
//! );
//! # }
//! ```
//!
//! # Where the colour comes from
//!
//! The same place every other colour in this crate comes from: the style sheet. A decoration
//! normally names a custom property and the cascade answers, so a decoration re-colours with the
//! theme and needs no rebuilding. A literal colour is there for the cases a sheet cannot reach —
//! a colour computed from the content, such as a blame heat map.

use std::borrow::Cow;
use std::ops::Range;

use compact_str::CompactString;
use zgui::canvas::zgui_color::Color;

/// Where one mark's colour comes from.
#[derive(Clone, PartialEq, Debug)]
pub enum Paint {
    /// A custom property, named without its leading dashes: `"editor-error"` reads
    /// `--editor-error`.
    ///
    /// Resolved against the element's computed style, so it inherits, answers to a dark-mode rule
    /// and changes with the theme. A property no rule sets falls back to the element's own
    /// `color`, which is visible rather than invisible — a decoration nobody can see is worse
    /// than one in the wrong colour, because only one of the two gets reported.
    Property(Cow<'static, str>),
    /// A colour worked out by the application.
    Color(Color),
}

impl From<&'static str> for Paint {
    fn from(property: &'static str) -> Self {
        Self::Property(Cow::Borrowed(property))
    }
}

impl From<String> for Paint {
    fn from(property: String) -> Self {
        Self::Property(Cow::Owned(property))
    }
}

impl From<Color> for Paint {
    fn from(color: Color) -> Self {
        Self::Color(color)
    }
}

/// How a line under the text is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnderlineStyle {
    /// One straight line.
    Straight,
    /// A dotted line.
    Dotted,
    /// A dashed line.
    Dashed,
    /// The wave an error is conventionally drawn with.
    Squiggly,
}

/// What one decoration does to the text it covers.
#[derive(Clone, PartialEq, Debug)]
pub enum DecorationKind {
    /// A band behind the text, under the selection.
    Background(Paint),
    /// A line under the text, over it in the painting order so it is never hidden by a glyph.
    Underline {
        /// How the line is drawn.
        style: UnderlineStyle,
        /// What it is drawn with.
        paint: Paint,
    },
    /// A band across the whole text area of every line the range touches.
    ///
    /// From the gutter's edge to the view's edge, wherever the text is scrolled to: what a diff
    /// tints an added line with, and what a caret's row is lit with.
    LineBackground(Paint),
    /// A short bar at the left edge of every line the range touches.
    ///
    /// Two device pixels wide, over the gutter's own background. What marks a run of lines as
    /// one thing without tinting them.
    LineBar(Paint),
}

impl DecorationKind {
    /// What the decoration is drawn with.
    pub fn paint(&self) -> &Paint {
        match self {
            Self::Background(paint)
            | Self::LineBackground(paint)
            | Self::LineBar(paint)
            | Self::Underline { paint, .. } => paint,
        }
    }
}

/// One decorated range of the text.
#[derive(Clone, PartialEq, Debug)]
pub struct Decoration {
    /// The bytes it covers. An empty range decorates nothing and is skipped.
    pub range: Range<usize>,
    /// How it is drawn.
    pub kind: DecorationKind,
}

impl Decoration {
    /// A band behind `range`.
    pub fn background(range: Range<usize>, paint: impl Into<Paint>) -> Self {
        Self {
            range,
            kind: DecorationKind::Background(paint.into()),
        }
    }

    /// A line under `range`.
    pub fn underline(range: Range<usize>, style: UnderlineStyle, paint: impl Into<Paint>) -> Self {
        Self {
            range,
            kind: DecorationKind::Underline {
                style,
                paint: paint.into(),
            },
        }
    }

    /// A full-width band behind every line `range` touches.
    pub fn line_background(range: Range<usize>, paint: impl Into<Paint>) -> Self {
        Self {
            range,
            kind: DecorationKind::LineBackground(paint.into()),
        }
    }

    /// A bar at the left edge of every line `range` touches.
    pub fn line_bar(range: Range<usize>, paint: impl Into<Paint>) -> Self {
        Self {
            range,
            kind: DecorationKind::LineBar(paint.into()),
        }
    }
}

/// What a mark covers, and what it is drawn with.
///
/// What a layer needs of its items to find the ones on screen without visiting the rest: a
/// half-open span on one axis — bytes for a decoration, lines for a gutter mark — and the paint
/// whose colour the style sheet has to answer for.
pub trait Mark {
    /// Where the mark starts.
    fn start(&self) -> usize;
    /// Where it ends, exclusive.
    fn end(&self) -> usize;
    /// What it is drawn with.
    fn paint(&self) -> &Paint;
}

impl Mark for Decoration {
    fn start(&self) -> usize {
        self.range.start
    }

    fn end(&self) -> usize {
        self.range.end
    }

    fn paint(&self) -> &Paint {
        self.kind.paint()
    }
}

impl Mark for GutterMark {
    fn start(&self) -> usize {
        self.line
    }

    fn end(&self) -> usize {
        self.line + 1
    }

    fn paint(&self) -> &Paint {
        &self.paint
    }
}

/// One mark beside a line's number.
///
/// The label is text rather than a shape so that it is drawn by the editor's own shaper in the
/// editor's own font — which is what makes an icon font's glyph line up with the line it is about
/// at every size the editor is used at.
#[derive(Clone, PartialEq, Debug)]
pub struct GutterMark {
    /// Which line it is beside, counting from zero.
    pub line: usize,
    /// What is drawn there. One or two characters; anything longer is clipped by the gutter.
    pub text: CompactString,
    /// What it is drawn with.
    pub paint: Paint,
}

impl GutterMark {
    /// A mark on `line`.
    pub fn new(line: usize, text: impl Into<CompactString>, paint: impl Into<Paint>) -> Self {
        Self {
            line,
            text: text.into(),
            paint: paint.into(),
        }
    }
}

/// What one line of a custom gutter shows.
#[derive(Clone, PartialEq, Debug)]
pub struct GutterLabel {
    /// The text, right-aligned where a line number would be.
    pub text: CompactString,
    /// What it is drawn with. The gutter's own colour when not given.
    pub paint: Option<Paint>,
}

/// Where a custom gutter's labels come from.
///
/// Asked once per visible line per frame, so a source answers from what it already holds. The
/// paints its labels may name are declared up front: that is what lets the style sheet answer
/// for them before the first label is drawn.
#[derive(Clone)]
pub struct GutterSource {
    /// Every paint a label of this source may carry.
    pub paints: Vec<Paint>,
    /// The label for a line, when the line has one.
    pub label: std::rc::Rc<dyn Fn(usize) -> Option<GutterLabel>>,
}

/// One named group of marks.
///
/// Sorted by where each mark starts, so that the marks over one stretch of the text are found by
/// a binary search rather than by a walk over every mark the application ever set.
pub struct Layer<T> {
    /// What the application called it.
    pub name: String,
    /// What is in it, sorted by start.
    pub items: Vec<T>,
    /// How far the longest item reaches past its start.
    longest: usize,
    /// The custom properties the items name, each once.
    properties: Vec<String>,
}

impl<T: Mark> Layer<T> {
    /// A layer over `items`, sorted.
    fn new(name: &str, mut items: Vec<T>) -> Self {
        items.sort_by_key(Mark::start);
        let longest = items
            .iter()
            .map(|item| item.end().saturating_sub(item.start()))
            .max()
            .unwrap_or(0);
        let mut properties: Vec<String> = items
            .iter()
            .filter_map(|item| match item.paint() {
                Paint::Property(name) => Some(name.as_ref().to_owned()),
                Paint::Color(_) => None,
            })
            .collect();
        properties.sort_unstable();
        properties.dedup();
        Self {
            name: name.to_owned(),
            items,
            longest,
            properties,
        }
    }

    /// The items any part of which lies inside `span`, in order.
    ///
    /// Costs the logarithm of the layer plus what it answers: the first candidate is the first
    /// item that could still reach `span`, given the longest item there is.
    pub fn overlapping(&self, span: Range<usize>) -> impl Iterator<Item = &T> {
        let earliest = span.start.saturating_sub(self.longest);
        let from = self.items.partition_point(|item| item.start() < earliest);
        let to = from + self.items[from..].partition_point(|item| item.start() < span.end);
        self.items[from..to]
            .iter()
            .filter(move |item| item.end() > span.start)
    }

    /// The custom properties the items name.
    pub fn properties(&self) -> &[String] {
        &self.properties
    }
}

/// A set of named layers, in the order they were first named.
///
/// Layers paint in that order, so an application decides what sits over what by the order it
/// first names them rather than by a number every caller would have to keep consistent with
/// every other.
pub struct Layers<T> {
    layers: Vec<Layer<T>>,
}

// Written out rather than derived: a derived `Default` would ask `T` for one, and no mark this
// holds has a default that means anything.
impl<T> Default for Layers<T> {
    fn default() -> Self {
        Self { layers: Vec::new() }
    }
}

impl<T: Mark> Layers<T> {
    /// Replaces the layer called `name`, adding it at the end if it is new.
    ///
    /// Answers whether anything is different, so a caller that sets the same empty layer on every
    /// keystroke — which is what a search with no matches does — costs no repaint.
    pub fn set(&mut self, name: &str, items: Vec<T>) -> bool
    where
        T: PartialEq,
    {
        let fresh = Layer::new(name, items);
        match self.layers.iter_mut().find(|layer| layer.name == name) {
            Some(layer) => {
                if layer.items == fresh.items {
                    return false;
                }
                *layer = fresh;
                true
            }
            None => {
                if fresh.items.is_empty() {
                    return false;
                }
                self.layers.push(fresh);
                true
            }
        }
    }

    /// The items of every layer any part of which lies inside `span`, in painting order.
    pub fn overlapping(&self, span: Range<usize>) -> impl Iterator<Item = &T> {
        self.layers
            .iter()
            .flat_map(move |layer| layer.overlapping(span.clone()))
    }

    /// Every custom property any layer names, in painting order, a layer's own each once.
    pub fn properties(&self) -> impl Iterator<Item = &str> {
        self.layers
            .iter()
            .flat_map(|layer| layer.properties().iter().map(String::as_str))
    }

    /// Removes the layer called `name`. Answers whether there was one.
    pub fn clear(&mut self, name: &str) -> bool {
        let before = self.layers.len();
        self.layers.retain(|layer| layer.name != name);
        self.layers.len() != before
    }

    /// Every item in every layer, in painting order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.layers.iter().flat_map(|layer| layer.items.iter())
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|layer| layer.items.is_empty())
    }

    /// Every layer, in painting order.
    pub fn layers(&self) -> &[Layer<T>] {
        &self.layers
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoration, GutterMark, Layers, Paint, UnderlineStyle};

    #[test]
    fn a_layer_replaces_only_itself() {
        let mut layers: Layers<Decoration> = Layers::default();
        assert!(layers.set("search", vec![Decoration::background(0..4, "a")]));
        assert!(layers.set("diagnostics", vec![Decoration::background(8..9, "b")]));
        assert_eq!(layers.iter().count(), 2);

        assert!(layers.set("search", Vec::new()));
        assert_eq!(layers.iter().count(), 1);
        assert!(!layers.is_empty());
    }

    #[test]
    fn setting_a_layer_to_what_it_already_holds_changes_nothing() {
        // A search that re-runs on every keystroke and finds the same matches must not repaint.
        let mut layers: Layers<Decoration> = Layers::default();
        let items = || vec![Decoration::background(0..4, "a")];
        assert!(layers.set("search", items()));
        assert!(!layers.set("search", items()));
    }

    #[test]
    fn an_empty_layer_that_was_never_set_is_not_created() {
        let mut layers: Layers<Decoration> = Layers::default();
        assert!(!layers.set("search", Vec::new()));
        assert!(layers.is_empty());
        assert!(!layers.clear("search"));
    }

    #[test]
    fn layers_paint_in_the_order_they_were_first_named() {
        let mut layers: Layers<GutterMark> = Layers::default();
        layers.set("git", vec![GutterMark::new(0, "▌", "a")]);
        layers.set("diagnostics", vec![GutterMark::new(1, "!", "b")]);
        // Re-setting the first does not move it to the end.
        layers.set("git", vec![GutterMark::new(2, "▌", "a")]);
        let lines: Vec<usize> = layers.iter().map(|mark| mark.line).collect();
        assert_eq!(lines, [2, 1]);
    }

    #[test]
    fn a_layer_answers_the_marks_over_a_span_and_no_others() {
        let mut layers: Layers<Decoration> = Layers::default();
        layers.set(
            "kinds",
            vec![
                Decoration::line_background(400..410, "a"),
                Decoration::line_background(0..300, "a"),
                Decoration::line_background(350..360, "a"),
                Decoration::line_background(900..950, "a"),
            ],
        );
        // A long range that starts well before the span still reaches into it.
        let found: Vec<usize> = layers
            .overlapping(280..420)
            .map(|d| d.range.start)
            .collect();
        assert_eq!(found, [0, 350, 400]);
        assert_eq!(layers.overlapping(500..800).count(), 0);
        assert_eq!(layers.properties().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn gutter_marks_are_found_by_line() {
        let mut layers: Layers<GutterMark> = Layers::default();
        layers.set(
            "git",
            vec![
                GutterMark::new(30, "+", "added"),
                GutterMark::new(3, "-", "removed"),
                GutterMark::new(12, "+", "added"),
            ],
        );
        let lines: Vec<usize> = layers.overlapping(10..31).map(|m| m.line).collect();
        assert_eq!(lines, [12, 30]);
        let mut names: Vec<&str> = layers.properties().collect();
        names.sort_unstable();
        assert_eq!(names, ["added", "removed"]);
    }

    #[test]
    fn a_property_name_carries_no_dashes() {
        // The reader adds them, so that a name written with them is a mistake this catches
        // rather than a property that silently never resolves.
        let paint: Paint = "editor-error".into();
        assert_eq!(paint, Paint::Property("editor-error".into()));
        let _ = UnderlineStyle::Squiggly;
    }
}
