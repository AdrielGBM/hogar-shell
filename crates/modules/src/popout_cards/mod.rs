//! What each chip shows when the pointer rests on it.
//!
//! Every popout here reads a service that already exists and subscribes to it, so the card follows the value while it is up — hovering the volume chip and scrolling it is one gesture, and a card that froze at the level it opened with would be worse than no card. Nothing polls: each `watch` is bound to the popout surface and dies with it.

use telar::{RwSignal, signal, use_theme};

use config::theme::NordTheme;
use config::{AudioConfig, TemperatureConfig};
use services::{
    battery, bluetooth, brightness, gpu, hyprland, lockkeys, mpris, netspeed, network, pipewire,
    resources, volume,
};
use ui::card::Card;
use ui::glyph;
use ui::host::Host;
use util::reactive::{Live, derive, derive_pair, fixed, fixed_text};

pub fn volume(host: &Host) -> Card {
    audio_card(
        AudioSide::Output,
        host.options::<AudioConfig>(),
        use_theme::<NordTheme>(),
    )
}

pub fn mic(host: &Host) -> Card {
    audio_card(
        AudioSide::Input,
        host.options::<AudioConfig>(),
        use_theme::<NordTheme>(),
    )
}

pub fn brightness(_host: &Host) -> Card {
    brightness_card(use_theme::<NordTheme>())
}

pub fn battery(_host: &Host) -> Card {
    battery_card(use_theme::<NordTheme>())
}

pub fn network(_host: &Host) -> Card {
    network_card()
}

pub fn bluetooth(_host: &Host) -> Card {
    bluetooth_card(use_theme::<NordTheme>())
}

pub fn kblayout(_host: &Host) -> Card {
    keyboard_card()
}

pub fn lockstatus(_host: &Host) -> Card {
    lock_card()
}

pub fn activewindow(_host: &Host) -> Card {
    window_card()
}

pub fn media(_host: &Host) -> Card {
    media_card()
}

pub fn cpu(_host: &Host) -> Card {
    cpu_card(use_theme::<NordTheme>())
}

pub fn gpu(_host: &Host) -> Card {
    gpu_card(use_theme::<NordTheme>())
}

pub fn memory(_host: &Host) -> Card {
    memory_card(use_theme::<NordTheme>())
}

pub fn temperature(host: &Host) -> Card {
    temperature_card(
        host.options::<TemperatureConfig>(),
        use_theme::<NordTheme>(),
    )
}

pub fn netspeed(_host: &Host) -> Card {
    netspeed_card()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioSide {
    Output,
    Input,
}

/// Volume and microphone are the same card: a level, a mute state and the wheel step that moves it. Splitting them would duplicate every row to change one glyph and one string.
fn audio_card(side: AudioSide, audio: &AudioConfig, theme: NordTheme) -> Card {
    let initial = match side {
        AudioSide::Output => volume::current().unwrap_or(volume::Volume {
            level: 0,
            muted: false,
        }),
        AudioSide::Input => volume::current_mic().unwrap_or(volume::Volume {
            level: 0,
            muted: true,
        }),
    };
    let state = signal(initial);
    let sink = state;
    match side {
        AudioSide::Output => platform_wayland::watch(volume::subscribe, move |v| sink.set(v)),
        AudioSide::Input => platform_wayland::watch(volume::subscribe_mic, move |v| sink.set(v)),
    };

    // Which device the level belongs to. The chip is one glyph for whatever happens to be default, and after a headset is plugged in "is this the speakers or the headphones" is the question the hover is asked.
    let graph = signal(pipewire::current().unwrap_or_default());
    let graph_sink = graph;
    platform_wayland::watch(pipewire::subscribe, move |g| graph_sink.set(g));

    let ceiling = audio.ceiling() as f32;
    let title = match side {
        AudioSide::Output => telar::t!("popout.volume"),
        AudioSide::Input => telar::t!("popout.microphone"),
    };
    let glyph = derive(state, move |v| match side {
        AudioSide::Output => glyph::volume(v).to_string(),
        AudioSide::Input => glyph::microphone(v).to_string(),
    });
    let tint = derive(
        state,
        move |v| {
            if v.muted { theme.muted } else { theme.text }
        },
    );

    Card::titled(title)
        .icon(glyph)
        .icon_tint(tint)
        .subtitle(derive(state, |v| format!("{}%", v.level)))
        .meter(
            derive(state, move |v| v.level as f32 / ceiling.max(1.0)),
            derive(
                state,
                move |v| {
                    if v.muted { theme.muted } else { theme.accent }
                },
            ),
        )
        .row(
            fixed_text(telar::t!("popout.device")),
            derive(graph, move |g| {
                let device = match side {
                    AudioSide::Output => g.default_sink(),
                    AudioSide::Input => g.default_source(),
                };
                device
                    .map(|node| node.label())
                    .unwrap_or_else(|| telar::t!("sysinfo.no_reading"))
            }),
        )
        .row(
            fixed_text(telar::t!("popout.muted")),
            derive(state, |v| on_off(v.muted)),
        )
        .row(
            // Only the output side: an application recording is not something the shell can list without claiming more than PipeWire tells it, and the row would read as "nothing" on every machine.
            fixed_text(match side {
                AudioSide::Output => telar::t!("popout.playing"),
                AudioSide::Input => telar::t!("popout.step"),
            }),
            match side {
                AudioSide::Output => derive(graph, |g| playing_label(&g)),
                AudioSide::Input => fixed_text(format!("{}%", audio.step())),
            },
        )
}

/// What is making noise: the application when there is one, its name and one more when there are two, and a count past that — a popout has room for a line, not for a mixer.
fn playing_label(graph: &pipewire::Graph) -> String {
    let names: Vec<String> = graph
        .playback_streams()
        .filter(|node| !node.muted)
        .map(|node| node.label())
        .collect();
    match names.len() {
        0 => telar::t!("popout.nothing_playing"),
        1 | 2 => names.join(", "),
        n => telar::t!("popout.app_count", count = n.to_string()),
    }
}

fn brightness_card(theme: NordTheme) -> Card {
    let level = signal(brightness::current().unwrap_or(0));
    let sink = level;
    platform_wayland::watch(
        brightness::subscribe,
        move |snapshot: brightness::Snapshot| {
            if let Some(percent) = snapshot.level() {
                sink.set(percent);
            }
        },
    );

    Card::titled(telar::t!("popout.brightness"))
        .icon(fixed_text(glyph::brightness()))
        .subtitle(derive(level, |v| format!("{v}%")))
        .meter(derive(level, |v| v as f32 / 100.0), fixed(theme.accent))
        .row(
            fixed_text(telar::t!("popout.step")),
            fixed_text(format!("{}%", services::brightness::settings().step())),
        )
}

/// The battery card carries what the chip cannot: how long is left, and at what rate. `stream_details` is the same producer the battery panel uses, so hovering and clicking report the same numbers.
fn battery_card(theme: NordTheme) -> Card {
    let details = signal(battery::details());
    let sink = details;
    platform_wayland::watch(battery::stream_details, move |d| sink.set(Some(d)));

    let level = derive(details, |d| d.map(|d| d.level).unwrap_or(0));
    let charging = derive(details, |d| {
        d.map(|d| d.state.is_charging()).unwrap_or(false)
    });
    let charging_glyph = charging;
    let charging_tint = charging;
    let level_tint = level;

    Card::titled(telar::t!("popout.battery"))
        .icon(derive(charging_glyph, |c| glyph::battery(c).to_string()))
        .icon_tint(derive_pair(
            level_tint,
            charging_tint,
            move |level, charging| glyph::battery_tint(level, charging, theme, theme.text),
        ))
        .subtitle(derive(level, |l| format!("{l}%")))
        .meter(derive(level, |l| l as f32 / 100.0), fixed(theme.accent))
        .row(
            fixed_text(telar::t!("popout.status")),
            derive(details, |d| match d {
                Some(d) => battery_status(d),
                None => telar::t!("battery.none"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.rate")),
            derive(details, |d| match d {
                Some(d) if d.energy_rate > 0.0 => format!("{:.1} W", d.energy_rate),
                _ => telar::t!("sysinfo.no_reading"),
            }),
        )
}

fn battery_status(d: battery::BatteryDetails) -> String {
    use battery::ChargeState;
    match d.state {
        ChargeState::Charging => match duration_text(d.time_to_full) {
            Some(t) => telar::t!("battery.until_full", time = t),
            None => telar::t!("battery.charging"),
        },
        ChargeState::Discharging => match duration_text(d.time_to_empty) {
            Some(t) => telar::t!("battery.remaining", time = t),
            None => telar::t!("battery.on_battery"),
        },
        ChargeState::Full => telar::t!("battery.full"),
        ChargeState::Empty => telar::t!("battery.empty"),
        ChargeState::Pending => telar::t!("battery.pending"),
        ChargeState::Unknown => telar::t!("battery.unknown"),
    }
}

fn duration_text(secs: i64) -> Option<String> {
    if secs <= 0 {
        return None;
    }
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    Some(if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    })
}

/// The link verdict comes from sysfs and the detail from NetworkManager, which is why the card subscribes to both: the glyph and the "am I online" line keep working on a machine with no NetworkManager, and the SSID and band rows fill in where there is one.
fn network_card() -> Card {
    let state = signal(network::read());
    let sink = state;
    platform_wayland::watch(network::subscribe, move |net| sink.set(net));

    let wifi = signal(network::current_wifi().unwrap_or_default());
    let wifi_sink = wifi;
    platform_wayland::watch(network::subscribe_wifi, move |w| wifi_sink.set(w));

    Card::titled(telar::t!("popout.network"))
        .icon(derive(state, |net| glyph::network(net).to_string()))
        .subtitle(derive(wifi, |w| match w.active() {
            Some(point) => point.ssid.clone(),
            None => kind_label(network::read().kind),
        }))
        .row(
            fixed_text(telar::t!("popout.signal")),
            derive(state, |net| match net.kind {
                network::NetworkKind::Wifi => format!("{}%", net.signal),
                _ => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.security")),
            derive(wifi, |w| match w.active() {
                Some(point) if point.security == network::Security::Open => {
                    telar::t!("network.open")
                }
                Some(point) => point.security.id().to_uppercase(),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.band")),
            derive(wifi, |w| match w.active() {
                Some(point) if !point.band().is_empty() => point.band().to_string(),
                _ => telar::t!("sysinfo.no_reading"),
            }),
        )
}

fn kind_label(kind: network::NetworkKind) -> String {
    match kind {
        network::NetworkKind::Ethernet => telar::t!("popout.ethernet"),
        network::NetworkKind::Wifi => telar::t!("popout.wifi"),
        network::NetworkKind::Disconnected => telar::t!("popout.offline"),
    }
}

/// The chip is one glyph for four states; the popout is where "connected to what, and how much charge is left in it" fits. Which is the question a Bluetooth indicator is actually read for.
fn bluetooth_card(theme: NordTheme) -> Card {
    let state = signal(bluetooth::current().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(bluetooth::subscribe, move |bt| sink.set(bt));

    Card::titled(telar::t!("bluetooth.title"))
        .icon(derive(state, |bt| {
            glyph::bluetooth(bt.status()).to_string()
        }))
        .icon_tint(derive(state, move |bt| {
            glyph::bluetooth_tint(bt.status(), theme, theme.accent, theme.text)
        }))
        .subtitle(derive(state, |bt| {
            if !bt.available {
                telar::t!("bluetooth.no_adapter")
            } else if !bt.powered {
                telar::t!("bluetooth.off")
            } else if bt.discovering {
                telar::t!("bluetooth.scanning")
            } else {
                telar::t!(
                    "bluetooth.connected_count",
                    count = bt.connected_count().to_string()
                )
            }
        }))
        .row(
            fixed_text(telar::t!("popout.status")),
            derive(state, |bt| {
                if !bt.available {
                    telar::t!("bluetooth.no_adapter")
                } else if bt.powered {
                    telar::t!("bluetooth.on")
                } else {
                    telar::t!("bluetooth.off")
                }
            }),
        )
        .row(
            fixed_text(telar::t!("bluetooth.connected")),
            derive(state, |bt| match bt.primary() {
                Some(device) => device.label(),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.battery")),
            derive(state, |bt| match bt.primary().and_then(|d| d.battery) {
                Some(level) => format!("{level}%"),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
}

/// The chip shows a two-letter code; the popout is where the layout's full name fits.
fn keyboard_card() -> Card {
    let initial = hyprland::socket_dir()
        .and_then(|dir| hyprland::keyboard_layout(&dir))
        .unwrap_or_default();
    let layout = signal(initial);
    let sink = layout;
    platform_wayland::watch(hyprland::subscribe_keyboard, move |l| sink.set(l));

    Card::titled(telar::t!("popout.keyboard"))
        .icon(fixed_text("keyboard"))
        .subtitle(derive(layout, |l| {
            let name = l.name.trim();
            if name.is_empty() {
                telar::t!("sysinfo.no_reading")
            } else {
                name.to_string()
            }
        }))
}

fn lock_card() -> Card {
    let keys = signal(lockkeys::current().unwrap_or_else(lockkeys::read));
    let sink = keys;
    platform_wayland::watch(lockkeys::subscribe, move |k| sink.set(k));

    Card::titled(telar::t!("popout.lock_keys"))
        .icon(derive(keys, |k| {
            if k.caps { "lock" } else { "lock-open" }.to_string()
        }))
        .row(
            fixed_text(telar::t!("popout.caps_lock")),
            derive(keys, |k| on_off(k.caps)),
        )
        .row(
            fixed_text(telar::t!("popout.num_lock")),
            derive(keys, |k| on_off(k.num)),
        )
}

/// The bar truncates a window title to `max_chars`; the popout carries the whole one, plus the class a title alone doesn't identify.
fn window_card() -> Card {
    let initial = hyprland::socket_dir()
        .map(|dir| hyprland::active_window(&dir))
        .unwrap_or_default();
    let window = signal(initial);
    let sink = window;
    platform_wayland::watch(hyprland::subscribe_active_window, move |w| sink.set(w));

    Card::new(derive(window, |w| {
        let title = w.title.trim();
        if title.is_empty() {
            telar::t!("activewindow.none")
        } else {
            title.to_string()
        }
    }))
    .icon(fixed_text("app-window"))
    .subtitle(derive(window, |w| non_empty(&w.class)))
}

fn media_card() -> Card {
    let player = signal(mpris::current().unwrap_or_default());
    let sink = player;
    platform_wayland::watch(mpris::subscribe, move |p| sink.set(p));

    Card::new(derive(player, |p| {
        let title = p.title.trim();
        if title.is_empty() {
            telar::t!("popout.nothing_playing")
        } else {
            title.to_string()
        }
    }))
    .icon(derive(player, |p| crate::media::glyph(&p).to_string()))
    .subtitle(derive(player, |p| p.artist.clone()))
    .row(
        fixed_text(telar::t!("popout.album")),
        derive(player, |p| non_empty(&p.album)),
    )
    .row(
        fixed_text(telar::t!("popout.player")),
        derive(player, |p| non_empty(&p.identity)),
    )
}

fn cpu_card(theme: NordTheme) -> Card {
    let state = resource_signal();
    Card::titled(telar::t!("sysinfo.cpu"))
        .icon(fixed_text("cpu"))
        .subtitle(derive(state, |r| {
            // The model is what identifies the machine, and the popout is the only surface with room for it.
            match r.as_ref().map(|r| r.cpu_model.trim().to_string()) {
                Some(model) if !model.is_empty() => model,
                _ => percent(r.as_ref().map(|r| r.cpu)),
            }
        }))
        .meter(
            derive(state, |r| r.as_ref().map(|r| r.cpu / 100.0).unwrap_or(0.0)),
            fixed(theme.accent),
        )
        .row(
            fixed_text(telar::t!("popout.cores")),
            derive(state, |r| match r {
                Some(r) => r.cores.len().to_string(),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.peak")),
            derive(state, |r| percent(r.as_ref().map(|r| r.cpu_history.peak()))),
        )
        .row(
            fixed_text(telar::t!("popout.frequency")),
            derive(state, |r| match r.and_then(|r| r.cpu_mhz) {
                Some(mhz) if mhz >= 1000.0 => format!("{:.2} GHz", mhz / 1000.0),
                Some(mhz) => format!("{mhz:.0} MHz"),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
}

/// The GPU's card is the CPU's shape with a different set of unknowns: which of usage, temperature and VRAM a card answers is a property of its driver, so each row says "—" rather than a zero it did not measure.
fn gpu_card(theme: NordTheme) -> Card {
    let state = signal(gpu::current().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(gpu::subscribe, move |g| sink.set(g));

    Card::titled(telar::t!("sysinfo.gpu"))
        .icon(fixed_text(glyph::gpu()))
        .subtitle(derive(state, |g| {
            let name = g.name.trim().to_string();
            if name.is_empty() {
                telar::t!("sysinfo.no_reading")
            } else {
                name
            }
        }))
        .meter(
            derive(state, |g| g.usage.unwrap_or(0.0) / 100.0),
            fixed(theme.accent),
        )
        .row(
            fixed_text(telar::t!("popout.usage")),
            derive(state, |g| percent(g.usage)),
        )
        .row(
            fixed_text(telar::t!("popout.sensor")),
            derive(state, |g| match g.temperature {
                Some(c) => format!("{c:.0} °C"),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.vram")),
            derive(state, |g| match (g.vram_used, g.vram_total) {
                (Some(used), Some(total)) if total > 0 => format!(
                    "{} / {}",
                    resources::format_bytes(used),
                    resources::format_bytes(total)
                ),
                _ => telar::t!("sysinfo.no_reading"),
            }),
        )
}

fn memory_card(theme: NordTheme) -> Card {
    let state = resource_signal();
    Card::titled(telar::t!("sysinfo.memory"))
        .icon(fixed_text("memory-stick"))
        .subtitle(derive(state, |r| {
            percent(r.as_ref().map(|r| r.memory.used_percent()))
        }))
        .meter(
            derive(state, |r| {
                r.as_ref()
                    .map(|r| r.memory.used_percent() / 100.0)
                    .unwrap_or(0.0)
            }),
            fixed(theme.accent),
        )
        .row(
            fixed_text(telar::t!("popout.used")),
            derive(state, |r| match r {
                Some(r) => format!(
                    "{} / {}",
                    resources::format_bytes(r.memory.used),
                    resources::format_bytes(r.memory.total)
                ),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(
            fixed_text(telar::t!("popout.swap")),
            derive(state, |r| match r {
                Some(r) if r.memory.swap_total > 0 => format!(
                    "{} / {}",
                    resources::format_bytes(r.memory.swap_used),
                    resources::format_bytes(r.memory.swap_total)
                ),
                _ => telar::t!("sysinfo.no_reading"),
            }),
        )
        .row(fixed_text(telar::t!("popout.disk_io")), disk_row(state))
}

/// Names the sensor the reading came from, which is the one thing `[temperature] sensor` cannot be configured without: the chip shows a number, and only the popout says whose number it is.
fn temperature_card(settings: &TemperatureConfig, theme: NordTheme) -> Card {
    let state = resource_signal();
    let settings = settings.clone();
    let (unit, critical) = (settings.unit, settings.critical);
    let wanted = settings.sensor.clone();
    let for_label = wanted.clone();

    let celsius = derive(state, move |r| {
        r.as_ref().and_then(|r| r.temperature_of(&wanted))
    });
    let tint = celsius;
    let meter = celsius;
    let value = celsius;

    Card::titled(telar::t!("sysinfo.temperature"))
        .icon(fixed_text("thermometer"))
        .subtitle(derive(value, move |c| match c {
            Some(c) => unit.format(c),
            None => telar::t!("sysinfo.no_reading"),
        }))
        .meter(
            derive(meter, move |c| {
                (c.unwrap_or(0.0) / critical.max(1.0)).clamp(0.0, 1.0)
            }),
            derive(tint, move |c| {
                glyph::heat_tint(&settings, c, theme, theme.accent)
            }),
        )
        .row(
            fixed_text(telar::t!("popout.sensor")),
            derive(state, move |r| sensor_label(r.as_ref(), &for_label)),
        )
        .row(
            fixed_text(telar::t!("popout.critical")),
            fixed_text(unit.format(critical)),
        )
}

fn sensor_label(resources: Option<&resources::Resources>, wanted: &str) -> String {
    if !wanted.trim().is_empty() {
        return wanted.trim().to_string();
    }
    let Some(resources) = resources else {
        return telar::t!("sysinfo.no_reading");
    };
    resources
        .sensors
        .iter()
        .max_by(|a, b| a.celsius.total_cmp(&b.celsius))
        .map(|s| format!("{} {}", s.chip, s.label).trim().to_string())
        .unwrap_or_else(|| telar::t!("sysinfo.no_reading"))
}

fn netspeed_card() -> Card {
    let state = signal(netspeed::current());
    let sink = state;
    platform_wayland::watch(netspeed::subscribe, move |s| sink.set(Some(s)));

    Card::titled(telar::t!("popout.throughput"))
        .icon(fixed_text("arrow-down-up"))
        .row(
            fixed_text(telar::t!("popout.down")),
            derive(state, |s| rate(s.as_ref().map(|s| s.down))),
        )
        .row(
            fixed_text(telar::t!("popout.up")),
            derive(state, |s| rate(s.as_ref().map(|s| s.up))),
        )
        .row(
            fixed_text(telar::t!("popout.total")),
            derive(state, |s| match s {
                Some(s) => format!(
                    "{} / {}",
                    resources::format_bytes(s.total_down),
                    resources::format_bytes(s.total_up)
                ),
                None => telar::t!("sysinfo.no_reading"),
            }),
        )
}

/// Disk throughput has no chip of its own, so it rides on the memory card — the surface a user checks when the machine feels slow, which is the same question.
fn disk_row(state: RwSignal<Option<resources::Resources>>) -> Live<String> {
    derive(state, |r| match r {
        Some(r) => format!(
            "{} / {}",
            netspeed::format_rate(r.disk_read),
            netspeed::format_rate(r.disk_write)
        ),
        None => telar::t!("sysinfo.no_reading"),
    })
}

/// One subscription to the resource service, shared by whichever card asked for it. Three sysinfo popouts read the same snapshot, so they are all the same signal shaped differently.
pub(crate) fn resource_signal() -> RwSignal<Option<resources::Resources>> {
    let state = signal(resources::current());
    let sink = state;
    platform_wayland::watch(resources::subscribe, move |r| sink.set(Some(r)));
    state
}

fn on_off(value: bool) -> String {
    if value {
        telar::t!("common.on")
    } else {
        telar::t!("common.off")
    }
}

fn non_empty(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        telar::t!("sysinfo.no_reading")
    } else {
        text.to_string()
    }
}

pub(crate) fn percent(value: Option<f32>) -> String {
    match value {
        Some(v) => format!("{v:.0}%"),
        None => telar::t!("sysinfo.no_reading"),
    }
}

fn rate(value: Option<f64>) -> String {
    match value {
        Some(v) => netspeed::format_rate(v),
        None => telar::t!("sysinfo.no_reading"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sensor_row_names_the_configured_sensor_or_the_hottest_one() {
        let r = resources::Resources {
            sensors: vec![
                resources::Sensor {
                    chip: "coretemp".to_string(),
                    label: "Package".to_string(),
                    celsius: 55.0,
                },
                resources::Sensor {
                    chip: "k10temp".to_string(),
                    label: "Tctl".to_string(),
                    celsius: 71.0,
                },
            ],
            ..resources::Resources::default()
        };
        assert_eq!(sensor_label(Some(&r), " Tctl "), "Tctl");
        assert_eq!(sensor_label(Some(&r), ""), "k10temp Tctl");
    }
}
