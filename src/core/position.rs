//! Where a caret can be: byte offsets, lines, columns, and the grapheme boundaries between them.
//!
//! The canonical address of any position in a buffer is a **byte offset** into the rope. That is
//! the unit ropey slices by, tree-sitter edits by, and the shaper reports cluster origins in, so
//! nothing is converted at a seam more than once. Lines and columns are derived views: a line is
//! what ropey says it is, and a column counts grapheme clusters from the line's start, because a
//! grapheme is what a caret can sit between.
//!
//! Char indices are never used as an address. Ropey 1.x mutates by char index, so the conversion
//! happens exactly once, inside [`crate::core::buffer::Buffer`], and nowhere else.

use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

/// Which side of a boundary a caret leans toward.
///
/// One byte offset at a line-wrap or direction boundary describes two visual carets. Without soft
/// wrap the distinction only matters at the seam between lines, but the field is carried from the
/// start so selections survive the day soft wrap arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Affinity {
    /// The caret belongs to what follows the offset.
    #[default]
    Downstream,
    /// The caret belongs to what precedes the offset.
    Upstream,
}

/// The line `byte` falls on, counting from zero.
pub fn line_of(rope: &Rope, byte: usize) -> usize {
    rope.byte_to_line(byte.min(rope.len_bytes()))
}

/// The byte the line `line` starts at.
pub fn line_start(rope: &Rope, line: usize) -> usize {
    rope.line_to_byte(line.min(rope.len_lines()))
}

/// The byte just past the last character of `line`, before its line break.
pub fn line_end(rope: &Rope, line: usize) -> usize {
    let start = line_start(rope, line);
    let next = if line + 1 >= rope.len_lines() {
        rope.len_bytes()
    } else {
        rope.line_to_byte(line + 1)
    };
    next - line_break_len(rope, start, next)
}

/// How many bytes of line break sit at the end of the span from `start` to `next`.
fn line_break_len(rope: &Rope, start: usize, next: usize) -> usize {
    if next <= start {
        return 0;
    }
    let slice = rope.byte_slice(start..next);
    let len = slice.len_bytes();
    let mut bytes = [0u8; 2];
    let tail_len = len.min(2);
    for (offset, byte) in slice.bytes_at(len - tail_len).enumerate() {
        bytes[2 - tail_len + offset] = byte;
    }
    match (bytes[0], bytes[1]) {
        (b'\r', b'\n') => 2,
        (_, b'\n') | (_, b'\r') => 1,
        _ => 0,
    }
}

/// How many lines the buffer has.
///
/// A buffer ending in a line break has an empty last line, which is a line a caret can sit on
/// and is counted. This is ropey's own convention and every editor's.
pub fn line_count(rope: &Rope) -> usize {
    rope.len_lines()
}

/// The text of `line`, without its line break.
pub fn line_text(rope: &Rope, line: usize) -> Cow<'_, str> {
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    Cow::from(rope.byte_slice(start..end))
}

/// Whether `byte` sits on a grapheme boundary of its line, or at a line seam.
pub fn is_boundary(rope: &Rope, byte: usize) -> bool {
    let byte = byte.min(rope.len_bytes());
    let line = line_of(rope, byte);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if byte >= end {
        // Inside a line break only its start is a boundary.
        return byte == end || byte == line_start(rope, line + 1);
    }
    let text = line_text(rope, line);
    let offset = byte - start;
    text.grapheme_indices(true).any(|(at, _)| at == offset) || offset == 0
}

/// `byte`, moved to the nearest grapheme boundary at or before it.
pub fn snap(rope: &Rope, byte: usize) -> usize {
    let byte = byte.min(rope.len_bytes());
    let line = line_of(rope, byte);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if byte >= end {
        // Inside a line break the only boundaries are its two ends.
        let next = line_start(rope, line + 1).min(rope.len_bytes());
        return if byte == next { byte } else { end };
    }
    let text = line_text(rope, line);
    let offset = byte - start;
    let mut last = 0;
    for (at, grapheme) in text.grapheme_indices(true) {
        if at + grapheme.len() > offset {
            return start + if at <= offset { at } else { last };
        }
        last = at;
    }
    end
}

/// The boundary after `byte`: one grapheme along, or the start of the next line at a line's end.
pub fn next_grapheme(rope: &Rope, byte: usize) -> usize {
    let byte = byte.min(rope.len_bytes());
    if byte >= rope.len_bytes() {
        return rope.len_bytes();
    }
    let line = line_of(rope, byte);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if byte >= end {
        // Inside or at the line break: the next boundary is the next line's start.
        return line_start(rope, line + 1).min(rope.len_bytes());
    }
    let text = line_text(rope, line);
    let offset = byte - start;
    for (at, grapheme) in text.grapheme_indices(true) {
        if at <= offset && offset < at + grapheme.len() {
            return start + at + grapheme.len();
        }
    }
    end
}

/// The boundary before `byte`: one grapheme back, or the previous line's end at a line's start.
pub fn prev_grapheme(rope: &Rope, byte: usize) -> usize {
    let byte = byte.min(rope.len_bytes());
    if byte == 0 {
        return 0;
    }
    let line = line_of(rope, byte);
    let start = line_start(rope, line);
    let end = line_end(rope, line);
    if byte > end {
        // Inside the line break: back to the line's last character boundary.
        return end;
    }
    if byte == start {
        // At the line's start: back over the previous line's break.
        return line_end(rope, line - 1);
    }
    let text = line_text(rope, line);
    let offset = byte - start;
    let mut previous = 0;
    for (at, _) in text.grapheme_indices(true) {
        if at >= offset {
            break;
        }
        previous = at;
    }
    start + previous
}

/// The grapheme column `byte` sits at on its line, counting from zero.
pub fn grapheme_col(rope: &Rope, byte: usize) -> usize {
    let byte = byte.min(rope.len_bytes());
    let line = line_of(rope, byte);
    let start = line_start(rope, line);
    let offset = byte.saturating_sub(start);
    let text = line_text(rope, line);
    let mut col = 0;
    for (at, _) in text.grapheme_indices(true) {
        if at >= offset {
            return col;
        }
        col += 1;
    }
    col
}

/// The byte at grapheme column `col` of `line`, clamped to the line's end.
pub fn byte_at_col(rope: &Rope, line: usize, col: usize) -> usize {
    let start = line_start(rope, line);
    let text = line_text(rope, line);
    match text.grapheme_indices(true).nth(col) {
        Some((at, _)) => start + at,
        None => line_end(rope, line),
    }
}

/// The line and grapheme column of `byte`, for a status line.
pub fn line_col(rope: &Rope, byte: usize) -> (usize, usize) {
    (line_of(rope, byte), grapheme_col(rope, byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn a_line_ends_before_its_break() {
        let rope = rope("one\ntwo\nthree");
        assert_eq!(line_start(&rope, 0), 0);
        assert_eq!(line_end(&rope, 0), 3);
        assert_eq!(line_start(&rope, 1), 4);
        assert_eq!(line_end(&rope, 2), 13);
        assert_eq!(line_text(&rope, 1), "two");
    }

    #[test]
    fn a_crlf_break_is_two_bytes_of_break() {
        let rope = rope("one\r\ntwo");
        assert_eq!(line_end(&rope, 0), 3);
        assert_eq!(line_start(&rope, 1), 5);
        assert_eq!(line_text(&rope, 0), "one");
    }

    #[test]
    fn a_trailing_break_leaves_an_empty_last_line() {
        let rope = rope("one\n");
        assert_eq!(line_count(&rope), 2);
        assert_eq!(line_text(&rope, 1), "");
        assert_eq!(line_start(&rope, 1), 4);
        assert_eq!(line_end(&rope, 1), 4);
    }

    #[test]
    fn graphemes_step_one_cluster_at_a_time() {
        let rope = rope("ae\u{301}b");
        assert_eq!(next_grapheme(&rope, 0), 1);
        assert_eq!(
            next_grapheme(&rope, 1),
            4,
            "e + combining acute is one cluster"
        );
        assert_eq!(prev_grapheme(&rope, 4), 1);
        assert_eq!(prev_grapheme(&rope, 1), 0);
    }

    #[test]
    fn graphemes_step_over_line_breaks() {
        let rope = rope("ab\ncd");
        assert_eq!(
            next_grapheme(&rope, 2),
            3,
            "over the break to the next line"
        );
        assert_eq!(
            prev_grapheme(&rope, 3),
            2,
            "back over the break to the line end"
        );
    }

    #[test]
    fn graphemes_step_over_crlf_whole() {
        let rope = rope("ab\r\ncd");
        assert_eq!(next_grapheme(&rope, 2), 4);
        assert_eq!(prev_grapheme(&rope, 4), 2);
    }

    #[test]
    fn an_emoji_is_one_column() {
        let rope = rope("a\u{1F600}b");
        assert_eq!(grapheme_col(&rope, 0), 0);
        assert_eq!(grapheme_col(&rope, 1), 1);
        assert_eq!(grapheme_col(&rope, 5), 2);
        assert_eq!(byte_at_col(&rope, 0, 2), 5);
    }

    #[test]
    fn a_column_past_the_end_clamps_to_the_line_end() {
        let rope = rope("ab\ncd");
        assert_eq!(byte_at_col(&rope, 0, 99), 2);
        assert_eq!(byte_at_col(&rope, 1, 99), 5);
    }

    #[test]
    fn snapping_lands_on_a_boundary() {
        let rope = rope("e\u{301}x");
        assert_eq!(snap(&rope, 1), 0, "inside the cluster snaps to its start");
        assert_eq!(snap(&rope, 3), 3);
    }

    #[test]
    fn the_document_edges_hold() {
        let rope = rope("ab");
        assert_eq!(prev_grapheme(&rope, 0), 0);
        assert_eq!(next_grapheme(&rope, 2), 2);
    }
}
