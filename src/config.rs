//! What an application decides about an editor before styling it.
//!
//! Colours and fonts belong to CSS; behaviour belongs here. Everything has a default that makes
//! an unconfigured editor a sensible plain-text editor.

use crate::core::EditOptions;

/// How the gutter numbers its lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GutterMode {
    /// Every line shows its own number.
    #[default]
    Absolute,
    /// Every line shows its distance from the caret's line, which shows its own number.
    Relative,
    /// No gutter at all.
    None,
    /// Every line shows what the application's gutter source says, in a gutter this many cells
    /// wide plus the usual padding. See `EditorHandle::set_gutter_source`.
    Custom {
        /// How many character cells the labels need.
        cells: u16,
    },
}

/// What the caret looks like.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorStyle {
    /// A vertical bar between characters, the insert-mode caret.
    #[default]
    Bar,
    /// A filled cell over the character, the vim normal-mode caret.
    Block,
    /// A line under the character.
    Underline,
    /// An outlined cell, what an unfocused block becomes.
    Hollow,
}

/// The behaviour an editor is built with.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    /// How lines are numbered.
    pub gutter: GutterMode,
    /// Whether the view draws its own vertical scrollbar.
    ///
    /// Off for a view that scrolls in step with another one beside it, which shows the bar for
    /// both. The wheel, the keys and every scroll command still work.
    pub scrollbar: bool,
    /// What the caret looks like.
    pub cursor_style: CursorStyle,
    /// Whether the caret blinks.
    pub blink: bool,
    /// Whether wheel scrolling glides rather than jumps.
    pub smooth_scroll: bool,
    /// How far a caret-following scroll may move and still snap rather than glide, in lines.
    ///
    /// Four by default, which is the old behaviour: typing at the bottom of the view nudges it a
    /// line at a time and must never lag behind the keystroke, while a `G` or a search hit reads
    /// far better arriving as motion. Zero glides everything, which is what an application that
    /// would rather have every scroll animated asks for.
    pub glide_threshold_lines: f64,
    /// How many lines one wheel notch scrolls.
    pub scroll_lines: f64,
    /// How many lines stay visible above and below the caret when the view follows it.
    pub scrolloff: usize,
    /// What editing does beyond what the keys say.
    pub edit: EditOptions,
    /// Whether the selection is copied to the primary selection on mouse release, where the
    /// platform has one.
    pub copy_on_select: bool,
    /// Which lines the view draws, when it draws only some of them.
    ///
    /// A window turns one editor into a view of one part of a document: it draws exactly these
    /// lines, sizes itself to them, and never scrolls vertically. The gutter still numbers the
    /// real lines, the caret still moves through the whole text, and the history is still the
    /// document's own.
    ///
    /// What a rendered document puts a real editor over one block with, and what a preview shows
    /// a hit in its own place with. `None` draws the whole document, which is the usual thing.
    pub line_window: Option<std::ops::Range<usize>>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            gutter: GutterMode::default(),
            scrollbar: true,
            cursor_style: CursorStyle::default(),
            blink: true,
            smooth_scroll: true,
            glide_threshold_lines: 4.0,
            scroll_lines: 3.0,
            scrolloff: 0,
            edit: EditOptions::default(),
            copy_on_select: true,
            line_window: None,
        }
    }
}
