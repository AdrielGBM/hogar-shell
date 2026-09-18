//! Which module has a panel, what builds it, and whether opening it takes the keyboard.
//!
//! The other half of the composition root, beside [`crate::core::registry`]: the drawer, the float and the settings preview all present *a module's panel*, and this is the one place that says which module that is. Keeping it here rather than beside the drawer is what lets a surface be built without the modules it shows.
//!
//! `session` takes the keyboard for the second reason the mode exists: its tiles are a list, and a menu whose most destructive entries are two presses away is exactly the one a user wants to reach without moving their hand to the mouse.

use platform_wayland::KeyboardMode;
use ui::panels::PanelRegistry;

pub fn default_panels() -> PanelRegistry {
    // The clock is the fallback because it is the one panel that needs nothing to be configured to draw.
    let mut panels = PanelRegistry::new(|| {
        ::modules::clock::clock_panel::clock_panel(
            ::modules::clock::clock_panel::ClockPanelProps::props().build(),
            telar::Children::default(),
        )
    });
    let display = KeyboardMode::None;
    let typing = KeyboardMode::OnDemand;
    panels.register(
        "clock",
        || {
            ::modules::clock::clock_panel::clock_panel(
                ::modules::clock::clock_panel::ClockPanelProps::props().build(),
                telar::Children::default(),
            )
        },
        display,
    );
    panels.register("dashboard", modules::dashboard::dashboard_panel, display);
    panels.register(
        "battery",
        || {
            ::modules::battery::battery_panel::battery_panel(
                ::modules::battery::battery_panel::BatteryPanelProps::props().build(),
                telar::Children::default(),
            )
        },
        display,
    );
    panels.register("bluetooth", modules::bluetooth::bluetooth_panel, display);
    panels.register("network", modules::network::network_panel, display);
    panels.register("mixer", modules::mixer::mixer_panel, display);
    panels.register("notifications", modules::notifications::bell_panel, display);
    panels.register("notes", modules::notes::notes_panel, typing);
    panels.register("settings", settings::panel::settings_panel, typing);
    panels.register("utilities", modules::utilities::utilities_panel, display);
    panels.register("windowinfo", modules::windowinfo::window_panel, display);
    panels.register("session", modules::session::session_panel, typing);
    panels.register("logo", modules::session::session_panel, typing);
    // The settings window keeps its Revert snapshot for as long as it is open, and must not keep it across a user closing it.
    panels.on_close("settings", settings::panel::forget_panel_state);
    panels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wants(panels: &PanelRegistry, module: &str) -> bool {
        panels.def(module).expect("a registered panel").keyboard != KeyboardMode::None
    }

    #[test]
    fn only_panels_that_read_keys_ask_for_the_keyboard() {
        let panels = default_panels();
        assert!(wants(&panels, "notes"), "notes are edited in place");
        assert!(wants(&panels, "settings"), "settings has text fields");
        for navigable in ["session", "logo"] {
            assert!(
                wants(&panels, navigable),
                "the session tiles are arrow-navigable, which is the other reason to want the keyboard — and \
                 '{navigable}' shows them"
            );
        }
        for display_only in [
            "clock",
            "dashboard",
            "battery",
            "bluetooth",
            "network",
            "notifications",
        ] {
            assert!(
                !wants(&panels, display_only),
                "'{display_only}' only shows readings; taking keyboard focus from the window would make the \
                 compositor re-focus it on close, moving the viewport under a focus-following layout"
            );
        }
    }
}
