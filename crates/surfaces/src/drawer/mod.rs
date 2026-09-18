use telar::{Rect, SurfaceToken};

use config::SurfaceEnv;
use config::{Align, DrawerConfig, Edge, Zone};
use ui::panel::PanelSurface;
use ui::placement::{OffChip, Placement};
use ui::scale::space;

/// The padding `drawer_panel.rsx` draws its module's panel inside. Named here because [`span_along`] has to add it back to reach the drawer's outside height, and two copies of that number is how a drawer on a left bar starts overrunning the screen the day the padding changes.
pub(crate) fn panel_pad() -> f32 {
    space::xl()
}

/// Where a drawer opened without a chip aligns along its bar — IPC, a keybind. A chip's own rect is the answer whenever there is one ([`ui::module::from_chip`]), and this is all the config can say in its place: the zone the module is configured into, and the centre for one it cannot place at all.
fn align_for(origin: Option<Zone>) -> Align {
    match origin {
        Some(Zone::Start) => Align::Start,
        Some(Zone::End) => Align::End,
        _ => Align::Center,
    }
}

/// How much room the drawer takes *along* the bar it hangs off, which is what keeps one opened from the last chip clear of the far end of the screen.
///
/// Along a horizontal bar that is the configured width. Along a vertical one it is the height, which `drawer_panel.rsx` builds as the configured body plus [`panel_pad`] on both sides — an upper bound for a drawer with less in it than the body allows, and the bound is the side to err on: overshooting slides a short drawer a little further up the bar, undershooting hangs a full one off the bottom of the screen.
fn span_along(edge: Edge, drawer: DrawerConfig) -> f32 {
    if edge.is_vertical() {
        drawer.max_height + panel_pad() * 2.0
    } else {
        drawer.width
    }
}

/// Where `module_id`'s drawer sits on its bar: hanging off the chip that opened it, the way that chip's hover popout does, and aligned to the module's configured zone only when nothing pressed it.
fn placement_for(env: &SurfaceEnv, module_id: &str, chip: Option<Rect>) -> Placement {
    let span = Some(span_along(env.edge, env.config.panels.drawer));
    let placement = Placement::off_chip(OffChip::Panel, env, chip, span)
        .keyboard(panel_wants_keyboard(module_id));
    match chip {
        Some(_) => placement,
        None => placement.align(align_for(env.config.zone_of(env.edge, module_id))),
    }
}

pub use ui::panel::{content_radius, panel_fill, panel_transition};
pub use ui::panels::{build as module_panel, wants_keyboard as panel_wants_keyboard};

/// Which module a drawer shows and how big it may be, set on the drawer surface's own scope so `drawer_panel.rsx` reads it via `inject` — scoped to the surface, not a global thread-local.
///
/// The corner radius is *not* here: it is the bar's, so it comes from the surface's own environment ([`content_radius`]) rather than from a copy every panel surface would have to remember to pass on.
#[derive(Clone)]
struct DrawerCtx {
    module: String,
    config: DrawerConfig,
}

fn ctx() -> Option<DrawerCtx> {
    util::state::context::<DrawerCtx>()
}

pub fn set_drawer_ctx(module: String, drawer: DrawerConfig) {
    util::state::set_context(DrawerCtx {
        module,
        config: drawer,
    });
}

/// The module whose panel the drawer being built shows; read by `drawer_panel.rsx`.
pub fn current_drawer_module() -> String {
    ctx().map(|ctx| ctx.module).unwrap_or_default()
}

/// The drawer size (width / max height) for the drawer being built; read by `drawer_panel.rsx`.
pub fn current_drawer_config() -> DrawerConfig {
    ctx().map(|ctx| ctx.config).unwrap_or_default()
}

/// Opens `module_id`'s drawer as a surface floating off the bar edge on the bar's own monitor, hanging off `chip` — the rect of the chip that was pressed, exactly as the hover popout of that chip does. A panel with no chip behind it (IPC, a keybind) has only the module's configured zone to align to instead. Either way the distance off the bar is the shared [`Config::panel_gap`](config::Config), so every panel keeps the same config-controlled gap. The surface/dismiss/slide-in come from the rsx surface host, the panel from `drawer_panel.rsx`. Toggle/close is the caller's job ([`crate::panel::toggle_panel`]) via the returned token.
pub(crate) fn open_drawer(env: &SurfaceEnv, module_id: &str, chip: Option<Rect>) -> SurfaceToken {
    let placement = placement_for(env, module_id, chip);
    let module = module_id.to_string();
    // What is captured is what the drawer *is* — which module it shows. Everything the config decides is resolved per build by the panel surface, so a rebuilt drawer is a drawer that followed the edit rather than one still drawing the config it opened under.
    PanelSurface::new(placement, move |env| {
        set_drawer_ctx(module.clone(), env.config.panels.drawer);
        drawer_panel::drawer_panel(
            drawer_panel::DrawerPanelProps::props().build(),
            telar::Children::default(),
        )
        .expect("drawer panel build failed")
    })
    .animated()
    .open()
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    use platform_wayland::SurfaceAlign;
    use std::sync::Arc;

    fn env(edge: Edge, config: &str) -> SurfaceEnv {
        let config: config::Config = toml::from_str(config).expect("config parses");
        SurfaceEnv {
            edge,
            bar_size: config.bars.get(edge).size,
            output: None,
            config: Arc::new(config),
        }
    }

    fn chip(x: f32, y: f32) -> Rect {
        Rect {
            x,
            y,
            width: 30.0,
            height: 30.0,
        }
    }

    /// **What a drawer is positioned by.** A click and a hover on one chip open two different surfaces, and both have to land in the same place — the chip's own rect decides, on every edge.
    ///
    /// The drawer used to align to the *zone* its chip sat in, which could say no more than "this end of the bar" and could not always say that: a module placed in two zones resolved to whichever the config search reached first, so the chip at the other end opened its drawer across the screen from itself.
    #[test]
    fn a_drawer_hangs_off_its_chip_where_that_chips_popout_would() {
        let toml = "[bars.top]\nstart=[\"notifications\"]\nend=[\"notifications\"]\n\
                    [bars.left]\nsize=34\nstart=[\"battery\"]\n\
                    [bars.bottom]\nsize=34\nstart=[\"clock\"]\n\
                    [bars.right]\nsize=34\nstart=[\"network\"]\n";
        for edge in Edge::ALL {
            let env = env(edge, toml);
            let chip = chip(500.0, 500.0);
            let span = span_along(edge, env.config.panels.drawer);
            let drawer = placement_for(&env, "notifications", Some(chip)).hosted_placement();
            let popout =
                Placement::off_chip(OffChip::Card, &env, Some(chip), Some(span)).layer_config();
            assert_eq!(
                drawer.margin, popout.margin,
                "{edge:?}: a click and a hover on one chip must open in the same place"
            );
            assert_eq!(
                drawer.align,
                SurfaceAlign::Start,
                "{edge:?}: the margin lines the panel up from the end it packs against, so it has to pack \
                 against the end the margin is measured from"
            );
        }
    }

    /// The same chip at the two ends of one bar opens its drawer at the two ends of the screen — which is the case a zone lookup got wrong, since `notifications` sits in `start` and `end` at once here.
    #[test]
    fn one_module_on_two_chips_opens_two_drawers_in_two_places() {
        let env = env(
            Edge::Top,
            "[bars.top]\nstart=[\"notifications\"]\nend=[\"notifications\"]\n",
        );
        assert_eq!(
            env.config.zone_of(Edge::Top, "notifications"),
            Some(Zone::Start)
        );

        let near = placement_for(&env, "notifications", Some(chip(40.0, 0.0))).hosted_placement();
        let far = placement_for(&env, "notifications", Some(chip(1500.0, 0.0))).hosted_placement();
        assert!(
            far.margin.3 > near.margin.3,
            "the chip at the far end of the bar opens its drawer further along it: {} vs {}",
            far.margin.3,
            near.margin.3
        );
    }

    /// With no chip — IPC, a keybind — the module's configured zone is all there is to go on, and the centre is the honest answer for a module the config cannot place at all.
    #[test]
    fn a_drawer_opened_without_a_chip_falls_back_to_the_zone_the_module_is_configured_in() {
        let env = env(
            Edge::Top,
            "[bars.top]\nstart=[\"clock\"]\nend=[\"network\"]\n",
        );
        let align = |module| placement_for(&env, module, None).hosted_placement().align;
        assert_eq!(align("clock"), SurfaceAlign::Start);
        assert_eq!(align("network"), SurfaceAlign::End);
        assert_eq!(
            align("notes"),
            SurfaceAlign::Center,
            "a module on no bar still opens somewhere sensible"
        );

        let gap = env.config.panel_gap(Edge::Top) as i32;
        assert_eq!(
            placement_for(&env, "clock", None).hosted_placement().margin,
            (gap, gap, gap, gap),
            "and floats off every edge by the shared panel gap, since nothing pins it along the bar"
        );
    }

    /// The height the drawer is kept clear of the far end by is the panel's *outside* height, so a drawer opened from the last chip of a vertical bar does not hang off the bottom of the screen by its own padding.
    #[test]
    fn the_span_a_drawer_is_kept_on_screen_by_is_its_own_size_on_that_axis() {
        let drawer = DrawerConfig::default();
        assert_eq!(span_along(Edge::Top, drawer), drawer.width);
        assert_eq!(
            span_along(Edge::Left, drawer),
            drawer.max_height + panel_pad() * 2.0,
            "`drawer_panel.rsx` sizes the body and pads around it, so the panel is taller than `max_height`"
        );
    }
}
