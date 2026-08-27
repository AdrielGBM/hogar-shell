//! The compact status cluster: several service icons sharing one chip.
//!
//! The alternative is what the bar offers today — a chip per reading, each with its own padding, background and hover target. That is fine for two or three, and wasteful for eight. This draws the same glyphs, from the same [`shared::glyph`](ui::glyph) source the standalone chips use, inside a single chip.
//!
//! Ordered by config rather than by a fixed list, because the order icons sit in is the whole point of a cluster: a user who reads left-to-right wants their own priority, not the shell's.

use telar::{
    AlignItems, Color, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ReadSignal, signal,
};

use config::StatusIconsConfig;
use config::theme::NordTheme;
use services::{battery, bluetooth, lockkeys, network, volume};
use ui::glyph;
use ui::icon::icon_view;

/// One reading the cluster can show. The names are the ones `[status_icons] icons` accepts, and they match the module ids the same readings have as standalone chips so a user moving between the two is not renaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusIcon {
    Volume,
    Mic,
    Network,
    /// The wireless radio specifically, as opposed to [`Network`](Self::Network), which answers "am I online" over any link. A desktop with a cable wants the first; a laptop that roams wants both.
    Wifi,
    Bluetooth,
    Battery,
    Caps,
    Num,
}

impl StatusIcon {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id.trim() {
            "volume" => Self::Volume,
            "mic" => Self::Mic,
            "network" => Self::Network,
            "wifi" => Self::Wifi,
            "bluetooth" => Self::Bluetooth,
            "battery" => Self::Battery,
            "caps" => Self::Caps,
            "num" => Self::Num,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Volume => "volume",
            Self::Mic => "mic",
            Self::Network => "network",
            Self::Wifi => "wifi",
            Self::Bluetooth => "bluetooth",
            Self::Battery => "battery",
            Self::Caps => "caps",
            Self::Num => "num",
        }
    }
}

/// The icons to draw, in the order configured. An unknown name is dropped with a warning rather than failing the whole cluster: a typo should cost one icon, not the chip.
pub fn icons(config: &StatusIconsConfig) -> Vec<StatusIcon> {
    config
        .icons
        .iter()
        .filter_map(|id| match StatusIcon::from_id(id) {
            Some(icon) => Some(icon),
            None => {
                tracing::warn!("unknown status icon '{id}'");
                None
            }
        })
        .collect()
}

/// One icon, subscribed to its own service.
///
/// Each is a separate subscription rather than one combined snapshot, so a cluster that shows only the network never starts the audio watcher — the services are lazy, and asking for a reading is what starts one.
fn icon(
    which: StatusIcon,
    fg: ReadSignal<Color>,
    theme: NordTheme,
    size: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    match which {
        StatusIcon::Volume => {
            let state = signal(volume::current().unwrap_or(volume::Volume {
                level: 0,
                muted: false,
            }));
            let read = state.read_only();
            platform_wayland::watch(volume::subscribe, move |v| state.set(v));
            icon_view(
                move || glyph::volume(read.get()).to_string(),
                move || fg.get(),
                size,
            )
        }
        StatusIcon::Mic => {
            let state = signal(volume::current_mic().unwrap_or(volume::Volume {
                level: 0,
                muted: true,
            }));
            let read = state.read_only();
            platform_wayland::watch(volume::subscribe_mic, move |v| state.set(v));
            icon_view(
                move || glyph::microphone(read.get()).to_string(),
                move || fg.get(),
                size,
            )
        }
        StatusIcon::Network => {
            let state = signal(network::read());
            let read = state.read_only();
            platform_wayland::watch(network::subscribe, move |net| state.set(net));
            icon_view(
                move || glyph::network(read.get()).to_string(),
                move || fg.get(),
                size,
            )
        }
        StatusIcon::Wifi => {
            let state = signal(
                network::current_wifi()
                    .map(|w| w.status())
                    .unwrap_or_default(),
            );
            let glyph_state = state.read_only();
            let tint_state = state.read_only();
            platform_wayland::watch(network::subscribe_wifi, move |w| state.set(w.status()));
            icon_view(
                move || glyph::wifi(glyph_state.get()).to_string(),
                move || glyph::wifi_tint(tint_state.get(), theme, fg.get()),
                size,
            )
        }
        StatusIcon::Bluetooth => {
            let state = signal(
                bluetooth::current()
                    .map(|bt| bt.status())
                    .unwrap_or_default(),
            );
            let glyph_state = state.read_only();
            let tint_state = state.read_only();
            platform_wayland::watch(bluetooth::subscribe, move |bt| state.set(bt.status()));
            icon_view(
                move || glyph::bluetooth(glyph_state.get()).to_string(),
                move || glyph::bluetooth_tint(tint_state.get(), theme, theme.accent, fg.get()),
                size,
            )
        }
        StatusIcon::Battery => {
            let init = battery::read();
            let level = signal(init.map(|b| b.level).unwrap_or(0));
            let charging = signal(init.map(|b| b.charging).unwrap_or(false));
            let (level_read, charging_read) = (level.read_only(), charging.read_only());
            let charging_glyph = charging.read_only();
            platform_wayland::watch(battery::subscribe, move |b| {
                level.set(b.level);
                charging.set(b.charging);
            });
            icon_view(
                move || glyph::battery(charging_glyph.get()).to_string(),
                move || glyph::battery_tint(level_read.get(), charging_read.get(), theme, fg.get()),
                size,
            )
        }
        StatusIcon::Caps | StatusIcon::Num => {
            let keys = signal(lockkeys::current().unwrap_or_else(lockkeys::read));
            let read = keys.read_only();
            platform_wayland::watch(lockkeys::subscribe, move |k| keys.set(k));
            let caps = which == StatusIcon::Caps;
            icon_view(
                move || {
                    if caps {
                        glyph::caps_lock().to_string()
                    } else {
                        glyph::num_lock().to_string()
                    }
                },
                // A lock key is the one reading here usually *off*, so idle recedes to muted rather than sitting at full strength beside live ones.
                move || {
                    let engaged = if caps {
                        read.get().caps
                    } else {
                        read.get().num
                    };
                    if engaged { fg.get() } else { theme.muted }
                },
                size,
            )
        }
    }
}

/// The cluster's content: the configured icons in a row (or a column on a vertical bar).
pub fn cluster() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = ui::module::surface_env()
        .map(|env| env.config.status_icons.clone())
        .unwrap_or_default();
    let theme = telar::use_theme::<NordTheme>();
    let fg = ui::module::module_fg();
    let size = ui::module::icon_px();
    let vertical = ui::module::bar_is_vertical();

    let mut items: Vec<Box<dyn LayoutItem>> = Vec::new();
    for which in icons(&config) {
        items.push(icon(which, fg, theme, size)?);
    }

    let style = if vertical {
        LayoutStyle::new().flex_column()
    } else {
        LayoutStyle::new().flex_row()
    };
    Ok(Box::new(telar::Container::new(
        style
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .gap((size * config.spacing).round().max(1.0)),
        items,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_order_is_the_drawn_order() {
        // A cluster's order is the user's priority, so it follows the list rather than a fixed sequence.
        let config = StatusIconsConfig {
            icons: vec!["battery".into(), "volume".into(), "network".into()],
            ..StatusIconsConfig::default()
        };
        assert_eq!(
            icons(&config),
            vec![StatusIcon::Battery, StatusIcon::Volume, StatusIcon::Network]
        );
    }

    #[test]
    fn a_typo_costs_one_icon_rather_than_the_whole_chip() {
        let config = StatusIconsConfig {
            icons: vec!["volume".into(), "nonesuch".into(), "battery".into()],
            ..StatusIconsConfig::default()
        };
        assert_eq!(
            icons(&config),
            vec![StatusIcon::Volume, StatusIcon::Battery]
        );
    }

    /// A user moving a reading between the cluster and its own chip should not have to rename it. That the names it shares are module ids is checked where both lists are in scope, in the shell's composition root.
    #[test]
    fn every_name_round_trips() {
        for name in ["volume", "mic", "network", "bluetooth", "battery"] {
            let icon = StatusIcon::from_id(name).expect("a known icon");
            assert_eq!(icon.as_str(), name);
        }
        // `wifi` has no module of its own: the `network` chip already covers "am I online" over any link, and splitting it in two on the bar would be a second chip saying most of the same thing. In a cluster, where an icon costs almost nothing, the finer reading earns its place.
        assert_eq!(StatusIcon::from_id("wifi"), Some(StatusIcon::Wifi));
        // The exception, deliberately: `lockstatus` is one module drawing two indicators, so a cluster can take just one.
        assert_eq!(StatusIcon::from_id("caps"), Some(StatusIcon::Caps));
        assert_eq!(StatusIcon::from_id("num"), Some(StatusIcon::Num));
        assert_eq!(StatusIcon::from_id("lockstatus"), None);
    }

    /// Every icon in one cluster, built for real. Each arm of [`icon`] wires its own subscription and reads the shared foreground signal in its tint closure; doing that from inside another signal's `with` is a re-entrant borrow of the reactive runtime, which panics at build time and nowhere else.
    #[test]
    fn every_icon_the_cluster_offers_builds() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let fg = signal(NordTheme::new().text).read_only();
        for which in [
            StatusIcon::Volume,
            StatusIcon::Mic,
            StatusIcon::Network,
            StatusIcon::Wifi,
            StatusIcon::Bluetooth,
            StatusIcon::Battery,
            StatusIcon::Caps,
            StatusIcon::Num,
        ] {
            assert!(
                icon(which, fg, NordTheme::new(), 16.0).is_ok(),
                "'{}' builds",
                which.as_str()
            );
        }
    }

    #[test]
    fn the_default_cluster_is_the_readings_a_laptop_bar_carries() {
        let config = StatusIconsConfig::default();
        assert_eq!(
            icons(&config),
            vec![
                StatusIcon::Volume,
                StatusIcon::Mic,
                StatusIcon::Network,
                StatusIcon::Battery
            ]
        );
    }
}
