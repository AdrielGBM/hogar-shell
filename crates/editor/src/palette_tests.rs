//! The palette in every mode that has one, through real pointer events and real keys: an entry dragged between two chips of a bar, onto the cells of an open panel, into a container at the slot under the pointer and onto a loose widget's middle; the size chosen on an entry being the one put; and the keyboard's path — each one undo entry.

#[cfg(test)]
mod tests {
    use telar::{Key, NamedKey, signal};

    use layout::{
        Area, AreaId, AreaKind, Arrange, ChildCell, Group, GroupKind, Instance, InstanceId,
        LayerKind, Layout, Rect, Representation, Zone,
    };
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::WidgetSize;

    use crate::mode;
    use crate::modes::gesture;
    use crate::modes::grid::Cells;
    use crate::modes::palette::{self, Line, Pick};
    use crate::modes::widgets;
    use crate::rig::{
        NONE, Page, Rig, SCREEN, cell_group, centre, close_within, draw, enter, face, frame,
        rig_with, stored, tap, undoes_to, undraw, widget,
    };
    use crate::session::{self, Selection};

    const fn module(
        id: &'static str,
        name: &'static str,
        category: Category,
        chip: bool,
        sizes: &'static [WidgetSize],
    ) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name,
            icon: "circle",
            category,
            options: &[],
            representations: Representations {
                chip: match chip {
                    true => Some(ChipDef::new(face, Input::ReadOnly)),
                    false => None,
                },
                widget: match sizes.is_empty() {
                    true => None,
                    false => Some(WidgetDef {
                        sizes,
                        build: face,
                        input: Input::ReadOnly,
                    }),
                },
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    static PROBES: &[ModuleDescriptor] = &[
        module("clock", "Clock", Category::Time, true, &WidgetSize::ALL),
        module(
            "weather",
            "Weather",
            Category::Info,
            false,
            &[WidgetSize::S, WidgetSize::M],
        ),
        module("notes", "Notes", Category::Info, true, &[]),
        module("workspaces", "Workspaces", Category::Windows, true, &[]),
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
            undraw();
            palette::unpick();
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    /// The desktop grid holding a row, a grid and a free container, and one loose weather widget; nothing else on the desktop.
    fn three_containers(layout: &mut Layout) {
        let areas = &mut layout.outputs[0].layers.desktop.areas;
        areas.retain(|area| area.id.as_str() == "widgets");
        areas[0].groups = vec![
            Group {
                arrange: Some(Arrange::Row),
                children: vec![widget("row-a", "clock"), widget("row-b", "clock")],
                ..cell_group("shelf", (0, 0, 4, 2))
            },
            Group {
                arrange: Some(Arrange::Grid),
                children: vec![Instance {
                    cell: Some(ChildCell::at(0, 0)),
                    ..widget("board-a", "clock")
                }],
                ..cell_group("board", (0, 3, 4, 2))
            },
            Group {
                arrange: Some(Arrange::Free),
                children: vec![Instance {
                    rect: Some(Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 0.4,
                        h: 0.4,
                    }),
                    ..widget("pad-a", "clock")
                }],
                ..cell_group("pad", (5, 0, 4, 4))
            },
            Group {
                children: vec![Instance {
                    representation: Some(Representation::WidgetS),
                    ..widget("weather", "weather")
                }],
                ..cell_group("weather", (10, 5, 2, 2))
            },
        ];
    }

    fn bar_top() -> AreaId {
        AreaId::new("bar-top")
    }

    fn top_bar(layout: &mut Layout) -> &mut Area {
        layout.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id == bar_top())
            .expect("the shipped top bar")
    }

    /// The top bar's notes moved into its start zone, after the workspaces.
    fn two_chips_at_the_start(layout: &mut Layout) {
        let bar = top_bar(layout);
        let end = bar
            .groups
            .iter()
            .position(|group| group.id.as_str() == "end")
            .expect("the end zone");
        let notes = bar.groups.remove(end).children;
        bar.groups
            .iter_mut()
            .find(|group| group.id.as_str() == "start")
            .expect("the start zone")
            .children
            .extend(notes);
    }

    fn panel_node() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("clock-panel"))
    }

    /// The top bar's clock given a panel eight cells by six.
    fn clock_panel(layout: &mut Layout) {
        layout.outputs[0].layers.top.areas.push(Area {
            id: AreaId::new("clock-panel"),
            kind: Some(AreaKind::Panel {
                owner: Some(InstanceId::new("clock")),
                along: None,
                cols: Some(8),
                rows: Some(6),
                cell: None,
                gap: None,
            }),
            ..Area::default()
        });
    }

    /// A grid on the overlay layer too, as the desktop's is.
    fn overlay_grid(layout: &mut Layout) {
        let mut grid = layout.outputs[0].layers.desktop.areas[0].clone();
        grid.id = AreaId::new("overlay-widgets");
        grid.groups.clear();
        layout.outputs[0].layers.overlay.areas.push(grid);
    }

    fn zone(id: &AreaId, zone: Zone) -> Vec<String> {
        crate::rig::desktop()
            .resolved
            .area(LayerKind::Top, id)
            .map(|area| {
                area.groups
                    .iter()
                    .filter(|group| group.kind == GroupKind::Zone { zone })
                    .flat_map(|group| group.children.iter().map(|child| child.id.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn desktop_group(rig: &Rig, id: &str) -> Group {
        stored(rig).outputs[0].layers.desktop.areas[0]
            .groups
            .iter()
            .find(|group| group.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("the group {id} is written"))
    }

    fn children(rig: &Rig, group: &str) -> Vec<String> {
        desktop_group(rig, group)
            .children
            .iter()
            .map(|child| child.id.to_string())
            .collect()
    }

    /// The palette of the mode up, open and drawn, narrowed by typing `typed`.
    fn palette_page(typed: &str) -> Page {
        let layer = mode::current().expect("the mode is up").layer;
        palette::open().expect("the palette opens");
        let mut page = Page::of(palette::tree(SCREEN, layer));
        for ch in typed.chars() {
            page.key(Key::Char(ch));
        }
        page
    }

    fn names(lines: Vec<Line>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| match line {
                Line::Heading(heading) => format!("# {heading}"),
                Line::Entry { name, sizes, .. } => {
                    let letters: String = sizes
                        .iter()
                        .map(|size| crate::modes::desktop::size_letter(*size))
                        .collect();
                    match letters.is_empty() {
                        true => name,
                        false => format!("{name} {letters}"),
                    }
                }
            })
            .collect()
    }

    /// Each mode lists what its layer takes: the top mode the modules that draw a chip, with the sizes each draws in an open panel; the overlay a new stack after the container; the desktop the widgets, each with its sizes.
    #[test]
    fn each_mode_lists_what_its_layer_takes_with_the_sizes_each_module_draws() {
        let _rig = rig_with("palette-lines", |_| {});
        let _owner = Owner::new();
        assert_eq!(
            names(palette::lines(LayerKind::Desktop, "")),
            [
                "Container",
                "# Time",
                "Clock SML",
                "# Information",
                "Weather SM"
            ]
        );
        assert_eq!(
            names(palette::lines(LayerKind::Top, "")),
            [
                "Group on a plate",
                "Container",
                "# Time",
                "Clock SML",
                "# Windows and workspaces",
                "Workspaces",
                "# Information",
                "Notes"
            ],
            "a bar takes chips, so the weather, which draws none, is not offered"
        );
        assert_eq!(
            names(palette::lines(LayerKind::Overlay, ""))[..2],
            ["Container", "Card stack"]
        );
        assert!(crate::host::add_of(LayerKind::Top).is_some(), "a `+`");
    }

    /// Dragged out of the palette in the top mode and let go between two chips of a bar, a module is put there as a chip, the line and the tag saying where on the way; one undo takes it back.
    #[test]
    fn an_entry_dropped_between_two_chips_of_a_bar_is_put_there_as_a_chip() {
        let rig = rig_with("palette-bar", two_chips_at_the_start);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let before = stored(&rig);
        assert_eq!(zone(&bar_top(), Zone::Start), ["workspaces", "notes"]);
        let drawn = |id: &str| {
            rects::instance(Some(SCREEN), &InstanceId::new(id))
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| panic!("{id} is drawn"))
        };
        let (left, right) = (drawn("workspaces"), drawn("notes"));
        let between = (
            (left.x + left.width + right.x) / 2.0,
            left.y + left.height / 2.0,
        );

        let mut page = palette_page("clock");
        let entry = page.at("Clock");
        let hint = page.drag(entry, between).expect("the drag shows a hint");
        assert_eq!(hint.tag.as_deref(), Some("Clock · At the start"));
        let start = zone(&bar_top(), Zone::Start);
        assert_eq!(start.len(), 3, "{start:?}");
        assert_eq!(
            (start[0].as_str(), start[2].as_str()),
            ("workspaces", "notes")
        );
        assert!(start[1].starts_with("clock"), "{start:?}");
        let added = crate::rig::desktop()
            .resolved
            .area(LayerKind::Top, &bar_top())
            .and_then(|bar| {
                bar.groups
                    .iter()
                    .flat_map(|group| group.children.iter())
                    .find(|child| child.id.as_str() == start[1])
                    .cloned()
            })
            .expect("the new chip");
        assert_eq!(added.representation, Representation::Chip);
        assert!(!transient::is_open(palette::ID), "letting go closes it");
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Add Clock"),
            "called as a press or Enter calls it"
        );
        draw(LayerKind::Top);
        assert_eq!(
            session::selected(),
            Selection::Instance(
                rects::instance(Some(SCREEN), &added.id)
                    .expect("the new chip is drawn")
                    .0
            ),
            "selected once it is drawn"
        );
        undoes_to(&rig, &before);
    }

    /// Picked by a press in the top mode, a chip is put on the bar where the next press lands, the line following the pointer until then.
    #[test]
    fn an_entry_picked_in_the_top_mode_is_put_where_the_bar_is_pressed() {
        let rig = rig_with("palette-bar-press", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let before = stored(&rig);
        let mut page = palette_page("work");
        let entry = page.at("Workspaces");
        page.click(entry);
        assert_eq!(
            palette::picked().peek(),
            Some(Pick::Module("workspaces".into(), None))
        );
        let mode = mode::current().expect("the mode is up");
        let mut tools = Page::of(crate::modes::bars::tool(&mode));
        let bar = rects::rect(&crate::rig::bar()).expect("the bar is drawn");
        let near_the_end = (bar.x + bar.width - 8.0, bar.y + bar.height / 2.0);
        tools.click(near_the_end);
        assert!(palette::picked().peek().is_none(), "the pick is spent");
        let end = zone(&bar_top(), Zone::End);
        assert_eq!(end.len(), 2, "{end:?}");
        assert_eq!(end[0], "notes");
        assert!(end[1].starts_with("workspaces"), "{end:?}");
        assert_eq!(rig.undo_label().as_deref(), Some("Add Workspaces"));
        undoes_to(&rig, &before);
    }

    /// Let go over an open panel, a widget is put on the panel's cells under it, at the size its entry chose.
    #[test]
    fn an_entry_dropped_into_an_open_panel_takes_the_cells_under_it() {
        let rig = rig_with("palette-panel", clock_panel);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let pitch = AreaKind::CELL + AreaKind::GAP;
        let drawn = telar::Rect::new(800.0, 120.0, 8.0 * pitch, 6.0 * pitch);
        rects::track_spanning(panel_node(), vec![signal(drawn)]);
        let before = stored(&rig);
        let geometry = widgets::drop_grids(SCREEN, LayerKind::Top)
            .into_iter()
            .map(|(geometry, _)| geometry)
            .find(|geometry| geometry.area.as_str() == "clock-panel")
            .expect("the open panel takes widgets");
        let target = geometry.rect_of(Cells {
            col: 2,
            row: 3,
            cols: 4,
            rows: 2,
        });

        let mut page = palette_page("clock");
        let entry = page.at("Clock");
        let hint = page
            .drag(entry, centre(target))
            .expect("the drag shows a hint");
        assert_eq!(hint.tag.as_deref(), Some("Clock · col 3 · row 4"));
        assert_eq!(hint.ghost, Some(target), "a copy of the cells it covers");
        let panel = stored(&rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id.as_str() == "clock-panel")
            .cloned()
            .expect("the panel is written");
        let group = panel.groups.first().expect("a group on the panel");
        assert!(
            matches!(group.kind, Some(GroupKind::Cell { col: 2, row: 3, .. })),
            "{:?}",
            group.kind
        );
        assert_eq!(group.children[0].module.as_deref(), Some("clock"));
        assert_eq!(
            group.children[0].representation,
            Some(Representation::WidgetM)
        );
        undoes_to(&rig, &before);
    }

    /// L chosen on an entry is the size a press picks and a drag puts, the tag and the copy under the pointer the cells it covers.
    #[test]
    fn the_size_chosen_on_an_entry_is_the_one_put() {
        let rig = rig_with("palette-size", three_containers);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let mut page = palette_page("clock");
        page.click(page.at("L"));
        page.click(page.at("Clock"));
        assert_eq!(
            palette::picked().peek(),
            Some(Pick::Module("clock".into(), Some(Representation::WidgetL))),
            "a press picks it at L"
        );
        palette::unpick();

        let geometry = widgets::grids(SCREEN, LayerKind::Desktop)[0].0.clone();
        let target = geometry.rect_of(Cells {
            col: 13,
            row: 0,
            cols: 4,
            rows: 4,
        });
        let mut page = palette_page("clock");
        page.click(page.at("L"));
        let hint = page
            .drag(page.at("Clock"), centre(target))
            .expect("the drag shows a hint");
        assert_eq!(hint.tag.as_deref(), Some("Clock · col 14 · row 1"));
        assert_eq!(hint.ghost, Some(target));
        let placed = stored(&rig).outputs[0].layers.desktop.areas[0]
            .groups
            .iter()
            .find(|group| {
                matches!(
                    group.kind,
                    Some(GroupKind::Cell {
                        col: 13,
                        row: 0,
                        ..
                    })
                )
            })
            .cloned()
            .expect("put where it was let go");
        assert_eq!(
            placed.children[0].representation,
            Some(Representation::WidgetL)
        );
        undoes_to(&rig, &before);
    }

    /// Let go anywhere over a container, a new widget joins it where the pointer is — a row's slot, a grid container's cell, a box centred on the pointer in a free one.
    #[test]
    fn an_entry_dropped_into_a_container_joins_it_at_the_slot_under_the_pointer() {
        let rig = rig_with("palette-into", three_containers);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);

        let shelf = frame("shelf");
        let mut page = palette_page("clock");
        let hint = page
            .drag(page.at("Clock"), centre(shelf.inner))
            .expect("the drag shows a hint");
        assert_eq!(hint.tag.as_deref(), Some("Clock · Into the container"));
        let row = children(&rig, "shelf");
        assert_eq!(
            (row.len(), row[0].as_str(), row[2].as_str()),
            (3, "row-a", "row-b")
        );
        assert!(row[1].starts_with("clock"), "{row:?}");
        undoes_to(&rig, &before);

        let board = frame("board");
        let lower_right = (
            board.inner.x + board.inner.width * 0.75,
            board.inner.y + board.inner.height * 0.75,
        );
        let mut page = palette_page("clock");
        page.drag(page.at("Clock"), lower_right);
        let joined = desktop_group(&rig, "board");
        assert_eq!(joined.children.len(), 2);
        assert_eq!(joined.children[1].cell, Some(ChildCell::at(1, 1)));
        undoes_to(&rig, &before);

        let pad = frame("pad");
        let at = (
            pad.inner.x + pad.inner.width * 0.5,
            pad.inner.y + pad.inner.height * 0.4,
        );
        let mut page = palette_page("clock");
        page.drag(page.at("Clock"), at);
        let joined = desktop_group(&rig, "pad");
        let placed = joined.children[1].rect.expect("a box of its own");
        assert!(
            close_within(placed.x, 0.25, 1e-3) && close_within(placed.y, 0.15, 1e-3),
            "{placed:?}"
        );
        undoes_to(&rig, &before);
    }

    /// Let go over the middle of a loose widget, a new one stacks with it into one Smart Stack on its cells.
    #[test]
    fn an_entry_dropped_on_a_loose_widgets_middle_makes_a_smart_stack() {
        let rig = rig_with("palette-stack", three_containers);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let geometry = widgets::grids(SCREEN, LayerKind::Desktop)[0].0.clone();
        let weather = geometry.rect_of(Cells {
            col: 10,
            row: 5,
            cols: 2,
            rows: 2,
        });
        let mut page = palette_page("clock");
        let hint = page
            .drag(page.at("Clock"), centre(weather))
            .expect("the drag shows a hint");
        assert_eq!(hint.tag.as_deref(), Some("Clock · Stack"));
        let stack = desktop_group(&rig, "weather");
        assert_eq!(stack.arrange, Some(Arrange::Pages));
        let stacked = children(&rig, "weather");
        assert_eq!(stacked.len(), 2);
        assert_eq!(stacked[0], "weather");
        assert!(stacked[1].starts_with("clock"), "{stacked:?}");
        assert!(
            matches!(
                stack.kind,
                Some(GroupKind::Cell {
                    col: 10,
                    row: 5,
                    ..
                })
            ),
            "on the cells it had"
        );
        undoes_to(&rig, &before);
    }

    /// The keyboard's path: `a` opens the palette, typing narrows it, → steps the size of the entry pointed at and Enter puts the first match near the selection at that size — on a grid; in the top mode, at the end of the zone of the selected chip.
    #[test]
    fn the_keys_put_the_first_match_at_the_size_stepped_to() {
        let rig = rig_with("palette-keys", three_containers);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);
        assert!(tap(Key::Char('a'), NONE));
        assert!(transient::is_open(palette::ID));
        let mut page = Page::of(palette::tree(SCREEN, LayerKind::Desktop));
        for ch in "clo".chars() {
            page.key(Key::Char(ch));
        }
        page.key(Key::Named(NamedKey::ArrowRight));
        page.key(Key::Named(NamedKey::Enter));
        assert!(!transient::is_open(palette::ID), "choosing closes it");
        let added: Vec<Group> = stored(&rig).outputs[0].layers.desktop.areas[0]
            .groups
            .iter()
            .filter(|group| {
                !before.outputs[0].layers.desktop.areas[0]
                    .groups
                    .contains(group)
            })
            .filter(|group| {
                group
                    .children
                    .iter()
                    .any(|child| child.module.as_deref() == Some("clock"))
            })
            .cloned()
            .collect();
        assert_eq!(added.len(), 1, "{added:?}");
        assert_eq!(
            added[0].children[0].representation,
            Some(Representation::WidgetL),
            "M stepped once to L"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Add Clock"));
        undoes_to(&rig, &before);
        mode::leave();

        let _top = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let before = stored(&rig);
        let clock = rects::instance(Some(SCREEN), &InstanceId::new("clock"))
            .map(|(node, _)| node)
            .expect("the bar's clock is drawn");
        assert!(session::select(Selection::Instance(clock)));
        assert!(
            tap(Key::Char('a'), NONE),
            "the top mode has the palette too"
        );
        let mut page = Page::of(palette::tree(SCREEN, LayerKind::Top));
        for ch in "work".chars() {
            page.key(Key::Char(ch));
        }
        page.key(Key::Named(NamedKey::Enter));
        let middle = zone(&bar_top(), Zone::Center);
        assert_eq!(middle.len(), 2, "{middle:?}");
        assert_eq!(middle[0], "clock");
        assert!(middle[1].starts_with("workspaces"), "{middle:?}");
        undoes_to(&rig, &before);
    }

    /// The overlay mode has no palette while the overlay has no grid, and says why.
    #[test]
    fn the_overlay_without_a_grid_has_no_palette() {
        let _rig = rig_with("palette-overlay-none", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        assert_eq!(
            palette::open().map_err(|why| why.to_string()),
            Err("The overlay has no grid to put a widget on: make one first".to_string())
        );
    }

    /// The overlay mode has the palette once the overlay has a grid, and a press on its stack entry makes a stack at once.
    #[test]
    fn the_overlay_has_the_palette_once_it_has_a_grid() {
        let rig = rig_with("palette-overlay", overlay_grid);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        let stacks = |rig: &Rig| {
            stored(rig).outputs[0]
                .layers
                .overlay
                .areas
                .iter()
                .filter(|area| matches!(area.kind, Some(AreaKind::Stack { .. })))
                .count()
        };
        let before = stacks(&rig);
        assert!(tap(Key::Char('a'), NONE));
        assert!(transient::is_open(palette::ID));
        let mut page = Page::of(palette::tree(SCREEN, LayerKind::Overlay));
        page.click(page.at("Card stack"));
        assert_eq!(stacks(&rig), before + 1, "a press makes the stack at once");
    }

    /// A chip carried off every bar says beside the pointer that letting go takes it off the layout.
    #[test]
    fn a_chip_carried_off_every_bar_says_so_at_the_pointer() {
        let _rig = rig_with("palette-taken-away", |_| {});
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let clock = rects::instance(Some(SCREEN), &InstanceId::new("clock"))
            .map(|(_, rect)| rect)
            .expect("the bar's clock is drawn");
        let mode = mode::current().expect("the mode is up");
        let mut tools = Page::of(crate::modes::bars::tool(&mode));
        let from = centre(clock);
        tools.move_to(from);
        tools.button(from, true);
        tools.move_to((from.0, from.1 + 8.0));
        tools.move_to((960.0, 600.0));
        let hint = gesture::hint().peek().expect("a hint");
        assert_eq!(
            hint.tag.as_deref(),
            Some("Let go to take it off the layout; undo brings it back")
        );
        tools.move_to(from);
        assert!(
            gesture::hint().peek().is_none_or(|hint| hint.tag.is_none()),
            "back over the bar it says nothing"
        );
        tools.button(from, false);
    }

    /// Esc while an entry is carried over the bar drops nothing: the layout is as it was and no entry is recorded.
    #[test]
    fn escape_while_an_entry_is_carried_puts_nothing_on_the_bar() {
        let rig = rig_with("palette-escape", two_chips_at_the_start);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Top);
        draw(LayerKind::Top);
        let before = stored(&rig);
        let chip = |id: &str| {
            rects::instance(Some(SCREEN), &InstanceId::new(id))
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| panic!("{id} is drawn"))
        };
        let (left, right) = (chip("workspaces"), chip("notes"));
        let between = (
            (left.x + left.width + right.x) / 2.0,
            left.y + left.height / 2.0,
        );

        let mut page = palette_page("clock");
        let entry = page.at("Clock");
        page.move_to(entry);
        page.button(entry, true);
        page.move_to((entry.0 + 8.0, entry.1 + 8.0));
        page.move_to(between);
        assert!(gesture::hint().peek().is_some(), "the drag shows where");
        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        page.button(between, false);

        assert_eq!(stored(&rig), before);
        assert_eq!(rig.undo_label(), None);
        assert_eq!(gesture::hint().peek(), None, "and shows nothing after");
        assert_eq!(zone(&bar_top(), Zone::Start), ["workspaces", "notes"]);
    }
}
