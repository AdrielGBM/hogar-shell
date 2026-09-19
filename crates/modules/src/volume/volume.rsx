[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::services::volume;
use ::ui::glyph;

// The container wires the click that toggles mute and pops the OSD (where the exact level lives).
let host = ui::host::Host::current()?;
let state = signal(volume::current().unwrap_or(volume::Volume {
    level: 0,
    muted: false,
}));
let read = state.read_only();
let fg = host.foreground;
platform_wayland::watch(volume::subscribe, move |v: volume::Volume| state.set(v));

[view]
icon_glyph name:(Reactive::of(move || glyph::volume(read.get()).to_string())) tint:(Reactive::of(move || fg)) size:(host.icon_size())

[preview "Volume" fixture:ui::preview::bar_chip]
volume
