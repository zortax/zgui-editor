//! A completion popover, anchored to the caret, built entirely outside the component.
//!
//! The editor supplies three things and no more: the caret's window rectangle as a signal,
//! events saying the text or selection changed, and first refusal on every key. The popover —
//! the word list, the filtering, the accept/dismiss keys — is all application code, which is
//! exactly how an IDE's completion, signature help or hover would sit on top of this editor.
//!
//! Run with `cargo run --example completion --features lang-rust`.

use std::collections::BTreeSet;
use std::ops::Range;

use zgui::prelude::*;
use zgui::vocab::{Key, NamedKey};
#[allow(unused_imports)]
use zgui_editor::{
    Command, EditKind, Editor, EditorEvent, EditorHandle, EditorProps, KeyFilter, Selection,
};

const SHEET: &str = zgui::css!(
    ":root {
        display: flex;
        background: #1e1e2e;
        color: #cdd6f4;
        font-family: 'JetBrains Mono', monospace;
        font-size: 14px;
    }
    .app { flex: 1; min-width: 0; min-height: 0; position: relative; }
    .app__editor {
        --editor-selection: rgba(137, 180, 250, 0.3);
        --editor-cursor: #f5e0dc;
        --editor-cursor-text: #1e1e2e;
        --editor-current-line: rgba(88, 91, 112, 0.35);
        --editor-gutter-fg: #6c7086;
        --editor-scrollbar-thumb: rgba(108, 112, 134, 0.5);
        --syntax-keyword: #cba6f7;
        --syntax-function: #89b4fa;
        --syntax-string: #a6e3a1;
        --syntax-comment: #6c7086;
        --syntax-type: #f9e2af;
    }
    .completion {
        position: absolute;
        display: flex;
        flex-direction: column;
        min-width: 220px;
        max-height: 240px;
        overflow: hidden;
        background: #181825;
        border: 1px solid #313244;
        border-radius: 6px;
        box-shadow: 0px 8px 24px rgba(0, 0, 0, 0.5);
        font-size: 13px;
        z-index: 10;
    }
    .completion__item { padding: 3px 10px; }
    .completion__item.selected { background: #313244; color: #89b4fa; }"
);

/// The open popover: what it offers, which entry is lit, and what it would replace.
#[derive(Clone, PartialEq)]
struct Completion {
    items: Vec<String>,
    selected: usize,
    replaces: Range<usize>,
}

/// The word being typed at the caret, when one is.
fn prefix_at(handle: &EditorHandle) -> Option<(Range<usize>, String)> {
    handle.query(|snapshot| {
        let caret = snapshot.selections().primary().head;
        let rope = snapshot.rope();
        let line = rope.byte_to_line(caret);
        let start = rope.line_to_byte(line);
        let text: String = rope.byte_slice(start..caret).to_string();
        let from = text
            .rfind(|character: char| !character.is_alphanumeric() && character != '_')
            .map(|at| at + 1)
            .unwrap_or(0);
        let word = &text[from..];
        if word.len() < 2 || word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        Some((start + from..caret, word.to_string()))
    })
}

/// Every word in the buffer starting with `prefix`, except the prefix itself.
fn candidates(handle: &EditorHandle, prefix: &str) -> Vec<String> {
    handle.query(|snapshot| {
        let mut words = BTreeSet::new();
        let mut word = String::new();
        // Bounded, so a pathological buffer costs a bounded scan; a real completion engine
        // would index instead.
        for character in snapshot.rope().chars().take(500_000) {
            if character.is_alphanumeric() || character == '_' {
                word.push(character);
            } else {
                if word.starts_with(prefix) && word != prefix && word.len() > prefix.len() {
                    words.insert(word.clone());
                }
                word.clear();
            }
        }
        if word.starts_with(prefix) && word != prefix {
            words.insert(word);
        }
        words.into_iter().take(12).collect()
    })
}

fn main() -> Result<(), zgui::Error> {
    let text = include_str!("completion.rs").to_string();

    app()
        .with_title("zgui-editor completion")
        .with_size(1000.0, 700.0)
        .with_stylesheet(SHEET)
        .run(move || {
            let text = text.clone();
            let completion: RwSignal<Option<Completion>, LocalStorage> = RwSignal::new_local(None);
            let editor: RwSignal<Option<EditorHandle>, LocalStorage> = RwSignal::new_local(None);

            // The text changed: refresh or close the popover from what the buffer now holds.
            let on_event: Box<dyn Fn(EditorEvent)> = Box::new(move |event| match event {
                EditorEvent::Edited {
                    kind: EditKind::Typing | EditKind::Deletion,
                    ..
                } => {
                    let Some(handle) = editor.get_untracked() else {
                        return;
                    };
                    let refreshed = prefix_at(&handle).and_then(|(replaces, prefix)| {
                        let items = candidates(&handle, &prefix);
                        (!items.is_empty()).then_some(Completion {
                            items,
                            selected: 0,
                            replaces,
                        })
                    });
                    completion.set(refreshed);
                }
                EditorEvent::Edited { .. }
                | EditorEvent::SelectionMoved
                | EditorEvent::Scrolled
                | EditorEvent::Blurred => completion.set(None),
                _ => {}
            });

            // While the popover is open, it has first claim on the keys that drive it.
            let on_key: KeyFilter = Box::new(move |event, _modifiers, handle| {
                let Some(open) = completion.get_untracked() else {
                    return false;
                };
                match &event.key {
                    Key::Named(NamedKey::ArrowDown) => {
                        completion.set(Some(Completion {
                            selected: (open.selected + 1) % open.items.len(),
                            ..open
                        }));
                        true
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        completion.set(Some(Completion {
                            selected: (open.selected + open.items.len() - 1) % open.items.len(),
                            ..open
                        }));
                        true
                    }
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                        let chosen = open.items[open.selected].clone();
                        handle.command(Command::ReplaceRanges(vec![(open.replaces, chosen)]));
                        completion.set(None);
                        true
                    }
                    Key::Named(NamedKey::Escape) => {
                        completion.set(None);
                        true
                    }
                    _ => false,
                }
            });

            view! {
                box(class = "app") {
                    Editor(
                        class = "app__editor",
                        text = text,
                        language = "rust",
                        on_event = on_event,
                        on_key = on_key,
                        on_ready = Box::new(move |handle| editor.set(Some(handle)))
                            as Box<dyn Fn(EditorHandle)>,
                    )
                    if move || completion.get().is_some() {
                        column(
                            class = "completion",
                            style:left = move || {
                                let handle = editor.get()?;
                                let rect = handle.caret_rect().get()?;
                                Some(format!("{}px", rect.x))
                            },
                            style:top = move || {
                                let handle = editor.get()?;
                                let rect = handle.caret_rect().get()?;
                                Some(format!("{}px", rect.y + rect.height + 2.0))
                            },
                        ) {
                            for entry in move || {
                                completion
                                    .get()
                                    .map(|open| {
                                        open.items
                                            .iter()
                                            .cloned()
                                            .enumerate()
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default()
                            }, key = |entry: &(usize, String)| entry.1.clone() {
                                box(
                                    class = "completion__item",
                                    class:selected = {
                                        let index = entry.0;
                                        move || {
                                            completion
                                                .get()
                                                .map(|open| open.selected == index)
                                                .unwrap_or(false)
                                        }
                                    },
                                ) {
                                    text {{entry.1.clone()}}
                                }
                            }
                        }
                    }
                }
            }
        })
}
