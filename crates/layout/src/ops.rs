//! The edits a layout can undergo, and how to take each one back.
//!
//! Every change to a layout — a drag in an edit mode, a value scrubbed in a popover, a line of `hogar-shell layout …` — becomes one of these. That is what lets a single undo stack cover all three: the stack holds operations, not a record of which part of the interface made them.
//!
//! [`apply`] returns the operation that undoes what it just did, built from the values it displaced. Undo is therefore exact rather than approximate: putting an instance back puts it back in the group and at the index it came from, with the id it always had, so a rule or an IPC command that addressed it still finds it. Redo is applying the inverse of the inverse, which [`apply`] produces in the same way.
//!
//! An operation that cannot be carried out — an area that is no longer there, an index past the end of a list — is an [`OpError`], never a panic and never a silent no-op. A caller that got one has a layout it did not expect, and the transaction it belongs to is abandoned whole rather than applied in part.
//!
//! Two of the model's rules are refused here as well as reported by validation, because both are about what an edit may *do* rather than what a file may say: the lock layer's prompt is never removed, turned into something else or hidden (TA-8), and a workspace rule never adds, removes or resizes what reserves space (TA-2). Validation catches a file written by hand; this is what keeps an edit from writing one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::model::*;
use crate::validate::reserving_under;

/// Which layer of which output rule an operation acts on — the rule's own, or one of its workspace rules'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub output: OutputMatch,
    /// A workspace rule of that output rule, by its `match`. `None` is the output rule's own layers.
    pub workspace: Option<WorkspaceMatch>,
    pub layer: LayerKind,
}

impl Site {
    pub fn new(output: impl Into<String>, layer: LayerKind) -> Self {
        Self {
            output: OutputMatch(output.into()),
            workspace: None,
            layer,
        }
    }

    /// The rule that speaks for every output, which is where an edit lands unless it was made for one monitor.
    pub fn everywhere(layer: LayerKind) -> Self {
        Self::new("*", layer)
    }

    /// The same layer, in one of this output rule's workspace rules.
    pub fn on_workspace(self, workspace: impl Into<String>) -> Self {
        Self {
            workspace: Some(WorkspaceMatch(workspace.into())),
            ..self
        }
    }
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outputs.{}", self.output.0)?;
        if let Some(workspace) = &self.workspace {
            write!(f, ".workspaces.{}", workspace.0)?;
        }
        write!(f, ".layers.{}", self.layer)
    }
}

/// The group an instance sits in, named in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    pub site: Site,
    pub area: AreaId,
    pub group: GroupId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutOp {
    InsertArea {
        site: Site,
        index: usize,
        area: Box<Area>,
    },
    DeleteArea {
        site: Site,
        id: AreaId,
    },
    /// Changes an area's z-order within its layer.
    MoveArea {
        site: Site,
        id: AreaId,
        index: usize,
    },
    /// Puts another area where one stands, at the same place in the z-order: a reset putting back what the built-in layout says. The replacement may carry another id.
    ReplaceArea {
        site: Site,
        id: AreaId,
        area: Box<Area>,
    },
    /// Replaces an area's geometry: a bar dragged to another edge, a thickness handle, a rectangle resized.
    SetAreaKind {
        site: Site,
        id: AreaId,
        kind: Box<Option<AreaKind>>,
    },
    SetAreaStyle {
        site: Site,
        id: AreaId,
        style: Box<AreaStyle>,
    },
    SetAreaFlags {
        site: Site,
        id: AreaId,
        reserve: Option<bool>,
        above_fullscreen: Option<bool>,
        visible: Option<Expr>,
    },
    /// Rebinds what the gestures on an area's own background run.
    SetAreaActions {
        site: Site,
        id: AreaId,
        actions: BTreeMap<Trigger, Action>,
    },
    InsertGroup {
        site: Site,
        area: AreaId,
        index: usize,
        group: Box<Group>,
    },
    DeleteGroup {
        site: Site,
        area: AreaId,
        id: GroupId,
    },
    SetGroupKind {
        site: Site,
        area: AreaId,
        id: GroupId,
        kind: Option<GroupKind>,
    },
    /// Turns "one at a time" on or off for a group, wherever it is placed: an instance dropped onto another, and the same stack taken apart again.
    SetGroupStacked {
        site: Site,
        area: AreaId,
        id: GroupId,
        stacked: Option<bool>,
    },
    InsertInstance {
        spot: Spot,
        index: usize,
        instance: Box<Instance>,
    },
    DeleteInstance {
        spot: Spot,
        id: InstanceId,
    },
    /// Takes an instance out of one group and puts it in another, keeping its id, options and state. A chip dragged between bars and a chip turned into a widget are both this.
    MoveInstance {
        from: Spot,
        to: Spot,
        id: InstanceId,
        index: usize,
    },
    /// Replaces everything about one instance but its id.
    SetInstance {
        spot: Spot,
        id: InstanceId,
        instance: Box<Instance>,
    },
    /// Adds an output rule, so the first edit made for one monitor has a level of its own to land in.
    InsertOutputRule {
        index: usize,
        rule: Box<OutputRule>,
    },
    DeleteOutputRule {
        output: OutputMatch,
    },
    /// Adds a workspace rule to an output rule, so the first edit made for one workspace has a level of its own to land in.
    InsertWorkspaceRule {
        output: OutputMatch,
        index: usize,
        rule: Box<WorkspaceRule>,
    },
    DeleteWorkspaceRule {
        output: OutputMatch,
        workspace: WorkspaceMatch,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpError {
    NoOutputRule(String),
    NoWorkspaceRule {
        output: String,
        workspace: String,
    },
    /// A workspace rule has no lock layer, since no workspace is visible while the session is locked.
    NoLockLayer {
        workspace: String,
    },
    /// A second rule with the same `match` would leave which of the two an edit means to chance.
    RuleExists(String),
    NoArea(AreaId),
    NoGroup(GroupId),
    NoInstance(InstanceId),
    /// An index past the end of the list it names.
    OutOfRange {
        what: &'static str,
        index: usize,
        len: usize,
    },
    /// The lock layer's prompt, which an edit may move and restyle and nothing else (TA-8).
    Prompt {
        id: AreaId,
        refused: PromptEdit,
    },
    /// A workspace rule that would add, remove or resize an area that reserves space (TA-2).
    Reservation(AreaId),
}

/// What an edit tried to do to the lock layer's prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptEdit {
    Remove,
    ChangeKind,
    Hide,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpError::NoOutputRule(pattern) => {
                write!(f, "there is no output rule matching `{pattern}`")
            }
            OpError::NoWorkspaceRule { output, workspace } => write!(
                f,
                "the output rule `{output}` has no workspace rule matching `{workspace}`"
            ),
            OpError::NoLockLayer { workspace } => write!(
                f,
                "the workspace rule `{workspace}` has no lock layer: no workspace is visible while the screen is locked"
            ),
            OpError::RuleExists(at) => write!(f, "there is already a rule at `{at}`"),
            OpError::NoArea(id) => write!(f, "there is no area called `{id}`"),
            OpError::NoGroup(id) => write!(f, "there is no group called `{id}`"),
            OpError::NoInstance(id) => write!(f, "there is no instance called `{id}`"),
            OpError::OutOfRange { what, index, len } => {
                write!(f, "position {index} is past the {len} {what} there are")
            }
            OpError::Prompt { id, refused } => {
                write!(
                    f,
                    "`{id}` is the lock screen's password prompt, which can be moved and restyled "
                )?;
                match refused {
                    PromptEdit::Remove => f.write_str("but never removed"),
                    PromptEdit::ChangeKind => f.write_str("but never turned into another kind of area"),
                    PromptEdit::Hide => f.write_str(
                        "but never given a visibility expression: one that turned false would lock the user out",
                    ),
                }
            }
            OpError::Reservation(id) => write!(
                f,
                "a workspace rule cannot add, remove or resize `{id}` or change what it reserves: reservation is decided per output, so that switching workspaces never re-tiles windows"
            ),
        }
    }
}

impl std::error::Error for OpError {}

/// Carries out one operation and returns the one that takes it back.
pub fn apply(layout: &mut Layout, op: &LayoutOp) -> Result<LayoutOp, OpError> {
    match op {
        LayoutOp::InsertArea { site, index, area } => {
            leaves_reservation(layout, site, area)?;
            let areas = &mut layer_mut(layout, site)?.areas;
            bounds("areas", *index, areas.len())?;
            areas.insert(*index, (**area).clone());
            Ok(LayoutOp::DeleteArea {
                site: site.clone(),
                id: area.id.clone(),
            })
        }
        LayoutOp::DeleteArea { site, id } => {
            let areas = &mut layer_mut(layout, site)?.areas;
            let at =
                index_of(areas.iter().map(|a| &a.id), id).ok_or(OpError::NoArea(id.clone()))?;
            keeps_prompt(site, &areas[at], None)?;
            let area = areas.remove(at);
            Ok(LayoutOp::InsertArea {
                site: site.clone(),
                index: at,
                area: Box::new(area),
            })
        }
        LayoutOp::MoveArea { site, id, index } => {
            let areas = &mut layer_mut(layout, site)?.areas;
            let from =
                index_of(areas.iter().map(|a| &a.id), id).ok_or(OpError::NoArea(id.clone()))?;
            bounds("areas", *index, areas.len().saturating_sub(1))?;
            let area = areas.remove(from);
            areas.insert(*index, area);
            Ok(LayoutOp::MoveArea {
                site: site.clone(),
                id: id.clone(),
                index: from,
            })
        }
        LayoutOp::ReplaceArea { site, id, area } => {
            leaves_reservation(layout, site, area)?;
            let areas = &mut layer_mut(layout, site)?.areas;
            let at =
                index_of(areas.iter().map(|a| &a.id), id).ok_or(OpError::NoArea(id.clone()))?;
            keeps_prompt(site, &areas[at], Some((&area.kind, &area.visible)))?;
            let was = std::mem::replace(&mut areas[at], (**area).clone());
            Ok(LayoutOp::ReplaceArea {
                site: site.clone(),
                id: area.id.clone(),
                area: Box::new(was),
            })
        }
        LayoutOp::SetAreaKind { site, id, kind } => {
            if kind.is_some() && reserving_at(layout, site).is_some_and(|it| it.contains(id)) {
                return Err(OpError::Reservation(id.clone()));
            }
            let area = area_mut(layout, site, id)?;
            keeps_prompt(site, area, Some((&**kind, &area.visible)))?;
            let was = std::mem::replace(&mut area.kind, (**kind).clone());
            Ok(LayoutOp::SetAreaKind {
                site: site.clone(),
                id: id.clone(),
                kind: Box::new(was),
            })
        }
        LayoutOp::SetAreaStyle { site, id, style } => {
            let area = area_mut(layout, site, id)?;
            let was = std::mem::replace(&mut area.style, (**style).clone());
            Ok(LayoutOp::SetAreaStyle {
                site: site.clone(),
                id: id.clone(),
                style: Box::new(was),
            })
        }
        LayoutOp::SetAreaFlags {
            site,
            id,
            reserve,
            above_fullscreen,
            visible,
        } => {
            if site.workspace.is_some() && reserve.is_some() {
                return Err(OpError::Reservation(id.clone()));
            }
            let area = area_mut(layout, site, id)?;
            keeps_prompt(site, area, Some((&area.kind, visible)))?;
            let was = LayoutOp::SetAreaFlags {
                site: site.clone(),
                id: id.clone(),
                reserve: area.reserve,
                above_fullscreen: area.above_fullscreen,
                visible: area.visible.clone(),
            };
            area.reserve = *reserve;
            area.above_fullscreen = *above_fullscreen;
            area.visible = visible.clone();
            Ok(was)
        }
        LayoutOp::SetAreaActions { site, id, actions } => {
            let area = area_mut(layout, site, id)?;
            let was = std::mem::replace(&mut area.actions, actions.clone());
            Ok(LayoutOp::SetAreaActions {
                site: site.clone(),
                id: id.clone(),
                actions: was,
            })
        }
        LayoutOp::InsertGroup {
            site,
            area,
            index,
            group,
        } => {
            let groups = &mut area_mut(layout, site, area)?.groups;
            bounds("groups", *index, groups.len())?;
            groups.insert(*index, (**group).clone());
            Ok(LayoutOp::DeleteGroup {
                site: site.clone(),
                area: area.clone(),
                id: group.id.clone(),
            })
        }
        LayoutOp::DeleteGroup { site, area, id } => {
            let groups = &mut area_mut(layout, site, area)?.groups;
            let at =
                index_of(groups.iter().map(|g| &g.id), id).ok_or(OpError::NoGroup(id.clone()))?;
            let group = groups.remove(at);
            Ok(LayoutOp::InsertGroup {
                site: site.clone(),
                area: area.clone(),
                index: at,
                group: Box::new(group),
            })
        }
        LayoutOp::SetGroupKind {
            site,
            area,
            id,
            kind,
        } => {
            let group = group_mut(layout, site, area, id)?;
            let was = group.kind;
            group.kind = *kind;
            Ok(LayoutOp::SetGroupKind {
                site: site.clone(),
                area: area.clone(),
                id: id.clone(),
                kind: was,
            })
        }
        LayoutOp::SetGroupStacked {
            site,
            area,
            id,
            stacked,
        } => {
            let group = group_mut(layout, site, area, id)?;
            let was = std::mem::replace(&mut group.stacked, *stacked);
            Ok(LayoutOp::SetGroupStacked {
                site: site.clone(),
                area: area.clone(),
                id: id.clone(),
                stacked: was,
            })
        }
        LayoutOp::InsertInstance {
            spot,
            index,
            instance,
        } => {
            let children = &mut spot_mut(layout, spot)?.children;
            bounds("instances", *index, children.len())?;
            children.insert(*index, (**instance).clone());
            Ok(LayoutOp::DeleteInstance {
                spot: spot.clone(),
                id: instance.id.clone(),
            })
        }
        LayoutOp::DeleteInstance { spot, id } => {
            let children = &mut spot_mut(layout, spot)?.children;
            let at = index_of(children.iter().map(|i| &i.id), id)
                .ok_or(OpError::NoInstance(id.clone()))?;
            let instance = children.remove(at);
            Ok(LayoutOp::InsertInstance {
                spot: spot.clone(),
                index: at,
                instance: Box::new(instance),
            })
        }
        LayoutOp::MoveInstance {
            from,
            to,
            id,
            index,
        } => {
            let children = &mut spot_mut(layout, from)?.children;
            let was = index_of(children.iter().map(|i| &i.id), id)
                .ok_or(OpError::NoInstance(id.clone()))?;
            let instance = children.remove(was);

            let landing = &mut spot_mut(layout, to)?.children;
            if *index > landing.len() {
                let children = &mut spot_mut(layout, from)?.children;
                children.insert(was, instance);
                return Err(OpError::OutOfRange {
                    what: "instances",
                    index: *index,
                    len: landing_len(layout, to)?,
                });
            }
            landing.insert(*index, instance);

            Ok(LayoutOp::MoveInstance {
                from: to.clone(),
                to: from.clone(),
                id: id.clone(),
                index: was,
            })
        }
        LayoutOp::SetInstance { spot, id, instance } => {
            let children = &mut spot_mut(layout, spot)?.children;
            let at = index_of(children.iter().map(|i| &i.id), id)
                .ok_or(OpError::NoInstance(id.clone()))?;
            let was = children[at].clone();
            children[at] = (**instance).clone();
            children[at].id = id.clone();
            Ok(LayoutOp::SetInstance {
                spot: spot.clone(),
                id: id.clone(),
                instance: Box::new(was),
            })
        }
        LayoutOp::InsertOutputRule { index, rule } => {
            if layout.outputs.iter().any(|it| it.matches == rule.matches) {
                return Err(OpError::RuleExists(format!("outputs.{}", rule.matches.0)));
            }
            bounds("output rules", *index, layout.outputs.len())?;
            layout.outputs.insert(*index, (**rule).clone());
            Ok(LayoutOp::DeleteOutputRule {
                output: rule.matches.clone(),
            })
        }
        LayoutOp::DeleteOutputRule { output } => {
            let at = index_of(layout.outputs.iter().map(|it| &it.matches), output)
                .ok_or_else(|| OpError::NoOutputRule(output.0.clone()))?;
            let site = Site {
                output: output.clone(),
                workspace: None,
                layer: LayerKind::Lock,
            };
            for area in &layout.outputs[at].layers.lock.areas {
                keeps_prompt(&site, area, None)?;
            }
            let rule = layout.outputs.remove(at);
            Ok(LayoutOp::InsertOutputRule {
                index: at,
                rule: Box::new(rule),
            })
        }
        LayoutOp::InsertWorkspaceRule {
            output,
            index,
            rule,
        } => {
            let reserving = reserving_under(layout, output);
            for (_, layer) in rule.layers.each() {
                if let Some(id) = layer.remove.iter().find(|id| reserving.contains(*id)) {
                    return Err(OpError::Reservation(id.clone()));
                }
                if let Some(area) = layer
                    .areas
                    .iter()
                    .find(|area| touches_reservation(area, &reserving))
                {
                    return Err(OpError::Reservation(area.id.clone()));
                }
            }
            let workspaces = &mut rule_mut(layout, output)?.workspaces;
            if workspaces.iter().any(|it| it.matches == rule.matches) {
                return Err(OpError::RuleExists(format!(
                    "outputs.{}.workspaces.{}",
                    output.0, rule.matches.0
                )));
            }
            bounds("workspace rules", *index, workspaces.len())?;
            workspaces.insert(*index, (**rule).clone());
            Ok(LayoutOp::DeleteWorkspaceRule {
                output: output.clone(),
                workspace: rule.matches.clone(),
            })
        }
        LayoutOp::DeleteWorkspaceRule { output, workspace } => {
            let workspaces = &mut rule_mut(layout, output)?.workspaces;
            let at =
                index_of(workspaces.iter().map(|it| &it.matches), workspace).ok_or_else(|| {
                    OpError::NoWorkspaceRule {
                        output: output.0.clone(),
                        workspace: workspace.0.clone(),
                    }
                })?;
            let rule = workspaces.remove(at);
            Ok(LayoutOp::InsertWorkspaceRule {
                output: output.clone(),
                index: at,
                rule: Box::new(rule),
            })
        }
    }
}

/// Carries out a whole batch, or none of it.
///
/// A gesture is one transaction, so a batch that fails half way must not leave the layout in a state no interface can show. The operations already applied are taken back in reverse before the error is returned.
pub fn apply_all(layout: &mut Layout, ops: &[LayoutOp]) -> Result<Vec<LayoutOp>, OpError> {
    let mut undo = Vec::with_capacity(ops.len());
    for op in ops {
        match apply(layout, op) {
            Ok(back) => undo.push(back),
            Err(error) => {
                for back in undo.iter().rev() {
                    let _ = apply(layout, back);
                }
                return Err(error);
            }
        }
    }
    undo.reverse();
    Ok(undo)
}

/// The layer a site names, as the layout itself writes it.
pub fn layer<'a>(layout: &'a Layout, site: &Site) -> Result<&'a Layer, OpError> {
    let rule = layout
        .outputs
        .iter()
        .find(|rule| rule.matches == site.output)
        .ok_or_else(|| OpError::NoOutputRule(site.output.0.clone()))?;
    let Some(workspace) = &site.workspace else {
        return Ok(rule.layers.get(site.layer));
    };
    rule.workspaces
        .iter()
        .find(|it| &it.matches == workspace)
        .ok_or_else(|| no_workspace_rule(site, workspace))?
        .layers
        .get(site.layer)
        .ok_or_else(|| OpError::NoLockLayer {
            workspace: workspace.0.clone(),
        })
}

/// The areas one site holds, and none where the layout has no rule for it.
pub fn areas_at<'a>(layout: &'a Layout, site: &Site) -> &'a [Area] {
    layer(layout, site)
        .map(|layer| layer.areas.as_slice())
        .unwrap_or_default()
}

/// Every layer the layout writes, named the way an edit addresses it: each output rule's own layers first, then its workspace rules'. That order is what makes an id both levels mention mean the output rule's, which is the one the workspace rule refines.
pub fn sites(layout: &Layout) -> impl Iterator<Item = (Site, &Layer)> {
    let own = layout.outputs.iter().flat_map(|rule| {
        rule.layers.each().into_iter().map(|(kind, layer)| {
            (
                Site {
                    output: rule.matches.clone(),
                    workspace: None,
                    layer: kind,
                },
                layer,
            )
        })
    });
    let workspaces = layout.outputs.iter().flat_map(|rule| {
        rule.workspaces.iter().flat_map(|workspace| {
            workspace.layers.each().into_iter().map(|(kind, layer)| {
                (
                    Site {
                        output: rule.matches.clone(),
                        workspace: Some(workspace.matches.clone()),
                        layer: kind,
                    },
                    layer,
                )
            })
        })
    });
    own.chain(workspaces)
}

/// Where an instance is written: the group it is in, and its position in that group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub spot: Spot,
    pub index: usize,
}

/// Where the layout's own rules first write the instance `id`, in [`sites`] order.
pub fn placement_of(layout: &Layout, id: &InstanceId) -> Option<Placement> {
    sites(layout).find_map(|(site, layer)| {
        layer.areas.iter().find_map(|area| {
            area.groups.iter().find_map(|group| {
                let index = group.children.iter().position(|it| &it.id == id)?;
                Some(Placement {
                    spot: Spot {
                        site: site.clone(),
                        area: area.id.clone(),
                        group: group.id.clone(),
                    },
                    index,
                })
            })
        })
    })
}

/// Which rule and layer first write the area `id`, in [`sites`] order.
pub fn site_of_area(layout: &Layout, id: &AreaId) -> Option<Site> {
    sites(layout)
        .find(|(_, layer)| layer.areas.iter().any(|area| &area.id == id))
        .map(|(site, _)| site)
}

/// The group a spot names, as the layout writes it.
pub fn spot_of<'a>(layout: &'a Layout, spot: &Spot) -> Option<&'a Group> {
    areas_at(layout, &spot.site)
        .iter()
        .find(|area| area.id == spot.area)?
        .groups
        .iter()
        .find(|group| group.id == spot.group)
}

/// Refuses an edit that would take the lock layer's prompt away, turn it into another kind of area or give it an expression that could hide it. `after` is the kind and visibility the edit leaves the area with, `None` when it removes the area.
fn keeps_prompt(
    site: &Site,
    before: &Area,
    after: Option<(&Option<AreaKind>, &Option<Expr>)>,
) -> Result<(), OpError> {
    let is_prompt = |kind: &Option<AreaKind>| matches!(kind, Some(AreaKind::Prompt { .. }));
    if site.layer != LayerKind::Lock || !is_prompt(&before.kind) {
        return Ok(());
    }
    let refused = match after {
        None => PromptEdit::Remove,
        Some((kind, _)) if !is_prompt(kind) => PromptEdit::ChangeKind,
        Some((_, visible)) if visible.is_some() => PromptEdit::Hide,
        Some(_) => return Ok(()),
    };
    Err(OpError::Prompt {
        id: before.id.clone(),
        refused,
    })
}

/// Refuses an area for a workspace rule that says anything about reservation.
fn leaves_reservation(layout: &Layout, site: &Site, area: &Area) -> Result<(), OpError> {
    match reserving_at(layout, site).is_some_and(|reserving| touches_reservation(area, &reserving))
    {
        true => Err(OpError::Reservation(area.id.clone())),
        false => Ok(()),
    }
}

/// What a workspace-rule site must leave alone, and `None` for an output rule's own layers, which are where reservation is decided.
fn reserving_at(layout: &Layout, site: &Site) -> Option<BTreeSet<AreaId>> {
    site.workspace
        .is_some()
        .then(|| reserving_under(layout, &site.output))
}

/// Whether a workspace rule's area says anything about reservation: `reserve` at all, or geometry for an area that reserves — the same rule validation holds a written workspace rule to (TA-2).
fn touches_reservation(area: &Area, reserving: &BTreeSet<AreaId>) -> bool {
    area.reserve.is_some() || (area.kind.is_some() && reserving.contains(&area.id))
}

fn landing_len(layout: &mut Layout, spot: &Spot) -> Result<usize, OpError> {
    Ok(spot_mut(layout, spot)?.children.len())
}

fn bounds(what: &'static str, index: usize, len: usize) -> Result<(), OpError> {
    if index > len {
        return Err(OpError::OutOfRange { what, index, len });
    }
    Ok(())
}

fn index_of<'a, T: PartialEq + 'a>(
    mut ids: impl Iterator<Item = &'a T>,
    wanted: &T,
) -> Option<usize> {
    ids.position(|id| id == wanted)
}

fn no_workspace_rule(site: &Site, workspace: &WorkspaceMatch) -> OpError {
    OpError::NoWorkspaceRule {
        output: site.output.0.clone(),
        workspace: workspace.0.clone(),
    }
}

fn rule_mut<'a>(
    layout: &'a mut Layout,
    output: &OutputMatch,
) -> Result<&'a mut OutputRule, OpError> {
    layout
        .outputs
        .iter_mut()
        .find(|rule| &rule.matches == output)
        .ok_or_else(|| OpError::NoOutputRule(output.0.clone()))
}

fn layer_mut<'a>(layout: &'a mut Layout, site: &Site) -> Result<&'a mut Layer, OpError> {
    let rule = rule_mut(layout, &site.output)?;
    let Some(workspace) = &site.workspace else {
        return Ok(rule.layers.get_mut(site.layer));
    };
    rule.workspaces
        .iter_mut()
        .find(|it| &it.matches == workspace)
        .ok_or_else(|| no_workspace_rule(site, workspace))?
        .layers
        .get_mut(site.layer)
        .ok_or_else(|| OpError::NoLockLayer {
            workspace: workspace.0.clone(),
        })
}

fn area_mut<'a>(layout: &'a mut Layout, site: &Site, id: &AreaId) -> Result<&'a mut Area, OpError> {
    layer_mut(layout, site)?
        .areas
        .iter_mut()
        .find(|area| &area.id == id)
        .ok_or_else(|| OpError::NoArea(id.clone()))
}

fn group_mut<'a>(
    layout: &'a mut Layout,
    site: &Site,
    area: &AreaId,
    id: &GroupId,
) -> Result<&'a mut Group, OpError> {
    area_mut(layout, site, area)?
        .groups
        .iter_mut()
        .find(|group| &group.id == id)
        .ok_or_else(|| OpError::NoGroup(id.clone()))
}

fn spot_mut<'a>(layout: &'a mut Layout, spot: &Spot) -> Result<&'a mut Group, OpError> {
    group_mut(layout, &spot.site, &spot.area, &spot.group)
}
