[logic]
use ::services::brightness;

// A dimmer sun below 40%, a full sun above — so the single glyph still reads the level at a glance.
fn bright_glyph(level: i32) -> &'static str {
    if level < 40 { "sun-dim" } else { "sun" }
}

let level = signal(brightness::read().unwrap_or(0));
let level_glyph = level.read_only();
let fg = ui::module::module_fg();
// The chip shows one number, so it follows the snapshot's primary display — the internal panel on a laptop, the
// first monitor on a desk. Per-output levels are reached through `hogar-shell brightness` and the settings page.
platform_wayland::watch(
    brightness::subscribe,
    move |snapshot: brightness::Snapshot| {
        if let Some(level_now) = snapshot.level() {
            level.set(level_now);
        }
    },
);

let glyph = memo(move || bright_glyph(level_glyph.get()));

[view]
icon_glyph name(move || glyph.get().to_string()) tint(move || fg.get()) size:(ui::module::icon_px())

[preview "Brightness" fixture:ui::preview::bar_chip]
brightness
