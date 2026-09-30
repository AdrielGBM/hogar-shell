//! What one keyboard step changes in the layout: the selection moved one slot, cell, edge or anchor over, made one size bigger or smaller, or taken away — planned against the draft as it stands, so each repeat of a held key goes one step further than the last.
//!
//! **Moves.** An instance on a bar or a dock moves along it one place at a time, from zone to zone at the end of one and on to the nearest bar of the same kind that way at the end of the last; across the bar it goes to that bar straight away. An instance or a group in a grid moves one cell. A bar or a dock moves to the edge the arrow points at, and a bar shorter than its edge slides along it instead. A stack steps its anchor, and an area placed by a rectangle moves by a hundredth of the screen.
//!
//! **Sizes.** A widget steps S, M, L as far as its module draws it; a grid group spans one cell more or less; a bar is thicker away from its edge and longer along it; a stack is wider; a rectangle grows or shrinks from its right and bottom edges.
//!
//! **Where it is written.** A change lands where [`Written`] says the layout decides the thing. A move reorders instances within the groups the edited layout writes itself, since a layout laid over another can add to an inherited group but never reorder it.

use layout::{
    Anchor, AreaKind, Extent, Group, GroupId, GroupKind, InstanceId, Layout, LayoutOp,
    Representation, ResolvedArea, ResolvedAreaKind, ResolvedGroup, Spot, Zone,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node, Part};

use crate::keys::{Direction, nearest};
use crate::popover::AreaDraft;
use crate::popover::handles::{SHORTEST, THICKNESS};
use crate::session::{EditError, Selection};
use crate::written::Written;

/// How far one step moves or resizes an area placed by a rectangle, as a fraction of the screen.
const RECT_STEP: f32 = 0.01;
/// How far one step slides a bar along its edge, in pixels.
const OFFSET_STEP: f32 = 8.0;
/// How much one step makes a bar or a dock thicker.
const THICKNESS_STEP: f32 = 2.0;
/// How much one step makes a bar longer.
const LENGTH_STEP: f32 = 16.0;
/// How much one step makes a stack wider, and the widths it stays between.
const WIDTH_STEP: f32 = 16.0;
const WIDTHS: (f32, f32) = (160.0, 960.0);

const SIZES: [Representation; 3] = [
    Representation::WidgetS,
    Representation::WidgetM,
    Representation::WidgetL,
];

/// What the selection is, on the screen as it shows now.
struct Target {
    desktop: Desktop,
    area: ResolvedArea,
    node: Node,
}

impl Target {
    fn of(selection: &Selection) -> Result<Self, EditError> {
        let node = selection
            .node()
            .cloned()
            .ok_or_else(|| refused(telar::t!("editor.popover.nothing")))?;
        let desktop = reconcile::desktops()
            .iter()
            .find(|desktop| desktop.output == node.output)
            .cloned()
            .ok_or_else(|| gone(&node))?;
        let area = desktop
            .resolved
            .layer(node.layer)
            .and_then(|layer| layer.areas.iter().find(|area| area.id == node.area))
            .cloned()
            .ok_or_else(|| gone(&node))?;
        Ok(Self {
            desktop,
            area,
            node,
        })
    }

    fn written(&self, draft: &Layout) -> Result<Written, EditError> {
        self.written_of(draft, &self.area)
    }

    fn written_of(&self, draft: &Layout, area: &ResolvedArea) -> Result<Written, EditError> {
        Written::area(
            draft,
            self.node.output.as_deref(),
            self.node.layer,
            &area.id,
        )
        .map_err(EditError::Refused)
    }

    fn group(&self, id: &GroupId) -> Result<&ResolvedGroup, EditError> {
        self.area
            .groups
            .iter()
            .find(|group| group.id == *id)
            .ok_or_else(|| gone(&self.node))
    }

    /// How long the screen is along `vertical` or across it.
    fn longest(&self, vertical: bool) -> f32 {
        match vertical {
            true => self.desktop.size.1,
            false => self.desktop.size.0,
        }
    }

    fn cannot(&self) -> EditError {
        refused(telar::t!(
            "editor.keys.no_way",
            name = self.node.area.to_string()
        ))
    }
}

fn refused(why: String) -> EditError {
    EditError::Refused(why)
}

fn gone(node: &Node) -> EditError {
    refused(telar::t!("editor.popover.gone", id = node.area.to_string()))
}

/// What the history calls the selection: an instance by its module's name, a group or an area by its id.
pub(crate) fn name_of(selection: &Selection) -> String {
    let Some(node) = selection.node() else {
        return String::new();
    };
    match &node.part {
        Part::Instance(_, id) => reconcile::desktops()
            .iter()
            .flat_map(|desktop| desktop.resolved.instances())
            .find(|instance| instance.id == *id)
            .map(|instance| {
                ui::descriptor::find(&instance.module)
                    .map_or_else(|| instance.module.clone(), |module| module.name.to_string())
            })
            .unwrap_or_else(|| id.to_string()),
        Part::Group(group) => group.to_string(),
        Part::Area => node.area.to_string(),
    }
}

/// The selection one step `direction` over.
pub(crate) fn moved(
    selection: &Selection,
    draft: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let target = Target::of(selection)?;
    match &target.node.part {
        Part::Instance(_, id) => move_instance(&target, draft, id, direction),
        Part::Group(group) => move_group(&target, draft, group, direction),
        Part::Area => move_area(&target, draft, direction).map(|op| vec![op]),
    }
}

/// The selection one size bigger (right, up) or smaller (left, down) — or, for a bar or a rectangle, grown or shrunk along the arrow.
pub(crate) fn resized(
    selection: &Selection,
    draft: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let target = Target::of(selection)?;
    let op = match &target.node.part {
        Part::Instance(_, id) => resize_instance(&target, draft, id, direction)?,
        Part::Group(group) => resize_group(&target, draft, group, direction)?,
        Part::Area => resize_area(&target, draft, direction)?,
    };
    Ok(vec![op])
}

/// What takes a group or an area off the screen: out of the rule that writes it, or a group named in its area's `remove` over the inherited one. An area only a layout this one extends writes cannot be, since nothing names an area to leave out of a layer.
pub(crate) fn removal(selection: &Selection, draft: &Layout) -> Result<Vec<LayoutOp>, EditError> {
    let target = Target::of(selection)?;
    let written = target.written(draft)?;
    match &target.node.part {
        Part::Group(id) => {
            let writes =
                written.is_present() && written.area.groups.iter().any(|group| group.id == *id);
            if writes {
                return Ok(vec![LayoutOp::DeleteGroup {
                    site: written.site.clone(),
                    area: target.area.id.clone(),
                    id: id.clone(),
                }]);
            }
            let mut area = written.area.clone();
            area.remove.push(id.clone());
            Ok(vec![written.op(&area)])
        }
        Part::Area if written.is_present() => Ok(vec![LayoutOp::DeleteArea {
            site: written.site.clone(),
            id: target.area.id.clone(),
        }]),
        Part::Area => Err(refused(telar::t!(
            "editor.keys.inherited_area",
            id = target.area.id.to_string()
        ))),
        Part::Instance(..) => Err(refused(telar::t!("editor.popover.nothing"))),
    }
}

/// Whether a bar or dock on `edge` runs along `direction`.
fn runs_along(edge: config::Edge, direction: Direction) -> bool {
    direction.is_horizontal() != edge.is_vertical()
}

fn zone_rank(group: &ResolvedGroup) -> u8 {
    match group.kind {
        GroupKind::Zone { zone: Zone::Start } | GroupKind::Cell { .. } => 0,
        GroupKind::Zone { zone: Zone::Center } => 1,
        GroupKind::Zone { zone: Zone::End } => 2,
    }
}

/// An area's groups in the order they are drawn along it: a bar's and a dock's zone by zone, anything else as written.
fn along(area: &ResolvedArea) -> Vec<&ResolvedGroup> {
    let mut groups: Vec<&ResolvedGroup> = area.groups.iter().collect();
    if area.kind.edge().is_some() {
        groups.sort_by_key(|group| zone_rank(group));
    }
    groups
}

fn move_instance(
    target: &Target,
    draft: &Layout,
    id: &InstanceId,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let holding = target
        .area
        .groups
        .iter()
        .find(|group| group.children.iter().any(|child| child.id == *id))
        .ok_or_else(|| gone(&target.node))?;
    let horizontal = match &target.area.kind {
        ResolvedAreaKind::Grid { .. } => {
            return move_group(target, draft, &holding.id, direction);
        }
        ResolvedAreaKind::Bar { edge, .. } | ResolvedAreaKind::Dock { edge, .. } => {
            !edge.is_vertical()
        }
        ResolvedAreaKind::Free { .. } => false,
        _ => return Err(target.cannot()),
    };
    let groups = along(&target.area);
    let at = groups
        .iter()
        .position(|group| group.id == holding.id)
        .ok_or_else(|| gone(&target.node))?;
    let place = holding
        .children
        .iter()
        .position(|child| child.id == *id)
        .ok_or_else(|| gone(&target.node))?;
    let forward = direction.is_forward();
    if direction.is_horizontal() == horizontal {
        let within = match forward {
            true => (place + 1 < holding.children.len()).then_some(place + 1),
            false => place.checked_sub(1),
        };
        if let Some(index) = within {
            return instance_move(target, draft, (holding, id), (&target.area, holding, index));
        }
        let next = match forward {
            true => groups.get(at + 1),
            false => at.checked_sub(1).and_then(|at| groups.get(at)),
        };
        if let Some(next) = next {
            let index = if forward { 0 } else { next.children.len() };
            return instance_move(target, draft, (holding, id), (&target.area, next, index));
        }
    }
    let beside = beside(target, direction)?;
    let groups = along(&beside);
    let into = match direction.is_horizontal() == horizontal {
        true if forward => groups.first().copied(),
        true => groups.last().copied(),
        false => groups
            .iter()
            .find(|group| group.id == holding.id)
            .or_else(|| {
                groups
                    .iter()
                    .find(|group| zone_rank(group) == zone_rank(holding))
            })
            .or_else(|| groups.first())
            .copied(),
    }
    .ok_or_else(|| target.cannot())?;
    let index = match direction.is_horizontal() == horizontal && forward {
        true => 0,
        false => into.children.len(),
    };
    instance_move(target, draft, (holding, id), (&beside, into, index))
}

/// The nearest area of the selection's kind `direction` of its own, on the same layer.
fn beside(target: &Target, direction: Direction) -> Result<ResolvedArea, EditError> {
    let placed = rects::on(target.node.output.as_deref(), target.node.layer);
    let own = Node::area(
        target.node.output.as_deref(),
        target.node.layer,
        &target.area.id,
    );
    let from = placed
        .iter()
        .find(|(node, _)| *node == own)
        .map(|(_, rect)| *rect)
        .ok_or_else(|| target.cannot())?;
    let areas = target
        .desktop
        .resolved
        .layer(target.node.layer)
        .map(|layer| layer.areas.clone())
        .unwrap_or_default();
    let kind = target.area.kind.name();
    let candidates = placed.iter().filter_map(|(node, rect)| {
        if node.part != Part::Area || node.area == target.area.id {
            return None;
        }
        areas
            .iter()
            .find(|area| area.id == node.area && area.kind.name() == kind)
            .map(|area| (area, *rect))
    });
    nearest(from, direction, candidates)
        .cloned()
        .ok_or_else(|| target.cannot())
}

/// Moves the instance `id` out of `from` into the group `to` of `into` at `index`, both as the edited layout writes them. A group the layout only inherits cannot be reordered by an entry laid over it, so a move there is refused rather than landing somewhere else.
fn instance_move(
    target: &Target,
    draft: &Layout,
    (from, id): (&ResolvedGroup, &InstanceId),
    (into, to, index): (&ResolvedArea, &ResolvedGroup, usize),
) -> Result<Vec<LayoutOp>, EditError> {
    let source = target.written(draft)?;
    writes_as_shown(&source, from)?;
    let landing = match into.id == target.area.id {
        true => source.clone(),
        false => target.written_of(draft, into)?,
    };
    writes_as_shown(&landing, to)?;
    Ok(vec![LayoutOp::MoveInstance {
        from: Spot {
            site: source.site.clone(),
            area: target.area.id.clone(),
            group: from.id.clone(),
        },
        to: Spot {
            site: landing.site.clone(),
            area: into.id.clone(),
            group: to.id.clone(),
        },
        id: id.clone(),
        index,
    }])
}

/// Refuses a group whose instances `written` does not write exactly as the screen shows them.
fn writes_as_shown(written: &Written, group: &ResolvedGroup) -> Result<(), EditError> {
    let shown: Vec<&InstanceId> = group.children.iter().map(|child| &child.id).collect();
    let writes = written.is_present()
        && written.area.groups.iter().any(|held| {
            held.id == group.id
                && held
                    .children
                    .iter()
                    .map(|child| &child.id)
                    .eq(shown.iter().copied())
        });
    match writes {
        true => Ok(()),
        false => Err(refused(telar::t!(
            "editor.keys.inherited",
            id = group.id.to_string()
        ))),
    }
}

/// The operation that writes a change to the group `id` where the area is written: in place where the rule writes the group, as a partial entry naming only what changed where it inherits it.
fn group_op(written: &Written, id: &GroupId, change: impl FnOnce(&mut Group)) -> LayoutOp {
    let mut area = written.area.clone();
    match area.groups.iter_mut().find(|group| group.id == *id) {
        Some(group) => change(group),
        None => {
            let mut group = Group {
                id: id.clone(),
                ..Group::default()
            };
            change(&mut group);
            area.groups.push(group);
        }
    }
    written.op(&area)
}

fn stepped(value: u32, by: i32) -> Option<u32> {
    value.checked_add_signed(by)
}

fn move_group(
    target: &Target,
    draft: &Layout,
    id: &GroupId,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let group = target.group(id)?;
    let (dx, dy) = delta(direction);
    let kind = match (&target.area.kind, group.kind) {
        (
            ResolvedAreaKind::Grid { .. },
            GroupKind::Cell {
                col,
                row,
                col_span,
                row_span,
            },
        ) => GroupKind::Cell {
            col: stepped(col, dx).ok_or_else(|| target.cannot())?,
            row: stepped(row, dy).ok_or_else(|| target.cannot())?,
            col_span,
            row_span,
        },
        (
            ResolvedAreaKind::Bar { edge, .. } | ResolvedAreaKind::Dock { edge, .. },
            GroupKind::Zone { zone },
        ) if runs_along(*edge, direction) => {
            let zones = [Zone::Start, Zone::Center, Zone::End];
            let at = zones.iter().position(|held| *held == zone).unwrap_or(0);
            let next = match direction.is_forward() {
                true => zones.get(at + 1),
                false => at.checked_sub(1).and_then(|at| zones.get(at)),
            };
            GroupKind::Zone {
                zone: *next.ok_or_else(|| target.cannot())?,
            }
        }
        _ => return Err(target.cannot()),
    };
    let written = target.written(draft)?;
    Ok(vec![group_op(&written, id, |group| {
        group.kind = Some(kind)
    })])
}

fn delta(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
    }
}

/// `area` as the layout file writes it, with its geometry changed by `change`.
fn with_kind(
    written: &Written,
    kind: &'static str,
    change: impl FnOnce(&mut AreaKind),
) -> LayoutOp {
    let mut area = written.area.clone();
    if let Some(geometry) = AreaDraft::kind_mut(&mut area, kind) {
        change(geometry);
    }
    written.op(&area)
}

/// The rectangle `rect` moved one step, kept on the screen.
fn nudged(rect: layout::Rect, direction: Direction) -> layout::Rect {
    let (dx, dy) = delta(direction);
    layout::Rect {
        x: (rect.x + dx as f32 * RECT_STEP).clamp(0.0, (1.0 - rect.w).max(0.0)),
        y: (rect.y + dy as f32 * RECT_STEP).clamp(0.0, (1.0 - rect.h).max(0.0)),
        ..rect
    }
}

/// The rectangle `rect` grown or shrunk one step from its right or bottom edge, kept on the screen.
fn stretched(rect: layout::Rect, direction: Direction) -> layout::Rect {
    let (dx, dy) = delta(direction);
    layout::Rect {
        w: (rect.w + dx as f32 * RECT_STEP).clamp(RECT_STEP, (1.0 - rect.x).max(RECT_STEP)),
        h: (rect.h + dy as f32 * RECT_STEP).clamp(RECT_STEP, (1.0 - rect.y).max(RECT_STEP)),
        ..rect
    }
}

fn set_rect(kind: &mut AreaKind, to: layout::Rect) {
    if let AreaKind::Grid { rect, .. }
    | AreaKind::WallpaperRegion { rect, .. }
    | AreaKind::Texture { rect, .. }
    | AreaKind::Free { rect }
    | AreaKind::Prompt { rect } = kind
    {
        *rect = Some(to);
    }
}

fn rect_of(kind: &ResolvedAreaKind) -> Option<layout::Rect> {
    match kind {
        ResolvedAreaKind::Grid { rect, .. }
        | ResolvedAreaKind::WallpaperRegion { rect, .. }
        | ResolvedAreaKind::Texture { rect, .. }
        | ResolvedAreaKind::Free { rect }
        | ResolvedAreaKind::Prompt { rect } => Some(*rect),
        _ => None,
    }
}

/// How long a bar is along its edge, in pixels.
fn length_px(length: Extent, longest: f32) -> f32 {
    match length {
        Extent::Fill => longest,
        Extent::Px(px) => px,
        Extent::Fraction(fraction) => fraction * longest,
    }
}

fn move_area(target: &Target, draft: &Layout, direction: Direction) -> Result<LayoutOp, EditError> {
    let written = target.written(draft)?;
    let kind = target.area.kind.name();
    let unchanged = |same: bool| match same {
        true => Err(target.cannot()),
        false => Ok(()),
    };
    match target.area.kind.clone() {
        ResolvedAreaKind::Bar {
            edge,
            length,
            offset,
            ..
        } if runs_along(edge, direction) && length != Extent::Fill => {
            let longest = target.longest(edge.is_vertical());
            let room = (longest - length_px(length, longest)).max(0.0);
            let sign = if direction.is_forward() { 1.0 } else { -1.0 };
            let to = (offset + sign * OFFSET_STEP).clamp(0.0, room);
            unchanged(to == offset)?;
            Ok(with_kind(&written, kind, |geometry| {
                if let AreaKind::Bar { offset, .. } = geometry {
                    *offset = Some(to);
                }
            }))
        }
        ResolvedAreaKind::Bar { edge, .. } | ResolvedAreaKind::Dock { edge, .. } => {
            let to = direction.edge();
            unchanged(to == edge)?;
            Ok(with_kind(&written, kind, |geometry| match geometry {
                AreaKind::Bar { edge, .. } | AreaKind::Dock { edge, .. } => *edge = Some(to),
                _ => {}
            }))
        }
        ResolvedAreaKind::Stack { anchor, .. } => {
            let at = Anchor::ALL
                .iter()
                .position(|held| *held == anchor)
                .unwrap_or(0);
            let (dx, dy) = delta(direction);
            let col = (at % 3) as i32 + dx;
            let row = (at / 3) as i32 + dy;
            if !(0..3).contains(&col) || !(0..3).contains(&row) {
                return Err(target.cannot());
            }
            let to = Anchor::ALL[(row * 3 + col) as usize];
            Ok(with_kind(&written, kind, |geometry| {
                if let AreaKind::Stack { anchor, .. } = geometry {
                    *anchor = Some(to);
                }
            }))
        }
        other => {
            let rect = rect_of(&other).ok_or_else(|| target.cannot())?;
            let to = nudged(rect, direction);
            unchanged(to == rect)?;
            Ok(with_kind(&written, kind, |geometry| set_rect(geometry, to)))
        }
    }
}

fn resize_area(
    target: &Target,
    draft: &Layout,
    direction: Direction,
) -> Result<LayoutOp, EditError> {
    let written = target.written(draft)?;
    let kind = target.area.kind.name();
    let unchanged = |same: bool| match same {
        true => Err(target.cannot()),
        false => Ok(()),
    };
    match target.area.kind.clone() {
        ResolvedAreaKind::Bar { edge, length, .. } if runs_along(edge, direction) => {
            let longest = target.longest(edge.is_vertical());
            let now = length_px(length, longest);
            let sign = if direction.is_forward() { 1.0 } else { -1.0 };
            let to = (now + sign * LENGTH_STEP).clamp(SHORTEST, longest);
            unchanged(to == now)?;
            Ok(with_kind(&written, kind, |geometry| {
                if let AreaKind::Bar { length, .. } = geometry {
                    *length = Some(Extent::Px(to));
                }
            }))
        }
        ResolvedAreaKind::Bar {
            edge, thickness, ..
        }
        | ResolvedAreaKind::Dock { edge, thickness } => {
            if runs_along(edge, direction) {
                return Err(target.cannot());
            }
            let sign = if direction.edge() == edge { -1.0 } else { 1.0 };
            let to = (thickness + sign * THICKNESS_STEP).clamp(THICKNESS.min, THICKNESS.max);
            unchanged(to == thickness)?;
            Ok(with_kind(&written, kind, |geometry| match geometry {
                AreaKind::Bar { thickness, .. } | AreaKind::Dock { thickness, .. } => {
                    *thickness = Some(to)
                }
                _ => {}
            }))
        }
        ResolvedAreaKind::Stack { width, .. } if direction.is_horizontal() => {
            let sign = if direction.is_forward() { 1.0 } else { -1.0 };
            let to = (width + sign * WIDTH_STEP).clamp(WIDTHS.0, WIDTHS.1);
            unchanged(to == width)?;
            Ok(with_kind(&written, kind, |geometry| {
                if let AreaKind::Stack { width, .. } = geometry {
                    *width = Some(to);
                }
            }))
        }
        other => {
            let rect = rect_of(&other).ok_or_else(|| target.cannot())?;
            let to = stretched(rect, direction);
            unchanged(to == rect)?;
            Ok(with_kind(&written, kind, |geometry| set_rect(geometry, to)))
        }
    }
}

fn resize_group(
    target: &Target,
    draft: &Layout,
    id: &GroupId,
    direction: Direction,
) -> Result<LayoutOp, EditError> {
    let group = target.group(id)?;
    let GroupKind::Cell {
        col,
        row,
        col_span,
        row_span,
    } = group.kind
    else {
        return Err(target.cannot());
    };
    let (dx, dy) = delta(direction);
    let spanned = |span: u32, by: i32| stepped(span, by).filter(|span| *span >= 1);
    let kind = GroupKind::Cell {
        col,
        row,
        col_span: spanned(col_span, dx).ok_or_else(|| target.cannot())?,
        row_span: spanned(row_span, dy).ok_or_else(|| target.cannot())?,
    };
    let written = target.written(draft)?;
    Ok(group_op(&written, id, |group| group.kind = Some(kind)))
}

/// A widget one size bigger (right, up) or smaller (left, down), skipping sizes its module does not draw.
fn resize_instance(
    target: &Target,
    draft: &Layout,
    id: &InstanceId,
    direction: Direction,
) -> Result<LayoutOp, EditError> {
    let (group, instance) = target
        .area
        .groups
        .iter()
        .find_map(|group| {
            group
                .children
                .iter()
                .find(|child| child.id == *id)
                .map(|child| (group, child))
        })
        .ok_or_else(|| gone(&target.node))?;
    let no_size = || {
        refused(telar::t!(
            "editor.keys.no_size",
            name = instance.module.clone()
        ))
    };
    let at = SIZES
        .iter()
        .position(|size| *size == instance.representation)
        .ok_or_else(no_size)?;
    let bigger = matches!(direction, Direction::Right | Direction::Up);
    let drawn = |size: Representation| {
        ui::descriptor::find(&instance.module)
            .is_some_and(|module| module.input(surfaces::area::representation(size)).is_some())
    };
    let next = match bigger {
        true => SIZES[at + 1..].iter().copied().find(|size| drawn(*size)),
        false => SIZES[..at].iter().rev().copied().find(|size| drawn(*size)),
    }
    .ok_or_else(no_size)?;
    let written = target.written(draft)?.instance(&group.id, id);
    let mut changed = written.instance.clone();
    changed.representation = Some(next);
    Ok(written.op(&changed))
}
