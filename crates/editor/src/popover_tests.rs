//! DEC-3's example end to end, short of the right-click that opens it (T-6.4): the bar's popover opens, its corner handle rounds the real bar live, Esc puts it back exactly and a click outside keeps it as one entry in the history — and the popover is drawn where its item is.
//!
//! The windows of a headless shell are never built, so the bar's place on screen is registered by hand and the popover's tree is built as its window would build it.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, PointerButton, PointerSource, Rect, compute_layout, signal,
    };

    use layout::{AreaId, Corners, GroupId, InstanceId, LayerKind, ResolvedAreaKind};
    use surfaces::layer_window::WindowKey;
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use surfaces::transient;

    use crate::mode::{self, Compositor};
    use crate::popover;
    use crate::popover::handles::{CORNERS, Corner, clamp_name};
    use crate::rig::{SCREEN, rig};
    use crate::session;

    const BAR: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 34.0,
    };

    /// An owner for what a test builds, disposed when the test ends: left alive, the trees it mounted would still answer the signals they read while the thread's storage is torn down.
    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            telar::dispose_owner(self.0.id());
        }
    }

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    /// Where the bar is on screen, as its window would register it once built.
    fn place_bar() {
        rects::track_spanning(bar(), vec![signal(BAR)]);
    }

    /// The open popover's tree, laid out over the whole screen.
    fn laid() -> ComponentList {
        let item = popover::tree()
            .expect("a popover is open")
            .expect("its tree builds");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = Container::new(page, vec![item]).expect("a page");
        let node = root.layout_node();
        let tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(1920.0),
            AvailableSpace::Definite(1080.0),
        )
        .expect("the popover lays out");
        tree
    }

    /// As the runner does: the keyboard's state first, then the overlays and the dismiss stack, then the tree.
    fn route(tree: &mut ComponentList, event: &Event) {
        telar::observe_keyboard(event);
        if !telar::dispatch_overlays(event) {
            tree.on_event(event);
        }
    }

    fn press((x, y): (f32, f32)) -> Event {
        Event::PointerPressed {
            x: x.into(),
            y: y.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn to((x, y): (f32, f32)) -> Event {
        Event::PointerMoved {
            x: x.into(),
            y: y.into(),
            source: PointerSource::Mouse,
        }
    }

    fn release((x, y): (f32, f32)) -> Event {
        Event::PointerReleased {
            x: x.into(),
            y: y.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn key(named: NamedKey) -> Event {
        Event::KeyPressed {
            key: Key::Named(named),
            modifiers: ModifiersState::default(),
        }
    }

    fn holding_alt(alt: bool) -> Event {
        Event::ModifiersChanged {
            modifiers: ModifiersState {
                is_alt: alt,
                ..ModifiersState::default()
            },
        }
    }

    /// The bar's corners as the screen draws them now.
    fn corners_on_screen() -> Option<Corners> {
        reconcile::desktops()[0]
            .resolved
            .layer(LayerKind::Top)?
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .and_then(|area| match area.kind {
                ResolvedAreaKind::Bar { shape, .. } => shape.radius,
                _ => None,
            })
    }

    fn corner(which: Corner) -> telar::RwSignal<f32> {
        let name = match which {
            Corner::TopLeft => "radius.top_left",
            Corner::TopRight => "radius.top_right",
            Corner::BottomRight => "radius.bottom_right",
            Corner::BottomLeft => "radius.bottom_left",
        };
        popover::shared::<f32>(name).expect("the bar's popover has its corners")
    }

    /// DEC-3: dragging the corner handle rounds the real bar as it moves, stopping at half the bar's short side with the handle saying so, and Esc puts back exactly what was on screen and records nothing.
    #[test]
    fn the_corner_handle_rounds_the_real_bar_live_clamps_and_escape_puts_it_back_exactly() {
        let rig = rig("popover-escape");
        let _scope = Scope::new();
        place_bar();
        let before = reconcile::desktops();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut tree = laid();

        let start = Corner::TopLeft.point(BAR, corner(Corner::TopLeft).peek());
        route(&mut tree, &press(start));
        route(&mut tree, &to((60.0, 60.0)));
        let half = BAR.height / 2.0;
        assert_eq!(
            corners_on_screen(),
            Some(Corners::all(half)),
            "every corner follows the drag, live, and stops at half the short side"
        );
        let clamped = popover::shared::<bool>(clamp_name(Corner::TopLeft))
            .expect("the handle shares its clamped state");
        assert!(clamped.peek(), "pulled past the bound, the handle says so");
        route(&mut tree, &to((8.0, 8.0)));
        assert!(!clamped.peek(), "and back in range, it stops saying so");
        assert_eq!(corners_on_screen(), Some(Corners::all(8.0)));
        route(&mut tree, &release((8.0, 8.0)));

        route(&mut tree, &key(NamedKey::Escape));
        assert!(
            Rc::ptr_eq(&reconcile::desktops(), &before),
            "Esc puts back the very arrangement that was on screen"
        );
        assert_eq!(rig.undo_label(), None, "and records nothing");
        assert_eq!(popover::current(), None, "the popover closed");
        assert!(!transient::is_open(popover::ID));
    }

    /// TA-4: Alt isolates the corner being dragged, and Esc mid-drag puts every corner back as it was — the ones the drag moved without being dragged too.
    #[test]
    fn alt_isolates_one_corner_and_escape_mid_drag_puts_all_four_back() {
        let _rig = rig("popover-alt");
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut tree = laid();
        let prior: Vec<f32> = CORNERS.iter().map(|at| corner(*at).peek()).collect();

        let start = Corner::TopRight.point(BAR, prior[1]);
        route(&mut tree, &press(start));
        route(&mut tree, &holding_alt(true));
        route(&mut tree, &to((start.0 - 4.0, start.1 + 4.0)));
        let now: Vec<f32> = CORNERS.iter().map(|at| corner(*at).peek()).collect();
        assert_eq!((now[0], now[2], now[3]), (prior[0], prior[2], prior[3]));
        assert_eq!(now[1], prior[1] + 4.0, "only the dragged corner moves");

        route(&mut tree, &holding_alt(false));
        route(&mut tree, &to((start.0 - 6.0, start.1 + 6.0)));
        let uniform = prior[1] + 6.0;
        assert!(
            CORNERS.iter().all(|at| corner(*at).peek() == uniform),
            "without Alt all four follow"
        );

        route(&mut tree, &key(NamedKey::Escape));
        let back: Vec<f32> = CORNERS.iter().map(|at| corner(*at).peek()).collect();
        assert_eq!(back, prior, "Esc mid-drag puts all four back");
        assert!(popover::current().is_some(), "and leaves the popover open");
        route(&mut tree, &key(NamedKey::Escape));
        assert_eq!(popover::current(), None);
        route(&mut tree, &release(start));
    }

    /// F-7: a click outside keeps what the popover changed as one entry in the history, and undoing it puts the bar back.
    #[test]
    fn a_click_outside_keeps_the_change_as_one_undo_entry() {
        let rig = rig("popover-commit");
        let _scope = Scope::new();
        place_bar();
        let before = reconcile::desktops();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut tree = laid();
        let start = Corner::BottomLeft.point(BAR, corner(Corner::BottomLeft).peek());
        route(&mut tree, &press(start));
        route(&mut tree, &to((5.0, BAR.height - 5.0)));
        route(&mut tree, &release((5.0, BAR.height - 5.0)));
        let thickness = popover::shared::<f32>("thickness").expect("the bar's thickness");
        thickness.set(40.0);

        transient::close(popover::ID);
        assert_eq!(popover::current(), None);
        assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
        assert_eq!(corners_on_screen(), Some(Corners::all(5.0)));
        assert_eq!(session::undo().as_deref(), Ok("Customize bar-top"));
        assert_eq!(
            rig.undo_label(),
            None,
            "the handle and the row were one entry"
        );
        assert_eq!(reconcile::desktops()[0].resolved, before[0].resolved);
    }

    /// F-7: Enter keeps the change as well, and a popover that changed nothing records nothing.
    #[test]
    fn enter_keeps_the_change_and_an_unchanged_popover_records_nothing() {
        let rig = rig("popover-enter");
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("it opens");
        popover::close();
        assert_eq!(rig.undo_label(), None);

        popover::open_area(bar()).expect("it opens again");
        let mut tree = laid();
        popover::shared::<f32>("gap")
            .expect("the bar's gap")
            .set(6.0);
        route(&mut tree, &key(NamedKey::Enter));
        assert_eq!(popover::current(), None);
        assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
    }

    /// DEC-9: outside an edit mode the popover is drawn in the window its item is, the top window for a bar; inside one it is drawn in the mode's host window, above the host.
    #[test]
    fn a_popover_is_drawn_in_its_items_window_and_over_the_host_in_an_edit_mode() {
        let _rig = rig("popover-window");
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("it opens outside an edit mode");
        assert_eq!(
            transient::drawn_in(popover::ID),
            Some(WindowKey {
                output: Some(SCREEN.to_string()),
                layer: LayerKind::Top
            })
        );
        popover::close();

        mode::enter_as(
            LayerKind::Top,
            Some(SCREEN),
            &Compositor {
                restack: true,
                locked: false,
                lockable: Ok(()),
            },
        )
        .expect("top mode");
        popover::open_for(&session::Selection::Area(bar())).expect("it opens in the mode");
        assert_eq!(
            transient::drawn_in(popover::ID).map(|window| window.layer),
            Some(LayerKind::Overlay)
        );
        popover::close();
        mode::leave();
    }

    /// An instance's popover is generated from its module's options and opens on the instance itself.
    #[test]
    fn an_instances_popover_opens_with_its_modules_options() {
        let rig = rig("popover-instance");
        let _scope = Scope::new();
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        popover::open_instance(clock.clone()).expect("the clock's popover opens");
        assert_eq!(popover::current(), Some(clock));
        let _tree = laid();
        popover::close();
        assert_eq!(rig.undo_label(), None, "nothing changed, nothing recorded");
        assert!(
            popover::open_instance(bar()).is_err(),
            "an area is not an instance"
        );
    }
}
