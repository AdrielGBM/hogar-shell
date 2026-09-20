//! What a layout file says: which areas live on which layer of which output, what is inside them, and what each instance is.
//!
//! These are the *file* types, and every field a level can override is an `Option` or a list, because the same types are written at four levels — the built-in default, an `extends` parent, an output rule and a workspace rule — and a level says only what it changes. A level that adds an area fills the fields the area needs; a level that nudges one writes the id and that field alone. [`crate::resolve`] walks the levels and turns the result into [`crate::resolve::Resolved`], where every field is answered, and a field still missing there is a located validation error rather than a silent default.
//!
//! Ids are the merge key at every level, which is what lets a monitor rule say "this one also has a clock" without restating the bar. They are generated once, never reused, and an undo restores the same id, so IPC, rules and panels can address an instance by name for as long as it exists.

use config::{Edge, Shape};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Arc<str>);

        impl $name {
            pub fn new(id: impl AsRef<str>) -> Self {
                Self(Arc::from(id.as_ref()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
                out.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
                String::deserialize(input).map(Self::new)
            }
        }
    };
}

id_type!(
    /// The name a layout is stored and selected under: the stem of `layouts/<name>.toml`.
    LayoutId
);
id_type!(
    /// Identifies an area inside one layer. Unique per layer, not globally, so two outputs can both have a `bar-top`.
    AreaId
);
id_type!(
    /// Identifies a group inside one area.
    GroupId
);
id_type!(
    /// Identifies one placed module. Unique across the whole layout, because IPC, rules and panels address instances by it without naming the area they sit in.
    InstanceId
);

/// One named arrangement of everything the shell draws.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub id: LayoutId,
    /// What the user sees in the layout list. Falls back to `id` when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// A layout this one starts from. The chain is applied root first, so this layout's own rules win.
    pub extends: Option<LayoutId>,
    /// Output rules in file order. Every one whose glob matches is applied, most-specific glob last, so a `*` rule is the base and a named monitor refines it.
    pub outputs: Vec<OutputRule>,
}

impl Default for LayoutId {
    fn default() -> Self {
        Self::new("default")
    }
}

/// An id a level left out. It is never a usable id: validation reports it with the path it sits at, which is how a hand-written area with no `id` becomes a message instead of a silently unaddressable item.
macro_rules! empty_default {
    ($name:ident) => {
        impl Default for $name {
            fn default() -> Self {
                Self::new("")
            }
        }
    };
}

empty_default!(AreaId);
empty_default!(GroupId);
empty_default!(InstanceId);

/// What an output, or every output, is arranged like.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputRule {
    /// `*` for every output, or a glob over the connector name (`DP-*`, `eDP-1`).
    #[serde(rename = "match")]
    pub matches: OutputMatch,
    pub layers: Layers,
    /// Per-workspace refinements of this output. They may change contents and non-reserving areas only, so switching workspaces never re-tiles windows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceRule>,
}

/// Which outputs an [`OutputRule`] speaks for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct OutputMatch(pub String);

impl Default for OutputMatch {
    fn default() -> Self {
        Self("*".into())
    }
}

impl OutputMatch {
    pub fn is_every_output(&self) -> bool {
        self.0 == "*"
    }

    /// How narrow the pattern is, so that rules can be applied broadest first. A pattern with no wildcard is the most specific; otherwise the more literal characters it has, the more specific it is.
    ///
    /// `*` is the only wildcard, because the matching itself is `config::glob_matches` — the same globber `[tray] icon_subs` and `excluded_screens` already use. A second vocabulary here would make a pattern narrow by one rule and broad by the other.
    pub fn specificity(&self) -> (bool, usize) {
        (!self.0.contains('*'), self.0.len())
    }
}

/// A refinement that applies only while a given workspace is active on the output.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceRule {
    /// A workspace name, `id:<n>` or `special:<name>`. The prefixed forms need Hyprland; without it the rule is reported as inactive instead of being silently dropped.
    #[serde(rename = "match")]
    pub matches: WorkspaceMatch,
    /// The four wlr layers. `lock` is absent on purpose: no workspace is visible while the session is locked.
    pub layers: SessionLayers,
}

/// Which workspace a [`WorkspaceRule`] speaks for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct WorkspaceMatch(pub String);

/// What a workspace match needs in order to be answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMatchKind {
    /// Matched on the name every compositor reports through `ext-workspace-v1`.
    Name,
    /// Matched on the numeric id, which only Hyprland reports.
    Id,
    /// Matched on a special (scratchpad) workspace, which only Hyprland reports.
    Special,
}

impl WorkspaceMatch {
    pub fn kind(&self) -> WorkspaceMatchKind {
        if self.0.starts_with("id:") {
            WorkspaceMatchKind::Id
        } else if self.0.starts_with("special:") {
            WorkspaceMatchKind::Special
        } else {
            WorkspaceMatchKind::Name
        }
    }

    /// The part after the prefix, which is what the match is actually against.
    pub fn value(&self) -> &str {
        match self.kind() {
            WorkspaceMatchKind::Name => &self.0,
            WorkspaceMatchKind::Id => &self.0["id:".len()..],
            WorkspaceMatchKind::Special => &self.0["special:".len()..],
        }
    }
}

/// The five layers of one output. The first four are wlr layer-shell layers, one window each; `lock` is the same model on `ext-session-lock-v1` surfaces.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layers {
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub background: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub desktop: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub top: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub overlay: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub lock: Layer,
}

/// The four layers a workspace rule may refine.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionLayers {
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub background: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub desktop: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub top: Layer,
    #[serde(skip_serializing_if = "Layer::is_empty")]
    pub overlay: Layer,
}

/// Which of the five layers something is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    Background,
    Desktop,
    Top,
    Overlay,
    Lock,
}

impl LayerKind {
    /// Every layer, bottom first, which is also the order windows stack in.
    pub const ALL: [LayerKind; 5] = [
        LayerKind::Background,
        LayerKind::Desktop,
        LayerKind::Top,
        LayerKind::Overlay,
        LayerKind::Lock,
    ];

    /// The four layers that exist while the session is unlocked.
    pub const SESSION: [LayerKind; 4] = [
        LayerKind::Background,
        LayerKind::Desktop,
        LayerKind::Top,
        LayerKind::Overlay,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LayerKind::Background => "background",
            LayerKind::Desktop => "desktop",
            LayerKind::Top => "top",
            LayerKind::Overlay => "overlay",
            LayerKind::Lock => "lock",
        }
    }

    /// The layer-shell namespace of this layer's window. `hogar-shell-bottom` is avoided for the desktop layer because "bottom" means the bottom bar to a user reading their own compositor rules.
    pub fn namespace(self) -> Option<&'static str> {
        match self {
            LayerKind::Background => Some("hogar-shell-background"),
            LayerKind::Desktop => Some("hogar-shell-desktop"),
            LayerKind::Top => Some("hogar-shell-top"),
            LayerKind::Overlay => Some("hogar-shell-overlay"),
            LayerKind::Lock => None,
        }
    }
}

impl fmt::Display for LayerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One layer's areas. Their order is their z-order inside the layer's window.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layer {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<Area>,
    /// Ids of areas an earlier level placed that this one takes away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<AreaId>,
}

impl Layer {
    /// Whether this layer says nothing at all, which is what keeps an untouched layer out of a written file.
    pub fn is_empty(&self) -> bool {
        self.areas.is_empty() && self.remove.is_empty()
    }
}

/// A region of a layer that holds instances and has a geometry of its own.
///
/// Unknown keys are kept in `unknown` rather than refused outright, because the geometry is flattened into this table and serde cannot both flatten and reject. Keeping them means validation can still say `thikness is not a key of a bar area`, which is what a typo needs, instead of the key being silently dropped.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Area {
    pub id: AreaId,
    /// What kind of region this is. Required the first time the area is named; absent when a later level only adjusts one that already exists.
    #[serde(flatten)]
    pub kind: Option<AreaKind>,
    /// Whether the compositor keeps windows out of this area's edge. Only an output-level area may reserve: a workspace rule that changes this is rejected, so switching workspaces never re-tiles windows.
    pub reserve: Option<bool>,
    /// Whether this area stays visible over a fullscreen window, which costs direct scanout on that output for as long as it is mapped.
    pub above_fullscreen: Option<bool>,
    /// Which box this area's geometry is measured in: the whole output, or what is left of it once the reserving areas have taken their edges.
    pub within: Option<Within>,
    #[serde(skip_serializing_if = "AreaStyle::is_empty")]
    pub style: AreaStyle,
    /// An expression that decides whether the area draws. An invisible area contributes nothing to the input region.
    pub visible: Option<Expr>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// Ids of groups an earlier level placed that this one takes away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<GroupId>,
}

/// What kind of region an area is, and the geometry that kind needs, as a level wrote it.
///
/// Every field is optional so that a level can change one of them and inherit the rest: a monitor rule that makes one bar thicker names the area, repeats `kind = "bar"` so the variant is unambiguous, and writes `thickness` alone. Two levels that disagree about the variant do not merge — the later one replaces the area's geometry outright, because a grid and a bar share no fields worth carrying over. [`crate::resolve::ResolvedAreaKind`] is the answered form, and a field still missing when the area is resolved is a located error naming it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AreaKind {
    /// A strip along one edge: today's bar.
    Bar {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        /// How thick the strip is, across the edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f32>,
        /// How far it runs along the edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<Extent>,
        /// Where it starts along the edge, as a fraction of the edge, so several bars can share one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<f32>,
        #[serde(default, skip_serializing_if = "BarShape::is_empty")]
        shape: BarShape,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        autohide: Option<AutoHide>,
    },
    /// A grid of cells that widgets are placed into by explicit coordinates.
    Grid {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cell: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap: Option<f32>,
        /// Where the cells sit inside `rect` when they do not fill it. A grid of one widget is the common case, and without this it could only ever sit in the corner its origin is at.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<Anchor>,
    },
    /// A column that notification, toast and OSD cards are routed into.
    Stack {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<Anchor>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_policy: Option<StackOutputPolicy>,
        /// Which cards land here. A card goes to the first stack whose routes accept it; a stack with no routes accepts everything.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        routes: Vec<Route>,
    },
    /// A region of the output that draws a wallpaper of its own.
    WallpaperRegion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<Fit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition: Option<Transition>,
    },
    /// An image or gradient painted over what is behind it.
    Texture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gradient: Option<Gradient>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tile: Option<Tile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blend: Option<Blend>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f32>,
    },
    /// A strip that hugs an edge and sizes itself to its contents, like today's visualiser.
    Dock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f32>,
    },
    /// A rectangle placed by hand.
    Free {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
    },
    /// The lock layer's password field, status line and biometric hint. Exactly one exists per output and it can never be removed or hidden.
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        #[serde(default, skip_serializing_if = "PromptStyle::is_empty")]
        style: PromptStyle,
    },
}

impl AreaKind {
    pub fn name(&self) -> &'static str {
        match self {
            AreaKind::Bar { .. } => "bar",
            AreaKind::Grid { .. } => "grid",
            AreaKind::Stack { .. } => "stack",
            AreaKind::WallpaperRegion { .. } => "wallpaper_region",
            AreaKind::Texture { .. } => "texture",
            AreaKind::Dock { .. } => "dock",
            AreaKind::Free { .. } => "free",
            AreaKind::Prompt { .. } => "prompt",
        }
    }

    /// Whether two levels are talking about the same kind of region, and therefore whether their fields merge or the later one replaces the earlier outright.
    pub fn is_same_kind(&self, other: &AreaKind) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// The box an area's geometry is measured against.
///
/// Every layer's window is the whole output and they share one coordinate space, so a fraction has to say which box it is a fraction *of*. It matters because the two answers disagree exactly where a user notices: a wallpaper belongs under the bars and so is measured against the output, while a clock placed at the bottom right means the bottom right of the space the bars left — measured against the output it would sit underneath one.
///
/// This is why it is written down rather than inferred from the layer. Inferring it would make the same `rect = { … }` mean different things on the background and the desktop, which is the kind of rule nobody can predict without reading the code that implements it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Within {
    /// The whole output, edge to edge, under anything that reserves.
    #[default]
    Output,
    /// What is left of the output once every reserving area has taken its edge. Follows a bar that changes thickness, and differs per monitor, which a fraction baked at one size could not.
    Usable,
}

/// How far an area runs along its edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Extent {
    /// The whole edge, minus whatever the adjacent edges already took.
    #[default]
    Fill,
    /// A fixed number of logical pixels.
    Px(f32),
    /// A fraction of the edge, so the same layout fits any monitor.
    Fraction(f32),
}

/// A rectangle in fractions of the output, so one layout describes every monitor. `0,0` is the top left corner and `1,1` the bottom right.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for Rect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

impl Rect {
    /// Whether the rectangle is a real region that lies on the output it describes.
    pub fn is_on_output(&self) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && self.x >= 0.0
            && self.y >= 0.0
            && self.x + self.w <= 1.0 + f32::EPSILON
            && self.y + self.h <= 1.0 + f32::EPSILON
    }
}

/// One of the nine places a stack or a free area can be pinned to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

/// Which outputs a stack appears on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackOutputPolicy {
    /// Follow the focused output, which is how the one notification column behaves today.
    #[default]
    Focused,
    /// Stay on the output this rule describes.
    Here,
    /// Appear on every output at once.
    All,
}

/// Which cards a stack accepts. An empty field matches anything.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Route {
    pub kind: Option<CardKind>,
    /// The application id a notification came from.
    pub app: Option<String>,
    pub urgency: Option<Urgency>,
}

impl Route {
    pub fn is_empty(&self) -> bool {
        self.kind.is_none() && self.app.is_none() && self.urgency.is_none()
    }
}

/// What kind of transient card the stack is routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Notification,
    Toast,
    Osd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

/// How a wallpaper fills its region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
    Stretch,
    Tile,
}

/// How a wallpaper changes to the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    None,
    #[default]
    Fade,
    Slide,
}

/// A gradient with its stops in order along `angle`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gradient {
    /// Degrees clockwise from a left-to-right sweep.
    pub angle: f32,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GradientStop {
    /// Where along the gradient this stop sits, from 0 to 1.
    pub at: f32,
    /// A theme token name or a hex colour, the same vocabulary `[theme.colors]` uses.
    pub color: String,
}

/// How an image repeats inside its rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tile {
    #[default]
    None,
    Repeat,
    /// Stretch the middle and keep the corners, for a border or a frame image. The four insets are in the image's own pixels and say where its corners end, which is what the renderer needs and what a slice cannot be drawn without.
    NineSlice {
        top: f32,
        right: f32,
        bottom: f32,
        left: f32,
    },
}

/// How a texture combines with what is behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
}

/// Per-area appearance. Every field is optional because the theme answers whatever an area does not.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AreaStyle {
    /// A theme token name or a hex colour.
    pub fill: Option<String>,
    pub radius: Option<f32>,
    pub opacity: Option<f32>,
    pub padding: Option<f32>,
    pub backdrop: Option<Backdrop>,
}

impl AreaStyle {
    pub fn is_empty(&self) -> bool {
        self.fill.is_none()
            && self.radius.is_none()
            && self.opacity.is_none()
            && self.padding.is_none()
            && self.backdrop.is_none()
    }
}

/// What happens to what is behind an area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backdrop {
    #[default]
    None,
    /// Asks the compositor to blur behind this area through `ext-background-effect-v1`. Without the protocol the area stays translucent and unblurred, and the report says so.
    Blur,
}

/// A bar's own shape, overriding the global `[shape]` where it is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarShape {
    pub mode: Option<Shape>,
    pub gap: Option<f32>,
    pub spacing: Option<f32>,
    pub radius: Option<f32>,
}

impl BarShape {
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.gap.is_none() && self.spacing.is_none() && self.radius.is_none()
    }
}

/// A bar that hides itself off its edge and comes back when the pointer reaches the strip it leaves behind.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoHide {
    /// How much of the bar stays on screen while it is hidden, and therefore how big the rectangle that reveals it is.
    pub peek: f32,
    /// Whether resting the pointer on that strip is enough to bring the bar back. With it off the bar is revealed by pulling inward from the edge instead, which is a deliberate gesture rather than something a pointer crossing the screen can trigger.
    pub on_hover: bool,
}

impl Default for AutoHide {
    fn default() -> Self {
        Self {
            peek: 2.0,
            on_hover: true,
        }
    }
}

/// How the lock layer's prompt is drawn.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptStyle {
    pub fill: Option<String>,
    pub radius: Option<f32>,
    /// Kept at or above 0.9 by validation: a prompt faded into its background is a lockout.
    pub opacity: Option<f32>,
}

impl PromptStyle {
    pub fn is_empty(&self) -> bool {
        self.fill.is_none() && self.radius.is_none() && self.opacity.is_none()
    }
}

/// A run of instances inside an area, and how the area places it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Group {
    pub id: GroupId,
    #[serde(flatten)]
    pub kind: Option<GroupKind>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Instance>,
    /// Ids of instances an earlier level placed that this one takes away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<InstanceId>,
}

/// Where in its area a group sits.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "place", rename_all = "snake_case")]
pub enum GroupKind {
    /// One of a bar's three runs.
    Zone { zone: Zone },
    /// An explicit cell in a grid, which is what keeps a widget where it was put instead of reflowing it.
    Cell {
        col: u32,
        row: u32,
        #[serde(default = "one_span", skip_serializing_if = "is_one_span")]
        col_span: u32,
        #[serde(default = "one_span", skip_serializing_if = "is_one_span")]
        row_span: u32,
    },
    /// Several instances stacked in one footprint, shown one at a time.
    SmartStack,
}

fn one_span() -> u32 {
    1
}

/// A span of one is what a cell already is, so writing it says nothing. Six of them on an imported lock layer is six lines between a reader and the two that matter.
fn is_one_span(span: &u32) -> bool {
    *span == 1
}

/// Which run of a bar a group is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Start,
    Center,
    End,
}

/// One placed module.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Instance {
    pub id: InstanceId,
    /// The module descriptor this instance shows. Required the first time the instance is named.
    pub module: Option<String>,
    pub representation: Option<Representation>,
    /// Option overrides for this instance alone, on top of the module's global defaults.
    #[serde(skip_serializing_if = "toml::Table::is_empty")]
    pub options: toml::Table,
    /// Properties driven by an expression instead of a fixed value, keyed by the property's path.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Expr>,
    /// What each gesture on this instance runs.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<Trigger, Action>,
}

/// How big a placed module is drawn, which is the same module seen at a different size rather than a different module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    Chip,
    #[serde(rename = "widget_s")]
    WidgetS,
    #[serde(rename = "widget_m")]
    WidgetM,
    #[serde(rename = "widget_l")]
    WidgetL,
    Card,
}

impl Representation {
    pub fn as_str(self) -> &'static str {
        match self {
            Representation::Chip => "chip",
            Representation::WidgetS => "widget_s",
            Representation::WidgetM => "widget_m",
            Representation::WidgetL => "widget_l",
            Representation::Card => "card",
        }
    }
}

/// What sets an action off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Press,
    LongPress,
    ScrollUp,
    ScrollDown,
    Middle,
    Secondary,
}

/// A chain of shell commands a trigger runs. Each line is an IPC command validated against the command table when the layout loads, so a typo is a located error rather than a gesture that silently does nothing.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Action(pub Vec<String>);

/// A bound expression, held as written.
///
/// The evaluator arrives with the data and automation sprint. Until then an expression parses, round-trips and is reported where a layer forbids it, which is what lets the model be complete before the language exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Expr(pub String);

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
