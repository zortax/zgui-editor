//! Undo and redo, with consecutive typing folded into one step.
//!
//! The rule is zgui-edit's: characters typed in one burst undo as one, and anything else — a
//! motion, a click, a different kind of change, or a pause — seals the step. Sealing is explicit
//! ([`History::seal`]) as well as timed, because the caller knows about motions and this module
//! does not.
//!
//! A step holds whole transactions rather than merged edits, because each transaction's offsets
//! are valid in the text as it stood when it applied. Undo inverts them newest-first and redo
//! re-applies them oldest-first, and no offset ever has to be rebased.

use std::time::Duration;

use crate::core::edit::Transaction;

/// How long a pause turns continued typing into a new undo step.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(750);

/// One undoable step: one or more transactions that undo together.
#[derive(Clone, Debug)]
pub struct Step {
    /// The transactions, oldest first.
    pub transactions: Vec<Transaction>,
}

/// The undone and the redoable.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Whether the top of the undo stack may still grow by coalescing.
    open: bool,
}

impl History {
    /// A history with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `tx` as applied, folding it into the step before it when it continues one.
    ///
    /// Any recorded change makes the redoable future unreachable, so the redo stack empties.
    pub fn push(&mut self, tx: Transaction) {
        self.redo.clear();
        if self.open
            && let Some(top) = self.undo.last_mut()
            && tx.coalesces_with(
                top.transactions.last().expect("a step is never empty"),
                COALESCE_WINDOW,
            )
        {
            top.transactions.push(tx);
            return;
        }
        self.undo.push(Step {
            transactions: vec![tx],
        });
        self.open = true;
    }

    /// Ends the growing step, as a motion, a click, or a mode change does.
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// The step to undo, when there is one. The caller applies each transaction's inversion,
    /// **newest first** — the order the step's iterator does not supply by itself.
    pub fn undo(&mut self) -> Option<Step> {
        self.open = false;
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        Some(step)
    }

    /// The step to re-apply, when there is one. The caller re-applies its transactions oldest
    /// first, as they are stored.
    pub fn redo(&mut self) -> Option<Step> {
        self.open = false;
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        Some(step)
    }

    /// How many undo steps there are.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::core::edit::{Edit, EditKind};
    use crate::core::selection::Selections;

    fn typing(at: Instant, range: std::ops::Range<usize>, text: &str) -> Transaction {
        Transaction {
            edits: vec![Edit {
                range,
                inserted: text.to_string(),
                deleted: String::new(),
            }],
            before: Selections::caret(0),
            after: Selections::caret(0),
            kind: EditKind::Typing,
            at,
        }
    }

    #[test]
    fn quick_typing_is_one_step() {
        let start = Instant::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.push(typing(start + Duration::from_millis(100), 1..1, "b"));
        assert_eq!(history.undo_depth(), 1);
        let step = history.undo().expect("a step");
        assert_eq!(step.transactions.len(), 2, "both keystrokes come back out");
        assert_eq!(
            step.transactions[0].edits[0].inserted, "a",
            "stored oldest first"
        );
    }

    #[test]
    fn a_pause_starts_a_new_step() {
        let start = Instant::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.push(typing(start + Duration::from_secs(3), 1..1, "b"));
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn sealing_starts_a_new_step() {
        let start = Instant::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.seal();
        history.push(typing(start + Duration::from_millis(10), 1..1, "b"));
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn undo_and_redo_round_trip() {
        let start = Instant::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.push(typing(start + Duration::from_millis(10), 1..1, "b"));
        let undone = history.undo().expect("a step");
        assert_eq!(undone.transactions.len(), 2);
        let redone = history.redo().expect("a step");
        assert_eq!(redone.transactions.len(), 2);
        assert_eq!(history.undo_depth(), 1);
    }

    #[test]
    fn a_new_change_clears_redo() {
        let start = Instant::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.seal();
        let _ = history.undo();
        history.push(typing(start + Duration::from_millis(10), 0..0, "c"));
        assert!(history.redo().is_none());
    }
}
