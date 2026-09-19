[logic]
use crate::activewindow::{IconSlotProps, icon_slot};
use crate::activewindow::{compact_label, label};
use ::config::theme::{FontRole, NordTheme};
use ::services::hyprland::{self, ActiveWindow};

let host = ui::host::Host::current()?;
let config = *host.options::<::config::ActiveWindowConfig>();

fn text_for(window: &ActiveWindow, config: &::config::ActiveWindowConfig) -> String {
    if window.is_empty() {
        return telar::t!("activewindow.none");
    }
    if config.compact {
        compact_label(window)
    } else {
        label(window)
    }
}

// Seeded from the service's last reading rather than from Hyprland's socket, so the chip draws on a compositor that has none. The subscription below delivers the first one, but only on the next turn of the loop, which would leave the chip empty for a frame.
let initial = hyprland::current_active_window().unwrap_or_default();

let title = signal(text_for(&initial, &config));
let icon_name = signal(initial.class.clone());
let title_view = title.read_only();
let icon_view = icon_name.read_only();

platform_wayland::watch(
    hyprland::subscribe_active_window,
    move |window: ActiveWindow| {
        title.set(text_for(&window, &config));
        icon_name.set(window.class.clone());
    },
);

let fg = host.foreground;
let size = host.icon_size();
// The app's own artwork, not a tinted glyph: the point of this chip is recognising the app at a glance. A class with no installed icon simply renders nothing, leaving the title to carry the chip.
let inverted = config.inverted;
let leading = config.show_icon && !inverted;
let trailing = config.show_icon && inverted;

[view]
// Which side the icon sits on is config, decided once; *which* icon is the focused window, so the slot is keyed on the class and rebuilt whenever that changes — the artwork is a widget of a different kind per class (vector or raster), which no amount of reactive props can swap in place.
row align:center
    if leading
        match $icon_view as class key class.clone()
            class
                icon_slot class:class.clone() size:size inverted:inverted
    text "{$title_view}" font_size:$theme.font(FontRole::Body) color:fg lines:1 ellipsis
    if trailing
        match $icon_view as class key class.clone()
            class
                icon_slot class:class.clone() size:size inverted:inverted

[preview "Activewindow" fixture:crate::preview::activewindow]
activewindow
