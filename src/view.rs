//! The component an application mounts.
//!
//! Everything here is wiring: the model, the caches and the painting live in
//! [`EditorShared`](crate::render::shared::EditorShared); the handlers below turn framework
//! events into [`Command`]s and say what changed through the element handle. The one custom
//! element is sized by CSS and paints only the visible lines, which is what keeps a
//! ten-million-line buffer costing what the viewport costs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use zgui::prelude::*;
use zgui::reactive::RenderEffect;

use crate::command::{Clipboard, Command, InsertPoint};
use crate::config::EditorConfig;
use crate::core::position;
use crate::core::selection::Selection;
use crate::event::{EditorEvent, KeyFilter};
use crate::handle::EditorHandle;
use crate::input::keymap::default_keymap;
use crate::input::mouse::{ClickCounter, ClickKind};
use crate::render::element::EditorElement;
use crate::render::shared::EditorShared;

zgui::style! { pub EditorStyle =>
    ":scope {
        display: flex;
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        cursor: text;
    }"
    ":scope > custom {
        flex: 1;
        min-width: 0;
        min-height: 0;
    }"
}

/// The stylesheet's registered name.
const SHEET: &str = "zgui-editor";

/// How often the caret blinks, each way.
const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// A code editor over `text`.
///
/// The component fills the space its container gives it (`flex: 1`), draws entirely through its
/// own element, and is themed through CSS custom properties — see the crate documentation for
/// the list. Everything an application does to it goes through the [`EditorHandle`] delivered
/// by `on_ready` (and through local context); everything modal — a vim layer — is built on
/// `on_key`, which hears every key first.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn Editor(
    /// What the buffer starts as. Ignored when `document` is given, which brings its own text.
    #[prop(optional, into)]
    text: Option<String>,
    /// The buffer to show, when it is one another editor is showing too.
    ///
    /// Two editors given the same document are two windows onto one file: an edit in either is
    /// the edit, either can undo it, and each keeps its own carets and its own scroll position.
    /// Left out, the editor makes a document of its own over `text`.
    #[prop(optional)]
    document: Option<crate::document::Document>,
    /// The behaviour.
    #[prop(optional)]
    config: Option<EditorConfig>,
    /// Called once with the handle, so the application can drive the editor.
    #[prop(optional)]
    on_ready: Option<Box<dyn Fn(EditorHandle)>>,
    /// Everything the editor reports.
    #[prop(optional)]
    on_event: Option<Box<dyn Fn(EditorEvent)>>,
    /// A key before the editor interprets it. Answer `true` to consume it.
    #[prop(optional)]
    on_key: Option<KeyFilter>,
    /// The language to highlight as, by its name in the registry.
    #[prop(optional, into)]
    language: Option<String>,
    /// The languages this editor can highlight. The bundled set when not given.
    #[prop(optional)]
    registry: Option<crate::syntax::registry::LanguageRegistry>,
    /// Whether the editor takes focus as soon as it is mounted.
    #[prop(default = true)]
    autofocus: bool,
    /// Whether the editor can hold the keyboard at all.
    ///
    /// `false` makes a view that is read and pointed at while the keys stay with the element
    /// around it: a click sets the caret and reports it, the wheel scrolls, and every key passes
    /// by untouched.
    #[prop(default = true)]
    focusable: bool,
    /// Classes the caller put on the editor.
    #[prop(into, optional)]
    class: Classes,
    /// Anything else the caller forwarded.
    #[prop(attrs)]
    attrs: Attrs,
) -> impl IntoView {
    install_stylesheet(SHEET, EditorStyle::CSS);
    let config = config.unwrap_or_default();

    let fonts = use_context::<zgui::app::Fonts>();
    let shaper = fonts.as_ref().map(|fonts| fonts.shaper());
    let document =
        document.unwrap_or_else(|| crate::document::Document::new(text.as_deref().unwrap_or("")));
    let shared = Rc::new(RefCell::new(EditorShared::new(
        document.clone(),
        config.clone(),
        shaper,
    )));

    let (element_view, element_handle) =
        zgui::custom::custom(EditorElement::new(Rc::clone(&shared)));
    let port = NodeRef::new();
    let clipboards = try_use_clipboard();

    let registry =
        registry.unwrap_or_else(|| crate::syntax::registry::LanguageRegistry::new().with_bundled());
    let handle = EditorHandle::new(
        document.clone(),
        Rc::clone(&shared),
        element_handle.clone(),
        port,
        clipboards,
        on_event,
        registry,
    );

    // This view registers with its document, so an edit made in another window onto the same
    // file reaches it. The registration is weak on the document's side and given back here, so a
    // view that goes away neither keeps its document alive nor is told about it again.
    handle.attach_to_document();
    {
        let handle = handle.clone();
        on_cleanup_local(move || handle.detach_from_document());
    }

    // The syntax worker: its own thread, told about edits and the viewport, answering with
    // highlighted lines that are dropped when they describe text that has already changed.
    {
        let (frames_tx, frames_rx) = flume::unbounded();
        shared.borrow_mut().syntax_tx = Some(crate::syntax::worker::spawn(frames_tx));
        let pump = zgui::task::spawn_local({
            let handle = handle.clone();
            async move {
                while let Ok(message) = frames_rx.recv_async().await {
                    handle.apply_highlight(message);
                }
            }
        });
        on_cleanup_local(move || pump.cancel());
    }
    if let Some(language) = language.as_deref() {
        handle.set_language(Some(language));
    }

    // What layout read off the style sheet. Reported into a signal so the change becomes a
    // repaint on the frame after — the element may not invalidate itself from inside its own
    // layout.
    let styled = RwSignal::new_local(None::<crate::render::shared::StyleReport>);
    shared.borrow_mut().on_style = Some(Rc::new(move |report| styled.set(Some(report))));
    let restyle = RenderEffect::new({
        let handle = handle.clone();
        move |_| {
            if styled.get().is_some() {
                let mut shared = handle.ctx.shared.borrow_mut();
                // The viewport may have changed; keep the view inside the document.
                let total = shared.line_count();
                let viewport = shared.viewport_lines();
                let top = shared.scroll.pos.line;
                shared.scroll.scroll_to(top, total, viewport);
                handle.update_signals(&mut shared);
                drop(shared);
                handle.ctx.element.repaint();
            }
        }
    });
    on_cleanup_local(move || drop(restyle));

    // The clock, taken once here rather than looked up wherever a timer is wanted.
    //
    // `set_timeout` and `set_interval` find the window's clock through the reactive context, and a
    // listener runs under the owner of the node it is attached to. An event delivered to a view
    // that is on its way out — a focus announcement reaching the pane a split has just replaced —
    // therefore runs under an owner that is already disposed, where the lookup finds nothing and
    // the free function panics rather than declining. Held here, the clock is the one this view
    // was built with for as long as the view exists anywhere, and a view built outside a window
    // simply has none.
    let clock = zgui::view::time::Timers::current();

    // The scroll glide: a frame callback re-armed only while the view is short of its target.
    //
    // A frame callback rather than an interval, because the two are paced by different things. A
    // pending frame callback makes the window count as animating, so its frames come at the
    // display's own refresh interval — one step per frame the output can show, whether that is
    // sixty or two hundred and forty a second. An interval is wall-clock and knows nothing of the
    // output: any period written here is wrong on most displays, and a step timer beating against
    // the vsync grid draws a motion made of uneven steps.

    /// Everything one glide step needs, cloneable into the next frame's callback.
    #[derive(Clone)]
    struct Glide {
        /// The editor state the step advances.
        shared: Rc<RefCell<EditorShared>>,
        /// The handle the step publishes through.
        handle: EditorHandle,
        /// Where the pending registration is held, which is also the double-start guard.
        slot: Rc<RefCell<Option<zgui::view::time::FrameHandle>>>,
        /// The moment the previous step was for, so dt is measured between frame moments.
        last: Rc<Cell<Option<zgui::view::Timestamp>>>,
        /// The clock the view was built with. `None` outside a window, where nothing animates.
        clock: Option<zgui::view::time::Timers>,
    }

    /// Registers the next step.
    fn arm(glide: &Glide) -> Option<zgui::view::time::FrameHandle> {
        let step = glide.clone();
        glide
            .clock
            .as_ref()
            .map(|clock| clock.request_frame(move |at| tick(&step, at)))
    }

    /// One step: dt from successive frame moments, never from the wall clock.
    ///
    /// The moment handed in is the one the frame is *for*, the same one the framework's own
    /// animations are sampled against, so a motion measured between them is drawn as even steps.
    fn tick(glide: &Glide, at: zgui::view::Timestamp) {
        let dt = glide
            .last
            .get()
            .map_or(0.001, |last| at.saturating_since(last).as_secs_f64());
        glide.last.set(Some(at));
        let running = {
            let mut shared = glide.shared.borrow_mut();
            let running = shared.scroll.step(dt.max(0.001));
            glide.handle.update_signals(&mut shared);
            running
        };
        glide.handle.ctx.element.repaint();
        // A finished glide stops by not registering again. The handle this replaces or drops has
        // already run, and cancelling a spent registration does nothing.
        *glide.slot.borrow_mut() = if running { arm(glide) } else { None };
    }

    let glide: Rc<RefCell<Option<zgui::view::time::FrameHandle>>> = Rc::new(RefCell::new(None));
    let glide_last: Rc<Cell<Option<zgui::view::Timestamp>>> = Rc::new(Cell::new(None));
    let start_glide: Box<dyn Fn()> = {
        let glide = Glide {
            shared: Rc::clone(&shared),
            handle: handle.clone(),
            slot: Rc::clone(&glide),
            last: Rc::clone(&glide_last),
            clock: clock.clone(),
        };
        Box::new(move || {
            if glide.slot.borrow().is_some() {
                return;
            }
            // The first step has no previous frame moment; it takes the floor and starts
            // measuring from the frame that runs it.
            glide.last.set(None);
            *glide.slot.borrow_mut() = arm(&glide);
        })
    };
    *handle.ctx.start_glide.borrow_mut() = Some(start_glide);

    // The caret's blink, running only while the editor has focus.
    let blink: Rc<RefCell<Option<zgui::view::time::IntervalHandle>>> = Rc::new(RefCell::new(None));

    // ---- Keyboard ----------------------------------------------------------------------

    let key_down = {
        let handle = handle.clone();
        let on_key = on_key;
        move |cx: &mut EventCx<'_, events::KeyDown>| {
            if !focusable {
                return;
            }
            let event: zgui::vocab::KeyEvent = (*cx).clone();
            let modifiers = cx.modifiers;
            if let Some(filter) = on_key.as_ref()
                && filter(&event, modifiers, &handle)
            {
                cx.prevent_default();
                cx.stop_propagation();
                return;
            }
            if let Some(command) = default_keymap(&event, modifiers) {
                handle.command(command);
                cx.prevent_default();
                cx.stop_propagation();
            }
        }
    };

    // ---- Pointer -----------------------------------------------------------------------

    /// What the pointer is holding, which decides what moving it does.
    enum Drag {
        /// A selection, at the granularity the click chose.
        Select {
            granularity: ClickKind,
            origin: std::ops::Range<usize>,
        },
        /// The scrollbar thumb, grabbed `grab` pixels below its own top.
        Scrollbar { grab: f64 },
    }

    let clicks = Rc::new(RefCell::new(ClickCounter::new()));
    let drag: Rc<RefCell<Option<Drag>>> = Rc::new(RefCell::new(None));
    // Whether the pointer sits on a scrollbar, which turns the text cursor into the arrow.
    let over_scrollbar = RwSignal::new_local(false);
    // Where the pointer last was, for the autoscroll that keeps a drag selecting off-screen.
    let drag_point = Rc::new(Cell::new((0.0f32, 0.0f32)));
    let autoscroll: Rc<RefCell<Option<zgui::view::time::IntervalHandle>>> =
        Rc::new(RefCell::new(None));
    let autoscroll_last = Rc::new(Cell::new(Instant::now()));

    // What dragging to `byte` selects, given how the drag began.
    let drag_selection = {
        let handle = handle.clone();
        Rc::new(
            move |byte: usize, granularity: ClickKind, origin: &std::ops::Range<usize>| {
                match granularity {
                    ClickKind::Single => Selection::new(origin.start, byte),
                    ClickKind::Double => {
                        let word = handle.query(|snapshot| snapshot.word_at(byte));
                        if word.start < origin.start {
                            Selection::new(origin.end, word.start)
                        } else {
                            Selection::new(origin.start, word.end.max(origin.end))
                        }
                    }
                    ClickKind::Triple => {
                        let lines = handle.query(|snapshot| {
                            let rope = snapshot.rope();
                            let line = position::line_of(rope, byte);
                            let start = position::line_start(rope, line);
                            let end = if line + 1 >= position::line_count(rope) {
                                rope.len_bytes()
                            } else {
                                position::line_start(rope, line + 1)
                            };
                            start..end
                        });
                        if lines.start < origin.start {
                            Selection::new(origin.end, lines.start)
                        } else {
                            Selection::new(origin.start, lines.end.max(origin.end))
                        }
                    }
                }
            },
        )
    };

    // Where a pointer event landed, in element-local device pixels.
    //
    // Against the *window* box. `bounds` is relative to the parent, so it is the element's offset
    // inside whatever holds it — which is exactly right for an editor that fills the window and
    // wrong by the height of a header for one that does not. A caret that lands a line or two
    // from the pointer is this.
    let local_point = move |position: zgui::geom::Point<zgui::geom::CssPx, zgui::geom::Css>| {
        let bounds = port.window_bounds()?;
        let scale = port.scale();
        Some((
            position.x.0 * scale - bounds.origin.x.0,
            position.y.0 * scale - bounds.origin.y.0,
        ))
    };

    let pointer_down = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let clicks = Rc::clone(&clicks);
        let drag = Rc::clone(&drag);
        move |cx: &mut EventCx<'_, events::PointerDown>| {
            let Some((x, y)) = local_point(cx.position) else {
                return;
            };
            if focusable {
                cx.request_focus(cx.current);
                port.focus();
            }

            // The scrollbar, before the text: it sits on top of it.
            {
                let mut shared = shared.borrow_mut();
                let bar = shared.scrollbar();
                if shared.config.scrollbar
                    && x >= shared.viewport.0 - shared.scrollbar_width()
                    && let Some((top, height)) = bar.thumb(shared.scroll.pos.line)
                {
                    let grab = if (f64::from(y) >= top) && (f64::from(y) < top + height) {
                        f64::from(y) - top
                    } else {
                        // A track press jumps the thumb under the pointer, then drags it.
                        height / 2.0
                    };
                    let line = bar.line_at(f64::from(y) - grab);
                    let total = shared.line_count();
                    let viewport = shared.viewport_lines();
                    shared.scroll.scroll_to(line, total, viewport);
                    handle.update_signals(&mut shared);
                    drop(shared);
                    *drag.borrow_mut() = Some(Drag::Scrollbar { grab });
                    over_scrollbar.set(true);
                    handle.ctx.element.repaint();
                    cx.capture_pointer();
                    // The bar took the press. An ancestor that reads presses as presses on the
                    // text would otherwise act on a drag of the thumb.
                    cx.stop_propagation();
                    return;
                }
            }

            match cx.button {
                Some(PointerButton::Secondary) => {
                    let byte = shared.borrow_mut().hit_test(x, y);
                    if let Some(tell) = handle.ctx.on_event.borrow().as_ref() {
                        tell(EditorEvent::ContextMenu { byte });
                    }
                    return;
                }
                Some(PointerButton::Middle) => {
                    let byte = shared.borrow_mut().hit_test(x, y);
                    handle.command(Command::SetSelections {
                        selections: vec![Selection::caret(byte)],
                        primary: 0,
                    });
                    if let Some(clipboards) = handle.ctx.clipboards.as_ref() {
                        let handle = handle.clone();
                        clipboards.read_text(ClipboardKind::Primary, move |text| {
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
                    return;
                }
                _ => {}
            }

            let byte = shared.borrow_mut().hit_test(x, y);
            let kind = clicks
                .borrow_mut()
                .click((f64::from(x), f64::from(y)), Instant::now());
            match kind {
                ClickKind::Single => {
                    if cx.modifiers.shift() {
                        let anchor =
                            handle.query(|snapshot| snapshot.selections().primary().anchor);
                        handle.command(Command::SetSelections {
                            selections: vec![Selection::new(anchor, byte)],
                            primary: 0,
                        });
                        *drag.borrow_mut() = Some(Drag::Select {
                            granularity: ClickKind::Single,
                            origin: anchor..anchor,
                        });
                    } else {
                        handle.command(Command::SetSelections {
                            selections: vec![Selection::caret(byte)],
                            primary: 0,
                        });
                        *drag.borrow_mut() = Some(Drag::Select {
                            granularity: ClickKind::Single,
                            origin: byte..byte,
                        });
                    }
                }
                ClickKind::Double => {
                    let word = handle.query(|snapshot| snapshot.word_at(byte));
                    handle.command(Command::SetSelections {
                        selections: vec![Selection::new(word.start, word.end)],
                        primary: 0,
                    });
                    *drag.borrow_mut() = Some(Drag::Select {
                        granularity: ClickKind::Double,
                        origin: word,
                    });
                }
                ClickKind::Triple => {
                    let lines = handle.query(|snapshot| {
                        let rope = snapshot.rope();
                        let line = position::line_of(rope, byte);
                        let start = position::line_start(rope, line);
                        let end = if line + 1 >= position::line_count(rope) {
                            rope.len_bytes()
                        } else {
                            position::line_start(rope, line + 1)
                        };
                        start..end
                    });
                    handle.command(Command::SetSelections {
                        selections: vec![Selection::new(lines.start, lines.end)],
                        primary: 0,
                    });
                    *drag.borrow_mut() = Some(Drag::Select {
                        granularity: ClickKind::Triple,
                        origin: lines,
                    });
                }
            }
            cx.capture_pointer();
        }
    };

    let pointer_move = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let drag = Rc::clone(&drag);
        let drag_point = Rc::clone(&drag_point);
        let drag_selection = Rc::clone(&drag_selection);
        let autoscroll = Rc::clone(&autoscroll);
        let autoscroll_last = Rc::clone(&autoscroll_last);
        let clock = clock.clone();
        move |cx: &mut EventCx<'_, events::PointerMove>| {
            let Some((x, y)) = local_point(cx.position) else {
                return;
            };
            drag_point.set((x, y));
            let holding = drag.borrow();
            // The arrow over the bars, the I-beam over the text — and the arrow held through
            // a thumb drag wherever the pointer strays meanwhile.
            let over = matches!(holding.as_ref(), Some(Drag::Scrollbar { .. }))
                || (holding.is_none() && shared.borrow().over_scrollbar(x, y));
            if over_scrollbar.get_untracked() != over {
                over_scrollbar.set(over);
            }
            match holding.as_ref() {
                None => {}
                Some(Drag::Scrollbar { grab }) => {
                    let grab = *grab;
                    drop(holding);
                    let mut shared = shared.borrow_mut();
                    let bar = shared.scrollbar();
                    let line = bar.line_at(f64::from(y) - grab);
                    let total = shared.line_count();
                    let viewport = shared.viewport_lines();
                    shared.scroll.scroll_to(line, total, viewport);
                    handle.update_signals(&mut shared);
                    drop(shared);
                    handle.ctx.element.repaint();
                }
                Some(Drag::Select {
                    granularity,
                    origin,
                }) => {
                    let granularity = *granularity;
                    let origin = origin.clone();
                    drop(holding);
                    let byte = shared.borrow_mut().hit_test(x, y);
                    let selection = drag_selection(byte, granularity, &origin);
                    handle.command(Command::SetSelections {
                        selections: vec![selection],
                        primary: 0,
                    });

                    // A pointer dragged past the edge keeps selecting: an interval scrolls the
                    // view toward it and re-extends until it comes back or lets go.
                    let outside = y < 0.0 || y > shared.borrow().viewport.1;
                    if outside && autoscroll.borrow().is_none() {
                        autoscroll_last.set(Instant::now());
                        let tick = {
                            let shared = Rc::clone(&shared);
                            let handle = handle.clone();
                            let drag = Rc::clone(&drag);
                            let drag_point = Rc::clone(&drag_point);
                            let drag_selection = Rc::clone(&drag_selection);
                            let autoscroll_last = Rc::clone(&autoscroll_last);
                            move || {
                                let now = Instant::now();
                                let dt = now.duration_since(autoscroll_last.get()).as_secs_f64();
                                autoscroll_last.set(now);
                                let (x, y) = drag_point.get();
                                let (granularity, origin) = match drag.borrow().as_ref() {
                                    Some(Drag::Select {
                                        granularity,
                                        origin,
                                    }) => (*granularity, origin.clone()),
                                    _ => return,
                                };
                                let byte = {
                                    let mut shared = shared.borrow_mut();
                                    let height = shared.viewport.1;
                                    let overshoot = if y < 0.0 {
                                        f64::from(y)
                                    } else if y > height {
                                        f64::from(y - height)
                                    } else {
                                        return;
                                    };
                                    let rate = crate::scroll::autoscroll_rate(
                                        overshoot.abs(),
                                        f64::from(shared.metrics.line_height),
                                    ) * overshoot.signum();
                                    let total = shared.line_count();
                                    let viewport = shared.viewport_lines();
                                    let target = shared.scroll.pos.line + rate * dt;
                                    shared.scroll.scroll_to(target, total, viewport);
                                    handle.update_signals(&mut shared);
                                    shared.hit_test(x, y.clamp(0.0, height))
                                };
                                handle.command(Command::SetSelections {
                                    selections: vec![drag_selection(byte, granularity, &origin)],
                                    primary: 0,
                                });
                            }
                        };
                        *autoscroll.borrow_mut() = clock
                            .as_ref()
                            .map(|clock| clock.set_interval(Duration::from_millis(30), tick));
                    } else if !outside {
                        autoscroll.borrow_mut().take();
                    }
                }
            }
        }
    };

    let pointer_up = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let drag = Rc::clone(&drag);
        let autoscroll = Rc::clone(&autoscroll);
        let copy_on_select = config.copy_on_select;
        move |cx: &mut EventCx<'_, events::PointerUp>| {
            autoscroll.borrow_mut().take();
            match drag.borrow_mut().take() {
                Some(Drag::Select { .. }) => {
                    cx.release_pointer();
                    let selection = shared.borrow().selections.primary();
                    if copy_on_select && !selection.is_caret() {
                        handle.command(Command::Copy(Clipboard::Primary));
                    }
                }
                Some(Drag::Scrollbar { .. }) => cx.release_pointer(),
                None => {}
            }
        }
    };

    let pointer_leave = move |_: &mut EventCx<'_, events::PointerLeave>| {
        if over_scrollbar.get_untracked() {
            over_scrollbar.set(false);
        }
    };

    // ---- Wheel -------------------------------------------------------------------------

    let wheel = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let smooth = config.smooth_scroll;
        let lines_per = config.scroll_lines;
        move |cx: &mut EventCx<'_, events::Wheel>| {
            let mut needs_glide = false;
            {
                let mut shared = shared.borrow_mut();
                let total = shared.line_count();
                let viewport = shared.viewport_lines();
                let line_height = f64::from(shared.metrics.line_height);
                // The event is the web's `wheel`: a positive delta scrolls the view down and
                // to the right, so deltas apply as they are.
                match cx.delta {
                    ScrollDelta::Lines { x, y } => {
                        let lines = f64::from(y) * lines_per;
                        if smooth {
                            shared.scroll.scroll_by(lines, total, viewport);
                            needs_glide = true;
                        } else {
                            let target = shared.scroll.pos.line + lines;
                            shared.scroll.scroll_to(target, total, viewport);
                        }
                        if x != 0.0 {
                            let text_width = f64::from(shared.text_area_width());
                            let max = (f64::from(shared.horizontal_extent()) - text_width).max(0.0);
                            let cell = f64::from(shared.metrics.cell_advance);
                            shared.scroll.pos.x_px = (shared.scroll.pos.x_px
                                + f64::from(x) * cell * lines_per)
                                .clamp(0.0, max);
                        }
                    }
                    ScrollDelta::Pixels(delta) => {
                        let scale = f64::from(shared.metrics.scale.max(0.001));
                        let dy = f64::from(delta.height.0) * scale;
                        let dx = f64::from(delta.width.0) * scale;
                        let target = shared.scroll.pos.line + dy / line_height;
                        shared.scroll.scroll_to(target, total, viewport);
                        if dx != 0.0 {
                            let text_width = f64::from(shared.text_area_width());
                            let max = (f64::from(shared.horizontal_extent()) - text_width).max(0.0);
                            shared.scroll.pos.x_px = (shared.scroll.pos.x_px + dx).clamp(0.0, max);
                        }
                    }
                    _ => return,
                }
                handle.update_signals(&mut shared);
            }
            handle.ctx.element.repaint();
            if needs_glide && let Some(start) = handle.ctx.start_glide.borrow().as_ref() {
                start();
            }
            if let Some(tell) = handle.ctx.on_event.borrow().as_ref() {
                tell(EditorEvent::Scrolled);
            }
            cx.stop_propagation();
        }
    };

    // ---- Focus -------------------------------------------------------------------------

    let focus_in = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let blink = Rc::clone(&blink);
        let blinking = config.blink;
        let clock = clock.clone();
        move |_: &mut EventCx<'_, events::FocusIn>| {
            {
                let mut shared = shared.borrow_mut();
                shared.focused = true;
                shared.blink_on = true;
            }
            handle.ctx.element.repaint();
            if blinking && blink.borrow().is_none() {
                let shared = Rc::clone(&shared);
                let element = handle.ctx.element.clone();
                *blink.borrow_mut() = clock.as_ref().map(|clock| {
                    clock.set_interval(BLINK_INTERVAL, move || {
                        let mut shared = shared.borrow_mut();
                        shared.blink_on = !shared.blink_on;
                        drop(shared);
                        element.repaint();
                    })
                });
            }
            if let Some(tell) = handle.ctx.on_event.borrow().as_ref() {
                tell(EditorEvent::Focused);
            }
        }
    };
    let focus_out = {
        let shared = Rc::clone(&shared);
        let handle = handle.clone();
        let blink = Rc::clone(&blink);
        move |_: &mut EventCx<'_, events::FocusOut>| {
            {
                let mut shared = shared.borrow_mut();
                shared.focused = false;
                shared.blink_on = true;
            }
            blink.borrow_mut().take();
            handle.ctx.element.repaint();
            if let Some(tell) = handle.ctx.on_event.borrow().as_ref() {
                tell(EditorEvent::Blurred);
            }
        }
    };

    // The editor takes the keys as soon as it is there, when asked to.
    if autofocus && let Some(clock) = clock.as_ref() {
        let claim = clock.set_timeout(Duration::ZERO, move || port.focus());
        on_cleanup_local(move || drop(claim));
    }

    handle.scan_max_width();

    if let Some(ready) = on_ready.as_ref() {
        ready(handle.clone());
    }
    provide_local_context(handle.clone());

    AnyView::new(view! {
        box(
            class = EditorStyle::CLASS,
            class = "editor",
            class = class,
            node_ref = port,
            tabindex = if focusable { Focus::Sequential } else { Focus::Programmatic },
            on:key_down = key_down,
            on:pointer_down = pointer_down,
            on:pointer_move = pointer_move,
            on:pointer_up = pointer_up,
            on:pointer_leave = pointer_leave,
            style:cursor = move || {
                over_scrollbar.get().then(|| "default".to_string())
            },
            on:wheel = wheel,
            on:focus_in = focus_in,
            on:focus_out = focus_out,
            {..attrs}
        ) {
            {element_view.into_view()}
        }
    })
}
