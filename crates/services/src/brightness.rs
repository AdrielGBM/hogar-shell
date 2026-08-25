//! Screen brightness, per output.
//!
//! A laptop has one panel behind a sysfs backlight; a desk has two or three monitors, none of which has one. Both are the same question — "make that screen dimmer" — so the service publishes a *snapshot* of every controllable display rather than one number, and the scalar helpers the bar chip and the OSD use are that snapshot's primary reading. Which display is primary is the internal panel where there is one, since that is the screen a laptop's brightness keys mean.
//!
//! Internal panels are written through logind (permitted for the active session, so no root and no udev rule) and read back from a kernel uevent, which is what makes the chip follow the function keys. External monitors go through `ddcutil` (see [`super::ddc`]) — a process per call, so they are read once at detection and then tracked optimistically.

use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use platform_wayland::EventSender;

use crate::ddc;
use crate::hyprland;
use config::BrightnessConfig;
use util::broadcast::{Broadcast, Service};

const BACKLIGHT_DIR: &str = "/sys/class/backlight";
const LOGIND: &str = "org.freedesktop.login1";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";
/// Poll used only when the uevent socket cannot be opened, so the level still tracks external changes eventually.
const FALLBACK_POLL: Duration = Duration::from_secs(5);
/// How often a blocked receive gives the watcher a chance to notice its last subscriber has gone.
const WAKE: Duration = Duration::from_secs(1);
/// Kernel uevents are far smaller than this; anything longer is not a backlight event.
const UEVENT_MAX: usize = 4096;
/// The kernel's own broadcast, and udev's rebroadcast once it has processed the event.
const UEVENT_GROUP_KERNEL: u32 = 1;
const UEVENT_GROUP_UDEV: u32 = 2;

/// How a display's brightness is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A sysfs backlight, written through logind. `device` is its `/sys/class/backlight` name.
    Internal { device: String },
    /// An external monitor over DDC/CI, addressed by its I²C bus.
    External { bus: u8 },
}

impl Kind {
    pub fn is_internal(&self) -> bool {
        matches!(self, Kind::Internal { .. })
    }
}

/// One display the shell can dim, and how bright it is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    /// The compositor's connector name (`eDP-1`, `DP-1`) wherever it could be resolved — which is what every other per-output feature in the shell keys on. Falls back to the backlight device or `i2c-N` when it could not.
    pub output: String,
    /// What a human recognises it by: the monitor's model, or the backlight device.
    pub label: String,
    pub level: i32,
    pub kind: Kind,
}

/// Every controllable display at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub displays: Vec<Display>,
}

impl Snapshot {
    /// The reading a single number stands for: the internal panel where there is one, else the first display.
    ///
    /// The internal panel wins because that is what a laptop's brightness keys and the bar chip mean — on a desk with no panel the first monitor is a better answer than nothing.
    pub fn primary(&self) -> Option<&Display> {
        self.displays
            .iter()
            .find(|display| display.kind.is_internal())
            .or_else(|| self.displays.first())
    }

    pub fn level(&self) -> Option<i32> {
        self.primary().map(|display| display.level)
    }

    /// The display on `output`, matched by connector name.
    pub fn get(&self, output: &str) -> Option<&Display> {
        self.displays
            .iter()
            .find(|display| display.output.eq_ignore_ascii_case(output))
    }

    pub fn is_empty(&self) -> bool {
        self.displays.is_empty()
    }
}

fn backlight_dirs() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(BACKLIGHT_DIR) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("brightness").exists() && path.join("max_brightness").exists())
        .collect();
    // Sorted so the primary display is the same one across restarts rather than whatever `read_dir` yielded first.
    dirs.sort();
    dirs
}

fn first_backlight_dir() -> Option<PathBuf> {
    backlight_dirs().into_iter().next()
}

fn read_int(dir: &Path, name: &str) -> Option<i64> {
    fs::read_to_string(dir.join(name)).ok()?.trim().parse().ok()
}

fn percent_of(dir: &Path) -> Option<i32> {
    let current = read_int(dir, "brightness")?;
    let max = read_int(dir, "max_brightness")?;
    if max <= 0 {
        return None;
    }
    Some(((current * 100 + max / 2) / max).clamp(0, 100) as i32)
}

/// The first backlight's brightness as a 0–100 percentage, or `None` when there is no backlight (a desktop) or sysfs is unreadable.
pub fn read() -> Option<i32> {
    percent_of(&first_backlight_dir()?)
}

/// Every internal panel, read straight from sysfs.
fn internal_displays() -> Vec<Display> {
    backlight_dirs()
        .into_iter()
        .filter_map(|dir| {
            let device = dir.file_name()?.to_str()?.to_string();
            let level = percent_of(&dir)?;
            Some(Display {
                output: connector_for(&device),
                label: device.clone(),
                level,
                kind: Kind::Internal { device },
            })
        })
        .collect()
}

/// The connector name for a backlight device.
///
/// A panel backlight has no connector of its own in sysfs, and every compositor calls the internal panel `eDP-1` (or `LVDS-1` on older hardware) — so the *shell's* answer is the internal output the compositor reports, and the device name is only the fallback for a machine where none can be found.
fn connector_for(device: &str) -> String {
    internal_output().unwrap_or_else(|| device.to_string())
}

/// The compositor's name for the internal panel, if it has one connected.
fn internal_output() -> Option<String> {
    compositor_screens()
        .into_iter()
        .map(|screen| screen.name)
        .find(|name| is_internal_connector(name))
}

/// Whether a connector name is the kind a built-in panel uses.
fn is_internal_connector(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.starts_with("edp") || lowered.starts_with("lvds") || lowered.starts_with("dsi")
}

/// Every external monitor, with the level each reports. Slow: a process per monitor plus the detection itself.
fn external_displays() -> Vec<Display> {
    if !settings().external || !ddc::available() {
        return Vec::new();
    }
    let screens = compositor_screens();
    ddc::detect()
        .into_iter()
        .map(|monitor| {
            let level = ddc::level(monitor.bus).unwrap_or(0);
            Display {
                output: resolve_output(&monitor, &screens),
                label: if monitor.model.is_empty() {
                    format!("i2c-{}", monitor.bus)
                } else {
                    monitor.model.clone()
                },
                level,
                kind: Kind::External { bus: monitor.bus },
            }
        })
        .collect()
}

fn compositor_screens() -> Vec<hyprland::Screen> {
    hyprland::socket_dir()
        .map(|dir| hyprland::screens(&dir))
        .unwrap_or_default()
}

/// The compositor output a DDC monitor is behind.
///
/// `ddcutil` 1.4 reports the DRM connector, which *is* the answer; older builds do not, and then the only common ground is the EDID — so the model and serial are matched against what the compositor read off the same monitor. Failing both, the bus is the name, which at least stays stable and addressable.
fn resolve_output(monitor: &ddc::Monitor, screens: &[hyprland::Screen]) -> String {
    if !monitor.connector.is_empty() {
        return monitor.connector.clone();
    }
    if !monitor.serial.is_empty()
        && let Some(screen) = screens
            .iter()
            .find(|screen| screen.serial.eq_ignore_ascii_case(&monitor.serial))
    {
        return screen.name.clone();
    }
    if !monitor.model.is_empty()
        && let Some(screen) = screens.iter().find(|screen| {
            !screen.model.is_empty()
                && monitor
                    .model
                    .to_lowercase()
                    .contains(&screen.model.to_lowercase())
        })
    {
        return screen.name.clone();
    }
    format!("i2c-{}", monitor.bus)
}

static BRIGHTNESS: Service<Snapshot> = Service::new("hogar-shell-brightness", run);

/// Registers `tx` for live brightness readings, starting the single shared producer on first use. Called from a bar chip's `watch` producer.
pub fn subscribe(tx: EventSender<Snapshot>) {
    BRIGHTNESS.subscribe(tx);
}

fn run(out: &Arc<Broadcast<Snapshot>>) {
    let mut displays = internal_displays();
    if displays.is_empty() && !settings().external {
        // No panel and no DDC: nothing to report and nothing to watch, so the producer retires instead of spinning. The chip keeps whatever it seeded with.
        return;
    }
    // Published before the slow half so a laptop's chip is correct immediately rather than after a DDC detection.
    if !displays.is_empty() {
        out.publish(Snapshot {
            displays: displays.clone(),
        });
    }
    displays.extend(external_displays());
    if displays.is_empty() {
        return;
    }
    out.publish(Snapshot { displays });

    if watch_uevents(out).is_none() {
        poll_fallback(out);
    }
}

/// Replaces the internal panels' levels in the current snapshot, leaving every external monitor as it was — their levels cost a subprocess each and a backlight uevent says nothing about them.
fn refresh_internal(out: &Broadcast<Snapshot>) {
    let mut snapshot = out.current().unwrap_or_default();
    let fresh = internal_displays();
    for display in &mut snapshot.displays {
        if let Kind::Internal { device } = &display.kind
            && let Some(current) = fresh.iter().find(|candidate| match &candidate.kind {
                Kind::Internal { device: other } => other == device,
                _ => false,
            })
        {
            display.level = current.level;
        }
    }
    out.publish(snapshot);
}

/// Follows backlight uevents — the kernel emits one whenever the brightness changes, whoever changed it — so the chip tracks function keys and other tools without polling sysfs.
///
/// This used to run `udevadm monitor` and read its stdout, which cost a subprocess for the life of the subscription to learn a single fact. `udevadm monitor` is a reader of the socket opened here, so this is the same event from the same source with nothing in between.
fn watch_uevents(out: &Broadcast<Snapshot>) -> Option<()> {
    let socket = match uevent_socket() {
        Ok(socket) => socket,
        Err(reason) => {
            tracing::debug!("backlight uevents unavailable ({reason}); polling instead");
            return None;
        }
    };
    let mut message = [0u8; UEVENT_MAX];
    let mut last = read();
    while out.wanted() {
        // SAFETY: the kernel writes at most `message.len()` bytes into a buffer this thread owns.
        let received = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                message.as_mut_ptr().cast(),
                message.len(),
                0,
            )
        };
        if received < 0 {
            // The receive timeout is what lets this notice a subscription ending; every other error is the socket itself going away, which the poll fallback is there to survive.
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted => continue,
                _ => return None,
            }
        }
        if !is_backlight(&message[..received as usize]) {
            continue;
        }
        let current = read();
        if current != last {
            last = current;
            refresh_internal(out);
        }
    }
    Some(())
}

/// A uevent is a line, then NUL-separated `KEY=value` properties. Matching the whole property rather than searching for the word keeps an unrelated device whose path merely contains "backlight" out.
fn is_backlight(message: &[u8]) -> bool {
    message
        .split(|byte| *byte == 0)
        .any(|property| property == b"SUBSYSTEM=backlight")
}

/// The netlink socket the kernel broadcasts uevents on.
///
/// Both groups are asked for, and the order matters. Group 1 is the kernel's own broadcast and group 2 is udev's rebroadcast after it has processed the event; taking both means the watch still works where udevd is not running, which is what `udevadm monitor --udev` could not do. Binding to the kernel group is the half that may be refused, so a failure falls back to udev's alone rather than giving up.
fn uevent_socket() -> std::io::Result<OwnedFd> {
    // SAFETY: a bare `socket(2)`; the descriptor is adopted by `OwnedFd` before anything can return early.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor this thread owns and has not handed anywhere else.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };

    let timeout = libc::timeval {
        tv_sec: WAKE.as_secs() as libc::time_t,
        tv_usec: 0,
    };
    // SAFETY: a `timeval` of its own documented length, against a descriptor that is open.
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const timeout).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        );
    }

    for groups in [UEVENT_GROUP_KERNEL | UEVENT_GROUP_UDEV, UEVENT_GROUP_UDEV] {
        // SAFETY: `sockaddr_nl` is C integers and padding throughout, for which all-zero is a valid value — and it is the value wanted for `nl_pid`, which asks the kernel to allocate one rather than naming a port that another listener may already hold.
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_groups = groups;
        // SAFETY: a `sockaddr_nl` of its own documented length, against a descriptor that is open.
        let bound = unsafe {
            libc::bind(
                socket.as_raw_fd(),
                (&raw const address).cast(),
                size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if bound == 0 {
            return Ok(socket);
        }
    }
    Err(std::io::Error::last_os_error())
}

fn poll_fallback(out: &Broadcast<Snapshot>) {
    let mut last = read();
    while out.wanted() {
        std::thread::sleep(FALLBACK_POLL);
        let current = read();
        if current != last {
            last = current;
            refresh_internal(out);
        }
    }
}

/// Sets the primary display to `percent` — the internal panel on a laptop, the first monitor on a desk. What the brightness keys, the chip's wheel and `hogar-shell brightness set` with no monitor named all mean.
pub fn set(percent: i32) {
    let Some(output) = snapshot().primary().map(|display| display.output.clone()) else {
        // Nothing detected yet: on a laptop the panel is still the right guess, and the sysfs path answers without waiting for the producer's first publish.
        set_backlight_directly(percent);
        return;
    };
    set_output(&output, percent);
}

/// Sets the display on `output` to `percent`.
///
/// The snapshot is updated and published *first*, so the chip and the OSD move on the frame the wheel turned; the slow part — a D-Bus call or a `ddcutil` process — runs on a thread of its own. Which is also why an external monitor's level is what the shell last asked for rather than what the wire says: asking costs another process.
pub fn set_output(output: &str, percent: i32) {
    let percent = percent.clamp(0, 100);
    let mut snapshot = snapshot();
    let Some(display) = snapshot
        .displays
        .iter_mut()
        .find(|display| display.output.eq_ignore_ascii_case(output))
    else {
        return;
    };
    display.level = percent;
    let kind = display.kind.clone();
    *LAST_SET.lock().unwrap() = Some(display.output.clone());
    BRIGHTNESS.publish(snapshot);
    apply(kind, percent);
}

/// The display the shell most recently changed, which is not always the primary one.
static LAST_SET: Mutex<Option<String>> = Mutex::new(None);

/// The level an OSD should show: the display that was last changed, else the primary one.
///
/// An OSD is a report of what just happened, so `hogar-shell brightness up DP-2` on a desk must draw DP-2's level and not the first monitor's. The chip is the opposite question — it stands for the machine — and stays on `current`.
pub fn osd_level() -> Option<i32> {
    let last = LAST_SET.lock().unwrap().clone();
    let snapshot = snapshot();
    last.and_then(|output| snapshot.get(&output).map(|display| display.level))
        .or_else(current)
}

fn apply(kind: Kind, percent: i32) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-brightness-set".to_string())
        .spawn(move || match kind {
            Kind::Internal { device } => {
                let Some(absolute) = absolute_for(&device, percent) else {
                    return;
                };
                if write_logind(&device, absolute).is_none() {
                    tracing::warn!("brightness: logind SetBrightness failed for '{device}'");
                }
            }
            Kind::External { bus } => {
                if !ddc::set(bus, percent) {
                    tracing::warn!("brightness: ddcutil could not set bus {bus}");
                }
            }
        });
}

/// The raw sysfs value `percent` means for `device`, which is the unit logind takes.
fn absolute_for(device: &str, percent: i32) -> Option<u32> {
    let dir = Path::new(BACKLIGHT_DIR).join(device);
    let max = read_int(&dir, "max_brightness").filter(|max| *max > 0)?;
    Some((max * percent.clamp(0, 100) as i64 / 100).clamp(0, max) as u32)
}

/// The pre-snapshot path: a laptop panel written before the producer has published anything.
fn set_backlight_directly(percent: i32) {
    let Some(dir) = first_backlight_dir() else {
        return;
    };
    let Some(device) = dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    apply(
        Kind::Internal {
            device: device.to_string(),
        },
        percent,
    );
}

fn write_logind(device: &str, absolute: u32) -> Option<()> {
    let conn = crate::bus::system(None)?;
    conn.call_method(
        Some(LOGIND),
        SESSION_PATH,
        Some(SESSION_IFACE),
        "SetBrightness",
        &("backlight", device, absolute),
    )
    .ok()?;
    Some(())
}

/// Every controllable display, starting the producer on first use.
pub fn snapshot() -> Snapshot {
    BRIGHTNESS.current().unwrap_or_default()
}

/// Detects displays again, for a monitor plugged in since the shell started.
///
/// Asked for rather than guessed: DDC/CI has no hotplug signal to subscribe to, a kernel `drm` uevent says nothing about whether the new monitor answers DDC, and detection is far too slow to repeat on a timer. So the shell detects once at startup and `hogar-shell brightness refresh` is how a desk that changed says so.
pub fn refresh() {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-brightness-detect".to_string())
        .spawn(|| {
            let mut displays = internal_displays();
            displays.extend(external_displays());
            BRIGHTNESS.publish(Snapshot { displays });
        });
}

/// The primary display's level, falling back to sysfs before the producer has published anything.
pub fn current() -> Option<i32> {
    snapshot().level().or_else(read)
}

/// The level of one output, or `None` when nothing controllable is on it.
pub fn current_output(output: &str) -> Option<i32> {
    snapshot().get(output).map(|display| display.level)
}

/// The running `[brightness]` settings, or the defaults outside a started shell (a unit test, a service thread — [`config::config`] lives on the driver thread, where every caller of this runs).
pub fn settings() -> BrightnessConfig {
    config::shared_config()
        .map(|c| c.brightness)
        .unwrap_or_default()
}

/// Steps the primary display by `delta` percentage points; the shape a scroll or key-repeat gesture wants. No-op on a machine with nothing controllable.
pub fn step(delta: i32) {
    if let Some(level) = current() {
        set(level + delta);
    }
}

/// Steps one output by `delta` percentage points.
pub fn step_output(output: &str, delta: i32) {
    if let Some(level) = current_output(output) {
        set_output(output, level + delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprland::Screen;

    fn internal(output: &str, level: i32) -> Display {
        Display {
            output: output.to_string(),
            label: "intel_backlight".to_string(),
            level,
            kind: Kind::Internal {
                device: "intel_backlight".to_string(),
            },
        }
    }

    fn external(output: &str, bus: u8, level: i32) -> Display {
        Display {
            output: output.to_string(),
            label: "DEL DELL U2415".to_string(),
            level,
            kind: Kind::External { bus },
        }
    }

    #[test]
    fn the_primary_display_is_the_panel_a_brightness_key_means() {
        let laptop = Snapshot {
            displays: vec![external("DP-1", 6, 30), internal("eDP-1", 70)],
        };
        assert_eq!(
            laptop.primary().map(|d| d.output.as_str()),
            Some("eDP-1"),
            "the internal panel wins wherever it is in the list"
        );
        assert_eq!(laptop.level(), Some(70));

        // A desk has no panel, and the first monitor is a better answer than none.
        let desk = Snapshot {
            displays: vec![external("DP-1", 6, 30), external("HDMI-A-1", 9, 55)],
        };
        assert_eq!(desk.primary().map(|d| d.output.as_str()), Some("DP-1"));
        assert_eq!(desk.level(), Some(30));

        assert_eq!(Snapshot::default().level(), None);
        assert!(Snapshot::default().is_empty());
    }

    #[test]
    fn a_display_is_found_by_its_connector_name() {
        let snapshot = Snapshot {
            displays: vec![internal("eDP-1", 70), external("DP-1", 6, 30)],
        };
        assert_eq!(snapshot.get("DP-1").map(|d| d.level), Some(30));
        assert_eq!(
            snapshot.get("dp-1").map(|d| d.level),
            Some(30),
            "a name typed in the wrong case is the same monitor"
        );
        assert_eq!(snapshot.get("DP-2"), None);
    }

    fn screen(name: &str, model: &str, serial: &str) -> Screen {
        Screen {
            name: name.to_string(),
            model: model.to_string(),
            serial: serial.to_string(),
            ..Screen::default()
        }
    }

    /// The mapping the whole per-output story rests on: a DDC monitor has to become the same name the compositor, the bars and `[background.monitors]` all use, or nothing can be aimed at it.
    #[test]
    fn a_ddc_monitor_resolves_to_the_compositor_s_name() {
        let screens = vec![
            screen("DP-1", "DELL U2415", "7MT0184N0AKS"),
            screen("HDMI-A-1", "LG HDR 4K", "0x00023f2b"),
        ];

        // ddcutil 1.4 says which connector it is, and that answer needs no matching at all.
        let reported = ddc::Monitor {
            bus: 6,
            connector: "DP-1".to_string(),
            model: "DEL DELL U2415".to_string(),
            serial: "7MT0184N0AKS".to_string(),
        };
        assert_eq!(resolve_output(&reported, &screens), "DP-1");

        // An older build reports none, so the EDID serial is what identifies the monitor.
        let by_serial = ddc::Monitor {
            bus: 9,
            connector: String::new(),
            model: "GSM LG HDR 4K".to_string(),
            serial: "0x00023f2b".to_string(),
        };
        assert_eq!(resolve_output(&by_serial, &screens), "HDMI-A-1");

        // No serial either: the model text is the last identifying thing, and ddcutil prefixes the maker.
        let by_model = ddc::Monitor {
            bus: 9,
            connector: String::new(),
            model: "DEL DELL U2415".to_string(),
            serial: String::new(),
        };
        assert_eq!(resolve_output(&by_model, &screens), "DP-1");

        // Nothing matches: the bus is at least stable and addressable, where a blank name would not be.
        let unknown = ddc::Monitor {
            bus: 4,
            connector: String::new(),
            model: "Some Other Panel".to_string(),
            serial: "nope".to_string(),
        };
        assert_eq!(resolve_output(&unknown, &screens), "i2c-4");
        assert_eq!(resolve_output(&unknown, &[]), "i2c-4");
    }

    #[test]
    fn only_a_built_in_panel_s_connector_reads_as_internal() {
        for name in ["eDP-1", "edp-2", "LVDS-1", "DSI-1"] {
            assert!(is_internal_connector(name), "{name} is a built-in panel");
        }
        for name in ["DP-1", "HDMI-A-1", "DVI-D-1", ""] {
            assert!(!is_internal_connector(name), "{name} is not");
        }
    }

    /// A snapshot refresh must not disturb what it did not measure: a backlight uevent says nothing about a monitor on the other end of a cable, and re-reading one costs a subprocess.
    #[test]
    fn an_external_monitor_keeps_its_level_when_a_panel_changes() {
        let snapshot = Snapshot {
            displays: vec![internal("eDP-1", 70), external("DP-1", 6, 30)],
        };
        let fresh = [internal("eDP-1", 45)];
        let mut updated = snapshot.clone();
        for display in &mut updated.displays {
            if let Kind::Internal { device } = &display.kind
                && let Some(current) = fresh.iter().find(|candidate| match &candidate.kind {
                    Kind::Internal { device: other } => other == device,
                    _ => false,
                })
            {
                display.level = current.level;
            }
        }
        assert_eq!(updated.get("eDP-1").map(|d| d.level), Some(45));
        assert_eq!(
            updated.get("DP-1").map(|d| d.level),
            Some(30),
            "the monitor nobody asked about is unchanged"
        );
    }
}
