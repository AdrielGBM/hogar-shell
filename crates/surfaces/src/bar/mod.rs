use std::rc::Rc;
use std::sync::Arc;

use telar::{
    AlignItems, BorderRadius, ChildSlot, Clip, ClippedItem, Color, Container, JustifyContent,
    LayoutError, LayoutItem, LayoutStyle, RectStyle, RwSignal, Slots, StyledContainer,
    box_transform, motion::Animated, track_layout,
};

use crate::actions::{Bound, Wheel};
use crate::area::Surround;
use crate::expressions::{Expressions, Overlay};
use crate::layer_window::{LayerWindowContext, Reserved};
use crate::rects;
use crate::transient::chips::Site;
use crate::transient::standoff;
use config::theme::NordTheme;
use config::{Config, Edge, ResolvedShape, Shape, Variant};
use layout::{
    AreaStyle, AutoHide, BarShape, Corners, Extent, GroupId, GroupKind, LayerKind, ResolvedArea,
    ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Zone,
};
use ui::descriptor::{Built, ChipDef, ChipFrame, ModuleDescriptor};
use ui::host::{Audience, Host, Instance, InstanceId, Representation, Size};
use ui::layout::{fill, painted_chrome};
use ui::module::{DragOpen, module_foreground, resting_fill};
use ui::module_shell::{ModuleShellProps, module_shell};
use ui::placeholder::placeholder;

/// Builds the content tree for `area`, branching on its resolved `mode` (bar/sections/chips); visual properties come from gap/spacing/radius, not mode.
///
/// Everything that says where the bar's parts go comes off the area and off what surrounds it on the output: the edge it hangs off, how thick it is, how far it runs, its shape and the zones it holds. `surround.config` stays for what is behaviour rather than arrangement — `[popouts] enabled`, `[panels] drag_threshold`, `[theme] opacity` and `[shape] frame` — and for the `[modules.<id>]` defaults each instance's own options are laid over.
///
/// The node places itself. One window is the whole output, so the bar is an absolute box at the strip it occupies rather than a child that fills its parent: without that the three zones would divide the screen instead of the strip.
pub fn build_bar(
    area: &ResolvedArea,
    surround: Surround,
    modules: &[ModuleDescriptor],
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let &ResolvedAreaKind::Bar {
        edge,
        thickness,
        length,
        offset,
        shape,
        autohide,
    } = &area.kind
    else {
        return Err(LayoutError::Engine(format!(
            "'{}' is a {} area, and only a bar has zones to draw",
            area.id,
            area.kind.name()
        )));
    };
    let config = surround.config;
    let own_corners = shape.radius;
    let shape = bar_shape(config, shape);
    let run = run_of(
        edge,
        surround.bounds,
        surround.reserved,
        config.gap_of(&shape) as f32,
    );
    let zones = zones_of(&area.groups);
    let site = Site {
        output: surround.output.map(str::to_string),
        layer: LayerWindowContext::current().map_or(LayerKind::Top, |window| window.layer),
        edge,
        chrome: ui::chrome::Chrome::new(
            Arc::clone(config),
            shape,
            surround.output.map(str::to_string),
        ),
        gap: standoff(config, &shape),
    };
    site.chrome.provide();
    let chrome = BarFrame {
        config,
        edge,
        thickness,
        shape,
        corners: bar_corners(config, own_corners, shape),
        dress: Dress::of(config, &area.style, &surround.theme),
        abut: ends_abut(length, offset, run),
        theme: surround.theme,
        output: surround.output,
        autohide,
        strip: strip_of(
            edge,
            thickness,
            length,
            offset,
            run,
            config.gap_of(&shape) as f32,
            surround.bounds,
        ),
        site,
        area,
        surround,
    };
    match shape.mode {
        Shape::Bar => build_whole_bar(&chrome, &zones, modules),
        Shape::Sections => build_units(&chrome, &zones, modules, Granularity::Section),
        Shape::Chips => build_units(&chrome, &zones, modules, Granularity::Chip),
    }
}

/// Where `area` lands on the output `surround` describes. `None` for an area that is not a bar.
///
/// The one answer [`build_bar`] places itself by, exposed for whatever has to know where a bar ended up without building it — a sweep measuring what it painted, for one.
pub fn strip_of_area(area: &ResolvedArea, surround: Surround) -> telar::Rect {
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        length,
        offset,
        shape,
        ..
    } = area.kind
    else {
        return surround.bounds;
    };
    let gap = surround.config.gap_of(&bar_shape(surround.config, shape)) as f32;
    let run = run_of(edge, surround.bounds, surround.reserved, gap);
    strip_of(edge, thickness, length, offset, run, gap, surround.bounds)
}

/// Where a bar lies along its edge, in the window's own coordinates: the run the edge leaves it, and the stretch of that run it takes. Its `offset` is measured from the run's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub run_start: f32,
    pub run_length: f32,
    pub at: f32,
    pub along: f32,
}

impl Span {
    pub fn end(&self) -> f32 {
        self.at + self.along
    }

    pub fn run_end(&self) -> f32 {
        self.run_start + self.run_length
    }
}

/// The stretch of `edge` a bar floating `gap` off it places itself along, on the output whose box is `bounds` and whose reserving areas take `reserved`: where it starts, and how long it is.
pub fn run_along(edge: Edge, bounds: telar::Rect, reserved: Reserved, gap: f32) -> (f32, f32) {
    let run = run_of(edge, bounds, reserved, gap);
    (run.start, run.length)
}

/// Where the bar `area` lies along its edge of the output whose box is `bounds` and whose reserving areas take `reserved`. `None` for an area that is not a bar.
pub fn span_of(
    area: &ResolvedArea,
    config: &Config,
    bounds: telar::Rect,
    reserved: Reserved,
) -> Option<Span> {
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        length,
        offset,
        shape,
        ..
    } = area.kind
    else {
        return None;
    };
    let gap = config.gap_of(&bar_shape(config, shape)) as f32;
    let run = run_of(edge, bounds, reserved, gap);
    let strip = strip_of(edge, thickness, length, offset, run, gap, bounds);
    let (at, along) = match edge.is_vertical() {
        true => (strip.y, strip.height),
        false => (strip.x, strip.width),
    };
    Some(Span {
        run_start: run.start,
        run_length: run.length,
        at,
        along,
    })
}

/// The stretch of its edge a bar has to place itself along: where it starts and how long it is, in the window's own coordinates.
///
/// A horizontal bar owns its corners, so it runs the whole edge less its own gap at each end; a vertical one stops where the bar above or below it has already reserved, and falls back to its own gap where neither has. That is the corner rule the shell has always drawn by ([`config::Config::corner_owner`]), said once here instead of once per caller.
fn run_of(edge: Edge, bounds: telar::Rect, reserved: Reserved, gap: f32) -> Run {
    if edge.is_horizontal() {
        return Run {
            start: bounds.x + gap,
            length: (bounds.width - 2.0 * gap).max(0.0),
            abut: (false, false),
        };
    }
    let clear = |taken: f32| if taken > 0.0 { taken } else { gap };
    let (head, foot) = (clear(reserved.top), clear(reserved.bottom));
    Run {
        start: bounds.y + head,
        length: (bounds.height - head - foot).max(0.0),
        abut: (reserved.top > 0.0, reserved.bottom > 0.0),
    }
}

/// The stretch of an edge a bar places itself along, and which of its ends run into something already there.
#[derive(Clone, Copy)]
struct Run {
    start: f32,
    length: f32,
    abut: (bool, bool),
}

/// Which of the bar's two ends run into something, and so are owed a chip's worth of air rather than reaching a free end.
///
/// A bar that says where it starts and how far it runs has both ends where the layout put them, in the middle of its edge, with nothing there to keep clear of. [`Extent::Fill`] at offset zero is the one case where the ends are wherever the neighbouring edges left them, which is what [`run_of`] answered.
fn ends_abut(length: Extent, offset: f32, run: Run) -> (bool, bool) {
    if length != Extent::Fill || offset != 0.0 {
        return (false, false);
    }
    run.abut
}

/// The strip the bar occupies on the output: `thickness` across its edge at the area's own gap from it, and `length` of the run at `offset` along it.
fn strip_of(
    edge: Edge,
    thickness: f32,
    length: Extent,
    offset: f32,
    run: Run,
    gap: f32,
    bounds: telar::Rect,
) -> telar::Rect {
    let along = match length {
        Extent::Fill => run.length,
        Extent::Px(px) => px.min(run.length),
        Extent::Fraction(part) => run.length * part.clamp(0.0, 1.0),
    };
    let at = run.start + offset.clamp(0.0, (run.length - along).max(0.0));
    match edge {
        Edge::Top => telar::Rect::new(at, bounds.y + gap, along, thickness),
        Edge::Bottom => telar::Rect::new(
            at,
            bounds.y + bounds.height - gap - thickness,
            along,
            thickness,
        ),
        Edge::Left => telar::Rect::new(bounds.x + gap, at, thickness, along),
        Edge::Right => telar::Rect::new(
            bounds.x + bounds.width - gap - thickness,
            at,
            thickness,
            along,
        ),
    }
}

/// The area's own shape, and the theme under whatever it leaves unset.
///
/// The model measures gap, spacing and radius as floats where a resolved shape takes whole pixels, so they are rounded on the way in: each is a distance in logical px, and no part of the shell draws a fraction of one.
///
/// The one radius it answers is the largest corner's, which is what the chips on the bar nest their own inside; the strip itself is rounded corner by corner ([`bar_corners`]).
pub fn bar_shape(config: &Config, shape: BarShape) -> ResolvedShape {
    let px = |value: Option<f32>| value.map(|px| px.round() as u32);
    config.shape_from(
        shape.mode,
        px(shape.gap),
        px(shape.spacing),
        px(shape.radius.map(Corners::largest)),
    )
}

/// One thing a zone lays out: a chip, a stacked group's chips shown one at a time in one place, or a repeated group's copies.
enum Slot<'a> {
    Chip(&'a ResolvedGroup, &'a ResolvedInstance),
    Stacked(&'a ResolvedGroup),
    Repeated(&'a ResolvedGroup),
}

/// A bar's three zones, each what is placed in it.
type Zones<'a> = [(Vec<Slot<'a>>, Zone); 3];

/// The area's groups gathered into the three runs a bar draws. Several groups may name the same zone, so a zone is every group that claimed it in the order they were placed, rather than one group's children.
fn zones_of(groups: &[ResolvedGroup]) -> Zones<'_> {
    let run = |wanted: Zone| {
        groups
            .iter()
            .filter(|group| matches!(group.kind, GroupKind::Zone { zone } if zone == wanted))
            .flat_map(|group| match (group.repeat.is_some(), group.stacked) {
                (true, _) => vec![Slot::Repeated(group)],
                (false, true) => vec![Slot::Stacked(group)],
                (false, false) => group
                    .children
                    .iter()
                    .map(|instance| Slot::Chip(group, instance))
                    .collect(),
            })
            .collect()
    };
    [
        (run(Zone::Start), Zone::Start),
        (run(Zone::Center), Zone::Center),
        (run(Zone::End), Zone::End),
    ]
}

fn justify(zone: Zone) -> JustifyContent {
    match zone {
        Zone::Start => JustifyContent::START,
        Zone::Center => JustifyContent::CENTER,
        Zone::End => JustifyContent::END,
    }
}

#[derive(Clone)]
struct BarFrame<'a> {
    config: &'a Arc<Config>,
    edge: Edge,
    /// How thick the area is across its edge, which is the only size a chip on it is given.
    thickness: f32,
    shape: ResolvedShape,
    corners: BorderRadius,
    dress: Dress,
    /// Whether the leading and trailing ends meet something; see [`ends_abut`].
    abut: (bool, bool),
    theme: NordTheme,
    output: Option<&'a str>,
    /// Where on the output the bar sits, which is what it places itself at.
    strip: telar::Rect,
    /// How the bar takes itself off screen when it is not wanted, and how much of it stays behind.
    autohide: Option<AutoHide>,
    /// Where this bar's chips are, for whatever hangs off one of them.
    site: Site,
    area: &'a ResolvedArea,
    surround: Surround<'a>,
}

/// The box a chip is given: the area's thickness across it, and no length of its own along it.
fn chip_extent(edge: Edge, thickness: f32) -> Size {
    if edge.is_vertical() {
        Size {
            width: thickness,
            height: f32::INFINITY,
        }
    } else {
        Size {
            width: f32::INFINITY,
            height: thickness,
        }
    }
}

#[derive(Clone, Copy)]
enum Granularity {
    Section,
    Chip,
}

/// One owner per pixel: a bar is never [`crate::area::dressed`], because its strip already paints the whole box; the style is read into that paint instead, and its corners stay `shape.radius`, which its chips nest inside.
#[derive(Clone, Copy)]
struct Dress {
    fill: Option<Color>,
    alpha: f32,
    padding: Option<f32>,
}

impl Dress {
    fn of(config: &Config, style: &AreaStyle, theme: &NordTheme) -> Self {
        Self {
            fill: style
                .fill
                .as_deref()
                .map(|fill| layout::color_of(fill, theme)),
            alpha: style
                .opacity
                .unwrap_or_else(|| config.opacity())
                .clamp(0.0, 1.0),
            padding: style.padding,
        }
    }
}

/// A `fill` of the area's own paints the strip in any mode; under `[shape] frame` every bar fills its strip flat to make the ring; otherwise only `bar` mode has a background, so in `sections` and `chips` a press between chips belongs to the window underneath ([`painted_chrome`]).
fn strip_fill(config: &Config, mode: Shape, dress: Dress, token: Color) -> Color {
    match dress.fill {
        Some(own) => own.with_alpha(dress.alpha),
        None if config.shape.frame || matches!(mode, Shape::Bar) => token.with_alpha(dress.alpha),
        None => Color::TRANSPARENT,
    }
}

/// What a section's panel or a resting chip paints. Nothing while a frame is up: the bar has already filled its strip flat, and a second fill over those pixels is a darker band along every edge the two share.
fn inner_fill(config: &Config, dress: Dress, token: Color) -> Color {
    if config.shape.frame {
        return Color::TRANSPARENT;
    }
    token.with_alpha(dress.alpha)
}

/// How an autohiding bar is away: how far it is moved off its edge, and how far on screen it is (0 away, 1 shown).
///
/// **The bar moves by transform, not by layout.** A translate is a `PushMatrix` change, which telar's diff scopes to the subtree that moved (F-5.2), where moving it by layout would re-measure three zones on every frame of the slide. The input region follows the transform — `interactive_rects` lifts every claim through its node's placements — so the only part of the bar the compositor is handed while it is away is the `peek` that is still on screen. That is the whole of the hot rect: there is no second strip to keep in step with the bar, which is what the surface-moving version had to do.
///
/// **What brings it back.** With `on_hover` the pointer reaching the peek does, and leaving the bar sends it away. Without it only a deliberate gesture does: a pull from the peek, inward across the edge, at least [`PULL`] px — travel along the edge brings nothing back, so a pointer sliding down the side of the screen cannot. The pull is taken by a band laid over the peek alone, since a bar's own chrome claims the pointer and would keep a drag from anything around it; the band gives the pointer back to the bar's chips once the bar is on its way in. The bar then stays until the pointer leaves it, at once if the pull was let go past it.
#[derive(Clone, Copy)]
struct Hiding {
    edge: Edge,
    peek: f32,
    on_hover: bool,
    /// How far the bar is moved while it is away.
    away: (f32, f32),
    shown: Animated<f32>,
    pulling: RwSignal<bool>,
}

impl Hiding {
    fn of(chrome: &BarFrame) -> Option<Self> {
        let hide = chrome.autohide?;
        let away = (chrome.thickness - hide.peek).max(0.0);
        let away = match chrome.edge {
            Edge::Top => (0.0, -away),
            Edge::Bottom => (0.0, away),
            Edge::Left => (-away, 0.0),
            Edge::Right => (away, 0.0),
        };
        Some(Self {
            edge: chrome.edge,
            peek: hide.peek,
            on_hover: hide.on_hover,
            away,
            // Built at 0 and retargeted rather than at its destination, which would leave it inert — the same rule the wallpaper's cross-fade follows.
            shown: Animated::new(0.0f32, chrome.config.animation.tween_ms(160, 1_000)),
            pulling: telar::signal(false),
        })
    }

    /// `placed` moved off its edge for as long as the bar is away, and brought back by hovering where that is what it asks for.
    fn wrap(self, placed: StyledContainer) -> StyledContainer {
        let Hiding { away, shown, .. } = self;
        let placed = placed.with_transform(move |rect| {
            let out = 1.0 - shown.get();
            box_transform(rect, 0.0, 1.0, 1.0, away.0 * out, away.1 * out)
        });
        if self.on_hover {
            return placed.on_hover(move |inside| shown.retarget(if inside { 1.0 } else { 0.0 }));
        }
        let pulling = self.pulling;
        placed.on_hover(move |inside| {
            if !inside && !pulling.peek() {
                shown.retarget(0.0);
            }
        })
    }

    /// Where the band that takes a pull lies in the bar's own box `size` big: over the part of it that is still on screen while it is away.
    fn band(self, size: telar::Rect) -> telar::Rect {
        let peek = self.peek;
        match self.edge {
            Edge::Top => telar::Rect::new(0.0, size.height - peek, size.width, peek),
            Edge::Bottom => telar::Rect::new(0.0, 0.0, size.width, peek),
            Edge::Left => telar::Rect::new(size.width - peek, 0.0, peek, size.height),
            Edge::Right => telar::Rect::new(0.0, 0.0, peek, size.height),
        }
    }

    /// The band over the peek that a pull inward starts on, for a bar that hovering does not bring back.
    fn puller(self, size: telar::Rect) -> Built {
        let Hiding {
            edge,
            away,
            shown,
            pulling,
            ..
        } = self;
        let band = self.band(size);
        let pulled_in = move |(x, y): (f32, f32)| match edge {
            Edge::Top => y - band.height,
            Edge::Bottom => -y,
            Edge::Left => x - band.width,
            Edge::Right => -x,
        };
        Ok(Box::new(
            StyledContainer::new(crate::area::at(band), |_| RectStyle::default(), Vec::new())?
                .inert(move || !pulling.get() && shown.get() > 0.0)
                .drag_axis(match edge.is_vertical() {
                    true => telar::DragAxis::Horizontal,
                    false => telar::DragAxis::Vertical,
                })
                .drag_threshold(4.0)
                .on_drag(move |x, y| {
                    if pulling.peek() || pulled_in((x, y)) < PULL {
                        return;
                    }
                    pulling.set(true);
                    shown.retarget(1.0);
                })
                .on_drag_end(move |x, y| {
                    if !pulling.peek() {
                        return;
                    }
                    pulling.set(false);
                    // Measured in the frame the pull was pressed in, where the bar was still away.
                    let (x, y) = (band.x + x + away.0, band.y + y + away.1);
                    let over = (0.0..=size.width).contains(&x) && (0.0..=size.height).contains(&y);
                    if !over {
                        shown.retarget(0.0);
                    }
                }),
        ))
    }
}

/// How far a pull from a hidden bar's peek has to travel inward before it brings back a bar that hovering does not.
pub const PULL: f32 = 12.0;

/// Cuts the bar off at its own strip, and takes it off its edge while it hides itself ([`Hiding`]).
///
/// A chip is routinely a shade wider than the strip its zone was given — a padded box is narrower than the bar and a square chip is sized from the bar itself — and while a bar had a surface of its own, the surface cut that overhang off for nothing. With one window per layer there is no surface to cut it: the overhang lands on the desktop, drawn but claimed by nothing, so a press on it reaches the application underneath. The clip is at the bar's root rather than at a zone for exactly that reason: what a zone cuts is one run running into another, and what this cuts is the bar running off its own edge.
///
/// The cut is inside what hides the bar, so it travels with it: a chip overhanging a vertical bar's inner edge stays cut while the bar is away, instead of the overhang sliding into a cut that stayed where the bar was shown and claiming more of the screen than its peek (F-10.30).
fn strip_clipped(chrome: &BarFrame, bar: StyledContainer) -> Built {
    let hiding = Hiding::of(chrome);
    let size = telar::Rect::new(0.0, 0.0, chrome.strip.width, chrome.strip.height);
    let mut children: Vec<Box<dyn LayoutItem>> =
        vec![Box::new(ClippedItem::new(Box::new(bar), Clip::both()))];
    if let Some(hiding) = hiding.filter(|hiding| !hiding.on_hover) {
        children.push(hiding.puller(size)?);
    }
    let placed = StyledContainer::new(
        crate::area::at(chrome.strip),
        |_| RectStyle::default(),
        children,
    )?;
    Ok(Box::new(match hiding {
        Some(hiding) => hiding.wrap(placed),
        None => placed,
    }))
}

/// How far each corner of a bar's own background is rounded: the bar's own corners where it names them, else the one radius its shape resolved to. A ring made of rounded pills is four floating bars rather than a frame, so under `[shape] frame` the strip is square. Its rounded *inner* corners went with the surface that drew them: a concave corner at the junction of two strips lies inside neither, so no area can paint it (F-10.23).
fn bar_corners(config: &Config, own: Option<Corners>, shape: ResolvedShape) -> BorderRadius {
    if config.shape.frame {
        return BorderRadius::zero();
    }
    let px = |corner: f32| corner.round().max(0.0);
    match own {
        Some(corners) => Corners::each(
            px(corners.top_left()),
            px(corners.top_right()),
            px(corners.bottom_right()),
            px(corners.bottom_left()),
        )
        .into(),
        None => BorderRadius::all(shape.radius),
    }
}

fn build_whole_bar(
    chrome: &BarFrame,
    zones: &Zones,
    modules: &[ModuleDescriptor],
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let BarFrame {
        config,
        edge,
        shape,
        abut,
        theme,
        ..
    } = *chrome;
    let base = strip_fill(config, Shape::Bar, chrome.dress, theme.base);
    let spacing = shape.spacing;
    let padding = chrome.dress.padding.unwrap_or_else(|| shape.padding());
    // The whole bar already pads every side by `padding`, so an end that meets another bar is only owed the rest of a chip's worth of air.
    let ends = end_air(abut, (spacing - padding).max(0.0));
    let mut slots = Vec::with_capacity(3);
    for (entries, in_zone) in zones {
        // Modules blend into the shared surface (transparent rest); STRETCH makes every chip the bar's height so text pills and icon chips line up. The hover/press (and Filled) highlight rounds at the theme's chip radius, matching chip mode.
        let items = build_items(
            chrome,
            entries,
            modules,
            Color::TRANSPARENT,
            shape.chip_radius(),
        )?;
        slots.push(zone(
            edge,
            *in_zone,
            spacing,
            ends,
            AlignItems::STRETCH,
            items,
        )?);
    }
    let corners = chrome.corners;
    let style = axis(
        fill().align_items(AlignItems::CENTER).padding_all(padding),
        edge,
    );
    strip_clipped(
        chrome,
        painted_chrome(
            crate::area::empty_space(
                chrome.area,
                chrome.surround,
                StyledContainer::new(
                    style,
                    move |_r| RectStyle::filled(base, 0.0).with_radius(corners),
                    slots,
                )?,
                base.a > 0.0,
            ),
            base,
        ),
    )
}

fn build_units(
    chrome: &BarFrame,
    zones: &Zones,
    modules: &[ModuleDescriptor],
    granularity: Granularity,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let BarFrame {
        config,
        edge,
        shape,
        abut,
        theme,
        ..
    } = *chrome;
    let spacing = shape.spacing;
    // Section: modules share a per-zone surface panel (wrapped in `unit`); Chip: each module is its own free-standing pill, no `unit`.
    let surface = inner_fill(config, chrome.dress, theme.surface);
    let (rest, shell_radius) = match granularity {
        Granularity::Section => (Color::TRANSPARENT, shape.chip_radius()),
        Granularity::Chip => (surface, shape.chip_radius()),
    };
    let mut slots = Vec::with_capacity(3);
    for (entries, in_zone) in zones {
        let items = build_items(chrome, entries, modules, rest, shell_radius)?;
        let content: Vec<ChildSlot> = if items.is_empty() {
            Vec::new()
        } else {
            match granularity {
                Granularity::Section => {
                    vec![ChildSlot::stat(unit(
                        edge,
                        *in_zone,
                        shape.radius,
                        spacing,
                        surface,
                        items,
                    )?)]
                }
                Granularity::Chip => items,
            }
        };
        // STRETCH ensures height is parent-driven by bar size, not content-driven.
        slots.push(zone(
            edge,
            *in_zone,
            spacing,
            end_air(abut, spacing),
            AlignItems::STRETCH,
            content,
        )?);
    }
    // No gap between the zones here: there are only ever three of them, so the only two joins it could space are the two the sides already hold open with a margin of their own (see [`zone`]). Both applying left twice the air at exactly the place a side is cut — a hole where the rest of the bar has one chip's worth.
    let style = axis(
        fill()
            .align_items(AlignItems::STRETCH)
            .padding_all(chrome.dress.padding.unwrap_or(0.0)),
        edge,
    );
    let base = strip_fill(config, shape.mode, chrome.dress, theme.base);
    let corners = chrome.corners;
    strip_clipped(
        chrome,
        painted_chrome(
            crate::area::empty_space(
                chrome.area,
                chrome.surround,
                StyledContainer::new(
                    style,
                    move |_r| RectStyle::filled(base, 0.0).with_radius(corners),
                    slots,
                )?,
                base.a > 0.0,
            ),
            base,
        ),
    )
}

/// Shared surface panel behind a zone's modules (sections mode); children STRETCH with no inner padding so a filled chip reaches the panel edges instead of leaving a thin sliver.
///
/// It is never longer than the zone holding it. Sized from its content instead, a panel behind more chips than the zone can take ran past the cut and was clipped square there — so the section ended in a flat grey stub with nothing drawn on it, which reads as a hole in the bar rather than as a section that ran out of room. Giving up the length it cannot use puts its own rounded end back at the cut, and the chips that overflow it are the clip's business, as they already were.
///
/// It packs its chips the way its zone does, for the same reason: what a shortened panel pushes out has to go out the end nearest the centre, not spill from both at once.
fn unit(
    edge: Edge,
    in_zone: Zone,
    radius: f32,
    spacing: f32,
    fill: Color,
    items: Vec<ChildSlot>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let style = LayoutStyle::new()
        .align_items(AlignItems::STRETCH)
        .justify_content(justify(in_zone))
        .gap(spacing)
        .flex_shrink(1.0);
    let style = if edge.is_horizontal() {
        style.min_width(0.0)
    } else {
        style.min_height(0.0)
    };
    Ok(Box::new(painted_chrome(
        StyledContainer::from_slots(
            axis(style, edge),
            move |_r| RectStyle::filled(fill, radius),
            items,
        )?,
        fill,
    )))
}

/// One of a bar's three zones.
///
/// The centre keeps its own size and the two sides split everything else (`flex-basis: 0`), which is what puts the centre on the middle of the *bar*. Sizing all three from their content plus an equal share of the slack centres it on the leftover space instead — so it slid sideways every time a chip next to it changed width, and the chip that does that constantly is the window title.
///
/// A side that outgrows its half is cut off at the centre's edge rather than allowed to push: the minimum along the bar is zero, so the zone keeps its half whatever it holds, and the clip stops the overflow from being drawn — or clicked — over the centre. The chip that reaches the boundary is cut mid-way, which is what says "there is more here" better than a chip that vanishes whole, and the ones past it never appear. Their zone is justified towards its outer end, so what gets cut is always the side nearest the centre.
///
/// The clip runs along the bar only. Across it a chip is routinely a shade wider than the strip its zone was given — the padded box is narrower than the bar, and a square chip is sized from the bar itself — so cutting on that axis too shaved the edge off every one of them, which is a rounded pill with its corners sanded flat and an icon missing its outermost pixels.
///
/// A side stops `spacing` short of the centre, so the cut edge never lands flush against the centre's first chip: a sliced chip touching a whole one reads as one wide chip with a seam, where the same slice with air after it reads as what it is. The margin comes out of the free space both sides divide, so it costs the centre nothing and leaves it exactly where it was.
///
/// Both sides give up the same length in total whichever end owes air off another bar, or the centre would slide off the middle by half the difference.
fn zone(
    edge: Edge,
    in_zone: Zone,
    spacing: f32,
    ends: (f32, f32),
    cross: AlignItems,
    items: Vec<ChildSlot>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let style = LayoutStyle::new()
        .align_items(cross)
        .justify_content(justify(in_zone))
        .gap(spacing);
    if let Zone::Center = in_zone {
        return Ok(Box::new(holding(axis(style, edge), items)?));
    }
    let style = style.flex_grow(1.0).flex_basis(0.0);
    let (lead, trail) = ends;
    let reach = spacing + lead.max(trail);
    let (start, end) = match in_zone {
        Zone::Start => (lead, reach - lead),
        _ => (reach - trail, trail),
    };
    let (style, clip) = if edge.is_horizontal() {
        let style = style
            .min_width(0.0)
            .margin_inline_start(start)
            .margin_inline_end(end);
        (style, Clip::x())
    } else {
        let style = style
            .min_height(0.0)
            .margin_block_start(start)
            .margin_block_end(end);
        (style, Clip::y())
    };
    let zone = holding(axis(style, edge), items)?;
    Ok(Box::new(ClippedItem::new(Box::new(zone), clip)))
}

/// A box around `slots`: a plain one unless a repeated group's copies are among them, which need a box that reconciles them in place as its list moves.
fn holding(style: LayoutStyle, slots: Vec<ChildSlot>) -> Result<Container, LayoutError> {
    if slots
        .iter()
        .any(|slot| matches!(slot, ChildSlot::Dynamic(_)))
    {
        return Container::from_slots(style, slots);
    }
    let items = slots
        .into_iter()
        .filter_map(|slot| match slot {
            ChildSlot::Static(item) => Some(item),
            ChildSlot::Dynamic(_) => None,
        })
        .collect();
    Container::new(style, items)
}

fn end_air(abut: (bool, bool), air: f32) -> (f32, f32) {
    let owed = |abuts: bool| if abuts { air } else { 0.0 };
    (owed(abut.0), owed(abut.1))
}

/// How a chip's wrapper is dressed: how it sits in its zone, and the surface it rests on.
#[derive(Clone, Copy)]
struct Wrapper {
    cross: AlignItems,
    edge: Edge,
    elastic: bool,
    fill: Color,
    radius: f32,
}

/// A box wrapped around a module's own content to carry what its chip cannot: a wheel handler, the presses its layout binds and the surface a chip rests on for a self-managed module (which has no [`module_shell`] to put them on), and the pointer tracking behind a hover popout. They live here rather than on the chip so a self-managed module gets them on the same terms as any other; the wrapper shrink-wraps its child, so the rect it tracks and the surface it paints are the chip's own.
///
/// `cross` is what the wrapper would otherwise silently change. A chip is a direct zone child under `AlignItems::STRETCH`, so it fills the bar's thickness; a wrapper that centred it instead would shrink every popout-bearing chip to its content. A self-managed module lays itself out and is centred, as it was before any wrapper existed.
///
/// It runs along the bar for the same reason [`zone`] does. A wrapper fixed to a row applies `cross` across the *screen's* vertical, so on a left or right bar it stretched each chip's height — which is already its content — and left its width free: every wrapped chip then sat at its own content width, ragged against the bar's inner edge, with the wide ones running off the screen. Thirteen chips carry a popout, so on a vertical bar that was most of them. `elastic` is the chip's own, forwarded. The wrapper is what the zone actually sizes, so a rigid one around an elastic chip is a chip that never gets the chance to give anything up — and both modules whose label elides carry a popout, which is to say both of them are wrapped.
fn chip_wrapper(
    content: Box<dyn LayoutItem>,
    on_scroll: Option<Wheel>,
    popout: Option<(Instance, Site)>,
    dress: Wrapper,
    presses: &Bound,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Wrapper {
        cross,
        edge,
        elastic,
        fill,
        radius,
    } = dress;
    let style = LayoutStyle::new().align_items(cross);
    let style = if elastic {
        style.flex_shrink(1.0).min_width(0.0).min_height(0.0)
    } else {
        style.flex_shrink(0.0)
    };
    let style = axis(style, edge);
    let mut wrapper = presses.presses(painted_chrome(
        StyledContainer::new(
            style,
            move |_r| RectStyle::filled(fill, radius),
            vec![content],
        )?,
        fill,
    ));
    if let Some(on_scroll) = on_scroll {
        wrapper = wrapper.on_scroll(move |dx, dy| on_scroll(dx, dy));
    }
    if let Some((instance, site)) = popout {
        // Tracked before the handler that reads it is attached, so the popout has a rect the first time the pointer arrives.
        let rect = track_layout(wrapper.layout_node())
            .expect("a container registers its rect")
            .read_only();
        wrapper = wrapper.on_hover(move |entered| {
            crate::popout::hover(&instance, site.anchor(rect.get()), entered)
        });
    }
    Ok(Box::new(wrapper))
}

/// The drag-to-open gesture for a chip, when it has a panel to open and the gesture is switched on; a chip with nothing to open gets none, since arming a gesture that can only do nothing would still cancel its tap.
fn drag_open_for(config: &Config, module: &ModuleDescriptor, edge: Edge) -> Option<DragOpen> {
    if !opens_panel(module) {
        return None;
    }
    let threshold = config.panels.drag_threshold()?;
    Some(DragOpen {
        module: module.id.to_string(),
        edge,
        threshold,
    })
}

/// A chip opens its module's panel when the module has one and the chip was given no press of its own.
fn opens_panel(module: &ModuleDescriptor) -> bool {
    let chip_acts = module
        .representations
        .chip
        .is_some_and(|chip| chip.press.is_some());
    module.representations.panel.is_some() && !chip_acts
}

fn axis(style: LayoutStyle, edge: Edge) -> LayoutStyle {
    if edge.is_horizontal() {
        style.flex_row()
    } else {
        style.flex_column()
    }
}

/// What a chip speaks for: its state keyed by the module, and its entry's own options under what its bindings say over them (`overlay`).
///
/// Keyed by the module rather than by the placed instance's own id: a chip's press, a keybind and `hogar-shell panel toggle` all name a module, so a chip that kept its state under a layout id would stop sharing it with the three ways the same instance is reached from outside the bar. The options are the entry's own, so what it draws and what it opens follow what the layout says about it (TA-2).
fn chip_instance(instance: &ResolvedInstance, overlay: Option<&Overlay>) -> Instance {
    Instance::new(
        InstanceId::of_module(&instance.module),
        &instance.module,
        Overlay::options_or(overlay, &instance.options),
    )
}

/// Builds each thing a zone lays out; a module no id answers to, one with no chip, and a chip whose build fails or panics are each drawn as a [placeholder](ui::placeholder) where declared, so the mistake stays on screen.
///
/// A group has no box of its own on a bar — its chips share their zone with every other group's — so where it is is the smallest rect around its chips, except for a stacked group, which is one box.
fn build_items(
    chrome: &BarFrame,
    slots: &[Slot],
    modules: &[ModuleDescriptor],
    rest: Color,
    radius: f32,
) -> Result<Vec<ChildSlot>, LayoutError> {
    let kit = Rc::new(ChipKit::of(chrome, modules, rest, radius));
    let mut items: Vec<ChildSlot> = Vec::with_capacity(slots.len());
    let mut spans: Vec<(&GroupId, Vec<RwSignal<telar::Rect>>)> = Vec::new();
    for slot in slots {
        match slot {
            Slot::Chip(group, instance) => {
                let (item, rect) = kit.chip(&group.id, instance)?;
                let at = match spans.iter().position(|(id, _)| **id == group.id) {
                    Some(at) => at,
                    None => {
                        spans.push((&group.id, Vec::new()));
                        spans.len() - 1
                    }
                };
                spans[at].1.extend(rect);
                items.push(ChildSlot::stat(item));
            }
            Slot::Stacked(group) => {
                items.push(ChildSlot::stat(kit.stacked(group, chrome.surround)?))
            }
            Slot::Repeated(group) => items.extend(kit.repeated(group, chrome.surround)),
        }
    }
    for (group, rects) in spans {
        rects::track_spanning(kit.at.group(group), rects);
    }
    Ok(items)
}

/// A chip, and the rect it is tracked by in [`rects`].
type Tracked = (Box<dyn LayoutItem>, Option<RwSignal<telar::Rect>>);

/// Everything building one chip of a bar needs, owned, so a stacked group on the bar can build its chips again whenever it is cycled.
struct ChipKit {
    config: Arc<Config>,
    edge: Edge,
    thickness: f32,
    shape: ResolvedShape,
    theme: NordTheme,
    output: Option<String>,
    site: Site,
    modules: Rc<[ModuleDescriptor]>,
    rest: Color,
    radius: f32,
    /// The bar itself in [`rects`], which each chip is registered under.
    at: rects::Node,
    audience: Audience,
}

impl ChipKit {
    fn of(chrome: &BarFrame, modules: &[ModuleDescriptor], rest: Color, radius: f32) -> Self {
        Self {
            config: Arc::clone(chrome.config),
            edge: chrome.edge,
            thickness: chrome.thickness,
            shape: chrome.shape,
            theme: chrome.theme,
            output: chrome.output.map(str::to_string),
            site: chrome.site.clone(),
            modules: modules.into(),
            rest,
            radius,
            at: rects::Node::area(chrome.output, chrome.surround.layer, &chrome.area.id),
            audience: chrome.surround.audience,
        }
    }

    /// The host a chip is built under.
    fn host(&self, instance: Instance, accent: Color, foreground: Color) -> Host {
        Host::placed(
            instance,
            Arc::clone(&self.config),
            Representation::Chip,
            chip_extent(self.edge, self.thickness),
            Some(self.edge),
            self.shape,
            accent,
            foreground,
            self.output.clone(),
        )
    }

    /// One instance's chip, registered in [`rects`] under `group`, and the rect it is tracked by. A chip with bindings is built again, alone, each time what they say changes.
    fn chip(
        self: &Rc<Self>,
        group: &GroupId,
        instance: &ResolvedInstance,
    ) -> Result<Tracked, LayoutError> {
        let at = self.at.instance(group, &instance.id);
        let style = ui::descriptor::lookup(&self.modules, &instance.module)
            .and_then(|module| module.representations.chip)
            .map_or_else(
                || axis(LayoutStyle::new(), self.edge),
                |chip| chip_box(&chip, self.edge),
            );
        let (kit, dressing, node) = (Rc::clone(self), instance.clone(), at.clone());
        let item = Expressions::here(self.audience).bound_instance(
            instance,
            &at,
            style,
            move |_, overlay| kit.dressed(&node, &dressing, overlay),
        )?;
        let rect = track_layout(item.layout_node());
        Ok((item, rect))
    }

    /// One build of a chip, its written options under what `overlay` binds over them. It lays itself out in its chip's own box, whatever box its bindings are rebuilt in.
    fn dressed(
        &self,
        at: &rects::Node,
        instance: &ResolvedInstance,
        overlay: Option<&Overlay>,
    ) -> Built {
        let config = &self.config;
        let id = &instance.module;
        let speaks_for = chip_instance(instance, overlay);
        let presentation = speaks_for.presentation(config);
        let variant = presentation.variant;
        let accent = Overlay::accent_or(overlay, || {
            self.theme.accent_by_name(config.accent_name(&presentation))
        });
        let host = self.host(
            speaks_for,
            accent,
            module_foreground(variant, accent, self.theme),
        );
        let menu = crate::menu::on(at.clone(), self.audience);
        let placed = ui::descriptor::lookup(&self.modules, id)
            .and_then(|module| Some((*module, module.representations.chip?)));
        let Some((module, chip)) = placed else {
            let item = placeholder(id, None, &host, self.theme, menu)?;
            rects::track(at.clone(), item.layout_node());
            return Ok(item);
        };
        let look = Look {
            variant,
            rest: self.rest,
            accent,
            radius: self.radius,
            edge: self.edge,
            popout: module.representations.popout.is_some() && config.popouts.enabled,
            drag_open: drag_open_for(config, &module, self.edge),
            site: self.site.clone(),
            bound: Bound::of(&instance.actions, self.audience).with_menu(menu.clone()),
        };
        let style = chip_box(&chip, self.edge);
        let built = host.clone();
        let item = ui::descriptor::guard(id, &host, style, menu, move || {
            placed_chip(&module, &chip, &built, look)
        })?;
        if let Some(rect) = track_layout(item.layout_node()) {
            rects::track_chip(
                at.clone(),
                rect,
                rects::Chip {
                    instance: host.instance(),
                    site: self.site.clone(),
                },
            );
        }
        Ok(item)
    }

    /// A stacked group's chips, one at a time where one chip would be.
    fn stacked(self: &Rc<Self>, group: &ResolvedGroup, surround: Surround) -> Built {
        let key = (self.output.clone(), self.at.area.clone(), group.id.clone());
        let style = axis(
            LayoutStyle::new()
                .align_items(AlignItems::STRETCH)
                .flex_shrink(0.0),
            self.edge,
        );
        let kit = Rc::clone(self);
        let id = group.id.clone();
        let node =
            crate::area::smart_stack(key, &group.children, style, surround, move |instance, _| {
                kit.chip(&id, instance).map(|(item, _)| item)
            })?;
        rects::track(self.at.group(&group.id), node.layout_node());
        Ok(node)
    }

    /// A repeated group's chips, one per copy, straight into the zone beside every other chip — or, stacked, its copies one at a time where one chip would be. The group is where its chips are now, whichever copies it has.
    fn repeated(self: &Rc<Self>, group: &ResolvedGroup, surround: Surround) -> Vec<ChildSlot> {
        let repeat = group
            .repeat
            .as_ref()
            .map(|expr| Expressions::here(self.audience).repeat(&self.at.group(&group.id), expr));
        let chips: rects::Copies = telar::signal(Default::default());
        rects::track_copies(self.at.group(&group.id), chips);
        let (kit, id) = (Rc::clone(self), group.id.clone());
        let build: crate::area::BuildCopy = Rc::new(move |copy: &ResolvedInstance, _: Surround| {
            let (item, rect) = kit.chip(&id, copy)?;
            if let Some(rect) = rect {
                let copy = copy.id.clone();
                chips.update(|held| {
                    held.insert(copy.clone(), rect);
                });
                telar::on_cleanup(move || {
                    if chips.is_alive() {
                        chips.update(|held| {
                            held.remove(&copy);
                        });
                    }
                });
            }
            Ok(item)
        });
        let key = (self.output.clone(), self.at.area.clone(), group.id.clone());
        let pages = axis(
            LayoutStyle::new()
                .align_items(AlignItems::STRETCH)
                .flex_shrink(0.0),
            self.edge,
        );
        crate::area::copies(group, repeat, (key, pages), surround, build)
    }
}

/// How a chip is dressed on this bar, resolved before its build so the build can run where a failure is caught.
struct Look {
    variant: Variant,
    rest: Color,
    accent: Color,
    radius: f32,
    edge: Edge,
    popout: bool,
    drag_open: Option<DragOpen>,
    site: Site,
    /// What the layout binds to this instance's gestures, each in place of the chip's own for that gesture alone, and its context menu on a secondary press nothing is bound to.
    bound: Bound,
}

/// The box a chip's failure is caught in, carrying the part the chip plays in its zone: a filler takes the room left over, an elastic chip gives up length, and every other chip holds its own.
fn chip_box(chip: &ChipDef, edge: Edge) -> LayoutStyle {
    let style = axis(LayoutStyle::new().align_items(AlignItems::STRETCH), edge);
    match (chip.frame, chip.elastic) {
        (ChipFrame::Filler, _) => style.flex_grow(1.0).flex_shrink(1.0),
        (_, true) => style.flex_shrink(1.0).min_width(0.0).min_height(0.0),
        (_, false) => style.flex_shrink(0.0),
    }
}

fn placed_chip(
    module: &ModuleDescriptor,
    chip: &ChipDef,
    host: &Host,
    look: Look,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let content = module.build(host).unwrap_or_else(|| {
        Err(LayoutError::Engine(format!(
            "'{}' declares no chip",
            module.id
        )))
    })?;
    let popout = look.popout.then(|| (host.instance(), look.site.clone()));
    let wheel = look.bound.wheel(wheel(chip, host));
    if chip.is_bare() {
        return bare_chip(
            content,
            chip,
            wheel,
            popout,
            look.edge,
            resting_fill(look.variant, look.rest, look.accent),
            look.radius,
            &look.bound,
        );
    }
    // Handed over bare: the chip shell dispatches every press with its own rect in scope, a panel toggle and a press of the chip's own alike, so whatever this opens can hang off the chip without being told where it is.
    let own_press: Option<Rc<dyn Fn()>> = match chip.press {
        Some(press) => Some(Rc::new(press)),
        None if opens_panel(module) => {
            let id = module.id;
            Some(Rc::new(move || crate::panel::toggle_panel(id)))
        }
        None => None,
    };
    let mut inner = Slots::new();
    inner.push(None, content);
    let shell = module_shell(
        ModuleShellProps::props()
            .variant(look.variant)
            .rest(look.rest)
            .accent(look.accent)
            .radius(look.radius)
            .square(chip.square)
            .inset(host.inset())
            .vertical(host.is_vertical())
            .elastic(chip.elastic)
            .on_press(look.bound.press(own_press))
            .on_long_press(look.bound.long_press())
            .on_alt_press(look.bound.alt_press())
            .on_scroll(wheel)
            .drag_open(look.drag_open)
            .build(),
        telar::Children::new({
            let inner = std::cell::RefCell::new(Some(inner));
            move || {
                inner
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| LayoutError::Engine("children built twice".into()))
            }
        }),
    )?;
    // Outside the chip rather than on it: the chip's own hover already swaps its paint, and stacking a second meaning onto that callback would tie the two together.
    match popout {
        Some(module) => chip_wrapper(
            shell,
            None,
            Some(module),
            Wrapper {
                cross: AlignItems::STRETCH,
                edge: look.edge,
                elastic: chip.elastic,
                fill: Color::TRANSPARENT,
                radius: 0.0,
            },
            &Bound::default(),
        ),
        None => Ok(shell),
    }
}

/// The chip's wheel handler bound to the host it was built under, so a notch acts on this bar's options rather than the global config.
fn wheel(chip: &ChipDef, host: &Host) -> Option<Wheel> {
    let scroll = chip.scroll?;
    let host = host.clone();
    Some(Rc::new(move |dx, dy| scroll(&host, dx, dy)))
}

/// A self-managed module skips `module_shell` — it lays itself out and takes its own presses — so its wheel handler, the presses its layout binds, popout tracking and the surface it rests on go on a wrapper with no padding or hover state. What the module answers itself stays its own: it is inside the wrapper, and answers first.
#[allow(clippy::too_many_arguments)]
fn bare_chip(
    content: Box<dyn LayoutItem>,
    chip: &ChipDef,
    wheel: Option<Wheel>,
    popout: Option<(Instance, Site)>,
    edge: Edge,
    resting: Color,
    radius: f32,
    bound: &Bound,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fill = match chip.frame {
        ChipFrame::Filler => Color::TRANSPARENT,
        _ => resting,
    };
    if wheel.is_none() && popout.is_none() && fill == Color::TRANSPARENT && !bound.has_presses() {
        return Ok(content);
    }
    chip_wrapper(
        content,
        wheel,
        popout,
        Wrapper {
            cross: AlignItems::CENTER,
            edge,
            elastic: false,
            fill,
            radius,
        },
        bound,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{AvailableSpace, compute_layout, reset_layout_runtime, set_theme};
    use ui::descriptor::{CardDef, Input, Representations};

    /// Builds `area` on a screen exactly the size of the page the test lays it out on, with nothing else on the output.
    ///
    /// A bar places itself on its output now, so a test has to say how big that output is or the strip lands somewhere the page cannot show. `page` is the same pair the test hands `compute_layout`, which is what keeps the two from drifting.
    fn built(
        config: &Config,
        area: &ResolvedArea,
        modules: &[ModuleDescriptor],
        page: (f32, f32),
    ) -> Result<Box<dyn LayoutItem>, LayoutError> {
        built_amid(config, area, &[], modules, page)
    }

    /// [`built`], but with `neighbours` on the same output, so their reservation reaches `area` the way a bar above or below a vertical one does.
    fn built_amid(
        config: &Config,
        area: &ResolvedArea,
        neighbours: &[ResolvedArea],
        modules: &[ModuleDescriptor],
        page: (f32, f32),
    ) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let config = Arc::new(config.clone());
        let reserved = Reserved::of(&resolved_of(area, neighbours), &config);
        build_bar(
            area,
            Surround {
                config: &config,
                theme: NordTheme::new(),
                output: None,
                layer: LayerKind::Top,
                bounds: telar::Rect::new(0.0, 0.0, page.0, page.1),
                reserved,
                audience: ui::host::Audience::Owner,
            },
            modules,
        )
    }

    /// `area` and `neighbours` as the one-output arrangement [`Reserved::of`] reads its reservation from.
    fn resolved_of(area: &ResolvedArea, neighbours: &[ResolvedArea]) -> layout::Resolved {
        let mut areas = neighbours.to_vec();
        areas.push(area.clone());
        layout::Resolved::of("test", [(LayerKind::Top, layout::ResolvedLayer { areas })])
    }

    /// Where `area` actually lands on `page`, for a test measuring the air at its ends.
    fn strip(
        config: &Config,
        area: &ResolvedArea,
        neighbours: &[ResolvedArea],
        page: (f32, f32),
    ) -> telar::Rect {
        let config = Arc::new(config.clone());
        let ResolvedAreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            ..
        } = area.kind
        else {
            unreachable!("the helper builds a bar")
        };
        let reserved = Reserved::of(&resolved_of(area, neighbours), &config);
        let gap = bar_shape(&config, shape).gap as f32;
        let bounds = telar::Rect::new(0.0, 0.0, page.0, page.1);
        let run = run_of(edge, bounds, reserved, gap);
        strip_of(edge, thickness, length, offset, run, gap, bounds)
    }

    /// A bar area in the layout model's own words: `thickness` across `edge`, running the whole edge, holding whichever modules `zones` names in its start, center and end zones.
    fn bar_area(edge: Edge, thickness: f32, zones: [&[&str]; 3]) -> ResolvedArea {
        shaped_bar_area(edge, thickness, zones, BarShape::default(), None)
    }

    /// [`bar_area`] drawn in `shape`.
    fn bar_in(shape: BarShape, edge: Edge, thickness: f32, zones: [&[&str]; 3]) -> ResolvedArea {
        shaped_bar_area(edge, thickness, zones, shape, None)
    }

    /// A bar's own shape as a layout file writes it.
    fn shape_of(text: &str) -> BarShape {
        toml::from_str(text).expect("a bar shape")
    }

    /// [`bar_area`], with its shape and autohide named explicitly instead of defaulted.
    fn shaped_bar_area(
        edge: Edge,
        thickness: f32,
        zones: [&[&str]; 3],
        shape: BarShape,
        autohide: Option<AutoHide>,
    ) -> ResolvedArea {
        area_of(
            edge,
            thickness,
            shape,
            autohide,
            vec![
                zone_group("start", Zone::Start, zones[0]),
                zone_group("center", Zone::Center, zones[1]),
                zone_group("end", Zone::End, zones[2]),
            ],
        )
    }

    /// The bar area every other builder here assembles: an id, the geometry a bar needs, and whatever `groups` it was handed.
    fn area_of(
        edge: Edge,
        thickness: f32,
        shape: BarShape,
        autohide: Option<AutoHide>,
        groups: Vec<ResolvedGroup>,
    ) -> ResolvedArea {
        ResolvedArea {
            id: layout::AreaId::new(format!("bar-{}", edge.as_str())),
            kind: ResolvedAreaKind::Bar {
                edge,
                thickness,
                length: Extent::Fill,
                offset: 0.0,
                shape,
                autohide,
            },
            reserve: true,
            above_fullscreen: false,
            within: layout::Within::Output,
            style: layout::AreaStyle::default(),
            visible: None,
            actions: Default::default(),
            groups,
        }
    }

    fn zone_group(id: &str, zone: Zone, ids: &[&str]) -> ResolvedGroup {
        ResolvedGroup {
            id: layout::GroupId::new(id),
            kind: GroupKind::Zone { zone },
            stacked: false,
            repeat: None,
            children: ids.iter().map(|id| instance(id)).collect(),
        }
    }

    fn instance(id: &str) -> ResolvedInstance {
        ResolvedInstance {
            id: layout::InstanceId::new(id),
            module: id.to_string(),
            representation: layout::Representation::Chip,
            options: toml::Table::new(),
            bindings: std::collections::BTreeMap::new(),
            actions: std::collections::BTreeMap::new(),
        }
    }

    /// An instance carrying its own `variant` and `accent` options, which is where a chip's own look is read from.
    fn styled_instance(id: &str, variant: Variant, accent: &str) -> ResolvedInstance {
        let mut placed = instance(id);
        placed
            .options
            .insert("variant".into(), toml::Value::try_from(variant).unwrap());
        placed
            .options
            .insert("accent".into(), toml::Value::String(accent.to_string()));
        placed
    }

    /// A chip on a 32px bar along `edge`, dressed exactly as [`BarFrame::host`] dresses one.
    fn test_host(edge: Edge) -> Host {
        let theme = NordTheme::new();
        let config = Config::default();
        let shape = config.shape_from(None, None, None, None);
        Host::placed(
            Instance::of_module("dummy"),
            Arc::new(config),
            Representation::Chip,
            chip_extent(edge, 32.0),
            Some(edge),
            shape,
            theme.accent,
            theme.text,
            None,
        )
    }

    /// A [`Site`] for a chip's popout to hang off, with nothing behind it worth naming.
    fn test_site(edge: Edge) -> Site {
        Site {
            output: None,
            layer: LayerKind::Top,
            edge,
            chrome: ui::chrome::Chrome::global(Arc::new(Config::default()), None),
            gap: 8.0,
        }
    }

    fn module(id: &'static str, chip: ChipDef) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(chip),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    fn chip(build: ui::descriptor::Build) -> ChipDef {
        ChipDef::new(build, Input::ReadOnly)
    }

    fn with_popout(mut module: ModuleDescriptor) -> ModuleDescriptor {
        module.representations.popout = Some(CardDef {
            build: |_| ui::card::Card::titled("probe"),
            input: Input::ReadOnly,
        });
        module
    }

    fn dummy(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(20.0).height(20.0),
            |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
            vec![],
        )?))
    }

    /// A chip the width of a window title, next to which `dummy` is the same chip after the title got shorter.
    fn wide(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(320.0).height(20.0),
            |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
            vec![],
        )?))
    }

    thread_local! {
        static CENTRED: std::cell::RefCell<Option<telar::RwSignal<telar::Rect>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// `dummy`, publishing where it landed — the one chip a centring test needs to find again.
    fn centred(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let item = dummy(host)?;
        let rect = track_layout(item.layout_node()).expect("a container registers its rect");
        CENTRED.with(|c| *c.borrow_mut() = Some(rect));
        Ok(item)
    }

    thread_local! {
        static YIELDED: std::cell::RefCell<Option<telar::RwSignal<telar::Rect>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// A module whose own content will give up width if anything above it lets the pressure through, and says how much it kept — the probe for whether a chip's elasticity survives the wrappers around it.
    fn stretchy(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let item = StyledContainer::new(
            LayoutStyle::new()
                .width(320.0)
                .height(20.0)
                .flex_shrink(1.0)
                .min_width(0.0),
            |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
            vec![],
        )?;
        let rect = track_layout(item.layout_node()).expect("a container registers its rect");
        YIELDED.with(|y| *y.borrow_mut() = Some(rect));
        Ok(Box::new(item))
    }

    fn registry() -> Vec<ModuleDescriptor> {
        vec![
            module("dummy", chip(dummy)),
            module("wide", chip(wide)),
            module("centred", chip(centred)),
            // Both eliding modules carry a hover popout, so the wrapper is part of the path under test.
            with_popout(module("stretchy", chip(stretchy).elastic())),
            with_popout(module("rigid", chip(stretchy))),
            module("panics", chip(panics)),
            module("fails", chip(fails)),
        ]
    }

    fn panics(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        panic!("a chip that panics while it builds")
    }

    fn fails(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        Err(LayoutError::Engine("a chip that fails to build".into()))
    }

    thread_local! {
        static BUILT: std::cell::RefCell<Vec<&'static str>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn wanted(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        BUILT.with(|built| built.borrow_mut().push("wanted"));
        dummy(host)
    }

    fn unwanted(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
        BUILT.with(|built| built.borrow_mut().push("unwanted"));
        dummy(host)
    }

    /// A module no zone names costs nothing at all.
    ///
    /// The registry holds every module the shell ships, so a build that walked *it* rather than the zones would construct a chip nobody asked for — and constructing a chip is what subscribes it, which turns an unused module into a running service. That is the residency half of the rule, reached through the one door that makes it invisible: the widget tree, where an extra chip is off-screen rather than wrong.
    #[test]
    fn a_module_no_zone_names_is_never_built() {
        let registry = vec![
            module("wanted", chip(wanted)),
            module("unwanted", chip(unwanted)),
        ];

        reset_layout_runtime();
        set_theme(NordTheme::new());
        BUILT.with(|built| built.borrow_mut().clear());
        let config = config::Config::default();
        let area = bar_area(Edge::Top, 34.0, [&[], &["wanted"], &[]]);

        built(&config, &area, &registry, (600.0, 34.0)).expect("the bar builds");

        assert_eq!(
            BUILT.with(|built| built.borrow().clone()),
            vec!["wanted"],
            "only the module the config put on the bar was built"
        );
    }

    /// A chip that carries a hover popout is wrapped in an extra box to track the pointer. That box sits between the zone and the chip, so a press has to pass through it — and a wrapper that swallowed one would leave every popout-bearing chip (volume, brightness, media, mic, battery) looking dead to a click while still opening its card on hover.
    #[test]
    fn a_popout_wrapper_lets_a_click_through_to_the_chip() {
        use std::cell::Cell;
        use std::rc::Rc;
        use telar::{AvailableSpace, Event, PointerButton, PointerSource, compute_layout};

        let clicked = Rc::new(Cell::new(false));
        let sink = Rc::clone(&clicked);
        reset_layout_runtime();
        set_theme(NordTheme::new());
        let mut inner = Slots::new();
        inner.push(None, dummy(&test_host(Edge::Top)).unwrap());
        let chip = module_shell(
            ModuleShellProps::props()
                .variant(config::Variant::Default)
                .rest(Color::TRANSPARENT)
                .accent(NordTheme::new().accent)
                .radius(8.0)
                .square(true)
                .on_press(Some(Rc::new(move || sink.set(true)) as Rc<dyn Fn()>))
                .build(),
            telar::Children::new({
                let inner = std::cell::RefCell::new(Some(inner));
                move || {
                    inner
                        .borrow_mut()
                        .take()
                        .ok_or_else(|| LayoutError::Engine("chip children built twice".into()))
                }
            }),
        )
        .unwrap();
        let mut wrapped = chip_wrapper(
            chip,
            None,
            Some((Instance::of_module("volume"), test_site(Edge::Top))),
            Wrapper {
                cross: AlignItems::STRETCH,
                edge: Edge::Top,
                elastic: false,
                fill: Color::TRANSPARENT,
                radius: 0.0,
            },
            &Bound::default(),
        )
        .unwrap();

        let node = wrapped.layout_node();
        compute_layout(
            node,
            AvailableSpace::Definite(200.0),
            AvailableSpace::Definite(32.0),
        )
        .unwrap();

        let (x, y) = (10.0, 10.0);
        wrapped.on_event(&Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        wrapped.on_event(&Event::PointerReleased {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        assert!(clicked.get(), "the chip's own press handler never fired");
    }

    thread_local! {
        static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// A misspelt module used to vanish, and the bar closed up around the gap as if it had never been asked for; a chip whose build failed or panicked took the whole bar down. Either is drawn where it was declared instead — between the chips it was written between, as thick as they are, naming itself where there is room to — and a press on it lands on it and opens the settings window rather than falling through to whatever is under the bar. Every shape, down a vertical bar as well as across a horizontal one.
    #[test]
    fn an_unknown_or_failing_module_holds_its_place_between_its_neighbours() {
        use telar::{DrawCommand, Event, Paint, PointerButton, PointerSource, Rect};

        let theme = NordTheme::new();
        ui::module::set_panel_opener(|panel| {
            OPENED.with(|opened| opened.borrow_mut().push(panel.to_string()))
        });
        for broken in ["nope", "panics", "fails"] {
            for (edge, mode) in [Edge::Top, Edge::Left]
                .into_iter()
                .flat_map(|edge| ["bar", "sections", "chips"].map(|mode| (edge, mode)))
            {
                reset_layout_runtime();
                set_theme(theme);
                OPENED.with(|opened| opened.borrow_mut().clear());
                let cfg = Config::default();
                let area = area_of(
                    edge,
                    32.0,
                    shape_of(&format!("mode=\"{mode}\"\n")),
                    None,
                    vec![ResolvedGroup {
                        id: layout::GroupId::new("start"),
                        kind: GroupKind::Zone { zone: Zone::Start },
                        stacked: false,
                        repeat: None,
                        children: vec![
                            styled_instance("dummy", config::Variant::Filled, "green"),
                            instance(broken),
                            styled_instance("dummy", config::Variant::Filled, "purple"),
                        ],
                    }],
                );
                let (w, h) = if edge.is_horizontal() {
                    (600.0, 32.0)
                } else {
                    (32.0, 600.0)
                };
                let bar = built(&cfg, &area, &registry(), (w, h)).expect("the bar builds");
                let page =
                    Container::new(axis(LayoutStyle::new(), edge).width(w).height(h), vec![bar])
                        .unwrap();
                let root = page.layout_node();
                let mut tree = telar::ComponentList::new(page);
                compute_layout(
                    root,
                    AvailableSpace::Definite(w),
                    AvailableSpace::Definite(h),
                )
                .unwrap();

                let rect_of = |fill: Color| {
                    tree.commands().iter().find_map(|command| match command {
                        DrawCommand::Rect { rect, style, .. }
                            if style.fill == Some(Paint::Solid(fill)) =>
                        {
                            Some(*rect)
                        }
                        _ => None,
                    })
                };
                let along = |r: Rect| {
                    if edge.is_horizontal() {
                        (r.x, r.width)
                    } else {
                        (r.y, r.height)
                    }
                };
                let across = |r: Rect| {
                    if edge.is_horizontal() {
                        r.height
                    } else {
                        r.width
                    }
                };
                let before = rect_of(theme.green).expect("the chip before it draws");
                let after = rect_of(theme.purple).expect("the chip after it draws");
                let placeholder = rect_of(ui::placeholder::fill(theme)).unwrap_or_else(|| {
                    panic!("{broken}/{edge:?}/{mode}: the placeholder put nothing on the bar")
                });

                assert!(
                    along(before).0 < along(placeholder).0 && along(placeholder).0 < along(after).0,
                    "{broken}/{edge:?}/{mode}: the placeholder is at {:?}, not between the chips at {:?} and {:?} it was declared \
                     between",
                    along(placeholder),
                    along(before),
                    along(after)
                );
                assert_eq!(
                    across(placeholder),
                    across(before),
                    "{broken}/{edge:?}/{mode}: the placeholder is not as thick as the chip beside it"
                );
                assert!(
                    along(placeholder).1 >= test_host(edge).icon_size(),
                    "{broken}/{edge:?}/{mode}: the placeholder is {}px long, too short to hold its own glyph",
                    along(placeholder).1
                );
                let named = tree
                    .commands()
                    .iter()
                    .any(|command| matches!(command, DrawCommand::Text { text, .. } if &**text == broken));
                assert_eq!(
                    named,
                    edge.is_horizontal(),
                    "{broken}/{edge:?}/{mode}: the id is written along a horizontal bar, and only there — down a vertical one \
                     there is no length to write it along"
                );

                let (x, y) = (
                    f64::from(placeholder.x + placeholder.width / 2.0),
                    f64::from(placeholder.y + placeholder.height / 2.0),
                );
                for event in [
                    Event::PointerPressed {
                        x,
                        y,
                        button: PointerButton::Primary,
                        source: PointerSource::Mouse,
                    },
                    Event::PointerReleased {
                        x,
                        y,
                        button: PointerButton::Primary,
                        source: PointerSource::Mouse,
                    },
                ] {
                    tree.on_event(&event);
                }
                assert_eq!(
                    OPENED.with(|opened| opened.borrow().clone()),
                    ["settings"],
                    "{broken}/{edge:?}/{mode}: a press on the placeholder has to land on it and open the settings window"
                );
            }
        }
    }

    /// F-10.38 on a bar: the area's `fill` is the strip's colour in every mode and its `opacity` stands in for `[theme] opacity` on everything the bar paints — with the strip the one rect that carries it, rounded by the bar's own shape rather than by the `style.radius` beside it.
    #[test]
    fn a_bars_own_style_is_its_strip_painted_once() {
        let theme = NordTheme::new();
        let green = Color::from_hex("#00ff00").unwrap();
        for mode in ["bar", "sections", "chips"] {
            reset_layout_runtime();
            set_theme(theme);
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\nradius=6\n"));
            let mut area = bar_in(shape, Edge::Top, 32.0, [&["dummy"], &[], &[]]);
            area.style = AreaStyle {
                fill: Some("#00ff00".to_string()),
                radius: Some(Corners::all(9.0)),
                opacity: Some(0.4),
                padding: Some(3.0),
                backdrop: None,
            };
            let bar = built(&cfg, &area, &registry(), (400.0, 32.0)).expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().flex_row().width(400.0).height(32.0),
                vec![bar],
            )
            .unwrap();
            let root = page.layout_node();
            let tree = telar::ComponentList::new(page);
            compute_layout(
                root,
                AvailableSpace::Definite(400.0),
                AvailableSpace::Definite(32.0),
            )
            .unwrap();

            let painted = |fill: Color| -> Vec<(telar::Rect, BorderRadius)> {
                tree.commands()
                    .iter()
                    .filter_map(|command| match command {
                        telar::DrawCommand::Rect { rect, style }
                            if style.fill == Some(telar::Paint::Solid(fill)) =>
                        {
                            Some((*rect, style.radius))
                        }
                        _ => None,
                    })
                    .collect()
            };
            assert_eq!(
                painted(green.with_alpha(0.4)),
                [(
                    telar::Rect::new(0.0, 0.0, 400.0, 32.0),
                    BorderRadius::all(6.0)
                )],
                "{mode}: the strip is the one rect in the area's fill, at its opacity, rounded by `shape.radius`"
            );
            if mode == "chips" {
                assert!(
                    !painted(theme.surface.with_alpha(0.4)).is_empty(),
                    "{mode}: a resting chip is painted at the bar's own opacity too"
                );
            }
        }
    }

    #[test]
    fn a_self_managed_module_rests_on_a_chip_like_its_neighbours() {
        let surfaces = |mode: &str, def: ChipDef| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let registry = vec![module("probe", def)];
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\n"));
            let area = bar_in(shape, Edge::Top, 32.0, [&[], &["probe"], &[]]);
            let surface = telar::Paint::Solid(inner_fill(
                &cfg,
                Dress::of(&cfg, &AreaStyle::default(), &NordTheme::new()),
                NordTheme::new().surface,
            ));
            let bar = built(&cfg, &area, &registry, (400.0, 32.0)).expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().flex_row().width(400.0).height(32.0),
                vec![bar],
            )
            .unwrap();
            let root = page.layout_node();
            let tree = telar::ComponentList::new(page);
            compute_layout(
                root,
                AvailableSpace::Definite(400.0),
                AvailableSpace::Definite(32.0),
            )
            .unwrap();
            tree.commands()
                .iter()
                .filter(|command| {
                    matches!(command, telar::DrawCommand::Rect { style, .. } if style.fill == Some(surface))
                })
                .count()
        };

        assert_eq!(
            surfaces("chips", chip(dummy)),
            1,
            "a plain chip rests on the chip bar's surface"
        );
        assert_eq!(
            surfaces("chips", chip(dummy).self_managed()),
            1,
            "a self-managed module on a chip bar sat on nothing — workspaces, tray and lockstatus floated bare between \
             their neighbours' pills"
        );
        assert_eq!(
            surfaces("chips", chip(dummy).filler()),
            0,
            "a filler is a gap, and a gap with a pill behind it is a chip with nothing in it"
        );
        assert_eq!(
            surfaces("bar", chip(dummy).self_managed()),
            0,
            "a whole bar is the surface, so nothing on it rests on one of its own"
        );
    }

    #[test]
    fn every_mode_builds_a_tree() {
        for mode in ["bar", "sections", "chips"] {
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\ngap=6\nradius=10\nspacing=8\n"));
            let area = bar_in(shape, Edge::Top, 32.0, [&["dummy"], &["dummy"], &["dummy"]]);
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let bar = built(&cfg, &area, &registry(), (1920.0, 32.0));
            assert!(bar.is_ok(), "mode {mode} builds a tree");
        }
    }

    /// The centre zone is centred on the bar, whatever the chips beside it are doing.
    ///
    /// The regression is the one thing a bar cannot get away with: all three zones took their content width plus an equal share of the slack, so the centre sat on the middle of the *leftover* space. Every chip that changes width slid it — and the window title changes width on every focus change, which walked the clock and the launcher sideways all day.
    #[test]
    fn the_centre_holds_still_while_a_side_chip_changes_width() {
        const BAR: f32 = 1920.0;

        let midpoint = |start: &[&str], mode: &str| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\nspacing=8\n"));
            let area = bar_in(shape, Edge::Top, 32.0, [start, &["centred"], &["dummy"]]);
            let bar = built(&cfg, &area, &registry(), (BAR, 32.0)).expect("the bar builds");
            compute_layout(
                bar.layout_node(),
                AvailableSpace::Definite(BAR),
                AvailableSpace::Definite(32.0),
            )
            .expect("the bar lays out");
            let rect = CENTRED
                .with(|c| *c.borrow())
                .expect("the centre chip published its rect")
                .get();
            rect.x + rect.width / 2.0
        };

        for mode in ["bar", "sections", "chips"] {
            let narrow = midpoint(&["dummy"], mode);
            let widened = midpoint(&["wide"], mode);
            assert_eq!(
                narrow,
                BAR / 2.0,
                "{mode}: the centre chip sits at {narrow}, not on the middle of a {BAR}px bar"
            );
            assert_eq!(
                widened, narrow,
                "{mode}: growing a start chip by 300px moved the centre chip from {narrow} to {widened} — \
                 the window title would drag the whole centre section around with it"
            );
            // Four 320px chips want 1304px of a 956px half. A zone free to claim its content would shove the centre 350px sideways; this one is cut off at the centre's edge instead.
            let overrun = midpoint(&["wide", "wide", "wide", "wide"], mode);
            assert_eq!(
                overrun, narrow,
                "{mode}: a start zone holding more than fits still moved the centre to {overrun} — it has to \
                 be cut where the centre begins, not push it out of the way"
            );
        }
    }

    /// What is cut off stops a chip's width of air short of the centre, on both sides.
    ///
    /// Measured on a bar whose own layout puts nothing between the zones (`bar` mode), which is the case with no gap to hide behind: a slice that ends flush against the centre's first chip reads as one wide chip with a seam down it rather than as a chip that ran out of room.
    #[test]
    fn a_cut_side_stops_a_gap_short_of_the_centre() {
        const BAR: f32 = 1920.0;
        const SPACING: f32 = 8.0;

        // Both axes: a vertical bar's zones run down it, so the air belongs on their top and bottom edges, where `margin_start`/`margin_end` would have put it on the sides — across a bar that has no room to spare and nothing there to separate.
        for edge in [Edge::Top, Edge::Left] {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let host = test_host(edge);
            let overrun = || -> Vec<ChildSlot> {
                (0..4)
                    .map(|_| ChildSlot::stat(wide(&host).expect("a chip builds")))
                    .collect()
            };

            let start_zone = zone(
                edge,
                Zone::Start,
                SPACING,
                (0.0, 0.0),
                AlignItems::STRETCH,
                overrun(),
            )
            .unwrap();
            let start_rect = track_layout(start_zone.layout_node()).expect("the zone registers");
            let centre_chip = dummy(&host).expect("a chip builds");
            let centre_rect = track_layout(centre_chip.layout_node()).expect("the chip registers");
            let centre_zone = zone(
                edge,
                Zone::Center,
                SPACING,
                (0.0, 0.0),
                AlignItems::STRETCH,
                vec![ChildSlot::stat(centre_chip)],
            )
            .unwrap();
            let end_zone = zone(
                edge,
                Zone::End,
                SPACING,
                (0.0, 0.0),
                AlignItems::STRETCH,
                overrun(),
            )
            .unwrap();
            let end_rect = track_layout(end_zone.layout_node()).expect("the zone registers");
            let (w, h) = if edge.is_horizontal() {
                (BAR, 32.0)
            } else {
                (32.0, BAR)
            };
            let root = Container::new(
                axis(LayoutStyle::new(), edge).width(w).height(h),
                vec![start_zone, centre_zone, end_zone],
            )
            .unwrap();
            compute_layout(
                root.layout_node(),
                AvailableSpace::Definite(w),
                AvailableSpace::Definite(h),
            )
            .unwrap();

            // Along the bar, whichever way it runs.
            let along = |r: telar::Rect| {
                if edge.is_horizontal() {
                    (r.x, r.width)
                } else {
                    (r.y, r.height)
                }
            };
            let (centre_at, centre_len) = along(centre_rect.get());
            let (start_at, start_len) = along(start_rect.get());
            let (end_at, _) = along(end_rect.get());
            assert!(
                centre_at - (start_at + start_len) >= SPACING,
                "{edge:?}: the start side is cut {}px before the centre chip, not the {SPACING}px a chip \
                 is given",
                centre_at - (start_at + start_len)
            );
            assert!(
                end_at - (centre_at + centre_len) >= SPACING,
                "{edge:?}: and the end side starts {}px after it",
                end_at - (centre_at + centre_len)
            );
        }
    }

    #[test]
    fn a_vertical_bar_keeps_its_end_chips_off_the_bars_it_runs_into() {
        const LENGTH: f32 = 1000.0;
        const SPACING: f32 = 8.0;

        let above = || bar_area(Edge::Top, 30.0, [&[], &["dummy"], &[]]);
        let below = || bar_area(Edge::Bottom, 30.0, [&[], &["dummy"], &[]]);

        // The chip's rect and the strip the bar landed on, both in the screen's own coordinates: a vertical bar stops where the bars above and below it left it, so its own middle is no longer the screen's.
        let probe = |mode: &str, neighbours: &[ResolvedArea], zones: [&[&str]; 3]| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\nspacing=8\n"));
            let area = bar_in(shape, Edge::Left, 32.0, zones);
            let bar = built_amid(&cfg, &area, neighbours, &registry(), (32.0, LENGTH))
                .expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().flex_column().width(32.0).height(LENGTH),
                vec![bar],
            )
            .expect("a screen to stand the bar on");
            compute_layout(
                page.layout_node(),
                AvailableSpace::Definite(32.0),
                AvailableSpace::Definite(LENGTH),
            )
            .expect("the bar lays out");
            let chip = CENTRED
                .with(|c| *c.borrow())
                .expect("the probe published its rect")
                .get();
            (chip, strip(&cfg, &area, neighbours, (32.0, LENGTH)))
        };
        let air = |mode: &str, neighbours: &[ResolvedArea]| {
            let (first, strip) = probe(mode, neighbours, [&["centred"], &[], &[]]);
            let (last, _) = probe(mode, neighbours, [&[], &[], &["centred"]]);
            (
                first.y - strip.y,
                (strip.y + strip.height) - (last.y + last.height),
            )
        };

        let boxed_in = vec![above(), below()];
        let in_bar_mode = air("bar", &boxed_in);
        for mode in ["bar", "sections", "chips"] {
            let (lead, trail) = air(mode, &boxed_in);
            assert!(
                lead >= SPACING && trail >= SPACING,
                "{mode}: the end chips sit {lead}px and {trail}px off the bars above and below — flush against \
                 them rather than a chip's worth away"
            );
            assert_eq!(
                (lead, trail),
                in_bar_mode,
                "{mode}: the air at the ends cannot depend on the mode"
            );
            let (bare_lead, bare_trail) = air(mode, &[]);
            assert!(
                bare_lead < lead && bare_trail < trail,
                "{mode}: with no bar above or below, the end chips still hold {bare_lead}px and {bare_trail}px \
                 off the screen's edges"
            );

            let (centre, strip) = probe(mode, &[above()], [&["dummy"], &["centred"], &[]]);
            assert_eq!(
                centre.y + centre.height / 2.0,
                strip.y + strip.height / 2.0,
                "{mode}: air owed at the top end only moved the centre off the middle of the bar"
            );
        }
    }

    /// The elastic chip's give reaches it through everything the bar wraps it in.
    ///
    /// Elasticity is a property of a chain: the zone, the popout wrapper, the chip shell and the label all have to agree to give, and any one of them holding firm makes the whole thing rigid while every part of it still looks right on its own. The wrapper was exactly that — `flex-shrink: 0`, and both modules whose label elides carry a popout, so it would have made the elide unreachable in the shell while the chip's own test passed.
    #[test]
    fn an_elastic_chip_gives_way_through_the_wrappers_around_it() {
        let kept = |module: &str| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg = Config::default();
            let shape = shape_of("mode=\"chips\"\nspacing=8\n");
            let area = bar_in(shape, Edge::Top, 32.0, [&["wide", module], &["dummy"], &[]]);
            let bar = built(&cfg, &area, &registry(), (600.0, 32.0)).expect("the bar builds");
            compute_layout(
                bar.layout_node(),
                AvailableSpace::Definite(600.0),
                AvailableSpace::Definite(32.0),
            )
            .expect("the bar lays out");
            YIELDED
                .with(|y| *y.borrow())
                .expect("the module published its rect")
                .get()
                .width
        };

        assert!(
            kept("stretchy") < 320.0,
            "the elastic module kept all {}px of its width in a zone with room for half that — something \
             above it refused to pass the pressure down, and its label will never elide",
            kept("stretchy")
        );
        assert_eq!(
            kept("rigid"),
            320.0,
            "and a module that never asked to be elastic must keep every pixel of its width"
        );
    }

    /// A module that draws past its zone is cut by the zone, and knows nothing about it.
    ///
    /// The point is where the rule lives. A self-managed module lays itself out — `workspaces` paints a column of pills whose length is the number of workspaces and the icons on them — and it has no idea what else is on the bar or where the centre begins. So the cut cannot be its job, or it would be every module's job: the zone it sits in publishes a clip, and whatever runs past it stops being drawn. This module is the awkward shape that proves it — self-managed *and* scrollable, so it reaches the zone through the wrapper rather than directly, exactly as `workspaces` does.
    #[test]
    fn a_module_that_overruns_its_zone_is_cut_by_the_zone_and_never_by_itself() {
        const RUN: f32 = 900.0;
        const BAR: f32 = 400.0;

        fn overrunner(_host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
            Ok(Box::new(StyledContainer::new(
                LayoutStyle::new().width(32.0).height(RUN).flex_shrink(0.0),
                |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
                vec![],
            )?))
        }
        fn nudge(_host: &Host, _dx: f32, _dy: f32) {}

        reset_layout_runtime();
        set_theme(NordTheme::new());
        let registry = vec![
            module("dummy", chip(dummy)),
            module(
                "overrunner",
                chip(overrunner).self_managed().on_scroll(nudge),
            ),
        ];
        let cfg = Config::default();
        let shape = shape_of("mode=\"bar\"\nspacing=8\n");
        let area = bar_in(shape, Edge::Left, 32.0, [&["overrunner"], &["dummy"], &[]]);
        let bar = built(&cfg, &area, &registry, (32.0, BAR)).expect("the bar builds");
        let page = Container::new(
            LayoutStyle::new().flex_column().width(32.0).height(BAR),
            vec![bar],
        )
        .unwrap();
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(32.0),
            AvailableSpace::Definite(BAR),
        )
        .unwrap();

        // The strip's own draw is honest about its size; what makes it stop at the zone is the clip around it.
        let commands = tree.commands().to_vec();
        let mut depth = 0usize;
        let mut clipped_run = None;
        let mut clip_end = None;
        for command in &commands {
            match command {
                telar::DrawCommand::PushClip { rect, .. } => {
                    depth += 1;
                    clip_end = Some(rect.y + rect.height);
                }
                telar::DrawCommand::PopClip => depth -= 1,
                telar::DrawCommand::Rect { rect, .. } if rect.height == RUN => {
                    clipped_run = Some((depth, rect.y + rect.height));
                }
                _ => {}
            }
        }
        let (depth_at_run, run_end) = clipped_run.expect("the overrunning module drew its strip");
        assert!(
            depth_at_run > 0,
            "a {RUN}px module on a {BAR}px bar drew outside every clip — nothing stops it painting over the \
             centre, and each module would have to bound itself"
        );
        let end = clip_end.expect("the zone published a clip");
        assert!(
            end < run_end,
            "the clip around it ends at {end}, past the {run_end} the strip reaches — it cuts nothing"
        );
    }

    /// The air at a cut is the same in every mode.
    ///
    /// The zone-level test above pins it to one `spacing`; this one is about the two mechanisms that were both trying to provide it. `sections` and `chips` laid their zones out with a gap between them, and the sides hold one of their own — so those two modes opened twice the air at exactly the place a side is cut, a visible hole where every other join on the bar has one chip's worth. `bar` mode, with no gap of its own, looked right the whole time, which is what kept it hidden.
    #[test]
    fn the_air_at_a_cut_does_not_depend_on_the_mode() {
        const BAR: f32 = 1920.0;

        let gap_before_centre = |mode: &str| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\nspacing=8\n"));
            let area = bar_in(
                shape,
                Edge::Top,
                32.0,
                [
                    &["wide", "wide", "wide", "wide"],
                    &["centred"],
                    &["wide", "wide", "wide", "wide"],
                ],
            );
            let bar = built(&cfg, &area, &registry(), (BAR, 32.0)).expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().flex_row().width(BAR).height(32.0),
                vec![bar],
            )
            .unwrap();
            let root = page.layout_node();
            let tree = telar::ComponentList::new(page);
            compute_layout(
                root,
                AvailableSpace::Definite(BAR),
                AvailableSpace::Definite(32.0),
            )
            .unwrap();

            // The only two clips on a bar are its two sides; the start zone's is the one that ends first.
            let cut = tree
                .commands()
                .iter()
                .filter_map(|c| match c {
                    telar::DrawCommand::PushClip { rect, .. } => Some(rect.x + rect.width),
                    _ => None,
                })
                .fold(f32::INFINITY, f32::min);
            let centre = CENTRED
                .with(|c| *c.borrow())
                .expect("the centre chip published its rect")
                .get();
            centre.x - cut
        };

        let (bar, sections, chips) = (
            gap_before_centre("bar"),
            gap_before_centre("sections"),
            gap_before_centre("chips"),
        );
        assert_eq!(
            (sections, chips),
            (bar, bar),
            "a side is cut {bar}px before the centre in `bar` mode, {sections} in `sections` and {chips} in \
             `chips` — the same join cannot be three different distances"
        );
    }

    /// A section's panel is never longer than the zone it sits in.
    ///
    /// It used to be sized from its content, so a zone holding more chips than fit drew its panel past the cut and had it clipped square there: the section ended in a flat grey stub with nothing on it — the chip that stub belonged to being off past the boundary — which reads as a hole in the bar rather than as a section that ran out of room. The clip was doing its job; the panel was lying about its length.
    #[test]
    fn a_section_panel_is_no_longer_than_the_zone_it_fills() {
        const BAR: f32 = 1920.0;

        reset_layout_runtime();
        set_theme(NordTheme::new());
        let cfg = Config::default();
        let shape = shape_of("mode=\"sections\"\nspacing=8\nradius=8\n");
        let area = bar_in(
            shape,
            Edge::Top,
            32.0,
            [&["wide", "wide", "wide", "wide"], &["dummy"], &[]],
        );
        let bar = built(&cfg, &area, &registry(), (BAR, 32.0)).expect("the bar builds");
        let page = Container::new(
            LayoutStyle::new().flex_row().width(BAR).height(32.0),
            vec![bar],
        )
        .unwrap();
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(BAR),
            AvailableSpace::Definite(32.0),
        )
        .unwrap();

        // The panel is the zone's only child, so it is the first thing drawn inside the zone's clip; element markers draw nothing and are skipped.
        let commands: Vec<_> = tree
            .commands()
            .iter()
            .filter(|command| {
                !matches!(
                    command,
                    telar::DrawCommand::PushElement { .. } | telar::DrawCommand::PopElement
                )
            })
            .cloned()
            .collect();
        let (clip, panel) = commands
            .iter()
            .zip(commands.iter().skip(1))
            .find_map(|(clip, next)| match (clip, next) {
                (
                    telar::DrawCommand::PushClip { rect: clip, .. },
                    telar::DrawCommand::Rect { rect, .. },
                ) => Some((*clip, *rect)),
                _ => None,
            })
            .expect("the start zone drew its panel inside its clip");
        assert!(
            panel.width <= clip.width,
            "the panel is {}px wide in a {}px zone — the part past the cut is a stub of empty surface",
            panel.width,
            clip.width
        );
    }

    /// DEC-18 took `[shape]` away as a fallback, so the built-in layout carries what those defaults gave: the shipped bar is the one solid, edge-hugging strip a fresh install always drew, rounded and spaced by the theme — and a config still writing the retired keys changes nothing about it.
    #[test]
    fn the_built_in_bar_keeps_the_shape_the_old_defaults_gave() {
        let theme = Config::starter().resolve_theme();
        let top = layout::built_in()
            .outputs
            .into_iter()
            .flat_map(|rule| rule.layers.top.areas)
            .find_map(|area| match area.kind {
                Some(layout::AreaKind::Bar { shape, .. }) => Some(shape),
                _ => None,
            })
            .expect("the built-in layout has a bar");
        for config in [
            Config::starter(),
            toml::from_str("[shape]\nmode = \"chips\"\ngap = 12\nspacing = 30\nradius = 20\n")
                .expect("a config with the retired keys still parses"),
        ] {
            let shape = bar_shape(&config, top);
            assert_eq!(
                (shape.mode, shape.gap, shape.spacing, shape.radius),
                (Shape::Bar, 0, theme.spacing, theme.radius)
            );
            assert_eq!(config.gap_of(&shape), 0, "it hugs its edge");
        }
    }

    #[test]
    fn center_only_sections_builds_a_notch() {
        let cfg = Config::default();
        let shape = shape_of("mode=\"sections\"\ngap=8\nradius=12\n");
        let area = bar_in(shape, Edge::Top, 34.0, [&[], &["dummy"], &[]]);
        reset_layout_runtime();
        set_theme(NordTheme::new());
        assert!(built(&cfg, &area, &registry(), (1920.0, 34.0)).is_ok());
    }

    #[test]
    fn vertical_bar_builds_in_every_mode() {
        for mode in ["bar", "sections", "chips"] {
            let cfg = Config::default();
            let shape = shape_of(&format!("mode=\"{mode}\"\nradius=8\n"));
            let area = bar_in(shape, Edge::Left, 44.0, [&["dummy"], &[], &["dummy"]]);
            reset_layout_runtime();
            set_theme(NordTheme::new());
            assert!(
                built(&cfg, &area, &registry(), (44.0, 1080.0)).is_ok(),
                "vertical {mode} builds"
            );
        }
    }

    /// Every chip fills the bar's thickness, whichever edge it is on.
    ///
    /// The regression: the wrapper a popout-bearing chip sits in was fixed to a row, so on a left or right bar it stretched the chip's height (already its content) and left the width free. Each wrapped chip then took its own content width, ragged against the bar's inner edge, and the wide ones ran off the screen. Building proves none of that — the wrapper builds happily either way — so this lays a real bar out and measures the chips.
    #[test]
    fn a_wrapped_chip_fills_a_vertical_bar_across() {
        let side = 44.0;
        // A chip wider than the bar is the case that shows the bug: a clock, a window title, a netspeed readout. Its own width is content-driven, so only `align-items: stretch` on the right axis reins it in.
        let content_width = 120.0;

        for edge in [Edge::Left, Edge::Top] {
            reset_layout_runtime();
            set_theme(NordTheme::new());

            let inner =
                Container::new(LayoutStyle::new().width(content_width).height(20.0), vec![])
                    .unwrap();
            let chip =
                Container::new(LayoutStyle::new().flex_row(), vec![Box::new(inner)]).unwrap();
            let chip_node = chip.layout_node();
            let chip_rect = track_layout(chip_node).expect("the chip registers its rect");

            let wrapped = chip_wrapper(
                Box::new(chip),
                None,
                Some((Instance::of_module("clock"), test_site(edge))),
                Wrapper {
                    cross: AlignItems::STRETCH,
                    edge,
                    elastic: false,
                    fill: Color::TRANSPARENT,
                    radius: 0.0,
                },
                &Bound::default(),
            )
            .unwrap();
            // The zone the bar puts a chip in: along the bar, stretching its children across it.
            let zone = zone(
                edge,
                Zone::Start,
                0.0,
                (0.0, 0.0),
                AlignItems::STRETCH,
                vec![ChildSlot::stat(wrapped)],
            )
            .unwrap();
            let (w, h) = if edge.is_vertical() {
                (side, 600.0)
            } else {
                (600.0, side)
            };
            compute_layout(
                zone.layout_node(),
                AvailableSpace::Definite(w),
                AvailableSpace::Definite(h),
            )
            .unwrap();

            let rect = chip_rect.get();
            let across = if edge.is_vertical() {
                rect.width
            } else {
                rect.height
            };
            assert_eq!(
                across, side,
                "{edge:?}: a {content_width}px chip measures {across} across a {side}px bar — it should be \
                 reined in to the bar's thickness, not left at its content width to spill off the screen"
            );
        }
    }
    /// **A hidden bar hands the compositor its peek strip.**
    ///
    /// Autohide's contract on all four edges: the bar is off its edge but for `peek`, and what the window claims along that edge is what is left on screen — so a press anywhere else reaches the application underneath rather than a bar nobody can see. Asserted through `interactive_rects`, which is what the driver actually hands the compositor, rather than through the transform, which would be restating the arithmetic.
    ///
    /// **It asks about the bar's own chrome, not about every claim.** A chip inside a hidden vertical bar still overshoots the strip by a couple of pixels (F-10.30); narrowing the question is what keeps this test guarding the part that works instead of failing for a reason it does not describe.
    #[test]
    fn a_hidden_bar_claims_its_peek_strip_and_no_more() {
        const PEEK: f32 = 1.0;
        const SIZE: f32 = 32.0;
        const SCREEN: (f32, f32) = (600.0, 600.0);

        for (edge, on_hover) in Edge::ALL
            .into_iter()
            .flat_map(|edge| [(edge, true), (edge, false)])
        {
            telar::reset_layout_runtime();
            set_theme(NordTheme::new());
            let _scope = telar::owner_scope();
            let cfg = Config::default();
            let area = shaped_bar_area(
                edge,
                SIZE,
                [&[], &["dummy"], &[]],
                BarShape::default(),
                Some(layout::AutoHide {
                    peek: PEEK,
                    on_hover,
                }),
            );

            let bar = built(&cfg, &area, &registry(), SCREEN).expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().width(SCREEN.0).height(SCREEN.1),
                vec![bar],
            )
            .expect("a screen to stand the bar on");
            let root = page.layout_node();
            let _tree = telar::ComponentList::new(page);
            telar::compute_layout(
                root,
                telar::AvailableSpace::Definite(SCREEN.0),
                telar::AvailableSpace::Definite(SCREEN.1),
            )
            .expect("the bar lays out");

            let on_screen = telar::Rect::new(0.0, 0.0, SCREEN.0, SCREEN.1);
            let claimed: Vec<telar::Rect> = telar::interactive_rects()
                .into_iter()
                .filter_map(|rect| rect.intersect(on_screen))
                .filter(|rect| rect.width > 0.5 && rect.height > 0.5)
                .collect();

            let (across, along) = match edge.is_horizontal() {
                true => (
                    (|rect: &telar::Rect| rect.height) as fn(&telar::Rect) -> f32,
                    (|rect: &telar::Rect| rect.width) as fn(&telar::Rect) -> f32,
                ),
                false => (
                    (|rect: &telar::Rect| rect.width) as fn(&telar::Rect) -> f32,
                    (|rect: &telar::Rect| rect.height) as fn(&telar::Rect) -> f32,
                ),
            };
            // The bar's own chrome is the claim that runs the whole edge; the chips inside it are shorter.
            let strip = claimed
                .iter()
                .find(|rect| along(rect) > SCREEN.0 / 2.0)
                .unwrap_or_else(|| {
                    panic!("{edge:?}: nothing claims the strip that brings the bar back")
                });
            assert!(
                across(strip) <= PEEK + 0.5,
                "{edge:?}: the hidden bar claims {}px of the screen, where only its {PEEK}px peek is on it",
                across(strip)
            );
            for claim in &claimed {
                assert!(
                    across(claim) <= PEEK + 0.5,
                    "{edge:?}: {claim:?} reaches past the {PEEK}px peek, which is all of the bar left on screen (F-10.30)"
                );
            }
        }
    }

    /// **A bar that hovering does not bring back comes back for a pull** (F-10.30): pressed on its peek strip and pulled inward across its edge it slides on screen and stays while the pointer is over it; resting on the strip, or sliding along it, brings nothing back. On all four edges, read through `interactive_rects` as the hidden bar's own test reads it.
    #[test]
    fn a_bar_shown_only_by_a_pull_comes_back_for_a_pull_inward_and_for_nothing_else() {
        const PEEK: f32 = 2.0;
        const SIZE: f32 = 32.0;
        const SCREEN: (f32, f32) = (600.0, 600.0);
        let mouse = || telar::PointerSource::Mouse;
        let at = |edge: Edge, inward: f32, along: f32| -> (f64, f64) {
            let (x, y) = match edge {
                Edge::Top => (along, inward),
                Edge::Bottom => (along, SCREEN.1 - inward),
                Edge::Left => (inward, along),
                Edge::Right => (SCREEN.0 - inward, along),
            };
            (f64::from(x), f64::from(y))
        };

        for edge in Edge::ALL {
            telar::reset_layout_runtime();
            set_theme(NordTheme::new());
            let _scope = telar::owner_scope();
            let area = shaped_bar_area(
                edge,
                SIZE,
                [&[], &["dummy"], &[]],
                BarShape::default(),
                Some(layout::AutoHide {
                    peek: PEEK,
                    on_hover: false,
                }),
            );
            let bar =
                built(&Config::default(), &area, &registry(), SCREEN).expect("the bar builds");
            let page = Container::new(
                LayoutStyle::new().width(SCREEN.0).height(SCREEN.1),
                vec![bar],
            )
            .expect("a screen to stand the bar on");
            let root = page.layout_node();
            let mut tree = telar::ComponentList::new(page);
            let settle = || {
                // An animation's first tick only starts its clock.
                let now = std::time::Instant::now();
                telar::motion::tick(now);
                telar::motion::tick(now + std::time::Duration::from_secs(5));
                telar::compute_layout(
                    root,
                    telar::AvailableSpace::Definite(SCREEN.0),
                    telar::AvailableSpace::Definite(SCREEN.1),
                )
                .expect("the bar lays out");
            };
            let deepest = || {
                let on_screen = telar::Rect::new(0.0, 0.0, SCREEN.0, SCREEN.1);
                telar::interactive_rects()
                    .into_iter()
                    .filter_map(|rect| rect.intersect(on_screen))
                    .map(|rect| match edge.is_horizontal() {
                        true => rect.height,
                        false => rect.width,
                    })
                    .fold(0.0, f32::max)
            };
            settle();
            let stroke = |tree: &mut telar::ComponentList, path: &[(f64, f64)]| {
                let (first, rest) = path.split_first().expect("a stroke has a start");
                tree.on_event(&telar::Event::PointerMoved {
                    x: first.0,
                    y: first.1,
                    source: mouse(),
                });
                tree.on_event(&telar::Event::PointerPressed {
                    x: first.0,
                    y: first.1,
                    button: telar::PointerButton::Primary,
                    source: mouse(),
                });
                for point in rest {
                    tree.on_event(&telar::Event::PointerMoved {
                        x: point.0,
                        y: point.1,
                        source: mouse(),
                    });
                }
                let last = path.last().expect("a stroke has an end");
                tree.on_event(&telar::Event::PointerReleased {
                    x: last.0,
                    y: last.1,
                    button: telar::PointerButton::Primary,
                    source: mouse(),
                });
            };

            tree.on_event(&telar::Event::PointerMoved {
                x: at(edge, 1.0, 300.0).0,
                y: at(edge, 1.0, 300.0).1,
                source: mouse(),
            });
            settle();
            assert!(
                deepest() <= PEEK + 0.5,
                "{edge:?}: resting on the peek shows nothing"
            );

            stroke(
                &mut tree,
                &[
                    at(edge, 1.0, 100.0),
                    at(edge, 1.0, 200.0),
                    at(edge, 1.0, 300.0),
                ],
            );
            settle();
            assert!(
                deepest() <= PEEK + 0.5,
                "{edge:?}: a slide along the edge shows nothing"
            );

            stroke(
                &mut tree,
                &[
                    at(edge, 1.0, 300.0),
                    at(edge, 8.0, 300.0),
                    at(edge, 1.0 + PULL + 4.0, 300.0),
                ],
            );
            settle();
            assert_eq!(
                deepest(),
                SIZE,
                "{edge:?}: a pull inward brings the whole bar back"
            );

            for inward in [20.0, 200.0] {
                tree.on_event(&telar::Event::PointerMoved {
                    x: at(edge, inward, 300.0).0,
                    y: at(edge, inward, 300.0).1,
                    source: mouse(),
                });
                settle();
            }
            assert!(
                deepest() <= PEEK + 0.5,
                "{edge:?}: leaving the bar sends it away again"
            );

            stroke(
                &mut tree,
                &[
                    at(edge, 1.0, 300.0),
                    at(edge, 8.0, 300.0),
                    at(edge, 120.0, 300.0),
                ],
            );
            settle();
            assert!(
                deepest() <= PEEK + 0.5,
                "{edge:?}: a pull let go past the bar leaves it where the pointer is not"
            );
        }
    }

    thread_local! {
        static RAN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        static OWN: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Stands in for the IPC table: every line an action runs is written down, and answered.
    fn listen_for_actions() {
        RAN.with(|ran| ran.borrow_mut().clear());
        OWN.with(|own| own.borrow_mut().clear());
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
    }

    fn ran() -> Vec<String> {
        RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    fn own() -> Vec<&'static str> {
        OWN.with(|own| std::mem::take(&mut *own.borrow_mut()))
    }

    fn own_press() {
        OWN.with(|own| own.borrow_mut().push("press"));
    }

    fn own_scroll(_: &Host, _: f32, dy: f32) {
        OWN.with(|own| own.borrow_mut().push(if dy > 0.0 { "up" } else { "down" }));
    }

    /// A chip with a press and a wheel of its own, which is what a bound action has to take the place of.
    fn gestured() -> Vec<ModuleDescriptor> {
        vec![
            module(
                "pressy",
                chip(dummy).on_press(own_press).on_scroll(own_scroll),
            ),
            module("dummy", chip(dummy)),
            module("wanted", chip(wanted)),
            module("unwanted", chip(unwanted)),
        ]
    }

    fn actions(
        bound: &[(layout::Trigger, &str)],
    ) -> std::collections::BTreeMap<layout::Trigger, layout::Action> {
        bound
            .iter()
            .map(|(trigger, line)| (*trigger, layout::Action(vec![line.to_string()])))
            .collect()
    }

    const SCREEN: (f32, f32) = (600.0, 600.0);

    /// `area` built and laid out on [`SCREEN`], ready for events.
    fn laid(area: &ResolvedArea, modules: &[ModuleDescriptor]) -> telar::ComponentList {
        let bar = built(&Config::default(), area, modules, SCREEN).expect("the bar builds");
        let page = Container::new(
            LayoutStyle::new().width(SCREEN.0).height(SCREEN.1),
            vec![bar],
        )
        .expect("a screen");
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the bar lays out");
        tree
    }

    fn middle(rect: telar::Rect) -> (f64, f64) {
        (
            f64::from(rect.x + rect.width / 2.0),
            f64::from(rect.y + rect.height / 2.0),
        )
    }

    fn where_is(area: &ResolvedArea, group: &str, instance: &str) -> telar::Rect {
        rects::rect(
            &rects::Node::area(None, LayerKind::Top, &area.id)
                .instance(&GroupId::new(group), &layout::InstanceId::new(instance)),
        )
        .unwrap_or_else(|| panic!("`{instance}` is in the registry"))
    }

    fn tap(tree: &mut telar::ComponentList, at: (f64, f64), button: telar::PointerButton) {
        let source = telar::PointerSource::Mouse;
        tree.on_event(&telar::Event::PointerMoved {
            x: at.0,
            y: at.1,
            source: source.clone(),
        });
        tree.on_event(&telar::Event::PointerPressed {
            x: at.0,
            y: at.1,
            button,
            source: source.clone(),
        });
        tree.on_event(&telar::Event::PointerReleased {
            x: at.0,
            y: at.1,
            button,
            source,
        });
    }

    fn wheel_at(tree: &mut telar::ComponentList, at: (f64, f64), dy: f32) {
        tree.on_event(&telar::Event::PointerMoved {
            x: at.0,
            y: at.1,
            source: telar::PointerSource::Mouse,
        });
        tree.on_event(&telar::Event::Scrolled {
            delta: telar::ScrollDelta::Pixels { x: 0.0, y: dy },
            x: at.0,
            y: at.1,
        });
    }

    /// Every trigger an instance binds runs its own chain, and each one in place of what the chip would have done with that gesture: the chip's own press and wheel never fire.
    #[test]
    fn every_gesture_a_chip_binds_runs_its_action_instead_of_the_chips_own() {
        use layout::Trigger;
        use telar::PointerButton;
        for edge in Edge::ALL {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            listen_for_actions();
            let _scope = telar::owner_scope();
            let mut area = bar_area(edge, 32.0, [&[], &["pressy"], &[]]);
            area.groups[1].children[0].actions = actions(&[
                (Trigger::Press, "probe press"),
                (Trigger::LongPress, "probe long"),
                (Trigger::Middle, "probe middle"),
                (Trigger::Secondary, "probe secondary"),
                (Trigger::ScrollUp, "probe up"),
                (Trigger::ScrollDown, "probe down"),
            ]);
            let mut tree = laid(&area, &gestured());
            let chip = middle(where_is(&area, "center", "pressy"));

            tap(&mut tree, chip, PointerButton::Primary);
            assert_eq!(ran(), ["probe press"], "{edge:?}");
            tap(&mut tree, chip, PointerButton::Auxiliary);
            assert_eq!(ran(), ["probe middle"], "{edge:?}");
            tap(&mut tree, chip, PointerButton::Secondary);
            assert_eq!(ran(), ["probe secondary"], "{edge:?}");
            wheel_at(&mut tree, chip, 60.0);
            assert_eq!(ran(), ["probe up"], "{edge:?}");
            wheel_at(&mut tree, chip, -60.0);
            assert_eq!(ran(), ["probe down"], "{edge:?}");
            wheel_at(&mut tree, chip, 20.0);
            assert!(
                ran().is_empty(),
                "{edge:?}: a fraction of a notch runs nothing"
            );

            if edge == Edge::Top {
                tree.on_event(&telar::Event::PointerPressed {
                    x: chip.0,
                    y: chip.1,
                    button: PointerButton::Primary,
                    source: telar::PointerSource::Mouse,
                });
                std::thread::sleep(std::time::Duration::from_millis(600));
                tree.on_event(&telar::Event::PointerReleased {
                    x: chip.0,
                    y: chip.1,
                    button: PointerButton::Primary,
                    source: telar::PointerSource::Mouse,
                });
                assert_eq!(ran(), ["probe long"], "a held press is the long one");
            }
            assert!(
                own().is_empty(),
                "{edge:?}: the chip's own press and wheel gave way"
            );
        }
    }

    /// A bound press takes the press and nothing else: the chip's own wheel still turns.
    #[test]
    fn a_bound_press_leaves_the_chips_own_wheel_alone() {
        use layout::Trigger;
        reset_layout_runtime();
        set_theme(NordTheme::new());
        listen_for_actions();
        let _scope = telar::owner_scope();
        let mut area = bar_area(Edge::Top, 32.0, [&["pressy"], &[], &[]]);
        area.groups[0].children[0].actions = actions(&[(Trigger::Press, "probe press")]);
        let mut tree = laid(&area, &gestured());
        let chip = middle(where_is(&area, "start", "pressy"));

        tap(&mut tree, chip, telar::PointerButton::Primary);
        wheel_at(&mut tree, chip, 60.0);
        assert_eq!(ran(), ["probe press"]);
        assert_eq!(own(), ["up"], "the wheel is still the chip's");
    }

    /// A bar's own actions answer only on its empty part. A press on a chip is the chip's whether or not it answers one, and so is the wheel over it — which, unlike a press, an input-opaque chip lets through to the bar.
    #[test]
    fn a_bar_answers_its_own_actions_only_where_no_chip_is() {
        use layout::Trigger;
        use telar::PointerButton;
        for edge in Edge::ALL {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            listen_for_actions();
            let _scope = telar::owner_scope();
            let mut area = bar_area(edge, 32.0, [&["pressy"], &[], &["dummy"]]);
            area.actions = actions(&[
                (Trigger::Press, "bar press"),
                (Trigger::Secondary, "bar secondary"),
                (Trigger::ScrollUp, "bar up"),
            ]);
            area.groups[0].children[0].actions = actions(&[(Trigger::Press, "chip press")]);
            let mut tree = laid(&area, &gestured());
            let pressy = middle(where_is(&area, "start", "pressy"));
            let quiet = middle(where_is(&area, "end", "dummy"));
            let strip = strip(&Config::default(), &area, &[], SCREEN);
            let empty = middle(strip);

            tap(&mut tree, pressy, PointerButton::Primary);
            assert_eq!(ran(), ["chip press"], "{edge:?}: the chip's own action");
            tap(&mut tree, quiet, PointerButton::Primary);
            tap(&mut tree, quiet, PointerButton::Secondary);
            wheel_at(&mut tree, quiet, 60.0);
            assert!(
                ran().is_empty(),
                "{edge:?}: a chip that answers nothing is still not the bar's empty space"
            );
            tap(&mut tree, empty, PointerButton::Primary);
            tap(&mut tree, empty, PointerButton::Secondary);
            wheel_at(&mut tree, empty, 60.0);
            assert_eq!(
                ran(),
                ["bar press", "bar secondary", "bar up"],
                "{edge:?}: the empty part of the bar"
            );
        }
    }

    /// Nothing binds on the lock layer, whatever the file says: validation refuses it, and the build does not take its word for that.
    #[test]
    fn a_bar_built_for_the_lock_binds_no_action() {
        use layout::Trigger;
        reset_layout_runtime();
        set_theme(NordTheme::new());
        listen_for_actions();
        let _scope = telar::owner_scope();
        let mut area = bar_area(Edge::Top, 32.0, [&["pressy"], &[], &[]]);
        area.actions = actions(&[(Trigger::Press, "bar press")]);
        area.groups[0].children[0].actions = actions(&[(Trigger::Press, "chip press")]);
        let config = Arc::new(Config::default());
        let bar = build_bar(
            &area,
            Surround {
                config: &config,
                theme: NordTheme::new(),
                output: None,
                layer: LayerKind::Lock,
                bounds: telar::Rect::new(0.0, 0.0, SCREEN.0, SCREEN.1),
                reserved: Reserved::default(),
                audience: ui::host::Audience::Anyone,
            },
            &gestured(),
        )
        .expect("the bar builds");
        let page = Container::new(
            LayoutStyle::new().width(SCREEN.0).height(SCREEN.1),
            vec![bar],
        )
        .expect("a screen");
        let root = page.layout_node();
        let mut tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the bar lays out");
        let chip = rects::rect(
            &rects::Node::area(None, LayerKind::Lock, &area.id)
                .instance(&GroupId::new("start"), &layout::InstanceId::new("pressy")),
        )
        .expect("the chip is in the registry");

        tap(&mut tree, middle(chip), telar::PointerButton::Primary);
        tap(&mut tree, (300.0, 16.0), telar::PointerButton::Primary);
        assert!(ran().is_empty());
    }

    /// A stacked group on a bar is one chip at a time where one chip would be: only the chip on show is built, the wheel over it moves on, and the group is one box in the registry.
    #[test]
    fn a_stacked_group_on_a_bar_shows_one_chip_at_a_time() {
        for edge in Edge::ALL {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            BUILT.with(|built| built.borrow_mut().clear());
            let _scope = telar::owner_scope();
            let mut area = bar_area(edge, 32.0, [&[], &[], &["wanted", "unwanted"]]);
            area.groups[2].stacked = true;
            let mut tree = laid(&area, &gestured());
            assert_eq!(
                BUILT.with(|built| built.borrow().clone()),
                ["wanted"],
                "{edge:?}: only the chip on show"
            );
            let group = rects::rect(
                &rects::Node::area(None, LayerKind::Top, &area.id).group(&GroupId::new("end")),
            )
            .expect("the stack is in the registry");
            let shown = where_is(&area, "end", "wanted");
            assert!(
                group.contains(shown.x + 1.0, shown.y + 1.0),
                "{edge:?}: the chip on show is inside its stack"
            );
            let strip = strip(&Config::default(), &area, &[], SCREEN);
            let (end_x, end_y) = (group.x + group.width, group.y + group.height);
            let (strip_x, strip_y) = (strip.x + strip.width, strip.y + strip.height);
            assert!(
                match edge.is_horizontal() {
                    true => strip_x - end_x < strip.width / 4.0,
                    false => strip_y - end_y < strip.height / 4.0,
                },
                "{edge:?}: a stack in the end zone sits at the end, {group:?} of {strip:?}"
            );

            wheel_at(&mut tree, middle(group), 60.0);
            assert_eq!(
                BUILT.with(|built| built.borrow().last().copied()),
                Some("unwanted"),
                "{edge:?}: a notch shows the next chip"
            );
        }
    }
}
