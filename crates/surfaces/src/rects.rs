//! Where each area, group and instance the layout placed is on its output right now.
//!
//! Every window of an output is the whole output (TA-1), so a rect read here is the same rect in whichever window asks — where a transient hangs, an outline is drawn or a handle sits. Entries are keyed by what the model calls things, the output, the layer the area was written on and the ids, rather than by module: two clocks are two instances. An entry is added as its node is built and goes with it, so what this answers is always what is on screen.
//!
//! Reactive: read inside an effect or a build, it runs again when an entry comes or goes, and when a rect it read moves.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use telar::{NodeId, Rect, RwSignal, signal, track_layout};

use crate::transient::chips::Site;
use layout::{AreaId, GroupId, InstanceId, LayerKind};

/// One placed thing on one output, by the ids the layout gave it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    pub output: Option<String>,
    /// The layer the area was written on, which is not always the window it is drawn in: an area above fullscreen is drawn in the overlay window.
    pub layer: LayerKind,
    pub area: AreaId,
    pub part: Part,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Area,
    Group(GroupId),
    Instance(GroupId, InstanceId),
}

impl Node {
    pub fn area(output: Option<&str>, layer: LayerKind, area: &AreaId) -> Self {
        Self {
            output: output.map(str::to_string),
            layer,
            area: area.clone(),
            part: Part::Area,
        }
    }

    pub fn group(&self, group: &GroupId) -> Self {
        Self {
            part: Part::Group(group.clone()),
            ..self.clone()
        }
    }

    pub fn instance(&self, group: &GroupId, instance: &InstanceId) -> Self {
        Self {
            part: Part::Instance(group.clone(), instance.clone()),
            ..self.clone()
        }
    }

    /// Whether this is `area` itself or something inside it.
    pub fn is_in(&self, output: Option<&str>, layer: LayerKind, area: &AreaId) -> bool {
        self.output.as_deref() == output && self.layer == layer && self.area == *area
    }
}

/// What a chip tells whatever it opens: the module and options it speaks for, and where it sits.
#[derive(Clone)]
pub struct Chip {
    pub instance: ui::host::Instance,
    pub site: Site,
}

enum Measure {
    Laid(RwSignal<Rect>),
    /// The smallest rect around several nodes: a bar's group, whose instances share their zone with other groups' and have no box of their own.
    Spanning(Vec<RwSignal<Rect>>),
    /// The smallest rect around a repeated group's chips on a bar, which come and go with its list.
    Copies(Copies),
}

impl Measure {
    fn is_alive(&self) -> bool {
        match self {
            Measure::Laid(rect) => rect.is_alive(),
            Measure::Spanning(rects) => rects.iter().any(RwSignal::is_alive),
            Measure::Copies(copies) => copies.is_alive(),
        }
    }

    fn get(&self) -> Rect {
        match self {
            Measure::Laid(rect) => rect.get(),
            Measure::Spanning(rects) => rects
                .iter()
                .filter(|rect| rect.is_alive())
                .map(|rect| rect.get())
                .reduce(union)
                .unwrap_or_default(),
            Measure::Copies(copies) => copies
                .with(|held| held.values().copied().collect::<Vec<_>>())
                .into_iter()
                .filter(|rect| rect.is_alive())
                .map(|rect| rect.get())
                .reduce(union)
                .unwrap_or_default(),
        }
    }

    fn peek(&self) -> Rect {
        match self {
            Measure::Laid(rect) => rect.peek(),
            Measure::Spanning(rects) => rects
                .iter()
                .filter(|rect| rect.is_alive())
                .map(|rect| rect.peek())
                .reduce(union)
                .unwrap_or_default(),
            Measure::Copies(copies) => copies
                .peek()
                .values()
                .filter(|rect| rect.is_alive())
                .map(|rect| rect.peek())
                .reduce(union)
                .unwrap_or_default(),
        }
    }
}

struct Entry {
    token: u64,
    node: Node,
    measure: Measure,
    chip: Option<Chip>,
    /// Whether it says where its node is, rather than only where the chip that node holds is.
    placed: bool,
}

thread_local! {
    static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(0) };
    static REVISION: RwSignal<u64> = telar::detached(|| signal(0));
}

/// Records where the layout node `laid` lands as `node`, for as long as the owner building it lives, and hands back the rect it is tracked by.
pub fn track(node: Node, laid: NodeId) -> Option<RwSignal<Rect>> {
    let rect = track_layout(laid)?;
    enter(node, Measure::Laid(rect), None, true);
    Some(rect)
}

/// [`track`] for a chip on a bar, which also says what the chip opens and where; `rect` is the chip's own tracked rect.
pub fn track_chip(node: Node, rect: RwSignal<Rect>, chip: Chip) {
    enter(node, Measure::Laid(rect), Some(chip), true);
}

/// What the chip at `node` opens and where the chip itself is, for a chip whose node [`track`] already records by the box it was placed in, which can be larger than the chip: only what a press or a command opens finds it.
pub fn track_opener(node: Node, rect: RwSignal<Rect>, chip: Chip) {
    enter(node, Measure::Laid(rect), Some(chip), false);
}

/// Records `node` as the smallest rect around `rects`, for a group with no box of its own.
pub fn track_spanning(node: Node, rects: Vec<RwSignal<Rect>>) {
    if !rects.is_empty() {
        enter(node, Measure::Spanning(rects), None, true);
    }
}

/// The rects of a repeated group's chips, by copy, as copies are built and dropped.
pub type Copies = RwSignal<BTreeMap<InstanceId, RwSignal<Rect>>>;

/// Records `node` as the smallest rect around the chips `copies` holds at the time it is read, for a repeated group on a bar.
pub fn track_copies(node: Node, copies: Copies) {
    enter(node, Measure::Copies(copies), None, true);
}

fn enter(node: Node, measure: Measure, chip: Option<Chip>, placed: bool) {
    let token = NEXT.with(|next| next.replace(next.get() + 1));
    ENTRIES.with(|entries| {
        let mut entries = entries.borrow_mut();
        entries.retain(|entry| entry.measure.is_alive());
        entries.push(Entry {
            token,
            node,
            measure,
            chip,
            placed,
        });
    });
    changed();
    telar::on_cleanup(move || {
        ENTRIES.with(|entries| entries.borrow_mut().retain(|entry| entry.token != token));
        changed();
    });
}

fn changed() {
    REVISION.with(|revision| revision.update(|n| *n = n.wrapping_add(1)));
}

fn subscribe() {
    REVISION.with(|revision| revision.with(|_| ()));
}

/// Where `node` is. `None` while it is not built — an area that failed, an instance a Smart Stack is not showing.
pub fn rect(node: &Node) -> Option<Rect> {
    subscribe();
    ENTRIES.with(|entries| {
        entries
            .borrow()
            .iter()
            .rev()
            .find(|entry| entry.placed && entry.node == *node && entry.measure.is_alive())
            .map(|entry| entry.measure.get())
    })
}

/// Everything placed on `output` from `layer`, areas first, in the order it was built.
pub fn on(output: Option<&str>, layer: LayerKind) -> Vec<(Node, Rect)> {
    subscribe();
    ENTRIES.with(|entries| {
        entries
            .borrow()
            .iter()
            .filter(|entry| entry.placed)
            .filter(|entry| entry.node.output.as_deref() == output && entry.node.layer == layer)
            .filter(|entry| entry.measure.is_alive())
            .map(|entry| (entry.node.clone(), entry.measure.get()))
            .collect()
    })
}

/// Where the instance `id` is on `output`, whichever layer, area and group hold it.
pub fn instance(output: Option<&str>, id: &InstanceId) -> Option<(Node, Rect)> {
    subscribe();
    ENTRIES.with(|entries| {
        entries
            .borrow()
            .iter()
            .rev()
            .filter(|entry| entry.placed)
            .filter(|entry| entry.node.output.as_deref() == output)
            .filter(|entry| matches!(&entry.node.part, Part::Instance(_, held) if held == id))
            .find(|entry| entry.measure.is_alive())
            .map(|entry| (entry.node.clone(), entry.measure.get()))
    })
}

/// Whether `at` is over one of `area`'s instances, which is what tells its empty space from the rest of it. Read without subscribing: it answers a gesture, not a build.
pub fn over_instance(
    output: Option<&str>,
    layer: LayerKind,
    area: &AreaId,
    at: (f32, f32),
) -> bool {
    ENTRIES.with(|entries| {
        entries.borrow().iter().any(|entry| {
            entry.placed
                && matches!(entry.node.part, Part::Instance(..))
                && entry.node.is_in(output, layer, area)
                && entry.measure.is_alive()
                && entry.measure.peek().contains(at.0, at.1)
        })
    })
}

/// Every chip, on a bar or anywhere else, with where the chip itself is now, in the order they were built. Read without subscribing, for what a press or a command opens.
pub(crate) fn chips() -> Vec<(Node, Chip, Rect)> {
    ENTRIES.with(|entries| {
        entries
            .borrow()
            .iter()
            .filter(|entry| entry.measure.is_alive())
            .filter_map(|entry| {
                Some((
                    entry.node.clone(),
                    entry.chip.clone()?,
                    entry.measure.peek(),
                ))
            })
            .collect()
    })
}

fn union(a: Rect, b: Rect) -> Rect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect::new(left, top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    /// A reader is told when an entry arrives, when the rect it reads moves, and when the node goes.
    #[test]
    fn a_reader_hears_an_entry_arrive_move_and_go() {
        let _scope = telar::owner_scope();
        let node = Node::area(Some("DP-1"), LayerKind::Top, &AreaId::new("bar"))
            .group(&GroupId::new("end"));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let heard = Rc::clone(&seen);
        let reading = node.clone();
        let _reader = telar::effect(move || heard.borrow_mut().push(rect(&reading)));

        let built = telar::owner_scope();
        let owner = built.id();
        let (a, b) = (
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(20.0, 0.0, 10.0, 10.0),
        );
        let first = signal(a);
        track_spanning(node.clone(), vec![first, signal(b)]);
        drop(built);
        first.set(Rect::new(40.0, 0.0, 10.0, 10.0));
        telar::dispose_owner(owner);

        assert_eq!(
            *seen.borrow(),
            [
                None,
                Some(Rect::new(0.0, 0.0, 30.0, 10.0)),
                Some(Rect::new(20.0, 0.0, 30.0, 10.0)),
                None
            ]
        );
    }
}
