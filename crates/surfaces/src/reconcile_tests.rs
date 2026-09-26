//! What a reconcile has to keep answering: what an edge reserves, what a workspace rule may never change, and that every screen is planned on its own.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use platform_wayland::OutputDescriptor;

    use config::{Config, Edge};
    use layout::{
        ActiveWorkspace, Area, AreaId, AreaKind, AutoHide, BarShape, Extent, Group, GroupId,
        GroupKind, Instance, InstanceId, Layer, LayerKind, Layers, Layout, LayoutId, OutputMatch,
        OutputRule, Representation, SessionLayers, WorkspaceMatch, WorkspaceRule, Zone,
    };

    use crate::layer_window::Reserved;
    use crate::reconcile::{Desktop, plan};

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
            outputs: vec![OutputRule {
                matches: OutputMatch("*".into()),
                layers: Layers {
                    top: Layer {
                        areas: top,
                        remove: Vec::new(),
                    },
                    ..Layers::default()
                },
                workspaces,
            }],
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
                remove: Vec::new(),
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
            &BTreeMap::new(),
            outputs,
            &|_| workspace.clone(),
        )
    }

    fn planned(layout: &Layout) -> Vec<Desktop> {
        let (desktops, report) = on(layout, &outputs(), None);
        assert!(report.is_clean(), "{}", report.render());
        desktops
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
        assert_eq!(
            only.reserved.top,
            34.0 + only.config.edge_gap(Edge::Top) as f32,
            "a window must clear the bar and the air it floats in, or it tiles underneath it"
        );
        assert_eq!(
            (
                only.reserved.left,
                only.reserved.right,
                only.reserved.bottom
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

        let gap = desktops[0].config.edge_gap(Edge::Top) as f32;
        assert_eq!(desktops[0].reserved.top, 2.0 + gap);
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
            (ruled[0].reserved.top, ruled[0].reserved.bottom),
            (plain[0].reserved.top, plain[0].reserved.bottom),
            "and it takes nothing off the screen, whichever workspace is active"
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
            desktops[0].reserved.top, desktops[1].reserved.top,
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
        for quiet in [
            LayerKind::Background,
            LayerKind::Desktop,
            LayerKind::Overlay,
        ] {
            assert!(
                desktops[0]
                    .resolved
                    .layer(quiet)
                    .is_none_or(|layer| layer.areas.is_empty()),
                "{quiet} was never written to"
            );
        }
    }
}
