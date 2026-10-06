//! The desktop mode's tools: widgets added from the palette, dropped, stacked, detached and resized on grids without ever taking one off the screen, a chip turned into a widget and back as the same instance, edits for one workspace alone, and areas an extended level places taken away.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use telar::{
        AvailableSpace, ComponentList, Container, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, PointerButton, PointerSource, compute_layout,
    };
    use telar::{RectStyle, StyledContainer};

    use layout::{
        ActiveWorkspace, Area, AreaId, AreaKind, Arrange, GroupId, GroupKind, InstanceId,
        LayerKind, Layout, LayoutId, OutputMatch, OutputRule, Representation, ResolvedArea,
        ResolvedGroup, WorkspaceMatch,
    };
    use surfaces::menu::{Asked, Pointed};
    use surfaces::rects::Node;
    use surfaces::transient;
    use ui::descriptor::{
        Built, Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef,
    };
    use ui::host::{Host, WidgetSize};

    use crate::keys::{self, Press};
    use crate::mode::{self};
    use crate::modes::desktop::{self, Landing};
    use crate::modes::grid::{self, Cells, Room};
    use crate::modes::palette::{self, Line, Pick};
    use crate::modes::widgets;
    use crate::rig::{Rig, SCREEN, enter};
    use crate::session::{self, Selection};
    use crate::{context, popover, variant};

    fn face(_: &Host) -> Built {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(40.0).height(20.0),
            |_| RectStyle::default(),
            Vec::new(),
        )?))
    }

    const fn module(
        id: &'static str,
        name: &'static str,
        category: Category,
        chip: bool,
        sizes: &'static [WidgetSize],
        input: Input,
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
                widget: Some(WidgetDef {
                    sizes,
                    build: face,
                    input,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    static PROBES: &[ModuleDescriptor] = &[
        module(
            "clock",
            "Clock",
            Category::Time,
            true,
            &WidgetSize::ALL,
            Input::ReadOnly,
        ),
        module(
            "weather",
            "Weather",
            Category::Info,
            false,
            &[WidgetSize::S, WidgetSize::M],
            Input::ReadOnly,
        ),
        module(
            "mixer",
            "Mixer",
            Category::Media,
            false,
            &[WidgetSize::M],
            Input::Interactive,
        ),
    ];

    /// An owner for what a test builds, disposed when it ends.
    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            mode::leave();
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

    fn tap(key: Key, modifiers: ModifiersState) -> bool {
        telar::observe_keyboard(&Event::KeyPressed {
            key: key.clone(),
            modifiers,
        });
        let taken = telar::dispatch_overlays(&Event::KeyPressed {
            key: key.clone(),
            modifiers,
        }) || keys::press_as(&key, modifiers, Press::First);
        telar::observe_keyboard(&Event::KeyReleased {
            key: key.clone(),
            modifiers,
        });
        keys::settle_released();
        taken
    }

    fn stored(rig: &Rig) -> Layout {
        rig.store.borrow().active().clone()
    }

    /// The built-in layout with its clock on the first cells of the grid, which is what these tests move, stack and resize.
    fn clock_on_grid(layout: &mut Layout) {
        let areas = &mut layout.outputs[0].layers.desktop.areas;
        let centre = areas.pop().expect("the clock's own area");
        let mut clock = centre.groups.into_iter().next().expect("its group");
        clock.kind = Some(GroupKind::Cell {
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
        });
        areas[0].groups.push(clock);
    }

    fn rig_with(test: &str, edit: impl FnOnce(&mut Layout)) -> Rig {
        rig_on(test, None, edit)
    }

    fn rig_on(test: &str, workspace: Option<&str>, edit: impl FnOnce(&mut Layout)) -> Rig {
        crate::rig::rig_on(test, workspace, |layout| {
            clock_on_grid(layout);
            edit(layout);
        })
    }

    fn widgets_area() -> AreaId {
        AreaId::new("widgets")
    }

    fn shown_area(layer: LayerKind, id: &AreaId) -> Option<ResolvedArea> {
        surfaces::reconcile::desktops()[0]
            .resolved
            .layer(layer)?
            .areas
            .iter()
            .find(|area| area.id == *id)
            .cloned()
    }

    fn grid_now() -> ResolvedArea {
        shown_area(LayerKind::Desktop, &widgets_area()).expect("the desktop grid")
    }

    /// The group the instance `id` is in on the desktop grid, as the screen shows it.
    fn holding(id: &str) -> Option<ResolvedGroup> {
        grid_now()
            .groups
            .into_iter()
            .find(|group| group.children.iter().any(|child| child.id.as_str() == id))
    }

    fn cells_of(id: &str) -> Option<Cells> {
        holding(id).as_ref().and_then(grid::cells_of)
    }

    fn node_of(id: &str) -> Node {
        let group = holding(id).expect("the instance is on the grid");
        Node::area(Some(SCREEN), LayerKind::Desktop, &widgets_area())
            .instance(&group.id, &InstanceId::new(id))
    }

    fn commit(ops: Vec<layout::LayoutOp>) {
        context::commit("test".to_string(), ops).expect("the edit commits");
    }

    fn at(col: u32, row: u32, cols: u32, rows: u32) -> Cells {
        Cells {
            col,
            row,
            cols,
            rows,
        }
    }

    /// A small, repeatable source of choices.
    struct Choices(u64);

    impl Choices {
        fn below(&mut self, bound: u32) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % u64::from(bound.max(1))) as u32
        }
    }

    fn nearness(cells: &Cells, to: (u32, u32)) -> (u64, u32, u32) {
        let dx = i64::from(cells.col) - i64::from(to.0);
        let dy = i64::from(cells.row) - i64::from(to.1);
        ((dx * dx + dy * dy) as u64, cells.row, cells.col)
    }

    /// Tolerant reflow, whatever is dropped wherever: nothing overlaps, nothing is lost or resized, nothing the drop does not cover moves, and each widget it displaces lands on the free cells nearest where it was — no free cells inside the grid are nearer, given what was settled before it.
    #[test]
    fn reflow_never_overlaps_never_loses_and_moves_the_displaced_to_the_nearest_free_cells() {
        let mut choices = Choices(0x9E37_79B9_7F4A_7C15);
        let sizes = [(1, 1), (2, 1), (2, 2), (4, 2), (4, 4)];
        for _ in 0..2_000 {
            let room = Room {
                cols: 4 + choices.below(12),
                rows: 3 + choices.below(8),
            };
            let mut groups: Vec<(GroupId, Cells)> = Vec::new();
            for nth in 0..choices.below(10) {
                let (cols, rows) = sizes[choices.below(sizes.len() as u32) as usize];
                for _ in 0..20 {
                    let cells = at(
                        choices.below(room.cols),
                        choices.below(room.rows),
                        cols,
                        rows,
                    );
                    if cells.fits(room) && !groups.iter().any(|(_, other)| other.overlaps(&cells)) {
                        groups.push((GroupId::new(format!("g{nth}")), cells));
                        break;
                    }
                }
            }
            let (moved, size) = match groups.is_empty() || choices.below(4) == 0 {
                true => {
                    let (cols, rows) = sizes[choices.below(sizes.len() as u32) as usize];
                    (GroupId::new("new"), at(0, 0, cols, rows))
                }
                false => groups[choices.below(groups.len() as u32) as usize].clone(),
            };
            let to = size.at(choices.below(room.cols + 3), choices.below(room.rows + 3));
            let placed = grid::reflow(&groups, &moved, to, room);
            let cells = |id: &GroupId| {
                placed
                    .iter()
                    .find(|(held, _)| held == id)
                    .map(|(_, cells)| *cells)
            };

            let before: BTreeSet<&GroupId> =
                groups.iter().map(|(id, _)| id).chain([&moved]).collect();
            let after: BTreeSet<&GroupId> = placed.iter().map(|(id, _)| id).collect();
            assert_eq!(before, after, "no group is lost or made up");
            assert_eq!(placed.len(), after.len(), "each group once");
            for (at, (one, a)) in placed.iter().enumerate() {
                for (other, b) in &placed[at + 1..] {
                    assert!(!a.overlaps(b), "{one} {a:?} overlaps {other} {b:?}");
                }
            }
            let landed = to.within(room);
            assert_eq!(cells(&moved), Some(landed));
            let mut displaced: Vec<(GroupId, Cells)> = Vec::new();
            for (id, was) in &groups {
                let now = cells(id).expect("still placed");
                assert_eq!(
                    (now.cols, now.rows),
                    (was.cols, was.rows),
                    "{id} kept its size"
                );
                let moved_away = *id != moved && now != *was;
                assert_eq!(
                    moved_away,
                    *id != moved && was.overlaps(&landed),
                    "{id} is displaced exactly when the drop covers it"
                );
                if moved_away {
                    displaced.push((id.clone(), *was));
                }
            }
            displaced.sort_by_key(|(_, was)| (was.row, was.col));
            let mut settled: Vec<Cells> = groups
                .iter()
                .filter(|(id, _)| *id != moved && !displaced.iter().any(|(held, _)| held == id))
                .map(|(_, cells)| *cells)
                .chain([landed])
                .collect();
            for (id, was) in &displaced {
                let now = cells(id).expect("placed");
                let free = |cells: &Cells| !settled.iter().any(|other| other.overlaps(cells));
                let nearer = (0..room.rows)
                    .flat_map(|row| (0..room.cols).map(move |col| was.at(col, row)))
                    .filter(|cells| cells.fits(room) && free(cells))
                    .find(|cells| match now.fits(room) {
                        true => {
                            nearness(cells, (was.col, was.row)) < nearness(&now, (was.col, was.row))
                        }
                        false => true,
                    });
                assert_eq!(nearer, None, "{id} from {was:?} landed at {now:?}");
                settled.push(now);
            }
        }
    }

    /// A grid's cells are where its rectangle puts them, whatever is on it: the built-in grid, empty, has every cell the screen fits inside its padding, centred in what is left over, and a widget added to it moves none of them.
    #[test]
    fn a_grids_cells_hold_still_whatever_is_on_it() {
        let _rig = crate::rig::rig_with("desktop-lattice", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let geometry = || {
            let desktop = surfaces::reconcile::desktops()[0].clone();
            widgets::Geometry::of(&desktop, &grid_now()).expect("a grid")
        };
        let empty = geometry();
        assert!(grid_now().groups.is_empty(), "nothing on it yet");
        assert_eq!(empty.room, Room { cols: 19, rows: 10 });
        assert_eq!(
            empty.origin,
            (56.0, 68.0),
            "centred in what the cells leave"
        );

        desktop::put(
            &Pick::Module("weather".to_string()),
            Some((widgets_area(), (7, 4))),
            LayerKind::Desktop,
        )
        .expect("a widget is added");
        assert_eq!(cells_of("weather").map(|at| (at.col, at.row)), Some((7, 4)));
        assert_eq!(geometry(), empty, "the cells are where they were");
    }

    /// The acceptance: a widget put on cells another covers takes them, and the one it displaced moves to the free cells nearest — it is never taken off the screen. One undo takes the whole drop back.
    #[test]
    fn a_widget_dropped_on_an_occupied_cell_never_deletes_the_occupant() {
        let rig = rig_with("desktop-drop-occupied", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        assert_eq!(cells_of("clock-2"), Some(at(0, 0, 4, 2)));

        desktop::put(
            &Pick::Module("weather".into()),
            Some((widgets_area(), (0, 0))),
            LayerKind::Desktop,
        )
        .expect("the palette puts it there");
        assert_eq!(
            cells_of("weather"),
            Some(at(0, 0, 4, 2)),
            "the new widget takes the cells"
        );
        assert_eq!(
            cells_of("clock-2"),
            Some(at(0, 2, 4, 2)),
            "and the clock is still there, on the free cells nearest where it was"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Add Weather"));

        let dropped = desktop::dropped(
            &session::draft().peek(),
            &surfaces::reconcile::desktops()[0],
            &node_of("clock-2"),
            &widgets_area(),
            &Landing::Cell { col: 1, row: 0 },
        )
        .expect("the clock is dropped over the weather");
        commit(dropped);
        assert_eq!(cells_of("clock-2"), Some(at(1, 0, 4, 2)));
        let weather = cells_of("weather").expect("the weather is not taken away");
        assert!(!weather.overlaps(&at(1, 0, 4, 2)), "{weather:?}");

        session::undo().expect("the drop is undone");
        session::undo().expect("the add is undone");
        assert_eq!(stored(&rig), before, "each was one entry");
    }

    /// Enter in the palette puts the entry on the free cells nearest the selection, under an id read off its module and never one already taken.
    #[test]
    fn a_widget_added_from_the_keyboard_lands_near_the_selection_with_a_readable_id() {
        let rig = rig_with("desktop-add-near", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::Instance(node_of("clock-2"))));
        let before = stored(&rig);
        desktop::put(&Pick::Module("clock".into()), None, LayerKind::Desktop)
            .expect("added near the selection");
        let fresh = holding("clock-3").expect("the bar's clock and the desktop's keep theirs");
        assert_eq!(grid::cells_of(&fresh), Some(at(0, 2, 4, 2)));
        assert_eq!(
            cells_of("clock-2"),
            Some(at(0, 0, 4, 2)),
            "nothing had to move"
        );
        session::undo().expect("the add is undone");
        assert_eq!(stored(&rig), before);
    }

    /// The palette lists each category's widgets under its heading, narrowed by what is typed, the bars' chips after them; on the lock screen only readings, and no bars (TA-8).
    #[test]
    fn the_palette_groups_by_category_narrows_by_what_is_typed_and_offers_readings_on_the_lock() {
        let _rig = rig_with("desktop-palette", |_| {});
        let _owner = Owner::new();
        let desktop = surfaces::reconcile::desktops()[0].clone();
        let names = |lines: Vec<Line>| -> Vec<String> {
            lines
                .into_iter()
                .map(|line| match line {
                    Line::Heading(heading) => format!("# {heading}"),
                    Line::Entry { name, .. } => name,
                })
                .collect()
        };
        assert_eq!(
            names(palette::lines(LayerKind::Desktop, "")),
            [
                "# Time",
                "Clock",
                "# Media",
                "Mixer",
                "# Information",
                "Weather"
            ],
            "what is on the bars is another layer's, and never offered here"
        );
        assert_eq!(
            names(palette::lines(LayerKind::Desktop, "WEA")),
            ["# Information", "Weather"]
        );
        assert_eq!(
            names(palette::lines(LayerKind::Lock, "")),
            ["# Time", "Clock", "# Information", "Weather"],
            "the mixer answers the pointer, so the lock screen is not offered it"
        );
        let mixer = ui::descriptor::find("mixer").expect("installed");
        assert_eq!(palette::offered(mixer, LayerKind::Lock), None);
        assert_eq!(
            palette::offered(mixer, LayerKind::Desktop),
            Some(Representation::WidgetM)
        );
        let refused = desktop::added(
            &session::draft().peek(),
            &desktop,
            LayerKind::Lock,
            &AreaId::new("lock-readings"),
            &desktop::Adding {
                module: "mixer",
                representation: Representation::WidgetM,
                at: None,
                near: (0, 0),
            },
        );
        assert!(
            matches!(&refused, Err(session::EditError::Refused(why)) if why.contains("readings only")),
            "{refused:?}"
        );
    }

    /// Dropped onto the middle of another widget, a widget stacks with it at its cells; a third joins the stack; dragged out, each leaves it for cells of its own, and a stack of one is a widget again.
    #[test]
    fn dropping_onto_a_widget_stacks_them_and_dragging_out_detaches() {
        let rig = rig_with("desktop-stack", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let add = |id_of: &str, cell: (u32, u32)| {
            let (ops, id) = desktop::added(
                &session::draft().peek(),
                &surfaces::reconcile::desktops()[0],
                LayerKind::Desktop,
                &widgets_area(),
                &desktop::Adding {
                    module: id_of,
                    representation: Representation::WidgetM,
                    at: Some(cell),
                    near: (0, 0),
                },
            )
            .expect("added");
            commit(ops);
            id
        };
        let drop = |id: &str, landing: Landing| {
            let ops = desktop::dropped(
                &session::draft().peek(),
                &surfaces::reconcile::desktops()[0],
                &node_of(id),
                &widgets_area(),
                &landing,
            )
            .expect("dropped");
            commit(ops);
        };
        add("weather", (5, 0));
        let target = holding("weather").expect("the weather").id;

        drop("clock-2", Landing::Onto(target.clone()));
        let stack = holding("clock-2").expect("the clock is still placed");
        assert_eq!(stack.id, target, "in the weather's group");
        assert!(stack.is_pages(), "one at a time");
        assert_eq!(stack.children.len(), 2);
        assert_eq!(
            grid::cells_of(&stack).map(|cells| (cells.col, cells.row)),
            Some((5, 0))
        );
        assert!(
            grid_now()
                .groups
                .iter()
                .all(|group| group.id.as_str() != "clock"),
            "the clock's own group, empty, is gone"
        );

        let third = add("clock", (0, 4));
        drop(third.as_str(), Landing::Onto(target.clone()));
        assert_eq!(
            holding(third.as_str()).map(|group| group.children.len()),
            Some(3),
            "the stack takes one more"
        );

        drop("clock-2", Landing::Cell { col: 10, row: 0 });
        let alone = holding("clock-2").expect("detached, still placed");
        assert_ne!(alone.id, target);
        assert!(!alone.is_pages());
        assert_eq!(
            grid::cells_of(&alone).map(|cells| (cells.col, cells.row)),
            Some((10, 0))
        );
        assert!(holding("weather").is_some_and(|group| group.is_pages()));

        drop(third.as_str(), Landing::Cell { col: 0, row: 4 });
        let left = holding("weather").expect("the weather stays");
        assert_eq!(left.children.len(), 1);
        assert!(!left.is_pages(), "a stack of one is a widget again");

        for _ in 0..6 {
            session::undo().expect("one entry each");
        }
        assert_eq!(stored(&rig), before);
    }

    /// Ctrl+→ steps the selected widget one size up, and what it now covers moves to the free cells nearest where it was.
    #[test]
    fn a_size_step_moves_what_the_widget_grows_over() {
        let rig = rig_with("desktop-size", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let (ops, _) = desktop::added(
            &session::draft().peek(),
            &surfaces::reconcile::desktops()[0],
            LayerKind::Desktop,
            &widgets_area(),
            &desktop::Adding {
                module: "weather",
                representation: Representation::WidgetS,
                at: Some((0, 2)),
                near: (0, 0),
            },
        )
        .expect("added under the clock");
        commit(ops);
        let before = stored(&rig);
        assert!(session::select(Selection::Instance(node_of("clock-2"))));
        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_ctrl: true,
                ..NONE
            }
        ));
        assert_eq!(cells_of("clock-2"), Some(at(0, 0, 4, 4)), "large now");
        assert_eq!(
            cells_of("weather"),
            Some(at(0, 4, 2, 2)),
            "moved to the free cells nearest where it was"
        );
        session::undo().expect("one entry");
        assert_eq!(stored(&rig), before);
    }

    fn resolved_on(layout: &Layout, workspace: Option<&str>) -> layout::Resolved {
        let active = workspace.map(|name| ActiveWorkspace {
            name: name.to_string(),
            id: None,
            special: None,
        });
        layout::resolve(layout, &layout::Library::default(), SCREEN, active.as_ref()).0
    }

    fn instance_on<'a>(
        resolved: &'a layout::Resolved,
        id: &str,
    ) -> Option<&'a layout::ResolvedInstance> {
        resolved
            .instances()
            .find(|instance| instance.id.as_str() == id)
    }

    fn group_on(resolved: &layout::Resolved, id: &str) -> Option<ResolvedGroup> {
        resolved
            .layer(LayerKind::Desktop)?
            .areas
            .iter()
            .flat_map(|area| area.groups.iter())
            .find(|group| group.children.iter().any(|child| child.id.as_str() == id))
            .cloned()
    }

    fn stacked_clock(layout: &mut Layout) {
        let areas = &mut layout.outputs[0].layers.desktop.areas;
        let group = areas[0].groups.last_mut().expect("the clock's group");
        group.arrange = Some(Arrange::Pages);
        group.gap = Some(4.0);
    }

    fn tidy_the_clock() -> Vec<layout::LayoutOp> {
        let group = holding("clock-2").expect("the clock is placed").id;
        desktop::tidied(
            &session::draft().peek(),
            &surfaces::reconcile::desktops()[0],
            LayerKind::Desktop,
            &widgets_area(),
            &group,
        )
        .expect("tidied")
    }

    /// A stack of one whose `pages` the edited level wrote itself is a widget again by deleting those keys, with nothing taken back.
    #[test]
    fn a_stack_of_one_written_by_the_edited_level_loses_its_keys() {
        let rig = rig_with("desktop-own-stack", stacked_clock);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        assert!(holding("clock-2").is_some_and(|group| group.is_pages()));
        commit(tidy_the_clock());
        let layout = stored(&rig);
        let group = layout.outputs[0].layers.desktop.areas[0]
            .groups
            .last()
            .expect("the clock's group");
        assert_eq!((group.arrange, group.gap), (None, None));
        assert!(group.unset.is_empty());
        assert!(holding("clock-2").is_some_and(|group| !group.is_pages()));
    }

    /// A stack of one whose `pages` a broader level writes is taken back from the workspace's rule by `unset = ["arrange"]`, so that workspace draws a widget while the others keep the stack.
    #[test]
    fn a_stack_of_one_a_broader_level_writes_is_taken_back_by_unset() {
        let rig = rig_on("desktop-inherited-stack", Some("2"), stacked_clock);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        variant::set(true).expect("the screen says which workspace is up");
        commit(tidy_the_clock());
        let layout = stored(&rig);
        let rule = &layout.outputs[0].workspaces[0];
        let group = &rule.layers.desktop.areas[0].groups[0];
        assert_eq!(group.unset, [layout::Unset::Arrange]);
        assert_eq!(group.arrange, None);
        let pages = |workspace: Option<&str>| {
            group_on(&resolved_on(&layout, workspace), "clock-2").map(|group| group.is_pages())
        };
        assert_eq!(pages(Some("2")), Some(false));
        assert_eq!(pages(Some("3")), Some(true));
        assert_eq!(pages(None), Some(true));
    }

    /// With "this workspace only" switched on under an open instance popover, what it changed moves into that workspace's rule, and a widget dropped on other cells lands there alone too.
    #[test]
    fn an_instance_edited_for_one_workspace_is_written_there_alone() {
        let rig = rig_on("desktop-variant", Some("2"), |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        popover::open_instance(node_of("clock-2")).expect("its popover opens");
        let _tree = popover::tree().expect("a popover").expect("it builds");
        let draft = popover::instance_draft().expect("an instance's draft");
        draft.set_option(
            &popover::path_of("format"),
            toml::Value::String("%H".into()),
        );
        variant::set(true).expect("the screen says which workspace is up");
        popover::close();

        let layout = stored(&rig);
        let format = |workspace: Option<&str>| {
            instance_on(&resolved_on(&layout, workspace), "clock-2")
                .and_then(|clock| clock.options.get("format").cloned())
        };
        assert_eq!(format(Some("2")), Some(toml::Value::String("%H".into())));
        assert_eq!(
            format(Some("3")),
            None,
            "other workspaces keep what they had"
        );
        assert_eq!(format(None), None);
        assert_eq!(
            layout.outputs[0].workspaces.len(),
            1,
            "the rule for the workspace was made"
        );

        let dropped = desktop::dropped(
            &session::draft().peek(),
            &surfaces::reconcile::desktops()[0],
            &node_of("clock-2"),
            &widgets_area(),
            &Landing::Cell { col: 6, row: 1 },
        )
        .expect("dropped");
        commit(dropped);
        let layout = stored(&rig);
        let placed = |workspace: Option<&str>| {
            group_on(&resolved_on(&layout, workspace), "clock-2")
                .and_then(|group| grid::cells_of(&group))
        };
        assert_eq!(
            placed(Some("2")).map(|cells| (cells.col, cells.row)),
            Some((6, 1))
        );
        assert_eq!(
            placed(Some("3")).map(|cells| (cells.col, cells.row)),
            Some((0, 0))
        );
    }

    /// An area only the extended layout places is taken off by naming it in the layer's `remove` — by Delete and by its menu's Remove — and one undo puts it back.
    #[test]
    fn deleting_an_area_a_broader_level_places_names_it_in_the_layers_remove() {
        let rig = rig_with("desktop-delete-inherited", |layout| {
            *layout = Layout {
                id: LayoutId::new("mine"),
                extends: Some(LayoutId::new(layout::BUILT_IN)),
                outputs: vec![OutputRule {
                    matches: OutputMatch("*".into()),
                    ..OutputRule::default()
                }],
                ..Layout::default()
            };
            layout.outputs[0].layers.top.areas.push(Area {
                id: AreaId::new("bar-top"),
                reserve: Some(false),
                ..Area::default()
            });
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let widgets = Node::area(Some(SCREEN), LayerKind::Desktop, &widgets_area());

        assert!(session::select(Selection::Area(widgets.clone())));
        assert!(tap(Key::Named(NamedKey::Delete), NONE));
        assert!(
            shown_area(LayerKind::Desktop, &widgets_area()).is_none(),
            "Delete takes it off"
        );
        assert_eq!(
            stored(&rig).outputs[0].layers.desktop.remove,
            [widgets_area()],
            "named in the layer's remove"
        );
        session::undo().expect("undone");
        assert_eq!(stored(&rig), before);

        context::open(Asked {
            node: widgets,
            window: LayerKind::Overlay,
            at: None,
        })
        .expect("its menu opens");
        context::pick("Remove");
        assert!(
            shown_area(LayerKind::Desktop, &widgets_area()).is_none(),
            "and so does Remove"
        );
        session::undo().expect("undone");
        assert_eq!(stored(&rig), before);
    }

    /// Joining two regions while editing one workspace alone hides the one taken away on that workspace, although every workspace's rule writes it.
    #[test]
    fn a_join_for_one_workspace_hides_the_region_every_workspace_places() {
        let rig = rig_on("desktop-join-variant", Some("2"), |layout| {
            let background = &mut layout.outputs[0].layers.background;
            if let Some(AreaKind::WallpaperRegion { rect, .. }) = &mut background.areas[0].kind {
                *rect = Some(layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                });
            }
            background.areas.push(Area {
                id: AreaId::new("right"),
                kind: Some(AreaKind::WallpaperRegion {
                    rect: Some(layout::Rect {
                        x: 0.5,
                        y: 0.0,
                        w: 0.5,
                        h: 1.0,
                    }),
                    source: None,
                    fit: None,
                    transition: None,
                }),
                ..Area::default()
            });
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        variant::set(true).expect("the screen says which workspace is up");
        assert!(session::select(Selection::Area(Node::area(
            Some(SCREEN),
            LayerKind::Background,
            &AreaId::new("background")
        ))));
        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_alt: true,
                ..NONE
            }
        ));
        let layout = stored(&rig);
        let rule = &layout.outputs[0].workspaces[0];
        assert_eq!(rule.matches, WorkspaceMatch("2".into()));
        assert_eq!(rule.layers.background.remove, [AreaId::new("right")]);
        let regions = |workspace: Option<&str>| {
            crate::modes::regions::tiles_of(&resolved_on(&layout, workspace), LayerKind::Background)
                .into_iter()
                .map(|tile| tile.id.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(regions(Some("2")), ["background"]);
        assert_eq!(regions(Some("3")), ["background", "right"]);
    }

    /// A drag reads the pointer against the grid's cells: over the middle of a widget it stacks, nearer its edge it goes on the cells under the carried widget's corner, and over no grid it lands nowhere.
    #[test]
    fn the_pointer_stacks_over_a_widgets_middle_and_places_beside_it_elsewhere() {
        let _rig = rig_with("desktop-landing", |layout| {
            layout.outputs[0].layers.desktop.areas[0].kind = Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                }),
                cell: None,
                gap: None,
                anchor: Some(layout::Anchor::TopLeft),
            });
        });
        let _owner = Owner::new();
        let grids = widgets::grids(SCREEN, LayerKind::Desktop);
        let (geometry, _) = &grids[0];
        let clock = geometry.rect_of(at(0, 0, 4, 2));
        let centre = (clock.x + clock.width / 2.0, clock.y + clock.height / 2.0);
        let one = at(0, 0, 1, 1);
        let landed = |point| widgets::landing_at(&grids, point, (0.0, 0.0), one, None);
        assert!(matches!(
            landed(centre),
            Some((_, Landing::Onto(group), widgets::Aim::Onto(_))) if group.as_str() == "clock"
        ));
        assert!(matches!(
            landed((clock.x + 4.0, clock.y + 4.0)),
            Some((_, Landing::Cell { col: 0, row: 0 }, widgets::Aim::Cells(_)))
        ));
        let own = (&widgets_area(), &GroupId::new("clock"));
        assert!(
            matches!(
                widgets::landing_at(&grids, centre, (0.0, 0.0), one, Some(own)),
                Some((_, Landing::Cell { .. }, _))
            ),
            "never onto its own group"
        );
        assert_eq!(landed((1900.0, 500.0)), None, "the right half has no grid");
        let far = (
            geometry.origin.0 + 5.0 * geometry.pitch + 3.0,
            geometry.origin.1 + 3.0 * geometry.pitch,
        );
        assert!(matches!(
            landed(far),
            Some((_, Landing::Cell { col: 5, row: 3 }, _))
        ));
        assert!(matches!(
            grid_now().groups[0].kind,
            GroupKind::Cell { col: 0, row: 0, .. }
        ));
    }

    /// The desktop mode's layer over the screen, the palette and a widget's popover build, the size row among its rows.
    #[test]
    fn the_desktop_tools_the_palette_and_a_widgets_popover_build() {
        let _rig = rig_with("desktop-builds", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let mode = mode::current().expect("the mode is up");
        widgets::tool(&mode).expect("the desktop tool builds");
        palette::tree(SCREEN, LayerKind::Desktop).expect("the palette builds");
        palette::open().expect("the palette opens");
        assert!(transient::is_open(palette::ID));
        transient::close(palette::ID);
        popover::open_instance(node_of("clock-2")).expect("the widget's popover opens");
        popover::tree().expect("a popover").expect("it builds");
        popover::close();
    }

    /// B7: a size drag Esc called off leaves nothing behind for the next one, which reads the widget where it is then: moved two rows down in between, a small pull on its corner keeps its size.
    #[test]
    fn a_cancelled_size_drag_starts_the_next_one_where_the_widget_is() {
        let rig = rig_with("desktop-size-cancel", |layout| {
            layout.outputs[0].layers.desktop.areas[0].kind = Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                }),
                cell: None,
                gap: None,
                anchor: Some(layout::Anchor::TopLeft),
            });
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let geometry = widgets::grids(SCREEN, LayerKind::Desktop)[0].0.clone();
        let clock = node_of("clock-2");
        let drawn = telar::signal(geometry.rect_of(cells_of("clock-2").expect("on the grid")));
        surfaces::rects::track_spanning(clock.clone(), vec![drawn]);
        assert!(session::select(Selection::Instance(clock)));
        let mode = mode::current().expect("the mode is up");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = Pointed::new(Box::new(
            Container::new(
                page,
                vec![widgets::tool(&mode).expect("the desktop tool builds")],
            )
            .expect("a page"),
        ));
        let node = root.layout_node();
        let mut tree = ComponentList::new(root);
        let lay_out = || {
            compute_layout(
                node,
                AvailableSpace::Definite(1920.0),
                AvailableSpace::Definite(1080.0),
            )
            .expect("the tool lays out")
        };
        lay_out();
        let mut route = |event: Event| {
            telar::observe_keyboard(&event);
            if !telar::dispatch_overlays(&event) {
                tree.on_event(&event);
            }
        };
        let corner = |rect: telar::Rect| (rect.x + rect.width, rect.y + rect.height);
        let press = |(x, y): (f32, f32)| Event::PointerPressed {
            x: x.into(),
            y: y.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        };
        let to = |(x, y): (f32, f32)| Event::PointerMoved {
            x: x.into(),
            y: y.into(),
            source: PointerSource::Mouse,
        };
        let before = stored(&rig);

        let start = corner(drawn.peek());
        route(press(start));
        route(to((start.0 + 6.0, start.1 + 2.0 * geometry.pitch)));
        assert_eq!(
            holding("clock-2").map(|group| group.children[0].representation),
            Some(Representation::WidgetL),
            "the pull previews the large size"
        );
        route(Event::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: NONE,
        });
        assert_eq!(stored(&rig), before, "Esc called it off");
        route(Event::PointerReleased {
            x: start.0.into(),
            y: start.1.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });

        for _ in 0..2 {
            assert!(tap(
                Key::Named(NamedKey::ArrowDown),
                ModifiersState {
                    is_shift: true,
                    ..NONE
                }
            ));
        }
        drawn.set(geometry.rect_of(cells_of("clock-2").expect("still on the grid")));
        lay_out();
        let moved = stored(&rig);
        let start = corner(drawn.peek());
        route(press(start));
        route(to((start.0 + 6.0, start.1 + 6.0)));
        assert_eq!(
            holding("clock-2").map(|group| group.children[0].representation),
            Some(Representation::WidgetM),
            "measured from where the widget is now, the pull asks for the size it has"
        );
        route(Event::PointerReleased {
            x: (start.0 + 6.0).into(),
            y: (start.1 + 6.0).into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        assert_eq!(stored(&rig), moved);
    }

    /// B8: what only a mode can do — the palette, a new grid, a new bar, a new stack — says so when asked outside one, rather than calling it a missing workspace or nothing to customize.
    #[test]
    fn what_only_a_mode_does_says_so_outside_one() {
        let _rig = rig_with("desktop-no-mode", |_| {});
        let _owner = Owner::new();
        let needs = Err(crate::session::EditError::Refused(
            "Only an edit mode can do this".to_string(),
        ));
        assert_eq!(palette::open(), needs);
        assert_eq!(desktop::create_grid(), needs);
        assert_eq!(crate::modes::top::create_on(config::Edge::Bottom), needs);
        assert_eq!(crate::modes::overlay::add_stack(), needs);
    }

    fn pill(
        module: &str,
        representation: Representation,
        repeat: Option<&str>,
    ) -> layout::Komponent {
        layout::Komponent {
            parameters: [(
                "label".to_string(),
                layout::Parameter {
                    ty: layout::ParameterType::parse("text").expect("a type"),
                    default: layout::Expr("'Home'".into()),
                },
            )]
            .into(),
            repeat: repeat.map(|text| layout::Expr(text.into())),
            children: vec![layout::Instance {
                id: InstanceId::new("face"),
                module: Some(module.to_string()),
                representation: Some(representation),
                ..layout::Instance::default()
            }],
            ..layout::Komponent::default()
        }
    }

    /// A rig whose library holds a komponent of the clock, the weather, the mixer (which answers the pointer) and a repeating clock.
    fn with_komponents(test: &str) -> Rig {
        let rig = rig_with(test, |_| {});
        for (name, komponent) in [
            ("clock-pill", pill("clock", Representation::WidgetM, None)),
            (
                "weather-pill",
                pill("weather", Representation::WidgetS, None),
            ),
            ("mixer-pill", pill("mixer", Representation::WidgetM, None)),
            (
                "loop",
                pill("clock", Representation::WidgetM, Some("{1, 2}")),
            ),
        ] {
            rig.store
                .borrow_mut()
                .add_komponent(layout::KomponentId::new(name), komponent)
                .expect("a komponent");
        }
        rig
    }

    fn listed(layer: LayerKind) -> Vec<String> {
        palette::lines(layer, "")
            .into_iter()
            .map(|line| match line {
                Line::Heading(heading) => format!("# {heading}"),
                Line::Entry { name, .. } => name,
            })
            .collect()
    }

    fn komponent_group(id: &str) -> Option<ResolvedGroup> {
        grid_now().groups.into_iter().find(|group| {
            group
                .komponent
                .as_ref()
                .is_some_and(|used| used.id.as_str() == id)
        })
    }

    /// The palette lists the library's komponents after the modules, each with how many parameters it takes and narrowed by what is typed; a repeating one is left out of the grid, and on the lock screen so is one holding a control.
    #[test]
    fn the_palette_lists_komponents_with_their_parameters_and_the_lock_leaves_out_controls() {
        let _rig = with_komponents("palette-komponents");
        let _owner = Owner::new();
        let on_desktop = listed(LayerKind::Desktop);
        let at = on_desktop
            .iter()
            .position(|line| line == "# Komponents")
            .expect("a section for them");
        assert_eq!(
            on_desktop[at + 1..at + 4],
            [
                "clock-pill · 1 parameter",
                "mixer-pill · 1 parameter",
                "weather-pill · 1 parameter"
            ],
            "{on_desktop:?}"
        );
        assert!(
            on_desktop[at..]
                .iter()
                .all(|line| !line.starts_with("loop")),
            "a grid cell cannot repeat: {on_desktop:?}"
        );
        assert!(
            on_desktop[..at].iter().any(|line| line == "Weather"),
            "after the modules"
        );

        let on_lock = listed(LayerKind::Lock);
        assert!(
            on_lock.contains(&"clock-pill · 1 parameter".to_string()),
            "{on_lock:?}"
        );
        assert!(
            on_lock.contains(&"weather-pill · 1 parameter".to_string()),
            "{on_lock:?}"
        );
        assert!(
            !on_lock.iter().any(|line| line.starts_with("mixer-pill")),
            "the mixer answers the pointer: {on_lock:?}"
        );

        let narrowed: Vec<Line> = palette::lines(LayerKind::Desktop, "WEATHER-P");
        assert_eq!(narrowed.len(), 2, "its heading and the one entry");
    }

    /// Enter in the palette adds the komponent as a group of its own on the free cells nearest the selection, its parameters at their defaults, selected once there and taken back by one undo.
    #[test]
    fn a_komponent_chosen_from_the_keyboard_is_added_as_a_group_of_its_own() {
        let rig = with_komponents("palette-komponent-keys");
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::Instance(node_of("clock-2"))));
        let before = stored(&rig);

        assert!(tap(Key::Char('a'), NONE));
        assert!(transient::is_open(palette::ID));
        let (mut card, _) = page(palette::tree(SCREEN, LayerKind::Desktop));
        let press = |card: &mut ComponentList, key: Key| {
            let event = Event::KeyPressed {
                key,
                modifiers: NONE,
            };
            telar::observe_keyboard(&event);
            card.on_event(&event)
        };
        for ch in "weather-p".chars() {
            press(&mut card, Key::Char(ch));
        }
        press(&mut card, Key::Named(NamedKey::Enter));
        assert!(
            !transient::is_open(palette::ID),
            "choosing closes the palette"
        );

        let used = komponent_group("weather-pill").expect("the group draws the komponent");
        assert_eq!(used.id.as_str(), "weather-pill");
        assert!(
            stored(&rig).outputs[0].layers.desktop.areas[0]
                .groups
                .iter()
                .any(|group| group.id.as_str() == "weather-pill" && group.parameters.is_empty()),
            "the parameters are the komponent's own defaults"
        );
        assert_eq!(used.children[0].id.as_str(), "widgets.weather-pill/face");
        assert_eq!(
            grid::cells_of(&used).map(|cells| (cells.cols, cells.rows)),
            Some((2, 2)),
            "the footprint of its small widget"
        );
        assert!(matches!(session::selected(), Selection::Group(_)));
        assert_eq!(rig.undo_label().as_deref(), Some("Add weather-pill"));
        session::undo().expect("one undo takes it back");
        assert_eq!(stored(&rig), before);
        assert!(komponent_group("weather-pill").is_none());
    }

    fn page(built: Built) -> (ComponentList, telar::NodeId) {
        let root = Pointed::new(Box::new(
            Container::new(
                LayoutStyle::new().width(1920.0).height(1080.0),
                vec![built.expect("it builds")],
            )
            .expect("a page"),
        ));
        let node = root.layout_node();
        let tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(1920.0),
            AvailableSpace::Definite(1080.0),
        )
        .expect("it lays out");
        (tree, node)
    }

    fn where_drawn(tree: &ComponentList, wanted: &str) -> (f32, f32) {
        let mut found = None;
        telar::for_each_with_matrix(&tree.commands(), |command, [a, b, c, d, e, f]| {
            if let telar::DrawCommand::Text { text, rect, .. } = command
                && text.to_string() == wanted
            {
                found = Some((
                    a * (rect.x + 4.0) + c * (rect.y + rect.height / 2.0) + e,
                    b * (rect.x + 4.0) + d * (rect.y + rect.height / 2.0) + f,
                ));
            }
        });
        found.unwrap_or_else(|| panic!("{wanted:?} is drawn"))
    }

    fn pointer(event: fn(f64, f64) -> Event, tree: &mut ComponentList, (x, y): (f32, f32)) {
        let event = event(f64::from(x), f64::from(y));
        telar::observe_keyboard(&event);
        if !telar::dispatch_overlays(&event) {
            tree.on_event(&event);
        }
    }

    fn pressed(x: f64, y: f64) -> Event {
        Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn released(x: f64, y: f64) -> Event {
        Event::PointerReleased {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn moved(x: f64, y: f64) -> Event {
        Event::PointerMoved {
            x,
            y,
            source: PointerSource::Mouse,
        }
    }

    fn typed_into(card: &mut ComponentList, key: Key) {
        let event = Event::KeyPressed {
            key,
            modifiers: NONE,
        };
        telar::observe_keyboard(&event);
        card.on_event(&event);
        for _ in 0..3 {
            telar::relayout_if_dirty();
        }
    }

    /// The palette open and as its window builds it, narrowed by typing `typed` into it, and where the entry `name` is drawn there.
    fn palette_showing(typed: &str, name: &str) -> (ComponentList, (f32, f32)) {
        palette::open().expect("the palette opens");
        let (mut card, _) = page(palette::tree(SCREEN, LayerKind::Desktop));
        for _ in 0..3 {
            telar::relayout_if_dirty();
        }
        for ch in typed.chars() {
            typed_into(&mut card, Key::Char(ch));
        }
        let entry = where_drawn(&card, name);
        (card, entry)
    }

    fn entries(layer: LayerKind) -> Vec<(Pick, String)> {
        palette::lines(layer, "")
            .into_iter()
            .filter_map(|line| match line {
                Line::Entry { pick, name, .. } => Some((pick, name)),
                Line::Heading(_) => None,
            })
            .collect()
    }

    /// The palette's lines fill what its capped card leaves them, so the pointer reaches them where they are drawn: the first entry pressed is picked, and the last, which the arrows scroll into view, is drawn on screen and picked by a press too.
    #[test]
    fn the_palettes_lines_are_inside_its_card_where_a_press_reaches_them() {
        let _rig = with_komponents("palette-lines-pressed");
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let listed = entries(LayerKind::Desktop);
        let (first, first_name) = listed.first().cloned().expect("an entry");
        let (last, last_name) = listed.last().cloned().expect("an entry");

        let (mut card, at) = palette_showing("", &first_name);
        let title = where_drawn(&card, "Add a widget");
        assert!(at.1 > title.1, "the lines are under the title: {at:?}");
        pointer(moved, &mut card, at);
        pointer(pressed, &mut card, at);
        pointer(released, &mut card, at);
        assert_eq!(palette::picked().peek(), Some(first));
        palette::unpick();

        let (mut card, _) = palette_showing("", &first_name);
        for _ in 1..listed.len() {
            typed_into(&mut card, Key::Named(NamedKey::ArrowDown));
        }
        let at = where_drawn(&card, &last_name);
        assert!(
            at.1 > title.1 && at.1 < 1080.0,
            "the entry pointed at is scrolled into view: {at:?}"
        );
        pointer(moved, &mut card, at);
        pointer(pressed, &mut card, at);
        pointer(released, &mut card, at);
        assert_eq!(palette::picked().peek(), Some(last));
        palette::unpick();
    }

    /// Through the pointer, a komponent pressed in the palette is picked, and the next press on a grid's cells puts it there as a group of its own: one entry, the cells under the pointer.
    #[test]
    fn a_komponent_pressed_in_the_palette_is_put_on_the_cells_pressed() {
        let rig = with_komponents("palette-komponent-press");
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let (mut palette_tree, entry) = palette_showing("clock-p", "clock-pill · 1 parameter");
        pointer(moved, &mut palette_tree, entry);
        pointer(pressed, &mut palette_tree, entry);
        pointer(released, &mut palette_tree, entry);
        assert_eq!(
            palette::picked().peek(),
            Some(Pick::Komponent(layout::KomponentId::new("clock-pill")))
        );

        let mode = mode::current().expect("the mode is up");
        let (mut tool, _) = page(widgets::tool(&mode));
        for _ in 0..2 {
            telar::relayout_if_dirty();
        }
        let geometry = widgets::grids(SCREEN, LayerKind::Desktop)[0].0.clone();
        let target = geometry.rect_of(at(0, 6, 1, 1));
        let point = (
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        );
        pointer(moved, &mut tool, point);
        pointer(pressed, &mut tool, point);
        pointer(released, &mut tool, point);

        let used = komponent_group("clock-pill").expect("put on the grid");
        assert!(
            matches!(used.kind, GroupKind::Cell { row: 6, .. }),
            "{:?}",
            used.kind
        );
        assert!(palette::picked().peek().is_none(), "the pick is spent");
        assert_eq!(rig.undo_label().as_deref(), Some("Add clock-pill"));
        session::undo().expect("one undo takes it back");
        assert_eq!(stored(&rig), before);
    }

    /// Dragged out of the palette and let go over the grid, a komponent lands where it is let go, previewed on the way and one undo entry once let go.
    #[test]
    fn a_komponent_dragged_from_the_palette_lands_where_it_is_let_go() {
        let rig = with_komponents("palette-komponent-drag");
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let before = stored(&rig);
        let (mut palette_tree, entry) = palette_showing("clock-p", "clock-pill · 1 parameter");
        let geometry = widgets::grids(SCREEN, LayerKind::Desktop)[0].0.clone();
        let target = geometry.rect_of(at(0, 4, 4, 2));
        let point = (
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        );

        pointer(moved, &mut palette_tree, entry);
        pointer(pressed, &mut palette_tree, entry);
        pointer(moved, &mut palette_tree, (entry.0 + 8.0, entry.1));
        pointer(moved, &mut palette_tree, point);
        assert!(
            komponent_group("clock-pill").is_some(),
            "the drop is previewed under the pointer"
        );
        assert_eq!(stored(&rig), before, "and nothing is written yet");
        pointer(released, &mut palette_tree, point);

        let used = komponent_group("clock-pill").expect("let go on the grid");
        assert!(
            matches!(used.kind, GroupKind::Cell { col: 0, row: 4, .. }),
            "{:?}",
            used.kind
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Place a widget"));
        session::undo().expect("one undo takes it back");
        assert_eq!(stored(&rig), before);
    }

    /// The layout's own rules hold where a komponent is put from the editor: the lock layer takes only readings, a grid cell cannot repeat, a bar takes a repeating one in its end zone, and what is put validates as written.
    #[test]
    fn a_komponent_is_put_where_the_layout_would_accept_it_and_nowhere_else() {
        let rig = with_komponents("komponent-plan");
        let _owner = Owner::new();
        let desktop = surfaces::reconcile::desktops()[0].clone();
        let layout = stored(&rig);
        let library = rig.store.borrow().all().clone();
        let catalogue = surfaces::catalogue::Descriptors::installed();
        let plan = |layer, area: &str, name: &str| {
            crate::komponent::plan_use(
                &layout,
                &library,
                &catalogue,
                (&desktop.resolved, Some(&desktop)),
                &crate::komponent::Placing {
                    layer,
                    area: &AreaId::new(area),
                    group: None,
                    output: Some(SCREEN),
                    workspace: None,
                    cell: None,
                    near: (0, 0),
                    zone: None,
                },
                &crate::komponent::Use::of(layout::KomponentId::new(name)),
            )
        };

        assert!(matches!(
            plan(LayerKind::Lock, "lock-readings", "mixer-pill"),
            Err(crate::komponent::Refusal::Use(layout::UseError::Lock(..)))
        ));
        assert!(plan(LayerKind::Lock, "lock-readings", "weather-pill").is_ok());
        assert!(matches!(
            plan(LayerKind::Desktop, "widgets", "loop"),
            Err(crate::komponent::Refusal::Use(
                layout::UseError::CellRepeats
            ))
        ));
        let (ops, group) = plan(LayerKind::Top, "bar-top", "loop").expect("a bar repeats");
        let mut after = layout.clone();
        layout::ops::apply_all(&mut after, &ops).expect("the operations apply");
        let (resolved, _) = layout::resolve(&after, &library, SCREEN, None);
        let placed = resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))
            .and_then(|bar| bar.groups.iter().find(|held| held.id == group))
            .expect("the group is drawn");
        assert_eq!(placed.id.as_str(), "loop");
        assert!(matches!(
            placed.kind,
            GroupKind::Zone {
                zone: layout::Zone::End
            }
        ));
        assert!(
            layout::validate_komponents(&after, &library, &catalogue).is_clean(),
            "{}",
            layout::validate_komponents(&after, &library, &catalogue).render()
        );
    }
}
