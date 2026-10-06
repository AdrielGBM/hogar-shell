//! The background mode's tools (TA-5): wallpaper regions split, joined and resized on the screen itself, each with its own picture, fit and transition, and textures over them.
//!
//! **Distinct hotspots (F-7).** Splitting and joining are never the same place: a region's two split buttons sit at its centre, one for each way it can be cut, and a join button sits on the edge two regions share, beside the grip that moves that edge. A split button pressed splits the region in half; dragged, it places the cut where it is let go. The grip moves the edge between neighbours, and every region on either side of it follows, so no gap opens and nothing overlaps.
//!
//! **Every drag has a key (WCAG 2.5.7).** `s` splits the selected region side by side and Shift+S top and bottom, Alt+arrows join it with the region that way, and the generic Shift+arrows and Ctrl+arrows move and resize it as the grips do, neighbours following ([`super::regions`]). A focused grip moves with the arrows. `w` switches editing the workspace that is up alone on and off ([`crate::variant`]); `t` lays a texture over the selected region, as the toolbar's button does, and `n` and `[` `]` slice a texture's image and turn its gradient ([`super::texture`]).
//!
//! **One undo entry each.** A split, a join, an edge moved and a texture added or taken away are each one edit.

use std::rc::Rc;

use telar::{
    Children, Cursor, LayoutError, LayoutItem, LayoutStyle, ReactiveList, Rect, Transaction,
    effect, signal,
};

use layout::{
    AreaId, Fit, LayerKind, Layout, LayoutOp, ResolvedArea, ResolvedAreaKind, Transition,
};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::Node;
use ui::descriptor::Built;

use crate::context;
use crate::host::{self, HOTSPOT, passthrough, whole};
use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::{Mode, said};
use crate::popover::area::{chosen, rect_rows, variants};
use crate::popover::rows::{self, label};
use crate::popover::{AreaDraft, Inspector, help, kind_field, kind_read};
use crate::session::{self, Edit, EditError, Selection};
use crate::snap;
use crate::written::Work;

use super::gesture;
use super::regions::{self, Cut, Plan, Tile};
use super::texture;

/// How far a join button sits along its edge from the grip in the middle of it.
const APART: f32 = 36.0;
/// How big the grip on a shared edge is across.
const GRIP: f32 = 14.0;

/// The toolbar button that lays a texture over the selected region, in every mode with the region tools.
pub(crate) const TEXTURE_BUTTON: crate::host::StripButton = (
    || telar::t!("editor.texture.add"),
    || said(texture_selected(&session::selected())),
);

pub(crate) fn install() {
    crate::popover::add_area_tool("wallpaper_region", region_tool);
    crate::popover::add_area_tool("texture", texture::tool);
    crate::host::add_tool(LayerKind::Background, tool);
    crate::host::add_toolbar_button(LayerKind::Background, TEXTURE_BUTTON);
    crate::host::set_add(LayerKind::Background, || {
        texture_selected(&session::selected())
    });
    context::add_area_rows("wallpaper_region", region_rows);
    keys::add_key_op(
        "wallpaper_region",
        KeyOp {
            name: "region-split",
            keys: vec![Chord::char('s')],
            label: || telar::t!("editor.keys.op.region-split"),
            run: Run::Step(split_side_by_side),
        },
    );
    keys::add_key_op(
        "wallpaper_region",
        KeyOp {
            name: "region-split-stacked",
            keys: vec![Chord::char('s').shift()],
            label: || telar::t!("editor.keys.op.region-split-stacked"),
            run: Run::Step(split_stacked),
        },
    );
    keys::add_key_op(
        "wallpaper_region",
        KeyOp {
            name: "region-join",
            keys: keys::arrows_with(Chord::alt),
            label: || telar::t!("editor.keys.op.region-join"),
            run: Run::Toward(join_toward),
        },
    );
    keys::add_key_op(
        "wallpaper_region",
        KeyOp {
            name: "texture-add",
            keys: vec![Chord::char('t')],
            label: || telar::t!("editor.keys.op.texture-add"),
            run: Run::Act(texture_selected),
        },
    );
    keys::add_key_op(
        "texture",
        KeyOp {
            name: "texture-nine-slice",
            keys: vec![Chord::char('n')],
            label: || telar::t!("editor.keys.op.texture-nine-slice"),
            run: Run::Step(texture::nine_slice),
        },
    );
    keys::add_key_op(
        "texture",
        KeyOp {
            name: "texture-gradient",
            keys: vec![Chord::char(']')],
            label: || telar::t!("editor.keys.op.texture-gradient"),
            run: Run::Step(texture::turn),
        },
    );
    keys::add_key_op(
        "texture",
        KeyOp {
            name: "texture-gradient-back",
            keys: vec![Chord::char('[')],
            label: || telar::t!("editor.keys.op.texture-gradient-back"),
            run: Run::Step(texture::turn_back),
        },
    );
}

fn split_by(selection: &Selection, layout: &Layout, cut: Cut) -> Result<Vec<LayoutOp>, EditError> {
    let node = selection.node().ok_or_else(EditError::nothing)?;
    Plan::for_node(layout, node)?.split_in_half(&node.area, cut)
}

/// `s`: the selected region split in half, side by side.
pub(crate) fn split_side_by_side(
    selection: &Selection,
    layout: &Layout,
) -> Result<Vec<LayoutOp>, EditError> {
    split_by(selection, layout, Cut::SideBySide)
}

/// Shift+S: the selected region split in half, one above the other.
pub(crate) fn split_stacked(
    selection: &Selection,
    layout: &Layout,
) -> Result<Vec<LayoutOp>, EditError> {
    split_by(selection, layout, Cut::Stacked)
}

/// Alt+arrow: the selected region joined with the one sharing its whole edge that way.
pub(crate) fn join_toward(
    selection: &Selection,
    layout: &Layout,
    direction: keys::Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let node = selection.node().ok_or_else(EditError::nothing)?;
    Plan::for_node(layout, node)?.join_toward(&node.area, direction)
}

/// A keyboard move or resize of the region `node` names, its neighbours following ([`regions::Plan::move_step`], [`regions::Plan::resize_step`]).
pub(crate) fn stepped(
    node: &Node,
    layout: &Layout,
    direction: keys::Direction,
    resize: bool,
) -> Result<Vec<LayoutOp>, EditError> {
    let plan = Plan::for_node(layout, node)?;
    match resize {
        true => plan.resize_step(&node.area, direction),
        false => plan.move_step(&node.area, direction),
    }
}

/// What the background mode lays over the edited screen: every region's split buttons, every shared edge's grip and join button.
pub(crate) fn tool(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let building = output.clone();
    let list = ReactiveList::with_style(
        whole(),
        move || controls(&output, layer),
        |control: &Control| control.clone(),
        move |control: Control| control.build(&building, layer),
    )?;
    Ok(Box::new(passthrough(whole(), vec![Box::new(list)])?))
}

/// One control over the edited regions, named by what it acts on so it stays the same node while a drag previews the regions around it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    Split {
        id: AreaId,
        cut: Cut,
    },
    Join {
        first: AreaId,
        second: AreaId,
    },
    Edge {
        cut: Cut,
        before: Vec<AreaId>,
        after: Vec<AreaId>,
        /// How far it can move, in grid steps: a grip is built again when an edit elsewhere changes how far.
        range: (u32, u32),
    },
}

/// The regions of `layer` on `output` as its windows draw them now, each group measured in one box.
fn drawn(output: &str, layer: LayerKind) -> Option<(Desktop, Vec<Tile>)> {
    let desktop = reconcile::desktop(Some(output))?;
    let tiles = regions::tiles_of(&desktop.resolved, layer);
    Some((desktop, tiles))
}

fn controls(output: &str, layer: LayerKind) -> Vec<Control> {
    let Some((_, all)) = drawn(output, layer) else {
        return Vec::new();
    };
    let mut controls = Vec::new();
    for within in [layout::Within::Output, layout::Within::Usable] {
        let tiles: Vec<Tile> = all
            .iter()
            .filter(|tile| tile.within == within)
            .cloned()
            .collect();
        for tile in &tiles {
            for cut in [Cut::SideBySide, Cut::Stacked] {
                controls.push(Control::Split {
                    id: tile.id.clone(),
                    cut,
                });
            }
        }
        for (at, first) in tiles.iter().enumerate() {
            for second in &tiles[at + 1..] {
                if regions::join(first.rect, second.rect).is_some() {
                    controls.push(Control::Join {
                        first: first.id.clone(),
                        second: second.id.clone(),
                    });
                }
            }
        }
        for line in regions::lines(&tiles) {
            let (low, high) = regions::range(&tiles, &line);
            let steps = |at: f32| (at / regions::GRID).round().max(0.0) as u32;
            controls.push(Control::Edge {
                cut: line.cut,
                before: line.before,
                after: line.after,
                range: (steps(low), steps(high)),
            });
        }
    }
    controls
}

/// Where the region `node` names is on screen now, in pixels, and the box its fractions are of.
fn placed(node: &Node) -> Option<(Rect, Rect)> {
    let (desktop, tiles) = drawn(node.output.as_deref()?, node.layer)?;
    let tile = tiles.into_iter().find(|tile| tile.id == node.area)?;
    let bounds = desktop.reserved.box_of(tile.within, desktop.size);
    Some((surfaces::area::within(tile.rect, bounds), bounds))
}

/// The fraction of `bounds` a pointer at `point` is at along the axis a `cut` line is placed on.
fn fraction(cut: Cut, bounds: Rect, point: (f32, f32)) -> f32 {
    match cut {
        Cut::SideBySide => (point.0 - bounds.x) / bounds.width.max(1.0),
        Cut::Stacked => (point.1 - bounds.y) / bounds.height.max(1.0),
    }
}

impl Control {
    fn build(&self, output: &str, layer: LayerKind) -> Result<Box<dyn LayoutItem>, LayoutError> {
        match self {
            Control::Split { id, cut } => split_button(Node::area(Some(output), layer, id), *cut),
            Control::Join { first, second } => {
                join_button(Node::area(Some(output), layer, first), second)
            }
            Control::Edge {
                cut, before, after, ..
            } => edge_grip(output, layer, (*cut, before.clone(), after.clone())),
        }
    }
}

/// A region's split button for one cut: pressed, it splits the region in half; dragged, the cut follows the pointer and lands where it is let go.
fn split_button(region: Node, cut: Cut) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let label = telar::t!("editor.region.split", name = region.area.to_string());
    let edit = Edit::new(label.clone());
    let side = match cut {
        Cut::SideBySide => -1.0,
        Cut::Stacked => 1.0,
    };
    let at = {
        let region = region.clone();
        move || {
            let (rect, _) = placed(&region)?;
            Some((
                rect.x + rect.width / 2.0 + side * (HOTSPOT / 2.0 + 4.0),
                rect.y + rect.height / 2.0,
            ))
        }
    };
    let corner = at.clone();
    let (pressed, dragged) = (region.clone(), region);
    let previewing = edit.clone();
    Ok(Box::new(gesture::drag(
        host::hotspot(host::cut_mark(cut == Cut::SideBySide)?, at)?
            .cursor(Cursor::Pointer)
            .on_press(move || {
                let done = Plan::for_node(&session::draft().peek(), &pressed)
                    .and_then(|plan| plan.split_in_half(&pressed.area, cut))
                    .and_then(|ops| context::commit(label.clone(), ops));
                said(done);
            }),
        edit.transaction(),
        move |_| corner().map(|(x, y)| (x - HOTSPOT / 2.0, y - HOTSPOT / 2.0)),
        move |origin: &(f32, f32), (x, y)| {
            let (Some(before), Some((_, bounds))) =
                (previewing.transaction().before(), placed(&dragged))
            else {
                return;
            };
            let point = (origin.0 + x, origin.1 + y);
            let planned = Plan::for_node(&before, &dragged).and_then(|plan| {
                let lines = plan.cut_lines(&dragged.area, cut)?;
                gesture::show_guides(point, &snap::guides(cut.into(), &lines), bounds);
                let at = snap::region_line(
                    fraction(cut, bounds, point),
                    &lines,
                    snap::CUT_TOLERANCE,
                    snap::free(),
                );
                plan.split(&dragged.area, cut, at)
            });
            if let Ok(ops) = planned {
                let _ = previewing.preview(ops);
            }
        },
        |_, _| {},
    )))
}

/// The button on the edge the region `first` names shares with `second`, which joins them.
fn join_button(first: Node, second: &AreaId) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let second = Node::area(first.output.as_deref(), first.layer, second);
    let at = {
        let (first, second) = (first.clone(), second.clone());
        move || {
            let (a, _) = placed(&first)?;
            let (b, _) = placed(&second)?;
            let side_by_side =
                (a.x + a.width - b.x).abs() < 1.0 || (b.x + b.width - a.x).abs() < 1.0;
            Some(match side_by_side {
                true => (
                    a.x.max(b.x).min(a.x + a.width),
                    a.y + a.height / 2.0 + APART,
                ),
                false => (
                    a.x + a.width / 2.0 + APART,
                    a.y.max(b.y).min(a.y + a.height),
                ),
            })
        }
    };
    let label = telar::t!(
        "editor.region.joined",
        name = first.area.to_string(),
        other = second.area.to_string()
    );
    Ok(Box::new(
        host::hotspot(host::join_mark()?, at)?
            .cursor(Cursor::Pointer)
            .on_press(move || {
                let done = Plan::for_node(&session::draft().peek(), &first)
                    .and_then(|plan| plan.join(&first.area, &second.area))
                    .and_then(|ops| context::commit(label.clone(), ops));
                said(done);
            }),
    ))
}

/// Where the edge `key` names is now, and the span it runs along, in pixels.
fn edge_now(
    output: &str,
    layer: LayerKind,
    key: &(Cut, Vec<AreaId>, Vec<AreaId>),
) -> Option<(f32, (f32, f32), Rect)> {
    let like = key.1.first().or(key.2.first())?;
    let (desktop, all) = drawn(output, layer)?;
    let within = all.iter().find(|tile| tile.id == *like)?.within;
    let tiles: Vec<Tile> = all
        .into_iter()
        .filter(|tile| tile.within == within)
        .collect();
    let line = regions::lines(&tiles)
        .into_iter()
        .find(|line| line.key() == *key)?;
    Some((
        line.at,
        line.span,
        desktop.reserved.box_of(within, desktop.size),
    ))
}

/// The grip on an edge that moves it, every region on either side following; a focused grip moves with the arrows, one undo entry a press.
fn edge_grip(
    output: &str,
    layer: LayerKind,
    key: (Cut, Vec<AreaId>, Vec<AreaId>),
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Some((at, _, _)) = edge_now(output, layer, &key) else {
        return Ok(Box::new(passthrough(LayoutStyle::new(), Vec::new())?));
    };
    let like = key.1.first().or(key.2.first()).cloned().unwrap_or_default();
    let output = output.to_string();
    let value = signal(at);
    let edit = Edit::new(telar::t!(
        "editor.region.edge_moved",
        name = like.to_string()
    ));
    let plan_to = {
        let (key, region) = (key.clone(), Node::area(Some(&output), layer, &like));
        move |layout: &Layout, to: f32| {
            Plan::for_node(layout, &region).and_then(|plan| plan.move_line(&region.area, &key, to))
        }
    };
    let settling = plan_to.clone();
    let (committing, reverting) = (edit.clone(), edit.clone());
    let hd = gesture::HandleDragging::new();
    let (on_end_commit, on_end_revert) = (hd.on_end_fn(), hd.on_end_fn());
    let transaction = Transaction::new(value)
        .on_commit(move |_, after| {
            on_end_commit();
            if !committing.is_open() && committing.begin().is_err() {
                return;
            }
            let Some(before) = committing.transaction().before() else {
                return;
            };
            let previewed = settling(&before, *after).and_then(|ops| committing.preview(ops));
            match previewed {
                Ok(()) => said(committing.commit()),
                Err(why) => {
                    tracing::info!("{why}");
                    let _ = committing.revert();
                }
            }
        })
        .on_revert(move |_| {
            on_end_revert();
            if reverting.is_open() {
                let _ = reverting.revert();
            }
        });
    let previewing = edit.clone();
    effect(move || {
        let to = value.get();
        if !transaction.is_open() {
            return;
        }
        if !previewing.is_open() && previewing.begin().is_err() {
            return;
        }
        if let Some(before) = previewing.transaction().before()
            && let Ok(ops) = plan_to(&before, to)
        {
            let _ = previewing.preview(ops);
        }
    });
    {
        let (output, key) = (output.clone(), key.clone());
        effect(move || {
            let Some((now, _, _)) = edge_now(&output, layer, &key) else {
                return;
            };
            if !transaction.is_open() && value.peek() != now {
                value.set(now);
            }
        });
    }
    let cut = key.0;
    let (reading, placing) = ((output.clone(), key.clone()), (output, key.clone()));
    let range = {
        let tiles: Vec<Tile> = drawn(&reading.0, layer)
            .map(|(_, tiles)| tiles)
            .unwrap_or_default();
        regions::lines(&tiles)
            .into_iter()
            .find(|line| line.key() == key)
            .map_or((0.0, 1.0), |line| regions::range(&tiles, &line))
    };
    telar::handle(
        telar::HandleProps::props()
            .transaction(transaction)
            .to_value(Rc::new(hd.wrap_to_value(move |x: f32, y: f32| {
                let (output, key) = &reading;
                let Some((_, _, bounds)) = edge_now(output, layer, key) else {
                    return 0.0;
                };
                let lines = drawn(output, layer).map_or_else(Vec::new, |(desktop, _)| {
                    snap::grid_lines_in(&desktop, layer, cut.into(), bounds, range, 0.0)
                });
                gesture::show_guides((x, y), &snap::guides(cut.into(), &lines), bounds);
                snap::region_line(
                    fraction(cut, bounds, (x, y)),
                    &lines,
                    snap::EDGE_TOLERANCE,
                    snap::free(),
                )
            })))
            .to_point(Rc::new(move |to: f32| {
                let (output, key) = &placing;
                let Some((_, (from, until), bounds)) = edge_now(output, layer, key) else {
                    return (-HOTSPOT, -HOTSPOT);
                };
                let middle = (from + until) / 2.0;
                match cut {
                    Cut::SideBySide => (
                        bounds.x + to * bounds.width,
                        bounds.y + middle * bounds.height,
                    ),
                    Cut::Stacked => (
                        bounds.x + middle * bounds.width,
                        bounds.y + to * bounds.height,
                    ),
                }
            }))
            .min(range.0)
            .max(range.1)
            .step(regions::STEP)
            .size(GRIP)
            .cursor(match cut {
                Cut::SideBySide => Cursor::EwResize,
                Cut::Stacked => Cursor::NsResize,
            })
            .build(),
        Children::default(),
    )
}

/// A wallpaper region's popover: its picture — one from the wallpaper library, a path, or whatever `[background]` says (F-10.22) — its fit and transition and its rectangle.
fn region_tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::WallpaperRegion {
        rect,
        source,
        fit,
        transition,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let picture = draft.setting(
        "source",
        "source",
        {
            let source = source.clone();
            kind_read!(WallpaperRegion { source }, source)
        },
        |area, path: &String| {
            kind_field!(
                area,
                "wallpaper_region",
                WallpaperRegion { source },
                path.trim().to_string()
            )
        },
    );
    let mut list = vec![draft.marked(
        &["source"],
        rows::together(vec![
            rows::listed(
                label!("editor.area.source"),
                help("AreaKind::WallpaperRegion", "source"),
                picture,
                Rc::from(library(&source)),
            )?,
            rows::text(
                label!("editor.area.source"),
                help("AreaKind::WallpaperRegion", "source"),
                picture,
            )?,
        ])?,
    )?];
    list.extend(chosen(
        draft,
        "fit",
        label!("editor.area.fit"),
        help("AreaKind::WallpaperRegion", "fit"),
        variants("Fit"),
        kind_read!(WallpaperRegion { fit }, fit),
        |area, fit: Fit| kind_field!(area, "wallpaper_region", WallpaperRegion { fit }, fit),
    )?);
    list.extend(chosen(
        draft,
        "transition",
        label!("editor.area.transition"),
        help("AreaKind::WallpaperRegion", "transition"),
        variants("Transition"),
        move |area| match area.kind {
            ResolvedAreaKind::WallpaperRegion {
                transition: now, ..
            } => now,
            _ => transition,
        },
        |area, transition: Transition| {
            kind_field!(
                area,
                "wallpaper_region",
                WallpaperRegion { transition },
                transition
            )
        },
    )?);
    list.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// What a region's picture can be picked from: whatever `[background]` says first, then every picture in the wallpaper library, and the region's own where the library does not hold it.
fn library(current: &str) -> Vec<(String, String)> {
    let mut choices = vec![(String::new(), telar::t!("editor.region.follow"))];
    for entry in services::wallpaper::all() {
        let shown = match entry.folder.is_empty() {
            true => entry.name.clone(),
            false => format!("{}/{}", entry.folder, entry.name),
        };
        choices.push((entry.path.display().to_string(), shown));
    }
    if !current.is_empty() && !choices.iter().any(|(path, _)| path == current) {
        let name = std::path::Path::new(current)
            .file_name()
            .map_or_else(|| current.to_string(), |name| name.to_string_lossy().into());
        choices.push((current.to_string(), name));
    }
    choices
}

/// A region's menu rows: its two splits, and a texture laid over it.
fn region_rows(_: &ResolvedArea, node: &Node) -> Vec<telar::MenuEntry> {
    let splits = [
        (Cut::SideBySide, telar::t!("editor.region.split_side")),
        (Cut::Stacked, telar::t!("editor.region.split_stacked")),
    ];
    let mut rows: Vec<telar::MenuEntry> = splits
        .into_iter()
        .map(|(cut, label)| {
            let node = node.clone();
            telar::MenuEntry::row(label, "", move || {
                let selection = Selection::Area(node.clone());
                let done = split_by(&selection, &session::draft().peek(), cut).and_then(|ops| {
                    context::commit(
                        telar::t!("editor.region.split", name = node.area.to_string()),
                        ops,
                    )
                });
                said(done);
            })
        })
        .collect();
    let node = node.clone();
    rows.push(telar::MenuEntry::row(
        telar::t!("editor.region.add_texture"),
        "",
        move || said(add_texture(&node)),
    ));
    rows
}

/// `t` and the toolbar's button: a texture laid over the region selected, or the one the selection is in.
fn texture_selected(selection: &Selection) -> Result<(), EditError> {
    let node = selection.node().ok_or_else(no_region)?;
    add_texture(&Node::area(node.output.as_deref(), node.layer, &node.area))
}

fn no_region() -> EditError {
    EditError::refused(telar::t!("editor.texture.no_region"))
}

/// Lays a texture over the region `node` names, right above it in the layer, as one undo entry.
pub(crate) fn add_texture(node: &Node) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let region = desktop
        .resolved
        .area(node.layer, &node.area)
        .ok_or_else(|| EditError::gone(&node.area))?;
    let mut work = Work::new(&layout, &desktop, node.layer);
    let id = layout::ops::free_area_id(&layout, &work.known, node.layer, "texture");
    let texture = texture::over(region, id).ok_or_else(no_region)?;
    work.insert_after(node.layer, &node.area, texture)?;
    context::commit(
        telar::t!("editor.region.texture_added", name = node.area.to_string()),
        work.done(),
    )
}
