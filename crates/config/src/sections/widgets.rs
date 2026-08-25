//! `[widgets]` — what the shell draws on the desktop itself.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use serde::{Deserialize, Serialize};

use crate::sections::*;

/// Widgets drawn on the desktop, on a surface of their own: a clock face, an audio visualiser. All off by default, and the surface exists only while one of them is on.
///
/// **Not the wallpaper.** The wallpaper covers the whole screen under every window; this sits in what the bars left free, so a widget lines up with the applications rather than with the screen — and a visualiser that repaints with the music repaints that area instead of the whole screen behind it.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WidgetsConfig {
    pub clock: DesktopClockConfig,
    pub visualiser: DesktopVisualiserConfig,
}

impl WidgetsConfig {
    /// Whether the widgets surface is opened at all — only while something asks to be drawn on it.
    pub fn is_enabled(&self) -> bool {
        self.clock.enabled || self.visualiser.enabled
    }
}

/// Where in the widget area a widget sits.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockPlacement {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    #[default]
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl ClockPlacement {
    pub const ALL: [ClockPlacement; 9] = [
        ClockPlacement::TopLeft,
        ClockPlacement::TopCenter,
        ClockPlacement::TopRight,
        ClockPlacement::CenterLeft,
        ClockPlacement::Center,
        ClockPlacement::CenterRight,
        ClockPlacement::BottomLeft,
        ClockPlacement::BottomCenter,
        ClockPlacement::BottomRight,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ClockPlacement::TopLeft => "top_left",
            ClockPlacement::TopCenter => "top_center",
            ClockPlacement::TopRight => "top_right",
            ClockPlacement::CenterLeft => "center_left",
            ClockPlacement::Center => "center",
            ClockPlacement::CenterRight => "center_right",
            ClockPlacement::BottomLeft => "bottom_left",
            ClockPlacement::BottomCenter => "bottom_center",
            ClockPlacement::BottomRight => "bottom_right",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ClockPlacement::ALL
            .into_iter()
            .find(|placement| placement.id() == id.trim().to_ascii_lowercase().replace('-', "_"))
    }

    /// The row and column this placement occupies, as `flex` alignment values.
    pub fn alignment(self) -> (Align, Align) {
        let vertical = match self {
            ClockPlacement::TopLeft | ClockPlacement::TopCenter | ClockPlacement::TopRight => {
                Align::Start
            }
            ClockPlacement::CenterLeft | ClockPlacement::Center | ClockPlacement::CenterRight => {
                Align::Center
            }
            _ => Align::End,
        };
        let horizontal = match self {
            ClockPlacement::TopLeft | ClockPlacement::CenterLeft | ClockPlacement::BottomLeft => {
                Align::Start
            }
            ClockPlacement::TopCenter | ClockPlacement::Center | ClockPlacement::BottomCenter => {
                Align::Center
            }
            _ => Align::End,
        };
        (vertical, horizontal)
    }
}

/// The audio visualiser drawn on the desktop (`[widgets.visualiser]`), along one edge of the widget area. Off by default, and it costs nothing while it is: nothing captures audio until a surface subscribes.
///
/// What the bars *are* — how many, how smooth, how loud — is `[visualiser]`, shared with every other consumer. This section is only the look, so turning the count up here would be turning it up on the media card too, which is why it is not a key here.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct DesktopVisualiserConfig {
    pub enabled: bool,
    /// Which edge of the widget area the bars stand on. They always grow away from it, so `left` gives a column up the side.
    pub edge: Edge,
    /// How far the tallest bar reaches from that edge, in px.
    pub reach: u32,
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
    /// How far the row is held off that edge, in px, on top of the gap the widget area already keeps from the bars.
    pub margin: u32,
}

impl Default for DesktopVisualiserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: Edge::Bottom,
            reach: 140,
            gap: 3.0,
            radius: 3.0,
            opacity: 0.75,
            hide_when_silent: true,
            accent: true,
            margin: 0,
        }
    }
}

impl DesktopVisualiserConfig {
    /// How far the bars reach, bounded: a reach of zero is a row that cannot be seen, and one taller than any screen is a wallpaper made of bars.
    pub fn reach_px(&self) -> f32 {
        self.reach.clamp(8, 2000) as f32
    }

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

/// A clock drawn on the desktop (`[widgets.clock]`), the way a lock screen or a phone's home screen shows one. Off by default. `format`/`date_format` fall back to `[clock]`, so the desktop face and the bar chip read the same unless one is deliberately given its own.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct DesktopClockConfig {
    pub enabled: bool,
    /// One of the nine positions: `top_left` … `bottom_right`, `center` being the default.
    pub position: ClockPlacement,
    /// Multiplies the theme's display size, so the face can be made as large as the screen allows.
    pub scale: f32,
    /// How far the face is kept from the edges of the widget area, in px.
    pub margin: u32,
    /// Draw the face in the theme's base colour instead of its text colour — for a pale wallpaper, where light text disappears.
    pub invert: bool,
    pub show_date: bool,
    /// Overrides `[clock] format` for the desktop face only. A desktop clock usually wants `%H:%M` where the bar chip wants seconds.
    pub format: Option<String>,
    /// Overrides `[clock] date_format` for the desktop face only.
    pub date_format: Option<String>,
    /// Paint a plate behind the face, so it stays legible over a busy photograph.
    pub background: bool,
    /// How opaque that plate is, `0`–`1`.
    pub background_opacity: f32,
    /// How far the plate's edge is feathered into the wallpaper, in px. `0` gives a hard-edged card.
    pub background_blur: f32,
    /// Drop a shadow under the glyphs, which is what keeps a plateless face readable over a light wallpaper.
    pub shadow: bool,
}

impl Default for DesktopClockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            position: ClockPlacement::Center,
            scale: 3.0,
            margin: 48,
            invert: false,
            show_date: true,
            format: None,
            date_format: None,
            background: false,
            background_opacity: 0.35,
            background_blur: 0.0,
            shadow: true,
        }
    }
}

impl DesktopClockConfig {
    /// The `strftime` pattern the face renders: its own override, else `[clock]`'s answer without seconds — a desktop clock that ticks every second is a surface that repaints every second.
    pub fn time_format<'a>(&'a self, clock: &'a ClockConfig) -> &'a str {
        if let Some(format) = &self.format {
            return format;
        }
        if clock.format.is_some() {
            return clock.time_format();
        }
        if clock.twelve_hour {
            "%I:%M %p"
        } else {
            "%H:%M"
        }
    }

    pub fn date_format<'a>(&'a self, clock: &'a ClockConfig) -> &'a str {
        self.date_format.as_deref().unwrap_or(&clock.date_format)
    }

    /// The plate's fill, bounded so `background = true` cannot resolve to an invisible plate or a fully opaque one the user did not ask for.
    pub fn plate_opacity(&self) -> f32 {
        if self.background_opacity.is_finite() {
            self.background_opacity.clamp(0.05, 1.0)
        } else {
            0.35
        }
    }

    pub fn resolved_scale(&self) -> f32 {
        if self.scale.is_finite() {
            self.scale.clamp(0.5, 20.0)
        } else {
            1.0
        }
    }
}
