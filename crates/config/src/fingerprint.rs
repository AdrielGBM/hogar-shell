//! What the config files hold, told apart by their bytes.
//!
//! A reload is worth running only when the files hold something the shell is not already showing, and that is a question about *content*. A modification time answers a different one — whether a file was written — so it reloads on a `touch`, on an editor saving a buffer it never changed, and on the settings window's own write coming back; and it cannot tell apart two different writes that land inside one tick of the clock the kernel stamps them with, which is a user's edit taken for the echo of the shell's and left off screen. A hash of the bytes answers the right question on every filesystem.
//!
//! [`Stamp`] is the other half: what the running shell, or one surface in it, was last brought up to date with. One rule decides both — a reload reaches whatever does not already reflect the content it carries — so an echo skipped by the shell and a change skipped by the window that made it are the same comparison rather than two mechanisms that can disagree.
//!
//! **What a reload reads, and so what a fingerprint covers.** Exactly these, because an input left out is an edit the shell cannot see:
//!
//! - `config.toml`, read by `Config::load` — which writes the starter config in its place when it is missing.
//! - `tokens.toml` beside it, read by `Config::load` through `TokenOverrides::load`. A missing or unreadable one is the same as none.
//! - `monitors/<output>/config.toml`, read by `Config::for_output` for each screen the reconcile plans, and merged over `config.toml` for that screen.
//! - `layouts/*.toml`, read by `layout::LayoutStore::load` — where everything the shell draws is written down. They are not config, and the reload path treats them apart: what a layout edit needs is the store read again, not the config. They are fingerprinted here all the same, because there is one watcher and a second one polling a second set of files would be two answers to "did anything change".
//!
//! Nothing else a load produces comes from a file: no section deserializes from one or defaults to one, and what the config names — the `[paths]` directories, the palette cache — is read by whatever uses it, after the load. A wallpaper-derived palette is not a config input at all; it reaches the shell as a reload somebody asked for, [`Reload::Always`].

use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

use crate::{Config, TokenOverrides};

/// The files a reload reads, each with what it held — see the module doc for which files, and why exactly those.
///
/// The hash never leaves the process, so `DefaultHasher` giving different answers across Rust versions costs nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint(Vec<(PathBuf, Held)>);

/// What one file held. `config.toml` can be any of the three and each loads differently — a missing one is replaced by the starter config, an unreadable one is an error the user is told about — so each is its own state. `tokens.toml` and an override are listed only when they have bytes, the one case the load reads them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Missing,
    Unreadable,
    Bytes(u64),
}

impl Fingerprint {
    /// Reads every file the config at `config_path` is made of: a few kilobytes, read and hashed in microseconds.
    pub fn read(config_path: &Path) -> Self {
        Self::around(config_path, held(config_path))
    }

    /// The files as they stand, except that `config.toml` is taken to hold `text` rather than read — `None` being a file that could not be read, taken as missing.
    ///
    /// For a caller holding those bytes because it has just read or written them itself: fingerprinting them rather than reading the file again leaves no moment for another writer to land in (see [`crate::Saved`]). The rest of the set is still read from disk, which is exact for a caller that writes nothing but `config.toml` — the settings window writes nothing else.
    pub fn with_config(config_path: &Path, text: Option<&str>) -> Self {
        let held = text.map_or(Held::Missing, |text| hashed(text.as_bytes()));
        Self::around(config_path, held)
    }

    fn around(config_path: &Path, config: Held) -> Self {
        let mut files = vec![(config_path.to_path_buf(), config)];
        let tokens = TokenOverrides::path(config_path);
        if let held @ Held::Bytes(_) = held(&tokens) {
            files.push((tokens, held));
        }
        for file in Config::monitor_overrides(config_path) {
            if let held @ Held::Bytes(_) = held(&file) {
                files.push((file, held));
            }
        }
        for file in layout_files(config_path) {
            if let held @ Held::Bytes(_) = held(&file) {
                files.push((file, held));
            }
        }
        Self(files)
    }

    /// Whether `config.toml` is there at all. Some editors save by removing the old file before the new one lands, and a reload in that moment would load — and write — the starter config in place of the user's.
    pub fn settled(&self) -> bool {
        self.0
            .first()
            .is_some_and(|(_, held)| *held != Held::Missing)
    }
}

/// Every layout file beside `config_path`, sorted so two reads of an unchanged directory fingerprint alike.
fn layout_files(config_path: &Path) -> Vec<PathBuf> {
    let Some(dir) = config_path.parent().map(|dir| dir.join("layouts")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|it| it.to_str()) == Some("toml"))
        .collect();
    files.sort();
    files
}

fn held(path: &Path) -> Held {
    match std::fs::read(path) {
        Ok(bytes) => hashed(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Held::Missing,
        Err(_) => Held::Unreadable,
    }
}

/// The one hash both a read file and bytes in hand go through, so the same content fingerprints the same whichever way it arrived.
fn hashed(bytes: &[u8]) -> Held {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    Held::Bytes(hasher.finish())
}

/// How much a reload asks for when the files hold exactly what was last applied.
///
/// A monitor arriving or leaving is neither kind: it changes which surfaces exist and nothing the files hold, so it plans the running config over the new set of screens rather than loading `config.toml` again — a reload for it would load, and report, a file nobody touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reload {
    /// Nothing. What the file watcher asks for: it wakes on any change it sees, and a write that leaves the files as they were — the settings window's own save coming back, an edit undone before the watcher looked — has nothing to deliver.
    IfChanged,
    /// Everything, whatever the files hold. What a reload somebody asked for is — `hogar-shell shell reload`, a palette that moved under an unchanged file — and delivering what no fingerprint of the config covers (the palette, a font or an icon theme installed since) is what it is for.
    Always,
}

/// The content something was last brought up to date with: the running shell as a whole, or one surface in it.
///
/// Empty until something says otherwise, and an empty stamp reflects nothing, so the first reload always reaches it: a stamp that guessed would suppress whichever reload it guessed wrong about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp(Option<Fingerprint>);

impl Stamp {
    /// Whether whatever carries this stamp is already showing `content`.
    pub fn reflects(&self, content: &Fingerprint) -> bool {
        self.0.as_ref() == Some(content)
    }

    /// Whether a reload of `content` has anything to do here.
    pub fn needs(&self, reload: Reload, content: &Fingerprint) -> bool {
        reload == Reload::Always || !self.reflects(content)
    }

    /// Records that `content` is what this now reflects.
    ///
    /// **Never ahead of the truth.** Take the fingerprint *before* the load or build it describes, or of the very bytes a write has just put on disk — never after a load: a stamp that names content the shell has not shown yet is one that suppresses the very reload that would have shown it. A stamp that lags costs a spare reload and nothing else.
    pub fn record(&mut self, content: Fingerprint) {
        self.0 = Some(content);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    const CLOCK: &str = "[clock]\nformat = \"%H:%M\"\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hogar-shell-fingerprint-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    fn set_modified(path: &Path, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    fn cleanup(path: &Path) {
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// **An identical rewrite reloads nothing.** A `touch`, an editor saving a buffer it never changed, a script writing back what it read: each moves the modification time and leaves nothing on disk the shell is not already showing. Reloading for it would rebuild every open surface and toast "config reloaded" about a config that did not change.
    #[test]
    fn an_identical_rewrite_or_a_touch_reloads_nothing() {
        let path = scratch("identical");
        std::fs::write(&path, CLOCK).unwrap();
        let mut applied = Stamp::default();
        applied.record(Fingerprint::read(&path));

        std::fs::write(&path, CLOCK).unwrap();
        set_modified(&path, SystemTime::now() + Duration::from_secs(60));

        let seen = Fingerprint::read(&path);
        assert!(
            !applied.needs(Reload::IfChanged, &seen),
            "the same bytes under a new modification time are the config already on screen"
        );
        assert!(
            applied.needs(Reload::Always, &seen),
            "but a reload somebody asked for still runs: it is how what no fingerprint covers reaches the shell"
        );
        cleanup(&path);
    }

    /// **A different write in the same instant as the shell's is a change.** The kernel stamps a write from a clock that moves every few milliseconds on most filesystems and every second or two on some, so two writes a millisecond apart can share a modification time — and when they are also the same length, as a user's edit to the value the settings window just wrote often is, nothing a `stat` returns tells them apart. A watcher comparing times would take the user's edit for the echo of the shell's write and leave it off screen until the next one.
    #[test]
    fn a_different_write_within_a_millisecond_of_a_shell_write_reloads() {
        let path = scratch("same-tick");
        util::writer::write(&path, CLOCK.as_bytes().to_vec()).unwrap();
        let shell_wrote = std::fs::metadata(&path).unwrap();
        let mut applied = Stamp::default();
        applied.record(Fingerprint::read(&path));

        std::fs::write(&path, "[clock]\nformat = \"%H:%S\"\n").unwrap();
        set_modified(&path, shell_wrote.modified().unwrap());
        let edited = std::fs::metadata(&path).unwrap();
        assert_eq!(
            (edited.modified().unwrap(), edited.len()),
            (shell_wrote.modified().unwrap(), shell_wrote.len()),
            "the two writes share a time and a size, as on a filesystem whose clock did not tick between them"
        );

        assert!(
            applied.needs(Reload::IfChanged, &Fingerprint::read(&path)),
            "different bytes are a change however close behind the shell's write they land"
        );
        cleanup(&path);
    }

    /// **A parse failure moves nothing, so an edit back to the last good content is not a change.** A reload that fails to parse keeps the last working config on screen and reports the error, and the shell records only a config that loaded — so the stamp still names the content on screen, and the edit that fixes a typo by putting the file back delivers nothing new: no rebuild, and no "reloaded" toast for a config that never stopped running.
    #[test]
    fn a_parse_failure_then_an_edit_back_to_the_last_good_content_reloads_nothing() {
        let path = scratch("parse-failure");
        std::fs::write(&path, CLOCK).unwrap();
        let mut applied = Stamp::default();
        let good = Fingerprint::read(&path);
        assert!(Config::load(&path).is_ok());
        applied.record(good);

        std::fs::write(&path, "[clock\nformat = ").unwrap();
        let broken = Fingerprint::read(&path);
        assert!(
            applied.needs(Reload::IfChanged, &broken),
            "a typo is a change: its reload is what tells the user the edit did not take"
        );
        assert!(
            Config::load(&path).is_err(),
            "and that reload fails, so nothing it read is recorded as applied"
        );

        std::fs::write(&path, CLOCK).unwrap();
        assert!(
            !applied.needs(Reload::IfChanged, &Fingerprint::read(&path)),
            "the last good content is what is still on screen, so going back to it is nothing to reload"
        );
        cleanup(&path);
    }

    /// The per-monitor files are part of what a reload reads, so writing, changing or deleting one is a change like an edit to `config.toml` — and a moment with no `config.toml` at all is a change too, but not one to reload in.
    #[test]
    fn monitor_overrides_and_a_missing_config_are_part_of_the_fingerprint() {
        let path = scratch("monitors");
        std::fs::write(&path, CLOCK).unwrap();
        let bare = Fingerprint::read(&path);

        let over = Config::monitor_dir(&path).join("DP-1").join("config.toml");
        std::fs::create_dir_all(over.parent().unwrap()).unwrap();
        std::fs::write(&over, "[bars.left]\nsize = 48\n").unwrap();
        let overridden = Fingerprint::read(&path);
        assert_ne!(
            overridden, bare,
            "a new override changes what a bar on that screen is built from"
        );

        std::fs::write(&over, "[bars.left]\nsize = 40\n").unwrap();
        assert_ne!(
            Fingerprint::read(&path),
            overridden,
            "and so does an edit to one"
        );

        std::fs::remove_file(&over).unwrap();
        assert_eq!(
            Fingerprint::read(&path),
            bare,
            "deleting it puts the files back to what they were without it"
        );

        std::fs::remove_file(&path).unwrap();
        let missing = Fingerprint::read(&path);
        assert_ne!(
            missing, bare,
            "a missing config.toml is part of the fingerprint"
        );
        assert!(
            !missing.settled(),
            "and is the moment in an editor's save a reload must wait out"
        );
        cleanup(&path);
    }

    /// **`tokens.toml` is an input of the load, so an edit to it is a change.** `Config::load` reads it beside `config.toml`, and a fingerprint that left it out would make a token edit invisible — the very bug a content fingerprint exists to remove, which the modification-time watcher had too.
    #[test]
    fn an_edit_to_tokens_toml_is_a_change() {
        let path = scratch("tokens");
        std::fs::write(&path, CLOCK).unwrap();
        let bare = Fingerprint::read(&path);
        let tokens = TokenOverrides::path(&path);

        std::fs::write(&tokens, "radius = 4.0\n").unwrap();
        assert_eq!(
            Config::load(&path).unwrap().tokens.radius,
            Some(4.0),
            "the load reads it, which is what puts it in the fingerprint"
        );
        let four = Fingerprint::read(&path);
        assert_ne!(
            four, bare,
            "writing a token file changes what the load produces"
        );

        std::fs::write(&tokens, "radius = 8.0\n").unwrap();
        assert_ne!(Fingerprint::read(&path), four, "and so does editing one");

        std::fs::remove_file(&tokens).unwrap();
        assert_eq!(
            Fingerprint::read(&path),
            bare,
            "a missing token file is no overrides at all, the same as never having written one"
        );
        cleanup(&path);
    }

    /// **The bytes a save hands back fingerprint as the file it found and the file it left.** A caller stamping from them — rather than from a second look at the file, with room for another writer in between — is only right if both routes agree on the same content: one hash for bytes read from disk and bytes in hand, and a save that hands back exactly what it read and wrote, a missing file included.
    #[test]
    fn a_save_s_own_bytes_fingerprint_as_the_file_it_found_and_the_file_it_left() {
        let path = scratch("saved");
        let found = Fingerprint::read(&path);
        let clock = toml::Table::from_iter([("format".to_string(), toml::Value::from("%H:%M"))]);
        let saved = Config::save_section(&path, "clock", &clock).unwrap();
        assert_eq!(
            Fingerprint::with_config(&path, saved.read.as_deref()),
            found,
            "a save into a missing file read nothing, and says so"
        );
        assert_eq!(
            Fingerprint::with_config(&path, Some(&saved.written)),
            Fingerprint::read(&path),
            "what it wrote is the file it left"
        );

        std::fs::write(TokenOverrides::path(&path), "radius = 4.0\n").unwrap();
        let found = Fingerprint::read(&path);
        let clock = toml::Table::from_iter([("format".to_string(), toml::Value::from("%H:%S"))]);
        let saved = Config::save_section(&path, "clock", &clock).unwrap();
        assert_eq!(
            Fingerprint::with_config(&path, saved.read.as_deref()),
            found,
            "what it read is the file it found, with the rest of the set read from disk"
        );
        assert_eq!(
            Fingerprint::with_config(&path, Some(&saved.written)),
            Fingerprint::read(&path),
            "and what it wrote is the file it left"
        );
        cleanup(&path);
    }
}
