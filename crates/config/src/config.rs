use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use telar::Color;
use toml_edit::{DocumentMut, Item};

use crate::load::{
    GLOBAL_ONLY_SECTIONS, LoadError, SaveError, keep_subtables_with_their_parent, merge_into,
    monitor_config_path, update_table,
};
use crate::scheme;
use crate::sections::*;
use crate::theme::{NordTheme, OPACITY_RANGE};
use util::{paths, writer};

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    /// Design-token overrides read from the sibling `tokens.toml`, not from `config.toml` — skipped from serialization so a section save can never write them into the user's config file.
    #[serde(skip)]
    pub tokens: TokenOverrides,
    pub general: GeneralConfig,
    pub theme: ThemeConfig,
    pub shape: ShapeConfig,
    pub panels: PanelsConfig,
    pub popouts: PopoutsConfig,
    pub icons: IconsConfig,
    pub stack: StackConfig,
    pub notifications: NotificationsConfig,
    pub toasts: ToastsConfig,
    pub screenshot: ScreenshotConfig,
    pub recorder: RecorderConfig,
    pub utilities: UtilitiesConfig,
    pub sidebar: SidebarConfig,
    pub background: BackgroundConfig,
    pub wallpaper: WallpaperConfig,
    pub active_window: ActiveWindowConfig,
    pub clock: ClockConfig,
    pub media: MediaConfig,
    pub lyrics: LyricsConfig,
    pub workspaces: WorkspacesConfig,
    pub windows: WindowsConfig,
    pub dock: DockConfig,
    pub launcher: LauncherConfig,
    pub audio: AudioConfig,
    pub visualiser: VisualiserConfig,
    pub brightness: BrightnessConfig,
    pub temperature: TemperatureConfig,
    pub battery: BatteryConfig,
    pub lock_status: LockStatusConfig,
    pub lock: LockConfig,
    pub idle: IdleConfig,
    pub status_icons: StatusIconsConfig,
    pub network: NetworkConfig,
    pub bluetooth: BluetoothConfig,
    pub gpu: GpuConfig,
    pub weather: WeatherConfig,
    pub dashboard: DashboardConfig,
    pub paths: PathsConfig,
    pub tray: TrayConfig,
    pub animation: AnimationConfig,
    pub keynav: KeyNavConfig,
    pub automation: AutomationConfig,
    pub rules: Vec<RuleConfig>,
    pub modules: HashMap<String, ModuleOverride>,
}

impl Config {
    /// How long a wallpaper transition actually runs, after `[animation]` has had its say. Zero when animation is off or the transition is `none`, which is what makes "wait for the picture to settle" a no-op there instead of a pause with nothing happening in it.
    pub fn wallpaper_transition(&self) -> Duration {
        if self.background.transition == WallpaperTransition::None {
            return Duration::ZERO;
        }
        self.animation.duration(Duration::from_millis(
            self.background.transition_ms.min(10_000),
        ))
    }

    /// The mode and variant a dynamic scheme is generated at.
    ///
    /// `auto` resolves through the fallback palette rather than to a hardcoded dark: a user whose fallback is Catppuccin Latte has already said which end of the ramp they live at, and asking them to say it twice is how the two settings end up disagreeing.
    pub fn scheme_selection(&self) -> (scheme::Mode, scheme::Variant) {
        let mode = self
            .theme
            .requested_mode()
            .unwrap_or_else(|| scheme::Mode::of(&NordTheme::named(&self.theme.fallback)));
        (mode, self.theme.requested_variant())
    }

    /// The config a fresh install is seeded with. Every default: what the shell draws where is the layout's, and the built-in layout is what a fresh install shows.
    pub fn starter() -> Self {
        Self::default()
    }

    /// The command for a well-known helper application: `[general.apps]`, else the legacy `[general] terminal` for the terminal, else the built-in fallback. Resolved here rather than at each call site so every affordance that opens a real application agrees on which one that is.
    pub fn app_command(&self, which: HelperApp) -> String {
        let apps = &self.general.apps;
        let configured = match which {
            HelperApp::Terminal => &apps.terminal,
            HelperApp::FileManager => &apps.file_manager,
            HelperApp::AudioMixer => &apps.audio_mixer,
            HelperApp::MediaPlayer => &apps.media_player,
            HelperApp::Browser => &apps.browser,
            HelperApp::Editor => &apps.editor,
        };
        let configured = configured.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        if which == HelperApp::Terminal && !self.general.terminal.trim().is_empty() {
            return self.general.terminal.trim().to_string();
        }
        which.fallback().to_string()
    }

    /// The wallpaper library. Falls back to a `Wallpapers` folder inside the user's own pictures directory.
    pub fn wallpaper_dir(&self) -> PathBuf {
        self.resolved_path(&self.paths.wallpapers, || {
            paths::user_dir("XDG_PICTURES_DIR", "Pictures").join("Wallpapers")
        })
    }

    /// Where local `.lrc` files are looked up. Not a user-content directory by convention, so it defaults inside the shell's own data directory rather than inventing a folder in `$HOME`.
    pub fn lyrics_dir(&self) -> PathBuf {
        self.resolved_path(&self.paths.lyrics, || paths::data_dir().join("lyrics"))
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.resolved_path(&self.paths.recordings, || {
            paths::user_dir("XDG_VIDEOS_DIR", "Videos").join("Recordings")
        })
    }

    pub fn screenshot_dir(&self) -> PathBuf {
        self.resolved_path(&self.paths.screenshots, || {
            paths::user_dir("XDG_PICTURES_DIR", "Pictures").join("Screenshots")
        })
    }

    /// A directory searched before the shell's built-in assets, when one is configured.
    pub fn assets_dir(&self) -> Option<PathBuf> {
        let configured = self.paths.assets.trim();
        (!configured.is_empty()).then(|| paths::expand_tilde(Path::new(configured)))
    }

    fn resolved_path(&self, configured: &str, fallback: impl FnOnce() -> PathBuf) -> PathBuf {
        let configured = configured.trim();
        if configured.is_empty() {
            return fallback();
        }
        paths::expand_tilde(Path::new(configured))
    }

    /// The effective UI language (BCP-47 tag): the `[general] language` override, else the shipped locale that best matches the environment's, else the catalog's default. Each surface applies it via `telar::set_locale` when it builds.
    pub fn language(&self) -> String {
        let configured = self.general.language.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        let catalog = &crate::__rsx_i18n::CATALOG;
        telar::negotiate_locale(
            &telar::system_locales_from_env(),
            catalog.locales,
            catalog.default_locale,
        )
        .to_string()
    }

    /// How `module` is presented: its `[modules.<id>]` table with an instance's own `options` over it (TA-2), so a chip, the panel it opens and the card it shows on hover are all dressed and sized by one answer.
    pub fn presentation(&self, module: &str, options: &toml::Table) -> ModuleOverride {
        let defaults = self.modules.get(module).cloned().unwrap_or_default();
        crate::options::overlaid(&defaults, options, crate::fields::presentation_keys())
    }

    /// The accent-token name a module drawn with `presentation` uses: its own, else the global `[theme] accent`; resolve via [`NordTheme::accent_by_name`](crate::NordTheme).
    pub fn accent_name<'a>(&'a self, presentation: &'a ModuleOverride) -> &'a str {
        presentation.accent.as_deref().unwrap_or(&self.theme.accent)
    }

    /// A shape from what an area of the layout writes on itself, with the theme under whatever it leaves unset.
    ///
    /// Taken as four loose options rather than as a struct because the layout model owns the struct these now come from, and `crates/config` is below it: a parameter of that type here would invert the dependency and make the config crate need the model that is built on top of it.
    pub fn shape_from(
        &self,
        mode: Option<Shape>,
        gap: Option<u32>,
        spacing: Option<u32>,
        radius: Option<u32>,
    ) -> ResolvedShape {
        ResolvedShape {
            mode: mode.unwrap_or_default(),
            gap: gap.unwrap_or(0),
            spacing: spacing
                .map(|s| s as f32)
                .unwrap_or_else(|| self.resolve_theme().spacing),
            radius: radius
                .map(|r| r as f32)
                .unwrap_or_else(|| self.resolve_theme().radius),
        }
    }

    /// The palette `[theme] name` selects, before any override: the wallpaper's for `dynamic`, else the built-in — switched to its light or dark sibling when `[theme] mode` asks for one it has.
    ///
    /// A dynamic theme with nothing extracted yet resolves to `[theme] fallback` rather than to a blank or a hardcoded default, so the first frame after an install is already the palette the user asked to fall back to instead of a colour scheme they never chose.
    fn base_palette(&self, t: &ThemeConfig) -> NordTheme {
        if t.is_dynamic() {
            return scheme::theme().unwrap_or_else(|| Self::in_requested_mode(t, &t.fallback));
        }
        Self::in_requested_mode(t, &t.name)
    }

    fn in_requested_mode(t: &ThemeConfig, name: &str) -> NordTheme {
        match t.requested_mode() {
            Some(mode) => NordTheme::named(NordTheme::in_mode(name, mode)),
            None => NordTheme::named(name),
        }
    }

    /// The theme this config selects, with every `[theme]` override applied — accent, numeric tokens, and per-token `[theme.colors]` hex. The single place a theme is resolved, so its tokens back the config defaults everywhere.
    pub fn resolve_theme(&self) -> NordTheme {
        self.theme_with(&self.theme)
    }

    /// The palette a `[theme]` section *would* produce, without adopting it.
    ///
    /// The settings application's swatches and its preview both need to draw a selection the user has made but not saved, and resolving one at the call site would be a second copy of the rules below — the accent lookup, the light/dark sibling, `dynamic`'s fallback, `[theme.colors]`, `tokens.toml`. Hence a parameter rather than a helper next to the picker, which is the convention the rest of this file keeps.
    pub fn theme_with(&self, t: &ThemeConfig) -> NordTheme {
        let mut theme = self.base_palette(t).with_accent(&t.accent);
        if let Some(r) = t.radius {
            theme.radius = r as f32;
        }
        if let Some(s) = t.spacing {
            theme.spacing = s as f32;
        }
        if let Some(f) = t.font_size {
            theme.font_size = f;
        }
        if let Some(i) = t.icon_size {
            theme.icon_size = i;
        }
        if let Some(s) = t.icon_stroke {
            theme.icon_stroke = Some(s);
        }
        theme.fonts = t.fonts;
        for (name, hex) in &t.colors {
            match crate::theme::parse_hex(hex) {
                Some(c) => theme = theme.with_color(name, c),
                None => tracing::warn!("theme color '{name}': invalid hex '{hex}'"),
            }
        }
        // Last, and over the absolute overrides above: a scale means "relative to the size I chose", so applying it first would leave a pinned token unscaled and the two settings disagreeing.
        if !t.scale.is_identity() {
            theme.radius *= ScaleConfig::factor(t.scale.rounding);
            theme.spacing = (theme.spacing * ScaleConfig::factor(t.scale.spacing)).round();
            theme.font_size *= ScaleConfig::factor(t.scale.font);
            theme.icon_size *= ScaleConfig::factor(t.scale.icon);
        }
        // `tokens.toml` reaches past the supported `[theme]` surface, so it is applied after everything else and always wins — a user editing raw tokens has said which answer they want.
        if !self.tokens.is_empty() {
            self.tokens.apply(&mut theme);
        }
        theme
    }

    /// How far an area of `shape` stands off its edge: nothing under `[shape] frame`, whose ring every bar is part of, and its own gap otherwise.
    pub fn gap_of(&self, shape: &ResolvedShape) -> u32 {
        match self.shape.frame {
            true => 0,
            false => shape.gap,
        }
    }

    /// The space between two stacked cards — a run of toasts, a run of notification popups.
    ///
    /// The shell's own `spacing` token, which is also what separates two chips on a bar: they are the same question asked one level out, and answering it twice is how two stacks of cards end up with different rhythms for no reason anybody chose. The theme's rather than a bar's, since a stack hangs off no bar in particular.
    pub fn card_gap(&self) -> f32 {
        self.resolve_theme().spacing
    }

    /// How opaque the shell paints itself, `0.2`–`1` — every bar, panel, card and flash, from `[theme] opacity`. One key rather than one per surface: a shell whose drawer is translucent and whose bar is not is not a preference anybody holds, it is two settings that drifted.
    ///
    /// This is also the half of "a blurred shell" that belongs here: the blur is the compositor's, asked for per area through `ext-background-effect-v1`, and it cannot see through an opaque paint.
    ///
    /// Floored well above transparent, and clamped rather than trusted: a shell painted at `0` is one whose panels are invisible and whose clicks land on them anyway, which reads as the whole thing being broken. A non-finite value in the file falls back to solid instead of poisoning every colour it touches.
    pub fn opacity(&self) -> f32 {
        if self.theme.opacity.is_finite() {
            self.theme
                .opacity
                .clamp(*OPACITY_RANGE.start(), *OPACITY_RANGE.end())
        } else {
            1.0
        }
    }

    /// The background every panel paints: the theme's surface token at the shell's opacity.
    pub fn panel_fill(&self) -> Color {
        self.resolve_theme().surface.with_alpha(self.opacity())
    }

    /// Reads and parses `config.toml`, writing the starter config on a fresh install. Parse failures are returned rather than swallowed — a typo must not silently replace a user's whole setup with the starter bar, which is what discarding the error would do — and the caller decides what runs instead: a reload keeps the last config that loaded, and startup, which has none, falls back to the starter config; both say why.
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        Self::load_or_seed(path).map(|(config, _)| config)
    }

    /// [`load`](Self::load), also handing back the starter config's text when `config.toml` was missing and the load wrote it there — `None` when the file was read, or when the starter could not be written.
    ///
    /// For the caller that stamps what it applied (see [`crate::fingerprint::Stamp::record`]). A fingerprint taken before the load names a missing file, and the starter the load leaves behind is a change against it, so a fresh install's first look at its own starter config would reload everything and say "config reloaded". These are the bytes that file holds, fingerprinted without a second read for another writer to land in.
    pub fn load_or_seed(path: &Path) -> Result<(Self, Option<String>), LoadError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::starter();
                let written = cfg.write_to(path);
                return Ok((cfg, written));
            }
            Err(e) => return Err(LoadError::Io(e)),
        };
        let document: toml::Value = toml::from_str(&text).map_err(LoadError::Parse)?;
        let mut config: Config = document.try_into().map_err(LoadError::Parse)?;
        config.tokens = TokenOverrides::load(path);
        Ok((config, None))
    }

    /// The config as `output` sees it: `config.toml` with `monitors/<output>/config.toml` deep-merged over it.
    ///
    /// A merge rather than a replacement, so a per-monitor file says only what differs rather than being a copy of the whole config that then drifts. Tables merge key by key; anything else (a scalar, an array) replaces outright, because a half-overridden array is not something a user can predict.
    ///
    /// Sections in [`GLOBAL_ONLY_SECTIONS`] are dropped from the override without a word here: one process owns them, so honouring them per monitor would be a setting that silently did nothing on every screen but one. Saying so is the config report's job — it names the section at the line the override sets it, once, where a log line here would repeat it at every merge.
    pub fn for_output(path: &Path, output: Option<&str>) -> Result<Self, LoadError> {
        let Some(output) = output else {
            return Config::load(path);
        };
        let Ok(override_text) = std::fs::read_to_string(monitor_config_path(path, output)) else {
            return Config::load(path);
        };
        let base_text = std::fs::read_to_string(path).map_err(LoadError::Io)?;
        let mut merged: toml::Value = toml::from_str(&base_text).map_err(LoadError::Parse)?;
        let mut over: toml::Value = toml::from_str(&override_text).map_err(LoadError::Parse)?;
        if let Some(table) = over.as_table_mut() {
            for section in GLOBAL_ONLY_SECTIONS {
                table.remove(*section);
            }
        }
        merge_into(&mut merged, over);
        let mut config: Config = merged.try_into().map_err(LoadError::Parse)?;
        config.tokens = TokenOverrides::load(path);
        Ok(config)
    }

    /// Where a monitor's override lives: `<config dir>/monitors/<output>/config.toml`.
    pub fn monitor_dir(path: &Path) -> PathBuf {
        path.parent().unwrap_or(Path::new(".")).join("monitors")
    }

    /// Every monitor override's path beside the config at `path`, one per directory under [`monitor_dir`](Self::monitor_dir), in name order so whoever lists them lists them the same way every time.
    ///
    /// Whether one is there to read is the caller's question, because each asks it differently: the fingerprint keeps a file with bytes in it, and the report keeps one it can open.
    pub fn monitor_overrides(path: &Path) -> Vec<PathBuf> {
        let Ok(outputs) = std::fs::read_dir(Self::monitor_dir(path)) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = outputs
            .flatten()
            .map(|output| monitor_config_path(path, output.file_name()))
            .collect();
        files.sort();
        files
    }

    /// Serializes the whole config to `path`, creating its directory, and hands back the text it wrote — `None` when nothing was written. Used only to seed a fresh install; edits to an existing file go through [`save_section`](Self::save_section), which preserves formatting.
    ///
    /// Through [`util::writer`] like every other write the shell makes, even though the file it is seeding cannot yet be half of anything: the writer is the single owner of the path, and a write that went around it could be renamed over by a save the settings panel had already queued.
    fn write_to(&self, path: &Path) -> Option<String> {
        let text = toml::to_string_pretty(self).ok()?;
        match writer::write(path, text.clone().into_bytes()) {
            Ok(()) => Some(text),
            Err(e) => {
                tracing::warn!(
                    "could not write the starter config to {}: {e}",
                    path.display()
                );
                None
            }
        }
    }

    /// [`load`](Self::load) with the starter config as the fallback. For call sites with nothing better to fall back to (a panel building itself, a test); the running shell uses `load` so that a reload can keep the last config that loaded, and so that whether startup parsed decides what it counts as applied.
    pub fn load_or_default(path: &Path) -> Self {
        Config::load(path).unwrap_or_else(|e| {
            tracing::warn!("{e}; using the starter config");
            Config::starter()
        })
    }

    pub fn default_path() -> PathBuf {
        paths::config_dir().join("config.toml")
    }

    /// Persists a single `[name]` section back to `config.toml`, updating that table key by key (changed keys are rewritten, keys the value no longer writes are removed, new ones are added) while preserving every other section, key order, and comment in the file, including those inside the table (format-preserving via `toml_edit`). `value` is a section struct such as [`ThemeConfig`]. Creates the file and its parent directory if missing. The running shell's config watcher then hot-reloads the change, so a save applies live.
    ///
    /// The file itself is replaced by [`util::writer`], which stages a whole copy and renames it into place: the user's hand-written config is the one file in the shell that cannot be regenerated, and a truncating write that died half way through would take their comments and every section this function promises to preserve with it. The wait for the writer is what keeps the `Result` meaningful — a caller that reports a failed save has to be told about one, and the settings panel's forms do.
    ///
    /// Hands back the file as it found it and as it left it (see [`Saved`]).
    pub fn save_section<T: Serialize>(
        path: &Path,
        name: &str,
        value: &T,
    ) -> Result<Saved, SaveError> {
        Self::save_sections(path, &[SectionEdit::new(name, value)?])
    }

    /// [`save_section`](Self::save_section) for several tables in one write, for an edit that spans them: the reload it causes sees all of them or none.
    pub fn save_sections(path: &Path, sections: &[SectionEdit]) -> Result<Saved, SaveError> {
        let read = std::fs::read_to_string(path).ok();
        let mut doc = read
            .as_deref()
            .unwrap_or_default()
            .parse::<DocumentMut>()
            .map_err(SaveError::Parse)?;
        for edit in sections {
            let section = edit
                .rendered
                .parse::<DocumentMut>()
                .map_err(SaveError::Parse)?;
            match doc.get_mut(&edit.name) {
                Some(Item::Table(existing)) => update_table(existing, section.as_table()),
                _ => {
                    doc.insert(&edit.name, Item::Table(section.as_table().clone()));
                }
            }
        }
        keep_subtables_with_their_parent(&mut doc);
        let written = doc.to_string();
        writer::write(path, written.clone().into_bytes()).map_err(SaveError::Io)?;
        Ok(Saved { read, written })
    }
}

/// One `[name]` table, as [`Config::save_sections`] writes it.
pub struct SectionEdit {
    name: String,
    rendered: String,
}

impl SectionEdit {
    pub fn new<T: Serialize>(name: &str, value: &T) -> Result<Self, SaveError> {
        Ok(Self {
            name: name.to_string(),
            rendered: toml::to_string(value).map_err(SaveError::Serialize)?,
        })
    }
}

/// What a [`Config::save_section`] found in the file and what it left there: its one read and its one write.
///
/// For a caller that has to know what its save carried. The save rewrites one table around the file *as it stands*, so an edit made elsewhere a moment earlier goes back to disk inside it — and a caller reading the file again to find out, before the save or after it, is reading at a different moment from the save's own, with room for another writer to land in between. These are the save's own bytes, so there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The file as the save read it, or `None` when it could not be read — missing, most often — which the save treats as empty and replaces.
    pub read: Option<String>,
    /// The file as the save wrote it: the bytes it put on disk, which are there by the time it returns.
    pub written: String,
}
