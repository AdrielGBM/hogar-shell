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

        fn option_problems(&self, module: &str, options: &toml::Table) -> Vec<(String, String)> {
            match module {
                "clock" => config::fields::check(
                    &config::fields::section("clock").expect("[clock]"),
                    options,
                ),
                _ => Vec::new(),
            }
        }
    }

    fn theme() -> config::theme::NordTheme {
        config::theme::NordTheme::default()
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

    /// A bar is rounded by its `shape.radius`; a `style.radius` beside it would be a second answer for the same corners, so it is reported where it is written rather than silently losing to the first.
    #[test]
    fn a_style_radius_on_a_bar_is_reported_rather_than_drawn() {
        let rounded = layout(&ONE_BAR.replace(
            "reserve = true\n",
            "reserve = true\nstyle = { radius = 8, fill = \"surface\" }\n",
        ));
        let report = validate(&rounded, &Modules);
        let warned: Vec<&str> = report.warnings.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            warned,
            ["outputs.*.layers.top.areas.bar-top.style.radius"],
            "{}",
            report.render()
        );
        assert!(report.errors.is_empty(), "{}", report.render());
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
        assert!(validate_resolved(&resolved, "layouts/test.toml", &theme()).is_clean());
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
        let report = validate_resolved(&resolved, "layouts/test.toml", &theme());
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
        let report = validate_resolved(&alone(&faded, "DP-1"), "layouts/test.toml", &theme());
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

    /// An instance option is declared once, on its module's options type, so a key that is not there — or a value its type cannot hold — is an error placed where it was written rather than an option that silently does nothing.
    #[test]
    fn an_option_the_module_does_not_declare_is_reported_where_it_was_written() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.options]
            show_date = "yes"
            date_format = "%A"
            colour = "red"
            "#
        ));
        let report = validate(&parsed, &Modules);
        let found: Vec<(&str, &str, &str)> = report
            .findings()
            .map(|f| {
                (
                    f.file.to_str().unwrap_or(""),
                    f.key.as_str(),
                    f.message.as_str(),
                )
            })
            .collect();
        let at = "outputs.*.layers.top.areas.bar-top.groups.start.children.clock-1.options";
        assert_eq!(
            found,
            [
                (
                    "layouts/test.toml",
                    format!("{at}.colour").as_str(),
                    "`clock`: `colour` is not one of its options"
                ),
                (
                    "layouts/test.toml",
                    format!("{at}.show_date").as_str(),
                    "`clock`: `show_date` takes true or false"
                ),
            ],
            "{}",
            report.render()
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
            LayoutOp::SetGroupStacked {
                site: Site::everywhere(LayerKind::Top),
                area: AreaId::new("bar-top"),
                id: GroupId::new("start"),
                stacked: Some(true),
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
                    radius: Some(Corners::each(8.0, 0.0, 8.0, 0.0)),
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
                offset: Some(Offset { x: -12.0, y: 24.0 }),
                width: Some(360.0),
                output_policy: Some(StackOutputPolicy::Focused),
                routes: vec![Route::default()],
                launcher: Some(true),
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
            },
        ];

        for kind in kinds {
            let name = kind.name();
            let area = Area {
                id: AreaId::new("a"),
                kind: Some(kind),
                reserve: Some(false),
                above_fullscreen: Some(false),
                within: Some(Within::Usable),
                style: AreaStyle {
                    fill: Some("surface".into()),
                    radius: Some(Corners::each(12.0, 12.0, 0.0, 0.0)),
                    opacity: Some(0.9),
                    padding: Some(4.0),
                    backdrop: Some(Backdrop::Blur),
                },
                visible: Some(Expr("true".into())),
                remove: vec![GroupId::new("gone")],
                actions: BTreeMap::from([(
                    Trigger::ScrollUp,
                    Action(vec!["panel toggle clock".into()]),
                )]),
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
                    stacked: Some(true),
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
    fn a_stacked_group_keeps_the_place_it_was_given() {
        let parsed = layout(
            r#"
            id = "t"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "weather"
            place = "cell"
            col = 2
            row = 1
            stacked = true
            [[outputs.layers.desktop.areas]]
            id = "dock"
            kind = "dock"
            edge = "bottom"
            thickness = 40
            [[outputs.layers.desktop.areas.groups]]
            id = "end"
            place = "zone"
            zone = "end"
            stacked = true
            [[outputs.layers.desktop.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        let groups: Vec<(String, GroupKind, bool)> = resolved
            .layer(LayerKind::Desktop)
            .expect("the desktop layer")
            .areas
            .iter()
            .flat_map(|area| &area.groups)
            .map(|group| (group.id.to_string(), group.kind, group.stacked))
            .collect();
        assert_eq!(
            groups,
            [
                (
                    "weather".to_string(),
                    GroupKind::Cell {
                        col: 2,
                        row: 1,
                        col_span: 1,
                        row_span: 1
                    },
                    true
                ),
                ("end".to_string(), GroupKind::Zone { zone: Zone::End }, true),
                (
                    "start".to_string(),
                    GroupKind::Zone { zone: Zone::Start },
                    false
                ),
            ]
        );
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
            [
                "clock-2",
                "workspaces",
                "clock",
                "notes",
                "lock-clock",
                "lock-user",
                "lock-media",
                "lock-notifications"
            ],
            "the desktop and the lock screen a fresh install has always shown"
        );
        assert_eq!(
            area_ids(&resolved, LayerKind::Lock),
            ["lock-readings", "prompt"],
            "with the prompt last, so nothing the layer holds is stacked over it"
        );
        assert!(
            validate_resolved(&resolved, "built-in", &theme()).is_clean(),
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

    /// Two bars side by side along one edge share its band, so the edge reserves the deeper of them rather than both stacked.
    #[test]
    fn bars_beside_each_other_on_one_edge_reserve_the_deepest_not_the_sum() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "left-half"
            kind = "bar"
            edge = "top"
            thickness = 32
            length = { px = 900 }
            reserve = true
            [[outputs.layers.top.areas]]
            id = "right-half"
            kind = "bar"
            edge = "top"
            thickness = 40
            length = { px = 900 }
            offset = 1000
            reserve = true
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        assert_eq!(resolved.reserved(config::Edge::Top), 40.0);
    }

    /// Whatever sequence of edits a gesture, a popover or a script makes, undoing them all comes back to exactly the layout it started from, and redoing them all goes forward to exactly where it stopped.
    ///
    /// Over random sequences rather than a chosen one, because the failure this guards against is a *pair* of operations: one that displaces something the other one's inverse then puts back in the wrong place. A fixed list proves the ones whoever wrote it thought of. The generator is seeded and printed, so a failure is a sequence anybody can run again.
    #[test]
    fn any_sequence_of_edits_undoes_back_to_where_it_started() {
        let dir = scratch("property");
        let mine = LayoutId::new("mine");
        let written = |store: &LayoutStore| {
            toml::to_string(store.get(&mine).expect("the store holds it")).unwrap()
        };

        let mut edits = 0;
        for seed in 1..=64u64 {
            let mut store = store_with(&dir, &layout(TWO_ZONES));
            let start = written(&store);
            let mut random = Random::from(seed);
            let mut states = vec![start.clone()];
            let mut made = 0;

            for step in 0..8 {
                let op = random.op(store.get(&mine).expect("it is there"), step);
                let Some(op) = op else { continue };
                if store
                    .commit(Transaction::new(
                        format!("Edit {step}"),
                        mine.clone(),
                        vec![op],
                    ))
                    .is_err()
                {
                    // An operation the layout cannot carry out is abandoned whole, so the state it left is the state it found.
                    continue;
                }
                states.push(written(&store));
                made += 1;
            }

            for back in (0..made).rev() {
                store
                    .undo()
                    .unwrap_or_else(|why| panic!("seed {seed}: {why}"));
                assert_eq!(
                    written(&store),
                    states[back],
                    "seed {seed}: undoing edit {back} did not restore the layout it was made to"
                );
            }
            for forward in 0..made {
                store
                    .redo()
                    .unwrap_or_else(|why| panic!("seed {seed}: {why}"));
                assert_eq!(
                    written(&store),
                    states[forward + 1],
                    "seed {seed}: redoing edit {forward} did not put it back"
                );
            }
            edits += made;
        }
        // A generator that produced nothing would pass every assertion above without testing anything, which is exactly how the unknown-key test first passed with its own fault in place (F-10.4).
        assert!(edits > 256, "only {edits} edits were generated");
    }

    /// A layout with two runs and three modules, so a move has somewhere to move to and an index to be wrong about.
    const TWO_ZONES: &str = r#"
        id = "test"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar-top"
        kind = "bar"
        edge = "top"
        thickness = 32
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        [[outputs.layers.top.areas.groups.children]]
        id = "clock-1"
        module = "clock"
        [[outputs.layers.top.areas.groups.children]]
        id = "battery-1"
        module = "battery"
        [[outputs.layers.top.areas.groups]]
        id = "end"
        place = "zone"
        zone = "end"
        [[outputs.layers.top.areas.groups.children]]
        id = "mixer-1"
        module = "mixer"
    "#;

    /// A generator the sequence of edits above is drawn from. Not a dependency: what the property needs is a reproducible spread of operations, and that is a multiply-and-add away.
    struct Random(u64);

    impl Random {
        fn from(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
        }

        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn upto(&mut self, bound: usize) -> usize {
            match bound {
                0 => 0,
                bound => (self.next() % bound as u64) as usize,
            }
        }

        /// One edit against the layout as it now stands, addressing something that is actually in it — a sequence of operations against ids that were never there would only ever prove that `apply` refuses them.
        fn op(&mut self, layout: &Layout, step: usize) -> Option<LayoutOp> {
            let groups: Vec<GroupId> = layout.outputs[0].layers.top.areas[0]
                .groups
                .iter()
                .map(|group| group.id.clone())
                .collect();
            let placed: Vec<(GroupId, InstanceId)> = layout.outputs[0].layers.top.areas[0]
                .groups
                .iter()
                .flat_map(|group| {
                    group
                        .children
                        .iter()
                        .map(|child| (group.id.clone(), child.id.clone()))
                })
                .collect();
            let spot = |group: &GroupId| Spot {
                site: Site::everywhere(LayerKind::Top),
                area: AreaId::new("bar-top"),
                group: group.clone(),
            };
            let group = groups.get(self.upto(groups.len()))?.clone();

            Some(match self.next() % 5 {
                0 => LayoutOp::InsertInstance {
                    spot: spot(&group),
                    index: self.upto(count(layout, &group) + 1),
                    instance: Box::new(instance(&format!("added-{step}"), "clock")),
                },
                1 => {
                    let (group, id) = placed.get(self.upto(placed.len()))?.clone();
                    LayoutOp::DeleteInstance {
                        spot: spot(&group),
                        id,
                    }
                }
                2 => {
                    let (from, id) = placed.get(self.upto(placed.len()))?.clone();
                    let to = groups.get(self.upto(groups.len()))?.clone();
                    let room = match from == to {
                        true => count(layout, &to).saturating_sub(1),
                        false => count(layout, &to),
                    };
                    LayoutOp::MoveInstance {
                        from: spot(&from),
                        to: spot(&to),
                        id,
                        index: self.upto(room + 1),
                    }
                }
                3 => {
                    let (group, id) = placed.get(self.upto(placed.len()))?.clone();
                    LayoutOp::SetInstance {
                        spot: spot(&group),
                        id,
                        instance: Box::new(instance("ignored", "battery")),
                    }
                }
                _ => LayoutOp::SetAreaFlags {
                    site: Site::everywhere(LayerKind::Top),
                    id: AreaId::new("bar-top"),
                    reserve: Some(self.next().is_multiple_of(2)),
                    above_fullscreen: None,
                    visible: None,
                },
            })
        }
    }

    fn count(layout: &Layout, group: &GroupId) -> usize {
        layout.outputs[0].layers.top.areas[0]
            .groups
            .iter()
            .find(|it| &it.id == group)
            .map(|it| it.children.len())
            .unwrap_or(0)
    }

    /// The shell's own write coming back through the watcher is not an edit: the layout keeps what it is holding, including the history that would take the write back.
    ///
    /// Without this, `layout undo` would work until the file settled and then stop working — which is the worst shape a command can have, since the moment it stops is a quarter of a second the user does not see.
    #[test]
    fn a_reload_after_the_store_s_own_write_keeps_the_edit_and_its_history() {
        let dir = scratch("echo");
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
            .expect("the edit commits");
        assert!(store.flush().is_clean());
        util::writer::flush();

        assert!(store.reload().is_clean());
        assert_eq!(
            children(store.get(&mine).expect("still there")),
            ["clock-1", "battery-1"],
            "the edit the store itself wrote is still on screen"
        );
        assert_eq!(
            store.undo_label(),
            Some("Add a battery"),
            "and can still be taken back"
        );
        store.undo().expect("undo");
        assert_eq!(
            children(store.get(&mine).expect("still there")),
            ["clock-1"]
        );
    }

    /// An edit somebody else made is the file's to win, and the history of that layout goes with the bytes it described — but a layout nobody touched keeps its own.
    #[test]
    fn a_reload_after_someone_else_s_edit_takes_the_file_and_drops_that_layout_s_history() {
        let dir = scratch("external");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        std::fs::write(
            dir.join("other.toml"),
            toml::to_string_pretty(&layout(ONE_BAR)).unwrap(),
        )
        .unwrap();
        assert!(store.reload().is_clean());

        let mine = LayoutId::new("mine");
        let other = LayoutId::new("other");
        for id in [&mine, &other] {
            store
                .commit(Transaction::new(
                    format!("Add a battery to {id}"),
                    id.clone(),
                    vec![LayoutOp::InsertInstance {
                        spot: top_zone(),
                        index: 1,
                        instance: Box::new(instance("battery-1", "battery")),
                    }],
                ))
                .expect("the edit commits");
        }

        // Somebody's editor rewrites one of the two under the shell.
        std::fs::write(
            store.path_of(&mine),
            toml::to_string_pretty(&layout(TWO_ZONES)).unwrap(),
        )
        .unwrap();
        assert!(store.reload().is_clean());

        assert_eq!(
            children(store.get(&mine).expect("still there")),
            ["clock-1", "battery-1"],
            "the file is what `mine` holds now, not the uncommitted edit"
        );
        assert_eq!(
            store.undo_label(),
            Some("Add a battery to other"),
            "and the history left is the untouched layout's"
        );
        store.undo().expect("undo");
        assert_eq!(
            children(store.get(&other).expect("still there")),
            ["clock-1"]
        );
        assert!(matches!(store.undo(), Err(StoreError::NothingToUndo)));
    }

    /// A layout file that stops parsing leaves what was drawn on screen and says what is wrong, rather than taking the desktop away over one bad save.
    #[test]
    fn a_reload_of_a_file_that_stopped_parsing_keeps_what_is_drawn() {
        let dir = scratch("unparseable");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        store.use_layout(&mine).expect("it is there");

        std::fs::write(store.path_of(&mine), "this is not toml = = =").unwrap();
        let report = store.reload();
        assert!(!report.is_clean(), "it says so");
        assert_eq!(store.active_id(), &mine, "and keeps drawing it");
        assert_eq!(
            children(store.get(&mine).expect("still there")),
            ["clock-1"]
        );
    }

    /// A file taken away is taken away, unless the store is holding an edit nothing has written yet — which is the moment between a fork and its flush.
    #[test]
    fn a_layout_whose_file_is_gone_goes_with_it_unless_it_is_waiting_to_be_written() {
        let dir = scratch("vanished");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        let fork = LayoutId::new("fork");
        store.fork(&mine, fork.clone()).expect("it copies");

        std::fs::remove_file(store.path_of(&mine)).unwrap();
        assert!(store.reload().is_clean());
        assert!(
            store.get(&mine).is_none(),
            "the file is gone, so it is gone"
        );
        assert!(
            store.get(&fork).is_some(),
            "but a fork nothing has written yet is not something a reload can lose"
        );
        assert_eq!(
            store.active_id().as_str(),
            BUILT_IN,
            "and the store falls back to the layout that cannot go missing"
        );
    }

    /// `--safe-layout` exists to rescue a session, so it must not be able to write over the files it was started to rescue. It cannot see the user's layouts, so a fork would pick a name from an empty set.
    #[test]
    fn the_recovery_store_refuses_every_edit_rather_than_forking_the_built_in_layout() {
        let mut store = LayoutStore::safe(scratch("safe"));
        assert!(store.is_safe());
        let refused = store
            .fork(&LayoutId::new(BUILT_IN), LayoutId::new("custom"))
            .expect_err("it refuses");
        assert!(matches!(refused, StoreError::Safe), "{refused}");
        assert!(refused.to_string().contains("--safe-layout"), "{refused}");
        assert!(matches!(
            store.commit(Transaction::new(
                "Anything",
                LayoutId::new("custom"),
                Vec::new()
            )),
            Err(StoreError::Safe)
        ));
        assert!(matches!(store.undo(), Err(StoreError::Safe)));
        assert!(matches!(store.redo(), Err(StoreError::Safe)));
        assert!(!store.has_unsaved());
    }

    /// The copy that a broken file falls back to is written once, however many times the pass that keeps it runs — it runs on every reload and every monitor change.
    #[test]
    fn a_last_good_copy_is_kept_once_until_the_layout_changes() {
        let dir = scratch("keep-once");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        let copy = dir.join(".last-good").join("mine.toml");

        store.keep_last_good(&mine).expect("it is kept");
        util::writer::flush();
        assert!(std::fs::metadata(&copy).is_ok(), "there is a copy");

        // A marker rather than a modification time: two writes a moment apart can share one on a filesystem whose clock did not tick between them, which is the very thing this has to tell apart.
        std::fs::write(&copy, "# untouched\n").unwrap();
        store.keep_last_good(&mine).expect("and again");
        util::writer::flush();
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            "# untouched\n",
            "the same layout is not written a second time"
        );

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
        store.keep_last_good(&mine).expect("it is kept again");
        util::writer::flush();
        let kept: Layout =
            toml::from_str(&std::fs::read_to_string(&copy).unwrap()).expect("it parses");
        assert!(
            children(&kept).is_empty(),
            "and a layout that changed is written again"
        );
    }

    /// The rescue draws the copy and leaves the file alone: a shell that quietly wrote an older copy over a broken layout would take away the evidence of what broke it.
    #[test]
    fn restoring_the_last_good_copy_changes_what_is_drawn_and_not_what_is_stored() {
        let dir = scratch("restore");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        store.keep_last_good(&mine).expect("it is kept");
        util::writer::flush();

        let broken = "this is not toml = = =";
        std::fs::write(store.path_of(&mine), broken).unwrap();
        assert!(!store.reload().is_clean());

        assert!(store.restore_last_good(&mine), "there is a copy");
        assert_eq!(
            children(store.get(&mine).expect("it is drawn")),
            ["clock-1"]
        );
        assert!(!store.has_unsaved(), "and nothing is queued to be written");
        assert_eq!(
            std::fs::read_to_string(store.path_of(&mine)).unwrap(),
            broken,
            "the file the user has to fix is exactly as they left it"
        );
    }

    #[derive(serde::Deserialize, serde::Serialize)]
    struct Rounded {
        radius: Corners,
    }

    fn radius_of(text: &str) -> Result<Corners, toml::de::Error> {
        toml::from_str::<Rounded>(text).map(|it| it.radius)
    }

    /// A radius is the one number a person types, or four when the corners differ — and a layout written back keeps whichever shape it had, so an edit never turns a hand-written `8` into four eights.
    #[test]
    fn a_radius_is_one_number_or_four_corners() {
        assert_eq!(radius_of("radius = 8").unwrap(), Corners::all(8.0));
        assert_eq!(radius_of("radius = 6.5").unwrap(), Corners::all(6.5));
        assert_eq!(
            radius_of("radius = [12, 12.0, 0, 4]").unwrap(),
            Corners::each(12.0, 12.0, 0.0, 4.0)
        );
        assert!(radius_of("radius = [1, 2, 3]").is_err(), "three corners");
        assert!(radius_of("radius = [1, 2, 3, 4, 5]").is_err(), "five");
        assert!(radius_of("radius = \"round\"").is_err(), "a word");

        let uniform = toml::to_string(&Rounded {
            radius: Corners::each(8.0, 8.0, 8.0, 8.0),
        })
        .unwrap();
        assert_eq!(uniform.trim(), "radius = 8.0");
        let per_corner = toml::to_string(&Rounded {
            radius: Corners::each(12.0, 12.0, 0.0, 0.0),
        })
        .unwrap();
        assert_eq!(
            radius_of(&per_corner).unwrap(),
            Corners::each(12.0, 12.0, 0.0, 0.0),
            "{per_corner}"
        );
    }

    /// A monitor rule that squares one bar's lower corners names the radius alone, and keeps the chips mode the bar was given everywhere.
    #[test]
    fn a_monitor_rule_changes_a_bar_s_corners_and_keeps_the_rest_of_its_shape() {
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
            [outputs.layers.top.areas.shape]
            mode = "chips"
            radius = 8
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            [outputs.layers.top.areas.shape]
            radius = [8, 8, 0, 0]
            [outputs.layers.top.areas.style]
            radius = [0, 0, 4, 4]
            "#,
        );
        let shape_on = |output: &str| {
            let resolved = alone(&parsed, output);
            let area = resolved.layer(LayerKind::Top).unwrap().areas[0].clone();
            let ResolvedAreaKind::Bar { shape, .. } = area.kind else {
                panic!("a bar");
            };
            (shape, area.style.radius)
        };
        let (shape, style) = shape_on("DP-1");
        assert_eq!(shape.radius, Some(Corners::each(8.0, 8.0, 0.0, 0.0)));
        assert_eq!(shape.mode, Some(config::Shape::Chips), "kept from `*`");
        assert_eq!(style, Some(Corners::each(0.0, 0.0, 4.0, 4.0)));
        let (shape, style) = shape_on("HDMI-A-1");
        assert_eq!(shape.radius, Some(Corners::all(8.0)));
        assert_eq!(style, None);
    }

    const DEAD_ZONE: &str = r#"
        id = "test"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar-top"
        kind = "bar"
        edge = "top"
        thickness = 32
        [outputs.layers.top.areas.actions]
        press = ["panel toggle launcher"]
        scroll_up = ["panel toggle volume"]
        [[outputs]]
        match = "DP-1"
        [[outputs.layers.top.areas]]
        id = "bar-top"
        [outputs.layers.top.areas.actions]
        press = ["panel toggle clock"]
    "#;

    /// A press on the bar between its chips is the area's own gesture, and a monitor rule rebinds one gesture without restating the other.
    #[test]
    fn an_area_s_actions_resolve_and_merge_by_gesture() {
        let parsed = layout(DEAD_ZONE);
        assert!(validate(&parsed, &Modules).is_clean());
        let actions = |output: &str| {
            alone(&parsed, output).layer(LayerKind::Top).unwrap().areas[0]
                .actions
                .iter()
                .map(|(trigger, action)| (trigger.as_str(), action.0.join("; ")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            actions("DP-1"),
            [
                ("press", "panel toggle clock".to_string()),
                ("scroll_up", "panel toggle volume".to_string())
            ]
        );
        assert_eq!(actions("HDMI-A-1")[0].1, "panel toggle launcher");
    }

    /// An area's command lines are checked on load exactly like an instance's, and refused on the lock layer the same way (TA-8).
    #[test]
    fn an_area_s_actions_are_checked_like_an_instance_s() {
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
            [outputs.layers.top.areas.actions]
            middle = ["summon the dead"]
            [[outputs.layers.lock.areas]]
            id = "readings"
            kind = "grid"
            [outputs.layers.lock.areas.actions]
            press = ["panel toggle launcher"]
            "#,
        );
        let report = validate(&parsed, &Modules);
        let keys: Vec<(&str, &str)> = report
            .findings()
            .map(|f| (f.key.as_str(), f.message.as_str()))
            .collect();
        assert!(
            keys.iter()
                .any(|(key, message)| key.ends_with("bar-top.actions.middle")
                    && message.contains("summon the dead")),
            "{keys:?}"
        );
        assert!(
            keys.iter().any(
                |(key, message)| key.ends_with("lock.areas.readings.actions")
                    && message.contains("readings, never controls")
            ),
            "{keys:?}"
        );
        assert!(
            !validate::validate_lock(&parsed, &Modules).is_clean(),
            "and the lock's own check, which decides the minimal lock, says so too"
        );
    }

    /// One bar that reserves, and a workspace rule of the same output with nothing in it yet.
    const RULED: &str = r#"
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
        [[outputs.workspaces]]
        match = "games"
    "#;

    fn games(layer: LayerKind) -> Site {
        Site::everywhere(layer).on_workspace("games")
    }

    fn bare_area(id: &str) -> Area {
        Area {
            id: AreaId::new(id),
            ..Area::default()
        }
    }

    /// Takes `edit` back and checks the layout is the one it started as, field for field and as written.
    fn undoes_exactly(start: &Layout, edit: &LayoutOp) {
        let mut edited = start.clone();
        let back = ops::apply(&mut edited, edit).unwrap_or_else(|why| panic!("{edit:?}: {why}"));
        assert_ne!(&edited, start, "{edit:?} changed nothing");
        let again = ops::apply(&mut edited, &back).expect("the undo applies");
        assert_eq!(
            &edited, start,
            "undoing {edit:?} did not restore the layout"
        );
        assert_eq!(
            toml::to_string(&edited).unwrap(),
            toml::to_string(start).unwrap()
        );
        let mut redone = edited.clone();
        ops::apply(&mut redone, &again).expect("the redo applies");
        ops::apply(&mut edited, edit).expect("the edit applies again");
        assert_eq!(redone, edited, "redoing {edit:?} is the edit itself");
    }

    #[test]
    fn every_edit_the_tools_add_can_be_taken_back_exactly() {
        let start = layout(RULED);
        let bar = Site::everywhere(LayerKind::Top);
        let edits = [
            LayoutOp::ReplaceArea {
                site: bar.clone(),
                id: AreaId::new("bar-top"),
                area: Box::new(Area {
                    kind: Some(AreaKind::Free {
                        rect: Some(Rect::default()),
                    }),
                    ..bare_area("note")
                }),
            },
            LayoutOp::SetAreaActions {
                site: bar.clone(),
                id: AreaId::new("bar-top"),
                actions: BTreeMap::from([(
                    Trigger::Secondary,
                    Action(vec!["panel toggle clock".into()]),
                )]),
            },
            LayoutOp::SetAreaStyle {
                site: bar.clone(),
                id: AreaId::new("bar-top"),
                style: Box::new(AreaStyle {
                    radius: Some(Corners::each(0.0, 0.0, 12.0, 12.0)),
                    ..AreaStyle::default()
                }),
            },
            LayoutOp::InsertOutputRule {
                index: 1,
                rule: Box::new(OutputRule {
                    matches: OutputMatch("DP-1".into()),
                    ..OutputRule::default()
                }),
            },
            LayoutOp::DeleteOutputRule {
                output: OutputMatch("*".into()),
            },
            LayoutOp::InsertWorkspaceRule {
                output: OutputMatch("*".into()),
                index: 0,
                rule: Box::new(WorkspaceRule {
                    matches: WorkspaceMatch("id:3".into()),
                    ..WorkspaceRule::default()
                }),
            },
            LayoutOp::DeleteWorkspaceRule {
                output: OutputMatch("*".into()),
                workspace: WorkspaceMatch("games".into()),
            },
            LayoutOp::InsertArea {
                site: games(LayerKind::Desktop),
                index: 0,
                area: Box::new(Area {
                    kind: Some(AreaKind::Grid {
                        rect: None,
                        cell: None,
                        gap: None,
                        anchor: None,
                    }),
                    ..bare_area("games-grid")
                }),
            },
            LayoutOp::SetLayerRemove {
                site: Site::everywhere(LayerKind::Desktop),
                remove: vec![AreaId::new("inherited-grid")],
            },
            LayoutOp::SetLayerRemove {
                site: games(LayerKind::Desktop),
                remove: vec![AreaId::new("inherited-grid"), AreaId::new("other")],
            },
        ];
        for edit in &edits {
            undoes_exactly(&start, edit);
        }

        let mut with_override = start.clone();
        ops::apply(
            &mut with_override,
            &LayoutOp::InsertArea {
                site: games(LayerKind::Top),
                index: 0,
                area: Box::new(Area {
                    groups: vec![Group {
                        id: GroupId::new("start"),
                        ..Group::default()
                    }],
                    ..bare_area("bar-top")
                }),
            },
        )
        .expect("a workspace rule may add to what a reserving bar holds");
        undoes_exactly(
            &with_override,
            &LayoutOp::InsertInstance {
                spot: Spot {
                    site: games(LayerKind::Top),
                    area: AreaId::new("bar-top"),
                    group: GroupId::new("start"),
                },
                index: 0,
                instance: Box::new(instance("battery-1", "battery")),
            },
        );
        undoes_exactly(
            &with_override,
            &LayoutOp::SetAreaStyle {
                site: games(LayerKind::Top),
                id: AreaId::new("bar-top"),
                style: Box::new(AreaStyle {
                    opacity: Some(0.5),
                    ..AreaStyle::default()
                }),
            },
        );
    }

    /// An editor's first edit for one monitor or one workspace makes the level it lands in, and a second level with the same match would leave which one an edit means to chance.
    #[test]
    fn a_rule_is_made_once_and_an_edit_lands_in_it() {
        let mut edited = layout(RULED);
        ops::apply(
            &mut edited,
            &LayoutOp::InsertOutputRule {
                index: 1,
                rule: Box::new(OutputRule {
                    matches: OutputMatch("DP-1".into()),
                    ..OutputRule::default()
                }),
            },
        )
        .expect("a monitor rule is made");
        ops::apply(
            &mut edited,
            &LayoutOp::InsertArea {
                site: Site::new("DP-1", LayerKind::Desktop),
                index: 0,
                area: Box::new(Area {
                    kind: Some(AreaKind::Free {
                        rect: Some(Rect::default()),
                    }),
                    ..bare_area("only-here")
                }),
            },
        )
        .expect("and holds an area");
        assert_eq!(
            area_ids(&alone(&edited, "DP-1"), LayerKind::Desktop),
            ["only-here"]
        );
        assert!(area_ids(&alone(&edited, "HDMI-A-1"), LayerKind::Desktop).is_empty());

        assert_eq!(
            ops::apply(
                &mut edited,
                &LayoutOp::InsertOutputRule {
                    index: 0,
                    rule: Box::new(OutputRule::default()),
                },
            )
            .unwrap_err(),
            OpError::RuleExists("outputs.*".into())
        );
        assert_eq!(
            ops::apply(
                &mut edited,
                &LayoutOp::InsertWorkspaceRule {
                    output: OutputMatch("*".into()),
                    index: 0,
                    rule: Box::new(WorkspaceRule {
                        matches: WorkspaceMatch("games".into()),
                        ..WorkspaceRule::default()
                    }),
                },
            )
            .unwrap_err(),
            OpError::RuleExists("outputs.*.workspaces.games".into())
        );
        assert_eq!(
            ops::apply(
                &mut edited,
                &LayoutOp::InsertArea {
                    site: games(LayerKind::Lock),
                    index: 0,
                    area: Box::new(bare_area("x")),
                },
            )
            .unwrap_err(),
            OpError::NoLockLayer {
                workspace: "games".into()
            }
        );
        assert!(matches!(
            ops::apply(
                &mut edited,
                &LayoutOp::DeleteArea {
                    site: Site::everywhere(LayerKind::Top).on_workspace("nowhere"),
                    id: AreaId::new("bar-top"),
                },
            ),
            Err(OpError::NoWorkspaceRule { .. })
        ));
    }

    /// TA-2's invariant held where the edit is made: a workspace rule changes what a reserving area holds, never whether it is there, what it reserves or how big it is.
    #[test]
    fn an_edit_in_a_workspace_rule_may_not_touch_reservation() {
        let start = layout(RULED);
        let refused = |edit: LayoutOp| {
            let mut edited = start.clone();
            let outcome = ops::apply(&mut edited, &edit);
            assert!(
                matches!(outcome, Err(OpError::Reservation(_))),
                "{edit:?} gave {outcome:?}"
            );
            assert_eq!(edited, start, "and changed nothing");
        };

        refused(LayoutOp::InsertArea {
            site: games(LayerKind::Top),
            index: 0,
            area: Box::new(Area {
                kind: Some(AreaKind::Bar {
                    edge: Some(config::Edge::Bottom),
                    thickness: Some(40.0),
                    length: None,
                    offset: None,
                    shape: BarShape::default(),
                    autohide: None,
                }),
                reserve: Some(true),
                ..bare_area("games-bar")
            }),
        });
        refused(LayoutOp::InsertArea {
            site: games(LayerKind::Top),
            index: 0,
            area: Box::new(Area {
                kind: Some(AreaKind::Bar {
                    edge: None,
                    thickness: Some(64.0),
                    length: None,
                    offset: None,
                    shape: BarShape::default(),
                    autohide: None,
                }),
                ..bare_area("bar-top")
            }),
        });
        refused(LayoutOp::InsertWorkspaceRule {
            output: OutputMatch("*".into()),
            index: 0,
            rule: Box::new(WorkspaceRule {
                matches: WorkspaceMatch("films".into()),
                layers: SessionLayers {
                    top: Layer {
                        areas: Vec::new(),
                        remove: vec![AreaId::new("bar-top")],
                    },
                    ..SessionLayers::default()
                },
            }),
        });
        refused(LayoutOp::SetLayerRemove {
            site: games(LayerKind::Top),
            remove: vec![AreaId::new("bar-top")],
        });

        let mut holding = start.clone();
        ops::apply(
            &mut holding,
            &LayoutOp::InsertArea {
                site: games(LayerKind::Top),
                index: 0,
                area: Box::new(bare_area("bar-top")),
            },
        )
        .expect("naming the bar to change what it holds is allowed");
        for edit in [
            LayoutOp::SetAreaKind {
                site: games(LayerKind::Top),
                id: AreaId::new("bar-top"),
                kind: Box::new(Some(AreaKind::Bar {
                    edge: None,
                    thickness: Some(64.0),
                    length: None,
                    offset: None,
                    shape: BarShape::default(),
                    autohide: None,
                })),
            },
            LayoutOp::SetAreaFlags {
                site: games(LayerKind::Top),
                id: AreaId::new("bar-top"),
                reserve: Some(false),
                above_fullscreen: None,
                visible: None,
            },
        ] {
            let mut edited = holding.clone();
            let outcome = ops::apply(&mut edited, &edit);
            assert!(
                matches!(outcome, Err(OpError::Reservation(_))),
                "{edit:?} gave {outcome:?}"
            );
            assert_eq!(edited, holding);
        }
        assert!(
            validate(&holding, &Modules).is_clean(),
            "what the ops allowed, validation accepts"
        );
    }

    /// The built-in layout's lock layer, where the prompt lives.
    fn locked() -> Layout {
        crate::built_in()
    }

    fn lock() -> Site {
        Site::everywhere(LayerKind::Lock)
    }

    /// The prompt can be moved and restyled, never removed, turned into something else or given an expression that could hide it — by any edit, whatever made it (TA-8).
    #[test]
    fn an_edit_can_move_and_restyle_the_prompt_and_nothing_else() {
        let start = locked();
        let prompt = AreaId::new("prompt");
        let refused = |edit: LayoutOp, expected: ops::PromptEdit| {
            let mut edited = start.clone();
            assert_eq!(
                ops::apply(&mut edited, &edit),
                Err(OpError::Prompt {
                    id: AreaId::new("prompt"),
                    refused: expected
                }),
                "{edit:?}"
            );
            assert_eq!(edited, start, "and changed nothing");
        };

        refused(
            LayoutOp::DeleteArea {
                site: lock(),
                id: prompt.clone(),
            },
            ops::PromptEdit::Remove,
        );
        refused(
            LayoutOp::DeleteOutputRule {
                output: OutputMatch("*".into()),
            },
            ops::PromptEdit::Remove,
        );
        refused(
            LayoutOp::SetAreaKind {
                site: lock(),
                id: prompt.clone(),
                kind: Box::new(Some(AreaKind::Free {
                    rect: Some(Rect::default()),
                })),
            },
            ops::PromptEdit::ChangeKind,
        );
        refused(
            LayoutOp::SetAreaKind {
                site: lock(),
                id: prompt.clone(),
                kind: Box::new(None),
            },
            ops::PromptEdit::ChangeKind,
        );
        refused(
            LayoutOp::ReplaceArea {
                site: lock(),
                id: prompt.clone(),
                area: Box::new(bare_area("prompt")),
            },
            ops::PromptEdit::ChangeKind,
        );
        refused(
            LayoutOp::SetAreaFlags {
                site: lock(),
                id: prompt.clone(),
                reserve: None,
                above_fullscreen: None,
                visible: Some(Expr("$battery.percent > 50".into())),
            },
            ops::PromptEdit::Hide,
        );
        refused(
            LayoutOp::SetLayerRemove {
                site: lock(),
                remove: vec![prompt.clone()],
            },
            ops::PromptEdit::Remove,
        );

        undoes_exactly(
            &start,
            &LayoutOp::SetAreaKind {
                site: lock(),
                id: prompt.clone(),
                kind: Box::new(Some(AreaKind::Prompt {
                    rect: Some(Rect {
                        x: 0.1,
                        y: 0.6,
                        w: 0.3,
                        h: 0.3,
                    }),
                })),
            },
        );
        undoes_exactly(
            &start,
            &LayoutOp::SetAreaStyle {
                site: lock(),
                id: prompt.clone(),
                style: Box::new(AreaStyle {
                    fill: Some("base".into()),
                    radius: Some(Corners::each(24.0, 24.0, 0.0, 0.0)),
                    ..AreaStyle::default()
                }),
            },
        );
        undoes_exactly(
            &start,
            &LayoutOp::DeleteArea {
                site: lock(),
                id: AreaId::new("lock-readings"),
            },
        );
    }

    fn prompt_filled(fill: &str) -> Resolved {
        alone(
            &layout(&format!(
                r#"
                id = "test"
                [[outputs]]
                match = "*"
                [[outputs.layers.lock.areas]]
                id = "prompt"
                kind = "prompt"
                style = {{ fill = "{fill}" }}
                "#
            )),
            "DP-1",
        )
    }

    /// A prompt drawn on a card its own text cannot be read on is as much a lockout as a hidden one, so a fill below WCAG AA is refused — and a refusal is the minimal lock, not a quietly restyled prompt (TA-8).
    #[test]
    fn a_prompt_card_its_text_cannot_be_read_on_is_refused() {
        let theme = theme();
        for unreadable in ["text", "#eceff4", "subtle"] {
            let report = validate_resolved(&prompt_filled(unreadable), "layouts/test.toml", &theme);
            assert!(
                report
                    .findings()
                    .any(|f| f.key.ends_with("prompt.style.fill") && f.message.contains("WCAG AA")),
                "`{unreadable}` under the prompt's text: {}",
                report.render()
            );
        }
        for readable in ["surface", "base", "#000000"] {
            let report = validate_resolved(&prompt_filled(readable), "layouts/test.toml", &theme);
            assert!(report.is_clean(), "`{readable}`: {}", report.render());
        }
    }

    /// The prompt's card is the area's own `style`, and it has to come back from the file it was written to: a style the prompt kept somewhere a file could not reach was a style the lock never drew.
    #[test]
    fn a_prompt_s_style_survives_being_written_and_read_back() {
        let mut styled = locked();
        let prompt = styled.outputs[0]
            .layers
            .lock
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "prompt")
            .expect("the built-in lock has a prompt");
        prompt.style = AreaStyle {
            fill: Some("overlay".into()),
            radius: Some(Corners::each(20.0, 20.0, 4.0, 4.0)),
            opacity: Some(0.95),
            ..AreaStyle::default()
        };
        let again = layout(&toml::to_string_pretty(&styled).unwrap());
        assert_eq!(again, styled);
        let resolved = alone(&again, "DP-1");
        let drawn = resolved
            .layer(LayerKind::Lock)
            .and_then(|layer| layer.areas.iter().find(|area| area.id.as_str() == "prompt"))
            .expect("it resolves");
        assert_eq!(drawn.style.fill.as_deref(), Some("overlay"));
        assert!(validate_resolved(&resolved, "layouts/test.toml", &theme()).is_clean());
    }

    fn thickness_of(layout: &Layout, known: &BTreeMap<LayoutId, Layout>) -> Option<f32> {
        let (resolved, _) = resolve(layout, known, "DP-1", None);
        resolved
            .layer(LayerKind::Top)?
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .and_then(|area| match area.kind {
                ResolvedAreaKind::Bar { thickness, .. } => Some(thickness),
                _ => None,
            })
    }

    fn thicken(layout: &mut Layout, to: f32) {
        let bar = layout.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the bar");
        if let Some(AreaKind::Bar { thickness, .. }) = &mut bar.kind {
            *thickness = Some(to);
        }
    }

    /// A reset puts back what the layout the edited one extends says, laid over its own parents — and what the built-in one says when it extends none.
    #[test]
    fn a_reset_restores_from_the_layout_extended_and_else_from_the_built_in_one() {
        let mut parent = built_in();
        parent.id = LayoutId::new("parent");
        thicken(&mut parent, 40.0);
        let mut mine = built_in();
        mine.id = LayoutId::new("mine");
        mine.extends = Some(LayoutId::new("parent"));
        thicken(&mut mine, 50.0);
        let known = BTreeMap::from([
            (parent.id.clone(), parent.clone()),
            (mine.id.clone(), mine.clone()),
        ]);
        let bar = AreaId::new("bar-top");

        let base = reset::base_of(&mine, &known);
        let ops = reset::ops(&mine, &base, reset::Target::Area(&bar)).expect("the bar is known");
        let mut reset = mine.clone();
        ops::apply_all(&mut reset, &ops).expect("the reset applies");
        assert_eq!(
            thickness_of(&reset, &known),
            Some(40.0),
            "back to the parent's"
        );

        mine.extends = None;
        let base = reset::base_of(&mine, &known);
        let ops = reset::ops(&mine, &base, reset::Target::Area(&bar)).expect("the bar is known");
        let mut reset = mine.clone();
        ops::apply_all(&mut reset, &ops).expect("the reset applies");
        assert_eq!(
            thickness_of(&reset, &known),
            thickness_of(&built_in(), &known),
            "back to the built-in bar"
        );
        assert!(
            reset::ops(
                &mine,
                &base,
                reset::Target::Instance(&InstanceId::new("nothing"))
            )
            .is_none()
        );
    }

    /// A new area is named after the one it came from, counting past every id of its layer the layout or one it extends has ever written there, and a count on the stem is not counted twice.
    #[test]
    fn a_free_area_id_counts_past_every_id_its_layer_has() {
        let mut mine = built_in();
        mine.id = LayoutId::new("mine");
        mine.extends = Some(LayoutId::new(BUILT_IN));
        mine.outputs[0]
            .layers
            .background
            .remove
            .push(AreaId::new("background-2"));
        let known = BTreeMap::from([(LayoutId::new(BUILT_IN), built_in())]);
        let free = |stem: &str| ops::free_area_id(&mine, &known, LayerKind::Background, stem);
        assert_eq!(free("background"), AreaId::new("background-3"));
        assert_eq!(free("background-3"), AreaId::new("background-3"));
        assert_eq!(free("left"), AreaId::new("left"));
        assert_eq!(
            ops::free_area_id(&mine, &known, LayerKind::Top, "background"),
            AreaId::new("background"),
            "ids are per layer"
        );
    }

    /// A new instance is named after its module, counting past every instance the layout or one it extends places or takes away anywhere: ids are unique across the whole layout, not per layer.
    #[test]
    fn a_free_instance_id_counts_past_every_instance_of_the_chain() {
        let mut mine = Layout {
            id: LayoutId::new("mine"),
            extends: Some(LayoutId::new(BUILT_IN)),
            outputs: vec![OutputRule::default()],
            ..Layout::default()
        };
        mine.outputs[0].layers.desktop.areas.push(Area {
            groups: vec![Group {
                id: GroupId::new("gone"),
                remove: vec![InstanceId::new("clock-3")],
                ..Group::default()
            }],
            ..bare_area("widgets")
        });
        let known = BTreeMap::from([(LayoutId::new(BUILT_IN), built_in())]);
        let free = |stem: &str| ops::free_instance_id(&mine, &known, stem);
        assert_eq!(
            free("clock"),
            InstanceId::new("clock-4"),
            "the bar's, the desktop's and one taken away are taken"
        );
        assert_eq!(free("battery"), InstanceId::new("battery"));
        assert_eq!(free("workspaces-2"), InstanceId::new("workspaces-2"));
    }

    /// A new group is named after its zone, counting past every group its area has at any level — placed by the layout it extends, taken away, refined for a workspace — and only in that area.
    #[test]
    fn a_free_group_id_counts_past_every_group_its_area_has() {
        let mut mine = Layout {
            id: LayoutId::new("mine"),
            extends: Some(LayoutId::new(BUILT_IN)),
            outputs: vec![OutputRule::default()],
            ..Layout::default()
        };
        mine.outputs[0].layers.top.areas.push(Area {
            remove: vec![GroupId::new("end")],
            ..bare_area("bar-top")
        });
        mine.outputs[0].workspaces.push(WorkspaceRule {
            matches: WorkspaceMatch("2".into()),
            layers: SessionLayers {
                top: Layer {
                    areas: vec![Area {
                        groups: vec![Group {
                            id: GroupId::new("end-2"),
                            ..Group::default()
                        }],
                        ..bare_area("bar-top")
                    }],
                    ..Layer::default()
                },
                ..SessionLayers::default()
            },
        });
        let known = BTreeMap::from([(LayoutId::new(BUILT_IN), built_in())]);
        let bar = AreaId::new("bar-top");
        let free = |stem: &str| ops::free_group_id(&mine, &known, LayerKind::Top, &bar, stem);
        assert_eq!(
            free("end"),
            GroupId::new("end-3"),
            "the one taken away and the workspace's are taken"
        );
        assert_eq!(free("start"), GroupId::new("start-2"));
        assert_eq!(free("middle"), GroupId::new("middle"));
        assert_eq!(
            ops::free_group_id(
                &mine,
                &known,
                LayerKind::Desktop,
                &AreaId::new("widgets"),
                "end"
            ),
            GroupId::new("end"),
            "ids are per area"
        );
    }

    /// An area a broader level places is taken off one level by naming it in that level's `remove`, and the op that does so gives back exactly what the list said before.
    #[test]
    fn a_layer_takes_an_inherited_area_away_through_its_remove_list() {
        let mut mine = Layout {
            id: LayoutId::new("mine"),
            extends: Some(LayoutId::new(BUILT_IN)),
            outputs: vec![OutputRule::default()],
            ..Layout::default()
        };
        let known = BTreeMap::from([(LayoutId::new(BUILT_IN), built_in())]);
        let hide = LayoutOp::SetLayerRemove {
            site: Site::everywhere(LayerKind::Overlay),
            remove: vec![AreaId::new("stack")],
        };
        let back = ops::apply(&mut mine, &hide).expect("an inherited area is taken away");
        let (resolved, _) = resolve(&mine, &known, "DP-1", None);
        assert!(area_ids(&resolved, LayerKind::Overlay).is_empty());
        ops::apply(&mut mine, &back).expect("and put back");
        let (resolved, _) = resolve(&mine, &known, "DP-1", None);
        assert_eq!(area_ids(&resolved, LayerKind::Overlay), ["stack"]);
    }

    /// The built-in layout's desktop shows the clock face where the old default `[widgets.clock]` put it: centred on the whole output, held 48 px off its edges, at the medium size, one instance of the clock module like the bar's chip is another.
    #[test]
    fn the_built_in_desktop_shows_the_clock_where_the_old_default_put_it() {
        let resolved = alone(&built_in(), "DP-1");
        let desktop = resolved.layer(LayerKind::Desktop).expect("a desktop layer");
        let [widgets] = desktop.areas.as_slice() else {
            panic!(
                "one area on the desktop: {:?}",
                area_ids(&resolved, LayerKind::Desktop)
            );
        };
        assert_eq!(
            widgets.kind,
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::Center,
            }
        );
        assert_eq!(widgets.within, Within::Output);
        assert_eq!(widgets.style.padding, Some(48.0));
        let [clock] = widgets.groups.as_slice() else {
            panic!("one group");
        };
        assert!(matches!(clock.kind, GroupKind::Cell { col: 0, row: 0, .. }));
        let [face] = clock.children.as_slice() else {
            panic!("one instance");
        };
        assert_eq!(
            (face.id.as_str(), face.module.as_str()),
            ("clock-2", "clock")
        );
        assert_eq!(face.representation, Representation::WidgetM);
        assert!(
            face.options.is_empty(),
            "drawn as the clock module draws it by default"
        );
    }

    fn routed(
        kind: CardKind,
        app: Option<&'static str>,
        urgency: Option<Urgency>,
    ) -> RoutedCard<'static> {
        RoutedCard { kind, app, urgency }
    }

    fn route(kind: Option<CardKind>, app: Option<&str>, urgency: Option<Urgency>) -> Route {
        Route {
            kind,
            app: app.map(str::to_string),
            urgency,
        }
    }

    /// Every field a route sets has to match, the app without regard to case; a field it leaves out matches anything, and an app or an urgency is something only a notification has.
    #[test]
    fn a_route_takes_what_every_field_it_sets_matches() {
        use CardKind::*;
        let firefox = Some("Firefox");
        let table: &[(Route, RoutedCard, bool)] = &[
            (Route::default(), routed(Toast, None, None), true),
            (
                Route::default(),
                routed(Notification, firefox, Some(Urgency::Low)),
                true,
            ),
            (route(Some(Osd), None, None), routed(Osd, None, None), true),
            (
                route(Some(Osd), None, None),
                routed(Toast, None, None),
                false,
            ),
            (
                route(None, Some("firefox"), None),
                routed(Notification, firefox, Some(Urgency::Normal)),
                true,
            ),
            (
                route(None, Some("firefox"), None),
                routed(Notification, Some("Slack"), Some(Urgency::Normal)),
                false,
            ),
            (
                route(None, Some("firefox"), None),
                routed(Toast, None, None),
                false,
            ),
            (
                route(Some(Notification), None, Some(Urgency::Critical)),
                routed(Notification, firefox, Some(Urgency::Critical)),
                true,
            ),
            (
                route(Some(Notification), None, Some(Urgency::Critical)),
                routed(Notification, firefox, Some(Urgency::Normal)),
                false,
            ),
            (
                route(None, None, Some(Urgency::Low)),
                routed(Osd, None, None),
                false,
            ),
            (
                route(Some(Notification), Some("Slack"), Some(Urgency::Critical)),
                routed(Notification, Some("slack"), Some(Urgency::Critical)),
                true,
            ),
            (
                route(Some(Notification), Some("Slack"), Some(Urgency::Critical)),
                routed(Notification, Some("slack"), Some(Urgency::Low)),
                false,
            ),
        ];
        for (route, card, takes) in table {
            assert_eq!(route.takes(card), *takes, "{route:?} for {card:?}");
        }
    }

    /// F-2.8: a card goes to the first stack with a route that takes it, and a stack with no routes takes the rest — the first of them, wherever it is in the order — while a stack whose routes take nothing of the card's never sees it.
    #[test]
    fn a_card_goes_to_the_first_route_that_takes_it_and_else_to_the_first_stack_with_none() {
        let critical = route(Some(CardKind::Notification), None, Some(Urgency::Critical));
        let osd = route(Some(CardKind::Osd), None, None);
        let stacks: Vec<(&str, Vec<Route>)> = vec![
            ("corner", Vec::new()),
            ("centre", vec![critical.clone()]),
            ("spare", Vec::new()),
            ("bottom", vec![osd, critical]),
        ];
        let landing =
            |card: RoutedCard| route_card(&stacks, |(_, routes)| routes, &card).map(|(id, _)| *id);
        assert_eq!(
            landing(routed(
                CardKind::Notification,
                Some("mail"),
                Some(Urgency::Critical)
            )),
            Some("centre"),
            "the first route that takes it, though a stack with none comes before it"
        );
        assert_eq!(landing(routed(CardKind::Osd, None, None)), Some("bottom"));
        assert_eq!(
            landing(routed(CardKind::Toast, None, None)),
            Some("corner"),
            "no route takes a toast, so the first stack with none does"
        );
        assert_eq!(
            landing(routed(
                CardKind::Notification,
                Some("mail"),
                Some(Urgency::Normal)
            )),
            Some("corner")
        );
        let routed_only = &stacks[1..2];
        assert_eq!(
            route_card(
                routed_only,
                |(_, routes)| routes,
                &routed(CardKind::Toast, None, None)
            ),
            None,
            "with no stack that routes nothing, what no route takes is shown nowhere"
        );
    }
}
