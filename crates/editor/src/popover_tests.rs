//! DEC-3's example end to end, short of the right-click that opens it: the bar's popover opens, its corner handle rounds the real bar live, Esc puts it back exactly and a click outside keeps it as one entry in the history — and the popover is drawn where its item is.
//!
//! The windows of a headless shell are never built, so the bar's place on screen is registered by hand and the popover's tree is built as its window would build it.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, PointerButton, PointerSource, Rect, compute_layout, signal,
    };

    use config::Edge;
    use layout::{
        Anchor, Area, AreaId, Corners, Group, GroupId, Instance, InstanceId, LayerKind, Layout,
        Level, OutputMatch, OutputRule, ResolvedAreaKind, WorkspaceMatch, WorkspaceRule,
    };
    use surfaces::layer_window::WindowKey;
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use toml::{Table, Value};

    use crate::mode::{self};
    use crate::popover::area::{edges, help, parsed, spelled, variants};
    use crate::popover::handles::{CORNERS, Corner, clamp_name};
    use crate::popover::place::{GAP, card_at};
    use crate::popover::value::{Step, get, path_of, set, unset};
    use crate::popover::{self, Provenance};
    use crate::rig::{Rig, SCREEN, rig, rig_with};
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

        crate::rig::open_mode(LayerKind::Top);
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

    #[test]
    fn a_model_value_round_trips_through_its_spelling() {
        assert_eq!(spelled(&Anchor::BottomRight), "bottom_right");
        assert_eq!(parsed::<Anchor>("top_left"), Some(Anchor::TopLeft));
        assert_eq!(parsed::<Edge>("left"), Some(Edge::Left));
        assert_eq!(parsed::<Anchor>("sideways"), None);
        assert_eq!(&*variants("Fit"), ["cover", "contain", "stretch", "tile"]);
        assert_eq!(&*edges(), ["top", "bottom", "left", "right"]);
    }

    #[test]
    fn a_rows_help_is_the_doc_comment_of_what_it_edits() {
        let thickness = help("AreaKind::Bar", "thickness").expect("documented");
        assert!(thickness.contains("thick"), "{thickness}");
        assert!(help("Style", "fill").is_some());
        assert_eq!(help("AreaKind::Bar", "nothing"), None);
    }

    /// A corner's handle sits over the centre of its arc, and a pointer there asks for the radius it is at, whichever corner it is.
    #[test]
    fn a_corner_handle_reads_back_the_radius_it_is_drawn_at() {
        for corner in CORNERS {
            for radius in [0.0, 6.0, 17.0] {
                let point = corner.point(BAR, radius);
                assert_eq!(corner.radius(BAR, point), radius, "{corner:?} at {radius}");
            }
        }
        assert_eq!(Corner::BottomRight.point(BAR, 8.0), (1912.0, 26.0));
    }

    const USABLE: Rect = Rect {
        x: 0.0,
        y: 34.0,
        width: 1920.0,
        height: 1046.0,
    };
    const CARD: (f32, f32) = (320.0, 400.0);

    fn inside(at: (f32, f32)) -> bool {
        at.0 >= USABLE.x
            && at.1 >= USABLE.y
            && at.0 + CARD.0 <= USABLE.x + USABLE.width
            && at.1 + CARD.1 <= USABLE.y + USABLE.height
    }

    /// Whatever it customizes and wherever that is, the card is on screen — a bar on any edge, a chip in any corner, a region the size of the screen.
    #[test]
    fn the_card_never_leaves_the_usable_area() {
        let items = [
            (Rect::new(0.0, 0.0, 1920.0, 34.0), Some(Edge::Top)),
            (Rect::new(0.0, 1046.0, 1920.0, 34.0), Some(Edge::Bottom)),
            (Rect::new(0.0, 34.0, 40.0, 1046.0), Some(Edge::Left)),
            (Rect::new(1880.0, 34.0, 40.0, 1046.0), Some(Edge::Right)),
            (Rect::new(1890.0, 0.0, 30.0, 34.0), Some(Edge::Top)),
            (Rect::new(1890.0, 1046.0, 30.0, 34.0), Some(Edge::Bottom)),
            (Rect::new(0.0, 0.0, 1920.0, 1080.0), None),
            (Rect::new(1700.0, 900.0, 200.0, 160.0), None),
        ];
        for (item, edge) in items {
            let at = card_at(item, edge, CARD, USABLE);
            assert!(inside(at), "{item:?} on {edge:?}: {at:?}");
        }
    }

    /// A card for a bar sits on the side of it the screen is on, centred on it where there is room.
    #[test]
    fn a_bars_card_hangs_off_the_side_it_faces() {
        let top = Rect::new(0.0, 0.0, 1920.0, 34.0);
        assert_eq!(card_at(top, Some(Edge::Top), CARD, USABLE), (800.0, 42.0));
        let bottom = Rect::new(0.0, 1046.0, 1920.0, 34.0);
        assert_eq!(
            card_at(bottom, Some(Edge::Bottom), CARD, USABLE).1,
            1046.0 - GAP - CARD.1
        );
    }

    fn table(text: &str) -> Table {
        toml::from_str(text).expect("a table")
    }

    #[test]
    fn a_nested_key_is_written_into_a_table_of_its_own_and_nothing_else_is_pinned() {
        let shown = table("[face]\nscale = 1.0\nhands = true\n");
        let mut own = Table::new();
        set(&mut own, &shown, &path_of("face.scale"), Value::Float(2.0));
        assert_eq!(own, table("[face]\nscale = 2.0\n"));
        assert_eq!(get(&own, &path_of("face.scale")), Some(&Value::Float(2.0)));
    }

    #[test]
    fn an_element_of_an_inherited_list_takes_the_whole_list_with_it() {
        let shown = table("hidden = [\"a\", \"b\", \"c\"]\n");
        let mut own = Table::new();
        let path = vec![Step::Key("hidden".into()), Step::Index(1)];
        set(&mut own, &shown, &path, Value::String("x".into()));
        assert_eq!(own, table("hidden = [\"a\", \"x\", \"c\"]\n"));
        unset(&mut own, &path);
        assert_eq!(own, table("hidden = [\"a\", \"c\"]\n"));
        unset(&mut own, &path_of("hidden"));
        assert!(own.is_empty());
    }

    /// Every text the tree draws, where it is drawn.
    fn texts(tree: &ComponentList) -> Vec<(String, Rect)> {
        let mut found = Vec::new();
        telar::for_each_with_matrix(&tree.commands(), |command, [a, b, c, d, e, f]| {
            if let telar::DrawCommand::Text { text, rect, .. } = command {
                let at = Rect::new(
                    a * rect.x + c * rect.y + e,
                    b * rect.x + d * rect.y + f,
                    rect.width,
                    rect.height,
                );
                found.push((text.to_string(), at));
            }
        });
        found
    }

    fn shows(tree: &ComponentList, wanted: &str) -> bool {
        texts(tree).iter().any(|(text, _)| text == wanted)
    }

    /// The open popover's tree with the node it is laid out from, laid out again after every event as its window lays it out.
    struct Card {
        tree: ComponentList,
        node: telar::NodeId,
    }

    impl Card {
        fn open() -> Self {
            let item = popover::tree()
                .expect("a popover is open")
                .expect("its tree builds");
            let page = LayoutStyle::new().width(1920.0).height(1080.0);
            let root = Container::new(page, vec![item]).expect("a page");
            let node = root.layout_node();
            let card = Self {
                tree: ComponentList::new(root),
                node,
            };
            card.lay_out();
            card
        }

        fn lay_out(&self) {
            crate::rig::lay_out(self.node, (1920.0, 1080.0));
        }

        fn route(&mut self, event: &Event) {
            route(&mut self.tree, event);
            self.lay_out();
        }
    }

    /// Turns the wheel over the card until `wanted` is among the rows it shows, then clicks the first `pressed` drawn below it.
    fn press_below(card: &mut Card, wanted: &str, pressed: &str) {
        let frame = crate::rig::card_of(&card.tree);
        let over = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
        for _ in 0..80 {
            let row = texts(&card.tree)
                .into_iter()
                .find(|(text, _)| text == wanted)
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| panic!("{wanted:?} is drawn"));
            let Some(pixels) = crate::rig::wheel_toward(frame, row) else {
                break;
            };
            card.route(&crate::rig::wheel_at(over, pixels));
        }
        let found = texts(&card.tree);
        let above = found
            .iter()
            .find(|(text, _)| text == wanted)
            .map(|(_, rect)| rect.y)
            .expect("the row is drawn");
        let rect = found
            .iter()
            .filter(|(text, rect)| text == pressed && rect.y > above)
            .map(|(_, rect)| *rect)
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap_or_else(|| panic!("{pressed:?} is drawn below {wanted:?}: {found:?}"));
        let at = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        card.route(&press(at));
        card.route(&release(at));
    }

    /// The built-in layout with `fill` on the `*` rule's bar, and a rule for the rig's screen that writes the bar as `here` says.
    fn filled(fill: &'static str, here: impl FnOnce(&mut Area)) -> impl FnOnce(&mut Layout) {
        layered(move |bar| bar.style.fill = Some(fill.to_string()), here)
    }

    /// The built-in layout with its `*` rule's bar as `under` makes it, and a rule for the rig's screen that writes the bar as `here` says — the narrowest level writing it, so the one the bar's popover writes into.
    fn layered(
        under: impl FnOnce(&mut Area),
        here: impl FnOnce(&mut Area),
    ) -> impl FnOnce(&mut Layout) {
        move |mine| {
            under(
                mine.outputs[0]
                    .layers
                    .top
                    .areas
                    .iter_mut()
                    .find(|area| area.id.as_str() == "bar-top")
                    .expect("the built-in layout has a top bar"),
            );
            let mut area = Area {
                id: AreaId::new("bar-top"),
                ..Area::default()
            };
            here(&mut area);
            let mut screen = OutputRule {
                matches: OutputMatch(SCREEN.to_string()),
                ..OutputRule::default()
            };
            screen.layers.top.areas.push(area);
            mine.outputs.push(screen);
        }
    }
    fn rule(rig: &Rig, output: &str, workspace: Option<&str>) -> Level {
        Level {
            layout: rig.store.borrow().active_id().clone(),
            output: OutputMatch(output.to_string()),
            workspace: workspace.map(|workspace| WorkspaceMatch(workspace.to_string())),
        }
    }

    fn fill_on_screen() -> Option<String> {
        reconcile::desktops()[0]
            .resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))
            .and_then(|area| area.style.fill.clone())
    }

    /// On a monitor rule, a fill only the `*` rule writes says where it comes from, and a key nobody writes says it is the default.
    #[test]
    fn an_inherited_fill_names_the_rule_and_file_it_comes_from() {
        let rig = rig_with("origin-inherited", filled("surface", |_| {}));
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let tree = laid();
        let draft = popover::area_draft().expect("an area's popover");
        let every = rule(&rig, "*", None);
        assert_eq!(
            draft.provenance(&["style.fill"]),
            Provenance::Inherited(every.clone())
        );
        assert!(!draft.writes(&["style.fill"]), "nothing to reset here");
        assert!(
            shows(&tree, &format!("From outputs.* in {}", every.file())),
            "the row says it"
        );
        assert_eq!(draft.provenance(&["style.opacity"]), Provenance::Default);
        assert!(shows(&tree, "Default"));
        popover::close();
        assert_eq!(
            rig.undo_label(),
            None,
            "reading where a key comes from writes nothing"
        );
    }

    /// Reset on a key the popover's level writes takes it off there, so the row and the bar show what the `*` rule gives; closing keeps that as one entry, and one undo puts the key back.
    #[test]
    fn reset_takes_a_key_off_where_the_popover_writes_and_one_undo_puts_it_back() {
        let rig = rig_with(
            "origin-reset",
            filled("surface", |area| area.style.fill = Some("red".into())),
        );
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        let draft = popover::area_draft().expect("an area's popover");
        assert_eq!(draft.provenance(&["style.fill"]), Provenance::Here);
        assert!(draft.writes(&["style.fill"]));
        assert!(shows(&card.tree, "Set here"));

        press_below(&mut card, "Fill", "Reset");
        let fill = popover::shared::<String>("style.fill").expect("the fill row");
        assert_eq!(fill.peek(), "surface", "the row shows what it inherits now");
        assert_eq!(
            fill_on_screen().as_deref(),
            Some("surface"),
            "and so does the bar"
        );
        assert!(!draft.writes(&["style.fill"]));
        assert_eq!(
            draft.provenance(&["style.fill"]),
            Provenance::Inherited(rule(&rig, "*", None))
        );

        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
        let written = rig.store.borrow().active().outputs[1].layers.top.areas[0].clone();
        assert_eq!(
            written.style.fill, None,
            "the key is gone from the screen's rule"
        );
        assert_eq!(session::undo().as_deref(), Ok("Customize bar-top"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(fill_on_screen().as_deref(), Some("red"));
    }

    /// A key a level after the popover's writes — a workspace rule, while the popover writes for every workspace — is reported as overridden there, since a change written here would not show.
    #[test]
    fn a_key_a_later_level_writes_is_reported_as_overridden() {
        let rig = crate::rig::rig_on("origin-beyond", Some("2"), |mine| {
            let mut ruled = WorkspaceRule {
                matches: WorkspaceMatch("2".into()),
                ..WorkspaceRule::default()
            };
            ruled.layers.top.areas.push(Area {
                id: AreaId::new("bar-top"),
                style: layout::Style {
                    fill: Some("red".into()),
                    ..layout::Style::default()
                },
                ..Area::default()
            });
            mine.outputs[0].workspaces.push(ruled);
        });
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let tree = laid();
        let draft = popover::area_draft().expect("an area's popover");
        let later = rule(&rig, "*", Some("2"));
        assert_eq!(
            draft.provenance(&["style.fill"]),
            Provenance::Overridden(later.clone())
        );
        assert!(shows(
            &tree,
            &format!(
                "Overridden by outputs.*.workspaces.2 in {}: a change here would not show",
                later.file()
            )
        ));
        popover::close();
    }

    /// An instance's option says the same — set here with a Reset, inherited from the `*` rule, or nobody's and so its module's configuration — and Reset puts the row at what it inherits.
    #[test]
    fn an_options_reset_is_the_same_reset_and_the_config_is_the_default() {
        let rig = rig_with("origin-options", |mine| {
            let clock = mine.outputs[0]
                .layers
                .top
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "bar-top")
                .and_then(|area| {
                    area.groups
                        .iter_mut()
                        .find(|group| group.id.as_str() == "center")
                })
                .and_then(|group| {
                    group
                        .children
                        .iter_mut()
                        .find(|child| child.id.as_str() == "clock")
                })
                .expect("the built-in bar has a clock");
            clock
                .options
                .insert("show_date".into(), Value::Boolean(true));
            clock
                .options
                .insert("twelve_hour".into(), Value::Boolean(false));
            let mut options = Table::new();
            options.insert("twelve_hour".into(), Value::Boolean(true));
            let mut screen = OutputRule {
                matches: OutputMatch(SCREEN.to_string()),
                ..OutputRule::default()
            };
            screen.layers.top.areas.push(Area {
                id: AreaId::new("bar-top"),
                groups: vec![Group {
                    id: GroupId::new("center"),
                    children: vec![Instance {
                        id: InstanceId::new("clock"),
                        options,
                        ..Instance::default()
                    }],
                    ..Group::default()
                }],
                ..Area::default()
            });
            mine.outputs.push(screen);
        });
        let _scope = Scope::new();
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let _tree = laid();
        let draft = popover::instance_draft().expect("an instance's popover");
        let twelve = path_of("twelve_hour");
        assert_eq!(draft.provenance(&twelve), Provenance::Here);
        assert_eq!(
            draft.provenance(&path_of("show_date")),
            Provenance::Inherited(rule(&rig, "*", None))
        );
        assert_eq!(
            draft.provenance(&path_of("date_format")),
            Provenance::Default
        );

        assert_eq!(draft.shown_at(&twelve), Some(Value::Boolean(true)));
        draft.reset(&twelve);
        assert_eq!(
            draft.provenance(&twelve),
            Provenance::Inherited(rule(&rig, "*", None))
        );
        assert_eq!(
            draft.shown_at(&twelve),
            Some(Value::Boolean(false)),
            "a row built again starts at what it inherits, not at what was set here"
        );
        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock"));
    }

    /// Reset takes one key out of what a level writes and nothing beside it: a border's width leaves its colour, and a table emptied by it goes with it, so the entry inherits the whole of it again.
    #[test]
    fn taking_a_key_out_of_an_entry_leaves_the_rest_of_it() {
        let area: Area = toml::from_str(
            "id = \"bar-top\"\nkind = \"bar\"\nthickness = 30\nstyle = { fill = \"red\", border = { width = 2.0, color = \"base\" } }\n",
        )
        .expect("an area");
        let without = popover::origin::without(&area, "style.border.width").expect("written");
        let border = without
            .style
            .border
            .clone()
            .expect("the colour is still written");
        assert_eq!(
            (border.width, border.color.as_deref()),
            (None, Some("base"))
        );
        assert_eq!(without.style.fill.as_deref(), Some("red"));

        let bare = popover::origin::without(&without, "style.border.color").expect("written");
        assert_eq!(
            bare.style.border, None,
            "an emptied table goes with its last key"
        );
        let thin = popover::origin::without(&bare, "thickness").expect("written");
        assert!(matches!(
            thin.kind,
            Some(layout::AreaKind::Bar {
                thickness: None,
                ..
            })
        ));
        assert_eq!(
            popover::origin::without(&thin, "thickness"),
            None,
            "nothing to take"
        );
        let entry = popover::origin::table_of(&thin).expect("a table");
        assert!(popover::origin::holds(&entry, "style.fill"));
        assert!(!popover::origin::holds(&entry, "style.border"));
    }

    /// The built-in layout with `thickness` on the `*` rule's bar, and a rule for the rig's screen that writes the bar as `here` says.
    fn thick(thickness: f32, here: impl FnOnce(&mut Area)) -> impl FnOnce(&mut Layout) {
        layered(
            move |bar| {
                if let Some(layout::AreaKind::Bar { thickness: own, .. }) = &mut bar.kind {
                    *own = Some(thickness);
                }
            },
            here,
        )
    }

    fn thickness_on_screen() -> Option<f32> {
        match reconcile::desktops()[0]
            .resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))?
            .kind
        {
            ResolvedAreaKind::Bar { thickness, .. } => Some(thickness),
            _ => None,
        }
    }

    /// A bar's thickness only the `*` rule writes says where it comes from; written on the screen's rule it says so and has a Reset, which takes it back to what the `*` rule gives, in the row and on the bar, as one undo entry.
    #[test]
    fn a_bars_thickness_shows_its_origin_and_resets_to_what_it_inherits() {
        let rig = rig_with("origin-thickness", thick(40.0, |_| {}));
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let tree = laid();
        let draft = popover::area_draft().expect("an area's popover");
        let every = rule(&rig, "*", None);
        assert_eq!(
            draft.provenance(&["thickness"]),
            Provenance::Inherited(every.clone())
        );
        assert!(shows(&tree, &format!("From outputs.* in {}", every.file())));
        popover::close();

        let rig = rig_with(
            "origin-thickness-reset",
            thick(40.0, |area| {
                if let Some(layout::AreaKind::Bar { thickness, .. }) =
                    popover::AreaDraft::kind_mut(area, "bar")
                {
                    *thickness = Some(48.0);
                }
            }),
        );
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        let draft = popover::area_draft().expect("an area's popover");
        assert_eq!(draft.provenance(&["thickness"]), Provenance::Here);
        assert!(draft.writes(&["thickness"]));
        assert_eq!(thickness_on_screen(), Some(48.0));

        press_below(&mut card, "Thickness", "Reset");
        let thickness = popover::shared::<f32>("thickness").expect("the thickness row");
        assert_eq!(thickness.peek(), 40.0, "the row shows what it inherits now");
        assert_eq!(thickness_on_screen(), Some(40.0), "and so does the bar");
        assert!(!draft.writes(&["thickness"]));
        assert_eq!(
            draft.provenance(&["thickness"]),
            Provenance::Inherited(rule(&rig, "*", None))
        );

        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
        assert_eq!(session::undo().as_deref(), Ok("Customize bar-top"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(thickness_on_screen(), Some(48.0));
    }

    /// A Reset made inside an event's batch, as a pressed button makes it, puts a bar that inherits the whole edge back to running it: what keeps the switch in step with the length does not take the length Reset puts back for one the user chose.
    #[test]
    fn a_reset_inside_an_event_batch_keeps_the_bar_running_the_whole_edge() {
        let _rig = rig_with(
            "reset-batched",
            layered(
                |_| {},
                |area| {
                    if let Some(layout::AreaKind::Bar { length, .. }) =
                        popover::AreaDraft::kind_mut(area, "bar")
                    {
                        *length = Some(layout::Extent::Px(800.0));
                    }
                },
            ),
        );
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let _card = Card::open();
        let draft = popover::area_draft().expect("an area's popover");
        let fills = popover::shared::<bool>("length_fill").expect("the full length switch");
        assert!(!fills.peek(), "the screen's rule runs it 800 px");
        assert!(draft.writes(&["length"]));

        telar::batch(|| draft.reset(&["length"]));

        assert!(fills.peek(), "the switch shows the whole edge it inherits");
        assert!(!draft.writes(&["length"]), "and no length is pinned here");
        assert!(!draft.is_resetting(), "the Reset is over once its batch is");
        popover::close();
    }

    /// An expression row says where its expression comes from in the same words as every other row, and Remove, which takes the inherited one back, takes the line away with it.
    #[test]
    fn an_expression_row_shows_its_origin() {
        let rig = rig_with("origin-expression", |mine| {
            mine.outputs[0]
                .layers
                .top
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "bar-top")
                .expect("the built-in layout has a top bar")
                .visible = Some(layout::Expr("1 < 2".to_string()));
            let mut screen = OutputRule {
                matches: OutputMatch(SCREEN.to_string()),
                ..OutputRule::default()
            };
            screen.layers.top.areas.push(Area {
                id: AreaId::new("bar-top"),
                ..Area::default()
            });
            mine.outputs.push(screen);
        });
        let _scope = Scope::new();
        place_bar();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        let line = format!("From outputs.* in {}", rule(&rig, "*", None).file());
        let said = |card: &Card| {
            texts(&card.tree)
                .iter()
                .filter(|(text, _)| *text == line)
                .count()
        };
        let before = said(&card);

        press_below(&mut card, "Shown while", "Remove");
        assert_eq!(
            said(&card),
            before - 1,
            "taken back here, the expression no longer comes from there"
        );
        popover::close();
    }
}
