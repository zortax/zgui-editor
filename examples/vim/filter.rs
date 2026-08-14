//! The `on_key` filter: where the layer meets the editor.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::vocab::{Key, NamedKey};
use zgui_editor::KeyFilter;

use crate::actions;
use crate::keymap::{self, Step};
use crate::mode::VimState;

/// Builds the filter over `state`; `on_update` hears every change, which is what the status
/// line binds through.
pub fn make_filter(
    state: Rc<RefCell<VimState>>,
    on_update: impl Fn(&VimState) + 'static,
) -> KeyFilter {
    Box::new(move |event, modifiers, handle| {
        let mut state = state.borrow_mut();

        // The `/` line eats every key while it is open.
        let consumed = if state.search_input.is_some() {
            match &event.key {
                Key::Named(NamedKey::Enter) => {
                    let needle = state.search_input.take().unwrap_or_default();
                    if !needle.is_empty() {
                        state.last_search = Some(needle.clone());
                        actions::search_to(handle, &needle, false);
                    }
                }
                Key::Named(NamedKey::Escape) => state.search_input = None,
                Key::Named(NamedKey::Backspace) => {
                    let empty = state
                        .search_input
                        .as_mut()
                        .map(|input| input.pop().is_none())
                        .unwrap_or(true);
                    if empty {
                        state.search_input = None;
                    }
                }
                key => {
                    if let Some(text) = key.inserted_text()
                        && let Some(input) = state.search_input.as_mut()
                    {
                        input.push_str(text);
                    }
                }
            }
            true
        } else {
            let mode = state.mode;
            let step = keymap::translate(mode, &mut state.pending, event, modifiers);
            match step {
                Step::Consume(list) => {
                    for action in list {
                        actions::perform(&mut state, action, handle);
                    }
                    true
                }
                Step::Pending => true,
                Step::PassThrough => false,
            }
        };

        on_update(&state);
        consumed
    })
}
