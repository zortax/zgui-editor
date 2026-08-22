//! What one change to the text is, in a form that can be applied, undone, and described to a
//! parser.
//!
//! A [`Transaction`] is the unit of undo and of revision: every command that changes text builds
//! one, however many selections it acted for. Its edits are kept sorted by start **descending**,
//! because that is the order they apply in without any offset fixing — an edit changes nothing
//! before itself.

use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::core::selection::Selections;

/// When a transaction happened.
///
/// Milliseconds since the first time this crate was asked. An [`Instant`] cannot be written down
/// and means nothing across a process anyway; what the number is for is one comparison against
/// [`COALESCE_WINDOW`](crate::core::history::COALESCE_WINDOW), and a number does that.
///
/// A session restored from disk brings times from a previous run's origin. Comparing those to
/// this run's is meaningless, so [`since`](Self::since) saturates: a restored transaction is
/// never *later* than a live one, and a restored history is sealed anyway.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EditTime(pub u64);

impl EditTime {
    /// Now.
    #[must_use]
    pub fn now() -> Self {
        static ORIGIN: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
        Self(ORIGIN.elapsed().as_millis() as u64)
    }

    /// How long after `earlier` this is, or nothing when it is not after it at all.
    #[must_use]
    pub fn since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

impl std::ops::Add<Duration> for EditTime {
    type Output = Self;

    fn add(self, later: Duration) -> Self {
        Self(self.0.saturating_add(later.as_millis() as u64))
    }
}

/// One replacement: `deleted` at `range` gives way to `inserted`.
///
/// The range is in bytes of the text the edit applies to. `deleted` carries the replaced text —
/// redundantly with the buffer, but it is what makes the transaction invertible after the buffer
/// has moved on.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Edit {
    /// The bytes replaced.
    pub range: Range<usize>,
    /// What takes their place.
    pub inserted: String,
    /// What they were.
    pub deleted: String,
}

impl Edit {
    /// How this edit moves a position sitting after it.
    fn delta(&self) -> isize {
        self.inserted.len() as isize - (self.range.end - self.range.start) as isize
    }
}

/// One replacement, as somebody outside the crate is told about it.
///
/// The same shape as an [`Edit`] without the replaced text, which nothing outside needs and which
/// is only kept inside so that a transaction can be inverted. A language server's incremental
/// synchronisation is exactly this list in exactly this order, which is why the editor reports it
/// rather than leaving it to be worked out by comparing two revisions of the rope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    /// The bytes replaced, in the coordinates of the text this change applied to.
    pub range: Range<usize>,
    /// What took their place.
    pub text: String,
}

/// Where `byte` of the text before a change sits in the text after it.
///
/// The list is the one a [`ChangeInfo`](crate::core::ChangeInfo) carries: descending by start,
/// the order the replacements applied in. A position inside a replaced range lands at the end of
/// what replaced it, which is where a caret that was there belongs.
///
/// This is what a second view of a document moves its own carets through when the first one
/// edits.
pub fn map_through(changes: &[TextChange], byte: usize) -> usize {
    let mut delta = 0isize;
    for change in changes.iter().rev() {
        if byte < change.range.start {
            break;
        }
        if byte < change.range.end {
            return (change.range.start as isize + delta) as usize + change.text.len();
        }
        delta += change.text.len() as isize - (change.range.end - change.range.start) as isize;
    }
    (byte as isize + delta) as usize
}

/// What kind of change a transaction is, which is what decides whether it folds into the
/// undo step before it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum EditKind {
    /// Characters typed at the carets.
    Typing,
    /// Characters removed at the carets, as backspace and delete do.
    Deletion,
    /// A line break inserted.
    Newline,
    /// Text put in from elsewhere.
    Paste,
    /// Anything else.
    Other,
}

/// One applied (or applicable) change: the edits, the selections around them, and when.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transaction {
    /// The edits, sorted by start descending, disjoint.
    pub edits: Vec<Edit>,
    /// The selections before the change, which undo restores.
    pub before: Selections,
    /// The selections after the change, which redo restores.
    pub after: Selections,
    /// What kind of change this is.
    pub kind: EditKind,
    /// When it happened, which is what bounds undo coalescing.
    pub at: EditTime,
}

impl Transaction {
    /// A transaction over `edits` in any order; they are sorted here.
    pub fn new(
        mut edits: Vec<Edit>,
        before: Selections,
        after: Selections,
        kind: EditKind,
        at: EditTime,
    ) -> Self {
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        Self {
            edits,
            before,
            after,
            kind,
            at,
        }
    }

    /// The transaction that puts the text back, addressed in the changed text's coordinates.
    pub fn invert(&self) -> Transaction {
        // Ascending order accumulates how far the edits below have moved this one's site.
        let mut delta = 0isize;
        let mut inverted: Vec<Edit> = self
            .edits
            .iter()
            .rev()
            .map(|edit| {
                let start = (edit.range.start as isize + delta) as usize;
                delta += edit.delta();
                Edit {
                    range: start..start + edit.inserted.len(),
                    inserted: edit.deleted.clone(),
                    deleted: edit.inserted.clone(),
                }
            })
            .collect();
        inverted.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        Transaction {
            edits: inverted,
            before: self.after.clone(),
            after: self.before.clone(),
            kind: EditKind::Other,
            at: self.at,
        }
    }

    /// Where `byte` of the old text sits in the new one.
    ///
    /// A position inside a replaced range lands at the end of what replaced it, which is where a
    /// caret that was there belongs.
    pub fn map(&self, byte: usize) -> usize {
        let mut delta = 0isize;
        for edit in self.edits.iter().rev() {
            if byte < edit.range.start {
                break;
            }
            if byte < edit.range.end {
                return (edit.range.start as isize + delta) as usize + edit.inserted.len();
            }
            delta += edit.delta();
        }
        (byte as isize + delta) as usize
    }

    /// Like [`map`](Self::map), but a position at an edit's start stays put rather than moving
    /// past what was inserted there — the bias an explicit caret placement wants.
    pub fn map_before(&self, byte: usize) -> usize {
        let mut delta = 0isize;
        for edit in self.edits.iter().rev() {
            if byte <= edit.range.start {
                break;
            }
            if byte < edit.range.end {
                return (edit.range.start as isize + delta) as usize;
            }
            delta += edit.delta();
        }
        (byte as isize + delta) as usize
    }

    /// The edits as a consumer outside the crate takes them, in the order they applied.
    ///
    /// Descending by start, the same order as [`input_edits`](Self::input_edits) and for the same
    /// reason: when an edit applies, everything at or below its own start is still exactly as the
    /// text before the transaction had it, so its original coordinates are the right ones at its
    /// own application time. A consumer that applies them in this order needs no offset fixing.
    pub fn changes(&self) -> Vec<TextChange> {
        self.edits
            .iter()
            .map(|edit| TextChange {
                range: edit.range.clone(),
                text: edit.inserted.clone(),
            })
            .collect()
    }

    /// The edits as tree-sitter describes them, against `before`, the text they applied to.
    ///
    /// One `InputEdit` per edit, in the descending order they are applied in: when an edit
    /// applies, everything at or below its own start is still exactly as `before` had it, so its
    /// original coordinates are the right ones at its own application time.
    pub fn input_edits(&self, before: &Rope) -> Vec<tree_sitter::InputEdit> {
        self.edits
            .iter()
            .map(|edit| {
                let start_position = point_of(before, edit.range.start);
                tree_sitter::InputEdit {
                    start_byte: edit.range.start,
                    old_end_byte: edit.range.end,
                    new_end_byte: edit.range.start + edit.inserted.len(),
                    start_position,
                    old_end_position: point_of(before, edit.range.end),
                    new_end_position: advance(start_position, &edit.inserted),
                }
            })
            .collect()
    }

    /// Whether this transaction continues `earlier` closely enough to share its undo step.
    ///
    /// Consecutive typing at the same place within a moment is one thought; so is consecutive
    /// deleting. Anything else, and any pause, is its own step.
    pub fn coalesces_with(&self, earlier: &Transaction, window: std::time::Duration) -> bool {
        if self.kind != earlier.kind {
            return false;
        }
        if !matches!(self.kind, EditKind::Typing | EditKind::Deletion) {
            return false;
        }
        self.at.since(earlier.at) <= window
    }
}

/// The row and byte-column of `byte`, as tree-sitter counts them.
fn point_of(rope: &Rope, byte: usize) -> tree_sitter::Point {
    let byte = byte.min(rope.len_bytes());
    let row = rope.byte_to_line(byte);
    let column = byte - rope.line_to_byte(row);
    tree_sitter::Point { row, column }
}

/// Where `text` inserted at `from` ends.
fn advance(from: tree_sitter::Point, text: &str) -> tree_sitter::Point {
    let mut row = from.row;
    let mut column = from.column;
    for byte in text.bytes() {
        if byte == b'\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    tree_sitter::Point { row, column }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::Buffer;

    fn transaction(edits: Vec<Edit>) -> Transaction {
        Transaction::new(
            edits,
            Selections::caret(0),
            Selections::caret(0),
            EditKind::Other,
            EditTime::now(),
        )
    }

    #[test]
    fn inverting_a_transaction_puts_the_text_back() {
        let mut buffer = Buffer::from_str("one two three");
        let tx = transaction(vec![
            Edit {
                range: 0..3,
                inserted: "1".to_string(),
                deleted: "one".to_string(),
            },
            Edit {
                range: 8..13,
                inserted: "3".to_string(),
                deleted: "three".to_string(),
            },
        ]);
        buffer.apply(&tx.edits);
        assert_eq!(buffer.to_string(), "1 two 3");
        let back = tx.invert();
        buffer.apply(&back.edits);
        assert_eq!(buffer.to_string(), "one two three");
    }

    #[test]
    fn mapping_moves_positions_past_an_edit() {
        let tx = transaction(vec![Edit {
            range: 2..5,
            inserted: "x".to_string(),
            deleted: "abc".to_string(),
        }]);
        assert_eq!(tx.map(1), 1, "before the edit nothing moves");
        assert_eq!(tx.map(3), 3, "inside lands after the replacement");
        assert_eq!(tx.map(7), 5, "after the edit the delta applies");
    }

    #[test]
    fn input_edits_carry_rows_and_columns() {
        let rope = Rope::from_str("one\ntwo\nthree");
        let tx = transaction(vec![Edit {
            range: 4..7,
            inserted: "2\n2".to_string(),
            deleted: "two".to_string(),
        }]);
        let edits = tx.input_edits(&rope);
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].start_position,
            tree_sitter::Point { row: 1, column: 0 }
        );
        assert_eq!(
            edits[0].old_end_position,
            tree_sitter::Point { row: 1, column: 3 }
        );
        assert_eq!(
            edits[0].new_end_position,
            tree_sitter::Point { row: 2, column: 1 }
        );
        assert_eq!(edits[0].new_end_byte, 7);
    }

    #[test]
    fn typing_soon_after_typing_coalesces() {
        let mut a = transaction(vec![]);
        a.kind = EditKind::Typing;
        let mut b = transaction(vec![]);
        b.kind = EditKind::Typing;
        b.at = a.at + std::time::Duration::from_millis(100);
        assert!(b.coalesces_with(&a, std::time::Duration::from_millis(750)));
        b.at = a.at + std::time::Duration::from_secs(2);
        assert!(!b.coalesces_with(&a, std::time::Duration::from_millis(750)));
    }
}
