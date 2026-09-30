//! The keyboard path through every session mode, pressed the way the host's window hears it — the dismiss stack first, then the host's keys — against a running shell's store and screen: every pointer gesture of an edit mode has a key, and each keyboard edit is one entry in the history (TA-4, WCAG 2.5.7).
//!
//! The windows of a headless shell are never built, so where the edited layer's areas, groups and instances are on screen is registered by hand, as their windows would register it, and so is the dismiss entry the host's window adds when it builds the host.

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use telar::{DismissRegistration, Event, Key, LayoutStyle, ModifiersState, NamedKey, Rect};
    use telar::{RectStyle, StyledContainer, signal};

    use config::Edge;
    use layout::{
        Anchor, Area, AreaId, AreaKind, BarShape, Extent, Group, GroupId, GroupKind, Instance,
        InstanceId, LayerKind, Layout, Representation, ResolvedArea, ResolvedAreaKind, Zone,
    };
    use surfaces::reconcile;
    use surfaces::rects::{self, Node, Part};
    use surfaces::transient;
    use ui::descriptor::{Built, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::{Host, WidgetSize};

    use crate::keys::{self, Chord, Direction, KeyOp, Press, Run, Scope};
    use crate::mode::{self, Compositor};
    use crate::rig::{Rig, SCREEN, rig_with};
    use crate::session::{self, Selection};
    use crate::{context, host, pie, popover};

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
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(face, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &[WidgetSize::S, WidgetSize::M, WidgetSize::L],
                build: face,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    /// An owner for the places a test registers, disposed when it ends.
    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
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
    const CTRL: ModifiersState = ModifiersState {
        is_ctrl: true,
        ..NONE
    };

    fn named(key: NamedKey) -> Key {
        Key::Named(key)
    }

    fn arrow(direction: Direction) -> Key {
        named(match direction {
            Direction::Left => NamedKey::ArrowLeft,
            Direction::Right => NamedKey::ArrowRight,
            Direction::Up => NamedKey::ArrowUp,
            Direction::Down => NamedKey::ArrowDown,
        })
    }

    /// A key going down as the host's window hears it: the keyboard's state, then the dismiss stack, then the host's keys. Answers whether anything took it.
    fn down(key: &Key, modifiers: ModifiersState, press: Press) -> bool {
        let event = Event::KeyPressed {
            key: key.clone(),
            modifiers,
        };
        telar::observe_keyboard(&event);
        telar::dispatch_overlays(&event) || keys::press_as(key, modifiers, press)
    }

    fn let_go(key: &Key, modifiers: ModifiersState) {
        telar::observe_keyboard(&Event::KeyReleased {
            key: key.clone(),
            modifiers,
        });
        keys::settle_released();
    }

    /// A key pressed and let go.
    fn tap(key: Key, modifiers: ModifiersState) -> bool {
        let taken = down(&key, modifiers, Press::First);
        let_go(&key, modifiers);
        taken
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

    fn area(layer: LayerKind, id: &str) -> Node {
        Node::area(Some(SCREEN), layer, &AreaId::new(id))
    }

    fn instance(layer: LayerKind, area_id: &str, group: &str, id: &str) -> Node {
        area(layer, area_id).instance(&GroupId::new(group), &InstanceId::new(id))
    }

    fn place(placed: &[(Node, Rect)]) {
        for (node, rect) in placed {
            rects::track_spanning(node.clone(), vec![signal(*rect)]);
        }
    }

    /// The area `id` of `layer` as the screen shows it now.
    fn shown(layer: LayerKind, id: &str) -> Option<ResolvedArea> {
        reconcile::desktops()[0]
            .resolved
            .layer(layer)?
            .areas
            .iter()
            .find(|area| area.id.as_str() == id)
            .cloned()
    }

    fn is_shown(node: &Node) -> bool {
        let Some(area) = shown(node.layer, node.area.as_str()) else {
            return false;
        };
        match &node.part {
            Part::Area => true,
            Part::Group(group) => area.groups.iter().any(|held| held.id == *group),
            Part::Instance(_, id) => area
                .groups
                .iter()
                .any(|group| group.children.iter().any(|child| child.id == *id)),
        }
    }

    /// One keyboard edit: pressed on `node` towards `direction` with `modifiers`, read back through `seen`.
    struct Stepped {
        node: Node,
        direction: Direction,
        seen: fn() -> String,
    }

    /// Everything one mode's keys are checked against.
    struct Case {
        test: &'static str,
        layer: LayerKind,
        layout: fn(&mut Layout),
        placed: fn() -> Vec<(Node, Rect)>,
        /// An arrow from the first selects the second.
        walk: (Node, Direction, Node),
        /// The areas Tab goes through from nothing selected, in order, before it comes round.
        tabbed: Vec<Node>,
        /// Enter on it opens its popover.
        customized: Node,
        removed: Node,
        moved: Stepped,
        resized: Stepped,
    }

    fn selected() -> Option<Node> {
        session::selected().node().cloned()
    }

    fn check(case: Case) {
        let rig: Rig = rig_with(case.test, case.layout);
        let _scope = Owner::new();
        let _host = enter(case.layer);
        place(&(case.placed)());

        let (from, direction, to) = &case.walk;
        assert!(session::select(Selection::of(from.clone())));
        assert!(tap(arrow(*direction), NONE));
        assert_eq!(
            selected().as_ref(),
            Some(to),
            "an arrow walks the selection"
        );

        session::clear_selection();
        for expected in case.tabbed.iter().chain(case.tabbed.first()) {
            assert!(tap(named(NamedKey::Tab), NONE));
            assert_eq!(selected().as_ref(), Some(expected), "Tab cycles the areas");
        }

        assert!(session::select(Selection::of(case.customized.clone())));
        assert!(tap(named(NamedKey::Enter), NONE));
        assert_eq!(
            popover::current(),
            Some(case.customized.clone()),
            "Enter customizes"
        );
        assert!(
            !tap(arrow(Direction::Right), NONE),
            "the open popover has the keys"
        );
        popover::close();

        assert!(tap(named(NamedKey::ContextMenu), NONE));
        assert!(
            transient::is_open(context::ID),
            "the menu key opens its menu"
        );
        transient::close(context::ID);

        assert!(session::select(Selection::of(case.removed.clone())));
        assert!(tap(named(NamedKey::Delete), NONE));
        assert!(!is_shown(&case.removed), "Delete takes it away");
        assert!(rig.undo_label().is_some());
        session::undo().expect("the removal is undone");
        assert!(is_shown(&case.removed), "undo puts it back");
        assert_eq!(rig.undo_label(), None);

        for (stepped, modifiers) in [(&case.moved, SHIFT), (&case.resized, CTRL)] {
            let key = arrow(stepped.direction);
            let before = (stepped.seen)();

            assert!(session::select(Selection::of(stepped.node.clone())));
            assert!(down(&key, modifiers, Press::First));
            assert_ne!((stepped.seen)(), before, "the first press previews a step");
            assert!(tap(named(NamedKey::Escape), NONE));
            assert!(
                selected().is_some(),
                "Esc took the held step, not the selection"
            );
            assert_eq!((stepped.seen)(), before, "Esc puts a held step back");
            assert_eq!(rig.undo_label(), None, "and records nothing");
            assert!(down(&key, modifiers, Press::Repeat));
            assert_eq!(
                (stepped.seen)(),
                before,
                "the rest of that press does nothing"
            );
            let_go(&key, modifiers);

            assert!(session::select(Selection::of(stepped.node.clone())));
            assert!(down(&key, modifiers, Press::First));
            let first = (stepped.seen)();
            assert!(down(&key, modifiers, Press::Repeat));
            assert!(down(&key, modifiers, Press::Repeat));
            let_go(&key, modifiers);
            assert_ne!((stepped.seen)(), first, "the repeats step further");
            assert!(rig.undo_label().is_some(), "the release commits");
            session::undo().expect("the step is undone");
            assert_eq!(
                (stepped.seen)(),
                before,
                "one undo takes back the whole press"
            );
            assert_eq!(rig.undo_label(), None, "the whole press was one entry");
        }

        assert!(session::select(Selection::of(from.clone())));
        tap(named(NamedKey::Escape), NONE);
        assert_eq!(selected(), None, "the first Esc clears the selection");
        assert!(mode::current().is_some(), "and leaves the mode up");
        tap(named(NamedKey::Escape), NONE);
        assert_eq!(mode::current(), None, "the next Esc leaves the mode");
    }

    fn top_layout(layout: &mut Layout) {
        let top = &mut layout.outputs[0].layers.top;
        top.areas.push(Area {
            id: AreaId::new("bar-bottom"),
            kind: Some(AreaKind::Bar {
                edge: Some(Edge::Bottom),
                thickness: Some(30.0),
                length: Some(Extent::Fill),
                offset: Some(0.0),
                shape: BarShape::default(),
                autohide: None,
            }),
            reserve: Some(false),
            groups: vec![Group {
                id: GroupId::new("center"),
                kind: Some(GroupKind::Zone { zone: Zone::Center }),
                children: vec![Instance {
                    id: InstanceId::new("clock-2"),
                    module: Some("clock".to_string()),
                    representation: Some(Representation::Chip),
                    ..Instance::default()
                }],
                ..Group::default()
            }],
            ..Area::default()
        });
    }

    fn top_placed() -> Vec<(Node, Rect)> {
        let top = LayerKind::Top;
        let bar = area(top, "bar-top");
        vec![
            (bar.clone(), Rect::new(0.0, 0.0, 1920.0, 34.0)),
            (
                area(top, "bar-bottom"),
                Rect::new(0.0, 1050.0, 1920.0, 30.0),
            ),
            (
                bar.group(&GroupId::new("start")),
                Rect::new(0.0, 0.0, 100.0, 34.0),
            ),
            (
                bar.group(&GroupId::new("center")),
                Rect::new(900.0, 0.0, 120.0, 34.0),
            ),
            (
                bar.group(&GroupId::new("end")),
                Rect::new(1800.0, 0.0, 120.0, 34.0),
            ),
            (
                instance(top, "bar-top", "start", "workspaces"),
                Rect::new(10.0, 2.0, 80.0, 30.0),
            ),
            (
                instance(top, "bar-top", "center", "clock"),
                Rect::new(920.0, 2.0, 80.0, 30.0),
            ),
            (
                instance(top, "bar-top", "end", "notes"),
                Rect::new(1810.0, 2.0, 80.0, 30.0),
            ),
        ]
    }

    /// Where the top bar's clock is: its zone and its place in it.
    fn clock_place() -> String {
        let bar = shown(LayerKind::Top, "bar-top").expect("the top bar");
        bar.groups
            .iter()
            .find_map(|group| {
                let at = group
                    .children
                    .iter()
                    .position(|child| child.id.as_str() == "clock")?;
                Some(format!("{}:{at}", group.id))
            })
            .unwrap_or_default()
    }

    fn top_thickness() -> String {
        match shown(LayerKind::Top, "bar-top").map(|bar| bar.kind) {
            Some(ResolvedAreaKind::Bar { thickness, .. }) => thickness.to_string(),
            _ => String::new(),
        }
    }

    #[test]
    fn top_mode_answers_every_key() {
        let top = LayerKind::Top;
        let clock = instance(top, "bar-top", "center", "clock");
        check(Case {
            test: "keys-top",
            layer: top,
            layout: top_layout,
            placed: top_placed,
            walk: (
                clock.clone(),
                Direction::Right,
                instance(top, "bar-top", "end", "notes"),
            ),
            tabbed: vec![area(top, "bar-top"), area(top, "bar-bottom")],
            customized: clock.clone(),
            removed: clock.clone(),
            moved: Stepped {
                node: clock,
                direction: Direction::Right,
                seen: clock_place,
            },
            resized: Stepped {
                node: area(top, "bar-top"),
                direction: Direction::Down,
                seen: top_thickness,
            },
        });
    }

    fn background_layout(layout: &mut Layout) {
        let background = &mut layout.outputs[0].layers.background;
        let half = |x: f32| layout::Rect {
            x,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        if let Some(AreaKind::WallpaperRegion { rect, .. }) = &mut background.areas[0].kind {
            *rect = Some(half(0.0));
        }
        background.areas.push(Area {
            id: AreaId::new("right"),
            kind: Some(AreaKind::WallpaperRegion {
                rect: Some(half(0.5)),
                source: None,
                fit: None,
                transition: None,
            }),
            ..Area::default()
        });
    }

    fn background_placed() -> Vec<(Node, Rect)> {
        vec![
            (
                area(LayerKind::Background, "background"),
                Rect::new(0.0, 0.0, 960.0, 1080.0),
            ),
            (
                area(LayerKind::Background, "right"),
                Rect::new(960.0, 0.0, 960.0, 1080.0),
            ),
        ]
    }

    fn region(id: &str) -> Option<layout::Rect> {
        match shown(LayerKind::Background, id)?.kind {
            ResolvedAreaKind::WallpaperRegion { rect, .. } => Some(rect),
            _ => None,
        }
    }

    fn left_region_x() -> String {
        region("background").map_or_else(String::new, |rect| rect.x.to_string())
    }

    fn right_region_w() -> String {
        region("right").map_or_else(String::new, |rect| rect.w.to_string())
    }

    #[test]
    fn background_mode_answers_every_key() {
        let background = LayerKind::Background;
        check(Case {
            test: "keys-background",
            layer: background,
            layout: background_layout,
            placed: background_placed,
            walk: (
                area(background, "background"),
                Direction::Right,
                area(background, "right"),
            ),
            tabbed: vec![area(background, "background"), area(background, "right")],
            customized: area(background, "right"),
            removed: area(background, "right"),
            moved: Stepped {
                node: area(background, "background"),
                direction: Direction::Right,
                seen: left_region_x,
            },
            resized: Stepped {
                node: area(background, "right"),
                direction: Direction::Left,
                seen: right_region_w,
            },
        });
    }

    fn widget(id: &str) -> Instance {
        Instance {
            id: InstanceId::new(id),
            module: Some("clock".to_string()),
            representation: Some(Representation::WidgetM),
            ..Instance::default()
        }
    }

    fn cell(id: &str, col: u32, child: &str) -> Group {
        Group {
            id: GroupId::new(id),
            kind: Some(GroupKind::Cell {
                col,
                row: 0,
                col_span: 1,
                row_span: 1,
            }),
            children: vec![widget(child)],
            ..Group::default()
        }
    }

    fn grid(id: &str, x: f32, groups: Vec<Group>) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x,
                    y: 0.1,
                    w: 0.5,
                    h: 0.9,
                }),
                cell: Some(80.0),
                gap: Some(16.0),
                anchor: Some(Anchor::TopLeft),
            }),
            groups,
            ..Area::default()
        }
    }

    fn desktop_layout(layout: &mut Layout) {
        let desktop = &mut layout.outputs[0].layers.desktop;
        desktop.areas.push(grid(
            "widgets",
            0.0,
            vec![cell("a", 0, "desk-clock"), cell("b", 3, "desk-clock-2")],
        ));
        desktop.areas.push(grid("more", 0.5, Vec::new()));
    }

    fn desktop_placed() -> Vec<(Node, Rect)> {
        let desktop = LayerKind::Desktop;
        let widgets = area(desktop, "widgets");
        vec![
            (widgets.clone(), Rect::new(0.0, 108.0, 960.0, 972.0)),
            (area(desktop, "more"), Rect::new(960.0, 108.0, 960.0, 972.0)),
            (
                widgets.group(&GroupId::new("a")),
                Rect::new(0.0, 108.0, 80.0, 80.0),
            ),
            (
                widgets.group(&GroupId::new("b")),
                Rect::new(288.0, 108.0, 80.0, 80.0),
            ),
            (
                instance(desktop, "widgets", "a", "desk-clock"),
                Rect::new(0.0, 108.0, 80.0, 80.0),
            ),
            (
                instance(desktop, "widgets", "b", "desk-clock-2"),
                Rect::new(288.0, 108.0, 80.0, 80.0),
            ),
        ]
    }

    fn widgets() -> ResolvedArea {
        shown(LayerKind::Desktop, "widgets").expect("the widget grid")
    }

    fn first_cell() -> String {
        widgets()
            .groups
            .iter()
            .find(|group| group.id.as_str() == "a")
            .map(|group| format!("{:?}", group.kind))
            .unwrap_or_default()
    }

    fn first_size() -> String {
        widgets()
            .groups
            .iter()
            .flat_map(|group| group.children.iter())
            .find(|child| child.id.as_str() == "desk-clock")
            .map(|child| format!("{:?}", child.representation))
            .unwrap_or_default()
    }

    #[test]
    fn desktop_mode_answers_every_key() {
        let desktop = LayerKind::Desktop;
        let clock = instance(desktop, "widgets", "a", "desk-clock");
        check(Case {
            test: "keys-desktop",
            layer: desktop,
            layout: desktop_layout,
            placed: desktop_placed,
            walk: (
                clock.clone(),
                Direction::Right,
                instance(desktop, "widgets", "b", "desk-clock-2"),
            ),
            tabbed: vec![area(desktop, "widgets"), area(desktop, "more")],
            customized: clock.clone(),
            removed: instance(desktop, "widgets", "b", "desk-clock-2"),
            moved: Stepped {
                node: clock.clone(),
                direction: Direction::Right,
                seen: first_cell,
            },
            resized: Stepped {
                node: area(desktop, "widgets").group(&GroupId::new("a")),
                direction: Direction::Right,
                seen: first_cell,
            },
        });
    }

    /// Ctrl+arrows step a widget through the sizes its module draws, and no further.
    #[test]
    fn a_widget_steps_through_its_sizes() {
        let _rig = rig_with("keys-sizes", desktop_layout);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        place(&desktop_placed());
        let clock = instance(LayerKind::Desktop, "widgets", "a", "desk-clock");
        assert!(session::select(Selection::of(clock)));
        let mut sizes = vec![first_size()];
        for direction in [
            Direction::Right,
            Direction::Right,
            Direction::Left,
            Direction::Left,
            Direction::Left,
        ] {
            assert!(tap(arrow(direction), CTRL));
            sizes.push(first_size());
        }
        assert_eq!(
            sizes,
            [
                "WidgetM", "WidgetL", "WidgetL", "WidgetM", "WidgetS", "WidgetS"
            ]
        );
        mode::leave();
    }

    fn overlay_layout(layout: &mut Layout) {
        layout.outputs[0].layers.overlay.areas.push(Area {
            id: AreaId::new("critical"),
            kind: Some(AreaKind::Stack {
                anchor: Some(Anchor::Center),
                width: Some(480.0),
                output_policy: None,
                routes: Vec::new(),
            }),
            ..Area::default()
        });
    }

    fn overlay_placed() -> Vec<(Node, Rect)> {
        vec![
            (
                area(LayerKind::Overlay, "stack"),
                Rect::new(1540.0, 0.0, 380.0, 400.0),
            ),
            (
                area(LayerKind::Overlay, "critical"),
                Rect::new(720.0, 340.0, 480.0, 400.0),
            ),
        ]
    }

    fn stack() -> ResolvedAreaKind {
        shown(LayerKind::Overlay, "stack")
            .expect("the corner stack")
            .kind
    }

    fn stack_anchor() -> String {
        match stack() {
            ResolvedAreaKind::Stack { anchor, .. } => format!("{anchor:?}"),
            _ => String::new(),
        }
    }

    fn stack_width() -> String {
        match stack() {
            ResolvedAreaKind::Stack { width, .. } => width.to_string(),
            _ => String::new(),
        }
    }

    #[test]
    fn overlay_mode_answers_every_key() {
        let overlay = LayerKind::Overlay;
        check(Case {
            test: "keys-overlay",
            layer: overlay,
            layout: overlay_layout,
            placed: overlay_placed,
            walk: (
                area(overlay, "stack"),
                Direction::Left,
                area(overlay, "critical"),
            ),
            tabbed: vec![area(overlay, "stack"), area(overlay, "critical")],
            customized: area(overlay, "critical"),
            removed: area(overlay, "critical"),
            moved: Stepped {
                node: area(overlay, "stack"),
                direction: Direction::Left,
                seen: stack_anchor,
            },
            resized: Stepped {
                node: area(overlay, "stack"),
                direction: Direction::Right,
                seen: stack_width,
            },
        });
    }

    /// At the end of its bar a chip goes on to the nearest bar of its kind that way, and across the bar it goes there at once.
    #[test]
    fn a_chip_moves_on_to_the_next_bar() {
        let _rig = rig_with("keys-across", top_layout);
        let _scope = Owner::new();
        let _host = enter(LayerKind::Top);
        place(&top_placed());
        let clock = instance(LayerKind::Top, "bar-top", "center", "clock");
        assert!(session::select(Selection::of(clock)));
        assert!(tap(arrow(Direction::Down), SHIFT));
        let bottom = shown(LayerKind::Top, "bar-bottom").expect("the bottom bar");
        let ids: Vec<&str> = bottom.groups[0]
            .children
            .iter()
            .map(|child| child.id.as_str())
            .collect();
        assert_eq!(ids, ["clock-2", "clock"], "into the same zone, at its end");
        assert_eq!(
            selected(),
            Some(instance(LayerKind::Top, "bar-bottom", "center", "clock")),
            "the selection follows it"
        );
        mode::leave();
    }

    /// TA-8: the prompt can be moved and restyled, never removed, by any path.
    #[test]
    fn delete_never_takes_the_lock_prompt_away() {
        let rig = rig_with("keys-prompt", |_| {});
        let _scope = Owner::new();
        let _host = enter(LayerKind::Lock);
        let prompt = area(LayerKind::Lock, "prompt");
        assert!(session::select(Selection::of(prompt.clone())));
        assert!(tap(named(NamedKey::Delete), NONE));
        assert!(is_shown(&prompt));
        assert_eq!(rig.undo_label(), None);
        mode::leave();
    }

    /// The switcher key opens the pie, an arrow switches to the mode lying that way, and the same key folds it again.
    #[test]
    fn the_pie_opens_from_the_keyboard_and_an_arrow_picks_a_mode() {
        let _rig = rig_with("keys-pie", |_| {});
        let _scope = Owner::new();
        let _host = enter(LayerKind::Top);
        assert!(tap(Key::Char('m'), NONE));
        assert!(pie::shown().peek());
        assert!(tap(Key::Char('m'), NONE));
        assert!(!pie::shown().peek(), "the same key folds it");

        assert!(tap(Key::Char('m'), NONE));
        assert!(tap(arrow(Direction::Left), NONE));
        let expected = pie::toward(LayerKind::Top, Direction::Left);
        assert_eq!(mode::current().map(|mode| mode.layer), expected);
        assert!(!pie::shown().peek());
        mode::leave();
    }

    /// `?` shows the key list the strip draws from the table, and Esc folds it before it reaches the selection or the mode.
    #[test]
    fn the_key_list_shows_and_folds() {
        let _rig = rig_with("keys-help", |_| {});
        let _scope = Owner::new();
        let _host = enter(LayerKind::Top);
        assert!(tap(Key::Char('?'), SHIFT));
        assert!(keys::help().peek());
        let lines = keys::help_rows(LayerKind::Top);
        assert!(
            lines
                .iter()
                .any(|line| line.keys == "Ctrl+Z" && line.what == "Undo"),
            "{lines:?}"
        );
        tap(named(NamedKey::Escape), NONE);
        assert!(!keys::help().peek());
        assert!(mode::current().is_some());
        mode::leave();
    }

    thread_local! {
        static RAN: Cell<usize> = const { Cell::new(0) };
    }

    /// A tool's key answers for its area kind — before a generic row on the same key — is listed with it, and does nothing where nothing of its kind is selected.
    #[test]
    fn a_tools_key_answers_for_its_kind() {
        let _rig = rig_with("keys-tool", top_layout);
        let _scope = Owner::new();
        keys::add_key_op(
            "bar",
            KeyOp {
                name: "probe",
                keys: vec![Chord::char('x'), Chord::named(NamedKey::Enter)],
                label: || "Probe".to_string(),
                run: Run::Act(|_| {
                    RAN.with(|ran| ran.set(ran.get() + 1));
                    Ok(())
                }),
            },
        );
        let _host = enter(LayerKind::Top);
        place(&top_placed());
        let row = keys::table(LayerKind::Top)
            .into_iter()
            .find(|row| row.name == "probe")
            .expect("the tool's row is in the table");
        assert_eq!(row.scope, Scope::Kind("bar"));
        assert!(
            keys::help_rows(LayerKind::Top)
                .iter()
                .any(|line| line.what == "Probe")
        );

        assert!(!tap(Key::Char('x'), NONE), "nothing selected");
        assert!(session::select(Selection::of(area(
            LayerKind::Top,
            "bar-top"
        ))));
        assert!(tap(Key::Char('x'), NONE));
        assert!(tap(named(NamedKey::Enter), NONE));
        assert_eq!(RAN.with(Cell::get), 2);
        assert_eq!(
            popover::current(),
            None,
            "the tool's Enter won over Customize"
        );
        mode::leave();
    }

    /// Every operation TA-4 and TA-5 name for a mode is a row of its key table, or waits on the task that builds it. As a task lands and registers its keys, its operations come off this list, and this test fails until they do.
    #[test]
    fn every_ta5_operation_has_a_key_or_waits_on_its_task() {
        const TA4: &[&str] = &[
            "select",
            "cycle-areas",
            "customize",
            "context-menu",
            "remove",
            "move",
            "resize",
            "undo",
            "redo",
            "deselect",
            "exit",
            "switch-mode",
        ];
        const TA5: &[(LayerKind, &[&str])] = &[
            (
                LayerKind::Background,
                &[
                    "region-split",
                    "region-join",
                    "region-move",
                    "region-resize",
                    "region-source",
                    "region-fit",
                    "region-transition",
                    "region-workspace-variant",
                    "texture-tile",
                    "texture-nine-slice",
                    "texture-blend",
                    "texture-opacity",
                    "texture-gradient",
                ],
            ),
            (
                LayerKind::Desktop,
                &[
                    "widget-cell",
                    "widget-add",
                    "widget-size",
                    "widget-stack",
                    "widget-from-bar",
                    "widget-visibility",
                    "desktop-workspace-variant",
                ],
            ),
            (
                LayerKind::Top,
                &[
                    "bar-create",
                    "bar-edge",
                    "bar-thickness",
                    "bar-length",
                    "bar-offset",
                    "bar-split",
                    "bar-join",
                    "chip-zone",
                    "chip-bar",
                    "chip-detach",
                    "bar-shape",
                    "bar-reserve",
                    "bar-autohide",
                    "bar-above-fullscreen",
                    "bar-output",
                ],
            ),
            (
                LayerKind::Overlay,
                &[
                    "stack-anchor",
                    "stack-offset",
                    "stack-width",
                    "stack-routes",
                    "stack-output-policy",
                    "stack-add",
                    "osd-placement",
                    "launcher-placement",
                ],
            ),
            (
                LayerKind::Lock,
                &[
                    "lock-regions",
                    "lock-grid",
                    "lock-palette",
                    "prompt-move",
                    "prompt-style",
                    "lock-privacy",
                ],
            ),
        ];
        const PENDING: &[(&str, &str)] = &[
            ("region-split", "T-7.1"),
            ("region-join", "T-7.1"),
            ("region-workspace-variant", "T-7.1"),
            ("texture-nine-slice", "T-7.1"),
            ("texture-gradient", "T-7.1"),
            ("widget-add", "T-7.2"),
            ("widget-stack", "T-7.2"),
            ("desktop-workspace-variant", "T-7.2"),
            ("widget-visibility", "T-8.3"),
            ("bar-create", "T-7.3"),
            ("bar-split", "T-7.3"),
            ("bar-join", "T-7.3"),
            ("chip-detach", "T-7.3"),
            ("bar-output", "T-7.3"),
            ("stack-offset", "T-7.4"),
            ("stack-routes", "T-7.4"),
            ("stack-add", "T-7.4"),
            ("osd-placement", "T-7.4"),
            ("launcher-placement", "T-7.4"),
            ("lock-regions", "T-7.5"),
            ("lock-grid", "T-7.5"),
            ("lock-palette", "T-7.5"),
            ("prompt-move", "T-7.5"),
            ("prompt-style", "T-7.5"),
            ("lock-privacy", "T-7.5"),
        ];
        telar::set_locale("en");
        for (layer, operations) in TA5 {
            let table = keys::table(*layer);
            for operation in TA4.iter().chain(operations.iter()) {
                let row = table.iter().find(|row| row.covers.contains(operation));
                let pending = PENDING.iter().find(|(name, _)| name == operation);
                match (row, pending) {
                    (Some(_), None) | (None, Some(_)) => {}
                    (Some(row), Some((_, task))) => panic!(
                        "{layer}: `{operation}` has a key now ({}), so take it off the list waiting on {task}",
                        row.name
                    ),
                    (None, None) => {
                        panic!("{layer}: `{operation}` has no key and waits on no task")
                    }
                }
            }
        }
        for (operation, task) in PENDING {
            assert!(
                TA5.iter()
                    .any(|(_, operations)| operations.contains(operation)),
                "`{operation}` waits on {task} but is no TA-5 operation"
            );
        }
    }
}
