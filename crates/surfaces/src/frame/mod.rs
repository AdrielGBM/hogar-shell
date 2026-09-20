use std::sync::Arc;

use telar::{
    App, Canvas, Color, Component, FillRule, LayoutStyle, PathStyle, RenderNode, ShapeStyle,
    SizeDimension, WindowConfig, reset_layout_runtime, set_theme,
};

use config::Edge;
use config::geometry::{InnerEdges, frame_path};
use telar::WindowRoot;

/// Per-output frame: full-screen transparent surface drawing a continuous even-odd ring around content.
///
/// **It claims nothing of its own, and does not need to.** A ring is one even-odd path, so an input claim made here would be the whole screen with no hole in the middle — a region is made of rectangles and the ring is not one. The strips it paints are exactly the ones the four bars occupy, which is why a bar paints no background under a frame, so each bar claims the strip the ring drew on it (`bar::strip_fill`) and the ring is covered edge to edge. What is left over is the four nubs the rounded inner corners put past the strips, each a fraction of the corner radius squared.
///
/// Once the shell draws one window per layer, the ring stops being a surface and becomes decoration inside the bars' own tree, which is where the claim already lives.
pub struct FrameApp {
    pub config: config::LiveConfig,
}

impl App for FrameApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        let theme = config.resolve_theme();
        set_theme(theme);
        // The frame is the bars' own edge continued around the screen, so it follows their opacity rather than a key of its own — a ring painted solid against translucent bars reads as a bug, not a choice.
        let base = theme.base.with_alpha(config.opacity());
        let inner_radius = config
            .shape
            .radius
            .map(|r| r as f32)
            .unwrap_or(theme.radius)
            + config.shape.gap as f32;
        let canvas = Canvas::new(
            LayoutStyle::new()
                .width(SizeDimension::Percent(1.0))
                .height(SizeDimension::Percent(1.0)),
            move |r| {
                let inner = InnerEdges {
                    left: config.edge_thickness(Edge::Left) as f32,
                    top: config.edge_thickness(Edge::Top) as f32,
                    right: config.edge_thickness(Edge::Right) as f32,
                    bottom: config.edge_thickness(Edge::Bottom) as f32,
                };
                let path = frame_path((r.width, r.height), inner, inner_radius);
                RenderNode::path(
                    Arc::new(path),
                    PathStyle::default()
                        .with_fill(base)
                        .with_fill_rule(FillRule::EvenOdd),
                )
            },
        )
        .expect("frame canvas build failed");
        Box::new(WindowRoot::wrapping(Box::new(canvas)).expect("frame layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }

    fn window_config(&self) -> Option<WindowConfig> {
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }
}
