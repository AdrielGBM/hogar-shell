//! The network: a dependency-free link verdict, and the full NetworkManager view on top of it.
//!
//! Two layers on purpose. [`read`] answers "am I online, and over what" from sysfs alone — no NetworkManager, no D-Bus, correct on a machine running `systemd-networkd` or nothing at all — and it is what the bar chip and the status cluster draw. Everything a *panel* needs (an SSID, the networks in range, whether they are saved, and the calls that join one) only NetworkManager knows, so it lives in the [`Wifi`] service beside it and simply reports `available: false` where NM is not running.
//!
//! Keeping them apart is what makes the chip survive a machine without NM instead of going blank on it.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use config::NetworkConfig;
use util::broadcast::{Broadcast, Service};

const NET_DIR: &str = "/sys/class/net";
const NM_BUS: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// NetworkManager's device-type enum; only the two the shell distinguishes.
const NM_DEVICE_TYPE_ETHERNET: u32 = 1;
const NM_DEVICE_TYPE_WIFI: u32 = 2;

/// Poll used only when NetworkManager isn't on the bus and there is nothing to subscribe to.
const FALLBACK_POLL: Duration = Duration::from_secs(5);

/// Joining a network is a negotiation with a peer — DHCP, an authentication round-trip — so it gets far longer than a property read, which is a local call to a daemon that is either answering or wedged.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);

/// A scan republishes a signal strength per visible network, which is exactly the burst a coalescing refresher exists to fold into one re-read.
const COALESCE: Duration = Duration::from_millis(150);
const WIRELESS_STATUS: &str = "/proc/net/wireless";
/// `/proc/net/wireless` reports link quality on a 0–70 scale on most drivers; used to normalise it to a percentage.
const LINK_QUALITY_MAX: f32 = 70.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkKind {
    Ethernet,
    Wifi,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    pub kind: NetworkKind,
    /// Wi-Fi signal strength 0–100; 0 for ethernet and disconnected.
    pub signal: i32,
}

/// Reads the current network state from sysfs and `/proc/net/wireless`: a wired link wins when present (it's the active route), otherwise the first associated Wi-Fi interface with its signal strength, otherwise disconnected. Dependency-free — no NetworkManager required.
pub fn read() -> Network {
    let mut wifi: Option<i32> = None;
    let mut ethernet = false;
    if let Ok(entries) = fs::read_dir(NET_DIR) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == "lo" || !is_physical(name) || operstate(name) != "up" {
                continue;
            }
            if is_wireless(name) {
                wifi = Some(wifi_signal(name).unwrap_or(0));
            } else {
                ethernet = true;
            }
        }
    }
    match (ethernet, wifi) {
        (true, _) => Network {
            kind: NetworkKind::Ethernet,
            signal: 0,
        },
        (false, Some(signal)) => Network {
            kind: NetworkKind::Wifi,
            signal,
        },
        (false, None) => Network {
            kind: NetworkKind::Disconnected,
            signal: 0,
        },
    }
}

/// A real NIC has a backing device on a bus; virtual interfaces (`docker0`, `veth*`, VPN `tun*`) don't, so this keeps `read` to physical links and stops a bridge from masquerading as a wired connection.
fn is_physical(iface: &str) -> bool {
    Path::new(NET_DIR).join(iface).join("device").exists()
}

fn is_wireless(iface: &str) -> bool {
    let base = Path::new(NET_DIR).join(iface);
    base.join("wireless").exists() || base.join("phy80211").exists()
}

fn operstate(iface: &str) -> String {
    fs::read_to_string(Path::new(NET_DIR).join(iface).join("operstate"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Wi-Fi signal as a 0–100 percentage from `/proc/net/wireless`' link-quality column, or `None` when the interface isn't listed there.
fn wifi_signal(iface: &str) -> Option<i32> {
    let status = fs::read_to_string(WIRELESS_STATUS).ok()?;
    for line in status.lines() {
        let Some(rest) = line.trim_start().strip_prefix(iface) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        // Columns after "iface:" are: status, link-quality, level, noise.
        let link: f32 = rest
            .split_whitespace()
            .nth(1)?
            .trim_end_matches('.')
            .parse()
            .ok()?;
        return Some((link / LINK_QUALITY_MAX * 100.0).round().clamp(0.0, 100.0) as i32);
    }
    None
}

/// How a network is protected. The distinctions that matter to someone choosing one: whether it needs a password at all, whether it needs a *company's* login, and whether the encryption is one to avoid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Security {
    #[default]
    Open,
    /// Broken since 2001 and still deployed. Worth naming rather than folding into "secured".
    Wep,
    Wpa,
    Wpa2,
    Wpa3,
    /// 802.1X — a username and a certificate, which a shell cannot prompt for on its own.
    Enterprise,
}

impl Security {
    pub fn needs_password(self) -> bool {
        !matches!(self, Self::Open)
    }

    /// Whether the shell can join this on its own. Enterprise needs a certificate and an identity that belong in NetworkManager's own editor, so the panel sends the user there instead of asking for a password that would not be enough.
    pub fn joinable_with_a_password(self) -> bool {
        !matches!(self, Self::Enterprise)
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Wep => "wep",
            Self::Wpa => "wpa",
            Self::Wpa2 => "wpa2",
            Self::Wpa3 => "wpa3",
            Self::Enterprise => "enterprise",
        }
    }
}

/// Reads the security in force from an access point's three flag words.
///
/// The strongest key management on offer wins, because that is what a client will actually negotiate: an AP advertising both WPA2 and WPA3 is a WPA3 network to anything that can speak it. `PRIVACY` without any key management at all is the signature of WEP — the only case where the absence of the newer fields is itself the answer.
fn security_from_flags(flags: u32, wpa: u32, rsn: u32) -> Security {
    const PRIVACY: u32 = 0x1;
    const KEY_MGMT_PSK: u32 = 0x100;
    const KEY_MGMT_8021X: u32 = 0x200;
    const KEY_MGMT_SAE: u32 = 0x400;

    if rsn & KEY_MGMT_SAE != 0 {
        Security::Wpa3
    } else if (wpa | rsn) & KEY_MGMT_8021X != 0 {
        Security::Enterprise
    } else if rsn & KEY_MGMT_PSK != 0 {
        Security::Wpa2
    } else if wpa & KEY_MGMT_PSK != 0 {
        Security::Wpa
    } else if flags & PRIVACY != 0 {
        Security::Wep
    } else {
        Security::Open
    }
}

/// One network in range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessPoint {
    /// NetworkManager's object path — the handle every action takes, and unique where two networks share an SSID.
    pub path: String,
    pub ssid: String,
    /// 0–100.
    pub strength: u8,
    pub security: Security,
    /// MHz, which is what names the band.
    pub frequency: u32,
    /// A saved connection exists, so joining needs no password.
    pub saved: bool,
    pub active: bool,
}

impl AccessPoint {
    /// The band a user recognises, from the channel frequency.
    pub fn band(&self) -> &'static str {
        match self.frequency {
            0 => "",
            f if f >= 5925 => "6 GHz",
            f if f >= 4900 => "5 GHz",
            _ => "2.4 GHz",
        }
    }
}

/// The wireless radio and what it can see. `available` is false when NetworkManager isn't running or the machine has no wireless device — which is what lets a panel say so instead of showing an empty list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wifi {
    pub available: bool,
    pub enabled: bool,
    pub scanning: bool,
    /// The wireless interface (`wlan0`), for the actions and for a detail row.
    pub device: String,
    /// NetworkManager's object path for that device.
    pub device_path: String,
    pub points: Vec<AccessPoint>,
    /// The wired interface and whether it is up, which is the other half of "what am I connected over".
    pub ethernet: Option<String>,
}

/// What a one-glyph indicator needs, without the scan list behind it. `Copy`, so a chip can hold it in a signal and read it with a plain `get` — reading a signal inside another's `with` is a re-entrant borrow of the reactive runtime, and it panics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiStatus {
    pub available: bool,
    pub enabled: bool,
    pub connected: bool,
    /// The connected network's signal, 0–100; 0 when there is none.
    pub strength: u8,
}

impl Wifi {
    /// The summary an indicator draws from.
    pub fn status(&self) -> WifiStatus {
        let active = self.active();
        WifiStatus {
            available: self.available,
            enabled: self.enabled,
            connected: active.is_some(),
            strength: active.map(|p| p.strength).unwrap_or(0),
        }
    }

    pub fn active(&self) -> Option<&AccessPoint> {
        self.points.iter().find(|p| p.active)
    }

    pub fn point(&self, path: &str) -> Option<&AccessPoint> {
        self.points.iter().find(|p| p.path == path)
    }

    /// The strongest access point for each SSID, active first, then saved, then by signal.
    ///
    /// Deduplicating by name is the whole difference between a usable list and a scan dump: a mesh or a repeater publishes the same SSID from every radio it owns, and an office can show the same name a dozen times. A user picks a *network*, not a radio.
    pub fn networks(&self) -> Vec<AccessPoint> {
        let mut best: Vec<AccessPoint> = Vec::new();
        for point in &self.points {
            match best.iter_mut().find(|p| p.ssid == point.ssid) {
                Some(existing) => {
                    existing.saved |= point.saved;
                    existing.active |= point.active;
                    if point.strength > existing.strength {
                        existing.strength = point.strength;
                        existing.path = point.path.clone();
                        existing.frequency = point.frequency;
                        existing.security = point.security;
                    }
                }
                None => best.push(point.clone()),
            }
        }
        best.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then(b.saved.cmp(&a.saved))
                .then(b.strength.cmp(&a.strength))
                .then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
        });
        best
    }
}

fn nm_connection(timeout: Duration) -> Option<Connection> {
    crate::bus::system(Some(timeout))
}

fn property(conn: &Connection, path: &str, iface: &str, name: &str) -> Option<OwnedValue> {
    conn.call_method(Some(NM_BUS), path, Some(PROPERTIES), "Get", &(iface, name))
        .ok()?
        .body()
        .deserialize()
        .ok()
}

fn bool_property(conn: &Connection, path: &str, iface: &str, name: &str) -> bool {
    property(conn, path, iface, name)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn u32_property(conn: &Connection, path: &str, iface: &str, name: &str) -> u32 {
    property(conn, path, iface, name)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

fn string_property(conn: &Connection, path: &str, iface: &str, name: &str) -> String {
    property(conn, path, iface, name)
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_default()
}

fn paths_property(conn: &Connection, path: &str, iface: &str, name: &str) -> Vec<String> {
    property(conn, path, iface, name)
        .and_then(|v| Vec::<OwnedObjectPath>::try_from(v).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.as_str().to_string())
        .collect()
}

/// An SSID is a byte string, not text: the spec allows any 32 bytes, and a router configured in a non-UTF-8 locale will happily broadcast them. Lossy decoding keeps such a network listed (and joinable, since the actions key on the object path) rather than dropping it.
fn ssid_of(conn: &Connection, path: &str) -> String {
    let bytes = property(conn, path, AP_IFACE, "Ssid")
        .and_then(|v| Vec::<u8>::try_from(v).ok())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).to_string()
}

/// The SSIDs NetworkManager has a saved connection for, so the list can say which need a password.
fn saved_ssids(conn: &Connection) -> Vec<String> {
    let Ok(reply) = conn.call_method(
        Some(NM_BUS),
        SETTINGS_PATH,
        Some(SETTINGS_IFACE),
        "ListConnections",
        &(),
    ) else {
        return Vec::new();
    };
    let Ok(paths) = reply.body().deserialize::<Vec<OwnedObjectPath>>() else {
        return Vec::new();
    };
    paths
        .iter()
        .filter_map(|path| saved_ssid(conn, path.as_str()))
        .collect()
}

/// The SSID a saved connection is for, or `None` when it isn't a wireless one.
fn saved_ssid(conn: &Connection, path: &str) -> Option<String> {
    let reply = conn
        .call_method(
            Some(NM_BUS),
            path,
            Some(CONNECTION_IFACE),
            "GetSettings",
            &(),
        )
        .ok()?;
    let settings: HashMap<String, HashMap<String, OwnedValue>> = reply.body().deserialize().ok()?;
    let wireless = settings.get("802-11-wireless")?;
    let bytes = Vec::<u8>::try_from(wireless.get("ssid")?.try_clone().ok()?).ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

/// The object path of the saved connection for `ssid`, for activating or deleting it.
fn saved_connection(conn: &Connection, ssid: &str) -> Option<String> {
    let reply = conn
        .call_method(
            Some(NM_BUS),
            SETTINGS_PATH,
            Some(SETTINGS_IFACE),
            "ListConnections",
            &(),
        )
        .ok()?;
    let paths: Vec<OwnedObjectPath> = reply.body().deserialize().ok()?;
    paths
        .into_iter()
        .find(|path| saved_ssid(conn, path.as_str()).as_deref() == Some(ssid))
        .map(|path| path.as_str().to_string())
}

/// The full wireless picture in one pass: the radio switch, the wireless and wired devices, every access point the device can currently see, and which of them are already saved.
fn read_wifi(conn: &Connection) -> Wifi {
    let devices = paths_property(conn, NM_PATH, NM_IFACE, "Devices");
    if devices.is_empty() && !bool_property(conn, NM_PATH, NM_IFACE, "NetworkingEnabled") {
        return Wifi::default();
    }

    let mut wifi = Wifi {
        available: true,
        enabled: bool_property(conn, NM_PATH, NM_IFACE, "WirelessEnabled"),
        ..Wifi::default()
    };

    for device in &devices {
        match u32_property(conn, device, DEVICE_IFACE, "DeviceType") {
            NM_DEVICE_TYPE_WIFI if wifi.device_path.is_empty() => {
                wifi.device = string_property(conn, device, DEVICE_IFACE, "Interface");
                wifi.device_path = device.clone();
            }
            NM_DEVICE_TYPE_ETHERNET if wifi.ethernet.is_none() => {
                let iface = string_property(conn, device, DEVICE_IFACE, "Interface");
                if !iface.is_empty() {
                    wifi.ethernet = Some(iface);
                }
            }
            _ => {}
        }
    }

    if wifi.device_path.is_empty() {
        // A machine with no wireless device: NetworkManager is there, the radio is not.
        return Wifi {
            available: true,
            enabled: false,
            ..wifi
        };
    }

    wifi.scanning = scan_in_flight();
    let active = property(conn, &wifi.device_path, WIRELESS_IFACE, "ActiveAccessPoint")
        .and_then(|v| OwnedObjectPath::try_from(v).ok())
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let saved = saved_ssids(conn);

    for path in paths_property(conn, &wifi.device_path, WIRELESS_IFACE, "AccessPoints") {
        let ssid = ssid_of(conn, &path);
        if ssid.is_empty() {
            continue;
        }
        let security = security_from_flags(
            u32_property(conn, &path, AP_IFACE, "Flags"),
            u32_property(conn, &path, AP_IFACE, "WpaFlags"),
            u32_property(conn, &path, AP_IFACE, "RsnFlags"),
        );
        wifi.points.push(AccessPoint {
            active: path == active,
            saved: saved.contains(&ssid),
            strength: property(conn, &path, AP_IFACE, "Strength")
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(0),
            frequency: u32_property(conn, &path, AP_IFACE, "Frequency"),
            security,
            ssid,
            path,
        });
    }
    wifi
}

static WIFI: Service<Wifi> = Service::new("hogar-shell-wifi", run_wifi);

/// The `[network]` settings, or the defaults outside a started shell. Read through the cross-thread snapshot: the rescan timer runs on the producer, which cannot see the driver thread's copy.
fn settings() -> NetworkConfig {
    config::shared_config()
        .map(|c| c.network)
        .unwrap_or_default()
}

fn run_wifi(out: &Arc<Broadcast<Wifi>>) {
    let Some(conn) = nm_connection(READ_TIMEOUT) else {
        out.publish(Wifi::default());
        return;
    };
    let mut last = read_wifi(&conn);
    out.publish(last.clone());

    let (tx, rx) = sync_channel::<()>(1);
    if watch_nm_signals(tx.clone()).is_none() {
        tracing::warn!("wifi: no NetworkManager signals; the list will only refresh on rescan");
    }
    // A rescan on a timer, because an access point that goes away emits nothing: NetworkManager ages it out of its own list, and only a fresh scan notices. The interval is config, since a laptop in a café wants a faster one than a desktop that never moves.
    let rescan = tx.clone();
    let _ = std::thread::Builder::new()
        .name("hogar-shell-wifi-rescan".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(settings().rescan());
                request_scan();
                let _ = rescan.try_send(());
            }
        });

    while rx.recv().is_ok() {
        std::thread::sleep(COALESCE);
        while rx.try_recv().is_ok() {}
        let current = read_wifi(&conn);
        if current != last {
            last = current.clone();
            out.publish(current);
        }
    }
}

fn watch_nm_signals(ping: SyncSender<()>) -> Option<()> {
    let conn = nm_connection(READ_TIMEOUT)?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(NM_BUS)
        .ok()?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).ok()?;
    std::thread::Builder::new()
        .name("hogar-shell-wifi-signals".to_string())
        .spawn(move || {
            for _ in signals {
                let _ = ping.try_send(());
            }
        })
        .ok()?;
    Some(())
}

/// Registers `tx` for the wireless view — unless `[network] enabled` is off, in which case no D-Bus connection, no rescan timer and no thread are created. Guarded here rather than inside the producer because `Service` spawns on first touch, so a check further in would still cost a thread.
pub fn subscribe_wifi(tx: EventSender<Wifi>) {
    if !settings().enabled {
        return;
    }
    WIFI.subscribe(tx);
}

pub fn current_wifi() -> Option<Wifi> {
    if !settings().enabled {
        return None;
    }
    WIFI.current()
}

/// The connection every mutation goes through, kept for the process so an activation is not racing its own connection teardown.
fn control() -> Option<&'static Connection> {
    static CONTROL: std::sync::OnceLock<Option<Connection>> = std::sync::OnceLock::new();
    CONTROL
        .get_or_init(|| nm_connection(ACTION_TIMEOUT))
        .as_ref()
}

fn act(what: &'static str, job: impl FnOnce(&Connection) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-wifi-act".to_string())
        .spawn(move || match control() {
            Some(conn) => job(conn),
            None => tracing::warn!("wifi: cannot reach NetworkManager to {what}"),
        });
}

/// Turns the wireless radio on or off, publishing the target first so a toggle moves on the same frame.
pub fn set_wifi_enabled(enabled: bool) {
    let Some(state) = current_wifi().filter(|w| w.available) else {
        return;
    };
    WIFI.publish(Wifi {
        enabled,
        points: if enabled {
            state.points.clone()
        } else {
            Vec::new()
        },
        ..state
    });
    act("switch the radio", move |conn| {
        if let Err(e) = conn.call_method(
            Some(NM_BUS),
            NM_PATH,
            Some(PROPERTIES),
            "Set",
            &(NM_IFACE, "WirelessEnabled", Value::Bool(enabled)),
        ) {
            tracing::warn!("wifi: switching the radio: {e}");
        }
    });
}

pub fn toggle_wifi() {
    if let Some(state) = current_wifi() {
        set_wifi_enabled(!state.enabled);
    }
}

/// How long after a scan request the UI keeps saying it is scanning.
///
/// NetworkManager publishes no "scanning" property — only `LastScan`, a timestamp — and results trickle in as signals over several seconds. Timing out from the request is the honest approximation: it is what the shell actually knows, and a spinner that stops on its own beats one waiting for an event that never arrives.
const SCAN_WINDOW: Duration = Duration::from_secs(6);

static SCAN_STARTED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn scan_in_flight() -> bool {
    SCAN_STARTED
        .lock()
        .ok()
        .and_then(|at| *at)
        .is_some_and(|at| at.elapsed() < SCAN_WINDOW)
}

/// Asks NetworkManager to scan. Results arrive as signals, not as a reply — `RequestScan` returns as soon as the scan is queued — so there is nothing to wait for here.
pub fn request_scan() {
    let Some(device) = current_wifi()
        .filter(|w| w.available && w.enabled)
        .map(|w| w.device_path)
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    if let Ok(mut started) = SCAN_STARTED.lock() {
        *started = Some(std::time::Instant::now());
    }
    if let Some(state) = current_wifi() {
        WIFI.publish(Wifi {
            scanning: true,
            ..state
        });
    }
    act("scan", move |conn| {
        let options: HashMap<&str, Value> = HashMap::new();
        if let Err(e) = conn.call_method(
            Some(NM_BUS),
            device.as_str(),
            Some(WIRELESS_IFACE),
            "RequestScan",
            &(options,),
        ) {
            // A scan refused because one is already running is normal, not worth a warning at error level.
            tracing::debug!("wifi: scan refused: {e}");
        }
    });
}

/// Joins the network at `path`. A saved connection is activated as-is; anything else is created, which is where `password` is needed. An open network takes `None`.
pub fn connect(path: &str, password: Option<String>) {
    let Some(state) = current_wifi() else { return };
    let Some(point) = state.point(path).cloned() else {
        return;
    };
    let device = state.device_path.clone();
    let path = path.to_string();
    act("join the network", move |conn| {
        if let Some(saved) = saved_connection(conn, &point.ssid) {
            if let Err(e) = conn.call_method(
                Some(NM_BUS),
                NM_PATH,
                Some(NM_IFACE),
                "ActivateConnection",
                &(
                    object_path(&saved),
                    object_path(&device),
                    object_path(&path),
                ),
            ) {
                tracing::warn!("wifi: activating '{}': {e}", point.ssid);
            }
            return;
        }
        add_and_activate(conn, &point, &device, &path, password.as_deref());
    });
}

/// Builds the minimum connection NetworkManager needs for a new network and activates it in one call, which is what `AddAndActivateConnection` exists for — adding then activating would leave a half-configured connection behind whenever the second step failed.
fn add_and_activate(
    conn: &Connection,
    point: &AccessPoint,
    device: &str,
    ap_path: &str,
    password: Option<&str>,
) {
    let mut wireless: HashMap<&str, Value> = HashMap::new();
    wireless.insert("ssid", Value::from(point.ssid.as_bytes().to_vec()));

    let mut connection: HashMap<&str, Value> = HashMap::new();
    connection.insert("id", Value::from(point.ssid.clone()));
    connection.insert("type", Value::from("802-11-wireless"));

    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    settings.insert("connection", connection);
    settings.insert("802-11-wireless", wireless);

    if let Some(password) = password.filter(|p| !p.is_empty()) {
        let mut security: HashMap<&str, Value> = HashMap::new();
        // `wpa-psk` covers WPA, WPA2 and — as far as the key material goes — a WPA3 transition network; NetworkManager upgrades to SAE itself when the AP requires it.
        security.insert("key-mgmt", Value::from("wpa-psk"));
        security.insert("psk", Value::from(password.to_string()));
        settings.insert("802-11-wireless-security", security);
    }

    if let Err(e) = conn.call_method(
        Some(NM_BUS),
        NM_PATH,
        Some(NM_IFACE),
        "AddAndActivateConnection",
        &(settings, object_path(device), object_path(ap_path)),
    ) {
        tracing::warn!("wifi: joining '{}': {e}", point.ssid);
    }
}

/// `/` is NetworkManager's "no particular object" path, which is what an optional argument takes.
fn object_path(path: &str) -> zbus::zvariant::ObjectPath<'_> {
    zbus::zvariant::ObjectPath::try_from(path)
        .unwrap_or_else(|_| zbus::zvariant::ObjectPath::from_static_str_unchecked("/"))
}

/// Drops the saved connection for `ssid`, so the network is forgotten rather than merely disconnected.
pub fn forget(ssid: &str) {
    let ssid = ssid.to_string();
    act("forget the network", move |conn| {
        let Some(saved) = saved_connection(conn, &ssid) else {
            tracing::warn!("wifi: '{ssid}' has no saved connection to forget");
            return;
        };
        if let Err(e) = conn.call_method(
            Some(NM_BUS),
            saved.as_str(),
            Some(CONNECTION_IFACE),
            "Delete",
            &(),
        ) {
            tracing::warn!("wifi: forgetting '{ssid}': {e}");
        }
    });
}

/// Disconnects the wireless device, leaving the connection saved.
pub fn disconnect() {
    let Some(device) = current_wifi()
        .map(|w| w.device_path)
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    act("disconnect", move |conn| {
        if let Err(e) = conn.call_method(
            Some(NM_BUS),
            device.as_str(),
            Some(DEVICE_IFACE),
            "Disconnect",
            &(),
        ) {
            tracing::warn!("wifi: disconnecting: {e}");
        }
    });
}

static NETWORK: Service<Network> = Service::new("hogar-shell-network", run);

/// Registers `tx` for live network state, starting the single shared producer on first use. Called from a bar chip's `watch` producer.
pub fn subscribe(tx: EventSender<Network>) {
    NETWORK.subscribe(tx);
}

fn run(out: &Arc<Broadcast<Network>>) {
    out.publish(read());
    // sysfs stays the source of truth — it needs no NetworkManager and is already covered by tests — while NetworkManager is used purely as the trigger telling us when it is worth re-reading.
    if watch_network_manager(out).is_none() {
        poll_fallback(out);
    }
}

/// Blocks on every `PropertiesChanged` NetworkManager emits, on any of its objects: the manager's own state (connect/disconnect, primary connection type) and each access point's signal strength. One subscription therefore covers both what the icon's shape shows and how full its arc is, with no polling.
fn watch_network_manager(out: &Broadcast<Network>) -> Option<()> {
    let conn = Connection::system().ok()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(NM_BUS)
        .ok()?
        .interface("org.freedesktop.DBus.Properties")
        .ok()?
        .member("PropertiesChanged")
        .ok()?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).ok()?;
    let mut last = read();
    for _ in signals {
        // Strength updates are chatty and mostly land in the same display bucket, so only a reading that actually differs is worth waking every surface for.
        let current = read();
        if current != last {
            last = current;
            out.publish(current);
        }
    }
    Some(())
}

fn poll_fallback(out: &Broadcast<Network>) {
    let mut last = read();
    while out.wanted() {
        std::thread::sleep(FALLBACK_POLL);
        let current = read();
        if current != last {
            last = current;
            out.publish(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, strength: u8, saved: bool, active: bool) -> AccessPoint {
        AccessPoint {
            path: format!("/ap/{ssid}/{strength}"),
            ssid: ssid.to_string(),
            strength,
            saved,
            active,
            frequency: 2437,
            security: Security::Wpa2,
        }
    }

    #[test]
    fn the_strongest_key_management_on_offer_is_the_security_in_force() {
        const PRIVACY: u32 = 0x1;
        const PSK: u32 = 0x100;
        const EAP: u32 = 0x200;
        const SAE: u32 = 0x400;

        assert_eq!(security_from_flags(0, 0, 0), Security::Open);
        assert_eq!(
            security_from_flags(PRIVACY, 0, 0),
            Security::Wep,
            "privacy with no key management is WEP"
        );
        assert_eq!(security_from_flags(PRIVACY, PSK, 0), Security::Wpa);
        assert_eq!(security_from_flags(PRIVACY, 0, PSK), Security::Wpa2);
        assert_eq!(security_from_flags(PRIVACY, 0, SAE), Security::Wpa3);
        // A transition-mode AP advertises both; a client that speaks WPA3 gets WPA3, so that is what it is.
        assert_eq!(security_from_flags(PRIVACY, PSK, PSK | SAE), Security::Wpa3);
        assert_eq!(security_from_flags(PRIVACY, 0, EAP), Security::Enterprise);
    }

    #[test]
    fn what_the_shell_can_join_on_its_own_is_not_everything_secured() {
        assert!(!Security::Open.needs_password());
        assert!(Security::Wpa2.needs_password());
        // Enterprise needs an identity and a certificate; asking for a password would fail after the prompt.
        assert!(!Security::Enterprise.joinable_with_a_password());
        assert!(Security::Wpa3.joinable_with_a_password());
    }

    #[test]
    fn one_row_per_network_rather_than_one_per_radio() {
        // A mesh publishes the same SSID from every node; a list that showed each would be unusable.
        let wifi = Wifi {
            available: true,
            enabled: true,
            points: vec![
                ap("home", 42, true, false),
                ap("home", 88, false, true),
                ap("cafe", 55, false, false),
            ],
            ..Wifi::default()
        };
        let networks = wifi.networks();
        assert_eq!(networks.len(), 2, "two names, two rows");

        let home = &networks[0];
        assert_eq!(home.ssid, "home", "the connected one leads");
        assert_eq!(home.strength, 88, "and reports its strongest radio");
        assert!(home.saved, "saved on any radio is saved");
        assert!(home.active);
        assert_eq!(
            home.path, "/ap/home/88",
            "the action targets the radio it named"
        );
    }

    #[test]
    fn the_list_orders_by_what_a_user_would_reach_for() {
        let wifi = Wifi {
            points: vec![
                ap("weak-open", 20, false, false),
                ap("strong-new", 90, false, false),
                ap("known", 30, true, false),
                ap("connected", 10, true, true),
            ],
            ..Wifi::default()
        };
        let names: Vec<String> = wifi.networks().into_iter().map(|p| p.ssid).collect();
        assert_eq!(names, vec!["connected", "known", "strong-new", "weak-open"]);
    }

    #[test]
    fn the_band_comes_from_the_channel_frequency() {
        let at = |frequency| AccessPoint {
            frequency,
            ..AccessPoint::default()
        };
        assert_eq!(at(2437).band(), "2.4 GHz");
        assert_eq!(at(5180).band(), "5 GHz");
        assert_eq!(at(6135).band(), "6 GHz");
        assert_eq!(at(0).band(), "", "a network that reports none says nothing");
    }

    #[test]
    fn no_networkmanager_reads_as_unavailable_rather_than_as_a_radio_that_is_off() {
        // The distinction a panel needs: unavailable explains itself, off offers a switch.
        let absent = Wifi::default();
        assert!(!absent.available && !absent.enabled);
        assert!(absent.active().is_none());
        assert!(absent.networks().is_empty());
    }

    #[test]
    fn read_never_panics_and_is_self_consistent() {
        let net = read();
        // Only Wi-Fi carries a meaningful signal; the other kinds report zero.
        if net.kind != NetworkKind::Wifi {
            assert_eq!(net.signal, 0);
        }
        assert!((0..=100).contains(&net.signal));
    }
}
