//! What a layout binds to the gestures on an instance or on an area's empty space, and running it.
//!
//! An action is a chain of IPC lines, run through the same command table the socket answers ([`services::command::run_chain`]), so a press, a keybind and `hogar-shell …` reach the same code. A bound trigger takes the place of whatever the shell itself would have done with that gesture — a chip's own press, its wheel — and leaves every other gesture as it was. Nothing is ever bound on the lock layer (TA-8).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use telar::{LayoutItem, PointerButton, StyledContainer, track_layout};

use layout::{Action, AreaId, LayerKind, Trigger};
use ui::host::Audience;

use crate::rects::Node;

/// A wheel handler, as `(dx, dy)` in pixels.
pub type Wheel = Rc<dyn Fn(f32, f32)>;

/// About one mouse-wheel notch, which telar counts as ±60 px: a chain runs once a notch rather than once per smooth-scroll event.
pub const NOTCH: f32 = 50.0;

/// The gestures one instance or one area's empty space binds, and the context menu a secondary press opens where none is bound.
#[derive(Clone, Default)]
pub struct Bound {
    actions: Rc<BTreeMap<Trigger, Action>>,
    menu: Option<Rc<dyn Fn()>>,
    owner: Option<Node>,
    own_press: Option<Rc<dyn Fn()>>,
}

impl Bound {
    /// What `actions` binds for whoever is in front of the screen: nothing at all on the lock layer, whatever the file says.
    pub fn of(actions: &BTreeMap<Trigger, Action>, audience: Audience) -> Self {
        match audience {
            Audience::Owner => Self {
                actions: Rc::new(actions.clone()),
                ..Self::default()
            },
            Audience::Anyone => Self::default(),
        }
    }

    /// The same, opening `menu` on a secondary press nothing is bound to ([`crate::menu::on`]).
    pub fn with_menu(self, menu: Option<Rc<dyn Fn()>>) -> Self {
        Self { menu, ..self }
    }

    /// The same, for the instance at `owner`: a press nothing is bound to opens the panel the layout gives it, where it has one ([`crate::panel::owns_panel`]).
    pub fn owning(self, owner: Node) -> Self {
        Self {
            owner: Some(owner),
            ..self
        }
    }

    /// The same, a press nothing bound and no owned panel answers running `own_press`: the module's own press or panel.
    pub fn with_own_press(self, own_press: Option<Rc<dyn Fn()>>) -> Self {
        Self { own_press, ..self }
    }

    /// Whether nothing is bound and no menu offered, so there is nothing to answer.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
            && self.menu.is_none()
            && self.own_press.is_none()
            && !self.opens_panel()
    }

    /// What `trigger` runs, if it is bound to a chain with a command in it.
    pub fn runs(&self, trigger: Trigger) -> Option<Rc<dyn Fn()>> {
        let action = self
            .actions
            .get(&trigger)
            .filter(|action| !action.0.is_empty())?
            .clone();
        Some(Rc::new(move || run(&action)))
    }

    /// What a press runs, first that answers: its bound action, the panel the layout gives the instance, then its own press.
    pub fn press(&self) -> Option<Rc<dyn Fn()>> {
        self.runs(Trigger::Press)
            .or_else(|| self.owned_panel(crate::panel::toggle_owned, self.own_press.clone()))
    }

    /// What pulling the instance off its bar runs: it opens the panel the layout gives it, else `built_in`. A bound press has no say, since a chain is nothing to open.
    pub fn drag(&self, built_in: Option<Rc<dyn Fn()>>) -> Option<Rc<dyn Fn()>> {
        self.owned_panel(crate::panel::open_owned_at, built_in)
    }

    fn opens_panel(&self) -> bool {
        self.owner.as_ref().is_some_and(crate::panel::owns_panel)
    }

    /// Read again at the press, so a panel taken out of the layout since the build hands the press back to `built_in`.
    fn owned_panel(
        &self,
        act: fn(&Node) -> bool,
        built_in: Option<Rc<dyn Fn()>>,
    ) -> Option<Rc<dyn Fn()>> {
        let Some(owner) = self.owner.clone().filter(|_| self.opens_panel()) else {
            return built_in;
        };
        Some(Rc::new(move || {
            if !act(&owner)
                && let Some(built_in) = &built_in
            {
                built_in();
            }
        }))
    }

    pub fn long_press(&self) -> Option<Rc<dyn Fn()>> {
        self.runs(Trigger::LongPress)
    }

    /// A secondary press runs its bound action, else the context menu, where one is offered: what the layout binds wins.
    pub fn secondary(&self) -> Option<Rc<dyn Fn()>> {
        self.runs(Trigger::Secondary).or_else(|| self.menu.clone())
    }

    /// The middle and secondary presses, as one handler for [`StyledContainer::on_alt_press`].
    pub fn alt_press(&self) -> Option<Rc<dyn Fn(PointerButton)>> {
        let middle = self.runs(Trigger::Middle);
        let secondary = self.secondary();
        if middle.is_none() && secondary.is_none() {
            return None;
        }
        Some(Rc::new(move |button| {
            let run = match button {
                PointerButton::Auxiliary => &middle,
                PointerButton::Secondary => &secondary,
                PointerButton::Primary => &None,
            };
            if let Some(run) = run {
                run();
            }
        }))
    }

    /// The wheel: a bound direction runs its action once a notch, and whatever is not bound goes to `built_in`.
    pub fn wheel(&self, built_in: Option<Wheel>) -> Option<Wheel> {
        let up = self.runs(Trigger::ScrollUp);
        let down = self.runs(Trigger::ScrollDown);
        if up.is_none() && down.is_none() {
            return built_in;
        }
        let travelled = Cell::new(0.0f32);
        Some(Rc::new(move |dx, dy| {
            // `dy > 0` is a scroll up: the platform already flips Wayland's axis.
            let bound = match dy {
                dy if dy > 0.0 => &up,
                dy if dy < 0.0 => &down,
                _ => &None,
            };
            let Some(run) = bound else {
                travelled.set(0.0);
                if let Some(built_in) = &built_in {
                    built_in(dx, dy);
                }
                return;
            };
            let so_far = match travelled.get().signum() == dy.signum() {
                true => travelled.get() + dy,
                false => dy,
            };
            if so_far.abs() < NOTCH {
                travelled.set(so_far);
                return;
            }
            travelled.set(0.0);
            run();
        }))
    }

    /// Every bound gesture on `container`, and the menu, for a root that has no built-in gesture of its own to give way.
    pub fn on(&self, container: StyledContainer) -> StyledContainer {
        let wheel = self.wheel(None);
        self.presses(container)
            .maybe_on_scroll(wheel.map(|run| move |dx, dy| run(dx, dy)))
    }

    /// The bound presses alone — primary, long, middle and secondary, the last falling back to the menu — for a root whose wheel is merged with its own elsewhere.
    pub fn presses(&self, container: StyledContainer) -> StyledContainer {
        let press = self.press();
        let long_press = self.long_press();
        let alt_press = self.alt_press();
        container
            .maybe_on_press(press.map(|run| move || run()))
            .maybe_on_long_press(long_press.map(|run| move || run()))
            .maybe_on_alt_press(alt_press.map(|run| move |button| run(button)))
    }

    /// The presses alone, for a placeholder standing in for the instance.
    pub fn answers(&self) -> ui::placeholder::Presses {
        ui::placeholder::Presses {
            press: self.press(),
            long_press: self.long_press(),
            alt_press: self.alt_press(),
        }
    }

    /// [`Bound::answers`] for a placeholder standing in for a module that failed, whose press nothing else answers opens the settings window it can be fixed in.
    pub fn fixing(&self) -> ui::placeholder::Presses {
        self.clone()
            .with_own_press(Some(Rc::new(ui::placeholder::open_settings)))
            .answers()
    }

    /// Whether any press is bound or a menu offered, which is what a chip with nothing else to wrap it needs a wrapper for.
    pub fn has_presses(&self) -> bool {
        self.menu.is_some()
            || self.own_press.is_some()
            || self.opens_panel()
            || [
                Trigger::Press,
                Trigger::LongPress,
                Trigger::Middle,
                Trigger::Secondary,
            ]
            .iter()
            .any(|trigger| self.actions.contains_key(trigger))
    }

    /// Every bound gesture on an area's root, and the menu, answering only over its empty space: a gesture that lands on one of the area's instances is that instance's, whether or not it answers it.
    ///
    /// Presses mostly never get here, since a chip claims its own rect. The wheel does — an input-opaque box lets it through to its ancestors so a scroll area of cards still scrolls — which is why where the pointer is is asked of [`crate::rects`] rather than left to dispatch.
    pub fn on_empty_space(&self, container: StyledContainer, of: EmptySpace) -> StyledContainer {
        if self.is_empty() {
            return container;
        }
        let Some(rect) = track_layout(container.layout_node()) else {
            return self.on(container);
        };
        let pointer = Rc::new(Cell::new(None::<(f32, f32)>));
        let empty: Rc<dyn Fn() -> bool> = {
            let pointer = Rc::clone(&pointer);
            Rc::new(move || {
                pointer.get().is_none_or(|at| {
                    !crate::rects::over_instance(of.output.as_deref(), of.layer, &of.area, at)
                })
            })
        };
        let only_there = |run: Rc<dyn Fn()>| -> Rc<dyn Fn()> {
            let empty = Rc::clone(&empty);
            Rc::new(move || {
                if empty() {
                    run();
                }
            })
        };
        let press = self.press().map(only_there);
        let long_press = self.long_press().map(only_there);
        let alt_press = self.alt_press().map(|run| {
            let empty = Rc::clone(&empty);
            move |button| {
                if empty() {
                    run(button);
                }
            }
        });
        let wheel = self.wheel(None).map(|run| {
            let empty = Rc::clone(&empty);
            move |dx, dy| {
                if empty() {
                    run(dx, dy);
                }
            }
        });
        container
            .on_pointer_move(move |x, y| {
                let origin = rect.peek();
                pointer.set(Some((origin.x + x, origin.y + y)));
            })
            .maybe_on_press(press.map(|run| move || run()))
            .maybe_on_long_press(long_press.map(|run| move || run()))
            .maybe_on_alt_press(alt_press)
            .maybe_on_scroll(wheel)
    }
}

/// Which area's empty space a root is, by the keys [`crate::rects`] holds its instances under.
pub struct EmptySpace {
    pub output: Option<String>,
    pub layer: LayerKind,
    pub area: AreaId,
}

/// Runs `action`'s chain, stopping at the first line the shell refuses ([`services::command::run_chain`]).
pub fn run(action: &Action) {
    if let Err(refused) = services::command::run_chain(&action.0) {
        tracing::warn!("action {refused}");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn ran() -> Vec<String> {
        RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    fn recording() {
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
    }

    fn menu() -> Option<Rc<dyn Fn()>> {
        Some(Rc::new(|| {
            RAN.with(|ran| ran.borrow_mut().push("menu".to_string()))
        }))
    }

    fn secondary(bound: &Bound) {
        let press = bound.alt_press().expect("a secondary press is answered");
        press(PointerButton::Secondary);
    }

    #[test]
    fn an_empty_chain_answers_nothing_so_the_press_falls_through() {
        recording();
        let bound = BTreeMap::from([(Trigger::Press, Action(Vec::new()))]);
        let built_in: Rc<dyn Fn()> =
            Rc::new(|| RAN.with(|ran| ran.borrow_mut().push("built-in".to_string())));
        let bound = Bound::of(&bound, Audience::Owner).with_own_press(Some(built_in));
        assert!(bound.runs(Trigger::Press).is_none());
        bound.press().expect("the built-in answers")();
        assert_eq!(ran(), ["built-in"]);
    }

    /// TA-4: what the layout binds to a secondary press wins, and the context menu opens only where nothing is bound to it.
    #[test]
    fn a_bound_secondary_action_wins_over_the_menu_and_the_menu_opens_otherwise() {
        recording();
        let bound = BTreeMap::from([(
            Trigger::Secondary,
            Action(vec!["launcher toggle".to_string()]),
        )]);
        secondary(&Bound::of(&bound, Audience::Owner).with_menu(menu()));
        assert_eq!(ran(), ["launcher toggle"]);

        let middle = BTreeMap::from([(Trigger::Middle, Action(vec!["media next".to_string()]))]);
        let offered = Bound::of(&middle, Audience::Owner).with_menu(menu());
        secondary(&offered);
        assert_eq!(
            ran(),
            ["menu"],
            "a middle binding leaves the secondary press to the menu"
        );
        offered.alt_press().expect("answered")(PointerButton::Auxiliary);
        assert_eq!(ran(), ["media next"]);

        assert!(
            Bound::of(&BTreeMap::new(), Audience::Owner)
                .alt_press()
                .is_none()
        );
        assert!(
            !Bound::of(&BTreeMap::new(), Audience::Owner)
                .with_menu(menu())
                .is_empty()
        );
    }
}
