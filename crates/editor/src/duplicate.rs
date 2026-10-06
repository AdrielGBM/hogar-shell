use layout::{
    Area, AreaId, AreaKind, Extent, Group, GroupId, GroupKind, Instance, InstanceId, KomponentId,
    LayerKind, Layout, LayoutOp, Paint, Placement, ResolvedArea, ResolvedAreaKind, ResolvedGroup,
    ResolvedInstance, Spot,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};
use util::report::Message;

use crate::keys::Chord;
use crate::modes::grid::{self, Cells};
use crate::modes::{desktop, top};
use crate::session::{self, EditError, Selection};
use crate::written::Work;
use crate::{context, mode, steps};

const CHILD_NUDGE: f32 = 0.05;
const AREA_NUDGE: f32 = 0.02;

pub(crate) fn chord() -> Chord {
    Chord::char('d').ctrl()
}

/// Copies what `selection` is beside it as one undo entry and selects the copy.
pub(crate) fn duplicate(selection: &Selection) -> Result<(), EditError> {
    let node = selection.node().cloned().ok_or_else(EditError::nothing)?;
    mode::ensure_editing(&node)?;
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let (ops, copy) = duplicated(&session::draft().peek(), &desktop, &node)?;
    let name = steps::name_of(&Selection::of(node));
    context::commit(telar::t!("editor.duplicate.done", name = name), ops)?;
    session::select(Selection::of(copy));
    Ok(())
}

/// What a copy is planned from: the original's entry in the layout file, or what the screen resolves the original to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Written,
    Shown,
}

/// The operations that copy what `node` names on `desktop`'s screen, and the copy: an instance on a grid's or a panel's cells onto the nearest free cells of the same span, any other instance right after itself; a group on cells onto the nearest free cells of its span, one in a zone right after itself, each with what it holds; an area right after itself. Every instance copied gets an id of its own.
///
/// A group or an area is copied from the entry the layout file writes for it wherever that draws the same as the original, so the copy carries no value the file leaves to its default; where the file writes only part of it, the rest inherited, it is copied as the screen shows it.
pub fn duplicated(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
) -> Result<(Vec<LayoutOp>, Node), EditError> {
    let area = Work::new(layout, desktop, node.layer).area(node.layer, &node.area)?;
    match &node.part {
        Part::Instance(group, id) => {
            let holding = held(&area, group)?;
            if let Some(used) = &holding.komponent {
                return Err(komponent_child(node, holding, &used.id));
            }
            let id = id.template();
            if area.kind.places_on_cells() && holding.arrange.is_none() {
                return as_written(layout, desktop, node.layer, |work, form| {
                    group_copy(work, &area, node, (holding, Some(&id)), form)
                });
            }
            let mut work = Work::new(layout, desktop, node.layer);
            let copy = instance_copy(&mut work, &area, node, holding, &id)?;
            Ok((work.done(), copy))
        }
        Part::Group(group) => {
            let shown = held(&area, group)?;
            as_written(layout, desktop, node.layer, |work, form| {
                group_copy(work, &area, node, (shown, None), form)
            })
        }
        Part::Area => {
            if let Some(why) = refusal(&area.kind) {
                return Err(EditError::Refused(why));
            }
            as_written(layout, desktop, node.layer, |work, form| {
                area_copy(work, &area, node, form)
            })
        }
    }
}

/// Why an area of `kind` is not copied: there is one of it, or its kind covers its layer between them. `None` for the kinds that are.
pub(crate) fn refusal(kind: &ResolvedAreaKind) -> Option<Message> {
    match kind {
        ResolvedAreaKind::Bar { .. }
        | ResolvedAreaKind::Stack { .. }
        | ResolvedAreaKind::Texture { .. }
        | ResolvedAreaKind::Free { .. } => None,
        ResolvedAreaKind::Prompt { .. } => Some(util::message!("editor.duplicate.prompt")),
        ResolvedAreaKind::Panel { .. } => Some(util::message!("editor.duplicate.panel")),
        ResolvedAreaKind::WallpaperRegion { .. } => {
            Some(util::message!("editor.duplicate.wallpaper_region"))
        }
        ResolvedAreaKind::Grid { .. } => Some(util::message!("editor.duplicate.grid")),
        ResolvedAreaKind::Dock { .. } => Some(util::message!("editor.duplicate.dock")),
    }
}

/// A komponent's file holds its children for every group that uses it, so one of them has no entry of its own to copy.
fn komponent_child(node: &Node, holding: &ResolvedGroup, komponent: &KomponentId) -> EditError {
    EditError::refused(util::message!(
        "editor.duplicate.komponent_child",
        name = steps::name_of(&Selection::of(node.clone())),
        komponent = komponent.to_string(),
        group = holding.id.to_string()
    ))
}

/// Plans a copy from the original's entry in the layout file and from what the screen shows of it, and keeps the first where both draw the same.
fn as_written(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    plan: impl Fn(&mut Work, Form) -> Result<Node, EditError>,
) -> Result<(Vec<LayoutOp>, Node), EditError> {
    let mut shown = Work::new(layout, desktop, layer);
    let copy = plan(&mut shown, Form::Shown)?;
    let mut written = Work::new(layout, desktop, layer);
    let same = plan(&mut written, Form::Written).is_ok_and(|planned| planned == copy)
        && drawn(&written, &copy) == drawn(&shown, &copy);
    match same {
        true => Ok((written.done(), copy)),
        false => Ok((shown.done(), copy)),
    }
}

/// The area `copy` is drawn in as `work` has planned it, holding the copy alone where it is a group or in one.
fn drawn(work: &Work, copy: &Node) -> Option<ResolvedArea> {
    let mut area = work.area(copy.layer, &copy.area).ok()?;
    if let Part::Group(group) | Part::Instance(group, _) = &copy.part {
        area.groups.retain(|held| held.id == *group);
    }
    Some(area)
}

fn held<'a>(area: &'a ResolvedArea, group: &GroupId) -> Result<&'a ResolvedGroup, EditError> {
    area.groups
        .iter()
        .find(|held| held.id == *group)
        .ok_or_else(|| EditError::gone(&area.id))
}

/// A child of a group that is not on cells by itself — in a container, a stack or a bar's zone — copied right after itself, as the layout file writes it.
fn instance_copy(
    work: &mut Work,
    area: &ResolvedArea,
    node: &Node,
    holding: &ResolvedGroup,
    id: &InstanceId,
) -> Result<Node, EditError> {
    let at = holding
        .children
        .iter()
        .position(|child| child.id == *id)
        .ok_or_else(|| EditError::gone(&area.id))?;
    let written = work.written(node.layer, &area.id)?;
    steps::writes_as_shown(&written, holding)?;
    let mut copy = written
        .area
        .groups
        .iter()
        .find(|held| held.id == holding.id)
        .and_then(|held| held.children.get(at))
        .cloned()
        .ok_or_else(|| EditError::gone(&area.id))?;
    let fresh = fresh_id(work, &copy);
    copy.id = fresh.clone();
    copy.cell = None;
    copy.rect = copy.rect.map(|rect| nudged(rect, CHILD_NUDGE));
    work.apply(vec![LayoutOp::InsertInstance {
        spot: Spot {
            site: written.site.clone(),
            area: area.id.clone(),
            group: holding.id.clone(),
        },
        index: at + 1,
        instance: Box::new(copy),
    }])?;
    Ok(Node::area(node.output.as_deref(), node.layer, &area.id).instance(&holding.id, &fresh))
}

/// A copy of the group `shown` of `area` holding a copy of each of its children, or of the child `only` alone: on the free cells of its span nearest it where `area` places groups on cells, else right after it.
fn group_copy(
    work: &mut Work,
    area: &ResolvedArea,
    node: &Node,
    (shown, only): (&ResolvedGroup, Option<&InstanceId>),
    form: Form,
) -> Result<Node, EditError> {
    let (entry, mut children) = match form {
        Form::Written => written_group(work, node.layer, &area.id, &shown.id)?,
        Form::Shown => shown_group(shown),
    };
    if let Some(only) = only {
        children.retain(|child| child.id == *only);
    }
    let stem = match (only, children.first()) {
        (Some(_), Some(child)) => fresh_id(work, child).to_string(),
        (Some(_), None) => return Err(EditError::gone(&area.id)),
        (None, _) => shown.id.to_string(),
    };
    let id = layout::ops::free_group_id(&work.layout, &work.known, node.layer, &area.id, &stem);
    let kind = match area.kind.places_on_cells() {
        true => Some(cells_near(work, area, shown)?),
        false => entry.kind,
    };
    let copy = Group {
        id: id.clone(),
        kind,
        ..entry
    };
    work.rewrite(node.layer, &area.id, |written| {
        let at = written
            .groups
            .iter()
            .position(|held| held.id == shown.id)
            .map_or(written.groups.len(), |at| at + 1);
        written.groups.insert(at, copy);
    })?;
    let ids = fill(work, node.layer, &area.id, &id, children)?;
    let on = Node::area(node.output.as_deref(), node.layer, &area.id);
    Ok(match (only, ids.first()) {
        (Some(_), Some(child)) => on.instance(&id, child),
        _ => on.group(&id),
    })
}

/// The group `id` of `area` as the layout file's entry writes it, without its children, and those children as written; refused where the file does not write it.
fn written_group(
    work: &Work,
    layer: LayerKind,
    area: &AreaId,
    id: &GroupId,
) -> Result<(Group, Vec<Instance>), EditError> {
    let written = work.written(layer, area)?;
    written
        .area
        .groups
        .into_iter()
        .find(|held| held.id == *id)
        .map(apart)
        .ok_or_else(EditError::nothing)
}

/// A written group without its children, and those children, each without what it takes back from a level under it: a copy has none under it.
fn apart(group: Group) -> (Group, Vec<Instance>) {
    let children = group
        .children
        .into_iter()
        .map(|child| Instance {
            unset: Vec::new(),
            ..child
        })
        .collect();
    let group = Group {
        children: Vec::new(),
        remove: Vec::new(),
        unset: Vec::new(),
        ..group
    };
    (group, children)
}

/// The group `shown` as a layout writes it in full, from what the screen shows, without its children, and those children likewise: a use of a komponent as that use with its parameters, holding none of its own.
fn shown_group(shown: &ResolvedGroup) -> (Group, Vec<Instance>) {
    match &shown.komponent {
        Some(used) => {
            let group = Group {
                id: shown.id.clone(),
                kind: Some(shown.kind),
                komponent: Some(used.id.clone()),
                parameters: used
                    .parameters
                    .iter()
                    .filter_map(|parameter| {
                        let value = parameter.value.as_ref()?;
                        Some((parameter.name.clone(), value.expr.clone()))
                    })
                    .collect(),
                style: shown.style.clone(),
                ..Group::default()
            };
            (group, Vec::new())
        }
        None => (
            shown.written(),
            shown.children.iter().map(shown_child).collect(),
        ),
    }
}

/// `shown` as a layout writes it, where it sits in its group included.
fn shown_child(shown: &ResolvedInstance) -> Instance {
    let mut child = desktop::placed_as(shown);
    match &shown.placement {
        Some(Placement::Weight(weight)) if *weight != 1.0 => child.weight = Some(*weight),
        Some(Placement::Cell(cell)) => child.cell = Some(*cell),
        Some(Placement::Rect(rect)) => child.rect = Some(*rect),
        _ => {}
    }
    child
}

/// Where a copy of the group `shown` of `area` goes on its cells: the free ones of its span nearest it.
fn cells_near(
    work: &Work,
    area: &ResolvedArea,
    shown: &ResolvedGroup,
) -> Result<GroupKind, EditError> {
    let GroupKind::Cell {
        col,
        row,
        col_span,
        row_span,
    } = shown.kind
    else {
        return Err(EditError::no_way(&shown.id));
    };
    let size = grid::cells_of(shown).unwrap_or(Cells::ONE);
    let room = desktop::room_of(work.desktop, area);
    let spot = grid::free_inside(&grid::taken(area), size, (col, row), room).ok_or_else(|| {
        EditError::refused(util::message!(
            "editor.duplicate.no_room",
            name = shown.id.to_string()
        ))
    })?;
    Ok(GroupKind::Cell {
        col: spot.col,
        row: spot.row,
        col_span,
        row_span,
    })
}

/// Puts `children` into the group `id` of `area` one at a time, each under an id no instance took before it, and answers those ids.
fn fill(
    work: &mut Work,
    layer: LayerKind,
    area: &AreaId,
    id: &GroupId,
    children: Vec<Instance>,
) -> Result<Vec<InstanceId>, EditError> {
    let mut ids = Vec::with_capacity(children.len());
    for child in children {
        let fresh = fresh_id(work, &child);
        let copy = Instance {
            id: fresh.clone(),
            ..child
        };
        work.rewrite(layer, area, |written| {
            if let Some(group) = written.groups.iter_mut().find(|held| held.id == *id) {
                group.children.push(copy);
            }
        })?;
        ids.push(fresh);
    }
    Ok(ids)
}

fn fresh_id(work: &Work, child: &Instance) -> InstanceId {
    let stem = child.module.clone().unwrap_or_else(|| child.id.to_string());
    layout::ops::free_instance_id(&work.layout, &work.known, &stem)
}

/// A group's id and the children copied into it.
type Filled = Vec<(GroupId, Vec<Instance>)>;

fn area_copy(
    work: &mut Work,
    area: &ResolvedArea,
    node: &Node,
    form: Form,
) -> Result<Node, EditError> {
    let kind = copied_kind(work, area)?;
    let (entry, children) = match form {
        Form::Written => written_area(work, node.layer, area, kind)?,
        Form::Shown => shown_area(area, kind),
    };
    let id = layout::ops::free_area_id(&work.layout, &work.known, node.layer, area.id.as_str());
    work.insert_after(
        node.layer,
        &area.id,
        Area {
            id: id.clone(),
            ..entry
        },
    )?;
    for (group, children) in children {
        fill(work, node.layer, &id, &group, children)?;
    }
    Ok(Node::area(node.output.as_deref(), node.layer, &id))
}

/// The area as the layout file's entry writes it, its geometry moved where `kind` puts the copy, without its groups' children, and those children as written; refused where the file does not write it.
fn written_area(
    work: &Work,
    layer: LayerKind,
    area: &ResolvedArea,
    kind: AreaKind,
) -> Result<(Area, Filled), EditError> {
    let written = work.written(layer, &area.id)?;
    if !written.is_present() {
        return Err(EditError::nothing());
    }
    let mut entry = written.area;
    let (groups, children) = std::mem::take(&mut entry.groups)
        .into_iter()
        .map(|group| {
            let id = group.id.clone();
            let (group, children) = apart(group);
            (group, (id, children))
        })
        .unzip();
    let entry = Area {
        kind: Some(moved(entry.kind.take(), kind)),
        groups,
        remove: Vec::new(),
        unset: Vec::new(),
        ..entry
    };
    Ok((entry, children))
}

/// `area` as a layout writes it in full, from what the screen shows, at the geometry `kind`, without its groups' children, and those children likewise.
fn shown_area(area: &ResolvedArea, kind: AreaKind) -> (Area, Filled) {
    let (groups, children) = area
        .groups
        .iter()
        .map(|group| {
            let (entry, children) = shown_group(group);
            (entry, (group.id.clone(), children))
        })
        .unzip();
    let entry = Area {
        id: area.id.clone(),
        kind: Some(kind),
        reserve: area.reserve.then_some(true),
        above_fullscreen: area.above_fullscreen.then_some(true),
        within: Some(area.within),
        style: area.style.clone(),
        visible: area.visible.as_ref().map(|visible| visible.expr.clone()),
        groups,
        actions: area.actions.clone(),
        ..Area::default()
    };
    (entry, children)
}

/// The geometry `written` gives the original, with what `copy` moves: a bar's stretch of its edge, a rectangle's place; a stack stays where the original is, and is never where the launcher opens. Where the original writes another kind or none, `copy` itself.
fn moved(written: Option<AreaKind>, copy: AreaKind) -> AreaKind {
    match (written, copy) {
        (
            Some(AreaKind::Bar {
                edge,
                thickness,
                shape,
                autohide,
                ..
            }),
            AreaKind::Bar { length, offset, .. },
        ) => AreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        },
        (
            Some(AreaKind::Stack {
                anchor,
                offset,
                width,
                flow,
                output_policy,
                routes,
                ..
            }),
            AreaKind::Stack { .. },
        ) => AreaKind::Stack {
            anchor,
            offset,
            width,
            flow,
            output_policy,
            routes,
            launcher: None,
        },
        (
            Some(AreaKind::Texture {
                image,
                gradient,
                tile,
                blend,
                opacity,
                ..
            }),
            AreaKind::Texture { rect, .. },
        ) => AreaKind::Texture {
            rect,
            image,
            gradient,
            tile,
            blend,
            opacity,
        },
        (Some(AreaKind::Free { anchor, .. }), AreaKind::Free { rect, .. }) => {
            AreaKind::Free { rect, anchor }
        }
        (_, copy) => copy,
    }
}

/// The geometry of a copy of `area`, in full: a bar in the free stretch of its edge, a stack where the original is, a rectangle moved a little off it.
fn copied_kind(work: &Work, area: &ResolvedArea) -> Result<AreaKind, EditError> {
    match area.kind.clone() {
        ResolvedAreaKind::Bar { .. } => bar_beside(work, area),
        ResolvedAreaKind::Stack {
            anchor,
            offset,
            width,
            flow,
            output_policy,
            routes,
            ..
        } => Ok(AreaKind::Stack {
            anchor: Some(anchor),
            offset: Some(offset),
            width: Some(width),
            flow: Some(flow),
            output_policy: Some(output_policy),
            routes,
            launcher: None,
        }),
        ResolvedAreaKind::Texture {
            rect,
            paint,
            tile,
            blend,
            opacity,
        } => {
            let (image, gradient) = match paint {
                Paint::Image(image) => (Some(image), None),
                Paint::Gradient(gradient) => (None, Some(gradient)),
            };
            Ok(AreaKind::Texture {
                rect: Some(nudged(rect, AREA_NUDGE)),
                image,
                gradient,
                tile: Some(tile),
                blend: Some(blend),
                opacity: Some(opacity),
            })
        }
        ResolvedAreaKind::Free { rect, anchor } => Ok(AreaKind::Free {
            rect: Some(nudged(rect, AREA_NUDGE)),
            anchor: Some(anchor),
        }),
        other => Err(refusal(&other).map_or_else(EditError::nothing, EditError::Refused)),
    }
}

/// A copy of the bar `area` in the longest stretch of its edge no bar is on, as long as it is where that fits.
fn bar_beside(work: &Work, area: &ResolvedArea) -> Result<AreaKind, EditError> {
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        shape,
        autohide,
        ..
    } = area.kind.clone()
    else {
        return Err(EditError::nothing());
    };
    let screen = work.screen();
    let no_room = || {
        EditError::refused(util::message!(
            "editor.top.no_room",
            edge = top::edge_name(edge)
        ))
    };
    let span = top::span_of(&screen, area).ok_or_else(no_room)?;
    let (start, end) = top::free_on(&screen, edge, (span.run_start, span.run_length), None)
        .into_iter()
        .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
        .filter(|(start, end)| end - start >= crate::popover::handles::SHORTEST)
        .ok_or_else(no_room)?;
    Ok(AreaKind::Bar {
        edge: Some(edge),
        thickness: Some(thickness),
        length: Some(Extent::Px(span.along.min(end - start).round())),
        offset: Some((start - span.run_start).round().max(0.0)),
        shape,
        autohide,
    })
}

fn nudged(rect: layout::Rect, by: f32) -> layout::Rect {
    let moved = |at: f32, extent: f32| {
        let at = (at + by).min((1.0 - extent).max(0.0));
        (at * 1000.0).round() / 1000.0
    };
    layout::Rect {
        x: moved(rect.x, rect.w),
        y: moved(rect.y, rect.h),
        ..rect
    }
}
