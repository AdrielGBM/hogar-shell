[logic]
use ::config::theme::{FontRole, NordTheme};
use ::services::resources::{self, Resources};

fn pressure_color(percent: f32, fg: Color) -> Color {
    let t = use_theme::<NordTheme>();
    if percent >= 90.0 {
        t.red
    } else if percent >= 75.0 {
        t.yellow
    } else {
        fg
    }
}

let initial = resources::current().unwrap_or_default();
let used = signal(initial.memory.used_percent());
let used_text = used.read_only();
let used_tint = used.read_only();

platform_wayland::watch(resources::subscribe, move |r: Resources| {
    used.set(r.memory.used_percent())
});

let fg = ui::module::module_fg();
let fg_tint = fg.clone();
let percent = memo(move || format!("{:.0}%", used_text.get()));

[view]
row align:center gap(::ui::scale::space::md())
    icon_glyph name(|| "memory-stick".to_string()) tint(move || pressure_color(used_tint.get(), fg_tint.get())) size:(ui::module::icon_px())
    text "{$percent}" size:theme.font(FontRole::Body) color:$fg

[preview "Memory" fixture:ui::preview::bar_chip]
memory
