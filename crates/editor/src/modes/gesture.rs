//! What the pointer targets of every mode share: a press that selects, a secondary press that opens a menu, a drag that previews one edit from its first move to its end and says at the pointer what it would do, and the targets a drag started on held still under its own preview.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use telar::{
    Component, Event, EventResult, LayoutItem, NodeId, PointerButton, Rect, RenderNode, RwSignal,
    StyledContainer, Transaction, detached, signal,
};

use layout::LayoutOp;
use surfaces::rects::Node;

use crate::session::Edit;

/// How far the pointer travels before a press becomes a drag, so a press on a target that also drags still only presses.
pub(crate) const THRESHOLD: f32 = 4.0;

/// What a drag shows at the pointer as it goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hint {
    pub(crate) pointer: (f32, f32),
    /// Where what it carries would land, said beside the pointer.
    pub(crate) tag: Option<String>,
    /// Where what it carries is held, drawn as a translucent copy of its box.
    pub(crate) ghost: Option<Rect>,
    /// The lines what it carries snapped to, or could snap to, each as wide or as tall as its box ([`crate::snap`]).
    pub(crate) guides: Vec<Rect>,
}

thread_local! {
    static DRAGGING: RwSignal<bool> = detached(|| signal(false));
    static HINT: RwSignal<Option<Hint>> = detached(|| signal(None));
}

/// Whether a drag is under way, read as a signal: a build that reads it runs again as one starts and ends.
pub(crate) fn dragging() -> bool {
    DRAGGING.with(|dragging| dragging.get())
}

/// What the drag under way shows at the pointer; cleared as it ends.
pub(crate) fn hint() -> RwSignal<Option<Hint>> {
    HINT.with(|hint| *hint)
}

/// Says at `pointer` what the drag under way snaps to: each of `guides` drawn across `bounds`.
pub(crate) fn show_guides(pointer: (f32, f32), guides: &[crate::snap::Guide], bounds: Rect) {
    hint().set(Some(Hint {
        pointer,
        guides: crate::snap::lines(guides, bounds),
        ..Hint::default()
    }));
}

fn started() {
    DRAGGING.with(|dragging| {
        if !dragging.peek() {
            dragging.set(true);
        }
    });
}

fn ended() {
    DRAGGING.with(|dragging| {
        if dragging.peek() {
            dragging.set(false);
        }
    });
    if hint().peek().is_some() {
        hint().set(None);
    }
}

/// The drag state of one handle: [`dragging`] is set by its first move and cleared as its transaction ends, whichever way, so the handle drags again without being rebuilt.
pub(crate) struct HandleDragging {
    moving: Rc<Cell<bool>>,
}

impl HandleDragging {
    pub(crate) fn new() -> Self {
        Self {
            moving: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn wrap_to_value<F, X>(&self, f: F) -> impl Fn(f32, f32) -> X + 'static
    where
        F: Fn(f32, f32) -> X + 'static,
        X: 'static,
    {
        let moving = self.moving.clone();
        move |x, y| {
            if !moving.replace(true) {
                started();
            }
            f(x, y)
        }
    }

    pub(crate) fn on_end_fn(&self) -> Rc<dyn Fn() + 'static> {
        let moving = self.moving.clone();
        Rc::new(move || {
            moving.set(false);
            ended();
        })
    }

    pub(crate) fn transaction(&self, value: RwSignal<f32>) -> Transaction<f32> {
        let (committed, reverted) = (self.on_end_fn(), self.on_end_fn());
        Transaction::new(value)
            .on_commit(move |_, _| committed())
            .on_revert(move |_| reverted())
    }
}

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

/// `target` answering a press by selecting what is under the pointer, as the selection tool's own boxes do, and a secondary press by opening its menu there; the pointer over it outlines what a press would select. `node` is anything on the screen and layer the target is over.
pub(crate) fn pressable(
    target: StyledContainer,
    node: impl Fn() -> Option<Node> + Clone + 'static,
) -> StyledContainer {
    let asked = node.clone();
    crate::select::hoverable(target, node.clone())
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

/// `target` dragged through `transaction` once the pointer travels past [`THRESHOLD`]. The first move asks `take` what the drag holds on to — what it read when it began, so its own preview cannot move what it aims at — told where the press that began it landed, in the window's coordinates as [`surfaces::menu::pointer`] gives them: a grab measured from there keeps what is carried under the very point it was taken hold of, not one the travel before the threshold has slid away from it. Every move, the first included, hands what was taken to `follow` with where the pointer is in the target's own coordinates. Whether the drag is let go or cancelled, what it held goes to `release`, told which (`true` for let go), and the drag holds nothing after. While it goes [`dragging`] says so, and as it ends its [`hint`] is cleared.
pub(crate) fn drag<T: 'static, X: Clone + 'static>(
    target: StyledContainer,
    transaction: Transaction<X>,
    take: impl Fn((f32, f32)) -> Option<T> + 'static,
    follow: impl Fn(&T, (f32, f32)) + 'static,
    release: impl Fn(Option<T>, bool) + 'static,
) -> Pressed {
    let pressed: Rc<Cell<Option<(f32, f32)>>> = Rc::default();
    let pressed_at = Rc::clone(&pressed);
    let holding: Rc<RefCell<Option<T>>> = Rc::default();
    let (ending, cancelling) = (Rc::clone(&holding), Rc::clone(&holding));
    let release = Rc::new(release);
    let cancelled = Rc::clone(&release);
    let target = target
        .drag_threshold(THRESHOLD)
        .drag_transaction(transaction)
        .on_drag(move |x, y| {
            started();
            if holding.borrow().is_none() {
                let Some(at) = pressed_at.get() else {
                    return;
                };
                let taken = take(at);
                *holding.borrow_mut() = taken;
            }
            if let Some(held) = holding.borrow().as_ref() {
                follow(held, (x, y));
            }
        })
        .on_drag_end(move |_, _| {
            ended();
            release(ending.borrow_mut().take(), true);
        })
        .on_drag_cancel(move || {
            ended();
            cancelled(cancelling.borrow_mut().take(), false);
        });
    Pressed { target, pressed }
}

/// A dragged target that notes where each primary press on it lands before the target hears it, since telar tells a drag only where the pointer is once it has travelled past its threshold.
pub(crate) struct Pressed {
    target: StyledContainer,
    pressed: Rc<Cell<Option<(f32, f32)>>>,
}

impl Component for Pressed {
    fn view(&self) -> RenderNode {
        self.target.view()
    }

    fn on_event(&mut self, event: &Event) -> EventResult {
        if let Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            ..
        } = event
        {
            self.pressed.set(Some((*x as f32, *y as f32)));
        }
        self.target.on_event(event)
    }

    fn debug_name(&self) -> &'static str {
        "Pressed"
    }
}

impl LayoutItem for Pressed {
    fn layout_node(&self) -> NodeId {
        self.target.layout_node()
    }

    fn occludes(&self) -> bool {
        self.target.occludes()
    }
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
