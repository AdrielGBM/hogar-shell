//! What the shell remembers across restarts.
//!
//! Distinct from `config.toml`, which the user owns and hand-edits: this is machine-written state — which
//! wallpaper is up, whether do-not-disturb is on, how often each app was launched. It lives in
//! `$XDG_STATE_HOME/hogar-shell/state.json` so a reload, a restart or a re-login lands back where the user left
//! off, and so a toggle flipped from one surface is the same toggle every other surface reads.

use std::collections::HashMap;
use std::path::PathBuf;

use platform_wayland::EventSender;
use serde::{Deserialize, Serialize};

use util::broadcast::Store;
use util::paths;

/// Every persisted field is `#[serde(default)]` so a state file written by an older build — or a hand-deleted
/// key — still loads instead of resetting the user's whole session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellState {
    /// The wallpaper currently applied, when one was set at runtime rather than pinned in the config.
    pub wallpaper: Option<PathBuf>,
    /// Per-output wallpaper, keyed by output name; falls back to `wallpaper`.
    pub wallpaper_monitors: HashMap<String, PathBuf>,
    pub dnd: bool,
    /// Applications whose notifications are recorded but never allowed to pop. Persisted because a mute the
    /// user set from the history panel is a standing decision about that application, not about this session.
    pub muted_apps: Vec<String>,
    pub game_mode: bool,
    pub idle_inhibit: bool,
    /// How many times each desktop-entry id was launched, so the launcher can rank by familiarity.
    pub launch_counts: HashMap<String, u32>,
}

fn path() -> PathBuf {
    paths::state_dir().join("state.json")
}

/// Reads a state file, falling back to defaults when it is missing or unreadable. A corrupt file is reported
/// and replaced by defaults rather than taken as fatal — losing remembered state is recoverable, refusing to
/// start is not.
fn load_from(path: &std::path::Path) -> ShellState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ShellState::default();
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!("{}: {e}; starting from defaults", path.display());
            ShellState::default()
        }
    }
}

fn load() -> ShellState {
    load_from(&path())
}

static STATE: Store<ShellState> = Store::new(load);

/// Writes `state` to disk off the UI thread — a synchronous write in a click handler would stall the frame.
/// Written to a sibling temp file and renamed, so a crash mid-write can't leave a truncated file behind.
fn persist(state: ShellState) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-state-write".to_string())
        .spawn(move || {
            let Ok(text) = serde_json::to_string_pretty(&state) else {
                return;
            };
            let path = path();
            paths::ensure_dir(paths::state_dir());
            let temp = path.with_extension("json.tmp");
            if std::fs::write(&temp, text).is_ok() {
                let _ = std::fs::rename(&temp, &path);
            }
        });
}

/// The current state.
pub fn get() -> ShellState {
    STATE.get()
}

/// Applies `change`, fans the result out to every subscriber, and persists it. The single write path, so no
/// caller has to remember to save.
pub fn update(change: impl FnOnce(&mut ShellState)) {
    let next = STATE.update(change);
    persist(next);
}

/// Registers `tx` for live state changes, sending the current value immediately. Pass to
/// `platform_wayland::watch` from a surface that reflects a persisted toggle.
pub fn subscribe(tx: EventSender<ShellState>) {
    STATE.subscribe(tx);
}

/// Records a launch of `entry_id`, so the launcher can rank frequently-used apps first.
pub fn record_launch(entry_id: &str) {
    let id = entry_id.to_string();
    update(move |s| *s.launch_counts.entry(id).or_insert(0) += 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_corrupt_files_both_yield_defaults() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("state.json");

        assert_eq!(load_from(&file), ShellState::default(), "no file at all");

        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(
            load_from(&file),
            ShellState::default(),
            "a corrupt file loses remembered state, but is not fatal"
        );

        std::fs::write(&file, r#"{"dnd":true}"#).unwrap();
        assert!(load_from(&file).dnd, "a valid file is read back");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_and_missing_keys_round_trip() {
        // A file written by another build carries keys this one doesn't know, and lacks ones it does.
        let text = r#"{"dnd": true, "some_future_key": 42}"#;
        let state: ShellState = serde_json::from_str(text).expect("tolerates unknown keys");
        assert!(state.dnd);
        assert_eq!(
            state.launch_counts.len(),
            0,
            "missing keys take their default"
        );
    }
}
