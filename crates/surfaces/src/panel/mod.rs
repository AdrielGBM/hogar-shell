use crate::shell;
use crate::{drawer, float, popout};
use config::OpenMode;
use config::surface_env;
use ui::descriptor;
use ui::module::pressed_chip;

/// Toggles the panel for `module_id`, opening it as a drawer or a floating window per the module's `[modules.<id>] open` config (drawer by default). The single entry point every panel-opening chip calls, so the bar never branches on presentation and both forms share the same open/close bookkeeping — which lives in [`crate::shell`], not here, so a panel toggled from a chip, from IPC and from a keybind is one surface.
///
/// The environment comes from the bar surface in scope when a chip was clicked, and is derived from the running config when there is none (IPC, keybind); a drawer likewise hangs off the pressed chip's own rect ([`pressed_chip`]) when a chip opened it.
pub fn toggle_panel(module_id: &str) {
    if !descriptor::has_panel(module_id) {
        tracing::warn!("'{module_id}' has no panel to toggle");
        return;
    }
    let Some(env) = surface_env().or_else(|| shell::env_for_module(module_id)) else {
        tracing::warn!("no shell context yet; ignoring toggle of '{module_id}'");
        return;
    };
    if is_panel_open(module_id) {
        descriptor::closed(module_id);
    }
    // A panel and the hover card of the same chip say the same thing twice, overlapping, and the card is the one the user did not ask for: it opened by resting the pointer somewhere. So a panel takes the screen from it, and `popout::open` refuses to bring it back for as long as the panel is up.
    popout::close();
    let chip = pressed_chip();
    match env.config.open_mode_for(module_id) {
        OpenMode::Drawer => {
            shell::toggle_drawer(module_id, || drawer::open_drawer(&env, module_id, chip))
        }
        // A window rather than a glance: it stands until it is closed by hand, and takes the screen from the drawer it would otherwise open underneath ([`shell::close_drawer`]).
        OpenMode::Float => {
            shell::toggle_standing_window(module_id, || float::open_float(&env, module_id))
        }
    }
}

/// Opens `module_id`'s panel if it isn't already up; idempotent, unlike [`toggle_panel`].
pub fn open_panel(module_id: &str) {
    if !is_panel_open(module_id) {
        toggle_panel(module_id);
    }
}

/// Closes `module_id`'s panel; a no-op when it isn't open.
pub fn close_panel(module_id: &str) {
    descriptor::closed(module_id);
    shell::close(module_id);
}

pub fn is_panel_open(module_id: &str) -> bool {
    shell::drawer_is_open(module_id) || shell::window_is_open(module_id)
}
