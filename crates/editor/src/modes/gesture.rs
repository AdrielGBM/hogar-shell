//! What the pointer targets of every mode share: a press that selects, a secondary press that opens a menu, a drag that previews one edit from its first move to its end, and the targets a drag started on held still under its own preview.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{PointerButton, RwSignal, StyledContainer, Transaction};

use layout::LayoutOp;
use surfaces::rects::Node;

use crate::session::Edit;

/// How far the pointer travels before a press becomes a drag, so a press on a target that also drags still only presses.
pub(crate) const THRESHOLD: f32 = 4.0;

/// A list that answers `now` until `frozen` is set and then keeps answering what it answered last, so the targets a gesture started on are not rebuilt under it by its own preview.
pub(crate) fn held<T: Clone + 'static>(
    frozen: RwSignal<bool>,
    now: impl Fn() -> Vec<T> + 'static,
) -> impl Fn() -> Vec<T> + 'static {
    let last: Rc<RefCell<Vec<T>>> = Rc::default();
    move || {
        if frozen.get() {
            return last.borrow().clone();
        }
        let listed = now();
        *last.borrow_mut() = listed.clone();
        listed
    }
}

/// `target` answering a press by selecting what is under the pointer, as the selection tool's own boxes do, and a secondary press by opening its menu there. `node` is anything on the screen and layer the target is over.
pub(crate) fn pressable(
    target: StyledContainer,
    node: impl Fn() -> Option<Node> + Clone + 'static,
) -> StyledContainer {
    let asked = node.clone();
    target
        .on_press(move || {
            if let (Some(node), Some(point)) = (node(), surfaces::menu::pointer()) {
                crate::select::press_at(&node, point);
            }
        })
        .on_alt_press(move |button| {
            if button != PointerButton::Secondary {
                return;
            }
            if let (Some(node), Some(point)) = (asked(), surfaces::menu::pointer()) {
                crate::select::menu_at(&node, point);
            }
        })
}

/// `target` dragged through `transaction` once the pointer travels past [`THRESHOLD`]. The first move asks `take` what the drag holds on to — what it read when it began, so its own preview cannot move what it aims at — and every move, the first included, hands that to `follow` with where the pointer is in the target's own coordinates. Whether the drag is let go or cancelled, what it held goes to `release`, told which (`true` for let go), and the drag holds nothing after.
pub(crate) fn drag<T: 'static, X: Clone + 'static>(
    target: StyledContainer,
    transaction: Transaction<X>,
    take: impl Fn((f32, f32)) -> Option<T> + 'static,
    follow: impl Fn(&T, (f32, f32)) + 'static,
    release: impl Fn(Option<T>, bool) + 'static,
) -> StyledContainer {
    let holding: Rc<RefCell<Option<T>>> = Rc::default();
    let (ending, cancelling) = (Rc::clone(&holding), Rc::clone(&holding));
    let release = Rc::new(release);
    let cancelled = Rc::clone(&release);
    target
        .drag_threshold(THRESHOLD)
        .drag_transaction(transaction)
        .on_drag(move |x, y| {
            if holding.borrow().is_none() {
                let taken = take((x, y));
                *holding.borrow_mut() = taken;
            }
            if let Some(held) = holding.borrow().as_ref() {
                follow(held, (x, y));
            }
        })
        .on_drag_end(move |_, _| release(ending.borrow_mut().take(), true))
        .on_drag_cancel(move || cancelled(cancelling.borrow_mut().take(), false))
}

/// Previews what a drag would do where the pointer is now, and marks it in `aim`: `planned` and what marks it, or — where nothing is planned there, or the layout refuses it — the layout as it was before the drag, marked nowhere.
pub(crate) fn aimed<A: Clone + PartialEq + 'static>(
    edit: &Edit,
    planned: Option<(Vec<LayoutOp>, A)>,
    aim: RwSignal<Option<A>>,
) {
    let shown = planned.and_then(|(ops, aimed)| edit.preview(ops).ok().map(|()| aimed));
    if shown.is_none() {
        let _ = edit.preview(Vec::new());
    }
    aim.set(shown);
}
