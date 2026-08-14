//! What has been typed of a command that is not finished yet.

/// An operator waiting for its motion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    /// `d` — delete what the motion spans.
    Delete,
    /// `c` — delete it and insert.
    Change,
    /// `y` — copy it.
    Yank,
}

/// The prefix collected so far: `2d` holds a count and an operator, waiting for a motion.
#[derive(Clone, Copy, Default)]
pub struct Pending {
    /// The count typed before the command, when one was.
    pub count: Option<u32>,
    /// The operator typed, when one was.
    pub operator: Option<Op>,
    /// Whether a `g` has been typed, waiting for another.
    pub awaiting_g: bool,
}

impl Pending {
    /// The count in force, which is one when none was typed.
    pub fn count(&self) -> u32 {
        self.count.unwrap_or(1).max(1)
    }

    /// Adds one digit to the count.
    pub fn push_digit(&mut self, digit: u32) {
        self.count = Some(
            self.count
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit),
        );
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// What the status line echoes: `2d`, `13`, `g`.
    pub fn echo(&self) -> String {
        let mut echo = String::new();
        if let Some(count) = self.count {
            echo.push_str(&count.to_string());
        }
        match self.operator {
            Some(Op::Delete) => echo.push('d'),
            Some(Op::Change) => echo.push('c'),
            Some(Op::Yank) => echo.push('y'),
            None => {}
        }
        if self.awaiting_g {
            echo.push('g');
        }
        echo
    }
}
