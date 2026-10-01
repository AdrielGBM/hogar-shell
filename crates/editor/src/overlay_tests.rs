//! The overlay mode's tools (T-7.4): stacks made, pinned, moved off their anchors and widened, each one undo entry; cards routed to the stack their routes name; and volume and brightness and the launcher placed at a stack.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DismissRegistration, DrawCommand, Event, Key,
        LayoutItem, LayoutStyle, ModifiersState, compute_layout,
    };

    use layout::{Anchor, AreaId, CardKind, LayerKind, Offset, Route, RoutedCard, Urgency};
    use surfaces::pinned;
    use surfaces::reconcile;
    use surfaces::rects::Node;
    use surfaces::transient::{self, Place};

    use crate::keys::{self, Direction, Press};
    use crate::mode::{self, Compositor};
    use crate::modes::overlay::{self, Placed, stacks_of};
    use crate::rig::{Rig, SCREEN, rig_with};
    use crate::session::{self, Edit, Selection};
    use crate::{context, popover};

    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn enter() -> DismissRegistration {
        mode::enter_as(
            LayerKind::Overlay,
            Some(SCREEN),
            &Compositor {
                restack: true,
                locked: false,
                lockable: Ok(()),
            },
        )
        .expect("the overlay mode opens");
        let id = crate::host::transient_id(SCREEN);
        DismissRegistration::new(Rc::new(move || transient::close(&id)))
    }

    fn screen() -> reconcile::Desktop {
        reconcile::desktops()
            .iter()
            .find(|desktop| desktop.output.as_deref() == Some(SCREEN))
            .cloned()
            .expect("the edited screen")
    }

    fn stack(id: &str) -> Option<Placed> {
        stacks_of(&screen())
            .into_iter()
            .find(|placed| placed.id.as_str() == id)
    }

    fn node(id: &str) -> Node {
        Node::area(Some(SCREEN), LayerKind::Overlay, &AreaId::new(id))
    }

    fn undo_label(rig: &Rig) -> Option<String> {
        rig.undo_label()
    }

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };
    const SHIFT: ModifiersState = ModifiersState {
        is_shift: true,
        ..NONE
    };
    const ALT: ModifiersState = ModifiersState {
        is_alt: true,
        ..NONE
    };

    /// A key pressed and let go, as the host's window hears it: the keyboard's state, the dismiss stack, then the host's keys.
    fn tap(key: Key, modifiers: ModifiersState) -> bool {
        let event = Event::KeyPressed {
            key: key.clone(),
            modifiers,
        };
        telar::observe_keyboard(&event);
        let taken =
            telar::dispatch_overlays(&event) || keys::press_as(&key, modifiers, Press::First);
        telar::observe_keyboard(&Event::KeyReleased {
            key: key.clone(),
            modifiers,
        });
        keys::settle_released();
        taken
    }

    fn arrow(direction: Direction) -> Key {
        Key::Named(direction.arrow())
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

    fn texts(tree: &ComponentList) -> Vec<String> {
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    fn lands(card: RoutedCard) -> Option<String> {
        reconcile::stack_for(&reconcile::stacks(), &Some(SCREEN.to_string()), &card)
            .map(|site| site.area.to_string())
    }

    const CRITICAL: RoutedCard = RoutedCard {
        kind: CardKind::Notification,
        app: Some("Mail"),
        urgency: Some(Urgency::Critical),
    };
    const NORMAL: RoutedCard = RoutedCard {
        kind: CardKind::Notification,
        app: Some("Mail"),
        urgency: Some(Urgency::Normal),
    };
    const TOAST: RoutedCard = RoutedCard {
        kind: CardKind::Toast,
        app: None,
        urgency: None,
    };
    const OSD: RoutedCard = RoutedCard {
        kind: CardKind::Osd,
        app: None,
        urgency: None,
    };

    /// TA-5: Shift+N makes a stack in the middle and selects it; an anchor pressed pins it there exactly; Alt+arrows move it off its anchor until the screen's edge stops it; its width is set from the popover — each one entry in the history, and undoing them all takes the stack away again.
    #[test]
    fn a_stack_is_made_pinned_moved_and_widened_each_as_one_undo_entry() {
        let rig = rig_with("overlay-stack", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        assert!(tap(Key::Char('N'), SHIFT), "Shift+N makes a stack");
        let made = stack("stack-2").expect("a new stack");
        assert_eq!((made.anchor, made.offset), (Anchor::Center, Offset::ZERO));
        assert!(made.routes.is_empty() && !made.launcher);
        assert_eq!(
            session::selected(),
            Selection::Area(node("stack-2")),
            "and it is selected"
        );
        assert_eq!(undo_label(&rig).as_deref(), Some("Make the stack stack-2"));

        overlay::pin(&node("stack-2"), Anchor::TopRight).expect("pinned");
        assert_eq!(
            stack("stack-2").map(|placed| placed.anchor),
            Some(Anchor::TopRight)
        );
        assert_eq!(
            undo_label(&rig).as_deref(),
            Some("Pin stack-2 to the top right")
        );

        assert!(tap(arrow(Direction::Down), ALT));
        assert!(tap(arrow(Direction::Left), ALT));
        assert_eq!(
            stack("stack-2").map(|placed| placed.offset),
            Some(Offset { x: -8.0, y: 8.0 })
        );
        assert!(tap(arrow(Direction::Right), ALT));
        assert!(
            tap(arrow(Direction::Right), ALT),
            "to the very edge of the screen"
        );
        let at_edge = stack("stack-2").expect("the stack");
        tap(arrow(Direction::Right), ALT);
        assert_eq!(
            stack("stack-2").map(|placed| placed.offset),
            Some(at_edge.offset),
            "no further than the edge"
        );
        assert!(
            tap(arrow(Direction::Up), SHIFT),
            "Shift+arrows still step the anchor"
        );
        assert_eq!(
            stack("stack-2").map(|placed| placed.anchor),
            Some(Anchor::TopRight)
        );

        popover::open_area(node("stack-2")).expect("its popover opens");
        let _tree = laid();
        popover::shared::<f32>("width")
            .expect("the stack's width")
            .set(520.0);
        popover::close();
        assert_eq!(stack("stack-2").map(|placed| placed.width), Some(520.0));
        assert_eq!(undo_label(&rig).as_deref(), Some("Customize stack-2"));

        let nudge = "Move the stack a few pixels off its anchor that way: stack-2";
        for label in [
            "Customize stack-2",
            nudge,
            nudge,
            nudge,
            nudge,
            "Pin stack-2 to the top right",
            "Make the stack stack-2",
        ] {
            assert_eq!(session::undo().as_deref(), Ok(label));
        }
        assert_eq!(stack("stack-2"), None, "undone, the stack is gone");
    }

    /// T-7.4's accept: with a stack in the middle routed critical notifications, a critical notification goes there while toasts and every other notification stay in the corner — whatever order the two stacks are in.
    #[test]
    fn a_critical_notification_goes_to_a_centred_stack_while_toasts_stay_in_the_corner() {
        let rig = rig_with("overlay-routes", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        overlay::add_stack().expect("a stack in the middle");
        popover::open_area(node("stack-2")).expect("its popover opens");
        let tree = laid();
        assert!(
            texts(&tree)
                .iter()
                .any(|text| text.starts_with("Takes nothing: stack has no routes either")),
            "a new stack says it takes nothing while the corner one routes nothing and comes first: {:?}",
            texts(&tree)
        );
        popover::shared::<Vec<Route>>("routes")
            .expect("the stack's routes")
            .set(vec![Route {
                kind: Some(CardKind::Notification),
                app: None,
                urgency: Some(Urgency::Critical),
            }]);
        let tree = laid();
        assert!(
            texts(&tree)
                .iter()
                .any(|text| text == "Takes critical notifications"),
            "{:?}",
            texts(&tree)
        );
        popover::close();
        assert_eq!(undo_label(&rig).as_deref(), Some("Customize stack-2"));

        assert_eq!(lands(CRITICAL).as_deref(), Some("stack-2"));
        assert_eq!(lands(TOAST).as_deref(), Some("stack"));
        assert_eq!(lands(NORMAL).as_deref(), Some("stack"));
        let (centre, corner) = (
            stack("stack-2").expect("the centred stack"),
            stack("stack").expect("the corner stack"),
        );
        let (middle, column) = (centre.bounds.x + centre.bounds.width / 2.0, centre.column());
        assert_eq!(column.x + column.width / 2.0, middle, "centred");
        let corner_column = corner.column();
        assert_eq!(
            corner_column.x + corner_column.width,
            corner.bounds.x + corner.bounds.width - surfaces::transient::DEFAULT_GAP,
            "in the corner"
        );
    }

    /// TA-5: `v` makes a stack where volume and brightness appear, and Shift+L where the launcher opens — at that stack's anchor, on no other stack of the screen — and pressed again each puts things back; undo does too.
    #[test]
    fn volume_and_brightness_and_the_launcher_are_placed_at_a_stack() {
        let rig = rig_with("overlay-placement", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        overlay::add_stack().expect("a stack in the middle");
        assert!(matches!(
            modules::launcher::placement(Some(SCREEN)),
            Place::Centred
        ));

        assert!(tap(Key::Char('v'), NONE));
        assert_eq!(lands(OSD).as_deref(), Some("stack-2"));
        assert_eq!(
            lands(TOAST).as_deref(),
            Some("stack"),
            "the rest stays in the corner"
        );
        assert_eq!(
            undo_label(&rig).as_deref(),
            Some("Show volume and brightness here")
        );
        assert!(tap(Key::Char('v'), NONE));
        assert_eq!(lands(OSD).as_deref(), Some("stack"));
        assert_eq!(
            session::undo().as_deref(),
            Ok("Stop showing volume and brightness here")
        );
        assert_eq!(lands(OSD).as_deref(), Some("stack-2"));

        assert!(tap(Key::Char('L'), SHIFT));
        assert!(matches!(
            modules::launcher::placement(Some(SCREEN)),
            Place::Pinned {
                anchor: Anchor::Center,
                ..
            }
        ));
        overlay::toggle_launcher(&node("stack")).expect("the corner stack opens it now");
        assert!(matches!(
            modules::launcher::placement(Some(SCREEN)),
            Place::Pinned {
                anchor: Anchor::TopRight,
                within: layout::Within::Usable,
                ..
            }
        ));
        assert_eq!(
            stack("stack-2").map(|placed| placed.launcher),
            Some(false),
            "and the other no longer does"
        );
        assert_eq!(session::undo().as_deref(), Ok("Open the launcher here"));
        assert!(matches!(
            modules::launcher::placement(Some(SCREEN)),
            Place::Pinned {
                anchor: Anchor::Center,
                ..
            }
        ));
        assert!(tap(Key::Char('L'), SHIFT));
        assert!(matches!(
            modules::launcher::placement(Some(SCREEN)),
            Place::Centred
        ));
    }

    /// DEC-17 in the overlay mode: a stack's popover offers "above fullscreen" with the note that it keeps the screen off direct scanout, its routes, volume and brightness and the launcher; its menu offers the launcher and a new stack, and on the stack that takes what no route takes, no way to give it volume and brightness alone.
    #[test]
    fn a_stacks_popover_and_menu_offer_what_the_overlay_mode_places() {
        let rig = rig_with("overlay-popover", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        popover::open_area(node("stack")).expect("its popover opens");
        let shown = texts(&laid());
        for wanted in [
            telar::t!("editor.area.above_fullscreen"),
            telar::t!("editor.area.scanout"),
            telar::t!("editor.overlay.osd_here"),
            telar::t!("editor.overlay.launcher_here"),
            telar::t!("editor.overlay.add_route"),
        ] {
            assert!(shown.contains(&wanted), "{wanted:?} in {shown:?}");
        }
        assert!(popover::shared::<bool>("above_fullscreen").is_some());
        popover::close();

        context::open(surfaces::menu::Asked {
            node: node("stack"),
            window: LayerKind::Overlay,
            at: None,
        })
        .expect("the stack's menu opens");
        let rows = context::rows();
        for wanted in [
            telar::t!("editor.overlay.launcher_here"),
            telar::t!("editor.overlay.new_stack"),
        ] {
            assert!(rows.contains(&wanted), "{wanted:?} in {rows:?}");
        }
        assert!(
            !rows.iter().any(|row| row.contains("volume and brightness")),
            "the stack that takes what no route takes has volume and brightness already: {rows:?}"
        );
        transient::close(context::ID);
        assert!(session::select(Selection::Area(node("stack"))));
        tap(Key::Char('v'), NONE);
        assert_eq!(
            stack("stack").map(|placed| placed.routes),
            Some(Vec::new()),
            "and `v` there is refused rather than make it take volume and brightness alone"
        );
        assert_eq!(rig.undo_label(), None);
    }

    /// The drag's own path short of the pointer: the stack follows where its first card would be let go, pinned and moved off its anchor as [`overlay::landing`] says, live; Esc puts it back as it was and letting go keeps it as one entry in the history.
    #[test]
    fn a_dragged_stack_follows_its_first_card_and_is_kept_or_put_back_whole() {
        let rig = rig_with("overlay-drag", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        let id = AreaId::new("stack");
        let before = stack("stack").expect("the corner stack");
        let card = before.ghost();
        for keep in [false, true] {
            let edit = Edit::new(telar::t!("editor.overlay.moved", name = id.to_string()));
            edit.begin().expect("the drag starts");
            for (dx, dy) in [(-300.0, 0.0), (-700.0, 380.0)] {
                let (anchor, offset) =
                    overlay::landing(before.bounds, before.width, (card.x + dx, card.y + dy));
                let at = edit
                    .transaction()
                    .before()
                    .expect("what the drag started from");
                edit.preview(
                    overlay::moved(&at, &screen(), LayerKind::Overlay, &id, anchor, offset)
                        .expect("the stack moves"),
                )
                .expect("previewed");
            }
            let dropped = stack("stack").expect("the stack");
            assert_eq!(
                dropped.anchor,
                Anchor::Center,
                "let go over the middle of the screen"
            );
            assert_eq!(
                (dropped.ghost().x, dropped.ghost().y),
                (card.x - 700.0, card.y + 380.0),
                "its first card is where it was let go"
            );
            match keep {
                false => {
                    edit.revert().expect("Esc");
                    assert_eq!(stack("stack"), Some(before.clone()));
                    assert_eq!(rig.undo_label(), None);
                }
                true => {
                    edit.commit().expect("let go");
                    assert_eq!(rig.undo_label().as_deref(), Some("Move stack"));
                    assert_eq!(session::undo().as_deref(), Ok("Move stack"));
                    assert_eq!(stack("stack"), Some(before.clone()));
                }
            }
        }
    }

    /// A stack's box takes the pointer through every layer the overlay tools lay over the screen: pressed and carried to the middle of the screen, the stack previews there.
    #[test]
    fn a_stack_carried_by_the_pointer_previews_where_it_is_carried() {
        let _rig = rig_with("overlay-pointer-drag", |_| {});
        let _owner = Owner::new();
        let _mode = enter();
        let card = stack("stack").expect("the corner stack").ghost();
        let mode = mode::current().expect("the mode is up");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = surfaces::menu::Pointed::new(Box::new(
            Container::new(
                page,
                vec![overlay::tool(&mode).expect("the overlay tools build")],
            )
            .expect("a page"),
        ));
        let node = root.layout_node();
        let mut tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(1920.0),
            AvailableSpace::Definite(1080.0),
        )
        .expect("the tools lay out");
        let moved = |x: f32, y: f32| Event::PointerMoved {
            x: x.into(),
            y: y.into(),
            source: telar::PointerSource::Mouse,
        };
        let (x, y) = (card.x + 20.0, card.y + 20.0);
        for event in [
            moved(x, y),
            Event::PointerPressed {
                x: x.into(),
                y: y.into(),
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
            moved(x - 5.0, y),
            moved(x - 700.0, y + 380.0),
        ] {
            tree.on_event(&event);
        }
        assert_eq!(
            stack("stack").map(|placed| placed.anchor),
            Some(Anchor::Center)
        );
    }

    const WHOLE: telar::Rect = telar::Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    /// A first card let go where an anchor alone would put it, or near enough, lands exactly on that anchor; let go further off, it keeps the distance as its offset — on every anchor.
    #[test]
    fn a_dropped_stack_snaps_to_the_anchor_it_is_let_go_near() {
        for anchor in Anchor::ALL {
            let natural =
                overlay::ghost(pinned::column(WHOLE, anchor, 380.0, Offset::ZERO), anchor);
            let (landed, offset) =
                overlay::landing(WHOLE, 380.0, (natural.x + 10.0, natural.y - 12.0));
            assert_eq!(
                (landed, offset),
                (anchor, Offset::ZERO),
                "{anchor:?}: near enough snaps"
            );
        }
        let corner = overlay::ghost(
            pinned::column(WHOLE, Anchor::TopRight, 380.0, Offset::ZERO),
            Anchor::TopRight,
        );
        let (landed, offset) = overlay::landing(WHOLE, 380.0, (corner.x - 100.0, corner.y + 60.0));
        assert_eq!(landed, Anchor::TopRight);
        assert_eq!(offset, Offset { x: -100.0, y: 60.0 });
        let placed = Placed {
            layer: LayerKind::Overlay,
            id: AreaId::new("stack"),
            anchor: landed,
            offset,
            width: 380.0,
            routes: Vec::new(),
            launcher: false,
            bounds: WHOLE,
        };
        assert_eq!(
            (placed.ghost().x, placed.ghost().y),
            (corner.x - 100.0, corner.y + 60.0),
            "and is drawn where it was let go"
        );
        let (middle, _) = overlay::landing(WHOLE, 380.0, (800.0, 500.0));
        assert_eq!(middle, Anchor::Center);
        let (bottom, _) = overlay::landing(WHOLE, 380.0, (760.0, 2000.0));
        assert_eq!(bottom, Anchor::Bottom, "past the screen is at its edge");
    }

    /// The popover says what each stack takes by the rule the stack itself follows: its routes where it has any, everything no route takes for the first stack with none, and nothing for any other.
    #[test]
    fn what_a_stack_takes_is_said_by_the_routing_rule() {
        telar::set_locale("en");
        let critical = Route {
            kind: Some(CardKind::Notification),
            app: None,
            urgency: Some(Urgency::Critical),
        };
        let mail = Route {
            app: Some("Mail".into()),
            ..critical.clone()
        };
        let stacks = vec![
            (AreaId::new("corner"), Vec::new()),
            (AreaId::new("centre"), vec![critical, overlay::osd_route()]),
            (AreaId::new("spare"), Vec::new()),
            (AreaId::new("mail"), vec![mail]),
        ];
        assert_eq!(
            overlay::takes(&stacks, &AreaId::new("centre")),
            "Takes critical notifications, volume and brightness"
        );
        assert_eq!(
            overlay::takes(&stacks, &AreaId::new("mail")),
            "Takes critical notifications from Mail"
        );
        assert_eq!(
            overlay::takes(&stacks, &AreaId::new("corner")),
            "Takes every card no route on this screen takes"
        );
        assert_eq!(
            overlay::takes(&stacks, &AreaId::new("spare")),
            "Takes nothing: corner has no routes either and comes first"
        );
        assert!(!overlay::rest_shown_nowhere(&stacks));
        assert!(overlay::rest_shown_nowhere(&stacks[1..2]));
    }

    #[test]
    fn volume_and_brightness_are_added_and_taken_away_as_one_route() {
        let routes = overlay::osd_toggled(Vec::new());
        assert_eq!(routes, vec![overlay::osd_route()]);
        assert!(overlay::shows_osd(&routes));
        assert!(overlay::osd_toggled(routes).is_empty());
    }
}
