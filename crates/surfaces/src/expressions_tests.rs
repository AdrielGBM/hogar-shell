//! What a layout's expressions do on screen: a `visible` that hides an area takes its paint and its input with it and rebuilds nothing, a binding that moves rebuilds its own instance and nothing else, and a hidden window stops reading.

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use config::Config;
    use layout::{
        AreaId, AreaStyle, Expr, GroupId, GroupKind, InstanceId as PlacedId, LayerKind, Rect,
        Representation, ResolvedArea, ResolvedAreaKind, ResolvedExpr, ResolvedGroup,
        ResolvedInstance, Within, Zone,
    };
    use telar::{
        Color, ComponentList, Container, DrawCommand, Event, LayoutItem, LayoutStyle, Paint,
        PointerButton, PointerSource, RectStyle, RwSignal, StyledContainer, set_theme, signal,
    };
    use telar_expression::Value;
    use ui::descriptor::{
        Built, Category, ChipDef, FieldDef, FieldType, Input, ModuleDescriptor, OptionsType,
        Privacy, Reading, Representations, Sink, SourceDef,
    };
    use ui::host::{Audience, Host, InstanceStore};

    use crate::area::Surround;
    use crate::layer_window::{Demands, LayerWindowContext, Reserved};

    const SCREEN: &str = "DP-1";
    const SIDE: f32 = 200.0;
    const INK: Color = Color::rgb(0.8, 0.1, 0.1);

    thread_local! {
        static SINKS: RefCell<Vec<(Rc<Cell<bool>>, Sink)>> = RefCell::default();
        static SUBSCRIBED: Cell<usize> = const { Cell::new(0) };
        static UNSUBSCRIBED: Cell<usize> = const { Cell::new(0) };
        static BUILT: RefCell<BTreeMap<String, Vec<Seen>>> = RefCell::default();
        static PRESSED: Cell<usize> = const { Cell::new(0) };
        static LISTS: RefCell<(Vec<&'static str>, Vec<&'static str>)> = RefCell::default();
        static LAST: Cell<(bool, f64)> = const { Cell::new((false, 0.0)) };
    }

    /// What one build of an instance was built with.
    #[derive(Clone, Debug, PartialEq)]
    struct Seen {
        date_format: String,
        accent: Color,
        kept: u32,
    }

    static KEPT: InstanceStore<u32> = InstanceStore::new(|| 0);

    fn probe_feed(sink: Sink) {
        SUBSCRIBED.with(|n| n.set(n.get() + 1));
        let alive = Rc::new(Cell::new(true));
        let ended = Rc::clone(&alive);
        telar::on_cleanup(move || {
            ended.set(false);
            UNSUBSCRIBED.with(|n| n.set(n.get() + 1));
        });
        SINKS.with(|sinks| sinks.borrow_mut().push((alive, sink)));
    }

    static PROBE: SourceDef = SourceDef {
        id: "probe",
        fields: &[
            FieldDef {
                name: "on",
                privacy: Privacy::Public,
                ty: FieldType::Bool,
            },
            FieldDef {
                name: "level",
                privacy: Privacy::Public,
                ty: FieldType::Number,
            },
            FieldDef {
                name: "names",
                privacy: Privacy::Public,
                ty: FieldType::List(&FieldType::Text),
            },
            FieldDef {
                name: "secrets",
                privacy: Privacy::Private,
                ty: FieldType::List(&FieldType::Text),
            },
        ],
        feed: probe_feed,
    };

    fn publish(on: bool, level: f64) {
        LAST.with(|last| last.set((on, level)));
        let (names, secrets) = LISTS.with(|lists| lists.borrow().clone());
        let list = |items: &[&str]| Value::list(items.iter().map(|item| Value::text(*item)));
        let mut sinks = SINKS.with(|sinks| std::mem::take(&mut *sinks.borrow_mut()));
        for (alive, sink) in &mut sinks {
            if alive.get() {
                sink(Reading::from([
                    Value::Bool(on),
                    Value::Number(level),
                    list(&names),
                    list(&secrets),
                ]));
            }
        }
        sinks.retain(|(alive, _)| alive.get());
        SINKS.with(|held| held.borrow_mut().extend(sinks));
    }

    /// Publishes `names` and `secrets` with the rest of the reading as it last was.
    fn publish_lists(names: &[&'static str], secrets: &[&'static str]) {
        LISTS.with(|lists| *lists.borrow_mut() = (names.to_vec(), secrets.to_vec()));
        let (on, level) = LAST.with(Cell::get);
        publish(on, level);
    }

    /// A square of ink, recorded as built with what `host` says.
    fn square(host: &Host) -> Result<StyledContainer, telar::LayoutError> {
        let seen = Seen {
            date_format: host.options::<config::ClockConfig>().date_format,
            accent: host.accent,
            kept: KEPT.get(&host.instance),
        };
        BUILT.with(|built| {
            built
                .borrow_mut()
                .entry(host.instance.as_str().to_string())
                .or_default()
                .push(seen)
        });
        StyledContainer::new(
            LayoutStyle::new().width(40.0).height(40.0),
            |_| RectStyle {
                fill: Some(Paint::Solid(INK)),
                ..RectStyle::default()
            },
            Vec::new(),
        )
    }

    /// A pressable square that says what it was built with.
    fn counter(host: &Host) -> Built {
        Ok(Box::new(
            square(host)?
                .input_opaque()
                .on_press(|| PRESSED.with(|n| n.set(n.get() + 1))),
        ))
    }

    /// The same square, answering nothing, so the lock screen builds it.
    fn reader(host: &Host) -> Built {
        Ok(Box::new(square(host)?))
    }

    const TABLE: &[ModuleDescriptor] = &[
        ModuleDescriptor {
            id: "counter",
            name: "Counter",
            icon: "circle",
            category: Category::Info,
            options: &[OptionsType::of::<config::ClockConfig>()],
            representations: Representations {
                chip: Some(ChipDef::new(counter, Input::Interactive)),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[PROBE],
        },
        ModuleDescriptor {
            id: "reader",
            name: "Reader",
            icon: "circle",
            category: Category::Info,
            options: &[OptionsType::of::<config::ClockConfig>()],
            representations: Representations {
                chip: Some(ChipDef::new(reader, Input::ReadOnly)),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        },
    ];

    /// `text` as the every-output rule of a layout called `written` writes it.
    fn written(text: &str) -> ResolvedExpr {
        ResolvedExpr {
            expr: Expr(text.to_string()),
            origin: layout::Origin::Level(layout::Level {
                layout: layout::LayoutId::new("written"),
                output: layout::OutputMatch::default(),
                workspace: None,
            }),
            within: None,
        }
    }

    fn placed(id: &str, bindings: &[(&str, &str)]) -> ResolvedInstance {
        ResolvedInstance {
            id: PlacedId::new(id),
            module: "counter".to_string(),
            representation: Representation::Chip,
            options: toml::Table::new(),
            bindings: bindings
                .iter()
                .map(|(path, expr)| (path.to_string(), written(expr)))
                .collect(),
            actions: BTreeMap::new(),
        }
    }

    fn free(visible: Option<&str>, children: Vec<ResolvedInstance>) -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new("readings"),
            kind: ResolvedAreaKind::Free {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                anchor: layout::Anchor::TopLeft,
            },
            reserve: false,
            above_fullscreen: false,
            within: Within::Output,
            style: AreaStyle::default(),
            visible: visible.map(written),
            groups: vec![ResolvedGroup {
                id: GroupId::new("g"),
                kind: GroupKind::Zone { zone: Zone::Start },
                stacked: false,
                repeat: None,
                komponent: None,
                children,
            }],
            actions: BTreeMap::new(),
        }
    }

    /// One area built the way a top window builds it, in a window whose being on screen the test decides.
    struct Rig {
        tree: ComponentList,
        root: telar::NodeId,
        mapped: RwSignal<bool>,
        owner: telar::OwnerId,
    }

    impl Rig {
        fn new(area: ResolvedArea) -> Self {
            Self::shown_to(area, Audience::Owner)
        }

        /// [`Rig::new`] for `audience`: the lock screen's is anyone in the room.
        fn shown_to(area: ResolvedArea, audience: Audience) -> Self {
            telar::reset_layout_runtime();
            let config = Arc::new(Config::default());
            set_theme(config.resolve_theme());
            ui::descriptor::install(TABLE);
            SINKS.with(|sinks| sinks.borrow_mut().clear());
            LISTS.with(|lists| *lists.borrow_mut() = Default::default());
            LAST.with(|last| last.set((false, 0.0)));
            BUILT.with(|built| built.borrow_mut().clear());
            let scope = telar::owner_scope();
            let owner = scope.id();
            let mapped = signal(true);
            telar::set_context(LayerWindowContext {
                layer: LayerKind::Top,
                output: Some(SCREEN.to_string()),
                demands: Rc::new(Demands::new(platform_wayland::Layer::Top)),
                mapped: mapped.read_only(),
            });
            let surround = Surround {
                config: &config,
                theme: config.resolve_theme(),
                output: Some(SCREEN),
                layer: LayerKind::Top,
                bounds: telar::Rect::new(0.0, 0.0, SIDE, SIDE),
                reserved: Reserved::default(),
                audience,
            };
            let node = crate::area::build(&area, surround)
                .expect("the area has a builder")
                .expect("the area builds");
            let page = Container::new(LayoutStyle::new().width(SIDE).height(SIDE), vec![node])
                .expect("a page");
            let root = page.layout_node();
            let tree = telar::testing::mount(page, SIDE as u32, SIDE as u32);
            drop(scope);
            Self {
                tree,
                root,
                mapped,
                owner,
            }
        }

        fn lay_out(&self) {
            telar::compute_layout(
                self.root,
                telar::AvailableSpace::Definite(SIDE),
                telar::AvailableSpace::Definite(SIDE),
            )
            .expect("the page lays out");
        }

        fn inked(&mut self) -> bool {
            self.lay_out();
            self.tree.commands().iter().any(|command| {
                matches!(command, DrawCommand::Rect { style, rect, .. }
                    if style.fill == Some(Paint::Solid(INK)) && rect.width > 0.0)
            })
        }

        /// Presses and releases at a point through the pointer, and answers whether anything took the tap.
        fn tap(&mut self, x: f64, y: f64) -> bool {
            self.lay_out();
            let before = PRESSED.with(Cell::get);
            for event in [
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
            PRESSED.with(Cell::get) > before
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            telar::dispose_owner(self.owner);
        }
    }

    fn builds(id: &str) -> Vec<Seen> {
        BUILT.with(|built| built.borrow().get(id).cloned().unwrap_or_default())
    }

    fn subscriptions() -> (usize, usize) {
        (SUBSCRIBED.with(Cell::get), UNSUBSCRIBED.with(Cell::get))
    }

    /// F-10.52's rule for a tool holds for an area its expression hides: tested by a press through the pointer, not by asking the area. Hidden, it draws nothing and nothing in it takes a press or claims the input region; shown again, it is the same nodes, never rebuilt.
    #[test]
    fn an_area_hidden_by_its_expression_paints_nothing_and_takes_no_press() {
        let mut rig = Rig::new(free(Some("$probe.on"), vec![placed("a", &[])]));
        assert!(rig.inked(), "shown before its expression first answers");
        assert!(rig.tap(20.0, 20.0));
        assert!(!telar::interactive_rects().is_empty());

        publish(false, 0.0);
        assert!(!rig.inked(), "hidden, it paints nothing");
        assert!(!rig.tap(20.0, 20.0), "and takes no press");
        assert!(
            telar::interactive_rects().is_empty(),
            "nor any of the input region: {:?}",
            telar::interactive_rects()
        );

        publish(true, 0.0);
        assert!(rig.inked());
        assert!(rig.tap(20.0, 20.0));
        assert_eq!(builds("a").len(), 1, "flipping it rebuilt nothing");
    }

    /// A binding may name a variable before the user sets it: until then its instance draws its written options, and once the variable is set it comes alive, with no reload — and goes back to the written options once it is removed. Waiting is not failing, as `layout check` only warns of it: nothing is reported, and an area whose `visible` waits is shown.
    #[test]
    fn a_binding_to_a_variable_set_later_comes_alive_when_it_is_set() {
        automation::vars::remove("exprs_test_later");
        automation::vars::remove("exprs_test_later_shown");
        let rig = Rig::new(free(
            Some("$exprs_test_later_shown"),
            vec![placed("a", &[("date_format", "$exprs_test_later")])],
        ));
        let written = config::ClockConfig::default().date_format;
        let format = || builds("a").last().map(|seen| seen.date_format.clone());
        assert_eq!(format(), Some(written.clone()));
        rig.lay_out();
        let chip = crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
            .instance(&GroupId::new("g"), &PlacedId::new("a"));
        assert!(
            crate::rects::rect(&chip).is_some_and(|rect| rect.width > 0.0),
            "an area whose `visible` waits is shown"
        );
        let reported: Vec<String> = automation::failures::report()
            .findings()
            .map(|finding| finding.message.english())
            .filter(|said| said.contains("exprs_test_later"))
            .collect();
        assert!(reported.is_empty(), "a wait is no failure: {reported:?}");

        automation::vars::set(
            "exprs_test_later",
            services::state::Var::Text("%Y".to_string()),
        )
        .unwrap();
        assert_eq!(format(), Some("%Y".to_string()));
        automation::vars::remove("exprs_test_later");
        assert_eq!(format(), Some(written));
    }

    /// T-8.2's acceptance: a binding re-evaluates only when what it reads changes, and its instance alone is built again, with what it keeps in its store intact (F-3.4).
    #[test]
    fn a_bound_value_that_changes_rebuilds_its_own_instance_and_nothing_else() {
        let theme = Config::default().resolve_theme();
        let red = Color::from_hex("#ff0000").expect("a colour");
        let _rig = Rig::new(free(
            None,
            vec![
                placed(
                    "a",
                    &[
                        ("date_format", "if($probe.level > 1, '%A', '%d')"),
                        ("accent", "if($probe.level > 1, #ff0000, $theme.accent)"),
                    ],
                ),
                placed("b", &[]),
            ],
        ));
        let section = config::ClockConfig::default().date_format;
        assert_eq!(
            builds("a"),
            [Seen {
                date_format: section.clone(),
                accent: theme.accent,
                kept: 0,
            }],
            "with no reading yet, the written options and accent stand"
        );

        KEPT.set(&ui::host::InstanceId::new("a"), 7);
        publish(false, 2.0);
        assert_eq!(
            builds("a").last(),
            Some(&Seen {
                date_format: "%A".to_string(),
                accent: red,
                kept: 7,
            })
        );
        assert_eq!(builds("a").len(), 2, "both bindings moved, one rebuild");

        publish(false, 3.0);
        assert_eq!(
            builds("a").len(),
            2,
            "a reading that changes no value rebuilds nothing"
        );
        publish(true, 3.0);
        assert_eq!(builds("a").len(), 2, "nor does a field it never read");

        publish(false, 0.0);
        assert_eq!(builds("a").len(), 3);
        assert_eq!(
            builds("a").last().map(|seen| seen.accent),
            Some(theme.accent)
        );
        assert_eq!(
            builds("b").len(),
            1,
            "the instance beside it was never rebuilt"
        );
    }

    /// T-8.6 on screen: what a komponent holds reads each use's own parameters — one the use sets, one left to a default that is itself a live reading — and two uses of one komponent are two instances, each with what it keeps under its own id.
    #[test]
    fn each_use_of_a_komponent_reads_its_own_parameters_and_keeps_its_own_state() {
        let red = Color::from_hex("#ff0000").expect("a colour");
        let stored: layout::Layout = toml::from_str(
            r#"
            id = "mine"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "readings"
            kind = "free"
            rect = { x = 0.0, y = 0.0, w = 1.0, h = 1.0 }
            [[outputs.layers.top.areas.groups]]
            id = "a"
            place = "zone"
            zone = "start"
            komponent = "pill"
            [outputs.layers.top.areas.groups.parameters]
            label = "'%A'"
            [[outputs.layers.top.areas.groups]]
            id = "b"
            place = "zone"
            zone = "start"
            komponent = "pill"
            "#,
        )
        .expect("the layout parses");
        let pill: layout::Komponent = toml::from_str(
            r#"
            [parameters.label]
            type = "text"
            default = "'%d'"
            [parameters.lit]
            type = "bool"
            default = "$probe.on"
            [[children]]
            id = "c"
            module = "counter"
            [children.bindings]
            date_format = "$label"
            accent = "if($lit, #ff0000, #00ff00)"
            "#,
        )
        .expect("the komponent parses");
        let library = layout::Library::default().with_komponent("pill", pill);
        let (resolved, report) = layout::resolve(&stored, &library, SCREEN, None);
        assert!(report.is_clean(), "{}", report.render());
        let area = resolved
            .area(LayerKind::Top, &AreaId::new("readings"))
            .expect("the area resolves")
            .clone();
        let (a, b) = ("readings.a/c", "readings.b/c");
        KEPT.set(&ui::host::InstanceId::new(a), 1);
        KEPT.set(&ui::host::InstanceId::new(b), 2);
        let _rig = Rig::new(area);
        let formats = |id: &str| {
            builds(id)
                .last()
                .map(|seen| (seen.date_format.clone(), seen.kept))
        };
        assert_eq!(
            formats(a),
            Some(("%A".to_string(), 1)),
            "the use's own value"
        );
        assert_eq!(formats(b), Some(("%d".to_string(), 2)), "the default");

        publish(true, 0.0);
        for id in [a, b] {
            assert_eq!(
                builds(id).last().map(|seen| seen.accent),
                Some(red),
                "{id} follows the reading its parameter's default reads"
            );
        }
    }

    /// F-2.16 keeps a hidden window's tree, so what stops a hidden window's bindings is the window being off screen, not the tree going away.
    #[test]
    fn a_hidden_window_s_bindings_stop_reading_and_start_again_when_it_is_shown() {
        let rig = Rig::new(free(
            None,
            vec![placed("a", &[("date_format", "if($probe.on, '%A', '%d')")])],
        ));
        let (subscribed, unsubscribed) = subscriptions();
        assert!(subscribed > 0);

        rig.mapped.set(false);
        assert_eq!(subscriptions(), (subscribed, unsubscribed + 1));
        publish(true, 0.0);
        assert_eq!(builds("a").len(), 1, "a hidden window hears nothing");

        rig.mapped.set(true);
        assert_eq!(subscriptions(), (subscribed + 1, unsubscribed + 1));
        publish(true, 0.0);
        assert_eq!(
            builds("a").last().map(|seen| seen.date_format.as_str()),
            Some("%A")
        );
    }

    /// A free area holding a group `g` of `child` repeated over `repeat`, stacked when `stacked`, and beside it a plain group holding `b`.
    fn repeating(repeat: &str, stacked: bool, child: ResolvedInstance) -> ResolvedArea {
        let mut area = free(None, vec![placed("b", &[])]);
        area.groups[0].id = GroupId::new("plain");
        area.groups.insert(
            0,
            ResolvedGroup {
                id: GroupId::new("g"),
                kind: GroupKind::Zone { zone: Zone::Start },
                stacked,
                repeat: Some(written(repeat)),
                komponent: None,
                children: vec![child],
            },
        );
        area
    }

    /// The child `p`, labelled with what its copy reads.
    fn labelled(module: &str) -> ResolvedInstance {
        ResolvedInstance {
            module: module.to_string(),
            ..placed("p", &[("date_format", "fmt('{}:{}', $index, $item)")])
        }
    }

    fn label(id: &str) -> Option<String> {
        builds(id).last().map(|seen| seen.date_format.clone())
    }

    fn drawn(id: &str) -> bool {
        crate::rects::instance(Some(SCREEN), &PlacedId::new(id)).is_some()
    }

    /// DEC-23: a repeated group draws its child once per item, each copy `<id>#<index>` reading its own item and index, and the child as written is never drawn itself.
    #[test]
    fn a_repeated_group_draws_one_copy_per_item_reading_its_item_and_index() {
        let _rig = Rig::new(repeating("$probe.names", false, labelled("counter")));
        assert!(
            !drawn("p#0") && builds("p#0").is_empty(),
            "no copies before the list first answers"
        );

        publish_lists(&["a", "b"], &[]);
        assert_eq!(label("p#0").as_deref(), Some("0:a"));
        assert_eq!(label("p#1").as_deref(), Some("1:b"));
        assert!(drawn("p#0") && drawn("p#1") && !drawn("p#2"));
        assert!(!drawn("p") && builds("p").is_empty());
        let group = crate::rects::rect(
            &crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
                .group(&GroupId::new("g")),
        );
        assert!(group.is_some(), "the group is one box around its copies");
    }

    /// What a drawn copy reads as `$item` is there for the editor, live, for as long as the group is drawn: a popover opened on a copy shows what that copy reads.
    #[test]
    fn what_each_drawn_copy_reads_is_there_for_the_editor_while_it_is_drawn() {
        let group =
            crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
                .group(&GroupId::new("g"));
        let rig = Rig::new(repeating("$probe.names", false, labelled("counter")));
        let second = telar::memo({
            let group = group.clone();
            move || crate::expressions::drawn_item(&group, 1)
        });
        assert_eq!(second.get(), None, "no copies before the list answers");

        publish_lists(&["a", "b"], &[]);
        assert_eq!(second.get(), Some(Value::text("b")));
        publish_lists(&["a", "c"], &[]);
        assert_eq!(second.get(), Some(Value::text("c")), "and follows it");

        drop(rig);
        assert_eq!(
            crate::expressions::drawn_item(&group, 1),
            None,
            "and goes with it"
        );
    }

    /// A list that grows or shrinks adds or drops copies at its end and builds nothing else again; an item that changes in place rebuilds the copy that reads it, alone.
    #[test]
    fn a_list_that_changes_adds_and_drops_copies_without_rebuilding_the_rest() {
        let _rig = Rig::new(repeating("$probe.names", false, labelled("counter")));
        publish_lists(&["a"], &[]);
        assert_eq!(builds("p#0").len(), 1);

        publish_lists(&["a", "b"], &[]);
        assert_eq!(builds("p#0").len(), 1, "the copy already drawn stays");
        assert_eq!(label("p#1").as_deref(), Some("1:b"));

        publish(true, 9.0);
        assert_eq!(
            (builds("p#0").len(), builds("p#1").len()),
            (1, 1),
            "a reading beside the list builds no copy again"
        );

        publish_lists(&["a"], &[]);
        assert!(!drawn("p#1"), "the copy past the end is dropped");
        assert_eq!(builds("p#0").len(), 1);

        publish_lists(&["z"], &[]);
        assert_eq!(builds("p#0").len(), 2);
        assert_eq!(label("p#0").as_deref(), Some("0:z"));
        assert_eq!(
            builds("b").len(),
            1,
            "the group beside the repeated one was never rebuilt"
        );
    }

    /// DEC-23: each copy keeps its own state, keyed by its index, through rebuilds; forgetting the child forgets every copy of it (F-3.4).
    #[test]
    fn each_copy_keeps_its_own_state_and_forgetting_the_child_forgets_them_all() {
        let _rig = Rig::new(repeating("$probe.names", false, labelled("counter")));
        publish_lists(&["a", "b"], &[]);
        let (first, second) = (
            ui::host::InstanceId::new("p#0"),
            ui::host::InstanceId::new("p#1"),
        );
        KEPT.set(&second, 5);
        publish_lists(&["a", "c"], &[]);
        assert_eq!(builds("p#1").last().map(|seen| seen.kept), Some(5));
        assert_eq!(KEPT.get(&first), 0, "the copy beside it keeps its own");

        crate::layouts::forget_gone(&layout::Placed {
            instances: [PlacedId::new("p")].into(),
            ..layout::Placed::default()
        });
        assert_eq!(KEPT.get(&second), 0);
    }

    /// DEC-23: a stacked group's copies are its pages, one shown at a time, and a list that changes length keeps the page on show.
    #[test]
    fn a_stacked_repeated_group_shows_its_copies_one_at_a_time() {
        let mut rig = Rig::new(repeating("$probe.names", true, labelled("counter")));
        publish_lists(&["a", "b", "c"], &[]);
        assert_eq!(label("p#0").as_deref(), Some("0:a"));
        assert!(builds("p#1").is_empty(), "only the page on show is built");

        rig.lay_out();
        let stack = crate::rects::rect(
            &crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
                .group(&GroupId::new("g")),
        )
        .expect("the stack is drawn");
        let at = (
            f64::from(stack.x + stack.width / 2.0),
            f64::from(stack.y + stack.height / 2.0),
        );
        rig.tree.on_event(&Event::PointerMoved {
            x: at.0,
            y: at.1,
            source: PointerSource::Mouse,
        });
        rig.tree.on_event(&Event::Scrolled {
            delta: telar::ScrollDelta::Pixels { x: 0.0, y: 60.0 },
            x: at.0,
            y: at.1,
        });
        assert_eq!(
            label("p#1").as_deref(),
            Some("1:b"),
            "a notch shows the next"
        );

        publish_lists(&["a", "b"], &[]);
        assert!(drawn("p#1"), "the page on show stays on show");
        assert!(!drawn("p#0"));
    }

    /// A list longer than [`crate::expressions::MAX_COPIES`] draws its first items only, and says so where the `repeat` is written for as long as it is that long; a source answering with a huge list cannot stall the shell.
    #[test]
    fn a_list_longer_than_the_cap_draws_its_first_items_and_is_reported() {
        let max = crate::expressions::MAX_COPIES;
        let truncated = || {
            automation::failures::report()
                .findings()
                .map(|finding| finding.message.english())
                .filter(|said| said.contains(&format!("only the first {max} are drawn")))
                .count()
        };
        let mut area = repeating("$probe.names", false, labelled("counter"));
        // The failures registry is process-wide: a group id of its own keeps the other repeat tests from recovering this site.
        area.groups[0].id = GroupId::new("capped");
        let _rig = Rig::new(area);
        let names: Vec<&'static str> = (0..max + 5).map(|at| &*format!("n{at}").leak()).collect();
        publish_lists(&names, &[]);
        assert!(drawn(&format!("p#{}", max - 1)));
        assert!(!drawn(&format!("p#{max}")), "nothing past the cap");
        assert_eq!(truncated(), 1);

        publish_lists(&["a", "b"], &[]);
        assert!(drawn("p#1") && !drawn("p#2"));
        assert_eq!(
            truncated(),
            0,
            "a list within the cap is no longer reported"
        );
    }

    /// On the lock screen a private list reads as empty, so a group repeated over it draws nothing, while one over a public list draws its copies there.
    #[test]
    fn on_the_lock_screen_a_private_list_draws_no_copies() {
        let _rig = Rig::shown_to(
            repeating("$probe.secrets", false, labelled("reader")),
            Audience::Anyone,
        );
        publish_lists(&["a"], &["one", "two"]);
        assert!(!drawn("p#0") && builds("p#0").is_empty());

        let _rig = Rig::shown_to(
            repeating("$probe.names", false, labelled("reader")),
            Audience::Anyone,
        );
        publish_lists(&["a"], &["one", "two"]);
        assert_eq!(label("p#0").as_deref(), Some("0:a"));
    }

    /// On a bar a repeated group's copies are chips in its zone beside every other chip, and the list growing builds the new chip alone.
    #[test]
    fn on_a_bar_a_repeated_group_s_copies_are_chips_of_its_zone() {
        let mut area = repeating("$probe.names", false, labelled("counter"));
        area.kind = ResolvedAreaKind::Bar {
            edge: config::Edge::Top,
            thickness: 32.0,
            length: layout::Extent::Fill,
            offset: 0.0,
            shape: layout::BarShape::default(),
            autohide: None,
        };
        let _rig = Rig::new(area);
        let chips = || builds("counter").len();
        assert_eq!(chips(), 1, "the plain group's chip, and no copy yet");

        publish_lists(&["a"], &[]);
        assert!(drawn("p#0"));
        assert_eq!(chips(), 2);

        publish_lists(&["a", "b"], &[]);
        assert!(drawn("p#1"));
        assert_eq!(chips(), 3, "only the new copy's chip was built");
        assert!(
            crate::rects::rect(
                &crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
                    .group(&GroupId::new("g")),
            )
            .is_some()
        );

        publish_lists(&[], &[]);
        assert!(!drawn("p#0") && !drawn("p#1"));
    }

    /// A bar chip's bindings follow the same rule as any other instance's: it is built again alone under what they say, and stays where the bar tracks its chips.
    #[test]
    fn a_bound_chip_on_a_bar_rebuilds_alone_and_keeps_its_place() {
        let red = Color::from_hex("#ff0000").expect("a colour");
        let mut area = free(
            None,
            vec![
                placed(
                    "a",
                    &[
                        ("date_format", "if($probe.level > 1, '%A', '%d')"),
                        ("accent", "if($probe.level > 1, #ff0000, $theme.accent)"),
                    ],
                ),
                ResolvedInstance {
                    module: "reader".to_string(),
                    ..placed("b", &[])
                },
            ],
        );
        area.kind = ResolvedAreaKind::Bar {
            edge: config::Edge::Top,
            thickness: 32.0,
            length: layout::Extent::Fill,
            offset: 0.0,
            shape: layout::BarShape::default(),
            autohide: None,
        };
        let rig = Rig::new(area);
        assert_eq!(builds("counter").len(), 1);

        publish(false, 2.0);
        assert_eq!(
            builds("counter")
                .last()
                .map(|seen| (seen.date_format.as_str(), seen.accent)),
            Some(("%A", red))
        );
        assert_eq!(builds("counter").len(), 2);
        assert_eq!(
            builds("reader").len(),
            1,
            "the chip beside it was never rebuilt"
        );
        rig.lay_out();
        assert!(
            crate::rects::rect(
                &crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("readings"))
                    .instance(&GroupId::new("g"), &PlacedId::new("a")),
            )
            .is_some_and(|rect| rect.width > 0.0),
            "the rebuilt chip is where the bar says its chip is"
        );
    }

    /// A parent that writes the whole bar for every output; what a child extending it writes is in [`KEYED`].
    const KEYED_PARENT: &str = r#"
        id = "keys-parent"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar"
        kind = "bar"
        edge = "top"
        thickness = 32
        visible = "1 +"
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        repeat = "$keys_test_nothing.at +"
        [[outputs.layers.top.areas.groups.children]]
        id = "chip"
        module = "keys-test"
        [outputs.layers.top.areas.groups.children.bindings]
        accent = "1 +"
        show = "2 +"
    "#;

    /// A child that inherits the bar's `visible` and `repeat`, overrides one binding on its own output rule and another on one workspace.
    const KEYED: &str = r#"
        id = "keys"
        extends = "keys-parent"
        [[outputs]]
        match = "DP-*"
        [[outputs.layers.top.areas]]
        id = "bar"
        [[outputs.layers.top.areas.groups]]
        id = "start"
        [[outputs.layers.top.areas.groups.children]]
        id = "chip"
        [outputs.layers.top.areas.groups.children.bindings]
        show = "3 +"
        [[outputs.workspaces]]
        match = "2"
        [[outputs.workspaces.layers.top.areas]]
        id = "bar"
        [[outputs.workspaces.layers.top.areas.groups]]
        id = "start"
        [[outputs.workspaces.layers.top.areas.groups.children]]
        id = "chip"
        [outputs.workspaces.layers.top.areas.groups.children.bindings]
        accent = "4 +"
    "#;

    /// A failure while running is named by the file and key `layout check` gives the same expression: the level that wrote it, whether that is a layout this one extends, an output rule or a workspace rule, so the notice and the check agree on where it is written; a copy's is the child it is made from.
    #[test]
    fn a_running_expression_is_keyed_as_layout_check_keys_it() {
        use crate::expressions::{Slot, expression_key};
        use crate::rects::Node;

        let parent: layout::Layout = toml::from_str(KEYED_PARENT).expect("the parent parses");
        let child: layout::Layout = toml::from_str(KEYED).expect("the child parses");
        let catalogue = crate::catalogue::Descriptors::installed();
        let checked: Vec<(String, String)> = [&parent, &child]
            .into_iter()
            .flat_map(|layout| {
                layout::validate(layout, &catalogue)
                    .findings()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .map(|finding| (finding.file.display().to_string(), finding.key))
            .collect();
        let known = layout::Library::of_layouts([parent.clone(), child.clone()]);
        let on = |workspace: &str| {
            let active = layout::ActiveWorkspace {
                name: workspace.to_string(),
                ..layout::ActiveWorkspace::default()
            };
            layout::resolve(&child, &known, SCREEN, Some(&active)).0
        };
        let bar_node = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar"));
        let group_node = bar_node.group(&GroupId::new("start"));
        let copy_node = group_node.instance(&GroupId::new("start"), &PlacedId::new("chip").copy(2));
        let keys = |workspace: &str| {
            let resolved = on(workspace);
            let bar = resolved
                .area(LayerKind::Top, &AreaId::new("bar"))
                .expect("the bar resolves")
                .clone();
            let group = bar.groups[0].clone();
            let chip = group.children[0].clone();
            let key = |origin: &layout::Origin, node: &Node, slot: Slot| {
                expression_key(origin, node, slot)
            };
            [
                key(
                    &bar.visible.expect("inherited").origin,
                    &bar_node,
                    Slot::Visible,
                ),
                key(
                    &group.repeat.expect("inherited").origin,
                    &group_node,
                    Slot::Repeat,
                ),
                key(
                    &chip.bindings["show"].origin,
                    &copy_node,
                    Slot::Binding("show"),
                ),
                key(
                    &chip.bindings["accent"].origin,
                    &copy_node,
                    Slot::Binding("accent"),
                ),
            ]
        };
        let parents = |key: &str| ("layouts/keys-parent.toml".to_string(), key.to_string());
        let own = |key: &str| ("layouts/keys.toml".to_string(), key.to_string());

        let [visible, repeat, show, accent] = keys("1");
        assert_eq!(
            visible,
            parents("outputs.*.layers.top.areas.bar.visible"),
            "inherited: the parent's file and rule"
        );
        assert_eq!(
            repeat,
            parents("outputs.*.layers.top.areas.bar.groups.start.repeat")
        );
        assert_eq!(
            show,
            own("outputs.DP-*.layers.top.areas.bar.groups.start.children.chip.bindings.show"),
            "overridden by the child's output rule, and a copy's at the child it is made from"
        );
        assert_eq!(
            accent,
            parents("outputs.*.layers.top.areas.bar.groups.start.children.chip.bindings.accent")
        );
        let [_, _, _, ruled] = keys("2");
        assert_eq!(
            ruled,
            own(
                "outputs.DP-*.workspaces.2.layers.top.areas.bar.groups.start.children.chip.bindings.accent"
            ),
            "the workspace rule that overrides it while its workspace is up"
        );

        for key in [visible, repeat, show, accent, ruled] {
            assert!(
                checked.contains(&key),
                "`{key:?}` is not one of {checked:#?}"
            );
        }
    }
}
