//! A box pinned to one of the nine anchors of a bigger one, moved by an offset and never past its edges: a stack's lane — the column or row its cards run along — and the launcher where a stack says it opens.

use telar::{AlignItems, JustifyContent, LayoutStyle, Rect};

use layout::{Anchor, Offset, StackFlow};

use crate::transient::DEFAULT_GAP;

/// Where along one axis an anchor pins a box: at the start of it, in the middle, or at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Start,
    Middle,
    End,
}

/// The side `anchor` pins a box to across, then down.
pub fn sides(anchor: Anchor) -> (Side, Side) {
    let across = match anchor {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => Side::Start,
        Anchor::Top | Anchor::Center | Anchor::Bottom => Side::Middle,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => Side::End,
    };
    let down = match anchor {
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight => Side::Start,
        Anchor::Left | Anchor::Center | Anchor::Right => Side::Middle,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => Side::End,
    };
    (across, down)
}

/// The anchor pinning a box to `across`, then `down`.
pub fn anchor_of(across: Side, down: Side) -> Anchor {
    match (down, across) {
        (Side::Start, Side::Start) => Anchor::TopLeft,
        (Side::Start, Side::Middle) => Anchor::Top,
        (Side::Start, Side::End) => Anchor::TopRight,
        (Side::Middle, Side::Start) => Anchor::Left,
        (Side::Middle, Side::Middle) => Anchor::Center,
        (Side::Middle, Side::End) => Anchor::Right,
        (Side::End, Side::Start) => Anchor::BottomLeft,
        (Side::End, Side::Middle) => Anchor::Bottom,
        (Side::End, Side::End) => Anchor::BottomRight,
    }
}

/// The anchor of the ninth of `bounds` that `point` is over, or of the ninth nearest it from outside.
pub fn anchor_at(bounds: Rect, point: (f32, f32)) -> Anchor {
    let third = |start: f32, length: f32, at: f32| match (at - start) / length.max(1.0) {
        part if part < 1.0 / 3.0 => Side::Start,
        part if part < 2.0 / 3.0 => Side::Middle,
        _ => Side::End,
    };
    anchor_of(
        third(bounds.x, bounds.width, point.0),
        third(bounds.y, bounds.height, point.1),
    )
}

/// Where a box `length` long starts along a run from `start` that is `run` long: a gap in from the side it is pinned to, moved by `by`, and kept on the run.
fn along(start: f32, run: f32, length: f32, side: Side, by: f32) -> f32 {
    let natural = match side {
        Side::Start => start + DEFAULT_GAP,
        Side::Middle => start + (run - length) / 2.0,
        Side::End => start + run - length - DEFAULT_GAP,
    };
    (natural + by).clamp(start, (start + run - length).max(start))
}

/// A box `size` big pinned to `anchor` inside `bounds`, moved by `offset`: the launcher where a stack says it opens.
pub fn pinned(bounds: Rect, anchor: Anchor, size: (f32, f32), offset: Offset) -> Rect {
    let (across, down) = sides(anchor);
    Rect::new(
        along(bounds.x, bounds.width, size.0, across, offset.x),
        along(bounds.y, bounds.height, size.1, down, offset.y),
        size.0,
        size.1,
    )
}

/// Where `anchor` pins a point inside `bounds`: a gap in from the edges it names, and in the middle of the others.
pub fn point(bounds: Rect, anchor: Anchor) -> (f32, f32) {
    let at = pinned(bounds, anchor, (0.0, 0.0), Offset::ZERO);
    (at.x, at.y)
}

/// How much of its box a stack keeps for its cards however far it is moved, as a share of the box along the run its cards grow along: a stack moved right up to the edge it grows away from would have no room left, and its cards would be drawn past that edge.
const ROOM: f32 = 0.25;

/// The stretch of a run `length` long from `start` that cards are laid along when pinned to `side`: from a gap in from that edge, moved by `by`, as far as the run allows the other way. Pinned to the middle it is as long as it can be with its middle where `by` moved it to. However far it is moved, it keeps [`ROOM`] of the run.
fn stretch(start: f32, length: f32, side: Side, by: f32) -> (f32, f32) {
    let (low, high) = (start, start + length);
    let room = length * ROOM;
    match side {
        Side::Start => (
            (low + DEFAULT_GAP + by).clamp(low, (high - room).max(low)),
            high - DEFAULT_GAP,
        ),
        Side::End => (
            low + DEFAULT_GAP,
            (high - DEFAULT_GAP + by).clamp((low + room).min(high), high),
        ),
        Side::Middle => {
            let middle = (low + length / 2.0 + by).clamp(low + room, (high - room).max(low + room));
            let half = (middle - low - DEFAULT_GAP)
                .min(high - DEFAULT_GAP - middle)
                .max(0.0);
            (middle - half, middle + half)
        }
    }
}

/// A stack's column inside `bounds`: `width` wide where its anchor pins it across, moved by `offset` and kept inside, and running from the end its cards grow from — a gap in from that edge, moved by `offset` — as far as the box allows the other way. A column pinned to the middle is as tall as it can be with its middle where it was moved to. However far it is moved, it keeps [`ROOM`] of the box for its cards.
pub fn column(bounds: Rect, anchor: Anchor, width: f32, offset: Offset) -> Rect {
    let (across, down) = sides(anchor);
    let x = along(bounds.x, bounds.width, width, across, offset.x);
    let (from, until) = stretch(bounds.y, bounds.height, down, offset.y);
    Rect::new(x, from, width, (until - from).max(0.0))
}

/// A stack's row inside `bounds`: running from the side its cards grow from, a gap in from that edge and moved by `offset`, as far as the box allows the other way, and as tall as the box allows with its cards pinned to the edge its anchor names down the box. A row pinned to the middle grows both ways from where it was moved to. However far it is moved, it keeps [`ROOM`] of the box for its cards.
pub fn row(bounds: Rect, anchor: Anchor, offset: Offset) -> Rect {
    let (across, down) = sides(anchor);
    let (left, right) = stretch(bounds.x, bounds.width, across, offset.x);
    let (top, bottom) = stretch(bounds.y, bounds.height, down, offset.y);
    Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
}

/// The side of its box a stack of `flow` pinned to `anchor` grows its cards from: the top or bottom a column is pinned to, the left or right a row is.
pub fn grows_from(anchor: Anchor, flow: StackFlow) -> Side {
    let (across, down) = sides(anchor);
    match flow {
        StackFlow::Column => down,
        StackFlow::Row => across,
    }
}

/// The side a stack of `flow` pinned to `anchor` keeps its cards to across the way they grow: the left or right a column is pinned to, the top or bottom a row is.
pub fn keeps_to(anchor: Anchor, flow: StackFlow) -> Side {
    let (across, down) = sides(anchor);
    match flow {
        StackFlow::Column => across,
        StackFlow::Row => down,
    }
}

/// The lane of a stack of `flow` inside `bounds`: [`column`] or [`row`].
pub fn stack(bounds: Rect, anchor: Anchor, width: f32, flow: StackFlow, offset: Offset) -> Rect {
    match flow {
        StackFlow::Column => column(bounds, anchor, width, offset),
        StackFlow::Row => row(bounds, anchor, offset),
    }
}

/// `style` laying its children out one after another the way `flow` runs.
pub fn flowing(style: LayoutStyle, flow: StackFlow) -> LayoutStyle {
    match flow {
        StackFlow::Column => style.flex_column(),
        StackFlow::Row => style.flex_row(),
    }
}

/// `style` as the lane of a stack of `flow` pinned to `anchor`: its cards run the way `flow` says from the side they grow from, and a row keeps them to the edge its anchor names across it. A column's cards fill its width, which is the stack's.
pub fn lane_style(style: LayoutStyle, anchor: Anchor, flow: StackFlow) -> LayoutStyle {
    let lane = flowing(style, flow).justify_content(match grows_from(anchor, flow) {
        Side::Start => JustifyContent::START,
        Side::Middle => JustifyContent::CENTER,
        Side::End => JustifyContent::END,
    });
    match flow {
        StackFlow::Column => lane,
        StackFlow::Row => lane.align_items(match keeps_to(anchor, flow) {
            Side::Start => AlignItems::START,
            Side::Middle => AlignItems::CENTER,
            Side::End => AlignItems::END,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    #[test]
    fn every_anchor_reads_back_as_its_two_sides() {
        for anchor in Anchor::ALL {
            let (across, down) = sides(anchor);
            assert_eq!(anchor_of(across, down), anchor);
        }
    }

    /// A column sits a gap in from the edges its anchor names and runs the rest of the way; an offset moves it, and never past the screen's edge.
    #[test]
    fn a_column_is_pinned_to_its_anchor_and_moved_by_its_offset_inside_the_screen() {
        let top_right = column(SCREEN, Anchor::TopRight, 380.0, Offset::ZERO);
        assert_eq!(top_right, Rect::new(1532.0, 8.0, 380.0, 1064.0));
        let moved = column(
            SCREEN,
            Anchor::TopRight,
            380.0,
            Offset { x: -100.0, y: 40.0 },
        );
        assert_eq!(moved, Rect::new(1432.0, 48.0, 380.0, 1024.0));
        let pushed_out = column(
            SCREEN,
            Anchor::TopRight,
            380.0,
            Offset {
                x: 500.0,
                y: -500.0,
            },
        );
        assert_eq!(
            (pushed_out.x, pushed_out.y),
            (1540.0, 0.0),
            "kept on screen"
        );

        let bottom = column(SCREEN, Anchor::Bottom, 400.0, Offset { x: 0.0, y: -60.0 });
        assert_eq!(bottom, Rect::new(760.0, 8.0, 400.0, 1004.0));

        let centre = column(SCREEN, Anchor::Center, 480.0, Offset { x: 0.0, y: 100.0 });
        assert_eq!(centre.x, 720.0);
        assert_eq!(
            centre.y + centre.height / 2.0,
            640.0,
            "its middle moved by the offset"
        );
        assert_eq!(
            centre.y + centre.height,
            1072.0,
            "as tall as the gap at the bottom allows"
        );
    }

    /// A row grows from the side its anchor names across: a left anchor grows right, a right one grows left, a middle one both ways. A column grows from the side it names down.
    #[test]
    fn a_stack_grows_from_the_side_its_anchor_names_along_its_flow() {
        for anchor in Anchor::ALL {
            let (across, down) = sides(anchor);
            assert_eq!(grows_from(anchor, StackFlow::Row), across, "{anchor:?}");
            assert_eq!(grows_from(anchor, StackFlow::Column), down, "{anchor:?}");
            assert_eq!(keeps_to(anchor, StackFlow::Row), down, "{anchor:?}");
            assert_eq!(keeps_to(anchor, StackFlow::Column), across, "{anchor:?}");
        }
    }

    /// A row starts a gap in from the edge it grows from and runs the rest of the way, from the middle it is a run centred on where it is moved to, and an offset moves it without taking it past the screen.
    #[test]
    fn a_row_runs_from_its_anchors_side_and_is_kept_inside_the_screen() {
        let left = row(SCREEN, Anchor::BottomLeft, Offset::ZERO);
        assert_eq!(left, Rect::new(8.0, 8.0, 1904.0, 1064.0));
        let from_right = row(SCREEN, Anchor::BottomRight, Offset::ZERO);
        assert_eq!(
            (from_right.x, from_right.x + from_right.width),
            (8.0, 1912.0)
        );
        let moved = row(SCREEN, Anchor::TopLeft, Offset { x: 100.0, y: 40.0 });
        assert_eq!(moved, Rect::new(108.0, 48.0, 1804.0, 1024.0));
        let pushed_out = row(
            SCREEN,
            Anchor::TopLeft,
            Offset {
                x: 5000.0,
                y: -500.0,
            },
        );
        assert_eq!(
            (pushed_out.x, pushed_out.y),
            (1440.0, 0.0),
            "kept on screen"
        );

        let centre = row(SCREEN, Anchor::Bottom, Offset { x: 100.0, y: 0.0 });
        assert_eq!(
            centre.x + centre.width / 2.0,
            1060.0,
            "centred on where it was moved"
        );
        assert_eq!(
            stack(SCREEN, Anchor::Top, 380.0, StackFlow::Row, Offset::ZERO),
            row(SCREEN, Anchor::Top, Offset::ZERO)
        );
        assert_eq!(
            stack(SCREEN, Anchor::Top, 380.0, StackFlow::Column, Offset::ZERO),
            column(SCREEN, Anchor::Top, 380.0, Offset::ZERO)
        );
    }

    #[test]
    fn a_pinned_box_stays_inside_what_it_is_pinned_in() {
        let corner = pinned(SCREEN, Anchor::BottomLeft, (600.0, 400.0), Offset::ZERO);
        assert_eq!(corner, Rect::new(8.0, 672.0, 600.0, 400.0));
        let centred = pinned(
            SCREEN,
            Anchor::Center,
            (600.0, 400.0),
            Offset { x: 0.0, y: -40.0 },
        );
        assert_eq!(centred, Rect::new(660.0, 300.0, 600.0, 400.0));
        let past = pinned(
            SCREEN,
            Anchor::BottomLeft,
            (600.0, 400.0),
            Offset { x: -50.0, y: 50.0 },
        );
        assert_eq!((past.x, past.y), (0.0, 680.0));
    }
}
