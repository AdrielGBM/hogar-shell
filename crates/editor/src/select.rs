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

use crate::host::{passthrough, see_through, whole};
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
        vec![see_through(targets)?, see_through(outline)?],
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
                if let Some(point) = pressed() {
                    press_at(&area, point);
                }
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

/// Selects what a press at `point` on the area `area` names is on: the smallest thing there, or on a second press the thing around it.
pub(crate) fn press_at(area: &Node, point: (f32, f32)) {
    crate::mode::clear_refusal();
    let placed = rects::on(area.output.as_deref(), area.layer);
    session::select(session::pick(&session::selected(), point, &placed));
}

/// Selects what a secondary press at `point` is on and opens its menu there. What is selected already stays selected where the press is inside it, so a menu asked for on a selected group is the group's area's rather than its smallest child's.
pub(crate) fn menu_at(area: &Node, point: (f32, f32)) {
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
        crate::mode::refuse(why);
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
