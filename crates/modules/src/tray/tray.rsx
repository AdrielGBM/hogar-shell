[logic]
use crate::tray::{tray_icon, visible};
use ::config::theme::NordTheme;
use ::services::tray::{self, TrayItem};

let config = ui::module::surface_env()
    .map(|env| env.config.tray.clone())
    .unwrap_or_default();
let filter_config = config.clone();

let items = signal(visible(&tray::current().unwrap_or_default(), &config));
let listed = items.read_only();

// Disabled costs nothing: without the subscription the service never starts, so no watcher name is claimed and no thread runs.
if config.enabled {
    platform_wayland::watch(tray::subscribe, move |all: Vec<TrayItem>| {
        items.set(visible(&all, &filter_config))
    });
}

let fg = ui::module::module_fg();
let theme = use_theme::<NordTheme>();
let size = ui::module::icon_px();
let radius = ui::module::chip_radius();
let gap = if config.compact {
    0.0
} else {
    (size * 0.15).round()
};

[view]
row align:center gap:gap
    for item in $listed key item.key.clone()
        build "tray_icon(item, config.clone(), fg.clone(), theme, size, radius)?"

[preview "Tray" fixture:crate::preview::tray]
tray
