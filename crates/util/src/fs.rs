//! Replacing a file's contents without ever leaving half of one behind.
//!
//! Everything the shell writes is a whole small document — `config.toml`, `state.json` — and `std::fs::write` on one of those truncates the file before it copies: a crash, a full disk or an OOM kill in the middle leaves the user with a file that is neither the old one nor the new one, which for a config the user hand-edited is the one loss the shell cannot make good. So a complete copy is written to a sibling of the target and then renamed over it, and every reader sees all of the old contents or all of the new ones and nothing in between.
//!
//! **Three details are what make that true rather than nearly true.** The copy has to be a *sibling*: `rename` is only atomic within one filesystem, so a temp file in `/tmp` would be a copy followed by a truncating write, which is the failure this module exists to remove. The copy has to be flushed before the rename, or the rename can be the durable half and the bytes it points at the half that is lost — the file comes back empty after a power cut. And the *directory* has to be flushed after the rename, which is the one everybody forgets: until it is, the new directory entry is only in the page cache, and a power cut takes the rename back and leaves the file exactly as it was before the write.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path` so that no reader and no crash can observe a partial file. Creates `path`'s parent directory, since a write is also how the shell seeds a config or state directory it has never had.
///
/// An `Err` means the write did not become durable. It does *not* mean the target is untouched — the rename can succeed and the flush that makes it survive a power cut still fail — but it does mean the target holds one whole document either way, which is the promise callers depend on.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    stage(path, bytes)?.commit()
}

/// The new contents, complete and flushed, sitting next to the file they are about to become.
///
/// Staging and committing are separate functions because the crash this module exists to survive happens *between* them, and that window is only testable if something can stand in it — [`write_atomic`] is the door every caller uses. Dropping a `Staged` that was never committed takes its temp file with it, so a write that failed part way through leaves no litter in the user's config directory.
struct Staged {
    temp: PathBuf,
    target: PathBuf,
    directory: PathBuf,
    committed: bool,
}

/// Writes the full new contents to a sibling temp file and flushes it to the device.
fn stage(path: &Path, bytes: &[u8]) -> std::io::Result<Staged> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&directory)?;
    let staged = Staged {
        temp: temp_path(&directory, path),
        target: path.to_path_buf(),
        directory,
        committed: false,
    };
    staged.fill(bytes)?;
    Ok(staged)
}

impl Staged {
    fn fill(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(&self.temp)?;
        // A brand-new file takes its mode from the umask rather than from the file it replaces, so a `config.toml` the user chmod'ed to 0600 would come back world-readable after a save from the settings panel.
        if let Ok(existing) = std::fs::metadata(&self.target) {
            file.set_permissions(existing.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// Renames the staged copy over the target and makes the rename itself durable.
    fn commit(mut self) -> std::io::Result<()> {
        std::fs::rename(&self.temp, &self.target)?;
        // From here the temp file no longer exists under its own name, so the cleanup in `drop` must not run.
        self.committed = true;
        let directory = std::fs::File::open(&self.directory)?;
        rustix::fs::fsync(&directory).map_err(std::io::Error::from)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// A name for the staged copy that no other writer can pick: hidden, so it never shows up in the directory the user browses, and stamped with the process and a counter, so two shells — or a shell and its own IPC client — writing one path cannot hand each other a half-written file to rename into place.
fn temp_path(directory: &Path, path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or(std::ffi::OsStr::new("file")));
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    directory.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-fs-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **The crash this module exists to survive, stood in the middle of.** `std::fs::write` truncates first, so a process that died between the two halves of a save left the user a `state.json` with the first half of a document in it and nothing that could read it. Staging cannot do that: until the rename runs, every byte of the new contents is in a file nobody is reading, and the target is untouched.
    #[test]
    fn a_crash_between_the_write_and_the_rename_leaves_the_old_file_intact() {
        let dir = scratch("crash");
        let path = dir.join("state.json");
        let before = r#"{"dnd":true}"#;
        std::fs::write(&path, before).unwrap();

        let staged = stage(&path, br#"{"dnd":false,"game_mode":true}"#).expect("the copy is made");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "the process is gone before the rename: the file still holds the whole old document, so it still parses"
        );
        assert_eq!(
            staged.temp.parent(),
            Some(dir.as_path()),
            "the copy has to be a sibling of the target — a rename across filesystems is not atomic"
        );
        assert!(
            std::fs::read(&staged.temp).unwrap().ends_with(b"true}"),
            "and it is a complete copy of the new contents, not a partial one"
        );

        // A write that never commits is a write that never happened, down to leaving nothing behind: a config directory that collected a temp file per failed save would be the user's problem to clean up.
        let temp = staged.temp.clone();
        drop(staged);
        assert!(!temp.exists(), "an abandoned copy takes itself with it");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole write, end to end: the new contents replace the old, the directory is made if the shell has never written there, the file's own mode survives, and nothing is left over.
    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_nothing_beside_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("write");
        let nested = dir.join("monitors").join("DP-1");
        let path = nested.join("config.toml");
        write_atomic(&path, b"[bars.top]\nsize = 34\n").expect("a first write seeds the directory");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[bars.top]\nsize = 34\n"
        );

        // A config the user has locked down is a decision about that file, and a save must not undo it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, b"[bars.top]\nsize = 40\n").expect("a second write replaces it");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[bars.top]\nsize = 40\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "the replacement carries the mode of the file it replaced"
        );

        let left: Vec<PathBuf> = std::fs::read_dir(&nested)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(left, vec![path], "a completed write leaves no temp file");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two writes to one path must not be able to collide over their staging file, or the second process renames the first one's half-written copy into place — the corruption this module removes, reintroduced by the fix for it.
    #[test]
    fn two_writers_stage_into_files_of_their_own() {
        let dir = scratch("unique");
        let path = dir.join("config.toml");
        let first = stage(&path, b"one").expect("staged");
        let second = stage(&path, b"two").expect("staged");
        assert_ne!(first.temp, second.temp);
        std::fs::remove_dir_all(&dir).ok();
    }
}
