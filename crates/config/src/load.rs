//! Reading `config.toml`, and the monitor overrides merged into it.
//!
//! There is no migration step and no `version` key: a key that moves is a key renamed by hand. That is a deliberate trade for a shell with one installation — see the non-goal in `features.md` for when to reopen it.

use std::path::{Path, PathBuf};

use crate::config::Config;

use toml_edit::{DocumentMut, Item};

/// Re-numbers every table in `doc` so each one renders where its key sits, with its children under it.
///
/// `toml_edit` carries a render position on every table it *parsed*, and a table built from scratch has none — so replacing `[theme]` with a value carrying `[theme.scale]`, `[theme.export]` and `[theme.fonts.*]` scattered those children through the file between unrelated sections, and printed `[theme]` itself *after* its own children. The result still parses, which is why nothing caught it; it also destroys the layout of a file this function promises to preserve.
///
/// Walking the document once and handing out positions in key order puts every child back under its parent without touching any decor, so the comments and key order the caller was promised survive.
pub(crate) fn keep_subtables_with_their_parent(doc: &mut DocumentMut) {
    fn walk(table: &mut toml_edit::Table, next: &mut isize) {
        for (_, item) in table.iter_mut() {
            match item {
                Item::Table(child) => {
                    child.set_position(Some(*next));
                    *next += 1;
                    walk(child, next);
                }
                // A list of tables (`[[idle.stages]]`) renders with its parent already; only its own children need positions, and it has none.
                Item::ArrayOfTables(_) | Item::Value(_) | Item::None => {}
            }
        }
    }
    let mut next: isize = 0;
    walk(doc.as_table_mut(), &mut next);
}

/// Sections one process owns, and which a per-monitor file therefore cannot change.
///
/// Each of these is read once for the whole shell rather than once per surface: the UI locale and the helper applications (`general`), the icon store (`icons`), the notification daemon (`notifications`), the launcher — a single overlay, not a per-output surface — the user's directories (`paths`), and every section whose job is to start a background producer. A per-monitor value here would apply on whichever screen happened to be reconciled last and do nothing on the rest, which is worse than not being allowed at all.
pub const GLOBAL_ONLY_SECTIONS: &[&str] = &[
    "general",
    "icons",
    "notifications",
    // The column follows the focused screen rather than existing per output, so where it sits is one answer for the whole shell; a per-monitor `edge` would apply on whichever screen it last opened on.
    "stack",
    "launcher",
    "paths",
    "audio",
    "brightness",
    "battery",
    "network",
    "bluetooth",
    "gpu",
    "weather",
];

pub(crate) fn monitor_config_path(path: &Path, output: &str) -> PathBuf {
    Config::monitor_dir(path).join(output).join("config.toml")
}

/// Deep-merges `over` into `base`: tables recurse key by key, everything else replaces.
///
/// Arrays replace rather than concatenate on purpose. A bar's module list is an array, and "the global list plus this monitor's" has no sensible reading — a user overriding `start` means *this* is the start zone.
pub(crate) fn merge_into(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(base), toml::Value::Table(over)) => {
            for (key, value) in over {
                match base.get_mut(&key) {
                    Some(existing) => merge_into(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, over) => *base = over,
    }
}

/// Why reading `config.toml` failed. Carries the `toml` error verbatim so the message the user sees names the offending key and line rather than just "invalid config".
#[derive(Debug)]
pub enum LoadError {
    Parse(toml::de::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Parse(e) => write!(f, "config parse error: {e}"),
            LoadError::Io(e) => write!(f, "config read error: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Why persisting a config section failed.
#[derive(Debug)]
pub enum SaveError {
    Serialize(toml::ser::Error),
    Parse(toml_edit::TomlError),
    Io(std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Serialize(e) => write!(f, "serializing config section: {e}"),
            SaveError::Parse(e) => write!(f, "parsing config file: {e}"),
            SaveError::Io(e) => write!(f, "writing config file: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}
