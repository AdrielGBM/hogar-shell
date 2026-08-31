[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::config::theme::{FontRole, NordTheme};
use ::services::resources::{self, Resources};

// The chip tints as load climbs, so a busy machine is visible without reading the number.
fn load_color(percent: f32, fg: Color) -> Color {
    let t = use_theme::<NordTheme>();
    if percent >= 90.0 {
        t.red
    } else if percent >= 70.0 {
        t.yellow
    } else {
        fg
    }
}

let initial = resources::current().unwrap_or_default();
let load = signal(initial.cpu);
let load_text = load.read_only();
let load_tint = load.read_only();

platform_wayland::watch(resources::subscribe, move |r: Resources| load.set(r.cpu));

let fg = ui::module::module_fg();
let fg_tint = fg.clone();
let percent = memo(move || format!("{:.0}%", load_text.get()));

[view]
row align:center gap:(::ui::scale::space::md())
    icon_glyph name:(Reactive::of(|| "cpu".to_string())) tint:(Reactive::of(move || load_color(load_tint.get(), fg_tint.get()))) size:(ui::module::icon_px())
    text "{$percent}" font_size:$theme.font(FontRole::Body) color:$fg

[preview "Cpu" fixture:ui::preview::bar_chip]
cpu
