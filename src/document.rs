//! A buffer, and the views that share it.
//!
//! A [`Document`] is the text, its undo history and its editing options — everything about a
//! buffer that is not about *looking* at it. Hand one to two [`Editor`](crate::Editor)
//! components and they are two windows onto one file: an edit in either is the edit, either can
//! undo it, and neither can drift from the other. Each keeps its own carets, its own scroll
//! position and its own theme, which is what makes the second window worth opening.
//!
//! ```no_run
//! # use zgui::prelude::*;
//! # use zgui::{component, view};
//! # use zgui_editor::{Document, Editor, EditorProps};
//! #[component]
//! fn Split() -> impl IntoView {
//!     let document = Document::new("fn main() {}\n");
//!     view! {
//!         row {
//!             Editor(document = document.clone(), language = "rust")
//!             Editor(document = document, language = "rust")
//!         }
//!     }
//! }
//! ```
//!
//! An `Editor` given no document makes one of its own, so nothing about the single-view case
//! changes.
//!
//! # What happens when one view edits
//!
//! The acting view applies the change and then the document tells the others. Each of them maps
//! its own carets through the replacements, drops what it had shaped and painted for the lines
//! that moved, tells its highlighter, and repaints. None of that reaches the application: a
//! second view is a second window onto the same text, not a second source of events.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};

use crate::core::{ChangeInfo, DocumentState};
use crate::handle::{EditorCtx, EditorHandle};

/// Which view of a document this is.
///
/// Handed out by [`Document::attach`] and given back by [`Document::detach`]. A number rather than
/// a pointer because a view is detached from its own cleanup, by which time the thing the pointer
/// would name is already going away.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewId(u64);

impl ViewId {
    /// The identity a view has before it is attached to anything.
    ///
    /// Never handed out, so a broadcast that arrives before a view has registered — which cannot
    /// happen, but would be a silent wrong answer rather than a loud one if it did — skips
    /// nobody rather than skipping the first view.
    pub(crate) const UNATTACHED: Self = Self(u64::MAX);
}

/// A buffer several editors can show at once.
///
/// Cloning one is cloning a handle, not the text: every clone is the same document.
#[derive(Clone)]
pub struct Document {
    inner: Rc<DocumentInner>,
}

struct DocumentInner {
    /// The text, the history and the options.
    state: RefCell<DocumentState>,
    /// Every view showing this document, weakly — a view that has gone away must not keep its
    /// document alive, and a document that outlives its last view is a buffer nobody can see.
    views: RefCell<Vec<(ViewId, Weak<EditorCtx>)>>,
    /// The next identity to hand out.
    next: Cell<u64>,
}

impl Document {
    /// A document over `text`.
    pub fn new(text: &str) -> Self {
        Self {
            inner: Rc::new(DocumentInner {
                state: RefCell::new(DocumentState::new(text)),
                views: RefCell::new(Vec::new()),
                next: Cell::new(0),
            }),
        }
    }

    /// The text, as an O(1) clone of the rope.
    pub fn rope(&self) -> ropey::Rope {
        self.inner.state.borrow().buffer.rope().clone()
    }

    /// The whole text, copied out.
    pub fn text(&self) -> String {
        self.inner.state.borrow().buffer.rope().to_string()
    }

    /// Which revision of the text this is, moving once per change.
    pub fn revision(&self) -> u64 {
        self.inner.state.borrow().buffer.revision()
    }

    /// How many bytes the text is.
    pub fn len_bytes(&self) -> usize {
        self.inner.state.borrow().buffer.len_bytes()
    }

    /// How many lines the text has.
    pub fn line_count(&self) -> usize {
        crate::core::position::line_count(self.inner.state.borrow().buffer.rope())
    }

    /// How many views are showing this document.
    pub fn view_count(&self) -> usize {
        self.inner.views.borrow().len()
    }

    /// Whether two handles name the same document.
    pub fn is(&self, other: &Document) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    /// The model, for reading.
    pub(crate) fn state(&self) -> Ref<'_, DocumentState> {
        self.inner.state.borrow()
    }

    /// The model, for one command.
    pub(crate) fn state_mut(&self) -> RefMut<'_, DocumentState> {
        self.inner.state.borrow_mut()
    }

    /// Registers a view, answering the identity it detaches with.
    pub(crate) fn attach(&self, ctx: &Rc<EditorCtx>) -> ViewId {
        let id = ViewId(self.inner.next.get());
        self.inner.next.set(id.0 + 1);
        self.inner.views.borrow_mut().push((id, Rc::downgrade(ctx)));
        id
    }

    /// Removes a view.
    pub(crate) fn detach(&self, id: ViewId) {
        self.inner
            .views
            .borrow_mut()
            .retain(|(held, _)| *held != id);
    }

    /// Tells every view but `actor` that the text changed under it.
    ///
    /// The list is copied out before anything is told, because telling a view makes it read the
    /// document — and a borrow held across that is a panic rather than a bug that can be found by
    /// reading the two functions side by side.
    pub(crate) fn broadcast(&self, change: &ChangeInfo, actor: ViewId) {
        let others: Vec<Rc<EditorCtx>> = {
            let views = self.inner.views.borrow();
            if views.len() < 2 {
                return;
            }
            views
                .iter()
                .filter(|(id, _)| *id != actor)
                .filter_map(|(_, weak)| weak.upgrade())
                .collect()
        };
        for ctx in others {
            EditorHandle::from_ctx(ctx).follow(change);
        }
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Document")
            .field("revision", &self.revision())
            .field("views", &self.view_count())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Document;

    #[test]
    fn a_clone_is_the_same_document() {
        let one = Document::new("hello");
        let two = one.clone();
        assert!(one.is(&two));
        assert!(!one.is(&Document::new("hello")));
        assert_eq!(two.text(), "hello");
    }

    #[test]
    fn a_document_with_no_views_still_holds_its_text() {
        // Which is what makes a buffer that is open but not shown anywhere possible at all.
        let document = Document::new("one\ntwo\n");
        assert_eq!(document.view_count(), 0);
        // Three: the empty line a trailing break leaves is a line the caret can sit on, which is
        // what the rest of the editor counts by.
        assert_eq!(document.line_count(), 3);
        assert_eq!(document.len_bytes(), 8);
    }
}
