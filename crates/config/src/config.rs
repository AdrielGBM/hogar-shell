use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use telar::Color;
use toml_edit::{DocumentMut, Item};

use crate::load::{
    GLOBAL_ONLY_SECTIONS, LoadError, SaveError, keep_subtables_with_their_parent, merge_into,
    monitor_config_path,
};
use crate::scheme;
use crate::sections::*;
use crate::theme::NordTheme;
use util::{paths, writer};

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    /// Design-token overrides read from the sibling `tokens.toml`, not from `config.toml` — skipped from serialization so a section save can never write them into the user's config file.
    #[serde(skip)]
    pub tokens: TokenOverrides,
    pub general: GeneralConfig,
    pub bars: BarsConfig,
    pub theme: ThemeConfig,
    pub shape: ShapeConfig,
    pub corners: CornersConfig,
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
    pub modules: HashMap<String, ModuleOverride>,
}

/// One bar per screen edge; empty bars collapse to zero. Default is all-empty by design (serde fills missing fields), so configs get only what they specify — see [`Config::starter`] for the initial setup.
///
/// `excluded_screens` names outputs that get no bars at all — a TV, a projector, a monitor that only ever shows one fullscreen thing. Each entry matches the connector name (`DP-1`) as a `*` pattern, so `HDMI-*` covers a port whose index moves between reboots. *Which* modules a screen shows is a per-monitor config override (`monitors/<output>/config.toml`) rather than a key here: it is the same `[bars.<edge>]` shape, so there is nothing new to learn and nothing to keep in step.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BarsConfig {
    pub excluded_screens: Vec<String>,
    pub top: BarConfig,
    pub bottom: BarConfig,
    pub left: BarConfig,
    pub right: BarConfig,
}

impl BarsConfig {
    pub fn get(&self, edge: Edge) -> &BarConfig {
        match edge {
            Edge::Top => &self.top,
            Edge::Bottom => &self.bottom,
            Edge::Left => &self.left,
            Edge::Right => &self.right,
        }
    }

    /// Whether this output should carry no bars. An output the compositor gave no name to is never excluded — there is nothing to match it by, and dropping the bars off an unnameable screen would look like a bug.
    pub fn excludes(&self, output: Option<&str>) -> bool {
        let Some(output) = output else {
            return false;
        };
        self.excluded_screens
            .iter()
            .any(|pattern| glob_matches(pattern, output))
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct BarConfig {
    pub size: u32,
    pub start: Vec<ModuleEntry>,
    pub center: Vec<ModuleEntry>,
    pub end: Vec<ModuleEntry>,
    pub shape: BarShape,
    /// Keep the bar on screen at all times (the default). Switched off, it reserves no space and sits off its own edge with only `peek` pixels showing, sliding in when the pointer reaches that strip and back out when the pointer leaves.
    pub persistent: bool,
    /// Reveal a non-persistent bar when the pointer reaches its peek strip. Switched off, only a drag inward past `[panels] drag_threshold` brings it in — which is what a touch screen wants, and what a pointer that keeps brushing the screen edge on its way somewhere else does not.
    pub show_on_hover: bool,
    /// How many logical pixels of a hidden bar stay on screen, as the strip that reveals it. Too thin to hit is worse than absent, so this is floored at 1.
    pub peek: u32,
}

impl BarConfig {
    pub fn is_empty(&self) -> bool {
        self.start.is_empty() && self.center.is_empty() && self.end.is_empty()
    }
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            size: 34,
            start: Vec::new(),
            center: Vec::new(),
            end: Vec::new(),
            shape: BarShape::default(),
            persistent: true,
            show_on_hover: true,
            peek: 2,
        }
    }
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

    /// Fresh-install starter config (distinct from `Default`, which is all-empty and backs serde's missing-field fill).
    pub fn starter() -> Self {
        Self {
            tokens: TokenOverrides::default(),
            bars: BarsConfig {
                top: BarConfig {
                    size: 34,
                    start: vec![ModuleEntry::bare("workspaces")],
                    center: vec![ModuleEntry::bare("clock")],
                    end: vec![ModuleEntry::bare("notes")],
                    ..BarConfig::default()
                },
                ..BarsConfig::default()
            },
            theme: ThemeConfig::default(),
            shape: ShapeConfig::default(),
            corners: CornersConfig::default(),
            panels: PanelsConfig::default(),
            popouts: PopoutsConfig::default(),
            icons: IconsConfig::default(),
            stack: StackConfig::default(),
            notifications: NotificationsConfig::default(),
            toasts: ToastsConfig::default(),
            screenshot: ScreenshotConfig::default(),
            recorder: RecorderConfig::default(),
            utilities: UtilitiesConfig::default(),
            sidebar: SidebarConfig::default(),
            background: BackgroundConfig::default(),
            wallpaper: WallpaperConfig::default(),
            active_window: ActiveWindowConfig::default(),
            clock: ClockConfig::default(),
            media: MediaConfig::default(),
            lyrics: LyricsConfig::default(),
            workspaces: WorkspacesConfig::default(),
            launcher: LauncherConfig::default(),
            audio: AudioConfig::default(),
            visualiser: VisualiserConfig::default(),
            brightness: BrightnessConfig::default(),
            temperature: TemperatureConfig::default(),
            battery: BatteryConfig::default(),
            lock_status: LockStatusConfig::default(),
            lock: LockConfig::default(),
            idle: IdleConfig::default(),
            status_icons: StatusIconsConfig::default(),
            network: NetworkConfig::default(),
            bluetooth: BluetoothConfig::default(),
            gpu: GpuConfig::default(),
            weather: WeatherConfig::default(),
            dashboard: DashboardConfig::default(),
            paths: PathsConfig::default(),
            tray: TrayConfig::default(),
            animation: AnimationConfig::default(),
            keynav: KeyNavConfig::default(),
            modules: HashMap::new(),
            general: GeneralConfig::default(),
        }
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

    /// The container variant for a module id, `Default` when it has no `[modules.<id>]` override.
    pub fn variant_for(&self, id: &str) -> Variant {
        self.modules.get(id).map(|m| m.variant).unwrap_or_default()
    }

    /// The accent-token name for a module id: its `[modules.<id>] accent` override, else the global `[theme] accent`; resolve via [`NordTheme::accent_by_name`](crate::NordTheme).
    pub fn accent_name_for(&self, id: &str) -> &str {
        self.modules
            .get(id)
            .and_then(|m| m.accent.as_deref())
            .unwrap_or(&self.theme.accent)
    }

    /// How a module's panel opens when clicked: its `[modules.<id>] open` override, else a drawer — except for the application panels, which have no drawer-sized form (see [`APPLICATION_PANELS`]).
    pub fn open_mode_for(&self, id: &str) -> OpenMode {
        match self.modules.get(id) {
            Some(over) => over.open,
            None if application_panel(id).is_some() => OpenMode::Float,
            None => OpenMode::default(),
        }
    }

    /// How big `id`'s float opens: its `[modules.<id>]` size override, else the global `[panels.float]`.
    ///
    /// Per-module rather than one number for every float because the panels are not one kind of thing. A media float is a card; the settings float is an application with a nav pane down its left-hand side, and a size that suits one makes the other either cramped or mostly empty. The fallback keeps `[panels.float]` meaning what it always did — nothing has to be said per module for a module that does not care.
    pub fn float_size_for(&self, id: &str) -> (u32, u32) {
        let over = self.modules.get(id);
        let (width, height) =
            application_panel(id).unwrap_or((self.panels.float.width, self.panels.float.height));
        (
            over.and_then(|m| m.width).unwrap_or(width),
            over.and_then(|m| m.height).unwrap_or(height),
        )
    }

    /// How tall the settings application's page area is: the surface it opens in, less its header and chrome.
    ///
    /// It has to be a number rather than "the rest of the box" because a scroll area is a layout *leaf* — its content is laid out as its own root, so nothing inside contributes to its height and a viewport with no height of its own measures zero and clips every form away. The same rule the launcher's result list is sized by.
    pub fn settings_page_height(&self) -> f32 {
        let surface = match self.open_mode_for("settings") {
            OpenMode::Float => self.float_size_for("settings").1 as f32,
            OpenMode::Drawer => self.panels.drawer.max_height,
        };
        (surface - SETTINGS_CHROME).max(160.0)
    }

    /// Which zone (start/center/end) a module occupies on `edge`, for deriving its drawer's alignment. A module placed twice answers with the first zone it appears in — the panel is keyed by module id, so there is only one of it to align.
    pub fn zone_of(&self, edge: Edge, module_id: &str) -> Option<Zone> {
        let bar = self.bars.get(edge);
        let holds = |entries: &[ModuleEntry]| entries.iter().any(|m| m.id == module_id);
        if holds(&bar.start) {
            Some(Zone::Start)
        } else if holds(&bar.center) {
            Some(Zone::Center)
        } else if holds(&bar.end) {
            Some(Zone::End)
        } else {
            None
        }
    }

    /// The container variant for a bar entry: its own `variant`, else the module's `[modules.<id>]` override.
    pub fn entry_variant(&self, entry: &ModuleEntry) -> Variant {
        entry.variant.unwrap_or_else(|| self.variant_for(&entry.id))
    }

    /// The accent-token name for a bar entry: its own `accent`, else the module's, else the global one.
    pub fn entry_accent_name<'a>(&'a self, entry: &'a ModuleEntry) -> &'a str {
        match entry.accent.as_deref() {
            Some(accent) => accent,
            None => self.accent_name_for(&entry.id),
        }
    }

    /// Effective shape for edge: per-bar override → global `[shape]` → (for spacing/radius) the theme.
    pub fn shape_for(&self, edge: Edge) -> ResolvedShape {
        let b = &self.bars.get(edge).shape;
        self.shape_from(b.mode, b.gap, b.spacing, b.radius)
    }

    /// The same three-step fallback — what was asked for, then global `[shape]`, then the theme — against overrides that came from somewhere other than `[bars.<edge>.shape]`.
    ///
    /// Taken as four loose options rather than as a struct because the layout model owns the struct these now come from, and `crates/config` is below it: a parameter of that type here would invert the dependency and make the config crate need the model that is built on top of it.
    pub fn shape_from(
        &self,
        mode: Option<Shape>,
        gap: Option<u32>,
        spacing: Option<u32>,
        radius: Option<u32>,
    ) -> ResolvedShape {
        let g = &self.shape;
        ResolvedShape {
            mode: mode.unwrap_or(g.mode),
            gap: gap.unwrap_or(g.gap),
            spacing: spacing
                .or(g.spacing)
                .map(|s| s as f32)
                .unwrap_or_else(|| self.resolve_theme().spacing),
            radius: radius
                .or(g.radius)
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
            match Color::from_hex(hex) {
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

    /// The corner radius for `edge`: per-bar override → global `[shape] radius` → the theme's `radius`.
    pub fn resolved_radius(&self, edge: Edge) -> f32 {
        let b = &self.bars.get(edge).shape;
        b.radius
            .or(self.shape.radius)
            .map(|r| r as f32)
            .unwrap_or_else(|| self.resolve_theme().radius)
    }

    /// The module spacing for `edge`: per-bar override → global `[shape] spacing` → the theme's `spacing`.
    pub fn resolved_spacing(&self, edge: Edge) -> f32 {
        let b = &self.bars.get(edge).shape;
        b.spacing
            .or(self.shape.spacing)
            .map(|s| s as f32)
            .unwrap_or_else(|| self.resolve_theme().spacing)
    }

    /// Whether a bar hugs its edge; frame forces hug, otherwise only at gap == 0.
    pub fn hugs(&self, edge: Edge) -> bool {
        self.shape.frame || self.shape_for(edge).gap == 0
    }

    /// The bar's effective outer gap on `edge`: 0 when it hugs (frame or gap == 0), else its configured gap.
    pub fn edge_gap(&self, edge: Edge) -> u32 {
        if self.hugs(edge) {
            0
        } else {
            self.shape_for(edge).gap
        }
    }

    /// Space the bar reserves from its edge — its outer gap plus thickness — i.e. how far a panel or app must sit from the edge to clear it.
    ///
    /// An auto-hidden bar reserves nothing *of its own*, which is the whole meaning of `persistent = false`: a bar that is not there most of the time must not carve a strip out of every window's idea of the screen. Its peek strip is deliberately not counted either — reserving four pixels would tile every window four pixels short for a sliver the user asked to be able to ignore.
    ///
    /// But the `[shape] frame` ring is not the bar. It is drawn on the background layer, so it is only visible where no window covers it, and it is asked for on *every* edge at once. Reserving nothing at all for an auto-hiding edge tiled the windows straight over that edge's ring — three sides framed and one not. So an auto-hidden edge reserves exactly what it would with no bar on it at all.
    pub fn edge_reserved(&self, edge: Edge) -> u32 {
        if !self.bar_is_persistent(edge) {
            return match self.shape.frame {
                true => self.edge_gap(edge) + self.shape.inactive_size,
                false => 0,
            };
        }
        self.edge_gap(edge) + self.edge_thickness(edge)
    }

    /// Whether `edge`'s bar stays on screen. A bar with nothing on it is persistent by definition — there is no bar to hide — so this only ever answers `false` for an edge that actually carries one.
    pub fn bar_is_persistent(&self, edge: Edge) -> bool {
        !self.edge_present(edge) || self.bars.get(edge).persistent
    }

    /// How many logical pixels of a hidden bar stay on screen. Floored at 1: a strip the pointer cannot land on is a bar with no way back.
    pub fn bar_peek(&self, edge: Edge) -> u32 {
        self.bars.get(edge).peek.max(1)
    }

    /// The margin an auto-hidden bar sits at on its own anchored edge, so that exactly [`bar_peek`] pixels of it are on screen. Negative, and measured from the screen edge rather than from the bar's usual gap: a bar that is hiding has no gap to keep.
    ///
    /// [`bar_peek`]: Self::bar_peek
    pub fn bar_hidden_offset(&self, edge: Edge) -> i32 {
        self.bar_peek(edge) as i32 - self.edge_thickness(edge) as i32
    }

    /// The gap every panel keeps from the bar and the screen edges: the bar's own outer gap when it floats, so panels float in step with it, else a default so a hugging bar's panels still breathe. This is the "gaps_out"-style spacing that keeps a panel off the bar and off the corners.
    ///
    /// Derived, with no key to override it. There is no third case — a panel at a distance the bar is not at is not a preference, it is the shell losing its spacing.
    pub fn panel_gap(&self, edge: Edge) -> u32 {
        match self.edge_gap(edge) {
            0 => DEFAULT_PANEL_GAP,
            gap => gap,
        }
    }

    /// The corner radius a panel uses: the same as the bar on `edge` (its resolved `radius`, which itself falls back to the theme), so a drawer, float, OSD and notification card all carry the bar's rounding instead of a per-panel value.
    pub fn panel_radius(&self, edge: Edge) -> f32 {
        self.resolved_radius(edge)
    }

    /// The space between two stacked cards — a run of toasts, a run of notification popups.
    ///
    /// The shell's own `spacing` token, which is also what separates two chips on a bar: they are the same question asked one level out, and answering it twice is how two stacks of cards end up with different rhythms for no reason anybody chose. Read from the global `[shape] spacing` rather than a bar's, since a stack hangs off no bar in particular.
    pub fn card_gap(&self) -> f32 {
        self.shape
            .spacing
            .map(|spacing| spacing as f32)
            .unwrap_or_else(|| self.resolve_theme().spacing)
    }

    /// How opaque the shell paints itself, `0.2`–`1` — every bar, panel, card and flash, from `[theme] opacity`. One key rather than one per surface: a shell whose drawer is translucent and whose bar is not is not a preference anybody holds, it is two settings that drifted.
    ///
    /// This is also the half of "a blurred shell" that belongs here. The blur itself is the compositor's — hogar-shell names every surface it opens, so Hyprland can be told to blur them:
    ///
    /// ```text
    /// layer_rule = blur, ^hogar-shell
    /// ```
    ///
    /// Drawing it here instead would mean copying the screen behind every surface each frame and blurring it on the CPU, to reproduce what the compositor is already doing on the GPU. What the compositor cannot do is see through an opaque surface, which is what this key is for: without it the rule above blurs a region nothing shows.
    ///
    /// Floored well above transparent, and clamped rather than trusted: a shell painted at `0` is one whose panels are invisible and whose clicks land on them anyway, which reads as the whole thing being broken. A non-finite value in the file falls back to solid instead of poisoning every colour it touches.
    pub fn opacity(&self) -> f32 {
        if self.theme.opacity.is_finite() {
            self.theme.opacity.clamp(0.2, 1.0)
        } else {
            1.0
        }
    }

    /// The background every panel paints: the theme's surface token at the shell's opacity.
    pub fn panel_fill(&self) -> Color {
        self.resolve_theme().surface.with_alpha(self.opacity())
    }

    /// A panel's margin `(top, right, bottom, left)` off the screen edges: uniformly the [`panel_gap`](Self::panel_gap). A panel surface uses `exclusive_zone = 0`, so the compositor already positions it past every bar's reserved zone (the reservation strip's exclusive zone); the panel only adds the standard gap beyond that — re-adding the bar's thickness here would double the distance off the bar. The one distance rule every panel shares, so a drawer, an OSD and a notification stack all clear the bar by the same config-controlled gap.
    pub fn panel_margin(&self, edge: Edge) -> (i32, i32, i32, i32) {
        let g = self.panel_gap(edge) as i32;
        (g, g, g, g)
    }

    /// Thickness of the surface on edge: bar size if active, inactive_size strip under frame, else 0.
    pub fn edge_thickness(&self, edge: Edge) -> u32 {
        let bar = self.bars.get(edge);
        if !bar.is_empty() {
            bar.size
        } else if self.shape.frame {
            self.shape.inactive_size
        } else {
            0
        }
    }

    pub fn edge_present(&self, edge: Edge) -> bool {
        self.edge_thickness(edge) > 0
    }

    /// The edge that owns corner (horizontal preferred over vertical); None if neither is active.
    pub fn corner_owner(&self, corner: Corner) -> Option<Edge> {
        let h = corner.horizontal_edge();
        let v = corner.vertical_edge();
        if self.edge_present(h) {
            Some(h)
        } else if self.edge_present(v) {
            Some(v)
        } else {
            None
        }
    }

    /// Corner modules for edge's leading and trailing ends (routed via start/end zones, no separate surfaces).
    pub fn corner_modules_for(&self, edge: Edge) -> (Option<&str>, Option<&str>) {
        let (lead, trail) = edge.corners();
        let owned = |c: Corner| {
            if self.corner_owner(c) == Some(edge) {
                self.corners.get(c)
            } else {
                None
            }
        };
        (owned(lead), owned(trail))
    }

    /// Whether the bar surface is fully opaque, which is what lets it be cleared to a solid colour and declared non-transparent to the compositor. Only a hugging `bar` at full opacity, with no frame, qualifies.
    ///
    /// **Both exclusions are things that used to be invisible.** A bar below full opacity cleared to a solid colour is a bar whose opacity does nothing — the clear paints over what the alpha was for. And a framed bar paints no background of its own at all, because the frame's ring already covers the strip; clearing it solid puts that background back and darkens where the two overlap.
    pub fn bar_surface_opaque(&self, edge: Edge) -> bool {
        if self.shape.frame || self.opacity() < 1.0 {
            return false;
        }
        let s = self.shape_for(edge);
        s.mode == Shape::Bar && s.gap == 0 && s.radius == 0.0
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
    /// A merge rather than a replacement, so a per-monitor file says only what differs — a vertical bar on the second screen is four lines, not a copy of the whole config that then drifts. Tables merge key by key; anything else (a scalar, an array, a module list) replaces outright, because a half-overridden array is not something a user can predict.
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

    /// Persists a single `[name]` section back to `config.toml`, replacing just that table while preserving every other section, key order, and comment in the file (format-preserving via `toml_edit`). `value` is a section struct such as [`ThemeConfig`]. Creates the file and its parent directory if missing. The running shell's config watcher then hot-reloads the change, so a save applies live.
    ///
    /// The file itself is replaced by [`util::writer`], which stages a whole copy and renames it into place: the user's hand-written config is the one file in the shell that cannot be regenerated, and a truncating write that died half way through would take their comments and every section this function promises to preserve with it. The wait for the writer is what keeps the `Result` meaningful — a caller that reports a failed save has to be told about one, and the settings panel's forms do.
    ///
    /// Hands back the file as it found it and as it left it (see [`Saved`]).
    pub fn save_section<T: Serialize>(
        path: &Path,
        name: &str,
        value: &T,
    ) -> Result<Saved, SaveError> {
        let read = std::fs::read_to_string(path).ok();
        let mut doc = read
            .as_deref()
            .unwrap_or_default()
            .parse::<DocumentMut>()
            .map_err(SaveError::Parse)?;
        let rendered = toml::to_string(value).map_err(SaveError::Serialize)?;
        let section = rendered.parse::<DocumentMut>().map_err(SaveError::Parse)?;
        let mut table = section.as_table().clone();
        // Carry over the existing header's decor (its leading comment) so replacing the table keeps the section's surrounding comments, not just its values.
        if let Some(existing) = doc.get(name).and_then(Item::as_table) {
            *table.decor_mut() = existing.decor().clone();
        }
        doc.insert(name, Item::Table(table));
        keep_subtables_with_their_parent(&mut doc);
        let written = doc.to_string();
        writer::write(path, written.clone().into_bytes()).map_err(SaveError::Io)?;
        Ok(Saved { read, written })
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
