//! A document its owner writes while a view reads it, driven through a testkit window.
//!
//! What is asserted is what a log viewer needs: a read-only view takes the owner's text, keeps no
//! undo step for it, and keeps its reader on the same line while lines above it go.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui_editor::{Command, Document, EditOptions, EditorConfig, EditorHandle, ScrollCmd};
use zgui_testkit_view::Window;

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

/// One read-only editor over `document`, and its handle.
fn reader(document: &Document) -> (Window, EditorHandle) {
    let window = Window::open();
    window.place(window.root, 0.0, 0.0, 800.0, 600.0);
    let taken: Rc<RefCell<Option<EditorHandle>>> = Rc::new(RefCell::new(None));
    let config = EditorConfig {
        edit: EditOptions {
            read_only: true,
            ..EditOptions::default()
        },
        smooth_scroll: false,
        ..EditorConfig::default()
    };

    let built = {
        let taken = Rc::clone(&taken);
        let document = document.clone();
        window.scope.with(|| {
            let view = view! {
                Editor(
                    document = document,
                    config = config,
                    autofocus = false,
                    on_ready = Box::new(move |handle| *taken.borrow_mut() = Some(handle))
                        as Box<dyn Fn(EditorHandle)>,
                )
            };
            use zgui::view::IntoView;
            let mut built = view.into_view().build(&mut window.cx.cx());
            built.mount(&window.dom_handle, window.root, None);
            built
        })
    };
    // The view outlives the test; dropping it would unmount the editor.
    std::mem::forget(built);
    window.frame();
    let handle = taken
        .borrow_mut()
        .take()
        .expect("the editor reported its handle");
    (window, handle)
}

/// `count` numbered lines, each ending in a break.
fn lines(from: usize, count: usize) -> String {
    (from..from + count)
        .map(|at| format!("line {at}\n"))
        .collect()
}

#[test]
fn a_read_only_view_shows_what_its_owner_writes() {
    let document = Document::new("");
    let (window, handle) = reader(&document);

    handle.command(Command::Insert("typed".to_owned()));
    assert!(document.write(vec![(0..0, lines(0, 3))]));
    window.frame();

    let text = handle.query(|snapshot| snapshot.rope().to_string());
    assert_eq!(
        text,
        lines(0, 3),
        "the owner writes and the reader types nothing"
    );
    handle.command(Command::Undo);
    assert_eq!(
        document.text(),
        lines(0, 3),
        "a write leaves nothing to undo"
    );
}

#[test]
fn the_reader_stays_on_its_line_while_lines_above_it_go() {
    let document = Document::new(&lines(0, 500));
    let (window, handle) = reader(&document);
    handle.command(Command::Scroll(ScrollCmd::ToLine(200)));
    window.frame();
    assert_eq!(handle.scroll_state().get_untracked().top_line, 200.0);

    let cut = document.rope().line_to_byte(50);
    assert!(document.write(vec![(0..cut, String::new())]));
    window.frame();

    assert_eq!(
        handle.scroll_state().get_untracked().top_line,
        150.0,
        "the view moved up by the lines that went"
    );
    let top = handle.query(|snapshot| {
        let rope = snapshot.rope();
        rope.line(150).to_string()
    });
    assert_eq!(top, "line 200\n");
}

#[test]
fn lines_added_below_the_reader_leave_it_in_place() {
    let document = Document::new(&lines(0, 500));
    let (window, handle) = reader(&document);
    handle.command(Command::Scroll(ScrollCmd::ToLine(100)));
    window.frame();

    let end = document.len_bytes();
    assert!(document.write(vec![(end..end, lines(500, 100))]));
    window.frame();

    assert_eq!(handle.scroll_state().get_untracked().top_line, 100.0);
}
