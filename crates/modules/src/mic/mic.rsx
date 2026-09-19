[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::services::volume::{self, Volume};
use ::ui::glyph;

// `None` until the graph has answered, never a stand-in reading: the chip used to seed itself `muted: true`, so a live microphone drew mic-off for as long as the PipeWire listener took to publish its first batch. Of the two ways to be wrong, claiming muted is the one that gets someone to speak freely into a live mic.
let host = ui::host::Host::current()?;
let state = signal(volume::current_mic());
let read = state.read_only();
let fg = host.foreground;
let icon = host.icon_size();

platform_wayland::watch(volume::subscribe_mic, move |mic: Volume| state.set(Some(mic)));

[view]
icon_glyph name:(Reactive::of(move || read.get().map_or("mic", glyph::microphone).to_string())) tint:(Reactive::of(move || fg)) size:(icon)

[preview "Mic" fixture:ui::preview::bar_chip]
mic
