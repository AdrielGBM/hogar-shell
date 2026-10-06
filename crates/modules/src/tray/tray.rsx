[logic]
use crate::tray::visible;
use crate::tray::{TrayIconProps, tray_icon};
use ::config::theme::NordTheme;
use ::services::tray::{self, TrayItem};

let host = ui::host::Host::current()?;
let config = host.options::<::config::TrayConfig>();
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
let compact = config.compact;
let sized = host.live_icon_size();
let gap = memo({
    let sized = sized.clone();
    move || if compact { 0.0 } else { (sized.get() * 0.15).round() }
});
// The size is in each entry's key, so an icon whose chip is laid out at another size is drawn again at it.
let entries = memo(move || {
    let size = sized.get().to_bits();
    listed.get().into_iter().map(|item| (item, size)).collect::<Vec<_>>()
});

[view]
row align:center gap:$gap
    for entry in $entries key (entry.0.key.clone(), entry.1)
        tray_icon item:entry.0.clone() config:config.clone() host:host.clone() theme:theme

[preview "Tray" fixture:crate::preview::tray]
tray
