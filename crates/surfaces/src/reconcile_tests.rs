//! What a reconcile has to keep answering: what an edge reserves, what a workspace rule may never change, and that every screen is planned on its own.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use platform_wayland::OutputDescriptor;

    use config::{Config, Edge};
    use layout::{
        ActiveWorkspace, Area, AreaId, AreaKind, AutoHide, BarShape, Extent, Group, GroupId,
        GroupKind, Instance, InstanceId, Layer, LayerKind, Layers, Layout, LayoutId, OutputMatch,
        OutputRule, Representation, SessionLayers, WorkspaceMatch, WorkspaceRule, Zone,
    };

    use crate::layer_window::Content;
    use crate::layer_window::Reserved;
    use crate::reconcile::{self, Desktop, Shell, plan, stacks};

    const SCREEN: &str = "DP-1";

    fn screen(name: &str, size: (i32, i32)) -> OutputDescriptor {
        OutputDescriptor {
            name: Some(name.to_string()),
            logical_size: Some(size),
            position: (0, 0),
            scale: 1,
        }
    }

    /// The config path a plan resolves per-monitor overrides against. Under the scratch root every test gets, never a literal relative path: `Config::for_output` reads and seeds beside whatever it is handed, and a bare `"config.toml"` seeds one into the crate directory (F-1.11).
    fn config_path() -> std::path::PathBuf {
        util::paths::config_dir().join("config.toml")
    }

    fn outputs() -> Vec<OutputDescriptor> {
        vec![screen(SCREEN, (1920, 1080))]
    }

    fn layout_of(top: Vec<Area>, workspaces: Vec<WorkspaceRule>) -> Layout {
        Layout {
            id: LayoutId::new("test"),
            name: "Test".into(),
            extends: None,
            sources: Default::default(),
            outputs: vec![OutputRule {
                matches: OutputMatch("*".into()),
                layers: Layers {
                    top: Layer {
                        areas: top,
                        remove: Vec::new(),
                    },
                    overlay: Layer {
                        areas: vec![stack()],
                        remove: Vec::new(),
                    },
                    ..Layers::default()
                },
                workspaces,
            }],
        }
    }

    fn stack() -> Area {
        Area {
            id: AreaId::new("stack"),
            kind: Some(AreaKind::Stack {
                anchor: Some(layout::Anchor::TopRight),
                offset: None,
                width: Some(380.0),
                flow: None,
                output_policy: None,
                routes: Vec::new(),
                launcher: None,
            }),
            ..Area::default()
        }
    }

    fn bar(id: &str, edge: Edge, thickness: f32, modules: &[&str]) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::Bar {
                edge: Some(edge),
                thickness: Some(thickness),
                length: Some(Extent::Fill),
                offset: Some(0.0),
                shape: BarShape::default(),
                autohide: None,
            }),
            reserve: Some(true),
            groups: vec![Group {
                id: GroupId::new("start"),
                kind: Some(GroupKind::Zone { zone: Zone::Start }),
                children: modules
                    .iter()
                    .map(|module| Instance {
                        id: InstanceId::new(*module),
                        module: Some((*module).to_string()),
                        representation: Some(Representation::Chip),
                        ..Instance::default()
                    })
                    .collect(),
                ..Group::default()
            }],
            ..Area::default()
        }
    }

    fn on(
        layout: &Layout,
        outputs: &[OutputDescriptor],
        workspace: Option<ActiveWorkspace>,
    ) -> (Vec<Desktop>, util::report::Report) {
        let config = Arc::new(Config::default());
        plan(
            &config_path(),
            &config,
            layout,
            &layout::Library::default(),
            outputs,
            &|_| workspace.clone(),
        )
    }

    fn planned(layout: &Layout) -> Vec<Desktop> {
        let (desktops, report) = on(layout, &outputs(), None);
        assert!(report.is_clean(), "{}", report.render());
        desktops
    }

    /// Notifications, toasts and OSDs go to a stack area and nowhere else, so a layout with none has quietly switched them all off — which is reported rather than left for the user to notice.
    #[test]
    fn a_layout_with_no_stack_area_is_reported() {
        let mut bare = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        );
        bare.outputs[0].layers.overlay.areas.clear();
        let (_, report) = on(&bare, &outputs(), None);
        assert!(
            report
                .findings()
                .any(|finding| finding.message.key() == Some("finding.no_stack")),
            "{}",
            report.render()
        );
    }

    /// Without `ext-background-effect-v1` an area styled to blur draws translucent and unblurred, and the report says so — there is no compositor in a test, so the protocol is absent here.
    #[test]
    fn an_area_that_asks_for_blur_the_compositor_cannot_give_is_reported() {
        let mut blurred = bar("bar-top", Edge::Top, 34.0, &["clock"]);
        blurred.style.backdrop = Some(layout::Backdrop::Blur);
        let mut layout = layout_of(vec![blurred], Vec::new());
        layout.outputs[0].layers.background.areas.push(Area {
            id: AreaId::new("frosted"),
            kind: Some(AreaKind::WallpaperRegion {
                rect: None,
                source: None,
                fit: None,
                transition: None,
            }),
            style: layout::Style {
                backdrop: Some(layout::Backdrop::Blur),
                ..layout::Style::default()
            },
            ..Area::default()
        });
        let (_, report) = on(&layout, &outputs(), None);
        assert!(
            report.findings().any(|finding| finding.key == "top.bar-top"
                && finding.message.key() == Some("finding.no_blur")),
            "{}",
            report.render()
        );
        assert!(
            !report
                .findings()
                .any(|finding| finding.key == "background.frosted"),
            "the shell blurs what the background's own surface drew, on any compositor: {}",
            report.render()
        );
    }

    /// The strip an edge commits is what its areas take plus the air they float at — the thing a bar cannot answer for itself and the thing a window most needs right.
    #[test]
    fn an_edge_reserves_what_its_areas_take_and_the_gap_they_float_at() {
        let desktops = planned(&layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        ));
        let only = &desktops[0];

        assert_eq!(only.output.as_deref(), Some(SCREEN));
        assert_eq!(only.size, (1920.0, 1080.0));
        let gap = only
            .config
            .gap_of(&crate::bar::bar_shape(&only.config, BarShape::default()))
            as f32;
        assert_eq!(
            only.reserved.on(Edge::Top),
            34.0 + gap,
            "a window must clear the bar and the air it floats in, or it tiles underneath it"
        );
        assert_eq!(
            (
                only.reserved.on(Edge::Left),
                only.reserved.on(Edge::Right),
                only.reserved.on(Edge::Bottom)
            ),
            (0.0, 0.0, 0.0),
            "and no other edge is taken"
        );
    }

    /// A bar that hides itself gives the screen back: only the strip it peeks with stays reserved, so a window is not tiled short of a bar the user asked to be able to ignore.
    #[test]
    fn an_autohiding_bar_reserves_only_its_peek() {
        let mut hiding = bar("bar-top", Edge::Top, 34.0, &["clock"]);
        let Some(AreaKind::Bar { autohide, .. }) = hiding.kind.as_mut() else {
            unreachable!("the helper builds a bar")
        };
        *autohide = Some(AutoHide {
            peek: 2.0,
            on_hover: true,
        });

        let desktops = planned(&layout_of(vec![hiding], Vec::new()));

        let gap = desktops[0].config.gap_of(&crate::bar::bar_shape(
            &desktops[0].config,
            BarShape::default(),
        )) as f32;
        assert_eq!(desktops[0].reserved.on(Edge::Top), 2.0 + gap);
    }

    /// **A workspace switch may never re-tile the user's windows.** A workspace rule can add and remove areas; the strip each edge commits is still the one the output's own rules asked for (F-6.7).
    #[test]
    fn no_workspace_rule_changes_what_an_edge_reserves() {
        let quiet = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        );
        let busy = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            vec![WorkspaceRule {
                matches: WorkspaceMatch("web".into()),
                layers: SessionLayers {
                    top: Layer {
                        areas: vec![bar("bar-bottom", Edge::Bottom, 40.0, &["notes"])],
                        remove: Vec::new(),
                    },
                    ..SessionLayers::default()
                },
            }],
        );

        let plain = planned(&quiet);
        let (ruled, _) = on(
            &busy,
            &outputs(),
            Some(ActiveWorkspace {
                name: "web".into(),
                id: None,
                special: None,
            }),
        );

        assert!(
            ruled[0]
                .resolved
                .layer(LayerKind::Top)
                .is_some_and(|top| top.areas.len() == 2),
            "the rule's bar is on screen while that workspace is active"
        );
        assert_eq!(
            (
                ruled[0].reserved.on(Edge::Top),
                ruled[0].reserved.on(Edge::Bottom)
            ),
            (
                plain[0].reserved.on(Edge::Top),
                plain[0].reserved.on(Edge::Bottom)
            ),
            "and it takes nothing off the screen, whichever workspace is active"
        );
    }

    /// What stays reserved on an edge and the fillet its corners take come from the output's own rules too, so a workspace rule that makes a reserving bar hide itself moves no corner and changes no run along the edge.
    #[test]
    fn no_workspace_rule_changes_what_an_edge_holds_or_how_its_corners_round() {
        let rounded = |id: &str, edge: Edge| {
            let mut side = bar(id, edge, 34.0, &["clock"]);
            if let Some(AreaKind::Bar { shape, .. }) = side.kind.as_mut() {
                shape.mode = Some(config::Shape::Bar);
                shape.fillet = Some(12.0);
            }
            side
        };
        let hidden = Area {
            id: AreaId::new("bar-left"),
            kind: Some(AreaKind::Bar {
                edge: None,
                thickness: None,
                length: None,
                offset: None,
                shape: BarShape::default(),
                autohide: Some(AutoHide {
                    peek: 2.0,
                    on_hover: true,
                }),
            }),
            ..Area::default()
        };
        let sides = || {
            vec![
                rounded("bar-top", Edge::Top),
                rounded("bar-left", Edge::Left),
            ]
        };
        let plain = planned(&layout_of(sides(), Vec::new()));
        let (ruled, _) = on(
            &layout_of(
                sides(),
                vec![WorkspaceRule {
                    matches: WorkspaceMatch("web".into()),
                    layers: SessionLayers {
                        top: Layer {
                            areas: vec![hidden],
                            remove: Vec::new(),
                        },
                        ..SessionLayers::default()
                    },
                }],
            ),
            &outputs(),
            Some(ActiveWorkspace {
                name: "web".into(),
                id: None,
                special: None,
            }),
        );

        let left = ruled[0]
            .resolved
            .area(LayerKind::Top, &AreaId::new("bar-left"))
            .expect("the left bar is drawn");
        assert!(
            matches!(
                left.kind,
                layout::ResolvedAreaKind::Bar {
                    autohide: Some(_),
                    ..
                }
            ),
            "the rule's autohide is what the workspace draws"
        );
        assert_eq!(plain[0].reserved.fillet_on(Edge::Left), Some(12.0));
        assert_eq!(
            ruled[0].reserved, plain[0].reserved,
            "and the edges, what stays on them and their corners are the output's own"
        );
    }

    /// `above_fullscreen` survives resolution, because the window host is what reads it off and the layer an area lands on is the only place that answer can be given.
    #[test]
    fn an_area_above_fullscreen_keeps_saying_so_after_it_resolves() {
        let mut over = bar("bar-top", Edge::Top, 34.0, &["clock"]);
        over.above_fullscreen = Some(true);

        let desktops = planned(&layout_of(vec![over], Vec::new()));

        let top = desktops[0]
            .resolved
            .layer(LayerKind::Top)
            .expect("the top layer");
        assert!(top.areas[0].above_fullscreen);
    }

    /// Resolution is per output and independent, which is what lets a monitor be re-planned on hotplug without touching the screens that were already there.
    #[test]
    fn every_output_is_planned_on_its_own() {
        let layout = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        );
        let two = vec![
            screen(SCREEN, (1920, 1080)),
            screen("HDMI-A-1", (2560, 1440)),
        ];

        let (desktops, _) = on(&layout, &two, None);

        assert_eq!(desktops.len(), 2);
        assert_eq!(desktops[0].size, (1920.0, 1080.0));
        assert_eq!(desktops[1].size, (2560.0, 1440.0));
        assert_eq!(
            desktops[0].reserved.on(Edge::Top),
            desktops[1].reserved.on(Edge::Top),
            "a `*` rule reaches both screens"
        );
    }

    /// A screen the compositor has not sized yet is still a screen: a window built against zero would lay every area out at nothing and be thrown away the moment the size arrived.
    #[test]
    fn an_output_with_no_size_yet_is_planned_at_a_usable_one() {
        let layout = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        );
        let unsized_screen = vec![OutputDescriptor {
            name: Some(SCREEN.to_string()),
            logical_size: None,
            position: (0, 0),
            scale: 1,
        }];

        let (desktops, _) = on(&layout, &unsized_screen, None);

        assert!(desktops[0].size.0 > 0.0 && desktops[0].size.1 > 0.0);
    }

    /// A layout that resolves nothing still plans a screen, so its windows are there to be hidden rather than absent — and nothing reserves.
    #[test]
    fn an_empty_layout_plans_a_screen_that_reserves_nothing() {
        let desktops = planned(&layout_of(Vec::new(), Vec::new()));

        assert_eq!(desktops.len(), 1);
        assert_eq!(desktops[0].reserved, Reserved::default());
        assert!(
            desktops[0]
                .resolved
                .layer(LayerKind::Top)
                .is_none_or(|top| top.areas.is_empty())
        );
    }

    /// Whatever a layout could not answer is reported and missing from the arrangement, never fatal: one unfinished area is not a reason for a user to lose their desktop.
    #[test]
    fn an_unfinished_area_is_reported_and_the_rest_still_resolves() {
        let mut half_written = bar("bar-left", Edge::Left, 40.0, &["clock"]);
        let Some(AreaKind::Bar { thickness, .. }) = half_written.kind.as_mut() else {
            unreachable!("the helper builds a bar")
        };
        *thickness = None;
        let layout = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"]), half_written],
            Vec::new(),
        );

        let (desktops, report) = on(&layout, &outputs(), None);

        assert!(!report.is_clean(), "the missing thickness is said out loud");
        let top = desktops[0]
            .resolved
            .layer(LayerKind::Top)
            .expect("the top layer");
        assert_eq!(top.areas.len(), 1, "and only the area it names is dropped");
        assert_eq!(top.areas[0].id, AreaId::new("bar-top"));
    }

    /// The windows and the strips are handed one number, so a bar and the zone carved for it can never disagree about the screen.
    #[test]
    fn the_windows_and_the_strips_measure_the_same_screen() {
        let desktops = planned(&layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        ));
        let handed = desktops[0].plan();

        assert_eq!(handed.reserved, desktops[0].reserved);
        assert_eq!(handed.size, desktops[0].size);
        assert_eq!(handed.output, desktops[0].output.as_deref());
    }

    /// A rule matching on a Hyprland-only prefix, on a compositor that cannot answer it, is reported as inactive rather than quietly never matching (DEC-16).
    #[test]
    fn a_rule_the_compositor_cannot_answer_is_reported_rather_than_silent() {
        let layout = layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            vec![WorkspaceRule {
                matches: WorkspaceMatch("id:3".into()),
                layers: SessionLayers::default(),
            }],
        );

        let (_, report) = on(
            &layout,
            &outputs(),
            Some(ActiveWorkspace {
                name: "3".into(),
                id: None,
                special: None,
            }),
        );

        assert!(
            !report.is_clean(),
            "a rule that can never fire has to be visible, not mysterious"
        );
    }

    /// An area is planned onto the layer it was written on, and the layers nobody wrote to stay empty — which is what lets their windows stay off the screen.
    #[test]
    fn a_bar_is_planned_onto_the_layer_it_was_written_on() {
        let desktops = planned(&layout_of(
            vec![bar("bar-top", Edge::Top, 34.0, &["clock"])],
            Vec::new(),
        ));

        assert!(
            desktops[0]
                .resolved
                .layer(LayerKind::Top)
                .is_some_and(|top| !top.areas.is_empty())
        );
        for quiet in [LayerKind::Background, LayerKind::Desktop] {
            assert!(
                desktops[0]
                    .resolved
                    .layer(quiet)
                    .is_none_or(|layer| layer.areas.is_empty()),
                "{quiet} was never written to"
            );
        }
    }

    /// What an edit mode draws its reference outlines from is what the windows were handed, output by output, and it is there once the reconcile is done.
    #[test]
    fn the_published_arrangement_is_what_the_plan_resolved() {
        // Nothing reserves: a strip queued before any window would make the driver's surface queue outlive telar's surface table when the test thread exits.
        let layout = layout_of(Vec::new(), Vec::new());
        let two = vec![
            screen(SCREEN, (1920, 1080)),
            screen("HDMI-A-1", (2560, 1440)),
        ];
        let (desktops, _) = on(&layout, &two, None);
        let mut shell = Shell::new();

        shell.reconcile(&desktops, Content::Rebuild);

        let published = reconcile::desktops();
        assert_eq!(published.len(), desktops.len());
        for (published, planned) in published.iter().zip(&desktops) {
            assert_eq!(published.output, planned.output);
            assert_eq!(
                format!("{:?}", published.resolved),
                format!("{:?}", planned.resolved)
            );
            assert_eq!(published.reserved, planned.reserved);
            assert_eq!(published.size, planned.size);
        }
        assert_eq!(
            stacks().len(),
            2,
            "the stack sites are read off the same publication"
        );
    }

    /// An edit mode learns that the output it edits went away from the arrangement it already reads, after the windows on that output are gone.
    #[test]
    fn an_output_going_away_is_news_to_whoever_reads_the_arrangement() {
        let layout = layout_of(Vec::new(), Vec::new());
        let two = vec![
            screen(SCREEN, (1920, 1080)),
            screen("HDMI-A-1", (2560, 1440)),
        ];
        let mut shell = Shell::new();
        shell.reconcile(&on(&layout, &two, None).0, Content::Rebuild);

        let seen: Rc<RefCell<Vec<Vec<Option<String>>>>> = Rc::default();
        let _watching = telar::effect({
            let seen = Rc::clone(&seen);
            move || {
                let outputs = reconcile::desktops()
                    .iter()
                    .map(|it| it.output.clone())
                    .collect();
                seen.borrow_mut().push(outputs);
            }
        });
        shell.reconcile(&on(&layout, &outputs(), None).0, Content::Changed);

        assert_eq!(
            seen.borrow().last().cloned(),
            Some(vec![Some(SCREEN.to_string())]),
            "the reader ran again and found one screen: {:?}",
            seen.borrow()
        );
        assert!(
            !shell.windows().is_open(Some("HDMI-A-1"), LayerKind::Top),
            "by the time it is told, the windows on the screen that left are gone"
        );
    }

    fn site(output: &str, area: &str, routes: Vec<layout::Route>) -> reconcile::StackSite {
        reconcile::StackSite {
            output: Some(output.to_string()),
            layer: LayerKind::Overlay,
            area: AreaId::new(area),
            policy: layout::StackOutputPolicy::Here,
            routes,
        }
    }

    /// F-2.8, per output: a card is offered only the stacks of the output it is shown on — the first route there that takes it, else the first stack there that routes nothing — and never one on another screen.
    #[test]
    fn a_card_lands_in_a_stack_of_its_own_output() {
        let critical = layout::Route {
            kind: Some(layout::CardKind::Notification),
            app: None,
            urgency: Some(layout::Urgency::Critical),
        };
        let sites = vec![
            site("DP-1", "corner", Vec::new()),
            site("HDMI-A-1", "critical", vec![critical.clone()]),
            site("DP-1", "critical", vec![critical]),
            site("HDMI-A-1", "corner", Vec::new()),
        ];
        let urgent = layout::RoutedCard {
            kind: layout::CardKind::Notification,
            app: Some("mail"),
            urgency: Some(layout::Urgency::Critical),
        };
        let toast = layout::RoutedCard {
            kind: layout::CardKind::Toast,
            app: None,
            urgency: None,
        };
        let landing = |output: &str, card: &layout::RoutedCard| {
            reconcile::stack_for(&sites, &Some(output.to_string()), card).map(|site| {
                (
                    site.output.clone().unwrap_or_default(),
                    site.area.to_string(),
                )
            })
        };
        for output in ["DP-1", "HDMI-A-1"] {
            assert_eq!(
                landing(output, &urgent),
                Some((output.to_string(), "critical".to_string()))
            );
            assert_eq!(
                landing(output, &toast),
                Some((output.to_string(), "corner".to_string()))
            );
        }
        assert_eq!(
            landing("eDP-1", &toast),
            None,
            "a screen with no stack shows no card"
        );
    }
}
