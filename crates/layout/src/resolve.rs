//! Turning a layout into the arrangement one output shows right now.
//!
//! Resolution is a pure function of the layout, the output and the active workspace: no service is read, no surface is opened and nothing is cached, so the same three inputs always give the same arrangement and a test can ask for one without a compositor. Each output resolves on its own, which is what lets a monitor be re-planned on hotplug without touching the others.
//!
//! The precedence is fixed and total, each level laid over the one before it: the `extends` chain, root first; then every [`OutputRule`] whose glob matches this output, broadest glob first so a named monitor refines a `*` one; then the [`WorkspaceRule`] for the workspace that is active on it. Merging is by id at every level ([`crate::merge`]).
//!
//! The last step is the one that turns *what the file said* into *what was decided*: the partial [`AreaKind`] becomes a [`ResolvedAreaKind`] with every field answered. A field no level ever filled is where an arrangement stops being drawable, so it becomes a [`Finding`] naming the area and the field, and that area alone is dropped. The rest of the layer still resolves, because one unfinished bar is not a reason for a user to lose their desktop.

use std::collections::BTreeMap;
use std::sync::Arc;

use config::{Edge, glob_matches};
use telar_expression::Type;
use util::report::{Finding, Message, Report};

use crate::container::Placement;
use crate::library::{Library, komponent_path};
use crate::merge::{At, Origins, merge_layers, merge_session_layers, merge_sources};
use crate::model::*;

/// The workspace a resolution is for, as much of it as the compositor could say.
///
/// `name` is what every compositor reports through `ext-workspace-v1`. `id` and `special` are Hyprland's alone, and a rule that needs one the compositor did not report is reported as inactive rather than quietly failing to match, so a rule that never fires is visible instead of mysterious.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActiveWorkspace {
    pub name: String,
    pub id: Option<i64>,
    pub special: Option<bool>,
}

/// The level of a layout that wrote an expression resolution kept: the layout whose file says it, and the rule in that file — an output rule, or one of that rule's workspace rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub layout: LayoutId,
    pub output: OutputMatch,
    pub workspace: Option<WorkspaceMatch>,
}

impl Level {
    pub fn file(&self) -> String {
        layout_path(&self.layout)
    }

    /// The key of the rule in its file, which every key validation reports inside that rule starts with.
    pub fn rule(&self) -> String {
        match &self.workspace {
            None => format!("outputs.{}", self.output.0),
            Some(workspace) => format!("outputs.{}.workspaces.{}", self.output.0, workspace.0),
        }
    }
}

/// Who wrote an expression resolution kept: a level of a layout, or the komponent a group uses, whose own file holds what it draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Level(Level),
    Komponent(KomponentId),
}

impl Origin {
    /// The file the expression is written in.
    pub fn file(&self) -> String {
        match self {
            Origin::Level(level) => level.file(),
            Origin::Komponent(id) => komponent_path(id),
        }
    }

    /// The level that wrote it, for one a layout writes.
    pub fn level(&self) -> Option<&Level> {
        match self {
            Origin::Level(level) => Some(level),
            Origin::Komponent(_) => None,
        }
    }
}

/// An expression resolution kept, with who wrote it — where a failure of it is reported, and what a level has to come after to take it back — and, for one a komponent holds, the use whose parameters it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedExpr {
    pub expr: Expr,
    pub origin: Origin,
    /// The komponent use this expression is drawn in, whose parameters it reads before any of the shell's names; `None` for one a layout writes.
    pub within: Option<Arc<KomponentUse>>,
}

/// A group drawing a komponent: which one, where, and what each of its parameters reads there.
#[derive(Clone, Debug, PartialEq)]
pub struct KomponentUse {
    pub id: KomponentId,
    pub layer: LayerKind,
    pub area: AreaId,
    pub group: GroupId,
    /// In the order the komponent declares them. Empty where the komponent is missing.
    pub parameters: Vec<ResolvedParameter>,
}

/// One parameter of a komponent use: what it holds, what the komponent gives it, and what the use sets it to, if it does.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedParameter {
    pub name: String,
    pub ty: Type,
    /// What the komponent's file gives it.
    pub default: Expr,
    /// What the use sets it to, with the level that wrote it.
    pub value: Option<ResolvedExpr>,
}

impl ResolvedParameter {
    /// What it reads: the use's own value, else the default.
    pub fn expr(&self) -> &Expr {
        self.value
            .as_ref()
            .map_or(&self.default, |value| &value.expr)
    }
}

/// What one output shows, with every field answered.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub output: String,
    pub workspace: Option<ActiveWorkspace>,
    /// Keyed by layer, and a `BTreeMap` so iteration is bottom-up, which is the order the windows stack in.
    pub layers: BTreeMap<LayerKind, ResolvedLayer>,
    /// What each edge takes off the screen, in `Edge::ALL` order, settled before any workspace rule ran. Read through [`Resolved::reserved`].
    reserved: [f32; 4],
    /// Which level wrote each key resolution kept. Read through [`Resolved::origin`].
    origins: Arc<Origins>,
}

/// Two arrangements are the same when they draw the same: who wrote a key is not part of what is drawn, so a layout flattened from its `extends` chain resolves to the arrangement the chain does.
impl PartialEq for Resolved {
    fn eq(&self, other: &Self) -> bool {
        self.output == other.output
            && self.workspace == other.workspace
            && self.layers == other.layers
            && self.reserved == other.reserved
    }
}

/// What holds a key a level writes: an area, one of its groups, or an instance in one of those.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder<'a> {
    Area(&'a AreaId),
    Group(&'a AreaId, &'a GroupId),
    Instance(&'a AreaId, &'a GroupId, &'a InstanceId),
}

impl Resolved {
    /// An arrangement assembled directly rather than resolved from a layout — a lock snapshot, a preview, a test. What each edge reserves is derived from `layers`, because there are no workspace rules here to keep out of it.
    pub fn of(
        output: impl Into<String>,
        layers: impl IntoIterator<Item = (LayerKind, ResolvedLayer)>,
    ) -> Self {
        let layers: BTreeMap<LayerKind, ResolvedLayer> = layers.into_iter().collect();
        let reserved = reserved_edges(&layers);
        Self {
            output: output.into(),
            workspace: None,
            layers,
            reserved,
            origins: Arc::default(),
        }
    }

    /// The level that wrote `key` of what `holder` names on `layer`, where resolution kept it — the key spelled as the file spells it under its holder: `thickness`, `shape.radius`, `style.fill`, `style.border.width`, `reserve`, `actions.press`, `options.face.scale`, a group's `arrange`, `cols` or `gap`, a child's `weight`, `cell` or `rect`, or an expression's `visible`, `repeat`, `parameters.<name>` or `bindings.<path>`. An option inside a list or a table a level wrote whole is that level's. `None` where no level writes it, so what is drawn there is a default — and for every key of an arrangement assembled directly ([`Resolved::of`]).
    ///
    /// A group's `arrange` is written by the last level that writes any key its arrangement is made of (`arrange`, `cols`, `rows`, `gap`), which is the level `unset = ["arrange"]` has to come after to take the arrangement back.
    pub fn origin(&self, layer: LayerKind, holder: Holder<'_>, key: &str) -> Option<&Level> {
        let (area, group, instance) = match holder {
            Holder::Area(area) => (area, None, None),
            Holder::Group(area, group) => (area, Some(group), None),
            Holder::Instance(area, group, instance) => (area, Some(group), Some(instance)),
        };
        self.origins.nearest(&(
            layer,
            area.clone(),
            group.cloned(),
            instance.cloned(),
            key.to_string(),
        ))
    }

    pub fn layer(&self, kind: LayerKind) -> Option<&ResolvedLayer> {
        self.layers.get(&kind)
    }

    /// The area `id` on `layer`.
    pub fn area(&self, layer: LayerKind, id: &AreaId) -> Option<&ResolvedArea> {
        self.layer(layer)?.areas.iter().find(|area| area.id == *id)
    }

    /// Every area of every layer, bottom layer first.
    pub fn areas(&self) -> impl Iterator<Item = (LayerKind, &ResolvedArea)> {
        self.layers
            .iter()
            .flat_map(|(kind, layer)| layer.areas.iter().map(move |area| (*kind, area)))
    }

    pub fn instances(&self) -> impl Iterator<Item = &ResolvedInstance> {
        self.areas()
            .flat_map(|(_, area)| area.groups.iter())
            .flat_map(|group| group.children.iter())
    }

    /// How deep `edge`'s reserving areas are, which is what its reservation strip commits.
    ///
    /// Derived from the output-level arrangement alone — the one every workspace on that screen shares — so switching workspaces can add and remove areas but can never re-tile the user's windows (F-6.7). It is the areas' own depth; the air a floating bar sits in is its shape's `gap`, which the config can still take away (`[shape] frame`).
    pub fn reserved(&self, edge: Edge) -> f32 {
        self.reserved[Edge::ALL.iter().position(|it| *it == edge).unwrap_or(0)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedLayer {
    pub areas: Vec<ResolvedArea>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedArea {
    pub id: AreaId,
    pub kind: ResolvedAreaKind,
    pub reserve: bool,
    pub above_fullscreen: bool,
    /// Which box this area's geometry is measured in.
    pub within: Within,
    pub style: Style,
    pub visible: Option<ResolvedExpr>,
    pub groups: Vec<ResolvedGroup>,
    pub actions: BTreeMap<Trigger, Action>,
}

/// An area's geometry with nothing left to decide.
#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedAreaKind {
    Bar {
        edge: Edge,
        thickness: f32,
        length: Extent,
        offset: f32,
        shape: BarShape,
        autohide: Option<AutoHide>,
    },
    Grid {
        rect: Rect,
        cell: f32,
        gap: f32,
        anchor: Anchor,
    },
    Stack {
        anchor: Anchor,
        offset: Offset,
        width: f32,
        output_policy: StackOutputPolicy,
        routes: Vec<Route>,
        launcher: bool,
    },
    WallpaperRegion {
        rect: Rect,
        source: String,
        fit: Fit,
        transition: Transition,
    },
    Texture {
        rect: Rect,
        paint: Paint,
        tile: Tile,
        blend: Blend,
        opacity: f32,
    },
    Dock {
        edge: Edge,
        thickness: f32,
    },
    Free {
        rect: Rect,
        anchor: Anchor,
    },
    /// Only ever one off the lock layer whose owner is an instance of the same layer outside any panel, and the first panel that owner has: validation reports the others, and a file edited past that leaves them out. `along` only where the owner sits in a bar, and `cols` and `rows` at least 1.
    Panel {
        owner: InstanceId,
        along: bool,
        cols: u32,
        rows: u32,
        cell: f32,
        gap: f32,
    },
    Prompt {
        rect: Rect,
    },
}

/// What a resolved texture paints. Exactly one of the two, because an area that named both would otherwise draw whichever the renderer happened to check first.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Image(String),
    Gradient(Gradient),
}

impl ResolvedAreaKind {
    pub fn rect(&self) -> Option<Rect> {
        match self {
            ResolvedAreaKind::Grid { rect, .. }
            | ResolvedAreaKind::WallpaperRegion { rect, .. }
            | ResolvedAreaKind::Texture { rect, .. }
            | ResolvedAreaKind::Free { rect, .. }
            | ResolvedAreaKind::Prompt { rect } => Some(*rect),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            ResolvedAreaKind::Bar { .. } => "bar",
            ResolvedAreaKind::Grid { .. } => "grid",
            ResolvedAreaKind::Stack { .. } => "stack",
            ResolvedAreaKind::WallpaperRegion { .. } => "wallpaper_region",
            ResolvedAreaKind::Texture { .. } => "texture",
            ResolvedAreaKind::Dock { .. } => "dock",
            ResolvedAreaKind::Free { .. } => "free",
            ResolvedAreaKind::Panel { .. } => "panel",
            ResolvedAreaKind::Prompt { .. } => "prompt",
        }
    }

    pub fn edge(&self) -> Option<Edge> {
        match self {
            ResolvedAreaKind::Bar { edge, .. } | ResolvedAreaKind::Dock { edge, .. } => Some(*edge),
            _ => None,
        }
    }

    /// Whether this kind holds instances at all. A wallpaper region and a texture are paint, so a group placed in one is reported rather than silently ignored.
    pub fn holds_instances(&self) -> bool {
        !matches!(
            self,
            ResolvedAreaKind::WallpaperRegion { .. } | ResolvedAreaKind::Texture { .. }
        )
    }

    /// How much this area would take off `edge` if it reserves. A bar that hides itself reserves only the strip it leaves behind, which is what keeps an autohidden bar from holding a gap the user cannot see.
    fn reserving_thickness(&self, edge: Edge) -> Option<f32> {
        match self {
            ResolvedAreaKind::Bar {
                edge: on,
                thickness,
                autohide,
                ..
            } if *on == edge => Some(match autohide {
                Some(hide) => hide.peek,
                None => *thickness,
            }),
            ResolvedAreaKind::Dock {
                edge: on,
                thickness,
            } if *on == edge => Some(*thickness),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedGroup {
    pub id: GroupId,
    pub kind: GroupKind,
    /// How the group lays out its children. Never anything but `pages` in a zone: validation refuses it there, and a file edited past that draws a loose run. The level that wrote it is [`Resolved::origin`] of `arrange` on the group.
    pub arrange: Option<Arrange>,
    /// How many columns the inner grid a `grid` group's children are placed on has.
    pub cols: u32,
    /// How many rows that inner grid has.
    pub rows: u32,
    /// The space between two children it arranges; left out, what is drawn picks one for where the group sits.
    pub gap: Option<f32>,
    /// The list the children are drawn once per item of. Never on a grid cell: validation refuses it there, and a file edited past that draws the children once, as written.
    pub repeat: Option<ResolvedExpr>,
    /// The komponent the group draws, where it uses one: its children, how they are arranged and `repeat` are then the komponent's, each child under its use's id (`<area>.<group>/<child>`). A komponent the library does not hold is one placeholder child named by its file.
    pub komponent: Option<Arc<KomponentUse>>,
    /// The group's own style: a use of a komponent keeps it, since what the komponent holds is drawn inside it.
    pub style: Style,
    pub children: Vec<ResolvedInstance>,
}

impl ResolvedGroup {
    /// Whether the group shows its children one at a time.
    pub fn is_pages(&self) -> bool {
        self.arrange == Some(Arrange::Pages)
    }

    /// The group as a layout writes it in full, without its children: `cols` and `rows` only where it arranges them on an inner grid, the one arrangement that reads them.
    pub fn written(&self) -> Group {
        let grid = self.arrange == Some(Arrange::Grid);
        Group {
            id: self.id.clone(),
            kind: Some(self.kind),
            arrange: self.arrange,
            cols: grid.then_some(self.cols),
            rows: grid.then_some(self.rows),
            gap: self.gap,
            repeat: self.repeat.as_ref().map(|repeat| repeat.expr.clone()),
            style: self.style.clone(),
            ..Group::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedInstance {
    pub id: InstanceId,
    pub module: String,
    pub representation: Representation,
    pub options: toml::Table,
    pub bindings: BTreeMap<String, ResolvedExpr>,
    pub style: Style,
    /// Where it sits in a group that arranges its children; `None` in a loose run or a `pages` group.
    pub placement: Option<Placement>,
    pub actions: BTreeMap<Trigger, Action>,
}

/// Resolves `layout` for one output and workspace, following `extends` and the komponents its groups use through `known`.
///
/// Always returns an arrangement. Whatever could not be decided is in the [`Report`] and is missing from the arrangement, so a caller draws what the user asked for minus the parts that were not finished, and says what those were.
pub fn resolve(
    layout: &Layout,
    known: &Library,
    output: &str,
    workspace: Option<&ActiveWorkspace>,
) -> (Resolved, Report) {
    let mut report = Report::default();
    let chain: Vec<std::borrow::Cow<'_, Layout>> = chain_of(layout, known, &mut report)
        .into_iter()
        .map(|level| known.trust.gate_layout(level))
        .collect();

    let mut layers = Layers::default();
    // The same arrangement with every workspace rule left out, which is the only thing an exclusive zone may be derived from: a rule that could re-tile the user's windows would do it on every workspace switch (F-6.7). Kept beside rather than recomputed, because it is the same merge and two of them could drift.
    let mut without_rules = Layers::default();
    let (mut origins, mut origins_without_rules) = (Origins::default(), Origins::default());
    let mut ruled = false;
    for level in chain.iter().map(|level| level.as_ref()) {
        let rules = rules_for(level, output);

        for rule in &rules {
            let origin = Level {
                layout: level.id.clone(),
                output: rule.matches.clone(),
                workspace: None,
            };
            origins.lay(&layers, rule.layers.each(), &origin);
            merge_layers(&mut layers, &rule.layers);
            origins_without_rules.lay(&without_rules, rule.layers.each(), &origin);
            merge_layers(&mut without_rules, &rule.layers);
        }
        for rule in &rules {
            for workspace_rule in &rule.workspaces {
                match matches_workspace(&workspace_rule.matches, workspace) {
                    WorkspaceVerdict::Matches => {
                        let origin = Level {
                            layout: level.id.clone(),
                            output: rule.matches.clone(),
                            workspace: Some(workspace_rule.matches.clone()),
                        };
                        origins.lay(&layers, workspace_rule.layers.each(), &origin);
                        merge_session_layers(&mut layers, &workspace_rule.layers);
                        ruled = true;
                    }
                    WorkspaceVerdict::Differs => {}
                    WorkspaceVerdict::Unanswerable(why) => report.warn(Finding::new(
                        layout_path(&level.id),
                        format!(
                            "outputs.{}.workspaces.{}",
                            rule.matches.0, workspace_rule.matches.0
                        ),
                        why,
                    )),
                }
            }
        }
    }

    let answered = answer_layers(&layers, &origins, layout, known, &mut report);
    // A workspace rule may only add, remove and restyle; what each edge takes off the screen is settled before any of them runs. Answering the rule-free arrangement a second time is the cost of that, and only where a rule actually matched — its own findings are the ones already reported, so they go to a report nobody reads.
    let reserved = match ruled {
        false => reserved_edges(&answered),
        true => reserved_edges(&answer_layers(
            &without_rules,
            &origins_without_rules,
            layout,
            known,
            &mut Report::default(),
        )),
    };

    let resolved = Resolved {
        output: output.to_string(),
        workspace: workspace.cloned(),
        reserved,
        layers: answered
            .into_iter()
            .filter(|(_, layer)| !layer.areas.is_empty())
            .collect(),
        origins: Arc::new(origins),
    };
    (resolved, report)
}

fn answer_layers(
    layers: &Layers,
    origins: &Origins,
    layout: &Layout,
    library: &Library,
    report: &mut Report,
) -> BTreeMap<LayerKind, ResolvedLayer> {
    LayerKind::ALL
        .into_iter()
        .map(|kind| {
            let answering = Answering {
                layer: kind,
                origins,
                layout,
                library,
            };
            (kind, answer_layer(layers.get(kind), answering, report))
        })
        .collect()
}

/// What answering one layer of a merge reads besides the merge: which layer it is, who wrote each expression the merge kept, the layout being resolved, whose file names what could not be answered, and the komponents its groups may use.
#[derive(Clone, Copy)]
struct Answering<'a> {
    layer: LayerKind,
    origins: &'a Origins,
    layout: &'a Layout,
    library: &'a Library,
}

impl Answering<'_> {
    /// `expr`, which the merge kept at `at`, with the level that wrote it.
    fn sourced(&self, expr: &Expr, (layer, area, group, instance, path): At) -> ResolvedExpr {
        let origin = self
            .origins
            .of(&(layer, area, group, instance, path.to_string()))
            .expect("the merge records the level of every expression it keeps")
            .clone();
        ResolvedExpr {
            expr: expr.clone(),
            origin: Origin::Level(origin),
            within: None,
        }
    }
}

/// What each edge of the output is taken by, in `Edge::ALL` order: its deepest reserving area, since every area on one edge hugs that edge and the ones beside each other along it share one band rather than stacking.
fn reserved_edges(layers: &BTreeMap<LayerKind, ResolvedLayer>) -> [f32; 4] {
    Edge::ALL.map(|edge| {
        layers
            .values()
            .flat_map(|layer| layer.areas.iter())
            .filter(|area| area.reserve)
            .filter_map(|area| area.kind.reserving_thickness(edge))
            .fold(0.0, f32::max)
    })
}

/// The sources `layout` declares, each laid over what the layouts it extends declare under the same name.
///
/// A source that is still missing what it runs once every level has had its say — a `poll` or `listen` with no `cmd`, an `http` with no `url` — is reported and left out, so the rest still run. So is one whose command or address came with a bundle and was not accepted at that text ([`held_sources`]), which [`crate::trust::held`] reports rather than this: it is waiting for the user, not wrong.
pub fn sources(layout: &Layout, known: &Library) -> (BTreeMap<String, Source>, Report) {
    let mut report = Report::default();
    let (mut merged, _) = merged_sources(layout, known, &mut report);
    merged.retain(|name, source| {
        let missing = match source {
            Source::Poll { cmd: None, .. } | Source::Listen { cmd: None, .. } => Some("cmd"),
            Source::Http { url: None, .. } => Some("url"),
            _ => None,
        };
        let Some(missing) = missing else {
            return true;
        };
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("sources.{name}.{missing}"),
            util::message!(
                "finding.source_missing",
                kind = source.kind_name(),
                key = missing
            ),
        ));
        false
    });
    (merged, report)
}

/// The sources `layout` declares that [`sources`] holds back until the user trusts what they run: an expression reading one waits for that rather than reading a variable of its name.
pub fn held_sources(layout: &Layout, known: &Library) -> BTreeMap<String, Source> {
    merged_sources(layout, known, &mut Report::default()).1
}

/// `layout`'s sources merged down its chain, split into those that may run and those held for the user's trust.
fn merged_sources(
    layout: &Layout,
    known: &Library,
    report: &mut Report,
) -> (BTreeMap<String, Source>, BTreeMap<String, Source>) {
    let mut merged = BTreeMap::new();
    let chain = chain_of(layout, known, report);
    for level in &chain {
        merge_sources(&mut merged, &level.sources);
    }
    let writers = crate::trust::source_items(&chain);
    merged.into_iter().partition(|(name, _)| {
        writers
            .get(name)
            .is_none_or(|item| known.trust.verdict(item).runs())
    })
}

/// How many layouts an `extends` chain may hold, the layout itself included: far more than anybody layers by hand, and few enough that a bundle of hundreds of layouts each extending the next costs nothing to resolve, every time each of them is.
pub const EXTENDS_DEPTH: usize = 16;

/// The layouts to apply, root first. A cycle stops at the layout that closes it, and a chain longer than [`EXTENDS_DEPTH`] at the layout past it, each reported once at `extends`, so a file that extends itself is a message rather than a hang and a deep one a message rather than a cost.
pub(crate) fn chain_of<'a>(
    layout: &'a Layout,
    known: &'a Library,
    report: &mut Report,
) -> Vec<&'a Layout> {
    let mut chain = vec![layout];
    let mut seen = vec![layout.id.clone()];
    let mut parent = layout.extends.clone();

    while let Some(id) = parent {
        if chain.len() == EXTENDS_DEPTH {
            report.error(Finding::new(
                layout_path(&layout.id),
                "extends",
                util::message!("finding.extends_too_deep", id = id, limit = EXTENDS_DEPTH),
            ));
            break;
        }
        if seen.contains(&id) {
            report.error(Finding::new(
                layout_path(&layout.id),
                "extends",
                util::message!("finding.extends_itself", id = id),
            ));
            break;
        }
        let Some(found) = known.layout(&id) else {
            report.error(Finding::new(
                layout_path(&layout.id),
                "extends",
                util::message!("finding.unknown_layout", id = id),
            ));
            break;
        };
        seen.push(id);
        parent = found.extends.clone();
        chain.push(found);
    }

    chain.reverse();
    chain
}

/// Where a layout's file is, as a finding names it, by the id it answers to.
pub fn layout_path(id: &LayoutId) -> String {
    format!("layouts/{id}.toml")
}

enum WorkspaceVerdict {
    Matches,
    Differs,
    /// The compositor did not report what the rule matches on.
    Unanswerable(Message),
}

fn matches_workspace(
    pattern: &WorkspaceMatch,
    workspace: Option<&ActiveWorkspace>,
) -> WorkspaceVerdict {
    let Some(active) = workspace else {
        return WorkspaceVerdict::Differs;
    };
    match pattern.kind() {
        WorkspaceMatchKind::Name => {
            if glob_matches(pattern.value(), &active.name) {
                WorkspaceVerdict::Matches
            } else {
                WorkspaceVerdict::Differs
            }
        }
        WorkspaceMatchKind::Id => match (active.id, pattern.value().parse::<i64>()) {
            (Some(active_id), Ok(wanted)) if active_id == wanted => WorkspaceVerdict::Matches,
            (Some(_), Ok(_)) => WorkspaceVerdict::Differs,
            (_, Err(_)) => WorkspaceVerdict::Unanswerable(util::message!(
                "finding.workspace_number",
                value = pattern.value()
            )),
            (None, Ok(_)) => WorkspaceVerdict::Unanswerable(util::message!(
                "finding.workspace_id_needs_hyprland"
            )),
        },
        WorkspaceMatchKind::Special => match active.special {
            Some(true)
                if glob_matches(pattern.value(), active.name.trim_start_matches("special:")) =>
            {
                WorkspaceVerdict::Matches
            }
            Some(_) => WorkspaceVerdict::Differs,
            None => WorkspaceVerdict::Unanswerable(util::message!(
                "finding.workspace_special_needs_hyprland"
            )),
        },
    }
}

fn answer_layer(layer: &Layer, answering: Answering<'_>, report: &mut Report) -> ResolvedLayer {
    let mut areas: Vec<ResolvedArea> = layer
        .areas
        .iter()
        .filter_map(|area| answer_area(area, answering, report))
        .collect();
    if answering.layer == LayerKind::Lock {
        areas.sort_by_key(|area| matches!(area.kind, ResolvedAreaKind::Prompt { .. }));
    }
    ResolvedLayer {
        areas: owned_panels(areas, answering.layer),
    }
}

/// `areas` with each panel kept only where validation would let it be drawn: off the lock layer, its owner an instance of this layer outside every panel, and no earlier panel of that owner. `along` is dropped where the owner is not in a bar. An owner a narrower level takes away takes its panel with it, which is how a panel goes from one monitor without being named there.
fn owned_panels(mut areas: Vec<ResolvedArea>, layer: LayerKind) -> Vec<ResolvedArea> {
    let in_bar: BTreeMap<InstanceId, bool> = areas
        .iter()
        .filter(|area| !matches!(area.kind, ResolvedAreaKind::Panel { .. }))
        .flat_map(|area| {
            let in_bar = matches!(area.kind, ResolvedAreaKind::Bar { .. });
            area.groups
                .iter()
                .flat_map(|group| group.children.iter())
                .map(move |child| (child.id.clone(), in_bar))
        })
        .collect();
    let mut opened = std::collections::BTreeSet::new();
    areas.retain_mut(|area| {
        let ResolvedAreaKind::Panel { owner, along, .. } = &mut area.kind else {
            return true;
        };
        let Some(in_bar) = in_bar.get(owner).filter(|_| layer != LayerKind::Lock) else {
            return false;
        };
        *along &= *in_bar;
        opened.insert(owner.clone())
    });
    areas
}

fn answer_area(area: &Area, answering: Answering<'_>, report: &mut Report) -> Option<ResolvedArea> {
    let Answering { layer, layout, .. } = answering;
    let at = format!("layers.{layer}.areas.{}", area.id);
    let mut miss = |field: &str, kind: &str| {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.{field}"),
            util::message!("finding.area_needs", kind = kind, field = field),
        ));
    };

    if area.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("layers.{layer}.areas"),
            util::message!("finding.area_no_id"),
        ));
        return None;
    }

    let Some(kind) = &area.kind else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.kind"),
            util::message!("finding.area_no_kind"),
        ));
        return None;
    };

    let kind = answer_kind(kind, &mut miss)?;

    let groups = if kind.holds_instances() {
        area.groups
            .iter()
            .filter_map(|group| answer_group(group, &area.id, &at, answering, report))
            .collect()
    } else {
        if !area.groups.is_empty() {
            report.warn(Finding::new(
                layout_path(&layout.id),
                format!("{at}.groups"),
                util::message!("finding.paint_holds_nothing", kind = kind.name()),
            ));
        }
        Vec::new()
    };
    let reserve = area.reserve.unwrap_or(matches!(
        kind,
        ResolvedAreaKind::Bar { .. } | ResolvedAreaKind::Dock { .. }
    )) && !matches!(kind, ResolvedAreaKind::Panel { .. });

    Some(ResolvedArea {
        id: area.id.clone(),
        kind,
        reserve,
        above_fullscreen: area.above_fullscreen.unwrap_or(false),
        within: area.within.unwrap_or_default(),
        style: area.style.clone(),
        visible: area.visible.as_ref().map(|visible| {
            answering.sourced(
                visible,
                (layer, area.id.clone(), None, None, Unset::Visible),
            )
        }),
        groups,
        actions: area.actions.clone(),
    })
}

fn answer_kind(kind: &AreaKind, miss: &mut impl FnMut(&str, &str)) -> Option<ResolvedAreaKind> {
    let name = kind.name();
    let mut need = |present: bool, field: &str| {
        if !present {
            miss(field, name);
        }
        present
    };

    match kind {
        AreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        } => {
            let ok = need(edge.is_some(), "edge") & need(thickness.is_some(), "thickness");
            ok.then(|| ResolvedAreaKind::Bar {
                edge: edge.expect("checked"),
                thickness: thickness.expect("checked"),
                length: length.unwrap_or_default(),
                offset: offset.unwrap_or(0.0),
                shape: *shape,
                autohide: *autohide,
            })
        }
        AreaKind::Grid {
            rect,
            cell,
            gap,
            anchor,
        } => Some(ResolvedAreaKind::Grid {
            rect: rect.unwrap_or_default(),
            cell: cell.unwrap_or(AreaKind::CELL),
            gap: gap.unwrap_or(AreaKind::GAP),
            anchor: anchor.unwrap_or(Anchor::TopLeft),
        }),
        AreaKind::Stack {
            anchor,
            offset,
            width,
            output_policy,
            routes,
            launcher,
        } => need(width.is_some(), "width").then(|| ResolvedAreaKind::Stack {
            anchor: anchor.unwrap_or_default(),
            offset: offset.unwrap_or_default(),
            width: width.expect("checked"),
            output_policy: output_policy.clone().unwrap_or_default(),
            routes: routes.clone(),
            launcher: launcher.unwrap_or(false),
        }),
        AreaKind::WallpaperRegion {
            rect,
            source,
            fit,
            transition,
        } => Some(ResolvedAreaKind::WallpaperRegion {
            rect: rect.unwrap_or_default(),
            // A region with no source is not unfinished: it is what one says when it means "whatever `[background]` is set to", so changing the desktop picture stays a `[background]` edit and a `hogar-shell wallpaper set` rather than a layout edit.
            source: source.clone().unwrap_or_default(),
            fit: fit.unwrap_or_default(),
            transition: transition.unwrap_or_default(),
        }),
        AreaKind::Texture {
            rect,
            image,
            gradient,
            tile,
            blend,
            opacity,
        } => {
            let paint = match (image, gradient) {
                (Some(image), _) => Some(Paint::Image(image.clone())),
                (None, Some(gradient)) => Some(Paint::Gradient(gradient.clone())),
                (None, None) => {
                    miss("image` or `gradient", name);
                    None
                }
            }?;
            Some(ResolvedAreaKind::Texture {
                rect: rect.unwrap_or_default(),
                paint,
                tile: tile.unwrap_or_default(),
                blend: blend.unwrap_or_default(),
                opacity: opacity.unwrap_or(1.0),
            })
        }
        AreaKind::Dock { edge, thickness } => {
            let ok = need(edge.is_some(), "edge") & need(thickness.is_some(), "thickness");
            ok.then(|| ResolvedAreaKind::Dock {
                edge: edge.expect("checked"),
                thickness: thickness.expect("checked"),
            })
        }
        AreaKind::Free { rect, anchor } => {
            need(rect.is_some(), "rect").then(|| ResolvedAreaKind::Free {
                rect: rect.expect("checked"),
                anchor: anchor.unwrap_or(Anchor::TopLeft),
            })
        }
        AreaKind::Panel {
            owner,
            along,
            cols,
            rows,
            cell,
            gap,
        } => need(owner.is_some(), "owner").then(|| ResolvedAreaKind::Panel {
            owner: owner.clone().expect("checked"),
            along: along.unwrap_or(false),
            cols: cols.unwrap_or(AreaKind::PANEL_COLS).max(1),
            rows: rows.unwrap_or(AreaKind::PANEL_ROWS).max(1),
            cell: cell.unwrap_or(AreaKind::CELL),
            gap: gap.unwrap_or(AreaKind::GAP),
        }),
        AreaKind::Prompt { rect } => Some(ResolvedAreaKind::Prompt {
            rect: rect.unwrap_or(Rect {
                x: 0.3,
                y: 0.35,
                w: 0.4,
                h: 0.3,
            }),
        }),
    }
}

fn answer_group(
    group: &Group,
    area: &AreaId,
    at: &str,
    answering: Answering<'_>,
    report: &mut Report,
) -> Option<ResolvedGroup> {
    let layout = answering.layout;
    let at = format!("{at}.groups.{}", group.id);
    if group.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            at,
            util::message!("finding.group_no_id"),
        ));
        return None;
    }
    let Some(kind) = group.kind else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.place"),
            util::message!("finding.group_no_place"),
        ));
        return None;
    };
    if let Some(komponent) = &group.komponent {
        return Some(answer_use(
            group, kind, komponent, area, &at, answering, report,
        ));
    }
    if !group.parameters.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.parameters"),
            util::message!("finding.parameters_without_komponent"),
        ));
    }
    let held = (area, &group.id);
    let drawn = group
        .children
        .iter()
        .filter_map(|instance| {
            let resolved = answer_instance(instance, held, &at, answering, report)?;
            Some((instance, resolved))
        })
        .collect();
    let arrange = arranged(group.arrange, kind);
    let (cols, rows) = tracks(group.cols, group.rows);
    let file = layout_path(&layout.id);
    let children = placed((arrange, cols, rows), drawn, report, |child| {
        (file.clone(), format!("{at}.children.{}.cell", child.id))
    });
    let at = |slot| {
        (
            answering.layer,
            area.clone(),
            Some(group.id.clone()),
            None,
            slot,
        )
    };
    Some(ResolvedGroup {
        id: group.id.clone(),
        kind,
        arrange,
        cols,
        rows,
        gap: group.gap,
        repeat: group
            .repeat
            .as_ref()
            .filter(|_| !matches!(kind, GroupKind::Cell { .. }))
            .map(|repeat| answering.sourced(repeat, at(Unset::Repeat))),
        komponent: None,
        style: group.style.clone(),
        children,
    })
}

/// A group drawing the komponent `id`: the komponent's children under the use's ids, how it arranges them and its `repeat`, and what each parameter reads here. What the merged group holds besides is reported and not drawn; a komponent the library does not hold is one placeholder child named by its file, so the area still draws and says what is missing.
fn answer_use(
    group: &Group,
    kind: GroupKind,
    id: &KomponentId,
    area: &AreaId,
    at: &str,
    answering: Answering<'_>,
    report: &mut Report,
) -> ResolvedGroup {
    let file = layout_path(&answering.layout.id);
    if !group.children.is_empty() || group.repeat.is_some() || group.writes_arrangement() {
        report.error(Finding::new(
            file.clone(),
            format!("{at}.komponent"),
            util::message!("finding.komponent_holds_more", komponent = id),
        ));
    }
    let mut used = KomponentUse {
        id: id.clone(),
        layer: answering.layer,
        area: area.clone(),
        group: group.id.clone(),
        parameters: Vec::new(),
    };
    let on_cell = matches!(kind, GroupKind::Cell { .. });
    let Some(komponent) = answering
        .library
        .komponent(id)
        .map(|komponent| answering.library.trust.gate_komponent(id, komponent))
    else {
        report.error(Finding::new(
            file,
            format!("{at}.komponent"),
            util::message!(
                "finding.unknown_komponent",
                komponent = id,
                file = komponent_path(id)
            ),
        ));
        let standing_in = ResolvedInstance {
            id: InstanceId::in_komponent(area, &group.id, &InstanceId::new(id.as_str())),
            module: komponent_path(id),
            representation: match on_cell {
                true => Representation::WidgetS,
                false => Representation::Chip,
            },
            options: toml::Table::new(),
            bindings: BTreeMap::new(),
            style: Style::default(),
            placement: None,
            actions: BTreeMap::new(),
        };
        return ResolvedGroup {
            id: group.id.clone(),
            kind,
            arrange: None,
            cols: Arrange::TRACKS,
            rows: Arrange::TRACKS,
            gap: None,
            repeat: None,
            komponent: Some(Arc::new(used)),
            style: group.style.clone(),
            children: vec![standing_in],
        };
    };
    for (name, declared) in &komponent.parameters {
        let value = group.parameters.get(name).map(|expr| {
            let slot = (
                answering.layer,
                area.clone(),
                Some(group.id.clone()),
                None,
                Unset::parameter(name),
            );
            answering.sourced(expr, slot)
        });
        used.parameters.push(ResolvedParameter {
            name: name.clone(),
            ty: declared.ty.0.clone(),
            default: declared.default.clone(),
            value,
        });
    }
    for name in group
        .parameters
        .keys()
        .filter(|name| !komponent.parameters.contains_key(*name))
    {
        report.error(Finding::new(
            file.clone(),
            format!("{at}.parameters.{name}"),
            util::message!(
                "finding.unknown_parameter",
                komponent = id,
                name = name,
                declared = declared_names(&komponent)
            ),
        ));
    }
    let used = Arc::new(used);
    let drawn = |expr: &Expr| ResolvedExpr {
        expr: expr.clone(),
        origin: Origin::Komponent(id.clone()),
        within: Some(Arc::clone(&used)),
    };
    let children = komponent
        .children
        .iter()
        .filter_map(|child| {
            let resolved = answer_komponent_child(child, &used, &drawn, report)?;
            Some((child, resolved))
        })
        .collect();
    let arrange = arranged(komponent.arrange, kind);
    let (cols, rows) = tracks(komponent.cols, komponent.rows);
    let children = placed((arrange, cols, rows), children, report, |child| {
        (komponent_path(id), format!("children.{}.cell", child.id))
    });
    ResolvedGroup {
        id: group.id.clone(),
        kind,
        arrange,
        cols,
        rows,
        gap: komponent.gap,
        repeat: komponent.repeat.as_ref().filter(|_| !on_cell).map(&drawn),
        komponent: Some(Arc::clone(&used)),
        style: group.style.clone(),
        children,
    }
}

/// The `arrange` a group placed as `kind` is drawn with: in a zone, of whatever area, anything but `pages` is refused by validation and drawn as a loose run.
fn arranged(arrange: Option<Arrange>, kind: GroupKind) -> Option<Arrange> {
    arrange.filter(|arrange| *arrange == Arrange::Pages || matches!(kind, GroupKind::Cell { .. }))
}

/// The columns and rows of a group's inner grid: 2 each where it names none, and never fewer than one.
fn tracks(cols: Option<u32>, rows: Option<u32>) -> (u32, u32) {
    let track = |written: Option<u32>| written.unwrap_or(Arrange::TRACKS).max(1);
    (track(cols), track(rows))
}

/// The resolved children of a group arranged as `arrange` on an inner grid of `cols` × `rows`, each with where it sits, beside what its file wrote. A written `cell` pulled back onto the inner grid is a warning at the file and key `cell_at` names.
fn placed(
    (arrange, cols, rows): (Option<Arrange>, u32, u32),
    drawn: Vec<(&Instance, ResolvedInstance)>,
    report: &mut Report,
    cell_at: impl Fn(&Instance) -> (String, String),
) -> Vec<ResolvedInstance> {
    let written: Vec<&Instance> = drawn.iter().map(|(written, _)| *written).collect();
    let placements = crate::container::placements(arrange, (cols, rows), &written);
    drawn
        .into_iter()
        .zip(placements)
        .map(|((written, mut resolved), placement)| {
            if let (Some(Placement::Cell(kept)), Some(cell)) = (placement, written.cell)
                && kept != cell
            {
                let (file, key) = cell_at(written);
                report.warn(Finding::new(
                    file,
                    key,
                    util::message!(
                        "finding.cell_off_grid",
                        cols = cols,
                        rows = rows,
                        col = kept.col,
                        row = kept.row
                    ),
                ));
            }
            resolved.placement = placement;
            resolved
        })
        .collect()
}

/// The parameters `komponent` declares, as a sentence lists them.
pub(crate) fn declared_names(komponent: &Komponent) -> String {
    match komponent.parameters.is_empty() {
        true => "—".to_string(),
        false => komponent
            .parameters
            .keys()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// One child of a komponent, as the use `used` draws it: under the use's id, its expressions read through `drawn`. What its file leaves out is reported in that file.
fn answer_komponent_child(
    child: &Instance,
    used: &KomponentUse,
    drawn: &dyn Fn(&Expr) -> ResolvedExpr,
    report: &mut Report,
) -> Option<ResolvedInstance> {
    let file = komponent_path(&used.id);
    if child.id.is_empty() {
        report.error(Finding::new(
            file,
            "children",
            util::message!("finding.instance_no_id"),
        ));
        return None;
    }
    let Some(module) = child.module.clone() else {
        report.error(Finding::new(
            file,
            format!("children.{}.module", child.id),
            util::message!("finding.instance_no_module"),
        ));
        return None;
    };
    Some(ResolvedInstance {
        id: InstanceId::in_komponent(&used.area, &used.group, &child.id),
        module,
        representation: child.representation.unwrap_or(Representation::Chip),
        options: child.options.clone(),
        bindings: child
            .bindings
            .iter()
            .map(|(path, expr)| (path.clone(), drawn(expr)))
            .collect(),
        style: child.style.clone(),
        placement: None,
        actions: child.actions.clone(),
    })
}

fn answer_instance(
    instance: &Instance,
    (area, group): (&AreaId, &GroupId),
    at: &str,
    answering: Answering<'_>,
    report: &mut Report,
) -> Option<ResolvedInstance> {
    let layout = answering.layout;
    let at = format!("{at}.children.{}", instance.id);
    if instance.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            at,
            util::message!("finding.instance_no_id"),
        ));
        return None;
    }
    let Some(module) = instance.module.clone() else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.module"),
            util::message!("finding.instance_no_module"),
        ));
        return None;
    };
    let bindings = instance
        .bindings
        .iter()
        .map(|(path, expr)| {
            let at = (
                answering.layer,
                area.clone(),
                Some(group.clone()),
                Some(instance.id.clone()),
                Unset::binding(path),
            );
            (path.clone(), answering.sourced(expr, at))
        })
        .collect();
    Some(ResolvedInstance {
        id: instance.id.clone(),
        module,
        representation: instance.representation.unwrap_or(Representation::Chip),
        options: instance.options.clone(),
        bindings,
        style: instance.style.clone(),
        placement: None,
        actions: instance.actions.clone(),
    })
}

/// The output rules of `layout` that speak for a screen called `screen`, in the order resolution lays them: broadest glob first, file order between equals.
fn rules_for<'a>(layout: &'a Layout, screen: &str) -> Vec<&'a OutputRule> {
    let mut rules: Vec<&OutputRule> = layout
        .outputs
        .iter()
        .filter(|rule| rule.matches.matches(screen))
        .collect();
    rules.sort_by_key(|rule| rule.matches.specificity());
    rules
}

/// Whether what `level` writes is laid over what `writer` wrote on a screen called `screen`, whichever workspace is up (TA-2). `level` is a level of `layout`, `writer` one of `layout` or of a layout it extends: every level of a layout it extends is under every level of its own, and within one layout the output rules come first, broadest first, then their workspace rules in the same order. Nothing is laid over what a komponent's own file writes.
pub fn lays_over(layout: &Layout, screen: &str, level: &Level, writer: &Origin) -> bool {
    let Origin::Level(writer) = writer else {
        return false;
    };
    if level.layout != layout.id {
        return false;
    }
    if writer.layout != layout.id {
        return true;
    }
    let rules = rules_for(layout, screen);
    let order: Vec<(&OutputMatch, Option<&WorkspaceMatch>)> = rules
        .iter()
        .map(|rule| (&rule.matches, None))
        .chain(rules.iter().flat_map(|rule| {
            rule.workspaces
                .iter()
                .map(|workspace| (&rule.matches, Some(&workspace.matches)))
        }))
        .collect();
    let at = |origin: &Level| {
        order.iter().position(|(output, workspace)| {
            **output == origin.output && *workspace == origin.workspace.as_ref()
        })
    };
    matches!((at(level), at(writer)), (Some(level), Some(writer)) if level > writer)
}
