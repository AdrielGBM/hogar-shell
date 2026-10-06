//! The top mode's tools (TA-5): bars made, moved from edge to edge, split, joined and sent to another screen, and chips carried between zones and bars.
//!
//! **Several bars per edge.** A bar that does not run the whole edge leaves room beside it, and every bar on one edge keeps out of the others' way: a new one fills a free stretch, one moved there fits into one, a slide or a longer length stops at its neighbour ([`room_around`]). Splitting a bar cuts it in two at a point along it, its chips past the cut going to the new bar in the zones they were in; joining two bars that follow each other on an edge makes the first run over both, the second's chips appended zone by zone after its own — so a join undoes a split.
//!
//! **Distinct hotspots (F-7).** A bar's split button sits over the middle of its inner side and the join button over the seam between two bars, never in one place; a split button pressed cuts in half, dragged it cuts where it is let go.
//!
//! **Chips go where the insertion line is.** A chip dragged over a bar shows the line it would be put at, between two chips of the zone under the pointer, on any bar of any edge ([`ChipLanding`]). Carried off every bar and let go it is taken off the layout (F-7's drag out to detach), which one undo brings back: a chip stays an element of its own layer and never becomes one of another's.
//!
//! **Reservation re-tiles on commit only.** Every drag previews through [`crate::session::Edit`], which the windows draw without re-reserving (`surfaces::reconcile::preview`), so the user's windows re-tile once, when the drag is let go (F-6.7).
//!
//! **Every drag has a key (WCAG 2.5.7).** Ctrl+Shift+arrows make a bar on the edge the arrow points at, `s` splits the selected bar in half (or just before the selected chip), Alt+Shift+arrows join it with the bar that way along its edge, `o` sends the bar to the next screen, and Shift+N puts an empty group on a plate in it for chips to be dragged into; the generic Shift+arrows and Ctrl+arrows move and resize bars and chips. A bar's menu has the same rows, and "Move to <screen>" for each other screen: moving a bar between screens is the menu's and the keyboard's until a drag across outputs is checked live on two.
//!
//! **One undo entry each.** A new bar, a move, a split, a join, a chip carried and a bar sent to another screen are each one edit.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use telar::RwSignal;

use config::Edge;
use layout::{
    Action, Area, AreaId, AreaKind, AutoHide, Extent, Group, GroupId, GroupKind, InstanceId,
    LayerKind, Layout, LayoutOp, Library, OutputMatch, OutputRule, ResolvedArea, ResolvedAreaKind,
    ResolvedGroup, Site, Spot, Trigger, Zone,
};
use surfaces::bar::Span;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node, Part};

use crate::context;
use crate::host::StripButton;
use crate::keys::{self, Chord, Direction, KeyOp, Run};
use crate::mode::said;
use crate::popover::area::{chosen, chosen_as, edges, variants};
use crate::popover::handles::{self, SHORTEST};
use crate::popover::look::{self, BarStyle};
use crate::popover::rows::{self, Range, label};
use crate::popover::{AreaDraft, Inspector, help, kind_field};
use crate::session::{self, EditError, Selection};
use crate::written::Work;

use super::desktop::placed_as;

/// How far apart two positions along an edge may be and still count as the same place, in pixels.
const TOUCH: f32 = 0.5;

/// The toolbar's buttons: a new bar on each edge.
const NEW_BARS: [StripButton; 4] = [
    (
        || telar::t!("editor.top.new_bar", edge = edge_name(Edge::Top)),
        || said(create_on(Edge::Top)),
    ),
    (
        || telar::t!("editor.top.new_bar", edge = edge_name(Edge::Bottom)),
        || said(create_on(Edge::Bottom)),
    ),
    (
        || telar::t!("editor.top.new_bar", edge = edge_name(Edge::Left)),
        || said(create_on(Edge::Left)),
    ),
    (
        || telar::t!("editor.top.new_bar", edge = edge_name(Edge::Right)),
        || said(create_on(Edge::Right)),
    ),
];

pub(crate) fn install() {
    crate::popover::add_area_tool("bar", bar_tool);
    crate::host::add_tool(LayerKind::Top, super::bars::tool);
    crate::host::add_tool(LayerKind::Top, super::widgets::tool);
    for button in NEW_BARS {
        crate::host::add_adding_button(LayerKind::Top, button);
    }
    context::add_area_rows("bar", bar_rows);
    keys::add_mode_key_op(
        LayerKind::Top,
        KeyOp {
            name: "plate-create",
            keys: vec![Chord::char('n').shift()],
            label: || telar::t!("editor.keys.op.plate-create"),
            run: Run::Act(|_| super::container::create()),
        },
    );
    keys::add_mode_key_op(
        LayerKind::Top,
        KeyOp {
            name: "bar-create",
            keys: keys::arrows_with(|chord| chord.ctrl().shift()),
            label: || telar::t!("editor.keys.op.bar-create"),
            run: Run::Toward(create_toward),
        },
    );
    keys::add_key_op(
        "bar",
        KeyOp {
            name: "bar-split",
            keys: vec![Chord::char('s')],
            label: || telar::t!("editor.keys.op.bar-split"),
            run: Run::Step(split_step),
        },
    );
    keys::add_key_op(
        "bar",
        KeyOp {
            name: "bar-join",
            keys: keys::arrows_with(|chord| chord.alt().shift()),
            label: || telar::t!("editor.keys.op.bar-join"),
            run: Run::Toward(join_toward),
        },
    );

    keys::add_key_op(
        "bar",
        KeyOp {
            name: "bar-output",
            keys: vec![Chord::char('o')],
            label: || telar::t!("editor.keys.op.bar-output"),
            run: Run::Step(output_step),
        },
    );
}

/// What an edge is called in a sentence.
pub(crate) fn edge_name(edge: Edge) -> String {
    match edge {
        Edge::Top => telar::t!("editor.top.edge.top"),
        Edge::Bottom => telar::t!("editor.top.edge.bottom"),
        Edge::Left => telar::t!("editor.top.edge.left"),
        Edge::Right => telar::t!("editor.top.edge.right"),
    }
}

/// A bar of a screen, and where it lies along its edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub layer: LayerKind,
    pub id: AreaId,
    pub edge: Edge,
    pub span: Span,
}

/// Where the bar `area` lies along its edge on `desktop`'s screen.
pub(crate) fn span_of(desktop: &Desktop, area: &ResolvedArea) -> Option<Span> {
    surfaces::bar::span_of(
        area,
        &desktop.config,
        desktop.reserved.box_of(area.within, desktop.size),
        desktop.reserved,
    )
}

/// Every bar on `desktop`'s screen, whichever layer it is on: all of them hug the same edges.
pub fn bars_of(desktop: &Desktop) -> Vec<Placed> {
    desktop
        .resolved
        .areas()
        .filter_map(|(layer, area)| {
            let edge = match area.kind {
                ResolvedAreaKind::Bar { edge, .. } => edge,
                _ => return None,
            };
            Some(Placed {
                layer,
                id: area.id.clone(),
                edge,
                span: span_of(desktop, area)?,
            })
        })
        .collect()
}

/// The stretches of `edge` no bar but `except` is on, in order along it, within `run`.
pub fn free_on(
    desktop: &Desktop,
    edge: Edge,
    run: (f32, f32),
    except: Option<(LayerKind, &AreaId)>,
) -> Vec<(f32, f32)> {
    let mut taken: Vec<(f32, f32)> = bars_of(desktop)
        .into_iter()
        .filter(|bar| bar.edge == edge && Some((bar.layer, &bar.id)) != except)
        .map(|bar| (bar.span.at, bar.span.end()))
        .collect();
    taken.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut from, until) = (run.0, run.0 + run.1);
    let mut free = Vec::new();
    for (start, end) in taken {
        if start - from > TOUCH {
            free.push((from, start.min(until)));
        }
        from = from.max(end);
    }
    if until - from > TOUCH {
        free.push((from, until));
    }
    free
}

/// The stretch of its edge the bar `id` of `layer` can use without running into another bar: from where the one before it ends to where the one after it starts. A bar lying over it already constrains nothing, since there is nowhere it could be kept out of.
pub fn room_around(desktop: &Desktop, layer: LayerKind, id: &AreaId) -> Option<(f32, f32)> {
    let bars = bars_of(desktop);
    let own = bars
        .iter()
        .find(|bar| bar.layer == layer && bar.id == *id)?;
    let (mut low, mut high) = (own.span.run_start, own.span.run_end());
    for other in bars
        .iter()
        .filter(|bar| bar.edge == own.edge && !(bar.layer == layer && bar.id == *id))
    {
        if other.span.end() <= own.span.at + TOUCH {
            low = low.max(other.span.end());
        } else if other.span.at >= own.span.end() - TOUCH {
            high = high.min(other.span.at);
        }
    }
    Some((low, high))
}

/// Where a bar is put on an edge that has other bars on it: in the free stretch the pointer is in or nearest, `along` long if that fits and as long as the stretch is if not. `None` when no stretch is long enough for a bar.
fn fitted(free: &[(f32, f32)], along: Option<f32>, near: Option<f32>) -> Option<(f32, f32)> {
    let chosen = match near {
        Some(near) => free
            .iter()
            .filter(|(start, end)| end - start >= SHORTEST)
            .min_by(|a, b| distance(**a, near).total_cmp(&distance(**b, near))),
        None => free
            .iter()
            .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0))),
    }?;
    let (start, end) = *chosen;
    if end - start < SHORTEST {
        return None;
    }
    let length = along.map_or(end - start, |along| along.min(end - start));
    let at = match near {
        Some(near) => (near - length / 2.0).clamp(start, end - length),
        None => start,
    };
    Some((at, length))
}

fn distance((start, end): (f32, f32), point: f32) -> f32 {
    if point < start {
        start - point
    } else if point > end {
        point - end
    } else {
        0.0
    }
}

/// The bar `id` of the edit's layer, as the layout planned so far resolves it.
fn bar(work: &Work, id: &AreaId) -> Result<ResolvedArea, EditError> {
    let area = work.area(work.layer, id)?;
    match area.kind {
        ResolvedAreaKind::Bar { .. } => Ok(area),
        _ => Err(EditError::refused(telar::t!(
            "editor.top.not_a_bar",
            id = id.to_string()
        ))),
    }
}

/// Where `area` lies along its edge on the screen as the layout planned so far arranges it.
fn span(work: &Work, area: &ResolvedArea) -> Result<Span, EditError> {
    span_of(&work.screen(), area).ok_or_else(EditError::nothing)
}

/// The bar `id` placed `length` px along its edge from `at`, both on its screen: `length` running the whole edge where that is what it covers.
fn place(work: &mut Work, id: &AreaId, at: f32, length: f32) -> Result<(), EditError> {
    let now = span(work, &bar(work, id)?)?;
    let whole = (at - now.run_start).abs() <= TOUCH && (at + length - now.run_end()).abs() <= TOUCH;
    let (extent, offset) = match whole {
        true => (Extent::Fill, 0.0),
        false => (
            Extent::Px(length.round()),
            (at - now.run_start).round().max(0.0),
        ),
    };
    work.rewrite(work.layer, id, |written| {
        kind_field!(written, "bar", Bar { length }, extent);
        kind_field!(written, "bar", Bar { offset }, offset);
    })
}

/// The run a bar made on `edge` places itself along: the one the shipped bar would have there.
pub(crate) fn new_run(desktop: &Desktop, edge: Edge) -> (f32, f32) {
    let (gap, hides) = match layout::default_bar(AreaId::new(""), edge).kind {
        Some(AreaKind::Bar {
            shape, autohide, ..
        }) => (
            desktop
                .config
                .gap_of(&surfaces::bar::bar_shape(&desktop.config, shape)) as f32,
            autohide.is_some(),
        ),
        _ => (0.0, false),
    };
    surfaces::bar::run_along(
        edge,
        desktop
            .reserved
            .box_of(layout::Within::Output, desktop.size),
        desktop.reserved,
        gap,
        hides,
    )
}

/// A new bar on `edge` of `layer`, as the shipped one is ([`layout::default_bar`]): the whole edge where it is free, else the free stretch the pointer is in or nearest `near` along it. Made for one workspace alone it does not reserve, since a workspace rule never re-tiles windows (TA-2).
pub(crate) fn created(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    edge: Edge,
    near: Option<f32>,
) -> Result<(Vec<LayoutOp>, AreaId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let id = layout::ops::free_area_id(
        &work.layout,
        &work.known,
        layer,
        &format!("bar-{}", edge.as_str()),
    );
    let run = new_run(desktop, edge);
    let free = free_on(&work.screen(), edge, run, None);
    let whole = free.len() == 1 && (free[0].1 - free[0].0 - run.1).abs() <= TOUCH;
    let mut area = layout::default_bar(id.clone(), edge);
    area.reserve = Some(work.workspace.is_none());
    if !whole {
        let (at, length) = fitted(&free, None, near).ok_or_else(|| {
            EditError::refused(telar::t!("editor.top.no_room", edge = edge_name(edge)))
        })?;
        if let Some(AreaKind::Bar {
            length: extent,
            offset,
            ..
        }) = &mut area.kind
        {
            *extent = Some(Extent::Px(length.round()));
            *offset = Some((at - run.0).round().max(0.0));
        }
    }
    work.add(layer, area)?;
    Ok((work.done(), id))
}

/// The bar `id` of `layer` moved to `edge`, fitted beside what is there already: in the free stretch nearest `near` along it, as long as it was where that fits, and centred on `near` where it does not run the whole edge.
pub(crate) fn moved_to_edge(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    edge: Edge,
    near: Option<f32>,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let area = bar(&work, id)?;
    let ResolvedAreaKind::Bar {
        edge: from, length, ..
    } = area.kind
    else {
        return Err(EditError::nothing());
    };
    if from == edge {
        return Err(EditError::no_way(id));
    }
    let was = span(&work, &area)?;
    work.rewrite(layer, id, |written| {
        kind_field!(written, "bar", Bar { edge }, edge)
    })?;
    let run = span(&work, &bar(&work, id)?)?;
    let free = free_on(
        &work.screen(),
        edge,
        (run.run_start, run.run_length),
        Some((layer, id)),
    );
    let alone = free.len() == 1 && (free[0].1 - free[0].0 - run.run_length).abs() <= TOUCH;
    if alone && length == Extent::Fill {
        return Ok(work.done());
    }
    let along = (length != Extent::Fill).then_some(was.along);
    let (at, length) = fitted(&free, along, near).ok_or_else(|| {
        EditError::refused(telar::t!("editor.top.no_room", edge = edge_name(edge)))
    })?;
    place(&mut work, id, at, length)?;
    Ok(work.done())
}

/// The bar `id` of `layer` slid along its own edge so it is centred on `near`, kept clear of its neighbours. A bar running the whole edge has nowhere to slide.
pub(crate) fn slid(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    near: f32,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let now = span(&work, &bar(&work, id)?)?;
    let (low, high) = room_around(&work.screen(), layer, id).ok_or_else(EditError::nothing)?;
    let at = (near - now.along / 2.0).clamp(low, (high - now.along).max(low));
    if (at - now.at).abs() <= TOUCH {
        return Ok(Vec::new());
    }
    place(&mut work, id, at, now.along)?;
    Ok(work.done())
}

/// Where each group and chip of one bar is drawn along its edge, from its first pixel to its last: taken before a gesture previews anything, so what the gesture plans holds still under its own preview.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn(Vec<(Part, (f32, f32))>);

impl Drawn {
    /// The bar `id` of `layer` on the screen `output`, as its window draws it now.
    pub fn of(output: Option<&str>, layer: LayerKind, id: &AreaId, edge: Edge) -> Self {
        Self(
            rects::on(output, layer)
                .into_iter()
                .filter(|(node, _)| node.area == *id && node.part != Part::Area)
                .map(|(node, rect)| {
                    let along = match edge.is_vertical() {
                        true => (rect.y, rect.y + rect.height),
                        false => (rect.x, rect.x + rect.width),
                    };
                    (node.part, along)
                })
                .collect(),
        )
    }

    pub fn along(&self, part: &Part) -> Option<(f32, f32)> {
        self.0
            .iter()
            .find(|(held, _)| held == part)
            .map(|(_, along)| *along)
    }

    /// Whether the group or chip `part` lies past a cut at `at`: its middle does.
    fn past(&self, part: &Part, at: f32) -> bool {
        self.along(part)
            .is_some_and(|(from, until)| (from + until) / 2.0 >= at)
    }
}

/// The bar `id` of `layer` cut in two at `at` along its edge: it keeps what is before the cut, and a new bar beside it takes the rest, with every chip `drawn` past the cut in the zone it was in. Each part is at least as long as the shortest bar.
pub(crate) fn split(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    at: f32,
    drawn: &Drawn,
) -> Result<(Vec<LayoutOp>, AreaId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let area = bar(&work, id)?;
    let now = span(&work, &area)?;
    let at = at.round();
    if at - now.at < SHORTEST || now.end() - at < SHORTEST {
        return Err(EditError::refused(telar::t!(
            "editor.top.too_short",
            name = id.to_string()
        )));
    }
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        shape,
        autohide,
        ..
    } = area.kind
    else {
        return Err(EditError::nothing());
    };
    let moving: Vec<(ResolvedGroup, Vec<InstanceId>)> = area
        .groups
        .iter()
        .filter_map(|group| {
            let children: Vec<InstanceId> = match group.is_pages() {
                true if drawn.past(&Part::Group(group.id.clone()), at) => group
                    .children
                    .iter()
                    .map(|child| child.id.clone())
                    .collect(),
                true => Vec::new(),
                false => group
                    .children
                    .iter()
                    .filter(|child| {
                        drawn.past(&Part::Instance(group.id.clone(), child.id.clone()), at)
                    })
                    .map(|child| child.id.clone())
                    .collect(),
            };
            (!children.is_empty()).then(|| (group.clone(), children))
        })
        .collect();
    let source = work.written(layer, id)?;
    for (group, _) in &moving {
        crate::steps::writes_as_shown(&source, group)?;
    }
    place(&mut work, id, now.at, at - now.at)?;
    let new = layout::ops::free_area_id(&work.layout, &work.known, layer, id.as_str());
    let rest = Area {
        id: new.clone(),
        kind: Some(AreaKind::Bar {
            edge: Some(edge),
            thickness: Some(thickness),
            length: Some(Extent::Px(now.end() - at)),
            offset: Some((at - now.run_start).max(0.0)),
            shape,
            autohide,
        }),
        reserve: Some(area.reserve),
        above_fullscreen: Some(area.above_fullscreen),
        within: Some(area.within),
        style: area.style.clone(),
        actions: area.actions.clone(),
        groups: moving.iter().map(|(group, _)| group.written()).collect(),
        ..Area::default()
    };
    work.insert_after(layer, id, rest)?;
    let site = work.written(layer, id)?.site;
    let moves = moving
        .iter()
        .flat_map(|(group, children)| {
            children
                .iter()
                .enumerate()
                .map(|(index, child)| LayoutOp::MoveInstance {
                    from: Spot {
                        site: site.clone(),
                        area: id.clone(),
                        group: group.id.clone(),
                    },
                    to: Spot {
                        site: site.clone(),
                        area: new.clone(),
                        group: group.id.clone(),
                    },
                    id: child.clone(),
                    index,
                })
        })
        .collect();
    work.apply(moves)?;
    Ok((work.done(), new))
}

/// The bars `first` and `second` of `layer`, which follow each other on one edge, made one: the one that comes first along the edge runs over both, and the other's chips follow its own, zone by zone.
pub(crate) fn joined(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    first: &AreaId,
    second: &AreaId,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let (a, b) = (bar(&work, first)?, bar(&work, second)?);
    let (span_a, span_b) = (span(&work, &a)?, span(&work, &b)?);
    let apart = || {
        EditError::refused(telar::t!(
            "editor.top.not_beside",
            name = first.to_string(),
            other = second.to_string()
        ))
    };
    if a.kind.edge() != b.kind.edge() {
        return Err(apart());
    }
    let ((lead, lead_span), (trail, trail_span)) = match span_a.at <= span_b.at {
        true => ((a, span_a), (b, span_b)),
        false => ((b, span_b), (a, span_a)),
    };
    let edge = lead.kind.edge().ok_or_else(EditError::nothing)?;
    let between = bars_of(&work.screen()).into_iter().any(|bar| {
        bar.edge == edge
            && bar.id != lead.id
            && bar.id != trail.id
            && bar.span.at >= lead_span.end() - TOUCH
            && bar.span.end() <= trail_span.at + TOUCH
    });
    if between || trail_span.at < lead_span.end() - TOUCH {
        return Err(apart());
    }
    let carried = carried_actions(&lead, &trail)?;
    let lead_written = work.written(layer, &lead.id)?;
    let trail_written = work.written(layer, &trail.id)?;
    let mut landing: Vec<(ResolvedGroup, Option<GroupId>)> = Vec::new();
    for group in crate::steps::along(&trail) {
        crate::steps::writes_as_shown(&trail_written, group)?;
        let into = group
            .arrange
            .is_none()
            .then(|| {
                lead.groups
                    .iter()
                    .find(|held| held.kind == group.kind && held.arrange.is_none())
            })
            .flatten();
        if let Some(held) = into {
            crate::steps::writes_as_shown(&lead_written, held)?;
        }
        landing.push((group.clone(), into.map(|held| held.id.clone())));
    }
    place(
        &mut work,
        &lead.id,
        lead_span.at,
        trail_span.end() - lead_span.at,
    )?;
    if !carried.is_empty() {
        work.rewrite(layer, &lead.id, |written| written.actions.extend(carried))?;
    }
    let mut moving: Vec<(ResolvedGroup, GroupId)> = Vec::new();
    for (group, into) in landing {
        let into = match into {
            Some(held) => held,
            None => {
                let id = layout::ops::free_group_id(
                    &work.layout,
                    &work.known,
                    layer,
                    &lead.id,
                    group.id.as_str(),
                );
                let made = Group {
                    id: id.clone(),
                    ..group.written()
                };
                work.rewrite(layer, &lead.id, |written| written.groups.push(made))?;
                id
            }
        };
        moving.push((group, into));
    }
    let to_site = work.written(layer, &lead.id)?.site;
    let from_site = work.written(layer, &trail.id)?.site;
    for (group, into) in &moving {
        let already = bar(&work, &lead.id)?
            .groups
            .iter()
            .find(|held| held.id == *into)
            .map_or(0, |held| held.children.len());
        let moves = group
            .children
            .iter()
            .enumerate()
            .map(|(index, child)| LayoutOp::MoveInstance {
                from: Spot {
                    site: from_site.clone(),
                    area: trail.id.clone(),
                    group: group.id.clone(),
                },
                to: Spot {
                    site: to_site.clone(),
                    area: lead.id.clone(),
                    group: into.clone(),
                },
                id: child.id.clone(),
                index: already + index,
            })
            .collect();
        work.apply(moves)?;
    }
    work.remove(layer, &trail.id)?;
    Ok(work.done())
}

/// What the bar `trail` runs on its own background that the bar `lead` it is joined into has to take on, so a join keeps every gesture either bar answered: each one `lead` does not bind, and nothing for one both bind alike. A gesture both bind to different chains is refused rather than one of them dropped, naming it, so the user decides which stays before joining.
fn carried_actions(
    lead: &ResolvedArea,
    trail: &ResolvedArea,
) -> Result<BTreeMap<Trigger, Action>, EditError> {
    let mut carried = BTreeMap::new();
    for (trigger, chain) in &trail.actions {
        match lead.actions.get(trigger) {
            None => {
                carried.insert(*trigger, chain.clone());
            }
            Some(held) if held == chain => {}
            Some(_) => {
                return Err(EditError::refused(telar::t!(
                    "editor.top.join_actions_clash",
                    name = lead.id.to_string(),
                    other = trail.id.to_string(),
                    gesture = trigger.as_str()
                )));
            }
        }
    }
    Ok(carried)
}

/// Where a carried chip is put on a bar: into `zone` of the bar `area`, before the chip that is `index`th in that zone once the carried one is out of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipLanding {
    pub area: AreaId,
    pub zone: Zone,
    pub index: usize,
}

/// The chip `node` names carried to `landing`, on any bar of its layer: into the group of that zone the index falls in, or a new one where the zone has none. A group the edited layout only inherits cannot be reordered from here (F-10.51), so a move into or out of one is refused.
pub(crate) fn chip_moved(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    landing: &ChipLanding,
) -> Result<Vec<LayoutOp>, EditError> {
    let Part::Instance(from_group, id) = &node.part else {
        return Err(EditError::nothing());
    };
    let id = &id.template();
    let layer = node.layer;
    let mut work = Work::new(layout, desktop, layer);
    let source = bar(&work, &node.area)?;
    let holding = source
        .groups
        .iter()
        .find(|group| group.id == *from_group)
        .ok_or_else(EditError::nothing)?;
    crate::steps::writes_as_shown(&work.written(layer, &node.area)?, holding)?;
    let (group, index) = zone_slot(&mut work, landing, Some(id))?;
    let now = holding.children.iter().position(|child| child.id == *id);
    if node.area == landing.area && group == *from_group && now == Some(index) {
        return Ok(Vec::new());
    }
    let from = work.written(layer, &node.area)?.site;
    let to = work.written(layer, &landing.area)?.site;
    work.apply(vec![LayoutOp::MoveInstance {
        from: Spot {
            site: from,
            area: node.area.clone(),
            group: from_group.clone(),
        },
        to: Spot {
            site: to,
            area: landing.area.clone(),
            group,
        },
        id: id.clone(),
        index,
    }])?;
    Ok(work.done())
}

/// A group the edited layout only inherits is refused.
fn zone_slot(
    work: &mut Work,
    landing: &ChipLanding,
    carried: Option<&InstanceId>,
) -> Result<(GroupId, usize), EditError> {
    let layer = work.layer;
    let target = bar(work, &landing.area)?;
    let zone_groups: Vec<&ResolvedGroup> = target
        .groups
        .iter()
        .filter(|group| {
            group.kind == GroupKind::Zone { zone: landing.zone } && group.arrange.is_none()
        })
        .collect();
    let others = |group: &ResolvedGroup| {
        group
            .children
            .iter()
            .filter(|child| Some(&child.id) != carried)
            .count()
    };
    let mut left = landing.index;
    let mut into: Option<(GroupId, usize)> = None;
    for group in &zone_groups {
        let count = others(group);
        if left <= count {
            into = Some((group.id.clone(), left));
            break;
        }
        left -= count;
    }
    match into.or_else(|| {
        zone_groups
            .last()
            .map(|group| (group.id.clone(), others(group)))
    }) {
        Some((group, index)) => {
            let held = target
                .groups
                .iter()
                .find(|held| held.id == group)
                .ok_or_else(EditError::nothing)?;
            crate::steps::writes_as_shown(&work.written(layer, &landing.area)?, held)?;
            Ok((group, index))
        }
        None => {
            let group = layout::ops::free_group_id(
                &work.layout,
                &work.known,
                layer,
                &landing.area,
                zone_name(landing.zone),
            );
            let made = Group {
                id: group.clone(),
                kind: Some(GroupKind::Zone { zone: landing.zone }),
                ..Group::default()
            };
            work.rewrite(layer, &landing.area, |written| written.groups.push(made))?;
            Ok((group, 0))
        }
    }
}

/// Puts a new, empty group on a plate in the selected bar, in the zone of what is selected there (the start zone of the layer's first bar otherwise), as one undo entry, and selects it. A bar's zones lay their chips out themselves, so what is made is a group of chips drawn on a plate, never a container.
pub(crate) fn plate() -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let (bar, zone) = super::container::bar_near(&desktop, mode.layer)
        .ok_or_else(|| EditError::refused(telar::t!("editor.container.no_bar")))?;
    let (ops, id) =
        super::container::in_bar(&session::draft().peek(), &desktop, mode.layer, &bar, zone)?;
    context::commit(plated(&id), ops)?;
    session::select(Selection::Group(
        Node::area(Some(&mode.output), mode.layer, &bar).group(&id),
    ));
    Ok(())
}

/// What the history calls putting the group `id` on a plate in a bar.
pub(crate) fn plated(id: &GroupId) -> String {
    telar::t!("editor.top.plate.made", name = id.to_string())
}

pub(crate) fn chip_added(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    module: &str,
    landing: &ChipLanding,
) -> Result<(Vec<LayoutOp>, InstanceId), EditError> {
    let descriptor = ui::descriptor::find(module).ok_or_else(|| {
        EditError::refused(telar::t!("editor.desktop.unknown_module", module = module))
    })?;
    if descriptor.representations.chip.is_none() {
        return Err(EditError::refused(telar::t!(
            "editor.top.no_chip",
            name = descriptor.name
        )));
    }
    let mut work = Work::new(layout, desktop, layer);
    let instance = super::desktop::fresh(&work, module, layout::Representation::Chip)?;
    let id = instance.id.clone();
    let (group, index) = zone_slot(&mut work, landing, None)?;
    work.rewrite(layer, &landing.area, |written| {
        put_chip(written, (&group, landing.zone), index, instance)
    })?;
    Ok((work.done(), id))
}

/// Puts `instance` at `index` of the group `group` of `bar`, making that group in `zone` where the bar does not write it.
pub(crate) fn put_chip(
    bar: &mut Area,
    (group, zone): (&GroupId, Zone),
    index: usize,
    instance: layout::Instance,
) {
    let at = match bar.groups.iter().position(|held| held.id == *group) {
        Some(at) => at,
        None => {
            bar.groups.push(Group {
                id: group.clone(),
                kind: Some(GroupKind::Zone { zone }),
                ..Group::default()
            });
            bar.groups.len() - 1
        }
    };
    let held = &mut bar.groups[at];
    held.children
        .insert(index.min(held.children.len()), instance);
}

fn zone_name(zone: Zone) -> &'static str {
    match zone {
        Zone::Start => "start",
        Zone::Center => "center",
        Zone::End => "end",
    }
}

/// Whether `layout` places the area `id` on `layer` of `desktop`'s screen.
fn places(
    layout: &Layout,
    known: &Library,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
) -> bool {
    desktop
        .resolving(layout, known)
        .resolved
        .area(layer, id)
        .is_some()
}

/// The index of the output rule written for `output` alone, made first where the layout has none.
fn own_rule(work: &mut Work, output: &str) -> Result<usize, EditError> {
    if let Some(at) = work
        .layout
        .outputs
        .iter()
        .position(|rule| rule.matches.0 == output)
    {
        return Ok(at);
    }
    let index = work.layout.outputs.len();
    work.apply(vec![LayoutOp::InsertOutputRule {
        index,
        rule: Box::new(OutputRule {
            matches: OutputMatch(output.to_string()),
            ..OutputRule::default()
        }),
    }])?;
    Ok(index)
}

/// The bar `id` of `layer` moved from its screen to the screen `to`, arranged as it was: written into the rule for `to` alone (made for it where there is none) unless a rule both screens share already places it there, and taken off its own screen alone — deleted from a rule that speaks for it but not for `to`, else named in the `remove` of its screen's own rule.
pub(crate) fn moved_to_output(
    layout: &Layout,
    from: &Desktop,
    onto: &Desktop,
    layer: LayerKind,
    id: &AreaId,
) -> Result<Vec<LayoutOp>, EditError> {
    let (Some(source), Some(to)) = (from.output.as_deref(), onto.output.as_deref()) else {
        return Err(EditError::nothing());
    };
    if source == to {
        return Err(EditError::refused(telar::t!(
            "editor.top.no_other_output",
            name = id.to_string()
        )));
    }
    let mut work = Work::new(layout, from, layer);
    work.workspace = None;
    let area = bar(&work, id)?;
    if !places(&work.layout, &work.known, onto, layer, id) {
        let rule = own_rule(&mut work, to)?;
        let site = Site::new(to, layer);
        let removed = &work.layout.outputs[rule].layers.get(layer).remove;
        if removed.contains(id) {
            let remove = removed.iter().filter(|gone| *gone != id).cloned().collect();
            work.apply(vec![LayoutOp::SetLayerRemove {
                site: site.clone(),
                remove,
            }])?;
        }
        if !places(&work.layout, &work.known, onto, layer, id) {
            let whole = written_as(&area);
            let areas = layout::ops::areas_at(&work.layout, &site);
            let op = match areas.iter().any(|written| written.id == *id) {
                true => LayoutOp::ReplaceArea {
                    site,
                    id: id.clone(),
                    area: Box::new(whole),
                },
                false => LayoutOp::InsertArea {
                    index: areas.len(),
                    site,
                    area: Box::new(whole),
                },
            };
            work.apply(vec![op])?;
        }
    }
    let written = work.written(layer, id)?;
    let speaks_for_both = work
        .layout
        .outputs
        .iter()
        .find(|rule| rule.matches == written.site.output)
        .is_some_and(|rule| rule.matches.matches(to));
    if written.is_present() && !speaks_for_both {
        work.apply(vec![LayoutOp::DeleteArea {
            site: written.site.clone(),
            id: id.clone(),
        }])?;
    }
    if places(&work.layout, &work.known, from, layer, id) {
        let rule = own_rule(&mut work, source)?;
        let mut remove = work.layout.outputs[rule].layers.get(layer).remove.clone();
        remove.push(id.clone());
        work.apply(vec![LayoutOp::SetLayerRemove {
            site: Site::new(source, layer),
            remove,
        }])?;
    }
    let arrived = places(&work.layout, &work.known, onto, layer, id);
    let left = !places(&work.layout, &work.known, from, layer, id);
    if !(arrived && left) {
        return Err(EditError::refused(telar::t!(
            "editor.top.not_moved",
            name = id.to_string(),
            output = to
        )));
    }
    Ok(work.done())
}

/// An area as a layout writes it in full, from what a screen shows of it.
fn written_as(area: &ResolvedArea) -> Area {
    let kind = match &area.kind {
        ResolvedAreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        } => Some(AreaKind::Bar {
            edge: Some(*edge),
            thickness: Some(*thickness),
            length: Some(*length),
            offset: Some(*offset),
            shape: *shape,
            autohide: *autohide,
        }),
        _ => None,
    };
    Area {
        id: area.id.clone(),
        kind,
        reserve: Some(area.reserve),
        above_fullscreen: Some(area.above_fullscreen),
        within: Some(area.within),
        style: area.style.clone(),
        visible: area.visible.as_ref().map(|visible| visible.expr.clone()),
        groups: area
            .groups
            .iter()
            .map(|group| Group {
                children: group.children.iter().map(placed_as).collect(),
                ..group.written()
            })
            .collect(),
        actions: area.actions.clone(),
        ..Area::default()
    }
}

/// The screens other than `output`, in the order the shell draws them.
pub(crate) fn other_screens(output: Option<&str>) -> Vec<Desktop> {
    reconcile::desktops()
        .iter()
        .filter(|desktop| desktop.output.is_some() && desktop.output.as_deref() != output)
        .cloned()
        .collect()
}

/// The bar the selection is, or is in.
fn selected_bar(selection: &Selection) -> Result<(Node, Desktop), EditError> {
    let node = selection.node().ok_or_else(EditError::nothing)?.clone();
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    Ok((node, desktop))
}

/// Ctrl+Shift+arrows: a new bar on the edge the arrow points at.
fn create_toward(
    _: &Selection,
    layout: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    created(layout, &desktop, mode.layer, direction.edge(), None).map(|(ops, _)| ops)
}

/// `s`: the selected bar split in half — or, with one of its chips selected, just before that chip.
fn split_step(selection: &Selection, layout: &Layout) -> Result<Vec<LayoutOp>, EditError> {
    let (node, desktop) = selected_bar(selection)?;
    let area = bar(&Work::new(layout, &desktop, node.layer), &node.area)?;
    let edge = area.kind.edge().ok_or_else(EditError::nothing)?;
    let now = span_of(&desktop, &area).ok_or_else(EditError::nothing)?;
    let drawn = Drawn::of(node.output.as_deref(), node.layer, &node.area, edge);
    let at = match &node.part {
        Part::Instance(..) => drawn.along(&node.part).map(|(from, _)| from),
        _ => None,
    }
    .unwrap_or(now.at + now.along / 2.0);
    split(layout, &desktop, node.layer, &node.area, at, &drawn).map(|(ops, _)| ops)
}

/// The bar that follows `id` on its edge the way `forward` says, with nothing between them.
pub(crate) fn beside(
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    forward: bool,
) -> Option<AreaId> {
    let bars = bars_of(desktop);
    let own = bars
        .iter()
        .find(|bar| bar.layer == layer && bar.id == *id)?;
    let same_edge = bars
        .iter()
        .filter(|bar| bar.layer == layer && bar.edge == own.edge && bar.id != *id);
    match forward {
        true => same_edge
            .filter(|bar| bar.span.at >= own.span.end() - TOUCH)
            .min_by(|a, b| a.span.at.total_cmp(&b.span.at)),
        false => same_edge
            .filter(|bar| bar.span.end() <= own.span.at + TOUCH)
            .max_by(|a, b| a.span.end().total_cmp(&b.span.end())),
    }
    .map(|bar| bar.id.clone())
}

/// Alt+Shift+arrows: the selected bar joined with the one that way along its edge.
fn join_toward(
    selection: &Selection,
    layout: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let (node, desktop) = selected_bar(selection)?;
    let area = bar(&Work::new(layout, &desktop, node.layer), &node.area)?;
    let edge = area.kind.edge().ok_or_else(EditError::nothing)?;
    let none_that_way = || {
        EditError::refused(telar::t!(
            "editor.top.no_bar_that_way",
            name = node.area.to_string()
        ))
    };
    if direction.is_horizontal() != edge.is_horizontal() {
        return Err(none_that_way());
    }
    let other = beside(&desktop, node.layer, &node.area, direction.is_forward())
        .ok_or_else(none_that_way)?;
    joined(layout, &desktop, node.layer, &node.area, &other)
}

/// `o`: the selected bar sent to the screen after its own, round to the first after the last.
fn output_step(selection: &Selection, layout: &Layout) -> Result<Vec<LayoutOp>, EditError> {
    let (node, desktop) = selected_bar(selection)?;
    let screens: Vec<Desktop> = reconcile::desktops_now().iter().cloned().collect();
    let at = screens
        .iter()
        .position(|screen| screen.output == node.output)
        .ok_or_else(EditError::nothing)?;
    let next = (1..screens.len())
        .map(|step| &screens[(at + step) % screens.len()])
        .find(|screen| screen.output.is_some())
        .ok_or_else(|| {
            EditError::refused(telar::t!(
                "editor.top.no_other_output",
                name = node.area.to_string()
            ))
        })?;
    moved_to_output(layout, &desktop, next, node.layer, &node.area)
}

/// Makes a new bar on `edge` of the screen being edited, as one undo entry, and selects it.
pub(crate) fn create_on(edge: Edge) -> Result<(), EditError> {
    context::make(
        |layout, desktop, layer| created(layout, desktop, layer, edge, None),
        |id| telar::t!("editor.top.made", name = id.to_string()),
    )
}

/// A bar's menu rows: adding a komponent to one of its zones, splitting it, joining it with the bar before or after it and sending it to each other screen, and, in its own mode, a new bar on each edge.
fn bar_rows(area: &ResolvedArea, node: &Node) -> Vec<telar::MenuEntry> {
    let Some(desktop) = reconcile::desktop(node.output.as_deref()) else {
        return Vec::new();
    };
    let mut rows: Vec<telar::MenuEntry> = crate::komponent::bar_entry(node).into_iter().collect();
    if let (Some(span), Some(edge)) = (span_of(&desktop, area), area.kind.edge()) {
        let cut = node.clone();
        rows.push(telar::MenuEntry::row(
            telar::t!("editor.top.split_half"),
            keys::spell(&[Chord::char('s')]),
            move || {
                let drawn = Drawn::of(cut.output.as_deref(), cut.layer, &cut.area, edge);
                said(split_at(&cut, span.at + span.along / 2.0, &drawn))
            },
        ));
    }
    for forward in [false, true] {
        let Some(other) = beside(&desktop, node.layer, &node.area, forward) else {
            continue;
        };
        let joining = node.clone();
        rows.push(telar::MenuEntry::row(
            telar::t!("editor.top.join_with", other = other.to_string()),
            "",
            move || said(join(&joining, &other)),
        ));
    }
    if crate::mode::editing(node) {
        for edge in Edge::ALL {
            rows.push(telar::MenuEntry::row(
                telar::t!("editor.top.new_bar", edge = edge_name(edge)),
                "",
                move || said(create_on(edge)),
            ));
        }
    }
    for screen in other_screens(node.output.as_deref()) {
        let Some(output) = screen.output.clone() else {
            continue;
        };
        let moving = node.clone();
        rows.push(telar::MenuEntry::row(
            telar::t!("editor.top.to_output", output = output.clone()),
            "",
            move || said(send_to(&moving, &output)),
        ));
    }
    rows
}

/// Splits the bar `node` names at `at` along its edge, as one undo entry.
pub(crate) fn split_at(node: &Node, at: f32, drawn: &Drawn) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let (ops, _) = split(
        &session::draft().peek(),
        &desktop,
        node.layer,
        &node.area,
        at,
        drawn,
    )?;
    context::commit(
        telar::t!("editor.top.split", name = node.area.to_string()),
        ops,
    )
}

/// Joins the bar `node` names with `other`, as one undo entry.
pub(crate) fn join(node: &Node, other: &AreaId) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = joined(
        &session::draft().peek(),
        &desktop,
        node.layer,
        &node.area,
        other,
    )?;
    context::commit(
        telar::t!(
            "editor.top.joined",
            name = node.area.to_string(),
            other = other.to_string()
        ),
        ops,
    )
}

/// Sends the bar `node` names to the screen `output`, as one undo entry.
pub(crate) fn send_to(node: &Node, output: &str) -> Result<(), EditError> {
    let from = reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let onto = reconcile::desktop_now(Some(output)).ok_or_else(EditError::no_output)?;
    let ops = moved_to_output(
        &session::draft().peek(),
        &from,
        &onto,
        node.layer,
        &node.area,
    )?;
    context::commit(
        telar::t!(
            "editor.top.moved_to_output",
            name = node.area.to_string(),
            output = output
        ),
        ops,
    )
}
/// The values of a bar's popover that say where along its edge it lies, which its rows and handles share.
pub(crate) struct Along {
    /// The edge, as the layout file spells it.
    pub edge: RwSignal<String>,
    pub length: RwSignal<f32>,
    pub offset: RwSignal<f32>,
    pub fills: RwSignal<bool>,
    pub gap: RwSignal<f32>,
    pub hides: RwSignal<bool>,
}

/// Keeps the bar a popover edits clear of the other bars on its edge, whichever row or handle moves it: its offset and length stay inside the free stretch it is in, a bar asked to run the whole edge runs the whole of that stretch instead, and a turn to another edge fits it into the free stretch there nearest where it was. On an edge it has to itself nothing is held back.
pub(crate) fn keep_clear(draft: &AreaDraft, along: Along) {
    let Along {
        edge,
        length,
        offset,
        fills,
        gap,
        hides,
    } = along;
    let (node, within) = (draft.node.clone(), draft.resolved.within);
    let room = Cell::new(None::<(Edge, (f32, f32))>);
    let opened = Cell::new(false);
    telar::effect(move || {
        let Some(on) = crate::popover::parsed::<Edge>(&edge.get()) else {
            return;
        };
        let (now, from, whole, floats, hiding) = (
            length.get(),
            offset.get(),
            fills.get(),
            gap.get(),
            hides.get(),
        );
        let Some(desktop) = reconcile::planned()
            .iter()
            .find(|desktop| desktop.output == node.output)
            .cloned()
        else {
            return;
        };
        let run = surfaces::bar::run_along(
            on,
            desktop.reserved.box_of(within, desktop.size),
            desktop.reserved,
            floats,
            hiding,
        );
        let stretch = match room.get() {
            Some((held, stretch)) if held == on => stretch,
            _ => {
                let free = free_on(&desktop, on, run, Some((node.layer, &node.area)));
                let middle = run.0 + from + now / 2.0;
                let Some(stretch) = free
                    .iter()
                    .filter(|(start, end)| end - start >= SHORTEST)
                    .min_by(|a, b| distance(**a, middle).total_cmp(&distance(**b, middle)))
                    .copied()
                else {
                    return;
                };
                room.set(Some((on, stretch)));
                stretch
            }
        };
        // As it opens the popover only learns where the bar is; a bar already over another is left for the user to move.
        if !opened.replace(true) {
            return;
        }
        if (stretch.1 - stretch.0 - run.1).abs() <= TOUCH && (stretch.0 - run.0).abs() <= TOUCH {
            return;
        }
        let longest = (stretch.1 - stretch.0).round();
        let start = (stretch.0 - run.0).round().max(0.0);
        let (wanted, at) = match whole {
            true => (longest, start),
            false => {
                let wanted = now.clamp(SHORTEST.min(longest), longest);
                (
                    wanted,
                    from.clamp(start, (start + longest - wanted).max(start)),
                )
            }
        };
        telar::batch(|| {
            if whole {
                fills.set(false);
            }
            if length.peek() != wanted {
                length.set(wanted);
            }
            if offset.peek() != at {
                offset.set(at);
            }
        });
    });
}

/// A bar as the screen draws it, read back from the area once Reset has taken a key of it off.
struct BarNow {
    edge: Edge,
    thickness: f32,
    length: Extent,
    offset: f32,
    shape: layout::BarShape,
    autohide: Option<AutoHide>,
}

fn bar_now(area: &ResolvedArea) -> Option<BarNow> {
    match area.kind {
        ResolvedAreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        } => Some(BarNow {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        }),
        _ => None,
    }
}

/// A bar's popover rows: its edge, thickness, length and offset along the edge, kept clear of the bars beside it ([`keep_clear`]); its shape, gap, spacing and corners; and how it hides — with handles on the bar for its geometry. Each says where its value comes from, and has a Reset while the popover's level writes it.
fn bar_tool(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        length,
        offset,
        shape,
        autohide,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = draft.kind();
    let config = Arc::clone(&draft.config);
    let resolved_shape = surfaces::bar::bar_shape(&config, shape);
    let shape_now = {
        let config = Arc::clone(&config);
        move |area: &ResolvedArea| {
            bar_now(area).map_or(resolved_shape, |bar| {
                surfaces::bar::bar_shape(&config, bar.shape)
            })
        }
    };
    let mut list = chosen(
        draft,
        "edge",
        label!("editor.area.edge"),
        help("AreaKind::Bar", "edge"),
        edges(),
        move |area| bar_now(area).map_or(edge, |bar| bar.edge),
        move |area, edge: Edge| kind_field!(area, kind, Bar { edge }, edge),
    )?;
    let thickness = draft.setting(
        "thickness",
        "thickness",
        move |area| bar_now(area).map_or(thickness, |bar| bar.thickness),
        move |area, value: &f32| kind_field!(area, kind, Bar { thickness }, *value),
    );
    list.push(draft.marked(
        &["thickness"],
        rows::number(
            label!("editor.area.thickness"),
            help("AreaKind::Bar", "thickness"),
            thickness,
            handles::THICKNESS,
        )?,
    )?);

    let along = draft
        .rect()
        .map(|rect| match edge.is_vertical() {
            true => rect.height,
            false => rect.width,
        })
        .unwrap_or(0.0);
    let longest = match edge.is_vertical() {
        true => draft.screen.1,
        false => draft.screen.0,
    };
    let fills_edge = length == Extent::Fill;
    let length = draft.setting(
        "length",
        "length",
        move |area| match bar_now(area).map(|bar| bar.length) {
            Some(Extent::Px(px)) => px,
            _ => along,
        },
        move |area, value: &f32| kind_field!(area, kind, Bar { length }, Extent::Px(*value)),
    );
    let fills = draft.setting(
        "length_fill",
        "length",
        move |area| match bar_now(area) {
            Some(bar) => bar.length == Extent::Fill,
            None => fills_edge,
        },
        move |area, fill: &bool| {
            let extent = match fill {
                true => Extent::Fill,
                false => Extent::Px(length.peek()),
            };
            kind_field!(area, kind, Bar { length }, extent)
        },
    );
    let seeded = std::cell::Cell::new(false);
    let resetting = draft.clone();
    telar::effect(move || {
        length.with(|_| ());
        if seeded.replace(true) && !resetting.is_resetting() && fills.peek() {
            fills.set(false);
        }
    });
    list.push(draft.marked(
        &["length"],
        rows::together(vec![
            rows::toggle(
                label!("editor.area.fill_edge"),
                help("AreaKind::Bar", "length"),
                fills,
            )?,
            rows::number(
                label!("editor.area.length"),
                help("AreaKind::Bar", "length"),
                length,
                Range::whole(handles::SHORTEST, longest),
            )?,
        ])?,
    )?);
    let offset = draft.setting(
        "offset",
        "offset",
        move |area| bar_now(area).map_or(offset, |bar| bar.offset),
        move |area, value: &f32| kind_field!(area, kind, Bar { offset }, *value),
    );
    list.push(draft.marked(
        &["offset"],
        rows::number(
            label!("editor.area.offset"),
            help("AreaKind::Bar", "offset"),
            offset,
            Range::whole(0.0, longest),
        )?,
    )?);

    let reading = shape_now.clone();
    list.extend(chosen_as(
        draft,
        ("mode", "shape.mode"),
        label!("editor.area.mode"),
        help("BarShape", "mode"),
        variants("Shape"),
        move |area| reading(area).mode,
        move |area, mode| set_shape(area, kind, |shape| shape.mode = Some(mode)),
    )?);
    let hidden = autohide.unwrap_or_default();
    let hides = draft.setting(
        "autohide",
        "autohide",
        move |area| match bar_now(area) {
            Some(bar) => bar.autohide.is_some(),
            None => autohide.is_some(),
        },
        move |area, on: &bool| {
            let hide = on.then(|| autohide_of(area, kind).unwrap_or_default());
            if let Some(AreaKind::Bar { autohide, .. }) = AreaDraft::kind_mut(area, kind) {
                *autohide = hide;
            }
        },
    );
    let reading = shape_now.clone();
    let gap = draft.setting(
        "gap",
        "shape.gap",
        move |area| reading(area).gap as f32,
        move |area, value: &f32| set_shape(area, kind, |shape| shape.gap = Some(*value)),
    );
    list.push(draft.marked(
        &["shape.gap"],
        rows::number(
            label!("editor.area.gap"),
            help("BarShape", "gap"),
            gap,
            handles::GAP,
        )?,
    )?);
    if let Some(edge) = draft.shared::<String>("edge") {
        keep_clear(
            draft,
            Along {
                edge,
                length,
                offset,
                fills,
                gap,
                hides,
            },
        );
    }
    let reading = shape_now.clone();
    let spacing = draft.setting(
        "spacing",
        "shape.spacing",
        move |area| reading(area).spacing,
        move |area, value: &f32| set_shape(area, kind, |shape| shape.spacing = Some(*value)),
    );
    list.push(draft.marked(
        &["shape.spacing"],
        rows::number(
            label!("editor.area.spacing"),
            help("BarShape", "spacing"),
            spacing,
            Range::whole(0.0, 64.0),
        )?,
    )?);

    let rounded = look::rounded(&BarStyle(draft.clone()))?;
    list.push(rounded.row);

    let peek = draft.setting(
        "peek",
        "autohide",
        move |area| match bar_now(area).and_then(|bar| bar.autohide) {
            Some(hide) => hide.peek,
            None => hidden.peek,
        },
        move |area, value: &f32| set_autohide(area, kind, |hide| hide.peek = *value),
    );
    let on_hover = draft.setting(
        "on_hover",
        "autohide",
        move |area| match bar_now(area).and_then(|bar| bar.autohide) {
            Some(hide) => hide.on_hover,
            None => hidden.on_hover,
        },
        move |area, on: &bool| set_autohide(area, kind, |hide| hide.on_hover = *on),
    );
    list.push(draft.marked(
        &["autohide"],
        rows::together(vec![
            rows::toggle(
                label!("editor.area.autohide"),
                help("AreaKind::Bar", "autohide"),
                hides,
            )?,
            rows::number(
                label!("editor.area.peek"),
                help("AutoHide", "peek"),
                peek,
                Range::whole(0.0, 32.0),
            )?,
            rows::toggle(
                label!("editor.area.on_hover"),
                help("AutoHide", "on_hover"),
                on_hover,
            )?,
        ])?,
    )?);

    Ok(Inspector {
        rows: list,
        handles: handles::bar(
            draft,
            handles::BarValues {
                edge,
                thickness,
                length,
                offset,
                gap,
                corners: rounded.corners,
            },
        )?,
    })
}

fn set_shape(area: &mut Area, kind: &'static str, change: impl FnOnce(&mut layout::BarShape)) {
    if let Some(AreaKind::Bar { shape, .. }) = AreaDraft::kind_mut(area, kind) {
        change(shape);
    }
}

fn autohide_of(area: &mut Area, kind: &'static str) -> Option<AutoHide> {
    match AreaDraft::kind_mut(area, kind) {
        Some(AreaKind::Bar { autohide, .. }) => *autohide,
        _ => None,
    }
}

fn set_autohide(area: &mut Area, kind: &'static str, change: impl FnOnce(&mut AutoHide)) {
    if let Some(AreaKind::Bar { autohide, .. }) = AreaDraft::kind_mut(area, kind) {
        change(autohide.get_or_insert_with(AutoHide::default));
    }
}
