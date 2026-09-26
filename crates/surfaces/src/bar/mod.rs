mod autohide;

pub use autohide::{AutoHide, RevealMargins};

use std::rc::Rc;
use std::sync::Arc;

use telar::{
    AlignItems, Clip, ClippedItem, Color, Container, JustifyContent, LayoutError, LayoutItem,
    LayoutStyle, RectStyle, Slots, StyledContainer, track_layout,
};

use crate::area::Surround;
use crate::layer_window::Reserved;
use config::theme::NordTheme;
use config::{Config, Edge, ResolvedShape, Shape, Variant};
use layout::{
    BarShape, Extent, GroupKind, ResolvedArea, ResolvedAreaKind, ResolvedGroup, ResolvedInstance,
    Zone,
};
use ui::descriptor::{ChipDef, ChipFrame, ModuleDescriptor};
use ui::host::{Host, InstanceId, Representation, Size};
use ui::layout::painted_chrome;
use ui::module::{DragOpen, module_foreground, resting_fill};
use ui::module_shell::{ModuleShellProps, module_shell};
use ui::placeholder::placeholder;

/// Builds the content tree for `area`, branching on its resolved `mode` (bar/sections/chips); visual properties come from gap/spacing/radius, not mode.
///
/// Everything that says where the bar's parts go comes off the area and off what surrounds it on the output: the edge it hangs off, how thick it is, how far it runs and the zones it holds. `surround.config` stays for what is behaviour rather than arrangement — `[popouts] enabled`, `[panels] drag_threshold`, `[theme] opacity` and `[shape] frame` — and for the global `[shape]` and `[modules.<id>]` defaults the area's and the instance's own overrides are laid over.
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
        ..
    } = &area.kind
    else {
        return Err(LayoutError::Engine(format!(
            "'{}' is a {} area, and only a bar has zones to draw",
            area.id,
            area.kind.name()
        )));
    };
    let config = surround.config;
    let shape = bar_shape(config, shape);
    let run = run_of(edge, surround.bounds, surround.reserved, shape.gap as f32);
    let zones = zones_of(&area.groups);
    let chrome = Chrome {
        config,
        edge,
        thickness,
        shape,
        abut: ends_abut(length, offset, run),
        theme: surround.theme,
        output: surround.output,
        strip: strip_of(edge, thickness, length, offset, run, surround),
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
    let gap = bar_shape(surround.config, shape).gap as f32;
    let run = run_of(edge, surround.bounds, surround.reserved, gap);
    strip_of(edge, thickness, length, offset, run, surround)
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
    surround: Surround,
) -> telar::Rect {
    let gap = surround.config.edge_gap(edge) as f32;
    let bounds = surround.bounds;
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

/// The area's own shape over the global `[shape]`, and the theme under both.
///
/// The model measures gap, spacing and radius as floats where `[shape]` has always taken whole pixels, so they are rounded on the way in rather than the config being widened to match: each is a distance in logical px, and no part of the shell draws a fraction of one.
fn bar_shape(config: &Config, shape: BarShape) -> ResolvedShape {
    let px = |value: Option<f32>| value.map(|px| px.round() as u32);
    config.shape_from(
        shape.mode,
        px(shape.gap),
        px(shape.spacing),
        px(shape.radius),
    )
}

/// A bar's three zones, each the instances placed in it.
type Zones<'a> = [(Vec<&'a ResolvedInstance>, Zone); 3];

/// The area's groups gathered into the three runs a bar draws. Several groups may name the same zone, so a zone is every group that claimed it in the order they were placed, rather than one group's children.
fn zones_of(groups: &[ResolvedGroup]) -> Zones<'_> {
    let run = |wanted: Zone| {
        groups
            .iter()
            .filter(|group| matches!(group.kind, GroupKind::Zone { zone } if zone == wanted))
            .flat_map(|group| group.children.iter())
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

#[derive(Clone, Copy)]
struct Chrome<'a> {
    config: &'a Arc<Config>,
    edge: Edge,
    /// How thick the area is across its edge, which is the only size a chip on it is given.
    thickness: f32,
    shape: ResolvedShape,
    /// Whether the leading and trailing ends meet something; see [`ends_abut`].
    abut: (bool, bool),
    theme: NordTheme,
    output: Option<&'a str>,
    /// Where on the output the bar sits, which is what it places itself at.
    strip: telar::Rect,
}

impl Chrome<'_> {
    /// The host a chip is built under.
    ///
    /// Keyed by the module rather than by the placed instance's own id: a chip's press, a keybind and `hogar-shell panel toggle` all name a module, so a chip that kept its state under a layout id would stop sharing it with the three ways the same instance is reached from outside the bar. Giving those an instance to name is one change, and this is not it.
    fn host(&self, id: &str, accent: Color, foreground: Color) -> Host {
        Host::placed(
            InstanceId::of_module(id),
            Arc::clone(self.config),
            Representation::Chip,
            chip_extent(self.edge, self.thickness),
            Some(self.edge),
            self.shape,
            accent,
            foreground,
            self.output.map(str::to_string),
        )
    }
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

/// What the bar paints over the strip it occupies — and so, by [`painted_chrome`], what it claims for the window's input region.
///
/// Under `[shape] frame` the bars *are* the ring: each one fills its own strip flat, whatever its mode, because that is what a continuous band around the screen is made of now that no surface draws one. Otherwise only `bar` mode has a background of its own: in `sections` and `chips` the bar between two chips is a hole, and a press there belongs to the window underneath rather than to the shell.
fn strip_fill(config: &Config, mode: Shape, token: Color) -> Color {
    if config.shape.frame || matches!(mode, Shape::Bar) {
        return token.with_alpha(config.opacity());
    }
    Color::TRANSPARENT
}

/// What a section's panel or a resting chip paints. Nothing while a frame is up: the bar has already filled its strip flat, and a second fill over those pixels is a darker band along every edge the two share.
fn inner_fill(config: &Config, token: Color) -> Color {
    if config.shape.frame {
        return Color::TRANSPARENT;
    }
    token.with_alpha(config.opacity())
}

/// Cuts the bar off at its own strip.
///
/// A chip is routinely a shade wider than the strip its zone was given — a padded box is narrower than the bar and a square chip is sized from the bar itself — and while a bar had a surface of its own, the surface cut that overhang off for nothing. With one window per layer there is no surface to cut it: the overhang lands on the desktop, drawn but claimed by nothing, so a press on it reaches the application underneath. The clip is at the bar's root rather than at a zone for exactly that reason: what a zone cuts is one run running into another, and what this cuts is the bar running off its own edge.
fn strip_clipped(chrome: StyledContainer) -> Box<dyn LayoutItem> {
    Box::new(ClippedItem::new(Box::new(chrome), Clip::both()))
}

/// How far a bar's own background is rounded. A ring made of rounded pills is four floating bars rather than a frame, so under `[shape] frame` the strip is square. Its rounded *inner* corners went with the surface that drew them: a concave corner at the junction of two strips lies inside neither, so no area can paint it (F-10.23).
fn bar_radius(config: &Config, shape: ResolvedShape) -> f32 {
    if config.shape.frame {
        0.0
    } else {
        shape.radius
    }
}

fn build_whole_bar(
    chrome: &Chrome,
    zones: &Zones,
    modules: &[ModuleDescriptor],
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Chrome {
        config,
        edge,
        shape,
        abut,
        theme,
        strip,
        ..
    } = *chrome;
    let base = strip_fill(config, Shape::Bar, theme.base);
    let spacing = shape.spacing;
    // The whole bar already pads every side by `padding`, so an end that meets another bar is only owed the rest of a chip's worth of air.
    let ends = end_air(abut, (spacing - shape.padding()).max(0.0));
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
    let radius = bar_radius(config, shape);
    let style = axis(
        crate::area::at(strip)
            .align_items(AlignItems::CENTER)
            .padding_all(shape.padding()),
        edge,
    );
    Ok(strip_clipped(painted_chrome(
        StyledContainer::new(style, move |_r| RectStyle::filled(base, radius), slots)?,
        base,
    )))
}

fn build_units(
    chrome: &Chrome,
    zones: &Zones,
    modules: &[ModuleDescriptor],
    granularity: Granularity,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Chrome {
        config,
        edge,
        shape,
        abut,
        theme,
        strip,
        ..
    } = *chrome;
    let spacing = shape.spacing;
    // Section: modules share a per-zone surface panel (wrapped in `unit`); Chip: each module is its own free-standing pill, no `unit`.
    let surface = inner_fill(config, theme.surface);
    let (rest, shell_radius) = match granularity {
        Granularity::Section => (Color::TRANSPARENT, shape.chip_radius()),
        Granularity::Chip => (surface, shape.chip_radius()),
    };
    let mut slots = Vec::with_capacity(3);
    for (entries, in_zone) in zones {
        let items = build_items(chrome, entries, modules, rest, shell_radius)?;
        let content: Vec<Box<dyn LayoutItem>> = if items.is_empty() {
            Vec::new()
        } else {
            match granularity {
                Granularity::Section => {
                    vec![unit(edge, *in_zone, shape.radius, spacing, surface, items)?]
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
        crate::area::at(strip).align_items(AlignItems::STRETCH),
        edge,
    );
    let base = strip_fill(config, shape.mode, theme.base);
    let radius = bar_radius(config, shape);
    Ok(strip_clipped(painted_chrome(
        StyledContainer::new(style, move |_r| RectStyle::filled(base, radius), slots)?,
        base,
    )))
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
    items: Vec<Box<dyn LayoutItem>>,
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
        StyledContainer::new(
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
    items: Vec<Box<dyn LayoutItem>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let style = LayoutStyle::new()
        .align_items(cross)
        .justify_content(justify(in_zone))
        .gap(spacing);
    if let Zone::Center = in_zone {
        return Ok(Box::new(Container::new(axis(style, edge), items)?));
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
    let zone = Container::new(axis(style, edge), items)?;
    Ok(Box::new(ClippedItem::new(Box::new(zone), clip)))
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

/// A box wrapped around a module's own content to carry what its chip cannot: a wheel handler and the surface a chip rests on for a self-managed module (which has no [`module_shell`] to put either on), and the pointer tracking behind a hover popout. They live here rather than on the chip so a self-managed module gets them on the same terms as any other; the wrapper shrink-wraps its child, so the rect it tracks and the surface it paints are the chip's own.
///
/// `cross` is what the wrapper would otherwise silently change. A chip is a direct zone child under `AlignItems::STRETCH`, so it fills the bar's thickness; a wrapper that centred it instead would shrink every popout-bearing chip to its content. A self-managed module lays itself out and is centred, as it was before any wrapper existed.
///
/// It runs along the bar for the same reason [`zone`] does. A wrapper fixed to a row applies `cross` across the *screen's* vertical, so on a left or right bar it stretched each chip's height — which is already its content — and left its width free: every wrapped chip then sat at its own content width, ragged against the bar's inner edge, with the wide ones running off the screen. Thirteen chips carry a popout, so on a vertical bar that was most of them. `elastic` is the chip's own, forwarded. The wrapper is what the zone actually sizes, so a rigid one around an elastic chip is a chip that never gets the chance to give anything up — and both modules whose label elides carry a popout, which is to say both of them are wrapped.
fn chip_wrapper(
    content: Box<dyn LayoutItem>,
    on_scroll: Option<Wheel>,
    popout: Option<&str>,
    dress: Wrapper,
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
    let mut wrapper = painted_chrome(
        StyledContainer::new(
            style,
            move |_r| RectStyle::filled(fill, radius),
            vec![content],
        )?,
        fill,
    );
    if let Some(on_scroll) = on_scroll {
        wrapper = wrapper.on_scroll(move |dx, dy| on_scroll(dx, dy));
    }
    if let Some(module) = popout {
        // Tracked before the handler that reads it is attached, so the popout has a rect the first time the pointer arrives.
        let rect = track_layout(wrapper.layout_node())
            .expect("a container registers its rect")
            .read_only();
        let module = module.to_string();
        wrapper =
            wrapper.on_hover(move |entered| crate::popout::hover(&module, rect.get(), entered));
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

/// The container variant a placed chip draws with: its own `variant` option, else the module's `[modules.<id>]` override.
fn instance_variant(config: &Config, instance: &ResolvedInstance) -> Variant {
    instance
        .options
        .get("variant")
        .and_then(|value| value.clone().try_into().ok())
        .unwrap_or_else(|| config.variant_for(&instance.module))
}

/// The accent-token name a placed chip draws with: its own `accent` option, else the module's, else the global one.
fn instance_accent_name<'a>(config: &'a Config, instance: &'a ResolvedInstance) -> &'a str {
    match instance
        .options
        .get("accent")
        .and_then(|value| value.as_str())
    {
        Some(accent) => accent,
        None => config.accent_name_for(&instance.module),
    }
}

/// Builds each instance's content and wraps it in its base container; a module no id answers to, one with no chip, and a chip whose build fails or panics are each drawn as a [placeholder](ui::placeholder) where declared, so the mistake stays on screen.
fn build_items(
    chrome: &Chrome,
    instances: &[&ResolvedInstance],
    modules: &[ModuleDescriptor],
    rest: Color,
    radius: f32,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let config = chrome.config;
    let mut items: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(instances.len());
    for instance in instances {
        let id = &instance.module;
        let variant = instance_variant(config, instance);
        let accent = chrome
            .theme
            .accent_by_name(instance_accent_name(config, instance));
        let host = chrome.host(id, accent, module_foreground(variant, accent, chrome.theme));
        let placed = ui::descriptor::lookup(modules, id)
            .and_then(|module| Some((*module, module.representations.chip?)));
        let Some((module, chip)) = placed else {
            items.push(placeholder(id, None, &host, chrome.theme)?);
            continue;
        };
        let look = Look {
            variant,
            rest,
            accent,
            radius,
            edge: chrome.edge,
            popout: module.representations.popout.is_some() && config.popouts.enabled,
            drag_open: drag_open_for(config, &module, chrome.edge),
        };
        let style = chip_box(&chip, chrome.edge);
        let built = host.clone();
        items.push(ui::descriptor::guard(id, &host, style, move || {
            placed_chip(&module, &chip, &built, look)
        })?);
    }
    Ok(items)
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
    let popout = look.popout.then_some(module.id);
    let wheel = wheel(chip, host);
    if chip.is_bare() {
        return bare_chip(
            content,
            chip,
            wheel,
            popout,
            look.edge,
            resting_fill(look.variant, look.rest, look.accent),
            look.radius,
        );
    }
    // Handed over bare: the chip shell dispatches every press with its own rect in scope, a panel toggle and a press of the chip's own alike, so whatever this opens can hang off the chip without being told where it is.
    let on_press: Option<Rc<dyn Fn()>> = match chip.press {
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
            .on_press(on_press)
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
        ),
        None => Ok(shell),
    }
}

type Wheel = Rc<dyn Fn(f32, f32)>;

/// The chip's wheel handler bound to the host it was built under, so a notch acts on this bar's options rather than the global config.
fn wheel(chip: &ChipDef, host: &Host) -> Option<Wheel> {
    let scroll = chip.scroll?;
    let host = host.clone();
    Some(Rc::new(move |dx, dy| scroll(&host, dx, dy)))
}

/// A self-managed module skips `module_shell` — it lays itself out and takes its own presses — so its wheel handler, popout tracking and the surface it rests on go on a wrapper with no padding or hover state.
fn bare_chip(
    content: Box<dyn LayoutItem>,
    chip: &ChipDef,
    wheel: Option<Wheel>,
    popout: Option<&str>,
    edge: Edge,
    resting: Color,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fill = match chip.frame {
        ChipFrame::Filler => Color::TRANSPARENT,
        _ => resting,
    };
    if wheel.is_none() && popout.is_none() && fill == Color::TRANSPARENT {
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{AvailableSpace, compute_layout, reset_layout_runtime, set_theme};
    use ui::descriptor::{CardDef, Input, Representations};

    /// The bar `config` draws on `edge`, on a screen exactly the size of the page the test lays it out on.
    ///
    /// A bar places itself on its output now, so a test has to say how big that output is or the strip lands somewhere the page cannot show. `page` is the same pair the test hands `compute_layout`, which is what keeps the two from drifting.
    ///
    /// The tests describe a bar in `config.toml` and the builder reads an area, so [`area_of`] stands between them. It goes with `[bars]` itself in T-3.4, and these tests then say what they mean in the layout model's own words.
    fn built(
        config: &Config,
        edge: Edge,
        modules: &[ModuleDescriptor],
        page: (f32, f32),
    ) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let config = Arc::new(config.clone());
        build_bar(
            &area_of(&config, edge),
            Surround {
                config: &config,
                theme: NordTheme::new(),
                output: None,
                bounds: telar::Rect::new(0.0, 0.0, page.0, page.1),
                reserved: reserved_of(&config),
            },
            modules,
        )
    }

    /// What `config`'s own bars take off each edge, which is what a bar reads to know where its neighbours left it room.
    fn reserved_of(config: &Config) -> Reserved {
        let on = |edge| config.edge_reserved(edge) as f32;
        Reserved {
            top: on(Edge::Top),
            right: on(Edge::Right),
            bottom: on(Edge::Bottom),
            left: on(Edge::Left),
        }
    }

    /// Where on `page` the bar `config` describes for `edge` actually lands, for a test measuring the air at its ends.
    fn strip(config: &Config, edge: Edge, page: (f32, f32)) -> telar::Rect {
        let config = Arc::new(config.clone());
        let area = area_of(&config, edge);
        let ResolvedAreaKind::Bar {
            thickness,
            length,
            offset,
            shape,
            ..
        } = area.kind
        else {
            unreachable!("the helper builds a bar")
        };
        let surround = Surround {
            config: &config,
            theme: NordTheme::new(),
            output: None,
            bounds: telar::Rect::new(0.0, 0.0, page.0, page.1),
            reserved: reserved_of(&config),
        };
        let gap = bar_shape(&config, shape).gap as f32;
        let run = run_of(edge, surround.bounds, surround.reserved, gap);
        strip_of(edge, thickness, length, offset, run, surround)
    }

    /// The area one edge's `[bars.<edge>]` section describes, corner sugar routed into the start and end zones as [`Config::corner_modules_for`] answers it.
    fn area_of(config: &Config, edge: Edge) -> ResolvedArea {
        let bar = config.bars.get(edge);
        let (lead, trail) = config.corner_modules_for(edge);
        let mut start: Vec<config::ModuleEntry> = Vec::new();
        start.extend(lead.map(config::ModuleEntry::bare));
        start.extend(bar.start.iter().cloned());
        let mut end: Vec<config::ModuleEntry> = bar.end.clone();
        end.extend(trail.map(config::ModuleEntry::bare));
        ResolvedArea {
            id: layout::AreaId::new(format!("bar-{}", edge.as_str())),
            kind: ResolvedAreaKind::Bar {
                edge,
                thickness: bar.size as f32,
                length: Extent::Fill,
                offset: 0.0,
                shape: BarShape {
                    mode: bar.shape.mode,
                    gap: bar.shape.gap.map(|px| px as f32),
                    spacing: bar.shape.spacing.map(|px| px as f32),
                    radius: bar.shape.radius.map(|px| px as f32),
                },
                autohide: (!bar.persistent).then(|| layout::AutoHide {
                    peek: config.bar_peek(edge) as f32,
                    on_hover: bar.show_on_hover,
                }),
            },
            reserve: config.bar_is_persistent(edge),
            above_fullscreen: false,
            within: layout::Within::Output,
            style: layout::AreaStyle::default(),
            visible: None,
            groups: vec![
                zone_group("start", Zone::Start, &start),
                zone_group("center", Zone::Center, &bar.center),
                zone_group("end", Zone::End, &end),
            ],
        }
    }

    fn zone_group(id: &str, zone: Zone, entries: &[config::ModuleEntry]) -> ResolvedGroup {
        ResolvedGroup {
            id: layout::GroupId::new(id),
            kind: GroupKind::Zone { zone },
            children: entries.iter().map(placed).collect(),
        }
    }

    /// One `[bars.<edge>]` entry as a placed instance: its `variant` and `accent` become instance options, which is where a chip's own look is read from now that an entry is not what the builder sees.
    fn placed(entry: &config::ModuleEntry) -> ResolvedInstance {
        let mut options = toml::Table::new();
        if let Some(variant) = entry
            .variant
            .and_then(|variant| toml::Value::try_from(variant).ok())
        {
            options.insert("variant".into(), variant);
        }
        if let Some(accent) = &entry.accent {
            options.insert("accent".into(), toml::Value::String(accent.clone()));
        }
        ResolvedInstance {
            id: layout::InstanceId::new(&entry.id),
            module: entry.id.clone(),
            representation: layout::Representation::Chip,
            options,
            bindings: std::collections::BTreeMap::new(),
            actions: std::collections::BTreeMap::new(),
        }
    }

    /// A chip on a 32px bar along `edge`, dressed exactly as [`Chrome::host`] dresses one.
    fn test_host(edge: Edge) -> Host {
        let theme = NordTheme::new();
        let config = Config::default();
        let shape = config.shape_from(None, None, None, None);
        Host::placed(
            InstanceId::new("dummy"),
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

    fn module(id: &'static str, chip: ChipDef) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
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
        let config: config::Config =
            toml::from_str("[bars.top]\nsize=34\ncenter=[\"wanted\"]\n").unwrap();

        built(&config, Edge::Top, &registry, (600.0, 34.0)).expect("the bar builds");

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
            Some("volume"),
            Wrapper {
                cross: AlignItems::STRETCH,
                edge: Edge::Top,
                elastic: false,
                fill: Color::TRANSPARENT,
                radius: 0.0,
            },
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
                let cfg: Config = toml::from_str(&format!(
                    "[shape]\nmode=\"{mode}\"\n[bars.{}]\nsize=32\n\
                     start=[{{id=\"dummy\",variant=\"filled\",accent=\"green\"}},\"{broken}\",\
                     {{id=\"dummy\",variant=\"filled\",accent=\"purple\"}}]\n",
                    edge.as_str()
                ))
                .unwrap();
                let (w, h) = if edge.is_horizontal() {
                    (600.0, 32.0)
                } else {
                    (32.0, 600.0)
                };
                let bar = built(&cfg, edge, &registry(), (w, h)).expect("the bar builds");
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

    #[test]
    fn a_self_managed_module_rests_on_a_chip_like_its_neighbours() {
        let surfaces = |mode: &str, def: ChipDef| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let registry = vec![module("probe", def)];
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"{mode}\"\n[bars.top]\nsize=32\ncenter=[\"probe\"]\n"
            ))
            .unwrap();
            let surface = telar::Paint::Solid(inner_fill(&cfg, NordTheme::new().surface));
            let bar = built(&cfg, Edge::Top, &registry, (400.0, 32.0)).expect("the bar builds");
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
            let toml = format!(
                "[shape]\nmode=\"{mode}\"\ngap=6\nradius=10\nspacing=8\n\
                 [bars.top]\nstart=[\"dummy\"]\ncenter=[\"dummy\"]\nend=[\"dummy\"]\n"
            );
            let cfg: Config = toml::from_str(&toml).unwrap();
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let bar = built(&cfg, Edge::Top, &registry(), (1920.0, 32.0));
            assert!(bar.is_ok(), "mode {mode} builds a tree");
        }
    }

    /// The centre zone is centred on the bar, whatever the chips beside it are doing.
    ///
    /// The regression is the one thing a bar cannot get away with: all three zones took their content width plus an equal share of the slack, so the centre sat on the middle of the *leftover* space. Every chip that changes width slid it — and the window title changes width on every focus change, which walked the clock and the launcher sideways all day.
    #[test]
    fn the_centre_holds_still_while_a_side_chip_changes_width() {
        const BAR: f32 = 1920.0;

        let midpoint = |start: &str, mode: &str| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"{mode}\"\nspacing=8\n\
                 [bars.top]\nsize=32\nstart=[\"{start}\"]\ncenter=[\"centred\"]\nend=[\"dummy\"]\n"
            ))
            .unwrap();
            let bar = built(&cfg, Edge::Top, &registry(), (BAR, 32.0)).expect("the bar builds");
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
            let narrow = midpoint("dummy", mode);
            let widened = midpoint("wide", mode);
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
            let overrun = midpoint("wide\",\"wide\",\"wide\",\"wide", mode);
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
            let overrun = || -> Vec<Box<dyn LayoutItem>> {
                (0..4)
                    .map(|_| wide(&host).expect("a chip builds"))
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
                vec![centre_chip],
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
        const ABOVE: &str = "[bars.top]\nsize=30\ncenter=[\"dummy\"]\n";
        const BELOW: &str = "[bars.bottom]\nsize=30\ncenter=[\"dummy\"]\n";

        // The chip's rect and the strip the bar landed on, both in the screen's own coordinates: a vertical bar stops where the bars above and below it left it, so its own middle is no longer the screen's.
        let probe = |mode: &str, neighbours: &str, zones: &str| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"{mode}\"\nspacing=8\n{neighbours}[bars.left]\nsize=32\n{zones}"
            ))
            .unwrap();
            let bar = built(&cfg, Edge::Left, &registry(), (32.0, LENGTH)).expect("the bar builds");
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
            (chip, strip(&cfg, Edge::Left, (32.0, LENGTH)))
        };
        let air = |mode: &str, neighbours: &str| {
            let (first, strip) = probe(mode, neighbours, "start=[\"centred\"]\n");
            let (last, _) = probe(mode, neighbours, "end=[\"centred\"]\n");
            (
                first.y - strip.y,
                (strip.y + strip.height) - (last.y + last.height),
            )
        };

        let boxed_in = format!("{ABOVE}{BELOW}");
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
            let (bare_lead, bare_trail) = air(mode, "");
            assert!(
                bare_lead < lead && bare_trail < trail,
                "{mode}: with no bar above or below, the end chips still hold {bare_lead}px and {bare_trail}px \
                 off the screen's edges"
            );

            let (centre, strip) = probe(mode, ABOVE, "start=[\"dummy\"]\ncenter=[\"centred\"]\n");
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
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"chips\"\nspacing=8\n\
                 [bars.top]\nsize=32\nstart=[\"wide\",\"{module}\"]\ncenter=[\"dummy\"]\n"
            ))
            .unwrap();
            let bar = built(&cfg, Edge::Top, &registry(), (600.0, 32.0)).expect("the bar builds");
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
        let cfg: Config = toml::from_str(
            "[shape]\nmode=\"bar\"\nspacing=8\n\
             [bars.left]\nsize=32\nstart=[\"overrunner\"]\ncenter=[\"dummy\"]\n",
        )
        .unwrap();
        let bar = built(&cfg, Edge::Left, &registry, (32.0, BAR)).expect("the bar builds");
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
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"{mode}\"\nspacing=8\n\
                 [bars.top]\nsize=32\nstart=[\"wide\",\"wide\",\"wide\",\"wide\"]\n\
                 center=[\"centred\"]\nend=[\"wide\",\"wide\",\"wide\",\"wide\"]\n"
            ))
            .unwrap();
            let bar = built(&cfg, Edge::Top, &registry(), (BAR, 32.0)).expect("the bar builds");
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
        let cfg: Config = toml::from_str(
            "[shape]\nmode=\"sections\"\nspacing=8\nradius=8\n\
             [bars.top]\nsize=32\nstart=[\"wide\",\"wide\",\"wide\",\"wide\"]\ncenter=[\"dummy\"]\n",
        )
        .unwrap();
        let bar = built(&cfg, Edge::Top, &registry(), (BAR, 32.0)).expect("the bar builds");
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

    #[test]
    fn corner_module_routes_into_owning_bar() {
        for mode in ["bar", "sections", "chips"] {
            let cfg: Config = toml::from_str(&format!(
                "[shape]\nmode=\"{mode}\"\n[bars.top]\ncenter=[\"dummy\"]\n[corners]\ntop_left=\"dummy\"\n"
            ))
            .unwrap();
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let bar = built(&cfg, Edge::Top, &registry(), (1920.0, 34.0));
            assert!(bar.is_ok(), "corner routing builds in mode {mode}");
        }
    }

    #[test]
    fn center_only_sections_builds_a_notch() {
        let cfg: Config = toml::from_str(
            "[shape]\nmode=\"sections\"\ngap=8\nradius=12\n[bars.top]\ncenter=[\"dummy\"]\n",
        )
        .unwrap();
        reset_layout_runtime();
        set_theme(NordTheme::new());
        assert!(built(&cfg, Edge::Top, &registry(), (1920.0, 34.0)).is_ok());
    }

    #[test]
    fn vertical_bar_builds_in_every_mode() {
        for mode in ["bar", "sections", "chips"] {
            let toml = format!(
                "[shape]\nmode=\"{mode}\"\nradius=8\n[bars.left]\nsize=44\nstart=[\"dummy\"]\nend=[\"dummy\"]\n"
            );
            let cfg: Config = toml::from_str(&toml).unwrap();
            reset_layout_runtime();
            set_theme(NordTheme::new());
            assert!(
                built(&cfg, Edge::Left, &registry(), (44.0, 1080.0)).is_ok(),
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
                Some("clock"),
                Wrapper {
                    cross: AlignItems::STRETCH,
                    edge,
                    elastic: false,
                    fill: Color::TRANSPARENT,
                    radius: 0.0,
                },
            )
            .unwrap();
            // The zone the bar puts a chip in: along the bar, stretching its children across it.
            let zone = zone(
                edge,
                Zone::Start,
                0.0,
                (0.0, 0.0),
                AlignItems::STRETCH,
                vec![wrapped],
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
}
