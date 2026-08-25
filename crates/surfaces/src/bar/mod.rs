mod app;
mod autohide;

pub use app::BarApp;
pub use autohide::{AutoHide, RevealMargins};

use telar::{
    AlignItems, ClipAxis, ClippedItem, Color, Container, JustifyContent, LayoutError, LayoutItem,
    LayoutStyle, RectStyle, SizeDimension, Slots, StyledContainer, track_layout,
};

use config::theme::NordTheme;
use config::{Config, Edge, ModuleEntry, ResolvedShape, Shape, Zone};
use ui::module::{
    DragOpen, ModuleClick, ModuleCtx, ModuleDef, ModuleRegistry, module_foreground, set_module_fg,
};
use ui::{ModuleShellProps, module_shell};

/// The bar the running config draws, for [`crate::preview`] — every chip the user put on it, in the zones and the shape they configured, against the registry the app installed.
pub(crate) fn preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let env = ui::preview::bar_chip();
    let theme = env.config.resolve_theme();
    ui::module::with_registry(|registry| {
        build_bar(&env.config, env.edge, theme.accent, registry, theme)
    })
}

/// Builds the content tree for the bar, branching on its resolved `mode` (bar/sections/chips); visual properties come from gap/spacing/radius, not mode.
pub fn build_bar(
    config: &Config,
    edge: Edge,
    accent: Color,
    registry: &ModuleRegistry,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let bar = config.bars.get(edge);
    let shape = config.shape_for(edge);
    let ctx = ModuleCtx {
        theme,
        accent,
        bar_size: bar.size,
        edge,
    };
    // `[corners]` sugar: corner modules are routed to the owning bar's start/end zones, not separate surfaces.
    let (lead, trail) = config.corner_modules_for(edge);
    let mut start: Vec<ModuleEntry> = Vec::new();
    start.extend(lead.map(ModuleEntry::bare));
    start.extend(bar.start.iter().cloned());
    let mut end: Vec<ModuleEntry> = bar.end.clone();
    end.extend(trail.map(ModuleEntry::bare));
    let zones: Zones = [
        (start.as_slice(), Zone::Start),
        (bar.center.as_slice(), Zone::Center),
        (end.as_slice(), Zone::End),
    ];
    let chrome = Chrome { edge, shape, theme };
    match shape.mode {
        Shape::Bar => build_whole_bar(config, &chrome, &zones, registry, &ctx),
        Shape::Sections => build_units(
            config,
            &chrome,
            &zones,
            registry,
            &ctx,
            Granularity::Section,
        ),
        Shape::Chips => build_units(config, &chrome, &zones, registry, &ctx, Granularity::Chip),
    }
}

/// A bar's three zones, each the entries placed in it.
type Zones<'a> = [(&'a [ModuleEntry], Zone); 3];

fn justify(zone: Zone) -> JustifyContent {
    match zone {
        Zone::Start => JustifyContent::START,
        Zone::Center => JustifyContent::CENTER,
        Zone::End => JustifyContent::END,
    }
}

#[derive(Clone, Copy)]
struct Chrome {
    edge: Edge,
    shape: ResolvedShape,
    theme: NordTheme,
}

#[derive(Clone, Copy)]
enum Granularity {
    Section,
    Chip,
}

/// What a bar paints its own background with: the token at `[bars] opacity`, or nothing at all while a frame is up, because the frame draws the ring covering exactly these strips and two fills stacking is a darker band along every edge they share.
fn bar_fill(config: &Config, token: Color) -> Color {
    if config.shape.frame {
        return Color::TRANSPARENT;
    }
    token.with_alpha(config.opacity())
}

fn build_whole_bar(
    config: &Config,
    chrome: &Chrome,
    zones: &Zones,
    registry: &ModuleRegistry,
    ctx: &ModuleCtx,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Chrome { edge, shape, theme } = *chrome;
    // With a frame up, the ring it draws already fills the strip this bar sits in. Painting again on top is what made two translucent fills stack and darken along the edges the two share.
    let base = bar_fill(config, theme.base);
    let spacing = shape.spacing;
    let mut slots = Vec::with_capacity(3);
    for (entries, in_zone) in zones {
        // Modules blend into the shared surface (transparent rest); STRETCH makes every chip the bar's height so text pills and icon chips line up. The hover/press (and Filled) highlight rounds at the theme's chip radius, matching chip mode.
        let items = build_items(
            config,
            entries,
            registry,
            ctx,
            Color::TRANSPARENT,
            shape.chip_radius(),
        )?;
        slots.push(zone(edge, *in_zone, spacing, AlignItems::STRETCH, items)?);
    }
    let radius = shape.radius;
    let style = axis(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0))
            .align_items(AlignItems::CENTER)
            .padding_all(shape.padding()),
        edge,
    );
    Ok(Box::new(StyledContainer::new(
        style,
        move |_r| RectStyle::filled(base, radius),
        slots,
    )?))
}

fn build_units(
    config: &Config,
    chrome: &Chrome,
    zones: &Zones,
    registry: &ModuleRegistry,
    ctx: &ModuleCtx,
    granularity: Granularity,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Chrome { edge, shape, theme } = *chrome;
    let spacing = shape.spacing;
    // Section: modules share a per-zone surface panel (wrapped in `unit`); Chip: each module is its own free-standing pill, no `unit`.
    let surface = bar_fill(config, theme.surface);
    let (rest, shell_radius) = match granularity {
        Granularity::Section => (Color::TRANSPARENT, shape.chip_radius()),
        Granularity::Chip => (surface, shape.chip_radius()),
    };
    let mut slots = Vec::with_capacity(3);
    for (entries, in_zone) in zones {
        let items = build_items(config, entries, registry, ctx, rest, shell_radius)?;
        let content: Vec<Box<dyn LayoutItem>> = if items.is_empty() {
            Vec::new()
        } else {
            match granularity {
                Granularity::Section => {
                    vec![unit(edge, *in_zone, shape.radius, spacing, surface, items)?]
                }
                // The shells already are the chips; place them directly.
                Granularity::Chip => items,
            }
        };
        // STRETCH ensures height is parent-driven by bar size, not content-driven.
        slots.push(zone(edge, *in_zone, spacing, AlignItems::STRETCH, content)?);
    }
    // No gap between the zones here: there are only ever three of them, so the only two joins it could space are the two the sides already hold open with a margin of their own (see [`zone`]). Both applying left twice the air at exactly the place a side is cut — a hole where the rest of the bar has one chip's worth.
    let style = axis(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0))
            .align_items(AlignItems::STRETCH),
        edge,
    );
    Ok(Box::new(Container::new(style, slots)?))
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
    Ok(Box::new(StyledContainer::new(
        axis(style, edge),
        move |_r| RectStyle::filled(fill, radius),
        items,
    )?))
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
fn zone(
    edge: Edge,
    in_zone: Zone,
    spacing: f32,
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
    let leading = matches!(in_zone, Zone::Start);
    let (style, clip) = if edge.is_horizontal() {
        let style = style.min_width(0.0);
        let style = if leading {
            style.margin_inline_end(spacing)
        } else {
            style.margin_inline_start(spacing)
        };
        (style, ClipAxis::Horizontal)
    } else {
        let style = style.min_height(0.0);
        let style = if leading {
            style.margin_block_end(spacing)
        } else {
            style.margin_block_start(spacing)
        };
        (style, ClipAxis::Vertical)
    };
    let zone = Container::new(axis(style, edge), items)?;
    Ok(Box::new(ClippedItem::along(Box::new(zone), clip)))
}

/// An invisible box wrapped around a module's own content to carry what its chip cannot: a wheel handler for a self-managed module (which has no [`module_shell`] to put one on), and the pointer tracking behind a hover popout. Both live here rather than on the chip so a self-managed module gets them on the same terms as any other; the wrapper shrink-wraps its child, so the rect it tracks is the chip's own.
///
/// `cross` is what the wrapper would otherwise silently change. A chip is a direct zone child under `AlignItems::STRETCH`, so it fills the bar's thickness; a wrapper that centred it instead would shrink every popout-bearing chip to its content. A self-managed module lays itself out and is centred, as it was before any wrapper existed.
///
/// It runs along the bar for the same reason [`zone`] does. A wrapper fixed to a row applies `cross` across the *screen's* vertical, so on a left or right bar it stretched each chip's height — which is already its content — and left its width free: every wrapped chip then sat at its own content width, ragged against the bar's inner edge, with the wide ones running off the screen. Thirteen chips carry a popout, so on a vertical bar that was most of them. `elastic` is the chip's own, forwarded. The wrapper is what the zone actually sizes, so a rigid one around an elastic chip is a chip that never gets the chance to give anything up — and both modules whose label elides carry a popout, which is to say both of them are wrapped.
fn chip_wrapper(
    content: Box<dyn LayoutItem>,
    module_id: &str,
    on_scroll: Option<fn(f32, f32)>,
    popout: bool,
    cross: AlignItems,
    edge: Edge,
    elastic: bool,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let style = LayoutStyle::new().align_items(cross);
    let style = if elastic {
        style.flex_shrink(1.0).min_width(0.0).min_height(0.0)
    } else {
        style.flex_shrink(0.0)
    };
    let style = axis(style, edge);
    let mut wrapper = StyledContainer::new(
        style,
        |_r| RectStyle::filled(Color::TRANSPARENT, 0.0),
        vec![content],
    )?;
    if let Some(on_scroll) = on_scroll {
        wrapper = wrapper.on_scroll(on_scroll);
    }
    if popout {
        // Tracked before the handler that reads it is attached, so the popout has a rect the first time the pointer arrives.
        let rect = track_layout(wrapper.layout_node())
            .expect("a container registers its rect")
            .read_only();
        let module = module_id.to_string();
        wrapper =
            wrapper.on_hover(move |entered| crate::popout::hover(&module, rect.get(), entered));
    }
    Ok(Box::new(wrapper))
}

/// The drag-to-open gesture for a chip, when it has a panel to open and the gesture is switched on.
///
/// Only a module whose click *opens a panel* gets one: dragging a volume chip has nothing to open, and arming a gesture that can only do nothing would still cancel the tap that does something.
fn drag_open_for(id: &str, def: Option<&ModuleDef>, edge: Edge) -> Option<DragOpen> {
    if !matches!(def?.click, Some(ModuleClick::Panel)) {
        return None;
    }
    let threshold = config::surface_env()?.config.panels.drag_threshold()?;
    Some(DragOpen {
        module: id.to_string(),
        edge,
        threshold,
    })
}

fn axis(style: LayoutStyle, edge: Edge) -> LayoutStyle {
    if edge.is_horizontal() {
        style.flex_row()
    } else {
        style.flex_column()
    }
}

/// Builds each entry's content and wraps it in its base container. The variant and accent come from the entry when it names them and from `[modules.<id>]` otherwise, which is what lets the same module sit on a bar twice looking different.
fn build_items(
    config: &Config,
    entries: &[ModuleEntry],
    registry: &ModuleRegistry,
    ctx: &ModuleCtx,
    rest: Color,
    radius: f32,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let mut items: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = &entry.id;
        let variant = config.entry_variant(entry);
        let accent = ctx.theme.accent_by_name(config.entry_accent_name(entry));
        // Set the foreground BEFORE building the content so `module_fg()` snapshots this module's color.
        set_module_fg(module_foreground(variant, accent, ctx.theme));
        let content = match registry.build(id, ctx) {
            Some(Ok(content)) => content,
            Some(Err(e)) => return Err(e),
            None => {
                tracing::warn!("unknown module id: {id}");
                continue;
            }
        };
        let def = registry.def(id);
        let popout = def.is_some_and(|d| d.popout) && config.popouts.enabled;
        if def.is_some_and(|d| d.self_managed) {
            // A self-managed module skips `module_shell` — it paints its own layout — so its wheel handler and popout tracking go on a bare wrapper with no padding, fill or hover state.
            let scroll = def.and_then(|d| d.scroll);
            if scroll.is_none() && !popout {
                items.push(content);
            } else {
                items.push(chip_wrapper(
                    content,
                    id,
                    scroll,
                    popout,
                    AlignItems::CENTER,
                    ctx.edge,
                    false,
                )?);
            }
            continue;
        }
        // Handed over bare: the chip shell dispatches every press with its own rect in scope, `Panel` and `Action` alike, so whatever this opens can hang off the chip without being told where it is.
        let on_press: Option<Box<dyn Fn()>> = match def.and_then(|d| d.click) {
            Some(ModuleClick::Panel) => {
                let id = id.clone();
                Some(Box::new(move || crate::panel::toggle_panel(&id)))
            }
            Some(ModuleClick::Action(action)) => Some(Box::new(action)),
            None => None,
        };
        let mut inner = Slots::new();
        inner.push(None, content);
        let chip = module_shell(
            ModuleShellProps {
                variant,
                rest,
                accent,
                radius,
                square: def.is_some_and(|d| d.icon),
                elastic: def.is_some_and(|d| d.elastic),
                on_press,
                on_scroll: def
                    .and_then(|d| d.scroll)
                    .map(|wheel| Box::new(wheel) as Box<dyn Fn(f32, f32)>),
                drag_open: drag_open_for(id, def, ctx.edge),
            },
            inner,
        )?;
        // Outside the chip rather than on it: the chip's own hover already swaps its paint, and stacking a second meaning onto that callback would tie the two together.
        items.push(if popout {
            chip_wrapper(
                chip,
                id,
                None,
                true,
                AlignItems::STRETCH,
                ctx.edge,
                def.is_some_and(|d| d.elastic),
            )?
        } else {
            chip
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{AvailableSpace, compute_layout, reset_layout_runtime, set_theme};
    use ui::module::ModuleDef;

    fn dummy(_ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(20.0).height(20.0),
            |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
            vec![],
        )?))
    }

    /// A chip the width of a window title, next to which `dummy` is the same chip after the title got shorter.
    fn wide(_ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
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
    fn centred(ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let item = dummy(ctx)?;
        let rect = track_layout(item.layout_node()).expect("a container registers its rect");
        CENTRED.with(|c| *c.borrow_mut() = Some(rect));
        Ok(item)
    }

    thread_local! {
        static YIELDED: std::cell::RefCell<Option<telar::RwSignal<telar::Rect>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// A module whose own content will give up width if anything above it lets the pressure through, and says how much it kept — the probe for whether a chip's elasticity survives the wrappers around it.
    fn stretchy(_ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
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

    fn registry() -> ModuleRegistry {
        let mut r = ModuleRegistry::new();
        r.register("dummy", ModuleDef::new(dummy));
        r.register("wide", ModuleDef::new(wide));
        r.register("centred", ModuleDef::new(centred));
        // Both eliding modules carry a hover popout, so the wrapper is part of the path under test.
        let mut elastic = ModuleDef::new(stretchy).elastic();
        elastic.popout = true;
        r.register("stretchy", elastic);
        let mut rigid = ModuleDef::new(stretchy);
        rigid.popout = true;
        r.register("rigid", rigid);
        r
    }

    thread_local! {
        static BUILT: std::cell::RefCell<Vec<&'static str>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn wanted(ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
        BUILT.with(|built| built.borrow_mut().push("wanted"));
        dummy(ctx)
    }

    fn unwanted(ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
        BUILT.with(|built| built.borrow_mut().push("unwanted"));
        dummy(ctx)
    }

    /// A module no zone names costs nothing at all.
    ///
    /// The registry holds every module the shell ships, so a build that walked *it* rather than the zones would construct a chip nobody asked for — and constructing a chip is what subscribes it, which turns an unused module into a running service. That is the residency half of the rule, reached through the one door that makes it invisible: the widget tree, where an extra chip is off-screen rather than wrong.
    #[test]
    fn a_module_no_zone_names_is_never_built() {
        let mut registry = ModuleRegistry::new();
        registry.register("wanted", ModuleDef::new(wanted));
        registry.register("unwanted", ModuleDef::new(unwanted));

        reset_layout_runtime();
        set_theme(NordTheme::new());
        BUILT.with(|built| built.borrow_mut().clear());
        let config: config::Config =
            toml::from_str("[bars.top]\nsize=34\ncenter=[\"wanted\"]\n").unwrap();

        build_bar(
            &config,
            Edge::Top,
            NordTheme::new().accent,
            &registry,
            NordTheme::new(),
        )
        .expect("the bar builds");

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
        inner.push(
            None,
            dummy(&ModuleCtx {
                theme: NordTheme::new(),
                accent: NordTheme::new().accent,
                bar_size: 32,
                edge: Edge::Top,
            })
            .unwrap(),
        );
        let chip = module_shell(
            ModuleShellProps {
                variant: config::Variant::Default,
                rest: Color::TRANSPARENT,
                accent: NordTheme::new().accent,
                radius: 8.0,
                square: true,
                on_press: Some(Box::new(move || sink.set(true))),
                ..Default::default()
            },
            inner,
        )
        .unwrap();
        let mut wrapped = chip_wrapper(
            chip,
            "volume",
            None,
            true,
            AlignItems::STRETCH,
            Edge::Top,
            false,
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
            let bar = build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new(),
            );
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
            let bar = build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new(),
            )
            .expect("the bar builds");
            compute_layout(
                bar.layout_node(),
                AvailableSpace::Definite(BAR),
                AvailableSpace::Definite(32.0),
            )
            .expect("the bar lays out");
            let rect = CENTRED
                .with(|c| c.borrow().clone())
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
            let ctx = ModuleCtx {
                theme: NordTheme::new(),
                accent: NordTheme::new().accent,
                bar_size: 32,
                edge,
            };
            let overrun = || -> Vec<Box<dyn LayoutItem>> {
                (0..4).map(|_| wide(&ctx).expect("a chip builds")).collect()
            };

            let start_zone =
                zone(edge, Zone::Start, SPACING, AlignItems::STRETCH, overrun()).unwrap();
            let start_rect = track_layout(start_zone.layout_node()).expect("the zone registers");
            let centre_chip = dummy(&ctx).expect("a chip builds");
            let centre_rect = track_layout(centre_chip.layout_node()).expect("the chip registers");
            let centre_zone = zone(
                edge,
                Zone::Center,
                SPACING,
                AlignItems::STRETCH,
                vec![centre_chip],
            )
            .unwrap();
            let end_zone = zone(edge, Zone::End, SPACING, AlignItems::STRETCH, overrun()).unwrap();
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
            let bar = build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new(),
            )
            .expect("the bar builds");
            compute_layout(
                bar.layout_node(),
                AvailableSpace::Definite(600.0),
                AvailableSpace::Definite(32.0),
            )
            .expect("the bar lays out");
            YIELDED
                .with(|y| y.borrow().clone())
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

        fn overrunner(_ctx: &ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError> {
            Ok(Box::new(StyledContainer::new(
                LayoutStyle::new().width(32.0).height(RUN).flex_shrink(0.0),
                |_r| RectStyle::filled(telar::Color::from_rgb_u8(255, 255, 255), 0.0),
                vec![],
            )?))
        }
        fn nudge(_dx: f32, _dy: f32) {}

        reset_layout_runtime();
        set_theme(NordTheme::new());
        let mut registry = ModuleRegistry::new();
        registry.register("dummy", ModuleDef::new(dummy));
        registry.register(
            "overrunner",
            ModuleDef::new(overrunner).self_managed().on_scroll(nudge),
        );
        let cfg: Config = toml::from_str(
            "[shape]\nmode=\"bar\"\nspacing=8\n\
             [bars.left]\nsize=32\nstart=[\"overrunner\"]\ncenter=[\"dummy\"]\n",
        )
        .unwrap();
        let bar = build_bar(
            &cfg,
            Edge::Left,
            NordTheme::new().accent,
            &registry,
            NordTheme::new(),
        )
        .expect("the bar builds");
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
            let bar = build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new(),
            )
            .expect("the bar builds");
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
                .with(|c| c.borrow().clone())
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
        let bar = build_bar(
            &cfg,
            Edge::Top,
            NordTheme::new().accent,
            &registry(),
            NordTheme::new(),
        )
        .expect("the bar builds");
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

        // The panel is the zone's only child, so it is the first thing drawn inside the zone's clip.
        let commands = tree.commands().to_vec();
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
            let bar = build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new(),
            );
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
        assert!(
            build_bar(
                &cfg,
                Edge::Top,
                NordTheme::new().accent,
                &registry(),
                NordTheme::new()
            )
            .is_ok()
        );
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
                build_bar(
                    &cfg,
                    Edge::Left,
                    NordTheme::new().accent,
                    &registry(),
                    NordTheme::new()
                )
                .is_ok(),
                "vertical {mode} builds"
            );
        }
    }

    /// Every chip fills the bar's thickness, whichever edge it is on.
    ///
    /// The regression: the wrapper a popout-bearing chip sits in was fixed to a row, so on a left or right bar it stretched the chip's height (already its content) and left the width free. Each wrapped chip then took its own content width, ragged against the bar's inner edge, and the wide ones ran off the screen. Building proves none of that — the wrapper builds happily either way — so this lays a real bar out and measures the chips.
    #[test]
    fn a_wrapped_chip_fills_the_bar_thickness_on_a_vertical_bar() {
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
                "clock",
                None,
                true,
                AlignItems::STRETCH,
                edge,
                false,
            )
            .unwrap();
            // The zone the bar puts a chip in: along the bar, stretching its children across it.
            let zone = zone(edge, Zone::Start, 0.0, AlignItems::STRETCH, vec![wrapped]).unwrap();
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
