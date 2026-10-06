//! Containers in the edit modes: a group that arranges its children as a `row`, a `column`, a `grid` or `free`, made empty on a grid's first free span or in a bar's zone, and filled, rearranged and emptied through the pointer, the keys and the menus.
//!
//! **Made empty.** `Shift+N` on the desktop and the lock (and the palette's "Container" entry) puts one on the first span of [`SPANS`] still free on the grid, near the selection: a `row` where it is wider than tall, a `column` otherwise. In the top mode it is a group of its own in the selected bar's zone, drawn on a plate.
//!
//! **Children.** A widget let go anywhere over a container joins it where the pointer is — a slot of a row or a column, a cell of a grid, a box centred on the pointer in a free one. Dragged inside its own container a child takes another slot, another cell or another box, a free box snapping to its siblings and the container's middle and edges unless Alt is held; dragged out it is a widget of its own again, at the smallest size its module draws. Its handle — the end of a row's or a column's child, the corner of a grid's or a free one's — sets its weight, its span or its box, and the tag beside the pointer says what it now takes. Shift+arrows and Ctrl+arrows do the same a step at a time; Alt+↑ selects the container.
//!
//! **On a panel too.** A panel places its groups on cells as a grid does, so a container on one is made, filled, rearranged, resized and emptied the same way, by the pointer and by the keys.
//!
//! **Where it is written.** Every change is the child's own `weight`, `cell` or `rect`, or its place in its group's `children`, through the edit the layout writes for its area; an empty container stays until it is removed itself.

use telar::{MenuEntry, Rect};

use layout::{
    AreaId, Arrange, ChildCell, GroupId, GroupKind, Instance, InstanceId, LayerKind, Layout,
    LayoutOp, Placement, ResolvedArea, ResolvedAreaKind, ResolvedGroup, Style, Zone,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};

use crate::keys::{Chord, Direction, KeyOp, Run};
use crate::mode::said;
use crate::session::{self, EditError, Selection};
use crate::snap::{self, Guide, Motion, Moving};
use crate::written::Work;

use super::desktop;
use super::grid::{self, Cells, Room};
use super::widgets::Geometry;

/// The spans a new container on a grid tries, in order: the first with free cells for it is the one it takes.
pub const SPANS: [(u32, u32); 9] = [
    (6, 4),
    (4, 3),
    (6, 1),
    (4, 1),
    (1, 4),
    (2, 2),
    (2, 1),
    (1, 2),
    (1, 1),
];

/// How far one key step moves or grows a child of a `free` container, as a fraction of its box.
const RECT_STEP: f32 = 0.05;
/// The smallest a child of a `free` container is each way, as a fraction of its box.
const SMALLEST_RECT: f32 = 0.1;
const WEIGHT_STEP: f32 = 0.25;
/// The share of a row's or a column's length a child's handle stops at, either way.
const SHARE_BOUNDS: (f32, f32) = (0.08, 0.92);

/// Where a child sits in its container, or is put in one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    /// At this place in the order of a row's or a column's children.
    Index(usize),
    /// Its share of a row's or a column's length, against its siblings' weights.
    Weight(f32),
    /// On these cells of a grid container's inner grid.
    Cell(ChildCell),
    /// In this box, in fractions of a free container's box.
    Rect(layout::Rect),
}

pub(crate) fn create_key() -> KeyOp {
    KeyOp {
        name: "container-create",
        keys: vec![Chord::char('n').shift()],
        label: || telar::t!("editor.keys.op.container-create"),
        run: Run::Act(|_| create()),
    }
}

/// A group drawing a komponent is arranged by the komponent's file, never here.
pub(crate) fn arranges(group: &ResolvedGroup) -> bool {
    surfaces::container::arranges(group) && group.komponent.is_none()
}

fn rounded(value: f32, places: i32) -> f32 {
    let scale = 10f32.powi(places);
    (value * scale).round() / scale
}

/// A container as it was drawn when a gesture began: the group, the box its cells make on screen and the padded box its children share.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Frame {
    pub group: ResolvedGroup,
    pub outer: Rect,
    pub inner: Rect,
    pub gap: f32,
}

impl Frame {
    /// The container `group` of the grid `geometry` draws; `None` for a group that is no container.
    pub fn of(geometry: &Geometry, group: &ResolvedGroup) -> Option<Self> {
        if !arranges(group) {
            return None;
        }
        let outer = geometry.rect_of(grid::cells_of(group)?);
        let pad = surfaces::container::padding_of(group, false);
        let inner = Rect::new(
            outer.x + pad.left(),
            outer.y + pad.top(),
            (outer.width - pad.horizontal()).max(1.0),
            (outer.height - pad.vertical()).max(1.0),
        );
        Some(Self {
            group: group.clone(),
            outer,
            inner,
            gap: surfaces::container::gap_of(group, false),
        })
    }

    fn fraction(&self, point: (f32, f32)) -> (f32, f32) {
        (
            (point.0 - self.inner.x) / self.inner.width,
            (point.1 - self.inner.y) / self.inner.height,
        )
    }

    fn tracks(&self) -> (u32, u32) {
        (self.group.cols.max(1), self.group.rows.max(1))
    }

    fn child(&self, id: &InstanceId) -> Option<(usize, Option<Placement>)> {
        self.group
            .children
            .iter()
            .position(|child| child.id == *id)
            .map(|at| (at, self.group.children[at].placement))
    }

    fn others_weight(&self, id: &InstanceId) -> f32 {
        self.group
            .children
            .iter()
            .filter(|child| child.id != *id)
            .map(|child| surfaces::container::weight_of(child.placement))
            .sum()
    }

    /// Where a widget let go at `point` joins the container.
    pub fn adopting(&self, point: (f32, f32)) -> Slot {
        let (fx, fy) = self.fraction(point);
        let count = self.group.children.len();
        let index = |along: f32| {
            Slot::Index(((along * (count + 1) as f32).floor().max(0.0) as usize).min(count))
        };
        match self.group.arrange {
            Some(Arrange::Row) => index(fx),
            Some(Arrange::Column) => index(fy),
            Some(Arrange::Grid) => {
                let (cols, rows) = self.tracks();
                let on = |along: f32, tracks: u32| {
                    ((along * tracks as f32).floor().max(0.0) as u32).min(tracks - 1)
                };
                Slot::Cell(ChildCell::at(on(fx, cols), on(fy, rows)))
            }
            Some(Arrange::Free) => {
                let at = |along: f32| rounded((along - 0.25).clamp(0.0, 0.5), 3);
                Slot::Rect(layout::Rect {
                    x: at(fx),
                    y: at(fy),
                    w: 0.5,
                    h: 0.5,
                })
            }
            Some(Arrange::Pages) | None => Slot::Index(count),
        }
    }

    /// Where the child `id`, carried with its top left corner `grab` from the pointer at `point`, goes inside the container, and the lines a free box snapped to; `free` snaps nothing.
    pub fn moving(
        &self,
        id: &InstanceId,
        point: (f32, f32),
        grab: (f32, f32),
        free: bool,
    ) -> Option<(Slot, Vec<Guide>)> {
        let (at, placement) = self.child(id)?;
        let (fx, fy) = self.fraction(point);
        let count = self.group.children.len();
        let index = |along: f32| ((along * count as f32).floor().max(0.0) as usize).min(count - 1);
        let slot = match (self.group.arrange, placement) {
            (Some(Arrange::Row), _) => Slot::Index(index(fx)),
            (Some(Arrange::Column), _) => Slot::Index(index(fy)),
            (Some(Arrange::Grid), Some(Placement::Cell(cell))) => {
                let (cols, rows) = self.tracks();
                let on = |along: f32, tracks: u32, span: u32| {
                    ((along * tracks as f32).floor().max(0.0) as u32)
                        .min(tracks.saturating_sub(span))
                };
                Slot::Cell(ChildCell {
                    col: on(fx, cols, cell.col_span),
                    row: on(fy, rows, cell.row_span),
                    ..cell
                })
            }
            (Some(Arrange::Free), Some(Placement::Rect(rect))) => {
                let across = |at: f32, from: f32, length: f32, extent: f32| {
                    ((at - from) / length).clamp(0.0, (1.0 - extent).max(0.0))
                };
                let carried = layout::Rect {
                    x: across(point.0 - grab.0, self.inner.x, self.inner.width, rect.w),
                    y: across(point.1 - grab.1, self.inner.y, self.inner.height, rect.h),
                    ..rect
                };
                let siblings = snap::free_siblings(&self.group, &self.group.children[at].id);
                let snapped = snap::snap(
                    carried,
                    &siblings,
                    (self.inner.width, self.inner.height),
                    Moving::BODY,
                    free,
                );
                let kept =
                    |at: f32, extent: f32| rounded(at.clamp(0.0, (1.0 - extent).max(0.0)), 3);
                return Some((
                    Slot::Rect(layout::Rect {
                        x: kept(snapped.rect.x, rect.w),
                        y: kept(snapped.rect.y, rect.h),
                        ..rect
                    }),
                    snapped.guides,
                ));
            }
            _ => return None,
        };
        Some((slot, Vec::new()))
    }

    /// What the handle of the child `id`, drawn at `start` when it was taken hold of, sets with the pointer at `point`: its weight, its span or its box, and the lines a free box snapped to; `free` snaps nothing.
    pub fn resizing(
        &self,
        id: &InstanceId,
        start: Rect,
        point: (f32, f32),
        free: bool,
    ) -> Option<(Slot, Vec<Guide>)> {
        let (_, placement) = self.child(id)?;
        let count = self.group.children.len() as f32;
        let weighted = |length: f32, reach: f32| {
            let others = self.others_weight(id);
            if others <= 0.0 {
                return None;
            }
            let room = length - self.gap * (count - 1.0);
            let size = reach.clamp(room * SHARE_BOUNDS.0, room * SHARE_BOUNDS.1);
            let (low, high) = Instance::WEIGHTS;
            Some(Slot::Weight(rounded(
                (size * others / (room - size)).clamp(low, high),
                2,
            )))
        };
        let slot = match (self.group.arrange, placement) {
            (Some(Arrange::Row), _) => weighted(self.inner.width, point.0 - start.x)?,
            (Some(Arrange::Column), _) => weighted(self.inner.height, point.1 - start.y)?,
            (Some(Arrange::Grid), Some(Placement::Cell(cell))) => {
                let (cols, rows) = self.tracks();
                let span = |reach: f32, length: f32, tracks: u32, from: u32| {
                    let pitch = (length + self.gap) / tracks as f32;
                    (((reach + self.gap) / pitch).round().max(1.0) as u32).clamp(1, tracks - from)
                };
                Slot::Cell(ChildCell {
                    col_span: span(point.0 - start.x, self.inner.width, cols, cell.col),
                    row_span: span(point.1 - start.y, self.inner.height, rows, cell.row),
                    ..cell
                })
            }
            (Some(Arrange::Free), Some(Placement::Rect(rect))) => {
                let reach = |at: f32, from: f32, length: f32, origin: f32| {
                    ((at - from) / length).clamp(SMALLEST_RECT, (1.0 - origin).max(SMALLEST_RECT))
                };
                let pulled = layout::Rect {
                    w: reach(point.0, start.x, self.inner.width, rect.x),
                    h: reach(point.1, start.y, self.inner.height, rect.y),
                    ..rect
                };
                let siblings = snap::free_siblings(&self.group, id);
                let snapped = snap::snap(
                    pulled,
                    &siblings,
                    (self.inner.width, self.inner.height),
                    Moving {
                        x: Some(Motion::End),
                        y: Some(Motion::End),
                    },
                    free,
                );
                let kept = |extent: f32, origin: f32| {
                    rounded(
                        extent.clamp(SMALLEST_RECT, (1.0 - origin).max(SMALLEST_RECT)),
                        3,
                    )
                };
                return Some((
                    Slot::Rect(layout::Rect {
                        w: kept(snapped.rect.w, rect.x),
                        h: kept(snapped.rect.h, rect.y),
                        ..rect
                    }),
                    snapped.guides,
                ));
            }
            _ => return None,
        };
        Some((slot, Vec::new()))
    }

    /// What the tag beside a child's handle says it takes at `slot`: its share of the length, its span, or its box as a share of the container's.
    pub fn size_tag(&self, id: &InstanceId, slot: Slot) -> String {
        match slot {
            Slot::Weight(weight) => {
                let share = weight / (weight + self.others_weight(id)) * 100.0;
                telar::t!("editor.container.share", percent = share.round() as i64)
            }
            Slot::Cell(cell) => telar::t!(
                "editor.container.span",
                cols = cell.col_span,
                rows = cell.row_span
            ),
            Slot::Rect(rect) => telar::t!(
                "editor.container.box",
                width = (rect.w * 100.0).round() as i64,
                height = (rect.h * 100.0).round() as i64
            ),
            Slot::Index(_) => String::new(),
        }
    }
}

pub(crate) fn frame_in(
    grids: &[(Geometry, ResolvedArea)],
    area: &AreaId,
    group: &GroupId,
) -> Option<Frame> {
    let (geometry, held) = grids.iter().find(|(geometry, _)| geometry.area == *area)?;
    Frame::of(geometry, held.groups.iter().find(|held| held.id == *group)?)
}

pub(crate) fn placed_at(instance: &mut Instance, slot: Slot) {
    match slot {
        Slot::Index(_) => {}
        Slot::Weight(weight) => instance.weight = Some(weight),
        Slot::Cell(cell) => instance.cell = Some(cell),
        Slot::Rect(rect) => instance.rect = Some(rect),
    }
}

pub(crate) fn adopt(
    work: &mut Work,
    onto: &AreaId,
    target: &GroupId,
    mut instance: Instance,
    slot: Slot,
) -> Result<(), EditError> {
    placed_at(&mut instance, slot);
    work.rewrite(work.layer, onto, |written| {
        let into = match written.groups.iter().position(|group| group.id == *target) {
            Some(at) => &mut written.groups[at],
            None => {
                written.groups.push(layout::Group {
                    id: target.clone(),
                    ..layout::Group::default()
                });
                written.groups.last_mut().expect("a group was just pushed")
            }
        };
        let at = match slot {
            Slot::Index(at) => at.min(into.children.len()),
            _ => into.children.len(),
        };
        into.children.insert(at, instance);
    })
}

fn holder(work: &Work, node: &Node) -> Result<(ResolvedGroup, InstanceId), EditError> {
    let Part::Instance(group, id) = &node.part else {
        return Err(EditError::nothing());
    };
    let held = work
        .area(node.layer, &node.area)?
        .groups
        .into_iter()
        .find(|held| held.id == *group)
        .filter(arranges)
        .ok_or_else(EditError::nothing)?;
    Ok((held, id.template()))
}

/// The child `node` names put at `slot` of its own container: another place in its order, or another weight, cell or box.
pub(crate) fn rearranged(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    slot: Slot,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, node.layer);
    let (held, id) = holder(&work, node)?;
    match slot {
        Slot::Index(to) => {
            let from = held
                .children
                .iter()
                .position(|child| child.id == id)
                .ok_or_else(EditError::nothing)?;
            if from == to {
                return Err(EditError::nothing());
            }
            crate::steps::reordered(&mut work, node, &held, to)?;
        }
        other => {
            let written = work
                .written(work.layer, &node.area)?
                .instance(&held.id, &id);
            let mut changed = written.instance.clone();
            placed_at(&mut changed, other);
            if changed == written.instance {
                return Err(EditError::nothing());
            }
            work.apply(written.ops(&changed))?;
        }
    }
    Ok(work.done())
}

fn delta(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
    }
}

fn stepped_by(value: u32, by: i32, low: u32, high: u32) -> u32 {
    (value as i64 + i64::from(by)).clamp(i64::from(low), i64::from(high.max(low))) as u32
}

/// The child `node` names one step `direction` inside its container: one place on in its order, one cell over, or a twentieth of the box over.
pub(crate) fn stepped(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let work = Work::new(layout, desktop, node.layer);
    let (held, id) = holder(&work, node)?;
    let cannot = || EditError::no_way(&node.area);
    let (at, child) = held
        .children
        .iter()
        .enumerate()
        .find(|(_, child)| child.id == id)
        .ok_or_else(EditError::nothing)?;
    let (dx, dy) = delta(direction);
    let slot = match (held.arrange, child.placement) {
        (Some(Arrange::Row | Arrange::Column), _) => {
            let to = at as i64 + i64::from(dx + dy);
            if to < 0 || to >= held.children.len() as i64 {
                return Err(cannot());
            }
            Slot::Index(to as usize)
        }
        (Some(Arrange::Grid), Some(Placement::Cell(cell))) => {
            let to = ChildCell {
                col: stepped_by(cell.col, dx, 0, held.cols.saturating_sub(cell.col_span)),
                row: stepped_by(cell.row, dy, 0, held.rows.saturating_sub(cell.row_span)),
                ..cell
            };
            if to == cell {
                return Err(cannot());
            }
            Slot::Cell(to)
        }
        (Some(Arrange::Free), Some(Placement::Rect(rect))) => {
            let along = |at: f32, by: i32, extent: f32| {
                rounded(
                    (at + by as f32 * RECT_STEP).clamp(0.0, (1.0 - extent).max(0.0)),
                    3,
                )
            };
            let to = layout::Rect {
                x: along(rect.x, dx, rect.w),
                y: along(rect.y, dy, rect.h),
                ..rect
            };
            if to == rect {
                return Err(cannot());
            }
            Slot::Rect(to)
        }
        _ => return Err(cannot()),
    };
    rearranged(layout, desktop, node, slot)
}

/// The child `node` names one step bigger or smaller inside its container: a row's child wider to the right, a column's taller downwards, a grid's child one cell more or less each way, a free one a twentieth of the box.
pub(crate) fn grown(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let work = Work::new(layout, desktop, node.layer);
    let (held, id) = holder(&work, node)?;
    let cannot = || EditError::no_way(&node.area);
    let child = held
        .children
        .iter()
        .find(|child| child.id == id)
        .ok_or_else(EditError::nothing)?;
    let (dx, dy) = delta(direction);
    let slot = match (held.arrange, child.placement) {
        (Some(arrange @ (Arrange::Row | Arrange::Column)), placement) => {
            let step = match arrange {
                Arrange::Row => dx,
                _ => dy,
            };
            if step == 0 {
                return Err(EditError::refused(match arrange {
                    Arrange::Row => util::message!("editor.container.grows_across"),
                    _ => util::message!("editor.container.grows_down"),
                }));
            }
            let (low, high) = Instance::WEIGHTS;
            let now = surfaces::container::weight_of(placement);
            let to = rounded((now + step as f32 * WEIGHT_STEP).clamp(low, high), 2);
            if to == now {
                return Err(cannot());
            }
            Slot::Weight(to)
        }
        (Some(Arrange::Grid), Some(Placement::Cell(cell))) => {
            let to = ChildCell {
                col_span: stepped_by(cell.col_span, dx, 1, held.cols.saturating_sub(cell.col)),
                row_span: stepped_by(cell.row_span, dy, 1, held.rows.saturating_sub(cell.row)),
                ..cell
            };
            if to == cell {
                return Err(cannot());
            }
            Slot::Cell(to)
        }
        (Some(Arrange::Free), Some(Placement::Rect(rect))) => {
            let extent = |now: f32, by: i32, origin: f32| {
                rounded(
                    (now + by as f32 * RECT_STEP)
                        .clamp(SMALLEST_RECT, (1.0 - origin).max(SMALLEST_RECT)),
                    3,
                )
            };
            let to = layout::Rect {
                w: extent(rect.w, dx, rect.x),
                h: extent(rect.h, dy, rect.y),
                ..rect
            };
            if to == rect {
                return Err(cannot());
            }
            Slot::Rect(to)
        }
        _ => return Err(cannot()),
    };
    rearranged(layout, desktop, node, slot)
}

/// The child `node` names taken out of its container to a widget of its own, at the smallest size its module draws, on the free cells nearest the container.
pub(crate) fn taken_out(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, node.layer);
    let (held, _) = holder(&work, node)?;
    let near = grid::cells_of(&held).map_or((0, 0), |cells| (cells.col, cells.row));
    let mut moved = desktop::take_out(&mut work, node)?;
    moved.representation = moved
        .module
        .as_deref()
        .zip(moved.representation)
        .map(|(module, now)| desktop::loose_size(module, node.layer, now));
    desktop::put_on(&mut work, &node.area, moved, None, near)?;
    Ok(work.done())
}

/// The span a new container on the grid `area` takes and where: the first of [`SPANS`] with free cells for it inside `room`, on those nearest `near`.
pub(crate) fn free_span(area: &ResolvedArea, room: Room, near: (u32, u32)) -> Option<Cells> {
    let taken = grid::taken(area);
    SPANS.iter().find_map(|(cols, rows)| {
        let size = Cells {
            col: 0,
            row: 0,
            cols: *cols,
            rows: *rows,
        };
        grid::free_inside(&taken, size, near, room)
    })
}

/// The cells a container put on the grid `area` from the palette covers: the span it would take there, else one cell.
pub(crate) fn footprint_on(area: &ResolvedArea, room: Room) -> Cells {
    free_span(area, room, (0, 0)).map_or(Cells::ONE, |cells| cells.at(0, 0))
}

fn arrangement_of(cells: Cells) -> Arrange {
    match cells.cols > cells.rows {
        true => Arrange::Row,
        false => Arrange::Column,
    }
}

/// The operations that make a new, empty container on the grid `area` of `layer`, and its id: on the first free span of [`SPANS`] nearest `near`, or, where `at` is given, with that span's first cell there and what it covers moved out of its way.
pub(crate) fn on_grid(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    area: &AreaId,
    (at, near): (Option<(u32, u32)>, (u32, u32)),
) -> Result<(Vec<LayoutOp>, GroupId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let grid = work.area(layer, area)?;
    if !grid.kind.places_on_cells() {
        return Err(EditError::refused(util::message!(
            "editor.desktop.not_a_grid",
            id = area.to_string()
        )));
    }
    let room = desktop::room_of(desktop, &grid);
    let span = match at {
        Some(_) => free_span(&grid, room, near).unwrap_or(Cells::ONE),
        None => free_span(&grid, room, near)
            .ok_or_else(|| EditError::refused(util::message!("editor.container.no_room")))?,
    };
    let spot = at.unwrap_or((span.col, span.row));
    let id = layout::ops::free_group_id(&work.layout, &work.known, layer, area, "container");
    let kind = GroupKind::Cell {
        col: spot.0,
        row: spot.1,
        col_span: span.cols,
        row_span: span.rows,
    };
    desktop::put_group(
        &mut work,
        area,
        (id.clone(), span),
        (Some(spot), near),
        |_| layout::Group {
            kind: Some(kind),
            arrange: Some(arrangement_of(span)),
            ..layout::Group::default()
        },
    )?;
    Ok((work.done(), id))
}

/// The operations that make a new, empty group on a plate in the zone `zone` of the bar `bar` of `layer`, and its id.
pub(crate) fn in_bar(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    bar: &AreaId,
    zone: Zone,
) -> Result<(Vec<LayoutOp>, GroupId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let id = layout::ops::free_group_id(&work.layout, &work.known, layer, bar, "plate");
    let group = layout::Group {
        id: id.clone(),
        kind: Some(GroupKind::Zone { zone }),
        style: Style {
            fill: Some(ui::scale::plate::FILL.to_string()),
            ..Style::default()
        },
        ..layout::Group::default()
    };
    work.rewrite(layer, bar, |written| written.groups.push(group))?;
    Ok((work.done(), id))
}

/// The bar of `layer` a new container goes in, and its zone: the selected bar and the zone of what is selected in it, else the layer's first bar from its start.
pub(crate) fn bar_near(desktop: &Desktop, layer: LayerKind) -> Option<(AreaId, Zone)> {
    let bars: Vec<&ResolvedArea> = desktop
        .resolved
        .layer(layer)?
        .areas
        .iter()
        .filter(|area| matches!(area.kind, ResolvedAreaKind::Bar { .. }))
        .collect();
    let selected = session::selected();
    if let Some(node) = selected.node()
        && let Some(bar) = bars.iter().find(|bar| bar.id == node.area)
    {
        let zone = match &node.part {
            Part::Group(group) | Part::Instance(group, _) => bar
                .groups
                .iter()
                .find(|held| held.id == *group)
                .and_then(|held| match held.kind {
                    GroupKind::Zone { zone } => Some(zone),
                    GroupKind::Cell { .. } => None,
                }),
            Part::Area => None,
        };
        return Some((bar.id.clone(), zone.unwrap_or(Zone::Start)));
    }
    bars.first().map(|bar| (bar.id.clone(), Zone::Start))
}

/// Makes a new, empty container on the layer being edited, as one undo entry, and selects it: on the first free span of the grid or panel the selection is on, near it, else of the layer's first grid — or, in the top mode with no panel selected, a group on a plate in a bar's zone ([`super::top::plate`]).
pub(crate) fn create() -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let on_cells = match mode.layer {
        LayerKind::Top => desktop::selected_cells(&desktop, mode.layer),
        layer => desktop::target_near(&desktop, layer),
    };
    let (ops, area, id) = match (mode.layer, on_cells) {
        (layer, Some((area, near))) => {
            let (ops, id) = on_grid(&layout, &desktop, layer, &area, (None, near))?;
            (ops, area, id)
        }
        (LayerKind::Top, None) => return super::top::plate(),
        (_, None) => {
            return Err(EditError::refused(util::message!(
                "editor.container.no_grid"
            )));
        }
    };
    crate::context::commit(
        telar::t!("editor.container.made", name = id.to_string()),
        ops,
    )?;
    session::select(Selection::Group(
        Node::area(Some(&mode.output), mode.layer, &area).group(&id),
    ));
    Ok(())
}

/// The rows a child of a container adds to its menu: customizing the container, and, on a grid or a panel, taking it out to a widget of its own.
pub(crate) fn menu_rows(
    area: &ResolvedArea,
    node: &Node,
    holder: &ResolvedGroup,
) -> Vec<MenuEntry> {
    if !arranges(holder) {
        return Vec::new();
    }
    let container = Node::area(node.output.as_deref(), node.layer, &node.area).group(&holder.id);
    let mut rows = vec![MenuEntry::row(
        telar::t!("editor.container.customize"),
        "",
        move || {
            session::select(Selection::Group(container.clone()));
            said(crate::popover::open_group(container.clone()));
        },
    )];
    if area.kind.places_on_cells() {
        let taken = node.clone();
        rows.push(MenuEntry::row(
            telar::t!("editor.container.take_out"),
            "",
            move || said(take_out(&taken)),
        ));
    }
    rows
}

pub(crate) fn take_out(node: &Node) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = taken_out(&session::draft().peek(), &desktop, node)?;
    crate::context::commit(
        telar::t!(
            "editor.container.taken_out",
            name = crate::steps::name_of(&Selection::Instance(node.clone()))
        ),
        ops,
    )
}
