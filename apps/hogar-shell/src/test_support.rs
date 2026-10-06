//! What the app's tests share to put a screen in front of the commands under test.

#![cfg(test)]

use std::sync::Arc;

use layout::Resolved;
use surfaces::reconcile::{self, Desktop};

pub const SCREEN_SIZE: (f32, f32) = (1920.0, 1080.0);

/// The screen `output` as a shell with the default config and nothing reserved would show it, `resolved` drawn on it.
pub fn desktop(output: &str, resolved: Resolved) -> Desktop {
    Desktop {
        output: Some(output.to_string()),
        config: Arc::new(config::Config::default()),
        resolved,
        reserved: Default::default(),
        size: SCREEN_SIZE,
    }
}

/// [`desktop`] as the only screen the shell shows.
pub fn publish(output: &str, resolved: Resolved) {
    reconcile::publish(&[desktop(output, resolved)]);
}

/// The screen `output` of `size` under `config`, with the edges `resolved` reserves taken off it as a running shell takes them.
pub fn measured(
    output: &str,
    config: Arc<config::Config>,
    resolved: Resolved,
    size: (f32, f32),
) -> Desktop {
    Desktop {
        reserved: surfaces::layer_window::Reserved::of(&resolved, &config),
        config,
        size,
        ..desktop(output, resolved)
    }
}
