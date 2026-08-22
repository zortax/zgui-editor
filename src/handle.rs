//! The application's side of a mounted editor.
//!
//! An [`EditorHandle`] is how everything outside the component drives it: commands go in
//! fire-and-forget, state comes out either synchronously through [`EditorHandle::query`] — which
//! is what makes a vim layer or a completion engine implementable outside the crate — or
//! reactively through signals, which is what a status line binds to.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use zgui::platform::ClipboardKind;
use zgui::prelude::*;
use zgui::runtime::clipboard::Clipboards;

use crate::command::{Clipboard, Command, InsertPoint, Motion, ScrollCmd};
use crate::config::{CursorStyle, GutterMode};
use crate::core::motion::{self, MotionContext};
use crate::core::search::SearchDirection;
use crate::core::selection::{Selection, Selections};
use crate::core::{ChangeInfo, EditorState, Response, ScrollEffect, position, search, words};
use crate::decoration::{Decoration, GutterMark};
use crate::document::{Document, ViewId};
use crate::event::EditorEvent;
use crate::overlay::Overlay;
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
    /// The line at the top of the view, fractionally, right now.
    ///
    /// Mid-glide this is where the view has reached, which is what a scrollbar draws and what
    /// anything writing the position down should *not* use: see [`Self::target_line`].
    pub top_line: f64,
    /// The line the view is heading for. Equal to [`Self::top_line`] at rest.
    ///
    /// What to write down. A jump glides, so the moment a scroll is reported the view has barely
    /// begun to move, and the position recorded then would be the one it was leaving.
    pub target_line: f64,
    /// The greatest top line the document allows.
    pub max_top: f64,
    /// How many lines the view shows.
    pub viewport_lines: f64,
    /// How far the text is scrolled left, in device pixels.
    ///
    /// What a session writes down beside the top line: a view put back at the right line but at
    /// the left margin is a view that moved.
    pub x_px: f64,
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
///
/// The document is what every view of the buffer shares; the selections are the queried view's
/// alone. A vim layer asking where its caret is asks *this* view, and gets its own answer even
/// when a second window is open on the same file.
pub struct EditorSnapshot<'a> {
    doc: &'a crate::core::DocumentState,
    selections: &'a Selections,
    context: MotionContext,
    visible: Range<usize>,
}

impl EditorSnapshot<'_> {
    /// The lines any part of which is on screen.
    ///
    /// What bounds work that is only worth doing for what can be seen: labelling leap targets,
    /// laying out inline diagnostics, highlighting search matches.
    pub fn visible_lines(&self) -> Range<usize> {
        self.visible.clone()
    }

    /// The bytes any part of which is on screen.
    ///
    /// Empty while the editor has not been laid out yet, which is also when there is nothing to
    /// see. Half-open, and clamped to the text — the caller can slice the rope with it directly.
    pub fn visible_byte_range(&self) -> Range<usize> {
        let rope = self.doc.buffer.rope();
        if self.visible.is_empty() {
            return 0..0;
        }
        let start = position::line_start(rope, self.visible.start);
        let last = self.visible.end.saturating_sub(1);
        let end = if last + 1 >= position::line_count(rope) {
            rope.len_bytes()
        } else {
            position::line_start(rope, last + 1)
        };
        start..end.max(start)
    }

    /// The text. Cloning the rope is O(1).
    pub fn rope(&self) -> &ropey::Rope {
        self.doc.buffer.rope()
    }

    /// Which revision of the text this is.
    pub fn revision(&self) -> u64 {
        self.doc.buffer.revision()
    }

    /// This view's selections, in document order.
    pub fn selections(&self) -> &Selections {
        self.selections
    }

    /// How many lines the text has.
    pub fn line_count(&self) -> usize {
        position::line_count(self.doc.buffer.rope())
    }

    /// The text in `range`, copied out.
    pub fn text_in(&self, range: Range<usize>) -> String {
        self.doc.buffer.rope().byte_slice(range).to_string()
    }

    /// The line and grapheme column of `byte`.
    pub fn line_col(&self, byte: usize) -> CursorPos {
        let (line, col) = position::line_col(self.doc.buffer.rope(), byte);
        CursorPos { line, col }
    }

    /// The word under `byte`, as a double click selects it.
    pub fn word_at(&self, byte: usize) -> Range<usize> {
        words::word_at(self.doc.buffer.rope(), byte)
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
            self.doc.buffer.rope(),
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
        search::find(self.doc.buffer.rope(), needle, from, direction, wrap)
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
    /// The text, shared with every other view of the same buffer.
    pub document: Document,
    /// Which view of that document this is, so a change can be told to the others and not to
    /// itself.
    pub view_id: Cell<ViewId>,
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

/// Two handles are equal when they drive the same editor, not when they say the same things.
///
/// Identity is what a caller keeping a registry of mounted editors needs. A view rebuilt in place
/// — a pane the layout has just re-created around the same window and the same buffer — registers
/// the new editor *before* the old one's cleanup runs, so a registry that forgets by key alone
/// deletes the live entry on the way out and is left holding nothing for a pane that is on the
/// screen. Asking "is the one I am holding the one that is going away?" is the whole of the fix,
/// and it needs the handle to be comparable.
impl PartialEq for EditorHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.ctx, &other.ctx)
    }
}

impl Eq for EditorHandle {}

impl EditorHandle {
    pub(crate) fn new(
        document: Document,
        shared: Rc<RefCell<EditorShared>>,
        element: zgui::custom::CustomHandle<EditorElement>,
        port: NodeRef,
        clipboards: Option<Clipboards>,
        on_event: Option<Box<dyn Fn(EditorEvent)>>,
        registry: LanguageRegistry,
    ) -> Self {
        Self {
            ctx: Rc::new(EditorCtx {
                document,
                view_id: Cell::new(ViewId::UNATTACHED),
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
        let doc = self.ctx.document.state();
        let snapshot = EditorSnapshot {
            doc: &doc,
            selections: &shared.selections,
            context: shared.motion_context(),
            visible: shared.visible_lines(),
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
            let mut doc = self.ctx.document.state_mut();
            EditorState {
                doc: &mut doc,
                selections: &mut shared.selections,
            }
            .set_text(text)
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
            let (rope, revision) = shared.document.state().snapshot();
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
                if frame.revision != shared.revision() {
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

    /// Replaces the decorations in the layer called `layer`.
    ///
    /// Layers are independent: a language server replacing its diagnostics leaves the search
    /// highlight alone, and neither has to know the other exists. An empty list clears the layer
    /// without forgetting where it sits in the painting order, so the next set of diagnostics
    /// paints under the same things the last set did.
    ///
    /// Setting a layer to what it already holds costs nothing, which is what makes it reasonable
    /// to re-set the search highlight on every keystroke of an incremental search.
    pub fn set_decorations(&self, layer: &str, decorations: Vec<Decoration>) {
        let (changed, needs_style) = {
            let mut shared = self.ctx.shared.borrow_mut();
            let changed = shared.decorations.set(layer, decorations);
            (changed, changed && shared.decorations_need_style())
        };
        self.after_marks(changed, needs_style);
    }

    /// Removes the layer called `layer` and everything in it.
    pub fn clear_decorations(&self, layer: &str) {
        let changed = self.ctx.shared.borrow_mut().decorations.clear(layer);
        self.after_marks(changed, false);
    }

    /// Replaces the gutter marks in the layer called `layer`.
    ///
    /// The same layering as [`set_decorations`](Self::set_decorations), in the gutter's own
    /// padding: a mark never moves a line number and never widens the gutter.
    pub fn set_gutter_marks(&self, layer: &str, marks: Vec<GutterMark>) {
        let (changed, needs_style) = {
            let mut shared = self.ctx.shared.borrow_mut();
            let changed = shared.gutter_marks.set(layer, marks);
            (changed, changed && shared.decorations_need_style())
        };
        self.after_marks(changed, needs_style);
    }

    /// Removes the gutter-mark layer called `layer`.
    pub fn clear_gutter_marks(&self, layer: &str) {
        let changed = self.ctx.shared.borrow_mut().gutter_marks.clear(layer);
        self.after_marks(changed, false);
    }

    /// Asks for whatever a changed set of marks needs.
    ///
    /// A mark naming a colour the last layout never read has to wait for the style to be read
    /// again; anything else is a repaint, which is what an incremental search wants a keystroke
    /// to cost.
    fn after_marks(&self, changed: bool, needs_style: bool) {
        if !changed {
            return;
        }
        if needs_style {
            self.ctx.element.relayout();
        } else {
            self.ctx.element.repaint();
        }
    }

    /// Replaces the bands and the carets the application paints itself.
    ///
    /// What a modal layer's visual modes are drawn with; see [`overlay`](crate::overlay). An empty
    /// overlay hands the painting back to the selections.
    pub fn set_overlay(&self, overlay: Overlay) {
        {
            let mut shared = self.ctx.shared.borrow_mut();
            if shared.overlay == overlay {
                return;
            }
            shared.overlay = overlay;
            shared.blink_on = true;
            // The overlay moves the caret, and a status line reads where the caret is.
            self.update_signals(&mut shared);
        }
        self.ctx.element.repaint();
    }

    /// Changes what the caret looks like — a vim layer's mode change.
    pub fn set_cursor_style(&self, style: CursorStyle) {
        self.ctx.shared.borrow_mut().config.cursor_style = style;
        self.ctx.element.repaint();
    }

    /// Says whether this is the view a person is working in.
    ///
    /// What the caret's own line band follows. Separate from focus: an editor under a picker or a
    /// command line is still the view being worked in, and keeps its band. An application showing
    /// one editor never has to call this.
    pub fn set_active(&self, active: bool) {
        {
            let mut shared = self.ctx.shared.borrow_mut();
            if shared.active == active {
                return;
            }
            shared.active = active;
        }
        self.ctx.element.repaint();
    }

    /// Changes how the gutter numbers its lines.
    pub fn set_gutter(&self, mode: GutterMode) {
        self.ctx.shared.borrow_mut().config.gutter = mode;
        self.ctx.element.relayout();
    }

    /// Says which lines the view draws, or that it draws all of them.
    ///
    /// The window moves as the caret moves between blocks of a rendered document, so it is set
    /// here rather than only at build time: an editor unmounted to change which lines it shows
    /// would lose its carets, its history and its parsed tree every time somebody pressed a key.
    ///
    /// The view relays out, because a window is what decides how tall it is.
    pub fn set_line_window(&self, window: Option<std::ops::Range<usize>>) {
        {
            let mut shared = self.ctx.shared.borrow_mut();
            if shared.config.line_window == window {
                return;
            }
            shared.config.line_window = window;
        }
        self.ctx.element.relayout();
    }

    /// Which lines the view draws, when it draws only some of them.
    #[must_use]
    pub fn line_window(&self) -> Option<std::ops::Range<usize>> {
        self.ctx.shared.borrow().config.line_window.clone()
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

    /// Where `byte` sits on the window, in CSS pixels. `None` when its line is not on screen.
    ///
    /// The one call that turns a position in the text into a place on the screen, which is what
    /// anything drawn *at* a piece of code needs: a hover card over a symbol, a diagnostic beside
    /// the token it is about, the label a leap motion puts on each candidate. The rectangle is a
    /// caret's — as wide as the caret, as tall as the line — so a surface anchored to it lines up
    /// with the text rather than with the element.
    ///
    /// Geometry is the completed frame's, so this answers about where things were drawn last, not
    /// where a command issued in this same turn will put them.
    pub fn point_for_byte(&self, byte: usize) -> Option<CaretRect> {
        let mut shared = self.ctx.shared.borrow_mut();
        self.point_in(&mut shared, byte)
    }

    /// Where `byte` is *inside the editor's own box*, in CSS pixels. `None` when it is not on
    /// screen.
    ///
    /// The same place as [`point_for_byte`](Self::point_for_byte), measured from the element
    /// rather than from the window. What an overlay drawn inside the editor wants: a label placed
    /// with `position: absolute` in a box that fills the editor is positioned against the
    /// element's padding box, so a window coordinate would be out by wherever the editor happens
    /// to sit — which is exactly right at the origin and wrong everywhere else.
    #[must_use]
    pub fn local_point_for_byte(&self, byte: usize) -> Option<CaretRect> {
        let mut shared = self.ctx.shared.borrow_mut();
        let (line, x) = shared.caret_position(byte);
        if !shared.visible_lines().contains(&line) {
            return None;
        }
        let scale = shared.metrics.scale.max(0.001);
        Some(CaretRect {
            x: (shared.gutter_width() - shared.scroll.pos.x_px as f32 + x) / scale,
            y: shared.line_y(line) / scale,
            width: 2.0,
            height: shared.metrics.line_height / scale,
        })
    }

    /// Which byte a place on the window lands on, in CSS pixels. `None` when it is outside.
    ///
    /// The inverse of [`point_for_byte`](Self::point_for_byte), and what a click in a decoration
    /// an application drew over the editor has to be turned back into.
    pub fn byte_for_point(&self, x: f32, y: f32) -> Option<usize> {
        let bounds = self.ctx.port.window_bounds()?;
        let mut shared = self.ctx.shared.borrow_mut();
        let scale = shared.metrics.scale.max(0.001);
        let local_x = x * scale - bounds.origin.x.0;
        let local_y = y * scale - bounds.origin.y.0;
        if local_x < 0.0
            || local_y < 0.0
            || local_x > shared.viewport.0
            || local_y > shared.viewport.1
        {
            return None;
        }
        Some(shared.hit_test(local_x, local_y))
    }

    /// Scans the whole buffer for its longest line on a worker, so the horizontal scroll
    /// extent is right without the UI thread ever walking millions of lines.
    ///
    /// The answer is an estimate — characters times the cell advance — refined monotonically by
    /// the real shaped widths of whatever becomes visible.
    pub(crate) fn scan_max_width(&self) {
        let (rope, revision) = {
            let shared = self.ctx.shared.borrow();
            shared.document.state().snapshot()
        };
        let work = zgui::task::blocking(move || {
            rope.lines().map(|line| line.len_chars()).max().unwrap_or(0)
        });
        let handle = self.clone();
        let task = zgui::task::spawn_local(async move {
            let chars = work.await;
            let mut shared = handle.ctx.shared.borrow_mut();
            if shared.revision() != revision {
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

    /// Registers this view with its document.
    pub(crate) fn attach_to_document(&self) {
        let id = self.ctx.document.attach(&self.ctx);
        self.ctx.view_id.set(id);
    }

    /// Takes it off again, which the component does from its own cleanup.
    pub(crate) fn detach_from_document(&self) {
        self.ctx.document.detach(self.ctx.view_id.get());
    }

    /// The handle for a context somebody else is holding.
    pub(crate) fn from_ctx(ctx: Rc<EditorCtx>) -> Self {
        Self { ctx }
    }

    /// Takes a change another view of the same document made.
    ///
    /// This view's carets move through the replacements, its caches drop what the change touched,
    /// and its highlighter is told. Nothing is reported to the application: a second window onto
    /// a file is a second view of one change, not a second change.
    pub(crate) fn follow(&self, change: &ChangeInfo) {
        {
            let mut shared = self.ctx.shared.borrow_mut();
            if change.whole_text {
                // A replaced text has no positions to carry forward.
                shared.selections = Selections::caret(0);
            } else {
                let rope = shared.rope();
                let changes = Arc::clone(&change.changes);
                shared.selections.map(|selection| Selection {
                    anchor: position::snap(
                        &rope,
                        crate::core::edit::map_through(&changes, selection.anchor),
                    ),
                    head: position::snap(
                        &rope,
                        crate::core::edit::map_through(&changes, selection.head),
                    ),
                    affinity: selection.affinity,
                    goal_col: None,
                });
            }
            shared.absorb_change(change);
            self.update_signals(&mut shared);
        }
        self.ctx.element.relayout();
    }

    fn dispatch(&self, command: &Command) {
        let response = {
            let mut shared = self.ctx.shared.borrow_mut();
            let context = shared.motion_context();
            let mut doc = self.ctx.document.state_mut();
            EditorState {
                doc: &mut doc,
                selections: &mut shared.selections,
            }
            .apply(command, context)
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
                shared.absorb_change(change);
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

        // The other views of this document, which have to follow the change before the frame
        // this settle is part of is drawn.
        if let Some(change) = response.change.as_ref() {
            self.ctx.document.broadcast(change, self.ctx.view_id.get());
        }

        // The reports.
        let report = self.ctx.on_event.borrow();
        if let Some(tell) = report.as_ref() {
            if let Some(change) = response.change.as_ref() {
                tell(EditorEvent::Edited {
                    kind: change.kind,
                    revision: self.ctx.shared.borrow().revision(),
                    changes: Arc::clone(&change.changes),
                });
            } else if response.selection_changed {
                tell(EditorEvent::SelectionMoved);
            }
            if matches!(response.scroll, Some(ScrollEffect::Command(_))) {
                tell(EditorEvent::Scrolled);
            }
        }
    }

    /// Brings the primary caret into view: instantly for a nudge, gliding for a jump.
    fn ensure_caret_visible(&self, shared: &mut EditorShared) {
        // The overlay's caret when it places one: in a block selection that is the only thing that
        // knows which line the person is steering, and how far past its end.
        let (line, x) = match shared.overlay.carets.first().copied() {
            Some(caret) => (caret.line, shared.cell_x(caret.line, caret.column)),
            None => shared.caret_position(shared.selections.primary().head),
        };
        let total = shared.line_count();
        let viewport = shared.viewport_lines();
        let scrolloff = shared.config.scrolloff;

        // Probe on a copy: where would the view have to sit? Then decide how to get there.
        let before = shared.scroll.pos.line;
        let mut probe = shared.scroll;
        if probe.ensure_visible(line, scrolloff, total, viewport) {
            let distance = (probe.pos.line - before).abs();
            if shared.config.smooth_scroll && distance > shared.config.glide_threshold_lines {
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
        let total = shared.line_count();
        let viewport = shared.viewport_lines();
        let caret_line = position::line_of(&shared.rope(), shared.selections.primary().head) as f64;
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
            ScrollCmd::ToExact { line, x_px } => {
                // Both at once, and never a `ToLine` followed by a `HorizontalPx`: two commands
                // are two frames of the wrong picture, and `ToLine` cannot say "half-way down
                // line two" in the first place.
                shared.scroll.scroll_to(line, total, viewport);
                let text_width = f64::from(shared.text_area_width());
                let max = (f64::from(shared.horizontal_extent()) - text_width).max(0.0);
                shared.scroll.pos.x_px = x_px.clamp(0.0, max);
            }
            ScrollCmd::EnsureCursorVisible => self.ensure_caret_visible(shared),
        }
    }

    /// Publishes the reactive reads.
    pub(crate) fn update_signals(&self, shared: &mut EditorShared) {
        let primary = shared.selections.primary();
        let rope = shared.rope();
        // Where the caret is drawn is where a status line must say it is.
        let (line, col) = match shared.overlay.carets.first() {
            Some(caret) => (caret.line, caret.column as usize),
            None => position::line_col(&rope, primary.head),
        };
        self.ctx.signals.revision.set(shared.revision());
        self.ctx.signals.cursor.set(CursorPos { line, col });
        self.ctx.signals.selection.set(primary);
        let total = position::line_count(&rope);
        let viewport = shared.viewport_lines();
        self.ctx.signals.scroll.set(ScrollSnapshot {
            top_line: shared.scroll.pos.line,
            target_line: shared.scroll.target_line,
            max_top: crate::scroll::ScrollState::max_top(total, viewport),
            viewport_lines: viewport,
            x_px: shared.scroll.pos.x_px,
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
        let head = shared.selections.primary().head;
        self.point_in(shared, head)
    }

    /// Where `byte` sits on the window, given a view already borrowed.
    fn point_in(&self, shared: &mut EditorShared, byte: usize) -> Option<CaretRect> {
        let (line, x) = shared.caret_position(byte);
        if !shared.visible_lines().contains(&line) {
            return None;
        }
        // The window's space, not the parent's: `bounds` stops at the parent's border box, so an
        // editor inside anything at all would answer a place that is short by however far its
        // ancestors are down and across the window.
        let bounds = self.ctx.port.window_bounds()?;
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
