#[cfg(test)]
mod tests {
    use telar::{Rect, signal};

    use layout::{Area, AreaId, AreaKind, GroupId, InstanceId, LayerKind};
    use surfaces::menu::Asked;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::WidgetSize;

    use crate::popover::panel::{Grows, growing, grows, handle_at};
    use crate::rig::{Page, Rig, SCREEN, bar, enter, face, rig, stored, tap};
    use crate::session::{self, Selection};
    use crate::{context, mode, popover};

    static PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(face, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &[WidgetSize::M],
                build: face,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            popover::close();
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
        let _scope = Scope::new();
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
}
