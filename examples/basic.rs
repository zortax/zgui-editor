//! The smallest editor: one file, the default keymap, a dark theme.
//!
//! Run with `cargo run --example basic --features lang-rust [-- path/to/file]`.

use zgui::prelude::*;
#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

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
        --editor-cursor-text: #1e1e2e;
        --editor-current-line: rgba(88, 91, 112, 0.35);
        --editor-gutter-fg: #6c7086;
        --editor-gutter-current-fg: #cdd6f4;
        --editor-scrollbar-thumb: rgba(108, 112, 134, 0.5);
        --syntax-keyword: #cba6f7;
        --syntax-function: #89b4fa;
        --syntax-type: #f9e2af;
        --syntax-string: #a6e3a1;
        --syntax-number: #fab387;
        --syntax-comment: #6c7086;
        --syntax-constant: #fab387;
        --syntax-property: #b4befe;
        --syntax-operator: #94e2d5;
        --syntax-punctuation: #9399b2;
        --syntax-attribute: #f9e2af;
        --syntax-variable: #cdd6f4;
    }"
);

fn main() -> Result<(), zgui::Error> {
    let text = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|error| format!("could not read {path}: {error}\n")),
        None => include_str!("basic.rs").to_string(),
    };

    app()
        .with_title("zgui-editor")
        .with_size(1000.0, 700.0)
        .with_stylesheet(SHEET)
        .run(move || {
            let text = text.clone();
            view! {
                column(class = "app") {
                    Editor(class = "app__editor", text = text, language = "rust")
                }
            }
        })
}
