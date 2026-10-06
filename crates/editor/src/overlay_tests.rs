//! The overlay mode's tools: stacks made, pinned, moved off their anchors and widened, each one undo entry; cards routed to the stack their routes name; and volume and brightness and the launcher placed at a stack.

#[cfg(test)]
mod tests {

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, compute_layout,
    };

    use std::time::{Duration, Instant};

    use layout::{
        Anchor, AreaId, CardKind, LayerKind, Offset, Route, RoutedCard, StackFlow, Urgency,
    };
    use surfaces::card_samples::{self, Shown};
    use surfaces::pinned;
    use surfaces::reconcile;
    use surfaces::rects::Node;
    use surfaces::transient::{self, Place};

    use crate::keys::Direction;
    use crate::mode::{self};
    use crate::modes::overlay::{self, Placed, stacks_of};
    use crate::rig::{Card, Rig, SCREEN, enter, pointer_at, press_at, release_at, rig_with, tap};
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
    const ALT_SHIFT: ModifiersState = ModifiersState {
        is_alt: true,
        is_shift: true,
        ..NONE
    };

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

    /// Shift+N makes a stack in the middle and selects it; an anchor pressed pins it there exactly; Alt+Shift+arrows move it off its anchor until the screen's edge stops it; its width is set from the popover — each one entry in the history, and undoing them all takes the stack away again.
    #[test]
    fn a_stack_is_made_pinned_moved_and_widened_each_as_one_undo_entry() {
        let rig = rig_with("overlay-stack", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
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

        assert!(tap(arrow(Direction::Down), ALT_SHIFT));
        assert!(tap(arrow(Direction::Left), ALT_SHIFT));
        assert_eq!(
            stack("stack-2").map(|placed| placed.offset),
            Some(Offset { x: -8.0, y: 8.0 })
        );
        assert!(tap(arrow(Direction::Right), ALT_SHIFT));
        assert!(
            tap(arrow(Direction::Right), ALT_SHIFT),
            "to the very edge of the screen"
        );
        let at_edge = stack("stack-2").expect("the stack");
        tap(arrow(Direction::Right), ALT_SHIFT);
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

    /// With a stack in the middle routed critical notifications, a critical notification goes there while toasts and every other notification stay in the corner — whatever order the two stacks are in.
    #[test]
    fn a_critical_notification_goes_to_a_centred_stack_while_toasts_stay_in_the_corner() {
        let rig = rig_with("overlay-routes", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
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
        let (middle, lane) = (centre.bounds.x + centre.bounds.width / 2.0, centre.lane());
        assert_eq!(lane.x + lane.width / 2.0, middle, "centred");
        let corner_lane = corner.lane();
        assert_eq!(
            corner_lane.x + corner_lane.width,
            corner.bounds.x + corner.bounds.width - surfaces::transient::DEFAULT_GAP,
            "in the corner"
        );
    }

    /// `v` makes a stack where volume and brightness appear, and Shift+O where the launcher opens — at that stack's anchor, on no other stack of the screen — and pressed again each puts things back; undo does too.
    #[test]
    fn volume_and_brightness_and_the_launcher_are_placed_at_a_stack() {
        let rig = rig_with("overlay-placement", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
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

        assert!(tap(Key::Char('O'), SHIFT));
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
        assert!(tap(Key::Char('O'), SHIFT));
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
        let _mode = enter(LayerKind::Overlay);
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
        let _mode = enter(LayerKind::Overlay);
        let id = AreaId::new("stack");
        let before = stack("stack").expect("the corner stack");
        let card = before.ghost();
        for keep in [false, true] {
            let edit = Edit::new(telar::t!("editor.overlay.moved", name = id.to_string()));
            edit.begin().expect("the drag starts");
            for (dx, dy) in [(-300.0, 0.0), (-700.0, 380.0)] {
                let (anchor, offset) = overlay::landing(
                    before.bounds,
                    before.width,
                    before.flow,
                    (card.x + dx, card.y + dy),
                );
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
        let _mode = enter(LayerKind::Overlay);
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
        for (flow, anchor) in [StackFlow::Column, StackFlow::Row]
            .into_iter()
            .flat_map(|flow| Anchor::ALL.into_iter().map(move |anchor| (flow, anchor)))
        {
            let natural = overlay::first_card(
                pinned::stack(WHOLE, anchor, 380.0, flow, Offset::ZERO),
                anchor,
                flow,
                380.0,
            );
            let (landed, offset) =
                overlay::landing(WHOLE, 380.0, flow, (natural.x + 10.0, natural.y - 12.0));
            assert_eq!(
                (landed, offset),
                (anchor, Offset::ZERO),
                "{flow:?} {anchor:?}: near enough snaps"
            );
        }
        let corner = overlay::ghost(
            pinned::column(WHOLE, Anchor::TopRight, 380.0, Offset::ZERO),
            Anchor::TopRight,
        );
        let (landed, offset) = overlay::landing(
            WHOLE,
            380.0,
            StackFlow::Column,
            (corner.x - 100.0, corner.y + 60.0),
        );
        assert_eq!(landed, Anchor::TopRight);
        assert_eq!(offset, Offset { x: -100.0, y: 60.0 });
        let placed = Placed {
            layer: LayerKind::Overlay,
            id: AreaId::new("stack"),
            anchor: landed,
            offset,
            width: 380.0,
            flow: StackFlow::Column,
            routes: Vec::new(),
            launcher: false,
            bounds: WHOLE,
        };
        assert_eq!(
            (placed.ghost().x, placed.ghost().y),
            (corner.x - 100.0, corner.y + 60.0),
            "and is drawn where it was let go"
        );
        let (middle, _) = overlay::landing(WHOLE, 380.0, StackFlow::Column, (800.0, 500.0));
        assert_eq!(middle, Anchor::Center);
        let (bottom, _) = overlay::landing(WHOLE, 380.0, StackFlow::Column, (760.0, 2000.0));
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

    fn tap_t() -> bool {
        tap(Key::Char('t'), NONE)
    }

    fn sample_kinds() -> Vec<(CardKind, Option<Urgency>)> {
        card_samples::samples()
            .peek()
            .iter()
            .map(|sample| (sample.kind, sample.urgency))
            .collect()
    }

    fn shown(id: &str) -> Vec<(CardKind, Option<Urgency>)> {
        overlay::shown_in(SCREEN, LayerKind::Overlay, &AreaId::new(id))
            .into_iter()
            .filter_map(|shown| match shown {
                Shown::Card(sample) => Some((sample.kind, sample.urgency)),
                Shown::Launcher(_) => None,
            })
            .collect()
    }

    /// `t` sends the five samples in turn, each drawn in the stack `layout::route_card` picks: the critical one in the stack routed `urgency = "critical"`, the rest in the corner. Each goes after the lifetime its real card has under the default config, nothing reaches the toaster, and leaving the mode clears what is left.
    #[test]
    fn sample_cards_are_routed_into_the_stacks_expire_and_go_with_the_mode() {
        let _rig = rig_with("overlay-try", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        overlay::add_stack().expect("a stack in the middle");
        popover::open_area(node("stack-2")).expect("its popover opens");
        let _ = laid();
        popover::shared::<Vec<Route>>("routes")
            .expect("the stack's routes")
            .set(vec![Route {
                kind: Some(CardKind::Notification),
                app: None,
                urgency: Some(Urgency::Critical),
            }]);
        popover::close();

        for _ in 0..4 {
            assert!(tap_t());
        }
        let normal = (CardKind::Notification, Some(Urgency::Normal));
        let critical = (CardKind::Notification, Some(Urgency::Critical));
        assert_eq!(sample_kinds().len(), 4, "{:?}", sample_kinds());
        assert_eq!(shown("stack-2"), vec![critical]);
        assert_eq!(
            shown("stack"),
            vec![normal, (CardKind::Toast, None), (CardKind::Osd, None)]
        );
        assert!(services::toaster::current().is_empty());

        let now = Instant::now();
        card_samples::sweep(now + Duration::from_millis(2000));
        assert_eq!(
            sample_kinds().len(),
            4,
            "nothing goes before [stack] timeout_ms"
        );
        card_samples::sweep(now + Duration::from_millis(3100));
        assert_eq!(
            sample_kinds(),
            vec![critical],
            "everything but the sticky critical notification goes after [stack] timeout_ms"
        );
        card_samples::sweep(now + Duration::from_secs(121));
        assert!(
            sample_kinds().is_empty(),
            "and that one after [notifications] critical_max_secs"
        );

        assert!(tap_t());
        assert!(card_samples::launcher().peek().is_some());
        assert!(tap_t());
        assert_eq!(
            sample_kinds(),
            vec![normal],
            "the sixth press starts round again"
        );

        mode::leave();
        assert!(card_samples::samples().peek().is_empty());
        assert!(card_samples::launcher().peek().is_none());
    }

    /// Volume only in every stack: every sample but the OSD is refused, and each refusal moves `t` on to the next, so the OSD still comes up.
    #[test]
    fn a_refused_sample_does_not_hold_up_the_ones_after_it() {
        let _rig = rig_with("overlay-try-past-refusals", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        only_volume_everywhere();
        for refused in ["notification", "critical", "toast"] {
            assert!(tap_t(), "{refused}");
            assert!(mode::refusal().peek().is_some(), "{refused}");
            assert!(card_samples::samples().peek().is_empty(), "{refused}");
        }
        assert!(tap_t());
        assert_eq!(sample_kinds(), vec![(CardKind::Osd, None)]);
    }

    /// A sample stays up as long as the real card of its kind would under the edited screen's config: the stack's `[stack] timeout_ms`, but a critical notification under `critical_sticky` until `critical_max_secs`, for as long as the mode is up where that is `0`.
    #[test]
    fn a_sample_lives_as_long_as_the_config_keeps_its_card_up() {
        let mut config = config::Config::default();
        config.stack.timeout_ms = 5000;
        let toast = card_samples::Sample::toast("camera", String::new(), String::new());
        let osd = card_samples::Sample::osd(40);
        let critical = card_samples::Sample::notification(
            "UPower",
            Urgency::Critical,
            String::new(),
            String::new(),
        );
        let normal = card_samples::Sample::notification(
            "Mail",
            Urgency::Normal,
            String::new(),
            String::new(),
        );
        let five = Some(Duration::from_secs(5));
        for sample in [&toast, &osd, &normal] {
            assert_eq!(card_samples::lifetime(sample, &config), five, "{sample:?}");
        }
        assert_eq!(
            card_samples::lifetime(&critical, &config),
            Some(Duration::from_secs(120))
        );
        config.notifications.critical_max_secs = 0;
        assert_eq!(card_samples::lifetime(&critical, &config), None);
        config.notifications.critical_sticky = false;
        assert_eq!(card_samples::lifetime(&critical, &config), five);
    }

    /// Volume and brightness in every stack, and nothing else.
    fn only_volume_everywhere() {
        overlay::add_stack().expect("a stack");
        for id in ["stack", "stack-2"] {
            let node = node(id);
            let ops = overlay::osd_here(
                &session::draft().peek(),
                &screen(),
                LayerKind::Overlay,
                &node.area,
                &[],
            )
            .expect("routes only volume");
            context::commit("osd".to_string(), ops).expect("committed");
        }
    }

    /// A sample that no stack takes is refused and nothing is put up.
    #[test]
    fn a_sample_no_stack_takes_is_refused() {
        let _rig = rig_with("overlay-try-nowhere", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        only_volume_everywhere();
        assert!(overlay::try_card(overlay::Try::Toast).is_err());
        assert!(card_samples::samples().peek().is_empty());
    }

    /// The flow row of a stack's popover turns its column into a row, previewed live; closing keeps it as one undo entry and Esc puts it back with nothing recorded.
    #[test]
    fn a_stacks_flow_row_makes_it_a_row_and_escape_takes_it_back() {
        let rig = rig_with("overlay-flow", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        let flow = || stack("stack").map(|placed| placed.flow);
        assert_eq!(flow(), Some(StackFlow::Column));

        popover::open_area(node("stack")).expect("its popover opens");
        let _tree = laid();
        popover::shared::<String>("flow")
            .expect("the stack's flow")
            .set("row".to_string());
        assert_eq!(flow(), Some(StackFlow::Row), "previewed as it is chosen");
        assert!(tap(Key::Named(telar::NamedKey::Escape), NONE));
        assert_eq!(flow(), Some(StackFlow::Column), "Esc puts it back");
        assert_eq!(rig.undo_label(), None);

        popover::open_area(node("stack")).expect("its popover opens");
        let _tree = laid();
        popover::shared::<String>("flow")
            .expect("the stack's flow")
            .set("row".to_string());
        popover::close();
        assert_eq!(flow(), Some(StackFlow::Row));
        assert_eq!(rig.undo_label().as_deref(), Some("Customize stack"));
        assert_eq!(session::undo().as_deref(), Ok("Customize stack"));
        assert_eq!(flow(), Some(StackFlow::Column));
    }

    /// A stack's menu sends any sample from its "Try cards" rows, as the strip button and `t` do: a preview in the stack the card is routed to, never a layout edit.
    #[test]
    fn a_stacks_try_cards_menu_sends_a_sample_and_leaves_the_layout_alone() {
        let rig = rig_with("overlay-try-menu", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        let before = crate::rig::stored(&rig);
        context::open(surfaces::menu::Asked {
            node: node("stack"),
            window: LayerKind::Overlay,
            at: None,
        })
        .expect("the stack's menu opens");
        assert_eq!(
            context::sub_rows("Try cards"),
            [
                "Sample notification",
                "Sample critical notification",
                "Sample toast",
                "Sample volume OSD",
                "Sample launcher"
            ]
        );
        context::pick("Sample toast");
        assert_eq!(shown("stack"), vec![(CardKind::Toast, None)]);
        assert_eq!(rig.undo_label(), None);
        assert_eq!(crate::rig::stored(&rig), before);

        context::open(surfaces::menu::Asked {
            node: node("stack"),
            window: LayerKind::Overlay,
            at: None,
        })
        .expect("the stack's menu opens");
        context::pick("Sample launcher");
        assert!(card_samples::launcher().peek().is_some());
        assert_eq!(crate::rig::stored(&rig), before);
    }

    /// Presses the width handle of a stack whose width the layout never wrote, moves the pointer `travel` px along (and Escapes if `cancel`), lets go and closes the popover: the layout is as stored, and nothing is recorded.
    fn width_handle_leaves_the_width_unwritten(test: &str, travel: Option<f32>, cancel: bool) {
        let rig = rig_with(test, |layout| {
            let mut screen = layout::OutputRule {
                matches: layout::OutputMatch(SCREEN.to_string()),
                ..layout::OutputRule::default()
            };
            screen.layers.overlay.areas.push(layout::Area {
                id: AreaId::new("stack"),
                kind: Some(layout::AreaKind::Stack {
                    anchor: None,
                    offset: None,
                    width: None,
                    flow: None,
                    output_policy: None,
                    routes: Vec::new(),
                    launcher: None,
                }),
                ..layout::Area::default()
            });
            layout.outputs.push(screen);
        });
        let _owner = Owner::new();
        let lane = telar::Rect::new(1500.0, 40.0, 420.0, 1000.0);
        surfaces::rects::track_spanning(node("stack"), vec![telar::signal(lane)]);
        let before = crate::rig::stored(&rig);
        let wide = stack("stack").expect("the stack").width;
        popover::open_area(node("stack")).expect("its popover opens");
        let mut card = Card::open();
        let start = overlay::width_point(lane, Anchor::TopRight, wide);

        card.route(&press_at(start));
        if let Some(travel) = travel {
            card.route(&pointer_at((start.0 - travel, start.1)));
            assert_ne!(session::draft().peek(), before, "the drag previews");
        }
        if cancel {
            card.escape();
        }
        card.route(&release_at(start));
        transient::close(popover::ID);

        assert_eq!(crate::rig::stored(&rig), before);
        assert_eq!(undo_label(&rig), None, "nothing is recorded");
    }

    #[test]
    fn a_width_drag_cancelled_with_esc_leaves_the_width_unwritten() {
        width_handle_leaves_the_width_unwritten("overlay-width-cancelled", Some(60.0), true);
    }

    #[test]
    fn a_press_on_the_width_handle_without_travel_writes_nothing() {
        width_handle_leaves_the_width_unwritten("overlay-width-press", None, false);
    }
}
