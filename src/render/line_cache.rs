//! What each visible line costs once, and answers many times.
//!
//! A cached line is the shaped runs plus the colour slices painting draws them in, stamped with
//! everything that went into it: the buffer revision, the metrics generation, the highlight
//! version and the theme version. A stamp that no longer matches is a line rebuilt; a line off
//! the screen long enough is a line let go.
//!
//! The cluster bytes of the shaped runs are also the caret geometry: [`caret_x`] and
//! [`hit_byte`] answer the two questions a caret ever asks of shaped text.

use std::ops::Range;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use zgui::canvas::zgui_color::Color;

use crate::render::shaping::ShapedLine;

/// One same-coloured stretch of one run's glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    /// Which run of the line.
    pub run: u16,
    /// Which of its glyphs.
    pub glyphs: Range<u32>,
    /// The colour they are drawn in.
    pub color: Color,
}

/// One line, ready to paint.
#[derive(Clone, Debug)]
pub struct CachedLine {
    /// The buffer revision the text was read at.
    pub revision: u64,
    /// The metrics generation the runs were shaped at.
    pub generation: u32,
    /// The highlight version the slices were coloured at.
    pub hl_version: u64,
    /// The theme version the slices were coloured at.
    pub theme_version: u32,
    /// The shaped text.
    pub shaped: ShapedLine,
    /// The colour slices, covering every glyph of every run.
    pub slices: SmallVec<[Slice; 8]>,
}

/// The cached lines, by line index.
#[derive(Default)]
pub struct LineCache {
    map: FxHashMap<usize, CachedLine>,
}

/// How many viewports of lines are kept around the one on screen.
const BUDGET_VIEWPORTS: usize = 4;

impl LineCache {
    /// The cached line, whatever its stamps say.
    pub fn get(&self, line: usize) -> Option<&CachedLine> {
        self.map.get(&line)
    }

    /// Stores `cached` for `line`.
    pub fn put(&mut self, line: usize, cached: CachedLine) {
        self.map.insert(line, cached);
    }

    /// Lets go of every line at or after `line` — an edit there may have moved them all.
    pub fn invalidate_from(&mut self, line: usize) {
        self.map.retain(|held, _| *held < line);
    }

    /// Lets go of the lines from `first` through `last` alone — the shape of an edit that
    /// neither added nor removed a line, which moves nothing below itself.
    pub fn invalidate_range(&mut self, first: usize, last: usize) {
        self.map.retain(|held, _| *held < first || *held > last);
    }

    /// Lets go of everything.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Keeps the cache within its budget, dropping the lines furthest from `center`.
    pub fn trim(&mut self, center: usize, viewport_lines: usize) {
        let budget = (viewport_lines.max(10) * BUDGET_VIEWPORTS).max(64);
        if self.map.len() <= budget {
            return;
        }
        let mut distances: Vec<usize> = self.map.keys().map(|line| line.abs_diff(center)).collect();
        distances.sort_unstable();
        let cutoff = distances[budget.saturating_sub(1)];
        self.map.retain(|line, _| line.abs_diff(center) <= cutoff);
    }
}

/// The colour slices for `shaped`, given `spans` over the buffer line's bytes.
///
/// `spans` are `(start, end, color)` in **buffer** byte offsets of the line, sorted by start and
/// non-overlapping. Every glyph outside every span is drawn in `default`. Glyphs are coloured
/// one at a time by their cluster byte and merged when neighbours agree, which is what makes a
/// right-to-left run — whose cluster bytes descend — come out right without being special.
pub fn build_slices(
    shaped: &ShapedLine,
    spans: &[(u32, u32, Color)],
    default: Color,
) -> SmallVec<[Slice; 8]> {
    let mut slices: SmallVec<[Slice; 8]> = SmallVec::new();
    for (run_index, run) in shaped.runs.iter().enumerate() {
        let mut open: Option<Slice> = None;
        for (glyph_index, cluster) in run.clusters.iter().enumerate() {
            let buffer_byte = shaped.tab_map.to_buffer(*cluster);
            let color = color_at(spans, buffer_byte, default);
            match open.as_mut() {
                Some(slice) if slice.color == color => {
                    slice.glyphs.end = glyph_index as u32 + 1;
                }
                _ => {
                    if let Some(done) = open.take() {
                        slices.push(done);
                    }
                    open = Some(Slice {
                        run: run_index as u16,
                        glyphs: glyph_index as u32..glyph_index as u32 + 1,
                        color,
                    });
                }
            }
        }
        if let Some(done) = open {
            slices.push(done);
        }
    }
    slices
}

/// The colour `spans` give the glyph whose cluster starts at `byte`.
fn color_at(spans: &[(u32, u32, Color)], byte: u32, default: Color) -> Color {
    let index = spans.partition_point(|(start, _, _)| *start <= byte);
    if index > 0 {
        let (_, end, color) = spans[index - 1];
        if byte < end {
            return color;
        }
    }
    default
}

/// Where a caret at `buffer_byte` of this line sits, in device pixels from the line's left edge.
pub fn caret_x(shaped: &ShapedLine, buffer_byte: u32) -> f32 {
    let expanded = shaped.tab_map.to_expanded(buffer_byte);
    if expanded >= shaped.expanded_len {
        return shaped.width;
    }
    // The glyph anchoring the cluster at or before the byte; exact matches win.
    let mut best: Option<(u32, f32)> = None;
    for run in shaped.runs.iter() {
        for (index, cluster) in run.clusters.iter().enumerate() {
            if *cluster == expanded {
                return run.glyphs[index].x;
            }
            if *cluster < expanded {
                let x = run.glyphs[index].x;
                if best.map(|(held, _)| *cluster > held).unwrap_or(true) {
                    best = Some((*cluster, x));
                }
            }
        }
    }
    best.map(|(_, x)| x).unwrap_or(0.0)
}

/// The buffer byte a click at `x` device pixels from the line's left edge lands between.
///
/// The answer is the nearest caret boundary: the click's own cluster when it is in the left
/// half, the next when it is in the right, and the line's end past the last glyph.
pub fn hit_byte(shaped: &ShapedLine, x: f32) -> u32 {
    let mut best_byte = shaped.tab_map.to_buffer(shaped.expanded_len);
    let mut best_distance = (shaped.width - x).abs();
    for run in shaped.runs.iter() {
        for (index, cluster) in run.clusters.iter().enumerate() {
            let distance = (run.glyphs[index].x - x).abs();
            if distance < best_distance {
                best_distance = distance;
                best_byte = shaped.tab_map.to_buffer(*cluster);
            }
        }
    }
    best_byte
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use zgui::custom::{FaceId, ShapedGlyph, ShapedRunOwned};

    use super::*;
    use crate::render::shaping::TabMap;

    fn line(clusters: Vec<u32>, xs: Vec<f32>, width: f32) -> ShapedLine {
        let glyphs = xs
            .iter()
            .map(|x| ShapedGlyph {
                glyph: 0,
                x: *x,
                y: 12.0,
            })
            .collect();
        ShapedLine {
            runs: Rc::new(vec![ShapedRunOwned {
                face: FaceId(0),
                size: 16.0,
                synthetic_bold: 0.0,
                synthetic_slant: 0.0,
                has_color: false,
                glyphs,
                clusters: clusters.clone(),
            }]),
            tab_map: Rc::new(TabMap::default()),
            expanded_len: clusters
                .iter()
                .copied()
                .max()
                .map(|last| last + 1)
                .unwrap_or(0),
            width,
        }
    }

    fn color(value: f32) -> Color {
        Color::srgb(value, 0.0, 0.0, 1.0)
    }

    #[test]
    fn slices_merge_neighbours_of_one_colour() {
        let shaped = line(vec![0, 1, 2, 3], vec![0.0, 8.0, 16.0, 24.0], 32.0);
        let spans = [(1u32, 3u32, color(0.5))];
        let slices = build_slices(&shaped, &spans, color(0.0));
        assert_eq!(slices.len(), 3);
        assert_eq!(slices[0].glyphs, 0..1);
        assert_eq!(slices[1].glyphs, 1..3);
        assert_eq!(slices[1].color, color(0.5));
        assert_eq!(slices[2].glyphs, 3..4);
    }

    #[test]
    fn a_descending_rtl_run_still_slices_by_colour() {
        let shaped = line(vec![3, 2, 1, 0], vec![0.0, 8.0, 16.0, 24.0], 32.0);
        let spans = [(0u32, 2u32, color(0.5))];
        let slices = build_slices(&shaped, &spans, color(0.0));
        assert_eq!(slices.len(), 2, "two visual stretches");
        assert_eq!(slices[0].color, color(0.0));
        assert_eq!(slices[1].color, color(0.5));
    }

    #[test]
    fn the_caret_sits_on_cluster_starts_and_the_line_end() {
        let shaped = line(vec![0, 1, 2], vec![0.0, 8.0, 16.0], 24.0);
        assert_eq!(caret_x(&shaped, 0), 0.0);
        assert_eq!(caret_x(&shaped, 2), 16.0);
        assert_eq!(
            caret_x(&shaped, 3),
            24.0,
            "past the last glyph is the width"
        );
    }

    #[test]
    fn clicks_land_on_the_nearest_boundary() {
        let shaped = line(vec![0, 1, 2], vec![0.0, 8.0, 16.0], 24.0);
        assert_eq!(hit_byte(&shaped, 3.0), 0);
        assert_eq!(
            hit_byte(&shaped, 5.0),
            1,
            "the right half belongs to the next"
        );
        assert_eq!(hit_byte(&shaped, 100.0), 3, "far right is the line end");
    }
}
