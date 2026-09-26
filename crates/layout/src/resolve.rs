//! Turning a layout into the arrangement one output shows right now.
//!
//! Resolution is a pure function of the layout, the output and the active workspace: no service is read, no surface is opened and nothing is cached, so the same three inputs always give the same arrangement and a test can ask for one without a compositor. Each output resolves on its own, which is what lets a monitor be re-planned on hotplug without touching the others.
//!
//! The precedence is fixed and total, each level laid over the one before it: the `extends` chain, root first; then every [`OutputRule`] whose glob matches this output, broadest glob first so a named monitor refines a `*` rule; then the [`WorkspaceRule`] for the workspace that is active on it. Merging is by id at every level ([`crate::merge`]).
//!
//! The last step is the one that turns *what the file said* into *what was decided*: the partial [`AreaKind`] becomes a [`ResolvedAreaKind`] with every field answered. A field no level ever filled is where an arrangement stops being drawable, so it becomes a [`Finding`] naming the area and the field, and that area alone is dropped. The rest of the layer still resolves, because one unfinished bar is not a reason for a user to lose their desktop.

use std::collections::BTreeMap;

use config::{Edge, glob_matches};
use util::report::{Finding, Report};

use crate::merge::{merge_layers, merge_session_layers};
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

/// What one output shows, with every field answered.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub output: String,
    pub workspace: Option<ActiveWorkspace>,
    /// Keyed by layer, and a `BTreeMap` so iteration is bottom-up, which is the order the windows stack in.
    pub layers: BTreeMap<LayerKind, ResolvedLayer>,
    /// What each edge takes off the screen, in `Edge::ALL` order, settled before any workspace rule ran. Read through [`Resolved::reserved`].
    reserved: [f32; 4],
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
        }
    }

    pub fn layer(&self, kind: LayerKind) -> Option<&ResolvedLayer> {
        self.layers.get(&kind)
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
    /// Derived from the output-level arrangement alone — the one every workspace on that screen shares — so switching workspaces can add and remove areas but can never re-tile the user's windows (F-6.7). It is the areas' own depth; the air a floating bar sits in is `[shape] gap`, which only the config can answer.
    pub fn reserved(&self, edge: Edge) -> f32 {
        self.reserved[Edge::ALL.iter().position(|it| *it == edge).unwrap_or(0)]
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResolvedLayer {
    pub areas: Vec<ResolvedArea>,
}

#[derive(Clone, Debug)]
pub struct ResolvedArea {
    pub id: AreaId,
    pub kind: ResolvedAreaKind,
    pub reserve: bool,
    pub above_fullscreen: bool,
    /// Which box this area's geometry is measured in.
    pub within: Within,
    pub style: AreaStyle,
    pub visible: Option<Expr>,
    pub groups: Vec<ResolvedGroup>,
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
        width: f32,
        output_policy: StackOutputPolicy,
        routes: Vec<Route>,
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
    },
    Prompt {
        rect: Rect,
        style: PromptStyle,
    },
}

/// What a resolved texture paints. Exactly one of the two, because an area that named both would otherwise draw whichever the renderer happened to check first.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Image(String),
    Gradient(Gradient),
}

impl ResolvedAreaKind {
    pub fn name(&self) -> &'static str {
        match self {
            ResolvedAreaKind::Bar { .. } => "bar",
            ResolvedAreaKind::Grid { .. } => "grid",
            ResolvedAreaKind::Stack { .. } => "stack",
            ResolvedAreaKind::WallpaperRegion { .. } => "wallpaper_region",
            ResolvedAreaKind::Texture { .. } => "texture",
            ResolvedAreaKind::Dock { .. } => "dock",
            ResolvedAreaKind::Free { .. } => "free",
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

#[derive(Clone, Debug)]
pub struct ResolvedGroup {
    pub id: GroupId,
    pub kind: GroupKind,
    pub children: Vec<ResolvedInstance>,
}

#[derive(Clone, Debug)]
pub struct ResolvedInstance {
    pub id: InstanceId,
    pub module: String,
    pub representation: Representation,
    pub options: toml::Table,
    pub bindings: BTreeMap<String, Expr>,
    pub actions: BTreeMap<Trigger, Action>,
}

/// Resolves `layout` for one output and workspace, following `extends` through `known`.
///
/// Always returns an arrangement. Whatever could not be decided is in the [`Report`] and is missing from the arrangement, so a caller draws what the user asked for minus the parts that were not finished, and says what those were.
pub fn resolve(
    layout: &Layout,
    known: &BTreeMap<LayoutId, Layout>,
    output: &str,
    workspace: Option<&ActiveWorkspace>,
) -> (Resolved, Report) {
    let mut report = Report::default();
    let chain = chain_of(layout, known, &mut report);

    let mut layers = Layers::default();
    // The same arrangement with every workspace rule left out, which is the only thing an exclusive zone may be derived from: a rule that could re-tile the user's windows would do it on every workspace switch (F-6.7). Kept beside rather than recomputed, because it is the same merge and two of them could drift.
    let mut without_rules = Layers::default();
    let mut ruled = false;
    for level in &chain {
        let mut rules: Vec<&OutputRule> = level
            .outputs
            .iter()
            .filter(|rule| matches_output(&rule.matches, output))
            .collect();
        rules.sort_by_key(|rule| rule.matches.specificity());

        for rule in &rules {
            merge_layers(&mut layers, &rule.layers);
            merge_layers(&mut without_rules, &rule.layers);
        }
        for rule in &rules {
            for workspace_rule in &rule.workspaces {
                match matches_workspace(&workspace_rule.matches, workspace) {
                    WorkspaceVerdict::Matches => {
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

    let answered = answer_layers(&layers, layout, &mut report);
    // A workspace rule may only add, remove and restyle; what each edge takes off the screen is settled before any of them runs. Answering the rule-free arrangement a second time is the cost of that, and only where a rule actually matched — its own findings are the ones already reported, so they go to a report nobody reads.
    let reserved = match ruled {
        false => reserved_edges(&answered),
        true => reserved_edges(&answer_layers(
            &without_rules,
            layout,
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
    };
    (resolved, report)
}

fn answer_layers(
    layers: &Layers,
    layout: &Layout,
    report: &mut Report,
) -> BTreeMap<LayerKind, ResolvedLayer> {
    LayerKind::ALL
        .into_iter()
        .map(|kind| {
            (
                kind,
                answer_layer(layer_of(layers, kind), kind, layout, report),
            )
        })
        .collect()
}

/// What each edge of the output is taken by, in `Edge::ALL` order.
fn reserved_edges(layers: &BTreeMap<LayerKind, ResolvedLayer>) -> [f32; 4] {
    Edge::ALL.map(|edge| {
        layers
            .values()
            .flat_map(|layer| layer.areas.iter())
            .filter(|area| area.reserve)
            .filter_map(|area| area.kind.reserving_thickness(edge))
            .sum()
    })
}

fn layer_of(layers: &Layers, kind: LayerKind) -> &Layer {
    match kind {
        LayerKind::Background => &layers.background,
        LayerKind::Desktop => &layers.desktop,
        LayerKind::Top => &layers.top,
        LayerKind::Overlay => &layers.overlay,
        LayerKind::Lock => &layers.lock,
    }
}

/// The layouts to apply, root first. A cycle stops at the layout that closes it, reported once, so a file that extends itself is a message rather than a hang.
fn chain_of<'a>(
    layout: &'a Layout,
    known: &'a BTreeMap<LayoutId, Layout>,
    report: &mut Report,
) -> Vec<&'a Layout> {
    let mut chain = vec![layout];
    let mut seen = vec![layout.id.clone()];
    let mut parent = layout.extends.clone();

    while let Some(id) = parent {
        if seen.contains(&id) {
            report.error(Finding::new(
                layout_path(&layout.id),
                "extends",
                format!("`{id}` extends itself through this chain, so it was not applied"),
            ));
            break;
        }
        let Some(found) = known.get(&id) else {
            report.error(Finding::new(
                layout_path(&layout.id),
                "extends",
                format!("there is no layout called `{id}`"),
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

fn layout_path(id: &LayoutId) -> String {
    format!("layouts/{id}.toml")
}

fn matches_output(pattern: &OutputMatch, output: &str) -> bool {
    pattern.is_every_output() || glob_matches(&pattern.0, output)
}

enum WorkspaceVerdict {
    Matches,
    Differs,
    /// The compositor did not report what the rule matches on.
    Unanswerable(String),
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
            (_, Err(_)) => WorkspaceVerdict::Unanswerable(format!(
                "`{}` is not a workspace number, so this rule never applies",
                pattern.value()
            )),
            (None, Ok(_)) => WorkspaceVerdict::Unanswerable(
                "matching a workspace by number needs Hyprland, which is not running, so this rule is inactive".into(),
            ),
        },
        WorkspaceMatchKind::Special => match active.special {
            Some(true) if glob_matches(pattern.value(), active.name.trim_start_matches("special:")) => {
                WorkspaceVerdict::Matches
            }
            Some(_) => WorkspaceVerdict::Differs,
            None => WorkspaceVerdict::Unanswerable(
                "matching a special workspace needs Hyprland, which is not running, so this rule is inactive".into(),
            ),
        },
    }
}

fn answer_layer(
    layer: &Layer,
    kind: LayerKind,
    layout: &Layout,
    report: &mut Report,
) -> ResolvedLayer {
    let areas = layer
        .areas
        .iter()
        .filter_map(|area| answer_area(area, kind, layout, report))
        .collect();
    ResolvedLayer { areas }
}

fn answer_area(
    area: &Area,
    layer: LayerKind,
    layout: &Layout,
    report: &mut Report,
) -> Option<ResolvedArea> {
    let at = format!("layers.{layer}.areas.{}", area.id);
    let mut miss = |field: &str, kind: &str| {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.{field}"),
            format!(
                "a `{kind}` area needs `{field}`, and nothing set it, so this area was left out"
            ),
        ));
    };

    if area.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("layers.{layer}.areas"),
            "an area has no `id`, so nothing can address it and it was left out".to_string(),
        ));
        return None;
    }

    let Some(kind) = &area.kind else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.kind"),
            "this area never says what kind it is, so it was left out".to_string(),
        ));
        return None;
    };

    let kind = answer_kind(kind, &mut miss)?;

    let groups = if kind.holds_instances() {
        area.groups
            .iter()
            .filter_map(|group| answer_group(group, &at, layout, report))
            .collect()
    } else {
        if !area.groups.is_empty() {
            report.warn(Finding::new(
                layout_path(&layout.id),
                format!("{at}.groups"),
                format!(
                    "a `{}` area is paint and holds nothing, so its groups were left out",
                    kind.name()
                ),
            ));
        }
        Vec::new()
    };

    Some(ResolvedArea {
        id: area.id.clone(),
        kind,
        reserve: area.reserve.unwrap_or(false),
        above_fullscreen: area.above_fullscreen.unwrap_or(false),
        within: area.within.unwrap_or_default(),
        style: area.style.clone(),
        visible: area.visible.clone(),
        groups,
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
            cell: cell.unwrap_or(80.0),
            gap: gap.unwrap_or(16.0),
            anchor: anchor.unwrap_or(Anchor::TopLeft),
        }),
        AreaKind::Stack {
            anchor,
            width,
            output_policy,
            routes,
        } => need(width.is_some(), "width").then(|| ResolvedAreaKind::Stack {
            anchor: anchor.unwrap_or_default(),
            width: width.expect("checked"),
            output_policy: output_policy.clone().unwrap_or_default(),
            routes: routes.clone(),
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
        AreaKind::Free { rect } => need(rect.is_some(), "rect").then(|| ResolvedAreaKind::Free {
            rect: rect.expect("checked"),
        }),
        AreaKind::Prompt { rect, style } => Some(ResolvedAreaKind::Prompt {
            rect: rect.unwrap_or(Rect {
                x: 0.3,
                y: 0.35,
                w: 0.4,
                h: 0.3,
            }),
            style: style.clone(),
        }),
    }
}

fn answer_group(
    group: &Group,
    at: &str,
    layout: &Layout,
    report: &mut Report,
) -> Option<ResolvedGroup> {
    let at = format!("{at}.groups.{}", group.id);
    if group.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            at,
            "a group has no `id`, so nothing can address it and it was left out".to_string(),
        ));
        return None;
    }
    let Some(kind) = group.kind else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.place"),
            "this group never says where it sits, so it was left out".to_string(),
        ));
        return None;
    };
    let children = group
        .children
        .iter()
        .filter_map(|instance| answer_instance(instance, &at, layout, report))
        .collect();
    Some(ResolvedGroup {
        id: group.id.clone(),
        kind,
        children,
    })
}

fn answer_instance(
    instance: &Instance,
    at: &str,
    layout: &Layout,
    report: &mut Report,
) -> Option<ResolvedInstance> {
    let at = format!("{at}.children.{}", instance.id);
    if instance.id.is_empty() {
        report.error(Finding::new(
            layout_path(&layout.id),
            at,
            "an instance has no `id`, so nothing can address it and it was left out".to_string(),
        ));
        return None;
    }
    let Some(module) = instance.module.clone() else {
        report.error(Finding::new(
            layout_path(&layout.id),
            format!("{at}.module"),
            "this instance never says which module it shows, so it was left out".to_string(),
        ));
        return None;
    };
    Some(ResolvedInstance {
        id: instance.id.clone(),
        module,
        representation: instance.representation.unwrap_or(Representation::Chip),
        options: instance.options.clone(),
        bindings: instance.bindings.clone(),
        actions: instance.actions.clone(),
    })
}
