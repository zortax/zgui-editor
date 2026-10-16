//! The custom element the editor is drawn by.
//!
//! One element, sized by the [frame](crate::render::frame) to the frame's padding box, painting
//! only the visible lines. The framework replays an unchanged element's primitives without
//! calling `paint`, so every change anywhere in [`EditorShared`] is followed by a `repaint()` on
//! the handle — the discipline the whole render layer is built around.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::custom::{CustomElement, CustomLayoutCx, CustomMeasured, ScenePainter, Space};

use crate::render::shared::EditorShared;

/// The element. All state lives in the shared cell; this is the frame's way in.
pub struct EditorElement {
    shared: Rc<RefCell<EditorShared>>,
}

impl EditorElement {
    /// An element over `shared`.
    pub fn new(shared: Rc<RefCell<EditorShared>>) -> Self {
        Self { shared }
    }
}

impl CustomElement for EditorElement {
    fn layout(&mut self, cx: &mut CustomLayoutCx<'_>) -> CustomMeasured {
        // The frame gives the final size. The fallbacks answer a probe that gives none.
        let width = cx.known_width.unwrap_or(match cx.available.0 {
            Space::Definite(width) => width,
            _ => 640.0 * cx.scale,
        });
        let height = cx.known_height.unwrap_or(match cx.available.1 {
            Space::Definite(height) => height,
            _ => 400.0 * cx.scale,
        });

        // The style is read first because the line height comes out of it, and a windowed view
        // with no height given is as tall as its lines rather than as tall as what it was
        // offered. The second read is what gives the view the height it is about to be given.
        let height = {
            let mut shared = self.shared.borrow_mut();
            shared.read_style(cx.style, cx.scale, (width, height), false);
            match (cx.known_height, shared.line_window()) {
                (None, Some(window)) => window.len() as f32 * shared.metrics.line_height,
                _ => height,
            }
        };
        self.shared
            .borrow_mut()
            .read_style(cx.style, cx.scale, (width, height), cx.final_pass);
        CustomMeasured {
            width,
            height,
            ..CustomMeasured::default()
        }
    }

    fn paint(&mut self, painter: &mut ScenePainter<'_>) {
        self.shared.borrow_mut().paint(painter);
    }
}
