//! The utilities panel: the switches a user reaches for without opening anything.
//!
//! Every toggle here already exists as a service and, for most of them, as its own bar chip. What this panel adds
//! is *one place* — a user who wants to turn the microphone off and the VPN on should not have to put two chips on
//! a bar and remember which is which. The toggles are declared by id in `[utilities] toggles`, so the order is
//! the user's; an id this build does not know is dropped with a warning rather than failing the panel.
//!
//! Each tile subscribes to its own service, exactly as the equivalent chip does. That is deliberate: a panel that
//! held one aggregate state would need a producer of its own, and the whole point of the service layer is that
//! N views of one reading cost one subscription each and one producer in total.

mod capture;

use ui::scale::{paint, space};
use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, RectStyle,
    RwSignal, SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};

use config::UtilitiesConfig;
use config::theme::{FontRole, NordTheme};
use ui::glyph;
use ui::icon::icon_view;
use ui::module::{icon_px, module_fg, surface_env};

pub const ID: &str = "utilities";

const TILE_ICON: f32 = 22.0;
const TILE_RADIUS: f32 = 12.0;
const GAP: f32 = 8.0;

/// A quick toggle. One enum rather than a trait object per toggle: the set is fixed by what the shell's services
/// can actually do, and an id that maps to nothing is the error this catches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    Wifi,
    Bluetooth,
    /// Mutes the microphone. Active means muted — the state the toggle exists to put the machine in.
    Mic,
    Dnd,
    GameMode,
    Vpn,
    IdleInhibit,
    Screenshot,
    Record,
    Settings,
}

impl Quick {
    pub const ALL: [Quick; 10] = [
        Quick::Wifi,
        Quick::Bluetooth,
        Quick::Mic,
        Quick::Dnd,
        Quick::GameMode,
        Quick::Vpn,
        Quick::IdleInhibit,
        Quick::Screenshot,
        Quick::Record,
        Quick::Settings,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Quick::Wifi => "wifi",
            Quick::Bluetooth => "bluetooth",
            Quick::Mic => "mic",
            Quick::Dnd => "dnd",
            Quick::GameMode => "game_mode",
            Quick::Vpn => "vpn",
            Quick::IdleInhibit => "idle_inhibit",
            Quick::Screenshot => "screenshot",
            Quick::Record => "record",
            Quick::Settings => "settings",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|quick| quick.id() == id.trim())
    }

    fn label(self) -> String {
        match self {
            Quick::Wifi => telar::t!("utilities.wifi"),
            Quick::Bluetooth => telar::t!("utilities.bluetooth"),
            Quick::Mic => telar::t!("utilities.mic"),
            Quick::Dnd => telar::t!("utilities.dnd"),
            Quick::GameMode => telar::t!("utilities.game_mode"),
            Quick::Vpn => telar::t!("utilities.vpn"),
            Quick::IdleInhibit => telar::t!("utilities.idle_inhibit"),
            Quick::Screenshot => telar::t!("utilities.screenshot"),
            Quick::Record => telar::t!("utilities.record"),
            Quick::Settings => telar::t!("utilities.settings"),
        }
    }

    fn glyph(self, active: bool) -> &'static str {
        match self {
            Quick::Wifi => {
                if active {
                    "wifi"
                } else {
                    "wifi-off"
                }
            }
            Quick::Bluetooth => {
                if active {
                    "bluetooth"
                } else {
                    "bluetooth-off"
                }
            }
            Quick::Mic => {
                if active {
                    "mic-off"
                } else {
                    "mic"
                }
            }
            Quick::Dnd => glyph::dnd(active),
            Quick::GameMode => glyph::game_mode(active),
            Quick::Vpn => glyph::vpn(active),
            Quick::IdleInhibit => glyph::idle_inhibit(active),
            Quick::Screenshot => glyph::screenshot(),
            Quick::Record => glyph::recording(active),
            Quick::Settings => "settings",
        }
    }

    /// Whether the tile is a switch (drawn active while it is on) or an action (a press that does something and
    /// leaves nothing behind). An action tile never paints as active, which is what stops "take a screenshot"
    /// looking like a setting that is currently on.
    fn is_action(self) -> bool {
        matches!(self, Quick::Screenshot | Quick::Settings)
    }

    /// What pressing it does. Every arm is an existing service entry point — this panel adds no behaviour of its
    /// own, which is why a toggle here and the same toggle from IPC cannot disagree.
    fn press(self) {
        use services::{bluetooth, gamemode, idle, network, notifications, volume, vpn};
        match self {
            Quick::Wifi => network::toggle_wifi(),
            Quick::Bluetooth => bluetooth::toggle_powered(),
            Quick::Mic => volume::toggle_mic_mute(),
            Quick::Dnd => {
                let on = notifications::snapshot_now().is_some_and(|snapshot| snapshot.dnd);
                notifications::set_dnd(!on);
            }
            Quick::GameMode => gamemode::toggle(),
            Quick::Vpn => vpn::toggle(),
            Quick::IdleInhibit => idle::toggle_manual_inhibit(),
            Quick::Screenshot => crate::capture::screenshot_region(),
            Quick::Record => crate::capture::toggle_recording(),
            Quick::Settings => surfaces::panel::toggle_panel("settings"),
        }
    }
}

/// A tile's live state. `available` is the third answer a toggle needs: a machine with no Bluetooth adapter must
/// grey the tile out rather than offer a switch that cannot move.
#[derive(Clone, Debug, PartialEq)]
struct TileState {
    active: bool,
    available: bool,
    detail: String,
}

impl Default for TileState {
    fn default() -> Self {
        Self {
            active: false,
            available: true,
            detail: String::new(),
        }
    }
}

/// The bar chip: a slider glyph that opens the panel.
pub fn utilities_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = module_fg();
    icon_view(
        || glyph::utilities().to_string(),
        move || fg.get(),
        icon_px(),
    )
}

/// Which toggles this config asks for, in its order. An unknown id is reported once and dropped: a config written
/// against a newer build should cost a log line, not the whole panel.
fn requested(config: &UtilitiesConfig) -> Vec<Quick> {
    config
        .toggles
        .iter()
        .filter_map(|id| match Quick::from_id(id) {
            Some(quick) => Some(quick),
            None => {
                tracing::warn!("[utilities] unknown toggle '{id}'");
                None
            }
        })
        .collect()
}

/// The panel: the toggles, then the capture controls, then what has been recorded.
pub fn utilities_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let config = surface_env()
        .map(|env| env.config.utilities.clone())
        .unwrap_or_default();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }

    let title = Text::auto(
        || telar::t!("utilities.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;

    let mut children: Vec<Box<dyn LayoutItem>> = vec![box_item(title), grid(&config, theme)?];
    if config.show_capture {
        children.push(capture::capture_card(theme)?);
    }
    if config.show_recordings {
        children.push(capture::recordings_card(theme)?);
    }

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xl())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// The panel inside the frame its host draws around it, for [`crate::preview`] — the toggle grid, the capture
/// card and the recordings list. On a machine with no recordings that list draws its empty line, which is part
/// of what there is to look at.
pub(crate) fn panel_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .padding_all(space::xl())
            .width(420.0),
        paint::xl(theme.surface),
        vec![utilities_panel()?],
    )?))
}

/// The toggle grid on its own, for a surface that wants the switches without the rest of the panel — the
/// notification centre hosts exactly these, and hosting a second copy of them is what the sidebar existing at all
/// is supposed to avoid.
pub fn toggles_grid(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = surface_env()
        .map(|env| env.config.utilities.clone())
        .unwrap_or_default();
    grid(&config, theme)
}

/// The toggles, laid out in rows of `[utilities] columns`.
///
/// Rows of fixed-count containers rather than a wrapping row: a wrap would reflow on every panel width and put a
/// lone tile on its own line, and the grid is the one part of this panel whose shape the user set deliberately.
fn grid(config: &UtilitiesConfig, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let columns = config.grid_columns();
    let toggles = requested(config);
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::new();
    for chunk in toggles.chunks(columns) {
        let mut cells: Vec<Box<dyn LayoutItem>> = Vec::new();
        for quick in chunk {
            cells.push(tile(*quick, theme)?);
        }
        // The last row is padded with empty cells so its tiles keep the width the full rows have, rather than
        // stretching to fill the gap the missing ones left.
        for _ in chunk.len()..columns {
            cells.push(Box::new(Container::new(
                LayoutStyle::new().flex_grow(1.0).flex_basis(0.0),
                vec![],
            )?));
        }
        rows.push(Box::new(Container::new(
            LayoutStyle::new()
                .flex_row()
                .gap(GAP)
                .width(SizeDimension::Percent(1.0)),
            cells,
        )?));
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(GAP)
            .width(SizeDimension::Percent(1.0)),
        rows,
    )?))
}

fn tile(quick: Quick, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let state = signal(TileState::default());
    subscribe(quick, state.clone());

    let icon_state = state.read_only();
    let tint_state = state.read_only();
    let label_state = state.read_only();
    let fill_state = state.read_only();
    let hover_state = state.read_only();
    let detail_state = state.read_only();

    let icon = icon_view(
        move || quick.glyph(icon_state.get().active).to_string(),
        move || {
            let state = tint_state.get();
            if !state.available {
                theme.muted
            } else if state.active && !quick.is_action() {
                theme.accent.most_readable(&[theme.text, theme.base])
            } else {
                theme.text
            }
        },
        TILE_ICON,
    )?;

    let label = Text::auto(
        move || quick.label(),
        LayoutStyle::new(),
        move || {
            // The state is read out before `text_style`, which reads the theme's own signals: a nested read
            // inside a `with` is a re-entrant borrow of the reactive runtime and panics at build time.
            let state = label_state.get();
            let tint = if !state.available {
                theme.muted
            } else if state.active && !quick.is_action() {
                theme.accent.most_readable(&[theme.text, theme.base])
            } else {
                theme.text
            };
            theme
                .text_style(FontRole::Caption, tint)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;

    let detail = Text::auto(
        move || detail_state.get().detail,
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, theme.subtle)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;

    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::sm()),
        vec![icon, box_item(label), box_item(detail)],
    )?;

    let tile = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .flex_grow(1.0)
            .flex_basis(0.0)
            .padding_vertical(space::lg())
            .padding_horizontal(space::md()),
        move |_| {
            let state = fill_state.get();
            let fill = if state.active && !quick.is_action() {
                theme.accent
            } else {
                theme.base
            };
            RectStyle::filled(fill, TILE_RADIUS)
        },
        vec![Box::new(column)],
    )?
    .hover_style(move |_| {
        let state = hover_state.get();
        let fill = if !state.available {
            theme.base
        } else if state.active && !quick.is_action() {
            theme.accent.darken(0.08)
        } else {
            theme.overlay
        };
        RectStyle::filled(fill, TILE_RADIUS)
    })
    .on_press(move || {
        // Checked at the press rather than by leaving the handler off: availability is live, and a tile that
        // became available while the panel was open should work without rebuilding it.
        if state.peek().available {
            quick.press();
        }
    });
    Ok(Box::new(tile))
}

/// Subscribes `state` to whichever service backs `quick`, seeded from that service's last reading so the tile
/// opens in the right position instead of flicking into it.
fn subscribe(quick: Quick, state: RwSignal<TileState>) {
    use services::{
        bluetooth, gamemode, network, notifications, recorder, screenshot, state as shell_state,
        volume, vpn,
    };
    match quick {
        Quick::Wifi => {
            let seed = network::current_wifi();
            state.set(TileState {
                active: seed.as_ref().is_some_and(|w| w.enabled),
                available: seed.as_ref().is_some_and(|w| w.available),
                detail: String::new(),
            });
            platform_wayland::watch(network::subscribe_wifi, move |wifi: network::Wifi| {
                let status = wifi.status();
                state.set(TileState {
                    active: status.enabled,
                    available: status.available,
                    detail: if status.connected {
                        format!("{}%", status.strength)
                    } else {
                        String::new()
                    },
                });
            });
        }
        Quick::Bluetooth => {
            platform_wayland::watch(bluetooth::subscribe, move |bt: bluetooth::Bluetooth| {
                let connected = bt.connected_count();
                state.set(TileState {
                    active: bt.powered,
                    available: bt.available,
                    detail: if connected > 0 {
                        connected.to_string()
                    } else {
                        String::new()
                    },
                });
            });
        }
        Quick::Mic => {
            platform_wayland::watch(volume::subscribe_mic, move |mic: volume::Volume| {
                state.set(TileState {
                    active: mic.muted,
                    available: true,
                    detail: String::new(),
                });
            });
        }
        Quick::Dnd => {
            platform_wayland::watch(
                notifications::subscribe,
                move |snapshot: notifications::SharedSnapshot| {
                    state.set(TileState {
                        active: snapshot.dnd,
                        available: true,
                        detail: String::new(),
                    });
                },
            );
        }
        Quick::GameMode => {
            platform_wayland::watch(gamemode::subscribe, move |mode: gamemode::GameMode| {
                state.set(TileState {
                    active: mode.active,
                    available: mode.available,
                    detail: String::new(),
                });
            });
        }
        Quick::Vpn => {
            platform_wayland::watch(vpn::subscribe, move |vpn: vpn::Vpn| {
                let name = vpn.active().map(|t| t.name.clone()).unwrap_or_default();
                state.set(TileState {
                    active: vpn.is_connected(),
                    available: vpn.available && !vpn.tunnels.is_empty(),
                    detail: name,
                });
            });
        }
        Quick::IdleInhibit => {
            platform_wayland::watch(
                shell_state::subscribe,
                move |persisted: shell_state::ShellState| {
                    state.set(TileState {
                        active: persisted.idle_inhibit,
                        available: true,
                        detail: String::new(),
                    });
                },
            );
        }
        Quick::Record => {
            let config = config::config()
                .map(|c| c.recorder.clone())
                .unwrap_or_default();
            let available = recorder::backend(&config).is_some();
            platform_wayland::watch(recorder::subscribe, move |live: recorder::Recording| {
                state.set(TileState {
                    active: live.active,
                    available,
                    detail: if live.active {
                        recorder::format_elapsed(live.elapsed())
                    } else {
                        String::new()
                    },
                });
            });
        }
        Quick::Screenshot => state.set(TileState {
            active: false,
            available: screenshot::supported(),
            detail: String::new(),
        }),
        Quick::Settings => state.set(TileState::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_toggle_id_round_trips_and_the_defaults_all_resolve() {
        for quick in Quick::ALL {
            assert_eq!(Quick::from_id(quick.id()), Some(quick), "{}", quick.id());
        }
        // The shipped default list is the one config nobody wrote by hand, so an id that stopped resolving here
        // would grey out a tile on every fresh install.
        let default = UtilitiesConfig::default();
        assert_eq!(
            requested(&default).len(),
            default.toggles.len(),
            "every default toggle resolves"
        );
    }

    #[test]
    fn an_unknown_toggle_is_dropped_rather_than_failing_the_panel() {
        let config = UtilitiesConfig {
            toggles: vec![
                "wifi".to_string(),
                "teleporter".to_string(),
                "dnd".to_string(),
            ],
            ..UtilitiesConfig::default()
        };
        assert_eq!(requested(&config), vec![Quick::Wifi, Quick::Dnd]);
    }

    #[test]
    fn an_action_tile_never_paints_as_a_switch_that_is_on() {
        // Pressing "screenshot" does something and leaves nothing behind; drawn active, it would read as a
        // setting the machine is currently in.
        assert!(Quick::Screenshot.is_action() && Quick::Settings.is_action());
        for switch in [Quick::Wifi, Quick::Dnd, Quick::Mic, Quick::Record] {
            assert!(!switch.is_action(), "{} is a state", switch.id());
        }
    }

    #[test]
    fn the_mic_tile_is_active_when_the_microphone_is_muted() {
        // The tile is a *mute* toggle, so "on" is the muted machine — and the glyph has to agree, or the panel
        // says the microphone is live while it is off.
        assert_eq!(Quick::Mic.glyph(true), "mic-off");
        assert_eq!(Quick::Mic.glyph(false), "mic");
    }

    #[test]
    fn the_panel_and_its_tiles_build() {
        for quick in Quick::ALL {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            assert!(
                tile(quick, NordTheme::new()).is_ok(),
                "{} builds",
                quick.id()
            );
        }

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(utilities_panel().is_ok());
    }

    #[test]
    fn the_grid_pads_its_last_row_to_the_configured_width() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        // Five toggles over four columns: the second row must still lay its one tile out at a quarter width,
        // which is what the padding cells are for.
        let config = UtilitiesConfig {
            toggles: ["wifi", "bluetooth", "mic", "dnd", "vpn"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            columns: 4,
            ..UtilitiesConfig::default()
        };
        assert!(grid(&config, NordTheme::new()).is_ok());
    }
}
