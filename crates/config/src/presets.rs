//! Theme presets: a `[theme]` kept under a name, as `themes/<name>.toml` beside `config.toml`, and put back into the config when picked.
//!
//! A preset is the look and nothing else, so `[theme.export]` — where the palette is written for other programs — stays out of it: picking a preset keeps whatever export the config already has.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, Saved};
use crate::load::SaveError;
use crate::sections::ThemeConfig;

const EXTENSION: &str = "toml";

#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
struct PresetFile {
    theme: ThemeConfig,
}

#[derive(Debug)]
pub enum PresetError {
    Name(String),
    Missing(String),
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    Save(SaveError),
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PresetError::Name(name) => write!(
                f,
                "'{name}' is not a preset name: use lowercase letters, digits, '-' and '_'"
            ),
            PresetError::Missing(name) => write!(f, "there is no preset named '{name}'"),
            PresetError::Io(e) => write!(f, "{e}"),
            PresetError::Parse(e) => write!(f, "reading the preset: {e}"),
            PresetError::Serialize(e) => write!(f, "writing the preset: {e}"),
            PresetError::Save(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PresetError {}

/// Where the presets of the config at `config_path` live.
pub fn dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("themes")
}

/// Whether `name` can name a preset: lowercase ASCII letters, digits, `-` and `_`, the rule bundles and komponents follow, so a file name never needs escaping and two presets never differ only by case.
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn path(config_path: &Path, name: &str) -> Result<PathBuf, PresetError> {
    if !is_name(name) {
        return Err(PresetError::Name(name.to_string()));
    }
    Ok(dir(config_path).join(format!("{name}.{EXTENSION}")))
}

/// Every preset beside the config at `config_path`, by name, in order; a file whose name no preset could have is passed over.
pub fn list(config_path: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir(config_path)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != EXTENSION || !path.is_file() {
                return None;
            }
            let name = path.file_stem()?.to_str()?;
            is_name(name).then(|| name.to_string())
        })
        .collect();
    names.sort();
    names
}

/// Keeps `theme` as the preset `name`, replacing one already called that.
pub fn save(config_path: &Path, name: &str, theme: &ThemeConfig) -> Result<(), PresetError> {
    let path = path(config_path, name)?;
    let file = PresetFile {
        theme: ThemeConfig {
            export: Default::default(),
            ..theme.clone()
        },
    };
    let mut document = toml::Table::try_from(&file).map_err(PresetError::Serialize)?;
    if let Some(toml::Value::Table(theme)) = document.get_mut("theme") {
        theme.remove("export");
    }
    let text = toml::to_string_pretty(&document).map_err(PresetError::Serialize)?;
    util::writer::write(path, text.into_bytes()).map_err(PresetError::Io)
}

/// The `[theme]` the preset `name` keeps.
pub fn load(config_path: &Path, name: &str) -> Result<ThemeConfig, PresetError> {
    let path = path(config_path, name)?;
    let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PresetError::Missing(name.to_string()),
        _ => PresetError::Io(e),
    })?;
    let file: PresetFile = toml::from_str(&text).map_err(PresetError::Parse)?;
    Ok(file.theme)
}

pub fn delete(config_path: &Path, name: &str) -> Result<(), PresetError> {
    let path = path(config_path, name)?;
    std::fs::remove_file(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PresetError::Missing(name.to_string()),
        _ => PresetError::Io(e),
    })
}

/// `preset` in place of `current`: its look, with the export `current` already has.
pub fn applied_to(current: &ThemeConfig, preset: ThemeConfig) -> ThemeConfig {
    ThemeConfig {
        export: current.export.clone(),
        ..preset
    }
}

/// Writes the preset `name` into the `[theme]` of the config at `config_path`, around whatever else the file says.
pub fn apply(config_path: &Path, name: &str) -> Result<Saved, PresetError> {
    let preset = load(config_path, name)?;
    let theme = applied_to(&Config::load_or_default(config_path).theme, preset);
    Config::save_section(config_path, "theme", &theme).map_err(PresetError::Save)
}
