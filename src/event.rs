//! What the editor reports, and how an application intercepts its keys.

use crate::core::edit::EditKind;

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
