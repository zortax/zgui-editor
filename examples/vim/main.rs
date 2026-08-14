//! Modal editing over the plain editor component, built entirely outside it.
//!
//! The editor knows nothing about modes. Everything vim-shaped lives in this example: the mode
//! state machine, the count/operator grammar, the registers, the `/` search line — all of it
//! driving the component through its public `on_key` filter and `EditorHandle`.
//!
//! Run with `cargo run --example vim --features lang-rust [-- path/to/file]`.

mod actions;
mod filter;
mod keymap;
mod mode;
mod pending;
mod registers;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
#[allow(unused_imports)]
use zgui_editor::{
    CursorStyle, Editor, EditorConfig, EditorHandle, EditorProps, GutterMode, KeyFilter,
};

use crate::mode::VimState;

const SHEET: &str = zgui::css!(
    ":root {
        display: flex;
        background: #1e1e2e;
        color: #cdd6f4;
        font-family: 'Mononoki Nerd Font Mono', monospace;
        font-weight: 600;
        font-size: 14px;
    }
    .app { flex: 1; min-width: 0; min-height: 0; }
    .app__editor {
        --editor-selection: rgba(137, 180, 250, 0.3);
        --editor-cursor: #f5e0dc;
        --editor-cursor-text: #1e1e2e;
        --editor-current-line: rgba(88, 91, 112, 0.35);
        --editor-gutter-fg: #6c7086;
        --editor-gutter-current-fg: #f9e2af;
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
    }
    .statusline {
        align-items: center;
        gap: 12px;
        padding: 4px 10px;
        background: #181825;
        font-size: 12px;
    }
    .statusline__mode {
        padding: 2px 10px;
        border-radius: 4px;
        background: #89b4fa;
        color: #11111b;
        font-weight: 700;
    }
    .statusline__mode[data-mode='INSERT'] { background: #a6e3a1; }
    .statusline__mode[data-mode='VISUAL'], .statusline__mode[data-mode='V-LINE'] { background: #cba6f7; }
    .statusline__search { color: #f9e2af; }
    .statusline__fill { flex: 1; }
    .statusline__echo { color: #6c7086; }
    .statusline__pos { color: #9399b2; }"
);

fn main() -> Result<(), zgui::Error> {
    let text = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|error| format!("could not read {path}: {error}\n")),
        None => include_str!("main.rs").to_string(),
    };

    app()
        .with_title("zgui-editor vim")
        .with_size(1100.0, 750.0)
        .with_stylesheet(SHEET)
        .run(move || {
            let text = text.clone();

            let state = Rc::new(RefCell::new(VimState::new()));
            let mode_label = RwSignal::new_local(mode::Mode::Normal.label());
            let echo = RwSignal::new_local(String::new());
            let search_line = RwSignal::new_local(None::<String>);
            let editor: RwSignal<Option<EditorHandle>, LocalStorage> = RwSignal::new_local(None);

            let filter: KeyFilter = filter::make_filter(Rc::clone(&state), move |state| {
                mode_label.set(state.mode.label());
                echo.set(state.pending.echo());
                search_line.set(state.search_input.clone());
            });

            let config = EditorConfig {
                gutter: GutterMode::Relative,
                cursor_style: CursorStyle::Block,
                scrolloff: 3,
                ..EditorConfig::default()
            };

            view! {
                column(class = "app") {
                    Editor(
                        class = "app__editor",
                        text = text,
                        language = "rust",
                        config = config,
                        on_key = filter,
                        on_ready = Box::new(move |handle| editor.set(Some(handle)))
                            as Box<dyn Fn(EditorHandle)>,
                    )
                    row(class = "statusline") {
                        box(
                            class = "statusline__mode",
                            attr:data-mode = move || Some(mode_label.get().to_string()),
                        ) {
                            text {{move || mode_label.get().to_string()}}
                        }
                        text(class = "statusline__search") {{move || {
                            search_line
                                .get()
                                .map(|input| format!("/{input}"))
                                .unwrap_or_default()
                        }}}
                        box(class = "statusline__fill") {}
                        text(class = "statusline__echo") {{move || echo.get()}}
                        text(class = "statusline__pos") {{move || {
                            let position = editor
                                .get()
                                .map(|handle| handle.cursor_position().get())
                                .unwrap_or_default();
                            format!("{}:{}", position.line + 1, position.col + 1)
                        }}}
                    }
                }
            }
        })
}
