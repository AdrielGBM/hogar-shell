//! `[audio]`, `[visualiser]`, `[media]` and `[lyrics]`.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Audio control (`[audio]`). `increment` is what one wheel notch over the volume or microphone chip moves and what `hogar-shell volume up` steps by. `max_volume` is the ceiling the sink can be raised to: PipeWire lets a sink boost past 100 %, which rescues a quiet laptop and wrecks a good speaker, so it belongs to the user rather than to a constant in the code.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct AudioConfig {
    pub increment: i32,
    pub max_volume: i32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            increment: 5,
            max_volume: 150,
        }
    }
}

impl AudioConfig {
    /// The step, bounded so `0` or a negative typo can't leave the chip inert or make the wheel run backwards.
    pub fn step(&self) -> i32 {
        self.increment.clamp(1, 50)
    }

    /// The sink ceiling, never under 100 % — a sink must at least reach its own nominal maximum.
    pub fn ceiling(&self) -> i32 {
        self.max_volume.clamp(100, 300)
    }
}

/// The audio visualiser's *source* (`[visualiser]`): how the sound coming out of the speakers is turned into bars. Shared by everything that draws it — the desktop background, the media card — so the analysis is described once and the look belongs to each consumer's own section.
///
/// Nothing here starts a capture on its own: the service behind it runs only while something is subscribed, so a shell with no visualiser switched on never opens a stream.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct VisualiserConfig {
    /// How a row of these bars placed on the desktop is drawn.
    pub face: VisualiserFaceConfig,
    /// How many bands the spectrum is folded into — the number of bars every consumer draws.
    pub bars: u32,
    /// How much of the previous frame each bar keeps, 0–1. Higher is smoother and slower; `0` follows the transform exactly and shimmers.
    pub smoothing: f32,
    /// How quiet a band has to be to read as nothing, in decibels below full scale. Raise it towards `-40` to make quiet passages flatter, lower it towards `-80` to give them more life.
    pub floor_db: f32,
    /// A multiplier applied before the bars are normalised. Above `1` suits quiet sources; the bars clip at the top rather than distorting.
    pub gain: f32,
    /// How far above its recent average the bass has to jump to count as a beat. Around `1.1` finds a beat in almost anything; above `2` only the most obvious ones.
    pub beat_sensitivity: f32,
    /// Transforms per second. The upper bound on how often a visualiser surface repaints, so it is also the cost.
    pub frame_rate: u32,
}

/// How a visualiser placed on the desktop is drawn (`[visualiser.face]`), as against what the bars *are* — how many, how smooth, how loud — which is `[visualiser]`'s other keys and is shared with every consumer.
///
/// Which edge the bars stand on, how far the tallest reaches and how far the row is held off that edge are not here: they are a dock area's `edge`, its `thickness` and its `padding`, because they describe an arrangement rather than a visualiser.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct VisualiserFaceConfig {
    /// The gap between two bars, in px.
    pub gap: f32,
    /// How round a bar's ends are, in px. `0` is square; half the bar's own width is a capsule.
    pub radius: f32,
    /// How opaque the bars are over the wallpaper, `0`–`1`.
    pub opacity: f32,
    /// Fade the bars out when nothing is playing, rather than leaving a flat line across the screen.
    pub hide_when_silent: bool,
    /// Draw the bars in the theme's accent colour rather than its text colour.
    pub accent: bool,
}

impl Default for VisualiserFaceConfig {
    fn default() -> Self {
        Self {
            gap: 3.0,
            radius: 3.0,
            opacity: 0.75,
            hide_when_silent: true,
            accent: true,
        }
    }
}
impl Default for VisualiserConfig {
    fn default() -> Self {
        Self {
            face: VisualiserFaceConfig::default(),
            bars: 48,
            smoothing: 0.6,
            floor_db: -60.0,
            gain: 1.0,
            beat_sensitivity: 1.35,
            frame_rate: 60,
        }
    }
}

impl VisualiserConfig {
    /// How many bands to compute. Never zero — a spectrum with no bands is a division by its own length — and bounded above because past a couple of hundred a band is narrower than one FFT bin.
    pub fn band_count(&self) -> usize {
        self.bars.clamp(1, 256) as usize
    }

    pub fn rate(&self) -> u32 {
        self.frame_rate.clamp(10, 144)
    }

    /// How fast a bar rises. Deliberately not the smoothing the user set: a bar that climbs as slowly as it falls misses the attack of every note, which is the part a visualiser exists to show. So rising is always most of the way there in one frame and only the *fall* is smoothed.
    pub fn attack(&self) -> f32 {
        (1.0 - self.smoothed() * 0.4).clamp(0.05, 1.0)
    }

    /// How fast a bar falls back.
    pub fn decay(&self) -> f32 {
        (1.0 - self.smoothed()).clamp(0.02, 1.0)
    }

    fn smoothed(&self) -> f32 {
        if self.smoothing.is_finite() {
            self.smoothing.clamp(0.0, 0.98)
        } else {
            0.6
        }
    }

    /// The noise floor, always negative — a floor at or above full scale leaves every bar at zero.
    pub fn floor_db(&self) -> f32 {
        if self.floor_db.is_finite() {
            self.floor_db.clamp(-100.0, -10.0)
        } else {
            -60.0
        }
    }

    pub fn gain(&self) -> f32 {
        if self.gain.is_finite() {
            self.gain.clamp(0.1, 10.0)
        } else {
            1.0
        }
    }

    /// The beat threshold, never at or below `1.0`: a ratio of one calls every frame a beat.
    pub fn sensitivity(&self) -> f32 {
        if self.beat_sensitivity.is_finite() {
            self.beat_sensitivity.clamp(1.05, 5.0)
        } else {
            1.35
        }
    }
}

/// Timed lyrics (`[lyrics]`).
///
/// Where hand-kept `.lrc` files live is `[paths] lyrics`, with every other folder the shell owns — a file sitting next to the audio track is found without either. `online` is the only part of the feature that asks a third party anything (it sends the artist, title, album and length of what is playing to LRCLIB), so it is a switch of its own rather than part of `enabled`.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct LyricsConfig {
    pub enabled: bool,
    /// Look the words up on LRCLIB when there is no local file. Sends the track's tags to that service.
    pub online: bool,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            online: true,
        }
    }
}

/// The `media` module. `preferred_player` names an MPRIS bus suffix (`spotify`, `mpv`) to favour when several players are running — it only wins while that player is actually up, so naming one you don't always run never blanks the chip. `aliases` renames a player for display, since players name themselves badly often enough (`com.github.th_ch.youtube_music` → `YT Music`) to be worth a config key.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct MediaConfig {
    pub preferred_player: String,
    /// Max characters of `artist — title` on the bar, bounding a value with no natural size.
    pub max_chars: u32,
    /// What the wheel over the chip does: `volume`, `track`, `seek`, or `none`.
    pub scroll: MediaScroll,
    /// Scroll a title longer than `max_chars` instead of cutting it. Off by default: a bar that never moves is easier to read past, and a marquee costs a repaint per step for as long as the track is playing.
    pub marquee: bool,
    /// Milliseconds per character of marquee travel.
    pub marquee_speed_ms: u32,
    /// Seconds the wheel moves the playhead per notch, when `scroll = "seek"`.
    pub seek_seconds: u32,
    /// Ring the dashboard's cover art with the audio visualiser. Costs an audio capture for as long as the media page is open — see `[visualiser]` for what the bars are made of.
    pub visualiser: bool,
    pub aliases: HashMap<String, String>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            preferred_player: String::new(),
            max_chars: 40,
            scroll: MediaScroll::default(),
            marquee: false,
            marquee_speed_ms: 220,
            seek_seconds: 5,
            visualiser: false,
            aliases: HashMap::new(),
        }
    }
}

impl MediaConfig {
    /// Clamped on read: below about 60 ms the text is a blur, and above a few seconds it reads as stuck.
    pub fn marquee_step(&self) -> Duration {
        Duration::from_millis(self.marquee_speed_ms.clamp(60, 2000) as u64)
    }

    /// The wheel's seek step in microseconds, which is MPRIS's unit.
    pub fn seek_micros(&self) -> i64 {
        self.seek_seconds.clamp(1, 600) as i64 * 1_000_000
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaScroll {
    #[default]
    Volume,
    Track,
    /// Move the playhead. Needs a player that reports `CanSeek`; on one that does not, the wheel does nothing rather than pretending.
    Seek,
    None,
}

impl VisualiserFaceConfig {
    /// The bar opacity, never fully transparent — a visualiser switched on is one that can be seen.
    pub fn alpha(&self) -> f32 {
        if self.opacity.is_finite() {
            self.opacity.clamp(0.05, 1.0)
        } else {
            0.75
        }
    }

    pub fn gap_px(&self) -> f32 {
        if self.gap.is_finite() {
            self.gap.clamp(0.0, 40.0)
        } else {
            3.0
        }
    }

    pub fn radius_px(&self) -> f32 {
        if self.radius.is_finite() {
            self.radius.clamp(0.0, 40.0)
        } else {
            3.0
        }
    }
}
