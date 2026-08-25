//! The gesture every card in the column answers to.
//!
//! It lives here rather than with any one card because it is the *column's* rule: a notification, a toast and an OSD are all dragged aside the same way, and a gesture defined by one of them would be that one deciding how the other two behave. What differs between them is only what letting go means — retiring a notification to the history, dropping a toast, clearing the OSD — which arrives as `retire`.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{ReadSignal, RwSignal, StyledContainer, signal};

/// The column's swipe distance for a card drawn at its full width, or `None` with the gesture switched off.
pub(crate) fn column_threshold() -> Option<f32> {
    let stack = ::config::config().map(|c| c.stack).unwrap_or_default();
    stack.swipe_distance(stack.width)
}

/// Makes `card` follow a sideways drag and run `retire` if it is let go past `threshold`.
///
/// The card fades as it travels, so the gesture says what it will do before it does it — a card that slid and then sprang back with no visual difference reads as a failure rather than as a cancel.
///
/// **The displacement must never outlive the gesture**, and that is the whole correctness of this. The column is click-through except where its content registers as pressable, and what a widget registers is its *laid-out* rect (`StyledContainer::mark_interactive`) — which `with_transform` never moves. So a displaced card is drawn in one place and pressable in another.
///
/// While the pointer is down that costs nothing: the compositor delivers the rest of the gesture to whichever surface took the press, without consulting the input region again. It is a card *left* displaced that is unreachable — no press can land on it to put it back — and a card with no timeout, a sticky `critical` notification above all, then stays up until the shell is restarted. Resetting on every ending, rather than only on the snap-back, is what bounds the mismatch to the moment it is harmless.
///
/// The complete fix belongs in telar, where a transformed widget would register its transformed rect; until then this holds the invariant from the outside, so keep the reset unconditional.
pub(crate) fn swipe_aside(
    card: StyledContainer,
    threshold: f32,
    retire: impl Fn() + 'static,
) -> StyledContainer {
    let swiped = signal(0.0f32);
    with_offset(card, swiped.clone(), swiped.read_only(), threshold, retire)
}

/// [`swipe_aside`] against a caller's own offset signal, for a card that draws something from how far it has travelled — and for the tests, which read the offset to check the card was put back.
pub(crate) fn with_offset(
    card: StyledContainer,
    swiped: RwSignal<f32>,
    offset: ReadSignal<f32>,
    threshold: f32,
    retire: impl Fn() + 'static,
) -> StyledContainer {
    // The drag reports the pointer local to the card, so the *start* has to be remembered to get a delta — a press near the right edge would otherwise read as an instant swipe of nearly the card's width.
    let start: Rc<RefCell<Option<f32>>> = Rc::new(RefCell::new(None));
    let began = Rc::clone(&start);
    let tracking = swiped.clone();
    let settling = swiped;
    let fade = offset.clone();
    card.on_drag(move |x, _y| {
        let from = *began.borrow_mut().get_or_insert(x);
        tracking.set(x - from);
    })
    .on_drag_end(move |x, _y| {
        let from = start.borrow_mut().take().unwrap_or(x);
        let far_enough = (x - from).abs() >= threshold;
        // Unconditional, and before the retirement: whether the card actually goes is its owner's answer, and one still on screen when that answer is "no" must not be left where the gesture dropped it.
        settling.set(0.0);
        if far_enough {
            retire();
        }
    })
    .with_transform(move |_rect| {
        let dx = offset.get();
        (dx != 0.0).then_some([1.0, 0.0, 0.0, 1.0, dx, 0.0])
    })
    .with_opacity(move || 1.0 - (fade.get().abs() / threshold).clamp(0.0, 0.85))
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{
        AvailableSpace, Event, LayoutItem, LayoutStyle, PointerButton, PointerSource, RectStyle,
        compute_layout, new_container,
    };

    /// Drags `card` sideways by `dx` and lets go, as the compositor would deliver it.
    fn drag_by(card: &mut impl LayoutItem, from: (f64, f64), dx: f64) {
        card.on_event(&Event::PointerPressed {
            x: from.0,
            y: from.1,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        card.on_event(&Event::PointerMoved {
            x: from.0 + dx,
            y: from.1,
            source: PointerSource::Mouse,
        });
        card.on_event(&Event::PointerReleased {
            x: from.0 + dx,
            y: from.1,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
    }

    fn swiped_by(dx: f64, threshold: f32) -> (f32, bool) {
        const WIDTH: f32 = 360.0;
        telar::reset_layout_runtime();
        let retired = Rc::new(std::cell::Cell::new(false));
        let sink = Rc::clone(&retired);
        let swiped = signal(0.0f32);
        let box_ = StyledContainer::new(
            LayoutStyle::new().width(WIDTH).height(80.0),
            |_| RectStyle::default(),
            vec![],
        )
        .expect("the box builds");
        let mut card = with_offset(
            box_,
            swiped.clone(),
            swiped.read_only(),
            threshold,
            move || sink.set(true),
        );
        let root = new_container(
            LayoutStyle::new().flex_column().width(WIDTH).height(200.0),
            &[card.layout_node()],
        )
        .expect("root node");
        compute_layout(
            root,
            AvailableSpace::Definite(WIDTH),
            AvailableSpace::Definite(200.0),
        )
        .expect("layout");
        drag_by(&mut card, (8.0, 8.0), dx);
        (swiped.get(), retired.get())
    }

    /// **No gesture may leave a card displaced.**
    ///
    /// A card the swipe has moved is drawn in one place and pressable in another, because what a widget registers as its pointer target is the laid-out rect a transform never moves. While the pointer is down that costs nothing — the compositor delivers the rest of the gesture to whoever took the press — but a card *left* displaced is one nothing can reach, since no press can land on it to put it back.
    ///
    /// The regression this pins is the dismissing branch, which used to retire the card and return without resetting: correct only for as long as retiring really removed it. Nothing removes it here, which is exactly the case a sticky `critical` notification hit — on screen, untouchable, until a restart.
    #[test]
    fn a_dismissing_swipe_never_leaves_the_card_displaced() {
        let (offset, retired) = swiped_by(360.0, 126.0);
        assert!(retired, "a drag well past the threshold retires the card");
        assert_eq!(
            offset, 0.0,
            "the card is left {offset}px from where it can be pressed, and nothing can press it back"
        );
    }

    /// The snap-back, which has always worked, kept honest alongside the branch that did not.
    #[test]
    fn a_short_swipe_puts_the_card_back_and_keeps_it() {
        let (offset, retired) = swiped_by(10.0, 126.0);
        assert!(!retired, "short of the threshold the card stays");
        assert_eq!(offset, 0.0, "and it goes back where it was");
    }
}
