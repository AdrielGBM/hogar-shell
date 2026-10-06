//! The radius and padding tools through real pointer and key events: linked and unlinked writes, Alt isolating a corner, a corner squared off near its corner, arrows on a focused handle, padding on bars and containers, and a bar's padding moving its zones and nothing else.

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, PointerButton, PointerSource, Rect, compute_layout, signal,
    };

    use layout::{
        Area, AreaId, AreaKind, Arrange, Corners, GroupId, GroupKind, InstanceId, LayerKind,
        Layout, Representation, Sides,
    };

    use surfaces::menu::Pointed;

    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::WidgetSize;

    use crate::keys::{self};
    use crate::mode;
    use crate::popover::handles::{
        CORNERS, Corner, NEAREST, Side, handle_point, most_padding_in, most_radius_in,
    };
    use crate::rig::{
        Rig, SCREEN, bar, desktop, draw, enter, face, rig, rig_with, stored, tap, undraw,
    };
    use crate::session::{self, Selection};
    use crate::tools::{self, Tool};
    use crate::{host, popover};

    const SIZE: (f32, f32) = (1920.0, 1080.0);
    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    const fn module(id: &'static str, name: &'static str) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name,
            icon: "circle",
            category: Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(ChipDef::new(face, Input::ReadOnly)),
                widget: Some(WidgetDef {
                    sizes: &WidgetSize::ALL,
                    build: face,
                    input: Input::ReadOnly,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    static PROBES: &[ModuleDescriptor] = &[
        module("workspaces", "Workspaces"),
        module("clock", "Clock"),
        module("notes", "Notes"),
    ];

    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            tools::put_away();
            if !tools::linked_now() {
                tools::toggle_linked();
            }
            undraw();
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    /// The mode's tools as its host builds them, over the whole screen.
    struct Screen(ComponentList);

    impl Screen {
        fn new() -> Self {
            let current = mode::current().expect("the mode is up");
            let tools = vec![host::tools(&current).expect("the tools build")];
            let page = LayoutStyle::new().width(SIZE.0).height(SIZE.1);
            let root = Pointed::new(Box::new(Container::new(page, tools).expect("a page")));
            let node = root.layout_node();
            let tree = ComponentList::new(root);
            compute_layout(
                node,
                AvailableSpace::Definite(SIZE.0),
                AvailableSpace::Definite(SIZE.1),
            )
            .expect("the tools lay out");
            let mut screen = Self(tree);
            screen.settle();
            screen
        }

        fn send(&mut self, event: Event) {
            telar::observe_keyboard(&event);
            if !telar::dispatch_overlays(&event) {
                self.0.on_event(&event);
            }
            self.settle();
        }

        fn settle(&mut self) {
            for _ in 0..3 {
                telar::relayout_if_dirty();
            }
            let _ = self.0.commands();
        }

        fn press(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
            self.send(Event::PointerPressed {
                x: x.into(),
                y: y.into(),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }

        fn to(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
        }

        fn release(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerReleased {
                x: x.into(),
                y: y.into(),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }

        /// A press at `from`, a drag to `to` by way of a point halfway, and a release there.
        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            self.press(from);
            self.to(((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
            self.to(to);
            self.release(to);
        }

        fn alt(&mut self, held: bool) {
            self.send(Event::ModifiersChanged {
                modifiers: ModifiersState {
                    is_alt: held,
                    ..NONE
                },
            });
        }

        fn key(&mut self, named: NamedKey) {
            self.down(named.clone(), NONE);
            self.up(named, NONE);
        }

        /// A key going down, or repeated by the keyboard while it is held, with `modifiers`.
        fn down(&mut self, named: NamedKey, modifiers: ModifiersState) {
            self.send(Event::KeyPressed {
                key: Key::Named(named),
                modifiers,
            });
        }

        /// A key let go, and the release heard as the host's timer would hear it.
        fn up(&mut self, named: NamedKey, modifiers: ModifiersState) {
            self.send(Event::KeyReleased {
                key: Key::Named(named),
                modifiers,
            });
            keys::settle_released();
            self.settle();
        }

        /// How many times `wanted` is written on the screen.
        fn says(&self, wanted: &str) -> usize {
            let mut found = 0;
            telar::for_each_with_matrix(&self.0.commands(), |command, _| {
                if let DrawCommand::Text { text, .. } = command
                    && text.as_ref() == wanted
                {
                    found += 1;
                }
            });
            found
        }
    }

    fn written_bar(rig: &Rig) -> Area {
        stored(rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .cloned()
            .expect("the bar is written")
    }

    fn bar_radius(rig: &Rig) -> Option<Corners> {
        match written_bar(rig).kind {
            Some(AreaKind::Bar { shape, .. }) => shape.radius,
            _ => None,
        }
    }

    /// The four corners of the bar as the screen draws them.
    fn drawn_corners() -> [f32; 4] {
        tools::target::radius_of(&desktop(), &bar()).expect("the bar is drawn")
    }

    fn rect_of(node: &Node) -> Rect {
        rects::rect(node).expect("drawn")
    }

    /// Where a corner's handle is drawn for the radius it has now.
    fn corner_handle(node: &Node, corner: Corner, radius: f32) -> (f32, f32) {
        let rect = rect_of(node);
        let mut radii = tools::target::radius_of(&desktop(), node).expect("the node is drawn");
        radii[corner.index()] = radius;
        handle_point(rect, corner, radii, NEAREST.min(most_radius_in(rect)))
    }

    /// The top mode with the top bar drawn and selected and `tool` taken up for it with its key.
    fn bar_with(tool: Tool) -> Screen {
        draw(LayerKind::Top);
        assert!(session::select(Selection::Area(bar())));
        let key = match tool {
            Tool::Radius => 'r',
            Tool::Padding => 'i',
        };
        assert!(tap(Key::Char(key), NONE), "the tool's key answers");
        assert_eq!(tools::active(), Some(tool));
        Screen::new()
    }

    /// A linked drag rounds all four corners to one number, unlinking the four from the keyboard makes the next drag round its own corner alone and write all four, and each drag is one entry in the history.
    #[test]
    fn a_linked_radius_drag_writes_one_number_and_an_unlinked_one_four() {
        let rig = rig("tools-radius-linked");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Radius);
        let rect = rect_of(&bar());
        let before = drawn_corners();

        let start = corner_handle(&bar(), Corner::TopLeft, before[0]);
        screen.drag(start, Corner::TopLeft.point(rect, 16.0));
        let one = bar_radius(&rig).expect("the bar's corners are written");
        assert_eq!(one, Corners::all(16.0));
        assert!(one.is_uniform(), "linked, one number is written");
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Round the corners of bar-top")
        );
        assert_eq!(screen.says("16"), 1, "linked, one tag says the radius");

        assert!(tap(Key::Char('u'), NONE), "the link key answers");
        assert!(!tools::linked_now());
        screen.settle();
        assert_eq!(screen.says("16"), 4, "unlinked, every corner says its own");
        let (x, y) = corner_handle(&bar(), Corner::TopRight, 16.0);
        let (to_x, to_y) = Corner::TopRight.point(rect, 10.0);
        screen.drag((x, y), (to_x, to_y));
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::each(16.0, 10.0, 16.0, 16.0)),
            "unlinked, the dragged corner moves alone and all four are written"
        );
        assert_eq!(
            crate::session::undo().as_deref(),
            Ok("Round the corners of bar-top")
        );
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::all(16.0)),
            "one undo takes back one drag"
        );
    }

    /// On a bar too thin for the handles of its two ends to sit apart at a big radius, they slide apart along their edges so each is still taken hold of: pressed where it is drawn, each moves its own corner alone.
    #[test]
    fn the_handles_of_a_thin_bar_stay_apart_and_each_is_reachable() {
        let rig = rig("tools-radius-apart");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Radius);
        let rect = rect_of(&bar());
        let before = drawn_corners();
        let start = corner_handle(&bar(), Corner::TopLeft, before[0]);
        screen.drag(start, Corner::TopLeft.point(rect, 16.0));
        assert_eq!(bar_radius(&rig), Some(Corners::all(16.0)));
        assert!(tap(Key::Char('u'), NONE));
        screen.settle();

        let shown_from = NEAREST.min(most_radius_in(rect));
        let points = CORNERS.map(|corner| handle_point(rect, corner, [16.0; 4], shown_from));
        for (at, one) in points.iter().enumerate() {
            for other in &points[at + 1..] {
                let apart = (one.0 - other.0).hypot(one.1 - other.1);
                assert!(apart >= 12.0, "{one:?} and {other:?} are {apart} apart");
            }
        }

        for corner in [Corner::TopLeft, Corner::BottomLeft, Corner::BottomRight] {
            let mut radii = [16.0; 4];
            radii[corner.index()] = 12.0;
            let from = handle_point(rect, corner, [16.0; 4], shown_from);
            let to = handle_point(rect, corner, radii, shown_from);
            screen.drag(from, to);
            assert_eq!(
                bar_radius(&rig),
                Some(Corners::each(radii[0], radii[1], radii[2], radii[3])),
                "{corner:?} moves alone"
            );
            assert!(crate::session::undo().is_ok());
            assert_eq!(bar_radius(&rig), Some(Corners::all(16.0)));
            screen.settle();
        }
    }

    /// Alt held while the four are linked isolates the corner dragged, and Esc mid-drag puts all four back with nothing recorded.
    #[test]
    fn alt_isolates_one_corner_while_linked_and_escape_puts_all_four_back() {
        let rig = rig("tools-radius-alt");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Radius);
        let rect = rect_of(&bar());
        let before = drawn_corners();

        let shown_from = NEAREST.min(most_radius_in(rect));
        let linked = handle_point(rect, Corner::BottomRight, [15.0; 4], shown_from);
        let alone = handle_point(
            rect,
            Corner::BottomRight,
            [before[0], before[1], 15.0, before[3]],
            shown_from,
        );
        let start = corner_handle(&bar(), Corner::BottomRight, before[2]);
        screen.press(start);
        screen.to(linked);
        assert_eq!(drawn_corners(), [15.0; 4], "linked, all four follow live");
        screen.key(NamedKey::Escape);
        assert_eq!(drawn_corners(), before, "Esc puts all four back");
        screen.release(linked);
        assert_eq!(rig.undo_label(), None, "and nothing is recorded");

        screen.press(start);
        screen.alt(true);
        screen.to(alone);
        screen.release(alone);
        screen.alt(false);
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::each(before[0], before[1], 15.0, before[3])),
            "with Alt only the dragged corner moves"
        );
    }

    /// A corner let go within a few pixels of its corner is squared off rather than left barely rounded, and one pulled past half the short side stops there.
    #[test]
    fn a_corner_let_go_near_its_corner_is_squared_off_and_one_pulled_too_far_stops_at_half_the_short_side()
     {
        let rig = rig("tools-radius-square");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Radius);
        let rect = rect_of(&bar());
        let before = drawn_corners();

        let start = corner_handle(&bar(), Corner::TopLeft, before[0]);
        screen.drag(start, Corner::TopLeft.point(rect, 5.0));
        assert_eq!(bar_radius(&rig), Some(Corners::all(0.0)));

        let start = corner_handle(&bar(), Corner::TopLeft, 0.0);
        let far = Corner::TopLeft.point(rect, 400.0);
        screen.press(start);
        screen.to(far);
        let said = crate::modes::gesture::hint()
            .peek()
            .and_then(|hint| hint.tag);
        assert_eq!(
            said.as_deref(),
            Some("At most 17 px: half the short side"),
            "the pointer is told why the corner stops"
        );
        screen.release(far);
        assert_eq!(crate::modes::gesture::hint().peek(), None);
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::all(most_radius_in(rect))),
            "half the short side is as round as it gets"
        );
    }

    /// A press on a handle that does not move it records nothing; linked, an arrow on the handle it focused moves all four corners as a drag does, and with Alt only its own, a tenth of a step; each press is one entry in the history.
    #[test]
    fn arrows_on_a_focused_handle_move_all_four_while_linked_and_alt_isolates_its_own() {
        let rig = rig("tools-radius-keys");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Radius);
        let before = drawn_corners();

        let at = corner_handle(&bar(), Corner::BottomLeft, before[3]);
        screen.press(at);
        screen.release(at);
        assert_eq!(
            rig.undo_label(),
            None,
            "a press that moves nothing records nothing"
        );

        screen.key(NamedKey::ArrowRight);
        let stepped = before[3] + 1.0;
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::all(stepped)),
            "linked, the other three follow the focused corner"
        );
        assert_eq!(drawn_corners(), [stepped; 4]);

        let alt = ModifiersState {
            is_alt: true,
            ..NONE
        };
        screen.alt(true);
        screen.down(NamedKey::ArrowRight, alt);
        screen.up(NamedKey::ArrowRight, alt);
        screen.alt(false);
        let written = bar_radius(&rig).expect("the bar's corners are written");
        assert_eq!(
            [
                written.top_left(),
                written.top_right(),
                written.bottom_right()
            ],
            [stepped; 3],
            "with Alt the other three stay"
        );
        assert!(
            (written.bottom_left() - (stepped + 0.1)).abs() < 1e-4,
            "and the focused corner steps a tenth: {written:?}"
        );

        assert_eq!(
            crate::session::undo().as_deref(),
            Ok("Round the corners of bar-top")
        );
        assert_eq!(
            bar_radius(&rig),
            Some(Corners::all(stepped)),
            "each press is its own entry"
        );
        assert_eq!(
            crate::session::undo().as_deref(),
            Ok("Round the corners of bar-top")
        );
        assert_eq!(drawn_corners(), before);
    }

    /// An arrow held on a focused handle previews every step the keyboard repeats and is one entry in the history once it is let go, as every held key of the editor is; pressed again, it is another.
    #[test]
    fn a_held_arrow_on_a_focused_handle_is_one_undo_entry() {
        let rig = rig("tools-radius-held");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Padding);
        assert!(tap(Key::Char('u'), NONE), "the link key answers");
        screen.settle();
        let rect = rect_of(&bar());
        let before = tools::target::padding_of(&desktop(), &bar()).expect("the bar is drawn");
        let left = Side::Left.point(rect, before[3]);
        screen.press(left);
        screen.release(left);

        for _ in 0..3 {
            screen.down(NamedKey::ArrowRight, NONE);
        }
        let held = [before[0], before[1], before[2], before[3] + 3.0];
        assert_eq!(
            tools::target::padding_of(&desktop(), &bar()),
            Some(held),
            "every repeat is previewed"
        );
        assert_eq!(
            rig.undo_label(),
            None,
            "nothing is recorded while it is held"
        );
        screen.up(NamedKey::ArrowRight, NONE);
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(held[0], held[1], held[2], held[3]))
        );

        screen.key(NamedKey::ArrowLeft);
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(held[0], held[1], held[2], held[3] - 1.0))
        );
        assert_eq!(crate::session::undo().as_deref(), Ok("Pad bar-top"));
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(held[0], held[1], held[2], held[3])),
            "a press after the held arrow is let go is an entry of its own"
        );
        assert_eq!(crate::session::undo().as_deref(), Ok("Pad bar-top"));
        assert_eq!(
            tools::target::padding_of(&desktop(), &bar()),
            Some(before),
            "one undo takes back the whole hold"
        );
    }

    /// Two panels on the top layer: the clock's beside it, and the workspaces' along the bar.
    fn with_panels(layout: &mut Layout) {
        for (id, owner, along) in [
            ("clock-panel", "clock", false),
            ("workspaces-panel", "workspaces", true),
        ] {
            layout.outputs[0].layers.top.areas.push(Area {
                id: AreaId::new(id),
                kind: Some(AreaKind::Panel {
                    owner: Some(InstanceId::new(owner)),
                    along: Some(along),
                    cols: None,
                    rows: None,
                    cell: None,
                    gap: None,
                }),
                ..Area::default()
            });
        }
    }

    /// Where its style writes no radius, an owned panel's corners are read as it draws them — at the theme's radius beside its owner and square along a bar — which is what the tool's handles and the popover's radius row both start at, so neither jumps on its first change.
    #[test]
    fn an_owned_panel_is_read_at_the_radius_it_rests_at() {
        let _rig = rig_with("tools-panel-radius", with_panels);
        let _owner = Owner::new();
        let mut rounded = desktop();
        let mut config = (*rounded.config).clone();
        config.theme.radius = Some(14);
        rounded.config = Arc::new(config);
        let panel = |id: &str| Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new(id));

        assert_eq!(
            tools::target::radius_of(&rounded, &panel("clock-panel")),
            Some([14.0; 4]),
            "beside its owner, at the theme's radius"
        );
        assert_eq!(
            tools::target::radius_of(&rounded, &panel("workspaces-panel")),
            Some([0.0; 4]),
            "along its owner's bar, square"
        );
    }

    /// The tool goes away when the selection changes, and so does it when a popover opens over the selection.
    #[test]
    fn the_tool_is_put_away_by_another_selection_and_by_a_popover() {
        let _rig = rig("tools-put-away");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let _screen = bar_with(Tool::Radius);
        session::clear_selection();
        assert_eq!(tools::active(), None);

        assert!(session::select(Selection::Area(bar())));
        assert!(tap(Key::Char('r'), NONE));
        popover::open_area(bar()).expect("the bar's popover opens");
        assert_eq!(tools::active(), None);
        popover::close();
    }

    /// Linked, the padding tool writes one number into the bar's `style.padding`; unlinked, the dragged side moves alone and all four are written.
    #[test]
    fn the_padding_tool_writes_one_number_linked_and_four_unlinked() {
        let rig = rig("tools-padding");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Padding);
        let rect = rect_of(&bar());
        let before = tools::target::padding_of(&desktop(), &bar()).expect("the bar is drawn");

        screen.drag(Side::Top.point(rect, before[0]), Side::Top.point(rect, 9.0));
        assert_eq!(written_bar(&rig).style.padding, Some(Sides::all(9.0)));

        assert!(tap(Key::Char('u'), NONE));
        screen.settle();
        screen.drag(Side::Left.point(rect, 9.0), Side::Left.point(rect, 13.0));
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(9.0, 9.0, 9.0, 13.0))
        );

        screen.drag(
            Side::Bottom.point(rect, 9.0),
            Side::Bottom.point(rect, 30.0),
        );
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(9.0, 9.0, most_padding_in(rect), 13.0)),
            "no side pads past half the short side less the room left"
        );

        let right = Side::Right.point(rect, 9.0);
        screen.press(right);
        screen.release(right);
        screen.key(NamedKey::ArrowLeft);
        assert_eq!(
            written_bar(&rig).style.padding,
            Some(Sides::each(9.0, 8.0, most_padding_in(rect), 13.0)),
            "an arrow on a focused side moves that side alone"
        );
    }

    /// Padding is refused for what has none of its own — an instance — and leaves no tool up.
    #[test]
    fn an_instance_has_no_padding_tool() {
        let _rig = rig("tools-padding-refused");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let (chip, _) = rects::instance(Some(SCREEN), &InstanceId::new("clock")).expect("drawn");
        assert!(session::select(Selection::Instance(chip)));
        tap(Key::Char('i'), NONE);
        assert_eq!(tools::active(), None);
        assert!(mode::refusal().peek().is_some(), "the strip says why");
        assert!(tap(Key::Char('r'), NONE), "corners it has");
        assert_eq!(tools::active(), Some(Tool::Radius));
    }

    /// A bar's padding moves what is in its zones and nothing else: the bar keeps its strip and what it reserves, its chips at the far end stay put, and the only change written is the bar's own padding.
    #[test]
    fn a_bars_padding_relays_out_its_zones_only() {
        let rig = rig("tools-padding-zones");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = bar_with(Tool::Padding);
        let rect = rect_of(&bar());
        let chip = |id: &str| {
            rects::instance(Some(SCREEN), &InstanceId::new(id))
                .map(|(_, rect)| rect)
                .expect("drawn")
        };
        let (start, end) = (chip("workspaces"), chip("notes"));
        let reserved = desktop().reserved;
        let layout = stored(&rig);
        let before = tools::target::padding_of(&desktop(), &bar()).expect("the bar is drawn");
        let reconciles = rig.reconciles.get();

        assert!(tap(Key::Char('u'), NONE));
        screen.settle();
        screen.press(Side::Left.point(rect, before[3]));
        screen.to(Side::Left.point(rect, before[3] + 9.0));
        assert_eq!(
            rig.reconciles.get(),
            reconciles,
            "nothing re-tiles during the drag"
        );
        screen.release(Side::Left.point(rect, before[3] + 9.0));

        let mut put_back = stored(&rig);
        let written = &mut put_back.outputs[0].layers.top.areas[0];
        assert_eq!(written.id.as_str(), "bar-top");
        assert_eq!(
            written.style.padding,
            Some(Sides::each(
                before[0],
                before[1],
                before[2],
                before[3] + 9.0
            ))
        );
        written.style.padding = None;
        assert_eq!(put_back, layout, "the bar's padding is all that changed");
        assert_eq!(
            desktop().reserved,
            reserved,
            "what the bar reserves is as it was"
        );

        draw(LayerKind::Top);
        assert_eq!(rect_of(&bar()), rect, "the bar keeps its strip");
        assert_eq!(
            chip("workspaces").x,
            start.x + 9.0,
            "the start zone moves in"
        );
        let now = chip("notes");
        assert!(
            (now.x - end.x).abs() <= 1.0 && now.y == end.y && now.width == end.width,
            "the end zone stays where it was, give or take the pixel a zone rounds to: {now:?} against {end:?}"
        );
    }

    /// A row container on the desktop grid, holding one clock.
    fn with_container(layout: &mut Layout) {
        layout.outputs[0].layers.desktop.areas[0]
            .groups
            .push(layout::Group {
                id: GroupId::new("shelf"),
                kind: Some(GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 4,
                    row_span: 2,
                }),
                arrange: Some(Arrange::Row),
                children: vec![layout::Instance {
                    id: InstanceId::new("shelf-clock"),
                    module: Some("clock".to_string()),
                    representation: Some(Representation::WidgetM),
                    ..layout::Instance::default()
                }],
                ..layout::Group::default()
            });
    }

    fn grid() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    const BOX: Rect = Rect {
        x: 700.0,
        y: 300.0,
        width: 400.0,
        height: 200.0,
    };

    fn written_shelf(rig: &Rig) -> layout::Group {
        stored(rig).outputs[0].layers.desktop.areas[0]
            .groups
            .iter()
            .find(|group| group.id.as_str() == "shelf")
            .cloned()
            .expect("the container is written")
    }

    /// The radius tool rounds a container and the instance in it through their own styles, the padding tool pads the container, and neither touches the area they are in.
    #[test]
    fn groups_and_instances_are_rounded_and_containers_padded_through_their_own_style() {
        let rig = rig_with("tools-group", with_container);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let shelf = grid().group(&GroupId::new("shelf"));
        let clock = grid().instance(&GroupId::new("shelf"), &InstanceId::new("shelf-clock"));
        rects::track_spanning(shelf.clone(), vec![signal(BOX)]);
        rects::track_spanning(clock.clone(), vec![signal(BOX)]);
        let style = stored(&rig).outputs[0].layers.desktop.areas[0]
            .style
            .clone();

        assert!(session::select(Selection::Group(shelf.clone())));
        assert!(tap(Key::Char('r'), NONE));
        let mut screen = Screen::new();
        let drawn = tools::target::radius_of(&desktop(), &shelf).expect("the container is drawn");
        screen.drag(
            corner_handle(&shelf, Corner::TopLeft, drawn[0]),
            Corner::TopLeft.point(BOX, 30.0),
        );
        assert_eq!(written_shelf(&rig).style.radius, Some(Corners::all(30.0)));

        assert!(tap(Key::Char('i'), NONE));
        assert_eq!(
            tools::active(),
            Some(Tool::Padding),
            "a container has padding"
        );
        screen.settle();
        let drawn = tools::target::padding_of(&desktop(), &shelf).expect("the container is drawn");
        screen.drag(Side::Top.point(BOX, drawn[0]), Side::Top.point(BOX, 20.0));
        assert_eq!(written_shelf(&rig).style.padding, Some(Sides::all(20.0)));

        assert!(session::select(Selection::Instance(clock.clone())));
        assert!(tap(Key::Char('r'), NONE));
        screen.settle();
        let drawn = tools::target::radius_of(&desktop(), &clock).expect("the clock is drawn");
        screen.drag(
            corner_handle(&clock, Corner::BottomRight, drawn[2]),
            Corner::BottomRight.point(BOX, 24.0),
        );
        assert_eq!(
            written_shelf(&rig).children[0].style.radius,
            Some(Corners::all(24.0))
        );
        assert_eq!(
            stored(&rig).outputs[0].layers.desktop.areas[0].style,
            style,
            "the grid's own style is untouched"
        );
    }
}
