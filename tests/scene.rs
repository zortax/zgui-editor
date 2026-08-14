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
    .app__editor {
        --editor-current-line: rgba(88, 91, 112, 0.4);
        --editor-cursor: #f5e0dc;
        --editor-scrollbar-thumb: rgba(108, 112, 134, 0.5);
        --editor-search: rgba(249, 226, 175, 0.35);
    }"
);

fn mounted(text: &'static str) -> (Harness<Runtime>, EditorHandle) {
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
                let view = view! {
                    column(class = "app") {
                        Editor(
                            class = "app__editor",
                            text = text,
                            on_ready = Box::new(move |handle: EditorHandle| {
                                *taken.borrow_mut() = Some(handle);
                            }) as Box<dyn Fn(EditorHandle)>,
                        )
                    }
                };
                use zgui::view::IntoView;
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
