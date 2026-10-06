//! Rectangles placed by hand, handled on the screen itself: a free area, a texture, a grid or the lock's prompt.
//!
//! **Corners.** The selected one has a handle on each corner that resizes it from there, the opposite corner staying put, and no side shorter than [`SMALLEST`] — [`SMALLEST_PROMPT`] for the prompt, which also stays wholly on its screen. The edges a corner drags snap to the siblings in its box and to the box's start, middle and end ([`snap`]), with the lines drawn and its size said beside the pointer as a share of its box.
//!
//! **Bodies.** A texture's body carries it, snapping as a whole and kept on its box, as the lock does with its prompt. A free area's body pins what it holds to the ninth of it the pointer is over, and the selected free area shows its nine anchors to press.
//!
//! **Every handle has a row and a key (WCAG 2.5.7).** The rectangle and the anchor are rows of the area's popover, and Shift+arrows and Ctrl+arrows move and resize the rectangle a step at a time ([`crate::steps`]). Hold Alt to drag without snapping.

use telar::{Border, Cursor, LayoutStyle, ReactiveList, RectStyle, StyledContainer, use_theme};

use config::theme::NordTheme;
use layout::{
    Anchor, AreaKind, LayerKind, Layout, LayoutOp, Rect, ResolvedAreaKind, SMALLEST_PROMPT,
};
use surfaces::reconcile;
use surfaces::rects::Node;
use ui::descriptor::Built;

use crate::context;
use crate::host::{self, passthrough, see_through, whole};
use crate::mode::{self, Mode, said};
use crate::popover::handles::{CORNERS, Corner};
use crate::popover::{AreaDraft, kind_field};
use crate::session::{self, Edit, EditError, Selection};
use crate::snap::{self, Motion, Moving, Snapped};
use crate::written::Written;

use super::gesture::{self, Hint, pressable};
use super::{lock, overlay};

const HANDLE: f32 = 14.0;
/// The shortest a side of a resized rectangle gets, as a share of its box.
pub const SMALLEST: f32 = 0.02;
/// The kinds of area whose rectangle the corners resize.
const CORNERED: [&str; 4] = ["free", "texture", "grid", "prompt"];

/// Lays the bodies of textures and free areas over the selection tool of every session layer's mode and under its own tools, so a split button over a texture still takes its press. The lock lays them itself, over its own selection tool.
pub(crate) fn install_bodies() {
    for layer in LayerKind::SESSION {
        host::add_tool(layer, bodies);
    }
}

/// Lays the corners and anchors of the selection over every other tool of every layer's mode.
pub(crate) fn install() {
    for layer in LayerKind::ALL {
        host::add_tool(layer, corners);
    }
}

/// What dragging `corner` does to the rectangle each way.
fn moving(corner: Corner) -> Moving {
    let motion = |start: bool| match start {
        true => Some(Motion::Start),
        false => Some(Motion::End),
    };
    Moving {
        x: motion(corner.is_left()),
        y: motion(corner.is_top()),
    }
}

/// `rect` with its `corner` dragged to `to`, in fractions of its box and kept on it: the opposite corner stays put, and neither side gets shorter than `smallest`.
pub fn resized(rect: Rect, corner: Corner, to: (f32, f32), smallest: f32) -> Rect {
    let (to_x, to_y) = (to.0.clamp(0.0, 1.0), to.1.clamp(0.0, 1.0));
    let (mut left, mut top, mut right, mut bottom) =
        (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
    match corner.is_left() {
        true => left = to_x.min(right - smallest),
        false => right = to_x.max(left + smallest),
    }
    match corner.is_top() {
        true => top = to_y.min(bottom - smallest),
        false => bottom = to_y.max(top + smallest),
    }
    Rect {
        x: left,
        y: top,
        w: right - left,
        h: bottom - top,
    }
}

/// What a rectangle's size tag says: its width and height as a share of its box.
pub fn size_said(rect: Rect) -> String {
    telar::t!(
        "editor.rect.size",
        w = (rect.w * 100.0).round(),
        h = (rect.h * 100.0).round()
    )
}

/// Where a drag took hold of an area placed by a rectangle: the pointer at the press, the area's rectangle as the layout gives it, the box that rectangle is a fraction of, in pixels, and what it snaps to there.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    pub(crate) node: Node,
    pub(crate) kind: &'static str,
    pub(crate) from: (f32, f32),
    pub(crate) rect: Rect,
    pub(crate) bounds: telar::Rect,
    pub(crate) siblings: Vec<Rect>,
}

impl Held {
    /// The area `node` is on, taken hold of at `from`; `None` for an area no rectangle places.
    pub(crate) fn of(node: &Node, from: (f32, f32)) -> Option<Self> {
        let desktop = reconcile::desktop_now(node.output.as_deref())?;
        let layer = desktop.resolved.layer(node.layer)?;
        let area = layer.areas.iter().find(|area| area.id == node.area)?;
        Some(Self {
            node: Node::area(node.output.as_deref(), node.layer, &node.area),
            kind: area.kind.name(),
            from,
            rect: area.kind.rect()?,
            bounds: desktop.reserved.box_of(area.within, desktop.size),
            siblings: snap::area_siblings(layer, &node.area),
        })
    }

    fn size(&self) -> (f32, f32) {
        (self.bounds.width.max(1.0), self.bounds.height.max(1.0))
    }

    /// How far the pointer at `point` has gone from the press, as a share of the box.
    fn travel(&self, point: (f32, f32)) -> (f32, f32) {
        let size = self.size();
        (
            (point.0 - self.from.0) / size.0,
            (point.1 - self.from.1) / size.1,
        )
    }

    fn smallest(&self) -> f32 {
        match self.kind {
            "prompt" => SMALLEST_PROMPT,
            _ => SMALLEST,
        }
    }

    /// Where the rectangle is carried whole with the pointer at `point`, snapped unless `free` and kept on its box.
    pub(crate) fn carried(&self, point: (f32, f32), free: bool) -> Snapped {
        let (by_x, by_y) = self.travel(point);
        let carried = Rect {
            x: self.rect.x + by_x,
            y: self.rect.y + by_y,
            ..self.rect
        };
        let mut snapped = snap::snap(carried, &self.siblings, self.size(), Moving::BODY, free);
        snapped.rect = snapped.rect.kept_on_output(0.0);
        snapped
    }

    /// The rectangle with its `corner` dragged by the pointer's travel to `point`, the edges it drags snapped unless `free` or where snapping would make a side too short.
    pub(crate) fn resized(&self, corner: Corner, point: (f32, f32), free: bool) -> Snapped {
        let (by_x, by_y) = self.travel(point);
        let (x, y) = corner.of((self.rect.x, self.rect.y, self.rect.w, self.rect.h));
        let smallest = self.smallest();
        let rect = resized(self.rect, corner, (x + by_x, y + by_y), smallest);
        let snapped = snap::snap(rect, &self.siblings, self.size(), moving(corner), free);
        let short =
            snapped.rect.w < smallest - f32::EPSILON || snapped.rect.h < smallest - f32::EPSILON;
        match short {
            true => Snapped {
                rect,
                guides: Vec::new(),
            },
            false => snapped,
        }
    }

    fn drawn(&self) -> telar::Rect {
        surfaces::area::within(self.rect, self.bounds)
    }
}

/// The operations that place the area `node` names, of the kind `kind`, at `to` — written where the layout decides it, and for the prompt kept wholly on its screen and no smaller than a prompt may be.
pub(crate) fn placed_at(
    layout: &Layout,
    node: &Node,
    kind: &'static str,
    to: Rect,
) -> Result<Vec<LayoutOp>, EditError> {
    if kind == "prompt" {
        return lock::moved_to(layout, node, to);
    }
    let written = written(layout, node)?;
    let mut area = written.area.clone();
    if let Some(
        AreaKind::Grid { rect, .. } | AreaKind::Texture { rect, .. } | AreaKind::Free { rect, .. },
    ) = AreaDraft::kind_mut(&mut area, kind)
    {
        *rect = Some(to);
    }
    Ok(written.ops(&area))
}

pub(crate) fn anchored(
    layout: &Layout,
    node: &Node,
    anchor: Anchor,
) -> Result<Vec<LayoutOp>, EditError> {
    let written = written(layout, node)?;
    let mut area = written.area.clone();
    kind_field!(&mut area, "free", Free { anchor }, anchor);
    Ok(written.ops(&area))
}

fn written(layout: &Layout, node: &Node) -> Result<Written, EditError> {
    Written::area(
        layout,
        node.output.as_deref(),
        node.layer,
        &node.area,
        crate::variant::editing_for(node).as_ref(),
    )
    .map_err(EditError::Refused)
}

/// Previews `held` placed where `snapped` says, with its guides and `tag` at the pointer at `point`; the strip says why where the layout refuses it.
pub(crate) fn previewed(
    edit: &Edit,
    held: &Held,
    snapped: Snapped,
    point: (f32, f32),
    tag: Option<String>,
) {
    gesture::hint().set(Some(Hint {
        pointer: point,
        tag,
        guides: snap::lines(&snapped.guides, held.bounds),
        ghost: None,
    }));
    let Some(before) = edit.transaction().before() else {
        return;
    };
    let placed =
        placed_at(&before, &held.node, held.kind, snapped.rect).and_then(|ops| edit.preview(ops));
    if let Err(why) = placed {
        mode::refuse(why);
    }
}

/// Where the area `node` names is on screen now, in pixels, and its kind.
fn drawn(node: &Node) -> Option<(telar::Rect, ResolvedAreaKind)> {
    let desktop = reconcile::desktop(node.output.as_deref())?;
    let area = desktop.resolved.area(node.layer, &node.area)?;
    let bounds = desktop.reserved.box_of(area.within, desktop.size);
    Some((
        surfaces::area::within(area.kind.rect()?, bounds),
        area.kind.clone(),
    ))
}

fn drawn_rect(node: &Node) -> Option<telar::Rect> {
    drawn(node).map(|(rect, _)| rect)
}

fn carried_areas(output: &str, layer: LayerKind) -> Vec<(Node, &'static str)> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    let Some(held) = desktop.resolved.layer(layer) else {
        return Vec::new();
    };
    held.areas
        .iter()
        .filter(|area| {
            matches!(
                area.kind,
                ResolvedAreaKind::Texture { .. } | ResolvedAreaKind::Free { .. }
            )
        })
        .map(|area| (Node::area(Some(output), layer, &area.id), area.kind.name()))
        .collect()
}

/// A box over every texture and free area of the edited layer, dragged by its body: a texture is carried, a free area pins what it holds.
pub(crate) fn bodies(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let list = ReactiveList::with_style(
        whole(),
        move || carried_areas(&output, layer),
        |body: &(Node, &'static str)| body.clone(),
        |(node, kind): (Node, &'static str)| match kind {
            "free" => free_body(node),
            _ => texture_body(node),
        },
    )?;
    Ok(Box::new(passthrough(whole(), vec![see_through(list)?])?))
}

/// A box over `node` that selects as every target does and, where `takes_hold` lets a drag take hold of it, drags as `follow` says.
fn body(
    node: Node,
    edit: &Edit,
    cursor: Cursor,
    takes_hold: fn(&Node) -> bool,
    follow: impl Fn(&Held, (f32, f32)) + 'static,
) -> Built {
    let (pressing, placing, taking) = (node.clone(), node.clone(), node);
    Ok(Box::new(gesture::drag(
        pressable(
            StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?,
            move || Some(pressing.clone()),
        )
        .styled_by(move || surfaces::area::at(drawn_rect(&placing).unwrap_or_default()))
        .cursor(cursor),
        edit.transaction(),
        move |pressed| {
            if !takes_hold(&taking) {
                return None;
            }
            let held = Held::of(&taking, pressed)?;
            session::select(Selection::Area(held.node.clone()));
            Some(held)
        },
        move |held: &Held, _| {
            if let Some(point) = surfaces::menu::pointer() {
                follow(held, point);
            }
        },
        |_, _| {},
    )))
}

fn texture_body(node: Node) -> Built {
    let label = telar::t!("editor.rect.moved", name = node.area.to_string());
    carried_body(node, label)
}

/// The body of the area `node` names: dragged, it carries the area, snapping as a whole and kept on its box, as one undo entry called `label`.
pub(crate) fn carried_body(node: Node, label: String) -> Built {
    let edit = Edit::new(label);
    let previewing = edit.clone();
    body(
        node,
        &edit,
        Cursor::Grab,
        |_| true,
        move |held, point| {
            previewed(
                &previewing,
                held,
                held.carried(point, snap::free()),
                point,
                None,
            );
        },
    )
}

/// A free area's body: dragged while it or something in it is selected, what it holds is pinned to the ninth of it the pointer is over. A free area often covers the whole screen, so a drag across it that picked nothing first moves nothing.
fn free_body(node: Node) -> Built {
    let edit = Edit::new(telar::t!(
        "editor.rect.anchored",
        name = node.area.to_string()
    ));
    let previewing = edit.clone();
    body(
        node,
        &edit,
        Cursor::Move,
        selected_in,
        move |held, point| {
            let anchor = overlay::anchor_at(held.drawn(), point);
            gesture::hint().set(Some(Hint {
                pointer: point,
                tag: Some(overlay::anchor_name(anchor)),
                ..Hint::default()
            }));
            let Some(before) = previewing.transaction().before() else {
                return;
            };
            let pinned =
                anchored(&before, &held.node, anchor).and_then(|ops| previewing.preview(ops));
            if let Err(why) = pinned {
                mode::refuse(why);
            }
        },
    )
}

fn selected_in(node: &Node) -> bool {
    session::selected().node().is_some_and(|selected| {
        selected.output == node.output && selected.layer == node.layer && selected.area == node.area
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Handle {
    Corner(Node, Corner),
    Anchors(Node),
}

/// The handles over the selection where it is an area of `layer` on the screen `output` that a rectangle places: its four corners, and for a free area its nine anchors.
fn handles(output: &str, layer: LayerKind) -> Vec<Handle> {
    if crate::tools::active().is_some() {
        return Vec::new();
    }
    let Selection::Area(node) = session::selection().get() else {
        return Vec::new();
    };
    if node.layer != layer || node.output.as_deref() != Some(output) {
        return Vec::new();
    }
    let Some((_, kind)) = drawn(&node) else {
        return Vec::new();
    };
    if !CORNERED.contains(&kind.name()) {
        return Vec::new();
    }
    let mut handles: Vec<Handle> = CORNERS
        .into_iter()
        .map(|corner| Handle::Corner(node.clone(), corner))
        .collect();
    if matches!(kind, ResolvedAreaKind::Free { .. }) {
        handles.push(Handle::Anchors(node));
    }
    handles
}

/// The corners of the selected rectangle and the anchors of a selected free area, over every other tool.
pub(crate) fn corners(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let list = ReactiveList::with_style(
        whole(),
        move || handles(&output, layer),
        |handle: &Handle| handle.clone(),
        |handle: Handle| match handle {
            Handle::Corner(node, corner) => corner_handle(node, corner),
            Handle::Anchors(node) => anchor_dots(node),
        },
    )?;
    Ok(Box::new(passthrough(whole(), vec![see_through(list)?])?))
}

/// The handle on one corner of the selected rectangle: dragged, it resizes the rectangle from there.
fn corner_handle(node: Node, corner: Corner) -> Built {
    let theme = use_theme::<NordTheme>();
    let edit = Edit::new(telar::t!(
        "editor.rect.resized",
        name = node.area.to_string()
    ));
    let previewing = edit.clone();
    let (placing, taking) = (node.clone(), node);
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| {
                RectStyle::filled(theme.surface, ui::scale::corner::xs())
                    .with_border(Border::uniform(theme.accent, 2.0))
            },
            Vec::new(),
        )?
        .styled_by(move || {
            let (x, y) = drawn_rect(&placing)
                .map(|rect| corner.at(rect))
                .unwrap_or((-2.0 * HANDLE, -2.0 * HANDLE));
            surfaces::area::at(telar::Rect::new(
                x - HANDLE / 2.0,
                y - HANDLE / 2.0,
                HANDLE,
                HANDLE,
            ))
        })
        .cursor(corner.cursor())
        .input_opaque(),
        edit.transaction(),
        move |pressed| Held::of(&taking, pressed),
        move |held: &Held, _| {
            let Some(point) = surfaces::menu::pointer() else {
                return;
            };
            let snapped = held.resized(corner, point, snap::free());
            let tag = size_said(snapped.rect);
            previewed(&previewing, held, snapped, point, Some(tag));
        },
        |_, _| {},
    )))
}

/// The nine anchors of the selected free area, the one what it holds is pinned to filled: pressing one pins it there.
fn anchor_dots(node: Node) -> Built {
    let (reading, choosing) = (node.clone(), node.clone());
    overlay::anchor_dots(
        move || drawn_rect(&reading),
        move || match drawn(&choosing) {
            Some((_, ResolvedAreaKind::Free { anchor, .. })) => Some(anchor),
            _ => None,
        },
        move |anchor| said(pin(&node, anchor)),
    )
}

/// Pins what the free area `node` names holds to `anchor`, as one undo entry.
pub(crate) fn pin(node: &Node, anchor: Anchor) -> Result<(), EditError> {
    crate::popover::close();
    let ops = anchored(&session::draft().peek(), node, anchor)?;
    context::commit(
        telar::t!(
            "editor.rect.pinned",
            name = node.area.to_string(),
            anchor = overlay::anchor_name(anchor)
        ),
        ops,
    )
}
