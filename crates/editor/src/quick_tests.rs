#[cfg(test)]
mod tests {
    use telar::{Event, Key, ModifiersState, NamedKey, Rect};

    use config::{Edge, Shape};
    use layout::{
        Area, AreaId, AreaKind, BarShape, Extent, GroupId, InstanceId, LayerKind, Layout,
    };
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::WidgetSize;

    use crate::host::Under;
    use crate::quick::{self, Button, Placing, place};
    use crate::rig::{
        self, NONE, Page, Rig, SCREEN, bar, draw, enter, face, rig, rig_with, stored, undraw,
    };
    use crate::session::{self, Selection};
    use crate::tools::{self, Tool};
    use crate::{host, keys, mode, popover};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

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
            quick::release();
            tools::put_away();
            if !tools::linked_now() {
                tools::toggle_linked();
            }
            popover::close();
            undraw();
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    struct Screen(Page);

    impl Screen {
        fn new() -> Self {
            let current = mode::current().expect("the mode is up");
            let mut screen = Self(Page::of(host::tree(&current, Under::Nothing)));
            screen.settle();
            screen
        }

        fn settle(&mut self) {
            rig::settle();
            let _ = self.0.0.commands();
        }

        fn press(&mut self, button: Button) {
            let at = quick::centre_of(button)
                .unwrap_or_else(|| panic!("the quick bar shows {button:?}: {:?}", shown()));
            self.0.click(at);
            self.settle();
        }

        fn tap(&mut self, key: Key, modifiers: ModifiersState) -> bool {
            let pressed = Event::KeyPressed {
                key: key.clone(),
                modifiers,
            };
            telar::observe_keyboard(&pressed);
            let taken = telar::dispatch_overlays(&pressed)
                || self.0.0.on_event(&pressed) == telar::EventResult::Handled;
            self.0.send(Event::KeyReleased { key, modifiers });
            keys::settle_released();
            self.settle();
            taken
        }

        fn key(&mut self, named: NamedKey) -> bool {
            self.tap(Key::Named(named), NONE)
        }

        fn dot(&mut self) -> bool {
            self.tap(Key::Char('.'), NONE)
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            self.0.move_to(from);
            self.0.send(rig::press_at(from));
            self.0
                .move_to(((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
            self.0.move_to(to);
            self.0.send(rig::release_at(to));
            self.settle();
        }
    }

    fn shown() -> Vec<Button> {
        quick::buttons(&session::selected())
    }

    fn clock() -> Node {
        bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))
    }

    fn clock_panel() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("clock-panel"))
    }

    fn top_areas(rig: &Rig) -> Vec<Area> {
        stored(rig).outputs[0].layers.top.areas.clone()
    }

    fn written_panel(rig: &Rig) -> Option<Area> {
        top_areas(rig)
            .into_iter()
            .find(|area| matches!(area.kind, Some(AreaKind::Panel { .. })))
    }

    fn selecting(layer: LayerKind, node: Node) -> Screen {
        draw(layer);
        assert!(session::select(Selection::of(node)));
        Screen::new()
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    fn placing(selection: Rect, size: (f32, f32), vertical: bool) -> Placing {
        Placing {
            selection,
            size,
            screen: Rect::new(0.0, 0.0, SIZE.0, SIZE.1),
            vertical,
            below: quick::CLEARANCE,
            inset: [0.0; 4],
        }
    }

    #[test]
    fn a_lying_bar_goes_above_where_it_fits_and_below_clear_of_the_size_tag_where_it_does_not() {
        let size = (200.0, 36.0);
        let lower = Rect::new(800.0, 500.0, 300.0, 120.0);
        let (x, y) = place(placing(lower, size, false));
        assert_eq!(y, lower.y - quick::CLEARANCE - size.1);
        assert_eq!(x + size.0 / 2.0, lower.x + lower.width / 2.0);

        let high = Rect::new(800.0, 10.0, 300.0, 120.0);
        let mut tagged = placing(high, size, false);
        tagged.below = quick::SIZE_TAG_ROOM;
        let (_, y) = place(tagged);
        assert_eq!(y, high.y + high.height + quick::SIZE_TAG_ROOM);
    }

    #[test]
    fn a_tall_selection_gets_an_upright_bar_beside_it_on_the_side_with_room() {
        let size = (36.0, 200.0);
        let left = Rect::new(0.0, 0.0, 34.0, 1080.0);
        let (x, _) = place(placing(left, size, true));
        assert_eq!(x, left.x + left.width + quick::CLEARANCE);
        let right = Rect::new(1886.0, 0.0, 34.0, 1080.0);
        let (x, _) = place(placing(right, size, true));
        assert_eq!(x, right.x - quick::CLEARANCE - size.0);
    }

    #[test]
    fn a_selection_filling_the_screen_holds_the_bar_inside_past_its_padding() {
        let whole = Rect::new(0.0, 0.0, SIZE.0, SIZE.1);
        let mut padded = placing(whole, (200.0, 36.0), false);
        padded.inset = [48.0; 4];
        assert_eq!(place(padded).1, 48.0 + quick::INSET);
        let mut upright = placing(whole, (36.0, 200.0), true);
        upright.inset = [48.0; 4];
        assert_eq!(place(upright).0, 48.0 + quick::INSET);
    }

    #[test]
    fn the_bar_never_covers_a_selection_that_leaves_it_room() {
        let size = (230.0, 36.0);
        for x in (0..1900).step_by(137) {
            for y in (0..1060).step_by(91) {
                for (width, height) in [(20.0, 20.0), (300.0, 120.0), (34.0, 600.0), (900.0, 500.0)]
                {
                    let selection = Rect::new(x as f32, y as f32, width, height);
                    let vertical = height > width * quick::TALL;
                    let size = match vertical {
                        true => (size.1, size.0),
                        false => size,
                    };
                    let (bx, by) = place(placing(selection, size, vertical));
                    let drawn = Rect::new(bx, by, size.0, size.1);
                    assert!(
                        !overlaps(drawn, selection),
                        "{drawn:?} covers {selection:?}"
                    );
                    assert!(
                        bx >= 0.0 && by >= 0.0 && bx + size.0 <= SIZE.0 && by + size.1 <= SIZE.1
                    );
                }
            }
        }
    }

    #[test]
    fn on_screen_the_bar_hangs_under_a_chip_clear_of_it_and_stands_beside_a_side_bar() {
        let _rig = rig_with("quick-placed", with_left_bar);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let _screen = selecting(LayerKind::Top, clock());
        let chip = rects::rect(&clock()).expect("the clock is drawn");
        let drawn = quick::drawn_box().expect("the quick bar is shown");
        assert!(!overlaps(drawn, chip), "{drawn:?} covers {chip:?}");
        assert!(
            drawn.y >= chip.y + chip.height,
            "below the chip on a top bar"
        );

        let side = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-left"));
        assert!(session::select(Selection::Area(side.clone())));
        let strip = rects::rect(&side).expect("the side bar is drawn");
        let drawn = quick::drawn_box().expect("the quick bar is shown");
        assert!(
            drawn.height > drawn.width,
            "upright beside a tall selection"
        );
        assert!(!overlaps(drawn, strip));
    }

    #[test]
    fn the_desktop_grid_filling_the_screen_holds_the_bar_inside_it() {
        let _rig = rig("quick-inside");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let grid = Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"));
        let _screen = selecting(LayerKind::Desktop, grid.clone());
        let area = rects::rect(&grid).expect("the grid is drawn");
        let drawn = quick::drawn_box().expect("the quick bar is shown");
        assert!(
            drawn.x >= area.x && drawn.y >= area.y + quick::INSET,
            "inside the grid, past its padding: {drawn:?} in {area:?}"
        );
    }

    fn with_left_bar(layout: &mut Layout) {
        layout.outputs[0].layers.top.areas.push(Area {
            id: AreaId::new("bar-left"),
            kind: Some(AreaKind::Bar {
                edge: Some(Edge::Left),
                thickness: Some(34.0),
                length: Some(Extent::Fill),
                offset: Some(0.0),
                shape: BarShape {
                    mode: Some(Shape::Bar),
                    gap: Some(0.0),
                    ..BarShape::default()
                },
                autohide: None,
            }),
            ..Area::default()
        });
    }

    #[test]
    fn customize_opens_the_selections_popover_and_the_bar_steps_aside_under_it() {
        let _rig = rig("quick-customize");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, clock());
        assert_eq!(shown().first(), Some(&Button::Customize));
        screen.press(Button::Customize);
        assert_eq!(popover::current(), Some(clock()));
        assert_eq!(quick::drawn_box(), None, "hidden under the popover");
        popover::close();
        assert!(quick::drawn_box().is_some(), "back once it closes");

        assert!(screen.dot(), "`.` reaches the quick bar");
        assert_eq!(quick::keyboard(), Some(0));
        assert!(screen.key(NamedKey::Enter));
        assert_eq!(popover::current(), Some(clock()), "Enter presses Customize");
        assert_eq!(
            quick::keyboard(),
            None,
            "and the keyboard goes to the popover"
        );
    }

    #[test]
    fn the_tool_buttons_take_their_tool_up_and_away_and_the_link_shows_while_one_is_up() {
        let _rig = rig("quick-tools");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, bar());
        assert!(!shown().contains(&Button::Link));
        screen.press(Button::Radius);
        assert_eq!(tools::active(), Some(Tool::Radius));
        assert!(
            shown().contains(&Button::Link),
            "the link while a tool is up"
        );
        screen.settle();
        screen.press(Button::Link);
        assert!(!tools::linked_now(), "pressed, the four move on their own");
        screen.press(Button::Link);
        assert!(tools::linked_now());
        screen.press(Button::Padding);
        assert_eq!(tools::active(), Some(Tool::Padding));
        screen.press(Button::Padding);
        assert_eq!(tools::active(), None);
        assert!(!shown().contains(&Button::Link));
    }

    #[test]
    fn the_arrows_walk_the_buttons_and_enter_presses_them_by_the_keyboard() {
        let _rig = rig("quick-keys");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, bar());
        let buttons = shown();
        let radius = buttons
            .iter()
            .position(|button| *button == Button::Radius)
            .expect("a bar's corners round");
        assert!(screen.dot());
        for _ in 0..radius {
            assert!(screen.key(NamedKey::ArrowRight));
        }
        assert_eq!(quick::keyboard(), Some(radius));
        assert_eq!(
            session::selected(),
            Selection::Area(bar()),
            "the arrows walk the bar rather than the selection"
        );
        assert!(screen.key(NamedKey::Enter));
        assert_eq!(tools::active(), Some(Tool::Radius));
        let link = shown()
            .iter()
            .position(|button| *button == Button::Link)
            .expect("the link is up");
        assert!(screen.key(NamedKey::End));
        assert!(screen.key(NamedKey::ArrowLeft));
        assert_eq!(quick::keyboard(), Some(link));
        assert!(screen.key(NamedKey::Space));
        assert!(!tools::linked_now());
        assert!(screen.key(NamedKey::Home));
        assert!(screen.key(NamedKey::ArrowLeft));
        assert_eq!(
            quick::keyboard(),
            Some(shown().len() - 1),
            "round from the first"
        );

        assert!(screen.key(NamedKey::Escape));
        assert_eq!(quick::keyboard(), None, "Esc hands the keyboard back");
        assert_eq!(
            session::selected(),
            Selection::Area(bar()),
            "and keeps the selection"
        );
        assert!(screen.tap(
            Key::Named(NamedKey::ArrowDown),
            ModifiersState {
                is_alt: true,
                ..NONE
            }
        ));
        assert_ne!(
            session::selected(),
            Selection::Area(bar()),
            "the keys select again"
        );
    }

    #[test]
    fn remove_takes_the_selection_away_as_one_undo_entry_and_is_absent_where_refused() {
        let rig = rig("quick-remove");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, clock());
        screen.press(Button::Remove);
        assert_eq!(rig.undo_label().as_deref(), Some("Remove Clock"));
        assert_eq!(session::undo().as_deref(), Ok("Remove Clock"));
        drop(screen);
        mode::leave();

        let _mode = enter(LayerKind::Lock);
        draw(LayerKind::Lock);
        let prompt = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("prompt"));
        assert!(session::select(Selection::Area(prompt)));
        let buttons = shown();
        assert!(!buttons.contains(&Button::Remove), "{buttons:?}");
        assert!(!buttons.contains(&Button::Panel), "{buttons:?}");
    }

    #[test]
    fn the_panel_button_gives_a_panel_as_one_undo_entry_selects_it_and_is_lit_after() {
        let rig = rig("quick-panel");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, clock());
        screen.press(Button::Panel);
        let panel = written_panel(&rig).expect("the clock's panel is written");
        assert_eq!(panel.id.as_str(), "clock-panel");
        assert_eq!(
            panel.kind,
            Some(AreaKind::Panel {
                owner: Some(InstanceId::new("clock")),
                along: None,
                cols: None,
                rows: None,
                cell: None,
                gap: None,
            })
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Give Clock a panel"));
        assert_eq!(session::selected(), Selection::Area(clock_panel()));
        assert!(
            surfaces::transient::is_open(
                &surfaces::panel::Owner::of(&clock())
                    .expect("an instance")
                    .id()
            ),
            "the panel opens"
        );

        transient::close(
            &surfaces::panel::Owner::of(&clock())
                .expect("an instance")
                .id(),
        );
        assert!(session::select(Selection::Instance(clock())));
        assert!(
            crate::panel::offered(&clock()).is_some_and(|offer| offer.owned.is_some()),
            "lit: it owns one now"
        );
        assert!(screen.dot());
        let at = shown()
            .iter()
            .position(|button| *button == Button::Panel)
            .expect("the panel button");
        for _ in 0..at {
            assert!(screen.key(NamedKey::ArrowRight));
        }
        assert!(screen.key(NamedKey::Enter));
        assert_eq!(
            session::selected(),
            Selection::Area(clock_panel()),
            "by the keyboard, it selects the panel it owns"
        );
        assert_eq!(top_areas(&rig).len(), 2, "and writes no second one");

        assert_eq!(session::undo().as_deref(), Ok("Give Clock a panel"));
        assert_eq!(written_panel(&rig), None, "one undo takes it back");
    }

    #[test]
    fn the_grip_moves_the_bar_for_its_own_selection_alone() {
        let _rig = rig("quick-grip");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, clock());
        let before = quick::drawn_box().expect("shown");
        let grip = quick::grip_centre().expect("a grip");
        screen.drag(grip, (grip.0 - 300.0, grip.1 + 200.0));
        let moved = quick::drawn_box().expect("shown");
        assert_eq!((moved.x, moved.y), (before.x - 300.0, before.y + 200.0));

        assert!(session::select(Selection::Area(bar())));
        let other = quick::drawn_box().expect("shown");
        assert!(session::select(Selection::Instance(clock())));
        assert_eq!(
            quick::drawn_box(),
            Some(moved),
            "the clock's bar stays where it was put"
        );
        assert_ne!((other.x, other.y), (moved.x, moved.y));
    }

    /// Where the grip put the bar lasts as long as the mode: leaving it and coming back finds the bar where it lies by default.
    #[test]
    fn the_grip_moves_the_bar_until_the_mode_changes() {
        let _rig = rig("quick-grip-mode");
        let _owner = Owner::new();
        let first = enter(LayerKind::Top);
        let mut screen = selecting(LayerKind::Top, clock());
        let before = quick::drawn_box().expect("shown");
        let grip = quick::grip_centre().expect("a grip");
        screen.drag(grip, (grip.0 - 300.0, grip.1 + 200.0));
        assert_ne!(quick::drawn_box(), Some(before), "the grip moved it");
        drop(screen);
        drop(first);
        mode::leave();

        let _again = enter(LayerKind::Top);
        let _screen = selecting(LayerKind::Top, clock());
        assert_eq!(quick::drawn_box(), Some(before));
    }

    /// Each button says the key that does what it does, as the key table of the mode up has it, and none where no mode is up.
    #[test]
    fn the_buttons_name_their_keys_from_the_key_table() {
        telar::set_locale("en");
        let _rig = rig("quick-hints");
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        assert_eq!(quick::label_of(Button::Customize), "Customize (Enter)");
        assert_eq!(quick::label_of(Button::Radius), "Round the corners (R)");
        assert_eq!(quick::label_of(Button::Padding), "Padding (I)");
        assert_eq!(quick::label_of(Button::Remove), "Remove (Delete)");
        mode::leave();
        assert_eq!(quick::label_of(Button::Remove), "Remove");
    }
}
