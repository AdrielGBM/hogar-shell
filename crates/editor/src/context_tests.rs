//! Context menus through the hooks the surfaces call: a secondary press on a real bar, built as its window builds it, opens the menu of what is under the pointer; the menu is kept on the screen wherever it was asked for; and its rows edit the layout as one undo entry each.
//!
//! The windows of a headless shell are never built, so each test builds the area it presses on itself, the way the window would, and the menu's tree as the transient would.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, Paint, PointerButton, PointerSource, Rect, RectStyle,
        StyledContainer, compute_layout, use_theme,
    };

    use config::theme::NordTheme;
    use config::{Edge, Shape};
    use layout::{
        AreaId, AreaKind, Group, GroupId, GroupKind, Instance, InstanceId, LayerKind, Layout,
        Representation, ResolvedArea, ResolvedAreaKind,
    };
    use surfaces::area::Surround;
    use surfaces::menu::{Asked, Pointed};
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{
        ActionDef, Built, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef,
    };
    use ui::host::{Audience, Host, WidgetSize};

    use crate::context;
    use crate::keys::{self, Press};
    use crate::mode::{self, Compositor};
    use crate::popover;
    use crate::popover::handles::Corner;
    use crate::rig::{Rig, SCREEN, rig, rig_with};
    use crate::session::{self, Selection};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    fn face(_: &Host) -> Built {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(40.0).height(20.0),
            |_| RectStyle::default(),
            Vec::new(),
        )?))
    }

    static PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        category: ui::descriptor::Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(face, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &[WidgetSize::M],
                build: face,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[
            ActionDef {
                id: "toggle",
                command: "dashboard toggle",
            },
            ActionDef {
                id: "next",
                command: "media next",
            },
            ActionDef {
                id: "previous",
                command: "media previous",
            },
        ],
        sources: &[],
    }];

    /// An owner for what a test builds, disposed when the test ends.
    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    fn clock() -> Node {
        bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))
    }

    /// `built` laid out over the whole screen, under the root every layer window has.
    fn screen(built: Built) -> ComponentList {
        let page = LayoutStyle::new().width(SIZE.0).height(SIZE.1);
        let root = Container::new(
            page,
            vec![Box::new(Pointed::new(built.expect("it builds"))) as Box<dyn LayoutItem>],
        )
        .expect("a page");
        let node = root.layout_node();
        let tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(SIZE.0),
            AvailableSpace::Definite(SIZE.1),
        )
        .expect("it lays out");
        tree
    }

    /// The layer's area `id` as the screen resolves it now.
    fn resolved(layer: LayerKind, id: &str) -> ResolvedArea {
        reconcile::desktops()[0]
            .resolved
            .layer(layer)
            .and_then(|layer| layer.areas.iter().find(|area| area.id.as_str() == id))
            .cloned()
            .unwrap_or_else(|| panic!("{id} is on screen"))
    }

    /// `area` built as the top window builds it.
    fn built(area: &ResolvedArea, audience: Audience) -> ComponentList {
        let desktop = reconcile::desktops()[0].clone();
        let config = Arc::clone(&desktop.config);
        let surround = Surround {
            config: &config,
            theme: config.resolve_theme(),
            output: Some(SCREEN),
            layer: LayerKind::Top,
            bounds: Rect::new(0.0, 0.0, SIZE.0, SIZE.1),
            reserved: desktop.reserved,
            audience,
        };
        screen(surfaces::area::build(area, surround).expect("a bar has a builder"))
    }

    /// As the runner does: the keyboard's state first, then the overlays and the dismiss stack, then the tree.
    fn route(tree: &mut ComponentList, event: &Event) {
        telar::observe_keyboard(event);
        if !telar::dispatch_overlays(event) {
            tree.on_event(event);
        }
    }

    fn click(tree: &mut ComponentList, (x, y): (f32, f32), button: PointerButton) {
        let (x, y) = (f64::from(x), f64::from(y));
        route(
            tree,
            &Event::PointerMoved {
                x,
                y,
                source: PointerSource::Mouse,
            },
        );
        for pressed in [true, false] {
            let event = match pressed {
                true => Event::PointerPressed {
                    x,
                    y,
                    button,
                    source: PointerSource::Mouse,
                },
                false => Event::PointerReleased {
                    x,
                    y,
                    button,
                    source: PointerSource::Mouse,
                },
            };
            route(tree, &event);
        }
    }

    fn drag(tree: &mut ComponentList, from: (f32, f32), to: (f32, f32)) {
        let at = |(x, y): (f32, f32)| (f64::from(x), f64::from(y));
        let ((fx, fy), (tx, ty)) = (at(from), at(to));
        route(
            tree,
            &Event::PointerPressed {
                x: fx,
                y: fy,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            },
        );
        route(
            tree,
            &Event::PointerMoved {
                x: tx,
                y: ty,
                source: PointerSource::Mouse,
            },
        );
        route(
            tree,
            &Event::PointerReleased {
                x: tx,
                y: ty,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            },
        );
    }

    fn key(key: Key, modifiers: ModifiersState) -> Event {
        Event::KeyPressed { key, modifiers }
    }

    fn named(named: NamedKey) -> Event {
        key(Key::Named(named), ModifiersState::default())
    }

    /// The window root an overlay is laid out against, as a layer window's root is: a menu is an overlay, carried by the window rather than by where it was declared.
    struct Window {
        root: telar::NodeId,
        _tree: ComponentList,
    }

    impl Window {
        fn new() -> Self {
            let page = Container::new(LayoutStyle::new().width(SIZE.0).height(SIZE.1), Vec::new())
                .expect("a page");
            let root = page.layout_node();
            telar::set_overlay_host(root);
            let window = Self {
                root,
                _tree: ComponentList::new(page),
            };
            window.lay_out();
            window
        }

        fn lay_out(&self) {
            compute_layout(
                self.root,
                AvailableSpace::Definite(SIZE.0),
                AvailableSpace::Definite(SIZE.1),
            )
            .expect("the window lays out");
        }

        /// The open menu's tree, laid out in this window.
        fn menu(&self) -> Menu {
            let owner = telar::owner_scope();
            let item = context::built()
                .expect("a menu is open")
                .expect("its tree builds");
            self.lay_out();
            let tree = ComponentList::new(item);
            Menu {
                tree: Some(tree),
                owner: owner.id(),
            }
        }
    }

    /// A menu's tree under an owner of its own, gone with it as the transient's row would take it: the tree's own effects keep it alive until then, and a menu left alive would still take every press in the window.
    struct Menu {
        tree: Option<ComponentList>,
        owner: telar::OwnerId,
    }

    impl std::ops::Deref for Menu {
        type Target = ComponentList;

        fn deref(&self) -> &ComponentList {
            self.tree
                .as_ref()
                .expect("the tree is there until the menu goes")
        }
    }

    impl std::ops::DerefMut for Menu {
        fn deref_mut(&mut self) -> &mut ComponentList {
            self.tree
                .as_mut()
                .expect("the tree is there until the menu goes")
        }
    }

    impl Drop for Menu {
        fn drop(&mut self) {
            self.tree.take();
            telar::dispose_owner(self.owner);
        }
    }

    /// The open popover's tree, laid out over the whole screen.
    fn popover_tree() -> ComponentList {
        screen(popover::tree().expect("a popover is open"))
    }

    /// Where the open menu's panel is drawn: the one box painted in its background.
    fn panel(tree: &ComponentList) -> Rect {
        let theme = use_theme::<NordTheme>();
        tree.commands()
            .iter()
            .find_map(|command| match command {
                DrawCommand::Rect { rect, style }
                    if style.fill == Some(Paint::Solid(theme.surface)) =>
                {
                    Some(*rect)
                }
                _ => None,
            })
            .expect("the menu draws its panel")
    }

    /// The middle of the open menu's row that says `label`: its rows are 28 px each, inside the panel's 4 px of padding.
    fn row(tree: &ComponentList, label: &str) -> (f32, f32) {
        let at = context::rows()
            .iter()
            .position(|row| row == label)
            .unwrap_or_else(|| panic!("{label:?} is a row: {:?}", context::rows()));
        let drawn = panel(tree);
        (
            drawn.x + drawn.width / 2.0,
            drawn.y + 4.0 + 28.0 * at as f32 + 14.0,
        )
    }

    fn centre(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    fn is_on_screen(rect: Rect) -> bool {
        rect.x >= 0.0
            && rect.y >= 0.0
            && rect.x + rect.width <= SIZE.0
            && rect.y + rect.height <= SIZE.1
    }

    fn corners_on_screen() -> Option<layout::Corners> {
        match resolved(LayerKind::Top, "bar-top").kind {
            ResolvedAreaKind::Bar { shape, .. } => shape.radius,
            _ => None,
        }
    }

    fn opened(node: Node, at: (f32, f32)) {
        context::open(Asked {
            node,
            window: LayerKind::Top,
            at: Some(at),
        })
        .expect("the menu opens");
    }

    /// DEC-3 end to end, through the hooks a window's tree calls: a secondary press on the bar's empty space opens its menu, "Customize bar…" opens its popover, the corner handle rounds the real bar live, Esc puts back exactly what was on screen — and the same way round again, a click outside keeps it as one entry in the history.
    #[test]
    fn a_secondary_press_on_the_bar_customizes_it_live_and_escape_or_a_click_outside_decides() {
        let rig = rig("menu-dec3");
        let _scope = Scope::new();
        let window = Window::new();
        let mut bar_tree = built(&resolved(LayerKind::Top, "bar-top"), Audience::Owner);
        let strip = rects::rect(&bar()).expect("the bar is registered where it was laid out");
        let empty = (strip.x + strip.width * 0.25, strip.y + strip.height / 2.0);
        let before = reconcile::desktops();

        for decided_by_escape in [true, false] {
            click(&mut bar_tree, empty, PointerButton::Secondary);
            assert!(
                transient::is_open(context::ID),
                "the secondary press opened a menu"
            );
            assert_eq!(
                transient::drawn_in(context::ID).map(|window| window.layer),
                Some(LayerKind::Top),
                "in the bar's own window (DEC-9)"
            );
            let mut menu = window.menu();
            let customize = row(&menu, "Customize bar…");
            click(&mut menu, customize, PointerButton::Primary);
            drop(menu);
            assert!(
                !transient::is_open(context::ID),
                "picking a row closes the menu"
            );
            assert_eq!(
                popover::current(),
                Some(bar()),
                "and opens the bar's popover"
            );

            let mut handles = popover_tree();
            let radius = popover::shared::<f32>("radius.top_left")
                .expect("the corner")
                .peek();
            let start = Corner::TopLeft.point(strip, radius);
            drag(&mut handles, start, (strip.x + 6.0, strip.y + 6.0));
            assert_eq!(
                corners_on_screen(),
                Some(layout::Corners::all(6.0)),
                "the real bar follows the handle"
            );
            if decided_by_escape {
                route(&mut handles, &named(NamedKey::Escape));
                assert!(
                    std::rc::Rc::ptr_eq(&reconcile::desktops(), &before),
                    "Esc puts back the very arrangement that was on screen"
                );
                assert_eq!(rig.undo_label(), None);
            } else {
                transient::close(popover::ID);
                assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
                assert_eq!(corners_on_screen(), Some(layout::Corners::all(6.0)));
            }
            assert_eq!(popover::current(), None);
        }
    }

    /// F-10.41 and T-6.4: wherever it is asked for — any corner or edge of the screen — the menu opens where it fits and never past an edge, however many rows it has.
    #[test]
    fn the_menu_opens_on_the_screen_at_every_corner_and_edge() {
        let _rig = rig("menu-corners");
        let _scope = Scope::new();
        let window = Window::new();
        let (right, bottom) = (SIZE.0 - 1.0, SIZE.1 - 1.0);
        let asked = [
            (0.0, 0.0),
            (right, 0.0),
            (0.0, bottom),
            (right, bottom),
            (SIZE.0 / 2.0, 0.0),
            (SIZE.0 / 2.0, bottom),
            (0.0, SIZE.1 / 2.0),
            (right, SIZE.1 / 2.0),
        ];
        for node in [bar(), clock()] {
            for at in asked {
                opened(node.clone(), at);
                let rows = context::rows().len();
                let drawn = panel(&window.menu());
                assert!(
                    is_on_screen(drawn),
                    "{node:?} asked at {at:?}: {rows} rows drawn at {drawn:?}"
                );
                assert!(
                    drawn.height >= rows as f32 * 28.0,
                    "every row is inside the panel: {drawn:?}"
                );
                if at.0 + drawn.width <= SIZE.0 {
                    assert_eq!(
                        drawn.x, at.0,
                        "where it fits across, it opens at the pointer"
                    );
                }
                if at.1 + drawn.height <= SIZE.1 {
                    assert_eq!(drawn.y, at.1, "where it fits down, it opens at the pointer");
                }
                transient::close(context::ID);
            }
        }
    }

    /// TA-4: right-click reaches every chip of a bar on every edge and in every shape mode, and opens that chip's menu on the screen; the bar's own empty space opens the bar's where the strip is painted.
    #[test]
    fn right_click_opens_a_chips_menu_on_every_edge_and_in_every_shape_mode() {
        let _rig = rig("menu-edges");
        let _scope = Scope::new();
        let window = Window::new();
        for edge in Edge::ALL {
            for mode in [Shape::Bar, Shape::Sections, Shape::Chips] {
                let mut area = resolved(LayerKind::Top, "bar-top");
                if let ResolvedAreaKind::Bar {
                    edge: at, shape, ..
                } = &mut area.kind
                {
                    *at = edge;
                    shape.mode = Some(mode);
                }
                let own = telar::owner_scope();
                let mut tree = built(&area, Audience::Owner);
                let chip = rects::rect(&clock()).expect("the clock chip is registered");
                click(&mut tree, centre(chip), PointerButton::Secondary);
                assert!(
                    transient::is_open(context::ID),
                    "{edge:?} {mode:?}: the clock's menu opened"
                );
                assert!(
                    context::rows().contains(&"Customize Clock…".to_string()),
                    "{edge:?} {mode:?}: it is the clock's: {:?}",
                    context::rows()
                );
                assert!(is_on_screen(panel(&window.menu())), "{edge:?} {mode:?}");
                transient::close(context::ID);
                drop(tree);
                let id = own.id();
                drop(own);
                telar::dispose_owner(id);
            }
        }
    }

    /// The instance menu offers the module's own actions, customizing it, moving it where it is drawn the other way, removing it and editing its layer; the bar's offers what acts on the bar itself, outside its mode too.
    #[test]
    fn an_instances_menu_offers_its_actions_and_the_layout_rows() {
        let _rig = rig_with("menu-rows", with_desktop_grid);
        let _scope = Scope::new();
        opened(clock(), (900.0, 17.0));
        assert_eq!(
            context::rows(),
            [
                "Open or close",
                "Next",
                "Previous",
                "Customize Clock…",
                "Move to desktop as widget",
                "Remove",
                "Edit Top…",
            ]
        );
        opened(bar(), (400.0, 17.0));
        assert_eq!(
            context::rows(),
            ["Customize bar…", "Split in half", "Remove", "Edit Top…"]
        );
    }

    /// A grid on the desktop in place of the shipped one, its first cells taken by an empty group.
    fn with_desktop_grid(layout: &mut Layout) {
        let rule = &mut layout.outputs[0];
        rule.layers.desktop.areas = vec![layout::Area {
            id: AreaId::new("widgets"),
            kind: Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.1,
                    w: 1.0,
                    h: 0.9,
                }),
                cell: Some(80.0),
                gap: Some(16.0),
                anchor: Some(layout::Anchor::TopLeft),
            }),
            groups: vec![Group {
                id: GroupId::new("board"),
                kind: Some(GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 4,
                    row_span: 2,
                }),
                ..Group::default()
            }],
            ..layout::Area::default()
        }];
        for group in &mut rule.layers.top.areas[0].groups {
            for child in &mut group.children {
                if child.id.as_str() == "clock" {
                    child.options.insert("format".into(), "%H:%M".into());
                }
            }
        }
    }

    /// The group `group` of the desktop area `area` as the screen shows it.
    fn shown_group(area: &AreaId, group: &GroupId) -> Option<layout::ResolvedGroup> {
        reconcile::desktops()[0]
            .resolved
            .layer(LayerKind::Desktop)?
            .areas
            .iter()
            .find(|held| held.id == *area)?
            .groups
            .iter()
            .find(|held| held.id == *group)
            .cloned()
    }

    /// Where the instance `id` is written, and as what, in the active layout.
    fn written(rig: &Rig, id: &str) -> Option<(AreaId, GroupId, Instance)> {
        let store = rig.store.borrow();
        store.active().outputs.iter().find_map(|rule| {
            rule.layers.each().into_iter().find_map(|(_, layer)| {
                layer.areas.iter().find_map(|area| {
                    area.groups.iter().find_map(|group| {
                        group
                            .children
                            .iter()
                            .find(|child| child.id.as_str() == id)
                            .map(|child| (area.id.clone(), group.id.clone(), child.clone()))
                    })
                })
            })
        })
    }

    /// TA-3: a chip moved to the desktop becomes a widget there and a widget moved to a bar becomes a chip, keeping its id and options, each as one undo entry.
    #[test]
    fn converting_a_chip_to_a_widget_and_back_keeps_its_id_and_options() {
        let rig = rig_with("menu-convert", with_desktop_grid);
        let _scope = Scope::new();
        let (_, _, before) = written(&rig, "clock").expect("the clock is on the bar");

        opened(clock(), (900.0, 17.0));
        context::pick("Move to desktop as widget");
        let (area, group, widget) = written(&rig, "clock").expect("the clock is still placed");
        assert_eq!(
            (area.as_str(), group.as_str()),
            ("widgets", "clock"),
            "a widget of its own on the grid, on the free cells nearest its first"
        );
        let placed = shown_group(&area, &group).expect("its group is on the grid");
        assert!(
            matches!(placed.kind, GroupKind::Cell { col: 0, row: 2, .. }),
            "{:?}",
            placed.kind
        );
        assert_eq!(widget.representation, Some(Representation::WidgetM));
        assert_eq!(widget.options, before.options, "it keeps its options");
        assert_eq!(rig.undo_label().as_deref(), Some("Move Clock to desktop"));

        let on_desktop =
            Node::area(Some(SCREEN), LayerKind::Desktop, &area).instance(&group, &widget.id);
        opened(on_desktop, (100.0, 200.0));
        context::pick("Move to bar as chip");
        let (area, _, chip) = written(&rig, "clock").expect("the clock is back");
        assert_eq!(area.as_str(), "bar-top");
        assert_eq!(chip.representation, Some(Representation::Chip));
        assert_eq!((chip.id, chip.options), (before.id, before.options));

        assert_eq!(session::undo().as_deref(), Ok("Move Clock to bar"));
        assert_eq!(
            written(&rig, "clock")
                .map(|(area, ..)| area.to_string())
                .as_deref(),
            Some("widgets")
        );
    }

    /// "Remove" takes the instance out as one undo entry, and undo puts it back where it was.
    #[test]
    fn remove_takes_the_instance_out_and_undo_puts_it_back() {
        let rig = rig("menu-remove");
        let _scope = Scope::new();
        let before = written(&rig, "clock");
        opened(clock(), (900.0, 17.0));
        context::pick("Remove");
        assert_eq!(written(&rig, "clock"), None);
        assert_eq!(rig.undo_label().as_deref(), Some("Remove Clock"));
        assert_eq!(session::undo().as_deref(), Ok("Remove Clock"));
        assert_eq!(written(&rig, "clock"), before);
    }

    /// TA-7: a placeholder's menu fixes the layout rather than config — remove, and reset to what the layout it extends (here the built-in one) has there.
    #[test]
    fn a_placeholders_menu_removes_it_or_resets_it_from_the_base_layout() {
        let rig = rig_with("menu-fix", |layout| {
            for group in &mut layout.outputs[0].layers.top.areas[0].groups {
                for child in &mut group.children {
                    if child.id.as_str() == "clock" {
                        child.module = Some("clokc".to_string());
                    }
                }
            }
        });
        let _scope = Scope::new();
        opened(clock(), (900.0, 17.0));
        assert_eq!(context::rows(), ["Remove", "Reset", "Edit Top…"]);
        context::pick("Reset");
        let (_, _, fixed) = written(&rig, "clock").expect("the clock is placed");
        assert_eq!(
            fixed.module.as_deref(),
            Some("clock"),
            "reset to the built-in clock"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Reset clokc"));
    }

    /// TA-8: nothing on the lock layer offers a menu — not an item, not an area, not the lock preview an edit mode draws.
    #[test]
    fn nothing_on_the_lock_layer_offers_a_menu() {
        let _rig = rig("menu-lock");
        let _scope = Scope::new();
        let reading = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-readings"));
        assert!(surfaces::menu::on(reading.clone(), Audience::Owner).is_none());
        assert!(surfaces::menu::on(bar(), Audience::Anyone).is_none());
        assert!(
            context::open(Asked {
                node: reading,
                window: LayerKind::Overlay,
                at: Some((100.0, 100.0)),
            })
            .is_err()
        );

        let desktop = reconcile::desktops()[0].clone();
        let lock = surfaces::layouts::read(|store| {
            modules::lock::LockLayout::of(store.active(), store.all())
        })
        .expect("a store");
        let mut preview = screen(modules::lock::preview(
            &desktop.config,
            Ok(&lock),
            Some(SCREEN),
            SIZE,
        ));
        for at in [
            (100.0, 100.0),
            (SIZE.0 / 2.0, SIZE.1 / 2.0),
            (1800.0, 1000.0),
        ] {
            click(&mut preview, at, PointerButton::Secondary);
        }
        assert!(!transient::is_open(context::ID));

        let mut top_as_lock = built(&resolved(LayerKind::Top, "bar-top"), Audience::Anyone);
        let chip = rects::rect(&clock()).expect("the chip is registered");
        click(&mut top_as_lock, centre(chip), PointerButton::Secondary);
        assert!(
            !transient::is_open(context::ID),
            "nothing built for anyone offers one"
        );
    }

    /// Never pointer-only: in an edit mode the menu key, and Shift+F10, open the menu of what is selected, and the arrows and Enter pick from it.
    #[test]
    fn the_menu_key_opens_the_selected_items_menu_and_the_keyboard_picks_from_it() {
        let _rig = rig("menu-key");
        let _scope = Scope::new();
        let window = Window::new();
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
        assert!(session::select(Selection::Instance(clock())));

        assert!(keys::press_as(
            &Key::Named(NamedKey::ContextMenu),
            ModifiersState::default(),
            Press::First
        ));
        assert!(transient::is_open(context::ID));
        assert_eq!(
            transient::drawn_in(context::ID).map(|window| window.layer),
            Some(LayerKind::Overlay),
            "over the mode's host"
        );
        assert!(
            !context::rows().iter().any(|row| row == "Edit Top…"),
            "Top is being edited already"
        );
        transient::close(context::ID);

        let shift = ModifiersState {
            is_shift: true,
            ..ModifiersState::default()
        };
        assert!(keys::press_as(
            &Key::Named(NamedKey::F10),
            shift,
            Press::First
        ));
        let mut menu = window.menu();
        for _ in 0..4 {
            route(&mut menu, &named(NamedKey::ArrowDown));
        }
        route(&mut menu, &named(NamedKey::Enter));
        assert_eq!(
            popover::current(),
            Some(clock()),
            "the fourth row customizes the clock"
        );
        popover::close();
        mode::leave();
    }

    /// B10, TA-4: an area's menu offers what acts on the area itself in and out of edit mode, and what acts on the mode — a new bar, stack or grid, the palette — only in that area's own mode on its screen.
    #[test]
    fn a_menu_offers_the_modes_own_rows_only_in_that_mode() {
        let _rig = rig_with("menu-modes", with_desktop_grid);
        let _scope = Scope::new();
        let area = |layer, id: &str| Node::area(Some(SCREEN), layer, &AreaId::new(id));
        let cases: [(LayerKind, &str, &[&str], &[&str]); 4] = [
            (
                LayerKind::Background,
                "background",
                &["Split side by side", "Add a texture over it"],
                &[],
            ),
            (
                LayerKind::Desktop,
                "widgets",
                &[],
                &["Add widget…", "New grid"],
            ),
            (
                LayerKind::Top,
                "bar-top",
                &["Split in half"],
                &["New bar at the bottom"],
            ),
            (
                LayerKind::Overlay,
                "stack",
                &["Open the launcher here"],
                &["New stack"],
            ),
        ];
        for (layer, id, always, in_mode) in cases {
            for editing in [false, true] {
                if editing {
                    mode::enter_as(
                        layer,
                        Some(SCREEN),
                        &Compositor {
                            restack: true,
                            locked: false,
                            lockable: Ok(()),
                        },
                    )
                    .expect("the mode opens");
                }
                opened(area(layer, id), (10.0, 10.0));
                let rows = context::rows();
                for row in always {
                    assert!(
                        rows.contains(&row.to_string()),
                        "{layer} {editing}: {rows:?}"
                    );
                }
                for row in in_mode {
                    assert_eq!(
                        rows.contains(&row.to_string()),
                        editing,
                        "{layer} {editing}: {row} in {rows:?}"
                    );
                }
                transient::close(context::ID);
                mode::leave();
            }
        }
    }
}
