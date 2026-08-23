//! VPN tunnels: what is configured, what is up, and switching between the two.
//!
//! Two sources, because a desktop has two kinds of tunnel and they know nothing about each other.
//! NetworkManager owns the ones with a profile — OpenVPN, WireGuard imported into NM, corporate IPsec — and
//! reports them over D-Bus. Raw `wg-quick` interfaces owned by systemd or a script are invisible there and show
//! up only in the kernel, under `/sys/class/net/<iface>` with a `wireguard` device type.
//!
//! Both are listed, tagged with where they came from, and toggled through whichever mechanism owns them.
//! Anything else would leave half a user's tunnels unlistable depending on how they set them up.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use util::broadcast::{Broadcast, Service};

const NM_BUS: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

const NET_DIR: &str = "/sys/class/net";

const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Bringing a tunnel up waits on a handshake with a server that may be far away.
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const COALESCE: Duration = Duration::from_millis(120);

/// How often the kernel side is re-read. WireGuard interfaces appear and disappear with no event source at
/// all, so this is a poll — but only of a directory listing, and only while something is subscribed.
const KERNEL_POLL: Duration = Duration::from_secs(5);

/// Who owns a tunnel, which decides how it is switched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    #[default]
    NetworkManager,
    /// A `wireguard` device in the kernel with no NetworkManager profile — `wg-quick` or systemd put it there.
    Kernel,
}

/// One tunnel the shell can list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tunnel {
    /// NetworkManager's connection path, or the interface name for a kernel tunnel. The handle actions take.
    pub id: String,
    /// What to call it in a list.
    pub name: String,
    /// `wireguard`, `openvpn`, `vpn` — NetworkManager's service type, or `wireguard` for a kernel tunnel.
    pub kind: String,
    pub owner: Owner,
    pub active: bool,
}

/// Every tunnel, and whether any of them is up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vpn {
    /// NetworkManager is on the bus. A kernel-only machine still lists its WireGuard interfaces.
    pub available: bool,
    pub tunnels: Vec<Tunnel>,
}

impl Vpn {
    pub fn is_connected(&self) -> bool {
        self.tunnels.iter().any(|t| t.active)
    }

    /// The tunnel a one-line summary is about: the first active one, in listed order.
    pub fn active(&self) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.active)
    }

    pub fn tunnel(&self, id: &str) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.id == id)
    }
}

fn connection(timeout: Duration) -> Option<Connection> {
    crate::bus::system(Some(timeout))
}

fn property(conn: &Connection, path: &str, iface: &str, name: &str) -> Option<OwnedValue> {
    conn.call_method(Some(NM_BUS), path, Some(PROPERTIES), "Get", &(iface, name))
        .ok()?
        .body()
        .deserialize()
        .ok()
}

/// The `connection` group of a saved profile, which is where its id and type live.
fn profile(conn: &Connection, path: &str) -> Option<(String, String)> {
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
    let group = settings.get("connection")?;
    let kind = String::try_from(group.get("type")?.try_clone().ok()?).ok()?;
    let id = String::try_from(group.get("id")?.try_clone().ok()?).ok()?;
    Some((id, kind))
}

/// Whether a NetworkManager connection type is a tunnel. `vpn` covers every plugin (OpenVPN, OpenConnect,
/// IPsec); `wireguard` is its own type because NetworkManager implements it natively rather than as a plugin.
fn is_tunnel(kind: &str) -> bool {
    matches!(kind, "vpn" | "wireguard")
}

/// The connection paths NetworkManager currently has active, so a profile can be marked as up.
fn active_paths(conn: &Connection) -> Vec<String> {
    let Some(value) = property(conn, NM_PATH, NM_IFACE, "ActiveConnections") else {
        return Vec::new();
    };
    let Ok(actives) = Vec::<OwnedObjectPath>::try_from(value) else {
        return Vec::new();
    };
    actives
        .iter()
        .filter_map(|active| {
            let value = property(conn, active.as_str(), ACTIVE_IFACE, "Connection")?;
            OwnedObjectPath::try_from(value)
                .ok()
                .map(|p| p.as_str().to_string())
        })
        .collect()
}

fn read_networkmanager(conn: &Connection) -> Vec<Tunnel> {
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
    let active = active_paths(conn);
    paths
        .iter()
        .filter_map(|path| {
            let path = path.as_str().to_string();
            let (name, kind) = profile(conn, &path)?;
            is_tunnel(&kind).then(|| Tunnel {
                active: active.contains(&path),
                owner: Owner::NetworkManager,
                id: path,
                name,
                kind,
            })
        })
        .collect()
}

/// WireGuard interfaces the kernel knows about but NetworkManager does not.
///
/// `/sys/class/net/<iface>/uevent` carries `DEVTYPE=wireguard`, which is the kernel's own answer and needs no
/// name-shape guessing — an interface called `wg0` might be anything, and a tunnel might be called `office`.
/// An interface only counts as up when it is actually carrying traffic, which for WireGuard means `operstate`
/// reads `unknown` (it is point-to-point and never reports `up`) *and* the link is not down.
fn read_kernel(known: &[Tunnel]) -> Vec<Tunnel> {
    let Ok(entries) = fs::read_dir(NET_DIR) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if known.iter().any(|t| t.name == name) {
                return None;
            }
            let uevent = fs::read_to_string(entry.path().join("uevent")).ok()?;
            if !uevent
                .lines()
                .any(|line| line.trim() == "DEVTYPE=wireguard")
            {
                return None;
            }
            Some(Tunnel {
                active: is_link_up(&entry.path()),
                owner: Owner::Kernel,
                kind: "wireguard".to_string(),
                id: name.clone(),
                name,
            })
        })
        .collect()
}

fn is_link_up(device: &Path) -> bool {
    fs::read_to_string(device.join("operstate"))
        .map(|s| s.trim() != "down")
        .unwrap_or(false)
}

fn read(conn: Option<&Connection>) -> Vpn {
    let mut tunnels = conn.map(read_networkmanager).unwrap_or_default();
    tunnels.extend(read_kernel(&tunnels));
    tunnels.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Vpn {
        available: conn.is_some(),
        tunnels,
    }
}

static VPN: Service<Vpn> = Service::new("hogar-shell-vpn", run);

fn run(out: &Arc<Broadcast<Vpn>>) {
    let conn = connection(READ_TIMEOUT);
    let mut last = read(conn.as_ref());
    out.publish(last.clone());

    let (tx, rx) = sync_channel::<()>(1);
    if conn.is_some() && watch_signals(tx.clone()).is_none() {
        tracing::warn!("vpn: no NetworkManager signals; the list will only refresh on the timer");
    }
    // The kernel side has no event source, so a slow timer covers it — and doubles as the fallback that keeps
    // the list live on a machine with no NetworkManager at all.
    let _ = std::thread::Builder::new()
        .name("hogar-shell-vpn-poll".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(KERNEL_POLL);
                if tx.try_send(()).is_err() {
                    continue;
                }
            }
        });

    while rx.recv().is_ok() {
        std::thread::sleep(COALESCE);
        while rx.try_recv().is_ok() {}
        let current = read(conn.as_ref());
        if current != last {
            last = current.clone();
            out.publish(current);
        }
    }
}

fn watch_signals(ping: SyncSender<()>) -> Option<()> {
    let conn = connection(READ_TIMEOUT)?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(NM_BUS)
        .ok()?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).ok()?;
    std::thread::Builder::new()
        .name("hogar-shell-vpn-signals".to_string())
        .spawn(move || {
            for _ in signals {
                let _ = ping.try_send(());
            }
        })
        .ok()?;
    Some(())
}

pub fn subscribe(tx: EventSender<Vpn>) {
    VPN.subscribe(tx);
}

pub fn current() -> Option<Vpn> {
    VPN.current()
}

fn act(what: &'static str, job: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-vpn-act".to_string())
        .spawn(move || {
            tracing::debug!("vpn: {what}");
            job();
        });
}

/// Brings `id` up or down, through whichever mechanism owns it.
pub fn set_active(id: &str, up: bool) {
    let Some(tunnel) = current().and_then(|v| v.tunnel(id).cloned()) else {
        return;
    };
    if let Some(state) = current() {
        let tunnels = state
            .tunnels
            .iter()
            .map(|t| Tunnel {
                active: if t.id == tunnel.id { up } else { t.active },
                ..t.clone()
            })
            .collect();
        VPN.publish(Vpn { tunnels, ..state });
    }
    match tunnel.owner {
        Owner::NetworkManager => act("switching an NM tunnel", move || {
            switch_networkmanager(&tunnel, up)
        }),
        // `wg-quick` needs root, so this goes through the same detached shell every other launch uses rather
        // than pretending the shell can raise an interface itself.
        Owner::Kernel => act("switching a wg-quick tunnel", move || {
            let verb = if up { "up" } else { "down" };
            crate::apps::run_detached(format!("wg-quick {verb} {}", tunnel.name));
        }),
    }
}

fn switch_networkmanager(tunnel: &Tunnel, up: bool) {
    let Some(conn) = connection(ACTION_TIMEOUT) else {
        tracing::warn!("vpn: cannot reach NetworkManager");
        return;
    };
    let Ok(path) = zbus::zvariant::ObjectPath::try_from(tunnel.id.as_str()) else {
        return;
    };
    let root = zbus::zvariant::ObjectPath::from_static_str_unchecked("/");
    let result = if up {
        conn.call_method(
            Some(NM_BUS),
            NM_PATH,
            Some(NM_IFACE),
            "ActivateConnection",
            &(path, root.clone(), root),
        )
    } else {
        match active_handle(&conn, &tunnel.id) {
            Some(active) => conn.call_method(
                Some(NM_BUS),
                NM_PATH,
                Some(NM_IFACE),
                "DeactivateConnection",
                &(active,),
            ),
            None => return,
        }
    };
    if let Err(e) = result {
        tracing::warn!("vpn: switching '{}': {e}", tunnel.name);
    }
}

/// The *active-connection* object for a saved profile. Deactivation takes that, not the profile itself — the
/// profile is the recipe, the active connection is the running instance.
fn active_handle(conn: &Connection, profile_path: &str) -> Option<OwnedObjectPath> {
    let value = property(conn, NM_PATH, NM_IFACE, "ActiveConnections")?;
    let actives = Vec::<OwnedObjectPath>::try_from(value).ok()?;
    actives.into_iter().find(|active| {
        property(conn, active.as_str(), ACTIVE_IFACE, "Connection")
            .and_then(|v| OwnedObjectPath::try_from(v).ok())
            .is_some_and(|p| p.as_str() == profile_path)
    })
}

/// Brings the first active tunnel down, or the first listed one up. What a single quick toggle does.
pub fn toggle() {
    let Some(state) = current() else { return };
    match state.active() {
        Some(active) => set_active(&active.id.clone(), false),
        None => {
            if let Some(first) = state.tunnels.first() {
                set_active(&first.id.clone(), true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel(name: &str, active: bool, owner: Owner) -> Tunnel {
        Tunnel {
            id: name.to_string(),
            name: name.to_string(),
            kind: "wireguard".to_string(),
            owner,
            active,
        }
    }

    #[test]
    fn only_tunnel_connection_types_are_listed() {
        assert!(is_tunnel("vpn"), "every plugin reports the generic type");
        assert!(
            is_tunnel("wireguard"),
            "which NetworkManager implements natively"
        );
        for ordinary in ["802-11-wireless", "802-3-ethernet", "bridge", "loopback"] {
            assert!(!is_tunnel(ordinary), "'{ordinary}' is not a tunnel");
        }
    }

    #[test]
    fn a_kernel_tunnel_is_not_listed_twice_when_networkmanager_owns_it() {
        // NetworkManager names its WireGuard connections after the interface it creates, so without this the
        // same tunnel appears once per source — with two different switches, one of which would not work.
        let from_nm = vec![tunnel("wg0", true, Owner::NetworkManager)];
        assert!(
            read_kernel(&from_nm).iter().all(|t| t.name != "wg0"),
            "a tunnel NetworkManager already reported is skipped"
        );
    }

    #[test]
    fn the_active_tunnel_leads_the_list() {
        let mut state = Vpn {
            available: true,
            tunnels: vec![
                tunnel("alpha", false, Owner::NetworkManager),
                tunnel("zulu", true, Owner::Kernel),
                tunnel("beta", false, Owner::NetworkManager),
            ],
        };
        state.tunnels.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let names: Vec<&str> = state.tunnels.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["zulu", "alpha", "beta"]);
        assert!(state.is_connected());
        assert_eq!(state.active().map(|t| t.name.as_str()), Some("zulu"));
    }

    #[test]
    fn nothing_configured_is_not_the_same_as_nothing_running() {
        let empty = Vpn::default();
        assert!(!empty.available, "no NetworkManager and no kernel tunnels");
        assert!(!empty.is_connected());
        assert!(empty.active().is_none());
    }

    #[test]
    fn reading_the_kernel_side_never_panics_on_this_machine() {
        for tunnel in read_kernel(&[]) {
            assert_eq!(tunnel.owner, Owner::Kernel);
            assert!(!tunnel.name.is_empty());
        }
    }
}
