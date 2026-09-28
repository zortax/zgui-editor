//! The commands an editor is driven by.
//!
//! Everything that can happen to the text or the selections is a [`Command`], whether it came
//! from the default keymap, from a vim layer outside the component, or from application code
//! holding an [`EditorHandle`](crate::EditorHandle). The vocabulary is deliberately mode-neutral:
//! vim's grammar — counts, operators, registers — lowers to it without the editor knowing vim
//! exists.

use crate::core::selection::Selection;

/// A movement, before it is aimed at a selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    /// One grapheme left, stopping at the line start.
    Left,
    /// One grapheme right, stopping at the line end.
    Right,
    /// One line up, keeping the goal column.
    Up,
    /// One line down, keeping the goal column.
    Down,
    /// To the start of the next word. `big` counts only whitespace as a divider.
    WordForward {
        /// Whether only whitespace divides words.
        big: bool,
    },
    /// To the start of this or the previous word.
    WordBackward {
        /// Whether only whitespace divides words.
        big: bool,
    },
    /// To the end of this or the next word.
    WordEnd {
        /// Whether only whitespace divides words.
        big: bool,
    },
    /// To column zero.
    LineStart,
    /// To the first character on the line that is not whitespace.
    LineFirstNonBlank,
    /// To the line's end.
    LineEnd,
    /// To the first line.
    DocumentStart,
    /// To past the last character.
    DocumentEnd,
    /// To the start of line `line`, counting from zero.
    GotoLine(usize),
    /// One viewport up.
    PageUp,
    /// One viewport down.
    PageDown,
    /// Half a viewport up.
    HalfPageUp,
    /// Half a viewport down.
    HalfPageDown,
    /// To the byte offset, as a search hit or a mark supplies one.
    To(usize),
}

/// Where pasted or supplied text lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InsertPoint {
    /// At each caret, replacing each selection.
    AtCarets,
    /// After each caret — vim's `p` for characterwise text.
    AfterCarets,
}

/// A scroll request, moving the view without moving the selections.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScrollCmd {
    /// By this many lines; positive scrolls the text up (toward later lines).
    Lines(f64),
    /// By this many viewports.
    Pages(f64),
    /// Until `line` is the top line.
    ToLine(usize),
    /// Until the primary caret's line is centered.
    CursorCenter,
    /// Until the primary caret's line is at the top margin.
    CursorTop,
    /// Until the primary caret's line is at the bottom margin.
    CursorBottom,
    /// Horizontally, by device pixels; positive scrolls the text left.
    HorizontalPx(f64),
    /// Exactly here: a fractional top line and a horizontal offset, both at once.
    ///
    /// What puts a view back where a session left it.
    ToExact {
        /// The line at the top, fractionally.
        line: f64,
        /// How far the text is scrolled left, in device pixels.
        x_px: f64,
    },
    /// Until the primary caret is inside the viewport.
    EnsureCursorVisible,
}

/// Which clipboard a copy or paste talks to.
///
/// Mirrors the framework's `ClipboardKind` without naming it, so the command vocabulary stays
/// window-free and testable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Clipboard {
    /// The desktop clipboard.
    #[default]
    Standard,
    /// The primary selection, where the platform has one; a no-op elsewhere.
    Primary,
}

/// One thing done to the editor.
#[derive(Clone, Debug)]
pub enum Command {
    /// Moves every selection through `motion`, `count` times; `extend` keeps the anchors.
    Move {
        /// The movement.
        motion: Motion,
        /// How many times it applies.
        count: u32,
        /// Whether anchors stay put, turning movement into selection.
        extend: bool,
    },
    /// Replaces the selections outright.
    SetSelections {
        /// The new selections, in any order.
        selections: Vec<Selection>,
        /// Which of them is primary, by index into `selections`.
        primary: usize,
    },
    /// Selects the whole buffer.
    SelectAll,
    /// Selects the word under each caret.
    SelectWord,
    /// Selects `count` whole lines from each selection.
    SelectLines {
        /// How many lines.
        count: u32,
    },
    /// Collapses each selection to its head.
    CollapseToHead,
    /// Collapses each selection to its anchor.
    CollapseToAnchor,
    /// Swaps each selection's ends — vim's `o` in visual mode.
    SwapHeadAnchor,
    /// Inserts at every selection, replacing what is selected.
    Insert(String),
    /// Inserts a line break, copying the leading whitespace of the line above when the
    /// configuration says to.
    InsertNewline,
    /// Deletes the selection, or one grapheme back from each caret.
    Backspace,
    /// Deletes the selection, or one grapheme forward from each caret.
    DeleteForward,
    /// Deletes what is selected, leaving carets.
    DeleteSelection,
    /// Deletes from each selection through `motion` — vim's `d{motion}` and `dd`.
    DeleteMotion {
        /// The movement that spans the deletion.
        motion: Motion,
        /// How many times it applies.
        count: u32,
        /// Whether whole lines are taken.
        linewise: bool,
    },
    /// Replaces the given byte ranges, the raw escape hatch completion and refactors use.
    ReplaceRanges(Vec<(std::ops::Range<usize>, String)>),
    /// Replaces the given byte ranges and places the selections, as one undo step.
    ///
    /// The ranges address the text before the change. The selections address the text after it.
    /// With no selections, they map through the change as they do for
    /// [`ReplaceRanges`](Command::ReplaceRanges).
    Edit {
        /// The ranges and their new text.
        replacements: Vec<(std::ops::Range<usize>, String)>,
        /// Where the selections land, in the changed text.
        selections: Option<Vec<Selection>>,
        /// Which selection is primary, by index into `selections`.
        primary: usize,
        /// What kind of change it is, for coalescing and for listeners.
        kind: crate::core::edit::EditKind,
    },
    /// Inserts one level of indentation at every selection.
    ///
    /// The level is [`EditOptions::indent`](crate::EditOptions::indent), or a tab when
    /// [`EditOptions::hard_tabs`](crate::EditOptions::hard_tabs) is set.
    InsertIndent,
    /// Indents (or dedents) every line a selection touches.
    IndentLines {
        /// Whether to remove a level instead of adding one.
        dedent: bool,
    },
    /// Undoes the last step.
    Undo,
    /// Redoes the last undone step.
    Redo,
    /// Copies the selections.
    Copy(Clipboard),
    /// Copies the selections and deletes them.
    Cut(Clipboard),
    /// Pastes over the selections.
    Paste(Clipboard),
    /// Inserts `text` at the given point; `linewise` opens its own line — vim's `p`/`P` with a
    /// linewise register.
    InsertAt {
        /// Where the text lands.
        at: InsertPoint,
        /// The text.
        text: String,
        /// Whether the text takes whole lines of its own.
        linewise: bool,
    },
    /// Moves the view.
    Scroll(ScrollCmd),
}
