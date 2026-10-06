//! What the desktop mode lays over the edited screen: every widget on a grid, dragged by itself, with the cells it would land on outlined as it goes; the selected widget's corner handle, which steps its size; and the cells a widget picked from the palette can be put on.
//!
//! **The cells under the pointer hold still.** A drag reads the pointer against the grids as they were drawn when it began ([`Geometry`]), so the preview growing a grid, or an anchored grid centring itself again, does not move the cells from under it.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Border, Cursor, LayoutItem, LayoutStyle, ReactiveList, Rect, RectStyle, Role, RwSignal,
    StyledContainer, detached, signal, use_theme,
};

use config::theme::NordTheme;
use layout::{AreaId, GroupId, InstanceId, LayerKind, ResolvedArea, ResolvedAreaKind};
use surfaces::reconcile;
use surfaces::rects::{self, Node, Part};
use ui::descriptor::Built;

use crate::host::{passthrough, see_through, whole};
use crate::mode::{Mode, said};
use crate::session::{self, Edit, EditError, Selection};

use super::container::{self, Frame};
use super::desktop::{self, Landing};
use super::gesture::{self, Hint, pressable};
use super::grid::{self, Cells, Room};
use super::palette::{self, Pick};

/// How big the size handle on a widget's corner is across.
const HANDLE: f32 = 18.0;

/// Where a grid's cells are on screen, as it was drawn when a drag began.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub area: AreaId,
    /// The box the area itself is placed at.
    pub region: Rect,
    /// The top left corner of its first cell.
    pub origin: (f32, f32),
    pub cell: f32,
    /// From one cell to the next: a cell and a gap.
    pub pitch: f32,
    pub room: Room,
}

impl Geometry {
    /// The grid `area` as `desktop`'s screen places it; `None` for an area that is no grid.
    pub fn of(desktop: &surfaces::reconcile::Desktop, area: &ResolvedArea) -> Option<Self> {
        let ResolvedAreaKind::Grid {
            rect, cell, gap, ..
        } = area.kind
        else {
            return None;
        };
        let region = desktop::region_of(desktop, area.within, rect);
        let block = surfaces::area::grid_block(area, region)?;
        Some(Self {
            area: area.id.clone(),
            region,
            origin: (block.x, block.y),
            cell,
            pitch: cell + gap,
            room: desktop::room_of(desktop, area),
        })
    }

    /// `None` for a panel that is shut.
    pub fn of_panel(output: &str, layer: LayerKind, area: &ResolvedArea) -> Option<Self> {
        let ResolvedAreaKind::Panel {
            cols,
            rows,
            cell,
            gap,
            ..
        } = area.kind
        else {
            return None;
        };
        let region = rects::rect(&Node::area(Some(output), layer, &area.id))
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)?;
        let pad = area.style.padding.unwrap_or_default();
        Some(Self {
            area: area.id.clone(),
            region,
            origin: (region.x + pad.left(), region.y + pad.top()),
            cell,
            pitch: cell + gap,
            room: Room {
                cols: cols.max(1),
                rows: rows.max(1),
            },
        })
    }

    /// Where `cells` are on screen.
    pub fn rect_of(&self, cells: Cells) -> Rect {
        let gap = self.pitch - self.cell;
        Rect::new(
            self.origin.0 + cells.col as f32 * self.pitch,
            self.origin.1 + cells.row as f32 * self.pitch,
            cells.cols as f32 * self.pitch - gap,
            cells.rows as f32 * self.pitch - gap,
        )
    }

    /// The cell whose top left corner is nearest `point`.
    pub fn cell_at(&self, point: (f32, f32)) -> (u32, u32) {
        let along = |at: f32, from: f32| ((at - from) / self.pitch).round().max(0.0) as u32;
        (along(point.0, self.origin.0), along(point.1, self.origin.1))
    }
}

/// Where a drag would put what it carries, drawn as it goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Aim {
    /// On these cells, beside what is around them.
    Cells(Rect),
    /// Onto the widget here, stacking with it.
    Onto(Rect),
}

/// Every grid of a layer with where its cells are, as [`grids`] finds them.
pub(crate) type Grids = Vec<(Geometry, ResolvedArea)>;

thread_local! {
    static AIM: RwSignal<Option<Aim>> = detached(|| signal(None));
}

/// Where the drag under way would put what it carries: what the outline over the grids is drawn from.
pub(crate) fn aim() -> RwSignal<Option<Aim>> {
    AIM.with(|aim| *aim)
}

/// Every grid on `layer` of the screen `output` shows now, with its cells, and its groups on them.
pub(crate) fn grids(output: &str, layer: LayerKind) -> Grids {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    desktop
        .resolved
        .layer(layer)
        .map(|held| {
            held.areas
                .iter()
                .filter_map(|area| Some((Geometry::of(&desktop, area)?, area.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// Open panels come first: they are drawn over the grids.
pub(crate) fn drop_grids(output: &str, layer: LayerKind) -> Grids {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    let Some(held) = desktop.resolved.layer(layer) else {
        return Vec::new();
    };
    let panels = held
        .areas
        .iter()
        .filter_map(|area| Some((Geometry::of_panel(output, layer, area)?, area.clone())));
    panels.chain(grids(output, layer)).collect()
}

pub(crate) fn cell_landing(
    (geometry, area): &(Geometry, ResolvedArea),
    point: (f32, f32),
    grab: (f32, f32),
    footprint: Cells,
) -> (AreaId, Landing, Aim) {
    let corner = (point.0 - grab.0, point.1 - grab.1);
    let (col, row) = geometry.cell_at(corner);
    let cells = footprint.at(col, row).within(geometry.room);
    (
        area.id.clone(),
        Landing::Cell {
            col: cells.col,
            row: cells.row,
        },
        Aim::Cells(geometry.rect_of(cells)),
    )
}

/// Where something `footprint` big, carried with its top left corner `grab` from the pointer, would land with the pointer at `point`: into the container the pointer is anywhere over, at the slot under it; onto the widget whose middle the pointer is over; else on the cells under its corner — on the first of `grids` the pointer is over, `None` over none. `own` is the group it is carried out of, which it never lands in.
pub(crate) fn landing_at(
    grids: &[(Geometry, ResolvedArea)],
    point: (f32, f32),
    grab: (f32, f32),
    footprint: Cells,
    own: Option<(&AreaId, &GroupId)>,
) -> Option<(AreaId, Landing, Aim)> {
    let over = grids
        .iter()
        .find(|(geometry, _)| geometry.region.contains(point.0, point.1))?;
    let (geometry, area) = over;
    let middle = |rect: Rect| {
        Rect::new(
            rect.x + rect.width / 4.0,
            rect.y + rect.height / 4.0,
            rect.width / 2.0,
            rect.height / 2.0,
        )
    };
    let onto = area.groups.iter().find_map(|group| {
        if own == Some((&area.id, &group.id)) {
            return None;
        }
        let rect = geometry.rect_of(grid::cells_of(group)?);
        match container::Frame::of(geometry, group) {
            Some(frame) => frame.outer.contains(point.0, point.1).then(|| {
                (
                    Landing::Into(group.id.clone(), frame.adopting(point)),
                    Aim::Onto(rect),
                )
            }),
            None => middle(rect)
                .contains(point.0, point.1)
                .then(|| (Landing::Onto(group.id.clone()), Aim::Onto(rect))),
        }
    });
    if let Some((landing, aimed)) = onto {
        return Some((area.id.clone(), landing, aimed));
    }
    Some(cell_landing(over, point, grab, footprint))
}

/// What the tag at the pointer says of `landing`: the cell the carried widget's corner goes on, counted from one, or what letting go onto a group does — joins a container, or stacks.
pub(crate) fn landing_tag(landing: &Landing) -> String {
    match landing {
        Landing::Cell { col, row } => {
            telar::t!("editor.pointer.cell", col = col + 1, row = row + 1)
        }
        Landing::Into(..) => telar::t!("editor.pointer.into"),
        Landing::Onto(_) => telar::t!("editor.pointer.stack"),
    }
}

/// The desktop mode's layer over the edited screen.
pub(crate) fn tool(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let dragging: RwSignal<Option<Vec<InstanceId>>> = signal(None);
    let (listing, building) = (output.clone(), output.clone());
    let targets = ReactiveList::with_style(
        whole(),
        move || match dragging.get() {
            Some(frozen) => frozen,
            None => widgets(&listing, layer),
        },
        |id: &InstanceId| id.clone(),
        move |id: InstanceId| target(&building, layer, id, dragging),
    )?;
    let placing = placing(&output, layer)?;
    let handle = {
        let (output, building) = (output.clone(), output.clone());
        ReactiveList::with_style(
            whole(),
            move || selected_widget(&output, layer).into_iter().collect(),
            |node: &Node| node.clone(),
            {
                let output = building;
                move |node: Node| match container_of(&drop_grids(&output, layer), &node) {
                    Some(frame) => child_handle(node, frame),
                    None => size_handle(node),
                }
            },
        )?
    };
    Ok(Box::new(passthrough(
        whole(),
        vec![
            see_through(targets)?,
            see_through(placing)?,
            see_through(handle)?,
            outline()?,
        ],
    )?))
}

pub(crate) fn placing_tool(mode: &Mode) -> Built {
    Ok(Box::new(passthrough(
        whole(),
        vec![see_through(placing(&mode.output, mode.layer)?)?, outline()?],
    )?))
}

fn placing(output: &str, layer: LayerKind) -> Result<ReactiveList, telar::LayoutError> {
    let (listing, building) = (output.to_string(), output.to_string());
    ReactiveList::with_style(
        whole(),
        move || match palette::picked().get() {
            Some(_) => drop_grids(&listing, layer)
                .into_iter()
                .map(|(geometry, _)| geometry)
                .collect(),
            None => Vec::new(),
        },
        |geometry: &Geometry| format!("{geometry:?}"),
        move |geometry: Geometry| cells_target(&building, geometry, layer),
    )
}

/// Every widget on a grid of the edited layer, each once.
fn widgets(output: &str, layer: LayerKind) -> Vec<InstanceId> {
    let grids: Vec<AreaId> = drop_grids(output, layer)
        .into_iter()
        .map(|(geometry, _)| geometry.area)
        .collect();
    let mut found: Vec<InstanceId> = Vec::new();
    for (node, _) in rects::on(Some(output), layer) {
        if let Part::Instance(_, id) = &node.part
            && grids.contains(&node.area)
            && !found.contains(&id.template())
        {
            found.push(id.template());
        }
    }
    found
}

/// The selected widget, where it is one on a grid of the edited layer.
fn selected_widget(output: &str, layer: LayerKind) -> Option<Node> {
    let Selection::Instance(node) = session::selection().get() else {
        return None;
    };
    let on_a_grid = drop_grids(output, layer)
        .iter()
        .any(|(geometry, _)| geometry.area == node.area);
    on_a_grid.then_some(node)
}

/// A box over one widget that takes its presses and drags: a press selects, a secondary press opens its menu, a drag carries it — to other cells, onto another widget, out of its stack or to another grid.
fn target(
    output: &str,
    layer: LayerKind,
    id: InstanceId,
    dragging: RwSignal<Option<Vec<InstanceId>>>,
) -> Built {
    let output = output.to_string();
    let name = rects::instance(Some(&output), &id)
        .and_then(|(node, _)| {
            let selection = Selection::Instance(node);
            Some(crate::steps::name_of(&selection)).filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| id.to_string());
    let edit = Edit::new(telar::t!("editor.desktop.moved", name = name));
    let last: Rc<Cell<Option<Rect>>> = Rc::default();
    let placed = {
        let (output, id) = (output.clone(), id.clone());
        move || {
            let now = rects::instance(Some(&output), &id).map(|(_, rect)| rect);
            if let Some(rect) = now {
                last.set(Some(rect));
            }
            last.get().unwrap_or_default()
        }
    };
    let frozen: RwSignal<Option<Rect>> = signal(None);
    let styled = {
        let placed = placed.clone();
        move || surfaces::area::at(frozen.get().unwrap_or_else(&placed))
    };
    let node_now = {
        let (output, id) = (output.clone(), id.clone());
        move || rects::instance(Some(&output), &id).map(|(node, _)| node)
    };
    let taking = node_now.clone();
    let previewing = edit.clone();
    Ok(Box::new(gesture::drag(
        pressable(
            StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?,
            node_now,
        )
        .styled_by(styled)
        .cursor(Cursor::Grab),
        edit.transaction(),
        move |pressed| {
            let node = taking()?;
            let rect = placed();
            frozen.set(Some(rect));
            dragging.set(Some(widgets(&output, layer)));
            session::select(Selection::Instance(node.clone()));
            Some(Carried::of(node, rect, pressed, &output, layer))
        },
        move |carried, _| {
            if let Some(point) = surfaces::menu::pointer() {
                carried.preview(&previewing, point);
            }
        },
        move |_, _| {
            frozen.set(None);
            dragging.set(None);
            aim().set(None);
        },
    )))
}

/// A widget being carried: what it is, where it was taken hold of, the container it was taken out of, and the grids as they were when it was.
struct Carried {
    node: Node,
    grab: (f32, f32),
    /// How big the widget was drawn when it was taken hold of.
    size: (f32, f32),
    footprint: Cells,
    grids: Grids,
    container: Option<Frame>,
}

impl Carried {
    fn of(node: Node, rect: Rect, point: (f32, f32), output: &str, layer: LayerKind) -> Self {
        let grids = drop_grids(output, layer);
        let (footprint, container) = match &node.part {
            Part::Instance(group, id) => {
                let held = grids
                    .iter()
                    .find(|(geometry, _)| geometry.area == node.area)
                    .and_then(|(geometry, area)| {
                        Some((geometry, area.groups.iter().find(|held| held.id == *group)?))
                    });
                let container = held.and_then(|(geometry, group)| Frame::of(geometry, group));
                let footprint = held.and_then(|(_, group)| {
                    let child = group
                        .children
                        .iter()
                        .find(|child| child.id == id.template());
                    match (container.is_some(), group.children.len() == 1) {
                        (true, _) => child.map(|child| {
                            desktop::footprint(desktop::loose_size(
                                &child.module,
                                layer,
                                child.representation,
                            ))
                        }),
                        (false, true) => grid::cells_of(group),
                        (false, false) => {
                            child.map(|child| desktop::footprint(child.representation))
                        }
                    }
                });
                (footprint, container)
            }
            _ => (None, None),
        };
        Self {
            node,
            grab: (point.0 - rect.x, point.1 - rect.y),
            size: (rect.width, rect.height),
            footprint: footprint.unwrap_or(Cells::ONE),
            grids,
            container,
        }
    }

    /// Previews the child taken to another slot, cell or box of its own container with the pointer at `point`, and the lines it snapped to; `None` once the pointer is outside it.
    fn preview_inside(&self, edit: &Edit, point: (f32, f32)) -> Option<()> {
        let frame = self.container.as_ref()?;
        if !frame.outer.contains(point.0, point.1) {
            return None;
        }
        let Part::Instance(_, id) = &self.node.part else {
            return None;
        };
        let moved = frame.moving(&id.template(), point, self.grab, crate::snap::free());
        let guides = moved
            .as_ref()
            .map(|(_, guides)| crate::snap::lines(guides, frame.inner))
            .unwrap_or_default();
        gesture::hint().set(Some(Hint {
            pointer: point,
            guides,
            ..Hint::default()
        }));
        let planned = moved.and_then(|(slot, _)| {
            let before = edit.transaction().before()?;
            let desktop = reconcile::desktop_now(self.node.output.as_deref())?;
            let ops =
                container::rearranged(&before, &desktop, &self.node, slot).unwrap_or_default();
            Some((ops, Aim::Onto(frame.outer)))
        });
        gesture::aimed(edit, planned, aim());
        Some(())
    }

    /// Previews where the widget lands with the pointer at `point`, and outlines it, with its ghost under the pointer and a tag beside it saying where it lands; over no grid, the layout stays as it was.
    fn preview(&self, edit: &Edit, point: (f32, f32)) {
        if self.preview_inside(edit, point).is_some() {
            return;
        }
        let own = match &self.node.part {
            Part::Instance(group, _) => Some((&self.node.area, group)),
            _ => None,
        };
        let landed = landing_at(&self.grids, point, self.grab, self.footprint, own);
        gesture::hint().set(Some(Hint {
            pointer: point,
            tag: landed.as_ref().map(|(_, landing, _)| landing_tag(landing)),
            ghost: Some(Rect::new(
                point.0 - self.grab.0,
                point.1 - self.grab.1,
                self.size.0,
                self.size.1,
            )),
            guides: Vec::new(),
        }));
        let planned = landed.and_then(|(onto, landing, aimed)| {
            let before = edit.transaction().before()?;
            let desktop = reconcile::desktop_now(self.node.output.as_deref())?;
            desktop::dropped(&before, &desktop, &self.node, &onto, &landing)
                .ok()
                .map(|ops| (ops, aimed))
        });
        gesture::aimed(edit, planned, aim());
    }
}

/// The outline of where a drag would put what it carries: the cells it would cover, or the widget it would stack onto.
fn outline() -> Result<Box<dyn LayoutItem>, telar::LayoutError> {
    let theme = use_theme::<NordTheme>();
    let list = ReactiveList::with_style(
        whole(),
        || aim().get().into_iter().collect(),
        |aimed: &Aim| format!("{aimed:?}"),
        move |aimed: Aim| {
            let (rect, fill, width) = match aimed {
                Aim::Cells(rect) => (rect, theme.accent.with_alpha(0.12), 2.0),
                Aim::Onto(rect) => (rect, theme.accent.with_alpha(0.28), 3.0),
            };
            let radius = ui::scale::corner::md();
            Ok(Box::new(
                StyledContainer::new(
                    surfaces::area::at(rect),
                    move |_| {
                        RectStyle::filled(fill, radius)
                            .with_border(Border::uniform(theme.accent, width))
                    },
                    Vec::new(),
                )?
                .input_transparent(),
            ) as Box<dyn LayoutItem>)
        },
    )?;
    see_through(list)
}

/// While a widget picked in the palette waits for a place, a box over the cells of one grid or open panel: a press puts it on the cells under the pointer, and the cells it would cover are outlined as the pointer moves.
fn cells_target(output: &str, geometry: Geometry, layer: LayerKind) -> Built {
    let hovering = geometry.clone();
    let pressing = geometry.clone();
    let footprint_of = {
        let (output, area) = (output.to_string(), geometry.area.clone());
        move |pick: &Pick| {
            let grids = drop_grids(&output, layer);
            let over = grids.iter().find(|(geometry, _)| geometry.area == area);
            picked_footprint(pick, over)
        }
    };
    let footprint_pressed = footprint_of.clone();
    Ok(Box::new(
        StyledContainer::new(
            surfaces::area::at(geometry.region),
            |_| RectStyle::default(),
            Vec::new(),
        )?
        .cursor(Cursor::Crosshair)
        .on_pointer_move(move |_, _| {
            let (Some(point), Some(pick)) = (surfaces::menu::pointer(), palette::picked().peek())
            else {
                return;
            };
            let cells = footprint_of(&pick).at(0, 0);
            let (col, row) = hovering.cell_at(corner_for(&hovering, cells, point));
            aim().set(Some(Aim::Cells(
                hovering.rect_of(cells.at(col, row).within(hovering.room)),
            )));
        })
        .on_press(move || {
            let (Some(point), Some(pick)) = (surfaces::menu::pointer(), palette::picked().peek())
            else {
                return;
            };
            let cells = footprint_pressed(&pick);
            let (col, row) = pressing.cell_at(corner_for(&pressing, cells, point));
            let at = cells.at(col, row).within(pressing.room);
            palette::unpick();
            said(desktop::put(
                &pick,
                Some((pressing.area.clone(), (at.col, at.row))),
                layer,
            ));
        }),
    ))
}

/// Where the top left corner of something `cells` big goes for the pointer to be over its middle.
pub(crate) fn corner_for(geometry: &Geometry, cells: Cells, point: (f32, f32)) -> (f32, f32) {
    let rect = geometry.rect_of(cells.at(0, 0));
    (point.0 - rect.width / 2.0, point.1 - rect.height / 2.0)
}

/// The cells what the palette picked covers once it is placed: a widget's at the size chosen for it, and on the grid `over`, for a container, the span that grid has room for.
pub(crate) fn picked_footprint(pick: &Pick, over: Option<&(Geometry, ResolvedArea)>) -> Cells {
    match pick {
        Pick::Komponent(id) => crate::written::known()
            .komponent(id)
            .map_or(Cells::ONE, crate::komponent::footprint),
        Pick::Container => over.map_or(Cells::ONE, |(geometry, area)| {
            container::footprint_on(area, geometry.room)
        }),
        Pick::Module(_, size) => size.map_or(Cells::ONE, desktop::footprint),
        Pick::Plate | Pick::Stack => Cells::ONE,
    }
}

/// The instance `node` names, on its screen as it shows now.
fn shown(node: &Node) -> Option<layout::ResolvedInstance> {
    desktop::shown_instance(&reconcile::desktop(node.output.as_deref())?, node)
}

/// The handle on the selected widget's corner that steps its size: dragged out it grows, dragged in it shrinks, to whichever size its module draws that the pointer is nearest; pressed, it takes the next size, round to the smallest after the largest. What it grows over moves out of its way.
fn size_handle(node: Node) -> Built {
    let theme = use_theme::<NordTheme>();
    let name = crate::steps::name_of(&Selection::Instance(node.clone()));
    let edit = Edit::new(telar::t!("editor.desktop.resized", name = name));
    let placed = {
        let node = node.clone();
        move || rects::rect(&node).unwrap_or_default()
    };
    let (pressed, dragged) = (node.clone(), node);
    let previewing = edit.clone();
    let at = placed.clone();
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| {
                RectStyle::filled(theme.surface, HANDLE / 2.0)
                    .with_border(Border::uniform(theme.accent, 2.0))
            },
            Vec::new(),
        )?
        .styled_by(move || {
            let rect = at();
            LayoutStyle::new()
                .absolute()
                .inset_start(rect.x + rect.width - HANDLE / 2.0)
                .inset_top(rect.y + rect.height - HANDLE / 2.0)
                .width(HANDLE)
                .height(HANDLE)
        })
        .control(Role::Button)
        .cursor(Cursor::NwseResize)
        .on_press(move || said(step_size(&pressed))),
        edit.transaction(),
        move |_| Some(placed()),
        move |rect, _| {
            let Some(point) = surfaces::menu::pointer() else {
                return;
            };
            let Some(size) = size_toward(&dragged, *rect, point) else {
                return;
            };
            let planned = previewing.transaction().before().and_then(|before| {
                let desktop = reconcile::desktop_now(dragged.output.as_deref())?;
                desktop::resized_to(&before, &desktop, &dragged, size).ok()
            });
            if let Some(ops) = planned {
                let _ = previewing.preview(ops);
            }
        },
        |_, _| {},
    )))
}

/// The container the instance `node` is a child of, as `grids` drew it.
fn container_of(grids: &[(Geometry, ResolvedArea)], node: &Node) -> Option<Frame> {
    let Part::Instance(group, _) = &node.part else {
        return None;
    };
    container::frame_in(grids, &node.area, group)
}

/// Where the handle of a child of `frame` drawn at `rect` sits: the end of a row's or a column's child, the bottom right corner of a grid's or a free one's.
fn handle_point(frame: &Frame, rect: Rect) -> ((f32, f32), Cursor) {
    match frame.group.arrange {
        Some(layout::Arrange::Row) => (
            (rect.x + rect.width, rect.y + rect.height / 2.0),
            Cursor::EwResize,
        ),
        Some(layout::Arrange::Column) => (
            (rect.x + rect.width / 2.0, rect.y + rect.height),
            Cursor::NsResize,
        ),
        _ => (
            (rect.x + rect.width, rect.y + rect.height),
            Cursor::NwseResize,
        ),
    }
}

/// The handle of the selected child of a container that sets what it takes of it: dragged, a row's or a column's child its weight, a grid's child its span and a free one its box, snapping to its siblings unless Alt is held, with a tag beside the pointer saying what it now takes.
fn child_handle(node: Node, frame: Frame) -> Built {
    let theme = use_theme::<NordTheme>();
    let name = crate::steps::name_of(&Selection::Instance(node.clone()));
    let edit = Edit::new(telar::t!("editor.desktop.resized", name = name));
    let placed = {
        let node = node.clone();
        move || rects::rect(&node).unwrap_or_default()
    };
    let at = placed.clone();
    let cursor = handle_point(&frame, placed()).1;
    let held = frame;
    let (dragged, taken) = (node.clone(), node.clone());
    let output_now = node.output.clone().unwrap_or_default();
    let dragged_layer = node.layer;
    let previewing = edit.clone();
    let Part::Instance(_, id) = &node.part else {
        return Err(telar::LayoutError::Engine(
            "a child's handle is for an instance".to_string(),
        ));
    };
    let id = id.template();
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| {
                RectStyle::filled(theme.surface, HANDLE / 2.0)
                    .with_border(Border::uniform(theme.accent, 2.0))
            },
            Vec::new(),
        )?
        .styled_by(move || {
            let ((x, y), _) = handle_point(&held, at());
            LayoutStyle::new()
                .absolute()
                .inset_start(x - HANDLE / 2.0)
                .inset_top(y - HANDLE / 2.0)
                .width(HANDLE)
                .height(HANDLE)
        })
        .control(Role::Button)
        .cursor(cursor),
        edit.transaction(),
        move |_| {
            Some((
                placed(),
                container_of(&drop_grids(output_now.as_str(), dragged_layer), &taken)?,
            ))
        },
        move |(start, frame), _| {
            let Some(point) = surfaces::menu::pointer() else {
                return;
            };
            let Some((slot, guides)) = frame.resizing(&id, *start, point, crate::snap::free())
            else {
                return;
            };
            gesture::hint().set(Some(Hint {
                pointer: point,
                tag: Some(frame.size_tag(&id, slot)),
                guides: crate::snap::lines(&guides, frame.inner),
                ..Hint::default()
            }));
            let planned = previewing.transaction().before().and_then(|before| {
                let desktop = reconcile::desktop_now(dragged.output.as_deref())?;
                Some(container::rearranged(&before, &desktop, &dragged, slot).unwrap_or_default())
            });
            if let Some(ops) = planned {
                let _ = previewing.preview(ops);
            }
        },
        |_, _| {},
    )))
}

/// The size of the widget `node` whose footprint, from the corner of `rect` it keeps, reaches nearest `point`.
fn size_toward(node: &Node, rect: Rect, point: (f32, f32)) -> Option<layout::Representation> {
    let module = shown(node)?.module;
    let geometry = drop_grids(node.output.as_deref()?, node.layer)
        .into_iter()
        .find(|(geometry, _)| geometry.area == node.area)?
        .0;
    let reach = (
        (point.0 - rect.x) / geometry.pitch,
        (point.1 - rect.y) / geometry.pitch,
    );
    desktop::sizes_of(&module, node.layer)
        .into_iter()
        .min_by(|a, b| {
            let off = |size: layout::Representation| {
                let cells = desktop::footprint(size);
                (cells.cols as f32 - reach.0).powi(2) + (cells.rows as f32 - reach.1).powi(2)
            };
            off(*a).total_cmp(&off(*b))
        })
}

/// The selected widget one size on, round to the smallest after the largest, as one undo entry.
fn step_size(node: &Node) -> Result<(), EditError> {
    let shown = shown(node).ok_or_else(EditError::nothing)?;
    let sizes = desktop::sizes_of(&shown.module, node.layer);
    let next = match sizes.iter().position(|size| *size == shown.representation) {
        Some(at) => sizes[(at + 1) % sizes.len()],
        None => *sizes.first().ok_or_else(|| {
            EditError::refused(telar::t!(
                "editor.keys.no_size",
                name = shown.module.clone()
            ))
        })?,
    };
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = desktop::resized_to(&session::draft().peek(), &desktop, node, next)?;
    crate::context::commit(
        telar::t!(
            "editor.desktop.resized",
            name = crate::steps::name_of(&Selection::Instance(node.clone()))
        ),
        ops,
    )
}
