//! Containers in the edit modes, through real pointer events and real keys: made empty with `Shift+N` and from the palette, filled by dropping a widget anywhere over one at the slot under the pointer, their children reordered, moved between cells and carried in a free box that snaps unless Alt is held, taken out by dragging or from the menu, and resized by the end or corner handle — each one undo entry.

#[cfg(test)]
mod tests {
    use telar::{Key, NamedKey, signal};

    use layout::{
        Area, AreaId, AreaKind, Arrange, ChildCell, Group, GroupId, GroupKind, Instance,
        InstanceId, LayerKind, Layout, Rect, Representation, ResolvedGroup, Zone,
    };
    use surfaces::menu::Asked;
    use surfaces::rects::{self, Node};
    use ui::descriptor::ModuleDescriptor;
    use ui::host::WidgetSize;

    use crate::modes::container;
    use crate::modes::desktop::Landing;
    use crate::modes::palette::{self, Line, Pick};
    use crate::modes::widgets;
    use crate::rig::{
        NONE, Owner, Page, Rig, SCREEN, cell_group, centre, close_within, draw, enter, frame,
        hold_alt, module, rig_with, stored, tap, undoes_to, undraw, widget, with,
    };
    use crate::session::{self, Selection};
    use crate::{context, mode, popover, select};

    static PROBES: &[ModuleDescriptor] = &[
        module("clock", "Clock", &WidgetSize::ALL),
        module("weather", "Weather", &[WidgetSize::S, WidgetSize::M]),
    ];

    fn owner() -> Owner {
        Owner::installing(PROBES).tearing_down(|| {
            hold_alt(false);
            undraw();
        })
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The desktop grid holding a row, a grid and a free container, a column, and one loose weather widget; the clock face's own area taken away.
    fn four_containers(layout: &mut Layout) {
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
                children: vec![
                    Instance {
                        rect: Some(rect(0.0, 0.0, 0.4, 0.4)),
                        ..widget("pad-a", "clock")
                    },
                    Instance {
                        rect: Some(rect(0.5, 0.5, 0.3, 0.3)),
                        ..widget("pad-b", "clock")
                    },
                ],
                ..cell_group("pad", (5, 0, 4, 4))
            },
            Group {
                arrange: Some(Arrange::Column),
                children: vec![widget("col-a", "clock"), widget("col-b", "clock")],
                ..cell_group("col", (10, 0, 2, 4))
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

    fn widgets_area() -> AreaId {
        AreaId::new("widgets")
    }

    fn grid_node() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &widgets_area())
    }

    fn written_area(rig: &Rig) -> Area {
        stored(rig).outputs[0].layers.desktop.areas[0].clone()
    }

    fn written(rig: &Rig, group: &str) -> Group {
        written_area(rig)
            .groups
            .into_iter()
            .find(|held| held.id.as_str() == group)
            .unwrap_or_else(|| panic!("the group {group} is written"))
    }

    fn child_ids(rig: &Rig, group: &str) -> Vec<String> {
        written(rig, group)
            .children
            .iter()
            .map(|child| child.id.to_string())
            .collect()
    }

    fn child(rig: &Rig, group: &str, id: &str) -> Instance {
        written(rig, group)
            .children
            .into_iter()
            .find(|child| child.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is in {group}"))
    }

    fn shown(group: &str) -> ResolvedGroup {
        crate::rig::desktop()
            .resolved
            .area(LayerKind::Desktop, &widgets_area())
            .and_then(|area| {
                area.groups
                    .iter()
                    .find(|held| held.id.as_str() == group)
                    .cloned()
            })
            .unwrap_or_else(|| panic!("the group {group} is shown"))
    }

    fn holder_of(id: &str) -> Option<String> {
        crate::rig::desktop()
            .resolved
            .area(LayerKind::Desktop, &widgets_area())?
            .groups
            .iter()
            .find(|group| group.children.iter().any(|child| child.id.as_str() == id))
            .map(|group| group.id.to_string())
    }

    fn node_of(id: &str) -> Node {
        let group = holder_of(id).expect("the instance is on the grid");
        grid_node().instance(&GroupId::new(&group), &InstanceId::new(id))
    }

    fn drawn(id: &str) -> telar::Rect {
        rects::rect(&node_of(id)).unwrap_or_else(|| panic!("{id} is drawn"))
    }

    /// The selection tool and the desktop mode's tools over the whole screen, the desktop layer drawn under them.
    fn tools_page() -> Page {
        draw(LayerKind::Desktop);
        let current = mode::current().expect("the mode is up");
        Page::over(vec![
            select::tool(&current).expect("the selection builds"),
            widgets::tool(&current).expect("the desktop tools build"),
        ])
    }

    #[test]
    fn shift_n_makes_a_container_on_the_first_free_span_and_alt_n_a_grid() {
        let rig = rig_with("container-create", |layout| {
            layout.outputs[0]
                .layers
                .desktop
                .areas
                .retain(|area| area.id.as_str() == "widgets");
        });
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);

        assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
        let made = written(&rig, "container");
        assert_eq!(made.arrange, Some(Arrange::Row), "wider than tall");
        assert_eq!(
            made.kind,
            Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 6,
                row_span: 4
            }),
            "the first span of the list, on an empty grid at its first cell"
        );
        assert!(made.children.is_empty());
        assert_eq!(
            session::selected(),
            Selection::Group(grid_node().group(&GroupId::new("container")))
        );
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Make the container container")
        );

        assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
        let second = written(&rig, "container-2");
        assert!(
            matches!(
                second.kind,
                Some(GroupKind::Cell {
                    col_span: 6,
                    row_span: 4,
                    ..
                })
            ),
            "beside the first while there is room: {:?}",
            second.kind
        );
        undoes_to(&rig, &{
            let mut once = before.clone();
            once.outputs[0].layers.desktop.areas[0]
                .groups
                .push(made.clone());
            once
        });
        undoes_to(&rig, &before);

        let areas = stored(&rig).outputs[0].layers.desktop.areas.len();
        assert!(tap(Key::Char('n'), with(|held| held.is_alt = true)));
        assert_eq!(
            stored(&rig).outputs[0].layers.desktop.areas.len(),
            areas + 1,
            "Alt+N makes a grid"
        );
    }

    #[test]
    fn a_full_grid_takes_the_largest_span_that_still_fits_and_a_tall_one_is_a_column() {
        let rig = rig_with("container-span", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let grids = widgets::grids(SCREEN, LayerKind::Desktop);
        let (geometry, area) = &grids[0];
        let expected = container::free_span(area, geometry.room, (0, 0)).expect("room for one");
        assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
        let made = written(&rig, "container");
        assert_eq!(
            made.kind,
            Some(GroupKind::Cell {
                col: expected.col,
                row: expected.row,
                col_span: expected.cols,
                row_span: expected.rows,
            })
        );
        let wanted = match expected.cols > expected.rows {
            true => Arrange::Row,
            false => Arrange::Column,
        };
        assert_eq!(made.arrange, Some(wanted));
    }

    #[test]
    fn shift_n_in_the_top_mode_puts_a_plated_group_in_the_bars_zone() {
        let rig = rig_with("container-top", |_| {});
        let _owner = owner();
        let _host = enter(LayerKind::Top);
        assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
        let bar = stored(&rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .cloned()
            .expect("the top bar");
        let made = bar
            .groups
            .iter()
            .find(|group| group.id.as_str() == "plate")
            .expect("a group of its own");
        assert_eq!(made.kind, Some(GroupKind::Zone { zone: Zone::Start }));
        assert_eq!(
            made.style.fill.as_deref(),
            Some("overlay"),
            "drawn on a plate"
        );
        assert_eq!(made.arrange, None, "a zone lays its own run out");
    }

    #[test]
    fn shift_n_on_the_lock_puts_a_container_on_its_grid() {
        let rig = rig_with("container-lock", |_| {});
        let _owner = owner();
        let _host = enter(LayerKind::Lock);
        assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
        let lock = &stored(&rig).outputs[0].layers.lock.areas;
        assert!(
            lock.iter()
                .find(|area| area.id.as_str() == "lock-readings")
                .is_some_and(|area| area
                    .groups
                    .iter()
                    .any(|group| group.id.as_str() == "container" && group.arrange.is_some())),
            "{lock:?}"
        );
    }

    #[test]
    fn the_palette_offers_a_container_first_and_enter_makes_one() {
        let rig = rig_with("container-palette", |layout| {
            layout.outputs[0]
                .layers
                .desktop
                .areas
                .retain(|area| area.id.as_str() == "widgets");
        });
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let first = palette::lines(LayerKind::Desktop, "")
            .into_iter()
            .next()
            .expect("lines");
        assert!(
            matches!(first, Line::Entry { pick: Pick::Container, ref name, .. } if name == "Container")
        );
        assert!(
            palette::lines(LayerKind::Desktop, "wea")
                .iter()
                .all(|line| !matches!(
                    line,
                    Line::Entry {
                        pick: Pick::Container,
                        ..
                    }
                )),
            "narrowed away"
        );
        crate::modes::desktop::put(&Pick::Container, None, LayerKind::Desktop)
            .expect("the container is made");
        assert_eq!(written(&rig, "container").arrange, Some(Arrange::Row));
        assert!(matches!(session::selected(), Selection::Group(_)));
    }

    #[test]
    fn a_widget_dropped_over_a_row_joins_it_at_the_slot_under_the_pointer() {
        let rig = rig_with("container-adopt-row", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let before = stored(&rig);
        let shelf = frame("shelf");
        let between = (
            shelf.inner.x + shelf.inner.width / 2.0,
            shelf.inner.y + shelf.inner.height / 2.0,
        );
        let hint = screen.drag(centre(drawn("weather")), between);
        assert_eq!(
            hint.and_then(|hint| hint.tag).as_deref(),
            Some("Into the container")
        );
        assert_eq!(child_ids(&rig, "shelf"), ["row-a", "weather", "row-b"]);
        assert!(
            !written_area(&rig)
                .groups
                .iter()
                .any(|group| group.id.as_str() == "weather"),
            "its own group goes with it"
        );
        undoes_to(&rig, &before);
    }

    #[test]
    fn a_widget_dropped_over_a_grid_container_takes_the_cell_under_the_pointer() {
        let rig = rig_with("container-adopt-grid", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let board = frame("board");
        let lower_right = (
            board.inner.x + board.inner.width * 0.75,
            board.inner.y + board.inner.height * 0.75,
        );
        screen.drag(centre(drawn("weather")), lower_right);
        assert_eq!(
            child(&rig, "board", "weather").cell,
            Some(ChildCell::at(1, 1))
        );
    }

    #[test]
    fn a_widget_dropped_over_a_free_container_is_centred_on_the_pointer() {
        let rig = rig_with("container-adopt-free", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let pad = frame("pad");
        let at = (
            pad.inner.x + pad.inner.width * 0.5,
            pad.inner.y + pad.inner.height * 0.4,
        );
        screen.drag(centre(drawn("weather")), at);
        let placed = child(&rig, "pad", "weather")
            .rect
            .expect("a box of its own");
        assert!(
            close_within(placed.x, 0.25, 1e-3) && close_within(placed.y, 0.15, 1e-3),
            "{placed:?}"
        );
        assert!(
            close_within(placed.w, 0.5, 1e-3) && close_within(placed.h, 0.5, 1e-3),
            "{placed:?}"
        );
    }

    #[test]
    fn a_widget_dropped_over_a_column_joins_it_and_a_loose_widgets_middle_stacks() {
        let rig = rig_with("container-adopt-column", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let grids = widgets::grids(SCREEN, LayerKind::Desktop);
        let one = crate::modes::grid::Cells::ONE;
        let loose = grids[0].0.rect_of(
            crate::modes::grid::cells_of(&shown("weather")).expect("the loose widget's cells"),
        );
        let over_middle = widgets::landing_at(&grids, centre(loose), (0.0, 0.0), one, None)
            .expect("over the grid");
        assert!(matches!(over_middle.1, Landing::Onto(ref group) if group.as_str() == "weather"));
        assert_eq!(widgets::landing_tag(&over_middle.1), "Stack");
        let shelf = frame("shelf");
        let near_its_edge = (shelf.outer.x + 2.0, shelf.outer.y + 2.0);
        let over_edge = widgets::landing_at(&grids, near_its_edge, (0.0, 0.0), one, None)
            .expect("over the grid");
        assert!(
            matches!(over_edge.1, Landing::Into(ref group, _) if group.as_str() == "shelf"),
            "a container takes it anywhere over its cells: {:?}",
            over_edge.1
        );

        let col = frame("col");
        let top = (col.inner.x + 4.0, col.inner.y + 4.0);
        screen.drag(centre(drawn("weather")), top);
        assert_eq!(child_ids(&rig, "col"), ["weather", "col-a", "col-b"]);
    }

    #[test]
    fn a_child_dragged_inside_a_row_takes_another_place_in_its_order() {
        let rig = rig_with("container-reorder", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let before = stored(&rig);
        let shelf = frame("shelf");
        let right = (
            shelf.inner.x + shelf.inner.width * 0.9,
            shelf.inner.y + shelf.inner.height / 2.0,
        );
        screen.drag(centre(drawn("row-a")), right);
        assert_eq!(child_ids(&rig, "shelf"), ["row-b", "row-a"]);
        assert_eq!(rig.undo_label().as_deref(), Some("Move Clock"));
        undoes_to(&rig, &before);
    }

    #[test]
    fn a_child_dragged_inside_a_grid_container_moves_to_the_cell_under_the_pointer() {
        let rig = rig_with("container-cell", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let board = frame("board");
        let lower_right = (
            board.inner.x + board.inner.width * 0.8,
            board.inner.y + board.inner.height * 0.8,
        );
        screen.drag(centre(drawn("board-a")), lower_right);
        assert_eq!(
            child(&rig, "board", "board-a").cell,
            Some(ChildCell::at(1, 1))
        );
        assert_eq!(child_ids(&rig, "board"), ["board-a"]);
    }

    #[test]
    fn a_free_child_snaps_to_its_sibling_unless_alt_is_held() {
        let rig = rig_with("container-free-move", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let before = stored(&rig);
        let pad = frame("pad");
        let start = drawn("pad-a");
        let from = (start.x + 10.0, start.y + 10.0);
        let near_sibling = (pad.inner.x + pad.inner.width * 0.5 + 3.0 + 10.0, from.1);
        screen.drag(from, near_sibling);
        let snapped = child(&rig, "pad", "pad-a").rect.expect("a box");
        assert!(
            close_within(snapped.x, 0.5, 1e-3),
            "its start lands on the sibling's: {snapped:?}"
        );
        assert!(
            close_within(snapped.w, 0.4, 1e-3) && close_within(snapped.h, 0.4, 1e-3),
            "{snapped:?}"
        );
        undoes_to(&rig, &before);

        undraw();
        let mut screen = tools_page();
        hold_alt(true);
        screen.drag(from, near_sibling);
        hold_alt(false);
        let free = child(&rig, "pad", "pad-a").rect.expect("a box");
        assert!(
            (free.x - 0.5).abs() > 1e-3 && (free.x - 0.5).abs() < 0.02,
            "Alt leaves it a few pixels off: {free:?}"
        );
    }

    #[test]
    fn a_child_dragged_out_of_its_container_is_a_widget_of_its_own_at_its_smallest_size() {
        let rig = rig_with("container-drag-out", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let mut screen = tools_page();
        let before = stored(&rig);
        let grids = widgets::grids(SCREEN, LayerKind::Desktop);
        let (geometry, _) = &grids[0];
        let empty = geometry.rect_of(crate::modes::grid::Cells {
            col: 0,
            row: 7,
            cols: 1,
            rows: 1,
        });
        let grab = drawn("row-b");
        screen.drag((grab.x + 2.0, grab.y + 2.0), (empty.x + 2.0, empty.y + 2.0));
        assert_eq!(child_ids(&rig, "shelf"), ["row-a"]);
        let loose = written(&rig, "row-b");
        assert_eq!(loose.arrange, None);
        assert_eq!(
            loose.children[0].representation,
            Some(Representation::WidgetS)
        );
        assert!(
            matches!(loose.kind, Some(GroupKind::Cell { col: 0, row: 7, .. })),
            "{:?}",
            loose.kind
        );
        undoes_to(&rig, &before);
    }

    #[test]
    fn the_last_child_out_leaves_the_container_empty_rather_than_gone() {
        let rig = rig_with("container-empty", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        container::take_out(&node_of("board-a")).expect("taken out");
        assert!(written(&rig, "board").children.is_empty());
        assert_eq!(written(&rig, "board").arrange, Some(Arrange::Grid));
    }

    #[test]
    fn a_childs_menu_customizes_its_container_and_takes_it_out() {
        let rig = rig_with("container-menu", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let ask = |id: &str| {
            context::open(Asked {
                node: node_of(id),
                window: LayerKind::Overlay,
                at: None,
            })
            .expect("the menu opens");
        };
        ask("row-b");
        let rows = context::rows();
        assert!(
            rows.iter().any(|row| row == "Customize the container…"),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "Take out of the container"),
            "{rows:?}"
        );
        context::pick("Take out of the container");
        assert_eq!(child_ids(&rig, "shelf"), ["row-a"]);
        let loose = written(&rig, "row-b");
        assert_eq!(
            loose.children[0].representation,
            Some(Representation::WidgetS)
        );
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Take Clock out of the container")
        );
        undoes_to(&rig, &before);

        ask("row-a");
        context::pick("Customize the container…");
        assert_eq!(
            session::selected(),
            Selection::Group(grid_node().group(&GroupId::new("shelf")))
        );
        assert!(popover::tree().is_some(), "the container's popover is open");
        popover::close();

        ask("weather");
        assert!(
            !context::rows()
                .iter()
                .any(|row| row == "Take out of the container"),
            "a loose widget is in no container"
        );
    }

    #[test]
    fn a_rows_child_handle_sets_its_weight_and_says_its_share() {
        let rig = rig_with("container-weight", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::Instance(node_of("row-a"))));
        let mut screen = tools_page();
        let before = stored(&rig);
        let shelf = frame("shelf");
        let start = drawn("row-a");
        let room = shelf.inner.width - shelf.gap;
        let handle = (start.x + start.width, start.y + start.height / 2.0);
        let hint = screen.drag(handle, (start.x + room * 0.6, handle.1));
        assert_eq!(hint.and_then(|hint| hint.tag).as_deref(), Some("60 %"));
        let weight = child(&rig, "shelf", "row-a").weight.expect("a weight");
        assert!(close_within(weight, 1.5, 1e-3), "{weight}");
        undoes_to(&rig, &before);
    }

    #[test]
    fn a_grid_childs_corner_sets_its_span_and_a_free_ones_its_box() {
        let rig = rig_with("container-corner", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::Instance(node_of("board-a"))));
        let mut screen = tools_page();
        let board = frame("board");
        let start = drawn("board-a");
        let hint = screen.drag(
            (start.x + start.width, start.y + start.height),
            (
                board.inner.x + board.inner.width,
                board.inner.y + board.inner.height,
            ),
        );
        assert_eq!(hint.and_then(|hint| hint.tag).as_deref(), Some("2 × 2"));
        let cell = child(&rig, "board", "board-a").cell.expect("a cell");
        assert_eq!((cell.col_span, cell.row_span), (2, 2));

        assert!(session::select(Selection::Instance(node_of("pad-b"))));
        undraw();
        let mut screen = tools_page();
        let pad = frame("pad");
        let start = drawn("pad-b");
        let to = (
            start.x + pad.inner.width * 0.4,
            start.y + pad.inner.height * 0.4,
        );
        let hint = screen.drag((start.x + start.width, start.y + start.height), to);
        assert_eq!(hint.and_then(|hint| hint.tag).as_deref(), Some("40 × 40 %"));
        let placed = child(&rig, "pad", "pad-b").rect.expect("a box");
        assert!(
            close_within(placed.w, 0.4, 1e-3) && close_within(placed.h, 0.4, 1e-3),
            "{placed:?}"
        );
        assert!(
            close_within(placed.x, 0.5, 1e-3) && close_within(placed.y, 0.5, 1e-3),
            "its corner stays: {placed:?}"
        );
    }

    #[test]
    fn keys_move_and_resize_a_child_inside_its_container() {
        let rig = rig_with("container-keys", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let shift = with(|held| held.is_shift = true);
        let ctrl = with(|held| held.is_ctrl = true);
        let right = Key::Named(NamedKey::ArrowRight);
        let down = Key::Named(NamedKey::ArrowDown);
        let before = stored(&rig);

        assert!(session::select(Selection::Instance(node_of("row-a"))));
        assert!(tap(right.clone(), shift));
        assert_eq!(child_ids(&rig, "shelf"), ["row-b", "row-a"]);
        undoes_to(&rig, &before);

        assert!(session::select(Selection::Instance(node_of("row-a"))));
        assert!(tap(right.clone(), ctrl));
        assert_eq!(child(&rig, "shelf", "row-a").weight, Some(1.25));
        tap(down.clone(), ctrl);
        assert_eq!(
            child(&rig, "shelf", "row-a").weight,
            Some(1.25),
            "a row's child only grows across"
        );
        assert!(mode::refusal().peek().is_some(), "the strip says why");

        assert!(session::select(Selection::Instance(node_of("board-a"))));
        assert!(tap(right.clone(), shift));
        assert_eq!(
            child(&rig, "board", "board-a").cell,
            Some(ChildCell::at(1, 0))
        );
        tap(right.clone(), shift);
        assert_eq!(
            child(&rig, "board", "board-a").cell,
            Some(ChildCell::at(1, 0)),
            "never off its inner grid"
        );
        assert!(tap(down.clone(), ctrl));
        assert_eq!(
            child(&rig, "board", "board-a").cell,
            Some(ChildCell {
                col: 1,
                row: 0,
                col_span: 1,
                row_span: 2
            })
        );

        assert!(session::select(Selection::Instance(node_of("pad-a"))));
        assert!(tap(right.clone(), shift));
        assert_eq!(
            child(&rig, "pad", "pad-a").rect,
            Some(rect(0.05, 0.0, 0.4, 0.4))
        );
        assert!(tap(right.clone(), ctrl));
        assert_eq!(
            child(&rig, "pad", "pad-a").rect,
            Some(rect(0.05, 0.0, 0.45, 0.4))
        );

        assert!(session::select(Selection::Instance(node_of("col-a"))));
        assert!(tap(down.clone(), ctrl));
        assert_eq!(
            child(&rig, "col", "col-a").weight,
            Some(1.25),
            "a column's child grows down"
        );

        assert!(tap(
            Key::Named(NamedKey::ArrowUp),
            with(|held| held.is_alt = true)
        ));
        assert_eq!(
            session::selected(),
            Selection::Group(grid_node().group(&GroupId::new("col"))),
            "Alt+↑ selects the container"
        );
    }

    #[test]
    fn ctrl_shift_arrows_put_a_widget_into_the_container_that_way_and_the_menu_key_takes_it_out() {
        let rig = rig_with("container-keys-adopt", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        draw(LayerKind::Desktop);
        let before = stored(&rig);
        assert!(session::select(Selection::Instance(node_of("weather"))));
        assert!(tap(
            Key::Named(NamedKey::ArrowUp),
            with(|held| {
                held.is_ctrl = true;
                held.is_shift = true;
            })
        ));
        assert_eq!(child_ids(&rig, "col"), ["col-a", "col-b", "weather"]);
        undoes_to(&rig, &before);

        assert!(session::select(Selection::Instance(node_of("col-b"))));
        assert!(tap(Key::Named(NamedKey::ContextMenu), NONE));
        context::pick("Take out of the container");
        assert_eq!(child_ids(&rig, "col"), ["col-a"]);
        assert_eq!(holder_of("col-b").as_deref(), Some("col-b"));
    }

    fn clock_panel() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("clock-panel"))
    }

    fn in_panel(id: &str) -> Node {
        clock_panel().instance(&GroupId::new("shelf"), &InstanceId::new(id))
    }

    /// The top bar's clock given a panel eight cells by six, holding a row container of two clocks.
    fn shelf_in_a_panel(layout: &mut Layout) {
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
            groups: vec![Group {
                arrange: Some(Arrange::Row),
                children: vec![widget("row-a", "clock"), widget("row-b", "clock")],
                ..cell_group("shelf", (0, 0, 4, 2))
            }],
            ..Area::default()
        });
    }

    fn written_panel(rig: &Rig) -> Area {
        stored(rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id.as_str() == "clock-panel")
            .cloned()
            .expect("the panel is written")
    }

    fn panel_children(rig: &Rig) -> Vec<String> {
        written_panel(rig)
            .groups
            .iter()
            .find(|group| group.id.as_str() == "shelf")
            .expect("the container")
            .children
            .iter()
            .map(|child| child.id.to_string())
            .collect()
    }

    /// The panel open at a place of its own, its container's two children halving the container, as the panel's window draws them; answers the container as drawn.
    fn open_panel() -> container::Frame {
        let pitch = AreaKind::CELL + AreaKind::GAP;
        rects::track_spanning(
            clock_panel(),
            vec![signal(telar::Rect::new(
                800.0,
                120.0,
                8.0 * pitch,
                6.0 * pitch,
            ))],
        );
        let shelf = container::frame_in(
            &widgets::drop_grids(SCREEN, LayerKind::Top),
            &AreaId::new("clock-panel"),
            &GroupId::new("shelf"),
        )
        .expect("the container on the open panel");
        let half = (shelf.inner.width - shelf.gap) / 2.0;
        for (at, id) in ["row-a", "row-b"].into_iter().enumerate() {
            let x = shelf.inner.x + at as f32 * (half + shelf.gap);
            rects::track_spanning(
                in_panel(id),
                vec![signal(telar::Rect::new(
                    x,
                    shelf.inner.y,
                    half,
                    shelf.inner.height,
                ))],
            );
        }
        shelf
    }

    /// A container on a panel takes every key a container on a grid does: Shift+arrows reorder its children, Ctrl+arrows weigh them, the menu takes one out onto the panel's cells, and Shift+N on the selected panel makes another container there — each one undo entry.
    #[test]
    fn a_container_on_a_panel_answers_every_key() {
        let rig = rig_with("container-panel-keys", shelf_in_a_panel);
        let _owner = owner();
        let _host = enter(LayerKind::Top);
        open_panel();
        let before = stored(&rig);
        let shift = with(|held| held.is_shift = true);
        let ctrl = with(|held| held.is_ctrl = true);

        assert!(session::select(Selection::Instance(in_panel("row-a"))));
        assert!(tap(Key::Named(NamedKey::ArrowRight), shift));
        assert_eq!(panel_children(&rig), ["row-b", "row-a"]);
        undoes_to(&rig, &before);

        assert!(session::select(Selection::Instance(in_panel("row-a"))));
        assert!(tap(Key::Named(NamedKey::ArrowRight), ctrl));
        let weight = written_panel(&rig).groups[0].children[0].weight;
        assert_eq!(weight, Some(1.25));
        undoes_to(&rig, &before);

        context::open(Asked {
            node: in_panel("row-a"),
            window: LayerKind::Top,
            at: Some((900.0, 300.0)),
        })
        .expect("the menu opens");
        context::pick("Take out of the container");
        assert_eq!(panel_children(&rig), ["row-b"]);
        let loose = written_panel(&rig)
            .groups
            .into_iter()
            .find(|group| {
                group
                    .children
                    .iter()
                    .any(|child| child.id.as_str() == "row-a")
            })
            .expect("a group of its own on the panel");
        assert!(
            matches!(loose.kind, Some(GroupKind::Cell { .. })),
            "{loose:?}"
        );
        undoes_to(&rig, &before);

        assert!(session::select(Selection::Area(clock_panel())));
        assert!(tap(Key::Char('N'), shift));
        let made = written_panel(&rig)
            .groups
            .into_iter()
            .find(|group| group.id.as_str() == "container")
            .expect("a container on the panel");
        assert!(
            made.arrange.is_some() && made.children.is_empty(),
            "{made:?}"
        );
        undoes_to(&rig, &before);
    }

    /// On an open panel in the top mode, a child is carried to another place in its container, and its end handle weighs it — each one undo entry.
    #[test]
    fn a_container_on_an_open_panel_takes_the_pointer() {
        let rig = rig_with("container-panel-pointer", shelf_in_a_panel);
        let _owner = owner();
        let _host = enter(LayerKind::Top);
        let shelf = open_panel();
        let current = mode::current().expect("the mode is up");
        let mut page = Page::over(vec![widgets::tool(&current).expect("the tools build")]);
        let before = stored(&rig);

        let start = rects::rect(&in_panel("row-a")).expect("drawn");
        let far_end = (
            shelf.inner.x + shelf.inner.width * 0.9,
            shelf.inner.y + shelf.inner.height / 2.0,
        );
        page.drag(centre(start), far_end);
        assert_eq!(panel_children(&rig), ["row-b", "row-a"]);
        undoes_to(&rig, &before);

        assert!(session::select(Selection::Instance(in_panel("row-a"))));
        let mut page = Page::over(vec![widgets::tool(&current).expect("the tools build")]);
        let handle = (start.x + start.width, start.y + start.height / 2.0);
        let room = shelf.inner.width - shelf.gap;
        let hint = page.drag(handle, (start.x + room * 0.6, handle.1));
        assert_eq!(hint.and_then(|hint| hint.tag).as_deref(), Some("60 %"));
        let weight = written_panel(&rig).groups[0].children[0]
            .weight
            .expect("a weight");
        assert!(close_within(weight, 1.5, 1e-3), "{weight}");
        undoes_to(&rig, &before);
    }

    /// Selecting climbs from a child to its container and on to the grid, by clicking the same spot again and by Alt+Up, and Alt+Down goes back in; selecting writes nothing.
    #[test]
    fn a_child_selects_its_container_then_the_grid_by_click_and_by_alt_arrow() {
        let rig = rig_with("container-climb", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let shelf = || grid_node().group(&GroupId::new("shelf"));
        let mut screen = tools_page();
        let at = centre(drawn("row-b"));
        let apart = |step: f32| (at.0 + step * 20.0, at.1);
        screen.click(apart(0.0));
        assert_eq!(session::selected(), Selection::Instance(node_of("row-b")));
        screen.click(apart(1.0));
        assert_eq!(session::selected(), Selection::Group(shelf()));
        screen.click(apart(2.0));
        assert_eq!(session::selected(), Selection::Area(grid_node()));

        let alt = with(|held| held.is_alt = true);
        assert!(session::select(Selection::Instance(node_of("row-b"))));
        assert!(tap(Key::Named(NamedKey::ArrowUp), alt));
        assert_eq!(session::selected(), Selection::Group(shelf()));
        assert!(tap(Key::Named(NamedKey::ArrowUp), alt));
        assert_eq!(session::selected(), Selection::Area(grid_node()));
        assert!(tap(Key::Named(NamedKey::ArrowDown), alt));
        assert_eq!(session::selected(), Selection::Group(shelf()));
        assert!(tap(Key::Named(NamedKey::ArrowDown), alt));
        assert_eq!(session::selected(), Selection::Instance(node_of("row-a")));
        assert_eq!(stored(&rig), before);
    }

    /// A container made from the keyboard in the top mode and on the lock is one entry in the history, as it is on the desktop.
    #[test]
    fn shift_n_is_one_undo_entry_in_the_top_mode_and_on_the_lock() {
        for layer in [LayerKind::Top, LayerKind::Lock] {
            let rig = rig_with("container-layers", |_| {});
            let _owner = owner();
            let _host = enter(layer);
            let before = stored(&rig);
            assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
            assert_ne!(stored(&rig), before, "{layer}");
            undoes_to(&rig, &before);
        }
    }

    /// Where a container gesture named `what` starts and where it is carried to, on the screen as it is drawn now.
    fn gesture(what: &str) -> ((f32, f32), (f32, f32)) {
        let grids = widgets::grids(SCREEN, LayerKind::Desktop);
        let empty = grids[0].0.rect_of(crate::modes::grid::Cells {
            col: 0,
            row: 7,
            cols: 1,
            rows: 1,
        });
        let (shelf, board) = (frame("shelf"), frame("board"));
        let middle = |inner: telar::Rect, x: f32, y: f32| {
            (inner.x + inner.width * x, inner.y + inner.height * y)
        };
        match what {
            "reorder" => (centre(drawn("row-a")), middle(shelf.inner, 0.9, 0.5)),
            "take out" => {
                let out = drawn("row-b");
                ((out.x + 2.0, out.y + 2.0), (empty.x + 2.0, empty.y + 2.0))
            }
            "adopt" => (centre(drawn("weather")), middle(shelf.inner, 0.5, 0.5)),
            "weight" => {
                let at = drawn("row-a");
                (
                    (at.x + at.width, at.y + at.height / 2.0),
                    (
                        at.x + (shelf.inner.width - shelf.gap) * 0.6,
                        at.y + at.height / 2.0,
                    ),
                )
            }
            "span" => {
                let at = drawn("board-a");
                (
                    (at.x + at.width, at.y + at.height),
                    middle(board.inner, 1.0, 1.0),
                )
            }
            other => panic!("no gesture called {other}"),
        }
    }

    /// Esc before a container gesture is let go puts the layout back exactly and records nothing: a reorder, taking a child out, a widget dropped in, a row child's weight and a grid child's span.
    #[test]
    fn escape_mid_gesture_puts_every_container_gesture_back_with_nothing_recorded() {
        let rig = rig_with("container-escape", four_containers);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        for (what, selected) in [
            ("reorder", None),
            ("take out", None),
            ("adopt", None),
            ("weight", Some("row-a")),
            ("span", Some("board-a")),
        ] {
            undraw();
            session::clear_selection();
            if let Some(id) = selected {
                assert!(session::select(Selection::Instance(node_of(id))));
            }
            let mut screen = tools_page();
            let (from, to) = gesture(what);
            assert!(
                screen.drag_cancelled(&before, from, to),
                "{what} previews first"
            );
            assert_eq!(session::draft().peek(), before, "{what}: Esc puts it back");
            assert_eq!(stored(&rig), before, "{what}");
            assert_eq!(rig.undo_label(), None, "{what}: nothing is recorded");
        }
    }

    /// Where the layer has no grid to put a container on, Shift+N says so and writes nothing.
    #[test]
    fn shift_n_with_no_grid_is_refused_and_writes_nothing() {
        for layer in [LayerKind::Desktop, LayerKind::Lock] {
            let rig = rig_with("container-no-grid", |layout| {
                layout.outputs[0]
                    .layers
                    .desktop
                    .areas
                    .retain(|area| area.id.as_str() != "widgets");
                layout.outputs[0]
                    .layers
                    .lock
                    .areas
                    .retain(|area| area.id.as_str() != "lock-readings");
            });
            let _owner = owner();
            let _host = enter(layer);
            let before = stored(&rig);
            assert!(tap(Key::Char('N'), with(|held| held.is_shift = true)));
            assert_eq!(stored(&rig), before, "{layer}");
            assert_eq!(rig.undo_label(), None, "{layer}");
            assert!(
                mode::refusal()
                    .peek()
                    .is_some_and(|said| said.contains("grid")),
                "{layer}: {:?}",
                mode::refusal().peek()
            );
        }
    }
}
