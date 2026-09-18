use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use util::broadcast::Store;
use util::writer;

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

static NOTES: Store<Vec<Note>> = Store::new(|| load_from(&notes_path()));

/// The notes as last saved, or an empty list when the file is missing or unparseable.
///
/// Answered from memory: the file is read once, the first time anything asks, and every [`save`] replaces the value before it queues the write. The panel asks every time it opens, so an edit made just before closing it comes back as itself rather than as the note it replaced, and the panel's thread never waits for the disk to catch up.
pub fn load() -> Vec<Note> {
    NOTES.get()
}

fn load_from(path: &Path) -> Vec<Note> {
    let Ok(text) = std::fs::read_to_string(path) else {
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

/// Makes `notes` what [`load`] answers from now on, and persists them. Best-effort: a write failure is logged, not surfaced.
///
/// Handed to [`util::writer`]'s queue, because nothing can regenerate a note the way a cache or an export is rebuilt. The writer replaces the file whole, so a crash mid-save leaves the previous copy rather than half of each, and keeps saves in the order they were made, so two edits a moment apart still leave the later one on disk. The panel's thread never waits for any of it.
pub fn save(notes: &[Note]) {
    save_to(&NOTES, notes_path(), notes);
}

fn save_to(saved: &'static Store<Vec<Note>>, path: PathBuf, notes: &[Note]) {
    let file = NotesFile {
        notes: saved.update(|current| *current = notes.to_vec()),
    };
    match toml::to_string_pretty(&file) {
        Ok(text) => writer::queue(path, text.into_bytes()),
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

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("hogar-shell-notes-{}", std::process::id()))
    }

    static SAVED: Store<Vec<Note>> = Store::new(|| load_from(&scratch().join("notes.toml")));

    /// The panel saves on a timer and loads every time it opens, so an edit and a reopen can be milliseconds apart. Both halves are pinned here, into a data dir that does not exist yet: a load straight after two saves answers with the later one without waiting for either write, and once the writer has caught up, the later one is also what is on disk.
    #[test]
    fn a_note_saved_twice_reads_back_as_the_later_edit() {
        let dir = scratch();
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("notes.toml");
        let note = |body: &str| Note {
            id: 1,
            icon: None,
            title: "Groceries".to_string(),
            body: body.to_string(),
        };

        assert_eq!(SAVED.get(), Vec::new(), "the first open finds no notes");
        save_to(&SAVED, path.clone(), &[note("Milk")]);
        save_to(&SAVED, path.clone(), &[note("Milk, eggs")]);
        assert_eq!(
            SAVED.get(),
            vec![note("Milk, eggs")],
            "the reopened panel shows the last edit, not the one before it"
        );

        writer::flush();
        assert_eq!(
            load_from(&path),
            vec![note("Milk, eggs")],
            "and the later edit is the one the next start reads"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
