//! The desktop mode's tools (TA-5): grids of widgets placed by explicit cells, a palette to add them from, widgets dragged between cells and onto one another, and sizes stepped S, M, L.
//!
//! **Tolerant reflow (F-7).** A widget dropped, added or grown onto cells another widget covers never deletes it: the one displaced moves to the free cells nearest where it was, and nothing else moves ([`super::grid`]). The widgets that move slide there (F-5.8), since a change of cells alone keeps the grid's nodes ([`surfaces::area::move_cells`]).
//!
//! **Smart Stack by dropping onto.** A widget let go over the middle of another joins it in one stack at that widget's cells, a stack there already taking one more; let go nearer an edge it is placed beside instead, on the cells under it. Dragged out of a stack, it leaves as a widget of its own (F-7's drag out to detach), and a stack left with one widget is a widget again.
//!
//! **Continuity (TA-3).** "From bars…" in the palette, and a chip's menu, move the chip's instance onto a grid as a widget — the same instance, id, options and state, only drawn another way.
//!
//! **Containers.** A widget let go anywhere over a container joins it where the pointer is, and one dragged out of a container is a widget of its own again ([`super::container`]).
//!
//! **Every drag has a key (WCAG 2.5.7).** `a` opens the palette, where Enter places the entry it points at on the first free cells near the selection; `Alt+N` makes a grid and `Shift+N` a container; Shift+arrows move a widget one cell (out of its stack, if it is in one) and Ctrl+arrows step its size, both reflowing what they cover; Ctrl+Shift+arrows stack it onto the widget that way; `w` switches editing the workspace that is up alone on and off ([`crate::variant`]).
//!
//! **One undo entry each.** A drop, an add, a size step, a stack and a new grid are each one edit, whatever they moved out of the way.

use telar::{LayoutError, Rect};

use layout::{
    Anchor, Area, AreaId, AreaKind, Arrange, Group, GroupId, GroupKind, Instance, InstanceId,
    LayerKind, Layout, LayoutOp, Representation, ResolvedArea, ResolvedAreaKind, ResolvedInstance,
    Unset, Within,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};
use ui::descriptor::{Input, ModuleDescriptor};

use crate::context;
use crate::keys::{self, Chord, Direction, KeyOp, Run};
use crate::mode::said;
use crate::popover::area::{chosen, rect_rows, variants};
use crate::popover::rows::{self, Range, Rows, label};
use crate::popover::{AreaDraft, Inspector, InstanceDraft, help, kind_field, kind_read};
use crate::session::{self, EditError, Selection};
use crate::written::Work;

use super::container::{self, Slot};
use super::grid::{self, Cells, Room};
use super::palette::{self, Pick};
use super::widgets;

/// The widget sizes a grid steps through, smallest first.
pub const SIZES: [Representation; 3] = [
    Representation::WidgetS,
    Representation::WidgetM,
    Representation::WidgetL,
];

/// The toolbar buttons of every mode that has the grid tools: a new grid, and the palette.
const GRID_BUTTONS: [crate::host::StripButton; 2] = [
    (
        || telar::t!("editor.desktop.new_grid"),
        || said(create_grid()),
    ),
    (
        || telar::t!("editor.desktop.add_widget"),
        || said(palette::open()),
    ),
];

pub(crate) fn install() {
    palette::install();
    crate::popover::add_area_tool("grid", grid_tool);
    crate::popover::add_instance_tool(instance_tool);
    crate::popover::settle_instances_in("grid", settle);
    crate::host::add_tool(LayerKind::Desktop, widgets::tool);
    add_grid_tools(LayerKind::Desktop);
    context::add_area_rows("grid", grid_rows);
    keys::add_key_op(
        "grid",
        KeyOp {
            name: "widget-stack",
            keys: keys::arrows_with(|chord| chord.ctrl().shift()),
            label: || telar::t!("editor.keys.op.widget-stack"),
            run: Run::Toward(stack_toward),
        },
    );
}

/// The key that makes a new grid.
pub(crate) fn grid_key() -> Chord {
    Chord::char('n').alt()
}

/// Gives the mode of `layer` the grid tools' toolbar buttons and keys: a new grid, a new container, and the palette.
pub(crate) fn add_grid_tools(layer: LayerKind) {
    for button in GRID_BUTTONS {
        crate::host::add_adding_button(layer, button);
    }
    crate::host::set_add(layer, palette::open);
    keys::add_mode_key_op(layer, palette::add_key());
    keys::add_mode_key_op(
        layer,
        KeyOp {
            name: "grid-create",
            keys: vec![grid_key()],
            label: || telar::t!("editor.keys.op.grid-create"),
            run: Run::Act(|_| create_grid()),
        },
    );
    keys::add_mode_key_op(layer, container::create_key());
}

/// Where a dragged widget is let go on a grid.
#[derive(Clone, Debug, PartialEq)]
pub enum Landing {
    /// Placed with its first cell here, beside whatever is around it.
    Cell { col: u32, row: u32 },
    /// Onto the group called this, joining it in one stack, or at the end of the container it is.
    Onto(GroupId),
    /// Into the container called this, at the slot the pointer is over.
    Into(GroupId, Slot),
}

/// The grid or panel `id` of the edit's layer, as the layout planned so far resolves it: both place their groups on cells.
fn grid_of(work: &Work, id: &AreaId) -> Result<ResolvedArea, EditError> {
    let area = work.area(work.layer, id)?;
    match area.kind.places_on_cells() {
        true => Ok(area),
        false => Err(EditError::refused(telar::t!(
            "editor.desktop.not_a_grid",
            id = id.to_string()
        ))),
    }
}

/// The group `moved` of the grid `area` put on `to`, whatever it now covers moved to the nearest free cells, and each group that moved written at its new cells.
fn reflow(work: &mut Work, area: &AreaId, moved: &GroupId, to: Cells) -> Result<(), EditError> {
    let now = grid_of(work, area)?;
    let placed = grid::reflow(&grid::placed(&now), moved, to, room_of(work.desktop, &now));
    let changed: Vec<(GroupId, GroupKind)> = placed
        .iter()
        .filter_map(|(id, cells)| {
            let group = now.groups.iter().find(|group| group.id == *id)?;
            let GroupKind::Cell {
                col,
                row,
                col_span,
                row_span,
            } = group.kind
            else {
                return None;
            };
            ((col, row) != (cells.col, cells.row)).then(|| {
                (
                    id.clone(),
                    GroupKind::Cell {
                        col: cells.col,
                        row: cells.row,
                        col_span,
                        row_span,
                    },
                )
            })
        })
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    work.rewrite(work.layer, area, |written| {
        for (id, kind) in changed {
            group_mut(written, &id).kind = Some(kind);
        }
    })
}

/// The cells the group `id` of the grid `area` covers now.
fn cells(work: &Work, area: &AreaId, id: &GroupId) -> Result<Cells, EditError> {
    grid_of(work, area)?
        .groups
        .iter()
        .find(|group| group.id == *id)
        .and_then(grid::cells_of)
        .ok_or_else(EditError::nothing)
}

/// The group `group` of the grid `area` kept where it is, and what it covers now moved out of its way.
fn settle_group(work: &mut Work, area: &AreaId, group: &GroupId) -> Result<(), EditError> {
    let now = cells(work, area, group)?;
    reflow(work, area, group, now)
}

/// Takes the instance `node` names out of its group, as its menu's Remove does, keeping what it is so it can be put somewhere else.
pub(crate) fn take_out(work: &mut Work, node: &Node) -> Result<Instance, EditError> {
    let Part::Instance(group, id) = &node.part else {
        return Err(EditError::nothing());
    };
    let resolved = work
        .area(node.layer, &node.area)?
        .groups
        .into_iter()
        .find(|held| held.id == *group)
        .and_then(|held| {
            held.children
                .into_iter()
                .find(|child| child.id == id.template())
        })
        .ok_or_else(EditError::nothing)?;
    let ops = context::removal(&work.layout, work.desktop, node)?;
    work.apply(ops)?;
    Ok(placed_as(&resolved))
}

/// A new group of the grid `area` holding `instance` alone: its first cell at `at` where that is given, what it covers moved out of its way, else on the free cells nearest `near`.
pub(crate) fn put_on(
    work: &mut Work,
    area: &AreaId,
    instance: Instance,
    at: Option<(u32, u32)>,
    near: (u32, u32),
) -> Result<(), EditError> {
    let id = layout::ops::free_group_id(
        &work.layout,
        &work.known,
        work.layer,
        area,
        instance.id.as_str(),
    );
    let size = instance.representation.map_or(Cells::ONE, footprint);
    put_group(work, area, (id, size), (at, near), |kind| Group {
        kind: Some(kind),
        children: vec![instance],
        ..Group::default()
    })
}

/// A new group `id` of the grid `area`, covering `size` cells and holding what `fill` makes of the cell it is placed at: its first cell at `at` where that is given, what it covers moved out of its way, else on the free cells nearest `near`.
pub(crate) fn put_group(
    work: &mut Work,
    area: &AreaId,
    (id, size): (GroupId, Cells),
    (at, near): (Option<(u32, u32)>, (u32, u32)),
    fill: impl FnOnce(GroupKind) -> Group,
) -> Result<(), EditError> {
    let at = match at {
        Some(at) => at,
        None => {
            let grid = grid_of(work, area)?;
            let free = grid::nearest_free(
                &grid::taken(&grid),
                size,
                near,
                room_of(work.desktop, &grid),
            );
            (free.col, free.row)
        }
    };
    let kind = GroupKind::Cell {
        col: at.0,
        row: at.1,
        col_span: 1,
        row_span: 1,
    };
    let group = Group {
        id: id.clone(),
        ..fill(kind)
    };
    work.rewrite(work.layer, area, |written| written.groups.push(group))?;
    settle_group(work, area, &id)
}

/// The group `id` of an area as its entry writes it, made a partial entry naming only its id where the entry has none.
fn group_mut<'a>(area: &'a mut Area, id: &GroupId) -> &'a mut Group {
    match area.groups.iter().position(|group| group.id == *id) {
        Some(at) => &mut area.groups[at],
        None => {
            area.groups.push(Group {
                id: id.clone(),
                ..Group::default()
            });
            area.groups.last_mut().expect("a group was just pushed")
        }
    }
}

/// An instance as a layout writes it, from what the screen shows of it: everything but where it is.
pub(crate) fn placed_as(resolved: &ResolvedInstance) -> Instance {
    Instance {
        id: resolved.id.clone(),
        module: Some(resolved.module.clone()),
        representation: Some(resolved.representation),
        options: resolved.options.clone(),
        bindings: resolved
            .bindings
            .iter()
            .map(|(path, bound)| (path.clone(), bound.expr.clone()))
            .collect(),
        style: resolved.style.clone(),
        actions: resolved.actions.clone(),
        unset: Vec::new(),
        ..Instance::default()
    }
}

/// How many cells the grid `area` has room for inside its rectangle on `desktop`'s screen, its padding taken off: the lattice it draws ([`surfaces::area::room`]); a panel's are its columns and rows.
pub(crate) fn room_of(desktop: &Desktop, area: &ResolvedArea) -> Room {
    let rect = match area.kind {
        ResolvedAreaKind::Grid { rect, .. } => rect,
        ResolvedAreaKind::Panel { cols, rows, .. } => {
            return Room {
                cols: cols.max(1),
                rows: rows.max(1),
            };
        }
        _ => return Room { cols: 1, rows: 1 },
    };
    let region = region_of(desktop, area.within, rect);
    surfaces::area::room(area, region).map_or(Room { cols: 1, rows: 1 }, |room| Room {
        cols: u32::from(room.columns),
        rows: u32::from(room.rows),
    })
}

/// Where a rectangle measured in `within` lands on `desktop`'s screen, in pixels.
pub(crate) fn region_of(desktop: &Desktop, within: Within, rect: layout::Rect) -> Rect {
    surfaces::area::within(rect, desktop.reserved.box_of(within, desktop.size))
}

/// The widget `instance` names moved to `landing` on the grid `onto` of its layer: to other cells, out of a stack, onto another grid, or onto another widget to stack with it — whatever it lands on moved out of its way, never taken off the screen.
pub(crate) fn dropped(
    layout: &Layout,
    desktop: &Desktop,
    instance: &Node,
    onto: &AreaId,
    landing: &Landing,
) -> Result<Vec<LayoutOp>, EditError> {
    let Part::Instance(group, _) = &instance.part else {
        return Err(EditError::nothing());
    };
    let mut work = Work::new(layout, desktop, instance.layer);
    let from = grid_of(&work, &instance.area)?;
    let holding = from
        .groups
        .iter()
        .find(|held| held.id == *group)
        .ok_or_else(EditError::nothing)?;
    grid_of(&work, onto)?;
    match landing {
        Landing::Onto(target) if instance.area == *onto && target == group => {
            return Err(EditError::nothing());
        }
        Landing::Into(target, _) if instance.area == *onto && target == group => {
            return Err(EditError::nothing());
        }
        Landing::Onto(target) => {
            let moved = take_out(&mut work, instance)?;
            stack_onto(&mut work, onto, target, moved)?;
        }
        Landing::Into(target, slot) => {
            let moved = take_out(&mut work, instance)?;
            container::adopt(&mut work, onto, target, moved, *slot)?;
        }
        Landing::Cell { col, row }
            if instance.area == *onto
                && holding.children.len() == 1
                && !container::arranges(holding) =>
        {
            let cells = grid::cells_of(holding).ok_or_else(EditError::nothing)?;
            reflow(&mut work, onto, group, cells.at(*col, *row))?;
        }
        Landing::Cell { col, row } => {
            let mut moved = take_out(&mut work, instance)?;
            if container::arranges(holding)
                && let (Some(module), Some(now)) = (moved.module.as_deref(), moved.representation)
            {
                moved.representation = Some(loose_size(module, instance.layer, now));
            }
            put_on(&mut work, onto, moved, Some((*col, *row)), (0, 0))?;
        }
    }
    Ok(work.done())
}

pub(crate) fn stack_onto(
    work: &mut Work,
    onto: &AreaId,
    target: &GroupId,
    instance: Instance,
) -> Result<(), EditError> {
    let arranged = grid_of(work, onto)?
        .groups
        .iter()
        .any(|held| held.id == *target && held.arrange.is_some());
    work.rewrite(work.layer, onto, |written| {
        let into = group_mut(written, target);
        into.children.push(instance);
        if !arranged {
            into.arrange = Some(Arrange::Pages);
        }
    })?;
    settle_group(work, onto, target)
}

/// The size a widget taken out of a container is drawn at on its own: the smallest its module draws on `layer`, else `now`.
pub(crate) fn loose_size(module: &str, layer: LayerKind, now: Representation) -> Representation {
    sizes_of(module, layer).first().copied().unwrap_or(now)
}

/// Stacks the selected widget onto the one nearest it that way on its grid.
fn stack_toward(
    selection: &Selection,
    layout: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let Selection::Instance(node) = selection else {
        return Err(EditError::nothing());
    };
    let Part::Instance(group, _) = &node.part else {
        return Err(EditError::nothing());
    };
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let area = grid_of(&Work::new(layout, &desktop, node.layer), &node.area)?;
    let geometry = widgets::Geometry::of(&desktop, &area).ok_or_else(EditError::nothing)?;
    let from = area
        .groups
        .iter()
        .find(|held| held.id == *group)
        .and_then(grid::cells_of)
        .ok_or_else(EditError::nothing)?;
    let candidates = area
        .groups
        .iter()
        .filter(|held| held.id != *group)
        .filter_map(|held| Some((held.id.clone(), geometry.rect_of(grid::cells_of(held)?))));
    let target = keys::nearest(geometry.rect_of(from), direction, candidates)
        .ok_or_else(|| EditError::no_way(&node.area))?;
    dropped(layout, &desktop, node, &node.area, &Landing::Onto(target))
}

/// A new widget, as the palette adds one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adding<'a> {
    pub module: &'a str,
    pub representation: Representation,
    /// The first cell it covers, where the pointer put it; `None` for the free cells nearest `near`.
    pub at: Option<(u32, u32)>,
    pub near: (u32, u32),
}

/// A new instance of `adding.module` on the grid `area` of `layer`: at `adding.at` where that is given, what it covers moved out of its way, else on the free cells nearest `adding.near`. The lock screen takes readings only (TA-8), so a representation that answers the pointer is refused there with the reason.
pub fn added(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    area: &AreaId,
    adding: &Adding,
) -> Result<(Vec<LayoutOp>, InstanceId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let instance = fresh(&work, adding.module, adding.representation)?;
    let id = instance.id.clone();
    put_on(&mut work, area, instance, adding.at, adding.near)?;
    Ok((work.done(), id))
}

/// Refused, with the reason, where the module does not draw that way or the lock screen would take a control.
pub(crate) fn fresh(
    work: &Work,
    module: &str,
    representation: Representation,
) -> Result<Instance, EditError> {
    let descriptor = ui::descriptor::find(module).ok_or_else(|| {
        EditError::refused(telar::t!("editor.desktop.unknown_module", module = module))
    })?;
    let input = descriptor
        .input(surfaces::area::representation(representation))
        .ok_or_else(|| {
            EditError::refused(telar::t!(
                "editor.desktop.cannot_draw",
                name = descriptor.name,
                size = representation.as_str()
            ))
        })?;
    if !placeable(input, work.layer) {
        return Err(EditError::refused(telar::t!(
            "editor.desktop.lock_readings",
            name = descriptor.name
        )));
    }
    Ok(Instance {
        id: layout::ops::free_instance_id(&work.layout, &work.known, module),
        module: Some(module.to_string()),
        representation: Some(representation),
        ..Instance::default()
    })
}

/// The cells a widget drawn as `representation` covers on its own.
pub(crate) fn footprint(representation: Representation) -> Cells {
    match surfaces::area::representation(representation) {
        ui::host::Representation::Widget(size) => {
            let span = size.footprint();
            Cells {
                cols: u32::from(span.columns),
                rows: u32::from(span.rows),
                ..Cells::ONE
            }
        }
        _ => Cells::ONE,
    }
}

/// The widget `instance` names drawn as `representation`, what it grew over moved out of its way.
pub(crate) fn resized_to(
    layout: &Layout,
    desktop: &Desktop,
    instance: &Node,
    representation: Representation,
) -> Result<Vec<LayoutOp>, EditError> {
    let Part::Instance(group, id) = &instance.part else {
        return Err(EditError::nothing());
    };
    let mut work = Work::new(layout, desktop, instance.layer);
    let written = work
        .written(work.layer, &instance.area)?
        .instance(group, &id.template());
    let mut changed = written.instance.clone();
    changed.representation = Some(representation);
    work.apply(written.ops(&changed))?;
    settle_group(&mut work, &instance.area, group)?;
    Ok(work.done())
}

/// The group `group` of the grid `area` moved so its first cell is `to`, what it lands on moved out of its way.
pub(crate) fn group_moved(
    layout: &Layout,
    desktop: &Desktop,
    area: &Node,
    group: &GroupId,
    to: (u32, u32),
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, area.layer);
    let now = cells(&work, &area.area, group)?;
    reflow(&mut work, &area.area, group, now.at(to.0, to.1))?;
    Ok(work.done())
}

/// The group `group` of the grid `area` placed as `kind` says — its cell and its spans — what it grows over moved out of its way.
pub(crate) fn group_spanned(
    layout: &Layout,
    desktop: &Desktop,
    area: &Node,
    group: &GroupId,
    kind: GroupKind,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, area.layer);
    work.rewrite(work.layer, &area.area, |written| {
        group_mut(written, group).kind = Some(kind)
    })?;
    settle_group(&mut work, &area.area, group)?;
    Ok(work.done())
}

/// What a popover's change to a group, or to an instance in one, on a grid of `desktop`'s screen brings with it: whatever the group now covers, moved out of its way.
fn settle(node: &Node, desktop: &Desktop, after: &Layout) -> Vec<LayoutOp> {
    let (Part::Instance(group, _) | Part::Group(group)) = &node.part else {
        return Vec::new();
    };
    let mut work = Work::new(after, desktop, node.layer);
    match settle_group(&mut work, &node.area, group) {
        Ok(()) => work.done(),
        Err(why) => {
            tracing::info!("nothing moved out of the way: {why}");
            Vec::new()
        }
    }
}

/// What a grid is left needing once the instance `id` has left the group `group` of its area `area`: a group with nothing in it taken away, since it would hold its cells empty — but for a container, which says it is empty and waits to be filled — and a stack of one made a widget again.
pub(crate) fn tidied(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    area: &AreaId,
    group: &GroupId,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let Ok(now) = grid_of(&work, area) else {
        return Ok(Vec::new());
    };
    let Some(held) = now.groups.iter().find(|held| held.id == *group) else {
        return Ok(Vec::new());
    };
    let still = |work: &Work, test: &dyn Fn(&layout::ResolvedGroup) -> bool| {
        work.area(layer, area)
            .map(|area| {
                area.groups
                    .iter()
                    .any(|held| held.id == *group && test(held))
            })
            .unwrap_or(false)
    };
    if held.children.is_empty() && !container::arranges(held) {
        work.rewrite(layer, area, |written| {
            written.groups.retain(|held| held.id != *group)
        })?;
        if still(&work, &|_| true) {
            work.rewrite(layer, area, |written| written.remove.push(group.clone()))?;
        }
    } else if held.children.len() == 1 && held.is_pages() {
        work.rewrite(layer, area, |written| {
            group_mut(written, group).clear_arrangement()
        })?;
        if still(&work, &|held| held.is_pages()) {
            work.rewrite(layer, area, |written| {
                group_mut(written, group).unset.push(Unset::Arrange)
            })?;
        }
    }
    Ok(work.done())
}

/// A new, empty grid on the middle of the usable part of the screen being edited, as one undo entry, selected once it is there.
pub(crate) fn create_grid() -> Result<(), EditError> {
    context::make(new_grid, |id| {
        telar::t!("editor.desktop.grid_made", name = id.to_string())
    })
}

/// The operations that make a new, empty grid on `layer`: the middle of what the reserving areas leave, its cells from the top left.
pub(crate) fn new_grid(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
) -> Result<(Vec<LayoutOp>, AreaId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let id = layout::ops::free_area_id(layout, &work.known, layer, "widgets");
    let area = Area {
        id: id.clone(),
        kind: Some(AreaKind::Grid {
            rect: Some(layout::Rect {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5,
            }),
            cell: None,
            gap: None,
            anchor: Some(Anchor::TopLeft),
        }),
        within: Some(Within::Usable),
        ..Area::default()
    };
    work.add(layer, area)?;
    Ok((work.done(), id))
}

/// Where the widget the palette adds goes: the grid or panel the selection is on, near what is selected, else the first grid of the edited layer from its first cell.
pub(crate) fn target_near(desktop: &Desktop, layer: LayerKind) -> Option<(AreaId, (u32, u32))> {
    selected_cells(desktop, layer).or_else(|| {
        desktop
            .resolved
            .layer(layer)?
            .areas
            .iter()
            .find(|area| matches!(area.kind, ResolvedAreaKind::Grid { .. }))
            .map(|area| (area.id.clone(), (0, 0)))
    })
}

/// The grid or panel of `layer` the selection is on, and the cell of what is selected there.
pub(crate) fn selected_cells(desktop: &Desktop, layer: LayerKind) -> Option<(AreaId, (u32, u32))> {
    let selected = session::selected();
    let node = selected.node().filter(|node| node.layer == layer)?;
    let area = desktop
        .resolved
        .area(layer, &node.area)
        .filter(|area| area.kind.places_on_cells())?;
    let near = match &node.part {
        Part::Instance(group, _) | Part::Group(group) => area
            .groups
            .iter()
            .find(|held| held.id == *group)
            .and_then(grid::cells_of)
            .map_or((0, 0), |cells| (cells.col, cells.row)),
        Part::Area => (0, 0),
    };
    Some((area.id.clone(), near))
}

/// Puts what `pick` names on a grid of `layer` on the screen being edited, as one undo entry, and selects it: on the cells `at` of the grid there where the pointer put it, else on the free cells nearest the selection — on a grid made for it where the layer has none.
pub(crate) fn put(
    pick: &Pick,
    at: Option<(AreaId, (u32, u32))>,
    layer: LayerKind,
) -> Result<(), EditError> {
    if let Pick::Stack = pick {
        return super::overlay::add_stack();
    }
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let (mut ops, area, cell, near) = match at {
        Some((area, cell)) => (Vec::new(), area, Some(cell), (0, 0)),
        None => match target_near(&desktop, layer) {
            Some((area, near)) => (Vec::new(), area, None, near),
            None => {
                let (made, area) = new_grid(&layout, &desktop, layer)?;
                (made, area, None, (0, 0))
            }
        },
    };
    let mut after = layout.clone();
    layout::ops::apply_all(&mut after, &ops)?;
    if let Pick::Container = pick {
        let (placed, group) = container::on_grid(&after, &desktop, layer, &area, (cell, near))?;
        ops.extend(placed);
        context::commit(
            telar::t!("editor.container.made", name = group.to_string()),
            ops,
        )?;
        session::select(Selection::Group(
            Node::area(Some(&mode.output), layer, &area).group(&group),
        ));
        return Ok(());
    }
    if let Pick::Komponent(id) = pick {
        let (placed, group) = crate::komponent::planned(
            &after,
            &desktop,
            layer,
            &area,
            id,
            crate::komponent::Where::Cell { at: cell, near },
        )?;
        ops.extend(placed);
        context::commit(
            telar::t!("editor.komponent.used", komponent = id.to_string()),
            ops,
        )?;
        session::select(Selection::Group(
            Node::area(Some(&mode.output), layer, &area).group(&group),
        ));
        return Ok(());
    }
    let (module, representation) = pick.widget()?;
    let name = ui::descriptor::find(module).map_or(module, |found| found.name);
    let adding = Adding {
        module,
        representation,
        at: cell,
        near,
    };
    let (placed, id) = added(&after, &desktop, layer, &area, &adding)?;
    ops.extend(placed);
    context::commit(telar::t!("editor.desktop.added", name = name), ops)?;
    if let Some((node, _)) = surfaces::rects::instance(Some(&mode.output), &id) {
        session::select(Selection::Instance(node));
    }
    Ok(())
}

/// A grid's menu rows, in its own mode only since each acts on the mode: adding a widget or a container to it, and making another grid.
fn grid_rows(_: &ResolvedArea, node: &Node) -> Vec<telar::MenuEntry> {
    if !crate::mode::editing(node) {
        return Vec::new();
    }
    let (selected, holding) = (node.clone(), node.clone());
    vec![
        telar::MenuEntry::row(
            telar::t!("editor.desktop.add_widget"),
            keys::spell(&[Chord::char('a')]),
            move || {
                session::select(Selection::Area(selected.clone()));
                said(palette::open());
            },
        ),
        telar::MenuEntry::row(
            telar::t!("editor.container.new"),
            keys::spell(&container::create_key().keys),
            move || {
                session::select(Selection::Area(holding.clone()));
                said(container::create());
            },
        ),
        telar::MenuEntry::row(
            telar::t!("editor.desktop.new_grid"),
            keys::spell(&[grid_key()]),
            || said(create_grid()),
        ),
    ]
}

/// A grid's popover rows: how big its cells are and how far apart, what it is anchored to, and its rectangle.
fn grid_tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::Grid {
        rect,
        cell,
        gap,
        anchor,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let cell = draft.setting(
        "cell",
        "cell",
        kind_read!(Grid { cell }, cell),
        |area, value: &f32| kind_field!(area, "grid", Grid { cell }, *value),
    );
    let gap = draft.setting(
        "gap",
        "gap",
        kind_read!(Grid { gap }, gap),
        |area, value: &f32| kind_field!(area, "grid", Grid { gap }, *value),
    );
    let mut list = vec![
        draft.marked(
            &["cell"],
            rows::number(
                label!("editor.area.cell"),
                help("AreaKind::Grid", "cell"),
                cell,
                Range::whole(16.0, 400.0),
            )?,
        )?,
        draft.marked(
            &["gap"],
            rows::number(
                label!("editor.area.gap"),
                help("AreaKind::Grid", "gap"),
                gap,
                crate::popover::handles::GAP,
            )?,
        )?,
    ];
    list.extend(chosen(
        draft,
        "anchor",
        label!("editor.area.anchor"),
        help("AreaKind::Grid", "anchor"),
        variants("Anchor"),
        kind_read!(Grid { anchor }, anchor),
        |area, anchor: Anchor| kind_field!(area, "grid", Grid { anchor }, anchor),
    )?);
    list.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// A widget's popover rows: its size stepped S, M, L through what its module draws, the stepper beside the corner handle (TA-5).
fn instance_tool(draft: &InstanceDraft) -> Result<Inspector, LayoutError> {
    let rows = match draft.area_kind {
        "grid" if !crate::popover::instance::arranged(draft) => size_rows(draft)?,
        _ => Vec::new(),
    };
    Ok(Inspector {
        rows,
        handles: Vec::new(),
    })
}

/// The sizes `module` draws as a widget on `layer`, smallest first: what the size row, the corner handle, Ctrl+arrows and the palette all step through ([`sizes_drawn`]).
pub(crate) fn sizes_of(module: &str, layer: LayerKind) -> Vec<Representation> {
    ui::descriptor::find(module).map_or_else(Vec::new, |descriptor| sizes_drawn(descriptor, layer))
}

/// The sizes `descriptor` draws as a widget on `layer`, smallest first: on the lock screen only those that only read (TA-8).
pub(crate) fn sizes_drawn(descriptor: &ModuleDescriptor, layer: LayerKind) -> Vec<Representation> {
    SIZES
        .into_iter()
        .filter(|size| {
            descriptor
                .input(surfaces::area::representation(*size))
                .is_some_and(|input| placeable(input, layer))
        })
        .collect()
}

/// Whether a representation that takes `input` may be placed on `layer`: anything on the session's layers, only readings on the lock screen (TA-8).
fn placeable(input: Input, layer: LayerKind) -> bool {
    layer != LayerKind::Lock || input == Input::ReadOnly
}

/// What the size row calls a size.
pub(crate) fn size_letter(size: Representation) -> &'static str {
    match size {
        Representation::WidgetS => "S",
        Representation::WidgetM => "M",
        Representation::WidgetL => "L",
        Representation::Chip | Representation::Card => "",
    }
}

fn size_rows(draft: &InstanceDraft) -> Rows {
    let sizes = sizes_of(&draft.resolved.module, draft.node.layer);
    if sizes.len() < 2 {
        return Ok(Vec::new());
    }
    let letters: std::rc::Rc<[&'static str]> =
        sizes.iter().map(|size| size_letter(*size)).collect();
    let picked = telar::signal(size_letter(draft.resolved.representation).to_string());
    let stepping = draft.clone();
    let seeded = std::cell::Cell::new(false);
    telar::effect(move || {
        let letter = picked.get();
        if !seeded.replace(true) {
            return;
        }
        if let Some(size) = SIZES.into_iter().find(|size| size_letter(*size) == letter) {
            stepping.update("representation", move |held| {
                held.representation = Some(size)
            });
        }
    });
    Ok(vec![rows::choice(
        label!("editor.desktop.size"),
        help("Instance", "representation"),
        picked,
        letters,
    )?])
}

/// The instance `node` names moved onto the grid `onto` of `layer` as a widget drawn `representation`, keeping its id, options and state (TA-3): on the cells `at`, what it lands on moved out of its way, else on the free cells nearest the grid's first. A chip's move from its bar is this, and so is "From bars…".
pub(crate) fn moved_onto(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    (layer, onto): (LayerKind, &AreaId),
    representation: Representation,
    at: Option<(u32, u32)>,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let mut moved = take_out(&mut work, node)?;
    moved.representation = Some(representation);
    put_on(&mut work, onto, moved, at, (0, 0))?;
    Ok(work.done())
}

/// The instance `node` names, as `desktop`'s screen shows it.
pub(crate) fn shown_instance(desktop: &Desktop, node: &Node) -> Option<ResolvedInstance> {
    let Part::Instance(group, id) = &node.part else {
        return None;
    };
    desktop
        .resolved
        .area(node.layer, &node.area)?
        .groups
        .iter()
        .find(|held| held.id == *group)?
        .children
        .iter()
        .find(|child| child.id == id.template())
        .cloned()
}
