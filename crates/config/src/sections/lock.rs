//! `[lock]` and the `[idle]` stages that lead to it.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use serde::{Deserialize, Serialize};

/// The lock screen (`[lock]`): what it authenticates against, and what it shows while it waits.
///
/// The screen only comes up on a compositor that implements `ext-session-lock-v1` and with a PAM library the shell can load. Both are checked *before* the lock is taken, because the failure mode of finding out afterwards is a user staring at a screen with no way back in.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct LockConfig {
    /// The PAM service to authenticate against — a file under `/etc/pam.d`. Empty picks the first of `hogar-shell`, `swaylock`, `login` that exists, so a machine with no hogar-shell-specific stack still unlocks instead of refusing every password.
    pub pam_service: String,
    /// Where `libpam` is. Empty tries `libpam.so.0`, `libpam.so` and NixOS's `/run/current-system/sw/lib/libpam.so.0`, which between them cover every machine met so far; set it only if `hogar-shell lock status` says the library could not be loaded.
    pub pam_library: String,
    /// Attempts before the field locks itself out for `lockout_seconds`. `0` never locks out.
    pub max_tries: u32,
    pub lockout_seconds: u64,
    /// Verify a fingerprint through fprintd alongside the password, when a reader is enrolled.
    pub fingerprint: bool,
    pub max_fprint_tries: u32,
    /// The Howdy face-unlock command, run with the user name appended; empty disables it. Exit status 0 is a successful match, as Howdy's own PAM module treats it.
    pub howdy_command: String,
    pub max_howdy_tries: u32,
    /// Attempt face unlock as soon as the lock screen appears, rather than only when asked.
    pub trigger_on_wake: bool,
    /// Lock before the machine suspends, so the screen is already covered when it wakes.
    pub lock_before_sleep: bool,
    pub show_avatar: bool,
    pub show_media: bool,
    pub show_weather: bool,
    pub show_resources: bool,
    pub show_notifications: bool,
    /// Start with the notification dock collapsed — the lock screen is the one surface where a stranger can read what arrived without unlocking.
    pub hide_notifs: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            pam_service: String::new(),
            pam_library: String::new(),
            max_tries: 5,
            lockout_seconds: 30,
            fingerprint: false,
            max_fprint_tries: 3,
            howdy_command: String::new(),
            max_howdy_tries: 3,
            trigger_on_wake: false,
            lock_before_sleep: true,
            show_avatar: true,
            show_media: true,
            show_weather: false,
            show_resources: false,
            show_notifications: true,
            // Bodies hidden by default: the count and the app are enough to know something arrived.
            hide_notifs: true,
        }
    }
}

/// One idle timeout, declared as an `[[idle.stages]]` table: what to run once the seat has been idle that long, and what to run when it stops being.
///
/// Both actions are request lines the shell already answers — the same strings `hogar-shell` takes on the command line — so a stage needs no new vocabulary and anything bindable to a key is bindable to a timeout. `hogar-shell --list` is the full menu; `lock on`, `shell dpms off` and `session do suspend` are the usual three.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct IdleStage {
    pub timeout: u64,
    pub action: String,
    /// Run when the seat wakes, if this stage had fired. Empty leaves the action standing — which is right for a lock and wrong for a blanked screen, so the dpms stage below pairs them.
    pub return_action: String,
}

impl Default for IdleStage {
    fn default() -> Self {
        Self {
            timeout: 300,
            action: String::new(),
            return_action: String::new(),
        }
    }
}

impl IdleStage {
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout.max(1))
    }
}

/// Idle behaviour (`[idle]`): the timeouts, and what keeps them from firing.
///
/// `respect_inhibitors` is not a condition the shell evaluates — it selects which question is asked of the compositor. `ext-idle-notify-v1` has one request that stays quiet while any client holds an idle inhibitor and another that reports raw input idleness, and the compositor is the only thing that can tell them apart.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct IdleConfig {
    pub enabled: bool,
    pub stages: Vec<IdleStage>,
    /// Hold every stage while something is playing audio — a film should not be interrupted by a lock screen.
    pub inhibit_when_audio: bool,
    /// Hold every stage while the machine is on mains power.
    pub inhibit_when_charging: bool,
    /// Honour idle inhibitors taken out by other applications.
    pub respect_inhibitors: bool,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            // Off out of the box: a shell that locks a machine the user never asked it to lock is a bug, and the timeouts below are a starting point rather than a policy anyone consented to.
            enabled: false,
            stages: vec![
                IdleStage {
                    timeout: 300,
                    action: "lock on".to_string(),
                    return_action: String::new(),
                },
                IdleStage {
                    timeout: 360,
                    action: "shell dpms off".to_string(),
                    return_action: "shell dpms on".to_string(),
                },
            ],
            inhibit_when_audio: true,
            inhibit_when_charging: false,
            respect_inhibitors: true,
        }
    }
}
