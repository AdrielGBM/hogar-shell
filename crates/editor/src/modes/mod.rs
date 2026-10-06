//! The per-layer tools of the edit modes (TA-5, A-7): what each mode adds over its layer, to the popovers of the areas it edits, to their context menus and to the key table — all registered by [`install`], from [`crate::install`].

pub(crate) mod background;
pub(crate) mod bars;
pub(crate) mod container;
pub(crate) mod desktop;
pub(crate) mod gesture;
pub(crate) mod grid;
pub(crate) mod lock;
pub(crate) mod overlay;
pub(crate) mod palette;
pub(crate) mod rect_handles;
pub(crate) mod regions;
pub(crate) mod texture;
pub(crate) mod top;
pub(crate) mod widgets;

pub use desktop::{Adding, added};
pub use palette::offered;

pub(crate) fn install() {
    rect_handles::install_bodies();
    background::install();
    desktop::install();
    top::install();
    overlay::install();
    lock::install();
    rect_handles::install();
}
