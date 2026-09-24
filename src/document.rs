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
use std::ops::Range;
use std::rc::{Rc, Weak};

use crate::command::Command;
use crate::core::motion::MotionContext;
use crate::core::selection::Selections;
use crate::core::{ChangeInfo, DocumentState, EditorState};
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

    /// A document over `text` that already has `history` behind it.
    ///
    /// What a session restores with. No view has attached yet, so there is nothing to tell about
    /// the text and no cache to make stale; handing this to an editor is the same as handing it a
    /// document read from a file, except that undo reaches back past the restart.
    pub fn restore(text: &str, history: crate::core::history::History) -> Self {
        Self {
            inner: Rc::new(DocumentInner {
                state: RefCell::new(DocumentState::restore(text, history)),
                views: RefCell::new(Vec::new()),
                next: Cell::new(0),
            }),
        }
    }

    /// Reads the undo history, for something that is writing it down.
    pub fn with_history<R>(&self, read: impl FnOnce(&crate::core::history::History) -> R) -> R {
        read(&self.inner.state.borrow().history)
    }

    /// Replaces `replacements` in the text, with no view acting.
    ///
    /// What something that is not an editor edits through: a cell in a table, an attribute in a
    /// drawing, a checkbox in a rendered document. The change goes into the shared history, so
    /// undo in a text view of the same buffer takes it back, and every view is told.
    ///
    /// One view also *reports* it, as [`EditorEvent::Edited`](crate::EditorEvent::Edited). An
    /// application hangs its dirty mark, its session writes and its language servers off that
    /// event, and a change nobody acted for would otherwise reach none of them.
    ///
    /// Each range addresses the text as it is now. Overlapping ranges are a caller mistake and
    /// the earlier one wins, which is what [`Command::ReplaceRanges`] does. Answers whether
    /// anything changed: a read-only document, and a list that inserts nothing anywhere, both
    /// answer `false`.
    ///
    /// ```
    /// # use zgui_editor::Document;
    /// let document = Document::new("a,b\n1,2\n");
    /// assert!(document.apply(vec![(4..5, "9".to_owned())]));
    /// assert_eq!(document.text(), "a,b\n9,2\n");
    /// ```
    pub fn apply(&self, replacements: Vec<(Range<usize>, String)>) -> bool {
        // Where undo puts the caret afterwards, which is the earliest byte the change touched.
        // A caret at zero would send somebody who undid a change at the end of a file to the top
        // of it.
        let at = replacements
            .iter()
            .map(|(range, _)| range.start)
            .min()
            .unwrap_or(0);

        let change = {
            let mut state = self.inner.state.borrow_mut();
            let at = crate::core::position::snap(state.buffer.rope(), at);
            let mut selections = Selections::caret(at);
            EditorState {
                doc: &mut state,
                selections: &mut selections,
            }
            .apply(
                &Command::ReplaceRanges(replacements),
                // No view acts, so no motion resolves and the page size is never read.
                MotionContext { viewport_lines: 1 },
            )
            .change
        };
        self.spread(change)
    }

    /// Replaces `replacements` in the text as its owner, with no view acting.
    ///
    /// The change goes through a read-only document and records no undo step, so the history
    /// starts empty after it. This is how a document that shows text written elsewhere takes new
    /// text: a log gets its newest lines at the end and loses its oldest at the start, while the
    /// people who read it can select and copy but never type. Every view is told, and one view
    /// reports it, as [`apply`](Self::apply) does.
    ///
    /// ```
    /// # use zgui_editor::Document;
    /// let document = Document::new("one\n");
    /// assert!(document.write(vec![(4..4, "two\n".to_owned())]));
    /// assert_eq!(document.text(), "one\ntwo\n");
    /// ```
    pub fn write(&self, replacements: Vec<(Range<usize>, String)>) -> bool {
        let change = {
            let mut state = self.inner.state.borrow_mut();
            let mut selections = Selections::caret(0);
            EditorState {
                doc: &mut state,
                selections: &mut selections,
            }
            .write(replacements)
            .change
        };
        self.spread(change)
    }

    /// Tells the views about `change` and reports it. Answers whether there was one.
    fn spread(&self, change: Option<ChangeInfo>) -> bool {
        // Told outside the borrow: a view hearing about a change reads the document.
        match change {
            Some(change) => {
                self.tell(&change, None);
                self.report(&change);
                true
            }
            None => false,
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
    pub(crate) fn broadcast(&self, change: &ChangeInfo, actor: ViewId) {
        self.tell(change, Some(actor));
    }

    /// Reports `change` to the application, through the first view that is listening.
    ///
    /// One report for one change: an edit made by a view is reported by that view, and this is the
    /// same rule for an edit made by nobody. A document with no view has nothing to report through
    /// and nothing showing it either.
    fn report(&self, change: &ChangeInfo) {
        let revision = self.revision();
        let listeners: Vec<Rc<EditorCtx>> = {
            let views = self.inner.views.borrow();
            views
                .iter()
                .filter_map(|(_, weak)| weak.upgrade())
                .collect()
        };
        for ctx in listeners {
            let report = ctx.on_event.borrow();
            if let Some(tell) = report.as_ref() {
                tell(crate::EditorEvent::Edited {
                    kind: change.kind,
                    revision,
                    changes: std::sync::Arc::clone(&change.changes),
                });
                return;
            }
        }
    }

    /// Tells every view except `actor`, or every view at all when nothing acted.
    ///
    /// The list is copied out before anything is told, because telling a view makes it read the
    /// document — and a borrow held across that is a panic rather than a bug that can be found by
    /// reading the two functions side by side.
    fn tell(&self, change: &ChangeInfo, actor: Option<ViewId>) {
        let others: Vec<Rc<EditorCtx>> = {
            let views = self.inner.views.borrow();
            views
                .iter()
                .filter(|(id, _)| Some(*id) != actor)
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
    fn an_applied_change_moves_the_revision() {
        let document = Document::new("one\ntwo\n");
        let before = document.revision();
        assert!(document.apply(vec![(4..7, "TWO".to_owned())]));
        assert_eq!(document.text(), "one\nTWO\n");
        assert_ne!(document.revision(), before);
    }

    #[test]
    fn several_replacements_apply_as_one() {
        // Each range addresses the text as it is now, so two edits in one call must not shift
        // one another.
        let document = Document::new("a,b,c\n");
        assert!(document.apply(vec![(0..1, "xx".to_owned()), (4..5, "yy".to_owned())]));
        assert_eq!(document.text(), "xx,b,yy\n");
    }

    #[test]
    fn an_applied_change_undoes() {
        // The whole reason it goes through the shared history: `u` in a text view of the same
        // buffer takes back what a table or a drawing did.
        let document = Document::new("one\n");
        document.apply(vec![(0..3, "two".to_owned())]);
        assert_eq!(document.text(), "two\n");

        let mut selections = crate::core::selection::Selections::caret(0);
        let mut state = document.inner.state.borrow_mut();
        crate::core::EditorState {
            doc: &mut state,
            selections: &mut selections,
        }
        .apply(
            &crate::command::Command::Undo,
            crate::core::motion::MotionContext { viewport_lines: 1 },
        );
        assert_eq!(state.buffer.to_string(), "one\n");
    }

    #[test]
    fn a_change_that_changes_nothing_says_so() {
        let document = Document::new("text");
        assert!(!document.apply(Vec::new()));
        assert!(!document.apply(vec![(2..2, String::new())]));
        assert_eq!(document.revision(), Document::new("text").revision());
    }

    #[test]
    fn a_read_only_document_refuses() {
        let document = Document::new("text");
        document.inner.state.borrow_mut().options.read_only = true;
        assert!(!document.apply(vec![(0..4, "other".to_owned())]));
        assert_eq!(document.text(), "text");
    }

    #[test]
    fn a_write_reaches_a_read_only_document_and_leaves_no_history() {
        let document = Document::new("one\n");
        document.apply(vec![(0..3, "two".to_owned())]);
        document.inner.state.borrow_mut().options.read_only = true;
        assert!(document.write(vec![(4..4, "three\n".to_owned())]));
        assert_eq!(document.text(), "two\nthree\n");
        document.with_history(|history| assert_eq!(history.undo_depth(), 0));
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
