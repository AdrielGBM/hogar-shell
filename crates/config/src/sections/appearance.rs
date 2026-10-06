//! `[theme]`, `[shape]`, `[icons]` and the rest of how the shell looks.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::scheme;
use crate::theme::{FontRole, NordTheme};
use util::paths;
use util::report::{Finding, Report};

/// Background granularity (`Bar`/`Sections`/`Chips`); visual appearance (hug/float/rounding) is controlled by gap/spacing/radius, not mode.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Bar,
    Sections,
    Chips,
}

impl Shape {
    /// Whether a bar in this mode is drawn as one strip: the only mode with a background of its own to edge, lift or curve a fillet out of.
    pub fn draws_strip(self) -> bool {
        matches!(self, Shape::Bar)
    }
}

/// What every bar shares about its shape. A bar's own mode, gap, spacing and radius are its `shape` in the layout; what it leaves unset follows the theme.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ShapeConfig {
    /// Draw the bars as one ring around the screen: each bar fills its strip flat and square, and none floats off its edge.
    pub frame: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedShape {
    pub mode: Shape,
    pub gap: u32,
    /// The bar's own, else the theme's.
    pub spacing: f32,
    /// The bar's own, else the theme's.
    pub radius: f32,
}

impl ResolvedShape {
    pub fn padding(self) -> f32 {
        (self.spacing / 2.0).round()
    }

    /// Chip radius shrunk to nest inside a unit.
    pub fn chip_radius(self) -> f32 {
        (self.radius - self.padding()).max(0.0)
    }
}

/// Container background: `Default` is transparent (blends into the bar, highlights on hover/press); `Filled` paints a solid accent with an auto-contrast foreground.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    #[default]
    Default,
    Filled,
}

/// Where bar icons come from: an Iconify-compatible HTTP endpoint (`{provider}/{set}/{name}.svg`) and the default set applied to a bare icon name. A name may override the set inline as `set:name` (e.g. `mdi:home`), so multiple icon sets work through one endpoint. `provider` is configurable because Iconify is self-hostable/mirrorable.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct IconsConfig {
    pub provider: String,
    pub default_set: String,
    /// The freedesktop icon theme applications' own icons are looked up in — on the tray, in the launcher, on notifications and window chips. Empty follows the GTK settings, falling back to `hicolor`.
    pub app_icon_theme: String,
    /// The shape every application's own icon is cut to: `none` draws it as its theme made it, `circle` and `squircle` give every app the same silhouette. Symbolic glyphs, menu entries and a tray icon recoloured to the bar's ink are never cut.
    pub mask: IconMask,
}

impl Default for IconsConfig {
    fn default() -> Self {
        Self {
            provider: "https://api.iconify.design".to_string(),
            default_set: "lucide".to_string(),
            app_icon_theme: String::new(),
            mask: IconMask::None,
        }
    }
}

/// The shape an application's icon is cut to.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IconMask {
    /// As the icon theme drew it.
    #[default]
    None,
    Circle,
    /// A superellipse: a square with continuously rounded sides.
    Squircle,
}

impl IconMask {
    pub const ALL: [IconMask; 3] = [IconMask::None, IconMask::Circle, IconMask::Squircle];

    pub fn id(self) -> &'static str {
        match self {
            IconMask::None => "none",
            IconMask::Circle => "circle",
            IconMask::Squircle => "squircle",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mask| mask.id() == id.trim())
    }
}

/// The design tokens themselves, overridable from `~/.config/hogar-shell/tokens.toml`.
///
/// **Unstable, and deliberately so.** `[theme]` is the supported surface: it names the handful of knobs a theme is *meant* to expose, and those keys will keep working. This file reaches past that into the token set the shell draws from, which exists to serve the widgets and moves when they do — a token can be renamed or dropped in any release. It is here because a user building a palette wants every number in one place without waiting for each to grow a config key, not because it is a stable API.
///
/// Applied last in [`Config::resolve_theme`], after `[theme]` and after `[theme.scale]`, so it always wins.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TokenOverrides {
    pub radius: Option<f32>,
    pub spacing: Option<f32>,
    pub font_size: Option<f32>,
    pub icon_size: Option<f32>,
    pub icon_stroke: Option<f32>,
    /// Palette tokens by the same names [`NordTheme::accent_by_name`](crate::NordTheme) uses.
    pub colors: HashMap<String, String>,
}

impl TokenOverrides {
    /// Reads `tokens.toml` from the config directory. A missing file is the normal case and reads as "no overrides"; an unparseable one is warned about and ignored, because a token file is a garnish and must never be the reason a shell refuses to start.
    pub fn load(config_path: &Path) -> Self {
        let path = Self::path(config_path);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(tokens) => tokens,
            Err(e) => {
                tracing::warn!("{}: {e}; ignoring the token overrides", path.display());
                Self::default()
            }
        }
    }

    pub fn path(config_path: &Path) -> PathBuf {
        config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("tokens.toml")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.radius.is_none()
            && self.spacing.is_none()
            && self.font_size.is_none()
            && self.icon_size.is_none()
            && self.icon_stroke.is_none()
            && self.colors.is_empty()
    }

    /// Stamps these overrides onto a resolved theme.
    pub(crate) fn apply(&self, theme: &mut NordTheme) {
        if let Some(r) = self.radius {
            theme.radius = r;
        }
        if let Some(s) = self.spacing {
            theme.spacing = s;
        }
        if let Some(f) = self.font_size {
            theme.font_size = f;
        }
        if let Some(i) = self.icon_size {
            theme.icon_size = i;
        }
        if self.icon_stroke.is_some() {
            theme.icon_stroke = self.icon_stroke;
        }
        for (name, hex) in &self.colors {
            match crate::theme::parse_hex(hex) {
                Some(c) => *theme = theme.with_color(name, c),
                None => tracing::warn!("token color '{name}': invalid hex '{hex}'"),
            }
        }
    }
}

/// One text role's overrides (`[theme.fonts.<role>]`), each unset by default so a role keeps the size the theme derives for it.
///
/// No `family`: rsx's `TextStyle` carries no font family — the family is process-wide, applied through `telar::set_default_font_family` from `[theme] font_family`. Per-role families need `TextStyle` to carry one and the renderer to select on it, which is an upstream change rather than a config key.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(default)]
pub struct FontSpec {
    pub size: Option<f32>,
    /// From `100` (thin) to `900` (black): `400` is regular and `700` bold. Unset keeps the role's own.
    pub weight: Option<u16>,
    pub italic: Option<bool>,
}

impl FontSpec {
    /// The size for this role: the override, bounded to what a screen can actually render, else `derived`.
    pub(crate) fn size_for(self, derived: f32) -> f32 {
        self.size
            .filter(|s| s.is_finite())
            .map(|s| s.clamp(4.0, 200.0))
            .unwrap_or(derived)
    }
}

/// Per-role text overrides (`[theme.fonts]`). The roles are the ones the shell actually draws with, so there is no role here that nothing reads.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(default)]
pub struct FontsConfig {
    pub display: FontSpec,
    pub title: FontSpec,
    pub body: FontSpec,
    pub caption: FontSpec,
}

impl FontsConfig {
    pub fn spec(&self, role: FontRole) -> FontSpec {
        match role {
            FontRole::Display => self.display,
            FontRole::Title => self.title,
            FontRole::Body => self.body,
            FontRole::Caption => self.caption,
        }
    }

    pub fn spec_mut(&mut self, role: FontRole) -> &mut FontSpec {
        match role {
            FontRole::Display => &mut self.display,
            FontRole::Title => &mut self.title,
            FontRole::Body => &mut self.body,
            FontRole::Caption => &mut self.caption,
        }
    }
}

/// How the shell moves (`[animation]`).
///
/// Two curve families rather than one, because rsx has two motion models and they answer different questions. `curve` names a **spring**, for motion that chases a target that can move mid-flight — the workspace indicator, which has to bend its path when you hold a workspace key rather than restart. `easing` names a **timing function**, for a transition with a start, an end and a duration — a panel opening, a bar hiding itself.
///
/// `duration_scale` multiplies every duration at once, so "make it all a bit quicker" is one number; `enabled = false` collapses every duration to zero, for a user on a remote desktop who wants no motion at all.
///
/// `reduced` is the accessibility answer: while motion is reduced, a panel, drawer or popout that would slide in fades instead, a bar that hides itself moves at once, and no transition runs longer than 100 ms. `auto` follows the desktop's own reduced-motion setting, read from the desktop portal; with no portal, motion is as configured here.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct AnimationConfig {
    pub enabled: bool,
    pub duration_scale: f32,
    /// The spring preset for continuous motion: `gentle`, `snappy` or `bouncy`.
    pub curve: String,
    /// The timing function for duration-based transitions: `linear`, `ease-in`, `ease-out`, `ease-in-out`.
    pub easing: String,
    /// How long a panel takes to enter or leave, before `duration_scale`.
    pub panel_duration_ms: u64,
    /// How long a bar that hides itself takes to slide away or back, before `duration_scale`.
    pub autohide_duration_ms: u64,
    /// Whether motion is reduced: `auto` follows the desktop's reduced-motion setting, `on` and `off` decide regardless of it.
    pub reduced: ReducedMotion,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_scale: 1.0,
            curve: "gentle".to_string(),
            easing: "ease-out".to_string(),
            panel_duration_ms: 180,
            autohide_duration_ms: 160,
            reduced: ReducedMotion::Auto,
        }
    }
}

/// Whether the shell reduces its motion.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReducedMotion {
    /// As the desktop's reduced-motion setting says.
    #[default]
    Auto,
    /// Always reduced.
    On,
    /// Never reduced.
    Off,
}

impl ReducedMotion {
    pub const ALL: [ReducedMotion; 3] =
        [ReducedMotion::Auto, ReducedMotion::On, ReducedMotion::Off];

    pub fn id(self) -> &'static str {
        match self {
            ReducedMotion::Auto => "auto",
            ReducedMotion::On => "on",
            ReducedMotion::Off => "off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reduced| reduced.id() == id.trim())
    }

    pub fn applies(self, desktop_reduced: bool) -> bool {
        match self {
            ReducedMotion::Auto => desktop_reduced,
            ReducedMotion::On => true,
            ReducedMotion::Off => false,
        }
    }
}

impl AnimationConfig {
    /// The longest any transition runs while motion is reduced.
    pub const REDUCED_CEILING: Duration = Duration::from_millis(100);

    /// The multiplier, bounded: `0` (or a negative, or NaN) would make every animation instant by accident rather than by the `enabled` switch that says so, and an unbounded one makes the shell feel broken.
    fn scale(&self) -> f32 {
        if self.duration_scale.is_finite() {
            self.duration_scale.clamp(0.1, 10.0)
        } else {
            1.0
        }
    }

    /// Whether motion is reduced now: `reduced`, with `auto` answered by the desktop.
    pub fn is_reduced(&self) -> bool {
        self.reduced
            .applies(crate::motion::desktop_prefers_reduced())
    }

    /// `base` scaled by `duration_scale` and held to [`Self::REDUCED_CEILING`] while motion is reduced, or zero while animation is off. The one place a duration is derived, so every surface shortens and lengthens together instead of each carrying its own constant.
    pub fn duration(&self, base: Duration) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }
        let scaled = base.mul_f32(self.scale());
        match self.is_reduced() {
            true => scaled.min(Self::REDUCED_CEILING),
            false => scaled,
        }
    }

    /// The spring every chase-a-moving-target animation uses.
    pub fn spring(&self) -> telar::motion::Spring {
        match self.curve.trim().to_ascii_lowercase().as_str() {
            "snappy" => telar::motion::Spring::snappy(),
            "bouncy" => telar::motion::Spring::bouncy(),
            _ => telar::motion::Spring::gentle(),
        }
    }

    /// The timing function every duration-based transition uses.
    pub fn easing(&self) -> telar::motion::Easing {
        match self.easing.trim().to_ascii_lowercase().as_str() {
            "linear" => telar::motion::Easing::Linear,
            "ease-in" | "ease_in" => telar::motion::Easing::EaseIn,
            "ease-in-out" | "ease_in_out" => telar::motion::Easing::EaseInOut,
            _ => telar::motion::Easing::EaseOut,
        }
    }

    /// A panel's enter/exit transition, ready to hand to `Animated`.
    pub fn panel_tween(&self) -> telar::motion::Tween {
        self.tween_ms(self.panel_duration_ms, 2_000)
    }

    /// How a bar that hides itself slides away and back: at once while motion is reduced.
    pub fn autohide_tween(&self) -> telar::motion::Tween {
        self.travel_tween_ms(self.autohide_duration_ms, 2_000)
    }

    /// A tween of `base_ms`, scaled and eased by `[animation]`, and bounded by `max_ms` so a mistyped duration is a slow transition rather than one that never ends. The general form `panel_tween` is a preset of.
    pub fn tween_ms(&self, base_ms: u64, max_ms: u64) -> telar::motion::Tween {
        telar::motion::tween(
            self.duration(Duration::from_millis(base_ms.clamp(0, max_ms))),
            self.easing(),
        )
    }

    /// [`Self::tween_ms`] for something that travels across the screen rather than fading in place, which reduced motion makes instant.
    pub fn travel_tween_ms(&self, base_ms: u64, max_ms: u64) -> telar::motion::Tween {
        match self.is_reduced() {
            true => telar::motion::tween(Duration::ZERO, self.easing()),
            false => self.tween_ms(base_ms, max_ms),
        }
    }
}

/// Proportional multipliers over the theme's numeric tokens (`[theme.scale]`).
///
/// An absolute override answers "what should the radius be"; a scale answers "make everything a bit rounder", which is the question a user actually has and the one that keeps a palette's proportions intact. Applied last in [`Config::resolve_theme`], so scaling a token the user also pinned scales *their* number, not the palette's — otherwise the two settings would silently fight.
///
/// `font` scales the base size every other role steps off, so one number moves all the text at once.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct ScaleConfig {
    pub rounding: f32,
    pub spacing: f32,
    pub font: f32,
    pub icon: f32,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            rounding: 1.0,
            spacing: 1.0,
            font: 1.0,
            icon: 1.0,
        }
    }
}

impl ScaleConfig {
    /// Whether every multiplier is the identity, so `resolve_theme` can skip the whole step — and so a config that never mentions scaling reads exactly as it did before the section existed.
    pub(crate) fn is_identity(self) -> bool {
        [self.rounding, self.spacing, self.font, self.icon]
            .iter()
            .all(|f| *f == 1.0)
    }

    /// A multiplier bounded away from the two ways it breaks a surface: `0` (or negative, or NaN) collapses what it scales to nothing, and an unbounded one grows a chip past the screen it sits on.
    pub(crate) fn factor(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(0.25, 4.0)
        } else {
            1.0
        }
    }
}

/// Theme selection and overrides. `name` picks a built-in palette, `custom`, or `dynamic` (a palette generated from the current wallpaper); the rest override individual tokens on top of it — numbers directly, `[theme.scale]` proportionally, and `[theme.colors]` per-token hex (`base = "#2e3440"`), keyed by the same names [`NordTheme::accent_by_name`] uses. Any unset field keeps the built-in's value.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ThemeConfig {
    pub name: String,
    /// One of the palette's hues by name (`blue`, `cyan`, `teal`, `red`, `orange`, `yellow`, `green`, `purple`), an opaque `#rrggbb` (the `#` and six digits; `bad`, `add` and `#ff880080` are refused), or empty for the palette's own accent.
    pub accent: String,
    /// `dark`, `light`, or `auto` (the default) to keep whatever the named palette already is. A built-in with a sibling in the asked-for mode switches to it (`gruvbox` ↔ `gruvbox-light`); one without keeps its own.
    pub mode: String,
    /// How much colour a `dynamic` scheme carries: `vibrant` (the default), `content`, `expressive`, `fidelity` or `muted`. Ignored by the built-in palettes, which carry their own.
    pub variant: String,
    /// The palette a `dynamic` theme falls back to before a wallpaper has been quantised — on the very first start, or with no wallpaper set at all.
    pub fallback: String,
    pub export: SchemeExportConfig,
    pub radius: Option<u32>,
    pub spacing: Option<u32>,
    pub font_size: Option<f32>,
    pub icon_size: Option<f32>,
    /// Font family the whole shell renders in (must be installed). Unset keeps the renderer's default. Applied process-wide via [`telar::set_default_font_family`], not carried in the (`Copy`) theme struct.
    pub font_family: Option<String>,
    /// Stroke width forced on stroke-based icon glyphs (e.g. `1.5`). Unset keeps each glyph's own stroke.
    pub icon_stroke: Option<f32>,
    /// How opaque every surface the shell paints is, from `0.2` to `1.0` — bars, panels, cards and flashes alike. One key for the whole shell and no way to break it apart: a drawer at an opacity the bar it hangs off does not share is not a look anybody chooses, it is two settings that drifted.
    ///
    /// **This is the half `backdrop = "blur"` cannot supply.** An area asks for blur behind it through `ext-background-effect-v1` (see [Compositor rules](../guides/compositor-rules.md) for what moved off per-namespace compositor rules), and it shows nothing through a surface painted opaque. Lowering this is what gives it something to blur.
    pub opacity: f32,
    pub scale: ScaleConfig,
    pub fonts: FontsConfig,
    pub colors: HashMap<String, String>,
}

/// Where the resolved palette is written for the rest of the desktop to read (`[theme.export]`).
///
/// A wallpaper-driven scheme is only worth having if the applications around the shell follow it, and none of them reads `config.toml`. Each switch writes one flat file of the same tokens into `dir`: `scheme.json`, `scheme.css` (GTK `@define-color`), `scheme.conf` (an ini for Qt/Kvantum themes) and `scheme.sh` plus `sequences` (shell variables and the OSC escapes that recolour a running terminal). Once the files are on disk the shell raises `colors_changed`, which is where a `[[rules]]` entry running `gsettings` or `makoctl reload` belongs.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct SchemeExportConfig {
    pub enabled: bool,
    /// Where the files land. Empty means the shell's own config directory, next to `config.toml`.
    pub dir: String,
    pub json: bool,
    pub gtk: bool,
    pub qt: bool,
    pub terminal: bool,
}

impl Default for SchemeExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: String::new(),
            json: true,
            gtk: true,
            qt: false,
            terminal: false,
        }
    }
}

impl SchemeExportConfig {
    pub fn resolved_dir(&self) -> PathBuf {
        let configured = self.dir.trim();
        if configured.is_empty() {
            return Config::default_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
        }
        paths::expand_tilde(Path::new(configured))
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "nord".to_string(),
            accent: "cyan".to_string(),
            mode: "auto".to_string(),
            variant: "vibrant".to_string(),
            fallback: "nord".to_string(),
            export: SchemeExportConfig::default(),
            radius: None,
            spacing: None,
            font_size: None,
            icon_size: None,
            font_family: None,
            icon_stroke: None,
            opacity: 1.0,
            scale: ScaleConfig::default(),
            fonts: FontsConfig::default(),
            colors: HashMap::new(),
        }
    }
}

impl ThemeConfig {
    /// Whether this config asks for a wallpaper-derived palette rather than a built-in one.
    pub fn is_dynamic(&self) -> bool {
        self.name.trim().eq_ignore_ascii_case(scheme::DYNAMIC)
    }

    /// The mode asked for, or `None` for `auto` — "keep whatever the palette already is".
    pub fn requested_mode(&self) -> Option<scheme::Mode> {
        scheme::Mode::from_id(&self.mode)
    }

    pub fn requested_variant(&self) -> scheme::Variant {
        scheme::Variant::from_id(&self.variant).unwrap_or_default()
    }

    /// What `[theme]` names that no palette, accent or token answers to, attributed to `file`, for `hogar-shell config check` and the notice the running shell keeps up while a problem lasts. Each one used to fall back without a word — an unknown theme to nord and an unknown accent to the palette's own, an unknown `[theme.colors]` token to nothing at all — so a typo looked exactly like the setting having no effect.
    ///
    /// A theme or an accent nobody has is a **warning**, because the shell puts one of its own in its place, and that is what a warning is: something done instead of what was asked. A token nobody has is an **error**: nothing takes its place, and the colour is simply not applied.
    ///
    /// Asked of the same lookups [`NordTheme::named`], [`NordTheme::accent_by_name`] and [`NordTheme::with_color`] make, so the report and the palette on screen cannot disagree about which names count. `fallback` is checked whatever `name` is: it is only read for `dynamic`, and a misspelt one would otherwise surface on the first start with no wallpaper, far from the edit that caused it.
    pub fn check(&self, file: &Path) -> Report {
        let mut report = Report::default();
        for (key, name) in [
            ("theme.name", &self.name),
            ("theme.fallback", &self.fallback),
        ] {
            if !NordTheme::has_palette(name) {
                report.warn(Finding::new(
                    file,
                    key,
                    util::message!("finding.unknown_theme", name = name),
                ));
            }
        }
        if !NordTheme::has_accent(&self.accent) {
            report.warn(Finding::new(
                file,
                "theme.accent",
                util::message!("finding.unknown_accent", name = &self.accent),
            ));
        }
        let mut tokens: Vec<&String> = self
            .colors
            .keys()
            .filter(|name| !NordTheme::is_token(name))
            .collect();
        tokens.sort();
        for name in tokens {
            report.error(Finding::new(
                file,
                format!("theme.colors.{name}"),
                util::message!("finding.unknown_token", name = name),
            ));
        }
        report
    }
}
