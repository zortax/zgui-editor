//! What the editor reports, and how an application intercepts its keys.

use std::sync::Arc;

use crate::core::edit::{EditKind, TextChange};

/// What is asked about a key before the editor interprets it.
///
/// The filter runs first, with the raw event — before the default keymap, before insertion,
/// before Tab moves focus. Answering `true` consumes the key entirely; answering `false` lets
/// the default keymap have it. This is the whole seam a modal layer needs: a vim mode consumes
/// everything in normal mode and almost nothing in insert mode.
pub type KeyFilter = Box<
    dyn Fn(&zgui::vocab::KeyEvent, zgui::vocab::Modifiers, &crate::handle::EditorHandle) -> bool,
>;

/// Something the editor did that an application may want to hear about.
#[derive(Clone, Debug)]
pub enum EditorEvent {
    /// The text changed.
    Edited {
        /// What kind of change it was.
        kind: EditKind,
        /// The revision the buffer now has.
        revision: u64,
        /// The replacements that made the change, in the order they applied.
        ///
        /// Empty when the whole text was replaced through
        /// [`set_text`](crate::EditorHandle::set_text): that is not a change anything can apply
        /// on top of what it already held, so a consumer that synchronises incrementally — a
        /// language server — resends the document instead.
        changes: Arc<[TextChange]>,
    },
    /// The selections moved without the text changing.
    SelectionMoved,
    /// The view scrolled.
    Scrolled,
    /// The editor lost focus.
    Blurred,
    /// The editor gained focus.
    Focused,
    /// A context menu was asked for.
    ContextMenu {
        /// Where, as a byte offset into the buffer.
        byte: usize,
    },
}
