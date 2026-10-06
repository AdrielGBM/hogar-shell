#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, Event, LayoutItem, LayoutStyle, PointerButton,
        PointerSource, Rect, compute_layout,
    };

    use layout::{
        Action, AreaId, AreaKind, Group, GroupId, GroupKind, LayerKind, Layout, ResolvedArea,
        Trigger,
    };
    use surfaces::area::Surround;
    use surfaces::menu::Pointed;
    use surfaces::reconcile;
    use surfaces::transient;
    use ui::host::Audience;

    use crate::context;
    use crate::mode;
    use crate::rig::{self, SCREEN, Scope, rig_with, stored};

    const SIZE: (f32, f32) = (1920.0, 1080.0);
    const EMPTY: (f32, f32) = (960.0, 40.0);

    thread_local! {
        static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn with_desktop_grid(layout: &mut Layout) {
        layout.outputs[0].layers.desktop.areas = vec![layout::Area {
            id: AreaId::new("widgets"),
            kind: Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.1,
                    w: 1.0,
                    h: 0.9,
                }),
                cell: Some(80.0),
                gap: Some(16.0),
                anchor: Some(layout::Anchor::TopLeft),
            }),
            groups: vec![Group {
                id: GroupId::new("board"),
                kind: Some(GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 4,
                    row_span: 2,
                }),
                ..Group::default()
            }],
            ..layout::Area::default()
        }];
    }

    fn wallpaper() -> ResolvedArea {
        reconcile::desktops()[0]
            .resolved
            .layer(LayerKind::Background)
            .and_then(|layer| {
                layer
                    .areas
                    .iter()
                    .find(|area| area.id.as_str() == "background")
            })
            .cloned()
            .expect("the wallpaper is on screen")
    }

    fn background_window() -> ComponentList {
        let desktop = reconcile::desktops()[0].clone();
        let config = Arc::clone(&desktop.config);
        let surround = Surround {
            config: &config,
            theme: config.resolve_theme(),
            output: Some(SCREEN),
            layer: LayerKind::Background,
            bounds: Rect::new(0.0, 0.0, SIZE.0, SIZE.1),
            reserved: desktop.reserved,
            audience: Audience::Owner,
        };
        let built = surfaces::area::build(&wallpaper(), surround)
            .expect("a wallpaper region has a builder")
            .expect("it builds");
        let root = Container::new(
            LayoutStyle::new().width(SIZE.0).height(SIZE.1),
            vec![Box::new(Pointed::new(built)) as Box<dyn LayoutItem>],
        )
        .expect("a page");
        let node = root.layout_node();
        let tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(SIZE.0),
            AvailableSpace::Definite(SIZE.1),
        )
        .expect("it lays out");
        tree
    }

    fn click(tree: &mut ComponentList, (x, y): (f32, f32), button: PointerButton) {
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
                button,
                source: PointerSource::Mouse,
            },
            Event::PointerReleased {
                x,
                y,
                button,
                source: PointerSource::Mouse,
            },
        ] {
            if !telar::dispatch_overlays(&event) {
                tree.on_event(&event);
            }
        }
    }

    fn strings(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    #[test]
    fn a_secondary_press_on_the_empty_background_opens_the_shell_menu_outside_a_mode() {
        let _rig = rig_with("shell-menu-outside", with_desktop_grid);
        let _scope = Scope::new();
        let mut window = background_window();

        click(&mut window, EMPTY, PointerButton::Secondary);

        assert_eq!(
            context::rows(),
            strings(&[
                "Add",
                "Customize grid…",
                "Theme…",
                "New layout from template…",
                "Edit",
                "Lock"
            ])
        );
        assert_eq!(
            context::sub_rows("Add"),
            strings(&["Add widget…", "New container"])
        );
        assert_eq!(
            context::sub_rows("Edit"),
            strings(&["Background", "Desktop", "Top", "Overlay", "Lock screen"])
        );
        assert!(mode::current().is_none(), "opening it enters no mode");
    }

    #[test]
    fn a_primary_press_on_the_background_does_nothing() {
        let rig = rig_with("shell-menu-primary", with_desktop_grid);
        let _scope = Scope::new();
        let before = stored(&rig);
        let mut window = background_window();

        click(&mut window, EMPTY, PointerButton::Primary);

        assert!(context::rows().is_empty(), "{:?}", context::rows());
        assert!(mode::current().is_none());
        assert_eq!(stored(&rig), before);
    }

    /// In a mode, "Add ▸" lists the layer's add tools and none of its other toolbar buttons — no "Try a card" on the overlay — and the menu ends with the mode's rows.
    #[test]
    fn in_a_mode_the_shell_menu_adds_with_the_layers_tools_and_ends_with_the_modes_rows() {
        let _rig = rig_with("shell-menu-mode", with_desktop_grid);
        let _scope = Scope::new();
        let mut window = background_window();
        let cases: [(LayerKind, &[&str], &[&str]); 3] = [
            (
                LayerKind::Desktop,
                &["Add widget…", "New container", "New grid"],
                &[],
            ),
            (
                LayerKind::Top,
                &["New group on a plate", "New bar at the bottom"],
                &["New container"],
            ),
            (LayerKind::Overlay, &["New stack"], &["Try a card"]),
        ];
        for (layer, adds, absent) in cases {
            rig::open_mode(layer);
            click(&mut window, EMPTY, PointerButton::Secondary);

            let rows = context::rows();
            for row in [
                "Add",
                "Edit another layer",
                "Undo",
                "Redo",
                "Theme…",
                "Keys",
                "Done",
            ] {
                assert!(rows.contains(&row.to_string()), "{layer}: {rows:?}");
            }
            assert!(!rows.contains(&"Lock".to_string()), "{layer}: {rows:?}");
            let added = context::sub_rows("Add");
            for add in adds {
                assert!(added.contains(&add.to_string()), "{layer}: {added:?}");
            }
            for left_out in absent {
                assert!(!added.contains(&left_out.to_string()), "{layer}: {added:?}");
            }
            let mut unique = added.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), added.len(), "{layer}: {added:?}");
            assert!(
                !context::sub_rows("Edit another layer").contains(&mode::name_of(layer)),
                "{layer}"
            );
            transient::close(context::ID);
            mode::leave();
        }
    }

    #[test]
    fn a_mode_is_entered_from_the_shell_menu() {
        let _rig = rig_with("shell-menu-enter", with_desktop_grid);
        let _scope = Scope::new();
        let mut window = background_window();

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Top");
        assert_eq!(mode::current().map(|mode| mode.layer), Some(LayerKind::Top));

        mode::leave();
        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Customize grid…");
        assert_eq!(
            mode::current().map(|mode| mode.layer),
            Some(LayerKind::Desktop)
        );
        assert!(
            crate::popover::tree().is_some(),
            "the grid's popover is open"
        );
    }

    #[test]
    fn a_bound_secondary_action_wins_over_the_shell_menu() {
        let _rig = rig_with("shell-menu-bound", |layout| {
            with_desktop_grid(layout);
            layout.outputs[0].layers.background.areas[0].actions =
                BTreeMap::from([(Trigger::Secondary, Action(vec!["probe secondary".into()]))]);
        });
        let _scope = Scope::new();
        RAN.with(|ran| ran.borrow_mut().clear());
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
        let mut window = background_window();

        click(&mut window, EMPTY, PointerButton::Secondary);

        assert_eq!(RAN.with(|ran| ran.borrow().clone()), ["probe secondary"]);
        assert!(context::rows().is_empty(), "{:?}", context::rows());
    }

    /// A layer's add tools are the buttons of its toolbar that add to it, never the others: the lock's privacy and the overlay's sample cards are on their toolbars and not among them.
    #[test]
    fn the_add_tools_are_the_toolbar_buttons_that_add() {
        let _rig = rig_with("shell-menu-add-tools", with_desktop_grid);
        telar::set_locale("en");
        let said = |buttons: Vec<crate::host::StripButton>| -> Vec<String> {
            buttons.iter().map(|(label, _)| label()).collect()
        };
        for (layer, other) in [
            (LayerKind::Lock, "Privacy…"),
            (LayerKind::Overlay, "Try a card"),
        ] {
            let adds = said(crate::host::add_tools_of(layer));
            assert!(
                said(crate::host::toolbar_of(layer)).contains(&other.to_string()),
                "{layer}"
            );
            assert!(!adds.contains(&other.to_string()), "{layer}: {adds:?}");
            assert!(!adds.is_empty(), "{layer}");
        }
    }

    /// What the shell menu's rows do, outside a mode and in one: "New container" enters the desktop mode and makes one as a single undo entry, "Add widget…" opens the palette, "Theme…" opens the theme popover, "Undo" takes the container back and "Done" ends the mode.
    #[test]
    fn the_shell_menus_rows_enter_the_mode_and_do_what_they_say() {
        let rig = rig_with("shell-menu-rows", with_desktop_grid);
        let _scope = Scope::new();
        let mut window = background_window();
        let before = stored(&rig);

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("New container");
        assert_eq!(
            mode::current().map(|mode| mode.layer),
            Some(LayerKind::Desktop)
        );
        let groups = |layout: &Layout| layout.outputs[0].layers.desktop.areas[0].groups.len();
        assert_eq!(groups(&stored(&rig)), groups(&before) + 1);
        assert!(
            rig.undo_label()
                .is_some_and(|label| label.starts_with("Make the container")),
            "{:?}",
            rig.undo_label()
        );

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Undo");
        assert_eq!(
            stored(&rig),
            before,
            "one entry made it and one undo takes it back"
        );

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Add widget…");
        assert!(transient::is_open(crate::modes::palette::ID));
        transient::close(crate::modes::palette::ID);

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Theme…");
        assert!(transient::is_open(crate::theme::ID));
        transient::close(crate::theme::ID);

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Done");
        assert!(mode::current().is_none(), "Done ends the mode");
    }

    /// Outside a mode "Theme…" enters one first, because the popover is the mode's.
    #[test]
    fn the_theme_row_outside_a_mode_enters_the_mode_and_opens_the_popover() {
        let _rig = rig_with("shell-menu-theme", with_desktop_grid);
        let _scope = Scope::new();
        let mut window = background_window();

        click(&mut window, EMPTY, PointerButton::Secondary);
        context::pick("Theme…");
        assert!(mode::current().is_some());
        assert!(transient::is_open(crate::theme::ID));
        transient::close(crate::theme::ID);
    }
}
