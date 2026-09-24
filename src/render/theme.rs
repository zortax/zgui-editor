//! Reading the editor's colours off the style sheet.
//!
//! Every colour the editor paints is a CSS custom property, so a theme is written where the rest
//! of an application's colours are: in the cascade, inheriting, answering to a class or a
//! dark-mode rule. The element's own `color` is the text, its `background` is painted by the
//! framework, and everything else is `--editor-*` or `--syntax-*`. What a sheet does not name
//! falls back to something derived from those two, so an unthemed editor is already readable.

use std::borrow::Cow;

use zgui::canvas::zgui_color::Color;
use zgui_css::ComputedStyle;

/// The capture names a sheet can colour, each read from `--syntax-<name>` with dots as hyphens.
///
/// The list is the conventional tree-sitter highlight vocabulary; a capture outside it falls
/// back along its dots and then to the foreground.
pub const SYNTAX_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment-doc",
    "constant",
    "constant-builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function-builtin",
    "function-macro",
    "function-method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation-bracket",
    "punctuation-delimiter",
    "punctuation-special",
    "string",
    "string-special",
    "string-special-key",
    "tag",
    "type",
    "type-builtin",
    "variable",
    "variable-builtin",
    "variable-parameter",
];

/// The colours the editor paints with, as one comparable value.
#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    /// The text, from the element's own `color`.
    pub fg: Color,
    /// The element's background, which defaults derive from.
    pub bg: Color,
    /// The band behind selected text.
    pub selection: Color,
    /// The band behind selected text when the editor is unfocused.
    pub selection_inactive: Color,
    /// The caret.
    pub cursor: Color,
    /// Text under a block caret.
    pub cursor_text: Color,
    /// The band behind the caret's line, when a sheet asks for one.
    pub current_line: Option<Color>,
    /// The gutter's own background, when a sheet asks for one.
    pub gutter_bg: Option<Color>,
    /// The line numbers.
    pub gutter_fg: Color,
    /// The caret line's number.
    pub gutter_current_fg: Color,
    /// The scrollbar thumb.
    pub scrollbar_thumb: Color,
    /// The scrollbar track, when a sheet asks for one.
    pub scrollbar_track: Option<Color>,
    /// The syntax colours a sheet named, by capture name with dots as hyphens.
    pub syntax: Vec<(Cow<'static, str>, Color)>,
}

/// A colour with its alpha replaced.
pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color::new(color.space(), color.components(), alpha)
}

/// The colour `--{name}` holds, when a rule set one.
fn read(style: &ComputedStyle, name: &str) -> Option<Color> {
    zgui_css::values::custom::color(style, name)
}

/// The theme `style` describes.
///
/// The syntax colours are the conventional vocabulary plus every name in `captures` and each
/// shorter name along its dots, so an application that colours its own captures names their
/// colours in the sheet as well.
pub fn from_style(style: &ComputedStyle, captures: &[String]) -> Theme {
    let fg = zgui_css::values::color::to_color(zgui_css::values::color::current(style));
    let bg = zgui_css::values::color::resolve(
        &style.get_background().background_color,
        zgui_css::values::color::current(style),
    );
    let selection = read(style, "editor-selection").unwrap_or_else(|| with_alpha(fg, 0.25));
    let cursor = read(style, "editor-cursor").unwrap_or(fg);
    let mut syntax: Vec<(Cow<'static, str>, Color)> = Vec::new();
    for name in SYNTAX_NAMES {
        if let Some(color) = read(style, &format!("syntax-{name}")) {
            syntax.push((Cow::Borrowed(*name), color));
        }
    }
    for capture in captures {
        let mut name = capture.replace('.', "-");
        loop {
            let known = SYNTAX_NAMES.contains(&name.as_str())
                || syntax.iter().any(|(held, _)| *held == name);
            if !known && let Some(color) = read(style, &format!("syntax-{name}")) {
                syntax.push((Cow::Owned(name.clone()), color));
            }
            match name.rfind('-') {
                Some(cut) => name.truncate(cut),
                None => break,
            }
        }
    }
    Theme {
        fg,
        bg,
        selection,
        selection_inactive: read(style, "editor-selection-inactive")
            .unwrap_or_else(|| with_alpha(fg, 0.12)),
        cursor,
        cursor_text: read(style, "editor-cursor-text").unwrap_or(bg),
        current_line: read(style, "editor-current-line"),
        gutter_bg: read(style, "editor-gutter-bg"),
        gutter_fg: read(style, "editor-gutter-fg").unwrap_or_else(|| with_alpha(fg, 0.4)),
        gutter_current_fg: read(style, "editor-gutter-current-fg").unwrap_or(fg),
        scrollbar_thumb: read(style, "editor-scrollbar-thumb")
            .unwrap_or_else(|| with_alpha(fg, 0.25)),
        scrollbar_track: read(style, "editor-scrollbar-track"),
        syntax,
    }
}

impl Theme {
    /// A theme derived from nothing, for before the first layout.
    pub fn fallback() -> Self {
        let fg = Color::srgb(0.9, 0.9, 0.9, 1.0);
        let bg = Color::srgb(0.1, 0.1, 0.12, 1.0);
        Theme {
            fg,
            bg,
            selection: with_alpha(fg, 0.25),
            selection_inactive: with_alpha(fg, 0.12),
            cursor: fg,
            cursor_text: bg,
            current_line: None,
            gutter_bg: None,
            gutter_fg: with_alpha(fg, 0.4),
            gutter_current_fg: fg,
            scrollbar_thumb: with_alpha(fg, 0.25),
            scrollbar_track: None,
            syntax: Vec::new(),
        }
    }

    /// The colour for a capture like `function.method`: the exact name, then along its dots,
    /// then the foreground.
    pub fn capture_color(&self, capture: &str) -> Color {
        let mut name = capture.replace('.', "-");
        loop {
            if let Some((_, color)) = self.syntax.iter().find(|(held, _)| *held == name) {
                return *color;
            }
            match name.rfind('-') {
                Some(cut) => name.truncate(cut),
                None => return self.fg,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_colours_fall_back_along_their_dots() {
        let mut theme = Theme::fallback();
        let green = Color::srgb(0.0, 1.0, 0.0, 1.0);
        theme.syntax.push((Cow::Borrowed("function"), green));
        assert_eq!(theme.capture_color("function.method"), green);
        assert_eq!(theme.capture_color("function"), green);
        assert_eq!(theme.capture_color("keyword"), theme.fg);
    }
}
