//! Placing a surface against the bar chip that opened it.
//!
//! A bar is only its own thickness tall, so anything larger than a chip has to become a surface of its own. A module's drawer, the tray's context menus and the hover popouts all anchor the same way: the chip's laid-out rect decides where the surface sits along the bar, and the bar's edge decides which side it hangs off. No platform capability beyond an anchor and a margin is involved, which is why all three work on all four edges unchanged — and why a click and a hover on one chip open two surfaces in the same place.
//!
//! This is the arithmetic only. Which *shape* of surface it positions is [`Placement::off_chip`](crate::placement::Placement::off_chip), the one door all three go through.

use telar::Rect;

use config::Edge;
use config::SurfaceEnv;

/// Stand-in for an output the compositor has not reported a logical size for yet. Only ever feeds the clamp arithmetic, which needs a finite screen to clamp against.
const ASSUMED_OUTPUT: (f32, f32) = (1920.0, 1080.0);

/// The logical size of the monitor this bar is on, for keeping an anchored surface on screen.
fn output_size(env: &SurfaceEnv) -> (f32, f32) {
    let outputs = platform_wayland::outputs();
    let matched = match &env.output {
        Some(name) => outputs.iter().find(|o| o.name.as_deref() == Some(name)),
        None => outputs.first(),
    };
    matched
        .and_then(|o| o.logical_size)
        .map(|(w, h)| (w as f32, h as f32))
        .unwrap_or(ASSUMED_OUTPUT)
}

/// How far along the bar the surface starts, in the coordinates its margin is measured in.
///
/// A horizontal bar centres the surface on its chip, which is what a menu hanging under an icon should do. A vertical one lines the surface's top up with the chip's instead: `span` there is the surface's *height*, which is content-derived for a menu and therefore often unknown, and a centre that cannot be computed is worse than an edge that can.
///
/// One conversion stands between the chip and that margin, and leaving it out puts the surface off screen. The chip's rect is in the window's coordinates, which are the whole output's (TA-1); the margin is relative to the *usable* area, because this surface asks for no exclusive zone of its own and so is placed inside everyone else's — the perpendicular bars and, under `[shape] frame`, the ring. Clamping against the whole output instead is what pushed the popout on a bar's last chip past the far edge of the screen.
///
/// There used to be a second conversion, for a chip rect measured against the bar's own surface, which sat at its own gap off the screen edge. One window per layer took that surface away and with it the correction; the two terms happened to cancel wherever a bar's margin equalled what the edge beside it reserved, which is every arrangement with no gap (F-10.31).
fn along(env: &SurfaceEnv, chip: Rect, span: Option<f32>) -> f32 {
    let config = &env.config;
    let gap = config.panel_gap(env.edge) as f32;
    let span = span.unwrap_or_default();
    let (width, height) = output_size(env);
    let (start, extent, lead, trail) = if env.edge.is_vertical() {
        (chip.y, height, Edge::Top, Edge::Bottom)
    } else {
        let centred = chip.x + chip.width / 2.0 - span / 2.0;
        (centred, width, Edge::Left, Edge::Right)
    };
    let leading = config.edge_reserved(lead) as f32;
    let usable = (extent - leading - config.edge_reserved(trail) as f32).max(0.0);
    let far = (usable - span - gap).max(gap);
    (start - leading).clamp(gap, far)
}

/// The margin `(top, right, bottom, left)` for a surface hanging `off_bar` px off the bar and lined up with `chip` along it. `span` is the surface's own extent along the bar, when it is known — without it the surface can still be positioned, only not kept clear of the far end.
pub fn chip_margin(
    env: &SurfaceEnv,
    chip: Rect,
    off_bar: f32,
    span: Option<f32>,
) -> (i32, i32, i32, i32) {
    let along = along(env, chip, span) as i32;
    let off_bar = off_bar as i32;
    match env.edge {
        Edge::Top => (off_bar, 0, 0, along),
        Edge::Bottom => (0, 0, off_bar, along),
        Edge::Left => (along, 0, 0, off_bar),
        Edge::Right => (along, off_bar, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const SPAN: f32 = 260.0;

    fn env(edge: Edge) -> SurfaceEnv {
        SurfaceEnv::for_edge(Arc::new(config::Config::starter()), edge, None)
    }

    fn chip(x: f32, y: f32) -> Rect {
        Rect {
            x,
            y,
            width: 30.0,
            height: 30.0,
        }
    }

    #[test]
    fn a_surface_under_a_top_bar_clears_the_bar_and_centres_on_its_chip() {
        let env = env(Edge::Top);
        let gap = env.config.panel_gap(Edge::Top) as i32;
        let (top, _, _, left) = chip_margin(&env, chip(500.0, 0.0), gap as f32, Some(SPAN));
        assert_eq!(
            top, gap,
            "the surface hangs the standard panel gap off the bar"
        );
        assert_eq!(
            left,
            (500.0 + 15.0 - SPAN / 2.0) as i32,
            "and centres on the chip"
        );
    }

    #[test]
    fn a_chip_at_the_end_of_the_bar_keeps_its_surface_on_screen() {
        let env = env(Edge::Top);
        let (width, _) = output_size(&env);
        let (_, _, _, left) = chip_margin(&env, chip(width - 20.0, 0.0), 8.0, Some(SPAN));
        assert!(
            (left as f32) + SPAN <= width,
            "the surface was pushed off the right edge: left {left} + {SPAN} > {width}"
        );
    }

    /// The margin is measured from the usable area, not from the screen, because the surface takes no exclusive zone and the compositor places it inside everyone else's. Under `[shape] frame` that area is inset on every edge at once, so a clamp against the output overshoots by the whole ring — which is exactly how the popout on the last chip of a top bar ended up hanging past the right edge of the screen.
    #[test]
    fn a_popout_stays_inside_the_frame_ring_not_merely_inside_the_screen() {
        let framed = SurfaceEnv::for_edge(
            Arc::new(
                toml::from_str(
                    "[shape]\nframe=true\ngap=0\ninactive_size=8\n\
                     [bars.top]\nsize=32\nend=[\"network\"]\n\
                     [bars.left]\nsize=32\nstart=[\"workspaces\"]\n",
                )
                .unwrap(),
            ),
            Edge::Top,
            None,
        );
        let (width, _) = output_size(&framed);
        let inner_left = framed.config.edge_reserved(Edge::Left) as f32;
        let inner_right = width - framed.config.edge_reserved(Edge::Right) as f32;
        assert!(inner_left > 0.0, "the left bar reserves a strip to clear");

        let (_, _, _, left) = chip_margin(&framed, chip(width - 40.0, 0.0), 8.0, Some(SPAN));
        // The margin is relative to the usable area's left edge, so this is where the surface actually lands.
        let placed = inner_left + left as f32;
        assert!(
            placed + SPAN <= inner_right,
            "a popout on the last chip runs past the ring: {placed} + {SPAN} > {inner_right}"
        );

        // And the near end is measured in the same space: a popout on the first chip clears the ring rather than starting under it.
        let (_, _, _, near) = chip_margin(&framed, chip(inner_left + 4.0, 0.0), 8.0, Some(SPAN));
        assert!(
            near >= 0,
            "a popout at the start of the bar sits inside the usable area, not before it"
        );
    }

    #[test]
    fn a_bottom_bar_puts_its_surface_above_itself() {
        let env = env(Edge::Bottom);
        let (top, _, bottom, _) = chip_margin(&env, chip(100.0, 0.0), 12.0, Some(SPAN));
        assert_eq!(bottom, 12);
        assert_eq!(top, 0, "an upward surface is pinned from the bottom only");
    }

    /// A chip's rect is in the window's coordinates, which are the whole output's, and the margin is measured against the usable area — so lining the two up is a subtraction of whatever the edge above reserved.
    ///
    /// The starter config has a 34 px top bar, and a left bar starts below it, so a chip 300 px down the screen is 266 px down the area a surface is placed in. The two used to cancel: the chip rect was measured against the bar's own surface, which the shell then offset by the same 34 (F-10.31).
    #[test]
    fn a_vertical_bar_puts_its_surface_beside_itself() {
        let env = env(Edge::Left);
        let above = env.config.edge_reserved(Edge::Top) as i32;
        let (top, _, _, left) = chip_margin(&env, chip(0.0, 300.0), 12.0, None);
        assert_eq!(left, 12, "the surface clears the bar's width");
        assert_eq!(
            top,
            300 - above,
            "and lines up with the chip it belongs to, in the box it is placed in"
        );
    }

    #[test]
    fn a_chip_near_the_bottom_of_a_vertical_bar_keeps_a_known_span_on_screen() {
        let env = env(Edge::Left);
        let (_, height) = output_size(&env);
        let (top, _, _, _) = chip_margin(&env, chip(0.0, height - 40.0), 12.0, Some(SPAN));
        assert!(
            (top as f32) + SPAN <= height,
            "a surface whose height is known is kept clear of the bottom edge"
        );
    }
}
