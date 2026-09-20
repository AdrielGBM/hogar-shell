//! What the layout model has to keep doing: the precedence, the merge by id, and the two rules that protect the user's windows and their lock screen.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::resolve::{ActiveWorkspace, Resolved, ResolvedAreaKind, resolve};
    use crate::validate::{Catalogue, validate, validate_resolved};
    use crate::*;

    /// A catalogue that knows three modules: `clock` and `battery` are readings, `mixer` can be interacted with.
    struct Modules;

    impl Catalogue for Modules {
        fn knows_module(&self, module: &str) -> bool {
            matches!(module, "clock" | "battery" | "mixer")
        }

        fn has_representation(&self, module: &str, representation: Representation) -> bool {
            self.knows_module(module) && representation != Representation::Card
        }

        fn is_read_only(&self, module: &str, _representation: Representation) -> bool {
            module != "mixer"
        }

        fn command_resolves(&self, line: &str) -> bool {
            line.starts_with("panel toggle")
        }
    }

    fn layout(toml: &str) -> Layout {
        toml::from_str(toml).expect("the layout parses")
    }

    fn alone(layout: &Layout, output: &str) -> Resolved {
        resolve(layout, &BTreeMap::new(), output, None).0
    }

    fn area_ids(resolved: &Resolved, layer: LayerKind) -> Vec<String> {
        resolved
            .layer(layer)
            .map(|layer| layer.areas.iter().map(|a| a.id.to_string()).collect())
            .unwrap_or_default()
    }

    fn instance_ids(resolved: &Resolved) -> Vec<String> {
        resolved.instances().map(|i| i.id.to_string()).collect()
    }

    const ONE_BAR: &str = r#"
        id = "test"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar-top"
        kind = "bar"
        edge = "top"
        thickness = 32
        reserve = true
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        [[outputs.layers.top.areas.groups.children]]
        id = "clock-1"
        module = "clock"
    "#;

    #[test]
    fn a_layout_round_trips_through_toml() {
        let parsed = layout(ONE_BAR);
        let text = toml::to_string_pretty(&parsed).expect("serializes");
        let again: Layout = toml::from_str(&text).expect("re-parses");
        let resolved = alone(&again, "DP-1");
        assert_eq!(area_ids(&resolved, LayerKind::Top), ["bar-top"]);
        assert_eq!(instance_ids(&resolved), ["clock-1"]);
    }

    #[test]
    fn a_monitor_rule_changes_one_field_and_inherits_the_rest() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 48
            "#
        ));

        let refined = alone(&parsed, "DP-1");
        let other = alone(&parsed, "eDP-1");

        let ResolvedAreaKind::Bar {
            edge, thickness, ..
        } = &refined.layer(LayerKind::Top).unwrap().areas[0].kind
        else {
            panic!("the refined area is still a bar");
        };
        assert_eq!(*thickness, 48.0, "the monitor rule set the thickness");
        assert_eq!(*edge, config::Edge::Top, "and inherited the edge");
        assert_eq!(
            instance_ids(&refined),
            ["clock-1"],
            "and kept what was inside it"
        );

        let ResolvedAreaKind::Bar { thickness, .. } =
            &other.layer(LayerKind::Top).unwrap().areas[0].kind
        else {
            panic!("the untouched area is still a bar");
        };
        assert_eq!(*thickness, 32.0, "every other output is untouched");
    }

    #[test]
    fn a_monitor_rule_adds_an_instance_without_restating_the_bar() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "battery-1"
            module = "battery"
            "#
        ));

        assert_eq!(
            instance_ids(&alone(&parsed, "DP-1")),
            ["clock-1", "battery-1"],
            "the added instance follows the ones already there"
        );
        assert_eq!(
            instance_ids(&alone(&parsed, "eDP-1")),
            ["clock-1"],
            "an output the glob misses is untouched"
        );
    }

    #[test]
    fn a_level_removes_by_id() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "eDP-1"
            [outputs.layers.top]
            remove = ["bar-top"]
            "#
        ));

        assert!(
            area_ids(&alone(&parsed, "eDP-1"), LayerKind::Top).is_empty(),
            "the named output lost the bar"
        );
        assert_eq!(
            area_ids(&alone(&parsed, "DP-1"), LayerKind::Top),
            ["bar-top"]
        );
    }

    #[test]
    fn the_more_specific_glob_is_applied_last() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 64
            [[outputs]]
            match = "DP-*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 48
            "#
        ));

        let resolved = alone(&parsed, "DP-1");
        let ResolvedAreaKind::Bar { thickness, .. } =
            &resolved.layer(LayerKind::Top).unwrap().areas[0].kind
        else {
            panic!("still a bar");
        };
        assert_eq!(
            *thickness, 64.0,
            "the exact name wins over the glob, whatever order they are written in"
        );
    }

    #[test]
    fn an_extends_chain_is_applied_root_first() {
        let base = layout(ONE_BAR);
        let child: Layout = toml::from_str(
            r#"
            id = "mine"
            extends = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 40
            "#,
        )
        .expect("parses");

        let known = BTreeMap::from([(base.id.clone(), base)]);
        let (resolved, report) = resolve(&child, &known, "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());

        let ResolvedAreaKind::Bar {
            edge, thickness, ..
        } = &resolved.layer(LayerKind::Top).unwrap().areas[0].kind
        else {
            panic!("still a bar");
        };
        assert_eq!(*thickness, 40.0, "the child wins");
        assert_eq!(*edge, config::Edge::Top, "and inherits the parent's edge");
        assert_eq!(instance_ids(&resolved), ["clock-1"]);
    }

    #[test]
    fn a_layout_that_extends_itself_is_reported_rather_than_hanging() {
        let parsed: Layout = toml::from_str(
            r#"
            id = "loop"
            extends = "loop"
            "#,
        )
        .expect("parses");
        let known = BTreeMap::from([(parsed.id.clone(), parsed.clone())]);
        let (_, report) = resolve(&parsed, &known, "DP-1", None);
        assert!(!report.is_clean(), "the cycle is reported");
    }

    #[test]
    fn resolving_twice_gives_the_same_arrangement() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 48
            [[outputs.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            "#
        ));

        let once = alone(&parsed, "DP-1");
        let twice = alone(&parsed, "DP-1");
        assert_eq!(
            area_ids(&once, LayerKind::Top),
            area_ids(&twice, LayerKind::Top)
        );
        assert_eq!(
            area_ids(&once, LayerKind::Desktop),
            area_ids(&twice, LayerKind::Desktop)
        );
        assert_eq!(instance_ids(&once), instance_ids(&twice));
        assert_eq!(
            once.layer(LayerKind::Top).unwrap().areas[0].kind,
            twice.layer(LayerKind::Top).unwrap().areas[0].kind
        );
    }

    #[test]
    fn an_area_that_never_says_its_kind_is_left_out_and_reported() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "nameless"
            "#,
        );
        let (resolved, report) = resolve(&parsed, &BTreeMap::new(), "DP-1", None);
        assert!(area_ids(&resolved, LayerKind::Top).is_empty());
        assert!(
            report.findings().any(|f| f.key.ends_with("nameless.kind")),
            "the finding names the area and the field: {}",
            report.render()
        );
    }

    #[test]
    fn a_bar_with_no_thickness_is_left_out_and_the_rest_of_the_layer_survives() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.top.areas]]
            id = "half-written"
            kind = "bar"
            edge = "bottom"
            "#
        ));
        let (resolved, report) = resolve(&parsed, &BTreeMap::new(), "DP-1", None);
        assert_eq!(
            area_ids(&resolved, LayerKind::Top),
            ["bar-top"],
            "the finished bar still draws"
        );
        assert!(
            report
                .findings()
                .any(|f| f.key.ends_with("half-written.thickness")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn a_workspace_rule_may_change_contents_but_not_reservation() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.workspaces]]
            match = "web"
            [[outputs.workspaces.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            thickness = 64
            "#
        ));

        let report = validate(&parsed, &Modules);
        assert!(
            report.findings().any(|f| f.key.ends_with("bar-top.kind")),
            "resizing a reserving area from a workspace rule is rejected with its path: {}",
            report.render()
        );
    }

    #[test]
    fn a_workspace_rule_may_not_remove_a_reserving_area() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.workspaces]]
            match = "id:3"
            [outputs.workspaces.layers.top]
            remove = ["bar-top"]
            "#
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            report.findings().any(|f| f.key.contains("remove")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn a_workspace_rule_that_only_changes_contents_is_accepted_and_applied() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.workspaces]]
            match = "web"
            [[outputs.workspaces.layers.top.areas]]
            id = "bar-top"
            [[outputs.workspaces.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            [[outputs.workspaces.layers.top.areas.groups.children]]
            id = "battery-1"
            module = "battery"
            "#
        ));

        assert!(validate(&parsed, &Modules).is_clean());

        let web = ActiveWorkspace {
            name: "web".into(),
            ..ActiveWorkspace::default()
        };
        let (resolved, report) = resolve(&parsed, &BTreeMap::new(), "DP-1", Some(&web));
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(instance_ids(&resolved), ["clock-1", "battery-1"]);
    }

    #[test]
    fn a_workspace_number_rule_without_hyprland_is_reported_inactive() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.workspaces]]
            match = "id:3"
            [[outputs.workspaces.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            "#
        ));

        let nameless = ActiveWorkspace {
            name: "3".into(),
            ..ActiveWorkspace::default()
        };
        let (resolved, report) = resolve(&parsed, &BTreeMap::new(), "DP-1", Some(&nameless));
        assert!(
            area_ids(&resolved, LayerKind::Desktop).is_empty(),
            "the rule did not apply"
        );
        assert!(
            report.findings().any(|f| f.message.contains("Hyprland")),
            "and said why: {}",
            report.render()
        );
    }

    #[test]
    fn a_workspace_number_rule_applies_where_hyprland_answers() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.workspaces]]
            match = "id:3"
            [[outputs.workspaces.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            "#
        ));

        let third = ActiveWorkspace {
            name: "3".into(),
            id: Some(3),
            special: Some(false),
        };
        let (resolved, report) = resolve(&parsed, &BTreeMap::new(), "DP-1", Some(&third));
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(area_ids(&resolved, LayerKind::Desktop), ["grid"]);
    }

    const LOCKED: &str = r#"
        id = "test"
        [[outputs]]
        match = "*"
        [[outputs.layers.lock.areas]]
        id = "readings"
        kind = "grid"
        [[outputs.layers.lock.areas.groups]]
        id = "cell"
        place = "cell"
        col = 0
        row = 0
        [[outputs.layers.lock.areas.groups.children]]
        id = "lock-clock"
        module = "clock"
        representation = "widget_m"
        [[outputs.layers.lock.areas]]
        id = "prompt"
        kind = "prompt"
    "#;

    #[test]
    fn a_reading_on_the_lock_layer_is_accepted() {
        let parsed = layout(LOCKED);
        let report = validate(&parsed, &Modules);
        assert!(report.is_clean(), "{}", report.render());

        let resolved = alone(&parsed, "DP-1");
        assert!(validate_resolved(&resolved, "layouts/test.toml").is_clean());
    }

    #[test]
    fn a_control_on_the_lock_layer_is_refused() {
        let parsed = layout(&LOCKED.replace(r#"module = "clock""#, r#"module = "mixer""#));
        let report = validate(&parsed, &Modules);
        assert!(
            report.findings().any(|f| f.message.contains("readings")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn an_action_on_the_lock_layer_is_refused() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "prompt"
            kind = "prompt"
            [[outputs.layers.lock.areas]]
            id = "readings"
            kind = "grid"
            [[outputs.layers.lock.areas.groups]]
            id = "cell"
            place = "cell"
            col = 0
            row = 0
            [[outputs.layers.lock.areas.groups.children]]
            id = "lock-clock"
            module = "clock"
            representation = "widget_m"
            [outputs.layers.lock.areas.groups.children.actions]
            press = ["panel toggle clock"]
            "#,
        );
        let report = validate(&parsed, &Modules);
        assert!(
            report.findings().any(|f| f.key.ends_with(".actions")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn a_lock_layer_without_a_prompt_is_refused() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "readings"
            kind = "grid"
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        let report = validate_resolved(&resolved, "layouts/test.toml");
        assert!(
            report.findings().any(|f| f.message.contains("no prompt")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn a_prompt_that_could_be_hidden_or_covered_is_refused() {
        let faded = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "prompt"
            kind = "prompt"
            visible = "$battery.percent > 50"
            [outputs.layers.lock.areas.style]
            opacity = 0.2
            [[outputs.layers.lock.areas]]
            id = "over-it"
            kind = "free"
            rect = { x = 0.0, y = 0.0, w = 1.0, h = 1.0 }
            "#,
        );
        let report = validate_resolved(&alone(&faded, "DP-1"), "layouts/test.toml");
        let keys: Vec<&str> = report.findings().map(|f| f.key.as_str()).collect();
        assert!(
            keys.iter().any(|k| k.ends_with("prompt.visible")),
            "a visibility expression on the prompt is a lockout: {keys:?}"
        );
        assert!(
            keys.iter().any(|k| k.ends_with("style.opacity")),
            "and so is fading it away: {keys:?}"
        );
        assert!(
            keys.iter().any(|k| k.ends_with("over-it")),
            "and so is covering it: {keys:?}"
        );
    }

    #[test]
    fn an_unknown_module_and_an_unknown_command_are_both_reported() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar"
            kind = "bar"
            edge = "top"
            thickness = 32
            [[outputs.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "ghost-1"
            module = "ghost"
            [outputs.layers.top.areas.groups.children.actions]
            press = ["summon the dead"]
            "#,
        );
        let report = validate(&parsed, &Modules);
        let messages: Vec<&str> = report.findings().map(|f| f.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("`ghost`")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("summon the dead")),
            "{messages:?}"
        );
    }

    #[test]
    fn two_instances_may_not_share_an_id() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            module = "battery"
            "#
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            report
                .findings()
                .any(|f| f.message.contains("already used")),
            "{}",
            report.render()
        );
    }

    fn instance(id: &str, module: &str) -> Instance {
        Instance {
            id: InstanceId::new(id),
            module: Some(module.into()),
            ..Instance::default()
        }
    }

    fn top_zone() -> Spot {
        Spot {
            site: Site::everywhere(LayerKind::Top),
            area: AreaId::new("bar-top"),
            group: GroupId::new("start"),
        }
    }

    fn children(layout: &Layout) -> Vec<String> {
        layout.outputs[0].layers.top.areas[0].groups[0]
            .children
            .iter()
            .map(|i| i.id.to_string())
            .collect()
    }

    #[test]
    fn every_edit_can_be_taken_back_exactly() {
        let start = layout(ONE_BAR);
        let edits = [
            LayoutOp::InsertInstance {
                spot: top_zone(),
                index: 0,
                instance: Box::new(instance("battery-1", "battery")),
            },
            LayoutOp::DeleteInstance {
                spot: top_zone(),
                id: InstanceId::new("clock-1"),
            },
            LayoutOp::SetAreaFlags {
                site: Site::everywhere(LayerKind::Top),
                id: AreaId::new("bar-top"),
                reserve: Some(false),
                above_fullscreen: Some(true),
                visible: None,
            },
            LayoutOp::SetAreaKind {
                site: Site::everywhere(LayerKind::Top),
                id: AreaId::new("bar-top"),
                kind: Box::new(Some(AreaKind::Bar {
                    edge: Some(config::Edge::Bottom),
                    thickness: Some(48.0),
                    length: None,
                    offset: None,
                    shape: BarShape::default(),
                    autohide: None,
                })),
            },
            LayoutOp::InsertArea {
                site: Site::everywhere(LayerKind::Desktop),
                index: 0,
                area: Box::new(Area {
                    id: AreaId::new("grid"),
                    kind: Some(AreaKind::Grid {
                        rect: None,
                        cell: None,
                        gap: None,
                        anchor: None,
                    }),
                    ..Area::default()
                }),
            },
        ];

        for edit in edits {
            let mut edited = start.clone();
            let back = ops::apply(&mut edited, &edit).expect("the edit applies");
            let before = toml::to_string(&start).unwrap();
            let after_edit = toml::to_string(&edited).unwrap();
            assert_ne!(before, after_edit, "the edit changed something");

            ops::apply(&mut edited, &back).expect("the undo applies");
            assert_eq!(
                toml::to_string(&edited).unwrap(),
                before,
                "undoing {edit:?} did not restore the layout"
            );
        }
    }

    #[test]
    fn moving_an_instance_keeps_its_id_and_comes_back_to_its_slot() {
        let mut edited = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.top.areas.groups]]
            id = "end"
            place = "zone"
            zone = "end"
            "#
        ));
        edited.outputs[0].layers.top.areas[0].groups[0]
            .children
            .push(instance("battery-1", "battery"));

        let before = toml::to_string(&edited).unwrap();
        let end = Spot {
            group: GroupId::new("end"),
            ..top_zone()
        };
        let back = ops::apply(
            &mut edited,
            &LayoutOp::MoveInstance {
                from: top_zone(),
                to: end,
                id: InstanceId::new("clock-1"),
                index: 0,
            },
        )
        .expect("the move applies");

        assert_eq!(children(&edited), ["battery-1"], "it left the start zone");
        assert_eq!(
            edited.outputs[0].layers.top.areas[0].groups[1].children[0].id,
            InstanceId::new("clock-1"),
            "and arrived with the id it had"
        );

        ops::apply(&mut edited, &back).expect("the undo applies");
        assert_eq!(toml::to_string(&edited).unwrap(), before);
    }

    #[test]
    fn a_batch_that_fails_half_way_changes_nothing() {
        let mut edited = layout(ONE_BAR);
        let before = toml::to_string(&edited).unwrap();

        let outcome = ops::apply_all(
            &mut edited,
            &[
                LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 0,
                    instance: Box::new(instance("battery-1", "battery")),
                },
                LayoutOp::DeleteArea {
                    site: Site::everywhere(LayerKind::Top),
                    id: AreaId::new("not-there"),
                },
            ],
        );

        assert_eq!(
            outcome.unwrap_err(),
            ops::OpError::NoArea(AreaId::new("not-there"))
        );
        assert_eq!(
            toml::to_string(&edited).unwrap(),
            before,
            "the operation that had already been applied was taken back"
        );
    }

    /// `keys_of_kind` is a hand-written list of what each area kind has, and a hand-written list is a second copy of the truth that goes stale the first time a field is added — which it did, the day `Grid` gained its anchor, turning every anchored grid into a reported error. This asks the types themselves, so the list can never drift again without a failing test naming the field.
    #[test]
    fn every_field_an_area_kind_has_is_a_key_validation_knows() {
        let kinds = [
            AreaKind::Bar {
                edge: Some(config::Edge::Top),
                thickness: Some(32.0),
                length: Some(Extent::Fill),
                offset: Some(0.0),
                shape: BarShape {
                    mode: Some(config::Shape::Chips),
                    gap: Some(0.0),
                    spacing: Some(8.0),
                    radius: Some(8.0),
                },
                autohide: Some(AutoHide::default()),
            },
            AreaKind::Grid {
                rect: Some(Rect::default()),
                cell: Some(80.0),
                gap: Some(16.0),
                anchor: Some(Anchor::Center),
            },
            AreaKind::Stack {
                anchor: Some(Anchor::BottomRight),
                width: Some(360.0),
                output_policy: Some(StackOutputPolicy::Focused),
                routes: vec![Route::default()],
            },
            AreaKind::WallpaperRegion {
                rect: Some(Rect::default()),
                source: Some("a.png".into()),
                fit: Some(Fit::Cover),
                transition: Some(Transition::Fade),
            },
            AreaKind::Texture {
                rect: Some(Rect::default()),
                image: Some("a.png".into()),
                gradient: None,
                tile: Some(Tile::Repeat),
                blend: Some(Blend::Normal),
                opacity: Some(1.0),
            },
            AreaKind::Dock {
                edge: Some(config::Edge::Bottom),
                thickness: Some(140.0),
            },
            AreaKind::Free {
                rect: Some(Rect::default()),
            },
            AreaKind::Prompt {
                rect: Some(Rect::default()),
                style: PromptStyle {
                    fill: Some("base".into()),
                    radius: Some(8.0),
                    opacity: Some(1.0),
                },
            },
        ];

        for kind in kinds {
            let name = kind.name();
            let area = Area {
                id: AreaId::new("a"),
                kind: Some(kind),
                ..Area::default()
            };
            let report = validate::check_unknown_keys(&written_layout(area), &LayoutId::new("t"));
            assert!(
                report.is_clean(),
                "a `{name}` area writes keys validation does not know: {}",
                report.render()
            );
        }
    }

    /// Wraps one area in the smallest whole layout, written the way the store writes it. Building the text by hand instead would only prove that a hand-built string parses.
    fn written_layout(area: Area) -> String {
        let layout = Layout {
            id: LayoutId::new("t"),
            outputs: vec![OutputRule {
                layers: Layers {
                    top: Layer {
                        areas: vec![area],
                        remove: Vec::new(),
                    },
                    ..Layers::default()
                },
                ..OutputRule::default()
            }],
            ..Layout::default()
        };
        toml::to_string_pretty(&layout).expect("the layout serializes")
    }

    /// The same question for a group: `place` decides which extra keys it has, and that list drifts the same way.
    #[test]
    fn every_field_a_group_placement_has_is_a_key_validation_knows() {
        let placements = [
            GroupKind::Zone { zone: Zone::Start },
            GroupKind::Cell {
                col: 1,
                row: 2,
                col_span: 2,
                row_span: 3,
            },
            GroupKind::SmartStack,
        ];

        for kind in placements {
            let area = Area {
                id: AreaId::new("a"),
                kind: Some(AreaKind::Grid {
                    rect: None,
                    cell: None,
                    gap: None,
                    anchor: None,
                }),
                groups: vec![Group {
                    id: GroupId::new("g"),
                    kind: Some(kind),
                    ..Group::default()
                }],
                ..Area::default()
            };
            let report = validate::check_unknown_keys(&written_layout(area), &LayoutId::new("t"));
            assert!(
                report.is_clean(),
                "a group writes keys validation does not know: {}",
                report.render()
            );
        }
    }

    #[test]
    fn a_mistyped_key_is_reported_with_the_area_it_sits_in() {
        let text = r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            edge = "top"
            thikness = 32
        "#;
        let report = validate::check_unknown_keys(text, &LayoutId::new("test"));
        let messages: Vec<&str> = report.findings().map(|f| f.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("thikness")),
            "{messages:?}"
        );
        assert!(
            report.findings().any(|f| f.span.is_some()),
            "and points at where it is written"
        );
    }

    #[test]
    fn the_keys_an_area_really_has_are_accepted() {
        let text = r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            edge = "top"
            thickness = 32
            offset = 0.0
            reserve = true
            above_fullscreen = false
            autohide = { peek = 2 }
            [outputs.layers.top.areas.shape]
            mode = "chips"
            [outputs.layers.top.areas.style]
            opacity = 0.8
            [[outputs.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            [[outputs.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            cell = 80.0
            gap = 16.0
            [[outputs.layers.desktop.areas.groups]]
            id = "cell-0"
            place = "cell"
            col = 0
            row = 0
            col_span = 2
        "#;
        let report = validate::check_unknown_keys(text, &LayoutId::new("test"));
        assert!(report.is_clean(), "{}", report.render());
    }

    fn store_with(dir: &std::path::Path, mine: &Layout) -> LayoutStore {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("mine.toml"), toml::to_string_pretty(mine).unwrap()).unwrap();
        let (store, report) = LayoutStore::load(dir);
        assert!(report.is_clean(), "{}", report.render());
        store
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hogar-layout-tests-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A layout file is the only way to edit bars and widgets between this sprint and the editor, so what the store writes has to stay something a person can read. Empty scaffolding — a layer with no areas, a style with nothing set, an instance's three empty tables — buries the two lines that matter.
    #[test]
    fn a_written_layout_carries_no_empty_scaffolding() {
        let grid = Area {
            id: AreaId::new("desktop"),
            kind: Some(AreaKind::Grid {
                rect: None,
                cell: None,
                gap: None,
                anchor: Some(Anchor::Center),
            }),
            groups: vec![Group {
                id: GroupId::new("cell"),
                kind: Some(GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 1,
                    row_span: 1,
                }),
                children: vec![instance("clock", "clock")],
                ..Group::default()
            }],
            ..Area::default()
        };
        let mut layout = built_in::layout();
        layout.outputs[0].layers.desktop.areas.push(grid);

        let written = toml::to_string_pretty(&layout).expect("serializes");
        for noise in [
            "areas = []",
            "remove = []",
            "workspaces = []",
            "children = []",
            "groups = []",
            "[outputs.layers.background]",
            "[outputs.layers.overlay]",
            "col_span = 1",
            "row_span = 1",
        ] {
            assert!(
                !written.contains(noise),
                "`{noise}` should not be written:\n{written}"
            );
        }
        let again: Layout = toml::from_str(&written).expect("re-parses");
        assert_eq!(
            instance_ids(&alone(&again, "DP-1")),
            instance_ids(&alone(&layout, "DP-1")),
            "and leaving it out changed nothing"
        );
    }

    #[test]
    fn the_built_in_layout_resolves_and_validates_on_its_own() {
        let built_in = built_in::layout();
        let (resolved, report) = resolve(&built_in, &BTreeMap::new(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(area_ids(&resolved, LayerKind::Top), ["bar-top"]);
        assert_eq!(
            instance_ids(&resolved),
            ["workspaces", "clock", "notes"],
            "the arrangement a fresh install has always shown"
        );
        assert!(
            validate_resolved(&resolved, "built-in").is_clean(),
            "including a lock layer with its prompt"
        );
    }

    #[test]
    fn the_built_in_layout_refuses_to_be_edited_and_can_be_forked_instead() {
        let dir = scratch("readonly");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut store, _) = LayoutStore::load(&dir);

        let edit = || {
            Transaction::new(
                "Remove the clock",
                LayoutId::new(BUILT_IN),
                vec![LayoutOp::DeleteInstance {
                    spot: top_zone_of("center"),
                    id: InstanceId::new("clock"),
                }],
            )
        };
        assert!(
            matches!(store.commit(edit()), Err(StoreError::ReadOnly(_))),
            "the built-in layout is read-only"
        );

        store
            .fork(&LayoutId::new(BUILT_IN), LayoutId::new("mine"))
            .expect("it forks");
        let mut forked = edit();
        forked.layout = LayoutId::new("mine");
        store.commit(forked).expect("the copy takes the edit");

        assert_eq!(
            store.get(&LayoutId::new("mine")).unwrap().outputs[0]
                .layers
                .top
                .areas[0]
                .groups[1]
                .children
                .len(),
            0
        );
        assert_eq!(
            store.get(&LayoutId::new(BUILT_IN)).unwrap().outputs[0]
                .layers
                .top
                .areas[0]
                .groups[1]
                .children
                .len(),
            1,
            "and the built-in one is untouched"
        );
    }

    fn top_zone_of(group: &str) -> Spot {
        Spot {
            site: Site::everywhere(LayerKind::Top),
            area: AreaId::new("bar-top"),
            group: GroupId::new(group),
        }
    }

    #[test]
    fn undo_and_redo_walk_the_same_edits_back_and_forward() {
        let dir = scratch("history");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        fn at(store: &LayoutStore, id: &LayoutId) -> String {
            toml::to_string(store.get(id).expect("the store holds it")).unwrap()
        }

        let start = at(&store, &mine);
        store
            .commit(Transaction::new(
                "Add a battery",
                mine.clone(),
                vec![LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("battery-1", "battery")),
                }],
            ))
            .expect("the edit commits");
        let added = at(&store, &mine);
        assert_ne!(start, added);

        store
            .commit(Transaction::new(
                "Remove the clock",
                mine.clone(),
                vec![LayoutOp::DeleteInstance {
                    spot: top_zone(),
                    id: InstanceId::new("clock-1"),
                }],
            ))
            .expect("the edit commits");
        let removed = at(&store, &mine);

        assert_eq!(store.undo_label(), Some("Remove the clock"));
        store.undo().expect("undo");
        assert_eq!(at(&store, &mine), added, "the second edit is back out");
        store.undo().expect("undo");
        assert_eq!(at(&store, &mine), start, "and so is the first");
        assert!(matches!(store.undo(), Err(StoreError::NothingToUndo)));

        store.redo().expect("redo");
        assert_eq!(at(&store, &mine), added);
        store.redo().expect("redo");
        assert_eq!(
            at(&store, &mine),
            removed,
            "and forward again to where it was"
        );
    }

    #[test]
    fn a_new_edit_drops_what_was_undone() {
        let dir = scratch("branch");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");

        store
            .commit(Transaction::new(
                "Add a battery",
                mine.clone(),
                vec![LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("battery-1", "battery")),
                }],
            ))
            .unwrap();
        store.undo().unwrap();
        store
            .commit(Transaction::new(
                "Add a mixer",
                mine.clone(),
                vec![LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("mixer-1", "mixer")),
                }],
            ))
            .unwrap();

        assert!(
            matches!(store.redo(), Err(StoreError::NothingToRedo)),
            "redoing across a different edit would replay operations against a layout they no longer describe"
        );
    }

    #[test]
    fn a_committed_edit_is_written_once_it_settles() {
        let dir = scratch("flush");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");

        assert!(!store.has_unsaved());
        store
            .commit(Transaction::new(
                "Add a battery",
                mine.clone(),
                vec![LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("battery-1", "battery")),
                }],
            ))
            .unwrap();

        assert!(store.has_unsaved(), "it is waiting to be written");
        assert!(
            !store.is_settled(std::time::Instant::now()),
            "but not while the change is still fresh, so a drag does not write per frame"
        );
        assert!(store.is_settled(std::time::Instant::now() + SETTLE));

        assert!(store.flush().is_clean());
        util::writer::flush();
        let written = std::fs::read_to_string(store.path_of(&mine)).unwrap();
        assert!(written.contains("battery-1"), "{written}");
        assert!(!store.has_unsaved());
    }

    #[test]
    fn an_unreadable_layout_is_reported_and_the_others_still_load() {
        let dir = scratch("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.toml"), ONE_BAR).unwrap();
        std::fs::write(dir.join("broken.toml"), "this is not toml = = =").unwrap();

        let (store, report) = LayoutStore::load(&dir);
        assert!(!report.is_clean(), "the broken one is reported");
        let names: Vec<String> = store.names().map(|id| id.to_string()).collect();
        assert!(names.contains(&"mine".to_string()), "{names:?}");
        assert!(names.contains(&BUILT_IN.to_string()), "{names:?}");
    }

    #[test]
    fn a_last_good_copy_is_what_a_broken_edit_falls_back_to() {
        let dir = scratch("lastgood");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");

        store.keep_last_good(&mine).expect("it is kept");
        util::writer::flush();

        store
            .commit(Transaction::new(
                "Remove the bar",
                mine.clone(),
                vec![LayoutOp::DeleteArea {
                    site: Site::everywhere(LayerKind::Top),
                    id: AreaId::new("bar-top"),
                }],
            ))
            .unwrap();

        let fallback = store.last_good(&mine).expect("there is one");
        assert_eq!(
            area_ids(&alone(&fallback, "DP-1"), LayerKind::Top),
            ["bar-top"],
            "the copy still has what the edit took away"
        );
    }

    /// The clock the import writes has to land where it lands today, which is inside what the bars left — not under one.
    #[test]
    fn an_area_says_which_box_its_fractions_are_of() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            within = "usable"
            [[outputs.layers.background.areas]]
            id = "wall"
            kind = "wallpaper_region"
            source = "a.png"
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        let desktop = &resolved.layer(LayerKind::Desktop).unwrap().areas[0];
        let background = &resolved.layer(LayerKind::Background).unwrap().areas[0];
        assert_eq!(desktop.within, Within::Usable);
        assert_eq!(
            background.within,
            Within::Output,
            "a wallpaper belongs under the bars, so the output is the right default"
        );
        assert!(
            validate::check_unknown_keys(
                &toml::to_string_pretty(&parsed).unwrap(),
                &LayoutId::new("test")
            )
            .is_clean()
        );
    }

    /// The renderer holds a gradient's stops in a fixed array of eight. A ninth is not a subtlety that gets lost in the blend — it is a colour the user wrote and will never see — so it is reported rather than truncated.
    #[test]
    fn a_gradient_with_more_stops_than_can_be_drawn_is_reported() {
        let stops: String = (0..10)
            .map(|n| {
                format!(
                    "[[outputs.layers.background.areas.gradient.stops]]\nat = {}\ncolor = \"base\"\n",
                    n as f32 / 9.0
                )
            })
            .collect();
        let parsed = layout(&format!(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.background.areas]]
            id = "texture"
            kind = "texture"
            [outputs.layers.background.areas.gradient]
            angle = 90.0
            {stops}"#
        ));

        let report = validate(&parsed, &Modules);
        assert!(
            report
                .findings()
                .any(|f| f.message.contains("at most 8 stops")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn an_autohidden_bar_reserves_only_its_peek_strip() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            edge = "top"
            thickness = 32
            reserve = true
            autohide = { peek = 2 }
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        assert_eq!(resolved.reserved(config::Edge::Top), 2.0);
        assert_eq!(resolved.reserved(config::Edge::Bottom), 0.0);
    }
}
