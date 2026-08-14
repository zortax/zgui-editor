//! The application's side of a mounted editor.
//!
//! An [`EditorHandle`] is how everything outside the component drives it: commands go in
//! fire-and-forget, state comes out either synchronously through [`EditorHandle::query`] — which
//! is what makes a vim layer or a completion engine implementable outside the crate — or
//! reactively through signals, which is what a status line binds to.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use zgui::platform::ClipboardKind;
use zgui::prelude::*;
use zgui::runtime::clipboard::Clipboards;

use crate::command::{Clipboard, Command, InsertPoint, Motion, ScrollCmd};
use crate::config::{CursorStyle, GutterMode};
use crate::core::motion::{self, MotionContext};
use crate::core::search::SearchDirection;
use crate::core::selection::{Selection, Selections};
use crate::core::{EditorState, Response, ScrollEffect, position, search, words};
use crate::event::EditorEvent;
use crate::render::element::EditorElement;
use crate::render::shared::EditorShared;
use crate::syntax::registry::LanguageRegistry;

/// The caret's place, as a status line shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CursorPos {
    /// The line, counting from zero.
    pub line: usize,
    /// The grapheme column, counting from zero.
    pub col: usize,
}

/// Where the view sits, as a scrollbar or minimap outside the component would draw it.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ScrollSnapshot {
    /// The line at the top of the view, fractionally.
    pub top_line: f64,
    /// The greatest top line the document allows.
    pub max_top: f64,
    /// How many lines the view shows.
    pub viewport_lines: f64,
}

/// Where the primary caret sits on the window, in CSS pixels — what anchors a popover.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CaretRect {
    /// The left edge.
    pub x: f32,
    /// The top edge.
    pub y: f32,
    /// The caret's width.
    pub width: f32,
    /// The line's height.
    pub height: f32,
}

/// What a [`EditorHandle::query`] closure is handed: read access to the model.
pub struct EditorSnapshot<'a> {
    state: &'a EditorState,
    context: MotionContext,
}

impl EditorSnapshot<'_> {
    /// The text. Cloning the rope is O(1).
    pub fn rope(&self) -> &ropey::Rope {
        self.state.buffer.rope()
    }

    /// Which revision of the text this is.
    pub fn revision(&self) -> u64 {
        self.state.buffer.revision()
    }

    /// The selections, in document order.
    pub fn selections(&self) -> &Selections {
        &self.state.selections
    }

    /// How many lines the text has.
    pub fn line_count(&self) -> usize {
        position::line_count(self.state.buffer.rope())
    }

    /// The text in `range`, copied out.
    pub fn text_in(&self, range: Range<usize>) -> String {
        self.state.buffer.rope().byte_slice(range).to_string()
    }

    /// The line and grapheme column of `byte`.
    pub fn line_col(&self, byte: usize) -> CursorPos {
        let (line, col) = position::line_col(self.state.buffer.rope(), byte);
        CursorPos { line, col }
    }

    /// The word under `byte`, as a double click selects it.
    pub fn word_at(&self, byte: usize) -> Range<usize> {
        words::word_at(self.state.buffer.rope(), byte)
    }

    /// The bytes `motion` would span from `selection` — what an operator like `d` or `y` takes,
    /// answered without moving anything.
    pub fn motion_range(
        &self,
        selection: Selection,
        motion: Motion,
        count: u32,
        linewise: bool,
    ) -> Range<usize> {
        motion::motion_range(
            self.state.buffer.rope(),
            selection,
            motion,
            count,
            linewise,
            self.context,
        )
    }

    /// The first match for `needle` from `from`, looking `direction`, wrapping when asked.
    pub fn search(
        &self,
        needle: &str,
        from: usize,
        direction: SearchDirection,
        wrap: bool,
    ) -> Option<Range<usize>> {
        search::find(self.state.buffer.rope(), needle, from, direction, wrap)
    }
}

/// The reactive reads a handle publishes.
pub(crate) struct HandleSignals {
    pub revision: RwSignal<u64, LocalStorage>,
    pub cursor: RwSignal<CursorPos, LocalStorage>,
    pub selection: RwSignal<Selection, LocalStorage>,
    pub scroll: RwSignal<ScrollSnapshot, LocalStorage>,
    pub caret_rect: RwSignal<Option<CaretRect>, LocalStorage>,
}

impl HandleSignals {
    fn new() -> Self {
        Self {
            revision: RwSignal::new_local(0),
            cursor: RwSignal::new_local(CursorPos::default()),
            selection: RwSignal::new_local(Selection::caret(0)),
            scroll: RwSignal::new_local(ScrollSnapshot::default()),
            caret_rect: RwSignal::new_local(None),
        }
    }
}

/// The report callback an application gave the component, when it gave one.
type EventReporter = Option<Box<dyn Fn(EditorEvent)>>;

/// Everything a dispatch needs beyond the model itself. Owned by the component, shared with
/// every handle clone.
pub(crate) struct EditorCtx {
    pub shared: Rc<RefCell<EditorShared>>,
    pub element: zgui::custom::CustomHandle<EditorElement>,
    pub port: NodeRef,
    pub clipboards: Option<Clipboards>,
    pub on_event: RefCell<EventReporter>,
    pub signals: HandleSignals,
    /// Starts the scroll glide's timer; set by the view, which owns the timers.
    pub start_glide: RefCell<Option<Box<dyn Fn()>>>,
    /// The languages this editor can highlight.
    pub registry: LanguageRegistry,
}

/// Drives one mounted editor. Cloneable; UI-thread only.
#[derive(Clone)]
pub struct EditorHandle {
    pub(crate) ctx: Rc<EditorCtx>,
}

impl EditorHandle {
    pub(crate) fn new(
        shared: Rc<RefCell<EditorShared>>,
        element: zgui::custom::CustomHandle<EditorElement>,
        port: NodeRef,
        clipboards: Option<Clipboards>,
        on_event: Option<Box<dyn Fn(EditorEvent)>>,
        registry: LanguageRegistry,
    ) -> Self {
        Self {
            ctx: Rc::new(EditorCtx {
                shared,
                element,
                port,
                clipboards,
                on_event: RefCell::new(on_event),
                signals: HandleSignals::new(),
                start_glide: RefCell::new(None),
                registry,
            }),
        }
    }

    /// Applies one command.
    pub fn command(&self, command: Command) {
        self.dispatch(&command);
    }

    /// Applies several commands as one gesture.
    pub fn commands(&self, commands: impl IntoIterator<Item = Command>) {
        for command in commands {
            self.dispatch(&command);
        }
    }

    /// Reads the model synchronously.
    pub fn query<R>(&self, read: impl FnOnce(&EditorSnapshot<'_>) -> R) -> R {
        let shared = self.ctx.shared.borrow();
        let snapshot = EditorSnapshot {
            state: &shared.state,
            context: shared.motion_context(),
        };
        read(&snapshot)
    }

    /// Replaces the whole text, as opening a file does.
    pub fn set_text(&self, text: &str) {
        let response = {
            let mut shared = self.ctx.shared.borrow_mut();
            shared.lines.clear();
            shared.syntax.clear();
            shared.max_line_width = 0.0;
            shared.max_line_chars = None;
            shared.state.set_text(text)
        };
        self.settle(response);
        self.scan_max_width();
        self.ctx.element.relayout();
    }

    /// Highlights as `language` — a name in the registry — from now on; `None` turns
    /// highlighting off.
    pub fn set_language(&self, language: Option<&str>) {
        let config = language.and_then(|name| self.ctx.registry.by_name(name));
        let mut shared = self.ctx.shared.borrow_mut();
        let theme = shared.theme.clone();
        shared.syntax.set_captures(Vec::new(), &theme);
        shared.lines.clear();
        if let Some(tx) = shared.syntax_tx.as_ref() {
            let (rope, revision) = shared.state.snapshot();
            let _ = tx.send(crate::syntax::worker::ToWorker::SetLanguage(
                config, rope, revision,
            ));
            let visible = shared.visible_lines();
            let _ = tx.send(crate::syntax::worker::ToWorker::Window(visible.clone()));
            shared.requested_window = visible;
        }
        drop(shared);
        self.ctx.element.repaint();
    }

    /// The registry this editor resolves language names against.
    pub fn registry(&self) -> &LanguageRegistry {
        &self.ctx.registry
    }

    /// Takes one message from the syntax worker in.
    pub(crate) fn apply_highlight(&self, message: crate::syntax::worker::FromWorker) {
        let mut shared = self.ctx.shared.borrow_mut();
        match message {
            crate::syntax::worker::FromWorker::Captures(names) => {
                let theme = shared.theme.clone();
                shared.syntax.set_captures(names, &theme);
            }
            crate::syntax::worker::FromWorker::Frame(frame) => {
                if frame.revision != shared.state.buffer.revision() {
                    // The frame describes text that no longer exists; the worker converges on
                    // the newest text on its own.
                    return;
                }
                for (offset, spans) in frame.spans.into_iter().enumerate() {
                    shared.syntax.put_line(frame.lines.start + offset, spans);
                }
                shared.syntax.bump();
            }
        }
        drop(shared);
        self.ctx.element.repaint();
    }

    /// Changes what the caret looks like — a vim layer's mode change.
    pub fn set_cursor_style(&self, style: CursorStyle) {
        self.ctx.shared.borrow_mut().config.cursor_style = style;
        self.ctx.element.repaint();
    }

    /// Changes how the gutter numbers its lines.
    pub fn set_gutter(&self, mode: GutterMode) {
        self.ctx.shared.borrow_mut().config.gutter = mode;
        self.ctx.element.relayout();
    }

    /// Gives the editor focus.
    pub fn focus(&self) {
        self.ctx.port.focus();
    }

    /// The buffer revision, moving once per change.
    pub fn revision(&self) -> Signal<u64, LocalStorage> {
        self.ctx.signals.revision.into()
    }

    /// The primary caret's line and column.
    pub fn cursor_position(&self) -> Signal<CursorPos, LocalStorage> {
        self.ctx.signals.cursor.into()
    }

    /// The primary selection.
    pub fn primary_selection(&self) -> Signal<Selection, LocalStorage> {
        self.ctx.signals.selection.into()
    }

    /// Where the view sits.
    pub fn scroll_state(&self) -> Signal<ScrollSnapshot, LocalStorage> {
        self.ctx.signals.scroll.into()
    }

    /// Where the primary caret sits on the window, for anchoring popovers. `None` while it is
    /// scrolled out of view.
    pub fn caret_rect(&self) -> Signal<Option<CaretRect>, LocalStorage> {
        self.ctx.signals.caret_rect.into()
    }

    /// Scans the whole buffer for its longest line on a worker, so the horizontal scroll
    /// extent is right without the UI thread ever walking millions of lines.
    ///
    /// The answer is an estimate — characters times the cell advance — refined monotonically by
    /// the real shaped widths of whatever becomes visible.
    pub(crate) fn scan_max_width(&self) {
        let (rope, revision) = {
            let shared = self.ctx.shared.borrow();
            shared.state.snapshot()
        };
        let work = zgui::task::blocking(move || {
            rope.lines().map(|line| line.len_chars()).max().unwrap_or(0)
        });
        let handle = self.clone();
        let task = zgui::task::spawn_local(async move {
            let chars = work.await;
            let mut shared = handle.ctx.shared.borrow_mut();
            if shared.state.buffer.revision() != revision {
                return;
            }
            let extent_before = shared.horizontal_extent();
            shared.max_line_chars = Some(chars);
            if shared.horizontal_extent() != extent_before {
                drop(shared);
                handle.ctx.element.repaint();
            }
        });
        drop(task);
    }

    // ---- Internals ---------------------------------------------------------------------

    fn dispatch(&self, command: &Command) {
        let response = {
            let mut shared = self.ctx.shared.borrow_mut();
            let context = shared.motion_context();
            shared.state.apply(command, context)
        };
        self.settle(response);
    }

    /// Applies everything a [`Response`] asked of the view.
    pub(crate) fn settle(&self, response: Response) {
        let mut relayout = false;
        {
            let mut shared = self.ctx.shared.borrow_mut();
            let gutter_before = shared.gutter_width();

            if let Some(change) = response.change.as_ref() {
                // How many lines the change added or removed, and the last line it touched —
                // readable straight off the tree-sitter deltas it already carries.
                let line_delta: isize = change
                    .input_edits
                    .iter()
                    .map(|edit| {
                        edit.new_end_position.row as isize - edit.old_end_position.row as isize
                    })
                    .sum();
                let last_changed_line = change
                    .input_edits
                    .iter()
                    .map(|edit| edit.old_end_position.row.max(edit.new_end_position.row))
                    .max()
                    .unwrap_or(change.first_changed_line);
                if line_delta == 0 {
                    // Nothing below the edit moved: only the touched lines rebuild, and the
                    // highlight spans stay put — slightly stale for a frame or two, which
                    // reads as nothing, where dropping them reads as a white flash.
                    shared
                        .lines
                        .invalidate_range(change.first_changed_line, last_changed_line);
                } else {
                    shared.lines.invalidate_from(change.first_changed_line);
                    shared
                        .syntax
                        .shift_lines(change.first_changed_line, line_delta);
                }
                if let Some(tx) = shared.syntax_tx.as_ref() {
                    let (snapshot, revision) = shared.state.snapshot();
                    let _ = tx.send(crate::syntax::worker::ToWorker::Edited {
                        input_edits: change.input_edits.clone(),
                        snapshot,
                        revision,
                    });
                }
            }

            if response.change.is_some() || response.selection_changed {
                shared.blink_on = true;
            }

            match response.scroll {
                Some(ScrollEffect::EnsureVisible) => {
                    self.ensure_caret_visible(&mut shared);
                }
                Some(ScrollEffect::Command(command)) => {
                    self.apply_scroll(&mut shared, command);
                }
                None => {}
            }

            if shared.gutter_width() != gutter_before {
                relayout = true;
            }
            self.update_signals(&mut shared);
        }

        if relayout {
            self.ctx.element.relayout();
        } else {
            self.ctx.element.repaint();
        }

        // The glide timer, when a scroll command left the view short of its target.
        if self.ctx.shared.borrow().scroll.gliding()
            && let Some(start) = self.ctx.start_glide.borrow().as_ref()
        {
            start();
        }

        // The clipboard, and the paste that comes back through a command.
        if let Some((clipboard, text)) = response.copied
            && let Some(clipboards) = self.ctx.clipboards.as_ref()
        {
            clipboards.set_text(kind_of(clipboard), text);
        }
        if let Some(clipboard) = response.paste_requested
            && let Some(clipboards) = self.ctx.clipboards.as_ref()
        {
            let handle = self.clone();
            clipboards.read_text(kind_of(clipboard), move |text| {
                if let Some(text) = text
                    && !text.is_empty()
                {
                    handle.command(Command::InsertAt {
                        at: InsertPoint::AtCarets,
                        text,
                        linewise: false,
                    });
                }
            });
        }

        // The reports.
        let report = self.ctx.on_event.borrow();
        if let Some(tell) = report.as_ref() {
            if let Some(change) = response.change.as_ref() {
                tell(EditorEvent::Edited {
                    kind: change.kind,
                    revision: self.ctx.shared.borrow().state.buffer.revision(),
                });
            } else if response.selection_changed {
                tell(EditorEvent::SelectionMoved);
            }
            if matches!(response.scroll, Some(ScrollEffect::Command(_))) {
                tell(EditorEvent::Scrolled);
            }
        }
    }

    /// How far a caret-following scroll may move and still snap rather than glide, in lines.
    ///
    /// Typing at the viewport's edge moves the view a line at a time and must never lag; a
    /// `G`, a search hit or a half-page jump reads far better arriving as motion.
    const GLIDE_THRESHOLD_LINES: f64 = 4.0;

    /// Brings the primary caret into view: instantly for a nudge, gliding for a jump.
    fn ensure_caret_visible(&self, shared: &mut EditorShared) {
        let head = shared.state.selections.primary().head;
        let (line, x) = shared.caret_position(head);
        let total = position::line_count(shared.state.buffer.rope());
        let viewport = shared.viewport_lines();
        let scrolloff = shared.config.scrolloff;

        // Probe on a copy: where would the view have to sit? Then decide how to get there.
        let before = shared.scroll.pos.line;
        let mut probe = shared.scroll;
        if probe.ensure_visible(line, scrolloff, total, viewport) {
            let distance = (probe.pos.line - before).abs();
            if shared.config.smooth_scroll && distance > Self::GLIDE_THRESHOLD_LINES {
                shared.scroll.target_line = probe.pos.line;
            } else {
                shared.scroll = probe;
            }
        }

        let text_width = shared.text_area_width();
        shared.scroll.ensure_visible_x(
            f64::from(x),
            f64::from(text_width),
            f64::from(shared.metrics.cell_advance * 2.0),
        );
    }

    /// Aims the view at `line` as its top: gliding when the editor scrolls smoothly, snapping
    /// otherwise.
    fn aim(&self, shared: &mut EditorShared, line: f64, total: usize, viewport: f64) {
        if shared.config.smooth_scroll {
            shared.scroll.target_line =
                line.clamp(0.0, crate::scroll::ScrollState::max_top(total, viewport));
        } else {
            shared.scroll.scroll_to(line, total, viewport);
        }
    }

    /// Applies an explicit scroll command.
    fn apply_scroll(&self, shared: &mut EditorShared, command: ScrollCmd) {
        let total = position::line_count(shared.state.buffer.rope());
        let viewport = shared.viewport_lines();
        let caret_line = position::line_of(
            shared.state.buffer.rope(),
            shared.state.selections.primary().head,
        ) as f64;
        match command {
            ScrollCmd::Lines(lines) => shared.scroll.scroll_by(lines, total, viewport),
            ScrollCmd::Pages(pages) => shared.scroll.scroll_by(pages * viewport, total, viewport),
            ScrollCmd::ToLine(line) => shared.scroll.scroll_to(line as f64, total, viewport),
            ScrollCmd::CursorCenter => {
                self.aim(shared, caret_line - viewport / 2.0, total, viewport)
            }
            ScrollCmd::CursorTop => {
                let margin = shared.config.scrolloff as f64;
                self.aim(shared, caret_line - margin, total, viewport)
            }
            ScrollCmd::CursorBottom => {
                let margin = shared.config.scrolloff as f64;
                self.aim(
                    shared,
                    caret_line - viewport + 1.0 + margin,
                    total,
                    viewport,
                )
            }
            ScrollCmd::HorizontalPx(px) => {
                let text_width = f64::from(shared.text_area_width());
                let max = (f64::from(shared.horizontal_extent()) - text_width).max(0.0);
                shared.scroll.pos.x_px = (shared.scroll.pos.x_px + px).clamp(0.0, max);
            }
            ScrollCmd::EnsureCursorVisible => self.ensure_caret_visible(shared),
        }
    }

    /// Publishes the reactive reads.
    pub(crate) fn update_signals(&self, shared: &mut EditorShared) {
        let primary = shared.state.selections.primary();
        let rope = shared.state.buffer.rope().clone();
        let (line, col) = position::line_col(&rope, primary.head);
        self.ctx
            .signals
            .revision
            .set(shared.state.buffer.revision());
        self.ctx.signals.cursor.set(CursorPos { line, col });
        self.ctx.signals.selection.set(primary);
        let total = position::line_count(&rope);
        let viewport = shared.viewport_lines();
        self.ctx.signals.scroll.set(ScrollSnapshot {
            top_line: shared.scroll.pos.line,
            max_top: crate::scroll::ScrollState::max_top(total, viewport),
            viewport_lines: viewport,
        });
        self.ctx.signals.caret_rect.set(self.caret_rect_now(shared));

        // The worker follows the viewport, asked again only when the view has moved far
        // enough that the overscan it was given is running out.
        if let Some(tx) = shared.syntax_tx.as_ref() {
            let visible = shared.visible_lines();
            let requested = shared.requested_window.clone();
            let slack = crate::syntax::worker::OVERSCAN_LINES / 2;
            let moved = requested.is_empty()
                || visible.start.abs_diff(requested.start) > slack
                || visible.end.abs_diff(requested.end) > slack;
            if moved {
                let _ = tx.send(crate::syntax::worker::ToWorker::Window(visible.clone()));
                shared.requested_window = visible;
            }
        }
    }

    /// Where the primary caret sits on the window right now, when it is visible.
    fn caret_rect_now(&self, shared: &mut EditorShared) -> Option<CaretRect> {
        let head = shared.state.selections.primary().head;
        let (line, x) = shared.caret_position(head);
        if !shared.visible_lines().contains(&line) {
            return None;
        }
        let bounds = self.ctx.port.bounds()?;
        let scale = shared.metrics.scale.max(0.001);
        let element_x = shared.gutter_width() - shared.scroll.pos.x_px as f32 + x;
        let element_y = shared.line_y(line);
        Some(CaretRect {
            x: (bounds.origin.x.0 + element_x) / scale,
            y: (bounds.origin.y.0 + element_y) / scale,
            width: 2.0,
            height: shared.metrics.line_height / scale,
        })
    }
}

/// The framework's name for one of ours.
fn kind_of(clipboard: Clipboard) -> ClipboardKind {
    match clipboard {
        Clipboard::Standard => ClipboardKind::Standard,
        Clipboard::Primary => ClipboardKind::Primary,
    }
}
