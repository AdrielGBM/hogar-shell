//! Putting a part of a layout back to what the layout it extends says, or the built-in one when it extends none.
//!
//! **Something the layout no longer has is still a reset**, because a reset a user reaches for after removing the bar is exactly the one that has to put it back — so a target missing from the layout is looked for in its base too, and only an id neither of them knows is nothing to reset.
//!
//! **The output and workspace rules a layout writes are left where they are**: they are the shape of the file rather than something placed in it. What their layers hold is reset like the rest, and a base with no rule of the same match has nothing to say about one, so what such a rule placed is taken away.
//!
//! **The lock screen's prompt is put back, never taken away** (TA-8): it is replaced by the base's where it stands, and one the base has no counterpart for is left as it is.

use util::report::Report;

use crate::library::Library;
use crate::merge::merge_layer;
use crate::model::*;
use crate::ops::{LayoutOp, Placement, Site, areas_at, placement_of, site_of_area, sites, spot_of};

/// What a reset puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// Every layer of every rule.
    All,
    Layer(LayerKind),
    Area(&'a AreaId),
    Instance(&'a InstanceId),
}

/// What `layout` is reset to: the layouts it extends, laid over each other root first, or the built-in layout when it extends none (or one that is not there).
pub fn base_of(layout: &Layout, known: &Library) -> Layout {
    let Some(parent) = layout.extends.as_ref().and_then(|id| known.layout(id)) else {
        return crate::built_in();
    };
    let mut flat = Layout {
        id: parent.id.clone(),
        ..Layout::default()
    };
    for level in crate::resolve::chain_of(parent, known, &mut Report::default()) {
        for rule in &level.outputs {
            lay_rule(&mut flat, rule);
        }
    }
    flat
}

/// Lays one level's rule over the rule of the same match in `flat`, or adds it where `flat` has none.
fn lay_rule(flat: &mut Layout, rule: &OutputRule) {
    let Some(held) = flat
        .outputs
        .iter_mut()
        .find(|held| held.matches == rule.matches)
    else {
        flat.outputs.push(rule.clone());
        return;
    };
    for kind in LayerKind::ALL {
        merge_layer(held.layers.get_mut(kind), rule.layers.get(kind));
    }
    for workspace in &rule.workspaces {
        match held
            .workspaces
            .iter_mut()
            .find(|it| it.matches == workspace.matches)
        {
            Some(it) => {
                for kind in LayerKind::SESSION {
                    if let (Some(into), Some(over)) =
                        (it.layers.get_mut(kind), workspace.layers.get(kind))
                    {
                        merge_layer(into, over);
                    }
                }
            }
            None => held.workspaces.push(workspace.clone()),
        }
    }
}

/// The operations that put `target` back as `base` has it, or `None` when neither `layout` nor `base` knows it.
pub fn ops(layout: &Layout, base: &Layout, target: Target<'_>) -> Option<Vec<LayoutOp>> {
    match target {
        Target::All => Some(refill(layout, base, &LayerKind::ALL)),
        Target::Layer(layer) => Some(refill(layout, base, &[layer])),
        Target::Area(id) => {
            let site = site_of_area(layout, id).or_else(|| site_of_area(base, id))?;
            Some(area(layout, base, &site, id))
        }
        Target::Instance(id) => {
            let at = placement_of(layout, id).or_else(|| placement_of(base, id))?;
            Some(instance(layout, base, &at, id))
        }
    }
}

/// Empties the named layers of every rule and fills them again from the base's rule of the same match, which has nothing to say about a rule the user added — so that rule's areas are taken away rather than replaced.
fn refill(layout: &Layout, base: &Layout, layers: &[LayerKind]) -> Vec<LayoutOp> {
    let mut ops = Vec::new();
    for (site, layer) in sites(layout).filter(|(site, _)| layers.contains(&site.layer)) {
        let (prompts, rest): (Vec<&Area>, Vec<&Area>) =
            layer.areas.iter().partition(|area| is_prompt(&site, area));
        for area in rest {
            ops.push(LayoutOp::DeleteArea {
                site: site.clone(),
                id: area.id.clone(),
            });
        }
        let wanted = areas_at(base, &site);
        let replacement = prompts
            .first()
            .and_then(|_| wanted.iter().position(|area| is_prompt(&site, area)));
        // Inserted in front of the prompts that stay, which is where the base stacks its own: under the prompt.
        for (index, area) in wanted
            .iter()
            .enumerate()
            .filter(|(at, _)| Some(*at) != replacement)
            .map(|(_, area)| area)
            .enumerate()
        {
            ops.push(LayoutOp::InsertArea {
                site: site.clone(),
                index,
                area: Box::new(area.clone()),
            });
        }
        if let (Some(prompt), Some(at)) = (prompts.first(), replacement) {
            ops.push(LayoutOp::ReplaceArea {
                site: site.clone(),
                id: prompt.id.clone(),
                area: Box::new(wanted[at].clone()),
            });
        }
    }
    ops
}

/// Puts the base's area where this one stands — or does only half of that, when the layout no longer has it or the base never did.
fn area(layout: &Layout, base: &Layout, site: &Site, id: &AreaId) -> Vec<LayoutOp> {
    let here = areas_at(layout, site);
    let at = here.iter().position(|area| &area.id == id);
    let there = areas_at(base, site);
    let based_at = there.iter().position(|area| &area.id == id);
    match (at, based_at) {
        (Some(_), Some(from)) => vec![LayoutOp::ReplaceArea {
            site: site.clone(),
            id: id.clone(),
            area: Box::new(there[from].clone()),
        }],
        (Some(_), None) => vec![LayoutOp::DeleteArea {
            site: site.clone(),
            id: id.clone(),
        }],
        (None, Some(from)) => vec![LayoutOp::InsertArea {
            site: site.clone(),
            index: from.min(here.len()),
            area: Box::new(there[from].clone()),
        }],
        (None, None) => Vec::new(),
    }
}

/// The same for one placed instance. `at` is wherever it was found — in the layout, or in the base when the layout has taken it out.
fn instance(layout: &Layout, base: &Layout, at: &Placement, id: &InstanceId) -> Vec<LayoutOp> {
    let here = placement_of(layout, id);
    let mut ops = Vec::new();
    if let Some(here) = &here {
        ops.push(LayoutOp::DeleteInstance {
            spot: here.spot.clone(),
            id: id.clone(),
        });
    }
    if let Some(found) = placement_of(base, id)
        && let Some(group) = spot_of(base, &found.spot)
    {
        let room = spot_of(layout, &at.spot).map_or(0, |group| group.children.len());
        ops.push(LayoutOp::InsertInstance {
            spot: at.spot.clone(),
            index: match &here {
                Some(here) => here.index,
                None => found.index.min(room),
            },
            instance: Box::new(group.children[found.index].clone()),
        });
    }
    ops
}

/// Whether `area` is the lock screen's prompt, which a reset may replace but never take away.
fn is_prompt(site: &Site, area: &Area) -> bool {
    site.layer == LayerKind::Lock && matches!(area.kind, Some(AreaKind::Prompt { .. }))
}
