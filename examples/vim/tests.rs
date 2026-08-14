//! The whole layer, end to end: keys into a mounted editor, vim semantics out.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::vocab::{Key, NamedKey};
use zgui_editor::EditorHandle;
use zgui_testkit_view::Window;

use crate::filter::make_filter;
use crate::mode::{Mode, VimState};

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

struct Vim {
    window: Window,
    node: zgui::view::NodeId,
    handle: EditorHandle,
    state: Rc<RefCell<VimState>>,
}

fn mount(text: &str) -> Vim {
    let window = Window::open();
    window.place(window.root, 0.0, 0.0, 800.0, 600.0);
    let state = Rc::new(RefCell::new(VimState::new()));
    let filter = make_filter(Rc::clone(&state), |_| {});
    let taken: Rc<RefCell<Option<EditorHandle>>> = Rc::new(RefCell::new(None));
    let text = text.to_string();
    let built = {
        let taken = Rc::clone(&taken);
        window.scope.with(|| {
            let view = view! {
                Editor(
                    text = text.clone(),
                    autofocus = false,
                    on_ready = Box::new(move |handle| {
                        *taken.borrow_mut() = Some(handle);
                    }) as Box<dyn Fn(EditorHandle)>,
                    on_key = filter,
                )
            };
            use zgui::view::IntoView;
            let mut built = view.into_view().build(&mut window.cx.cx());
            built.mount(&window.dom_handle, window.root, None);
            built
        })
    };
    std::mem::forget(built);
    window.frame();
    let handle = taken.borrow_mut().take().expect("on_ready ran");
    let node = window
        .dom
        .tree()
        .children(window.root)
        .into_iter()
        .find(|child| !window.dom.tree().is_marker(*child))
        .expect("the editor mounted");
    Vim {
        window,
        node,
        handle,
        state,
    }
}

impl Vim {
    fn keys(&self, keys: &str) {
        for character in keys.chars() {
            self.window
                .dispatcher()
                .key(self.node, Key::character(character.to_string()));
            self.window.frame();
        }
    }

    fn escape(&self) {
        self.window
            .dispatcher()
            .key(self.node, Key::Named(NamedKey::Escape));
        self.window.frame();
    }

    fn enter(&self) {
        self.window
            .dispatcher()
            .key(self.node, Key::Named(NamedKey::Enter));
        self.window.frame();
    }

    fn text(&self) -> String {
        self.handle.query(|snapshot| snapshot.rope().to_string())
    }

    fn mode(&self) -> Mode {
        self.state.borrow().mode
    }

    fn caret(&self) -> usize {
        self.handle
            .query(|snapshot| snapshot.selections().primary().head)
    }
}

#[test]
fn normal_mode_swallows_letters_and_i_enters_insert() {
    let vim = mount("hello");
    vim.keys("q");
    assert_eq!(vim.text(), "hello", "normal mode types nothing");
    vim.keys("ix");
    assert_eq!(vim.mode(), Mode::Insert);
    assert_eq!(vim.text(), "xhello", "insert mode types");
    vim.escape();
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn motions_and_counts_move_the_caret() {
    let vim = mount("one two three four");
    vim.keys("2w");
    assert_eq!(vim.caret(), 8, "two words along");
    vim.keys("$");
    assert_eq!(vim.caret(), 18);
    vim.keys("0");
    assert_eq!(vim.caret(), 0);
    vim.keys("e");
    assert_eq!(vim.caret(), 2, "on the word's last character");
}

#[test]
fn dd_deletes_the_line_and_p_puts_it_below() {
    let vim = mount("one\ntwo\nthree");
    vim.keys("dd");
    assert_eq!(vim.text(), "two\nthree");
    vim.keys("p");
    assert_eq!(vim.text(), "two\none\nthree", "the line lands below");
}

#[test]
fn dw_deletes_a_word_and_undo_restores_it() {
    let vim = mount("one two three");
    vim.keys("dw");
    assert_eq!(vim.text(), "two three");
    vim.keys("u");
    assert_eq!(vim.text(), "one two three");
}

#[test]
fn x_deletes_under_the_caret_with_count() {
    let vim = mount("abcdef");
    vim.keys("3x");
    assert_eq!(vim.text(), "def");
}

#[test]
fn cw_changes_a_word_into_insert_mode() {
    let vim = mount("one two");
    vim.keys("cw");
    assert_eq!(vim.mode(), Mode::Insert);
    vim.keys("ONE");
    assert_eq!(vim.text(), "ONE two");
}

#[test]
fn yy_then_p_duplicates_the_line() {
    let vim = mount("alpha\nbeta");
    vim.keys("yyp");
    assert_eq!(vim.text(), "alpha\nalpha\nbeta");
}

#[test]
fn visual_selection_deletes_what_it_covers() {
    let vim = mount("one two three");
    vim.keys("vw");
    assert_eq!(vim.mode(), Mode::Visual);
    vim.keys("d");
    assert_eq!(vim.text(), "two three");
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn visual_line_takes_whole_lines() {
    let vim = mount("one\ntwo\nthree");
    vim.keys("Vj");
    assert_eq!(vim.mode(), Mode::VisualLine);
    vim.keys("d");
    assert_eq!(vim.text(), "three");
}

#[test]
fn gg_and_capital_g_jump_the_document() {
    let vim = mount("one\ntwo\nthree");
    vim.keys("G");
    assert!(vim.caret() >= 8, "at the last line");
    vim.keys("gg");
    assert_eq!(vim.caret(), 0);
}

#[test]
fn o_opens_below_and_capital_o_above() {
    let vim = mount("one\ntwo");
    vim.keys("o");
    vim.keys("new");
    vim.escape();
    assert_eq!(vim.text(), "one\nnew\ntwo");
    vim.keys("gg");
    vim.keys("O");
    vim.keys("top");
    vim.escape();
    assert_eq!(vim.text(), "top\none\nnew\ntwo");
}

#[test]
fn slash_searches_and_n_repeats() {
    let vim = mount("alpha beta alpha beta");
    vim.keys("/beta");
    vim.enter();
    assert_eq!(vim.caret(), 6);
    vim.keys("n");
    assert_eq!(vim.caret(), 17);
    vim.keys("n");
    assert_eq!(vim.caret(), 6, "the search wraps");
}
