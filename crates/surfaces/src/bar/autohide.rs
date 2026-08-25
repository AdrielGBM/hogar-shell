//! `[bars.<edge>] persistent = false`: a bar that is only on screen when it is wanted.
//!
//! The bar is not hidden by drawing it somewhere else — it is *moved*. The layer surface sits at a negative margin on its own anchored edge, far enough off that only `[bars.<edge>] peek` logical pixels remain, and reveals itself by animating that margin back to the bar's usual gap. Two things follow from doing it that way. The bar takes no input over the strip it is not occupying, because it is genuinely not there — no input region to carve, nothing to get wrong. And the peek strip is the bar's own edge rather than a second surface that has to be kept in step with it, so there is one thing to place, one thing to reload, and one thing that can be hovered.
//!
//! What tells it to move is the plainest possible signal: the compositor delivers `CursorEntered` and `CursorLeft` to the surface, and the only part of the surface the pointer can reach while it is hidden *is* the peek strip.

use std::cell::Cell;
use std::rc::Rc;

use telar::motion::Animated;
use telar::{Component, Effect, Event, EventResult, LayoutItem, NodeId, PointerButton, RenderNode};

use config::{Config, Edge};

/// A layer-shell margin, as `(top, right, bottom, left)` logical pixels — the shape the compositor takes and the one thing this module passes around.
pub type Margin = (i32, i32, i32, i32);

/// The margins an auto-hidden bar moves between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevealMargins {
    pub hidden: Margin,
    pub shown: Margin,
}

impl RevealMargins {
    /// The two positions of `edge`'s bar, derived from the margin it would have if it were persistent.
    ///
    /// Only the anchored edge's own component moves: a top bar slides up, a left bar slides left, and the insets that keep it clear of a perpendicular bar are the same either way. Anything else would make a bar that shrinks as it hides rather than one that leaves.
    pub fn new(config: &Config, edge: Edge, shown: Margin) -> Self {
        let off = config.bar_hidden_offset(edge);
        let (t, r, b, l) = shown;
        let hidden = match edge {
            Edge::Top => (off, r, b, l),
            Edge::Bottom => (t, r, off, l),
            Edge::Left => (t, r, b, off),
            Edge::Right => (t, off, b, l),
        };
        Self { hidden, shown }
    }

    /// The margin at `reveal`, where 0 is hidden and 1 is fully out. Rounded rather than truncated so the two ends are reached exactly — a bar that settles one pixel short of its gap is a bar that never quite arrives.
    pub fn at(&self, reveal: f32) -> Margin {
        let mix = |from: i32, to: i32| {
            (from as f32 + (to - from) as f32 * reveal.clamp(0.0, 1.0)).round() as i32
        };
        (
            mix(self.hidden.0, self.shown.0),
            mix(self.hidden.1, self.shown.1),
            mix(self.hidden.2, self.shown.2),
            mix(self.hidden.3, self.shown.3),
        )
    }
}

/// Wraps a bar's content and moves the surface it is drawn on.
///
/// **The margin is pushed from an effect, and it has to be.** The obvious place is `view` — the surface's position is part of what is being drawn — but the runner composes a frame only when the tree is *dirty* (`if !tree_dirty && !needs_keepalive { return }`), and a bar sliding in redraws nothing: its content is identical at every position. So `view` ran once, the animation ran to completion unread, and the bar never moved — while `motion_has_active()` held the loop at 60 fps doing nothing at all. An effect subscribed to the animation's own signal is re-run by the motion tick's flush whether or not anything repaints, which is exactly the shape of this: the compositor moves the surface, the renderer has nothing to say about it.
///
/// The handle is held here because dropping it would deregister the effect — it would seed correctly, run once, and never fire again, which is the failure that looks like it works.
pub struct AutoHide {
    content: Box<dyn LayoutItem>,
    reveal: Animated<f32>,
    show_on_hover: bool,
    /// How far a press has to be pulled inward before it reveals the bar, for a pointer that cannot hover — a touch screen. `None` switches the gesture off (`[panels] drag_threshold = 0`).
    drag_threshold: Option<f32>,
    edge: Edge,
    dragging_from: Cell<Option<(f32, f32)>>,
    committed: Rc<Cell<Option<Margin>>>,
    _margin: Effect,
}

impl AutoHide {
    pub fn new(
        content: Box<dyn LayoutItem>,
        config: &Config,
        edge: Edge,
        shown_margin: Margin,
    ) -> Self {
        let bar = config.bars.get(edge);
        // Born settled at "hidden", where the surface was already placed when it was created, so nothing schedules a frame until the pointer actually arrives. The usual trap runs the other way: an animation that is *supposed* to be moving must not be constructed at its goal.
        let reveal = Animated::new(0.0, config.animation.panel_tween());
        let margins = RevealMargins::new(config, edge, shown_margin);
        let committed = Rc::new(Cell::new(None));
        let progress = reveal.read();
        let sink = Rc::clone(&committed);
        let push = telar::effect(move || {
            let margin = margins.at(progress.get());
            // The driver coalesces requests, but a bar at rest should be asking for nothing at all.
            if sink.get() != Some(margin) {
                platform_wayland::request_margin(margin);
                sink.set(Some(margin));
            }
        });
        Self {
            content,
            reveal,
            show_on_hover: bar.show_on_hover,
            drag_threshold: config.panels.drag_threshold(),
            edge,
            dragging_from: Cell::new(None),
            committed,
            _margin: push,
        }
    }

    fn reveal(&self) {
        self.reveal.retarget(1.0);
    }

    fn hide(&self) {
        self.reveal.retarget(0.0);
    }

    /// The margin last pushed to the compositor, or `None` before the first frame. The one observable the reveal has: `request_margin` reaches a live surface and nothing else, so a test drives the gesture and the clock and reads the answer here.
    pub fn committed_margin(&self) -> Option<Margin> {
        self.committed.get()
    }

    /// Whether a press that started at `from` and has reached `to` has been pulled far enough *into* the screen to count as a reveal. Only travel along the bar's own axis counts, and only inward — a swipe along a hidden top bar is a swipe, not a pull on it.
    fn pulled_inward(&self, from: (f32, f32), to: (f32, f32)) -> bool {
        let Some(threshold) = self.drag_threshold else {
            return false;
        };
        let travel = match self.edge {
            Edge::Top => to.1 - from.1,
            Edge::Bottom => from.1 - to.1,
            Edge::Left => to.0 - from.0,
            Edge::Right => from.0 - to.0,
        };
        travel >= threshold
    }
}

impl LayoutItem for AutoHide {
    fn layout_node(&self) -> NodeId {
        self.content.layout_node()
    }
}

impl Component for AutoHide {
    fn view(&self) -> RenderNode {
        self.content.view()
    }

    fn on_event(&mut self, event: &Event) -> EventResult {
        match event {
            Event::CursorEntered if self.show_on_hover => self.reveal(),
            Event::CursorLeft => {
                self.dragging_from.set(None);
                self.hide();
            }
            Event::PointerPressed {
                x,
                y,
                button: PointerButton::Primary,
                ..
            } => self.dragging_from.set(Some((*x as f32, *y as f32))),
            Event::PointerMoved { x, y, .. } => {
                if let Some(from) = self.dragging_from.get()
                    && self.pulled_inward(from, (*x as f32, *y as f32))
                {
                    self.dragging_from.set(None);
                    self.reveal();
                }
            }
            Event::PointerReleased { .. } => self.dragging_from.set(None),
            _ => {}
        }
        // Never `Handled`: revealing the bar happens *alongside* whatever the pointer was doing, not instead of it — a chip on a revealed bar has to stay clickable.
        self.content.on_event(event)
    }

    fn debug_name(&self) -> &'static str {
        "AutoHide"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Config;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn a_hidden_bar_leaves_exactly_its_peek_strip_on_screen() {
        let cfg = config(
            "[bars.top]\nsize=40\npersistent=false\npeek=3\ncenter=[\"clock\"]\n\
             [bars.left]\nsize=48\npersistent=false\npeek=6\nstart=[\"clock\"]\n",
        );

        let top = RevealMargins::new(&cfg, Edge::Top, (8, 8, 0, 8));
        assert_eq!(
            top.hidden.0,
            3 - 40,
            "a 40px bar showing 3px must sit 37px above the screen edge"
        );
        assert_eq!(
            (top.hidden.1, top.hidden.2, top.hidden.3),
            (8, 0, 8),
            "only the anchored edge moves — a hiding bar leaves, it does not shrink"
        );

        let left = RevealMargins::new(&cfg, Edge::Left, (0, 0, 0, 8));
        assert_eq!(left.hidden.3, 6 - 48, "a left bar slides out to the left");
        assert_eq!(
            left.hidden.0, 0,
            "and its perpendicular insets are unchanged"
        );
    }

    #[test]
    fn the_reveal_lands_exactly_on_both_ends() {
        let cfg = config("[bars.bottom]\nsize=34\npersistent=false\ncenter=[\"clock\"]\n");
        let margins = RevealMargins::new(&cfg, Edge::Bottom, (0, 6, 6, 6));

        assert_eq!(margins.at(0.0), margins.hidden);
        assert_eq!(
            margins.at(1.0),
            margins.shown,
            "a bar that settles a pixel short of its gap is a bar that never quite arrives"
        );
        // Past either end the surface must stop, not keep going: a spring overshoot would otherwise fling the bar off the far side of its own gap.
        assert_eq!(margins.at(-0.5), margins.hidden);
        assert_eq!(margins.at(1.5), margins.shown);
    }

    #[test]
    fn an_auto_hidden_bar_reserves_nothing_and_a_persistent_one_still_does() {
        let hidden = config("[bars.top]\nsize=40\npersistent=false\ncenter=[\"clock\"]\n");
        assert_eq!(
            hidden.edge_reserved(Edge::Top),
            0,
            "a bar that is not there most of the time must not tile every window short"
        );
        assert!(!hidden.bar_is_persistent(Edge::Top));

        let persistent = config("[bars.top]\nsize=40\ncenter=[\"clock\"]\n");
        assert_eq!(
            persistent.edge_reserved(Edge::Top),
            40 + persistent.edge_gap(Edge::Top)
        );
        assert!(
            persistent.bar_is_persistent(Edge::Bottom),
            "an edge with no bar on it has nothing to hide, so it is persistent by definition"
        );
    }

    /// The `[shape] frame` ring is not the bar, and an auto-hiding bar must not take it down with it.
    ///
    /// The ring is drawn on the background layer, so it shows only where no window covers it. An auto-hidden edge that reserved nothing at all let the windows tile straight over that edge's ring — three sides framed and one not, which reads as the frame being broken rather than as the bar hiding.
    #[test]
    fn an_auto_hidden_bar_still_reserves_the_frame_ring_it_is_not() {
        let framed = config(
            "[shape]\nframe=true\ngap=0\ninactive_size=8\n\
             [bars.top]\nsize=40\npersistent=false\ncenter=[\"clock\"]\n",
        );
        assert_eq!(
            framed.edge_reserved(Edge::Top),
            8 + framed.edge_gap(Edge::Top),
            "the ring is still there when the bar is not, so its own strip is still reserved"
        );

        let bare = config(
            "[shape]\nframe=false\n[bars.top]\nsize=40\npersistent=false\ncenter=[\"clock\"]\n",
        );
        assert_eq!(
            bare.edge_reserved(Edge::Top),
            0,
            "with no ring to keep clear, an auto-hidden edge reserves nothing"
        );
    }

    #[test]
    fn a_peek_strip_is_never_thinner_than_a_pixel() {
        let cfg = config("[bars.top]\nsize=40\npersistent=false\npeek=0\ncenter=[\"clock\"]\n");
        assert_eq!(
            cfg.bar_peek(Edge::Top),
            1,
            "a strip the pointer cannot land on is a bar with no way back"
        );
    }

    /// The reveal, driven the way the runner drives it: the pointer arrives, the clock advances, the reactive runtime flushes — and **nothing repaints**, because a bar sliding in draws exactly what it drew before.
    ///
    /// That last part is the test. The first version of this pushed the margin from `view`, and it worked perfectly here while doing nothing at all on screen: the runner composes a frame only when the tree is dirty, so `view` ran once and the animation played out with no one reading it. Driving `tick` + flush without a render is what makes this test able to fail the way the shell failed.
    #[test]
    fn the_pointer_arriving_moves_the_bar_and_leaving_puts_it_back() {
        use std::time::{Duration, Instant};

        let cfg = config("[bars.top]\nsize=32\npersistent=false\npeek=4\ncenter=[\"clock\"]\n");
        telar::reset_layout_runtime();
        telar::motion::reset();
        let shown = (8, 8, 0, 8);
        let content = telar::box_item(
            telar::Container::new(telar::LayoutStyle::new(), vec![]).expect("empty container"),
        );
        let mut bar = AutoHide::new(content, &cfg, Edge::Top, shown);
        let margins = RevealMargins::new(&cfg, Edge::Top, shown);

        assert_eq!(
            bar.committed_margin(),
            Some(margins.hidden),
            "registering the effect seeds it, and the seed is where the surface was already placed"
        );

        // The runner's animation pass: tick the clock, flush the writes it produced, compose nothing.
        let advance = |from: Instant| {
            for step in 1..=40 {
                telar::motion::tick(from + Duration::from_millis(step * 16));
                telar::batch(|| {});
            }
        };

        bar.on_event(&telar::Event::CursorEntered);
        advance(Instant::now());
        assert_eq!(
            bar.committed_margin(),
            Some(margins.shown),
            "the pointer reaching the peek strip has to bring the bar all the way out — a reveal that never \
             reaches the compositor is a bar that hides once and stays hidden"
        );

        bar.on_event(&telar::Event::CursorLeft);
        advance(Instant::now());
        assert_eq!(
            bar.committed_margin(),
            Some(margins.hidden),
            "and the pointer leaving has to put it back"
        );
    }

    #[test]
    fn only_an_inward_pull_past_the_threshold_reveals_the_bar() {
        let cfg = config(
            "[panels]\ndrag_threshold=48\n[bars.top]\nsize=40\npersistent=false\ncenter=[\"clock\"]\n",
        );
        telar::reset_layout_runtime();
        let content = telar::box_item(
            telar::Container::new(telar::LayoutStyle::new(), vec![]).expect("empty container"),
        );
        let bar = AutoHide::new(content, &cfg, Edge::Top, (6, 6, 0, 6));

        assert!(
            bar.pulled_inward((100.0, 1.0), (100.0, 60.0)),
            "a pull down off a hidden top bar reveals it"
        );
        assert!(
            !bar.pulled_inward((100.0, 1.0), (100.0, 20.0)),
            "a short pull is a tap that wandered, not a gesture"
        );
        assert!(
            !bar.pulled_inward((100.0, 1.0), (400.0, 1.0)),
            "a swipe *along* a hidden bar is a swipe, not a pull on it"
        );
        assert!(
            !bar.pulled_inward((100.0, 60.0), (100.0, 1.0)),
            "and a pull the other way is the gesture that puts it back, not the one that opens it"
        );
    }
}
