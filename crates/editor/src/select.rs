//! The selection tool every session layer's mode starts with: a click on a real item selects it, and the selection is outlined where it is.
//!
//! The host is above the raised window, so the tool lays a box that answers the pointer over each area of the edited layer and nowhere else — a click anywhere else still reaches whatever is under the host. Where the click lands is answered from [`surfaces::rects`], the same registry the outline is drawn from.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Border, LayoutStyle, PointerButton, ReactiveList, RectStyle, StyledContainer, use_theme,
};

use config::theme::NordTheme;
use layout::LayerKind;
use surfaces::rects::{self, Node, Part};
use ui::descriptor::Built;

use crate::host::{passthrough, whole};
use crate::mode::Mode;
use crate::session::{self, Selection};

/// How thick the selection outline is.
const OUTLINE: f32 = 2.0;

pub(crate) fn tool(mode: &Mode) -> Built {
    let theme = use_theme::<NordTheme>();
    let (output, layer) = (mode.output.clone(), mode.layer);
    let targets = ReactiveList::with_style(
        whole(),
        move || areas(&output, layer),
        |node: &Node| node.clone(),
        move |node: Node| target(node),
    )?;
    let outline = ReactiveList::with_style(
        whole(),
        || {
            session::selection()
                .get()
                .node()
                .cloned()
                .into_iter()
                .collect()
        },
        |node: &Node| node.clone(),
        move |node: Node| outline(node, theme),
    )?;
    Ok(Box::new(passthrough(
        whole(),
        vec![Box::new(targets), Box::new(outline)],
    )?))
}

fn areas(output: &str, layer: LayerKind) -> Vec<Node> {
    let mut areas: Vec<Node> = Vec::new();
    for (node, _) in rects::on(Some(output), layer) {
        if node.part == Part::Area && !areas.contains(&node) {
            areas.push(node);
        }
    }
    areas
}

/// A box over one area that paints nothing and takes the click, selecting what is under it; a secondary press selects it too, unless it is inside the selection already, and opens its context menu there.
fn target(area: Node) -> Built {
    let pointer = Rc::new(Cell::new((0.0, 0.0)));
    let placed = area.clone();
    let seen = Rc::clone(&pointer);
    let at = {
        let (area, pointer) = (area.clone(), Rc::clone(&pointer));
        move || {
            let origin = rects::rect(&area)?;
            let (x, y) = pointer.get();
            Some((origin.x + x, origin.y + y))
        }
    };
    let pressed = at.clone();
    let asked = area.clone();
    Ok(Box::new(
        StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?
            .styled_by(move || surfaces::area::at(rects::rect(&placed).unwrap_or_default()))
            .on_pointer_move(move |x, y| seen.set((x, y)))
            .on_press(move || {
                let Some(point) = pressed() else {
                    return;
                };
                let placed = rects::on(area.output.as_deref(), area.layer);
                session::select(session::pick(&session::selected(), point, &placed));
            })
            .on_alt_press(move |button| {
                if button != PointerButton::Secondary {
                    return;
                }
                let Some(point) = at() else {
                    return;
                };
                menu_at(&asked, point);
            }),
    ))
}

/// Selects what a secondary press at `point` is on and opens its menu there. What is selected already stays selected where the press is inside it, so a menu asked for on a selected group is the group's area's rather than its smallest child's.
fn menu_at(area: &Node, point: (f32, f32)) {
    let placed = rects::on(area.output.as_deref(), area.layer);
    let current = session::selected();
    let inside = current.node().is_some_and(|node| {
        placed
            .iter()
            .any(|(held, rect)| held == node && rect.contains(point.0, point.1))
    });
    let chosen = match inside {
        true => current,
        false => session::pick(&Selection::None, point, &placed),
    };
    session::select(chosen.clone());
    let Some(node) = chosen.node().cloned() else {
        return;
    };
    let about = match node.part {
        Part::Group(_) => Node::area(node.output.as_deref(), node.layer, &node.area),
        _ => node,
    };
    if let Err(why) = crate::context::open(surfaces::menu::Asked {
        node: about,
        window: LayerKind::Overlay,
        at: Some(point),
    }) {
        tracing::info!("no context menu: {why}");
    }
}

fn outline(node: Node, theme: NordTheme) -> Built {
    let edge = theme.accent;
    let fill = theme.accent.with_alpha(0.12);
    let radius = ui::scale::corner::xs();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| RectStyle::filled(fill, radius).with_border(Border::uniform(edge, OUTLINE)),
            Vec::new(),
        )?
        .styled_by(move || surfaces::area::at(rects::rect(&node).unwrap_or_default()))
        .input_transparent(),
    ))
}

#[cfg(test)]
mod tests {
    use telar::Rect;

    use layout::{AreaId, GroupId, InstanceId};

    use crate::session::Selection;

    use super::*;

    fn bar() -> Node {
        Node::area(Some("DP-1"), LayerKind::Top, &AreaId::new("bar-top"))
    }

    /// A bar, its centre zone and the clock in it, one inside the other, and the end zone beside them.
    fn placed() -> Vec<(Node, Rect)> {
        let center = GroupId::new("center");
        vec![
            (bar(), Rect::new(0.0, 0.0, 1920.0, 34.0)),
            (bar().group(&center), Rect::new(900.0, 0.0, 120.0, 34.0)),
            (
                bar().instance(&center, &InstanceId::new("clock")),
                Rect::new(920.0, 2.0, 80.0, 30.0),
            ),
            (
                bar().group(&GroupId::new("end")),
                Rect::new(1800.0, 0.0, 120.0, 34.0),
            ),
        ]
    }

    /// The smallest thing under the pointer is what a click selects, and clicking it again climbs to what holds it — instance, group, area — and round again.
    #[test]
    fn a_click_selects_the_smallest_thing_and_the_next_click_the_one_around_it() {
        let placed = placed();
        let on_the_clock = (950.0, 10.0);
        let center = GroupId::new("center");
        let mut selected = Selection::None;
        let mut picked = Vec::new();
        for _ in 0..4 {
            selected = session::pick(&selected, on_the_clock, &placed);
            picked.push(selected.clone());
        }
        assert_eq!(
            picked,
            [
                Selection::Instance(bar().instance(&center, &InstanceId::new("clock"))),
                Selection::Group(bar().group(&center)),
                Selection::Area(bar()),
                Selection::Instance(bar().instance(&center, &InstanceId::new("clock"))),
            ]
        );
    }

    #[test]
    fn a_click_elsewhere_starts_again_from_the_smallest_there_and_a_click_on_nothing_selects_nothing()
     {
        let placed = placed();
        let clock = session::pick(&Selection::None, (950.0, 10.0), &placed);
        assert_eq!(
            session::pick(&clock, (1850.0, 10.0), &placed),
            Selection::Group(bar().group(&GroupId::new("end")))
        );
        assert_eq!(
            session::pick(&clock, (500.0, 500.0), &placed),
            Selection::None
        );
    }
}
