use layout::{Area, AreaId, AreaKind, InstanceId, LayerKind, ResolvedAreaKind};
use surfaces::panel::Owner;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};
use telar::MenuEntry;

use crate::mode;
use crate::session::{self, EditError, Selection};
use crate::written::Work;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Offer {
    pub(crate) owner: InstanceId,
    pub(crate) owned: Option<AreaId>,
    pub(crate) in_bar: bool,
}

pub(crate) fn offer(desktop: &Desktop, node: &Node) -> Option<Offer> {
    let Part::Instance(_, id) = &node.part else {
        return None;
    };
    if node.layer == LayerKind::Lock || id.komponent_child().is_some() {
        return None;
    }
    let layer = desktop.resolved.layer(node.layer)?;
    let holder = layer.areas.iter().find(|area| area.id == node.area)?;
    if matches!(holder.kind, ResolvedAreaKind::Panel { .. }) {
        return None;
    }
    let owner = id.template();
    let owned = layer
        .areas
        .iter()
        .find(|area| matches!(&area.kind, ResolvedAreaKind::Panel { owner: held, .. } if *held == owner))
        .map(|area| area.id.clone());
    Some(Offer {
        owner,
        owned,
        in_bar: matches!(holder.kind, ResolvedAreaKind::Bar { .. }),
    })
}

pub(crate) fn offered(node: &Node) -> Option<Offer> {
    reconcile::desktop(node.output.as_deref()).and_then(|desktop| offer(&desktop, node))
}

/// How a panel given to an instance opens off it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Beside its owner, its cells across and down.
    Beside,
    /// Along the whole of its owner's bar.
    Along,
}

pub(crate) fn give(node: &Node, shape: Shape) -> Result<(), EditError> {
    if node.layer == LayerKind::Lock {
        return Err(EditError::refused(telar::t!("editor.panel.lock")));
    }
    mode::ensure_editing(node)?;
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let offer = offer(&desktop, node)
        .ok_or_else(|| EditError::refused(telar::t!("editor.panel.nested")))?;
    if let Some(owned) = &offer.owned {
        return show(node, owned);
    }
    let along = shape == Shape::Along;
    if along && !offer.in_bar {
        return Err(EditError::refused(telar::t!("editor.panel.no_bar")));
    }
    let layout = session::draft().peek();
    let mut work = Work::new(&layout, &desktop, node.layer);
    let stem = format!("{}-panel", offer.owner);
    let id = layout::ops::free_area_id(&layout, &work.known, node.layer, &stem);
    work.add(
        node.layer,
        Area {
            id: id.clone(),
            kind: Some(AreaKind::Panel {
                owner: Some(offer.owner.clone()),
                along: along.then_some(true),
                cols: None,
                rows: None,
                cell: None,
                gap: None,
            }),
            ..Area::default()
        },
    )?;
    let name = crate::steps::name_of(&Selection::of(node.clone()));
    let label = match shape {
        Shape::Along => telar::t!("editor.panel.docked", name = name),
        Shape::Beside => telar::t!("editor.panel.given", name = name),
    };
    crate::context::commit(label, work.done())?;
    show(node, &id)
}

fn show(node: &Node, owned: &AreaId) -> Result<(), EditError> {
    if let Some(owner) = Owner::of(node)
        && !surfaces::transient::is_open(&owner.id())
    {
        surfaces::panel::toggle_owned(node);
    }
    session::select(Selection::Area(Node::area(
        node.output.as_deref(),
        node.layer,
        owned,
    )));
    Ok(())
}

pub(crate) fn rows(node: &Node) -> Vec<MenuEntry> {
    let Some(offer) =
        reconcile::desktop_now(node.output.as_deref()).and_then(|desktop| offer(&desktop, node))
    else {
        return Vec::new();
    };
    let row = |label: String, shape: Shape| {
        let node = node.clone();
        MenuEntry::row(label, "", move || mode::said(give(&node, shape)))
    };
    match offer.owned {
        Some(_) => vec![row(telar::t!("editor.panel.edit"), Shape::Beside)],
        None => {
            let mut rows = vec![row(telar::t!("editor.panel.give"), Shape::Beside)];
            if offer.in_bar {
                rows.push(row(telar::t!("editor.panel.dock"), Shape::Along));
            }
            rows
        }
    }
}
