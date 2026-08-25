//! Whether the session is locked, and everything that decides it.
//!
//! One state, three writers and one performer. The **writers** are the session menu, `hogar-shell lock`, logind's `Lock`/`Unlock` signals and the idle timers — all of which do nothing but change [`LockState`]. The **performer** is [`on_state`], which runs on the driver thread and is the only place that takes or releases the compositor's session lock. Splitting them that way is what makes a lock requested from a keybind, from `loginctl`, and from a click the same lock rather than three racing attempts at one.
//!
//! Two things are checked *before* the screen is covered, never after: that the compositor implements `ext-session-lock-v1`, and that PAM can be loaded. A lock this process cannot undo is the one failure with no way out for the user, so it is refused with a message instead.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use platform_wayland::{EventSender, LockHandle};

use crate::pam::{self, AuthError};
use util::broadcast::Store;

/// How often the shell asks the compositor whether the lock it requested has actually been granted. A one-shot chain rather than a standing interval: it exists only while a lock does.
const CONFIRM_POLL: Duration = Duration::from_millis(250);

/// Which unlock method is running, so the screen can say what it is waiting for rather than just spinning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Password,
    Fingerprint,
    Face,
}

impl Method {
    pub fn message_key(self) -> &'static str {
        match self {
            Method::Password => "lock.checking",
            Method::Fingerprint => "lock.touch_sensor",
            Method::Face => "lock.looking",
        }
    }
}

/// What the lock screen shows and what the rest of the shell reads.
///
/// `wanted` and `locked` are deliberately separate: between asking the compositor and being granted, the desktop may still be on screen. Anything security-sensitive — suspending, reporting the session as locked over IPC — must wait for `locked`, which is the compositor's own word that nothing is visible.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LockState {
    /// The shell wants the session locked.
    pub wanted: bool,
    /// The compositor confirmed it.
    pub locked: bool,
    /// An authentication attempt is in flight; the field is inert until it lands.
    pub busy: Option<Method>,
    /// Consecutive failed attempts, reset by a success.
    pub failures: u32,
    /// The i18n key of the line under the password field, if any.
    pub message: Option<String>,
    /// While this is in the future, attempts are refused without troubling PAM.
    pub locked_out_until: Option<Instant>,
    /// Set when a lock was asked for and could not be taken, so the caller learns rather than waits.
    pub refused: Option<String>,
}

impl LockState {
    /// Whether the field should take a password right now.
    pub fn accepts_input(&self) -> bool {
        self.busy.is_none() && !self.is_locked_out()
    }

    pub fn is_locked_out(&self) -> bool {
        self.locked_out_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Seconds left on a lockout, for the countdown the screen shows.
    pub fn lockout_remaining(&self) -> u64 {
        self.locked_out_until
            .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0)
    }
}

static STATE: Store<LockState> = Store::new(LockState::default);

pub fn current() -> LockState {
    STATE.get()
}

/// Registers `tx` for lock-state changes. Every lock surface subscribes, and so does the driver-side performer.
pub fn subscribe(tx: EventSender<LockState>) {
    STATE.subscribe(tx);
}

/// Whether the session is locked *and the compositor has confirmed it* — what `hogar-shell lock status` reports and what a `lockstatus`-style indicator reads.
pub fn is_locked() -> bool {
    STATE.get().locked
}

/// Asks for the session to be locked. Idempotent; the performer does the work on the driver thread.
pub fn lock() {
    if STATE.get().wanted {
        return;
    }
    STATE.update(|state| {
        state.wanted = true;
        state.refused = None;
        state.failures = 0;
        state.message = None;
        state.locked_out_until = None;
    });
}

/// Asks for the session to be unlocked. Only reached after a successful authentication, or from `hogar-shell lock off` — which is a deliberate escape hatch for a shell that has locked a machine its user cannot authenticate to, and is exactly as privileged as the process already is.
pub fn unlock() {
    if !STATE.get().wanted {
        return;
    }
    STATE.update(|state| {
        state.wanted = false;
        state.busy = None;
        state.message = None;
    });
}

/// The reason a lock could not be taken, if the last attempt was refused.
pub fn refusal() -> Option<String> {
    STATE.get().refused
}

thread_local! {
    // The live lock, owned by the driver thread — the only thread that may take or release one.
    static HANDLE: RefCell<Option<LockHandle>> = const { RefCell::new(None) };
    // Whether a confirmation poll is already in flight, so a burst of state updates arms one chain, not ten.
    static POLLING: RefCell<bool> = const { RefCell::new(false) };
    // How to put the lock screen up. Installed at startup: what the locked screen *draws* is a surface, and a service has no business knowing one — it owns when the session is locked, not what that looks like.
    static SESSION: RefCell<Option<Box<dyn Fn() -> LockHandle>>> = const { RefCell::new(None) };
}

/// Registers how the lock screen is put up. Set once at startup by whoever owns the surface.
pub fn set_session_opener(open: impl Fn() -> LockHandle + 'static) {
    SESSION.with(|hook| *hook.borrow_mut() = Some(Box::new(open)));
}

fn open_session() -> Option<LockHandle> {
    SESSION.with(|hook| hook.borrow().as_ref().map(|open| open()))
}

/// Whether this machine can lock at all: the compositor implements the protocol and PAM will load. Read on the driver thread — `lock_supported` is answered by the driver's own view of the compositor's globals.
pub fn can_lock() -> Result<(), String> {
    if !platform_wayland::lock_supported() {
        return Err("this compositor does not implement ext-session-lock-v1".to_string());
    }
    let library = config::config()
        .map(|c| c.lock.pam_library.clone())
        .unwrap_or_default();
    if !pam::is_available(&library) {
        return Err(
            "libpam could not be loaded, so nothing could unlock the screen; set [lock] pam_library".to_string(),
        );
    }
    Ok(())
}

/// The performer: reconciles the compositor's lock with what [`LockState`] asks for. Registered once at startup with `platform_wayland::watch(lock::subscribe, lock::on_state)`, so it runs on the driver thread — the only one that may open a surface.
pub fn on_state(state: LockState) {
    let held = HANDLE.with(|handle| handle.borrow().is_some());
    match (state.wanted, held) {
        (true, false) => take(),
        (false, true) => release(),
        _ => {}
    }
}

fn take() {
    if let Err(reason) = can_lock() {
        tracing::error!("refusing to lock: {reason}");
        STATE.update(|state| {
            state.wanted = false;
            state.refused = Some(reason);
        });
        return;
    }
    let Some(handle) = open_session() else {
        tracing::error!("refusing to lock: no lock surface is installed");
        STATE.update(|state| state.wanted = false);
        return;
    };
    HANDLE.with(|slot| *slot.borrow_mut() = Some(handle));
    arm_confirmation_poll();
    crate::biometrics::start();
    crate::session::set_locked_hint(true);
}

fn release() {
    crate::biometrics::stop();
    crate::session::set_locked_hint(false);
    HANDLE.with(|slot| {
        if let Some(handle) = slot.borrow_mut().take() {
            handle.unlock();
        }
    });
    STATE.update(|state| {
        state.locked = false;
        state.failures = 0;
        state.locked_out_until = None;
    });
}

/// Follows the lock from "asked for" to "granted", and notices a compositor that refuses or takes it back.
///
/// A one-shot timer that re-arms itself while a lock is held rather than a standing interval: the driver's loop has no way to remove an app-level source once registered, so a permanent ticker would outlive every lock the session ever takes.
fn arm_confirmation_poll() {
    if POLLING.with(|polling| std::mem::replace(&mut *polling.borrow_mut(), true)) {
        return;
    }
    schedule_confirmation_poll();
}

fn schedule_confirmation_poll() {
    platform_wayland::timeout(CONFIRM_POLL, || {
        let verdict = HANDLE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|handle| (handle.is_locked(), handle.is_finished()))
        });
        let Some((locked, finished)) = verdict else {
            POLLING.with(|polling| *polling.borrow_mut() = false);
            return;
        };
        if finished {
            // The compositor ended the lock itself. The session is *not* locked, and saying otherwise would let a suspend go ahead behind an uncovered screen.
            tracing::warn!("the compositor ended the session lock");
            HANDLE.with(|slot| *slot.borrow_mut() = None);
            POLLING.with(|polling| *polling.borrow_mut() = false);
            crate::biometrics::stop();
            STATE.update(|state| {
                state.wanted = false;
                state.locked = false;
                state.refused = Some("the compositor ended the session lock".to_string());
            });
            return;
        }
        if locked != STATE.get().locked {
            STATE.update(|state| state.locked = locked);
        }
        schedule_confirmation_poll();
    });
}

/// Takes a password attempt. Returns immediately: PAM is run on a worker thread, because `pam_unix` sleeps for seconds after a wrong password and the lock screen must keep drawing while it does.
pub fn submit(password: String) {
    let state = STATE.get();
    if !state.wanted || !state.accepts_input() {
        return;
    }
    if password.is_empty() {
        STATE.update(|state| state.message = Some("lock.empty_password".to_string()));
        return;
    }
    let config = config::shared_config()
        .map(|c| c.lock.clone())
        .unwrap_or_default();
    let service = pam::service_name(&config.pam_service);
    let user = pam::current_user();
    STATE.update(|state| {
        state.busy = Some(Method::Password);
        state.message = None;
    });
    let _ = std::thread::Builder::new()
        .name("hogar-shell-pam".to_string())
        .spawn(move || {
            let verdict = pam::authenticate(&service, &user, &password, &config.pam_library);
            drop(password);
            match verdict {
                Ok(()) => succeed(Method::Password),
                Err(error) => fail(error, config.max_tries, config.lockout_seconds),
            }
        });
}

/// A successful unlock, whatever proved it. The one path out of the lock, so a fingerprint and a password leave exactly the same state behind.
pub fn succeed(method: Method) {
    tracing::info!("unlocked by {method:?}");
    STATE.update(|state| {
        state.wanted = false;
        state.busy = None;
        state.failures = 0;
        state.message = None;
        state.locked_out_until = None;
    });
}

/// A failed attempt: counts it, says why, and starts a lockout once the configured limit is reached.
pub fn fail(error: AuthError, max_tries: u32, lockout_seconds: u64) {
    let key = error.message_key().to_string();
    if let AuthError::Unavailable(detail) = &error {
        tracing::error!("authentication is unavailable: {detail}");
    }
    let next = STATE.update(|state| {
        state.busy = None;
        state.failures += 1;
        state.message = Some(key);
        // `max_tries = 0` never locks out — a machine whose owner would rather keep retrying than be shut out for thirty seconds every time they fumble a long passphrase.
        if max_tries > 0 && state.failures >= max_tries && lockout_seconds > 0 {
            state.locked_out_until = Some(Instant::now() + Duration::from_secs(lockout_seconds));
        }
    });
    if let Some(deadline) = next.locked_out_until {
        clear_lockout_when_elapsed(deadline);
    }
}

/// Publishes one more state change when the lockout expires, so the screen re-enables its field on its own rather than only when the user next presses a key it is ignoring.
fn clear_lockout_when_elapsed(deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let _ = std::thread::Builder::new()
        .name("hogar-shell-lockout".to_string())
        .spawn(move || {
            std::thread::sleep(remaining + Duration::from_millis(50));
            STATE.update(|state| {
                if !state.is_locked_out() {
                    state.locked_out_until = None;
                    state.failures = 0;
                    state.message = None;
                }
            });
        });
}

/// Marks a biometric attempt as running, so the screen says what it is waiting for and a password typed meanwhile is not thrown away by a competing attempt.
pub fn set_busy(method: Option<Method>) {
    if STATE.get().busy == method {
        return;
    }
    STATE.update(|state| state.busy = method);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wanting_a_lock_and_holding_one_are_different_questions() {
        // The window between the two is the whole reason they are separate fields: a suspend triggered on `wanted` would race the compositor's first covered frame.
        let asked = LockState {
            wanted: true,
            locked: false,
            ..LockState::default()
        };
        assert!(!asked.locked, "asking is not being locked");
        let granted = LockState {
            locked: true,
            ..asked.clone()
        };
        assert!(granted.locked);
    }

    #[test]
    fn a_lockout_refuses_input_until_it_elapses() {
        let mut state = LockState {
            wanted: true,
            ..LockState::default()
        };
        assert!(state.accepts_input(), "an idle field takes a password");

        state.busy = Some(Method::Password);
        assert!(
            !state.accepts_input(),
            "a check in flight is not a second prompt"
        );

        state.busy = None;
        state.locked_out_until = Some(Instant::now() + Duration::from_secs(30));
        assert!(state.is_locked_out());
        assert!(!state.accepts_input());
        assert!(state.lockout_remaining() > 25);

        state.locked_out_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!state.is_locked_out(), "an elapsed lockout is over");
        assert_eq!(state.lockout_remaining(), 0);
        assert!(state.accepts_input());
    }

    #[test]
    fn every_method_says_what_it_is_waiting_for() {
        for method in [Method::Password, Method::Fingerprint, Method::Face] {
            assert!(method.message_key().starts_with("lock."));
        }
        assert_ne!(
            Method::Fingerprint.message_key(),
            Method::Face.message_key(),
            "a sensor to touch and a camera to face are different instructions"
        );
    }
}
