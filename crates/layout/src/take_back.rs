//! Where taking an expression back is written: at a level laid over every level that writes it on the screens the edit is for (DEC-26, TA-2), so the `unset` it writes takes back what is drawn there rather than something under it.

use std::collections::BTreeMap;

use util::report::Message;

use crate::model::*;
use crate::ops::Site;
use crate::resolve::{Origin, Resolved, resolve};

/// The expression taken back, and the area it is written under.
#[derive(Clone, Copy, Debug)]
pub struct Taken<'a> {
    pub layer: LayerKind,
    pub area: &'a AreaId,
    pub held: Held<'a>,
}

/// Which expression under an area is taken back.
#[derive(Clone, Copy, Debug)]
pub enum Held<'a> {
    /// The area's `visible`.
    Visible,
    /// The `repeat` of one of its groups.
    Repeat(&'a GroupId),
    /// The binding at `path` of an instance in one of its groups.
    Binding {
        group: &'a GroupId,
        instance: &'a InstanceId,
        path: &'a str,
    },
}

impl Taken<'_> {
    fn origin_in(&self, resolved: &Resolved) -> Option<Origin> {
        let area = resolved.area(self.layer, self.area)?;
        let group = |id: &GroupId| area.groups.iter().find(|held| held.id == *id);
        let expr = match self.held {
            Held::Visible => area.visible.as_ref(),
            Held::Repeat(id) => group(id)?.repeat.as_ref(),
            Held::Binding {
                group: id,
                instance,
                path,
            } => group(id)?
                .children
                .iter()
                .find(|held| held.id == *instance)?
                .bindings
                .get(path),
        };
        expr.map(|expr| expr.origin.clone())
    }

    /// Whether `layer` writes the expression itself.
    fn written_in(&self, layer: &Layer) -> bool {
        let Some(area) = layer.areas.iter().find(|held| held.id == *self.area) else {
            return false;
        };
        let group = |id: &GroupId| area.groups.iter().find(|held| held.id == *id);
        match self.held {
            Held::Visible => area.visible.is_some(),
            Held::Repeat(id) => group(id).is_some_and(|group| group.repeat.is_some()),
            Held::Binding {
                group: id,
                instance,
                path,
            } => group(id).is_some_and(|group| {
                group
                    .children
                    .iter()
                    .any(|held| held.id == *instance && held.bindings.contains_key(path))
            }),
        }
    }

    /// Deletes the expression `layer` writes, and names it in the `unset` of its holder there when `unset` — an entry naming only the ids on the way, where the layer writes none.
    fn take_back_in(&self, layer: &mut Layer, unset: bool) {
        let Some(area) = entry(&mut layer.areas, self.area, unset, |id| Area {
            id: id.clone(),
            ..Area::default()
        }) else {
            return;
        };
        let (group, instance, path) = match self.held {
            Held::Visible => {
                area.visible = None;
                return mark(&mut area.unset, Unset::Visible, unset);
            }
            Held::Repeat(id) => {
                if let Some(group) = group_entry(area, id, unset) {
                    group.repeat = None;
                    mark(&mut group.unset, Unset::Repeat, unset);
                }
                return;
            }
            Held::Binding {
                group,
                instance,
                path,
            } => (group, instance, path),
        };
        let Some(group) = group_entry(area, group, unset) else {
            return;
        };
        let Some(instance) = entry(&mut group.children, instance, unset, |id| Instance {
            id: id.clone(),
            ..Instance::default()
        }) else {
            return;
        };
        instance.bindings.remove(path);
        mark(&mut instance.unset, Unset::binding(path), unset);
    }

    fn name(&self) -> Message {
        match self.held {
            Held::Visible => util::message!("finding.held.visible", area = self.area),
            Held::Repeat(group) => {
                util::message!("finding.held.repeat", area = self.area, group = group)
            }
            Held::Binding { instance, path, .. } => {
                util::message!("finding.held.binding", path = path, instance = instance)
            }
        }
    }
}

/// What `id` names in `held`, made as an entry naming only that id when `make` is asked for and there is none.
fn entry<'a, T, Id: PartialEq>(
    held: &'a mut Vec<T>,
    id: &Id,
    make: bool,
    new: impl FnOnce(&Id) -> T,
) -> Option<&'a mut T>
where
    T: Identified<Id>,
{
    let at = match held.iter().position(|it| it.identity() == id) {
        Some(at) => at,
        None if make => {
            held.push(new(id));
            held.len() - 1
        }
        None => return None,
    };
    Some(&mut held[at])
}

fn group_entry<'a>(area: &'a mut Area, id: &GroupId, make: bool) -> Option<&'a mut Group> {
    entry(&mut area.groups, id, make, |id| Group {
        id: id.clone(),
        ..Group::default()
    })
}

fn mark(unsets: &mut Vec<Unset>, unset: Unset, taking: bool) {
    if taking && !unsets.contains(&unset) {
        unsets.push(unset);
    }
}

/// What an entry of a level is found by.
trait Identified<Id> {
    fn identity(&self) -> &Id;
}

impl Identified<AreaId> for Area {
    fn identity(&self) -> &AreaId {
        &self.id
    }
}

impl Identified<GroupId> for Group {
    fn identity(&self) -> &GroupId {
        &self.id
    }
}

impl Identified<InstanceId> for Instance {
    fn identity(&self) -> &InstanceId {
        &self.id
    }
}

/// Where taking an expression back is written.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeBack {
    /// The output rule written in.
    pub site: Site,
    /// Whether that rule writes the expression itself, which is deleted there.
    pub own: bool,
    /// Whether that rule names the expression in its `unset`, for a level under it that still writes it.
    pub unset: bool,
}

/// Where `layout` takes `taken` back on every screen called one of `screens`, on every workspace: in the narrowest of its output rules that speaks for all of them, where what that rule writes itself is deleted and the expression is named in its `unset` while a level under it still gives one. `known` holds the layouts it may extend.
///
/// Refused when nothing gives the expression on any of the screens, when no rule of the layout speaks for all of them, and when a level that rule is not laid over would still give it there — a narrower rule, or one of the layout's workspace rules, which come after every output rule. The refusal names that level, which is where it can be deleted instead.
pub fn taking_back(
    layout: &Layout,
    known: &BTreeMap<LayoutId, Layout>,
    screens: &[String],
    taken: Taken<'_>,
) -> Result<TakeBack, Message> {
    let named = screen_names(screens);
    let given = |layout: &Layout| {
        screens.iter().find_map(|screen| {
            let (resolved, _) = resolve(layout, known, screen, None);
            taken.origin_in(&resolved)
        })
    };
    let ruled = screens.iter().find_map(|screen| {
        layout
            .outputs
            .iter()
            .filter(|rule| rule.matches.matches(screen))
            .find_map(|rule| {
                let workspace = rule.workspaces.iter().find(|workspace| {
                    workspace
                        .layers
                        .get(taken.layer)
                        .is_some_and(|layer| taken.written_in(layer))
                })?;
                Some(Origin {
                    layout: layout.id.clone(),
                    output: rule.matches.clone(),
                    workspace: Some(workspace.matches.clone()),
                })
            })
    });
    if given(layout).is_none() && ruled.is_none() {
        return Err(util::message!(
            "finding.nothing_to_take_back",
            held = taken.name(),
            screens = &named
        ));
    }
    let narrowest = layout
        .outputs
        .iter()
        .filter(|rule| screens.iter().all(|screen| rule.matches.matches(screen)))
        .max_by_key(|rule| rule.matches.specificity())
        .ok_or_else(|| {
            util::message!(
                "finding.nowhere_to_take_back",
                screens = &named,
                held = taken.name()
            )
        })?;
    // An edit addresses a rule by its `match`, which is the first rule of the file written with it.
    let at = layout
        .outputs
        .iter()
        .position(|rule| rule.matches == narrowest.matches)
        .expect("the narrowest rule is one of the layout's");
    let own = taken.written_in(layout.outputs[at].layers.get(taken.layer));
    let mut after = layout.clone();
    taken.take_back_in(after.outputs[at].layers.get_mut(taken.layer), false);
    let unset = given(&after).is_some();
    taken.take_back_in(after.outputs[at].layers.get_mut(taken.layer), unset);
    if let Some(writer) = ruled.or_else(|| given(&after)) {
        return Err(util::message!(
            "finding.taken_back_elsewhere",
            held = taken.name(),
            rule = writer.rule(),
            file = writer.file(),
            screens = &named,
            narrowest = &narrowest.matches.0
        ));
    }
    Ok(TakeBack {
        site: Site {
            output: narrowest.matches.clone(),
            workspace: None,
            layer: taken.layer,
        },
        own,
        unset,
    })
}

fn screen_names(screens: &[String]) -> Message {
    match screens {
        [every] if every == "*" => util::message!("finding.every_output"),
        _ => Message::verbatim(
            screens
                .iter()
                .map(|screen| format!("`{screen}`"))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}
