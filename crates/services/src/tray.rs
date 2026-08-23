//! The system tray, as a StatusNotifierItem host.
//!
//! Two D-Bus roles in one service. The shell *owns* `org.kde.StatusNotifierWatcher` — the registry every tray
//! application looks for before it will show itself — and it *is* a host, registering
//! `org.kde.StatusNotifierHost-<pid>` so applications that stay hidden until a host exists (most of them) come
//! out. When another shell already owns the watcher, this degrades to a plain client: the item list is then
//! read off that watcher's property instead of the local registry, and everything downstream is identical.
//!
//! Three threads, because the two roles must not block each other: the watcher connection parks on its object
//! server, a signal reader parks on one `MessageIterator`, and a refresher owns the connection that actually
//! reads item properties. Registrations and signals both land as a ping on the refresher's channel, so no
//! interface handler ever makes a blocking call — doing that from inside zbus's own executor is how a tray
//! deadlocks the moment a slow application registers.

use std::collections::HashMap;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator, fdo::DBusProxy, fdo::PropertiesProxy};
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, Value};

use util::broadcast::{Broadcast, Service};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

/// Bursts are normal — an application emits `NewIcon`, `NewToolTip` and `NewStatus` back to back on a single
/// state change — so the refresher waits this long after a ping and drains whatever else arrived, turning a
/// burst into one re-read instead of three.
const COALESCE: Duration = Duration::from_millis(40);

/// A tray application can wedge somewhere no signal reaches — a GPU driver deadlock will do it — and then it
/// accepts a method call and never answers. Without a bound, the refresher parks on that one application and
/// every *other* icon stops updating with it. Applied to every connection that calls into an application, so a
/// process that will never reply costs one slow refresh rather than the whole tray.
const METHOD_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether each item implements `Activate`, keyed by [`item_key`]. See [`implements_activate`].
static ACTIVATE_CACHE: LazyLock<Mutex<HashMap<String, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A session connection that gives up on an unanswered call. See [`METHOD_TIMEOUT`].
fn session() -> Option<Connection> {
    crate::bus::session(Some(METHOD_TIMEOUT))
}

/// How an item wants to be presented, per the spec's `Status` property.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// The item asks to be hidden; the shell honours it by not drawing the icon.
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl Status {
    fn parse(raw: &str) -> Self {
        match raw {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            _ => Self::Active,
        }
    }
}

/// An icon an application handed over as raw pixels rather than a name. Behind an `Arc` because every publish
/// clones the whole item list, and a 48×48 RGBA buffer per item is not worth copying on each redraw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One tray application, as the bar draws it and the click handlers act on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayItem {
    /// `bus/path` — unique per item, stable while it lives, and the reactive list's key.
    pub key: String,
    pub bus: String,
    pub path: String,
    /// The application's own identifier (`Id`), which is what `[tray] hidden` and `icon_subs` match on.
    pub id: String,
    pub title: String,
    pub status: Status,
    pub icon_name: String,
    pub attention_icon_name: String,
    /// A private icon directory the application ships (`IconThemePath`, a KDE extension). Several applications
    /// name an icon that exists nowhere in the user's theme and point here instead; without it they render
    /// blank.
    pub icon_theme_path: String,
    pub pixmap: Option<Arc<Pixmap>>,
    pub attention_pixmap: Option<Arc<Pixmap>>,
    /// Object path of the item's `com.canonical.dbusmenu`, empty when it exposes none.
    pub menu: String,
    /// The item asks that a primary click open its menu rather than call `Activate`.
    pub item_is_menu: bool,
    /// Whether the application actually implements `Activate`.
    ///
    /// Not a formality: everything built on libappindicator — Steam among them — implements only `Scroll` and
    /// `SecondaryActivate` and expects all interaction to go through its menu. Calling `Activate` there returns
    /// `UnknownMethod` and the icon looks inert, so the click has to know beforehand which verb the item speaks.
    pub has_activate: bool,
    pub tooltip: String,
}

impl TrayItem {
    /// The icon reference to draw: the attention icon while the item is asking for attention, else its normal
    /// one.
    pub fn icon_reference(&self) -> &str {
        if self.status == Status::NeedsAttention && !self.attention_icon_name.is_empty() {
            &self.attention_icon_name
        } else {
            &self.icon_name
        }
    }

    /// The raw-pixel icon matching [`Self::icon_reference`], for an item that named no icon at all.
    pub fn icon_pixmap(&self) -> Option<&Arc<Pixmap>> {
        if self.status == Status::NeedsAttention && self.attention_pixmap.is_some() {
            self.attention_pixmap.as_ref()
        } else {
            self.pixmap.as_ref()
        }
    }

    /// What a tooltip or a hover popout says: the item's tooltip, falling back to its title, then its id — so
    /// there is always something to identify it by.
    pub fn label(&self) -> &str {
        for candidate in [&self.tooltip, &self.title, &self.id] {
            if !candidate.trim().is_empty() {
                return candidate;
            }
        }
        ""
    }
}

/// Splits the string an application passes to `RegisterStatusNotifierItem` into the bus name and object path
/// to talk to it on.
///
/// The spec says "service name", applications disagree: KDE's own libraries pass a bare bus name, GTK's
/// AppIndicator passes an object path and expects the sender to be used as the bus, and a few pass
/// `bus/path` outright. All three appear in the wild, so all three are accepted.
fn split_service(service: &str, sender: &str) -> Option<(String, String)> {
    let service = service.trim();
    if service.is_empty() {
        return None;
    }
    if let Some(path) = service.strip_prefix('/') {
        let bus = sender.trim();
        if bus.is_empty() {
            return None;
        }
        return Some((bus.to_string(), format!("/{path}")));
    }
    match service.split_once('/') {
        Some((bus, path)) if !bus.is_empty() => Some((bus.to_string(), format!("/{path}"))),
        _ => Some((service.to_string(), DEFAULT_ITEM_PATH.to_string())),
    }
}

fn item_key(bus: &str, path: &str) -> String {
    format!("{bus}{path}")
}

/// Repacks the spec's `IconPixmap` entry — ARGB32 in network (big-endian) byte order — into the RGBA8 every
/// renderer here expects. `None` when the declared size doesn't match the bytes, which is a malformed item
/// rather than something to draw garbage for.
fn pixmap_from_argb(width: i32, height: i32, argb: &[u8]) -> Option<Pixmap> {
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    let pixels = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || argb.len() < pixels * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for chunk in argb.chunks_exact(4).take(pixels) {
        rgba.extend_from_slice(&[chunk[1], chunk[2], chunk[3], chunk[0]]);
    }
    Some(Pixmap {
        width,
        height,
        rgba,
    })
}

/// The largest pixmap an application offers, since a bar scales down far better than up. `IconPixmap` is
/// `a(iiay)`: a list of (width, height, ARGB32 bytes) ordered however the application felt like.
fn largest_pixmap(value: &Value<'_>) -> Option<Arc<Pixmap>> {
    let Value::Array(entries) = value else {
        return None;
    };
    entries
        .iter()
        .filter_map(|entry| {
            let Value::Structure(fields) = entry else {
                return None;
            };
            let fields = fields.fields();
            if fields.len() < 3 {
                return None;
            }
            let width = i32::try_from(&fields[0]).ok()?;
            let height = i32::try_from(&fields[1]).ok()?;
            let Value::Array(bytes) = &fields[2] else {
                return None;
            };
            let argb: Vec<u8> = bytes.iter().filter_map(|b| u8::try_from(b).ok()).collect();
            pixmap_from_argb(width, height, &argb)
        })
        .max_by_key(|p| p.width * p.height)
        .map(Arc::new)
}

/// The `ToolTip` property, whose useful part is buried: it is `(sa(iiay)ss)` — icon name, icon pixmaps, title,
/// body — and the title is what a one-line label wants.
fn tooltip_text(value: &Value<'_>) -> String {
    let Value::Structure(fields) = value else {
        return String::new();
    };
    let fields = fields.fields();
    let at = |i: usize| match fields.get(i) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    };
    let title = at(2);
    if title.trim().is_empty() {
        at(3)
    } else {
        title
    }
}

/// Whether the item's interface declares `Activate`, read from its introspection XML.
///
/// A substring check rather than an XML parse: the one fact needed is whether one method name is declared, and
/// the alternative is a parser dependency for a question that fits in a `contains`. A method that is absent
/// here is absent for good — an item does not grow one at runtime.
/// Cached per item, because the answer cannot change while the item lives and the refresher runs on every icon
/// blink — introspecting each one again every time would be a round trip per item per refresh, into exactly the
/// applications most likely to be slow. Pruned by [`forget_departed`] so it cannot outgrow the tray.
fn implements_activate(conn: &Connection, bus: &str, path: &str) -> bool {
    let key = item_key(bus, path);
    if let Some(known) = ACTIVATE_CACHE.lock().unwrap().get(&key) {
        return *known;
    }
    let answer = introspect_activate(conn, bus, path);
    ACTIVATE_CACHE.lock().unwrap().insert(key, answer);
    answer
}

fn introspect_activate(conn: &Connection, bus: &str, path: &str) -> bool {
    let Ok(name) = BusName::try_from(bus.to_string()) else {
        return false;
    };
    let Ok(reply) = conn.call_method(
        Some(name),
        path,
        Some("org.freedesktop.DBus.Introspectable"),
        "Introspect",
        &(),
    ) else {
        // Unreadable introspection is not evidence of absence; assume the spec's own verb works.
        return true;
    };
    match reply.body().deserialize::<String>() {
        Ok(xml) => xml.contains("name=\"Activate\""),
        Err(_) => true,
    }
}

/// Drops cache entries for items that are no longer registered, so a session that opens and closes tray
/// applications all day doesn't accumulate them.
fn forget_departed(live: &[(String, String)]) {
    let mut cache = ACTIVATE_CACHE.lock().unwrap();
    if cache.len() <= live.len() {
        return;
    }
    cache.retain(|key, _| live.iter().any(|(bus, path)| &item_key(bus, path) == key));
}

fn read_item(conn: &Connection, bus: &str, path: &str) -> Option<TrayItem> {
    let name = BusName::try_from(bus.to_string()).ok()?;
    let object = ObjectPath::try_from(path.to_string()).ok()?;
    let props = PropertiesProxy::builder(conn)
        .destination(name)
        .ok()?
        .path(object)
        .ok()?
        .build()
        .ok()?;
    // One round-trip for every property instead of one per property: a tray with a slow application in it
    // should not cost a dozen sequential calls each time it blinks.
    let all: HashMap<String, zbus::zvariant::OwnedValue> =
        props.get_all(ITEM_IFACE.try_into().ok()?).ok()?;

    let string = |key: &str| match all.get(key).map(|v| Value::from(v.clone())) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    };
    let menu = match all.get("Menu").map(|v| Value::from(v.clone())) {
        Some(Value::ObjectPath(p)) => p.to_string(),
        _ => String::new(),
    };

    Some(TrayItem {
        has_activate: implements_activate(conn, bus, path),
        key: item_key(bus, path),
        bus: bus.to_string(),
        path: path.to_string(),
        id: string("Id"),
        title: string("Title"),
        status: Status::parse(&string("Status")),
        icon_name: string("IconName"),
        attention_icon_name: string("AttentionIconName"),
        icon_theme_path: string("IconThemePath"),
        pixmap: all
            .get("IconPixmap")
            .and_then(|v| largest_pixmap(&Value::from(v.clone()))),
        attention_pixmap: all
            .get("AttentionIconPixmap")
            .and_then(|v| largest_pixmap(&Value::from(v.clone()))),
        menu,
        item_is_menu: all
            .get("ItemIsMenu")
            .and_then(|v| bool::try_from(Value::from(v.clone())).ok())
            .unwrap_or(false),
        tooltip: all
            .get("ToolTip")
            .map(|v| tooltip_text(&Value::from(v.clone())))
            .unwrap_or_default(),
    })
}

/// The registry behind the watcher interface. Shared with the interface object, which only ever mutates the
/// service list and pings the refresher — never touches the bus.
#[derive(Default)]
struct Registry {
    services: Mutex<Vec<(String, String)>>,
    host_registered: Mutex<bool>,
}

impl Registry {
    fn add(&self, bus: String, path: String) -> bool {
        let mut services = self.services.lock().unwrap();
        if services.iter().any(|(b, p)| b == &bus && p == &path) {
            return false;
        }
        services.push((bus, path));
        true
    }

    /// Drops every item owned by `bus`, for a tray application that exited without unregistering — which is the
    /// normal case, since a crashed or killed process never gets to.
    fn remove_owner(&self, bus: &str) -> bool {
        let mut services = self.services.lock().unwrap();
        let before = services.len();
        services.retain(|(b, _)| b != bus);
        services.len() != before
    }

    fn snapshot(&self) -> Vec<(String, String)> {
        self.services.lock().unwrap().clone()
    }
}

struct WatcherIface {
    registry: Arc<Registry>,
    ping: SyncSender<()>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherIface {
    fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let Some((bus, path)) = split_service(service, &sender) else {
            return;
        };
        if self.registry.add(bus, path) {
            let _ = self.ping.try_send(());
        }
    }

    fn register_status_notifier_host(&self, _service: &str) {
        *self.registry.host_registered.lock().unwrap() = true;
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registry
            .snapshot()
            .into_iter()
            .map(|(bus, path)| item_key(&bus, &path))
            .collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        // The shell is itself a host, so this is true from the moment the watcher exists. Applications gate
        // showing themselves on it, and answering `false` until some *other* host appears would hide the tray
        // from the only shell that can draw it.
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

static TRAY: Service<Vec<TrayItem>> = Service::new("hogar-shell-tray", run);

fn run(out: &Arc<Broadcast<Vec<TrayItem>>>) {
    let Some(conn) = session() else {
        tracing::info!("no session bus; the system tray is unavailable");
        out.publish(Vec::new());
        return;
    };
    out.publish(Vec::new());

    let registry = Arc::new(Registry::default());
    // Bounded and `try_send`: a ping is "something changed", so a full queue already carries that message and
    // dropping the extra one costs nothing. It also means an interface handler can never block on a slow
    // refresher.
    let (ping, pings) = sync_channel::<()>(8);

    let owned = own_watcher(Arc::clone(&registry), ping.clone());
    if !owned {
        tracing::info!("{WATCHER_NAME} is owned by another shell; following it as a client");
    }
    register_as_host(&conn);

    spawn_signal_reader(ping.clone());

    let local = owned.then(|| Arc::clone(&registry));
    let out = Arc::clone(out);
    let mut last: Vec<TrayItem> = Vec::new();
    // The first pass happens without waiting for a ping, so a shell started after the tray applications still
    // finds them.
    loop {
        let items = read_all(&conn, local.as_deref());
        if items != last {
            last = items.clone();
            out.publish(items);
        }
        if pings.recv().is_err() {
            return;
        }
        std::thread::sleep(COALESCE);
        while pings.try_recv().is_ok() {}
        prune_dead(&conn, local.as_deref());
    }
}

/// Claims the watcher name and serves the registry on it. `false` when another shell got there first, which is
/// not an error — the tray then follows that watcher instead of competing with it.
fn own_watcher(registry: Arc<Registry>, ping: SyncSender<()>) -> bool {
    let built = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name(WATCHER_NAME))
        .and_then(|b| b.serve_at(WATCHER_PATH, WatcherIface { registry, ping }))
        .and_then(|b| b.build());
    match built {
        Ok(conn) => {
            let _ = std::thread::Builder::new()
                .name("hogar-shell-tray-watcher".to_string())
                .spawn(move || {
                    // The object server runs on the connection's own executor; this thread exists only to keep
                    // the connection — and therefore the name — alive for the process.
                    let _conn = conn;
                    loop {
                        std::thread::park();
                    }
                });
            true
        }
        Err(_) => false,
    }
}

/// Registers the shell as a tray host. Applications commonly stay invisible until a host exists, so this is
/// what makes icons appear at all — including when another shell owns the watcher.
fn register_as_host(conn: &Connection) {
    // zbus warns that this claims a name with no object server behind it, and that is correct and intended: a
    // host name is a presence marker, not an endpoint. Applications look for one before they will show
    // themselves, and the traffic runs the other way — the host calls items, and the watcher only tracks that
    // the name exists. Serving an empty object here to quiet the warning would add an interface nobody calls.
    let name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let Ok(host) =
        zbus::blocking::connection::Builder::session().and_then(|b| b.name(name.clone()))
    else {
        return;
    };
    let Ok(host) = host.build() else { return };
    let _ = std::thread::Builder::new()
        .name("hogar-shell-tray-host".to_string())
        .spawn(move || {
            let _host = host;
            loop {
                std::thread::park();
            }
        });
    if let Err(e) = conn.call_method(
        Some(WATCHER_NAME),
        WATCHER_PATH,
        Some(WATCHER_NAME),
        "RegisterStatusNotifierHost",
        &name,
    ) {
        tracing::debug!("could not register as a tray host: {e}");
    }
}

/// Wakes the refresher on anything that can change what the tray shows: an item's own `New*` signals, the
/// watcher's registration signals (ours or another shell's), and a bus name vanishing.
fn spawn_signal_reader(ping: SyncSender<()>) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-tray-signals".to_string())
        .spawn(move || {
            let Some(conn) = session() else {
                return;
            };
            let Some(items) = signal_rule(ITEM_IFACE) else {
                return;
            };
            let Ok(dbus) = DBusProxy::new(&conn) else {
                return;
            };
            for extra in [signal_rule(WATCHER_NAME), owner_rule()]
                .into_iter()
                .flatten()
            {
                let _ = dbus.add_match_rule(extra);
            }
            let Ok(signals) = MessageIterator::for_match_rule(items, &conn, None) else {
                return;
            };
            for _ in signals {
                if ping.try_send(()).is_err() {
                    // A full queue means a refresh is already pending; only a closed one is fatal.
                    if ping.try_send(()).is_err() {
                        continue;
                    }
                }
            }
        });
}

fn signal_rule(interface: &str) -> Option<zbus::MatchRule<'static>> {
    zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(interface.to_string())
        .ok()
        .map(|b| b.build())
}

fn owner_rule() -> Option<zbus::MatchRule<'static>> {
    zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender("org.freedesktop.DBus")
        .ok()?
        .interface("org.freedesktop.DBus")
        .ok()?
        .member("NameOwnerChanged")
        .ok()
        .map(|b| b.build())
}

/// The services the watcher knows about: the local registry when this shell owns the watcher, else the other
/// watcher's property. Reading our own property over the bus would mean calling into our own object server
/// from outside it, so the local path is both faster and safer.
fn registered_services(conn: &Connection, local: Option<&Registry>) -> Vec<(String, String)> {
    if let Some(registry) = local {
        return registry.snapshot();
    }
    let Ok(props) = PropertiesProxy::builder(conn)
        .destination(WATCHER_NAME)
        .and_then(|b| b.path(WATCHER_PATH))
        .and_then(|b| b.build())
    else {
        return Vec::new();
    };
    let Ok(iface) = WATCHER_NAME.try_into() else {
        return Vec::new();
    };
    let Ok(value) = props.get(iface, "RegisteredStatusNotifierItems") else {
        return Vec::new();
    };
    Vec::<String>::try_from(value)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| split_service(s, ""))
        .collect()
}

/// Forgets items whose application is gone. Only meaningful for the local registry — another shell's watcher
/// prunes its own.
fn prune_dead(conn: &Connection, local: Option<&Registry>) {
    let Some(registry) = local else { return };
    let Ok(dbus) = DBusProxy::new(conn) else {
        return;
    };
    for (bus, _) in registry.snapshot() {
        let alive = BusName::try_from(bus.clone())
            .ok()
            .and_then(|name| dbus.name_has_owner(name).ok())
            .unwrap_or(false);
        if !alive {
            registry.remove_owner(&bus);
        }
    }
}

/// Every registered item, read fresh. An item that fails to answer is dropped rather than shown stale: it is
/// either mid-exit or broken, and a dead icon that still takes a click is worse than a missing one.
fn read_all(conn: &Connection, local: Option<&Registry>) -> Vec<TrayItem> {
    let services = registered_services(conn, local);
    forget_departed(&services);
    services
        .into_iter()
        .filter_map(|(bus, path)| read_item(conn, &bus, &path))
        .collect()
}

pub fn subscribe(tx: EventSender<Vec<TrayItem>>) {
    TRAY.subscribe(tx);
}

pub fn current() -> Option<Vec<TrayItem>> {
    TRAY.current()
}

/// Hands the module a list of items without starting the host — no D-Bus name is claimed and no thread runs, so
/// what a `[preview]` seeds is what it draws. See [`util::broadcast::Service::seed`].
pub fn seed(items: Vec<TrayItem>) {
    TRAY.seed(items);
}

/// Calls `method` on an item, off the UI thread. Every tray interaction is a round-trip to another application,
/// which may be busy; none of them may run on the frame.
fn invoke(item: &TrayItem, method: &'static str, args: (i32, i32)) {
    let bus = item.bus.clone();
    let path = item.path.clone();
    let _ = std::thread::Builder::new()
        .name("hogar-shell-tray-call".to_string())
        .spawn(move || {
            let Some(conn) = session() else { return };
            let Ok(name) = BusName::try_from(bus.clone()) else {
                return;
            };
            if let Err(e) =
                conn.call_method(Some(name), path.as_str(), Some(ITEM_IFACE), method, &args)
            {
                tracing::debug!("tray {method} on {bus}: {e}");
            }
        });
}

/// A primary click. The coordinates are the spec's hint for where a menu should pop up; applications that
/// ignore them (most) simply toggle their window.
pub fn activate(item: &TrayItem, x: i32, y: i32) {
    invoke(item, "Activate", (x, y));
}

pub fn secondary_activate(item: &TrayItem, x: i32, y: i32) {
    invoke(item, "SecondaryActivate", (x, y));
}

pub fn context_menu(item: &TrayItem, x: i32, y: i32) {
    invoke(item, "ContextMenu", (x, y));
}

/// The wheel over an icon, forwarded as the spec's `Scroll(delta, orientation)` — which is how a volume applet
/// in the tray responds to scrolling.
pub fn scroll(item: &TrayItem, delta: i32, horizontal: bool) {
    let bus = item.bus.clone();
    let path = item.path.clone();
    let orientation = if horizontal { "horizontal" } else { "vertical" };
    let _ = std::thread::Builder::new()
        .name("hogar-shell-tray-call".to_string())
        .spawn(move || {
            let Some(conn) = session() else { return };
            let Ok(name) = BusName::try_from(bus.clone()) else {
                return;
            };
            if let Err(e) = conn.call_method(
                Some(name),
                path.as_str(),
                Some(ITEM_IFACE),
                "Scroll",
                &(delta, orientation),
            ) {
                tracing::debug!("tray Scroll on {bus}: {e}");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_string_is_accepted_in_all_three_forms_seen_in_the_wild() {
        assert_eq!(
            split_service("org.kde.StatusNotifierItem-1-1", ":1.42"),
            Some((
                "org.kde.StatusNotifierItem-1-1".to_string(),
                DEFAULT_ITEM_PATH.to_string()
            )),
            "a bare bus name takes the spec's default path"
        );
        assert_eq!(
            split_service("/StatusNotifierItem", ":1.42"),
            Some((":1.42".to_string(), "/StatusNotifierItem".to_string())),
            "an object path means the sender is the bus — the AppIndicator form"
        );
        assert_eq!(
            split_service(":1.7/org/ayatana/NotificationItem/nm_applet", ""),
            Some((
                ":1.7".to_string(),
                "/org/ayatana/NotificationItem/nm_applet".to_string()
            )),
            "some applications pass both at once"
        );
    }

    #[test]
    fn a_path_without_a_sender_is_refused_rather_than_guessed() {
        assert_eq!(split_service("/StatusNotifierItem", ""), None);
        assert_eq!(split_service("", ":1.42"), None);
        assert_eq!(split_service("   ", ":1.42"), None);
    }

    #[test]
    fn argb_pixels_become_rgba_and_a_short_buffer_is_refused() {
        // One opaque red pixel, ARGB32: alpha, red, green, blue.
        let argb = [0xFF, 0xFF, 0x00, 0x00];
        let pixmap = pixmap_from_argb(1, 1, &argb).expect("a well-formed pixel decodes");
        assert_eq!(pixmap.rgba, vec![0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!((pixmap.width, pixmap.height), (1, 1));

        assert!(
            pixmap_from_argb(4, 4, &argb).is_none(),
            "a buffer shorter than the declared size is malformed, not something to draw"
        );
        assert!(pixmap_from_argb(0, 0, &[]).is_none());
        assert!(pixmap_from_argb(-1, 1, &argb).is_none());
    }

    #[test]
    fn an_item_asking_for_attention_swaps_to_its_attention_icon() {
        let mut item = TrayItem {
            icon_name: "normal".to_string(),
            attention_icon_name: "urgent".to_string(),
            ..TrayItem::default()
        };
        assert_eq!(item.icon_reference(), "normal");
        item.status = Status::NeedsAttention;
        assert_eq!(item.icon_reference(), "urgent");

        // An item that asks for attention without shipping a second icon keeps the one it has.
        item.attention_icon_name.clear();
        assert_eq!(item.icon_reference(), "normal");
    }

    #[test]
    fn a_label_always_identifies_the_item() {
        let mut item = TrayItem {
            id: "nm-applet".to_string(),
            ..TrayItem::default()
        };
        assert_eq!(item.label(), "nm-applet", "the id is the last resort");
        item.title = "Network".to_string();
        assert_eq!(item.label(), "Network");
        item.tooltip = "Wired connection 1".to_string();
        assert_eq!(
            item.label(),
            "Wired connection 1",
            "the tooltip is the most specific"
        );
        assert_eq!(TrayItem::default().label(), "");
    }

    #[test]
    fn status_defaults_to_active_for_anything_unrecognised() {
        assert_eq!(Status::parse("Passive"), Status::Passive);
        assert_eq!(Status::parse("NeedsAttention"), Status::NeedsAttention);
        assert_eq!(Status::parse("Active"), Status::Active);
        assert_eq!(
            Status::parse(""),
            Status::Active,
            "an item that reports nothing is shown rather than hidden"
        );
    }

    #[test]
    fn the_activate_cache_forgets_items_that_have_gone_away() {
        let live = vec![(":9.1".to_string(), DEFAULT_ITEM_PATH.to_string())];
        {
            let mut cache = ACTIVATE_CACHE.lock().unwrap();
            cache.clear();
            cache.insert(item_key(":9.1", DEFAULT_ITEM_PATH), true);
            cache.insert(item_key(":9.2", DEFAULT_ITEM_PATH), false);
            cache.insert(item_key(":9.3", DEFAULT_ITEM_PATH), true);
        }

        forget_departed(&live);
        let cache = ACTIVATE_CACHE.lock().unwrap();
        assert_eq!(cache.len(), 1, "only the still-registered item survives");
        assert_eq!(cache.get(&item_key(":9.1", DEFAULT_ITEM_PATH)), Some(&true));
        assert!(
            cache.get(&item_key(":9.2", DEFAULT_ITEM_PATH)).is_none(),
            "an application that exited must not keep an entry for the rest of the session"
        );
    }

    #[test]
    fn a_registry_ignores_a_duplicate_registration_and_forgets_a_dead_owner() {
        let registry = Registry::default();
        assert!(registry.add(":1.5".to_string(), DEFAULT_ITEM_PATH.to_string()));
        assert!(
            !registry.add(":1.5".to_string(), DEFAULT_ITEM_PATH.to_string()),
            "an application that registers twice must not appear twice"
        );
        assert!(registry.add(":1.6".to_string(), DEFAULT_ITEM_PATH.to_string()));
        assert_eq!(registry.snapshot().len(), 2);

        assert!(registry.remove_owner(":1.5"));
        assert_eq!(
            registry.snapshot(),
            vec![(":1.6".to_string(), DEFAULT_ITEM_PATH.to_string())]
        );
        assert!(
            !registry.remove_owner(":1.5"),
            "removing a stranger changes nothing"
        );
    }
}
