//! Layout styles, and the input claim, more than one surface or module reaches for.

use telar::{AlignItems, Color, JustifyContent, LayoutStyle, SizeDimension, StyledContainer};

use config::Align;

/// Declares `chrome` as a box the pointer stops at — but only where it paints something to stop at.
///
/// A window hands the compositor the union of its input-opaque rects, so the region follows what is drawn rather than where a handler happens to sit: a bar's background, the gap between two chips and a card's body all belong to the shell, handler or no handler. `fill` is what the box paints, and a transparent one claims nothing: a rect nobody can see that still catches the press is the bug a faded KLWP layer is famous for.
pub fn painted_chrome(chrome: StyledContainer, fill: Color) -> StyledContainer {
    if fill.a > 0.0 {
        chrome.input_opaque()
    } else {
        chrome
    }
}

/// The whole of the parent's box, so boxes given it stack over each other.
pub fn fill() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

pub fn align_items(align: Align) -> AlignItems {
    match align {
        Align::Start => AlignItems::FLEX_START,
        Align::Center => AlignItems::CENTER,
        Align::End => AlignItems::FLEX_END,
    }
}

pub fn justify(align: Align) -> JustifyContent {
    match align {
        Align::Start => JustifyContent::FLEX_START,
        Align::Center => JustifyContent::CENTER,
        Align::End => JustifyContent::FLEX_END,
    }
}
