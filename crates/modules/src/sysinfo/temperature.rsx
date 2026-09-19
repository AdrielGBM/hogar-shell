[logic]
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use ::config::TemperatureConfig;
use ::config::theme::{FontRole, NordTheme};
use ::services::resources::{self, Resources};

// A machine with no hwmon (a VM, some ARM boards) has nothing to show; the chip renders a dash rather than a misleading 0 °C.
fn heat_text(celsius: Option<f32>, config: &TemperatureConfig) -> String {
    match celsius {
        Some(c) => config.unit.format(c),
        None => telar::t!("sysinfo.no_reading"),
    }
}

let host = ui::host::Host::current()?;
let config = host.options::<::config::TemperatureConfig>().clone();
let text_config = config.clone();
let tint_config = config.clone();
let sensor = config.sensor.clone();

let initial = resources::current().unwrap_or_default();
let temp = signal(initial.temperature_of(&config.sensor));
let temp_text = temp.read_only();
let temp_tint = temp.read_only();

platform_wayland::watch(resources::subscribe, move |r: Resources| {
    temp.set(r.temperature_of(&sensor))
});

let fg = host.foreground;
let heat = use_theme::<NordTheme>();
let reading = memo(move || heat_text(temp_text.get(), &text_config));

[view]
row align:center gap:(::ui::scale::space::md())
    icon_glyph name:(Reactive::of(|| "thermometer".to_string())) tint:(Reactive::of(move || ::ui::glyph::heat_tint(&tint_config, temp_tint.get(), heat, fg))) size:(host.icon_size())
    text "{$reading}" font_size:$theme.font(FontRole::Body) color:fg

[preview "Temperature" fixture:ui::preview::bar_chip]
temperature
