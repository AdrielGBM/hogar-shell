//! Ending the session: lock, log out, reboot, power off, suspend, hibernate.
//!
//! Every action goes through logind rather than through `systemctl`, for two reasons. It works without
//! privileges — logind decides what the active session's user is allowed to do — and it can be *asked* first:
//! `CanHibernate` tells the shell whether to offer hibernate at all, so the session menu greys out what this
//! machine cannot do instead of offering a button that fails.

use std::time::{Duration, Instant};

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";

/// What a session menu can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Lock,
    Logout,
    Suspend,
    Hibernate,
    Reboot,
    Shutdown,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Lock,
        Action::Logout,
        Action::Suspend,
        Action::Hibernate,
        Action::Reboot,
        Action::Shutdown,
    ];

    /// The stable id used in config, IPC and the i18n catalogs.
    pub fn id(self) -> &'static str {
        match self {
            Action::Lock => "lock",
            Action::Logout => "logout",
            Action::Suspend => "suspend",
            Action::Hibernate => "hibernate",
            Action::Reboot => "reboot",
            Action::Shutdown => "shutdown",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The default Iconify glyph; a config may override it per action.
    pub fn icon(self) -> &'static str {
        match self {
            Action::Lock => "lock",
            Action::Logout => "log-out",
            Action::Suspend => "moon",
            Action::Hibernate => "snowflake",
            Action::Reboot => "rotate-ccw",
            Action::Shutdown => "power",
        }
    }

    /// The logind manager method, and the `Can…` property that says whether it is available. `Lock` and
    /// `Logout` act on the session object instead, so they have no manager method here.
    fn manager_method(self) -> Option<(&'static str, &'static str)> {
        match self {
            Action::Suspend => Some(("Suspend", "CanSuspend")),
            Action::Hibernate => Some(("Hibernate", "CanHibernate")),
            Action::Reboot => Some(("Reboot", "CanReboot")),
            Action::Shutdown => Some(("PowerOff", "CanPowerOff")),
            Action::Lock | Action::Logout => None,
        }
    }
}

fn connection() -> Option<Connection> {
    crate::bus::system(None)
}

/// Whether logind will let this session perform `action`. The `Can…` methods answer `"yes"`, `"no"`,
/// `"challenge"` (would prompt for authentication) or `"na"` (unsupported by the machine — no swap for
/// hibernate, say). `"challenge"` counts as available: the prompt is the polkit agent's job, not the shell's.
pub fn is_available(action: Action) -> bool {
    let Some((_, probe)) = action.manager_method() else {
        return true; // lock and logout always apply to one's own session
    };
    let Some(conn) = connection() else {
        return false;
    };
    let reply: Result<String, _> = conn
        .call_method(Some(LOGIN1), MANAGER_PATH, Some(MANAGER_IFACE), probe, &())
        .and_then(|m| m.body().deserialize());
    matches!(reply.as_deref(), Ok("yes" | "challenge"))
}

/// Every action this machine can actually perform, in menu order.
pub fn available() -> Vec<Action> {
    Action::ALL
        .into_iter()
        .filter(|a| is_available(*a))
        .collect()
}

/// Performs `action`. Runs off the UI thread — a D-Bus round-trip that suspends the machine must not happen
/// inside a click handler — and reports failures rather than returning them, since a caller has nothing useful
/// to do about a refused power action beyond what logind already told the user.
pub fn perform(action: Action) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-session".to_string())
        .spawn(move || {
            if let Err(e) = call(action) {
                tracing::warn!("session action '{}' failed: {e}", action.id());
            }
        });
}

fn call(action: Action) -> Result<(), zbus::Error> {
    let conn = connection().ok_or(zbus::Error::InvalidField)?;
    match action {
        // `Lock` asks the session's lock handler (us) to lock; it does not lock by itself. Emitting it rather
        // than locking directly keeps one code path whether the request came from here or from `loginctl`.
        Action::Lock => {
            conn.call_method(Some(LOGIN1), SESSION_PATH, Some(SESSION_IFACE), "Lock", &())?;
        }
        Action::Logout => {
            conn.call_method(
                Some(LOGIN1),
                SESSION_PATH,
                Some(SESSION_IFACE),
                "Terminate",
                &(),
            )?;
        }
        other => {
            let (method, _) = other
                .manager_method()
                .expect("non-session actions have one");
            // `false` = don't ask the policy layer to prompt; the caller already confirmed in the menu.
            conn.call_method(
                Some(LOGIN1),
                MANAGER_PATH,
                Some(MANAGER_IFACE),
                method,
                &(false,),
            )?;
        }
    }
    Ok(())
}

/// Tells logind whether this session is locked, so `loginctl session-status` — and anything else that asks it
/// rather than asking the shell — agrees with what is on screen.
///
/// Off the UI thread, like every other logind call here: a hint is not worth a frame.
pub fn set_locked_hint(locked: bool) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-locked-hint".to_string())
        .spawn(move || {
            let Some(conn) = connection() else { return };
            if let Err(e) = conn.call_method(
                Some(LOGIN1),
                SESSION_PATH,
                Some(SESSION_IFACE),
                "SetLockedHint",
                &(locked,),
            ) {
                tracing::debug!("logind: SetLockedHint({locked}): {e}");
            }
        });
}

/// How long the shell may hold the machine awake while it puts the lock screen up. Long enough for a
/// compositor to grant a lock and paint one covered frame, short enough that a shell which cannot lock delays
/// the lid closing by a moment rather than by a minute.
const SLEEP_GRACE: Duration = Duration::from_secs(5);

/// What logind tells the shell about its session. Delivered to the driver thread, which is the only one that
/// may act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// `loginctl lock-session`, or anything else that asked this session to lock.
    Lock,
    Unlock,
    /// The machine came back from suspend.
    Resumed,
}

/// Parks on logind's session signals and reports them to the driver thread — and, while it is there, holds the
/// sleep inhibitor that makes `lock_before_sleep` mean anything.
///
/// The inhibitor is the whole reason this is a thread rather than a subscription. logind announces a suspend
/// with `PrepareForSleep(true)` and then waits only for the *delay* inhibitors clients hold; without one, the
/// machine is asleep before the compositor has drawn a single covered frame, and the desktop is briefly on
/// screen when it wakes. So the fd is taken up front, released once the lock is confirmed, and taken again on
/// the way back — which has to happen on whichever thread owns it.
pub fn watch(tx: EventSender<Event>) {
    let Some(conn) = connection() else {
        tracing::info!("no system bus; `loginctl lock-session` will not reach the shell");
        return;
    };
    let Some(signals) = session_signals(&conn) else {
        tracing::warn!("logind: cannot watch the session's Lock/Unlock signals");
        return;
    };
    let mut inhibitor = take_sleep_inhibitor(&conn);

    for message in signals {
        let Ok(message) = message else { continue };
        let member = message.header().member().map(|m| m.to_string());
        match member.as_deref() {
            Some("Lock") => {
                if !tx.send(Event::Lock) {
                    return;
                }
            }
            Some("Unlock") => {
                if !tx.send(Event::Unlock) {
                    return;
                }
            }
            Some("PrepareForSleep") => {
                let Ok(sleeping) = message.body().deserialize::<bool>() else {
                    continue;
                };
                if sleeping {
                    if !tx.send(Event::Lock) {
                        return;
                    }
                    wait_until_locked();
                    // Only now: dropping the fd is what tells logind it may suspend.
                    inhibitor = None;
                } else {
                    inhibitor = take_sleep_inhibitor(&conn);
                    if !tx.send(Event::Resumed) {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
    drop(inhibitor);
}

/// Blocks until the compositor confirms the lock, or the grace runs out. `wanted` is not enough here — the
/// point of the delay is that the screen is *covered* before the machine sleeps.
fn wait_until_locked() {
    if !config::shared_config()
        .map(|c| c.lock.lock_before_sleep)
        .unwrap_or(true)
    {
        return;
    }
    let deadline = Instant::now() + SLEEP_GRACE;
    while Instant::now() < deadline {
        // The compositor's own flag, not the shell's polled copy of it ([`crate::lock::is_locked`]). That copy
        // is refreshed by a timer on the driver's loop, so a driver with work queued ahead of it could not tell
        // this thread the screen was already covered — and a wait that cannot observe success gives up and
        // sleeps the machine anyway. It happened: a critical-battery suspend while the driver was busy, and the
        // laptop slept with the session open.
        if platform_wayland::session_is_locked() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    tracing::error!(
        "the session did not lock within {SLEEP_GRACE:?}; suspending with the screen uncovered"
    );
}

/// One iterator over every signal this shell cares about. A match rule cannot express "either of these", so it
/// is broadened to logind's own signals and narrowed by member here — logind emits few enough that the cost is
/// nothing, unlike the same trick on the session bus.
fn session_signals(conn: &Connection) -> Option<MessageIterator> {
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(LOGIN1)
        .ok()?
        .build();
    MessageIterator::for_match_rule(rule, conn, None).ok()
}

/// Takes a `delay` sleep inhibitor. `delay` rather than `block`: the shell is asking for a moment to cover the
/// screen, not for a veto over suspending — a veto is the kind of thing that leaves a laptop cooking in a bag.
fn take_sleep_inhibitor(conn: &Connection) -> Option<zbus::zvariant::OwnedFd> {
    let reply = conn
        .call_method(
            Some(LOGIN1),
            MANAGER_PATH,
            Some(MANAGER_IFACE),
            "Inhibit",
            &(
                "sleep",
                "hogar-shell",
                "Locking the session before sleep",
                "delay",
            ),
        )
        .inspect_err(|e| tracing::warn!("logind: cannot take a sleep inhibitor: {e}"))
        .ok()?;
    reply.body().deserialize().ok()
}

/// Runs one logind session event on the driver thread. Everything it does goes through the lock service, so a
/// `loginctl lock-session` and a click on the session menu's Lock are the same lock.
pub fn on_event(event: Event) {
    match event {
        Event::Lock => crate::lock::lock(),
        Event::Unlock => crate::lock::unlock(),
        // A machine coming back from suspend is exactly when a face-unlock user wants the camera to try, and
        // the lock screen has been up since before it slept, so nothing else would trigger it.
        Event::Resumed => {
            let trigger = config::config()
                .map(|c| c.lock.trigger_on_wake)
                .unwrap_or(false);
            if trigger && crate::lock::current().wanted {
                crate::biometrics::retry_face();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_cover_every_action() {
        for action in Action::ALL {
            assert_eq!(
                Action::from_id(action.id()),
                Some(action),
                "'{}' must parse back",
                action.id()
            );
            assert!(!action.icon().is_empty());
        }
        assert_eq!(Action::from_id("explode"), None);
    }

    #[test]
    fn lock_and_logout_act_on_the_session_not_the_manager() {
        assert_eq!(Action::Lock.manager_method(), None);
        assert_eq!(Action::Logout.manager_method(), None);
        assert_eq!(
            Action::Shutdown.manager_method(),
            Some(("PowerOff", "CanPowerOff"))
        );
    }

    #[test]
    fn actions_on_ones_own_session_are_always_offered() {
        // These need no capability probe, so they must not be filtered out on a machine with no system bus.
        assert!(is_available(Action::Lock));
        assert!(is_available(Action::Logout));
    }
}
