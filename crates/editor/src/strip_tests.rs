//! The mode strip (T-4.11) and the history it jumps through (T-4.12), pressed through the pointer as the host's window lays them out: what it says of an undo, a redo and the first edit of the built-in layout, where it sits, its grip, its layer list, its `+`, and the history list that walks several entries at once.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DismissRegistration, DrawCommand, Event, Key,
        LayoutItem, LayoutStyle, ModifiersState, NodeId, PointerButton, PointerSource, Rect,
        compute_layout,
    };

    use config::Edge;
    use layout::{
        AreaId, BUILT_IN, GroupId, InstanceId, LayerKind, Layout, LayoutId, LayoutOp, Site, Spot,
    };
    use surfaces::menu::{Asked, Pointed};
    use surfaces::rects::Node;
    use surfaces::transient;

    use crate::host::{self, BOTTOM_EDGE_CLEARANCE, Under};
    use crate::keys::{self, Press};
    use crate::mode::{self, Compositor};
    use crate::rig::{Rig, SCREEN, rig};
    use crate::{context, history, session};

    const WIDTH: f32 = 1920.0;
    const HEIGHT: f32 = 1080.0;

    fn page() -> LayoutStyle {
        LayoutStyle::new().width(WIDTH).height(HEIGHT)
    }

    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn enter(layer: LayerKind) -> DismissRegistration {
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
        let id = host::transient_id(SCREEN);
        DismissRegistration::new(Rc::new(move || transient::close(&id)))
    }

    /// The host of the mode that is up, laid out over the whole screen as its window lays it out, the pointer followed as the window follows it.
    struct Screen {
        tree: ComponentList,
        node: NodeId,
    }

    impl Screen {
        fn of_mode() -> Self {
            let mode = mode::current().expect("a mode is up");
            let item = host::tree(&mode, Under::Nothing).expect("the host builds");
            let root = Container::new(page(), vec![item]).expect("a page");
            let node = root.layout_node();
            let screen = Self {
                tree: ComponentList::new(Pointed::new(Box::new(root))),
                node,
            };
            screen.settle();
            screen
        }

        fn settle(&self) {
            compute_layout(
                self.node,
                AvailableSpace::Definite(WIDTH),
                AvailableSpace::Definite(HEIGHT),
            )
            .expect("the host lays out");
            for _ in 0..3 {
                telar::relayout_if_dirty();
            }
        }

        fn route(&mut self, event: &Event) {
            telar::observe_keyboard(event);
            if !telar::dispatch_overlays(event) {
                self.tree.on_event(event);
            }
            self.settle();
        }

        fn texts(&self) -> Vec<(String, Rect)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, [a, b, c, d, e, f]| {
                if let DrawCommand::Text { text, rect, .. } = command {
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

        fn said(&self) -> Vec<String> {
            self.texts().into_iter().map(|(text, _)| text).collect()
        }

        fn where_is(&self, wanted: &str) -> Rect {
            self.texts()
                .into_iter()
                .find(|(text, _)| text == wanted)
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| panic!("{wanted:?} is drawn: {:?}", self.said()))
        }

        fn press(&mut self, wanted: &str) {
            let rect = self.where_is(wanted);
            let at = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            self.click(at);
        }

        fn click(&mut self, (x, y): (f32, f32)) {
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
                self.route(&event);
            }
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            let steps = 4;
            self.route(&Event::PointerMoved {
                x: f64::from(from.0),
                y: f64::from(from.1),
                source: PointerSource::Mouse,
            });
            self.route(&Event::PointerPressed {
                x: f64::from(from.0),
                y: f64::from(from.1),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
            for step in 1..=steps {
                let part = step as f32 / steps as f32;
                self.route(&Event::PointerMoved {
                    x: f64::from(from.0 + (to.0 - from.0) * part),
                    y: f64::from(from.1 + (to.1 - from.1) * part),
                    source: PointerSource::Mouse,
                });
            }
            self.route(&Event::PointerReleased {
                x: f64::from(to.0),
                y: f64::from(to.1),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }
    }

    fn clock_in(group: &str) -> Spot {
        Spot {
            site: Site::everywhere(LayerKind::Top),
            area: AreaId::new("bar-top"),
            group: GroupId::new(group),
        }
    }

    fn move_clock(from: &str, to: &str) -> LayoutOp {
        LayoutOp::MoveInstance {
            from: clock_in(from),
            to: clock_in(to),
            id: InstanceId::new("clock"),
            index: 0,
        }
    }

    const EDITS: [&str; 3] = [
        "Move the clock to the start",
        "Move the clock to the end",
        "Remove the clock",
    ];

    /// Commits the three edits of [`EDITS`], answering the layout as it stood before them and after each.
    fn three_edits(rig: &Rig) -> Vec<Layout> {
        let stored = || rig.store.borrow().active().clone();
        let ops = [
            move_clock("center", "start"),
            move_clock("start", "end"),
            LayoutOp::DeleteInstance {
                spot: clock_in("end"),
                id: InstanceId::new("clock"),
            },
        ];
        let mut layouts = vec![stored()];
        for (label, op) in EDITS.into_iter().zip(ops) {
            context::commit(label.to_string(), vec![op]).expect("the edit commits");
            layouts.push(stored());
        }
        layouts
    }

    fn key(ch: char, modifiers: ModifiersState) {
        let key = Key::Char(ch);
        let event = Event::KeyPressed {
            key: key.clone(),
            modifiers,
        };
        telar::observe_keyboard(&event);
        if !telar::dispatch_overlays(&event) {
            keys::press_as(&key, modifiers, Press::First);
        }
        telar::observe_keyboard(&Event::KeyReleased { key, modifiers });
        keys::settle_released();
    }

    fn ctrl(shift: bool) -> ModifiersState {
        ModifiersState {
            is_ctrl: true,
            is_shift: shift,
            ..ModifiersState::default()
        }
    }

    /// The first edit of the built-in layout lands in a copy, and the strip says so, naming the copy; the next edit, already in the copy, says nothing of it.
    #[test]
    fn the_first_edit_of_the_built_in_layout_says_it_forked() {
        let rig = rig("strip-fork");
        let _scope = Scope::new();
        rig.store
            .borrow_mut()
            .use_layout(&LayoutId::new(BUILT_IN))
            .expect("the built-in layout is there");
        let _mode = enter(LayerKind::Top);
        let screen = Screen::of_mode();

        context::commit(EDITS[0].to_string(), vec![move_clock("center", "start")])
            .expect("the edit commits");
        assert_eq!(rig.store.borrow().active_id().as_str(), "custom");
        let forked = "The built-in layout is read-only: forked into 'custom'";
        assert_eq!(mode::confirmation().peek().as_deref(), Some(forked));
        screen.settle();
        assert!(
            screen.said().iter().any(|text| text == forked),
            "{:?}",
            screen.said()
        );

        mode::unsay(mode::said_serial());
        context::commit(EDITS[1].to_string(), vec![move_clock("start", "end")])
            .expect("the edit commits");
        assert_eq!(
            mode::confirmation().peek(),
            None,
            "only the first edit forks"
        );
        services::state::update(|state| state.layout = None);
    }

    /// Ctrl+Z and Ctrl+Shift+Z say what they took back and put again; one with nothing to walk over says why instead.
    #[test]
    fn undo_and_redo_say_what_they_took_back_and_put_again() {
        let rig = rig("strip-undone");
        let _scope = Scope::new();
        let _mode = enter(LayerKind::Desktop);
        context::commit(EDITS[0].to_string(), vec![move_clock("center", "start")])
            .expect("the edit commits");

        key('z', ctrl(false));
        assert_eq!(
            mode::confirmation().peek().as_deref(),
            Some("Undone: Move the clock to the start")
        );
        assert_eq!(rig.undo_label(), None);
        key('z', ctrl(true));
        assert_eq!(
            mode::confirmation().peek().as_deref(),
            Some("Redone: Move the clock to the start")
        );
        assert_eq!(rig.undo_label().as_deref(), Some(EDITS[0]));

        key('z', ctrl(true));
        assert_eq!(mode::confirmation().peek(), None);
        assert!(mode::refusal().peek().is_some(), "nothing to redo says so");
    }

    /// What the strip says goes once its time is up, unless something newer was said since.
    #[test]
    fn what_the_strip_says_goes_once_its_time_is_up() {
        let _rig = rig("strip-unsay");
        let _scope = Scope::new();
        mode::confirm("first");
        let first = mode::said_serial();
        mode::refuse("second");
        mode::unsay(first);
        assert_eq!(mode::refusal().peek().as_deref(), Some("second"));
        assert_eq!(mode::confirmation().peek(), None);
        mode::unsay(mode::said_serial());
        assert_eq!(mode::refusal().peek(), None);
    }

    /// Where the "Done" of the strip is drawn in `layer`'s mode.
    fn done_in(layer: LayerKind) -> Rect {
        let _mode = enter(layer);
        let rect = Screen::of_mode().where_is("Done");
        mode::leave();
        rect
    }

    /// In the top mode the strip sits higher while the bottom edge has no bar, so the strip along that edge a new bar is pulled out of stays reachable; once a bar is there, it sits where every other mode puts it.
    #[test]
    fn the_top_modes_strip_rises_clear_of_a_bottom_edge_with_no_bar() {
        let _rig = rig("strip-clear");
        let _scope = Scope::new();
        assert!(host::clears_bottom_edge(LayerKind::Top, SCREEN));
        assert!(!host::clears_bottom_edge(LayerKind::Background, SCREEN));
        let background = done_in(LayerKind::Background);
        let top = done_in(LayerKind::Top);
        assert_eq!(background.y - top.y, BOTTOM_EDGE_CLEARANCE);

        let _mode = enter(LayerKind::Top);
        crate::modes::top::create_on(Edge::Bottom).expect("a bar is made on the bottom edge");
        mode::leave();
        assert!(!host::clears_bottom_edge(LayerKind::Top, SCREEN));
        assert_eq!(done_in(LayerKind::Background).y, done_in(LayerKind::Top).y);
    }

    /// The grip drags the strip, and it stays where it was let go across modes.
    #[test]
    fn the_grip_drags_the_strip_and_it_stays_there() {
        let _rig = rig("strip-grip");
        let _scope = Scope::new();
        host::strip_moved().set((0.0, 0.0));
        let _mode = enter(LayerKind::Desktop);
        let mut screen = Screen::of_mode();
        let before = screen.where_is("Done");
        let name = screen.where_is("Desktop ▾");
        let grip = (name.x - 40.0, name.y + name.height / 2.0);
        screen.drag(grip, (grip.0 - 300.0, grip.1 - 200.0));
        let after = screen.where_is("Done");
        assert_eq!((after.x - before.x, after.y - before.y), (-300.0, -200.0));

        mode::leave();
        let _other = enter(LayerKind::Background);
        let elsewhere = Screen::of_mode().where_is("Done");
        assert_eq!(elsewhere.y, after.y);
        assert_eq!(host::strip_moved().peek(), (-300.0, -200.0));
        host::strip_moved().set((0.0, 0.0));
    }

    /// The layer's name opens every layer to switch to, the one being edited ticked; picking another switches to it.
    #[test]
    fn the_layer_name_opens_the_layers_to_switch_to() {
        let _rig = rig("strip-layers");
        let _scope = Scope::new();
        let _mode = enter(LayerKind::Desktop);
        let mut screen = Screen::of_mode();
        screen.press("Desktop ▾");
        assert!(
            screen.said().iter().any(|text| text == "✓ Desktop"),
            "{:?}",
            screen.said()
        );
        screen.press("   Top");
        assert_eq!(mode::current().map(|mode| mode.layer), Some(LayerKind::Top));
    }

    /// The `+` opens the mode's Add: the palette on the desktop.
    #[test]
    fn the_plus_opens_the_modes_add() {
        let _rig = rig("strip-add");
        let _scope = Scope::new();
        let _mode = enter(LayerKind::Desktop);
        let mut screen = Screen::of_mode();
        screen.press("+");
        assert!(transient::is_open(crate::modes::palette::ID));
    }

    /// T-4.12: three back then two forward is three undos and two redos, landing where the second edit left the layout.
    #[test]
    fn travelling_three_back_then_two_forward_lands_on_the_second_edit() {
        let rig = rig("strip-travel");
        let _scope = Scope::new();
        let layouts = three_edits(&rig);
        let stored = || rig.store.borrow().active().clone();

        let back = session::travel(-3).expect("three back");
        assert_eq!(back.map(|step| step.label).as_deref(), Some(EDITS[0]));
        assert_eq!(stored(), layouts[0]);
        let forward = session::travel(2).expect("two forward");
        assert_eq!(
            forward.map(|step| step.said()).as_deref(),
            Some("Redone: Move the clock to the end")
        );
        assert_eq!(stored(), layouts[2]);
        assert_eq!(rig.undo_label().as_deref(), Some(EDITS[1]));
        assert_eq!(session::travel(0), Ok(None));
    }

    /// The strip's history lists where the layout is and how far each entry is, redo entries after; picking "At the start" walks all the way back, and picking a redo entry walks forward to it.
    #[test]
    fn the_strips_history_jumps_back_and_forward() {
        let rig = rig("strip-history");
        let _scope = Scope::new();
        let layouts = three_edits(&rig);
        let stored = || rig.store.borrow().active().clone();
        let _mode = enter(LayerKind::Top);
        let mut screen = Screen::of_mode();

        screen.press("History ▸");
        let said = screen.said();
        for (line, hint) in [
            ("   At the start", "3 back"),
            ("   Move the clock to the start", "2 back"),
            ("   Move the clock to the end", "1 back"),
            ("● Remove the clock", "now"),
        ] {
            assert!(said.iter().any(|text| text == line), "{line}: {said:?}");
            assert!(said.iter().any(|text| text == hint), "{hint}: {said:?}");
        }
        screen.press("   At the start");
        assert_eq!(stored(), layouts[0]);
        assert_eq!(
            mode::confirmation().peek().as_deref(),
            Some("Undone: Move the clock to the start")
        );

        screen.press("History ▸");
        let lines = history::lines();
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.marked(), line.hint.clone(), line.ahead))
                .collect::<Vec<_>>(),
            [
                ("● At the start".to_string(), "now".to_string(), false),
                (
                    "   Move the clock to the start".to_string(),
                    "1 forward".to_string(),
                    true
                ),
                (
                    "   Move the clock to the end".to_string(),
                    "2 forward".to_string(),
                    true
                ),
                (
                    "   Remove the clock".to_string(),
                    "3 forward".to_string(),
                    true
                ),
            ]
        );
        screen.press("   Move the clock to the end");
        assert_eq!(stored(), layouts[2]);
    }

    /// The context menu of anything on the edited layer has undo, redo and the history, and the strip's actions — the hook "Theme…" goes through.
    #[test]
    fn the_menu_in_a_mode_has_the_history_and_the_strip_actions() {
        let rig = rig("strip-menu");
        let _scope = Scope::new();
        three_edits(&rig);
        host::add_strip_action((|| "Strip action".to_string(), || {}));
        let _mode = enter(LayerKind::Top);
        let bar = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"));
        context::open(Asked {
            node: bar,
            window: LayerKind::Overlay,
            at: Some((400.0, 17.0)),
        })
        .expect("the menu opens");
        let rows = context::rows();
        for row in ["Undo", "Redo", "History", "Strip action"] {
            assert!(rows.contains(&row.to_string()), "{row}: {rows:?}");
        }
        let screen = Screen::of_mode();
        assert!(
            screen.said().iter().any(|text| text == "Strip action"),
            "{:?}",
            screen.said()
        );
    }
}
