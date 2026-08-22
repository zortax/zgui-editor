//! What an application paints in place of what the selections would.
//!
//! A selection is bytes, and bytes stop at the end of a line. A modal layer's visual modes need
//! more than that: a charwise selection reaches through the character its caret is on, a linewise
//! one takes whole lines while its caret stays on a column, and a block one is a rectangle that
//! keeps its shape over lines ending inside it. All three are cells — a line and a column — so
//! cells are what an application hands over.
//!
//! ```no_run
//! # use zgui_editor::{Band, Caret, EditorHandle, Overlay};
//! # fn example(handle: &EditorHandle) {
//! handle.set_overlay(Overlay {
//!     bands: (2..5).map(|line| Band { line, columns: 4..12 }).collect(),
//!     carets: vec![Caret { line: 4, column: 11 }],
//! });
//! # }
//! ```
//!
//! The colours are the editor's own: a band is the selection colour and a caret is the cursor
//! colour, because an overlay says where those two things are and not what they look like.

use std::ops::Range;

/// Where one caret sits: a line, and a column counted in graphemes.
///
/// A column past the end of its line is a real place, counted on in whole cells. That is where a
/// block selection's caret sits when the line it is on is short.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caret {
    /// Which line, counting from zero.
    pub line: usize,
    /// How many graphemes from the line's start.
    pub column: u32,
}

/// A run of cells on one line, painted as selected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Band {
    /// Which line, counting from zero.
    pub line: usize,
    /// The columns it covers, the last excluded.
    pub columns: Range<u32>,
}

/// The bands and the carets an application draws itself.
///
/// Each half stands alone: bands, when there are any, replace the bands the selections would
/// paint, and carets replace the carets they would. An empty overlay gives both back.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Overlay {
    /// The cells that read as selected.
    pub bands: Vec<Band>,
    /// Where the carets paint.
    pub carets: Vec<Caret>,
}

impl Overlay {
    /// Whether it says nothing at all.
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty() && self.carets.is_empty()
    }
}
