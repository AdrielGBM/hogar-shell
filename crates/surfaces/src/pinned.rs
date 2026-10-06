//! A box pinned to one of the nine anchors of a bigger one, moved by an offset and never past its edges: a stack's column, and the launcher where a stack says it opens.

use telar::Rect;

use layout::{Anchor, Offset};

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

/// How much of its box a column keeps for its cards however far it is moved, as a share of the box's height: a column moved right up to the edge it grows away from would have no room left, and its cards would be drawn past that edge.
const ROOM: f32 = 0.25;

/// A stack's column inside `bounds`: `width` wide where its anchor pins it across, moved by `offset` and kept inside, and running from the end its cards grow from — a gap in from that edge, moved by `offset` — as far as the box allows the other way. A column pinned to the middle is as tall as it can be with its middle where it was moved to. However far it is moved, it keeps [`ROOM`] of the box for its cards.
pub fn column(bounds: Rect, anchor: Anchor, width: f32, offset: Offset) -> Rect {
    let (across, down) = sides(anchor);
    let x = along(bounds.x, bounds.width, width, across, offset.x);
    let (top, bottom) = (bounds.y, bounds.y + bounds.height);
    let room = bounds.height * ROOM;
    let (from, until) = match down {
        Side::Start => (
            (top + DEFAULT_GAP + offset.y).clamp(top, (bottom - room).max(top)),
            bottom - DEFAULT_GAP,
        ),
        Side::End => (
            top + DEFAULT_GAP,
            (bottom - DEFAULT_GAP + offset.y).clamp((top + room).min(bottom), bottom),
        ),
        Side::Middle => {
            let middle = (top + bounds.height / 2.0 + offset.y)
                .clamp(top + room, (bottom - room).max(top + room));
            let half = (middle - top - DEFAULT_GAP)
                .min(bottom - DEFAULT_GAP - middle)
                .max(0.0);
            (middle - half, middle + half)
        }
    };
    Rect::new(x, from, width, (until - from).max(0.0))
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
