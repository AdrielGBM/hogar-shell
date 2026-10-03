//! Where a change is written: in place where the narrowest rule writes the area, as a partial entry where it inherits it, and into a workspace's rule made for it.

#[cfg(test)]
mod tests {
    use layout::{
        Area, AreaId, AreaKind, Expr, GroupId, InstanceId, LayerKind, Layout, LayoutId, LayoutOp,
        OutputMatch, OutputRule, Site, WorkspaceMatch,
    };

    use crate::written::Written;

    fn bar() -> AreaId {
        AreaId::new("bar-top")
    }

    fn thicker(area: &Area) -> Area {
        let mut changed = area.clone();
        if let Some(AreaKind::Bar { thickness, .. }) = &mut changed.kind {
            *thickness = Some(50.0);
        }
        changed
    }

    /// The built-in layout writes its bar in the rule for every screen, so that is where a change to it goes, in place.
    #[test]
    fn an_area_the_layout_writes_is_changed_where_it_is_written() {
        let layout = layout::built_in();
        let written =
            Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar(), None).expect("the bar");
        assert!(written.is_present());
        assert_eq!(written.site, Site::everywhere(LayerKind::Top));
        assert!(matches!(
            written.ops(&thicker(&written.area)).as_slice(),
            [LayoutOp::ReplaceArea { .. }]
        ));
    }

    /// A layout that only extends another writes a partial entry in the narrowest rule covering the screen, after everything already in that layer.
    #[test]
    fn an_inherited_area_is_overridden_by_a_partial_entry_in_the_narrowest_rule() {
        let mut layout = Layout {
            id: LayoutId::new("mine"),
            extends: Some(LayoutId::new("default")),
            ..Layout::default()
        };
        for pattern in ["*", "DP-*", "HDMI-A-1"] {
            layout.outputs.push(OutputRule {
                matches: OutputMatch(pattern.to_string()),
                ..OutputRule::default()
            });
        }
        let written = Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar(), None)
            .expect("a rule covers it");
        assert!(!written.is_present());
        assert_eq!(written.site.output, OutputMatch("DP-*".to_string()));
        assert_eq!(
            written.area,
            Area {
                id: bar(),
                ..Area::default()
            }
        );
        assert!(matches!(
            written.ops(&written.area).as_slice(),
            [LayoutOp::InsertArea { index: 0, .. }]
        ));
    }

    #[test]
    fn a_screen_no_rule_covers_is_refused() {
        let layout = Layout::default();
        assert!(Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar(), None).is_err());
    }

    /// An instance the rule writes is set on its own; one it inherits is laid over the inherited one through its area.
    #[test]
    fn an_instance_is_set_alone_where_written_and_through_its_area_where_inherited() {
        let layout = layout::built_in();
        let written =
            Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar(), None).expect("the bar");
        let clock = written.instance(&GroupId::new("center"), &InstanceId::new("clock"));
        assert!(matches!(
            clock.ops(&clock.instance).as_slice(),
            [LayoutOp::SetInstance { .. }]
        ));
        let elsewhere = written.instance(&GroupId::new("center"), &InstanceId::new("nothing"));
        let ops = elsewhere.ops(&elsewhere.instance);
        let [LayoutOp::ReplaceArea { area, .. }] = ops.as_slice() else {
            panic!("an inherited instance is written through its area");
        };
        assert!(area.groups.iter().any(|group| {
            group
                .children
                .iter()
                .any(|child| child.id.as_str() == "nothing")
        }));
    }

    /// An expression is part of what is written: a binding on an instance and an area's `visible` each come out of the operations that write the change, and the operations that undo them take each back exactly — which is what puts an expression edit in the history.
    #[test]
    fn a_binding_and_a_visible_are_written_and_undone_with_their_instance_and_area() {
        let before = layout::built_in();
        let written =
            Written::area(&before, Some("DP-1"), LayerKind::Top, &bar(), None).expect("the bar");
        let clock = written.instance(&GroupId::new("center"), &InstanceId::new("clock"));
        let mut bound = clock.instance.clone();
        bound.bindings.insert(
            "accent".to_string(),
            Expr("if($battery.level < 20, #f00, #0f0)".to_string()),
        );
        let mut shown_while = written.area.clone();
        shown_while.visible = Some(Expr("$battery.level < 20".to_string()));

        for ops in [clock.ops(&bound), written.ops(&shown_while)] {
            assert!(!ops.is_empty(), "the change is written");
            let mut after = before.clone();
            let undo = layout::ops::apply_all(&mut after, &ops).expect("it applies");
            assert_ne!(after, before);
            layout::ops::apply_all(&mut after, &undo).expect("and is undone");
            assert_eq!(after, before, "undo takes the expression back exactly");
        }
        let mut after = before.clone();
        layout::ops::apply_all(&mut after, &clock.ops(&bound)).expect("it applies");
        let again = Written::area(&after, Some("DP-1"), LayerKind::Top, &bar(), None)
            .expect("the bar")
            .instance(&GroupId::new("center"), &InstanceId::new("clock"));
        assert_eq!(again.instance.bindings, bound.bindings);
    }

    /// A change for one workspace goes into that workspace's rule of the narrowest output rule, which the first such change makes; once the rule writes the area, the next change is made to it in place.
    #[test]
    fn a_change_for_one_workspace_makes_its_rule_and_then_edits_it_in_place() {
        let mut layout = layout::built_in();
        let second = WorkspaceMatch("2".to_string());
        let written = Written::area(
            &layout,
            Some("DP-1"),
            LayerKind::Background,
            &AreaId::new("background"),
            Some(&second),
        )
        .expect("the output rule covers the screen");
        assert!(!written.is_present());
        assert_eq!(
            written.site,
            Site::everywhere(LayerKind::Background).on_workspace("2")
        );
        let ops = written.ops(&written.area);
        assert!(matches!(
            ops.as_slice(),
            [
                LayoutOp::InsertWorkspaceRule { index: 0, .. },
                LayoutOp::InsertArea { index: 0, .. }
            ]
        ));
        layout::ops::apply_all(&mut layout, &ops).expect("the rule and its entry apply");

        let again = Written::area(
            &layout,
            Some("DP-1"),
            LayerKind::Background,
            &AreaId::new("background"),
            Some(&second),
        )
        .expect("the rule is there now");
        assert!(again.is_present());
        assert!(matches!(
            again.ops(&again.area).as_slice(),
            [LayoutOp::ReplaceArea { .. }]
        ));
        assert!(
            Written::area(
                &layout,
                Some("DP-1"),
                LayerKind::Lock,
                &AreaId::new("prompt"),
                Some(&second)
            )
            .is_err(),
            "no workspace is visible while the screen is locked"
        );
    }
}
