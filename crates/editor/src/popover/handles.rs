//! Handles on the real item: points laid over it that are dragged to set its geometry, each the pointer's way to a value the popover also scrubs (WCAG 2.5.7).
//!
//! **Modifiers.** Shift ×10 and Alt ×0.1 are the shell's one step convention, and they scale what steps: a scrub and an arrow key on a focused handle. On a corner or side handle Alt also isolates: while the four are linked a drag or an arrow moves all four together, and with Alt held only the one it is on, leaving the other three where they were; unlinked (`u`), each moves only its own. A dragged handle follows the pointer rather than stepping, so there Alt only isolates; an arrow with Alt both isolates and steps a tenth. The popover scrubs each corner on its own and all four at once.

use std::cell::Cell;
use std::rc::Rc;

use telar::{Children, Color, Cursor, LayoutError, LayoutItem, Rect, RwSignal, Transaction, batch};

use config::Edge;
use surfaces::rects::{self, Node};

use super::draft::AreaDraft;
use super::rows::Range;
use crate::modes::gesture::{self, HandleDragging, Hint};

/// How thick a bar or dock may be made.
pub const THICKNESS: Range = Range::whole(4.0, 256.0);
/// Any gap a row or a handle sets: between a grid's cells, between a container's children, or between a bar and its edge.
pub const GAP: Range = Range::whole(0.0, 64.0);
/// The shortest a bar may be made along its edge.
pub const SHORTEST: f32 = 48.0;
/// How near its corner a corner handle is ever drawn, so one at a radius of 0 is still taken hold of apart from the corner and from the handles beside it.
pub const NEAREST: f32 = 12.0;
/// How far apart the centres of two corner handles on one side are kept, so a short side never lays one over the other.
pub const APART: f32 = 18.0;
/// What share of [`NEAREST`] from its corner a corner handle is pulled within to square the corner off.
pub const SQUARING: f32 = 0.6;
/// How much of a box's half short side its padding leaves free, so what it holds always keeps some room.
pub const ROOM_LEFT: f32 = 4.0;

/// The corners of a rectangle, clockwise from the top left, as the layout lists a radius per corner.
pub const CORNERS: [Corner; 4] = [
    Corner::TopLeft,
    Corner::TopRight,
    Corner::BottomRight,
    Corner::BottomLeft,
];

/// The sides of a rectangle, clockwise from the top, as the layout lists a padding per side.
pub const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Corner {
    pub fn index(self) -> usize {
        match self {
            Corner::TopLeft => 0,
            Corner::TopRight => 1,
            Corner::BottomRight => 2,
            Corner::BottomLeft => 3,
        }
    }

    /// Which way the corner points, as `(x, y)` signs.
    pub fn signs(self) -> (f32, f32) {
        match self {
            Corner::TopLeft => (1.0, 1.0),
            Corner::TopRight => (-1.0, 1.0),
            Corner::BottomRight => (-1.0, -1.0),
            Corner::BottomLeft => (1.0, -1.0),
        }
    }

    pub fn is_left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }

    pub fn at(self, rect: Rect) -> (f32, f32) {
        self.of((rect.x, rect.y, rect.width, rect.height))
    }

    pub fn of(self, (x, y, width, height): (f32, f32, f32, f32)) -> (f32, f32) {
        (
            if self.is_left() { x } else { x + width },
            if self.is_top() { y } else { y + height },
        )
    }

    /// Where the handle for a radius of `radius` sits: that far in from the corner along both edges, over the centre of the arc it rounds.
    pub fn point(self, rect: Rect, radius: f32) -> (f32, f32) {
        let (x, y) = self.at(rect);
        let (sx, sy) = self.signs();
        (x + sx * radius, y + sy * radius)
    }

    /// The radius a pointer at `(x, y)` asks for: how far in from the corner it is, along both edges on average.
    pub fn radius(self, rect: Rect, (x, y): (f32, f32)) -> f32 {
        let (cx, cy) = self.at(rect);
        let (sx, sy) = self.signs();
        ((x - cx) * sx + (y - cy) * sy) / 2.0
    }

    pub fn cursor(self) -> Cursor {
        match self {
            Corner::TopLeft | Corner::BottomRight => Cursor::NwseResize,
            Corner::TopRight | Corner::BottomLeft => Cursor::NeswResize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub fn index(self) -> usize {
        match self {
            Side::Top => 0,
            Side::Right => 1,
            Side::Bottom => 2,
            Side::Left => 3,
        }
    }

    /// Where the handle for a padding of `width` sits: on the inner edge of the padding, halfway along the side.
    pub fn point(self, rect: Rect, width: f32) -> (f32, f32) {
        let (cx, cy) = centre(rect);
        match self {
            Side::Top => (cx, rect.y + width),
            Side::Right => (rect.x + rect.width - width, cy),
            Side::Bottom => (cx, rect.y + rect.height - width),
            Side::Left => (rect.x + width, cy),
        }
    }

    /// The padding a pointer at `(x, y)` asks for: how far in from the side it is.
    pub fn width(self, rect: Rect, (x, y): (f32, f32)) -> f32 {
        match self {
            Side::Top => y - rect.y,
            Side::Right => rect.x + rect.width - x,
            Side::Bottom => rect.y + rect.height - y,
            Side::Left => x - rect.x,
        }
    }

    fn cursor(self) -> Cursor {
        match self {
            Side::Top | Side::Bottom => Cursor::NsResize,
            Side::Left | Side::Right => Cursor::EwResize,
        }
    }
}

/// The largest radius a box `rect` big takes: half its short side, where two arcs meet.
pub fn most_radius_in(rect: Rect) -> f32 {
    (rect.width.min(rect.height) / 2.0).floor().max(0.0)
}

/// The largest padding a box `rect` big takes: half its short side, less the [`ROOM_LEFT`] it keeps for what it holds.
pub fn most_padding_in(rect: Rect) -> f32 {
    (rect.width.min(rect.height) / 2.0 - ROOM_LEFT)
        .floor()
        .max(0.0)
}

/// The largest radius the area's corners take: half its short side, where two arcs meet.
pub fn most_radius(draft: &AreaDraft) -> f32 {
    most_radius_in(draft.rect().unwrap_or_default())
}

/// The four values a box's corner or side handles drag, clockwise from the top left corner or the top side, and the most any of them may be.
#[derive(Clone, Copy)]
pub struct Four {
    pub values: [RwSignal<f32>; 4],
    pub most: f32,
}

/// What a corner or side handle tells whoever drives its edit: a drag taking hold, every move of a drag with all four values as it leaves them, a drag let go, each arrow pressed on it with all four as the arrow leaves them, and a drag dropped, once it has put back what it moved besides its own value. And the colour it is drawn in, transparent for the theme's accent.
#[derive(Clone)]
pub struct Ends {
    pub grabbed: Rc<dyn Fn()>,
    pub moved: Rc<dyn Fn([f32; 4])>,
    pub kept: Rc<dyn Fn()>,
    pub stepped: Rc<dyn Fn([f32; 4])>,
    pub dropped: Rc<dyn Fn()>,
    pub color: Color,
}

impl Default for Ends {
    fn default() -> Self {
        Self {
            grabbed: Rc::new(|| {}),
            moved: Rc::new(|_| {}),
            kept: Rc::new(|| {}),
            stepped: Rc::new(|_| {}),
            dropped: Rc::new(|| {}),
            color: Color::TRANSPARENT,
        }
    }
}

/// The values a bar's handles set, each also a row of its popover.
pub struct BarValues {
    pub edge: Edge,
    pub thickness: RwSignal<f32>,
    pub length: RwSignal<f32>,
    pub offset: RwSignal<f32>,
    pub gap: RwSignal<f32>,
    pub corners: [RwSignal<f32>; 4],
}

/// Every handle a bar has: its four corners, how thick it is, where it starts and ends along its edge, and how far it floats off that edge.
pub fn bar(draft: &AreaDraft, values: BarValues) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let at_open = draft.rect().unwrap_or_default();
    let edge = values.edge;
    let mut handles = corners(draft, values.corners)?;
    handles.push(thickness(draft, edge, values.thickness)?);
    handles.push(offset(draft, edge, values.offset, at_open)?);
    handles.push(length(draft, edge, values.length)?);
    handles.push(gap(draft, edge, values.gap, at_open)?);
    Ok(handles)
}

/// The four corner handles, rebuilt whenever the short side they are clamped to changes — a thickness handle dragged, the bar turned to another edge.
fn corners(
    draft: &AreaDraft,
    corners: [RwSignal<f32>; 4],
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let built = draft.clone();
    let rebuilt = telar::ReactiveList::with_style(
        crate::host::whole(),
        {
            let draft = draft.clone();
            move || vec![most_radius(&draft) as u32]
        },
        |most: &u32| *most,
        move |most: u32| {
            let four = Four {
                values: corners,
                most: most as f32,
            };
            let handles = CORNERS
                .iter()
                .map(|corner| {
                    let clamped = built.value(clamp_name(*corner), || false, |_, _| {});
                    corner_handle(&built.node, *corner, four, clamped, gripped(&built))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Box::new(crate::host::passthrough(crate::host::whole(), handles)?) as _)
        },
    )?;
    Ok(vec![Box::new(crate::host::passthrough(
        crate::host::whole(),
        vec![Box::new(rebuilt)],
    )?)])
}

fn gripped(draft: &AreaDraft) -> Ends {
    let grip = draft.grip();
    let (holding, keeping, dropping) = (grip.clone(), grip.clone(), grip);
    Ends {
        grabbed: Rc::new(move || holding.hold()),
        kept: Rc::new(move || keeping.release()),
        dropped: Rc::new(move || dropping.put_back()),
        ..Ends::default()
    }
}

/// One corner's handle over the box `node` is drawn at: a drag rounds all four corners to where it is while they are linked, and only its own when they are not or Alt is held, and one pulled near enough the corner squares it off. Letting go keeps it, and Esc or the other button puts all four back as they were.
pub fn corner_handle(
    node: &Node,
    corner: Corner,
    four: Four,
    clamped: RwSignal<bool>,
    ends: Ends,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let shown_from = NEAREST.min(four.most);
    let (values, most) = (four.values, four.most);
    let own = corner.index();
    four_handle(
        node,
        own,
        four,
        Geometry {
            to_value: Rc::new(move |rect, at| {
                let radii = values.map(|value| value.peek());
                let moving = CornerDrag {
                    rect,
                    corner,
                    radii,
                    shown_from,
                    together: !isolated(),
                };
                squared(moving.asked(at, most)).round()
            }),
            to_point: Rc::new(move |rect, radius| {
                let mut radii = values.map(|value| value.get());
                radii[own] = radius;
                handle_point(rect, corner, radii, shown_from)
            }),
            cursor: corner.cursor(),
            limit: |most| telar::t!("editor.tool.radius_most", most = most),
        },
        clamped,
        ends,
    )
}

/// Where the handle of `corner` is drawn when the four are rounded by `radii`: over the centre of its arc, but never nearer its corner than `shown_from`, and slid along its own edge where it would lie over the handle at the other end of a short side. Of two that would meet, the bottom one slides in along the bottom edge and the right one down the right edge.
pub fn handle_point(rect: Rect, corner: Corner, radii: [f32; 4], shown_from: f32) -> (f32, f32) {
    let shown = radii.map(|radius| radius.max(shown_from));
    let (across, along) = match corner {
        Corner::TopLeft => (3, 1),
        Corner::TopRight => (2, 0),
        Corner::BottomRight => (1, 3),
        Corner::BottomLeft => (0, 2),
    };
    let own = shown[corner.index()];
    let (x, y) = corner.point(rect, own);
    let (sx, sy) = corner.signs();
    let down = parted(rect.height, own, shown[across]);
    let over = parted(rect.width, own, shown[along]);
    (
        x + if sy < 0.0 { sx * down } else { 0.0 },
        y + if sx < 0.0 { sy * over } else { 0.0 },
    )
}

/// A corner handle being dragged on a box: where the pointer must be for the radius it asks, which moves the handle itself where sides are short.
struct CornerDrag {
    rect: Rect,
    corner: Corner,
    radii: [f32; 4],
    shown_from: f32,
    together: bool,
}

impl CornerDrag {
    /// How far the handle is drawn off where it would rest, once the radius is `radius`.
    fn slid(&self, radius: f32) -> (f32, f32) {
        let own = self.corner.index();
        let mut radii = self.radii;
        match self.together {
            true => radii = [radius; 4],
            false => radii[own] = radius,
        }
        let (x, y) = handle_point(self.rect, self.corner, radii, self.shown_from);
        let resting = self.corner.point(self.rect, radius.max(self.shown_from));
        (x - resting.0, y - resting.1)
    }

    /// The radius whose handle is drawn at `at`, as near as it gets between none and `most`: how far the handle lies off the radius it is asked for is itself a function of that radius, so the one that agrees with the pointer is found by halving.
    fn asked(&self, at: (f32, f32), most: f32) -> f32 {
        let lacking = |radius: f32| {
            let (dx, dy) = self.slid(radius);
            self.corner.radius(self.rect, (at.0 - dx, at.1 - dy)) - radius
        };
        let (mut low, mut high) = (0.0, most);
        if lacking(low) <= 0.0 {
            return low;
        }
        let beyond = lacking(high);
        if beyond >= 0.0 {
            return high + beyond;
        }
        for _ in 0..BISECTIONS {
            let middle = (low + high) / 2.0;
            match lacking(middle) > 0.0 {
                true => low = middle,
                false => high = middle,
            }
        }
        (low + high) / 2.0
    }
}

const BISECTIONS: usize = 16;

/// How far two handles `first` and `second` in from the two ends of a side `length` long must slide apart, perpendicular to it, to be [`APART`] from one another.
fn parted(length: f32, first: f32, second: f32) -> f32 {
    (APART - (length - first - second).abs()).max(0.0)
}

/// One side's handle over the box `node` is drawn at, on the inner edge of its padding: a drag pads all four sides to where it is while they are linked, and only its own when they are not or Alt is held.
pub fn side_handle(
    node: &Node,
    side: Side,
    four: Four,
    clamped: RwSignal<bool>,
    ends: Ends,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    four_handle(
        node,
        side.index(),
        four,
        Geometry {
            to_value: Rc::new(move |rect, at| side.width(rect, at).round()),
            to_point: Rc::new(move |rect, width| side.point(rect, width)),
            cursor: side.cursor(),
            limit: |most| telar::t!("editor.tool.padding_most", most = most),
        },
        clamped,
        ends,
    )
}

/// The radius a corner dragged `asked` in from its corner takes: none at all once it is pulled within [`SQUARING`] of [`NEAREST`] of the corner.
pub fn squared(asked: f32) -> f32 {
    match asked < NEAREST * SQUARING {
        true => 0.0,
        false => asked,
    }
}

fn isolated() -> bool {
    !crate::tools::linked_now() || telar::modifiers().is_alt
}

/// Where a corner or side handle reads its value from and draws it at, in the box it is over, and what it says when asked for more than the box takes.
struct Geometry {
    to_value: Rc<dyn Fn(Rect, (f32, f32)) -> f32>,
    to_point: Rc<dyn Fn(Rect, f32) -> (f32, f32)>,
    cursor: Cursor,
    limit: fn(f32) -> String,
}

/// Where a drag took hold of a handle, what the four were then, and whether the pointer has since travelled far enough from there to move anything.
#[derive(Clone, Copy)]
struct Grab {
    prior: [f32; 4],
    at: (f32, f32),
    travelled: bool,
}

fn four_handle(
    node: &Node,
    own: usize,
    four: Four,
    geometry: Geometry,
    clamped: RwSignal<bool>,
    ends: Ends,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Four { values, most } = four;
    let grab: Rc<Cell<Option<Grab>>> = Rc::default();
    let dragging = HandleDragging::new();
    let (kept_end, dropped_end) = (dragging.on_end_fn(), dragging.on_end_fn());
    let transaction = Transaction::new(values[own])
        .on_commit({
            let (grab, kept, stepped) = (
                Rc::clone(&grab),
                Rc::clone(&ends.kept),
                Rc::clone(&ends.stepped),
            );
            move |_, _| {
                kept_end();
                match grab.take() {
                    Some(_) => kept(),
                    None => stepped(followed(values, own)),
                }
            }
        })
        .on_revert({
            let (grab, dropped) = (Rc::clone(&grab), Rc::clone(&ends.dropped));
            move |_| {
                if let Some(held) = grab.take() {
                    restore(values, own, held.prior);
                }
                dropped_end();
                dropped();
            }
        });
    let reading = node.clone();
    let (to_value, limit, moved, grabbed) =
        (geometry.to_value, geometry.limit, ends.moved, ends.grabbed);
    let to_value = dragging.wrap_to_value(move |x, y| {
        let mut held = grab.get().unwrap_or_else(|| {
            grabbed();
            Grab {
                prior: values.map(|value| value.peek()),
                at: (x, y),
                travelled: false,
            }
        });
        held.travelled |= (x - held.at.0).hypot(y - held.at.1) >= gesture::THRESHOLD;
        grab.set(Some(held));
        let prior = held.prior;
        if !held.travelled {
            return prior[own];
        }
        let asked = to_value(rect_of(&reading), (x, y));
        let kept = asked.clamp(0.0, most);
        let now = match isolated() {
            true => {
                let mut now = prior;
                now[own] = kept;
                now
            }
            false => [kept; 4],
        };
        batch(|| {
            for (at, value) in values.iter().enumerate().filter(|(at, _)| *at != own) {
                if value.peek() != now[at] {
                    value.set(now[at]);
                }
            }
        });
        say_limit((x, y), (asked > most).then(|| limit(most)));
        moved(now);
        asked
    });
    let placing = node.clone();
    let to_point = geometry.to_point;
    let color = ends.color;
    telar::handle(
        telar::HandleProps::props()
            .transaction(transaction)
            .to_value(Rc::new(to_value))
            .to_point(Rc::new(move |value| to_point(rect_of(&placing), value)))
            .min(0.0)
            .max(most)
            .cursor(geometry.cursor)
            .clamped(clamped)
            .color(color)
            .build(),
        Children::default(),
    )
}

/// Says at `pointer` why the drag stops short of where it is, or stops saying it.
fn say_limit(pointer: (f32, f32), said: Option<String>) {
    let wanted = said.map(|tag| Hint {
        pointer,
        tag: Some(tag),
        ..Hint::default()
    });
    let hint = gesture::hint();
    if hint.peek() != wanted {
        hint.set(wanted);
    }
}

/// The name a corner's clamped state is shared by, for whatever shows it besides the handle itself.
pub fn clamp_name(corner: Corner) -> &'static str {
    match corner {
        Corner::TopLeft => "radius.top_left.clamped",
        Corner::TopRight => "radius.top_right.clamped",
        Corner::BottomRight => "radius.bottom_right.clamped",
        Corner::BottomLeft => "radius.bottom_left.clamped",
    }
}

/// A commit no drag opened is an arrow on the focused handle, which telar steps on its own value alone; while the four are linked and Alt is not held the other three follow it, as they do a drag.
fn followed(values: [RwSignal<f32>; 4], own: usize) -> [f32; 4] {
    let now = values[own].peek();
    if !isolated() {
        batch(|| {
            for value in values {
                if value.peek() != now {
                    value.set(now);
                }
            }
        });
    }
    values.map(|value| value.peek())
}

fn restore(values: [RwSignal<f32>; 4], own: usize, prior: [f32; 4]) {
    batch(|| {
        for (at, value) in values.iter().enumerate().filter(|(at, _)| *at != own) {
            if value.peek() != prior[at] {
                value.set(prior[at]);
            }
        }
    });
}

fn rect_of(node: &Node) -> Rect {
    rects::rect(node).unwrap_or_default()
}

fn centre(rect: Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

/// On the bar's inner edge, dragged across it.
fn thickness(
    draft: &AreaDraft,
    edge: Edge,
    value: RwSignal<f32>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let node = draft.node.clone();
    let reading = node.clone();
    let to_value = Rc::new(move |x: f32, y: f32| {
        let rect = rect_of(&reading);
        match edge {
            Edge::Top => y - rect.y,
            Edge::Bottom => rect.y + rect.height - y,
            Edge::Left => x - rect.x,
            Edge::Right => rect.x + rect.width - x,
        }
    });
    let to_point = Rc::new(move |thickness: f32| {
        let rect = rect_of(&node);
        let (cx, cy) = centre(rect);
        match edge {
            Edge::Top => (cx, rect.y + thickness),
            Edge::Bottom => (cx, rect.y + rect.height - thickness),
            Edge::Left => (rect.x + thickness, cy),
            Edge::Right => (rect.x + rect.width - thickness, cy),
        }
    });
    along_or_across(draft, value, to_value, to_point, THICKNESS, across(edge))
}

/// On the end of the bar its offset is measured from, dragged along its edge: the whole bar slides.
fn offset(
    draft: &AreaDraft,
    edge: Edge,
    value: RwSignal<f32>,
    at_open: Rect,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let origin = match edge.is_vertical() {
        true => at_open.y - value.peek(),
        false => at_open.x - value.peek(),
    };
    let node = draft.node.clone();
    let to_value = Rc::new(move |x: f32, y: f32| match edge.is_vertical() {
        true => y - origin,
        false => x - origin,
    });
    let to_point = Rc::new(move |offset: f32| {
        let (cx, cy) = centre(rect_of(&node));
        match edge.is_vertical() {
            true => (cx, origin + offset),
            false => (origin + offset, cy),
        }
    });
    let longest = match edge.is_vertical() {
        true => draft.screen.1,
        false => draft.screen.0,
    };
    along_or_across(
        draft,
        value,
        to_value,
        to_point,
        Range::whole(0.0, longest),
        along(edge),
    )
}

/// On the far end of the bar, dragged along its edge: the bar runs as far as it is dragged.
fn length(
    draft: &AreaDraft,
    edge: Edge,
    value: RwSignal<f32>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let node = draft.node.clone();
    let reading = node.clone();
    let to_value = Rc::new(move |x: f32, y: f32| {
        let rect = rect_of(&reading);
        match edge.is_vertical() {
            true => y - rect.y,
            false => x - rect.x,
        }
    });
    let to_point = Rc::new(move |length: f32| {
        let rect = rect_of(&node);
        let (cx, cy) = centre(rect);
        match edge.is_vertical() {
            true => (cx, rect.y + length),
            false => (rect.x + length, cy),
        }
    });
    let longest = match edge.is_vertical() {
        true => draft.screen.1,
        false => draft.screen.0,
    };
    along_or_across(
        draft,
        value,
        to_value,
        to_point,
        Range::whole(SHORTEST, longest),
        along(edge),
    )
}

/// On the bar's outer edge, dragged away from the screen's edge: how far the bar floats off it.
fn gap(
    draft: &AreaDraft,
    edge: Edge,
    value: RwSignal<f32>,
    at_open: Rect,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let floated = value.peek();
    let origin = match edge {
        Edge::Top => at_open.y - floated,
        Edge::Bottom => at_open.y + at_open.height + floated,
        Edge::Left => at_open.x - floated,
        Edge::Right => at_open.x + at_open.width + floated,
    };
    let node = draft.node.clone();
    let to_value = Rc::new(move |x: f32, y: f32| match edge {
        Edge::Top => y - origin,
        Edge::Bottom => origin - y,
        Edge::Left => x - origin,
        Edge::Right => origin - x,
    });
    let to_point = Rc::new(move |gap: f32| {
        let (cx, cy) = centre(rect_of(&node));
        match edge {
            Edge::Top => (cx, origin + gap),
            Edge::Bottom => (cx, origin - gap),
            Edge::Left => (origin + gap, cy),
            Edge::Right => (origin - gap, cy),
        }
    });
    along_or_across(draft, value, to_value, to_point, GAP, across(edge))
}

fn across(edge: Edge) -> Cursor {
    match edge.is_vertical() {
        true => Cursor::EwResize,
        false => Cursor::NsResize,
    }
}

fn along(edge: Edge) -> Cursor {
    match edge.is_vertical() {
        true => Cursor::NsResize,
        false => Cursor::EwResize,
    }
}

fn along_or_across(
    draft: &AreaDraft,
    value: RwSignal<f32>,
    to_value: Rc<dyn Fn(f32, f32) -> f32>,
    to_point: Rc<dyn Fn(f32) -> (f32, f32)>,
    range: Range,
    cursor: Cursor,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let grip = draft.grip();
    let (keeping, dropping) = (grip.clone(), grip.clone());
    let transaction = Transaction::new(value)
        .on_commit(move |_, _| keeping.release())
        .on_revert(move |_| dropping.put_back());
    telar::handle(
        telar::HandleProps::props()
            .transaction(transaction)
            .to_value(Rc::new(move |x, y| {
                grip.hold();
                to_value(x, y)
            }))
            .to_point(to_point)
            .min(range.min)
            .max(range.max)
            .step(range.step)
            .cursor(cursor)
            .build(),
        Children::default(),
    )
}
