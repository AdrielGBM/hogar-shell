//! The wallpaper library and which image is on which screen.
//!
//! Two questions, one owner. **What is available** is a recursive scan of `[paths] wallpapers`, with a thumbnail cache so a grid of two hundred images does not decode two hundred full-resolution photographs. **What is showing** is a per-output assignment that outlives a restart, because a wallpaper picked at random or chosen from a grid is state the shell owns, not a preference the user hand-edited into `config.toml` — the same split every other runtime toggle follows.
//!
//! Resolution order for one screen, most specific first: the runtime per-output choice, the runtime global one, `[background.monitors]`, `[background] image`. A user who pinned an image in their config still sees it until something sets one at runtime, and `hogar-shell wallpaper clear` puts them back.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use ::image::{DynamicImage, ImageFormat, ImageResult};
use platform_wayland::EventSender;

use crate::state;
use config::{Config, WallpaperConfig};
use util::broadcast::Store;
use util::paths;

/// One image in the library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// The file name without its extension — what a grid puts under the thumbnail.
    pub name: String,
    /// The folder it was found in, relative to the library root; empty at the top level. What a "browse by folder" view groups on (K9).
    pub folder: String,
}

/// How often the library is re-fingerprinted. A wallpaper collection changes when a human adds a file to it, so the interval is set by how long that human will wait to see it, not by how fast a directory can change.
const WATCH_INTERVAL: Duration = Duration::from_secs(10);

fn settings() -> WallpaperConfig {
    config::shared_config()
        .map(|config| config.wallpaper.clone())
        .unwrap_or_default()
}

fn library_dir() -> PathBuf {
    config::shared_config()
        .map(|config| config.wallpaper_dir())
        .unwrap_or_else(|| paths::user_dir("XDG_PICTURES_DIR", "Pictures").join("Wallpapers"))
}

/// Walks `root` for images, deepest-last and capped.
///
/// Iterative rather than recursive: a symlink loop in a picture folder is not exotic, and a recursive walk would meet it with a stack overflow instead of the cap. The cap is what bounds it either way — `visited` would need canonical paths and a set, which is more machinery than "stop after `max_entries`" earns.
fn scan(root: &Path, config: &WallpaperConfig) -> Vec<Entry> {
    let mut found = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    let cap = config.max_entries.max(1) as usize;
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= cap {
                break;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if config.recursive {
                    queue.push(path);
                }
                continue;
            }
            if !config.accepts(&path) {
                continue;
            }
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            let folder = path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
                .map(|relative| relative.to_string_lossy().to_string())
                .unwrap_or_default();
            found.push(Entry { path, name, folder });
        }
        if found.len() >= cap {
            break;
        }
    }
    found.sort_by(|a, b| {
        (a.folder.to_lowercase(), a.name.to_lowercase())
            .cmp(&(b.folder.to_lowercase(), b.name.to_lowercase()))
    });
    found
}

/// A [`Store`] rather than a `Service`: the library is seeded on first read, so a grid opened a millisecond after start gets the images instead of an empty page it has to wait for. The watcher below is what keeps it current while that grid is open.
static LIBRARY: Store<Vec<Entry>> = Store::new(|| {
    let config = settings();
    if config.enabled {
        scan(&library_dir(), &config)
    } else {
        Vec::new()
    }
});

/// Every wallpaper in the library, sorted by folder then name.
pub fn all() -> Vec<Entry> {
    ensure_watching();
    LIBRARY.get()
}

/// Registers `tx` for the library, sending the current one immediately — for a grid that stays up while the folder is being filled.
pub fn subscribe_library(tx: EventSender<Vec<Entry>>) {
    ensure_watching();
    LIBRARY.subscribe(tx);
}

/// Re-scans the library folder. Returns how many images it holds.
pub fn reload() -> usize {
    let config = settings();
    let dir = library_dir();
    LIBRARY
        .update(|library| {
            *library = if config.enabled {
                scan(&dir, &config)
            } else {
                Vec::new()
            }
        })
        .len()
}

static WATCHER: OnceLock<()> = OnceLock::new();

fn ensure_watching() {
    WATCHER.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("hogar-shell-wallpapers".to_string())
            .spawn(watch);
    });
}

fn watch() {
    let mut last = fingerprint(&library_dir(), &settings());
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let config = settings();
        if !config.enabled {
            continue;
        }
        let current = fingerprint(&library_dir(), &config);
        if current != last {
            last = current;
            reload();
        }
    }
}

/// A cheap stand-in for "has the collection changed": every listed image's name, size and mtime, combined so the order `read_dir` yields them in does not matter. Directory mtimes alone would miss a store-based distribution, for the same reason the application scanner does not trust them.
fn fingerprint(root: &Path, config: &WallpaperConfig) -> u64 {
    let mut total: u64 = 0;
    for entry in scan(root, config) {
        let Ok(meta) = std::fs::metadata(&entry.path) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut hash: u64 = meta.len() ^ modified.rotate_left(32);
        for byte in entry.path.as_os_str().as_encoded_bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(*byte as u64);
        }
        total = total.wrapping_add(hash);
    }
    total
}

/// The image `output` should be painting, or `None` for the theme's base colour.
///
/// The one resolution order, so the surface, the scheme extractor and `hogar-shell wallpaper get` cannot disagree about which image is showing.
pub fn current_image(config: &Config, output: Option<&str>) -> Option<PathBuf> {
    let state = state::get();
    let chosen = output
        .and_then(|name| state.wallpaper_monitors.get(name).cloned())
        .or_else(|| state.wallpaper.clone())
        .or_else(|| config.background.image_for(output).cloned());
    chosen.map(|path| paths::expand_tilde(&path))
}

/// What every wallpaper surface listens to: the assignment changed, and here is the whole of it.
///
/// Published as the full map rather than as one output's path so a surface can tell "my screen changed" from "another one did" without a second lookup, and so a `clear` reaches every screen in one message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assignment {
    pub global: Option<PathBuf>,
    pub monitors: std::collections::HashMap<String, PathBuf>,
}

impl Assignment {
    /// The runtime choice for `output`, before the config fallbacks. `None` means "nothing set at runtime".
    pub fn for_output(&self, output: Option<&str>) -> Option<&PathBuf> {
        output
            .and_then(|name| self.monitors.get(name))
            .or(self.global.as_ref())
    }
}

static ASSIGNED: Store<Assignment> = Store::new(|| {
    let state = state::get();
    Assignment {
        global: state.wallpaper.clone(),
        monitors: state.wallpaper_monitors.clone(),
    }
});

pub fn assignment() -> Assignment {
    ASSIGNED.get()
}

/// Registers `tx` for runtime wallpaper changes. Pass to `platform_wayland::watch` from a wallpaper surface.
pub fn subscribe(tx: EventSender<Assignment>) {
    ASSIGNED.subscribe(tx);
}

/// Sets the wallpaper — for one output when `output` names one, for every screen otherwise.
///
/// Setting the global one clears the per-output overrides on purpose: "set this wallpaper" means all of them, and a screen quietly keeping its old picture would read as the command having half worked.
pub fn set(path: &Path, output: Option<&str>) {
    let path = paths::expand_tilde(path);
    match output {
        Some(name) => {
            let name = name.to_string();
            let value = path.clone();
            state::update(move |s| {
                s.wallpaper_monitors.insert(name, value);
            });
        }
        None => {
            let value = path.clone();
            state::update(move |s| {
                s.wallpaper = Some(value);
                s.wallpaper_monitors.clear();
            });
        }
    }
    publish();
}

/// Drops the runtime choice, putting `[background]` back in charge.
pub fn clear(output: Option<&str>) {
    match output {
        Some(name) => {
            let name = name.to_string();
            state::update(move |s| {
                s.wallpaper_monitors.remove(&name);
            });
        }
        None => state::update(|s| {
            s.wallpaper = None;
            s.wallpaper_monitors.clear();
        }),
    }
    publish();
}

fn publish() {
    let state = state::get();
    ASSIGNED.update(|assigned| {
        *assigned = Assignment {
            global: state.wallpaper.clone(),
            monitors: state.wallpaper_monitors.clone(),
        }
    });
}

/// A decoded wallpaper, ready for a surface to paint without touching the disk on the frame.
#[derive(Clone)]
pub struct Frame {
    pub path: PathBuf,
    pub image: std::sync::Arc<telar::ImageData>,
}

/// How often a parked producer checks that the surface it feeds is still there.
///
/// Not a poll for state — the producer waits on the store for that. It is how a thread whose surface was torn down by a config reload learns to stop, since the only liveness signal a `watch` channel offers is a failed send. Without it a shell reloaded fifty times would hold fifty parked threads until the next wallpaper change happened to reap them.
const LIVENESS: Duration = Duration::from_secs(5);

/// The producer a wallpaper surface hands to `platform_wayland::watch`: waits for the runtime choice to change, decodes what `output` should now be showing, and delivers it ready to draw. `None` is the liveness heartbeat and means nothing changed.
///
/// Decoding here rather than in the consumer is the whole point — a full-resolution JPEG takes long enough that doing it on the driver thread would drop frames on every other surface at exactly the moment the user is watching the wallpaper change.
pub fn frames(
    output: Option<String>,
    painted: Option<PathBuf>,
) -> impl FnOnce(EventSender<Option<Frame>>) + Send + 'static {
    move |tx| {
        let (changes, rx) = std::sync::mpsc::channel();
        ASSIGNED.listen(changes);
        // Seeded with what the surface already drew at build time, so the immediate first delivery — the current assignment — decodes nothing and the shell does not cross-fade an image into itself.
        let mut showing = painted;
        loop {
            match rx.recv_timeout(LIVENESS) {
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if !tx.send(None) {
                        return;
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
            let Some(config) = config::shared_config() else {
                continue;
            };
            let Some(path) = current_image(&config, output.as_deref()) else {
                showing = None;
                continue;
            };
            if showing.as_deref() == Some(path.as_path()) {
                continue;
            }
            let Some(image) = util::picture::decode(&path) else {
                tracing::warn!("wallpaper '{}' could not be loaded", path.display());
                continue;
            };
            showing = Some(path.clone());
            if !tx.send(Some(Frame {
                path,
                image: std::sync::Arc::new(image),
            })) {
                return;
            }
        }
    }
}

/// Picks a wallpaper from the library at random, avoiding the one already showing when there is a choice.
///
/// Deterministic randomness is not wanted here and a PRNG crate would be one dependency for one call: the nanoseconds since the epoch are as unpredictable as this needs to be, and repeating a picture occasionally is not a bug worth a dependency.
pub fn random(exclude: Option<&Path>) -> Option<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    choose(&all(), exclude, nanos).map(|entry| entry.path.clone())
}

/// The choice `random` makes, with the roll passed in so the rule can be tested without one.
fn choose<'a>(library: &'a [Entry], exclude: Option<&Path>, roll: usize) -> Option<&'a Entry> {
    let others: Vec<&Entry> = library
        .iter()
        .filter(|entry| Some(entry.path.as_path()) != exclude)
        .collect();
    // A library of one is the case where excluding what is up leaves nothing: repeating it beats doing nothing.
    let choices = if others.is_empty() {
        library.iter().collect()
    } else {
        others
    };
    choices.get(roll % choices.len().max(1)).copied()
}

/// The cached thumbnail for `source`, generating it on first ask.
///
/// Keyed by path *and* mtime, so replacing an image in place shows the new one rather than the stale thumbnail of what used to be there. Callers run this off the UI thread: a cache miss decodes and rescales a full-size photograph.
pub fn thumbnail(source: &Path, size: u32) -> Option<PathBuf> {
    let size = clamp_size(size);
    let cached = thumbnail_path(source, size);
    if cached.exists() {
        return Some(cached);
    }
    let image = ::image::open(source).ok()?;
    match store(&image.thumbnail(size, size), &cached) {
        Ok(()) => Some(cached),
        Err(e) => {
            tracing::warn!("thumbnail for {}: {e}", source.display());
            None
        }
    }
}

/// Encodes `thumb` in the format `path`'s extension names and puts it there whole.
///
/// Written with [`util::fs::write_atomic`] rather than `DynamicImage::save`, which truncates the target before it encodes into it: [`thumbnail`] and [`cached_thumbnail`] answer from `exists()`, so a half-written thumbnail would reach the decoder as a finished one, and one a crash cut short would be served that way until its source changed. Directly rather than through the writer's queue, because the name is the source's hash, mtime and size, so there is no order between writes to protect.
fn store(thumb: &DynamicImage, path: &Path) -> ImageResult<()> {
    let format = ImageFormat::from_path(path)?;
    let mut encoded = Cursor::new(Vec::new());
    thumb.write_to(&mut encoded, format)?;
    util::fs::write_atomic(path, encoded.get_ref())?;
    Ok(())
}

/// The thumbnail for `source` only if it has already been generated. What a grid asks before queueing work: a cache hit is one `exists` and can be drawn on the frame it is asked for, where a miss is a full-size decode.
pub fn cached_thumbnail(source: &Path, size: u32) -> Option<PathBuf> {
    let cached = thumbnail_path(source, clamp_size(size));
    cached.exists().then_some(cached)
}

/// Small enough to be pointless, large enough to be the original: neither is a thumbnail.
fn clamp_size(size: u32) -> u32 {
    size.clamp(32, 1024)
}

fn thumbnail_path(source: &Path, size: u32) -> PathBuf {
    let stamp = std::fs::metadata(source)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hash: u64 = 1469598103934665603;
    for byte in source.as_os_str().as_encoded_bytes() {
        hash = (hash ^ *byte as u64).wrapping_mul(1099511628211);
    }
    hash = (hash ^ stamp).wrapping_mul(1099511628211);
    paths::cache_dir()
        .join("wallpapers")
        .join(format!("{hash:016x}-{size}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hogar-shell-wall-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"not really an image").unwrap();
    }

    #[test]
    fn the_scan_finds_images_in_sub_folders_and_ignores_everything_else() {
        let root = temp("scan");
        touch(&root.join("a.png"));
        touch(&root.join("notes.txt"));
        touch(&root.join("nature/b.JPG"));
        touch(&root.join("nature/deep/c.webp"));

        let config = WallpaperConfig::default();
        let found = scan(&root, &config);
        let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a", "b", "c"],
            "sorted by folder then name: {names:?}"
        );
        assert_eq!(found[0].folder, "", "a top-level image has no folder");
        assert_eq!(found[1].folder, "nature");
        assert_eq!(found[2].folder, "nature/deep");

        let flat = scan(
            &root,
            &WallpaperConfig {
                recursive: false,
                ..WallpaperConfig::default()
            },
        );
        assert_eq!(flat.len(), 1, "non-recursive stops at the top level");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_cap_bounds_a_folder_that_is_really_a_picture_archive() {
        let root = temp("cap");
        for index in 0..20 {
            touch(&root.join(format!("{index:02}.png")));
        }
        let found = scan(
            &root,
            &WallpaperConfig {
                max_entries: 5,
                ..WallpaperConfig::default()
            },
        );
        assert_eq!(found.len(), 5);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extensions_are_matched_case_insensitively_and_without_the_dot() {
        let config = WallpaperConfig::default();
        assert!(config.accepts(Path::new("/x/a.PNG")));
        assert!(config.accepts(Path::new("/x/a.jpeg")));
        assert!(!config.accepts(Path::new("/x/a.txt")));
        assert!(!config.accepts(Path::new("/x/noextension")));
        let dotted = WallpaperConfig {
            extensions: vec![".bmp".to_string()],
            ..WallpaperConfig::default()
        };
        assert!(
            dotted.accepts(Path::new("/x/a.bmp")),
            "a user writing '.bmp' means bmp"
        );
    }

    #[test]
    fn scanning_a_missing_folder_is_not_fatal() {
        let config = WallpaperConfig::default();
        assert!(scan(Path::new("/nonexistent-wallpapers"), &config).is_empty());
        assert_eq!(
            fingerprint(Path::new("/nonexistent-wallpapers"), &config),
            0
        );
    }

    #[test]
    fn the_fingerprint_moves_when_an_image_is_added_or_replaced() {
        let root = temp("fingerprint");
        let config = WallpaperConfig::default();
        let empty = fingerprint(&root, &config);

        let image = root.join("a.png");
        touch(&image);
        let one = fingerprint(&root, &config);
        assert_ne!(one, empty);

        // Same name, same second, different length — the size is what catches a replacement inside one tick.
        std::fs::write(&image, b"a rather longer stand-in for an image").unwrap();
        assert_ne!(fingerprint(&root, &config), one);

        std::fs::remove_file(&image).unwrap();
        assert_eq!(fingerprint(&root, &config), empty);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_runtime_choice_wins_over_the_config_and_a_per_output_one_over_both() {
        let assignment = Assignment {
            global: Some(PathBuf::from("/runtime/global.png")),
            monitors: [("DP-1".to_string(), PathBuf::from("/runtime/dp1.png"))]
                .into_iter()
                .collect(),
        };
        assert_eq!(
            assignment.for_output(Some("DP-1")),
            Some(&PathBuf::from("/runtime/dp1.png"))
        );
        assert_eq!(
            assignment.for_output(Some("HDMI-A-1")),
            Some(&PathBuf::from("/runtime/global.png")),
            "a screen with no entry of its own follows the global choice"
        );
        assert_eq!(
            assignment.for_output(None),
            Some(&PathBuf::from("/runtime/global.png"))
        );
        assert_eq!(Assignment::default().for_output(Some("DP-1")), None);
    }

    #[test]
    fn random_avoids_repeating_what_is_already_up_unless_there_is_nothing_else() {
        let entry = |path: &str| Entry {
            path: PathBuf::from(path),
            name: path.to_string(),
            folder: String::new(),
        };
        let library = vec![entry("/a.png"), entry("/b.png"), entry("/c.png")];
        let showing = PathBuf::from("/a.png");
        for roll in 0..12 {
            let chosen =
                choose(&library, Some(&showing), roll).expect("a library of three always answers");
            assert_ne!(
                chosen.path, showing,
                "roll {roll} picked the one already up"
            );
        }

        // A library of one: excluding what is showing empties the list, and doing nothing would read as the command having failed.
        let single = vec![entry("/a.png")];
        assert_eq!(
            choose(&single, Some(&showing), 5).map(|e| e.path.clone()),
            Some(showing)
        );
        assert!(
            choose(&[], None, 0).is_none(),
            "an empty library has no answer"
        );
    }

    /// A cache hit is only an `exists()`, and the cache directory is shared by every shell the user runs, so one can be decoding a thumbnail at the moment another, which missed the cache too, stores the same one again. The store has to replace the file rather than rewrite it: a reader that already opened it keeps the whole image it opened, instead of one truncated and refilled underneath it.
    #[test]
    fn storing_a_thumbnail_again_leaves_a_reader_of_the_first_one_a_whole_image() {
        use std::io::Read;

        let root = temp("thumbnail");
        let cached = root.join("wallpapers").join("0123456789abcdef-320.png");
        store(&DynamicImage::new_rgb8(4, 4), &cached)
            .expect("the first store creates the cache directory");
        let mut reader = std::fs::File::open(&cached).unwrap();

        store(&DynamicImage::new_rgb8(8, 8), &cached).expect("stored again");
        let mut opened = Vec::new();
        reader.read_to_end(&mut opened).unwrap();
        assert_eq!(
            ::image::load_from_memory(&opened)
                .expect("the reader still holds a whole image")
                .width(),
            4,
            "and it is the one it opened"
        );
        assert_eq!(
            ::image::open(&cached).unwrap().width(),
            8,
            "a reader arriving now gets the new one"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
