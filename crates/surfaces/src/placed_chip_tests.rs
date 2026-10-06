//! A chip placed anywhere but a bar — a child of a container, a chip on a grid or a dock — answers its own gestures in a chip's order, claims the pointer only where it is drawn, and hangs what it opens off itself.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use config::{Config, Edge};
    use layout::{
        Action, Arrange, GroupKind, InstanceId, LayerKind, Placement, Representation, Resolved,
        ResolvedArea, ResolvedAreaKind, ResolvedGroup, ResolvedInstance, ResolvedLayer, Sides,
        Style, Trigger, Zone,
    };
    use telar::{App, ComponentList, Event, PointerButton, PointerSource, Rect, set_theme};
    use ui::descriptor::{
        Built, Category, ChipDef, Input, ModuleDescriptor, PanelDef, Representations,
    };
    use ui::host::Host;

    use crate::layer_window::{LayerApp, Reserved, Screen, WindowAreas};
    use crate::panel::Owner;
    use crate::reconcile::Desktop;
    use crate::rects::Node;
    use crate::test_rig::{SCREEN, area, bar_kind, cell, grid_kind, group, instance};
    use crate::transient::{self, Place};

    const WIDE: u32 = 800;
    const HIGH: u32 = 600;
    const DOT: f32 = 24.0;

    thread_local! {
        static HEARD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn heard() -> Vec<String> {
        HEARD.with(|heard| std::mem::take(&mut *heard.borrow_mut()))
    }

    fn hear(what: String) {
        HEARD.with(|heard| heard.borrow_mut().push(what));
    }

    fn dot(_: &Host) -> Built {
        Ok(Box::new(telar::Container::new(
            telar::LayoutStyle::new().width(DOT).height(DOT),
            Vec::new(),
        )?))
    }

    fn turned(host: &Host, _: f32, dy: f32) {
        hear(format!("{} turned {dy}", host.instance.as_str()));
    }

    /// Names the chip a press of it has in scope, which is what whatever it opens hangs off.
    fn tapped() {
        let pressed = ui::module::pressed_chip().and_then(|pressed| {
            let node = pressed.placement?.get::<Node>()?.clone();
            match node.part {
                crate::rects::Part::Instance(_, id) => Some(id.as_str().to_string()),
                _ => None,
            }
        });
        hear(format!("tapped {}", pressed.unwrap_or_default()));
    }

    const MODULES: &[ModuleDescriptor] = &[
        module(
            "knob",
            ChipDef::new(dot, Input::Interactive).on_scroll(turned),
            true,
        ),
        module(
            "tapper",
            ChipDef::new(dot, Input::Interactive).on_press(tapped),
            false,
        ),
        module(
            "wired",
            ChipDef::new(dot, Input::ReadOnly).self_managed(),
            false,
        ),
    ];

    const fn module(id: &'static str, chip: ChipDef, panel: bool) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
            category: Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(chip),
                panel: match panel {
                    true => Some(PanelDef::new(dot, Input::ReadOnly)),
                    false => None,
                },
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    fn chip(id: &str, module: &str) -> ResolvedInstance {
        ResolvedInstance {
            placement: Some(Placement::Weight(1.0)),
            ..instance(id, module, Representation::Chip)
        }
    }

    /// A row six cells long and one high at the grid's corner, holding `children` in shares of it, its plate at `opacity`.
    fn row(children: Vec<ResolvedInstance>, opacity: f32) -> ResolvedGroup {
        ResolvedGroup {
            arrange: Some(Arrange::Row),
            gap: Some(8.0),
            style: Style {
                padding: Some(Sides::all(0.0)),
                opacity: Some(opacity),
                ..Style::default()
            },
            ..group(
                "row",
                GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 6,
                    row_span: 1,
                },
                children,
            )
        }
    }

    fn widgets(groups: Vec<ResolvedGroup>) -> ResolvedArea {
        area("widgets", grid_kind(), groups)
    }

    fn owned_by(owner: &str) -> ResolvedArea {
        area(
            "panel",
            ResolvedAreaKind::Panel {
                owner: InstanceId::new(owner),
                along: false,
                cols: 2,
                rows: 1,
                cell: 40.0,
                gap: 8.0,
            },
            Vec::new(),
        )
    }

    struct Shown {
        _app: LayerApp,
        tree: ComponentList,
    }

    impl Shown {
        fn of(layer: LayerKind, areas: Vec<ResolvedArea>) -> Self {
            telar::reset_layout_runtime();
            let mut config = Config::default();
            config.animation.enabled = false;
            let config = Arc::new(config);
            config::set_output_config(SCREEN, Arc::clone(&config));
            set_theme(config.resolve_theme());
            ui::descriptor::install(MODULES);
            transient::close_all();
            heard();
            services::command::set_runner(
                |line| {
                    hear(line.to_string());
                    "ok".to_string()
                },
                |_| true,
            );
            let resolved = Resolved::of(SCREEN, [(layer, ResolvedLayer { areas })]);
            let screen = Screen {
                size: (WIDE as f32, HIGH as f32),
                reserved: Reserved::of(&resolved, &config),
            };
            crate::reconcile::publish(&[Desktop {
                output: Some(SCREEN.to_string()),
                config: Arc::clone(&config),
                resolved: resolved.clone(),
                reserved: screen.reserved,
                size: screen.size,
            }]);
            let app = LayerApp::standing(
                layer,
                WindowAreas::of(&resolved, layer),
                config,
                screen,
                Rc::new(crate::area::ShellAreas),
            );
            let tree = telar::testing::mount(app.root(), WIDE, HIGH);
            let shown = Self { _app: app, tree };
            shown.frame();
            shown
        }

        fn frame(&self) {
            telar::relayout_if_dirty();
            self.tree.commands();
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

        fn scroll(&mut self, (x, y): (f32, f32), dy: f32) {
            self.tree.on_event(&Event::Scrolled {
                delta: telar::ScrollDelta::Pixels { x: 0.0, y: dy },
                x: f64::from(x),
                y: f64::from(y),
            });
        }
    }

    /// Where the chip of the instance `id` is drawn: the chip itself, which a share can be larger than.
    fn chip_at(id: &str) -> Rect {
        crate::rects::chips()
            .into_iter()
            .rev()
            .find(|(node, _, _)| {
                matches!(&node.part, crate::rects::Part::Instance(_, held) if held.as_str() == id)
            })
            .map(|(_, _, rect)| rect)
            .unwrap_or_else(|| panic!("`{id}` is a chip something opens from"))
    }

    fn centre(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    fn claimed((x, y): (f32, f32)) -> bool {
        telar::interactive_rects()
            .iter()
            .any(|rect| rect.contains(x, y))
    }

    /// Where the drawer `module` opened hangs: off which rect, on which side, as the edge a bar there would face.
    fn hung(module: &str) -> (Rect, Edge, Option<String>) {
        match transient::place_of(module) {
            Some(Place::Beside(anchor)) => (anchor.rect, anchor.edge, anchor.output),
            Some(Place::Hanging(anchor)) => {
                let rect = transient::rect_of(module).expect("the drawer is laid out");
                let side = match rect.y >= anchor.rect.y + anchor.rect.height {
                    true => Edge::Top,
                    false => Edge::Bottom,
                };
                (anchor.rect, side, anchor.output)
            }
            Some(_) => panic!("`{module}` opened, but not beside anything"),
            None => panic!("`{module}` is not open"),
        }
    }

    /// The benchmark's empty-space rule inside a container: a press between two of its children reaches whatever is under the shell while the container paints nothing there, and only a filled plate claims it.
    #[test]
    fn a_press_between_the_children_of_an_unfilled_container_goes_through() {
        for (opacity, claims) in [(0.0, false), (0.6, true)] {
            let _shown = Shown::of(
                LayerKind::Desktop,
                vec![widgets(vec![row(
                    vec![chip("a", "knob"), chip("b", "knob")],
                    opacity,
                )])],
            );
            let (a, b) = (chip_at("a"), chip_at("b"));
            assert!(a.x + a.width < b.x, "two chips side by side: {a:?} {b:?}");
            assert!(
                claimed(centre(a)) && claimed(centre(b)),
                "each chip claims itself"
            );
            let between = ((a.x + a.width + b.x) / 2.0, a.y + a.height / 2.0);
            assert_eq!(
                claimed(between),
                claims,
                "at opacity {opacity}, the gap between {a:?} and {b:?}"
            );
        }
    }

    /// A chip-framed module drawn as a chip in a container sits in the chip shell a bar gives it, padded around its own content; a self-managed one stays as it lays itself out.
    #[test]
    fn a_chip_framed_module_in_a_container_is_drawn_in_the_bar_chip_shell() {
        let _shown = Shown::of(
            LayerKind::Desktop,
            vec![widgets(vec![row(
                vec![chip("framed", "knob"), chip("bare", "wired")],
                0.0,
            )])],
        );
        let framed = chip_at("framed");
        assert!(
            framed.width > DOT && framed.height > DOT,
            "padded around its content: {framed:?}"
        );
        let bare = chip_at("bare");
        assert_eq!(
            (bare.width, bare.height),
            (DOT, DOT),
            "as it lays itself out"
        );
    }

    /// Each child of a container answers its own press, in the order a chip on a bar does: what the layout binds, else the panel its instance owns, else its module's panel, hung off the child; and its module's own wheel.
    #[test]
    fn each_child_of_a_container_answers_its_own_press_in_a_chips_order() {
        let mut bound = chip("bound", "knob");
        bound.actions = BTreeMap::from([(Trigger::Press, Action(vec!["bound press".into()]))]);
        let mut shown = Shown::of(
            LayerKind::Desktop,
            vec![
                widgets(vec![row(
                    vec![bound, chip("owner", "knob"), chip("plain", "knob")],
                    0.0,
                )]),
                owned_by("owner"),
            ],
        );
        let owner = Owner {
            output: Some(SCREEN.to_string()),
            instance: InstanceId::new("owner"),
        };

        shown.press(centre(chip_at("bound")));
        assert_eq!(heard(), ["bound press"]);
        assert!(!transient::is_open("knob"), "and nothing else");

        shown.press(centre(chip_at("owner")));
        assert!(crate::panel::is_owned_open(&owner), "the panel it owns");
        assert!(!transient::is_open("knob"), "rather than its module's");
        transient::close_all();
        shown.frame();

        let plain = chip_at("plain");
        shown.press(centre(plain));
        assert!(transient::is_open("knob"), "its module's panel");
        let (rect, edge, output) = hung("knob");
        assert_eq!(rect, plain, "hung off the child that was pressed");
        assert_eq!(edge, Edge::Top, "below it, where the screen has room");
        assert_eq!(output.as_deref(), Some(SCREEN));
        transient::close_all();
        shown.frame();

        shown.scroll(centre(plain), 60.0);
        assert_eq!(heard(), ["plain turned 60"]);
    }

    /// A drawer opened from a chip on no edge hangs below it where it fits, else above it.
    #[test]
    fn a_drawer_from_a_chip_on_the_grid_hangs_off_it_where_there_is_room() {
        for (row, side) in [(0, Edge::Top), (5, Edge::Bottom)] {
            let mut shown = Shown::of(
                LayerKind::Desktop,
                vec![widgets(vec![group(
                    "loose",
                    cell(2, row),
                    vec![instance("loose", "knob", Representation::Chip)],
                )])],
            );
            let at = chip_at("loose");
            shown.press(centre(at));
            assert_eq!(
                hung("knob"),
                (at, side, Some(SCREEN.to_string())),
                "row {row}"
            );
            transient::close_all();

            crate::panel::toggle_named("loose").expect("a chip with a panel");
            shown.frame();
            let (rect, edge, _) = hung("knob");
            assert_eq!(
                (rect, edge),
                (at, side),
                "row {row}: `panel toggle` opens it off the same chip"
            );
            transient::close_all();
        }
    }

    /// `panel toggle` on a chip inside a container hangs its drawer off the chip itself, not off the larger share the container gives it.
    #[test]
    fn panel_toggle_on_a_container_chip_hangs_off_the_chip_not_its_share() {
        let shown = Shown::of(
            LayerKind::Desktop,
            vec![widgets(vec![row(
                vec![chip("a", "knob"), chip("b", "knob")],
                0.0,
            )])],
        );
        let (chip, share) = (
            chip_at("a"),
            crate::rects::instance(Some(SCREEN), &InstanceId::new("a"))
                .expect("placed")
                .1,
        );
        assert_ne!(chip, share, "the share is larger than the chip");
        crate::panel::toggle_named("a").expect("a chip with a panel");
        shown.frame();
        assert_eq!(hung("knob").0, chip);
        transient::close_all();
    }

    #[test]
    fn a_chip_hangs_below_itself_where_it_fits_else_above_else_where_there_is_more_room() {
        let usable = Rect::new(0.0, 40.0, 800.0, 560.0);
        let at = |y: f32| Rect::new(100.0, y, 40.0, 30.0);
        let hanging = transient::hanging;
        assert_eq!(hanging(at(40.0), usable, 200.0), Edge::Top);
        assert_eq!(
            hanging(at(320.0), usable, 200.0),
            Edge::Top,
            "it fits below"
        );
        assert_eq!(
            hanging(at(320.0), usable, 240.0),
            Edge::Bottom,
            "it fits above"
        );
        assert_eq!(
            hanging(at(285.0), usable, 400.0),
            Edge::Top,
            "it fits nowhere, and below has more room"
        );
        assert_eq!(
            hanging(at(320.0), usable, 400.0),
            Edge::Bottom,
            "it fits nowhere, and above has more room"
        );
        assert_eq!(hanging(at(560.0), usable, 100.0), Edge::Bottom);
    }

    fn dock(edge: Edge, children: Vec<ResolvedInstance>) -> ResolvedArea {
        area(
            "dock",
            ResolvedAreaKind::Dock {
                edge,
                thickness: 48.0,
            },
            vec![group(
                "run",
                GroupKind::Zone { zone: Zone::Center },
                children,
            )],
        )
    }

    fn bar(edge: Edge, children: Vec<ResolvedInstance>) -> ResolvedArea {
        ResolvedArea {
            reserve: true,
            ..area(
                "bar",
                bar_kind(edge),
                vec![group(
                    "start",
                    GroupKind::Zone { zone: Zone::Start },
                    children,
                )],
            )
        }
    }

    /// A dock chip whose module also has a chip on a bar opens its drawer off itself, on its own edge, and the bar chip off itself, on the bar's: each press names the chip and the screen it was.
    #[test]
    fn a_dock_chip_and_a_bar_chip_of_one_module_each_open_its_drawer_off_themselves() {
        let mut shown = Shown::of(
            LayerKind::Top,
            vec![
                bar(
                    Edge::Left,
                    vec![instance("on-bar", "knob", Representation::Chip)],
                ),
                dock(
                    Edge::Bottom,
                    vec![instance("in-dock", "knob", Representation::Chip)],
                ),
            ],
        );
        let docked = chip_at("in-dock");
        shown.press(centre(docked));
        assert_eq!(
            hung("knob"),
            (docked, Edge::Bottom, Some(SCREEN.to_string())),
            "off the dock chip"
        );
        transient::close_all();
        shown.frame();

        let barred = chip_at("on-bar");
        shown.press(centre(barred));
        assert_eq!(
            hung("knob"),
            (barred, Edge::Left, Some(SCREEN.to_string())),
            "off the bar chip, on the bar's side"
        );
        transient::close_all();
    }

    /// A bar listed before a dock and a grid on the same layer keeps its chips live: neither area takes a press outside what it paints or holds, a filled dock included.
    #[test]
    fn a_bar_built_before_a_dock_and_a_grid_in_one_window_keeps_its_chips_live() {
        for fill in [None, Some("#336699".to_string())] {
            let mut shown = Shown::of(
                LayerKind::Top,
                vec![
                    bar(
                        Edge::Left,
                        vec![instance("on-bar", "tapper", Representation::Chip)],
                    ),
                    ResolvedArea {
                        style: Style {
                            fill: fill.clone(),
                            ..Style::default()
                        },
                        ..dock(
                            Edge::Bottom,
                            vec![instance("in-dock", "tapper", Representation::Chip)],
                        )
                    },
                    widgets(vec![group(
                        "loose",
                        cell(2, 2),
                        vec![instance("on-grid", "tapper", Representation::Chip)],
                    )]),
                ],
            );
            for id in ["on-bar", "in-dock", "on-grid"] {
                shown.press(centre(chip_at(id)));
                assert_eq!(heard(), [format!("tapped {id}")], "fill {fill:?}");
            }
        }
    }

    /// A chip's own press runs with that chip in scope wherever it is placed, so what it opens can find it.
    #[test]
    fn a_chips_own_press_names_the_chip_wherever_it_is_placed() {
        let mut shown = Shown::of(
            LayerKind::Top,
            vec![dock(
                Edge::Bottom,
                vec![instance("pressed", "tapper", Representation::Chip)],
            )],
        );
        shown.press(centre(chip_at("pressed")));
        assert_eq!(heard(), ["tapped pressed"]);
    }
}
