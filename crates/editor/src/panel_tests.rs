#[cfg(test)]
mod tests {
    use telar::{Rect, signal};

    use layout::{Area, AreaId, AreaKind, GroupId, InstanceId, LayerKind};
    use surfaces::menu::Asked;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::ModuleDescriptor;
    use ui::host::WidgetSize;

    use crate::popover::panel::{Grows, growing, grows, handle_at};
    use crate::rig::{Page, Rig, SCREEN, Scope, bar, enter, module, rig, rig_with, stored, tap};
    use crate::session::{self, Selection};
    use crate::{context, mode, popover};

    static PROBES: &[ModuleDescriptor] = &[module("clock", "Clock", &[WidgetSize::M])];

    fn scope() -> Scope {
        ui::descriptor::install(PROBES);
        Scope::new()
    }

    fn clock() -> Node {
        bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))
    }

    fn panel_node(id: &str) -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new(id))
    }

    fn panel(rig: &Rig) -> Option<Area> {
        stored(rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| matches!(area.kind, Some(AreaKind::Panel { .. })))
            .cloned()
    }

    fn menu_of(node: Node) -> Vec<String> {
        context::open(Asked {
            node,
            window: LayerKind::Top,
            at: Some((900.0, 17.0)),
        })
        .expect("the menu opens");
        context::rows()
    }

    fn along(area: &Area) -> Option<bool> {
        match area.kind {
            Some(AreaKind::Panel { along, .. }) => along,
            _ => None,
        }
    }

    #[test]
    fn a_chip_in_a_bar_is_offered_a_panel_and_a_dock_and_the_dock_runs_along_the_bar() {
        let rig = rig("panel-dock");
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        let rows = menu_of(clock());
        assert!(rows.iter().any(|row| row == "Give it a panel"), "{rows:?}");
        assert!(
            rows.iter()
                .any(|row| row == "Give it a panel along the whole bar"),
            "{rows:?}"
        );
        context::pick("Give it a panel along the whole bar");
        let written = panel(&rig).expect("the dock is written");
        assert_eq!(along(&written), Some(true));
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Give Clock a panel along the whole bar")
        );
        assert_eq!(
            session::selected(),
            Selection::Area(panel_node("clock-panel"))
        );

        let rows = menu_of(clock());
        assert!(rows.iter().any(|row| row == "Edit its panel"), "{rows:?}");
        assert!(
            !rows.iter().any(|row| row.starts_with("Give it")),
            "{rows:?}"
        );
        assert!(session::select(Selection::Instance(clock())));
        context::pick("Edit its panel");
        assert_eq!(
            session::selected(),
            Selection::Area(panel_node("clock-panel"))
        );
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Give Clock a panel along the whole bar"),
            "editing writes nothing"
        );

        assert_eq!(
            session::undo().as_deref(),
            Ok("Give Clock a panel along the whole bar")
        );
        assert_eq!(panel(&rig), None, "one undo takes it back");
    }

    #[test]
    fn outside_its_mode_the_row_enters_the_mode_first() {
        let rig = rig("panel-outside");
        let _scope = scope();
        let rows = menu_of(clock());
        assert!(rows.iter().any(|row| row == "Give it a panel"), "{rows:?}");
        context::pick("Give it a panel");
        assert_eq!(mode::current().map(|mode| mode.layer), Some(LayerKind::Top));
        assert_eq!(along(&panel(&rig).expect("written")), None);
        assert_eq!(
            session::selected(),
            Selection::Area(panel_node("clock-panel"))
        );
    }

    #[test]
    fn removing_the_panel_is_one_undo_entry_and_keeps_its_owner() {
        let rig = rig("panel-remove");
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        crate::panel::give(&clock(), crate::panel::Shape::Beside).expect("the panel is given");
        crate::keys::remove(&Selection::Area(panel_node("clock-panel"))).expect("removed");
        assert_eq!(panel(&rig), None);
        assert!(
            stored(&rig).outputs[0].layers.top.areas[0].places(&InstanceId::new("clock")),
            "the owner stays"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Remove clock-panel"));
        assert_eq!(session::undo().as_deref(), Ok("Remove clock-panel"));
        assert!(panel(&rig).is_some(), "one undo puts it back");
    }

    #[test]
    fn the_modes_keys_answer_while_the_panel_it_opened_is_up() {
        let rig = rig("panel-keys");
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        crate::panel::give(&clock(), crate::panel::Shape::Beside).expect("the panel is given");
        let owner = surfaces::panel::Owner::of(&clock()).expect("an instance");
        assert!(transient::is_open(&owner.id()), "the panel is up");
        assert!(
            tap(
                telar::Key::Named(telar::NamedKey::Delete),
                telar::ModifiersState::default()
            ),
            "the mode's keys still answer"
        );
        assert_eq!(panel(&rig), None, "Delete takes the selected panel away");
    }

    #[test]
    fn the_lock_layer_has_no_panel_rows_and_gives_no_panel() {
        let _rig = rig("panel-lock");
        let _scope = scope();
        let _mode = enter(LayerKind::Lock);
        let desktop = crate::rig::desktop();
        let lock = desktop
            .resolved
            .layer(LayerKind::Lock)
            .expect("a lock layer");
        let (area, group, child) = lock
            .areas
            .iter()
            .find_map(|area| {
                let group = area
                    .groups
                    .iter()
                    .find(|group| !group.children.is_empty())?;
                Some((
                    area.id.clone(),
                    group.id.clone(),
                    group.children[0].id.clone(),
                ))
            })
            .expect("the lock shows a reading");
        let reading = Node::area(Some(SCREEN), LayerKind::Lock, &area).instance(&group, &child);
        assert_eq!(crate::panel::offer(&desktop, &reading), None);
        assert!(crate::panel::rows(&reading).is_empty());
        assert!(crate::panel::give(&reading, crate::panel::Shape::Beside).is_err());
    }

    #[test]
    fn the_panel_popover_steps_cells_by_row_and_by_its_handle_each_one_undo_entry() {
        let rig = rig("panel-popover");
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        crate::panel::give(&clock(), crate::panel::Shape::Beside).expect("the panel is given");
        let node = panel_node("clock-panel");
        let drawn = Rect::new(800.0, 50.0, 368.0, 288.0);
        rects::track_spanning(node.clone(), vec![signal(drawn)]);
        rects::track_spanning(clock(), vec![signal(Rect::new(940.0, 5.0, 40.0, 24.0))]);

        popover::open_area(node.clone()).expect("the panel's popover opens");
        let _page = popover_page();
        assert!(popover::edits("along"), "a chip in a bar can be docked");
        let cols = popover::shared::<f32>("cols").expect("the columns row");
        cols.set(6.0);
        popover::close();
        let cells = |rig: &Rig| match panel(rig).and_then(|area| area.kind) {
            Some(AreaKind::Panel { cols, rows, .. }) => (cols, rows),
            _ => (None, None),
        };
        assert_eq!(cells(&rig), (Some(6), None));
        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock-panel"));

        popover::open_area(node).expect("it opens again");
        let mut page = popover_page();
        let corner = handle_at(drawn, grows(Some(config::Edge::Top), false));
        let pitch = layout::AreaKind::CELL + layout::AreaKind::GAP;
        let to = (corner.0 + pitch, corner.1 + pitch);
        page.button(corner, true);
        page.move_to(((corner.0 + to.0) / 2.0, corner.1));
        page.move_to(to);
        page.button(to, false);
        popover::close();
        assert_eq!(
            cells(&rig),
            (Some(8), Some(4)),
            "centred on its owner, a pitch across is a column each side, and a pitch down a row"
        );
        assert_eq!(session::undo().as_deref(), Ok("Customize clock-panel"));
        assert_eq!(cells(&rig), (Some(6), None), "one undo takes back the drag");
    }

    #[test]
    fn the_handle_sits_where_the_panel_grows() {
        let rect = Rect::new(100.0, 100.0, 200.0, 100.0);
        let at = |edge, docked| handle_at(rect, grows(edge, docked));
        assert_eq!(at(Some(config::Edge::Top), false), (300.0, 200.0));
        assert_eq!(at(Some(config::Edge::Bottom), false), (300.0, 100.0));
        assert_eq!(at(Some(config::Edge::Right), false), (100.0, 200.0));
        assert_eq!(at(Some(config::Edge::Top), true), (200.0, 200.0));
        assert_eq!(at(Some(config::Edge::Left), true), (300.0, 150.0));
        let beside: Grows = grows(None, false);
        assert_eq!(beside.across, 2.0);
    }

    /// Off any bar, a panel opens under its owner where it fits there and above it otherwise, and its handle sits on the corner it grows from either way.
    #[test]
    fn off_a_bar_the_handle_follows_the_side_the_panel_opens_on() {
        let _rig = rig("panel-hangs");
        let _scope = scope();
        let _mode = enter(LayerKind::Desktop);
        let id = InstanceId::new("clock-2");
        let owner = Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("centre"))
            .instance(&GroupId::new("clock"), &id);
        crate::panel::give(&owner, crate::panel::Shape::Beside).expect("the widget gets a panel");
        let panel = Node::area(
            Some(SCREEN),
            LayerKind::Desktop,
            &AreaId::new("clock-2-panel"),
        );
        let drawn = Rect::new(800.0, 300.0, 368.0, 288.0);
        rects::track_spanning(panel.clone(), vec![signal(drawn)]);
        let at = signal(Rect::new(900.0, 100.0, 160.0, 160.0));
        rects::track_spanning(owner, vec![at]);

        let below = growing(&panel, &id, None, false);
        assert!(below.down, "room under its owner: {below:?}");
        assert_eq!(handle_at(drawn, below), (1168.0, 588.0));

        at.set(Rect::new(900.0, 880.0, 160.0, 160.0));
        let above = growing(&panel, &id, None, false);
        assert!(!above.down, "no room under its owner: {above:?}");
        assert_eq!(handle_at(drawn, above), (1168.0, 300.0));
    }

    fn popover_page() -> Page {
        Page::of(popover::tree().expect("a popover is open"))
    }

    fn with_left_clock(layout: &mut layout::Layout) {
        let mut side = layout::default_bar(AreaId::new("bar-left"), config::Edge::Left);
        side.groups = vec![layout::Group {
            id: GroupId::new("start-l"),
            kind: Some(layout::GroupKind::Zone {
                zone: layout::Zone::Start,
            }),
            children: vec![layout::Instance {
                id: InstanceId::new("clock-l"),
                module: Some("clock".to_string()),
                ..layout::Instance::default()
            }],
            ..layout::Group::default()
        }];
        layout.outputs[0].layers.top.areas.push(side);
    }

    fn cells(rig: &Rig, id: &str) -> (Option<u32>, Option<u32>) {
        let found = stored(rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id.as_str() == id)
            .cloned();
        match found.and_then(|area| area.kind) {
            Some(AreaKind::Panel { cols, rows, .. }) => (cols, rows),
            _ => (None, None),
        }
    }

    /// A panel along a horizontal bar is dragged to its depth by rows alone and one along a vertical bar by columns alone, each one undo entry; Esc mid-drag puts the depth back with nothing recorded.
    #[test]
    fn a_docked_panels_handle_sets_its_depth_on_either_kind_of_bar() {
        let rig = rig_with("panel-dock-handle", with_left_clock);
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        let pitch = layout::AreaKind::CELL + layout::AreaKind::GAP;
        let left_clock = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-left"))
            .instance(&GroupId::new("start-l"), &InstanceId::new("clock-l"));
        for (owner, panel_id, edge, drawn, owner_box) in [
            (
                clock(),
                "clock-panel",
                config::Edge::Top,
                Rect::new(0.0, 34.0, 1920.0, 288.0),
                Rect::new(940.0, 5.0, 40.0, 24.0),
            ),
            (
                left_clock,
                "clock-l-panel",
                config::Edge::Left,
                Rect::new(34.0, 0.0, 368.0, 1080.0),
                Rect::new(5.0, 100.0, 24.0, 40.0),
            ),
        ] {
            crate::panel::give(&owner, crate::panel::Shape::Along).expect("the dock is given");
            let node = panel_node(panel_id);
            rects::track_spanning(node.clone(), vec![signal(drawn)]);
            rects::track_spanning(owner, vec![signal(owner_box)]);
            let corner = handle_at(drawn, grows(Some(edge), true));
            let to = (corner.0 + 3.0 * pitch, corner.1 + 3.0 * pitch);
            let given = stored(&rig);
            let before = given.clone();

            let depth = match edge.is_vertical() {
                true => ("cols", layout::AreaKind::PANEL_COLS),
                false => ("rows", layout::AreaKind::PANEL_ROWS),
            };
            let at_rest = || {
                popover::shared::<f32>(depth.0)
                    .expect("the depth row")
                    .peek()
            };
            let escape = || {
                assert!(tap(
                    telar::Key::Named(telar::NamedKey::Escape),
                    telar::ModifiersState::default()
                ));
            };

            popover::open_area(node.clone()).expect("the panel's popover opens");
            let mut page = popover_page();
            page.button(corner, true);
            page.move_to(to);
            assert_eq!(
                at_rest(),
                (depth.1 + 3) as f32,
                "{edge:?}: the drag previews"
            );
            escape();
            assert_eq!(
                at_rest(),
                depth.1 as f32,
                "{edge:?}: Esc puts the depth back"
            );
            page.button(to, false);
            escape();
            assert_eq!(stored(&rig), before, "{edge:?}: nothing is recorded");

            popover::open_area(node).expect("it opens again");
            let mut page = popover_page();
            page.button(corner, true);
            page.move_to(((corner.0 + to.0) / 2.0, (corner.1 + to.1) / 2.0));
            page.move_to(to);
            page.button(to, false);
            popover::close();
            let (cols, rows) = cells(&rig, panel_id);
            match edge.is_vertical() {
                true => assert_eq!(
                    (cols, rows),
                    (Some(layout::AreaKind::PANEL_COLS + 3), None),
                    "{edge:?}"
                ),
                false => assert_eq!(
                    (cols, rows),
                    (None, Some(layout::AreaKind::PANEL_ROWS + 3)),
                    "{edge:?}"
                ),
            }
            assert_eq!(rig.undo_label(), Some(format!("Customize {panel_id}")),);
            session::undo().expect("one undo takes back the drag");
            assert_eq!(stored(&rig), given, "{edge:?}");
        }
    }

    #[test]
    fn a_handle_drag_cancelled_with_esc_leaves_both_dimensions_as_written() {
        let pitch = AreaKind::CELL + AreaKind::GAP;
        for (shape, drawn) in [
            (
                crate::panel::Shape::Along,
                Rect::new(0.0, 34.0, 1920.0, 288.0),
            ),
            (
                crate::panel::Shape::Beside,
                Rect::new(800.0, 50.0, 368.0, 288.0),
            ),
        ] {
            let rig = rig("panel-handle-cancelled");
            let _scope = scope();
            let _mode = enter(LayerKind::Top);
            crate::panel::give(&clock(), shape).expect("the panel is given");
            let node = panel_node("clock-panel");
            rects::track_spanning(node.clone(), vec![signal(drawn)]);
            rects::track_spanning(clock(), vec![signal(Rect::new(940.0, 5.0, 40.0, 24.0))]);
            let docked = shape == crate::panel::Shape::Along;
            let corner = handle_at(drawn, grows(Some(config::Edge::Top), docked));
            let to = (corner.0 + 3.0 * pitch, corner.1 + 3.0 * pitch);
            let given = stored(&rig);
            let (written, label) = (cells(&rig, "clock-panel"), rig.undo_label());

            popover::open_area(node).expect("the panel's popover opens");
            let mut page = popover_page();
            page.button(corner, true);
            page.move_to(((corner.0 + to.0) / 2.0, (corner.1 + to.1) / 2.0));
            page.move_to(to);
            assert_ne!(
                crate::session::draft().peek(),
                given,
                "{shape:?}: the drag previews"
            );
            assert!(tap(
                telar::Key::Named(telar::NamedKey::Escape),
                telar::ModifiersState::default()
            ));
            page.button(to, false);
            popover::close();

            assert_eq!(cells(&rig, "clock-panel"), written, "{shape:?}");
            assert_eq!(stored(&rig), given, "{shape:?}: nothing is recorded");
            assert_eq!(rig.undo_label(), label, "{shape:?}");
        }
    }

    #[test]
    fn a_press_on_the_handle_without_travel_leaves_both_dimensions_as_written() {
        for (shape, drawn) in [
            (
                crate::panel::Shape::Along,
                Rect::new(0.0, 34.0, 1920.0, 288.0),
            ),
            (
                crate::panel::Shape::Beside,
                Rect::new(800.0, 50.0, 368.0, 288.0),
            ),
        ] {
            let rig = rig("panel-handle-press");
            let _scope = scope();
            let _mode = enter(LayerKind::Top);
            crate::panel::give(&clock(), shape).expect("the panel is given");
            let node = panel_node("clock-panel");
            rects::track_spanning(node.clone(), vec![signal(drawn)]);
            rects::track_spanning(clock(), vec![signal(Rect::new(940.0, 5.0, 40.0, 24.0))]);
            let docked = shape == crate::panel::Shape::Along;
            let corner = handle_at(drawn, grows(Some(config::Edge::Top), docked));
            let given = stored(&rig);
            let label = rig.undo_label();

            popover::open_area(node).expect("the panel's popover opens");
            let mut page = popover_page();
            page.button(corner, true);
            page.button(corner, false);
            popover::close();

            assert_eq!(stored(&rig), given, "{shape:?}: nothing is written");
            assert_eq!(rig.undo_label(), label, "{shape:?}");
        }
    }

    /// Removing the owner with Delete takes its panel with it as the one entry, and one undo puts both back.
    #[test]
    fn removing_the_owner_takes_its_panel_with_it_as_one_undo_entry() {
        let rig = rig("panel-owner-removed");
        let _scope = scope();
        let _mode = enter(LayerKind::Top);
        crate::panel::give(&clock(), crate::panel::Shape::Beside).expect("the panel is given");
        let given = stored(&rig);
        assert!(session::select(Selection::Instance(clock())));
        assert!(tap(
            telar::Key::Named(telar::NamedKey::Delete),
            telar::ModifiersState::default()
        ));
        assert_eq!(panel(&rig), None, "no panel is left opening from nothing");
        assert!(
            !stored(&rig).outputs[0].layers.top.areas[0].places(&InstanceId::new("clock")),
            "the clock is gone"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Remove Clock"));
        session::undo().expect("one undo takes it back");
        assert_eq!(stored(&rig), given, "the clock and its panel are both back");
    }

    fn drawn_says(page: &Page, wanted: &str) -> bool {
        let mut found = false;
        telar::for_each_with_matrix(&page.0.commands(), |command, _| {
            if let telar::DrawCommand::Text { text, .. } = command
                && text.to_string() == wanted
            {
                found = true;
            }
        });
        found
    }

    /// Whichever edge its bar is on, a panel beside its owner has columns and rows and one along the bar a single depth, and the popover offers the shape of a panel on a bar.
    #[test]
    fn a_panels_popover_rows_follow_its_shape_on_every_edge() {
        for edge in config::Edge::ALL {
            let _rig = rig_with(&format!("panel-edge-{edge:?}"), |layout| {
                if let Some(AreaKind::Bar { edge: at, .. }) =
                    &mut layout.outputs[0].layers.top.areas[0].kind
                {
                    *at = Some(edge);
                }
            });
            let _scope = scope();
            let _mode = enter(LayerKind::Top);
            for shape in [crate::panel::Shape::Beside, crate::panel::Shape::Along] {
                crate::panel::give(&clock(), shape).expect("the panel is given");
                popover::open_area(panel_node("clock-panel")).expect("the popover opens");
                let page = popover_page();
                let says = |wanted: &str| drawn_says(&page, wanted);
                assert!(says("Shape"), "{edge:?} {shape:?}");
                let beside = shape == crate::panel::Shape::Beside;
                assert_eq!(says("Columns"), beside, "{edge:?} {shape:?}");
                assert_eq!(says("Rows"), beside, "{edge:?} {shape:?}");
                assert_eq!(says("Depth"), !beside, "{edge:?} {shape:?}");
                popover::close();
                crate::keys::remove(&Selection::Area(panel_node("clock-panel")))
                    .expect("the panel goes");
            }
        }
    }
}
