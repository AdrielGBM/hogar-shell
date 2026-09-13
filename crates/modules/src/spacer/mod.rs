//! An empty module that takes up whatever room is left.
//!
//! A bar's three zones only give three anchor points. `spacer` buys the layout every arrangement in between — pinning one module hard left and the next just off it, or splitting a zone in two — by growing to fill the slack instead of hugging its content like every other module.

use telar::{Container, LayoutError, LayoutItem, LayoutStyle};

/// A filler, so the bar places it bare and gives it no surface: a chip shell would give it padding, a hover highlight and a press state, and a chip bar a pill behind it, none of which make sense for a gap.
pub fn spacer() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let style = LayoutStyle::new().flex_grow(1.0).flex_shrink(1.0);
    Ok(Box::new(Container::new(style, vec![])?))
}
