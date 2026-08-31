[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::config::theme::NordTheme;
use ::services::battery;
use ::ui::glyph;

let init = battery::read();
let level = signal(init.map(|b| b.level).unwrap_or(0));
let charging = signal(init.map(|b| b.charging).unwrap_or(false));
let level_tint = level.read_only();
let charging_tint = charging.read_only();
let charging_glyph = charging.read_only();
let fg = ui::module::module_fg();
let theme = use_theme::<NordTheme>();
// Subscribe to the single shared battery source (UPower sub-second on plug/unplug), no-op headless.
platform_wayland::watch(battery::subscribe, move |b| {
    level.set(b.level);
    charging.set(b.charging);
});

[view]
icon_glyph name:(Reactive::of(move || glyph::battery(charging_glyph.get()).to_string())) tint:(Reactive::of(move || glyph::battery_tint(level_tint.get(), charging_tint.get(), theme, fg.get()))) size:(ui::module::icon_px())

[preview "Battery" fixture:ui::preview::bar_chip]
battery
