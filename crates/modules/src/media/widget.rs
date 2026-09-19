//! What is playing, as a widget: the hover card when small, the cover beside the track when wider. Readings only — the transport lives on the dashboard's card, which is why this is not that card.

use telar::{LayoutError, LayoutItem};

use ui::card::Density;
use ui::host::Host;

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let card = if host.is_small() {
        crate::popout_cards::media(host)
    } else {
        crate::dashboard::cards::track(host)
    };
    card.build(Density::Widget)
}
