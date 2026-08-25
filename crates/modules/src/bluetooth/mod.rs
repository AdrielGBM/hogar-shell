//! The Bluetooth chip and the device list it opens.
//!
//! One panel does the whole job a user has with Bluetooth: turn the radio on, look for something new, connect or disconnect what is listed, and forget what they are done with. Everything it shows comes from the shared [`bluetooth`](services::bluetooth) service, so the chip, the cluster icon, the popout card and this panel are four views of one subscription rather than four readers of the bus.

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ReactiveList,
    RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};
use ui::scale::space;

use config::BluetoothConfig;
use config::theme::{FontRole, NordTheme};
use services::bluetooth::{self, Bluetooth, Device};
use ui::glyph;
use ui::icon::icon_view;
use ui::module::{icon_px, module_fg, surface_env};

const ROW_ICON: f32 = 22.0;
const ROW_RADIUS: f32 = 8.0;

/// The bar chip: the radio's state in one glyph, opening the device panel on click.
pub fn chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let accent = theme.accent;
    // The `Copy` summary, not the whole state: the tint closure also reads the foreground signal, and a `with` over the state would still be holding the reactive runtime's borrow when it did.
    let state = signal(
        bluetooth::current()
            .map(|bt| bt.status())
            .unwrap_or_default(),
    );
    let sink = state.clone();
    platform_wayland::watch(bluetooth::subscribe, move |bt| sink.set(bt.status()));

    let fg = module_fg();
    let glyph_state = state.read_only();
    let tint_state = state.read_only();
    icon_view(
        move || glyph::bluetooth(glyph_state.get()).to_string(),
        move || glyph::bluetooth_tint(tint_state.get(), theme, accent, fg.get()),
        icon_px(),
    )
}

/// The panel: the adapter's controls over the devices it knows about.
pub fn bluetooth_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = surface_env()
        .map(|env| env.config.bluetooth)
        .unwrap_or_default();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    bluetooth_view(config)
}

/// The panel's whole content, taking its config rather than reading the surface's, so a caller that already resolved one — a drawer, a float — does not have to be a surface for this to build.
pub fn bluetooth_view(config: BluetoothConfig) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();

    let state = signal(bluetooth::current().unwrap_or_default());
    let sink = state.clone();
    platform_wayland::watch(bluetooth::subscribe, move |bt| sink.set(bt));

    // Opening the panel is the gesture that means "find me a device", so it is also what starts looking. The scan stops itself; see `bluetooth::set_discovering`.
    if config.scan_on_open && state.peek().powered {
        bluetooth::set_discovering(true);
    }

    let armed = signal(String::new());
    let children = vec![
        header(state.clone(), theme)?,
        list(state.clone(), armed, config, theme)?,
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

fn header(
    state: RwSignal<Bluetooth>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // One handle per closure: a signal is not `Copy`, and each reader below outlives the others.
    let subtitle_state = state.read_only();
    let power_label = state.read_only();
    let power_active = state.read_only();
    let scan_label = state.read_only();
    let scan_active = state.read_only();

    // Read out, then translate: `adapter_line` calls `t!`, and a `with` here would still hold the reactive runtime's borrow when it read the locale signal.
    let subtitle = Text::auto(
        move || adapter_line(&subtitle_state.get()),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let title = Text::auto(
        || telar::t!("bluetooth.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(title), box_item(subtitle)],
    )?;

    let power = pill(
        move || {
            if power_label.get().powered {
                telar::t!("bluetooth.on")
            } else {
                telar::t!("bluetooth.off")
            }
        },
        move || power_active.get().powered,
        bluetooth::toggle_powered,
        theme,
    )?;
    let scan = pill(
        move || {
            if scan_label.get().discovering {
                telar::t!("bluetooth.stop")
            } else {
                telar::t!("bluetooth.scan")
            }
        },
        move || scan_active.get().discovering,
        bluetooth::toggle_discovering,
        theme,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(labels), power, scan],
    )?))
}

/// What the adapter itself is doing, under the title: its name, whether it is scanning, and how much is connected — the three facts that decide what the list below means.
fn adapter_line(bt: &Bluetooth) -> String {
    if !bt.available {
        return telar::t!("bluetooth.no_adapter");
    }
    if !bt.powered {
        return telar::t!("bluetooth.off");
    }
    let connected = bt.connected_count();
    if bt.discovering {
        telar::t!("bluetooth.scanning")
    } else if connected > 0 {
        telar::t!("bluetooth.connected_count", count = connected.to_string())
    } else if bt.adapter.trim().is_empty() {
        telar::t!("bluetooth.on")
    } else {
        bt.adapter.clone()
    }
}

/// The devices worth listing, in the service's order: unnamed ones are dropped unless asked for (a scan in a public place turns up dozens of bare addresses), and the rest are capped.
fn listed(bt: &Bluetooth, config: BluetoothConfig) -> Vec<Device> {
    bt.devices
        .iter()
        .filter(|d| config.show_unnamed || !d.name.trim().is_empty())
        .take(config.device_limit())
        .cloned()
        .collect()
}

fn list(
    state: RwSignal<Bluetooth>,
    armed: RwSignal<String>,
    config: BluetoothConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = state.read_only();
    let empty_state = state.read_only();
    let rows = ReactiveList::new(
        move || listed(&source.get(), config),
        // Keyed on what the row draws, not on the device's identity: a headset keeps its path while it connects, gains a battery reading and changes its subtitle, and a row keyed on the path alone would still be showing "Paired" long after it came up.
        |d: &Device| row_key(d),
        {
            let armed = armed.clone();
            move |device: Device| row(device, armed.clone(), theme)
        },
        6.0,
    )?;

    let empty = Text::auto(
        move || empty_line(&empty_state.get(), config),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(rows), box_item(empty)],
    )?))
}

fn row_key(d: &Device) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        d.path,
        d.connected,
        d.paired,
        d.battery.unwrap_or(0),
        d.label()
    )
}

/// The line under an empty list — never blank, so the panel always says why there is nothing to choose from.
fn empty_line(bt: &Bluetooth, config: BluetoothConfig) -> String {
    if !bt.available {
        telar::t!("bluetooth.no_adapter")
    } else if !bt.powered {
        telar::t!("bluetooth.turn_on")
    } else if listed(bt, config).is_empty() {
        telar::t!("bluetooth.no_devices")
    } else {
        String::new()
    }
}

/// One device: press to connect or disconnect, right-click to forget.
///
/// Forgetting arms first, like the session menu's destructive tiles. A pairing is a key exchange the other device also has to be told about — undoing a mis-click means putting the headset back in pairing mode — so it is not something a stray right-click should be able to do.
fn row(
    device: Device,
    armed: RwSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let path = device.path.clone();
    let label = device.label();
    let connected = device.connected;

    let armed_text = armed.read_only();
    let armed_tint = armed.read_only();
    let armed_fill = armed.read_only();
    let armed_hover = armed.read_only();
    let is_armed = {
        let path = path.clone();
        move |signal: &telar::ReadSignal<String>| signal.get() == path
    };

    let icon = icon_view(
        {
            let kind = device.icon.clone();
            move || glyph::bluetooth_device(&kind).to_string()
        },
        move || if connected { theme.accent } else { theme.text },
        ROW_ICON,
    )?;

    let name = Text::auto(
        move || label.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Body, theme.text),
    )?;
    let status = Text::auto(
        {
            let device = device.clone();
            let is_armed = is_armed.clone();
            move || {
                if is_armed(&armed_text) {
                    telar::t!("bluetooth.forget_confirm")
                } else {
                    status_line(&device)
                }
            }
        },
        LayoutStyle::new(),
        {
            let is_armed = is_armed.clone();
            move || {
                let tint = if is_armed(&armed_tint) {
                    theme.red
                } else {
                    theme.subtle
                };
                theme.text_style(FontRole::Caption, tint)
            }
        },
    )?;
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(name), box_item(status)],
    )?;

    let trailing = Text::auto(
        {
            let device = device.clone();
            move || trailing_line(&device)
        },
        LayoutStyle::new().flex_shrink(0.0),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    let paired = device.paired;
    let press_path = path.clone();
    let alt_path = path.clone();
    let disarm = armed.clone();
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .gap(space::lg())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        {
            let is_armed = is_armed.clone();
            move |_| {
                let fill = if is_armed(&armed_fill) {
                    theme.red
                } else {
                    theme.base
                };
                RectStyle::filled(fill, ROW_RADIUS)
            }
        },
        vec![icon, Box::new(labels), box_item(trailing)],
    )?
    .hover_style({
        let is_armed = is_armed.clone();
        move |_| {
            let fill = if is_armed(&armed_hover) {
                theme.red
            } else {
                theme.overlay
            };
            RectStyle::filled(fill, ROW_RADIUS)
        }
    })
    .on_press(move || {
        // A press on an armed row is a change of mind about forgetting it, not a connection.
        if disarm.peek() == press_path {
            disarm.set(String::new());
            return;
        }
        bluetooth::toggle_device(&press_path);
    })
    .on_alt_press(move |_button| {
        if !paired {
            return;
        }
        if armed.peek() == alt_path {
            bluetooth::forget(&alt_path);
            armed.set(String::new());
            return;
        }
        // Arming one row disarms any other, so two half-pressed rows can never both be live.
        armed.set(alt_path.clone());
    });
    Ok(Box::new(row))
}

/// The device's own state, most useful fact first: what it is doing now, then what it is to this machine.
fn status_line(device: &Device) -> String {
    if device.connected {
        telar::t!("bluetooth.connected")
    } else if device.paired {
        telar::t!("bluetooth.paired")
    } else {
        telar::t!("bluetooth.available")
    }
}

/// The right-hand column: a battery where the device reports one, else how well it is heard.
fn trailing_line(device: &Device) -> String {
    if let Some(battery) = device.battery {
        return format!("{battery}%");
    }
    match device.rssi {
        Some(rssi) => format!("{rssi} dBm"),
        None => String::new(),
    }
}

fn pill(
    label: impl Fn() -> String + 'static,
    active: impl Fn() -> bool + 'static,
    on_press: fn(),
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let active = std::rc::Rc::new(active);
    let (fill_active, hover_active, text_active) = (active.clone(), active.clone(), active.clone());
    let text = Text::auto(label, LayoutStyle::new(), move || {
        let tint = if text_active() {
            theme.accent.most_readable(&[theme.text, theme.base])
        } else {
            theme.text
        };
        theme.text_style(FontRole::Caption, tint)
    })?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .padding_horizontal(space::lg())
                .padding_vertical(space::sm())
                .flex_shrink(0.0)
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER),
            move |_| {
                let fill = if fill_active() {
                    theme.accent
                } else {
                    theme.base
                };
                RectStyle::filled(fill, ROW_RADIUS)
            },
            vec![box_item(text)],
        )?
        .hover_style(move |_| {
            let fill = if hover_active() {
                theme.accent.darken(0.08)
            } else {
                theme.overlay
            };
            RectStyle::filled(fill, ROW_RADIUS)
        })
        .on_press(on_press),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, paired: bool, battery: Option<u8>) -> Device {
        Device {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.to_string(),
            paired,
            battery,
            ..Device::default()
        }
    }

    #[test]
    fn a_scan_full_of_bare_addresses_does_not_become_the_list() {
        let bt = Bluetooth {
            available: true,
            powered: true,
            devices: vec![
                device("WH-1000", true, None),
                Device {
                    address: "AA:BB:CC:DD:EE:FF".into(),
                    ..Device::default()
                },
            ],
            ..Bluetooth::default()
        };
        let hidden = BluetoothConfig::default();
        assert_eq!(listed(&bt, hidden).len(), 1, "the unnamed one is dropped");

        let shown = BluetoothConfig {
            show_unnamed: true,
            ..hidden
        };
        assert_eq!(listed(&bt, shown).len(), 2);

        let capped = BluetoothConfig {
            show_unnamed: true,
            max_devices: 1,
            ..hidden
        };
        assert_eq!(listed(&bt, capped).len(), 1);
        // A cap of zero would be a panel that lists nothing while devices are there to choose from.
        let zero = BluetoothConfig {
            max_devices: 0,
            ..hidden
        };
        assert_eq!(listed(&bt, zero).len(), 1);
    }

    #[test]
    fn a_row_is_keyed_on_what_it_draws() {
        // Same device, one connection later: the key has to move or the row keeps its old subtitle.
        let idle = device("WH-1000", true, None);
        let live = Device {
            connected: true,
            battery: Some(80),
            ..idle.clone()
        };
        assert_ne!(row_key(&idle), row_key(&live));
        assert_eq!(row_key(&idle), row_key(&idle.clone()));
    }

    /// Regression: the chip's tint closure read the foreground signal from *inside* a `with` over the state signal, and the panel's subtitle called `t!` — which reads the locale signal — from inside another. Both hold the reactive runtime's borrow across the inner read, which panics with "RefCell already borrowed" the moment the surface is built. Nothing catches it at compile time, and it only fires when the closures actually run, which is here.
    #[test]
    fn the_chip_and_the_panel_build_without_a_re_entrant_borrow() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(chip().is_ok(), "the bar chip builds");

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(bluetooth_panel().is_ok(), "the device panel builds");
    }

    #[test]
    fn the_trailing_column_prefers_a_battery_to_a_signal() {
        let mut d = device("WH-1000", true, Some(80));
        d.rssi = Some(-55);
        assert_eq!(trailing_line(&d), "80%");
        d.battery = None;
        assert_eq!(trailing_line(&d), "-55 dBm");
        d.rssi = None;
        assert_eq!(trailing_line(&d), "", "a remembered device reports neither");
    }
}
