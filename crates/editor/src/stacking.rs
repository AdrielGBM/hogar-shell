use telar::MenuEntry;

use layout::{AreaId, Arrange, Layout, LayoutOp, ResolvedArea, ResolvedAreaKind, ResolvedGroup};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{Node, Part};

use crate::keys::Chord;
use crate::session::{self, EditError, Selection};
use crate::written::Work;
use crate::{context, mode, steps};

/// Which way along its stack the selection goes: one step, or all the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Forward,
    Backward,
    Front,
    Back,
}

impl Order {
    pub(crate) const ALL: [Order; 4] = [Order::Forward, Order::Backward, Order::Front, Order::Back];

    pub(crate) fn chord(self) -> Chord {
        match self {
            Order::Forward => Chord::char(']').ctrl(),
            Order::Backward => Chord::char('[').ctrl(),
            Order::Front => Chord::char(']').ctrl().shift(),
            Order::Back => Chord::char('[').ctrl().shift(),
        }
    }

    /// The order `pressed` asks for, Shift turning a step into all the way.
    pub(crate) fn of(pressed: &Chord) -> Option<Self> {
        [Order::Front, Order::Back, Order::Forward, Order::Backward]
            .into_iter()
            .find(|order| order.chord().matches(&pressed.key, pressed.modifiers))
    }

    fn label(self) -> String {
        match self {
            Order::Forward => telar::t!("editor.order.forward"),
            Order::Backward => telar::t!("editor.order.backward"),
            Order::Front => telar::t!("editor.order.front"),
            Order::Back => telar::t!("editor.order.back"),
        }
    }

    fn toward_front(self) -> bool {
        matches!(self, Order::Forward | Order::Front)
    }

    /// Where something at `at` among `len` drawn back to front goes.
    fn target(self, at: usize, len: usize) -> usize {
        let last = len.saturating_sub(1);
        match self {
            Order::Forward => (at + 1).min(last),
            Order::Backward => at.saturating_sub(1),
            Order::Front => last,
            Order::Back => 0,
        }
    }
}

/// What the selection is drawn over and under: the areas of its layer that overlap, or the children of the `free` container it is in, back to front.
enum Stack {
    Areas(Vec<AreaId>),
    Children(Box<ResolvedGroup>),
}

impl Stack {
    fn of(desktop: &Desktop, node: &Node) -> Option<(Self, usize, usize)> {
        let layer = desktop.resolved.layer(node.layer)?;
        match &node.part {
            Part::Area => {
                let ids: Vec<AreaId> = layer
                    .areas
                    .iter()
                    .filter(|area| restacks(area))
                    .map(|area| area.id.clone())
                    .collect();
                let at = ids.iter().position(|id| *id == node.area)?;
                let len = ids.len();
                (len > 1).then_some((Stack::Areas(ids), at, len))
            }
            Part::Instance(group, id) => {
                let area = layer.areas.iter().find(|area| area.id == node.area)?;
                let holding = area.groups.iter().find(|held| held.id == *group)?;
                let free = holding.arrange == Some(Arrange::Free) && holding.komponent.is_none();
                let at = holding
                    .children
                    .iter()
                    .position(|child| child.id == id.template())?;
                let len = holding.children.len();
                (free && len > 1).then(|| (Stack::Children(Box::new(holding.clone())), at, len))
            }
            Part::Group(_) => None,
        }
    }
}

/// Whether `area` is of a kind that overlaps others of its layer, and so has an order among them: a bar, a region, a grid or a dock tiles its layer or its edge, and the lock's prompt is drawn over everything whatever the order says.
fn restacks(area: &ResolvedArea) -> bool {
    matches!(
        area.kind,
        ResolvedAreaKind::Free { .. }
            | ResolvedAreaKind::Texture { .. }
            | ResolvedAreaKind::Stack { .. }
    )
}

/// Moves what `selection` is `order` way along its stack, as one undo entry.
pub(crate) fn restack(selection: &Selection, order: Order) -> Result<(), EditError> {
    let node = selection.node().cloned().ok_or_else(EditError::nothing)?;
    mode::ensure_editing(&node)?;
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = restacked(&session::draft().peek(), &desktop, &node, order)?;
    let label = telar::t!(
        "editor.keys.did",
        what = order.label(),
        name = steps::name_of(selection)
    );
    context::commit(label, ops)
}

/// The operations that move what `node` names `order` way along what it is drawn over and under, on `desktop`'s screen: an area among the areas of its layer, a child of a `free` container among its siblings. What another level writes the place of is refused, since a level laid over another never restacks what it inherits.
pub fn restacked(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
    order: Order,
) -> Result<Vec<LayoutOp>, EditError> {
    let name = steps::name_of(&Selection::of(node.clone()));
    let (stack, at, len) = Stack::of(desktop, node)
        .ok_or_else(|| EditError::refused(util::message!("editor.order.none")))?;
    let to = order.target(at, len);
    if to == at {
        return Err(EditError::refused(match order.toward_front() {
            true => util::message!("editor.order.in_front", name = name),
            false => util::message!("editor.order.behind", name = name),
        }));
    }
    let mut work = Work::new(layout, desktop, node.layer);
    match stack {
        Stack::Areas(ids) => areas(&mut work, node, &ids, (at, to))?,
        Stack::Children(group) => steps::reordered(&mut work, node, &group, to)?,
    }
    Ok(work.done())
}

fn inherited(id: impl ToString) -> EditError {
    EditError::refused(util::message!("editor.keys.inherited", id = id.to_string()))
}

/// The area `node` names moved from `at` to `to` among `ids`, by moving it past the area now at `to` in the rule that writes it, and only where that rule's order is what the screen draws.
fn areas(
    work: &mut Work,
    node: &Node,
    ids: &[AreaId],
    (at, to): (usize, usize),
) -> Result<(), EditError> {
    let written = work.written(node.layer, &node.area)?;
    if !written.is_present() {
        return Err(inherited(&node.area));
    }
    let past = &ids[to];
    let others: Vec<&AreaId> = layout::ops::areas_at(&work.layout, &written.site)
        .iter()
        .map(|area| &area.id)
        .filter(|id| **id != node.area)
        .collect();
    let beside = others
        .iter()
        .position(|id| *id == past)
        .ok_or_else(|| inherited(past))?;
    let index = match to > at {
        true => beside + 1,
        false => beside,
    };
    work.apply(vec![LayoutOp::MoveArea {
        site: written.site.clone(),
        id: node.area.clone(),
        index,
    }])?;
    let mut wanted = ids.to_vec();
    let moved = wanted.remove(at);
    wanted.insert(to, moved);
    let drawn: Vec<AreaId> = work
        .screen()
        .resolved
        .layer(node.layer)
        .map(|layer| {
            layer
                .areas
                .iter()
                .filter(|area| restacks(area))
                .map(|area| area.id.clone())
                .collect()
        })
        .unwrap_or_default();
    match drawn == wanted {
        true => Ok(()),
        false => Err(inherited(&node.area)),
    }
}

/// "Order ▸" for what `node` names, where it is drawn over or under anything: a row for each way, those it cannot go any further greyed.
pub(crate) fn menu(node: &Node) -> Option<MenuEntry> {
    let desktop = reconcile::desktop_now(node.output.as_deref())?;
    let (_, at, len) = Stack::of(&desktop, node)?;
    let entries = Order::ALL
        .into_iter()
        .map(|order| {
            let selection = Selection::of(node.clone());
            let row = MenuEntry::row(order.label(), order.chord().spelled(), move || {
                mode::said(restack(&selection, order))
            });
            match order.target(at, len) == at {
                true => row.disabled(),
                false => row,
            }
        })
        .collect();
    Some(MenuEntry::Sub {
        label: telar::t!("editor.order.menu"),
        entries,
    })
}
