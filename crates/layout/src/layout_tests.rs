//! What the layout model has to keep doing: the precedence, the merge by id, and the two rules that protect the user's windows and their lock screen.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::resolve::{ActiveWorkspace, Resolved, ResolvedAreaKind, resolve};
    use crate::validate::{Catalogue, validate, validate_resolved, validate_unsets};
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

        fn option_problems(
            &self,
            module: &str,
            options: &toml::Table,
        ) -> Vec<(String, util::report::Message)> {
            match module {
                "clock" => config::fields::check(
                    &config::fields::section("clock").expect("[clock]"),
                    options,
                ),
                _ => Vec::new(),
            }
        }

        fn is_service_source(&self, name: &str) -> bool {
            name == "battery"
        }

        /// `$battery.percent` is a public number, `$battery.cells` a list of them, and `$secrets` a private list that reads empty on the lock; `$temp` is a command source that is not `lock_safe`.
        fn compile_with(
            &self,
            source: &str,
            on_lock: bool,
            locals: &crate::Locals,
        ) -> Result<telar_expression::Compiled, telar_expression::Errors> {
            use telar_expression::{HostError, Reference, Registry, Type};
            let names = move |reference: &Reference| {
                if reference.path.is_empty()
                    && let Some(ty) = locals.get(&reference.name)
                {
                    return Ok(ty.clone());
                }
                match reference.dotted().as_str() {
                    "battery.percent" => Ok(Type::Number),
                    "battery.cells" => Ok(Type::list(Type::Number)),
                    "secrets" => Ok(Type::list(Type::Text)),
                    "temp" if on_lock => Err(HostError::new(
                        "test.lock",
                        "`$temp` runs a command, and the lock screen reads only sources marked `lock_safe = true`",
                    )),
                    "temp" => Ok(Type::Number),
                    other => Err(HostError::new(
                        "test.unknown",
                        format!("nothing is called `${other}`"),
                    )),
                }
            };
            telar_expression::compile(source, &names, &Registry::standard())
        }

        /// `$later` is a variable not set yet.
        fn awaits_variable(&self, source: &str, error: &telar_expression::Error) -> bool {
            source.get(error.span.range()) == Some("$later")
        }

        fn binding_type(
            &self,
            module: &str,
            path: &str,
        ) -> Result<telar_expression::Type, util::report::Message> {
            use telar_expression::Type;
            if let Some(key) = StyleBinding::from_path(path) {
                return Ok(key.ty());
            }
            match (module, path) {
                (_, "accent") => Ok(Type::Color),
                ("clock", "show_date") => Ok(Type::Bool),
                ("clock", "date_format") => Ok(Type::Text),
                _ => Err(util::report::Message::verbatim(format!(
                    "`{module}` has no option `{path}` to bind"
                ))),
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
        resolve(layout, &Library::default(), output, None).0
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

        let known = Library::of_layouts([base]);
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
        let known = Library::of_layouts([parsed.clone()]);
        let (_, report) = resolve(&parsed, &known, "DP-1", None);
        assert!(!report.is_clean(), "the cycle is reported");
    }

    /// A chain of layouts each extending the next is followed no deeper than `EXTENDS_DEPTH`, and the layout past it is a finding at `extends`: a bundle of hundreds of layouts in a line costs no more to resolve than a short chain.
    #[test]
    fn a_chain_deeper_than_the_limit_is_reported_at_extends() {
        let chain: Vec<Layout> = (0..40)
            .map(|n| Layout {
                id: LayoutId::new(format!("l{n}")),
                extends: (n < 39).then(|| LayoutId::new(format!("l{}", n + 1))),
                ..Layout::default()
            })
            .collect();
        let known = Library::of_layouts(chain.clone());
        let (_, report) = resolve(&chain[0], &known, "DP-1", None);
        let deep: Vec<_> = report
            .errors
            .iter()
            .filter(|finding| finding.message.key() == Some("finding.extends_too_deep"))
            .collect();
        assert_eq!(deep.len(), 1, "{}", report.render());
        assert_eq!(deep[0].key, "extends");
        assert_eq!(deep[0].file, std::path::Path::new("layouts/l0.toml"));
        let shallow = Library::of_layouts(chain[40 - crate::resolve::EXTENDS_DEPTH..].to_vec());
        let (_, report) = resolve(
            &chain[40 - crate::resolve::EXTENDS_DEPTH],
            &shallow,
            "DP-1",
            None,
        );
        assert!(
            report
                .errors
                .iter()
                .all(|finding| finding.message.key() != Some("finding.extends_too_deep")),
            "a chain of exactly the limit is followed whole: {}",
            report.render()
        );
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
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
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
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
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
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", Some(&web));
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
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", Some(&nameless));
        assert!(
            area_ids(&resolved, LayerKind::Desktop).is_empty(),
            "the rule did not apply"
        );
        assert!(
            report
                .findings()
                .any(|f| f.message.key() == Some("finding.workspace_id_needs_hyprland")),
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
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", Some(&third));
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
            report
                .findings()
                .any(|f| f.message.key() == Some("finding.lock_interactive")),
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
            report
                .findings()
                .any(|f| f.message.key() == Some("finding.no_prompt")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn a_prompt_that_could_be_hidden_or_faded_is_refused() {
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
        let resolved = alone(&faded, "DP-1");
        let report = validate_resolved(&resolved, "layouts/test.toml", &theme());
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
            !keys.iter().any(|k| k.ends_with("over-it")),
            "an area the file lists after it is resolved under it, so it covers nothing: {keys:?}"
        );
        assert_eq!(
            area_ids(&resolved, LayerKind::Lock),
            ["over-it", "prompt"],
            "the prompt is drawn last"
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
        let messages: Vec<String> = report.findings().map(|f| f.message.english()).collect();
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
        let said: Vec<String> = report.findings().map(|f| f.message.english()).collect();
        let found: Vec<(&str, &str, &str)> = report
            .findings()
            .zip(&said)
            .map(|(f, said)| (f.file.to_str().unwrap_or(""), f.key.as_str(), said.as_str()))
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

    const BINDING_AT: &str =
        "outputs.*.layers.top.areas.bar-top.groups.start.children.clock-1.bindings";

    /// Every binding is compiled where it is written, and each mistake is a finding at its key with the span inside the expression it is about.
    #[test]
    fn a_binding_that_does_not_compile_is_reported_at_its_key_with_its_span() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "$battery.percent > 50 && $nope"
            date_format = "fmt('{{}}%', $battery.percent)"
            "#
        ));
        let report = validate(&parsed, &Modules);
        let said: Vec<String> = report.findings().map(|f| f.message.english()).collect();
        let found: Vec<(&str, &str, Option<std::ops::Range<usize>>)> = report
            .findings()
            .zip(&said)
            .map(|(f, said)| {
                (
                    f.key.as_str(),
                    said.as_str(),
                    f.span.as_ref().map(|span| span.bytes.clone()),
                )
            })
            .collect();
        assert_eq!(
            found,
            [(
                format!("{BINDING_AT}.show_date").as_str(),
                "nothing is called `$nope`",
                Some(25..30)
            )],
            "{}",
            report.render()
        );
    }

    /// Variables are set while the shell runs, so a layout may name one before it is: a warning where the variable is read, every other mistake still an error, and nothing refused by `layout set`.
    #[test]
    fn a_variable_not_set_yet_is_a_warning_where_it_is_read() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "$later && $battery.percent > 50"
            date_format = "$later + $nope"
            "#
        ));
        let report = validate(&parsed, &Modules);
        let said = |findings: &[util::report::Finding]| {
            findings
                .iter()
                .map(|f| {
                    (
                        f.key.trim_start_matches(BINDING_AT).to_string(),
                        f.span.as_ref().map(|span| span.bytes.clone()),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            said(&report.errors),
            [(".date_format".to_string(), Some(9..14))],
            "{}",
            report.render()
        );
        assert_eq!(
            said(&report.warnings),
            [
                (".date_format".to_string(), Some(0..6)),
                (".show_date".to_string(), Some(0..6)),
            ],
            "{}",
            report.render()
        );
        let expr = Expr("$later".to_string());
        assert!(binding_errors(&Modules, Some("clock"), "show_date", &expr, false).is_empty());
    }

    #[test]
    fn a_binding_must_give_the_type_its_option_takes() {
        let parsed = layout(&format!(
            r##"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "$battery.percent"
            accent = "#88c0d0"
            colour = "true"
            "##
        ));
        let report = validate(&parsed, &Modules);
        let said: Vec<String> = report.findings().map(|f| f.message.english()).collect();
        let found: Vec<(&str, &str)> = report
            .findings()
            .zip(&said)
            .map(|(f, said)| (f.key.as_str(), said.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    format!("{BINDING_AT}.colour").as_str(),
                    "`clock` has no option `colour` to bind"
                ),
                (
                    format!("{BINDING_AT}.show_date").as_str(),
                    "expected bool, but this gives number"
                ),
            ],
            "{}",
            report.render()
        );
        let mismatch = report
            .findings()
            .find(|f| f.key.ends_with("show_date"))
            .and_then(|f| f.span.clone())
            .expect("located");
        assert_eq!(
            mismatch.bytes,
            0..16,
            "the whole expression is the wrong type"
        );
    }

    #[test]
    fn an_area_s_visible_has_to_give_true_or_false() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.top.areas]]
            id = "shown"
            kind = "free"
            rect = {{ x = 0.0, y = 0.0, w = 0.5, h = 0.5 }}
            visible = "$battery.percent > 50"
            [[outputs.layers.top.areas]]
            id = "counted"
            kind = "free"
            rect = {{ x = 0.0, y = 0.0, w = 0.5, h = 0.5 }}
            visible = "$battery.percent"
            "#
        ));
        let report = validate(&parsed, &Modules);
        let keys: Vec<&str> = report.findings().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["outputs.*.layers.top.areas.counted.visible"],
            "{}",
            report.render()
        );
    }

    /// A reserving area hidden by its expression still holds its edge, since reservation never follows a workspace or a reading (F-6.7), and that is worth saying.
    #[test]
    fn a_reserving_area_with_a_visible_expression_is_warned_about() {
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
            visible = "$battery.percent > 50"
            "#,
        );
        let report = validate(&parsed, &Modules);
        assert!(report.errors.is_empty(), "{}", report.render());
        assert!(
            report
                .warnings
                .iter()
                .any(|f| f.key == "outputs.*.layers.top.areas.bar-top.visible"),
            "{}",
            report.render()
        );
    }

    /// On the lock layer an expression reads what anyone may: a command source that is not `lock_safe` is refused there and nowhere else, and the lock's own check says so.
    #[test]
    fn the_lock_layer_refuses_a_command_source_that_is_not_lock_safe() {
        let on = |layer: &str| {
            layout(&format!(
                r#"
                id = "test"
                [[outputs]]
                match = "*"
                [[outputs.layers.{layer}.areas]]
                id = "readings"
                kind = "free"
                rect = {{ x = 0.0, y = 0.0, w = 0.5, h = 0.5 }}
                visible = "$temp > 30"
                [[outputs.layers.{layer}.areas.groups]]
                id = "g"
                place = "zone"
                zone = "start"
                [[outputs.layers.{layer}.areas.groups.children]]
                id = "clock-1"
                module = "clock"
                [outputs.layers.{layer}.areas.groups.children.bindings]
                show_date = "$temp > 30"
                "#
            ))
        };
        assert!(
            validate(&on("desktop"), &Modules).is_clean(),
            "{}",
            validate(&on("desktop"), &Modules).render()
        );

        let report = crate::validate_lock(&on("lock"), &Modules);
        assert!(
            report.errors.is_empty(),
            "a binding the lock cannot read is left out, never a reason to fall back to the minimal lock: {}",
            report.render()
        );
        let keys: Vec<&str> = report.findings().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "outputs.*.layers.lock.areas.readings.visible",
                "outputs.*.layers.lock.areas.readings.groups.g.children.clock-1.bindings.show_date",
            ],
            "{}",
            report.render()
        );
        assert!(
            report
                .findings()
                .all(|f| f.message.english().contains("lock_safe")),
            "{}",
            report.render()
        );
    }

    /// `layout check` has the file's text, so it points at the mistake in the file rather than inside the expression.
    #[test]
    fn an_expression_finding_is_placed_in_the_file_it_was_written_in() {
        let text = format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "1 > $nope"
            "#
        );
        let mut report = validate(&layout(&text), &Modules);
        crate::locate_expressions(&text, &mut report);
        let span = report
            .findings()
            .next()
            .and_then(|f| f.span.clone())
            .expect("a located finding");
        assert_eq!(&text[span.bytes.clone()], "$nope");
        assert_eq!(
            span.line,
            text[..span.bytes.start].matches('\n').count() + 1
        );
    }

    /// `ONE_BAR`'s start group repeated over `repeat`, its clock bound as `bindings` says.
    fn repeated(repeat: &str, bindings: &str) -> String {
        format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            {bindings}
            "#
        )
        .replacen(
            "zone = \"start\"",
            &format!("zone = \"start\"\n        repeat = \"{repeat}\""),
            1,
        )
    }

    const START_AT: &str = "outputs.*.layers.top.areas.bar-top.groups.start";

    /// Inside a repeated group a child reads its item, of the list's element type, and its index; outside one neither name exists.
    #[test]
    fn a_repeated_group_s_children_read_their_item_and_index() {
        let parsed = layout(&repeated(
            "$battery.cells",
            "show_date = \"$item > 50 && $index < 3\"\ndate_format = \"fmt('{}', $item)\"",
        ));
        let report = validate(&parsed, &Modules);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(
            alone(&parsed, "DP-1")
                .layer(LayerKind::Top)
                .expect("a top layer")
                .areas[0]
                .groups[0]
                .repeat
                .as_ref()
                .map(|repeat| &repeat.expr),
            Some(&Expr("$battery.cells".into()))
        );

        let typed = validate(
            &layout(&repeated("$battery.cells", "show_date = \"$item\"")),
            &Modules,
        );
        let said: Vec<String> = typed.findings().map(|f| f.message.english()).collect();
        let found: Vec<(&str, &str)> = typed
            .findings()
            .zip(&said)
            .map(|(f, said)| (f.key.as_str(), said.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                format!("{BINDING_AT}.show_date").as_str(),
                "expected bool, but this gives number"
            )],
            "`$item` is a number here, since the list is of numbers: {}",
            typed.render()
        );

        let outside = validate(
            &layout(&format!(
                "{ONE_BAR}\n[outputs.layers.top.areas.groups.children.bindings]\nshow_date = \"$index > 0\"\n"
            )),
            &Modules,
        );
        assert!(
            outside
                .findings()
                .any(|f| f.key.ends_with("show_date") && f.message.english().contains("$index")),
            "{}",
            outside.render()
        );
    }

    /// A `repeat` has to give a list, and its mistakes are located in the file like any expression's. One that gives no list is reported there once, not again by every child that reads `$item`.
    #[test]
    fn a_repeat_that_gives_no_list_is_reported_once_at_its_key() {
        let text = repeated("$battery.percent", "show_date = \"$item > 1\"");
        let report = validate(&layout(&text), &Modules);
        let said: Vec<String> = report.findings().map(|f| f.message.english()).collect();
        let found: Vec<(&str, &str)> = report
            .findings()
            .zip(&said)
            .map(|(f, said)| (f.key.as_str(), said.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                format!("{START_AT}.repeat").as_str(),
                "expected a list to repeat the group's children over, but this gives number"
            )],
            "{}",
            report.render()
        );

        let text = repeated("$battery.nope", "show_date = \"$item > 1\"");
        let mut report = validate(&layout(&text), &Modules);
        crate::locate_expressions(&text, &mut report);
        let finding = report.findings().next().expect("a finding");
        assert_eq!(finding.key, format!("{START_AT}.repeat"));
        let span = finding.span.clone().expect("located");
        assert_eq!(&text[span.bytes], "$battery.nope");
    }

    /// DEC-23: a grid cell covers the cells it was placed at and no more, so a `repeat` there is refused — and a file edited past that draws the children once, as written.
    #[test]
    fn a_grid_cell_refuses_repeat_and_draws_its_children_once() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            rect = { x = 0.0, y = 0.0, w = 0.5, h = 0.5 }
            [[outputs.layers.desktop.areas.groups]]
            id = "g"
            place = "cell"
            col = 0
            row = 0
            repeat = "$battery.cells"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            representation = "widget_s"
            "#,
        );
        let report = validate(&parsed, &Modules);
        let keys: Vec<&str> = report.errors.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["outputs.*.layers.desktop.areas.widgets.groups.g.repeat"],
            "{}",
            report.render()
        );
        let resolved = alone(&parsed, "DP-1");
        let group = &resolved
            .layer(LayerKind::Desktop)
            .expect("a desktop layer")
            .areas[0]
            .groups[0];
        assert_eq!(group.repeat, None);
        assert_eq!(group.children.len(), 1);
    }

    /// A monitor rule that adds a child to a group the `*` rule repeats is checked as a child of that repeat, and a group the layout inherits from its parent, which this file cannot see, is given the benefit of the doubt rather than a false error.
    #[test]
    fn a_child_added_by_another_level_reads_the_repeat_of_the_group_it_lands_in() {
        let added = layout(&format!(
            r#"{}
                [[outputs]]
                match = "DP-1"
                [[outputs.layers.top.areas]]
                id = "bar-top"
                [[outputs.layers.top.areas.groups]]
                id = "start"
                [[outputs.layers.top.areas.groups.children]]
                id = "clock-2"
                module = "clock"
                [outputs.layers.top.areas.groups.children.bindings]
                show_date = "$item > 1"
                "#,
            repeated("$battery.cells", "")
        ));
        let report = validate(&added, &Modules);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(
            crate::child_locals(
                &added,
                &Modules,
                &Site::new("DP-1", LayerKind::Top),
                &AreaId::new("bar-top"),
                &GroupId::new("start"),
            ),
            crate::Locals::of_copy(telar_expression::Type::Number),
            "what `layout set` checks a binding of that child against"
        );

        let parent_only = layout(
            r#"
            id = "child"
            extends = "parent"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-2"
            module = "clock"
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "$index > 1"
            "#,
        );
        let report = validate(&parent_only, &Modules);
        assert!(report.is_clean(), "{}", report.render());
    }

    /// DEC-23: a copy is drawn as `<id>#<index>`, so a written id may not contain the mark, and a copy's id names the child it copies.
    #[test]
    fn a_copy_s_id_names_its_child_and_no_written_id_may_look_like_one() {
        let player = InstanceId::new("player");
        let copy = player.copy(2);
        assert_eq!(copy.as_str(), "player#2");
        assert_eq!(copy.template(), player);
        assert_eq!(copy.copy_index(), Some(2));
        assert_eq!(player.copy_index(), None);
        assert_eq!(player.template(), player);
        assert_eq!(InstanceId::new("a#b").template(), InstanceId::new("a#b"));

        let report = validate(&layout(&ONE_BAR.replace("clock-1", "clock#1")), &Modules);
        assert!(
            report
                .errors
                .iter()
                .any(|f| f.key.ends_with("children.clock#1")
                    && f.message.key() == Some("finding.copy_mark")),
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
                .any(|f| f.message.key() == Some("finding.shared_instance_id")),
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
            LayoutOp::SetGroupArrange {
                site: Site::everywhere(LayerKind::Top),
                area: AreaId::new("bar-top"),
                id: GroupId::new("start"),
                arrange: Some(Arrange::Pages),
                cols: None,
                rows: None,
                gap: Some(4.0),
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
                    fillet: Some(12.0),
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
                anchor: None,
            },
            AreaKind::Panel {
                owner: Some(InstanceId::new("clock")),
                along: Some(true),
                cols: Some(4),
                rows: Some(3),
                cell: Some(80.0),
                gap: Some(16.0),
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
                style: Style {
                    fill: Some("surface".into()),
                    radius: Some(Corners::each(12.0, 12.0, 0.0, 0.0)),
                    opacity: Some(0.9),
                    padding: Some(Sides::each(4.0, 8.0, 4.0, 8.0)),
                    border: Some(Border {
                        width: Some(1.0),
                        color: Some("highlight_low".into()),
                    }),
                    shadow: Some(2),
                    backdrop: Some(Backdrop::Blur),
                },
                visible: Some(Expr("true".into())),
                remove: vec![GroupId::new("gone")],
                actions: BTreeMap::from([(
                    Trigger::ScrollUp,
                    Action(vec!["panel toggle clock".into()]),
                )]),
                unset: vec![Unset::Visible],
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
                    arrange: Some(Arrange::Grid),
                    cols: Some(3),
                    rows: Some(2),
                    gap: Some(8.0),
                    repeat: Some(Expr("$battery.cells".into())),
                    style: Style {
                        fill: Some("overlay".into()),
                        padding: Some(Sides::each(8.0, 12.0, 8.0, 12.0)),
                        border: Some(Border {
                            width: Some(1.0),
                            color: None,
                        }),
                        shadow: Some(1),
                        ..Style::default()
                    },
                    children: vec![Instance {
                        id: InstanceId::new("clock-1"),
                        module: Some("clock".into()),
                        style: Style {
                            fill: Some("#1f1d2e".into()),
                            radius: Some(Corners::each(16.0, 16.0, 4.0, 4.0)),
                            shadow: Some(2),
                            ..Style::default()
                        },
                        bindings: BTreeMap::from([(
                            "show_date".to_string(),
                            Expr("$battery.percent > 50".into()),
                        )]),
                        actions: BTreeMap::from([(
                            Trigger::Press,
                            Action(vec!["panel toggle clock".into()]),
                        )]),
                        unset: vec![Unset::binding("accent")],
                        weight: Some(2.0),
                        cell: Some(ChildCell {
                            col: 1,
                            row: 0,
                            col_span: 2,
                            row_span: 1,
                        }),
                        rect: Some(Rect::default()),
                        ..Instance::default()
                    }],
                    unset: vec![Unset::Repeat],
                    ..Group::default()
                }],
                ..Area::default()
            };
            let written = written_layout(area);
            let report = validate::check_unknown_keys(&written, &LayoutId::new("t"));
            assert!(
                report.is_clean(),
                "a group writes keys validation does not know: {}",
                report.render()
            );
            toml::from_str::<Layout>(&written)
                .expect("an instance's own keys are refused by serde, and these all parse back");
        }
    }

    /// The same guard for a declared source: its `kind` decides which keys it has.
    #[test]
    fn every_field_a_source_kind_has_is_a_key_validation_knows() {
        let filled = [
            Source::Poll {
                cmd: Some("date".into()),
                every: Some("5s".into()),
                initial: Some(toml::Value::Integer(0)),
                parse: Some("text".into()),
                while_: Some(While::Always),
                lock_safe: Some(true),
            },
            Source::Listen {
                cmd: Some("date".into()),
                initial: Some(toml::Value::Integer(0)),
                parse: Some("text".into()),
                while_: Some(While::Always),
                lock_safe: Some(true),
            },
            Source::Http {
                url: Some("https://example.org".into()),
                every: Some("5m".into()),
                initial: Some(toml::Value::Integer(0)),
                parse: Some("json:.a".into()),
                while_: Some(While::Always),
                lock_safe: Some(true),
            },
        ];
        for source in filled {
            let kind = source.kind_name();
            let layout = Layout {
                id: LayoutId::new("t"),
                sources: BTreeMap::from([("probe".to_string(), source)]),
                ..Layout::default()
            };
            let text = toml::to_string_pretty(&layout).expect("the layout serializes");
            let report = validate::check_unknown_keys(&text, &LayoutId::new("t"));
            assert!(
                report.is_clean(),
                "a `{kind}` source writes keys validation does not know: {}",
                report.render()
            );
            assert_eq!(
                toml::from_str::<Layout>(&text).expect("it parses back"),
                layout
            );
        }
    }

    #[test]
    fn a_key_a_source_does_not_have_is_found_where_it_is_written() {
        let text = "id = \"t\"\n[sources.load]\nkind = \"poll\"\ncmd = \"date\"\nurl = \"https://example.org\"\n";
        let report = validate::check_unknown_keys(text, &LayoutId::new("t"));
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(report.errors[0].key, "sources.load.url");
        assert_eq!(
            report.errors[0].span.as_ref().map(|span| span.line),
            Some(5)
        );
    }

    #[test]
    fn sources_merge_by_name_along_the_extends_chain() {
        let base: Layout = toml::from_str(
            r#"
            id = "base"
            [sources.load]
            kind = "poll"
            cmd = "cat /proc/loadavg"
            every = "5s"
            [sources.feed]
            kind = "poll"
            cmd = "date"
            "#,
        )
        .expect("parses");
        let child: Layout = toml::from_str(
            r#"
            id = "mine"
            extends = "base"
            [sources.load]
            kind = "poll"
            every = "10s"
            [sources.feed]
            kind = "http"
            url = "https://example.org/feed"
            [sources.own]
            kind = "listen"
            cmd = "tail -f log"
            "#,
        )
        .expect("parses");
        let known = Library::of_layouts([base]);
        let (merged, report) = crate::sources(&child, &known);
        assert!(report.is_clean(), "{}", report.render());

        assert_eq!(merged["load"].cmd(), Some("cat /proc/loadavg"), "inherited");
        assert_eq!(merged["load"].every(), Some("10s"), "the child wins");
        assert_eq!(
            merged["feed"].url(),
            Some("https://example.org/feed"),
            "a level that changes the kind replaces the source outright"
        );
        assert_eq!(merged["feed"].cmd(), None);
        assert_eq!(merged["own"].kind_name(), "listen");
    }

    /// `lock_safe` vouches for one command. A layout that extends a lock-safe source and swaps only its command would otherwise put a command nobody vouched for on the lock screen.
    #[test]
    fn a_level_that_changes_what_a_source_runs_has_to_say_lock_safe_again() {
        let base: Layout = toml::from_str(
            r#"
            id = "base"
            [sources.load]
            kind = "poll"
            cmd = "cat /proc/loadavg"
            lock_safe = true
            [sources.feed]
            kind = "http"
            url = "https://example.org/a"
            lock_safe = true
            [sources.tail]
            kind = "listen"
            cmd = "tail -f a"
            lock_safe = true
            [sources.kept]
            kind = "poll"
            cmd = "date"
            lock_safe = true
            "#,
        )
        .expect("parses");
        let child: Layout = toml::from_str(
            r#"
            id = "mine"
            extends = "base"
            [sources.load]
            kind = "poll"
            cmd = "cat /etc/shadow"
            [sources.feed]
            kind = "http"
            url = "https://example.org/b"
            [sources.tail]
            kind = "listen"
            cmd = "tail -f b"
            lock_safe = true
            [sources.kept]
            kind = "poll"
            every = "1m"
            "#,
        )
        .expect("parses");
        let known = Library::of_layouts([base]);
        let (merged, report) = crate::sources(&child, &known);
        assert!(report.is_clean(), "{}", report.render());

        assert!(
            !merged["load"].lock_safe(),
            "a swapped command is not vouched for"
        );
        assert!(!merged["feed"].lock_safe(), "nor a swapped address");
        assert!(
            merged["tail"].lock_safe(),
            "the level that swaps it may vouch for it itself"
        );
        assert!(
            merged["kept"].lock_safe(),
            "a level that leaves the command alone keeps what was said about it"
        );
    }

    #[test]
    fn a_source_with_nothing_to_run_after_every_level_is_reported_and_left_out() {
        let lone: Layout =
            toml::from_str("id = \"mine\"\n[sources.load]\nkind = \"poll\"\nevery = \"10s\"\n")
                .expect("parses");
        let (merged, report) = crate::sources(&lone, &Library::default());
        assert!(merged.is_empty());
        assert_eq!(report.errors[0].key, "sources.load.cmd");
    }

    #[test]
    fn a_level_is_validated_by_what_it_calls_its_sources_and_nothing_it_says_twice() {
        let written: Layout = toml::from_str(
            r#"
            id = "t"
            [sources.battery]
            kind = "poll"
            cmd = "acpi"
            [sources.2fast]
            kind = "poll"
            cmd = "date"
            [sources.eager]
            kind = "poll"
            cmd = "date"
            every = "0s"
            [sources.fine]
            kind = "poll"
            cmd = "date"
            "#,
        )
        .expect("parses");
        let report = validate(&written, &Modules);
        let keys: Vec<&str> = report.errors.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["sources.2fast", "sources.battery"],
            "what a source says is checked once, where the merged chain is read to run: {}",
            report.render()
        );
    }

    #[test]
    fn a_pages_group_keeps_the_place_it_was_given() {
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
            arrange = "pages"
            [[outputs.layers.desktop.areas]]
            id = "dock"
            kind = "dock"
            edge = "bottom"
            thickness = 40
            [[outputs.layers.desktop.areas.groups]]
            id = "end"
            place = "zone"
            zone = "end"
            arrange = "pages"
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
            .map(|group| (group.id.to_string(), group.kind, group.is_pages()))
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
        let messages: Vec<String> = report.findings().map(|f| f.message.english()).collect();
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
        let (resolved, report) = resolve(&built_in, &Library::default(), "DP-1", None);
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

    /// F-10.2: the built-in layout is read-only, so a file named after it does not stand in for it — at load or at a reload — and says so, naming itself, while everything that names `default` still means the layout that ships.
    #[test]
    fn a_file_named_after_the_built_in_layout_is_reported_and_never_read() {
        let dir = scratch("shadow");
        std::fs::create_dir_all(&dir).unwrap();
        let mut shadow = built_in::layout();
        shadow.name = "shadow".to_string();
        shadow.outputs.clear();
        std::fs::write(
            dir.join("default.toml"),
            toml::to_string_pretty(&shadow).unwrap(),
        )
        .unwrap();

        let (mut store, report) = LayoutStore::load(&dir);
        let finding = report
            .warnings
            .iter()
            .find(|finding| finding.message.key() == Some("finding.built_in_file"))
            .unwrap_or_else(|| panic!("{}", report.render()));
        assert_eq!(finding.file, dir.join("default.toml"));
        assert_eq!(
            store.get(&LayoutId::new(BUILT_IN)),
            Some(&built_in::layout())
        );

        let report = store.reload();
        assert!(
            report
                .warnings
                .iter()
                .any(|finding| finding.message.key() == Some("finding.built_in_file")),
            "{}",
            report.render()
        );
        assert_eq!(
            store.get(&LayoutId::new(BUILT_IN)),
            Some(&built_in::layout())
        );
        assert_eq!(store.names().count(), 1);
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

    /// The history lists every entry an undo and a redo walk through, and three back then two forward lands where the second edit left the layout.
    #[test]
    fn the_history_lists_both_ways_and_a_jump_lands_on_the_right_layout() {
        let dir = scratch("history-list");
        let mut store = store_with(&dir, &layout(ONE_BAR));
        let mine = LayoutId::new("mine");
        let at = |store: &LayoutStore| toml::to_string(store.get(&mine).unwrap()).unwrap();
        let edits = [
            (
                "Add a battery",
                LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("battery-1", "battery")),
                },
            ),
            (
                "Add a mixer",
                LayoutOp::InsertInstance {
                    spot: top_zone(),
                    index: 1,
                    instance: Box::new(instance("mixer-1", "mixer")),
                },
            ),
            (
                "Remove the clock",
                LayoutOp::DeleteInstance {
                    spot: top_zone(),
                    id: InstanceId::new("clock-1"),
                },
            ),
        ];
        let start = at(&store);
        let mut after = Vec::new();
        for (label, op) in edits {
            store
                .commit(Transaction::new(label, mine.clone(), vec![op]))
                .expect("the edit commits");
            after.push(at(&store));
        }
        assert_eq!(
            store.history(),
            History {
                undo: vec![
                    "Add a battery".into(),
                    "Add a mixer".into(),
                    "Remove the clock".into()
                ],
                redo: vec![],
            }
        );

        for _ in 0..3 {
            store.undo().expect("undo");
        }
        assert_eq!(at(&store), start);
        assert_eq!(
            store.history().redo,
            ["Add a battery", "Add a mixer", "Remove the clock"],
            "the next redo first"
        );
        for _ in 0..2 {
            store.redo().expect("redo");
        }
        assert_eq!(at(&store), after[1], "where the second edit left it");
        assert_eq!(
            store.history(),
            History {
                undo: vec!["Add a battery".into(), "Add a mixer".into()],
                redo: vec!["Remove the clock".into()],
            }
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
                .any(|f| f.message.key() == Some("finding.gradient_stops")),
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
        let said: Vec<String> = report.findings().map(|f| f.message.english()).collect();
        let keys: Vec<(&str, &str)> = report
            .findings()
            .zip(&said)
            .map(|(f, said)| (f.key.as_str(), said.as_str()))
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
                        anchor: None,
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
                style: Box::new(Style {
                    radius: Some(Corners::each(0.0, 0.0, 12.0, 12.0)),
                    ..Style::default()
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
                style: Box::new(Style {
                    opacity: Some(0.5),
                    ..Style::default()
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
                        anchor: None,
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
                    anchor: None,
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
                style: Box::new(Style {
                    fill: Some("base".into()),
                    radius: Some(Corners::each(24.0, 24.0, 0.0, 0.0)),
                    ..Style::default()
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
                    .any(|f| f.key.ends_with("prompt.style.fill")
                        && f.message.key() == Some("finding.prompt_contrast")),
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
        prompt.style = Style {
            fill: Some("overlay".into()),
            radius: Some(Corners::each(20.0, 20.0, 4.0, 4.0)),
            opacity: Some(0.95),
            ..Style::default()
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

    /// The prompt is the last thing the lock layer stacks, whatever a monitor rule appended after it.
    #[test]
    fn a_monitor_rule_adding_a_lock_area_resolves_under_the_prompt() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "prompt"
            kind = "prompt"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.lock.areas]]
            id = "lock-banner"
            kind = "free"
            rect = { x = 0.0, y = 0.0, w = 1.0, h = 0.1 }
            "#,
        );
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(
            area_ids(&resolved, LayerKind::Lock)
                .last()
                .map(String::as_str),
            Some("prompt")
        );
        assert!(area_ids(&resolved, LayerKind::Lock).contains(&"lock-banner".to_string()));
        assert!(
            validate_resolved(&resolved, "layouts/test.toml", &theme()).is_clean(),
            "nothing is stacked over the prompt"
        );
    }

    /// A bar or a dock reserves unless it says otherwise; an area of any other kind does not.
    #[test]
    fn a_bar_and_a_dock_reserve_by_default_and_nothing_else_does() {
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
            [[outputs.layers.top.areas]]
            id = "dock-left"
            kind = "dock"
            edge = "left"
            thickness = 48
            [[outputs.layers.top.areas]]
            id = "bar-bottom"
            kind = "bar"
            edge = "bottom"
            thickness = 30
            reserve = false
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            "#,
        );
        let resolved = alone(&parsed, "DP-1");
        assert_eq!(resolved.reserved(config::Edge::Top), 32.0);
        assert_eq!(resolved.reserved(config::Edge::Left), 48.0);
        assert_eq!(resolved.reserved(config::Edge::Bottom), 0.0);
        let reserves = |layer, id: &str| {
            resolved
                .area(layer, &AreaId::new(id))
                .map(|area| area.reserve)
        };
        assert_eq!(reserves(LayerKind::Desktop, "widgets"), Some(false));
        assert_eq!(reserves(LayerKind::Top, "bar-bottom"), Some(false));
    }

    /// A hidden bar with no `reserve` still holds back only the strip it leaves behind.
    #[test]
    fn an_autohidden_bar_with_no_reserve_reserves_only_its_peek() {
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
            autohide = { peek = 2 }
            "#,
        );
        assert_eq!(alone(&parsed, "DP-1").reserved(config::Edge::Top), 2.0);
    }

    fn bar_with_shape(shape: &str) -> Layout {
        layout(&format!(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            edge = "top"
            thickness = 32
            shape = {shape}
            "#
        ))
    }

    /// A fillet is a bar's, and is reported where the bar is not one strip or where it is negative.
    #[test]
    fn a_bar_fillet_resolves_and_is_checked() {
        let drawn = bar_with_shape(r#"{ mode = "bar", fillet = 12 }"#);
        assert!(validate(&drawn, &Modules).is_clean());
        let fillet = |resolved: &Resolved| match resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))
            .map(|area| area.kind.clone())
        {
            Some(ResolvedAreaKind::Bar { shape, .. }) => shape.fillet,
            other => panic!("a bar: {other:?}"),
        };
        assert_eq!(fillet(&alone(&drawn, "DP-1")), Some(12.0));

        for mode in ["sections", "chips"] {
            let report = validate(
                &bar_with_shape(&format!(r#"{{ mode = "{mode}", fillet = 12 }}"#)),
                &Modules,
            );
            assert!(
                finding_at(&report, "areas.bar-top.fillet", "finding.fillet_mode"),
                "{mode}: {}",
                report.render()
            );
        }
        let report = validate(
            &bar_with_shape(r#"{ mode = "bar", fillet = -4 }"#),
            &Modules,
        );
        assert!(
            finding_at(&report, "areas.bar-top.fillet", "finding.fillet_negative"),
            "{}",
            report.render()
        );
    }

    /// A monitor rule that writes a fillet over a bar another rule set to `chips` is where the mismatch is reported.
    #[test]
    fn a_fillet_a_monitor_rule_writes_over_a_chips_bar_is_reported() {
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
            shape = { mode = "chips" }
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            shape = { fillet = 10 }
            "#,
        );
        let report = validate(&parsed, &Modules);
        assert!(
            finding_at(&report, "areas.bar-top.fillet", "finding.fillet_mode"),
            "{}",
            report.render()
        );
        let resolved = alone(&parsed, "DP-1");
        assert!(matches!(
            resolved.area(LayerKind::Top, &AreaId::new("bar-top")).map(|a| a.kind.clone()),
            Some(ResolvedAreaKind::Bar { shape, .. }) if shape.fillet == Some(10.0)
        ));
    }

    /// A panel's tracks are judged as a group's inner grid is — no columns or rows, a negative gap, a non-positive cell.
    #[test]
    fn a_panel_with_no_tracks_or_a_negative_gap_or_non_positive_cell_is_refused() {
        for (keys, key, message) in [
            ("cols = 0", "panel-clock.cols", "finding.no_tracks"),
            ("rows = 0", "panel-clock.rows", "finding.no_tracks"),
            ("gap = -2", "panel-clock.gap", "finding.gap_negative"),
            ("cell = -4", "panel-clock.cell", "finding.cell_not_positive"),
        ] {
            let parsed = with_owners(&panel(
                "panel-clock",
                &format!("owner = \"clock-1\"\n{keys}"),
            ));
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(&report, key, message),
                "{keys}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{keys}: {}", report.render());
        }
        let sound = with_owners(&panel(
            "panel-clock",
            "owner = \"clock-1\"\ncols = 2\nrows = 1\ngap = 0\ncell = 80",
        ));
        assert!(validate(&sound, &Modules).is_clean());
    }

    /// A grid area's cell must be positive and its gap must not be negative.
    #[test]
    fn a_grid_with_non_positive_cell_or_negative_gap_is_refused() {
        for (keys, key, message) in [
            ("cell = 0", "grid.cell", "finding.cell_not_positive"),
            ("gap = -1", "grid.gap", "finding.gap_negative"),
        ] {
            let parsed = layout(&format!(
                r#"{ONE_BAR}
                [[outputs.layers.desktop.areas]]
                id = "grid"
                kind = "grid"
                {keys}
                "#
            ));
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(&report, key, message),
                "{keys}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{keys}: {}", report.render());
        }
        let ok = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            cell = 80
            gap = 16
            "#
        ));
        assert!(validate(&ok, &Modules).is_clean());
    }

    fn thickness_of(layout: &Layout, known: &Library) -> Option<f32> {
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
        let known = Library::of_layouts([parent.clone(), mine.clone()]);
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
        let known = Library::of_layouts([built_in()]);
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
        let known = Library::of_layouts([built_in()]);
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
        let known = Library::of_layouts([built_in()]);
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
        let known = Library::of_layouts([built_in()]);
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

    /// The built-in layout's desktop has an empty grid of every cell the output fits, held 48 px off its edges, and the clock face where the old default `[widgets.clock]` put it: centred on the whole output at the medium size, in a free area of its own, one instance of the clock module like the bar's chip is another.
    #[test]
    fn the_built_in_desktop_shows_the_clock_where_the_old_default_put_it() {
        let resolved = alone(&built_in(), "DP-1");
        let desktop = resolved.layer(LayerKind::Desktop).expect("a desktop layer");
        let [widgets, centre] = desktop.areas.as_slice() else {
            panic!(
                "a grid and the clock's area on the desktop: {:?}",
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
        assert_eq!(widgets.style.padding, Some(Sides::all(48.0)));
        assert!(widgets.groups.is_empty(), "nothing on the grid yet");
        assert_eq!(
            centre.kind,
            ResolvedAreaKind::Free {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                anchor: Anchor::Center,
            }
        );
        assert_eq!(centre.within, Within::Output);
        let [clock] = centre.groups.as_slice() else {
            panic!("one group");
        };
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

    /// A bar whose visibility, whose start zone's copies and whose clock's accent and date are each driven by an expression, for every output.
    const EXPRESSIVE: &str = r##"
        id = "base"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar-top"
        kind = "bar"
        edge = "top"
        thickness = 32
        visible = "$battery.percent > 50"
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        repeat = "$battery.cells"
        [[outputs.layers.top.areas.groups.children]]
        id = "clock-1"
        module = "clock"
        bindings = { accent = "#ff0000", show_date = "$battery.percent > 20" }
    "##;

    /// A rule taking back all three kinds of expression the bar inherits, for the outputs `matches` names.
    fn taking_back(matches: &str) -> String {
        format!(
            r#"
            [[outputs]]
            match = "{matches}"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            unset = ["visible"]
            [[outputs.layers.top.areas.groups]]
            id = "start"
            unset = ["repeat"]
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            unset = ["bindings.accent"]
            "#
        )
    }

    /// The bar's `visible`, its start zone's `repeat` and the paths its clock binds, as `resolved` has them.
    fn expressions(resolved: &Resolved) -> (Option<Expr>, Option<Expr>, Vec<String>) {
        let bar = resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))
            .expect("the bar resolves");
        let start = &bar.groups[0];
        (
            bar.visible.clone().map(|visible| visible.expr),
            start.repeat.clone().map(|repeat| repeat.expr),
            start.children[0].bindings.keys().cloned().collect(),
        )
    }

    fn expressive_shown() -> (Option<Expr>, Option<Expr>, Vec<String>) {
        (
            Some(Expr("$battery.percent > 50".into())),
            Some(Expr("$battery.cells".into())),
            vec!["accent".to_string(), "show_date".to_string()],
        )
    }

    fn taken_back() -> (Option<Expr>, Option<Expr>, Vec<String>) {
        (None, None, vec!["show_date".to_string()])
    }

    #[test]
    fn unset_round_trips_through_toml_as_the_paths_it_takes_back() {
        let parsed = layout(&format!("{EXPRESSIVE}{}", taking_back("DP-1")));
        let layers = &parsed.outputs[1].layers.top;
        let bar = &layers.areas[0];
        assert_eq!(bar.unset, [Unset::Visible]);
        assert_eq!(bar.groups[0].unset, [Unset::Repeat]);
        assert_eq!(bar.groups[0].children[0].unset, [Unset::binding("accent")]);

        let text = toml::to_string_pretty(&parsed).expect("serializes");
        assert!(text.contains(r#"unset = ["bindings.accent"]"#), "{text}");
        assert_eq!(toml::from_str::<Layout>(&text).expect("re-parses"), parsed);

        for (written, read) in [
            ("visible", Unset::Visible),
            ("repeat", Unset::Repeat),
            ("bindings.style.color", Unset::binding("style.color")),
            ("bindings.", Unset::Unknown("bindings.".into())),
            ("accent", Unset::Unknown("accent".into())),
        ] {
            assert_eq!(Unset::from(written), read);
            assert_eq!(
                read.to_string(),
                written,
                "and it is written back as it was"
            );
        }
    }

    /// DEC-26 across output rules: a monitor's rule takes back each kind of expression the `*` rule gives, and every other monitor keeps them.
    #[test]
    fn an_output_rule_takes_back_what_a_broader_rule_drives_by_an_expression() {
        let parsed = layout(&format!("{EXPRESSIVE}{}", taking_back("DP-1")));
        assert_eq!(expressions(&alone(&parsed, "DP-1")), taken_back());
        assert_eq!(expressions(&alone(&parsed, "eDP-1")), expressive_shown());
        assert!(
            validate(&parsed, &Modules).is_clean(),
            "{}",
            validate(&parsed, &Modules).render()
        );
        assert!(validate_unsets(&parsed, &Library::default()).is_clean());
    }

    /// DEC-26 across `extends`: a layout takes back what the layout it extends drives, and a level above it can drive it again, since a level's own keys are laid over after what it takes back.
    #[test]
    fn a_layout_takes_back_what_the_layout_it_extends_drives_by_an_expression() {
        let base = layout(EXPRESSIVE);
        let known = Library::of_layouts([base.clone()]);
        let mut mine = layout(&format!(
            "id = \"mine\"\nextends = \"base\"\n{}",
            taking_back("*")
        ));
        let (resolved, report) = resolve(&mine, &known, "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(expressions(&resolved), taken_back());
        assert!(validate_unsets(&mine, &known).is_clean());

        let driven_again: Layout = layout(
            r#"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            visible = "$battery.percent > 80"
            "#,
        );
        mine.outputs.extend(driven_again.outputs);
        let (resolved, _) = resolve(&mine, &known, "DP-1", None);
        assert_eq!(
            expressions(&resolved).0,
            Some(Expr("$battery.percent > 80".into()))
        );
        assert_eq!(
            expressions(&resolve(&mine, &known, "eDP-1", None).0).0,
            None
        );
    }

    /// Flattening an `extends` chain into one level — what a reset puts back — keeps what a level of it took back, so the chain and its flattened form show the same.
    #[test]
    fn a_chain_flattened_into_one_level_keeps_what_a_level_of_it_took_back() {
        let base = layout(EXPRESSIVE);
        let middle = layout(&format!(
            "id = \"middle\"\nextends = \"base\"\n{}",
            taking_back("*")
        ));
        let known = Library::of_layouts([base.clone(), middle.clone()]);
        let mine = layout("id = \"mine\"\nextends = \"middle\"\n");
        let flat = crate::reset::base_of(&mine, &known);
        assert_eq!(
            expressions(&resolve(&mine, &known, "DP-1", None).0),
            taken_back()
        );
        assert_eq!(expressions(&alone(&flat, "DP-1")), taken_back());
    }

    /// A level that writes a key and takes it back too is an error naming the key, since which of the two it meant is anybody's guess.
    #[test]
    fn a_level_that_writes_and_takes_back_the_same_key_is_an_error_naming_it() {
        let parsed = layout(&format!(
            r##"{EXPRESSIVE}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            visible = "$battery.percent > 80"
            unset = ["visible"]
            [[outputs.layers.top.areas.groups]]
            id = "start"
            repeat = "$battery.cells"
            unset = ["repeat"]
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            bindings = {{ accent = "#00ff00" }}
            unset = ["bindings.show_date", "bindings.accent"]
            "##
        ));
        let report = validate(&parsed, &Modules);
        let at = "outputs.DP-1.layers.top.areas.bar-top";
        let said: Vec<String> = report.errors.iter().map(|f| f.message.english()).collect();
        let errors: Vec<(&str, &str)> = report
            .errors
            .iter()
            .zip(&said)
            .map(|(finding, said)| (finding.key.as_str(), said.as_str()))
            .collect();
        assert_eq!(errors.len(), 3, "{}", report.render());
        for (key, path) in [
            (format!("{at}.unset[0]"), "`visible`"),
            (format!("{at}.groups.start.unset[0]"), "`repeat`"),
            (
                format!("{at}.groups.start.children.clock-1.unset[1]"),
                "`bindings.accent`",
            ),
        ] {
            assert!(
                errors
                    .iter()
                    .any(|(found, message)| *found == key && message.contains(path)),
                "{key} says {path}: {}",
                report.render()
            );
        }
    }

    /// A path a holder has no expression at is an error where it is written; so is a binding the instance's module cannot have, the module read from the level that names it.
    #[test]
    fn a_path_that_names_nothing_to_take_back_is_an_error_where_it_is_written() {
        let text = format!(
            r#"{EXPRESSIVE}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            unset = ["visible", "repeat", "colour"]
            [[outputs.layers.top.areas.groups]]
            id = "start"
            unset = ["visible"]
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            unset = ["accent", "bindings.nope", "bindings.show_date"]
            "#
        );
        let parsed = layout(&text);
        let mut report = validate(&parsed, &Modules);
        let at = "outputs.DP-1.layers.top.areas.bar-top";
        let mut keys: Vec<&str> = report.errors.iter().map(|f| f.key.as_str()).collect();
        keys.sort_unstable();
        let mut wanted = vec![
            format!("{at}.unset[1]"),
            format!("{at}.unset[2]"),
            format!("{at}.groups.start.unset[0]"),
            format!("{at}.groups.start.children.clock-1.unset[0]"),
            format!("{at}.groups.start.children.clock-1.unset[1]"),
        ];
        wanted.sort_unstable();
        assert_eq!(keys, wanted, "{}", report.render());
        assert!(
            report
                .findings()
                .any(|finding| finding.message.key() == Some("finding.unset_no_binding")),
            "{}",
            report.render()
        );

        crate::locate_unsets(&text, &mut report);
        let colour = report
            .errors
            .iter()
            .find(|finding| finding.key == format!("{at}.unset[2]"))
            .and_then(|finding| finding.span.as_ref())
            .expect("the entry is found in the file");
        let line = text
            .lines()
            .position(|line| line.contains("\"colour\""))
            .unwrap()
            + 1;
        assert_eq!(colour.line, line);
        assert_eq!(&text[colour.bytes.clone()], "\"colour\"");
    }

    /// Taking back what nothing under the level writes changes nothing, so it is a warning — against the `extends` chain and the rules applied first, and never against a rule for another monitor.
    #[test]
    fn taking_back_what_nothing_under_the_level_writes_is_a_warning() {
        let plain = layout(&format!("{ONE_BAR}{}", taking_back("DP-1")));
        let report = validate_unsets(&plain, &Library::default());
        assert!(report.errors.is_empty(), "{}", report.render());
        let at = "outputs.DP-1.layers.top.areas.bar-top";
        let mut warned: Vec<&str> = report.warnings.iter().map(|f| f.key.as_str()).collect();
        warned.sort_unstable();
        assert_eq!(
            warned,
            [
                format!("{at}.groups.start.children.clock-1.unset[0]"),
                format!("{at}.groups.start.unset[0]"),
                format!("{at}.unset[0]"),
            ],
            "{}",
            report.render()
        );
        assert!(
            report.warnings[0].message.key() == Some("finding.unset_nothing"),
            "{}",
            report.render()
        );

        let elsewhere = layout(&format!(
            "{}{}",
            EXPRESSIVE.replace("match = \"*\"", "match = \"HDMI-1\""),
            taking_back("DP-1")
        ));
        assert_eq!(
            validate_unsets(&elsewhere, &Library::default())
                .warnings
                .len(),
            3,
            "a rule for another monitor is never under this one"
        );

        let base = layout(EXPRESSIVE);
        let known = Library::of_layouts([base]);
        let extending = layout(&format!(
            "id = \"mine\"\nextends = \"base\"\n{}",
            taking_back("DP-*")
        ));
        assert!(validate_unsets(&extending, &known).is_clean());
        assert_eq!(
            validate_unsets(&extending, &Library::default())
                .warnings
                .len(),
            3,
            "with the layout it extends missing, nothing is under it"
        );
    }

    /// A workspace rule takes back what its output's rules drive, which is under it.
    #[test]
    fn a_workspace_rule_takes_back_what_its_output_drives() {
        let parsed = layout(&format!(
            r#"{EXPRESSIVE}
            [[outputs.workspaces]]
            match = "2"
            [[outputs.workspaces.layers.top.areas]]
            id = "bar-top"
            unset = ["visible"]
            "#
        ));
        assert!(validate(&parsed, &Modules).is_clean());
        assert!(validate_unsets(&parsed, &Library::default()).is_clean());
        let on = |name: &str| {
            let workspace = ActiveWorkspace {
                name: name.into(),
                ..ActiveWorkspace::default()
            };
            expressions(&resolve(&parsed, &Library::default(), "DP-1", Some(&workspace)).0).0
        };
        assert_eq!(on("2"), None);
        assert_eq!(on("1"), Some(Expr("$battery.percent > 50".into())));
    }

    /// The lock's prompt can never be hidden, so it never has a `visible` to take back: one written takes nothing back, and is no reason to refuse the lock screen.
    #[test]
    fn taking_back_the_lock_prompts_visibility_is_harmless() {
        let parsed = layout(
            r#"
            id = "locked"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "prompt"
            kind = "prompt"
            unset = ["visible"]
            "#,
        );
        assert!(validate(&parsed, &Modules).errors.is_empty());
        assert!(crate::validate_lock(&parsed, &Modules).errors.is_empty());
        let resolved = alone(&parsed, "DP-1");
        assert!(validate_resolved(&resolved, "locked", &theme()).is_clean());
        assert_eq!(
            validate_unsets(&parsed, &Library::default()).warnings.len(),
            1,
            "nothing under it gives the prompt an expression"
        );
    }

    /// Merging is by id, so a monitor's rule naming an instance where the `*` rule placed it refines that instance; the same id placed anywhere else is a second instance, which IPC could not tell apart.
    #[test]
    fn a_rule_refines_an_instance_where_it_is_placed_and_nowhere_else() {
        let refined = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            options = {{ format = "%H" }}
            "#
        ));
        assert!(
            validate(&refined, &Modules).errors.is_empty(),
            "{}",
            validate(&refined, &Modules).render()
        );

        let moved = layout(&format!(
            r#"{ONE_BAR}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "end"
            place = "zone"
            zone = "end"
            [[outputs.layers.top.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            "#
        ));
        let report = validate(&moved, &Modules);
        assert!(
            report.errors.iter().any(|finding| finding.key
                == "outputs.DP-1.layers.top.areas.bar-top.groups.end.children.clock-1"),
            "{}",
            report.render()
        );
    }

    const PROVENANCE_BASE: &str = r##"
        id = "base"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar"
        kind = "bar"
        edge = "top"
        thickness = 32
        visible = "true"
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        repeat = "{1, 2}"
        [[outputs.layers.top.areas.groups.children]]
        id = "clock"
        module = "clock"
        bindings = { accent = "#ff0000", show_date = "true" }
    "##;

    const PROVENANCE_MINE: &str = r##"
        id = "mine"
        extends = "base"
        [[outputs]]
        match = "*"
        [[outputs]]
        match = "DP-1"
        [[outputs.layers.top.areas]]
        id = "bar"
        visible = "false"
        [[outputs.workspaces]]
        match = "2"
        [[outputs.workspaces.layers.top.areas]]
        id = "bar"
        [[outputs.workspaces.layers.top.areas.groups]]
        id = "start"
        [[outputs.workspaces.layers.top.areas.groups.children]]
        id = "clock"
        bindings = { accent = "#00ff00" }
        [[outputs]]
        match = "HDMI-*"
        [[outputs.layers.top.areas]]
        id = "bar"
        unset = ["visible"]
        [[outputs.layers.top.areas.groups]]
        id = "start"
        remove = ["clock"]
        [[outputs.layers.top.areas.groups.children]]
        id = "clock"
        module = "clock"
    "##;

    fn provenance() -> (Layout, Library) {
        let (base, mine) = (layout(PROVENANCE_BASE), layout(PROVENANCE_MINE));
        let known = Library::of_layouts([base, mine.clone()]);
        (mine, known)
    }

    fn origin(layout: &str, output: &str, workspace: Option<&str>) -> Origin {
        Origin::Level(Level {
            layout: LayoutId::new(layout),
            output: OutputMatch(output.to_string()),
            workspace: workspace.map(|workspace| WorkspaceMatch(workspace.to_string())),
        })
    }

    /// Resolution keeps, beside each expression, the level that wrote it: a layout this one extends, one of its output rules, or the workspace rule up on the screen; a level that takes an expression back or places an item afresh leaves nothing of the old one's.
    #[test]
    fn every_resolved_expression_names_the_level_that_wrote_it() {
        let (mine, known) = provenance();
        let on = |output: &str, workspace: &str| {
            let active = ActiveWorkspace {
                name: workspace.to_string(),
                ..ActiveWorkspace::default()
            };
            let resolved = resolve(&mine, &known, output, Some(&active)).0;
            resolved
                .area(LayerKind::Top, &AreaId::new("bar"))
                .expect("the bar")
                .clone()
        };
        let bar = on("DP-1", "1");
        let clock = &bar.groups[0].children[0];
        assert_eq!(
            bar.visible.as_ref().map(|visible| visible.origin.clone()),
            Some(origin("mine", "DP-1", None))
        );
        assert_eq!(
            bar.groups[0]
                .repeat
                .as_ref()
                .map(|repeat| repeat.origin.clone()),
            Some(origin("base", "*", None))
        );
        assert_eq!(clock.bindings["accent"].origin, origin("base", "*", None));
        assert_eq!(
            clock.bindings["show_date"].origin,
            origin("base", "*", None)
        );

        let bar = on("DP-1", "2");
        let clock = &bar.groups[0].children[0];
        assert_eq!(
            clock.bindings["accent"].origin,
            origin("mine", "DP-1", Some("2"))
        );
        assert_eq!(clock.bindings["accent"].expr, Expr("#00ff00".into()));
        assert_eq!(
            origin("mine", "DP-1", Some("2")).level().map(Level::rule),
            Some("outputs.DP-1.workspaces.2".to_string())
        );

        let bar = on("HDMI-A-1", "1");
        assert_eq!(bar.visible, None, "taken back");
        assert!(
            bar.groups[0].children[0].bindings.is_empty(),
            "a child placed afresh keeps nothing of the one it replaced"
        );
    }

    /// Taking an expression back is written in the narrowest rule of the edited layout for every screen it is for, which has to come after whatever writes it there: what the rule writes itself is deleted, and it names the expression in its `unset` while a level under it still gives one.
    #[test]
    fn taking_back_is_written_where_it_is_laid_over_every_writer() {
        let (mine, known) = provenance();
        let screens = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        let (bar, start) = (AreaId::new("bar"), GroupId::new("start"));
        let visible = Taken {
            layer: LayerKind::Top,
            area: &bar,
            held: Held::Visible,
        };
        let repeat = Taken {
            held: Held::Repeat(&start),
            ..visible
        };

        assert_eq!(
            crate::taking_back(&mine, &known, &screens(&["DP-1"]), visible),
            Ok(TakeBack {
                site: Site::new("DP-1", LayerKind::Top),
                own: true,
                unset: true,
            }),
            "its own expression deleted, and the inherited one under it taken back"
        );
        assert_eq!(
            crate::taking_back(&mine, &known, &screens(&["*"]), repeat),
            Ok(TakeBack {
                site: Site::everywhere(LayerKind::Top),
                own: false,
                unset: true,
            })
        );

        let refused = crate::taking_back(&mine, &known, &screens(&["DP-1", "eDP-1"]), visible)
            .expect_err("`*` is the rule for both, and `DP-1` comes after it")
            .english();
        assert!(
            refused.contains("`outputs.DP-1` of `layouts/mine.toml`"),
            "{refused}"
        );

        let mut ruled = mine.clone();
        ruled.outputs[1].workspaces[0].layers.top.areas[0].visible = Some(Expr("true".into()));
        let refused = crate::taking_back(&ruled, &known, &screens(&["DP-1"]), visible)
            .expect_err("a workspace rule comes after every output rule")
            .english();
        assert!(refused.contains("`outputs.DP-1.workspaces.2`"), "{refused}");

        let refused = crate::taking_back(&mine, &known, &screens(&["HDMI-A-1"]), visible)
            .expect_err("the HDMI rule takes it back already")
            .english();
        assert!(
            refused.contains("nothing gives `visible` of `bar`"),
            "{refused}"
        );
    }

    /// A binding is taken back the way `visible` and `repeat` are: in the narrowest rule laid over every level that gives it on the screens the edit is for, named in that rule's `unset` while a level under it still gives one.
    #[test]
    fn a_binding_is_taken_back_where_it_is_laid_over_every_writer() {
        let (mine, known) = provenance();
        let (bar, start, clock) = (
            AreaId::new("bar"),
            GroupId::new("start"),
            InstanceId::new("clock"),
        );
        let binding = |path| Taken {
            layer: LayerKind::Top,
            area: &bar,
            held: Held::Binding {
                group: &start,
                instance: &clock,
                path,
            },
        };
        let every = ["*".to_string()];

        assert_eq!(
            crate::taking_back(&mine, &known, &every, binding("show_date")),
            Ok(TakeBack {
                site: Site::everywhere(LayerKind::Top),
                own: false,
                unset: true,
            }),
            "the layout it extends gives it, so the broadest rule of this one takes it back"
        );

        let refused = crate::taking_back(&mine, &known, &["DP-1".to_string()], binding("accent"))
            .expect_err("a workspace rule of `DP-1` gives it, which comes after every output rule");
        assert_eq!(refused.key(), Some("finding.taken_back_elsewhere"));
        assert!(
            refused.english().contains("`outputs.DP-1.workspaces.2`"),
            "{}",
            refused.english()
        );

        let refused = crate::taking_back(
            &mine,
            &known,
            &["HDMI-A-1".to_string()],
            binding("show_date"),
        )
        .expect_err("the HDMI rule places the clock afresh, without it");
        assert_eq!(
            refused.english(),
            "nothing gives `bindings.show_date` of `clock` on `HDMI-A-1`, so there is nothing to take back"
        );
        assert_eq!(
            refused.render_in("es"),
            "nada da el `bindings.show_date` de `clock` en `HDMI-A-1`, así que no hay nada que retirar"
        );
    }

    #[test]
    fn every_finding_has_words_in_every_language_the_shell_speaks() {
        assert_eq!(
            util::report::untranslated(&crate::__rsx_i18n::CATALOG, &["finding."]),
            Vec::<String>::new()
        );
    }

    /// A finding is carried as what it says rather than as words, so the command line and a Spanish session read the same finding each in their own language.
    #[test]
    fn a_finding_reads_in_english_on_the_command_line_and_in_spanish_in_the_shell() {
        let parsed = layout(&format!(
            r#"{ONE_BAR}
            [outputs.layers.top.areas.groups.children.bindings]
            show_date = "$battery.percent"
            "#
        ));
        let report = validate(&parsed, &Modules);
        let finding = &report.errors[0];
        assert_eq!(finding.message.key(), Some("expression.expected_type"));
        assert_eq!(
            finding.message.english(),
            "expected bool, but this gives number"
        );
        assert_eq!(
            finding.message.render_in("es"),
            "se esperaba booleano, pero esto da número"
        );
    }

    const PILL: &str = r#"
        [parameters.label]
        type = "text"
        default = "'Battery'"
        [parameters.low]
        type = "number"
        default = "20"
        [[children]]
        id = "battery"
        module = "battery"
        [children.bindings]
        accent = "if($battery.percent < $low, #ff0000, #00ff00)"
        [[children]]
        id = "clock"
        module = "clock"
        [children.bindings]
        date_format = "$label"
    "#;

    const TWO_PILLS: &str = r#"
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
        komponent = "pill"
        [outputs.layers.top.areas.groups.parameters]
        label = "'Home'"
        [[outputs.layers.top.areas.groups]]
        id = "end"
        place = "zone"
        zone = "end"
        komponent = "pill"
    "#;

    fn pill() -> Komponent {
        toml::from_str(PILL).expect("the komponent parses")
    }

    fn with_pill(layouts: impl IntoIterator<Item = Layout>) -> Library {
        Library::of_layouts(layouts).with_komponent("pill", pill())
    }

    fn group_of<'a>(resolved: &'a Resolved, layer: LayerKind, group: &str) -> &'a ResolvedGroup {
        resolved
            .layer(layer)
            .into_iter()
            .flat_map(|layer| layer.areas.iter())
            .flat_map(|area| area.groups.iter())
            .find(|held| held.id.as_str() == group)
            .unwrap_or_else(|| panic!("the group {group} is drawn"))
    }

    fn child_ids(group: &ResolvedGroup) -> Vec<String> {
        group
            .children
            .iter()
            .map(|child| child.id.to_string())
            .collect()
    }

    /// A use's parameter reads what the use sets it to, with the level that set it; one the use leaves alone reads the komponent's default; and what the komponent holds is drawn under the use's own ids, its expressions written in the komponent's file and reading the use's parameters.
    #[test]
    fn a_parameter_reads_what_its_use_sets_it_to_else_its_default() {
        let parsed = layout(TWO_PILLS);
        let (resolved, report) = resolve(&parsed, &with_pill([]), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());

        let start = group_of(&resolved, LayerKind::Top, "start");
        let used = start
            .komponent
            .as_ref()
            .expect("the group draws a komponent");
        assert_eq!(used.id, KomponentId::new("pill"));
        let read: Vec<(&str, &str)> = used
            .parameters
            .iter()
            .map(|parameter| (parameter.name.as_str(), parameter.expr().0.as_str()))
            .collect();
        assert_eq!(read, [("label", "'Home'"), ("low", "20")]);
        let label = &used.parameters[0];
        assert_eq!(
            label.value.as_ref().map(|value| value.origin.clone()),
            Some(Origin::Level(Level {
                layout: LayoutId::new("test"),
                output: OutputMatch("*".into()),
                workspace: None,
            }))
        );
        assert_eq!(
            used.parameters[1].value, None,
            "`low` is left to its default"
        );

        assert_eq!(
            child_ids(start),
            ["bar-top.start/battery", "bar-top.start/clock"]
        );
        let clock = &start.children[1];
        let bound = &clock.bindings["date_format"];
        assert_eq!(bound.expr, Expr("$label".into()));
        assert_eq!(bound.origin, Origin::Komponent(KomponentId::new("pill")));
        assert_eq!(bound.origin.file(), "components/pill.toml");
        assert_eq!(
            bound.within.as_deref(),
            Some(&**used),
            "it reads this use's parameters"
        );
        assert_eq!(
            clock.id.komponent_child(),
            Some(InstanceId::new("clock")),
            "the id it has in its komponent"
        );
        let end = group_of(&resolved, LayerKind::Top, "end");
        assert_eq!(
            end.komponent
                .as_ref()
                .map(|used| used.parameters[0].expr().clone()),
            Some(Expr("'Battery'".into())),
            "the other use sets nothing, so it reads the default"
        );
    }

    /// Two uses of one komponent never share an id, so nothing kept by id — an instance's state, a stack's page — is shared between them, and both are placed for as long as the layout uses them.
    #[test]
    fn two_uses_of_one_komponent_draw_its_children_under_ids_of_their_own() {
        let parsed = layout(TWO_PILLS);
        let library = with_pill([]);
        let (resolved, _) = resolve(&parsed, &library, "DP-1", None);
        let ids = instance_ids(&resolved);
        assert_eq!(
            ids,
            [
                "bar-top.start/battery",
                "bar-top.start/clock",
                "bar-top.end/battery",
                "bar-top.end/clock"
            ]
        );
        let placed = Placed::of(&parsed, &library);
        for id in &ids {
            assert!(
                placed.instances.contains(&InstanceId::new(id)),
                "{id} is placed"
            );
        }
        let mut alone = parsed.clone();
        alone.outputs[0].layers.top.areas[0].groups.pop();
        let gone = placed.gone(&Placed::of(&alone, &library));
        assert_eq!(
            gone.instances,
            std::collections::BTreeSet::from([
                InstanceId::new("bar-top.end/battery"),
                InstanceId::new("bar-top.end/clock")
            ]),
            "dropping one use forgets its children and keeps the other's"
        );
    }

    /// Robustness: a komponent nobody can read is one placeholder, named by its file and reported where the group names it, and the area around it still draws.
    #[test]
    fn a_missing_komponent_is_a_placeholder_and_a_finding_not_a_missing_area() {
        let parsed = layout(TWO_PILLS);
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert_eq!(area_ids(&resolved, LayerKind::Top), ["bar-top"]);
        let start = group_of(&resolved, LayerKind::Top, "start");
        assert_eq!(child_ids(start), ["bar-top.start/pill"]);
        assert_eq!(start.children[0].module, "components/pill.toml");
        let keys: Vec<&str> = report
            .errors
            .iter()
            .map(|finding| finding.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "layers.top.areas.bar-top.groups.start.komponent",
                "layers.top.areas.bar-top.groups.end.komponent"
            ]
        );
        assert!(
            report.errors[0]
                .message
                .english()
                .contains("components/pill.toml"),
            "{}",
            report.render()
        );
    }

    /// A use that sets a parameter the komponent does not declare is told which it has, where it set it; one that sets one to the wrong type is told so by validation, at the same key, with the span inside the expression.
    #[test]
    fn a_parameter_a_use_cannot_set_is_reported_where_it_is_set() {
        let unknown = layout(&TWO_PILLS.replace("label = \"'Home'\"", "colour = \"'red'\""));
        let (_, report) = resolve(&unknown, &with_pill([]), "DP-1", None);
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(
            report.errors[0].key,
            "layers.top.areas.bar-top.groups.start.parameters.colour"
        );
        assert!(
            report.errors[0]
                .message
                .english()
                .contains("`label`, `low`")
        );

        let mistyped = layout(&TWO_PILLS.replace("label = \"'Home'\"", "low = \"'x'\""));
        let report = validate_komponents(&mistyped, &with_pill([]), &Modules);
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(
            report.errors[0].key,
            "outputs.*.layers.top.areas.bar-top.groups.start.parameters.low"
        );
        assert!(validate_komponents(&layout(TWO_PILLS), &with_pill([]), &Modules).is_clean());
    }

    /// A komponent's own expressions are checked with its parameters in scope — `$label` reads one, `$nope` reads nothing — and a parameter may not take a name a copy or a module already reads.
    #[test]
    fn a_komponent_is_checked_with_its_parameters_in_scope() {
        let id = KomponentId::new("pill");
        let report = validate_komponent(&id, &pill(), &Modules);
        assert!(report.is_clean(), "{}", report.render());

        let broken: Komponent =
            toml::from_str(&PILL.replace("date_format = \"$label\"", "date_format = \"$nope\""))
                .expect("it parses");
        let report = validate_komponent(&id, &broken, &Modules);
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(
            report.errors[0].file.display().to_string(),
            "components/pill.toml"
        );
        assert_eq!(report.errors[0].key, "children.clock.bindings.date_format");

        let named: Komponent = toml::from_str(
            &PILL
                .replace("[parameters.label]", "[parameters.item]")
                .replace("$label", "$item")
                .replace("[parameters.low]", "[parameters.battery]")
                .replace("$low", "$battery"),
        )
        .expect("it parses");
        let keys: Vec<String> = validate_komponent(&id, &named, &Modules)
            .errors
            .iter()
            .map(|finding| finding.key.clone())
            .collect();
        assert!(keys.contains(&"parameters.item".to_string()), "{keys:?}");
        assert!(keys.contains(&"parameters.battery".to_string()), "{keys:?}");
    }

    /// TA-8 reaches inside a komponent: one used on the lock layer may hold readings only and no actions, whatever its file allows elsewhere — the full check and the lock opener's both refuse it.
    #[test]
    fn a_komponent_on_the_lock_layer_may_not_hold_a_control() {
        let mut controls = pill();
        controls.children[1].module = Some("mixer".into());
        controls.children[0].actions =
            BTreeMap::from([(Trigger::Press, Action(vec!["panel toggle mixer".into()]))]);
        let library = Library::default().with_komponent("pill", controls);
        let locked = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "readings"
            kind = "free"
            rect = { x = 0.0, y = 0.0, w = 0.2, h = 0.2 }
            [[outputs.layers.lock.areas.groups]]
            id = "g"
            place = "zone"
            zone = "start"
            komponent = "pill"
            "#,
        );
        for report in [
            validate_komponents(&locked, &library, &Modules),
            validate_komponents_lock(&locked, &library, &Modules),
        ] {
            let said: Vec<String> = report
                .errors
                .iter()
                .map(|finding| finding.message.english())
                .collect();
            assert!(
                said.iter().any(|it| it.contains("`mixer`")),
                "the control is refused: {said:?}"
            );
            assert!(
                said.iter().any(|it| it.contains("has actions")),
                "and so is the action: {said:?}"
            );
            assert!(
                report.errors.iter().all(|finding| finding.key
                    == "outputs.*.layers.lock.areas.readings.groups.g.komponent")
            );
        }
        assert!(
            validate_komponents(&layout(TWO_PILLS), &library, &Modules).is_clean(),
            "the same komponent off the lock layer is fine"
        );
    }

    /// A level that names a komponent replaces what the group held under it, and the parameters a use sets are laid level over level by name — one taken back with `unset` reads its default again.
    #[test]
    fn a_komponent_replaces_what_a_group_inherits_and_its_parameters_merge_by_name() {
        let parent = layout(&ONE_BAR.replace("id = \"test\"", "id = \"parent\""));
        let child = layout(
            r#"
            id = "child"
            extends = "parent"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            komponent = "pill"
            [outputs.layers.top.areas.groups.parameters]
            label = "'Home'"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            unset = ["parameters.label"]
            [outputs.layers.top.areas.groups.parameters]
            low = "5"
            "#,
        );
        let library = with_pill([parent]);
        let reads = |output: &str| {
            let (resolved, report) = resolve(&child, &library, output, None);
            assert!(report.is_clean(), "{}", report.render());
            let start = group_of(&resolved, LayerKind::Top, "start").clone();
            let read: Vec<String> = start
                .komponent
                .as_ref()
                .expect("a use")
                .parameters
                .iter()
                .map(|parameter| parameter.expr().0.clone())
                .collect();
            (child_ids(&start), read)
        };
        let (ids, read) = reads("HDMI-A-1");
        assert_eq!(
            ids,
            ["bar-top.start/battery", "bar-top.start/clock"],
            "clock-1 is gone"
        );
        assert_eq!(read, ["'Home'", "20"]);
        let (_, read) = reads("DP-1");
        assert_eq!(read, ["'Battery'", "5"]);
        assert!(validate_unsets(&child, &library).is_clean());
    }

    /// Detach is the use drawn as plain instances: the komponent's children under ids the layout does not use yet, each parameter written into what reads it, what the use replaced kept away — one transaction, which one undo takes back.
    #[test]
    fn detaching_a_use_draws_what_it_drew_and_one_undo_takes_it_back() {
        let parent = layout(&ONE_BAR.replace("id = \"test\"", "id = \"parent\""));
        let mut mine = layout(
            r#"
            extends = "parent"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            komponent = "pill"
            [outputs.layers.top.areas.groups.parameters]
            label = "'Home'"
            "#,
        );
        mine.id = LayoutId::new("mine");
        let dir = scratch("detach").join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("parent.toml"),
            toml::to_string_pretty(&parent).unwrap(),
        )
        .unwrap();
        let components = components_beside(&dir);
        std::fs::create_dir_all(&components).unwrap();
        std::fs::write(components.join("pill.toml"), PILL).unwrap();
        let mut store = store_with(&dir, &mine);
        let before = store.get(&LayoutId::new("mine")).unwrap().clone();
        let (drawn, _) = resolve(&before, store.all(), "DP-1", None);
        let used = group_of(&drawn, LayerKind::Top, "start").clone();

        let ops = crate::components::detach(
            &before,
            store.all(),
            (
                &Site::everywhere(LayerKind::Top),
                &AreaId::new("bar-top"),
                &GroupId::new("start"),
            ),
            ("DP-1", None),
        )
        .expect("the group draws a komponent this layout names");
        store
            .commit(Transaction::new("Detach pill", LayoutId::new("mine"), ops))
            .expect("it commits");

        let after = store.get(&LayoutId::new("mine")).unwrap().clone();
        let (now, report) = resolve(&after, store.all(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        let start = group_of(&now, LayerKind::Top, "start");
        assert_eq!(start.komponent, None);
        assert_eq!(
            child_ids(start),
            ["battery", "clock"],
            "plain ids, and clock-1 stays away"
        );
        for (plain, drawn) in start.children.iter().zip(&used.children) {
            assert_eq!(plain.module, drawn.module);
            assert_eq!(plain.representation, drawn.representation);
            assert_eq!(plain.options, drawn.options);
            assert_eq!(plain.actions, drawn.actions);
        }
        assert_eq!(
            start.children[1].bindings["date_format"].expr,
            Expr("('Home')".into())
        );
        assert_eq!(
            start.children[0].bindings["accent"].expr,
            Expr("if($battery.percent < (20), #ff0000, #00ff00)".into())
        );
        assert!(
            validate(&after, &Modules).is_clean(),
            "{}",
            validate(&after, &Modules).render()
        );

        assert_eq!(
            store.undo().as_deref().ok(),
            Some("Detach pill"),
            "one entry takes it all back"
        );
        assert_eq!(store.get(&LayoutId::new("mine")), Some(&before));
    }

    /// Saving a group turns the values picked into parameters — an option read back through a binding of its key — and the group, written as a use of what it was saved as, draws the same modules with the same values.
    #[test]
    fn a_group_saved_as_a_komponent_draws_what_it_drew() {
        let bar = layout(&ONE_BAR.replace(
            "module = \"clock\"",
            "module = \"clock\"\n        options = { date_format = \"%d\", show_date = true }",
        ));
        let (drawn, _) = resolve(&bar, &Library::default(), "DP-1", None);
        let group = group_of(&drawn, LayerKind::Top, "start").clone();
        let offered = crate::components::candidates(&group, &|module, key| {
            Modules.binding_type(module, key).ok()
        });
        let names: Vec<&str> = offered.iter().map(|it| it.name.as_str()).collect();
        assert_eq!(names, ["date_format", "show_date"]);
        assert_eq!(offered[0].default, Expr("'%d'".into()));

        let saved = crate::components::saved(&group, &offered[..1]);
        assert!(validate_komponent(&KomponentId::new("clock-pill"), &saved, &Modules).is_clean());
        let text = toml::to_string_pretty(&saved).expect("it serializes");
        assert_eq!(
            toml::from_str::<Komponent>(&text).expect("it parses back"),
            saved
        );

        let mut used = bar.clone();
        let written = &mut used.outputs[0].layers.top.areas[0].groups[0];
        *written = crate::components::used(written, &KomponentId::new("clock-pill"));
        let library = Library::default().with_komponent("clock-pill", saved);
        let (now, report) = resolve(&used, &library, "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        let clock = &group_of(&now, LayerKind::Top, "start").children[0];
        assert_eq!(clock.id, InstanceId::new("bar-top.start/clock-1"));
        assert_eq!(clock.module, "clock");
        assert_eq!(
            clock.options.get("show_date"),
            Some(&toml::Value::Boolean(true))
        );
        assert_eq!(
            clock.options.get("date_format"),
            None,
            "the option became a parameter"
        );
        assert_eq!(
            clock.bindings["date_format"].expr,
            Expr("$date_format".into())
        );
        assert_eq!(
            crate::komponents_of(&used, &library),
            std::collections::BTreeSet::from([KomponentId::new("clock-pill")])
        );
    }

    /// F-10.4's guard for komponents: a group using one writes keys validation knows, and a komponent file, every field filled, parses back as itself while a key no komponent has is refused.
    #[test]
    fn every_key_a_komponent_and_its_use_write_is_one_validation_knows() {
        let area = Area {
            id: AreaId::new("a"),
            kind: Some(AreaKind::Bar {
                edge: Some(config::Edge::Top),
                thickness: Some(32.0),
                length: None,
                offset: None,
                shape: BarShape::default(),
                autohide: None,
            }),
            groups: vec![Group {
                id: GroupId::new("g"),
                kind: Some(GroupKind::Zone { zone: Zone::Start }),
                komponent: Some(KomponentId::new("pill")),
                parameters: BTreeMap::from([("label".to_string(), Expr("'Home'".into()))]),
                unset: vec![Unset::parameter("low")],
                ..Group::default()
            }],
            ..Area::default()
        };
        let written = written_layout(area);
        let report = validate::check_unknown_keys(&written, &LayoutId::new("t"));
        assert!(report.is_clean(), "{}", report.render());
        toml::from_str::<Layout>(&written).expect("it parses back");

        let mut filled = pill();
        filled.arrange = Some(Arrange::Pages);
        filled.gap = Some(4.0);
        filled.children[0].weight = Some(2.0);
        filled.repeat = Some(Expr("$battery.cells".into()));
        filled.children[0].representation = Some(Representation::Chip);
        filled.children[0].options =
            toml::Table::from_iter([("format".to_string(), toml::Value::String("%H".into()))]);
        filled.children[0].actions =
            BTreeMap::from([(Trigger::Press, Action(vec!["panel toggle clock".into()]))]);
        filled.children[0].unset = vec![Unset::binding("accent")];
        let text = toml::to_string_pretty(&filled).expect("it serializes");
        assert_eq!(
            toml::from_str::<Komponent>(&text).expect("it parses back"),
            filled
        );
        assert!(toml::from_str::<Komponent>(&format!("colour = \"red\"\n{PILL}")).is_err());
        assert!(
            toml::from_str::<Komponent>(&PILL.replace("type = \"number\"", "type = \"amount\""))
                .is_err(),
            "a type the language has no name for"
        );
    }

    /// The store reads komponents from `components/` beside the layouts, takes a rewritten one from its file and keeps one whose file holds what it last read or wrote, writes a saved one on the next flush, and refuses a second of the same name.
    #[test]
    fn the_store_reads_reloads_and_writes_komponents_by_content() {
        let root = scratch("komponent-store");
        let dir = root.join("layouts");
        let components = root.join("components");
        std::fs::create_dir_all(&components).unwrap();
        std::fs::write(components.join("pill.toml"), PILL).unwrap();
        let mut store = store_with(&dir, &layout(TWO_PILLS));
        assert_eq!(store.komponent(&KomponentId::new("pill")), Some(&pill()));

        std::fs::write(
            components.join("pill.toml"),
            PILL.replace("'Battery'", "'Power'"),
        )
        .unwrap();
        assert!(store.reload().is_clean());
        assert_eq!(
            store
                .komponent(&KomponentId::new("pill"))
                .map(|it| it.parameters["label"].default.clone()),
            Some(Expr("'Power'".into()))
        );

        let saved = Komponent::default();
        store
            .add_komponent(KomponentId::new("empty"), saved.clone())
            .expect("a new name");
        assert!(matches!(
            store.add_komponent(KomponentId::new("pill"), saved.clone()),
            Err(StoreError::KomponentExists(_))
        ));
        assert!(store.has_unsaved());
        assert!(store.flush().is_clean());
        util::writer::flush();
        assert!(components.join("empty.toml").exists(), "the flush wrote it");
        assert!(store.reload().is_clean(), "its own write is no edit");
        assert_eq!(store.komponent(&KomponentId::new("empty")), Some(&saved));

        store
            .add_komponent(KomponentId::new("later"), saved)
            .expect("a new name");
        assert!(store.discard_komponent(&KomponentId::new("later")));
        assert!(
            !store.discard_komponent(&KomponentId::new("empty")),
            "a written one stays"
        );
        std::fs::remove_file(components.join("pill.toml")).unwrap();
        assert!(store.reload().is_clean());
        assert_eq!(
            store.komponent(&KomponentId::new("pill")),
            None,
            "its file is gone"
        );
    }

    const STYLED: &str = r##"
        id = "test"
        [[outputs]]
        match = "*"
        [[outputs.layers.desktop.areas]]
        id = "widgets"
        kind = "grid"
        style = { fill = "base", padding = 24 }
        [[outputs.layers.desktop.areas.groups]]
        id = "container"
        place = "cell"
        col = 0
        row = 0
        col_span = 6
        row_span = 4
        style = { fill = "overlay", opacity = 0.6, radius = 16, padding = [8, 12, 8, 12], border = { width = 1, color = "highlight_low" }, shadow = 1 }
        [[outputs.layers.desktop.areas.groups.children]]
        id = "clock-1"
        module = "clock"
        representation = "widget_m"
        style = { fill = "#1f1d2e", radius = [16, 16, 4, 4], shadow = 2 }
    "##;

    fn style_of_group<'a>(resolved: &'a Resolved, group: &str) -> &'a Style {
        &group_of(resolved, LayerKind::Desktop, group).style
    }

    fn style_of_instance<'a>(resolved: &'a Resolved, id: &str) -> &'a Style {
        &resolved
            .instances()
            .find(|instance| instance.id.as_str() == id)
            .unwrap_or_else(|| panic!("`{id}` resolves"))
            .style
    }

    /// One style is written on an area, a group and an instance alike, and each comes back from its file as it was written: padding as one number or four sides, a border with its two keys, a shadow step.
    #[test]
    fn a_style_on_an_area_a_group_and_an_instance_round_trips() {
        let parsed = layout(STYLED);
        let group = &parsed.outputs[0].layers.desktop.areas[0].groups[0];
        assert_eq!(group.style.padding, Some(Sides::each(8.0, 12.0, 8.0, 12.0)));
        assert_eq!(
            group.style.border,
            Some(Border {
                width: Some(1.0),
                color: Some("highlight_low".into())
            })
        );
        assert_eq!(group.style.shadow, Some(1));
        assert_eq!(
            group.children[0].style.radius,
            Some(Corners::each(16.0, 16.0, 4.0, 4.0))
        );
        assert_eq!(
            parsed.outputs[0].layers.desktop.areas[0].style.padding,
            Some(Sides::all(24.0))
        );

        let text = toml::to_string_pretty(&parsed).expect("it serializes");
        assert_eq!(layout(&text), parsed);
        assert!(
            text.contains("padding = 24"),
            "padding the same on every side is written as one number:\n{text}"
        );
        assert!(validate::check_unknown_keys(&text, &LayoutId::new("test")).is_clean());

        let resolved = alone(&parsed, "DP-1");
        assert_eq!(style_of_group(&resolved, "container"), &group.style);
        assert_eq!(style_of_instance(&resolved, "clock-1").shadow, Some(2));
        assert!(
            validate(&parsed, &Modules).is_clean(),
            "{}",
            validate(&parsed, &Modules).render()
        );
    }

    #[test]
    fn sides_are_one_number_or_four() {
        #[derive(Debug, serde::Deserialize, serde::Serialize)]
        struct Held {
            padding: Sides,
        }
        let read = |text: &str| toml::from_str::<Held>(text).map(|held| held.padding);
        assert_eq!(read("padding = 6").unwrap(), Sides::all(6.0));
        assert_eq!(read("padding = 1.5").unwrap(), Sides::all(1.5));
        let each = read("padding = [1, 2, 3, 4]").unwrap();
        assert_eq!(
            (each.top(), each.right(), each.bottom(), each.left()),
            (1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!((each.horizontal(), each.vertical()), (6.0, 4.0));
        assert!(read("padding = [1, 2, 3]").is_err(), "three sides");
        assert!(read("padding = [1, 2, 3, 4, 5]").is_err(), "five sides");
        let written = toml::to_string(&Held { padding: each }).unwrap();
        assert_eq!(written.trim(), "padding = [1.0, 2.0, 3.0, 4.0]");
    }

    /// A monitor rule restyles an inherited group and instance key by key: the border's colour without its width, the shadow without the fill.
    #[test]
    fn a_monitor_rule_merges_a_style_key_by_key() {
        let parsed = layout(&format!(
            r#"{STYLED}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "container"
            style = {{ border = {{ color = "red" }}, padding = 4 }}
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            style = {{ shadow = 0, opacity = 0.8 }}
            "#
        ));
        let refined = alone(&parsed, "DP-1");
        let group = style_of_group(&refined, "container");
        assert_eq!(
            group.border,
            Some(Border {
                width: Some(1.0),
                color: Some("red".into())
            }),
            "the width is inherited, the colour is the rule's"
        );
        assert_eq!(group.padding, Some(Sides::all(4.0)));
        assert_eq!(group.fill.as_deref(), Some("overlay"));
        assert_eq!(group.shadow, Some(1));
        let clock = style_of_instance(&refined, "clock-1");
        assert_eq!(
            (clock.shadow, clock.opacity, clock.fill.as_deref()),
            (Some(0), Some(0.8), Some("#1f1d2e"))
        );

        let other = alone(&parsed, "eDP-1");
        assert_eq!(style_of_instance(&other, "clock-1").shadow, Some(2));
        assert_eq!(
            style_of_group(&other, "container").border,
            Some(Border {
                width: Some(1.0),
                color: Some("highlight_low".into())
            })
        );
    }

    /// The style is the group's own, not what it holds: a level that names another komponent replaces the children and keeps the paint around them.
    #[test]
    fn a_group_keeps_its_style_when_a_level_names_a_komponent() {
        let parsed = layout(&format!(
            r#"{STYLED}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "container"
            komponent = "pill"
            "#
        ));
        let library = Library::default().with_komponent("pill", Komponent::default());
        let (refined, _) = resolve(&parsed, &library, "DP-1", None);
        assert_eq!(style_of_group(&refined, "container").shadow, Some(1));
    }

    /// Changing one key of an inherited instance on one monitor writes that key and its id, nothing the instance inherits.
    #[test]
    fn a_monitor_rule_changing_only_an_inherited_shadow_writes_one_key() {
        let mut parsed = layout(STYLED);
        parsed.outputs.push(OutputRule {
            matches: OutputMatch("DP-1".into()),
            layers: Layers {
                desktop: Layer {
                    areas: vec![Area {
                        id: AreaId::new("widgets"),
                        groups: vec![Group {
                            id: GroupId::new("container"),
                            children: vec![Instance {
                                id: InstanceId::new("clock-1"),
                                style: Style {
                                    shadow: Some(3),
                                    ..Style::default()
                                },
                                ..Instance::default()
                            }],
                            ..Group::default()
                        }],
                        ..Area::default()
                    }],
                    ..Layer::default()
                },
                ..Layers::default()
            },
            ..OutputRule::default()
        });
        let text = toml::to_string(&parsed).expect("it serializes");
        let written: toml::Table = toml::from_str(&text).unwrap();
        let rule = &written["outputs"].as_array().unwrap()[1];
        let child = rule["layers"]["desktop"]["areas"][0]["groups"][0]["children"][0]
            .as_table()
            .unwrap();
        assert_eq!(
            child.keys().collect::<Vec<_>>(),
            ["id", "style"],
            "only the id it is addressed by and what changed: {child:?}"
        );
        assert_eq!(
            child["style"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["shadow"]
        );

        let refined = alone(&layout(&text), "DP-1");
        let clock = style_of_instance(&refined, "clock-1");
        assert_eq!(clock.shadow, Some(3));
        assert_eq!(
            clock.fill.as_deref(),
            Some("#1f1d2e"),
            "the rest is inherited"
        );
        assert_eq!(
            refined
                .instances()
                .find(|it| it.id.as_str() == "clock-1")
                .unwrap()
                .representation,
            Representation::WidgetM
        );
    }

    #[test]
    fn setting_a_group_s_style_is_undone_exactly() {
        let start = layout(STYLED);
        undoes_exactly(
            &start,
            &LayoutOp::SetGroupStyle {
                site: Site::everywhere(LayerKind::Desktop),
                area: AreaId::new("widgets"),
                id: GroupId::new("container"),
                style: Box::new(Style {
                    shadow: Some(3),
                    padding: Some(Sides::each(0.0, 4.0, 0.0, 4.0)),
                    ..Style::default()
                }),
            },
        );
    }

    fn finding_at(report: &util::report::Report, key: &str, message: &str) -> bool {
        report
            .findings()
            .any(|f| f.key.ends_with(key) && f.message.key() == Some(message))
    }

    /// A style key its holder cannot draw is reported where it is written, and so is a value outside what the key takes.
    #[test]
    fn a_style_key_its_holder_cannot_draw_is_refused() {
        let parsed = layout(
            r##"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            style = { backdrop = "blur", padding = [0, -2, 0, 0] }
            [[outputs.layers.desktop.areas.groups]]
            id = "container"
            place = "cell"
            col = 0
            row = 0
            style = { backdrop = "blur", shadow = 4 }
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            style = { padding = 4, border = { width = -1 } }
            "##,
        );
        let report = validate(&parsed, &Modules);
        let group = "areas.widgets.groups.container";
        let child = "groups.container.children.clock-1";
        assert!(finding_at(
            &report,
            &format!("{group}.style.backdrop"),
            "finding.style_backdrop"
        ));
        assert!(finding_at(
            &report,
            &format!("{group}.style.shadow"),
            "finding.style_shadow"
        ));
        assert!(finding_at(
            &report,
            &format!("{child}.style.padding"),
            "finding.style_padding"
        ));
        assert!(finding_at(
            &report,
            &format!("{child}.style.border.width"),
            "finding.style_negative"
        ));
        assert!(finding_at(
            &report,
            "areas.widgets.style.padding",
            "finding.style_negative"
        ));
        assert!(
            !report
                .findings()
                .any(|f| f.key.ends_with("areas.widgets.style.backdrop")),
            "an area has a backdrop: {}",
            report.render()
        );
        assert_eq!(report.findings().count(), 5, "{}", report.render());
    }

    /// A written opacity runs from 0 to 1 on an area, a group and an instance alike; one outside it, or no number at all, is refused where it is written.
    #[test]
    fn a_written_opacity_runs_from_zero_to_one() {
        let styled = |area: &str, group: &str, instance: &str| {
            layout(&format!(
                r#"
                id = "test"
                [[outputs]]
                match = "*"
                [[outputs.layers.desktop.areas]]
                id = "widgets"
                kind = "grid"
                style = {{ opacity = {area} }}
                [[outputs.layers.desktop.areas.groups]]
                id = "box"
                place = "cell"
                col = 0
                row = 0
                style = {{ opacity = {group} }}
                [[outputs.layers.desktop.areas.groups.children]]
                id = "clock-1"
                module = "clock"
                style = {{ opacity = {instance} }}
                "#
            ))
        };
        let report = validate(&styled("1.5", "-0.1", "nan"), &Modules);
        for key in [
            "areas.widgets.style.opacity",
            "groups.box.style.opacity",
            "children.clock-1.style.opacity",
        ] {
            assert!(
                finding_at(&report, key, "finding.style_opacity"),
                "{key}: {}",
                report.render()
            );
        }
        assert_eq!(report.errors.len(), 3, "{}", report.render());
        let report = validate(&styled("0", "0.5", "1"), &Modules);
        assert!(report.is_clean(), "{}", report.render());
    }

    /// A bar in `sections` or `chips` mode paints no strip, so a border or a shadow written on it is a warning, wherever the layout writes the mode from.
    #[test]
    fn a_border_on_a_bar_in_sections_or_chips_mode_is_a_warning() {
        let bar = |keys: &str, more: &str| {
            layout(&format!(
                r#"
                id = "test"
                [[outputs]]
                match = "*"
                [[outputs.layers.top.areas]]
                id = "bar-top"
                kind = "bar"
                edge = "top"
                thickness = 32
                {keys}
                {more}
                "#
            ))
        };
        let report = validate(
            &bar(
                "shape = { mode = \"chips\" }\nstyle = { border = { width = 1 }, shadow = 2 }",
                "",
            ),
            &Modules,
        );
        assert!(
            report.errors.is_empty(),
            "a warning, not an error: {}",
            report.render()
        );
        assert!(finding_at(
            &report,
            "bar-top.style.border",
            "finding.stripless_edge"
        ));
        assert!(finding_at(
            &report,
            "bar-top.style.shadow",
            "finding.stripless_edge"
        ));

        let sections = validate(
            &bar(
                "shape = { mode = \"sections\" }\nstyle = { border = { width = 1 }, shadow = 2 }",
                "",
            ),
            &Modules,
        );
        assert!(finding_at(
            &sections,
            "bar-top.style.border",
            "finding.stripless_edge"
        ));
        assert!(finding_at(
            &sections,
            "bar-top.style.shadow",
            "finding.stripless_edge"
        ));

        let later = bar(
            "style = { shadow = 2 }",
            "[[outputs]]\nmatch = \"DP-1\"\n[[outputs.layers.top.areas]]\nid = \"bar-top\"\nkind = \"bar\"\nshape = { mode = \"chips\" }",
        );
        let report = validate(&later, &Modules);
        assert!(
            report
                .findings()
                .any(|f| f.key.contains("outputs.DP-1") && f.key.ends_with("bar-top.style.shadow")),
            "the rule that makes it chips is told: {}",
            report.render()
        );

        let painted = bar("style = { border = { width = 1 }, shadow = 2 }", "");
        assert!(validate(&painted, &Modules).is_clean());
    }

    /// Whatever is drawn inside the lock's prompt is held to the prompt's own rules: never fainter than it may be, never a card its text cannot be read on, and never painted by an expression that could answer either (TA-8).
    #[test]
    fn what_the_prompt_holds_cannot_fade_or_hide_it() {
        let held = |style: &str, bindings: &str| {
            alone(
                &layout(&format!(
                    r#"
                    id = "test"
                    [[outputs]]
                    match = "*"
                    [[outputs.layers.lock.areas]]
                    id = "prompt"
                    kind = "prompt"
                    [[outputs.layers.lock.areas.groups]]
                    id = "card"
                    place = "cell"
                    col = 0
                    row = 0
                    style = {{ {style} }}
                    [[outputs.layers.lock.areas.groups.children]]
                    id = "clock-1"
                    module = "clock"
                    style = {{ {style} }}
                    bindings = {{ {bindings} }}
                    "#
                )),
                "DP-1",
            )
        };
        let report =
            |resolved: &Resolved| validate_resolved(resolved, "layouts/test.toml", &theme());

        let faint = report(&held("opacity = 0.5", ""));
        assert!(finding_at(
            &faint,
            "groups.card.style.opacity",
            "finding.prompt_faint"
        ));
        assert!(finding_at(
            &faint,
            "children.clock-1.style.opacity",
            "finding.prompt_faint"
        ));

        let unreadable = report(&held("fill = \"text\"", ""));
        assert!(finding_at(
            &unreadable,
            "groups.card.style.fill",
            "finding.prompt_contrast"
        ));

        let bound = report(&held("shadow = 1", "\"style.opacity\" = \"0.1\""));
        assert!(finding_at(
            &bound,
            "children.clock-1.bindings.style.opacity",
            "finding.prompt_style_bound"
        ));

        assert!(report(&held("shadow = 1, border = { width = 2 }", "")).is_clean());
    }

    /// A style colour a save makes a parameter is read back through a binding of the instance's style, so the komponent draws what the group drew: a hex as itself, a theme token as the reading of that token.
    #[test]
    fn a_saved_komponent_offers_an_instance_s_style_as_parameters() {
        let parsed = layout(STYLED);
        let (drawn, _) = resolve(&parsed, &Library::default(), "DP-1", None);
        let mut group = group_of(&drawn, LayerKind::Desktop, "container").clone();
        group.children[0].style.border = Some(Border {
            width: None,
            color: Some("teal".into()),
        });
        let offered = crate::components::candidates(&group, &|module, key| {
            Modules.binding_type(module, key).ok()
        });
        let defaults: BTreeMap<&str, &str> = offered
            .iter()
            .map(|it| (it.key.as_str(), it.default.0.as_str()))
            .collect();
        assert_eq!(defaults.get("style.fill"), Some(&"#1f1d2e"));
        assert_eq!(defaults.get("style.border.color"), Some(&"$theme.teal"));

        let fill: Vec<_> = offered
            .iter()
            .filter(|it| it.key == "style.fill")
            .cloned()
            .collect();
        let saved = crate::components::saved(&group, &fill);
        let child = &saved.children[0];
        assert_eq!(child.style.fill, None, "the parameter paints it now");
        assert_eq!(child.style.shadow, Some(2), "what was not chosen stays");
        assert_eq!(
            child.bindings["style.fill"],
            Expr(format!("${}", fill[0].name))
        );
        assert!(validate_komponent(&KomponentId::new("pill"), &saved, &Modules).is_clean());
    }

    /// A desktop grid holding one cell group `box`, which `group` adds keys to, and its children `children`, written as TOML tables.
    fn container(group: &str, children: &str) -> Layout {
        layout(&format!(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            col_span = 4
            row_span = 4
            {group}
            {children}
            "#
        ))
    }

    fn child(id: &str, keys: &str) -> String {
        format!(
            r#"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "{id}"
            module = "clock"
            {keys}
            "#
        )
    }

    fn placements_of(group: &ResolvedGroup) -> Vec<Option<Placement>> {
        group.children.iter().map(|child| child.placement).collect()
    }

    /// A group's `arrange`, inner grid and gap merge by id like any key, and so does where a child sits, so a monitor rule can turn a row into a column and give one child a larger share.
    #[test]
    fn a_monitor_rule_rearranges_a_container_by_id() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            arrange = "row"
            gap = 8
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            weight = 2
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-2"
            module = "clock"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            arrange = "column"
            gap = 4
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-2"
            weight = 3
            "#,
        );
        assert!(validate(&parsed, &Modules).is_clean());

        let everywhere = alone(&parsed, "HDMI-A-1");
        let row = group_of(&everywhere, LayerKind::Desktop, "box");
        assert_eq!(
            (row.arrange, row.cols, row.rows, row.gap),
            (Some(Arrange::Row), 2, 2, Some(8.0))
        );
        assert_eq!(
            placements_of(row),
            [Some(Placement::Weight(2.0)), Some(Placement::Weight(1.0))]
        );

        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        let column = group_of(&resolved, LayerKind::Desktop, "box");
        assert_eq!(
            (column.arrange, column.gap),
            (Some(Arrange::Column), Some(4.0))
        );
        assert_eq!(
            placements_of(column),
            [Some(Placement::Weight(2.0)), Some(Placement::Weight(3.0))],
            "what the monitor rule leaves alone it inherits"
        );
    }

    /// A grid of one cell group `box` arranged as a row with two children, written for every output, and `rules` after it.
    fn arranged_row(rules: &str) -> String {
        format!(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            arrange = "row"
            gap = 8
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-2"
            module = "clock"
            {rules}
            "#
        )
    }

    fn loose(unset_rule: &str) -> String {
        format!(
            r#"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            {unset_rule}
            "#
        )
    }

    /// A narrower level takes back the arrangement a broader one wrote with `unset = ["arrange"]`: the group is a loose run again there, its `cols`, `rows` and `gap` gone with it, and every other output keeps the row.
    #[test]
    fn an_output_rule_takes_back_a_container_arrangement_with_its_tracks() {
        let parsed = layout(&arranged_row(&loose(r#"unset = ["arrange"]"#)));
        assert_eq!(
            parsed.outputs[1].layers.desktop.areas[0].groups[0].unset,
            [Unset::Arrange]
        );
        let text = toml::to_string_pretty(&parsed).expect("serializes");
        assert!(text.contains(r#"unset = ["arrange"]"#), "{text}");
        assert_eq!(toml::from_str::<Layout>(&text).expect("re-parses"), parsed);
        assert_eq!(Unset::from("arrange"), Unset::Arrange);

        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        let group = group_of(&resolved, LayerKind::Desktop, "box");
        assert_eq!(
            (group.arrange, group.cols, group.rows, group.gap),
            (None, 2, 2, None)
        );
        let elsewhere = alone(&parsed, "HDMI-A-1");
        assert_eq!(
            group_of(&elsewhere, LayerKind::Desktop, "box").arrange,
            Some(Arrange::Row)
        );
        assert!(validate_unsets(&parsed, &Library::default()).is_clean());
        assert!(
            validate(&parsed, &Modules).is_clean(),
            "{}",
            validate(&parsed, &Modules).render()
        );
    }

    /// The same take-back across `extends`, and a level above the one that takes it back can arrange the group again, since a level's own keys are laid over after what it takes back.
    #[test]
    fn a_layout_takes_back_the_arrangement_of_the_layout_it_extends() {
        let base = layout(&arranged_row(""));
        let known = Library::of_layouts([base.clone()]);
        let mine = layout(&format!(
            "id = \"mine\"\nextends = \"test\"\n{}",
            loose(r#"unset = ["arrange"]"#).replace("DP-1", "*")
        ));
        let (resolved, report) = resolve(&mine, &known, "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(group_of(&resolved, LayerKind::Desktop, "box").arrange, None);
        assert!(validate_unsets(&mine, &known).is_clean());

        let again = layout(&format!(
            "id = \"mine\"\nextends = \"test\"\n{}{}",
            loose(r#"unset = ["arrange"]"#).replace("DP-1", "*"),
            loose(r#"arrange = "column""#),
        ));
        let (resolved, _) = resolve(&again, &known, "DP-1", None);
        let group = group_of(&resolved, LayerKind::Desktop, "box");
        assert_eq!((group.arrange, group.gap), (Some(Arrange::Column), None));
        assert!(validate_unsets(&again, &known).is_clean());
    }

    /// A level that takes the arrangement back and writes any key of it is an error at the `unset` entry, and so is a child place its group no longer takes; taking back what nothing under the level arranges is only a warning.
    #[test]
    fn taking_back_an_arrangement_is_judged_after_the_take_back() {
        for written in [r#"arrange = "column""#, "gap = 2.0", "cols = 3", "rows = 3"] {
            let parsed = layout(&arranged_row(&loose(&format!(
                "{written}\nunset = [\"arrange\"]"
            ))));
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(&report, "groups.box.unset[0]", "finding.unset_and_written"),
                "{written}: {}",
                report.render()
            );
        }

        let weighted = arranged_row("").replace("id = \"clock-1\"", "id = \"clock-1\"\nweight = 2");
        let taking = layout(&format!("{weighted}{}", loose(r#"unset = ["arrange"]"#)));
        let report = validate(&taking, &Modules);
        assert!(
            finding_at(
                &report,
                "outputs.DP-1.layers.desktop.areas.widgets.groups.box.children.clock-1.weight",
                "finding.child_weight"
            ),
            "{}",
            report.render()
        );
        let kept = layout(&weighted);
        assert!(validate(&kept, &Modules).is_clean());

        let nothing = layout(&format!(
            "id = \"test\"\n{}",
            loose(r#"unset = ["arrange"]"#).replace("DP-1", "*")
        ));
        let report = validate_unsets(&nothing, &Library::default());
        assert!(report.errors.is_empty(), "{}", report.render());
        assert_eq!(report.warnings.len(), 1, "{}", report.render());
        assert_eq!(
            report.warnings[0].message.key(),
            Some("finding.unset_nothing")
        );
    }

    /// An area or an instance cannot take `arrange` back, whatever a group can.
    #[test]
    fn only_a_group_takes_an_arrangement_back() {
        let parsed = layout(&format!(
            "{}{}",
            arranged_row(""),
            r#"
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            unset = ["arrange"]
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            unset = ["arrange"]
            "#
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            finding_at(&report, "widgets.unset[0]", "finding.unset_on_area"),
            "{}",
            report.render()
        );
        assert!(
            finding_at(
                &report,
                "children.clock-1.unset[0]",
                "finding.unset_on_instance"
            ),
            "{}",
            report.render()
        );
    }

    /// The rename is clean: `stacked = true` is a located error on the key itself that names what to write instead, in a layout and in a komponent's file alike.
    #[test]
    fn a_written_stacked_points_at_arrange_pages() {
        let text = r#"id = "t"
[[outputs]]
match = "*"
[[outputs.layers.desktop.areas]]
id = "widgets"
kind = "grid"
[[outputs.layers.desktop.areas.groups]]
id = "weather"
place = "cell"
col = 0
row = 0
stacked = true
"#;
        let report = validate::check_unknown_keys(text, &LayoutId::new("t"));
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        let finding = &report.errors[0];
        assert_eq!(
            finding.key,
            "outputs.*.layers.desktop.areas.widgets.groups.weather.stacked"
        );
        assert_eq!(finding.message.key(), Some("finding.retired_key"));
        assert!(
            finding.message.english().contains(r#"arrange = "pages""#),
            "{}",
            finding.message.english()
        );
        let span = finding.span.as_ref().expect("located");
        assert_eq!((span.line, span.column), (12, 1), "at the key itself");

        let komponent = format!("stacked = true\n{PILL}");
        assert!(toml::from_str::<Komponent>(&komponent).is_err());
        let finding = validate::retired_in_komponent(&komponent, "components/pill.toml")
            .expect("named for what it is");
        assert_eq!(
            (finding.key.as_str(), finding.message.key()),
            ("stacked", Some("finding.retired_key"))
        );
        assert_eq!(finding.span.map(|span| span.line), Some(1));
        assert!(validate::retired_in_komponent(PILL, "components/pill.toml").is_none());
    }

    /// A bar's zone already lays its run along the edge, so a group there takes `pages` and nothing else — and one written otherwise is drawn as a loose run.
    #[test]
    fn a_zone_group_arranges_its_children_only_as_pages() {
        let zoned = |arrange: &str| {
            layout(&ONE_BAR.replace(
                "id = \"start\"",
                &format!("id = \"start\"\narrange = \"{arrange}\""),
            ))
        };
        for arrange in ["row", "column", "grid", "free"] {
            let parsed = zoned(arrange);
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(&report, "groups.start.arrange", "finding.zone_arrange"),
                "{arrange}: {}",
                report.render()
            );
            let resolved = alone(&parsed, "DP-1");
            assert_eq!(
                group_of(&resolved, LayerKind::Top, "start").arrange,
                None,
                "{arrange}"
            );
        }
        let pages = zoned("pages");
        assert!(validate(&pages, &Modules).is_clean());
        assert!(group_of(&alone(&pages, "DP-1"), LayerKind::Top, "start").is_pages());
    }

    /// `weight`, `cell` and `rect` each place a child of one arrangement, and mean nothing anywhere else.
    #[test]
    fn a_child_place_needs_the_arrangement_that_takes_it() {
        let cases = [
            (
                "arrange = \"row\"",
                "cell = { col = 0, row = 0 }",
                "cell",
                "finding.child_cell",
            ),
            (
                "arrange = \"column\"",
                "rect = { x = 0, y = 0, w = 1, h = 1 }",
                "rect",
                "finding.child_rect",
            ),
            (
                "arrange = \"grid\"",
                "weight = 2",
                "weight",
                "finding.child_weight",
            ),
            (
                "arrange = \"free\"",
                "cell = { col = 0, row = 0 }",
                "cell",
                "finding.child_cell",
            ),
            (
                "arrange = \"pages\"",
                "weight = 2",
                "weight",
                "finding.child_weight",
            ),
            (
                "",
                "rect = { x = 0, y = 0, w = 1, h = 1 }",
                "rect",
                "finding.child_rect",
            ),
        ];
        for (arrange, place, key, message) in cases {
            let report = validate(&container(arrange, &child("clock-1", place)), &Modules);
            assert!(
                finding_at(&report, &format!("children.clock-1.{key}"), message),
                "{arrange} with {place}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{}", report.render());
        }
        for (arrange, place) in [
            ("arrange = \"row\"", "weight = 2"),
            ("arrange = \"column\"", "weight = 0.5"),
            ("arrange = \"grid\"", "cell = { col = 1, row = 1 }"),
            (
                "arrange = \"free\"",
                "rect = { x = 0.5, y = 0, w = 0.5, h = 1 }",
            ),
        ] {
            let report = validate(&container(arrange, &child("clock-1", place)), &Modules);
            assert!(
                report.is_clean(),
                "{arrange} with {place}: {}",
                report.render()
            );
        }
    }

    /// A place written at one level and an arrangement written at another are judged on the merged group, and reported at the level that writes either half.
    #[test]
    fn a_child_place_is_judged_on_the_merged_group() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            arrange = "grid"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            cell = { col = 1, row = 0 }
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            arrange = "row"
            "#,
        );
        let report = validate(&parsed, &Modules);
        let keys: Vec<&str> = report.errors.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["outputs.DP-1.layers.desktop.areas.widgets.groups.box.children.clock-1.cell"],
            "the monitor rule makes the cell mean nothing, so it is the one told: {}",
            report.render()
        );

        let extending = layout(
            r#"
            id = "test"
            extends = "parent"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-1"
            module = "clock"
            cell = { col = 1, row = 0 }
            "#,
        );
        assert!(
            validate(&extending, &Modules).is_clean(),
            "a group the parent places may be a grid there"
        );
    }

    /// A weight is a share of the group's length, so one that is not above 0 is refused wherever it is written.
    #[test]
    fn a_weight_is_above_zero() {
        for weight in ["0", "-1", "nan"] {
            let parsed = container(
                "arrange = \"row\"",
                &child("clock-1", &format!("weight = {weight}")),
            );
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(
                    &report,
                    "children.clock-1.weight",
                    "finding.weight_not_positive"
                ),
                "{weight}: {}",
                report.render()
            );
            assert_eq!(
                placements_of(group_of(&alone(&parsed, "DP-1"), LayerKind::Desktop, "box")),
                [Some(Placement::Weight(1.0))],
                "{weight} is drawn as the weight a child has unless it says"
            );
        }
    }

    /// A weight counts between the bounds `Instance::WEIGHTS` names: one outside them is a warning and is drawn at the nearer bound, one inside is neither.
    #[test]
    fn a_weight_counts_within_its_bounds() {
        for (weight, drawn) in [
            ("0.1", 0.25),
            ("20", 8.0),
            ("0.25", 0.25),
            ("8", 8.0),
            ("3", 3.0),
        ] {
            let parsed = container(
                "arrange = \"row\"",
                &child("clock-1", &format!("weight = {weight}")),
            );
            let report = validate(&parsed, &Modules);
            let told = finding_at(
                &report,
                "children.clock-1.weight",
                "finding.weight_out_of_range",
            );
            assert_eq!(told, drawn != weight.parse::<f32>().unwrap(), "{weight}");
            assert!(report.errors.is_empty(), "{weight}: {}", report.render());
            assert_eq!(
                placements_of(group_of(&alone(&parsed, "DP-1"), LayerKind::Desktop, "box")),
                [Some(Placement::Weight(drawn))],
                "{weight}"
            );
        }
    }

    /// An inner grid has at least one column and one row, and a gap is not negative.
    #[test]
    fn an_inner_grid_has_tracks_and_a_gap_is_not_negative() {
        let report = validate(
            &container("arrange = \"grid\"\ncols = 0\nrows = 0\ngap = -4", ""),
            &Modules,
        );
        assert!(finding_at(&report, "groups.box.cols", "finding.no_tracks"));
        assert!(finding_at(&report, "groups.box.rows", "finding.no_tracks"));
        assert!(finding_at(
            &report,
            "groups.box.gap",
            "finding.gap_negative"
        ));
        assert_eq!(report.errors.len(), 3, "{}", report.render());
    }

    /// A gap that is no number at all is refused as a negative one is, on a group, a grid and a panel alike.
    #[test]
    fn a_gap_that_is_no_number_is_refused() {
        let grouped = container("arrange = \"row\"\ngap = nan", "");
        let gridded = layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.desktop.areas]]
            id = "grid"
            kind = "grid"
            gap = nan
            "#
        ));
        let paneled = with_owners(&panel("panel-clock", "owner = \"clock-1\"\ngap = nan"));
        for (parsed, key) in [
            (grouped, "groups.box.gap"),
            (gridded, "areas.grid.gap"),
            (paneled, "areas.panel-clock.gap"),
        ] {
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(&report, key, "finding.gap_negative"),
                "{key}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{key}: {}", report.render());
        }
    }

    /// `cols` and `rows` size a `grid` group's inner grid and `gap` spaces what a group arranges, so one written where its group's arrangement reads none is reported, wherever the arrangement comes from.
    #[test]
    fn arrangement_keys_the_arrangement_does_not_read_are_reported() {
        for (group, key, message) in [
            (
                "arrange = \"row\"\ncols = 3",
                "groups.box.cols",
                "finding.tracks_off_grid",
            ),
            (
                "arrange = \"free\"\nrows = 3",
                "groups.box.rows",
                "finding.tracks_off_grid",
            ),
            (
                "arrange = \"pages\"\ngap = 4",
                "groups.box.gap",
                "finding.gap_unspaced",
            ),
            ("gap = 4", "groups.box.gap", "finding.gap_unspaced"),
        ] {
            let report = validate(&container(group, ""), &Modules);
            assert!(
                finding_at(&report, key, message),
                "{group}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{group}: {}", report.render());
        }
        for group in [
            "arrange = \"grid\"\ncols = 3\nrows = 3\ngap = 4",
            "arrange = \"row\"\ngap = 4",
            "arrange = \"free\"\ngap = 4",
        ] {
            let report = validate(&container(group, ""), &Modules);
            assert!(report.is_clean(), "{group}: {}", report.render());
        }

        let rearranged = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            arrange = "grid"
            cols = 3
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            arrange = "column"
            "#,
        );
        let report = validate(&rearranged, &Modules);
        assert!(
            finding_at(
                &report,
                "outputs.DP-1.layers.desktop.areas.widgets.groups.box.cols",
                "finding.tracks_off_grid"
            ),
            "the level that changes the arrangement hears of it: {}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());

        let mut komponent = pill();
        komponent.arrange = Some(Arrange::Row);
        komponent.cols = Some(2);
        let report = validate_komponent(&KomponentId::new("pill"), &komponent, &Modules);
        assert!(
            finding_at(&report, "cols", "finding.tracks_off_grid"),
            "{}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());
    }

    /// Each child of a `grid` or `free` group has a place of its own, so a list repeated over them is refused — where a grid cell's own rule does not already say so.
    #[test]
    fn repeat_is_refused_where_each_child_has_a_place() {
        for arrange in ["grid", "free"] {
            let zoned = layout(&ONE_BAR.replace(
                "id = \"start\"",
                &format!("id = \"start\"\narrange = \"{arrange}\"\nrepeat = \"$battery.cells\""),
            ));
            let report = validate(&zoned, &Modules);
            assert!(
                finding_at(&report, "groups.start.repeat", "finding.arranged_repeats"),
                "{}",
                report.render()
            );

            let celled = container(
                &format!("arrange = \"{arrange}\"\nrepeat = \"$battery.cells\""),
                "",
            );
            let report = validate(&celled, &Modules);
            let messages: Vec<_> = report
                .errors
                .iter()
                .filter_map(|f| f.message.key())
                .collect();
            assert_eq!(messages, ["finding.cell_repeats"], "said once");

            let mut komponent = pill();
            komponent.arrange = Some(if arrange == "grid" {
                Arrange::Grid
            } else {
                Arrange::Free
            });
            komponent.repeat = Some(Expr("$battery.cells".into()));
            let report = validate_komponent(&KomponentId::new("pill"), &komponent, &Modules);
            assert!(
                finding_at(&report, "repeat", "finding.arranged_repeats"),
                "{}",
                report.render()
            );
        }
        let rows = layout(&ONE_BAR.replace(
            "id = \"start\"",
            "id = \"start\"\narrange = \"pages\"\nrepeat = \"$battery.cells\"",
        ));
        assert!(validate(&rows, &Modules).is_clean());
    }

    /// A komponent arranges its children itself: what its file places is checked against its own `arrange`, and a use in a bar's zone takes `pages` only.
    #[test]
    fn a_komponent_arranges_its_children_and_a_zone_use_takes_pages() {
        let id = KomponentId::new("pill");
        let mut grid = pill();
        grid.arrange = Some(Arrange::Grid);
        grid.cols = Some(0);
        grid.children[0].cell = Some(ChildCell::at(1, 0));
        grid.children[1].weight = Some(2.0);
        let report = validate_komponent(&id, &grid, &Modules);
        assert!(finding_at(&report, "cols", "finding.no_tracks"));
        assert!(finding_at(
            &report,
            "children.clock.weight",
            "finding.child_weight"
        ));
        assert_eq!(report.errors.len(), 2, "{}", report.render());
        assert_eq!(report.errors[0].key, "cols");

        let report = validate_komponents(&layout(TWO_PILLS), &with_pill_as(grid.clone()), &Modules);
        assert!(
            finding_at(&report, "groups.start.komponent", "finding.zone_arrange"),
            "{}",
            report.render()
        );

        let used = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            komponent = "pill"
            "#,
        );
        grid.cols = Some(3);
        let library = with_pill_as(grid);
        let no_parameters = BTreeMap::new();
        let at_zone = check_use(
            &library,
            &Modules,
            (&KomponentId::new("pill"), &no_parameters),
            LayerKind::Top,
            false,
        );
        assert_eq!(at_zone.err(), Some(UseError::ZoneArrange(Arrange::Grid)));
        assert!(validate_komponents(&used, &library, &Modules).is_clean());
        let (resolved, _) = resolve(&used, &library, "DP-1", None);
        let drawn = group_of(&resolved, LayerKind::Desktop, "box");
        assert_eq!((drawn.arrange, drawn.cols), (Some(Arrange::Grid), 3));
        assert_eq!(
            placements_of(drawn),
            [
                Some(Placement::Cell(ChildCell::at(1, 0))),
                Some(Placement::Cell(ChildCell::at(0, 0))),
            ]
        );

        let holding_more = layout(&TWO_PILLS.replace(
            "zone = \"end\"\n        komponent = \"pill\"",
            "zone = \"end\"\n        komponent = \"pill\"\n        arrange = \"pages\"",
        ));
        let (_, report) = resolve(&holding_more, &with_pill([]), "DP-1", None);
        assert!(
            finding_at(
                &report,
                "groups.end.komponent",
                "finding.komponent_holds_more"
            ),
            "{}",
            report.render()
        );
    }

    fn with_pill_as(komponent: Komponent) -> Library {
        Library::default().with_komponent("pill", komponent)
    }

    /// The lock layer takes containers: what it refuses is decided per child, as for any group there.
    #[test]
    fn a_lock_layer_container_still_holds_readings_only() {
        let parsed = layout(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.layers.lock.areas]]
            id = "readings"
            kind = "grid"
            [[outputs.layers.lock.areas.groups]]
            id = "box"
            place = "cell"
            col = 0
            row = 0
            arrange = "row"
            [[outputs.layers.lock.areas.groups.children]]
            id = "lock-clock"
            module = "clock"
            weight = 2
            [[outputs.layers.lock.areas.groups.children]]
            id = "lock-mixer"
            module = "mixer"
            [[outputs.layers.lock.areas]]
            id = "prompt"
            kind = "prompt"
            "#,
        );
        for report in [
            validate(&parsed, &Modules),
            crate::validate::validate_lock(&parsed, &Modules),
        ] {
            let keys: Vec<(&str, Option<&str>)> = report
                .errors
                .iter()
                .map(|f| (f.key.as_str(), f.message.key()))
                .collect();
            assert_eq!(
                keys,
                [(
                    "outputs.*.layers.lock.areas.readings.groups.box.children.lock-mixer.module",
                    Some("finding.lock_interactive")
                )],
                "{}",
                report.render()
            );
        }
    }

    /// A child with no cell takes the first cell no other child covers, in reading order; one past the inner grid is pulled back onto it and said so; and a `free` child with no `rect` is half the box, a step down from the one before.
    #[test]
    fn missing_and_stray_child_places_are_answered() {
        use crate::container::cells;
        let wide = ChildCell {
            col: 0,
            row: 0,
            col_span: 2,
            row_span: 1,
        };
        assert_eq!(
            cells(&[None, Some(wide), None, None], 3, 2),
            [
                ChildCell::at(2, 0),
                wide,
                ChildCell::at(0, 1),
                ChildCell::at(1, 1),
            ],
            "the written cell is placed first, then the rest in reading order around it"
        );
        assert_eq!(
            cells(
                &[Some(ChildCell {
                    col: 5,
                    row: 1,
                    col_span: 4,
                    row_span: 3,
                })],
                2,
                2
            ),
            [ChildCell {
                col: 1,
                row: 1,
                col_span: 1,
                row_span: 1,
            }]
        );
        assert_eq!(
            cells(&[None, None], 1, 1),
            [ChildCell::at(0, 0), ChildCell::at(0, 0)],
            "a full inner grid puts the rest at its top left"
        );

        let stray = container(
            "arrange = \"grid\"",
            &format!(
                "{}{}",
                child("clock-1", "cell = { col = 4, row = 0 }"),
                child("clock-2", "")
            ),
        );
        let (resolved, report) = resolve(&stray, &Library::default(), "DP-1", None);
        assert_eq!(
            placements_of(group_of(&resolved, LayerKind::Desktop, "box")),
            [
                Some(Placement::Cell(ChildCell::at(1, 0))),
                Some(Placement::Cell(ChildCell::at(0, 0))),
            ]
        );
        let keys: Vec<(&str, Option<&str>)> = report
            .warnings
            .iter()
            .map(|f| (f.key.as_str(), f.message.key()))
            .collect();
        assert_eq!(
            keys,
            [(
                "layers.desktop.areas.widgets.groups.box.children.clock-1.cell",
                Some("finding.cell_off_grid")
            )]
        );
        assert!(report.errors.is_empty(), "{}", report.render());

        let free = container(
            "arrange = \"free\"",
            &format!(
                "{}{}{}",
                child("clock-1", ""),
                child("clock-2", "rect = { x = 0.5, y = 0.5, w = 0.5, h = 0.5 }"),
                child("clock-3", "")
            ),
        );
        let half = |x: f32, y: f32| {
            Some(Placement::Rect(Rect {
                x,
                y,
                w: 0.5,
                h: 0.5,
            }))
        };
        assert_eq!(
            placements_of(group_of(&alone(&free, "DP-1"), LayerKind::Desktop, "box")),
            [half(0.0, 0.0), half(0.5, 0.5), half(0.1, 0.1)]
        );

        let loose = container("", &child("clock-1", ""));
        assert_eq!(
            placements_of(group_of(&alone(&loose, "DP-1"), LayerKind::Desktop, "box")),
            [None]
        );
    }

    /// Saving a container as a komponent keeps how it is arranged and where each child sits, cells written out as they are drawn; detaching gives the group the arrangement back.
    #[test]
    fn a_saved_container_keeps_its_arrangement_and_places() {
        let parsed = container(
            "arrange = \"grid\"\ncols = 3\ngap = 4",
            &format!(
                "{}{}",
                child("clock-1", "cell = { col = 2, row = 0 }"),
                child("clock-2", "")
            ),
        );
        let resolved = alone(&parsed, "DP-1");
        let saved = crate::components::saved(group_of(&resolved, LayerKind::Desktop, "box"), &[]);
        assert_eq!(
            (saved.arrange, saved.cols, saved.rows, saved.gap),
            (Some(Arrange::Grid), Some(3), Some(2), Some(4.0))
        );
        let cells: Vec<Option<ChildCell>> = saved.children.iter().map(|it| it.cell).collect();
        assert_eq!(
            cells,
            [Some(ChildCell::at(2, 0)), Some(ChildCell::at(0, 0))]
        );
        assert!(validate_komponent(&KomponentId::new("box"), &saved, &Modules).is_clean());

        let row = container("arrange = \"row\"", &child("clock-1", ""));
        let saved = crate::components::saved(
            group_of(&alone(&row, "DP-1"), LayerKind::Desktop, "box"),
            &[],
        );
        assert_eq!((saved.cols, saved.rows), (None, None));
        assert_eq!(
            saved.children[0].weight, None,
            "a weight of 1 is what a child has"
        );
    }

    /// Turning a group into a stack and back is one operation each way, its inner grid and gap riding along.
    #[test]
    fn set_group_arrange_puts_back_what_it_replaced() {
        let mut parsed = container("arrange = \"grid\"\ncols = 3", "");
        let before = parsed.clone();
        let undo = crate::ops::apply(
            &mut parsed,
            &LayoutOp::SetGroupArrange {
                site: Site::everywhere(LayerKind::Desktop),
                area: AreaId::new("widgets"),
                id: GroupId::new("box"),
                arrange: Some(Arrange::Pages),
                cols: None,
                rows: None,
                gap: Some(6.0),
            },
        )
        .expect("applies");
        let group = &parsed.outputs[0].layers.desktop.areas[0].groups[0];
        assert_eq!(
            (group.arrange, group.cols, group.gap),
            (Some(Arrange::Pages), None, Some(6.0))
        );
        crate::ops::apply(&mut parsed, &undo).expect("undoes");
        assert_eq!(parsed, before);
    }

    /// A bar with `clock-1` in it and a free area with `battery-1` on the top layer, a grid with `clock-2` on the desktop, and whatever `extra` adds.
    fn with_owners(extra: &str) -> Layout {
        layout(&format!(
            r#"{ONE_BAR}
            [[outputs.layers.top.areas]]
            id = "corner"
            kind = "free"
            rect = {{ x = 0.0, y = 0.0, w = 0.2, h = 0.2 }}
            [[outputs.layers.top.areas.groups]]
            id = "held"
            place = "zone"
            zone = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "battery-1"
            module = "battery"
            [[outputs.layers.desktop.areas]]
            id = "widgets"
            kind = "grid"
            [[outputs.layers.desktop.areas.groups]]
            id = "w"
            place = "cell"
            col = 0
            row = 0
            [[outputs.layers.desktop.areas.groups.children]]
            id = "clock-2"
            module = "clock"
            representation = "widget_m"
            {extra}
            "#
        ))
    }

    /// A panel on the top layer of the `*` rule, `keys` written in it.
    fn panel(id: &str, keys: &str) -> String {
        format!(
            r#"
            [[outputs.layers.top.areas]]
            id = "{id}"
            kind = "panel"
            {keys}
            "#
        )
    }

    fn panel_of(resolved: &Resolved, id: &str) -> Option<ResolvedAreaKind> {
        resolved
            .area(LayerKind::Top, &AreaId::new(id))
            .map(|area| area.kind.clone())
    }

    /// A panel needs nothing but its owner: it is 4 × 3 of a grid's cells beside it, and holds cell groups like a grid. What it writes is what it is.
    #[test]
    fn a_panel_resolves_from_its_owner_with_a_grid_s_cells() {
        let parsed = with_owners(&panel("panel-clock", "owner = \"clock-1\""));
        assert!(
            validate(&parsed, &Modules).is_clean(),
            "{}",
            validate(&parsed, &Modules).render()
        );
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(
            panel_of(&resolved, "panel-clock"),
            Some(ResolvedAreaKind::Panel {
                owner: InstanceId::new("clock-1"),
                along: false,
                cols: 4,
                rows: 3,
                cell: 80.0,
                gap: 16.0,
            })
        );

        let written = with_owners(&format!(
            "{}{}",
            panel(
                "panel-clock",
                "owner = \"clock-1\"\nalong = true\ncols = 16\nrows = 2\ncell = 64\ngap = 8"
            ),
            r#"
            [[outputs.layers.top.areas.groups]]
            id = "inside"
            place = "cell"
            col = 0
            row = 0
            [[outputs.layers.top.areas.groups.children]]
            id = "battery-2"
            module = "battery"
            representation = "widget_s"
            "#
        ));
        assert!(validate(&written, &Modules).is_clean());
        let resolved = alone(&written, "DP-1");
        assert_eq!(
            panel_of(&resolved, "panel-clock"),
            Some(ResolvedAreaKind::Panel {
                owner: InstanceId::new("clock-1"),
                along: true,
                cols: 16,
                rows: 2,
                cell: 64.0,
                gap: 8.0,
            })
        );
        assert_eq!(
            child_ids(group_of(&resolved, LayerKind::Top, "inside")),
            ["battery-2"]
        );
    }

    /// A panel nothing opens is unfinished, like a bar with no edge: it is left out and the rest of the layer draws.
    #[test]
    fn a_panel_with_no_owner_is_left_out_and_reported() {
        let parsed = with_owners(&panel("panel-clock", "cols = 2"));
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(
            finding_at(
                &report,
                "layers.top.areas.panel-clock.owner",
                "finding.area_needs"
            ),
            "{}",
            report.render()
        );
        assert_eq!(area_ids(&resolved, LayerKind::Top), ["bar-top", "corner"]);
    }

    /// The owner is an instance of the panel's own layer: one on another layer, or none at all, opens nothing, so the panel is reported and not drawn.
    #[test]
    fn a_panel_s_owner_is_an_instance_on_its_own_layer() {
        for owner in ["clock-2", "nobody"] {
            let parsed = with_owners(&panel("panel-x", &format!("owner = \"{owner}\"")));
            let report = validate(&parsed, &Modules);
            assert!(
                finding_at(
                    &report,
                    "outputs.*.layers.top.areas.panel-x.owner",
                    "finding.panel_owner_missing"
                ),
                "{owner}: {}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{owner}: {}", report.render());
            assert_eq!(
                panel_of(&alone(&parsed, "DP-1"), "panel-x"),
                None,
                "{owner}"
            );
        }
    }

    /// Nothing inside a panel opens another one, so a panel of an instance in a panel is reported and only the outer one is drawn.
    #[test]
    fn nothing_inside_a_panel_opens_another() {
        let parsed = with_owners(&format!(
            "{}{}{}",
            panel("panel-clock", "owner = \"clock-1\""),
            r#"
            [[outputs.layers.top.areas.groups]]
            id = "inside"
            place = "cell"
            col = 0
            row = 0
            [[outputs.layers.top.areas.groups.children]]
            id = "battery-2"
            module = "battery"
            "#,
            panel("panel-battery", "owner = \"battery-2\"")
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            finding_at(
                &report,
                "areas.panel-battery.owner",
                "finding.panel_owner_in_panel"
            ),
            "{}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        let resolved = alone(&parsed, "DP-1");
        assert!(panel_of(&resolved, "panel-clock").is_some());
        assert_eq!(panel_of(&resolved, "panel-battery"), None);
    }

    /// `along` runs a panel along its owner's bar, so it is said of an owner in a bar alone; one elsewhere is reported and drawn beside its owner.
    #[test]
    fn along_is_for_an_owner_in_a_bar() {
        let off_bar = with_owners(&panel(
            "panel-battery",
            "owner = \"battery-1\"\nalong = true",
        ));
        let report = validate(&off_bar, &Modules);
        assert!(
            finding_at(&report, "areas.panel-battery.along", "finding.panel_along"),
            "{}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert!(matches!(
            panel_of(&alone(&off_bar, "DP-1"), "panel-battery"),
            Some(ResolvedAreaKind::Panel { along: false, .. })
        ));

        let beside = with_owners(&panel("panel-battery", "owner = \"battery-1\""));
        assert!(validate(&beside, &Modules).is_clean());
        let said_beside = with_owners(&panel(
            "panel-battery",
            "owner = \"battery-1\"\nalong = false",
        ));
        let report = validate(&said_beside, &Modules);
        assert!(
            report.is_clean(),
            "`along = false` asks for what it gets: {}",
            report.render()
        );
        let in_bar = with_owners(&panel("panel-clock", "owner = \"clock-1\"\nalong = true"));
        assert!(validate(&in_bar, &Modules).is_clean());
    }

    /// An instance opens one panel at most, so a press has one answer: the second is reported and only the first is drawn.
    #[test]
    fn an_instance_opens_one_panel_at_most() {
        let parsed = with_owners(&format!(
            "{}{}",
            panel("panel-clock", "owner = \"clock-1\""),
            panel("dock-clock", "owner = \"clock-1\"\nalong = true")
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            finding_at(
                &report,
                "areas.dock-clock.owner",
                "finding.panel_owner_taken"
            ),
            "{}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        let resolved = alone(&parsed, "DP-1");
        assert!(panel_of(&resolved, "panel-clock").is_some());
        assert_eq!(panel_of(&resolved, "dock-clock"), None);
    }

    /// A panel opens over what is there: one that says it reserves is reported, and reserves nothing.
    #[test]
    fn a_panel_never_reserves() {
        let parsed = with_owners(&panel("panel-clock", "owner = \"clock-1\"\nreserve = true"));
        let report = validate(&parsed, &Modules);
        assert!(
            finding_at(
                &report,
                "areas.panel-clock.reserve",
                "finding.panel_reserve"
            ),
            "{}",
            report.render()
        );
        let resolved = alone(&parsed, "DP-1");
        assert_eq!(
            resolved
                .area(LayerKind::Top, &AreaId::new("panel-clock"))
                .map(|area| area.reserve),
            Some(false)
        );
        assert_eq!(resolved.reserved(config::Edge::Top), 32.0);
    }

    /// The lock layer holds readings and opens nothing, so a panel there is refused — by the check that decides whether the lock falls back to the minimal one too.
    #[test]
    fn a_panel_is_refused_on_the_lock_layer() {
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
            id = "time"
            place = "cell"
            col = 0
            row = 0
            [[outputs.layers.lock.areas.groups.children]]
            id = "lock-clock"
            module = "clock"
            representation = "widget_m"
            [[outputs.layers.lock.areas]]
            id = "panel-clock"
            kind = "panel"
            owner = "lock-clock"
            "#,
        );
        for report in [
            validate(&parsed, &Modules),
            validate::validate_lock(&parsed, &Modules),
        ] {
            assert!(
                finding_at(
                    &report,
                    "layers.lock.areas.panel-clock.kind",
                    "finding.panel_on_lock"
                ),
                "{}",
                report.render()
            );
            assert_eq!(report.errors.len(), 1, "{}", report.render());
        }
        assert_eq!(
            area_ids(&alone(&parsed, "DP-1"), LayerKind::Lock),
            ["readings", "prompt"]
        );
    }

    /// A panel reserves nothing, so a workspace rule may add one, and switching to that workspace re-tiles nothing.
    #[test]
    fn a_workspace_rule_may_add_a_panel() {
        let parsed = with_owners(
            r#"
            [[outputs.workspaces]]
            match = "web"
            [[outputs.workspaces.layers.top.areas]]
            id = "panel-clock"
            kind = "panel"
            owner = "clock-1"
            along = true
            "#,
        );
        let report = validate(&parsed, &Modules);
        assert!(report.is_clean(), "{}", report.render());
        let web = ActiveWorkspace {
            name: "web".into(),
            ..ActiveWorkspace::default()
        };
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", Some(&web));
        assert!(report.is_clean(), "{}", report.render());
        assert!(panel_of(&resolved, "panel-clock").is_some());
        assert_eq!(resolved.reserved(config::Edge::Top), 32.0);

        let mut edited = with_owners("");
        crate::ops::apply_all(
            &mut edited,
            &[
                LayoutOp::InsertWorkspaceRule {
                    output: OutputMatch("*".into()),
                    index: 0,
                    rule: Box::new(WorkspaceRule {
                        matches: WorkspaceMatch("web".into()),
                        ..WorkspaceRule::default()
                    }),
                },
                LayoutOp::InsertArea {
                    site: Site::everywhere(LayerKind::Top).on_workspace("web"),
                    index: 0,
                    area: Box::new(Area {
                        id: AreaId::new("panel-clock"),
                        kind: Some(AreaKind::Panel {
                            owner: Some(InstanceId::new("clock-1")),
                            along: None,
                            cols: None,
                            rows: None,
                            cell: None,
                            gap: None,
                        }),
                        ..Area::default()
                    }),
                },
            ],
        )
        .expect("an edit adds a panel in a workspace rule");
    }

    /// A monitor rule names a panel by id and changes one key, inheriting the rest.
    #[test]
    fn a_monitor_rule_merges_a_panel_key_by_key() {
        let parsed = with_owners(&format!(
            r#"{}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "panel-clock"
            kind = "panel"
            rows = 2
            "#,
            panel("panel-clock", "owner = \"clock-1\"\ncols = 6\ncell = 64")
        ));
        assert!(validate(&parsed, &Modules).is_clean());
        assert_eq!(
            panel_of(&alone(&parsed, "DP-1"), "panel-clock"),
            Some(ResolvedAreaKind::Panel {
                owner: InstanceId::new("clock-1"),
                along: false,
                cols: 6,
                rows: 2,
                cell: 64.0,
                gap: 16.0,
            })
        );
        assert!(matches!(
            panel_of(&alone(&parsed, "HDMI-A-1"), "panel-clock"),
            Some(ResolvedAreaKind::Panel { rows: 3, .. })
        ));
    }

    /// A monitor rule that takes the owner away takes its panel with it on that monitor, without naming the panel and without a finding; every other monitor keeps both.
    #[test]
    fn a_panel_goes_where_its_owner_is_taken_away() {
        let parsed = with_owners(&format!(
            r#"{}
            [[outputs]]
            match = "DP-1"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            [[outputs.layers.top.areas.groups]]
            id = "start"
            remove = ["clock-1"]
            "#,
            panel("panel-clock", "owner = \"clock-1\"")
        ));
        let report = validate(&parsed, &Modules);
        assert!(report.is_clean(), "{}", report.render());
        let (resolved, report) = resolve(&parsed, &Library::default(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(panel_of(&resolved, "panel-clock"), None);
        assert!(panel_of(&alone(&parsed, "HDMI-A-1"), "panel-clock").is_some());
    }

    /// A child of a komponent a group draws can own a panel, by the id it is drawn under.
    #[test]
    fn a_komponent_s_child_can_own_a_panel() {
        let parsed = layout(&format!(
            "{TWO_PILLS}{}",
            panel("panel-battery", "owner = \"bar-top.end/battery\"")
        ));
        let report = validate(&parsed, &Modules);
        assert!(
            report
                .findings()
                .all(|f| f.message.key() != Some("finding.panel_owner_missing")),
            "{}",
            report.render()
        );
        let library = with_pill([]);
        let report = validate_komponents(&parsed, &library, &Modules);
        assert!(report.is_clean(), "{}", report.render());
        let (resolved, _) = resolve(&parsed, &library, "DP-1", None);
        assert!(panel_of(&resolved, "panel-battery").is_some());
    }

    /// An owner under a komponent's use names a child the komponent holds: one it does not hold opens nothing, so it is reported where the library is known and not drawn.
    #[test]
    fn a_komponent_owner_names_a_child_the_komponent_holds() {
        let parsed = layout(&format!(
            "{TWO_PILLS}{}",
            panel("panel-battery", "owner = \"bar-top.end/typo\"")
        ));
        let library = with_pill([]);
        let report = validate_komponents(&parsed, &library, &Modules);
        assert!(
            finding_at(
                &report,
                "outputs.*.layers.top.areas.panel-battery.owner",
                "finding.panel_owner_missing"
            ),
            "{}",
            report.render()
        );
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        let (resolved, _) = resolve(&parsed, &library, "DP-1", None);
        assert_eq!(panel_of(&resolved, "panel-battery"), None);
    }

    /// Detaching a komponent gives its children ids of their own, and a panel one of them opens follows it there rather than going with the old id.
    #[test]
    fn a_panel_follows_its_owner_out_of_a_detached_komponent() {
        let mut parsed = layout(&format!(
            "{TWO_PILLS}{}",
            panel("panel-battery", "owner = \"bar-top.end/battery\"")
        ));
        let library = with_pill([]);
        let ops = crate::components::detach(
            &parsed,
            &library,
            (
                &Site::everywhere(LayerKind::Top),
                &AreaId::new("bar-top"),
                &GroupId::new("end"),
            ),
            ("DP-1", None),
        )
        .expect("it detaches");
        crate::ops::apply_all(&mut parsed, &ops).expect("applies");
        let (resolved, _) = resolve(&parsed, &library, "DP-1", None);
        let end = child_ids(group_of(&resolved, LayerKind::Top, "end"));
        assert!(
            matches!(
                panel_of(&resolved, "panel-battery"),
                Some(ResolvedAreaKind::Panel { owner, .. }) if end.contains(&owner.to_string())
            ),
            "{end:?}: {:?}",
            panel_of(&resolved, "panel-battery")
        );
    }

    /// Removing a panel's owner — the way `hogar-shell layout remove <id>` does it — removes the panel in the same transaction, and one undo brings both back; a redo takes both away again.
    #[test]
    fn removing_an_owner_removes_its_panel_and_one_undo_brings_both_back() {
        let dir = scratch("panel-owner");
        let mine = with_owners(&panel("panel-clock", "owner = \"clock-1\""));
        let mut store = store_with(&dir, &mine);
        let id = LayoutId::new("mine");
        let start = store.get(&id).expect("held").clone();
        let owner = InstanceId::new("clock-1");

        let at = crate::ops::placement_of(&start, &owner).expect("the owner is placed");
        store
            .commit(Transaction::new(
                "Remove `clock-1`",
                id.clone(),
                vec![LayoutOp::DeleteInstance {
                    spot: at.spot,
                    id: owner.clone(),
                }],
            ))
            .expect("the edit commits");
        let removed = store.get(&id).expect("held").clone();
        assert!(crate::ops::placement_of(&removed, &owner).is_none());
        assert_eq!(
            area_ids(&alone(&removed, "DP-1"), LayerKind::Top),
            ["bar-top", "corner"],
            "the panel went with its owner"
        );
        assert!(validate(&removed, &Modules).is_clean());

        assert_eq!(store.undo_label(), Some("Remove `clock-1`"));
        store.undo().expect("undo");
        assert_eq!(store.get(&id), Some(&start), "one undo brings both back");
        assert!(matches!(store.undo(), Err(StoreError::NothingToUndo)));
        store.redo().expect("redo");
        assert_eq!(store.get(&id), Some(&removed));
    }

    /// Whatever takes the owner out of the layout takes its panel: its group, its area, or a whole rule; an edit that keeps the owner somewhere keeps the panel.
    #[test]
    fn a_panel_goes_with_its_owner_however_the_owner_goes() {
        let mine = with_owners(&panel("panel-clock", "owner = \"clock-1\""));
        let top = Site::everywhere(LayerKind::Top);
        let has_panel = |layout: &Layout| {
            crate::ops::areas_at(layout, &top)
                .iter()
                .any(|area| area.id == AreaId::new("panel-clock"))
        };
        for ops in [
            vec![LayoutOp::DeleteGroup {
                site: top.clone(),
                area: AreaId::new("bar-top"),
                id: GroupId::new("start"),
            }],
            vec![LayoutOp::DeleteArea {
                site: top.clone(),
                id: AreaId::new("bar-top"),
            }],
        ] {
            let mut edited = mine.clone();
            let undo = crate::ops::apply_all(&mut edited, &ops).expect("applies");
            assert!(!has_panel(&edited), "{ops:?}");
            crate::ops::apply_all(&mut edited, &undo).expect("undoes");
            assert_eq!(edited, mine, "{ops:?}");
        }

        let mut moved = mine.clone();
        crate::ops::apply_all(
            &mut moved,
            &[LayoutOp::MoveInstance {
                from: top_zone_of("start"),
                to: Spot {
                    site: top.clone(),
                    area: AreaId::new("corner"),
                    group: GroupId::new("held"),
                },
                id: InstanceId::new("clock-1"),
                index: 0,
            }],
        )
        .expect("moves");
        assert!(has_panel(&moved), "the owner is still placed");

        let mut replaced = mine.clone();
        crate::ops::apply_all(
            &mut replaced,
            &[
                LayoutOp::DeleteInstance {
                    spot: top_zone_of("start"),
                    id: InstanceId::new("clock-1"),
                },
                LayoutOp::InsertInstance {
                    spot: top_zone_of("start"),
                    index: 0,
                    instance: Box::new(instance("clock-1", "clock")),
                },
            ],
        )
        .expect("applies");
        assert!(
            has_panel(&replaced),
            "an owner put back in the same transaction keeps its panel"
        );
    }

    const KEYS_BASE: &str = r##"
        id = "keys"
        [[outputs]]
        match = "*"
        [[outputs.layers.top.areas]]
        id = "bar"
        kind = "bar"
        edge = "top"
        thickness = 32
        shape = { radius = 8, gap = 4 }
        reserve = false
        style = { fill = "base", border = { width = 1.0 } }
        actions = { press = ["panel toggle clock"] }
        [[outputs.layers.top.areas.groups]]
        id = "start"
        place = "zone"
        zone = "start"
        arrange = "pages"
        gap = 6
        [[outputs.layers.top.areas.groups.children]]
        id = "clock"
        module = "clock"
        weight = 2.0
        options = { face = { scale = 1.5 }, hidden = ["a", "b"] }
        [[outputs.layers.top.areas.groups.children]]
        id = "battery"
        module = "battery"
        [[outputs.layers.desktop.areas]]
        id = "grid"
        kind = "grid"
        cell = 80
        [[outputs.layers.background.areas]]
        id = "paint"
        kind = "texture"
        gradient = { angle = 90, stops = [{ at = 0.0, color = "base" }, { at = 1.0, color = "surface" }] }

        [[outputs]]
        match = "DP-1"
        [[outputs.layers.top.areas]]
        id = "bar"
        kind = "bar"
        thickness = 40
        style = { border = { color = "red" } }
        [[outputs.layers.top.areas.groups]]
        id = "start"
        [[outputs.layers.top.areas.groups.children]]
        id = "clock"
        options = { face = { hands = true } }
        [[outputs.layers.desktop.areas]]
        id = "grid"
        kind = "free"
        rect = { x = 0.1, y = 0.1, w = 0.5, h = 0.5 }
        [[outputs.layers.background.areas]]
        id = "paint"
        kind = "texture"
        image = "paint.png"
        [[outputs.workspaces]]
        match = "2"
        [[outputs.workspaces.layers.top.areas]]
        id = "bar"
        style = { fill = "surface" }

        [[outputs]]
        match = "HDMI-*"
        [[outputs.layers.top.areas]]
        id = "bar"
        [[outputs.layers.top.areas.groups]]
        id = "start"
        unset = ["arrange"]
        remove = ["clock"]
        [[outputs.layers.top.areas.groups.children]]
        id = "battery"
        options = { face = 3 }
    "##;

    fn level(output: &str, workspace: Option<&str>) -> Level {
        Level {
            layout: LayoutId::new("keys"),
            output: OutputMatch(output.to_string()),
            workspace: workspace.map(|workspace| WorkspaceMatch(workspace.to_string())),
        }
    }

    fn keyed(output: &str, workspace: Option<&str>) -> Resolved {
        let active = workspace.map(|name| ActiveWorkspace {
            name: name.to_string(),
            ..ActiveWorkspace::default()
        });
        resolve(
            &layout(KEYS_BASE),
            &Library::default(),
            output,
            active.as_ref(),
        )
        .0
    }

    /// Resolution keeps, beside every key it kept and not only the expressions, the level that wrote it: geometry, a bar's shape and a border key by key, flags, actions, a group's arrangement, a child's placement and its options down to the key a level merged.
    #[test]
    fn every_key_a_level_writes_names_that_level() {
        let resolved = keyed("DP-1", None);
        let (bar, start, clock) = (
            AreaId::new("bar"),
            GroupId::new("start"),
            InstanceId::new("clock"),
        );
        let area = |key: &str| {
            resolved
                .origin(LayerKind::Top, Holder::Area(&bar), key)
                .cloned()
        };
        let (every, screen) = (Some(level("*", None)), Some(level("DP-1", None)));
        assert_eq!(area("kind"), screen);
        assert_eq!(area("thickness"), screen);
        assert_eq!(area("edge"), every, "a key the narrower level leaves out");
        assert_eq!(area("shape.radius"), every);
        assert_eq!(area("shape.gap"), every);
        assert_eq!(area("reserve"), every);
        assert_eq!(area("style.fill"), every);
        assert_eq!(area("style.border.width"), every);
        assert_eq!(area("style.border.color"), screen);
        assert_eq!(area("actions.press"), every);
        assert_eq!(
            area("above_fullscreen"),
            None,
            "nobody writes it: a default"
        );
        assert_eq!(area("style.opacity"), None);

        let group = |key: &str| {
            resolved
                .origin(LayerKind::Top, Holder::Group(&bar, &start), key)
                .cloned()
        };
        assert_eq!(group("arrange"), every);
        assert_eq!(group("gap"), every);
        assert_eq!(group("cols"), None);

        let child = |key: &str| {
            resolved
                .origin(LayerKind::Top, Holder::Instance(&bar, &start, &clock), key)
                .cloned()
        };
        assert_eq!(child("module"), every);
        assert_eq!(child("weight"), every);
        assert_eq!(child("options.face.scale"), every);
        assert_eq!(
            child("options.face.hands"),
            screen,
            "options merge key by key"
        );
        assert_eq!(
            child("options.hidden"),
            every,
            "a list is one value a level writes whole"
        );
        assert_eq!(child("options.hidden.0"), every, "and so is what is in it");
        assert_eq!(child("cell"), None);

        let on_two = keyed("DP-1", Some("2"));
        assert_eq!(
            on_two
                .origin(LayerKind::Top, Holder::Area(&bar), "style.fill")
                .cloned(),
            Some(level("DP-1", Some("2"))),
            "the workspace rule up on the screen comes last"
        );
    }

    /// A level that replaces what it writes over takes the keys of what it replaced with it: geometry of another kind, a texture's picture in place of its gradient, an `unset` of the arrangement, an option written over a table, and a child taken away.
    #[test]
    fn what_a_level_replaces_keeps_no_origin() {
        let resolved = keyed("DP-1", None);
        let grid = AreaId::new("grid");
        let at =
            |id: &AreaId, layer, key: &str| resolved.origin(layer, Holder::Area(id), key).cloned();
        assert_eq!(
            at(&grid, LayerKind::Desktop, "cell"),
            None,
            "a grid's cell went with the grid"
        );
        assert_eq!(
            at(&grid, LayerKind::Desktop, "rect"),
            Some(level("DP-1", None))
        );
        let paint = AreaId::new("paint");
        assert_eq!(at(&paint, LayerKind::Background, "gradient"), None);
        assert_eq!(
            at(&paint, LayerKind::Background, "image"),
            Some(level("DP-1", None))
        );

        let resolved = keyed("HDMI-A-1", None);
        let (bar, start) = (AreaId::new("bar"), GroupId::new("start"));
        let group = |key: &str| {
            resolved
                .origin(LayerKind::Top, Holder::Group(&bar, &start), key)
                .cloned()
        };
        assert_eq!(group("arrange"), None, "taken back");
        assert_eq!(group("gap"), None, "with the arrangement");
        let clock = InstanceId::new("clock");
        assert_eq!(
            resolved
                .origin(
                    LayerKind::Top,
                    Holder::Instance(&bar, &start, &clock),
                    "module"
                )
                .cloned(),
            None,
            "a child taken away takes its keys"
        );
        let battery = InstanceId::new("battery");
        let child = |key: &str| {
            resolved
                .origin(
                    LayerKind::Top,
                    Holder::Instance(&bar, &start, &battery),
                    key,
                )
                .cloned()
        };
        assert_eq!(child("options.face"), Some(level("HDMI-*", None)));
        assert_eq!(child("module"), Some(level("*", None)));
    }

    /// Who wrote a key is not part of what is drawn: an arrangement assembled directly has none, and two arrangements that draw the same are equal whoever wrote them.
    #[test]
    fn origins_do_not_make_two_arrangements_differ() {
        let resolved = keyed("DP-1", None);
        let assembled = Resolved::of("DP-1", resolved.layers.clone());
        assert_eq!(
            assembled.origin(
                LayerKind::Top,
                Holder::Area(&AreaId::new("bar")),
                "thickness"
            ),
            None
        );
        assert_eq!(assembled.layers, resolved.layers);
    }
}
