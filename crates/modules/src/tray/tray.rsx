[logic]
use crate::tray::visible;
use crate::tray::{TrayIconProps, tray_icon};
use ::config::theme::NordTheme;
use ::services::tray::{self, TrayItem};

let host = ui::host::Host::current()?;
let config = host.options::<::config::TrayConfig>().clone();
let filter_config = config.clone();

let items = signal(visible(&tray::current().unwrap_or_default(), &config));
let listed = items.read_only();

// Disabled costs nothing: without the subscription the service never starts, so no watcher name is claimed and no thread runs.
if config.enabled {
    platform_wayland::watch(tray::subscribe, move |all: Vec<TrayItem>| {
        items.set(visible(&all, &filter_config))
    });
}

let theme = use_theme::<NordTheme>();
let gap = if config.compact {
    0.0
} else {
    (host.icon_size() * 0.15).round()
};

[view]
row align:center gap:gap
    for item in $listed key item.key.clone()
        tray_icon item:item config:config.clone() host:host.clone() theme:theme

[preview "Tray" fixture:crate::preview::tray]
tray
