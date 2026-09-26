//! One area of a resolved layout, built into the node its layer draws.
//!
//! An area carries its own geometry, so a builder here takes the area and nothing else about where it was found: a [`ResolvedAreaKind::Grid`] decides how big its cells are, a [`ResolvedAreaKind::Dock`] how deep its strip is, a [`ResolvedAreaKind::WallpaperRegion`] which picture is showing and how it fades, a [`ResolvedAreaKind::Texture`] what paints over whatever is behind it. What every area shares is what surrounds it — the config an instance reads its options from, the theme it takes its colours from and the monitor it is on — and that is [`Surround`].
//!
//! This is why the instances here are built through [`Host::placed`] and not [`ui::host::Host::chip`]: a chip's box is `[bars.<edge>] size` looked up by edge, which only answers while one edge means one bar. An area answers for its own box, and several areas can share an edge.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use telar::{
    AlignItems, BlendMode, Color, Container, Gradient, Image, ImageData, ImageSlice, Insets,
    LayoutError, LayoutItem, LayoutStyle, ObjectFit, Paint, Point, Raster, RectStyle,
    SizeDimension, StyledContainer, TemplateTrack, box_item, motion::Animated, signal,
};

use crate::layer_window::{AreaContext, Areas, Reserved};
use config::theme::NordTheme;
use config::{Align, Config, Edge};
use layout::{
    Anchor, AreaId, Blend, Fit, GroupKind, Paint as AreaPaint, Rect, Representation as Placed,
    ResolvedArea, ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Tile, Transition, Zone,
};
use services::wallpaper;
use ui::descriptor::Built;
use ui::host::{Footprint, Host, InstanceId, Representation, Size, WidgetSize};
use ui::layout::{align_items, fill, justify};

/// What an area's contents are built against beyond the area itself: the [`AreaContext`] the window hands down, minus what only the host acts on.
#[derive(Clone, Copy)]
pub struct Surround<'a> {
    pub config: &'a Arc<Config>,
    pub theme: NordTheme,
    pub output: Option<&'a str>,
    /// The box this area's geometry is measured in, in the window's coordinate space, with [`layout::Within`] already applied — so a fractional [`Rect`] is a fraction of this and nothing here reads `area.within`.
    pub bounds: telar::Rect,
    /// What the output's reserving areas take off each edge. A bar running [`Extent::Fill`] is as long as the edges beside it leave it, and those edges are on layers this window cannot see.
    pub reserved: Reserved,
}

impl<'a> Surround<'a> {
    pub fn of(context: &'a AreaContext<'a>) -> Self {
        Self {
            config: context.config,
            theme: context.theme,
            output: context.output,
            bounds: context.bounds,
            reserved: context.reserved,
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

/// The node `area` draws, or `None` for a kind no builder answers for yet, so a layer draws the areas it can rather than failing whole over the one it cannot.
pub fn build(area: &ResolvedArea, surround: Surround) -> Option<Built> {
    match &area.kind {
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
        _ => None,
    }
}

/// A free area: its groups down the box `rect` names, each instance at the size its representation asks for.
///
/// It is the kind with no arrangement of its own — no cells, no zones, no edge — so it is what a layout says when the answer to "where" is simply a rectangle.
pub fn free(area: &ResolvedArea, rect: Rect, surround: Surround) -> Built {
    let groups = area
        .groups
        .iter()
        .map(|group| {
            let items = group
                .children
                .iter()
                .map(|instance| {
                    place(
                        instance,
                        Size {
                            width: f32::INFINITY,
                            height: f32::INFINITY,
                        },
                        None,
                        LayoutStyle::new(),
                        surround,
                    )
                })
                .collect::<Result<Vec<_>, LayoutError>>()?;
            Ok(
                Box::new(Container::new(LayoutStyle::new().flex_column(), items)?)
                    as Box<dyn LayoutItem>,
            )
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    Ok(Box::new(Container::new(
        region(rect, surround)
            .flex_column()
            .padding_all(inset(area)),
        groups,
    )?))
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
        .map(|group| cell_group(group, cell, gap, surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let block = Container::new(tracks(covered(area), cell, gap), placed)?;
    let (vertical, horizontal) = anchored(anchor);
    Ok(Box::new(Container::new(
        region(rect, surround)
            .flex_row()
            .padding_all(inset(area))
            .align_items(align_items(vertical))
            .justify_content(justify(horizontal)),
        vec![Box::new(block) as Box<dyn LayoutItem>],
    )?))
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
        .map(|group| run(group, edge, thickness, surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
    let strip = Container::new(along(edge, thickness), runs)?;
    let (vertical, horizontal) = hugging(edge);
    Ok(Box::new(Container::new(
        region(Rect::default(), surround)
            .flex_row()
            .padding_all(inset(area))
            .align_items(align_items(vertical))
            .justify_content(justify(horizontal)),
        vec![Box::new(strip) as Box<dyn LayoutItem>],
    )?))
}

/// Reading where the hand-over between a `WallpaperRegion`'s two image layers has got to, and moving it. `Rc` on the reading half because both layers hold one; `Box` on the writing half because only the frame consumer does.
type FadeControl = (Rc<dyn Fn() -> f32>, Box<dyn Fn(f32)>);

/// One [`ResolvedAreaKind::WallpaperRegion`] as a node confined to its `rect`: the two-slot cross-fade, the off-thread decode and the `Fade`/`Slide` transitions. Keyed by `area`'s id as well as the output, so two regions on one screen never share a decode slot or a fade timeline.
///
/// A runtime wallpaper change arrives as an event on the live surface, carrying an image [`services::wallpaper`] already decoded off the UI thread, and the two layers are simply ping-ponged — rebuilding this node for it would be useless for a cross-fade anyway, since a fresh tree has nothing left of the old image to fade *from*.
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
    // No source of its own means "whatever `[background]` is set to", which is what the wallpaper service answers — and keeps answering as the user changes it, through the watcher below.
    let initial = match source.is_empty() {
        false => Some(PathBuf::from(source)),
        true => wallpaper::current_image(surround.config, surround.output),
    };
    let first = initial.as_deref().and_then(|path| decoded(&key, path));
    if first.is_none() && initial.is_some() {
        tracing::warn!("wallpaper '{source}' could not be loaded; using the theme base colour");
    }

    let slot_a = signal(first);
    let slot_b = signal(None::<Arc<ImageData>>);
    let (read_fade, set_fade) = fade_control(surround.config, transition);
    let object_fit = to_object_fit(fit);

    let layer_a = image_layer(
        slot_a.read_only(),
        read_fade.clone(),
        0.0,
        transition,
        object_fit,
    )?;
    let layer_b = image_layer(slot_b.read_only(), read_fade, 1.0, transition, object_fit)?;

    // Which slot holds the newest image. A plain `Cell`: it only ever changes on the driver thread, from the consumer below, so a signal would buy reactivity that nothing reads.
    let showing_b = Rc::new(Cell::new(false));
    let output = surround.output.map(str::to_string);
    platform_wayland::watch(
        wallpaper::frames(output, initial),
        move |frame: Option<wallpaper::Frame>| {
            // `None` is the producer's liveness heartbeat, not a wallpaper.
            let Some(frame) = frame else { return };
            let next_is_b = !showing_b.get();
            if next_is_b {
                slot_b.set(Some(frame.image));
            } else {
                slot_a.set(Some(frame.image));
            }
            showing_b.set(next_is_b);
            set_fade(if next_is_b { 1.0 } else { 0.0 });
        },
    );

    let stack = Container::new(fill(), vec![layer_a, layer_b])?;
    let base = surround.theme.base;
    Ok(Box::new(StyledContainer::new(
        region(rect, surround),
        move |_| RectStyle::filled(base, 0.0),
        vec![box_item(stack)],
    )?))
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

/// The picture a region last opened, decoded at most once per file.
///
/// A config reload rebuilds a surface's content, and a settings form applies itself while the user is still typing — so decoding the same file again on the UI thread would put a full image decode between every burst of keystrokes and the frame that answers it. One entry per (screen, area) rather than per screen alone, since two `WallpaperRegion` or `Texture` areas on the same output must not share a slot — keyed by mtime as well as path so a picture overwritten in place still lands.
///
/// Only the *opening* image of a `WallpaperRegion` comes through here on a live channel: a wallpaper chosen at runtime arrives already decoded, off the UI thread, from the wallpaper service. A `Texture`'s image is static, so every one of its decodes comes through here.
fn decoded(key: &(Option<String>, AreaId), path: &Path) -> Option<Arc<ImageData>> {
    /// The picture a region last opened: which file, when it was written, and its pixels.
    type Opened = (PathBuf, SystemTime, Arc<ImageData>);
    thread_local! {
        static LAST: RefCell<HashMap<(Option<String>, AreaId), Opened>> = RefCell::new(HashMap::new());
    }
    let stamp = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok();
    let hit = LAST.with(|last| {
        last.borrow()
            .get(key)
            .filter(|(cached, at, _)| cached == path && stamp == Some(*at))
            .map(|(_, _, image)| Arc::clone(image))
    });
    if hit.is_some() {
        return hit;
    }
    let image = Arc::new(util::picture::decode(path)?);
    if let Some(stamp) = stamp {
        LAST.with(|last| {
            last.borrow_mut()
                .insert(key.clone(), (path.to_path_buf(), stamp, Arc::clone(&image)))
        });
    }
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

    Ok(Box::new(
        StyledContainer::new(
            region(rect, surround),
            |_| RectStyle::default(),
            vec![content],
        )?
        .with_opacity(move || opacity)
        .with_blend(move || blend_mode),
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
        .map(|stop| (stop.at, stop_color(theme, &stop.color)))
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

/// A theme token name or a hex colour — the same vocabulary a gradient stop's colour shares with `[theme.colors]`.
fn stop_color(theme: NordTheme, token_or_hex: &str) -> Color {
    Color::from_hex(token_or_hex).unwrap_or_else(|| theme.token(token_or_hex))
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

/// The box `rect` names, as the fraction of [`Surround::bounds`] that it is, taken out of flow so the areas of one layer stack over each other instead of pushing each other along.
///
/// In pixels rather than percentages, because the percentage would be of the window — the whole output — and an area written `within = "usable"` means a fraction of what the reserving areas left, which is a different box on every monitor and after every bar edit.
fn region(rect: Rect, surround: Surround) -> LayoutStyle {
    pixels(within(rect, surround.bounds))
}

/// Where `rect` lands, as a fraction of `bounds`, in the window's own coordinate space.
pub(crate) fn within(rect: Rect, bounds: telar::Rect) -> telar::Rect {
    telar::Rect::new(
        bounds.x + rect.x * bounds.width,
        bounds.y + rect.y * bounds.height,
        rect.w * bounds.width,
        rect.h * bounds.height,
    )
}

/// An absolute box at exactly `rect`, which is how every area is placed: one window is the whole output, so an area positions itself in it rather than being flowed with its neighbours.
pub(crate) fn at(rect: telar::Rect) -> LayoutStyle {
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

/// How far an area holds its contents off its own edges.
fn inset(area: &ResolvedArea) -> f32 {
    area.style.padding.unwrap_or(0.0)
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

/// One group on its cells, its instances down the box those cells make, each at the footprint its own size asks for.
fn cell_group(
    group: &ResolvedGroup,
    cell: f32,
    gap: f32,
    surround: Surround,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let children = group
        .children
        .iter()
        .map(|instance| {
            place(
                instance,
                footprint(instance).extent(cell, gap),
                None,
                LayoutStyle::new(),
                surround,
            )
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
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
    Ok(Box::new(Container::new(style, children)?))
}

/// One group's share of a dock: an equal part of the strip's length, across the whole of its thickness, its instances packed towards its own zone.
fn run(
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
    let children = group
        .children
        .iter()
        .map(|instance| place(instance, extent, Some(edge), fill(), surround))
        .collect::<Result<Vec<_>, LayoutError>>()?;
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
    Ok(Box::new(Container::new(style, children)?))
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

/// One instance under a host told the box it was given, built inside the descriptor table's error boundary so a module that fails shows its placeholder instead of taking the area down with it.
fn place(
    instance: &ResolvedInstance,
    extent: Size,
    axis: Option<Edge>,
    style: LayoutStyle,
    surround: Surround,
) -> Built {
    let host = Host::placed(
        InstanceId::new(instance.id.as_str()),
        Arc::clone(surround.config),
        representation(instance.representation),
        extent,
        axis,
        surround.config.shape_from(None, None, None, None),
        surround.theme.accent,
        surround.theme.text,
        surround.output.map(str::to_string),
    );
    ui::descriptor::place(&instance.module, &host, style)
}

/// The representation a host is built for, from the one the layout named.
fn representation(placed: Placed) -> Representation {
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
    let held = group.children.iter().map(footprint).fold(
        Footprint {
            columns: 0,
            rows: 0,
        },
        |so_far, child| Footprint {
            columns: so_far.columns.max(child.columns),
            rows: so_far.rows.saturating_add(child.rows),
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
            children,
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
            bounds: telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1),
            reserved: Reserved::default(),
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
            zone_of(&group(GroupKind::SmartStack, Vec::new())),
            Zone::Start
        );
        assert_eq!(
            zone_of(&group(GroupKind::Zone { zone: Zone::End }, Vec::new())),
            Zone::End
        );
    }
}
