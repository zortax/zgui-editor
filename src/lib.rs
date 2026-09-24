//! An embeddable, high-performance code editor component for Zgui applications.
//!
//! One [`Editor`] component renders any buffer — up to millions of lines — through a single
//! retained element that paints only what is visible, shapes each line once, and scrolls to
//! pixel precision with its own `f64` scroll model. Syntax highlighting is tree-sitter,
//! incremental, computed on a background worker and delivered per visible line.
//!
//! # Driving it
//!
//! Everything is a [`Command`], applied through the [`EditorHandle`] the component hands to
//! `on_ready` (and provides as local context). The handle also answers synchronous reads
//! through [`EditorHandle::query`] and publishes signals for status lines and scrollbars.
//! Key handling is layered: an application's `on_key` filter hears every key first — which is
//! how a vim mode is built *outside* this crate (see `examples/vim`) — and whatever it declines
//! falls through to the default keymap.
//!
//! A change with no editor in front of it goes through [`Document::apply`]. It records in the
//! same history and reaches the same views, so a buffer edited by something that is not an
//! editor is still undone by one.
//!
//! # Theming
//!
//! Colours are CSS custom properties, inherited like any other: `--editor-bg` is the element's
//! own `background`, the text is its `color`, and the rest are `--editor-selection`,
//! `--editor-cursor`, `--editor-cursor-text`, `--editor-current-line`, `--editor-gutter-bg`,
//! `--editor-gutter-fg`, `--editor-gutter-current-fg`, `--editor-scrollbar-thumb`,
//! `--editor-scrollbar-track`, and one `--syntax-<capture>` per highlight capture name with
//! dots written as hyphens (`--syntax-function-method`), falling back along the dots and then
//! to the foreground. Fonts, `tab-size` and `cursor` are ordinary CSS on the element.
//!
//! # Marking the text
//!
//! An application draws on the buffer through [`decoration`]: named layers of bands, underlines
//! and gutter marks, each naming a custom property of its own so a diagnostic or a search hit is
//! themed where everything else is. [`EditorHandle::point_for_byte`] turns a byte into a place on
//! the window, which is what a hover card, an inline diagnostic or a leap label is positioned by.
//!
//! Where the carets sit and which cells read as selected can be taken over as well, through
//! [`overlay`]. That is what a modal layer's visual modes are drawn with: they select through the
//! character a caret is on, take whole lines, and reach past the end of a short line, none of
//! which a byte range says.
//!
//! [`styles`] puts colours, weights, slants and lines on stretches of the text over the
//! highlighter, which is what terminal output is shown with.
//!
//! # A view of one part of a document
//!
//! [`EditorConfig::line_window`] makes the view draw exactly the lines it names and size itself to
//! them, with no vertical scrolling. The gutter still numbers the real lines and the history is
//! still the document's own, so a rendered document can put a real editor over one block and a
//! preview can show a hit in its own place. [`EditorHandle::set_line_window`] moves the window
//! without unmounting the view, which is what keeps the carets and the parsed tree.
//!
//! # What is not there yet
//!
//! Input methods do not compose over the editor — see [`ime`] for the platform limitation and
//! the upstream path. Lines do not soft-wrap; long lines scroll horizontally.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod command;
pub mod config;
pub mod core;
pub mod decoration;
pub mod document;
pub mod event;
pub mod handle;
pub mod ime;
pub mod input;
pub mod overlay;
pub mod render;
pub mod scroll;
pub mod styles;
pub mod syntax;
mod view;

pub use crate::command::{Clipboard, Command, InsertPoint, Motion, ScrollCmd};
pub use crate::config::{CursorStyle, EditorConfig, GutterMode};
pub use crate::core::edit::{Edit, EditKind, EditTime, TextChange, Transaction};
pub use crate::core::history::{History, Step};
pub use crate::core::search::SearchDirection;
pub use crate::core::selection::{Selection, Selections};
pub use crate::core::{DocumentState, EditOptions, EditorState};
pub use crate::decoration::{
    Decoration, DecorationKind, GutterLabel, GutterMark, GutterSource, Mark, Paint, UnderlineStyle,
};
pub use crate::document::Document;
pub use crate::event::{EditorEvent, KeyFilter};
pub use crate::handle::{CaretRect, CursorPos, EditorHandle, EditorSnapshot, ScrollSnapshot};
pub use crate::overlay::{Band, Caret, Overlay};
pub use crate::styles::{StyleSpan, StyleSpans, TextStyle};
pub use crate::syntax::oneshot::{Highlighted, highlight};
pub use crate::syntax::registry::{LanguageConfig, LanguageRegistry};
pub use crate::view::{Editor, EditorProps};
