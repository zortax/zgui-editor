//! The carets and the selections, of which there may be several.
//!
//! A selection is an anchor and a head, both byte offsets; a caret is a selection whose two ends
//! coincide. The head is the end that moves and the end the caret is painted at. The list is kept
//! sorted by position with overlaps merged, because every consumer — painting, editing, the
//! status line — wants them in document order and non-overlapping, and normalizing once at the
//! mutation seam is cheaper than defending everywhere else.

use std::ops::Range;

use smallvec::{SmallVec, smallvec};

use crate::core::position::Affinity;

/// One selection: an anchor that stays and a head that moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    /// The end that stays put when the selection extends.
    pub anchor: usize,
    /// The end that moves, where the caret paints.
    pub head: usize,
    /// Which side of a boundary the caret leans toward.
    pub affinity: Affinity,
    /// The grapheme column vertical movement aims for, kept across lines shorter than it.
    pub goal_col: Option<u32>,
}

impl Selection {
    /// A caret at `at`.
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
            affinity: Affinity::default(),
            goal_col: None,
        }
    }

    /// A selection from `anchor` to `head`.
    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            affinity: Affinity::default(),
            goal_col: None,
        }
    }

    /// Whether the two ends coincide.
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected bytes, lowest first.
    pub fn range(&self) -> Range<usize> {
        if self.anchor <= self.head {
            self.anchor..self.head
        } else {
            self.head..self.anchor
        }
    }

    /// The lower of the two ends.
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The higher of the two ends.
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// This selection collapsed to its head.
    pub fn collapsed(&self) -> Self {
        Self {
            anchor: self.head,
            ..*self
        }
    }
}

/// Every selection, in document order, with a primary among them.
#[derive(Clone, Debug)]
pub struct Selections {
    list: SmallVec<[Selection; 1]>,
    primary: usize,
}

impl Selections {
    /// One caret at `at`.
    pub fn caret(at: usize) -> Self {
        Self {
            list: smallvec![Selection::caret(at)],
            primary: 0,
        }
    }

    /// The given selections, sorted and merged, with `primary` naming one of them by its index
    /// in `selections` before sorting.
    pub fn new(selections: Vec<Selection>, primary: usize) -> Self {
        let mut this = Self {
            list: SmallVec::from_vec(selections),
            primary,
        };
        if this.list.is_empty() {
            this.list.push(Selection::caret(0));
        }
        this.normalize();
        this
    }

    /// The selections, in document order.
    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.list.iter()
    }

    /// How many selections there are. Never zero.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Whether there is exactly one selection.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The primary selection, the one the viewport follows and the status line reports.
    pub fn primary(&self) -> Selection {
        self.list[self.primary.min(self.list.len() - 1)]
    }

    /// Replaces every selection with one.
    pub fn set_one(&mut self, selection: Selection) {
        self.list.clear();
        self.list.push(selection);
        self.primary = 0;
    }

    /// Replaces the whole set.
    pub fn set(&mut self, selections: Vec<Selection>, primary: usize) {
        if selections.is_empty() {
            return;
        }
        self.list = SmallVec::from_vec(selections);
        self.primary = primary;
        self.normalize();
    }

    /// Maps every selection through `f`, then restores order and disjointness.
    pub fn map(&mut self, mut f: impl FnMut(Selection) -> Selection) {
        for selection in self.list.iter_mut() {
            *selection = f(*selection);
        }
        self.normalize();
    }

    /// Sorts by start and merges overlapping ranges; two carets at one place become one.
    ///
    /// The primary follows the selection it named through the sort and any merge it takes part
    /// in, so "the selection the user is steering" survives normalization.
    fn normalize(&mut self) {
        let mut indexed: SmallVec<[(usize, Selection); 1]> =
            self.list.iter().copied().enumerate().collect();
        indexed.sort_by_key(|(index, selection)| (selection.start(), selection.end(), *index));

        let mut merged: SmallVec<[Selection; 1]> = SmallVec::new();
        let mut primary_at = 0;
        for (index, selection) in indexed {
            let overlaps = merged.last().is_some_and(|last: &Selection| {
                let touching_carets = selection.is_caret() && last.is_caret();
                selection.start() < last.end()
                    || (touching_carets && selection.start() == last.end())
            });
            if overlaps {
                let last = merged.last_mut().expect("overlap checked against a last");
                // The union, keeping the newer selection's direction and goal.
                let start = last.start().min(selection.start());
                let end = last.end().max(selection.end());
                *last = if selection.head >= selection.anchor {
                    Selection {
                        anchor: start,
                        head: end,
                        ..selection
                    }
                } else {
                    Selection {
                        anchor: end,
                        head: start,
                        ..selection
                    }
                };
            } else {
                merged.push(selection);
            }
            if index == self.primary {
                primary_at = merged.len() - 1;
            }
        }
        self.list = merged;
        self.primary = primary_at.min(self.list.len() - 1);
    }
}

impl Default for Selections {
    fn default() -> Self {
        Self::caret(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_knows_its_range_whichever_way_it_points() {
        assert_eq!(Selection::new(2, 5).range(), 2..5);
        assert_eq!(Selection::new(5, 2).range(), 2..5);
    }

    #[test]
    fn selections_sort_into_document_order() {
        let mut selections = Selections::caret(0);
        selections.set(vec![Selection::caret(9), Selection::caret(3)], 0);
        let at: Vec<usize> = selections.iter().map(|selection| selection.head).collect();
        assert_eq!(at, vec![3, 9]);
    }

    #[test]
    fn overlapping_selections_merge() {
        let mut selections = Selections::caret(0);
        selections.set(vec![Selection::new(0, 5), Selection::new(3, 8)], 1);
        assert_eq!(selections.len(), 1);
        assert_eq!(selections.primary().range(), 0..8);
    }

    #[test]
    fn coinciding_carets_become_one() {
        let mut selections = Selections::caret(0);
        selections.set(vec![Selection::caret(4), Selection::caret(4)], 0);
        assert_eq!(selections.len(), 1);
    }

    #[test]
    fn adjacent_ranges_stay_apart() {
        let mut selections = Selections::caret(0);
        selections.set(vec![Selection::new(0, 3), Selection::new(3, 6)], 0);
        assert_eq!(selections.len(), 2, "touching ranges select different text");
    }

    #[test]
    fn the_primary_survives_sorting() {
        let mut selections = Selections::caret(0);
        selections.set(vec![Selection::caret(9), Selection::caret(3)], 0);
        assert_eq!(selections.primary().head, 9);
    }
}
