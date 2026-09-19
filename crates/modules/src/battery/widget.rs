//! The battery as a widget: the hover card when small, the dashboard's card with its time left when wider.

use telar::{LayoutError, LayoutItem};

use ui::card::Density;
use ui::host::Host;

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let card = if host.is_small() {
        crate::popout_cards::battery(host)
    } else {
        crate::dashboard::cards::battery(host)
    };
    card.build(Density::Widget)
}
