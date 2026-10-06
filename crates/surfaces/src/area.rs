//! One area of a resolved layout, built into the node its layer draws.
//!
//! An area carries its own geometry, so a builder here takes the area and nothing else about where it was found: a [`ResolvedAreaKind::Grid`] decides how big its cells are, a [`ResolvedAreaKind::Dock`] how deep its strip is, a [`ResolvedAreaKind::WallpaperRegion`] which picture is showing and how it fades, a [`ResolvedAreaKind::Texture`] what paints over whatever is behind it. What every area shares is what surrounds it — the config an instance reads its options from, the theme it takes its colours from and the monitor it is on — and that is [`Surround`].
//!
//! This is why the instances here are built through [`Host::placed`]: an area answers for its own box and shape, and several areas can share an edge.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use telar::{
    AlignItems, BlendMode, ChildSlot, Clip, ClippedItem, Color, ConsumedKeys, Container, Gradient,
    Image, ImageData, ImageSlice, Insets, Key, LayoutError, LayoutItem, LayoutStyle, Memo,
    ObjectFit, Paint, Point, Raster, ReactiveList, RectStyle, Role, RwSignal, SizeDimension,
    StyledContainer, TemplateTrack, Text, box_item, motion::Animated, signal, track_layout,
};

use crate::actions::{Bound, EmptySpace, NOTCH, Wheel};
use crate::container;
use crate::expressions::{self, Expressions, Overlay, Repeat};
use crate::layer_window::{
    AreaContext, Areas, Blur, Building, LayerWindowContext, Reserved, WindowAreas, blur_of,
    build_window_areas,
};
use crate::look::{self, Look, Rest};
use crate::reconcile::Desktop;
use crate::rects;
use crate::transient::chips::Site;
use config::theme::{FontRole, NordTheme};
use config::{Align, Config, Edge};
use layout::{
    Anchor, AreaId, Blend, Fit, Focus, GroupId, GroupKind, InstanceId as PlacedId, LayerKind,
    Paint as AreaPaint, Rect, Representation as Placed, ResolvedArea, ResolvedAreaKind,
    ResolvedGroup, ResolvedInstance, Sides, Style, Tile, Transition, Zone,
};
use services::wallpaper::{self, Desk};
use ui::descriptor::{Built, ChipDef};
use ui::host::{
    Audience, Extent, Footprint, Host, Instance, InstanceId, OwnSecondary, Representation, Size,
    WidgetSize,
};
use ui::keynav::Move;
use ui::layout::{align_items, fill, justify, painted_chrome};
use ui::module::module_foreground;

/// What an area's contents are built against beyond the area itself: the [`AreaContext`] the window hands down, minus what only the host acts on.
#[derive(Clone, Copy)]
pub struct Surround<'a> {
    pub config: &'a Arc<Config>,
    pub theme: NordTheme,
    pub output: Option<&'a str>,
    /// The layer the area was written on, which is what [`crate::rects`] and a layout edit address it by — not always the window it is drawn in.
    pub layer: LayerKind,
    /// The box this area's geometry is measured in, in the window's coordinate space, with [`layout::Within`] already applied — so a fractional [`Rect`] is a fraction of this and nothing here reads `area.within`.
    pub bounds: telar::Rect,
    /// What the output's reserving areas take off each edge. A bar running [`Extent::Fill`] is as long as the edges beside it leave it, and those edges are on layers this window cannot see.
    pub reserved: Reserved,
    /// Who is in front of this screen. Everything the shell draws while the session is unlocked is the signed-in user's; the lock layer is built for [`Audience::Anyone`], and what that changes is both what a reading may say — a private field draws empty — and what may be built at all: a representation that answers the pointer is drawn as a placeholder there, whatever the file said.
    pub audience: Audience,
}

impl<'a> Surround<'a> {
    pub fn of(context: &'a AreaContext<'a>) -> Self {
        Self {
            config: context.config,
            theme: context.theme,
            output: context.output,
            layer: context.home,
            bounds: context.bounds,
            reserved: context.reserved,
            // A session layer is the signed-in user's by construction: the compositor draws it only while the screen is not locked.
            audience: Audience::Owner,
        }
    }
}

/// Every area of a session layer, built the way the shell draws them.
pub struct ShellAreas;

impl Areas for ShellAreas {
    fn build(&self, context: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
        match build(context.area, Surround::of(context)) {
            Some(built) => built,
            None => Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?)),
        }
    }
}

/// Every area `layer` draws on `desktop`'s output, built into the window being built rather than into `layer`'s own: what an edit mode shows in the overlay window for a layer it cannot raise there (TA-4, R-6), while [`crate::transient::conceal`] keeps the layer's own window from drawing them twice. The builders are the window's own, so it is the same tree, not a second renderer.
///
/// Nothing is asked of the compositor's blur: the region belongs to the window drawing these, and its own areas already set it.
pub fn stand_in(desktop: &Desktop, layer: LayerKind) -> Built {
    let window = LayerWindowContext::current().ok_or_else(|| {
        LayoutError::Engine("a stand-in layer is built inside a layer window".to_string())
    })?;
    let nodes = build_window_areas(
        &ShellAreas,
        &WindowAreas::of(&desktop.resolved, layer),
        &Building {
            window: window.layer,
            output: desktop.output.as_deref(),
            config: &desktop.config,
            theme: desktop.config.resolve_theme(),
            size: desktop.size,
            reserved: desktop.reserved,
            demands: &window.demands,
        },
    );
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
        nodes,
    )?))
}

/// What builds a stack area. The column is a module's — it draws notification, toast and OSD cards — so it is handed in rather than known here.
pub type StackBuilder = for<'a> fn(&ResolvedArea, Surround<'a>) -> Built;

thread_local! {
    static STACK: Cell<Option<StackBuilder>> = const { Cell::new(None) };
}

/// Installs what builds a stack area. Set once at startup; until it is, a stack area draws nothing.
pub fn set_stack_builder(build: StackBuilder) {
    STACK.with(|stack| stack.set(Some(build)));
}

/// The node `area` draws, or `None` for a kind no builder answers for yet, so a layer draws the areas it can rather than failing whole over the one it cannot.
///
/// An area with a `visible` it can read is taken out of layout while that is false, which hides its paint and every input target in it without rebuilding it.
pub fn build(area: &ResolvedArea, surround: Surround) -> Option<Built> {
    let Some(visible) = &area.visible else {
        return drawn(area, surround);
    };
    telar::Scope::with(|| {
        let at = rects::Node::area(surround.output, surround.layer, &area.id);
        let shown = Expressions::here(surround.audience).visible(&at, visible);
        let built = drawn(area, surround)?;
        Some(built.and_then(|item| {
            let layer = StyledContainer::new(
                LayoutStyle::new()
                    .absolute()
                    .inset_start(0.0)
                    .inset_top(0.0)
                    .width(SizeDimension::Percent(1.0))
                    .height(SizeDimension::Percent(1.0)),
                |_| RectStyle::default(),
                vec![item],
            )?
            .with_opacity(move || shown.opacity());
            let node = layer.layout_node();
            telar::effect(move || {
                telar::set_display(node, shown.displayed());
                let _ = telar::mark_dirty(node);
            });
            Ok(Box::new(layer) as Box<dyn LayoutItem>)
        }))
    })
}

fn drawn(area: &ResolvedArea, surround: Surround) -> Option<Built> {
    let built = match &area.kind {
        ResolvedAreaKind::Bar { .. } => Some(crate::bar::build_bar(
            area,
            surround,
            ui::descriptor::installed(),
        )),
        ResolvedAreaKind::Grid {
            rect,
            cell,
            gap,
            anchor,
        } => Some(grid(area, *rect, *cell, *gap, *anchor, surround)),
        ResolvedAreaKind::Dock { edge, thickness } => Some(dock(area, *edge, *thickness, surround)),
        ResolvedAreaKind::WallpaperRegion { .. } => Some(wallpaper_region(area, surround)),
        ResolvedAreaKind::Texture { .. } => Some(texture(area, surround)),
        ResolvedAreaKind::Free { rect, anchor } => Some(free(area, *rect, *anchor, surround)),
        ResolvedAreaKind::Stack { .. } => STACK
            .with(|stack| stack.get())
            .map(|build| build(area, surround)),
        // A panel is drawn where its owner opens it, not where it is written: see `crate::panel`.
        _ => None,
    }?;
    if let Ok(node) = &built {
        rects::track(
            rects::Node::area(surround.output, surround.layer, &area.id),
            node.layout_node(),
        );
    }
    Some(built)
}

/// `root` answering the gestures `area` binds to its empty space, for whichever kind of area it is the root of, and opening the area's context menu on a secondary press nothing is bound to. Nothing is bound on the lock layer, and no menu is offered there.
///
/// The area's menu is offered only where `painted` says the root claims the pointer already, by what it draws: a root that answered a press only for the menu's sake would take every press over its whole box from the windows under it. The background layer is the exception, being under everything else: what it does not paint is the screen's empty space, and offers the shell's own menu there.
///
/// A root that neither paints nor answers lets presses through, its instances still claiming their own: a window gives a press to the topmost box under it whether or not that box wants it, so such a root would take it from an area built before it in the same window.
pub fn empty_space(
    area: &ResolvedArea,
    surround: Surround,
    root: StyledContainer,
    painted: bool,
) -> StyledContainer {
    let menu = match (painted, surround.layer) {
        (true, _) => crate::menu::on(
            rects::Node::area(surround.output, surround.layer, &area.id),
            surround.audience,
        ),
        (false, LayerKind::Background) => crate::menu::shell_on(surround.output, surround.audience),
        (false, _) => None,
    };
    let bound = Bound::of(&area.actions, surround.audience).with_menu(menu);
    if bound.is_empty() && !painted {
        return root.input_transparent();
    }
    bound.on_empty_space(
        root,
        EmptySpace {
            output: surround.output.map(str::to_string),
            layer: surround.layer,
            area: area.id.clone(),
        },
    )
}

/// Whether an area [`dressed`] in `style` paints a fill, which is what makes it claim the pointer.
pub fn is_filled(style: &Style, theme: &NordTheme) -> bool {
    style.paint(theme).is_some_and(|fill| fill.a > 0.0)
}

/// A free area: its groups down the box `rect` names, each instance at the size its representation asks for, the column they make sitting where `anchor` says.
///
/// It is the kind with no arrangement of its own — no cells, no zones, no edge — so it is what a layout says when the answer to "where" is simply a rectangle.
pub fn free(area: &ResolvedArea, rect: Rect, anchor: Anchor, surround: Surround) -> Built {
    let (vertical, horizontal) = anchored(anchor);
    let groups = area
        .groups
        .iter()
        .map(|group| {
            arranged(
                area,
                group,
                LayoutStyle::new().flex_column(),
                None,
                surround,
                |instance, node, surround| {
                    place(
                        instance,
                        node,
                        Size {
                            width: f32::INFINITY,
                            height: f32::INFINITY,
                        },
                        None,
                        (LayoutStyle::new(), Frame::Bare),
                        surround,
                    )
                },
            )
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    Ok(Box::new(empty_space(
        area,
        surround,
        frosted(
            dressed(
                &area.style,
                &surround.theme,
                region(rect, surround)
                    .flex_column()
                    .justify_content(justify(vertical))
                    .align_items(align_items(horizontal)),
                groups,
            )?,
            area,
            surround,
        ),
        is_filled(&area.style, &surround.theme),
    )))
}

/// A grid: every group on the cells it was placed at, `cell` px each and `gap` apart, on the lattice of every cell that fits `rect` inside the area's padding.
///
/// The lattice is sized to `rect`, never to what is on it, so a cell is in the same place whatever else the grid holds ([`lattice`]). `anchor` says where it sits in what `rect` has left over once whole cells are taken.
pub fn grid(
    area: &ResolvedArea,
    rect: Rect,
    cell: f32,
    gap: f32,
    anchor: Anchor,
    surround: Surround,
) -> Built {
    let live = placing(area, surround);
    let slide = surround.config.animation.travel_tween_ms(200, 2_000);
    let placed = area
        .groups
        .iter()
        .map(|group| {
            cell_group(area, group, (cell, gap), live, surround)
                .map(|built| Box::new(telar::animate_layout(built, slide)) as Box<dyn LayoutItem>)
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let room = room_in(area, within(rect, surround.bounds), cell, gap);
    let block = Container::new(tracks(room.holding(covered(area)), cell, gap), placed)?
        .styled_by(move || live.with(|now| tracks(room.holding(covered(now)), cell, gap)));
    let (vertical, horizontal) = anchored(anchor);
    Ok(Box::new(empty_space(
        area,
        surround,
        frosted(
            dressed(
                &area.style,
                &surround.theme,
                region(rect, surround)
                    .flex_row()
                    .align_items(align_items(vertical))
                    .justify_content(justify(horizontal)),
                vec![Box::new(block) as Box<dyn LayoutItem>],
            )?,
            area,
            surround,
        ),
        is_filled(&area.style, &surround.theme),
    )))
}

/// A panel's groups on its own `cols` × `rows` cells, each placed as a grid places it.
pub(crate) fn panel_cells(area: &ResolvedArea, surround: Surround) -> Built {
    let ResolvedAreaKind::Panel {
        cols,
        rows,
        cell,
        gap,
        ..
    } = area.kind
    else {
        return Err(LayoutError::Engine(format!("'{}' is no panel", area.id)));
    };
    let live = signal(area.clone());
    let placed = area
        .groups
        .iter()
        .map(|group| cell_group(area, group, (cell, gap), live, surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let room = Footprint {
        columns: cell_span(cols),
        rows: cell_span(rows),
    };
    Ok(Box::new(Container::new(
        tracks(room.holding(covered(area)), cell, gap),
        placed,
    )?))
}

/// Which grid a live placement is of: the output, the window it is drawn in, the layer it is written on and its id.
type GridKey = (Option<String>, LayerKind, LayerKind, AreaId);

/// One grid built now, by the token its build took, and the arrangement it places its groups from.
type Placing = (u64, RwSignal<ResolvedArea>);

thread_local! {
    /// Every grid built into a layer window now, with the arrangement it places its groups from.
    static GRIDS: RefCell<HashMap<GridKey, Vec<Placing>>> = RefCell::new(HashMap::new());
    static GRID_TOKENS: Cell<u64> = const { Cell::new(0) };
}

/// The arrangement `area`'s groups are placed from for as long as its grid lives, which [`move_cells`] moves them in. Outside a layer window nothing moves it, and the grid is placed once.
fn placing(area: &ResolvedArea, surround: Surround) -> RwSignal<ResolvedArea> {
    let live = signal(area.clone());
    let Some(window) = LayerWindowContext::current() else {
        return live;
    };
    let key = (
        surround.output.map(str::to_string),
        window.layer,
        surround.layer,
        area.id.clone(),
    );
    let token = GRID_TOKENS.with(|next| {
        next.set(next.get().wrapping_add(1));
        next.get()
    });
    GRIDS.with(|grids| {
        grids
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .push((token, live))
    });
    telar::on_cleanup(move || {
        GRIDS.with(|grids| {
            let mut grids = grids.borrow_mut();
            if let Some(held) = grids.get_mut(&key) {
                held.retain(|(own, _)| *own != token);
                if held.is_empty() {
                    grids.remove(&key);
                }
            }
        })
    });
    live
}

/// Moves the groups of every grid built for `area` in the window `window` of `output`, written on `home`, to the cells `area` places them on, where they slide; answers whether a grid built now took it. The one change to a built grid that keeps its nodes: see [`moves_only`].
pub fn move_cells(
    output: Option<&str>,
    window: LayerKind,
    home: LayerKind,
    area: &ResolvedArea,
) -> bool {
    let key = (output.map(str::to_string), window, home, area.id.clone());
    let live: Vec<RwSignal<ResolvedArea>> = GRIDS.with(|grids| {
        grids
            .borrow()
            .get(&key)
            .map(|held| held.iter().map(|(_, live)| *live).collect())
            .unwrap_or_default()
    });
    let alive: Vec<RwSignal<ResolvedArea>> =
        live.into_iter().filter(|live| live.is_alive()).collect();
    for live in &alive {
        if live.peek_with(|now| now != area) {
            live.set(area.clone());
        }
    }
    !alive.is_empty()
}

/// Whether `now` is `was` with its groups on other cells and its containers rearranged, and nothing else changed: the same grid holding the same groups, in the same order, with the same instances outside its containers. A container may take another span, arrangement or gap, and its children other places, another order, or be added or taken out, as long as each child it keeps is otherwise as it was. A grid changed that way keeps its nodes and slides them ([`move_cells`]) rather than being built again.
pub fn moves_only(was: &ResolvedArea, now: &ResolvedArea) -> bool {
    if !matches!(now.kind, ResolvedAreaKind::Grid { .. }) || was.groups.len() != now.groups.len() {
        return false;
    }
    let mut placed_as_before = now.clone();
    for (group, before) in placed_as_before.groups.iter_mut().zip(&was.groups) {
        if keyed(group) && keyed(before) {
            if !rearranges(before, group) {
                return false;
            }
            group.arrange = before.arrange;
            group.cols = before.cols;
            group.rows = before.rows;
            group.gap = before.gap;
            group.children.clone_from(&before.children);
        }
        group.kind = before.kind;
    }
    placed_as_before == *was
}

/// A container whose children are built one by one by instance id, so a change to how it shares its box out keeps them ([`contained`]). Copies of a repeated one are not instances of their own and are built with the group.
fn keyed(group: &ResolvedGroup) -> bool {
    container::arranges(group) && group.repeat.is_none()
}

/// Whether every child `now` holds that `was` held too is as it was but for where it sits.
fn rearranges(was: &ResolvedGroup, now: &ResolvedGroup) -> bool {
    now.children.iter().all(|child| {
        was.children
            .iter()
            .find(|held| held.id == child.id)
            .is_none_or(|held| {
                *held
                    == ResolvedInstance {
                        placement: held.placement,
                        ..child.clone()
                    }
            })
    })
}
/// How many cells a grid `area` placed at `region` has room for: every whole cell that fits inside its padding, never less than one each way.
fn room_in(area: &ResolvedArea, region: telar::Rect, cell: f32, gap: f32) -> Footprint {
    let pad = area.style.padding.unwrap_or_default();
    let fitting = |length: f32, pad: f32| {
        (((length - pad + gap) / (cell + gap).max(1.0)).floor() as u16).max(1)
    };
    Footprint {
        columns: fitting(region.width, pad.horizontal()),
        rows: fitting(region.height, pad.vertical()),
    }
}

/// The lattice a grid `area` placed at `region` draws: the cells it has room for, and past them only as far as a group written beyond them reaches. `None` for an area that is no grid.
pub fn lattice(area: &ResolvedArea, region: telar::Rect) -> Option<Footprint> {
    let ResolvedAreaKind::Grid { cell, gap, .. } = area.kind else {
        return None;
    };
    Some(room_in(area, region, cell, gap).holding(covered(area)))
}

pub fn room(area: &ResolvedArea, region: telar::Rect) -> Option<Footprint> {
    let ResolvedAreaKind::Grid { cell, gap, .. } = area.kind else {
        return None;
    };
    Some(room_in(area, region, cell, gap))
}

/// Where the lattice of a grid sits when the area itself is placed at `region`: inside its padding, as big as [`lattice`] makes it and aligned by its anchor — what a pointer over the grid is read against to find the cell under it. `None` for an area that is no grid.
pub fn grid_block(area: &ResolvedArea, region: telar::Rect) -> Option<telar::Rect> {
    let ResolvedAreaKind::Grid {
        cell, gap, anchor, ..
    } = area.kind
    else {
        return None;
    };
    let pad = area.style.padding.unwrap_or_default();
    let (width, height) = (
        (region.width - pad.horizontal()).max(0.0),
        (region.height - pad.vertical()).max(0.0),
    );
    let extent = lattice(area, region)?.extent(cell, gap);
    let (vertical, horizontal) = anchored(anchor);
    let offset = |align: Align, room: f32| match align {
        Align::Start => 0.0,
        Align::Center => room / 2.0,
        Align::End => room,
    };
    Some(telar::Rect::new(
        region.x + pad.left() + offset(horizontal, width - extent.width),
        region.y + pad.top() + offset(vertical, height - extent.height),
        extent.width,
        extent.height,
    ))
}

/// A dock: a strip hugging `edge`, `thickness` across and the whole of that edge long, its groups sharing the length in zone order.
///
/// It has no length of its own to decide, which is the one thing that tells a dock from a bar in the model: a bar carries `length` and `offset` so several can share an edge, and a dock carries neither because it is the edge.
pub fn dock(area: &ResolvedArea, edge: Edge, thickness: f32, surround: Surround) -> Built {
    let runs = [Zone::Start, Zone::Center, Zone::End]
        .into_iter()
        .flat_map(|zone| {
            area.groups
                .iter()
                .filter(move |group| zone_of(group) == zone)
        })
        .map(|group| run(area, group, edge, thickness, surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let strip = Container::new(along(edge, thickness), runs)?;
    let (vertical, horizontal) = hugging(edge);
    Ok(Box::new(empty_space(
        area,
        surround,
        frosted(
            dressed(
                &area.style,
                &surround.theme,
                pixels(docked(edge, thickness, area.style.padding, surround.bounds))
                    .flex_row()
                    .align_items(align_items(vertical))
                    .justify_content(justify(horizontal)),
                vec![Box::new(strip) as Box<dyn LayoutItem>],
            )?,
            area,
            surround,
        ),
        is_filled(&area.style, &surround.theme),
    )))
}

/// The box a dock holds: the whole of its edge long, and its strip and the padding either side of it across.
fn docked(edge: Edge, thickness: f32, padding: Option<Sides>, bounds: telar::Rect) -> telar::Rect {
    let sides = padding.unwrap_or_default();
    let across = match edge.is_horizontal() {
        true => (thickness + sides.vertical()).min(bounds.height),
        false => (thickness + sides.horizontal()).min(bounds.width),
    };
    let (right, bottom) = (bounds.x + bounds.width, bounds.y + bounds.height);
    match edge {
        Edge::Top => telar::Rect::new(bounds.x, bounds.y, bounds.width, across),
        Edge::Bottom => telar::Rect::new(bounds.x, bottom - across, bounds.width, across),
        Edge::Left => telar::Rect::new(bounds.x, bounds.y, across, bounds.height),
        Edge::Right => telar::Rect::new(right - across, bounds.y, across, bounds.height),
    }
}

/// Reading where the hand-over between a `WallpaperRegion`'s two image layers has got to, and moving it. `Rc` on the reading half because both layers hold one; `Box` on the writing half because only the [`Handover`] does.
type FadeControl = (Rc<dyn Fn() -> f32>, Box<dyn Fn(f32)>);

/// Per area as well as per output, so two regions on one screen never share a decode slot or a fade timeline.
type RegionKey = (Option<String>, AreaId);

/// A region with its own `source` never follows the per-output feed (F-10.22). A rebuild — a layout or config edit — starts from what [`opened`] says it last showed, since a fresh node has nothing of the old picture to fade from.
pub fn wallpaper_region(area: &ResolvedArea, surround: Surround) -> Built {
    let ResolvedAreaKind::WallpaperRegion {
        rect,
        source,
        fit,
        transition,
        focus,
        dim,
        blur,
        parallax,
    } = &area.kind
    else {
        unreachable!("build only routes a WallpaperRegion area here")
    };
    let (rect, fit, transition, dim, blur) = (*rect, *fit, *transition, *dim, *blur);
    let reduced = surround.config.animation.is_reduced();
    let transition = match transition {
        Transition::Slide if reduced => Transition::Fade,
        transition => transition,
    };
    let laying = Laying {
        fit,
        size: inside(within(rect, surround.bounds), area.style.padding),
        focus: *focus,
        parallax: match fit == Fit::Cover && !reduced {
            true => *parallax,
            false => 0.0,
        },
    };
    if laying.parallax > 0.0 || dim > 0.0 || blur > 0.0 {
        crate::desk::follow(surround.output);
    }

    let key = (surround.output.map(str::to_string), area.id.clone());
    let follows_background = source.is_empty();
    let wanted = match follows_background {
        false => Some(util::paths::expand_tilde(Path::new(source))),
        true => wallpaper::current_image(surround.config, surround.output),
    };
    let before = opened(&key);
    let arriving = wanted.as_deref().and_then(|path| decoded(&key, path));
    if arriving.is_none() {
        forget(&key);
        if let Some(path) = &wanted {
            tracing::warn!(
                "wallpaper '{}' could not be loaded; using the region's fill",
                path.display()
            );
        }
    }
    let fades_from = before.filter(|was| wanted.as_deref() != Some(was.path.as_path()));

    let (read_fade, set_fade) = fade_control(surround.config, transition);
    let handover = match fades_from {
        Some(was) => {
            let handover = Handover::new(Some(was.image), set_fade);
            handover.show(arriving);
            handover
        }
        None => Handover::new(arriving, set_fade),
    };
    let along = slid_along(surround, laying.parallax > 0.0);
    let layer_a = image_layer(
        handover.a.read_only(),
        (read_fade.clone(), 0.0),
        transition,
        laying,
        Rc::clone(&along),
    )?;
    let layer_b = image_layer(
        handover.b.read_only(),
        (read_fade, 1.0),
        transition,
        laying,
        along,
    )?;

    if follows_background {
        let output = surround.output.map(str::to_string);
        platform_wayland::watch(
            wallpaper::frames(output, wanted),
            move |frame: Option<wallpaper::Frame>| {
                // `None` is the producer's liveness heartbeat, not a wallpaper.
                let Some(frame) = frame else { return };
                remember(&key, &frame.path, Arc::clone(&frame.image));
                handover.show(Some(frame.image));
            },
        );
    }

    let mut layers = vec![layer_a, layer_b];
    layers.extend(veil(surround, dim, blur)?);
    let stack = ClippedItem::new(Box::new(Container::new(fill(), layers)?), Clip::both());
    let mut painted = picture(
        &area.style,
        Some(surround.theme.base),
        &surround.theme,
        fill(),
        vec![box_item(stack)],
    )?;
    if let Some(opacity) = area.style.opacity {
        painted = painted.with_opacity(move || opacity);
    }
    framed(
        area,
        surround,
        region(rect, surround),
        frosted(painted, area, surround),
    )
}

struct Handover {
    a: RwSignal<Option<Arc<ImageData>>>,
    b: RwSignal<Option<Arc<ImageData>>>,
    /// A plain `Cell`: it only ever changes on the driver thread, from [`Handover::show`], so a signal would buy reactivity that nothing reads.
    showing_b: Cell<bool>,
    fade: Box<dyn Fn(f32)>,
}

impl Handover {
    fn new(resting: Option<Arc<ImageData>>, fade: Box<dyn Fn(f32)>) -> Self {
        Self {
            a: signal(resting),
            b: signal(None),
            showing_b: Cell::new(false),
            fade,
        }
    }

    fn show(&self, image: Option<Arc<ImageData>>) {
        let next_is_b = !self.showing_b.get();
        match next_is_b {
            true => self.b.set(image),
            false => self.a.set(image),
        }
        self.showing_b.set(next_is_b);
        (self.fade)(if next_is_b { 1.0 } else { 0.0 });
    }
}

/// How the layers are handed over: reading the current position, and moving it ([`eased`]).
fn fade_control(config: &Config, transition: Transition) -> FadeControl {
    let instant = transition == Transition::None
        || !config.animation.enabled
        || config.background.transition_ms == 0;
    let tween = config
        .animation
        .tween_ms(config.background.transition_ms, 10_000);
    eased(0.0, (!instant).then_some(tween))
}

/// A value read and moved through a pair of closures, starting at `from`: tweened by `tween`, or set at once without one.
///
/// An `Animated` is retargeted, never built at its destination, which would leave it inert; and no tween at all drives a plain signal rather than an `Animated` with a zero-length tween, which has no duration to divide by.
fn eased(from: f32, tween: Option<telar::motion::Tween>) -> FadeControl {
    let Some(tween) = tween else {
        let at = signal(from);
        let reading = at.read_only();
        return (
            Rc::new(move || reading.get()),
            Box::new(move |to| {
                if at.peek() != to {
                    at.set(to)
                }
            }),
        );
    };
    let moving = Animated::new(from, tween);
    (
        Rc::new(move || moving.get()),
        Box::new(move |to| moving.retarget(to)),
    )
}

#[derive(Clone)]
struct Opened {
    path: PathBuf,
    stamp: Option<SystemTime>,
    image: Arc<ImageData>,
}

thread_local! {
    /// Kept across the rebuilds that replace a region's node, so a rebuild can fade from what was showing.
    static OPENED: RefCell<HashMap<RegionKey, Opened>> = RefCell::new(HashMap::new());
}

/// The size, in its own pixels, of the picture the area `id` on the screen `output` last drew — a region's, a texture's — where it drew one: what a nine-slice's insets are measured in.
pub fn picture_size(output: Option<&str>, id: &AreaId) -> Option<(u32, u32)> {
    opened(&(output.map(str::to_string), id.clone()))
        .map(|shown| (shown.image.width, shown.image.height))
}

fn opened(key: &RegionKey) -> Option<Opened> {
    OPENED.with(|opened| opened.borrow().get(key).cloned())
}

fn remember(key: &RegionKey, path: &Path, image: Arc<ImageData>) {
    let shown = Opened {
        path: path.to_path_buf(),
        stamp: modified(path),
        image,
    };
    OPENED.with(|opened| opened.borrow_mut().insert(key.clone(), shown));
}

fn forget(key: &RegionKey) {
    OPENED.with(|opened| opened.borrow_mut().remove(key));
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Decoded at most once per file and mtime: a config reload or a settings form rebuilds the region, and a full decode on the UI thread each time would stall the frames between keystrokes.
fn decoded(key: &RegionKey, path: &Path) -> Option<Arc<ImageData>> {
    let stamp = modified(path);
    let hit = opened(key)
        .filter(|was| was.path == path && stamp.is_some() && was.stamp == stamp)
        .map(|was| was.image);
    if hit.is_some() {
        return hit;
    }
    let image = Arc::new(util::picture::decode(path)?);
    remember(key, path, Arc::clone(&image));
    Some(image)
}

/// How a region lays its picture: the fit, the box it fills, and for `cover` the point it keeps in view and how far it slides across the workspaces.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Laying {
    fit: Fit,
    size: (f32, f32),
    focus: Focus,
    parallax: f32,
}

/// How long the picture takes to dim and blur as a window covers the screen, and to come back, before `[animation]` scales it.
const VEIL_MS: u64 = 240;
/// How long the picture takes to slide to where the workspace switched to puts it, before `[animation]` scales it.
const PARALLAX_MS: u64 = 450;

fn inside(rect: telar::Rect, padding: Option<Sides>) -> (f32, f32) {
    let sides = padding.unwrap_or_default();
    (
        (rect.width - sides.horizontal()).max(0.0),
        (rect.height - sides.vertical()).max(0.0),
    )
}

/// Where a picture `intrinsic` pixels big lies to cover a box `size` big, from the box's top left: scaled to cover the box widened by `parallax` of its width, and placed so `focus` sits at the middle of that box as far as the picture reaches past it.
pub fn covering(
    intrinsic: (f32, f32),
    size: (f32, f32),
    focus: Focus,
    parallax: f32,
) -> telar::Rect {
    let (width, height) = (size.0 * (1.0 + parallax.max(0.0)), size.1);
    if intrinsic.0 <= 0.0 || intrinsic.1 <= 0.0 || width <= 0.0 || height <= 0.0 {
        return telar::Rect::new(0.0, 0.0, width, height);
    }
    let scale = (width / intrinsic.0).max(height / intrinsic.1);
    let (drawn_width, drawn_height) = (intrinsic.0 * scale, intrinsic.1 * scale);
    let focus = focus.clamped();
    let placed = |room: f32, length: f32, at: f32| {
        (room / 2.0 - at * length).clamp((room - length).min(0.0), 0.0)
    };
    telar::Rect::new(
        placed(width, drawn_width, focus.x),
        placed(height, drawn_height, focus.y),
        drawn_width,
        drawn_height,
    )
}

/// How far left a picture with `parallax` is slid in a box `width` wide, with the workspace up `along` the screen's workspaces.
pub fn parallax_shift(width: f32, parallax: f32, along: f32) -> f32 {
    -(width * parallax.max(0.0) * along.clamp(0.0, 1.0))
}

/// How far along the screen's workspaces the one up is, eased there as it changes; half way, and never moving, for a region with no parallax.
fn slid_along(surround: Surround, slides: bool) -> Rc<dyn Fn() -> f32> {
    if !slides {
        return Rc::new(|| 0.5);
    }
    let output = surround.output.map(str::to_string);
    let animation = &surround.config.animation;
    let tween = animation
        .enabled
        .then(|| animation.travel_tween_ms(PARALLAX_MS, 2_000));
    let resting = crate::desk::now(surround.output).along.unwrap_or(0.5);
    let (along, slide_to) = eased(resting, tween);
    telar::effect(move || slide_to(crate::desk::of(output.as_deref()).along.unwrap_or(0.5)));
    along
}

/// What darkens and blurs the picture while a window covers the screen: nothing for a region with neither.
fn veil(surround: Surround, dim: f32, blur: f32) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    if dim <= 0.0 && blur <= 0.0 {
        return Ok(Vec::new());
    }
    let output = surround.output.map(str::to_string);
    let veiling = |desk: Desk| match desk.covered {
        true => 1.0,
        false => 0.0,
    };
    let animation = &surround.config.animation;
    let tween = animation
        .enabled
        .then(|| animation.tween_ms(VEIL_MS, 2_000));
    let (veiled, veil_to) = eased(veiling(crate::desk::now(surround.output)), tween);
    telar::effect(move || veil_to(veiling(crate::desk::of(output.as_deref()))));
    let blurring = Rc::clone(&veiled);
    let blurred =
        StyledContainer::new(fill().absolute_fill(), |_| RectStyle::default(), Vec::new())?
            .with_backdrop_blur(move || blur * blurring());
    let dimmed = StyledContainer::new(
        fill().absolute_fill(),
        move |_| RectStyle::filled(Color::BLACK.with_alpha(dim * veiled()), 0.0),
        Vec::new(),
    )?;
    Ok(vec![Box::new(blurred), Box::new(dimmed)])
}

/// One image slot, shown in proportion to how close `fade` is to `visible_at` (0 or 1) and laid as `laying` says, its picture slid by `along` where it has parallax.
///
/// The image takes its data as a closure, so the layer is one node for the life of the surface and swapping the picture is a signal write, not a rebuild.
fn image_layer(
    slot: telar::ReadSignal<Option<Arc<ImageData>>>,
    (fade, visible_at): (Rc<dyn Fn() -> f32>, f32),
    transition: Transition,
    laying: Laying,
    along: Rc<dyn Fn() -> f32>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let covers = laying.fit == Fit::Cover;
    let object_fit = match covers {
        true => ObjectFit::Fill,
        false => to_object_fit(laying.fit),
    };
    let image = Image::new(
        fill(),
        move || slot.get().unwrap_or_else(blank),
        || Raster::Smooth,
        move || object_fit,
    )?;

    let laid = move |held: Option<Arc<ImageData>>| match (covers, held) {
        (true, Some(image)) => pixels(covering(
            (image.width as f32, image.height as f32),
            laying.size,
            laying.focus,
            laying.parallax,
        )),
        _ => fill().absolute_fill(),
    };
    let opacity_fade = Rc::clone(&fade);
    let mut layer = StyledContainer::new(
        laid(slot.peek()),
        |_| RectStyle::default(),
        vec![box_item(image)],
    )?
    .styled_by(move || laid(slot.get()))
    .with_opacity(move || {
        // Both read before the early return: a slot that is empty this frame must still re-run when it fills.
        let at = opacity_fade();
        if slot.with(Option::is_none) {
            return 0.0;
        }
        1.0 - (at - visible_at).abs()
    });

    let slides = transition == Transition::Slide;
    if slides || laying.parallax > 0.0 {
        // A slide moves only the layer being left, so the arriving one comes to rest exactly where the other was.
        layer = layer.with_transform(move |_| {
            let mut by = parallax_shift(laying.size.0, laying.parallax, along());
            if slides {
                by += (fade() - visible_at).abs() * laying.size.0;
            }
            (by != 0.0).then_some([1.0, 0.0, 0.0, 1.0, by, 0.0])
        });
    }
    Ok(Box::new(layer))
}

/// telar's closest [`ObjectFit`] for a resolved [`layout::Fit`].
fn to_object_fit(fit: Fit) -> ObjectFit {
    match fit {
        Fit::Cover => ObjectFit::Cover,
        Fit::Contain => ObjectFit::Contain,
        Fit::Stretch => ObjectFit::Fill,
        Fit::Tile => ObjectFit::Tile { scale: 1.0 },
    }
}

/// telar's [`BlendMode`] for a resolved [`layout::Blend`]. telar has every one of these (and more besides), so this is a plain rename, not a gap.
fn to_blend_mode(blend: Blend) -> BlendMode {
    match blend {
        Blend::Normal => BlendMode::Normal,
        Blend::Multiply => BlendMode::Multiply,
        Blend::Screen => BlendMode::Screen,
        Blend::Overlay => BlendMode::Overlay,
        Blend::Add => BlendMode::Plus,
    }
}

/// One [`ResolvedAreaKind::Texture`] as a node confined to its `rect`: an image or a gradient painted over whatever is behind it. Static, unlike a `WallpaperRegion` — nothing here watches the wallpaper service, so there is no cross-fade to keep and no live channel to key by region; only the decode cache is shared with it.
pub fn texture(area: &ResolvedArea, surround: Surround) -> Built {
    let ResolvedAreaKind::Texture {
        rect,
        paint,
        tile,
        blend,
        opacity,
    } = &area.kind
    else {
        unreachable!("build only routes a Texture area here")
    };
    let (rect, tile, blend, opacity) = (*rect, *tile, *blend, *opacity);
    let blend_mode = to_blend_mode(blend);

    let content: Box<dyn LayoutItem> = match paint {
        AreaPaint::Image(path) => {
            let key = (surround.output.map(str::to_string), area.id.clone());
            let decoded_image = decoded(&key, Path::new(path));
            if decoded_image.is_none() {
                tracing::warn!("texture image '{path}' could not be loaded; drawing nothing there");
            }
            let data = decoded_image.unwrap_or_else(blank);
            let (fit, slice) = match tile {
                Tile::None => (ObjectFit::Fill, None),
                Tile::Repeat => (ObjectFit::Tile { scale: 1.0 }, None),
                // A slice draws its own borders and middle, so the fit under it only decides what happens to the centre patch.
                Tile::NineSlice {
                    top,
                    right,
                    bottom,
                    left,
                } => (
                    ObjectFit::Fill,
                    Some(ImageSlice::new(Insets::new(top, right, bottom, left))),
                ),
            };
            let image = Image::new(
                fill(),
                move || Arc::clone(&data),
                || Raster::Smooth,
                move || fit,
            )?;
            box_item(match slice {
                Some(slice) => image.with_slice(move || slice),
                None => image,
            })
        }
        AreaPaint::Gradient(gradient) => {
            if tile != Tile::None {
                tracing::warn!(
                    "a texture with a gradient paint ignores `tile`: a gradient has no source pixels to tile"
                );
            }
            let gradient = gradient.clone();
            let theme = surround.theme;
            box_item(StyledContainer::new(
                fill(),
                move |r| RectStyle {
                    fill: Some(gradient_paint(&gradient, theme, (r.width, r.height))),
                    ..RectStyle::default()
                },
                vec![],
            )?)
        }
    };

    let opacity = opacity * area.style.opacity.unwrap_or(1.0);
    let painted = picture(&area.style, None, &surround.theme, fill(), vec![content])?
        .with_opacity(move || opacity)
        .with_blend(move || blend_mode);
    framed(
        area,
        surround,
        region(rect, surround),
        frosted(painted, area, surround),
    )
}

/// telar's `Paint::Gradient` for a resolved [`layout::Gradient`], sized to the texture's own pixel box.
///
/// `angle` is degrees clockwise from a left-to-right sweep (the field's own doc on [`layout::Gradient`]), so the direction is `(cos, sin)` in the box's own coordinates, where y already points down. The gradient line is sized to the box's own support in that direction, the same "to the farthest corner" sizing CSS gives an angled `linear-gradient`.
fn gradient_paint(gradient: &layout::Gradient, theme: NordTheme, size: (f32, f32)) -> Paint {
    let (width, height) = size;
    let theta = gradient.angle.to_radians();
    let (dx, dy) = (theta.cos(), theta.sin());
    let (half_w, half_h) = (width / 2.0, height / 2.0);
    let reach = half_w * dx.abs() + half_h * dy.abs();
    let center = Point::new(half_w, half_h);
    let start = Point::new(center.x - dx * reach, center.y - dy * reach);
    let end = Point::new(center.x + dx * reach, center.y + dy * reach);

    let mut stops: Vec<(f32, Color)> = gradient
        .stops
        .iter()
        .map(|stop| (stop.at, layout::color_of(&stop.color, &theme)))
        .collect();
    if stops.len() > 8 {
        // `GradientStops` is a fixed `[GradientStop; 8]` (crates/renderer/renderer-core/src/style/gradient.rs in the telar checkout); telar keeps only the first 8 itself, this just makes the drop visible.
        tracing::warn!(
            "a gradient names {} stops; only the first 8 are drawn",
            stops.len()
        );
        stops.truncate(8);
    }
    Paint::Gradient(Gradient::linear(start, end, &stops))
}

/// A single transparent pixel, stood in for an empty `WallpaperRegion`/`Texture` slot.
///
/// Shared rather than built per call: `ImageData::new` mints a new id every time, and a slot that handed the renderer a fresh id on every frame would fill the texture cache with copies of nothing.
fn blank() -> Arc<ImageData> {
    thread_local! {
        static BLANK: Arc<ImageData> = Arc::new(ImageData::new(vec![0, 0, 0, 0], 1, 1));
    }
    BLANK.with(Arc::clone)
}

/// The pointer stops only where the fill can be seen ([`painted_chrome`]); a bar never comes through here, because its strip is its paint.
pub fn dressed(
    style: &Style,
    theme: &NordTheme,
    layout: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, LayoutError> {
    let look = Look::area(style, theme);
    Ok(painted_chrome(
        StyledContainer::new(
            padded(layout, style.padding),
            move |rect| look.paint(rect),
            children,
        )?,
        look.fill,
    ))
}

/// How far across the shell blurs what its own surface drew under an area styled `backdrop = "blur"`.
pub const BACKDROP_BLUR: f32 = 24.0;

/// `painted` over a blurred copy of what the same surface drew under it, where `area` asks for a blur nothing but this surface can give ([`Blur::InSurface`]); unchanged otherwise, the compositor's blur being asked for by the window instead.
fn frosted(painted: StyledContainer, area: &ResolvedArea, surround: Surround) -> StyledContainer {
    match blur_of(surround.layer, area) {
        Some(Blur::InSurface) => painted.with_backdrop_blur(|| BACKDROP_BLUR),
        Some(Blur::Compositor) | None => painted,
    }
}

/// `layout` holding its contents off each side by what `padding` says there, and by nothing where it says nothing.
pub fn padded(layout: LayoutStyle, padding: Option<Sides>) -> LayoutStyle {
    let sides = padding.unwrap_or_default();
    layout
        .padding_top(sides.top())
        .padding_right(sides.right())
        .padding_bottom(sides.bottom())
        .padding_left(sides.left())
}

/// A picture claims the pointer only where the layout binds a gesture to its empty space ([`empty_space`]): otherwise it is behind the shell, not a part of it to press. It is the area's own box without the shadow and border [`framed`] draws around it, and its fill before `opacity`, which its callers apply to the whole picture.
fn picture(
    style: &Style,
    under: Option<Color>,
    theme: &NordTheme,
    layout: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, LayoutError> {
    let look = Look {
        fill: style
            .fill_color(theme)
            .or(under)
            .unwrap_or(Color::TRANSPARENT),
        border: None,
        shadow: None,
        ..Look::area(style, theme)
    };
    StyledContainer::new(
        padded(layout, style.padding),
        move |rect| look.paint(rect),
        children,
    )
}

/// `picture` cut to the area's corners inside a box at `layout` that casts the area's shadow under it and draws its border over it, so the cut takes neither. Only the paint is cut, and the gestures the area binds are on the box, so they answer over the whole of it.
fn framed(
    area: &ResolvedArea,
    surround: Surround,
    layout: LayoutStyle,
    picture: StyledContainer,
) -> Built {
    let look = Look::area(&area.style, &surround.theme);
    let shade = Look {
        fill: Color::TRANSPARENT,
        border: None,
        ..look
    };
    let edge = Look {
        fill: Color::TRANSPARENT,
        shadow: None,
        ..look
    };
    let cut: Box<dyn LayoutItem> = match area.style.radius {
        Some(radius) => Box::new(look::cut(Box::new(picture), radius.into(), true)),
        None => Box::new(picture),
    };
    let border = StyledContainer::new(
        LayoutStyle::new().absolute_fill(),
        move |rect| edge.paint(rect),
        Vec::new(),
    )?;
    let frame = StyledContainer::new(
        layout,
        move |rect| shade.paint(rect),
        vec![cut, Box::new(border)],
    )?;
    Ok(Box::new(empty_space(area, surround, frame, false)))
}

/// The box `rect` names, as the fraction of [`Surround::bounds`] that it is, taken out of flow so the areas of one layer stack over each other instead of pushing each other along.
///
/// In pixels rather than percentages, because the percentage would be of the window — the whole output — and an area written `within = "usable"` means a fraction of what the reserving areas left, which is a different box on every monitor and after every bar edit.
fn region(rect: Rect, surround: Surround) -> LayoutStyle {
    pixels(within(rect, surround.bounds))
}

/// Where `rect` lands, as a fraction of `bounds`, in the window's own coordinate space.
pub fn within(rect: Rect, bounds: telar::Rect) -> telar::Rect {
    telar::Rect::new(
        bounds.x + rect.x * bounds.width,
        bounds.y + rect.y * bounds.height,
        rect.w * bounds.width,
        rect.h * bounds.height,
    )
}

/// An absolute box at exactly `rect`, which is how every area is placed: one window is the whole output, so an area positions itself in it rather than being flowed with its neighbours.
pub fn at(rect: telar::Rect) -> LayoutStyle {
    pixels(rect)
}

fn pixels(rect: telar::Rect) -> LayoutStyle {
    LayoutStyle::new()
        .absolute()
        .inset_start(rect.x)
        .inset_top(rect.y)
        .width(rect.width)
        .height(rect.height)
}

/// The grid box: `span` cells of `cell` px `gap` apart, sized to exactly the block they make, so what is left of the region is what the anchor has to place it in.
fn tracks(span: Footprint, cell: f32, gap: f32) -> LayoutStyle {
    let extent = span.extent(cell, gap);
    LayoutStyle::new()
        .display_grid()
        .grid_template_columns(vec![TemplateTrack::repeat(
            span.columns,
            TemplateTrack::px(cell),
        )])
        .grid_template_rows(vec![TemplateTrack::repeat(
            span.rows,
            TemplateTrack::px(cell),
        )])
        .gap(gap)
        .width(extent.width)
        .height(extent.height)
}

/// How many cells the whole grid covers: as far along each axis as any group of it reaches.
fn covered(area: &ResolvedArea) -> Footprint {
    area.groups.iter().fold(
        Footprint {
            columns: 0,
            rows: 0,
        },
        |so_far, group| {
            let (col, row) = origin_of(group);
            let span = cells_of(group);
            Footprint {
                columns: so_far.columns.max(col.saturating_add(span.columns)),
                rows: so_far.rows.max(row.saturating_add(span.rows)),
            }
        },
    )
}

/// `build` is handed the surround again because a Smart Stack builds its child later, whenever it is cycled. `follows` keeps the group's box in step with where a live placement moves it.
fn arranged(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    style: LayoutStyle,
    follows: Option<Box<dyn Fn() -> LayoutStyle>>,
    surround: Surround,
    build: impl Fn(&ResolvedInstance, &rects::Node, Surround) -> Built + 'static,
) -> Built {
    let at = rects::Node::area(surround.output, surround.layer, &area.id);
    let group_id = group.id.clone();
    let instance_at = at.clone();
    let build: BuildCopy = Rc::new(
        move |instance: &ResolvedInstance, surround: Surround| -> Built {
            let node = instance_at.instance(&group_id, &instance.id);
            let built = build(instance, &node, surround)?;
            rects::track(node, built.layout_node());
            Ok(built)
        },
    );
    let key = (
        surround.output.map(str::to_string),
        area.id.clone(),
        group.id.clone(),
    );
    let slots = match &group.repeat {
        Some(expr) => {
            let repeat = Expressions::here(surround.audience).repeat(&at.group(&group.id), expr);
            let pages = LayoutStyle::new().flex_column();
            copies(group, Some(repeat), (key, pages), surround, build)
        }
        // A stack is the one child of the box its placement gives the group, so it is packed where the group would be: a dock's end zone packs it to the end instead of it stretching across the run.
        None => match group.is_pages() {
            true => {
                let stack = ChildSlot::stat(smart_stack(
                    key,
                    &group.children,
                    LayoutStyle::new().flex_column(),
                    surround,
                    move |instance, surround| build(instance, surround),
                )?);
                match group.children.is_empty() {
                    true => vec![stack, empty_hint_slot(surround, || true)],
                    false => vec![stack],
                }
            }
            false => group
                .children
                .iter()
                .map(|instance| build(instance, surround).map(ChildSlot::stat))
                .collect::<Result<Vec<_>, LayoutError>>()?,
        },
    };
    // A container shares its padded box out to its children itself ([`container::shares`]).
    let padding = match container::arranges(group) {
        true => None,
        false => group.style.padding,
    };
    let follows = follows.map(|follows| {
        Box::new(move || padded(follows(), padding)) as Box<dyn Fn() -> LayoutStyle>
    });
    let node = group_box(
        padded(style, padding),
        slots,
        plate_of(group, surround.theme),
        follows,
    )?;
    rects::track(at.group(&group.id), node.layout_node());
    Ok(node)
}

fn plate_of(group: &ResolvedGroup, theme: NordTheme) -> Option<Look> {
    look::draws_plate(group, false).then(|| Look::plate(&group.style, &theme, false))
}

/// A group's own box around `slots`, painted as `plate` says, kept in step with `follows`.
pub(crate) fn group_box(
    style: LayoutStyle,
    slots: Vec<ChildSlot>,
    plate: Option<Look>,
    follows: Option<Box<dyn Fn() -> LayoutStyle>>,
) -> Built {
    let fixed = slots
        .iter()
        .all(|slot| matches!(slot, ChildSlot::Static(_)));
    let unslotted = |slots: Vec<ChildSlot>| -> Vec<Box<dyn LayoutItem>> {
        slots
            .into_iter()
            .filter_map(|slot| match slot {
                ChildSlot::Static(item) => Some(item),
                ChildSlot::Dynamic(_) => None,
            })
            .collect()
    };
    Ok(match plate {
        Some(look) => {
            let paint = move |rect| look.paint(rect);
            let painted = match fixed {
                true => StyledContainer::new(style, paint, unslotted(slots))?,
                false => StyledContainer::from_slots(style, paint, slots)?,
            };
            let painted = painted_chrome(painted, look.fill);
            Box::new(match follows {
                Some(follows) => painted.styled_by(follows),
                None => painted,
            })
        }
        None => {
            let bare = match fixed {
                true => Container::new(style, unslotted(slots))?,
                false => Container::from_slots(style, slots)?,
            };
            Box::new(match follows {
                Some(follows) => bare.styled_by(follows),
                None => bare,
            })
        }
    })
}

/// Builds one instance of a group, under the surround it is handed — which a Smart Stack hands it again whenever it is cycled.
pub(crate) type BuildCopy = Rc<dyn Fn(&ResolvedInstance, Surround) -> Built>;

/// `group`'s children once per item of `repeat`, as what fills the group's own box: each copy a child of the box keyed by its index, so a list that grows or shrinks adds or drops copies at its end and leaves the others built; or, in a `pages` group, the copies as the pages of a stack laid out by `pages`, the stack built again only when how many there are changes. A `repeat` that cannot be read fills nothing.
pub(crate) fn copies(
    group: &ResolvedGroup,
    repeat: Option<Repeat>,
    (key, pages): (StackKey, LayoutStyle),
    surround: Surround,
    build: BuildCopy,
) -> Vec<ChildSlot> {
    let Some(repeat) = repeat else {
        return Vec::new();
    };
    let children: Rc<[ResolvedInstance]> = group.children.clone().into();
    let kept = Rc::new(Kept::of(surround));
    let entered: BuildCopy = {
        let repeat = repeat.clone();
        Rc::new(move |copy: &ResolvedInstance, surround: Surround| {
            repeat.enter(copy);
            build(copy, surround)
        })
    };
    let slot = match group.is_pages() {
        true => telar::fragment(
            move || vec![repeat.copies(&children)],
            |copies: &Vec<ResolvedInstance>| copies.len(),
            move |copies: Vec<ResolvedInstance>| {
                let entered = Rc::clone(&entered);
                or_nothing(smart_stack(
                    key.clone(),
                    &copies,
                    pages.clone(),
                    kept.surround(),
                    move |page, surround| entered(page, surround),
                ))
            },
            0.0,
        ),
        false => telar::fragment(
            move || repeat.copies(&children),
            |copy: &ResolvedInstance| copy.id.clone(),
            move |copy: ResolvedInstance| or_nothing(entered(&copy, kept.surround())),
            0.0,
        ),
    };
    vec![slot]
}

/// What a child built on its own draws when building it failed outright, below the error boundary every module is built in: nothing, rather than taking the group with it.
fn or_nothing(built: Built) -> Built {
    built.or_else(|failed| {
        tracing::warn!("a child of a group could not be built: {failed}");
        Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
    })
}

/// Per output as well: the same group is drawn on every output, and each is cycled on its own.
pub type StackKey = (Option<String>, AreaId, GroupId);

thread_local! {
    /// Kept across rebuilds, so an edit elsewhere in the layout does not undo cycling.
    static SHOWN: RefCell<HashMap<StackKey, PlacedId>> = RefCell::new(HashMap::new());
}

pub fn shown_in(key: &StackKey) -> Option<PlacedId> {
    SHOWN.with(|shown| shown.borrow().get(key).cloned())
}

/// Drops what is kept across rebuilds for areas, groups and instances the layout no longer places: the child a stack was showing, the picture a region last faded to (F-3.4).
pub fn forget_gone(gone: &layout::Placed) {
    SHOWN.with(|shown| {
        shown.borrow_mut().retain(|(_, area, group), child| {
            !gone.areas.contains(area)
                && !gone.groups.contains(&(area.clone(), group.clone()))
                && !gone.instances.contains(&child.template())
        })
    });
    OPENED.with(|opened| {
        opened
            .borrow_mut()
            .retain(|(_, area), _| !gone.areas.contains(area))
    });
}

/// Only the shown child is built, so a hidden widget runs no subscriptions; on the lock layer ([`Audience::Anyone`]) it answers nothing (TA-8).
pub(crate) fn smart_stack(
    key: StackKey,
    children: &[ResolvedInstance],
    style: LayoutStyle,
    surround: Surround,
    build: impl Fn(&ResolvedInstance, Surround) -> Built + 'static,
) -> Built {
    let children: Rc<[ResolvedInstance]> = children.into();
    let count = children.len();
    let start = shown_in(&key)
        .and_then(|id| children.iter().position(|child| child.id == id))
        .unwrap_or(0);
    let shown = signal(start);
    let kept = Kept::of(surround);
    let showing = Rc::clone(&children);
    let child =
        ReactiveList::with_style(
            LayoutStyle::new().flex_column().flex_grow(1.0),
            move || vec![shown.get()],
            |at: &usize| *at,
            move |at: usize| match showing.get(at) {
                Some(child) => build(child, kept.surround()),
                None => Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?)
                    as Box<dyn LayoutItem>),
            },
        )?;
    if count < 2 {
        return Ok(Box::new(Container::new(style, vec![Box::new(child)])?));
    }

    let menu_of_shown = shown_menu(&key, Rc::clone(&children), shown, surround.layer);
    let pick: Rc<dyn Fn(usize)> = Rc::new(move |at: usize| {
        let Some(child) = children.get(at) else {
            return;
        };
        shown.set(at);
        SHOWN.with(|shown| shown.borrow_mut().insert(key.clone(), child.id.clone()));
    });
    let answers = surround.audience == Audience::Owner;
    let dots = dots(
        count,
        shown,
        surround.theme,
        answers.then(|| Rc::clone(&pick)),
    )?;
    let stack = StyledContainer::new(style, |_| RectStyle::default(), vec![Box::new(child), dots])?;
    if !answers {
        return Ok(Box::new(stack));
    }

    let step: Rc<dyn Fn(isize)> = {
        let pick = Rc::clone(&pick);
        Rc::new(move |by: isize| {
            let next = (shown.peek() as isize + by).rem_euclid(count as isize);
            pick(next as usize);
        })
    };
    let travelled = Cell::new(0.0f32);
    let wheel = Rc::clone(&step);
    let navigation = ui::keynav::from_config(&surround.config.keynav);
    Ok(Box::new(
        stack
            .control(Role::TabPanel)
            .consumes_keys(|| ConsumedKeys::VERTICAL_ARROWS.with(ConsumedKeys::EDGES))
            .on_scroll(move |_, dy| {
                let so_far = travelled.get() + dy;
                if so_far.abs() < NOTCH {
                    travelled.set(so_far);
                    return;
                }
                travelled.set(0.0);
                wheel(if so_far > 0.0 { 1 } else { -1 });
            })
            .on_focused_key(move |key: &Key| match navigation.interpret(key) {
                _ if crate::menu::is_menu_key(key, telar::modifiers()) => {
                    menu_of_shown();
                    true
                }
                Some(Move::Next) => {
                    step(1);
                    true
                }
                Some(Move::Previous) => {
                    step(-1);
                    true
                }
                Some(Move::First) => {
                    pick(0);
                    true
                }
                Some(Move::Last) => {
                    pick(count - 1);
                    true
                }
                _ => false,
            }),
    ))
}

/// Opens the context menu of the child a stack is showing, from the keyboard: the stack is what holds the focus, and the child is what a menu is about.
fn shown_menu(
    key: &StackKey,
    children: Rc<[ResolvedInstance]>,
    shown: RwSignal<usize>,
    layer: LayerKind,
) -> Rc<dyn Fn()> {
    let (output, area, group) = key.clone();
    let window = LayerWindowContext::current().map_or(layer, |window| window.layer);
    Rc::new(move || {
        let Some(child) = children.get(shown.peek()) else {
            return;
        };
        crate::menu::open(crate::menu::Asked {
            node: rects::Node::area(output.as_deref(), layer, &area).instance(&group, &child.id),
            window,
            at: None,
        });
    })
}

const DOT: f32 = 6.0;
const DOT_GAP: f32 = 4.0;

/// Laid over the child rather than under it, so the stack keeps exactly the footprint its widgets cover.
fn dots(
    count: usize,
    shown: RwSignal<usize>,
    theme: NordTheme,
    pick: Option<Rc<dyn Fn(usize)>>,
) -> Built {
    let dots = (0..count)
        .map(|at| {
            let dot = StyledContainer::new(
                LayoutStyle::new().width(DOT).height(DOT),
                move |_| {
                    let ink = match shown.get() == at {
                        true => theme.text,
                        false => theme.text.with_alpha(0.35),
                    };
                    RectStyle::filled(ink, DOT / 2.0)
                },
                Vec::new(),
            )?;
            let dot = match &pick {
                Some(pick) => {
                    let pick = Rc::clone(pick);
                    dot.control(Role::Tab)
                        .toggled(move || shown.get() == at)
                        .on_press(move || pick(at))
                }
                None => dot,
            };
            Ok(Box::new(dot) as Box<dyn LayoutItem>)
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_end(0.0)
            .inset_bottom(DOT_GAP)
            .flex_row()
            .justify_content(telar::JustifyContent::CENTER)
            .gap(DOT_GAP),
        dots,
    )?))
}

struct Kept {
    config: Arc<Config>,
    theme: NordTheme,
    output: Option<String>,
    layer: LayerKind,
    bounds: telar::Rect,
    reserved: Reserved,
    audience: Audience,
}

impl Kept {
    fn of(surround: Surround) -> Self {
        Self {
            config: Arc::clone(surround.config),
            theme: surround.theme,
            output: surround.output.map(str::to_string),
            layer: surround.layer,
            bounds: surround.bounds,
            reserved: surround.reserved,
            audience: surround.audience,
        }
    }

    fn surround(&self) -> Surround<'_> {
        Surround {
            config: &self.config,
            theme: self.theme,
            output: self.output.as_deref(),
            layer: self.layer,
            bounds: self.bounds,
            reserved: self.reserved,
            audience: self.audience,
        }
    }
}

/// Where a group sits on its grid's tracks, and the column its instances are laid down.
fn on_cells(group: &ResolvedGroup, gap: f32) -> LayoutStyle {
    let span = cells_of(group);
    let style = match container::arranges(group) {
        true => LayoutStyle::new(),
        false => LayoutStyle::new().flex_column().gap(gap),
    };
    match group.kind {
        GroupKind::Cell { col, row, .. } => style
            .grid_column(line(cell_at(col)), span.columns)
            .grid_row(line(cell_at(row)), span.rows),
        _ => style
            .grid_column_span(span.columns)
            .grid_row_span(span.rows),
    }
}

/// One group on its cells, its instances down the box those cells make, each at the footprint its own size asks for. It follows the cells `live` places it on, and slides there.
fn cell_group(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    (cell, gap): (f32, f32),
    live: RwSignal<ResolvedArea>,
    surround: Surround,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let id = group.id.clone();
    let fallback = group.clone();
    let held = telar::memo(move || {
        live.with(|now| now.groups.iter().find(|held| held.id == id).cloned())
            .unwrap_or_else(|| fallback.clone())
    });
    if keyed(group) {
        return contained(area, group, held, (cell, gap), surround);
    }
    let follows = move || held.with(|now| on_cells(now, gap));
    let build: BuildPlaced = match container::arranges(group) {
        true => shared_out(group, (cell, gap)),
        false => Box::new(move |instance, node, surround| {
            place(
                instance,
                node,
                footprint(instance).extent(cell, gap),
                None,
                (LayoutStyle::new(), Frame::Bare),
                surround,
            )
        }),
    };
    arranged(
        area,
        group,
        on_cells(group, gap),
        Some(Box::new(follows)),
        surround,
        build,
    )
}

type BuildPlaced = Box<dyn Fn(&ResolvedInstance, &rects::Node, Surround) -> Built>;

/// A container on its cells: each child in its share of the box they make, drawn at what fits there and cut to it, and an empty one saying so while its layer's edit mode is up. Each child is built once and kept by its instance id through whatever [`moves_only`] lets through, which only lays it out again. A child given a share of another size follows it through its host's extent, and is built again, alone, only once that share fits another representation.
fn contained(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    held: Memo<ResolvedGroup>,
    (cell, gap): (f32, f32),
    surround: Surround,
) -> Built {
    let at = rects::Node::area(surround.output, surround.layer, &area.id);
    let audience = surround.audience;
    let shares = telar::memo(move || held.with(|group| Share::all(group, (cell, gap), audience)));
    let kept = Rc::new(Kept::of(surround));
    let slide = surround.config.animation.travel_tween_ms(200, 2_000);
    let (group_id, instance_at) = (group.id.clone(), at.clone());
    let children = telar::fragment(
        move || shares.get(),
        move |share: &Share| (share.instance.id.clone(), share.representation.as_str()),
        move |share: Share| {
            let node = instance_at.instance(&group_id, &share.instance.id);
            let (id, last) = (share.instance.id.clone(), (share.rect, share.told(cell)));
            let now = telar::memo(move || {
                shares.with(|now| {
                    now.iter()
                        .find(|now| now.instance.id == id)
                        .map_or(last, |now| (now.rect, now.told(cell)))
                })
            });
            let follows = move || slot_at(now.get().0);
            let told = Extent::following(move || now.get().1);
            let built = or_nothing(in_share(
                &share,
                &node,
                told,
                kept.surround(),
                Some(Box::new(follows)),
            ))?;
            rects::track(node, built.layout_node());
            Ok(Box::new(telar::animate_layout(built, slide)) as Box<dyn LayoutItem>)
        },
        0.0,
    );
    let hint = empty_hint_slot(surround, move || shares.with(Vec::is_empty));
    let follows = move || held.with(|now| on_cells(now, gap));
    let node = group_box(
        on_cells(group, gap),
        vec![children, hint],
        plate_of(group, surround.theme),
        Some(Box::new(follows)),
    )?;
    rects::track(at.group(&group.id), node.layout_node());
    Ok(node)
}

/// The hint a group draws over its box while `empty` holds and its layer's edit mode is up.
fn empty_hint_slot(surround: Surround, empty: impl Fn() -> bool + 'static) -> ChildSlot {
    let (output, layer, theme) = (
        surround.output.map(str::to_string),
        surround.layer,
        surround.theme,
    );
    telar::fragment(
        move || {
            let shown = empty() && expressions::is_edited(output.as_deref(), layer);
            shown.then_some(()).into_iter().collect()
        },
        |_: &()| (),
        move |()| empty_hint(theme),
        0.0,
    )
}

fn empty_hint(theme: NordTheme) -> Built {
    let text = Text::new(
        || telar::t!("container.empty"),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0))
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(telar::JustifyContent::CENTER),
        vec![box_item(text)],
    )?))
}

/// One child of a container in its share of the container's box, at the representation that fits there.
#[derive(Clone, PartialEq)]
struct Share {
    instance: ResolvedInstance,
    representation: Placed,
    rect: telar::Rect,
}

impl Share {
    fn of(
        instance: &ResolvedInstance,
        rect: telar::Rect,
        grid: (f32, f32),
        audience: Audience,
    ) -> Self {
        let room = Size {
            width: rect.width,
            height: rect.height,
        };
        Self {
            instance: instance.clone(),
            representation: container::fitted(&instance.module, room, grid, audience),
            rect,
        }
    }

    fn all(group: &ResolvedGroup, (cell, gap): (f32, f32), audience: Audience) -> Vec<Self> {
        let size = cells_of(group).extent(cell, gap);
        group
            .children
            .iter()
            .zip(container::shares(
                group,
                size,
                container::gap_of(group, false),
            ))
            .map(|(instance, rect)| Self::of(instance, rect, (cell, gap), audience))
            .collect()
    }

    /// The box the child is told it has: its whole share, or as much of it as a chip takes.
    fn told(&self, cell: f32) -> Size {
        let room = Size {
            width: self.rect.width,
            height: self.rect.height,
        };
        match self.representation {
            Placed::Chip => container::chip_extent(room, cell),
            _ => room,
        }
    }
}

/// Each child of a repeated container in its share of the box the container's cells make, built with the group.
fn shared_out(group: &ResolvedGroup, (cell, gap): (f32, f32)) -> BuildPlaced {
    let size = cells_of(group).extent(cell, gap);
    let shares: HashMap<PlacedId, telar::Rect> = group
        .children
        .iter()
        .map(|child| child.id.clone())
        .zip(container::shares(
            group,
            size,
            container::gap_of(group, false),
        ))
        .collect();
    let whole = telar::Rect::new(0.0, 0.0, size.width, size.height);
    Box::new(move |instance, node, surround| {
        let rect = shares.get(&instance.id).copied().unwrap_or(whole);
        let share = Share::of(instance, rect, (cell, gap), surround.audience);
        in_share(&share, node, share.told(cell).into(), surround, None)
    })
}

/// `share`'s child in a slot at its share, kept there by `follows` where the share moves, and cut to it.
fn in_share(
    share: &Share,
    node: &rects::Node,
    told: Extent,
    surround: Surround,
    follows: Option<Box<dyn Fn() -> LayoutStyle>>,
) -> Built {
    let drawn = ResolvedInstance {
        representation: share.representation,
        ..share.instance.clone()
    };
    let style = match share.representation {
        Placed::Chip => LayoutStyle::new(),
        _ => fill(),
    };
    let child = place(&drawn, node, told, None, (style, Frame::Shell), surround)?;
    let slot = Container::new(slot_at(share.rect), vec![child])?;
    let slot = match follows {
        Some(follows) => slot.styled_by(follows),
        None => slot,
    };
    // A styled child cuts its own content to its plate, so the share only has to stop short of the plate's shadow.
    let shadow = share
        .instance
        .style
        .shadow
        .and_then(ui::scale::elevation::shadow)
        .map_or(0.0, ui::scale::elevation::reach);
    Ok(Box::new(ClippedItem::new(
        Box::new(slot),
        Clip::both().inset(-shadow),
    )))
}

fn slot_at(share: telar::Rect) -> LayoutStyle {
    pixels(share)
        .flex_row()
        .align_items(AlignItems::CENTER)
        .justify_content(telar::JustifyContent::CENTER)
}

/// One group's share of a dock: an equal part of the strip's length, across the whole of its thickness, its instances packed towards its own zone.
fn run(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    edge: Edge,
    thickness: f32,
    surround: Surround,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let extent = if edge.is_horizontal() {
        Size {
            width: f32::INFINITY,
            height: thickness,
        }
    } else {
        Size {
            width: thickness,
            height: f32::INFINITY,
        }
    };
    let style = LayoutStyle::new()
        .flex_grow(1.0)
        .flex_basis(0.0)
        .align_items(AlignItems::STRETCH)
        .justify_content(justify(packing(zone_of(group))));
    let style = if edge.is_horizontal() {
        style.flex_row()
    } else {
        style.flex_column()
    };
    arranged(
        area,
        group,
        style,
        None,
        surround,
        move |instance, node, surround| {
            place(
                instance,
                node,
                extent,
                Some(edge),
                (fill(), Frame::Docked),
                surround,
            )
        },
    )
}

/// The strip a dock draws: the whole of its edge long, `thickness` across, laid out along that edge.
fn along(edge: Edge, thickness: f32) -> LayoutStyle {
    if edge.is_horizontal() {
        LayoutStyle::new()
            .flex_row()
            .width(SizeDimension::Percent(1.0))
            .height(thickness)
    } else {
        LayoutStyle::new()
            .flex_column()
            .width(thickness)
            .height(SizeDimension::Percent(1.0))
    }
}

/// How a chip is drawn where it is placed: bare, laid out by its module alone as a grid places one, docked, bare too but told it runs along a dock, or in the chip shell a bar puts it in, which pads it, lights it on hover and takes its presses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    Bare,
    Docked,
    Shell,
}

/// One instance under a host told the box it was given and the options its entry sets, built inside the descriptor table's error boundary so a module that fails shows its placeholder instead of taking the area down with it. An instance with bindings is built again, alone, each time what they say changes.
fn place(
    instance: &ResolvedInstance,
    node: &rects::Node,
    extent: impl Into<Extent>,
    axis: Option<Edge>,
    (style, frame): (LayoutStyle, Frame),
    surround: Surround,
) -> Built {
    // The last of three lines on "readings only, never controls" (TA-8): validation refuses a representation that acts on the lock layer, `layout add` refuses to place one, and a file that was hand-edited past both is drawn as a placeholder rather than built. Placed here because this is the one point every lock instance goes through, whatever kind of area holds it.
    if surround.audience == Audience::Anyone && !reads_only(instance) {
        return ui::placeholder::neutral(surround.theme);
    }
    let (placing, at, kept) = (instance.clone(), node.clone(), Kept::of(surround));
    let extent = extent.into();
    Expressions::here(surround.audience).bound_instance(
        instance,
        node,
        style,
        move |style, bound| {
            placed(
                &placing,
                &at,
                (extent, axis),
                (style, frame),
                kept.surround(),
                bound,
            )
        },
    )
}

/// A chip is registered as what its module's panel and its own press open from, at its own box, which a container's share can be larger than.
fn placed(
    instance: &ResolvedInstance,
    node: &rects::Node,
    (extent, axis): (Extent, Option<Edge>),
    (style, frame): (LayoutStyle, Frame),
    surround: Surround,
    bound: Option<&Overlay>,
) -> Built {
    let placed = Instance::new(
        InstanceId::new(instance.id.as_str()),
        &instance.module,
        Overlay::options_or(bound, &instance.options),
    );
    let presentation = placed.presentation(surround.config);
    let accent = Overlay::accent_or(bound, || {
        surround
            .theme
            .accent_by_name(surround.config.accent_name(&presentation))
    });
    let chip = match instance.representation {
        Placed::Chip => {
            ui::descriptor::find(&instance.module).and_then(|module| module.representations.chip)
        }
        _ => None,
    };
    let shell = chip.filter(|chip| frame == Frame::Shell && !chip.is_bare());
    let foreground = match shell {
        Some(_) => module_foreground(presentation.variant, accent, surround.theme),
        None => surround.theme.text,
    };
    let host = Host::placed(
        placed,
        Arc::clone(surround.config),
        representation(instance.representation),
        extent,
        axis,
        surround.config.shape_from(None, None, None, None),
        accent,
        foreground,
        surround.output.map(str::to_string),
    )
    .shown_to(surround.audience)
    .in_dock(frame == Frame::Docked);
    let own_secondary = (frame == Frame::Docked && surround.audience == Audience::Owner)
        .then(OwnSecondary::default);
    let gestures = Bound::of(&instance.actions, surround.audience)
        .with_menu(crate::menu::on(node.clone(), surround.audience))
        .owning(node.clone())
        .with_own_press(crate::panel::chip_press(&host, node));
    let gestures = match &own_secondary {
        Some(own) => gestures.with_own_secondary(own.clone()),
        None => gestures,
    };
    let wheel = gestures.wheel(
        chip.filter(|_| surround.audience == Audience::Owner)
            .and_then(|chip| crate::bar::wheel(&chip, &host)),
    );
    let plate = Look::of_instance(
        &instance.style,
        bound,
        &surround.theme,
        Rest::on_surface(
            &surround.theme,
            surround.config.opacity(),
            surround.config.shape_from(None, None, None, None).radius,
        ),
    );
    let item = match shell {
        Some(chip) => shelled(&host, chip, (&gestures, wheel), node, style, plate)?,
        None => bare(
            &host,
            (&gestures, wheel),
            style,
            plate,
            own_secondary.as_ref(),
        )?,
    };
    if chip.is_some()
        && surround.audience == Audience::Owner
        && let Some(rect) = track_layout(item.layout_node())
    {
        rects::track_opener(
            node.clone(),
            rect,
            rects::Chip {
                instance: host.instance(),
                site: Site::of_host(&host),
            },
        );
    }
    Ok(item)
}

/// The module's own tree, wrapped where its instance's style paints a plate or its gestures answer.
fn bare(
    host: &Host,
    (gestures, wheel): (&Bound, Option<Wheel>),
    style: LayoutStyle,
    plate: Option<Look>,
    own_secondary: Option<&OwnSecondary>,
) -> Built {
    let module = host.module();
    if gestures.is_empty() && wheel.is_none() && plate.is_none() {
        return ui::descriptor::place(module, host, style);
    }
    // Around the module's own tree rather than in it: whatever the module answers itself, a button inside a widget, stays its own, and the bound gestures and the menu answer everywhere else on it — a placeholder standing in for a module that failed included.
    let inner = LayoutStyle::new().flex_column().flex_grow(1.0);
    let answering = match gestures.is_empty() {
        true => ui::placeholder::Presses::fixing(),
        false => ui::placeholder::Presses::default(),
    };
    let placed = match (plate, own_secondary) {
        (None, None) => ui::descriptor::place_answering(module, host, inner, answering)?,
        (plate, own_secondary) => telar::Scope::with(|| {
            if let Some(own) = own_secondary {
                own.provide();
            }
            if plate.is_some() {
                ui::chrome::Plated::provide();
            }
            ui::descriptor::place_answering(module, host, inner, answering)
        })?,
    };
    let placed: Box<dyn LayoutItem> = match plate {
        Some(look) => Box::new(look::cut(placed, look.radius, false)),
        None => placed,
    };
    let wrapper = StyledContainer::new(
        style.flex_column(),
        move |rect| plate.map_or_else(RectStyle::default, |look| look.paint(rect)),
        vec![placed],
    )?;
    let wrapper = painted_chrome(wrapper, plate.map_or(Color::TRANSPARENT, |look| look.fill));
    Ok(Box::new(gestures.presses(wrapper).maybe_on_scroll(
        wheel.map(|run| move |dx, dy| run(dx, dy)),
    )))
}

/// A chip-framed module in the chip shell a bar puts it in, resting on what it is placed on: its presses answer in a chip's order — what the layout binds, the panel its instance owns, then its own press or its module's panel — with the chip in scope, so what they open hangs off it.
fn shelled(
    host: &Host,
    chip: ChipDef,
    (gestures, wheel): (&Bound, Option<Wheel>),
    at: &rects::Node,
    style: LayoutStyle,
    plate: Option<Look>,
) -> Built {
    let module = host.module();
    let descriptor = ui::descriptor::find(module).copied();
    let (built, gestures, at) = (host.clone(), gestures.clone(), at.clone());
    let shell = ui::descriptor::guard(module, host, style.clone(), gestures.fixing(), move || {
        let content = descriptor
            .and_then(|descriptor| descriptor.build(&built))
            .unwrap_or_else(|| {
                Err(LayoutError::Engine(format!(
                    "'{}' declares no chip",
                    built.module()
                )))
            })?;
        let look = crate::bar::ChipLook {
            variant: built.presentation().variant,
            rest: Color::TRANSPARENT,
            accent: built.accent,
            radius: built.corner_radius(),
            drag_open: None,
        };
        crate::bar::chip_shell(&built, &chip, &gestures, wheel, &at, look, content)
    })?;
    Ok(match plate {
        Some(look) => Box::new(painted_chrome(
            StyledContainer::new(style, move |rect| look.paint(rect), vec![shell])?,
            look.fill,
        )),
        None => shell,
    })
}

/// Whether the module's own build of this representation registers nothing that answers the pointer.
///
/// A module the installed table does not have answers `false`, which is the safe way round: a screen anyone can touch draws a placeholder rather than whatever an unknown id turns out to build.
fn reads_only(instance: &ResolvedInstance) -> bool {
    ui::descriptor::find(&instance.module)
        .and_then(|found| found.input(representation(instance.representation)))
        .is_some_and(|input| input == ui::descriptor::Input::ReadOnly)
}

/// The representation a host is built for, from the one the layout named. The layout's five placeable sizes; the descriptor table has two more, `Panel` and `Popout`, which are opened rather than placed.
pub fn representation(placed: Placed) -> Representation {
    match placed {
        Placed::Chip => Representation::Chip,
        Placed::WidgetS => Representation::Widget(WidgetSize::S),
        Placed::WidgetM => Representation::Widget(WidgetSize::M),
        Placed::WidgetL => Representation::Widget(WidgetSize::L),
        Placed::Card => Representation::Card,
    }
}

/// How many cells an instance covers: the footprint its widget size steps to, or a single cell for a representation the desktop grid has no size for.
fn footprint(instance: &ResolvedInstance) -> Footprint {
    match representation(instance.representation) {
        Representation::Widget(size) => size.footprint(),
        _ => Footprint {
            columns: 1,
            rows: 1,
        },
    }
}

/// The cells a group covers: a container exactly the span it was placed with, any other group that span widened to hold its instances down the column they are laid out in — or, in a `pages` group, the largest of them.
pub fn cells_of(group: &ResolvedGroup) -> Footprint {
    let asked = match group.kind {
        GroupKind::Cell {
            col_span, row_span, ..
        } => Footprint {
            columns: cell_span(col_span),
            rows: cell_span(row_span),
        },
        _ => Footprint {
            columns: 1,
            rows: 1,
        },
    };
    if container::arranges(group) {
        return asked;
    }
    // A Smart Stack shows one child at a time, so it covers the largest of them; any other group is a column of all of them.
    let one_at_a_time = group.is_pages();
    let held = group.children.iter().map(footprint).fold(
        Footprint {
            columns: 0,
            rows: 0,
        },
        |so_far, child| Footprint {
            columns: so_far.columns.max(child.columns),
            rows: match one_at_a_time {
                true => so_far.rows.max(child.rows),
                false => so_far.rows.saturating_add(child.rows),
            },
        },
    );
    Footprint {
        columns: asked.columns.max(held.columns),
        rows: asked.rows.max(held.rows),
    }
}

/// The cell a group starts at. A group that names none is auto-placed, and counts from the origin towards the grid's own size.
fn origin_of(group: &ResolvedGroup) -> (u16, u16) {
    match group.kind {
        GroupKind::Cell { col, row, .. } => (cell_at(col), cell_at(row)),
        _ => (0, 0),
    }
}

/// A cell coordinate as the grid counts them, clamped to what a grid of this many tracks can hold.
fn cell_at(at: u32) -> u16 {
    u16::try_from(at).unwrap_or(u16::MAX)
}

/// A span as a cell count, never zero: a group covering no cell is a group nothing in it could be seen in.
fn cell_span(span: u32) -> u16 {
    cell_at(span).max(1)
}

/// The 1-based grid line a 0-based cell starts at, which is how CSS and taffy count them.
fn line(at: u16) -> i16 {
    i16::try_from(at.saturating_add(1)).unwrap_or(i16::MAX)
}

/// Where a block sits inside its region, on the two axes a row lays it out by: the vertical is that row's cross axis and the horizontal its main one.
fn anchored(anchor: Anchor) -> (Align, Align) {
    match anchor {
        Anchor::TopLeft => (Align::Start, Align::Start),
        Anchor::Top => (Align::Start, Align::Center),
        Anchor::TopRight => (Align::Start, Align::End),
        Anchor::Left => (Align::Center, Align::Start),
        Anchor::Center => (Align::Center, Align::Center),
        Anchor::Right => (Align::Center, Align::End),
        Anchor::BottomLeft => (Align::End, Align::Start),
        Anchor::Bottom => (Align::End, Align::Center),
        Anchor::BottomRight => (Align::End, Align::End),
    }
}

/// Where a strip sits against the edge it hugs, on the same two axes [`anchored`] answers on.
fn hugging(edge: Edge) -> (Align, Align) {
    match edge {
        Edge::Top => (Align::Start, Align::Center),
        Edge::Bottom => (Align::End, Align::Center),
        Edge::Left => (Align::Center, Align::Start),
        Edge::Right => (Align::Center, Align::End),
    }
}

/// Which end of its share of the strip a run packs towards.
fn packing(zone: Zone) -> Align {
    match zone {
        Zone::Start => Align::Start,
        Zone::Center => Align::Center,
        Zone::End => Align::End,
    }
}

/// Which run of the strip a group is in. A group that names no zone opens it, since the three runs of an edge are all a dock has to put one in.
fn zone_of(group: &ResolvedGroup) -> Zone {
    match group.kind {
        GroupKind::Zone { zone } => zone,
        _ => Zone::Start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use telar::{
        AvailableSpace, BorderRadius, RwSignal, compute_layout, new_container,
        reset_layout_runtime, set_theme, track_layout,
    };

    use layout::{AreaId, Arrange, GroupId, InstanceId as PlacedId, Style};
    use ui::descriptor::{Input, ModuleDescriptor, Representations, WidgetDef};

    use crate::test_rig::{self, cell};

    fn instance(module: &str, representation: Placed) -> ResolvedInstance {
        test_rig::instance(module, module, representation)
    }

    fn group(kind: GroupKind, children: Vec<ResolvedInstance>) -> ResolvedGroup {
        test_rig::group("run", kind, children)
    }

    fn stacked_group(kind: GroupKind, children: Vec<ResolvedInstance>) -> ResolvedGroup {
        ResolvedGroup {
            arrange: Some(Arrange::Pages),
            ..group(kind, children)
        }
    }

    fn area(kind: ResolvedAreaKind, groups: Vec<ResolvedGroup>) -> ResolvedArea {
        test_rig::area("area", kind, groups)
    }

    thread_local! {
        static LANDED: RefCell<Option<(Size, RwSignal<telar::Rect>)>> =
            const { RefCell::new(None) };
    }

    /// Publishes the box `host` was told it has and where the widget built for it ended up, which is the only way to ask an area where it put something.
    fn landed(host: &Host, item: Container) -> Built {
        let rect = track_layout(item.layout_node()).expect("a container registers its rect");
        LANDED.with(|seen| *seen.borrow_mut() = Some((host.extent(), rect)));
        Ok(Box::new(item))
    }

    /// A widget with a size of its own, which lands wherever the area put it and nowhere else.
    fn probe(host: &Host) -> Built {
        landed(
            host,
            Container::new(LayoutStyle::new().width(10.0).height(10.0), Vec::new())?,
        )
    }

    /// A widget that takes everything it is given, which is how a box that was never made definite shows up as one that collapsed.
    fn filler(host: &Host) -> Built {
        landed(host, Container::new(fill(), Vec::new())?)
    }

    const fn widget(build: ui::descriptor::Build) -> Representations {
        Representations {
            chip: None,
            widget: Some(WidgetDef {
                sizes: &WidgetSize::ALL,
                build,
                input: Input::ReadOnly,
            }),
            card: None,
            panel: None,
            popout: None,
        }
    }

    static PROBES: &[ModuleDescriptor] = &[
        ModuleDescriptor {
            id: "probe",
            name: "probe",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: widget(probe),
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "filler",
            name: "filler",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: widget(filler),
            actions: &[],
            sources: &[],
        },
    ];

    /// The screen these tests stand an area on, which is also the page they lay it out at.
    const PAGE: (f32, f32) = (1000.0, 800.0);

    /// Lays `built` out on a `width` by `height` surface and answers what the probe in it was given and where it landed.
    fn measured(built: Built, width: f32, height: f32) -> (Size, telar::Rect) {
        let item = built.expect("the area builds");
        let root = new_container(
            LayoutStyle::new().width(width).height(height),
            &[item.layout_node()],
        )
        .expect("a surface to lay out on");
        compute_layout(
            root,
            AvailableSpace::Definite(width),
            AvailableSpace::Definite(height),
        )
        .expect("a laid out surface");
        let (extent, rect) = LANDED
            .with(|landed| *landed.borrow())
            .expect("the probe was built");
        (extent, rect.get())
    }

    /// An area built on a screen the size of the page [`measured`] lays it out on, so where it places itself is where the page can show it.
    fn surrounded(config: &Arc<Config>) -> Surround<'_> {
        Surround {
            config,
            theme: config.resolve_theme(),
            output: None,
            layer: LayerKind::Desktop,
            bounds: telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1),
            reserved: Reserved::default(),
            audience: Audience::Owner,
        }
    }

    /// A widget sits on the cell it was placed at, and the box it is told it has is the one its footprint covers — which is what a face shrinks itself to fit.
    #[test]
    fn a_grid_puts_a_widget_on_its_cell_and_tells_it_the_footprint_it_covers() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();

        let placed = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![group(cell(1, 1), vec![instance("probe", Placed::WidgetM)])],
        );
        let (extent, rect) = measured(
            grid(
                &placed,
                Rect::default(),
                80.0,
                16.0,
                Anchor::TopLeft,
                surrounded(&config),
            ),
            1000.0,
            800.0,
        );

        assert_eq!(extent, WidgetSize::M.extent());
        assert_eq!(
            (rect.x, rect.y),
            (96.0, 96.0),
            "one cell and one gap in on each axis"
        );
    }

    /// A widget is handed its footprint's width and is left to say how tall it is, which is the box every widget the shell ships is measured in and why a grid of them lines up.
    #[test]
    fn a_grid_hands_a_widget_the_width_of_the_cells_it_covers() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();

        let placed = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::Center,
            },
            vec![group(cell(0, 0), vec![instance("filler", Placed::WidgetS)])],
        );
        let (_, rect) = measured(
            grid(
                &placed,
                Rect::default(),
                80.0,
                16.0,
                Anchor::Center,
                surrounded(&config),
            ),
            1000.0,
            800.0,
        );

        assert_eq!(rect.width, WidgetSize::S.extent().width);
    }

    /// A dock's contents are given the whole of the strip, and the strip is against the edge it hugs — which is what a row of bars stands on.
    #[test]
    fn a_dock_hands_its_run_the_strip_it_hugs_its_edge_with() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();

        let placed = area(
            ResolvedAreaKind::Dock {
                edge: Edge::Bottom,
                thickness: 200.0,
            },
            vec![group(
                GroupKind::Zone { zone: Zone::Center },
                vec![instance("filler", Placed::WidgetL)],
            )],
        );
        let (extent, rect) = measured(
            dock(&placed, Edge::Bottom, 200.0, surrounded(&config)),
            1000.0,
            800.0,
        );

        assert_eq!(extent.height, 200.0, "as deep as the strip");
        assert!(
            extent.width.is_infinite(),
            "and with no length of its own along it"
        );
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (0.0, 600.0, 1000.0, 200.0),
            "the strip is against the bottom edge and its run has the whole of it"
        );
    }

    /// A widget covers the cells its size steps to, whatever span the group was written with: a placement says where a widget is, and how big it is is the widget's own answer.
    #[test]
    fn a_group_covers_its_widget_even_where_it_was_placed_on_one_cell() {
        let span = cells_of(&group(cell(0, 0), vec![instance("clock", Placed::WidgetM)]));
        assert_eq!((span.columns, span.rows), (4, 2));
        assert_eq!(
            span.extent(80.0, 16.0),
            WidgetSize::M.extent(),
            "and the box those cells make is the footprint's own"
        );

        let wide = cells_of(&group(
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 6,
                row_span: 1,
            },
            vec![instance("clock", Placed::WidgetM)],
        ));
        assert_eq!(
            (wide.columns, wide.rows),
            (6, 2),
            "a span wider than the widget is the group's, a span narrower is the widget's"
        );

        let stacked = cells_of(&group(
            cell(0, 0),
            vec![
                instance("clock", Placed::WidgetM),
                instance("visualiser", Placed::WidgetL),
            ],
        ));
        assert_eq!(
            (stacked.columns, stacked.rows),
            (4, 6),
            "instances in one group are a column, so their rows add and their columns do not"
        );
    }

    /// The grid is as big as the block its groups cover, not as big as the region it is anchored in.
    #[test]
    fn the_grid_box_is_the_cells_its_groups_reach() {
        let placed = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::Center,
            },
            vec![
                group(cell(0, 0), vec![instance("clock", Placed::WidgetM)]),
                group(cell(4, 2), vec![instance("visualiser", Placed::WidgetS)]),
            ],
        );
        let span = covered(&placed);
        assert_eq!((span.columns, span.rows), (6, 4));

        let box_of = tracks(span, 80.0, 16.0);
        assert_eq!(box_of.width_px(), Some(6.0 * 80.0 + 5.0 * 16.0));
        assert_eq!(box_of.height_px(), Some(4.0 * 80.0 + 3.0 * 16.0));
    }

    #[test]
    fn a_cell_is_placed_on_the_grid_line_after_it() {
        assert_eq!(line(0), 1);
        assert_eq!(line(3), 4);
        assert_eq!(cell_span(0), 1, "a span of no cells is still one cell");
        assert_eq!(cell_at(u32::MAX), u16::MAX);
    }

    /// The nine anchors and the four edges land in nine and four distinct places, so no two ways of asking for a corner mean the same one.
    #[test]
    fn every_anchor_and_every_edge_places_a_block_somewhere_of_its_own() {
        let corners: Vec<(Align, Align)> = [
            Anchor::TopLeft,
            Anchor::Top,
            Anchor::TopRight,
            Anchor::Left,
            Anchor::Center,
            Anchor::Right,
            Anchor::BottomLeft,
            Anchor::Bottom,
            Anchor::BottomRight,
        ]
        .into_iter()
        .map(anchored)
        .collect();
        for (index, corner) in corners.iter().enumerate() {
            for other in &corners[index + 1..] {
                assert_ne!(corner, other, "two anchors land in the same spot");
            }
        }

        let edges: Vec<(Align, Align)> = Edge::ALL.into_iter().map(hugging).collect();
        for (index, at) in edges.iter().enumerate() {
            for other in &edges[index + 1..] {
                assert_ne!(at, other, "two edges hug the same side");
            }
        }
    }

    /// A dock's strip is the whole of its edge long and its thickness across, whichever edge it hangs off.
    #[test]
    fn a_strip_runs_the_length_of_its_edge_and_no_further_across() {
        for edge in Edge::ALL {
            let strip = along(edge, 200.0);
            let (along_it, across_it) = if edge.is_horizontal() {
                (strip.width_px(), strip.height_px())
            } else {
                (strip.height_px(), strip.width_px())
            };
            assert_eq!(across_it, Some(200.0), "{edge:?} is its thickness across");
            assert_eq!(
                along_it, None,
                "{edge:?} runs the whole edge, not a length of its own"
            );
        }
    }

    #[test]
    fn a_group_that_names_no_zone_opens_the_strip() {
        assert_eq!(
            zone_of(&group(
                GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 1,
                    row_span: 1
                },
                Vec::new()
            )),
            Zone::Start
        );
        assert_eq!(
            zone_of(&group(GroupKind::Zone { zone: Zone::End }, Vec::new())),
            Zone::End
        );
    }

    /// Lays `built` out on the page and hands back what it draws, for a test that reads the paint rather than where a probe landed.
    fn drawn(built: Built) -> telar::ComponentList {
        let page = Container::new(
            LayoutStyle::new().width(PAGE.0).height(PAGE.1),
            vec![built.expect("the area builds")],
        )
        .expect("a page");
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(PAGE.0),
            AvailableSpace::Definite(PAGE.1),
        )
        .expect("a laid out page");
        tree
    }

    fn rects_filled(tree: &telar::ComponentList, fill: Color) -> Vec<(telar::Rect, BorderRadius)> {
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::Rect { rect, style }
                    if style.fill == Some(Paint::Solid(fill)) =>
                {
                    Some((*rect, style.radius))
                }
                _ => None,
            })
            .collect()
    }

    /// F-10.38: what an area's `style` says is what its box draws — the fill at its opacity across the whole region, each corner at its own radius, and the contents held off the edges by its padding.
    #[test]
    fn an_areas_fill_radius_opacity_and_padding_reach_the_box_it_draws() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();

        let mut placed = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![group(cell(0, 0), vec![instance("probe", Placed::WidgetS)])],
        );
        placed.style = Style {
            fill: Some("#ff0000".to_string()),
            radius: Some(layout::Corners::each(1.0, 2.0, 3.0, 4.0)),
            opacity: Some(0.5),
            padding: Some(layout::Sides::all(10.0)),
            backdrop: None,
            ..Style::default()
        };
        let tree = drawn(build(&placed, surrounded(&config)).expect("a grid builds"));

        let red = Color::from_hex("#ff0000").unwrap().with_alpha(0.5);
        assert_eq!(
            rects_filled(&tree, red),
            [(
                telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1),
                BorderRadius {
                    top_left: 1.0,
                    top_right: 2.0,
                    bottom_right: 3.0,
                    bottom_left: 4.0,
                }
            )],
            "one rect, the whole region, at the fill's colour and the area's opacity, each corner its own"
        );
        let (_, landed) = LANDED
            .with(|landed| *landed.borrow())
            .expect("the probe was built");
        let landed = landed.get();
        assert_eq!(
            (landed.x, landed.y),
            (10.0, 10.0),
            "held off the edges by its padding"
        );

        let theme = config.resolve_theme();
        placed.style = Style {
            fill: Some("surface".to_string()),
            ..Style::default()
        };
        let tree = drawn(build(&placed, surrounded(&config)).expect("a grid builds"));
        assert_eq!(
            rects_filled(&tree, theme.surface).len(),
            1,
            "a theme token paints the theme's colour, as it is"
        );
    }

    /// An area, a container in it and an instance in that each draw the border and the shadow their style names, at their own box; the shadows reach past those boxes and none of that is claimed for the input region.
    #[test]
    fn a_style_draws_its_border_and_shadow_at_its_box_and_claims_none_of_the_shadow() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();

        let styled = |line: &str, shadow: u8| Style {
            fill: Some("surface".to_string()),
            border: Some(layout::Border {
                width: Some(2.0),
                color: Some(line.to_string()),
            }),
            shadow: Some(shadow),
            ..Style::default()
        };
        let child = ResolvedInstance {
            style: styled("#00ff00", 1),
            placement: Some(layout::Placement::Weight(1.0)),
            ..instance("filler", Placed::WidgetS)
        };
        let container = ResolvedGroup {
            arrange: Some(Arrange::Row),
            style: styled("#0000ff", 2),
            ..group(
                GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span: 4,
                    row_span: 2,
                },
                vec![child],
            )
        };
        let mut placed = area(
            ResolvedAreaKind::Grid {
                rect: Rect {
                    x: 0.1,
                    y: 0.1,
                    w: 0.5,
                    h: 0.5,
                },
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![container],
        );
        placed.style = styled("#ff0000", 3);
        let tree = drawn(build(&placed, surrounded(&config)).expect("a grid builds"));

        let edged = |line: &str| -> Vec<(telar::Rect, Option<telar::Shadow>)> {
            let line = Paint::Solid(Color::from_hex(line).expect("a colour"));
            tree.commands()
                .iter()
                .filter_map(|command| match command {
                    telar::DrawCommand::Rect { rect, style }
                        if style.border.is_some_and(|border| border.paint == line) =>
                    {
                        Some((*rect, style.shadow))
                    }
                    _ => None,
                })
                .collect()
        };
        let region = telar::Rect::new(100.0, 80.0, 500.0, 400.0);
        let pad = ui::scale::plate::padding(false);
        assert_eq!(
            edged("#ff0000"),
            [(region, ui::scale::elevation::shadow(3))],
            "the area's"
        );
        assert_eq!(
            edged("#0000ff"),
            [(
                telar::Rect::new(100.0, 80.0, 368.0, 176.0),
                ui::scale::elevation::shadow(2)
            )],
            "the container's plate, on the cells it covers"
        );
        assert_eq!(
            edged("#00ff00"),
            [(
                telar::Rect::new(
                    100.0 + pad,
                    80.0 + pad,
                    368.0 - 2.0 * pad,
                    176.0 - 2.0 * pad
                ),
                ui::scale::elevation::shadow(1)
            )],
            "the instance's, on its share of the padded plate"
        );

        let claimed = telar::interactive_rects();
        assert!(claimed.contains(&region), "the filled area is claimed");
        for claim in claimed {
            assert_eq!(
                claim.intersect(region),
                Some(claim),
                "{claim:?} reaches past every box into a shadow"
            );
        }
    }

    fn picture_file(dir: &Path, name: &str, pixel: [u8; 4]) -> PathBuf {
        std::fs::create_dir_all(dir).expect("a scratch directory");
        let path = dir.join(name);
        image::RgbaImage::from_pixel(2, 2, image::Rgba(pixel))
            .save(&path)
            .expect("a picture on disk");
        path
    }

    fn scratch(test: &str) -> PathBuf {
        util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("area-{test}"))
    }

    fn region_area(id: &str, rect: Rect, source: &Path, transition: Transition) -> ResolvedArea {
        test_rig::area(
            id,
            ResolvedAreaKind::WallpaperRegion {
                rect,
                source: source.display().to_string(),
                fit: Fit::Cover,
                transition,
                focus: Focus::MIDDLE,
                dim: 0.0,
                blur: 0.0,
                parallax: 0.0,
            },
            Vec::new(),
        )
    }

    /// Every picture a tree draws, in the order it draws them, with the part of the page it can reach — its rect, placed by the translations around it and cut by every clip — leaving out the blank an empty layer stands in with.
    fn pictures(tree: &telar::ComponentList) -> Vec<(Arc<ImageData>, telar::Rect)> {
        let mut offsets: Vec<(f32, f32)> = vec![(0.0, 0.0)];
        let mut clips: Vec<telar::Rect> = Vec::new();
        let mut found = Vec::new();
        let placed = |rect: &telar::Rect, (dx, dy): (f32, f32)| {
            telar::Rect::new(rect.x + dx, rect.y + dy, rect.width, rect.height)
        };
        for command in tree.commands().iter() {
            let here = *offsets.last().expect("the page's own origin");
            match command {
                telar::DrawCommand::PushMatrix { matrix } => {
                    offsets.push((here.0 + matrix[4], here.1 + matrix[5]))
                }
                telar::DrawCommand::PopMatrix => {
                    offsets.pop();
                }
                telar::DrawCommand::PushClip { rect, .. } => clips.push(placed(rect, here)),
                telar::DrawCommand::PopClip => {
                    clips.pop();
                }
                telar::DrawCommand::Image { data, rect, .. } if !Arc::ptr_eq(data, &blank()) => {
                    let reach = clips.iter().fold(placed(rect, here), |reach, clip| {
                        let (x, y) = (reach.x.max(clip.x), reach.y.max(clip.y));
                        let right = (reach.x + reach.width).min(clip.x + clip.width);
                        let bottom = (reach.y + reach.height).min(clip.y + clip.height);
                        telar::Rect::new(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
                    });
                    found.push((Arc::clone(data), reach));
                }
                _ => {}
            }
        }
        found
    }

    /// F-10.22: a region that names its own picture shows it whatever `[background]` says, and one that names none shows `[background]`.
    #[test]
    fn a_region_keeps_its_own_source_and_one_without_follows_the_background() {
        reset_layout_runtime();
        let dir = scratch("precedence");
        let own = picture_file(&dir, "own.png", [255, 0, 0, 255]);
        let configured = picture_file(&dir, "configured.png", [0, 0, 255, 255]);
        let mut config = Config::starter();
        config.background.image = Some(configured.clone());
        let config = Arc::new(config);
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();

        let mine = region_area("mine", Rect::default(), &own, Transition::None);
        let _tree = drawn(build(&mine, surrounded(&config)).expect("a region builds"));
        let shown = opened(&(None, AreaId::new("mine"))).expect("it opened a picture");
        assert_eq!(
            shown.path, own,
            "its own source, not the configured wallpaper"
        );

        let follows = region_area("follows", Rect::default(), Path::new(""), Transition::None);
        let _tree = drawn(build(&follows, surrounded(&config)).expect("a region builds"));
        let shown = opened(&(None, AreaId::new("follows"))).expect("it opened a picture");
        assert_eq!(
            shown.path, configured,
            "no source of its own is whatever [background] says"
        );
    }

    fn rounded_cuts(tree: &telar::ComponentList) -> Vec<(telar::Rect, BorderRadius)> {
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::PushClip { rect, radius }
                    if *radius != BorderRadius::zero() =>
                {
                    Some((*rect, *radius))
                }
                _ => None,
            })
            .collect()
    }

    /// A corner is never rounder than half the short side, and the cut on a picture and on a styled instance's content holds to it as the paint does, so the two stay one outline.
    #[test]
    fn a_cut_rounds_no_further_than_the_paint_it_matches() {
        reset_layout_runtime();
        let dir = scratch("clamped");
        let own = picture_file(&dir, "own.png", [255, 0, 0, 255]);
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(PROBES);
        let _scope = telar::owner_scope();
        let huge = Some(layout::Corners::each(5000.0, 5000.0, 5000.0, 7.0));

        let mut region = region_area("round", Rect::default(), &own, Transition::None);
        region.style.radius = huge;
        let tree = drawn(build(&region, surrounded(&config)).expect("a region builds"));
        let cuts = rounded_cuts(&tree);
        assert!(!cuts.is_empty(), "the picture is cut to its corners");
        for (rect, radius) in cuts {
            let most = rect.width.min(rect.height) / 2.0;
            assert_eq!(
                (radius.top_left, radius.bottom_left),
                (most, 7.0),
                "{rect:?}"
            );
        }

        let mut styled = instance("probe", Placed::WidgetS);
        styled.style = Style {
            fill: Some("surface".to_string()),
            radius: huge,
            ..Style::default()
        };
        let grid = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![group(cell(0, 0), vec![styled])],
        );
        let tree = drawn(build(&grid, surrounded(&config)).expect("a grid builds"));
        let cuts = rounded_cuts(&tree);
        assert!(!cuts.is_empty(), "the content is cut to the plate");
        for (rect, radius) in cuts {
            let most = rect.width.min(rect.height) / 2.0;
            assert_eq!(radius.top_left, most, "{rect:?}");
        }
    }

    fn backdrop_blurs(tree: &telar::ComponentList) -> Vec<f32> {
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::PushLayer { backdrop_blur, .. } if *backdrop_blur > 0.0 => {
                    Some(*backdrop_blur)
                }
                _ => None,
            })
            .collect()
    }

    /// F-2.12, F-10.49: what is under an area of the background layer is the pictures its own surface drew, so the shell frosts them itself, on any compositor; an area of a layer application windows sit among leaves its blur to the compositor, and its own paint stays unblurred.
    #[test]
    fn a_background_area_styled_to_blur_frosts_what_its_own_surface_drew_under_it() {
        reset_layout_runtime();
        let dir = scratch("frosted");
        let own = picture_file(&dir, "own.png", [255, 0, 0, 255]);
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();
        let mut frosted = region_area("frosted", Rect::default(), &own, Transition::None);
        frosted.style.backdrop = Some(layout::Backdrop::Blur);

        let on_background = Surround {
            layer: LayerKind::Background,
            ..surrounded(&config)
        };
        let tree = drawn(build(&frosted, on_background).expect("a region builds"));
        assert_eq!(backdrop_blurs(&tree), [BACKDROP_BLUR]);

        let tree = drawn(build(&frosted, surrounded(&config)).expect("a region builds"));
        assert!(
            backdrop_blurs(&tree).is_empty(),
            "the desktop layer's blur is the compositor's"
        );
    }

    /// A rebuild is how a region's own picture changes — `wallpaper set --region` is a layout edit — and a fresh node has nothing of the old picture to fade from, so the region remembers what it showed and starts there.
    #[test]
    fn a_region_rebuilt_for_a_new_picture_fades_from_the_one_it_showed() {
        reset_layout_runtime();
        let dir = scratch("fade");
        let before = picture_file(&dir, "before.png", [255, 0, 0, 255]);
        let after = picture_file(&dir, "after.png", [0, 255, 0, 255]);
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();

        let first = drawn(
            build(
                &region_area("left", Rect::default(), &before, Transition::None),
                surrounded(&config),
            )
            .expect("a region builds"),
        );
        let old = pictures(&first)
            .first()
            .map(|(data, _)| Arc::clone(data))
            .expect("the first picture is drawn");

        let rebuilt = drawn(
            build(
                &region_area("left", Rect::default(), &after, Transition::None),
                surrounded(&config),
            )
            .expect("a region builds"),
        );
        let drawn_now = pictures(&rebuilt);
        assert!(
            drawn_now.iter().any(|(data, _)| Arc::ptr_eq(data, &old)),
            "the rebuilt region still holds the picture it was showing, to hand over from"
        );
        assert!(
            drawn_now.iter().any(|(data, _)| !Arc::ptr_eq(data, &old)),
            "and the new one beside it"
        );
        assert_eq!(
            opened(&(None, AreaId::new("left"))).map(|shown| shown.path),
            Some(after),
            "what it shows now is what the next rebuild fades from"
        );
    }

    /// Two regions side by side paint only inside their own boxes, so a hand-over in one — which changes only that region's layers, and telar's diff damages only the elements that changed (F-5.1, F-5.2) — repaints nothing of the other.
    #[test]
    fn two_regions_each_paint_only_inside_their_own_box() {
        reset_layout_runtime();
        let dir = scratch("split");
        let red = picture_file(&dir, "red.png", [255, 0, 0, 255]);
        let blue = picture_file(&dir, "blue.png", [0, 0, 255, 255]);
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();

        let halves = [
            (
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                },
                &red,
                "left",
            ),
            (
                Rect {
                    x: 0.5,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                },
                &blue,
                "right",
            ),
        ];
        let built = halves
            .iter()
            .map(|(rect, source, id)| {
                build(
                    &region_area(id, *rect, source, Transition::None),
                    surrounded(&config),
                )
                .expect("a region builds")
            })
            .collect::<Result<Vec<_>, _>>()
            .expect("both regions build");
        let tree = drawn(Ok(Box::new(
            Container::new(fill(), built).expect("a layer of two regions"),
        )));

        let drawn_images = pictures(&tree);
        assert_eq!(drawn_images.len(), 2, "one picture each");
        for ((data, rect), (half, _, id)) in drawn_images.iter().zip(&halves) {
            let own = within(*half, telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1));
            assert_eq!(
                *rect, own,
                "`{id}` paints its picture over exactly its own box and nowhere else"
            );
            let shown = opened(&(None, AreaId::new(id))).expect("it opened a picture");
            assert!(
                Arc::ptr_eq(data, &shown.image),
                "`{id}` draws its own picture"
            );
        }
    }

    fn wide_picture(dir: &Path, name: &str, (width, height): (u32, u32)) -> PathBuf {
        std::fs::create_dir_all(dir).expect("a scratch directory");
        let path = dir.join(name);
        image::RgbaImage::from_pixel(width, height, image::Rgba([90, 120, 200, 255]))
            .save(&path)
            .expect("a picture on disk");
        path
    }

    fn laid_region(
        source: &Path,
        focus: Focus,
        (dim, blur, parallax): (f32, f32, f32),
    ) -> ResolvedArea {
        test_rig::area(
            "laid",
            ResolvedAreaKind::WallpaperRegion {
                rect: Rect::default(),
                source: source.display().to_string(),
                fit: Fit::Cover,
                transition: Transition::None,
                focus,
                dim,
                blur,
                parallax,
            },
            Vec::new(),
        )
    }

    fn unanimated(reduced: config::ReducedMotion) -> Arc<Config> {
        let mut config = Config::starter();
        config.animation.enabled = false;
        config.animation.reduced = reduced;
        Arc::new(config)
    }

    /// Where every picture the tree draws is laid, placed by the translations around it but not cut by any clip.
    fn laid_pictures(tree: &telar::ComponentList) -> Vec<telar::Rect> {
        let mut offsets: Vec<(f32, f32)> = vec![(0.0, 0.0)];
        let mut found = Vec::new();
        for command in tree.commands().iter() {
            let here = *offsets.last().expect("the page's own origin");
            match command {
                telar::DrawCommand::PushMatrix { matrix } => {
                    offsets.push((here.0 + matrix[4], here.1 + matrix[5]))
                }
                telar::DrawCommand::PopMatrix => {
                    offsets.pop();
                }
                telar::DrawCommand::Image { data, rect, .. } if !Arc::ptr_eq(data, &blank()) => {
                    found.push(telar::Rect::new(
                        rect.x + here.0,
                        rect.y + here.1,
                        rect.width,
                        rect.height,
                    ));
                }
                _ => {}
            }
        }
        found
    }

    /// How far the tree blurs what is under a layer, and how dark it paints a black wash, at most.
    fn veiled(tree: &telar::ComponentList) -> (f32, f32) {
        tree.commands()
            .iter()
            .fold((0.0, 0.0), |(blur, dim), command| match command {
                telar::DrawCommand::PushLayer { backdrop_blur, .. } => {
                    (blur.max(*backdrop_blur), dim)
                }
                telar::DrawCommand::Rect { style, .. } => match style.fill {
                    Some(Paint::Solid(colour))
                        if colour.r == 0.0 && colour.g == 0.0 && colour.b == 0.0 =>
                    {
                        (blur, dim.max(colour.a))
                    }
                    _ => (blur, dim),
                },
                _ => (blur, dim),
            })
    }

    /// `cover` scales a picture to the smallest size that covers the box and centres the focus in it as far as the picture reaches past the box, so the focus never leaves the box; parallax widens the box the picture covers.
    #[test]
    fn cover_centres_the_focus_as_far_as_the_picture_reaches() {
        let at = |intrinsic, focus: (f32, f32), parallax| {
            let rect = covering(
                intrinsic,
                (100.0, 100.0),
                Focus {
                    x: focus.0,
                    y: focus.1,
                },
                parallax,
            );
            (rect.x, rect.y, rect.width, rect.height)
        };
        assert_eq!(
            at((400.0, 200.0), (0.5, 0.5), 0.0),
            (-50.0, 0.0, 200.0, 100.0)
        );
        assert_eq!(
            at((400.0, 200.0), (0.0, 0.5), 0.0),
            (0.0, 0.0, 200.0, 100.0),
            "the left edge"
        );
        assert_eq!(
            at((400.0, 200.0), (1.0, 0.5), 0.0),
            (-100.0, 0.0, 200.0, 100.0),
            "the right edge"
        );
        assert_eq!(
            at((400.0, 200.0), (0.375, 0.5), 0.0),
            (-25.0, 0.0, 200.0, 100.0),
            "centred on it"
        );
        assert_eq!(
            at((100.0, 400.0), (0.5, 0.1), 0.0),
            (0.0, 0.0, 100.0, 400.0),
            "held at the top"
        );
        assert_eq!(
            at((100.0, 400.0), (0.5, 0.5), 0.0),
            (0.0, -150.0, 100.0, 400.0)
        );
        assert_eq!(
            at((100.0, 400.0), (0.5, 2.0), 0.0),
            (0.0, -300.0, 100.0, 400.0),
            "a focus past the edge is the edge"
        );
        assert_eq!(
            at((400.0, 200.0), (0.5, 0.5), 0.5),
            (-25.0, 0.0, 200.0, 100.0),
            "covering a box half as wide again"
        );
        assert_eq!(
            at((200.0, 200.0), (0.5, 0.5), 0.5),
            (0.0, -25.0, 150.0, 150.0)
        );
        assert_eq!(
            at((0.0, 0.0), (0.5, 0.5), 0.0),
            (0.0, 0.0, 100.0, 100.0),
            "nothing to scale fills the box"
        );
    }

    /// The picture slides by the share of the region's width parallax gives it, from none on the first workspace to all of it on the last.
    #[test]
    fn parallax_slides_the_picture_by_where_the_workspace_up_is() {
        assert_eq!(parallax_shift(1000.0, 0.1, 0.0), 0.0);
        assert_eq!(parallax_shift(1000.0, 0.1, 0.5), -50.0);
        assert_eq!(parallax_shift(1000.0, 0.1, 1.0), -100.0);
        assert_eq!(
            parallax_shift(1000.0, 0.1, 3.0),
            -100.0,
            "never past the last"
        );
        assert_eq!(parallax_shift(1000.0, 0.0, 1.0), 0.0, "off");
    }

    /// A region drawn with `cover` lays its picture so the focus stays in view: at the left edge for a focus there, at the right for one there.
    #[test]
    fn a_covering_region_lays_its_picture_around_its_focus() {
        reset_layout_runtime();
        let wide = wide_picture(&scratch("focus"), "wide.png", (400, 100));
        let config = unanimated(config::ReducedMotion::Off);
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();

        for (x, laid_at) in [(0.0, 0.0), (0.5, -1100.0), (1.0, -2200.0)] {
            let tree = drawn(
                build(
                    &laid_region(&wide, Focus { x, y: 0.5 }, (0.0, 0.0, 0.0)),
                    surrounded(&config),
                )
                .expect("a region builds"),
            );
            assert_eq!(
                laid_pictures(&tree),
                vec![telar::Rect::new(laid_at, 0.0, 3200.0, 800.0)],
                "a focus at {x}"
            );
            assert_eq!(
                pictures(&tree)
                    .into_iter()
                    .map(|(_, reach)| reach)
                    .collect::<Vec<_>>(),
                vec![telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1)],
                "and cut to the region"
            );
        }
    }

    /// While a window covers the screen the picture is dimmed and blurred as far as the region says, and it clears as the screen is uncovered.
    #[test]
    fn a_window_covering_the_screen_dims_and_blurs_the_picture() {
        reset_layout_runtime();
        let wide = wide_picture(&scratch("veil"), "veiled.png", (400, 200));
        let config = unanimated(config::ReducedMotion::Off);
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();
        crate::desk::show(None, Desk::default());

        let tree = drawn(
            build(
                &laid_region(&wide, Focus::MIDDLE, (0.4, 12.0, 0.0)),
                surrounded(&config),
            )
            .expect("a region builds"),
        );
        assert_eq!(veiled(&tree), (0.0, 0.0), "nothing covers the screen");

        crate::desk::show(
            None,
            Desk {
                covered: true,
                along: None,
            },
        );
        let (blur, dim) = veiled(&tree);
        assert_eq!(blur, 12.0);
        assert!((dim - 0.4).abs() < 1e-6, "dimmed by {dim}");

        crate::desk::show(None, Desk::default());
        assert_eq!(veiled(&tree), (0.0, 0.0), "uncovered again");

        let plain = drawn(
            build(
                &laid_region(&wide, Focus::MIDDLE, (0.0, 0.0, 0.0)),
                surrounded(&config),
            )
            .expect("a region builds"),
        );
        crate::desk::show(
            None,
            Desk {
                covered: true,
                along: None,
            },
        );
        assert_eq!(
            veiled(&plain),
            (0.0, 0.0),
            "a region that asks for neither keeps its picture as it is"
        );
        crate::desk::show(None, Desk::default());
    }

    /// With parallax the picture covers a box wider by that share of the region and slides across it as the workspace up moves along the screen's; reduced motion holds it still.
    #[test]
    fn parallax_slides_the_picture_with_the_workspace_up() {
        reset_layout_runtime();
        let square = wide_picture(&scratch("parallax"), "square.png", (100, 100));
        let config = unanimated(config::ReducedMotion::Off);
        set_theme(config.resolve_theme());
        let _scope = telar::owner_scope();
        crate::desk::show(
            None,
            Desk {
                covered: false,
                along: Some(0.0),
            },
        );

        let tree = drawn(
            build(
                &laid_region(&square, Focus::MIDDLE, (0.0, 0.0, 0.2)),
                surrounded(&config),
            )
            .expect("a region builds"),
        );
        let covers = |x: f32| vec![telar::Rect::new(x, -200.0, 1200.0, 1200.0)];
        assert_eq!(
            laid_pictures(&tree),
            covers(0.0),
            "the first workspace shows the left edge"
        );
        crate::desk::show(
            None,
            Desk {
                covered: false,
                along: Some(1.0),
            },
        );
        assert_eq!(laid_pictures(&tree), covers(-200.0), "the last its right");
        crate::desk::show(
            None,
            Desk {
                covered: false,
                along: Some(0.5),
            },
        );
        assert_eq!(laid_pictures(&tree), covers(-100.0));

        let reduced = unanimated(config::ReducedMotion::On);
        let still = drawn(
            build(
                &laid_region(&square, Focus::MIDDLE, (0.0, 0.0, 0.2)),
                surrounded(&reduced),
            )
            .expect("a region builds"),
        );
        assert_eq!(
            laid_pictures(&still),
            vec![telar::Rect::new(0.0, -100.0, 1000.0, 1000.0)],
            "reduced motion covers the region alone and holds it there"
        );
        crate::desk::show(None, Desk::default());
    }

    thread_local! {
        static STACKED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn stacked(name: &'static str) -> Built {
        STACKED.with(|built| built.borrow_mut().push(name));
        Ok(Box::new(Container::new(
            LayoutStyle::new().width(40.0).height(40.0),
            Vec::new(),
        )?))
    }

    fn weather(_: &Host) -> Built {
        stacked("weather")
    }

    fn calendar(_: &Host) -> Built {
        stacked("calendar")
    }

    static STACK_PROBES: &[ModuleDescriptor] = &[
        ModuleDescriptor {
            id: "weather",
            name: "weather",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: widget(weather),
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "calendar",
            name: "calendar",
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: widget(calendar),
            actions: &[],
            sources: &[],
        },
    ];

    fn last_stacked() -> Option<&'static str> {
        STACKED.with(|built| built.borrow().last().copied())
    }

    const START: GroupKind = GroupKind::Zone { zone: Zone::Start };

    fn smart_stack_in(kind: ResolvedAreaKind, place: GroupKind) -> ResolvedArea {
        let child = |module: &str| {
            let mut placed = instance(module, Placed::WidgetS);
            placed.id = PlacedId::new(module);
            placed
        };
        area(
            kind,
            vec![stacked_group(
                place,
                vec![child("weather"), child("calendar")],
            )],
        )
    }

    fn scrolled(tree: &mut telar::ComponentList, at: (f32, f32), dy: f32) {
        tree.on_event(&telar::Event::Scrolled {
            delta: telar::ScrollDelta::Pixels { x: 0.0, y: dy },
            x: f64::from(at.0),
            y: f64::from(at.1),
        });
    }

    /// A Smart Stack builds only the child on show, a wheel notch over it moves on to the next and back, a dot shows its own child, and which child was on show outlives the node: a rebuild — any layout edit — comes back on it.
    #[test]
    fn a_smart_stack_shows_one_child_cycles_and_remembers_it_across_a_rebuild() {
        for kind in [
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            ResolvedAreaKind::Free {
                rect: Rect::default(),
                anchor: Anchor::TopLeft,
            },
            ResolvedAreaKind::Dock {
                edge: Edge::Top,
                thickness: 80.0,
            },
        ] {
            reset_layout_runtime();
            SHOWN.with(|shown| shown.borrow_mut().clear());
            STACKED.with(|built| built.borrow_mut().clear());
            let config = Arc::new(Config::starter());
            set_theme(config.resolve_theme());
            ui::descriptor::install(STACK_PROBES);
            let _scope = telar::owner_scope();
            let kind_name = kind.name();
            let placed = smart_stack_in(kind, START);

            let mut tree = drawn(build(&placed, surrounded(&config)).expect("the area builds"));
            assert_eq!(
                STACKED.with(|built| built.borrow().clone()),
                ["weather"],
                "{kind_name}: only the child on show is built"
            );
            let inside = (20.0, 20.0);
            scrolled(&mut tree, inside, 30.0);
            assert_eq!(
                last_stacked(),
                Some("weather"),
                "{kind_name}: half a notch is not a step"
            );
            scrolled(&mut tree, inside, 30.0);
            assert_eq!(
                last_stacked(),
                Some("calendar"),
                "{kind_name}: a notch down is the next child"
            );
            scrolled(&mut tree, inside, 60.0);
            assert_eq!(
                last_stacked(),
                Some("weather"),
                "{kind_name}: and past the last it wraps"
            );
            scrolled(&mut tree, inside, -60.0);
            assert_eq!(
                last_stacked(),
                Some("calendar"),
                "{kind_name}: up goes back, wrapping too"
            );

            let key = (None, AreaId::new("area"), GroupId::new("run"));
            assert_eq!(shown_in(&key), Some(PlacedId::new("calendar")));
            drop(tree);
            let _again = drawn(build(&placed, surrounded(&config)).expect("the area builds again"));
            assert_eq!(
                last_stacked(),
                Some("calendar"),
                "{kind_name}: a rebuild comes back on the child it was left on"
            );
        }
    }

    /// Two dots, the one on show solid; pressing the other shows its child. The keyboard path is the stack itself: once focused, the arrows step it.
    #[test]
    fn a_smart_stacks_dots_and_keys_pick_its_child() {
        reset_layout_runtime();
        STACKED.with(|built| built.borrow_mut().clear());
        let config = Arc::new(Config::starter());
        let theme = config.resolve_theme();
        set_theme(theme);
        ui::descriptor::install(STACK_PROBES);
        let _scope = telar::owner_scope();
        let placed = smart_stack_in(
            ResolvedAreaKind::Free {
                rect: Rect::default(),
                anchor: Anchor::TopLeft,
            },
            START,
        );

        let mut tree = drawn(build(&placed, surrounded(&config)).expect("the area builds"));
        let dots: Vec<telar::Rect> = tree
            .commands()
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::Rect { rect, .. }
                    if rect.width == DOT && rect.height == DOT =>
                {
                    Some(*rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(dots.len(), 2, "one dot per child");
        assert_eq!(
            rects_filled(&tree, theme.text).len(),
            1,
            "and the one on show is solid"
        );

        let second = dots[1];
        let (x, y) = (
            f64::from(second.x + second.width / 2.0),
            f64::from(second.y + second.height / 2.0),
        );
        for event in [
            telar::Event::PointerPressed {
                x,
                y,
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
            telar::Event::PointerReleased {
                x,
                y,
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
        ] {
            tree.on_event(&event);
        }
        assert_eq!(
            last_stacked(),
            Some("calendar"),
            "a press on a dot shows its child"
        );

        let first = dots[0];
        let (x, y) = (f64::from(first.x) - 12.0, f64::from(first.y) - 12.0);
        for event in [
            telar::Event::PointerPressed {
                x,
                y,
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
            telar::Event::PointerReleased {
                x,
                y,
                button: telar::PointerButton::Primary,
                source: telar::PointerSource::Mouse,
            },
        ] {
            tree.on_event(&event);
        }
        tree.on_event(&telar::Event::KeyPressed {
            key: Key::Named(telar::NamedKey::ArrowDown),
            modifiers: Default::default(),
        });
        assert_eq!(
            last_stacked(),
            Some("weather"),
            "once a press has focused the stack, the arrow steps it on and wraps"
        );
    }

    fn registered(node: &rects::Node) -> telar::Rect {
        rects::rect(node).unwrap_or_else(|| panic!("{node:?} is in the registry"))
    }

    /// "One at a time" is a flag beside the placement, not a placement of its own: a stacked group put on a cell is on that cell, and one put in a dock's end zone is at the end.
    #[test]
    fn a_stacked_group_keeps_the_cell_and_the_zone_it_was_placed_at() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(STACK_PROBES);
        let _scope = telar::owner_scope();
        let child = |module: &str| instance(module, Placed::WidgetS);
        let two = || vec![child("weather"), child("calendar")];
        let at = rects::Node::area(None, LayerKind::Desktop, &AreaId::new("area"));

        let grid = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![stacked_group(cell(2, 1), two())],
        );
        let _grid = drawn(build(&grid, surrounded(&config)).expect("the grid builds"));
        let placed = registered(&at.group(&GroupId::new("run")));
        assert_eq!(
            (placed.x, placed.y),
            (2.0 * 96.0, 96.0),
            "on the cell it names"
        );
        let shown = registered(&at.instance(&GroupId::new("run"), &PlacedId::new("weather")));
        assert_eq!(
            (shown.x, shown.y),
            (placed.x, placed.y),
            "and so is the child on show"
        );
        drop(_grid);

        let dock = area(
            ResolvedAreaKind::Dock {
                edge: Edge::Bottom,
                thickness: 80.0,
            },
            vec![stacked_group(GroupKind::Zone { zone: Zone::End }, two())],
        );
        let _dock = drawn(build(&dock, surrounded(&config)).expect("the dock builds"));
        let shown = registered(&at.instance(&GroupId::new("run"), &PlacedId::new("weather")));
        assert_eq!(
            shown.x + shown.width,
            PAGE.0,
            "packed to the end of the strip, {shown:?}"
        );
        assert_eq!(shown.y + shown.height, PAGE.1, "on the bottom edge");
    }

    thread_local! {
        static ACTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn acted() -> Vec<String> {
        ACTED.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    fn tap(tree: &mut telar::ComponentList, at: (f64, f64)) {
        use telar::{Event, PointerButton, PointerSource};
        tree.on_event(&Event::PointerMoved {
            x: at.0,
            y: at.1,
            source: PointerSource::Mouse,
        });
        for event in [
            Event::PointerPressed {
                x: at.0,
                y: at.1,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            },
            Event::PointerReleased {
                x: at.0,
                y: at.1,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            },
        ] {
            tree.on_event(&event);
        }
    }

    /// A widget's bound press runs its own chain, and the grid's runs only on the grid's empty space.
    #[test]
    fn a_widget_answers_its_own_action_and_the_grid_its_empty_space() {
        use layout::{Action, Trigger};
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(STACK_PROBES);
        ACTED.with(|ran| ran.borrow_mut().clear());
        services::command::set_runner(
            |line| {
                ACTED.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
        let _scope = telar::owner_scope();
        let mut weather = instance("weather", Placed::WidgetS);
        weather.actions = BTreeMap::from([(Trigger::Press, Action(vec!["widget press".into()]))]);
        let mut grid = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![group(cell(0, 0), vec![weather])],
        );
        grid.actions = BTreeMap::from([(Trigger::Press, Action(vec!["grid press".into()]))]);
        let mut tree = drawn(build(&grid, surrounded(&config)).expect("the grid builds"));
        let widget = registered(
            &rects::Node::area(None, LayerKind::Desktop, &grid.id)
                .instance(&GroupId::new("run"), &PlacedId::new("weather")),
        );

        tap(
            &mut tree,
            (
                f64::from(widget.x + widget.width / 2.0),
                f64::from(widget.y + widget.height / 2.0),
            ),
        );
        assert_eq!(acted(), ["widget press"]);
        tap(&mut tree, (900.0, 700.0));
        assert_eq!(acted(), ["grid press"]);
    }

    /// The registry holds what is built and nothing longer: an area, its group and its instance are there while the node is, at the rect layout gave them, and gone with it.
    #[test]
    fn what_the_registry_holds_goes_with_the_node() {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(STACK_PROBES);
        let at = rects::Node::area(None, LayerKind::Desktop, &AreaId::new("area"));
        let grid = area(
            ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![group(
                cell(1, 0),
                vec![instance("weather", Placed::WidgetS)],
            )],
        );
        let scope = telar::owner_scope();
        let owner = scope.id();
        let tree = drawn(build(&grid, surrounded(&config)).expect("the grid builds"));
        assert_eq!(
            registered(&at),
            telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1),
            "the area is its region"
        );
        let group_at = registered(&at.group(&GroupId::new("run")));
        let weather = registered(&at.instance(&GroupId::new("run"), &PlacedId::new("weather")));
        assert_eq!((group_at.x, weather.x), (96.0, 96.0));
        assert_eq!(
            rects::instance(None, &PlacedId::new("weather")).map(|(_, rect)| rect),
            Some(weather),
            "and found by its id alone"
        );

        drop(scope);
        drop(tree);
        telar::dispose_owner(owner);
        assert_eq!(rects::rect(&at), None);
        assert_eq!(rects::rect(&at.group(&GroupId::new("run"))), None);
        assert!(rects::instance(None, &PlacedId::new("weather")).is_none());
    }

    /// F-3.4: what is kept across rebuilds goes with the area, group or instance it was kept for, and nothing else's goes with it.
    #[test]
    fn what_is_kept_across_rebuilds_goes_with_what_it_was_kept_for() {
        let key = |output: &str, area: &str, group: &str| {
            (
                Some(output.to_string()),
                AreaId::new(area),
                GroupId::new(group),
            )
        };
        let shows = |key: &StackKey, child: &str| {
            SHOWN.with(|shown| shown.borrow_mut().insert(key.clone(), PlacedId::new(child)));
        };
        let (removed_area, removed_group, removed_child, kept) = (
            key("DP-1", "grid-gone", "run"),
            key("DP-1", "grid", "gone"),
            key("DP-2", "grid", "run"),
            key("DP-2", "grid", "other"),
        );
        shows(&removed_area, "weather");
        shows(&removed_group, "weather");
        shows(&removed_child, "clock");
        shows(&kept, "weather");
        let picture = Arc::new(ImageData::new(vec![0, 0, 0, 0], 1, 1));
        let region = |area: &str| (Some("DP-1".to_string()), AreaId::new(area));
        remember(
            &region("wall-gone"),
            Path::new("/gone.png"),
            Arc::clone(&picture),
        );
        remember(&region("wall"), Path::new("/kept.png"), picture);

        forget_gone(&layout::Placed {
            areas: [AreaId::new("grid-gone"), AreaId::new("wall-gone")].into(),
            groups: [(AreaId::new("grid"), GroupId::new("gone"))].into(),
            instances: [PlacedId::new("clock")].into(),
        });

        for gone in [&removed_area, &removed_group, &removed_child] {
            assert_eq!(shown_in(gone), None, "{gone:?}");
        }
        assert_eq!(shown_in(&kept), Some(PlacedId::new("weather")));
        assert!(opened(&region("wall-gone")).is_none());
        assert!(opened(&region("wall")).is_some());
    }
}
