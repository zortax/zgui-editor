//! What mode the layer is in, and everything it is holding.

use crate::pending::Pending;
use crate::registers::Registers;

/// The four modes this example implements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Keys are commands.
    Normal,
    /// Keys are text.
    Insert,
    /// Motions extend a selection.
    Visual,
    /// Motions extend a selection of whole lines.
    VisualLine,
}

impl Mode {
    /// What the status line shows.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
        }
    }
}

/// The whole of the vim layer's state. Lives outside the editor component, which knows nothing
/// about any of it — that is the point of the example.
pub struct VimState {
    /// Which mode keys are read in.
    pub mode: Mode,
    /// The count, operator and prefix collected so far.
    pub pending: Pending,
    /// What yanks and deletes filled.
    pub registers: Registers,
    /// The last `/` search, which `n` and `N` reuse.
    pub last_search: Option<String>,
    /// The `/` line being typed, while one is.
    pub search_input: Option<String>,
}

impl VimState {
    /// A layer starting in normal mode.
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            pending: Pending::default(),
            registers: Registers::default(),
            last_search: None,
            search_input: None,
        }
    }
}
