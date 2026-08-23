//! Unlocking without typing: a fingerprint through fprintd, and a face through Howdy.
//!
//! Both are *alternatives* to the password, never replacements. They run alongside the field, they stop the
//! moment the screen unlocks, and each has its own attempt budget — after which the shell stops asking and
//! leaves the password as the only way in. That ordering is the whole safety argument: a biometric that keeps
//! retrying forever is a sensor an attacker can keep feeding.
//!
//! Neither is required. fprintd is a D-Bus service that simply is not on the bus without a reader; Howdy is a
//! command that is not installed. In both cases the lock screen behaves as though the feature were switched
//! off, which is what `[lock] fingerprint` / `howdy_command` also do explicitly.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;

use crate::lock::{self, Method};
use config::LockConfig;

const FPRINT: &str = "net.reactivated.Fprint";
const MANAGER_PATH: &str = "/net/reactivated/Fprint/Manager";
const MANAGER_IFACE: &str = "net.reactivated.Fprint.Manager";
const DEVICE_IFACE: &str = "net.reactivated.Fprint.Device";

/// Claiming a reader talks to hardware and to polkit; a wedged one must not park the attempt thread forever.
const METHOD_TIMEOUT: Duration = Duration::from_secs(10);

/// Which run of the lock screen an attempt belongs to. A verification can be parked inside fprintd when the
/// screen unlocks, and its late result must not unlock the *next* lock — so every attempt carries the
/// generation it started in and a stale one is discarded.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts whatever biometric methods the config enables for this lock. Called once as the screen comes up.
pub fn start() {
    let config = config::shared_config()
        .map(|c| c.lock.clone())
        .unwrap_or_default();
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    RUNNING.store(true, Ordering::Relaxed);
    if config.fingerprint && config.max_fprint_tries > 0 {
        spawn("hogar-shell-fprint", move || {
            run_fingerprint(generation, config.max_fprint_tries)
        });
    }
    let howdy = config.howdy_command.trim().to_string();
    if config.trigger_on_wake && !howdy.is_empty() && config.max_howdy_tries > 0 {
        spawn("hogar-shell-howdy", move || {
            run_face(generation, &howdy, config.max_howdy_tries)
        });
    }
}

/// Ends every attempt in flight. A verification already inside fprintd cannot be interrupted from here, so the
/// generation is bumped instead: whatever it eventually answers is answered for a lock that is over.
pub fn stop() {
    RUNNING.store(false, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Runs face unlock once, on demand — the lock screen's "try again" for a machine that did not want it
/// triggered automatically.
pub fn retry_face() {
    let config = config::shared_config()
        .map(|c| c.lock.clone())
        .unwrap_or_default();
    let howdy = config.howdy_command.trim().to_string();
    if howdy.is_empty() {
        return;
    }
    let generation = GENERATION.load(Ordering::Relaxed);
    spawn("hogar-shell-howdy", move || run_face(generation, &howdy, 1));
}

/// Whether either method is configured at all, so the screen can offer them rather than showing a control
/// that does nothing.
pub fn offered(config: &LockConfig) -> (bool, bool) {
    (
        config.fingerprint && config.max_fprint_tries > 0,
        !config.howdy_command.trim().is_empty() && config.max_howdy_tries > 0,
    )
}

fn spawn(name: &str, task: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(task);
}

/// Whether this attempt still belongs to the lock that is on screen.
fn current(generation: u64) -> bool {
    RUNNING.load(Ordering::Relaxed) && GENERATION.load(Ordering::Relaxed) == generation
}

fn connection() -> Option<Connection> {
    crate::bus::system(Some(METHOD_TIMEOUT))
}

/// The reader fprintd would use, or `None` on a machine with none — which is the ordinary case and not worth a
/// warning on every lock.
fn default_device(conn: &Connection) -> Option<zbus::zvariant::OwnedObjectPath> {
    conn.call_method(
        Some(FPRINT),
        MANAGER_PATH,
        Some(MANAGER_IFACE),
        "GetDefaultDevice",
        &(),
    )
    .ok()?
    .body()
    .deserialize()
    .ok()
}

/// One reader, up to `max_tries` fingers.
///
/// fprintd's verification is a claim, a start, a signal, and a stop — and the claim is exclusive, so it is
/// released on every path out. A reader left claimed by a shell that unlocked is one no login screen can use
/// afterwards.
fn run_fingerprint(generation: u64, max_tries: u32) {
    let Some(conn) = connection() else {
        return;
    };
    let Some(device) = default_device(&conn) else {
        tracing::debug!("no fingerprint reader; skipping fingerprint unlock");
        return;
    };
    let path = device.as_str().to_string();
    // An empty user name means "whoever is logged in", which is the right question for a lock screen.
    if let Err(e) = conn.call_method(
        Some(FPRINT),
        path.as_str(),
        Some(DEVICE_IFACE),
        "Claim",
        &("",),
    ) {
        tracing::debug!("fprintd: cannot claim the reader: {e}");
        return;
    }

    for attempt in 1..=max_tries {
        if !current(generation) {
            break;
        }
        lock::set_busy(Some(Method::Fingerprint));
        match verify_once(&conn, &path, generation) {
            Some(true) => {
                if current(generation) {
                    lock::succeed(Method::Fingerprint);
                }
                break;
            }
            Some(false) => {
                tracing::debug!("fprintd: no match ({attempt}/{max_tries})");
                if attempt == max_tries {
                    lock::set_busy(None);
                }
            }
            None => break,
        }
    }
    lock::set_busy(None);
    let _ = conn.call_method(
        Some(FPRINT),
        path.as_str(),
        Some(DEVICE_IFACE),
        "Release",
        &(),
    );
}

/// Starts one verification and parks on `VerifyStatus` until it resolves. `Some(true)` is a match, `Some(false)`
/// a rejected finger worth retrying, `None` a reader that stopped answering.
fn verify_once(conn: &Connection, path: &str, generation: u64) -> Option<bool> {
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(FPRINT)
        .ok()?
        .interface(DEVICE_IFACE)
        .ok()?
        .member("VerifyStatus")
        .ok()?
        .path(path.to_string())
        .ok()?
        .build();
    // Subscribed before the verification starts, so a reader fast enough to answer immediately is not missed.
    let signals = MessageIterator::for_match_rule(rule, conn, None).ok()?;
    conn.call_method(
        Some(FPRINT),
        path,
        Some(DEVICE_IFACE),
        "VerifyStart",
        &("any",),
    )
    .ok()?;

    let mut verdict = None;
    for message in signals {
        let Ok(message) = message else { continue };
        let Ok((result, done)) = message.body().deserialize::<(String, bool)>() else {
            continue;
        };
        if result == "verify-match" {
            verdict = Some(true);
        } else if done || result.starts_with("verify-") {
            verdict = Some(false);
        }
        if done || verdict == Some(true) || !current(generation) {
            break;
        }
    }
    let _ = conn.call_method(Some(FPRINT), path, Some(DEVICE_IFACE), "VerifyStop", &());
    verdict
}

/// Howdy, run as the command the config names with the user appended — the same contract Howdy's own PAM
/// module uses: exit status 0 is a match, anything else is not.
///
/// A subprocess rather than a library because Howdy has no stable one, and off the UI thread with a bound on
/// how long it may look: a camera that never resolves must not leave the screen saying "looking" forever.
fn run_face(generation: u64, command: &str, max_tries: u32) {
    let user = crate::pam::current_user();
    for attempt in 1..=max_tries {
        if !current(generation) {
            return;
        }
        lock::set_busy(Some(Method::Face));
        match run_once(command, &user) {
            Some(true) => {
                if current(generation) {
                    lock::succeed(Method::Face);
                }
                return;
            }
            Some(false) => tracing::debug!("howdy: no match ({attempt}/{max_tries})"),
            None => {
                tracing::debug!("howdy: '{command}' could not be run; face unlock is off");
                break;
            }
        }
    }
    lock::set_busy(None);
}

/// Runs the configured command once. `None` means it could not be started at all — an uninstalled Howdy, which
/// is a reason to stop trying rather than to count a failed attempt.
fn run_once(command: &str, user: &str) -> Option<bool> {
    let mut words = command.split_whitespace();
    let program = words.next()?;
    let status = util::process::command(program)
        .args(words)
        .arg(user)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    Some(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_with_no_budget_is_never_offered() {
        let none = LockConfig {
            fingerprint: false,
            howdy_command: String::new(),
            ..LockConfig::default()
        };
        assert_eq!(offered(&none), (false, false));

        let both = LockConfig {
            fingerprint: true,
            max_fprint_tries: 3,
            howdy_command: "howdy compare".to_string(),
            max_howdy_tries: 2,
            ..LockConfig::default()
        };
        assert_eq!(offered(&both), (true, true));

        // A budget of zero is off, not unlimited — otherwise `max_*_tries = 0` would be the most permissive
        // setting in the file rather than the most restrictive.
        let budgetless = LockConfig {
            fingerprint: true,
            max_fprint_tries: 0,
            howdy_command: "howdy compare".to_string(),
            max_howdy_tries: 0,
            ..LockConfig::default()
        };
        assert_eq!(offered(&budgetless), (false, false));
    }

    #[test]
    fn an_attempt_from_a_previous_lock_is_never_honoured() {
        // The case this guards: a finger presented as the screen unlocks, answered by fprintd afterwards. If
        // generation were not checked, that answer would unlock whatever lock came next.
        stop();
        let stale = GENERATION.load(Ordering::Relaxed);
        assert!(!current(stale), "nothing is current once the lock is over");

        RUNNING.store(true, Ordering::Relaxed);
        let live = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        assert!(current(live));
        assert!(!current(stale), "the older attempt is still stale");
        stop();
    }

    /// The command is never run here: a suite that executed `howdy_command` would be pointing the machine's
    /// camera at whoever ran `cargo test`.
    #[test]
    fn the_face_command_is_parsed_without_being_run() {
        assert!("".split_whitespace().next().is_none());
        let mut words = "howdy compare --extra".split_whitespace();
        assert_eq!(words.next(), Some("howdy"));
        assert_eq!(words.collect::<Vec<_>>(), vec!["compare", "--extra"]);
    }
}
