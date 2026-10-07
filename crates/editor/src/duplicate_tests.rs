#[cfg(test)]
mod tests {
    use telar::Key;

    use config::Edge;

    use layout::{
        Area, AreaId, AreaKind, Arrange, Extent, Group, GroupId, GroupKind, Instance, InstanceId,
        KomponentId, LayerKind, Layout, Rect,
    };
    use surfaces::rects::Node;
    use ui::descriptor::ModuleDescriptor;
    use ui::host::WidgetSize;

    use crate::rig::{
        CTRL, Owner, Rig, SCREEN, bar, cell_group, enter, module, rig_prepared, rig_with, stored,
        tap, widget,
    };
    use crate::session::{self, Selection};
    use crate::{context, mode};

    static PROBES: &[ModuleDescriptor] = &[
        module("clock", "Clock", &WidgetSize::ALL),
        module("weather", "Weather", &WidgetSize::ALL),
        module("workspaces", "Workspaces", &WidgetSize::ALL),
        module("notes", "Notes", &WidgetSize::ALL),
    ];

    fn owner() -> Owner {
        Owner::installing(PROBES)
    }

    fn ctrl_d() -> bool {
        tap(Key::Char('d'), CTRL)
    }

    /// The desktop grid alone, holding a row container, a free container and one loose weather widget.
    fn on_the_grid(layout: &mut Layout) {
        let areas = &mut layout.outputs[0].layers.desktop.areas;
        areas.retain(|area| area.id.as_str() == "widgets");
        areas[0].groups = vec![
            Group {
                arrange: Some(Arrange::Row),
                children: vec![widget("row-a", "clock"), widget("row-b", "clock")],
                ..cell_group("shelf", (0, 0, 4, 2))
            },
            Group {
                arrange: Some(Arrange::Free),
                children: vec![
                    Instance {
                        rect: Some(Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 0.4,
                            h: 0.4,
                        }),
                        ..widget("pad-a", "clock")
                    },
                    Instance {
                        rect: Some(Rect {
                            x: 0.7,
                            y: 0.7,
                            w: 0.3,
                            h: 0.3,
                        }),
                        ..widget("pad-b", "clock")
                    },
                ],
                ..cell_group("pad", (5, 0, 4, 4))
            },
            Group {
                children: vec![widget("weather", "weather")],
                ..cell_group("weather", (10, 5, 2, 2))
            },
        ];
    }

    fn grid() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    fn written_grid(rig: &Rig) -> Area {
        stored(rig).outputs[0].layers.desktop.areas[0].clone()
    }

    fn written_group(rig: &Rig, id: &str) -> Group {
        written_grid(rig)
            .groups
            .into_iter()
            .find(|group| group.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is written"))
    }

    fn ids(group: &Group) -> Vec<String> {
        group
            .children
            .iter()
            .map(|child| child.id.to_string())
            .collect()
    }

    fn cells(group: &Group) -> (u32, u32, u32, u32) {
        match group.kind {
            Some(GroupKind::Cell {
                col,
                row,
                col_span,
                row_span,
            }) => (col, row, col_span, row_span),
            other => panic!("{} is not on cells: {other:?}", group.id),
        }
    }

    fn select(node: Node) {
        assert!(session::select(Selection::of(node)), "selectable");
    }

    fn refused_with(selection: Selection, said: &str) {
        let before = session::draft().peek();
        let why = crate::duplicate::duplicate(&selection)
            .expect_err("the copy is refused")
            .to_string();
        assert!(why.contains(said), "{why:?} does not say {said:?}");
        assert_eq!(session::draft().peek(), before, "nothing changed");
    }

    /// A loose widget goes onto the nearest free cells of its own span as a group of its own, with an id of its own, selected; one undo takes it back.
    #[test]
    fn a_loose_widget_is_copied_onto_the_nearest_free_cells_of_its_span() {
        let rig = rig_with("duplicate-loose", on_the_grid);
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);
        select(grid().instance(&GroupId::new("weather"), &InstanceId::new("weather")));

        assert!(ctrl_d(), "Ctrl+D answers");
        let written = written_grid(&rig);
        assert_eq!(written.groups.len(), 4, "{:?}", written.groups);
        let copy = written.groups.last().expect("a new group").clone();
        assert_eq!(ids(&copy), vec!["weather-2"]);
        let (col, row, cols, rows) = cells(&copy);
        assert_eq!((cols, rows), (2, 2), "the same span");
        assert_ne!((col, row), (10, 5), "beside the original, not over it");
        assert_eq!(
            session::selected(),
            Selection::Instance(grid().instance(&copy.id, &InstanceId::new("weather-2")))
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Duplicate Weather"));
        assert_eq!(session::undo().as_deref(), Ok("Duplicate Weather"));
        assert_eq!(stored(&rig), before, "one undo takes it back");
    }

    /// A child of a container is copied right after itself in it; in a free container the copy sits a little off the original.
    #[test]
    fn a_child_of_a_container_is_copied_right_after_itself() {
        let rig = rig_with("duplicate-child", on_the_grid);
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        select(grid().instance(&GroupId::new("shelf"), &InstanceId::new("row-a")));
        assert!(ctrl_d());
        let shelf = written_group(&rig, "shelf");
        assert_eq!(ids(&shelf), vec!["row-a", "clock-2", "row-b"]);
        assert_eq!(shelf.children[1].module.as_deref(), Some("clock"));

        select(grid().instance(&GroupId::new("pad"), &InstanceId::new("pad-a")));
        assert!(ctrl_d());
        let pad = written_group(&rig, "pad");
        assert_eq!(ids(&pad)[..2], ["pad-a".to_string(), "clock-3".to_string()]);
        assert_eq!(
            pad.children[1].rect,
            Some(Rect {
                x: 0.05,
                y: 0.05,
                w: 0.4,
                h: 0.4,
            })
        );
        assert_eq!(session::undo().as_deref(), Ok("Duplicate Clock"));
        assert_eq!(ids(&written_group(&rig, "pad")), vec!["pad-a", "pad-b"]);
    }

    /// A container is copied with everything it holds onto the nearest free cells of its span, each child under an id of its own.
    #[test]
    fn a_container_is_copied_with_its_children() {
        let rig = rig_with("duplicate-group", on_the_grid);
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        select(grid().group(&GroupId::new("shelf")));
        assert!(ctrl_d());
        let copy = written_group(&rig, "shelf-2");
        assert_eq!(copy.arrange, Some(Arrange::Row));
        assert_eq!(ids(&copy), vec!["clock-2", "clock-3"]);
        let (col, row, cols, rows) = cells(&copy);
        assert_eq!((cols, rows), (4, 2));
        assert_ne!((col, row), (0, 0));
        assert_eq!(
            session::selected(),
            Selection::Group(grid().group(&GroupId::new("shelf-2")))
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Duplicate shelf"));
    }

    /// A group that uses a komponent is copied as another use of it, under a group id of its own.
    #[test]
    fn a_komponent_use_keeps_its_komponent() {
        let pill = layout::Komponent {
            children: vec![widget("face", "clock")],
            ..layout::Komponent::default()
        };
        let rig = rig_prepared(
            "duplicate-komponent",
            |layout| {
                on_the_grid(layout);
                layout.outputs[0].layers.desktop.areas[0]
                    .groups
                    .push(Group {
                        komponent: Some(KomponentId::new("pill")),
                        ..cell_group("pill", (0, 5, 2, 2))
                    });
            },
            |store| {
                store
                    .add_komponent(KomponentId::new("pill"), pill)
                    .expect("a komponent");
            },
        );
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        select(grid().group(&GroupId::new("pill")));
        assert!(ctrl_d());
        let copy = written_group(&rig, "pill-2");
        assert_eq!(copy.komponent, Some(KomponentId::new("pill")));
        assert!(
            copy.children.is_empty(),
            "the komponent holds what it draws"
        );
    }

    /// A chip is copied right after itself on its bar.
    #[test]
    fn a_chip_is_copied_beside_itself() {
        let rig = rig_with("duplicate-chip", |_| {});
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        select(bar().instance(&GroupId::new("center"), &InstanceId::new("clock")));
        assert!(ctrl_d());
        let center = stored(&rig).outputs[0].layers.top.areas[0]
            .groups
            .iter()
            .find(|group| group.id.as_str() == "center")
            .cloned()
            .expect("the centre group");
        assert_eq!(ids(&center), vec!["clock", "clock-3"]);
    }

    /// A bar is copied right after itself into the free stretch of its edge, as long as it is.
    #[test]
    fn a_bar_is_copied_into_the_free_stretch_of_its_edge() {
        let rig = rig_with("duplicate-bar", |layout| {
            if let Some(AreaKind::Bar { length, offset, .. }) =
                &mut layout.outputs[0].layers.top.areas[0].kind
            {
                *length = Some(Extent::Px(600.0));
                *offset = Some(0.0);
            }
        });
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        select(bar());
        assert!(ctrl_d());
        let areas = stored(&rig).outputs[0].layers.top.areas.clone();
        assert_eq!(
            areas[1].id.as_str(),
            "bar-top-2",
            "right after the original"
        );
        let Some(AreaKind::Bar {
            edge,
            length,
            offset,
            ..
        }) = areas[1].kind
        else {
            panic!("a bar: {:?}", areas[1])
        };
        assert_eq!(edge, Some(Edge::Top));
        assert!(
            offset.is_some_and(|offset| offset >= 600.0),
            "past the original: {offset:?}"
        );
        assert_eq!(length, Some(Extent::Px(600.0)));
        assert_eq!(
            session::selected(),
            Selection::Area(Node::area(
                Some(SCREEN),
                LayerKind::Top,
                &AreaId::new("bar-top-2")
            ))
        );
    }

    /// A bar whose edge has no free stretch is refused.
    #[test]
    fn a_bar_on_a_full_edge_is_refused() {
        let _rig = rig_with("duplicate-bar-full", |_| {});
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        refused_with(Selection::Area(bar()), "no room left for another bar");
    }

    /// What there is one of, or what covers its layer between its kind, is refused with why, and its menu offers no copy.
    #[test]
    fn what_cannot_be_copied_is_refused_with_why() {
        let _rig = rig_with("duplicate-refused", |layout| {
            layout.outputs[0].layers.top.areas.push(Area {
                id: AreaId::new("dock"),
                kind: Some(AreaKind::Dock {
                    edge: Some(Edge::Bottom),
                    thickness: Some(48.0),
                }),
                ..Area::default()
            });
        });
        let _owner = owner();
        let area = |layer: LayerKind, id: &str| {
            Selection::Area(Node::area(Some(SCREEN), layer, &AreaId::new(id)))
        };

        let _mode = enter(LayerKind::Desktop);
        refused_with(area(LayerKind::Desktop, "widgets"), "not the whole grid");
        mode::leave();

        let _mode = enter(LayerKind::Background);
        refused_with(area(LayerKind::Background, "background"), "split one");
        mode::leave();

        let _mode = enter(LayerKind::Lock);
        refused_with(area(LayerKind::Lock, "prompt"), "password field");
        mode::leave();

        let _mode = enter(LayerKind::Top);
        refused_with(area(LayerKind::Top, "dock"), "one dock per edge");
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        crate::panel::give(&clock, crate::panel::Shape::Beside).expect("the clock gets a panel");
        refused_with(area(LayerKind::Top, "clock-panel"), "belongs to the widget");
        let dock = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("dock"));
        context::open(surfaces::menu::Asked {
            node: dock,
            window: LayerKind::Top,
            at: Some((900.0, 1000.0)),
        })
        .expect("the dock's menu opens");
        assert!(
            !context::rows().iter().any(|row| row == "Duplicate"),
            "{:?}",
            context::rows()
        );
    }

    /// The menu of a widget offers the copy, and picking it is the key's copy.
    #[test]
    fn the_menu_row_copies_as_the_key_does() {
        let rig = rig_with("duplicate-menu", on_the_grid);
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        let weather = grid().instance(&GroupId::new("weather"), &InstanceId::new("weather"));
        context::open(surfaces::menu::Asked {
            node: weather,
            window: LayerKind::Desktop,
            at: Some((900.0, 500.0)),
        })
        .expect("the menu opens");
        context::pick("Duplicate");
        assert_eq!(rig.undo_label().as_deref(), Some("Duplicate Weather"));
        assert_eq!(written_grid(&rig).groups.len(), 4);
    }

    /// A chip in a group the extended layout writes is not copied beside itself: a level laid over another never reorders what it inherits.
    #[test]
    fn a_chip_in_an_inherited_group_is_refused() {
        let _rig = rig_with("duplicate-inherited", |layout| {
            *layout = Layout {
                id: layout::LayoutId::new("mine"),
                extends: Some(layout::LayoutId::new(layout::BUILT_IN)),
                outputs: vec![layout::OutputRule {
                    matches: layout::OutputMatch("*".into()),
                    ..layout::OutputRule::default()
                }],
                ..Layout::default()
            };
        });
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        refused_with(
            Selection::Instance(bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))),
            "'center' is written by a layout this one extends",
        );
    }

    /// The entry `copy` writes, its id, geometry and children's ids put back to `original`'s: what a copy written as the original is must equal.
    fn as_original(copy: &Area, original: &Area) -> Area {
        Area {
            id: original.id.clone(),
            kind: original.kind.clone(),
            groups: copy
                .groups
                .iter()
                .zip(&original.groups)
                .map(|(group, was)| Group {
                    children: group
                        .children
                        .iter()
                        .zip(&was.children)
                        .map(|(child, was)| Instance {
                            id: was.id.clone(),
                            ..child.clone()
                        })
                        .collect(),
                    ..group.clone()
                })
                .collect(),
            ..copy.clone()
        }
    }

    /// A bar is copied as its entry writes it: nothing the file leaves to its default — where it is measured from, whether it reserves — is written into the copy.
    #[test]
    fn a_bar_is_copied_as_its_entry_writes_it() {
        let rig = rig_with("duplicate-bar-written", |layout| {
            if let Some(AreaKind::Bar { length, .. }) =
                &mut layout.outputs[0].layers.top.areas[0].kind
            {
                *length = Some(Extent::Px(600.0));
            }
        });
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        let original = stored(&rig).outputs[0].layers.top.areas[0].clone();
        select(bar());
        assert!(ctrl_d());
        let copy = stored(&rig).outputs[0].layers.top.areas[1].clone();
        assert_eq!(copy.id.as_str(), "bar-top-2");
        assert_eq!(copy.within, original.within, "{copy:?}");
        assert_eq!(copy.reserve, original.reserve, "{copy:?}");
        assert_ne!(
            ids(&copy.groups[1]),
            ids(&original.groups[1]),
            "ids of its own"
        );
        assert_eq!(as_original(&copy, &original), original);
    }

    /// A group in a bar's zone is copied right after itself in that zone, as its entry writes it, each chip in it under an id of its own; one undo takes it back.
    #[test]
    fn a_group_in_a_zone_is_copied_right_after_itself() {
        let rig = rig_with("duplicate-zone", |_| {});
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        let before = stored(&rig);
        select(bar().group(&GroupId::new("center")));
        assert!(ctrl_d(), "Ctrl+D answers");
        let groups = stored(&rig).outputs[0].layers.top.areas[0].groups.clone();
        let at = groups
            .iter()
            .position(|group| group.id.as_str() == "center")
            .expect("the original stays");
        let copy = &groups[at + 1];
        assert_eq!(copy.id.as_str(), "center-2", "right after the original");
        assert_eq!(copy.kind, groups[at].kind, "in the same zone");
        assert_eq!(copy.children.len(), 1);
        assert_ne!(copy.children[0].id, groups[at].children[0].id);
        assert_eq!(
            Instance {
                id: groups[at].children[0].id.clone(),
                ..copy.children[0].clone()
            },
            groups[at].children[0],
            "the chip as its entry writes it"
        );
        assert_eq!(
            session::selected(),
            Selection::Group(bar().group(&GroupId::new("center-2")))
        );
        assert_eq!(session::undo().as_deref(), Ok("Duplicate center"));
        assert_eq!(stored(&rig), before, "one undo takes it back");
    }

    /// A group of a free area, which places its groups in zones too, is copied right after itself.
    #[test]
    fn a_group_in_a_free_area_is_copied_right_after_itself() {
        let rig = rig_with("duplicate-free-zone", |_| {});
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        let centre = Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("centre"));
        select(centre.group(&GroupId::new("clock")));
        assert!(ctrl_d());
        let area = stored(&rig).outputs[0]
            .layers
            .desktop
            .areas
            .iter()
            .find(|area| area.id.as_str() == "centre")
            .cloned()
            .expect("the free area");
        let names: Vec<&str> = area.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(names, ["clock", "clock-2"], "a group id is its area's own");
        assert_eq!(area.groups[1].kind, area.groups[0].kind);
        assert_eq!(area.groups[1].children[0].module.as_deref(), Some("clock"));
    }

    /// A widget a komponent draws is refused, saying which komponent holds it and which group to copy instead.
    #[test]
    fn a_child_of_a_komponent_is_refused_with_where_it_is_written() {
        let pill = layout::Komponent {
            children: vec![widget("face", "clock")],
            ..layout::Komponent::default()
        };
        let _rig = rig_prepared(
            "duplicate-komponent-child",
            |layout| {
                on_the_grid(layout);
                layout.outputs[0].layers.desktop.areas[0]
                    .groups
                    .push(Group {
                        komponent: Some(KomponentId::new("pill")),
                        ..cell_group("pill", (0, 5, 2, 2))
                    });
            },
            |store| {
                store
                    .add_komponent(KomponentId::new("pill"), pill)
                    .expect("a komponent");
            },
        );
        let _owner = owner();
        let _mode = enter(LayerKind::Desktop);
        let child = InstanceId::in_komponent(
            &AreaId::new("widgets"),
            &GroupId::new("pill"),
            &InstanceId::new("face"),
        );
        refused_with(
            Selection::Instance(grid().instance(&GroupId::new("pill"), &child)),
            "drawn by the komponent pill",
        );
        refused_with(
            Selection::Instance(grid().instance(&GroupId::new("pill"), &child)),
            "duplicate the group pill",
        );
    }

    /// A copy of the owner of a panel is a chip of its own and the panel stays the original's; one undo takes the copy away and leaves the panel.
    #[test]
    fn a_copy_of_a_panels_owner_has_no_panel_of_its_own() {
        let rig = rig_with("dup-panel-owner", |_| {});
        let _owner = owner();
        let _mode = enter(LayerKind::Top);
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        crate::panel::give(&clock, crate::panel::Shape::Beside).expect("the panel is given");
        let given = stored(&rig);
        assert!(session::select(Selection::Instance(clock)));
        assert!(ctrl_d());

        let layer = &stored(&rig).outputs[0].layers.top;
        let panels: Vec<_> = layer
            .areas
            .iter()
            .filter_map(|area| area.kind.as_ref()?.owner().cloned())
            .collect();
        assert_eq!(
            panels,
            [InstanceId::new("clock")],
            "one panel, the original's"
        );
        let center = layer.areas[0]
            .groups
            .iter()
            .find(|group| group.id.as_str() == "center")
            .expect("the centre zone");
        assert_eq!(center.children.len(), 2);
        assert_ne!(center.children[0].id, center.children[1].id);
        crate::rig::undoes_to(&rig, &given);
    }
}
