[logic]
use crate::media::{glyph, label, marquee, marquee_ticks, overflows};
use ::config::theme::{FontRole, NordTheme};
use ::services::mpris::{self, Player};

let config = ui::module::surface_env()
    .map(|e| e.config.media.clone())
    .unwrap_or_default();
let for_frame = config.clone();

let initial = mpris::current().unwrap_or_default();
let player = signal(initial.clone());
let icon_name = signal(glyph(&initial).to_string());
let icon_view = icon_name.read_only();
// A vertical bar has no room for a track title, so it shows only the transport glyph; the same module works on every edge instead of needing a second one.
let vertical = ui::module::bar_is_vertical();

// A read handle taken before the watch closure moves the signal in: a signal is not `Copy`.
let text_player = player.read_only();
platform_wayland::watch(mpris::subscribe, move |p: Player| {
    icon_name.set(glyph(&p).to_string());
    player.set(p);
});

// The marquee's step. Only subscribed when the user asked for one, so a bar without it runs no ticker at all; the step still only *moves* the text while a title actually overflows.
let frame = signal(0u64);
if config.marquee && !vertical {
    platform_wayland::watch(marquee_ticks, move |tick: u64| frame.set(tick));
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

let fg = ui::module::module_fg();
let fg_icon = fg.clone();
let show_text = memo(move || !vertical && !text_empty.get().is_empty());

[view]
row align:center gap(::ui::scale::space::md())
    icon_glyph name(move || icon_view.get()) tint(move || fg_icon.get()) size:(ui::module::icon_px())
    if $show_text
        text "{$text_view}" size:theme.font(FontRole::Body) color:$fg lines:1 ellipsis:true

[preview "Media" fixture:ui::preview::bar_chip]
media
