//! Colour schemes derived from the wallpaper.
//!
//! `[theme] name = "dynamic"` resolves through here instead of through [`NordTheme::named`]: an image is reduced
//! to one seed colour, and that seed is expanded into the same token set every built-in palette fills, so every
//! surface picks a dynamic scheme up through the reload path it already has. Nothing downstream learns a new
//! concept — a dynamic theme is a `NordTheme` like any other.
//!
//! Two decisions are worth stating up front.
//!
//! **The ramp is built in OkLCH, not RGB.** A palette is a set of lightness steps at a shared hue, and only a
//! perceptual space makes "one step lighter" mean the same thing at every hue; the same nudge in RGB moves a
//! yellow far more than a blue.
//!
//! **Semantic colours keep their own hue.** An error that came out green because the wallpaper was a forest is
//! not a theme, it is a bug. `red`/`green`/`yellow`/`blue` are pinned to fixed hues and only *harmonised* toward
//! the seed — rotated by at most [`HARMONY`] degrees — which is enough to make them belong to the palette and
//! not enough to make them lie.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use platform_wayland::EventSender;
use serde::{Deserialize, Serialize};
use telar::Color;

use crate::theme::{NordTheme, THEME_TOKENS as TOKENS, hex};
use crate::{Config, SchemeExportConfig};
use util::broadcast::Store;
use util::paths;

/// The config name that selects a wallpaper-derived scheme.
pub const DYNAMIC: &str = "dynamic";

/// How far a semantic hue may be rotated toward the seed. Enough that a red belongs to the palette, small
/// enough that it is still a red.
const HARMONY: f32 = 15.0;

/// The contrast body text must keep against the base it is read on (WCAG AA for normal text). A generated
/// palette has no designer to catch an unreadable pairing, so the ramp is corrected until it clears this.
const MIN_TEXT_CONTRAST: f32 = 4.5;

/// Whether the scheme is built for a dark or a light desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Dark, Mode::Light];

    pub fn id(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Mode::Dark),
            "light" => Some(Mode::Light),
            _ => None,
        }
    }

    /// The mode a palette actually is, judged by whether its text is lighter than its base. Lets a built-in
    /// theme answer "am I the light one" without a table that could disagree with the palette.
    pub fn of(theme: &NordTheme) -> Self {
        if theme.base.relative_luminance() > theme.text.relative_luminance() {
            Mode::Light
        } else {
            Mode::Dark
        }
    }
}

/// How much colour the scheme carries. The names match what other Material-You-style generators call the same
/// idea, so a user moving from one does not have to relearn them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// Balanced: a clearly tinted accent over near-neutral surfaces.
    #[default]
    Vibrant,
    /// The wallpaper's own chroma, carried into the surfaces as well as the accent.
    Content,
    /// Louder than the source, with the secondary hues pushed apart.
    Expressive,
    /// The seed reproduced as closely as the ramp allows — the accent *is* the wallpaper's colour.
    Fidelity,
    /// Barely tinted, for a wallpaper that should stay in the background.
    Muted,
}

impl Variant {
    pub const ALL: [Variant; 5] = [
        Variant::Vibrant,
        Variant::Content,
        Variant::Expressive,
        Variant::Fidelity,
        Variant::Muted,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Variant::Vibrant => "vibrant",
            Variant::Content => "content",
            Variant::Expressive => "expressive",
            Variant::Fidelity => "fidelity",
            Variant::Muted => "muted",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "vibrant" => Some(Variant::Vibrant),
            "content" => Some(Variant::Content),
            "expressive" => Some(Variant::Expressive),
            "fidelity" => Some(Variant::Fidelity),
            "muted" => Some(Variant::Muted),
            _ => None,
        }
    }

    /// How much of the seed's chroma the neutral surfaces carry.
    ///
    /// Calibrated against the built-in palettes rather than guessed: their bases and surfaces sit between
    /// C 0.015 (Everforest) and C 0.036 (Tokyo Night), and Nord, Catppuccin and Rosé Pine all land near 0.030.
    /// The first pass here used a tenth of that, which turned a blue-sky wallpaper into a grey shell with one
    /// blue accent — technically a tint, visibly a monochrome.
    fn neutral_chroma(self) -> f32 {
        match self {
            Variant::Muted => 0.010,
            Variant::Vibrant => 0.030,
            Variant::Expressive => 0.040,
            Variant::Content | Variant::Fidelity => 0.055,
        }
    }

    /// The chroma the accent and the semantic colours are drawn at. `Fidelity` is the exception: it takes the
    /// source's own chroma instead of a fixed one, which is what makes it a reproduction rather than a style.
    fn accent_chroma(self, seed: f32) -> f32 {
        match self {
            Variant::Muted => 0.055,
            Variant::Content => 0.095,
            Variant::Vibrant => 0.135,
            Variant::Expressive => 0.165,
            Variant::Fidelity => seed.clamp(0.02, 0.22),
        }
    }

    /// How far the secondary hues are spread away from the seed's, in degrees.
    fn spread(self) -> f32 {
        match self {
            Variant::Expressive => 1.4,
            Variant::Muted => 0.6,
            _ => 1.0,
        }
    }
}

/// A resolved scheme: the seed it came from and the palette built out of it. Serialised to the cache so a
/// restart repaints in the user's colours immediately instead of flashing the fallback palette while an image
/// is quantised again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scheme {
    /// The wallpaper the seed was taken from, for the cache key and for `hogar-shell scheme status`.
    pub source: PathBuf,
    /// The seed colour, as hex.
    pub seed: String,
    pub mode: Mode,
    pub variant: Variant,
    /// Every palette token by the name `[theme.colors]` uses, so the export files and the theme are built from
    /// one list rather than two that can disagree.
    pub colors: Vec<(String, String)>,
}

impl Scheme {
    /// The palette as a theme, starting from the built-in metrics so radius/spacing/type scale stay the design's
    /// rather than being invented per wallpaper.
    pub fn theme(&self) -> NordTheme {
        let mut theme = NordTheme::new();
        for (name, hex) in &self.colors {
            if let Some(color) = Color::from_hex(hex) {
                theme = theme.with_color(name, color);
            }
        }
        theme
    }

    pub fn color(&self, name: &str) -> Option<Color> {
        self.colors
            .iter()
            .find(|(token, _)| token == name)
            .and_then(|(_, hex)| Color::from_hex(hex))
    }
}

/// The dominant colours of an RGBA buffer, most populous first.
///
/// A histogram rather than k-means: the buckets are fixed, so the answer does not depend on where the centroids
/// happened to start, and a wallpaper reduces to the same palette every time it is opened. Four bits per channel
/// is coarse enough that a photograph's gradient collapses into a handful of buckets and fine enough to keep two
/// distinct colours apart.
fn histogram(rgba: &[u8], samples: usize) -> Vec<(Color, u32)> {
    const BUCKETS: usize = 16 * 16 * 16;
    let pixels = rgba.len() / 4;
    if pixels == 0 {
        return Vec::new();
    }
    // Sampling, not reading every pixel: a 4K wallpaper is eight million pixels and the histogram converges
    // long before that. The stride is prime-ish so a repeating pattern is not sampled in phase with itself.
    let stride = (pixels / samples.max(1)).max(1);
    let mut counts = vec![0u32; BUCKETS];
    let mut sums = vec![[0u32; 3]; BUCKETS];
    for index in (0..pixels).step_by(stride) {
        let at = index * 4;
        let (r, g, b, a) = (rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]);
        if a < 128 {
            continue;
        }
        let bucket = (r as usize >> 4) * 256 + (g as usize >> 4) * 16 + (b as usize >> 4);
        counts[bucket] += 1;
        sums[bucket][0] += r as u32;
        sums[bucket][1] += g as u32;
        sums[bucket][2] += b as u32;
    }
    let mut found: Vec<(Color, u32)> = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(bucket, count)| {
            let n = *count;
            let average = Color::from_rgb_u8(
                (sums[bucket][0] / n) as u8,
                (sums[bucket][1] / n) as u8,
                (sums[bucket][2] / n) as u8,
            );
            (average, n)
        })
        .collect();
    found.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    found
}

/// The seed colour of an image: the bucket that best combines "there is a lot of it" with "it is actually a
/// colour".
///
/// Population alone picks the sky out of every landscape and the grey out of every screenshot, which is how a
/// generated palette ends up with no colour in it. Chroma alone picks a single red pixel of a logo. The product
/// of a damped population and a capped chroma is what lands on the colour a person would name if asked what the
/// picture is.
pub fn seed_of(rgba: &[u8], samples: usize) -> Option<Color> {
    let buckets = histogram(rgba, samples);
    if buckets.is_empty() {
        return None;
    }
    let scored = buckets.iter().max_by(|a, b| {
        score(a.0, a.1)
            .partial_cmp(&score(b.0, b.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Every candidate scoring zero means a greyscale image, which is a legitimate wallpaper and not a failure:
    // the most populous bucket then seeds a near-neutral scheme.
    match scored {
        Some((color, count)) if score(*color, *count) > 0.0 => Some(*color),
        _ => buckets.first().map(|(color, _)| *color),
    }
}

fn score(color: Color, count: u32) -> f32 {
    let (lightness, chroma, _, _) = color.to_oklcha();
    // The extremes carry no usable hue and are where compression artefacts pile up.
    if !(0.12..=0.92).contains(&lightness) || chroma < 0.02 {
        return 0.0;
    }
    (count as f32).sqrt() * (chroma / 0.16).min(1.0)
}

/// The closest sRGB colour to `(lightness, chroma, hue)` that survives the round trip.
///
/// OkLCH describes far more colours than a screen can show, and asking for one it cannot does not fail — it
/// clips each channel on its own, which does not desaturate the colour, it *rotates* it. A red anchored at hue
/// 27° at full chroma came back at 40°, which is an orange; the semantic-hue test is what caught it. Halving the
/// chroma until the round trip agrees gives up only the saturation the screen was never going to show, and keeps
/// the hue the token's name promises.
fn in_gamut(lightness: f32, chroma: f32, hue: f32) -> Color {
    let fits = |candidate: Color, wanted: f32| {
        let (l, c, h, _) = candidate.to_oklcha();
        let drift = ((h - hue + 540.0).rem_euclid(360.0) - 180.0).abs();
        drift <= 0.5 && (l - lightness).abs() <= 0.01 && (c - wanted).abs() <= 0.005
    };
    let full = Color::from_oklch(lightness, chroma, hue);
    if chroma <= 0.0 || fits(full, chroma) {
        return full;
    }
    let (mut low, mut high) = (0.0f32, chroma);
    let mut best = Color::from_oklch(lightness, 0.0, hue);
    for _ in 0..16 {
        let mid = (low + high) / 2.0;
        let candidate = Color::from_oklch(lightness, mid, hue);
        if fits(candidate, mid) {
            best = candidate;
            low = mid;
        } else {
            high = mid;
        }
    }
    best
}

/// Rotates `hue` toward `toward` by at most [`HARMONY`] degrees, the short way round.
fn harmonise(hue: f32, toward: f32) -> f32 {
    let difference = (toward - hue + 540.0).rem_euclid(360.0) - 180.0;
    (hue + difference.clamp(-HARMONY, HARMONY)).rem_euclid(360.0)
}

/// The hue each semantic token is anchored at, before harmonisation. These are the hues the names mean; a
/// palette that moved them would be renaming its own tokens.
const HUES: &[(&str, f32)] = &[
    ("red", 27.0),
    ("orange", 55.0),
    ("yellow", 100.0),
    ("green", 145.0),
    ("teal", 175.0),
    ("cyan", 215.0),
    ("blue", 260.0),
    ("purple", 320.0),
];

/// The lightness ramp for a mode, as the tokens that step through it.
///
/// The steps are taken from where the built-in palettes actually sit, so a dynamic theme is recognisably a
/// member of the same family rather than a much darker stranger: their bases run L 0.21–0.32 and their surfaces
/// 0.24–0.38. An earlier ramp starting at 0.17 produced a near-black shell that no shipped palette resembles.
fn neutrals(mode: Mode) -> [(&'static str, f32); 9] {
    match mode {
        Mode::Dark => [
            ("base", 0.24),
            ("surface", 0.30),
            ("overlay", 0.35),
            ("highlight_low", 0.28),
            ("highlight_med", 0.36),
            ("highlight_high", 0.44),
            ("muted", 0.56),
            ("subtle", 0.80),
            ("text", 0.94),
        ],
        // Not the dark ramp inverted: a light theme raises a panel by *darkening* it, so surface and overlay
        // step down from the base rather than up (the same rule Catppuccin Latte follows).
        Mode::Light => [
            ("base", 0.96),
            ("surface", 0.92),
            ("overlay", 0.87),
            ("highlight_low", 0.93),
            ("highlight_med", 0.86),
            ("highlight_high", 0.78),
            ("muted", 0.62),
            ("subtle", 0.46),
            ("text", 0.35),
        ],
    }
}

/// Builds the full token set from one seed.
pub fn palette(seed: Color, mode: Mode, variant: Variant) -> Vec<(String, String)> {
    let (_, seed_chroma, seed_hue, _) = seed.to_oklcha();
    // Scaled by how colourful the wallpaper actually is, with no floor: a photograph of a grey city should keep
    // a grey shell, and a floor here is exactly what would tint it by the faint cast its sky happened to have.
    let neutral_chroma = variant.neutral_chroma() * (seed_chroma / 0.10).min(1.2);
    let chroma = variant.accent_chroma(seed_chroma);
    let accent_lightness = match mode {
        Mode::Dark => 0.80,
        Mode::Light => 0.52,
    };

    let mut colors: Vec<(String, String)> = neutrals(mode)
        .into_iter()
        .map(|(name, lightness)| {
            (
                name.to_string(),
                hex(in_gamut(lightness, neutral_chroma, seed_hue)),
            )
        })
        .collect();

    colors.push((
        "accent".to_string(),
        hex(in_gamut(accent_lightness, chroma, seed_hue)),
    ));

    for (name, base_hue) in HUES {
        // Spread pushes each anchor away from the seed before harmonisation pulls it back, which is what makes
        // `expressive` read as more colours rather than as one louder one.
        let offset = (base_hue - seed_hue + 540.0).rem_euclid(360.0) - 180.0;
        let spread = (seed_hue + offset * variant.spread()).rem_euclid(360.0);
        let hue = harmonise(spread, seed_hue);
        colors.push((
            name.to_string(),
            hex(in_gamut(accent_lightness, chroma, hue)),
        ));
    }

    let by_name = |colors: &[(String, String)], name: &str| {
        colors
            .iter()
            .find(|(token, _)| token == name)
            .map(|(_, hex)| hex.clone())
            .unwrap_or_default()
    };
    for (semantic, source) in [
        ("success", "green"),
        ("warning", "yellow"),
        ("error", "red"),
        ("info", "blue"),
    ] {
        colors.push((semantic.to_string(), by_name(&colors, source)));
    }

    readable(&mut colors, mode);
    colors.sort_by_key(|(name, _)| TOKENS.iter().position(|t| t == name).unwrap_or(usize::MAX));
    colors
}

/// Darkens (or lightens) body text until it clears [`MIN_TEXT_CONTRAST`] against the base.
///
/// The ramp above is chosen to clear it already; this is the guard for the case it cannot — a `fidelity` scheme
/// off a very light or very saturated wallpaper, where the neutral tint pushes the two ends together. Unreadable
/// text is the one failure a generated palette must not be allowed to ship.
fn readable(colors: &mut [(String, String)], mode: Mode) {
    let value = |colors: &[(String, String)], name: &str| {
        colors
            .iter()
            .find(|(token, _)| token == name)
            .and_then(|(_, hex)| Color::from_hex(hex))
    };
    let (Some(base), Some(text)) = (value(colors, "base"), value(colors, "text")) else {
        return;
    };
    let (mut lightness, chroma, hue, _) = text.to_oklcha();
    let mut corrected = text;
    for _ in 0..24 {
        if corrected.contrast_ratio(base) >= MIN_TEXT_CONTRAST {
            break;
        }
        lightness = match mode {
            Mode::Dark => (lightness + 0.02).min(1.0),
            Mode::Light => (lightness - 0.02).max(0.0),
        };
        corrected = in_gamut(lightness, chroma, hue);
    }
    if let Some(entry) = colors.iter_mut().find(|(token, _)| token == "text") {
        entry.1 = hex(corrected);
    }
}

/// Derives a scheme from an image file. Decoding and quantising is tens of milliseconds on a large wallpaper, so
/// every caller runs it off the UI thread.
pub fn from_image(path: &Path, mode: Mode, variant: Variant) -> Option<Scheme> {
    let image = ::image::open(path).ok()?.to_rgba8();
    let seed = seed_of(image.as_raw(), 40_000)?;
    Some(Scheme {
        source: path.to_path_buf(),
        seed: hex(seed),
        mode,
        variant,
        colors: palette(seed, mode, variant),
    })
}

/// The scheme the shell is currently painting with, if any.
///
/// A process-global rather than a field on `Config`: `Config::resolve_theme` is called from every surface build
/// and is pure, while the scheme is derived asynchronously from a file the config only names. Publishing it here
/// keeps `resolve_theme` synchronous and keeps the extraction off the frame. A [`Store`] rather than a plain
/// lock so the driver thread can *hear* a palette land instead of polling for it.
static CURRENT: Store<Option<Scheme>> = Store::new(|| None);

pub fn current() -> Option<Scheme> {
    CURRENT.get()
}

/// Registers `tx` for palette changes. Pass to `platform_wayland::watch` with [`on_change`].
pub fn subscribe(tx: EventSender<Option<Scheme>>) {
    CURRENT.subscribe(tx);
}

thread_local! {
    /// The palette the surfaces on this thread were last built from, so a delivery that changes nothing does
    /// not rebuild the shell. Seeded by the immediate send `subscribe` makes, which is the scheme startup
    /// already resolved.
    static PAINTED: RefCell<Option<Option<Scheme>>> = const { RefCell::new(None) };
}

/// Records the palette the surfaces about to be built will carry, so the delivery that follows is recognised as
/// old news.
///
/// Called from the reload path, which resolves the scheme *before* it rebuilds the surfaces: without this, the
/// rebuild would be followed by a delivery that looked like a change and asked for a second, identical reload.
pub fn mark_painted() {
    PAINTED.with(|painted| *painted.borrow_mut() = Some(current()));
}

/// The driver-thread consumer for [`subscribe`]: rebuilds every surface when the palette actually moved.
///
/// A reload is how a theme reaches the shell — the same path a `[theme]` edit takes — so a dynamic scheme needs
/// no second mechanism. What is left for this to catch is the case nothing else can: a palette that finishes
/// being extracted seconds after the surfaces were built, on a thread of its own.
pub fn on_change(scheme: Option<Scheme>) {
    let changed = PAINTED.with(|painted| {
        let mut painted = painted.borrow_mut();
        let changed = painted.as_ref().is_some_and(|last| *last != scheme);
        *painted = Some(scheme);
        changed
    });
    if changed {
        crate::live::request_reload();
    }
}

/// The dynamic theme, or `None` when no wallpaper has been quantised yet — which is what makes
/// `[theme] fallback` a real setting rather than a formality.
pub fn theme() -> Option<NordTheme> {
    current().map(|scheme| scheme.theme())
}

fn cache_path(source: &Path, mode: Mode, variant: Variant) -> PathBuf {
    let stamp = std::fs::metadata(source)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hash: u64 = 1469598103934665603;
    for byte in source.as_os_str().as_encoded_bytes() {
        hash = (hash ^ *byte as u64).wrapping_mul(1099511628211);
    }
    hash = (hash ^ stamp).wrapping_mul(1099511628211);
    paths::cache_dir().join("schemes").join(format!(
        "{hash:016x}-{}-{}.json",
        mode.id(),
        variant.id()
    ))
}

fn load_cached(source: &Path, mode: Mode, variant: Variant) -> Option<Scheme> {
    let text = std::fs::read_to_string(cache_path(source, mode, variant)).ok()?;
    serde_json::from_str(&text).ok()
}

fn store_cached(scheme: &Scheme) {
    let path = cache_path(&scheme.source, scheme.mode, scheme.variant);
    if let Some(parent) = path.parent() {
        paths::ensure_dir(parent.to_path_buf());
    }
    let Ok(text) = serde_json::to_string(scheme) else {
        return;
    };
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("scheme: cannot cache {}: {e}", path.display());
    }
}

/// Resolves the scheme for `source` and publishes it, returning whether the palette changed.
///
/// Synchronous, and cheap when the cache hits — which is the startup path. The miss path decodes an image, so
/// callers on the driver thread go through [`refresh`] instead.
pub fn resolve(source: &Path, mode: Mode, variant: Variant) -> bool {
    let scheme = match load_cached(source, mode, variant) {
        Some(cached) => Some(cached),
        None => {
            let derived = from_image(source, mode, variant);
            if let Some(scheme) = &derived {
                store_cached(scheme);
            }
            derived
        }
    };
    let Some(scheme) = scheme else {
        tracing::warn!("scheme: cannot read a palette out of {}", source.display());
        return false;
    };
    if CURRENT.get().as_ref() == Some(&scheme) {
        return false;
    }
    CURRENT.update(|current| *current = Some(scheme));
    true
}

/// Whether a cached palette for this image is already on disk, i.e. whether [`resolve`] would decode anything.
pub fn is_cached(source: &Path, mode: Mode, variant: Variant) -> bool {
    cache_path(source, mode, variant).exists()
}

/// The image a dynamic palette is derived from: whatever the focused screen is showing, falling back through
/// the service's own resolution order to the global choice.
///
/// The focused screen rather than the global image, because there is only ever one palette and a multi-monitor
/// desktop has to take it from somewhere. Reading the global one meant a per-monitor wallpaper change re-derived
/// nothing at all — the command answered `ok` and the colours stayed where they were.
fn source_image(config: &Config) -> Option<PathBuf> {
    crate::live::wallpaper_source(config)
}

/// Re-derives the scheme for `config`'s current wallpaper off the UI thread. The one entry point for "the
/// wallpaper changed" and "the mode changed" alike, so the two cannot drift into different behaviours.
///
/// `settle` is how long to wait before publishing. Landing a palette *is* a reload, and a reload rebuilds every
/// surface's content — including the wallpaper surface that is halfway through cross-fading to the very image
/// the palette came from, whose fade lives in the tree being replaced. Waiting out the transition means the
/// colours arrive once the picture has, which is both what the eye expects and the only way the fade survives.
/// Zero everywhere a transition is not running.
/// Re-derives the palette after *the shell itself* changed the wallpaper, reading the running config for both the
/// dynamic check and the transition to wait out.
///
/// Every path that sets a wallpaper has to call this, and there is more than one: the IPC commands, and the
/// launcher's `@` grid — which shipped without it, so a dynamic theme kept the old picture's colours until the next
/// reload. One helper rather than the two lines at each call site is what stops the third one forgetting too.
pub fn refresh_current() {
    if let Some(config) = crate::live::config() {
        let settle = config.wallpaper_transition();
        refresh(&config, settle);
    }
}

pub fn refresh(config: &Config, settle: std::time::Duration) {
    if !config.theme.is_dynamic() {
        return;
    }
    let Some(source) = source_image(config) else {
        return;
    };
    let (mode, variant) = config.scheme_selection();
    let export = config.theme.export.clone();
    let _ = std::thread::Builder::new()
        .name("hogar-shell-scheme".to_string())
        .spawn(move || {
            if !settle.is_zero() {
                std::thread::sleep(settle);
            }
            if resolve(&source, mode, variant)
                && let Some(scheme) = current()
            {
                export_scheme(&scheme, &export);
            }
        });
}

/// Loads the scheme for `config` synchronously when it is already cached, so a restart paints in the user's
/// colours on its first frame; otherwise hands the work to [`refresh`].
pub fn init(config: &Config) {
    if !config.theme.is_dynamic() {
        return;
    }
    let Some(source) = source_image(config) else {
        return;
    };
    let (mode, variant) = config.scheme_selection();
    if is_cached(&source, mode, variant) {
        resolve(&source, mode, variant);
        if let Some(scheme) = current() {
            export_scheme(&scheme, &config.theme.export);
        }
        return;
    }
    refresh(config, std::time::Duration::ZERO);
}

/// Which `[theme]` key a scheme choice writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// `[theme] name` — the palette itself.
    Palette,
    Mode,
    Variant,
}

impl Choice {
    fn key(self) -> &'static str {
        match self {
            Choice::Palette => "name",
            Choice::Mode => "mode",
            Choice::Variant => "variant",
        }
    }
}

/// Applies a scheme choice by writing it to `[theme]`, and lets the config watcher reload the shell.
///
/// A palette is a preference, not session state: it belongs in the file the user owns. Going through
/// `save_section` is what makes a scheme picked from the launcher, from a keybind, from the settings panel and
/// from a hand edit the same change arriving by the same route — and the format-preserving write is why doing
/// it from the UI does not cost the user their comments.
pub fn apply(choice: Choice, value: &str) -> Result<String, String> {
    let path = Config::default_path();
    let mut theme = Config::load_or_default(&path).theme;
    match choice {
        Choice::Palette => theme.name = value.to_string(),
        Choice::Mode => theme.mode = value.to_string(),
        Choice::Variant => theme.variant = value.to_string(),
    }
    Config::save_section(&path, "theme", &theme)
        .map_err(|e| format!("saving [theme] {}: {e}", choice.key()))?;
    Ok(value.to_string())
}

/// Every palette, mode and variant a picker should offer, in the order it should offer them.
pub fn choices() -> Vec<(Choice, String)> {
    let mut options: Vec<(Choice, String)> = crate::theme::BUILT_IN_THEMES
        .iter()
        .map(|name| (Choice::Palette, (*name).to_string()))
        .collect();
    options.push((Choice::Palette, DYNAMIC.to_string()));
    options.extend(
        ["auto", "dark", "light"]
            .into_iter()
            .map(|mode| (Choice::Mode, mode.to_string())),
    );
    options.extend(
        Variant::ALL
            .into_iter()
            .map(|variant| (Choice::Variant, variant.id().to_string())),
    );
    options
}

/// Writes the resolved palette out for the applications that are not this shell (J4).
///
/// The point of a dynamic scheme is a desktop that agrees with itself, and nothing else on it reads
/// `config.toml`. Each format is a flat list of the same tokens, so adding a consumer is a template here rather
/// than a second place the palette is decided.
///
/// Written on a thread of its own: the cached-startup path calls this from the driver thread, and a hook that
/// reloads a slow application must not be the reason a bar takes a second to appear.
pub fn export_scheme(scheme: &Scheme, config: &SchemeExportConfig) {
    if !config.enabled {
        return;
    }
    let scheme = scheme.clone();
    let config = config.clone();
    let _ = std::thread::Builder::new()
        .name("hogar-shell-scheme-export".to_string())
        .spawn(move || write_exports(&scheme, &config));
}

fn write_exports(scheme: &Scheme, config: &SchemeExportConfig) {
    let dir = paths::ensure_dir(config.resolved_dir());
    let write = |name: &str, body: String| {
        let path = dir.join(name);
        if let Err(e) = std::fs::write(&path, body) {
            tracing::warn!("scheme export: cannot write {}: {e}", path.display());
        }
    };
    if config.json {
        write("scheme.json", as_json(scheme));
    }
    if config.gtk {
        write("scheme.css", as_gtk(scheme));
    }
    if config.qt {
        write("scheme.conf", as_ini(scheme));
    }
    if config.terminal {
        write("scheme.sh", as_shell(scheme));
        write("sequences", as_sequences(scheme));
    }
    for hook in &config.hooks {
        let hook = hook.trim();
        if !hook.is_empty() {
            util::process::run_detached(hook.to_string());
        }
    }
}

fn as_json(scheme: &Scheme) -> String {
    serde_json::to_string_pretty(scheme).unwrap_or_default() + "\n"
}

fn as_gtk(scheme: &Scheme) -> String {
    let mut out = String::from("/* Generated by hogar-shell. Edits are overwritten. */\n");
    for (name, value) in &scheme.colors {
        out.push_str(&format!("@define-color {name} {value};\n"));
    }
    out
}

fn as_ini(scheme: &Scheme) -> String {
    let mut out = String::from("# Generated by hogar-shell. Edits are overwritten.\n[Colors]\n");
    for (name, value) in &scheme.colors {
        out.push_str(&format!("{name}={value}\n"));
    }
    out
}

fn as_shell(scheme: &Scheme) -> String {
    let mut out = String::from("# Generated by hogar-shell. Edits are overwritten.\n");
    out.push_str(&format!("wallpaper='{}'\n", scheme.source.display()));
    for (name, value) in &scheme.colors {
        out.push_str(&format!("{}='{value}'\n", name.to_ascii_lowercase()));
    }
    out
}

/// The sixteen ANSI colours as OSC escapes, which is how a running terminal is recoloured without restarting it
/// (`cat sequences > /dev/pts/N`). The bright half is the same hue one lightness step up, so the pairs stay
/// recognisably the same colour.
fn as_sequences(scheme: &Scheme) -> String {
    let get = |name: &str| scheme.color(name).unwrap_or(Color::BLACK);
    let brighter = |color: Color| {
        let (lightness, chroma, hue, _) = color.to_oklcha();
        in_gamut((lightness + 0.12).min(1.0), chroma, hue)
    };
    let ansi = [
        get("base"),
        get("red"),
        get("green"),
        get("yellow"),
        get("blue"),
        get("purple"),
        get("cyan"),
        get("subtle"),
    ];
    let mut out = String::new();
    for (index, color) in ansi.iter().enumerate() {
        out.push_str(&format!("\x1b]4;{index};{}\x1b\\", hex(*color)));
    }
    for (index, color) in ansi.iter().enumerate() {
        out.push_str(&format!(
            "\x1b]4;{};{}\x1b\\",
            index + 8,
            hex(brighter(*color))
        ));
    }
    out.push_str(&format!("\x1b]10;{}\x1b\\", hex(get("text"))));
    out.push_str(&format!("\x1b]11;{}\x1b\\", hex(get("base"))));
    out.push_str(&format!("\x1b]12;{}\x1b\\", hex(get("accent"))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer of one repeated colour, plus `noise` pixels of a second one.
    fn image(main: [u8; 3], count: usize, other: [u8; 3], noise: usize) -> Vec<u8> {
        let mut pixels = Vec::new();
        for _ in 0..count {
            pixels.extend_from_slice(&[main[0], main[1], main[2], 255]);
        }
        for _ in 0..noise {
            pixels.extend_from_slice(&[other[0], other[1], other[2], 255]);
        }
        pixels
    }

    #[test]
    fn the_seed_is_the_colour_a_person_would_name() {
        // Mostly sky, with a small area of vivid orange. Population alone answers "grey-blue"; the score is what
        // keeps the picture's actual colour.
        let pixels = image([120, 140, 160], 900, [230, 120, 20], 100);
        let seed = seed_of(&pixels, 4000).expect("a seed comes out");
        let (_, chroma, hue, _) = seed.to_oklcha();
        assert!(chroma > 0.05, "the seed carries colour, not grey: {chroma}");
        assert!(
            (20.0..100.0).contains(&hue),
            "it is the orange, not the sky: {hue}"
        );
    }

    #[test]
    fn a_greyscale_wallpaper_still_yields_a_scheme() {
        // Every candidate scores zero here. Refusing would leave the shell with no palette at all, which is
        // worse than a near-neutral one.
        let pixels = image([90, 90, 90], 500, [200, 200, 200], 100);
        let seed = seed_of(&pixels, 4000).expect("grey is a wallpaper too");
        assert!(seed.to_oklcha().1 < 0.02);
        let colors = palette(seed, Mode::Dark, Variant::Vibrant);
        assert_eq!(colors.len(), TOKENS.len(), "a full token set either way");
    }

    #[test]
    fn an_empty_buffer_is_not_a_scheme() {
        assert!(seed_of(&[], 100).is_none());
        // Fully transparent pixels are skipped, so this is empty too rather than seeding from garbage.
        assert!(seed_of(&[10, 20, 30, 0, 40, 50, 60, 0], 100).is_none());
    }

    #[test]
    fn every_token_the_theme_reads_is_produced() {
        let seed = Color::from_rgb_u8(200, 90, 40);
        for mode in Mode::ALL {
            for variant in Variant::ALL {
                let colors = palette(seed, mode, variant);
                for token in TOKENS {
                    assert!(
                        colors.iter().any(|(name, _)| name == token),
                        "{token} missing from {}/{}",
                        mode.id(),
                        variant.id()
                    );
                }
                assert!(
                    colors.iter().all(|(_, hex)| Color::from_hex(hex).is_some()),
                    "every value parses back"
                );
            }
        }
    }

    #[test]
    fn a_generated_palette_is_readable_in_both_modes() {
        // The guard that matters: nothing else checks a wallpaper-derived palette before it is on screen.
        for seed in [
            Color::from_rgb_u8(200, 90, 40),
            Color::from_rgb_u8(20, 30, 90),
            Color::from_rgb_u8(250, 245, 200),
            Color::from_rgb_u8(10, 10, 10),
        ] {
            for mode in Mode::ALL {
                for variant in Variant::ALL {
                    let scheme = Scheme {
                        source: PathBuf::new(),
                        seed: hex(seed),
                        mode,
                        variant,
                        colors: palette(seed, mode, variant),
                    };
                    let theme = scheme.theme();
                    assert!(
                        theme.text.contrast_ratio(theme.base) >= MIN_TEXT_CONTRAST,
                        "{}/{} text is unreadable on its base",
                        mode.id(),
                        variant.id()
                    );
                    assert_ne!(theme.base, theme.surface, "a panel must lift off the page");
                    assert_eq!(
                        Mode::of(&theme),
                        mode,
                        "a {} scheme must actually be {}",
                        mode.id(),
                        mode.id()
                    );
                }
            }
        }
    }

    #[test]
    fn a_colourful_wallpaper_tints_the_surfaces_not_only_the_accent() {
        // The failure this guards is not a crash and not an unreadable pairing, so nothing else catches it: a
        // sky-blue wallpaper produced surfaces at C 0.010, which reads as a grey shell with one blue accent.
        // The floor is where the built-in palettes sit — Everforest, the flattest of them, is C 0.015.
        let sky = Color::from_rgb_u8(109, 134, 236);
        let (_, seed_chroma, seed_hue, _) = sky.to_oklcha();
        for mode in Mode::ALL {
            let colors = palette(sky, mode, Variant::Vibrant);
            for token in ["base", "surface", "overlay", "text"] {
                let color = colors
                    .iter()
                    .find(|(name, _)| name == token)
                    .and_then(|(_, hex)| Color::from_hex(hex))
                    .expect("token present");
                let (_, chroma, hue, _) = color.to_oklcha();
                assert!(
                    chroma >= 0.015,
                    "{}/{token} is C{chroma:.3} — a tint nobody can see",
                    mode.id()
                );
                let drift = ((hue - seed_hue + 540.0).rem_euclid(360.0) - 180.0).abs();
                assert!(
                    drift < 20.0,
                    "{}/{token} is not the wallpaper's hue",
                    mode.id()
                );
            }
        }

        let base = colors_of(sky, Mode::Dark, "base");
        assert!(
            (0.20..=0.34).contains(&base.to_oklcha().0),
            "a dark base outside the range every shipped palette occupies: {:?}",
            base.to_oklcha().0
        );

        let grey = Color::from_rgb_u8(128, 128, 128);
        assert!(
            colors_of(grey, Mode::Dark, "base").to_oklcha().1 < 0.005,
            "a greyscale wallpaper stays grey"
        );
        assert!(
            seed_chroma > 0.05,
            "the fixture is a colourful seed, or this test proves nothing"
        );
    }

    fn colors_of(seed: Color, mode: Mode, token: &str) -> Color {
        palette(seed, mode, Variant::Vibrant)
            .iter()
            .find(|(name, _)| name == token)
            .and_then(|(_, hex)| Color::from_hex(hex))
            .expect("token present")
    }

    #[test]
    fn semantic_colours_keep_their_own_hue() {
        // A blue-green wallpaper must not produce a green error. Harmonisation may rotate the anchors, never
        // rename them.
        let seed = Color::from_rgb_u8(30, 140, 120);
        let colors = palette(seed, Mode::Dark, Variant::Vibrant);
        let hue_of = |name: &str| {
            colors
                .iter()
                .find(|(token, _)| token == name)
                .and_then(|(_, hex)| Color::from_hex(hex))
                .map(|c| c.to_oklcha().2)
                .expect("token present")
        };
        let red = hue_of("red");
        assert!(
            !(60.0..300.0).contains(&red),
            "the error colour is still a red: {red}"
        );
        let distance = |a: f32, b: f32| ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs();
        // The tolerance is the harmony budget plus what a hex round trip costs: the palette is stored as
        // 8-bit `#rrggbb`, and quantising a colour moves its hue by a degree or so.
        const QUANTISATION: f32 = 2.0;
        for (name, anchor) in HUES {
            let drift = distance(hue_of(name), *anchor);
            assert!(
                drift <= HARMONY + QUANTISATION,
                "'{name}' drifted {drift:.1}° off its anchor"
            );
        }
    }

    #[test]
    fn the_variants_are_actually_different() {
        let seed = Color::from_rgb_u8(180, 60, 200);
        let chroma_of = |variant: Variant| {
            palette(seed, Mode::Dark, variant)
                .iter()
                .find(|(name, _)| name == "accent")
                .and_then(|(_, hex)| Color::from_hex(hex))
                .map(|c| c.to_oklcha().1)
                .expect("accent present")
        };
        assert!(
            chroma_of(Variant::Muted) < chroma_of(Variant::Vibrant),
            "muted is quieter than vibrant"
        );
        assert!(
            chroma_of(Variant::Vibrant) < chroma_of(Variant::Expressive),
            "expressive is louder than vibrant"
        );
    }

    #[test]
    fn ids_round_trip_through_config_and_ipc() {
        for mode in Mode::ALL {
            assert_eq!(Mode::from_id(mode.id()), Some(mode));
        }
        for variant in Variant::ALL {
            assert_eq!(Variant::from_id(variant.id()), Some(variant));
        }
        assert_eq!(Mode::from_id("  LIGHT "), Some(Mode::Light));
        assert_eq!(Variant::from_id("nope"), None);
    }

    #[test]
    fn the_export_files_carry_every_token() {
        let seed = Color::from_rgb_u8(120, 90, 200);
        let scheme = Scheme {
            source: PathBuf::from("/tmp/wall.png"),
            seed: hex(seed),
            mode: Mode::Dark,
            variant: Variant::Vibrant,
            colors: palette(seed, Mode::Dark, Variant::Vibrant),
        };
        let css = as_gtk(&scheme);
        let ini = as_ini(&scheme);
        for token in TOKENS {
            assert!(
                css.contains(&format!("@define-color {token} ")),
                "{token} in gtk"
            );
            assert!(ini.contains(&format!("{token}=#")), "{token} in ini");
        }
        assert!(as_shell(&scheme).contains("wallpaper='/tmp/wall.png'"));
        // The escapes are what a terminal actually reads; a plain hex dump would be silently useless.
        assert!(as_sequences(&scheme).contains("\x1b]4;0;"));
        assert!(
            as_sequences(&scheme).contains("\x1b]11;"),
            "the background is set"
        );
        let parsed: Scheme = serde_json::from_str(&as_json(&scheme)).expect("json round-trips");
        assert_eq!(parsed, scheme);
    }
}
