//! The widgets surface: what the shell draws on the desktop itself — a clock face, an audio visualiser; kept off the wallpaper's surface since a changing layer forces a full redraw, which would rasterize the whole wallpaper photo every frame the visualiser moved.

use std::sync::Arc;

use telar::{
    App, Color, Component, Container, LayoutItem, LayoutStyle, WindowConfig, WindowRoot,
    reset_layout_runtime, set_theme,
};

use config::{Config, SurfaceEnv};
use ui::descriptor::Built;
use ui::host::{Host, InstanceId, Representation, Size, WidgetSize};
use ui::layout::{align_items, fill, justify};

/// Per-output widgets: a click-through surface over the free area of the screen, carrying whatever `[widgets]` asks for.
pub struct WidgetsApp {
    /// Read at every build rather than held: the surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
}

impl App for WidgetsApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        let content = ui::panel::or_empty("widgets", content(&config));
        Box::new(WindowRoot::wrapping(content).expect("widgets surface root"))
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

/// The desktop's widgets as this surface draws them, for [`crate::preview`]. The clock is forced on because it is the widget that has something to look at with no music playing and no wallpaper set.
pub(crate) fn preview() -> Built {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    config.widgets.clock.enabled = true;
    content(&Arc::new(config))
}

/// Every widget `[widgets]` asks for, stacked over the same area.
fn content(config: &Arc<Config>) -> Built {
    let layers = layers(config)
        .into_iter()
        .map(|(_, layer)| layer)
        .collect::<Result<Vec<Box<dyn LayoutItem>>, _>>()?;
    Ok(Box::new(Container::new(fill(), layers)?))
}

/// Each widget `[widgets]` switches on, by module id, placed on the surface.
pub fn layers(config: &Arc<Config>) -> Vec<(&'static str, Built)> {
    let mut layers = Vec::new();
    if config.widgets.clock.enabled {
        layers.push(("clock", clock_layer(config)));
    }
    if config.widgets.visualiser.enabled {
        layers.push(("visualiser", visualiser_layer(config)));
    }
    layers
}

/// `module`'s widget at `size` in a box laid out by `style`, under a host with no bound on its extent so the widget keeps its configured size.
fn widget(config: &Arc<Config>, module: &str, size: WidgetSize, style: LayoutStyle) -> Built {
    let env = SurfaceEnv::for_edge(Arc::clone(config), ui::panel::drawn_edge(config), None);
    let unbounded = Size {
        width: f32::INFINITY,
        height: f32::INFINITY,
    };
    let host = Host::on_surface(
        InstanceId::of_module(module),
        Representation::Widget(size),
        &env,
        unbounded,
    );
    ui::descriptor::place(module, &host, style)
}

/// The clock face (`[widgets.clock]`) at one of the nine positions.
fn clock_layer(config: &Arc<Config>) -> Built {
    let settings = &config.widgets.clock;
    let (vertical, horizontal) = settings.position.alignment();
    Ok(Box::new(Container::new(
        fill()
            .absolute_fill()
            .flex_row()
            .padding_all(settings.margin as f32)
            .align_items(align_items(vertical))
            .justify_content(justify(horizontal)),
        vec![widget(config, "clock", WidgetSize::M, LayoutStyle::new())?],
    )?))
}

/// The audio visualiser (`[widgets.visualiser]`), across the whole area minus its margin; the widget stands its row on the configured edge.
fn visualiser_layer(config: &Arc<Config>) -> Built {
    Ok(Box::new(Container::new(
        fill()
            .absolute_fill()
            .padding_all(config.widgets.visualiser.margin as f32),
        vec![widget(config, "visualiser", WidgetSize::L, fill())?],
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Align, ClockPlacement};

    /// With no module table installed there is nothing to place, and each layer is a placeholder rather than a surface that fails to exist.
    #[test]
    fn a_widget_with_no_installed_module_is_a_placeholder_layer() {
        telar::reset_layout_runtime();
        let mut config = Config::starter();
        telar::set_theme(config.resolve_theme());
        config.widgets.clock.enabled = true;
        config.widgets.visualiser.enabled = true;
        let layers = layers(&Arc::new(config.clone()));
        assert_eq!(layers.len(), 2);
        assert!(layers.iter().all(|(_, layer)| layer.is_ok()));
        let _ = WidgetsApp {
            config: Arc::new(config).into(),
        }
        .root();
    }

    #[test]
    fn the_nine_positions_map_onto_distinct_corners() {
        let corners: Vec<(Align, Align)> = ClockPlacement::ALL
            .into_iter()
            .map(|placement| placement.alignment())
            .collect();
        for (index, corner) in corners.iter().enumerate() {
            for other in &corners[index + 1..] {
                assert_ne!(corner, other, "two placements land in the same spot");
            }
        }
        assert_eq!(
            ClockPlacement::TopLeft.alignment(),
            (Align::Start, Align::Start),
            "the row is the vertical axis and the column the horizontal one"
        );
        assert_eq!(
            ClockPlacement::from_id("bottom-right"),
            Some(ClockPlacement::BottomRight)
        );
        assert_eq!(ClockPlacement::from_id("nowhere"), None);
    }
}
