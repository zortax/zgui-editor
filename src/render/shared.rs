//! The one place the view's handlers and the element's frames meet.
//!
//! Everything the editor is — the model, the scroll position, the metrics, the theme, the
//! caches — lives here behind one `Rc<RefCell<_>>`. Event handlers mutate it and say so through
//! the element handle; `layout` reads the style into it; `paint` reads it out as primitives.
//! Nothing else holds state, so nothing else can disagree.

use std::ops::Range;
use std::rc::Rc;

use zgui::app::Shaper;
use zgui::canvas::zgui_color::Color;
use zgui::custom::{PaintSlot, ScenePainter, ShapedRun};
use zgui::geom::{Device, DevicePx, Point, Rect, Size};
use zgui_css::ComputedStyle;
use zgui_interned::Ident;

use crate::config::{CursorStyle, EditorConfig, GutterMode};
use crate::core::motion::MotionContext;
use crate::core::{EditorState, position};
use crate::render::line_cache::{self, CachedLine, LineCache};
use crate::render::metrics::TextMetrics;
use crate::render::shaping::ShapingCache;
use crate::render::theme::{self, Theme};
use crate::render::{families, gutter};
use crate::scroll::{ScrollState, Scrollbar};
use crate::syntax::SyntaxState;

/// How wide the scrollbar is, in device pixels at scale one.
const SCROLLBAR_WIDTH: f32 = 10.0;

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

/// The editor, as one mutable value.
pub struct EditorShared {
    /// The model.
    pub state: EditorState,
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
    /// The way to the syntax worker, once one runs.
    pub syntax_tx: Option<flume::Sender<crate::syntax::worker::ToWorker>>,
    /// The visible range last asked of the worker, so scrolling asks again only when it must.
    pub requested_window: Range<usize>,
    /// The element's content size, in device pixels, from the last layout.
    pub viewport: (f32, f32),
    /// Whether the editor has focus.
    pub focused: bool,
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
    /// A fresh editor over `text`.
    pub fn new(text: &str, config: EditorConfig, shaper: Option<Shaper>) -> Self {
        Self {
            state: {
                let mut state = EditorState::new(text);
                state.options = config.edit.clone();
                state
            },
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
            syntax_tx: None,
            requested_window: 0..0,
            viewport: (0.0, 0.0),
            focused: false,
            blink_on: true,
            max_line_width: 0.0,
            max_line_chars: None,
            on_style: None,
            reported: None,
        }
    }

    // ---- Geometry ----------------------------------------------------------------------

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

    /// How wide the scrollbar is, in device pixels.
    pub fn scrollbar_width(&self) -> f32 {
        SCROLLBAR_WIDTH * self.metrics.scale.max(0.5)
    }

    /// The vertical scrollbar's geometry right now.
    pub fn scrollbar(&self) -> Scrollbar {
        Scrollbar {
            track: f64::from(self.viewport.1),
            total: position::line_count(self.state.buffer.rope()),
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
            x >= self.viewport.0 - bar && self.scrollbar().thumb(self.scroll.pos.line).is_some();
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
            position::line_count(self.state.buffer.rope()),
            self.metrics.cell_advance,
        )
    }

    /// The top of `line`, in device pixels from the content box's top.
    pub fn line_y(&self, line: usize) -> f32 {
        ((line as f64 - self.scroll.pos.line) * f64::from(self.metrics.line_height)) as f32
    }

    /// The lines any part of which is on screen.
    pub fn visible_lines(&self) -> Range<usize> {
        let total = position::line_count(self.state.buffer.rope());
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

        let theme = theme::from_style(style);
        if theme != self.theme {
            self.syntax.resolve_colors(&theme);
            self.theme = theme;
            self.theme_version = self.theme_version.wrapping_add(1);
        }

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

    // ---- Lines -------------------------------------------------------------------------

    /// The painted form of `line`, rebuilt if any of its stamps went stale.
    pub fn ensure_line(&mut self, line: usize) -> Option<CachedLine> {
        let revision = self.state.buffer.revision();
        let generation = self.metrics.generation;
        let hl_version = self.syntax.version();
        let theme_version = self.theme_version;

        if let Some(cached) = self.lines.get(line)
            && cached.revision == revision
            && cached.generation == generation
        {
            if cached.hl_version == hl_version && cached.theme_version == theme_version {
                return Some(cached.clone());
            }
            // The text is fine; only the colours moved.
            let shaped = cached.shaped.clone();
            let spans = self.syntax.colored_spans(line);
            let slices = line_cache::build_slices(&shaped, &spans, self.theme.fg);
            let rebuilt = CachedLine {
                revision,
                generation,
                hl_version,
                theme_version,
                shaped,
                slices,
            };
            self.lines.put(line, rebuilt.clone());
            return Some(rebuilt);
        }

        let shaper = self.shaper.as_mut()?;
        let text = position::line_text(self.state.buffer.rope(), line);
        let shaped = self
            .shaping
            .shape(shaper, &self.families, &text, &self.metrics);
        self.max_line_width = self.max_line_width.max(shaped.width);
        let spans = self.syntax.colored_spans(line);
        let slices = line_cache::build_slices(&shaped, &spans, self.theme.fg);
        let cached = CachedLine {
            revision,
            generation,
            hl_version,
            theme_version,
            shaped,
            slices,
        };
        self.lines.put(line, cached.clone());
        Some(cached)
    }

    /// Where the caret at `byte` sits: its line, and its x within the text area before
    /// horizontal scrolling.
    pub fn caret_position(&mut self, byte: usize) -> (usize, f32) {
        let rope = self.state.buffer.rope().clone();
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
        let rope = self.state.buffer.rope().clone();
        let total = position::line_count(&rope);
        let line_f = self.scroll.pos.line + f64::from(y) / f64::from(self.metrics.line_height);
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
        let rope = self.state.buffer.rope().clone();
        let caret_lines: Vec<usize> = self
            .state
            .selections
            .iter()
            .map(|selection| position::line_of(&rope, selection.head))
            .collect();

        // The gutter's own background.
        if let Some(color) = self.theme.gutter_bg
            && gutter_w > 0.0
        {
            fill(painter, 0.0, 0.0, gutter_w, height, color);
        }

        // The band behind each caret's line.
        if let Some(color) = self.theme.current_line {
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

        // Selection bands.
        let selection_color = if self.focused {
            self.theme.selection
        } else {
            self.theme.selection_inactive
        };
        let selections: Vec<crate::core::selection::Selection> =
            self.state.selections.iter().copied().collect();
        for selection in &selections {
            if selection.is_caret() {
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

        // The gutter's numbers.
        if self.config.gutter != GutterMode::None {
            let caret_line = caret_lines.first().copied().unwrap_or(0);
            let number_right = gutter_w - metrics.cell_advance;
            for line in visible.clone() {
                let Some(label) = gutter::label(self.config.gutter, line, caret_line) else {
                    continue;
                };
                let color = if line == caret_line {
                    self.theme.gutter_current_fg
                } else {
                    self.theme.gutter_fg
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
                    ..run.as_run(PaintSlot(0))
                };
                painter.glyphs(&borrowed, origin, slice.color);
            }
        }

        // The carets.
        if self.focused || !selections.iter().all(|s| s.is_caret()) {
            self.paint_carets(painter, &rope, &visible, text_x0);
        }

        // The scrollbars.
        let bar = self.scrollbar();
        let bar_width = self.scrollbar_width();
        if let Some(track) = self.theme.scrollbar_track
            && bar.thumb(0.0).is_some()
        {
            fill(painter, width - bar_width, 0.0, bar_width, height, track);
        }
        if let Some((top, thumb_height)) = bar.thumb(self.scroll.pos.line) {
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
        let selections: Vec<crate::core::selection::Selection> =
            self.state.selections.iter().copied().collect();
        for selection in &selections {
            let head = selection.head;
            let line = position::line_of(rope, head);
            if !visible.contains(&line) {
                continue;
            }
            let line_start = position::line_start(rope, line);
            let local = (head - line_start) as u32;
            let Some(cached) = self.ensure_line(line) else {
                continue;
            };
            let x = text_x0 + line_cache::caret_x(&cached.shaped, local);
            let y = self.line_y(line);
            match style {
                CursorStyle::Bar => {
                    let width = (2.0 * metrics.scale).max(1.0);
                    fill(painter, x, y, width, metrics.line_height, self.theme.cursor);
                }
                CursorStyle::Underline => {
                    let height = (2.0 * metrics.scale).max(1.0);
                    let width = self.grapheme_width(&cached, rope, line_start, head);
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
                    let width = self.grapheme_width(&cached, rope, line_start, head);
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
