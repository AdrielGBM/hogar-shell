[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use crate::osd::{OsdKind, current_osd_kind, current_osd_radius};
use ::config::theme::NordTheme;
use ::services::{brightness, volume};
use ::ui::glyph;
use ::ui::panel::panel_fill;

const TRACK_W: f32 = 172.0;
const TRACK_H: f32 = 6.0;

fn osd_tint(dimmed: bool) -> Color {
    let t = use_theme::<NordTheme>();
    if dimmed { t.muted } else { t.text }
}

// A single-shot snapshot — the OSD is transient, and every trigger (click, scroll, key) replaces it with a freshly built one. It reads the shared services' cached value rather than the system: `volume::read()` forks `wpctl`, which has no business running while a surface is being laid out.
let (glyph, frac, dimmed) = match current_osd_kind() {
    OsdKind::Volume => {
        let v = volume::current().unwrap_or(volume::Volume {
            level: 0,
            muted: false,
        });
        (
            glyph::volume(v),
            v.level.clamp(0, 100) as f32 / 100.0,
            v.muted,
        )
    }
    OsdKind::Brightness => {
        let level = brightness::osd_level().unwrap_or(0).clamp(0, 100);
        (glyph::brightness(), level as f32 / 100.0, false)
    }
    OsdKind::Microphone => {
        let v = volume::current_mic().unwrap_or(volume::Volume {
            level: 0,
            muted: true,
        });
        (
            glyph::microphone(v),
            v.level.clamp(0, 100) as f32 / 100.0,
            v.muted,
        )
    }
};
let fill_w = (frac.clamp(0.0, 1.0) * TRACK_W).max(0.0);
// A capsule: half the track's own height, so the ends stay round whatever height it is given.
let track_rad = TRACK_H / 2.0;
let rad = current_osd_radius();

[view]
// No `height:100%`: this is a card in the shell's one column now, not a surface of its own, and a card is as tall as what is in it. Filling was right while the OSD had a 280×60 surface to fill; in a column sized for a full run of cards it stretched one OSD over the lot.
box axis:row align:center justify:center gap:(::ui::scale::space::xl()) pad_x:(::ui::scale::space::xl()) pad_y:(::ui::scale::space::xl()) width:100% fill:panel_fill() radius:rad
    icon_glyph name:(Reactive::of(move || glyph.to_string())) tint:(Reactive::of(move || osd_tint(dimmed))) size:(theme.get().icon_size)
    box axis:row align:center width:TRACK_W height:TRACK_H fill:$theme.muted radius:track_rad
        box width:fill_w height:TRACK_H fill:$theme.accent radius:track_rad

[preview "Osd" fixture:crate::preview::osd]
osd
