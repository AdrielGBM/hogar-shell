//! The top mode's tools: bars made, moved and resized on every edge, split and joined back, chips carried between zones and bars and off every bar, bars sent to another screen, and reservation renegotiated once, when a gesture is committed.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use telar::{
        Event, Key, LayoutItem, LayoutStyle, ModifiersState, NamedKey, RectStyle, StyledContainer,
    };

    use config::{Edge, Shape};
    use layout::{
        AreaId, Extent, InstanceId, LayerKind, Layout, ResolvedArea, ResolvedAreaKind, Zone,
    };
    use surfaces::layer_window::{Demands, LayerWindowContext};
    use surfaces::menu::{Asked, Pointed};
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{
        Built, Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef,
    };
    use ui::host::{Host, WidgetSize};

    use crate::keys::{self, Direction, Press};
    use crate::mode::{self};
    use crate::modes::bars::{Seen, measured, nearest_edge};
    use crate::modes::top::{self, ChipLanding, Drawn};
    use crate::rig::{Rig, SCREEN, enter, rig_on, rig_prepared, rig_screens, rig_with};
    use crate::session::{self, Edit, EditError, Selection};
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
        sizes: &'static [WidgetSize],
    ) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name,
            icon: "circle",
            category: Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(ChipDef::new(face, Input::ReadOnly)),
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
        module("workspaces", "Workspaces", &[]),
        module("clock", "Clock", &WidgetSize::ALL),
        module("notes", "Notes", &[]),
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
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn stored(rig: &Rig) -> Layout {
        rig.store.borrow().active().clone()
    }

    fn commit(ops: Vec<layout::LayoutOp>) {
        context::commit("test".to_string(), ops).expect("the edit commits");
    }

    fn bar_top() -> AreaId {
        AreaId::new("bar-top")
    }

    fn screen() -> reconcile::Desktop {
        reconcile::desktops()
            .iter()
            .find(|desktop| desktop.output.as_deref() == Some(SCREEN))
            .cloned()
            .expect("the edited screen")
    }

    fn bar(id: &AreaId) -> Option<ResolvedArea> {
        screen()
            .resolved
            .layer(LayerKind::Top)?
            .areas
            .iter()
            .find(|area| area.id == *id)
            .cloned()
    }

    fn edge_of(id: &AreaId) -> Option<Edge> {
        bar(id)?.kind.edge()
    }

    fn span(id: &AreaId) -> surfaces::bar::Span {
        top::span_of(&screen(), &bar(id).expect("the bar is on screen")).expect("it is a bar")
    }

    /// The instances of a bar's zone, in order.
    fn zone(id: &AreaId, zone: Zone) -> Vec<String> {
        bar(id)
            .map(|area| {
                area.groups
                    .iter()
                    .filter(|group| group.kind == layout::GroupKind::Zone { zone })
                    .flat_map(|group| group.children.iter().map(|child| child.id.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    thread_local! {
        static DRAWN: RefCell<Option<(telar::OwnerId, telar::ComponentList)>> = const { RefCell::new(None) };
    }

    fn undraw() {
        if let Some((owner, tree)) = DRAWN.with(|drawn| drawn.borrow_mut().take()) {
            drop(tree);
            telar::dispose_owner(owner);
        }
    }

    /// Draws the edited screen's top layer as its window would, from the arrangement the windows show now, so what the tools read from the rect registry is there — the rig runs no compositor, so its windows build nothing. What was drawn before is taken down first.
    fn draw() {
        undraw();
        let scope = telar::owner_scope();
        telar::set_context(LayerWindowContext {
            layer: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            demands: Rc::new(Demands::new(platform_wayland::Layer::Top)),
            mapped: telar::signal(true).read_only(),
        });
        let layer =
            surfaces::area::stand_in(&screen(), LayerKind::Top).expect("the top layer builds");
        let page =
            telar::Container::new(LayoutStyle::new().width(1920.0).height(1080.0), vec![layer])
                .expect("a screen");
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        telar::compute_layout(
            root,
            telar::AvailableSpace::Definite(1920.0),
            telar::AvailableSpace::Definite(1080.0),
        )
        .expect("the top layer lays out");
        DRAWN.with(|drawn| *drawn.borrow_mut() = Some((scope.id(), tree)));
    }

    /// The chip `id`, drawn as the screen shows it now.
    fn chip(id: &str) -> Node {
        draw();
        rects::instance(Some(SCREEN), &InstanceId::new(id))
            .map(|(node, _)| node)
            .unwrap_or_else(|| panic!("`{id}` is drawn"))
    }

    /// What takes the bar `id` off the edited screen.
    fn removal(id: &AreaId) -> Vec<layout::LayoutOp> {
        let layout = session::draft().peek();
        let known = crate::written::known();
        crate::written::area_removal(
            &layout,
            &known,
            &screen().resolving(&layout, &known).resolved,
            LayerKind::Top,
            id,
            None,
        )
        .expect("the bar can be taken away")
    }

    fn without_bars(layout: &mut Layout) {
        for rule in &mut layout.outputs {
            rule.layers.top.areas.clear();
        }
    }

    /// Standing rules: a bar is made on every edge of a screen it has to itself, running the whole edge; an edge a bar already fills has no room for another, and the new one there fills what the others leave.
    #[test]
    fn a_bar_is_made_on_every_edge_running_what_the_edge_leaves_free() {
        let _owner = Owner::new();
        let rig = rig_with("top-create", without_bars);
        let _mode = enter(LayerKind::Top);
        for edge in Edge::ALL {
            let (ops, id) = top::created(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                edge,
                None,
            )
            .unwrap_or_else(|why| panic!("{edge:?}: {why}"));
            commit(ops);
            assert_eq!(id.as_str(), format!("bar-{}", edge.as_str()));
            assert_eq!(edge_of(&id), Some(edge));
            let span = span(&id);
            assert_eq!(
                span.along, span.run_length,
                "{edge:?}: a free edge is run whole"
            );
            assert!(
                matches!(
                    bar(&id).map(|area| area.kind),
                    Some(ResolvedAreaKind::Bar {
                        length: Extent::Fill,
                        ..
                    })
                ),
                "{edge:?}"
            );
            assert_eq!(rig.undo_label().as_deref(), Some("test"));
            let refused = top::created(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                edge,
                None,
            );
            assert!(
                refused.is_err(),
                "{edge:?}: a filled edge has no room for another bar"
            );
        }

        let (ops, _) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &AreaId::new("bar-top"),
            1200.0,
            &Drawn::default(),
        )
        .expect("the top bar splits");
        commit(ops);
        let freed = top::free_on(
            &screen(),
            Edge::Top,
            top::new_run(&screen(), Edge::Top),
            None,
        );
        assert!(
            freed.is_empty(),
            "two halves still fill the edge: {freed:?}"
        );
    }

    /// Standing rules: the shipped bar moved to every other edge, and back; on each it runs the whole edge, and a bar on an edge with another on it fits beside it instead of over it.
    #[test]
    fn a_bar_moves_to_every_edge_and_fits_beside_the_bars_already_there() {
        let _owner = Owner::new();
        let _rig = rig_with("top-move", |_| {});
        let _mode = enter(LayerKind::Top);
        for edge in [Edge::Left, Edge::Bottom, Edge::Right, Edge::Top] {
            let ops = top::moved_to_edge(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                &bar_top(),
                edge,
                None,
            )
            .unwrap_or_else(|why| panic!("{edge:?}: {why}"));
            commit(ops);
            assert_eq!(edge_of(&bar_top()), Some(edge));
            let span = span(&bar_top());
            assert_eq!(
                span.along, span.run_length,
                "{edge:?}: it runs the whole edge"
            );
        }

        let (ops, _) = top::created(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            Edge::Bottom,
            None,
        )
        .expect("a bottom bar");
        commit(ops);
        let (ops, half) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &AreaId::new("bar-bottom"),
            960.0,
            &Drawn::default(),
        )
        .expect("the bottom bar splits");
        commit(ops);
        commit(removal(&AreaId::new("bar-bottom")));
        let ops = top::moved_to_edge(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            Edge::Bottom,
            Some(100.0),
        )
        .expect("the top bar fits into the free half");
        commit(ops);
        let (moved, other) = (span(&bar_top()), span(&half));
        assert!(
            moved.end() <= other.at + 0.5,
            "the moved bar {moved:?} keeps clear of {other:?}"
        );
    }

    /// Standing rules: on every edge a bar grows and shrinks along it by a step and never into the bar beside it, and slides along it only as far as its neighbour.
    #[test]
    fn a_bar_resizes_and_slides_along_every_edge_up_to_its_neighbour() {
        for edge in Edge::ALL {
            let _owner = Owner::new();
            let _rig = rig_with("top-resize", without_bars);
            let _mode = enter(LayerKind::Top);
            let (ops, id) = top::created(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                edge,
                None,
            )
            .expect("a bar");
            commit(ops);
            let whole = span(&id);
            let middle = whole.at + whole.along / 2.0;
            let (ops, second) = top::split(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                &id,
                middle,
                &Drawn::default(),
            )
            .expect("it splits");
            commit(ops);
            let node = Node::area(Some(SCREEN), LayerKind::Top, &id);
            let selection = Selection::Area(node);
            let (backward, forward) = match edge.is_vertical() {
                true => (Direction::Up, Direction::Down),
                false => (Direction::Left, Direction::Right),
            };
            let shorter = crate::steps::resized(&selection, &session::draft().peek(), backward)
                .unwrap_or_else(|why| panic!("{edge:?}: {why}"));
            commit(shorter);
            assert_eq!(
                span(&id).along,
                whole.along / 2.0 - 16.0,
                "{edge:?}: one step shorter"
            );
            let longer =
                crate::steps::resized(&selection, &session::draft().peek(), forward).expect("back");
            commit(longer);
            assert!(
                crate::steps::resized(&selection, &session::draft().peek(), forward).is_err(),
                "{edge:?}: it cannot grow into {second}"
            );
            assert!(
                crate::steps::moved(&selection, &session::draft().peek(), forward).is_err(),
                "{edge:?}: it cannot slide into {second}"
            );
            let slid = crate::steps::moved(
                &Selection::Area(Node::area(Some(SCREEN), LayerKind::Top, &second)),
                &session::draft().peek(),
                backward,
            );
            assert!(
                slid.is_err(),
                "{edge:?}: nor can its neighbour slide into it"
            );
        }
    }

    /// The top bar dragged to the left edge lays its chips down the edge while the drag previews it, and the edges reserve what they did until it is let go — then the screen is brought in line once.
    #[test]
    fn a_bar_dragged_from_top_to_left_relays_its_chips_and_reserves_again_only_on_release() {
        let _owner = Owner::new();
        let rig = rig_with("top-drag", |_| {});
        let _mode = enter(LayerKind::Top);
        let before = screen().reserved;
        let reconciled = rig.reconciles.get();
        let edit = Edit::new("drag the bar");
        edit.begin().expect("the drag begins");
        for near in [100.0, 300.0, 500.0, 700.0] {
            let ops = top::moved_to_edge(
                &edit.transaction().before().expect("a snapshot"),
                &screen(),
                LayerKind::Top,
                &bar_top(),
                Edge::Left,
                Some(near),
            )
            .expect("it previews on the left");
            edit.preview(ops).expect("it previews");
            assert_eq!(screen().reserved, before, "a preview never re-reserves");
            assert_eq!(
                rig.reconciles.get(),
                reconciled,
                "nor brings the screen in line"
            );
        }
        assert_eq!(edge_of(&bar_top()), Some(Edge::Left), "the preview is live");
        let (workspaces, clock, notes) = (
            rects::rect(&chip("workspaces")).expect("drawn"),
            rects::rect(&chip("clock")).expect("drawn"),
            rects::rect(&chip("notes")).expect("drawn"),
        );
        assert!(
            workspaces.y < clock.y && clock.y < notes.y,
            "down the edge, start to end: {workspaces:?} {clock:?} {notes:?}"
        );
        assert!(
            (workspaces.x - notes.x).abs() < 1.0 && (clock.x - notes.x).abs() < 1.0,
            "in one column: {workspaces:?} {clock:?} {notes:?}"
        );

        edit.commit().expect("the drag is let go");
        assert_eq!(
            rig.reconciles.get(),
            reconciled + 1,
            "one reconcile, on release"
        );
        assert_eq!(edge_of(&bar_top()), Some(Edge::Left));
        assert_eq!(rig.undo_label().as_deref(), Some("drag the bar"));
    }

    /// What an edge reserves follows a bar only when the move is committed: during the drag the committed arrangement and the drawn one reserve the same, and on release the edges trade their reservation in one reconcile.
    #[test]
    fn reservation_changes_once_on_commit_and_never_during_the_drag() {
        let _owner = Owner::new();
        let rig = rig_with("top-reserve", |layout| {
            for rule in &mut layout.outputs {
                for area in &mut rule.layers.top.areas {
                    area.reserve = Some(true);
                }
            }
        });
        let _mode = enter(LayerKind::Top);
        let reserved =
            |desktop: &reconcile::Desktop| Edge::ALL.map(|edge| desktop.resolved.reserved(edge));
        let planned_before = reserved(&reconcile::planned()[0]);
        assert_eq!(
            planned_before,
            [34.0, 0.0, 0.0, 0.0],
            "the shipped bar reserves the top"
        );
        let reconciled = rig.reconciles.get();
        let edit = Edit::new("drag the bar");
        edit.begin().expect("the drag begins");
        for edge in [Edge::Left, Edge::Bottom, Edge::Right, Edge::Left] {
            let ops = top::moved_to_edge(
                &edit.transaction().before().expect("a snapshot"),
                &screen(),
                LayerKind::Top,
                &bar_top(),
                edge,
                None,
            )
            .expect("it previews");
            edit.preview(ops).expect("it previews");
            assert_eq!(
                screen().reserved,
                reconcile::planned()[0].reserved,
                "{edge:?}: the drawn screen reserves what was committed"
            );
            assert_eq!(
                reserved(&reconcile::planned()[0]),
                planned_before,
                "{edge:?}"
            );
        }
        assert_eq!(
            rig.reconciles.get(),
            reconciled,
            "no reconcile during the drag"
        );
        edit.commit().expect("let go");
        assert_eq!(
            rig.reconciles.get(),
            reconciled + 1,
            "one reconcile on release"
        );
        assert_eq!(
            reserved(&reconcile::planned()[0]),
            [0.0, 0.0, 34.0, 0.0],
            "the left edge reserves now, and the top no longer"
        );
    }

    /// Trust decides what runs, not what a file says (DEC-30): splitting a bar of a bundle's file gives the new half the bar's own actions as the file writes them, its held `shell run` line included, and the chips moved onto it keep theirs — still held, rather than erased by the split.
    #[test]
    fn a_split_keeps_the_lines_a_bundle_holds_back() {
        const HELD: &str = "shell run date";
        let held = || layout::Action(vec![HELD.to_string()]);
        let _owner = Owner::new();
        let rig = rig_prepared(
            "top-split-held",
            |layout| {
                let bar = &mut layout.outputs[0].layers.top.areas[0];
                bar.actions.insert(layout::Trigger::Press, held());
                for group in &mut bar.groups {
                    for child in &mut group.children {
                        if child.id.as_str() == "clock" {
                            child.actions.insert(layout::Trigger::Press, held());
                        }
                    }
                }
            },
            |store| {
                let mut trust = layout::Trust::default();
                trust.import("layouts/mine.toml", "nord");
                store.set_trust(trust);
            },
        );
        let _mode = enter(LayerKind::Top);
        draw();
        let drawn = Drawn::of(Some(SCREEN), LayerKind::Top, &bar_top(), Edge::Top);
        let (ops, second) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            700.0,
            &drawn,
        )
        .expect("it splits");
        commit(ops);

        let written = stored(&rig);
        let areas = &written.outputs[0].layers.top.areas;
        let rest = areas
            .iter()
            .find(|area| area.id == second)
            .expect("the new half is written");
        assert_eq!(rest.actions.get(&layout::Trigger::Press), Some(&held()));
        let clock = rest
            .groups
            .iter()
            .flat_map(|group| group.children.iter())
            .find(|child| child.id.as_str() == "clock")
            .expect("the clock moved onto the new half");
        assert_eq!(clock.actions.get(&layout::Trigger::Press), Some(&held()));
        assert!(
            bar(&second).is_some_and(|area| area.actions.values().all(|it| it.0.is_empty())),
            "still held on screen"
        );
    }

    /// A split and a join are each other's undoing: the bar cut in two, its chips past the cut on the second half in the zones they were in, and joined back it is the layout it was.
    #[test]
    fn a_split_bar_joined_again_is_the_bar_it_was() {
        let _owner = Owner::new();
        let rig = rig_with("top-split-join", |_| {});
        let _mode = enter(LayerKind::Top);
        let original = stored(&rig);
        draw();
        let drawn = Drawn::of(Some(SCREEN), LayerKind::Top, &bar_top(), Edge::Top);
        let (ops, second) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            700.0,
            &drawn,
        )
        .expect("it splits");
        commit(ops);
        assert_eq!(second.as_str(), "bar-top-2");
        assert_eq!(zone(&bar_top(), Zone::Start), ["workspaces"]);
        assert!(zone(&bar_top(), Zone::Center).is_empty());
        assert_eq!(zone(&second, Zone::Center), ["clock"]);
        assert_eq!(zone(&second, Zone::End), ["notes"]);
        let (first, rest) = (span(&bar_top()), span(&second));
        assert_eq!(first.end(), 700.0);
        assert_eq!(rest.at, 700.0);
        assert_eq!(rest.end(), first.run_end());

        let splitting = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            20.0,
            &drawn,
        );
        assert!(
            splitting.is_err(),
            "a part shorter than the shortest bar is refused"
        );

        let ops = top::joined(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &second,
            &bar_top(),
        )
        .expect("they join, in either order");
        commit(ops);
        assert_eq!(
            stored(&rig),
            original,
            "joined again, the layout is what it was"
        );
    }

    /// A join keeps what each bar runs on its own background: a gesture only the second bar binds goes with it onto the joined bar and one both bind alike stays, while one they bind to different chains refuses the join, naming the gesture, rather than dropping either.
    #[test]
    fn a_join_keeps_both_bars_own_actions_or_names_the_gesture_they_disagree_on() {
        let chain = |line: &str| layout::Action(vec![line.to_string()]);
        let _owner = Owner::new();
        let rig = rig_with("top-join-actions", |layout| {
            layout.outputs[0].layers.top.areas[0].actions.insert(
                layout::Trigger::Press,
                layout::Action(vec!["panel toggle clock".to_string()]),
            );
        });
        let _mode = enter(LayerKind::Top);
        draw();
        let drawn = Drawn::of(Some(SCREEN), LayerKind::Top, &bar_top(), Edge::Top);
        let (ops, second) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            700.0,
            &drawn,
        )
        .expect("it splits");
        commit(ops);
        let site = layout::Site {
            output: stored(&rig).outputs[0].matches.clone(),
            workspace: None,
            layer: LayerKind::Top,
        };
        let bind = |actions: &[(layout::Trigger, &str)]| {
            commit(vec![layout::LayoutOp::SetAreaActions {
                site: site.clone(),
                id: second.clone(),
                actions: actions
                    .iter()
                    .map(|(trigger, line)| (*trigger, chain(line)))
                    .collect(),
            }]);
        };
        let join = || {
            top::joined(
                &session::draft().peek(),
                &screen(),
                LayerKind::Top,
                &bar_top(),
                &second,
            )
        };

        bind(&[(layout::Trigger::Press, "launcher toggle")]);
        match join() {
            Err(EditError::Refused(why)) => assert!(why.contains("press"), "{why}"),
            other => panic!("the two presses disagree, so the join is refused: {other:?}"),
        }
        assert!(
            stored(&rig).outputs[0]
                .layers
                .top
                .areas
                .iter()
                .any(|area| area.id == second)
        );

        bind(&[
            (layout::Trigger::Press, "panel toggle clock"),
            (layout::Trigger::ScrollUp, "volume up"),
        ]);
        commit(join().expect("nothing they bind disagrees"));
        let joined = stored(&rig);
        let areas = &joined.outputs[0].layers.top.areas;
        assert!(areas.iter().all(|area| area.id != second));
        let kept = &areas
            .iter()
            .find(|area| area.id == bar_top())
            .expect("the joined bar")
            .actions;
        assert_eq!(
            kept.get(&layout::Trigger::Press),
            Some(&chain("panel toggle clock"))
        );
        assert_eq!(
            kept.get(&layout::Trigger::ScrollUp),
            Some(&chain("volume up")),
            "the second bar's own gesture goes with it"
        );
    }

    /// Chips go where the insertion line says: into another zone before or after the chips there, onto another bar on another edge — into a zone it had no group for — and a move to where a chip already is changes nothing.
    #[test]
    fn a_chip_lands_at_its_insertion_index_in_any_zone_of_any_bar() {
        let _owner = Owner::new();
        let _rig = rig_with("top-chips", |_| {});
        let _mode = enter(LayerKind::Top);
        let land = |node: &Node, area: &AreaId, zone: Zone, index: usize| {
            let ops = top::chip_moved(
                &session::draft().peek(),
                &screen(),
                node,
                &ChipLanding {
                    area: area.clone(),
                    zone,
                    index,
                },
            )
            .expect("it lands");
            commit(ops);
        };
        land(&chip("clock"), &bar_top(), Zone::Start, 0);
        assert_eq!(zone(&bar_top(), Zone::Start), ["clock", "workspaces"]);
        land(&chip("clock"), &bar_top(), Zone::Start, 1);
        assert_eq!(zone(&bar_top(), Zone::Start), ["workspaces", "clock"]);
        let unchanged = top::chip_moved(
            &session::draft().peek(),
            &screen(),
            &chip("clock"),
            &ChipLanding {
                area: bar_top(),
                zone: Zone::Start,
                index: 1,
            },
        )
        .expect("a move to where it is");
        assert!(unchanged.is_empty(), "changes nothing");

        let (ops, left) = top::created(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            Edge::Left,
            None,
        )
        .expect("a left bar");
        commit(ops);
        land(&chip("notes"), &left, Zone::End, 0);
        assert_eq!(zone(&left, Zone::End), ["notes"]);
        assert!(zone(&bar_top(), Zone::End).is_empty());
        land(&chip("clock"), &left, Zone::End, 0);
        assert_eq!(
            zone(&left, Zone::End),
            ["clock", "notes"],
            "before the chip it was dropped ahead of"
        );
    }

    /// "Move to <screen>": a bar every screen shares leaves this one alone and stays on the other; a bar only this screen writes is written for the other screen instead — and one undo takes either back.
    #[test]
    fn a_bar_moves_to_another_screen_by_its_rule_and_leaves_its_own() {
        const OTHER: &str = "HDMI-A-1";
        let _owner = Owner::new();
        let rig = rig_screens("top-output", &[SCREEN, OTHER], |layout| {
            let mut own = layout::default_bar(AreaId::new("bar-dp"), Edge::Bottom);
            own.reserve = Some(false);
            let mut rule = layout::OutputRule {
                matches: layout::OutputMatch(SCREEN.to_string()),
                ..layout::OutputRule::default()
            };
            rule.layers.top.areas.push(own);
            layout.outputs.push(rule);
        });
        let _mode = enter(LayerKind::Top);
        let placed_on = |output: &str, id: &AreaId| {
            reconcile::planned()
                .iter()
                .find(|desktop| desktop.output.as_deref() == Some(output))
                .and_then(|desktop| desktop.resolved.layer(LayerKind::Top).cloned())
                .is_some_and(|layer| layer.areas.iter().any(|area| area.id == *id))
        };
        context::open(Asked {
            node: Node::area(Some(SCREEN), LayerKind::Top, &bar_top()),
            window: LayerKind::Top,
            at: None,
        })
        .expect("the bar's menu opens");
        let menu = context::rows();
        assert!(menu.contains(&format!("Move to {OTHER}")), "{menu:?}");
        transient::close(context::ID);

        let original = stored(&rig);
        let from = screen();
        let onto = reconcile::desktops()
            .iter()
            .find(|desktop| desktop.output.as_deref() == Some(OTHER))
            .cloned()
            .expect("the other screen");
        let ops = top::moved_to_output(
            &session::draft().peek(),
            &from,
            &onto,
            LayerKind::Top,
            &bar_top(),
        )
        .expect("the shared bar moves");
        commit(ops);
        assert!(!placed_on(SCREEN, &bar_top()));
        assert!(placed_on(OTHER, &bar_top()));
        assert!(
            stored(&rig).outputs.iter().any(|rule| rule.matches.0 == SCREEN && rule.layers.top.remove.contains(&bar_top())),
            "hidden on this screen by its own rule"
        );
        session::undo().expect("undo");
        assert_eq!(stored(&rig), original);

        let bottom = AreaId::new("bar-dp");
        let written_here = crate::written::Written::area(
            &session::draft().peek(),
            Some(SCREEN),
            LayerKind::Top,
            &bottom,
            None,
        )
        .expect("written");
        assert!(written_here.is_present());
        let ops = top::moved_to_output(
            &session::draft().peek(),
            &screen(),
            &onto,
            LayerKind::Top,
            &bottom,
        )
        .expect("the bottom bar moves");
        commit(ops);
        assert!(!placed_on(SCREEN, &bottom));
        assert!(placed_on(OTHER, &bottom));
        session::undo().expect("undo");
        assert_eq!(stored(&rig), original);
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

    fn shaped(mode: Shape) -> impl FnOnce(&mut Layout) {
        move |layout: &mut Layout| {
            for rule in &mut layout.outputs {
                for area in &mut rule.layers.top.areas {
                    if let Some(layout::AreaKind::Bar { shape, .. }) = &mut area.kind {
                        shape.mode = Some(mode);
                    }
                }
            }
        }
    }

    /// Standing rules: in every shape mode and on every edge, the mode's tools build over a bar and its halves, and so does a bar's popover.
    #[test]
    fn the_top_tools_and_a_bars_popover_build_on_every_edge_in_every_shape() {
        for shape in [Shape::Bar, Shape::Sections, Shape::Chips] {
            for edge in Edge::ALL {
                let _owner = Owner::new();
                let _rig = rig_with("top-builds", shaped(shape));
                let _mode = enter(LayerKind::Top);
                if edge != Edge::Top {
                    commit(
                        top::moved_to_edge(
                            &session::draft().peek(),
                            &screen(),
                            LayerKind::Top,
                            &bar_top(),
                            edge,
                            None,
                        )
                        .expect("it moves"),
                    );
                }
                draw();
                let whole = span(&bar_top());
                let drawn = Drawn::of(Some(SCREEN), LayerKind::Top, &bar_top(), edge);
                let (ops, _) = top::split(
                    &session::draft().peek(),
                    &screen(),
                    LayerKind::Top,
                    &bar_top(),
                    whole.at + whole.along / 2.0,
                    &drawn,
                )
                .unwrap_or_else(|why| panic!("{shape:?} {edge:?}: {why}"));
                commit(ops);
                draw();
                let mode = mode::current().expect("the mode is up");
                crate::modes::bars::tool(&mode)
                    .unwrap_or_else(|why| panic!("{shape:?} {edge:?}: {why}"));
                popover::open_area(Node::area(Some(SCREEN), LayerKind::Top, &bar_top()))
                    .expect("the bar's popover opens");
                popover::tree().expect("a popover").expect("it builds");
                popover::close();
            }
        }
    }

    /// Whichever row or handle moves it, a bar's popover keeps it clear of the bar beside it: slid or stretched towards its neighbour it stops at the seam, and asked to run the whole edge it runs what is free of it.
    #[test]
    fn a_bars_popover_keeps_it_clear_of_the_bar_beside_it() {
        let _owner = Owner::new();
        let rig = rig_with("top-popover", |_| {});
        let _mode = enter(LayerKind::Top);
        let (ops, second) = top::split(
            &session::draft().peek(),
            &screen(),
            LayerKind::Top,
            &bar_top(),
            960.0,
            &Drawn::default(),
        )
        .expect("it splits");
        commit(ops);
        draw();
        popover::open_area(Node::area(Some(SCREEN), LayerKind::Top, &bar_top()))
            .expect("the popover opens");
        let _rows = popover::tree().expect("a popover").expect("it builds");
        let offset = popover::shared::<f32>("offset").expect("an offset");
        let length = popover::shared::<f32>("length").expect("a length");
        let fills = popover::shared::<bool>("length_fill").expect("a full length switch");
        length.set(1500.0);
        assert_eq!(length.peek(), 960.0, "it stops where the next bar starts");
        length.set(600.0);
        offset.set(900.0);
        assert_eq!(offset.peek(), 360.0, "it slides as far as the next bar");
        fills.set(true);
        assert!(
            !fills.peek(),
            "running the whole edge would run over its neighbour"
        );
        assert_eq!(
            (offset.peek(), length.peek()),
            (0.0, 960.0),
            "so it runs what is free"
        );
        popover::close();
        let first = span(&bar_top());
        assert!(first.end() <= span(&second).at + 0.5, "{first:?}");
        assert!(rig.undo_label().is_some());
    }

    /// Every drag has a key: Ctrl+Shift+arrows make a bar on that edge, `s` splits the selected bar, Alt+Shift+arrows join it with the bar that way — each one entry in the history.
    #[test]
    fn the_keys_make_split_and_join_bars() {
        let _owner = Owner::new();
        let rig = rig_with("top-keys", |_| {});
        let _mode = enter(LayerKind::Top);
        let original = stored(&rig);
        draw();
        assert!(session::select(Selection::Area(Node::area(
            Some(SCREEN),
            LayerKind::Top,
            &bar_top()
        ))));
        assert!(tap(Key::Char('s'), NONE), "`s` splits");
        assert!(
            bar(&AreaId::new("bar-top-2")).is_some(),
            "the second half is there"
        );
        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_alt: true,
                is_shift: true,
                ..NONE
            }
        ));
        assert_eq!(stored(&rig), original, "joined back, it is the bar it was");

        session::clear_selection();
        assert!(tap(
            Key::Named(NamedKey::ArrowDown),
            ModifiersState {
                is_ctrl: true,
                is_shift: true,
                ..NONE
            }
        ));
        assert_eq!(
            edge_of(&AreaId::new("bar-bottom")),
            Some(Edge::Bottom),
            "a bar at the bottom"
        );
        let history = rig.store.borrow().undo_label().map(str::to_string);
        assert!(history.is_some());
    }

    /// A5: a chip carried into a zone the bar has no group for makes one named after the zone, past every id a level of the layout gives that bar — here one this very entry takes away.
    #[test]
    fn a_new_zone_group_never_revives_an_id_a_level_took_away() {
        let _owner = Owner::new();
        let rig = rig_with("top-group-id", |layout| {
            let bar = layout.outputs[0]
                .layers
                .top
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "bar-top")
                .expect("the shipped bar");
            bar.groups.retain(|group| group.id.as_str() != "end");
            bar.remove.push(layout::GroupId::new("end"));
        });
        let _mode = enter(LayerKind::Top);
        let ops = top::chip_moved(
            &session::draft().peek(),
            &screen(),
            &chip("clock"),
            &ChipLanding {
                area: bar_top(),
                zone: Zone::End,
                index: 0,
            },
        )
        .expect("it lands");
        commit(ops);
        assert_eq!(zone(&bar_top(), Zone::End), ["clock"]);
        let written: Vec<String> = stored(&rig).outputs[0]
            .layers
            .top
            .areas
            .iter()
            .find(|area| area.id == bar_top())
            .expect("the bar")
            .groups
            .iter()
            .map(|group| group.id.to_string())
            .collect();
        assert_eq!(written, ["start", "center", "end-2"]);
    }

    /// A3: the top layer has no workspace variants, since its bars reserve space (TA-2): its mode refuses the switch and its popovers offer none, and a bar edited from another mode with the switch on is refused rather than written into the workspace's rule.
    #[test]
    fn bars_are_never_edited_for_one_workspace() {
        let _owner = Owner::new();
        let rig = rig_on("top-variant", Some("2"), |_| {});
        let _mode = enter(LayerKind::Top);
        assert!(!variant::allowed(LayerKind::Top));
        assert_eq!(
            variant::set(true),
            Err(EditError::Refused(
                "Bars reserve space, which a workspace rule may not change, so the top layer has no workspace variants".to_string()
            ))
        );
        assert_eq!(variant::active(), None, "so no popover offers it");
        mode::leave();

        let _desktop = enter(LayerKind::Desktop);
        variant::set(true).expect("the desktop has variants");
        let before = stored(&rig);
        let edge = edge_of(&bar_top()).expect("the bar's edge");
        let middle = span(&bar_top());
        let drawn = Drawn::of(Some(SCREEN), LayerKind::Top, &bar_top(), edge);
        let split = top::split_at(
            &Node::area(Some(SCREEN), LayerKind::Top, &bar_top()),
            middle.at + middle.along / 2.0,
            &drawn,
        );
        assert!(split.is_err(), "{split:?}");
        assert_eq!(stored(&rig), before, "nothing is written for the workspace");
        assert!(stored(&rig).outputs[0].workspaces.is_empty());
    }

    /// The bar's own box takes the pointer through every layer the top tools lay over the screen: pressed between its chips and carried to the foot of the screen, it previews on the bottom edge.
    #[test]
    fn a_bar_carried_by_the_pointer_previews_on_the_edge_it_is_carried_to() {
        let _owner = Owner::new();
        let _rig = rig_with("top-pointer-drag", |_| {});
        let _mode = enter(LayerKind::Top);
        draw();
        let mode = mode::current().expect("the mode is up");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = Pointed::new(Box::new(
            telar::Container::new(
                page,
                vec![crate::modes::bars::tool(&mode).expect("the top tools build")],
            )
            .expect("a page"),
        ));
        let node = root.layout_node();
        let mut tree = telar::ComponentList::new(root);
        telar::compute_layout(
            node,
            telar::AvailableSpace::Definite(1920.0),
            telar::AvailableSpace::Definite(1080.0),
        )
        .expect("the tools lay out");
        let moved = |x: f64, y: f64| Event::PointerMoved {
            x,
            y,
            source: telar::PointerSource::Mouse,
        };
        let between = (600.0, 17.0);
        for event in [
            moved(between.0, between.1),
            Event::PointerPressed {
                x: between.0,
                y: between.1,
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
            moved(600.0, 300.0),
            moved(960.0, 1060.0),
        ] {
            tree.on_event(&event);
        }
        assert_eq!(edge_of(&bar_top()), Some(Edge::Bottom));
        telar::dispatch_overlays(&Event::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: ModifiersState::default(),
        });
        assert_eq!(edge_of(&bar_top()), Some(Edge::Top), "Esc put it back");
    }

    fn seen(edge: Edge, rect: telar::Rect, zones: [&[(f32, f32)]; 3]) -> Seen {
        Seen {
            id: AreaId::new("bar"),
            edge,
            rect,
            zones: [
                (Zone::Start, zones[0].to_vec()),
                (Zone::Center, zones[1].to_vec()),
                (Zone::End, zones[2].to_vec()),
            ],
        }
    }

    /// A chip let go over a bar goes into the third of it the pointer is over, before the first chip there whose middle is past the pointer, and the line is drawn between the chips it goes between — along either axis.
    #[test]
    fn the_insertion_line_is_between_the_chips_the_pointer_is_between() {
        let zones: [&[(f32, f32)]; 3] = [&[(0.0, 40.0), (44.0, 84.0)], &[(580.0, 620.0)], &[]];
        for (edge, rect) in [
            (Edge::Top, telar::Rect::new(0.0, 0.0, 1200.0, 34.0)),
            (Edge::Left, telar::Rect::new(0.0, 0.0, 34.0, 1200.0)),
        ] {
            let bar = seen(edge, rect, zones);
            let (landing, line) = bar.landing(50.0);
            assert_eq!((landing.zone, landing.index), (Zone::Start, 1), "{edge:?}");
            assert_eq!(line, 42.0, "{edge:?}: between the two start chips");
            let (landing, line) = bar.landing(10.0);
            assert_eq!((landing.zone, landing.index), (Zone::Start, 0), "{edge:?}");
            assert_eq!(line, -2.0, "{edge:?}: ahead of the first");
            let (landing, _) = bar.landing(700.0);
            assert_eq!((landing.zone, landing.index), (Zone::Center, 1), "{edge:?}");
            let (landing, line) = bar.landing(1000.0);
            assert_eq!((landing.zone, landing.index), (Zone::End, 0), "{edge:?}");
            assert_eq!(
                line, 1196.0,
                "{edge:?}: an empty end zone takes it at the far end"
            );
            let across = bar.line_at(42.0);
            match edge.is_vertical() {
                true => assert_eq!(across, telar::Rect::new(0.0, 41.0, 34.0, 2.0)),
                false => assert_eq!(across, telar::Rect::new(41.0, 0.0, 2.0, 34.0)),
            }
        }
    }

    /// A drag lands a bar on whichever edge the pointer is nearest.
    #[test]
    fn a_dragged_bar_goes_to_the_edge_the_pointer_is_nearest() {
        let size = (1920.0, 1080.0);
        assert_eq!(nearest_edge(size, (960.0, 20.0)), Edge::Top);
        assert_eq!(nearest_edge(size, (960.0, 1060.0)), Edge::Bottom);
        assert_eq!(nearest_edge(size, (30.0, 540.0)), Edge::Left);
        assert_eq!(nearest_edge(size, (1900.0, 540.0)), Edge::Right);
        assert_eq!(measured(Edge::Right, size, (1900.0, 540.0)), (20.0, 540.0));
    }
}
