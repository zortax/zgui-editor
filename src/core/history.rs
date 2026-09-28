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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// How many undo groups are open. While one is open, every change joins one step.
    group: usize,
    /// Whether the open group has not recorded a change yet.
    group_fresh: bool,
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
        if self.group > 0 {
            match self.undo.last_mut() {
                Some(top) if !self.group_fresh => top.transactions.push(tx),
                _ => self.undo.push(Step {
                    transactions: vec![tx],
                }),
            }
            self.group_fresh = false;
            self.open = true;
            return;
        }
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
        if self.group == 0 {
            self.open = false;
        }
    }

    /// Replaces where the newest transaction left the selections.
    ///
    /// Redo puts the selections back there. A change that places its own carets calls this.
    pub fn amend_after(&mut self, after: crate::core::selection::Selections) {
        if let Some(tx) = self
            .undo
            .last_mut()
            .and_then(|step| step.transactions.last_mut())
        {
            tx.after = after;
        }
    }

    /// Opens an undo group. Every change until the matching [`end_group`](Self::end_group)
    /// undoes as one step. Groups nest; the outermost one decides the step.
    pub fn begin_group(&mut self) {
        if self.group == 0 {
            self.group_fresh = true;
        }
        self.group += 1;
    }

    /// Closes an undo group. Closing the outermost group seals the step.
    pub fn end_group(&mut self) {
        self.group = self.group.saturating_sub(1);
        if self.group == 0 {
            self.open = false;
        }
    }

    /// Whether an undo group is open.
    pub fn in_group(&self) -> bool {
        self.group > 0
    }

    /// The step to undo, when there is one. The caller applies each transaction's inversion,
    /// **newest first** — the order the step's iterator does not supply by itself.
    pub fn undo(&mut self) -> Option<Step> {
        self.open = false;
        self.group = 0;
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        Some(step)
    }

    /// The step to re-apply, when there is one. The caller re-applies its transactions oldest
    /// first, as they are stored.
    pub fn redo(&mut self) -> Option<Step> {
        self.open = false;
        self.group = 0;
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        Some(step)
    }

    /// How many undo steps there are.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// The steps that can be undone, oldest first.
    ///
    /// For writing a history down. The stacks are private because pushing to them out of order
    /// would make undo apply edits against text they were never valid in.
    pub fn undo_steps(&self) -> &[Step] {
        &self.undo
    }

    /// The steps that can be redone, the next one last.
    pub fn redo_steps(&self) -> &[Step] {
        &self.redo
    }

    /// A history holding `undo` and `redo`, sealed.
    ///
    /// What a restored document is built with. Sealed, because whatever is typed next is a new
    /// thought however close together the two runs happened to be, and because the times in a
    /// restored step came from another run's clock.
    pub fn from_parts(undo: Vec<Step>, redo: Vec<Step>) -> Self {
        Self {
            undo,
            redo,
            open: false,
            group: 0,
            group_fresh: false,
        }
    }

    /// How many bytes of replaced and replacing text the whole history holds.
    ///
    /// What decides whether it is small enough to be worth writing down.
    pub fn text_bytes(&self) -> usize {
        let of = |steps: &Vec<Step>| {
            steps
                .iter()
                .flat_map(|step| step.transactions.iter())
                .flat_map(|tx| tx.edits.iter())
                .map(|edit| edit.inserted.len() + edit.deleted.len())
                .sum::<usize>()
        };
        of(&self.undo) + of(&self.redo)
    }

    /// Drops the oldest steps until at most `steps` remain and their text is under `bytes`.
    ///
    /// The oldest, because the recent ones are the ones anybody undoes. Answers whether anything
    /// was dropped, so a session can say so rather than quietly shortening somebody's history.
    ///
    /// The redo stack is capped by `steps` as well: nobody redoes across a restart, so it is the
    /// first thing worth losing.
    pub fn trim(&mut self, steps: usize, bytes: usize) -> bool {
        let before = (self.undo.len(), self.redo.len());

        if self.redo.len() > steps {
            let over = self.redo.len() - steps;
            self.redo.drain(..over);
        }
        if self.undo.len() > steps {
            let over = self.undo.len() - steps;
            self.undo.drain(..over);
        }
        while self.text_bytes() > bytes {
            // The redo stack first: it is the half nobody comes back to.
            if self.redo.is_empty() && self.undo.is_empty() {
                break;
            }
            if !self.redo.is_empty() {
                self.redo.remove(0);
            } else {
                self.undo.remove(0);
            }
        }

        before != (self.undo.len(), self.redo.len())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::core::edit::{Edit, EditKind, EditTime};
    use crate::core::selection::Selections;

    fn typing(at: EditTime, range: std::ops::Range<usize>, text: &str) -> Transaction {
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
        let start = EditTime::now();
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
        let start = EditTime::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.push(typing(start + Duration::from_secs(3), 1..1, "b"));
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn sealing_starts_a_new_step() {
        let start = EditTime::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.seal();
        history.push(typing(start + Duration::from_millis(10), 1..1, "b"));
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn a_group_joins_every_change_into_one_step() {
        let start = EditTime::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "x"));
        history.begin_group();
        history.push(typing(start + Duration::from_secs(5), 1..1, "a"));
        history.seal();
        history.push(typing(start + Duration::from_secs(10), 2..2, "b"));
        history.end_group();
        history.push(typing(start + Duration::from_secs(10), 3..3, "c"));
        assert_eq!(history.undo_depth(), 3);
        let undone = history.undo().expect("a step");
        assert_eq!(undone.transactions.len(), 1);
        let undone = history.undo().expect("a step");
        assert_eq!(undone.transactions.len(), 2);
    }

    #[test]
    fn nested_groups_close_with_the_outermost() {
        let start = EditTime::now();
        let mut history = History::new();
        history.begin_group();
        history.begin_group();
        history.push(typing(start, 0..0, "a"));
        history.end_group();
        assert!(history.in_group());
        history.push(typing(start + Duration::from_secs(5), 1..1, "b"));
        history.end_group();
        assert!(!history.in_group());
        assert_eq!(history.undo_depth(), 1);
    }

    #[test]
    fn undo_and_redo_round_trip() {
        let start = EditTime::now();
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
        let start = EditTime::now();
        let mut history = History::new();
        history.push(typing(start, 0..0, "a"));
        history.seal();
        let _ = history.undo();
        history.push(typing(start + Duration::from_millis(10), 0..0, "c"));
        assert!(history.redo().is_none());
    }
}

#[cfg(test)]
mod restoring {
    use super::*;
    use crate::core::edit::{Edit, EditKind, EditTime};
    use crate::core::selection::Selections;

    fn step(text: &str) -> Step {
        Step {
            transactions: vec![Transaction {
                edits: vec![Edit {
                    range: 0..0,
                    inserted: text.to_owned(),
                    deleted: String::new(),
                }],
                before: Selections::caret(0),
                after: Selections::caret(text.len()),
                kind: EditKind::Typing,
                at: EditTime::now(),
            }],
        }
    }

    #[test]
    fn a_restored_history_can_be_undone() {
        let mut history = History::from_parts(vec![step("a"), step("b")], Vec::new());
        assert_eq!(history.undo_depth(), 2);
        assert!(history.undo().is_some());
        assert_eq!(history.undo_depth(), 1);
    }

    #[test]
    fn a_restored_history_is_sealed() {
        // Whatever is typed next is a new thought, however close the two runs happened to be.
        let mut history = History::from_parts(vec![step("a")], Vec::new());
        history.push(step("b").transactions.remove(0));
        assert_eq!(history.undo_depth(), 2, "the new change is its own step");
    }

    #[test]
    fn the_stacks_can_be_read_back_out() {
        let history = History::from_parts(vec![step("a")], vec![step("b")]);
        assert_eq!(history.undo_steps().len(), 1);
        assert_eq!(history.redo_steps().len(), 1);
    }

    #[test]
    fn trimming_drops_the_oldest_steps_first() {
        let mut history = History::from_parts(
            vec![step("oldest"), step("middle"), step("newest")],
            Vec::new(),
        );
        assert!(history.trim(2, usize::MAX));
        assert_eq!(history.undo_steps().len(), 2);
        // The one anybody would actually undo is the one kept.
        let kept = &history.undo_steps().last().expect("a step").transactions[0].edits[0].inserted;
        assert_eq!(kept, "newest");
    }

    #[test]
    fn trimming_by_bytes_gives_the_redo_stack_up_first() {
        let mut history = History::from_parts(vec![step("keep")], vec![step("drop")]);
        assert!(history.text_bytes() > 4);
        assert!(history.trim(usize::MAX, 5));
        assert!(history.redo_steps().is_empty());
        assert_eq!(history.undo_steps().len(), 1);
    }

    #[test]
    fn trimming_something_already_small_enough_changes_nothing() {
        let mut history = History::from_parts(vec![step("a")], Vec::new());
        assert!(!history.trim(10, 1024));
        assert_eq!(history.undo_steps().len(), 1);
    }
}
