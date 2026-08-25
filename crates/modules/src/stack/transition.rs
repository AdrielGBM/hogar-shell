//! How a card arrives and how it leaves.
//!
//! The same shape as [`ui::panel::panel_transition`], and deliberately so: one `Animated` progress where **1 is away and transparent, 0 is settled**, so the exit is the entrance reversed rather than a second animation that has to be kept in step with the first. It is built away from its goal and retargeted at once, never at the goal — an `Animated` born settled never registers with the ticker, so nothing would schedule the frames that carry it in.
//!
//! Applied by the column to every card it holds rather than by each module to its own, which is what makes a notification, a toast and an OSD arrive the same way. Before this they simply appeared: `ReactiveList` builds a new row and disposes a gone one in the same breath, so nothing on screen distinguished the card that just arrived from the four that had been there, or said that one had left rather than been replaced.
//!
//! **A card cannot borrow the panel's exit.** A panel animates out because the driver holds its whole *surface* mapped for as long as `on_close` asks; a card is one row inside a surface shared with every other card, and the list drops it the instant its source does. So the progress lives here, keyed by slot, where the column can reach it after the card's own widget is gone — [`leaving`] is the column saying "play the exit", and [`settled`] is it collecting what has finished.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use telar::motion::Animated;
use telar::{LayoutError, LayoutItem, LayoutStyle, RectStyle, StyledContainer};

use config::AnimationConfig;

/// How far a card travels as it arrives and leaves, in px. Sideways, matching the swipe: the column's one gesture is horizontal, so a card that arrives along the same axis reads as the same object.
const TRAVEL: f32 = 28.0;

/// A gap a card is currently closing: how far it was displaced when the column re-laid out, and the progress carrying it from there back to nothing.
struct Closing {
    distance: f32,
    carry: Animated<f32>,
}

thread_local! {
    /// One progress per card slot, outliving the widget that draws it so the column can start the exit after the source has already dropped the card. Keyed by slot rather than by [`Card::key`], since a card whose contents changed is the same card in the same place and must not restart its entrance.
    static PROGRESS: RefCell<HashMap<String, Animated<f32>>> = RefCell::new(HashMap::new());
    /// When each departing card's exit is due to have finished.
    static LEAVING: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
}

/// Wraps `content` so it slides and fades in, and can later be asked to slide and fade back out.
///
/// The progress is looked up by slot and reused: a card the list rebuilds because its text changed keeps the entrance it already played, so an edited notification does not slide in again where it already sits.
pub(crate) fn arriving(
    slot: &str,
    content: Box<dyn LayoutItem>,
    animation: &AnimationConfig,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let tween = animation.tween_ms(200, 2_000);
    if tween.duration.is_zero() {
        return Ok(content);
    }
    let progress = PROGRESS.with(|held| {
        held.borrow_mut()
            .entry(slot.to_string())
            .or_insert_with(|| {
                let progress = Animated::new(1.0f32, tween);
                progress.retarget(0.0);
                progress
            })
            .clone()
    });
    let slide = progress.clone();
    let fade = progress;
    let was_at: Rc<Cell<Option<f32>>> = Rc::new(Cell::new(None));
    let closing: Rc<RefCell<Option<Closing>>> = Rc::new(RefCell::new(None));
    Ok(Box::new(
        StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), vec![content])?
            .with_transform(move |rect| {
                // The column re-lays out the instant a card above it goes, so this box's own top jumps by the height of the hole. Taking that jump as a starting offset and letting it decay to zero turns the relayout into a slide: the card is laid out where it belongs and merely *drawn* on its way there, which is why nothing below it has to know that it is moving.
                //
                // A fresh `Animated` per jump rather than one retargeted, because `Animated` has no way to be put back to a value — and a tween retargeted to 1 would travel there instead of starting there, which is the entrance played backwards rather than a card closing a gap.
                if let Some(was) = was_at.replace(Some(rect.y))
                    && (rect.y - was).abs() > 0.5
                {
                    let carry = Animated::new(1.0f32, tween);
                    carry.retarget(0.0);
                    *closing.borrow_mut() = Some(Closing {
                        distance: was - rect.y,
                        carry,
                    });
                }
                let dy = match closing.borrow().as_ref() {
                    Some(closing) => closing.distance * closing.carry.get(),
                    None => 0.0,
                };
                let at = slide.get();
                (at != 0.0 || dy != 0.0).then_some([1.0, 0.0, 0.0, 1.0, TRAVEL * at, dy])
            })
            .with_opacity(move || 1.0 - fade.get()),
    ))
}

/// Starts `slot`'s exit and answers when it will be over, so the column knows how long to keep drawing it.
///
/// Idempotent: a slot already on its way out keeps the deadline it was given rather than restarting, which is what stops a card that leaves while the column is rebuilding from animating out twice.
pub(crate) fn leaving(slot: &str, animation: &AnimationConfig) -> Duration {
    let tween = animation.tween_ms(200, 2_000);
    if tween.duration.is_zero() {
        forget(slot);
        return Duration::ZERO;
    }
    if LEAVING.with(|held| held.borrow().contains_key(slot)) {
        return tween.duration;
    }
    PROGRESS.with(|held| {
        if let Some(progress) = held.borrow().get(slot) {
            progress.retarget(1.0);
        }
    });
    LEAVING.with(|held| {
        held.borrow_mut()
            .insert(slot.to_string(), Instant::now() + tween.duration);
    });
    tween.duration
}

/// Whether `slot` is still playing its exit, and so still has to be drawn.
pub(crate) fn still_leaving(slot: &str) -> bool {
    LEAVING.with(|held| {
        held.borrow()
            .get(slot)
            .is_some_and(|until| Instant::now() < *until)
    })
}

/// Whether any card is still on its way out — what keeps the surface up for the last one's exit.
pub(crate) fn anything_leaving() -> bool {
    LEAVING.with(|held| held.borrow().values().any(|until| Instant::now() < *until))
}

/// Drops every exit that has finished, answering their slots so the column can stop drawing them.
pub(crate) fn settled() -> Vec<String> {
    let now = Instant::now();
    let done: Vec<String> = LEAVING.with(|held| {
        held.borrow()
            .iter()
            .filter(|(_, until)| now >= **until)
            .map(|(slot, _)| slot.clone())
            .collect()
    });
    for slot in &done {
        forget(slot);
    }
    done
}

/// Forgets a slot entirely, so the same card arriving again plays its entrance rather than appearing settled.
pub(crate) fn forget(slot: &str) {
    LEAVING.with(|held| held.borrow_mut().remove(slot));
    PROGRESS.with(|held| held.borrow_mut().remove(slot));
}

/// Cancels a departure because the card came back before its exit finished — a notification re-sent, an OSD retriggered while the last one was fading. Without this the returning card would keep fading out.
pub(crate) fn returning(slot: &str) {
    if LEAVING
        .with(|held| held.borrow_mut().remove(slot))
        .is_none()
    {
        return;
    }
    PROGRESS.with(|held| {
        if let Some(progress) = held.borrow().get(slot) {
            progress.retarget(0.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear() {
        PROGRESS.with(|held| held.borrow_mut().clear());
        LEAVING.with(|held| held.borrow_mut().clear());
    }

    fn content() -> Box<dyn LayoutItem> {
        telar::box_item(telar::Container::new(LayoutStyle::new(), vec![]).unwrap())
    }

    /// The entrance is played once per slot. A card the list rebuilds — a notification whose body changed keeps its slot and changes its key — must not slide in again from where it already sits.
    #[test]
    fn a_rebuilt_card_keeps_the_entrance_it_already_played() {
        telar::reset_layout_runtime();
        clear();
        let animation = AnimationConfig::default();
        arriving("notification\u{1}7", content(), &animation).expect("first build");
        let first = PROGRESS.with(|held| held.borrow().len());
        arriving("notification\u{1}7", content(), &animation).expect("rebuild");
        assert_eq!(
            PROGRESS.with(|held| held.borrow().len()),
            first,
            "the rebuild reused the slot's progress rather than starting a second entrance"
        );
    }

    /// A card is drawn for as long as its exit lasts, and no longer — the column asks `still_leaving` to decide whether to keep it in the list after its source has dropped it.
    #[test]
    fn a_departing_card_is_drawn_until_its_exit_is_over() {
        telar::reset_layout_runtime();
        clear();
        let animation = AnimationConfig::default();
        arriving("toast\u{1}vpn", content(), &animation).expect("build");

        let duration = leaving("toast\u{1}vpn", &animation);
        assert!(!duration.is_zero(), "there is an exit to play");
        assert!(still_leaving("toast\u{1}vpn"), "and it is playing now");
        assert!(anything_leaving(), "so the surface has to stay up");
        assert!(
            settled().is_empty(),
            "nothing has finished on the frame the exit started"
        );

        // The deadline is what decides, so moving it into the past is the same as waiting it out.
        LEAVING.with(|held| {
            held.borrow_mut()
                .insert("toast\u{1}vpn".to_string(), Instant::now());
        });
        assert!(!still_leaving("toast\u{1}vpn"));
        assert_eq!(settled(), vec!["toast\u{1}vpn".to_string()]);
        assert!(!anything_leaving(), "and now the surface may go");
    }

    /// A card that comes back mid-exit stays. A notification re-sent, or an OSD retriggered while the last one was fading, must not keep fading out because of a departure nobody cancelled.
    #[test]
    fn a_card_that_returns_mid_exit_stops_leaving() {
        telar::reset_layout_runtime();
        clear();
        let animation = AnimationConfig::default();
        arriving("osd\u{1}volume", content(), &animation).expect("build");
        leaving("osd\u{1}volume", &animation);
        assert!(still_leaving("osd\u{1}volume"));

        returning("osd\u{1}volume");
        assert!(!still_leaving("osd\u{1}volume"), "it is staying after all");
        assert!(!anything_leaving());
    }

    /// With animation switched off there is no wrapper and no exit to wait for — a card appears and goes, which is what `[animation] enabled = false` asks for.
    #[test]
    fn animation_off_leaves_the_card_untouched_and_the_exit_instant() {
        telar::reset_layout_runtime();
        clear();
        let off = AnimationConfig {
            enabled: false,
            ..AnimationConfig::default()
        };
        arriving("toast\u{1}dnd", content(), &off).expect("build");
        assert!(
            PROGRESS.with(|held| held.borrow().is_empty()),
            "nothing to animate means nothing to keep"
        );
        assert!(leaving("toast\u{1}dnd", &off).is_zero());
        assert!(!anything_leaving());
    }
}
