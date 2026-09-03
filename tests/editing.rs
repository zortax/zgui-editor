//! The key pipeline, driven through a testkit window: keys become commands become text.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::vocab::{Key, NamedKey};
use zgui_editor::{Command, EditorHandle, Motion};
use zgui_testkit_view::Window;

/// The editor, mounted, with its handle and its focusable node.
struct Mounted {
    window: Window,
    node: zgui::view::NodeId,
    handle: EditorHandle,
}

fn mount(text: &str, on_key: Option<zgui_editor::KeyFilter>) -> Mounted {
    mount_with(text, on_key, true)
}

fn mount_with(text: &str, on_key: Option<zgui_editor::KeyFilter>, focusable: bool) -> Mounted {
    let window = Window::open();
    window.place(window.root, 0.0, 0.0, 800.0, 600.0);
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
                    on_key = on_key,
                    focusable = focusable,
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
    let handle = taken.borrow_mut().take().expect("on_ready ran at build");
    let node = window
        .dom
        .tree()
        .children(window.root)
        .into_iter()
        .find(|child| !window.dom.tree().is_marker(*child))
        .expect("the editor mounted one element");
    Mounted {
        window,
        node,
        handle,
    }
}

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

fn press(mounted: &Mounted, key: Key) {
    mounted.window.dispatcher().key(mounted.node, key);
    mounted.window.frame();
}

fn press_with(mounted: &Mounted, key: Key, modifiers: Modifiers) {
    mounted
        .window
        .dispatcher()
        .with_modifiers(modifiers)
        .key(mounted.node, key);
    mounted.window.frame();
}

fn type_str(mounted: &Mounted, text: &str) {
    for character in text.chars() {
        press(mounted, Key::character(character.to_string()));
    }
}

fn text_of(mounted: &Mounted) -> String {
    mounted.handle.query(|snapshot| snapshot.rope().to_string())
}

#[test]
fn typing_inserts_text() {
    let mounted = mount("", None);
    type_str(&mounted, "hello");
    assert_eq!(text_of(&mounted), "hello");
    assert_eq!(mounted.handle.query(|s| s.selections().primary().head), 5);
}

#[test]
fn enter_and_backspace_edit_lines() {
    let mounted = mount("", None);
    type_str(&mounted, "ab");
    press(&mounted, Key::Named(NamedKey::Enter));
    type_str(&mounted, "cd");
    assert_eq!(text_of(&mounted), "ab\ncd");
    press(&mounted, Key::Named(NamedKey::Backspace));
    assert_eq!(text_of(&mounted), "ab\nc");
}

#[test]
fn arrows_move_and_shift_extends() {
    let mounted = mount("hello", None);
    press(&mounted, Key::Named(NamedKey::ArrowRight));
    press(&mounted, Key::Named(NamedKey::ArrowRight));
    assert_eq!(mounted.handle.query(|s| s.selections().primary().head), 2);
    press_with(&mounted, Key::Named(NamedKey::ArrowRight), Modifiers::SHIFT);
    let selection = mounted.handle.query(|s| s.selections().primary());
    assert_eq!(selection.anchor, 2);
    assert_eq!(selection.head, 3);
}

#[test]
fn ctrl_z_undoes_typing() {
    let mounted = mount("", None);
    type_str(&mounted, "abc");
    press_with(&mounted, Key::character("z"), Modifiers::CONTROL);
    assert_eq!(text_of(&mounted), "", "one burst of typing undoes as one");
}

#[test]
fn an_editor_that_cannot_take_focus_leaves_every_key_alone() {
    let mounted = mount_with("", None, false);
    type_str(&mounted, "x");
    assert_eq!(text_of(&mounted), "", "the default keymap never ran");
    // A command still drives it: the keys are what it declines, not the handle.
    mounted.handle.command(Command::Insert("y".to_string()));
    mounted.window.frame();
    assert_eq!(text_of(&mounted), "y");
}

#[test]
fn a_key_filter_consumes_before_the_default_keymap() {
    let filter: zgui_editor::KeyFilter =
        Box::new(|event, _modifiers, _handle| event.key == Key::Named(NamedKey::Tab));
    let mounted = mount("", Some(filter));
    press(&mounted, Key::Named(NamedKey::Tab));
    assert_eq!(text_of(&mounted), "", "the filter kept the tab");
    type_str(&mounted, "x");
    assert_eq!(text_of(&mounted), "x", "everything else falls through");
}

#[test]
fn commands_drive_the_editor_directly() {
    let mounted = mount("one\ntwo\nthree", None);
    mounted.handle.command(Command::Move {
        motion: Motion::Down,
        count: 1,
        extend: false,
    });
    mounted.handle.command(Command::DeleteMotion {
        motion: Motion::Down,
        count: 1,
        linewise: true,
    });
    mounted.window.frame();
    assert_eq!(text_of(&mounted), "one\nthree");
}

#[test]
fn the_revision_signal_follows_edits() {
    let mounted = mount("", None);
    let revision = mounted.handle.revision();
    assert_eq!(revision.get_untracked(), 0);
    type_str(&mounted, "a");
    assert_eq!(revision.get_untracked(), 1);
}
