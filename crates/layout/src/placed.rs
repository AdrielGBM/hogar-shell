//! Every id a layout places, which is what an edit can take away from it.
//!
//! State kept by id outlives the node that showed it — an instance's store, the child a stack is showing, the picture a wallpaper region last faded to — so something has to say when an id is gone for good. That is a question about the layout rather than about any one output: an instance on a monitor that is unplugged right now is still placed, and one written only for another workspace still exists.

use std::collections::{BTreeMap, BTreeSet};

use util::report::Report;

use crate::model::*;
use crate::resolve::chain_of;

/// The areas, groups and instances a layout places at any level — its `extends` chain, every output rule and every workspace rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub areas: BTreeSet<AreaId>,
    /// A group by the area it is in: group ids are unique within their area only.
    pub groups: BTreeSet<(AreaId, GroupId)>,
    pub instances: BTreeSet<InstanceId>,
}

impl Placed {
    pub fn of(layout: &Layout, known: &BTreeMap<LayoutId, Layout>) -> Self {
        let mut placed = Self::default();
        for level in chain_of(layout, known, &mut Report::default()) {
            for rule in &level.outputs {
                let own = rule.layers.each().map(|(_, layer)| layer);
                let ruled = rule
                    .workspaces
                    .iter()
                    .flat_map(|workspace| workspace.layers.each().map(|(_, layer)| layer));
                for layer in own.into_iter().chain(ruled) {
                    placed.add(layer);
                }
            }
        }
        placed
    }

    fn add(&mut self, layer: &Layer) {
        for area in &layer.areas {
            self.areas.insert(area.id.clone());
            for group in &area.groups {
                self.groups.insert((area.id.clone(), group.id.clone()));
                self.instances
                    .extend(group.children.iter().map(|child| child.id.clone()));
            }
        }
    }

    /// What this places that `now` no longer does.
    pub fn gone(&self, now: &Placed) -> Placed {
        Placed {
            areas: self.areas.difference(&now.areas).cloned().collect(),
            groups: self.groups.difference(&now.groups).cloned().collect(),
            instances: self.instances.difference(&now.instances).cloned().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.areas.is_empty() && self.groups.is_empty() && self.instances.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{LayoutOp, Site, Spot, apply};

    fn clock_spot() -> Spot {
        Spot {
            site: Site::everywhere(LayerKind::Top),
            area: AreaId::new("bar-top"),
            group: GroupId::new("center"),
        }
    }

    #[test]
    fn a_removed_instance_is_gone_and_a_moved_one_is_not() {
        let known = BTreeMap::new();
        let before = crate::built_in::layout();
        let mut after = before.clone();
        apply(
            &mut after,
            &LayoutOp::MoveInstance {
                from: clock_spot(),
                to: Spot {
                    group: GroupId::new("end"),
                    ..clock_spot()
                },
                id: InstanceId::new("clock"),
                index: 0,
            },
        )
        .expect("the clock moves");
        assert!(
            Placed::of(&before, &known)
                .gone(&Placed::of(&after, &known))
                .is_empty(),
            "a move keeps every id"
        );

        apply(
            &mut after,
            &LayoutOp::DeleteGroup {
                site: Site::everywhere(LayerKind::Top),
                area: AreaId::new("bar-top"),
                id: GroupId::new("start"),
            },
        )
        .expect("the start zone goes");
        let gone = Placed::of(&before, &known).gone(&Placed::of(&after, &known));
        assert_eq!(
            gone.instances,
            BTreeSet::from([InstanceId::new("workspaces")])
        );
        assert_eq!(
            gone.groups,
            BTreeSet::from([(AreaId::new("bar-top"), GroupId::new("start"))])
        );
        assert!(gone.areas.is_empty());
    }

    #[test]
    fn what_a_parent_layout_still_places_is_not_gone() {
        let parent = crate::built_in::layout();
        let mut known = BTreeMap::new();
        known.insert(parent.id.clone(), parent.clone());
        let child = Layout {
            id: LayoutId::new("child"),
            extends: Some(parent.id.clone()),
            outputs: parent.outputs.clone(),
            ..Layout::default()
        };
        let mut emptied = child.clone();
        emptied.outputs.clear();
        assert!(
            Placed::of(&child, &known)
                .gone(&Placed::of(&emptied, &known))
                .is_empty()
        );
    }
}
