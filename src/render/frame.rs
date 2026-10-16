//! The element around the drawing element: the box CSS styles and sizes.
//!
//! The frame is the component's root. It reads its own padding and border, gives its padding box
//! to the [`EditorElement`](crate::render::element::EditorElement) inside it, and stores the
//! padding in [`EditorShared`]. The editor then draws the way a CSS scroll container does: the
//! scrollbars at the padding box's edges, the text inset by the padding, and scrolled text
//! moving through the padding up to the frame's clip.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::custom::{CustomElement, CustomLayoutCx, CustomMeasured, ScenePainter, Space};
use zgui_css::ComputedStyle;
use zgui_css::values::border::{BorderSideWidthValue, BorderStyleValue};
use zgui_css::values::size::PaddingValue;

use crate::render::shared::{EditorShared, Insets};

/// The frame. It paints nothing of its own; CSS paints its background and border.
pub struct EditorFrame {
    shared: Rc<RefCell<EditorShared>>,
}

impl EditorFrame {
    /// A frame over `shared`.
    pub fn new(shared: Rc<RefCell<EditorShared>>) -> Self {
        Self { shared }
    }
}

impl CustomElement for EditorFrame {
    fn layout(&mut self, cx: &mut CustomLayoutCx<'_>) -> CustomMeasured {
        // The editor fills what it is given; a caller that wants a size says so in CSS.
        let width = cx.known_width.unwrap_or(match cx.available.0 {
            Space::Definite(width) => width,
            _ => 640.0 * cx.scale,
        });
        let padding = padding_of(cx.style, cx.scale, width);
        let border = border_of(cx.style, cx.scale);

        // The borrow ends here: the child's layout borrows the same cell.
        let windowed = {
            let mut shared = self.shared.borrow_mut();
            if cx.final_pass {
                shared.set_insets(padding);
            }
            shared.line_window().is_some()
        };

        let padded_width = width + padding.left + padding.right;
        let height = match cx.known_height {
            Some(height) => height,
            // A windowed view is as tall as its lines, which only the drawing element knows.
            None if windowed && cx.access.child_count() > 0 => {
                cx.access
                    .measure_child(
                        0,
                        (Some(padded_width), None),
                        (Space::Definite(padded_width), Space::MaxContent),
                    )
                    .height
            }
            None => match cx.available.1 {
                Space::Definite(height) => height,
                _ => 400.0 * cx.scale,
            },
        };

        if cx.final_pass && cx.access.child_count() > 0 {
            let padded_height = height + padding.top + padding.bottom;
            cx.access.layout_child(
                0,
                (Some(padded_width), Some(padded_height)),
                (
                    Space::Definite(padded_width),
                    Space::Definite(padded_height),
                ),
            );
            cx.access.place_child(0, border.left, border.top);
        }
        CustomMeasured {
            width,
            height,
            ..CustomMeasured::default()
        }
    }

    fn paint(&mut self, _painter: &mut ScenePainter<'_>) {}
}

/// The padding of `style`, in whole device pixels.
///
/// A percentage resolves against `basis`, the frame's content width. CSS resolves it against the
/// containing block's width, so a percentage is approximate.
fn padding_of(style: &ComputedStyle, scale: f32, basis: f32) -> Insets {
    let padding = style.get_padding();
    let side = |value: &PaddingValue| {
        let css = zgui_css::values::length::evaluate_at(
            &value.0,
            zgui::geom::CssPx(basis / scale.max(0.001)),
        );
        (css.0 * scale).round().max(0.0)
    };
    Insets {
        top: side(&padding.padding_top),
        right: side(&padding.padding_right),
        bottom: side(&padding.padding_bottom),
        left: side(&padding.padding_left),
    }
}

/// The border widths of `style`, in whole device pixels. A side with no style has no width.
fn border_of(style: &ComputedStyle, scale: f32) -> Insets {
    let border = style.get_border();
    let side = |width: &BorderSideWidthValue, style: BorderStyleValue| {
        if style.none_or_hidden() {
            0.0
        } else {
            (width.0.to_f32_px() * scale).round()
        }
    };
    Insets {
        top: side(&border.border_top_width, border.border_top_style),
        right: side(&border.border_right_width, border.border_right_style),
        bottom: side(&border.border_bottom_width, border.border_bottom_style),
        left: side(&border.border_left_width, border.border_left_style),
    }
}
