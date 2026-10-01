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
