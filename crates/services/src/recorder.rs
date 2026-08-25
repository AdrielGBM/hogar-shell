//! Recording the screen, by driving a recorder that already exists.
//!
//! Unlike a screenshot, a recording is not something a shell can do itself: it is an encoder, a muxer and a frame pump, and every Wayland session already has one — `wf-recorder` or `gpu-screen-recorder`. So this service owns the *session* rather than the pixels: which backend, what it is recording, since when, and the one thing a wrapper must get right, which is stopping it properly. A recorder killed rather than interrupted leaves an unplayable file, so `stop` sends `SIGINT` and lets the encoder write its own trailer.
//!
//! One process at a time, tracked by pid. The child is owned by a waiter thread rather than by whoever pressed stop, so a recorder that exits on its own — a full disk, a missing codec — updates the shell exactly like one the user stopped.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use platform_wayland::EventSender;
use util::deps::{self, Dep};

use crate::screenshot::Area;
use config::RecorderConfig;
use util::broadcast::Store;

/// The recorders this wrapper knows how to drive, in the order `auto` tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    WfRecorder,
    GpuScreenRecorder,
}

impl Backend {
    pub const ALL: [Backend; 2] = [Backend::WfRecorder, Backend::GpuScreenRecorder];

    /// The declared dependency this backend is, which is what carries its name and its probe.
    pub fn dep(self) -> Dep {
        match self {
            Backend::WfRecorder => Dep::WfRecorder,
            Backend::GpuScreenRecorder => Dep::GpuScreenRecorder,
        }
    }

    pub fn program(self) -> &'static str {
        deps::entry(self.dep())
            .program()
            .expect("a recorder backend is a program")
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|backend| backend.program() == id.trim())
    }

    /// The container each backend writes. `wf-recorder` picks its muxer off the extension, and gpu-screen-recorder defaults to mp4; asking for the one it already wants avoids a re-encode nobody asked for.
    fn extension(self) -> &'static str {
        match self {
            Backend::WfRecorder => "mkv",
            Backend::GpuScreenRecorder => "mp4",
        }
    }

    /// Whether the backend can suspend a recording in place. Only gpu-screen-recorder can (`SIGUSR2`), so the UI asks rather than offering a button that would do nothing.
    pub fn can_pause(self) -> bool {
        matches!(self, Backend::GpuScreenRecorder)
    }

    fn is_installed(self) -> bool {
        deps::available(self.dep())
    }
}

/// What is being recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every screen, as the backend understands "the whole desktop".
    Screen,
    Output(String),
    Area(Area),
}

/// The live recording, or the last one that finished. One value rather than two, because every surface asking about it wants the same three facts: whether it is running, for how long, and where the file is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recording {
    pub active: bool,
    pub paused: bool,
    /// When recording started, in seconds since the epoch.
    pub started_at: Option<u64>,
    /// Seconds spent paused so far, so elapsed time counts recorded seconds rather than wall clock.
    pub paused_for: u64,
    /// When the current pause began, for the same reason.
    paused_since: Option<u64>,
    pub path: Option<PathBuf>,
    pub backend: Option<Backend>,
    pub error: Option<String>,
}

impl Recording {
    /// Recorded seconds so far — of a live recording, or of the one that just finished.
    pub fn elapsed(&self) -> u64 {
        let Some(started) = self.started_at else {
            return 0;
        };
        let paused_now = self
            .paused_since
            .map(|since| now().saturating_sub(since))
            .unwrap_or(0);
        now()
            .saturating_sub(started)
            .saturating_sub(self.paused_for + paused_now)
    }
}

/// `m:ss`, or `h:mm:ss` past an hour — the shape a stopwatch reads in, so a glance at the bar says how long.
pub fn format_elapsed(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

static STATE: Store<Recording> = Store::new(Recording::default);

/// The running recorder's pid, or 0. Kept apart from the [`Child`] on purpose: the child is owned by the waiter thread, so stopping is a signal rather than a lock everyone has to take.
static PID: AtomicI32 = AtomicI32::new(0);

pub fn subscribe(tx: EventSender<Recording>) {
    STATE.subscribe(tx);
}

pub fn current() -> Recording {
    STATE.get()
}

/// The backend a recording would use: the configured one when it is installed, else the first that is. `None` means neither is available, which is what greys the recorder controls out.
pub fn backend(config: &RecorderConfig) -> Option<Backend> {
    match Backend::from_id(&config.backend) {
        Some(wanted) => wanted.is_installed().then_some(wanted),
        None => Backend::ALL.into_iter().find(|b| b.is_installed()),
    }
}

pub fn is_recording() -> bool {
    STATE.get().active
}

/// Starts recording `scope`. A no-op while one is already running — two recorders on one screen would fight over the encoder and produce two half files.
pub fn start(scope: Scope) {
    if is_recording() {
        return;
    }
    let config = config::shared_config()
        .map(|c| c.recorder.clone())
        .unwrap_or_default();
    let dir = config::shared_config()
        .map(|c| c.recordings_dir())
        .unwrap_or_else(|| util::paths::data_dir().join("recordings"));
    let Some(backend) = backend(&config) else {
        fail(telar::t!("recorder.no_backend"));
        return;
    };
    util::paths::ensure_dir(dir.clone());
    let stem = chrono::Local::now().format(&config.file_name).to_string();
    let path = dir.join(format!("{stem}.{}", backend.extension()));
    let args = command_args(backend, &scope, &path, &config);

    let Some(mut recorder) = deps::command(backend.dep()) else {
        fail(format!("{} is not a program", backend.program()));
        return;
    };
    let spawned = recorder
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let child = match spawned {
        Ok(child) => child,
        Err(e) => {
            fail(format!("{}: {e}", backend.program()));
            return;
        }
    };
    PID.store(child.id() as i32, Ordering::Relaxed);
    STATE.update(|state| {
        *state = Recording {
            active: true,
            started_at: Some(now()),
            path: Some(path.clone()),
            backend: Some(backend),
            ..Recording::default()
        };
    });
    let started = telar::t!("recorder.started_title");
    let where_to = telar::t!("recorder.started", file = file_label(&path));
    toast(&started, &where_to);
    if config.notify {
        crate::notifications::notify_local("hogar-shell", &started, &where_to);
    }
    reap(child, config.notify);
}

/// Owns the child and publishes what it did when it exits — whether that was `stop`, a crash, or a full disk. The stderr it wrote is the message, since a recorder's own complaint is more useful than an exit code.
fn reap(mut child: Child, notify: bool) {
    let stderr = child.stderr.take();
    let _ = std::thread::Builder::new()
        .name("hogar-shell-recorder-wait".to_string())
        .spawn(move || {
            let status = child.wait();
            let complaint = stderr.and_then(|mut pipe| {
                use std::io::Read;
                let mut text = String::new();
                pipe.read_to_string(&mut text).ok()?;
                let text = text.trim().to_string();
                (!text.is_empty()).then_some(text)
            });
            PID.store(0, Ordering::Relaxed);
            let failed = !matches!(status, Ok(status) if status.success());
            let finished = STATE.update(|state| {
                state.active = false;
                state.paused = false;
                state.paused_since = None;
                // A recorder interrupted with SIGINT reports a signal exit, which is what a clean stop looks like — so only a complaint on stderr is taken as a failure worth showing.
                state.error = failed
                    .then(|| complaint.clone().unwrap_or_default())
                    .filter(|e| !e.is_empty());
            });
            let (title, body) = match (&finished.error, &finished.path) {
                (Some(error), _) => (telar::t!("recorder.failed_title"), error.clone()),
                (None, Some(path)) => (
                    telar::t!("recorder.saved_title"),
                    telar::t!("recorder.saved", file = file_label(path)),
                ),
                (None, None) => (telar::t!("recorder.saved_title"), String::new()),
            };
            toast(&title, &body);
            if notify {
                crate::notifications::notify_local("hogar-shell", &title, &body);
            }
        });
}

/// Stops the recording the way the encoder expects: `SIGINT`, which is what tells it to write its trailer and close the file. Killing it instead leaves a container with no index — a file that plays nowhere.
pub fn stop() {
    let pid = PID.load(Ordering::Relaxed);
    if pid == 0 {
        return;
    }
    signal(pid, libc::SIGINT);
}

pub fn toggle(scope: Scope) {
    if is_recording() { stop() } else { start(scope) }
}

/// Suspends or resumes a recording in place, on a backend that can. `wf-recorder` cannot, so this says so rather than stopping the recording the user asked to pause.
pub fn toggle_pause() -> Result<bool, String> {
    let state = STATE.get();
    if !state.active {
        return Err(telar::t!("recorder.not_recording"));
    }
    let backend = state
        .backend
        .ok_or_else(|| telar::t!("recorder.not_recording"))?;
    if !backend.can_pause() {
        return Err(telar::t!(
            "recorder.no_pause",
            backend = backend.program().to_string()
        ));
    }
    let pid = PID.load(Ordering::Relaxed);
    if pid == 0 {
        return Err(telar::t!("recorder.not_recording"));
    }
    signal(pid, libc::SIGUSR2);
    let paused = !state.paused;
    STATE.update(|state| {
        state.paused = paused;
        if paused {
            state.paused_since = Some(now());
        } else if let Some(since) = state.paused_since.take() {
            state.paused_for += now().saturating_sub(since);
        }
    });
    Ok(paused)
}

/// The same message as a toast, for the user who wants the acknowledgement without a notification in their history. Gated by `[toasts.events] recording`, which is on by default — a recording that started silently is the one that runs for an hour unnoticed.
fn toast(title: &str, body: &str) {
    let state = STATE.get();
    crate::toaster::post(
        crate::toaster::Event::Recording,
        crate::recorder::glyph(state.active),
        title.to_string(),
        body.to_string(),
    );
}

fn signal(pid: i32, signal: i32) {
    // Safe: `pid` is a process this shell spawned and has not reaped, and both signals are defined for it.
    unsafe { libc::kill(pid, signal) };
}

fn fail(reason: String) {
    tracing::warn!("recorder: {reason}");
    STATE.update(|state| {
        state.active = false;
        state.error = Some(reason.clone());
    });
    crate::notifications::notify_local("hogar-shell", &telar::t!("recorder.failed_title"), &reason);
}

/// The backend's argument list. Split out and tested rather than built inline: this is the whole difference between the two recorders, and getting a flag wrong means a file the user finds out about after the meeting.
fn command_args(
    backend: Backend,
    scope: &Scope,
    path: &Path,
    config: &RecorderConfig,
) -> Vec<String> {
    let file = path.to_string_lossy().to_string();
    match backend {
        Backend::WfRecorder => {
            let mut args = vec![
                "-f".to_string(),
                file,
                "-r".to_string(),
                config.fps().to_string(),
            ];
            match scope {
                Scope::Screen => {}
                Scope::Output(name) => {
                    args.push("-o".to_string());
                    args.push(name.clone());
                }
                Scope::Area(area) => {
                    args.push("-g".to_string());
                    args.push(format!(
                        "{},{} {}x{}",
                        area.x, area.y, area.width, area.height
                    ));
                }
            }
            if config.audio {
                // `--audio=<device>` with a device, bare `--audio` without: passing an empty value makes wf-recorder look for a device literally called "".
                args.push(match config.audio_device.trim() {
                    "" => "--audio".to_string(),
                    device => format!("--audio={device}"),
                });
            }
            args
        }
        Backend::GpuScreenRecorder => {
            let window = match scope {
                Scope::Screen | Scope::Area(_) => "screen".to_string(),
                Scope::Output(name) => name.clone(),
            };
            let mut args = vec![
                "-w".to_string(),
                window,
                "-f".to_string(),
                config.fps().to_string(),
                "-o".to_string(),
                file,
            ];
            if let Scope::Area(area) = scope {
                args.push("-region".to_string());
                args.push(format!(
                    "{}x{}+{}+{}",
                    area.width, area.height, area.x, area.y
                ));
            }
            if config.audio {
                args.push("-a".to_string());
                args.push(match config.audio_device.trim() {
                    "" => "default_output".to_string(),
                    device => device.to_string(),
                });
            }
            args
        }
    }
}

/// One file in the recordings directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub bytes: u64,
    /// Seconds since the epoch, so the list can be ordered and dated without a second stat.
    pub modified: u64,
}

impl Entry {
    pub fn name(&self) -> String {
        file_label(&self.path)
    }

    /// The size as a person reads it — a recording is megabytes, and "18261568" is not an answer.
    pub fn size_label(&self) -> String {
        const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
        let mut size = self.bytes as f64;
        let mut unit = 0;
        while size >= 1024.0 && unit + 1 < UNITS.len() {
            size /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{} {}", self.bytes, UNITS[unit])
        } else {
            format!("{size:.1} {}", UNITS[unit])
        }
    }
}

/// Which extensions count as a recording. Anything else in the directory is the user's — a thumbnail, a note — and a list that offered to delete it would be a list nobody trusts.
const VIDEO: &[&str] = &["mp4", "mkv", "webm", "mov", "avi"];

/// The recordings in `dir`, newest first, capped at `limit`. Read on demand rather than watched: a panel that is closed has no reason to hold an inotify handle, and the directory changes once per recording.
pub fn recordings(dir: &Path, limit: usize) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Entry> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())?;
            if !VIDEO.contains(&extension.as_str()) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some(Entry {
                bytes: meta.len(),
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|since| since.as_secs())
                    .unwrap_or_default(),
                path,
            })
        })
        .collect();
    found.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
    found.truncate(limit);
    found
}

/// Deletes a recording. Restricted to `dir` rather than taking any path: the caller is a UI list, and a delete button that could be handed an arbitrary path is a delete button that eventually is.
pub fn delete(dir: &Path, path: &Path) -> Result<(), String> {
    if path.parent() != Some(dir) {
        return Err(format!(
            "{} is not in the recordings directory",
            path.display()
        ));
    }
    std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Beside the state it reads rather than with the other glyphs, so the toast this service posts and the chip a bar draws take the same answer from one place without the service having to reach up for it.
pub fn glyph(active: bool) -> &'static str {
    if active { "circle-stop" } else { "video" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RecorderConfig {
        RecorderConfig::default()
    }

    #[test]
    fn wf_recorder_is_told_the_file_the_rate_and_the_pixels() {
        let path = Path::new("/tmp/out.mkv");
        let args = command_args(Backend::WfRecorder, &Scope::Screen, path, &config());
        assert_eq!(args[0..2], ["-f", "/tmp/out.mkv"]);
        assert!(
            !args.contains(&"-o".to_string()),
            "the whole desktop names no output"
        );

        let one = command_args(
            Backend::WfRecorder,
            &Scope::Output("DP-1".to_string()),
            path,
            &config(),
        );
        assert!(one.windows(2).any(|pair| pair == ["-o", "DP-1"]), "{one:?}");

        let region = command_args(
            Backend::WfRecorder,
            &Scope::Area(Area {
                x: 10,
                y: 20,
                width: 300,
                height: 200,
            }),
            path,
            &config(),
        );
        assert!(region.contains(&"10,20 300x200".to_string()), "{region:?}");
    }

    #[test]
    fn an_audio_device_is_named_only_when_there_is_one() {
        let path = Path::new("/tmp/out.mkv");
        let on = RecorderConfig {
            audio: true,
            ..config()
        };
        let args = command_args(Backend::WfRecorder, &Scope::Screen, path, &on);
        assert!(
            args.contains(&"--audio".to_string()),
            "no device means the default one, not a device called '': {args:?}"
        );

        let named = RecorderConfig {
            audio: true,
            audio_device: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_string(),
            ..config()
        };
        let args = command_args(Backend::WfRecorder, &Scope::Screen, path, &named);
        assert!(
            args.iter().any(|a| a.starts_with("--audio=alsa_output")),
            "{args:?}"
        );
    }

    #[test]
    fn gpu_screen_recorder_takes_a_window_and_a_region_separately() {
        let path = Path::new("/tmp/out.mp4");
        let args = command_args(
            Backend::GpuScreenRecorder,
            &Scope::Area(Area {
                x: 5,
                y: 6,
                width: 100,
                height: 50,
            }),
            path,
            &config(),
        );
        assert!(
            args.windows(2).any(|pair| pair == ["-w", "screen"]),
            "{args:?}"
        );
        assert!(args.contains(&"100x50+5+6".to_string()), "{args:?}");
        assert!(args.windows(2).any(|pair| pair == ["-o", "/tmp/out.mp4"]));
    }

    #[test]
    fn only_a_backend_that_can_pause_offers_to() {
        assert!(!Backend::WfRecorder.can_pause());
        assert!(Backend::GpuScreenRecorder.can_pause());
        assert_eq!(Backend::from_id("wf-recorder"), Some(Backend::WfRecorder));
        assert_eq!(
            Backend::from_id("auto"),
            None,
            "auto is not a backend, it is a choice"
        );
    }

    #[test]
    fn elapsed_counts_recorded_seconds_not_wall_clock() {
        let live = Recording {
            active: true,
            started_at: Some(now() - 100),
            paused_for: 40,
            ..Recording::default()
        };
        assert_eq!(
            live.elapsed(),
            60,
            "the forty paused seconds are not recorded"
        );

        // Still paused: the current pause counts too, or the readout would keep climbing while nothing is being written.
        let paused = Recording {
            paused: true,
            paused_since: Some(now() - 10),
            ..live
        };
        assert_eq!(paused.elapsed(), 50);
        assert_eq!(Recording::default().elapsed(), 0);
    }

    #[test]
    fn elapsed_reads_as_a_stopwatch() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(9), "0:09");
        assert_eq!(format_elapsed(75), "1:15");
        assert_eq!(format_elapsed(3661), "1:01:01");
    }

    #[test]
    fn the_index_lists_recordings_newest_first_and_leaves_the_rest_alone() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.mkv"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a recording").unwrap();
        let listed = recordings(&dir, 10);
        assert_eq!(listed.len(), 1, "only video files: {listed:?}");
        assert_eq!(listed[0].name(), "a.mkv");

        // A delete is scoped to the directory the list came from.
        let outside = dir.join("..").join("elsewhere.mkv");
        assert!(delete(&dir, &outside).is_err());
        assert!(delete(&dir, &dir.join("a.mkv")).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_size_reads_in_the_unit_a_person_would_use() {
        let entry = |bytes| Entry {
            path: PathBuf::from("/x.mkv"),
            bytes,
            modified: 0,
        };
        assert_eq!(entry(512).size_label(), "512 B");
        assert_eq!(entry(2048).size_label(), "2.0 kB");
        assert_eq!(entry(18_261_568).size_label(), "17.4 MB");
    }
}
