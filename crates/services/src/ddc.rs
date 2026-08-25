//! External monitors over DDC/CI, through `ddcutil`.
//!
//! The only way to dim a desktop monitor: it has no sysfs backlight, because the panel's backlight is the monitor's own business and the cable is how you ask. `ddcutil` speaks that protocol over the I²C bus behind each output, and there is no library worth binding — the CLI is the interface every other shell uses too.
//!
//! Two things make it unlike the internal backlight, and both shape the service around it. It is a *process*, so every call has a deadline and none of them may happen on the UI thread. And a `getvcp` is slow enough (tens to hundreds of milliseconds per monitor) that reading one on a timer would be a permanent background cost for a value that only changes when somebody changes it — so levels are read once at detection and then tracked optimistically, which is why a change made with the monitor's own buttons is not noticed.

use std::time::Duration;

use util::deps::{self, Dep};

/// The VCP feature code for "brightness" in the MCCS standard every DDC/CI monitor implements.
const BRIGHTNESS_FEATURE: &str = "10";

/// Detection talks to every I²C bus in turn, and a monitor that answers slowly is normal rather than broken.
const DETECT_TIMEOUT: Duration = Duration::from_secs(15);

/// One monitor's read or write, which is a couple of short exchanges over the wire.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// One monitor `ddcutil` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Monitor {
    /// The I²C bus behind it, which is how every later call names it.
    pub bus: u8,
    /// The DRM connector `ddcutil` reported (`DP-1`), empty when this build does not report one. When it is there it *is* the compositor's output name, which beats matching on model text.
    pub connector: String,
    /// `MFG:MODEL:SERIAL` as the monitor's EDID spells it — what identifies it when there is no connector.
    pub model: String,
    pub serial: String,
}

/// Whether `ddcutil` is installed at all. One cheap call, so a machine without it never pays for a detection.
pub fn available() -> bool {
    deps::available(Dep::Ddcutil)
}

/// Every DDC/CI monitor on the machine.
///
/// Blocking and slow — seconds, on a bus with a monitor that answers lazily — so this only ever runs on the brightness service's own thread.
pub fn detect() -> Vec<Monitor> {
    let Some(stdout) = deps::output(Dep::Ddcutil, &["detect", "--brief"], DETECT_TIMEOUT) else {
        return Vec::new();
    };
    parse_detect(&stdout)
}

/// The current brightness of `bus` as a percentage, or `None` when the monitor does not answer.
pub fn level(bus: u8) -> Option<i32> {
    let bus = bus.to_string();
    let stdout = deps::output(
        Dep::Ddcutil,
        &["--bus", &bus, "getvcp", BRIGHTNESS_FEATURE, "--brief"],
        CALL_TIMEOUT,
    )?;
    parse_level(&stdout)
}

/// Sets `bus` to `percent`. Blocking; returns whether the monitor took it.
pub fn set(bus: u8, percent: i32) -> bool {
    let bus = bus.to_string();
    let value = percent.clamp(0, 100).to_string();
    deps::output(
        Dep::Ddcutil,
        &["--bus", &bus, "setvcp", BRIGHTNESS_FEATURE, &value],
        CALL_TIMEOUT,
    )
    .is_some()
}

/// Reads `ddcutil detect --brief`.
///
/// The format is stanzas of indented `key: value` lines under a `Display N` heading, and what is present varies by version and by monitor: `DRM connector` only appears on 1.4 and later, and a display the tool could not talk to (`Invalid display`) has no I²C bus to address. Anything without a bus is therefore dropped rather than kept as an entry nothing can be done with.
fn parse_detect(stdout: &str) -> Vec<Monitor> {
    let mut found = Vec::new();
    let mut current: Option<Monitor> = None;
    let mut usable = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Display ") || trimmed.starts_with("Invalid display") {
            if let Some(monitor) = current.take().filter(|_| usable) {
                found.push(monitor);
            }
            current = Some(Monitor::default());
            usable = trimmed.starts_with("Display ");
            continue;
        }
        let Some(monitor) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            // `/dev/i2c-6` — the number is the bus.
            "I2C bus" => {
                if let Some(bus) = value.rsplit('-').next().and_then(|n| n.parse().ok()) {
                    monitor.bus = bus;
                }
            }
            // `card1-DP-1`, whose tail is the connector the compositor calls `DP-1`.
            "DRM connector" => monitor.connector = connector_of(value),
            // `Monitor` carries all three EDID fields at once, colon-separated — which is why this parse cannot simply split the line on every colon.
            "Monitor" => {
                let mut parts = value.split(':');
                let make = parts.next().unwrap_or_default().trim();
                let model = parts.next().unwrap_or_default().trim();
                monitor.serial = parts.next().unwrap_or_default().trim().to_string();
                monitor.model = if make.is_empty() {
                    model.to_string()
                } else {
                    format!("{make} {model}")
                };
            }
            _ => {}
        }
    }
    if let Some(monitor) = current.filter(|_| usable) {
        found.push(monitor);
    }
    found.retain(|monitor| monitor.bus != 0 || !monitor.connector.is_empty());
    found
}

/// The connector name out of a DRM card path: `card1-DP-1` is `DP-1`.
fn connector_of(value: &str) -> String {
    value
        .split_once('-')
        .map(|(_card, rest)| rest)
        .unwrap_or(value)
        .to_string()
}

/// Reads `getvcp 10 --brief`: `VCP 10 C 40 100` — feature, type, current value, maximum.
///
/// The maximum is a monitor's own scale and is not always 100, so the percentage is computed rather than assumed.
fn parse_level(stdout: &str) -> Option<i32> {
    let line = stdout.lines().find(|l| l.contains("VCP"))?;
    let mut fields = line.split_whitespace().skip_while(|f| *f != "VCP").skip(1);
    let _feature = fields.next()?;
    let _kind = fields.next()?;
    let current: i64 = fields.next()?.parse().ok()?;
    let max: i64 = fields.next().and_then(|m| m.parse().ok()).unwrap_or(100);
    if max <= 0 {
        return None;
    }
    Some(((current * 100 + max / 2) / max).clamp(0, 100) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETECT_14: &str = "\
Display 1
   I2C bus:  /dev/i2c-6
   DRM connector: card1-DP-1
   Monitor:  DEL:DELL U2415:7MT0184N0AKS
Display 2
   I2C bus:  /dev/i2c-9
   DRM connector: card1-HDMI-A-1
   Monitor:  GSM:LG HDR 4K:0x00023f2b
";

    #[test]
    fn detect_reads_the_bus_the_connector_and_the_monitor() {
        let found = parse_detect(DETECT_14);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].bus, 6);
        assert_eq!(
            found[0].connector, "DP-1",
            "the card prefix is dropped: the compositor calls it DP-1"
        );
        assert_eq!(found[0].model, "DEL DELL U2415");
        assert_eq!(found[0].serial, "7MT0184N0AKS");
        assert_eq!(found[1].bus, 9);
        assert_eq!(found[1].connector, "HDMI-A-1");
    }

    /// An older ddcutil reports no connector at all, which is the case the model-matching fallback exists for.
    #[test]
    fn a_build_with_no_connector_still_yields_an_addressable_monitor() {
        let older = "\
Display 1
   I2C bus:  /dev/i2c-4
   EDID version:  1.3
   Monitor:  ACR:Acer XV272U:LHREE0048
";
        let found = parse_detect(older);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bus, 4);
        assert!(found[0].connector.is_empty());
        assert_eq!(found[0].model, "ACR Acer XV272U");
    }

    /// A display ddcutil could not talk to has nothing to control, and keeping it would put a slider on screen that does nothing.
    #[test]
    fn a_display_that_cannot_be_addressed_is_dropped() {
        let mixed = "\
Invalid display
   I2C bus:  /dev/i2c-3
   Monitor:  laptop panel
Display 1
   I2C bus:  /dev/i2c-7
   Monitor:  DEL:DELL U2415:ABC
";
        let found = parse_detect(mixed);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bus, 7);

        assert!(parse_detect("").is_empty());
        assert!(parse_detect("No displays found").is_empty());
    }

    #[test]
    fn a_level_is_a_percentage_of_the_monitors_own_maximum() {
        assert_eq!(parse_level("VCP 10 C 40 100"), Some(40));
        assert_eq!(
            parse_level("VCP 10 C 32 64"),
            Some(50),
            "a monitor whose scale is not 100 still reports a percentage"
        );
        assert_eq!(parse_level("VCP 10 C 0 100"), Some(0));
        // ddcutil prints a leading blank line on some builds, and a warning on others.
        assert_eq!(parse_level("\nVCP 10 C 75 100\n"), Some(75));
        assert_eq!(parse_level("DDC data error"), None);
        assert_eq!(parse_level("VCP 10 ERR"), None);
        assert_eq!(
            parse_level("VCP 10 C 5 0"),
            None,
            "a zero maximum is not a scale"
        );
    }
}
