//! Blanking and waking the screens.
//!
//! The protocol first, and the compositor's own dispatcher only where there is no protocol. That ordering is worth more here than usual: `wlr-output-power-management` reports the mode back, so a blank is *verified*, while Hyprland's dispatcher answers nothing and has to be probed by trying a call shape and re-reading the state — which is what [`crate::hyprland::set_dpms`] does and why it exists.

/// Whether the screens can be blanked at all. `None` when no compositor could be reached, which is not the same answer as "it cannot".
pub fn supported() -> Option<bool> {
    match platform_wayland::output_power_supported() {
        Some(false) => Some(crate::hyprland::socket_dir().is_some()),
        answer => answer,
    }
}

/// Whether every screen is on, or `None` where nothing can say.
pub fn screens_on() -> Option<bool> {
    platform_wayland::output_power_on()
}

/// Switches every screen on or off, returning once something has confirmed it.
///
/// The protocol's own error is what a caller is told when both routes fail: it is the one that can distinguish an output that cannot be blanked from one another client is holding, where the dispatcher can only say that the state did not move.
pub fn set_screens(on: bool) -> Result<(), String> {
    let over_protocol = platform_wayland::set_output_power(on);
    if over_protocol.is_ok() {
        return over_protocol;
    }
    if let Some(dir) = crate::hyprland::socket_dir()
        && crate::hyprland::set_dpms(&dir, on)
    {
        return Ok(());
    }
    over_protocol
}
