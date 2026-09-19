//! The glyph and tint each service's state reads as.
//!
//! One home for it because the same state is drawn in four places — its own bar chip, the OSD, a hover popout and the status cluster — and four copies of "which icon means muted" drift. They already had: the volume chip and the OSD carried separate copies of the same three-way glyph, and the battery was tinted by three different rules depending on which surface you looked at.

use telar::Color;

use config::theme::NordTheme;
use config::{Severity, TemperatureConfig};
use services::bluetooth::Status;
use services::network::{Network, NetworkKind, WifiStatus};
use services::volume::Volume;
use services::weather::Condition;

/// Muted wins over the level, because it is the state that matters at a glance; below that the glyph tracks how far the sink is turned down.
pub fn volume(v: Volume) -> &'static str {
    if v.muted || v.level == 0 {
        "volume-x"
    } else if v.level < 50 {
        "volume-1"
    } else {
        "volume-2"
    }
}

pub fn microphone(v: Volume) -> &'static str {
    if v.muted || v.level == 0 {
        "mic-off"
    } else {
        "mic"
    }
}

pub fn brightness() -> &'static str {
    "sun"
}

/// A wired port, an off symbol when down, or a Wi-Fi arc whose fill tracks the signal strength.
pub fn network(net: Network) -> &'static str {
    match net.kind {
        NetworkKind::Ethernet => "ethernet-port",
        NetworkKind::Disconnected => "wifi-off",
        NetworkKind::Wifi => wifi_signal(net.signal.clamp(0, 100) as u8),
    }
}

/// The arc for a signal strength, 0–100. Shared by the chip, the cluster and every row of the network list, so "three bars" means the same number everywhere.
pub fn wifi_signal(strength: u8) -> &'static str {
    match strength {
        s if s >= 70 => "wifi",
        s if s >= 45 => "wifi-high",
        s if s >= 20 => "wifi-low",
        _ => "wifi-zero",
    }
}

/// The radio itself, for a cluster entry that reports Wi-Fi specifically rather than "am I online" — which is what [`network`] already covers, wired included.
pub fn wifi(status: WifiStatus) -> &'static str {
    if !status.available || !status.enabled {
        "wifi-off"
    } else if status.connected {
        wifi_signal(status.strength)
    } else {
        "wifi-zero"
    }
}

/// A radio that is on but joined to nothing recedes to muted: it is idle, not broken, and should not read as loudly as a live connection beside it.
pub fn wifi_tint(status: WifiStatus, theme: NordTheme, fg: Color) -> Color {
    if !status.available || !status.enabled {
        theme.muted
    } else if status.connected {
        fg
    } else {
        theme.subtle
    }
}

/// The radio's state in one glyph, most specific first: something connected outranks a scan, and a scan outranks an idle radio, because that is the order a user cares about them in.
///
/// Takes the `Copy` summary rather than the whole state so a chip can hold it in a signal and read it with a plain `get`; see [`bluetooth::Status`](services::bluetooth::Status).
pub fn bluetooth(bt: Status) -> &'static str {
    if !bt.available || !bt.powered {
        "bluetooth-off"
    } else if bt.connected > 0 {
        "bluetooth-connected"
    } else if bt.discovering {
        "bluetooth-searching"
    } else {
        "bluetooth"
    }
}

/// A connected radio reads in the accent, so "something is paired" is visible without reading the glyph's shape; an unavailable one recedes. `fg` is what the caller would otherwise paint with.
pub fn bluetooth_tint(bt: Status, theme: NordTheme, accent: Color, fg: Color) -> Color {
    if !bt.available || !bt.powered {
        theme.muted
    } else if bt.connected > 0 {
        accent
    } else {
        fg
    }
}

/// What kind of thing a Bluetooth device is, from BlueZ's `Icon` property. BlueZ names a freedesktop icon the user's theme may or may not carry; mapping it to the shell's own icon set is what makes a headset draw as a headset on every machine rather than only where that theme is installed.
pub fn bluetooth_device(icon: &str) -> &'static str {
    match icon {
        "audio-headset" | "audio-headphones" => "headphones",
        "audio-card" | "audio-speakers" => "speaker",
        "input-keyboard" => "keyboard",
        "input-mouse" | "input-tablet" => "mouse",
        "input-gaming" => "gamepad-2",
        "phone" => "smartphone",
        "computer" => "laptop",
        "printer" => "printer",
        "camera-photo" | "camera-video" => "camera",
        "network-wireless" => "wifi",
        _ => "bluetooth",
    }
}

pub fn battery(charging: bool) -> &'static str {
    if charging {
        "battery-charging"
    } else {
        "battery"
    }
}

/// Charging reads green, a low charge warns, and anything else takes the surrounding foreground so the icon sits with its neighbours. `fg` is what the caller would otherwise paint with — a chip's own foreground, which follows the container variant, or a panel's text token.
pub fn battery_tint(level: i32, charging: bool, theme: NordTheme, fg: Color) -> Color {
    if charging {
        theme.green
    } else if level <= 15 {
        theme.red
    } else if level <= 30 {
        theme.yellow
    } else {
        fg
    }
}

/// A reading past `[temperature] warn` reads amber and one past `critical` red; below both, or with no reading, it takes `fg`, the tint the caller would otherwise paint with.
pub fn heat_tint(
    config: &TemperatureConfig,
    celsius: Option<f32>,
    theme: NordTheme,
    fg: Color,
) -> Color {
    match celsius.map(|c| config.severity(c)) {
        Some(Severity::Critical) => theme.red,
        Some(Severity::Warn) => theme.yellow,
        Some(Severity::Normal) | None => fg,
    }
}

/// The sky. `day` picks between the sun and moon variants where the two differ, which is the difference between a clear night and a card that claims the sun is out at 2am.
pub fn weather(condition: Condition, day: bool) -> &'static str {
    match condition {
        Condition::Clear if day => "sun",
        Condition::Clear => "moon",
        Condition::MostlyClear if day => "cloud-sun",
        Condition::MostlyClear => "cloud-moon",
        Condition::Cloudy => "cloud",
        Condition::Overcast => "cloudy",
        Condition::Fog => "cloud-fog",
        Condition::Drizzle => "cloud-drizzle",
        Condition::Rain => "cloud-rain",
        Condition::FreezingRain => "cloud-hail",
        Condition::Snow | Condition::SnowShowers => "cloud-snow",
        Condition::Showers => "cloud-rain-wind",
        Condition::Thunderstorm => "cloud-lightning",
        Condition::Unknown => "cloud",
    }
}

/// The graphics card. Lucide has a `cpu` and nothing for a GPU, and drawing both readings with the same chip glyph would make two numbers on one bar indistinguishable — so this one comes from MDI.
pub fn gpu() -> &'static str {
    "mdi:expansion-card"
}

pub fn caps_lock() -> &'static str {
    "mdi:apple-keyboard-caps"
}

pub fn num_lock() -> &'static str {
    "mdi:numeric"
}

/// Do-Not-Disturb, which the bell chip and the quick toggle both draw.
pub fn dnd(on: bool) -> &'static str {
    if on { "bell-off" } else { "bell" }
}

pub fn game_mode(active: bool) -> &'static str {
    if active { "gamepad-2" } else { "gamepad" }
}

pub fn vpn(connected: bool) -> &'static str {
    if connected {
        "shield-check"
    } else {
        "shield-off"
    }
}

/// Holding the idle timers off reads as a machine kept awake, not as one asleep.
pub fn idle_inhibit(held: bool) -> &'static str {
    if held { "coffee" } else { "moon" }
}

pub fn keyboard_layout() -> &'static str {
    "keyboard"
}

pub fn now_playing() -> &'static str {
    "music"
}

/// Both live beside the state that decides them, so the service's own toast and a bar chip take the same answer; re-exported here so every glyph is still reached by one name.
pub use services::recorder::glyph as recording;
pub use services::screenshot::glyph as screenshot;

pub fn utilities() -> &'static str {
    "sliders-horizontal"
}

pub fn window_info() -> &'static str {
    "app-window"
}

pub fn area_select() -> &'static str {
    "crop"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muting_wins_over_the_level_in_both_directions() {
        let loud_but_muted = Volume {
            level: 90,
            muted: true,
        };
        assert_eq!(volume(loud_but_muted), "volume-x");
        assert_eq!(microphone(loud_but_muted), "mic-off");
        // A sink at zero is muted in every way that matters to someone reading the bar.
        let silent = Volume {
            level: 0,
            muted: false,
        };
        assert_eq!(volume(silent), "volume-x");
        assert_eq!(microphone(silent), "mic-off");
    }

    #[test]
    fn the_wifi_arc_fills_with_the_signal() {
        let at = |signal| {
            network(Network {
                kind: NetworkKind::Wifi,
                signal,
            })
        };
        assert_eq!(at(90), "wifi");
        assert_eq!(at(50), "wifi-high");
        assert_eq!(at(30), "wifi-low");
        assert_eq!(at(5), "wifi-zero");
        assert_eq!(
            network(Network {
                kind: NetworkKind::Ethernet,
                signal: 0
            }),
            "ethernet-port",
            "a wired link ignores the signal field it does not have"
        );
    }

    #[test]
    fn the_bluetooth_glyph_reports_the_most_specific_state() {
        let radio = |powered, connected, discovering| Status {
            available: true,
            powered,
            discovering,
            connected,
        };
        assert_eq!(bluetooth(Status::default()), "bluetooth-off", "no radio");
        assert_eq!(bluetooth(radio(false, 0, false)), "bluetooth-off");
        assert_eq!(bluetooth(radio(true, 0, false)), "bluetooth");
        assert_eq!(bluetooth(radio(true, 0, true)), "bluetooth-searching");
        assert_eq!(
            bluetooth(radio(true, 1, true)),
            "bluetooth-connected",
            "a connected device outranks a scan still running behind it"
        );

        let theme = NordTheme::new();
        assert_eq!(
            bluetooth_tint(radio(true, 1, false), theme, theme.orange, theme.text),
            theme.orange
        );
        assert_eq!(
            bluetooth_tint(radio(true, 0, false), theme, theme.orange, theme.text),
            theme.text
        );
        assert_eq!(
            bluetooth_tint(Status::default(), theme, theme.orange, theme.text),
            theme.muted
        );
    }

    #[test]
    fn the_sky_reads_differently_by_night() {
        assert_eq!(weather(Condition::Clear, true), "sun");
        assert_eq!(weather(Condition::Clear, false), "moon");
        assert_eq!(weather(Condition::MostlyClear, false), "cloud-moon");
        // Everything with weather in it looks the same at either hour, so it draws the same glyph.
        assert_eq!(
            weather(Condition::Rain, true),
            weather(Condition::Rain, false)
        );
        assert_eq!(weather(Condition::Thunderstorm, true), "cloud-lightning");
    }

    #[test]
    fn charging_outranks_a_low_charge_in_the_battery_tint() {
        let theme = NordTheme::new();
        assert_eq!(battery_tint(5, true, theme, theme.text), theme.green);
        assert_eq!(battery_tint(5, false, theme, theme.text), theme.red);
        assert_eq!(battery_tint(25, false, theme, theme.text), theme.yellow);
        assert_eq!(
            battery_tint(80, false, theme, theme.text),
            theme.text,
            "a healthy battery takes the surrounding foreground rather than a colour of its own"
        );
    }
}
