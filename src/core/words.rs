//! What a word is, for word motions and for a double click.
//!
//! Vim's small words know three classes — word characters, punctuation, and whitespace — and a
//! run of one class is one word. Big words (`W`, `B`, `E`) know only whitespace and everything
//! else. Both are defined here over graphemes, so a motion never lands inside a cluster.

use std::borrow::Cow;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use crate::core::position;

/// Which of vim's three classes a grapheme belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    /// Spaces, tabs, and other blank space.
    Whitespace,
    /// Letters, digits, and the underscore.
    Word,
    /// Everything else.
    Punctuation,
}

/// The class of `grapheme`, judged by its first character.
pub fn class_of(grapheme: &str) -> CharClass {
    let Some(first) = grapheme.chars().next() else {
        return CharClass::Whitespace;
    };
    if first.is_whitespace() {
        CharClass::Whitespace
    } else if first.is_alphanumeric() || first == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// The class of `grapheme` when only whitespace divides words.
pub fn big_class_of(grapheme: &str) -> CharClass {
    if class_of(grapheme) == CharClass::Whitespace {
        CharClass::Whitespace
    } else {
        CharClass::Word
    }
}

/// The graphemes of the line `byte` is on, with their byte offsets, and where the line starts.
fn line_graphemes(rope: &Rope, byte: usize) -> (usize, Cow<'_, str>) {
    let line = position::line_of(rope, byte);
    (
        position::line_start(rope, line),
        position::line_text(rope, line),
    )
}

/// The word (or punctuation run) around `byte`, for a double click and for `viw`.
///
/// On whitespace the run of whitespace is the answer, which is what a double click selects.
pub fn word_at(rope: &Rope, byte: usize) -> std::ops::Range<usize> {
    let byte = byte.min(rope.len_bytes());
    let (start, text) = line_graphemes(rope, byte);
    if text.is_empty() {
        return byte..byte;
    }
    let offset = (byte - start).min(text.len().saturating_sub(1));
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let here = graphemes
        .iter()
        .rposition(|(at, _)| *at <= offset)
        .unwrap_or(0);
    let class = class_of(graphemes[here].1);
    let mut from = here;
    while from > 0 && class_of(graphemes[from - 1].1) == class {
        from -= 1;
    }
    let mut to = here;
    while to + 1 < graphemes.len() && class_of(graphemes[to + 1].1) == class {
        to += 1;
    }
    let word_start = start + graphemes[from].0;
    let word_end = start + graphemes[to].0 + graphemes[to].1.len();
    word_start..word_end
}

/// The start of the next word after `byte` — vim's `w` and `W`.
pub fn next_word_start(rope: &Rope, byte: usize, big: bool) -> usize {
    let class = if big { big_class_of } else { class_of };
    let len = rope.len_bytes();
    let mut at = byte.min(len);
    let Some(mut current) = grapheme_at(rope, at).map(|g| class(&g)) else {
        return len;
    };
    // Leave the run the cursor is in, then any whitespace after it.
    loop {
        let next = position::next_grapheme(rope, at);
        if next == at || next >= len {
            return len;
        }
        at = next;
        let Some(grapheme) = grapheme_at(rope, at) else {
            return len;
        };
        let this = class(&grapheme);
        if this != current {
            if this == CharClass::Whitespace {
                current = this;
                continue;
            }
            return at;
        }
        // An empty line is a word of its own to vim; crossing onto one stops there.
        if this == CharClass::Whitespace {
            let line = position::line_of(rope, at);
            if position::line_start(rope, line) == at && position::line_end(rope, line) == at {
                return at;
            }
        }
    }
}

/// The start of the word `byte` is in, or of the word before — vim's `b` and `B`.
pub fn prev_word_start(rope: &Rope, byte: usize, big: bool) -> usize {
    let class = if big { big_class_of } else { class_of };
    let mut at = byte.min(rope.len_bytes());
    // Step back at least once, then over whitespace, then to the start of that run.
    loop {
        let prev = position::prev_grapheme(rope, at);
        if prev == at {
            return at;
        }
        at = prev;
        let Some(grapheme) = grapheme_at(rope, at) else {
            continue;
        };
        if class(&grapheme) != CharClass::Whitespace {
            break;
        }
        // An empty line is a stop of its own.
        let line = position::line_of(rope, at);
        if position::line_start(rope, line) == at && position::line_end(rope, line) == at {
            return at;
        }
        if at == 0 {
            return 0;
        }
    }
    let current = grapheme_at(rope, at).map(|g| class(&g));
    loop {
        let prev = position::prev_grapheme(rope, at);
        if prev == at {
            return at;
        }
        match grapheme_at(rope, prev) {
            Some(grapheme) if Some(class(&grapheme)) == current => at = prev,
            _ => return at,
        }
    }
}

/// The end of the word at or after `byte` — vim's `e` and `E`.
pub fn word_end(rope: &Rope, byte: usize, big: bool) -> usize {
    let class = if big { big_class_of } else { class_of };
    let len = rope.len_bytes();
    let mut at = byte.min(len);
    // Step forward at least once, then over whitespace.
    loop {
        let next = position::next_grapheme(rope, at);
        if next == at {
            return at;
        }
        at = next;
        match grapheme_at(rope, at) {
            Some(grapheme) if class(&grapheme) != CharClass::Whitespace => break,
            Some(_) => continue,
            None => return position::prev_grapheme(rope, len).max(byte),
        }
    }
    let current = grapheme_at(rope, at).map(|g| class(&g));
    loop {
        let next = position::next_grapheme(rope, at);
        if next == at {
            return at;
        }
        match grapheme_at(rope, next) {
            Some(grapheme) if Some(class(&grapheme)) == current => at = next,
            _ => return at,
        }
    }
}

/// The grapheme starting at `byte`, when one does; a line break reads as whitespace.
fn grapheme_at(rope: &Rope, byte: usize) -> Option<Cow<'_, str>> {
    if byte >= rope.len_bytes() {
        return None;
    }
    let line = position::line_of(rope, byte);
    let start = position::line_start(rope, line);
    let end = position::line_end(rope, line);
    if byte >= end {
        return Some(Cow::from("\n"));
    }
    let text = position::line_text(rope, line);
    let offset = byte - start;
    let grapheme = text
        .grapheme_indices(true)
        .find(|(at, _)| *at == offset)
        .map(|(_, grapheme)| grapheme.to_string())?;
    Some(Cow::from(grapheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn a_double_click_takes_the_word_under_the_pointer() {
        let rope = rope("one two.three");
        assert_eq!(word_at(&rope, 1), 0..3);
        assert_eq!(word_at(&rope, 4), 4..7);
        assert_eq!(word_at(&rope, 7), 7..8, "punctuation is its own run");
        assert_eq!(word_at(&rope, 9), 8..13);
    }

    #[test]
    fn w_stops_at_each_class_change() {
        let rope = rope("one two.three");
        assert_eq!(next_word_start(&rope, 0, false), 4);
        assert_eq!(next_word_start(&rope, 4, false), 7);
        assert_eq!(next_word_start(&rope, 7, false), 8);
    }

    #[test]
    fn big_w_stops_only_at_whitespace() {
        let rope = rope("one two.three four");
        assert_eq!(next_word_start(&rope, 0, true), 4);
        assert_eq!(next_word_start(&rope, 4, true), 14);
    }

    #[test]
    fn w_crosses_lines() {
        let rope = rope("one\ntwo");
        assert_eq!(next_word_start(&rope, 0, false), 4);
    }

    #[test]
    fn w_stops_on_an_empty_line() {
        let rope = rope("one\n\ntwo");
        assert_eq!(next_word_start(&rope, 0, false), 4);
        assert_eq!(next_word_start(&rope, 4, false), 5);
    }

    #[test]
    fn b_goes_back_to_word_starts() {
        let rope = rope("one two.three");
        assert_eq!(prev_word_start(&rope, 13, false), 8);
        assert_eq!(prev_word_start(&rope, 8, false), 7);
        assert_eq!(prev_word_start(&rope, 7, false), 4);
        assert_eq!(prev_word_start(&rope, 4, false), 0);
        assert_eq!(
            prev_word_start(&rope, 2, false),
            0,
            "inside a word goes to its start"
        );
    }

    #[test]
    fn e_goes_to_word_ends() {
        let rope = rope("one two.three");
        assert_eq!(
            word_end(&rope, 0, false),
            2,
            "to the last character of the word"
        );
        assert_eq!(word_end(&rope, 2, false), 6);
        assert_eq!(word_end(&rope, 6, false), 7);
    }
}
