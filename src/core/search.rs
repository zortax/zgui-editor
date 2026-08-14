//! Finding plain text in the rope, without materializing the file.
//!
//! Plain-text search is what `/` needs and what an incremental search bar starts from. The
//! needle is matched bytewise across chunk boundaries, so a ten-million-line file is searched
//! without one big allocation.

use ropey::Rope;

/// Which way to look.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchDirection {
    /// Toward the end of the buffer.
    #[default]
    Forward,
    /// Toward the start.
    Backward,
}

/// The first match for `needle` from `from`, looking `direction`, wrapping when asked.
///
/// A forward search begins at `from`; a backward one ends before `from`. The answer is the byte
/// range of the match. An empty needle matches nothing.
pub fn find(
    rope: &Rope,
    needle: &str,
    from: usize,
    direction: SearchDirection,
    wrap: bool,
) -> Option<std::ops::Range<usize>> {
    if needle.is_empty() || rope.len_bytes() == 0 {
        return None;
    }
    let from = from.min(rope.len_bytes());
    match direction {
        SearchDirection::Forward => find_forward(rope, needle, from)
            .or_else(|| wrap.then(|| find_forward(rope, needle, 0)).flatten()),
        SearchDirection::Backward => find_backward(rope, needle, from).or_else(|| {
            wrap.then(|| find_backward(rope, needle, rope.len_bytes()))
                .flatten()
        }),
    }
}

/// Every match in `range`, for highlighting what is on screen.
pub fn find_in(
    rope: &Rope,
    needle: &str,
    range: std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    let mut matches = Vec::new();
    if needle.is_empty() {
        return matches;
    }
    let mut at = range.start;
    while let Some(found) = find_forward(rope, needle, at) {
        if found.start >= range.end {
            break;
        }
        at = found.start + 1;
        matches.push(found);
    }
    matches
}

/// The first match at or after `from`.
fn find_forward(rope: &Rope, needle: &str, from: usize) -> Option<std::ops::Range<usize>> {
    let needle = needle.as_bytes();
    let len = rope.len_bytes();
    if from >= len {
        return None;
    }
    // Matched bytewise with a running list of partial-match candidates. Worst case is
    // O(len * needle) with a pathological needle, but the list stays a few entries deep on
    // real text and nothing is allocated per byte.
    let mut candidates: Vec<usize> = Vec::new();
    for (offset, byte) in rope.bytes_at(from).enumerate() {
        let at = from + offset;
        candidates.retain_mut(|matched| {
            if needle[*matched] == byte {
                *matched += 1;
                true
            } else {
                false
            }
        });
        if byte == needle[0] {
            candidates.push(1);
        }
        // Every candidate completing here describes the same range, ending at this byte.
        if candidates.contains(&needle.len()) {
            let end = at + 1;
            return Some(end - needle.len()..end);
        }
    }
    None
}

/// The last match strictly before `from`.
fn find_backward(rope: &Rope, needle: &str, from: usize) -> Option<std::ops::Range<usize>> {
    // Walk forward through the prefix keeping the last match seen. Search is rare next to
    // painting, and this keeps one matcher correct instead of two.
    let mut last = None;
    let mut at = 0;
    while let Some(found) = find_forward(rope, needle, at) {
        if found.end > from {
            break;
        }
        at = found.start + 1;
        last = Some(found);
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn a_match_is_found_forward() {
        let rope = rope("one two one");
        assert_eq!(
            find(&rope, "one", 0, SearchDirection::Forward, false),
            Some(0..3)
        );
        assert_eq!(
            find(&rope, "one", 1, SearchDirection::Forward, false),
            Some(8..11)
        );
    }

    #[test]
    fn a_search_can_wrap() {
        let rope = rope("one two");
        assert_eq!(find(&rope, "one", 5, SearchDirection::Forward, false), None);
        assert_eq!(
            find(&rope, "one", 5, SearchDirection::Forward, true),
            Some(0..3)
        );
    }

    #[test]
    fn a_match_is_found_backward() {
        let rope = rope("one two one");
        assert_eq!(
            find(&rope, "one", 11, SearchDirection::Backward, false),
            Some(8..11)
        );
        assert_eq!(
            find(&rope, "one", 8, SearchDirection::Backward, false),
            Some(0..3)
        );
    }

    #[test]
    fn overlapping_candidates_are_found() {
        let rope = rope("aaab");
        assert_eq!(
            find(&rope, "aab", 0, SearchDirection::Forward, false),
            Some(1..4)
        );
    }

    #[test]
    fn every_match_in_a_window_is_reported() {
        let rope = rope("x ax ax a");
        assert_eq!(find_in(&rope, "x a", 0..9), vec![0..3, 3..6, 6..9]);
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        let rope = rope("abc");
        assert_eq!(find(&rope, "", 0, SearchDirection::Forward, true), None);
    }
}
