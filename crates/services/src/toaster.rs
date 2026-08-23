//! In-shell toasts: the small, self-dismissing messages the shell says about itself.
//!
//! Deliberately not freedesktop notifications. A notification is a record — it belongs to an application, it goes
//! into history, it can be acted on, and under Do-Not-Disturb it is *kept* rather than shown. "Caps Lock is on"
//! is none of those things: it is feedback about a key the user just pressed, it is worthless a second later, and
//! filing it in the notification history would be filing the user's own keystrokes. So toasts have their own
//! queue, their own surface and their own per-event switches, and nothing here reaches the daemon.
//!
//! Expiry runs on a thread of its own. Toasts are posted from wherever the event happened — a service thread, an
//! IPC handler, a click — so the one thing they cannot rely on is a surface being up to time them out.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use platform_wayland::EventSender;

use util::broadcast::Store;

/// Every one is a switch in `[toasts.events]`, so the type is the config's; this is the name the queue reads it
/// by.
pub use config::policy::ToastEvent as Event;

/// One toast on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// Monotonic, so a list can key on it and a dismissal can name one.
    pub id: u64,
    pub event: Event,
    /// An Iconify name, resolved by the surface like every other icon in the shell.
    pub icon: String,
    pub title: String,
    pub body: String,
    expires_at: Instant,
}

impl Toast {
    /// A row's list key: what it draws, not just which toast it is — a replaced toast keeps its slot and has to
    /// redraw with the new text.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.id, self.title, self.body)
    }

    /// A toast built without going through the queue, so a surface test has a card to draw. Not gated on
    /// `cfg(test)`: the surface that draws one lives in another crate, where this crate's test build is not in
    /// scope.
    pub fn sample(event: Event, icon: &str, title: &str, body: &str) -> Self {
        Self {
            id: 1,
            event,
            icon: icon.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            expires_at: Instant::now() + Duration::from_secs(5),
        }
    }
}

static TOASTS: Store<Vec<Toast>> = Store::new(Vec::new);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static QUEUE: OnceLock<Sender<Message>> = OnceLock::new();

enum Message {
    Post(Toast),
    Dismiss(u64),
    Clear,
}

pub fn subscribe(tx: EventSender<Vec<Toast>>) {
    TOASTS.subscribe(tx);
}

pub fn current() -> Vec<Toast> {
    TOASTS.get()
}

/// Shows a toast for `event`, unless `[toasts]` has that event — or toasts altogether — switched off.
///
/// The gate lives here rather than at each call site: every place that reports something is a place that would
/// otherwise have to remember to ask, and the one that forgot would be the one the user cannot switch off.
pub fn post(event: Event, icon: &str, title: impl Into<String>, body: impl Into<String>) {
    let config = config::shared_config();
    let allowed = config
        .as_ref()
        .map(|c| c.toasts.allows(event))
        .unwrap_or(true);
    if !allowed {
        return;
    }
    // The column's lifetime, not a toast's own: a toast, a notification popup and an OSD are one stack now.
    let lifetime = config
        .as_ref()
        .map(|c| c.stack.lifetime())
        .unwrap_or_else(|| config::StackConfig::default().lifetime());
    let toast = Toast {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        event,
        icon: icon.to_string(),
        title: title.into(),
        body: body.into(),
        expires_at: Instant::now() + lifetime,
    };
    let _ = queue().send(Message::Post(toast));
}

pub fn dismiss(id: u64) {
    let _ = queue().send(Message::Dismiss(id));
}

pub fn clear() {
    let _ = queue().send(Message::Clear);
}

/// The expiry thread's channel, started on the first toast. A shell whose user switched every event off never
/// spawns it.
fn queue() -> &'static Sender<Message> {
    QUEUE.get_or_init(|| {
        let (tx, rx) = channel::<Message>();
        let _ = std::thread::Builder::new()
            .name("hogar-shell-toaster".to_string())
            .spawn(move || {
                let mut live: Vec<Toast> = Vec::new();
                loop {
                    // Wait until the next toast expires, or forever when nothing is showing — a queue with
                    // nothing in it must not wake once a second to discover that.
                    let message = match next_wait(&live) {
                        Some(wait) => match rx.recv_timeout(wait) {
                            Ok(message) => Some(message),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                            Err(_) => return,
                        },
                        None => match rx.recv() {
                            Ok(message) => Some(message),
                            Err(_) => return,
                        },
                    };
                    match message {
                        Some(Message::Post(toast)) => {
                            let max = config::shared_config()
                                .map(|c| c.stack.visible())
                                .unwrap_or(3);
                            admit(&mut live, toast, max);
                        }
                        Some(Message::Dismiss(id)) => live.retain(|toast| toast.id != id),
                        Some(Message::Clear) => live.clear(),
                        None => {}
                    }
                    let now = Instant::now();
                    live.retain(|toast| toast.expires_at > now);
                    TOASTS.update(|showing| *showing = live.clone());
                }
            });
        tx
    })
}

/// Puts `toast` on the stack: replacing the one already there for the same event, and dropping the oldest when
/// the stack is full.
///
/// Replacement rather than stacking, because the events here are *states*: two "microphone muted" toasts one
/// after the other are one fact reported twice, and a user spinning the volume wheel would otherwise bury their
/// own screen in identical cards.
fn admit(live: &mut Vec<Toast>, toast: Toast, max: usize) {
    if let Some(existing) = live.iter_mut().find(|showing| showing.event == toast.event) {
        *existing = toast;
        return;
    }
    live.push(toast);
    while live.len() > max.max(1) {
        live.remove(0);
    }
}

/// How long until the next toast expires, or `None` when none is showing.
fn next_wait(live: &[Toast]) -> Option<Duration> {
    let soonest = live.iter().map(|toast| toast.expires_at).min()?;
    Some(
        soonest
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(16)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(id: u64, event: Event, title: &str) -> Toast {
        Toast {
            id,
            event,
            icon: "info".to_string(),
            title: title.to_string(),
            body: String::new(),
            expires_at: Instant::now() + Duration::from_secs(5),
        }
    }

    #[test]
    fn a_second_toast_about_the_same_thing_replaces_the_first() {
        let mut live = vec![toast(1, Event::AudioOutput, "Speakers")];
        admit(&mut live, toast(2, Event::AudioOutput, "Headphones"), 3);
        assert_eq!(live.len(), 1, "one fact, one card");
        assert_eq!(live[0].title, "Headphones", "and it is the current one");

        admit(&mut live, toast(3, Event::Vpn, "VPN on"), 3);
        assert_eq!(live.len(), 2, "a different event is a different card");
    }

    #[test]
    fn a_full_stack_drops_the_oldest_rather_than_refusing_the_newest() {
        let mut live = vec![
            toast(1, Event::Dnd, "first"),
            toast(2, Event::Vpn, "second"),
        ];
        admit(&mut live, toast(3, Event::GameMode, "third"), 2);
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].title, "second", "the oldest made room");
        assert_eq!(live[1].title, "third");

        // A max of zero still shows the toast that was just posted; the alternative is a switch that silently
        // disables the feature while `enabled` says it is on.
        let mut live = vec![];
        admit(&mut live, toast(4, Event::Dnd, "only"), 0);
        assert_eq!(live.len(), 1);
    }

    #[test]
    fn an_empty_stack_parks_instead_of_polling() {
        assert!(
            next_wait(&[]).is_none(),
            "nothing showing, nothing to wake for"
        );
        let wait = next_wait(&[toast(1, Event::Dnd, "x")]).expect("a live toast has a deadline");
        assert!(wait <= Duration::from_secs(5) && wait > Duration::from_secs(4));

        // Already past: still a positive wait, so the loop turns once and reaps rather than spinning.
        let stale = Toast {
            expires_at: Instant::now() - Duration::from_secs(1),
            ..toast(2, Event::Dnd, "old")
        };
        assert!(next_wait(&[stale]).expect("a deadline") >= Duration::from_millis(16));
    }

    #[test]
    fn every_event_has_a_stable_id_that_round_trips() {
        for event in Event::ALL {
            assert_eq!(Event::from_id(event.id()), Some(event), "{}", event.id());
        }
        assert_eq!(Event::from_id("nonsense"), None);
    }

    #[test]
    fn a_row_is_keyed_on_what_it_draws() {
        let first = toast(1, Event::Vpn, "VPN on");
        let replaced = Toast {
            title: "VPN off".to_string(),
            ..first.clone()
        };
        assert_ne!(
            first.key(),
            replaced.key(),
            "the text changed, so the row must redraw"
        );
    }
}
