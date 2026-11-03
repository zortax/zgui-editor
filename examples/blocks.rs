//! Content between the lines: review threads under the lines they are about.
//!
//! Each thread is an ordinary zgui box, mounted as a child of the editor and placed at the
//! rectangle `EditorHandle::blocks` publishes for it. The box is as tall as its text, and that
//! height goes back to the editor through `set_blocks`, which makes the room under the line. A
//! thread starts with no room, so it is mounted and measured before the lines below move for it.
//! Scroll, click, type and resize: the threads stay on their lines, the wheel over a thread
//! scrolls the code, and a press on a thread leaves the caret where it is.
//!
//! Run with `cargo run --example blocks --features lang-rust [-- path/to/file]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RenderEffect;
#[allow(unused_imports)]
use zgui_editor::{Block, BlockId, BlockRect, Editor, EditorHandle, EditorProps};

const SHEET: &str = zgui::css!(
    ":root {
        display: flex;
        background: #1e1e2e;
        color: #cdd6f4;
        font-family: 'JetBrains Mono', monospace;
        font-size: 14px;
    }
    .app {
        flex: 1;
        min-width: 0;
        min-height: 0;
    }
    .app__editor {
        --editor-selection: rgba(137, 180, 250, 0.3);
        --editor-cursor: #f5e0dc;
        --editor-current-line: rgba(88, 91, 112, 0.35);
        --editor-gutter-fg: #6c7086;
        --editor-gutter-current-fg: #cdd6f4;
        --editor-scrollbar-thumb: rgba(108, 112, 134, 0.5);
        --syntax-keyword: #cba6f7;
        --syntax-function: #89b4fa;
        --syntax-type: #f9e2af;
        --syntax-string: #a6e3a1;
        --syntax-comment: #6c7086;
    }
    .thread {
        position: absolute;
        display: flex;
        flex-direction: column;
        gap: 4px;
        margin: 6px 12px 6px 0px;
        padding: 8px 12px;
        background: #181825;
        border-left: 3px solid #89b4fa;
        border-radius: 4px;
        font-family: sans-serif;
        font-size: 13px;
    }
    .thread__author { color: #89b4fa; }
    .thread__body { color: #cdd6f4; }"
);

/// One review thread: the line it is about and what was said.
struct Thread {
    /// What the editor calls its block.
    id: BlockId,
    /// The line it sits under.
    line: usize,
    /// Who wrote it.
    author: &'static str,
    /// What they wrote.
    body: &'static str,
}

/// The threads shown, all over the example's own source.
fn threads() -> Vec<Thread> {
    vec![
        Thread {
            id: BlockId(1),
            line: 0,
            author: "reviewer",
            body: "The crate doc says what the example shows. Keep it that short.",
        },
        Thread {
            id: BlockId(2),
            line: 11,
            author: "reviewer",
            body: "A HashMap is enough here: the threads are few, and the order the room is made \
                   in comes from the editor, not from this map.",
        },
        Thread {
            id: BlockId(3),
            line: 120,
            author: "agent",
            body: "The height is read off the laid-out box and given back in CSS pixels. The \
                   editor rounds it to whole device pixels, so the lines below stay sharp.",
        },
    ]
}

/// The threads, placed on their blocks, with their measured heights given back to the editor.
///
/// Built inside the editor, so the handle comes from context.
fn thread_views(threads: Vec<Thread>) -> impl IntoView {
    let handle = use_local_context::<EditorHandle>().expect("the editor provides its handle");
    let threads = Rc::new(threads);
    let heights: Rc<RefCell<HashMap<BlockId, f32>>> = Rc::new(RefCell::new(HashMap::new()));

    // The room every thread takes: what it measured, or nothing before it has been measured.
    let publish = {
        let handle = handle.clone();
        let threads = Rc::clone(&threads);
        let heights = Rc::clone(&heights);
        Rc::new(move || {
            let heights = heights.borrow();
            let blocks = threads
                .iter()
                .map(|thread| {
                    let height = heights.get(&thread.id).copied().unwrap_or(0.0);
                    Block::new(thread.id, thread.line, height)
                })
                .collect();
            handle.set_blocks("threads", blocks);
        })
    };
    publish();

    let blocks = handle.blocks();
    view! {
        For(
            each = move || blocks.get(),
            key = |rect: &BlockRect| rect.id,
            let:rect
        ) {
            {row(rect.id, &threads, &heights, &publish, blocks)}
        }
    }
}

/// The view of the thread with block `id`, which keeps its measured height in `heights` and
/// gives every new height to the editor through `publish`.
fn row(
    id: BlockId,
    threads: &[Thread],
    heights: &Rc<RefCell<HashMap<BlockId, f32>>>,
    publish: &Rc<impl Fn() + 'static>,
    blocks: Signal<Vec<BlockRect>, LocalStorage>,
) -> Option<AnyView> {
    let thread = threads.iter().find(|thread| thread.id == id)?;
    let heights = Rc::clone(heights);
    let publish = Rc::clone(publish);
    let measured = move |id: BlockId, height: f32| {
        let old = heights.borrow_mut().insert(id, height);
        if old != Some(height) {
            publish();
        }
    };
    Some(thread_view(thread, blocks, measured))
}

/// One thread, at its block's rectangle, telling `measured` how tall it is.
fn thread_view(
    thread: &Thread,
    blocks: Signal<Vec<BlockRect>, LocalStorage>,
    measured: impl Fn(BlockId, f32) + 'static,
) -> AnyView {
    let id = thread.id;
    let node = NodeRef::new();
    let size = node.observe_border_size();
    // The margins are part of the room: the box is laid out inside them.
    const MARGIN_Y: f32 = 12.0;
    let watching = RenderEffect::new(move |_| {
        if let Some(size) = size.get() {
            let scale = node.scale().max(0.001);
            measured(id, size.height.0 / scale + MARGIN_Y);
        }
    });
    on_cleanup_local(move || drop(watching));

    let place = move |pick: fn(&BlockRect) -> f32| {
        move || {
            blocks.with(|rects| {
                rects
                    .iter()
                    .find(|rect| rect.id == id)
                    .map(|rect| format!("{}px", pick(rect)))
            })
        }
    };
    let author = thread.author.to_owned();
    let body = thread.body.to_owned();
    AnyView::new(view! {
        box(
            class = "thread",
            node_ref = node,
            style:left = place(|rect| rect.x),
            style:top = place(|rect| rect.y),
            style:width = place(|rect| (rect.width - 12.0).max(0.0))
        ) {
            text(class = "thread__author") {{author}}
            text(class = "thread__body") {{body}}
        }
    })
}

fn main() -> Result<(), zgui::Error> {
    let text = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|error| format!("could not read {path}: {error}\n")),
        None => include_str!("blocks.rs").to_string(),
    };

    app()
        .with_title("zgui-editor: blocks")
        .with_size(1000.0, 700.0)
        .with_stylesheet(SHEET)
        .run(move || {
            let text = text.clone();
            view! {
                column(class = "app") {
                    Editor(class = "app__editor", text = text, language = "rust") {
                        {thread_views(threads())}
                    }
                }
            }
        })
}
