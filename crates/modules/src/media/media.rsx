[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use crate::media::{glyph, label, marquee, marquee_ticks, overflows};
use ::config::theme::{FontRole, NordTheme};
use ::services::mpris::{self, Player};

let host = ui::host::Host::current()?;
let config = host.options::<::config::MediaConfig>().clone();
let for_frame = config.clone();

let initial = mpris::current().unwrap_or_default();
let player = signal(initial.clone());
let icon_name = signal(glyph(&initial).to_string());
let icon_view = icon_name.read_only();
// A vertical bar has no room for a track title, so it shows only the transport glyph; the same module works on every edge instead of needing a second one.
let vertical = host.is_vertical();

// A read handle taken before the watch closure moves the signal in: a signal is not `Copy`.
let text_player = player.read_only();
platform_wayland::watch(mpris::subscribe, move |p: Player| {
    icon_name.set(glyph(&p).to_string());
    player.set(p);
});

// The marquee's step. Only subscribed when the user asked for one, so a bar without it runs no ticker at all; the step still only *moves* the text while a title actually overflows.
let frame = signal(0u64);
if config.marquee && !vertical {
    let step = config.marquee_step();
    platform_wayland::watch(move |tx| marquee_ticks(tx, step), move |tick: u64| frame.set(tick));
}

let frame_read = frame.read_only();
let text_view = memo(move || {
    let p = text_player.get();
    if for_frame.marquee && overflows(&p, &for_frame) {
        marquee(&p, &for_frame, frame_read.get() as usize)
    } else {
        label(&p)
    }
});
let text_empty = text_view.clone();

let fg = host.foreground;
let show_text = memo(move || !vertical && !text_empty.get().is_empty());

[view]
row align:center gap:(::ui::scale::space::md())
    icon_glyph name:(Reactive::of(move || icon_view.get())) tint:(Reactive::of(move || fg)) size:(host.icon_size())
    if $show_text
        text "{$text_view}" font_size:$theme.font(FontRole::Body) color:fg lines:1 ellipsis

[preview "Media" fixture:ui::preview::bar_chip]
media
