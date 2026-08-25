//! Bluetooth, through BlueZ's D-Bus API.
//!
//! Everything the shell shows and every action it offers comes from one object tree: BlueZ publishes the adapter and every known device under `org.bluez` as managed objects, and emits `InterfacesAdded`, `InterfacesRemoved` and `PropertiesChanged` as they change. So there is nothing to poll — one subscription covers a device connecting, a scan finding a new one, a headset's battery dropping and the adapter being switched off.
//!
//! Two threads, for the same reason the tray needs them: the reader parks on a `MessageIterator` and must never block on a method call, so it only pings, and a refresher owns the connection that re-reads the tree. A scan emits an `RSSI` update per device per second, which is exactly the burst a coalescing refresher exists to fold into one re-read.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use util::broadcast::{Broadcast, Service};

const BLUEZ: &str = "org.bluez";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const BATTERY_IFACE: &str = "org.bluez.Battery1";

/// A burst of property changes is the normal case, not the exception: a scan reports a new signal strength for every visible device every second or so. The refresher waits this long after the first ping and drains the rest, turning the burst into one re-read.
const COALESCE: Duration = Duration::from_millis(120);

/// Reading the object tree is a local call to a daemon that is either answering or wedged; a bound keeps a wedged BlueZ from parking the refresher forever (see the tray's note on blocking calls into other processes).
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Connecting and pairing are slow *by design* — a headset can take ten seconds to come up, and pairing waits on the peer — so an action gets its own, far more generous bound than a read.
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);

/// How long a scan runs before stopping itself. Long enough to find a device someone is holding a pairing button on, short enough that a forgotten scan is not a radio left running all afternoon.
const SCAN_WINDOW: Duration = Duration::from_secs(45);

/// Which scan the auto-stop below belongs to. Every start or stop invalidates the previous window, so a scan restarted at second 44 gets a full one rather than inheriting one second.
static SCAN_GENERATION: AtomicU64 = AtomicU64::new(0);

/// One device BlueZ knows about: paired, previously seen, or currently in range of a scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// The object path (`/org/bluez/hci0/dev_AA_BB_…`) — the handle every action takes.
    pub path: String,
    pub address: String,
    pub name: String,
    /// BlueZ's `Icon`: a freedesktop icon name naming the *kind* of device (`audio-headset`, `input-mouse`), which is what lets the UI draw a headset as a headset rather than as a generic dot.
    pub icon: String,
    pub paired: bool,
    pub trusted: bool,
    pub blocked: bool,
    pub connected: bool,
    /// Signal strength in dBm while the device is in range of a scan; absent for one merely remembered.
    pub rssi: Option<i16>,
    /// Battery percentage, for the devices that report one (`org.bluez.Battery1`).
    pub battery: Option<u8>,
}

impl Device {
    /// What to call it in a list: BlueZ's name where there is one, else the hardware address, which is at least unique. Never empty — a row with no label is a row a user cannot act on.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.address.is_empty() {
            self.address.clone()
        } else {
            self.path.rsplit('/').next().unwrap_or_default().to_string()
        }
    }
}

/// The adapter and everything it knows about. `available` is false when there is no Bluetooth hardware at all (or no BlueZ), which is what lets a chip retire instead of showing a permanently-off radio.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bluetooth {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    /// The adapter's friendly name (`Alias`), which is what the machine advertises itself as.
    pub adapter: String,
    pub adapter_path: String,
    pub devices: Vec<Device>,
}

/// What a one-glyph indicator needs, without the device list behind it.
///
/// `Copy`, and that is the point: a chip holds this in its signal rather than the whole [`Bluetooth`], so reading it costs no allocation and — more importantly — needs no `with`. Holding the reactive runtime's borrow across a closure that also reads the foreground signal is a re-entrant borrow, and it panics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub connected: usize,
}

impl Bluetooth {
    /// The summary an indicator draws from.
    pub fn status(&self) -> Status {
        Status {
            available: self.available,
            powered: self.powered,
            discovering: self.discovering,
            connected: self.connected_count(),
        }
    }

    pub fn connected_count(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }

    /// The device a one-line summary is about: the first connected one, in the order [`sort_devices`] put them.
    pub fn primary(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.connected)
    }

    pub fn device(&self, path: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.path == path)
    }
}

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
type Props = HashMap<String, OwnedValue>;

fn as_bool(props: &Props, key: &str) -> bool {
    matches!(
        props.get(key).map(|v| Value::from(v.clone())),
        Some(Value::Bool(true))
    )
}

fn as_string(props: &Props, key: &str) -> String {
    match props.get(key).map(|v| Value::from(v.clone())) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn as_i16(props: &Props, key: &str) -> Option<i16> {
    match props.get(key).map(|v| Value::from(v.clone())) {
        Some(Value::I16(n)) => Some(n),
        _ => None,
    }
}

fn as_u8(props: &Props, key: &str) -> Option<u8> {
    match props.get(key).map(|v| Value::from(v.clone())) {
        Some(Value::U8(n)) => Some(n),
        _ => None,
    }
}

/// Connected first, then paired, then whatever a scan can currently hear, strongest signal first. The order a list is read in is the order the actions are wanted in: disconnect what is on, connect what is known, pair what is new.
fn sort_devices(devices: &mut [Device]) {
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then(b.rssi.cmp(&a.rssi))
            .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
    });
}

/// Turns BlueZ's object tree into the state the shell draws. Split from the D-Bus call so the shape of the answer is testable without a bus.
fn state_from_objects(objects: &ManagedObjects) -> Bluetooth {
    // The first adapter, by path, so a machine with a built-in radio and a dongle picks the same one every time rather than whichever the hash map happened to yield first.
    let mut adapters: Vec<(&OwnedObjectPath, &Props)> = objects
        .iter()
        .filter_map(|(path, ifaces)| ifaces.get(ADAPTER_IFACE).map(|props| (path, props)))
        .collect();
    adapters.sort_by_key(|(path, _)| path.as_str());
    let Some((adapter_path, adapter_props)) = adapters.first() else {
        return Bluetooth::default();
    };
    let adapter_path = adapter_path.as_str().to_string();

    let mut devices: Vec<Device> = objects
        .iter()
        .filter_map(|(path, ifaces)| {
            let props = ifaces.get(DEVICE_IFACE)?;
            if as_string(props, "Adapter") != adapter_path {
                return None;
            }
            Some(Device {
                path: path.as_str().to_string(),
                address: as_string(props, "Address"),
                name: as_string(props, "Alias"),
                icon: as_string(props, "Icon"),
                paired: as_bool(props, "Paired"),
                trusted: as_bool(props, "Trusted"),
                blocked: as_bool(props, "Blocked"),
                connected: as_bool(props, "Connected"),
                rssi: as_i16(props, "RSSI"),
                // The battery lives on a second interface of the same object, so it is read here rather than costing a call of its own per device.
                battery: ifaces
                    .get(BATTERY_IFACE)
                    .and_then(|b| as_u8(b, "Percentage")),
            })
        })
        .collect();
    sort_devices(&mut devices);

    Bluetooth {
        available: true,
        powered: as_bool(adapter_props, "Powered"),
        discovering: as_bool(adapter_props, "Discovering"),
        adapter: as_string(adapter_props, "Alias"),
        adapter_path,
        devices,
    }
}

fn connection(timeout: Duration) -> Option<Connection> {
    crate::bus::system(Some(timeout))
}

fn read_state(conn: &Connection) -> Bluetooth {
    let Ok(reply) = conn.call_method(
        Some(BLUEZ),
        "/",
        Some(OBJECT_MANAGER),
        "GetManagedObjects",
        &(),
    ) else {
        return Bluetooth::default();
    };
    match reply.body().deserialize::<ManagedObjects>() {
        Ok(objects) => state_from_objects(&objects),
        Err(e) => {
            tracing::warn!("bluetooth: cannot read BlueZ's object tree: {e}");
            Bluetooth::default()
        }
    }
}

static BLUETOOTH: Service<Bluetooth> = Service::new("hogar-shell-bluetooth", run);

fn run(out: &Arc<Broadcast<Bluetooth>>) {
    let Some(conn) = connection(READ_TIMEOUT) else {
        // No system bus at all. Publishing the empty state rather than nothing is what tells a subscribed chip there is no radio here, instead of leaving it waiting for a first reading that never comes.
        out.publish(Bluetooth::default());
        return;
    };
    let mut last = read_state(&conn);
    out.publish(last.clone());

    let (tx, rx) = sync_channel::<()>(1);
    if watch_signals(tx).is_none() {
        tracing::warn!("bluetooth: no signal subscription; state will not update");
        return;
    }
    while rx.recv().is_ok() {
        std::thread::sleep(COALESCE);
        while rx.try_recv().is_ok() {}
        let current = read_state(&conn);
        if current != last {
            last = current.clone();
            out.publish(current);
        }
    }
}

/// Parks a thread on every signal BlueZ emits and pings the refresher. It reads nothing itself: a blocking call from the thread draining the message queue is how a D-Bus client deadlocks against a slow peer.
fn watch_signals(ping: SyncSender<()>) -> Option<()> {
    let conn = connection(READ_TIMEOUT)?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ)
        .ok()?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).ok()?;
    std::thread::Builder::new()
        .name("hogar-shell-bluetooth-signals".to_string())
        .spawn(move || {
            for _ in signals {
                // A full channel already carries an unserviced ping, so dropping this one loses nothing.
                let _ = ping.try_send(());
            }
        })
        .ok()?;
    Some(())
}

/// The `[bluetooth]` settings, read through the cross-thread snapshot so a producer sees them too.
fn settings() -> config::BluetoothConfig {
    config::shared_config()
        .map(|c| c.bluetooth)
        .unwrap_or_default()
}

/// Registers `tx` for live Bluetooth state, starting the single shared producer on first use — unless `[bluetooth] enabled` is off, in which case no BlueZ connection and no thread are created. Guarded here rather than inside the producer because `Service` spawns on first touch.
pub fn subscribe(tx: EventSender<Bluetooth>) {
    if !settings().enabled {
        return;
    }
    BLUETOOTH.subscribe(tx);
}

/// The last published state, with no round-trip — what a click handler acts on.
pub fn current() -> Option<Bluetooth> {
    if !settings().enabled {
        return None;
    }
    BLUETOOTH.current()
}

/// The connection every mutation goes through, kept for the process.
///
/// Not an implementation detail: BlueZ scopes `StartDiscovery` to the D-Bus client that called it and stops the scan the moment that client disconnects. A mutation on a throwaway connection would therefore start a scan that ended before the first device was reported. Every other action is happy to share it.
fn control() -> Option<&'static Connection> {
    static CONTROL: std::sync::OnceLock<Option<Connection>> = std::sync::OnceLock::new();
    CONTROL.get_or_init(|| connection(ACTION_TIMEOUT)).as_ref()
}

/// Runs a BlueZ mutation off the UI thread. A click handler must never block on a pairing negotiation, and the reply is not interesting anyway: the state change arrives as a signal, on the same path a change made from any other application takes.
fn act(what: &'static str, job: impl FnOnce(&Connection) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-bluetooth-act".to_string())
        .spawn(move || match control() {
            Some(conn) => job(conn),
            None => tracing::warn!("bluetooth: cannot reach the system bus to {what}"),
        });
}

fn call(conn: &Connection, path: &str, iface: &str, method: &str) {
    if let Err(e) = conn.call_method(Some(BLUEZ), path, Some(iface), method, &()) {
        tracing::warn!("bluetooth: {method} on {path}: {e}");
    }
}

fn set_property(conn: &Connection, path: &str, iface: &str, name: &str, value: Value<'_>) {
    if let Err(e) = conn.call_method(
        Some(BLUEZ),
        path,
        Some(PROPERTIES),
        "Set",
        &(iface, name, value),
    ) {
        tracing::warn!("bluetooth: setting {iface}.{name} on {path}: {e}");
    }
}

/// Powers the adapter on or off, publishing the target first so the chip flips on the same frame. Switching off also clears the device list and any scan: BlueZ reports both a moment later, and showing a connected headset under a radio the user just turned off is worse than showing nothing.
pub fn set_powered(on: bool) {
    let Some(state) = current().filter(|s| s.available) else {
        return;
    };
    BLUETOOTH.publish(Bluetooth {
        powered: on,
        discovering: on && state.discovering,
        devices: if on {
            state.devices.clone()
        } else {
            Vec::new()
        },
        ..state.clone()
    });
    let path = state.adapter_path;
    act("power the adapter", move |conn| {
        set_property(conn, &path, ADAPTER_IFACE, "Powered", Value::Bool(on))
    });
}

pub fn toggle_powered() {
    if let Some(state) = current() {
        set_powered(!state.powered);
    }
}

/// Starts or stops a scan. A scan on an adapter that is off would be refused, so this powers it on first — the user asked to look for a device, not to be told the radio is off.
///
/// A scan is bounded by [`SCAN_WINDOW`] rather than left running. Discovery keeps the radio busy, drains a laptop and emits a signal per visible device per second, and nothing closes it on the way out: the surface that asked for it can be dismissed by a click on the scrim, which no handler sees. A self-limiting scan is the one shape that cannot leak.
pub fn set_discovering(on: bool) {
    let Some(state) = current().filter(|s| s.available) else {
        return;
    };
    if on && !state.powered {
        set_powered(true);
    }
    BLUETOOTH.publish(Bluetooth {
        discovering: on,
        powered: state.powered || on,
        ..state.clone()
    });
    let path = state.adapter_path;
    let method = if on {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };
    act("scan", move |conn| call(conn, &path, ADAPTER_IFACE, method));

    let generation = SCAN_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    if !on {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("hogar-shell-bluetooth-scan".to_string())
        .spawn(move || {
            std::thread::sleep(SCAN_WINDOW);
            // A later start or stop owns the scan now; this one has nothing left to end.
            if SCAN_GENERATION.load(Ordering::Relaxed) == generation {
                set_discovering(false);
            }
        });
}

pub fn toggle_discovering() {
    if let Some(state) = current() {
        set_discovering(!state.discovering);
    }
}

/// Connects a device, pairing first when it isn't paired yet. BlueZ's `Connect` on an unpaired device fails with an authentication error, so the one gesture a user has — "use this device" — has to cover both.
pub fn connect(path: &str) {
    let Some(device) = current().and_then(|s| s.device(path).cloned()) else {
        return;
    };
    let path = device.path.clone();
    act("connect", move |conn| {
        if !device.paired {
            call(conn, &path, DEVICE_IFACE, "Pair");
            // Pairing an audio device auto-connects it on most stacks; asking again is harmless where it did and necessary where it did not.
        }
        call(conn, &path, DEVICE_IFACE, "Connect");
    });
}

pub fn disconnect(path: &str) {
    let path = path.to_string();
    act("disconnect", move |conn| {
        call(conn, &path, DEVICE_IFACE, "Disconnect")
    });
}

/// The one gesture a device row offers: connect what is off, disconnect what is on.
pub fn toggle_device(path: &str) {
    let connected = current()
        .and_then(|s| s.device(path).map(|d| d.connected))
        .unwrap_or(false);
    if connected {
        disconnect(path);
    } else {
        connect(path);
    }
}

/// Removes the pairing entirely, so the device is forgotten rather than merely disconnected.
pub fn forget(path: &str) {
    let Some(state) = current().filter(|s| s.device(path).is_some()) else {
        return;
    };
    let (adapter, device) = (state.adapter_path, path.to_string());
    act("forget the device", move |conn| {
        let object = match zbus::zvariant::ObjectPath::try_from(device.as_str()) {
            Ok(object) => object,
            Err(e) => {
                tracing::warn!("bluetooth: '{device}' is not an object path: {e}");
                return;
            }
        };
        if let Err(e) = conn.call_method(
            Some(BLUEZ),
            adapter.as_str(),
            Some(ADAPTER_IFACE),
            "RemoveDevice",
            &(object,),
        ) {
            tracing::warn!("bluetooth: forgetting {device}: {e}");
        }
    });
}

/// Trusting a device is what lets it reconnect on its own — a keyboard that has to be re-authorised at every boot is a keyboard you cannot log in with.
pub fn set_trusted(path: &str, trusted: bool) {
    let path = path.to_string();
    act("trust the device", move |conn| {
        set_property(conn, &path, DEVICE_IFACE, "Trusted", Value::Bool(trusted))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, connected: bool, paired: bool, rssi: Option<i16>) -> Device {
        Device {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.to_string(),
            connected,
            paired,
            rssi,
            ..Device::default()
        }
    }

    #[test]
    fn the_list_reads_in_the_order_the_actions_are_wanted() {
        let mut devices = vec![
            device("far", false, false, Some(-90)),
            device("near", false, false, Some(-40)),
            device("known", false, true, None),
            device("headset", true, true, Some(-55)),
        ];
        sort_devices(&mut devices);
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["headset", "known", "near", "far"]);
    }

    #[test]
    fn a_device_always_has_something_to_call_it() {
        let unnamed = Device {
            address: "AA:BB:CC:DD:EE:FF".into(),
            ..Device::default()
        };
        assert_eq!(unnamed.label(), "AA:BB:CC:DD:EE:FF");
        let anonymous = Device {
            path: "/org/bluez/hci0/dev_AA".into(),
            ..Device::default()
        };
        assert_eq!(anonymous.label(), "dev_AA", "never an empty row");
    }

    /// The object tree BlueZ answers `GetManagedObjects` with, built by hand so the mapping is tested without a bus: one adapter, one connected headset with a battery, one device belonging to a second adapter.
    fn objects() -> ManagedObjects {
        let owned = |v: Value<'static>| OwnedValue::try_from(v).unwrap();
        let path = |p: &str| OwnedObjectPath::try_from(p.to_string()).unwrap();

        let adapter: Props = HashMap::from([
            ("Powered".to_string(), owned(Value::Bool(true))),
            ("Discovering".to_string(), owned(Value::Bool(true))),
            ("Alias".to_string(), owned(Value::Str("desk".into()))),
        ]);
        let headset: Props = HashMap::from([
            (
                "Adapter".to_string(),
                owned(Value::Str("/org/bluez/hci0".into())),
            ),
            ("Address".to_string(), owned(Value::Str("AA:BB".into()))),
            ("Alias".to_string(), owned(Value::Str("WH-1000".into()))),
            (
                "Icon".to_string(),
                owned(Value::Str("audio-headset".into())),
            ),
            ("Connected".to_string(), owned(Value::Bool(true))),
            ("Paired".to_string(), owned(Value::Bool(true))),
            ("RSSI".to_string(), owned(Value::I16(-55))),
        ]);
        let battery: Props = HashMap::from([("Percentage".to_string(), owned(Value::U8(80)))]);
        let other: Props = HashMap::from([
            (
                "Adapter".to_string(),
                owned(Value::Str("/org/bluez/hci1".into())),
            ),
            (
                "Alias".to_string(),
                owned(Value::Str("dongle mouse".into())),
            ),
        ]);

        HashMap::from([
            (
                path("/org/bluez/hci0"),
                HashMap::from([(ADAPTER_IFACE.to_string(), adapter)]),
            ),
            (
                path("/org/bluez/hci0/dev_AA_BB"),
                HashMap::from([
                    (DEVICE_IFACE.to_string(), headset),
                    (BATTERY_IFACE.to_string(), battery),
                ]),
            ),
            (
                path("/org/bluez/hci1/dev_CC_DD"),
                HashMap::from([(DEVICE_IFACE.to_string(), other)]),
            ),
        ])
    }

    #[test]
    fn the_object_tree_becomes_the_state_the_shell_draws() {
        let state = state_from_objects(&objects());
        assert!(state.available && state.powered && state.discovering);
        assert_eq!(state.adapter, "desk");
        assert_eq!(state.adapter_path, "/org/bluez/hci0");
        assert_eq!(
            state.devices.len(),
            1,
            "a second adapter's devices belong to that adapter, not this one"
        );
        let headset = &state.devices[0];
        assert_eq!(headset.label(), "WH-1000");
        assert_eq!(headset.icon, "audio-headset");
        assert_eq!(headset.rssi, Some(-55));
        assert_eq!(
            headset.battery,
            Some(80),
            "the battery is a second interface on the same object, not a second call"
        );
        assert_eq!(state.connected_count(), 1);
        assert_eq!(state.primary().map(|d| d.label()), Some("WH-1000".into()));
    }

    #[test]
    fn no_adapter_reads_as_no_bluetooth_rather_than_a_radio_that_is_off() {
        // The difference matters: an unavailable radio lets a chip retire, an off one asks to be switched on.
        let empty: ManagedObjects = HashMap::new();
        let state = state_from_objects(&empty);
        assert!(!state.available);
        assert!(!state.powered);
        assert!(state.devices.is_empty());
    }
}
