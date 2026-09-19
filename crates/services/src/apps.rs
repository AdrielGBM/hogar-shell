//! The installed applications, from their `.desktop` files.
//!
//! Scanned once and cached for the process: the XDG application directories hold a few hundred entries, parsing them costs a few milliseconds, and a launcher that re-read them on every keystroke would be doing that work hundreds of times for a list that changes when you install software. A watcher notices when it does, so an install shows up in the launcher on its own; [`reload`] is the same thing on demand.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use platform_wayland::EventSender;

use util::broadcast::Store;

/// One launchable application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct App {
    /// The desktop-entry id (`firefox.desktop` → `firefox`), stable across restarts and what config and the launch-count store key on.
    pub id: String,
    pub name: String,
    /// `GenericName` or `Comment` — the subtitle a launcher row shows.
    pub description: String,
    /// The `Icon` key: an icon-theme name or an absolute path, for the freedesktop resolver.
    pub icon: String,
    /// `Exec` with its field codes removed.
    pub exec: String,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    /// The entry asks to run inside a terminal emulator.
    pub terminal: bool,
}

impl App {
    /// Everything a search should match against, not just the name: `keywords` is where an entry lists the words users actually type (`www`, `browser`) and `description` catches the rest.
    pub fn haystack(&self) -> String {
        let mut text = self.name.clone();
        if !self.description.is_empty() {
            text.push(' ');
            text.push_str(&self.description);
        }
        for keyword in &self.keywords {
            text.push(' ');
            text.push_str(keyword);
        }
        text
    }
}

/// The XDG directories holding `.desktop` files, most specific first so a user override shadows the system one.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(util::paths::xdg_data_home().join("applications"));
    let system = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    dirs.extend(
        system
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|d| PathBuf::from(d).join("applications")),
    );
    dirs
}

/// Strips the `Exec` field codes the spec defines (`%f`, `%U`, `%i`, `%c`, `%k`, …). They stand for files or URLs to open, and launching with them left in passes the literal `%U` to the program as an argument.
fn clean_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|word| !(word.len() == 2 && word.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses a desktop entry's `[Desktop Entry]` group. Returns `None` for anything not worth showing: a non-application type, `NoDisplay`, `Hidden`, or an entry with no name or no command.
fn parse_entry(id: &str, text: &str) -> Option<App> {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the main group; the `[Desktop Action …]` groups that follow describe extra launch entries.
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // Localised keys (`Name[es]`) are ignored in favour of the plain one; picking the right locale is the shell's `locale` service's job and not worth a second parse here.
            fields.entry(key.trim()).or_insert_with(|| value.trim());
        }
    }

    if fields.get("Type").is_some_and(|t| *t != "Application") {
        return None;
    }
    if fields.get("NoDisplay").is_some_and(|v| *v == "true")
        || fields.get("Hidden").is_some_and(|v| *v == "true")
    {
        return None;
    }
    let name = fields.get("Name")?.to_string();
    let exec = clean_exec(fields.get("Exec")?);
    if name.is_empty() || exec.is_empty() {
        return None;
    }

    let split = |key: &str| -> Vec<String> {
        fields
            .get(key)
            .map(|v| {
                v.split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    Some(App {
        id: id.to_string(),
        name,
        description: fields
            .get("GenericName")
            .or_else(|| fields.get("Comment"))
            .unwrap_or(&"")
            .to_string(),
        icon: fields.get("Icon").unwrap_or(&"").to_string(),
        exec,
        categories: split("Categories"),
        keywords: split("Keywords"),
        terminal: fields.get("Terminal").is_some_and(|v| *v == "true"),
    })
}

/// Scans `dirs` for desktop entries. Earlier directories win, which is what makes `~/.local/share/applications` override a system entry of the same id.
fn scan(dirs: &[PathBuf]) -> Vec<App> {
    let mut found: HashMap<String, App> = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if found.contains_key(id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = parse_entry(id, &text) {
                found.insert(id.to_string(), app);
            }
        }
    }
    let mut apps: Vec<App> = found.into_values().collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// How often the application directories are fingerprinted. Installing software is a human-paced event, so the interval is set by how long a user will tolerate the launcher not knowing about what they just installed, not by how fast the directories can change.
const WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// A [`Store`] rather than a [`Service`](util::broadcast::Service): the list is seeded synchronously on first read, so a launcher opened a millisecond after start gets the applications instead of an empty list it would have to wait for. The watcher below is what makes it live.
static APPS: Store<Vec<App>> = Store::new(|| scan(&application_dirs()));

/// Every installed application, sorted by name. Scanned on first call, and kept current from there.
pub fn all() -> Vec<App> {
    ensure_watching();
    APPS.get()
}

/// Registers `tx` for the list, sending the current one immediately — for a surface that stays up across an install (an app browser) rather than reading the list once when it opens.
pub fn subscribe(tx: EventSender<Vec<App>>) {
    ensure_watching();
    APPS.subscribe(tx);
}

/// Re-scans the application directories — for after installing software, and for the IPC `apps reload`.
pub fn reload() -> usize {
    APPS.update(|apps| *apps = scan(&application_dirs())).len()
}

static WATCHER: OnceLock<()> = OnceLock::new();

/// Starts the directory watcher, once per process. Lazy for the same reason every service is: a shell with no launcher and no app browser never asks for the list, and should not pay for a thread watching it.
fn ensure_watching() {
    WATCHER.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("hogar-shell-apps-watch".to_string())
            .spawn(watch);
    });
}

fn watch() {
    let dirs = application_dirs();
    let mut last = fingerprint(&dirs);
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let current = fingerprint(&dirs);
        if current != last {
            last = current;
            reload();
        }
    }
}

/// A cheap stand-in for "have the entries changed": every `.desktop` file's name, size and modification time, combined so the order `read_dir` happens to yield them in doesn't matter.
///
/// Directory mtimes alone would be the obvious choice and are the wrong one here. On a store-based distribution a profile's `applications` directory is a symlink into an immutable store where every path carries the same zeroed timestamp, so switching generations — installing software — changes no mtime the shell can see. Listing the entries does catch it, and a few hundred `stat` calls every few seconds costs well under a millisecond.
fn fingerprint(dirs: &[PathBuf]) -> u64 {
    let mut total: u64 = 0;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut hash: u64 = meta.len() ^ modified.rotate_left(32);
            for byte in entry.file_name().as_encoded_bytes() {
                hash = hash.wrapping_mul(31).wrapping_add(*byte as u64);
            }
            total = total.wrapping_add(hash);
        }
    }
    total
}

/// Launches `app`, detached from the shell.
///
/// Detaching matters: a child of the shell would die with it, and would inherit its file descriptors. `setsid` puts the program in its own session so neither happens. A terminal entry is wrapped in the configured terminal, since running it bare would give it no tty.
pub fn launch(app: &App) {
    let mut command = app.exec.clone();
    if app.terminal {
        let terminal = config::config()
            .map(|c| c.app_command(config::HelperApp::Terminal))
            .unwrap_or_else(|| "xterm".to_string());
        command = format!("{terminal} -e {command}");
    }
    crate::state::record_launch(&app.id);
    run_detached(command);
}

/// Launching a desktop entry is launching a program, and the config's own hooks want the same thing, so the spawn lives in `util` and this is the name the app-launching code already reaches for.
pub use util::process::run_detached;

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Type=Application
Name=Firefox
GenericName=Web Browser
Comment=Browse the web
Exec=firefox %u
Icon=firefox
Terminal=false
Categories=Network;WebBrowser;
Keywords=Internet;WWW;Browser;

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window
";

    #[test]
    fn a_desktop_entry_parses_into_a_launchable_app() {
        let app = parse_entry("firefox", FIREFOX).expect("a normal entry parses");
        assert_eq!(app.name, "Firefox");
        assert_eq!(
            app.description, "Web Browser",
            "GenericName wins over Comment"
        );
        assert_eq!(app.icon, "firefox");
        assert_eq!(app.exec, "firefox", "the %u field code is stripped");
        assert_eq!(app.keywords, vec!["Internet", "WWW", "Browser"]);
        assert!(!app.terminal);
    }

    #[test]
    fn the_action_groups_after_the_main_one_are_ignored() {
        // Without stopping at the group boundary, `Name=New Window` would overwrite the app's own name.
        let app = parse_entry("firefox", FIREFOX).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.exec, "firefox");
    }

    #[test]
    fn entries_that_should_not_be_listed_are_dropped() {
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(parse_entry("x", hidden).is_none());

        let link = "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n";
        assert!(parse_entry("x", link).is_none(), "only applications launch");

        let no_exec = "[Desktop Entry]\nType=Application\nName=X\n";
        assert!(parse_entry("x", no_exec).is_none(), "nothing to run");

        let no_name = "[Desktop Entry]\nType=Application\nExec=x\n";
        assert!(parse_entry("x", no_name).is_none(), "nothing to show");
    }

    #[test]
    fn field_codes_are_stripped_but_real_arguments_survive() {
        assert_eq!(clean_exec("firefox %u"), "firefox");
        assert_eq!(clean_exec("env FOO=1 mpv --fs %F"), "env FOO=1 mpv --fs");
        assert_eq!(
            clean_exec("wine start /unix %f"),
            "wine start /unix",
            "only two-character %x tokens are field codes"
        );
        assert_eq!(clean_exec("prog --width=100%"), "prog --width=100%");
    }

    #[test]
    fn the_haystack_covers_the_words_users_actually_type() {
        let app = parse_entry("firefox", FIREFOX).unwrap();
        let haystack = app.haystack().to_lowercase();
        for word in ["firefox", "web browser", "www"] {
            assert!(haystack.contains(word), "'{word}' is searchable");
        }
    }

    #[test]
    fn a_user_entry_shadows_the_system_one_of_the_same_id() {
        let root = std::env::temp_dir().join(format!("hogar-shell-apps-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(
            user.join("editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=My Editor\nExec=mine\n",
        )
        .unwrap();
        std::fs::write(
            system.join("editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=System Editor\nExec=theirs\n",
        )
        .unwrap();

        let apps = scan(&[user, system]);
        assert_eq!(apps.len(), 1, "one id, one entry");
        assert_eq!(apps[0].name, "My Editor", "the user's copy wins");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scanning_a_missing_directory_is_not_fatal() {
        assert!(scan(&[PathBuf::from("/nonexistent-applications")]).is_empty());
        assert_eq!(
            fingerprint(&[PathBuf::from("/nonexistent-applications")]),
            0
        );
    }

    #[test]
    fn the_fingerprint_moves_when_an_entry_is_installed_or_edited() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = [dir.clone()];
        let empty = fingerprint(&dirs);

        let entry = dir.join("editor.desktop");
        std::fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nName=A\nExec=a\n",
        )
        .unwrap();
        let installed = fingerprint(&dirs);
        assert_ne!(installed, empty, "a new entry is a change");

        // Same name, same second, different length — the size is what catches an edit inside one tick.
        std::fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nName=A Longer Name\nExec=a\n",
        )
        .unwrap();
        assert_ne!(fingerprint(&dirs), installed, "an edited entry is a change");

        // A file the scan would not read cannot move the fingerprint either, or every icon cache write in the directory would re-scan the world.
        let before = fingerprint(&dirs);
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        assert_eq!(fingerprint(&dirs), before);

        std::fs::remove_file(&entry).unwrap();
        assert_eq!(fingerprint(&dirs), empty, "removing it puts the list back");

        std::fs::remove_dir_all(&dir).ok();
    }
}
