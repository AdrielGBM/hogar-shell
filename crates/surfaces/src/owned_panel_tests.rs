//! The panel the layout gives an instance: opened beside its owner or along its bar on every edge, kept across a reload while the layout still gives it, and repainting nothing outside its own box.

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use config::{Config, Edge};
    use layout::{
        AreaId, GroupKind, InstanceId, LayerKind, Representation, Resolved, ResolvedArea,
        ResolvedAreaKind, ResolvedLayer, Within, Zone,
    };
    use telar::{
        App, ComponentList, DrawCommand, Event, PointerButton, PointerSource, Rect, set_theme,
    };
    use ui::descriptor::{
        Category, ChipDef, FieldDef, FieldType, Input, ModuleDescriptor, PanelDef, Privacy,
        Reading, Representations, Sink, SourceDef, WidgetDef,
    };

    use crate::layer_window::{LayerApp, Reserved, Screen, WindowAreas};
    use crate::panel::Owner;
    use crate::reconcile::Desktop;
    use crate::rects::Node;
    use crate::test_rig::{SCREEN, area, bar_kind, cell, grid_kind, group, instance};
    use crate::transient;

    const WIDE: u32 = 800;
    const HIGH: u32 = 600;

    fn probe(_: &ui::host::Host) -> ui::descriptor::Built {
        Ok(Box::new(telar::Container::new(
            telar::LayoutStyle::new().width(24.0).height(24.0),
            Vec::new(),
        )?))
    }

    const PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "probe",
        name: "probe",
        icon: "circle",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(probe, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &ui::host::WidgetSize::ALL,
                build: probe,
                input: Input::ReadOnly,
            }),
            panel: Some(PanelDef::new(probe, Input::ReadOnly)),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[GATE],
    }];

    fn bar(edge: Edge) -> ResolvedArea {
        ResolvedArea {
            reserve: true,
            ..area(
                "bar",
                bar_kind(edge),
                vec![group(
                    "start",
                    GroupKind::Zone { zone: Zone::Start },
                    vec![instance("owner", "probe", Representation::Chip)],
                )],
            )
        }
    }

    fn desktop_grid() -> ResolvedArea {
        area(
            "widgets",
            grid_kind(),
            vec![group(
                "owner",
                cell(2, 1),
                vec![instance("owner", "probe", Representation::WidgetS)],
            )],
        )
    }

    fn panel(along: bool) -> ResolvedArea {
        area(
            "panel",
            ResolvedAreaKind::Panel {
                owner: InstanceId::new("owner"),
                along,
                cols: 3,
                rows: 2,
                cell: 40.0,
                gap: 8.0,
            },
            vec![group(
                "inside",
                cell(0, 0),
                vec![instance("inside", "probe", Representation::WidgetS)],
            )],
        )
    }

    fn quiet() -> Arc<Config> {
        let mut config = Config::default();
        config.animation.enabled = false;
        Arc::new(config)
    }

    fn owner() -> Owner {
        Owner {
            output: Some(SCREEN.to_string()),
            instance: InstanceId::new("owner"),
        }
    }

    struct Shown {
        app: LayerApp,
        tree: ComponentList,
        layer: LayerKind,
    }

    impl Shown {
        fn frame(&self) -> Vec<DrawCommand> {
            telar::relayout_if_dirty();
            self.tree.commands().to_vec()
        }

        fn panel_rect(&self) -> Rect {
            crate::rects::rect(&Node::area(Some(SCREEN), self.layer, &AreaId::new("panel")))
                .expect("the panel is drawn")
        }

        fn owner_rect(&self) -> Rect {
            crate::rects::instance(Some(SCREEN), &InstanceId::new("owner"))
                .expect("the owner is drawn")
                .1
        }

        fn press(&mut self, (x, y): (f32, f32)) {
            let (x, y) = (f64::from(x), f64::from(y));
            for event in [
                Event::PointerMoved {
                    x,
                    y,
                    source: PointerSource::Mouse,
                },
                Event::PointerPressed {
                    x,
                    y,
                    button: PointerButton::Primary,
                    source: PointerSource::Mouse,
                },
                Event::PointerReleased {
                    x,
                    y,
                    button: PointerButton::Primary,
                    source: PointerSource::Mouse,
                },
            ] {
                self.tree.on_event(&event);
            }
            self.frame();
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            let at = |(x, y): (f32, f32)| (f64::from(x), f64::from(y));
            let ((x, y), (to_x, to_y)) = (at(from), at(to));
            let steps = 8;
            let mut events = vec![
                Event::PointerMoved {
                    x,
                    y,
                    source: PointerSource::Mouse,
                },
                Event::PointerPressed {
                    x,
                    y,
                    button: PointerButton::Primary,
                    source: PointerSource::Mouse,
                },
            ];
            events.extend((1..=steps).map(|step| {
                let part = f64::from(step) / f64::from(steps);
                Event::PointerMoved {
                    x: x + (to_x - x) * part,
                    y: y + (to_y - y) * part,
                    source: PointerSource::Mouse,
                }
            }));
            events.push(Event::PointerReleased {
                x: to_x,
                y: to_y,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
            for event in events {
                self.tree.on_event(&event);
            }
            self.frame();
        }
    }

    /// A chip pulled away from its bar opens the panel its instance owns, which the press finds by where the bar placed the chip rather than by where it is drawn.
    #[test]
    fn a_chip_pulled_off_its_bar_opens_the_panel_its_instance_owns() {
        let mut shown = show(LayerKind::Top, vec![bar(Edge::Top), panel(false)]);
        let (x, y) = centre(shown.owner_rect());
        assert!(!crate::panel::is_owned_open(&owner()));
        shown.drag((x, y), (x, y + 120.0));
        assert!(
            crate::panel::is_owned_open(&owner()),
            "the pull opens the owner's panel"
        );
        transient::close_all();
    }

    fn centre(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    fn resolved(layer: LayerKind, areas: Vec<ResolvedArea>) -> Resolved {
        Resolved::of(SCREEN, [(layer, ResolvedLayer { areas })])
    }

    fn publish(resolved: &Resolved, config: &Arc<Config>) {
        crate::reconcile::publish(&[Desktop {
            output: Some(SCREEN.to_string()),
            config: Arc::clone(config),
            resolved: resolved.clone(),
            reserved: Reserved::of(resolved, config),
            size: (WIDE as f32, HIGH as f32),
        }]);
    }

    fn show(layer: LayerKind, areas: Vec<ResolvedArea>) -> Shown {
        show_with(layer, areas, quiet())
    }

    fn show_with(layer: LayerKind, areas: Vec<ResolvedArea>, config: Arc<Config>) -> Shown {
        telar::reset_layout_runtime();
        config::set_output_config(SCREEN, Arc::clone(&config));
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        transient::close_all();
        let resolved = resolved(layer, areas);
        publish(&resolved, &config);
        let app = LayerApp::standing(
            layer,
            WindowAreas::of(&resolved, layer),
            Arc::clone(&config),
            Screen {
                size: (WIDE as f32, HIGH as f32),
                reserved: Reserved::of(&resolved, &config),
            },
            Rc::new(crate::area::ShellAreas),
        );
        let tree = telar::testing::mount(app.root(), WIDE, HIGH);
        let shown = Shown { app, tree, layer };
        shown.frame();
        shown
    }

    fn inside(rect: &Rect, bounds: Rect) -> bool {
        rect.x >= bounds.x - 0.5
            && rect.y >= bounds.y - 0.5
            && rect.x + rect.width <= bounds.x + bounds.width + 0.5
            && rect.y + rect.height <= bounds.y + bounds.height + 0.5
    }

    fn overlaps(rect: &Rect, other: Rect) -> bool {
        rect.x < other.x + other.width
            && other.x < rect.x + rect.width
            && rect.y < other.y + other.height
            && other.y < rect.y + rect.height
    }

    fn assert_confined(after: &[DrawCommand], before: &[DrawCommand], bounds: Rect, what: &str) {
        let damaged = telar::testing::damage(after, before)
            .unwrap_or_else(|| panic!("{what}: the whole surface was damaged"));
        assert!(
            damaged
                .iter()
                .any(|rect| rect.width > 0.0 && rect.height > 0.0),
            "{what}: nothing was repainted"
        );
        assert!(
            damaged.iter().all(|rect| inside(rect, bounds)),
            "{what}: damage {damaged:?} runs past the panel at {bounds:?}"
        );
    }

    /// A panel opens beside its owner on every edge, and along the whole bar when it asks to, and opening or closing it repaints nothing outside its own box.
    #[test]
    fn a_panel_on_each_bar_edge_opens_and_closes_repainting_only_its_own_box() {
        for edge in Edge::ALL {
            for along in [false, true] {
                let what = format!("{edge:?} along={along}");
                let shown = show(LayerKind::Top, vec![bar(edge), panel(along)]);
                let closed = shown.frame();
                assert!(crate::panel::open_owned(&owner()), "{what}: it has one");
                let opened = shown.frame();
                let rect = shown.panel_rect();
                let strip = crate::rects::rect(&Node::area(
                    Some(SCREEN),
                    LayerKind::Top,
                    &AreaId::new("bar"),
                ))
                .expect("the bar is drawn");
                assert!(rect.width > 0.0 && rect.height > 0.0, "{what}: {rect:?}");
                let off_the_bar = match edge {
                    Edge::Top => rect.y >= strip.y + strip.height,
                    Edge::Bottom => rect.y + rect.height <= strip.y,
                    Edge::Left => rect.x >= strip.x + strip.width,
                    Edge::Right => rect.x + rect.width <= strip.x,
                };
                assert!(off_the_bar, "{what}: {rect:?} over the bar at {strip:?}");
                if along {
                    let length = match edge.is_vertical() {
                        true => rect.height,
                        false => rect.width,
                    };
                    let side = match edge.is_vertical() {
                        true => HIGH as f32,
                        false => WIDE as f32,
                    };
                    assert_eq!(length, side, "{what}: along the whole bar");
                }
                assert_confined(&opened, &closed, rect, &format!("{what} opening"));

                crate::panel::close_owned(&owner());
                let gone = shown.frame();
                assert!(!crate::panel::is_owned_open(&owner()), "{what}");
                assert_confined(&gone, &opened, rect, &format!("{what} closing"));
            }
        }
    }

    fn bar_in(edge: Edge, mode: config::Shape) -> ResolvedArea {
        let mut area = bar(edge);
        let ResolvedAreaKind::Bar { shape, .. } = &mut area.kind else {
            unreachable!("a bar");
        };
        shape.mode = Some(mode);
        area
    }

    fn show_sized(layer: LayerKind, areas: Vec<ResolvedArea>, size: (u32, u32)) -> Shown {
        telar::reset_layout_runtime();
        let config = quiet();
        config::set_output_config(SCREEN, Arc::clone(&config));
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        transient::close_all();
        let resolved = resolved(layer, areas);
        crate::reconcile::publish(&[Desktop {
            output: Some(SCREEN.to_string()),
            config: Arc::clone(&config),
            resolved: resolved.clone(),
            reserved: Reserved::of(&resolved, &config),
            size: (size.0 as f32, size.1 as f32),
        }]);
        let app = LayerApp::standing(
            layer,
            WindowAreas::of(&resolved, layer),
            Arc::clone(&config),
            Screen {
                size: (size.0 as f32, size.1 as f32),
                reserved: Reserved::of(&resolved, &config),
            },
            Rc::new(crate::area::ShellAreas),
        );
        let tree = telar::testing::mount(app.root(), size.0, size.1);
        let shown = Shown { app, tree, layer };
        shown.frame();
        shown
    }

    /// Beside its owner or along its bar, on every edge and monitor, in `bar`, `sections` and `chips`, a panel opens on the screen, clear of the bar and of its owner, and an `along` one runs the bar's whole length.
    #[test]
    fn a_panel_opens_clear_of_its_bar_on_every_edge_shape_and_monitor() {
        for size in [(1920, 1080), (1080, 1920)] {
            let screen = Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32);
            for mode in [
                config::Shape::Bar,
                config::Shape::Sections,
                config::Shape::Chips,
            ] {
                for edge in Edge::ALL {
                    for along in [false, true] {
                        let what = format!("{edge:?} {mode:?} along={along} on {size:?}");
                        let scope = telar::owner_scope();
                        let owner_scope = scope.id();
                        let shown = show_sized(
                            LayerKind::Top,
                            vec![bar_in(edge, mode), panel(along)],
                            size,
                        );
                        assert!(crate::panel::open_owned(&owner()), "{what}: it has one");
                        shown.frame();
                        let rect = shown.panel_rect();
                        let chip = shown.owner_rect();
                        let strip = crate::rects::rect(&Node::area(
                            Some(SCREEN),
                            LayerKind::Top,
                            &AreaId::new("bar"),
                        ))
                        .expect("the bar is drawn");
                        assert!(inside(&rect, screen), "{what}: {rect:?} is off the screen");
                        assert!(
                            !overlaps(&rect, strip),
                            "{what}: {rect:?} covers the bar at {strip:?}"
                        );
                        if along {
                            let (length, whole) = match edge.is_vertical() {
                                true => (rect.height, strip.height),
                                false => (rect.width, strip.width),
                            };
                            assert_eq!(length, whole, "{what}: along the whole bar");
                        } else {
                            assert!(
                                !overlaps(&rect, chip),
                                "{what}: {rect:?} covers its owner at {chip:?}"
                            );
                            let (owner_at, from, to) = match edge.is_vertical() {
                                true => (centre(chip).1, rect.y, rect.y + rect.height),
                                false => (centre(chip).0, rect.x, rect.x + rect.width),
                            };
                            assert!(
                                from - 0.5 <= owner_at && owner_at <= to + 0.5,
                                "{what}: {rect:?} is not beside its owner at {chip:?}"
                            );
                        }
                        transient::close_all();
                        drop(shown);
                        drop(scope);
                        telar::dispose_owner(owner_scope);
                    }
                }
            }
        }
    }

    /// A desktop widget's panel opens below it, or above it where there is no room below, and opening or closing it repaints nothing outside its own box.
    #[test]
    fn a_desktop_widget_s_panel_opens_and_closes_repainting_only_its_own_box() {
        let shown = show(LayerKind::Desktop, vec![desktop_grid(), panel(false)]);
        let closed = shown.frame();
        crate::panel::open_owned(&owner());
        let opened = shown.frame();
        let rect = shown.panel_rect();
        let widget = shown.owner_rect();
        assert!(
            rect.y >= widget.y + widget.height,
            "below the widget: {rect:?} {widget:?}"
        );
        assert_eq!(
            rect.x + rect.width / 2.0,
            widget.x + widget.width / 2.0,
            "centred under it"
        );
        assert_confined(&opened, &closed, rect, "opening");
        crate::panel::close_owned(&owner());
        let gone = shown.frame();
        assert_confined(&gone, &opened, rect, "closing");
    }

    /// The panel is the layout's own: its fill and radius where its style names them, and the theme's surface otherwise — square along a bar, at the theme's radius beside its owner.
    #[test]
    fn a_panel_is_painted_from_its_style_over_the_theme_s_surface() {
        for (along, styled) in [(false, false), (true, false), (false, true)] {
            let mut drawn = panel(along);
            if styled {
                drawn.style.fill = Some("accent".to_string());
                drawn.style.radius = Some(layout::Corners::all(6.0));
            }
            let shown = show(LayerKind::Top, vec![bar(Edge::Top), drawn]);
            crate::panel::open_owned(&owner());
            let frame = shown.frame();
            let rect = shown.panel_rect();
            let config = quiet();
            let theme = config.resolve_theme();
            let painted = frame
                .iter()
                .find_map(|command| match command {
                    DrawCommand::Rect { rect: at, style }
                        if *at == rect && style.fill.is_some() =>
                    {
                        Some(style.clone())
                    }
                    _ => None,
                })
                .expect("the panel paints its box");
            let (fill, radius) = match (styled, along) {
                (true, _) => (theme.accent, 6.0),
                (false, true) => (theme.surface.with_alpha(config.opacity()), 0.0),
                (false, false) => (
                    theme.surface.with_alpha(config.opacity()),
                    config.shape_from(None, None, None, None).radius,
                ),
            };
            assert_eq!(
                painted.fill,
                Some(telar::Paint::Solid(fill)),
                "along={along} styled={styled}"
            );
            assert_eq!(
                painted.radius.top_left, radius,
                "along={along} styled={styled}"
            );
        }
    }

    /// The owner's press opens its panel and a second press closes it, on a bar chip and on a desktop widget alike, when nothing is bound to the press.
    #[test]
    fn the_owner_s_press_toggles_its_panel_on_a_bar_and_on_the_desktop() {
        for (layer, holder) in [
            (LayerKind::Top, bar(Edge::Top)),
            (LayerKind::Desktop, desktop_grid()),
        ] {
            let mut shown = show(layer, vec![holder, panel(false)]);
            let at = centre(shown.owner_rect());
            shown.press(at);
            assert!(crate::panel::is_owned_open(&owner()), "{layer}: opened");
            assert_eq!(
                transient::drawn_in(&owner().id()).map(|window| window.layer),
                Some(layer),
                "in its owner's own window"
            );
            shown.press(at);
            assert!(!crate::panel::is_owned_open(&owner()), "{layer}: closed");
        }
    }

    /// What closes a drawer closes a panel: a press outside it, Esc, and any window opening.
    #[test]
    fn a_press_outside_esc_and_a_window_opening_each_close_the_panel() {
        let mut shown = show(LayerKind::Desktop, vec![desktop_grid(), panel(false)]);
        crate::panel::open_owned(&owner());
        shown.frame();
        let rect = shown.panel_rect();
        shown.press((rect.x + rect.width + 200.0, rect.y + rect.height + 100.0));
        assert!(!crate::panel::is_owned_open(&owner()), "a press outside");

        crate::panel::open_owned(&owner());
        shown.frame();
        shown.press(centre(shown.panel_rect()));
        assert!(
            crate::panel::is_owned_open(&owner()),
            "a press inside leaves it open"
        );
        assert!(telar::dismiss_top(), "Esc has something to dismiss");
        assert!(!crate::panel::is_owned_open(&owner()), "Esc");

        crate::panel::open_owned(&owner());
        transient::open(
            transient::Spec::new(
                "launcher",
                transient::Place::Centred,
                Rc::new(|_: &ui::chrome::Chrome| {
                    Ok(Box::new(telar::Container::new(telar::LayoutStyle::new(), vec![])?) as _)
                }),
            )
            .slot(transient::Slot::Standing),
        );
        assert!(!crate::panel::is_owned_open(&owner()), "a window opening");
        transient::close_all();
    }

    /// A reload builds every area again and keeps what the user opened, and only a layout that no longer gives the owner a panel closes it.
    #[test]
    fn a_reload_keeps_the_panel_open_until_the_layout_takes_it_away() {
        let shown = show(LayerKind::Top, vec![bar(Edge::Top), panel(true)]);
        crate::panel::open_owned(&owner());
        let before = shown.frame();
        let rect = shown.panel_rect();

        let config = quiet();
        let same = resolved(LayerKind::Top, vec![bar(Edge::Top), panel(true)]);
        publish(&same, &config);
        crate::panel::prune_owned(&[(Some(SCREEN), &same)]);
        shown.app.rebuild();
        let after = shown.frame();
        assert!(
            crate::panel::is_owned_open(&owner()),
            "kept across a reload"
        );
        assert_eq!(shown.panel_rect(), rect, "where it was");
        let damaged = telar::testing::damage(&after, &before).expect("a rebuild of the bar alone");
        assert!(
            damaged.iter().all(|damage| !overlaps(damage, rect)),
            "and the panel itself is not repainted by it: {damaged:?}"
        );

        let without = resolved(LayerKind::Top, vec![bar(Edge::Top)]);
        publish(&without, &config);
        crate::panel::prune_owned(&[(Some(SCREEN), &without)]);
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "a layout with no panel for it closes it"
        );
    }

    /// Under a fullscreen window the top layer is out of sight, so a bar owner's panel opens in the overlay window instead.
    #[test]
    fn a_panel_whose_layer_is_hidden_opens_in_the_overlay_window() {
        let _shown = show(LayerKind::Top, vec![bar(Edge::Top), panel(false)]);
        transient::set_hidden_layers(|_, layer| layer == LayerKind::Top);
        crate::panel::open_owned(&owner());
        assert_eq!(
            transient::drawn_in(&owner().id()).map(|window| window.layer),
            Some(LayerKind::Overlay)
        );
        transient::set_hidden_layers(|_, _| false);
        transient::close_all();
    }

    /// A press bound to an action runs the action, and the panel stays shut.
    #[test]
    fn a_bound_press_wins_over_the_owned_panel() {
        let mut held = desktop_grid();
        held.groups[0].children[0].actions = BTreeMap::from([(
            layout::Trigger::Press,
            layout::Action(vec!["launcher toggle".to_string()]),
        )]);
        let ran = Rc::new(Cell::new(0));
        let counted = Rc::clone(&ran);
        services::command::set_runner(
            move |_| {
                counted.set(counted.get() + 1);
                "ok".to_string()
            },
            |_| true,
        );
        let mut shown = show(LayerKind::Desktop, vec![held, panel(false)]);
        shown.press(centre(shown.owner_rect()));
        assert_eq!(ran.get(), 1);
        assert!(!crate::panel::is_owned_open(&owner()));
    }

    fn written(text: &str) -> layout::ResolvedExpr {
        layout::ResolvedExpr {
            expr: layout::Expr(text.to_string()),
            origin: layout::Origin::Level(layout::Level {
                layout: layout::LayoutId::new("written"),
                output: layout::OutputMatch::default(),
                workspace: None,
            }),
            within: None,
        }
    }

    thread_local! {
        static SINKS: std::cell::RefCell<Vec<(Rc<Cell<bool>>, Sink)>> = std::cell::RefCell::default();
        static NOW: Cell<bool> = const { Cell::new(true) };
    }

    fn gate_feed(mut sink: Sink) {
        sink(Reading::from([telar_expression::Value::Bool(
            NOW.with(Cell::get),
        )]));
        let alive = Rc::new(Cell::new(true));
        let ended = Rc::clone(&alive);
        telar::on_cleanup(move || ended.set(false));
        SINKS.with(|sinks| sinks.borrow_mut().push((alive, sink)));
    }

    static GATE: SourceDef = SourceDef {
        id: "gate",
        fields: &[FieldDef {
            name: "on",
            privacy: Privacy::Public,
            ty: FieldType::Bool,
        }],
        feed: gate_feed,
    };

    fn set_shown(shown: bool) {
        NOW.with(|now| now.set(shown));
        let mut sinks = SINKS.with(|sinks| std::mem::take(&mut *sinks.borrow_mut()));
        for (alive, sink) in &mut sinks {
            if alive.get() {
                sink(Reading::from([telar_expression::Value::Bool(shown)]));
            }
        }
        sinks.retain(|(alive, _)| alive.get());
        SINKS.with(|held| held.borrow_mut().extend(sinks));
    }

    fn hideable() -> ResolvedArea {
        let mut hideable = panel(false);
        hideable.visible = Some(written("$gate.on"));
        hideable
    }

    fn layer_opacities(frame: &[DrawCommand]) -> Vec<f32> {
        frame
            .iter()
            .filter_map(|command| match command {
                DrawCommand::PushLayer { opacity, .. } => Some(*opacity),
                _ => None,
            })
            .collect()
    }

    /// A panel its `visible` reads false does not open, and one open when it turns false closes.
    #[test]
    fn a_panel_its_visible_hides_does_not_open_and_closes_when_it_turns_false() {
        set_shown(false);
        let _shown = show(LayerKind::Top, vec![bar(Edge::Top), hideable()]);
        assert!(
            crate::panel::open_owned(&owner()),
            "it has a panel, though it is hidden"
        );
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "hidden, it stays shut"
        );
        let at = crate::rects::instance(Some(SCREEN), &InstanceId::new("owner"))
            .expect("the owner is drawn")
            .0;
        assert!(crate::panel::toggle_owned(&at));
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "and a press leaves it shut"
        );

        set_shown(true);
        crate::panel::open_owned(&owner());
        assert!(crate::panel::is_owned_open(&owner()), "shown, it opens");

        set_shown(false);
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "turning false closes it"
        );
    }

    /// In its layer's edit mode a hidden panel opens drawn dim so it can be selected, and leaving the mode closes it again; another layer's mode does nothing for it.
    #[test]
    fn a_hidden_panel_opens_dim_in_its_layer_s_edit_mode() {
        set_shown(false);
        let shown = show(LayerKind::Top, vec![bar(Edge::Top), hideable()]);

        crate::expressions::set_edited(Some((Some(SCREEN.to_string()), LayerKind::Desktop)));
        crate::panel::open_owned(&owner());
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "another layer's mode does not draw it"
        );

        crate::expressions::set_edited(Some((Some(SCREEN.to_string()), LayerKind::Top)));
        crate::panel::open_owned(&owner());
        assert!(
            crate::panel::is_owned_open(&owner()),
            "its own layer's does"
        );
        assert_eq!(
            layer_opacities(&shown.frame()),
            [crate::expressions::HIDDEN_OPACITY]
        );
        let node = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("panel"));
        assert_eq!(
            crate::expressions::hidden_by(&node).as_deref(),
            Some("$gate.on"),
            "and says what hides it"
        );

        set_shown(true);
        assert!(
            layer_opacities(&shown.frame()).is_empty(),
            "shown, it is at full strength"
        );

        set_shown(false);
        crate::expressions::set_edited(None);
        assert!(
            !crate::panel::is_owned_open(&owner()),
            "leaving the mode closes what the expression hides"
        );
    }

    fn blurring() -> ResolvedArea {
        let mut blurring = panel(false);
        blurring.style.backdrop = Some(layout::Backdrop::Blur);
        blurring
    }

    /// A panel styled `backdrop = "blur"` asks the compositor to blur behind its box for as long as it is up, beside what the window's areas ask.
    #[test]
    fn a_panel_that_blurs_asks_the_compositor_to_blur_behind_its_box_while_it_is_up() {
        let mut strip = bar(Edge::Top);
        strip.style.backdrop = Some(layout::Backdrop::Blur);
        for layer in [LayerKind::Top, LayerKind::Desktop] {
            let holder = match layer {
                LayerKind::Top => strip.clone(),
                _ => desktop_grid(),
            };
            let shown = show(layer, vec![holder, blurring()]);
            let before = shown.app.demands().blur_region();
            assert!(before.len() <= 1, "{layer}: only the bar's: {before:?}");
            crate::panel::open_owned(&owner());
            shown.frame();
            let blurred = shown.app.demands().blur_region();
            assert_eq!(blurred.len(), before.len() + 1, "{layer}: {blurred:?}");
            assert!(
                blurred.contains(&shown.panel_rect()),
                "{layer}: the panel's own box: {blurred:?}"
            );
            crate::panel::close_owned(&owner());
            shown.frame();
            assert_eq!(
                shown.app.demands().blur_region(),
                before,
                "{layer}: closing takes it back"
            );
        }
    }

    /// A panel that does not ask for a blur adds nothing to the region.
    #[test]
    fn a_panel_that_does_not_blur_asks_for_no_blur_region() {
        let shown = show(LayerKind::Top, vec![bar(Edge::Top), panel(false)]);
        crate::panel::open_owned(&owner());
        shown.frame();
        assert!(shown.app.demands().blur_region().is_empty());
    }

    /// On the background layer nothing but the window's own surface is under a panel, so the shell blurs it itself.
    #[test]
    fn a_panel_that_blurs_on_the_background_layer_frosts_what_its_own_surface_drew() {
        let shown = show(LayerKind::Background, vec![desktop_grid(), blurring()]);
        crate::panel::open_owned(&owner());
        let frame = shown.frame();
        assert!(
        frame.iter().any(|command| matches!(
            command,
            DrawCommand::PushLayer { backdrop_blur, .. } if *backdrop_blur == crate::area::BACKDROP_BLUR
        )),
        "frosted in the surface"
    );
        assert!(
            shown.app.demands().blur_region().is_empty(),
            "and no compositor region"
        );
    }

    fn rounded(radius: f32) -> ResolvedArea {
        let mut rounded = panel(false);
        rounded.style.radius = Some(layout::Corners::all(radius));
        rounded.style.fill = Some("accent".to_string());
        rounded
    }

    fn cuts(frame: &[DrawCommand], rect: Rect) -> Vec<f32> {
        frame
            .iter()
            .filter_map(|command| match command {
                DrawCommand::PushClip { rect: at, radius } if *at == rect => Some(radius.top_left),
                _ => None,
            })
            .collect()
    }

    /// What the panel holds is cut to its rounded corners, and no corner is rounder than half the short side.
    #[test]
    fn what_a_panel_holds_is_cut_to_its_radius_held_to_half_the_short_side() {
        let shown = show(LayerKind::Top, vec![bar(Edge::Top), rounded(12.0)]);
        crate::panel::open_owned(&owner());
        let frame = shown.frame();
        assert_eq!(cuts(&frame, shown.panel_rect()), [12.0]);
        crate::panel::close_owned(&owner());

        let shown = show(LayerKind::Top, vec![bar(Edge::Top), rounded(10_000.0)]);
        crate::panel::open_owned(&owner());
        let frame = shown.frame();
        let rect = shown.panel_rect();
        assert_eq!(cuts(&frame, rect), [rect.width.min(rect.height) / 2.0]);
    }

    fn animated() -> Arc<Config> {
        let mut config = Config::default();
        config.animation.enabled = true;
        config.animation.panel_duration_ms = 200;
        Arc::new(config)
    }

    fn square_cuts(frame: &[DrawCommand], rect: Rect) -> usize {
        cuts(frame, rect)
            .iter()
            .filter(|radius| **radius == 0.0)
            .count()
    }

    /// While a drawer or a panel slides in or out, its paint stays inside the box it ends in, and once it has settled nothing is cut that was not before.
    #[test]
    fn a_slide_never_damages_outside_the_box_it_ends_in() {
        for edge in Edge::ALL {
            let what = format!("{edge:?}");
            let shown = show_with(LayerKind::Top, vec![bar(edge), rounded(12.0)], animated());
            let closed = shown.frame();
            crate::panel::open_owned(&owner());
            let mut before = closed;
            let start = std::time::Instant::now();
            let mut rect = None;
            for step in 0..=14u32 {
                telar::motion::tick(start + std::time::Duration::from_millis(u64::from(step) * 20));
                let frame = shown.frame();
                let at = *rect.get_or_insert_with(|| shown.panel_rect());
                if step < 8 {
                    assert!(
                        square_cuts(&frame, at) > 0,
                        "{what} step {step}: moving, so cut"
                    );
                }
                if let Some(damaged) = telar::testing::damage(&frame, &before) {
                    assert!(
                        damaged.iter().all(|damage| inside(damage, at)),
                        "{what} step {step}: damage {damaged:?} runs past {at:?}"
                    );
                } else {
                    panic!("{what} step {step}: the whole surface was damaged");
                }
                before = frame;
            }
            let at = rect.expect("a frame was drawn");
            assert_eq!(
                square_cuts(&before, at),
                0,
                "{what}: at rest nothing is cut"
            );

            crate::panel::close_owned(&owner());
            let start = start + std::time::Duration::from_millis(400);
            for step in 0..=14u32 {
                telar::motion::tick(start + std::time::Duration::from_millis(u64::from(step) * 20));
                let frame = shown.frame();
                if let Some(damaged) = telar::testing::damage(&frame, &before) {
                    assert!(
                        damaged.iter().all(|damage| inside(damage, at)),
                        "{what} closing step {step}: damage {damaged:?} runs past {at:?}"
                    );
                }
                before = frame;
            }
        }
    }

    /// A drawer beside a chip slides in the same way, and the same cut holds it.
    #[test]
    fn a_drawer_slides_inside_the_box_it_ends_in() {
        let config = animated();
        let shown = show_with(LayerKind::Top, vec![bar(Edge::Top)], Arc::clone(&config));
        let mut before = shown.frame();
        let anchor = transient::Anchor {
            output: Some(SCREEN.to_string()),
            layer: LayerKind::Top,
            edge: Edge::Top,
            rect: shown.owner_rect(),
            chrome: ui::chrome::Chrome::global(config, Some(SCREEN.to_string())),
            gap: 8.0,
        };
        transient::open(
            transient::Spec::new(
                "drawer",
                transient::Place::Beside(anchor.clone()),
                Rc::new(|_: &ui::chrome::Chrome| {
                    Ok(Box::new(telar::StyledContainer::new(
                        telar::LayoutStyle::new().width(120.0).height(80.0),
                        |_| telar::RectStyle {
                            fill: Some(telar::Paint::Solid(telar::Color::rgb(0.2, 0.4, 0.8))),
                            ..telar::RectStyle::default()
                        },
                        Vec::new(),
                    )?) as _)
                }),
            )
            .slot(transient::Slot::Drawer)
            .dismiss_on_outside()
            .motion(transient::Motion::Slide(Edge::Top)),
        );
        let start = std::time::Instant::now();
        let mut steps = Vec::new();
        for step in 0..=14u32 {
            telar::motion::tick(start + std::time::Duration::from_millis(u64::from(step) * 20));
            let frame = shown.frame();
            steps.push(telar::testing::damage(&frame, &before));
            before = frame;
        }
        let screen = shown.app.screen();
        let usable = screen.reserved.box_of(Within::Usable, screen.size);
        let (x, y) = transient::beside(&anchor, (120.0, 80.0), usable);
        let at = Rect::new(x, y, 120.0, 80.0);
        for (step, damaged) in steps.into_iter().enumerate() {
            let damaged = damaged.unwrap_or_else(|| panic!("step {step}: the whole surface"));
            assert!(
                damaged.iter().all(|damage| inside(damage, at)),
                "step {step}: damage {damaged:?} runs past {at:?}"
            );
        }
        assert_eq!(square_cuts(&before, at), 0, "at rest nothing is cut");
        transient::close_all();
    }

    thread_local! {
        static RAN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn ran() -> Vec<String> {
        RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    fn recording() {
        ran();
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
        ui::module::set_panel_opener(|panel| {
            RAN.with(|ran| ran.borrow_mut().push(format!("open {panel}")))
        });
    }

    fn owner_of(module: &str, bound: bool) -> layout::ResolvedInstance {
        let mut owner = instance("owner", module, Representation::Chip);
        if bound {
            owner.actions = BTreeMap::from([(
                layout::Trigger::Press,
                layout::Action(vec!["launcher toggle".to_string()]),
            )]);
        }
        owner
    }

    fn bar_holding(owner: layout::ResolvedInstance, hides: bool) -> ResolvedArea {
        let mut kind = bar_kind(Edge::Top);
        if let ResolvedAreaKind::Bar { autohide, .. } = &mut kind {
            *autohide = hides.then_some(layout::AutoHide {
                peek: 2.0,
                on_hover: true,
            });
        }
        area(
            "bar",
            kind,
            vec![group(
                "start",
                GroupKind::Zone { zone: Zone::Start },
                vec![owner],
            )],
        )
    }

    /// A press on a bar chip runs its bound action, else opens the panel the layout gives its instance, else its module's own panel — the settings window for a placeholder, whose module is missing.
    #[test]
    fn a_chip_press_runs_its_action_else_its_owned_panel_else_its_module_panel() {
        for module in ["probe", "missing"] {
            for (bound, owns) in [(true, true), (false, true), (false, false)] {
                let what = format!("{module} bound={bound} owns={owns}");
                recording();
                let mut areas = vec![bar_holding(owner_of(module, bound), false)];
                if owns {
                    areas.push(panel(false));
                }
                let mut shown = show(LayerKind::Top, areas);
                shown.press(centre(shown.owner_rect()));
                let module_panel = match module {
                    "probe" => transient::is_open("probe"),
                    _ => RAN.with(|ran| ran.borrow().contains(&"open settings".to_string())),
                };
                let owned = crate::panel::is_owned_open(&owner());
                match (bound, owns) {
                    (true, _) => {
                        assert_eq!(ran(), ["launcher toggle"], "{what}");
                        assert!(!owned && !module_panel, "{what}: the action alone");
                    }
                    (false, true) => {
                        assert!(owned, "{what}: the owned panel opens");
                        assert!(!module_panel, "{what}: and nothing else");
                    }
                    (false, false) => assert!(module_panel, "{what}: the module's panel opens"),
                }
                transient::close_all();
            }
        }
    }

    /// A chip on a dock answers a press as a bar chip does: with nothing bound and no panel of its own, its module's panel opens.
    #[test]
    fn a_dock_chip_press_opens_its_module_panel() {
        recording();
        let dock = area(
            "dock",
            ResolvedAreaKind::Dock {
                edge: Edge::Bottom,
                thickness: 48.0,
            },
            vec![group(
                "start",
                GroupKind::Zone { zone: Zone::Start },
                vec![instance("owner", "probe", Representation::Chip)],
            )],
        );
        let mut shown = show(LayerKind::Top, vec![dock]);
        shown.press(centre(shown.owner_rect()));
        assert!(transient::is_open("probe"));
        shown.press(centre(shown.owner_rect()));
        assert!(!transient::is_open("probe"), "a second press closes it");
        transient::close_all();
    }

    /// `panel toggle <instance>` opens the panel the layout gives the instance, else its module's panel; a module id keeps toggling its module's panel, and a name that is neither is refused.
    #[test]
    fn panel_toggle_names_an_instance_or_a_module() {
        let _shown = show(LayerKind::Top, vec![bar(Edge::Top), panel(false)]);
        crate::panel::toggle_named("owner").expect("the instance has a panel");
        assert!(crate::panel::is_owned_open(&owner()));
        assert!(!transient::is_open("probe"));
        crate::panel::toggle_named("owner").expect("again");
        assert!(!crate::panel::is_owned_open(&owner()));

        let _shown = show(LayerKind::Top, vec![bar(Edge::Top)]);
        crate::panel::toggle_named("owner").expect("its module has a panel");
        assert!(transient::is_open("probe"), "the instance's module panel");
        crate::panel::toggle_named("probe").expect("a module id");
        assert!(
            !transient::is_open("probe"),
            "toggled shut by its module id"
        );
        assert!(crate::panel::toggle_named("nothing").is_err());
        transient::close_all();
    }

    /// A drawer opened from a chip inside an owned panel is that panel's child: opening it leaves the panel open, Esc closes the drawer first, and closing the panel closes the drawer with it.
    #[test]
    fn a_drawer_opened_from_inside_a_panel_is_its_child() {
        let panel = ResolvedArea {
            groups: vec![group(
                "inside",
                cell(0, 0),
                vec![instance("inner", "probe", Representation::Chip)],
            )],
            ..panel(false)
        };
        let mut shown = show(LayerKind::Top, vec![bar(Edge::Top), panel]);
        let inner = || {
            centre(
                crate::rects::instance(Some(SCREEN), &InstanceId::new("inner"))
                    .expect("the chip inside the panel is drawn")
                    .1,
            )
        };
        crate::panel::open_owned(&owner());
        shown.frame();
        shown.press(inner());
        assert!(transient::is_open("probe"), "the drawer opens");
        assert!(
            crate::panel::is_owned_open(&owner()),
            "and the panel it was opened from stays open"
        );

        assert!(telar::dismiss_top(), "Esc has something to dismiss");
        assert!(!transient::is_open("probe"), "Esc closes the drawer first");
        assert!(
            crate::panel::is_owned_open(&owner()),
            "and leaves the panel"
        );

        shown.press(inner());
        assert!(transient::is_open("probe"), "opened again");
        crate::panel::close_owned(&owner());
        assert!(
            !transient::is_open("probe"),
            "closing the panel closes its child"
        );
        transient::close_all();
    }

    /// How far the top bar is moved up off its edge in `frame`; what hangs below it is moved down.
    fn bar_offset(frame: &[DrawCommand]) -> f32 {
        frame
            .iter()
            .filter_map(|command| match command {
                DrawCommand::PushMatrix { matrix } => Some(-matrix[5]),
                _ => None,
            })
            .fold(0.0, f32::max)
    }

    fn settled(shown: &Shown) -> Vec<DrawCommand> {
        let now = std::time::Instant::now();
        telar::motion::tick(now);
        telar::motion::tick(now + std::time::Duration::from_secs(5));
        shown.frame()
    }

    fn hover(shown: &mut Shown, (x, y): (f32, f32)) {
        shown.tree.on_event(&Event::PointerMoved {
            x: f64::from(x),
            y: f64::from(y),
            source: PointerSource::Mouse,
        });
    }

    /// An autohidden bar stays out while the panel its chip owns, or its module's drawer, is open, and goes away once it closes and the pointer is elsewhere.
    #[test]
    fn an_autohidden_bar_stays_out_while_a_panel_of_its_chip_is_open() {
        for owns in [true, false] {
            let what = if owns { "owned panel" } else { "drawer" };
            recording();
            let mut areas = vec![bar_holding(owner_of("probe", false), true)];
            if owns {
                areas.push(panel(false));
            }
            let mut shown = show(LayerKind::Top, areas);
            assert!(
                bar_offset(&settled(&shown)) > 1.0,
                "{what}: hidden at first"
            );
            let chip = centre(shown.owner_rect());
            hover(&mut shown, (chip.0, 1.0));
            assert_eq!(
                bar_offset(&settled(&shown)),
                0.0,
                "{what}: hovering brings it"
            );

            shown.press(chip);
            let open = || match owns {
                true => crate::panel::is_owned_open(&owner()),
                false => transient::is_open("probe"),
            };
            assert!(open(), "{what}: opened");
            hover(&mut shown, (WIDE as f32 / 2.0, HIGH as f32 - 20.0));
            assert_eq!(
                bar_offset(&settled(&shown)),
                0.0,
                "{what}: the bar stays out while it is open"
            );

            transient::close_all();
            assert!(!open());
            assert!(
                bar_offset(&settled(&shown)) > 1.0,
                "{what}: and goes once it closes"
            );
        }
    }
}
