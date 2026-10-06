//! The radius tool: the selection outlined with its corners as they are rounded, a handle over each corner's arc, and its radius said beside it — once for all four while they are linked.

use telar::{
    Border, BorderRadius, Color, LayoutStyle, Rect, RectStyle, StyledContainer, signal, use_theme,
};

use config::theme::NordTheme;
use surfaces::rects::{self, Node};
use ui::descriptor::Built;

use crate::host::{passthrough, whole};
use crate::popover::handles::{CORNERS, Corner, Four, NEAREST, corner_handle};

use super::{live, marks, target};

/// How far past its handle a corner's tag sits, across and down from the corner.
const TAG_OFFSET: (f32, f32) = (20.0, 14.0);

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
        target::radius_of,
        target::radius_ops,
        telar::t!("editor.tool.rounded", name = name),
        color,
    );
    let mut items = vec![outline(node.clone(), four, theme)?];
    for corner in CORNERS {
        items.push(corner_handle(
            &node,
            corner,
            four,
            signal(false),
            ends.clone(),
        )?);
    }
    let tagged: &[Corner] = match linked {
        true => &CORNERS[..1],
        false => &CORNERS,
    };
    for corner in tagged.iter().copied() {
        let value = four.values[corner.index()];
        items.push(marks::value_tag(
            node.clone(),
            move || value.get(),
            move |rect| tag_at(corner, rect, value.get(), most),
        )?);
    }
    Ok(Box::new(passthrough(whole(), items)?))
}

/// Where the tag of `corner` at `radius` goes in a box `rect` big: beyond its handle, away from the corner.
fn tag_at(corner: Corner, rect: Rect, radius: f32, most: f32) -> (f32, f32) {
    let (x, y) = corner.at(rect);
    let (sx, sy) = corner.signs();
    let off = radius.min(most).max(NEAREST.min(most));
    (x + sx * (off + TAG_OFFSET.0), y + sy * (off + TAG_OFFSET.1))
}

/// The selection's edge, rounded as its corners are now.
fn outline(node: Node, four: Four, theme: NordTheme) -> Built {
    let values = four.values;
    let placed = node.clone();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| {
                let [top_left, top_right, bottom_right, bottom_left] =
                    values.map(|value| value.get());
                let rect = rects::rect(&node).unwrap_or_default();
                RectStyle::default()
                    .with_border(Border::uniform(theme.accent, 1.0))
                    .with_radius(surfaces::look::within(
                        BorderRadius {
                            top_left,
                            top_right,
                            bottom_right,
                            bottom_left,
                        },
                        rect,
                    ))
            },
            Vec::new(),
        )?
        .styled_by(move || surfaces::area::at(rects::rect(&placed).unwrap_or_default()))
        .input_transparent(),
    ))
}
