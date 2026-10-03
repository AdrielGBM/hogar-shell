//! What a layout file says: which areas live on which layer of which output, what is inside them, and what each instance is.
//!
//! These are the *file* types, and every field a level can override is an `Option` or a list, because the same types are written at four levels — the built-in default, an `extends` parent, an output rule and a workspace rule — and a level says only what it changes. A level that adds an area fills the fields the area needs; a level that nudges one writes the id and that field alone. [`crate::resolve`] walks the levels and turns the result into [`crate::resolve::Resolved`], where every field is answered, and a field still missing there is a located validation error rather than a silent default.
//!
//! Ids are the merge key at every level, which is what lets a monitor rule say "this one also has a clock" without restating the bar. They are generated once, never reused, and an undo restores the same id, so IPC, rules and panels can address an instance by name for as long as it exists.

use config::theme::NordTheme;
use config::{Edge, Shape};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use telar::Color;

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

/// What separates a repeated child's id from the index of one of its copies: `player#2`. Refused in a written id, so a copy can never share an id with a child.
pub const COPY_MARK: char = '#';

impl InstanceId {
    /// The copy at `index` of this child of a group with `repeat`.
    pub fn copy(&self, index: usize) -> Self {
        Self::new(format!("{self}{COPY_MARK}{index}"))
    }

    /// The child as the layout writes it: this id, or the one it is a copy of.
    pub fn template(&self) -> Self {
        match self.0.rsplit_once(COPY_MARK) {
            Some((template, index)) if index.parse::<usize>().is_ok() => Self::new(template),
            _ => self.clone(),
        }
    }

    /// Which copy this is, for a copy of a repeated child.
    pub fn copy_index(&self) -> Option<usize> {
        self.0.rsplit_once(COPY_MARK)?.1.parse().ok()
    }
}

/// One named arrangement of everything the shell draws.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// The name everything addresses this layout by — `layout use`, `extends`, the last-good copy. It comes from the file name, so writing something else here changes nothing.
    pub id: LayoutId,
    /// What the user sees in the layout list. Falls back to `id` when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// A layout this one starts from. The chain is applied root first, so this layout's own rules win.
    pub extends: Option<LayoutId>,
    /// Readings this layout declares itself, by the name an expression reads each as (`$name`): a command run on an interval, a command that prints a line per update, or an address fetched on an interval. Merged by name along `extends`, so a layout can change one key of a source it inherits. A name a module's own source already has is an error.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, Source>,
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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

    /// Whether this rule speaks for the output called `output`.
    pub fn matches(&self, output: &str) -> bool {
        self.is_every_output() || config::glob_matches(&self.0, output)
    }

    /// How narrow the pattern is, so that rules can be applied broadest first. A pattern with no wildcard is the most specific; otherwise the more literal characters it has, the more specific it is.
    ///
    /// `*` is the only wildcard, because the matching itself is `config::glob_matches` — the same globber `[tray] icon_subs` and `excluded_screens` already use. A second vocabulary here would make a pattern narrow by one rule and broad by the other.
    pub fn specificity(&self) -> (bool, usize) {
        (!self.0.contains('*'), self.0.len())
    }
}

/// A refinement that applies only while a given workspace is active on the output.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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

impl Layers {
    pub fn get(&self, kind: LayerKind) -> &Layer {
        match kind {
            LayerKind::Background => &self.background,
            LayerKind::Desktop => &self.desktop,
            LayerKind::Top => &self.top,
            LayerKind::Overlay => &self.overlay,
            LayerKind::Lock => &self.lock,
        }
    }

    pub fn get_mut(&mut self, kind: LayerKind) -> &mut Layer {
        match kind {
            LayerKind::Background => &mut self.background,
            LayerKind::Desktop => &mut self.desktop,
            LayerKind::Top => &mut self.top,
            LayerKind::Overlay => &mut self.overlay,
            LayerKind::Lock => &mut self.lock,
        }
    }

    /// Every layer with the kind it is, bottom first — which is the order the windows stack in, and the order anything walking a whole level has to take them in.
    pub fn each(&self) -> [(LayerKind, &Layer); 5] {
        LayerKind::ALL.map(|kind| (kind, self.get(kind)))
    }
}

/// The four layers a workspace rule may refine.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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

impl SessionLayers {
    /// `None` for the lock layer, which a workspace rule does not have.
    pub fn get(&self, kind: LayerKind) -> Option<&Layer> {
        match kind {
            LayerKind::Background => Some(&self.background),
            LayerKind::Desktop => Some(&self.desktop),
            LayerKind::Top => Some(&self.top),
            LayerKind::Overlay => Some(&self.overlay),
            LayerKind::Lock => None,
        }
    }

    pub fn get_mut(&mut self, kind: LayerKind) -> Option<&mut Layer> {
        match kind {
            LayerKind::Background => Some(&mut self.background),
            LayerKind::Desktop => Some(&mut self.desktop),
            LayerKind::Top => Some(&mut self.top),
            LayerKind::Overlay => Some(&mut self.overlay),
            LayerKind::Lock => None,
        }
    }

    /// Every layer a workspace rule may refine, bottom first. The lock layer is not one of them: no workspace is visible while the screen is locked.
    pub fn each(&self) -> [(LayerKind, &Layer); 4] {
        [
            (LayerKind::Background, &self.background),
            (LayerKind::Desktop, &self.desktop),
            (LayerKind::Top, &self.top),
            (LayerKind::Overlay, &self.overlay),
        ]
    }
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

    /// The layer a command or a file named, by the one spelling [`LayerKind::as_str`] gives it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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
/// Its geometry is written *in this table*, beside the keys below: `kind` says which kind of region it is and the keys that kind has follow it. A key no kind of area has is reported with its line and column rather than ignored — see `layout check`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Area {
    /// What a rule, a command or another level of the same layout addresses this area by. Unique within its layer.
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
    /// An expression giving true or false that decides whether the area draws: while it is false the area paints nothing and takes no input, and nothing else on its layer is rebuilt when it flips. Shown until it first answers, and through an evaluation error after that it keeps its last answer. Not allowed on the lock prompt.
    pub visible: Option<Expr>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// Ids of groups an earlier level placed that this one takes away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<GroupId>,
    /// What each gesture on the area's own background runs — a press or a scroll that lands between its instances rather than on one. Refused on the lock layer, which holds readings, never controls.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<Trigger, Action>,
    /// Expressions a level under this one gave the area that this level takes back, as though nothing under it had written them: `unset = ["visible"]` shows on one monitor an area a broader rule hides behind an expression. An area takes back `visible`. They are taken back before this level's own keys apply, so a level that takes a key back and writes it too is an error, and one that takes back what nothing under it writes is reported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<Unset>,
}

/// What kind of region an area is, and the geometry that kind needs, as a level wrote it.
///
/// Every field is optional so that a level can change one of them and inherit the rest: a monitor rule that makes one bar thicker names the area, repeats `kind = "bar"` so the variant is unambiguous, and writes `thickness` alone. Two levels that disagree about the variant do not merge — the later one replaces the area's geometry outright, because a grid and a bar share no fields worth carrying over. [`crate::resolve::ResolvedAreaKind`] is the answered form, and a field still missing when the area is resolved is a located error naming it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AreaKind {
    /// A strip along one edge: today's bar.
    Bar {
        /// Which edge of the output the strip hugs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        /// How thick the strip is, across the edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f32>,
        /// How far it runs along the edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<Extent>,
        /// Where it starts along the edge, in pixels from the edge's start, so several bars can share one.
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
        /// How big one cell is, in logical pixels. A widget covers whole cells, so this is what decides how big every widget on this grid is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cell: Option<f32>,
        /// The space between two cells.
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
        /// How far the column is moved from where its anchor puts it, in logical pixels. It never goes past the edge of the box it is measured in, and always keeps a quarter of that box's height for its cards.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<Offset>,
        /// How wide a card in this column is, in logical pixels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_policy: Option<StackOutputPolicy>,
        /// Which cards land here. A card goes to the first stack on its output with a route that takes it; a stack with no routes takes what no route on that output takes, the first such stack taking all of it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        routes: Vec<Route>,
        /// Whether the launcher opens here, at this column's anchor and offset, rather than in the middle of the screen. The first stack on an output that says so is the one used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launcher: Option<bool>,
    },
    /// A region of the output that draws a wallpaper of its own.
    WallpaperRegion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        /// A path to the picture this region shows, which `hogar-shell wallpaper set <path> --region <id>` writes. Left out or empty, it shows whatever `[background]` and a plain `wallpaper set` say — so changing the desktop's picture stays a config action, and only a region that names its own keeps it through one. An empty one is how a rule says so over a region another level gave a picture.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        /// How the picture is fitted to the region.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<Fit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition: Option<Transition>,
    },
    /// An image or gradient painted over what is behind it.
    Texture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
        /// A path to the image to paint. Exactly one of this and `gradient` is drawn; an area that names both is reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gradient: Option<Gradient>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tile: Option<Tile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blend: Option<Blend>,
        /// How opaque the paint is, from 0 to 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f32>,
    },
    /// A strip that hugs an edge and sizes itself to its contents, like today's visualiser.
    Dock {
        /// Which edge of the output the strip hugs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        /// How deep the strip is, across the edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f32>,
    },
    /// A rectangle placed by hand.
    Free {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
    },
    /// The lock layer's password field, status line and biometric hint. Exactly one exists per output and it can never be removed or hidden. Its card is drawn from the area's own `style`: `fill`, `radius` and `opacity`, the fill held to a contrast the field's text can be read on and the opacity to 0.9 or above.
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<Rect>,
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

    /// The rectangle kept wholly on its output and at least `smallest` on each side, moved back in rather than cut where it ran off an edge.
    pub fn kept_on_output(self, smallest: f32) -> Self {
        let w = self.w.clamp(smallest, 1.0);
        let h = self.h.clamp(smallest, 1.0);
        Self {
            x: self.x.clamp(0.0, 1.0 - w),
            y: self.y.clamp(0.0, 1.0 - h),
            w,
            h,
        }
    }
}

/// The output a layout is resolved against when no compositor named one — a session with one nameless screen, and every check that runs without a compositor. A `*` rule matches it and a rule naming a connector does not, which is the honest answer when there is no connector to name.
pub const NOMINAL_OUTPUT: &str = "";

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

impl Anchor {
    /// All nine, reading order, which is what a sweep over every anchor walks.
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];
}

/// How far something pinned to one of the nine anchors is moved from where the anchor puts it, in logical pixels: right and down are positive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Offset {
    /// How far right, or left where it is negative.
    pub x: f32,
    /// How far down, or up where it is negative.
    pub y: f32,
}

impl Offset {
    pub const ZERO: Offset = Offset { x: 0.0, y: 0.0 };
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
    /// Which kind of card this route accepts. Absent accepts every kind.
    pub kind: Option<CardKind>,
    /// The name of the application a notification came from, as the notification gives it, matched without regard to case.
    pub app: Option<String>,
    /// How insistent a notification has to be to land here.
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

/// One colour of a gradient, and where along it that colour sits.
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

/// How far each corner of a box is rounded, in logical pixels, clockwise from the top left.
///
/// Written as one number when every corner agrees and as `[top_left, top_right, bottom_right, bottom_left]` when they do not, so the common case stays the one number a person types and a layout written back keeps that shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corners([f32; 4]);

impl Corners {
    pub const fn all(radius: f32) -> Self {
        Self([radius; 4])
    }

    pub const fn each(top_left: f32, top_right: f32, bottom_right: f32, bottom_left: f32) -> Self {
        Self([top_left, top_right, bottom_right, bottom_left])
    }

    pub fn top_left(self) -> f32 {
        self.0[0]
    }

    pub fn top_right(self) -> f32 {
        self.0[1]
    }

    pub fn bottom_right(self) -> f32 {
        self.0[2]
    }

    pub fn bottom_left(self) -> f32 {
        self.0[3]
    }

    pub fn is_uniform(self) -> bool {
        self.0.iter().all(|corner| *corner == self.0[0])
    }

    pub fn largest(self) -> f32 {
        self.0.into_iter().fold(0.0, f32::max)
    }
}

impl From<Corners> for telar::BorderRadius {
    fn from(corners: Corners) -> Self {
        let [top_left, top_right, bottom_right, bottom_left] = corners.0;
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }
}

impl Serialize for Corners {
    fn serialize<S: serde::Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        match self.is_uniform() {
            true => out.serialize_f32(self.0[0]),
            false => self.0.serialize(out),
        }
    }
}

impl<'de> Deserialize<'de> for Corners {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        struct Written;

        impl<'de> serde::de::Visitor<'de> for Written {
            type Value = Corners;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "one radius, or four as [top_left, top_right, bottom_right, bottom_left]",
                )
            }

            fn visit_f64<E: serde::de::Error>(self, radius: f64) -> Result<Corners, E> {
                Ok(Corners::all(radius as f32))
            }

            fn visit_i64<E: serde::de::Error>(self, radius: i64) -> Result<Corners, E> {
                Ok(Corners::all(radius as f32))
            }

            fn visit_u64<E: serde::de::Error>(self, radius: u64) -> Result<Corners, E> {
                Ok(Corners::all(radius as f32))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Corners, A::Error> {
                let mut corners = [0.0; 4];
                for (at, corner) in corners.iter_mut().enumerate() {
                    *corner = seq
                        .next_element::<f32>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(at, &self))?;
                }
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(5, &self));
                }
                Ok(Corners(corners))
            }
        }

        input.deserialize_any(Written)
    }
}

/// Per-area appearance. Every field is optional because the theme answers whatever an area does not.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AreaStyle {
    /// A theme token name or a hex colour, painted across the area's whole box under what it holds. A bar paints it as its strip in place of the theme's base, and a wallpaper region shows it wherever its picture does not reach. Behind the lock's prompt it is the card the password field sits on, and one the field's text cannot be read on — below WCAG AA, 4.5:1 — is refused, because an unreadable prompt is a lockout too.
    pub fill: Option<String>,
    /// How far the corners are rounded, in logical pixels: one number for all four, or `[top_left, top_right, bottom_right, bottom_left]`. A wallpaper region or a texture is cut to it. A bar is rounded by its own `shape.radius`, so one written here on a bar is reported and not drawn.
    pub radius: Option<Corners>,
    /// How opaque the area's own paint is, from 0 to 1, whatever alpha its `fill` names: the fill, everything a bar paints — its strip, sections and resting chips — in place of `[theme] opacity`, a wallpaper region's picture, or a texture on top of its own `opacity`. What the area holds is drawn as it is. The lock's prompt is kept at 0.9 or above: a prompt faded into its background is a lockout.
    pub opacity: Option<f32>,
    /// How far the area holds its contents off its own edges. A bar that names none pads by half its spacing in `bar` mode and not at all in the others.
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

    /// The colour `fill` paints, at `opacity` where the area names one. `None` when it names no fill.
    pub fn paint(&self, theme: &NordTheme) -> Option<Color> {
        let fill = color_of(self.fill.as_deref()?, theme);
        Some(match self.opacity {
            Some(opacity) => fill.with_alpha(opacity.clamp(0.0, 1.0)),
            None => fill,
        })
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

/// A bar's shape: whether it is one surface, sections or chips, how far it floats and how round it is. What it leaves unset follows the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarShape {
    /// Whether the bar is one surface, three sections or a chip per module: `bar`, `sections` or `chips`.
    pub mode: Option<Shape>,
    /// How far the bar floats from its edge and from the screen's sides.
    pub gap: Option<f32>,
    /// The space between two modules on it.
    pub spacing: Option<f32>,
    /// How far its corners are rounded: one number for all four, or `[top_left, top_right, bottom_right, bottom_left]` so a bar that meets another at a corner can square that corner alone.
    pub radius: Option<Corners>,
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

/// The colour a layout names: a hex colour, or else a theme token — the vocabulary `fill` and a gradient stop share with `[theme.colors]`.
pub fn color_of(token_or_hex: &str, theme: &NordTheme) -> Color {
    Color::from_hex(token_or_hex).unwrap_or_else(|| theme.token(token_or_hex))
}

/// The card the lock paints behind its prompt, alpha included: the area's `fill`, or the theme's surface where it names none. The lock draws with it and validation measures the prompt's contrast against it, so the two can never be judging different cards.
pub fn prompt_card(style: &AreaStyle, theme: &NordTheme) -> Color {
    let fill = style
        .fill
        .as_deref()
        .map(|fill| color_of(fill, theme))
        .unwrap_or(theme.surface);
    // Validation already refuses anything fainter; a hand-edited file that got past it is clamped rather than drawn as written.
    fill.with_alpha(style.opacity.unwrap_or(1.0).clamp(FAINTEST_PROMPT, 1.0))
}

/// What the prompt's text is read against: its card over the lock's own opaque background, which is the colour a translucent card actually shows.
pub fn prompt_backdrop(style: &AreaStyle, theme: &NordTheme) -> Color {
    let card = prompt_card(style, theme);
    let under = theme.base;
    let mix = |over: f32, back: f32| over * card.a + back * (1.0 - card.a);
    Color::rgb(
        mix(card.r, under.r),
        mix(card.g, under.g),
        mix(card.b, under.b),
    )
}

/// The faintest a prompt may be drawn. Below this the field a user has to type into disappears into the wallpaper behind it.
pub const FAINTEST_PROMPT: f32 = 0.9;

/// The smallest a prompt may be, as a fraction of each side of its output. Anything smaller is a lockout on a large monitor as surely as a hidden one.
pub const SMALLEST_PROMPT: f32 = 0.05;

/// A run of instances inside an area, and how the area places it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Group {
    /// What another level of the same layout addresses this group by. Unique within its area, so two bars can each have an `end`.
    pub id: GroupId,
    /// Where in its area the group sits. `place` says which way, and the keys that way needs follow it.
    #[serde(flatten)]
    pub kind: Option<GroupKind>,
    /// Shows the group's instances one at a time, in the footprint of the largest, cycled by the wheel, the arrow keys or its dots, wherever `place` puts it. Off unless set.
    pub stacked: Option<bool>,
    /// An expression giving a list (`$notifications.apps`): the group's children are drawn once per item, in order, and each copy reads its item as `$item` and its place from 0 as `$index`. A copy is `<id>#<index>` where it is drawn — its own rect and its own state — while IPC and the editor address the child as written. In a stacked group the copies are its pages. Not allowed on a grid cell, whose footprint is fixed. Until the list first answers, and while it is empty, the group draws nothing; through an evaluation error it keeps its last list. On the lock layer a list the lock may not show reads as empty, so nothing is drawn.
    pub repeat: Option<Expr>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Instance>,
    /// Ids of instances an earlier level placed that this one takes away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<InstanceId>,
    /// Expressions a level under this one gave the group that this level takes back: `unset = ["repeat"]` draws its children once where a broader rule repeats them. A group takes back `repeat`, the way an area takes back `visible`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<Unset>,
}

/// Where in its area a group sits.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "place", rename_all = "snake_case")]
pub enum GroupKind {
    /// One of a bar's three runs.
    Zone { zone: Zone },
    /// An explicit cell in a grid, which is what keeps a widget where it was put instead of reflowing it.
    Cell {
        /// Which column of the grid the group starts at, counting from 0.
        col: u32,
        /// Which row it starts at.
        row: u32,
        /// How many columns it covers. A span of one is what a cell already is, so it is left out of a written layout.
        #[serde(default = "one_span", skip_serializing_if = "is_one_span")]
        col_span: u32,
        /// How many rows it covers.
        #[serde(default = "one_span", skip_serializing_if = "is_one_span")]
        row_span: u32,
    },
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Instance {
    /// What IPC, a rule and another level of the same layout address this placed module by. Unique across the whole layout, readable, and never reused — `clock`, then `clock-2`.
    pub id: InstanceId,
    /// The module descriptor this instance shows. Required the first time the instance is named.
    pub module: Option<String>,
    /// How big it is drawn: `chip`, `widget_s`, `widget_m`, `widget_l` or `card`. Defaults to `chip`.
    pub representation: Option<Representation>,
    /// Option overrides for this instance alone, over its module's defaults: any key of the module's own section (`[clock]` for a clock), and any of `[modules.<id>]` — `accent`, `variant`, `open` and the sizes of what it opens. A key the module does not have is an error.
    #[serde(skip_serializing_if = "toml::Table::is_empty")]
    pub options: toml::Table,
    /// Options driven by an expression instead of a fixed value, keyed by the option's path: any key `options` takes, of the type that option takes (`show_date = "$battery.level > 50"`), or `accent`, a colour (`accent = "mix($theme.accent, #f00, $cpu.usage / 100)"`). Each value is laid over `options` as it changes, and only this instance is drawn again. One that does not check is reported and left out; through an evaluation error a binding keeps its last value.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Expr>,
    /// What each gesture on this instance runs.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<Trigger, Action>,
    /// Bindings a level under this one gave the instance that this level takes back, each as `bindings.<path>`: `unset = ["bindings.accent"]` puts back the accent its options give it where a broader rule drives it by an expression. A path has to be one `bindings` could hold, the way an area takes back `visible`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<Unset>,
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
    /// Every size a layout can place. The descriptor table has two more, `Panel` and `Popout`, which are opened rather than placed.
    pub const ALL: [Representation; 5] = [
        Representation::Chip,
        Representation::WidgetS,
        Representation::WidgetM,
        Representation::WidgetL,
        Representation::Card,
    ];

    /// The size a command or a file named, by the one spelling [`Representation::as_str`] gives it.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == name)
    }

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

impl Trigger {
    pub const ALL: [Trigger; 6] = [
        Trigger::Press,
        Trigger::LongPress,
        Trigger::ScrollUp,
        Trigger::ScrollDown,
        Trigger::Middle,
        Trigger::Secondary,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Press => "press",
            Trigger::LongPress => "long_press",
            Trigger::ScrollUp => "scroll_up",
            Trigger::ScrollDown => "scroll_down",
            Trigger::Middle => "middle",
            Trigger::Secondary => "secondary",
        }
    }

    /// The gesture a command or a file named, by the one spelling [`Trigger::as_str`] gives it — which is also the key serde writes, since both come from the same snake case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == name)
    }
}

/// A chain of shell commands a trigger runs. Each line is an IPC command validated against the command table when the layout loads, so a typo is a located error rather than a gesture that silently does nothing.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Action(pub Vec<String>);

/// An expression in the shell's language (TA-6), held as written: `$battery.level < 20`, `mix($theme.accent, #f00, $cpu.usage / 100)`.
///
/// The model keeps the text, so a layout round-trips byte for byte and an edit never rewrites what the user typed. Validation compiles it against the names its layer may read and the type it drives, and reports a mistake at its key with the span inside it ([`crate::validate`]); a layer window binds it to live readings for as long as what it drives is built and the window is on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Expr(pub String);

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An expression a level takes back from the levels under it, written as the path of the key it takes back: `visible` on an area, `repeat` on a group, `bindings.<path>` on an instance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(from = "String", into = "String", rename_all = "snake_case")]
pub enum Unset {
    Visible,
    Repeat,
    Binding(String),
    /// A path that names no expression, kept as written so validation can say where it is. It takes nothing back.
    Unknown(String),
}

impl Unset {
    const BINDINGS: &str = "bindings.";

    pub fn binding(path: impl Into<String>) -> Self {
        Self::Binding(path.into())
    }
}

impl From<&str> for Unset {
    fn from(path: &str) -> Self {
        match path {
            "visible" => Self::Visible,
            "repeat" => Self::Repeat,
            _ => match path.strip_prefix(Self::BINDINGS) {
                Some(binding) if !binding.is_empty() => Self::Binding(binding.to_string()),
                _ => Self::Unknown(path.to_string()),
            },
        }
    }
}

impl From<String> for Unset {
    fn from(path: String) -> Self {
        Self::from(path.as_str())
    }
}

impl From<Unset> for String {
    fn from(unset: Unset) -> Self {
        unset.to_string()
    }
}

impl fmt::Display for Unset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unset::Visible => f.write_str("visible"),
            Unset::Repeat => f.write_str("repeat"),
            Unset::Binding(path) => write!(f, "{}{path}", Self::BINDINGS),
            Unset::Unknown(path) => f.write_str(path),
        }
    }
}

/// A reading a layout declares itself, under `[sources.<name>]`, which an expression reads as `$name`.
///
/// Each is run once however many instances read it, only while something that reads it is on screen (unless `while = "always"`), and within `[automation]`'s limits: a run that outstays the timeout or prints too much is stopped, and a failing source waits longer before each retry. What fails is reported with the source's name.
///
/// The fields are optional because a layout that extends this one may write only the key it changes; once every level is laid over the others, a `poll` or `listen` without `cmd` and an `http` without `url` are errors.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A command run on an interval, like eww's `defpoll`: each run's output is one reading.
    Poll {
        /// The command line, run through `sh -c`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cmd: Option<String>,
        /// How often it runs, counted from the start of one run to the start of the next: `500ms`, `5s`, `2m` or `1h`. Never more often than `[automation] min_interval_seconds`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        every: Option<String>,
        /// The reading before the first run answers, and the type of every reading after it: a number makes the source a number, `true` or `false` a bool, a list a list of texts. A text, or nothing at all, makes it text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial: Option<toml::Value>,
        /// How the output becomes a reading: `text` (the default: all of it, trimmed), `lines` (a list, one per line), `json:<path>` (the value at a path such as `.current.temp` or `.items[0].name`) or `regex:<pattern>` (the first match, or its first group when it has one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse: Option<String>,
        /// `visible` (the default) runs only while something showing the reading is on screen; `always` runs while anything reads it at all, hidden or not.
        #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
        while_: Option<While>,
        /// Whether the lock screen may show it. Off unless set: what a command prints is unaudited text, and the lock screen is read by whoever is in the room. A level that changes `cmd` says it again, or the source is not lock-safe: what a level under it vouched for was the command it replaced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_safe: Option<bool>,
    },
    /// A command that keeps running and prints one reading per line, like eww's `deflisten`. It is started again, after a wait, if it exits.
    Listen {
        /// The command line, run through `sh -c`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cmd: Option<String>,
        /// The reading before the first line arrives, and the type of every reading after it, as for a `poll` source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial: Option<toml::Value>,
        /// How each line becomes a reading, as for a `poll` source; `lines` is not one, since every line is a reading of its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse: Option<String>,
        /// `visible` (the default) keeps the command running only while something showing the reading is on screen; `always` while anything reads it at all.
        #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
        while_: Option<While>,
        /// Whether the lock screen may show it, as for a `poll` source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_safe: Option<bool>,
    },
    /// An address fetched on an interval: each response body is one reading.
    Http {
        /// What is fetched, with a `GET`: an `http://` or `https://` address.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// How often it is fetched, as for a `poll` source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        every: Option<String>,
        /// The reading before the first response, and the type of every reading after it, as for a `poll` source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial: Option<toml::Value>,
        /// How the body becomes a reading, as for a `poll` source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse: Option<String>,
        /// Whether it is fetched only while shown, as for a `poll` source.
        #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
        while_: Option<While>,
        /// Whether the lock screen may show it, as for a `poll` source: a level that changes `url` says it again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_safe: Option<bool>,
    },
}

impl Source {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Source::Poll { .. } => "poll",
            Source::Listen { .. } => "listen",
            Source::Http { .. } => "http",
        }
    }

    pub fn is_same_kind(&self, other: &Source) -> bool {
        self.kind_name() == other.kind_name()
    }

    /// The command line, for the two kinds that run one.
    pub fn cmd(&self) -> Option<&str> {
        match self {
            Source::Poll { cmd, .. } | Source::Listen { cmd, .. } => cmd.as_deref(),
            Source::Http { .. } => None,
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Source::Http { url, .. } => url.as_deref(),
            _ => None,
        }
    }

    /// The interval as written, for the two kinds that run on one.
    pub fn every(&self) -> Option<&str> {
        match self {
            Source::Poll { every, .. } | Source::Http { every, .. } => every.as_deref(),
            Source::Listen { .. } => None,
        }
    }

    pub fn initial(&self) -> Option<&toml::Value> {
        match self {
            Source::Poll { initial, .. }
            | Source::Listen { initial, .. }
            | Source::Http { initial, .. } => initial.as_ref(),
        }
    }

    pub fn parse(&self) -> Option<&str> {
        match self {
            Source::Poll { parse, .. }
            | Source::Listen { parse, .. }
            | Source::Http { parse, .. } => parse.as_deref(),
        }
    }

    pub fn running_while(&self) -> While {
        match self {
            Source::Poll { while_, .. }
            | Source::Listen { while_, .. }
            | Source::Http { while_, .. } => while_.unwrap_or_default(),
        }
    }

    pub fn lock_safe(&self) -> bool {
        match self {
            Source::Poll { lock_safe, .. }
            | Source::Listen { lock_safe, .. }
            | Source::Http { lock_safe, .. } => lock_safe.unwrap_or(false),
        }
    }
}

/// When a declared source runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum While {
    /// Only while something showing its reading is on screen.
    #[default]
    Visible,
    /// While anything reads it at all, on screen or not.
    Always,
}
