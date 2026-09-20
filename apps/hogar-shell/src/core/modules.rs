//! Every module the shell ships, one descriptor each: the only place that knows both the descriptor vocabulary and the modules themselves.

use platform_wayland::KeyboardMode;
use telar::Children;

use config::{
    ActiveWindowConfig, AudioConfig, BatteryConfig, BluetoothConfig, BrightnessConfig, ClockConfig,
    DashboardConfig, GeneralConfig, GpuConfig, LauncherConfig, LockStatusConfig, MediaConfig,
    NetworkConfig, NotificationsConfig, PathsConfig, RecorderConfig, StackConfig,
    StatusIconsConfig, TemperatureConfig, TrayConfig, UtilitiesConfig, VisualiserConfig,
    WeatherConfig, WidgetsConfig, WorkspacesConfig,
};
use ui::descriptor::{
    ActionDef, Built, CardDef, ChipDef, FieldDef, Input, ModuleDescriptor, OptionsType, PanelDef,
    Privacy, Representations, SourceDef, WidgetDef,
};
use ui::host::{Host, WidgetSize};

use modules::dashboard::cards;
use modules::popout_cards as popouts;

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

const fn public(name: &'static str) -> FieldDef {
    FieldDef {
        name,
        privacy: Privacy::Public,
    }
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

fn settings_panel(_host: &Host) -> Built {
    settings::panel::settings_panel()
}

pub static MODULES: &[ModuleDescriptor] = &[
    ModuleDescriptor {
        id: "activewindow",
        name: "Active window",
        icon: "app-window",
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
        sources: &[SourceDef {
            id: "battery",
            fields: &[public("level"), public("charging")],
        }],
    },
    ModuleDescriptor {
        id: "bluetooth",
        name: "Bluetooth",
        icon: "bluetooth",
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
        options: &[
            OptionsType::of::<ClockConfig>(),
            OptionsType::of::<WidgetsConfig>(),
        ],
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
        sources: &[SourceDef {
            id: "clock",
            fields: &[public("time"), public("date")],
        }],
    },
    ModuleDescriptor {
        id: "cpu",
        name: "CPU",
        icon: "cpu",
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
        sources: &[SourceDef {
            id: "cpu",
            fields: &[public("usage"), public("frequency")],
        }],
    },
    ModuleDescriptor {
        id: "dashboard",
        name: "Dashboard",
        icon: "layout-dashboard",
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
        sources: &[SourceDef {
            id: "gpu",
            fields: &[public("usage"), public("vram")],
        }],
    },
    ModuleDescriptor {
        id: "kblayout",
        name: "Keyboard layout",
        icon: "keyboard",
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
        sources: &[SourceDef {
            id: "media",
            fields: &[
                public("title"),
                public("artist"),
                public("art"),
                public("playing"),
            ],
        }],
    },
    ModuleDescriptor {
        id: "memory",
        name: "Memory",
        icon: "memory-stick",
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
        sources: &[SourceDef {
            id: "memory",
            fields: &[public("used"), public("total")],
        }],
    },
    ModuleDescriptor {
        id: "mic",
        name: "Microphone",
        icon: "mic",
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
        sources: &[SourceDef {
            id: "netspeed",
            fields: &[public("down"), public("up")],
        }],
    },
    ModuleDescriptor {
        id: "network",
        name: "Network",
        icon: "wifi",
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
        sources: &[SourceDef {
            id: "temperature",
            fields: &[public("celsius"), public("sensor")],
        }],
    },
    // Self-managed: one pressable box per application, each with its own click, middle-click, right-click and scroll — a single chip shell around the row could carry none of that.
    ModuleDescriptor {
        id: "tray",
        name: "Tray",
        icon: "panel-bottom",
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
        options: &[OptionsType::of::<DashboardConfig>()],
        representations: Representations {
            widget: reading(SMALL_AND_MEDIUM, modules::user::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[SourceDef {
            id: "user",
            fields: &[public("name"), public("avatar")],
        }],
    },
    ModuleDescriptor {
        id: "utilities",
        name: "Utilities",
        icon: "wrench",
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
        options: &[
            OptionsType::of::<VisualiserConfig>(),
            OptionsType::of::<WidgetsConfig>(),
        ],
        representations: Representations {
            widget: reading(EVERY_SIZE, modules::visualiser::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[SourceDef {
            id: "spectrum",
            fields: &[public("silent")],
        }],
    },
    ModuleDescriptor {
        id: "volume",
        name: "Volume",
        icon: "volume-2",
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
        sources: &[],
    },
    ModuleDescriptor {
        id: "weather",
        name: "Weather",
        icon: "cloud-sun",
        options: &[
            OptionsType::of::<WeatherConfig>(),
            OptionsType::of::<TemperatureConfig>(),
        ],
        representations: Representations {
            widget: reading(SMALL_AND_MEDIUM, modules::weather::widget),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[SourceDef {
            id: "weather",
            fields: &[public("place"), public("temperature"), public("condition")],
        }],
    },
    ModuleDescriptor {
        id: "windowinfo",
        name: "Window info",
        icon: "app-window",
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
        sources: &[],
    },
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use config::{Config, Edge};
    use telar::{LayoutItem, reset_layout_runtime, set_theme};
    use ui::descriptor::input_answer;
    use ui::host::{Audience, InstanceId, Representation, Size};

    use super::*;

    fn descriptor(id: &str) -> &'static ModuleDescriptor {
        ui::descriptor::lookup(MODULES, id).unwrap_or_else(|| panic!("'{id}' is not in the table"))
    }

    /// The box a representation is measured in: a bar's strip for a chip, the surface it opens on for the rest.
    fn extent(config: &Config, representation: Representation) -> Size {
        match representation {
            Representation::Chip => Size {
                width: 600.0,
                height: config.bars.top.size as f32,
            },
            Representation::Popout => Size {
                width: config.popouts.card_width(),
                height: config.popouts.card_height(),
            },
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
                InstanceId::of_module(id),
                Arc::clone(config),
                Edge::Top,
                theme.accent,
                theme.text,
                None,
            ),
            other => ui::preview::surface_host(id, other, extent(config, other)),
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
            let Size { width, height } = extent(&config, representation);
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

    /// The desktop surface places the `clock` and `visualiser` widgets and draws neither itself, so with the table installed each layer it is asked for builds, at every position and on every edge.
    #[test]
    fn the_desktop_surface_builds_its_clock_and_visualiser_through_the_table() {
        telar::set_locale("en");
        ui::descriptor::install(MODULES);
        let mut failed = Vec::new();
        for (position, edge) in config::ClockPlacement::ALL
            .into_iter()
            .zip(Edge::ALL.into_iter().cycle())
        {
            let mut config = Config::starter();
            config.widgets.clock.enabled = true;
            config.widgets.clock.position = position;
            config.widgets.visualiser.enabled = true;
            config.widgets.visualiser.edge = edge;
            reset_layout_runtime();
            set_theme(config.resolve_theme());
            let scope = telar::owner_scope();
            let owner = scope.id();
            for (module, layer) in surfaces::widgets::layers(&Arc::new(config)) {
                if let Err(e) = layer {
                    failed.push(format!("{module} at {position:?}/{edge:?} — {e}"));
                }
            }
            drop(scope);
            telar::dispose_owner(owner);
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
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

    /// Notification text is the one reading that must never reach a screen anyone can read.
    #[test]
    fn notification_text_is_private_and_its_count_and_apps_are_public() {
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
        assert_eq!(privacy("summary"), Privacy::Private);
        assert_eq!(privacy("body"), Privacy::Private);
        assert_eq!(privacy("count"), Privacy::Public);
        assert_eq!(privacy("apps"), Privacy::Public);
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
