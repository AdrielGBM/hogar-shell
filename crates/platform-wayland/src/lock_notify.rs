//! `hyprland-lock-notify-v1`: the compositor's own word on whether the session is locked.
//!
//! `ext-session-lock-v1` has no read side. A client learns that the session is locked by locking it, so "is the session locked right now?" is a question a starting shell cannot put to it — on an unlocked session, asking *is* locking. This protocol answers it. `get_lock_notification` sends `locked` at once when the session already is, and `locked` / `unlocked` at every transition after, and its `locked` is specified to correspond to ext-session-lock's own. So one roundtrip settles the current state — a `locked` before the sync means locked, nothing by the sync means unlocked — and the same kind of object keeps it current from then on.
//!
//! Hyprland's own protocol, which `wayland-protocols` does not carry, so the XML is vendored beside this file with its BSD licence header intact: `protocols/hyprland-lock-notify-v1.xml` from hyprwm/hyprland-protocols at tag v0.7.0 (`bd153e76f751f150a09328dbdeb5e4fab9d23622`), byte-identical to `5c2317e086324053d97f5e0aa8a5132dfef5e013`, the commit that added it and the last one to touch it.

use wayland_client::globals::GlobalList;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};

use crate::platform::Driver;
use protocol::hyprland_lock_notification_v1::{self, HyprlandLockNotificationV1};
use protocol::hyprland_lock_notifier_v1::HyprlandLockNotifierV1;

#[allow(
    dead_code,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    clippy::all
)]
mod protocol {
    use wayland_client;

    pub mod __interfaces {
        // The generated interface tables name `wayland_backend`, which wayland-client re-exports; aliasing it here saves a direct dependency that would have to be kept in step with the one wayland-client already pins.
        use wayland_client::backend as wayland_backend;
        wayland_scanner::generate_interfaces!("src/protocols/hyprland-lock-notify-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("src/protocols/hyprland-lock-notify-v1.xml");
}

/// The global a compositor advertises when it can say whether the session is locked.
pub const LOCK_NOTIFIER_INTERFACE: &str = "hyprland_lock_notifier_v1";

/// Whether the compositor says the session is locked.
///
/// Three answers, because the third is not a softer form of either of the others. [`CannotTell`](Self::CannotTell) means nothing here could ask: the compositor does not implement `hyprland-lock-notify-v1`, the first read failed, or the caller is not on the driver's thread — outside a running shell there is no driver at all. It is never evidence that the session is unlocked, and a caller deciding whether to lock must not read it as one. The same line [`advertises`](crate::advertises) draws between "the compositor does not have it" and "nothing here could tell", drawn one level up: here even a compositor that lacks the protocol leaves the *question* unanswered, because the session may be locked all the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompositorLock {
    Locked,
    Unlocked,
    #[default]
    CannotTell,
}

/// The compositor's answer now. Live state rather than a bind-time answer — the session locks and unlocks while the shell runs, whoever does it — kept current for the whole session by the notification the driver holds. Read on the driver thread; anywhere else it is [`CompositorLock::CannotTell`], because there is no driver there to have asked.
///
/// Its one reader today is the shell's startup restore (`services::lock::restore`), deciding whether to take back a lock the previous process died holding. Being live is what keeps that read true: a transition dispatched between the startup probe and the read has already rewritten the answer, where a probe-only answer would be stale.
///
/// Not the same question as [`session_is_locked`](crate::session_is_locked), which answers only for a lock this process took, and does so from any thread.
pub fn compositor_lock() -> CompositorLock {
    crate::platform::with_driver_facts(|facts| facts.compositor_lock)
}

/// One event on a notification object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notice {
    Locked,
    Unlocked,
}

fn notice(event: hyprland_lock_notification_v1::Event) -> Notice {
    match event {
        hyprland_lock_notification_v1::Event::Locked => Notice::Locked,
        hyprland_lock_notification_v1::Event::Unlocked => Notice::Unlocked,
    }
}

/// What the events a new notification received before a sync say about the session *now*: the last of them, or unlocked when there were none. The protocol sends `locked` at once to a notification created while the session is locked, and the compositor handles requests in order, so a sync sent after the creation comes back after that `locked` if there is one — its absence by then is the compositor saying the session is not locked.
fn settle(notices: &[Notice]) -> CompositorLock {
    match notices.last() {
        Some(Notice::Locked) => CompositorLock::Locked,
        Some(Notice::Unlocked) | None => CompositorLock::Unlocked,
    }
}

/// The state behind the first read's private queue.
#[derive(Default)]
struct FirstRead {
    notices: Vec<Notice>,
}

impl Dispatch<HyprlandLockNotificationV1, ()> for FirstRead {
    fn event(
        state: &mut Self,
        _proxy: &HyprlandLockNotificationV1,
        event: hyprland_lock_notification_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        state.notices.push(notice(event));
    }
}

delegate_noop!(Driver: ignore HyprlandLockNotifierV1);

/// The live notification: every transition after startup, whoever caused it.
impl Dispatch<HyprlandLockNotificationV1, ()> for Driver {
    fn event(
        _state: &mut Self,
        _proxy: &HyprlandLockNotificationV1,
        event: hyprland_lock_notification_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let answer = match notice(event) {
            Notice::Locked => CompositorLock::Locked,
            Notice::Unlocked => CompositorLock::Unlocked,
        };
        tracing::debug!("hyprland-lock-notify-v1: session {answer:?}");
        crate::platform::update_driver_facts(|facts| facts.compositor_lock = answer);
    }
}

/// Binds the notifier where the compositor has one, and settles [`compositor_lock`] before anything reads it. Called once, at driver init, because the answer has to be there before the shell's startup tasks run, and a roundtrip cannot be made from inside the driver's own dispatch.
///
/// Two notifications, and their order is the point. The live one goes on the driver's queue **first**, so every transition from that moment on reaches it; the probe goes on a private queue after it, and that queue's roundtrip reads the state at a moment the live one already covers. The other way round, a transition landing between the two would be seen by neither, and the answer would be stale from the start. The live one's own first `locked`, if the session is locked, lands later on the driver's queue and says the same thing.
pub(crate) fn bind(globals: &GlobalList, conn: &Connection, qh: &QueueHandle<Driver>) {
    let notifier = match globals.bind::<HyprlandLockNotifierV1, Driver, ()>(qh, 1..=1, ()) {
        Ok(notifier) => notifier,
        Err(e) => {
            tracing::info!("hyprland-lock-notify-v1 unavailable: {e}");
            return;
        }
    };
    notifier.get_lock_notification(qh, ());
    let mut queue = conn.new_event_queue::<FirstRead>();
    let probe = notifier.get_lock_notification(&queue.handle(), ());
    let mut first = FirstRead::default();
    let answer = match queue.roundtrip(&mut first) {
        Ok(_) => settle(&first.notices),
        Err(e) => {
            tracing::warn!("hyprland-lock-notify-v1: the first read failed: {e}");
            CompositorLock::CannotTell
        }
    };
    probe.destroy();
    // Objects made through the notifier outlive it, so the live notification keeps reporting.
    notifier.destroy();
    tracing::debug!("hyprland-lock-notify-v1: session {answer:?} at startup");
    crate::platform::update_driver_facts(|facts| facts.compositor_lock = answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_before_the_sync_say_whether_the_session_is_locked_now() {
        assert_eq!(
            settle(&[Notice::Locked]),
            CompositorLock::Locked,
            "a new notification is sent `locked` at once when the session already is, ahead of the sync"
        );
        assert_eq!(
            settle(&[]),
            CompositorLock::Unlocked,
            "nothing by the sync is the compositor saying the session is not locked — the one reading that tells the two apart"
        );
        assert_eq!(
            settle(&[Notice::Locked, Notice::Unlocked]),
            CompositorLock::Unlocked,
            "a session unlocked while the read was in flight is unlocked now: the last transition is the answer, not the first"
        );
    }

    #[test]
    fn the_advertised_name_is_the_one_the_binding_binds() {
        assert_eq!(
            LOCK_NOTIFIER_INTERFACE,
            protocol::__interfaces::HYPRLAND_LOCK_NOTIFIER_V1_INTERFACE.name,
            "a dependency probe asking the registry for another name would report the notifier missing on a compositor the driver binds it from"
        );
    }

    #[test]
    fn with_no_driver_to_ask_the_answer_is_cannot_tell_never_unlocked() {
        assert_eq!(
            compositor_lock(),
            CompositorLock::CannotTell,
            "a thread that never bound the notifier knows nothing, and reading that as unlocked would let a caller act on a guess"
        );
    }
}
