//! The active power profile, from power-profiles-daemon.
//!
//! Read-only: the reading is what a source shows and what `power_profile_changed` announces. Only the daemon's current bus name is watched, `org.freedesktop.UPower.PowerProfiles` (power-profiles-daemon 0.20 and later, and tuned-ppd); a daemon old enough to own `net.hadess.PowerProfiles` alone reads as unavailable, which is what the dependency row reports too.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use platform_wayland::EventSender;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedValue;

use util::broadcast::{Broadcast, Service};

use crate::events::{self, Edge, ShellEvent};

const DAEMON: &str = "org.freedesktop.UPower.PowerProfiles";
const DAEMON_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// A bound so a wedged daemon cannot park the producer.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A profile switch arrives as a burst — `ActiveProfile` and `ActiveProfileHolds`, sometimes a `PerformanceDegraded` — folded into one re-read.
const COALESCE: Duration = Duration::from_millis(80);

/// How often the profile is read when the daemon's signals cannot be watched. Slow, since a profile only changes when someone switches it.
const FALLBACK_POLL: Duration = Duration::from_secs(5);

/// How often the producer looks in on whether anyone still wants the reading, which bounds how long it outlives its last subscriber.
const LOOK_IN: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerProfile {
    /// Whether the daemon answered. False on a machine without it, which is what lets a reading say so rather than show an empty name.
    pub available: bool,
    /// The daemon's own name for the profile — `performance`, `balanced`, `power-saver` — or empty when unavailable.
    pub active: String,
}

fn connection() -> Option<Connection> {
    crate::bus::system(Some(READ_TIMEOUT))
}

fn read(conn: &Connection) -> PowerProfile {
    let active = conn
        .call_method(
            Some(DAEMON),
            DAEMON_PATH,
            Some(PROPERTIES),
            "Get",
            &(DAEMON, "ActiveProfile"),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<OwnedValue>().ok())
        .and_then(|value| String::try_from(value).ok());
    match active {
        Some(active) => PowerProfile {
            available: true,
            active,
        },
        None => PowerProfile::default(),
    }
}

static POWER_PROFILE: Service<PowerProfile> = Service::new("hogar-shell-power-profile", run);

/// The profile in the last reading published.
static ACTIVE: Edge<String> = Edge::new();

fn run(out: &Arc<Broadcast<PowerProfile>>) {
    let watch = Watch::start();
    if watch.is_none() {
        tracing::warn!(
            "power profiles: no signal subscription; the profile is read every {}s",
            FALLBACK_POLL.as_secs()
        );
    }
    follow(
        out,
        watch.as_ref().map(|watch| &watch.pings),
        FALLBACK_POLL,
        || connection().map_or_else(PowerProfile::default, |conn| read(&conn)),
    );
}

/// Publishes the profile, then reads it again whenever `pings` says it may have changed — or every `poll` once nothing can ping — until nobody wants it.
fn follow(
    out: &Broadcast<PowerProfile>,
    mut pings: Option<&Receiver<()>>,
    poll: Duration,
    read: impl Fn() -> PowerProfile,
) {
    let mut last = read();
    out.publish(last.clone());
    announce(&last);
    let mut due = Instant::now() + poll;
    while out.wanted() {
        let changed = match pings {
            Some(signals) => match signals.recv_timeout(LOOK_IN) {
                Ok(()) => {
                    std::thread::sleep(COALESCE);
                    while signals.try_recv().is_ok() {}
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::warn!(
                        "power profiles: the signal subscription ended; the profile is read every {}s",
                        poll.as_secs()
                    );
                    pings = None;
                    true
                }
            },
            None => {
                std::thread::sleep(LOOK_IN.min(due.saturating_duration_since(Instant::now())));
                Instant::now() >= due
            }
        };
        if !changed {
            continue;
        }
        due = Instant::now() + poll;
        let current = read();
        if current != last {
            last = current.clone();
            out.publish(current);
            announce(&last);
        }
    }
}

fn announce(reading: &PowerProfile) {
    if let Some(event) = profile_event(&ACTIVE, reading) {
        events::emit(event);
    }
}

/// A reading with the daemon gone says nothing about the profile, so it neither announces nor resets what was last seen.
fn profile_event(active: &Edge<String>, reading: &PowerProfile) -> Option<ShellEvent> {
    if !reading.available {
        return None;
    }
    active
        .observe(reading.active.clone())
        .map(|profile| ShellEvent::PowerProfileChanged { profile })
}

/// The daemon's property changes and its name changing hands, watched on a connection of their own and passed on as pings; a daemon started or restarted after the shell is then read rather than left at "unavailable".
///
/// The connection is only ever drained, never called on, for the reason `mpris` gives. Dropping the watch closes it, which ends the thread parked on its messages: a producer that stops takes its watch with it.
struct Watch {
    pings: Receiver<()>,
    conn: Connection,
}

impl Watch {
    fn start() -> Option<Self> {
        let from_daemon = zbus::MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(DAEMON)
            .ok()?
            .path(DAEMON_PATH)
            .ok()?
            .build();
        let ownership = zbus::MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface("org.freedesktop.DBus")
            .ok()?
            .member("NameOwnerChanged")
            .ok()?
            .arg(0, DAEMON)
            .ok()?
            .build();
        let conn = crate::bus::private_system(None)?;
        let dbus = DBusProxy::new(&conn).ok()?;
        dbus.add_match_rule(from_daemon).ok()?;
        if dbus.add_match_rule(ownership).is_err() {
            tracing::warn!("power profiles: cannot watch for the daemon appearing or going away");
        }
        let signals = MessageIterator::from(&conn);
        let (ping, pings) = sync_channel(1);
        std::thread::Builder::new()
            .name("hogar-shell-power-profile-signals".to_string())
            .spawn(move || {
                for message in signals.flatten() {
                    if message.message_type() == MessageType::Signal
                        && let Err(TrySendError::Disconnected(())) = ping.try_send(())
                    {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Self { pings, conn })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.conn.clone().close();
    }
}

/// Registers `tx` for the active profile, starting the single shared producer on first use.
pub fn subscribe(tx: EventSender<PowerProfile>) {
    POWER_PROFILE.subscribe(tx);
}

/// The last published reading, with no round-trip.
pub fn current() -> Option<PowerProfile> {
    POWER_PROFILE.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(profile: &str) -> PowerProfile {
        PowerProfile {
            available: true,
            active: profile.to_string(),
        }
    }

    #[test]
    fn a_profile_is_announced_only_when_the_daemon_switches_it() {
        let active = Edge::new();
        let announced: Vec<Option<ShellEvent>> = [
            on("balanced"),
            on("balanced"),
            on("power-saver"),
            PowerProfile::default(),
            on("power-saver"),
            on("performance"),
        ]
        .iter()
        .map(|reading| profile_event(&active, reading))
        .collect();
        assert_eq!(
            announced,
            [
                None,
                None,
                Some(ShellEvent::PowerProfileChanged {
                    profile: "power-saver".to_string()
                }),
                None,
                None,
                Some(ShellEvent::PowerProfileChanged {
                    profile: "performance".to_string()
                }),
            ],
            "the first reading and the daemon going away are not switches"
        );
    }

    fn running(name: &str) -> bool {
        util::broadcast::running_services()
            .iter()
            .any(|running| running == name)
    }

    fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    /// With no watch, the producer reads on a timer — and, like every producer that can, stops once nobody listens rather than reading for nobody for the life of the shell.
    #[test]
    fn without_a_watch_it_polls_while_wanted_and_stops_with_its_last_subscriber() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static READS: AtomicUsize = AtomicUsize::new(0);
        static POLLED: Service<PowerProfile> = Service::new("test-power-profile-polled", |out| {
            follow(out, None, Duration::from_millis(10), || {
                READS.fetch_add(1, Ordering::SeqCst);
                PowerProfile::default()
            })
        });

        let (tx, subscription) = platform_wayland::detached();
        POLLED.subscribe(tx);
        eventually("a few reads", || READS.load(Ordering::SeqCst) >= 3);
        assert!(running("test-power-profile-polled"));

        drop(subscription);
        eventually("the producer to retire", || {
            !running("test-power-profile-polled")
        });
        let idle = READS.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            READS.load(Ordering::SeqCst),
            idle,
            "nothing reads for nobody"
        );

        let (tx, _subscription) = platform_wayland::detached();
        POLLED.subscribe(tx);
        eventually("a fresh producer", || READS.load(Ordering::SeqCst) > idle);
    }

    /// A ping is a re-read; a watch that dies is no reason to stop reading, nor to keep a producer that will never read again.
    #[test]
    fn a_ping_reads_again_and_a_watch_that_ends_falls_back_to_polling() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc::SyncSender;
        static READS: AtomicUsize = AtomicUsize::new(0);
        static PINGS: Mutex<Option<Receiver<()>>> = Mutex::new(None);
        static PINGED: Service<PowerProfile> = Service::new("test-power-profile-pinged", |out| {
            let pings = PINGS.lock().unwrap().take();
            follow(out, pings.as_ref(), Duration::from_millis(10), || {
                READS.fetch_add(1, Ordering::SeqCst);
                PowerProfile::default()
            })
        });

        let (ping, pings): (SyncSender<()>, _) = sync_channel(1);
        *PINGS.lock().unwrap() = Some(pings);
        let (tx, subscription) = platform_wayland::detached();
        PINGED.subscribe(tx);
        eventually("the first read", || READS.load(Ordering::SeqCst) == 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            READS.load(Ordering::SeqCst),
            1,
            "a watched profile is read on a ping, not on a timer"
        );

        ping.send(()).unwrap();
        eventually("the read a ping asks for", || {
            READS.load(Ordering::SeqCst) == 2
        });

        drop(ping);
        eventually("polling once the watch is gone", || {
            READS.load(Ordering::SeqCst) >= 4
        });

        drop(subscription);
        eventually("the producer to retire", || {
            !running("test-power-profile-pinged")
        });
    }
}
