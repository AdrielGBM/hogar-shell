[logic]
use ::config::theme::{FontRole, NordTheme};
use ::services::hyprland::{self, KeyboardLayout};

// Hyprland names layouts in full ("English (US)"); a bar has room for the two-letter code people actually scan
// for, so this takes the initials of the leading words — "English (US)" → "EN", "Spanish" → "ES".
fn short_name(layout: &KeyboardLayout) -> String {
    let base = layout.name.split('(').next().unwrap_or(&layout.name).trim();
    let code: String = base
        .split_whitespace()
        .next()
        .unwrap_or(base)
        .chars()
        .filter(|c| c.is_alphabetic())
        .take(2)
        .collect();
    // An empty code would render an empty string, and a chip with no glyphs collapses to no area at all: the
    // module does not read as absent, it disappears while still occupying a slot.
    if code.is_empty() {
        return "--".to_string();
    }
    code.to_uppercase()
}

let initial = hyprland::socket_dir()
    .and_then(|dir| hyprland::keyboard_layout(&dir))
    .unwrap_or_default();

let code = signal(short_name(&initial));
let code_view = code.read_only();

platform_wayland::watch(
    hyprland::subscribe_keyboard,
    move |layout: KeyboardLayout| {
        code.set(short_name(&layout));
    },
);

let fg = ui::module::module_fg();

[view]
text "{$code_view}" size:theme.font(FontRole::Body) color:$fg

[preview "Kblayout" fixture:ui::preview::bar_chip]
kblayout
