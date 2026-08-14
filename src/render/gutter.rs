//! The line numbers beside the text.

use crate::config::GutterMode;

/// How many cells of space sit inside the gutter beyond its digits.
const PADDING_CELLS: f32 = 2.0;

/// How wide the gutter is, in device pixels.
pub fn width(mode: GutterMode, line_count: usize, cell_advance: f32) -> f32 {
    if mode == GutterMode::None {
        return 0.0;
    }
    let digits = digits(line_count).max(3) as f32;
    ((digits + PADDING_CELLS) * cell_advance).ceil()
}

/// The label line `line` shows, when the gutter shows one.
///
/// Relative numbering shows each line's distance from the caret's line, and the caret's line its
/// own number — the arrangement vim calls `relativenumber` with `number`.
pub fn label(mode: GutterMode, line: usize, caret_line: usize) -> Option<String> {
    match mode {
        GutterMode::None => None,
        GutterMode::Absolute => Some((line + 1).to_string()),
        GutterMode::Relative if line == caret_line => Some((line + 1).to_string()),
        GutterMode::Relative => Some(line.abs_diff(caret_line).to_string()),
    }
}

/// How many digits `line_count` needs.
fn digits(line_count: usize) -> u32 {
    line_count.max(1).ilog10() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gutter_grows_with_the_document() {
        assert!(width(GutterMode::Absolute, 100_000, 8.0) > width(GutterMode::Absolute, 100, 8.0));
        assert_eq!(width(GutterMode::None, 100, 8.0), 0.0);
    }

    #[test]
    fn short_documents_still_get_three_digits_of_room() {
        assert_eq!(
            width(GutterMode::Absolute, 5, 8.0),
            (3.0 + PADDING_CELLS) * 8.0
        );
    }

    #[test]
    fn relative_labels_count_from_the_caret() {
        assert_eq!(label(GutterMode::Relative, 10, 10), Some("11".to_string()));
        assert_eq!(label(GutterMode::Relative, 7, 10), Some("3".to_string()));
        assert_eq!(label(GutterMode::Relative, 13, 10), Some("3".to_string()));
        assert_eq!(label(GutterMode::Absolute, 0, 10), Some("1".to_string()));
        assert_eq!(label(GutterMode::None, 0, 10), None);
    }
}
