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

use util::report::Message;

use crate::library::Library;
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
        style: Box<Style>,
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
    /// Changes how a group lays out its children, with its inner grid and gap, wherever it is placed: an instance dropped onto another makes a `pages` group, and the same stack taken apart again a loose one. Where each child sits in it is the child's own, through [`LayoutOp::SetInstance`].
    SetGroupArrange {
        site: Site,
        area: AreaId,
        id: GroupId,
        arrange: Option<Arrange>,
        cols: Option<u32>,
        rows: Option<u32>,
        gap: Option<f32>,
    },
    SetGroupStyle {
        site: Site,
        area: AreaId,
        id: GroupId,
        style: Box<Style>,
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
    /// Replaces the ids of the areas a layer takes away from what earlier levels place: an area a broader rule or an extended layout writes, deleted on one monitor or one workspace alone.
    SetLayerRemove {
        site: Site,
        remove: Vec<AreaId>,
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
        what: Listed,
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

/// A list an edit names a position in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed {
    Areas,
    Groups,
    Instances,
    OutputRules,
    WorkspaceRules,
}

impl Listed {
    /// `len` of them, as a sentence counts them.
    fn counted(self, len: usize) -> Message {
        match self {
            Listed::Areas => util::message!("finding.listed.areas", len = len),
            Listed::Groups => util::message!("finding.listed.groups", len = len),
            Listed::Instances => util::message!("finding.listed.instances", len = len),
            Listed::OutputRules => util::message!("finding.listed.output_rules", len = len),
            Listed::WorkspaceRules => util::message!("finding.listed.workspace_rules", len = len),
        }
    }
}

impl OpError {
    /// Why the edit was refused, in no language yet.
    pub fn message(&self) -> Message {
        match self {
            OpError::NoOutputRule(pattern) => {
                util::message!("finding.no_output_rule", pattern = pattern)
            }
            OpError::NoWorkspaceRule { output, workspace } => util::message!(
                "finding.no_workspace_rule",
                output = output,
                workspace = workspace
            ),
            OpError::NoLockLayer { workspace } => {
                util::message!("finding.no_lock_layer", workspace = workspace)
            }
            OpError::RuleExists(at) => util::message!("finding.rule_exists", at = at),
            OpError::NoArea(id) => util::message!("finding.no_area", id = id),
            OpError::NoGroup(id) => util::message!("finding.no_group", id = id),
            OpError::NoInstance(id) => util::message!("finding.no_instance", id = id),
            OpError::OutOfRange { what, index, len } => util::message!(
                "finding.out_of_range",
                index = index,
                listed = what.counted(*len)
            ),
            OpError::Prompt { id, refused } => match refused {
                PromptEdit::Remove => util::message!("finding.prompt_removed", id = id),
                PromptEdit::ChangeKind => util::message!("finding.prompt_kind", id = id),
                PromptEdit::Hide => util::message!("finding.prompt_hidden", id = id),
            },
            OpError::Reservation(id) => util::message!("finding.reservation", id = id),
        }
    }
}

/// In English, as the command line says it.
impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message().english())
    }
}

impl std::error::Error for OpError {}

/// Carries out one operation and returns the one that takes it back.
pub fn apply(layout: &mut Layout, op: &LayoutOp) -> Result<LayoutOp, OpError> {
    match op {
        LayoutOp::InsertArea { site, index, area } => {
            leaves_reservation(layout, site, area)?;
            let areas = &mut layer_mut(layout, site)?.areas;
            bounds(Listed::Areas, *index, areas.len())?;
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
            bounds(Listed::Areas, *index, areas.len().saturating_sub(1))?;
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
            bounds(Listed::Groups, *index, groups.len())?;
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
        LayoutOp::SetGroupArrange {
            site,
            area,
            id,
            arrange,
            cols,
            rows,
            gap,
        } => {
            let group = group_mut(layout, site, area, id)?;
            Ok(LayoutOp::SetGroupArrange {
                site: site.clone(),
                area: area.clone(),
                id: id.clone(),
                arrange: std::mem::replace(&mut group.arrange, *arrange),
                cols: std::mem::replace(&mut group.cols, *cols),
                rows: std::mem::replace(&mut group.rows, *rows),
                gap: std::mem::replace(&mut group.gap, *gap),
            })
        }
        LayoutOp::SetGroupStyle {
            site,
            area,
            id,
            style,
        } => {
            let group = group_mut(layout, site, area, id)?;
            let was = std::mem::replace(&mut group.style, (**style).clone());
            Ok(LayoutOp::SetGroupStyle {
                site: site.clone(),
                area: area.clone(),
                id: id.clone(),
                style: Box::new(was),
            })
        }
        LayoutOp::InsertInstance {
            spot,
            index,
            instance,
        } => {
            let children = &mut spot_mut(layout, spot)?.children;
            bounds(Listed::Instances, *index, children.len())?;
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
                    what: Listed::Instances,
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
        LayoutOp::SetLayerRemove { site, remove } => {
            let was = &layer(layout, site)?.remove;
            let added: Vec<&AreaId> = remove.iter().filter(|id| !was.contains(id)).collect();
            if let Some(reserving) = reserving_at(layout, site)
                && let Some(id) = added.iter().find(|id| reserving.contains(**id))
            {
                return Err(OpError::Reservation((*id).clone()));
            }
            if site.layer == LayerKind::Lock
                && let Some(id) = added.iter().find(|id| is_prompt(layout, id))
            {
                return Err(OpError::Prompt {
                    id: (*id).clone(),
                    refused: PromptEdit::Remove,
                });
            }
            let was = std::mem::replace(&mut layer_mut(layout, site)?.remove, remove.clone());
            Ok(LayoutOp::SetLayerRemove {
                site: site.clone(),
                remove: was,
            })
        }
        LayoutOp::InsertOutputRule { index, rule } => {
            if layout.outputs.iter().any(|it| it.matches == rule.matches) {
                return Err(OpError::RuleExists(format!("outputs.{}", rule.matches.0)));
            }
            bounds(Listed::OutputRules, *index, layout.outputs.len())?;
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
            bounds(Listed::WorkspaceRules, *index, workspaces.len())?;
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
///
/// A panel goes with its owner: a batch that leaves the layout writing no instance a panel opens from, where it wrote one before, deletes that panel too, so whatever took the owner out — an instance, its group, its area — one undo puts both back.
pub fn apply_all(layout: &mut Layout, ops: &[LayoutOp]) -> Result<Vec<LayoutOp>, OpError> {
    let owners = owners_written(layout);
    let mut undo = Vec::with_capacity(ops.len());
    let applied = carry_out(layout, ops, &mut undo).and_then(|()| {
        let orphaned = panels_without_owner(layout, &owners);
        carry_out(layout, &orphaned, &mut undo)
    });
    if let Err(error) = applied {
        for back in undo.iter().rev() {
            let _ = apply(layout, back);
        }
        return Err(error);
    }
    undo.reverse();
    Ok(undo)
}

fn carry_out(
    layout: &mut Layout,
    ops: &[LayoutOp],
    undo: &mut Vec<LayoutOp>,
) -> Result<(), OpError> {
    for op in ops {
        undo.push(apply(layout, op)?);
    }
    Ok(())
}

/// The owners of the panels `layout` writes that it also places.
fn owners_written(layout: &Layout) -> BTreeSet<InstanceId> {
    panels(layout)
        .map(|(_, _, owner)| owner)
        .filter(|owner| placed_anywhere(layout, owner))
        .cloned()
        .collect()
}

/// The deletions of the panels `layout` writes whose owner is one of `owners` and is no longer placed anywhere in it.
fn panels_without_owner(layout: &Layout, owners: &BTreeSet<InstanceId>) -> Vec<LayoutOp> {
    if owners.is_empty() {
        return Vec::new();
    }
    panels(layout)
        .filter(|(_, _, owner)| owners.contains(*owner) && !placed_anywhere(layout, owner))
        .map(|(site, area, _)| LayoutOp::DeleteArea {
            site,
            id: area.id.clone(),
        })
        .collect()
}

/// The operations that make every panel `layout` writes for the instance `from` open from `to` instead: what an edit that gives an instance another id adds, so the panel follows it rather than going with the old id.
pub fn reowned(layout: &Layout, from: &InstanceId, to: &InstanceId) -> Vec<LayoutOp> {
    panels(layout)
        .filter(|(_, _, owner)| *owner == from)
        .map(|(site, area, _)| {
            let mut kind = area.kind.clone();
            if let Some(AreaKind::Panel { owner, .. }) = &mut kind {
                *owner = Some(to.clone());
            }
            LayoutOp::SetAreaKind {
                site,
                id: area.id.clone(),
                kind: Box::new(kind),
            }
        })
        .collect()
}

/// Every panel `layout` writes with its owner, where it names one, and the site it is written at.
fn panels(layout: &Layout) -> impl Iterator<Item = (Site, &Area, &InstanceId)> {
    sites(layout).flat_map(|(site, layer)| {
        layer.areas.iter().filter_map(move |area| {
            let owner = area.kind.as_ref()?.owner()?;
            Some((site.clone(), area, owner))
        })
    })
}

/// Whether `layout` places the instance `id` at any level.
fn placed_anywhere(layout: &Layout, id: &InstanceId) -> bool {
    sites(layout).any(|(_, layer)| layer.areas.iter().any(|area| area.places(id)))
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

/// [`sites`], to change, in the same order.
pub fn sites_mut(layout: &mut Layout) -> Vec<(Site, &mut Layer)> {
    let mut own = Vec::new();
    let mut workspaces = Vec::new();
    for rule in &mut layout.outputs {
        let output = rule.matches.clone();
        for (kind, layer) in rule.layers.each_mut() {
            own.push((
                Site {
                    output: output.clone(),
                    workspace: None,
                    layer: kind,
                },
                layer,
            ));
        }
        for workspace in &mut rule.workspaces {
            let matches = workspace.matches.clone();
            for (kind, layer) in workspace.layers.each_mut() {
                workspaces.push((
                    Site {
                        output: output.clone(),
                        workspace: Some(matches.clone()),
                        layer: kind,
                    },
                    layer,
                ));
            }
        }
    }
    own.extend(workspaces);
    own
}

/// An id no area of the layer `layer` has anywhere in `layout` or a layout it extends, readable and derived from `stem` (F-10.3): `left`, then `left-2`, `left-3`, a trailing count on `stem` taken off first so a split of `left-2` is `left-3` rather than `left-2-2`.
pub fn free_area_id(layout: &Layout, known: &Library, layer: LayerKind, stem: &str) -> AreaId {
    let taken: BTreeSet<&str> = crate::resolve::chain_of(layout, known, &mut Default::default())
        .into_iter()
        .flat_map(|level| {
            sites(level)
                .filter(move |(site, _)| site.layer == layer)
                .flat_map(|(_, written)| {
                    written
                        .areas
                        .iter()
                        .map(|area| area.id.as_str())
                        .chain(written.remove.iter().map(AreaId::as_str))
                })
        })
        .collect();
    AreaId::new(free_id(&taken, stem))
}

/// An id no instance has anywhere in `layout` or a layout it extends, placed or named in a group's `remove`, derived from `stem` the way [`free_area_id`] derives one: `clock`, then `clock-2`. Instance ids are unique across the whole layout, so a new one never lands on an inherited instance and merges into it.
pub fn free_instance_id(layout: &Layout, known: &Library, stem: &str) -> InstanceId {
    let taken = taken_instance_ids(layout, known);
    let taken: BTreeSet<&str> = taken.iter().map(String::as_str).collect();
    InstanceId::new(free_id(&taken, stem))
}

/// Every instance id `layout` or a layout it extends places or names in a group's `remove`.
pub(crate) fn taken_instance_ids(layout: &Layout, known: &Library) -> BTreeSet<String> {
    crate::resolve::chain_of(layout, known, &mut Default::default())
        .into_iter()
        .flat_map(|level| sites(level).flat_map(|(_, written)| written.areas.iter()))
        .flat_map(|area| area.groups.iter())
        .flat_map(|group| {
            group
                .children
                .iter()
                .map(|child| child.id.to_string())
                .chain(group.remove.iter().map(InstanceId::to_string))
        })
        .collect()
}

/// An id no group of the area `area` on `layer` has anywhere in `layout` or a layout it extends, placed or named in that area's `remove` at any level, derived from `stem` the way [`free_area_id`] derives one: `end`, then `end-2`. Group ids are scoped to their area, so a new group never merges into one a broader level places there, nor revives one a level took away.
pub fn free_group_id(
    layout: &Layout,
    known: &Library,
    layer: LayerKind,
    area: &AreaId,
    stem: &str,
) -> GroupId {
    let taken: BTreeSet<&str> = crate::resolve::chain_of(layout, known, &mut Default::default())
        .into_iter()
        .flat_map(|level| {
            sites(level)
                .filter(move |(site, _)| site.layer == layer)
                .flat_map(|(_, written)| written.areas.iter())
        })
        .filter(|written| written.id == *area)
        .flat_map(|written| {
            written
                .groups
                .iter()
                .map(|group| group.id.as_str())
                .chain(written.remove.iter().map(GroupId::as_str))
        })
        .collect();
    GroupId::new(free_id(&taken, stem))
}

pub(crate) fn free_id(taken: &BTreeSet<&str>, stem: &str) -> String {
    let stem = match stem.rsplit_once('-') {
        Some((base, count)) if !base.is_empty() && count.parse::<u32>().is_ok() => base,
        _ => stem,
    };
    if !taken.contains(stem) {
        return stem.to_string();
    }
    (2..)
        .map(|nth| format!("{stem}-{nth}"))
        .find(|id| !taken.contains(id.as_str()))
        .expect("the counting runs out long after the ids do")
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

/// Whether the layout writes `id` as a lock prompt anywhere, which no level may take away.
fn is_prompt(layout: &Layout, id: &AreaId) -> bool {
    sites(layout)
        .filter(|(site, _)| site.layer == LayerKind::Lock)
        .flat_map(|(_, layer)| layer.areas.iter())
        .any(|area| area.id == *id && matches!(area.kind, Some(AreaKind::Prompt { .. })))
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

fn bounds(what: Listed, index: usize, len: usize) -> Result<(), OpError> {
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
