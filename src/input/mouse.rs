//! Counting clicks and turning pointer places into buffer places.
//!
//! The framework reports presses; it does not count them. The counter here is the terminal's:
//! clicks close together in time and place count up through three and wrap, and the count is
//! what decides whether a press selects a point, a word, or a line.

use std::time::{Duration, Instant};

/// How long after a click the next one still counts up.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// How far apart two clicks may land and still count together, in device pixels.
const MULTI_CLICK_SLOP: f64 = 8.0;

/// What a press means by its count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickKind {
    /// Place the caret.
    Single,
    /// Select the word.
    Double,
    /// Select the line.
    Triple,
}

/// Counts presses into single, double and triple clicks.
#[derive(Debug)]
pub struct ClickCounter {
    last_at: Option<Instant>,
    last_pos: (f64, f64),
    count: u8,
}

impl ClickCounter {
    /// A counter that has seen nothing.
    pub fn new() -> Self {
        Self {
            last_at: None,
            last_pos: (0.0, 0.0),
            count: 0,
        }
    }

    /// Counts a press at `pos` device pixels, at `now`.
    pub fn click(&mut self, pos: (f64, f64), now: Instant) -> ClickKind {
        let near = (pos.0 - self.last_pos.0).abs() <= MULTI_CLICK_SLOP
            && (pos.1 - self.last_pos.1).abs() <= MULTI_CLICK_SLOP;
        let soon = self
            .last_at
            .is_some_and(|last| now.duration_since(last) <= MULTI_CLICK_WINDOW);
        self.count = if near && soon {
            (self.count % 3) + 1
        } else {
            1
        };
        self.last_at = Some(now);
        self.last_pos = pos;
        match self.count {
            2 => ClickKind::Double,
            3 => ClickKind::Triple,
            _ => ClickKind::Single,
        }
    }
}

impl Default for ClickCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_count_up_and_wrap() {
        let mut counter = ClickCounter::new();
        let start = Instant::now();
        let at = (10.0, 10.0);
        assert_eq!(counter.click(at, start), ClickKind::Single);
        assert_eq!(
            counter.click(at, start + Duration::from_millis(100)),
            ClickKind::Double
        );
        assert_eq!(
            counter.click(at, start + Duration::from_millis(200)),
            ClickKind::Triple
        );
        assert_eq!(
            counter.click(at, start + Duration::from_millis(300)),
            ClickKind::Single,
            "a fourth quick click starts over"
        );
    }

    #[test]
    fn a_slow_second_click_is_a_first_click() {
        let mut counter = ClickCounter::new();
        let start = Instant::now();
        counter.click((10.0, 10.0), start);
        assert_eq!(
            counter.click((10.0, 10.0), start + Duration::from_secs(1)),
            ClickKind::Single
        );
    }

    #[test]
    fn a_click_far_away_is_a_first_click() {
        let mut counter = ClickCounter::new();
        let start = Instant::now();
        counter.click((10.0, 10.0), start);
        assert_eq!(
            counter.click((100.0, 10.0), start + Duration::from_millis(100)),
            ClickKind::Single
        );
    }
}
