//! Which glyphs a conceal decoration leaves out of a line.
//!
//! A concealed character keeps its place: the shaped line stays as it was, the glyphs of the
//! concealed clusters are not drawn, and the paint pass draws the mark in their place.

use std::ops::Range;

use crate::core::position;
use crate::render::shaping::ShapedLine;
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
    concealed
        .iter()
        .filter(|(range, _, _)| range.start < end && range.end > start)
        .map(|(range, _, _)| {
            let from = (range.start.max(start) - start) as u32;
            let to = (range.end.min(end) - start) as u32;
            shaped.tab_map.to_expanded(from)..shaped.tab_map.to_expanded(to)
        })
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
    use super::shown;

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
