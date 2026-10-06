//! Every module the shell ships, one descriptor each: the only place that knows both the descriptor vocabulary and the modules themselves.

use platform_wayland::KeyboardMode;
use telar::Children;

use config::{
    ActiveWindowConfig, AudioConfig, BatteryConfig, BluetoothConfig, BrightnessConfig, ClockConfig,
    DashboardConfig, GeneralConfig, GpuConfig, LauncherConfig, LockStatusConfig, MediaConfig,
    NetworkConfig, NotificationsConfig, PathsConfig, RecorderConfig, StackConfig,
    StatusIconsConfig, TemperatureConfig, TrayConfig, UtilitiesConfig, VisualiserConfig,
    WeatherConfig, WorkspacesConfig,
};
use ui::descriptor::{
    ActionDef, Built, CardDef, Category, ChipDef, Input, ModuleDescriptor, OptionsType, PanelDef,
    Representations, WidgetDef,
};
use ui::host::{Host, WidgetSize};

use modules::dashboard::cards;
use modules::popout_cards as popouts;

use crate::core::sources;

/// A representation that is an `.rsx` component: parameterless, reading its host with `Host::current`.
macro_rules! rsx {
    ($($module:ident)::+, $component:ident, $props:ident) => {{
        fn build(_host: &Host) -> Built {
            $($module)::+::$component($($module)::+::$props::props().build(), Children::default())
        }
        build
    }};
}

const fn popout(build: fn(&Host) -> ui::card::Card) -> Option<CardDef> {
    Some(CardDef {
        build,
        input: Input::ReadOnly,
    })
}

const fn card(build: fn(&Host) -> ui::card::Card, input: Input) -> Option<CardDef> {
    Some(CardDef { build, input })
}

const fn action(id: &'static str, command: &'static str) -> ActionDef {
    ActionDef { id, command }
}

/// A reading at `sizes`: every widget the shell ships only shows what its sources read, so each is one the lock layer may place.
const fn reading(sizes: &'static [WidgetSize], build: ui::descriptor::Build) -> Option<WidgetDef> {
    Some(WidgetDef {
        sizes,
        build,
        input: Input::ReadOnly,
    })
}

const SMALL_AND_MEDIUM: &[WidgetSize] = &[WidgetSize::S, WidgetSize::M];
const EVERY_SIZE: &[WidgetSize] = &WidgetSize::ALL;

fn icon_chip(host: &Host, glyph: &'static str) -> Built {
    let fg = host.foreground;
    ui::icon::icon_view(move || glyph.to_string(), move || fg, host.icon_size())
}

fn session_panel(_host: &Host) -> Built {
    modules::session::session_panel()
}

fn settings_panel(host: &Host) -> Built {
    settings::panel::settings_panel(host.extent.height)
}

pub static MODULES: &[ModuleDescriptor] = &[
    ModuleDescriptor {
        id: "activewindow",
        name: "Active window",
        icon: "app-window",
        category: Category::Windows,
        options: &[OptionsType::of::<ActiveWindowConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(
                        modules::activewindow::activewindow,
                        activewindow,
                        ActivewindowProps
                    ),
                    Input::ReadOnly,
                )
                .on_press(modules::activewindow::focus_active)
                .elastic(),
            ),
            popout: popout(popouts::activewindow),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "battery",
        name: "Battery",
        icon: "battery",
        category: Category::System,
        options: &[OptionsType::of::<BatteryConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::battery::battery, battery, BatteryProps),
                    Input::ReadOnly,
                )
                .square(),
            ),
            widget: reading(SMALL_AND_MEDIUM, modules::battery::widget::widget),
            card: card(cards::battery, Input::ReadOnly),
            panel: Some(PanelDef::new(
                rsx!(
                    modules::battery::battery_panel,
                    battery_panel,
                    BatteryPanelProps
                ),
                Input::ReadOnly,
            )),
            popout: popout(popouts::battery),
        },
        actions: &[],
        sources: &[sources::BATTERY, sources::POWER],
    },
    ModuleDescriptor {
        id: "bluetooth",
        name: "Bluetooth",
        icon: "bluetooth",
        category: Category::Network,
        options: &[OptionsType::of::<BluetoothConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(modules::bluetooth::chip, Input::ReadOnly).square()),
            panel: Some(PanelDef::new(
                modules::bluetooth::bluetooth_panel,
                Input::Interactive,
            )),
            popout: popout(popouts::bluetooth),
            ..Representations::NONE
        },
        actions: &[action("power", "bluetooth power toggle")],
        sources: &[],
    },
    ModuleDescriptor {
        id: "brightness",
        name: "Brightness",
        icon: "sun",
        category: Category::System,
        options: &[OptionsType::of::<BrightnessConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::brightness::brightness, brightness, BrightnessProps),
                    Input::ReadOnly,
                )
                .square()
                .on_press(modules::osd::brightness_action)
                .on_scroll(modules::osd::brightness_scroll),
            ),
            popout: popout(popouts::brightness),
            ..Representations::NONE
        },
        actions: &[
            action("up", "brightness up"),
            action("down", "brightness down"),
        ],
        sources: &[],
    },
    ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        category: Category::Time,
        options: &[OptionsType::of::<ClockConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::clock::clock, clock, ClockProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::clock::face::widget),
            card: card(cards::clock, Input::ReadOnly),
            panel: Some(PanelDef::new(
                rsx!(modules::clock::clock_panel, clock_panel, ClockPanelProps),
                Input::ReadOnly,
            )),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::CLOCK],
    },
    ModuleDescriptor {
        id: "cpu",
        name: "CPU",
        icon: "cpu",
        category: Category::System,
        options: &[
            OptionsType::of::<DashboardConfig>(),
            OptionsType::of::<TemperatureConfig>(),
        ],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::sysinfo::cpu, cpu, CpuProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::sysinfo::widgets::cpu),
            card: card(cards::cpu, Input::ReadOnly),
            popout: popout(popouts::cpu),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::CPU],
    },
    ModuleDescriptor {
        id: "dashboard",
        name: "Dashboard",
        icon: "layout-dashboard",
        category: Category::Shell,
        options: &[OptionsType::of::<DashboardConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(modules::dashboard::dashboard_chip, Input::ReadOnly).square()),
            panel: Some(PanelDef::new(
                modules::dashboard::dashboard_panel,
                Input::Interactive,
            )),
            ..Representations::NONE
        },
        actions: &[action("toggle", "dashboard toggle")],
        sources: &[],
    },
    ModuleDescriptor {
        id: "gpu",
        name: "GPU",
        icon: "gpu",
        category: Category::System,
        options: &[
            OptionsType::of::<GpuConfig>(),
            OptionsType::of::<TemperatureConfig>(),
        ],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::sysinfo::gpu, gpu, GpuProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::sysinfo::widgets::gpu),
            card: card(cards::gpu, Input::ReadOnly),
            popout: popout(popouts::gpu),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::GPU],
    },
    ModuleDescriptor {
        id: "kblayout",
        name: "Keyboard layout",
        icon: "keyboard",
        category: Category::System,
        options: &[],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::kblayout::kblayout, kblayout, KblayoutProps),
                    Input::ReadOnly,
                )
                .on_press(services::hyprland::cycle_main_keyboard_layout),
            ),
            popout: popout(popouts::kblayout),
            ..Representations::NONE
        },
        actions: &[action("next", "keyboard next")],
        sources: &[],
    },
    ModuleDescriptor {
        id: "launcher",
        name: "Launcher",
        icon: "search",
        category: Category::Shell,
        options: &[OptionsType::of::<LauncherConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(|host| icon_chip(host, "search"), Input::ReadOnly)
                    .square()
                    .on_press(modules::launcher::toggle),
            ),
            ..Representations::NONE
        },
        actions: &[action("toggle", "launcher toggle")],
        sources: &[],
    },
    // Self-managed: it draws its own indicator row, and with `hide_inactive` that row can be empty — a chip shell would leave a padded gap in the bar where nothing is shown.
    ModuleDescriptor {
        id: "lockstatus",
        name: "Lock keys",
        icon: "lock",
        category: Category::System,
        options: &[OptionsType::of::<LockStatusConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::lockstatus::lockstatus, lockstatus, LockstatusProps),
                    Input::ReadOnly,
                )
                .self_managed(),
            ),
            popout: popout(popouts::lockstatus),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    // The session menu under the distribution's mark: the press opens the session panel rather than a panel of its own.
    ModuleDescriptor {
        id: "logo",
        name: "Logo",
        icon: "circle-dot",
        category: Category::Shell,
        options: &[OptionsType::of::<GeneralConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(modules::logo::logo_chip, Input::ReadOnly)
                    .square()
                    .on_press(|| surfaces::panel::toggle_panel("session")),
            ),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "media",
        name: "Media",
        icon: "music",
        category: Category::Media,
        options: &[
            OptionsType::of::<MediaConfig>(),
            OptionsType::of::<VisualiserConfig>(),
            OptionsType::of::<DashboardConfig>(),
        ],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::media::media, media, MediaProps),
                    Input::ReadOnly,
                )
                .on_press(modules::media::toggle)
                .on_scroll(modules::media::scroll)
                .elastic(),
            ),
            widget: reading(SMALL_AND_MEDIUM, modules::media::widget::widget),
            card: card(cards::media, Input::Interactive),
            popout: popout(popouts::media),
            ..Representations::NONE
        },
        actions: &[
            action("play-pause", "media play-pause"),
            action("next", "media next"),
            action("previous", "media previous"),
            action("stop", "media stop"),
        ],
        sources: &[modules::media::SOURCE],
    },
    ModuleDescriptor {
        id: "memory",
        name: "Memory",
        icon: "memory-stick",
        category: Category::System,
        options: &[OptionsType::of::<DashboardConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::sysinfo::memory, memory, MemoryProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::sysinfo::widgets::memory),
            card: card(cards::memory, Input::ReadOnly),
            popout: popout(popouts::memory),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::MEMORY],
    },
    ModuleDescriptor {
        id: "mic",
        name: "Microphone",
        icon: "mic",
        category: Category::Media,
        options: &[OptionsType::of::<AudioConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(rsx!(modules::mic::mic, mic, MicProps), Input::ReadOnly)
                    .square()
                    .on_press(modules::osd::mic_action)
                    .on_scroll(modules::osd::mic_scroll),
            ),
            popout: popout(popouts::mic),
            ..Representations::NONE
        },
        actions: &[
            action("up", "mic up"),
            action("down", "mic down"),
            action("mute", "mic mute"),
        ],
        sources: &[],
    },
    // The pointer path to a non-default device. The volume chip stays what it is — a level, a mute and a wheel — because a chip that opened a panel could no longer toggle mute with the same press.
    ModuleDescriptor {
        id: "mixer",
        name: "Mixer",
        icon: "sliders-horizontal",
        category: Category::Media,
        options: &[OptionsType::of::<AudioConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    |host| icon_chip(host, "sliders-horizontal"),
                    Input::ReadOnly,
                )
                .square(),
            ),
            panel: Some(PanelDef::new(
                modules::mixer::mixer_panel,
                Input::Interactive,
            )),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "netspeed",
        name: "Network speed",
        icon: "arrow-down-up",
        category: Category::Network,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::sysinfo::netspeed, netspeed, NetspeedProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::sysinfo::widgets::netspeed),
            card: card(cards::netspeed, Input::ReadOnly),
            popout: popout(popouts::netspeed),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::NETSPEED],
    },
    ModuleDescriptor {
        id: "network",
        name: "Network",
        icon: "wifi",
        category: Category::Network,
        options: &[OptionsType::of::<NetworkConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::network::network, network, NetworkProps),
                    Input::ReadOnly,
                )
                .square(),
            ),
            panel: Some(PanelDef::new(
                modules::network::network_panel,
                Input::Interactive,
            )),
            popout: popout(popouts::network),
            ..Representations::NONE
        },
        actions: &[
            action("radio", "wifi radio toggle"),
            action("scan", "wifi scan"),
        ],
        sources: &[],
    },
    ModuleDescriptor {
        id: "notes",
        name: "Notes",
        icon: "sticky-note",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(modules::notes::notes_chip, Input::ReadOnly).square()),
            panel: Some(
                PanelDef::new(modules::notes::notes_panel, Input::Interactive)
                    .keyboard(KeyboardMode::OnDemand),
            ),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "notifications",
        name: "Notifications",
        icon: "bell",
        category: Category::Info,
        options: &[
            OptionsType::of::<NotificationsConfig>(),
            OptionsType::of::<StackConfig>(),
        ],
        representations: Representations {
            chip: Some(ChipDef::new(
                modules::notifications::bell_module,
                Input::ReadOnly,
            )),
            widget: reading(EVERY_SIZE, modules::notifications::reading::widget),
            panel: Some(PanelDef::new(
                modules::notifications::bell_panel,
                Input::Interactive,
            )),
            ..Representations::NONE
        },
        actions: &[
            action("center", "notifs center toggle"),
            action("dnd", "notifs dnd toggle"),
            action("clear", "notifs clear"),
        ],
        sources: &[modules::notifications::reading::SOURCE],
    },
    // Its tiles are a list, and a menu whose most destructive entries are two presses away is exactly the one a user wants to reach without moving their hand to the mouse — so it takes the keyboard to be navigable.
    ModuleDescriptor {
        id: "session",
        name: "Session",
        icon: "power",
        category: Category::Shell,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(modules::session::power_chip, Input::ReadOnly).square()),
            panel: Some(
                PanelDef::new(session_panel, Input::Interactive).keyboard(KeyboardMode::OnDemand),
            ),
            ..Representations::NONE
        },
        actions: &[
            action("lock", "session do lock"),
            action("logout", "session do logout"),
            action("suspend", "session do suspend"),
            action("reboot", "session do reboot"),
            action("shutdown", "session do shutdown"),
        ],
        sources: &[],
    },
    // The window keeps its Revert snapshot for as long as it is open, and must not keep it across a user closing it.
    ModuleDescriptor {
        id: "settings",
        name: "Settings",
        icon: "settings",
        category: Category::Shell,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(settings::panel::settings_chip, Input::ReadOnly).square()),
            panel: Some(
                PanelDef::new(settings_panel, Input::Interactive)
                    .keyboard(KeyboardMode::OnDemand)
                    .on_close(settings::panel::forget_panel_state),
            ),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "spacer",
        name: "Spacer",
        icon: "move-horizontal",
        category: Category::Shell,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(|_host| modules::spacer::spacer(), Input::ReadOnly).filler()),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    // The chip shell but no press: which of several readings would a press act on? Each keeps its standalone module.
    ModuleDescriptor {
        id: "statusicons",
        name: "Status icons",
        icon: "signal",
        category: Category::System,
        options: &[OptionsType::of::<StatusIconsConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(modules::statusicons::cluster, Input::ReadOnly)),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "temperature",
        name: "Temperature",
        icon: "thermometer",
        category: Category::System,
        options: &[OptionsType::of::<TemperatureConfig>()],
        representations: Representations {
            chip: Some(ChipDef::new(
                rsx!(modules::sysinfo::temperature, temperature, TemperatureProps),
                Input::ReadOnly,
            )),
            widget: reading(SMALL_AND_MEDIUM, modules::sysinfo::widgets::temperature),
            popout: popout(popouts::temperature),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::TEMPERATURE],
    },
    // Self-managed: one pressable box per application, each with its own click, middle-click, right-click and scroll — a single chip shell around the row could carry none of that.
    ModuleDescriptor {
        id: "tray",
        name: "Tray",
        icon: "panel-bottom",
        category: Category::Windows,
        options: &[OptionsType::of::<TrayConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::tray::tray, tray, TrayProps),
                    Input::Interactive,
                )
                .self_managed(),
            ),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    // Readings only: the picture and the name are what a lock screen greets its user with, and the dashboard's user card, which changes the picture, is that card's.
    ModuleDescriptor {
        id: "user",
        name: "User",
        icon: "circle-user-round",
        category: Category::Info,
        options: &[OptionsType::of::<DashboardConfig>()],
        representations: Representations {
            widget: reading(SMALL_AND_MEDIUM, modules::user::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::USER],
    },
    ModuleDescriptor {
        id: "utilities",
        name: "Utilities",
        icon: "wrench",
        category: Category::Shell,
        options: &[
            OptionsType::of::<UtilitiesConfig>(),
            OptionsType::of::<RecorderConfig>(),
            OptionsType::of::<PathsConfig>(),
        ],
        representations: Representations {
            chip: Some(ChipDef::new(modules::utilities::utilities_chip, Input::ReadOnly).square()),
            panel: Some(PanelDef::new(
                modules::utilities::utilities_panel,
                Input::Interactive,
            )),
            ..Representations::NONE
        },
        actions: &[
            action("screenshot", "screenshot region"),
            action("record", "record toggle"),
            action("nightlight", "nightlight toggle"),
        ],
        sources: &[],
    },
    ModuleDescriptor {
        id: "visualiser",
        name: "Visualiser",
        icon: "audio-lines",
        category: Category::Media,
        options: &[OptionsType::of::<VisualiserConfig>()],
        representations: Representations {
            widget: reading(EVERY_SIZE, modules::visualiser::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::SPECTRUM],
    },
    ModuleDescriptor {
        id: "volume",
        name: "Volume",
        icon: "volume-2",
        category: Category::Media,
        options: &[OptionsType::of::<AudioConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::volume::volume, volume, VolumeProps),
                    Input::ReadOnly,
                )
                .square()
                .on_press(modules::osd::volume_action)
                .on_scroll(modules::osd::volume_scroll),
            ),
            popout: popout(popouts::volume),
            ..Representations::NONE
        },
        actions: &[
            action("up", "volume up"),
            action("down", "volume down"),
            action("mute", "volume mute"),
        ],
        sources: &[sources::VOLUME],
    },
    ModuleDescriptor {
        id: "weather",
        name: "Weather",
        icon: "cloud-sun",
        category: Category::Info,
        options: &[
            OptionsType::of::<WeatherConfig>(),
            OptionsType::of::<TemperatureConfig>(),
        ],
        representations: Representations {
            widget: reading(SMALL_AND_MEDIUM, modules::weather::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::WEATHER],
    },
    ModuleDescriptor {
        id: "windowinfo",
        name: "Window info",
        icon: "app-window",
        category: Category::Windows,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(modules::windowinfo::window_chip, Input::ReadOnly).square()),
            panel: Some(PanelDef::new(
                modules::windowinfo::window_panel,
                Input::Interactive,
            )),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    },
    ModuleDescriptor {
        id: "workspaces",
        name: "Workspaces",
        icon: "layout-grid",
        category: Category::Windows,
        options: &[OptionsType::of::<WorkspacesConfig>()],
        representations: Representations {
            chip: Some(
                ChipDef::new(
                    rsx!(modules::workspaces::workspaces, workspaces, WorkspacesProps),
                    Input::Interactive,
                )
                .self_managed()
                .on_scroll(modules::workspaces::scroll),
            ),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[sources::WORKSPACE],
    },
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use config::{Config, Edge};
    use telar::{LayoutItem, reset_layout_runtime, set_theme};
    use ui::descriptor::{FieldDef, input_answer};
    use ui::host::{Audience, Instance, Representation, Size};

    use super::*;

    fn descriptor(id: &str) -> &'static ModuleDescriptor {
        ui::descriptor::lookup(MODULES, id).unwrap_or_else(|| panic!("'{id}' is not in the table"))
    }

    const BAR: f32 = 34.0;

    /// The box a representation of `module` is measured in: a bar's strip for a chip, the surface it opens on for the rest.
    fn extent(config: &Config, module: &str, representation: Representation) -> Size {
        match representation {
            Representation::Chip => Size {
                width: 600.0,
                height: BAR,
            },
            Representation::Popout => {
                let (width, height) = config
                    .presentation(module, &toml::Table::new())
                    .popout_size();
                Size { width, height }
            }
            Representation::Widget(size) => size.extent(),
            Representation::Card | Representation::Panel => Size {
                width: 420.0,
                height: 600.0,
            },
        }
    }

    fn host(id: &str, representation: Representation, config: &Arc<Config>) -> Host {
        let theme = config.resolve_theme();
        match representation {
            Representation::Chip => Host::chip(
                Instance::of_module(id),
                Arc::clone(config),
                Edge::Top,
                BAR,
                theme.accent,
                theme.text,
                None,
            ),
            other => ui::preview::surface_host(id, other, extent(config, id, other)),
        }
    }

    /// Builds one representation on a fresh layout runtime and lays it out in its box, answering where — if anywhere — it acts on the pointer.
    fn built(
        module: &ModuleDescriptor,
        representation: Representation,
    ) -> Result<Option<(f32, f32)>, String> {
        let config = Arc::new(Config::starter());
        reset_layout_runtime();
        set_theme(config.resolve_theme());
        // Scoped and disposed before the next reset, or its effects run on against layout ids the next build reuses.
        let scope = telar::owner_scope();
        let owner = scope.id();
        let host = host(module.id, representation, &config);
        let targets = (|| {
            // Under a batch, as a surface builds: the runner holds one open, so an effect a build creates runs after the build rather than inside it.
            let item = telar::batch(|| module.build(&host))
                .ok_or_else(|| "declared but not built".to_string())?
                .map_err(|e| e.to_string())?;
            let Size { width, height } = extent(&config, module.id, representation);
            input_answer(item, width, height).map_err(|e| e.to_string())
        })();
        drop(scope);
        telar::dispose_owner(owner);
        targets
    }

    #[test]
    fn every_descriptor_builds_each_representation_it_declares() {
        telar::set_locale("en");
        let mut failed = Vec::new();
        for module in MODULES {
            for representation in module.declared() {
                if let Err(e) = built(module, representation) {
                    failed.push(format!("{}::{representation:?} — {e}", module.id));
                }
            }
        }
        assert!(
            failed.is_empty(),
            "{} representation(s) did not build:\n  {}",
            failed.len(),
            failed.join("\n  ")
        );
    }

    /// `ReadOnly` is what lets a representation be placed where no input may reach it — the lock screen — so it is checked against what the build actually does rather than trusted: any press, drag, wheel or hover target in its tree fails it.
    #[test]
    fn a_read_only_representation_answers_no_input() {
        telar::set_locale("en");
        let mut interactive = Vec::new();
        for module in MODULES {
            for representation in module.declared() {
                if module.input(representation) != Some(Input::ReadOnly) {
                    continue;
                }
                match built(module, representation) {
                    Ok(None) => {}
                    Ok(Some((x, y))) => interactive.push(format!(
                        "{}::{representation:?} answers the pointer at {x}x{y}",
                        module.id
                    )),
                    Err(e) => interactive.push(format!("{}::{representation:?} — {e}", module.id)),
                }
            }
        }
        assert!(
            interactive.is_empty(),
            "declared ReadOnly, yet:\n  {}",
            interactive.join("\n  ")
        );
    }

    /// A widget is placed by grid lines, spanning its size's footprint from the top-left cell, and has to fill that box: the area it was given is laid out to the footprint exactly, the widget sits inside it, and nothing it draws collapses.
    #[test]
    fn every_widget_size_lays_out_on_its_grid_footprint() {
        use telar::{
            AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle, TemplateTrack,
            compute_layout, new_container, track_layout,
        };
        use ui::host::{GRID_CELL, GRID_GAP};

        telar::set_locale("en");
        let config = Arc::new(Config::starter());
        let grid = WidgetSize::L.footprint();
        let area = WidgetSize::L.extent();
        let mut wrong = Vec::new();
        for module in MODULES {
            let Some(widget) = module.representations.widget else {
                continue;
            };
            for &size in widget.sizes {
                reset_layout_runtime();
                set_theme(config.resolve_theme());
                let scope = telar::owner_scope();
                let owner = scope.id();
                let representation = Representation::Widget(size);
                let host = host(module.id, representation, &config);
                let footprint = size.footprint();
                let measured = (|| -> Result<Vec<String>, telar::LayoutError> {
                    let item = telar::batch(|| module.build(&host)).expect("declared")?;
                    let item_node = item.layout_node();
                    let placed = Container::new(
                        LayoutStyle::new()
                            .flex_column()
                            .grid_column(1, footprint.columns)
                            .grid_row(1, footprint.rows),
                        vec![item],
                    )?;
                    let placed_node = placed.layout_node();
                    let desktop = || {
                        LayoutStyle::new()
                            .display_grid()
                            .grid_template_columns(vec![TemplateTrack::repeat(
                                grid.columns,
                                TemplateTrack::px(GRID_CELL),
                            )])
                            .grid_auto_rows(vec![TemplateTrack::px(GRID_CELL)])
                            .gap(GRID_GAP)
                            .width(area.width)
                            .height(area.height)
                    };
                    let root = new_container(desktop(), &[placed_node])?;
                    let tree =
                        ComponentList::new(Container::new(desktop(), vec![Box::new(placed)])?);
                    compute_layout(
                        root,
                        AvailableSpace::Definite(area.width),
                        AvailableSpace::Definite(area.height),
                    )?;
                    let extent = size.extent();
                    let rect = |node| {
                        track_layout(node)
                            .map(|rect| rect.get())
                            .unwrap_or_default()
                    };
                    let (given, drawn) = (rect(placed_node), rect(item_node));
                    let mut faults = Vec::new();
                    if (given.width - extent.width).abs() > 0.5
                        || (given.height - extent.height).abs() > 0.5
                    {
                        faults.push(format!(
                            "given {}x{}, not its {}x{} footprint",
                            given.width, given.height, extent.width, extent.height
                        ));
                    }
                    if drawn.width < 0.5
                        || drawn.height < 0.5
                        || drawn.width > extent.width + 0.5
                        || drawn.height > extent.height + 0.5
                    {
                        faults.push(format!(
                            "laid out {}x{} in its {}x{} footprint",
                            drawn.width, drawn.height, extent.width, extent.height
                        ));
                    }
                    for command in tree.commands().iter() {
                        let rect = match command {
                            DrawCommand::Rect { rect, .. } | DrawCommand::Image { rect, .. } => {
                                *rect
                            }
                            DrawCommand::Text { rect, text, .. } if !text.is_empty() => *rect,
                            _ => continue,
                        };
                        if rect.width < 0.5 || rect.height < 0.5 {
                            faults.push(format!(
                                "a draw collapsed to {}x{}",
                                rect.width, rect.height
                            ));
                        }
                    }
                    Ok(faults)
                })();
                drop(scope);
                telar::dispose_owner(owner);
                match measured {
                    Ok(faults) => wrong.extend(
                        faults
                            .into_iter()
                            .map(|fault| format!("{}::{size:?} — {fault}", module.id)),
                    ),
                    Err(e) => wrong.push(format!("{}::{size:?} — {e}", module.id)),
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "widgets off their footprint:\n  {}",
            wrong.join("\n  ")
        );
    }

    /// The desktop's areas draw nothing themselves — they place what is in them — so with the table installed a grid holding the clock builds at every anchor and a dock holding the visualiser builds on every edge.
    #[test]
    fn a_desktop_grid_and_dock_build_their_widgets_through_the_table() {
        telar::set_locale("en");
        ui::descriptor::install(MODULES);
        let config = Arc::new(Config::starter());
        let mut failed = Vec::new();
        for (anchor, edge) in layout::Anchor::ALL
            .into_iter()
            .zip(Edge::ALL.into_iter().cycle())
        {
            reset_layout_runtime();
            set_theme(config.resolve_theme());
            let scope = telar::owner_scope();
            let owner = scope.id();
            let surround = surfaces::area::Surround {
                config: &config,
                theme: config.resolve_theme(),
                output: None,
                layer: layout::LayerKind::Desktop,
                bounds: telar::Rect::new(0.0, 0.0, 1920.0, 1080.0),
                reserved: surfaces::layer_window::Reserved::default(),
                audience: Audience::Owner,
            };
            for area in [
                desktop_grid(anchor, "clock"),
                desktop_dock(edge, "visualiser"),
            ] {
                match surfaces::area::build(&area, surround) {
                    Some(Err(e)) => {
                        failed.push(format!("{} at {anchor:?}/{edge:?} — {e}", area.id))
                    }
                    Some(Ok(_)) => {}
                    None => failed.push(format!("nothing draws a {} area", area.kind.name())),
                }
            }
            drop(scope);
            telar::dispose_owner(owner);
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    fn desktop_grid(anchor: layout::Anchor, module: &str) -> layout::ResolvedArea {
        desktop_area(
            layout::ResolvedAreaKind::Grid {
                rect: layout::Rect::default(),
                cell: ui::host::GRID_CELL,
                gap: ui::host::GRID_GAP,
                anchor,
            },
            module,
        )
    }

    fn desktop_dock(edge: Edge, module: &str) -> layout::ResolvedArea {
        desktop_area(
            layout::ResolvedAreaKind::Dock {
                edge,
                thickness: 120.0,
            },
            module,
        )
    }

    fn desktop_area(kind: layout::ResolvedAreaKind, module: &str) -> layout::ResolvedArea {
        layout::ResolvedArea {
            id: layout::AreaId::new(module),
            kind,
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Usable,
            style: layout::Style::default(),
            visible: None,
            actions: Default::default(),
            groups: vec![layout::ResolvedGroup {
                id: layout::GroupId::new(module),
                kind: layout::GroupKind::Zone {
                    zone: layout::Zone::Center,
                },
                arrange: None,
                cols: layout::Arrange::TRACKS,
                rows: layout::Arrange::TRACKS,
                gap: None,
                repeat: None,
                komponent: None,
                style: layout::Style::default(),
                children: vec![layout::ResolvedInstance {
                    id: layout::InstanceId::new(module),
                    module: module.to_string(),
                    representation: layout::Representation::WidgetL,
                    options: toml::Table::new(),
                    bindings: Default::default(),
                    style: layout::Style::default(),
                    placement: None,
                    actions: Default::default(),
                }],
            }],
        }
    }

    /// What a reading draws is what its sources declare, each field marked public or private, so a module offering one says what that is.
    #[test]
    fn every_module_with_a_reading_declares_its_sources() {
        let mut silent = Vec::new();
        for module in MODULES {
            let reads = module
                .representations
                .widget
                .is_some_and(|widget| widget.input == Input::ReadOnly);
            if reads && module.sources.iter().all(|source| source.fields.is_empty()) {
                silent.push(module.id);
            }
        }
        assert!(
            silent.is_empty(),
            "readings with no declared source: {silent:?}"
        );
    }

    /// A reading built for anyone in front of the screen — the lock layer — builds like one built for the user, with every service absent.
    #[test]
    fn every_reading_builds_for_anyone_with_every_service_absent() {
        telar::set_locale("en");
        let config = Arc::new(Config::starter());
        let mut failed = Vec::new();
        for module in MODULES {
            for representation in module.declared() {
                if module.input(representation) != Some(Input::ReadOnly)
                    || !matches!(representation, Representation::Widget(_))
                {
                    continue;
                }
                reset_layout_runtime();
                set_theme(config.resolve_theme());
                let scope = telar::owner_scope();
                let owner = scope.id();
                let host = host(module.id, representation, &config).shown_to(Audience::Anyone);
                let built = telar::batch(|| module.build(&host));
                if !matches!(built, Some(Ok(_))) {
                    failed.push(format!("{}::{representation:?}", module.id));
                }
                drop(built);
                drop(scope);
                telar::dispose_owner(owner);
            }
        }
        assert!(failed.is_empty(), "did not build for anyone: {failed:?}");
    }

    /// The README's module list is the table's, id for id, and no id is declared twice.
    #[test]
    fn the_table_holds_each_module_the_readme_lists_once() {
        let ids: Vec<&str> = MODULES.iter().map(|module| module.id).collect();
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "an id is declared twice: {ids:?}");

        let readme = include_str!("../../../../README.md");
        let listed: BTreeSet<&str> = readme
            .split("**Modules.**")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .expect("the README lists the modules")
            .split('`')
            .skip(1)
            .step_by(2)
            .collect();
        assert_eq!(
            unique, listed,
            "the table and README.md list different modules"
        );
    }

    /// An action is an IPC line, so `--list` and anything offering a module's verbs enumerate the same commands. Resolved, never run.
    #[test]
    fn every_action_is_a_command_the_shell_answers() {
        for module in MODULES {
            for action in module.actions {
                assert!(
                    crate::core::commands::resolves(action.command),
                    "{}'s '{}' runs '{}', which is not a command the shell answers",
                    module.id,
                    action.id,
                    action.command
                );
            }
        }
    }

    /// A module's actions are rows of its context menu (T-6.4), so each one has a name there in every language the shell speaks.
    #[test]
    fn every_module_action_is_named_in_the_context_menu() {
        for module in MODULES {
            for action in module.actions {
                assert!(
                    editor::context::names_action(action.id),
                    "{}'s action `{}` has no `editor.action.{}` label",
                    module.id,
                    action.id,
                    action.id
                );
            }
        }
    }

    #[test]
    fn every_options_type_is_a_section_the_schema_documents_named_once() {
        for module in MODULES {
            let mut seen = BTreeSet::new();
            for options in module.options {
                assert!(
                    config::schema::outline(Some(options.section)).is_ok(),
                    "{} reads `[{}]`, which the schema does not document",
                    module.id,
                    options.section
                );
                assert!(
                    seen.insert(options.section),
                    "{} names `[{}]` twice",
                    module.id,
                    options.section
                );
            }
        }
    }

    /// The inspector (T-6.5) draws a control per option an instance can set, read off the same declaration the schema prints: so every option of every module is typed, every enum lists what it may be, and no module declares a key its presentation already has, which one instance option could not tell apart.
    #[test]
    fn every_module_option_carries_a_control_the_inspector_can_draw() {
        use config::fields::Control;
        fn untyped(field: &config::fields::OptionField) -> Option<String> {
            match &field.control {
                Control::Unknown(declared) => Some(format!("{} ({declared})", field.key)),
                Control::Enum(variants) if variants.is_empty() => {
                    Some(format!("{} lists no variants", field.key))
                }
                Control::List(element) | Control::Map(element) => match element.as_ref() {
                    Control::Unknown(declared) => Some(format!("{} ({declared})", field.key)),
                    _ => None,
                },
                Control::Table(inner) => inner.iter().find_map(untyped),
                _ => None,
            }
        }
        let presentation: BTreeSet<String> = config::fields::presentation()
            .into_iter()
            .map(|field| field.key)
            .collect();
        let mut typed = 0;
        for module in MODULES {
            let fields = module.option_fields();
            for field in &fields {
                typed += 1;
                assert_eq!(untyped(field), None, "`{}`", module.id);
            }
            if let Some(own) = module.options.first() {
                let clashes: Vec<String> = config::fields::section(own.section)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|field| field.key)
                    .filter(|key| presentation.contains(key))
                    .collect();
                assert!(
                    clashes.is_empty(),
                    "`[{}]` declares {clashes:?}, which every module's presentation already has",
                    own.section
                );
            }
        }
        assert!(typed > 100, "only {typed} options were typed");

        let keys = |id: &str| -> Vec<String> {
            descriptor(id)
                .option_fields()
                .into_iter()
                .map(|field| field.key)
                .collect()
        };
        let clock = keys("clock");
        assert!(clock.contains(&"face.scale".to_string()), "{clock:?}");
        assert!(
            clock.contains(&"drawer_width".to_string()),
            "a panel's size: {clock:?}"
        );
        assert!(keys("notifications").contains(&"sidebar_size".to_string()));
        let volume = keys("volume");
        assert!(
            volume.contains(&"popout_max_height".to_string()),
            "{volume:?}"
        );
        let variants = descriptor("workspaces")
            .option_fields()
            .into_iter()
            .find(|field| field.key == "capitalize")
            .map(|field| field.control);
        assert_eq!(
            variants,
            Some(Control::Enum(vec!["none", "upper", "lower", "title"]))
        );
    }

    /// T-6.5: an instance's popover builds a row for every option its module lets it set, whatever the option's control — a list or a table of them included.
    #[test]
    fn the_inspector_builds_a_row_for_every_option_of_every_module() {
        use editor::popover::{self, InstanceDraft};
        use editor::written::Written;
        use layout::{AreaId, GroupId, InstanceId, LayerKind, ResolvedInstance};
        use surfaces::rects::Node;

        let config = Config::starter();
        reset_layout_runtime();
        set_theme(config.resolve_theme());
        telar::set_locale("en");
        ui::descriptor::install(MODULES);
        let scope = telar::owner_scope();
        let owner = scope.id();
        let area = AreaId::new("inspected");
        let group = GroupId::new("group");
        let written = Written::area(
            &layout::built_in(),
            Some("DP-1"),
            LayerKind::Desktop,
            &area,
            None,
        )
        .expect("the built-in layout covers every screen");
        let edit = editor::session::Edit::new("Inspect");
        let mut built = 0;
        for module in MODULES {
            let id = InstanceId::new(module.id);
            let draft = InstanceDraft::new(
                &edit,
                Node::area(Some("DP-1"), LayerKind::Desktop, &area).instance(&group, &id),
                ResolvedInstance {
                    id: id.clone(),
                    module: module.id.to_string(),
                    representation: layout::Representation::WidgetM,
                    options: toml::Table::new(),
                    bindings: Default::default(),
                    style: layout::Style::default(),
                    placement: None,
                    actions: Default::default(),
                },
                "grid",
                popover::shown(&config, module.id, &toml::Table::new()),
                written.instance(&group, &id),
                None,
            );
            for field in module.option_fields() {
                let row =
                    telar::batch(|| popover::option(&draft, popover::path_of(&field.key), &field));
                assert!(
                    row.is_ok(),
                    "`{}` `{}`: {:?}",
                    module.id,
                    field.key,
                    row.err()
                );
                built += 1;
            }
        }
        drop(scope);
        telar::dispose_owner(owner);
        assert!(built > 100, "only {built} rows were built");
    }

    /// `$source.field` names one reading: two modules giving a source the same id, a source two fields of one name, or a source called `event` (where event readings live) would each make a name mean two things.
    #[test]
    fn every_source_and_field_has_a_name_of_its_own() {
        let mut ids = BTreeSet::new();
        for source in MODULES.iter().flat_map(|module| module.sources.iter()) {
            assert!(
                ids.insert(source.id),
                "two sources are called `{}`",
                source.id
            );
            assert_ne!(source.id, automation::EVENT);
            let mut fields = BTreeSet::new();
            for field in source.fields {
                assert!(
                    fields.insert(field.name),
                    "`{}` has two fields called `{}`",
                    source.id,
                    field.name
                );
            }
        }
        for expected in ["battery", "power", "clock", "media", "workspace", "cpu"] {
            assert!(ids.contains(expected), "no `{expected}` source");
        }
    }

    /// Every module reading can be named in an expression and is checked to the type it declares.
    #[test]
    fn every_module_reading_compiles_with_its_declared_type() {
        let env = automation::Environment::of_modules(MODULES, automation::UserSources::default());
        for source in MODULES.iter().flat_map(|module| module.sources.iter()) {
            for field in source.fields {
                let written = format!("${}.{}", source.id, field.name);
                let compiled = env
                    .compile(&written)
                    .unwrap_or_else(|errors| panic!("`{written}`: {errors}"));
                assert_eq!(*compiled.ty(), field.ty.ty(), "{written}");
            }
        }
    }

    /// Notification text is the one reading that must never reach a screen anyone can read, whatever `[lock]` asks for; which applications are waiting is the user's to allow, and how many is public either way.
    #[test]
    fn notification_text_is_private_at_every_setting_and_its_apps_are_the_user_s_to_allow() {
        let fields: Vec<FieldDef> = descriptor("notifications")
            .sources
            .iter()
            .flat_map(|source| source.fields.iter().copied())
            .collect();
        let privacy = |name: &str| {
            fields
                .iter()
                .find(|field| field.name == name)
                .unwrap_or_else(|| panic!("no '{name}' field"))
                .privacy
        };
        for detail in [
            config::NotificationDetail::Count,
            config::NotificationDetail::Apps,
        ] {
            let lock = config::LockConfig {
                notification_detail: detail,
                ..config::LockConfig::default()
            };
            assert!(!privacy("summary").allows_anyone(&lock), "{detail:?}");
            assert!(!privacy("body").allows_anyone(&lock), "{detail:?}");
            assert!(privacy("count").allows_anyone(&lock), "{detail:?}");
            assert_eq!(
                privacy("apps").allows_anyone(&lock),
                detail == config::NotificationDetail::Apps,
                "{detail:?}"
            );
        }
    }

    /// What is playing is named on a locked screen by default, as it has been since before the lock was a layer, and reduces to playing-or-paused where the user asks for that.
    #[test]
    fn what_is_playing_is_named_only_under_the_media_detail_the_user_chose() {
        let fields: Vec<FieldDef> = descriptor("media")
            .sources
            .iter()
            .flat_map(|source| source.fields.iter().copied())
            .collect();
        let privacy = |name: &str| {
            fields
                .iter()
                .find(|field| field.name == name)
                .unwrap_or_else(|| panic!("no '{name}' field"))
                .privacy
        };
        for (detail, named) in [
            (config::MediaDetail::Title, true),
            (config::MediaDetail::State, false),
        ] {
            let lock = config::LockConfig {
                media_detail: detail,
                ..config::LockConfig::default()
            };
            for field in ["title", "artist", "art"] {
                assert_eq!(
                    privacy(field).allows_anyone(&lock),
                    named,
                    "{field} under {detail:?}"
                );
            }
            assert!(
                privacy("playing").allows_anyone(&lock),
                "a pause symbol names nothing, so it is public either way"
            );
        }
    }

    #[test]
    fn only_panels_that_read_keys_ask_for_the_keyboard() {
        let wants = |id: &str| {
            descriptor(id)
                .representations
                .panel
                .expect("a panel")
                .keyboard
                != KeyboardMode::None
        };
        assert!(wants("notes"), "notes are edited in place");
        assert!(wants("settings"), "settings has text fields");
        assert!(
            wants("session"),
            "the session tiles are arrow-navigable, which is the other reason to want the keyboard"
        );
        for display_only in [
            "clock",
            "dashboard",
            "battery",
            "bluetooth",
            "network",
            "notifications",
        ] {
            assert!(
                !wants(display_only),
                "'{display_only}' only shows readings; taking keyboard focus from the window would make the \
                 compositor re-focus it on close, moving the viewport under a focus-following layout"
            );
        }
    }

    /// A reading shown in the `statusicons` cluster and a reading shown as its own chip are the same thing under the same name, so a user moving one between the two does not have to rename it.
    #[test]
    fn a_cluster_icon_and_its_own_chip_share_one_name() {
        use modules::statusicons::StatusIcon;
        for name in ["volume", "mic", "network", "bluetooth", "battery"] {
            assert!(StatusIcon::from_id(name).is_some(), "'{name}' is an icon");
            assert!(
                ui::descriptor::lookup(MODULES, name).is_some(),
                "'{name}' is also a module id"
            );
        }
        assert!(
            ui::descriptor::lookup(MODULES, "wifi").is_none(),
            "`wifi` is a cluster-only reading: the `network` chip already covers being online over any link"
        );
    }

    #[test]
    fn the_chips_carry_the_roles_their_modules_play() {
        let chip = |id: &str| descriptor(id).representations.chip.expect("a chip");
        assert_eq!(
            chip("spacer").frame,
            ui::descriptor::ChipFrame::Filler,
            "a gap gets no chip shell, padding, hover state or surface"
        );
        for self_managed in ["workspaces", "tray", "lockstatus"] {
            assert!(
                chip(self_managed).is_bare(),
                "'{self_managed}' lays itself out"
            );
        }
        assert!(
            chip("tray").press.is_none() && chip("tray").scroll.is_none(),
            "a single chip-level handler would act on the row, not on the application clicked"
        );
        assert!(
            chip("mic").press.is_some() && chip("mic").scroll.is_some(),
            "the mic chip mutes on click and adjusts on scroll, like the volume chip"
        );
        for readout in ["cpu", "memory", "temperature", "netspeed"] {
            assert!(
                chip(readout).press.is_none(),
                "'{readout}' is a readout, not a control"
            );
            assert!(descriptor(readout).representations.panel.is_none());
        }
        assert!(chip("logo").square, "the logo is a square icon chip");
        assert!(
            descriptor("logo").representations.panel.is_none(),
            "the logo opens the session panel rather than a copy of it"
        );
    }
}
