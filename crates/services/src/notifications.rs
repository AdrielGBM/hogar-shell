use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use platform_wayland::EventSender;
use serde::{Deserialize, Serialize};
use zbus::zvariant::Value;

const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
/// Most recent notifications kept in the persisted history, so the file stays bounded.
const MAX_HISTORY: usize = 50;

/// `[notifications]` gates on it, so the type is the config's; this is the name the daemon reads it by.
pub use config::policy::Urgency;

/// The `urgency` hint as the spec sends it: a byte, with anything unrecognised treated as normal.
fn urgency_from_hint(byte: u8) -> Urgency {
    match byte {
        0 => Urgency::Low,
        2 => Urgency::Critical,
        _ => Urgency::Normal,
    }
}

/// A notification's own image (from the `image-data`/`icon_data` hint), decoded to RGBA8. Runtime-only — not persisted (raw pixels would bloat the history file), so a restored notification falls back to its dot.
#[derive(Clone, Debug)]
pub struct NotificationImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One live notification, as delivered over `org.freedesktop.Notifications`. `actions` is the raw `[key, label, key, label, …]` list from the spec.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    #[serde(default)]
    pub actions: Vec<String>,
    pub urgency: Urgency,
    /// Whether this notification may still raise a popup: `true` for a fresh arrival, `false` for one restored from persisted history at startup (those belong in the history panel, not re-popped). Runtime-only — skipped from serialization, so every restored notification deserializes back to `false`.
    #[serde(skip)]
    pub popup: bool,
    /// The notification's image, when it carried one. Runtime-only (not persisted).
    #[serde(skip)]
    pub image: Option<NotificationImage>,
}

/// An immutable view of the daemon's state, broadcast to every subscribed surface. Shared behind `Arc` so a fan-out to N surfaces clones a pointer, not the list.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Active notifications, oldest first.
    pub active: Vec<Notification>,
    /// Notifications received since the history was last marked read.
    pub unread: u32,
    /// Do-Not-Disturb: popups are suppressed, history still records.
    pub dnd: bool,
    /// Applications whose notifications never pop. Carried in the snapshot so a panel can draw a group's mute state from the same reading it draws its cards from, rather than reading the state file per row.
    pub muted_apps: Vec<String>,
}

impl Snapshot {
    pub fn is_muted(&self, app_name: &str) -> bool {
        self.muted_apps.iter().any(|a| a == app_name)
    }
}

pub type SharedSnapshot = Arc<Snapshot>;

struct State {
    active: Vec<Notification>,
    next_id: u32,
    unread: u32,
    dnd: bool,
    muted_apps: Vec<String>,
}

impl State {
    fn snapshot(&self) -> SharedSnapshot {
        Arc::new(Snapshot {
            active: self.active.clone(),
            unread: self.unread,
            dnd: self.dnd,
            muted_apps: self.muted_apps.clone(),
        })
    }
}

/// The daemon's `[notifications]`-derived behaviour: the auto-dismiss defaults and the sound a pop makes.
///
/// Held behind a lock rather than copied in at startup, so [`set_policy`] can hand the daemon a new one on a config reload — the D-Bus name and the notification history stay put, only the policy moves.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub timeout: Duration,
    pub critical_sticky: bool,
    /// The ceiling on a sticky `critical` popup, after which it retires to the history like any other. `None` is the unbounded wait, which has no way out but a gesture.
    pub critical_max: Option<Duration>,
    /// A shell command run detached each time a notification pops; empty is silent.
    pub sound: String,
}

/// The in-process notification daemon: owns the D-Bus name, holds the live state, and fans each change out to every surface that subscribed. This is the shared source the architecture note calls for — one owner, many independent per-surface subscriptions.
struct Inner {
    state: Mutex<State>,
    subscribers: Mutex<Vec<EventSender<SharedSnapshot>>>,
    policy: Mutex<Policy>,
    /// What each notification's popup expiry *will* be, held until it is actually on screen.
    ///
    /// The clock cannot start when a notification arrives, because the column shows a bounded number of cards and the rest wait: a fifth notification whose timer began on arrival spends its whole life queued and is gone almost the moment it appears. It starts on [`shown`] instead, which the column calls for the cards it is drawing, and an entry is spent the first time that happens.
    pending: Mutex<HashMap<u32, (i32, Urgency)>>,
    /// The current history is shipped here after every change; a background thread debounces and writes it.
    saver: Sender<Vec<Notification>>,
}

impl Inner {
    /// Applies `mutate`, persists the new history (debounced, off-thread), then pushes a fresh snapshot to every live subscriber, dropping any whose surface has gone.
    fn commit(&self, mutate: impl FnOnce(&mut State)) {
        let snapshot = {
            let mut state = self.state.lock().unwrap();
            mutate(&mut state);
            state.snapshot()
        };
        let _ = self.saver.send(snapshot.active.clone());
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(SharedSnapshot::clone(&snapshot)));
    }

    fn policy(&self) -> Policy {
        self.policy.lock().unwrap().clone()
    }

    fn push(&self, mut notification: Notification, replaces_id: u32) -> u32 {
        let (mut assigned, mut popped) = (0, false);
        self.commit(|state| {
            let id = if replaces_id != 0 {
                replaces_id
            } else {
                state.next_id = state.next_id.wrapping_add(1).max(1);
                state.next_id
            };
            notification.id = id;
            assigned = id;
            // Decided at the daemon's single entry point, so a mute holds for the shell's own advisories as much as for anything arriving over D-Bus.
            //
            // Do-Not-Disturb is decided here too, and not where the card is drawn: a notification that arrives under it must be *recorded as not popping*, so that switching DND off later leaves it in the history rather than putting it on screen. Suppressed is not deferred.
            notification.popup &= !state.muted_apps.contains(&notification.app_name) && !state.dnd;
            popped = notification.popup;
            if let Some(existing) = state.active.iter_mut().find(|n| n.id == id) {
                *existing = notification;
            } else {
                state.active.push(notification);
                state.unread = state.unread.saturating_add(1);
            }
        });
        if popped && let Some(command) = sound_command(&self.policy().sound) {
            crate::apps::run_detached(command);
        }
        assigned
    }

    /// Sets Do-Not-Disturb, retiring every popup it is switching on over. See [`set_dnd`] for why suppressing is not deferring.
    fn set_dnd(&self, dnd: bool) {
        self.commit(|state| {
            state.dnd = dnd;
            if dnd {
                for notification in &mut state.active {
                    notification.popup = false;
                }
            }
        });
    }

    /// Removes `id` from the history entirely — a manual dismiss (a history-card tap or clear-all).
    fn close(&self, id: u32) {
        self.disarm_expiry(id);
        self.commit(|state| state.active.retain(|n| n.id != id));
    }

    /// Drops every notification `app_name` sent, answering with the ids that went so the caller can close them on the bus.
    fn clear_app(&self, app_name: &str) -> Vec<u32> {
        let mut closed = Vec::new();
        self.commit(|state| {
            state.active.retain(|n| {
                let keep = n.app_name != app_name;
                if !keep {
                    closed.push(n.id);
                }
                keep
            });
        });
        closed
    }

    /// Retires `id`'s popup while keeping it in the history: the popup stack stops showing it (it filters on `popup`), but the panel — which lists all of `active` — keeps it until dismissed. This is what a popup timeout does, so an auto-dismissed notification is still there to read later.
    fn expire(&self, id: u32) {
        self.disarm_expiry(id);
        self.commit(|state| {
            if let Some(n) = state.active.iter_mut().find(|n| n.id == id) {
                n.popup = false;
            }
        });
    }

    /// Records what `id`'s popup expiry will be once it is on screen. See [`Inner::pending`].
    fn arm_expiry(&self, id: u32, expire_timeout: i32, urgency: Urgency) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, (expire_timeout, urgency));
        }
    }

    /// Starts `id`'s expiry, if it has one and has not started already — what the column calls for a card it has put on screen.
    fn start_expiry(&self, id: u32) {
        let armed = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id));
        if let Some((expire_timeout, urgency)) = armed {
            self.schedule_expiry(id, expire_timeout, urgency);
        }
    }

    /// Forgets `id`'s armed expiry — it was dealt with before it was ever shown, so there is no clock to start.
    fn disarm_expiry(&self, id: u32) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }

    /// Schedules a popup expiry for `id` per the spec's `expire_timeout` (`>0` ms, `0` = never, `<0` = the configured default) and the urgency/critical-sticky policy. A detached timer keeps this independent of any surface, so popups expire correctly across focus changes and reloads. The notification stays in the history.
    fn schedule_expiry(&self, id: u32, expire_timeout: i32, urgency: Urgency) {
        let Some(after) = expiry_delay(expire_timeout, urgency, &self.policy()) else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("hogar-shell-notif-expiry".to_string())
            .spawn(move || {
                std::thread::sleep(after);
                expire(id);
            });
    }
}

/// How long a popup has on screen once it is shown, or `None` when it waits to be dealt with instead.
///
/// A sticky `critical` ignores both the sender's `expire_timeout` and the configured one — that is what sticky means — and waits out `critical_max` instead. Without a ceiling it waits forever, and forever is the one answer with no way out: the notification's only remaining exit is a gesture, so a gesture that does not land leaves it on screen until the shell restarts. Expiring is not dismissing, so the ceiling costs nothing — the notification is still in the history panel afterwards.
fn expiry_delay(expire_timeout: i32, urgency: Urgency, policy: &Policy) -> Option<Duration> {
    if urgency == Urgency::Critical && policy.critical_sticky {
        return policy.critical_max;
    }
    let ms = match expire_timeout {
        t if t > 0 => t as u64,
        // The spec's "never expire". Honoured for a non-critical notification, whose card can still be pressed and swiped — it is the critical one, floated above everything and outliving the column, that needs a floor under it.
        0 => return None,
        _ => policy.timeout.as_millis() as u64,
    };
    (ms > 0).then(|| Duration::from_millis(ms))
}

pub struct NotificationService {
    inner: Arc<Inner>,
}

static SERVICE: OnceLock<NotificationService> = OnceLock::new();
/// The daemon's live D-Bus connection, kept so action invocations and closes can emit their signals from any thread (the daemon thread just parks). Set once the bus name is claimed.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Starts the daemon once for the whole process (before any surface, so its state survives config reloads). Do-Not-Disturb and the per-application mutes are restored from the persisted shell state, so a toggle survives a restart rather than quietly re-arming every sender. Idempotent; a second call is a no-op — use [`set_policy`] to hand a running daemon a reloaded config.
pub fn init(policy: Policy) {
    SERVICE.get_or_init(|| {
        // Restored notifications deserialize with `popup = false`, so they populate the history panel without re-popping on login; ids continue past the highest restored one.
        let restored = load_history();
        let next_id = restored.iter().map(|n| n.id).max().unwrap_or(0);
        let (saver, saver_rx) = channel::<Vec<Notification>>();
        let _ = std::thread::Builder::new()
            .name("hogar-shell-notif-save".to_string())
            .spawn(move || run_saver(saver_rx));
        let remembered = crate::state::get();
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                active: restored,
                next_id,
                unread: 0,
                dnd: remembered.dnd,
                muted_apps: remembered.muted_apps,
            }),
            subscribers: Mutex::new(Vec::new()),
            policy: Mutex::new(policy),
            saver,
            pending: Mutex::new(HashMap::new()),
        });
        spawn_daemon(Arc::clone(&inner));
        NotificationService { inner }
    });
}

/// Replaces the running daemon's policy, so a `[notifications]` edit applies without restarting the shell (and without dropping the D-Bus name or the history). A no-op before [`init`].
pub fn set_policy(policy: Policy) {
    if let Some(service) = SERVICE.get() {
        *service.inner.policy.lock().unwrap() = policy;
    }
}

/// Registers `tx` (bound to a surface's event loop) to receive every state change, and immediately sends the current snapshot so the surface starts in sync. Called from a surface's `watch` producer; a no-op before [`init`].
pub fn subscribe(tx: EventSender<SharedSnapshot>) {
    if let Some(service) = SERVICE.get() {
        let snapshot = service.inner.state.lock().unwrap().snapshot();
        if tx.send(snapshot) {
            service.inner.subscribers.lock().unwrap().push(tx);
        }
    }
}

/// Raises a notification from inside the shell itself, without a D-Bus round-trip — how hogar-shell reports its own problems (a config that won't parse, a service that won't start) through the same surface every other app's notifications land on. `Critical` urgency, so with the default `critical_sticky` it waits to be read rather than timing out. Falls back to stderr before the daemon is up.
pub fn notify_local(app_name: &str, summary: &str, body: &str) {
    notify_shell(app_name, summary, body, "", Urgency::Critical);
}

/// [`notify_local`] with an icon and an urgency of its own — for the shell's own *advisories* (a battery running low) rather than its errors, which should not all shout at `Critical`.
pub fn notify_shell(app_name: &str, summary: &str, body: &str, app_icon: &str, urgency: Urgency) {
    let Some(service) = SERVICE.get() else {
        eprintln!("{app_name}: {summary} — {body}");
        return;
    };
    service.inner.push(
        Notification {
            id: 0,
            app_name: app_name.to_string(),
            app_icon: app_icon.to_string(),
            summary: summary.to_string(),
            body: body.to_string(),
            actions: Vec::new(),
            urgency,
            popup: true,
            image: None,
        },
        0,
    );
}

/// The current state without subscribing — for an initial read or tests; surfaces should [`subscribe`] to stay live.
pub fn snapshot_now() -> Option<SharedSnapshot> {
    SERVICE
        .get()
        .map(|service| service.inner.state.lock().unwrap().snapshot())
}

/// Tells the daemon `id`'s popup is on screen, which is when its expiry clock starts.
///
/// **Arrival is the wrong moment.** The shell shows a bounded column of cards and queues the rest, so a notification that arrives fifth waits — and one whose timer began on arrival would spend that wait burning its own life and vanish almost as it appeared. Called by the column for every card it draws, and spent the first time: a notification that stays up does not get its clock restarted on every repaint.
pub fn shown(id: u32) {
    if let Some(service) = SERVICE.get() {
        service.inner.start_expiry(id);
    }
}

/// Removes one notification from the history — a manual dismiss (history-card tap). Emits `NotificationClosed`.
pub fn close(id: u32) {
    if let Some(service) = SERVICE.get() {
        service.inner.close(id);
    }
    emit_closed(id, 2);
}

/// Retires a notification's popup after its timeout while keeping it in the history. Emits `NotificationClosed` with the expired reason, as the spec expects when a popup times out.
pub fn expire(id: u32) {
    if let Some(service) = SERVICE.get() {
        service.inner.expire(id);
    }
    emit_closed(id, 1);
}

/// Invokes a notification's action `key`: emits `ActionInvoked`, then closes it (the sender closes on invocation, per the spec). Wired to the history panel's action buttons.
pub fn invoke_action(id: u32, key: &str) {
    if let Some(conn) = CONNECTION.get() {
        let _ = conn.emit_signal(
            None::<&str>,
            OBJECT_PATH,
            BUS_NAME,
            "ActionInvoked",
            &(id, key),
        );
    }
    close(id);
}

/// Emits `NotificationClosed(id, reason)` (1 = expired, 2 = dismissed, 3 = app-requested) to any listeners.
fn emit_closed(id: u32, reason: u32) {
    if let Some(conn) = CONNECTION.get() {
        let _ = conn.emit_signal(
            None::<&str>,
            OBJECT_PATH,
            BUS_NAME,
            "NotificationClosed",
            &(id, reason),
        );
    }
}

/// Clears the whole history and resets the unread count.
pub fn clear_all() {
    if let Some(service) = SERVICE.get() {
        service.inner.commit(|state| {
            state.active.clear();
            state.unread = 0;
        });
    }
}

/// Clears one application's notifications — the group header's own clear, so dismissing a run of chat messages is one gesture rather than one per card. Every removed id gets its `NotificationClosed`, since a sender watching for its own notification to go away cannot tell a group clear from a card tap.
pub fn clear_app(app_name: &str) {
    let Some(service) = SERVICE.get() else { return };
    for id in service.inner.clear_app(app_name) {
        emit_closed(id, 2);
    }
}

/// Marks the history as seen without discarding it (e.g. when the bell panel opens).
pub fn mark_read() {
    if let Some(service) = SERVICE.get() {
        service.inner.commit(|state| state.unread = 0);
    }
}

/// Toggles Do-Not-Disturb; popups are suppressed while on, history keeps recording. Persisted, so the toggle means the same thing after a restart as it did before one.
///
/// **Switching it on retires what is already on screen**, rather than hiding it until the toggle comes back. Do-Not-Disturb is a request to stop being shown things, and a column that emptied and then refilled the moment it was switched off would be delivering the interruption it was asked to prevent — late, and with newer cards already above it. Everything retired stays in the history, which is where it was going anyway.
pub fn set_dnd(dnd: bool) {
    if let Some(service) = SERVICE.get() {
        service.inner.set_dnd(dnd);
    }
    crate::state::update(move |s| s.dnd = dnd);
}

/// Mutes or unmutes one application: its notifications keep arriving into the history and stop popping. Persisted alongside Do-Not-Disturb, and applied at the daemon's single entry point rather than per surface.
pub fn set_app_muted(app_name: &str, muted: bool) {
    let app = app_name.to_string();
    if let Some(service) = SERVICE.get() {
        let app = app.clone();
        service.inner.commit(move |state| {
            state.muted_apps.retain(|a| *a != app);
            if muted {
                state.muted_apps.push(app);
            }
        });
    }
    crate::state::update(move |s| {
        s.muted_apps.retain(|a| *a != app);
        if muted {
            s.muted_apps.push(app);
        }
    });
}

/// Whether `app_name` is muted, without subscribing — for IPC and tests; a surface reads the snapshot instead.
pub fn is_app_muted(app_name: &str) -> bool {
    snapshot_now().is_some_and(|s| s.is_muted(app_name))
}

/// The command a pop should run, if the policy configures one. Trimmed so a whitespace-only setting is silent.
fn sound_command(configured: &str) -> Option<String> {
    let command = configured.trim();
    (!command.is_empty()).then(|| command.to_string())
}

/// The persisted history file: a TOML array of tables under the data dir.
#[derive(Default, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    notifications: Vec<Notification>,
}

/// Owns the disk writes on its own thread: takes each new history snapshot, debounces a burst (keeping only the last), and writes it. Ends when the daemon — and thus the sender — is dropped (i.e. process exit).
fn run_saver(rx: Receiver<Vec<Notification>>) {
    let path = history_path();
    while let Ok(mut latest) = rx.recv() {
        while let Ok(next) = rx.recv_timeout(Duration::from_millis(500)) {
            latest = next;
        }
        save_history(&path, &latest);
    }
}

fn load_history() -> Vec<Notification> {
    let Ok(text) = fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    match toml::from_str::<HistoryFile>(&text) {
        Ok(file) => file.notifications,
        Err(e) => {
            tracing::warn!(
                "notification history parse error ({e}); starting with an empty history"
            );
            Vec::new()
        }
    }
}

/// Writes the most recent [`MAX_HISTORY`] notifications. Best-effort: a failure is logged, not surfaced.
fn save_history(path: &Path, active: &[Notification]) {
    let start = active.len().saturating_sub(MAX_HISTORY);
    let file = HistoryFile {
        notifications: active[start..].to_vec(),
    };
    match toml::to_string_pretty(&file) {
        Ok(text) => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(path, text) {
                tracing::warn!("notification history save failed: {e}");
            }
        }
        Err(e) => tracing::warn!("notification history serialize failed: {e}"),
    }
}

fn history_path() -> PathBuf {
    util::paths::data_dir().join("notifications.toml")
}

fn spawn_daemon(inner: Arc<Inner>) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-notifications".to_string())
        .spawn(move || run_daemon(inner));
}

fn run_daemon(inner: Arc<Inner>) {
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name(BUS_NAME))
        .and_then(|b| b.serve_at(OBJECT_PATH, NotificationsIface { inner }))
        .and_then(|b| b.build());
    match conn {
        Ok(conn) => {
            let _ = CONNECTION.set(conn);
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            tracing::warn!(
                "notifications daemon not started ({e}); another daemon likely owns {BUS_NAME}"
            );
        }
    }
}

struct NotificationsIface {
    inner: Arc<Inner>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsIface {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, Value<'_>>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints
            .get("urgency")
            .and_then(|v| u8::try_from(v).ok())
            .map(urgency_from_hint)
            .unwrap_or(Urgency::Normal);
        let image = extract_image(&hints);
        // freedesktop icon precedence for the card's leading visual: raw `image-data` (kept in `image`) wins, then the `image-path` hint, then the `app_icon` parameter. Folding image-path into `app_icon` lets the UI resolve a single reference — and, unlike the raw pixels, it persists into the history.
        let app_icon = image_path_hint(&hints).unwrap_or(app_icon);
        let id = self.inner.push(
            Notification {
                id: 0,
                app_name,
                app_icon,
                summary,
                body,
                actions,
                urgency,
                popup: true,
                image,
            },
            replaces_id,
        );
        self.inner.arm_expiry(id, expire_timeout, urgency);
        id
    }

    fn close_notification(&self, id: u32) {
        self.inner.close(id);
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".to_string(),
            "body-markup".to_string(),
            "actions".to_string(),
            "icon-static".to_string(),
        ]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "hogar-shell".to_string(),
            "hogar-shell".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }
}

/// Pulls a notification image out of the spec's raw-pixel hints (`image-data`, its underscore variant, or the legacy `icon_data`) — each an `(iiibiiay)` struct — and converts it to RGBA8. `None` if absent or malformed.
fn extract_image(hints: &HashMap<String, Value<'_>>) -> Option<NotificationImage> {
    ["image-data", "image_data", "icon_data"]
        .iter()
        .find_map(|key| hints.get(*key).and_then(image_from_hint))
}

/// The `image-path` hint (or its underscore variant): a file path, `file://` URI, or themed icon name for the notification's image. `None` if absent or empty.
fn image_path_hint(hints: &HashMap<String, Value<'_>>) -> Option<String> {
    ["image-path", "image_path"]
        .iter()
        .find_map(|key| match hints.get(*key) {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

fn image_from_hint(value: &Value<'_>) -> Option<NotificationImage> {
    let Value::Structure(s) = value else {
        return None;
    };
    let fields = s.fields();
    if fields.len() < 7 {
        return None;
    }
    let width = u32::try_from(i32::try_from(&fields[0]).ok()?).ok()?;
    let height = u32::try_from(i32::try_from(&fields[1]).ok()?).ok()?;
    let rowstride = usize::try_from(i32::try_from(&fields[2]).ok()?).ok()?;
    let channels = usize::try_from(i32::try_from(&fields[5]).ok()?).ok()?;
    let Value::Array(array) = &fields[6] else {
        return None;
    };
    let data: Vec<u8> = array.iter().filter_map(|v| u8::try_from(v).ok()).collect();
    let rgba = to_rgba(width, height, rowstride, channels, &data)?;
    Some(NotificationImage {
        width,
        height,
        rgba,
    })
}

/// Repacks raw image bytes (3-channel RGB or 4-channel RGBA, laid out with `rowstride`-byte rows) into tight RGBA8. `None` if the channel count is unsupported or the data is short.
fn to_rgba(
    width: u32,
    height: u32,
    rowstride: usize,
    channels: usize,
    data: &[u8],
) -> Option<Vec<u8>> {
    if channels != 3 && channels != 4 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = data.get(y * rowstride..y * rowstride + w * channels)?;
        for x in 0..w {
            let px = &row[x * channels..x * channels + channels];
            let alpha = if channels == 4 { px[3] } else { 255 };
            out.extend_from_slice(&[px[0], px[1], px[2], alpha]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daemon core with no D-Bus name, no saver thread and no subscribers — everything `Inner` decides is decided here, so the tests drive it directly rather than through the process-wide service.
    fn test_inner(muted_apps: Vec<String>) -> Inner {
        Inner {
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(State {
                active: Vec::new(),
                next_id: 0,
                unread: 0,
                dnd: false,
                muted_apps,
            }),
            subscribers: Mutex::new(Vec::new()),
            policy: Mutex::new(Policy {
                timeout: Duration::from_millis(5000),
                critical_sticky: true,
                critical_max: Some(Duration::from_secs(120)),
                sound: String::new(),
            }),
            saver: channel().0,
        }
    }

    fn policy_with(critical_sticky: bool, critical_max: Option<Duration>) -> Policy {
        Policy {
            timeout: Duration::from_millis(5000),
            critical_sticky,
            critical_max,
            sound: String::new(),
        }
    }

    /// **A sticky `critical` still has a floor under it.**
    ///
    /// Sticky means "long enough that it cannot be missed", and read as *forever* it has one failure mode with no way out: the card's only exit is a gesture, so a gesture that never lands leaves it on screen until the shell restarts. The ceiling retires it to the history rather than dismissing it, so nothing is lost.
    #[test]
    fn a_sticky_critical_expires_at_its_ceiling_and_only_waits_forever_when_asked_to() {
        let bounded = policy_with(true, Some(Duration::from_secs(120)));
        assert_eq!(
            expiry_delay(-1, Urgency::Critical, &bounded),
            Some(Duration::from_secs(120)),
            "a sticky critical waits out the ceiling rather than the configured timeout"
        );
        assert_eq!(
            expiry_delay(0, Urgency::Critical, &bounded),
            Some(Duration::from_secs(120)),
            "the ceiling outranks the sender asking for no expiry at all, which is the case that stranded one"
        );
        assert_eq!(
            expiry_delay(-1, Urgency::Critical, &policy_with(true, None)),
            None,
            "`critical_max_secs = 0` is still how to ask for the unbounded wait"
        );
    }

    /// The ceiling is the sticky path's alone: everything else keeps answering to the sender and the config.
    #[test]
    fn the_ceiling_leaves_every_other_notification_alone() {
        let policy = policy_with(true, Some(Duration::from_secs(120)));
        assert_eq!(
            expiry_delay(-1, Urgency::Normal, &policy),
            Some(Duration::from_millis(5000)),
            "a normal notification takes the configured timeout"
        );
        assert_eq!(
            expiry_delay(800, Urgency::Normal, &policy),
            Some(Duration::from_millis(800)),
            "a sender that names its own timeout still gets it"
        );
        assert_eq!(
            expiry_delay(0, Urgency::Normal, &policy),
            None,
            "the spec's `never` is honoured for a card that can still be pressed and swiped"
        );
        assert_eq!(
            expiry_delay(-1, Urgency::Critical, &policy_with(false, None)),
            Some(Duration::from_millis(5000)),
            "with stickiness off a critical is an ordinary notification"
        );
    }

    fn sample_from(app: &str, summary: &str) -> Notification {
        Notification {
            id: 0,
            app_name: app.into(),
            app_icon: String::new(),
            summary: summary.into(),
            body: String::new(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            popup: true,
            image: None,
        }
    }

    #[test]
    fn push_assigns_ids_replaces_and_counts_unread() {
        let inner = test_inner(Vec::new());
        let sample = |summary: &str| sample_from("app", summary);

        let first = inner.push(sample("a"), 0);
        let second = inner.push(sample("b"), 0);
        assert_ne!(first, second, "fresh notifications get distinct ids");
        assert_eq!(inner.state.lock().unwrap().unread, 2);

        inner.push(sample("b-edited"), second);
        let state = inner.state.lock().unwrap();
        assert_eq!(
            state.active.len(),
            2,
            "a replaces_id updates in place, no new entry"
        );
        assert_eq!(state.active[1].summary, "b-edited");
        assert_eq!(state.unread, 2, "a replacement does not bump unread");
        drop(state);

        inner.close(first);
        assert_eq!(inner.state.lock().unwrap().active.len(), 1);
    }

    #[test]
    fn history_round_trips_and_restored_notifications_do_not_popup() {
        let file = HistoryFile {
            notifications: vec![Notification {
                id: 7,
                app_name: "Slack".into(),
                app_icon: String::new(),
                summary: "Ada".into(),
                body: "review at 3?".into(),
                actions: vec!["default".into(), "Open".into()],
                urgency: Urgency::Critical,
                popup: true,
                image: None,
            }],
        };
        let text = toml::to_string_pretty(&file).expect("serialize");
        let parsed: HistoryFile = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.notifications.len(), 1);
        let n = &parsed.notifications[0];
        assert_eq!((n.id, n.summary.as_str()), (7, "Ada"));
        assert_eq!(n.urgency, Urgency::Critical);
        assert_eq!(n.actions, vec!["default".to_string(), "Open".to_string()]);
        // `popup` is runtime-only: it is never written, and a restored notification comes back non-popping.
        assert!(!text.contains("popup"), "popup must not be persisted");
        assert!(
            !n.popup,
            "restored notifications must not re-popup on startup"
        );
    }

    #[test]
    fn to_rgba_repacks_channels_and_honors_rowstride() {
        // 2×1 RGB, tight rows: alpha filled to 255.
        assert_eq!(
            to_rgba(2, 1, 6, 3, &[10, 20, 30, 40, 50, 60]).unwrap(),
            vec![10, 20, 30, 255, 40, 50, 60, 255]
        );
        // 1×2 RGBA: passes through unchanged.
        assert_eq!(
            to_rgba(1, 2, 4, 4, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
        // 1×2 RGB with a padded rowstride (3 bytes + 1 pad per row): the pad byte is skipped.
        assert_eq!(
            to_rgba(1, 2, 4, 3, &[1, 2, 3, 0, 4, 5, 6, 0]).unwrap(),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
        // Short data and unsupported channel counts are rejected.
        assert!(to_rgba(2, 1, 6, 3, &[1, 2, 3]).is_none());
        assert!(to_rgba(1, 1, 2, 2, &[1, 2]).is_none());
    }

    #[test]
    fn expiry_retires_the_popup_but_keeps_the_notification_in_history() {
        let inner = test_inner(Vec::new());
        let id = inner.push(sample_from("a", "hi"), 0);

        inner.expire(id);
        {
            let state = inner.state.lock().unwrap();
            assert_eq!(state.active.len(), 1, "expiry keeps it in the history");
            assert!(!state.active[0].popup, "but retires its popup");
        }

        inner.close(id);
        assert!(
            inner.state.lock().unwrap().active.is_empty(),
            "a manual dismiss removes it from the history"
        );
    }

    #[test]
    fn a_muted_app_is_recorded_and_never_popped() {
        let inner = test_inner(vec!["Slack".to_string()]);
        inner.push(sample_from("Slack", "muted"), 0);
        inner.push(sample_from("Calendar", "heard"), 0);

        let state = inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 2, "a mute silences, it does not drop");
        assert!(
            !state.active[0].popup,
            "the muted sender never reaches the screen"
        );
        assert!(state.active[1].popup, "and nobody else is affected by it");
        assert_eq!(state.unread, 2, "a muted notification is still one to read");
    }

    /// **Do-Not-Disturb suppresses; it does not defer.**
    ///
    /// Switching it on retires what is on screen, and anything arriving under it is recorded as not popping — so switching it *off* brings nothing back. The alternative, which this replaced, was a filter applied where the card is drawn: the notifications stayed marked as popping, so the moment the toggle went off the column refilled with everything it had been asked to hide, underneath whatever had arrived since.
    #[test]
    fn dnd_retires_what_it_hides_and_switching_it_off_brings_nothing_back() {
        let inner = test_inner(Vec::new());
        inner.push(sample_from("Calendar", "before"), 0);
        assert!(
            inner.state.lock().unwrap().active[0].popup,
            "on screen before the toggle"
        );

        inner.set_dnd(true);
        inner.push(sample_from("Slack", "during"), 0);

        inner.set_dnd(false);
        let state = inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 2, "both are still in the history");
        assert!(
            state.active.iter().all(|n| !n.popup),
            "neither the one it hid nor the one that arrived under it comes back"
        );
        assert_eq!(state.unread, 2, "and both are still unread");
    }

    #[test]
    fn clearing_one_group_leaves_every_other_app_alone() {
        let inner = test_inner(Vec::new());
        let first = inner.push(sample_from("Slack", "a"), 0);
        let second = inner.push(sample_from("Slack", "b"), 0);
        inner.push(sample_from("Calendar", "standup"), 0);

        let closed = inner.clear_app("Slack");
        assert_eq!(
            closed,
            vec![first, second],
            "every cleared id comes back to be closed on the bus"
        );
        let state = inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 1);
        assert_eq!(state.active[0].app_name, "Calendar");

        drop(state);
        assert!(
            inner.clear_app("Nobody").is_empty(),
            "clearing an app with nothing waiting closes nothing"
        );
    }

    // Live D-Bus round-trip. Run under a private bus so it never collides with the desktop's real daemon: `dbus-run-session -- cargo test -p hogar-shell --lib notifications::tests::daemon -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a session bus; run under dbus-run-session"]
    fn daemon_receives_notify_over_dbus() {
        init(Policy {
            timeout: Duration::from_millis(5000),
            critical_sticky: true,
            critical_max: Some(Duration::from_secs(120)),
            sound: String::new(),
        });
        let client = zbus::blocking::Connection::session().expect("session bus");
        let hints: HashMap<&str, Value> = HashMap::new();
        let mut sent = false;
        for _ in 0..50 {
            let call = client.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "test-app",
                    0u32,
                    "",
                    "Hello",
                    "World",
                    Vec::<&str>::new(),
                    &hints,
                    -1i32,
                ),
            );
            if call.is_ok() {
                sent = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(sent, "daemon claimed the name and answered Notify");

        let snapshot = snapshot_now().expect("service initialized");
        assert_eq!(snapshot.active.len(), 1);
        assert_eq!(snapshot.active[0].summary, "Hello");
        assert_eq!(snapshot.active[0].body, "World");
        assert_eq!(snapshot.unread, 1);
    }
}
