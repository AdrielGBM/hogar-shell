//! Handles on the real item: points laid over the bar itself that are dragged to set its geometry, each the pointer's way to a value the popover also scrubs (WCAG 2.5.7).
//!
//! **Modifiers.** Shift ×10 and Alt ×0.1 are the shell's one step convention, and they scale what steps: a scrub and an arrow key on a focused handle. A handle *dragged* follows the pointer rather than stepping, so on a corner handle Alt means something else there alone: it isolates that corner, leaving the other three where they were, where a plain drag rounds all four together (TA-4). Arrow keys on a corner handle move that corner alone and keep Alt's ×0.1, since there is nothing to isolate it from; the popover scrubs each corner on its own and all four at once.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Children, Cursor, LayoutError, LayoutItem, Rect, RwSignal, Transaction, batch, effect,
};

use config::Edge;
use surfaces::rects::{self, Node};

use super::draft::AreaDraft;
use super::rows::Range;

/// How thick a bar or dock may be made.
pub const THICKNESS: Range = Range::whole(4.0, 256.0);
/// How far a bar may float off its edge.
pub const GAP: Range = Range::whole(0.0, 64.0);
/// The shortest a bar may be made along its edge.
pub const SHORTEST: f32 = 48.0;

/// The corners of a rectangle, clockwise from the top left, as the layout lists a radius per corner.
pub const CORNERS: [Corner; 4] = [
    Corner::TopLeft,
    Corner::TopRight,
    Corner::BottomRight,
    Corner::BottomLeft,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Corner {
    fn index(self) -> usize {
        match self {
            Corner::TopLeft => 0,
            Corner::TopRight => 1,
            Corner::BottomRight => 2,
            Corner::BottomLeft => 3,
        }
    }

    /// Which way the corner points, as `(x, y)` signs.
    fn signs(self) -> (f32, f32) {
        match self {
            Corner::TopLeft => (1.0, 1.0),
            Corner::TopRight => (-1.0, 1.0),
            Corner::BottomRight => (-1.0, -1.0),
            Corner::BottomLeft => (1.0, -1.0),
        }
    }

    fn at(self, rect: Rect) -> (f32, f32) {
        match self {
            Corner::TopLeft => (rect.x, rect.y),
            Corner::TopRight => (rect.x + rect.width, rect.y),
            Corner::BottomRight => (rect.x + rect.width, rect.y + rect.height),
            Corner::BottomLeft => (rect.x, rect.y + rect.height),
        }
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

    fn cursor(self) -> Cursor {
        match self {
            Corner::TopLeft | Corner::BottomRight => Cursor::NwseResize,
            Corner::TopRight | Corner::BottomLeft => Cursor::NeswResize,
        }
    }
}

/// The largest radius the area's corners take: half its short side, where two arcs meet.
pub fn most_radius(draft: &AreaDraft) -> f32 {
    let rect = draft.rect().unwrap_or_default();
    (rect.width.min(rect.height) / 2.0).floor().max(0.0)
}

/// All four corners at once: the largest of them, and a change to it rounds all four to it.
pub fn uniform_radius(draft: &AreaDraft, corners: [RwSignal<f32>; 4]) -> RwSignal<f32> {
    let largest = move || {
        corners
            .iter()
            .map(|corner| corner.get())
            .fold(0.0, f32::max)
    };
    let largest_now = move || {
        corners
            .iter()
            .map(|corner| corner.peek())
            .fold(0.0, f32::max)
    };
    let seed = largest_now();
    let all = draft.value("radius", || seed, |_, _| {});
    effect(move || {
        let wanted = all.get();
        if wanted == largest_now() {
            return;
        }
        batch(|| {
            for corner in corners {
                if corner.peek() != wanted {
                    corner.set(wanted);
                }
            }
        });
    });
    effect(move || {
        let most = largest();
        if all.peek() != most {
            all.set(most);
        }
    });
    all
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
            let handles = CORNERS
                .iter()
                .map(|corner| corner_handle(&built, *corner, corners, most as f32))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Box::new(crate::host::passthrough(crate::host::whole(), handles)?) as _)
        },
    )?;
    Ok(vec![Box::new(crate::host::passthrough(
        crate::host::whole(),
        vec![Box::new(rebuilt)],
    )?)])
}

/// One corner's handle. A drag rounds all four corners to where it is, and with Alt held only its own; letting go keeps it, and Esc or the other button puts all four back as they were.
pub fn corner_handle(
    draft: &AreaDraft,
    corner: Corner,
    corners: [RwSignal<f32>; 4],
    most: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let own = corner.index();
    let before: Rc<Cell<Option<[f32; 4]>>> = Rc::default();
    let transaction = Transaction::new(corners[own])
        .on_commit({
            let before = Rc::clone(&before);
            move |_, _| before.set(None)
        })
        .on_revert({
            let before = Rc::clone(&before);
            move |_| {
                if let Some(prior) = before.take() {
                    restore(corners, own, prior);
                }
            }
        });
    let node = draft.node.clone();
    let clamped = draft.value(clamp_name(corner), || false, |_, _| {});
    let to_value = {
        let node = node.clone();
        Rc::new(move |x: f32, y: f32| {
            let asked = corner.radius(rect_of(&node), (x, y));
            let prior = before.get().unwrap_or_else(|| {
                let now = corners.map(|value| value.peek());
                before.set(Some(now));
                now
            });
            let others = match telar::modifiers().is_alt {
                true => None,
                false => Some(asked.clamp(0.0, most)),
            };
            batch(|| {
                for (at, value) in corners.iter().enumerate().filter(|(at, _)| *at != own) {
                    let wanted = others.unwrap_or(prior[at]);
                    if value.peek() != wanted {
                        value.set(wanted);
                    }
                }
            });
            asked
        })
    };
    telar::handle(
        telar::HandleProps::props()
            .transaction(transaction)
            .to_value(to_value)
            .to_point(Rc::new(move |radius| corner.point(rect_of(&node), radius)))
            .min(0.0)
            .max(most)
            .cursor(corner.cursor())
            .clamped(clamped)
            .build(),
        Children::default(),
    )
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

fn restore(corners: [RwSignal<f32>; 4], own: usize, prior: [f32; 4]) {
    batch(|| {
        for (at, value) in corners.iter().enumerate().filter(|(at, _)| *at != own) {
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
    along_or_across(value, to_value, to_point, THICKNESS, across(edge))
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
    along_or_across(value, to_value, to_point, GAP, across(edge))
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
    value: RwSignal<f32>,
    to_value: Rc<dyn Fn(f32, f32) -> f32>,
    to_point: Rc<dyn Fn(f32) -> (f32, f32)>,
    range: Range,
    cursor: Cursor,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    telar::handle(
        telar::HandleProps::props()
            .value(value)
            .to_value(to_value)
            .to_point(to_point)
            .min(range.min)
            .max(range.max)
            .step(range.step)
            .cursor(cursor)
            .build(),
        Children::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 34.0,
    };

    /// A corner's handle sits over the centre of its arc, and a pointer there asks for the radius it is at, whichever corner it is.
    #[test]
    fn a_corner_handle_reads_back_the_radius_it_is_drawn_at() {
        for corner in CORNERS {
            for radius in [0.0, 6.0, 17.0] {
                let point = corner.point(BAR, radius);
                assert_eq!(corner.radius(BAR, point), radius, "{corner:?} at {radius}");
            }
        }
        assert_eq!(Corner::BottomRight.point(BAR, 8.0), (1912.0, 26.0));
    }
}
