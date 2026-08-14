//! Deciding which faces the text is drawn in.
//!
//! The style sheet names families; each has to be one this machine really has. The generic
//! names — `monospace` and the rest — name no face at all, so a list holding only those would
//! measure nothing and draw nothing. What goes to the shaper is therefore the sheet's list with
//! commonly installed code faces added after it: the first that exists wins, and the ones after
//! it still answer for characters the first cannot draw.

use zgui::app::{LineRequest, Shaper};
use zgui_interned::Ident;
use zgui_text_style::FamilyName;

/// The families tried after the styled ones, ordered by what code is usually set in.
const FALLBACKS: &[&str] = &[
    "JetBrains Mono",
    "JetBrainsMono Nerd Font",
    "Fira Code",
    "FiraCode Nerd Font",
    "Cascadia Code",
    "Cascadia Mono",
    "Source Code Pro",
    "Hack",
    "Hack Nerd Font",
    "Iosevka",
    "Menlo",
    "Consolas",
    "SF Mono",
    "Ubuntu Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Courier New",
];

/// The families to draw with, given what the style sheet asked for.
///
/// An empty answer means nothing resolved at all, and the caller draws with fallback
/// measurements.
pub fn resolve(shaper: &Shaper, styled: &[FamilyName], size_device_px: f32) -> Vec<Ident> {
    let mut families: Vec<Ident> = Vec::with_capacity(styled.len() + FALLBACKS.len());
    for name in styled {
        if let FamilyName::Named(ident) = name
            && !families.contains(ident)
        {
            families.push(*ident);
        }
    }
    for name in FALLBACKS {
        let ident = Ident::new(name);
        if !families.contains(&ident) {
            families.push(ident);
        }
    }
    if resolves(shaper, &families, size_device_px) {
        return families;
    }
    // Nothing in the list as a whole; whichever one exists on its own still beats no text.
    for family in &families {
        let one = vec![*family];
        if resolves(shaper, &one, size_device_px) {
            return one;
        }
    }
    Vec::new()
}

/// Whether `families` names a face this machine has.
fn resolves(shaper: &Shaper, families: &[Ident], size_device_px: f32) -> bool {
    shaper
        .line_metrics(&plain_request(families, size_device_px))
        .is_some()
}

/// A plain request, for measuring and for asking whether a list resolves.
pub fn plain_request(families: &[Ident], size_device_px: f32) -> LineRequest<'_> {
    LineRequest {
        families,
        weight: 400,
        italic: false,
        size_device_px,
        letter_spacing: 0.0,
        ligatures: false,
    }
}
