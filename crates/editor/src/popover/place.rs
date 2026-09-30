//! Where a popover's card goes: beside what it customizes, on the side that item faces, and always inside what the screen's reserving areas leave.

use telar::Rect;

use config::Edge;

/// How far the card stands off what it customizes and off the edges of the usable area.
pub const GAP: f32 = 8.0;

/// The top-left corner of a `card` sized box for an item at `item`: across from the edge a bar or dock hangs off, else below the item, else above, else to its right or left, and over it where nothing else fits. Always inside `usable`, as far as the card is not bigger than it.
pub fn card_at(item: Rect, edge: Option<Edge>, card: (f32, f32), usable: Rect) -> (f32, f32) {
    let (width, height) = card;
    let centred = item.x + item.width / 2.0 - width / 2.0;
    let below = (centred, item.y + item.height + GAP);
    let above = (centred, item.y - GAP - height);
    let right = (item.x + item.width + GAP, item.y);
    let left = (item.x - GAP - width, item.y);
    let room_across_y =
        |(_, y): (f32, f32)| y >= usable.y + GAP && y + height <= usable.y + usable.height - GAP;
    let room_across_x =
        |(x, _): (f32, f32)| x >= usable.x + GAP && x + width <= usable.x + usable.width - GAP;
    let wanted = match edge {
        Some(Edge::Top) => below,
        Some(Edge::Bottom) => above,
        Some(Edge::Left) => right,
        Some(Edge::Right) => left,
        None => [below, above]
            .into_iter()
            .find(|at| room_across_y(*at))
            .or_else(|| [right, left].into_iter().find(|at| room_across_x(*at)))
            .unwrap_or(below),
    };
    clamped(wanted, card, usable)
}

/// `at` moved just far enough to keep the card inside `usable`.
fn clamped(at: (f32, f32), card: (f32, f32), usable: Rect) -> (f32, f32) {
    let within = |at: f32, start: f32, length: f32, extent: f32| {
        let far = (start + length - extent - GAP).max(start + GAP);
        at.clamp(start + GAP, far)
    };
    let x = within(at.0, usable.x, usable.width, card.0);
    let y = within(at.1, usable.y, usable.height, card.1);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USABLE: Rect = Rect {
        x: 0.0,
        y: 34.0,
        width: 1920.0,
        height: 1046.0,
    };
    const CARD: (f32, f32) = (320.0, 400.0);

    fn inside(at: (f32, f32)) -> bool {
        at.0 >= USABLE.x
            && at.1 >= USABLE.y
            && at.0 + CARD.0 <= USABLE.x + USABLE.width
            && at.1 + CARD.1 <= USABLE.y + USABLE.height
    }

    /// Whatever it customizes and wherever that is, the card is on screen — a bar on any edge, a chip in any corner, a region the size of the screen.
    #[test]
    fn the_card_never_leaves_the_usable_area() {
        let items = [
            (Rect::new(0.0, 0.0, 1920.0, 34.0), Some(Edge::Top)),
            (Rect::new(0.0, 1046.0, 1920.0, 34.0), Some(Edge::Bottom)),
            (Rect::new(0.0, 34.0, 40.0, 1046.0), Some(Edge::Left)),
            (Rect::new(1880.0, 34.0, 40.0, 1046.0), Some(Edge::Right)),
            (Rect::new(1890.0, 0.0, 30.0, 34.0), Some(Edge::Top)),
            (Rect::new(1890.0, 1046.0, 30.0, 34.0), Some(Edge::Bottom)),
            (Rect::new(0.0, 0.0, 1920.0, 1080.0), None),
            (Rect::new(1700.0, 900.0, 200.0, 160.0), None),
        ];
        for (item, edge) in items {
            let at = card_at(item, edge, CARD, USABLE);
            assert!(inside(at), "{item:?} on {edge:?}: {at:?}");
        }
    }

    /// A card for a bar sits on the side of it the screen is on, centred on it where there is room.
    #[test]
    fn a_bars_card_hangs_off_the_side_it_faces() {
        let top = Rect::new(0.0, 0.0, 1920.0, 34.0);
        assert_eq!(card_at(top, Some(Edge::Top), CARD, USABLE), (800.0, 42.0));
        let bottom = Rect::new(0.0, 1046.0, 1920.0, 34.0);
        assert_eq!(
            card_at(bottom, Some(Edge::Bottom), CARD, USABLE).1,
            1046.0 - GAP - CARD.1
        );
    }
}
