//! `[bars]`, `[panels]`, and every `[toml]` section a chip on a bar reads.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::sections::*;

/// Fallback gap a panel keeps from a hugging bar (one with no outer gap of its own) and from the screen edges.
pub const DEFAULT_PANEL_GAP: u32 = 8;

/// What the settings application spends on its own title bar, search row and padding before any form is drawn. Subtracted from the surface height to size the scrolling page area — see [`Config::settings_page_height`].
pub(crate) const SETTINGS_CHROME: f32 = 108.0;

/// Modules whose panel is an *application* rather than a card, and the float each needs, as `(id, w, h)`.
///
/// The default open mode is a drawer because that is what a panel is: a card dropped under the chip you pressed. Settings is not that — it is a nav pane with a page beside it — and in a 320px drawer the nav leaves no room for a form at all. Putting the answer here rather than in [`Config::starter`] is what makes it true for the installs that already have a config file, which is all of them after the first run; an explicit `[modules.<id>]` still wins over it.
const APPLICATION_PANELS: &[(&str, u32, u32)] = &[("settings", 920, 680)];

pub(crate) fn application_panel(id: &str) -> Option<(u32, u32)> {
    APPLICATION_PANELS
        .iter()
        .find(|(name, _, _)| *name == id)
        .map(|(_, width, height)| (*width, *height))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right];

    pub fn is_horizontal(self) -> bool {
        matches!(self, Edge::Top | Edge::Bottom)
    }

    pub fn is_vertical(self) -> bool {
        !self.is_horizontal()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }

    pub fn corners(self) -> (Corner, Corner) {
        match self {
            Edge::Top => (Corner::TopLeft, Corner::TopRight),
            Edge::Bottom => (Corner::BottomLeft, Corner::BottomRight),
            Edge::Left => (Corner::TopLeft, Corner::BottomLeft),
            Edge::Right => (Corner::TopRight, Corner::BottomRight),
        }
    }
}

/// How a module's panel opens: a drawer hanging off the bar edge (default), or a centred floating window with a title bar and close button.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpenMode {
    #[default]
    Drawer,
    Float,
}

/// Per-module presentation override, keyed by module id under `[modules.<id>]`: container variant, an accent token that wins over the global `[theme] accent`, how its panel opens, and how large it opens.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ModuleOverride {
    pub variant: Variant,
    pub accent: Option<String>,
    pub open: OpenMode,
    /// Overrides `[panels.float] width` for this module's float, in logical px. Unset follows the global size.
    pub width: Option<u32>,
    /// Overrides `[panels.float] height` for this module's float, in logical px.
    pub height: Option<u32>,
}

/// Which bar zone a module sits in; a drawer derives its cross-axis alignment from this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Start,
    Center,
    End,
}

/// One module placed on a bar.
///
/// Written as a bare id in the common case (`start = ["clock", "workspaces"]`) and as a table when an instance needs settings of its own (`{ id = "clock", accent = "red" }`). The table form is what lets the same module appear on a bar twice looking different — a `[modules.<id>]` override is keyed by id and so applies to every copy at once.
///
/// Deliberately presentation-only. `open` stays under `[modules.<id>]` because a panel is toggled by module id from three places — a chip, `hogar-shell panel toggle`, a keybind — and only one of them has an entry in hand; an entry-scoped answer would make the same panel open differently depending on how you asked for it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleEntry {
    pub id: String,
    pub variant: Option<Variant>,
    pub accent: Option<String>,
}

impl ModuleEntry {
    pub fn bare(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            variant: None,
            accent: None,
        }
    }

    /// Whether the entry carries nothing beyond its id, and so writes back as a plain string.
    fn is_bare(&self) -> bool {
        self.variant.is_none() && self.accent.is_none()
    }
}

/// The table form, and the shape a non-bare entry serialises to.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub(crate) struct ModuleEntryTable {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    variant: Option<Variant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum RawModuleEntry {
    Bare(String),
    Table(ModuleEntryTable),
}

impl<'de> Deserialize<'de> for ModuleEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match RawModuleEntry::deserialize(deserializer)? {
            RawModuleEntry::Bare(id) => Self::bare(id),
            RawModuleEntry::Table(t) => Self {
                id: t.id,
                variant: t.variant,
                accent: t.accent,
            },
        })
    }
}
impl Serialize for ModuleEntry {
    /// A bare entry writes back as the string it was read from, so a config that never used the table form round-trips through the settings panel unchanged.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.is_bare() {
            return serializer.serialize_str(&self.id);
        }
        ModuleEntryTable {
            id: self.id.clone(),
            variant: self.variant,
            accent: self.accent.clone(),
        }
        .serialize(serializer)
    }
}

/// The drawer panel's size (§4): a fixed width and a max height its content scrolls within.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct DrawerConfig {
    pub width: f32,
    pub max_height: f32,
}

impl Default for DrawerConfig {
    fn default() -> Self {
        Self {
            width: 320.0,
            max_height: 280.0,
        }
    }
}

/// A floating window's size (§5) in logical px.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct FloatConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for FloatConfig {
    fn default() -> Self {
        Self {
            width: 360,
            height: 240,
        }
    }
}

/// The compact status cluster (`[status_icons]`): several service readings sharing one chip.
///
/// `icons` is a list rather than a set of flags because the order is the point — it is what a user reads left-to-right, and a fixed order would make the cluster the shell's priority instead of theirs. The names match the module ids the same readings have as standalone chips, so moving one between the two is not a rename; `caps` and `num` are the exception, since `lockstatus` is one module drawing two indicators and a cluster should be able to take only one of them.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct StatusIconsConfig {
    pub icons: Vec<String>,
    /// Gap between icons, as a fraction of the icon size, so a cluster keeps its proportions on any bar.
    pub spacing: f32,
}

impl Default for StatusIconsConfig {
    fn default() -> Self {
        Self {
            icons: ["volume", "mic", "network", "battery"]
                .into_iter()
                .map(String::from)
                .collect(),
            spacing: 0.35,
        }
    }
}

/// Hover popouts (`[popouts]`): the readout a chip shows while the pointer rests on it, distinct from the drawer a click opens.
///
/// The delays are what separate a popout from a flicker. Without `open_delay`, dragging the pointer across the bar would fire every chip's popout in turn; without `close_delay`, the popout would vanish in the gap between the chip and itself. Both are clamped on read, so a typo can make a popout slow but never instant or permanent.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct PopoutsConfig {
    /// Off costs nothing: no chip tracks the pointer and no surface is ever opened.
    pub enabled: bool,
    /// How long the pointer must rest on a chip before its popout opens, in ms.
    pub open_delay: u64,
    /// How long the popout survives after the pointer leaves, in ms.
    pub close_delay: u64,
    pub width: f32,
    /// The tallest a popout may grow. Its surface is this tall whatever the card needs; the surplus is carved out of the input region, so it stays click-through rather than swallowing presses.
    pub max_height: f32,
}

impl Default for PopoutsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            open_delay: 280,
            close_delay: 200,
            width: 264.0,
            max_height: 300.0,
        }
    }
}

impl PopoutsConfig {
    /// The rest before opening. Never zero: an instant popout on a bar the pointer is only crossing is noise.
    pub fn open_after(&self) -> Duration {
        Duration::from_millis(self.open_delay.clamp(60, 5_000))
    }

    /// The grace after leaving. Never zero either — the pointer has to cross the gap between the chip and the popout to reach it, and a zero grace would close it mid-crossing.
    pub fn close_after(&self) -> Duration {
        Duration::from_millis(self.close_delay.clamp(60, 5_000))
    }

    pub fn card_width(&self) -> f32 {
        self.width.clamp(140.0, 900.0)
    }

    pub fn card_height(&self) -> f32 {
        self.max_height.clamp(80.0, 1200.0)
    }
}

/// Panel presentation shared by drawers and floating windows (`[panels]`): each form's size, and the gesture that opens one. One home for both so a drawer and a float are configured the same way.
///
/// **What is deliberately not here.** The gap a panel keeps from the bar is derived, never set: the bar's own outer gap when it floats, else a default so a hugging bar's panels still breathe. And its opacity is `[theme] opacity`, for every surface at once. Both used to be overridable per-panel, and neither key bought anything but the chance for a drawer to sit at a distance, or at an opacity, that nothing else on the screen shares.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct PanelsConfig {
    /// How far a chip must be dragged away from the bar before letting go opens its panel, in px. `0` switches the gesture off. One threshold for every panel rather than one each: the gesture is the same everywhere on the bar, and a per-panel distance would make the bar feel inconsistent under the same finger.
    pub drag_threshold: f32,
    pub drawer: DrawerConfig,
    pub float: FloatConfig,
}

impl Default for PanelsConfig {
    fn default() -> Self {
        Self {
            drag_threshold: 48.0,
            drawer: DrawerConfig::default(),
            float: FloatConfig::default(),
        }
    }
}

impl PanelsConfig {
    /// The drag distance that opens a panel, or `None` when the gesture is off. Floored well above the tap slop: a threshold a stray press could cross would open a panel every time a chip was clicked slightly unsteadily.
    pub fn drag_threshold(&self) -> Option<f32> {
        (self.drag_threshold.is_finite() && self.drag_threshold > 0.0)
            .then(|| self.drag_threshold.clamp(16.0, 400.0))
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
}

/// The `active_window` module. `compact` shows the app's class instead of the document title — stable while you move around inside one app, and much narrower.
///
/// Nothing bounds the title's length. The chip gives up width when its side of the bar runs short and the label elides, so a browser tab that runs to a paragraph costs the modules beside it nothing. A character count bounded it once and could not tell the two cases apart: it cut a short title on a wide bar exactly as readily as a long one on a narrow bar, which is the wrong unit for a question about room.
///
/// `inverted` puts the icon after the title instead of before it. Which reads better depends on where the chip sits: leading the icon points into the bar from the left, and trailing it does the same from the right, so a chip in the end zone usually wants this on.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct ActiveWindowConfig {
    pub compact: bool,
    pub show_icon: bool,
    pub inverted: bool,
}

impl Default for ActiveWindowConfig {
    fn default() -> Self {
        Self {
            compact: false,
            show_icon: true,
            inverted: false,
        }
    }
}

/// How a rendered label is cased. Applied after the template, so it works on `{name}` (which Hyprland reports however the user named the workspace) without every template having to spell the casing out.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Capitalize {
    #[default]
    None,
    Upper,
    Lower,
    /// First letter of every word, the rest lowered.
    Title,
}

impl Capitalize {
    pub fn apply(self, text: &str) -> String {
        match self {
            Capitalize::None => text.to_string(),
            Capitalize::Upper => text.to_uppercase(),
            Capitalize::Lower => text.to_lowercase(),
            Capitalize::Title => title_case(text),
        }
    }
}

/// Uppercases the first letter of every whitespace-separated word and lowers the rest, preserving the original separators so `my-notes  2` keeps its dash and its double space.
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for c in text.chars() {
        if c.is_whitespace() {
            at_word_start = true;
            out.push(c);
        } else if at_word_start {
            at_word_start = false;
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The `workspaces` module.
///
/// `shown` pins how many pills the bar draws regardless of how many workspaces exist, which is what keeps the bar's width from shifting every time one is created or destroyed; `0` shows exactly the ones that exist. `label` is a `{id}`/`{name}`/`{index}` template so a user can have numbers, names or icons without the shell enumerating presets, and `special_icons` maps a scratchpad's bare name to an Iconify glyph.
///
/// `occupied_label` and `active_label` override that template for a pill holding windows and for the focused one; both empty (the default) means every pill renders the same way, which is what most bars want.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct WorkspacesConfig {
    pub shown: u32,
    /// Show only the workspaces belonging to this bar's own monitor.
    pub per_monitor: bool,
    pub show_special: bool,
    /// Draw an app icon per window inside each pill, capped at `max_window_icons`.
    pub window_icons: bool,
    pub max_window_icons: u32,
    /// Tint a pill that holds windows differently from an empty one. Ignored while `indicator` is on, which needs every pill transparent to slide under them; the label colour carries occupancy there.
    pub occupied_background: bool,
    /// Mark the active workspace with one box that slides between pills instead of recolouring each pill in place. Off restores the older look exactly — the pill paints its own accent and nothing moves.
    pub indicator: bool,
    /// How far the indicator stretches along its direction of travel, as a fraction of the distance still to cover. `0` keeps it exactly one pill wide the whole way; the default gives it a little speed.
    pub indicator_trail: f32,
    /// The wheel over the pills switches workspace.
    pub scroll: bool,
    pub label: String,
    /// Template for a pill that holds windows; empty falls back to `label`.
    pub occupied_label: String,
    /// Template for the focused pill; empty falls back to `occupied_label`, then `label`.
    pub active_label: String,
    pub capitalize: Capitalize,
    pub special_icons: HashMap<String, String>,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            shown: 0,
            per_monitor: false,
            show_special: true,
            window_icons: false,
            max_window_icons: 4,
            occupied_background: true,
            indicator: true,
            indicator_trail: 0.35,
            scroll: true,
            label: "{id}".to_string(),
            occupied_label: String::new(),
            active_label: String::new(),
            capitalize: Capitalize::default(),
            special_icons: HashMap::new(),
        }
    }
}

impl WorkspacesConfig {
    /// The trail fraction, bounded: at `1` the box would reach the whole way to its goal on every frame and read as one long bar rather than as a pill in motion, and the indicator has to be off entirely for a trail to mean nothing.
    pub fn trail(&self) -> f32 {
        if !self.indicator || !self.indicator_trail.is_finite() {
            return 0.0;
        }
        self.indicator_trail.clamp(0.0, 0.9)
    }

    /// The template a pill in this state renders from: the most specific one the user set, falling back to the general `label` so setting only `active_label` leaves every other pill alone.
    fn template(&self, occupied: bool, active: bool) -> &str {
        let specific = if active {
            [&self.active_label, &self.occupied_label]
        } else if occupied {
            [&self.occupied_label, &self.label]
        } else {
            [&self.label, &self.label]
        };
        specific
            .into_iter()
            .find(|t| !t.trim().is_empty())
            .unwrap_or(&self.label)
    }

    /// Renders a pill's label from the template for its state, then applies `capitalize`. `{index}` is the pill's position, which is what a fixed-width bar wants when the ids themselves are sparse.
    pub fn render_label(
        &self,
        id: i32,
        name: &str,
        index: usize,
        occupied: bool,
        active: bool,
    ) -> String {
        let rendered = self
            .template(occupied, active)
            .replace("{id}", &id.to_string())
            .replace("{name}", name)
            .replace("{index}", &(index + 1).to_string());
        self.capitalize.apply(&rendered)
    }
}

/// Which scale a temperature is shown in.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    pub fn from_celsius(self, celsius: f32) -> f32 {
        match self {
            TemperatureUnit::Celsius => celsius,
            TemperatureUnit::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            TemperatureUnit::Celsius => "°C",
            TemperatureUnit::Fahrenheit => "°F",
        }
    }

    /// A whole degree plus its unit — the reading a bar chip has room for, and unambiguous once a user has switched scales.
    pub fn format(self, celsius: f32) -> String {
        format!("{:.0}{}", self.from_celsius(celsius), self.suffix())
    }
}

/// The `temperature` module. `sensor` names an hwmon chip (`k10temp`, `coretemp`) or a sensor label (`Tctl`, `Package id 0`) to follow; empty tracks the hottest sensor on the machine, which is what works without a per-machine config. `warn`/`critical` are the °C the chip tints amber and red at — a desktop CPU that idles at 65 °C should not show a permanent warning, so they are the user's numbers.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct TemperatureConfig {
    pub unit: TemperatureUnit,
    pub sensor: String,
    pub warn: f32,
    pub critical: f32,
}

impl Default for TemperatureConfig {
    fn default() -> Self {
        Self {
            unit: TemperatureUnit::default(),
            sensor: String::new(),
            warn: 70.0,
            critical: 85.0,
        }
    }
}

/// How hot a reading is against `[temperature] warn` and `critical`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warn,
    Critical,
}

impl TemperatureConfig {
    pub fn severity(&self, celsius: f32) -> Severity {
        if celsius >= self.critical {
            Severity::Critical
        } else if celsius >= self.warn {
            Severity::Warn
        } else {
            Severity::Normal
        }
    }
}

/// One charge level worth interrupting the user about, declared as a `[[battery.warn_levels]]` table. It fires once as the charge crosses down through `level` while discharging, and re-arms once the battery is charging again — so a laptop left at 19 % does not warn every minute.
///
/// `title` and `message` left empty take the shell's own translated text, so the defaults follow the UI language instead of pinning English into everyone's config; `{level}` in either is replaced with the charge at the moment it fired.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct BatteryWarning {
    pub level: i32,
    pub title: String,
    pub message: String,
    pub icon: String,
    /// Raise it at `Critical` urgency, so with the default `critical_sticky` it waits to be read rather than timing out behind whatever the user is doing.
    pub critical: bool,
}

impl Default for BatteryWarning {
    fn default() -> Self {
        Self {
            level: 20,
            title: String::new(),
            message: String::new(),
            icon: "battery-low".to_string(),
            critical: false,
        }
    }
}

impl BatteryWarning {
    /// The notification title: the user's own text, else the shell's translated default.
    pub fn title(&self, level: i32) -> String {
        let configured = self.title.trim();
        if configured.is_empty() {
            telar::t!("battery.warning.title")
        } else {
            configured.replace("{level}", &level.to_string())
        }
    }

    pub fn message(&self, level: i32) -> String {
        let configured = self.message.trim();
        if configured.is_empty() {
            telar::t!("battery.warning.body", level = level.to_string())
        } else {
            configured.replace("{level}", &level.to_string())
        }
    }
}

/// Low-battery behaviour (`[battery]`): the levels that raise a notification, and the action to take once the charge is low enough that the machine should put itself away. On a desktop none of it ever fires, since there is no battery to read.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct BatteryConfig {
    pub enabled: bool,
    pub warn_levels: Vec<BatteryWarning>,
    /// Charge at or below which `critical_action` runs; `0` (the default) never acts on the user's behalf.
    pub critical_level: i32,
    /// A `session` action id — `suspend`, `hibernate`, `shutdown`; empty runs nothing.
    pub critical_action: String,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Two warnings out of the box: a laptop shell that silently runs a battery flat is a bug, and a desktop never reaches this code because it has no battery to report.
            warn_levels: vec![
                BatteryWarning::default(),
                BatteryWarning {
                    level: 10,
                    icon: "battery-warning".to_string(),
                    critical: true,
                    ..BatteryWarning::default()
                },
            ],
            critical_level: 0,
            critical_action: String::new(),
        }
    }
}

/// The `lockstatus` module: caps- and num-lock indicators. `hide_inactive` shows an indicator only while its key is engaged, for a bar that should stay quiet; off (the default) keeps both glyphs in place, muted, so the module is visible the moment it is added and the bar's width never shifts.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct LockStatusConfig {
    pub caps: bool,
    pub num: bool,
    pub hide_inactive: bool,
}

impl Default for LockStatusConfig {
    fn default() -> Self {
        Self {
            caps: true,
            num: true,
            hide_inactive: false,
        }
    }
}

/// The system tray (`[tray]`).
///
/// `hidden` drops an application's icon by its `Id`, and `icon_subs` swaps one for an Iconify glyph so an application shipping a mismatched icon can be made to sit with the rest of the bar. Both match the id as a `*` pattern rather than a literal, because a good number of applications bury a PID in theirs.
///
/// `recolour` tints every icon to the bar's foreground. Coherent, but it flattens an application that uses colour to report state — a sync client going red — so it stays off unless asked for.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct TrayConfig {
    /// Off costs nothing: the module draws nothing and the service — three threads and a D-Bus name — is never started.
    pub enabled: bool,
    /// Drop the spacing between icons, for a bar with many of them.
    pub compact: bool,
    pub recolour: bool,
    /// Give every icon its own chip background instead of one shared strip.
    pub background: bool,
    pub hidden: Vec<String>,
    pub icon_subs: HashMap<String, String>,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compact: false,
            recolour: false,
            background: false,
            hidden: Vec::new(),
            icon_subs: HashMap::new(),
        }
    }
}

impl TrayConfig {
    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|p| glob_matches(p, id))
    }

    /// The Iconify glyph standing in for this application's own icon, if one is configured. The most specific pattern wins, so a blanket `*` can set a default without shadowing the entry that names one application — and the answer never depends on the map's iteration order.
    pub fn icon_sub_for(&self, id: &str) -> Option<&str> {
        self.icon_subs
            .iter()
            .filter(|(pattern, _)| glob_matches(pattern, id))
            .max_by_key(|(pattern, _)| pattern.trim_matches('*').len())
            .map(|(_, glyph)| glyph.as_str())
    }
}

/// The `clock` module and its panel. `format` and `date_format` are `strftime` patterns, so a user can have anything from `%H:%M` to a full locale date without the shell enumerating presets; `twelve_hour` is the one switch worth naming, since it is what most people actually mean by "change the clock format".
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct ClockConfig {
    pub twelve_hour: bool,
    /// Overrides `twelve_hour` when set, for a user who wants seconds, a weekday, or anything else.
    pub format: Option<String>,
    pub show_date: bool,
    pub date_format: String,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            twelve_hour: false,
            format: None,
            show_date: false,
            date_format: "%a %d %b".to_string(),
        }
    }
}

impl ClockConfig {
    /// The `strftime` pattern the chip renders: the explicit override, else a 12- or 24-hour clock with seconds.
    pub fn time_format(&self) -> &str {
        if let Some(format) = &self.format {
            return format;
        }
        if self.twelve_hour {
            "%I:%M:%S %p"
        } else {
            "%H:%M:%S"
        }
    }
}
