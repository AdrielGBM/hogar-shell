use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One persisted note: an optional icon (`set:name`, e.g. `mdi:home`), a title, and a body. Stored in a TOML array of tables (`[[notes]]`) under the data dir; the panel is the single editor, loading on open and saving on edit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Default, Serialize, Deserialize)]
struct NotesFile {
    #[serde(default)]
    notes: Vec<Note>,
}

/// The notes on disk, or an empty list when the file is missing or unparseable.
pub fn load() -> Vec<Note> {
    let path = notes_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<NotesFile>(&text) {
        Ok(file) => file.notes,
        Err(e) => {
            tracing::warn!("notes parse error ({e}); starting with an empty list");
            Vec::new()
        }
    }
}

/// Persists `notes`, creating the data dir if needed. Best-effort: a write failure is logged, not surfaced.
pub fn save(notes: &[Note]) {
    let path = notes_path();
    let file = NotesFile {
        notes: notes.to_vec(),
    };
    match toml::to_string_pretty(&file) {
        Ok(text) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&path, text) {
                tracing::warn!("notes save failed: {e}");
            }
        }
        Err(e) => tracing::warn!("notes serialize failed: {e}"),
    }
}

/// The next free note id: one past the current maximum (ids never reused within a session).
pub fn next_id(notes: &[Note]) -> u64 {
    notes.iter().map(|n| n.id).max().map_or(1, |m| m + 1)
}

fn notes_path() -> PathBuf {
    util::paths::data_dir().join("notes.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_round_trip_through_toml() {
        let notes = vec![
            Note {
                id: 1,
                icon: Some("mdi:home".to_string()),
                title: "Buy coffee".to_string(),
                body: "Ground, 250g.".to_string(),
            },
            Note {
                id: 2,
                icon: None,
                title: "Call the dentist".to_string(),
                body: String::new(),
            },
        ];
        let file = NotesFile {
            notes: notes.clone(),
        };
        let text = toml::to_string_pretty(&file).expect("serialize");
        let parsed: NotesFile = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.notes, notes);
        // A note without an icon omits the key entirely, and re-parses as `None`.
        assert!(!text.contains("icon = \"\""));
        assert_eq!(parsed.notes[1].icon, None);
    }

    #[test]
    fn next_id_is_one_past_the_max() {
        assert_eq!(next_id(&[]), 1);
        let notes = vec![
            Note {
                id: 3,
                icon: None,
                title: String::new(),
                body: String::new(),
            },
            Note {
                id: 7,
                icon: None,
                title: String::new(),
                body: String::new(),
            },
        ];
        assert_eq!(next_id(&notes), 8);
    }
}
