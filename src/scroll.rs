//! Where the view sits over the document, and how it gets somewhere else.
//!
//! The scroll position is the editor's own, held as a fractional line in an `f64` — a
//! ten-million-line file scrolls to pixel precision, which is the reason the framework's own
//! scroll container (whose offsets are `f32` device pixels) is not used. Everything here is
//! arithmetic: the animation timer and the repaint belong to the view.

/// Where the view sits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPos {
    /// The line at the top of the view, fractionally: 2.5 is half-way down line two.
    pub line: f64,
    /// How far the text is scrolled left, in device pixels.
    pub x_px: f64,
}

/// The scroll state the view drives: where it is, and where it is going.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollState {
    /// Where the view is.
    pub pos: ScrollPos,
    /// The line the glide is heading for; equal to `pos.line` at rest.
    pub target_line: f64,
}

/// How quickly the glide closes on its target: the time constant, in seconds.
///
/// After one constant about two thirds of the distance is gone; the glide reads as done in
/// roughly three. Small enough to feel immediate, large enough to read as motion.
const GLIDE_TAU: f64 = 0.075;

/// How close to the target counts as arrived, in lines.
const GLIDE_DONE: f64 = 0.002;

impl ScrollState {
    /// The greatest top line a document of `total` lines allows a view of `viewport` lines.
    pub fn max_top(total: usize, viewport: f64) -> f64 {
        (total as f64 - viewport).max(0.0)
    }

    /// Aims the glide `lines` further, clamped to the document.
    pub fn scroll_by(&mut self, lines: f64, total: usize, viewport: f64) {
        self.target_line = (self.target_line + lines).clamp(0.0, Self::max_top(total, viewport));
    }

    /// Moves the view immediately, target and all, as a trackpad's pixel deltas want.
    pub fn scroll_to(&mut self, line: f64, total: usize, viewport: f64) {
        let line = line.clamp(0.0, Self::max_top(total, viewport));
        self.pos.line = line;
        self.target_line = line;
    }

    /// One step of the glide after `dt` seconds. Answers whether the glide still runs.
    pub fn step(&mut self, dt: f64) -> bool {
        let distance = self.target_line - self.pos.line;
        if distance.abs() < GLIDE_DONE {
            self.pos.line = self.target_line;
            return false;
        }
        let closed = 1.0 - (-dt / GLIDE_TAU).exp();
        self.pos.line += distance * closed;
        true
    }

    /// Whether the glide has somewhere left to go.
    pub fn gliding(&self) -> bool {
        (self.target_line - self.pos.line).abs() >= GLIDE_DONE
    }

    /// Moves the view so line `line` is visible with `margin` lines around it, immediately.
    ///
    /// Immediately, because this follows the caret, and a caret that outruns its view while
    /// typing is worse than a jump. Answers whether anything moved.
    pub fn ensure_visible(
        &mut self,
        line: usize,
        margin: usize,
        total: usize,
        viewport: f64,
    ) -> bool {
        let line = line as f64;
        let margin = (margin as f64).min(((viewport - 1.0) / 2.0).max(0.0));
        let top = self.pos.line;
        let bottom = top + viewport - 1.0;
        let new_top = if line - margin < top {
            line - margin
        } else if line + margin > bottom {
            line + margin - viewport + 1.0
        } else {
            return false;
        };
        self.scroll_to(new_top, total, viewport);
        true
    }

    /// Scrolls horizontally so a caret at `x` device pixels is visible in a text area
    /// `width` wide, with `slack` pixels of lead. Answers whether anything moved.
    pub fn ensure_visible_x(&mut self, x: f64, width: f64, slack: f64) -> bool {
        let left = self.pos.x_px;
        let right = left + width;
        if x < left + slack {
            self.pos.x_px = (x - slack).max(0.0);
            true
        } else if x > right - slack {
            self.pos.x_px = x + slack - width;
            true
        } else {
            false
        }
    }
}

/// The vertical scrollbar's geometry, as pure arithmetic over the document and the view.
#[derive(Clone, Copy, Debug)]
pub struct Scrollbar {
    /// How tall the track is, in device pixels.
    pub track: f64,
    /// How many lines the document has.
    pub total: usize,
    /// How many lines the view shows.
    pub viewport: f64,
}

/// The least a thumb is drawn as, so a long document still leaves something to grab.
pub const MIN_THUMB: f64 = 30.0;

impl Scrollbar {
    /// Where the thumb sits for `top`, as a top offset and a height, when one is worth drawing.
    pub fn thumb(&self, top: f64) -> Option<(f64, f64)> {
        let max_top = ScrollState::max_top(self.total, self.viewport);
        if max_top <= 0.0 || self.track <= 0.0 {
            return None;
        }
        let span = (self.viewport / self.total as f64).clamp(0.0, 1.0);
        let height = (self.track * span).max(MIN_THUMB).min(self.track);
        let position = (top / max_top).clamp(0.0, 1.0);
        Some(((self.track - height) * position, height))
    }

    /// The top line a thumb dragged so its own top sits at `y` asks for.
    pub fn line_at(&self, y: f64) -> f64 {
        let Some((_, height)) = self.thumb(0.0) else {
            return 0.0;
        };
        let travel = (self.track - height).max(1.0);
        let position = (y / travel).clamp(0.0, 1.0);
        position * ScrollState::max_top(self.total, self.viewport)
    }
}

/// Lines per second an out-of-view drag scrolls, growing with how far outside the pointer is.
pub fn autoscroll_rate(overshoot_px: f64, line_height: f64) -> f64 {
    let lines_out = overshoot_px / line_height.max(1.0);
    (lines_out * 10.0).clamp(6.0, 150.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glide_converges_and_stops() {
        let mut scroll = ScrollState::default();
        scroll.scroll_by(100.0, 1000, 40.0);
        let mut steps = 0;
        while scroll.step(0.008) {
            steps += 1;
            assert!(steps < 1000, "the glide must settle");
        }
        assert!((scroll.pos.line - 100.0).abs() < 0.01);
        assert!(!scroll.gliding());
    }

    #[test]
    fn scrolling_clamps_to_the_document() {
        let mut scroll = ScrollState::default();
        scroll.scroll_by(-10.0, 100, 40.0);
        assert_eq!(scroll.target_line, 0.0);
        scroll.scroll_by(1000.0, 100, 40.0);
        assert_eq!(scroll.target_line, 60.0);
    }

    #[test]
    fn a_short_document_does_not_scroll() {
        let mut scroll = ScrollState::default();
        scroll.scroll_by(10.0, 5, 40.0);
        assert_eq!(scroll.target_line, 0.0);
    }

    #[test]
    fn ensure_visible_moves_only_when_needed() {
        let mut scroll = ScrollState::default();
        assert!(!scroll.ensure_visible(10, 0, 1000, 40.0), "already visible");
        assert!(scroll.ensure_visible(50, 0, 1000, 40.0));
        assert_eq!(scroll.pos.line, 11.0, "the line sits at the bottom edge");
        assert!(scroll.ensure_visible(5, 0, 1000, 40.0));
        assert_eq!(scroll.pos.line, 5.0, "the line sits at the top edge");
    }

    #[test]
    fn ensure_visible_honours_the_margin() {
        let mut scroll = ScrollState::default();
        assert!(scroll.ensure_visible(39, 3, 1000, 40.0));
        assert_eq!(scroll.pos.line, 3.0);
    }

    #[test]
    fn the_thumb_travels_the_whole_track() {
        let bar = Scrollbar {
            track: 800.0,
            total: 10_000,
            viewport: 40.0,
        };
        let (top, height) = bar.thumb(0.0).expect("a thumb");
        assert_eq!(top, 0.0);
        assert!(height >= MIN_THUMB);
        let max = ScrollState::max_top(10_000, 40.0);
        let (top, height) = bar.thumb(max).expect("a thumb");
        assert!((top + height - 800.0).abs() < 0.5);
    }

    #[test]
    fn dragging_maps_back_to_lines() {
        let bar = Scrollbar {
            track: 800.0,
            total: 10_000,
            viewport: 40.0,
        };
        assert_eq!(bar.line_at(0.0), 0.0);
        let max = ScrollState::max_top(10_000, 40.0);
        assert!((bar.line_at(800.0) - max).abs() < 0.01);
    }

    #[test]
    fn a_full_view_shows_no_thumb() {
        let bar = Scrollbar {
            track: 800.0,
            total: 10,
            viewport: 40.0,
        };
        assert!(bar.thumb(0.0).is_none());
    }

    #[test]
    fn horizontal_following_scrolls_both_ways() {
        let mut scroll = ScrollState::default();
        assert!(scroll.ensure_visible_x(500.0, 400.0, 16.0));
        assert!(scroll.pos.x_px > 0.0);
        assert!(scroll.ensure_visible_x(0.0, 400.0, 16.0));
        assert_eq!(scroll.pos.x_px, 0.0);
    }
}
