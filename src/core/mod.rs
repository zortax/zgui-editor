//! The editor's model: the buffer, the selections, the history, and the one function that
//! changes them.
//!
//! [`EditorState::apply`] is the whole write API. Every keystroke, mouse action, vim command and
//! programmatic edit becomes a [`Command`] and goes through it, so invariants — grapheme-aligned
//! selections, disjoint edits, history recording — are enforced in exactly one place, and the
//! model tests without a window.

pub mod buffer;
pub mod edit;
pub mod history;
pub mod motion;
pub mod position;
pub mod search;
pub mod selection;
pub mod words;

use std::ops::Range;
use std::sync::Arc;

use ropey::Rope;

use crate::command::{Clipboard, Command, InsertPoint, ScrollCmd};
use crate::core::buffer::Buffer;
use crate::core::edit::{Edit, EditKind, TextChange, Transaction};
use crate::core::history::History;
use crate::core::motion::MotionContext;
use crate::core::selection::{Selection, Selections};

/// What editing does beyond what the keys say.
#[derive(Clone, Debug)]
pub struct EditOptions {
    /// Whether a new line copies the leading whitespace of the line it left.
    pub auto_indent: bool,
    /// One level of indentation, as text.
    pub indent: String,
    /// Whether the buffer refuses changes.
    pub read_only: bool,
}

impl Default for EditOptions {
    fn default() -> Self {
        Self {
            auto_indent: true,
            indent: "    ".to_string(),
            read_only: false,
        }
    }
}

/// What the view does after a command, beyond repainting.
#[derive(Clone, Copy, Debug)]
pub enum ScrollEffect {
    /// Bring the primary caret into view.
    EnsureVisible,
    /// The explicit request a scroll command carried.
    Command(ScrollCmd),
}

/// What one applied command changed about the text.
#[derive(Clone, Debug)]
pub struct ChangeInfo {
    /// The first line whose content or position may differ, for cache invalidation.
    pub first_changed_line: usize,
    /// The edits as tree-sitter takes them, in the order they were applied.
    pub input_edits: Vec<tree_sitter::InputEdit>,
    /// The same edits as an application takes them, in the same order.
    ///
    /// Shared rather than owned because every view attached to the document is told about the
    /// change and the report goes out to the application beside them; one allocation serves all
    /// of them, and a keystroke's report should cost a refcount.
    pub changes: Arc<[TextChange]>,
    /// What kind of change it was.
    pub kind: EditKind,
    /// Whether the whole text was replaced rather than edited.
    ///
    /// A replacement is not something anything holding a position can be mapped through, so
    /// everything addressed in the old text — another view's carets, a language server's copy —
    /// starts again rather than moving.
    pub whole_text: bool,
}

/// What applying a command asks the view to do.
#[derive(Debug, Default)]
pub struct Response {
    /// The text changed.
    pub change: Option<ChangeInfo>,
    /// The selections changed.
    pub selection_changed: bool,
    /// The view should move.
    pub scroll: Option<ScrollEffect>,
    /// Text to put on this clipboard.
    pub copied: Option<(Clipboard, String)>,
    /// A paste was asked for; the view reads this clipboard and issues the insertion.
    pub paste_requested: Option<Clipboard>,
}

impl Response {
    fn nothing() -> Self {
        Self::default()
    }

    fn selection() -> Self {
        Self {
            selection_changed: true,
            scroll: Some(ScrollEffect::EnsureVisible),
            ..Self::default()
        }
    }
}

/// The text and its history: everything about a buffer that does not belong to one view of it.
///
/// A document is shared. Two windows onto the same file hold one of these between them, so an
/// edit made in either is the same edit, undone by either, and neither can drift from the other.
/// What is *not* here is what a view owns alone: where its carets are, where it is scrolled to,
/// how it is themed.
#[derive(Debug)]
pub struct DocumentState {
    /// The text.
    pub buffer: Buffer,
    /// The undo history, which is the document's rather than any view's: undoing in one window
    /// undoes the change, not that window's share of it.
    pub history: History,
    /// The options editing follows.
    pub options: EditOptions,
}

impl DocumentState {
    /// A document over `text`.
    pub fn new(text: &str) -> Self {
        Self {
            buffer: Buffer::from_str(text),
            history: History::new(),
            options: EditOptions::default(),
        }
    }

    /// A document over `text` that already has `history` behind it.
    ///
    /// What a restored session builds. A document made empty and then filled through
    /// [`EditorState::set_text`] would have its history cleared by the filling, which is exactly
    /// right for opening a file and exactly wrong for putting one back.
    pub fn restore(text: &str, history: History) -> Self {
        Self {
            buffer: Buffer::from_str(text),
            history,
            options: EditOptions::default(),
        }
    }

    /// A consistent snapshot for a worker: the rope (an O(1) clone) and its revision.
    pub fn snapshot(&self) -> (Rope, u64) {
        (self.buffer.rope().clone(), self.buffer.revision())
    }
}

/// One view's write access to a document: the text everyone shares, and the carets only this
/// view moves.
///
/// Borrowed rather than owned, and built for the length of one command. Every invariant the model
/// has — grapheme-aligned selections, disjoint edits, history recording — is enforced through
/// [`apply`](EditorState::apply), and a command applies to exactly one view's selections however
/// many views the document has.
#[derive(Debug)]
pub struct EditorState<'a> {
    /// The text, the history and the options.
    pub doc: &'a mut DocumentState,
    /// This view's selections. Always at least one, always grapheme-aligned, always disjoint.
    pub selections: &'a mut Selections,
}

impl EditorState<'_> {
    /// Replaces the whole text, clearing history and selections, as opening a file does.
    pub fn set_text(&mut self, text: &str) -> Response {
        self.doc.buffer.set_text(text);
        *self.selections = Selections::caret(0);
        self.doc.history = History::new();
        Response {
            change: Some(ChangeInfo {
                first_changed_line: 0,
                input_edits: Vec::new(),
                // A whole new text is not a list of edits anybody can apply on top of the old
                // one; a consumer that synchronises incrementally has to resend the document.
                changes: Arc::from([] as [TextChange; 0]),
                kind: EditKind::Other,
                whole_text: true,
            }),
            selection_changed: true,
            scroll: Some(ScrollEffect::Command(ScrollCmd::ToLine(0))),
            ..Response::default()
        }
    }

    /// Applies `command` and says what it did.
    pub fn apply(&mut self, command: &Command, context: MotionContext) -> Response {
        match command {
            Command::Move {
                motion,
                count,
                extend,
            } => {
                self.doc.history.seal();
                self.selections.map(|selection| {
                    motion::apply(
                        self.doc.buffer.rope(),
                        selection,
                        *motion,
                        *count,
                        *extend,
                        context,
                    )
                });
                Response::selection()
            }
            Command::SetSelections {
                selections,
                primary,
            } => {
                self.doc.history.seal();
                let rope = self.doc.buffer.rope().clone();
                let snapped: Vec<Selection> = selections
                    .iter()
                    .map(|selection| Selection {
                        anchor: position::snap(&rope, selection.anchor),
                        head: position::snap(&rope, selection.head),
                        ..*selection
                    })
                    .collect();
                self.selections.set(snapped, *primary);
                Response {
                    selection_changed: true,
                    ..Response::default()
                }
            }
            Command::SelectAll => {
                self.doc.history.seal();
                self.selections
                    .set_one(Selection::new(0, self.doc.buffer.len_bytes()));
                Response {
                    selection_changed: true,
                    ..Response::default()
                }
            }
            Command::SelectWord => {
                self.doc.history.seal();
                let rope = self.doc.buffer.rope().clone();
                self.selections.map(|selection| {
                    let word = words::word_at(&rope, selection.head);
                    Selection::new(word.start, word.end)
                });
                Response::selection()
            }
            Command::SelectLines { count } => {
                self.doc.history.seal();
                let rope = self.doc.buffer.rope().clone();
                let count = (*count).max(1) as usize;
                self.selections.map(|selection| {
                    let from = position::line_of(&rope, selection.start());
                    // A selection ending exactly at a line's start has not touched that line.
                    let end_at = selection.end();
                    let end_line = {
                        let line = position::line_of(&rope, end_at);
                        if !selection.is_caret()
                            && line > from
                            && end_at == position::line_start(&rope, line)
                        {
                            line - 1
                        } else {
                            line
                        }
                    };
                    let to = end_line + count - 1;
                    let last = position::line_count(&rope).saturating_sub(1);
                    let end = if to >= last {
                        rope.len_bytes()
                    } else {
                        position::line_start(&rope, to + 1)
                    };
                    Selection::new(position::line_start(&rope, from), end)
                });
                Response::selection()
            }
            Command::CollapseToHead => {
                self.doc.history.seal();
                self.selections.map(|selection| selection.collapsed());
                Response::selection()
            }
            Command::CollapseToAnchor => {
                self.doc.history.seal();
                self.selections
                    .map(|selection| Selection::caret(selection.anchor));
                Response::selection()
            }
            Command::SwapHeadAnchor => {
                self.doc.history.seal();
                self.selections.map(|selection| Selection {
                    anchor: selection.head,
                    head: selection.anchor,
                    ..selection
                });
                Response::selection()
            }
            Command::Insert(text) => self.replace_selections(|_, _| text.clone(), EditKind::Typing),
            Command::InsertNewline => {
                let rope = self.doc.buffer.rope().clone();
                let auto_indent = self.doc.options.auto_indent;
                self.replace_selections(
                    move |_, selection: &Selection| {
                        if !auto_indent {
                            return "\n".to_string();
                        }
                        let line = position::line_of(&rope, selection.start());
                        let text = position::line_text(&rope, line);
                        let lead: String = text
                            .chars()
                            .take_while(|character| *character == ' ' || *character == '\t')
                            .collect();
                        format!("\n{lead}")
                    },
                    EditKind::Newline,
                )
            }
            Command::Backspace => {
                let rope = self.doc.buffer.rope().clone();
                let ranges = self
                    .selections
                    .iter()
                    .map(|selection| {
                        if selection.is_caret() {
                            let from = position::prev_grapheme(&rope, selection.head);
                            (from..selection.head, String::new())
                        } else {
                            (selection.range(), String::new())
                        }
                    })
                    .collect();
                self.edit(ranges, EditKind::Deletion, None)
            }
            Command::DeleteForward => {
                let rope = self.doc.buffer.rope().clone();
                let ranges = self
                    .selections
                    .iter()
                    .map(|selection| {
                        if selection.is_caret() {
                            let to = position::next_grapheme(&rope, selection.head);
                            (selection.head..to, String::new())
                        } else {
                            (selection.range(), String::new())
                        }
                    })
                    .collect();
                self.edit(ranges, EditKind::Deletion, None)
            }
            Command::DeleteSelection => {
                let ranges = self
                    .selections
                    .iter()
                    .map(|selection| (selection.range(), String::new()))
                    .collect();
                self.edit(ranges, EditKind::Other, None)
            }
            Command::DeleteMotion {
                motion,
                count,
                linewise,
            } => {
                let rope = self.doc.buffer.rope().clone();
                let ranges: Vec<(Range<usize>, String)> = self
                    .selections
                    .iter()
                    .map(|selection| {
                        let range = motion::motion_range(
                            &rope, *selection, *motion, *count, *linewise, context,
                        );
                        (range, String::new())
                    })
                    .collect();
                let carets = linewise.then(|| {
                    ranges
                        .iter()
                        .map(|(range, _)| range.start)
                        .collect::<Vec<_>>()
                });
                let mut response = self.edit(ranges, EditKind::Other, carets);
                if *linewise && response.change.is_some() {
                    // The caret lands on the first non-blank of the line that moved up.
                    let rope = self.doc.buffer.rope().clone();
                    self.selections.map(|selection| {
                        let line = position::line_of(&rope, selection.head);
                        Selection::caret(motion::first_non_blank(&rope, line))
                    });
                    response.selection_changed = true;
                }
                response
            }
            Command::ReplaceRanges(replacements) => {
                self.edit(replacements.clone(), EditKind::Other, None)
            }
            Command::IndentLines { dedent } => self.indent(*dedent),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Copy(clipboard) => Response {
                copied: Some((*clipboard, self.selected_text())),
                ..Response::default()
            },
            Command::Cut(clipboard) => {
                let text = self.selected_text();
                let ranges = self
                    .selections
                    .iter()
                    .map(|selection| (selection.range(), String::new()))
                    .collect();
                let mut response = self.edit(ranges, EditKind::Other, None);
                response.copied = Some((*clipboard, text));
                response
            }
            Command::Paste(clipboard) => Response {
                paste_requested: Some(*clipboard),
                ..Response::default()
            },
            Command::InsertAt { at, text, linewise } => self.insert_at(*at, text, *linewise),
            Command::Scroll(cmd) => Response {
                scroll: Some(ScrollEffect::Command(*cmd)),
                ..Response::default()
            },
        }
    }

    /// The selected text, one selection per line break; carets contribute their whole line.
    pub fn selected_text(&self) -> String {
        let rope = self.doc.buffer.rope();
        let mut parts: Vec<String> = Vec::new();
        for selection in self.selections.iter() {
            if selection.is_caret() {
                let line = position::line_of(rope, selection.head);
                let start = position::line_start(rope, line);
                let end = if line + 1 >= position::line_count(rope) {
                    rope.len_bytes()
                } else {
                    position::line_start(rope, line + 1)
                };
                parts.push(rope.byte_slice(start..end).to_string());
            } else {
                parts.push(rope.byte_slice(selection.range()).to_string());
            }
        }
        parts.join(if parts.len() > 1 { "\n" } else { "" })
    }

    /// Replaces every selection with what `text` says for it.
    fn replace_selections(
        &mut self,
        text: impl Fn(usize, &Selection) -> String,
        kind: EditKind,
    ) -> Response {
        let ranges = self
            .selections
            .iter()
            .enumerate()
            .map(|(index, selection)| (selection.range(), text(index, selection)))
            .collect();
        self.edit(ranges, kind, None)
    }

    /// Replaces `replacements` as the owner of the text, and moves the selections.
    ///
    /// The change goes through a read-only buffer and records no undo step. The history then
    /// addresses text that has moved, so it starts empty. This is how a buffer that shows
    /// something written elsewhere, such as a log, takes new text while people only read it.
    pub fn write(&mut self, replacements: Vec<(Range<usize>, String)>) -> Response {
        let response = self.replace(replacements, EditKind::Other, None, false);
        if response.change.is_some() {
            self.doc.history = History::new();
        }
        response
    }

    /// Applies `replacements` as one transaction, records it, and moves the selections.
    ///
    /// Selections map through the edits by default — a caret inside a replaced range lands after
    /// the replacement, which is where typing wants it. `carets` overrides that with explicit
    /// pre-edit positions, each mapped through the transaction.
    fn edit(
        &mut self,
        replacements: Vec<(Range<usize>, String)>,
        kind: EditKind,
        carets: Option<Vec<usize>>,
    ) -> Response {
        if self.doc.options.read_only {
            return Response::nothing();
        }
        self.replace(replacements, kind, carets, true)
    }

    /// Applies `replacements` as one transaction and moves the selections. `record` says whether
    /// the transaction goes into the history.
    fn replace(
        &mut self,
        mut replacements: Vec<(Range<usize>, String)>,
        kind: EditKind,
        carets: Option<Vec<usize>>,
        record: bool,
    ) -> Response {
        replacements.sort_by_key(|(range, _)| range.start);
        replacements.dedup_by(|later, earlier| {
            // Overlapping requests are a caller error; the earlier one wins.
            later.0.start < earlier.0.end
        });
        if replacements
            .iter()
            .all(|(range, text)| range.start == range.end && text.is_empty())
        {
            return Response::nothing();
        }

        let before_rope = self.doc.buffer.rope().clone();
        let before = self.selections.clone();
        let edits: Vec<Edit> = replacements
            .iter()
            .rev()
            .map(|(range, text)| Edit {
                range: range.clone(),
                inserted: text.clone(),
                deleted: before_rope.byte_slice(range.clone()).to_string(),
            })
            .collect();

        let mut tx = Transaction::new(
            edits,
            before.clone(),
            before.clone(),
            kind,
            crate::core::edit::EditTime::now(),
        );
        self.doc.buffer.apply(&tx.edits);

        // Where the selections land: mapped through the edits, or where the caller said.
        match carets {
            Some(carets) => {
                let mapped: Vec<Selection> = carets
                    .iter()
                    .map(|at| Selection::caret(tx.map_before(*at)))
                    .collect();
                self.selections.set(mapped, 0);
            }
            None => {
                self.selections.map(|selection| Selection {
                    anchor: tx.map(selection.anchor),
                    head: tx.map(selection.head),
                    affinity: selection.affinity,
                    goal_col: None,
                });
            }
        }
        tx.after = self.selections.clone();

        let first_changed_line = tx
            .edits
            .last()
            .map(|edit| position::line_of(&before_rope, edit.range.start))
            .unwrap_or(0);
        let input_edits = tx.input_edits(&before_rope);
        let changes: Arc<[TextChange]> = Arc::from(tx.changes());
        if record {
            self.doc.history.push(tx);
        }

        Response {
            change: Some(ChangeInfo {
                first_changed_line,
                input_edits,
                changes,
                kind,
                whole_text: false,
            }),
            selection_changed: true,
            scroll: Some(ScrollEffect::EnsureVisible),
            ..Response::default()
        }
    }

    /// Indents or dedents every line a selection touches.
    fn indent(&mut self, dedent: bool) -> Response {
        let rope = self.doc.buffer.rope().clone();
        let indent = self.doc.options.indent.clone();
        let mut lines: Vec<usize> = Vec::new();
        for selection in self.selections.iter() {
            let from = position::line_of(&rope, selection.start());
            // A selection ending exactly at a line start does not touch that line.
            let end = selection.end();
            let to_line = position::line_of(&rope, end);
            let to = if !selection.is_caret()
                && end == position::line_start(&rope, to_line)
                && to_line > from
            {
                to_line - 1
            } else {
                to_line
            };
            for line in from..=to {
                if lines.last() != Some(&line) {
                    lines.push(line);
                }
            }
        }
        let replacements: Vec<(Range<usize>, String)> = lines
            .into_iter()
            .filter_map(|line| {
                let start = position::line_start(&rope, line);
                if dedent {
                    let text = position::line_text(&rope, line);
                    let mut take = 0;
                    for character in text.chars() {
                        if character == '\t' {
                            take += 1;
                            break;
                        }
                        if character == ' ' && take < indent.len() {
                            take += 1;
                        } else {
                            break;
                        }
                    }
                    (take > 0).then(|| (start..start + take, String::new()))
                } else {
                    Some((start..start, indent.clone()))
                }
            })
            .collect();
        if replacements.is_empty() {
            return Response::nothing();
        }
        self.edit(replacements, EditKind::Other, None)
    }

    /// Inserts supplied text — a paste, a register — where `at` and `linewise` say.
    fn insert_at(&mut self, at: InsertPoint, text: &str, linewise: bool) -> Response {
        let rope = self.doc.buffer.rope().clone();
        if linewise {
            // The text takes whole lines of its own, above (P) or below (p) each caret's line.
            let mut block = text.to_string();
            if !block.ends_with('\n') {
                block.push('\n');
            }
            let replacements: Vec<(Range<usize>, String)> = self
                .selections
                .iter()
                .map(|selection| {
                    let line = position::line_of(&rope, selection.head);
                    match at {
                        InsertPoint::AtCarets => {
                            let start = position::line_start(&rope, line);
                            (start..start, block.clone())
                        }
                        InsertPoint::AfterCarets => {
                            if line + 1 >= position::line_count(&rope) {
                                // Below the last line: the break comes first instead of last.
                                let end = rope.len_bytes();
                                let mut owned = String::with_capacity(block.len() + 1);
                                owned.push('\n');
                                owned.push_str(block.trim_end_matches('\n'));
                                (end..end, owned)
                            } else {
                                let start = position::line_start(&rope, line + 1);
                                (start..start, block.clone())
                            }
                        }
                    }
                })
                .collect();
            let carets: Vec<usize> = replacements.iter().map(|(range, _)| range.start).collect();
            let response = self.edit(replacements, EditKind::Paste, Some(carets));
            if response.change.is_some() {
                // The caret belongs on the first non-blank of the first pasted line.
                let rope = self.doc.buffer.rope().clone();
                self.selections.map(|selection| {
                    let line = position::line_of(&rope, selection.head);
                    Selection::caret(motion::first_non_blank(&rope, line))
                });
            }
            response
        } else {
            let replacements: Vec<(Range<usize>, String)> = self
                .selections
                .iter()
                .map(|selection| match at {
                    InsertPoint::AtCarets => (selection.range(), text.to_string()),
                    InsertPoint::AfterCarets => {
                        let from = if selection.is_caret() {
                            position::next_grapheme(&rope, selection.head)
                                .min(position::line_end(
                                    &rope,
                                    position::line_of(&rope, selection.head),
                                ))
                                .max(selection.head)
                        } else {
                            selection.end()
                        };
                        (from..from, text.to_string())
                    }
                })
                .collect();
            self.edit(replacements, EditKind::Paste, None)
        }
    }

    /// Undoes one step.
    fn undo(&mut self) -> Response {
        let Some(step) = self.doc.history.undo() else {
            return Response::nothing();
        };
        let mut input_edits = Vec::new();
        let mut changes: Vec<TextChange> = Vec::new();
        let mut first_changed_line = usize::MAX;
        for tx in step.transactions.iter().rev() {
            let inverted = tx.invert();
            let before_rope = self.doc.buffer.rope().clone();
            input_edits.extend(inverted.input_edits(&before_rope));
            changes.extend(inverted.changes());
            if let Some(edit) = inverted.edits.last() {
                first_changed_line =
                    first_changed_line.min(position::line_of(&before_rope, edit.range.start));
            }
            self.doc.buffer.apply(&inverted.edits);
        }
        *self.selections = step
            .transactions
            .first()
            .map(|tx| tx.before.clone())
            .unwrap_or_else(|| Selections::caret(0));
        Response {
            change: Some(ChangeInfo {
                first_changed_line: if first_changed_line == usize::MAX {
                    0
                } else {
                    first_changed_line
                },
                input_edits,
                changes: Arc::from(changes),
                kind: EditKind::Other,
                whole_text: false,
            }),
            selection_changed: true,
            scroll: Some(ScrollEffect::EnsureVisible),
            ..Response::default()
        }
    }

    /// Redoes one undone step.
    fn redo(&mut self) -> Response {
        let Some(step) = self.doc.history.redo() else {
            return Response::nothing();
        };
        let mut input_edits = Vec::new();
        let mut changes: Vec<TextChange> = Vec::new();
        let mut first_changed_line = usize::MAX;
        for tx in &step.transactions {
            let before_rope = self.doc.buffer.rope().clone();
            input_edits.extend(tx.input_edits(&before_rope));
            changes.extend(tx.changes());
            if let Some(edit) = tx.edits.last() {
                first_changed_line =
                    first_changed_line.min(position::line_of(&before_rope, edit.range.start));
            }
            self.doc.buffer.apply(&tx.edits);
        }
        *self.selections = step
            .transactions
            .last()
            .map(|tx| tx.after.clone())
            .unwrap_or_else(|| Selections::caret(0));
        Response {
            change: Some(ChangeInfo {
                first_changed_line: if first_changed_line == usize::MAX {
                    0
                } else {
                    first_changed_line
                },
                input_edits,
                changes: Arc::from(changes),
                kind: EditKind::Other,
                whole_text: false,
            }),
            selection_changed: true,
            scroll: Some(ScrollEffect::EnsureVisible),
            ..Response::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Motion;

    const NO_VIEW: MotionContext = MotionContext { viewport_lines: 10 };

    /// A document and the one view of it these tests act through.
    ///
    /// The two are separate values in the model, because several views share one document. A test
    /// holds both and pairs them for the length of a command, which is exactly what the component
    /// does.
    struct Editor {
        doc: DocumentState,
        selections: Selections,
    }

    impl Editor {
        fn state(&mut self) -> EditorState<'_> {
            EditorState {
                doc: &mut self.doc,
                selections: &mut self.selections,
            }
        }

        fn text(&self) -> String {
            self.doc.buffer.rope().to_string()
        }
    }

    fn editor(text: &str) -> Editor {
        Editor {
            doc: DocumentState::new(text),
            selections: Selections::caret(0),
        }
    }

    fn apply(editor: &mut Editor, command: Command) -> Response {
        editor.state().apply(&command, NO_VIEW)
    }

    #[test]
    fn a_change_reports_what_it_replaced() {
        // The report is what a language server synchronises from, so it has to describe the edit
        // in the coordinates of the text the edit applied to — not the text that came out.
        let mut editor = editor("hello world");
        apply(
            &mut editor,
            Command::Move {
                motion: Motion::WordForward { big: false },
                count: 1,
                extend: false,
            },
        );
        let response = apply(&mut editor, Command::Insert("brave ".to_string()));
        let change = response.change.expect("the text changed");
        assert_eq!(change.changes.len(), 1);
        assert_eq!(change.changes[0].range, 6..6);
        assert_eq!(change.changes[0].text, "brave ");
        assert_eq!(editor.text(), "hello brave world");
    }

    #[test]
    fn a_deletion_reports_the_bytes_it_took() {
        let mut editor = editor("hello world");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::new(0, 6)],
                primary: 0,
            },
        );
        let response = apply(&mut editor, Command::DeleteSelection);
        let change = response.change.expect("the text changed");
        assert_eq!(change.changes.len(), 1);
        assert_eq!(change.changes[0].range, 0..6);
        assert_eq!(change.changes[0].text, "");
    }

    #[test]
    fn undo_reports_the_change_that_puts_the_text_back() {
        let mut editor = editor("abc");
        apply(&mut editor, Command::Insert("X".to_string()));
        let response = apply(&mut editor, Command::Undo);
        let change = response.change.expect("undo changed the text");
        assert_eq!(change.changes.len(), 1);
        // Addressed in the changed text: the X that was inserted is what comes back out.
        assert_eq!(change.changes[0].range, 0..1);
        assert_eq!(change.changes[0].text, "");
        assert_eq!(editor.text(), "abc");
    }

    #[test]
    fn several_carets_report_several_changes_in_the_order_they_applied() {
        // Descending by start, which is the order that needs no offset fixing on the far side.
        let mut editor = editor("a\nb\n");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::caret(0), Selection::caret(2)],
                primary: 0,
            },
        );
        let response = apply(&mut editor, Command::Insert(">".to_string()));
        let change = response.change.expect("the text changed");
        let starts: Vec<usize> = change.changes.iter().map(|c| c.range.start).collect();
        assert_eq!(starts, [2, 0]);
        assert_eq!(editor.text(), ">a\n>b\n");
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut editor = editor("world");
        apply(&mut editor, Command::Insert("hello ".to_string()));
        assert_eq!(editor.doc.buffer.to_string(), "hello world");
        assert_eq!(editor.selections.primary().head, 6);
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut editor = editor("hello world");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::new(0, 5)],
                primary: 0,
            },
        );
        apply(&mut editor, Command::Insert("goodbye".to_string()));
        assert_eq!(editor.doc.buffer.to_string(), "goodbye world");
        assert_eq!(editor.selections.primary().head, 7);
        assert!(editor.selections.primary().is_caret());
    }

    #[test]
    fn multi_cursor_typing_hits_every_caret() {
        let mut editor = editor("a\nb\nc");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![
                    Selection::caret(0),
                    Selection::caret(2),
                    Selection::caret(4),
                ],
                primary: 0,
            },
        );
        apply(&mut editor, Command::Insert("x".to_string()));
        assert_eq!(editor.doc.buffer.to_string(), "xa\nxb\nxc");
        let heads: Vec<usize> = editor.selections.iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![1, 4, 7]);
    }

    #[test]
    fn backspace_deletes_a_grapheme_or_the_selection() {
        let mut editor = editor("ab");
        apply(
            &mut editor,
            Command::Move {
                motion: Motion::Right,
                count: 2,
                extend: false,
            },
        );
        apply(&mut editor, Command::Backspace);
        assert_eq!(editor.doc.buffer.to_string(), "a");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::new(0, 1)],
                primary: 0,
            },
        );
        apply(&mut editor, Command::Backspace);
        assert_eq!(editor.doc.buffer.to_string(), "");
    }

    #[test]
    fn newline_copies_the_indent() {
        let mut editor = editor("    code");
        apply(
            &mut editor,
            Command::Move {
                motion: Motion::LineEnd,
                count: 1,
                extend: false,
            },
        );
        apply(&mut editor, Command::InsertNewline);
        assert_eq!(editor.doc.buffer.to_string(), "    code\n    ");
    }

    #[test]
    fn undo_restores_text_and_selection() {
        let mut editor = editor("one");
        apply(&mut editor, Command::Insert("x".to_string()));
        assert_eq!(editor.doc.buffer.to_string(), "xone");
        apply(&mut editor, Command::Undo);
        assert_eq!(editor.doc.buffer.to_string(), "one");
        assert_eq!(editor.selections.primary().head, 0);
        apply(&mut editor, Command::Redo);
        assert_eq!(editor.doc.buffer.to_string(), "xone");
        assert_eq!(editor.selections.primary().head, 1);
    }

    #[test]
    fn coalesced_typing_undoes_as_one() {
        let mut editor = editor("");
        apply(&mut editor, Command::Insert("a".to_string()));
        apply(&mut editor, Command::Insert("b".to_string()));
        apply(&mut editor, Command::Insert("c".to_string()));
        assert_eq!(editor.doc.buffer.to_string(), "abc");
        apply(&mut editor, Command::Undo);
        assert_eq!(editor.doc.buffer.to_string(), "");
    }

    #[test]
    fn a_motion_seals_the_undo_step() {
        let mut editor = editor("");
        apply(&mut editor, Command::Insert("a".to_string()));
        apply(
            &mut editor,
            Command::Move {
                motion: Motion::Left,
                count: 1,
                extend: false,
            },
        );
        apply(&mut editor, Command::Insert("b".to_string()));
        apply(&mut editor, Command::Undo);
        assert_eq!(
            editor.doc.buffer.to_string(),
            "a",
            "only the second burst undoes"
        );
    }

    #[test]
    fn delete_motion_linewise_takes_whole_lines() {
        let mut editor = editor("one\ntwo\nthree");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::caret(5)],
                primary: 0,
            },
        );
        apply(
            &mut editor,
            Command::DeleteMotion {
                motion: Motion::Down,
                count: 1,
                linewise: true,
            },
        );
        assert_eq!(editor.doc.buffer.to_string(), "one\nthree");
        assert_eq!(editor.selections.primary().head, 4);
    }

    #[test]
    fn linewise_paste_opens_its_own_line() {
        let mut editor = editor("one\ntwo");
        apply(
            &mut editor,
            Command::InsertAt {
                at: InsertPoint::AfterCarets,
                text: "new".to_string(),
                linewise: true,
            },
        );
        assert_eq!(editor.doc.buffer.to_string(), "one\nnew\ntwo");
        assert_eq!(editor.selections.primary().head, 4);
    }

    #[test]
    fn linewise_paste_below_the_last_line() {
        let mut editor = editor("one");
        apply(
            &mut editor,
            Command::InsertAt {
                at: InsertPoint::AfterCarets,
                text: "new".to_string(),
                linewise: true,
            },
        );
        assert_eq!(editor.doc.buffer.to_string(), "one\nnew");
    }

    #[test]
    fn indent_and_dedent_move_selected_lines() {
        let mut editor = editor("one\ntwo");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::new(0, 6)],
                primary: 0,
            },
        );
        apply(&mut editor, Command::IndentLines { dedent: false });
        assert_eq!(editor.doc.buffer.to_string(), "    one\n    two");
        apply(&mut editor, Command::IndentLines { dedent: true });
        assert_eq!(editor.doc.buffer.to_string(), "one\ntwo");
    }

    #[test]
    fn a_write_goes_through_a_read_only_buffer_and_records_nothing() {
        let mut editor = editor("one\n");
        apply(&mut editor, Command::Insert("x".to_string()));
        editor.doc.options.read_only = true;
        let response = editor.state().write(vec![(5..5, "two\n".to_string())]);
        assert!(response.change.is_some());
        assert_eq!(editor.doc.buffer.to_string(), "xone\ntwo\n");
        let undone = apply(&mut editor, Command::Undo);
        assert!(
            undone.change.is_none(),
            "the history starts over after a write"
        );
        assert_eq!(editor.doc.buffer.to_string(), "xone\ntwo\n");
    }

    #[test]
    fn read_only_refuses_edits() {
        let mut editor = editor("text");
        editor.doc.options.read_only = true;
        let response = apply(&mut editor, Command::Insert("x".to_string()));
        assert!(response.change.is_none());
        assert_eq!(editor.doc.buffer.to_string(), "text");
    }

    #[test]
    fn cut_reports_what_it_took() {
        let mut editor = editor("hello world");
        apply(
            &mut editor,
            Command::SetSelections {
                selections: vec![Selection::new(0, 5)],
                primary: 0,
            },
        );
        let response = apply(&mut editor, Command::Cut(Clipboard::Standard));
        assert_eq!(
            response.copied,
            Some((Clipboard::Standard, "hello".to_string()))
        );
        assert_eq!(editor.doc.buffer.to_string(), " world");
    }

    #[test]
    fn copy_with_a_caret_takes_the_line() {
        let mut editor = editor("one\ntwo");
        let response = apply(&mut editor, Command::Copy(Clipboard::Standard));
        assert_eq!(
            response.copied,
            Some((Clipboard::Standard, "one\n".to_string()))
        );
    }
}
