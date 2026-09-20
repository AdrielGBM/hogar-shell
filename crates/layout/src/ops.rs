//! The edits a layout can undergo, and how to take each one back.
//!
//! Every change to a layout — a drag in an edit mode, a value scrubbed in a popover, a line of `hogar-shell layout …` — becomes one of these. That is what lets a single undo stack cover all three: the stack holds operations, not a record of which part of the interface made them.
//!
//! [`apply`] returns the operation that undoes what it just did, built from the values it displaced. Undo is therefore exact rather than approximate: putting an instance back puts it back in the group and at the index it came from, with the id it always had, so a rule or an IPC command that addressed it still finds it. Redo is applying the inverse of the inverse, which [`apply`] produces in the same way.
//!
//! An operation that cannot be carried out — an area that is no longer there, an index past the end of a list — is an [`OpError`], never a panic and never a silent no-op. A caller that got one has a layout it did not expect, and the transaction it belongs to is abandoned whole rather than applied in part.

use std::fmt;

use crate::model::*;

/// Which output rule and layer an operation acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub output: OutputMatch,
    pub layer: LayerKind,
}

impl Site {
    pub fn new(output: impl Into<String>, layer: LayerKind) -> Self {
        Self {
            output: OutputMatch(output.into()),
            layer,
        }
    }

    /// The rule that speaks for every output, which is where an edit lands unless it was made for one monitor.
    pub fn everywhere(layer: LayerKind) -> Self {
        Self::new("*", layer)
    }
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outputs.{}.layers.{}", self.output.0, self.layer)
    }
}

/// The group an instance sits in, named in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    pub site: Site,
    pub area: AreaId,
    pub group: GroupId,
}

#[derive(Clone, Debug)]
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpError {
    NoOutputRule(String),
    NoArea(AreaId),
    NoGroup(GroupId),
    NoInstance(InstanceId),
    /// An index past the end of the list it names.
    OutOfRange {
        what: &'static str,
        index: usize,
        len: usize,
    },
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpError::NoOutputRule(pattern) => {
                write!(f, "there is no output rule matching `{pattern}`")
            }
            OpError::NoArea(id) => write!(f, "there is no area called `{id}`"),
            OpError::NoGroup(id) => write!(f, "there is no group called `{id}`"),
            OpError::NoInstance(id) => write!(f, "there is no instance called `{id}`"),
            OpError::OutOfRange { what, index, len } => {
                write!(f, "position {index} is past the {len} {what} there are")
            }
        }
    }
}

impl std::error::Error for OpError {}

/// Carries out one operation and returns the one that takes it back.
pub fn apply(layout: &mut Layout, op: &LayoutOp) -> Result<LayoutOp, OpError> {
    match op {
        LayoutOp::InsertArea { site, index, area } => {
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
        LayoutOp::SetAreaKind { site, id, kind } => {
            let area = area_mut(layout, site, id)?;
            let was = area.kind.clone();
            area.kind = (**kind).clone();
            Ok(LayoutOp::SetAreaKind {
                site: site.clone(),
                id: id.clone(),
                kind: Box::new(was),
            })
        }
        LayoutOp::SetAreaStyle { site, id, style } => {
            let area = area_mut(layout, site, id)?;
            let was = area.style.clone();
            area.style = (**style).clone();
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
            let area = area_mut(layout, site, id)?;
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

fn layer_mut<'a>(layout: &'a mut Layout, site: &Site) -> Result<&'a mut Layer, OpError> {
    let rule = layout
        .outputs
        .iter_mut()
        .find(|rule| rule.matches == site.output)
        .ok_or_else(|| OpError::NoOutputRule(site.output.0.clone()))?;
    Ok(match site.layer {
        LayerKind::Background => &mut rule.layers.background,
        LayerKind::Desktop => &mut rule.layers.desktop,
        LayerKind::Top => &mut rule.layers.top,
        LayerKind::Overlay => &mut rule.layers.overlay,
        LayerKind::Lock => &mut rule.layers.lock,
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
