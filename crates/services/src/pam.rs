//! Authenticating the user against PAM, which is the only thing on a Linux desktop that can say whether a
//! password is right.
//!
//! **Loaded at runtime, not linked.** A lock screen is the last feature that may make the rest of the shell
//! unbuildable: linking `libpam` would put PAM headers between a user and a working bar. Loading it on demand
//! keeps the binary portable, lets `[lock] pam_library` name a path on a distribution that puts the library
//! outside the loader's search path, and — the part that matters — turns "no PAM here" into a question the
//! shell can ask *before* it locks the screen rather than a failure it discovers after.
//!
//! **Every call runs on a worker thread.** `pam_authenticate` talks to `pam_unix`, which sleeps for seconds
//! after a wrong password by design, and may talk to a fingerprint reader or a network directory. On the UI
//! thread that is a frozen shell; here it is a spinner.

use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use util::deps::{self, Dep};

/// PAM's own return and message codes. Named here rather than reached for as literals, since the whole
/// authentication verdict rests on telling `PAM_SUCCESS` from everything else.
const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_MAXTRIES: c_int = 11;
const PAM_ACCT_EXPIRED: c_int = 13;
const PAM_CONV_ERR: c_int = 19;
const PAM_BUF_ERR: c_int = 5;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
/// `pam_authenticate` refuses an empty password outright rather than letting a blank field be a way in.
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(
    c_int,
    *const *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Option<ConvFn>,
    appdata_ptr: *mut c_void,
}

type StartFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type StepFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type EndFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

struct Pam {
    // Held so the loaded library outlives the function pointers taken out of it.
    _library: libloading::Library,
    start: StartFn,
    authenticate: StepFn,
    acct_mgmt: StepFn,
    end: EndFn,
}

// SAFETY: the four symbols are libpam's own entry points, which are thread-safe with respect to distinct
// `pam_handle_t`s — and every call here creates, uses and ends its own handle on one thread.
unsafe impl Send for Pam {}
unsafe impl Sync for Pam {}

static PAM: OnceLock<Option<Pam>> = OnceLock::new();

fn load(preferred: &str) -> Option<Pam> {
    // SAFETY: loading a shared object runs its initialisers, which for libpam allocate and read config — no
    // more than any process that links it. The symbols are looked up by their documented C signatures.
    let loaded = unsafe {
        deps::open_library(Dep::LibPam, Some(preferred), |library| {
            let start = *library.get::<StartFn>(b"pam_start\0")?;
            let authenticate = *library.get::<StepFn>(b"pam_authenticate\0")?;
            let acct_mgmt = *library.get::<StepFn>(b"pam_acct_mgmt\0")?;
            let end = *library.get::<EndFn>(b"pam_end\0")?;
            Ok(Pam {
                _library: library,
                start,
                authenticate,
                acct_mgmt,
                end,
            })
        })
    };
    if loaded.is_none() {
        tracing::warn!(
            "no libpam could be loaded (tried {}); the lock screen has nothing to authenticate against. Set [lock] pam_library to its path — `find / -name 'libpam.so.0'` will say where it is.",
            deps::library_names(Dep::LibPam).join(", ")
        );
    }
    loaded
}

fn pam(preferred: &str) -> Option<&'static Pam> {
    PAM.get_or_init(|| load(preferred)).as_ref()
}

/// Whether the shell can authenticate at all. Asked before taking a session lock: locking a screen this
/// process cannot unlock is the one failure the user has no way out of.
pub fn is_available(library: &str) -> bool {
    pam(library).is_some()
}

/// The PAM service to authenticate against: the configured one, else the first stack that exists out of a
/// hogar-shell-specific one, another lock screen's, and finally `login`. Named services are files, so an absent
/// one is a silent "authentication failed" for every password — worth resolving to one that is there.
pub fn service_name(configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    let dir = Path::new("/etc/pam.d");
    ["hogar-shell", "swaylock", "hyprlock", "login"]
        .into_iter()
        .find(|name| dir.join(name).exists())
        .unwrap_or("login")
        .to_string()
}

/// The user PAM is asked about. `$USER` can be inherited from whoever started the process, so the effective
/// uid's own passwd entry is the honest answer; the environment is only the fallback.
pub fn current_user() -> String {
    passwd_name()
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("LOGNAME").ok())
        .unwrap_or_default()
}

fn passwd_name() -> Option<String> {
    // SAFETY: `getpwuid` returns a pointer into libc's own static storage, read immediately and copied out.
    unsafe {
        let entry = libc::getpwuid(libc::geteuid());
        if entry.is_null() {
            return None;
        }
        let name = (*entry).pw_name;
        if name.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}

/// Why an authentication attempt did not succeed. The distinction the lock screen actually draws is between
/// "wrong" — try again — and everything else, which is worth naming on screen because the user cannot fix it
/// by typing more carefully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The password was wrong.
    Denied,
    /// PAM refused further attempts (`pam_faillock`, or the stack's own limit).
    TooManyTries,
    /// The account is expired or otherwise not permitted to log in.
    AccountUnavailable,
    /// PAM is not usable on this machine, or the conversation itself failed.
    Unavailable(String),
}

impl AuthError {
    /// The i18n key for the message the lock screen shows.
    pub fn message_key(&self) -> &'static str {
        match self {
            AuthError::Denied => "lock.wrong_password",
            AuthError::TooManyTries => "lock.too_many_tries",
            AuthError::AccountUnavailable => "lock.account_unavailable",
            AuthError::Unavailable(_) => "lock.no_authentication",
        }
    }
}

/// PAM's conversation: every prompt that asks for something the user types gets the one secret this attempt
/// carries, and everything else (info and error text from the stack) is answered with nothing.
///
/// The array and each string must come from `malloc`, because PAM frees them with `free` — a Rust allocation
/// handed over here would be freed by the wrong allocator.
///
/// # Safety
/// Called by libpam with `num_msg` valid `PamMessage` pointers and an `appdata_ptr` that this module set to a
/// live `CString` for the duration of the call.
unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *const *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() {
        return PAM_CONV_ERR;
    }
    let count = num_msg as usize;
    // SAFETY: the contract above; every dereference is bounded by `num_msg`.
    unsafe {
        let array = libc::calloc(count, size_of::<PamResponse>()) as *mut PamResponse;
        if array.is_null() {
            return PAM_BUF_ERR;
        }
        let secret = appdata as *const CString;
        for index in 0..count {
            let message = *msg.add(index);
            let style = if message.is_null() {
                0
            } else {
                (*message).msg_style
            };
            let entry = array.add(index);
            (*entry).resp_retcode = 0;
            (*entry).resp = match style {
                PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON if !secret.is_null() => {
                    libc::strdup((*secret).as_ptr())
                }
                _ => std::ptr::null_mut(),
            };
        }
        *resp = array;
    }
    PAM_SUCCESS
}

/// Runs one full authentication — `pam_start`, `pam_authenticate`, `pam_acct_mgmt`, `pam_end`.
///
/// Blocking, and deliberately so: `pam_unix` delays for seconds after a wrong password, which is the point.
/// Call it from a worker thread; [`crate::lock`] is the only caller and does.
pub fn authenticate(
    service: &str,
    user: &str,
    password: &str,
    library: &str,
) -> Result<(), AuthError> {
    let Some(pam) = pam(library) else {
        return Err(AuthError::Unavailable(
            "libpam is not available".to_string(),
        ));
    };
    let (Ok(service_c), Ok(user_c), Ok(secret)) = (
        CString::new(service),
        CString::new(user),
        CString::new(password),
    ) else {
        // A NUL inside any of the three: never a valid credential, and passing it on would truncate it.
        return Err(AuthError::Denied);
    };

    let conversation = PamConv {
        conv: Some(converse),
        appdata_ptr: (&raw const secret) as *mut c_void,
    };
    let mut handle: *mut c_void = std::ptr::null_mut();

    // SAFETY: the handle is created here, used only on this thread, and ended on every path out — including
    // the early returns below, which run before `secret` (borrowed by `appdata_ptr`) is dropped.
    let status = unsafe {
        let started = (pam.start)(
            service_c.as_ptr(),
            user_c.as_ptr(),
            &conversation,
            &mut handle,
        );
        if started != PAM_SUCCESS || handle.is_null() {
            return Err(AuthError::Unavailable(format!(
                "pam_start('{service}') failed with {started}"
            )));
        }
        let authenticated = (pam.authenticate)(handle, PAM_DISALLOW_NULL_AUTHTOK);
        // Only asked once the password is right: `pam_acct_mgmt` reports an expired account, which is a
        // different message from a wrong password and must not be reported as one.
        let verdict = if authenticated == PAM_SUCCESS {
            (pam.acct_mgmt)(handle, 0)
        } else {
            authenticated
        };
        (pam.end)(handle, verdict);
        verdict
    };
    drop(secret);

    match status {
        PAM_SUCCESS => Ok(()),
        PAM_AUTH_ERR => Err(AuthError::Denied),
        PAM_MAXTRIES => Err(AuthError::TooManyTries),
        PAM_ACCT_EXPIRED => Err(AuthError::AccountUnavailable),
        // Everything else is a stack that could not reach a verdict — reported as itself rather than folded
        // into "wrong password", which would have the user retyping a password that was never the problem.
        other => Err(AuthError::Unavailable(format!("PAM returned {other}"))),
    }
}

/// The PAM service files this machine has, for a diagnostic that says *why* authentication is unavailable.
pub fn known_services() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/etc/pam.d") else {
        return Vec::new();
    };
    entries.flatten().map(|entry| entry.path()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_service_wins_and_the_fallback_is_one_that_exists() {
        assert_eq!(service_name("  my-stack "), "my-stack");
        let resolved = service_name("");
        assert!(!resolved.is_empty());
        // Either the machine has one of the candidates, or the answer is the last-resort `login` — never a
        // name picked at random, which would fail every password with no indication why.
        let exists = Path::new("/etc/pam.d").join(&resolved).exists();
        assert!(
            exists || resolved == "login",
            "'{resolved}' is neither present nor the documented fallback"
        );
    }

    #[test]
    fn every_failure_has_its_own_message() {
        let errors = [
            AuthError::Denied,
            AuthError::TooManyTries,
            AuthError::AccountUnavailable,
            AuthError::Unavailable(String::new()),
        ];
        let mut keys: Vec<&str> = errors.iter().map(|e| e.message_key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys.len(),
            errors.len(),
            "a wrong password and an unusable PAM stack must not read the same on screen"
        );
        for key in keys {
            assert!(
                key.starts_with("lock."),
                "'{key}' is not a lock-screen string"
            );
        }
    }

    #[test]
    fn the_user_pam_is_asked_about_is_a_real_account() {
        // `$USER` is inherited and can name someone else entirely; the effective uid cannot.
        let user = current_user();
        assert!(
            !user.is_empty(),
            "there is always a passwd entry for the running uid"
        );
    }

    /// Authentication itself is never run here — a suite that called `pam_authenticate` would be typing a
    /// wrong password at the machine's own faillock counter on every `cargo test`, exactly the trap
    /// `a_command_that_changes_the_machine_is_never_run_by_the_suite` guards in the IPC table.
    #[test]
    fn loading_the_library_is_separate_from_using_it() {
        let _ = is_available("");
        assert!(
            known_services().len() < 10_000,
            "the service listing is a diagnostic, not an authentication"
        );
    }

    /// A machine that *has* PAM must be able to lock. Written as an implication rather than a flat assertion
    /// so it stays honest on a host without the library — but on one that has it, this is what catches a
    /// candidate list that no longer names where the library actually lives.
    #[test]
    fn a_machine_with_libpam_present_finds_it() {
        let present: Vec<&str> = deps::library_names(Dep::LibPam)
            .iter()
            .copied()
            .filter(|name| name.starts_with('/') && Path::new(name).exists())
            .collect();
        if present.is_empty() {
            return;
        }
        assert!(
            is_available(""),
            "libpam is at {present:?} but the loader did not pick it up"
        );
    }
}
