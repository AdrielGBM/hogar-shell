//! The shared-source primitive every system service is built on.
//!
//! A service owns exactly one producer — a D-Bus subscription, a socket, a watcher process — running on its own thread for the whole shell. Surfaces don't read the system; they subscribe, and the producer fans each reading out to all of them. N bars therefore cost one connection and one parse per change, not N, and a surface never runs a timer of its own.
//!
//! A producer whose identity is only known at run time — a user's `poll` line, a URL — cannot be a `static`, so [`Keyed`] is the same fan-out filed by key: one producer per distinct key however many subscribers ask for it.
//!
//! A module consumes one by handing [`Service::subscribe`] to `platform_wayland::watch`, which delivers each value on that surface's own loop thread and unsubscribes it when the surface goes away.
//!
//! **A producer that nobody is listening to has to stop.** Starting lazily is only half the rule: a service whose last subscriber went away — the bar module switched off in a reload, the panel that closed — kept its thread, its connection and its poll timer for the life of the process. A polling producer with no subscriber is the clearest form of that. [`Broadcast::wanted`] is how a producer asks, and returning from the producer when it answers `false` is how a service opts in; a later subscription starts a fresh one.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use platform_wayland::EventSender;

/// Every lock here guards plain data that no panic can leave half-written, so a panic on another thread is no reason for this one to panic too — and a registry lock poisoned once would otherwise take down every surface that subscribes afterwards.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Producer threads alive right now, by census label, counted rather than flagged so a restart arriving before the outgoing thread has returned does not un-list the incoming one.
static RUNNING: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

/// The producer threads running right now. The census behind `shell status`: a service still listed when nothing on the bar asks for it is exactly the leak that command exists to make visible from a script.
pub fn running_services() -> Vec<String> {
    lock(&RUNNING).keys().cloned().collect()
}

fn producer_started(name: &str) {
    *lock(&RUNNING).entry(name.to_string()).or_insert(0) += 1;
}

fn producer_finished(name: &str) {
    let mut running = lock(&RUNNING);
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
    /// A surface reads through `EventSender`, which only the driver can hand out — so a producer that has to react to a service instead of to the system had no way to wait for one, and could only poll. The wallpaper surface is the case that needs it: the moment the choice changes, something has to decode a full-resolution image, and that something must be neither the UI thread nor a timer.
    listeners: Mutex<Vec<mpsc::Sender<T>>>,
    /// Whether a producer thread is live, so a second subscription does not start a second one. Cleared only by [`wanted`](Self::wanted), which is what lets a later subscriber start a fresh producer.
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

    /// Whether anything is still listening — and, when nothing is, the point at which the producer is released so that a later subscription starts a new one. A producer that polls asks this once a turn and returns when it answers `false`.
    ///
    /// **This is opt-in, and deliberately so.** A producer that never asks behaves exactly as every producer did before it existed: started once, never stopped. That is the right answer for one that registers a callback and returns *and has no way to take the registration back* — its work outlives the call, so releasing the flag would let a second subscriber start a second D-Bus connection — and it makes converting the rest a service at a time.
    ///
    /// A producer that *can* take it back may ask like any other, and `services::hyprland` does: it hands each registration a `platform_wayland::Interest` and retires it in the same breath as answering `false`, so the callback is dropped rather than left to be called by a watcher nobody wants. The rule that makes that safe is one token per producer run — a producer registered in two places whose registrations retire one at a time is exactly the second-producer bug below, reached the long way round.
    ///
    /// **A `false` is final: return, and do not ask again.** Answering `false` is the producer giving up its claim, and the next subscriber is free to start a replacement the instant it does. A producer that asked a second time could be told `true` by that subscriber's arrival and carry on — leaving two producers on one service, which is the duplicate-connection bug this whole mechanism exists to avoid.
    ///
    /// **Ask it after publishing, never before.** [`Service::current`] and [`Service::awaited`] start a producer without subscribing — an IPC `volume get` on a bar with no volume chip — so a producer that asked first would retire before taking the one reading its caller started it for, and answer `None` about a machine that had an answer.
    ///
    /// The `running` flag is locked for the whole check because [`Service::subscribe`] takes it *after* pushing its subscriber. Without that overlap, a subscription landing between "nobody is listening" and "the producer has stopped" would find a service still marked running and never start one.
    pub fn wanted(&self) -> bool {
        let mut running = lock(&self.running);
        let listening = {
            let mut subscribers = lock(&self.subscribers);
            subscribers.retain(EventSender::alive);
            !subscribers.is_empty()
        } || !lock(&self.listeners).is_empty();
        if listening {
            return true;
        }
        *running = false;
        false
    }

    /// Records `value` as the current reading and fans it out, dropping subscribers whose surface has closed (their channel receiver is gone, so `send` fails).
    pub fn publish(&self, value: T) {
        *lock(&self.current) = Some(value.clone());
        lock(&self.subscribers).retain(|tx| tx.send(value.clone()));
        lock(&self.listeners).retain(|tx| tx.send(value.clone()).is_ok());
    }

    pub fn current(&self) -> Option<T> {
        lock(&self.current).clone()
    }

    /// Spawns `producer` on a thread called `thread` unless one is already live, listing it in the census as `census`. Idempotent, and safe to call on every subscription.
    ///
    /// The two names are apart because only one of them may carry text a user wrote: a thread name with a NUL in it is a panic in `std`, so a thread is only ever named by the shell.
    fn start(
        self: &Arc<Self>,
        thread: String,
        census: String,
        producer: impl FnOnce(&Arc<Self>) + Send + 'static,
    ) where
        T: Send + 'static,
    {
        let mut running = lock(&self.running);
        if *running {
            return;
        }
        let owned = Arc::clone(self);
        producer_started(&census);
        let listed = census.clone();
        let spawned = std::thread::Builder::new().name(thread).spawn(move || {
            producer(&owned);
            producer_finished(&listed);
        });
        match spawned {
            Ok(_) => *running = true,
            Err(e) => {
                producer_finished(&census);
                tracing::warn!("could not start the {census} service: {e}");
            }
        }
    }

    /// Sends the current reading to `tx` and registers it, so a late subscriber starts in sync. `false` when `tx` is already gone and was not registered.
    fn attach(&self, tx: EventSender<T>) -> bool {
        if let Some(value) = self.current()
            && !tx.send(value)
        {
            return false;
        }
        lock(&self.subscribers).push(tx);
        true
    }
}

/// A lazily-started shared service. The producer thread spins up on the first subscription, so a shell configured without a battery chip never opens a UPower connection — and, for a producer that asks [`Broadcast::wanted`], winds down again when the last subscriber goes away.
///
/// The producer receives the broadcast as an `Arc`, not a borrow, so it can either park on a loop of its own or hand a clone to a callback and return — which is what a service reading off an event stream someone else owns has to do. Either way the broadcast outlives the producer, and the last reading survives a producer that stopped, so a chip that comes back draws it immediately rather than blank.
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
        broadcast.start(
            self.thread_name.to_string(),
            self.thread_name.to_string(),
            self.producer,
        );
    }

    fn started(&'static self) -> &'static Arc<Broadcast<T>> {
        let broadcast = self.broadcast();
        self.start(broadcast);
        broadcast
    }

    /// Registers `tx` for live readings, sending the current one immediately so the surface starts in sync rather than blank until the next change. Pass this as the producer to `platform_wayland::watch`.
    ///
    /// The subscriber is pushed *before* the producer is started, which is what closes the window against a producer winding itself down at the same moment — see [`Broadcast::wanted`].
    pub fn subscribe(&'static self, tx: EventSender<T>) {
        let broadcast = self.broadcast();
        if broadcast.attach(tx) {
            self.start(broadcast);
        }
    }

    /// Publishes a reading taken outside the producer thread — used when the shell itself causes the change (a mute toggle, a brightness step) so the UI reflects it immediately instead of at the producer's next turn.
    pub fn publish(&'static self, value: T) {
        self.started().publish(value);
    }

    /// The last published reading, without touching the system. Lets a UI handler act on the current value (a scroll stepping from it) without doing blocking I/O — or spawning a process — on the render thread.
    pub fn current(&'static self) -> Option<T> {
        self.started().current()
    }

    /// The last published reading, waiting up to `patience` for the first one when the producer has only just started.
    ///
    /// **[`current`](Self::current) starts the producer and reads in the same breath**, which is right for a UI handler — it either has a reading to step from or has nothing to draw — and wrong for a caller that is itself the reason the service started. An IPC `volume get` on a shell whose bar carries no volume chip asked a listener that had not had a turn yet, got `None`, and answered "no audio sink available" about a machine with one; `volume up` did nothing at all and said it had.
    ///
    /// Polled rather than signalled because the wait only ever happens once, on the first read of a service nothing had subscribed to, and a condvar per broadcast is a lot of machinery for that.
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

    /// Records `value` as the current reading *without* starting the producer, so a reader sees it and the thread that would have read the system never runs.
    ///
    /// What a `[preview]` fixture seeds a service with. [`publish`](Self::publish) is the wrong door for that: it starts the producer, which for the tray means claiming a D-Bus name and then overwriting the seeded reading with the machine's own — so a preview would draw whatever happens to be running.
    ///
    /// Claiming the producer slot is what enforces the "never runs" half. Every other reader — `current`, `awaited`, `publish` — starts the producer if none is live, so a seeded service that left the slot open would grow the very thread the seed exists to avoid the moment a preview read it.
    pub fn seed(&'static self, value: T) {
        let broadcast = self.broadcast();
        *lock(&broadcast.running) = true;
        broadcast.publish(value);
    }
}

/// Shared sources filed by a key chosen at run time, one producer per distinct key.
///
/// [`Service`] is a `static` with a fn-pointer producer, which is right for a battery and wrong for "whatever command the user wrote": that identity only exists once the layout is loaded. A `Keyed` is owned by whoever builds the registry, and each subscription brings the producer that *would* serve its key — used only when no producer for that key is live, so two subscribers on one key share one thread and the second one's closure is dropped unrun.
///
/// The semantics are [`Service`]'s, because the machinery is the same [`Broadcast`]: late subscribers are handed the current reading, a producer that asks [`Broadcast::wanted`] and hears `false` is gone for good, and the next subscriber starts a fresh one. Readings outlive their producer, so a key that comes back draws its last value at once. A key's entry stays until its owner withdraws it with [`Keyed::retain`], which is how a source a layout edit removed stops holding a reading nobody will ask for again.
///
/// Each key also keeps an `M`, handed to every producer that serves it: what one producer has to know about the last — when it last ran, how many runs in a row failed — lives as long as the key does, so a producer that retires and is started again carries on where the last one stopped rather than from nothing.
pub struct Keyed<K, T, M = ()> {
    name: &'static str,
    entries: Mutex<BTreeMap<K, Entry<T, M>>>,
    next_id: AtomicU64,
}

struct Entry<T, M> {
    broadcast: Arc<Broadcast<T>>,
    memory: Arc<M>,
    /// What the key's producer threads are named by: a key is whatever its owner chose, a user's command line among them, and no thread is named with that.
    id: u64,
}

/// A key as the census shows it: whole, so two keys are never one line, with control characters spelled out, so `shell status` stays one line per producer.
fn census_label(key: &str) -> String {
    let mut label = String::with_capacity(key.len());
    for c in key.chars() {
        match c.is_control() {
            true => label.extend(c.escape_default()),
            false => label.push(c),
        }
    }
    label
}

impl<K, T, M> Keyed<K, T, M>
where
    K: Ord + Clone + std::fmt::Display,
    T: Clone + Send + 'static,
    M: Default + Send + Sync + 'static,
{
    /// `name` prefixes the producer threads, which `shell status` lists as `name:key`.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            entries: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(0),
        }
    }

    /// Registers `tx` for `key`'s readings, sending the current one immediately, and starts `producer` for the key unless one is already live. The producer is handed the key's memory.
    ///
    /// The subscriber is pushed *before* the producer is started, for the reason given on [`Service::subscribe`].
    pub fn subscribe(
        &self,
        key: K,
        tx: EventSender<T>,
        producer: impl FnOnce(&Arc<Broadcast<T>>, &M) + Send + 'static,
    ) {
        let (broadcast, memory, id) = {
            let mut entries = lock(&self.entries);
            let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
                broadcast: Arc::new(Broadcast::new()),
                memory: Arc::default(),
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
            });
            (
                Arc::clone(&entry.broadcast),
                Arc::clone(&entry.memory),
                entry.id,
            )
        };
        if broadcast.attach(tx) {
            broadcast.start(
                format!("{}#{id}", self.name),
                format!("{}:{}", self.name, census_label(&key.to_string())),
                move |out| producer(out, &memory),
            );
        }
    }

    /// The last reading for `key`, without starting anything.
    pub fn current(&self, key: &K) -> Option<T> {
        lock(&self.entries).get(key)?.broadcast.current()
    }

    /// Forgets every key `keep` refuses, with its last reading and its memory, once no producer is serving it.
    ///
    /// A key whose producer is still running stays until it retires: dropping it then would let the next subscriber on the same key start a second producer beside the first. So a caller that withdraws keys asks again whenever one of its producers stops.
    pub fn retain(&self, mut keep: impl FnMut(&K) -> bool) {
        lock(&self.entries).retain(|key, entry| keep(key) || *lock(&entry.broadcast.running));
    }

    /// Every key this registry holds an entry for, running or not.
    pub fn keys(&self) -> Vec<K> {
        lock(&self.entries).keys().cloned().collect()
    }
}

/// A producerless shared value: the same one-writer/N-reader fan-out as [`Service`], for state the shell itself owns — persisted toggles, the current wallpaper, launch counts — rather than reads off the system. Nothing polls and no thread is spawned; the value is seeded by `init` on first touch and changed by [`Store::update`].
pub struct Store<T: 'static> {
    cell: OnceLock<Arc<Broadcast<T>>>,
    init: fn() -> T,
    /// Held from reading the value to publishing the change, so two writers on different threads cannot both start from the same value and lose one change.
    writing: Mutex<()>,
}

impl<T: Clone + Send + 'static> Store<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            cell: OnceLock::new(),
            init,
            writing: Mutex::new(()),
        }
    }

    fn started(&'static self) -> &'static Arc<Broadcast<T>> {
        self.cell.get_or_init(|| {
            let broadcast = Arc::new(Broadcast::new());
            *lock(&broadcast.current) = Some((self.init)());
            broadcast
        })
    }

    /// The current value, seeding it on first call.
    pub fn get(&'static self) -> T {
        self.started()
            .current()
            .expect("a store is seeded when it starts")
    }

    /// Applies `change` to the current value and fans the result out, one writer at a time. Returns the new value.
    pub fn update(&'static self, change: impl FnOnce(&mut T)) -> T {
        self.update_then(change, |_| {})
    }

    /// [`update`](Self::update), running `then` with the new value before the next writer may start — what keeps the writes a caller makes of each value in the order the changes were made.
    pub fn update_then(&'static self, change: impl FnOnce(&mut T), then: impl FnOnce(&T)) -> T {
        let changed = self.write(
            |next| {
                change(next);
                Ok::<(), Infallible>(())
            },
            then,
        );
        match changed {
            Ok((next, ())) => next,
            Err(never) => match never {},
        }
    }

    /// [`update_then`](Self::update_then) for a change that may be refused. An `Err` from `change` leaves the value as it was — whatever `change` did to its copy is dropped — publishes nothing and skips `then`, so a refused change costs no reader a wake-up and no caller a write.
    pub fn try_update_then<R, E>(
        &'static self,
        change: impl FnOnce(&mut T) -> Result<R, E>,
        then: impl FnOnce(&T),
    ) -> Result<R, E> {
        self.write(change, then).map(|(_, made)| made)
    }

    fn write<R, E>(
        &'static self,
        change: impl FnOnce(&mut T) -> Result<R, E>,
        then: impl FnOnce(&T),
    ) -> Result<(T, R), E> {
        let _writing = lock(&self.writing);
        let broadcast = self.started();
        let mut next = broadcast
            .current()
            .expect("a store is seeded when it starts");
        let made = change(&mut next)?;
        broadcast.publish(next.clone());
        then(&next);
        Ok((next, made))
    }

    /// Registers `tx` for changes, sending the current value immediately so a surface starts in sync.
    pub fn subscribe(&'static self, tx: EventSender<T>) {
        self.started().attach(tx);
    }

    /// Registers a plain channel for changes, sending the current value immediately. For a producer thread that has to *wait* on this store rather than poll it; a surface wants [`subscribe`](Self::subscribe).
    pub fn listen(&'static self, tx: mpsc::Sender<T>) {
        let broadcast = self.started();
        if let Some(value) = broadcast.current()
            && tx.send(value).is_err()
        {
            return;
        }
        lock(&broadcast.listeners).push(tx);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    static TURNS: AtomicUsize = AtomicUsize::new(0);

    fn is_running(name: &str) -> bool {
        running_services().iter().any(|running| running == name)
    }

    /// A poller in the shape every polling service has: take a reading, publish it, ask whether that was worth doing, and retire when it was not.
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
    /// Lazy start was only ever half of it. A service whose last subscriber went away — a module switched off in a config reload, a panel closed — kept its thread and its poll timer for the life of the process, reading the system for nobody. Subscribing again has to get a live service back, not a corpse.
    #[test]
    fn a_service_stops_when_its_last_subscriber_goes_and_starts_again_for_the_next() {
        let (tx, subscription) = platform_wayland::detached();
        POLLER.subscribe(tx);
        eventually("the producer to take a turn", || {
            TURNS.load(Ordering::SeqCst) > 0
        });
        assert!(
            is_running("test-poller"),
            "a running producer is visible to `shell status`"
        );

        drop(subscription);
        eventually("the producer to retire", || !is_running("test-poller"));
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

    /// The dedupe the source registry stands on: a thousand subscribers to one command are one process, not a thousand.
    #[test]
    fn two_subscribers_on_one_key_share_one_producer() {
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        let registry = Keyed::<String, u8>::new("test-keyed-shared");
        let producer = || {
            |out: &Arc<Broadcast<u8>>, _: &()| {
                STARTS.fetch_add(1, Ordering::SeqCst);
                out.publish(5);
                while out.wanted() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };

        let (first, first_sub) = platform_wayland::detached();
        registry.subscribe("a".into(), first, producer());
        eventually("the first reading", || {
            registry.current(&"a".to_string()) == Some(5)
        });

        let (second, second_sub) = platform_wayland::detached();
        registry.subscribe("a".into(), second, producer());
        assert_eq!(
            second_sub.try_recv(),
            Some(5),
            "a late subscriber is handed the current reading"
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(STARTS.load(Ordering::SeqCst), 1);
        drop(first_sub);
    }

    #[test]
    fn distinct_keys_get_distinct_producers() {
        let registry = Keyed::<&str, u8>::new("test-keyed-distinct");
        let producer = |value: u8| {
            move |out: &Arc<Broadcast<u8>>, _: &()| {
                out.publish(value);
                while out.wanted() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };

        let (one, one_sub) = platform_wayland::detached();
        let (two, two_sub) = platform_wayland::detached();
        registry.subscribe("one", one, producer(1));
        registry.subscribe("two", two, producer(2));

        eventually("both readings", || {
            registry.current(&"one") == Some(1) && registry.current(&"two") == Some(2)
        });
        assert!(is_running("test-keyed-distinct:one"));
        assert!(is_running("test-keyed-distinct:two"));
        drop((one_sub, two_sub));
    }

    #[test]
    fn dropping_every_subscriber_stops_the_producer_and_a_later_one_starts_a_new_one() {
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        let registry = Keyed::<&str, usize>::new("test-keyed-stop");
        let producer = || {
            |out: &Arc<Broadcast<usize>>, _: &()| {
                loop {
                    out.publish(STARTS.fetch_add(1, Ordering::SeqCst));
                    if !out.wanted() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };

        let (first, first_sub) = platform_wayland::detached();
        let (second, second_sub) = platform_wayland::detached();
        registry.subscribe("k", first, producer());
        registry.subscribe("k", second, producer());
        eventually("the producer to run", || is_running("test-keyed-stop:k"));

        drop(first_sub);
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            is_running("test-keyed-stop:k"),
            "one subscriber is still listening"
        );

        drop(second_sub);
        eventually("the producer to retire", || {
            !is_running("test-keyed-stop:k")
        });
        let idle = STARTS.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            STARTS.load(Ordering::SeqCst),
            idle,
            "nothing reads for nobody"
        );

        let (late, late_sub) = platform_wayland::detached();
        registry.subscribe("k", late, producer());
        eventually("a fresh producer", || STARTS.load(Ordering::SeqCst) > idle);
        assert!(late_sub.try_recv().is_some());
    }

    #[test]
    fn a_withdrawn_key_is_forgotten_once_its_producer_has_retired() {
        let registry = Keyed::<&str, u8>::new("test-keyed-retain");
        let producer = |out: &Arc<Broadcast<u8>>, _: &()| {
            out.publish(1);
            while out.wanted() {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let (tx, subscription) = platform_wayland::detached();
        registry.subscribe("gone", tx, producer);
        eventually("the producer to run", || {
            is_running("test-keyed-retain:gone")
        });

        registry.retain(|_| false);
        assert_eq!(
            registry.keys(),
            ["gone"],
            "a key still being served is kept, or its next subscriber would start a second producer"
        );

        drop(subscription);
        eventually("the producer to retire", || {
            !is_running("test-keyed-retain:gone")
        });
        registry.retain(|_| false);
        assert!(registry.keys().is_empty());
        assert_eq!(registry.current(&"gone"), None);
    }

    /// A key is whatever its owner chose — a user's command line among them — and `std` panics naming a thread with a NUL in it. That panic, taken while the producer slot was locked, poisoned the slot and took every later subscriber down with it.
    #[test]
    fn a_key_with_control_characters_in_it_still_gets_a_producer_and_a_readable_census_line() {
        let registry = Keyed::<String, String>::new("test-keyed-nul");
        let producer = |out: &Arc<Broadcast<String>>, _: &()| {
            out.publish(
                std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string(),
            );
            while out.wanted() {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let key = "echo \u{0}x\n".to_string();
        let (tx, subscription) = platform_wayland::detached();
        registry.subscribe(key.clone(), tx, producer);
        eventually("the producer to run", || registry.current(&key).is_some());

        let thread_name = registry.current(&key).unwrap();
        assert!(
            thread_name.starts_with("test-keyed-nul#") && !thread_name.contains("echo"),
            "the thread is named by the registry, not by the key: {thread_name:?}"
        );
        let listed: Vec<String> = running_services()
            .into_iter()
            .filter(|name| name.starts_with("test-keyed-nul:"))
            .collect();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            listed[0], "test-keyed-nul:echo \\u{0}x\\n",
            "control characters are spelled out"
        );
        drop(subscription);
    }

    /// What a key's producer remembers is the key's, so a producer that retires and starts again for a returning subscriber picks up where the last one left off.
    #[test]
    fn a_keys_memory_outlives_its_producer() {
        let registry = Keyed::<&str, usize, AtomicUsize>::new("test-keyed-memory");
        let producer = |out: &Arc<Broadcast<usize>>, runs: &AtomicUsize| {
            out.publish(runs.fetch_add(1, Ordering::SeqCst) + 1);
            while out.wanted() {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        for expected in 1..=3 {
            let (tx, subscription) = platform_wayland::detached();
            registry.subscribe("k", tx, producer);
            eventually("a run", || registry.current(&"k") == Some(expected));
            drop(subscription);
            eventually("the producer to retire", || {
                !is_running("test-keyed-memory:k")
            });
        }

        registry.retain(|_| false);
        let (tx, _subscription) = platform_wayland::detached();
        registry.subscribe("k", tx, producer);
        eventually("a run", || registry.current(&"k") == Some(1));
    }

    /// A producer that never asks [`Broadcast::wanted`] keeps the old contract — started once, never stopped — so a service whose work outlives the producer call cannot be handed a second connection by a second subscriber.
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

    #[test]
    fn a_refused_change_publishes_nothing_and_leaves_the_value_as_it_was() {
        static GUARDED: Store<u32> = Store::new(|| 1);
        let (tx, rx) = mpsc::channel();
        GUARDED.listen(tx);
        assert_eq!(rx.try_recv(), Ok(1), "a listener starts with the value");
        let mut then_ran = false;

        let refused = GUARDED.try_update_then(
            |n| {
                *n = 99;
                Err::<(), _>("no")
            },
            |_| then_ran = true,
        );

        assert_eq!(refused, Err("no"));
        assert_eq!(
            GUARDED.get(),
            1,
            "the change made before refusing is dropped"
        );
        assert!(rx.try_recv().is_err(), "nothing was published");
        assert!(!then_ran, "nor written by `then`");

        let made = GUARDED.try_update_then(
            |n| {
                *n += 1;
                Ok::<_, ()>("made")
            },
            |n| then_ran = *n == 2,
        );

        assert_eq!(made, Ok("made"));
        assert_eq!(rx.try_recv(), Ok(2));
        assert!(then_ran);
    }

    /// Two threads changing one store at once each start from what the other left: a change that read the value before the other's landed would put it back.
    #[test]
    fn writers_on_different_threads_never_lose_each_others_changes() {
        static TALLY: Store<u32> = Store::new(|| 0);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..200 {
                        TALLY.update(|n| {
                            let read = *n;
                            std::thread::yield_now();
                            *n = read + 1;
                        });
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(TALLY.get(), 1600);
    }

    /// **A caller that starts a service is the one caller that cannot use [`Service::current`].**
    ///
    /// `current` starts the producer and reads in the same breath, so the very first read of a service nothing had subscribed to is always `None` — which reached the user as `volume get` answering "no audio sink available" on a machine with one, and `volume up` doing nothing and reporting the step it had not taken.
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

    /// The wait is bounded: a service that genuinely has nothing to report still answers, rather than holding the caller for as long as it takes to find out there is no answer.
    #[test]
    fn a_service_with_nothing_to_say_still_answers_within_its_patience() {
        static SILENT: Service<u8> = Service::new("test-silent", |_| {});
        assert_eq!(SILENT.awaited(Duration::from_millis(30)), None);
    }
}
