//! What is playing, as a widget: the hover card when small, the cover beside the track when wider. Readings only — the transport lives on the dashboard's card, which is why this is not that card.
//!
//! On a screen anyone in the room can read, what it may say is `[lock] media_detail`'s decision. Where that stops the title, the artist and the cover, the widget draws whether something is playing and nothing that names it, rather than a card with its rows blanked out — an empty card reads as a broken one.

use telar::{LayoutError, LayoutItem};

use ui::card::{Card, Density};
use ui::host::Host;
use util::reactive::derive;

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    if !host.may_show(&super::TITLE) {
        return playing_only();
    }
    let card = if host.is_small() {
        crate::popout_cards::media(host)
    } else {
        crate::dashboard::cards::track(host)
    };
    card.build(Density::Widget)
}

/// Playing or paused, and nothing that names what.
fn playing_only() -> Result<Box<dyn LayoutItem>, LayoutError> {
    use services::mpris;
    let player = telar::signal(mpris::current().unwrap_or_default());
    let sink = player;
    platform_wayland::watch(mpris::subscribe, move |next| sink.set(next));
    Card::titled(telar::t!("media.title"))
        .icon(derive(player, |player| super::glyph(&player).to_string()))
        .figure(derive(player, |player| match player.playback {
            services::mpris::Playback::Playing => telar::t!("media.playing"),
            services::mpris::Playback::Paused => telar::t!("media.paused"),
            services::mpris::Playback::Stopped => telar::t!("media.stopped"),
        }))
        .build(Density::Widget)
}
