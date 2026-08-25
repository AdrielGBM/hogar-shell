//! One D-Bus connection per (bus, method timeout) instead of one per call.
//!
//! Opening a connection is not cheap: a socket, an auth handshake, an executor thread and a message queue. A service that opens one inside a poll loop or a click handler pays that several times a second, and every live connection costs a thread whose allocations glibc scatters across its own malloc arena.
//!
//! The timeout is part of the key because it is a per-connection setting in zbus and the services mean different things by it — a tray read gives up after 2 s, a VPN action is allowed 60 s. Collapsing those onto one connection would silently retime every call made through it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    System,
    Session,
}

type Slot = Arc<Mutex<Option<Connection>>>;
type Slots = Mutex<HashMap<(Kind, Option<Duration>), Slot>>;

fn slots() -> &'static Slots {
    static SLOTS: OnceLock<Slots> = OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

/// The shared system-bus connection for `timeout`, or `None` when the bus is unreachable.
pub fn system(timeout: Option<Duration>) -> Option<Connection> {
    shared(Kind::System, timeout)
}

/// The shared session-bus connection for `timeout`, or `None` when the bus is unreachable.
///
/// Not for a connection that owns a well-known name or serves objects — those are the connection's identity on the bus, so the notification server and the tray watcher/host keep their own.
pub fn session(timeout: Option<Duration>) -> Option<Connection> {
    shared(Kind::Session, timeout)
}

/// A session connection of its own, outside the shared pool.
///
/// For a caller that drains a connection's message stream: a blocking call made on the connection being drained deadlocks, because zbus queues every incoming message for the stream and its socket reader stops reading once that queue is full — with the awaited reply behind it.
pub fn private_session(timeout: Option<Duration>) -> Option<Connection> {
    build(Kind::Session, timeout)
}

fn shared(kind: Kind, timeout: Option<Duration>) -> Option<Connection> {
    let slot = slots()
        .lock()
        .ok()?
        .entry((kind, timeout))
        .or_default()
        .clone();
    // The map lock is released before the handshake: a bus that is slow to answer must not hold up a caller asking for a different connection.
    let mut slot = slot.lock().ok()?;
    if let Some(conn) = slot.as_ref() {
        return Some(conn.clone());
    }
    let conn = build(kind, timeout)?;
    *slot = Some(conn.clone());
    Some(conn)
}

// A failure is deliberately not cached: a service that isn't up yet, or a bus that isn't there on this machine, is asked again on the next call rather than written off for the life of the process.
fn build(kind: Kind, timeout: Option<Duration>) -> Option<Connection> {
    let builder = match kind {
        Kind::System => Builder::system(),
        Kind::Session => Builder::session(),
    }
    .ok()?;
    match timeout {
        Some(t) => builder.method_timeout(t),
        None => builder,
    }
    .build()
    .ok()
}
