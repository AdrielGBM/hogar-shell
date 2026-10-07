//! Textures in the background mode: what one paints — an image, tiled or cut into nine, or a gradient — edited on the texture itself and in its popover (TA-5, F-5.9, F-10.20).
//!
//! **Nine-slice.** Four handles on the texture set where its image is cut, each in the image's own pixels and whole ones, since smooth sampling reads across a cut that falls between pixels. A cut never passes the one opposite it, so what the model writes is what the renderer draws.
//!
//! **Gradient.** The gradient's axis is drawn over the texture, from where its first colour starts to where its last one ends. The handle at its end turns it, snapping to 45° while Shift is held; each stop is a handle on the axis, dragged along it and kept between its neighbours, taken off by dragging it away or with Delete, and a click on the axis adds one there. A gradient holds at most eight stops and at least two, and the popover says so when it is at either. Every one of these has a row too: the angle, where the chosen stop sits, its colour from the theme's swatches, and buttons to add and remove a stop.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Border, Children, Color, Cursor, Key, LayoutError, LayoutItem, LayoutStyle, NamedKey,
    ReactiveList, Rect, RectStyle, Role, RwSignal, StyledContainer, Transaction, effect, signal,
};

use config::theme::NordTheme;
use layout::{
    Area, AreaKind, Blend, Gradient, GradientStop, Layout, LayoutOp, Paint, ResolvedAreaKind, Tile,
};

use crate::popover::area::{chosen_as, variants};
use crate::popover::rows::{self, Range, label};
use crate::popover::{AreaDraft, Inspector, help, kind_field, kind_read};
use crate::session::{EditError, Selection};
use crate::written::Written;

use super::gesture;

/// How many colour stops the renderer holds (F-5.9).
pub const MOST_STOPS: usize = 8;
/// How few a gradient can be drawn from.
pub const FEWEST_STOPS: usize = 2;
/// What the angle snaps to while Shift is held, and what a key turns it by.
pub const SNAP_DEGREES: f32 = 45.0;
/// How far from the axis a dragged stop has to be let go to be taken off, in pixels.
const AWAY: f32 = 40.0;
/// How many places along the axis a click adds a stop at.
const TARGETS: usize = 24;
/// How big a stop's handle is across.
const STOP: f32 = 16.0;
/// How far one arrow key moves a focused stop along the axis.
const STOP_STEP: f32 = 0.01;
/// The insets a newly sliced image starts with where its size is not known yet.
const FIRST_INSET: f32 = 16.0;

/// Why a gradient kept the stops it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It holds [`MOST_STOPS`] already.
    Most,
    /// It would be left with fewer than [`FEWEST_STOPS`].
    Fewest,
}

impl Refusal {
    pub fn said(self) -> String {
        match self {
            Refusal::Most => telar::t!("editor.texture.most_stops"),
            Refusal::Fewest => telar::t!("editor.texture.fewest_stops"),
        }
    }
}

/// What a texture paints when it is first made: the theme's blue into its purple, top to bottom.
pub fn first_gradient() -> Gradient {
    Gradient {
        angle: 90.0,
        stops: vec![
            GradientStop {
                at: 0.0,
                color: "blue".to_string(),
            },
            GradientStop {
                at: 1.0,
                color: "purple".to_string(),
            },
        ],
    }
}

/// `gradient` with a stop at `at` in the colour of the stop nearest it, in its place along the axis, and where it went; refused once the gradient holds [`MOST_STOPS`].
pub fn with_stop(gradient: &Gradient, at: f32) -> Result<(Gradient, usize), Refusal> {
    if gradient.stops.len() >= MOST_STOPS {
        return Err(Refusal::Most);
    }
    let at = at.clamp(0.0, 1.0);
    let color = gradient
        .stops
        .iter()
        .min_by(|a, b| (a.at - at).abs().total_cmp(&(b.at - at).abs()))
        .map_or_else(|| "text".to_string(), |stop| stop.color.clone());
    let index = gradient
        .stops
        .iter()
        .position(|stop| stop.at > at)
        .unwrap_or(gradient.stops.len());
    let mut added = gradient.clone();
    added.stops.insert(index, GradientStop { at, color });
    Ok((added, index))
}

/// `gradient` without its stop `index`; refused where that would leave fewer than [`FEWEST_STOPS`].
pub fn without_stop(gradient: &Gradient, index: usize) -> Result<Gradient, Refusal> {
    if gradient.stops.len() <= FEWEST_STOPS {
        return Err(Refusal::Fewest);
    }
    let mut taken = gradient.clone();
    if index < taken.stops.len() {
        taken.stops.remove(index);
    }
    Ok(taken)
}

/// `gradient` with its stop `index` moved to `at`, kept between the stops either side of it so a drag never reorders them.
pub fn moved_stop(gradient: &Gradient, index: usize, at: f32) -> Gradient {
    let mut moved = gradient.clone();
    let low = index
        .checked_sub(1)
        .and_then(|before| gradient.stops.get(before))
        .map_or(0.0, |stop| stop.at);
    let high = gradient.stops.get(index + 1).map_or(1.0, |stop| stop.at);
    if let Some(stop) = moved.stops.get_mut(index) {
        stop.at = at.clamp(low, high.max(low));
    }
    moved
}

/// `angle` on the nearest multiple of [`SNAP_DEGREES`], in `0..360`.
pub fn snapped(angle: f32) -> f32 {
    ((angle / SNAP_DEGREES).round() * SNAP_DEGREES).rem_euclid(360.0)
}

/// `angle` snapped and then turned by `by`, in `0..360`.
pub fn turned(angle: f32, by: f32) -> f32 {
    (snapped(angle) + by).rem_euclid(360.0)
}

/// The angle a pointer at `point` asks for, seen from `centre`: degrees clockwise from a left-to-right sweep, as the model measures it, and on a multiple of [`SNAP_DEGREES`] while `snap`.
pub fn angle_at(centre: (f32, f32), point: (f32, f32), snap: bool) -> f32 {
    let angle = (point.1 - centre.1)
        .atan2(point.0 - centre.0)
        .to_degrees()
        .rem_euclid(360.0);
    match snap {
        true => snapped(angle),
        false => angle,
    }
}

/// The line a gradient is laid along over `rect`: through its centre at `angle`, as long as the box reaches that way — how the renderer sizes it, the way CSS sizes an angled `linear-gradient`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub start: (f32, f32),
    pub end: (f32, f32),
}

impl Axis {
    pub fn of(rect: Rect, angle: f32) -> Self {
        let theta = angle.to_radians();
        let (dx, dy) = (theta.cos(), theta.sin());
        let (half_w, half_h) = (rect.width / 2.0, rect.height / 2.0);
        let reach = half_w * dx.abs() + half_h * dy.abs();
        let centre = (rect.x + half_w, rect.y + half_h);
        Self {
            start: (centre.0 - dx * reach, centre.1 - dy * reach),
            end: (centre.0 + dx * reach, centre.1 + dy * reach),
        }
    }

    pub fn centre(&self) -> (f32, f32) {
        (
            (self.start.0 + self.end.0) / 2.0,
            (self.start.1 + self.end.1) / 2.0,
        )
    }

    /// The point `at` of the way along it.
    pub fn point(&self, at: f32) -> (f32, f32) {
        (
            self.start.0 + (self.end.0 - self.start.0) * at,
            self.start.1 + (self.end.1 - self.start.1) * at,
        )
    }

    /// How far along it `point` is, from 0 at the start to 1 at the end, and how far off it.
    pub fn project(&self, point: (f32, f32)) -> (f32, f32) {
        let (dx, dy) = (self.end.0 - self.start.0, self.end.1 - self.start.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length <= f32::EPSILON {
            return (0.0, 0.0);
        }
        let (px, py) = (point.0 - self.start.0, point.1 - self.start.1);
        let along = (px * dx + py * dy) / (length * length);
        let off = (px * dy - py * dx).abs() / length;
        (along.clamp(0.0, 1.0), off)
    }
}

/// A nine-slice cut at `value` image pixels in from its edge, whole, and never past the cut `opposite` it across an image `extent` pixels that way where its size is known.
pub fn clamp_inset(value: f32, opposite: f32, extent: Option<u32>) -> f32 {
    let most = extent.map_or(f32::INFINITY, |extent| (extent as f32 - opposite).max(0.0));
    value.round().clamp(0.0, most.floor())
}

/// The four cuts of a nine-slice, top, right, bottom, left, each kept within the image and short of the one opposite it.
pub fn clamp_insets(insets: [f32; 4], size: Option<(u32, u32)>) -> [f32; 4] {
    let (width, height) = (size.map(|size| size.0), size.map(|size| size.1));
    let top = clamp_inset(insets[0], 0.0, height);
    let right = clamp_inset(insets[1], 0.0, width);
    [
        top,
        right,
        clamp_inset(insets[2], top, height),
        clamp_inset(insets[3], right, width),
    ]
}

/// Where a newly sliced image is cut: a quarter of its short side in from each edge, or [`FIRST_INSET`] where its size is not known.
pub fn first_insets(size: Option<(u32, u32)>) -> [f32; 4] {
    let inset = size.map_or(FIRST_INSET, |(width, height)| {
        (width.min(height) as f32 / 4.0).floor()
    });
    [inset; 4]
}

fn insets_of(tile: Tile) -> Option<[f32; 4]> {
    match tile {
        Tile::NineSlice {
            top,
            right,
            bottom,
            left,
        } => Some([top, right, bottom, left]),
        Tile::None | Tile::Repeat => None,
    }
}

fn sliced(insets: [f32; 4]) -> Tile {
    Tile::NineSlice {
        top: insets[0],
        right: insets[1],
        bottom: insets[2],
        left: insets[3],
    }
}

/// How the tile picker spells a tile.
fn tile_name(tile: Tile) -> &'static str {
    match tile {
        Tile::None => "none",
        Tile::Repeat => "repeat",
        Tile::NineSlice { .. } => "nine_slice",
    }
}

const PAINTS: &[&str] = &["image", "gradient"];
const TILES: &[&str] = &["none", "repeat", "nine_slice"];

/// Writes what `paint` is into a texture as the layout file writes it: one of an image and a gradient, the other taken away.
fn write_paint(area: &mut Area, paint: &Paint) {
    if let Some(AreaKind::Texture {
        image, gradient, ..
    }) = AreaDraft::kind_mut(area, "texture")
    {
        match paint {
            Paint::Image(path) => {
                *image = Some(path.trim().to_string());
                *gradient = None;
            }
            Paint::Gradient(painted) => {
                *gradient = Some(painted.clone());
                *image = None;
            }
        }
    }
}

fn write_tile(area: &mut Area, tile: &Tile) {
    kind_field!(area, "texture", Texture { tile }, *tile)
}

/// Keeps `to` equal to what `from` makes of `source`, one way.
fn follow<S: Clone + PartialEq + 'static, T: Clone + PartialEq + 'static>(
    source: RwSignal<S>,
    to: RwSignal<T>,
    from: impl Fn(&S) -> T + 'static,
) {
    effect(move || {
        let next = from(&source.get());
        if to.peek_with(|now| *now != next) {
            to.set(next);
        }
    });
}

/// A texture's popover: what it paints and how, with handles on the texture for the slice and the gradient.
pub(crate) fn tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::Texture {
        rect,
        paint,
        tile,
        blend,
        opacity,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let size = surfaces::area::picture_size(draft.node.output.as_deref(), &draft.node.area);
    let painted = draft.setting(
        "texture.paint",
        "image",
        {
            let paint = paint.clone();
            kind_read!(Texture { paint }, paint)
        },
        write_paint,
    );
    let tiled = draft.setting(
        "texture.tile",
        "tile",
        kind_read!(Texture { tile }, tile),
        write_tile,
    );

    let (image_seed, gradient_seed) = match &paint {
        Paint::Image(path) => (path.clone(), first_gradient()),
        Paint::Gradient(gradient) => (String::new(), gradient.clone()),
    };
    let kind = signal(
        match paint {
            Paint::Image(_) => "image",
            Paint::Gradient(_) => "gradient",
        }
        .to_string(),
    );
    let image = signal(image_seed);
    let gradient = signal(gradient_seed);
    effect(move || {
        let next = match kind.get().as_str() {
            "image" => Paint::Image(image.get()),
            _ => Paint::Gradient(gradient.get()),
        };
        if painted.peek_with(|now| *now != next) {
            painted.set(next);
        }
    });
    effect(move || {
        let now = painted.get();
        telar::batch(|| match now {
            Paint::Image(path) => {
                if kind.peek() != "image" {
                    kind.set("image".to_string());
                }
                if image.peek_with(|held| *held != path) {
                    image.set(path);
                }
            }
            Paint::Gradient(wanted) => {
                if kind.peek() != "gradient" {
                    kind.set("gradient".to_string());
                }
                if gradient.peek_with(|held| *held != wanted) {
                    gradient.set(wanted);
                }
            }
        });
    });

    let slice = signal(insets_of(tile).unwrap_or_else(|| first_insets(size)));
    let tile_named = signal(tile_name(tile).to_string());
    effect(move || {
        let wanted = clamp_insets(slice.get(), size);
        if slice.peek() != wanted {
            slice.set(wanted);
        }
        let next = match tile_named.get().as_str() {
            "repeat" => Tile::Repeat,
            "nine_slice" => sliced(wanted),
            _ => Tile::None,
        };
        if tiled.peek() != next {
            tiled.set(next);
        }
    });
    effect(move || {
        let now = tiled.get();
        telar::batch(|| {
            if tile_named.peek() != tile_name(now) {
                tile_named.set(tile_name(now).to_string());
            }
            if let Some(insets) = insets_of(now)
                && slice.peek() != insets
            {
                slice.set(insets);
            }
        });
    });
    let insets: [RwSignal<f32>; 4] = std::array::from_fn(|side| signal(slice.peek()[side]));
    for (side, inset) in insets.into_iter().enumerate() {
        follow(inset, slice, move |value| {
            let mut next = slice.peek();
            next[side] = *value;
            next
        });
        follow(slice, inset, move |all| all[side]);
    }

    let angle = signal(gradient.peek().angle);
    follow(angle, gradient, move |value| Gradient {
        angle: *value,
        ..gradient.peek()
    });
    follow(gradient, angle, |gradient| gradient.angle);
    let chosen = signal(0usize);
    let stop_at = signal(0.0f32);
    let stop_colour = signal(String::new());
    effect(move || {
        let held = gradient.get();
        let at = chosen.get().min(held.stops.len().saturating_sub(1));
        if let Some(stop) = held.stops.get(at) {
            if stop_at.peek() != stop.at {
                stop_at.set(stop.at);
            }
            if stop_colour.peek_with(|now| *now != stop.color) {
                stop_colour.set(stop.color.clone());
            }
        }
    });
    effect(move || {
        let at = stop_at.get();
        let index = chosen.peek();
        let now = gradient.peek();
        if now.stops.get(index).is_some_and(|stop| stop.at != at) {
            gradient.set(moved_stop(&now, index, at));
        }
    });
    effect(move || {
        let colour = stop_colour.get();
        let index = chosen.peek();
        let mut now = gradient.peek();
        if let Some(stop) = now.stops.get_mut(index)
            && stop.color != colour
            && !colour.is_empty()
        {
            stop.color = colour;
            gradient.set(now);
        }
    });

    let parts = Parts {
        draft: draft.clone(),
        kind,
        image,
        tile_named,
        insets,
        size,
        gradient,
        angle,
        chosen,
        stop_at,
        stop_colour,
    };
    let paint_row = rows::together(vec![
        rows::choice(
            label!("editor.texture.paint"),
            help("AreaKind::Texture", "image"),
            kind,
            Rc::from(PAINTS),
        )?,
        parts.painting_rows()?,
    ])?;
    let mut list = vec![draft.marked(&["image", "gradient"], paint_row)?];
    list.push(parts.tiling_rows()?);
    list.extend(chosen_as(
        draft,
        ("texture.blend", "blend"),
        label!("editor.area.blend"),
        help("AreaKind::Texture", "blend"),
        variants("Blend"),
        kind_read!(Texture { blend }, blend),
        |area, blend: Blend| kind_field!(area, "texture", Texture { blend }, blend),
    )?);
    let strength = draft.setting(
        "texture.opacity",
        "opacity",
        kind_read!(Texture { opacity }, opacity),
        |area, value: &f32| kind_field!(area, "texture", Texture { opacity }, *value),
    );
    list.push(draft.marked(
        &["opacity"],
        rows::number(
            label!("editor.texture.opacity"),
            help("AreaKind::Texture", "opacity"),
            strength,
            Range::new(0.0, 1.0, 0.05),
        )?,
    )?);
    list.extend(crate::popover::area::rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: vec![parts.handles()?],
    })
}

/// What a texture's popover edits, shared by its rows and its handles.
#[derive(Clone)]
struct Parts {
    draft: AreaDraft,
    kind: RwSignal<String>,
    image: RwSignal<String>,
    tile_named: RwSignal<String>,
    insets: [RwSignal<f32>; 4],
    size: Option<(u32, u32)>,
    gradient: RwSignal<Gradient>,
    angle: RwSignal<f32>,
    chosen: RwSignal<usize>,
    stop_at: RwSignal<f32>,
    stop_colour: RwSignal<String>,
}

impl Parts {
    /// The rows for what the texture paints now: an image's path and tiling, or the gradient's.
    fn painting_rows(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let parts = self.clone();
        let kind = self.kind;
        Ok(Box::new(ReactiveList::with_style(
            LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
            move || vec![kind.get()],
            |kind: &String| kind.clone(),
            move |kind: String| {
                let rows = match kind.as_str() {
                    "image" => parts.image_rows()?,
                    _ => parts.gradient_rows()?,
                };
                Ok(Box::new(telar::Container::new(
                    LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
                    rows,
                )?) as Box<dyn LayoutItem>)
            },
        )?))
    }

    fn image_rows(&self) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
        Ok(vec![rows::text(
            label!("editor.area.image"),
            help("AreaKind::Texture", "image"),
            self.image,
        )?])
    }

    /// How an image is tiled, and for a nine-slice its four cuts, where the paint is an image.
    fn tiling_rows(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let parts = self.clone();
        let kind = self.kind;
        Ok(Box::new(ReactiveList::with_style(
            LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
            move || match kind.get().as_str() {
                "image" => vec![()],
                _ => Vec::new(),
            },
            |_: &()| (),
            move |_: ()| {
                let tile = rows::together(vec![
                    rows::choice(
                        label!("editor.area.tile"),
                        help("AreaKind::Texture", "tile"),
                        parts.tile_named,
                        Rc::from(TILES),
                    )?,
                    parts.slice_rows()?,
                ])?;
                parts.draft.marked(&["tile"], tile)
            },
        )?))
    }

    fn slice_rows(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let (insets, size, tile_named) = (self.insets, self.size, self.tile_named);
        Ok(Box::new(ReactiveList::with_style(
            LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
            move || match tile_named.get().as_str() {
                "nine_slice" => vec![()],
                _ => Vec::new(),
            },
            |_: &()| (),
            move |_: ()| {
                let labels = [
                    label!("editor.texture.inset_top"),
                    label!("editor.texture.inset_right"),
                    label!("editor.texture.inset_bottom"),
                    label!("editor.texture.inset_left"),
                ];
                let extents = [
                    size.map(|size| size.1),
                    size.map(|size| size.0),
                    size.map(|size| size.1),
                    size.map(|size| size.0),
                ];
                let slice = insets
                    .into_iter()
                    .zip(labels)
                    .zip(extents)
                    .map(|((inset, label), extent)| {
                        rows::number(
                            label,
                            help("Tile::NineSlice", "top"),
                            inset,
                            Range::whole(0.0, extent.map_or(4096.0, |extent| extent as f32)),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Box::new(telar::Container::new(
                    LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
                    slice,
                )?) as Box<dyn LayoutItem>)
            },
        )?))
    }

    fn gradient_rows(&self) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
        let (gradient, chosen) = (self.gradient, self.chosen);
        Ok(vec![
            rows::number(
                label!("editor.area.angle"),
                help("Gradient", "angle"),
                self.angle,
                Range::whole(0.0, 360.0),
            )?,
            rows::number(
                label!("editor.texture.stop_at"),
                help("GradientStop", "at"),
                self.stop_at,
                Range::new(0.0, 1.0, STOP_STEP),
            )?,
            rows::colour(
                label!("editor.texture.stop_colour"),
                help("GradientStop", "color"),
                self.stop_colour,
                Rc::from(config::theme::PAINT_TOKENS),
                Rc::new(ui::form::swatch_row::is_colour),
            )?,
            rows::action(
                || telar::t!("editor.texture.add_stop"),
                move || {
                    let now = gradient.peek();
                    let at = now.stops.get(chosen.peek()).map_or(0.5, |stop| {
                        let next = now.stops.get(chosen.peek() + 1).map_or(1.0, |next| next.at);
                        (stop.at + next) / 2.0
                    });
                    if let Ok((added, index)) = with_stop(&now, at) {
                        gradient.set(added);
                        chosen.set(index);
                    }
                },
            )?,
            rows::action(
                || telar::t!("editor.texture.remove_stop"),
                move || remove_stop(gradient, chosen, chosen.peek()),
            )?,
            rows::note(move || {
                let count = gradient.with(|gradient| gradient.stops.len());
                match count {
                    count if count >= MOST_STOPS => Refusal::Most.said(),
                    count if count <= FEWEST_STOPS => Refusal::Fewest.said(),
                    _ => String::new(),
                }
            })?,
        ])
    }

    /// The handles over the texture for what it paints now: the four cuts of a sliced image, or the gradient's axis, turning handle and stops.
    fn handles(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let parts = self.clone();
        let (kind, tile_named) = (self.kind, self.tile_named);
        crate::host::see_through(ReactiveList::with_style(
            crate::host::whole(),
            move || {
                let kind = kind.get();
                let sliced = tile_named.get() == "nine_slice";
                vec![(kind, sliced)]
            },
            |shown: &(String, bool)| shown.clone(),
            move |(kind, sliced): (String, bool)| {
                let handles = match (kind.as_str(), sliced) {
                    ("image", true) => parts.slice_handles()?,
                    ("image", false) => Vec::new(),
                    _ => parts.gradient_handles()?,
                };
                Ok(
                    Box::new(crate::host::passthrough(crate::host::whole(), handles)?)
                        as Box<dyn LayoutItem>,
                )
            },
        )?)
    }

    fn texture_rect(&self) -> Rect {
        self.draft.rect().unwrap_or_default()
    }

    fn slice_handles(&self) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
        let size = self.size;
        let most = |extent: Option<u32>| extent.map_or(4096.0, |extent| extent as f32);
        let sides: [(usize, f32, Cursor); 4] = [
            (0, most(size.map(|size| size.1)), Cursor::NsResize),
            (1, most(size.map(|size| size.0)), Cursor::EwResize),
            (2, most(size.map(|size| size.1)), Cursor::NsResize),
            (3, most(size.map(|size| size.0)), Cursor::EwResize),
        ];
        sides
            .into_iter()
            .map(|(side, max, cursor)| {
                let (reading, placing) = (self.draft.clone(), self.draft.clone());
                let grip = self.draft.grip();
                telar::handle(
                    telar::HandleProps::props()
                        .value(self.insets[side])
                        .to_value(Rc::new(move |x: f32, y: f32| {
                            let rect = reading.rect().unwrap_or_default();
                            match side {
                                0 => y - rect.y,
                                1 => rect.x + rect.width - x,
                                2 => rect.y + rect.height - y,
                                _ => x - rect.x,
                            }
                        }))
                        .to_point(Rc::new(move |inset: f32| {
                            let rect = placing.rect().unwrap_or_default();
                            let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
                            match side {
                                0 => (cx, rect.y + inset),
                                1 => (rect.x + rect.width - inset, cy),
                                2 => (cx, rect.y + rect.height - inset),
                                _ => (rect.x + inset, cy),
                            }
                        }))
                        .min(0.0)
                        .max(max)
                        .step(1.0)
                        .cursor(cursor)
                        .on_start(gesture::holding(grip.clone()))
                        .on_end(gesture::letting_go(grip))
                        .build(),
                    Children::default(),
                )
            })
            .collect()
    }

    fn gradient_handles(&self) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
        let mut handles: Vec<Box<dyn LayoutItem>> = (0..TARGETS)
            .map(|at| self.axis_target(at))
            .collect::<Result<_, _>>()?;
        handles.push(self.angle_handle()?);
        let parts = self.clone();
        let gradient = self.gradient;
        handles.push(crate::host::see_through(ReactiveList::with_style(
            crate::host::whole(),
            move || {
                let count = gradient.with(|gradient| gradient.stops.len());
                (0..count).map(|index| (index, count)).collect()
            },
            |shown: &(usize, usize)| *shown,
            move |(index, _): (usize, usize)| parts.stop_handle(index),
        )?)?);
        Ok(handles)
    }

    fn axis(&self) -> Axis {
        Axis::of(
            self.texture_rect(),
            self.gradient.with(|gradient| gradient.angle),
        )
    }

    /// One of the places along the axis a click adds a stop at, drawn as a dot of the dotted axis line.
    fn axis_target(&self, at: usize) -> Result<Box<dyn LayoutItem>, LayoutError> {
        const SIDE: f32 = 18.0;
        let theme = telar::use_theme::<NordTheme>();
        let t = (at as f32 + 0.5) / TARGETS as f32;
        let parts = self.clone();
        let placed = self.clone();
        let pointer = Rc::new(Cell::new((SIDE / 2.0, SIDE / 2.0)));
        let seen = Rc::clone(&pointer);
        let dot = StyledContainer::new(
            LayoutStyle::new()
                .absolute()
                .inset_start((SIDE - 4.0) / 2.0)
                .inset_top((SIDE - 4.0) / 2.0)
                .width(4.0)
                .height(4.0),
            move |_| RectStyle::filled(theme.text.with_alpha(0.8), 2.0),
            Vec::new(),
        )?;
        let target = StyledContainer::new(
            LayoutStyle::new()
                .absolute()
                .inset_start(0.0)
                .inset_top(0.0)
                .width(SIDE)
                .height(SIDE),
            |_| RectStyle::default(),
            vec![Box::new(dot)],
        )?;
        Ok(Box::new(
            crate::host::centred(target, SIDE, move || Some(placed.axis().point(t)))
                .cursor(Cursor::Crosshair)
                .input_opaque()
                .on_pointer_move(move |x, y| seen.set((x, y)))
                .on_press(move || {
                    let axis = parts.axis();
                    let (centre_x, centre_y) = axis.point(t);
                    let (x, y) = pointer.get();
                    let (along, _) =
                        axis.project((centre_x - SIDE / 2.0 + x, centre_y - SIDE / 2.0 + y));
                    let now = parts.gradient.peek();
                    if let Ok((added, index)) = with_stop(&now, along) {
                        parts.gradient.set(added);
                        parts.chosen.set(index);
                    }
                }),
        ))
    }

    /// The handle at the end of the axis that turns it, snapping to 45° while Shift is held.
    fn angle_handle(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let (reading, placing) = (self.clone(), self.clone());
        let grip = self.draft.grip();
        telar::handle(
            telar::HandleProps::props()
                .value(self.angle)
                .to_value(Rc::new(move |x: f32, y: f32| {
                    let centre = reading.axis().centre();
                    angle_at(centre, (x, y), telar::modifiers().is_shift)
                }))
                .to_point(Rc::new(move |angle: f32| {
                    Axis::of(placing.texture_rect(), angle).end
                }))
                .min(0.0)
                .max(360.0)
                .step(1.0)
                .cursor(Cursor::Grab)
                .on_start(gesture::holding(grip.clone()))
                .on_end(gesture::letting_go(grip))
                .build(),
            Children::default(),
        )
    }

    /// Stop `index`: dragged along the axis between its neighbours, let go far off it to take it away; the arrows move it and Delete takes it away while it has the focus.
    fn stop_handle(&self, index: usize) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let theme = telar::use_theme::<NordTheme>();
        let (gradient, chosen) = (self.gradient, self.chosen);
        let leaving = signal(false);
        let placed = self.clone();
        let where_now = move || {
            let at =
                gradient.with(|gradient| gradient.stops.get(index).map_or(0.0, |stop| stop.at));
            placed.axis().point(at)
        };
        let painted = where_now.clone();
        let dragged = self.clone();
        let grip = self.draft.grip();
        let holding = grip.clone();
        let transaction = Transaction::new(gradient);
        let face = StyledContainer::new(
            LayoutStyle::new()
                .absolute()
                .inset_start(0.0)
                .inset_top(0.0)
                .width(STOP)
                .height(STOP),
            move |_| {
                let colour = gradient.with(|held| {
                    held.stops
                        .get(index)
                        .map(|stop| layout::color_of(&stop.color, &theme))
                        .unwrap_or(Color::TRANSPARENT)
                });
                let ring = match chosen.get() == index {
                    true => theme.accent,
                    false => Color::WHITE,
                };
                let fill = match leaving.get() {
                    true => colour.with_alpha(0.3),
                    false => colour,
                };
                RectStyle::filled(fill, STOP / 2.0).with_border(Border::uniform(ring, 2.0))
            },
            Vec::new(),
        )?;
        let handle = crate::host::centred(face, STOP, move || Some(painted()))
            .control(Role::Slider)
            .cursor(Cursor::Grab)
            .on_press(move || chosen.set(index))
            .on_focused_key(move |key: &Key| -> bool {
                let by = match key {
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => STOP_STEP,
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => -STOP_STEP,
                    Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                        remove_stop(gradient, chosen, index);
                        return true;
                    }
                    _ => return false,
                };
                let now = gradient.peek();
                let at = now.stops.get(index).map_or(0.0, |stop| stop.at);
                let step = by * telar::step_factor(telar::modifiers());
                gradient.set(moved_stop(&now, index, at + step));
                chosen.set(index);
                true
            });
        Ok(Box::new(gesture::drag(
            handle,
            transaction,
            move |_| {
                holding.hold();
                let (x, y) = where_now();
                Some((x - STOP / 2.0, y - STOP / 2.0))
            },
            move |origin: &(f32, f32), (x, y)| {
                if chosen.peek() != index {
                    chosen.set(index);
                }
                let (along, off) = dragged.axis().project((origin.0 + x, origin.1 + y));
                let away = off > AWAY && gradient.with(|held| held.stops.len() > FEWEST_STOPS);
                if leaving.peek() != away {
                    leaving.set(away);
                }
                let moved = moved_stop(&gradient.peek(), index, along);
                if gradient.peek_with(|now| *now != moved) {
                    let _ = transaction.preview(|now| *now = moved);
                }
            },
            move |_, let_go| {
                match let_go {
                    true => grip.release(),
                    false => grip.put_back(),
                }
                if leaving.peek() {
                    leaving.set(false);
                    if let_go {
                        remove_stop(gradient, chosen, index);
                    }
                }
            },
        )))
    }
}

/// Takes stop `index` off the gradient, where it has more than [`FEWEST_STOPS`], and chooses the one before it.
fn remove_stop(gradient: RwSignal<Gradient>, chosen: RwSignal<usize>, index: usize) {
    if let Ok(taken) = without_stop(&gradient.peek(), index) {
        chosen.set(index.saturating_sub(1).min(taken.stops.len() - 1));
        gradient.set(taken);
    }
}

/// The texture the selection is, as the screen shows it, and where the layout writes it.
fn target(
    selection: &Selection,
    layout: &Layout,
) -> Result<(ResolvedAreaKind, Written), EditError> {
    let node = selection.node().ok_or_else(EditError::nothing)?;
    let area = surfaces::reconcile::desktop_now(node.output.as_deref())
        .and_then(|desktop| desktop.resolved.area(node.layer, &node.area).cloned())
        .ok_or_else(|| EditError::gone(&node.area))?;
    let written = Written::area(
        layout,
        node.output.as_deref(),
        node.layer,
        &node.area,
        crate::variant::editing().as_ref(),
    )
    .map_err(EditError::Refused)?;
    Ok((area.kind, written))
}

/// A key press slicing the selected texture's image into nine, or no longer slicing it.
pub(crate) fn nine_slice(
    selection: &Selection,
    layout: &Layout,
) -> Result<Vec<LayoutOp>, EditError> {
    let (kind, written) = target(selection, layout)?;
    let ResolvedAreaKind::Texture { paint, tile, .. } = kind else {
        return Err(EditError::nothing());
    };
    let node = selection.node().expect("a target has a node");
    if matches!(paint, Paint::Gradient(_)) {
        return Err(EditError::Refused(util::message!(
            "editor.texture.no_image",
            name = node.area.to_string()
        )));
    }
    let next = match tile {
        Tile::NineSlice { .. } => Tile::None,
        Tile::None | Tile::Repeat => sliced(first_insets(surfaces::area::picture_size(
            node.output.as_deref(),
            &node.area,
        ))),
    };
    let mut area = written.area.clone();
    write_tile(&mut area, &next);
    Ok(written.ops(&area))
}

/// A key press turning the selected texture's gradient 45° clockwise.
pub(crate) fn turn(selection: &Selection, layout: &Layout) -> Result<Vec<LayoutOp>, EditError> {
    turned_by(selection, layout, SNAP_DEGREES)
}

/// A key press turning it 45° the other way.
pub(crate) fn turn_back(
    selection: &Selection,
    layout: &Layout,
) -> Result<Vec<LayoutOp>, EditError> {
    turned_by(selection, layout, -SNAP_DEGREES)
}

fn turned_by(selection: &Selection, layout: &Layout, by: f32) -> Result<Vec<LayoutOp>, EditError> {
    let (kind, written) = target(selection, layout)?;
    let ResolvedAreaKind::Texture {
        paint: Paint::Gradient(gradient),
        ..
    } = kind
    else {
        return Err(EditError::nothing());
    };
    let mut area = written.area.clone();
    write_paint(
        &mut area,
        &Paint::Gradient(Gradient {
            angle: turned(gradient.angle, by),
            ..gradient
        }),
    );
    Ok(written.ops(&area))
}

/// A texture over the region `region` covers, as the layout file writes a new one: the first gradient at half strength, on the region's own rectangle and in its box.
pub(crate) fn over(region: &layout::ResolvedArea, id: layout::AreaId) -> Option<Area> {
    let ResolvedAreaKind::WallpaperRegion { rect, .. } = region.kind else {
        return None;
    };
    Some(Area {
        id,
        kind: Some(AreaKind::Texture {
            rect: Some(rect),
            image: None,
            gradient: Some(first_gradient()),
            tile: None,
            blend: None,
            opacity: Some(0.5),
        }),
        within: (region.within != layout::Within::Output).then_some(region.within),
        ..Area::default()
    })
}
