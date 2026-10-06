//! The selection tool every session layer's mode starts with: a click on a real item selects it and a double-click customizes it, the selection is outlined where it is with its size beneath it, what a click would select is outlined thinly as the pointer passes over it, and a drag says at the pointer what it would do.
//!
//! The host is above the raised window, so the tool lays a box that answers the pointer over each area of the edited layer and nowhere else — a click anywhere else still reaches whatever is under the host. Where the click lands is answered from [`surfaces::rects`], the same registry the outlines are drawn from.

use std::cell::Cell;
use std::time::{Duration, Instant};

use telar::{
    Border, Color, LayoutStyle, ReactiveList, Rect, RectStyle, RwSignal, StyledContainer, Text,
    box_item, detached, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::{LayerKind, ResolvedAreaKind};
use surfaces::reconcile;
use surfaces::rects::{self, Node, Part};
use ui::descriptor::Built;

use crate::host::{passthrough, see_through, whole};
use crate::mode::{self, Mode, said};
use crate::modes::gesture::{self, Hint, THRESHOLD};
use crate::modes::grid;
use crate::session::{self, Selection};

/// How thick the selection outline is.
const OUTLINE: f32 = 2.0;

/// How opaque the outline of what a click would select is, against the selection's full accent.
const HOVER_ALPHA: f32 = 0.6;

/// The longest gap between two presses that still reads as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How far below the selection its size tag hangs.
const SIZE_TAG_DROP: f32 = 6.0;

/// How far from the pointer a drag's tag sits, along each axis.
const POINTER_TAG_OFFSET: f32 = 14.0;

/// How near the right edge and the foot of the screen the pointer comes before the drag's tag goes over to its other side.
const POINTER_TAG_FLIP: (f32, f32) = (150.0, 44.0);

/// Where the pointer last moved over a target of the edited layer, and which target saw it.
#[derive(Clone, Debug, PartialEq)]
struct Hovering {
    target: u64,
    over: Node,
    point: (f32, f32),
}

thread_local! {
    static HOVER: RwSignal<Option<Hovering>> = detached(|| signal(None));
    static TARGETS: Cell<u64> = const { Cell::new(0) };
    static LAST_PRESS: Cell<Option<(Instant, (f32, f32))>> = const { Cell::new(None) };
}

/// Mounted after every other tool so the quick bar is never under another tool's handles or bodies.
pub(crate) fn install() {
    for layer in LayerKind::ALL {
        crate::host::add_tool(layer, crate::quick::tool);
    }
    crate::quick::install();
}

pub(crate) fn tool(mode: &Mode) -> Built {
    let theme = use_theme::<NordTheme>();
    let (output, layer) = (mode.output.clone(), mode.layer);
    let targets = ReactiveList::with_style(
        whole(),
        move || areas(&output, layer),
        |node: &Node| node.clone(),
        move |node: Node| target(node),
    )?;
    let hover = ReactiveList::with_style(
        whole(),
        || hovered().into_iter().collect(),
        |node: &Node| node.clone(),
        move |node: Node| hover_outline(node, theme),
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
    let size = ReactiveList::with_style(
        whole(),
        || sized().into_iter().collect(),
        |(said, rect): &(String, Rect)| format!("{said} {rect:?}"),
        move |(said, rect): (String, Rect)| {
            pointer_tag(said, theme.accent, move |laid| {
                (
                    rect.x + (rect.width - laid.width) / 2.0,
                    rect.y + rect.height + SIZE_TAG_DROP,
                )
            })
        },
    )?;
    let why = ReactiveList::with_style(
        whole(),
        || hidden_note().into_iter().collect(),
        |(said, rect): &(String, Rect)| format!("{said} {rect:?}"),
        move |(said, rect): (String, Rect)| {
            pointer_tag(said, theme.accent, move |laid| {
                (
                    rect.x + (rect.width - laid.width) / 2.0,
                    (rect.y - laid.height - SIZE_TAG_DROP).max(0.0),
                )
            })
        },
    )?;
    let screen = mode.output.clone();
    let hint = ReactiveList::with_style(
        whole(),
        || gesture::hint().get().into_iter().collect(),
        |hint: &Hint| format!("{hint:?}"),
        move |hint: Hint| drag_marks(hint, screen_size(&screen), theme),
    )?;
    Ok(Box::new(passthrough(
        whole(),
        vec![
            see_through(targets)?,
            see_through(hover)?,
            see_through(outline)?,
            see_through(size)?,
            see_through(why)?,
            see_through(hint)?,
        ],
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
    let placed = area.clone();
    Ok(Box::new(
        gesture::pressable(
            StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), Vec::new())?,
            move || Some(area.clone()),
        )
        .styled_by(move || surfaces::area::at(rects::rect(&placed).unwrap_or_default())),
    ))
}

/// `target` outlining, as the pointer moves over it, what a press there would select. `node` is anything on the screen and layer the target is over.
pub(crate) fn hoverable(
    target: StyledContainer,
    node: impl Fn() -> Option<Node> + 'static,
) -> StyledContainer {
    let token = TARGETS.with(util::serial::next_serial);
    target
        .on_pointer_move(move |_, _| {
            if let (Some(over), Some(point)) = (node(), surfaces::menu::pointer()) {
                hover_over(Hovering {
                    target: token,
                    over,
                    point,
                });
            }
        })
        .on_hover(move |inside| {
            if !inside {
                hover_left(token);
            }
        })
}

fn hover_over(hovering: Hovering) {
    HOVER.with(|hover| {
        if hover.peek().as_ref() != Some(&hovering) {
            hover.set(Some(hovering));
        }
    });
}

/// The pointer left the target `token`: what it outlined goes, unless another target has taken the pointer since.
fn hover_left(token: u64) {
    HOVER.with(|hover| {
        if hover
            .peek()
            .is_some_and(|hovering| hovering.target == token)
        {
            hover.set(None);
        }
    });
}

/// What the hover outline is drawn round now: what a press where the pointer is would select, but never during a drag and never the selection itself.
fn hovered() -> Option<Node> {
    if gesture::dragging() {
        return None;
    }
    let hovering = HOVER.with(RwSignal::get)?;
    if !mode::editing(&hovering.over) {
        return None;
    }
    let current = session::selection().get();
    let point = hovering.point;
    let placed = rects::on(hovering.over.output.as_deref(), hovering.over.layer);
    let on_selection = current.node().is_some_and(|selected| {
        placed
            .iter()
            .any(|(node, rect)| node == selected && rect.contains(point.0, point.1))
    });
    outlined_on_hover(
        &current,
        session::pick(&current, point, &placed),
        on_selection,
    )
}

/// What the hover outline shows with `current` selected, where a press would select `picked`, the pointer `on_selection` or not: nothing where the press would keep the selection, or would only climb from it to what holds it, since the selection's own outline already says where that is.
fn outlined_on_hover(current: &Selection, picked: Selection, on_selection: bool) -> Option<Node> {
    let picked = picked.node()?.clone();
    match current.node() {
        Some(selected) if *selected == picked => None,
        Some(selected) if on_selection && holds(&picked, selected) => None,
        _ => Some(picked),
    }
}

/// Whether `outer` is the area or the group `inner` is in.
fn holds(outer: &Node, inner: &Node) -> bool {
    let same_area =
        outer.output == inner.output && outer.layer == inner.layer && outer.area == inner.area;
    same_area
        && match (&outer.part, &inner.part) {
            (Part::Area, Part::Group(_) | Part::Instance(..)) => true,
            (Part::Group(group), Part::Instance(held, _)) => group == held,
            _ => false,
        }
}

/// Selects what a press at `point` on the area `area` names is on: the smallest thing there, or on a second press the thing around it. The second press of a double-click instead customizes what the first selected.
pub(crate) fn press_at(area: &Node, point: (f32, f32)) {
    mode::clear_refusal();
    let current = session::selected();
    if pressed_again(point) && current != Selection::None {
        said(crate::popover::open_for(&current));
        return;
    }
    let placed = rects::on(area.output.as_deref(), area.layer);
    session::select(session::pick(&current, point, &placed));
}

/// Whether a press at `point` is the second of a double-click: soon enough after the last press and where it was. The pair is spent by it, so a third press starts again.
fn pressed_again(point: (f32, f32)) -> bool {
    let now = Instant::now();
    LAST_PRESS.with(|last| {
        let again = last.get().is_some_and(|(then, at)| {
            now.duration_since(then) <= DOUBLE_CLICK
                && (at.0 - point.0).abs() <= THRESHOLD
                && (at.1 - point.1).abs() <= THRESHOLD
        });
        last.set((!again).then_some((now, point)));
        again
    })
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
        mode::refuse(why);
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

fn hover_outline(node: Node, theme: NordTheme) -> Built {
    let edge = theme.accent.with_alpha(HOVER_ALPHA);
    let radius = ui::scale::corner::xs();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            move |_| {
                RectStyle::filled(Color::TRANSPARENT, radius)
                    .with_border(Border::uniform(edge, 1.0))
            },
            Vec::new(),
        )?
        .styled_by(move || surfaces::area::at(rects::rect(&node).unwrap_or_default()))
        .input_transparent(),
    ))
}

/// The size tag under the selected group or instance, and where the selection is: none for an area, and none during a drag.
fn sized() -> Option<(String, Rect)> {
    if gesture::dragging() {
        return None;
    }
    let (Selection::Group(node) | Selection::Instance(node)) = session::selection().get() else {
        return None;
    };
    let rect = rects::rect(&node)?;
    Some((size_tag(&node, rect), rect))
}

pub(crate) fn size_tag_shown() -> bool {
    sized().is_some()
}

/// Where the selection is and why it is dim, while the area it is in is hidden by its `visible`: drawn at 30 % for its mode and selectable there.
fn hidden_note() -> Option<(String, Rect)> {
    if gesture::dragging() {
        return None;
    }
    let node = session::selection().get().node().cloned()?;
    let expr = surfaces::expressions::hidden_by(&node)?;
    let rect = rects::rect(&node)?;
    Some((telar::t!("editor.select.hidden", expr = expr), rect))
}

/// What the size tag under `node`, drawn at `rect`, says: the cells it spans where it sits on a grid's cells, then its size in pixels.
fn size_tag(node: &Node, rect: Rect) -> String {
    let (width, height) = (rect.width.round(), rect.height.round());
    match spanned(node) {
        Some(cells) => telar::t!(
            "editor.pointer.cells",
            cols = cells.cols,
            rows = cells.rows,
            width = width,
            height = height
        ),
        None => telar::t!("editor.pointer.pixels", width = width, height = height),
    }
}

/// The cells `node` spans: a group's on a grid, or a loose group's for the instance in it; `None` for anything else, an instance a container lays out included.
fn spanned(node: &Node) -> Option<grid::Cells> {
    let desktop = reconcile::desktop(node.output.as_deref())?;
    let area = desktop
        .resolved
        .layer(node.layer)?
        .areas
        .iter()
        .find(|area| area.id == node.area)?
        .clone();
    if !matches!(area.kind, ResolvedAreaKind::Grid { .. }) {
        return None;
    }
    let (group, loose_only) = match &node.part {
        Part::Group(group) => (group, false),
        Part::Instance(group, _) => (group, true),
        Part::Area => return None,
    };
    let group = area.groups.iter().find(|held| held.id == *group)?;
    if loose_only && group.arrange.is_some() {
        return None;
    }
    grid::cells_of(group)
}

fn screen_size(output: &str) -> (f32, f32) {
    reconcile::desktop(Some(output)).map_or((0.0, 0.0), |desktop| desktop.size)
}

/// Where the top left corner of a drag's tag `size` big goes with the pointer at `pointer` on a screen `screen` big: below and to the right of the pointer, and over to its left near the right edge and above it near the foot, so it stays on the screen.
fn drag_tag_at(pointer: (f32, f32), size: (f32, f32), screen: (f32, f32)) -> (f32, f32) {
    let along = |at: f32, extent: f32, room: f32, flip: f32| match at > room - flip {
        true => at - POINTER_TAG_OFFSET - extent,
        false => at + POINTER_TAG_OFFSET,
    };
    (
        along(pointer.0, size.0, screen.0, POINTER_TAG_FLIP.0),
        along(pointer.1, size.1, screen.1, POINTER_TAG_FLIP.1),
    )
}

/// What a drag shows at the pointer: the translucent ghost of what it carries, and the tag saying where it would land.
fn drag_marks(hint: Hint, screen: (f32, f32), theme: NordTheme) -> Built {
    let mut marks = Vec::new();
    for guide in hint.guides {
        let line = Rect::new(
            guide.x,
            guide.y,
            guide.width.max(1.0),
            guide.height.max(1.0),
        );
        marks.push(Box::new(
            StyledContainer::new(
                surfaces::area::at(line),
                move |_| RectStyle::filled(theme.accent, 0.0),
                Vec::new(),
            )?
            .input_transparent(),
        ) as Box<dyn telar::LayoutItem>);
    }
    if let Some(ghost) = hint.ghost {
        marks.push(Box::new(
            StyledContainer::new(
                surfaces::area::at(ghost),
                move |_| {
                    RectStyle::filled(theme.surface.with_alpha(0.5), ui::scale::corner::md())
                        .with_border(Border::uniform(theme.highlight_high, 1.0))
                },
                Vec::new(),
            )?
            .input_transparent(),
        ) as Box<dyn telar::LayoutItem>);
    }
    if let Some(said) = hint.tag {
        let pointer = hint.pointer;
        marks.push(pointer_tag(said, theme.highlight_med, move |laid| {
            drag_tag_at(pointer, (laid.width, laid.height), screen)
        })?);
    }
    Ok(Box::new(passthrough(whole(), marks)?))
}

/// A small plate saying `said`, edged in `edge`, its top left corner wherever `place` puts it given the size it is laid out at.
fn pointer_tag(said: String, edge: Color, place: impl Fn(Rect) -> (f32, f32) + 'static) -> Built {
    let theme = use_theme::<NordTheme>();
    let text = box_item(Text::new(
        move || said.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.text),
    )?);
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .absolute()
                .inset_start(0.0)
                .inset_top(0.0)
                .padding_horizontal(ui::scale::space::sm())
                .padding_vertical(ui::scale::space::xs()),
            move |_| {
                RectStyle::filled(theme.surface, ui::scale::corner::xs())
                    .with_border(Border::uniform(edge, 1.0))
            },
            vec![text],
        )?
        .with_transform(move |laid| {
            let (x, y) = place(laid);
            Some([1.0, 0.0, 0.0, 1.0, x - laid.x, y - laid.y])
        })
        .input_transparent(),
    ))
}
