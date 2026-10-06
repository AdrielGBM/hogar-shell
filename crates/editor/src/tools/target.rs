//! What the selection's corners and padding are as the screen draws them, and the operations that write new ones where the layout decides them.

use config::Shape;
use layout::{
    AreaKind, Corners, GroupId, Layout, LayoutOp, ResolvedArea, ResolvedAreaKind, ResolvedGroup,
    Sides, Style,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};

use crate::popover::{AreaDraft, group_entry};
use crate::session::{EditError, Selection};
use crate::written::Written;

use super::Tool;

/// Corners on any area, group or instance a layout writes, and padding where [`has_padding`] says.
pub(crate) fn offers(selection: &Selection, tool: Tool) -> bool {
    let Some(node) = selection.node() else {
        return false;
    };
    let Some(desktop) = reconcile::desktop_now(node.output.as_deref()) else {
        return false;
    };
    let Some(area) = desktop.resolved.area(node.layer, &node.area) else {
        return false;
    };
    match (tool, &node.part) {
        (Tool::Radius, Part::Area | Part::Group(_)) => true,
        (Tool::Radius, Part::Instance(_, id)) => id.komponent_child().is_none(),
        (Tool::Padding, part) => has_padding(area, part),
    }
}

/// Whether `part` of `area` holds anything off its edges ([`area_has_padding`], [`group_has_padding`]); an instance never does.
pub(crate) fn has_padding(area: &ResolvedArea, part: &Part) -> bool {
    match part {
        Part::Area => area_has_padding(&area.kind),
        Part::Group(id) => group_of(area, id).is_some_and(group_has_padding),
        Part::Instance(..) => false,
    }
}

/// An area that holds groups — a grid, a panel, a free area, a dock, a bar — and never a prompt, a stack, a wallpaper region or a texture.
pub(crate) fn area_has_padding(kind: &ResolvedAreaKind) -> bool {
    matches!(
        kind,
        ResolvedAreaKind::Grid { .. }
            | ResolvedAreaKind::Panel { .. }
            | ResolvedAreaKind::Free { .. }
            | ResolvedAreaKind::Dock { .. }
            | ResolvedAreaKind::Bar { .. }
    )
}

/// A container, which lays its children out inside its padding; a loose run or a group shown one at a time has none.
pub(crate) fn group_has_padding(group: &ResolvedGroup) -> bool {
    surfaces::container::arranges(group)
}

fn group_of<'a>(area: &'a ResolvedArea, id: &GroupId) -> Option<&'a ResolvedGroup> {
    area.groups.iter().find(|group| group.id == *id)
}

fn drawn<'a>(desktop: &'a Desktop, node: &Node) -> Option<&'a ResolvedArea> {
    desktop.resolved.area(node.layer, &node.area)
}

fn in_bar(area: &ResolvedArea) -> bool {
    matches!(area.kind, ResolvedAreaKind::Bar { .. })
}

/// The four corners of what `node` names as `desktop` draws them, top left first and clockwise: what its style or a bar's shape says, else what its kind rests at.
pub(crate) fn radius_of(desktop: &Desktop, node: &Node) -> Option<[f32; 4]> {
    let area = drawn(desktop, node)?;
    let corners = match &node.part {
        Part::Area => {
            let written = match &area.kind {
                ResolvedAreaKind::Bar { shape, .. } => shape.radius,
                _ => area.style.radius,
            };
            let along = matches!(area.kind, ResolvedAreaKind::Panel { along: true, .. });
            written
                .unwrap_or_else(|| surfaces::look::rest_radius(&area.kind, along, &desktop.config))
        }
        Part::Group(id) => group_of(area, id)?
            .style
            .radius
            .unwrap_or(Corners::all(ui::scale::plate::radius(in_bar(area)))),
        Part::Instance(group, id) => {
            let template = id.template();
            group_of(area, group)?
                .children
                .iter()
                .find(|child| child.id == template)?
                .style
                .radius
                .unwrap_or(Corners::all(
                    desktop.config.shape_from(None, None, None, None).radius,
                ))
        }
    };
    Some(corners.to_array())
}

/// The padding of what `node` names as `desktop` draws it, top first and clockwise: what its style says, else what its kind pads by.
pub(crate) fn padding_of(desktop: &Desktop, node: &Node) -> Option<[f32; 4]> {
    let area = drawn(desktop, node)?;
    let sides = match &node.part {
        Part::Area => area.style.padding.unwrap_or_else(|| match &area.kind {
            ResolvedAreaKind::Bar { shape, .. } => {
                let shape = surfaces::bar::bar_shape(&desktop.config, *shape);
                match shape.mode {
                    Shape::Bar => Sides::all(shape.padding()),
                    _ => Sides::all(0.0),
                }
            }
            _ => Sides::all(0.0),
        }),
        Part::Group(id) => surfaces::container::padding_of(group_of(area, id)?, in_bar(area)),
        Part::Instance(..) => return None,
    };
    Some(sides.to_array())
}

/// The operations that round the corners of what `node` names to `corners`, written into `layout` where it decides them: a bar's own `shape.radius`, anything else's `style.radius`.
pub(crate) fn radius_ops(
    node: &Node,
    layout: &Layout,
    corners: [f32; 4],
) -> Result<Vec<LayoutOp>, EditError> {
    let [top_left, top_right, bottom_right, bottom_left] = corners;
    let corners = Corners::each(top_left, top_right, bottom_right, bottom_left);
    let bar = reconcile::desktop_now(node.output.as_deref())
        .and_then(|desktop| desktop.resolved.area(node.layer, &node.area).map(in_bar))
        .unwrap_or(false);
    restyled(node, layout, |area_or_group| match area_or_group {
        Restyled::Area(area) if bar => {
            if let Some(AreaKind::Bar { shape, .. }) = AreaDraft::kind_mut(area, "bar") {
                shape.radius = Some(corners);
            }
        }
        Restyled::Area(area) => area.style.radius = Some(corners),
        Restyled::Style(style) => style.radius = Some(corners),
    })
}

/// The operations that pad what `node` names by `sides`, written into `layout` where it decides them, as its `style.padding`.
pub(crate) fn padding_ops(
    node: &Node,
    layout: &Layout,
    sides: [f32; 4],
) -> Result<Vec<LayoutOp>, EditError> {
    let [top, right, bottom, left] = sides;
    let sides = Sides::each(top, right, bottom, left);
    restyled(node, layout, |area_or_group| match area_or_group {
        Restyled::Area(area) => area.style.padding = Some(sides),
        Restyled::Style(style) => style.padding = Some(sides),
    })
}

/// What a change to a look is written into: the area as its level writes it, or the style of a group or an instance in it.
enum Restyled<'a> {
    Area(&'a mut layout::Area),
    Style(&'a mut Style),
}

fn restyled(
    node: &Node,
    layout: &Layout,
    change: impl FnOnce(Restyled),
) -> Result<Vec<LayoutOp>, EditError> {
    let workspace = crate::variant::editing_for(node);
    let written = Written::area(
        layout,
        node.output.as_deref(),
        node.layer,
        &node.area,
        workspace.as_ref(),
    )
    .map_err(EditError::Refused)?;
    match &node.part {
        Part::Area => {
            let mut area = written.area.clone();
            change(Restyled::Area(&mut area));
            Ok(written.ops(&area))
        }
        Part::Group(id) => {
            let mut area = written.area.clone();
            change(Restyled::Style(&mut group_entry(&mut area, id).style));
            Ok(written.ops(&area))
        }
        Part::Instance(group, id) => {
            let held = written.instance(group, &id.template());
            let mut instance = held.instance.clone();
            change(Restyled::Style(&mut instance.style));
            Ok(held.ops(&instance))
        }
    }
}
