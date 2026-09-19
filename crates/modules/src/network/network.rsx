[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::services::network::{self, Network};
use ::ui::glyph;

let host = ui::host::Host::current()?;
let state = signal(network::read());
let read = state.read_only();
let fg = host.foreground;
platform_wayland::watch(network::subscribe, move |net: Network| state.set(net));

[view]
icon_glyph name:(Reactive::of(move || glyph::network(read.get()).to_string())) tint:(Reactive::of(move || fg)) size:(host.icon_size())

[preview "Network" fixture:ui::preview::bar_chip]
network
