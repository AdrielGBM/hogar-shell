//! The shared-source primitive every system service is built on.
//!
//! A service owns exactly one producer — a D-Bus subscription, a socket, a watcher process — running on its own
//! thread for the whole shell. Surfaces don't read the system; they subscribe, and the producer fans each
//! reading out to all of them. N bars therefore cost one connection and one parse per change, not N, and a
//! surface never runs a timer of its own.
//!
//! A module consumes one by handing [`Service::subscribe`] to `platform_wayland::watch`, which delivers each
//! value on that surface's own loop thread and unsubscribes it when the surface goes away.
//!
//! **A producer that nobody is listening to has to stop.** Starting lazily is only half the rule: a service
//! whose last subscriber went away — the bar module switched off in a reload, the panel that closed — kept its
//! thread, its connection and its poll timer for the life of the process. A polling producer with no subscriber
//! is the clearest form of that. [`Broadcast::wanted`] is how a producer asks, and returning from the producer
//! when it answers `false` is how a service opts in; a later subscription starts a fresh one.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use platform_wayland::EventSender;

/// Producer threads alive right now, by thread name, counted rather than flagged so a restart arriving before
/// the outgoing thread has returned does not un-list the incoming one.
static RUNNING: Mutex<BTreeMap<&'static str, usize>> = Mutex::new(BTreeMap::new());

/// The producer threads running right now. The census behind `shell status`: a service still listed when
/// nothing on the bar asks for it is exactly the leak that command exists to make visible from a script.
pub fn running_services() -> Vec<&'static str> {
    RUNNING.lock().unwrap().keys().copied().collect()
}

fn producer_started(name: &'static str) {
    *RUNNING.lock().unwrap().entry(name).or_insert(0) += 1;
}

fn producer_finished(name: &'static str) {
    let mut running = RUNNING.lock().unwrap();
    if let Some(count) = running.get_mut(name) {
        *count -= 1;
        if *count == 0 {
            running.remove(name);
        }
    }
}

/// The current reading plus the surfaces listening for the next one.
pub struct Broadcast<T> {
    current: Mutex<Option<T>>,
    subscribers: Mutex<Vec<EventSender<T>>>,
    /// Plain-channel listeners, for another *producer* thread rather than a surface.
    ///
    /// A surface reads through `EventSender`, which only the driver can hand out — so a producer that has to
    /// react to a service instead of to the system had no way to wait for one, and could only poll. The
    /// wallpaper surface is the case that needs it: the moment the choice changes, something has to decode a
    /// full-resolution image, and that something must be neither the UI thread nor a timer.
    listeners: Mutex<Vec<mpsc::Sender<T>>>,
    /// Whether a producer thread is live, so a second subscription does not start a second one. Cleared only by
    /// [`wanted`](Self::wanted), which is what lets a later subscriber start a fresh producer.
    running: Mutex<bool>,
}

impl<T: Clone> Broadcast<T> {
    fn new() -> Self {
        Self {
            current: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            listeners: Mutex::new(Vec::new()),
            running: Mutex::new(false),
        }
    }

    /// Whether anything is still listening — and, when nothing is, the point at which the producer is released
    /// so that a later subscription starts a new one. A producer that polls asks this once a turn and returns
    /// when it answers `false`.
    ///
    /// **This is opt-in, and deliberately so.** A producer that never asks behaves exactly as every producer
    /// did before it existed: started once, never stopped. That is the right answer for one that registers a
    /// callback and returns *and has no way to take the registration back* — its work outlives the call, so
    /// releasing the flag would let a second subscriber start a second D-Bus connection — and it makes
    /// converting the rest a service at a time.
    ///
    /// A producer that *can* take it back may ask like any other, and `services::hyprland` does: it hands each
    /// registration a `platform_wayland::Interest` and retires it in the same breath as answering `false`, so
    /// the callback is dropped rather than left to be called by a watcher nobody wants. The rule that makes
    /// that safe is one token per producer run — a producer registered in two places whose registrations retire
    /// one at a time is exactly the second-producer bug below, reached the long way round.
    ///
    /// **A `false` is final: return, and do not ask again.** Answering `false` is the producer giving up its
    /// claim, and the next subscriber is free to start a replacement the instant it does. A producer that asked
    /// a second time could be told `true` by that subscriber's arrival and carry on — leaving two producers on
    /// one service, which is the duplicate-connection bug this whole mechanism exists to avoid.
    ///
    /// **Ask it after publishing, never before.** [`Service::current`] and [`Service::awaited`] start a producer
    /// without subscribing — an IPC `volume get` on a bar with no volume chip — so a producer that asked first
    /// would retire before taking the one reading its caller started it for, and answer `None` about a machine
    /// that had an answer.
    ///
    /// The `running` flag is locked for the whole check because [`Service::subscribe`] takes it *after* pushing
    /// its subscriber. Without that overlap, a subscription landing between "nobody is listening" and "the
    /// producer has stopped" would find a service still marked running and never start one.
    pub fn wanted(&self) -> bool {
        let mut running = self.running.lock().unwrap();
        let listening = {
            let mut subscribers = self.subscribers.lock().unwrap();
            subscribers.retain(EventSender::alive);
            !subscribers.is_empty()
        } || !self.listeners.lock().unwrap().is_empty();
        if listening {
            return true;
        }
        *running = false;
        false
    }

    /// Records `value` as the current reading and fans it out, dropping subscribers whose surface has closed
    /// (their channel receiver is gone, so `send` fails).
    pub fn publish(&self, value: T) {
        *self.current.lock().unwrap() = Some(value.clone());
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(value.clone()));
        self.listeners
            .lock()
            .unwrap()
            .retain(|tx| tx.send(value.clone()).is_ok());
    }

    pub fn current(&self) -> Option<T> {
        self.current.lock().unwrap().clone()
    }
}

/// A lazily-started shared service. The producer thread spins up on the first subscription, so a shell
/// configured without a battery chip never opens a UPower connection — and, for a producer that asks
/// [`Broadcast::wanted`], winds down again when the last subscriber goes away.
///
/// The producer receives the broadcast as an `Arc`, not a borrow, so it can either park on a loop of its own or
/// hand a clone to a callback and return — which is what a service reading off an event stream someone else
/// owns has to do. Either way the broadcast outlives the producer, and the last reading survives a producer
/// that stopped, so a chip that comes back draws it immediately rather than blank.
pub struct Service<T: 'static> {
    cell: OnceLock<Arc<Broadcast<T>>>,
    producer: fn(&Arc<Broadcast<T>>),
    thread_name: &'static str,
}

impl<T: Clone + Send + 'static> Service<T> {
    pub const fn new(thread_name: &'static str, producer: fn(&Arc<Broadcast<T>>)) -> Self {
        Self {
            cell: OnceLock::new(),
            producer,
            thread_name,
        }
    }

    fn broadcast(&'static self) -> &'static Arc<Broadcast<T>> {
        self.cell.get_or_init(|| Arc::new(Broadcast::new()))
    }

    /// Spawns the producer unless one is already live. Idempotent, and safe to call on every subscription.
    fn start(&'static self, broadcast: &'static Arc<Broadcast<T>>) {
        let mut running = broadcast.running.lock().unwrap();
        if *running {
            return;
        }
        let producer = self.producer;
        let name = self.thread_name;
        let owned = Arc::clone(broadcast);
        producer_started(name);
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                producer(&owned);
                producer_finished(name);
            });
        match spawned {
            Ok(_) => *running = true,
            Err(e) => {
                producer_finished(name);
                tracing::warn!("could not start the {name} service: {e}");
            }
        }
    }

    fn started(&'static self) -> &'static Arc<Broadcast<T>> {
        let broadcast = self.broadcast();
        self.start(broadcast);
        broadcast
    }

    /// Registers `tx` for live readings, sending the current one immediately so the surface starts in sync
    /// rather than blank until the next change. Pass this as the producer to `platform_wayland::watch`.
    ///
    /// The subscriber is pushed *before* the producer is started, which is what closes the window against a
    /// producer winding itself down at the same moment — see [`Broadcast::wanted`].
    pub fn subscribe(&'static self, tx: EventSender<T>) {
        let broadcast = self.broadcast();
        if let Some(value) = broadcast.current()
            && !tx.send(value)
        {
            return;
        }
        broadcast.subscribers.lock().unwrap().push(tx);
        self.start(broadcast);
    }

    /// Publishes a reading taken outside the producer thread — used when the shell itself causes the change (a
    /// mute toggle, a brightness step) so the UI reflects it immediately instead of at the producer's next turn.
    pub fn publish(&'static self, value: T) {
        self.started().publish(value);
    }

    /// The last published reading, without touching the system. Lets a UI handler act on the current value (a
    /// scroll stepping from it) without doing blocking I/O — or spawning a process — on the render thread.
    pub fn current(&'static self) -> Option<T> {
        self.started().current()
    }

    /// The last published reading, waiting up to `patience` for the first one when the producer has only just
    /// started.
    ///
    /// **[`current`](Self::current) starts the producer and reads in the same breath**, which is right for a UI
    /// handler — it either has a reading to step from or has nothing to draw — and wrong for a caller that is
    /// itself the reason the service started. An IPC `volume get` on a shell whose bar carries no volume chip
    /// asked a listener that had not had a turn yet, got `None`, and answered "no audio sink available" about a
    /// machine with one; `volume up` did nothing at all and said it had.
    ///
    /// Polled rather than signalled because the wait only ever happens once, on the first read of a service
    /// nothing had subscribed to, and a condvar per broadcast is a lot of machinery for that.
    pub fn awaited(&'static self, patience: Duration) -> Option<T> {
        let service = self.started();
        let deadline = Instant::now() + patience;
        loop {
            if let Some(value) = service.current() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Records `value` as the current reading *without* starting the producer, so a reader sees it and the
    /// thread that would have read the system never runs.
    ///
    /// What a `[preview]` fixture seeds a service with. [`publish`](Self::publish) is the wrong door for that:
    /// it starts the producer, which for the tray means claiming a D-Bus name and then overwriting the seeded
    /// reading with the machine's own — so a preview would draw whatever happens to be running.
    ///
    /// Claiming the producer slot is what enforces the "never runs" half. Every other reader — `current`,
    /// `awaited`, `publish` — starts the producer if none is live, so a seeded service that left the slot open
    /// would grow the very thread the seed exists to avoid the moment a preview read it.
    pub fn seed(&'static self, value: T) {
        let broadcast = self.broadcast();
        *broadcast.running.lock().unwrap() = true;
        broadcast.publish(value);
    }
}

/// A producerless shared value: the same one-writer/N-reader fan-out as [`Service`], for state the shell itself
/// owns — persisted toggles, the current wallpaper, launch counts — rather than reads off the system. Nothing
/// polls and no thread is spawned; the value is seeded by `init` on first touch and changed by [`Store::update`].
pub struct Store<T: 'static> {
    cell: OnceLock<Arc<Broadcast<T>>>,
    init: fn() -> T,
}

impl<T: Clone + Send + 'static> Store<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            cell: OnceLock::new(),
            init,
        }
    }

    fn started(&'static self) -> &'static Arc<Broadcast<T>> {
        self.cell.get_or_init(|| {
            let broadcast = Arc::new(Broadcast::new());
            *broadcast.current.lock().unwrap() = Some((self.init)());
            broadcast
        })
    }

    /// The current value, seeding it on first call.
    pub fn get(&'static self) -> T {
        self.started()
            .current()
            .expect("a store is seeded when it starts")
    }

    /// Applies `change` to the current value and fans the result out. Returns the new value so a caller can
    /// persist it without a second read racing another writer.
    pub fn update(&'static self, change: impl FnOnce(&mut T)) -> T {
        let broadcast = self.started();
        let mut next = broadcast
            .current()
            .expect("a store is seeded when it starts");
        change(&mut next);
        broadcast.publish(next.clone());
        next
    }

    /// Registers `tx` for changes, sending the current value immediately so a surface starts in sync.
    pub fn subscribe(&'static self, tx: EventSender<T>) {
        let broadcast = self.started();
        if let Some(value) = broadcast.current()
            && !tx.send(value)
        {
            return;
        }
        broadcast.subscribers.lock().unwrap().push(tx);
    }

    /// Registers a plain channel for changes, sending the current value immediately. For a producer thread
    /// that has to *wait* on this store rather than poll it; a surface wants [`subscribe`](Self::subscribe).
    pub fn listen(&'static self, tx: mpsc::Sender<T>) {
        let broadcast = self.started();
        if let Some(value) = broadcast.current()
            && tx.send(value).is_err()
        {
            return;
        }
        broadcast.listeners.lock().unwrap().push(tx);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    static TURNS: AtomicUsize = AtomicUsize::new(0);

    /// A poller in the shape every polling service has: take a reading, publish it, ask whether that was worth
    /// doing, and retire when it was not.
    static POLLER: Service<usize> = Service::new("test-poller", |out| {
        loop {
            out.publish(TURNS.fetch_add(1, Ordering::SeqCst));
            if !out.wanted() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    /// Waits for `check` to hold, so a test never depends on how fast a producer thread gets scheduled.
    fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for {what}");
    }

    /// **The shell's standing rule: nothing runs unless something is asking for it.**
    ///
    /// Lazy start was only ever half of it. A service whose last subscriber went away — a module switched off
    /// in a config reload, a panel closed — kept its thread and its poll timer for the life of the process,
    /// reading the system for nobody. Subscribing again has to get a live service back, not a corpse.
    #[test]
    fn a_service_stops_when_its_last_subscriber_goes_and_starts_again_for_the_next() {
        let (tx, subscription) = platform_wayland::detached();
        POLLER.subscribe(tx);
        eventually("the producer to take a turn", || {
            TURNS.load(Ordering::SeqCst) > 0
        });
        assert!(
            running_services().contains(&"test-poller"),
            "a running producer is visible to `shell status`"
        );

        drop(subscription);
        eventually("the producer to retire", || {
            !running_services().contains(&"test-poller")
        });
        let idle = TURNS.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            TURNS.load(Ordering::SeqCst),
            idle,
            "a service with nothing listening takes no readings at all"
        );

        let (tx, subscription) = platform_wayland::detached::<usize>();
        POLLER.subscribe(tx);
        eventually("the producer to start again", || {
            TURNS.load(Ordering::SeqCst) > idle
        });
        assert!(
            subscription.try_recv().is_some(),
            "and the new subscriber is fed by it"
        );
    }

    /// A producer that never asks [`Broadcast::wanted`] keeps the old contract — started once, never stopped —
    /// so a service whose work outlives the producer call cannot be handed a second connection by a second
    /// subscriber.
    #[test]
    fn a_producer_that_never_asks_is_started_exactly_once() {
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        static ONCE: Service<u8> = Service::new("test-once", |out| {
            STARTS.fetch_add(1, Ordering::SeqCst);
            out.publish(1);
        });

        let (first, first_sub) = platform_wayland::detached();
        ONCE.subscribe(first);
        eventually("the producer to run", || STARTS.load(Ordering::SeqCst) == 1);
        drop(first_sub);
        std::thread::sleep(Duration::from_millis(20));

        let (second, _second_sub) = platform_wayland::detached();
        ONCE.subscribe(second);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            STARTS.load(Ordering::SeqCst),
            1,
            "the second subscriber reuses the service rather than starting a second one"
        );
    }

    static COUNTER: Store<u32> = Store::new(|| 7);

    static PRODUCER_RAN: AtomicBool = AtomicBool::new(false);
    static SEEDED: Service<u32> = Service::new("test-seeded", |broadcast| {
        PRODUCER_RAN.store(true, Ordering::SeqCst);
        broadcast.publish(0);
    });

    #[test]
    fn a_seeded_service_answers_readers_without_starting_its_producer() {
        SEEDED.seed(9);
        assert_eq!(SEEDED.current(), Some(9), "a reader sees the seeded value");
        assert!(
            !PRODUCER_RAN.load(Ordering::SeqCst),
            "the thread that would read the system never started"
        );
    }

    #[test]
    fn store_seeds_from_init_and_updates_in_place() {
        assert_eq!(COUNTER.get(), 7, "seeded lazily from `init`");
        assert_eq!(
            COUNTER.update(|n| *n += 5),
            12,
            "update returns the new value"
        );
        assert_eq!(COUNTER.get(), 12, "and it is what later readers see");
    }

    /// **A caller that starts a service is the one caller that cannot use [`Service::current`].**
    ///
    /// `current` starts the producer and reads in the same breath, so the very first read of a service nothing
    /// had subscribed to is always `None` — which reached the user as `volume get` answering "no audio sink
    /// available" on a machine with one, and `volume up` doing nothing and reporting the step it had not taken.
    #[test]
    fn the_first_read_of_a_cold_service_waits_for_it_rather_than_answering_nothing() {
        static SLOW: Service<u8> = Service::new("test-slow", |out| {
            std::thread::sleep(Duration::from_millis(40));
            out.publish(7);
        });

        assert_eq!(
            SLOW.current(),
            None,
            "the producer has been started and has not had a turn: this is the answer that lied"
        );
        assert_eq!(SLOW.awaited(Duration::from_secs(2)), Some(7));
        assert_eq!(
            SLOW.current(),
            Some(7),
            "and it stands for every later read"
        );
    }

    /// The wait is bounded: a service that genuinely has nothing to report still answers, rather than holding
    /// the caller for as long as it takes to find out there is no answer.
    #[test]
    fn a_service_with_nothing_to_say_still_answers_within_its_patience() {
        static SILENT: Service<u8> = Service::new("test-silent", |_| {});
        assert_eq!(SILENT.awaited(Duration::from_millis(30)), None);
    }
}
