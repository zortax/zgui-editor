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
