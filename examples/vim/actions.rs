//! What a finished vim command is, and how it lowers to editor commands.
//!
//! Everything here drives the editor through its public [`EditorHandle`] — commands in,
//! synchronous queries out. Nothing reaches inside the component, which is the example's whole
//! claim: a modal layer is an application concern, and the editor's API is enough for one.

use zgui_editor::{
    Command, EditorHandle, InsertPoint, Motion, ScrollCmd, SearchDirection, Selection,
};

use crate::mode::{Mode, VimState};
use crate::pending::Op;

/// One finished command.
#[derive(Clone, Debug)]
pub enum VimAction {
    /// Enter insert mode, placed by which key asked.
    EnterInsert(InsertEntry),
    /// Back to normal mode.
    EnterNormal,
    /// Enter visual mode, linewise or not.
    EnterVisual {
        /// Whether whole lines are selected.
        line: bool,
    },
    /// A motion, moving or extending by mode.
    Motion {
        /// Where to.
        motion: Motion,
        /// How many times.
        count: u32,
    },
    /// An operator aimed by a motion — `d2w`, `yy`, `c$`.
    Operator {
        /// Which operator.
        op: Op,
        /// The motion that spans it.
        motion: Motion,
        /// How many times.
        count: u32,
        /// Whether whole lines are taken.
        linewise: bool,
    },
    /// An operator over the visual selection.
    OperatorOnSelection {
        /// Which operator.
        op: Op,
    },
    /// `x` — delete under the caret.
    DeleteChar {
        /// How many.
        count: u32,
    },
    /// `p` and `P`.
    Paste {
        /// Whether the text lands after the caret.
        after: bool,
    },
    /// `u`.
    Undo,
    /// `Ctrl-r`.
    Redo,
    /// `/` — start typing a search.
    StartSearch,
    /// `n` and `N`.
    SearchNext {
        /// Whether to look the other way.
        reverse: bool,
    },
    /// `zz`.
    CenterCaret,
    /// `Ctrl-d` and `Ctrl-u`.
    HalfPage {
        /// Whether downward.
        down: bool,
    },
}

/// How insert mode was entered, which decides where the caret goes first.
#[derive(Clone, Copy, Debug)]
pub enum InsertEntry {
    /// `i` — where the caret is.
    Here,
    /// `a` — after the caret's character.
    After,
    /// `I` — at the line's first non-blank.
    LineStart,
    /// `A` — at the line's end.
    LineEnd,
    /// `o` — on a new line below.
    OpenBelow,
    /// `O` — on a new line above.
    OpenAbove,
}

/// Applies one action to the editor and the layer's own state.
pub fn perform(state: &mut VimState, action: VimAction, handle: &EditorHandle) {
    match action {
        VimAction::EnterInsert(entry) => {
            match entry {
                InsertEntry::Here => {}
                InsertEntry::After => handle.command(Command::Move {
                    motion: Motion::Right,
                    count: 1,
                    extend: false,
                }),
                InsertEntry::LineStart => handle.command(Command::Move {
                    motion: Motion::LineFirstNonBlank,
                    count: 1,
                    extend: false,
                }),
                InsertEntry::LineEnd => handle.command(Command::Move {
                    motion: Motion::LineEnd,
                    count: 1,
                    extend: false,
                }),
                InsertEntry::OpenBelow => {
                    handle.command(Command::Move {
                        motion: Motion::LineEnd,
                        count: 1,
                        extend: false,
                    });
                    handle.command(Command::InsertNewline);
                }
                InsertEntry::OpenAbove => {
                    handle.command(Command::Move {
                        motion: Motion::LineStart,
                        count: 1,
                        extend: false,
                    });
                    handle.command(Command::Insert("\n".to_string()));
                    handle.command(Command::Move {
                        motion: Motion::Up,
                        count: 1,
                        extend: false,
                    });
                }
            }
            enter_mode(state, Mode::Insert, handle);
        }
        VimAction::EnterNormal => {
            handle.command(Command::CollapseToHead);
            enter_mode(state, Mode::Normal, handle);
        }
        VimAction::EnterVisual { line } => {
            if line {
                handle.command(Command::SelectLines { count: 1 });
                enter_mode(state, Mode::VisualLine, handle);
            } else {
                enter_mode(state, Mode::Visual, handle);
            }
        }
        VimAction::Motion { motion, count } => {
            let extend = matches!(state.mode, Mode::Visual | Mode::VisualLine);
            handle.command(Command::Move {
                motion,
                count,
                extend,
            });
            if state.mode == Mode::VisualLine {
                handle.command(Command::SelectLines { count: 1 });
            }
        }
        VimAction::Operator {
            op,
            motion,
            count,
            linewise,
        } => {
            // What the motion spans, read without moving anything — the register wants the
            // text whether or not the buffer changes.
            let (range, text) = handle.query(|snapshot| {
                let range =
                    snapshot.motion_range(snapshot.selections().primary(), motion, count, linewise);
                let text = snapshot.text_in(range.clone());
                (range, text)
            });
            state.registers.store(text, linewise);
            match op {
                Op::Yank => {}
                Op::Delete => handle.command(Command::DeleteMotion {
                    motion,
                    count,
                    linewise,
                }),
                Op::Change => {
                    if linewise {
                        // `cc`: the lines go, an empty line opens in their place.
                        handle.command(Command::DeleteMotion {
                            motion,
                            count,
                            linewise,
                        });
                        handle.command(Command::Move {
                            motion: Motion::LineStart,
                            count: 1,
                            extend: false,
                        });
                        handle.command(Command::Insert("\n".to_string()));
                        handle.command(Command::Move {
                            motion: Motion::Up,
                            count: 1,
                            extend: false,
                        });
                    } else {
                        handle.command(Command::ReplaceRanges(vec![(range, String::new())]));
                    }
                    enter_mode(state, Mode::Insert, handle);
                }
            }
        }
        VimAction::OperatorOnSelection { op } => {
            let linewise = state.mode == Mode::VisualLine;
            let text =
                handle.query(|snapshot| snapshot.text_in(snapshot.selections().primary().range()));
            state
                .registers
                .store(trim_linewise(text, linewise), linewise);
            match op {
                Op::Yank => {
                    handle.command(Command::CollapseToAnchor);
                    enter_mode(state, Mode::Normal, handle);
                }
                Op::Delete => {
                    handle.command(Command::DeleteSelection);
                    enter_mode(state, Mode::Normal, handle);
                }
                Op::Change => {
                    handle.command(Command::DeleteSelection);
                    if linewise {
                        handle.command(Command::Insert("\n".to_string()));
                        handle.command(Command::Move {
                            motion: Motion::Up,
                            count: 1,
                            extend: false,
                        });
                    }
                    enter_mode(state, Mode::Insert, handle);
                }
            }
        }
        VimAction::DeleteChar { count } => {
            let text = handle.query(|snapshot| {
                let range = snapshot.motion_range(
                    snapshot.selections().primary(),
                    Motion::Right,
                    count,
                    false,
                );
                snapshot.text_in(range)
            });
            state.registers.store(text, false);
            handle.command(Command::DeleteMotion {
                motion: Motion::Right,
                count,
                linewise: false,
            });
        }
        VimAction::Paste { after } => {
            if let Some((text, linewise)) = state.registers.take() {
                handle.command(Command::InsertAt {
                    at: if after {
                        InsertPoint::AfterCarets
                    } else {
                        InsertPoint::AtCarets
                    },
                    text,
                    linewise,
                });
            }
        }
        VimAction::Undo => handle.command(Command::Undo),
        VimAction::Redo => handle.command(Command::Redo),
        VimAction::StartSearch => {
            state.search_input = Some(String::new());
        }
        VimAction::SearchNext { reverse } => {
            if let Some(needle) = state.last_search.clone() {
                search_to(handle, &needle, reverse);
            }
        }
        VimAction::CenterCaret => {
            handle.command(Command::Scroll(ScrollCmd::CursorCenter));
        }
        VimAction::HalfPage { down } => {
            let motion = if down {
                Motion::HalfPageDown
            } else {
                Motion::HalfPageUp
            };
            handle.command(Command::Move {
                motion,
                count: 1,
                extend: matches!(state.mode, Mode::Visual | Mode::VisualLine),
            });
        }
    }
}

/// Moves the caret to the next hit for `needle` and remembers nothing — the caller keeps the
/// needle.
pub fn search_to(handle: &EditorHandle, needle: &str, reverse: bool) {
    let hit = handle.query(|snapshot| {
        let from = snapshot.selections().primary().head;
        let direction = if reverse {
            SearchDirection::Backward
        } else {
            SearchDirection::Forward
        };
        let start = if reverse {
            from
        } else {
            from.saturating_add(1)
        };
        snapshot.search(needle, start, direction, true)
    });
    if let Some(range) = hit {
        handle.command(Command::SetSelections {
            selections: vec![Selection::caret(range.start)],
            primary: 0,
        });
        // Placing a selection does not scroll — a mouse click must not move the view — so a
        // search that landed off screen asks for the view explicitly, and glides there.
        handle.command(Command::Scroll(ScrollCmd::EnsureCursorVisible));
    }
}

/// Switches mode, telling the editor what the caret now looks like.
pub fn enter_mode(state: &mut VimState, mode: Mode, handle: &EditorHandle) {
    state.mode = mode;
    state.pending.clear();
    handle.set_cursor_style(match mode {
        Mode::Insert => zgui_editor::CursorStyle::Bar,
        _ => zgui_editor::CursorStyle::Block,
    });
}

/// A linewise selection carries its final break; the register convention keeps one exactly.
fn trim_linewise(text: String, linewise: bool) -> String {
    if !linewise {
        return text;
    }
    let mut text = text;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}
