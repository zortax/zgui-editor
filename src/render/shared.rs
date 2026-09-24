//! The one place the view's handlers and the element's frames meet.
//!
//! Everything one view of a buffer is — its carets, its scroll position, its metrics, its theme,
//! its caches — lives here behind one `Rc<RefCell<_>>`. Event handlers mutate it and say so
//! through the element handle; `layout` reads the style into it; `paint` reads it out as
//! primitives. Nothing else holds view state, so nothing else can disagree.
//!
//! The text is not here. It is behind a second handle, because several views can share one
//! document — which is what makes two windows onto the same file two of these and one of that.

use std::ops::Range;
use std::rc::Rc;

use compact_str::CompactString;
use zgui::app::Shaper;
use zgui::canvas::zgui_color::Color;
use zgui::custom::{PaintSlot, ScenePainter, ShapedRun};
use zgui::elements::kurbo;
use zgui::geom::{Device, DevicePx, Point, Rect, Size};
use zgui_css::ComputedStyle;
use zgui_interned::Ident;

use crate::config::{CursorStyle, EditorConfig, GutterMode};
use crate::core::motion::MotionContext;
use crate::core::position;
use crate::core::selection::Selections;
use crate::decoration::{
    Decoration, DecorationKind, GutterMark, GutterSource, Layers, Paint, UnderlineStyle,
};
use crate::document::Document;
use crate::overlay::Overlay;
use crate::render::line_cache::{self, CachedLine, LineCache};
use crate::render::metrics::TextMetrics;
use crate::render::shaping::ShapingCache;
use crate::render::theme::{self, Theme};
use crate::render::{families, gutter};
use crate::scroll::{ScrollState, Scrollbar};
use crate::styles::StyleState;
use crate::syntax::SyntaxState;

/// How wide the scrollbar is, in device pixels at scale one.
const SCROLLBAR_WIDTH: f32 = 10.0;

/// How much a bold text style emboldens its glyphs, as a fraction of the size.
const SYNTHETIC_BOLD: f32 = 0.02;

/// How far an italic text style shears its glyphs, in degrees.
const SYNTHETIC_SLANT: f32 = 14.0;

/// What `layout` reports outward when the style or the space changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleReport {
    /// The measurements now in force.
    pub metrics: TextMetrics,
    /// Which theme this is, counting up.
    pub theme_version: u32,
    /// The element's content size, in device pixels.
    pub viewport: (f32, f32),
}

/// One view of one document, as one mutable value.
///
/// The document is behind a second handle because it is shared: two windows onto the same file
/// hold one document and two of these. Everything else here belongs to this view alone — its
/// carets, where it is scrolled to, what it measured, what it has shaped and painted.
pub struct EditorShared {
    /// The text, the history and the options, shared with every other view of this document.
    pub document: Document,
    /// This view's selections.
    pub selections: Selections,
    /// The bands and the carets the application paints itself, when it does.
    pub overlay: Overlay,
    /// The behaviour.
    pub config: EditorConfig,
    /// Where the view sits.
    pub scroll: ScrollState,
    /// The measurements text is laid out by.
    pub metrics: TextMetrics,
    /// The colours.
    pub theme: Theme,
    /// Counts theme changes, so caches know to re-colour.
    pub theme_version: u32,
    /// The families text is shaped in, resolved to faces this machine has.
    pub families: Vec<Ident>,
    /// The shaper, when the application has fonts at all.
    pub shaper: Option<Shaper>,
    /// Shaped lines, kept between frames.
    pub shaping: ShapingCache,
    /// Painted lines, kept between frames.
    pub lines: LineCache,
    /// What the highlighter has said so far.
    pub syntax: SyntaxState,
    /// The text styles the application put on the lines.
    pub styles: StyleState,
    /// Marks the application drew on the text.
    pub decorations: Layers<Decoration>,
    /// Marks the application drew in the gutter.
    pub gutter_marks: Layers<GutterMark>,
    /// What a custom gutter's labels are, when the application draws the gutter.
    pub gutter_source: Option<GutterSource>,
    /// The colours those marks named, as the style sheet answers for them.
    ///
    /// A name present with `None` is one the sheet does not set, which is different from one that
    /// has not been looked up yet — and the difference is what tells a decoration that named a
    /// new property that it has to wait for a layout before it can be drawn in its own colour.
    pub decoration_colors: rustc_hash::FxHashMap<String, Option<Color>>,
    /// The way to the syntax worker, once one runs.
    pub syntax_tx: Option<flume::Sender<crate::syntax::worker::ToWorker>>,
    /// The visible range last asked of the worker, so scrolling asks again only when it must.
    pub requested_window: Range<usize>,
    /// The element's content size, in device pixels, from the last layout.
    pub viewport: (f32, f32),
    /// Whether the editor has focus.
    pub focused: bool,
    /// Whether this is the view a person is working in.
    ///
    /// Separate from `focused`, which is where the keyboard is right now. An editor under a picker
    /// or a command line keeps its caret band, because it is still the view being worked in. An
    /// application with several views on screen tells each one through
    /// [`EditorHandle::set_active`](crate::EditorHandle::set_active); one with a single view leaves
    /// it alone.
    pub active: bool,
    /// Whether the caret is in the visible half of its blink.
    pub blink_on: bool,
    /// The widest shaped line seen so far, in device pixels.
    pub max_line_width: f32,
    /// The longest line's character count, from the background scan, surviving re-measures.
    pub max_line_chars: Option<usize>,
    /// Who to tell when layout read something new; the view answers with a repaint.
    pub on_style: Option<Rc<dyn Fn(StyleReport)>>,
    /// What was last reported, so the same answer is reported once.
    reported: Option<StyleReport>,
}

impl EditorShared {
    /// A view of `document`.
    pub fn new(document: Document, config: EditorConfig, shaper: Option<Shaper>) -> Self {
        document.state_mut().options = config.edit.clone();
        Self {
            document,
            selections: Selections::caret(0),
            overlay: Overlay::default(),
            config,
            scroll: ScrollState::default(),
            metrics: TextMetrics::default(),
            theme: Theme::fallback(),
            theme_version: 0,
            families: Vec::new(),
            shaper,
            shaping: ShapingCache::new(),
            lines: LineCache::default(),
            syntax: SyntaxState::default(),
            styles: StyleState::default(),
            decorations: Layers::default(),
            gutter_marks: Layers::default(),
            gutter_source: None,
            decoration_colors: rustc_hash::FxHashMap::default(),
            syntax_tx: None,
            requested_window: 0..0,
            viewport: (0.0, 0.0),
            focused: false,
            active: true,
            blink_on: true,
            max_line_width: 0.0,
            max_line_chars: None,
            on_style: None,
            reported: None,
        }
    }

    // ---- The document ------------------------------------------------------------------

    /// The text. Cloning a rope is a refcount, so this is what everything here reads through.
    pub fn rope(&self) -> ropey::Rope {
        self.document.rope()
    }

    /// Which revision of the text this is.
    pub fn revision(&self) -> u64 {
        self.document.revision()
    }

    /// How many lines the text has.
    pub fn line_count(&self) -> usize {
        self.document.line_count()
    }

    /// Drops what this view had shaped, painted and highlighted for the lines a change touched,
    /// and tells its own highlighter about it.
    ///
    /// Every view of a document does this for every change, whichever view made it — which is
    /// what makes the second window onto a file show the first window's typing.
    pub fn absorb_change(&mut self, change: &crate::core::ChangeInfo) {
        if change.whole_text {
            // Nothing about the old text survives, so nothing cached about it can.
            self.lines.clear();
            self.syntax.clear();
            self.styles.clear();
            self.max_line_width = 0.0;
            self.max_line_chars = None;
        } else {
            // How many lines the change added or removed, and the last line it touched —
            // readable straight off the tree-sitter deltas it already carries.
            let line_delta: isize = change
                .input_edits
                .iter()
                .map(|edit| edit.new_end_position.row as isize - edit.old_end_position.row as isize)
                .sum();
            let last_changed_line = change
                .input_edits
                .iter()
                .map(|edit| edit.old_end_position.row.max(edit.new_end_position.row))
                .max()
                .unwrap_or(change.first_changed_line);
            if line_delta == 0 {
                // Nothing below the edit moved: only the touched lines rebuild, and the highlight
                // spans stay put — slightly stale for a frame or two, which reads as nothing,
                // where dropping them reads as a white flash.
                self.lines
                    .invalidate_range(change.first_changed_line, last_changed_line);
            } else {
                self.lines.invalidate_from(change.first_changed_line);
                self.syntax
                    .shift_lines(change.first_changed_line, line_delta);
                self.styles
                    .shift_lines(change.first_changed_line, line_delta);
            }
        }

        if let Some(tx) = self.syntax_tx.as_ref() {
            let (snapshot, revision) = self.document.state().snapshot();
            // A replaced text carries no edits. Sent as an edit, the worker would parse it
            // against the old text's tree, and a grammar's scanner would be handed states from
            // text that is gone.
            let message = if change.whole_text {
                crate::syntax::worker::ToWorker::Replaced { snapshot, revision }
            } else {
                crate::syntax::worker::ToWorker::Edited {
                    input_edits: change.input_edits.clone(),
                    snapshot,
                    revision,
                }
            };
            let _ = tx.send(message);
        }
    }

    // ---- Geometry ----------------------------------------------------------------------

    /// Which lines this view draws, when it draws only some of them.
    ///
    /// Clamped to the text, so a window over lines a shrunken document no longer has is a window
    /// over what is left of them. An empty one is no window at all.
    pub fn line_window(&self) -> Option<Range<usize>> {
        let total = self.line_count();
        let window = self.config.line_window.clone()?;
        let start = window.start.min(total);
        let end = window.end.min(total);
        (start < end).then_some(start..end)
    }

    /// The line at the top of what is drawn.
    ///
    /// A windowed view is pinned to its first line: there is nothing above it to scroll to, so
    /// everything drawn is measured from there rather than from a scroll position nothing moves.
    pub fn top_line(&self) -> f64 {
        match self.line_window() {
            Some(window) => window.start as f64,
            None => self.scroll.pos.line,
        }
    }

    /// How many whole lines the viewport shows.
    pub fn viewport_lines(&self) -> f64 {
        (f64::from(self.viewport.1) / f64::from(self.metrics.line_height)).max(1.0)
    }

    /// The context motions resolve against.
    pub fn motion_context(&self) -> MotionContext {
        MotionContext {
            viewport_lines: self.viewport_lines().floor().max(1.0) as usize,
        }
    }

    /// How wide the scrollbar is, in device pixels: nothing when the view draws none.
    pub fn scrollbar_width(&self) -> f32 {
        if !self.config.scrollbar {
            return 0.0;
        }
        SCROLLBAR_WIDTH * self.metrics.scale.max(0.5)
    }

    /// The vertical scrollbar's geometry right now.
    pub fn scrollbar(&self) -> Scrollbar {
        Scrollbar {
            track: f64::from(self.viewport.1),
            // A windowed view is exactly as tall as what it draws, so its bar has no thumb and
            // every scroll gesture that asks the bar about one is answered with nothing.
            total: self
                .line_window()
                .map_or_else(|| self.line_count(), |window| window.len()),
            viewport: self.viewport_lines(),
        }
    }

    /// How wide the text area is, in device pixels.
    pub fn text_area_width(&self) -> f32 {
        (self.viewport.0 - self.gutter_width()).max(1.0)
    }

    /// How far the text can scroll horizontally: the widest line known, in device pixels.
    ///
    /// The real shaped widths of whatever has been visible, or the background scan's
    /// character-count estimate — whichever says more.
    pub fn horizontal_extent(&self) -> f32 {
        let estimate = self
            .max_line_chars
            .map(|chars| chars as f32 * self.metrics.cell_advance)
            .unwrap_or(0.0);
        self.max_line_width.max(estimate)
    }

    /// Whether `(x, y)` element-local device pixels sit on a scrollbar being drawn.
    ///
    /// This is what turns the text cursor back into the arrow: the bars are painted inside the
    /// one element, so no CSS `:hover` can tell them from the text.
    pub fn over_scrollbar(&self, x: f32, y: f32) -> bool {
        let bar = self.scrollbar_width();
        let vertical =
            x >= self.viewport.0 - bar && self.scrollbar().thumb(self.top_line()).is_some();
        let horizontal = y >= self.viewport.1 - bar && self.horizontal_thumb().is_some();
        vertical || horizontal
    }

    /// The horizontal thumb, as `(left, width)`, when one is worth drawing.
    pub fn horizontal_thumb(&self) -> Option<(f32, f32)> {
        let area = self.text_area_width();
        let max = self.horizontal_extent();
        if max <= area {
            return None;
        }
        let track = area;
        let width = (track * area / max).max(30.0).min(track);
        let travel = f64::from(track - width);
        let position = (self.scroll.pos.x_px / f64::from(max - area)).clamp(0.0, 1.0);
        Some((self.gutter_width() + (travel * position) as f32, width))
    }

    /// How wide the gutter is.
    pub fn gutter_width(&self) -> f32 {
        gutter::width(
            self.config.gutter,
            self.line_count(),
            self.metrics.cell_advance,
        )
    }

    /// The top of `line`, in device pixels from the content box's top.
    pub fn line_y(&self, line: usize) -> f32 {
        ((line as f64 - self.top_line()) * f64::from(self.metrics.line_height)) as f32
    }

    /// The lines any part of which is on screen.
    pub fn visible_lines(&self) -> Range<usize> {
        // A window is the answer on its own: it says which lines are drawn, and a box a line
        // taller than them must not draw the line after.
        if let Some(window) = self.line_window() {
            return window;
        }
        let total = self.line_count();
        let first = self.scroll.pos.line.floor().max(0.0) as usize;
        let rows =
            (f64::from(self.viewport.1) / f64::from(self.metrics.line_height)).ceil() as usize + 1;
        first.min(total)..(first + rows).min(total)
    }

    // ---- Style -------------------------------------------------------------------------

    /// Reads everything paint needs off the computed style. Called from the element's `layout`,
    /// which is the only place the style is handed over.
    ///
    /// The viewport is stored only on the final pass: layout runs several times with different
    /// constraints, and geometry taken from a measure pass is geometry of a box that was never
    /// placed — which is a scrollbar for nothing and a view a third the window tall.
    pub fn read_style(
        &mut self,
        style: &ComputedStyle,
        scale: f32,
        viewport: (f32, f32),
        final_pass: bool,
    ) {
        let lowered = zgui_text_style::lower::text_style(style);
        let size_device = lowered.size.0 * scale;
        let weight = lowered.weight.round().clamp(1.0, 1000.0) as u16;
        let line_height_px = match lowered.line_height {
            zgui_text_style::LineHeight::Normal => None,
            zgui_text_style::LineHeight::Number(multiple) => Some(size_device * multiple),
            zgui_text_style::LineHeight::Length(px) => Some(px.0 * scale),
        };
        // Letter spacing resolves its percentage against the font size, per the property.
        let letter_spacing = lowered.letter_spacing.resolve(lowered.size).0 * scale;
        let tab_width = match style.get_inherited_text().tab_size {
            zgui_css::values::text::TabSize::Number(count) => {
                (count.0.max(0.0).round() as u32).max(1)
            }
            zgui_css::values::text::TabSize::Length(_) => 1,
        };

        // Whether the face has to be measured again.
        let same = self.metrics.font_size == size_device
            && self.metrics.weight == weight
            && self.metrics.letter_spacing == letter_spacing
            && self.metrics.tab_width == tab_width
            && self.metrics.scale == scale
            && self.styled_families_match(&lowered.family);
        if !same {
            let generation = self.metrics.generation.wrapping_add(1);
            if let Some(shaper) = self.shaper.as_ref() {
                self.families = families::resolve(shaper, lowered.family.entries(), size_device);
                if let Some(resolved) =
                    shaper.line_metrics(&families::plain_request(&self.families, size_device))
                {
                    self.metrics = TextMetrics::from_face(
                        &resolved,
                        size_device,
                        line_height_px,
                        letter_spacing,
                        weight,
                        self.metrics.ligatures,
                        tab_width,
                        scale,
                        generation,
                    );
                } else {
                    self.metrics = TextMetrics {
                        font_size: size_device,
                        weight,
                        letter_spacing,
                        tab_width,
                        scale,
                        generation,
                        ..TextMetrics::default()
                    };
                }
            }
            self.shaping.clear();
            self.lines.clear();
            self.max_line_width = 0.0;
        }

        let theme = theme::from_style(style, self.syntax.capture_names());
        if theme != self.theme {
            self.syntax.resolve_colors(&theme);
            self.theme = theme;
            self.theme_version = self.theme_version.wrapping_add(1);
        }
        self.resolve_decoration_colors(style);
        self.resolve_looks();

        if !final_pass {
            return;
        }
        self.viewport = viewport;

        let report = StyleReport {
            metrics: self.metrics,
            theme_version: self.theme_version,
            viewport,
        };
        if self.reported != Some(report) {
            self.reported = Some(report);
            if let Some(tell) = self.on_style.as_ref() {
                tell(report);
            }
        }
    }

    /// Whether the styled family list is the one the current families were resolved from.
    ///
    /// Cheap and conservative: the resolved list always begins with the styled names that
    /// exist, so comparing the styled names against the front of the resolved list catches a
    /// changed sheet without holding the servo value.
    fn styled_families_match(&self, styled: &zgui_text_style::FontFamilyList) -> bool {
        if self.families.is_empty() {
            return false;
        }
        let mut wanted = styled.entries().iter().filter_map(|name| match name {
            zgui_text_style::FamilyName::Named(ident) => Some(*ident),
            zgui_text_style::FamilyName::Generic(_) => None,
        });
        let mut held = self.families.iter();
        wanted.all(|name| held.any(|resolved| *resolved == name))
    }

    // ---- Decorations -------------------------------------------------------------------

    /// Reads the colour of every custom property a decoration named off the style.
    ///
    /// Decorations name properties rather than carrying colours, so the set to resolve is
    /// whatever is in the layers right now. The names are collected first because resolving
    /// writes into the map the layers would otherwise be borrowed beside.
    fn resolve_decoration_colors(&mut self, style: &ComputedStyle) {
        let names: Vec<String> = self.named_properties().map(str::to_owned).collect();

        self.decoration_colors.clear();
        for name in names {
            let color = zgui_css::values::custom::color(style, &name);
            self.decoration_colors.insert(name, color);
        }
    }

    /// Every custom property a decoration, a gutter mark or the gutter source names.
    fn named_properties(&self) -> impl Iterator<Item = &str> {
        let source = self
            .gutter_source
            .iter()
            .flat_map(|source| source.paints.iter())
            .filter_map(|paint| match paint {
                Paint::Property(name) => Some(name.as_ref()),
                Paint::Color(_) => None,
            });
        self.decorations
            .properties()
            .chain(self.gutter_marks.properties())
            .chain(self.styles.properties())
            .chain(source)
    }

    /// Resolves the text styles with the colours the sheet answered.
    pub fn resolve_looks(&mut self) {
        let (fg, bg) = (self.theme.fg, self.theme.bg);
        let colors = &self.decoration_colors;
        self.styles.resolve(
            |paint| match paint {
                Paint::Color(color) => *color,
                Paint::Property(name) => colors.get(name.as_ref()).copied().flatten().unwrap_or(fg),
            },
            fg,
            bg,
        );
    }

    /// Whether any decoration names a property that has not been looked up yet.
    ///
    /// True after a layer is set that mentions a colour the last layout never saw, which is the
    /// one case where drawing has to wait for the style to be read again.
    pub fn decorations_need_style(&self) -> bool {
        self.named_properties()
            .any(|name| !self.decoration_colors.contains_key(name))
    }

    /// The bytes of the lines in `visible`, which is what a decoration layer is asked over.
    fn visible_bytes(&self, visible: &Range<usize>) -> Range<usize> {
        let rope = self.rope();
        if visible.is_empty() {
            return 0..0;
        }
        let start = position::line_start(&rope, visible.start);
        let end = if visible.end >= position::line_count(&rope) {
            rope.len_bytes()
        } else {
            position::line_start(&rope, visible.end)
        };
        start..end.max(start)
    }

    /// What one decoration is drawn with.
    ///
    /// A property no rule set falls back to the text's own colour: a decoration nobody can see is
    /// worse than one in the wrong colour, because only one of the two ever gets reported.
    fn paint_color(&self, paint: &Paint) -> Color {
        match paint {
            Paint::Color(color) => *color,
            Paint::Property(name) => self
                .decoration_colors
                .get(name.as_ref())
                .copied()
                .flatten()
                .unwrap_or(self.theme.fg),
        }
    }

    /// Where `range` sits on `line`, as x offsets inside the text area, when any of it does.
    ///
    /// The one piece of geometry both selections and decorations need: which part of a byte range
    /// falls on one line, in the coordinates that line's glyphs were placed in.
    fn extent_on_line(&mut self, range: &Range<usize>, line: usize) -> Option<(f32, f32)> {
        let rope = self.rope();
        let line_start = position::line_start(&rope, line);
        let line_end = position::line_end(&rope, line);
        if range.start > line_end || range.end < line_start {
            return None;
        }
        let cell = self.metrics.cell_advance;
        let cached = self.ensure_line(line)?;
        let seg_start = range.start.max(line_start);
        let x0 = if seg_start <= line_start {
            0.0
        } else {
            line_cache::caret_x(&cached.shaped, (seg_start - line_start) as u32)
        };
        let x1 = if range.end > line_end {
            // The break is covered too, shown as half a cell beyond the text.
            cached.shaped.width + cell * 0.5
        } else {
            line_cache::caret_x(&cached.shaped, (range.end - line_start) as u32)
        };
        Some((x0, x1))
    }

    /// How many graphemes `line` holds.
    fn column_count(&self, line: usize) -> u32 {
        let rope = self.rope();
        position::grapheme_col(&rope, position::line_end(&rope, line)) as u32
    }

    /// Where the left edge of `column` on `line` sits, in x offsets inside the text area.
    ///
    /// A column past the end of the line carries on in whole cells, which is what lets a block
    /// selection and its caret reach past the text.
    pub fn cell_x(&mut self, line: usize, column: u32) -> f32 {
        let cell = self.metrics.cell_advance;
        let columns = self.column_count(line);
        let rope = self.rope();
        let start = position::line_start(&rope, line);
        let byte = position::byte_at_col(&rope, line, column.min(columns) as usize);
        let Some(cached) = self.ensure_line(line) else {
            return column as f32 * cell;
        };
        let x = line_cache::caret_x(&cached.shaped, (byte - start) as u32);
        x + column.saturating_sub(columns) as f32 * cell
    }

    // ---- Lines -------------------------------------------------------------------------

    /// The painted form of `line`, rebuilt if any of its stamps went stale.
    pub fn ensure_line(&mut self, line: usize) -> Option<CachedLine> {
        let revision = self.revision();
        let generation = self.metrics.generation;
        let hl_version = self.syntax.version();
        let theme_version = self.theme_version;
        let style_version = self.styles.version();

        if let Some(cached) = self.lines.get(line)
            && cached.revision == revision
            && cached.generation == generation
        {
            if cached.hl_version == hl_version
                && cached.theme_version == theme_version
                && cached.style_version == style_version
            {
                return Some(cached.clone());
            }
            // The text is fine; only the colours moved.
            let shaped = cached.shaped.clone();
            let spans = self.syntax.colored_spans(line);
            let looks = self.styles.looks_of(line);
            let slices = line_cache::build_slices(&shaped, &spans, &looks, self.theme.fg);
            let rebuilt = CachedLine {
                revision,
                generation,
                hl_version,
                theme_version,
                style_version,
                shaped,
                slices,
            };
            self.lines.put(line, rebuilt.clone());
            return Some(rebuilt);
        }

        // Read before the shaper is taken: the text comes from the document, and holding both
        // borrows at once is what the borrow checker is here to stop.
        let rope = self.rope();
        let text = position::line_text(&rope, line);
        let shaper = self.shaper.as_mut()?;
        let shaped = self
            .shaping
            .shape(shaper, &self.families, &text, &self.metrics);
        self.max_line_width = self.max_line_width.max(shaped.width);
        let spans = self.syntax.colored_spans(line);
        let looks = self.styles.looks_of(line);
        let slices = line_cache::build_slices(&shaped, &spans, &looks, self.theme.fg);
        let cached = CachedLine {
            revision,
            generation,
            hl_version,
            theme_version,
            style_version,
            shaped,
            slices,
        };
        self.lines.put(line, cached.clone());
        Some(cached)
    }

    /// Where the caret at `byte` sits: its line, and its x within the text area before
    /// horizontal scrolling.
    pub fn caret_position(&mut self, byte: usize) -> (usize, f32) {
        let rope = self.rope();
        let line = position::line_of(&rope, byte);
        let local = (byte - position::line_start(&rope, line)) as u32;
        let x = self
            .ensure_line(line)
            .map(|cached| line_cache::caret_x(&cached.shaped, local))
            .unwrap_or(0.0);
        (line, x)
    }

    /// The byte a pointer at `(x, y)` element-local device pixels lands on.
    pub fn hit_test(&mut self, x: f32, y: f32) -> usize {
        let rope = self.rope();
        let total = position::line_count(&rope);
        let line_f = self.top_line() + f64::from(y) / f64::from(self.metrics.line_height);
        let line = (line_f.max(0.0) as usize).min(total.saturating_sub(1));
        let start = position::line_start(&rope, line);
        let text_x = x - self.gutter_width() + self.scroll.pos.x_px as f32;
        if text_x <= 0.0 {
            return start;
        }
        match self.ensure_line(line) {
            Some(cached) => {
                let local = line_cache::hit_byte(&cached.shaped, text_x);
                position::snap(&rope, start + local as usize)
            }
            None => start,
        }
    }

    // ---- Painting ----------------------------------------------------------------------

    /// Emits the whole frame.
    pub fn paint(&mut self, painter: &mut ScenePainter<'_>) {
        static PERF: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("ZGUI_EDITOR_PERF").is_some());
        let started = PERF.then(std::time::Instant::now);
        self.shaping.tick();
        let size = painter.size();
        let width = size.width.0;
        let height = size.height.0;
        // The painter's size is the placed content box — the one truth about how big the
        // editor is this frame. Everything geometric downstream reads `viewport`.
        self.viewport = (width, height);
        let metrics = self.metrics;
        let gutter_w = self.gutter_width();
        let text_x0 = gutter_w - self.scroll.pos.x_px as f32;
        let visible = self.visible_lines();
        let rope = self.rope();
        // Which line the caret is on: the overlay's when it places the carets, and the line the
        // selection's head is on otherwise.
        let caret_lines: Vec<usize> = if self.overlay.carets.is_empty() {
            self.selections
                .iter()
                .map(|selection| position::line_of(&rope, selection.head))
                .collect()
        } else {
            self.overlay.carets.iter().map(|caret| caret.line).collect()
        };

        // The gutter's own background.
        if let Some(color) = self.theme.gutter_bg
            && gutter_w > 0.0
        {
            fill(painter, 0.0, 0.0, gutter_w, height, color);
        }

        // The band behind each caret's line, in the view being worked in.
        if let Some(color) = self.theme.current_line.filter(|_| self.active) {
            for line in &caret_lines {
                if visible.contains(line) {
                    let y = self.line_y(*line);
                    fill(
                        painter,
                        gutter_w,
                        y,
                        width - gutter_w,
                        metrics.line_height,
                        color,
                    );
                }
            }
        }

        // The bands an application asked for, under the selection so that selecting decorated
        // text still reads as selected.
        self.paint_decoration_bands(painter, &visible, text_x0);
        self.paint_style_bands(painter, &visible, text_x0);

        // Selection bands.
        let selection_color = if self.focused {
            self.theme.selection
        } else {
            self.theme.selection_inactive
        };
        let selections: Vec<crate::core::selection::Selection> =
            self.selections.iter().copied().collect();
        // The overlay's cells, when it has any. They say what the bytes cannot: a rectangle that
        // keeps its width over a line ending inside it.
        for band in self.overlay.bands.clone() {
            if !visible.contains(&band.line) || band.columns.is_empty() {
                continue;
            }
            let x0 = self.cell_x(band.line, band.columns.start);
            let x1 = self.cell_x(band.line, band.columns.end);
            let y = self.line_y(band.line);
            fill(
                painter,
                text_x0 + x0,
                y,
                (x1 - x0).max(1.0),
                metrics.line_height,
                selection_color,
            );
        }
        for selection in &selections {
            if selection.is_caret() || !self.overlay.bands.is_empty() {
                continue;
            }
            let range = selection.range();
            let from_line = position::line_of(&rope, range.start).max(visible.start);
            let to_line = position::line_of(&rope, range.end).min(visible.end.saturating_sub(1));
            for line in from_line..=to_line.min(visible.end.saturating_sub(1)) {
                if !visible.contains(&line) {
                    continue;
                }
                let line_start = position::line_start(&rope, line);
                let line_end = position::line_end(&rope, line);
                let Some(cached) = self.ensure_line(line) else {
                    continue;
                };
                let seg_start = range.start.max(line_start);
                let x0 = if seg_start <= line_start {
                    0.0
                } else {
                    line_cache::caret_x(&cached.shaped, (seg_start - line_start) as u32)
                };
                let x1 = if range.end > line_end {
                    // The break is selected too, shown as half a cell beyond the text.
                    cached.shaped.width + metrics.cell_advance * 0.5
                } else {
                    line_cache::caret_x(&cached.shaped, (range.end - line_start) as u32)
                };
                let y = self.line_y(line);
                fill(
                    painter,
                    text_x0 + x0,
                    y,
                    (x1 - x0).max(1.0),
                    metrics.line_height,
                    selection_color,
                );
            }
        }

        // The gutter's numbers, or the application's labels in their place.
        if self.config.gutter != GutterMode::None {
            let caret_line = caret_lines.first().copied().unwrap_or(0);
            let number_right = gutter_w - metrics.cell_advance;
            let source = self.gutter_source.clone();
            for line in visible.clone() {
                let own = if line == caret_line {
                    self.theme.gutter_current_fg
                } else {
                    self.theme.gutter_fg
                };
                let (label, color) = match (&source, self.config.gutter) {
                    (Some(source), GutterMode::Custom { .. }) => match (source.label)(line) {
                        Some(label) => {
                            let color = label.paint.as_ref().map_or(own, |p| self.paint_color(p));
                            (label.text, color)
                        }
                        None => continue,
                    },
                    _ => match gutter::label(self.config.gutter, line, caret_line) {
                        Some(label) => (CompactString::from(label), own),
                        None => continue,
                    },
                };
                let Some(shaper) = self.shaper.as_mut() else {
                    break;
                };
                let shaped = self.shaping.shape(shaper, &self.families, &label, &metrics);
                let x = number_right - shaped.width;
                let y = self.line_y(line) + metrics.baseline - metrics.ascent;
                for run in shaped.runs.iter() {
                    painter.glyphs(
                        &run.as_run(PaintSlot(0)),
                        Point::new(DevicePx(x), DevicePx(y)),
                        color,
                    );
                }
            }
        }

        // The marks beside the numbers.
        self.paint_gutter_marks(painter, &visible);

        // The text.
        for line in visible.clone() {
            let Some(cached) = self.ensure_line(line) else {
                continue;
            };
            let y = self.line_y(line) + metrics.baseline - metrics.ascent;
            let origin = Point::new(DevicePx(text_x0), DevicePx(y));
            for slice in &cached.slices {
                let run = &cached.shaped.runs[slice.run as usize];
                let borrowed = ShapedRun {
                    glyphs: &run.glyphs[slice.glyphs.start as usize..slice.glyphs.end as usize],
                    synthetic_bold: if slice.bold {
                        run.synthetic_bold.max(SYNTHETIC_BOLD)
                    } else {
                        run.synthetic_bold
                    },
                    synthetic_slant: if slice.italic && run.synthetic_slant == 0.0 {
                        SYNTHETIC_SLANT
                    } else {
                        run.synthetic_slant
                    },
                    ..run.as_run(PaintSlot(0))
                };
                painter.glyphs(&borrowed, origin, slice.color);
            }
        }

        // The lines under the text, over it so a descender never hides an error.
        self.paint_decoration_underlines(painter, &visible, text_x0);
        self.paint_style_lines(painter, &visible, text_x0);

        // The carets.
        if self.focused
            || !self.overlay.bands.is_empty()
            || !selections.iter().all(|s| s.is_caret())
        {
            self.paint_carets(painter, &rope, &visible, text_x0);
        }

        // The scrollbars.
        let bar = self.scrollbar();
        let bar_width = self.scrollbar_width();
        if let Some(track) = self.theme.scrollbar_track
            && self.config.scrollbar
            && bar.thumb(0.0).is_some()
        {
            fill(painter, width - bar_width, 0.0, bar_width, height, track);
        }
        if self.config.scrollbar
            && let Some((top, thumb_height)) = bar.thumb(self.top_line())
        {
            fill(
                painter,
                width - bar_width,
                top as f32,
                bar_width,
                thumb_height as f32,
                self.theme.scrollbar_thumb,
            );
        }
        if let Some((left, thumb_width)) = self.horizontal_thumb() {
            fill(
                painter,
                left,
                height - bar_width,
                thumb_width,
                bar_width,
                self.theme.scrollbar_thumb,
            );
        }

        // The cache is kept a few viewports deep around what is on screen.
        let center = (visible.start + visible.end) / 2;
        let viewport_lines = self.viewport_lines() as usize;
        self.lines.trim(center, viewport_lines);

        if let Some(started) = started {
            let (hits, misses) = self.shaping.counts();
            eprintln!(
                "paint {:?}: lines {}..{}, shaped {} hit {} miss",
                started.elapsed(),
                visible.start,
                visible.end,
                hits,
                misses,
            );
        }
    }

    /// Draws the bands an application asked for behind the text.
    fn paint_decoration_bands(
        &mut self,
        painter: &mut ScenePainter<'_>,
        visible: &Range<usize>,
        text_x0: f32,
    ) {
        if self.decorations.is_empty() {
            return;
        }
        let rope = self.rope();
        let line_height = self.metrics.line_height;
        let gutter_w = self.gutter_width();
        let width = self.viewport.0;
        let bar = (2.0 * self.metrics.scale).max(1.0);
        let bytes = self.visible_bytes(visible);
        // Collected rather than iterated in place: drawing needs the shaped line, and shaping is
        // a mutation of the same value the layers live in. The line-wide kinds are drawn here,
        // under the byte bands, because they need no shaping at all.
        let mut bands: Vec<(Range<usize>, Color)> = Vec::new();
        for decoration in self.decorations.overlapping(bytes) {
            if decoration.range.is_empty() {
                continue;
            }
            match &decoration.kind {
                DecorationKind::Background(paint) => {
                    bands.push((decoration.range.clone(), self.paint_color(paint)));
                }
                DecorationKind::LineBackground(paint) => {
                    let color = self.paint_color(paint);
                    for line in lines_touched(&rope, &decoration.range, visible) {
                        let y = self.line_y(line);
                        fill(painter, gutter_w, y, width - gutter_w, line_height, color);
                    }
                }
                DecorationKind::LineBar(paint) => {
                    let color = self.paint_color(paint);
                    for line in lines_touched(&rope, &decoration.range, visible) {
                        let y = self.line_y(line);
                        fill(painter, 0.0, y, bar, line_height, color);
                    }
                }
                DecorationKind::Underline { .. } => {}
            }
        }

        for (range, color) in bands {
            for line in lines_of(&rope, &range, visible) {
                let Some((x0, x1)) = self.extent_on_line(&range, line) else {
                    continue;
                };
                let y = self.line_y(line);
                fill(
                    painter,
                    text_x0 + x0,
                    y,
                    (x1 - x0).max(1.0),
                    line_height,
                    color,
                );
            }
        }
    }

    /// Draws the backgrounds of the text styles behind the text.
    fn paint_style_bands(
        &mut self,
        painter: &mut ScenePainter<'_>,
        visible: &Range<usize>,
        text_x0: f32,
    ) {
        if self.styles.is_empty() {
            return;
        }
        let line_height = self.metrics.line_height;
        for line in visible.clone() {
            let looks = self.styles.looks_of(line);
            if looks.iter().all(|(_, _, look)| look.background.is_none()) {
                continue;
            }
            let Some(cached) = self.ensure_line(line) else {
                continue;
            };
            let y = self.line_y(line);
            for (start, end, look) in looks {
                let Some(color) = look.background else {
                    continue;
                };
                let x0 = line_cache::caret_x(&cached.shaped, start);
                let x1 = line_cache::caret_x(&cached.shaped, end);
                fill(
                    painter,
                    text_x0 + x0,
                    y,
                    (x1 - x0).max(1.0),
                    line_height,
                    color,
                );
            }
        }
    }

    /// Draws the underlines and the strike lines of the text styles over the text.
    fn paint_style_lines(
        &mut self,
        painter: &mut ScenePainter<'_>,
        visible: &Range<usize>,
        text_x0: f32,
    ) {
        if self.styles.is_empty() {
            return;
        }
        let metrics = self.metrics;
        let thickness = metrics.scale.max(1.0);
        for line in visible.clone() {
            let looks = self.styles.looks_of(line);
            if looks
                .iter()
                .all(|(_, _, look)| !look.underline && !look.strikethrough)
            {
                continue;
            }
            let Some(cached) = self.ensure_line(line) else {
                continue;
            };
            let top = self.line_y(line);
            let colors = self.syntax.colored_spans(line);
            for (start, end, look) in looks {
                let x0 = line_cache::caret_x(&cached.shaped, start);
                let x1 = line_cache::caret_x(&cached.shaped, end);
                let base = colors
                    .iter()
                    .find(|(from, to, _)| *from <= start && start < *to)
                    .map_or(self.theme.fg, |(_, _, color)| *color);
                let color = look.glyph_color(base);
                let width = (x1 - x0).max(1.0);
                if look.underline {
                    let y = top + metrics.baseline + thickness;
                    fill(painter, text_x0 + x0, y, width, thickness, color);
                }
                if look.strikethrough {
                    let y = top + metrics.baseline - metrics.ascent * 0.3;
                    fill(painter, text_x0 + x0, y, width, thickness, color);
                }
            }
        }
    }

    /// Draws the lines an application asked for under the text.
    fn paint_decoration_underlines(
        &mut self,
        painter: &mut ScenePainter<'_>,
        visible: &Range<usize>,
        text_x0: f32,
    ) {
        if self.decorations.is_empty() {
            return;
        }
        let rope = self.rope();
        let scale = self.metrics.scale.max(0.5);
        let line_height = self.metrics.line_height;
        let bytes = self.visible_bytes(visible);
        let underlines: Vec<(Range<usize>, UnderlineStyle, Color)> = self
            .decorations
            .overlapping(bytes)
            .filter(|decoration| !decoration.range.is_empty())
            .filter_map(|decoration| match &decoration.kind {
                DecorationKind::Underline { style, paint } => {
                    Some((decoration.range.clone(), *style, self.paint_color(paint)))
                }
                DecorationKind::Background(_)
                | DecorationKind::LineBackground(_)
                | DecorationKind::LineBar(_) => None,
            })
            .collect();

        for (range, style, color) in underlines {
            for line in lines_of(&rope, &range, visible) {
                let Some((x0, x1)) = self.extent_on_line(&range, line) else {
                    continue;
                };
                // Under the descenders rather than on the baseline, and inside the line box so a
                // squiggle never bleeds into the line below.
                let y = self.line_y(line) + line_height - 2.0 * scale;
                let (left, right) = (text_x0 + x0, text_x0 + x1.max(x0 + 1.0));
                match style {
                    UnderlineStyle::Straight => {
                        fill(painter, left, y, right - left, scale.max(1.0), color);
                    }
                    UnderlineStyle::Dotted | UnderlineStyle::Dashed | UnderlineStyle::Squiggly => {
                        stroke_underline(painter, left, right, y, scale, style, color);
                    }
                }
            }
        }
    }

    /// Draws the marks an application put beside the line numbers.
    ///
    /// In the gutter's leftmost cell, which is the padding the numbers already leave: a mark
    /// therefore never moves a number and never widens the gutter.
    fn paint_gutter_marks(&mut self, painter: &mut ScenePainter<'_>, visible: &Range<usize>) {
        if self.gutter_marks.is_empty() || self.config.gutter == GutterMode::None {
            return;
        }
        let metrics = self.metrics;
        let marks: Vec<(usize, CompactString, Color)> = self
            .gutter_marks
            .overlapping(visible.clone())
            .map(|mark| (mark.line, mark.text.clone(), self.paint_color(&mark.paint)))
            .collect();

        for (line, text, color) in marks {
            let Some(shaper) = self.shaper.as_mut() else {
                return;
            };
            let shaped = self.shaping.shape(shaper, &self.families, &text, &metrics);
            let y = self.line_y(line) + metrics.baseline - metrics.ascent;
            for run in shaped.runs.iter() {
                painter.glyphs(
                    &run.as_run(PaintSlot(0)),
                    Point::new(DevicePx(metrics.cell_advance * 0.25), DevicePx(y)),
                    color,
                );
            }
        }
    }

    /// Draws every visible caret in the configured style.
    fn paint_carets(
        &mut self,
        painter: &mut ScenePainter<'_>,
        rope: &ropey::Rope,
        visible: &Range<usize>,
        text_x0: f32,
    ) {
        let metrics = self.metrics;
        let style = if self.focused {
            self.config.cursor_style
        } else {
            CursorStyle::Hollow
        };
        if self.focused && self.config.blink && !self.blink_on {
            return;
        }
        // Each caret as the byte it sits on and how many whole cells past the end of its line it
        // is. Only an overlay reaches past the end; a selection's head is always a byte.
        let carets: Vec<(usize, u32)> = if self.overlay.carets.is_empty() {
            self.selections
                .iter()
                .map(|selection| (selection.head, 0))
                .collect()
        } else {
            let last = position::line_count(rope).saturating_sub(1);
            self.overlay
                .carets
                .iter()
                .map(|caret| {
                    let line = caret.line.min(last);
                    let columns = self.column_count(line);
                    (
                        position::byte_at_col(rope, line, caret.column.min(columns) as usize),
                        caret.column.saturating_sub(columns),
                    )
                })
                .collect()
        };
        for (head, past) in carets {
            let line = position::line_of(rope, head);
            if !visible.contains(&line) {
                continue;
            }
            let line_start = position::line_start(rope, line);
            let local = (head - line_start) as u32;
            let Some(cached) = self.ensure_line(line) else {
                continue;
            };
            let x = text_x0
                + line_cache::caret_x(&cached.shaped, local)
                + past as f32 * metrics.cell_advance;
            let y = self.line_y(line);
            // Past the end of the line every cell is one cell wide; on it, the grapheme decides.
            let width = if past > 0 {
                metrics.cell_advance
            } else {
                self.grapheme_width(&cached, rope, line_start, head)
            };
            match style {
                CursorStyle::Bar => {
                    let width = (2.0 * metrics.scale).max(1.0);
                    fill(painter, x, y, width, metrics.line_height, self.theme.cursor);
                }
                CursorStyle::Underline => {
                    let height = (2.0 * metrics.scale).max(1.0);
                    fill(
                        painter,
                        x,
                        y + metrics.line_height - height,
                        width,
                        height,
                        self.theme.cursor,
                    );
                }
                CursorStyle::Block | CursorStyle::Hollow => {
                    if style == CursorStyle::Hollow {
                        painter.stroke(
                            Rect::new(
                                Point::new(DevicePx(x), DevicePx(y)),
                                Size::new(DevicePx(width), DevicePx(metrics.line_height)),
                            ),
                            0.0,
                            (1.0 * metrics.scale).max(1.0),
                            self.theme.cursor,
                        );
                    } else {
                        fill(painter, x, y, width, metrics.line_height, self.theme.cursor);
                        if past > 0 {
                            // There is no character out here to redraw.
                            continue;
                        }
                        // The character under the block, redrawn in the cursor's text colour.
                        let expanded = cached.shaped.tab_map.to_expanded(local);
                        let origin = Point::new(
                            DevicePx(text_x0),
                            DevicePx(self.line_y(line) + metrics.baseline - metrics.ascent),
                        );
                        for run in cached.shaped.runs.iter() {
                            for (index, cluster) in run.clusters.iter().enumerate() {
                                if *cluster == expanded {
                                    let borrowed = ShapedRun {
                                        glyphs: &run.glyphs[index..index + 1],
                                        ..run.as_run(PaintSlot(0))
                                    };
                                    painter.glyphs(&borrowed, origin, self.theme.cursor_text);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// How wide the grapheme at `byte` paints, for block and underline carets.
    fn grapheme_width(
        &self,
        cached: &CachedLine,
        rope: &ropey::Rope,
        line_start: usize,
        byte: usize,
    ) -> f32 {
        let next = position::next_grapheme(rope, byte);
        let line_end = position::line_end(rope, position::line_of(rope, byte));
        if byte >= line_end || next > line_end {
            return self.metrics.cell_advance;
        }
        let here = line_cache::caret_x(&cached.shaped, (byte - line_start) as u32);
        let there = line_cache::caret_x(&cached.shaped, (next - line_start) as u32);
        (there - here).max(self.metrics.cell_advance * 0.5)
    }
}

/// Which visible lines `range` touches.
fn lines_of(rope: &ropey::Rope, range: &Range<usize>, visible: &Range<usize>) -> Range<usize> {
    let from = position::line_of(rope, range.start).max(visible.start);
    let to =
        position::line_of(rope, range.end.min(rope.len_bytes())).min(visible.end.saturating_sub(1));
    from..to.saturating_add(1).max(from)
}

/// The visible lines a half-open byte range holds any byte of.
///
/// Unlike [`lines_of`], a range that ends exactly where a line starts does not touch that line:
/// a line-wide mark over one line must not spill onto the next.
fn lines_touched(rope: &ropey::Rope, range: &Range<usize>, visible: &Range<usize>) -> Range<usize> {
    let last = range.end.saturating_sub(1).max(range.start);
    let from = position::line_of(rope, range.start).max(visible.start);
    let to = position::line_of(rope, last.min(rope.len_bytes())).min(visible.end.saturating_sub(1));
    from..to.saturating_add(1).max(from)
}

/// The outline and the stroke one underline is drawn as.
///
/// A wave and a dash pattern are both strokes rather than quads, so both are expressed here and
/// drawn by the one call that can say them. Separated from the painting so that the geometry —
/// where a squiggle's peaks land, how long a dash is — is assertable without a window.
pub(crate) fn underline_stroke(
    left: f32,
    right: f32,
    y: f32,
    scale: f32,
    style: UnderlineStyle,
) -> (kurbo::BezPath, kurbo::Stroke) {
    let width = scale.max(1.0);
    let mut path = kurbo::BezPath::new();
    let mut stroke = kurbo::Stroke::new(f64::from(width));
    let (left, right, y) = (f64::from(left), f64::from(right), f64::from(y));

    match style {
        UnderlineStyle::Squiggly => {
            // A triangle wave: a period of four device pixels at scale one reads as a squiggle at
            // every font size an editor is used at, and costs four points per period. The peaks
            // sit one amplitude either side of `y`, so the wave stays inside the room the caller
            // left for it at the bottom of the line box.
            let period = f64::from(4.0 * scale);
            let amplitude = f64::from(1.5 * scale);
            let mut x = left;
            path.move_to((x, y));
            let mut up = true;
            while x < right {
                let next = (x + period / 2.0).min(right);
                path.line_to((next, if up { y - amplitude } else { y + amplitude }));
                up = !up;
                x = next;
            }
            stroke = stroke
                .with_caps(kurbo::Cap::Round)
                .with_join(kurbo::Join::Round);
        }
        UnderlineStyle::Dotted => {
            path.move_to((left, y));
            path.line_to((right, y));
            let unit = f64::from(width);
            stroke = stroke
                .with_caps(kurbo::Cap::Round)
                .with_dashes(0.0, [unit, unit * 2.0]);
        }
        UnderlineStyle::Dashed => {
            path.move_to((left, y));
            path.line_to((right, y));
            let unit = f64::from(width);
            stroke = stroke.with_dashes(0.0, [unit * 4.0, unit * 3.0]);
        }
        UnderlineStyle::Straight => {
            path.move_to((left, y));
            path.line_to((right, y));
        }
    }

    (path, stroke)
}

/// Draws an underline that is not a plain bar, through the vector pipeline.
///
/// Dearer than a quad — a stroke is rasterised — which is why a straight underline is a quad and
/// does not come here.
fn stroke_underline(
    painter: &mut ScenePainter<'_>,
    left: f32,
    right: f32,
    y: f32,
    scale: f32,
    style: UnderlineStyle,
    color: Color,
) {
    let (path, stroke) = underline_stroke(left, right, y, scale, style);
    painter.shape(&zgui::canvas::Shape {
        path: std::sync::Arc::new(path),
        fill: None,
        stroke: Some(zgui::canvas::Stroke {
            paint: zgui::canvas::Paint::Solid(zgui::canvas::Ink::Solid(color)),
            style: stroke,
        }),
        clips: Vec::new(),
    });
}

/// One opaque rectangle, the quad pipeline's unit.
fn fill(painter: &mut ScenePainter<'_>, x: f32, y: f32, width: f32, height: f32, color: Color) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    painter.fill(
        Rect::new(
            Point::new(DevicePx(x), DevicePx(y)),
            Size::<DevicePx, Device>::new(DevicePx(width), DevicePx(height)),
        ),
        0.0,
        color,
    );
}

#[cfg(test)]
mod tests {
    use zgui::elements::kurbo::{PathEl, Shape as _};

    use super::{EditorShared, underline_stroke};
    use crate::config::EditorConfig;
    use crate::decoration::UnderlineStyle;
    use crate::document::Document;

    /// A view over `text` drawing `window`, with a viewport tall enough for the whole text.
    fn windowed(text: &str, window: Option<std::ops::Range<usize>>) -> EditorShared {
        let config = EditorConfig {
            line_window: window,
            ..EditorConfig::default()
        };
        let mut shared = EditorShared::new(Document::new(text), config, None);
        shared.viewport = (400.0, 20.0 * 20.0);
        shared
    }

    #[test]
    fn a_view_with_no_window_draws_from_where_it_is_scrolled() {
        let mut shared = windowed("a\nb\nc\nd\ne\n", None);
        shared.scroll.pos.line = 2.0;
        assert_eq!(shared.top_line(), 2.0);
        assert_eq!(shared.visible_lines().start, 2);
        assert_eq!(shared.line_y(2), 0.0, "the top line sits at the top");
    }

    #[test]
    fn a_window_is_the_lines_that_are_drawn() {
        let shared = windowed("a\nb\nc\nd\ne\n", Some(1..3));
        assert_eq!(shared.visible_lines(), 1..3);
        assert_eq!(shared.line_y(1), 0.0, "the window's first line is the top");
        assert_eq!(shared.line_y(2), shared.metrics.line_height);
    }

    #[test]
    fn a_window_does_not_scroll() {
        // Nothing above the first line to scroll to, so a scroll position that moved under it
        // must change nothing about what is drawn.
        let mut shared = windowed("a\nb\nc\nd\ne\n", Some(1..3));
        shared.scroll.pos.line = 4.0;
        assert_eq!(shared.top_line(), 1.0);
        assert_eq!(shared.visible_lines(), 1..3);
        assert!(
            shared.scrollbar().thumb(shared.top_line()).is_none(),
            "a view as tall as what it draws has no thumb"
        );
    }

    #[test]
    fn a_window_over_lines_that_have_gone_is_what_is_left_of_them() {
        // A document edited down under a window must leave a window over real lines, and never a
        // range that reads past the end of the rope.
        let shared = windowed("a\nb\n", Some(1..9));
        assert_eq!(shared.line_window(), Some(1..3));
        assert_eq!(shared.visible_lines(), 1..3);
    }

    #[test]
    fn a_window_over_nothing_is_no_window() {
        let shared = windowed("a\nb\nc\n", Some(2..2));
        assert_eq!(shared.line_window(), None);
        let past = windowed("a\nb\nc\n", Some(9..12));
        assert_eq!(past.line_window(), None);
    }

    #[test]
    fn a_squiggle_waves_inside_the_room_it_was_given() {
        let (path, stroke) = underline_stroke(10.0, 42.0, 100.0, 1.0, UnderlineStyle::Squiggly);
        let bounds = path.bounding_box();
        assert!(bounds.x0 >= 10.0 && bounds.x1 <= 42.0, "{bounds:?}");
        // One amplitude either side of the line it is under, and no further.
        assert!((bounds.y0 - 98.5).abs() < 0.01, "{bounds:?}");
        assert!((bounds.y1 - 101.5).abs() < 0.01, "{bounds:?}");
        assert!(stroke.dash_pattern.is_empty(), "a wave is not dashed");

        // Sixteen half-periods over thirty-two pixels, and a peak at each.
        let peaks = path
            .elements()
            .iter()
            .filter(|el| matches!(el, PathEl::LineTo(_)))
            .count();
        assert_eq!(peaks, 16);
    }

    #[test]
    fn a_squiggle_over_no_width_is_one_point() {
        // A decoration on an empty line has nowhere to wave, and must still be a path the
        // rasteriser can take rather than a loop that never ends.
        let (path, _) = underline_stroke(10.0, 10.0, 100.0, 1.0, UnderlineStyle::Squiggly);
        assert_eq!(path.elements().len(), 1);
    }

    #[test]
    fn a_squiggle_scales_with_the_display() {
        let (one, _) = underline_stroke(0.0, 40.0, 0.0, 1.0, UnderlineStyle::Squiggly);
        let (two, _) = underline_stroke(0.0, 40.0, 0.0, 2.0, UnderlineStyle::Squiggly);
        assert!(
            two.bounding_box().height() > one.bounding_box().height(),
            "a squiggle on a doubled display is drawn twice as tall"
        );
    }

    #[test]
    fn dashes_and_dots_are_one_straight_line_with_a_pattern() {
        for style in [UnderlineStyle::Dotted, UnderlineStyle::Dashed] {
            let (path, stroke) = underline_stroke(0.0, 40.0, 5.0, 1.0, UnderlineStyle::Straight);
            assert_eq!(path.elements().len(), 2);
            assert!(
                stroke.dash_pattern.is_empty(),
                "a straight line is not dashed"
            );

            let (path, stroke) = underline_stroke(0.0, 40.0, 5.0, 1.0, style);
            assert_eq!(path.elements().len(), 2, "{style:?} is one segment");
            assert!(
                !stroke.dash_pattern.is_empty(),
                "{style:?} carries a pattern"
            );
        }
    }
}
