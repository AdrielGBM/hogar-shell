//! The padding tool: the selection's padding tinted along each edge, the box what it holds is laid out in outlined — dashed while the four sides move on their own — a handle on the inner edge of each side, and its padding said beside it, once for all four while they are linked.

use std::sync::Arc;

use telar::{
    Border, Canvas, Color, LayoutStyle, PathData, PathStyle, Point, Rect, RectStyle, RenderNode,
    Stroke, StyledContainer, signal, use_theme,
};

use config::theme::NordTheme;
use surfaces::rects::{self, Node};
use ui::descriptor::Built;

use crate::host::{passthrough, whole};
use crate::popover::handles::{Four, SIDES, Side, side_handle};

use super::{live, marks, target};

/// How opaque the tint over the padding is.
const BAND_ALPHA: f32 = 0.24;
/// The dashes of the outline while the sides are unlinked: drawn, then skipped.
const DASHES: [f32; 2] = [4.0, 3.0];
/// How far a side's tag sits from its handle: along the side, and in from it.
const TAG_OFFSET: (f32, f32) = (30.0, 16.0);

pub(super) fn handles(node: Node, most: f32, linked: bool) -> Built {
    let theme = use_theme::<NordTheme>();
    let name = crate::steps::name_of(&crate::session::Selection::of(node.clone()));
    let color = match linked {
        true => Color::TRANSPARENT,
        false => theme.surface,
    };
    let (four, ends) = live(
        &node,
        most,
        target::padding_of,
        target::padding_ops,
        telar::t!("editor.tool.padded", name = name),
        color,
    );
    let mut items = Vec::new();
    for side in SIDES {
        items.push(band(node.clone(), side, four, theme)?);
    }
    items.push(content(node.clone(), four, linked, theme)?);
    for side in SIDES {
        items.push(side_handle(&node, side, four, signal(false), ends.clone())?);
    }
    let tagged: &[Side] = match linked {
        true => &SIDES[..1],
        false => &SIDES,
    };
    for side in tagged.iter().copied() {
        let value = four.values[side.index()];
        items.push(marks::value_tag(
            node.clone(),
            move || value.get(),
            move |rect| tag_at(side, rect, value.get()),
        )?);
    }
    Ok(Box::new(passthrough(whole(), items)?))
}

/// Where the tag of `side` padded by `width` goes in a box `rect` big: beside its handle, inside the padding's inner edge.
fn tag_at(side: Side, rect: Rect, width: f32) -> (f32, f32) {
    let (x, y) = side.point(rect, width);
    let (along, inward) = TAG_OFFSET;
    match side {
        Side::Top => (x + along, y + inward),
        Side::Bottom => (x + along, y - inward),
        Side::Left => (x + inward * 1.4, y - inward * 1.25),
        Side::Right => (x - inward * 1.4, y - inward * 1.25),
    }
}

/// The box `rect` leaves inside a padding of `[top, right, bottom, left]`.
fn inside(rect: Rect, [top, right, bottom, left]: [f32; 4]) -> Rect {
    Rect::new(
        rect.x + left,
        rect.y + top,
        (rect.width - left - right).max(0.0),
        (rect.height - top - bottom).max(0.0),
    )
}

/// The strip of the box `rect` that the padding of `side` takes, between the strips of the top and bottom for the sides.
fn strip(side: Side, rect: Rect, padding: [f32; 4]) -> Rect {
    let [top, right, bottom, left] = padding;
    let middle = (rect.height - top - bottom).max(0.0);
    match side {
        Side::Top => Rect::new(rect.x, rect.y, rect.width, top),
        Side::Bottom => Rect::new(rect.x, rect.y + rect.height - bottom, rect.width, bottom),
        Side::Left => Rect::new(rect.x, rect.y + top, left, middle),
        Side::Right => Rect::new(rect.x + rect.width - right, rect.y + top, right, middle),
    }
}

fn band(node: Node, side: Side, four: Four, theme: NordTheme) -> Built {
    let values = four.values;
    let tint = theme.accent.with_alpha(BAND_ALPHA);
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| RectStyle::filled(tint, 0.0),
            Vec::new(),
        )?
        .styled_by(move || {
            let rect = rects::rect(&node).unwrap_or_default();
            surfaces::area::at(strip(side, rect, values.map(|value| value.get())))
        })
        .input_transparent(),
    ))
}

/// The box what the selection holds is laid out in: edged solid while the four sides are linked, and in dashes while each moves on its own.
fn content(node: Node, four: Four, linked: bool, theme: NordTheme) -> Built {
    let values = four.values;
    if linked {
        return Ok(Box::new(
            StyledContainer::new(
                LayoutStyle::new(),
                move |_| RectStyle::default().with_border(Border::uniform(theme.accent, 1.0)),
                Vec::new(),
            )?
            .styled_by(move || {
                let rect = rects::rect(&node).unwrap_or_default();
                surfaces::area::at(inside(rect, values.map(|value| value.get())))
            })
            .input_transparent(),
        ));
    }
    let canvas = Canvas::new(whole(), move |_| {
        let rect = rects::rect(&node).unwrap_or_default();
        let held = inside(rect, values.map(|value| value.get()));
        let corners = [
            Point::new(held.x, held.y),
            Point::new(held.x + held.width, held.y),
            Point::new(held.x + held.width, held.y + held.height),
            Point::new(held.x, held.y + held.height),
        ];
        RenderNode::path(
            Arc::new(PathData::polygon(&corners)),
            PathStyle::default()
                .with_stroke(Stroke::new(theme.accent, 1.0).with_dash(&DASHES, 0.0)),
        )
    })?;
    Ok(Box::new(passthrough(whole(), vec![Box::new(canvas)])?))
}
