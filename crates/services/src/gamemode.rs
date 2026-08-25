//! Feral's GameMode: whether the machine is in its performance profile, and a switch to hold it there.
//!
//! GameMode is a reference count, not a flag. Games register themselves while they run and the daemon applies the governor and scheduling changes for as long as at least one client is registered. So "turn game mode on" from a shell means registering a client of its own — the shell's process — and "off" means dropping it, which is exactly what `gamemoded -r` / `-u` do and why the toggle here reports separately whether *this* shell is the one holding it: a game that is already running keeps game mode on whatever the shell does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;

use util::broadcast::{Broadcast, Service};

const GAMEMODE: &str = "com.feralinteractive.GameMode";
const GAMEMODE_PATH: &str = "/com/feralinteractive/GameMode";

/// Registering applies a CPU governor change and a scheduling policy, which is not instant; a bound keeps a wedged daemon from parking the acting thread rather than making the call fast.
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);

/// A registration burst is a game starting, which emits both the signal and a property change.
const COALESCE: Duration = Duration::from_millis(80);

/// Whether this shell is the one holding game mode. Kept here rather than derived from `clients`, which cannot distinguish the shell's own registration from a game's.
static HELD: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameMode {
    /// Whether `gamemoded` is on the bus at all. False on a machine without it, which is what lets a toggle grey out instead of failing silently.
    pub available: bool,
    /// At least one client is registered — the machine is in the performance profile.
    pub active: bool,
    pub clients: i32,
    /// This shell registered one of those clients, so the toggle knows which way it points.
    pub held: bool,
}

fn connection() -> Option<Connection> {
    crate::bus::session(Some(METHOD_TIMEOUT))
}

/// The number of registered clients, or `None` when the daemon isn't there.
fn client_count(conn: &Connection) -> Option<i32> {
    let reply = conn
        .call_method(
            Some(GAMEMODE),
            GAMEMODE_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(GAMEMODE, "ClientCount"),
        )
        .ok()?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    i32::try_from(value).ok()
}

fn read(conn: &Connection) -> GameMode {
    match client_count(conn) {
        Some(clients) => GameMode {
            available: true,
            active: clients > 0,
            clients,
            held: HELD.load(Ordering::Relaxed),
        },
        None => GameMode::default(),
    }
}

static GAME_MODE: Service<GameMode> = Service::new("hogar-shell-gamemode", run);

fn run(out: &Arc<Broadcast<GameMode>>) {
    let Some(conn) = connection() else {
        out.publish(GameMode::default());
        return;
    };
    let mut last = read(&conn);
    out.publish(last);

    let (tx, rx) = sync_channel::<()>(1);
    if watch_signals(tx).is_none() {
        tracing::warn!("gamemode: no signal subscription; state will not update");
        return;
    }
    while rx.recv().is_ok() {
        std::thread::sleep(COALESCE);
        while rx.try_recv().is_ok() {}
        let current = read(&conn);
        if current != last {
            last = current;
            out.publish(current);
        }
    }
}

/// Parks on every signal the daemon emits — `GameRegistered`, `GameUnregistered` and the property change that accompanies them — and pings the refresher. `NameOwnerChanged` is watched too, so a daemon that is started or stopped after the shell is noticed rather than leaving the state frozen at "unavailable".
fn watch_signals(ping: SyncSender<()>) -> Option<()> {
    let from_daemon = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(GAMEMODE)
        .ok()?
        .build();
    let ownership = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.DBus")
        .ok()?
        .member("NameOwnerChanged")
        .ok()?
        .arg(0, GAMEMODE)
        .ok()?
        .build();
    // A rule per thread, because a match rule cannot express "either of these": one iterator would have to be broad enough to wake on every signal on the session bus, which on a desktop is a great many.
    park_on(from_daemon, "hogar-shell-gamemode-signals", ping.clone())?;
    if park_on(ownership, "hogar-shell-gamemode-owner", ping).is_none() {
        tracing::warn!("gamemode: cannot watch for the daemon appearing or going away");
    }
    Some(())
}

fn park_on(rule: zbus::MatchRule<'static>, thread: &str, ping: SyncSender<()>) -> Option<()> {
    let conn = connection()?;
    let signals = MessageIterator::for_match_rule(rule, &conn, None).ok()?;
    std::thread::Builder::new()
        .name(thread.to_string())
        .spawn(move || {
            for _ in signals {
                // A full channel already carries an unserviced ping, so dropping this one loses nothing.
                let _ = ping.try_send(());
            }
        })
        .ok()?;
    Some(())
}

pub fn subscribe(tx: EventSender<GameMode>) {
    GAME_MODE.subscribe(tx);
}

pub fn current() -> Option<GameMode> {
    GAME_MODE.current()
}

/// Registers or drops this shell's own client. Off the UI thread: registering makes the daemon re-apply the governor, which is a privileged round-trip, not something to do on a frame.
pub fn set_held(held: bool) {
    if HELD.swap(held, Ordering::Relaxed) == held {
        return;
    }
    if let Some(state) = current() {
        GAME_MODE.publish(GameMode {
            held,
            // Optimistic: the daemon's own count arrives as a signal a moment later and reconciles this.
            active: state.active || held,
            ..state
        });
    }
    let method = if held {
        "RegisterGame"
    } else {
        "UnregisterGame"
    };
    let pid = std::process::id() as i32;
    let _ = std::thread::Builder::new()
        .name("hogar-shell-gamemode-act".to_string())
        .spawn(move || {
            let Some(conn) = connection() else {
                tracing::warn!("gamemode: cannot reach the session bus");
                return;
            };
            if let Err(e) = conn.call_method(
                Some(GAMEMODE),
                GAMEMODE_PATH,
                Some(GAMEMODE),
                method,
                &(pid,),
            ) {
                tracing::warn!("gamemode: {method}({pid}): {e}");
            }
        });
}

/// Toggles the shell's own hold. A game that registered itself keeps game mode on regardless, which is why this follows [`GameMode::held`] rather than [`GameMode::active`] — otherwise pressing it while a game runs would look like it did nothing.
pub fn toggle() {
    set_held(!HELD.load(Ordering::Relaxed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_daemon_reads_as_unavailable_rather_than_off() {
        // The distinction the UI needs: "no gamemoded here" greys the toggle out, "gamemoded says zero clients" offers to turn it on.
        let absent = GameMode::default();
        assert!(!absent.available && !absent.active);

        let idle = GameMode {
            available: true,
            clients: 0,
            active: false,
            held: false,
        };
        assert!(idle.available && !idle.active);
    }

    #[test]
    fn the_shells_hold_is_tracked_apart_from_the_daemons_count() {
        // A game registers itself; game mode is on, but the shell is not what is holding it, so its toggle still reads as off and pressing it adds the shell's own client rather than doing nothing.
        let game_running = GameMode {
            available: true,
            clients: 1,
            active: true,
            held: false,
        };
        assert!(game_running.active);
        assert!(!game_running.held);
    }
}
