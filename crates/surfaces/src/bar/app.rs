use std::sync::Arc;

use telar::{App, Color, Component, LayoutItem, WindowConfig, reset_layout_runtime, set_theme};

use config::{Edge, SurfaceEnv, set_surface_env};
use telar::WindowRoot;

use super::{AutoHide, build_bar};

pub struct BarApp {
    /// Read at every build rather than held: this surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
    pub edge: Edge,
    /// The monitor this bar surface lives on; threaded into `SurfaceEnv` so its panels open on the same screen.
    pub output: Option<String>,
}

impl App for BarApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        let theme = config.resolve_theme();
        set_theme(theme);
        // Apply the configured UI language on this surface's thread and subscribe it to live language switches.
        services::locale::attach(config.language());
        let bar_config = config.bars.get(self.edge);
        set_surface_env(SurfaceEnv {
            edge: self.edge,
            bar_size: bar_config.size,
            output: self.output.clone(),
            config: Arc::clone(&config),
        });
        let accent = theme.accent;
        let bar = ui::module::with_registry(|registry| {
            build_bar(&config, self.edge, accent, registry, theme)
        })
        .expect("bar build failed");
        // `persistent = false` moves the surface itself, so the wrapper goes here — around the whole bar, inside the surface root that drives it — rather than around any one zone.
        let bar: Box<dyn LayoutItem> = if config.bar_is_persistent(self.edge) {
            bar
        } else {
            Box::new(AutoHide::new(
                bar,
                &config,
                self.edge,
                config::bar_margin_for(&config, self.edge),
            ))
        };
        Box::new(WindowRoot::wrapping(bar).expect("bar layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        // Opaque bar fills entire surface; floating/sections/chips bar has gaps so surface must be transparent.
        let config = self.config.get();
        if config.bar_surface_opaque(self.edge) {
            Some(config.resolve_theme().base)
        } else {
            None
        }
    }

    fn window_config(&self) -> Option<WindowConfig> {
        if self.config.get().bar_surface_opaque(self.edge) {
            None
        } else {
            Some(WindowConfig {
                is_transparent: true,
                ..WindowConfig::default()
            })
        }
    }
}
