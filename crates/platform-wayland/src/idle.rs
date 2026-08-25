//! `ext-idle-notify-v1`: how long the seat has been idle, reported by the compositor.
//!
//! The compositor is the only thing that knows. It sees every input device, it knows which surface has focus, and it already tracks the idle inhibitors clients take out — so a shell that timed its own inactivity would be guessing at all three, and would dim the screen under a full-screen video that had asked it not to.
//!
//! The protocol distinguishes the two questions directly, which is why `respect_inhibitors` maps onto a choice of request rather than onto a condition the shell evaluates: `get_idle_notification` stays quiet while an inhibitor is held, `get_input_idle_notification` reports raw input idleness regardless.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::platform::Driver;

/// The version that added `get_input_idle_notification`, i.e. the one that can ignore idle inhibitors.
const IGNORES_INHIBITORS_SINCE: u32 = 2;

/// What a notification needs to exist: the compositor's notifier, the seat whose idleness is being watched, and the queue the driver dispatches on. Installed once by the driver; `None` on a compositor without the protocol, which is what makes [`idle_notification`] return `None` rather than panic.
struct IdleEnv {
    notifier: ExtIdleNotifierV1,
    seat: wl_seat::WlSeat,
    qh: QueueHandle<Driver>,
}

/// The identity a notification's events are routed by. Kept out of the protocol object's user data because the callbacks it resolves to are `Rc` closures built on the driver thread, which user data (`Send + Sync`) may not hold.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IdleId(u32);

struct Callbacks {
    idled: Rc<dyn Fn()>,
    resumed: Rc<dyn Fn()>,
}

thread_local! {
    static ENV: RefCell<Option<IdleEnv>> = const { RefCell::new(None) };
    static HANDLERS: RefCell<HashMap<IdleId, Callbacks>> = RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u32> = const { RefCell::new(0) };
}

pub(crate) fn install(notifier: ExtIdleNotifierV1, seat: wl_seat::WlSeat, qh: QueueHandle<Driver>) {
    ENV.with(|env| *env.borrow_mut() = Some(IdleEnv { notifier, seat, qh }));
}

/// Whether the compositor reports idleness at all.
pub fn idle_supported() -> bool {
    ENV.with(|env| env.borrow().is_some())
}

/// A live idle notification. Dropping it stops the notification — which is how an inhibitor is applied: the stage that must not fire simply has no notification registered while the inhibit holds.
pub struct IdleHandle {
    id: IdleId,
    notification: ExtIdleNotificationV1,
}

impl Drop for IdleHandle {
    fn drop(&mut self) {
        self.notification.destroy();
        HANDLERS.with(|handlers| handlers.borrow_mut().remove(&self.id));
    }
}

/// Asks the compositor to say when the seat has been idle for `timeout`, and when it stops being.
///
/// `respect_inhibitors` picks which question is asked: `true` (the protocol's original request) stays silent while any client holds an idle inhibitor, `false` reports raw input idleness. A compositor implementing only version 1 cannot answer the second, so it falls back to the first — reported once, rather than silently ignoring a configured preference.
///
/// Returns `None` where the compositor does not implement the protocol. Must be called from the driver thread.
pub fn idle_notification(
    timeout: Duration,
    respect_inhibitors: bool,
    on_idle: impl Fn() + 'static,
    on_resume: impl Fn() + 'static,
) -> Option<IdleHandle> {
    let id = NEXT_ID.with(|next| {
        let mut next = next.borrow_mut();
        *next += 1;
        IdleId(*next)
    });
    let notification = ENV.with(|env| {
        let env = env.borrow();
        let env = env.as_ref()?;
        let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
        let ignores = !respect_inhibitors && env.notifier.version() >= IGNORES_INHIBITORS_SINCE;
        if !respect_inhibitors && !ignores {
            tracing::warn!(
                "this compositor's ext-idle-notify is version {}; idle timers will respect inhibitors regardless of the config",
                env.notifier.version()
            );
        }
        Some(if ignores {
            env.notifier
                .get_input_idle_notification(millis, &env.seat, &env.qh, id)
        } else {
            env.notifier
                .get_idle_notification(millis, &env.seat, &env.qh, id)
        })
    })?;
    HANDLERS.with(|handlers| {
        handlers.borrow_mut().insert(
            id,
            Callbacks {
                idled: Rc::new(on_idle),
                resumed: Rc::new(on_resume),
            },
        )
    });
    Some(IdleHandle { id, notification })
}

impl Dispatch<ExtIdleNotifierV1, ()> for Driver {
    fn event(
        _state: &mut Self,
        _proxy: &ExtIdleNotifierV1,
        _event: <ExtIdleNotifierV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, IdleId> for Driver {
    fn event(
        _state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        id: &IdleId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Cloned out of the map before running: a stage's action may register or drop other notifications (locking arms the next stage), and doing that inside the map's own borrow would panic.
        let callback = HANDLERS.with(|handlers| {
            let handlers = handlers.borrow();
            let callbacks = handlers.get(id)?;
            Some(match event {
                ext_idle_notification_v1::Event::Idled => Rc::clone(&callbacks.idled),
                ext_idle_notification_v1::Event::Resumed => Rc::clone(&callbacks.resumed),
                _ => return None,
            })
        });
        if let Some(callback) = callback {
            callback();
        }
    }
}
