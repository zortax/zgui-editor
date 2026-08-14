//! One key in, one step out: the whole grammar, as a pure function.
//!
//! Purity is what makes the layer testable without a window — the tests at the bottom drive
//! the grammar with fabricated key events and read the actions straight back out.

use zgui::vocab::{Key, KeyEvent, Modifiers, NamedKey};
use zgui_editor::Motion;

use crate::actions::{InsertEntry, VimAction};
use crate::mode::Mode;
use crate::pending::{Op, Pending};

/// What one key amounted to.
#[derive(Debug)]
pub enum Step {
    /// A finished command (possibly several actions).
    Consume(Vec<VimAction>),
    /// Not ours; the editor's default keymap may have it.
    PassThrough,
    /// Part of a longer command; nothing happens yet.
    Pending,
}

/// Reads one key in `mode`, updating `pending` as the grammar collects a prefix.
pub fn translate(
    mode: Mode,
    pending: &mut Pending,
    event: &KeyEvent,
    modifiers: Modifiers,
) -> Step {
    match mode {
        Mode::Insert => translate_insert(event, modifiers),
        Mode::Normal | Mode::Visual | Mode::VisualLine => {
            translate_modal(mode, pending, event, modifiers)
        }
    }
}

/// Insert mode: almost everything is text, which the editor itself handles.
fn translate_insert(event: &KeyEvent, modifiers: Modifiers) -> Step {
    if event.key == Key::Named(NamedKey::Escape) {
        return Step::Consume(vec![
            VimAction::EnterNormal,
            VimAction::Motion {
                motion: Motion::Left,
                count: 1,
            },
        ]);
    }
    // Ctrl-[ is Escape to vim.
    if modifiers.control() && character_of(&event.key_without_modifiers) == Some("[") {
        return Step::Consume(vec![VimAction::EnterNormal]);
    }
    Step::PassThrough
}

/// Normal and the two visual modes share the grammar; what differs is what a motion does with
/// the selection, and that is the performer's business.
fn translate_modal(
    mode: Mode,
    pending: &mut Pending,
    event: &KeyEvent,
    modifiers: Modifiers,
) -> Step {
    let visual = matches!(mode, Mode::Visual | Mode::VisualLine);

    // Control chords first, matched on the unmodified key.
    if modifiers.control() {
        return match character_of(&event.key_without_modifiers) {
            Some("r") => finish(pending, vec![VimAction::Redo]),
            Some("d") => finish(pending, vec![VimAction::HalfPage { down: true }]),
            Some("u") => finish(pending, vec![VimAction::HalfPage { down: false }]),
            _ => Step::PassThrough,
        };
    }

    if event.key == Key::Named(NamedKey::Escape) {
        let had_prefix =
            pending.count.is_some() || pending.operator.is_some() || pending.awaiting_g;
        pending.clear();
        if visual {
            return Step::Consume(vec![VimAction::EnterNormal]);
        }
        return if had_prefix {
            Step::Pending
        } else {
            Step::Consume(vec![])
        };
    }

    // Arrows work in every mode.
    if let Key::Named(named) = &event.key {
        let motion = match named {
            NamedKey::ArrowLeft => Some(Motion::Left),
            NamedKey::ArrowRight => Some(Motion::Right),
            NamedKey::ArrowUp => Some(Motion::Up),
            NamedKey::ArrowDown => Some(Motion::Down),
            _ => None,
        };
        if let Some(motion) = motion {
            return motion_step(pending, motion);
        }
    }

    let Some(text) = character_of(&event.key) else {
        return Step::Consume(vec![]);
    };

    // A count digit; `0` with no count already begun is the line-start motion.
    if let Some(digit) = text.chars().next().and_then(|c| c.to_digit(10))
        && (digit != 0 || pending.count.is_some())
    {
        pending.push_digit(digit);
        return Step::Pending;
    }

    // The `g` prefix.
    if pending.awaiting_g {
        pending.awaiting_g = false;
        return match text {
            "g" => {
                let line = pending.count.map(|count| count.saturating_sub(1) as usize);
                motion_step(pending, Motion::GotoLine(line.unwrap_or(0)))
            }
            _ => {
                pending.clear();
                Step::Consume(vec![])
            }
        };
    }
    if text == "g" {
        pending.awaiting_g = true;
        return Step::Pending;
    }

    // Motions.
    let motion = match text {
        "h" => Some(Motion::Left),
        "l" => Some(Motion::Right),
        "j" => Some(Motion::Down),
        "k" => Some(Motion::Up),
        "w" => Some(Motion::WordForward { big: false }),
        "W" => Some(Motion::WordForward { big: true }),
        "b" => Some(Motion::WordBackward { big: false }),
        "B" => Some(Motion::WordBackward { big: true }),
        "e" => Some(Motion::WordEnd { big: false }),
        "E" => Some(Motion::WordEnd { big: true }),
        "0" => Some(Motion::LineStart),
        "^" => Some(Motion::LineFirstNonBlank),
        "$" => Some(Motion::LineEnd),
        "G" => Some(match pending.count {
            Some(count) => Motion::GotoLine(count.saturating_sub(1) as usize),
            None => Motion::DocumentEnd,
        }),
        _ => None,
    };
    if let Some(motion) = motion {
        return motion_step(pending, motion);
    }

    // Operators. Doubling one (`dd`) takes the line.
    let operator = match text {
        "d" => Some(Op::Delete),
        "c" => Some(Op::Change),
        "y" => Some(Op::Yank),
        _ => None,
    };
    if let Some(op) = operator {
        if visual {
            return finish(pending, vec![VimAction::OperatorOnSelection { op }]);
        }
        match pending.operator {
            Some(held) if held == op => {
                let count = pending.count();
                return finish(
                    pending,
                    vec![VimAction::Operator {
                        op,
                        motion: Motion::Down,
                        count,
                        linewise: true,
                    }],
                );
            }
            Some(_) => {
                pending.clear();
                return Step::Consume(vec![]);
            }
            None => {
                pending.operator = Some(op);
                return Step::Pending;
            }
        }
    }

    // Everything else is a whole command of its own.
    let count = pending.count();
    let actions = match text {
        "i" => vec![VimAction::EnterInsert(InsertEntry::Here)],
        "a" => vec![VimAction::EnterInsert(InsertEntry::After)],
        "I" => vec![VimAction::EnterInsert(InsertEntry::LineStart)],
        "A" => vec![VimAction::EnterInsert(InsertEntry::LineEnd)],
        "o" => vec![VimAction::EnterInsert(InsertEntry::OpenBelow)],
        "O" => vec![VimAction::EnterInsert(InsertEntry::OpenAbove)],
        "v" if mode == Mode::Visual => vec![VimAction::EnterNormal],
        "v" => vec![VimAction::EnterVisual { line: false }],
        "V" if mode == Mode::VisualLine => vec![VimAction::EnterNormal],
        "V" => vec![VimAction::EnterVisual { line: true }],
        "x" if visual => vec![VimAction::OperatorOnSelection { op: Op::Delete }],
        "x" => vec![VimAction::DeleteChar { count }],
        "p" => vec![VimAction::Paste { after: true }],
        "P" => vec![VimAction::Paste { after: false }],
        "u" => vec![VimAction::Undo],
        "/" => vec![VimAction::StartSearch],
        "n" => vec![VimAction::SearchNext { reverse: false }],
        "N" => vec![VimAction::SearchNext { reverse: true }],
        "z" => vec![VimAction::CenterCaret],
        _ => vec![],
    };
    finish(pending, actions)
}

/// A motion, aimed by the pending operator when one waits.
fn motion_step(pending: &mut Pending, motion: Motion) -> Step {
    let count = pending.count();
    let step = match pending.operator {
        Some(op) => {
            // Vim's special case: `cw` behaves as `ce` — the trailing space survives.
            let motion = match (op, motion) {
                (Op::Change, Motion::WordForward { big }) => Motion::WordEnd { big },
                _ => motion,
            };
            Step::Consume(vec![VimAction::Operator {
                op,
                motion,
                count,
                linewise: false,
            }])
        }
        None => Step::Consume(vec![VimAction::Motion { motion, count }]),
    };
    pending.clear();
    step
}

/// Ends the collected prefix with `actions`.
fn finish(pending: &mut Pending, actions: Vec<VimAction>) -> Step {
    pending.clear();
    Step::Consume(actions)
}

/// The text a key means, when it means text.
fn character_of(key: &Key) -> Option<&str> {
    match key {
        Key::Character(text) => Some(text.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use zgui::vocab::PhysicalKey;

    use super::*;

    fn key(text: &str) -> KeyEvent {
        KeyEvent {
            key: Key::character(text),
            key_without_modifiers: Key::character(text.to_lowercase()),
            physical: PhysicalKey::Unidentified(0),
            location: Default::default(),
            repeat: false,
        }
    }

    fn normal(pending: &mut Pending, text: &str) -> Step {
        translate(Mode::Normal, pending, &key(text), Modifiers::NONE)
    }

    #[test]
    fn counts_aim_motions() {
        let mut pending = Pending::default();
        assert!(matches!(normal(&mut pending, "2"), Step::Pending));
        assert!(matches!(normal(&mut pending, "3"), Step::Pending));
        let step = normal(&mut pending, "w");
        match step {
            Step::Consume(actions) => match &actions[0] {
                VimAction::Motion { motion, count } => {
                    assert_eq!(*motion, Motion::WordForward { big: false });
                    assert_eq!(*count, 23);
                }
                other => panic!("a motion, not {other:?}"),
            },
            other => panic!("consumed, not {other:?}"),
        }
        assert!(pending.count.is_none(), "the prefix was spent");
    }

    #[test]
    fn dd_is_a_linewise_delete() {
        let mut pending = Pending::default();
        assert!(matches!(normal(&mut pending, "d"), Step::Pending));
        let step = normal(&mut pending, "d");
        match step {
            Step::Consume(actions) => match &actions[0] {
                VimAction::Operator { op, linewise, .. } => {
                    assert_eq!(*op, Op::Delete);
                    assert!(linewise);
                }
                other => panic!("an operator, not {other:?}"),
            },
            other => panic!("consumed, not {other:?}"),
        }
    }

    #[test]
    fn d2w_spans_two_words() {
        let mut pending = Pending::default();
        let _ = normal(&mut pending, "d");
        let _ = normal(&mut pending, "2");
        let step = normal(&mut pending, "w");
        match step {
            Step::Consume(actions) => match &actions[0] {
                VimAction::Operator {
                    op, motion, count, ..
                } => {
                    assert_eq!(*op, Op::Delete);
                    assert_eq!(*motion, Motion::WordForward { big: false });
                    assert_eq!(*count, 2);
                }
                other => panic!("an operator, not {other:?}"),
            },
            other => panic!("consumed, not {other:?}"),
        }
    }

    #[test]
    fn gg_goes_to_the_top_and_counts_aim_it() {
        let mut pending = Pending::default();
        let _ = normal(&mut pending, "g");
        let step = normal(&mut pending, "g");
        assert!(matches!(
            step,
            Step::Consume(ref actions)
                if matches!(actions[0], VimAction::Motion { motion: Motion::GotoLine(0), .. })
        ));

        let _ = normal(&mut pending, "5");
        let _ = normal(&mut pending, "g");
        let step = normal(&mut pending, "g");
        assert!(matches!(
            step,
            Step::Consume(ref actions)
                if matches!(actions[0], VimAction::Motion { motion: Motion::GotoLine(4), .. })
        ));
    }

    #[test]
    fn zero_is_a_motion_until_a_count_begins() {
        let mut pending = Pending::default();
        let step = normal(&mut pending, "0");
        assert!(matches!(
            step,
            Step::Consume(ref actions)
                if matches!(actions[0], VimAction::Motion { motion: Motion::LineStart, .. })
        ));
        let _ = normal(&mut pending, "1");
        assert!(matches!(normal(&mut pending, "0"), Step::Pending));
        assert_eq!(pending.count, Some(10));
    }

    #[test]
    fn insert_mode_passes_text_through() {
        let mut pending = Pending::default();
        let step = translate(Mode::Insert, &mut pending, &key("x"), Modifiers::NONE);
        assert!(matches!(step, Step::PassThrough));
    }

    #[test]
    fn escape_leaves_insert_and_steps_back() {
        let mut pending = Pending::default();
        let escape = KeyEvent {
            key: Key::Named(NamedKey::Escape),
            key_without_modifiers: Key::Named(NamedKey::Escape),
            physical: PhysicalKey::Unidentified(0),
            location: Default::default(),
            repeat: false,
        };
        let step = translate(Mode::Insert, &mut pending, &escape, Modifiers::NONE);
        match step {
            Step::Consume(actions) => {
                assert!(matches!(actions[0], VimAction::EnterNormal));
                assert!(matches!(
                    actions[1],
                    VimAction::Motion {
                        motion: Motion::Left,
                        ..
                    }
                ));
            }
            other => panic!("consumed, not {other:?}"),
        }
    }
}
