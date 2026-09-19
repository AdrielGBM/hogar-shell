[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::config::theme::{FontRole, NordTheme};
use ::services::gpu::{self, Gpu};
use ::ui::glyph;

// The same load ramp the CPU chip uses, so two readouts side by side mean the same thing at the same colour.
fn load_color(percent: Option<f32>, fg: Color) -> Color {
    let t = use_theme::<NordTheme>();
    match percent {
        Some(p) if p >= 90.0 => t.red,
        Some(p) if p >= 70.0 => t.yellow,
        _ => fg,
    }
}

// Intel publishes no utilisation counter and NVIDIA needs its own tool; a card that cannot answer draws a dash rather than a 0% that reads as an idle GPU.
fn load_text(percent: Option<f32>) -> String {
    match percent {
        Some(p) => format!("{p:.0}%"),
        None => telar::t!("sysinfo.no_reading"),
    }
}

let host = ui::host::Host::current()?;
let initial = gpu::current().unwrap_or_default();
let load = signal(initial.usage);
let load_text_source = load.read_only();
let load_tint = load.read_only();

platform_wayland::watch(gpu::subscribe, move |g: Gpu| load.set(g.usage));

let fg = host.foreground;
let reading = memo(move || load_text(load_text_source.get()));

[view]
row align:center gap:(::ui::scale::space::md())
    icon_glyph name:(Reactive::of(|| glyph::gpu().to_string())) tint:(Reactive::of(move || load_color(load_tint.get(), fg))) size:(host.icon_size())
    text "{$reading}" font_size:$theme.font(FontRole::Body) color:fg

[preview "Gpu" fixture:ui::preview::bar_chip]
gpu
