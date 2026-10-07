//! What a secondary press does to the focus, driven through a testkit window.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::vocab::{EventKind, Payload, PointerButton, PointerEvent};
use zgui_editor::{EditorConfig, EditorHandle};
use zgui_testkit_view::{Command, Window};

#[allow(unused_imports)]
use zgui_editor::{Editor, EditorProps};

/// Mounts an editor built with `config`, presses it with the secondary button, and answers
/// whether the press asked for the focus.
fn secondary_press_focuses(config: EditorConfig) -> bool {
    let window = Window::open();
    window.place(window.root, 0.0, 0.0, 800.0, 600.0);
    let taken: Rc<RefCell<Option<EditorHandle>>> = Rc::new(RefCell::new(None));
    let built = {
        let taken = Rc::clone(&taken);
        window.scope.with(|| {
            let view = view! {
                Editor(
                    text = "one\ntwo\n".to_owned(),
                    config = config,
                    autofocus = false,
                    on_ready = Box::new(move |handle| {
                        *taken.borrow_mut() = Some(handle);
                    }) as Box<dyn Fn(EditorHandle)>,
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
    let node = window
        .dom
        .tree()
        .children(window.root)
        .into_iter()
        .find(|child| !window.dom.tree().is_marker(*child))
        .expect("the editor mounted one element");
    let mut stack = vec![node];
    while let Some(next) = stack.pop() {
        window.place(next, 0.0, 0.0, 800.0, 600.0);
        stack.extend(window.dom.tree().children(next));
    }
    window.frame();
    let position = zgui::geom::Point::new(zgui::geom::CssPx(20.0), zgui::geom::CssPx(10.0));
    let press = PointerEvent::mouse(position).with_button(PointerButton::Secondary);
    let delivered =
        window
            .dispatcher()
            .send_to(node, EventKind::PointerDown, Payload::Pointer(press));
    delivered
        .commands
        .iter()
        .any(|command| matches!(command, Command::RequestFocus(_)))
}

#[test]
fn a_secondary_press_takes_the_focus_by_default() {
    assert!(secondary_press_focuses(EditorConfig::default()));
}

#[test]
fn a_secondary_press_can_leave_the_focus_where_it_is() {
    let config = EditorConfig {
        focus_on_secondary: false,
        ..EditorConfig::default()
    };
    assert!(!secondary_press_focuses(config));
}
