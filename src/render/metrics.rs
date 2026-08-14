//! How tall a line is, how wide a cell is, and where the baseline sits.
//!
//! Every length is in device pixels and the line height is a whole number of them, because a
//! line that lands between pixels drifts against its neighbours and blurs its glyphs. The
//! generation moves whenever the face, the size or the scale changes, and is what every cache
//! of shaped text is keyed by.

use zgui::app::{LineRequest, ResolvedLineMetrics};
use zgui_interned::Ident;

/// The measurements text is laid out by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// How tall one line is, in whole device pixels.
    pub line_height: f32,
    /// How far below a line's top its baseline sits.
    pub baseline: f32,
    /// The face's ascent, which is where the shaper's own line box puts the baseline.
    ///
    /// Glyph positions come out of the shaper relative to its own line box; painting shifts
    /// them down by `baseline - ascent` so the baseline lands where this line height says.
    pub ascent: f32,
    /// The advance of a digit zero — the column unit for gutters, tabs and estimates.
    pub cell_advance: f32,
    /// The size glyphs are shaped at, in device pixels.
    pub font_size: f32,
    /// The CSS weight text is shaped at.
    pub weight: u16,
    /// Extra advance after every glyph, in device pixels.
    pub letter_spacing: f32,
    /// Whether ligatures are shaped. Code editors usually want them.
    pub ligatures: bool,
    /// How many columns a tab advances to a multiple of.
    pub tab_width: u32,
    /// The scale these were measured at.
    pub scale: f32,
    /// Which set of measurements these are, so caches know when to let go.
    pub generation: u32,
}

impl Default for TextMetrics {
    fn default() -> Self {
        Self {
            line_height: 20.0,
            baseline: 15.0,
            ascent: 15.0,
            cell_advance: 8.0,
            font_size: 14.0,
            weight: 400,
            letter_spacing: 0.0,
            ligatures: true,
            tab_width: 4,
            scale: 1.0,
            generation: 0,
        }
    }
}

impl TextMetrics {
    /// Measurements for a face measured by `resolved` at `size_device_px`.
    ///
    /// `line_height_px` is what the style asked for, when it asked; the face's own natural
    /// height carries otherwise.
    #[allow(clippy::too_many_arguments)]
    pub fn from_face(
        resolved: &ResolvedLineMetrics,
        size_device_px: f32,
        line_height_px: Option<f32>,
        letter_spacing: f32,
        weight: u16,
        ligatures: bool,
        tab_width: u32,
        scale: f32,
        generation: u32,
    ) -> Self {
        let face = &resolved.metrics;
        let ascent = face.ascent.0;
        let descent = face.descent.0;
        let natural = ascent + descent + face.line_gap.0;
        let line_height = line_height_px.unwrap_or(natural).round().max(1.0);
        // The face's own content sits in the middle of whatever the line height added.
        let leading = (line_height - (ascent + descent)) / 2.0;
        let baseline = (leading + ascent).round().clamp(1.0, line_height);
        Self {
            line_height,
            baseline,
            ascent,
            cell_advance: resolved.cell_advance.0 + letter_spacing,
            font_size: size_device_px,
            weight,
            letter_spacing,
            ligatures,
            tab_width: tab_width.max(1),
            scale,
            generation,
        }
    }

    /// The request that shapes text at these measurements.
    pub fn line_request<'a>(
        &self,
        families: &'a [Ident],
        italic: bool,
        bold: bool,
    ) -> LineRequest<'a> {
        LineRequest {
            families,
            weight: if bold {
                (self.weight + 300).min(1000)
            } else {
                self.weight
            },
            italic,
            size_device_px: self.font_size,
            letter_spacing: self.letter_spacing,
            ligatures: self.ligatures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_height_is_whole_pixels() {
        let metrics = TextMetrics::default();
        assert_eq!(metrics.line_height.fract(), 0.0);
    }
}
