//! Which glyphs a conceal decoration leaves out of a line.
//!
//! A concealed character keeps its place: the shaped line stays as it was, the glyphs of the
//! concealed clusters are not drawn, and the paint pass draws the mark in their place.

use std::ops::Range;

use crate::core::position;
use crate::render::shaping::{ShapedLine, TabMap};
use zgui::canvas::zgui_color::Color;

/// The parts of `line` that `concealed` covers, as byte ranges of the shaped (tab-expanded) line.
pub fn on_line(
    rope: &ropey::Rope,
    line: usize,
    concealed: &[(Range<usize>, char, Color)],
    shaped: &ShapedLine,
) -> Vec<Range<u32>> {
    if concealed.is_empty() {
        return Vec::new();
    }
    let start = position::line_start(rope, line);
    let end = position::line_end(rope, line);
    let ranges: Vec<Range<usize>> = concealed
        .iter()
        .map(|(range, _, _)| range.clone())
        .collect();
    expanded(start..end, &ranges, &shaped.tab_map)
}

/// The parts of the line at `line` (buffer bytes) that `ranges` cover, as byte ranges of the
/// line once `tabs` expanded it.
pub fn expanded(line: Range<usize>, ranges: &[Range<usize>], tabs: &TabMap) -> Vec<Range<u32>> {
    ranges
        .iter()
        .filter(|range| range.start < line.end && range.end > line.start)
        .map(|range| {
            let from = (range.start.max(line.start) - line.start) as u32;
            let to = (range.end.min(line.end) - line.start) as u32;
            tabs.to_expanded(from)..tabs.to_expanded(to)
        })
        .collect()
}

/// Where in `text`, a line starting at buffer byte `start`, the characters `range` covers
/// start, as bytes of the line. Each one takes one mark; a line break takes none.
pub fn marked(text: &str, start: usize, range: &Range<usize>) -> Vec<usize> {
    text.char_indices()
        .filter(|(local, character)| {
            let byte = start + local;
            byte >= range.start && byte < range.end && *character != '\n' && *character != '\r'
        })
        .map(|(local, _)| local)
        .collect()
}

/// The runs of glyph indices of `glyphs` that stay drawn: every glyph whose cluster, in
/// `clusters`, starts outside `hidden`.
pub fn shown(clusters: &[u32], glyphs: Range<u32>, hidden: &[Range<u32>]) -> Vec<Range<usize>> {
    let all = glyphs.start as usize..glyphs.end as usize;
    if hidden.is_empty() {
        return vec![all];
    }
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    for index in all.clone() {
        let cluster = clusters.get(index).copied().unwrap_or(0);
        let concealed = hidden.iter().any(|range| range.contains(&cluster));
        match (concealed, open) {
            (false, None) => open = Some(index),
            (true, Some(from)) => {
                out.push(from..index);
                open = None;
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        out.push(from..all.end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{expanded, marked, shown};
    use crate::render::shaping::expand_tabs;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn a_tab_before_the_value_moves_the_hidden_glyphs_with_it() {
        // `a\tb: pw`, tab stops of four: the value starts at byte 5 and at column 7.
        let (_, tabs) = expand_tabs("a\tb: pw", 4);
        assert_eq!(expanded(10..18, &[15..17], &tabs), vec![7..9]);
        assert_eq!(
            expanded(10..18, &[0..9], &tabs),
            Vec::<std::ops::Range<u32>>::new()
        );
    }

    #[test]
    fn every_character_takes_one_mark_whatever_its_bytes() {
        let text = "pw: pässwört";
        let value = 4..text.len();
        let marks = marked(text, 0, &value);
        assert_eq!(marks.len(), "pässwört".chars().count());
        assert_eq!(marks[..3], [4, 5, 7]);
        // A range that starts on a later line counts in bytes of the buffer.
        assert_eq!(marked("ab", 10, &(11..12)), [1]);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn a_concealed_cluster_splits_the_glyphs_around_it() {
        let held = [0, 1, 2, 3, 4];
        assert_eq!(shown(&held, 0..5, &[]), vec![0..5]);
        assert_eq!(shown(&held, 0..5, &[1..3]), vec![0..1, 3..5]);
        assert_eq!(
            shown(&held, 0..5, &[0..5]),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(shown(&held, 2..5, &[4..9]), vec![2..4]);
    }
}
