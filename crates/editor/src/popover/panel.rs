use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Children, Cursor, LayoutError, LayoutItem, LayoutStyle, Reactive, ReactiveList, Rect, RwSignal,
    SizeDimension,
};

use config::Edge;
use layout::{AreaKind, InstanceId, ResolvedArea, ResolvedAreaKind, Within};
use surfaces::rects::{self, Node};

use super::Inspector;
use super::area::help;
use super::draft::{AreaDraft, Grip, kind_field};
use super::handles;
use super::rows::{self, Range, label};
use crate::modes::gesture;

pub(crate) const CELLS: Range = Range::whole(1.0, 24.0);
const CELL: Range = Range::whole(16.0, 400.0);
const BESIDE: &str = "beside";
const ALONG: &str = "along";

#[derive(Clone, Copy)]
struct Sizes {
    cols: RwSignal<f32>,
    rows: RwSignal<f32>,
    cell: RwSignal<f32>,
    gap: RwSignal<f32>,
}

impl Sizes {
    fn pitch(&self) -> f32 {
        (self.cell.peek() + self.gap.peek()).max(1.0)
    }
}

pub(crate) fn tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::Panel { owner, .. } = draft.resolved.kind.clone() else {
        return Ok(Inspector::default());
    };
    let edge = owners_bar(&draft.node, &owner);
    let along = draft.setting(
        "along",
        "along",
        |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::Panel { along: true, .. } => ALONG.to_string(),
            _ => BESIDE.to_string(),
        },
        |area, shape: &String| kind_field!(area, "panel", Panel { along }, shape == ALONG),
    );
    let sizes = Sizes {
        cols: count(draft, "cols"),
        rows: count(draft, "rows"),
        cell: draft.setting(
            "cell",
            "cell",
            |area: &ResolvedArea| match area.kind {
                ResolvedAreaKind::Panel { cell, .. } => cell,
                _ => AreaKind::CELL,
            },
            |area, value: &f32| kind_field!(area, "panel", Panel { cell }, *value),
        ),
        gap: draft.setting(
            "gap",
            "gap",
            |area: &ResolvedArea| match area.kind {
                ResolvedAreaKind::Panel { gap, .. } => gap,
                _ => AreaKind::GAP,
            },
            |area, value: &f32| kind_field!(area, "panel", Panel { gap }, *value),
        ),
    };
    let mut list: Vec<Box<dyn LayoutItem>> = Vec::new();
    if edge.is_some() {
        let options: Rc<[(String, String)]> = Rc::from(vec![
            (BESIDE.to_string(), telar::t!("editor.panel.beside")),
            (ALONG.to_string(), telar::t!("editor.panel.along")),
        ]);
        list.push(draft.marked(
            &["along"],
            rows::listed(
                label!("editor.panel.shape"),
                help("AreaKind::Panel", "along"),
                along,
                options,
            )?,
        )?);
    }
    list.push(count_rows(draft, along, edge, sizes)?);
    list.push(draft.marked(
        &["cell"],
        rows::number(
            label!("editor.area.cell"),
            help("AreaKind::Panel", "cell"),
            sizes.cell,
            CELL,
        )?,
    )?);
    list.push(draft.marked(
        &["gap"],
        rows::number(
            label!("editor.area.gap"),
            help("AreaKind::Panel", "gap"),
            sizes.gap,
            handles::GAP,
        )?,
    )?);
    Ok(Inspector {
        rows: list,
        handles: vec![resize_handle(draft, owner, along, edge, sizes)?],
    })
}

fn count(draft: &AreaDraft, key: &'static str) -> RwSignal<f32> {
    let cols = key == "cols";
    draft.setting(
        key,
        key,
        move |area: &ResolvedArea| match area.kind {
            ResolvedAreaKind::Panel { cols: across, .. } if cols => across as f32,
            ResolvedAreaKind::Panel { rows: down, .. } => down as f32,
            _ if cols => AreaKind::PANEL_COLS as f32,
            _ => AreaKind::PANEL_ROWS as f32,
        },
        move |area, value: &f32| {
            let cells = value.round().clamp(CELLS.min, CELLS.max) as u32;
            match cols {
                true => kind_field!(area, "panel", Panel { cols }, cells),
                false => kind_field!(area, "panel", Panel { rows }, cells),
            }
        },
    )
}

fn deep_cols(edge: Option<Edge>) -> bool {
    edge.is_some_and(Edge::is_vertical)
}

fn count_rows(
    draft: &AreaDraft,
    along: RwSignal<String>,
    edge: Option<Edge>,
    sizes: Sizes,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let built = draft.clone();
    Ok(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        move || vec![edge.is_some() && along.with(|shape| shape == ALONG)],
        |docked: &bool| *docked,
        move |docked: bool| {
            let row = |key: &'static str, said: Reactive<String>, value: RwSignal<f32>| {
                built.marked(
                    &[key],
                    rows::number(said, help("AreaKind::Panel", key), value, CELLS)?,
                )
            };
            let shown = match (docked, deep_cols(edge)) {
                (true, true) => vec![row("cols", label!("editor.panel.depth"), sizes.cols)?],
                (true, false) => vec![row("rows", label!("editor.panel.depth"), sizes.rows)?],
                (false, _) => vec![
                    row("cols", label!("editor.panel.cols"), sizes.cols)?,
                    row("rows", label!("editor.panel.rows"), sizes.rows)?,
                ],
            };
            Ok(Box::new(crate::host::passthrough(
                LayoutStyle::new()
                    .flex_column()
                    .gap(ui::scale::space::sm())
                    .width(SizeDimension::Percent(1.0)),
                shown,
            )?) as Box<dyn LayoutItem>)
        },
    )?))
}

fn owners_bar(node: &Node, owner: &layout::InstanceId) -> Option<Edge> {
    let desktop = surfaces::reconcile::desktop_now(node.output.as_deref())?;
    match surfaces::panel::owner_area(desktop.resolved.layer(node.layer)?, owner)?.kind {
        ResolvedAreaKind::Bar { edge, .. } => Some(edge),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grows {
    pub(crate) right: bool,
    pub(crate) down: bool,
    pub(crate) across: f32,
    pub(crate) docked: Option<Edge>,
}

pub(crate) fn grows(edge: Option<Edge>, docked: bool) -> Grows {
    let beside = |right: bool, down: bool, across: f32| Grows {
        right,
        down,
        across,
        docked: None,
    };
    match (edge, docked) {
        (Some(edge), true) => Grows {
            right: edge != Edge::Right,
            down: edge != Edge::Bottom,
            across: 1.0,
            docked: Some(edge),
        },
        (Some(Edge::Bottom), _) => beside(true, false, 2.0),
        (Some(Edge::Left), _) => beside(true, true, 1.0),
        (Some(Edge::Right), _) => beside(false, true, 1.0),
        _ => beside(true, true, 2.0),
    }
}

/// Which way the panel `node` grows as its handle is pulled: away from the bar `bar` its owner is on, or, off a bar, away from the side of its owner `owner` it opens on ([`surfaces::transient::hanging`]).
pub(crate) fn growing(node: &Node, owner: &InstanceId, bar: Option<Edge>, docked: bool) -> Grows {
    grows(bar.or_else(|| opens_off(node, owner)), docked)
}

fn opens_off(node: &Node, owner: &InstanceId) -> Option<Edge> {
    let output = node.output.as_deref();
    let (_, owner) = rects::instance(output, owner)?;
    let panel = rects::rect(node)?;
    let usable = surfaces::reconcile::with_desktop_now(output, |desktop| {
        desktop.reserved.box_of(Within::Usable, desktop.size)
    })?;
    Some(surfaces::transient::hanging(owner, usable, panel.height))
}

pub(crate) fn handle_at(rect: Rect, grows: Grows) -> (f32, f32) {
    let x = match grows.right {
        true => rect.x + rect.width,
        false => rect.x,
    };
    let y = match grows.down {
        true => rect.y + rect.height,
        false => rect.y,
    };
    match grows.docked {
        Some(edge) if edge.is_vertical() => (x, rect.y + rect.height / 2.0),
        Some(_) => (rect.x + rect.width / 2.0, y),
        None => (x, y),
    }
}

#[derive(Clone, Copy)]
struct Grab {
    at: (f32, f32),
    grows: Grows,
    cols: f32,
    rows: f32,
}

fn resize_handle(
    draft: &AreaDraft,
    owner: InstanceId,
    along: RwSignal<String>,
    edge: Option<Edge>,
    sizes: Sizes,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let node = draft.node.clone();
    let grip = draft.grip();
    Ok(Box::new(ReactiveList::with_style(
        crate::host::whole(),
        move || vec![edge.is_some() && along.with(|shape| shape == ALONG)],
        |docked: &bool| *docked,
        move |docked: bool| {
            let (held, owner) = (node.clone(), owner.clone());
            let growing = Rc::new(move || growing(&held, &owner, edge, docked));
            let (own, other) = match !docked || deep_cols(edge) {
                true => (sizes.cols, sizes.rows),
                false => (sizes.rows, sizes.cols),
            };
            handle(node.clone(), growing, (own, other), sizes, grip.clone())
        },
    )?))
}

fn handle(
    node: Node,
    growing: Rc<dyn Fn() -> Grows>,
    (own, other): (RwSignal<f32>, RwSignal<f32>),
    sizes: Sizes,
    grip: Grip,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let grab: Rc<Cell<Option<Grab>>> = Rc::default();
    let now = growing();
    let cursor = match now.docked {
        Some(edge) if edge.is_vertical() => Cursor::EwResize,
        Some(_) => Cursor::NsResize,
        None if now.right == now.down => Cursor::NwseResize,
        None => Cursor::NeswResize,
    };
    let to_point = {
        let (grab, growing) = (Rc::clone(&grab), Rc::clone(&growing));
        move |_| {
            let grows = grab.get().map_or_else(|| growing(), |held| held.grows);
            handle_at(rects::rect(&node).unwrap_or_default(), grows)
        }
    };
    let on_start = gesture::holding(grip.clone());
    let on_end = {
        let (grab, let_go) = (Rc::clone(&grab), gesture::letting_go(grip));
        move |kept: bool| {
            grab.set(None);
            let_go(kept);
        }
    };
    let to_value = move |x: f32, y: f32| {
        let held = grab.get().unwrap_or_else(|| Grab {
            at: (x, y),
            grows: growing(),
            cols: sizes.cols.peek(),
            rows: sizes.rows.peek(),
        });
        grab.set(Some(held));
        let grows = held.grows;
        let pitch = sizes.pitch();
        let sign = |grows: bool| if grows { 1.0 } else { -1.0 };
        let dx = (x - held.at.0) * sign(grows.right) * grows.across / pitch;
        let dy = (y - held.at.1) * sign(grows.down) / pitch;
        let stepped = |from: f32, by: f32| (from + by).round().clamp(CELLS.min, CELLS.max);
        match grows.docked {
            Some(edge) if edge.is_vertical() => stepped(held.cols, dx),
            Some(_) => stepped(held.rows, dy),
            None => {
                let rows = stepped(held.rows, dy);
                if other.peek() != rows {
                    other.set(rows);
                }
                stepped(held.cols, dx)
            }
        }
    };
    telar::handle(
        telar::HandleProps::props()
            .value(own)
            .to_value(Rc::new(to_value))
            .to_point(Rc::new(to_point))
            .min(CELLS.min)
            .max(CELLS.max)
            .cursor(cursor)
            .on_start(on_start)
            .on_end(Rc::new(on_end))
            .build(),
        Children::default(),
    )
}
