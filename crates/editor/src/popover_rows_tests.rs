//! The look, structure and actions rows of the area, group and instance popovers.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use telar::{Event, Key, ModifiersState, Rect, signal};

    use layout::{
        Area, AreaId, AreaKind, Arrange, Border, Corners, GroupId, GroupKind, InstanceId,
        LayerKind, ResolvedArea, ResolvedGroup, ResolvedInstance, Sides, Trigger,
    };
    use surfaces::actions::Bound;
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use ui::host::Audience;

    use crate::popover::{self, Provenance};
    use crate::rig::{Card, Rig, SCREEN, Scope, bar, face, rig_with};
    use crate::session;

    fn widgets() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    /// A grid over the whole screen holding the built-in clock on its first cell and, eight columns on, a copy of it inside a row container.
    fn grid_rig(test: &str) -> Rig {
        rig_with(test, |layout| {
            let areas = &mut layout.outputs[0].layers.desktop.areas;
            let centre: Area = areas.pop().expect("the clock's own area");
            let mut clock = centre.groups.into_iter().next().expect("its group");
            clock.kind = Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            });
            let mut shelf = clock.clone();
            shelf.id = GroupId::new("shelf");
            shelf.kind = Some(GroupKind::Cell {
                col: 8,
                row: 0,
                col_span: 4,
                row_span: 2,
            });
            shelf.arrange = Some(Arrange::Row);
            for child in &mut shelf.children {
                child.id = InstanceId::new("shelf-clock");
            }
            areas[0].kind = Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                }),
                cell: None,
                gap: None,
                anchor: Some(layout::Anchor::TopLeft),
            });
            areas[0].groups.extend([clock, shelf]);
        })
    }

    fn place_grid() {
        rects::track_spanning(widgets(), vec![signal(Rect::new(0.0, 0.0, 1920.0, 1080.0))]);
    }

    fn area_on_screen(node: &Node) -> ResolvedArea {
        reconcile::desktops()[0]
            .resolved
            .area(node.layer, &node.area)
            .cloned()
            .expect("the area is drawn")
    }

    fn group_on_screen(id: &str) -> Option<ResolvedGroup> {
        area_on_screen(&widgets())
            .groups
            .into_iter()
            .find(|group| group.id.as_str() == id)
    }

    fn shelf_clock() -> ResolvedInstance {
        group_on_screen("shelf")
            .expect("the shelf")
            .children
            .into_iter()
            .next()
            .expect("its clock")
    }

    fn value<T: 'static>(name: &str) -> telar::RwSignal<T> {
        popover::shared::<T>(name).unwrap_or_else(|| panic!("the popover edits {name}"))
    }

    #[test]
    fn an_areas_look_rows_pad_round_border_and_lift_it_live() {
        let rig = grid_rig("rows-area-look");
        let _scope = Scope::new();
        place_grid();
        popover::open_area(widgets()).expect("the grid's popover opens");
        let card = Card::open();
        assert!(
            card.texts()
                .iter()
                .any(|(text, _)| text.ends_with("cells on this screen")),
            "the grid says how many cells it has room for"
        );

        value::<f32>("style.padding").set(20.0);
        value::<f32>("style.padding.left").set(32.0);
        assert_eq!(
            area_on_screen(&widgets()).style.padding,
            Some(Sides::each(20.0, 20.0, 20.0, 32.0))
        );
        assert_eq!(
            value::<f32>("style.padding").peek(),
            32.0,
            "all reads the widest"
        );

        value::<f32>("style.radius.top_right").set(12.0);
        assert_eq!(
            area_on_screen(&widgets()).style.radius,
            Some(Corners::each(0.0, 12.0, 0.0, 0.0))
        );
        value::<f32>("style.border.width").set(2.0);
        value::<String>("style.border.color").set("accent".into());
        value::<String>("style.shadow").set("2".into());
        let style = area_on_screen(&widgets()).style;
        assert_eq!(
            style.border,
            Some(Border {
                width: Some(2.0),
                color: Some("accent".into())
            })
        );
        assert_eq!(style.shadow, Some(2));
        let draft = popover::area_draft().expect("an area's popover");
        for key in [
            "style.padding",
            "style.radius",
            "style.border.width",
            "style.shadow",
        ] {
            assert_eq!(draft.provenance(&[key]), Provenance::Here, "{key}");
        }

        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize widgets"));
        assert_eq!(session::undo().as_deref(), Ok("Customize widgets"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(area_on_screen(&widgets()).style.border, None);

        popover::open_area(widgets()).expect("it opens again");
        let mut card = Card::open();
        let before = reconcile::desktops();
        value::<f32>("style.radius").set(24.0);
        assert_eq!(
            area_on_screen(&widgets()).style.radius,
            Some(Corners::all(24.0))
        );
        card.escape();
        assert!(
            std::rc::Rc::ptr_eq(&reconcile::desktops(), &before),
            "Esc puts back the very arrangement that was on screen"
        );
        assert_eq!(rig.undo_label(), None, "and records nothing");
    }

    #[test]
    fn resetting_the_corners_puts_every_corner_row_back() {
        let _rig = grid_rig("rows-area-reset");
        let _scope = Scope::new();
        place_grid();
        popover::open_area(widgets()).expect("the grid's popover opens");
        let _card = Card::open();
        value::<f32>("style.radius.bottom_left").set(9.0);
        let draft = popover::area_draft().expect("an area's popover");
        assert!(draft.writes(&["style.radius"]));
        telar::batch(|| draft.reset(&["style.radius"]));
        assert_eq!(value::<f32>("style.radius.bottom_left").peek(), 0.0);
        assert_eq!(value::<f32>("style.radius").peek(), 0.0);
        assert!(!draft.writes(&["style.radius"]), "nothing is pinned here");
        assert_eq!(area_on_screen(&widgets()).style.radius, None);
        popover::close();
    }

    #[test]
    fn a_groups_popover_arranges_paints_spans_and_removes_it() {
        let rig = grid_rig("rows-group");
        let _scope = Scope::new();
        place_grid();
        let shelf = widgets().group(&GroupId::new("shelf"));
        popover::open_for(&session::Selection::Group(shelf.clone()))
            .expect("the group's popover opens");
        assert_eq!(popover::current(), Some(shelf));
        let card = Card::open();
        assert!(card.shows("Arrangement"));
        assert!(
            popover::shared::<f32>("cols").is_none(),
            "a row has no inner columns"
        );

        value::<String>("arrange").set("grid".into());
        card.lay_out();
        value::<f32>("cols").set(3.0);
        value::<f32>("rows").set(1.0);
        value::<f32>("gap").set(12.0);
        value::<String>("style.fill").set("red".into());
        value::<f32>("style.padding.top").set(10.0);
        value::<f32>("col_span").set(6.0);
        let group = group_on_screen("shelf").expect("the shelf");
        assert_eq!(group.arrange, Some(Arrange::Grid));
        assert_eq!((group.cols, group.rows, group.gap), (3, 1, Some(12.0)));
        assert_eq!(group.style.fill.as_deref(), Some("red"));
        let plate = ui::scale::plate::padding(false);
        assert_eq!(
            group.style.padding,
            Some(Sides::each(10.0, plate, plate, plate)),
            "the sides left alone are written as drawn"
        );
        assert!(matches!(
            group.kind,
            GroupKind::Cell {
                col: 8,
                col_span: 6,
                row_span: 2,
                ..
            }
        ));
        let draft = popover::group_draft().expect("a group's popover");
        assert_eq!(draft.provenance(&["arrange"]), Provenance::Here);
        assert_eq!(draft.provenance(&["style.fill"]), Provenance::Here);
        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize shelf"));
        assert_eq!(session::undo().as_deref(), Ok("Customize shelf"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(
            group_on_screen("shelf").expect("the shelf").arrange,
            Some(Arrange::Row)
        );

        popover::open_group(widgets().group(&GroupId::new("shelf"))).expect("it opens again");
        let mut card = Card::open();
        value::<String>("style.fill").set("red".into());
        card.press("Remove shelf");
        assert!(group_on_screen("shelf").is_none(), "the group is gone");
        assert_eq!(popover::current(), None, "and its popover with it");
        assert_eq!(rig.undo_label().as_deref(), Some("Remove shelf"));
        assert_eq!(session::undo().as_deref(), Ok("Remove shelf"));
        assert_eq!(
            rig.undo_label(),
            None,
            "what the popover previewed was put back first"
        );
        assert_eq!(
            group_on_screen("shelf").expect("back").style.fill,
            None,
            "nothing it previewed was kept"
        );
    }

    #[test]
    fn an_instances_look_rows_and_reset_clear_its_options_and_style() {
        let rig = grid_rig("rows-instance");
        let _scope = Scope::new();
        place_grid();
        let clock = widgets().instance(&GroupId::new("shelf"), &InstanceId::new("shelf-clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let mut card = Card::open();
        assert!(
            card.shows("Inside the container shelf: its size is the container's to give"),
            "{:?}",
            card.texts()
        );

        value::<String>("style.fill").set("red".into());
        value::<f32>("style.radius.top_left").set(6.0);
        value::<f32>("style.border.width").set(1.0);
        value::<String>("style.shadow").set("3".into());
        let style = shelf_clock().style;
        assert_eq!(style.fill.as_deref(), Some("red"));
        let themed = reconcile::desktops()[0]
            .config
            .shape_from(None, None, None, None)
            .radius;
        assert_eq!(
            style.radius,
            Some(Corners::each(6.0, themed, themed, themed))
        );
        assert_eq!(style.border.map(|border| border.width()), Some(1.0));
        assert_eq!(style.shadow, Some(3));
        let draft = popover::instance_draft().expect("an instance's popover");
        assert_eq!(draft.provenance(&["style.fill"]), Provenance::Here);
        draft.set_option(&popover::path_of("show_date"), toml::Value::Boolean(true));
        assert!(!draft.instance().peek().options.is_empty());

        card.press("Reset options and look");
        let held = draft.instance().peek();
        assert!(held.options.is_empty(), "its options are off");
        assert!(held.style.is_empty(), "and its style");
        assert!(shelf_clock().style.is_empty(), "on the real clock too");
        assert_eq!(
            value::<String>("style.fill").peek(),
            "",
            "the rows read it again"
        );
        assert_eq!(draft.provenance(&["style.fill"]), Provenance::Default);

        value::<f32>("style.opacity").set(0.5);
        assert_eq!(
            shelf_clock().style.opacity,
            Some(0.5),
            "a row built again still writes"
        );
        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock"));
        assert_eq!(session::undo().as_deref(), Ok("Customize clock"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
    }

    thread_local! {
        static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn recording() {
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
    }

    fn pressed(node: &Node) -> Vec<String> {
        if let Some(run) =
            Bound::of(&area_on_screen(node).actions, Audience::Owner).runs(Trigger::Press)
        {
            run();
        }
        RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    #[test]
    fn a_press_bound_in_the_popover_runs_and_escape_takes_it_back() {
        let rig = rig_with("rows-actions", |_| {});
        let _scope = Scope::new();
        recording();
        rects::track_spanning(bar(), vec![signal(Rect::new(0.0, 0.0, 1920.0, 34.0))]);
        let before = reconcile::desktops();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        assert!(card.shows("An area receives the gestures that land between its widgets"));

        card.press("Add action");
        let actions = popover::area_draft().expect("an area's popover").actions();
        assert_eq!(actions.bound(), [Trigger::Press]);
        actions
            .set(Trigger::Press, "launcher toggle ; notifications center")
            .expect("ordinary lines");
        assert_eq!(
            pressed(&bar()),
            ["launcher toggle", "notifications center"],
            "the press runs on the real bar"
        );
        assert_eq!(actions.provenance(Trigger::Press), Provenance::Here);

        let refused = actions
            .set(Trigger::Press, "layout trust nord --all 0123456789abcdef")
            .expect_err("trust is the user's to give");
        assert!(refused.contains("trust is yours to give"), "{refused}");
        assert_eq!(
            actions.text(Trigger::Press),
            "launcher toggle; notifications center"
        );

        let second = actions.add().expect("a free gesture");
        assert_eq!(second, Trigger::LongPress);
        let taken = actions
            .retrigger(second, Trigger::Press)
            .expect_err("the press has its action");
        assert!(taken.contains("already has its action"), "{taken}");
        actions
            .retrigger(second, Trigger::Middle)
            .expect("the middle press is free");
        actions
            .set(Trigger::Middle, "launcher toggle")
            .expect("edited");
        assert_eq!(actions.bound(), [Trigger::Press, Trigger::Middle]);
        actions.remove(Trigger::Middle);
        assert_eq!(actions.bound(), [Trigger::Press]);

        card.escape();
        assert!(
            std::rc::Rc::ptr_eq(&reconcile::desktops(), &before),
            "Esc puts back the very arrangement that was on screen"
        );
        assert!(area_on_screen(&bar()).actions.is_empty());
        assert!(pressed(&bar()).is_empty(), "the press runs nothing again");
        assert_eq!(rig.undo_label(), None, "and records nothing");
    }

    /// The clock on the bar as the screen draws it now.
    fn drawn_clock() -> ResolvedInstance {
        area_on_screen(&bar())
            .groups
            .into_iter()
            .find(|group| group.id.as_str() == "center")
            .and_then(|group| {
                group
                    .children
                    .into_iter()
                    .find(|child| child.id.as_str() == "clock")
            })
            .expect("the clock")
    }

    /// Adding an action gives the gesture a row and writes nothing, so whatever answers the gesture now keeps answering it; a chain emptied again takes the binding off and keeps the row to type into.
    #[test]
    fn an_added_action_is_written_only_once_its_chain_runs_something() {
        let rig = rig_with("rows-actions-unwritten", |_| {});
        let _scope = Scope::new();
        recording();
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let mut card = Card::open();
        let actions = popover::instance_draft()
            .expect("an instance's popover")
            .actions();

        card.press("Add action");
        assert_eq!(actions.bound(), [Trigger::Press], "the row is there");
        assert!(!actions.writes(Trigger::Press));
        assert!(
            drawn_clock().actions.is_empty(),
            "nothing is bound to the press, so the clock's own press still answers it"
        );

        actions
            .set(Trigger::Press, "launcher toggle")
            .expect("an ordinary line");
        assert_eq!(
            drawn_clock()
                .actions
                .get(&Trigger::Press)
                .map(|action| action.0.clone()),
            Some(vec!["launcher toggle".to_string()])
        );

        actions.set(Trigger::Press, " ; ").expect("an empty chain");
        assert!(
            drawn_clock().actions.is_empty(),
            "an empty chain takes the binding off"
        );
        assert_eq!(actions.bound(), [Trigger::Press], "and keeps its row");

        let moved = actions.add().expect("a free gesture");
        actions
            .retrigger(moved, Trigger::Middle)
            .expect("the middle press is free");
        assert!(
            drawn_clock().actions.is_empty(),
            "an empty row moved to another gesture is still written nowhere"
        );
        assert_eq!(actions.bound(), [Trigger::Press, Trigger::Middle]);

        popover::close();
        assert_eq!(
            rig.undo_label(),
            None,
            "a popover that bound nothing records nothing"
        );
    }

    #[test]
    fn an_instances_action_is_its_own_and_one_entry() {
        let rig = rig_with("rows-instance-actions", |_| {});
        let _scope = Scope::new();
        recording();
        let clock = bar().instance(&GroupId::new("center"), &InstanceId::new("clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let _card = Card::open();
        let actions = popover::instance_draft()
            .expect("an instance's popover")
            .actions();
        let trigger = actions.add().expect("a free gesture");
        actions
            .set(trigger, "launcher toggle")
            .expect("an ordinary line");
        let drawn = area_on_screen(&bar())
            .groups
            .into_iter()
            .find(|group| group.id.as_str() == "center")
            .and_then(|group| {
                group
                    .children
                    .into_iter()
                    .find(|child| child.id.as_str() == "clock")
            })
            .expect("the clock");
        assert_eq!(
            drawn.actions.get(&trigger).map(|action| action.0.clone()),
            Some(vec!["launcher toggle".to_string()])
        );
        assert!(
            area_on_screen(&bar()).actions.is_empty(),
            "the bar's own gestures are untouched"
        );
        popover::close();
        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock"));
    }

    /// Where a group, an instance or a bar writes no radius or padding, its rows read what the screen draws and are marked default, and nothing is written until one is changed.
    #[test]
    fn rows_read_the_radius_and_padding_drawn_where_none_is_written() {
        let rig = grid_rig("rows-drawn-defaults");
        let _scope = Scope::new();
        place_grid();
        let shelf = widgets().group(&GroupId::new("shelf"));
        popover::open_for(&session::Selection::Group(shelf)).expect("the group's popover opens");
        let _card = Card::open();
        let (radius, padding) = (
            ui::scale::plate::radius(false),
            ui::scale::plate::padding(false),
        );
        assert!(padding > 0.0, "the plate is drawn padded");
        assert_eq!(value::<f32>("style.radius").peek(), radius);
        assert_eq!(value::<f32>("style.radius.top_left").peek(), radius);
        assert_eq!(value::<f32>("style.padding").peek(), padding);
        assert_eq!(value::<f32>("style.padding.left").peek(), padding);
        let draft = popover::group_draft().expect("a group's popover");
        assert_eq!(draft.provenance(&["style.radius"]), Provenance::Default);
        assert_eq!(draft.provenance(&["style.padding"]), Provenance::Default);
        popover::close();
        assert_eq!(rig.undo_label(), None, "reading writes nothing");
        assert!(
            group_on_screen("shelf")
                .expect("the shelf")
                .style
                .padding
                .is_none()
        );

        let clock = widgets().instance(&GroupId::new("shelf"), &InstanceId::new("shelf-clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let _card = Card::open();
        let themed = reconcile::desktops()[0]
            .config
            .shape_from(None, None, None, None)
            .radius;
        assert_eq!(value::<f32>("style.radius").peek(), themed);
        let draft = popover::instance_draft().expect("an instance's popover");
        assert_eq!(draft.provenance(&["style.radius"]), Provenance::Default);
        popover::close();
        assert_eq!(rig.undo_label(), None);

        popover::open_area(bar()).expect("the bar's popover opens");
        let _card = Card::open();
        let drawn = crate::tools::target::padding_of(&reconcile::desktops()[0], &bar())
            .expect("the bar is drawn");
        assert_eq!(value::<f32>("style.padding").peek(), drawn[0]);
        popover::close();
        assert_eq!(rig.undo_label(), None);
    }

    static CLOCK: ui::descriptor::ModuleDescriptor = ui::descriptor::ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "circle",
        category: ui::descriptor::Category::Info,
        options: &[],
        representations: ui::descriptor::Representations {
            widget: Some(ui::descriptor::WidgetDef {
                sizes: &ui::host::WidgetSize::ALL,
                build: face,
                input: ui::descriptor::Input::ReadOnly,
            }),
            ..ui::descriptor::Representations::NONE
        },
        actions: &[],
        sources: &[],
    };

    /// A widget inside a container that arranges its children has no size row, since the container decides its size; one on a grid's cells keeps it.
    #[test]
    fn a_widget_in_a_container_has_no_size_row() {
        ui::descriptor::install(std::slice::from_ref(&CLOCK));
        let _rig = grid_rig("rows-size-in-container");
        let _scope = Scope::new();
        let _mode = crate::rig::enter(LayerKind::Desktop);
        place_grid();
        let loose = group_on_screen("clock").expect("the clock's cell");
        let child = loose.children.first().expect("its clock").id.clone();
        popover::open_instance(widgets().instance(&loose.id, &child)).expect("it opens");
        let card = Card::open();
        assert!(card.shows("Size"), "{:?}", card.texts());
        popover::close();

        let held = widgets().instance(&GroupId::new("shelf"), &InstanceId::new("shelf-clock"));
        popover::open_instance(held).expect("it opens");
        let card = Card::open();
        assert!(!card.shows("Size"), "{:?}", card.texts());
        popover::close();
    }

    fn contrast_line(card: &Card) -> Option<String> {
        card.lay_out();
        card.texts()
            .into_iter()
            .map(|(text, _)| text)
            .find(|text| text.starts_with("Text is "))
    }

    /// A fill under text says how readable the theme's text is on it, live, and warns under AA without refusing anything; with no fill there is nothing to say.
    #[test]
    fn a_fill_under_text_shows_its_contrast_and_warns_below_aa() {
        let _rig = grid_rig("rows-fill-contrast");
        let _scope = Scope::new();
        place_grid();
        let held = widgets().instance(&GroupId::new("shelf"), &InstanceId::new("shelf-clock"));
        let shelf = widgets().group(&GroupId::new("shelf"));
        for open in [
            Box::new(|| popover::open_area(widgets())) as Box<dyn Fn() -> _>,
            Box::new(move || popover::open_for(&session::Selection::Group(shelf.clone()))),
            Box::new(move || popover::open_instance(held.clone())),
        ] {
            open().expect("the popover opens");
            let card = Card::open();
            assert_eq!(contrast_line(&card), None, "no fill, nothing to judge");

            value::<String>("style.fill").set("yellow".to_string());
            let line = contrast_line(&card).expect("a fill is judged");
            assert!(line.contains("WCAG AA"), "yellow is warned about: {line}");

            value::<String>("style.fill").set("base".to_string());
            let line = contrast_line(&card).expect("a fill is judged");
            assert!(!line.contains("WCAG AA"), "a dark fill is not: {line}");

            value::<String>("style.fill").set(String::new());
            assert_eq!(contrast_line(&card), None);
            popover::close();
        }
    }

    fn dock_styled(style: layout::Style) -> std::rc::Rc<layout::ResolvedArea> {
        std::rc::Rc::new(layout::ResolvedArea {
            id: layout::AreaId::new("dock"),
            kind: layout::ResolvedAreaKind::Dock {
                edge: config::Edge::Bottom,
                thickness: 40.0,
            },
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Output,
            style,
            visible: None,
            groups: Vec::new(),
            actions: Default::default(),
        })
    }

    fn filled(fill: &str, opacity: Option<f32>) -> layout::Style {
        layout::Style {
            fill: Some(fill.to_string()),
            opacity,
            ..layout::Style::default()
        }
    }

    /// A fill is judged as its surface draws it: an opacity takes the place of the alpha its colour names, a group's plate rests at the plate's opacity, and an instance is laid over its group's plate where the group draws one.
    #[test]
    fn a_fill_is_judged_as_its_surface_draws_it() {
        use crate::popover::look::{Backing, Held};
        use layout::{color_of, laid_over, text_contrast};

        let config = std::sync::Arc::new(config::Config::default());
        let theme = config.resolve_theme();
        let backing = |held| {
            Backing::new(
                std::sync::Arc::clone(&config),
                dock_styled(layout::Style::default()),
                held,
            )
        };

        let area = backing(Held::Area);
        let opaque = area
            .ratio(&filled("#ebcb8b80", Some(1.0)), &theme)
            .expect("a fill is judged");
        assert_eq!(
            opaque,
            text_contrast(color_of("#ebcb8b", &theme), theme.base, &theme),
            "an opacity of 1 is opaque, whatever alpha the colour names"
        );
        assert_eq!(
            area.ratio(&filled("#ebcb8b80", None), &theme),
            Some(text_contrast(
                color_of("#ebcb8b80", &theme),
                theme.base,
                &theme
            )),
            "with no opacity the colour's own alpha shows"
        );

        let group = backing(Held::Group);
        let plate = ui::scale::plate::OPACITY;
        assert_eq!(
            group.ratio(&filled("yellow", None), &theme),
            Some(text_contrast(
                color_of("yellow", &theme).with_alpha(plate),
                theme.base,
                &theme
            )),
            "a plate names no opacity and rests at the plate's"
        );
        assert_eq!(group.resting_opacity(Some("yellow"), &theme), plate);
        assert_eq!(backing(Held::Area).resting_opacity(None, &theme), 1.0);

        let style = filled("yellow", Some(0.4));
        let on_plate = backing(Held::Instance {
            plate: Some(filled("red", Some(0.5))),
        });
        let under = laid_over(color_of("red", &theme).with_alpha(0.5), theme.base);
        assert_eq!(
            on_plate.ratio(&style, &theme),
            Some(text_contrast(
                color_of("yellow", &theme).with_alpha(0.4),
                under,
                &theme
            )),
            "an instance is laid over its group's plate"
        );
        assert_ne!(
            on_plate.ratio(&style, &theme),
            backing(Held::Instance { plate: None }).ratio(&style, &theme)
        );

        let prompt = layout::Style::default();
        assert_eq!(
            layout::prompt_contrast(&prompt, &theme),
            text_contrast(layout::prompt_card(&prompt, &theme), theme.base, &theme),
            "the lock and validation judge the prompt by one helper"
        );
    }

    fn type_into(card: &mut Card, typed: &str) {
        let ctrl = ModifiersState {
            is_ctrl: true,
            ..ModifiersState::default()
        };
        card.route(&Event::KeyPressed {
            key: Key::Char('a'),
            modifiers: ctrl,
        });
        for ch in typed.chars() {
            card.route(&Event::KeyPressed {
                key: Key::Char(ch),
                modifiers: ModifiersState::default(),
            });
        }
    }

    /// The fill's colour field takes a theme token or a hex typed into it, writes each as it is typed, ignores what is neither, and Esc puts the whole popover back.
    #[test]
    fn the_colour_field_is_typed_into_and_writes_a_token_or_a_hex() {
        let rig = grid_rig("rows-colour-typed");
        let _scope = Scope::new();
        place_grid();
        let shelf = widgets().group(&GroupId::new("shelf"));
        popover::open_for(&session::Selection::Group(shelf)).expect("the group's popover opens");
        let mut card = Card::open();
        let fill = || group_on_screen("shelf").expect("the shelf").style.fill;

        card.press("token or #hex");
        type_into(&mut card, "base");
        assert_eq!(fill().as_deref(), Some("base"), "a token is written");
        type_into(&mut card, "#ff8800");
        assert_eq!(fill().as_deref(), Some("#ff8800"), "so is a hex");
        type_into(&mut card, "not a colour");
        assert_eq!(
            fill().as_deref(),
            Some("#ff8800"),
            "what is neither is not written over it"
        );
        card.escape();
        assert_eq!(
            fill().as_deref(),
            Some("#ff8800"),
            "the first Esc leaves the field"
        );
        card.escape();
        assert_eq!(fill(), None, "the next puts the popover back");
        assert_eq!(rig.undo_label(), None);
    }
}
