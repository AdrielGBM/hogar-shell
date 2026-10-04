//! What the top mode lays over the edited screen: a strip along every free stretch of each edge that a new bar is pulled out of, every bar dragged by itself to another edge or along its own, every chip dragged between zones and bars with the line it would be put at, or off every bar onto the desktop, and a split button over the middle of each bar and a join button over each seam between two.
//!
//! **What is under the pointer holds still.** A drag reads the pointer against the bars and chips as they were drawn when it began, so the preview moving them does not move the places it is aiming at, and the targets the drag started on stay the nodes they were.

use std::cell::{Cell, RefCell};

use telar::{
    Border, Cursor, LayoutStyle, ReactiveList, Rect, RectStyle, RwSignal, StyledContainer,
    detached, signal, use_theme,
};

use config::Edge;
use config::theme::NordTheme;
use layout::{AreaId, InstanceId, LayerKind, ResolvedAreaKind, Zone};
use surfaces::reconcile;
use surfaces::rects::{self, Node, Part};
use ui::descriptor::Built;

use crate::host::{self, HOTSPOT, passthrough, see_through, whole};
use crate::mode::{Mode, said};
use crate::session::{self, Edit, Selection};

use super::gesture::{self, held, pressable};
use super::top::{self, ChipLanding, Drawn};

/// How deep the strip along a free stretch of an edge is, that a new bar is pulled out of.
const HOT: f32 = 6.0;
/// How far a pull from an edge travels inward before it previews the bar it makes.
const INWARD: f32 = 24.0;
/// How far outside a bar a carried chip still lands on it.
const CATCH: f32 = 12.0;

/// Where a drag under way would put what it carries, drawn as it goes.
#[derive(Clone, Debug, PartialEq)]
enum Aim {
    /// The line a carried chip would be put at, across the bar it lands on.
    Line(Rect),
    /// A chip carried off every bar, with the pointer here: letting it go takes it off the layout.
    Off((f32, f32)),
}

thread_local! {
    static AIM: RwSignal<Option<Aim>> = detached(|| signal(None));
}

fn aim() -> RwSignal<Option<Aim>> {
    AIM.with(|aim| *aim)
}

/// The top mode's layer over the edited screen.
pub(crate) fn tool(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let frozen = signal(false);
    let strips = {
        let (listing, building) = (output.clone(), output.clone());
        ReactiveList::with_style(
            whole(),
            held(frozen, move || strips_of(&listing)),
            |strip: &Strip| format!("{strip:?}"),
            move |strip: Strip| edge_strip(&building, layer, strip, frozen),
        )?
    };
    let bars = {
        let (listing, building) = (output.clone(), output.clone());
        ReactiveList::with_style(
            whole(),
            held(frozen, move || bars_on(&listing, layer)),
            |id: &AreaId| id.clone(),
            move |id: AreaId| bar_target(Node::area(Some(&building), layer, &id), frozen),
        )?
    };
    let chips = {
        let (listing, building) = (output.clone(), output.clone());
        ReactiveList::with_style(
            whole(),
            held(frozen, move || chips_on(&listing, layer)),
            |id: &InstanceId| id.clone(),
            move |id: InstanceId| chip_target(&building, layer, id, frozen),
        )?
    };
    let buttons = {
        let (listing, building) = (output.clone(), output);
        ReactiveList::with_style(
            whole(),
            held(frozen, move || buttons_of(&listing, layer)),
            |button: &Button| button.clone(),
            move |button: Button| button.build(&building, layer, frozen),
        )?
    };
    Ok(Box::new(passthrough(
        whole(),
        vec![
            see_through(strips)?,
            see_through(bars)?,
            see_through(chips)?,
            see_through(buttons)?,
            marks()?,
        ],
    )?))
}

/// A free stretch of an edge, from `from` to `until` along it.
#[derive(Clone, Debug, PartialEq)]
struct Strip {
    edge: Edge,
    from: f32,
    until: f32,
}

fn strips_of(output: &str) -> Vec<Strip> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    Edge::ALL
        .into_iter()
        .flat_map(|edge| {
            let run = top::new_run(&desktop, edge);
            top::free_on(&desktop, edge, run, None)
                .into_iter()
                .map(move |(from, until)| Strip { edge, from, until })
        })
        .collect()
}

/// How far inward from `edge` of a screen `size` big a point is, and where along the edge.
pub(crate) fn measured(edge: Edge, size: (f32, f32), (x, y): (f32, f32)) -> (f32, f32) {
    match edge {
        Edge::Top => (y, x),
        Edge::Bottom => (size.1 - y, x),
        Edge::Left => (x, y),
        Edge::Right => (size.0 - x, y),
    }
}

/// The edge of a screen `size` big the point is nearest.
pub(crate) fn nearest_edge(size: (f32, f32), point: (f32, f32)) -> Edge {
    Edge::ALL
        .into_iter()
        .min_by(|a, b| {
            measured(*a, size, point)
                .0
                .total_cmp(&measured(*b, size, point).0)
        })
        .unwrap_or(Edge::Top)
}

/// The strip along one free stretch of an edge: pulled inward, it previews a new bar there, made on release and selected; Esc puts things back.
fn edge_strip(output: &str, layer: LayerKind, strip: Strip, frozen: RwSignal<bool>) -> Built {
    let size = reconcile::desktop(Some(output)).map_or((0.0, 0.0), |desktop| desktop.size);
    let length = strip.until - strip.from;
    let rect = match strip.edge {
        Edge::Top => Rect::new(strip.from, 0.0, length, HOT),
        Edge::Bottom => Rect::new(strip.from, size.1 - HOT, length, HOT),
        Edge::Left => Rect::new(0.0, strip.from, HOT, length),
        Edge::Right => Rect::new(size.0 - HOT, strip.from, HOT, length),
    };
    let edit = Edit::new(telar::t!(
        "editor.top.new_bar",
        edge = top::edge_name(strip.edge)
    ));
    let previewing = edit.clone();
    let (planning, selecting) = (output.to_string(), output.to_string());
    let edge = strip.edge;
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            surfaces::area::at(rect),
            |_| RectStyle::default(),
            Vec::new(),
        )?
        .cursor(Cursor::Crosshair),
        edit.transaction(),
        move |_| {
            frozen.set(true);
            Some(RefCell::new(None::<AreaId>))
        },
        move |made, _| {
            let (Some(point), Some(before)) =
                (surfaces::menu::pointer(), previewing.transaction().before())
            else {
                return;
            };
            let (inward, along) = measured(edge, size, point);
            let planned = (inward >= INWARD)
                .then(|| reconcile::desktop_now(Some(&planning)))
                .flatten()
                .and_then(|desktop| top::created(&before, &desktop, layer, edge, Some(along)).ok());
            let shown = planned.and_then(|(ops, id)| previewing.preview(ops).ok().map(|()| id));
            if shown.is_none() {
                let _ = previewing.preview(Vec::new());
            }
            *made.borrow_mut() = shown;
        },
        move |made, let_go| {
            frozen.set(false);
            if let (true, Some(id)) = (let_go, made.and_then(RefCell::into_inner)) {
                session::select(Selection::Area(Node::area(Some(&selecting), layer, &id)));
            }
        },
    )))
}

/// Every bar of the edited layer, each once.
fn bars_on(output: &str, layer: LayerKind) -> Vec<AreaId> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    desktop
        .resolved
        .layer(layer)
        .map(|held| {
            held.areas
                .iter()
                .filter(|area| matches!(area.kind, ResolvedAreaKind::Bar { .. }))
                .map(|area| area.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Every chip on a bar of the edited layer, each once.
fn chips_on(output: &str, layer: LayerKind) -> Vec<InstanceId> {
    let bars = bars_on(output, layer);
    let mut found: Vec<InstanceId> = Vec::new();
    for (node, _) in rects::on(Some(output), layer) {
        if let Part::Instance(_, id) = &node.part
            && bars.contains(&node.area)
            && !found.contains(&id.template())
        {
            found.push(id.template());
        }
    }
    found
}

/// The edge the bar `node` names runs along now.
fn edge_of(node: &Node) -> Option<Edge> {
    reconcile::desktop(node.output.as_deref())?
        .resolved
        .area(node.layer, &node.area)?
        .kind
        .edge()
}

/// A box over one bar: a press selects, a drag carries the bar — along its own edge where it does not run the whole of it, to whichever other edge the pointer is nearest otherwise, its chips laid along that edge as it goes.
fn bar_target(node: Node, frozen: RwSignal<bool>) -> Built {
    let edit = Edit::new(telar::t!("editor.top.moved", name = node.area.to_string()));
    let stopped: RwSignal<Option<Rect>> = signal(None);
    let placed = {
        let node = node.clone();
        move || {
            stopped
                .get()
                .unwrap_or_else(|| rects::rect(&node).unwrap_or_default())
        }
    };
    let area = {
        let node = node.clone();
        move || Some(node.clone())
    };
    let previewing = edit.clone();
    let (reading, dragged) = (node.clone(), node);
    Ok(Box::new(gesture::drag(
        pressable(
            StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?,
            area,
        )
        .styled_by(move || surfaces::area::at(placed()))
        .cursor(Cursor::Grab),
        edit.transaction(),
        move |_| {
            stopped.set(rects::rect(&reading));
            frozen.set(true);
            Some(())
        },
        move |(), _| {
            let (Some(point), Some(before), Some(desktop)) = (
                surfaces::menu::pointer(),
                previewing.transaction().before(),
                reconcile::desktop_now(dragged.output.as_deref()),
            ) else {
                return;
            };
            let own = desktop
                .resolved
                .area(dragged.layer, &dragged.area)
                .and_then(|area| area.kind.edge());
            let edge = nearest_edge(desktop.size, point);
            let (_, along) = measured(edge, desktop.size, point);
            let (layer, id) = (dragged.layer, &dragged.area);
            let planned = match own == Some(edge) {
                true => top::slid(&before, &desktop, layer, id, along),
                false => top::moved_to_edge(&before, &desktop, layer, id, edge, Some(along)),
            };
            let _ = previewing.preview(planned.unwrap_or_default());
        },
        move |_, _| {
            stopped.set(None);
            frozen.set(false);
        },
    )))
}

/// One bar as a carried chip reads it: where it was drawn, and its zones' chips along it, the carried one left out.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub(crate) id: AreaId,
    pub(crate) edge: Edge,
    pub(crate) rect: Rect,
    pub(crate) zones: [(Zone, Vec<(f32, f32)>); 3],
}

impl Seen {
    fn along(&self, point: (f32, f32)) -> f32 {
        match self.edge.is_vertical() {
            true => point.1,
            false => point.0,
        }
    }

    fn extent(&self) -> (f32, f32) {
        match self.edge.is_vertical() {
            true => (self.rect.y, self.rect.y + self.rect.height),
            false => (self.rect.x, self.rect.x + self.rect.width),
        }
    }

    /// Where on this bar a chip let go at `along` goes, and where the line saying so is drawn: into the third of the bar it is over, before the first chip of that zone whose middle is past it.
    pub(crate) fn landing(&self, along: f32) -> (ChipLanding, f32) {
        let (start, end) = self.extent();
        let third = (end - start) / 3.0;
        let zone = match along {
            at if at < start + third => Zone::Start,
            at if at < end - third => Zone::Center,
            _ => Zone::End,
        };
        let chips = &self
            .zones
            .iter()
            .find(|(held, _)| *held == zone)
            .map(|(_, chips)| chips.clone())
            .unwrap_or_default();
        let index = chips
            .iter()
            .filter(|(from, until)| (from + until) / 2.0 < along)
            .count();
        let line = match (
            index.checked_sub(1).and_then(|at| chips.get(at)),
            chips.get(index),
        ) {
            (Some(before), Some(after)) => (before.1 + after.0) / 2.0,
            (Some(before), None) => before.1 + 2.0,
            (None, Some(after)) => after.0 - 2.0,
            (None, None) => match zone {
                Zone::Start => start + 4.0,
                Zone::Center => (start + end) / 2.0,
                Zone::End => end - 4.0,
            },
        };
        (
            ChipLanding {
                area: self.id.clone(),
                zone,
                index,
            },
            line,
        )
    }

    pub(crate) fn line_at(&self, along: f32) -> Rect {
        match self.edge.is_vertical() {
            true => Rect::new(self.rect.x, along - 1.0, self.rect.width, 2.0),
            false => Rect::new(along - 1.0, self.rect.y, 2.0, self.rect.height),
        }
    }
}

/// A chip being carried: what it is, and the bars as they were when it was taken hold of.
struct Carried {
    node: Node,
    bars: Vec<Seen>,
}

impl Carried {
    fn of(node: Node, output: &str, layer: LayerKind) -> Self {
        let carried = match &node.part {
            Part::Instance(_, id) => Some(id.template()),
            _ => None,
        };
        let desktop = reconcile::desktop(Some(output));
        let bars = bars_on(output, layer)
            .into_iter()
            .filter_map(|id| {
                let area = desktop.as_ref()?.resolved.area(layer, &id)?.clone();
                let edge = area.kind.edge()?;
                let rect = rects::rect(&Node::area(Some(output), layer, &id))?;
                let drawn = Drawn::of(Some(output), layer, &id, edge);
                let zones = [Zone::Start, Zone::Center, Zone::End].map(|zone| {
                    let chips: Vec<(f32, f32)> = area
                        .groups
                        .iter()
                        .filter(|group| {
                            group.kind == layout::GroupKind::Zone { zone } && !group.stacked
                        })
                        .flat_map(|group| {
                            group
                                .children
                                .iter()
                                .filter(|child| Some(&child.id) != carried.as_ref())
                                .filter_map(|child| {
                                    drawn.along(&Part::Instance(group.id.clone(), child.id.clone()))
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    (zone, chips)
                });
                Some(Seen {
                    id,
                    edge,
                    rect,
                    zones,
                })
            })
            .collect();
        Self { node, bars }
    }

    /// Previews where the chip lands with the pointer at `point`, and marks it: on the bar under the pointer at its insertion line, or off every bar and so off the layout.
    fn preview(&self, edit: &Edit, point: (f32, f32)) {
        let (Some(before), Some(desktop)) = (
            edit.transaction().before(),
            reconcile::desktop_now(self.node.output.as_deref()),
        ) else {
            return;
        };
        let over = self.bars.iter().find(|bar| {
            let rect = bar.rect;
            Rect::new(
                rect.x - CATCH,
                rect.y - CATCH,
                rect.width + 2.0 * CATCH,
                rect.height + 2.0 * CATCH,
            )
            .contains(point.0, point.1)
        });
        let planned = match over {
            Some(bar) => {
                let (landing, line) = bar.landing(bar.along(point));
                top::chip_moved(&before, &desktop, &self.node, &landing)
                    .map(|ops| (ops, Aim::Line(bar.line_at(line))))
            }
            None => crate::context::removal(&before, &desktop, &self.node)
                .map(|ops| (ops, Aim::Off(point))),
        };
        gesture::aimed(edit, planned.ok(), aim());
    }
}

/// A box over one chip: a press selects, a secondary press opens its menu, a drag carries it to the insertion line under the pointer — another zone, another bar on any edge — or off every bar, which takes it off the layout.
fn chip_target(output: &str, layer: LayerKind, id: InstanceId, frozen: RwSignal<bool>) -> Built {
    let output = output.to_string();
    let name = rects::instance(Some(&output), &id)
        .map(|(node, _)| crate::steps::name_of(&Selection::Instance(node)))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| id.to_string());
    let edit = Edit::new(telar::t!("editor.top.chip_moved", name = name));
    let last: std::rc::Rc<Cell<Option<Rect>>> = std::rc::Rc::default();
    let placed = {
        let (output, id) = (output.clone(), id.clone());
        move || {
            if let Some((_, rect)) = rects::instance(Some(&output), &id) {
                last.set(Some(rect));
            }
            last.get().unwrap_or_default()
        }
    };
    let stopped: RwSignal<Option<Rect>> = signal(None);
    let styled = {
        let placed = placed.clone();
        move || surfaces::area::at(stopped.get().unwrap_or_else(&placed))
    };
    let node_now = {
        let (output, id) = (output.clone(), id.clone());
        move || rects::instance(Some(&output), &id).map(|(node, _)| node)
    };
    let previewing = edit.clone();
    let taking = node_now.clone();
    Ok(Box::new(gesture::drag(
        pressable(
            StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?,
            node_now,
        )
        .styled_by(styled)
        .cursor(Cursor::Grab),
        edit.transaction(),
        move |_| {
            let node = taking()?;
            stopped.set(Some(placed()));
            session::select(Selection::Instance(node.clone()));
            frozen.set(true);
            Some(Carried::of(node, &output, layer))
        },
        move |carried, _| {
            if let Some(point) = surfaces::menu::pointer() {
                carried.preview(&previewing, point);
            }
        },
        move |_, _| {
            stopped.set(None);
            frozen.set(false);
            aim().set(None);
        },
    )))
}

/// One button over the bars, named by what it acts on so it stays the same node while a drag previews the bars around it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Button {
    Split(AreaId),
    Join(AreaId, AreaId),
}

/// A split button for every bar long enough to cut in two, and a join button for every two bars that follow each other on an edge.
fn buttons_of(output: &str, layer: LayerKind) -> Vec<Button> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    let mut buttons = Vec::new();
    for id in bars_on(output, layer) {
        let long = top::bars_of(&desktop)
            .iter()
            .find(|bar| bar.layer == layer && bar.id == id)
            .is_some_and(|bar| bar.span.along >= 2.0 * crate::popover::handles::SHORTEST);
        if long {
            buttons.push(Button::Split(id.clone()));
        }
        if let Some(next) = top::beside(&desktop, layer, &id, true) {
            buttons.push(Button::Join(id, next));
        }
    }
    buttons
}

/// Where a button over the inner side of the bar `node` names sits, centred at `along` on it: just off the bar, towards the middle of the screen, so it covers none of its chips.
fn inner_side(node: &Node, along: impl Fn(Rect) -> f32) -> Option<(f32, f32)> {
    let rect = rects::rect(node)?;
    let at = along(rect);
    let off = HOTSPOT / 2.0 + 4.0;
    Some(match edge_of(node)? {
        Edge::Top => (at, rect.y + rect.height + off),
        Edge::Bottom => (at, rect.y - off),
        Edge::Left => (rect.x + rect.width + off, at),
        Edge::Right => (rect.x - off, at),
    })
}

impl Button {
    fn build(&self, output: &str, layer: LayerKind, frozen: RwSignal<bool>) -> Built {
        match self {
            Button::Split(id) => split_button(Node::area(Some(output), layer, id), frozen),
            Button::Join(first, second) => {
                join_button(Node::area(Some(output), layer, first), second)
            }
        }
    }
}

/// A bar's split button, over the middle of its inner side: pressed, it cuts the bar in half; dragged along the bar, the cut follows the pointer and lands where it is let go.
fn split_button(node: Node, frozen: RwSignal<bool>) -> Built {
    let edge = edge_of(&node).unwrap_or(Edge::Top);
    let middle = move |rect: Rect| match edge.is_vertical() {
        true => rect.y + rect.height / 2.0,
        false => rect.x + rect.width / 2.0,
    };
    let edit = Edit::new(telar::t!("editor.top.split", name = node.area.to_string()));
    let at = {
        let node = node.clone();
        move || inner_side(&node, middle)
    };
    let (pressed, taken, dragged) = (node.clone(), node.clone(), node);
    let previewing = edit.clone();
    Ok(Box::new(gesture::drag(
        host::hotspot(host::cut_mark(!edge.is_vertical())?, at)?
            .cursor(match edge.is_vertical() {
                true => Cursor::NsResize,
                false => Cursor::EwResize,
            })
            .on_press(move || {
                let Some(rect) = rects::rect(&pressed) else {
                    return;
                };
                let drawn = Drawn::of(
                    pressed.output.as_deref(),
                    pressed.layer,
                    &pressed.area,
                    edge,
                );
                said(top::split_at(&pressed, middle(rect), &drawn));
            }),
        edit.transaction(),
        move |_| {
            frozen.set(true);
            Some(Drawn::of(
                taken.output.as_deref(),
                taken.layer,
                &taken.area,
                edge,
            ))
        },
        move |drawn, _| {
            let (Some(point), Some(before), Some(desktop)) = (
                surfaces::menu::pointer(),
                previewing.transaction().before(),
                reconcile::desktop_now(dragged.output.as_deref()),
            ) else {
                return;
            };
            let along = match edge.is_vertical() {
                true => point.1,
                false => point.0,
            };
            let planned = top::split(
                &before,
                &desktop,
                dragged.layer,
                &dragged.area,
                along,
                drawn,
            );
            let _ = previewing.preview(planned.map(|(ops, _)| ops).unwrap_or_default());
        },
        move |_, _| frozen.set(false),
    )))
}

/// The button over the seam between the bar `first` names and the bar `second` after it on their edge, which joins them.
fn join_button(first: Node, second: &AreaId) -> Built {
    let edge = edge_of(&first).unwrap_or(Edge::Top);
    let next = Node::area(first.output.as_deref(), first.layer, second);
    let at = {
        let first = first.clone();
        move || {
            let next = rects::rect(&next)?;
            let seam = match edge.is_vertical() {
                true => next.y,
                false => next.x,
            };
            inner_side(&first, |rect| {
                let end = match edge.is_vertical() {
                    true => rect.y + rect.height,
                    false => rect.x + rect.width,
                };
                (end + seam) / 2.0
            })
        }
    };
    let second = second.clone();
    Ok(Box::new(
        host::hotspot(host::join_mark()?, at)?
            .cursor(Cursor::Pointer)
            .on_press(move || said(top::join(&first, &second))),
    ))
}

/// The line a carried chip would be put at, and the note that says what a chip carried off every bar would become.
fn marks() -> Built {
    let theme = use_theme::<NordTheme>();
    let list = ReactiveList::with_style(
        whole(),
        || aim().get().into_iter().collect(),
        |aimed: &Aim| format!("{aimed:?}"),
        move |aimed: Aim| -> Built {
            match aimed {
                Aim::Line(rect) => Ok(Box::new(
                    StyledContainer::new(
                        surfaces::area::at(rect),
                        move |_| RectStyle::filled(theme.accent, 1.0),
                        Vec::new(),
                    )?
                    .input_transparent(),
                )),
                Aim::Off((x, y)) => {
                    let said = || telar::t!("editor.top.taken_away");
                    Ok(Box::new(
                        StyledContainer::new(
                            LayoutStyle::new()
                                .absolute()
                                .inset_start(x + 16.0)
                                .inset_top(y + 16.0)
                                .padding_all(ui::scale::space::xs()),
                            move |_| {
                                RectStyle::filled(theme.surface, ui::scale::corner::xs())
                                    .with_border(Border::uniform(theme.accent, 1.0))
                            },
                            vec![crate::popover::rows::note(said)?],
                        )?
                        .input_transparent(),
                    ))
                }
            }
        },
    )?;
    see_through(list)
}
