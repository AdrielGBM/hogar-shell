//! What the selection's corners and padding are as the screen draws them, and the operations that write new ones where the layout decides them.

use config::Shape;
use layout::{
    Corners, GroupId, Layout, LayoutOp, ResolvedArea, ResolvedAreaKind, ResolvedGroup, Sides,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};

use crate::popover::look::{self, StyleOf, WrittenStyle};
use crate::session::{EditError, Selection};
use crate::written::Written;

use super::Tool;

/// Corners on any area, group or instance a layout writes, and padding where [`has_padding`] says.
pub(crate) fn offers(selection: &Selection, tool: Tool) -> bool {
    let Some(node) = selection.node() else {
        return false;
    };
    reconcile::with_desktop_now(node.output.as_deref(), |desktop| {
        let Some(area) = desktop.resolved.area(node.layer, &node.area) else {
            return false;
        };
        match (tool, &node.part) {
            (Tool::Radius, Part::Area | Part::Group(_)) => true,
            (Tool::Radius, Part::Instance(_, id)) => id.komponent_child().is_none(),
            (Tool::Padding, part) => has_padding(area, part),
        }
    })
    .unwrap_or(false)
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
    Ok(written_style(node, layout)?.ops(|style| (look::RADIUS.write)(style, corners)))
}

/// The operations that pad what `node` names by `sides`, written into `layout` where it decides them, as its `style.padding`.
pub(crate) fn padding_ops(
    node: &Node,
    layout: &Layout,
    sides: [f32; 4],
) -> Result<Vec<LayoutOp>, EditError> {
    Ok(written_style(node, layout)?.ops(|style| (look::PADDING.write)(style, sides)))
}

/// Where `layout` writes the style of what `node` names, at the level the editor writes into.
fn written_style(node: &Node, layout: &Layout) -> Result<WrittenStyle, EditError> {
    let workspace = crate::variant::editing_for(node);
    let written = Written::area(
        layout,
        node.output.as_deref(),
        node.layer,
        &node.area,
        workspace.as_ref(),
    )
    .map_err(EditError::Refused)?;
    let of = match &node.part {
        Part::Area if is_bar(node) => StyleOf::Bar,
        Part::Area => StyleOf::Area,
        Part::Group(id) => StyleOf::Group(id.clone()),
        Part::Instance(group, id) => StyleOf::Instance(group.clone(), id.template()),
    };
    Ok(WrittenStyle { written, of })
}

fn is_bar(node: &Node) -> bool {
    reconcile::with_desktop_now(node.output.as_deref(), |desktop| {
        desktop
            .resolved
            .area(node.layer, &node.area)
            .is_some_and(in_bar)
    })
    .unwrap_or(false)
}
