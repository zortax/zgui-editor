//! What a key means when nothing above the editor has claimed it.
//!
//! This is the whole default keymap, as one pure function: an event and the held modifiers in,
//! a [`Command`] out. Purity is what makes it testable without a window, and what makes it
//! replaceable — a vim layer runs *before* it and consumes what it wants; whatever it lets
//! through still means what any plain editor means.

use zgui::vocab::{Key, KeyEvent, Modifiers, NamedKey};

use crate::command::{Clipboard, Command, Motion};

/// The command `event` means by default, when it means one.
///
/// Shortcuts are matched against [`KeyEvent::key_without_modifiers`], so Ctrl+Z is Ctrl+Z on
/// every layout; insertion reads [`Key::inserted_text`] off the modified key, which is the text
/// the layout actually produced.
pub fn default_keymap(event: &KeyEvent, modifiers: Modifiers) -> Option<Command> {
    let extend = modifiers.shift();

    // Movement, with shift extending the selection.
    if let Key::Named(named) = &event.key {
        let motion = match named {
            NamedKey::ArrowLeft => Some(Motion::Left),
            NamedKey::ArrowRight => Some(Motion::Right),
            NamedKey::ArrowUp => Some(Motion::Up),
            NamedKey::ArrowDown => Some(Motion::Down),
            NamedKey::Home if modifiers.control() => Some(Motion::DocumentStart),
            NamedKey::End if modifiers.control() => Some(Motion::DocumentEnd),
            NamedKey::Home => Some(Motion::LineFirstNonBlank),
            NamedKey::End => Some(Motion::LineEnd),
            NamedKey::PageUp => Some(Motion::PageUp),
            NamedKey::PageDown => Some(Motion::PageDown),
            _ => None,
        };
        if let Some(mut motion) = motion {
            // Ctrl with the horizontal arrows steps by words, as every desktop editor has it.
            if modifiers.control() {
                motion = match motion {
                    Motion::Left => Motion::WordBackward { big: false },
                    Motion::Right => Motion::WordForward { big: false },
                    other => other,
                };
            }
            return Some(Command::Move {
                motion,
                count: 1,
                extend,
            });
        }
        match named {
            NamedKey::Backspace => return Some(Command::Backspace),
            NamedKey::Delete => return Some(Command::DeleteForward),
            NamedKey::Enter => return Some(Command::InsertNewline),
            NamedKey::Tab if modifiers.shift() => {
                return Some(Command::IndentLines { dedent: true });
            }
            NamedKey::Tab => return Some(Command::InsertIndent),
            NamedKey::Copy => return Some(Command::Copy(Clipboard::Standard)),
            NamedKey::Cut => return Some(Command::Cut(Clipboard::Standard)),
            NamedKey::Paste => return Some(Command::Paste(Clipboard::Standard)),
            NamedKey::Undo => return Some(Command::Undo),
            NamedKey::Redo => return Some(Command::Redo),
            _ => {}
        }
    }

    // Control shortcuts, layout-independent.
    if modifiers.control() && !modifiers.alt() {
        if let Key::Character(text) = &event.key_without_modifiers {
            match text.as_str() {
                "a" => return Some(Command::SelectAll),
                "c" => return Some(Command::Copy(Clipboard::Standard)),
                "x" => return Some(Command::Cut(Clipboard::Standard)),
                "v" => return Some(Command::Paste(Clipboard::Standard)),
                "z" if modifiers.shift() => return Some(Command::Redo),
                "z" => return Some(Command::Undo),
                "y" => return Some(Command::Redo),
                _ => return None,
            }
        }
        return None;
    }

    // Everything else that produces text inserts it.
    if !modifiers.control()
        && !modifiers.meta()
        && let Some(text) = event.key.inserted_text()
        && !text.is_empty()
        && !text.chars().any(|character| character.is_control())
    {
        return Some(Command::Insert(text.to_string()));
    }

    None
}

#[cfg(test)]
mod tests {
    use zgui::vocab::PhysicalKey;

    use super::*;

    fn key(key: Key, without: Key) -> KeyEvent {
        KeyEvent {
            key,
            key_without_modifiers: without,
            physical: PhysicalKey::Unidentified(0),
            location: Default::default(),
            repeat: false,
        }
    }

    fn character(text: &str) -> KeyEvent {
        key(Key::character(text), Key::character(text))
    }

    fn named(named: NamedKey) -> KeyEvent {
        key(Key::Named(named), Key::Named(named))
    }

    #[test]
    fn a_character_inserts_itself() {
        let command = default_keymap(&character("x"), Modifiers::NONE);
        assert!(matches!(command, Some(Command::Insert(text)) if text == "x"));
    }

    #[test]
    fn space_inserts_a_space() {
        let command = default_keymap(&named(NamedKey::Space), Modifiers::NONE);
        assert!(matches!(command, Some(Command::Insert(text)) if text == " "));
    }

    #[test]
    fn shift_extends_movement() {
        let command = default_keymap(&named(NamedKey::ArrowRight), Modifiers::SHIFT);
        assert!(matches!(
            command,
            Some(Command::Move {
                motion: Motion::Right,
                extend: true,
                ..
            })
        ));
    }

    #[test]
    fn control_shortcuts_match_the_unmodified_key() {
        let event = key(Key::character("\u{1a}"), Key::character("z"));
        let command = default_keymap(&event, Modifiers::CONTROL);
        assert!(matches!(command, Some(Command::Undo)));
        let command = default_keymap(&event, Modifiers::CONTROL | Modifiers::SHIFT);
        assert!(matches!(command, Some(Command::Redo)));
    }

    #[test]
    fn ctrl_arrows_step_by_words() {
        let command = default_keymap(&named(NamedKey::ArrowRight), Modifiers::CONTROL);
        assert!(matches!(
            command,
            Some(Command::Move {
                motion: Motion::WordForward { big: false },
                ..
            })
        ));
    }

    #[test]
    fn an_unclaimed_control_chord_does_nothing() {
        let event = key(Key::character("k"), Key::character("k"));
        assert!(default_keymap(&event, Modifiers::CONTROL).is_none());
    }
}
