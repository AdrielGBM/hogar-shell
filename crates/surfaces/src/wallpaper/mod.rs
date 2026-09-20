//! The background surface: the wallpaper and its transition.
//!
//! One surface per monitor, at the bottom of the background layer, painting one or more [`layout::ResolvedAreaKind::WallpaperRegion`] and [`layout::ResolvedAreaKind::Texture`] areas — the resolved layout model's account of what the background layer shows — cover-cropped over the theme's base colour. Nothing else: what is drawn *over* the desktop is [`crate::widgets`], on a surface of its own, so a widget that repaints with the music does not repaint a screen-sized photograph with it.
//!
//! The areas themselves are built by [`crate::area::build`], the one dispatcher every area kind goes through; this surface's only job is to say which areas exist. Until the background layer opens one window per output and hands this surface the areas a real layout resolved, [`WallpaperApp::background_areas`] stands in for that: it synthesises the one full-output region `[background]` describes.

use std::sync::Arc;

use telar::{
    App, Color, Component, Container, LayoutError, LayoutItem, WindowConfig, reset_layout_runtime,
    set_theme,
};

use config::{Config, WallpaperTransition};
use layout::{AreaId, ResolvedArea, ResolvedAreaKind};
use services::wallpaper;
use telar::WindowRoot;
use ui::layout::fill;

use crate::area::{self, Surround};

/// Per-output wallpaper: a full-screen background surface painting the current image (cover-cropped, aspect preserved) over the theme's base colour, or just the base colour when no image resolves.
pub struct WallpaperApp {
    /// Read at every build rather than held: the surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
    /// The monitor this wallpaper covers, so a `[background.monitors]` entry can target it.
    pub output: Option<String>,
}

impl App for WallpaperApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        Box::new(WindowRoot::wrapping(self.content(&config)).expect("wallpaper layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        // The base colour shows before/without an image (and behind any transparency), covering "theme base colour when there is no image".
        Some(self.config.get().resolve_theme().base)
    }

    fn window_config(&self) -> Option<WindowConfig> {
        // Opaque: a wallpaper replaces whatever the compositor draws behind it.
        None
    }
}

/// The desktop as this surface draws it — the configured image, cover-cropped and decoded for real — for [`crate::preview`].
pub(crate) fn preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    config.background.enabled = true;
    // The settled desktop, not the crossfade into it: a preview captures a handful of frames, and a 600ms transition is still halfway through when the last of them is taken.
    config.background.transition = WallpaperTransition::None;
    let config = Arc::new(config);
    let app = WallpaperApp {
        config: Arc::clone(&config).into(),
        output: None,
    };
    // No box of its own: the entry declares a `PreviewSurface`, which is what gives the image layers — absolutely positioned to fill their surface — something to fill.
    Ok(app.content(&config))
}

impl WallpaperApp {
    fn content(&self, config: &Arc<Config>) -> Box<dyn LayoutItem> {
        let surround = Surround {
            config,
            theme: config.resolve_theme(),
            output: self.output.as_deref(),
        };
        let mut layers: Vec<Box<dyn LayoutItem>> = Vec::new();
        for resolved in self.background_areas(config) {
            match area::build(&resolved, surround) {
                Some(Ok(item)) => layers.push(item),
                Some(Err(e)) => tracing::warn!("wallpaper area '{}': {e}", resolved.id),
                None => tracing::warn!(
                    "a `{}` area cannot be drawn on the background layer; skipped",
                    resolved.kind.name()
                ),
            }
        }
        Container::new(fill(), layers)
            .map(|container| Box::new(container) as Box<dyn LayoutItem>)
            .expect("wallpaper root container")
    }

    /// The background layer's areas for this output.
    ///
    /// Temporary, the same shape `bar::app::bar_area` gives the bar surface: exactly one full-output [`ResolvedAreaKind::WallpaperRegion`], synthesised from `[background]` by [`background_region`]. Goes once the background layer opens one window per output and hands this surface the areas a real layout resolved instead of building its own.
    fn background_areas(&self, config: &Config) -> Vec<ResolvedArea> {
        vec![background_region(config, self.output.as_deref())]
    }
}

/// The single full-output `WallpaperRegion` area today's `[background]` config describes.
///
/// Temporary: see [`WallpaperApp::background_areas`]. `source` is resolved once, through the same [`wallpaper::current_image`] resolution the live region re-applies on every runtime change (in [`area::wallpaper_region`]), so the area's declared picture and the one that ends up showing never disagree.
fn background_region(config: &Config, output: Option<&str>) -> ResolvedArea {
    let source = wallpaper::current_image(config, output)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    ResolvedArea {
        id: AreaId::new("background"),
        kind: ResolvedAreaKind::WallpaperRegion {
            rect: layout::Rect::default(),
            source,
            fit: layout::Fit::Cover,
            transition: match config.background.transition {
                WallpaperTransition::None => layout::Transition::None,
                WallpaperTransition::Fade => layout::Transition::Fade,
                WallpaperTransition::Wipe => layout::Transition::Slide,
            },
        },
        reserve: false,
        above_fullscreen: false,
        // A wallpaper belongs under the bars: measured against the whole output, not what they leave.
        within: layout::Within::Output,
        style: layout::AreaStyle::default(),
        visible: None,
        groups: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(config: Config) -> Box<dyn Component> {
        WallpaperApp {
            config: Arc::new(config).into(),
            output: None,
        }
        .root()
    }

    #[test]
    fn the_surface_builds_with_and_without_an_image_and_under_every_transition() {
        // The build is where a layout error would surface, and nothing else runs these closures.
        let _ = built(Config::starter());

        for transition in WallpaperTransition::ALL {
            let mut config = Config::starter();
            config.background.enabled = true;
            config.background.transition = transition;
            let _ = built(config);
        }
    }

    /// The wallpaper is asked for by a picture, and by nothing that is merely drawn over it.
    #[test]
    fn a_widget_is_not_a_reason_to_paint_the_desktop() {
        let mut config = Config::starter();
        config.widgets.clock.enabled = true;
        config.widgets.visualiser.enabled = true;
        assert!(!config.background.is_enabled());
    }
}
