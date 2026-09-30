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
    AlignItems, BlendMode, BorderRadius, Clip, ClippedItem, Color, ConsumedKeys, Container,
    Gradient, Image, ImageData, ImageSlice, Insets, Key, LayoutError, LayoutItem, LayoutStyle,
    ObjectFit, Paint, Point, Raster, ReactiveList, RectStyle, Role, RwSignal, SizeDimension,
    StyledContainer, TemplateTrack, box_item, motion::Animated, signal,
};

use crate::actions::{Bound, EmptySpace, NOTCH};
use crate::layer_window::{
    AreaContext, Areas, Building, LayerWindowContext, Reserved, WindowAreas, build_window_areas,
};
use crate::reconcile::Desktop;
use crate::rects;
use config::theme::NordTheme;
use config::{Align, Config, Edge};
use layout::{
    Anchor, AreaId, AreaStyle, Blend, Fit, GroupId, GroupKind, InstanceId as PlacedId, LayerKind,
    Paint as AreaPaint, Rect, Representation as Placed, ResolvedArea, ResolvedAreaKind,
    ResolvedGroup, ResolvedInstance, Tile, Transition, Zone,
};
use services::wallpaper;
use ui::descriptor::Built;
use ui::host::{Audience, Footprint, Host, Instance, InstanceId, Representation, Size, WidgetSize};
use ui::keynav::Move;
use ui::layout::{align_items, fill, justify, painted_chrome};

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
    let (nodes, _) = build_window_areas(
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
pub fn build(area: &ResolvedArea, surround: Surround) -> Option<Built> {
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
        ResolvedAreaKind::Free { rect } => Some(free(area, *rect, surround)),
        ResolvedAreaKind::Stack { .. } => STACK
            .with(|stack| stack.get())
            .map(|build| build(area, surround)),
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

/// `root` answering the gestures `area` binds to its empty space (TA-4's dead zones), for whichever kind of area it is the root of, and opening the area's context menu on a secondary press nothing is bound to. Nothing is bound on the lock layer, and no menu is offered there.
///
/// The menu is offered only where `painted` says the root claims the pointer already, by what it draws: a root that answered a press only for the menu's sake would take every press over its whole box from the windows under it.
pub fn empty_space(
    area: &ResolvedArea,
    surround: Surround,
    root: StyledContainer,
    painted: bool,
) -> StyledContainer {
    let menu = painted
        .then(|| {
            crate::menu::on(
                rects::Node::area(surround.output, surround.layer, &area.id),
                surround.audience,
            )
        })
        .flatten();
    Bound::of(&area.actions, surround.audience)
        .with_menu(menu)
        .on_empty_space(
            root,
            EmptySpace {
                output: surround.output.map(str::to_string),
                layer: surround.layer,
                area: area.id.clone(),
            },
        )
}

/// Whether an area [`dressed`] in `style` paints a fill, which is what makes it claim the pointer.
pub fn is_filled(style: &AreaStyle, theme: &NordTheme) -> bool {
    style.paint(theme).is_some_and(|fill| fill.a > 0.0)
}

/// A free area: its groups down the box `rect` names, each instance at the size its representation asks for.
///
/// It is the kind with no arrangement of its own — no cells, no zones, no edge — so it is what a layout says when the answer to "where" is simply a rectangle.
pub fn free(area: &ResolvedArea, rect: Rect, surround: Surround) -> Built {
    let groups = area
        .groups
        .iter()
        .map(|group| {
            arranged(
                area,
                group,
                LayoutStyle::new().flex_column(),
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
                        LayoutStyle::new(),
                        surround,
                    )
                },
            )
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    Ok(Box::new(empty_space(
        area,
        surround,
        dressed(
            &area.style,
            &surround.theme,
            region(rect, surround).flex_column(),
            groups,
        )?,
        is_filled(&area.style, &surround.theme),
    )))
}

/// A grid: every group on the cells it was placed at, `cell` px each and `gap` apart, the block they make anchored inside `rect` and held off its edges by the area's padding.
///
/// The block is sized to its cells rather than to `rect`, which is what `anchor` is for: a grid of one widget covers a corner of the region it is given, and without an anchor that corner could only ever be the one the origin is at.
pub fn grid(
    area: &ResolvedArea,
    rect: Rect,
    cell: f32,
    gap: f32,
    anchor: Anchor,
    surround: Surround,
) -> Built {
    let placed = area
        .groups
        .iter()
        .map(|group| cell_group(area, group, cell, gap, surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let block = Container::new(tracks(covered(area), cell, gap), placed)?;
    let (vertical, horizontal) = anchored(anchor);
    Ok(Box::new(empty_space(
        area,
        surround,
        dressed(
            &area.style,
            &surround.theme,
            region(rect, surround)
                .flex_row()
                .align_items(align_items(vertical))
                .justify_content(justify(horizontal)),
            vec![Box::new(block) as Box<dyn LayoutItem>],
        )?,
        is_filled(&area.style, &surround.theme),
    )))
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
        dressed(
            &area.style,
            &surround.theme,
            region(Rect::default(), surround)
                .flex_row()
                .align_items(align_items(vertical))
                .justify_content(justify(horizontal)),
            vec![Box::new(strip) as Box<dyn LayoutItem>],
        )?,
        is_filled(&area.style, &surround.theme),
    )))
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
    } = &area.kind
    else {
        unreachable!("build only routes a WallpaperRegion area here")
    };
    let (rect, fit, transition) = (*rect, *fit, *transition);

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

    let object_fit = to_object_fit(fit);
    let (read_fade, set_fade) = fade_control(surround.config, transition);
    let handover = match fades_from {
        Some(was) => {
            let handover = Handover::new(Some(was.image), set_fade);
            handover.show(arriving);
            handover
        }
        None => Handover::new(arriving, set_fade),
    };
    let layer_a = image_layer(
        handover.a.read_only(),
        read_fade.clone(),
        0.0,
        transition,
        object_fit,
    )?;
    let layer_b = image_layer(
        handover.b.read_only(),
        read_fade,
        1.0,
        transition,
        object_fit,
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

    let stack = Container::new(fill(), vec![layer_a, layer_b])?;
    let mut painted = picture(
        &area.style,
        Some(surround.theme.base),
        &surround.theme,
        region(rect, surround),
        vec![box_item(stack)],
    )?;
    if let Some(opacity) = area.style.opacity {
        painted = painted.with_opacity(move || opacity);
    }
    Ok(rounded(
        &area.style,
        empty_space(area, surround, painted, false),
    ))
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

/// How the layers are handed over: reading the current position, and moving it.
///
/// Two shapes behind one pair of closures. An animated transition drives an `Animated`, built at `0.0` and retargeted — never at its destination, which would leave it inert. `transition = "none"` (and animation switched off globally) drives a plain signal instead of an `Animated` with a zero-length tween, because a tween that has no duration to divide by is a division waiting to happen, and "no transition" should not go anywhere near the ticker.
fn fade_control(config: &Config, transition: Transition) -> FadeControl {
    let instant = transition == Transition::None
        || !config.animation.enabled
        || config.background.transition_ms == 0;
    if instant {
        let at = signal(0.0f32);
        let reading = at.read_only();
        return (
            Rc::new(move || reading.get()),
            Box::new(move |to| at.set(to)),
        );
    }
    let tween = config
        .animation
        .tween_ms(config.background.transition_ms, 10_000);
    let fade = Animated::new(0.0f32, tween);
    let reading = fade;
    (
        Rc::new(move || reading.get()),
        Box::new(move |to| fade.retarget(to)),
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

/// One image slot: filling its region, shown in proportion to how close `fade` is to `visible_at`.
///
/// The image itself is rebuilt whenever the slot's signal changes — `Image::new` takes the data as a closure, so the layer is one node for the life of the surface and swapping the picture is a signal write, not a re-layout.
fn image_layer(
    slot: telar::ReadSignal<Option<Arc<ImageData>>>,
    fade: Rc<dyn Fn() -> f32>,
    visible_at: f32,
    transition: Transition,
    fit: ObjectFit,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let data = slot;
    let image = Image::new(
        fill(),
        move || data.get().unwrap_or_else(blank),
        || Raster::Smooth,
        move || fit,
    )?;

    let present = slot;
    let opacity_fade = Rc::clone(&fade);
    let mut layer = StyledContainer::new(
        fill().absolute_fill(),
        |_| RectStyle::default(),
        vec![box_item(image)],
    )?
    .with_opacity(move || {
        // Both read before the early return: a slot that is empty this frame must still re-run when it fills.
        let at = opacity_fade();
        let filled = present.get().is_some();
        if !filled {
            return 0.0;
        }
        // `visible_at` is 0 or 1, so this is "how close the hand-over has got to me".
        1.0 - (at - visible_at).abs()
    });

    if transition == Transition::Slide {
        // A slide is the incoming layer sliding over the outgoing one, so only the layer being *left* moves — the arriving one has to end up at rest exactly where the other was. `r.width` is this layer's own laid-out width, already resolved from `region`'s percent geometry, so the distance scales with whatever this region's real size turns out to be.
        layer = layer.with_transform(move |r| {
            let distance = (fade() - visible_at).abs();
            (distance != 0.0).then_some([1.0, 0.0, 0.0, 1.0, distance * r.width, 0.0])
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
    let painted = picture(
        &area.style,
        None,
        &surround.theme,
        region(rect, surround),
        vec![content],
    )?
    .with_opacity(move || opacity)
    .with_blend(move || blend_mode);
    Ok(rounded(
        &area.style,
        empty_space(area, surround, painted, false),
    ))
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
    style: &AreaStyle,
    theme: &NordTheme,
    layout: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, LayoutError> {
    let fill = style.paint(theme);
    let radius = corners(style);
    let paint = RectStyle {
        fill: fill.map(Paint::Solid),
        radius,
        ..RectStyle::default()
    };
    Ok(painted_chrome(
        StyledContainer::new(
            layout.padding_all(style.padding.unwrap_or(0.0)),
            move |_| paint,
            children,
        )?,
        fill.unwrap_or(Color::TRANSPARENT),
    ))
}

fn corners(style: &AreaStyle) -> BorderRadius {
    style
        .radius
        .map_or(BorderRadius::zero(), BorderRadius::from)
}

/// A picture claims the pointer only where the layout binds a gesture to its empty space ([`empty_space`]): otherwise it is behind the shell, not a part of it to press.
fn picture(
    style: &AreaStyle,
    under: Option<Color>,
    theme: &NordTheme,
    layout: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, LayoutError> {
    let fill = style
        .fill
        .as_deref()
        .map(|fill| layout::color_of(fill, theme))
        .or(under);
    let paint = RectStyle {
        fill: fill.map(Paint::Solid),
        radius: corners(style),
        ..RectStyle::default()
    };
    StyledContainer::new(
        layout.padding_all(style.padding.unwrap_or(0.0)),
        move |_| paint,
        children,
    )
}

/// Only the paint is cut, so a gesture bound to the region answers over the whole of its box.
fn rounded(style: &AreaStyle, picture: StyledContainer) -> Box<dyn LayoutItem> {
    match style.radius {
        Some(radius) => Box::new(ClippedItem::new(
            Box::new(picture),
            Clip::both().rounded(radius).paint_only(),
        )),
        None => Box::new(picture),
    }
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
            let span = span_of(group);
            Footprint {
                columns: so_far.columns.max(col.saturating_add(span.columns)),
                rows: so_far.rows.max(row.saturating_add(span.rows)),
            }
        },
    )
}

/// `build` is handed the surround again because a Smart Stack builds its child later, whenever it is cycled.
fn arranged(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    style: LayoutStyle,
    surround: Surround,
    build: impl Fn(&ResolvedInstance, &rects::Node, Surround) -> Built + 'static,
) -> Built {
    let at = rects::Node::area(surround.output, surround.layer, &area.id);
    let group_id = group.id.clone();
    let instance_at = at.clone();
    let build = move |instance: &ResolvedInstance, surround: Surround| -> Built {
        let node = instance_at.instance(&group_id, &instance.id);
        let built = build(instance, &node, surround)?;
        rects::track(node, built.layout_node());
        Ok(built)
    };
    // A stack is the one child of the box its placement gives the group, so it is packed where the group would be: a dock's end zone packs it to the end instead of it stretching across the run.
    let items = match group.stacked {
        true => {
            let key = (
                surround.output.map(str::to_string),
                area.id.clone(),
                group.id.clone(),
            );
            vec![smart_stack(
                key,
                &group.children,
                LayoutStyle::new().flex_column(),
                surround,
                build,
            )?]
        }
        false => group
            .children
            .iter()
            .map(|instance| build(instance, surround))
            .collect::<Result<Vec<_>, LayoutError>>()?,
    };
    let node = Container::new(style, items)?;
    rects::track(at.group(&group.id), node.layout_node());
    Ok(Box::new(node))
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
                && !gone.instances.contains(child)
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

/// One group on its cells, its instances down the box those cells make, each at the footprint its own size asks for.
fn cell_group(
    area: &ResolvedArea,
    group: &ResolvedGroup,
    cell: f32,
    gap: f32,
    surround: Surround,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let span = span_of(group);
    let style = LayoutStyle::new().flex_column().gap(gap);
    let style = match group.kind {
        GroupKind::Cell { col, row, .. } => style
            .grid_column(line(cell_at(col)), span.columns)
            .grid_row(line(cell_at(row)), span.rows),
        _ => style
            .grid_column_span(span.columns)
            .grid_row_span(span.rows),
    };
    arranged(
        area,
        group,
        style,
        surround,
        move |instance, node, surround| {
            place(
                instance,
                node,
                footprint(instance).extent(cell, gap),
                None,
                LayoutStyle::new(),
                surround,
            )
        },
    )
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
        surround,
        move |instance, node, surround| place(instance, node, extent, Some(edge), fill(), surround),
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

/// One instance under a host told the box it was given and the options its entry sets, built inside the descriptor table's error boundary so a module that fails shows its placeholder instead of taking the area down with it.
fn place(
    instance: &ResolvedInstance,
    node: &rects::Node,
    extent: Size,
    axis: Option<Edge>,
    style: LayoutStyle,
    surround: Surround,
) -> Built {
    let placed = Instance::new(
        InstanceId::new(instance.id.as_str()),
        &instance.module,
        instance.options.clone(),
    );
    let accent = surround.theme.accent_by_name(
        surround
            .config
            .accent_name(&placed.presentation(surround.config)),
    );
    let host = Host::placed(
        placed,
        Arc::clone(surround.config),
        representation(instance.representation),
        extent,
        axis,
        surround.config.shape_from(None, None, None, None),
        accent,
        surround.theme.text,
        surround.output.map(str::to_string),
    )
    .shown_to(surround.audience);
    // The last of three lines on "readings only, never controls" (TA-8): validation refuses a representation that acts on the lock layer, `layout add` refuses to place one, and a file that was hand-edited past both is drawn as a placeholder rather than built. Placed here because this is the one point every lock instance goes through, whatever kind of area holds it.
    if surround.audience == Audience::Anyone && !reads_only(instance) {
        return ui::placeholder::neutral(surround.theme);
    }
    let bound = Bound::of(&instance.actions, surround.audience)
        .with_menu(crate::menu::on(node.clone(), surround.audience));
    if bound.is_empty() {
        return ui::descriptor::place(&instance.module, &host, style);
    }
    // Around the module's own tree rather than in it: whatever the module answers itself, a button inside a widget, stays its own, and the bound gestures and the menu answer everywhere else on it — a placeholder standing in for a module that failed included.
    let placed = ui::descriptor::place(
        &instance.module,
        &host,
        LayoutStyle::new().flex_column().flex_grow(1.0),
    )?;
    Ok(Box::new(bound.on(StyledContainer::new(
        style.flex_column(),
        |_| RectStyle::default(),
        vec![placed],
    )?)))
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

/// The cells a group covers: the span it was placed with, widened to hold its instances down the column they are laid out in.
fn span_of(group: &ResolvedGroup) -> Footprint {
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
    // A Smart Stack shows one child at a time, so it covers the largest of them; any other group is a column of all of them.
    let one_at_a_time = group.stacked;
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
        AvailableSpace, RwSignal, compute_layout, new_container, reset_layout_runtime, set_theme,
        track_layout,
    };

    use layout::{AreaId, AreaStyle, GroupId, InstanceId as PlacedId};
    use ui::descriptor::{Input, ModuleDescriptor, Representations, WidgetDef};

    fn instance(module: &str, representation: Placed) -> ResolvedInstance {
        ResolvedInstance {
            id: PlacedId::new(module),
            module: module.to_string(),
            representation,
            options: toml::Table::new(),
            bindings: BTreeMap::new(),
            actions: BTreeMap::new(),
        }
    }

    fn group(kind: GroupKind, children: Vec<ResolvedInstance>) -> ResolvedGroup {
        ResolvedGroup {
            id: GroupId::new("run"),
            kind,
            stacked: false,
            children,
        }
    }

    fn stacked_group(kind: GroupKind, children: Vec<ResolvedInstance>) -> ResolvedGroup {
        ResolvedGroup {
            stacked: true,
            ..group(kind, children)
        }
    }

    fn area(kind: ResolvedAreaKind, groups: Vec<ResolvedGroup>) -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new("area"),
            kind,
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Output,
            style: AreaStyle::default(),
            visible: None,
            actions: Default::default(),
            groups,
        }
    }

    fn cell(col: u32, row: u32) -> GroupKind {
        GroupKind::Cell {
            col,
            row,
            col_span: 1,
            row_span: 1,
        }
    }

    thread_local! {
        static LANDED: RefCell<Option<(Size, RwSignal<telar::Rect>)>> =
            const { RefCell::new(None) };
    }

    /// Publishes the box `host` was told it has and where the widget built for it ended up, which is the only way to ask an area where it put something.
    fn landed(host: &Host, item: Container) -> Built {
        let rect = track_layout(item.layout_node()).expect("a container registers its rect");
        LANDED.with(|seen| *seen.borrow_mut() = Some((host.extent, rect)));
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
            options: &[],
            representations: widget(probe),
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "filler",
            name: "filler",
            icon: "circle",
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
        let span = span_of(&group(cell(0, 0), vec![instance("clock", Placed::WidgetM)]));
        assert_eq!((span.columns, span.rows), (4, 2));
        assert_eq!(
            span.extent(80.0, 16.0),
            WidgetSize::M.extent(),
            "and the box those cells make is the footprint's own"
        );

        let wide = span_of(&group(
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

        let stacked = span_of(&group(
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
        placed.style = AreaStyle {
            fill: Some("#ff0000".to_string()),
            radius: Some(layout::Corners::each(1.0, 2.0, 3.0, 4.0)),
            opacity: Some(0.5),
            padding: Some(10.0),
            backdrop: None,
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
        placed.style = AreaStyle {
            fill: Some("surface".to_string()),
            ..AreaStyle::default()
        };
        let tree = drawn(build(&placed, surrounded(&config)).expect("a grid builds"));
        assert_eq!(
            rects_filled(&tree, theme.surface).len(),
            1,
            "a theme token paints the theme's colour, as it is"
        );
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
        let mut placed = area(
            ResolvedAreaKind::WallpaperRegion {
                rect,
                source: source.display().to_string(),
                fit: Fit::Cover,
                transition,
            },
            Vec::new(),
        );
        placed.id = AreaId::new(id);
        placed
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
            options: &[],
            representations: widget(weather),
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "calendar",
            name: "calendar",
            icon: "circle",
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
