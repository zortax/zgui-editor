//! Blocks through the whole pipeline: room reserved under lines, content mounted over it, and
//! every place that maps lines to the screen counting the room.
//!
//! Headless, through the real layout and event dispatch, with the shipped (empty) fonts: no text
//! is shaped, so what is asserted is geometry — where lines, carets and the mounted content land.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use zgui::platform::{Surface, SurfaceEvent};
use zgui::prelude::*;
use zgui::render::{RenderTarget, Renderer};
use zgui::runtime::{AppError, Runtime};
use zgui::view::{Anchor, BuildCx};
use zgui_editor::{Block, BlockId, Command, EditorConfig, EditorHandle, ScrollCmd};
use zgui_platform_headless::Harness;

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

const SHEET: &str = zgui::css!(
    "root { display: flex; background: #1e1e2e; color: #cdd6f4; }
    .app { flex: 1; min-width: 0; min-height: 0; }
    .app__header { height: 30px; flex: none; }
    .app__body { flex: 1; min-height: 0; min-width: 0; display: flex; }
    .app__side { width: 120px; flex: none; }
    .app__editor { --editor-cursor: #f5e0dc; }
    .app__editor.app__editor--snippet { flex: none; align-self: flex-start; width: 400px; }
    .note { position: absolute; background: #313244; }"
);

/// What a mounted editor hands back to a test.
struct Mounted {
    harness: Harness<Runtime>,
    handle: EditorHandle,
    /// The content mounted over the first block on screen.
    note: NodeRef,
    /// How many presses reached that content.
    presses: Rc<Cell<u32>>,
}

/// An editor over `text`, pushed away from the window's origin by a header and a sidebar, with
/// one piece of content that follows the first block on screen.
fn mounted(text: &str, config: EditorConfig, snippet: bool) -> Mounted {
    let text = text.to_owned();
    let taken: Rc<RefCell<Option<EditorHandle>>> = Rc::new(RefCell::new(None));
    let note = NodeRef::new();
    let presses = Rc::new(Cell::new(0));
    let fonts = zgui::app::Fonts::shipped_only();
    let context_fonts = fonts.clone();
    let runtime = {
        let taken = Rc::clone(&taken);
        let presses = Rc::clone(&presses);
        zgui::runtime::App::new()
            .with_title("blocks")
            .with_size(640.0, 480.0)
            .with_stylesheet(SHEET)
            .with_context(move || zgui::reactive::provide_context(context_fonts))
            .with_renderer(Box::new(
                |_surface: &Arc<dyn Surface>, target: RenderTarget| {
                    let mut renderer = zgui_testkit_scene::CaptureRenderer::new();
                    Renderer::configure(&mut renderer, target);
                    Ok::<_, AppError>(Box::new(renderer) as Box<dyn Renderer>)
                },
            ))
            .with_text_engine(Box::new(|| {
                Box::new(zgui_layout::Paragraphs::new(
                    zgui_testkit_scene::MonoShaper::new(),
                ))
            }))
            .with_glyph_raster(Box::new(|| Arc::new(zgui_testkit_scene::MonoRaster::new())))
            .with_custom(Box::new(|_document| zgui::custom::sources()))
            .into_handler(move |cx: &mut BuildCx<'_>| -> Box<dyn Anchor> {
                let taken = Rc::clone(&taken);
                let presses = Rc::clone(&presses);
                let on_ready = Box::new(move |handle: EditorHandle| {
                    *taken.borrow_mut() = Some(handle);
                }) as Box<dyn Fn(EditorHandle)>;
                let class = if snippet {
                    "app__editor app__editor--snippet"
                } else {
                    "app__editor"
                };
                use zgui::view::IntoView;
                let view = zgui::view::AnyView::new(view! {
                    column(class = "app") {
                        box(class = "app__header") {}
                        row(class = "app__body") {
                            box(class = "app__side") {}
                            Editor(
                                class = class,
                                text = text.clone(),
                                config = config.clone(),
                                autofocus = false,
                                on_ready = on_ready,
                            ) {
                                {first_block_note(note, Rc::clone(&presses))}
                            }
                        }
                    }
                });
                Box::new(view.into_view().build(cx))
            })
            .expect("the reactive runtime installs")
    };
    let mut harness = Harness::new(runtime);
    harness.deliver_to_first(SurfaceEvent::Resized(zgui::geom::Size::new(
        zgui::geom::DevicePx(640.0),
        zgui::geom::DevicePx(480.0),
    )));
    harness.settle(64);
    let handle = taken.borrow_mut().take().expect("on_ready ran");
    Mounted {
        harness,
        handle,
        note,
        presses,
    }
}

/// Content placed on the first block on screen, as an application places a thread.
///
/// Built inside the editor, so the handle comes from context.
fn first_block_note(note: NodeRef, presses: Rc<Cell<u32>>) -> impl IntoView {
    let handle = use_local_context::<EditorHandle>().expect("the editor provides its handle");
    let blocks = handle.blocks();
    let side = move |pick: fn(&zgui_editor::BlockRect) -> f32| {
        move || blocks.with(|rects| rects.first().map(|rect| format!("{}px", pick(rect))))
    };
    view! {
        box(
            class = "note",
            node_ref = note,
            style:left = side(|rect| rect.x),
            style:top = side(|rect| rect.y),
            style:width = side(|rect| rect.width),
            style:height = side(|rect| rect.height),
            tabindex = {Focus::Sequential},
            on:pointer_down = move |_| presses.set(presses.get() + 1)
        ) {}
    }
}

/// A plain editor, with smooth scrolling off so every scroll lands at once.
fn plain(text: &str) -> Mounted {
    let config = EditorConfig {
        smooth_scroll: false,
        ..EditorConfig::default()
    };
    mounted(text, config, false)
}

/// The byte line `line` starts at.
fn line_start(handle: &EditorHandle, line: usize) -> usize {
    handle.query(|snapshot| {
        let rope = snapshot.rope();
        rope.char_to_byte(rope.line_to_char(line))
    })
}

/// The line `byte` is on.
fn line_of(handle: &EditorHandle, byte: usize) -> usize {
    handle.query(|snapshot| snapshot.line_col(byte).line)
}

/// A press and a release of the primary button at `(x, y)` CSS pixels in the window.
fn click(harness: &mut Harness<Runtime>, x: f32, y: f32) {
    let position = zgui::geom::Point::new(zgui::geom::CssPx(x), zgui::geom::CssPx(y));
    for action in [
        zgui::vocab::PointerAction::Pressed,
        zgui::vocab::PointerAction::Released,
    ] {
        harness.deliver_to_first(SurfaceEvent::Pointer {
            action,
            event: zgui::vocab::PointerEvent::mouse(position)
                .with_button(zgui::vocab::PointerButton::Primary),
            modifiers: zgui::vocab::Modifiers::NONE,
            timestamp: zgui::vocab::Timestamp::ORIGIN,
        });
        harness.settle(8);
    }
}

/// A press and a release of a key that types `character`.
fn type_char(harness: &mut Harness<Runtime>, character: char) {
    let event = zgui::vocab::KeyEvent::character(character.to_string());
    for state in [
        zgui::vocab::KeyState::Pressed,
        zgui::vocab::KeyState::Released,
    ] {
        harness.deliver_to_first(SurfaceEvent::Key {
            state,
            event: event.clone(),
            modifiers: zgui::vocab::Modifiers::NONE,
            timestamp: zgui::vocab::Timestamp::ORIGIN,
        });
    }
    harness.settle(8);
}

/// One wheel notch down at `(x, y)` CSS pixels in the window.
fn wheel(harness: &mut Harness<Runtime>, x: f32, y: f32) {
    harness.deliver_to_first(SurfaceEvent::Wheel {
        event: zgui::vocab::WheelEvent {
            delta: zgui::vocab::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            phase: zgui::vocab::ScrollPhase::Discrete,
            position: zgui::geom::Point::new(zgui::geom::CssPx(x), zgui::geom::CssPx(y)),
            id: zgui::vocab::PointerId::MOUSE,
            kind: zgui::vocab::PointerKind::Mouse,
        },
        modifiers: zgui::vocab::Modifiers::NONE,
        timestamp: zgui::vocab::Timestamp::ORIGIN,
    });
    harness.settle(8);
}

/// Forty short lines.
fn forty_lines() -> String {
    (0..40).map(|line| format!("line {line}\n")).collect()
}

#[test]
fn a_line_below_a_block_sits_lower_by_the_blocks_height() {
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&forty_lines());
    let before = handle
        .point_for_byte(line_start(&handle, 3))
        .expect("line three is on screen");
    handle.set_blocks("review", vec![Block::new(BlockId(1), 2, 64.0)]);
    harness.settle(8);

    let after = handle
        .point_for_byte(line_start(&handle, 3))
        .expect("line three is still on screen");
    assert_eq!(after.y, before.y + 64.0, "pushed down by the room");
    let above = handle
        .point_for_byte(line_start(&handle, 2))
        .expect("line two");
    assert_eq!(
        above.y,
        before.y - above.height,
        "the block's own line stays"
    );

    handle.clear_blocks("review");
    harness.settle(8);
    assert_eq!(
        handle.point_for_byte(line_start(&handle, 3)).map(|at| at.y),
        Some(before.y),
        "clearing the layer gives the room back"
    );
}

#[test]
fn a_place_in_a_block_is_no_byte_and_a_place_below_it_is_the_next_line() {
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&forty_lines());
    handle.set_blocks("review", vec![Block::new(BlockId(1), 2, 64.0)]);
    harness.settle(8);
    let two = handle
        .point_for_byte(line_start(&handle, 2))
        .expect("line two");
    let block_y = two.y + two.height + 32.0;
    assert_eq!(handle.byte_for_point(two.x + 1.0, block_y), None);

    for line in [0usize, 1, 2, 3, 4, 10] {
        let at = handle
            .point_for_byte(line_start(&handle, line))
            .expect("on screen");
        let back = handle
            .byte_for_point(at.x + 1.0, at.y + at.height / 2.0)
            .expect("a line is a byte");
        assert_eq!(line_of(&handle, back), line, "the middle of line {line}");
    }
}

#[test]
fn the_blocks_signal_publishes_each_block_on_screen_and_follows_scrolling() {
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&"line\n".repeat(200));
    handle.set_blocks(
        "review",
        vec![
            Block::new(BlockId(1), 2, 40.0),
            Block::new(BlockId(2), 150, 40.0),
        ],
    );
    harness.settle(8);
    let rects = handle.blocks().get_untracked();
    assert_eq!(rects.len(), 1, "only the first is on screen: {rects:?}");
    let first = rects[0];
    let line = handle.local_point_for_byte(0).expect("on screen").height;
    assert_eq!(first.id, BlockId(1));
    assert_eq!(
        first.y,
        3.0 * line,
        "under line two, in the editor's own box"
    );
    assert_eq!(first.height, 40.0);
    assert!(first.x > 0.0, "right of the gutter");

    handle.command(Command::Scroll(ScrollCmd::Lines(1.0)));
    harness.settle(8);
    let moved = handle.blocks().get_untracked();
    assert_eq!(
        moved[0].y,
        first.y - line,
        "a line's scroll moves it a line"
    );

    handle.command(Command::Scroll(ScrollCmd::ToLine(140)));
    harness.settle(8);
    let ids: Vec<BlockId> = handle
        .blocks()
        .get_untracked()
        .iter()
        .map(|rect| rect.id)
        .collect();
    assert_eq!(ids, [BlockId(2)], "the first left, the second came in");
}

#[test]
fn content_mounted_in_the_editor_sits_on_its_block() {
    let Mounted {
        mut harness,
        handle,
        note,
        ..
    } = plain(&forty_lines());
    handle.set_blocks("review", vec![Block::new(BlockId(1), 4, 50.0)]);
    harness.settle(8);
    let rect = handle.blocks().get_untracked()[0];
    let bounds = note.window_bounds().expect("the content is laid out");
    let line = handle
        .point_for_byte(line_start(&handle, 4))
        .expect("line four");
    // The window box of the editor's padding box is the sidebar and the header away.
    assert_eq!(bounds.origin.x.0, 120.0 + rect.x);
    assert_eq!(bounds.origin.y.0, 30.0 + rect.y);
    assert_eq!(
        bounds.origin.y.0,
        line.y + line.height,
        "right under its line"
    );
    assert_eq!(bounds.size.height.0, 50.0);
    assert_eq!(bounds.size.width.0, rect.width);
}

#[test]
fn a_press_in_a_block_leaves_the_caret_and_a_press_on_its_content_is_the_contents() {
    let Mounted {
        mut harness,
        handle,
        note,
        presses,
    } = plain(&forty_lines());
    handle.set_blocks("review", vec![Block::new(BlockId(1), 4, 50.0)]);
    harness.settle(8);

    // A press on a line moves the caret there, which is what the other two must not do.
    let six = handle
        .point_for_byte(line_start(&handle, 6))
        .expect("line six");
    click(&mut harness, six.x + 1.0, six.y + six.height / 2.0);
    let caret = handle.query(|snapshot| snapshot.selections().primary().head);
    assert_eq!(line_of(&handle, caret), 6);

    let bounds = note.window_bounds().expect("the content is laid out");
    click(
        &mut harness,
        bounds.origin.x.0 + 10.0,
        bounds.origin.y.0 + 10.0,
    );
    assert_eq!(presses.get(), 1, "the content took the press");
    assert_eq!(
        handle.query(|snapshot| snapshot.selections().primary().head),
        caret,
        "the caret stays where it was"
    );

    // The empty room of the block, left of the content: over the gutter's column.
    click(&mut harness, 120.0 + 2.0, bounds.origin.y.0 + 10.0);
    assert_eq!(presses.get(), 1);
    assert_eq!(
        handle.query(|snapshot| snapshot.selections().primary().head),
        caret,
        "a press on the room is nobody's"
    );
}

#[test]
fn keys_typed_into_block_content_stay_out_of_the_text() {
    let Mounted {
        mut harness,
        handle,
        note,
        ..
    } = plain("one\ntwo\nthree\n");
    handle.set_blocks("review", vec![Block::new(BlockId(1), 0, 50.0)]);
    harness.settle(8);

    handle.focus();
    harness.settle(8);
    type_char(&mut harness, 'x');
    let text = || handle.query(|snapshot| snapshot.rope().to_string());
    assert_eq!(
        text(),
        "xone\ntwo\nthree\n",
        "the editor types when it has the keys"
    );

    let bounds = note.window_bounds().expect("the content is laid out");
    click(
        &mut harness,
        bounds.origin.x.0 + 5.0,
        bounds.origin.y.0 + 5.0,
    );
    type_char(&mut harness, 'y');
    assert_eq!(
        text(),
        "xone\ntwo\nthree\n",
        "a key for the content does not reach the text through the editor"
    );
}

#[test]
fn the_wheel_over_block_content_scrolls_the_editor() {
    let Mounted {
        mut harness,
        handle,
        note,
        ..
    } = plain(&"line\n".repeat(200));
    handle.set_blocks("review", vec![Block::new(BlockId(1), 4, 50.0)]);
    harness.settle(8);
    let bounds = note.window_bounds().expect("the content is laid out");
    wheel(
        &mut harness,
        bounds.origin.x.0 + 10.0,
        bounds.origin.y.0 + 10.0,
    );
    assert_eq!(handle.scroll_state().get_untracked().top_line, 3.0);
}

#[test]
fn scrolled_to_the_end_the_last_line_sits_at_the_bottom_under_every_block() {
    let text = "line\n".repeat(100);
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&text);
    handle.set_blocks(
        "review",
        (0..10)
            .map(|index| Block::new(BlockId(index), index as usize * 10, 33.0))
            .collect(),
    );
    handle.command(Command::Scroll(ScrollCmd::ToLine(1000)));
    harness.settle(8);
    let last = handle
        .local_point_for_byte(text.len())
        .expect("the last line is on screen");
    assert!(
        (last.y + last.height - 480.0 + 30.0).abs() < 1e-3,
        "the last line ends at the bottom: {last:?}"
    );
    let state = handle.scroll_state().get_untracked();
    assert!(
        state.max_top > 101.0 - state.viewport_lines,
        "the room of the blocks is scrollable: {state:?}"
    );
}

#[test]
fn moving_the_caret_down_past_blocks_keeps_it_in_view() {
    let text = "line\n".repeat(200);
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&text);
    handle.set_blocks(
        "review",
        (0..20)
            .map(|index| Block::new(BlockId(index), index as usize * 5, 60.0))
            .collect(),
    );
    harness.settle(8);
    let viewport = 480.0 - 30.0;
    for line in [30usize, 90, 150, 60, 5] {
        handle.command(Command::SetSelections {
            selections: vec![zgui_editor::Selection::caret(line_start(&handle, line))],
            primary: 0,
        });
        handle.command(Command::Scroll(ScrollCmd::EnsureCursorVisible));
        harness.settle(8);
        let caret = handle
            .local_point_for_byte(line_start(&handle, line))
            .expect("the caret's line is on screen");
        assert!(
            caret.y >= 0.0 && caret.y + caret.height <= viewport,
            "line {line} is inside the view: {caret:?}"
        );
    }
}

#[test]
fn a_new_line_above_a_block_takes_the_block_down_with_its_line() {
    let Mounted {
        mut harness,
        handle,
        ..
    } = plain(&forty_lines());
    handle.set_blocks("review", vec![Block::new(BlockId(1), 4, 50.0)]);
    harness.settle(8);
    handle.command(Command::Insert("new\n".to_owned()));
    harness.settle(8);
    let rect = handle.blocks().get_untracked()[0];
    assert_eq!(rect.line, 5, "the block went with its line");
    let line = handle
        .local_point_for_byte(line_start(&handle, 5))
        .expect("line five");
    assert_eq!(rect.y, line.y + line.height);
}

#[test]
fn a_window_is_as_tall_as_its_lines_and_the_blocks_under_them() {
    let config = EditorConfig {
        line_window: Some(1..4),
        ..EditorConfig::default()
    };
    let Mounted {
        mut harness,
        handle,
        note,
        ..
    } = mounted(&forty_lines(), config, true);
    let line = handle
        .local_point_for_byte(line_start(&handle, 1))
        .expect("on screen")
        .height;
    assert_eq!(editor_height(&mut harness), 3.0 * line);

    handle.set_blocks(
        "review",
        vec![
            Block::new(BlockId(1), 0, 100.0),
            Block::new(BlockId(2), 3, 45.0),
            Block::new(BlockId(3), 4, 100.0),
        ],
    );
    assert_eq!(
        editor_height(&mut harness),
        3.0 * line + 45.0,
        "only the block under the window's last line is inside it"
    );
    let bounds = note.window_bounds().expect("the content is laid out");
    assert_eq!(bounds.origin.y.0, 30.0 + 3.0 * line);
    assert_eq!(bounds.size.height.0, 45.0);
}

/// How tall the editor's frame is laid out: the largest custom element in the window.
fn editor_height(harness: &mut Harness<Runtime>) -> f32 {
    harness.settle(8);
    let window = &harness.app_mut().windows_mut()[0];
    let document = window.document().borrow();
    let layout = window.layout().borrow();
    let mut boxes = Vec::new();
    for key in layout.keys() {
        let Some(source) = layout.node(key).source else {
            continue;
        };
        let Some(index) = document.store().index_of(source) else {
            continue;
        };
        if document.store().core(index).local_name().as_str() != "custom" {
            continue;
        }
        if let Some(fragment) = layout
            .fragments_of_box(key)
            .first()
            .and_then(|fragment| layout.fragment(*fragment))
        {
            let border = fragment.border_box;
            boxes.push((border.size.width.0, border.size.height.0));
        }
    }
    boxes
        .into_iter()
        .max_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1)))
        .map_or(0.0, |(_, height)| height)
}
