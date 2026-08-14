//! Keeping shaped lines between frames, and flattening tabs before the shaper sees them.
//!
//! Shaping is the expensive part of drawing text, and an editor draws the same lines over and
//! over. The cache here is the terminal's: keyed by the text and the metrics generation, holding
//! owned runs behind an `Rc` so a hit is a refcount bump, evicting half at a time.
//!
//! The shaper does not expand tabs, so the editor does: a `\t` becomes the spaces that reach the
//! next tab stop, and a [`TabMap`] carries byte offsets between the buffer's text and the
//! expanded text that was shaped.

use std::rc::Rc;

use compact_str::CompactString;
use rustc_hash::FxHashMap;
use zgui::app::Shaper;
use zgui::custom::ShapedRunOwned;
use zgui_interned::Ident;

use crate::render::metrics::TextMetrics;

/// How many entries the cache holds before it starts letting go.
const CAPACITY: usize = 8192;

/// One tab of the original text, and where its spaces sit in the expanded text.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TabStop {
    /// The byte of the `\t` in the buffer's line.
    buffer: u32,
    /// The byte its first space starts at in the expanded line.
    expanded: u32,
    /// How many spaces it became.
    width: u32,
}

/// Byte offsets carried between a buffer line and its tab-expanded shadow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabMap {
    stops: Vec<TabStop>,
}

impl TabMap {
    /// Where `buffer_byte` of the original line sits in the expanded line.
    pub fn to_expanded(&self, buffer_byte: u32) -> u32 {
        let mut shift = 0i64;
        for stop in &self.stops {
            if stop.buffer >= buffer_byte {
                break;
            }
            shift += i64::from(stop.width) - 1;
        }
        (i64::from(buffer_byte) + shift) as u32
    }

    /// Where `expanded_byte` of the expanded line sits in the original.
    ///
    /// A byte inside a tab's spaces answers the tab itself.
    pub fn to_buffer(&self, expanded_byte: u32) -> u32 {
        let mut shift = 0i64;
        for stop in &self.stops {
            if stop.expanded > expanded_byte {
                break;
            }
            if expanded_byte < stop.expanded + stop.width {
                return stop.buffer;
            }
            shift += i64::from(stop.width) - 1;
        }
        (i64::from(expanded_byte) - shift) as u32
    }

    /// Whether the line had any tabs at all.
    pub fn is_identity(&self) -> bool {
        self.stops.is_empty()
    }
}

/// `text` with every tab flattened to spaces reaching the next stop of `tab_width` columns.
///
/// Columns are counted in characters, which is the approximation every monospace editor makes;
/// a wide character before a tab shifts the stop by half a cell and nothing breaks.
pub fn expand_tabs(text: &str, tab_width: u32) -> (CompactString, TabMap) {
    if !text.contains('\t') {
        return (CompactString::from(text), TabMap::default());
    }
    let tab_width = tab_width.max(1) as usize;
    let mut expanded = CompactString::default();
    let mut stops = Vec::new();
    let mut column = 0usize;
    for (byte, character) in text.char_indices() {
        if character == '\t' {
            let width = tab_width - (column % tab_width);
            stops.push(TabStop {
                buffer: byte as u32,
                expanded: expanded.len() as u32,
                width: width as u32,
            });
            for _ in 0..width {
                expanded.push(' ');
            }
            column += width;
        } else {
            expanded.push(character);
            column += 1;
        }
    }
    (expanded, TabMap { stops })
}

/// One shaped line: its runs, its width, and the map back to the buffer's bytes.
#[derive(Clone, Debug)]
pub struct ShapedLine {
    /// The style-uniform runs, in visual order, with cluster bytes into the *expanded* text.
    pub runs: Rc<Vec<ShapedRunOwned>>,
    /// Offsets between buffer bytes and expanded bytes.
    pub tab_map: Rc<TabMap>,
    /// How long the expanded text is, in bytes.
    pub expanded_len: u32,
    /// The advance of the whole line, in device pixels.
    pub width: f32,
}

/// What decides whether two lines shape the same way.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct LineKey {
    text: CompactString,
    generation: u32,
}

struct Entry {
    line: ShapedLine,
    used: u64,
}

/// Shapes lines, and keeps what it shaped.
pub struct ShapingCache {
    entries: FxHashMap<LineKey, Entry>,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl ShapingCache {
    /// A cache with nothing in it.
    pub fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Lets go of everything, which a change of face, size or scale makes necessary.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Starts a new frame, so recency means frames rather than calls.
    pub fn tick(&mut self) {
        self.clock += 1;
    }

    /// How often the cache answered without shaping, and how often it had to shape.
    pub fn counts(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// The shaped form of one buffer line.
    ///
    /// The end-of-line advance comes from shaping a sentinel space after the text and reading
    /// where it starts — the shaper reports glyph origins, not advances, and the sentinel's
    /// origin *is* the total advance. The sentinel's glyphs are stripped from the kept runs.
    pub fn shape(
        &mut self,
        shaper: &mut Shaper,
        families: &[Ident],
        text: &str,
        metrics: &TextMetrics,
    ) -> ShapedLine {
        let key = LineKey {
            text: text.into(),
            generation: metrics.generation,
        };
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.clock;
            self.hits += 1;
            return entry.line.clone();
        }
        self.misses += 1;

        let (expanded, tab_map) = expand_tabs(text, metrics.tab_width);
        let expanded_len = expanded.len() as u32;
        let mut with_sentinel = String::with_capacity(expanded.len() + 1);
        with_sentinel.push_str(&expanded);
        with_sentinel.push(' ');

        let request = metrics.line_request(families, false, false);
        let mut runs = shaper.shape_line(&with_sentinel, &request);

        // The sentinel starts where the real text's advance ends. In a bidirectional line it
        // may not be the visually last glyph, so the fallback is the widest edge seen.
        let mut width = 0.0f32;
        for run in &mut runs {
            let mut keep = run.glyphs.len();
            for (index, cluster) in run.clusters.iter().enumerate() {
                if *cluster >= expanded_len {
                    width = width.max(run.glyphs[index].x);
                    keep = keep.min(index);
                }
            }
            if keep < run.glyphs.len() {
                run.glyphs.truncate(keep);
                run.clusters.truncate(keep);
            }
        }
        for run in &runs {
            for glyph in &run.glyphs {
                width = width.max(glyph.x + metrics.cell_advance);
            }
        }
        let runs: Vec<ShapedRunOwned> = runs
            .into_iter()
            .filter(|run| !run.glyphs.is_empty())
            .collect();

        let line = ShapedLine {
            runs: Rc::new(runs),
            tab_map: Rc::new(tab_map),
            expanded_len,
            width,
        };
        if self.entries.len() >= CAPACITY {
            self.evict();
        }
        self.entries.insert(
            key,
            Entry {
                line: line.clone(),
                used: self.clock,
            },
        );
        line
    }

    /// Lets go of the entries that have gone longest without being drawn.
    fn evict(&mut self) {
        let mut ages: Vec<u64> = self.entries.values().map(|entry| entry.used).collect();
        ages.sort_unstable();
        // Half of them, so the cost of doing this is paid once rather than on every insert.
        let cutoff = ages.get(ages.len() / 2).copied().unwrap_or(0);
        self.entries.retain(|_, entry| entry.used > cutoff);
    }
}

impl Default for ShapingCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_without_tabs_maps_to_itself() {
        let (expanded, map) = expand_tabs("plain text", 4);
        assert_eq!(expanded, "plain text");
        assert!(map.is_identity());
        assert_eq!(map.to_expanded(5), 5);
        assert_eq!(map.to_buffer(5), 5);
    }

    #[test]
    fn a_tab_reaches_the_next_stop() {
        let (expanded, map) = expand_tabs("a\tb", 4);
        assert_eq!(expanded, "a   b");
        assert_eq!(map.to_expanded(0), 0);
        assert_eq!(map.to_expanded(2), 4, "the byte after the tab");
        assert_eq!(map.to_buffer(4), 2);
        assert_eq!(map.to_buffer(2), 1, "inside the tab's spaces is the tab");
    }

    #[test]
    fn a_leading_tab_is_a_full_stop_wide() {
        let (expanded, map) = expand_tabs("\tx", 8);
        assert_eq!(expanded, "        x");
        assert_eq!(map.to_expanded(1), 8);
        assert_eq!(map.to_buffer(8), 1);
    }

    #[test]
    fn consecutive_tabs_each_reach_a_stop() {
        let (expanded, _) = expand_tabs("\t\ta", 4);
        assert_eq!(expanded, "        a");
    }

    #[test]
    fn round_trips_hold_for_every_boundary() {
        let text = "fn\tmain()\t{";
        let (expanded, map) = expand_tabs(text, 4);
        for (byte, _) in text.char_indices() {
            let there = map.to_expanded(byte as u32);
            assert!(there < expanded.len() as u32 + 1);
            assert_eq!(map.to_buffer(there), byte as u32, "byte {byte}");
        }
    }
}
