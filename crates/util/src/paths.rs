//! Where this shell's files go.
//!
//! The XDG rule, the `~` expansion, the `user-dirs.dirs` reading and the app-name scoping all live in
//! `telar::paths`; what is left here is the part that is this shell's own — which directories it declares to
//! the runtime through [`ShellPaths`], and the fallbacks it takes when the session provides none.

use std::path::PathBuf;

use telar::{AppPathsProvider, paths};

pub use telar::paths::{ensure_dir, expand_tilde, home as home_dir, user_dir};

/// The name every app-scoped directory is nested under, and the one handed to the runner.
pub const APP: &str = "hogar-shell";

/// The directories this shell declares to Telar, so `telar::paths::*` and every widget behind it resolve the
/// same places the shell writes to. Handed to the runner in place of the three stubs that used to answer
/// `None` and leave every caller to work it out again.
pub struct ShellPaths;

impl AppPathsProvider for ShellPaths {
    fn config_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_CONFIG_HOME", ".config"))
    }

    fn data_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_DATA_HOME", ".local/share"))
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_CACHE_HOME", ".cache"))
    }

    fn state_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_STATE_HOME", ".local/state"))
    }

    /// `$XDG_RUNTIME_DIR` when the session provides one (tmpfs, cleaned on logout — where a socket belongs),
    /// else a `/tmp` path scoped to the user so two users on one machine never collide.
    fn runtime_dir(&self) -> Option<PathBuf> {
        Some(
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| {
                    let uid = std::env::var("UID").unwrap_or_else(|_| "user".to_string());
                    PathBuf::from(format!("/tmp/hogar-shell-{uid}"))
                }),
        )
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    paths::resolve_base(std::env::var_os(var), std::env::var_os("HOME"), fallback)
}

/// Falls back to resolving the base directly, for the paths a surface asks for before the runner has installed
/// anything — a bar builds its config path while the event loop is still being constructed.
fn scoped(installed: Option<PathBuf>, var: &str, fallback: &str) -> PathBuf {
    installed.unwrap_or_else(|| xdg(var, fallback).join(APP))
}

/// Persistent user state — notes and notification history.
pub fn data_dir() -> PathBuf {
    scoped(paths::data(), "XDG_DATA_HOME", ".local/share")
}

/// Machine-written state the user never edits: what the shell remembers across restarts, as opposed to the
/// config they own.
pub fn state_dir() -> PathBuf {
    scoped(paths::state(), "XDG_STATE_HOME", ".local/state")
}

/// Regenerable artefacts — icons, cover art, thumbnails — that are safe to delete.
pub fn cache_dir() -> PathBuf {
    scoped(paths::cache(), "XDG_CACHE_HOME", ".cache")
}

/// Where the IPC socket lives.
pub fn runtime_dir() -> PathBuf {
    paths::runtime()
        .or_else(|| ShellPaths.runtime_dir().map(|base| base.join(APP)))
        .unwrap_or_else(|| PathBuf::from("/tmp").join(APP))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every app directory is nested under the shell's own name, whether the runner installed a provider or the
    /// fallback resolved it — a bar that builds its path early must not write somewhere else than one that
    /// builds it late.
    #[test]
    fn every_app_directory_is_scoped_to_the_shell() {
        for dir in [data_dir(), state_dir(), cache_dir(), runtime_dir()] {
            assert!(
                dir.ends_with(APP),
                "{} is not scoped to {APP}",
                dir.display()
            );
        }
    }

    #[test]
    fn the_runtime_directory_is_user_scoped_when_the_session_offers_none() {
        let dir = ShellPaths.runtime_dir().expect("always answers");
        assert!(
            dir.is_absolute(),
            "a socket needs an absolute path: {}",
            dir.display()
        );
    }
}
