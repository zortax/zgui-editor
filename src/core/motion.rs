//! Where a motion lands, and what a motion spans.
//!
//! Everything here is a pure function of the rope and a selection, which is what makes a vim
//! layer possible outside the component: an operator asks [`motion_range`] what `d2w` would
//! take, and gets the same answer the motion itself would move by.

use ropey::Rope;

use crate::command::Motion;
use crate::core::position::{self, Affinity};
use crate::core::selection::Selection;
use crate::core::words;

/// What a motion needs to know about the view.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionContext {
    /// How many whole lines the viewport shows, for the page motions.
    pub viewport_lines: usize,
}

/// Where `motion`, applied `count` times, takes the head of `selection`.
///
/// The returned selection is collapsed to the destination when `extend` is false, and keeps its
/// anchor when `extend` is true. The goal column survives vertical movement and resets on
/// everything else.
pub fn apply(
    rope: &Rope,
    selection: Selection,
    motion: Motion,
    count: u32,
    extend: bool,
    context: MotionContext,
) -> Selection {
    let count = count.max(1);
    let mut head = selection.head;
    let mut goal = selection.goal_col;

    match motion {
        Motion::Up
        | Motion::Down
        | Motion::PageUp
        | Motion::PageDown
        | Motion::HalfPageUp
        | Motion::HalfPageDown => {
            let lines_per = match motion {
                Motion::Up | Motion::Down => 1,
                Motion::PageUp | Motion::PageDown => context.viewport_lines.max(1),
                _ => (context.viewport_lines / 2).max(1),
            };
            let down = matches!(
                motion,
                Motion::Down | Motion::PageDown | Motion::HalfPageDown
            );
            let by = lines_per * count as usize;
            let line = position::line_of(rope, head);
            let column = goal
                .map(|goal| goal as usize)
                .unwrap_or_else(|| position::grapheme_col(rope, head));
            goal = Some(column as u32);
            let target = if down {
                (line + by).min(position::line_count(rope).saturating_sub(1))
            } else {
                line.saturating_sub(by)
            };
            head = position::byte_at_col(rope, target, column);
        }
        Motion::Left => {
            for _ in 0..count {
                let line_start = position::line_start(rope, position::line_of(rope, head));
                if head == line_start {
                    break;
                }
                head = position::prev_grapheme(rope, head);
            }
            goal = None;
        }
        Motion::Right => {
            for _ in 0..count {
                let line_end = position::line_end(rope, position::line_of(rope, head));
                if head >= line_end {
                    break;
                }
                head = position::next_grapheme(rope, head);
            }
            goal = None;
        }
        Motion::WordForward { big } => {
            for _ in 0..count {
                head = words::next_word_start(rope, head, big);
            }
            goal = None;
        }
        Motion::WordBackward { big } => {
            for _ in 0..count {
                head = words::prev_word_start(rope, head, big);
            }
            goal = None;
        }
        Motion::WordEnd { big } => {
            for _ in 0..count {
                head = words::word_end(rope, head, big);
            }
            goal = None;
        }
        Motion::LineStart => {
            head = position::line_start(rope, position::line_of(rope, head));
            goal = None;
        }
        Motion::LineFirstNonBlank => {
            let line = position::line_of(rope, head);
            head = first_non_blank(rope, line);
            goal = None;
        }
        Motion::LineEnd => {
            let line = position::line_of(rope, head);
            let target = line + count as usize - 1;
            head = position::line_end(
                rope,
                target.min(position::line_count(rope).saturating_sub(1)),
            );
            goal = None;
        }
        Motion::DocumentStart => {
            head = 0;
            goal = None;
        }
        Motion::DocumentEnd => {
            head = rope.len_bytes();
            goal = None;
        }
        Motion::GotoLine(line) => {
            let line = line.min(position::line_count(rope).saturating_sub(1));
            head = first_non_blank(rope, line);
            goal = None;
        }
        Motion::To(byte) => {
            head = position::snap(rope, byte);
            goal = None;
        }
    }

    Selection {
        anchor: if extend { selection.anchor } else { head },
        head,
        affinity: Affinity::default(),
        goal_col: goal,
    }
}

/// The first non-whitespace byte of `line`, or its end when it is all whitespace.
pub fn first_non_blank(rope: &Rope, line: usize) -> usize {
    let start = position::line_start(rope, line);
    let text = position::line_text(rope, line);
    match text.find(|character: char| !character.is_whitespace()) {
        Some(offset) => start + offset,
        None => position::line_end(rope, line),
    }
}

/// The byte range `motion` spans from `selection`, as an operator takes it.
///
/// Charwise operators span from the head to where the motion lands; an inclusive motion (`e`)
/// takes its landing grapheme too. `linewise` widens to whole lines including their breaks,
/// which is what `dd` and `d j` take.
pub fn motion_range(
    rope: &Rope,
    selection: Selection,
    motion: Motion,
    count: u32,
    linewise: bool,
    context: MotionContext,
) -> std::ops::Range<usize> {
    if linewise {
        // The lines from the head's line through the motion's landing line. `dd` is `d` with a
        // vertical motion of zero lines, so an adjusted count of zero applies no motion at all.
        let adjusted = count.max(1).saturating_sub(linewise_count_adjust(motion));
        let landed = if adjusted == 0 {
            selection
        } else {
            apply(rope, selection, motion, adjusted, false, context)
        };
        let from_line =
            position::line_of(rope, selection.head).min(position::line_of(rope, landed.head));
        let to_line =
            position::line_of(rope, selection.head).max(position::line_of(rope, landed.head));
        let start = position::line_start(rope, from_line);
        let end = if to_line + 1 >= position::line_count(rope) {
            rope.len_bytes()
        } else {
            position::line_start(rope, to_line + 1)
        };
        return start..end;
    }
    let landed = apply(rope, selection, motion, count, false, context);
    let inclusive = matches!(motion, Motion::WordEnd { .. });
    let (from, to) = if landed.head >= selection.head {
        (
            selection.head,
            if inclusive {
                position::next_grapheme(rope, landed.head)
            } else {
                landed.head
            },
        )
    } else {
        (landed.head, selection.head)
    };
    from..to
}

/// How a count aims a linewise motion.
///
/// `2dd` deletes this line and the next: the motion inside it is one `Down`, not two. Vertical
/// motions inside a linewise operator span `count` lines, so one application is subtracted;
/// jumps are used as they are.
fn linewise_count_adjust(motion: Motion) -> u32 {
    match motion {
        Motion::Down | Motion::Up => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    fn caret(at: usize) -> Selection {
        Selection::caret(at)
    }

    const NO_VIEW: MotionContext = MotionContext { viewport_lines: 0 };

    #[test]
    fn left_and_right_stop_at_line_edges() {
        let rope = rope("ab\ncd");
        let sel = apply(&rope, caret(1), Motion::Right, 5, false, NO_VIEW);
        assert_eq!(sel.head, 2, "right stops before the break");
        let sel = apply(&rope, caret(4), Motion::Left, 5, false, NO_VIEW);
        assert_eq!(sel.head, 3, "left stops at the line start");
    }

    #[test]
    fn vertical_movement_keeps_the_goal_column() {
        let rope = rope("abcdef\nab\nabcdef");
        let down = apply(&rope, caret(4), Motion::Down, 1, false, NO_VIEW);
        assert_eq!(down.head, 9, "clamped to the short line's end");
        assert_eq!(down.goal_col, Some(4));
        let down_again = apply(&rope, down, Motion::Down, 1, false, NO_VIEW);
        assert_eq!(down_again.head, 14, "the goal column comes back");
    }

    #[test]
    fn counts_multiply_motions() {
        let rope = rope("one two three four");
        let sel = apply(
            &rope,
            caret(0),
            Motion::WordForward { big: false },
            2,
            false,
            NO_VIEW,
        );
        assert_eq!(sel.head, 8);
    }

    #[test]
    fn goto_line_lands_on_the_first_non_blank() {
        let rope = rope("one\n   two\nthree");
        let sel = apply(&rope, caret(0), Motion::GotoLine(1), 1, false, NO_VIEW);
        assert_eq!(sel.head, 7);
    }

    #[test]
    fn extending_keeps_the_anchor() {
        let rope = rope("one two");
        let sel = apply(
            &rope,
            caret(0),
            Motion::WordForward { big: false },
            1,
            true,
            NO_VIEW,
        );
        assert_eq!(sel.anchor, 0);
        assert_eq!(sel.head, 4);
    }

    #[test]
    fn a_charwise_range_spans_head_to_landing() {
        let rope = rope("one two three");
        let range = motion_range(
            &rope,
            caret(0),
            Motion::WordForward { big: false },
            1,
            false,
            NO_VIEW,
        );
        assert_eq!(range, 0..4, "dw takes the trailing space");
    }

    #[test]
    fn word_end_is_inclusive_for_operators() {
        let rope = rope("one two");
        let range = motion_range(
            &rope,
            caret(0),
            Motion::WordEnd { big: false },
            1,
            false,
            NO_VIEW,
        );
        assert_eq!(range, 0..3, "de takes the whole word");
    }

    #[test]
    fn a_linewise_range_takes_whole_lines() {
        let rope = rope("one\ntwo\nthree");
        let range = motion_range(&rope, caret(5), Motion::Down, 1, true, NO_VIEW);
        assert_eq!(range, 4..8, "dd with count 1 takes the current line");
        let range = motion_range(&rope, caret(5), Motion::Down, 2, true, NO_VIEW);
        assert_eq!(range, 4..13, "2dd takes this line and the next");
    }

    #[test]
    fn the_last_line_deletes_without_a_break_after_it() {
        let rope = rope("one\ntwo");
        let range = motion_range(&rope, caret(5), Motion::Down, 1, true, NO_VIEW);
        assert_eq!(range, 4..7);
    }
}
