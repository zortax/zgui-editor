//! Room an application reserves between lines, for content of its own.
//!
//! A block is a band of empty space under one line, as tall as the application says. The editor
//! draws nothing in it and puts no text there: the lines below move down to make the room, and
//! everything that maps lines to places on the screen — painting, the gutter, carets, hit
//! testing, scrolling, the scrollbar, a window's height — counts the room as part of the content.
//! What fills it is any view the application likes: a review thread under the line it is about,
//! a rendered image, an inline diff.
//!
//! ```no_run
//! # use zgui_editor::{Block, BlockId, EditorHandle};
//! # fn example(handle: &EditorHandle) {
//! // Room for a comment thread under line 12, 80 CSS pixels tall.
//! handle.set_blocks("review", vec![Block::new(BlockId(7), 12, 80.0)]);
//! // Where it is on screen right now, in the editor's own box.
//! let rects = handle.blocks();
//! # let _ = rects;
//! # }
//! ```
//!
//! # Putting content in the room
//!
//! [`EditorHandle::blocks`](crate::EditorHandle::blocks) publishes the rectangle of every block on
//! screen, in CSS pixels from the corner of the editor's padding box, and moves with every scroll
//! step. The children given to the [`Editor`](crate::Editor) are mounted in a layer over the text
//! that is exactly that box, so a child placed with `position: absolute` at a rectangle's `top`
//! and `left` sits on its block. The layer takes no pointer events itself, and its direct children
//! take them again. Wheel turns over a child scroll the editor; presses and keys inside a child
//! stay the child's.
//!
//! The application measures what it mounted and gives the height back through
//! [`set_blocks`](crate::EditorHandle::set_blocks): a block can start at zero, so its content is
//! mounted and measured before any room is made for it. A block above the view that grows or
//! shrinks keeps the text at the top of the view where it is.
//!
//! # Edits
//!
//! A block follows its line through edits: lines added or removed above it move it, and a block
//! whose line is removed goes to where the edit left the text. Replacing the whole text keeps every
//! block where it was, clamped to the new last line.

use std::ops::Range;

/// What an application calls one block. Chosen by the application, unique across layers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BlockId(pub u64);

/// Room under one line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Block {
    /// What the application calls it.
    pub id: BlockId,
    /// The line it sits under, counting from zero. Past the last line, it sits under the last.
    pub after_line: usize,
    /// How tall it is, in CSS pixels. Rounded to whole device pixels, so the lines below stay on
    /// the pixel grid. A negative height is no height.
    pub height: f32,
}

impl Block {
    /// Room `height` CSS pixels tall under `after_line`.
    pub fn new(id: BlockId, after_line: usize, height: f32) -> Self {
        Self {
            id,
            after_line,
            height,
        }
    }
}

/// Where one block is on screen, in CSS pixels from the corner of the editor's padding box.
///
/// The box the children of the [`Editor`](crate::Editor) are laid out in, so a child placed with
/// `position: absolute` at `left: x; top: y` sits on the block.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlockRect {
    /// Which block.
    pub id: BlockId,
    /// The line it sits under, after edits have moved it.
    pub line: usize,
    /// The left edge: where the text area starts, right of the gutter.
    pub x: f32,
    /// The top edge. Negative while the block is partly scrolled out above.
    pub y: f32,
    /// From the left edge to the right edge of the text area, clear of the scrollbar.
    pub width: f32,
    /// The room it takes, in whole device pixels written as CSS pixels.
    pub height: f32,
}

/// Where a vertical position in the content falls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Spot {
    /// On this line. Past the last line counts as a line past it; the caller clamps.
    Line(usize),
    /// Inside the block at this index of the sorted list.
    Block(usize),
}

/// One block as it is laid out: its line clamped, its height in device pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Placed {
    /// What the application calls it.
    pub id: BlockId,
    /// The line it sits under, clamped to the text.
    pub after_line: usize,
    /// How tall it is, in whole device pixels.
    pub height: f64,
}

/// One named group of blocks, as the application gave them.
struct BlockLayer {
    /// What the application called it.
    name: String,
    /// What is in it.
    blocks: Vec<Block>,
}

/// Every block of a view, by layer, and the sorted list the geometry reads.
///
/// The list is sorted by line, and `prefix` holds the running height in front of each entry, so
/// every question the view asks — how much room is above a line, which block or line a position
/// falls in — is a binary search. Rebuilt when a layer changes, when an edit moves blocks, and
/// when the scale or the number of lines changes.
pub(crate) struct BlockMap {
    /// The layers, in the order they were first named.
    layers: Vec<BlockLayer>,
    /// Every block, sorted by line; on one line, in layer order and then in the order given.
    placed: Vec<Placed>,
    /// `prefix[i]` is the height of `placed[..i]`, in device pixels. One longer than `placed`.
    prefix: Vec<f64>,
    /// The scale the heights were rounded at.
    scale: f32,
    /// How many lines the text had when the lines were clamped.
    line_count: usize,
}

impl Default for BlockMap {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            placed: Vec::new(),
            prefix: vec![0.0],
            scale: 1.0,
            line_count: 1,
        }
    }
}

impl BlockMap {
    /// Replaces the layer called `name`, adding it at the end if it is new.
    ///
    /// Answers whether anything is different. The list is rebuilt at `scale` for a text of
    /// `line_count` lines.
    pub fn set(&mut self, name: &str, blocks: Vec<Block>, scale: f32, line_count: usize) -> bool {
        match self.layers.iter_mut().find(|layer| layer.name == name) {
            Some(layer) => {
                if layer.blocks == blocks {
                    return false;
                }
                layer.blocks = blocks;
            }
            None => {
                if blocks.is_empty() {
                    return false;
                }
                self.layers.push(BlockLayer {
                    name: name.to_owned(),
                    blocks,
                });
            }
        }
        self.rebuild(scale, line_count);
        true
    }

    /// Removes the layer called `name`. Answers whether there was one with anything in it.
    pub fn clear(&mut self, name: &str, scale: f32, line_count: usize) -> bool {
        let before = self.placed.len();
        let had = self.layers.iter().any(|layer| layer.name == name);
        self.layers.retain(|layer| layer.name != name);
        if !had {
            return false;
        }
        self.rebuild(scale, line_count);
        self.placed.len() != before
    }

    /// Whether there is no room reserved anywhere.
    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Every block, sorted by line.
    pub fn placed(&self) -> &[Placed] {
        &self.placed
    }

    /// Rebuilds the list when the scale or the number of lines is not the one it was built for.
    pub fn ensure(&mut self, scale: f32, line_count: usize) {
        if self.scale != scale || self.line_count != line_count {
            self.rebuild(scale, line_count);
        }
    }

    /// Sorts the blocks, clamps their lines, rounds their heights and sums them.
    fn rebuild(&mut self, scale: f32, line_count: usize) {
        self.scale = scale;
        self.line_count = line_count;
        let last = line_count.saturating_sub(1);
        self.placed.clear();
        for layer in &self.layers {
            for block in &layer.blocks {
                self.placed.push(Placed {
                    id: block.id,
                    after_line: block.after_line.min(last),
                    height: device_height(block.height, scale),
                });
            }
        }
        // Stable, so blocks under one line keep the layer order and then the order given.
        self.placed.sort_by_key(|placed| placed.after_line);
        self.prefix.clear();
        self.prefix.push(0.0);
        let mut sum = 0.0;
        for placed in &self.placed {
            sum += placed.height;
            self.prefix.push(sum);
        }
    }

    /// Moves the blocks through the edits of one change, in the order they were applied.
    ///
    /// A block under a line before an edit stays. A block under a line after it moves by the
    /// lines the edit added or removed. A block under a line the edit reached into goes to the
    /// line the edit's new text ends on, which is where the end of its line went. Answers whether
    /// any block moved.
    pub fn follow(
        &mut self,
        edits: &[tree_sitter::InputEdit],
        scale: f32,
        line_count: usize,
    ) -> bool {
        if self.layers.is_empty() {
            return false;
        }
        let mut moved = false;
        for edit in edits {
            let start = edit.start_position.row;
            let old_end = edit.old_end_position.row;
            let new_end = edit.new_end_position.row;
            for block in self
                .layers
                .iter_mut()
                .flat_map(|layer| layer.blocks.iter_mut())
            {
                let line = block.after_line;
                let next = if line < start {
                    line
                } else if line >= old_end {
                    (line + new_end).saturating_sub(old_end)
                } else {
                    new_end
                };
                if next != line {
                    block.after_line = next;
                    moved = true;
                }
            }
        }
        if moved || self.line_count != line_count {
            self.rebuild(scale, line_count);
        }
        moved
    }

    /// The height of every block, in device pixels.
    pub fn total(&self) -> f64 {
        self.prefix.last().copied().unwrap_or(0.0)
    }

    /// How many blocks sit under lines before `line`.
    fn count_before(&self, line: usize) -> usize {
        self.placed
            .partition_point(|placed| placed.after_line < line)
    }

    /// The height of the blocks above `line`, in device pixels.
    pub fn above(&self, line: usize) -> f64 {
        self.prefix[self.count_before(line)]
    }

    /// The height of the blocks under the lines of `lines`, in device pixels.
    pub fn within(&self, lines: Range<usize>) -> f64 {
        self.above(lines.end) - self.above(lines.start)
    }

    /// The indices of the blocks under the lines of `lines`.
    pub fn indices_within(&self, lines: Range<usize>) -> Range<usize> {
        self.count_before(lines.start)
            ..self
                .count_before(lines.end)
                .max(self.count_before(lines.start))
    }

    /// Where the block at `index` starts, in device pixels from the top of the content, with
    /// lines `line_height` tall.
    pub fn top_of(&self, index: usize, line_height: f64) -> f64 {
        let placed = &self.placed[index];
        (placed.after_line as f64 + 1.0) * line_height + self.prefix[index]
    }

    /// Where `y` device pixels down from the top of the content falls, with lines
    /// `line_height` tall. A negative `y` falls on the first line.
    pub fn locate(&self, y: f64, line_height: f64) -> Spot {
        let y = y.max(0.0);
        let line_height = line_height.max(1.0);
        // Every block that starts at or above `y`. Their starts never go down the list, because
        // the lines are sorted and the running height only grows.
        let started = self.started_at_or_above(y, line_height);
        if started > 0 {
            let index = started - 1;
            if y < self.top_of(index, line_height) + self.placed[index].height {
                return Spot::Block(index);
            }
        }
        // Every block that started has also ended, so all of their room is above `y`.
        let line = ((y - self.prefix[started]) / line_height).floor();
        Spot::Line(line.max(0.0) as usize)
    }

    /// How many blocks start at or above `y`.
    fn started_at_or_above(&self, y: f64, line_height: f64) -> usize {
        let mut low = 0;
        let mut high = self.placed.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self.top_of(middle, line_height) <= y {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    /// The line `y` device pixels down falls on, or the line above the block it falls in.
    pub fn line_at(&self, y: f64, line_height: f64) -> usize {
        match self.locate(y, line_height) {
            Spot::Line(line) => line,
            Spot::Block(index) => self.placed[index].after_line,
        }
    }
}

/// A height in CSS pixels as whole device pixels, never less than none.
fn device_height(css: f32, scale: f32) -> f64 {
    if !css.is_finite() {
        return 0.0;
    }
    f64::from((css.max(0.0) * scale).round())
}

#[cfg(test)]
mod tests {
    use super::{Block, BlockId, BlockMap, Spot};

    /// Lines twenty device pixels tall.
    const LINE: f64 = 20.0;

    /// A map over a text of `lines` lines, at scale one.
    fn map(lines: usize, blocks: &[(u64, usize, f32)]) -> BlockMap {
        let mut map = BlockMap::default();
        map.set(
            "test",
            blocks
                .iter()
                .map(|&(id, line, height)| Block::new(BlockId(id), line, height))
                .collect(),
            1.0,
            lines,
        );
        map
    }

    #[test]
    fn an_empty_map_puts_every_line_at_its_own_height() {
        let map = BlockMap::default();
        assert_eq!(map.above(500), 0.0);
        assert_eq!(map.total(), 0.0);
        assert_eq!(map.locate(0.0, LINE), Spot::Line(0));
        assert_eq!(map.locate(19.9, LINE), Spot::Line(0));
        assert_eq!(map.locate(20.0, LINE), Spot::Line(1));
        assert_eq!(map.locate(-40.0, LINE), Spot::Line(0));
    }

    #[test]
    fn a_block_pushes_the_lines_below_it_down() {
        let map = map(10, &[(1, 2, 50.0)]);
        assert_eq!(map.above(2), 0.0, "the block sits under its line");
        assert_eq!(map.above(3), 50.0);
        assert_eq!(map.above(9), 50.0);
        assert_eq!(map.total(), 50.0);
        assert_eq!(map.top_of(0, LINE), 60.0, "under line two");
    }

    #[test]
    fn a_position_falls_on_a_line_or_in_a_block() {
        let map = map(10, &[(1, 2, 50.0), (2, 5, 10.0)]);
        // Lines 0..3 take 0..60, the first block 60..110, line 3 110..130.
        assert_eq!(map.locate(59.0, LINE), Spot::Line(2));
        assert_eq!(map.locate(60.0, LINE), Spot::Block(0));
        assert_eq!(map.locate(109.0, LINE), Spot::Block(0));
        assert_eq!(map.locate(110.0, LINE), Spot::Line(3));
        // Lines 3..6 take 110..170, the second block 170..180, line 6 from 180.
        assert_eq!(map.locate(169.0, LINE), Spot::Line(5));
        assert_eq!(map.locate(175.0, LINE), Spot::Block(1));
        assert_eq!(map.locate(180.0, LINE), Spot::Line(6));
        assert_eq!(map.line_at(75.0, LINE), 2, "a block counts as its line");
    }

    #[test]
    fn locating_is_the_inverse_of_the_line_tops() {
        let map = map(
            200,
            &[
                (1, 0, 13.0),
                (2, 0, 7.0),
                (3, 40, 100.0),
                (4, 41, 1.0),
                (5, 199, 30.0),
            ],
        );
        for line in 0..200 {
            let top = line as f64 * LINE + map.above(line);
            assert_eq!(map.locate(top, LINE), Spot::Line(line), "the top of {line}");
            assert_eq!(
                map.locate(top + LINE - 0.5, LINE),
                Spot::Line(line),
                "the foot of {line}"
            );
        }
        for index in 0..map.placed().len() {
            let top = map.top_of(index, LINE);
            assert_eq!(map.locate(top, LINE), Spot::Block(index));
            assert_eq!(
                map.locate(top + map.placed()[index].height - 0.5, LINE),
                Spot::Block(index)
            );
        }
    }

    #[test]
    fn blocks_under_one_line_stack_in_the_order_given() {
        let map = map(5, &[(1, 1, 10.0), (2, 1, 30.0)]);
        assert_eq!(map.placed()[0].id, BlockId(1));
        assert_eq!(map.top_of(0, LINE), 40.0);
        assert_eq!(map.top_of(1, LINE), 50.0);
        assert_eq!(map.locate(55.0, LINE), Spot::Block(1));
        assert_eq!(map.above(2), 40.0);
    }

    #[test]
    fn a_zero_height_block_takes_no_room_and_no_positions() {
        let map = map(5, &[(1, 1, 0.0)]);
        assert_eq!(map.above(2), 0.0);
        assert_eq!(map.locate(40.0, LINE), Spot::Line(2));
        assert_eq!(map.indices_within(0..5), 0..1, "it is still on screen");
    }

    #[test]
    fn heights_round_to_whole_device_pixels_at_the_scale() {
        let mut map = map(5, &[(1, 1, 10.3)]);
        assert_eq!(map.total(), 10.0);
        map.ensure(1.5, 5);
        assert_eq!(
            map.total(),
            15.0,
            "10.3 at one and a half is 15.45, drawn as 15"
        );
        let negative = super::BlockMap::default();
        assert_eq!(negative.total(), 0.0);
        let map = self::map(5, &[(1, 1, -4.0), (2, 1, f32::NAN)]);
        assert_eq!(map.total(), 0.0);
    }

    #[test]
    fn a_block_past_the_end_sits_under_the_last_line() {
        let mut map = map(3, &[(1, 10, 20.0)]);
        assert_eq!(map.placed()[0].after_line, 2);
        assert_eq!(map.above(3), 20.0);
        map.ensure(1.0, 20);
        assert_eq!(
            map.placed()[0].after_line,
            10,
            "the text grew back under it"
        );
    }

    #[test]
    fn layers_replace_only_themselves() {
        let mut map = map(10, &[(1, 1, 10.0)]);
        assert!(map.set("other", vec![Block::new(BlockId(2), 3, 5.0)], 1.0, 10));
        assert_eq!(map.total(), 15.0);
        assert!(!map.set("other", vec![Block::new(BlockId(2), 3, 5.0)], 1.0, 10));
        assert!(map.clear("test", 1.0, 10));
        assert_eq!(map.total(), 5.0);
        assert!(!map.clear("test", 1.0, 10));
        assert!(!map.set("never", Vec::new(), 1.0, 10));
    }

    #[test]
    fn a_range_of_lines_has_the_room_of_its_blocks() {
        let map = map(10, &[(1, 1, 10.0), (2, 4, 20.0), (3, 8, 40.0)]);
        assert_eq!(map.within(0..2), 10.0);
        assert_eq!(
            map.within(2..5),
            20.0,
            "the block under the window's last line is in it"
        );
        assert_eq!(map.within(5..8), 0.0);
        assert_eq!(map.indices_within(2..9), 1..3);
        assert_eq!(map.indices_within(5..8), 2..2);
    }

    /// One edit from `start` to `old_end` rows, ending at `new_end` rows afterwards.
    fn edit(start: usize, old_end: usize, new_end: usize) -> tree_sitter::InputEdit {
        let point = |row| tree_sitter::Point { row, column: 0 };
        tree_sitter::InputEdit {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 0,
            start_position: point(start),
            old_end_position: point(old_end),
            new_end_position: point(new_end),
        }
    }

    #[test]
    fn blocks_follow_their_lines_through_edits() {
        let mut map = map(20, &[(1, 2, 10.0), (2, 5, 10.0), (3, 9, 10.0)]);
        // Two lines added on line four.
        assert!(map.follow(&[edit(4, 4, 6)], 1.0, 22));
        let lines: Vec<usize> = map
            .placed()
            .iter()
            .map(|placed| placed.after_line)
            .collect();
        assert_eq!(lines, [2, 7, 11]);

        // Lines six to nine taken out: the block under line seven goes where the edit ended.
        assert!(map.follow(&[edit(6, 9, 6)], 1.0, 19));
        let lines: Vec<usize> = map
            .placed()
            .iter()
            .map(|placed| placed.after_line)
            .collect();
        assert_eq!(lines, [2, 6, 8]);

        // Typing inside a line moves nothing.
        assert!(!map.follow(&[edit(8, 8, 8)], 1.0, 19));
    }
}
