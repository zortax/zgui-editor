//! Two views of one document, driven through a testkit window.
//!
//! What is asserted is the whole point of a shared document: an edit made in either view is the
//! edit, both views see it, each keeps its own carets, and undo belongs to the document rather
//! than to whichever window happened to be focused.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui_editor::{Command, Document, EditorHandle, Motion, Selection};
use zgui_testkit_view::Window;

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

/// One document shown by two editors, with a handle to each.
struct Split {
    window: Window,
    document: Document,
    left: EditorHandle,
    right: EditorHandle,
    /// How many times each view reported a change, left first.
    reports: Rc<RefCell<(usize, usize)>>,
}

fn split(text: &str) -> Split {
    let window = Window::open();
    window.place(window.root, 0.0, 0.0, 800.0, 600.0);
    let document = Document::new(text);
    let taken: Rc<RefCell<Vec<EditorHandle>>> = Rc::new(RefCell::new(Vec::new()));
    let reports: Rc<RefCell<(usize, usize)>> = Rc::new(RefCell::new((0, 0)));

    let built = {
        let taken = Rc::clone(&taken);
        let document = document.clone();
        window.scope.with(|| {
            let one = Rc::clone(&taken);
            let two = Rc::clone(&taken);
            let told_left = Rc::clone(&reports);
            let told_right = Rc::clone(&reports);
            let view = view! {
                row {
                    Editor(
                        document = document.clone(),
                        autofocus = false,
                        on_ready = Box::new(move |handle| one.borrow_mut().push(handle))
                            as Box<dyn Fn(EditorHandle)>,
                        on_event = Box::new(move |event| {
                            if matches!(event, zgui_editor::EditorEvent::Edited { .. }) {
                                told_left.borrow_mut().0 += 1;
                            }
                        }) as Box<dyn Fn(zgui_editor::EditorEvent)>,
                    )
                    Editor(
                        document = document.clone(),
                        autofocus = false,
                        on_ready = Box::new(move |handle| two.borrow_mut().push(handle))
                            as Box<dyn Fn(EditorHandle)>,
                        on_event = Box::new(move |event| {
                            if matches!(event, zgui_editor::EditorEvent::Edited { .. }) {
                                told_right.borrow_mut().1 += 1;
                            }
                        }) as Box<dyn Fn(zgui_editor::EditorEvent)>,
                    )
                }
            };
            use zgui::view::IntoView;
            let mut built = view.into_view().build(&mut window.cx.cx());
            built.mount(&window.dom_handle, window.root, None);
            built
        })
    };
    // The view outlives the test; dropping it would unmount both editors.
    std::mem::forget(built);
    window.frame();

    let mut handles = taken.borrow_mut();
    assert_eq!(handles.len(), 2, "both editors reported their handles");
    let right = handles.pop().expect("two handles");
    let left = handles.pop().expect("two handles");
    drop(handles);

    Split {
        window,
        document,
        left,
        right,
        reports,
    }
}

fn text_of(handle: &EditorHandle) -> String {
    handle.query(|snapshot| snapshot.rope().to_string())
}

fn head_of(handle: &EditorHandle) -> usize {
    handle.query(|snapshot| snapshot.selections().primary().head)
}

#[test]
fn both_views_show_one_document() {
    let split = split("hello\n");
    assert_eq!(split.document.view_count(), 2);
    assert_eq!(text_of(&split.left), "hello\n");
    assert_eq!(text_of(&split.right), "hello\n");
}

#[test]
fn an_edit_in_one_view_is_the_text_in_the_other() {
    let split = split("hello\n");
    split.left.command(Command::Insert("say ".to_string()));
    split.window.frame();

    assert_eq!(text_of(&split.right), "say hello\n");
    assert_eq!(split.document.text(), "say hello\n");
    assert_eq!(
        split.document.revision(),
        split.right.revision().get(),
        "the following view's revision signal moved too"
    );
}

#[test]
fn each_view_keeps_its_own_carets() {
    let split = split("hello world\n");
    split.right.command(Command::Move {
        motion: Motion::LineEnd,
        count: 1,
        extend: false,
    });
    split.window.frame();

    assert_eq!(head_of(&split.left), 0, "the left view did not move");
    assert_eq!(head_of(&split.right), 11);
}

#[test]
fn a_following_view_moves_its_carets_through_the_change() {
    // The right view's caret sits after the place the left one types, so it has to move by
    // exactly what was inserted — otherwise the second window drifts one character per keystroke.
    let split = split("hello world\n");
    split.right.command(Command::SetSelections {
        selections: vec![Selection::caret(6)],
        primary: 0,
    });
    split.window.frame();
    assert_eq!(head_of(&split.right), 6);

    split.left.command(Command::Insert("say ".to_string()));
    split.window.frame();

    assert_eq!(text_of(&split.right), "say hello world\n");
    assert_eq!(head_of(&split.right), 10, "the caret followed its word");
    assert_eq!(
        head_of(&split.left),
        4,
        "the typing view's caret is after what it typed"
    );
}

#[test]
fn a_caret_before_the_change_stays_where_it_was() {
    let split = split("hello world\n");
    split.right.command(Command::SetSelections {
        selections: vec![Selection::caret(2)],
        primary: 0,
    });
    split.left.command(Command::SetSelections {
        selections: vec![Selection::caret(6)],
        primary: 0,
    });
    split.window.frame();

    split.left.command(Command::Insert("brave ".to_string()));
    split.window.frame();

    assert_eq!(text_of(&split.right), "hello brave world\n");
    assert_eq!(head_of(&split.right), 2, "nothing before the edit moved");
}

#[test]
fn undo_belongs_to_the_document_rather_than_the_window() {
    let split = split("hello\n");
    split.left.command(Command::Insert("say ".to_string()));
    split.window.frame();
    assert_eq!(split.document.text(), "say hello\n");

    // Undone from the other window, because it is the change that is undone, not that window's
    // share of it.
    split.right.command(Command::Undo);
    split.window.frame();
    assert_eq!(split.document.text(), "hello\n");
    assert_eq!(text_of(&split.left), "hello\n");
}

#[test]
fn replacing_the_text_puts_every_view_back_at_the_start() {
    let split = split("hello world\n");
    split.right.command(Command::SetSelections {
        selections: vec![Selection::caret(8)],
        primary: 0,
    });
    split.window.frame();

    split.left.set_text("a much shorter one\n");
    split.window.frame();

    assert_eq!(text_of(&split.right), "a much shorter one\n");
    assert_eq!(
        head_of(&split.right),
        0,
        "a replaced text has no positions to carry forward"
    );
}

#[test]
fn a_deletion_pulls_a_later_caret_back() {
    let split = split("hello brave world\n");
    split.right.command(Command::SetSelections {
        selections: vec![Selection::caret(12)],
        primary: 0,
    });
    split.left.command(Command::SetSelections {
        selections: vec![Selection::new(6, 12)],
        primary: 0,
    });
    split.window.frame();

    split.left.command(Command::DeleteSelection);
    split.window.frame();

    assert_eq!(text_of(&split.right), "hello world\n");
    assert_eq!(
        head_of(&split.right),
        6,
        "the caret came back with the text"
    );
}

#[test]
fn an_edit_with_no_view_acting_reaches_every_view() {
    // What a table cell or a drawing edits through. No view issued it, so both have to hear it,
    // including the one a `broadcast` would have called the actor.
    let split = split("hello world\n");
    split.right.command(Command::SetSelections {
        selections: vec![Selection::caret(6)],
        primary: 0,
    });
    split.window.frame();

    assert!(split.document.apply(vec![(0..5, "goodbye".to_string())]));
    split.window.frame();

    assert_eq!(text_of(&split.left), "goodbye world\n");
    assert_eq!(text_of(&split.right), "goodbye world\n");
    assert_eq!(head_of(&split.right), 8, "the caret followed the change");
    assert_eq!(
        split.document.revision(),
        split.left.revision().get(),
        "both revision signals moved"
    );
}

#[test]
fn a_change_nobody_acted_for_is_reported_once() {
    // An application hangs its dirty mark, its session writes and its language servers off the
    // edited event. A change with no view behind it reaches none of them unless one view reports
    // it — and two views reporting it would count one edit twice.
    let split = split("one\n");
    assert_eq!(*split.reports.borrow(), (0, 0));

    split.document.apply(vec![(0..3, "two".to_string())]);
    split.window.frame();

    let (left, right) = *split.reports.borrow();
    assert_eq!(left + right, 1, "one change, one report");
}

#[test]
fn a_view_undoes_what_no_view_did() {
    let split = split("one\n");
    split.document.apply(vec![(0..3, "two".to_string())]);
    split.window.frame();
    assert_eq!(text_of(&split.left), "two\n");

    split.left.command(Command::Undo);
    split.window.frame();

    assert_eq!(text_of(&split.right), "one\n");
    assert_eq!(head_of(&split.left), 0, "undo landed on the change");
}
