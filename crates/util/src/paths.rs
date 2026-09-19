//! Where this shell's files go; until [`install`] runs, every answer here lies under a scratch root private to the process, so a test, a preview or a benchmark never reads or writes the user's files.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use telar::{AppPathsProvider, paths};

pub use telar::paths::ensure_dir;

/// The name every app-scoped directory is nested under, and the one handed to the runner.
pub const APP: &str = "hogar-shell";

static INSTALLED: OnceLock<()> = OnceLock::new();
static ISOLATED: OnceLock<PathBuf> = OnceLock::new();

/// Points every directory here at the user's own, and declares them to Telar.
pub fn install() {
    let _ = INSTALLED.set(());
    paths::install(APP, Arc::new(ShellPaths));
}

fn installed() -> bool {
    INSTALLED.get().is_some()
}

/// The scratch root this process resolves under, or `None` once [`install`] has pointed it at the user's directories; never removed at exit since producer threads may still be writing then, so each process clears the roots of processes that are gone instead.
pub fn isolated_root() -> Option<PathBuf> {
    (!installed()).then(|| {
        ISOLATED
            .get_or_init(|| {
                let parent = isolated_parent();
                remove_stale_roots(&parent);
                parent.join(std::process::id().to_string())
            })
            .clone()
    })
}

fn isolated_parent() -> PathBuf {
    std::env::temp_dir().join(format!("{APP}-isolated"))
}

/// Removes every root under `parent` whose process has exited. A pid still running — this one, or one the kernel has reused — keeps its root.
fn remove_stale_roots(parent: &Path) {
    let Ok(entries) = parent.read_dir() else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if !Path::new("/proc").join(pid.to_string()).exists() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The directories this shell declares to Telar, so `telar::paths::*` and every widget behind it resolve the same places the shell writes to.
pub struct ShellPaths;

impl AppPathsProvider for ShellPaths {
    fn config_dir(&self) -> Option<PathBuf> {
        Some(xdg_config_home())
    }

    fn data_dir(&self) -> Option<PathBuf> {
        Some(xdg_data_home())
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_CACHE_HOME", ".cache"))
    }

    fn state_dir(&self) -> Option<PathBuf> {
        Some(xdg("XDG_STATE_HOME", ".local/state"))
    }

    /// `$XDG_RUNTIME_DIR` when the session provides one (tmpfs, cleaned on logout — where a socket belongs), else a `/tmp` path scoped to the user so two users on one machine never collide.
    fn runtime_dir(&self) -> Option<PathBuf> {
        if let Some(root) = isolated_root() {
            return Some(root.join("runtime"));
        }
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
    match isolated_root() {
        Some(root) => root.join("home").join(fallback),
        None => paths::resolve_base(std::env::var_os(var), std::env::var_os("HOME"), fallback),
    }
}

/// The user's home directory, or `None` where `$HOME` names nothing.
pub fn home_dir() -> Option<PathBuf> {
    match isolated_root() {
        Some(root) => Some(root.join("home")),
        None => paths::home(),
    }
}

/// Expands a leading `~` (bare or `~/…`) to the home directory, leaving every other path untouched.
pub fn expand_tilde(path: &Path) -> PathBuf {
    match (isolated_root(), path.strip_prefix("~")) {
        (None, _) => paths::expand_tilde(path),
        (Some(_), Ok(rest)) => home_dir().unwrap_or_default().join(rest),
        (Some(_), Err(_)) => path.to_path_buf(),
    }
}

/// A well-known user directory (`XDG_PICTURES_DIR`, `XDG_VIDEOS_DIR`, …), else `<home>/<fallback>`.
pub fn user_dir(name: &str, fallback: &str) -> PathBuf {
    match isolated_root() {
        Some(_) => home_dir().unwrap_or_default().join(fallback),
        None => paths::user_dir(name, fallback),
    }
}

/// The base every desktop reads user configuration from — `$XDG_CONFIG_HOME` — for what other programs put there, such as the GTK icon theme.
pub fn xdg_config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// The base every desktop reads user data from — `$XDG_DATA_HOME` — for what other programs put there, such as icon themes and `.desktop` files.
pub fn xdg_data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// The config the user owns and edits.
pub fn config_dir() -> PathBuf {
    xdg_config_home().join(APP)
}

/// Persistent user state — notes and notification history.
pub fn data_dir() -> PathBuf {
    xdg_data_home().join(APP)
}

/// Machine-written state the user never edits: what the shell remembers across restarts, as opposed to the config they own.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join(APP)
}

/// Regenerable artefacts — icons, cover art, thumbnails — that are safe to delete.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join(APP)
}

/// Where the IPC socket lives.
pub fn runtime_dir() -> PathBuf {
    ShellPaths
        .runtime_dir()
        .map(|base| base.join(APP))
        .unwrap_or_else(|| PathBuf::from("/tmp").join(APP))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every app directory is nested under the shell's own name, so two programs sharing a base never share a file.
    #[test]
    fn every_app_directory_is_scoped_to_the_shell() {
        for dir in [
            config_dir(),
            data_dir(),
            state_dir(),
            cache_dir(),
            runtime_dir(),
        ] {
            assert!(
                dir.ends_with(APP),
                "{} is not scoped to {APP}",
                dir.display()
            );
        }
    }

    #[test]
    fn the_runtime_directory_is_absolute() {
        let dir = ShellPaths.runtime_dir().expect("always answers");
        assert!(
            dir.is_absolute(),
            "a socket needs an absolute path: {}",
            dir.display()
        );
    }

    /// A test binary never installs, so everything it can be told about the user's directories has to land in its own scratch tree.
    #[test]
    fn a_process_that_never_installed_resolves_nothing_outside_its_scratch_root() {
        let root = isolated_root().expect("only the binary installs");
        assert!(root.starts_with(std::env::temp_dir()));
        let resolved = [
            config_dir(),
            data_dir(),
            state_dir(),
            cache_dir(),
            runtime_dir(),
            xdg_config_home(),
            xdg_data_home(),
            home_dir().expect("the scratch tree has a home"),
            expand_tilde(Path::new("~/.face")),
            expand_tilde(Path::new("~")),
            user_dir("XDG_PICTURES_DIR", "Pictures"),
            user_dir("XDG_VIDEOS_DIR", "Videos"),
        ];
        for path in resolved {
            assert!(
                path.starts_with(&root),
                "{} is outside {}",
                path.display(),
                root.display()
            );
        }
        assert_eq!(expand_tilde(Path::new("/etc/x")), Path::new("/etc/x"));
    }

    /// A scratch root outlives its process, so the next process clears the ones whose process is gone — and only those.
    #[test]
    fn only_the_roots_of_exited_processes_are_cleared() {
        let parent = isolated_root()
            .expect("only the binary installs")
            .join("stale-roots");
        let exited = parent.join(u32::MAX.to_string());
        let running = parent.join(std::process::id().to_string());
        let foreign = parent.join("not-a-pid");
        for root in [&exited, &running, &foreign] {
            std::fs::create_dir_all(root.join("home")).expect("a root to clear");
        }
        remove_stale_roots(&parent);
        assert!(!exited.exists(), "a root nothing runs under is left behind");
        assert!(running.exists(), "a running process lost its root");
        assert!(foreign.exists(), "a directory that is no root was removed");
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// The scratch root only holds if nothing works the user's directories out for itself, so the environment variables that name them are read here and nowhere else.
    #[test]
    fn only_this_module_reads_where_the_users_directories_are() {
        for needle in [
            "\"HOME\"",
            "\"XDG_CONFIG_HOME\"",
            "\"XDG_DATA_HOME\"",
            "\"XDG_STATE_HOME\"",
            "\"XDG_CACHE_HOME\"",
            "telar::paths",
            "services_core::app_paths",
        ] {
            let offenders = crate::deps::tests::sources_containing(needle, &["util/src/paths.rs"]);
            assert!(
                offenders.is_empty(),
                "these resolve a user directory without `util::paths` (`{needle}`): {offenders:#?}"
            );
        }
    }
}
