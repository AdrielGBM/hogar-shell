use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::fdo::PropertiesProxy;

use config::BatteryWarning;
use util::broadcast::{Broadcast, Service};

const SUPPLY_DIR: &str = "/sys/class/power_supply";
const UPOWER: &str = "org.freedesktop.UPower";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";

/// A battery reading from sysfs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    /// Charge level 0–100.
    pub level: i32,
    pub charging: bool,
}

/// UPower's charge state, mapped from its numeric `State` property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    Empty,
    Pending,
    Unknown,
}

impl ChargeState {
    fn from_upower(state: u32) -> Self {
        match state {
            1 => Self::Charging,
            2 => Self::Discharging,
            3 => Self::Empty,
            4 => Self::Full,
            5 | 6 => Self::Pending,
            _ => Self::Unknown,
        }
    }

    /// Whether the battery icon should show the charging glyph; `Full` counts, mirroring the sysfs `read`.
    pub fn is_charging(self) -> bool {
        matches!(self, Self::Charging | Self::Full)
    }
}

/// The richer reading the detail panel shows, sourced from UPower's DisplayDevice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryDetails {
    /// Charge level 0–100.
    pub level: i32,
    pub state: ChargeState,
    /// Seconds until empty while discharging; 0 when unknown or not applicable.
    pub time_to_empty: i64,
    /// Seconds until full while charging; 0 when unknown or not applicable.
    pub time_to_full: i64,
    /// Charge/discharge rate in watts; 0 when unknown.
    pub energy_rate: f64,
}

fn first_battery_dir() -> Option<PathBuf> {
    let entries = fs::read_dir(SUPPLY_DIR).ok()?;
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Battery"))
}

/// Reads the first battery's level and charging state, or `None` when there is no battery (a desktop) or sysfs is unreadable; `Full` and `Charging` both count as charging.
pub fn read() -> Option<Battery> {
    let dir = first_battery_dir()?;
    let level = fs::read_to_string(dir.join("capacity"))
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?
        .clamp(0, 100);
    let status = fs::read_to_string(dir.join("status")).unwrap_or_default();
    let charging = matches!(status.trim(), "Charging" | "Full");
    Some(Battery { level, charging })
}

static BATTERY: Service<Battery> = Service::new("hogar-shell-battery", run_battery);

fn run_battery(service: &Arc<Broadcast<Battery>>) {
    // Push the current value immediately so bars don't wait for the first change.
    if let Some(b) = read() {
        service.publish(b);
    }
    // UPower's DisplayDevice `PropertiesChanged` for sub-second plug/unplug (it only triggers; sysfs holds the authoritative values); slow sysfs poll when UPower/DBus is unavailable.
    if watch_upower(service).is_none() {
        poll_fallback(service);
    }
}

fn watch_upower(service: &Broadcast<Battery>) -> Option<()> {
    let conn = crate::bus::system(None)?;
    let props = PropertiesProxy::builder(&conn)
        .destination(UPOWER)
        .ok()?
        .path(DISPLAY_DEVICE)
        .ok()?
        .build()
        .ok()?;
    let changes = props.receive_properties_changed().ok()?;
    for _ in changes {
        if let Some(b) = read() {
            service.publish(b);
        }
    }
    Some(())
}

/// Belt-and-suspenders when UPower is missing: the pre-existing 30 s sysfs poll.
fn poll_fallback(service: &Broadcast<Battery>) {
    while service.wanted() {
        std::thread::sleep(Duration::from_secs(30));
        match read() {
            Some(b) => service.publish(b),
            None => return,
        }
    }
}

/// Registers `tx` (bound to a bar's event loop) for live battery readings and sends the current one, spinning up the single shared UPower/sysfs source on first use. Called from a bar chip's `watch` producer.
pub fn subscribe(tx: EventSender<Battery>) {
    BATTERY.subscribe(tx);
}

/// Reads the full battery detail for the panel: UPower's DisplayDevice when available (level, state, time-to-empty/full, power draw), else a sysfs-only reading with no time/rate; `None` on a machine with no battery.
pub fn details() -> Option<BatteryDetails> {
    upower_details().or_else(sysfs_details)
}

fn read_details(props: &PropertiesProxy) -> Option<BatteryDetails> {
    let get_f64 = |name: &str| -> Option<f64> {
        f64::try_from(props.get(DEVICE_IFACE.try_into().ok()?, name).ok()?).ok()
    };
    let get_i64 = |name: &str| -> Option<i64> {
        i64::try_from(props.get(DEVICE_IFACE.try_into().ok()?, name).ok()?).ok()
    };
    let get_u32 = |name: &str| -> Option<u32> {
        u32::try_from(props.get(DEVICE_IFACE.try_into().ok()?, name).ok()?).ok()
    };
    // Percentage is the one property every real battery reports; its absence means DisplayDevice isn't a battery (a desktop), so bail to the sysfs path.
    let level = get_f64("Percentage")?.round().clamp(0.0, 100.0) as i32;
    Some(BatteryDetails {
        level,
        state: ChargeState::from_upower(get_u32("State").unwrap_or(0)),
        time_to_empty: get_i64("TimeToEmpty").unwrap_or(0),
        time_to_full: get_i64("TimeToFull").unwrap_or(0),
        energy_rate: get_f64("EnergyRate").unwrap_or(0.0),
    })
}

fn upower_details() -> Option<BatteryDetails> {
    let conn = crate::bus::system(None)?;
    let props = PropertiesProxy::builder(&conn)
        .destination(UPOWER)
        .ok()?
        .path(DISPLAY_DEVICE)
        .ok()?
        .build()
        .ok()?;
    read_details(&props)
}

fn sysfs_details() -> Option<BatteryDetails> {
    let b = read()?;
    Some(BatteryDetails {
        level: b.level,
        state: if b.charging {
            ChargeState::Charging
        } else {
            ChargeState::Discharging
        },
        time_to_empty: 0,
        time_to_full: 0,
        energy_rate: 0.0,
    })
}

/// Streams battery detail to `tx`: seeds immediately, then re-reads UPower's DisplayDevice on each `PropertiesChanged` (sub-second on plug/unplug), falling back to a slow poll when UPower/DBus is unavailable. The detail-panel counterpart to [`stream`].
pub fn stream_details(tx: EventSender<BatteryDetails>) {
    if let Some(d) = details()
        && !tx.send(d)
    {
        return;
    }
    if watch_upower_details(&tx).is_none() {
        poll_details_fallback(&tx);
    }
}

fn watch_upower_details(tx: &EventSender<BatteryDetails>) -> Option<()> {
    let conn = crate::bus::system(None)?;
    let props = PropertiesProxy::builder(&conn)
        .destination(UPOWER)
        .ok()?
        .path(DISPLAY_DEVICE)
        .ok()?
        .build()
        .ok()?;
    let changes = props.receive_properties_changed().ok()?;
    for _ in changes {
        match read_details(&props) {
            Some(d) if tx.send(d) => {}
            _ => return Some(()),
        }
    }
    Some(())
}

fn poll_details_fallback(tx: &EventSender<BatteryDetails>) {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        match details() {
            Some(d) if tx.send(d) => {}
            _ => return,
        }
    }
}

/// The charge a crossing test compares against when there is nothing to compare to yet — no previous reading, or the machine was on mains. Nothing has been warned about at this charge, so unplugging a laptop that is already at 15 % raises the 20 % warning immediately rather than waiting for a threshold it has passed.
const NOTHING_WARNED_YET: i32 = i32::MAX;

fn previous_level(previous: Option<Battery>) -> i32 {
    match previous {
        Some(p) if !p.charging => p.level,
        _ => NOTHING_WARNED_YET,
    }
}

/// Whether the charge just crossed *down* through `threshold`. A level that merely sits below it does not count, which is what stops a laptop parked at 19 % from warning on every reading.
fn crossed_down(previous: Option<Battery>, now: Battery, threshold: i32) -> bool {
    !now.charging && threshold > 0 && previous_level(previous) > threshold && now.level <= threshold
}

/// The warning to raise for a change from `previous` to `now`: the most severe threshold the charge has just crossed. A drop straight from 30 % to 5 % raises one notification, not one per level passed.
pub fn warning_for(
    previous: Option<Battery>,
    now: Battery,
    levels: &[BatteryWarning],
) -> Option<&BatteryWarning> {
    levels
        .iter()
        .filter(|w| crossed_down(previous, now, w.level))
        .min_by_key(|w| w.level)
}

thread_local! {
    // The reading the last crossing test was made against. A thread-local because `on_reading` runs on the driver thread, the one place the live config is readable.
    static LAST: Cell<Option<Battery>> = const { Cell::new(None) };
}

/// Raises the configured low-battery warning as the charge crosses a threshold, and runs `[battery] critical_action` once it drops to `critical_level`.
///
/// Installed on the driver thread by the shell's startup path rather than run inside the producer: the producer thread has neither the live config nor a way to reach the notification daemon's surface.
pub fn on_reading(reading: Battery) {
    let previous = LAST.replace(Some(reading));
    let Some(config) = config::config() else {
        return;
    };
    let battery = &config.battery;
    if !battery.enabled {
        return;
    }
    if let Some(warning) = warning_for(previous, reading, &battery.warn_levels) {
        let urgency = if warning.critical {
            crate::notifications::Urgency::Critical
        } else {
            crate::notifications::Urgency::Normal
        };
        crate::notifications::notify_shell(
            "hogar-shell",
            &warning.title(reading.level),
            &warning.message(reading.level),
            &warning.icon,
            urgency,
        );
    }
    if crossed_down(previous, reading, battery.critical_level)
        && let Some(action) = crate::session::Action::from_id(&battery.critical_action)
    {
        tracing::warn!(
            "battery at {}%: running the configured critical action '{}'",
            reading.level,
            battery.critical_action
        );
        crate::session::perform(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discharging(level: i32) -> Battery {
        Battery {
            level,
            charging: false,
        }
    }

    fn levels() -> Vec<BatteryWarning> {
        config::BatteryConfig::default().warn_levels
    }

    #[test]
    fn a_warning_fires_once_as_the_charge_crosses_its_level() {
        let levels = levels();
        assert_eq!(
            warning_for(Some(discharging(21)), discharging(19), &levels).map(|w| w.level),
            Some(20)
        );
        assert!(
            warning_for(Some(discharging(19)), discharging(18), &levels).is_none(),
            "sitting below a threshold is not crossing it again"
        );
        assert_eq!(
            warning_for(Some(discharging(11)), discharging(9), &levels).map(|w| w.level),
            Some(10)
        );
    }

    #[test]
    fn a_drop_past_several_levels_raises_only_the_most_severe() {
        let levels = levels();
        let fired = warning_for(Some(discharging(30)), discharging(5), &levels);
        assert_eq!(fired.map(|w| w.level), Some(10));
        assert!(
            fired.unwrap().critical,
            "the 10 % warning is the sticky one"
        );
    }

    #[test]
    fn charging_never_warns_and_re_arms_the_thresholds() {
        let levels = levels();
        let charging = Battery {
            level: 5,
            charging: true,
        };
        assert!(
            warning_for(Some(discharging(30)), charging, &levels).is_none(),
            "a battery on mains is not a problem however low it is"
        );
        // Unplugging at a charge already under the threshold warns straight away rather than waiting for a crossing that has already happened.
        assert_eq!(
            warning_for(Some(charging), discharging(15), &levels).map(|w| w.level),
            Some(20)
        );
    }

    #[test]
    fn the_first_reading_of_a_shell_started_on_a_low_battery_warns() {
        let levels = levels();
        assert_eq!(
            warning_for(None, discharging(8), &levels).map(|w| w.level),
            Some(10)
        );
        assert!(
            warning_for(None, discharging(95), &levels).is_none(),
            "a healthy battery says nothing"
        );
    }

    #[test]
    fn a_disabled_critical_level_never_crosses() {
        assert!(
            !crossed_down(Some(discharging(30)), discharging(1), 0),
            "critical_level = 0 must not suspend the machine at 1 %"
        );
        assert!(crossed_down(Some(discharging(30)), discharging(1), 5));
    }

    #[test]
    fn warning_text_falls_back_to_the_translated_default() {
        telar::set_locale("en");
        let default = BatteryWarning::default();
        assert!(default.title(17).contains("battery") || default.title(17).contains("Battery"));
        assert!(
            default.message(17).contains("17"),
            "the default body names the charge it fired at"
        );

        let custom = BatteryWarning {
            title: "Only {level}% left".to_string(),
            message: "Plug in".to_string(),
            ..BatteryWarning::default()
        };
        assert_eq!(custom.title(17), "Only 17% left");
        assert_eq!(custom.message(17), "Plug in");
    }
}
