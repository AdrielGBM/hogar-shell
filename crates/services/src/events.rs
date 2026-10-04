//! What happened to the shell, as one typed stream: the `features.md` §9.7 vocabulary a rule can trigger on.
//!
//! Each event is emitted by the service that owns it — the battery performer, the lock performer, the session actions, the wallpaper store, the radios, the power-profile watcher, the startup path and the scheme export — so there is one place that decides each one rather than a poller per consumer guessing from readings. A listener is another producer thread (the rules engine, an `event` source), so the stream is a plain channel rather than a surface subscription.
//!
//! Emitting costs a lock and a channel send per listener and never waits on one, so it is safe from the driver thread. An emitter that must not go on before a listener has answered asks for [`emit_held`] and waits on what it returns, from a thread that is not the one the listener answers on.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use config::scheme::Mode;

/// One thing that happened, with what a rule would want to know about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// The shell finished starting: the config is applied, the surfaces are up and the startup subscriptions are taken.
    Started,
    /// A wallpaper was set or cleared at runtime.
    WallpaperChanged {
        /// The screen it was set for, or `None` for every screen.
        output: Option<String>,
        /// The image that screen now shows, or `None` for the theme's base colour.
        path: Option<PathBuf>,
    },
    /// The palette the shell paints with changed, whatever changed it (DEC-24): a built-in theme switched, colours edited, or a wallpaper-derived palette published — in which case the files `[theme.export]` writes for it are on disk by now.
    ColorsChanged,
    /// The palette the shell paints with switched between dark and light.
    ThemeModeChanged {
        mode: Mode,
    },
    /// The compositor confirmed a lock this shell took. A lock taken by another client is not reported: `ext-session-lock-v1` tells only the locking client.
    SessionLocked,
    /// A lock this shell held ended.
    SessionUnlocked,
    /// The session is about to be logged out; emitted before logind is asked.
    LoggingOut,
    /// The machine is about to reboot; emitted before logind is asked.
    Rebooting,
    /// The machine is about to power off; emitted before logind is asked.
    ShuttingDown,
    WifiEnabled,
    WifiDisabled,
    BluetoothEnabled,
    BluetoothDisabled,
    /// The battery started or stopped charging.
    BatteryStateChanged {
        level: i32,
        charging: bool,
    },
    /// The charge crossed down through a `[battery] warn_levels` threshold: once per crossing, the most severe one when a drop passes several.
    BatteryUnderThreshold {
        level: i32,
        threshold: i32,
    },
    /// power-profiles-daemon switched its active profile.
    PowerProfileChanged {
        profile: String,
    },
}

impl ShellEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            ShellEvent::Started => EventKind::Started,
            ShellEvent::WallpaperChanged { .. } => EventKind::WallpaperChanged,
            ShellEvent::ColorsChanged => EventKind::ColorsChanged,
            ShellEvent::ThemeModeChanged { .. } => EventKind::ThemeModeChanged,
            ShellEvent::SessionLocked => EventKind::SessionLocked,
            ShellEvent::SessionUnlocked => EventKind::SessionUnlocked,
            ShellEvent::LoggingOut => EventKind::LoggingOut,
            ShellEvent::Rebooting => EventKind::Rebooting,
            ShellEvent::ShuttingDown => EventKind::ShuttingDown,
            ShellEvent::WifiEnabled => EventKind::WifiEnabled,
            ShellEvent::WifiDisabled => EventKind::WifiDisabled,
            ShellEvent::BluetoothEnabled => EventKind::BluetoothEnabled,
            ShellEvent::BluetoothDisabled => EventKind::BluetoothDisabled,
            ShellEvent::BatteryStateChanged { .. } => EventKind::BatteryStateChanged,
            ShellEvent::BatteryUnderThreshold { .. } => EventKind::BatteryUnderThreshold,
            ShellEvent::PowerProfileChanged { .. } => EventKind::PowerProfileChanged,
        }
    }

    /// The stable name a rule triggers on.
    pub fn name(&self) -> &'static str {
        self.kind().as_str()
    }
}

/// Which event, without its payload: what a rule names in `config.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    Started,
    WallpaperChanged,
    ColorsChanged,
    ThemeModeChanged,
    SessionLocked,
    SessionUnlocked,
    LoggingOut,
    Rebooting,
    ShuttingDown,
    WifiEnabled,
    WifiDisabled,
    BluetoothEnabled,
    BluetoothDisabled,
    BatteryStateChanged,
    BatteryUnderThreshold,
    PowerProfileChanged,
}

/// The events a session action waits on rules for: each is emitted just before the action, and a rule answering one is finished first.
pub const SESSION_ENDING: [EventKind; 3] = [
    EventKind::LoggingOut,
    EventKind::Rebooting,
    EventKind::ShuttingDown,
];

impl EventKind {
    pub const ALL: [EventKind; 16] = [
        EventKind::Started,
        EventKind::WallpaperChanged,
        EventKind::ColorsChanged,
        EventKind::ThemeModeChanged,
        EventKind::SessionLocked,
        EventKind::SessionUnlocked,
        EventKind::LoggingOut,
        EventKind::Rebooting,
        EventKind::ShuttingDown,
        EventKind::WifiEnabled,
        EventKind::WifiDisabled,
        EventKind::BluetoothEnabled,
        EventKind::BluetoothDisabled,
        EventKind::BatteryStateChanged,
        EventKind::BatteryUnderThreshold,
        EventKind::PowerProfileChanged,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Started => "started",
            EventKind::WallpaperChanged => "wallpaper_changed",
            EventKind::ColorsChanged => "colors_changed",
            EventKind::ThemeModeChanged => "theme_mode_changed",
            EventKind::SessionLocked => "session_locked",
            EventKind::SessionUnlocked => "session_unlocked",
            EventKind::LoggingOut => "logging_out",
            EventKind::Rebooting => "rebooting",
            EventKind::ShuttingDown => "shutting_down",
            EventKind::WifiEnabled => "wifi_enabled",
            EventKind::WifiDisabled => "wifi_disabled",
            EventKind::BluetoothEnabled => "bluetooth_enabled",
            EventKind::BluetoothDisabled => "bluetooth_disabled",
            EventKind::BatteryStateChanged => "battery_state_changed",
            EventKind::BatteryUnderThreshold => "battery_under_threshold",
            EventKind::PowerProfileChanged => "power_profile_changed",
        }
    }

    /// Every event name, in vocabulary order: the one source for what a rule or an expression may name.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::ALL.into_iter().map(EventKind::as_str)
    }

    /// The event a rule named, by the one spelling [`EventKind::as_str`] gives it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

static LAST_OF_KIND: Mutex<Vec<ShellEvent>> = Mutex::new(Vec::new());

fn remember(event: &ShellEvent) {
    let mut seen = LAST_OF_KIND.lock().unwrap_or_else(PoisonError::into_inner);
    let kind = event.kind();
    match seen.iter_mut().find(|before| before.kind() == kind) {
        Some(before) => *before = event.clone(),
        None => seen.push(event.clone()),
    }
}

/// The most recent event of `kind` emitted in this process, whether or not anyone was listening when it fired.
pub fn last_of(kind: EventKind) -> Option<ShellEvent> {
    LAST_OF_KIND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|event| event.kind() == kind)
        .cloned()
}

/// Announces `event` to every listener, without waiting on any of them.
pub fn emit(event: ShellEvent) {
    drop(emit_held(event));
}

/// Announces `event`, and answers what to wait on until every listener that holds its kind (see [`listen_holding`]) is done with it.
///
/// For the events an action follows from: the shell is about to log out, and a rule that answers that has to be finished before logind is asked.
pub fn emit_held(event: ShellEvent) -> Held {
    tracing::debug!("event: {}", event.name());
    let pending = Arc::new(Pending::default());
    remember(&event);
    let kind = event.kind();
    holders().retain(|holder| {
        let hold = if holder.kinds.contains(&kind) {
            Hold::take(&pending)
        } else {
            Hold::default()
        };
        let delivery = Delivery {
            event: event.clone(),
            hold,
        };
        holder.tx.send(delivery).is_ok()
    });
    Held(pending)
}

/// What an emitter is waiting on: the listeners that were told to hold the event it emitted.
pub struct Held(Arc<Pending>);

impl Held {
    /// Waits until every hold has been let go, for at most `cap`. Answers whether they all were.
    pub fn wait(&self, cap: Duration) -> bool {
        let deadline = Instant::now() + cap;
        let mut holds = self.0.holds.lock().unwrap_or_else(PoisonError::into_inner);
        while *holds > 0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            holds = self
                .0
                .released
                .wait_timeout(holds, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

#[derive(Default)]
struct Pending {
    holds: Mutex<usize>,
    released: Condvar,
}

/// One listener's claim on an event: the emitter keeps waiting until it is dropped. Dropped with the listener, so a listener that goes away releases what it was holding.
#[derive(Default)]
pub struct Hold(Option<Arc<Pending>>);

impl Hold {
    fn take(pending: &Arc<Pending>) -> Self {
        *pending.holds.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Self(Some(Arc::clone(pending)))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let Some(pending) = self.0.take() else {
            return;
        };
        let mut holds = pending.holds.lock().unwrap_or_else(PoisonError::into_inner);
        *holds -= 1;
        if *holds == 0 {
            pending.released.notify_all();
        }
    }
}

/// An event as a holding listener hears it. `hold` is let go when the listener has finished with the event.
pub struct Delivery {
    pub event: ShellEvent,
    pub hold: Hold,
}

struct Holder {
    kinds: Vec<EventKind>,
    tx: mpsc::Sender<Delivery>,
}

static HOLDERS: Mutex<Vec<Holder>> = Mutex::new(Vec::new());

fn holders() -> MutexGuard<'static, Vec<Holder>> {
    HOLDERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts listening for every event emitted from now on, holding the ones of `kinds`: [`emit_held`] waits for each [`Delivery::hold`] of those kinds to be dropped.
pub fn listen_holding(kinds: &[EventKind]) -> Deliveries {
    let (tx, rx) = mpsc::channel();
    holders().push(Holder {
        kinds: kinds.to_vec(),
        tx,
    });
    Deliveries(rx)
}

/// What a holding listener hears, in emission order.
pub struct Deliveries(mpsc::Receiver<Delivery>);

impl Deliveries {
    /// The next delivery, waiting up to `patience` for one.
    pub fn next_within(&self, patience: Duration) -> Option<Delivery> {
        self.0.recv_timeout(patience).ok()
    }
}

/// Starts listening for every event emitted from now on, holding none of them.
pub fn listen() -> Events {
    Events(listen_holding(&[]))
}

/// The events emitted since [`listen`] was called, in emission order.
pub struct Events(Deliveries);

impl Events {
    /// The next event, waiting up to `patience` for one.
    pub fn next_within(&self, patience: Duration) -> Option<ShellEvent> {
        self.0.next_within(patience).map(|delivery| delivery.event)
    }
}

/// The last value a reading was seen to hold, so an owner can emit on a change rather than on every reading.
///
/// The first observation only records: a radio that is already on when the shell starts has not been switched on.
pub struct Edge<T>(Mutex<Option<T>>);

impl<T: Clone + PartialEq> Edge<T> {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Records `now` as the starting point when nothing has been seen yet, and does nothing after that.
    pub fn seed(&self, now: T) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(now);
    }

    /// Records `now`, answering it when it differs from a value seen before.
    pub fn observe(&self, now: T) -> Option<T> {
        let mut seen = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = seen.as_ref().is_some_and(|before| *before != now);
        *seen = Some(now.clone());
        changed.then_some(now)
    }
}

impl<T: Clone + PartialEq> Default for Edge<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A radio's on/off switch as events, announced from the first confirmed reading that shows it changed.
///
/// Feed it only what the daemon reported: a value the shell expects but has not seen confirmed would announce a switch that may never happen.
pub struct Radio {
    seen: Edge<bool>,
    on: ShellEvent,
    off: ShellEvent,
}

impl Radio {
    pub const fn new(on: ShellEvent, off: ShellEvent) -> Self {
        Self {
            seen: Edge::new(),
            on,
            off,
        }
    }

    /// The event a reading implies. A radio that is not `available` says nothing, so it neither announces nor resets what was last seen.
    pub fn observe(&self, available: bool, on: bool) -> Option<ShellEvent> {
        if !available {
            return None;
        }
        self.seen.observe(on).map(|now| {
            if now {
                self.on.clone()
            } else {
                self.off.clone()
            }
        })
    }

    pub fn announce(&self, available: bool, on: bool) {
        if let Some(event) = self.observe(available, on) {
            emit(event);
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn every_kind_round_trips_through_its_name() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::from_name("colours_changed"), None);
    }

    #[test]
    fn the_vocabulary_is_exactly_the_one_features_names() {
        let names: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            names,
            [
                "started",
                "wallpaper_changed",
                "colors_changed",
                "theme_mode_changed",
                "session_locked",
                "session_unlocked",
                "logging_out",
                "rebooting",
                "shutting_down",
                "wifi_enabled",
                "wifi_disabled",
                "bluetooth_enabled",
                "bluetooth_disabled",
                "battery_state_changed",
                "battery_under_threshold",
                "power_profile_changed",
            ]
        );
    }

    #[test]
    fn the_last_event_of_a_kind_is_known_without_anyone_listening() {
        assert_eq!(last_of(EventKind::BatteryUnderThreshold), None);
        emit(ShellEvent::BatteryUnderThreshold {
            level: 20,
            threshold: 20,
        });
        emit(ShellEvent::BatteryUnderThreshold {
            level: 9,
            threshold: 10,
        });
        emit(ShellEvent::BatteryStateChanged {
            level: 9,
            charging: true,
        });
        assert_eq!(
            last_of(EventKind::BatteryUnderThreshold),
            Some(ShellEvent::BatteryUnderThreshold {
                level: 9,
                threshold: 10
            })
        );
    }

    #[test]
    fn the_names_are_the_vocabulary() {
        assert_eq!(EventKind::names().count(), EventKind::ALL.len());
        assert!(EventKind::names().all(|name| EventKind::from_name(name).is_some()));
    }

    #[test]
    fn a_radio_is_announced_only_by_readings_that_show_it_switched() {
        let radio = Radio::new(ShellEvent::WifiEnabled, ShellEvent::WifiDisabled);
        let announced: Vec<Option<ShellEvent>> = [
            (true, true),
            (true, true),
            (true, false),
            (false, false),
            (true, false),
            (true, true),
        ]
        .into_iter()
        .map(|(available, on)| radio.observe(available, on))
        .collect();
        assert_eq!(
            announced,
            [
                None,
                None,
                Some(ShellEvent::WifiDisabled),
                None,
                None,
                Some(ShellEvent::WifiEnabled),
            ],
            "the first reading and the daemon going away are not switches"
        );
    }

    #[test]
    fn a_listener_hears_what_is_emitted_after_it_and_nothing_from_before() {
        let before = ShellEvent::PowerProfileChanged {
            profile: "events-test-before".to_string(),
        };
        emit(before.clone());
        let events = listen();
        let after = ShellEvent::PowerProfileChanged {
            profile: "events-test-after".to_string(),
        };
        emit(after.clone());

        let mut heard = Vec::new();
        while let Some(event) = events.next_within(Duration::from_millis(50)) {
            heard.push(event);
        }
        assert!(heard.contains(&after), "{heard:?}");
        assert!(
            !heard.contains(&before),
            "the event that was current when it subscribed is not replayed as news: {heard:?}"
        );
    }

    #[test]
    fn an_edge_answers_changes_only() {
        let edge = Edge::new();
        assert_eq!(
            edge.observe(true),
            None,
            "the first reading is a state, not a change"
        );
        assert_eq!(edge.observe(true), None);
        assert_eq!(edge.observe(false), Some(false));
        assert_eq!(edge.observe(false), None);
        assert_eq!(edge.observe(true), Some(true));
    }

    #[test]
    fn an_emitter_waits_for_a_holder_to_let_go_and_no_longer() {
        let events = listen_holding(&[EventKind::WifiEnabled]);
        let held = emit_held(ShellEvent::WifiEnabled);
        let released = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn({
            let released = released.clone();
            move || {
                let delivery = std::iter::from_fn(|| events.next_within(Duration::from_secs(2)))
                    .find(|delivery| delivery.event == ShellEvent::WifiEnabled)
                    .unwrap();
                std::thread::sleep(Duration::from_millis(50));
                released.store(true, Ordering::SeqCst);
                drop(delivery);
            }
        });
        assert!(held.wait(Duration::from_secs(5)));
        assert!(released.load(Ordering::SeqCst));
        worker.join().unwrap();
    }

    #[test]
    fn a_holder_that_never_lets_go_is_waited_on_only_up_to_the_cap() {
        let events = listen_holding(&[EventKind::WifiDisabled]);
        let held = emit_held(ShellEvent::WifiDisabled);
        let started = Instant::now();
        assert!(!held.wait(Duration::from_millis(100)));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(events);
    }

    #[test]
    fn a_holder_that_goes_away_releases_what_it_held() {
        let events = listen_holding(&[EventKind::BluetoothEnabled]);
        let held = emit_held(ShellEvent::BluetoothEnabled);
        drop(events);
        assert!(held.wait(Duration::from_secs(2)));
    }

    #[test]
    fn nobody_holding_means_no_wait() {
        let other = listen_holding(&[EventKind::BluetoothDisabled]);
        let held = emit_held(ShellEvent::Started);
        assert!(held.wait(Duration::ZERO));
        drop(other);
    }

    #[test]
    fn a_holding_listener_hears_the_kinds_it_does_not_hold_without_holding_them() {
        let events = listen_holding(&[EventKind::BluetoothEnabled]);
        let held = emit_held(ShellEvent::PowerProfileChanged {
            profile: "events-test-unheld".to_string(),
        });
        assert!(held.wait(Duration::ZERO));
        let mut heard = Vec::new();
        while let Some(delivery) = events.next_within(Duration::from_millis(50)) {
            heard.push(delivery.event);
        }
        assert!(heard.iter().any(|event| matches!(event, ShellEvent::PowerProfileChanged { profile } if profile == "events-test-unheld")));
    }

    #[test]
    fn a_seed_is_a_starting_point_and_not_a_change() {
        let edge = Edge::new();
        edge.seed(1);
        edge.seed(2);
        assert_eq!(edge.observe(1), None);
        assert_eq!(edge.observe(2), Some(2));
    }
}
