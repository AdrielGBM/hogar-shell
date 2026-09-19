//! Layout styles more than one surface or module reaches for.

use telar::{AlignItems, JustifyContent, LayoutStyle, SizeDimension};

use config::Align;

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
