[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::services::network::{self, Network};
use ::ui::glyph;

let state = signal(network::read());
let read = state.read_only();
let fg = ui::module::module_fg();
platform_wayland::watch(network::subscribe, move |net: Network| state.set(net));

[view]
icon_glyph name:(Reactive::of(move || glyph::network(read.get()).to_string())) tint:(Reactive::of(move || fg.get())) size:(ui::module::icon_px())

[preview "Network" fixture:ui::preview::bar_chip]
network
