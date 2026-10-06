//! What is being built is drawn inside: scoped to the owner that provided it, so each area and each transient in a shared layer window answers for itself.

use std::sync::Arc;

use telar::{Color, Container, LayoutItem, LayoutStyle};

use config::{Config, ResolvedShape};

use crate::descriptor::Built;

#[derive(Clone)]
pub struct Chrome {
    pub config: Arc<Config>,
    pub shape: ResolvedShape,
    pub output: Option<String>,
}

impl Chrome {
    pub fn new(config: Arc<Config>, shape: ResolvedShape, output: Option<String>) -> Self {
        Self {
            config,
            shape,
            output,
        }
    }

    pub fn global(config: Arc<Config>, output: Option<String>) -> Self {
        let shape = config.shape_from(None, None, None, None);
        Self::new(config, shape, output)
    }

    pub fn provide(&self) {
        util::state::set_context(self.clone());
    }

    pub fn current() -> Option<Self> {
        util::state::context::<Self>()
    }
}

/// Says that whatever placed the widget being built paints its plate, as a layout's instance `style` asks for, so the widget's own card leaves its box bare instead of painting a second plate inside the first.
#[derive(Clone, Copy)]
pub struct Plated;

impl Plated {
    pub fn provide() {
        util::state::set_context(Plated);
    }

    pub fn here() -> bool {
        util::state::context::<Plated>().is_some()
    }
}

fn global_config() -> Option<Arc<Config>> {
    Chrome::current()
        .map(|chrome| chrome.config)
        .or_else(config::config)
}

pub fn panel_fill() -> Color {
    match global_config() {
        Some(config) => config.panel_fill(),
        None => telar::use_theme::<config::theme::NordTheme>().surface,
    }
}

pub fn content_radius() -> f32 {
    shape().radius
}

pub fn content_spacing() -> f32 {
    shape().spacing
}

pub fn card_gap() -> f32 {
    global_config().map_or_else(
        || config::theme::NordTheme::new().spacing,
        |config| config.card_gap(),
    )
}

fn shape() -> ResolvedShape {
    if let Some(chrome) = Chrome::current() {
        return chrome.shape;
    }
    match config::config() {
        Some(config) => config.shape_from(None, None, None, None),
        None => {
            let theme = config::theme::NordTheme::new();
            ResolvedShape {
                mode: config::Shape::default(),
                gap: 0,
                spacing: theme.spacing,
                radius: theme.radius,
            }
        }
    }
}

/// Module builds inside are already guarded; what reaches here is the shell's own layout failing, which costs that one thing rather than the shell.
pub fn or_empty(what: &str, built: Built) -> Box<dyn LayoutItem> {
    built.unwrap_or_else(|e| {
        tracing::error!("the {what} failed to build: {e}");
        Box::new(Container::new(LayoutStyle::new(), Vec::new()).expect("an empty container"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_shaped_by_the_chrome_in_scope() {
        let config = Arc::new(Config::starter());
        let mut shape = config.shape_from(None, None, None, None);
        shape.radius = 3.0;
        shape.spacing = 5.0;
        telar::reset_runtime();
        Chrome::new(Arc::clone(&config), shape, None).provide();
        assert_eq!(content_radius(), 3.0);
        assert_eq!(content_spacing(), 5.0);
    }
}
