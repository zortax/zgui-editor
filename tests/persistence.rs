//! Writing a document's undo history down, and putting it back.
//!
//! What a session needs from this crate: the text and the history of every buffer somebody left
//! open, encoded, decoded, and still undoable. The encoding here is JSON because the test only
//! cares that the types round-trip; an application picks its own.

#![cfg(feature = "serde")]

use zgui_editor::{Command, Document, DocumentState, EditorState, History, Selections, Step};

/// How many lines a view of these documents shows. Nothing here scrolls.
const VIEW: zgui_editor::core::motion::MotionContext =
    zgui_editor::core::motion::MotionContext { viewport_lines: 10 };

/// A document with three separate undo steps typed into it.
fn typed() -> (String, Vec<Step>) {
    let mut doc = DocumentState::new("");
    let mut selections = Selections::caret(0);
    for text in ["one\n", "two\n", "three\n"] {
        let mut editor = EditorState {
            doc: &mut doc,
            selections: &mut selections,
        };
        editor.apply(&Command::Insert(text.to_owned()), VIEW);
        // Sealing is what makes each its own step, as a motion or a pause would.
        editor.doc.history.seal();
    }
    (
        doc.buffer.rope().to_string(),
        doc.history.undo_steps().to_vec(),
    )
}

#[test]
fn a_history_survives_being_written_down_and_read_back() {
    let (text, steps) = typed();
    assert!(steps.len() >= 2, "there is a history to lose");

    let encoded = serde_json::to_vec(&steps).expect("it encodes");
    let decoded: Vec<Step> = serde_json::from_slice(&encoded).expect("it decodes");

    let restored = Document::restore(&text, History::from_parts(decoded, Vec::new()));
    assert_eq!(restored.text(), text);
    assert_eq!(
        restored.with_history(History::undo_depth),
        steps.len(),
        "every step came back",
    );
}

#[test]
fn a_restored_document_undoes_back_past_the_restart() {
    // The whole point of writing a history down: `u` reaches text from the run before.
    let (text, steps) = typed();
    let encoded = serde_json::to_vec(&steps).expect("it encodes");
    let decoded: Vec<Step> = serde_json::from_slice(&encoded).expect("it decodes");

    let mut doc = DocumentState::restore(&text, History::from_parts(decoded, Vec::new()));
    let mut selections = Selections::caret(0);
    let mut editor = EditorState {
        doc: &mut doc,
        selections: &mut selections,
    };
    editor.apply(&Command::Undo, VIEW);

    assert_ne!(doc.buffer.rope().to_string(), text, "undo went back");
    assert!(doc.buffer.rope().to_string().contains("one"), "not too far");
}

#[test]
fn restoring_never_clears_what_it_was_given() {
    // `set_text` clears history by design, and a restore that went through it would silently
    // throw away everything it had just decoded.
    let (text, steps) = typed();
    let depth = steps.len();
    let doc = DocumentState::restore(&text, History::from_parts(steps, Vec::new()));
    assert_eq!(doc.history.undo_depth(), depth);
}

#[test]
fn selections_cannot_decode_into_an_invalid_set() {
    // The wire type routes through `Selections::new`, which sorts, merges and clamps. A file
    // nobody wrote by hand still must not produce a set the rest of the editor cannot use.
    let selections: Selections =
        serde_json::from_str(r#"{"list":[],"primary":99}"#).expect("it decodes");
    assert_eq!(selections.len(), 1, "there is always a caret");
    assert_eq!(selections.primary_index(), 0);
}

#[test]
fn a_set_of_selections_round_trips() {
    let selections = Selections::new(
        vec![
            zgui_editor::Selection::new(0, 3),
            zgui_editor::Selection::new(10, 14),
        ],
        1,
    );
    let encoded = serde_json::to_string(&selections).expect("it encodes");
    let decoded: Selections = serde_json::from_str(&encoded).expect("it decodes");
    assert_eq!(decoded.len(), selections.len());
    assert_eq!(decoded.primary(), selections.primary());
}
