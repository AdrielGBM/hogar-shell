//! The sections that describe the machine the shell runs on, and the helpers it shells out to.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use std::collections::HashMap;
use std::time::Duration;

use platform_wayland::CaptureBackend;
use serde::{Deserialize, Serialize};

/// App-wide settings that don't belong to a specific visual section. `language` is a BCP-47 tag (`"en"`, `"es"`); empty means "follow the OS locale, else English". `show_over_fullscreen` lifts the bars onto the overlay layer so they stay visible over a fullscreen window — off by default, since a fullscreen game or video is normally meant to cover them. `logo` is the icon the `logo` module shows; empty detects the distribution from `/etc/os-release`.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct GeneralConfig {
    pub language: String,
    pub show_over_fullscreen: bool,
    pub logo: String,
    /// The terminal used to run a desktop entry marked `Terminal=true`; empty falls back to `xterm`.
    ///
    /// Superseded by `[general.apps] terminal`, and still read when that one is unset — a config written before the section existed keeps working rather than silently reverting to `xterm`.
    pub terminal: String,
    pub apps: AppsConfig,
}

/// The real applications the shell hands off to (`[general.apps]`).
///
/// Every "open the real thing" affordance — a volume card's mixer button, a recording's folder, a media card opening its player — needs a command, and a shell that guessed one per affordance would be unconfigurable. One section, one command each, resolved through [`Config::app_command`].
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AppsConfig {
    pub terminal: String,
    pub file_manager: String,
    pub audio_mixer: String,
    pub media_player: String,
    pub browser: String,
    pub editor: String,
}

/// A well-known helper application the shell can hand off to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperApp {
    Terminal,
    FileManager,
    AudioMixer,
    MediaPlayer,
    Browser,
    Editor,
}

impl HelperApp {
    /// The command to run when nothing is configured. Each is either the freedesktop indirection that works everywhere (`xdg-open`) or the near-universal tool for the job; a machine without it gets a failed launch and a log line, which is better than an affordance that silently does nothing.
    pub(crate) fn fallback(self) -> &'static str {
        match self {
            Self::Terminal => "xterm",
            Self::FileManager => "xdg-open",
            Self::AudioMixer => "pavucontrol",
            Self::MediaPlayer => "xdg-open",
            Self::Browser => "xdg-open",
            Self::Editor => "xdg-open",
        }
    }
}

/// Backlight control (`[brightness]`): the step a wheel notch or `hogar-shell brightness up` moves. Its own section rather than a key under `[audio]` so the per-output and DDC/CI settings that follow have a home.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct BrightnessConfig {
    pub increment: i32,
    /// Control external monitors over DDC/CI with `ddcutil`, so a desktop with no backlight is dimmable at all. Costs one detection (a few seconds, on the service's own thread) at startup; does nothing when ddcutil is not installed.
    pub external: bool,
}

impl Default for BrightnessConfig {
    fn default() -> Self {
        Self {
            increment: 5,
            external: true,
        }
    }
}

impl BrightnessConfig {
    pub fn step(&self) -> i32 {
        self.increment.clamp(1, 50)
    }
}

/// The network (`[network]`): the wireless list its panel shows.
///
/// `rescan_seconds` is a trade, not a preference: an access point that goes out of range emits nothing, so only a fresh scan notices it left. But a scan takes the radio off its channel, which on a busy link is a visible stutter and on some drivers worse — so the background interval is deliberately slow, and the moment that actually needs a fresh list (opening the panel) triggers a scan of its own. Turn it down only if a stale list bothers you more than the scans do.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct NetworkConfig {
    /// Switches the NetworkManager layer off entirely: no D-Bus connection, no rescan timer, no threads. The bar chip keeps working — it reads sysfs and never needed NetworkManager.
    pub enabled: bool,
    pub rescan_seconds: u32,
    /// Upper bound on the networks the panel lists.
    pub max_networks: u32,
    /// List networks that broadcast no SSID. Off by default: a hidden network cannot be joined by picking it out of a list anyway, so it is a row that can only disappoint.
    pub show_hidden: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rescan_seconds: 300,
            max_networks: 20,
            show_hidden: false,
        }
    }
}

impl NetworkConfig {
    /// Clamped on read, with a floor well above what a config typo could reach: a rescan every few seconds keeps the radio off its channel often enough to hurt the connection it is scanning from.
    pub fn rescan(&self) -> Duration {
        Duration::from_secs(self.rescan_seconds.clamp(60, 3600) as u64)
    }

    pub fn network_limit(&self) -> usize {
        self.max_networks.max(1) as usize
    }
}

/// The weather (`[weather]`).
///
/// `location` is a place name (`"Madrid"`), geocoded once; `latitude`/`longitude` skip that step. With none of them set the service asks an IP-geolocation endpoint where this connection is — which is the only part of the feature that tells a third party anything, and setting either of the others avoids it.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct WeatherConfig {
    pub enabled: bool,
    pub location: String,
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    /// Minutes between refreshes. Clamped on read: the forecast changes hourly, and hammering a free service every few seconds is how a shell gets its users rate-limited.
    pub refresh_minutes: u32,
    pub forecast_days: u32,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            location: String::new(),
            latitude: None,
            longitude: None,
            refresh_minutes: 15,
            forecast_days: 7,
        }
    }
}

impl WeatherConfig {
    /// The configured point, when both halves of it are given. One without the other is not a location, so it falls through to the place name rather than being read as a point on the equator.
    pub fn coordinates(&self) -> Option<crate::policy::Coordinates> {
        Some(crate::policy::Coordinates {
            latitude: self.latitude?,
            longitude: self.longitude?,
        })
    }

    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_minutes.clamp(5, 24 * 60) as u64 * 60)
    }

    /// Open-Meteo serves up to 16 days; asking for none would make the daily block empty.
    pub fn forecast_days(&self) -> u32 {
        self.forecast_days.clamp(1, 16)
    }
}

/// One page of the dashboard. Named in `[dashboard] tabs`, so the ids are the config surface and stay stable regardless of the UI language — the same reason `Condition::id` is not the translated label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DashboardTab {
    #[default]
    Dash,
    Media,
    Performance,
    Weather,
}

impl DashboardTab {
    pub const ALL: [DashboardTab; 4] = [Self::Dash, Self::Media, Self::Performance, Self::Weather];

    pub fn id(self) -> &'static str {
        match self {
            Self::Dash => "dash",
            Self::Media => "media",
            Self::Performance => "performance",
            Self::Weather => "weather",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.id() == id.trim())
    }

    /// The glyph the tab strip draws.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Dash => "layout-dashboard",
            Self::Media => "disc-3",
            Self::Performance => "activity",
            Self::Weather => "cloud-sun",
        }
    }
}

/// The dashboard surface (`[dashboard]`).
///
/// The two intervals are here rather than on the services they read because the cost is the dashboard's, not theirs: the performance cards redraw a sparkline per tick, and the playhead is an MPRIS property no player signals — following it means asking for it. A bar chip's own rate is unaffected either way, and neither ticker exists while the dashboard is closed.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct DashboardConfig {
    /// Which pages the dashboard offers, in order; an id it doesn't know is dropped with a warning rather than failing the whole config parse, which would cost the user every other section over one typo.
    pub tabs: Vec<String>,
    /// Milliseconds between playhead reads while the media card is up.
    pub media_update_interval: u64,
    /// Milliseconds between resource refreshes in the performance cards.
    pub resource_update_interval: u64,
    /// Which column the calendar starts on: `monday`, `sunday` or `saturday`.
    pub first_day_of_week: String,
    /// The image the user card shows. Empty means the usual places — `~/.face`, `~/.face.icon`, then the AccountsService icon a display manager writes.
    pub avatar: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            tabs: DashboardTab::ALL
                .iter()
                .map(|t| t.id().to_string())
                .collect(),
            media_update_interval: 500,
            resource_update_interval: 1000,
            first_day_of_week: "monday".to_string(),
            avatar: String::new(),
        }
    }
}

impl DashboardConfig {
    /// The configured pages, unknown ids warned about and dropped. An empty list falls back to all of them: a dashboard with no pages is a surface that opens onto nothing.
    pub fn tabs(&self) -> Vec<DashboardTab> {
        let resolved: Vec<DashboardTab> = self
            .tabs
            .iter()
            .filter_map(|id| match DashboardTab::from_id(id) {
                Some(tab) => Some(tab),
                None => {
                    tracing::warn!("unknown dashboard tab '{id}'");
                    None
                }
            })
            .collect();
        if resolved.is_empty() {
            return DashboardTab::ALL.to_vec();
        }
        resolved
    }

    /// Clamped on read: below ~100 ms a playhead poll is a D-Bus round-trip per frame for a number that moves one pixel, and above a few seconds the scrubber visibly lags the audio.
    pub fn media_interval(&self) -> Duration {
        Duration::from_millis(self.media_update_interval.clamp(100, 5_000))
    }

    /// Clamped to the resource service's own tick at the fast end — asking more often than it publishes cannot produce a new reading, only more repaints.
    pub fn resource_interval(&self) -> Duration {
        Duration::from_millis(self.resource_update_interval.clamp(1_000, 60_000))
    }

    pub fn first_weekday(&self) -> chrono::Weekday {
        match self.first_day_of_week.trim().to_ascii_lowercase().as_str() {
            "sunday" | "sun" => chrono::Weekday::Sun,
            "saturday" | "sat" => chrono::Weekday::Sat,
            _ => chrono::Weekday::Mon,
        }
    }
}

/// Where the shell reads and writes user content (`[paths]`).
///
/// Every entry is empty by default, meaning "work it out": the wallpaper, screenshot and recording directories resolve through the user's own XDG directories, so they land in `Imágenes/Capturas` on a Spanish desktop rather than in a `Pictures` nobody has. `~` is expanded on read.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PathsConfig {
    pub wallpapers: String,
    pub lyrics: String,
    pub recordings: String,
    pub screenshots: String,
    /// Searched before the shell's built-in assets, so one can be substituted without rebuilding.
    pub assets: String,
}

/// The graphics processor (`[gpu]`).
///
/// `backend` is `auto` (the default), one of `amd` / `intel` / `nvidia`, or `none` to switch the service off entirely. Forcing one matters on a laptop with switchable graphics: the kernel lists the integrated GPU first, and "the first card with a driver we can read" is then the wrong one. `card` names the `drm` entry (`card1`) directly, which is the only way to tell two cards from the same vendor apart.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct GpuConfig {
    pub enabled: bool,
    pub backend: String,
    pub card: String,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: "auto".to_string(),
            card: String::new(),
        }
    }
}

/// Bluetooth (`[bluetooth]`): the chip and the device list its panel shows.
///
/// `scan_on_open` is what makes "pair something new" one gesture instead of two — a panel opened to find a device starts looking, and stops when it closes, so no scan outlives the surface that asked for it. `show_unnamed` is off because a scan in a public place turns up dozens of devices BlueZ knows only an address for, and a list of addresses is not a list anyone can choose from.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct BluetoothConfig {
    pub enabled: bool,
    pub scan_on_open: bool,
    /// Upper bound on the rows the panel lists, so a busy room can't grow the panel past the screen.
    pub max_devices: u32,
    pub show_unnamed: bool,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_on_open: true,
            max_devices: 12,
            show_unnamed: false,
        }
    }
}

impl BluetoothConfig {
    /// At least one row, so a misconfigured cap cannot produce an empty panel with devices behind it.
    pub fn device_limit(&self) -> usize {
        self.max_devices.max(1) as usize
    }
}

/// Whether `text` matches `pattern`, in which `*` stands for any run of characters. Matching ignores case.
///
/// A deliberate subset of a regex: tray applications put a PID or a version in their id (`steam_app_12345`, `chrome_status_icon_1`), which a wildcard covers, and a full regex engine is a dependency this shell does not otherwise carry.
pub(crate) fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let text = text.trim().to_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    // The two anchors must not overlap: `a*b` matches `ab`, but nothing shorter.
    if !text.starts_with(first) || !text.ends_with(last) || first.len() + last.len() > text.len() {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The application launcher: a modal opened by keybind or IPC.
///
/// One entry in the launcher's action mode, declared as a `[[launcher.actions]]` table and reached by typing `>`. `command` runs through `sh -c`, detached, exactly as a desktop entry's `Exec` does.
///
/// `dangerous` marks an action that should not run on a single keystroke — a reboot, a `rm`. Such an action arms on the first Enter and only runs on the second, and it is listed at all only when `enable_dangerous_actions` is on, so a shared or borrowed config can't hand someone a one-key wipe.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct LauncherAction {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub command: String,
    pub enabled: bool,
    pub dangerous: bool,
}

impl Default for LauncherAction {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            icon: "zap".to_string(),
            command: String::new(),
            // Declaring an action is what enables it; the key exists so one can be parked without deleting it.
            enabled: true,
            dangerous: false,
        }
    }
}

impl LauncherAction {
    /// Whether this action should appear at all: it needs something to run, a name to show, and — when it is flagged dangerous — the config's blanket permission for dangerous actions.
    pub fn is_listed(&self, dangerous_allowed: bool) -> bool {
        self.enabled
            && !self.name.trim().is_empty()
            && !self.command.trim().is_empty()
            && (dangerous_allowed || !self.dangerous)
    }
}

/// `fuzzy` off makes the query a plain substring match, for users who find fuzzy matching too loose. `hidden` lists desktop-entry ids to keep out of the results entirely, and `favourites` lists ids to pin above the ranking, in the order given. `actions` are the `>`-prefixed commands of the action mode.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct LauncherConfig {
    pub width: u32,
    pub height: u32,
    pub max_results: u32,
    pub fuzzy: bool,
    /// Show the calculator's answer above the app matches when the query reads as arithmetic. Unit conversions (`3 km in mi`) count as arithmetic.
    pub calculator: bool,
    /// Fall back to `qalc` (Qalculate) for an explicit `=` query the built-in evaluator cannot answer — currencies, constants, dates. Silently does nothing when qalc is not installed.
    pub qalc: bool,
    pub favourites: Vec<String>,
    pub hidden: Vec<String>,
    /// Off by default: an action that can destroy something should take a deliberate opt-in, not arrive with a config someone pasted from the internet.
    pub enable_dangerous_actions: bool,
    pub actions: Vec<LauncherAction>,
    /// Per-application icon overrides, keyed by desktop-entry id (`firefox`), valued as anything [`app_icon_view`] resolves: an icon-theme name or an absolute path.
    ///
    /// For the entries whose `Icon` key names something this machine's icon theme does not have — which is most self-packaged software — where the alternative is editing a `.desktop` file the package manager owns and will overwrite.
    pub icons: HashMap<String, String>,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 420,
            max_results: 12,
            fuzzy: true,
            calculator: true,
            qalc: true,
            favourites: Vec::new(),
            hidden: Vec::new(),
            enable_dangerous_actions: false,
            actions: Vec::new(),
            icons: HashMap::new(),
        }
    }
}

impl LauncherConfig {
    /// The icon reference to draw `id` with: the user's override, else the one the desktop entry declared.
    pub fn icon_for<'a>(&'a self, id: &str, declared: &'a str) -> &'a str {
        self.icons
            .get(id)
            .map(String::as_str)
            .filter(|icon| !icon.trim().is_empty())
            .unwrap_or(declared)
    }
}

/// Screenshots (`[screenshot]`).
///
/// `backend` names the protocol to capture through: `auto` (the default), `image-copy-capture` or `screencopy`. `auto` prefers `ext-image-copy-capture`, the standardised one, and falls back to `wlr-screencopy` on a compositor too old for it. Naming one means "this route or none" — a user who names a backend is usually debugging one, and a silent fallback is what hides the answer.
///
/// `annotator` is the command a saved capture is handed to, with `{file}` where the path goes (appended when the command does not name it): `satty --filename {file}`, `swappy -f`. Empty means the capture is simply saved.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Put every capture on the clipboard, as well as saving it.
    pub copy: bool,
    pub save: bool,
    pub include_cursor: bool,
    /// Hold the last frame on screen while a region is being selected, so a menu or a hover state can be captured without disappearing the moment the overlay takes the pointer.
    pub freeze: bool,
    /// Say where the file went, through the shell's own notification daemon.
    pub notify: bool,
    pub backend: String,
    /// `strftime` pattern for the file's stem; the extension is always `.png`.
    pub file_name: String,
    pub annotator: String,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            copy: true,
            save: true,
            include_cursor: false,
            freeze: true,
            notify: true,
            backend: "auto".to_string(),
            file_name: "screenshot_%Y-%m-%d_%H-%M-%S".to_string(),
            annotator: String::new(),
        }
    }
}

impl ScreenshotConfig {
    /// The route a capture takes. Anything unrecognised reads as `auto`, which is what a config carried over from a build that named a backend this one no longer has should do.
    pub fn backend(&self) -> CaptureBackend {
        match self.backend.trim().to_ascii_lowercase().as_str() {
            "image-copy-capture" => CaptureBackend::ImageCopyCapture,
            "screencopy" => CaptureBackend::Screencopy,
            _ => CaptureBackend::Auto,
        }
    }

    pub fn has_annotator(&self) -> bool {
        !self.annotator.trim().is_empty()
    }
}

/// Screen recording (`[recorder]`).
///
/// `backend` is `auto`, `wf-recorder` or `gpu-screen-recorder`; `auto` takes whichever is installed. Neither is a dependency of the shell — with no recorder present the controls grey out rather than failing on the press.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct RecorderConfig {
    pub backend: String,
    pub audio: bool,
    /// The PipeWire node to record from; empty is the session's default output.
    pub audio_device: String,
    pub fps: u32,
    /// `strftime` pattern for the file's stem; the backend decides the container.
    pub file_name: String,
    pub notify: bool,
    /// How many recordings the list in the utilities panel shows.
    pub max_entries: u32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            backend: "auto".to_string(),
            audio: false,
            audio_device: String::new(),
            fps: 60,
            file_name: "recording_%Y-%m-%d_%H-%M-%S".to_string(),
            notify: true,
            max_entries: 12,
        }
    }
}

impl RecorderConfig {
    /// Clamped to what an encoder will accept: a `0` here would be a recorder that writes no frames, and a four-figure rate is a typo rather than a request.
    pub fn fps(&self) -> u32 {
        self.fps.clamp(1, 240)
    }

    pub fn entries(&self) -> usize {
        self.max_entries.clamp(1, 200) as usize
    }
}

/// The utilities panel (`[utilities]`): the quick toggles it lists, and in which order.
///
/// `toggles` is a list of ids rather than a switch per toggle, because the order is the point — the toggles a user reaches for live at the front. Unknown ids are dropped with a warning rather than failing the config, so a name from a newer build costs a line in the log instead of the whole panel.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct UtilitiesConfig {
    pub toggles: Vec<String>,
    /// Show the capture (screenshot / recorder) card under the toggles.
    pub show_capture: bool,
    /// Show the recordings list under the capture controls.
    pub show_recordings: bool,
    /// How many columns the toggle grid uses.
    pub columns: u32,
    /// How often the window info panel re-captures its preview, in ms. `0` takes one still and leaves it, which is what a machine on battery wants — the preview is a screen capture per refresh.
    pub window_preview_ms: u64,
}

impl Default for UtilitiesConfig {
    fn default() -> Self {
        Self {
            toggles: [
                "wifi",
                "bluetooth",
                "mic",
                "dnd",
                "game_mode",
                "vpn",
                "idle_inhibit",
                "settings",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            show_capture: true,
            show_recordings: true,
            columns: 4,
            window_preview_ms: 1000,
        }
    }
}

impl UtilitiesConfig {
    pub fn grid_columns(&self) -> usize {
        self.columns.clamp(1, 8) as usize
    }

    /// The preview's refresh period, or `None` for a single still. Floored well above a frame: each refresh is a compositor round trip and a full-window copy, and asking for it 60 times a second would cost more than the panel showing it.
    pub fn window_preview_interval(&self) -> Option<Duration> {
        (self.window_preview_ms > 0)
            .then(|| Duration::from_millis(self.window_preview_ms.clamp(250, 60_000)))
    }
}

/// Keyboard navigation shared by every list surface (`[keynav]`).
///
/// `vim` is off by default and has to be: the launcher's list sits under a search field, and a list that reads `j` as "down" cannot also let you type `jitsi`. Turning it on is a deliberate trade a vim user makes knowingly — the arrows keep working either way.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default)]
#[serde(default)]
pub struct KeyNavConfig {
    pub vim: bool,
}
