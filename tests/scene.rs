//! The whole pipeline, headless: the editor mounts, lays out through the real engines, and its
//! element's primitives reach the display list.
//!
//! Fonts are the shipped (empty) set, so no real face resolves and no text is shaped — that
//! keeps the test deterministic on any machine. What is asserted is the chrome the element
//! paints regardless: quads for the caret and the current line, arriving through the real
//! layout, paint and capture pipeline.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use zgui::platform::{Surface, SurfaceEvent};
use zgui::prelude::*;
use zgui::render::{RenderTarget, Renderer};
use zgui::runtime::{AppError, Runtime};
use zgui::view::{Anchor, BuildCx};
use zgui_editor::EditorHandle;
use zgui_platform_headless::Harness;

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

const SHEET: &str = zgui::css!(
    "root { display: flex; background: #1e1e2e; color: #cdd6f4; }
    .app { flex: 1; min-width: 0; min-height: 0; }
    .app__header { height: 30px; flex: none; }
    .app__body { flex: 1; min-height: 0; min-width: 0; display: flex; }
    .app__side { width: 120px; flex: none; }
    .app__editor {
        --editor-current-line: rgba(88, 91, 112, 0.4);
        --editor-cursor: #f5e0dc;
        --editor-scrollbar-thumb: rgba(108, 112, 134, 0.5);
        --editor-search: rgba(249, 226, 175, 0.35);
    }"
);

/// The same, with the editor pushed away from the window's origin by a header and a sidebar.
///
/// The shape every real application has, and the one a coordinate bug hides from: at the origin,
/// window coordinates and element coordinates are the same number, so an origin that is never
/// subtracted looks correct.
fn mounted_offset(text: &'static str) -> (Harness<Runtime>, EditorHandle) {
    mounted_with(text, true)
}

fn mounted(text: &'static str) -> (Harness<Runtime>, EditorHandle) {
    mounted_with(text, false)
}

fn mounted_with(text: &'static str, offset: bool) -> (Harness<Runtime>, EditorHandle) {
    let taken: Rc<RefCell<Option<EditorHandle>>> = Rc::new(RefCell::new(None));
    let fonts = zgui::app::Fonts::shipped_only();
    let context_fonts = fonts.clone();
    let runtime = {
        let taken = Rc::clone(&taken);
        zgui::runtime::App::new()
            .with_title("scene")
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
                let on_ready = Box::new(move |handle: EditorHandle| {
                    *taken.borrow_mut() = Some(handle);
                }) as Box<dyn Fn(EditorHandle)>;
                use zgui::view::IntoView;
                let view: zgui::view::AnyView = if offset {
                    zgui::view::AnyView::new(view! {
                        column(class = "app") {
                            box(class = "app__header") {}
                            row(class = "app__body") {
                                box(class = "app__side") {}
                                Editor(class = "app__editor", text = text, on_ready = on_ready)
                            }
                        }
                    })
                } else {
                    zgui::view::AnyView::new(view! {
                        column(class = "app") {
                            Editor(class = "app__editor", text = text, on_ready = on_ready)
                        }
                    })
                };
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
    (harness, handle)
}

#[test]
fn the_editor_reaches_the_display_list() {
    let (mut harness, handle) = mounted("fn main() {\n    let greeting = \"hello\";\n}\n");
    // A settled window replays; a command re-encodes, which is what the capture shows.
    handle.command(zgui_editor::Command::Scroll(
        zgui_editor::ScrollCmd::EnsureCursorVisible,
    ));
    harness.settle(8);
    let window = &mut harness.app_mut().windows_mut()[0];
    let scene = window.scene();
    assert!(
        !scene.primitives.quads.is_empty(),
        "the editor painted quads through the real pipeline"
    );
}

#[test]
fn typing_updates_the_model_and_the_frame() {
    let (mut harness, handle) = mounted("");
    handle.command(zgui_editor::Command::Insert("hello".to_string()));
    harness.settle(16);
    assert_eq!(
        handle.query(|snapshot| snapshot.rope().to_string()),
        "hello"
    );
    let window = &mut harness.app_mut().windows_mut()[0];
    assert!(
        !window.scene().primitives.quads.is_empty(),
        "the frame after the edit painted"
    );
}

#[test]
fn a_decoration_layer_paints_and_clears() {
    let (mut harness, handle) = mounted("fn main() {\n    let greeting = \"hello\";\n}\n");
    // A settled window replays rather than re-encoding, so every measurement has to follow
    // something that made the element paint again.
    let quads = |harness: &mut Harness<Runtime>, handle: &EditorHandle| {
        handle.command(zgui_editor::Command::Scroll(
            zgui_editor::ScrollCmd::EnsureCursorVisible,
        ));
        harness.settle(8);
        harness.app_mut().windows_mut()[0]
            .scene()
            .primitives
            .quads
            .len()
    };
    let bare = quads(&mut harness, &handle);

    handle.set_decorations(
        "search",
        vec![
            zgui_editor::Decoration::background(3..7, "editor-search"),
            zgui_editor::Decoration::background(20..27, "editor-search"),
        ],
    );
    let decorated = quads(&mut harness, &handle);
    assert!(
        decorated > bare,
        "two bands should be two more quads: {bare} then {decorated}"
    );

    handle.clear_decorations("search");
    assert_eq!(
        quads(&mut harness, &handle),
        bare,
        "clearing a layer leaves the frame as it was"
    );
}

#[test]
fn a_straight_underline_reaches_the_display_list() {
    // A straight underline is a quad. The waves and the dashes are strokes, which this harness
    // has no rasteriser for; their geometry is asserted where it is built instead.
    let (mut harness, handle) = mounted("fn main() {}\n");
    let quads = |harness: &mut Harness<Runtime>, handle: &EditorHandle| {
        handle.command(zgui_editor::Command::Scroll(
            zgui_editor::ScrollCmd::EnsureCursorVisible,
        ));
        harness.settle(8);
        harness.app_mut().windows_mut()[0]
            .scene()
            .primitives
            .quads
            .len()
    };
    let bare = quads(&mut harness, &handle);

    handle.set_decorations(
        "diagnostics",
        vec![zgui_editor::Decoration::underline(
            3..7,
            zgui_editor::UnderlineStyle::Straight,
            zgui::canvas::zgui_color::Color::srgb(1.0, 0.2, 0.2, 1.0),
        )],
    );
    assert!(quads(&mut harness, &handle) > bare, "the underline painted");
}

#[test]
fn a_byte_maps_to_a_place_on_the_window_and_back() {
    let (mut harness, handle) = mounted("fn main() {\n    let greeting = \"hello\";\n}\n");
    harness.settle(8);

    let start = handle
        .point_for_byte(0)
        .expect("the first byte is on screen");
    let later = handle
        .point_for_byte(20)
        .expect("the second line is on screen");
    assert!(
        later.y > start.y,
        "a byte on a later line sits lower: {start:?} then {later:?}"
    );
    assert!(later.height > 0.0);

    // The inverse lands back on the line it came from, which is all a hit test promises: the
    // shaper here gives every character the same cell, so the column is exact too.
    let back = handle
        .byte_for_point(later.x, later.y + later.height / 2.0)
        .expect("the point is inside the editor");
    assert_eq!(
        handle.query(|snapshot| snapshot.line_col(back).line),
        handle.query(|snapshot| snapshot.line_col(20).line),
    );

    assert!(
        handle.byte_for_point(-50.0, -50.0).is_none(),
        "a point outside the element is not a byte"
    );
}

#[test]
fn the_visible_range_is_inside_the_text() {
    let text = "one\ntwo\nthree\nfour\nfive\n";
    let (mut harness, handle) = mounted(text);
    harness.settle(8);
    let visible = handle.query(|snapshot| snapshot.visible_byte_range());
    assert!(!visible.is_empty(), "something is on screen");
    assert_eq!(visible.start, 0, "the view starts at the top");
    assert!(visible.end <= text.len());
    // Slicing the rope with it is the whole point of answering in bytes.
    let seen = handle.query(|snapshot| snapshot.text_in(visible.clone()));
    assert!(seen.starts_with("one"));
}

#[test]
fn a_byte_maps_to_the_window_even_when_the_editor_is_not_at_the_origin() {
    // The shape every application has: a header above and a sidebar beside. A point mapping that
    // forgets the element's own origin is exactly right at (0, 0) and wrong everywhere else, so
    // this is the test that can tell.
    let (mut harness, handle) = mounted_offset("fn main() {\n    let greeting = \"hello\";\n}\n");
    harness.settle(8);

    let start = handle
        .point_for_byte(0)
        .expect("the first byte is on screen");
    assert!(
        start.x >= 120.0,
        "the first byte is to the right of the sidebar, not at the window's edge: {start:?}"
    );
    assert!(
        start.y >= 30.0,
        "and below the header: {start:?}"
    );

    // And back again: the point the first byte is at is the first byte.
    let back = handle
        .byte_for_point(start.x + 1.0, start.y + start.height / 2.0)
        .expect("the point is inside the editor");
    assert_eq!(
        handle.query(|snapshot| snapshot.line_col(back).line),
        0,
        "the round trip stays on the line it started on"
    );
}

#[test]
fn a_point_maps_to_the_line_under_it_when_the_editor_is_offset() {
    let (mut harness, handle) = mounted_offset("one\ntwo\nthree\nfour\nfive\nsix\n");
    harness.settle(8);

    // Every line, by the middle of the row `point_for_byte` says it is on.
    for line in 0..6usize {
        let byte = handle.query(|snapshot| {
            let rope = snapshot.rope();
            rope.char_to_byte(rope.line_to_char(line))
        });
        let Some(at) = handle.point_for_byte(byte) else {
            continue;
        };
        let back = handle
            .byte_for_point(at.x + 1.0, at.y + at.height / 2.0)
            .unwrap_or_else(|| panic!("line {line} at {at:?} is inside the editor"));
        assert_eq!(
            handle.query(|snapshot| snapshot.line_col(back).line),
            line,
            "clicking the middle of line {line}'s row lands on line {line}"
        );
    }
}

/// The element-local position of a byte, which is what an overlay drawn inside the editor wants.
#[test]
fn a_byte_maps_to_a_place_inside_the_element_too() {
    let (mut harness, handle) = mounted_offset("one\ntwo\nthree\n");
    harness.settle(8);

    let window = handle.point_for_byte(0).expect("on screen");
    let local = handle.local_point_for_byte(0).expect("on screen");

    assert!(
        local.x < window.x && local.y < window.y,
        "the element's own origin is subtracted: {local:?} against {window:?}"
    );
    // The first byte sits at the gutter's right edge, a little in from the element's left, and on
    // its first row.
    assert!(local.y < window.height, "the first line is the top row");
}
