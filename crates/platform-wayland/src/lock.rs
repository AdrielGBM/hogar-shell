//! `ext-session-lock-v1`: the one surface role the compositor blanks the screen for.
//!
//! A lock surface is not a layer surface with a higher layer. The compositor stops rendering normal clients
//! entirely, refuses input to them, and will keep the session locked even if this process dies — which is what
//! makes it a lock rather than a very insistent overlay. The protocol's rules follow from that:
//!
//! - Lock surfaces must be created for **every** output the moment the lock object exists, and for any output
//!   plugged in afterwards, or the compositor blanks that screen to a solid colour instead.
//! - A second lock surface on one output is a protocol error, so [`LockSession::covered`] tracks which outputs
//!   already have one rather than trusting the surface list to stay in step.
//! - The first commit must follow an `ack_configure`, and the buffer must match the size it acked.
//! - Once the `locked` event has arrived, `destroy` is a protocol error — it must be `unlock_and_destroy`.
//!
//! The session lives on the driver, alongside the layer surfaces, because it shares everything they use: one
//! Wayland connection, one seat, one loop, and the same rsx handler path (a lock surface renders through
//! [`LayerWindow`] exactly as a bar does).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::reexports::calloop::LoopHandle;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use telar::{App, build_surface_handler};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::platform::{BoxedHandler, Driver, Shell, SurfaceEntry, tear_down};
use crate::window::LayerWindow;

/// Builds the lock surface for one output, named as the compositor names it (`None` for an output with no
/// name). One handler per output, so the lock screen can differ per monitor exactly as the bars do.
type LockFactory = Box<dyn Fn(Option<String>) -> BoxedHandler>;

/// Whether the compositor has the session locked, readable from **any** thread.
///
/// The per-session [`LockShared`] below is reached through a driver-thread handle, and the shell's own mirror of
/// it is refreshed by a timer on the driver's loop — so a busy driver could not tell anyone the screen was
/// already covered. That is not a cosmetic delay: it is what a suspend waits on before letting the machine
/// sleep, and a wait that cannot observe success gives up and sleeps anyway. Written where the compositor's own
/// `Locked`/`Finished` events are handled, so it is true exactly when the screen is.
static SESSION_LOCKED: AtomicBool = AtomicBool::new(false);

/// Whether the compositor currently has the session locked. Safe to call from any thread, and deliberately not
/// routed through the shell's polled copy — see [`SESSION_LOCKED`].
pub fn session_is_locked() -> bool {
    SESSION_LOCKED.load(Ordering::Relaxed)
}

/// The lock's state as the shell sees it, shared with the driver by atomics because the shell reads it from a
/// UI handler while the driver writes it from a Wayland callback.
pub(crate) struct LockShared {
    /// The compositor confirmed the session is locked and nothing sensitive is on screen.
    locked: AtomicBool,
    /// The compositor will never send `locked` for this object (refused, or it took the lock back).
    finished: AtomicBool,
    /// The shell asked to unlock; the driver performs it on its next turn.
    unlock: AtomicBool,
}

/// A live session lock. **Dropping it does not unlock** — an unlock has to be an explicit decision, since a
/// handle going out of scope by accident would put the user's screen back on display.
pub struct LockHandle {
    shared: Arc<LockShared>,
}

impl LockHandle {
    /// Asks the compositor to unlock. Returns immediately; the driver performs `unlock_and_destroy` and tears
    /// the lock surfaces down on its next loop turn.
    pub fn unlock(&self) {
        self.shared.unlock.store(true, Ordering::Relaxed);
    }

    /// Whether the compositor has confirmed the lock. `false` between asking and being granted — the window in
    /// which the screen may still be showing the desktop, so nothing security-sensitive may act on it.
    pub fn is_locked(&self) -> bool {
        self.shared.locked.load(Ordering::Relaxed)
    }

    /// Whether the compositor refused the lock, or ended it itself. A refused lock leaves the session *unlocked*
    /// — the caller must treat it as a failure to lock, not as a lock it merely cannot draw on.
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }
}

struct PendingLock {
    factory: Rc<LockFactory>,
    shared: Arc<LockShared>,
}

thread_local! {
    // A lock requested on the UI thread, mounted by the driver on its next turn — the same hand-off dynamic
    // surfaces use, so a lock triggered from a click handler needs no second thread or connection.
    static LOCK_QUEUE: RefCell<Option<PendingLock>> = const { RefCell::new(None) };
}

/// Whether this compositor implements `ext-session-lock-v1` at all. Read before offering to lock: a shell that
/// blanks its own screen with an overlay it cannot enforce is worse than one that says it cannot lock.
pub fn lock_supported() -> bool {
    crate::platform::with_driver_facts(|facts| facts.lock_supported)
}

/// Locks the session, drawing `factory`'s surface on every output.
///
/// The factory is called once per output, immediately, and again for any output connected while the lock is
/// up. Returns a handle whose [`LockHandle::is_locked`] turns true once the compositor confirms; a compositor
/// that refuses reports through [`LockHandle::is_finished`] instead.
pub fn lock_session<A, F>(factory: F) -> LockHandle
where
    A: App + 'static,
    F: Fn(Option<String>) -> A + 'static,
{
    let shared = Arc::new(LockShared {
        locked: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        unlock: AtomicBool::new(false),
    });
    let boxed: LockFactory = Box::new(move |output| {
        build_surface_handler::<LayerWindow, A>(
            factory(output),
            std::sync::Arc::new(telar::NoPaths),
            "hogar-shell",
        )
    });
    LOCK_QUEUE.with(|queue| {
        let displaced = queue.borrow_mut().replace(PendingLock {
            factory: Rc::new(boxed),
            shared: Arc::clone(&shared),
        });
        // A request the driver never saw would otherwise leave its owner waiting on a lock that will never be
        // granted or refused. Reported as finished, which is the answer "this did not happen".
        if let Some(displaced) = displaced {
            tracing::warn!(
                "a session lock was requested twice before the driver ran; dropping the first"
            );
            displaced.shared.finished.store(true, Ordering::Relaxed);
        }
    });
    LockHandle { shared }
}

/// The driver's live lock: the protocol object, how to build a surface for a new output, and which outputs are
/// already covered.
pub(crate) struct LockSession {
    lock: ExtSessionLockV1,
    factory: Rc<LockFactory>,
    shared: Arc<LockShared>,
    /// Outputs that already carry a lock surface. A second surface on one output is a protocol error, so this
    /// is tracked rather than derived from the surface list — an entry torn down for an unrelated reason must
    /// not read as "this output is free again".
    covered: Vec<ObjectId>,
}

/// One turn of the lock's lifecycle, run at the top of the driver loop: start a requested lock, cover any
/// output that has no surface yet, and tear the whole thing down on unlock or on the compositor finishing it.
pub(crate) fn poll(
    driver: &mut Driver,
    compositor: &CompositorState,
    qh: &QueueHandle<Driver>,
    conn: &Connection,
    loop_handle: &LoopHandle<'static, Driver>,
) {
    if let Some(pending) = LOCK_QUEUE.with(|queue| queue.borrow_mut().take()) {
        start(driver, pending, qh);
    }
    let Some(session) = driver.lock.as_ref() else {
        return;
    };
    let unlock = session.shared.unlock.load(Ordering::Relaxed);
    let finished = session.shared.finished.load(Ordering::Relaxed);
    if unlock || finished {
        end(driver, unlock, conn, loop_handle);
        return;
    }
    cover_outputs(driver, compositor, qh);
}

/// Takes the lock and covers every output before the compositor's first frame, which is what the protocol asks
/// for: surfaces created up front let it send `locked` without ever showing a blank screen.
fn start(driver: &mut Driver, pending: PendingLock, qh: &QueueHandle<Driver>) {
    if driver.lock.is_some() {
        tracing::warn!("a session lock is already up; ignoring the second request");
        pending.shared.finished.store(true, Ordering::Relaxed);
        return;
    }
    let Some(manager) = driver.lock_manager.clone() else {
        // Not an error the shell can work around, and not one it should paper over with an overlay: without
        // the protocol there is no way to stop the compositor rendering the desktop underneath.
        tracing::error!("this compositor does not implement ext-session-lock-v1; cannot lock");
        pending.shared.finished.store(true, Ordering::Relaxed);
        return;
    };
    let lock = manager.lock(qh, ());
    driver.lock = Some(LockSession {
        lock,
        factory: pending.factory,
        shared: pending.shared,
        covered: Vec::new(),
    });
    tracing::info!("session lock requested");
}

/// Creates a lock surface for every output that has none — at lock time for all of them, and afterwards for a
/// monitor plugged in while the screen is locked, which would otherwise show the compositor's fallback colour.
fn cover_outputs(driver: &mut Driver, compositor: &CompositorState, qh: &QueueHandle<Driver>) {
    // Checked on every loop turn for as long as the screen is locked, so the ordinary answer — "every output
    // already has one" — costs a count rather than a collect.
    if driver
        .lock
        .as_ref()
        .is_some_and(|session| session.covered.len() == driver.output_state.outputs().count())
    {
        return;
    }
    let outputs: Vec<wl_output::WlOutput> = driver.output_state.outputs().collect();
    for output in outputs {
        let id = output.id();
        if driver
            .lock
            .as_ref()
            .is_some_and(|session| session.covered.contains(&id))
        {
            continue;
        }
        create_surface(driver, compositor, qh, &output);
    }
}

fn create_surface(
    driver: &mut Driver,
    compositor: &CompositorState,
    qh: &QueueHandle<Driver>,
    output: &wl_output::WlOutput,
) {
    let info = driver.output_state.info(output);
    let name = info.as_ref().and_then(|i| i.name.clone());
    let scale = info.as_ref().map(|i| i.scale_factor).unwrap_or(1).max(1);
    // A placeholder until the compositor's configure, which is authoritative: the lock surface must commit at
    // exactly the size it acked, so nothing is rendered before that arrives.
    let logical = info
        .as_ref()
        .and_then(|i| i.logical_size)
        .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32))
        .unwrap_or((1, 1));

    let Some((lock, factory)) = driver
        .lock
        .as_ref()
        .map(|session| (session.lock.clone(), Rc::clone(&session.factory)))
    else {
        return;
    };
    let surface = compositor.create_surface(qh);
    let lock_surface = lock.get_lock_surface(&surface, output, qh, ());
    let wl_id = surface.id();
    let handler = factory(name);

    let mut entry = SurfaceEntry::new(
        Shell::Lock {
            surface,
            lock: lock_surface,
        },
        wl_id,
        Some(handler),
        None,
        String::from("hogar-shell-lock"),
        scale,
        logical,
    );
    driver.attach_scaling(&mut entry, qh);
    driver.surfaces.push(entry);
    if let Some(session) = driver.lock.as_mut() {
        session.covered.push(output.id());
    }
}

/// Ends the lock: `unlock_and_destroy` once the compositor confirmed it (`destroy` would be a protocol error),
/// a plain `destroy` when it never did, then the surfaces. Flushed before returning, because the unlock is a
/// request the compositor must have processed — an exit racing it would leave the session locked.
fn end(
    driver: &mut Driver,
    unlock: bool,
    conn: &Connection,
    loop_handle: &LoopHandle<'static, Driver>,
) {
    let Some(session) = driver.lock.take() else {
        return;
    };
    let locked = session.shared.locked.load(Ordering::Relaxed);
    if locked {
        session.lock.unlock_and_destroy();
        if unlock {
            tracing::info!("session unlocked");
        }
    } else {
        session.lock.destroy();
        tracing::warn!("the compositor ended the session lock before it was granted");
    }
    // Only after the lock object is gone, per the protocol's ordering.
    let mut index = 0;
    while index < driver.surfaces.len() {
        if matches!(driver.surfaces[index].shell, Shell::Lock { .. }) {
            let entry = driver.surfaces.remove(index);
            tear_down(entry, loop_handle);
        } else {
            index += 1;
        }
    }
    session.shared.locked.store(false, Ordering::Relaxed);
    SESSION_LOCKED.store(false, Ordering::Relaxed);
    let _ = conn.flush();
}

/// Drops the lock surface belonging to a disconnected output, so reconnecting the monitor gets a fresh one
/// rather than being skipped as already covered.
pub(crate) fn forget_output(driver: &mut Driver, output: &wl_output::WlOutput) {
    let id = output.id();
    if let Some(session) = driver.lock.as_mut() {
        session.covered.retain(|covered| covered != &id);
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Driver {
    fn event(
        _state: &mut Self,
        _proxy: &ExtSessionLockManagerV1,
        _event: <ExtSessionLockManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockV1, ()> for Driver {
    fn event(
        state: &mut Self,
        _proxy: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(session) = state.lock.as_ref() else {
            return;
        };
        match event {
            ext_session_lock_v1::Event::Locked => {
                session.shared.locked.store(true, Ordering::Relaxed);
                SESSION_LOCKED.store(true, Ordering::Relaxed);
                tracing::info!("session locked");
            }
            ext_session_lock_v1::Event::Finished => {
                session.shared.finished.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for Driver {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        // Acked before anything is drawn: committing a lock surface before its first ack is a protocol error,
        // and the driver's render pass is gated on `configured`, which this is what sets.
        proxy.ack_configure(serial);
        let Some(entry) = state
            .surfaces
            .iter_mut()
            .find(|entry| matches!(&entry.shell, Shell::Lock { lock, .. } if lock == proxy))
        else {
            return;
        };
        entry.apply_configure(width.max(1), height.max(1));
    }
}
