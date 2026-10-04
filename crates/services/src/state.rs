//! What the shell remembers across restarts.
//!
//! Distinct from `config.toml`, which the user owns and hand-edits: this is machine-written state — which wallpaper is up, whether do-not-disturb is on, how often each app was launched. It lives in `$XDG_STATE_HOME/hogar-shell/state.json` so a reload, a restart or a re-login lands back where the user left off, and so a toggle flipped from one surface is the same toggle every other surface reads.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use platform_wayland::EventSender;
use serde::{Deserialize, Serialize};

use util::broadcast::Store;
use util::paths;
use util::writer;

/// Every persisted field is `#[serde(default)]` so a state file written by an older build — or a hand-deleted key — still loads instead of resetting the user's whole session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellState {
    /// The wallpaper currently applied, when one was set at runtime rather than pinned in the config.
    pub wallpaper: Option<PathBuf>,
    /// Per-output wallpaper, keyed by output name; falls back to `wallpaper`.
    pub wallpaper_monitors: HashMap<String, PathBuf>,
    pub dnd: bool,
    /// Applications whose notifications are recorded but never allowed to pop. Persisted because a mute the user set from the history panel is a standing decision about that application, not about this session.
    pub muted_apps: Vec<String>,
    pub game_mode: bool,
    pub idle_inhibit: bool,
    /// How many times each desktop-entry id was launched, so the launcher can rank by familiarity.
    pub launch_counts: HashMap<String, u32>,
    /// Which layout the shell draws, by name. Machine state rather than a config key: it is a choice about this installation, not a description of one, and `layout use` is what changes it. `None`, or a name no layout answers to, falls back to the built-in one.
    pub layout: Option<String>,
    /// Typed values the user's layouts, rules and scripts set and read by name (`$name` in an expression), kept across restarts.
    pub vars: BTreeMap<String, Var>,
    /// The bundles `layout import` brought in, by the name their manifest gives: which files each wrote, and what the user decided about what those files run (DEC-30).
    pub bundles: BTreeMap<String, BundleRecord>,
}

/// One imported bundle, as the shell remembers it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BundleRecord {
    /// What made the bundle the bundle it is when it was first imported — a hash of its name and the names of the layouts and komponents it writes (`layout::bundle::Bundle::identity`) — so a later bundle that takes its name but writes other files is refused rather than taken for a reimport.
    pub identity: String,
    /// Each file the import wrote, as a finding names it (`layouts/<name>.toml`, `components/<name>.toml`), with a hash of what it wrote there: a file still holding exactly that is one a reimport may replace.
    pub files: BTreeMap<String, String>,
    /// Every answer the user gave, each bound to the exact text it was given for.
    pub decisions: Vec<TrustDecision>,
}

/// The user's answer for one thing a bundle's file runs: a source's command or address, or an action line, where it is written and exactly as it was written when they answered.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrustDecision {
    pub file: String,
    pub key: String,
    pub text: String,
    #[serde(default)]
    pub lock_safe: bool,
    pub accepted: bool,
}

/// One stored variable: its type and its value together, so a value can never be read back as a type it was not set as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Var {
    Text(String),
    /// Always finite.
    Number(f64),
    Bool(bool),
    /// `#rrggbb` or `#rrggbbaa`.
    Color(String),
    /// A path to a picture.
    Image(String),
    /// A font family name.
    Font(String),
    List(VarList),
}

/// A list variable: every item is of the one declared type, so an empty list still has one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VarList {
    pub of: VarType,
    pub items: Vec<Var>,
}

/// What a variable holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VarType {
    Text,
    Number,
    Bool,
    Color,
    Image,
    Font,
    List(Box<VarType>),
}

impl Var {
    pub fn ty(&self) -> VarType {
        match self {
            Var::Text(_) => VarType::Text,
            Var::Number(_) => VarType::Number,
            Var::Bool(_) => VarType::Bool,
            Var::Color(_) => VarType::Color,
            Var::Image(_) => VarType::Image,
            Var::Font(_) => VarType::Font,
            Var::List(list) => VarType::List(Box::new(list.of.clone())),
        }
    }
}

fn path() -> PathBuf {
    paths::state_dir().join("state.json")
}

/// Reads a state file, falling back to defaults when it is missing or unreadable. A corrupt file is reported and replaced by defaults rather than taken as fatal — losing remembered state is recoverable, refusing to start is not.
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

/// Hands `state` to [`util::writer`], which writes it off the UI thread — a synchronous write in a click handler would stall the frame — to a sibling temp file it then renames, so a crash mid-write can't leave a truncated file behind.
///
/// A queue rather than a thread per call, which is what this used to be. Both writes were atomic and the race was in which one finished last: two toggles a few milliseconds apart could rename in either order, so the *older* state won and the user's last flick of the switch came back undone after a restart. The writer hands out a generation per path and drops anything a newer write has overtaken, so the last state the shell was in is the state on disk.
fn persist(state: &ShellState) {
    let Ok(text) = serde_json::to_string_pretty(state) else {
        return;
    };
    writer::queue(path(), text.into_bytes());
}

/// The current state.
pub fn get() -> ShellState {
    STATE.get()
}

/// What the state file holds: what the next start reads back, whatever this process holds in memory.
pub fn on_disk() -> ShellState {
    load_from(&path())
}

/// Applies `change`, fans the result out to every subscriber, and persists it. The single write path, so no caller has to remember to save; the write is queued before the next change can start, so the file ends with the last change made.
pub fn update(change: impl FnOnce(&mut ShellState)) {
    STATE.update_then(change, persist);
}

/// [`update`] for a change that may be refused: an `Err` from `change` leaves the state as it was, and nothing is fanned out or persisted.
pub fn try_update<R, E>(change: impl FnOnce(&mut ShellState) -> Result<R, E>) -> Result<R, E> {
    STATE.try_update_then(change, persist)
}

/// Registers `tx` for live state changes, sending the current value immediately. Pass to `platform_wayland::watch` from a surface that reflects a persisted toggle.
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

    /// DEC-30: what a bundle wrote and each answer about what it runs, bound to the text answered for, come back from the file as they went in.
    #[test]
    fn a_bundles_files_and_answers_survive_the_file() {
        let mut state = ShellState::default();
        state.bundles.insert(
            "nord".to_string(),
            BundleRecord {
                identity: "0f0f".to_string(),
                files: BTreeMap::from([(
                    "layouts/nord.toml".to_string(),
                    "00ff00ff00ff00ff".to_string(),
                )]),
                decisions: vec![TrustDecision {
                    file: "layouts/nord.toml".to_string(),
                    key: "sources.weather.cmd".to_string(),
                    text: "curl -s 'wttr.in?format=3'".to_string(),
                    lock_safe: true,
                    accepted: true,
                }],
            },
        );
        let written = serde_json::to_string(&state).expect("writes");
        assert_eq!(
            serde_json::from_str::<ShellState>(&written).expect("reads back"),
            state
        );
        assert_eq!(
            serde_json::from_str::<ShellState>(r#"{"dnd": true}"#)
                .expect("an older file")
                .bundles,
            BTreeMap::new(),
            "a file from before bundles reads as none imported"
        );
    }

    #[test]
    fn a_variable_keeps_its_type_through_the_file() {
        let text = r##"{"vars": {
            "count": {"type": "number", "value": 3},
            "accent": {"type": "color", "value": "#88c0d0"},
            "tags": {"type": "list", "value": {"of": "text", "items": [{"type": "text", "value": "a"}]}}
        }}"##;
        let state: ShellState = serde_json::from_str(text).expect("parses");
        assert_eq!(state.vars["count"], Var::Number(3.0));
        assert_eq!(state.vars["accent"].ty(), VarType::Color);
        assert_eq!(
            state.vars["tags"].ty(),
            VarType::List(Box::new(VarType::Text))
        );
        let written = serde_json::to_string(&state).expect("writes");
        assert_eq!(
            serde_json::from_str::<ShellState>(&written).expect("reads back"),
            state
        );
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
