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
    /// What the caret looks like.
    pub cursor_style: CursorStyle,
    /// Whether the caret blinks.
    pub blink: bool,
    /// Whether wheel scrolling glides rather than jumps.
    pub smooth_scroll: bool,
    /// How many lines one wheel notch scrolls.
    pub scroll_lines: f64,
    /// How many lines stay visible above and below the caret when the view follows it.
    pub scrolloff: usize,
    /// What editing does beyond what the keys say.
    pub edit: EditOptions,
    /// Whether the selection is copied to the primary selection on mouse release, where the
    /// platform has one.
    pub copy_on_select: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            gutter: GutterMode::default(),
            cursor_style: CursorStyle::default(),
            blink: true,
            smooth_scroll: true,
            scroll_lines: 3.0,
            scrolloff: 0,
            edit: EditOptions::default(),
            copy_on_select: true,
        }
    }
}
