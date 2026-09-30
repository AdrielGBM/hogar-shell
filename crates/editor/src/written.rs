//! Where a change to something on screen is written in the layout being edited.
//!
//! What a screen shows is every output rule of the edited layout that matches it, broadest first, laid over whatever that layout extends (F-10.1). A change is written into the narrowest of those rules that already writes the area, which is where the value it replaces came from. An area only a layout it extends writes gets a partial entry in the narrowest rule that covers the screen, naming just what changed, so the change lies over the inherited area rather than copying the rest of it.

use layout::{
    Area, AreaId, Group, GroupId, Instance, InstanceId, LayerKind, Layout, LayoutOp, OutputRule,
    Site, Spot,
};

/// An area as the rule that decides it writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub site: Site,
    /// The area as that rule writes it; for one it does not write yet, an entry naming only its id.
    pub area: Area,
    /// Where a new entry goes when the rule does not write the area: after everything in its layer, so it overrides without moving anything (merging never restacks).
    insert_at: Option<usize>,
}

impl Written {
    /// Where the edited layout decides the area `id` on the layer `layer` of the screen `output`.
    pub fn area(
        layout: &Layout,
        output: Option<&str>,
        layer: LayerKind,
        id: &AreaId,
    ) -> Result<Self, String> {
        let screen = output.unwrap_or_default();
        let mut rules: Vec<&OutputRule> = layout
            .outputs
            .iter()
            .filter(|rule| rule.matches.matches(screen))
            .collect();
        rules.sort_by_key(|rule| rule.matches.specificity());
        let site = |rule: &OutputRule| Site {
            output: rule.matches.clone(),
            workspace: None,
            layer,
        };
        let writer = rules.iter().rev().find_map(|rule| {
            let area = rule
                .layers
                .get(layer)
                .areas
                .iter()
                .find(|area| area.id == *id)?;
            Some((*rule, area))
        });
        if let Some((rule, area)) = writer {
            return Ok(Self {
                site: site(rule),
                area: area.clone(),
                insert_at: None,
            });
        }
        let rule = rules
            .last()
            .ok_or_else(|| telar::t!("editor.popover.no_rule", output = screen))?;
        Ok(Self {
            site: site(rule),
            area: Area {
                id: id.clone(),
                ..Area::default()
            },
            insert_at: Some(rule.layers.get(layer).areas.len()),
        })
    }

    /// Whether the rule writes the area already, rather than inheriting it.
    pub fn is_present(&self) -> bool {
        self.insert_at.is_none()
    }

    /// The operation that writes `changed` where this area was read from.
    pub fn op(&self, changed: &Area) -> LayoutOp {
        match self.insert_at {
            None => LayoutOp::ReplaceArea {
                site: self.site.clone(),
                id: self.area.id.clone(),
                area: Box::new(changed.clone()),
            },
            Some(index) => LayoutOp::InsertArea {
                site: self.site.clone(),
                index,
                area: Box::new(changed.clone()),
            },
        }
    }

    /// The instance `id` of the group `group` as this area writes it; for one it does not write, an entry naming only its id.
    pub fn instance(&self, group: &GroupId, id: &InstanceId) -> WrittenInstance {
        let written = self
            .area
            .groups
            .iter()
            .find(|held| held.id == *group)
            .and_then(|held| held.children.iter().find(|child| child.id == *id));
        WrittenInstance {
            area: self.clone(),
            group: group.clone(),
            present: self.is_present() && written.is_some(),
            instance: written.cloned().unwrap_or_else(|| Instance {
                id: id.clone(),
                ..Instance::default()
            }),
        }
    }
}

/// An instance as the rule that decides it writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct WrittenInstance {
    pub area: Written,
    pub group: GroupId,
    pub instance: Instance,
    present: bool,
}

impl WrittenInstance {
    /// The operation that writes `changed` where this instance was read from: the instance alone where the rule writes it, else the entries that lay it over the inherited one.
    pub fn op(&self, changed: &Instance) -> LayoutOp {
        if self.present {
            return LayoutOp::SetInstance {
                spot: self.spot(),
                id: changed.id.clone(),
                instance: Box::new(changed.clone()),
            };
        }
        let mut area = self.area.area.clone();
        let group = match area.groups.iter().position(|held| held.id == self.group) {
            Some(at) => &mut area.groups[at],
            None => {
                area.groups.push(Group {
                    id: self.group.clone(),
                    ..Group::default()
                });
                area.groups.last_mut().expect("a group was just pushed")
            }
        };
        match group
            .children
            .iter_mut()
            .find(|child| child.id == changed.id)
        {
            Some(child) => *child = changed.clone(),
            None => group.children.push(changed.clone()),
        }
        self.area.op(&area)
    }

    /// The operation that takes this instance out: from where the rule writes it, else by naming it in its group's `remove` over the inherited one.
    pub fn removal(&self) -> LayoutOp {
        if self.present {
            return LayoutOp::DeleteInstance {
                spot: self.spot(),
                id: self.instance.id.clone(),
            };
        }
        let mut area = self.area.area.clone();
        match area.groups.iter_mut().find(|held| held.id == self.group) {
            Some(group) => group.remove.push(self.instance.id.clone()),
            None => area.groups.push(Group {
                id: self.group.clone(),
                remove: vec![self.instance.id.clone()],
                ..Group::default()
            }),
        }
        self.area.op(&area)
    }

    fn spot(&self) -> Spot {
        Spot {
            site: self.area.site.clone(),
            area: self.area.area.id.clone(),
            group: self.group.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use layout::{AreaKind, LayoutId, OutputMatch};

    use super::*;

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
            Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar()).expect("the bar");
        assert!(written.is_present());
        assert_eq!(written.site, Site::everywhere(LayerKind::Top));
        assert!(matches!(
            written.op(&thicker(&written.area)),
            LayoutOp::ReplaceArea { .. }
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
        let written =
            Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar()).expect("a rule covers it");
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
            written.op(&written.area),
            LayoutOp::InsertArea { index: 0, .. }
        ));
    }

    #[test]
    fn a_screen_no_rule_covers_is_refused() {
        let layout = Layout::default();
        assert!(Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar()).is_err());
    }

    /// An instance the rule writes is set on its own; one it inherits is laid over the inherited one through its area.
    #[test]
    fn an_instance_is_set_alone_where_written_and_through_its_area_where_inherited() {
        let layout = layout::built_in();
        let written =
            Written::area(&layout, Some("DP-1"), LayerKind::Top, &bar()).expect("the bar");
        let clock = written.instance(&GroupId::new("center"), &InstanceId::new("clock"));
        assert!(matches!(
            clock.op(&clock.instance),
            LayoutOp::SetInstance { .. }
        ));
        let elsewhere = written.instance(&GroupId::new("center"), &InstanceId::new("nothing"));
        let LayoutOp::ReplaceArea { area, .. } = elsewhere.op(&elsewhere.instance) else {
            panic!("an inherited instance is written through its area");
        };
        assert!(area.groups.iter().any(|group| {
            group
                .children
                .iter()
                .any(|child| child.id.as_str() == "nothing")
        }));
    }
}
